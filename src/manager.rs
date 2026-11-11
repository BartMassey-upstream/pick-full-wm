//! ICCCM manager-selection ownership and cooperative window-manager replacement.

use std::collections::VecDeque;
use std::error::Error;
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ButtonIndex, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt,
    CreateWindowAux, EventMask, ModMask, PropMode, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent,
    SelectionRequestEvent, Timestamp, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;
use x11rb::{CURRENT_TIME, NONE};

use crate::atoms::Atoms;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

const REPLACE_TIMEOUT: Duration = Duration::from_secs(5);

/// Owns WM_Sn and the EWMH supporting window for one X screen.
pub struct Manager {
    pub selection: Atom,
    pub window: Window,
    timestamp: Timestamp,
    pub pending_events: VecDeque<Event>,
}

impl Manager {
    /// Claim the screen only after the previous manager releases its resources.
    /// Failure is non-destructive: no old window or client is killed on timeout.
    pub fn acquire(
        conn: &RustConnection,
        screen: usize,
        atoms: &Atoms,
        replace: bool,
    ) -> Result<Self> {
        let root = conn.setup().roots[screen].root;
        let selection = conn
            .intern_atom(false, format!("WM_S{screen}").as_bytes())?
            .reply()?
            .atom;
        let window = conn.generate_id()?;
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?
        .check()?;

        // Serialize the ownership check and claim so two ordinary startups cannot steal
        // the selection from one another. Never hold the server while waiting for a WM.
        conn.grab_server()?.check()?;
        let claim = (|| -> Result<(Window, Timestamp)> {
            let previous = conn.get_selection_owner(selection)?.reply()?.owner;
            if previous != NONE && !replace {
                return Err(
                    "another window manager is running; use --replace to request a handoff".into(),
                );
            }
            if previous != NONE {
                conn.change_window_attributes(
                    previous,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
                )?
                .check()?;
            }
            let timestamp = server_timestamp(conn, window, atoms._PICK_FULL_WM_TIMESTAMP)?;
            conn.set_selection_owner(window, selection, timestamp)?
                .check()?;
            if conn.get_selection_owner(selection)?.reply()?.owner != window {
                return Err("could not acquire the window-manager selection".into());
            }
            Ok((previous, timestamp))
        })();
        conn.ungrab_server()?.check()?;
        let (previous, timestamp) = claim?;
        let mut manager = Self {
            selection,
            window,
            timestamp,
            pending_events: VecDeque::new(),
        };

        if previous != NONE {
            let deadline = Instant::now() + REPLACE_TIMEOUT;
            loop {
                if conn.get_window_attributes(previous)?.reply().is_err() {
                    break;
                }
                if conn.get_selection_owner(selection)?.reply()?.owner != window {
                    return Err("another manager superseded this replacement attempt".into());
                }
                if Instant::now() >= deadline {
                    return Err("the previous window manager did not release its owner window within 5 seconds; leaving its clients untouched".into());
                }
                while let Some(event) = conn.poll_for_event()? {
                    manager.pending_events.push_back(event);
                }
                thread::sleep(Duration::from_millis(10));
            }
        }

        conn.change_window_attributes(root, &ChangeWindowAttributesAux::new()
            .event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY))?
            .check().map_err(|error| format!("cannot acquire root redirection; another window manager may still control this screen: {error}"))?;
        if conn.get_selection_owner(selection)?.reply()?.owner != window {
            return Err("lost the window-manager selection during startup".into());
        }
        let announcement = ClientMessageEvent::new(
            32,
            root,
            atoms.MANAGER,
            [timestamp, selection, window, 0, 0],
        );
        conn.send_event(false, root, EventMask::STRUCTURE_NOTIFY, announcement)?
            .check()?;
        Ok(manager)
    }

    /// Answer the required selection targets and ICCCM version query.
    pub fn selection_request(
        &self,
        conn: &RustConnection,
        atoms: &Atoms,
        request: SelectionRequestEvent,
    ) -> Result<()> {
        let mut property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let current =
            request.time == CURRENT_TIME || (request.time.wrapping_sub(self.timestamp) as i32) >= 0;
        let valid = request.selection == self.selection && request.owner == self.window && current;
        let converted = valid
            && if request.target == atoms.MULTIPLE {
                request.property != NONE
                    && self.convert_multiple(conn, atoms, request.requestor, property)
            } else {
                self.convert(conn, atoms, request.requestor, request.target, property)
            };
        if !converted {
            property = NONE;
        }
        let reply = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property,
        };
        // The requestor may have closed its window while the conversion was in flight.
        let _ = conn
            .send_event(false, request.requestor, EventMask::NO_EVENT, reply)?
            .check();
        Ok(())
    }

    fn convert(
        &self,
        conn: &RustConnection,
        atoms: &Atoms,
        requestor: Window,
        target: Atom,
        property: Atom,
    ) -> bool {
        if property == NONE {
            return false;
        }
        let (type_, data) = if target == atoms.TARGETS {
            (
                AtomEnum::ATOM,
                vec![
                    atoms.TARGETS,
                    atoms.MULTIPLE,
                    atoms.TIMESTAMP,
                    atoms.VERSION,
                ],
            )
        } else if target == atoms.TIMESTAMP {
            (AtomEnum::INTEGER, vec![self.timestamp])
        } else if target == atoms.VERSION {
            (AtomEnum::INTEGER, vec![2, 0])
        } else {
            return false;
        };
        conn.change_property32(PropMode::REPLACE, requestor, property, type_, &data)
            .ok()
            .is_some_and(|cookie| cookie.check().is_ok())
    }

    fn convert_multiple(
        &self,
        conn: &RustConnection,
        atoms: &Atoms,
        requestor: Window,
        property: Atom,
    ) -> bool {
        let Ok(cookie) = conn.get_property(false, requestor, property, atoms.ATOM_PAIR, 0, 4096)
        else {
            return false;
        };
        let Ok(reply) = cookie.reply() else {
            return false;
        };
        if reply.type_ != atoms.ATOM_PAIR || reply.bytes_after != 0 {
            return false;
        }
        let Some(values) = reply.value32() else {
            return false;
        };
        let mut pairs: Vec<_> = values.collect();
        if pairs.len() % 2 != 0 {
            return false;
        }
        for pair in pairs.as_chunks_mut::<2>().0 {
            if pair[1] == property || !self.convert(conn, atoms, requestor, pair[0], pair[1]) {
                pair[1] = NONE;
            }
        }
        conn.change_property32(
            PropMode::REPLACE,
            requestor,
            property,
            atoms.ATOM_PAIR,
            &pairs,
        )
        .ok()
        .is_some_and(|cookie| cookie.check().is_ok())
    }

    /// Release redirection and grabs before destroying the ownership window.
    pub fn release(&self, conn: &RustConnection, root: Window) -> Result<()> {
        conn.ungrab_key(0u8, root, ModMask::ANY)?.check()?;
        conn.ungrab_button(ButtonIndex::ANY, root, ModMask::ANY)?
            .check()?;
        conn.ungrab_pointer(CURRENT_TIME)?.check()?;
        conn.ungrab_keyboard(CURRENT_TIME)?.check()?;
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
        )?
        .check()?;
        conn.destroy_window(self.window)?.check()?;
        conn.flush()?;
        Ok(())
    }
}

/// Ask the server for a timestamp instead of claiming a selection with CurrentTime.
fn server_timestamp(conn: &RustConnection, window: Window, property: Atom) -> Result<Timestamp> {
    conn.change_property32(PropMode::APPEND, window, property, AtomEnum::INTEGER, &[])?
        .check()?;
    loop {
        if let Event::PropertyNotify(event) = conn.wait_for_event()?
            && event.window == window
            && event.atom == property
        {
            return Ok(event.time);
        }
    }
}

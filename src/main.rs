use std::process::ExitCode;

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::ConnectionExt;

mod atoms;
mod binaries;
mod calc;
mod events;
mod ewmh;
mod keymap;
mod manager;
mod panels;
mod safety;
mod settings;
mod setup;
mod state;
mod windows;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pick-full-wm: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> manager::Result<()> {
    let mut replace = false;
    let mut session_services = true;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--replace" => replace = true,
            "--no-session-services" => session_services = false,
            "-h" | "--help" => {
                println!(
                    "Usage: pick-full-wm [--replace] [--no-session-services]\n\n  --replace              Request a cooperative handoff from the current X11 window manager\n  --no-session-services  Leave D-Bus/systemd environments and desktop portal services alone"
                );
                return Ok(());
            }
            "--version" => {
                println!("pick-full-wm {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => return Err(format!("unknown argument {argument:?}; use --help for usage").into()),
        }
    }
    let mut state = state::State::init(replace)?;
    setup::internal::logging(&state);
    if session_services {
        setup::internal::dbus_init();
    }
    setup::internal::windows(&mut state);
    setup::ewmh::init(&mut state);
    panels::refresh(&mut state);
    setup::internal::adopt_windows(&mut state);
    setup::internal::custom_startups(&mut state);
    setup::input::mouse(&mut state);
    setup::input::keys(&mut state);
    setup::internal::startups(&mut state);
    log::info!("Finished with initialization.");

    loop {
        let event = match state.manager.pending_events.pop_front() {
            Some(event) => event,
            None => state.conn.wait_for_event()?,
        };
        // ignored events (mostly notifications of our own moves/maps) skip the per-event work
        if !matches!(
            event,
            Event::MapRequest(_)
                | Event::SelectionClear(_)
                | Event::SelectionRequest(_)
                | Event::MapNotify(_)
                | Event::PropertyNotify(_)
                | Event::ButtonPress(_)
                | Event::KeyRelease(_)
                | Event::MappingNotify(_)
                | Event::ConfigureRequest(_)
                | Event::UnmapNotify(_)
                | Event::DestroyNotify(_)
                | Event::ClientMessage(_)
        ) {
            continue;
        }
        calc::update_current_monitor(&mut state);
        match event {
            Event::SelectionClear(e)
                if e.selection == state.manager.selection && e.owner == state.manager.window =>
            {
                if state.conn.get_selection_owner(e.selection)?.reply()?.owner
                    != state.manager.window
                {
                    windows::core::prepare_handoff(&state)?;
                    state.manager.release(&state.conn, state.root)?;
                    return Ok(());
                }
            }
            Event::SelectionRequest(e) => {
                state
                    .manager
                    .selection_request(&state.conn, &state.atoms, e)?;
            }
            Event::MapNotify(_) => panels::refresh(&mut state),
            Event::PropertyNotify(e) => events::window::property(&mut state, e),
            Event::MapRequest(e) => {
                events::window::map_request(&mut state, e);
            }
            Event::ButtonPress(e) => {
                events::input::button(&mut state, e);
            }
            Event::KeyRelease(e) => {
                events::input::key(&mut state, e);
            }
            Event::MappingNotify(e) => {
                events::input::mapping(&mut state, e);
            }
            Event::ConfigureRequest(e) => {
                events::window::configure_request(&mut state, e);
            }
            Event::UnmapNotify(e) => {
                events::window::unmap(&mut state, e);
                panels::refresh(&mut state);
            }
            Event::DestroyNotify(e) => {
                events::window::destroy(&mut state, e);
                panels::refresh(&mut state);
            }
            Event::ClientMessage(e) => {
                events::client::message(&mut state, e);
            }
            _ => {}
        }
        panels::restack(&state);
        crate::windows::audits::full(&mut state);
    }
}

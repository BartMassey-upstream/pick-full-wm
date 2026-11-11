//! End-to-end protocol tests. Each test owns a private Xvfb display and temporary home.

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt, CreateWindowAux,
    EventMask, KEY_RELEASE_EVENT, KeyButMask, KeyReleaseEvent, PropMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

const SETTINGS: &str = r#"
[applications]
startups = []
excluded = []
[layout]
main_size = ["80%x100%", "80%x100%"]
top_left = ["0,0", "0,0"]
conditional_full = true
new_to_main = false
close_box = false
monitor_box = false
[bindings]
swaps = ["j", "k"]
close_main = "q"
fullscreen = "f"
help = "h"
key_hints = false
workspaces = ["1", "2"]
monitor = "tab"
"#;

static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

struct Session {
    conn: RustConnection,
    root: Window,
    display: String,
    directory: PathBuf,
    server: Child,
    managers: Vec<Child>,
    screen: usize,
}

impl Session {
    fn new(settings: &str) -> Self {
        Self::on_screen(settings, 0)
    }

    fn on_screen(settings: &str, screen: usize) -> Self {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/x11-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT_TEST.fetch_add(1, Ordering::Relaxed)
            ));
        let config = directory.join(".config/pick-full-wm");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("settings.toml"), settings).unwrap();
        // Even a regression in service opt-out must never touch the host session.
        let stubs = directory.join("service-stubs");
        fs::create_dir_all(&stubs).unwrap();
        for name in ["systemctl", "dbus-update-activation-environment"] {
            let path = stubs.join(name);
            fs::write(
                &path,
                "#!/bin/sh\nprintf '%s\\n' \"$0\" >> \"$PICK_FULL_TEST_SERVICE_CALLS\"\nexit 1\n",
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let display_file = directory.join("display");
        let mut server = Command::new("Xvfb")
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                "1280x720x24",
                "-screen",
                "1",
                "1280x720x24",
                "-nolisten",
                "tcp",
            ])
            .stdout(File::create(&display_file).unwrap())
            .stderr(File::create(directory.join("xvfb.log")).unwrap())
            .spawn()
            .expect("Install Xvfb to run the ignored X11 tests");
        let deadline = Instant::now() + Duration::from_secs(5);
        let display = loop {
            let value = fs::read_to_string(&display_file).unwrap();
            if !value.trim().is_empty() {
                break format!(":{}.{screen}", value.trim());
            }
            if server.try_wait().unwrap().is_some() || Instant::now() >= deadline {
                let _ = server.kill();
                let _ = server.wait();
                panic!("Xvfb failed to start; see {}", directory.display());
            }
            thread::sleep(Duration::from_millis(10));
        };
        let (conn, screen) = x11rb::connect(Some(&display)).unwrap();
        let root = conn.setup().roots[screen].root;
        Self {
            conn,
            root,
            display,
            directory,
            server,
            managers: Vec::new(),
            screen,
        }
    }

    fn spawn_wm(&mut self, args: &[&str]) -> usize {
        self.spawn_wm_with_services(args, false)
    }

    fn spawn_wm_with_services(&mut self, args: &[&str], services: bool) -> usize {
        let index = self.managers.len();
        let log = File::create(self.directory.join(format!("wm-{index}.log"))).unwrap();
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(self.directory.join("service-stubs"))
                .chain(std::env::split_paths(&inherited_path)),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_pick-full-wm"));
        if !services {
            command.arg("--no-session-services");
        }
        let child = command
            .args(args)
            .current_dir(&self.directory)
            .env("DISPLAY", &self.display)
            // Isolate the child's real configuration lookup from the developer's desktop.
            .env("HOME", &self.directory)
            .env("DBUS_SESSION_BUS_ADDRESS", "disabled-for-test")
            .env("PATH", path)
            .env(
                "PICK_FULL_TEST_SERVICE_CALLS",
                self.directory.join("service-calls"),
            )
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        self.managers.push(child);
        index
    }

    fn wait(&self, description: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for {description}; logs: {}",
                self.directory.display()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn atom(&self, name: &str) -> Atom {
        self.conn
            .intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    }

    fn values(&self, window: Window, name: &str) -> Vec<u32> {
        self.conn
            .get_property(false, window, self.atom(name), AtomEnum::ANY, 0, 1024)
            .unwrap()
            .reply()
            .unwrap()
            .value32()
            .map(|values| values.collect())
            .unwrap_or_default()
    }

    fn set(&self, window: Window, name: &str, type_: AtomEnum, values: &[u32]) {
        self.conn
            .change_property32(PropMode::REPLACE, window, self.atom(name), type_, values)
            .unwrap()
            .check()
            .unwrap();
    }

    fn window(&self, kind: Option<&str>, x: i16, y: i16, width: u16, height: u16) -> Window {
        let window = self.conn.generate_id().unwrap();
        self.conn
            .create_window(
                0,
                window,
                self.root,
                x,
                y,
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new(),
            )
            .unwrap()
            .check()
            .unwrap();
        if let Some(kind) = kind {
            self.set(
                window,
                "_NET_WM_WINDOW_TYPE",
                AtomEnum::ATOM,
                &[self.atom(kind)],
            );
        }
        window
    }

    fn map(&self, window: Window) {
        self.conn.map_window(window).unwrap().check().unwrap();
    }

    fn geometry(&self, window: Window) -> (i16, i16, u16, u16) {
        let geometry = self.conn.get_geometry(window).unwrap().reply().unwrap();
        (geometry.x, geometry.y, geometry.width, geometry.height)
    }

    fn expect_geometry(&self, window: Window, expected: (i16, i16, u16, u16)) {
        self.wait(&format!("window {window} geometry {expected:?}"), || {
            self.geometry(window) == expected
        });
    }

    fn expect_workarea(&self, expected: [u32; 4]) {
        let values: Vec<u32> = expected.into_iter().cycle().take(8).collect();
        self.wait("workarea", || {
            self.values(self.root, "_NET_WORKAREA") == values
        });
    }

    fn super_key(&self, keysym: u32) {
        let setup = self.conn.setup();
        let mapping = self
            .conn
            .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
            .unwrap()
            .reply()
            .unwrap();
        let index = mapping
            .keysyms
            .chunks(mapping.keysyms_per_keycode as usize)
            .position(|symbols| symbols.contains(&keysym))
            .unwrap();
        let event = KeyReleaseEvent {
            response_type: KEY_RELEASE_EVENT,
            detail: setup.min_keycode + index as u8,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root: self.root,
            event: self.root,
            child: 0,
            root_x: 640,
            root_y: 360,
            event_x: 640,
            event_y: 360,
            state: KeyButMask::MOD4,
            same_screen: true,
        };
        self.conn
            .send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT, event)
            .unwrap()
            .check()
            .unwrap();
    }
}

impl Session {
    fn selection(&self) -> Atom {
        self.atom(&format!("WM_S{}", self.screen))
    }

    fn owner(&self) -> Window {
        self.conn
            .get_selection_owner(self.selection())
            .unwrap()
            .reply()
            .unwrap()
            .owner
    }

    fn expect_new_manager(&self, previous: Window) -> Window {
        self.wait("new manager ready", || {
            let owner = self.owner();
            owner != 0
                && owner != previous
                && self.values(self.root, "_NET_SUPPORTING_WM_CHECK") == [owner]
        });
        self.owner()
    }

    fn wait_exit(&mut self, index: usize) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = self.managers[index].try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "WM did not exit; logs: {}",
                self.directory.display()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn log(&self, index: usize) -> String {
        fs::read_to_string(self.directory.join(format!("wm-{index}.log"))).unwrap()
    }

    fn fake_manager(&self) -> Window {
        self.conn
            .change_window_attributes(
                self.root,
                &ChangeWindowAttributesAux::new()
                    .event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY),
            )
            .unwrap()
            .check()
            .unwrap();
        let owner = self.window(None, 0, 0, 1, 1);
        self.conn
            .set_selection_owner(owner, self.selection(), x11rb::CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();
        self.set(
            self.root,
            "_NET_SUPPORTING_WM_CHECK",
            AtomEnum::WINDOW,
            &[owner],
        );
        owner
    }

    fn convert(&self, requestor: Window, target: Atom, property: Atom) -> Atom {
        self.conn
            .convert_selection(
                requestor,
                self.selection(),
                target,
                property,
                x11rb::CURRENT_TIME,
            )
            .unwrap()
            .check()
            .unwrap();
        let mut reply_property = None;
        self.wait("selection conversion", || {
            while let Some(event) = self.conn.poll_for_event().unwrap() {
                if let Event::SelectionNotify(reply) = event
                    && reply.requestor == requestor
                    && reply.target == target
                {
                    reply_property = Some(reply.property);
                    return true;
                }
            }
            false
        });
        reply_property.unwrap()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for child in &mut self.managers {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

#[test]
#[ignore = "requires Xvfb"]
fn panels_reserve_space_and_update_existing_windows() {
    let mut session = Session::new(SETTINGS);
    // An existing panel must be recognized even before the WM starts.
    let panel = session.window(Some("_NET_WM_WINDOW_TYPE_DOCK"), 0, 0, 1280, 40);
    session.set(panel, "_NET_WM_STRUT", AtomEnum::CARDINAL, &[0, 0, 100, 0]);
    session.set(
        panel,
        "_NET_WM_STRUT_PARTIAL",
        AtomEnum::CARDINAL,
        &[0, 0, 40, 0, 0, 0, 0, 0, 0, 1279, 0, 0],
    );
    session.map(panel);
    session.spawn_wm(&[]);
    session.expect_workarea([0, 40, 1280, 680]);
    let main = session.window(None, 100, 100, 200, 100);
    session.map(main);
    session.expect_geometry(main, (0, 40, 1280, 680));
    let side = session.window(None, 100, 100, 200, 100);
    session.map(side);
    session.expect_geometry(main, (0, 40, 1024, 680));
    session.expect_geometry(side, (1024, 40, 256, 680));

    session.set(
        panel,
        "_NET_WM_STRUT_PARTIAL",
        AtomEnum::CARDINAL,
        &[0, 0, 2, 0, 0, 0, 0, 0, 0, 1279, 0, 0],
    );
    session.expect_workarea([0, 2, 1280, 718]);
    session.expect_geometry(side, (1024, 2, 256, 718));

    session.set(panel, "_NET_WM_STRUT_PARTIAL", AtomEnum::CARDINAL, &[42]);
    session.expect_workarea([0, 0, 1280, 720]);

    session
        .conn
        .delete_property(panel, session.atom("_NET_WM_STRUT_PARTIAL"))
        .unwrap()
        .check()
        .unwrap();
    session.expect_workarea([0, 100, 1280, 620]);
    session.conn.unmap_window(panel).unwrap().check().unwrap();
    session.expect_workarea([0, 0, 1280, 720]);
    session.expect_geometry(main, (0, 0, 1024, 720));
    session.map(panel);
    session.expect_workarea([0, 100, 1280, 620]);
    session.conn.destroy_window(panel).unwrap().check().unwrap();
    session.expect_workarea([0, 0, 1280, 720]);
    session.expect_geometry(side, (1024, 0, 256, 720));
}

#[test]
#[ignore = "requires Xvfb"]
fn desktop_and_dock_windows_remain_untiled_and_correctly_stacked() {
    let mut session = Session::new(SETTINGS);
    session.spawn_wm(&[]);
    session.expect_workarea([0, 0, 1280, 720]);
    let desktop = session.window(Some("_NET_WM_WINDOW_TYPE_DESKTOP"), 0, 0, 1280, 720);
    session.map(desktop);
    let panel = session.window(Some("_NET_WM_WINDOW_TYPE_DOCK"), 0, 0, 1280, 30);
    session.map(panel);
    let main = session.window(None, 10, 10, 100, 100);
    session.map(main);
    session.expect_geometry(main, (0, 0, 1280, 720));
    session.expect_geometry(desktop, (0, 0, 1280, 720));
    session.expect_geometry(panel, (0, 0, 1280, 30));
    session
        .conn
        .configure_window(panel, &ConfigureWindowAux::new().x(20).y(690).width(1240))
        .unwrap()
        .check()
        .unwrap();
    session.expect_geometry(panel, (20, 690, 1240, 30));
    session.wait("desktop below client below panel", || {
        let children = session
            .conn
            .query_tree(session.root)
            .unwrap()
            .reply()
            .unwrap()
            .children;
        let index = |window| {
            children
                .iter()
                .position(|&candidate| candidate == window)
                .unwrap()
        };
        index(desktop) < index(main) && index(main) < index(panel)
    });
}

#[test]
#[ignore = "requires Xvfb"]
fn fullscreen_covers_panels_and_returns_to_the_latest_workarea() {
    let mut session = Session::new(SETTINGS);
    session.spawn_wm(&[]);
    session.expect_workarea([0, 0, 1280, 720]);
    let panel = session.window(Some("_NET_WM_WINDOW_TYPE_DOCK"), 0, 0, 1280, 40);
    session.set(panel, "_NET_WM_STRUT", AtomEnum::CARDINAL, &[0, 0, 40, 0]);
    session.map(panel);
    session.expect_workarea([0, 40, 1280, 680]);
    let main = session.window(None, 10, 10, 100, 100);
    session.map(main);
    session.expect_geometry(main, (0, 40, 1280, 680));
    session.super_key(u32::from('f'));
    session.expect_geometry(main, (0, 0, 1280, 720));
    session.set(panel, "_NET_WM_STRUT", AtomEnum::CARDINAL, &[0, 0, 60, 0]);
    session.expect_workarea([0, 60, 1280, 660]);
    session.expect_geometry(main, (0, 0, 1280, 720));
    session.wait("fullscreen above panel", || {
        session
            .conn
            .query_tree(session.root)
            .unwrap()
            .reply()
            .unwrap()
            .children
            .last()
            == Some(&main)
    });
    session.super_key(u32::from('f'));
    session.expect_geometry(main, (0, 60, 1280, 660));
}

#[test]
#[ignore = "requires Xvfb"]
fn empty_monitor_arrays_start_with_a_usable_layout() {
    let settings = SETTINGS
        .replace("main_size = [\"80%x100%\", \"80%x100%\"]", "main_size = []")
        .replace("top_left = [\"0,0\", \"0,0\"]", "top_left = []");
    let mut session = Session::new(&settings);
    session.spawn_wm(&[]);
    session.expect_workarea([0, 0, 1280, 720]);
    let main = session.window(None, 10, 10, 100, 100);
    session.map(main);
    session.expect_geometry(main, (0, 0, 1280, 720));
}

#[test]
fn command_line_help_and_errors_do_not_require_a_display() {
    let output = Command::new(env!("CARGO_BIN_EXE_pick-full-wm"))
        .arg("--help")
        .env_remove("DISPLAY")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--replace"));
    assert!(help.contains("--no-session-services"));
    let output = Command::new(env!("CARGO_BIN_EXE_pick-full-wm"))
        .arg("--unknown")
        .env_remove("DISPLAY")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("unknown argument")
    );
}

#[test]
#[ignore = "requires Xvfb"]
fn ordinary_startup_refuses_to_replace_an_existing_manager() {
    let mut session = Session::new(SETTINGS);
    let first = session.spawn_wm(&[]);
    let original_owner = session.expect_new_manager(0);
    fs::write(
        session.directory.join(".config/pick-full-wm/settings.toml"),
        SETTINGS.replace(
            "startups = []",
            "startups = [\"printf started > startup-ran\"]",
        ),
    )
    .unwrap();
    let second = session.spawn_wm(&[]);
    assert!(!session.wait_exit(second).success());
    assert!(session.managers[first].try_wait().unwrap().is_none());
    assert_eq!(session.owner(), original_owner);
    assert_eq!(
        session.values(session.root, "_NET_SUPPORTING_WM_CHECK"),
        [original_owner]
    );
    assert!(session.log(second).contains("use --replace"));
    assert!(!session.directory.join("startup-ran").exists());
}

#[test]
#[ignore = "requires Xvfb"]
fn replacing_pick_full_preserves_visible_and_hidden_clients() {
    let mut session = Session::new(SETTINGS);
    let first = session.spawn_wm(&[]);
    let original_owner = session.expect_new_manager(0);
    let main = session.window(None, 10, 10, 100, 100);
    session.map(main);
    session.expect_geometry(main, (0, 0, 1280, 720));
    session.super_key(u32::from('2'));
    session.wait("second workspace", || {
        session.values(session.root, "_NET_CURRENT_DESKTOP") == [1]
    });
    let other = session.window(None, 10, 10, 100, 100);
    session.map(other);
    session.expect_geometry(other, (0, 0, 1280, 720));

    let second = session.spawn_wm(&["--replace"]);
    let new_owner = session.expect_new_manager(original_owner);
    assert!(session.wait_exit(first).success());
    assert!(session.managers[second].try_wait().unwrap().is_none());
    assert!(
        session
            .conn
            .get_window_attributes(original_owner)
            .unwrap()
            .reply()
            .is_err()
    );
    for window in [main, other] {
        session.wait("adopted client visible", || {
            session
                .conn
                .get_window_attributes(window)
                .unwrap()
                .reply()
                .unwrap()
                .map_state
                == x11rb::protocol::xproto::MapState::VIEWABLE
        });
    }
    session.wait("both clients tiled", || {
        let mut widths = [session.geometry(main).2, session.geometry(other).2];
        widths.sort();
        widths == [256, 1024]
    });
    assert_eq!(
        session.values(new_owner, "_NET_SUPPORTING_WM_CHECK"),
        [new_owner]
    );
}

#[test]
#[ignore = "requires Xvfb"]
fn replacement_waits_for_owner_destruction_and_adopts_iconic_clients() {
    let mut session = Session::new(SETTINGS);
    let previous = session.fake_manager();
    let iconic = session.window(None, 10, 10, 100, 100);
    session
        .conn
        .change_property32(
            PropMode::REPLACE,
            iconic,
            session.atom("WM_STATE"),
            session.atom("WM_STATE"),
            &[3, 0],
        )
        .unwrap()
        .check()
        .unwrap();
    let withdrawn = session.window(None, 20, 20, 100, 100);
    session.spawn_wm(&["--replace"]);
    session.wait("old WM notified", || {
        while let Some(event) = session.conn.poll_for_event().unwrap() {
            if let Event::SelectionClear(clear) = event
                && clear.owner == previous
                && clear.selection == session.selection()
            {
                return true;
            }
        }
        false
    });
    session
        .conn
        .change_window_attributes(
            session.root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
        )
        .unwrap()
        .check()
        .unwrap();
    thread::sleep(Duration::from_millis(50));
    assert_eq!(
        session.values(session.root, "_NET_SUPPORTING_WM_CHECK"),
        [previous]
    );
    session
        .conn
        .destroy_window(previous)
        .unwrap()
        .check()
        .unwrap();
    session.expect_new_manager(previous);
    session.expect_geometry(iconic, (0, 0, 1280, 720));
    assert_eq!(
        session
            .conn
            .get_window_attributes(withdrawn)
            .unwrap()
            .reply()
            .unwrap()
            .map_state,
        x11rb::protocol::xproto::MapState::UNMAPPED
    );
}

#[test]
#[ignore = "requires Xvfb"]
fn replacement_times_out_without_destroying_the_previous_manager() {
    let mut session = Session::new(SETTINGS);
    let previous = session.fake_manager();
    let client = session.window(None, 10, 10, 100, 100);
    session.map(client);
    let incoming = session.spawn_wm(&["--replace"]);
    assert!(!session.wait_exit(incoming).success());
    assert!(session.log(incoming).contains("within 5 seconds"));
    assert!(
        session
            .conn
            .get_window_attributes(previous)
            .unwrap()
            .reply()
            .is_ok()
    );
    assert_eq!(session.geometry(client), (10, 10, 100, 100));
    assert_eq!(
        session.values(session.root, "_NET_SUPPORTING_WM_CHECK"),
        [previous]
    );
}

#[test]
#[ignore = "requires Xvfb"]
fn a_manager_without_a_selection_still_cannot_be_displaced_by_force() {
    let mut session = Session::new(SETTINGS);
    session
        .conn
        .change_window_attributes(
            session.root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_REDIRECT),
        )
        .unwrap()
        .check()
        .unwrap();
    for args in [&[][..], &["--replace"][..]] {
        let incoming = session.spawn_wm(args);
        assert!(!session.wait_exit(incoming).success());
        assert!(
            session
                .log(incoming)
                .contains("cannot acquire root redirection")
        );
        assert!(
            session
                .values(session.root, "_NET_SUPPORTING_WM_CHECK")
                .is_empty()
        );
        assert_eq!(session.owner(), 0);
    }
}

#[test]
#[ignore = "requires Xvfb"]
fn manager_announces_ownership_and_serves_selection_targets() {
    let mut session = Session::on_screen(SETTINGS, 1);
    session
        .conn
        .change_window_attributes(
            session.root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
        )
        .unwrap()
        .check()
        .unwrap();
    session.spawn_wm(&[]);
    let owner = session.expect_new_manager(0);
    assert_eq!(
        session
            .conn
            .get_selection_owner(session.atom("WM_S0"))
            .unwrap()
            .reply()
            .unwrap()
            .owner,
        0
    );
    let mut announcement = None;
    session.wait("MANAGER announcement", || {
        while let Some(event) = session.conn.poll_for_event().unwrap() {
            if let Event::ClientMessage(message) = event
                && message.type_ == session.atom("MANAGER")
            {
                assert_eq!(message.format, 32);
                announcement = Some(message.data.as_data32());
                return true;
            }
        }
        false
    });
    let data = announcement.unwrap();
    assert_ne!(data[0], x11rb::CURRENT_TIME);
    assert_eq!(&data[1..], &[session.selection(), owner, 0, 0]);
    let requestor = session.window(None, 0, 0, 1, 1);
    let output = session.atom("TEST_SELECTION");
    assert_eq!(
        session.convert(requestor, session.atom("VERSION"), output),
        output
    );
    assert_eq!(session.values(requestor, "TEST_SELECTION"), [2, 0]);
    assert_eq!(
        session.convert(requestor, session.atom("TIMESTAMP"), output),
        output
    );
    assert_eq!(session.values(requestor, "TEST_SELECTION"), [data[0]]);
    assert_eq!(
        session.convert(requestor, session.atom("TARGETS"), output),
        output
    );
    let targets = session.values(requestor, "TEST_SELECTION");
    for name in ["TARGETS", "MULTIPLE", "TIMESTAMP", "VERSION"] {
        assert!(targets.contains(&session.atom(name)));
    }
    assert_eq!(
        session.convert(requestor, session.atom("UNSUPPORTED"), output),
        0
    );
    assert_eq!(session.convert(requestor, session.atom("MULTIPLE"), 0), 0);

    let pairs = [
        session.atom("VERSION"),
        session.atom("TEST_VERSION"),
        session.atom("UNSUPPORTED"),
        session.atom("TEST_UNSUPPORTED"),
        session.atom("TIMESTAMP"),
        session.atom("TEST_TIME"),
    ];
    session
        .conn
        .change_property32(
            PropMode::REPLACE,
            requestor,
            output,
            session.atom("ATOM_PAIR"),
            &pairs,
        )
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(
        session.convert(requestor, session.atom("MULTIPLE"), output),
        output
    );
    assert_eq!(
        session.values(requestor, "TEST_SELECTION"),
        [pairs[0], pairs[1], pairs[2], 0, pairs[4], pairs[5]]
    );
    assert_eq!(session.values(requestor, "TEST_VERSION"), [2, 0]);
    assert_eq!(session.values(requestor, "TEST_TIME"), [data[0]]);
}

#[test]
#[ignore = "requires Xvfb"]
fn session_service_opt_out_preserves_the_shared_user_environment() {
    let mut isolated = Session::new(SETTINGS);
    isolated.spawn_wm(&[]);
    isolated.expect_workarea([0, 0, 1280, 720]);
    assert!(!isolated.directory.join("service-calls").exists());

    // Standalone startup retains upstream behavior, exercised only against stubs.
    let mut standalone = Session::new(SETTINGS);
    standalone.spawn_wm_with_services(&[], true);
    standalone.expect_workarea([0, 0, 1280, 720]);
    let calls = fs::read_to_string(standalone.directory.join("service-calls")).unwrap();
    assert_eq!(calls.lines().count(), 3);
    assert!(calls.contains("dbus-update-activation-environment"));
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.ends_with("systemctl"))
            .count(),
        2
    );
}

#[test]
#[ignore = "requires Xvfb"]
fn dialogs_keep_focus_above_fullscreen_clients_and_panels() {
    let mut session = Session::new(SETTINGS);
    session.spawn_wm(&[]);
    session.expect_workarea([0, 0, 1280, 720]);
    let panel = session.window(Some("_NET_WM_WINDOW_TYPE_DOCK"), 0, 0, 1280, 40);
    session.set(panel, "_NET_WM_STRUT", AtomEnum::CARDINAL, &[0, 0, 40, 0]);
    session.map(panel);
    session.expect_workarea([0, 40, 1280, 680]);
    let main = session.window(None, 10, 10, 100, 100);
    session.map(main);
    session.expect_geometry(main, (0, 40, 1280, 680));
    session.super_key(u32::from('f'));
    session.expect_geometry(main, (0, 0, 1280, 720));

    let dialog = session.window(Some("_NET_WM_WINDOW_TYPE_DIALOG"), 20, 20, 400, 200);
    session.set(dialog, "WM_TRANSIENT_FOR", AtomEnum::WINDOW, &[main]);
    session.map(dialog);
    session.wait("dialog focused above fullscreen", || {
        session
            .conn
            .get_input_focus()
            .unwrap()
            .reply()
            .unwrap()
            .focus
            == dialog
            && session
                .conn
                .query_tree(session.root)
                .unwrap()
                .reply()
                .unwrap()
                .children
                .last()
                == Some(&dialog)
    });
    session.set(panel, "_NET_WM_STRUT", AtomEnum::CARDINAL, &[0, 0, 60, 0]);
    session.expect_workarea([0, 60, 1280, 660]);
    session.expect_geometry(main, (0, 0, 1280, 720));
    session.wait(
        "dialog still focused above fullscreen after relayout",
        || {
            session
                .conn
                .get_input_focus()
                .unwrap()
                .reply()
                .unwrap()
                .focus
                == dialog
                && session
                    .conn
                    .query_tree(session.root)
                    .unwrap()
                    .reply()
                    .unwrap()
                    .children
                    .last()
                    == Some(&dialog)
        },
    );
}

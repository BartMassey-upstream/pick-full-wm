//! End-to-end protocol tests. Each test owns a private Xvfb display and temporary home.

use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConfigureWindowAux, ConnectionExt, CreateWindowAux, EventMask,
    KEY_RELEASE_EVENT, KeyButMask, KeyReleaseEvent, PropMode, Window, WindowClass,
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
}

impl Session {
    fn new(settings: &str) -> Self {
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
        let display_file = directory.join("display");
        let mut server = Command::new("Xvfb")
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
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
                break format!(":{}", value.trim());
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
        }
    }

    fn spawn_wm(&mut self, args: &[&str]) -> usize {
        let index = self.managers.len();
        let log = File::create(self.directory.join(format!("wm-{index}.log"))).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_pick-full-wm"))
            .args(args)
            .env("DISPLAY", &self.display)
            // Isolate the child's real configuration lookup from the developer's desktop.
            .env("HOME", &self.directory)
            .env("DBUS_SESSION_BUS_ADDRESS", "disabled-for-test")
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

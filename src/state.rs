use std::collections::{HashMap, HashSet};
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as RandrConnectionExt};
use x11rb::protocol::xproto::{GetKeyboardMappingReply, Window};
use x11rb::rust_connection::RustConnection;

pub struct State {
    pub settings: crate::settings::Settings,
    pub conn: RustConnection,
    pub screen_num: usize,
    pub root: Window,
    pub atoms: crate::atoms::Atoms,
    pub monitors: Vec<Monitor>,
    pub current_monitor: usize,
    pub current_workspace: usize,
    pub all_windows: HashSet<Window>,
    pub special_windows: HashMap<Window, crate::panels::WindowKind>,
    pub keyboard_mapping: Option<GetKeyboardMappingReply>,
    pub published_client_list: Vec<Window>,
    pub published_client_stacking: Vec<Window>,
    pub published_desktops: HashMap<Window, usize>,
    pub startup_warnings: Vec<String>, // logged once logging is set up
}
impl State {
    pub fn init() -> Self {
        let (settings, mut startup_warnings) = crate::settings::load_settings();
        let (conn, screen_num) = crate::setup::internal::connect();
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let atoms = crate::atoms::Atoms::new(&conn)
            .expect("Failed to intern atoms")
            .reply()
            .expect("Failed to get atom reply");
        let mut monitor_infos = get_monitor_infos(&conn, root);
        monitor_infos.sort_by_key(|m| m.position.0);
        let mut monitors: Vec<Monitor> = vec![];
        for (index, info) in monitor_infos.iter().enumerate() {
            info.print();
            let monitor = Monitor::new(&settings, info, index, &mut startup_warnings);
            monitor.print();
            monitors.push(monitor);
        }
        Self {
            settings,
            conn,
            screen_num,
            root,
            atoms,
            monitors,
            current_monitor: 0,
            current_workspace: 0,
            all_windows: HashSet::new(),
            special_windows: HashMap::new(),
            keyboard_mapping: None,
            published_client_list: vec![],
            published_client_stacking: vec![],
            published_desktops: HashMap::new(),
            startup_warnings,
        }
    }
    pub fn monitor(&self) -> &Monitor {
        &self.monitors[self.current_monitor]
    }
    pub fn mut_monitor(&mut self) -> &mut Monitor {
        &mut self.monitors[self.current_monitor]
    }
    pub fn workspace(&self) -> &Workspace {
        &self.monitor().workspaces[self.current_workspace]
    }
    pub fn mut_workspace(&mut self) -> &mut Workspace {
        let current_workspace = self.current_workspace;
        &mut self.mut_monitor().workspaces[current_workspace]
    }
}
pub struct Monitor {
    /// Physical bounds, independent of panel reservations and layout offsets.
    pub geometry: crate::panels::Rect,
    pub workarea: crate::panels::Rect,
    pub sizes: Sizes,
    pub position: (i32, i32),
    pub workspaces: Vec<Workspace>,
    pub close_box: Option<Window>,
    pub monitor_box: Option<Window>,
}
impl Monitor {
    pub fn new(
        settings: &crate::settings::Settings,
        monitor_info: &MonitorInfo,
        index: usize,
        warnings: &mut Vec<String>,
    ) -> Self {
        let geometry = crate::panels::Rect {
            x: monitor_info.position.0,
            y: monitor_info.position.1,
            width: monitor_info.size.0 as i32,
            height: monitor_info.size.1 as i32,
        };
        let mut workspaces: Vec<Workspace> = vec![];
        for _ in 0..settings.bindings.workspaces.len() {
            workspaces.push(Workspace::new());
        }
        let mut monitor = Self {
            geometry,
            workarea: geometry,
            sizes: Sizes {
                main: (1, 1),
                side: (0, 1),
                available: (1, 1),
            },
            position: monitor_info.position,
            workspaces,
            close_box: None,
            monitor_box: None,
        };
        monitor.set_workarea(settings, index, geometry, warnings);
        monitor
    }

    /// Apply configured margins and proportions inside the panel-free area.
    pub fn set_workarea(
        &mut self,
        settings: &crate::settings::Settings,
        index: usize,
        area: crate::panels::Rect,
        warnings: &mut Vec<String>,
    ) {
        self.workarea = area;
        let dimensions = (area.width as f32, area.height as f32);
        let offset = monitor_setting(
            crate::calc::get_position,
            dimensions,
            &settings.layout.top_left,
            index,
            crate::settings::DEFAULT_TOP_LEFT,
            "top_left",
            warnings,
        );
        let offset = (
            offset.0.clamp(0, area.width - 1),
            offset.1.clamp(0, area.height - 1),
        );
        self.position = (area.x + offset.0, area.y + offset.1);
        let available = (area.width - offset.0, area.height - offset.1);
        let requested = monitor_setting(
            crate::calc::get_full_size,
            dimensions,
            &settings.layout.main_size,
            index,
            crate::settings::DEFAULT_MAIN_SIZE,
            "main_size",
            warnings,
        );
        let main = (
            requested.0.clamp(1, available.0),
            requested.1.clamp(1, available.1),
        );
        self.sizes = Sizes {
            main,
            side: (available.0 - main.0, main.1),
            available,
        };
    }
    pub fn print(&self) {
        println!(
            "Calculated Monitor: {}x{} at ({}, {})",
            self.sizes.main.0 + self.sizes.side.0,
            self.sizes.main.1,
            self.position.0,
            self.position.1,
        );
    }
}

pub struct Sizes {
    pub available: (i32, i32),
    pub main: (i32, i32),
    pub side: (i32, i32),
}

/// Repeat the last monitor override; use defaults for empty or malformed entries.
fn monitor_setting(
    parse: fn(f32, f32, &str) -> Option<(i32, i32)>,
    screen: (f32, f32),
    entries: &[String],
    index: usize,
    default: &str,
    name: &str,
    warnings: &mut Vec<String>,
) -> (i32, i32) {
    let value = entries
        .get(index)
        .or_else(|| entries.last())
        .map(String::as_str)
        .unwrap_or(default);
    match parse(screen.0, screen.1, value) {
        Some(parsed) => parsed,
        None => {
            warnings.push(format!(
                "Invalid layout.{} \"{}\" for monitor {}: using default \"{}\".",
                name, value, index, default
            ));
            parse(screen.0, screen.1, default).unwrap_or((0, 0))
        }
    }
}

pub struct Workspace {
    pub main_window: Option<Window>,
    pub side_windows: Vec<Option<Window>>,
    pub help_window: Option<Window>,
    pub key_hint_windows: HashMap<String, Window>,
    pub floatings: Vec<Window>,
    pub fullscreen: Option<Window>, // stays fullscreen whenever it's this workspace's main
}
impl Workspace {
    fn new() -> Self {
        Self {
            main_window: None,
            side_windows: vec![],
            help_window: None,
            key_hint_windows: HashMap::new(),
            floatings: vec![],
            fullscreen: None,
        }
    }
    // the fullscreen window is currently main (so it covers the screen)
    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen.is_some() && self.fullscreen == self.main_window
    }
}

pub struct MonitorInfo {
    pub position: (i32, i32),
    pub size: (u32, u32),
}
impl MonitorInfo {
    pub fn print(&self) {
        println!(
            "Xrandr Monitor: {}x{} at ({}, {})",
            self.size.0, self.size.1, self.position.0, self.position.1,
        );
    }
}

pub fn get_monitor_infos(conn: &RustConnection, root: Window) -> Vec<MonitorInfo> {
    let mut monitor_infos: Vec<MonitorInfo> = vec![];

    // TODO: add more graceful logging around monitor errors
    let resources = conn
        .randr_get_screen_resources(root)
        .expect("Failed to get screen resources")
        .reply()
        .expect("Failed to get screen resources reply");

    for &output in &resources.outputs {
        let output_info = conn
            .randr_get_output_info(output, resources.config_timestamp)
            .expect("Failed to get output info")
            .reply()
            .expect("Failed to get output info reply");

        if output_info.connection == randr::Connection::CONNECTED && output_info.crtc != 0 {
            let crtc_info = conn
                .randr_get_crtc_info(output_info.crtc, resources.config_timestamp)
                .expect("Failed to get crtc info")
                .reply()
                .expect("Failed to get crtc info reply");

            monitor_infos.push(MonitorInfo {
                position: (crtc_info.x as i32, crtc_info.y as i32),
                size: (crtc_info.width as u32, crtc_info.height as u32),
            });
        }
    }
    monitor_infos
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(layout: &str) -> crate::settings::Settings {
        crate::settings::Settings {
            layout: toml::from_str(layout).unwrap(),
            ..Default::default()
        }
    }

    #[test]
    fn extra_monitors_repeat_the_last_size_and_offset_independently() {
        let settings = settings(
            r#"
            main_size = ["70%x100%", "60%x90%"]
            top_left = ["0,10%"]
            conditional_full = true
        "#,
        );
        let info = MonitorInfo {
            position: (2000, 0),
            size: (1000, 800),
        };
        let first = Monitor::new(&settings, &info, 0, &mut Vec::new());
        let third = Monitor::new(&settings, &info, 2, &mut Vec::new());
        assert_eq!(first.sizes.main, (700, 720));
        assert_eq!(third.sizes.main, (600, 720));
        assert_eq!(third.position, (2000, 80));
        assert_eq!(third.sizes.side, (400, 720));
    }

    #[test]
    fn missing_and_empty_arrays_use_defaults_for_any_monitor() {
        for layout in [
            "conditional_full = true",
            "main_size = []\ntop_left = []\nconditional_full = true",
        ] {
            let settings = settings(layout);
            let info = MonitorInfo {
                position: (3840, 1080),
                size: (1000, 800),
            };
            let mut monitor = Monitor::new(&settings, &info, 9, &mut Vec::new());
            assert_eq!(monitor.position, (3840, 1080));
            assert_eq!(monitor.sizes.main, (800, 800));
            monitor.set_workarea(
                &settings,
                9,
                crate::panels::Rect {
                    x: 3840,
                    y: 1120,
                    width: 1000,
                    height: 760,
                },
                &mut Vec::new(),
            );
            assert_eq!(monitor.position, (3840, 1120));
            assert_eq!(monitor.sizes.main, (800, 760));
        }
    }
}

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, MapState};
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

pub fn startups(state: &mut crate::state::State) {
    if state.settings.layout.close_box {
        for _ in 0..state.monitors.len() {
            crate::binaries::close_box();
        }
    }
    if state.settings.layout.monitor_box && state.monitors.len() > 1 {
        for _ in 0..state.monitors.len() {
            crate::binaries::monitor_box();
        }
    }
}

pub fn custom_startups(state: &mut crate::state::State) {
    for startup in &state.settings.applications.startups {
        crate::windows::misc::run_command(startup);
    }
}

pub fn dbus_init() {
    // systemd / dbus activated services (portals, etc) need this session's display
    match std::process::Command::new("dbus-update-activation-environment")
        .args([
            "--systemd",
            "DISPLAY",
            "XAUTHORITY",
            "XDG_CURRENT_DESKTOP=pick-full-wm",
        ])
        .status()
    {
        Ok(_) => log::info!("Updated dbus activation environment."),
        Err(e) => log::error!("Failed to update dbus activation environment: {}", e),
    }
    // a leftover wayland session's display would be tried first by gtk services
    match std::process::Command::new("systemctl")
        .args(["--user", "unset-environment", "WAYLAND_DISPLAY"])
        .status()
    {
        Ok(_) => log::info!("Unset WAYLAND_DISPLAY from systemd environment."),
        Err(e) => log::error!("Failed to unset WAYLAND_DISPLAY: {}", e),
    }
    // portals still running from another session keep its environment: restart if running
    match std::process::Command::new("systemctl")
        .args([
            "--user",
            "--no-block",
            "try-restart",
            "xdg-desktop-portal.service",
            "xdg-desktop-portal-gtk.service",
        ])
        .status()
    {
        Ok(_) => log::info!("Restarted any running xdg-desktop-portal services."),
        Err(e) => log::error!("Failed to restart xdg-desktop-portal services: {}", e),
    }
}

pub fn windows(state: &mut crate::state::State) {
    // Ownership has been acquired before modifying the root or starting clients.
    let screen = &state.conn.setup().roots[state.screen_num];
    state
        .conn
        .change_window_attributes(
            state.root,
            &ChangeWindowAttributesAux::new().background_pixel(screen.black_pixel),
        )
        .expect("Failed to set background");
    state
        .conn
        .clear_area(false, state.root, 0, 0, 0, 0)
        .expect("Failed to clear root window");

    root_cursor(state);

    state.conn.sync().expect("Failed to sync");
}

// the root has no cursor of its own: without one the pointer is invisible until a client sets one
fn root_cursor(state: &mut crate::state::State) {
    let database = match x11rb::resource_manager::new_from_default(&state.conn) {
        Ok(database) => database,
        Err(e) => {
            log::error!("setup::root_cursor(..) resource database error: {:?}", e);
            return;
        }
    };
    let handle = match x11rb::cursor::Handle::new(&state.conn, state.screen_num, &database) {
        Ok(cookie) => match cookie.reply() {
            Ok(handle) => handle,
            Err(e) => {
                log::error!("setup::root_cursor(..) cursor handle reply error: {:?}", e);
                return;
            }
        },
        Err(e) => {
            log::error!("setup::root_cursor(..) cursor handle error: {:?}", e);
            return;
        }
    };
    let cursor = match handle.load_cursor(&state.conn, "left_ptr") {
        Ok(cursor) => cursor,
        Err(e) => {
            log::error!("setup::root_cursor(..) load cursor error: {:?}", e);
            return;
        }
    };
    if let Err(e) = state
        .conn
        .change_window_attributes(state.root, &ChangeWindowAttributesAux::new().cursor(cursor))
    {
        log::error!("setup::root_cursor(..) set cursor error: {:?}", e);
    }
    log::info!("Set root window cursor.");
}

pub fn logging(state: &crate::state::State) {
    let dir_str = shellexpand::tilde(&state.settings.files.log_directory).to_string();
    let dir_path = std::path::Path::new(&dir_str);
    if !dir_path.exists() {
        let _ = std::fs::create_dir_all(dir_path);
    }
    let time: chrono::DateTime<chrono::offset::Local> = chrono::offset::Local::now();
    let time_str = time.format("%Y_%m_%d-%H_%M_%S").to_string();
    let log_path = format!("{}/{}.log", dir_path.to_str().unwrap(), time_str);
    // local time only resolves while single-threaded, so this must run early
    let mut config = simplelog::ConfigBuilder::new();
    let is_local = config.set_time_offset_to_local().is_ok();
    let _ = simplelog::WriteLogger::init(
        simplelog::LevelFilter::max(),
        config.build(),
        std::fs::File::create(&log_path).unwrap(),
    );
    log::info!("Logging initialized at path \"{}\".", &log_path);
    if !is_local {
        log::warn!("Failed to get local time offset: log times are UTC.");
    }
    for warning in &state.startup_warnings {
        log::warn!("{}", warning);
    }
    // panics otherwise only reach stderr (the tty), not the log
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("Panic: {}", info);
        default_hook(info);
    }));
}

/// Adopt clients left by the previous WM, including iconic windows on hidden desktops.
/// Override-redirect popups and withdrawn clients are not adopted.
pub fn adopt_windows(state: &mut crate::state::State) {
    let children = state
        .conn
        .query_tree(state.root)
        .expect("Failed to query existing windows")
        .reply()
        .expect("Failed to read existing windows")
        .children;
    let original_monitor = state.current_monitor;
    for window in children {
        if window == state.manager.window {
            continue;
        }
        let Ok(attributes) = state
            .conn
            .get_window_attributes(window)
            .expect("Failed to query client")
            .reply()
        else {
            continue;
        };
        if attributes.override_redirect {
            continue;
        }
        if attributes.map_state == MapState::UNMAPPED {
            let iconic = state
                .conn
                .get_property(
                    false,
                    window,
                    state.atoms.WM_STATE,
                    state.atoms.WM_STATE,
                    0,
                    2,
                )
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .and_then(|reply| reply.value32().and_then(|mut values| values.next()))
                == Some(3);
            if !iconic {
                continue;
            }
        }
        let Ok(geometry) = state
            .conn
            .get_geometry(window)
            .expect("Failed to query client geometry")
            .reply()
        else {
            continue;
        };
        let x = i32::from(geometry.x) + i32::from(geometry.width) / 2;
        let y = i32::from(geometry.y) + i32::from(geometry.height) / 2;
        state.current_monitor = state
            .monitors
            .iter()
            .position(|monitor| {
                x >= monitor.geometry.x
                    && x < monitor.geometry.right()
                    && y >= monitor.geometry.y
                    && y < monitor.geometry.bottom()
            })
            .unwrap_or(0);
        crate::events::window::manage_window(state, window);
    }
    state.current_monitor = original_monitor;
    crate::windows::layout::relayout(state);
}

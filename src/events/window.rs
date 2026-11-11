use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Allow, CONFIGURE_NOTIFY_EVENT, ConfigWindow, ConfigureNotifyEvent, ConfigureRequestEvent,
    ConfigureWindowAux, ConnectionExt, DestroyNotifyEvent, EventMask, MapRequestEvent,
    PropertyNotifyEvent, UnmapNotifyEvent,
};

pub fn map_request(state: &mut crate::state::State, event: MapRequestEvent) {
    manage_window(state, event.window);
}

/// Classify and map a new client or one inherited from the previous WM.
pub fn manage_window(state: &mut crate::state::State, window: u32) {
    if state.all_windows.contains(&window) {
        // app hid the window (unmap) and is now showing it again
        let mut slot: Option<(usize, usize)> = None;
        for (monitor_index, monitor) in state.monitors.iter().enumerate() {
            for (workspace_index, workspace) in monitor.workspaces.iter().enumerate() {
                if workspace.main_window == Some(window)
                    || workspace.side_windows.contains(&Some(window))
                {
                    slot = Some((monitor_index, workspace_index));
                }
            }
        }
        if let Some((monitor_index, workspace_index)) = slot {
            log::info!(
                "Remapping handled window \"{:?}\" ({}).",
                crate::windows::gets::window_name(state, window),
                window
            );
            // hidden workspaces get mapped by workspaces::switch(..) later
            if workspace_index == state.current_workspace {
                if let Err(e) = state.conn.map_window(window) {
                    log::error!("events::map_request(..) map window error: {:?}", e);
                }
                let real_current_monitor = state.current_monitor; // TODO: gross temp set
                state.current_monitor = monitor_index; // TODO: gross temp set
                crate::windows::layout::layout_side_space(state);
                state.current_monitor = real_current_monitor; // TODO: gross temp set
            }
            return;
        }
        log::warn!(
            "Handled window \"{:?}\" ({}) lost its slot: mapping as new.",
            crate::windows::gets::window_name(state, window),
            window
        );
        state.all_windows.remove(&window);
    }
    log::info!(
        "Map Window: {:?} ({})",
        crate::windows::gets::window_name(state, window),
        window,
    );
    if !crate::safety::window_exists(state, window) {
        log::error!("Map request for non-existent window {}.", window);
        if let Err(e) = state.conn.allow_events(Allow::ASYNC_BOTH, CURRENT_TIME) {
            log::error!("events::map_request(..) allow events error: {:?}", e);
        }
        if let Err(e) = state.conn.flush() {
            log::error!("events::map_request(..) flush error: {:?}", e);
        }
        return;
    }
    if let Err(e) = state.conn.map_window(window) {
        log::error!("events::map_request(..) map window error: {:?}", e);
    }
    if let Some(key) = crate::windows::gets::key_hint_window(state, window) {
        log::info!("Window {} should be a key hint window.", window);
        if let Some(entry) = state.mut_workspace().key_hint_windows.get_mut(&key) {
            let old_key = entry.clone();
            *entry = window;
            if old_key != window && crate::safety::window_exists(state, old_key) {
                if let Err(e) = state.conn.destroy_window(old_key) {
                    log::error!("events::map_request(..) destroy window error: {:?}", e);
                }
                if let Err(e) = state.conn.flush() {
                    log::error!("events::map_request(..) flush error: {:?}", e);
                }
            }
        } else {
            state.mut_workspace().key_hint_windows.insert(key, window);
        }
        crate::windows::layout::layout_side_space(state);
        return;
    }
    if crate::windows::checks::is_help_window(state, window) {
        log::info!("Window {} should be a help window.", window);
        crate::ewmh::set_active(state, window);
        if let Err(e) = state.conn.flush() {
            log::error!("events::map_request(..) flush error: {:?}", e);
        }
        state.mut_workspace().help_window = Some(window);
        return;
    }
    if crate::windows::checks::is_close_box(state, window) {
        log::info!("Window {} should be a close box.", window);
        for i in 0..state.monitors.len() {
            if state.monitors[i].close_box.is_none() {
                log::info!("connecting close box {} to monitor {}", window, i);
                state.monitors[i].close_box = Some(window);
                break;
            }
        }
        crate::windows::layout::place_close_boxes(state);
        return;
    }
    if crate::windows::checks::is_monitor_box(state, window) {
        log::info!("Window {} should be a monitor box.", window);
        for i in 0..state.monitors.len() {
            if state.monitors[i].monitor_box.is_none() {
                log::info!("connecting monitor box {} to monitor {}", window, i);
                state.monitors[i].monitor_box = Some(window);
                break;
            }
        }
        crate::windows::layout::place_monitor_boxes(state);
        return;
    }
    if crate::panels::window_kind(state, window).is_some() {
        crate::panels::refresh(state);
        return;
    }
    if crate::windows::checks::is_excepted_window(state, window) {
        return;
    }
    if crate::windows::checks::is_popup(state, window) {
        log::info!("Window {} should be a popup.", window);
        // apps can send a second map request before the first takes effect
        if !state.workspace().floatings.contains(&window) {
            state.mut_workspace().floatings.push(window);
        }
        crate::windows::layout::center_window(state, window);
        if crate::windows::checks::takes_focus(state, window) {
            crate::ewmh::set_active(state, window);
        }
        return;
    }
    if let Some(main) = state.workspace().main_window {
        // a fullscreen main keeps the screen: new windows open on the side
        if state.settings.layout.new_to_main && !state.workspace().is_fullscreen() {
            state.mut_workspace().main_window = None;
            crate::windows::core::send_side_space(state, main, None);
            crate::windows::core::fill_main_space(state, window);
        } else {
            crate::windows::core::send_side_space(state, window, None);
        }
    } else {
        crate::windows::core::fill_main_space(state, window);
    }
    crate::windows::layout::place_close_boxes(state);
    crate::windows::layout::place_monitor_boxes(state);
}

pub fn destroy(state: &mut crate::state::State, event: DestroyNotifyEvent) {
    log::info!(
        "Destroy Window: {:?} ({})",
        crate::windows::gets::window_name(state, event.window),
        event.window,
    );
    state.all_windows.remove(&event.window);
    crate::windows::core::release_window(state, event.window);
    crate::ewmh::restore_lost_focus(state);
}

pub fn unmap(state: &mut crate::state::State, event: UnmapNotifyEvent) {
    // the wm only unmaps windows on hidden workspaces (workspaces::switch(..)),
    // so an unmap on the visible workspace means the app hid its window
    if let Some((_, workspace_index)) = crate::windows::core::find_window(state, event.window)
        && workspace_index == state.current_workspace
    {
        log::info!("Window {} hidden by client.", event.window);
        state.all_windows.remove(&event.window);
        crate::windows::core::release_window(state, event.window);
    }
    crate::ewmh::restore_lost_focus(state);
}

pub fn configure_request(state: &mut crate::state::State, event: ConfigureRequestEvent) {
    let mut is_tiled = false;
    let mut is_floating = false;
    for monitor in &state.monitors {
        for workspace in &monitor.workspaces {
            if workspace.main_window == Some(event.window)
                || workspace.side_windows.contains(&Some(event.window))
            {
                is_tiled = true;
            }
            if workspace.floatings.contains(&event.window) {
                is_floating = true;
            }
            // wm helpers are placed and raised by the wm, so treat them like tiled windows
            if workspace
                .key_hint_windows
                .values()
                .any(|w| *w == event.window)
            {
                is_tiled = true;
            }
        }
        if monitor.close_box == Some(event.window) || monitor.monitor_box == Some(event.window) {
            is_tiled = true;
        }
    }
    if is_floating {
        // honor size (and stacking), but keep popups centered
        let mut aux = ConfigureWindowAux::from_configure_request(&event);
        aux.x = None;
        aux.y = None;
        if let Err(e) = state.conn.configure_window(event.window, &aux) {
            log::error!(
                "events::configure_request(..) configure window error: {:?}",
                e
            );
        }
        crate::windows::layout::center_window(state, event.window);
        return;
    }
    if !is_tiled {
        // not ours to lay out (excluded, unmapped, etc): honor the request
        if event.value_mask.contains(ConfigWindow::STACK_MODE) {
            log::info!("Honored restack request from window {}.", event.window);
        }
        if let Err(e) = state.conn.configure_window(
            event.window,
            &ConfigureWindowAux::from_configure_request(&event),
        ) {
            log::error!(
                "events::configure_request(..) configure window error: {:?}",
                e
            );
        }
        if let Err(e) = state.conn.flush() {
            log::error!("events::configure_request(..) flush error: {:?}", e);
        }
        return;
    }
    // tiled: deny, but tell the client its real geometry (ICCCM 4.1.5)
    log::info!(
        "Denied configure request for managed window {}.",
        event.window
    );
    let geometry = match state.conn.get_geometry(event.window) {
        Ok(cookie) => match cookie.reply() {
            Ok(reply) => reply,
            Err(_) => return,
        },
        Err(_) => return,
    };
    let notify = ConfigureNotifyEvent {
        response_type: CONFIGURE_NOTIFY_EVENT,
        sequence: 0,
        event: event.window,
        window: event.window,
        above_sibling: x11rb::NONE,
        x: geometry.x,
        y: geometry.y,
        width: geometry.width,
        height: geometry.height,
        border_width: geometry.border_width,
        override_redirect: false,
    };
    if let Err(e) = state
        .conn
        .send_event(false, event.window, EventMask::STRUCTURE_NOTIFY, notify)
    {
        log::error!("events::configure_request(..) send event error: {:?}", e);
    }
    if let Err(e) = state.conn.flush() {
        log::error!("events::configure_request(..) flush error: {:?}", e);
    }
}

/// Recompute panel space when a client changes its type or reservation.
pub fn property(state: &mut crate::state::State, event: PropertyNotifyEvent) {
    if [
        state.atoms._NET_WM_STRUT,
        state.atoms._NET_WM_STRUT_PARTIAL,
        state.atoms._NET_WM_WINDOW_TYPE,
    ]
    .contains(&event.atom)
    {
        crate::panels::refresh(state);
    }
}

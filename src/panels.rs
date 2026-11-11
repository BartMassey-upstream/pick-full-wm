//! EWMH panel reservations and stacking, in root-window coordinates.

use std::collections::HashMap;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt, EventMask,
    MapState, PropMode, StackMode, Window,
};
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

use crate::state::State;

/// A rectangle with an exclusive right and bottom edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn right(self) -> i32 {
        self.x + self.width
    }

    pub fn bottom(self) -> i32 {
        self.y + self.height
    }

    fn intersects(self, other: Self) -> bool {
        self.width > 0
            && self.height > 0
            && self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Dock,
    Desktop,
}

/// Read a client's preferred supported window type, independently of its title.
pub fn window_kind(state: &State, window: Window) -> Option<WindowKind> {
    for atom in property(
        state,
        window,
        state.atoms._NET_WM_WINDOW_TYPE,
        AtomEnum::ATOM,
    )? {
        if atom == state.atoms._NET_WM_WINDOW_TYPE_DOCK {
            return Some(WindowKind::Dock);
        }
        if atom == state.atoms._NET_WM_WINDOW_TYPE_DESKTOP {
            return Some(WindowKind::Desktop);
        }
        if atom == state.atoms._NET_WM_WINDOW_TYPE_NORMAL {
            return None;
        }
    }
    None
}

fn property(state: &State, window: Window, atom: Atom, type_: AtomEnum) -> Option<Vec<u32>> {
    let reply = state
        .conn
        .get_property(false, window, atom, type_, 0, 32)
        .ok()?
        .reply()
        .ok()?;
    if reply.type_ == u32::from(AtomEnum::NONE) {
        return None;
    }
    if reply.type_ != u32::from(type_) || reply.bytes_after != 0 {
        return Some(Vec::new());
    }
    Some(
        reply
            .value32()
            .map(|values| values.collect())
            .unwrap_or_default(),
    )
}

/// Four root-edge depths followed by the eight inclusive partial-strut bounds.
#[derive(Clone, Copy, Debug)]
struct Strut([u32; 12]);

impl Strut {
    fn read(state: &State, window: Window, root: Rect) -> Option<Self> {
        let partial = property(
            state,
            window,
            state.atoms._NET_WM_STRUT_PARTIAL,
            AtomEnum::CARDINAL,
        );
        let legacy = property(state, window, state.atoms._NET_WM_STRUT, AtomEnum::CARDINAL);
        Self::from_properties(partial.as_deref(), legacy.as_deref(), root)
    }

    fn from_properties(
        partial: Option<&[u32]>,
        legacy: Option<&[u32]>,
        root: Rect,
    ) -> Option<Self> {
        if let Some(values) = partial {
            // A malformed partial property must not accidentally enable a legacy reservation.
            return values.try_into().ok().map(Self);
        }
        let values: [u32; 4] = legacy?.try_into().ok()?;
        let last_x = root.width.saturating_sub(1) as u32;
        let last_y = root.height.saturating_sub(1) as u32;
        Some(Self([
            values[0], values[1], values[2], values[3], 0, last_y, 0, last_y, 0, last_x, 0, last_x,
        ]))
    }

    fn rectangles(self, root: Rect) -> [Rect; 4] {
        let v = self.0;
        let depth_x = |n: u32| n.min(root.width as u32) as i32;
        let depth_y = |n: u32| n.min(root.height as u32) as i32;
        let span = |start: u32, end: u32, limit: i32| {
            let start = start.min(limit as u32) as i32;
            let end = end.saturating_add(1).min(limit as u32) as i32;
            (start, (end - start).max(0))
        };
        let (left_y, left_height) = span(v[4], v[5], root.height);
        let (right_y, right_height) = span(v[6], v[7], root.height);
        let (top_x, top_width) = span(v[8], v[9], root.width);
        let (bottom_x, bottom_width) = span(v[10], v[11], root.width);
        [
            Rect {
                x: root.x,
                y: root.y + left_y,
                width: depth_x(v[0]),
                height: left_height,
            },
            Rect {
                x: root.right() - depth_x(v[1]),
                y: root.y + right_y,
                width: depth_x(v[1]),
                height: right_height,
            },
            Rect {
                x: root.x + top_x,
                y: root.y,
                width: top_width,
                height: depth_y(v[2]),
            },
            Rect {
                x: root.x + bottom_x,
                y: root.bottom() - depth_y(v[3]),
                width: bottom_width,
                height: depth_y(v[3]),
            },
        ]
    }
}

/// Intersect root-edge reservations with a monitor, without summing overlapping panels.
fn usable_area(monitor: Rect, root: Rect, struts: &[Strut]) -> Rect {
    let (mut left, mut right, mut top, mut bottom) =
        (monitor.x, monitor.right(), monitor.y, monitor.bottom());
    for strut in struts {
        let reserved = strut.rectangles(root);
        if reserved[0].intersects(monitor) {
            left = left.max(reserved[0].right());
        }
        if reserved[1].intersects(monitor) {
            right = right.min(reserved[1].x);
        }
        if reserved[2].intersects(monitor) {
            top = top.max(reserved[2].bottom());
        }
        if reserved[3].intersects(monitor) {
            bottom = bottom.min(reserved[3].y);
        }
    }
    // X11 windows must have a positive size even if a client reserves the entire screen.
    left = left.min(monitor.right() - 1);
    top = top.min(monitor.bottom() - 1);
    right = right.max(left + 1);
    bottom = bottom.max(top + 1);
    Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

/// Recompute reservations after mapping, withdrawal, destruction, or a strut change.
pub fn refresh(state: &mut State) {
    let Ok(geometry) = state
        .conn
        .get_geometry(state.root)
        .expect("Failed to query root size")
        .reply()
    else {
        return;
    };
    let root = Rect {
        x: 0,
        y: 0,
        width: geometry.width.into(),
        height: geometry.height.into(),
    };
    let Ok(tree) = state
        .conn
        .query_tree(state.root)
        .expect("Failed to query root children")
        .reply()
    else {
        return;
    };
    let mut special = HashMap::new();
    let mut struts = Vec::new();
    for window in tree.children {
        let Ok(attributes) = state
            .conn
            .get_window_attributes(window)
            .expect("Failed to query client")
            .reply()
        else {
            continue;
        };
        let _ = state
            .conn
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .expect("Failed to observe client properties")
            .check();
        if attributes.map_state == MapState::UNMAPPED || attributes.override_redirect {
            continue;
        }
        if let Some(kind) = window_kind(state, window) {
            special.insert(window, kind);
        }
        if let Some(strut) = Strut::read(state, window, root) {
            struts.push(strut);
        }
    }
    let mut changed = false;
    for &window in special.keys() {
        if crate::windows::core::find_window(state, window).is_some() {
            state.all_windows.remove(&window);
            crate::windows::core::release_window(state, window);
            changed = true;
        }
    }
    state.special_windows = special;
    for (index, monitor) in state.monitors.iter_mut().enumerate() {
        let area = usable_area(monitor.geometry, root, &struts);
        if area != monitor.workarea {
            monitor.set_workarea(&state.settings, index, area, &mut state.startup_warnings);
            changed = true;
        }
    }
    // EWMH has one rectangle per desktop; use a conservative root-wide rectangle there.
    let area = usable_area(root, root, &struts);
    let values: Vec<u32> = (0..state.settings.bindings.workspaces.len())
        .flat_map(|_| {
            [
                area.x as u32,
                area.y as u32,
                area.width as u32,
                area.height as u32,
            ]
        })
        .collect();
    state
        .conn
        .change_property32(
            PropMode::REPLACE,
            state.root,
            state.atoms._NET_WORKAREA,
            AtomEnum::CARDINAL,
            &values,
        )
        .expect("Failed to publish workarea");
    if changed {
        crate::windows::layout::relayout(state);
    }
    restack(state);
    state
        .conn
        .flush()
        .expect("Failed to flush workarea changes");
}

/// Keep desktop surfaces below clients and panels above ordinary tiled windows.
pub fn restack(state: &State) {
    for (&window, kind) in &state.special_windows {
        let mode = match kind {
            WindowKind::Desktop => StackMode::BELOW,
            WindowKind::Dock => StackMode::ABOVE,
        };
        state
            .conn
            .configure_window(window, &ConfigureWindowAux::new().stack_mode(mode))
            .expect("Failed to stack desktop window");
    }
    for monitor in &state.monitors {
        let workspace = &monitor.workspaces[state.current_workspace];
        if workspace.is_fullscreen()
            && let Some(window) = workspace.main_window
        {
            state
                .conn
                .configure_window(
                    window,
                    &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
                )
                .expect("Failed to raise fullscreen window");
            // Dialogs and help remain usable above a fullscreen client.
            for floating in workspace
                .floatings
                .iter()
                .copied()
                .chain(workspace.help_window)
            {
                state
                    .conn
                    .configure_window(
                        floating,
                        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
                    )
                    .expect("Failed to raise fullscreen dialog");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: Rect = Rect {
        x: 0,
        y: 0,
        width: 2304,
        height: 1024,
    };

    #[test]
    fn partial_strut_uses_root_coordinates_on_shorter_monitor() {
        let left = Rect {
            width: 1280,
            ..ROOT
        };
        let right = Rect {
            x: 1280,
            y: 0,
            width: 1024,
            height: 768,
        };
        let strut = Strut([0, 0, 0, 306, 0, 0, 0, 0, 0, 0, 1280, 2303]);
        assert_eq!(usable_area(left, ROOT, &[strut]), left);
        assert_eq!(
            usable_area(right, ROOT, &[strut]),
            Rect {
                height: 718,
                ..right
            }
        );
    }

    #[test]
    fn inclusive_range_can_reserve_the_last_pixel() {
        let strut = Strut([0, 0, 20, 0, 0, 0, 0, 0, 2303, 2303, 0, 0]);
        assert_eq!(
            usable_area(ROOT, ROOT, &[strut]),
            Rect {
                y: 20,
                height: 1004,
                ..ROOT
            }
        );
    }

    #[test]
    fn overlapping_reservations_use_maximum_and_partial_takes_precedence() {
        let legacy = [0, 0, 50, 0];
        let partial = [0, 0, 30, 0, 0, 0, 0, 0, 0, 2303, 0, 0];
        let strut = Strut::from_properties(Some(&partial), Some(&legacy), ROOT).unwrap();
        let other = Strut::from_properties(None, Some(&[0, 0, 20, 0]), ROOT).unwrap();
        assert_eq!(
            usable_area(ROOT, ROOT, &[strut, other]),
            Rect {
                y: 30,
                height: 994,
                ..ROOT
            }
        );
    }

    #[test]
    fn legacy_struts_span_both_monitors() {
        let strut = Strut::from_properties(None, Some(&[0, 0, 25, 0]), ROOT).unwrap();
        let right = Rect {
            x: 1280,
            y: 0,
            width: 1024,
            height: 768,
        };
        assert_eq!(
            usable_area(right, ROOT, &[strut]),
            Rect {
                y: 25,
                height: 743,
                ..right
            }
        );
    }

    #[test]
    fn all_four_edges_leave_the_expected_interior() {
        let strut = Strut::from_properties(None, Some(&[20, 30, 40, 50]), ROOT).unwrap();
        assert_eq!(
            usable_area(ROOT, ROOT, &[strut]),
            Rect {
                x: 20,
                y: 40,
                width: 2254,
                height: 934
            }
        );
    }

    #[test]
    fn malformed_or_extreme_properties_do_not_create_invalid_geometry() {
        assert!(Strut::from_properties(Some(&[1, 2]), Some(&[0, 0, 50, 0]), ROOT).is_none());
        assert!(Strut::from_properties(None, Some(&[1, 2, 3]), ROOT).is_none());
        let all = Strut::from_properties(None, Some(&[u32::MAX; 4]), ROOT).unwrap();
        let area = usable_area(ROOT, ROOT, &[all]);
        assert!(area.width > 0 && area.height > 0);
        assert!(area.right() <= ROOT.right() && area.bottom() <= ROOT.bottom());
    }
}

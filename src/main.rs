use x11rb::connection::Connection;
use x11rb::protocol::Event;

mod atoms;
mod binaries;
mod calc;
mod events;
mod ewmh;
mod keymap;
mod panels;
mod safety;
mod settings;
mod setup;
mod state;
mod windows;

fn main() {
    let mut state = state::State::init();
    setup::internal::logging(&state);
    setup::internal::dbus_init();
    setup::ewmh::init(&mut state);
    setup::internal::custom_startups(&mut state);
    setup::input::mouse(&mut state);
    setup::input::keys(&mut state);
    setup::internal::windows(&mut state);
    panels::refresh(&mut state);
    setup::internal::startups(&mut state);
    log::info!("Finished with initialization.");

    loop {
        let event = state
            .conn
            .wait_for_event()
            .expect("Failed to wait for event");
        // ignored events (mostly notifications of our own moves/maps) skip the per-event work
        if !matches!(
            event,
            Event::MapRequest(_)
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

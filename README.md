# pick-full-wm

`pick-full-wm` is an X11 window manager written in Rust using
the [x11rb](https://docs.rs/x11rb/) protocol bindings.

The primary workflow is to focus on one nearly full screen window at a time, with the ability to quickly switch to other windows preview-able on the side of the screen.

The name is lightly inspired by [Minecraft](https://www.minecraft.net/en-us)'s ["Pick Block"](https://minecraft.fandom.com/wiki/Controls) key-binding that allows the user to instantly switch to any visible block.

# Demo

https://github.com/user-attachments/assets/758ea78a-3893-43a3-b48f-3e881a7a093e

# Usage

Build the window manager and all helper programs together:

```sh
cargo build --release --locked --bins
```

The executable is `target/release/pick-full-wm`. If installing it
elsewhere, keep `help_window`, `key_hint`, `close_box`, and
`monitor_box` in the same directory.

For two independent XFCE logins under the same account, see
[XFCE and LightDM setup](docs/xfce-lightdm.md). The guide keeps
the existing XFCE/xfwm4 entry and adds a separate Pick-Full
profile using the installed XFCE components.

There two primary ways (that I know of) to launch into `pick-full-wm`.

The first is via an appropriate `.xinitrc` file from a TTY session via the [`startx`](https://manpages.debian.org/buster/xinit/startx.1.en.html) command: it should be placed directly at your user folder (`~` / `/home/username/`). An example `.xinitrc` is found in the root of this repository.

The second is from a display manager such as [`SDDM`](https://github.com/sddm/sddm), which looks for an appropriate [`.desktop` file](https://wiki.archlinux.org/title/Desktop_entries) in `/usr/share/xsessions/`, and can be loaded from some window manager menu (bottom left in SDDM's interface). An example `pick-full.desktop` is found in the root of this repository.

The next layer is configuring your `settings.toml` file. There is are two examples in the root of this repository: `settings-mouse.toml` and `settings-keyboard.toml`. Choose your preferred starter, copy it as "settings.toml" into `~/.config/pick-full-wm/`, and start editing as desired.

The current mouse defaults use [`xfce4-panel`](https://packages.debian.org/trixie/xfce4-panel) for both launcher and status bar. The current keyboard defaults use [`rofi`](https://github.com/davatorium/rofi) as an application launcher and [`polybar`](https://github.com/polybar/polybar) as a status bar. Both use `brightnessctl` for brightness control and `wpctl` for volume control. These are very much dynamic and change-able, though, of course: just startup and exclude clarifications.

By default, both settings place their log files in `~/.local/share/pick-full-wm/logs/`. A new log file is placed here upon every startup of the window manager: named along the lines of `DATE-TIME.log`.

# Replacing a running window manager

From an existing X11 session, request a cooperative handoff:

```sh
./target/release/pick-full-wm --replace &
```

Without `--replace`, startup refuses to displace an existing WM.
Replacement uses the screen's ICCCM `WM_Sn` selection and waits
up to five seconds for the previous owner window to disappear.
If it does not cooperate, or another client still owns root
redirection, startup fails without killing the old WM's clients.

Existing clients, including iconic windows, are adopted and
retiled on the first workspace. The old WM's layout and workspace
assignments are not imported. Pick-Full also cooperates when
another WM replaces it, exposing clients from hidden workspaces
before releasing control.

For an existing XFCE session, pass `--no-session-services` and
set `applications.startups = []`
in the Pick-Full configuration so its default startup commands
do not duplicate desktop components. XFCE can continue running
its panel, desktop, settings daemon, and session manager. A fresh
LightDM session should start only its chosen WM; `--replace` is
for switching an already running session.

`--no-session-services` skips publishing D-Bus/systemd activation
variables, clearing the shared Wayland environment, and restarting
portal services. Use it when XFCE or another session manager owns
those services, including an isolated XFCE profile.

# Panels

Panels that advertise EWMH struts automatically reserve space on
the affected monitors. Dock and desktop windows are recognized
by `_NET_WM_WINDOW_TYPE`; they do not need title exclusions.
Reservations update when panels appear, disappear, or change
their struts (including auto-hide). `_NET_WM_STRUT_PARTIAL` takes
precedence over the older `_NET_WM_STRUT` property.

Layout percentages and `top_left` offsets apply inside the usable
area. Remove old manual panel gaps to avoid reserving space twice.
Fullscreen still covers the physical monitor, including panels.
The root `_NET_WORKAREA` property reports one conservative
rectangle per workspace; tiling uses each monitor's own area.

# Monitor configuration

`layout.main_size` and `layout.top_left` are independent lists of
per-monitor overrides, ordered from left to right. If there are
more monitors than entries, the last entry repeats. One entry
therefore configures any number of monitors. An omitted or empty
list uses `80%x100%` for `main_size` or `0,0` for `top_left`.

```toml
[layout]
main_size = ["80%x100%"]
top_left = ["0,0"]
conditional_full = true
```

# Testing

Run unit tests with `cargo test --locked`. X11 integration tests
use isolated displays and require `Xvfb`:

```sh
cargo test --locked --test x11 -- --ignored
```

# Features and TODOs

- [x] Splits area in "main" and "side"
- [x] "Side" area auto lays out all non-main windows
- [x] Window for "main" area swappable with left click
- [x] "Side" windows close-able with right click
- [x] Configurable `settings.toml` from user `.config` folder
- [x] Set-able list of startup applications
- [x] Set-able list of applications to exclude from managing
- [x] Set-able "launcher" application to run with Super key
- [x] Set-able size of "main" area via percent or pixels
- [x] Implementation of x11 error handling
- [x] Keyboard options for window switching
- [x] Center mouse within and focus on main window on swap
- [x] Toggle-able "full screen" mode
- [x] Customizable position of area
- [x] Keybind to open integrated help window
- [x] Keybinding visual "hints" for side windows
- [x] Work with Extended Window Manager Hints
- [x] Built-in multi-monitor support
- [x] Tiling exceptions for pop-up style windows
- [x] Some implementation of virtual workspaces
- [x] Set new windows to main space (optional)
- [x] Dynamic keybinds to run any command
- [x] Ability to move main to different workspace
- [x] Mouse-driven way to close main window
- [x] Support for close and move-workspace client messages
- [x] Ability to move windows between monitors
- [x] Mouse driven way to move across monitors
- [x] Logging of basic functionality
- [x] Configurable side positioning for side windows
- [ ] Support for monitor rotation
- [ ] Way to move floating (pop-up) windows
- [ ] Integration with "cargo deb"
- [ ] Support for screenshot and capture tools
- [ ] Extra support for keybind settings (like "X + Y")
- [ ] Media keys shouldn't need SUPER held
- [ ] Support for horizontal rather than vertical side windows

One possible future direction is a preview system for side
windows instead of directly changing their size and position.

# Credit

This was primarily written by Benjamin Massey benjamin.w.massey@gmail.com

With xlib (x11 crate) => xcb (x11rb crate) conversion help by Bart Massey bart@cs.pdx.edu

# License

This project is licensed under the [GNU General Public License (GPLv3)](https://www.gnu.org/licenses/gpl-3.0.en.html).

A copy of said license can be found in `LICENSE.md`.

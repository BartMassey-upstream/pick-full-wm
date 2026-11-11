# Independent XFCE choices in LightDM

Keep the existing XFCE login and add **XFCE (Pick-Full)** for
the same Unix account. Switch by logging out, selecting the
other entry in LightDM, and logging back in. Both choices use
the installed XFCE programs, with separate personal desktop
settings and saved sessions.

This is a setup recommendation, not an installed configuration.
The development environment does not have LightDM and XFCE
installed together, so a real login test is still required on
the desktop machine. The startup details below were checked
against upstream XFCE 4.20; check the installed distribution's
files before applying them.

## What stays separate

The existing LightDM entry continues to launch its packaged
XFCE session with `xfwm4`. Its launcher, XFCE configuration,
autostarts, and saved-session cache stay as they are.

The new entry launches the installed `xfce4-session` with:

- Pick-Full as its only window manager;
- the usual settings daemon, panel, desktop, and file manager;
- its own configuration, cache, data, and state directories;
- a private D-Bus session, including its own `xfconfd`;
- its own ICE session-authentication file.

Separate XDG directories route XFCE settings and session saves
to the new profile. A private bus prevents its settings tools
from attaching to the daily session's configuration daemon.
[XFCE documents its configuration and cache locations][xfce-env];
[D-Bus documents the private bus lifecycle][dbus-run].

This protects the two XFCE profiles; it is not a sandbox for
every program run under the account. Both logins still share
the real home directory, documents, runtime directory, and
systemd user manager. Programs that ignore XDG paths can share
their own preferences, browser profiles, or credentials.
LightDM can also update ordinary login bookkeeping, such as
the last selected session and login logs.

Use one graphical session for this account at a time. **Fully
log out before switching**, rather than leaving either session
running through fast user switching. Check the profile boundary
with the round-trip test below before relying on the new login.

## Installed software and paths

Reuse the distribution's LightDM, Xorg, XFCE, and D-Bus packages.
The only new executable code should be Pick-Full, its helper
programs, and the small launcher below. No patched XFCE build or
replacement `xfwm4` executable is needed.

Check the desktop machine's actual commands and session entry:

```sh
command -v xfce4-session xfsettingsd xfce4-panel xfdesktop
command -v Thunar dbus-run-session iceauth xfconf-query
command -v xfce4-appfinder xfce4-terminal
xfce4-session --version
rg -n '^(Name|Exec|TryExec|DesktopNames)=' \
  /usr/share/xsessions/*.desktop
```

The examples assume `/usr/bin/xfce4-session`,
`/usr/bin/dbus-run-session`, and a home of `/home/bart`.
Substitute the paths actually found on the desktop machine.
If a launcher or terminal is missing, choose an already installed
equivalent in the Pick-Full settings.

Use this new profile tree:

```text
~/.local/share/xfce-pick-full/
  start-session
  bin/
    pick-full-wm
    help_window
    key_hint
    close_box
    monitor_box
  config/
  cache/
  data/
  state/
```

Keep `HOME` and the LightDM-provided `XDG_RUNTIME_DIR` unchanged.
The runtime directory belongs to the login infrastructure and
contains services such as audio sockets; it is not an extra
profile directory. Keep the system XDG search directories so
installed applications, icons, themes, and defaults remain
available. See the [XDG directory specification][xdg].

Build with `cargo build --release --locked --bins`. Copy the
five release executables into this profile's `bin/`, keeping
them together. Use stable copies rather than executable paths
under `target/`, where `cargo clean` could remove them. Create
the profile directories with user-only permissions, for example
under `umask 077`. Treat an existing profile as data to preserve,
not as something to overwrite during preparation.

## Prepare a separate XFCE session configuration

Do this offline, while the new profile has no running session or
`xfconfd`. Configuration edits belong only in its `config/`
directory. The [Xfconf documentation][xfconf] explains why manual
XML edits and a running settings daemon should not be mixed.

Locate the installed system `xfce4-session.xml` through the
effective `XDG_CONFIG_DIRS`. A common location is:

```text
/etc/xdg/xfce4/xfconf/xfce-perchannel-xml/xfce4-session.xml
```

Some distributions put an override earlier in that search path.
Use the effective distribution configuration, including any
inherited session definitions, rather than downloading another
distribution's file. Save an ordinary copy under:

```text
~/.local/share/xfce-pick-full/config/xfce4/xfconf/xfce-perchannel-xml/xfce4-session.xml
```

Start with an empty **new profile** cache. Leave the existing
XFCE saved sessions, usually `~/.cache/sessions`, untouched.

In the copied XML:

1. Read `/general/FailsafeSessionName` and find its matching
   `/sessions/<name>` group.
2. Find the `ClientN_Command` array that launches `xfwm4`.
   Replace that whole array with the absolute Pick-Full path
   and `--no-session-services`, as shown below. Omit `--replace`.
   Keep the client's priority and the other distribution-supplied
   clients and arguments.
3. Set `/general/SessionName` to `PickFull` and
   `/general/SaveOnExit` to boolean `false`. Keep the original
   `FailsafeSessionName`: it chooses the startup client list
   when there is no saved session.
4. Set `/startup/gpg-agent/enabled` and
   `/startup/ssh-agent/enabled` to boolean `false`.
   Also set `/compat/LaunchGNOME` and `/compat/LaunchKDE` to
   boolean `false` for the initial profile.

For example, if the WM is `Client0`, its replacement is:

```xml
<property name="Client0_Command" type="array">
  <value type="string"
    value="/home/bart/.local/share/xfce-pick-full/bin/pick-full-wm"/>
  <value type="string" value="--no-session-services"/>
</property>
```

XML does not expand `$HOME` or `~`. Use a literal absolute path.
The `--no-session-services` argument is required for this setup.
Ordinary standalone Pick-Full startup updates D-Bus/systemd
activation variables, unsets the shared Wayland display, and
requests restarts of running desktop portal services. This flag
skips those actions so XFCE and the private session bus retain
control of their environment. It does not disable separately
configured startup commands or XFCE autostarts.

Do not assume the WM is `Client0` on every distribution.
XFCE loads saved sessions before falling back to these clients;
an independent empty cache is part of selecting the right WM.
See the [session manager's startup configuration][xfsm-manager].

Merge these properties into the existing XML groups, creating
missing groups as needed. Do not add a second conflicting copy
of a property:

```xml
<property name="startup" type="empty">
  <property name="gpg-agent" type="empty">
    <property name="enabled" type="bool" value="false"/>
  </property>
  <property name="ssh-agent" type="empty">
    <property name="enabled" type="bool" value="false"/>
  </property>
</property>
<property name="compat" type="empty">
  <property name="LaunchGNOME" type="bool" value="false"/>
  <property name="LaunchKDE" type="bool" value="false"/>
</property>
```

These agent settings matter for isolation. In upstream 4.20,
XFCE's GPG-agent startup can stop an existing GPG agent, and its
SSH-agent startup can import an environment variable into the
shared systemd user manager. Disabling that management leaves
existing login-provided agents to their normal owners. This
does not disable agents launched independently by applications
or autostarts. See [XFCE's agent startup code][xfsm-startup].

### Familiar desktop settings

Start with installed XFCE defaults. If desired, take a one-time
copy of selected daily XFCE preferences and panel launchers
while the daily desktop is logged out. Exclude its session
channel, saved-session cache, and custom `xinitrc`.

Use independent files, not symlinks or hardlinks. Review copied
panel launchers, scripts, and absolute paths: a copied file can
still point at another profile's writable files. Copy any needed
user icons or launcher data into the new profile as well.
There should be no automatic copying or synchronization at
login or logout. Subsequent changes belong to the active profile.

### Autostarts need a separate review

System autostarts still apply through `XDG_CONFIG_DIRS` even
when the new profile's `config/autostart/` is empty. Review the
effective entries and any copied user entries before first use.
Look especially for extra WMs, dotfile synchronizers, agent
restarts, and commands that call
`systemctl --user import-environment`
or `dbus-update-activation-environment --systemd`.

Disable an unwanted system entry only in this profile by placing
an override with the same basename in `config/autostart/`:

```ini
[Desktop Entry]
Type=Application
Name=Disabled in the Pick-Full profile
Hidden=true
```

The [autostart specification][autostart] defines this override.
Retain the installed desktop services you need, such as the
policy authentication agent, power manager, and screen locker,
after checking their behavior. A private bus can affect programs
that expect the usual bus or systemd-activated services. Test
those integrations rather than exporting the private
profile's environment into the shared user manager.

## Pick-Full settings

Pick-Full currently reads a fixed path:
`~/.config/pick-full-wm/settings.toml`. It does **not** honor
`XDG_CONFIG_HOME` for this file. Normal `xfwm4` does not use it,
so it can remain Pick-Full-specific without changing XFCE's
daily settings. Preserve and review it if it already exists.

For a new file, copy `settings-keyboard.toml` or
`settings-mouse.toml` from the repository as `settings.toml`.
Use these values in the matching sections, replacing the
example's entire `[bindings.functions]` table. This uses XFCE's
installed components and keeps WM logs in the private profile:

```toml
[applications]
startups = []
excluded = []

[layout]
main_size = ["80%x100%"]
top_left = ["0,0"]
conditional_full = true
close_box = false
monitor_box = false

[bindings.functions]
space = "xfce4-appfinder"
t = "xfce4-terminal"
e = "Thunar"

[files]
log_directory = "~/.local/share/xfce-pick-full/state/wm-logs"
```

Keep the example's `[bindings]` navigation settings. Its other
layout settings can remain as supplied. The older
`applications.launcher` and `applications.terminal` keys are no
longer used; commands now belong in `[bindings.functions]`.
Missing settings receive built-in defaults, and malformed TOML
falls back to all defaults. Check for a settings-load warning in
the session log and the WM's `state/wm-logs/` directory.

XFCE starts the panel and desktop; an empty `startups` list
avoids adding Polybar or another launcher. Dock/desktop window
types and panel reservations are now handled automatically.
Manual panel gaps and title exclusions are unnecessary for
standard XFCE panels. One layout entry covers additional
monitors by repeating its value.

## The new session launcher

Save this as `~/.local/share/xfce-pick-full/start-session` and
make it executable. The absolute system paths must match the
earlier command checks. Run this only as a fresh LightDM login,
not from a terminal in either running desktop.

```sh
#!/bin/sh
set -eu
umask 077

if [ -z "${DISPLAY:-}" ] || [ -n "${SESSION_MANAGER:-}" ]; then
    echo 'Use a fresh LightDM X11 login for this profile.' >&2
    exit 1
fi
if [ "${XDG_SESSION_TYPE:-x11}" != x11 ]; then
    echo 'Pick-Full requires an X11 session.' >&2
    exit 1
fi

pf_profile="$HOME/.local/share/xfce-pick-full"
export XDG_CONFIG_HOME="$pf_profile/config"
export XDG_CACHE_HOME="$pf_profile/cache"
export XDG_DATA_HOME="$pf_profile/data"
export XDG_STATE_HOME="$pf_profile/state"
export XDG_CONFIG_DIRS="${XDG_CONFIG_DIRS:-/etc/xdg}"
XDG_DATA_DIRS="${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export XDG_DATA_DIRS
export XDG_CURRENT_DESKTOP=XFCE
export XDG_MENU_PREFIX=xfce-
export DESKTOP_SESSION=xfce-pick-full
export GDK_BACKEND=x11
export ICEAUTHORITY="$XDG_STATE_HOME/ICEauthority"
unset XFSM_VERBOSE

mkdir -p "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" \
    "$XDG_DATA_HOME" "$XDG_STATE_HOME"

pf_channels="$XDG_CONFIG_HOME/xfce4/xfconf/xfce-perchannel-xml"
test -r "$pf_channels/xfce4-session.xml"
test -r "$HOME/.config/pick-full-wm/settings.toml"
test -x "$pf_profile/bin/pick-full-wm"
test -x "$pf_profile/bin/help_window"
test -x "$pf_profile/bin/key_hint"
test -x "$pf_profile/bin/close_box"
test -x "$pf_profile/bin/monitor_box"

exec >>"$XDG_STATE_HOME/session.log" 2>&1
exec /usr/bin/dbus-run-session -- \
    /usr/bin/xfce4-session --disable-tcp
```

The private bus inherits the profile paths before any XFCE
settings service is activated. `dbus-run-session` sets the new
bus address and ends the bus when `xfce4-session` exits.
Keep LightDM's `DISPLAY`, `XAUTHORITY`, and login/session
metadata; the separate `ICEAUTHORITY` is for XFCE session
management. Keep `XFSM_VERBOSE` unset: it would write a shared
legacy log in the home directory instead of this profile's log.

Launching `xfce4-session` directly still provides the session
manager, configured desktop clients, autostarts, and logout.
The packaged session manager
[supports display-manager use][xfsm-man].
It bypasses `startxfce4` and XFCE's `xinitrc`: upstream's latter
script can publish the private XDG paths to the shared systemd
user environment. See the [upstream startup script][xinitrc].

This also skips that script's ancillary setup, including X
resource/keymap loading and user-directory updates. Check the
installed script for distribution-specific requirements. Add
only a needed, reviewed action to this private launcher; avoid
copying the whole script or changing the daily startup files.
LightDM's own system login wrapper still runs before this
launcher, so existing `.profile` or `.xsessionrc` hooks also
need review if they customize desktop startup.

## Add one LightDM choice

Prepare a new file named `xfce-pick-full.desktop`:

```ini
[Desktop Entry]
Name=XFCE (Pick-Full)
Comment=XFCE with independent Pick-Full settings
Type=Application
Exec=/home/bart/.local/share/xfce-pick-full/start-session
TryExec=/usr/bin/xfce4-session
DesktopNames=XFCE;
```

Use the real literal home path in `Exec`; desktop entries do
not expand shell variables or `~`. This example is specific to
that account. The session entry may be visible to other accounts
even though its launcher is intended for this one.

Once the profile is prepared and checked, installing this new
file into LightDM's existing X11 session directory is the only
required system-wide addition, commonly:

```text
/usr/share/xsessions/xfce-pick-full.desktop
```

Use a new, unoccupied filename, root ownership, and mode `0644`.
This installation is a separate administrative action; the
documentation change does not perform it. LightDM's configured
`sessions-directory` determines the location; its
[upstream defaults include `/usr/share/xsessions`][lightdm].

Keep the existing XFCE entry, LightDM defaults, autologin, and
session-wrapper configuration unchanged. Do not deploy this
repository's older `pick-full.desktop` as-is: it points at the
original author's paths and does not start this full profile.
Leave LightDM running; restarting it would terminate the current
graphical session. Check the chooser after a normal logout.
LightDM may remember the last choice, so explicitly select the
normal XFCE entry when returning to the daily desktop.

## Verify a round trip before daily use

1. Record the daily profile's actual XDG locations. After a
   normal logout, use a text console to back up and record file
   hashes for its XFCE configuration and saved-session cache.
   Include any daily panel launcher data. Record the new
   profile's initial files separately. Avoid taking the
   baseline while the daily desktop is still saving settings.
2. Select **XFCE (Pick-Full)** in LightDM. In a terminal opened
   there, confirm Pick-Full was started with
   `--no-session-services`. Check the four XDG home variables
   and `ICEAUTHORITY` point into
   `~/.local/share/xfce-pick-full`. Check
   `DBUS_SESSION_BUS_ADDRESS` names the private bus rather than
   the usual `/run/user/<uid>/bus`. Inspect the new session log
   and confirm the WM is Pick-Full, with no second WM running
   on this display. If installed, `xprop -root
   _NET_SUPPORTING_WM_CHECK` identifies the check window; query
   that window's `_NET_WM_NAME` to identify the WM.
3. Confirm the XFCE panel, desktop, app launcher, terminal,
   window switching, panel reservations, and each monitor work.
   Test locking/unlocking, audio, notifications, removable
   media, authentication prompts, and logout. Save one harmless
   panel preference to establish that this profile keeps changes.
4. Log out normally. From the text console, compare the daily
   profile against the baseline **before logging into daily
   XFCE again**. Its desktop configuration and saved-session
   files should be unchanged. Investigate any unexpected write
   before proceeding; normal LightDM logs are outside this check.
5. Snapshot the new profile's persistent configuration, then
   log into the original XFCE entry. Confirm `xfwm4` and the
   original desktop are intact. Log out and verify the new
   profile's persistent configuration stayed unchanged. Log
   into Pick-Full once more to confirm its preference survived.

Keep automatic session saving off initially, and leave the
logout dialog's save-session option unchecked. If enabling
session saving later, do it only in the new profile and confirm
it writes beneath that profile's `cache/sessions/`.

## Recovery and removal

If the new login fails, return to LightDM and select the original
XFCE entry. Inspect the new profile's `state/session.log`,
`state/wm-logs/`, and the relevant login log. If startup loops,
use a text console to remove the added session entry before
trying again.

A saved-session reset, if ever needed, applies only to
`~/.local/share/xfce-pick-full/cache/sessions/`, with the profile
logged out. Move that directory aside for inspection instead
of deleting it. The daily cache is never part of this reset.

To retire the setup, remove only the added LightDM entry and
archive the private profile. Preserve the Pick-Full settings
file if it predates this setup or is used elsewhere. Returning
to stock XFCE requires no WM reinstall, settings restoration,
global process killing, or changes to the original session.

[xfce-env]: https://docs.xfce.org/xfce/xfce4-session/advanced
[dbus-run]: https://manpages.debian.org/trixie/dbus-daemon/dbus-run-session.1.en.html
[xdg]: https://specifications.freedesktop.org/basedir/latest/
[xfconf]: https://docs.xfce.org/xfce/xfconf/start
[xfsm-manager]: https://gitlab.xfce.org/xfce/xfce4-session/-/blob/xfce-4.20/xfce4-session/xfsm-manager.c
[xfsm-startup]: https://gitlab.xfce.org/xfce/xfce4-session/-/blob/xfce-4.20/xfce4-session/xfsm-startup.c
[autostart]: https://specifications.freedesktop.org/autostart/latest/
[xfsm-man]: https://manpages.debian.org/trixie/xfce4-session/xfce4-session.1.en.html
[xinitrc]: https://gitlab.xfce.org/xfce/xfce4-session/-/blob/xfce-4.20/scripts/xinitrc.in
[lightdm]: https://github.com/ubuntu/lightdm/blob/main/data/lightdm.conf

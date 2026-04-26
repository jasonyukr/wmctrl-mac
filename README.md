# wmctrl-mac

Compact `yabai` shim for scripts that only need a small query/focus subset on macOS.

## Supported commands

- `wmctrl-mac --help`
- `wmctrl-mac -h`
- `wmctrl-mac -m query --spaces`
- `wmctrl-mac -m query --windows`
- `wmctrl-mac -m query --windows --space <index>`
- `wmctrl-mac -m window --focus <id>`
- `wmctrl-mac -m listwnd [-s] [space]`
- `wmctrl-mac listwnd [-s] [space]`
- `wmctrl-mac -m focus-next-window`
- `wmctrl-mac -m focus-prev-window`
- `wmctrl-mac -m focus-other-next-window`
- `wmctrl-mac -m focus-other-prev-window`
- `wmctrl-mac -m send-to-back`
- `wmctrl-mac -m launch-or-focus <app name>`

This is not a full yabai replacement. Other yabai commands, selectors, filters, multi-space behavior, rules, layouts, and window management actions are unsupported.

## Space and window model

The shim reports one synthetic space: space `1`. Window queries enumerate currently on-screen CoreGraphics windows only, so windows on other Mission Control spaces are not visible until their space is active.

`wmctrl-mac -m listwnd` prints listwnd-compatible lines as `<space> <has_focus> <id> "<app>"`. `wmctrl-mac listwnd` is also accepted for convenience. Use `-s` to sort by window id ascending; a valid space filter currently means `1`, while invalid, non-numeric, or out-of-range values list all spaces.

## Build and install

Build the `wmctrl-mac` binary with Cargo:

```sh
cargo build --release
```

Install or copy `target/release/wmctrl-mac` to the path your scripts call.

## Accessibility permission

`wmctrl-mac -m window --focus <id>` uses macOS Accessibility APIs. On first focus attempt, macOS may prompt for permission; approve the final installed `wmctrl-mac` binary in System Settings > Privacy & Security > Accessibility.

`wmctrl-mac -m focus-next-window` and `wmctrl-mac -m focus-prev-window` cycle focus between windows from the currently focused app on the focused space.

`wmctrl-mac -m focus-other-next-window` and `wmctrl-mac -m focus-other-prev-window` cycle focus between one representative window per app on the focused space, remembering the last focused window for each app in `${TMPDIR:-/tmp}`.

`wmctrl-mac -m send-to-back` sends the focused Accessibility window behind other compatible windows by raising those windows in current stacking order. If no focused window is found, it exits successfully without changing window focus.

`wmctrl-mac -m launch-or-focus <app name>` focuses a running app with a matching localized name, or asks macOS to launch the app by name. App names may be passed as one shell argument or multiple trailing arguments joined with spaces; paths are not supported.

Accessibility trust is tied to the exact binary path. If you move, reinstall, or switch from a debug build to an installed release binary, grant permission again for that final path.

# HeroPortal

The open and save dialogs of [HeroineOS](https://github.com/HeroineOS), for every app that
asks the desktop portal for them: Firefox, Chromium, Flatpak apps, and GTK/Qt apps set to
use portals. Built on [HeroUI](https://github.com/HeroineOS/HeroUI) (fltk-rs).

- **A dialog of the app that asked:** on HeroWM (and sway, KDE...) it floats, centered over
  that app's window, instead of being tiled next to it and pushing it aside. (The app's
  window is its parent through xdg-foreign, set before the dialog first shows.)
- **Light:** nothing runs until an app asks. D-Bus starts the backend (0.9 MB of its own
  memory), each dialog is a process of its own (4.3 MB of its own, 20 MB RSS with shared
  libraries), and the backend leaves a minute after the last dialog closes. Measured on
  sway.
- **Open, save, save several, pick folders;** the app's file type filters and button
  label; several files at once where the app allows it; replacing a file asks first.
- **Places:** Home and your folders (Desktop, Documents, Downloads, Music, Pictures,
  Videos, as `~/.config/user-dirs.dirs` names them), and the whole computer.
- **Keyboard:** arrows, Page Up/Down, Home/End move; Enter opens a folder or chooses;
  Backspace or Alt+Up goes up; Ctrl+H shows hidden files; Escape cancels. Type a folder or
  a full path in the path field (`~` works); in a save dialog, the name field takes a full
  path too.
- **Mouse:** click, Ctrl+click and Shift+click to select, double-click to open.

## Install

Debian packages for amd64 and arm64 are on the
[releases](https://github.com/HeroineOS/HeroPortal/releases) page. Then restart the portal so
it picks HeroPortal:

```sh
sudo apt install ./heroportal_*_arm64.deb
systemctl --user restart xdg-desktop-portal
```

On HeroWM the package sets it up: it installs `/etc/xdg/xdg-desktop-portal/fht-compositor-portals.conf`,
HeroWM's portal choices plus `FileChooser=hero`. Elsewhere, add to
`~/.config/xdg-desktop-portal/portals.conf` (or `DESKTOP-portals.conf`):

```ini
[preferred]
org.freedesktop.impl.portal.FileChooser=hero
```

**Firefox** uses the portal's dialogs only when told to, outside Flatpak/Snap: in
`about:config`, set `widget.use-xdg-desktop-portal.file-picker` to `1`.

The portal (and HeroPortal) need to know the Wayland display: start HeroWM with
`--session`, or run `dbus-update-activation-environment --systemd WAYLAND_DISPLAY
XDG_CURRENT_DESKTOP` once the session is up.

## Try it without an app

```sh
heroportal open --multiple    # prints the chosen paths
heroportal save --name notes.txt
```

## Planned

- Recent files, bookmarks, a grid with thumbnails (shared with HeroWallpaper's cache).
- More portal pieces: dark/light preference from HeroUI's theme (Settings), screenshots.

## Building

```sh
sudo apt install build-essential cmake pkg-config libdbus-1-dev libwayland-dev wayland-protocols \
  libxkbcommon-dev libpango1.0-dev libcairo2-dev libx11-dev libxext-dev libxft-dev \
  libxinerama-dev libxcursor-dev libxrender-dev libxfixes-dev libgl-dev
cargo build --release
```

It builds for x86_64, arm64, i686 and armv7.

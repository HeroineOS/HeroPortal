//! `heroportal status` and `heroportal use`: whether the desktop portal
//! offers HeroPortal's dialogs, and making it so. xdg-desktop-portal picks a
//! backend per interface from the first config file it finds (the user's
//! folder first; in each folder `<desktop>-portals.conf`, then
//! `portals.conf`). Without a FileChooser line for HeroPortal it uses GTK's.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use dbus::blocking::Connection;

const KEY: &str = "org.freedesktop.impl.portal.FileChooser";
const BACKEND: &str = "hero";

fn env_dirs(var: &str, default: &str) -> Vec<PathBuf> {
    let v = std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_owned());
    v.split(':').filter(|p| p.starts_with('/')).map(PathBuf::from).collect()
}

fn user_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
        .join("xdg-desktop-portal")
}

fn desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(str::to_lowercase).collect()
}

/// The config files in the order xdg-desktop-portal looks at them.
fn candidates() -> Vec<PathBuf> {
    let mut dirs = vec![user_dir()];
    dirs.extend(env_dirs("XDG_CONFIG_DIRS", "/etc/xdg").into_iter().map(|d| d.join("xdg-desktop-portal")));
    dirs.push(PathBuf::from("/etc/xdg-desktop-portal"));
    dirs.extend(env_dirs("XDG_DATA_DIRS", "/usr/local/share:/usr/share").into_iter().map(|d| d.join("xdg-desktop-portal")));
    let mut v = Vec::new();
    for d in dirs {
        for desk in desktops() {
            v.push(d.join(format!("{desk}-portals.conf")));
        }
        v.push(d.join("portals.conf"));
    }
    v
}

/// The file in force, and its text.
fn effective() -> Option<(PathBuf, String)> {
    candidates().into_iter().find_map(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
}

/// What the `[preferred]` section chooses for the file chooser (`a;b;`).
fn chooser_of(text: &str) -> Option<String> {
    let mut in_pref = false;
    let (mut specific, mut default) = (None, None);
    for l in text.lines().map(str::trim) {
        if l.starts_with('[') {
            in_pref = l == "[preferred]";
        } else if in_pref {
            if let Some((k, v)) = l.split_once('=') {
                match k.trim() {
                    KEY => specific = Some(v.trim().to_owned()),
                    "default" => default = Some(v.trim().to_owned()),
                    _ => {}
                }
            }
        }
    }
    specific.or(default)
}

/// The text with the file chooser set to `value` under `[preferred]`.
fn with_chooser(text: &str, value: &str) -> String {
    let line = format!("{KEY}={value}");
    let mut out: Vec<String> = Vec::new();
    let (mut in_pref, mut done, mut seen_section) = (false, false, false);
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            if in_pref && !done {
                out.push(line.clone());
                done = true;
            }
            in_pref = t == "[preferred]";
            seen_section |= in_pref;
            out.push(l.to_owned());
        } else if in_pref && t.split_once('=').is_some_and(|(k, _)| k.trim() == KEY) {
            if !done {
                out.push(line.clone());
                done = true;
            }
        } else {
            out.push(l.to_owned());
        }
    }
    if !done {
        if !seen_section {
            out.push("[preferred]".into());
        }
        out.push(line);
    }
    out.join("\n") + "\n"
}

fn bus_has(c: &Connection, name: &str) -> bool {
    let p = c.with_proxy("org.freedesktop.DBus", "/org/freedesktop/DBus", Duration::from_secs(3));
    p.method_call::<(bool,), _, _, _>("org.freedesktop.DBus", "NameHasOwner", (name,)).map(|r| r.0).unwrap_or(false)
}

fn activatable(c: &Connection, name: &str) -> bool {
    let p = c.with_proxy("org.freedesktop.DBus", "/org/freedesktop/DBus", Duration::from_secs(3));
    p.method_call::<(Vec<String>,), _, _, _>("org.freedesktop.DBus", "ListActivatableNames", ()).map(|r| r.0.iter().any(|n| n == name)).unwrap_or(false)
}

/// Prints the state; true when the portal will use HeroPortal.
pub fn status() -> bool {
    let mut ok = true;
    match Connection::new_session() {
        Err(e) => {
            println!("No session bus ({e}): not in a desktop session?");
            ok = false;
        }
        Ok(c) => {
            println!("xdg-desktop-portal: {}", if bus_has(&c, "org.freedesktop.portal.Desktop") { "running" } else { "not running (starts with the first request)" });
            let can = activatable(&c, crate::service::NAME);
            println!("HeroPortal service: {}", if bus_has(&c, crate::service::NAME) { "running" } else if can { "installed (D-Bus starts it when asked)" } else { "NOT installed (is the heroportal package installed?)" });
            ok &= can || bus_has(&c, crate::service::NAME);
        }
    }
    match effective() {
        None => {
            println!("Portal config: none found, so the portal picks backends itself (GTK's file dialog).");
            ok = false;
        }
        Some((path, text)) => {
            let choice = chooser_of(&text);
            println!("Portal config: {}", path.display());
            println!("File dialogs: {}", choice.as_deref().unwrap_or("not set (the portal picks)"));
            ok &= choice.as_deref().is_some_and(|c| c.split(';').map(str::trim).find(|b| !b.is_empty()) == Some(BACKEND));
        }
    }
    if let Some(d) = std::env::var_os("WAYLAND_DISPLAY").or_else(|| std::env::var_os("DISPLAY")) {
        println!("Display: {}", d.to_string_lossy());
    } else {
        println!("Display: not set here (the portal finds one itself)");
    }
    println!("{}", if ok { "OK: apps that ask the portal get HeroPortal's dialogs." } else { "Not set up: run `heroportal use`." });
    ok
}

/// Makes the portal use HeroPortal for file dialogs.
pub fn use_it() -> Result<(), String> {
    let desk = desktops();
    let dir = user_dir();
    // The user's own desktop-specific file, if there is one, else portals.conf.
    let target = desk.iter().map(|d| dir.join(format!("{d}-portals.conf"))).find(|p| p.exists()).unwrap_or_else(|| dir.join("portals.conf"));
    let base = std::fs::read_to_string(&target).ok().or_else(|| effective().map(|(_, t)| t)).unwrap_or_else(|| "[preferred]\ndefault=gtk;\n".into());
    let text = with_chooser(&base, BACKEND);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = target.with_extension("conf.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    println!("Wrote {}", target.display());
    // Services D-Bus starts later get the display too.
    let vars: Vec<&str> = ["WAYLAND_DISPLAY", "DISPLAY", "XDG_CURRENT_DESKTOP", "XDG_RUNTIME_DIR"].into_iter().filter(|v| std::env::var_os(v).is_some()).collect();
    if !vars.is_empty() {
        let _ = Command::new("dbus-update-activation-environment").arg("--systemd").args(&vars).status();
    }
    match Command::new("systemctl").args(["--user", "restart", "xdg-desktop-portal"]).status() {
        Ok(s) if s.success() => println!("Restarted xdg-desktop-portal."),
        _ => println!("Could not restart xdg-desktop-portal: log out and in, or run `systemctl --user restart xdg-desktop-portal`."),
    }
    std::thread::sleep(Duration::from_millis(500));
    println!();
    status();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_config() {
        let t = "[preferred]\ndefault=gtk;\norg.freedesktop.impl.portal.FileChooser=gtk;\norg.freedesktop.impl.portal.Access=gtk;\n";
        let n = with_chooser(t, "hero");
        assert_eq!(chooser_of(&n).as_deref(), Some("hero"));
        assert!(n.contains("Access=gtk;") && n.matches("FileChooser").count() == 1);
        assert_eq!(with_chooser("", "hero"), "[preferred]\norg.freedesktop.impl.portal.FileChooser=hero\n");
        let m = with_chooser("[preferred]\ndefault=gtk;\n[other]\nx=1\n", "hero");
        assert!(m.find("FileChooser").unwrap() < m.find("[other]").unwrap());
        assert_eq!(chooser_of("[preferred]\ndefault=wlr;gtk;\n").as_deref(), Some("wlr;gtk;"));
    }
}

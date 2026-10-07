//! The portal backend on the session bus. xdg-desktop-portal (the portal
//! apps talk to) forwards FileChooser calls here; each opens a dialog in a
//! process of its own (`heroportal --dialog`), and the call is answered
//! when that ends. Started by D-Bus when needed; leaves a minute after the
//! last dialog closes, so nothing runs between dialogs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use dbus::arg::{PropMap, RefArg, Variant};
use dbus::blocking::LocalConnection;
use dbus::channel::MatchingReceiver;
use dbus::message::MatchRule;
use dbus::Message;

use crate::request::{uri, Answer, Filter, Mode, Request};

pub const NAME: &str = "org.freedesktop.impl.portal.desktop.hero";
const PATH: &str = "/org/freedesktop/portal/desktop";
const FILE_CHOOSER: &str = "org.freedesktop.impl.portal.FileChooser";

/// How long to stay after the last dialog.
const LINGER: Duration = Duration::from_secs(60);

const INTROSPECTION: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
  <interface name="org.freedesktop.impl.portal.FileChooser">
    <method name="OpenFile"><arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/><arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/><arg type="a{sv}" name="options" direction="in"/><arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/></method>
    <method name="SaveFile"><arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/><arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/><arg type="a{sv}" name="options" direction="in"/><arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/></method>
    <method name="SaveFiles"><arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/><arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/><arg type="a{sv}" name="options" direction="in"/><arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/></method>
  </interface>
  <interface name="org.freedesktop.DBus.Introspectable"><method name="Introspect"><arg type="s" name="xml" direction="out"/></method></interface>
</node>"#;

/// A dialog open, and the call it answers.
struct Pending {
    call: Message,
    /// The request object the portal may ask to close.
    handle: String,
    child: Child,
    filters: Vec<Filter>,
    /// SaveFiles: the names, to put in the chosen folder.
    files: Option<Vec<String>>,
}

pub fn run() -> Result<(), String> {
    let c = LocalConnection::new_session().map_err(|e| format!("no session bus ({e})"))?;
    c.request_name(NAME, false, true, true).map_err(|e| e.to_string())?;
    let pending: Rc<RefCell<Vec<Pending>>> = Rc::default();
    {
        let pending = pending.clone();
        c.start_receive(
            MatchRule::new_method_call(),
            Box::new(move |msg, conn| {
                handle(msg, conn, &pending);
                true
            }),
        );
    }
    let mut idle_since = Instant::now();
    loop {
        let busy = !pending.borrow().is_empty();
        if c.process(if busy { Duration::from_millis(100) } else { Duration::from_secs(5) }).is_err() {
            // The session ended.
            return Ok(());
        }
        // Dialogs that ended.
        let done: Vec<Pending> = {
            let mut p = pending.borrow_mut();
            let mut done = vec![];
            let mut i = 0;
            while i < p.len() {
                if matches!(p[i].child.try_wait(), Ok(Some(_)) | Err(_)) {
                    done.push(p.remove(i));
                } else {
                    i += 1;
                }
            }
            done
        };
        for mut d in done {
            let mut out = String::new();
            if let Some(mut s) = d.child.stdout.take() {
                let _ = s.read_to_string(&mut out);
            }
            let answer: Option<Answer> = serde_json::from_str(&out).ok();
            let _ = c.channel().send(reply(&d, answer));
        }
        if !pending.borrow().is_empty() {
            idle_since = Instant::now();
        } else if idle_since.elapsed() > LINGER {
            return Ok(());
        }
    }
}

fn handle(msg: Message, conn: &LocalConnection, pending: &Rc<RefCell<Vec<Pending>>>) {
    let (Some(iface), Some(member)) = (msg.interface(), msg.member()) else { return };
    let (iface, member) = (iface.to_string(), member.to_string());
    let send = |m: Message| {
        let _ = conn.channel().send(m);
    };
    match (iface.as_str(), member.as_str()) {
        ("org.freedesktop.DBus.Introspectable", "Introspect") => send(msg.method_return().append1(INTROSPECTION)),
        // The app gave up: its dialog goes, and the call is answered as cancelled.
        ("org.freedesktop.impl.portal.Request", "Close") => {
            let path = msg.path().map(|p| p.to_string()).unwrap_or_default();
            for p in pending.borrow_mut().iter_mut().filter(|p| p.handle == path) {
                let _ = p.child.kill();
            }
            send(msg.method_return());
        }
        (FILE_CHOOSER, "OpenFile" | "SaveFile" | "SaveFiles") => {
            if msg.path().as_deref() != Some(PATH) {
                send(msg.error(&"org.freedesktop.DBus.Error.UnknownObject".into(), &std::ffi::CString::new("no such object").unwrap()));
                return;
            }
            let (handle, parent, title, options) = match read_call(&msg) {
                Ok(args) => args,
                Err(e) => {
                    send(msg.error(&"org.freedesktop.DBus.Error.InvalidArgs".into(), &std::ffi::CString::new(e.to_string()).unwrap_or_default()));
                    return;
                }
            };
            let mode = match member.as_str() {
                "OpenFile" => Mode::Open,
                "SaveFile" => Mode::Save,
                _ => Mode::SaveFiles,
            };
            let req = request(mode, title, parent, &options);
            match start(&req) {
                Ok(child) => {
                    let files = (mode == Mode::SaveFiles).then(|| req.files.clone());
                    pending.borrow_mut().push(Pending { call: msg, handle, child, filters: req.filters, files });
                }
                Err(e) => {
                    eprintln!("heroportal: can't open the dialog: {e}");
                    send(msg.method_return().append2(2u32, PropMap::new()));
                }
            }
        }
        _ => send(msg.error(&"org.freedesktop.DBus.Error.UnknownMethod".into(), &std::ffi::CString::new("unknown method").unwrap())),
    }
}

/// The dialog process, given the request.
fn start(req: &Request) -> std::io::Result<Child> {
    let exe = std::env::current_exe()?;
    let mut child = Command::new(exe).arg("--dialog").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    let json = serde_json::to_string(req).map_err(std::io::Error::other)?;
    if std::env::var_os("HEROPORTAL_DEBUG").is_some() {
        eprintln!("heroportal: {json}");
    }
    child.stdin.take().unwrap().write_all(json.as_bytes())?;
    Ok(child)
}

/// The answer to a call: 0 and the chosen URIs, or 1 (cancelled).
fn reply(d: &Pending, answer: Option<Answer>) -> Message {
    let Some(a) = answer.filter(|a| !a.paths.is_empty()) else {
        return d.call.method_return().append2(1u32, PropMap::new());
    };
    let paths: Vec<String> = match &d.files {
        Some(names) => names.iter().map(|n| format!("{}/{}", a.paths[0].trim_end_matches('/'), n)).collect(),
        None => a.paths.clone(),
    };
    let mut results: PropMap = HashMap::new();
    results.insert("uris".into(), Variant(Box::new(paths.iter().map(|p| uri(p)).collect::<Vec<_>>())));
    if let Some(f) = a.filter.and_then(|i| d.filters.get(i)) {
        results.insert("current_filter".into(), Variant(Box::new(filter_value(f))));
    }
    d.call.method_return().append2(0u32, results)
}

/// A filter as portals write it: (name, [(0 glob | 1 mime, pattern)]).
fn filter_value(f: &Filter) -> (String, Vec<(u32, String)>) {
    let pats = f.globs.iter().map(|g| (0u32, g.clone())).chain(f.mimes.iter().map(|m| (1u32, m.clone())));
    (f.name.clone(), pats.collect())
}

/// A filter from its D-Bus value `(sa(us))`.
fn filter_of(v: &dyn RefArg) -> Option<Filter> {
    let mut it = v.as_iter()?;
    let name = it.next()?.as_str()?.to_string();
    let mut f = Filter { name, ..Filter::default() };
    for pat in it.next()?.as_iter()? {
        let mut p = pat.as_iter()?;
        let kind = p.next()?.as_u64()?;
        let s = p.next()?.as_str()?.to_string();
        if kind == 0 { f.globs.push(s) } else { f.mimes.push(s) }
    }
    Some(f)
}

/// A NUL-terminated byte string (`ay`) as a path.
fn bytes_of(v: &(dyn RefArg + 'static)) -> Option<String> {
    // Byte arrays arrive as Vec<u8> (item by item, they don't read right).
    let bytes: Vec<u8> = match v.as_any().downcast_ref::<Vec<u8>>() {
        Some(b) => b.iter().copied().take_while(|&b| b != 0).collect(),
        None => return bytes_by_item(v),
    };
    (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

fn bytes_by_item(v: &dyn RefArg) -> Option<String> {
    let bytes: Vec<u8> = v.as_iter()?.filter_map(|b| b.as_u64().map(|b| b as u8)).take_while(|&b| b != 0).collect();
    (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

/// The file names in a list of paths (`aay`).
fn names_of(v: &(dyn RefArg + 'static)) -> Vec<String> {
    let paths: Vec<String> = match v.as_any().downcast_ref::<Vec<Vec<u8>>>() {
        Some(list) => list.iter().map(|b| String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).into_owned()).collect(),
        None => v.as_iter().map(|it| it.filter_map(bytes_by_item).collect()).unwrap_or_default(),
    };
    paths.iter().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()).filter(|n| !n.is_empty()).collect()
}

/// A FileChooser call's arguments: (handle, parent window, title,
/// options). Byte-string options (paths) are read as such and put in the
/// map as `Vec<u8>` / `Vec<Vec<u8>>`: read generically, the dbus crate
/// hands back their bytes from freed memory.
fn read_call(msg: &Message) -> Result<(String, String, String, PropMap), String> {
    let mut it = msg.iter_init();
    let handle: dbus::Path = it.read().map_err(|e| e.to_string())?;
    let _app_id: String = it.read().map_err(|e| e.to_string())?;
    let parent: String = it.read().map_err(|e| e.to_string())?;
    let title: String = it.read().map_err(|e| e.to_string())?;
    let mut options: PropMap = HashMap::new();
    let mut dict = it.recurse(dbus::arg::ArgType::Array).ok_or("no options")?;
    loop {
        if let Some(mut entry) = dict.recurse(dbus::arg::ArgType::DictEntry) {
            let key: String = entry.read().map_err(|e| e.to_string())?;
            if let Some(mut v) = entry.recurse(dbus::arg::ArgType::Variant) {
                let sig = v.signature().to_string();
                let value: Option<Box<dyn RefArg>> = match sig.as_str() {
                    "ay" => v.get::<Vec<u8>>().map(|b| Box::new(b) as Box<dyn RefArg>),
                    "aay" => v.get::<Vec<Vec<u8>>>().map(|b| Box::new(b) as Box<dyn RefArg>),
                    _ => v.get_refarg(),
                };
                if let Some(value) = value {
                    options.insert(key, Variant(value));
                }
            }
        }
        if !dict.next() {
            break;
        }
    }
    Ok((handle.to_string(), parent, title, options))
}

/// The request in the call's options.
pub fn request(mode: Mode, title: String, parent: String, o: &PropMap) -> Request {
    if std::env::var_os("HEROPORTAL_DEBUG").is_some() {
        for (k, v) in o {
            eprintln!("heroportal: option {k} = {:?} ({})", v.0, v.0.signature());
        }
    }
    let get = |k: &str| o.get(k).map(|v| &*v.0);
    let flag = |k: &str| get(k).and_then(|v| v.as_u64()).is_some_and(|v| v != 0);
    let mut filters: Vec<Filter> = get("filters").and_then(|v| v.as_iter()).map(|it| it.filter_map(filter_of).collect()).unwrap_or_default();
    let current = get("current_filter").and_then(filter_of);
    let current_filter = current.map(|c| match filters.iter().position(|f| f.name == c.name) {
        Some(i) => i,
        None => {
            filters.push(c);
            filters.len() - 1
        }
    });
    Request {
        mode,
        title,
        // "_Save" → "Save" (GTK mnemonics).
        accept: get("accept_label").and_then(|v| v.as_str()).map(|s| s.replace('_', "")),
        multiple: flag("multiple"),
        directory: flag("directory"),
        filters,
        current_filter,
        name: get("current_name").and_then(|v| v.as_str()).map(str::to_owned),
        folder: get("current_folder").and_then(bytes_of),
        file: get("current_file").and_then(bytes_of),
        files: get("files").map(names_of).unwrap_or_default(),
        parent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_read() {
        let mut o: PropMap = HashMap::new();
        o.insert("multiple".into(), Variant(Box::new(true)));
        o.insert("accept_label".into(), Variant(Box::new("_Upload".to_string())));
        o.insert("current_folder".into(), Variant(Box::new(b"/tmp\0".to_vec())));
        let f = |n: &str, p: Vec<(u32, String)>| (n.to_string(), p);
        o.insert("filters".into(), Variant(Box::new(vec![f("Images", vec![(0, "*.png".into()), (1, "image/jpeg".into())]), f("All", vec![(0, "*".into())])])));
        o.insert("current_filter".into(), Variant(Box::new(f("All", vec![(0, "*".into())]))));
        let r = request(Mode::Open, "Pick".into(), "wayland:abc".into(), &o);
        assert!(r.multiple && !r.directory);
        assert_eq!(r.accept.as_deref(), Some("Upload"));
        assert_eq!(r.folder.as_deref(), Some("/tmp"));
        assert_eq!(r.filters.len(), 2);
        assert_eq!(r.filters[0], Filter { name: "Images".into(), globs: vec!["*.png".into()], mimes: vec!["image/jpeg".into()] });
        assert_eq!(r.current_filter, Some(1));
    }

    /// Options as they come in a message (not built by hand).
    #[test]
    fn options_from_a_message() {
        let mut o: PropMap = HashMap::new();
        o.insert("current_folder".into(), Variant(Box::new(b"/tmp\0".to_vec())));
        o.insert("directory".into(), Variant(Box::new(true)));
        o.insert("files".into(), Variant(Box::new(vec![b"/x/a.txt\0".to_vec(), b"b.png\0".to_vec()])));
        let m = Message::new_method_call("a.b", "/a", "a.b", "c").unwrap().append3(dbus::Path::from("/r/1"), "app", "wayland:x").append2("Title", o);
        let (handle, parent, title, back) = read_call(&m).unwrap();
        assert_eq!((handle.as_str(), parent.as_str(), title.as_str()), ("/r/1", "wayland:x", "Title"));
        let r = request(Mode::Open, String::new(), String::new(), &back);
        assert_eq!(r.folder.as_deref(), Some("/tmp"));
        assert!(r.directory);
        assert_eq!(r.files, ["a.txt", "b.png"]);
    }
}

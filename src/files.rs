//! Folders and files: listing, the usual places, file type filters, and
//! how sizes and dates read.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::request::Filter;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl Entry {
    pub fn hidden(&self) -> bool {
        self.name.starts_with('.')
    }
}

/// A folder's entries: folders first, then files, each by name (case
/// ignored, numbers in order: "2" before "10").
pub fn list(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut out: Vec<Entry> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Following links: a link to a folder is a folder.
            let meta = std::fs::metadata(e.path()).or_else(|_| e.metadata()).ok();
            Entry {
                name,
                dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                size: meta.as_ref().map_or(0, |m| m.len()),
                modified: meta.and_then(|m| m.modified().ok()),
            }
        })
        .collect();
    out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| natural(&a.name, &b.name)));
    Ok(out)
}

/// Compares names as people read them.
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (num(&mut a), num(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                a.next();
                b.next();
            }
        }
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// The usual places that exist: home, the XDG user folders, the whole
/// computer. (label, path, icon)
pub fn places() -> Vec<(String, PathBuf, &'static str)> {
    let home = home();
    let dirs = std::fs::read_to_string(home.join(".config/user-dirs.dirs")).unwrap_or_default();
    // XDG_DOWNLOAD_DIR="$HOME/Downloads"
    let user_dir = |key: &str, default: &str| {
        dirs.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix("=")?.trim().strip_prefix('"')?.strip_suffix('"').map(str::to_owned))
            .map(|v| PathBuf::from(v.replacen("$HOME", &home.to_string_lossy(), 1)))
            .unwrap_or_else(|| home.join(default))
    };
    let mut out = vec![("Home".to_string(), home.clone(), "user-home")];
    for (key, default, label, icon) in [
        ("XDG_DESKTOP_DIR", "Desktop", "Desktop", "user-desktop"),
        ("XDG_DOCUMENTS_DIR", "Documents", "Documents", "folder-documents"),
        ("XDG_DOWNLOAD_DIR", "Downloads", "Downloads", "folder-download"),
        ("XDG_MUSIC_DIR", "Music", "Music", "folder-music"),
        ("XDG_PICTURES_DIR", "Pictures", "Pictures", "folder-pictures"),
        ("XDG_VIDEOS_DIR", "Videos", "Videos", "folder-videos"),
    ] {
        let p = user_dir(key, default);
        if p != home && p.is_dir() && !out.iter().any(|(_, q, _)| *q == p) {
            out.push((label.to_string(), p, icon));
        }
    }
    out.push(("Computer".to_string(), PathBuf::from("/"), "drive-harddisk"));
    out
}

/// A shell-style pattern (`*`, `?`), case ignored.
pub fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|i| go(&p[1..], &n[i..])),
            Some('?') => !n.is_empty() && go(&p[1..], &n[1..]),
            Some(c) => n.first().is_some_and(|d| c.to_lowercase().eq(d.to_lowercase())) && go(&p[1..], &n[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

/// The file name patterns of a MIME type ("image/png", "image/*"), from
/// the shared MIME database.
pub fn mime_globs(mime: &str) -> Vec<String> {
    thread_local!(static DB: Vec<(String, String)> = {
        let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        let mut v = vec![];
        for d in std::iter::once(home().join(".local/share")).chain(dirs.split(':').map(PathBuf::from)) {
            // weight:type/subtype:*.ext[:flags]
            for line in std::fs::read_to_string(d.join("mime/globs2")).unwrap_or_default().lines() {
                let mut f = line.splitn(4, ':');
                if let (Some(_), Some(m), Some(g)) = (f.next(), f.next(), f.next()) {
                    if !line.starts_with('#') {
                        v.push((m.to_string(), g.to_string()));
                    }
                }
            }
        }
        v
    });
    let prefix = mime.strip_suffix("/*").map(|p| format!("{p}/"));
    DB.with(|db| {
        let mut out: Vec<String> = db
            .iter()
            .filter(|(m, _)| match &prefix {
                Some(p) => m.starts_with(p.as_str()),
                None => m == mime,
            })
            .map(|(_, g)| g.clone())
            .collect();
        out.dedup();
        out
    })
}

/// Whether `name` (a file) passes `filter`.
pub fn passes(filter: &Filter, name: &str) -> bool {
    filter.globs.iter().any(|g| glob(g, name)) || filter.mimes.iter().any(|m| mime_globs(m).iter().any(|g| glob(g, name)))
}

/// "12 KB", "3.4 MB".
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} {}", if bytes == 1 { "byte" } else { "bytes" });
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if v < 10.0 { format!("{v:.1} {}", UNITS[u]) } else { format!("{v:.0} {}", UNITS[u]) }
}

/// The local time zone's offset (seconds east), asked once.
fn utc_offset() -> i64 {
    thread_local!(static OFF: i64 = std::process::Command::new("date").arg("+%z").output().ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let sign = if s.starts_with('-') { -1 } else { 1 };
            let d = s.trim_start_matches(['+', '-']);
            let (h, m) = (d.get(0..2)?.parse::<i64>().ok()?, d.get(2..4)?.parse::<i64>().ok()?);
            Some(sign * (h * 3600 + m * 60))
        })
        .unwrap_or(0));
    OFF.with(|o| *o)
}

/// Year, month, day of a day count since 1970 (proleptic Gregorian).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// "Today 14:05", "Yesterday 09:12", "Mar 3 18:40" (this year), "Mar 3, 2024".
pub fn date(t: SystemTime, now: SystemTime) -> String {
    let secs = |t: SystemTime| match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    } + utc_offset();
    let (s, n) = (secs(t), secs(now));
    let (day, nday) = (s.div_euclid(86_400), n.div_euclid(86_400));
    let (h, m) = (s.rem_euclid(86_400) / 3600, s.rem_euclid(3600) / 60);
    let (y, mo, d) = civil(day);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mon = MONTHS[(mo - 1) as usize];
    if day == nday {
        format!("Today {h:02}:{m:02}")
    } else if day + 1 == nday {
        format!("Yesterday {h:02}:{m:02}")
    } else if y == civil(nday).0 {
        format!("{mon} {d} {h:02}:{m:02}")
    } else {
        format!("{mon} {d}, {y}")
    }
}

/// An icon name for a file, by its extension.
pub fn icon_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "svg" | "bmp" | "tif" | "tiff" | "heic" | "qoi" | "jxl" => "image-x-generic",
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "ogv" => "video-x-generic",
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" => "audio-x-generic",
        "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" | "7z" | "rar" | "deb" | "rpm" => "package-x-generic",
        "pdf" => "application-pdf",
        "html" | "htm" => "text-html",
        "sh" | "py" | "rs" | "c" | "h" | "cpp" | "js" | "ts" | "toml" | "json" | "yaml" | "yml" => "text-x-script",
        _ => "text-x-generic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorting_and_globs() {
        let mut v = vec!["file10", "File2", "file1"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, ["file1", "File2", "file10"]);
        assert!(glob("*.PNG", "a.png"));
        assert!(glob("photo?.jpg", "photo1.jpg"));
        assert!(!glob("*.png", "a.jpg"));
    }

    #[test]
    fn readable() {
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(1500), "1.5 KB");
        assert_eq!(size(23_400_000), "23 MB");
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_000), (2024, 10, 4));
        let now = SystemTime::now();
        assert!(date(now, now).starts_with("Today "));
    }

    #[test]
    fn lists_folders_first() {
        let d = std::env::temp_dir().join(format!("heroportal-list-{}", std::process::id()));
        std::fs::create_dir_all(d.join("zdir")).unwrap();
        std::fs::write(d.join("a.txt"), "x").unwrap();
        let l = list(&d).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(l.iter().map(|e| (e.name.as_str(), e.dir)).collect::<Vec<_>>(), [("zdir", true), ("a.txt", false)]);
    }
}

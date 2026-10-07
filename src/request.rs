//! What an app asked for, as the D-Bus service hands it to the dialog
//! process (JSON on its stdin), and what was chosen (JSON on its stdout).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mode {
    /// Pick files to open (or a folder, with `directory`).
    #[default]
    Open,
    /// Name a file to save.
    Save,
    /// Pick a folder to save `files` in.
    SaveFiles,
}

/// A file type choice: shown as `name`; files match one of `globs` ("*.png")
/// or `mimes` ("image/png", "image/*").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub name: String,
    pub globs: Vec<String>,
    pub mimes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Request {
    pub mode: Mode,
    pub title: String,
    /// The accept button's label ("_Open" style mnemonics removed).
    pub accept: Option<String>,
    pub multiple: bool,
    /// Pick folders instead of files.
    pub directory: bool,
    pub filters: Vec<Filter>,
    pub current_filter: Option<usize>,
    /// Suggested file name (Save).
    pub name: Option<String>,
    /// Folder to start in.
    pub folder: Option<String>,
    /// The file being saved over (Save).
    pub file: Option<String>,
    /// Names of the files to save (SaveFiles).
    pub files: Vec<String>,
    /// The asking app's window, as the portal names it ("wayland:HANDLE").
    pub parent: String,
}

/// What was chosen.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub paths: Vec<String>,
    pub filter: Option<usize>,
}

/// A `file://` URI of an absolute path.
pub fn uri(path: &str) -> String {
    let mut s = String::from("file://");
    for &b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn uris() {
        assert_eq!(super::uri("/home/a b/ü.txt"), "file:///home/a%20b/%C3%BC.txt");
    }
}

//! HeroPortal: the desktop portal backend of HeroineOS. For now the file
//! chooser: the open and save dialogs apps ask the portal for.

mod dialog;
mod files;
mod request;
mod service;

use request::{Mode, Request};

fn usage() -> ! {
    println!(
        "heroportal {} - the HeroineOS desktop portal (open and save dialogs)

Usage:
  heroportal                 the portal backend (D-Bus starts it when needed)
  heroportal open [--multiple] [--folder] [--title T] [--in FOLDER]
  heroportal save [--name NAME] [--title T] [--in FOLDER]
                             show a dialog; print the chosen paths
  heroportal --dialog        a dialog for the request (JSON) on stdin
  heroportal --version
  heroportal --help",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {
            if let Err(e) = service::run() {
                eprintln!("heroportal: {e}");
                std::process::exit(1);
            }
        }
        Some("--dialog") => {
            let mut json = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut json);
            match serde_json::from_str::<Request>(&json) {
                Ok(req) => dialog::run(req, false),
                Err(e) => {
                    eprintln!("heroportal: bad request: {e}");
                    std::process::exit(2);
                }
            }
        }
        Some(cmd @ ("open" | "save")) => {
            let mut req = Request { mode: if cmd == "open" { Mode::Open } else { Mode::Save }, ..Request::default() };
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--multiple" => req.multiple = true,
                    "--folder" => req.directory = true,
                    "--title" => req.title = it.next().cloned().unwrap_or_default(),
                    "--name" => req.name = it.next().cloned(),
                    "--in" => req.folder = it.next().cloned(),
                    _ => usage(),
                }
            }
            dialog::run(req, true)
        }
        Some("-V" | "--version") => println!("heroportal {}", env!("CARGO_PKG_VERSION")),
        Some("-h" | "--help") => usage(),
        Some(other) => {
            eprintln!("heroportal: unknown argument {other:?} (see --help)");
            std::process::exit(2);
        }
    }
}

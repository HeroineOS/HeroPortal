//! The open/save dialog. Runs as a process of its own per request; prints
//! the answer and exits.
//!
//! Keys: the main button (Open/Save) has the focus, so Enter chooses; Tab,
//! Shift+Tab, Left and Right move between buttons and fields. Up/Down, Page
//! Up/Down, Home/End move in the files, and Enter then opens the folder or
//! chooses the file there. Backspace or Alt+Up goes to the parent folder;
//! Ctrl+H shows hidden files; Escape cancels. In a save dialog, typing
//! goes to the name. Click, Ctrl+click and Shift+click select
//! (several when the app allows), double-click opens.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use heroui::fltk::app;
use heroui::fltk::draw;
use heroui::fltk::enums::{Align, Event, EventState, FrameType, Key};
use heroui::fltk::frame::Frame;
use heroui::fltk::prelude::*;
use heroui::prelude::*;

use crate::files::{self, Entry};
use crate::request::{Answer, Mode, Request};

const ROW_H: i32 = 30;

#[derive(Debug, Clone, Copy)]
enum Move {
    By(i32),
    Home,
    End,
}

#[derive(Clone)]
enum Msg {
    Place(usize),
    Up,
    PathTyped(String),
    PathGo,
    Click { row: usize, ctrl: bool, shift: bool },
    Activate(usize),
    Move(Move),
    /// Enter outside the text fields.
    Enter,
    Name(String),
    Accept,
    Cancel,
    Filter(usize),
    Hidden(bool),
    Replace,
    KeepEditing,
}

struct Dialog {
    req: Request,
    /// Print paths one per line (command line use) instead of JSON.
    plain: bool,
    places: Vec<(String, PathBuf, &'static str)>,
    dir: PathBuf,
    /// The folder's entries, and those shown (hidden files, filter).
    all: Vec<Entry>,
    shown: Vec<Entry>,
    /// Selected rows of `shown`; the row keys move from; the one Shift
    /// extends from.
    sel: Vec<usize>,
    cursor: Option<usize>,
    anchor: Option<usize>,
    /// Bumped when the cursor should be scrolled into view.
    reveal: u64,
    filter: usize,
    filter_names: Vec<String>,
    hidden: bool,
    path_text: String,
    name: String,
    /// Chosen paths that exist, waiting for "Replace".
    confirm: Option<Vec<String>>,
    error: String,
}

impl Dialog {
    fn new(req: Request, plain: bool) -> Dialog {
        let start = req
            .folder
            .clone()
            .map(PathBuf::from)
            .or_else(|| req.file.as_deref().and_then(|f| Path::new(f).parent().map(Path::to_path_buf)))
            .filter(|d| d.is_dir())
            .unwrap_or_else(files::home);
        let name = req.name.clone().or_else(|| req.file.as_deref().and_then(|f| Path::new(f).file_name()).map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
        let filter_names = req.filters.iter().map(|f| f.name.clone()).collect();
        let mut d = Dialog {
            filter: req.current_filter.unwrap_or(0),
            req,
            plain,
            places: files::places(),
            dir: PathBuf::new(),
            all: vec![],
            shown: vec![],
            sel: vec![],
            cursor: None,
            anchor: None,
            reveal: 0,
            filter_names,
            hidden: false,
            path_text: String::new(),
            name,
            confirm: None,
            error: String::new(),
        };
        d.go(&start, None);
        d
    }

    /// Shows folder `dir`, with `select` (a name in it) selected.
    fn go(&mut self, dir: &Path, select: Option<&str>) {
        match files::list(dir) {
            Ok(all) => {
                self.dir = dir.to_path_buf();
                self.all = all;
                self.error.clear();
            }
            Err(e) => {
                self.error = format!("Can't open {}: {}", dir.display(), e.kind());
                return;
            }
        }
        self.path_text = self.dir.to_string_lossy().into_owned();
        self.refilter();
        self.sel.clear();
        self.anchor = None;
        self.cursor = select.and_then(|n| self.shown.iter().position(|e| e.name == n));
        if let Some(c) = self.cursor {
            self.sel = vec![c];
            self.anchor = Some(c);
        }
        self.reveal += 1;
    }

    fn refilter(&mut self) {
        let filter = self.req.filters.get(self.filter);
        let dirs_only = self.req.directory || self.req.mode == Mode::SaveFiles;
        self.shown = self
            .all
            .iter()
            .filter(|e| self.hidden || !e.hidden())
            .filter(|e| if dirs_only { e.dir } else { e.dir || filter.is_none_or(|f| files::passes(f, &e.name)) })
            .cloned()
            .collect();
    }

    fn path_of(&self, e: &Entry) -> PathBuf {
        self.dir.join(&e.name)
    }

    fn choose(&mut self, paths: Vec<PathBuf>) -> Task<Msg> {
        let paths: Vec<String> = paths.into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
        if self.plain {
            for p in &paths {
                println!("{p}");
            }
        } else {
            let a = Answer { paths, filter: (!self.req.filters.is_empty()).then_some(self.filter) };
            println!("{}", serde_json::to_string(&a).unwrap_or_default());
        }
        std::process::exit(0)
    }

    /// The chosen file for Save: `name` in the folder (or a path typed).
    fn save_target(&self) -> Option<PathBuf> {
        let n = self.name.trim();
        if n.is_empty() {
            return None;
        }
        Some(if let Some(rest) = n.strip_prefix("~/") { files::home().join(rest) } else if n.starts_with('/') { PathBuf::from(n) } else { self.dir.join(n) })
    }

    fn accept(&mut self) -> Task<Msg> {
        match self.req.mode {
            Mode::Open if self.req.directory => {
                let picked: Vec<PathBuf> = self.sel.iter().filter_map(|&i| self.shown.get(i)).map(|e| self.path_of(e)).collect();
                let picked = if picked.is_empty() { vec![self.dir.clone()] } else { picked };
                self.choose(picked)
            }
            Mode::Open => {
                let chosen: Vec<&Entry> = self.sel.iter().filter_map(|&i| self.shown.get(i)).collect();
                let files: Vec<PathBuf> = chosen.iter().filter(|e| !e.dir).map(|e| self.path_of(e)).collect();
                if !files.is_empty() {
                    return self.choose(files);
                }
                // Only a folder selected: into it.
                if let [e] = chosen.as_slice() {
                    let p = self.path_of(e);
                    self.go(&p, None);
                }
                Task::none()
            }
            Mode::Save => {
                let Some(target) = self.save_target() else {
                    self.error = "Name the file".into();
                    return Task::none();
                };
                if target.is_dir() {
                    self.name.clear();
                    self.go(&target, None);
                    return Task::none();
                }
                if !target.parent().is_some_and(Path::is_dir) {
                    self.error = "That folder doesn't exist".into();
                    return Task::none();
                }
                if target.exists() {
                    self.confirm = Some(vec![target.to_string_lossy().into_owned()]);
                    return Task::none();
                }
                self.choose(vec![target])
            }
            Mode::SaveFiles => {
                let dir = self.sel.first().and_then(|&i| self.shown.get(i)).map(|e| self.path_of(e)).unwrap_or_else(|| self.dir.clone());
                let existing: Vec<String> = self.req.files.iter().map(|n| dir.join(n)).filter(|p| p.exists()).map(|p| p.to_string_lossy().into_owned()).collect();
                if !existing.is_empty() {
                    self.confirm = Some(existing);
                    // Remembered for "Replace".
                    self.path_text = dir.to_string_lossy().into_owned();
                    return Task::none();
                }
                self.choose(vec![dir])
            }
        }
    }

    /// Opens a row: a folder goes in, a file is chosen.
    fn activate(&mut self, row: usize) -> Task<Msg> {
        let Some(e) = self.shown.get(row).cloned() else { return Task::none() };
        if e.dir {
            let p = self.path_of(&e);
            self.go(&p, None);
            return Task::none();
        }
        self.sel = vec![row];
        if self.req.mode == Mode::Save {
            self.name = e.name.clone();
        }
        self.accept()
    }

    /// The selection changed to `row`: in Save, a file's name is taken.
    fn picked(&mut self, row: usize) {
        if self.req.mode == Mode::Save {
            if let Some(e) = self.shown.get(row).filter(|e| !e.dir) {
                self.name = e.name.clone();
            }
        }
    }

    fn accept_label(&self) -> String {
        self.req.accept.clone().unwrap_or_else(|| {
            match self.req.mode {
                Mode::Open if self.req.directory => "Select",
                Mode::Open => "Open",
                Mode::Save | Mode::SaveFiles => "Save",
            }
            .into()
        })
    }
}

impl App for Dialog {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Place(i) => {
                if let Some((_, p, _)) = self.places.get(i).cloned() {
                    self.go(&p, None);
                }
            }
            Msg::Up => {
                if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
                    let from = self.dir.file_name().map(|n| n.to_string_lossy().into_owned());
                    self.go(&parent, from.as_deref());
                }
            }
            Msg::PathTyped(t) => self.path_text = t,
            Msg::PathGo => {
                let t = self.path_text.trim();
                let p = if let Some(rest) = t.strip_prefix("~/") { files::home().join(rest) } else if t == "~" { files::home() } else { PathBuf::from(t) };
                if p.is_dir() {
                    self.go(&p, None);
                } else if let (Some(parent), Some(name)) = (p.parent().filter(|d| d.is_dir()), p.file_name()) {
                    let name = name.to_string_lossy().into_owned();
                    self.go(parent, Some(&name));
                    if self.req.mode == Mode::Save {
                        self.name = name;
                    } else if p.is_file() {
                        return self.choose(vec![p]);
                    }
                } else {
                    self.error = "No such folder".into();
                }
            }
            Msg::Click { row, ctrl, shift } => {
                if row >= self.shown.len() {
                    return Task::none();
                }
                let many = self.req.multiple;
                if many && ctrl {
                    if let Some(i) = self.sel.iter().position(|&r| r == row) {
                        self.sel.remove(i);
                    } else {
                        self.sel.push(row);
                    }
                    self.anchor = Some(row);
                } else if many && shift && self.anchor.is_some() {
                    let a = self.anchor.unwrap();
                    self.sel = (a.min(row)..=a.max(row)).collect();
                } else {
                    self.sel = vec![row];
                    self.anchor = Some(row);
                }
                self.cursor = Some(row);
                self.picked(row);
            }
            Msg::Activate(row) => return self.activate(row),
            Msg::Move(m) => {
                let n = self.shown.len();
                if n == 0 {
                    return Task::none();
                }
                let c = match (m, self.cursor) {
                    (Move::Home, _) => 0,
                    (Move::End, _) => n - 1,
                    (Move::By(d), None) => if d < 0 { n - 1 } else { 0 },
                    (Move::By(d), Some(c)) => (c as i64 + d as i64).clamp(0, n as i64 - 1) as usize,
                };
                self.cursor = Some(c);
                self.sel = vec![c];
                self.anchor = Some(c);
                self.reveal += 1;
                self.picked(c);
            }
            Msg::Enter => {
                return match self.cursor.filter(|&c| self.sel.contains(&c)) {
                    Some(c) if self.shown.get(c).is_some_and(|e| e.dir) => self.activate(c),
                    _ => self.accept(),
                };
            }
            Msg::Name(n) => {
                self.name = n;
                self.error.clear();
            }
            Msg::Accept => return self.accept(),
            Msg::Cancel => std::process::exit(1),
            Msg::Filter(i) => {
                self.filter = i;
                self.refilter();
                self.sel.clear();
                self.cursor = None;
            }
            Msg::Hidden(on) => {
                self.hidden = on;
                let keep = self.cursor.and_then(|c| self.shown.get(c)).map(|e| e.name.clone());
                self.refilter();
                self.sel.clear();
                self.cursor = keep.and_then(|n| self.shown.iter().position(|e| e.name == n));
                if let Some(c) = self.cursor {
                    self.sel = vec![c];
                }
            }
            Msg::Replace => {
                if let Some(paths) = self.confirm.take() {
                    return if self.req.mode == Mode::SaveFiles { self.choose(vec![PathBuf::from(&self.path_text)]) } else { self.choose(paths.into_iter().map(PathBuf::from).collect()) };
                }
            }
            Msg::KeepEditing => {
                self.confirm = None;
                if self.req.mode == Mode::SaveFiles {
                    self.path_text = self.dir.to_string_lossy().into_owned();
                }
            }
        }
        Task::none()
    }

    fn close_requested(&self) -> Option<Msg> {
        Some(Msg::Cancel)
    }

    fn view(&self) -> Element<Self, Msg> {
        let save = self.req.mode == Mode::Save;
        let has_filters = self.filter_names.len() > 1;
        let places: Vec<Element<Dialog, Msg>> = (0..self.places.len()).map(|i| place_button(i).fixed(34)).chain([spacer()]).collect();
        column(vec![
            row(vec![
                button("Up", Msg::Up).fixed(64),
                text_input_submit(|d: &Dialog| d.path_text.clone(), Msg::PathTyped, Msg::PathGo),
            ])
            .fixed(34),
            row(vec![column(places).spacing(2).fixed(170), scroll(vec![file_list()])]),
            row(vec![label("Name").fixed(60), remember_name_field(text_input_submit(|d: &Dialog| d.name.clone(), Msg::Name, Msg::Accept))])
                .fixed(34)
                .visible(move |_: &Dialog| save),
            row(vec![
                note(|d: &Dialog| match &d.confirm {
                    Some(p) if p.len() == 1 => format!("\u{201c}{}\u{201d} already exists. Replace it?", Path::new(&p[0]).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                    Some(p) => format!("{} of these files already exist there. Replace them?", p.len()),
                    None => String::new(),
                }),
                button("Keep editing", Msg::KeepEditing).fixed(130),
                primary_button("Replace", Msg::Replace).fixed(110),
            ])
            .fixed(34)
            .visible(|d: &Dialog| d.confirm.is_some()),
            row(vec![
                note(|d: &Dialog| if !d.error.is_empty() { d.error.clone() } else if d.req.multiple && d.sel.len() > 1 { format!("{} selected", d.sel.len()) } else { String::new() }),
                dropdown(|d: &Dialog| &d.filter_names[..], |d: &Dialog| d.filter, Msg::Filter).fixed(220).visible(move |_: &Dialog| has_filters),
                toggle("Hidden", |d: &Dialog| d.hidden, Msg::Hidden).fixed(120),
                button("Cancel", Msg::Cancel).fixed(100),
                primary_button(&self.accept_label(), Msg::Accept).fixed(110).autofocus(),
            ])
            .fixed(34)
            .visible(|d: &Dialog| d.confirm.is_none()),
        ])
        .padding(12)
        .spacing(10)
    }
}

/// A place in the side column: its icon and name, lit when it's the folder shown.
fn place_button(i: usize) -> Element<Dialog, Msg> {
    Element::new(move |ctx| {
        let shown: Rc<RefCell<(String, &'static str, bool)>> = Rc::default();
        let mut b = custom_button({
            let shown = shown.clone();
            move |b| {
                let t = heroui::theme::current();
                let (label, icon, on) = &*shown.borrow();
                let a = heroui::hover::hover_amount(b);
                let bg = if *on { Some(t.surface_alt) } else if a > 0.0 { Some(heroui::widgets::mix(t.background, t.surface_alt, a)) } else { None };
                if let Some(c) = bg {
                    draw::set_draw_color(c);
                    draw::draw_rounded_rectf(b.x(), b.y(), b.w(), b.h(), t.radius.min(8));
                }
                let color = if *on { t.accent } else { t.text_dim };
                if !heroui::icons::draw(icon, b.x() + 10, b.y() + (b.h() - 18) / 2, 18, color) {
                    heroui::icons::draw("folder", b.x() + 10, b.y() + (b.h() - 18) / 2, 18, color);
                }
                draw::set_font(t.font(), t.font_size);
                draw::set_draw_color(if *on { t.text } else { heroui::widgets::mix(t.text, t.text_dim, 0.3) });
                draw::draw_text2(label, b.x() + 38, b.y(), b.w() - 42, b.h(), Align::Left | Align::Inside | Align::Clip);
            }
        });
        let emit = ctx.emitter();
        b.set_callback(move |_| emit(Msg::Place(i)));
        let mut w = b.clone();
        ctx.bind(move |d: &Dialog| {
            let (label, path, icon) = &d.places[i];
            // Lit for the place the folder is in (the deepest that holds it).
            let best = d.places.iter().enumerate().filter(|(_, (_, p, _))| d.dir.starts_with(p)).max_by_key(|(_, (_, p, _))| p.components().count()).map(|(j, _)| j);
            let now = (label.clone(), *icon, best == Some(i) && d.dir.starts_with(path));
            if *shown.borrow() != now {
                *shown.borrow_mut() = now;
                heroui::widgets::repaint(&mut w);
            }
        });
        b.as_base_widget()
    })
}

/// What the file list draws.
#[derive(Default, PartialEq)]
struct ListView {
    rows: Vec<Entry>,
    sel: Vec<usize>,
    cursor: Option<usize>,
    dir: PathBuf,
    error: String,
}

/// The folder's files: one widget drawing just the rows in view.
fn file_list() -> Element<Dialog, Msg> {
    Element::new(|ctx| {
        let view: Rc<RefCell<ListView>> = Rc::default();
        let hover: Rc<Cell<Option<usize>>> = Rc::default();
        let mut f = Frame::default();
        f.set_frame(FrameType::NoBox);
        {
            let (view, hover) = (view.clone(), hover.clone());
            f.draw(move |f| {
                let t = heroui::theme::current();
                let v = view.borrow();
                let (x, w) = (f.x(), f.w());
                // Only the rows inside the scroll area.
                let (top, bottom) = match f.parent().and_then(|p| p.parent()) {
                    Some(sc) => (sc.y(), sc.y() + sc.h()),
                    None => (f.y(), f.y() + f.h()),
                };
                if v.rows.is_empty() {
                    draw::set_font(t.font(), t.font_size);
                    draw::set_draw_color(t.text_dim);
                    let what = if v.error.is_empty() { "Nothing here" } else { v.error.as_str() };
                    draw::draw_text2(what, x, top, w, (bottom - top).min(120), Align::Center);
                    return;
                }
                let now = std::time::SystemTime::now();
                let first = ((top - f.y()) / ROW_H).max(0) as usize;
                let last = (((bottom - f.y()) / ROW_H + 1).max(0) as usize).min(v.rows.len());
                let r = t.radius.min(6);
                for i in first..last {
                    let e = &v.rows[i];
                    let y = f.y() + i as i32 * ROW_H;
                    let selected = v.sel.contains(&i);
                    if selected {
                        draw::set_draw_color(t.accent);
                        draw::draw_rounded_rectf(x, y + 1, w, ROW_H - 2, r);
                    } else if hover.get() == Some(i) {
                        draw::set_draw_color(heroui::widgets::mix(t.background, t.surface_alt, 0.7));
                        draw::draw_rounded_rectf(x, y + 1, w, ROW_H - 2, r);
                    }
                    if v.cursor == Some(i) && !selected {
                        draw::set_draw_color(t.accent);
                        draw::draw_rounded_rect(x, y + 1, w, ROW_H - 2, r);
                    }
                    let fg = if selected { t.accent_text } else { t.text };
                    let dim = if selected { t.accent_text } else { t.text_dim };
                    let icon = if e.dir { "folder" } else { files::icon_for(&e.name) };
                    if !heroui::icons::draw(icon, x + 8, y + (ROW_H - 18) / 2, 18, if e.dir { if selected { fg } else { t.accent } } else { dim }) {
                        // No themed icon: a page.
                        draw::set_draw_color(dim);
                        draw::draw_rect(x + 11, y + (ROW_H - 16) / 2, 12, 16);
                    }
                    let (size_w, date_w) = (90, 140);
                    let name_w = (w - 36 - size_w - date_w - 16).max(40);
                    draw::set_font(t.font(), t.font_size);
                    draw::set_draw_color(if e.hidden() && !selected { dim } else { fg });
                    draw::draw_text2(&e.name, x + 34, y, name_w, ROW_H, Align::Left | Align::Inside | Align::Clip);
                    draw::set_font(t.font(), (t.font_size - 1).max(9));
                    draw::set_draw_color(dim);
                    if !e.dir {
                        draw::draw_text2(&files::size(e.size), x + 34 + name_w, y, size_w, ROW_H, Align::Right | Align::Inside);
                    }
                    if let Some(m) = e.modified {
                        draw::draw_text2(&files::date(m, now), x + w - date_w - 8, y, date_w, ROW_H, Align::Right | Align::Inside);
                    }
                }
            });
        }
        let emit = ctx.emitter();
        {
            let (view, hover, emit) = (view.clone(), hover.clone(), emit.clone());
            f.handle(move |f, ev| {
                let row = {
                    let y = app::event_y() - f.y();
                    let n = view.borrow().rows.len();
                    (y >= 0 && (y / ROW_H) < n as i32).then_some((y / ROW_H) as usize)
                };
                match ev {
                    Event::Enter | Event::Move => {
                        if hover.replace(row) != row {
                            heroui::widgets::repaint(f);
                        }
                        true
                    }
                    Event::Leave => {
                        if hover.replace(None).is_some() {
                            heroui::widgets::repaint(f);
                        }
                        true
                    }
                    Event::Push => {
                        if let Some(row) = row {
                            let s = app::event_state();
                            if app::event_clicks() {
                                emit(Msg::Activate(row));
                            } else {
                                emit(Msg::Click { row, ctrl: s.contains(EventState::Ctrl), shift: s.contains(EventState::Shift) });
                            }
                        }
                        true
                    }
                    _ => false,
                }
            });
        }
        // Keys anywhere in the dialog, before the text fields (which keep
        // what they need: letters, Left/Right, Enter).
        {
            let emit = emit.clone();
            heroui::on_key(move || {
                let k = app::event_key();
                let st = app::event_state();
                let typing = app::focus().is_some_and(|w| heroui::fltk::input::Input::from_dyn_widget(&w).is_some());
                let on_button = app::focus().is_some_and(|w| heroui::fltk::button::Button::from_dyn_widget(&w).is_some());
                match k {
                    // Moving in the files: Enter then means the file.
                    Key::Up | Key::Down | Key::PageUp | Key::PageDown => IN_LIST.with(|l| l.set(true)),
                    // Moving between buttons: Enter presses them.
                    Key::Tab | Key::Left | Key::Right => IN_LIST.with(|l| l.set(false)),
                    _ => {}
                }
                // Typing a name while on a button (save dialogs).
                let text = app::event_text();
                if on_button && !st.intersects(EventState::Ctrl | EventState::Alt | EventState::Meta) && text.chars().next().is_some_and(|c| !c.is_control() && c != ' ') {
                    if let Some(mut input) = NAME_FIELD.with(|n| n.borrow().clone()).filter(|i| i.visible_r()) {
                        let _ = input.take_focus();
                        if let Some(mut i) = heroui::fltk::input::Input::from_dyn_widget(&input) {
                            // What's typed replaces the name, like a selected field.
                            let len = i.value().len() as i32;
                            let _ = i.set_position(0);
                            let _ = i.set_mark(len);
                        }
                        // The key goes on to the field.
                        return false;
                    }
                }
                let msg = match k {
                    Key::Escape => Msg::Cancel,
                    Key::Up if st.contains(EventState::Alt) => Msg::Up,
                    Key::Up => Msg::Move(Move::By(-1)),
                    Key::Down => Msg::Move(Move::By(1)),
                    Key::PageUp => Msg::Move(Move::By(-10)),
                    Key::PageDown => Msg::Move(Move::By(10)),
                    Key::Home if !typing => Msg::Move(Move::Home),
                    Key::End if !typing => Msg::Move(Move::End),
                    Key::BackSpace if !typing => Msg::Up,
                    // On a button, Enter presses it (HeroUI), unless the
                    // arrows were last moving in the files.
                    Key::Enter | Key::KPEnter if !typing && (!on_button || IN_LIST.with(Cell::get)) => Msg::Enter,
                    k if st.contains(EventState::Ctrl) && k == Key::from_char('h') => Msg::Move(Move::By(0)),
                    _ => return false,
                };
                if let Msg::Move(Move::By(0)) = msg {
                    // Ctrl+H: hidden files.
                    emit(Msg::Hidden(!HIDDEN.with(Cell::get)));
                } else {
                    emit(msg);
                }
                true
            });
        }
        let mut w = f.clone();
        let last_reveal = Cell::new(u64::MAX);
        ctx.bind(move |d: &Dialog| {
            HIDDEN.with(|h| h.set(d.hidden));
            let now = ListView { rows: d.shown.clone(), sel: d.sel.clone(), cursor: d.cursor, dir: d.dir.clone(), error: d.error.clone() };
            let moved_dir = view.borrow().dir != now.dir;
            if *view.borrow() != now {
                *view.borrow_mut() = now;
                heroui::widgets::repaint(&mut w);
            }
            // Keep the cursor in view (a new folder starts at the top).
            if last_reveal.replace(d.reveal) != d.reveal {
                if let Some(mut sc) = w.parent().and_then(|p| p.parent()).and_then(|p| heroui::fltk::group::Scroll::from_dyn_widget(&p)) {
                    let pos = sc.yposition();
                    let target = match d.cursor {
                        Some(c) => {
                            let (top, bottom) = (c as i32 * ROW_H, (c as i32 + 1) * ROW_H);
                            if moved_dir && top < sc.h() { 0 } else if top < pos { top } else if bottom > pos + sc.h() { bottom - sc.h() } else { pos }
                        }
                        None => 0,
                    };
                    if target != pos {
                        sc.scroll_to(0, target);
                    }
                }
            }
        });
        f.as_base_widget()
    })
    .fixed_with(|d: &Dialog| d.shown.len() as i32 * ROW_H)
}

/// Dim text on the left (messages).
fn note(f: impl Fn(&Dialog) -> String + 'static) -> Element<Dialog, Msg> {
    canvas(f, |s: &String, x, y, w, h, t: &Theme| {
        draw::set_font(t.font(), t.font_size - 1);
        draw::set_draw_color(t.text_dim);
        draw::draw_text2(s, x, y, w, h, Align::Left | Align::Inside | Align::Clip);
    })
}

thread_local! {
    static HIDDEN: Cell<bool> = const { Cell::new(false) };
    /// The arrows last moved in the file list (not between buttons).
    static IN_LIST: Cell<bool> = const { Cell::new(false) };
    /// The save dialog's name field, for typing into from anywhere.
    static NAME_FIELD: RefCell<Option<heroui::fltk::widget::Widget>> = const { RefCell::new(None) };
}

fn remember_name_field(field: Element<Dialog, Msg>) -> Element<Dialog, Msg> {
    Element::new(move |ctx| {
        let w = field.build(ctx);
        NAME_FIELD.with(|n| *n.borrow_mut() = Some(w.clone()));
        w
    })
}

/// Shows the dialog for `req`; prints the answer and exits.
pub fn run(req: Request, plain: bool) -> ! {
    let title = if req.title.is_empty() {
        match req.mode {
            Mode::Open if req.directory => "Choose a folder",
            Mode::Open => "Open",
            Mode::Save => "Save",
            Mode::SaveFiles => "Save files to",
        }
        .to_string()
    } else {
        req.title.clone()
    };
    let parent = req.parent.clone();
    let settings = Settings::new(&title).size(820, 540).class("heroportal").kind(WindowKind::Dialog).parent_window(&parent);
    let _ = heroui::run(Dialog::new(req, plain), settings);
    std::process::exit(1)
}

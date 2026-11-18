//! The screen: a search box, then tabs for albums, tracks, recent searches, downloads and the
//! library (finished downloads into the library). slskd is asked in the background, so the
//! screen never waits on it.

use crate::api::{thousands, Slskd};
use crate::library::{self, Todo};
use crate::rank::{self, Entry, Sort};
use crate::Config;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

// the nekozame palette (github.com/purplekairi1312/nekozame-theme)
const BG: Color = Color::Rgb(0x5a, 0x4f, 0x6e);
const PANEL: Color = Color::Rgb(0x4f, 0x45, 0x62);
const DEEP: Color = Color::Rgb(0x43, 0x3a, 0x55);
const SELECTION: Color = Color::Rgb(0x8a, 0x76, 0xa8);
const PINK: Color = Color::Rgb(0xff, 0xc0, 0xe5);
const SOFT: Color = Color::Rgb(0xff, 0xd4, 0xee);
const MUTED: Color = Color::Rgb(0xe8, 0xde, 0xf5);
const DIM: Color = Color::Rgb(0xb3, 0xa7, 0xcb);
const BUTTER: Color = Color::Rgb(0xff, 0xf1, 0xb0);
const WHITE: Color = Color::Rgb(0xff, 0xff, 0xff);

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Albums,
    Tracks,
    Recent,
    Downloads,
    Library,
}

const TABS: [(Tab, &str); 5] = [
    (Tab::Albums, "albums"),
    (Tab::Tracks, "tracks"),
    (Tab::Recent, "recent"),
    (Tab::Downloads, "downloads"),
    (Tab::Library, "library"),
];

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Search,
    Table,
    Edit,
}

enum Msg {
    Status(String),
    Started(Result<String, String>, String),
    Results(String, bool, Vec<Value>),
    History(Vec<Value>),
    Transfers(Vec<Value>),
    Note(String, bool),
}

struct App {
    cfg: Config,
    api: Arc<Slskd>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    status: String,
    query: String,
    cursor: usize,
    focus: Focus,
    back: Option<usize>,
    tab: Tab,
    sort: Sort,
    sid: Option<String>,
    text: String,
    wanted: Option<String>, // what you typed, when Soulseek wouldn't search it as it is
    fallbacks: Vec<String>, // the other ways of asking still to try
    searching: bool,
    polling: bool,
    albums: Vec<Entry>,
    tracks: Vec<Entry>,
    inside: bool,
    past: Vec<Value>,
    transfers: Vec<Value>,
    todo: Vec<Todo>,
    notes: Vec<String>,
    skipped: HashSet<PathBuf>,
    edit: String,
    toast: Option<(String, bool, Instant)>,
    tables: [TableState; 5],
    last_poll: Instant,
    last_transfers: Instant,
    last_status: Instant,
    last_library: Instant,
    history_at: Option<Instant>,
    quit: bool,
}

pub fn run(cfg: Config, first: Option<String>) -> Result<(), String> {
    let (tx, rx) = channel();
    let api = Arc::new(Slskd::new(&cfg.url, &cfg.api_key));
    let long_ago = Instant::now() - Duration::from_secs(3600);
    let mut app = App {
        cfg,
        api,
        tx,
        rx,
        status: "…".into(),
        query: String::new(),
        cursor: 0,
        focus: Focus::Search,
        back: None,
        tab: Tab::Albums,
        sort: Sort::Quality,
        sid: None,
        text: String::new(),
        wanted: None,
        fallbacks: vec![],
        searching: false,
        polling: false,
        albums: vec![],
        tracks: vec![],
        inside: false,
        past: vec![],
        transfers: vec![],
        todo: vec![],
        notes: vec![],
        skipped: HashSet::new(),
        edit: String::new(),
        toast: None,
        tables: Default::default(),
        last_poll: long_ago,
        last_transfers: long_ago,
        last_status: long_ago,
        last_library: long_ago,
        history_at: Some(Instant::now()),
        quit: false,
    };
    if let Some(q) = first {
        app.query = q.clone();
        app.cursor = q.chars().count();
        app.start(q);
    }
    let mut terminal = ratatui::init();
    let result = (|| -> std::io::Result<()> {
        while !app.quit {
            app.tick();
            terminal.draw(|f| app.draw(f))?;
            if event::poll(Duration::from_millis(120))? {
                if let Event::Key(k) = event::read()? {
                    if k.kind == KeyEventKind::Press {
                        app.key(k);
                    }
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result.map_err(|e| e.to_string())
}

impl App {
    fn idx(tab: Tab) -> usize {
        TABS.iter().position(|(t, _)| *t == tab).unwrap_or(0)
    }

    fn background(&self, job: impl FnOnce(&Slskd) -> Msg + Send + 'static) {
        let (api, tx) = (self.api.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job(&api));
        });
    }

    fn say(&mut self, text: impl Into<String>, bad: bool) {
        self.toast = Some((text.into(), bad, Instant::now()));
    }

    // --- what comes back from slskd, and what's asked of it, now and then

    fn tick(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Status(s) => self.status = s,
                Msg::Started(Ok(id), text) => {
                    self.sid = Some(id);
                    self.text = text;
                    self.searching = true;
                    self.albums.clear();
                    self.tracks.clear();
                    self.inside = false;
                    if !matches!(self.tab, Tab::Albums | Tab::Tracks) {
                        self.tab = Tab::Albums;
                    }
                    self.focus = Focus::Table;
                    self.history_at = Some(Instant::now() + Duration::from_secs(2));
                }
                Msg::Started(Err(e), _) => self.say(format!("couldn't search: {e}"), true),
                Msg::Results(id, done, responses) => {
                    self.polling = false;
                    if self.sid.as_deref() == Some(id.as_str()) {
                        self.albums = rank::albums(&responses);
                        self.tracks = rank::tracks(&responses);
                        if let Some(w) = &self.wanted {
                            self.albums.retain(|e| matches(e, w));
                            self.tracks.retain(|e| matches(e, w));
                        }
                        rank::sort(&mut self.albums, self.sort);
                        rank::sort(&mut self.tracks, self.sort);
                        self.searching = !done;
                        // nothing at all: the Soulseek server won't pass on searches for some
                        // artists and titles (The Beatles, Selected Ambient Works…, at rights
                        // holders' request), though the files are shared. asked other ways
                        // ("*eatles"), then only what matches everything typed is shown
                        if done && responses.is_empty() {
                            if self.wanted.is_none() {
                                self.wanted = Some(self.text.clone());
                                self.fallbacks = other_ways(&self.text);
                            }
                            if !self.fallbacks.is_empty() {
                                let t = self.fallbacks.remove(0);
                                self.searching = true; // still going, as far as you're concerned
                                self.background(move |api| Msg::Started(api.search(&t), t.clone()));
                            }
                        }
                        for (tab, n) in [(Tab::Albums, self.albums.len()), (Tab::Tracks, self.tracks.len())] {
                            let t = &mut self.tables[App::idx(tab)];
                            if n == 0 {
                                t.select(None);
                            } else if t.selected().is_none_or(|i| i >= n) {
                                t.select(Some(0));
                            }
                        }
                    }
                }
                Msg::History(h) => self.past = h,
                Msg::Transfers(t) => self.transfers = t,
                Msg::Note(text, bad) => self.say(text, bad),
            }
        }
        let now = Instant::now();
        if self.searching && !self.polling && now - self.last_poll > Duration::from_secs(1) {
            if let Some(id) = self.sid.clone() {
                self.polling = true;
                self.last_poll = now;
                self.background(move |api| {
                    let done = api.search_state(&id).map(|s| s["isComplete"].as_bool().unwrap_or(false)).unwrap_or(false);
                    Msg::Results(id.clone(), done, api.responses(&id).unwrap_or_default())
                });
            }
        }
        if matches!(self.tab, Tab::Downloads | Tab::Library) && now - self.last_transfers > Duration::from_secs(3) {
            self.last_transfers = now;
            self.background(|api| Msg::Transfers(api.downloads().unwrap_or_default()));
        }
        if now - self.last_status > Duration::from_secs(30) {
            self.last_status = now;
            self.background(|api| Msg::Status(api.status()));
        }
        if self.history_at.is_some_and(|t| now >= t) {
            self.history_at = None;
            self.background(|api| Msg::History(api.searches().unwrap_or_default()));
        }
        if self.tab == Tab::Library && self.focus != Focus::Edit && now - self.last_library > Duration::from_secs(10) {
            self.load_library();
        }
        if self.toast.as_ref().is_some_and(|(_, _, t)| now - *t > Duration::from_secs(6)) {
            self.toast = None;
        }
    }

    fn start(&mut self, text: String) {
        self.back = None;
        self.wanted = None;
        let t = text.clone();
        self.background(move |api| Msg::Started(api.search(&t), t.clone()));
    }

    fn open_past(&mut self, i: usize) {
        let Some(x) = self.past.get(i) else { return };
        let (id, text) = (x["id"].as_str().unwrap_or("").to_string(), x["searchText"].as_str().unwrap_or("").to_string());
        self.query = text.clone();
        self.cursor = text.chars().count();
        self.sid = Some(id);
        self.text = text;
        self.wanted = None;
        self.searching = true; // one look: slskd still has its results
        self.albums.clear();
        self.tracks.clear();
        self.tab = Tab::Albums;
    }

    fn download(&mut self, e: Entry) {
        let files: Vec<(String, u64)> = e.files.iter().map(|f| (f.filename.clone(), f.size)).collect();
        let what = e.name.clone().unwrap_or_else(|| rank::title(&e));
        let line = format!("downloading {what}: {:.1} MB, {} {}, from {} (d shows it)", e.mb, rank::quality(&e), e.format, e.user);
        let user = e.user.clone();
        self.background(move |api| match api.download(&user, &files) {
            Ok(()) => Msg::Note(line.clone(), false),
            Err(err) => Msg::Note(format!("couldn't queue it: {err}"), true),
        });
    }

    // --- the library tab

    fn load_library(&mut self) {
        self.last_library = Instant::now();
        if !self.cfg.library.is_dir() || !self.cfg.downloads.is_dir() {
            self.todo.clear();
            return;
        }
        let kept: Vec<Todo> = std::mem::take(&mut self.todo);
        for src in library::finished(&self.cfg.downloads, &self.transfers) {
            if self.skipped.contains(&src) {
                continue;
            }
            match kept.iter().position(|t| t.src == src) {
                Some(i) => {
                    let t = &kept[i]; // what you changed stays
                    self.todo.push(Todo { src: t.src.clone(), kind: t.kind, artist: t.artist.clone(), dest: t.dest.clone() });
                }
                None => self.todo.push(library::guess(&self.cfg.library, &src)),
            }
        }
        self.notes = self
            .todo
            .iter()
            .map(|t| {
                if t.dest.symlink_metadata().is_ok() {
                    format!("{} (new: {})", library::quality_of(&t.dest), library::quality_of(&t.src))
                } else {
                    String::new()
                }
            })
            .collect();
        let t = &mut self.tables[App::idx(Tab::Library)];
        if self.todo.is_empty() {
            t.select(None);
        } else if t.selected().is_none_or(|i| i >= self.todo.len()) {
            t.select(Some(0));
        }
    }

    fn library_row(&self) -> Option<usize> {
        if self.todo.is_empty() {
            return None;
        }
        self.tables[App::idx(Tab::Library)].selected().map(|i| i.min(self.todo.len() - 1))
    }

    fn sort_one(&mut self, replace: bool, both: bool) {
        let Some(i) = self.library_row() else { return };
        let mut dest = self.todo[i].dest.clone();
        if both && dest.symlink_metadata().is_ok() {
            let (stem, ext) = match (dest.file_stem(), dest.extension()) {
                (Some(s), Some(e)) if dest.is_file() => (s.to_string_lossy().to_string(), format!(".{}", e.to_string_lossy())),
                _ => (dest.file_name().unwrap_or_default().to_string_lossy().to_string(), String::new()),
            };
            let mut n = 2;
            while dest.with_file_name(format!("{stem} ({n}){ext}")).symlink_metadata().is_ok() {
                n += 1;
            }
            dest = dest.with_file_name(format!("{stem} ({n}){ext}"));
        }
        if dest.symlink_metadata().is_ok() && !replace {
            self.say("you have it already: o replaces it (the old one is kept aside), n keeps both", true);
            return;
        }
        match library::move_to(&self.todo[i].src, &dest, replace) {
            Ok(()) => {
                let rel = dest.strip_prefix(&self.cfg.library).unwrap_or(&dest).display().to_string();
                self.say(format!("in your library: {rel}"), false);
            }
            Err(e) => self.say(format!("not moved: {e}"), true),
        }
        self.load_library();
    }

    // --- keys

    fn key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.focus {
            Focus::Search => self.key_search(k),
            Focus::Edit => self.key_edit(k),
            Focus::Table => self.key_table(k),
        }
    }

    fn key_search(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Enter => {
                let q = self.query.trim().to_string();
                if !q.is_empty() {
                    self.start(q);
                }
            }
            KeyCode::Esc => self.focus = Focus::Table,
            KeyCode::Up | KeyCode::Down => {
                // your recent searches, like a shell's history
                let mut texts: Vec<String> = vec![];
                for x in &self.past {
                    let t = x["searchText"].as_str().unwrap_or("").to_string();
                    if !texts.contains(&t) {
                        texts.push(t);
                    }
                }
                if texts.is_empty() {
                    return;
                }
                self.back = match (k.code, self.back) {
                    (KeyCode::Up, None) => Some(0),
                    (KeyCode::Up, Some(i)) => Some((i + 1).min(texts.len() - 1)),
                    (_, Some(0)) | (_, None) => None,
                    (_, Some(i)) => Some(i - 1),
                };
                self.query = self.back.map(|i| texts[i].clone()).unwrap_or_default();
                self.cursor = self.query.chars().count();
            }
            _ => edit_line(&mut self.query, &mut self.cursor, k),
        }
    }

    fn key_edit(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Enter => {
                let new = self.edit.trim().trim_matches('/').to_string();
                if let Some(i) = self.library_row() {
                    if !new.is_empty() && !new.split('/').any(|p| p == "..") {
                        self.todo[i].dest = self.cfg.library.join(new);
                    }
                }
                self.focus = Focus::Table;
                self.load_library();
            }
            KeyCode::Esc => self.focus = Focus::Table,
            _ => {
                let mut c = self.edit.chars().count();
                edit_line(&mut self.edit, &mut c, k);
            }
        }
    }

    fn rows(&self) -> usize {
        match self.tab {
            Tab::Albums => self.albums.len(),
            Tab::Tracks => self.tracks.len(),
            Tab::Recent => self.past.len(),
            Tab::Downloads => self.transfers.iter().map(|u| u["directories"].as_array().map_or(0, |d| d.len())).sum(),
            Tab::Library => self.todo.len(),
        }
    }

    fn key_table(&mut self, k: KeyEvent) {
        let n = self.rows();
        let t = &mut self.tables[App::idx(self.tab)];
        let at = t.selected().unwrap_or(0);
        match k.code {
            KeyCode::Down if n > 0 => t.select(Some((at + 1).min(n - 1))),
            KeyCode::Up if n > 0 => t.select(Some(at.saturating_sub(1))),
            KeyCode::PageDown if n > 0 => t.select(Some((at + 20).min(n - 1))),
            KeyCode::PageUp if n > 0 => t.select(Some(at.saturating_sub(20))),
            KeyCode::Home if n > 0 => t.select(Some(0)),
            KeyCode::End if n > 0 => t.select(Some(n - 1)),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('/') => self.focus = Focus::Search,
            KeyCode::Char('a') => self.tab = Tab::Albums,
            KeyCode::Char('t') => self.tab = Tab::Tracks,
            KeyCode::Char('r') => {
                self.tab = Tab::Recent;
                self.history_at = Some(Instant::now());
            }
            KeyCode::Char('d') => {
                self.tab = Tab::Downloads;
                self.last_transfers -= Duration::from_secs(10);
            }
            KeyCode::Char('l') => {
                self.tab = Tab::Library;
                self.last_transfers -= Duration::from_secs(10);
                self.load_library();
            }
            KeyCode::Char(c @ ('b' | 's' | 'f')) => {
                self.sort = match c { 'b' => Sort::Quality, 's' => Sort::Size, _ => Sort::Speed };
                rank::sort(&mut self.albums, self.sort);
                rank::sort(&mut self.tracks, self.sort);
                for tab in [Tab::Albums, Tab::Tracks] {
                    if self.rows_of(tab) > 0 {
                        self.tables[App::idx(tab)].select(Some(0));
                    }
                }
            }
            KeyCode::Char(' ') if self.tab == Tab::Albums => self.inside = !self.inside && !self.albums.is_empty(),
            KeyCode::Enter => match self.tab {
                Tab::Albums => {
                    if let Some(e) = self.albums.get(at).cloned() {
                        self.download(e);
                    }
                }
                Tab::Tracks => {
                    if let Some(e) = self.tracks.get(at).cloned() {
                        self.download(e);
                    }
                }
                Tab::Recent => self.open_past(at),
                Tab::Library => self.sort_one(false, false),
                Tab::Downloads => {}
            },
            KeyCode::Char(c) if self.tab == Tab::Library => match c {
                'e' => {
                    if let Some(i) = self.library_row() {
                        self.edit = self.todo[i].dest.strip_prefix(&self.cfg.library).unwrap_or(&self.todo[i].dest).display().to_string();
                        self.focus = Focus::Edit;
                    }
                }
                'k' => {
                    if let Some(i) = self.library_row() {
                        let t = &mut self.todo[i];
                        t.kind = t.kind.next();
                        let artist = if t.artist.is_empty() { "Unknown Artist".to_string() } else { t.artist.clone() };
                        t.dest = library::destination(&self.cfg.library, t.kind, &artist, &t.src);
                        self.load_library();
                    }
                }
                'x' => {
                    if let Some(i) = self.library_row() {
                        self.skipped.insert(self.todo[i].src.clone());
                        self.load_library();
                    }
                }
                'u' => {
                    let m = library::undo_last();
                    self.say(m, false);
                    self.load_library();
                }
                'o' => self.sort_one(true, false),
                'n' => self.sort_one(false, true),
                _ => {}
            },
            _ => {}
        }
    }

    fn rows_of(&self, tab: Tab) -> usize {
        match tab {
            Tab::Albums => self.albums.len(),
            Tab::Tracks => self.tracks.len(),
            _ => 0,
        }
    }

    // --- drawing

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        f.render_widget(Block::default().style(Style::default().bg(BG).fg(WHITE)), area);
        let [top, input, summary, tabs, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(area);

        // the top line: the name, and who you're logged in as
        f.render_widget(Block::default().style(Style::default().bg(DEEP)), top);
        f.render_widget(Paragraph::new(Span::styled(" 🦈 nekozame-seek", Style::default().fg(PINK).add_modifier(Modifier::BOLD))), top);
        f.render_widget(Paragraph::new(Span::styled(format!("{} ", self.status), Style::default().fg(MUTED))).right_aligned(), top);

        // the search box
        let focused = self.focus == Focus::Search;
        let shown = if self.query.is_empty() && !focused {
            Span::styled("search Soulseek: artist and album, or a song, then Enter (/ to type)", Style::default().fg(DIM))
        } else if self.query.is_empty() {
            Span::styled("search Soulseek: artist and album, or a song, then Enter", Style::default().fg(DIM))
        } else {
            Span::raw(self.query.clone())
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if focused { PINK } else { SELECTION }))
            .style(Style::default().bg(DEEP));
        f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), shown])).block(block), input);
        if focused {
            let x = input.x + 2 + self.query.chars().take(self.cursor).map(|c| if c.is_ascii() { 1 } else { 2 }).sum::<u16>();
            f.set_cursor_position((x.min(input.right().saturating_sub(2)), input.y + 1));
        }

        // what's going on, or the last thing that happened
        let line = if let Some((t, bad, _)) = &self.toast {
            Span::styled(format!(" {t}"), Style::default().fg(if *bad { BUTTER } else { SOFT }).add_modifier(Modifier::BOLD))
        } else if self.sid.is_some() {
            let counts = format!("{} albums and {} tracks", self.albums.len(), thousands(self.tracks.len() as u64));
            let how = self.sort.describe();
            let typed = self.wanted.clone().unwrap_or_else(|| self.text.clone());
            let note = match &self.wanted {
                Some(_) => format!(" (Soulseek won't search it whole, so: “{}”, then what matches)", self.text),
                None => String::new(),
            };
            Span::styled(
                if self.searching {
                    format!(" searching for “{typed}”… {counts} so far, {how}{note}")
                } else if self.albums.is_empty() && self.tracks.is_empty() {
                    format!(" nothing found for “{typed}”: try other words{note}")
                } else {
                    format!(" {counts} for “{typed}”, {how}{note} · Enter downloads the one picked")
                },
                Style::default().fg(SOFT),
            )
        } else {
            Span::raw("")
        };
        f.render_widget(Paragraph::new(line), summary);

        // the tabs
        let mut spans = vec![Span::raw(" ")];
        for (tab, name) in TABS {
            let on = tab == self.tab;
            spans.push(Span::styled(
                name,
                if on { Style::default().fg(PINK).add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { Style::default().fg(MUTED) },
            ));
            spans.push(Span::raw("   "));
        }
        f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(PANEL)), tabs);

        match self.tab {
            Tab::Albums if self.sid.is_none() => self.draw_help(f, body),
            Tab::Albums => self.draw_albums(f, body),
            Tab::Tracks => self.draw_tracks(f, body),
            Tab::Recent => self.draw_recent(f, body),
            Tab::Downloads => self.draw_downloads(f, body),
            Tab::Library => self.draw_library(f, body),
        }

        // the keys
        let keys: &[(&str, &str)] = match (self.focus, self.tab) {
            (Focus::Search, _) => &[("Enter", "search"), ("↑ ↓", "recent searches"), ("Esc", "to the list"), ("ctrl+c", "quit")],
            (Focus::Edit, _) => &[("Enter", "keep it"), ("Esc", "never mind")],
            (_, Tab::Library) => &[("Enter", "move it"), ("e", "change where"), ("k", "kind"), ("x", "leave it"), ("o", "replace"), ("n", "keep both"), ("u", "undo"), ("/", "search"), ("q", "quit")],
            _ => &[("Enter", "download"), ("space", "what's inside"), ("b s f", "best / biggest / fastest"), ("a t r d l", "tabs"), ("/", "search"), ("q", "quit")],
        };
        let mut spans = vec![Span::raw(" ")];
        for (k, what) in keys {
            spans.push(Span::styled(*k, Style::default().fg(PINK).add_modifier(Modifier::BOLD)));
            spans.push(Span::styled(format!(" {what}   "), Style::default().fg(MUTED)));
        }
        f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(DEEP)), footer);
    }

    fn draw_help(&self, f: &mut Frame, area: Rect) {
        let p = |t: &str| Span::styled(t.to_string(), Style::default().fg(PINK).add_modifier(Modifier::BOLD));
        let s = |t: &str| Span::raw(t.to_string());
        let n = |t: &str| Span::styled(t.to_string(), Style::default().fg(SOFT));
        let text = vec![
            Line::from(p("How it works")),
            Line::from(""),
            Line::from(vec![s("  Type what you're looking for (artist and album, or a song) and press "), p("Enter"), s(".")]),
            Line::from(""),
            Line::from(vec![s("  Albums are on the "), p("albums"), s(" tab, single songs on "), p("tracks"), s(". Both come in best first:")]),
            Line::from(vec![n("    1. "), s("lossless (FLAC, WAV…) before lossy (MP3…)")]),
            Line::from(vec![n("    2. "), s("the highest bit depth: 32, then 24, then 16")]),
            Line::from(vec![n("    3. "), s("the highest sample rate: 192 kHz, then 96, then 48, then 44.1")]),
            Line::from(vec![n("    4. "), s("the biggest, counting only the music")]),
            Line::from(""),
            Line::from(vec![s("  "), p("★"), s(" marks the best one, already picked: "), p("Enter"), s(" downloads it (an album: all of it).")]),
            Line::from(vec![s("  "), p("↑ ↓"), s(" to pick another, "), p("space"), s(" to see what's inside an album.")]),
            Line::from(""),
            Line::from(vec![s("  "), p("recent"), s(" has your past searches, "), p("downloads"), s(" what's coming, and "), p("library"), s(" puts what's done in your library.")]),
        ];
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).block(Block::default().style(Style::default().bg(BG))), area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 1 }));
    }

    fn table<'a>(rows: Vec<Row<'a>>, header: &'a [&'a str], widths: Vec<Constraint>) -> Table<'a> {
        Table::new(rows, widths)
            .header(Row::new(header.iter().map(|h| Cell::from(*h))).style(Style::default().fg(PINK).bg(DEEP).add_modifier(Modifier::BOLD)))
            .style(Style::default().bg(PANEL).fg(WHITE))
            .row_highlight_style(Style::default().bg(PINK).fg(DEEP).add_modifier(Modifier::BOLD))
            .column_spacing(2)
    }

    fn entry_bits(e: &Entry) -> (String, String) {
        let bits = if e.lossless { e.depth.to_string() } else if e.bitrate > 0 { format!("{}k", e.bitrate) } else { "?".into() };
        (bits, rank::khz(e.rate))
    }

    fn slot(e: &Entry) -> String {
        if e.free { "now".into() } else { format!("{} in line", e.queue) }
    }

    fn draw_albums(&mut self, f: &mut Frame, area: Rect) {
        let (list, inside) = if self.inside {
            let [a, b] = Layout::vertical([Constraint::Min(3), Constraint::Length(14)]).areas(area);
            (a, Some(b))
        } else {
            (area, None)
        };
        let star = self.sort == Sort::Quality;
        let rows: Vec<Row> = self
            .albums
            .iter()
            .take(500)
            .enumerate()
            .map(|(i, e)| {
                let (bits, khz) = App::entry_bits(e);
                Row::new(vec![
                    if i == 0 && star { "★".to_string() } else { String::new() },
                    bits,
                    khz,
                    e.format.clone(),
                    format!("{:.0} MB", e.mb),
                    e.audio.len().to_string(),
                    e.user.chars().take(18).collect(),
                    App::slot(e),
                    rank::title(e),
                ])
            })
            .collect();
        let w = vec![
            Constraint::Length(1), Constraint::Length(4), Constraint::Length(5), Constraint::Length(10),
            Constraint::Length(8), Constraint::Length(6), Constraint::Length(18), Constraint::Length(11), Constraint::Fill(1),
        ];
        const H: &[&str] = &["", "bits", "kHz", "format", "size", "tracks", "from", "can start", "album"];
        f.render_stateful_widget(App::table(rows, H, w), list, &mut self.tables[0]);
        if let Some(area) = inside {
            let at = self.tables[0].selected().unwrap_or(0);
            if let Some(e) = self.albums.get(at) {
                let mut files: Vec<&rank::File> = e.files.iter().collect();
                files.sort_by_key(|f| (!e.audio.iter().any(|a| a.filename == f.filename), f.filename.clone()));
                let rows: Vec<Row> = files
                    .iter()
                    .map(|f| {
                        let music = e.audio.iter().any(|a| a.filename == f.filename);
                        let bits = if !music { String::new() } else if f.lossless() { if f.depth > 0 { f.depth.to_string() } else { String::new() } } else { format!("{}k", f.bitrate) };
                        Row::new(vec![
                            if music { "♪".to_string() } else { "·".to_string() },
                            f.name().to_string(),
                            bits,
                            if music { rank::khz(f.rate) } else { String::new() },
                            rank::minutes(f.length),
                            format!("{:.1} MB", f.size as f64 / 1e6),
                        ])
                    })
                    .collect();
                let w = vec![Constraint::Length(1), Constraint::Fill(1), Constraint::Length(4), Constraint::Length(5), Constraint::Length(6), Constraint::Length(10)];
                const H: &[&str] = &["", "inside", "bits", "kHz", "length", "size"];
                let t = App::table(rows, H, w).style(Style::default().bg(DEEP).fg(WHITE)).block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(PINK)));
                f.render_widget(t, area);
            }
        }
    }

    fn draw_tracks(&mut self, f: &mut Frame, area: Rect) {
        let star = self.sort == Sort::Quality;
        let rows: Vec<Row> = self
            .tracks
            .iter()
            .take(800)
            .enumerate()
            .map(|(i, e)| {
                let (bits, khz) = App::entry_bits(e);
                Row::new(vec![
                    if i == 0 && star { "★".to_string() } else { String::new() },
                    bits,
                    khz,
                    e.format.clone(),
                    format!("{:.1} MB", e.mb),
                    rank::minutes(e.length),
                    e.user.chars().take(18).collect(),
                    App::slot(e),
                    e.name.clone().unwrap_or_default(),
                    rank::title(e),
                ])
            })
            .collect();
        let w = vec![
            Constraint::Length(1), Constraint::Length(4), Constraint::Length(5), Constraint::Length(6), Constraint::Length(9),
            Constraint::Length(6), Constraint::Length(18), Constraint::Length(11), Constraint::Fill(3), Constraint::Fill(2),
        ];
        const H: &[&str] = &["", "bits", "kHz", "format", "size", "length", "from", "can start", "track", "in"];
        f.render_stateful_widget(App::table(rows, H, w), area, &mut self.tables[1]);
    }

    fn draw_recent(&mut self, f: &mut Frame, area: Rect) {
        let rows: Vec<Row> = self
            .past
            .iter()
            .map(|x| {
                let when = x["startedAt"]
                    .as_str()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                Row::new(vec![
                    x["searchText"].as_str().unwrap_or("").to_string(),
                    when,
                    format!("{} users", x["responseCount"].as_u64().unwrap_or(0)),
                    thousands(x["fileCount"].as_u64().unwrap_or(0)),
                ])
            })
            .collect();
        if self.tables[2].selected().is_none() && !rows.is_empty() {
            self.tables[2].select(Some(0));
        }
        let w = vec![Constraint::Fill(1), Constraint::Length(16), Constraint::Length(12), Constraint::Length(8)];
        const H: &[&str] = &["search (Enter opens it again)", "when", "results from", "files"];
        f.render_stateful_widget(App::table(rows, H, w), area, &mut self.tables[2]);
    }

    fn draw_downloads(&mut self, f: &mut Frame, area: Rect) {
        let mut rows = vec![];
        for u in &self.transfers {
            for d in u["directories"].as_array().into_iter().flatten() {
                let files = d["files"].as_array().cloned().unwrap_or_default();
                let size: u64 = files.iter().map(|x| x["size"].as_u64().unwrap_or(0)).sum::<u64>().max(1);
                let done: u64 = files.iter().map(|x| x["bytesTransferred"].as_u64().unwrap_or(0)).sum();
                let mut states: Vec<String> = files
                    .iter()
                    .map(|x| x["state"].as_str().unwrap_or("").rsplit(',').next().unwrap_or("").trim().to_lowercase())
                    .collect();
                states.sort();
                states.dedup();
                let state = if states == ["succeeded"] { "done".to_string() } else { states.join(", ") };
                let dir = d["directory"].as_str().unwrap_or("");
                let parts: Vec<&str> = dir.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
                rows.push(Row::new(vec![
                    parts[parts.len().saturating_sub(2)..].join(" / "),
                    u["username"].as_str().unwrap_or("").to_string(),
                    files.len().to_string(),
                    format!("{:.0}%", 100.0 * done as f64 / size as f64),
                    state,
                ]));
            }
        }
        let w = vec![Constraint::Fill(1), Constraint::Length(18), Constraint::Length(6), Constraint::Length(5), Constraint::Length(20)];
        const H: &[&str] = &["album", "from", "tracks", "done", "state"];
        f.render_stateful_widget(App::table(rows, H, w), area, &mut self.tables[3]);
    }

    fn draw_library(&mut self, f: &mut Frame, area: Rect) {
        let editing = self.focus == Focus::Edit;
        let [help, list, edit] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(if editing { 3 } else { 0 }),
        ])
        .areas(area);
        let p = |t: &str| Span::styled(t.to_string(), Style::default().fg(PINK).add_modifier(Modifier::BOLD));
        let s = |t: &str| Span::styled(t.to_string(), Style::default().fg(MUTED));
        let first = if !self.cfg.library.is_dir() || !self.cfg.downloads.is_dir() {
            Line::from(s(" the library tab sorts on the machine with the music: open nekozame-seek there"))
        } else {
            Line::from(vec![
                Span::styled(format!(" {} finished downloads", self.todo.len()), Style::default().fg(WHITE).add_modifier(Modifier::BOLD)),
                s(" to put in your library. Nothing moves until you say so; nothing is ever overwritten."),
            ])
        };
        let text = vec![
            first,
            Line::from(vec![s(" "), p("Enter"), s(": move it · "), p("e"), s(": change where · "), p("k"), s(": artist album / game / film / unsorted · "), p("x"), s(": leave it · "), p("u"), s(": undo the last move")]),
            Line::from(vec![s(" "), p("!"), s(" you have it already · "), p("o"), s(": replace it (the old one is kept aside) · "), p("n"), s(": keep both")]),
        ];
        f.render_widget(Paragraph::new(text), help);
        let lib = self.cfg.library.clone();
        let rows: Vec<Row> = self
            .todo
            .iter()
            .zip(&self.notes)
            .map(|(t, note)| {
                Row::new(vec![
                    if note.is_empty() { "→".to_string() } else { "!".to_string() },
                    t.src.file_name().unwrap_or_default().to_string_lossy().to_string(),
                    t.kind.name().to_string(),
                    note.clone(),
                    t.dest.strip_prefix(&lib).unwrap_or(&t.dest).display().to_string(),
                ])
            })
            .collect();
        let w = vec![Constraint::Length(1), Constraint::Fill(2), Constraint::Length(22), Constraint::Length(30), Constraint::Fill(3)];
        const H: &[&str] = &["", "download", "is", "you have", "goes to"];
        f.render_stateful_widget(App::table(rows, H, w), list, &mut self.tables[4]);
        if editing {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(PINK))
                .title(Span::styled(" where it goes, inside the library ", Style::default().fg(PINK)))
                .style(Style::default().bg(DEEP));
            f.render_widget(Paragraph::new(format!(" {}", self.edit)).block(block), edit);
            let x = edit.x + 2 + self.edit.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum::<u16>();
            f.set_cursor_position((x.min(edit.right().saturating_sub(2)), edit.y + 1));
        }
    }
}

/// a line of text being typed: letters, Backspace, Delete, ← →, Home, End
fn edit_line(text: &mut String, cursor: &mut usize, k: KeyEvent) {
    let mut chars: Vec<char> = text.chars().collect();
    *cursor = (*cursor).min(chars.len());
    match k.code {
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
            chars.insert(*cursor, c);
            *cursor += 1;
        }
        KeyCode::Backspace if *cursor > 0 => {
            chars.remove(*cursor - 1);
            *cursor -= 1;
        }
        KeyCode::Delete if *cursor < chars.len() => {
            chars.remove(*cursor);
        }
        KeyCode::Left => *cursor = cursor.saturating_sub(1),
        KeyCode::Right => *cursor = (*cursor + 1).min(chars.len()),
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = chars.len(),
        _ => {}
    }
    *text = chars.into_iter().collect();
}

/// whether an album or track has every word typed in its folders or file names (in any case,
/// with or without spaces and punctuation)
fn matches(e: &Entry, wanted: &str) -> bool {
    let squash = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let mut hay = squash(&e.path.join(" "));
    for f in &e.files {
        hay.push_str(&squash(f.name()));
    }
    wanted
        .split_whitespace()
        .filter(|w| !SMALL.contains(&w.to_lowercase().as_str()))
        .map(squash)
        .filter(|w| !w.is_empty())
        .all(|w| hay.contains(&w))
}

/// words left out of matching and starring: folders often drop them ("Beatles - Abbey Road")
const SMALL: &[&str] = &["the", "a", "an", "and", "of", "&"];

/// other ways to ask Soulseek for the same thing, in order: each longer word's first letter
/// as a wildcard (the server's blocked list doesn't catch "*eatles"), then fewer words of that
fn other_ways(text: &str) -> Vec<String> {
    let words: Vec<String> = text
        .split_whitespace()
        .filter(|w| !SMALL.contains(&w.to_lowercase().as_str()))
        .map(|w| {
            let chars: Vec<char> = w.chars().collect();
            if chars.len() >= 5 { format!("*{}", chars[1..].iter().collect::<String>()) } else { w.to_string() }
        })
        .collect();
    let mut out: Vec<String> = (1..=words.len()).rev().map(|n| words[..n].join(" ")).collect();
    out.retain(|w| w.contains('*') && w != text); // a search with no wildcard was blocked already
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ways_round_blocked_searches() {
        assert_eq!(other_ways("the beatles"), ["*eatles"]);
        assert_eq!(other_ways("The Beatles Abbey Road"), ["*eatles *bbey Road", "*eatles *bbey", "*eatles"]);
        assert_eq!(other_ways("aphex twin selected ambient works")[0], "*phex twin *elected *mbient *orks");
        assert_eq!(other_ways("aphex twin selected ambient works").last().unwrap(), "*phex");
    }
}

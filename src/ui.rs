//! The screen: a search box, then tabs for albums, tracks, recent searches, downloads and the
//! library (finished downloads into the library). slskd is asked in the background, so the
//! screen never waits on it.

use crate::api::{thousands, Slskd};
use crate::library::{self, Done};
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
    Sharing,
}

const TABS: [(Tab, &str); 6] = [
    (Tab::Albums, "albums"),
    (Tab::Tracks, "tracks"),
    (Tab::Recent, "recent"),
    (Tab::Downloads, "downloads"),
    (Tab::Library, "library"),
    (Tab::Sharing, "sharing"),
];

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Search,
    Table,
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
    todo: Vec<Done>,
    skipped: HashSet<PathBuf>,
    toast: Option<(String, bool, Instant)>,
    tables: [TableState; 6],
    share_dir: Option<PathBuf>,
    share_folders: Vec<PathBuf>,
    shares: Option<crate::shares::Shares>,
    last_poll: Instant,
    last_transfers: Instant,
    last_status: Instant,
    last_library: Instant,
    history_at: Option<Instant>,
    quit: bool,
    beets_now: Option<PathBuf>, // a download to hand to beets, with the screen given to it
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
        skipped: HashSet::new(),
        toast: None,
        tables: Default::default(),
        share_dir: None,
        share_folders: vec![],
        shares: None,
        last_poll: long_ago,
        last_transfers: long_ago,
        last_status: long_ago,
        last_library: long_ago,
        history_at: Some(Instant::now()),
        quit: false,
        beets_now: None,
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
            if let (Some(path), Some(beet)) = (app.beets_now.take(), app.cfg.beets.clone()) {
                // beets asks its questions in the terminal: the screen is its until it's done
                ratatui::restore();
                println!("\nbeets, for {} (its questions are its own; nekozame-seek comes back after)\n", path.display());
                let ok = library::beets(&beet, &["import"], &path);
                terminal = ratatui::init();
                terminal.clear()?;
                app.say(if ok { "beets put it in your library" } else { "it's still in downloads (beets skipped it, or you did)" }, !ok);
                app.load_library();
            }
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
        if self.tab == Tab::Library && now - self.last_library > Duration::from_secs(10) {
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

    // --- the library tab: finished downloads, handed to beets

    fn load_library(&mut self) {
        self.last_library = Instant::now();
        if !self.cfg.downloads.is_dir() {
            self.todo.clear();
            return;
        }
        self.todo = library::done(&self.cfg.downloads, &self.transfers)
            .into_iter()
            .filter(|d| !self.skipped.contains(&d.path))
            .collect();
        let t = &mut self.tables[App::idx(Tab::Library)];
        if self.todo.is_empty() {
            t.select(None);
        } else if t.selected().is_none_or(|i| i >= self.todo.len()) {
            t.select(Some(0));
        }
    }

    // --- the sharing tab: your folders, shared on Soulseek or not

    fn load_sharing(&mut self) {
        let Some(file) = &self.cfg.slskd_config else { return };
        match crate::shares::Shares::read(file, self.cfg.slskd_paths.as_deref()) {
            Ok(s) => {
                if self.share_dir.is_none() {
                    self.share_dir = s.shared.iter().find(|p| p.is_dir()).cloned();
                }
                self.shares = Some(s);
            }
            Err(e) => self.say(e, true),
        }
        self.share_folders = self.share_dir.as_deref().map(crate::shares::folders).unwrap_or_default();
        let t = &mut self.tables[App::idx(Tab::Sharing)];
        if self.share_folders.is_empty() {
            t.select(None);
        } else if t.selected().is_none_or(|i| i >= self.share_folders.len()) {
            t.select(Some(0));
        }
    }

    fn share_key(&mut self, code: KeyCode) {
        let at = self.tables[App::idx(Tab::Sharing)].selected();
        let here = at.and_then(|i| self.share_folders.get(i).cloned());
        match code {
            KeyCode::Enter | KeyCode::Right => {
                if let Some(p) = here {
                    self.share_dir = Some(p);
                    self.tables[App::idx(Tab::Sharing)].select(Some(0));
                    self.load_sharing();
                }
            }
            KeyCode::Left | KeyCode::Backspace => {
                if let Some(up) = self.share_dir.as_ref().and_then(|d| d.parent()).map(|p| p.to_path_buf()) {
                    let from = self.share_dir.replace(up);
                    self.load_sharing();
                    if let Some(i) = self.share_folders.iter().position(|p| Some(p) == from.as_ref()) {
                        self.tables[App::idx(Tab::Sharing)].select(Some(i));
                    }
                }
            }
            KeyCode::Char(' ') => {
                let (Some(p), Some(s)) = (here, self.shares.as_mut()) else { return };
                match s.toggle(&p) {
                    Ok(what) => {
                        self.say(format!("{what} (slskd is looking through your files again)"), false);
                        self.background(|api| match api.rescan() {
                            Ok(()) => Msg::Status(api.status()),
                            Err(e) => Msg::Note(format!("slskd didn't look again: {e}"), true),
                        });
                    }
                    Err(e) => self.say(e, true),
                }
            }
            _ => {}
        }
    }

    fn library_row(&self) -> Option<usize> {
        if self.todo.is_empty() {
            return None;
        }
        self.tables[App::idx(Tab::Library)].selected().map(|i| i.min(self.todo.len() - 1))
    }

    // --- keys

    fn key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.focus {
            Focus::Search => self.key_search(k),
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

    fn rows(&self) -> usize {
        match self.tab {
            Tab::Albums => self.albums.len(),
            Tab::Tracks => self.tracks.len(),
            Tab::Recent => self.past.len(),
            Tab::Downloads => self.transfers.iter().map(|u| u["directories"].as_array().map_or(0, |d| d.len())).sum(),
            Tab::Library => self.todo.len(),
            Tab::Sharing => self.share_folders.len(),
        }
    }

    fn key_table(&mut self, k: KeyEvent) {
        if self.tab == Tab::Sharing && matches!(k.code, KeyCode::Enter | KeyCode::Right | KeyCode::Left | KeyCode::Backspace | KeyCode::Char(' ')) {
            self.share_key(k.code);
            return;
        }
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
            KeyCode::Char('h') => {
                self.tab = Tab::Sharing;
                self.load_sharing();
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
                Tab::Library => match (self.library_row(), self.cfg.beets.is_some()) {
                    (Some(i), true) => self.beets_now = Some(self.todo[i].path.clone()),
                    (_, false) => self.say("putting downloads away needs beets: see docs/library.md", true),
                    _ => {}
                },
                Tab::Downloads | Tab::Sharing => {}
            },
            KeyCode::Char('x') if self.tab == Tab::Library => {
                if let Some(i) = self.library_row() {
                    self.skipped.insert(self.todo[i].path.clone());
                    self.load_library();
                }
            }
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
            Tab::Sharing => self.draw_sharing(f, body),
        }

        // the keys
        let keys: &[(&str, &str)] = match (self.focus, self.tab) {
            (Focus::Search, _) => &[("Enter", "search"), ("↑ ↓", "recent searches"), ("Esc", "to the list"), ("ctrl+c", "quit")],
            (_, Tab::Library) => &[("Enter", "beets, for this one"), ("x", "leave it"), ("a t r d h", "tabs"), ("/", "search"), ("q", "quit")],
            (_, Tab::Sharing) => &[("space", "share / don't share"), ("Enter →", "open"), ("←", "up"), ("a t r d l", "tabs"), ("q", "quit")],
            _ => &[("Enter", "download"), ("space", "what's inside"), ("b s f", "best / biggest / fastest"), ("a t r d l h", "tabs"), ("/", "search"), ("q", "quit")],
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
            Line::from(vec![s("  "), p("recent"), s(" has your past searches, "), p("downloads"), s(" what's coming, "), p("library"), s(" puts what's done in your library,")]),
            Line::from(vec![s("  and "), p("sharing"), s(" is what others can download from you.")]),
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

    fn draw_sharing(&mut self, f: &mut Frame, area: Rect) {
        use crate::shares::State;
        let [help, list] = Layout::vertical([Constraint::Length(2), Constraint::Min(3)]).areas(area);
        let s = |t: &str| Span::styled(t.to_string(), Style::default().fg(MUTED));
        let p = |t: &str| Span::styled(t.to_string(), Style::default().fg(PINK).add_modifier(Modifier::BOLD));
        let text = match (&self.cfg.slskd_config, &self.share_dir) {
            (None, _) => vec![Line::from(s(" set slskd_config in ~/.config/nekozame-seek/config.toml (on slskd's machine) to choose what you share here"))],
            (_, None) => vec![Line::from(s(" nothing is shared yet, and no folder to start from"))],
            (_, Some(dir)) => vec![
                Line::from(vec![s(" what others can browse and download from you, in "), p(&dir.display().to_string())]),
                Line::from(vec![s(" "), p("space"), s(": share it or not · "), p("Enter"), s(": open · "), p("←"), s(": up")]),
            ],
        };
        f.render_widget(Paragraph::new(text), help);
        let rows: Vec<Row> = self
            .share_folders
            .iter()
            .map(|p| {
                let (mark, what) = match self.shares.as_ref().map(|s| s.state(p)) {
                    Some(State::Shared) => ("●", "shared"),
                    Some(State::LeftOut) => ("○", "not shared"),
                    Some(State::InLeftOut) => ("○", "not shared (its folder isn't)"),
                    _ => ("·", "not shared"),
                };
                Row::new(vec![mark.to_string(), p.file_name().unwrap_or_default().to_string_lossy().to_string(), what.to_string()])
            })
            .collect();
        let w = vec![Constraint::Length(1), Constraint::Fill(1), Constraint::Length(30)];
        const H: &[&str] = &["", "folder", ""];
        f.render_stateful_widget(App::table(rows, H, w), list, &mut self.tables[5]);
    }

    fn draw_library(&mut self, f: &mut Frame, area: Rect) {
        let [help, list] = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(area);
        let p = |t: &str| Span::styled(t.to_string(), Style::default().fg(PINK).add_modifier(Modifier::BOLD));
        let s = |t: &str| Span::styled(t.to_string(), Style::default().fg(MUTED));
        let text = if self.cfg.beets.is_none() {
            vec![
                Line::from(vec![s(" Putting downloads in your library needs "), p("beets"), s(": it matches them on MusicBrainz, fixes their tags")]),
                Line::from(s(" and puts them where your library has them. Install it, and see docs/library.md.")),
            ]
        } else if !self.cfg.downloads.is_dir() {
            vec![Line::from(s(" set downloads in ~/.config/nekozame-seek/config.toml (on the machine with the music)"))]
        } else {
            vec![
                Line::from(vec![
                    Span::styled(format!(" {} finished downloads", self.todo.len()), Style::default().fg(WHITE).add_modifier(Modifier::BOLD)),
                    s(" waiting for you (what beets is sure of goes by itself, with nekozame-seek organize)."),
                ]),
                Line::from(vec![s(" "), p("Enter"), s(": beets, for this one (it shows its match and asks you) · "), p("x"), s(": leave it")]),
            ]
        };
        f.render_widget(Paragraph::new(text), help);
        let rows: Vec<Row> = self
            .todo
            .iter()
            .map(|d| {
                Row::new(vec![
                    d.path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                    if d.soundtrack { "soundtrack".to_string() } else { "album".to_string() },
                    d.quality.clone(),
                ])
            })
            .collect();
        let w = vec![Constraint::Fill(1), Constraint::Length(12), Constraint::Length(18)];
        const H: &[&str] = &["download", "looks like", "quality"];
        f.render_stateful_widget(App::table(rows, H, w), list, &mut self.tables[4]);
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

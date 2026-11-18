//! Finished downloads put where they belong in the library, the library's own way: a folder
//! per artist in "Artist Albums", each album in it under the name it came with (tracks keep
//! theirs too); soundtracks in "Soundtracks/Interactive Media" (games) or "Linear Media"
//! (films, series, anime); single songs loose in the artist's folder. Nothing moves without
//! being shown first, nothing is ever overwritten, and every move is written down so it can
//! be undone.

use crate::rank::{is_audio, LOSSLESS};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Artist,
    Game,
    Film,
    Unsorted,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Artist, Kind::Game, Kind::Film, Kind::Unsorted];

    pub fn folder(self) -> &'static str {
        match self {
            Kind::Artist => "Artist Albums",
            Kind::Game => "Soundtracks/Interactive Media",
            Kind::Film => "Soundtracks/Linear Media",
            Kind::Unsorted => "Unsorted Archives",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Artist => "artist album",
            Kind::Game => "game soundtrack",
            Kind::Film => "film/series soundtrack",
            Kind::Unsorted => "unsorted",
        }
    }

    pub fn next(self) -> Kind {
        let i = Kind::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Kind::ALL[(i + 1) % Kind::ALL.len()]
    }
}

pub struct Todo {
    pub src: PathBuf,
    pub kind: Kind,
    pub artist: String,
    pub dest: PathBuf,
}

static GAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(game|video ?game|vgm|nintendo|sega|playstation|xbox|snes|gba|psx|chiptune|soundtrack for the game)\b").unwrap()
});
static SOUNDTRACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(ost|o\.s\.t\.?|original (motion picture |video game |game )?soundtrack|soundtrack|score)\b").unwrap()
});
static ARTIST_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\[[^\]]*\]\s*|\(\d{4}\)\s*|\d{4}\s*[-.]\s*)?(.+?)\s+-\s+.+").unwrap());

fn ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()
}

pub fn music_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(music_files(&p));
            } else if is_audio(&ext(&p)) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// the first music file's tags: (albumartist, artist, album, genre)
fn tags(dir: &Path) -> (String, String, String, String) {
    use lofty::prelude::*;
    for f in music_files(dir) {
        if let Ok(t) = lofty::read_from_path(&f) {
            if let Some(tag) = t.primary_tag().or_else(|| t.first_tag()) {
                let get = |k: ItemKey| tag.get_string(k).unwrap_or("").to_string();
                return (get(ItemKey::AlbumArtist), get(ItemKey::TrackArtist), get(ItemKey::AlbumTitle), get(ItemKey::Genre));
            }
        }
    }
    Default::default()
}

/// an artist's name to compare: no case, accents, "the" or punctuation
pub fn plain(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let n: String = name.nfkd().filter(|c| c.is_ascii()).collect::<String>().to_lowercase();
    let n = n.strip_prefix("the ").unwrap_or(&n).to_string();
    n.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// the artist's folder as it already is in the library (its spelling, even a near one like
/// Gorrilaz for Gorillaz), or the name as it is
pub fn artist_folder(library: &Path, artist: &str) -> String {
    let want = plain(artist);
    let folders: Vec<String> = fs::read_dir(library.join(Kind::Artist.folder()))
        .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    if let Some(d) = folders.iter().find(|d| plain(d) == want) {
        return d.clone();
    }
    let close: Vec<&String> = folders
        .iter()
        .filter(|d| !want.is_empty() && strsim::sorensen_dice(&plain(d), &want) >= 0.85)
        .collect();
    if close.len() == 1 {
        return close[0].clone();
    }
    let a = artist.replace('/', "-").trim().to_string();
    if a.is_empty() { "Unknown Artist".into() } else { a }
}

/// where a finished download goes: its kind, artist and destination, from its tags and name
pub fn guess(library: &Path, src: &Path) -> Todo {
    let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let (albumartist, trackartist, album, genre) = tags(src);
    let text = format!("{name} {album} {genre}");
    let mut artist = if albumartist.is_empty() { trackartist } else { albumartist };
    if artist.is_empty() {
        if let Some(c) = ARTIST_NAME.captures(&name) {
            artist = c[1].to_string();
        }
    }
    let kind = if SOUNDTRACK.is_match(&text) || genre.to_lowercase().contains("soundtrack") {
        if GAME.is_match(&text) { Kind::Game } else { Kind::Film }
    } else if artist.is_empty() {
        Kind::Unsorted
    } else {
        Kind::Artist
    };
    let dest = destination(library, kind, &artist, src);
    Todo { src: src.to_path_buf(), kind, artist, dest }
}

pub fn destination(library: &Path, kind: Kind, artist: &str, src: &Path) -> PathBuf {
    let name = src.file_name().unwrap_or_default();
    let music = music_files(src);
    let single = music.len() == 1 && fs::read_dir(src).map(|rd| rd.flatten().all(|e| !e.path().is_dir())).unwrap_or(false);
    if kind == Kind::Artist {
        let base = library.join(Kind::Artist.folder()).join(artist_folder(library, artist));
        // one song on its own: loose in the artist's folder, like the library's other singles
        return if single { base.join(music[0].file_name().unwrap_or_default()) } else { base.join(name) };
    }
    library.join(kind.folder()).join(name)
}

/// a folder's (or a file's) music: the best bit depth/sample rate and its size
pub fn quality_of(p: &Path) -> String {
    use lofty::prelude::*;
    let files = if p.is_dir() { music_files(p) } else { vec![p.to_path_buf()] };
    let (mut depth, mut rate, mut size) = (0u32, 0u32, 0u64);
    for f in &files {
        size += fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        if let Ok(t) = lofty::read_from_path(f) {
            let props = t.properties();
            depth = depth.max(props.bit_depth().unwrap_or(0) as u32);
            rate = rate.max(props.sample_rate().unwrap_or(0));
        }
    }
    let q = if depth > 0 { format!("{}/{} ", depth, crate::rank::khz(rate)) } else { String::new() };
    format!("{q}{:.0} MB", size as f64 / 1e6)
}

/// the download folders that are complete: every file slskd fetched for them succeeded (none
/// still downloading, waiting or failed), and none has changed in the last minute
pub fn finished(downloads: &Path, transfers: &[Value]) -> Vec<PathBuf> {
    let mut busy = HashSet::new();
    for u in transfers {
        for d in u["directories"].as_array().into_iter().flatten() {
            let all_done = d["files"].as_array().into_iter().flatten().all(|f| f["state"].as_str().unwrap_or("").contains("Succeeded"));
            if !all_done {
                if let Some(dir) = d["directory"].as_str() {
                    busy.insert(dir.rsplit(['\\', '/']).next().unwrap_or(dir).to_string());
                }
            }
        }
    }
    let mut out: Vec<PathBuf> = fs::read_dir(downloads)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    out.sort();
    out.retain(|p| {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || !p.is_dir() || busy.contains(name) {
            return false;
        }
        let music = music_files(p);
        let newest = music.iter().filter_map(|f| fs::metadata(f).and_then(|m| m.modified()).ok()).max();
        !music.is_empty() && newest.is_some_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() > Duration::from_secs(60))
    });
    out
}

fn moves_log() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local/state/nekozame-seek/moves.jsonl")
}

fn sha256(p: &Path) -> std::io::Result<Vec<u8>> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let mut f = fs::File::open(p)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().to_vec())
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)?.flatten() {
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
    } else {
        fs::copy(from, to)?;
    }
    Ok(())
}

fn files_in(p: &Path) -> Vec<PathBuf> {
    if !p.is_dir() {
        return vec![p.to_path_buf()];
    }
    let mut out = Vec::new();
    for e in fs::read_dir(p).into_iter().flatten().flatten() {
        out.extend(files_in(&e.path()));
    }
    out
}

/// src to dest, safely. On the same disk: a rename (instant, can't half-happen). On another
/// disk: every file copied and checked byte for byte first. Never over anything: something
/// that's there already is only replaced when asked, and then kept aside, not deleted.
/// Written down, so it can be undone.
pub fn move_to(src: &Path, dest: &Path, replace: bool) -> Result<(), String> {
    let e = |x: std::io::Error| x.to_string();
    let single = LOSSLESS.contains(&ext(dest).as_str()) || is_audio(&ext(dest));
    let real_src = if single { music_files(src).into_iter().next().ok_or("no music in it")? } else { src.to_path_buf() };
    let mut aside = None;
    if dest.symlink_metadata().is_ok() {
        if !replace {
            return Err(format!("{} is there already", dest.display()));
        }
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let a = dest.with_file_name(format!(".replaced-{stamp}-{}", dest.file_name().unwrap_or_default().to_string_lossy()));
        fs::rename(dest, &a).map_err(e)?;
        aside = Some(a);
    }
    let parent = dest.parent().ok_or("no folder to put it in")?;
    fs::create_dir_all(parent).map_err(e)?;
    #[cfg(unix)]
    let same_disk = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&real_src).map_err(e)?.dev() == fs::metadata(parent).map_err(e)?.dev()
    };
    #[cfg(not(unix))]
    let same_disk = false;
    if same_disk {
        fs::rename(&real_src, dest).map_err(e)?;
    } else {
        let part = PathBuf::from(format!("{}.part", dest.display()));
        copy_tree(&real_src, &part).map_err(e)?;
        for f in files_in(&real_src) {
            let copy = if single { part.clone() } else { part.join(f.strip_prefix(&real_src).unwrap_or(&f)) };
            if sha256(&f).map_err(e)? != sha256(&copy).map_err(e)? {
                let _ = if single { fs::remove_file(&part) } else { fs::remove_dir_all(&part) };
                return Err(format!("the copy of {} doesn't match: nothing moved", f.display()));
            }
        }
        fs::rename(&part, dest).map_err(e)?;
        if single { fs::remove_file(&real_src).map_err(e)? } else { fs::remove_dir_all(&real_src).map_err(e)? }
    }
    if single && src.is_dir() && music_files(src).is_empty() {
        let _ = fs::remove_dir_all(src); // what came with the song (a cover, a log) goes with its folder
    }
    let log = moves_log();
    fs::create_dir_all(log.parent().unwrap()).map_err(e)?;
    let mut f = fs::OpenOptions::new().create(true).append(true).open(&log).map_err(e)?;
    let line = json!({
        "when": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        "from": real_src, "to": dest, "aside": aside,
    });
    writeln!(f, "{line}").map_err(e)?;
    Ok(())
}

/// the last move, back where it came from (and what it replaced, back in its place)
pub fn undo_last() -> String {
    let log = moves_log();
    let text = fs::read_to_string(&log).unwrap_or_default();
    let mut lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let Some(last) = lines.pop() else { return "nothing to undo".into() };
    let Ok(m) = serde_json::from_str::<Value>(last) else { return "the moves log is broken: nothing undone".into() };
    let (from, to) = (PathBuf::from(m["from"].as_str().unwrap_or("")), PathBuf::from(m["to"].as_str().unwrap_or("")));
    if from.symlink_metadata().is_ok() {
        return format!("can't undo: {} is there again", from.display());
    }
    if let Some(p) = from.parent() {
        let _ = fs::create_dir_all(p);
    }
    if let Err(err) = fs::rename(&to, &from) {
        return format!("couldn't undo: {err}");
    }
    if let Some(aside) = m["aside"].as_str() {
        let _ = fs::rename(aside, &to);
    }
    let rest: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let _ = fs::write(&log, rest);
    format!("back in downloads: {}", from.file_name().unwrap_or_default().to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_plainly() {
        assert_eq!(plain("The Cure"), plain("Cure"));
        assert_eq!(plain("Björk"), "bjork");
        assert!(strsim::sorensen_dice(&plain("Gorillaz"), &plain("Gorrilaz")) >= 0.85);
        assert!(strsim::sorensen_dice(&plain("Slowdive"), &plain("Swans")) < 0.85);
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn near_spellings() {
        let r = |a: &str, b: &str| strsim::sorensen_dice(&plain(a), &plain(b));
        assert!(r("Billie Eilish", "Billie Eillish") >= 0.85);
        assert!(r("Bauhaus", "Bjork") < 0.85);
        assert!(r("Swans", "Slowdive") < 0.85);
    }
}

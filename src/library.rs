//! Finished downloads into your library, through beets (beets.io): it looks each one up on
//! MusicBrainz, fixes its tags and moves it where its own `paths` say. nekozame-seek finds
//! what's finished and hands it over: by itself for what beets is sure of (organize), or
//! from the library tab, where beets asks you about the rest. Soundtracks always wait for
//! you (where they go is often your call: a game's, a film's...).

use crate::rank::is_audio;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

/// a finished download, for the library tab
pub struct Done {
    pub path: PathBuf,
    pub soundtrack: bool,
    pub quality: String,
}

static SOUNDTRACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(ost|o\.s\.t\.?|original (motion picture |video game |game )?soundtrack|soundtrack|score)\b").unwrap()
});

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

/// whether it looks like a soundtrack: by its name, or its first track's album and genre tags
pub fn soundtrack(dir: &Path) -> bool {
    use lofty::prelude::*;
    let mut text = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
    if let Some(f) = music_files(dir).first() {
        if let Ok(t) = lofty::read_from_path(f) {
            if let Some(tag) = t.primary_tag().or_else(|| t.first_tag()) {
                for k in [ItemKey::AlbumTitle, ItemKey::Genre] {
                    text.push(' ');
                    text.push_str(tag.get_string(k).unwrap_or(""));
                }
            }
        }
    }
    SOUNDTRACK.is_match(&text)
}

/// a folder's music: the best bit depth/sample rate and its size
pub fn quality_of(p: &Path) -> String {
    use lofty::prelude::*;
    let (mut depth, mut rate, mut size) = (0u32, 0u32, 0u64);
    for f in music_files(p) {
        size += fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
        if let Ok(t) = lofty::read_from_path(&f) {
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
    let mut out: Vec<PathBuf> = fs::read_dir(downloads).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
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

pub fn done(downloads: &Path, transfers: &[Value]) -> Vec<Done> {
    finished(downloads, transfers)
        .into_iter()
        .map(|path| Done { soundtrack: soundtrack(&path), quality: quality_of(&path), path })
        .collect()
}

/// beets on a download: BEET as configured (a command, maybe with arguments). Afterwards the
/// download's folder goes if beets took all its music (and only if it's empty by then)
pub fn beets(beet: &str, args: &[&str], path: &Path) -> bool {
    let mut words = beet.split_whitespace();
    let mut cmd = std::process::Command::new(words.next().unwrap_or("beet"));
    let ok = cmd.args(words).args(args).arg(path).status().map(|s| s.success()).unwrap_or(false);
    if ok && music_files(path).is_empty() {
        let _ = fs::remove_dir(path);
    }
    ok && !path.exists()
}

/// what beets is sure of, put away without asking (nekozame-seek organize, from a timer); the
/// rest (and every soundtrack) waits in the library tab. what went, one line each
pub fn organize(cfg: &crate::Config, api: &crate::api::Slskd) -> Result<Vec<String>, String> {
    let Some(beet) = &cfg.beets else {
        return Err("putting downloads away needs beets: see docs/library.md".into());
    };
    if !cfg.downloads.is_dir() {
        return Err(format!("{} isn't here", cfg.downloads.display()));
    }
    let transfers = api.downloads()?; // without knowing what's still downloading, nothing moves
    let mut out = vec![];
    for path in finished(&cfg.downloads, &transfers) {
        if soundtrack(&path) {
            continue;
        }
        if beets(beet, &["import", "-q"], &path) {
            out.push(format!("{}: in your library", path.display()));
        }
    }
    Ok(out)
}

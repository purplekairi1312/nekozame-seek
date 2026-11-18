//! Search results as albums (one user's folder) and tracks, ranked: lossless before lossy,
//! then the highest bit depth, then the highest sample rate, then the biggest (counting only
//! the music, not scans, booklets or a second copy in another format).

use serde_json::Value;
use std::cmp::Ordering;
use std::collections::BTreeMap;

pub const LOSSLESS: &[&str] = &["flac", "wav", "aiff", "aif", "alac", "ape", "wv", "dsf", "dff"];
pub const LOSSY: &[&str] = &["m4a", "mp3", "ogg", "opus", "aac", "wma"];

pub fn is_audio(ext: &str) -> bool {
    LOSSLESS.contains(&ext) || LOSSY.contains(&ext)
}

#[derive(Clone)]
pub struct File {
    pub filename: String,
    pub size: u64,
    pub depth: u32,
    pub rate: u32,
    pub bitrate: u32,
    pub length: u32,
    pub ext: String,
}

impl File {
    fn from(v: &Value) -> Option<File> {
        let filename = v["filename"].as_str()?.to_string();
        let ext = v["extension"]
            .as_str()
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| filename.rsplit('.').next().unwrap_or("").to_string())
            .trim_start_matches('.')
            .to_lowercase();
        let n = |k: &str| v[k].as_u64().unwrap_or(0) as u32;
        Some(File {
            size: v["size"].as_u64().unwrap_or(0),
            depth: n("bitDepth"),
            rate: n("sampleRate"),
            bitrate: n("bitRate"),
            length: n("length"),
            filename,
            ext,
        })
    }

    pub fn name(&self) -> &str {
        self.filename.rsplit(['\\', '/']).next().unwrap_or(&self.filename)
    }

    pub fn lossless(&self) -> bool {
        LOSSLESS.contains(&self.ext.as_str())
    }
}

/// an album or a single track, with what it's ranked by
#[derive(Clone)]
pub struct Entry {
    pub user: String,
    pub path: Vec<String>,
    pub files: Vec<File>,
    pub audio: Vec<File>,
    pub name: Option<String>, // a track's file name
    pub length: u32,
    pub lossless: bool,
    pub depth: u32,
    pub rate: u32,
    pub bitrate: u32,
    pub mb: f64,
    pub format: String,
    pub free: bool,
    pub speed: u64,
    pub queue: u64,
}

pub fn folder(path: &str) -> Vec<String> {
    let mut parts: Vec<String> = path.split(['\\', '/']).map(str::to_string).collect();
    parts.pop();
    parts
}

fn peer(r: &Value) -> (String, bool, u64, u64) {
    (
        r["username"].as_str().unwrap_or("?").to_string(),
        r["hasFreeUploadSlot"].as_bool().unwrap_or(false),
        r["uploadSpeed"].as_u64().unwrap_or(0),
        r["queueLength"].as_u64().unwrap_or(0),
    )
}

/// each user's folders with music in them, as albums
pub fn albums(responses: &[Value]) -> Vec<Entry> {
    let mut out = Vec::new();
    for r in responses {
        let (user, free, speed, queue) = peer(r);
        let mut by_folder: BTreeMap<Vec<String>, Vec<File>> = BTreeMap::new();
        for f in r["files"].as_array().into_iter().flatten().filter_map(File::from) {
            by_folder.entry(folder(&f.filename)).or_default().push(f);
        }
        for (path, files) in by_folder {
            let audio: Vec<File> = files.iter().filter(|f| is_audio(&f.ext)).cloned().collect();
            if audio.is_empty() {
                continue;
            }
            let lossless: Vec<File> = audio.iter().filter(|f| f.lossless()).cloned().collect();
            let main = if lossless.is_empty() { audio } else { lossless.clone() }; // its real format
            let mut depth = main.iter().map(|f| f.depth).max().unwrap_or(0);
            let mut rate = main.iter().map(|f| f.rate).max().unwrap_or(0);
            if !lossless.is_empty() && depth == 0 {
                depth = 16; // not said: CD quality, the usual
                rate = if rate == 0 { 44100 } else { rate };
            }
            let mut formats: Vec<String> = main.iter().map(|f| f.ext.to_uppercase()).collect();
            formats.sort();
            formats.dedup();
            let mut kinds: Vec<(u32, u32)> = main.iter().map(|f| (f.depth, f.rate)).collect();
            kinds.sort();
            kinds.dedup();
            out.push(Entry {
                user: user.clone(),
                path,
                lossless: !lossless.is_empty(),
                depth,
                rate,
                bitrate: main.iter().map(|f| f.bitrate).max().unwrap_or(0),
                mb: main.iter().map(|f| f.size).sum::<u64>() as f64 / 1e6,
                format: formats.join("/") + if kinds.len() > 1 { " mixed" } else { "" },
                length: main.iter().map(|f| f.length).sum(),
                audio: main,
                files,
                name: None,
                free,
                speed,
                queue,
            });
        }
    }
    out
}

/// every music file on its own (singles and single songs)
pub fn tracks(responses: &[Value]) -> Vec<Entry> {
    let mut out = Vec::new();
    for r in responses {
        let (user, free, speed, queue) = peer(r);
        for f in r["files"].as_array().into_iter().flatten().filter_map(File::from) {
            if !is_audio(&f.ext) {
                continue;
            }
            let (mut depth, mut rate) = (f.depth, f.rate);
            if f.lossless() && depth == 0 {
                depth = 16;
                rate = if rate == 0 { 44100 } else { rate };
            }
            out.push(Entry {
                user: user.clone(),
                path: folder(&f.filename),
                name: Some(f.name().to_string()),
                length: f.length,
                lossless: f.lossless(),
                depth,
                rate,
                bitrate: f.bitrate,
                mb: f.size as f64 / 1e6,
                format: f.ext.to_uppercase(),
                files: vec![f.clone()],
                audio: vec![f],
                free,
                speed,
                queue,
            });
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
pub enum Sort {
    Quality,
    Size,
    Speed,
}

impl Sort {
    pub fn describe(self) -> &'static str {
        match self {
            Sort::Quality => "best first",
            Sort::Size => "biggest first",
            Sort::Speed => "fastest first",
        }
    }
}

fn mb(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// best first, for each way of sorting
pub fn sort(entries: &mut [Entry], how: Sort) {
    entries.sort_by(|a, b| {
        let o = match how {
            Sort::Quality => a
                .lossless
                .cmp(&b.lossless)
                .then(a.depth.cmp(&b.depth))
                .then(a.rate.cmp(&b.rate))
                .then(if a.lossless { Ordering::Equal } else { a.bitrate.cmp(&b.bitrate) })
                .then(mb(a.mb, b.mb))
                .then(a.free.cmp(&b.free))
                .then(a.speed.cmp(&b.speed)),
            Sort::Size => mb(a.mb, b.mb)
                .then(a.lossless.cmp(&b.lossless))
                .then(a.depth.cmp(&b.depth))
                .then(a.rate.cmp(&b.rate)),
            Sort::Speed => a.free.cmp(&b.free).then(a.speed.cmp(&b.speed)).then(b.queue.cmp(&a.queue)),
        };
        o.reverse()
    });
}

/// "24/96", "320k"
pub fn quality(e: &Entry) -> String {
    if e.lossless {
        format!("{}/{}", e.depth, khz(e.rate))
    } else if e.bitrate > 0 {
        format!("{}k", e.bitrate)
    } else {
        "?".into()
    }
}

pub fn khz(rate: u32) -> String {
    if rate == 0 {
        return String::new();
    }
    let k = rate as f64 / 1000.0;
    if k.fract() == 0.0 { format!("{k:.0}") } else { format!("{k}") }
}

/// the last two folders: "Slowdive / Souvlaki (1993)"
pub fn title(e: &Entry) -> String {
    let parts: Vec<&String> = e.path.iter().filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return "(top folder)".into();
    }
    parts[parts.len().saturating_sub(2)..].iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" / ")
}

pub fn minutes(seconds: u32) -> String {
    if seconds == 0 { String::new() } else { format!("{}:{:02}", seconds / 60, seconds % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn best_album_first() {
        let resp = vec![
            json!({"username": "a", "hasFreeUploadSlot": true, "files": [
                {"filename": "M\\Slowdive\\Souvlaki (24-96)\\01.flac", "size": 90_000_000, "bitDepth": 24, "sampleRate": 96000},
                {"filename": "M\\Slowdive\\Souvlaki (24-96)\\scan.jpg", "size": 500_000_000}]}),
            json!({"username": "b", "files": [
                {"filename": "x\\Souvlaki\\01.flac", "size": 300_000_000, "bitDepth": 24, "sampleRate": 192000},
                {"filename": "x\\Souvlaki\\02.flac", "size": 300_000_000, "bitDepth": 24, "sampleRate": 192000}]}),
            json!({"username": "c", "files": [{"filename": "y\\Souvlaki CD\\01.flac", "size": 400_000_000}]}),
            json!({"username": "d", "files": [{"filename": "z\\Souvlaki mp3\\01.mp3", "size": 900_000_000, "bitRate": 320}]}),
            json!({"username": "e", "files": [{"filename": "w\\Big\\01.flac", "size": 700_000_000, "bitDepth": 24, "sampleRate": 192000}]}),
            json!({"username": "f", "files": [{"filename": "v\\Float\\01.flac", "size": 10_000_000, "bitDepth": 32, "sampleRate": 44100}]}),
        ];
        let mut a = albums(&resp);
        sort(&mut a, Sort::Quality);
        let order: Vec<&str> = a.iter().map(|e| e.user.as_str()).collect();
        assert_eq!(order, ["f", "e", "b", "a", "c", "d"]);
        assert_eq!(quality(&a[3]), "24/96");
        assert_eq!(a[3].mb, 90.0); // the scans don't count
        assert_eq!(quality(&a[4]), "16/44.1");
    }
}

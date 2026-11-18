//! What you share on Soulseek: slskd's shared folders ("shares: directories:" in its
//! slskd.yml, "!" before the ones left out), read and changed folder by folder. slskd picks
//! up the change by itself; nekozame-seek then has it look through your files again.

use std::fs;
use std::path::{Path, PathBuf};

pub struct Shares {
    pub file: PathBuf,
    /// slskd's paths and this machine's, when slskd sees them elsewhere (in a container)
    map: Option<(String, String)>,
    /// what's shared and what's left out, as this machine sees them
    pub shared: Vec<PathBuf>,
    pub excluded: Vec<PathBuf>,
}

#[derive(PartialEq)]
pub enum State {
    Shared,
    LeftOut,   // left out itself
    InLeftOut, // in a folder that's left out
    NotShared, // in nothing that's shared
}

impl Shares {
    /// slskd.yml's shares; MAP is "slskd's path=this machine's" ("/music=/mnt/Music")
    pub fn read(file: &Path, map: Option<&str>) -> Result<Shares, String> {
        let text = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let map = map.and_then(|m| m.split_once('=')).map(|(a, b)| (a.trim_end_matches('/').to_string(), b.trim_end_matches('/').to_string()));
        let mut s = Shares { file: file.to_path_buf(), map, shared: vec![], excluded: vec![] };
        let (_, lines, _) = block(&text);
        for raw in lines {
            if let Some(p) = raw.strip_prefix('!') {
                s.excluded.push(s.here(p));
            } else {
                s.shared.push(s.here(&raw));
            }
        }
        Ok(s)
    }

    fn here(&self, p: &str) -> PathBuf {
        match &self.map {
            Some((theirs, mine)) if p == theirs || p.starts_with(&format!("{theirs}/")) => PathBuf::from(format!("{mine}{}", &p[theirs.len()..])),
            _ => PathBuf::from(p),
        }
    }

    fn theirs(&self, p: &Path) -> String {
        let p = p.display().to_string();
        match &self.map {
            Some((theirs, mine)) if p == *mine || p.starts_with(&format!("{mine}/")) => format!("{theirs}{}", &p[mine.len()..]),
            _ => p,
        }
    }

    pub fn state(&self, p: &Path) -> State {
        if self.excluded.iter().any(|e| e == p) {
            return State::LeftOut;
        }
        if self.excluded.iter().any(|e| p.starts_with(e)) {
            return State::InLeftOut;
        }
        if self.shared.iter().any(|r| p.starts_with(r)) {
            State::Shared
        } else {
            State::NotShared
        }
    }

    /// shared → left out, left out → shared again, not shared → shared. What happened, in words
    pub fn toggle(&mut self, p: &Path) -> Result<String, String> {
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        match self.state(p) {
            State::Shared => {
                self.excluded.push(p.to_path_buf());
                self.write()?;
                Ok(format!("{name}: not shared any more"))
            }
            State::LeftOut => {
                self.excluded.retain(|e| e != p);
                self.write()?;
                Ok(format!("{name}: shared again"))
            }
            State::InLeftOut => {
                let e = self.excluded.iter().find(|e| p.starts_with(e)).cloned().unwrap_or_default();
                Err(format!("{name} is in {}, which isn't shared: share that first", e.display()))
            }
            State::NotShared => {
                self.shared.push(p.to_path_buf());
                self.write()?;
                Ok(format!("{name}: shared"))
            }
        }
    }

    /// back into slskd.yml (kept once as it was, the first time: slskd.yml.before-nekozame-seek)
    fn write(&self) -> Result<(), String> {
        let text = fs::read_to_string(&self.file).map_err(|e| e.to_string())?;
        let first = PathBuf::from(format!("{}.before-nekozame-seek", self.file.display()));
        if !first.exists() {
            fs::write(&first, &text).map_err(|e| e.to_string())?;
        }
        let mut lines: Vec<String> = self.shared.iter().map(|p| self.theirs(p)).collect();
        lines.extend(self.excluded.iter().map(|p| format!("!{}", self.theirs(p))));
        let list: String = lines.iter().map(|l| format!("    - \"{}\"\n", l.replace('"', "\\\""))).collect();
        let (start, _, end) = block(&text);
        let new = match (start, end) {
            (Some(a), Some(b)) => format!("{}{}{}", &text[..a], list, &text[b..]),
            _ => format!("{}\nshares:\n  directories:\n{list}", text.trim_end()),
        };
        let tmp = PathBuf::from(format!("{}.new", self.file.display()));
        fs::write(&tmp, new).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.file).map_err(|e| e.to_string())
    }
}

/// where "shares: directories:"'s list is in the file (start and end, in bytes) and what's in it
fn block(text: &str) -> (Option<usize>, Vec<String>, Option<usize>) {
    let mut pos = 0;
    let (mut in_shares, mut start, mut items) = (false, None, vec![]);
    for line in text.split_inclusive('\n') {
        let t = line.trim_end();
        if start.is_some() {
            if let Some(item) = t.trim_start().strip_prefix("- ").filter(|_| t.starts_with("    ")) {
                items.push(item.trim().trim_matches('"').trim_matches('\'').replace("\\\"", "\""));
                pos += line.len();
                continue;
            }
            return (start, items, Some(pos));
        }
        if t == "shares:" {
            in_shares = true;
        } else if in_shares && t == "  directories:" {
            start = Some(pos + line.len());
        } else if !t.starts_with(' ') && !t.is_empty() {
            in_shares = false;
        }
        pos += line.len();
    }
    (start, items, start.map(|_| pos))
}

/// the folders in a folder, by name
pub fn folders(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !p.file_name().unwrap_or_default().to_string_lossy().starts_with('.')).collect())
        .unwrap_or_default();
    out.sort_by_key(|p| p.file_name().unwrap_or_default().to_string_lossy().to_lowercase());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_in_and_out() {
        let dir = std::env::temp_dir().join(format!("nzs-shares-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let yml = dir.join("slskd.yml");
        fs::write(&yml, "directories:\n  downloads: /downloads\nshares:\n  directories:\n    - /music\n    - \"!/music/purplekairi1312\"\nsoulseek:\n  listen_port: 50300\n").unwrap();
        let mut s = Shares::read(&yml, Some("/music=/mnt/Music")).unwrap();
        assert!(s.state(Path::new("/mnt/Music/Artist Albums")) == State::Shared);
        assert!(s.state(Path::new("/mnt/Music/purplekairi1312")) == State::LeftOut);
        assert!(s.state(Path::new("/mnt/Music/purplekairi1312/demos")) == State::InLeftOut);
        assert!(s.state(Path::new("/mnt/Other")) == State::NotShared);
        s.toggle(Path::new("/mnt/Music/DJ Sets")).unwrap(); // shared → left out
        s.toggle(Path::new("/mnt/Music/purplekairi1312")).unwrap(); // left out → shared
        let text = fs::read_to_string(&yml).unwrap();
        assert!(text.contains("    - \"/music\"\n    - \"!/music/DJ Sets\"\nsoulseek:"));
        assert!(!text.contains("purplekairi1312"));
        assert!(text.contains("listen_port: 50300"));
        assert!(dir.join("slskd.yml.before-nekozame-seek").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}

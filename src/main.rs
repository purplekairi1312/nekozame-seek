//! nekozame-seek: a TUI for Soulseek, meant for servers and high-quality collections.
//!
//! slskd stays logged in, shares and downloads all the time; this is where you search, pick
//! and download, over ssh or anywhere. Close it and everything keeps going.

mod api;
mod library;
mod rank;
mod ui;

use std::path::PathBuf;

const HELP: &str = "nekozame-seek: a TUI for Soulseek, meant for servers and high-quality collections

  nekozame-seek                 open it
  nekozame-seek \"artist album\"  open it, searching straight away

Settings: ~/.config/nekozame-seek/config.toml
  url = \"http://localhost:5030\"            slskd's address
  api_key = \"...\"                          an slskd API key with the readwrite role
  downloads = \"/path/to/slskd/downloads\"   for the library tab
  library = \"/path/to/your/music\"
";

pub struct Config {
    pub url: String,
    pub api_key: String,
    pub downloads: PathBuf,
    pub library: PathBuf,
}

fn config() -> Config {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
    let text = std::fs::read_to_string(home.join(".config/nekozame-seek/config.toml")).unwrap_or_default();
    let t: toml::Table = text.parse().unwrap_or_default();
    let s = |k: &str, d: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string();
    let mut api_key = s("api_key", "");
    if api_key.is_empty() {
        // on slskd's own machine, set up the nekozame way: its key, read where it's kept
        if let Ok(yml) = std::fs::read_to_string(home.join("slskd/app/slskd.yml")) {
            let re = regex::Regex::new(r"nekozame_seek:\s*\n\s*key:\s*(\S+)").unwrap();
            if let Some(c) = re.captures(&yml) {
                api_key = c[1].to_string();
            }
        }
    }
    Config {
        url: s("url", "http://localhost:5030"),
        api_key,
        downloads: PathBuf::from(s("downloads", "/mnt/Megalodon/Downloads/Soulseek")),
        library: PathBuf::from(s("library", "/mnt/Megalodon/Music")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return;
    }
    let first = if args.is_empty() { None } else { Some(args.join(" ")) };
    if let Err(e) = ui::run(config(), first) {
        eprintln!("nekozame-seek: {e}");
        std::process::exit(1);
    }
}

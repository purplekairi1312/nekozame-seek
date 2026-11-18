# nekozame-seek

Soulseek in the terminal: search, pick and download from a TUI, while [slskd](https://github.com/slskd/slskd) stays logged in, shares and downloads on your server all the time. Open it over ssh, close it, and everything keeps going.

- Results come as albums (one user's folder), the best first: lossless before lossy, then the highest bit depth (32 before 24 before 16), then the highest sample rate, then the biggest in MB
- The size only counts the audio files, so a folder with big scans, a booklet or a second copy in MP3 doesn't win for it
- The best album is picked: Enter downloads the whole folder (music, cover, log)
- `space` shows what's inside the album you're on: every file, with its bit depth, sample rate, length and size. Albums whose tracks aren't all the same quality say `mixed`
- Single songs on the tracks tab, ranked the same way: Enter downloads just that one
- `b`, `s` and `f` sort best first, biggest or fastest; `a`, `t` and `d` go to albums, tracks or downloads; `/` searches again; `q` quits
- In nekozame colours

## Install

It needs a running slskd, and Python 3.11 or newer.

```sh
git clone https://git.sr.ht/~purplekairi1312/nekozame-seek
nekozame-seek/install.sh
```

Without Python's venv (Debian's `python3-venv`), bring Textual's wheels from another machine (`pip download textual -d wheels`) and run `install.sh wheels`.

Then `nekozame-seek`, or `nekozame-seek "artist album"` to search straight away.

## Settings

`~/.config/nekozame-seek/config.toml`:

```toml
url = "http://localhost:5030"   # slskd's address; from another machine, its Tailscale name
api_key = "..."                 # an slskd API key with the readwrite role
```

On slskd's own machine the key is read from `~/slskd/app/slskd.yml` (an API key called `nekozame_seek`) when it isn't set here.

## License

GPL-3.0-or-later.

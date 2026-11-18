# nekozame-seek

A TUI for Soulseek, meant for servers and high-quality collections. [slskd](https://github.com/slskd/slskd) stays logged in on the server; nekozame-seek is where you search, pick and download, over ssh or anywhere.

- Albums best first: lossless, then the highest bit depth, sample rate and size (only the music counts). Enter downloads the one picked
- Tracks on their own tab, `space` shows what's inside an album, and your recent searches are kept
- The library tab moves finished downloads into your library, one at a time after you've seen where to: never over anything, and every move can be undone
- In [nekozame colours](https://git.sr.ht/~purplekairi1312/nekozame-theme)

## Install

It needs Python 3.11 or newer.

```sh
git clone https://git.sr.ht/~purplekairi1312/nekozame-seek
nekozame-seek/install.sh
```

Then, in slskd's `slskd.yml`, give it an API key (any long random text) that can search and download:

```yaml
web:
  authentication:
    api_keys:
      nekozame_seek:
        key: PUT-A-LONG-RANDOM-KEY-HERE
        role: readwrite
```

And tell nekozame-seek about it, in `~/.config/nekozame-seek/config.toml`:

```toml
url = "http://localhost:5030"   # slskd's address
api_key = "PUT-A-LONG-RANDOM-KEY-HERE"
downloads = "/path/to/slskd/downloads"   # for the library tab
library = "/path/to/your/music"
```

## License

GPL-3.0-or-later.

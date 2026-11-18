# Putting downloads in your library

nekozame-seek hands finished downloads to [beets](https://beets.io), which puts them in your music library properly. Searching and downloading work without beets; putting downloads away needs it.

## Why beets

A download only says so much about itself: its folder name and its tags, which are often messy ("1993- Slowdive - Souvlaki [Vinyl 24-bit 96khz FLAC]", missing or wrong tags). beets looks each one up on [MusicBrainz](https://musicbrainz.org), the open music database, and finds the exact release, even which edition (a 2006 vinyl remaster, a 2-disc deluxe). Then:

- the album gets its real name, and goes where your library's layout says (beets' `paths`)
- the files' tags are fixed (artist, album, titles, track numbers, year), so Jellyfin, Navidrome and players show them right
- an album you already have is recognised, and left for you to decide
- scans, logs and cue sheets go along with the album (with the filetote plugin)

beets is free software (MIT), around since 2010, and widely used for exactly this.

## Install beets

From your system's packages (`beets` in most of them), with pip (`pip install beets beets-filetote`), or in a container: `contrib/beets/` has one (beets with filetote) and a `beet` command that runs it.

nekozame-seek uses beets by itself when `beet` is installed. Another command, or none:

```toml
# ~/.config/nekozame-seek/config.toml, on the machine with the music
downloads = "/path/to/slskd/downloads"
beets = "beet"   # "" to never use it
```

## Set up beets

`~/.config/beets/config.yaml` (with the container: `~/beets/config.yaml`). A start, to change to your library's layout:

```yaml
directory: /path/to/your/music
plugins: musicbrainz filetote
import:
  move: yes                # out of the downloads, into the library
  write: yes               # tags fixed in the files
  quiet_fallback: skip     # not sure: left in the downloads, for you
  duplicate_action: skip   # you have it already: left too
paths:
  default: $albumartist/$album%aunique{}/%if{$multidisc,$disc-}$track - $title
  singleton: $artist/$title
  comp: Various Artists/$album%aunique{}/%if{$multidisc,$disc-}$track - $title
  albumtype:soundtrack: Soundtracks/$album%aunique{}/%if{$multidisc,$disc-}$track - $title
filetote:
  extensions: .jpg .jpeg .png .pdf .log .cue .txt
```

`paths` is where things go: `$albumartist`, `$album`, `$year`, `$track`, `$title` and many more ([beets' paths](https://beets.readthedocs.io/en/stable/reference/pathformat.html)). For example, albums under an "Artist Albums" folder: `default: Artist Albums/$albumartist/$album%aunique{}/$track - $title`.

So beets knows what you already have, let it read your library once, with `contrib/beets/read-only.yaml`, which makes sure nothing is moved, copied or changed: `beet -c read-only.yaml import -q /path/to/your/music`. (beets' own `-A -C -W` flags aren't enough: with `move: yes` in its config, it still moves files.)

## Putting them away

**By themselves:** `nekozame-seek organize` hands every finished download to beets in its quiet mode. What beets is sure of goes into your library; the rest stays in the downloads, for you. Soundtracks always wait for you, because where they go is often your call. Run it every few minutes with the systemd timer in `contrib/`:

```sh
cp contrib/nekozame-seek-organize.* ~/.config/systemd/user/
systemctl --user enable --now nekozame-seek-organize.timer
```

Or from cron: `*/10 * * * * nekozame-seek organize`.

**By hand:** the library tab (`l`) lists finished downloads with their quality. Enter opens beets for one, right there: it shows its match and asks you (apply it, pick another, search again, skip...), then nekozame-seek comes back. `x` leaves one alone.

## Safe

- Only finished downloads: anything still downloading, failed, or changed in the last minute is left alone
- Not sure, or already in your library: beets leaves it where it is
- beets keeps a log of what it moved; its own `beet` commands list, move back or remove what's in its library

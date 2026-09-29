# Crate Rat

A terminal crate manager for DJs who keep the same playlists mirrored across multiple drives.

## Status

A runnable ratatui app. A "crate" is a named set of local folder paths meant to hold the same
content; each subfolder found under a crate's paths is treated as a playlist, and the files inside
it are its tracks. Playlists can carry tags and be linked to a streaming service, and you can play
local tracks (with cover art and metadata) right from the terminal.

- **Local playlists**: folders become playlists automatically; paths within a crate are treated as
  mirrors of each other (same playlists, same tracks).
- **Playback**: play/pause tracks in place, with cover art (rendered as halfblocks — works in any
  terminal) and tags (artist/album/title/genre/year/duration) read via `lofty`.
- **Spotify import**: paste a playlist link, log in once (OAuth + PKCE, browser-based), and pull
  every track's title/artist/album/duration into a local manifest (Spotify's API has no
  downloadable stream, so this is metadata-only — see below for turning it into real audio).
  Requires your own Spotify Developer app (Client ID, set from Settings).
- **SoundCloud import**: paste a link to a *public* playlist — no login or app registration
  needed. Downloads the actual audio (progressive streams only; Go+-exclusive tracks are skipped).
- **Tidal catalog search**: check which tracks in a playlist exist on Tidal (app-only, no user
  login — just a Client ID/Secret from Settings).
- **Tidal import + download**: paste a Tidal playlist link to link it (looks up just the name via
  the app-only Client ID/Secret); pressing `D` then downloads the whole playlist in one go via the
  externally-installed [`tidal-dl-ng`](https://github.com/exislow/tidal-dl-ng) `dl <link>` (needs
  its own real Tidal login/subscription — Crate Rat just calls it).
- **Spotify → Tidal → download**: for a Spotify-imported (metadata-only) playlist, find each
  track on Tidal and download it via `tidal-dl-ng` track by track instead.
- **One library, no duplicated metadata**: every track found in any playlist (local file or
  imported manifest entry) is stored once in `library.json`, keyed by artist + title — the same
  song sitting in five playlist folders is one entry with five file copies. Playlists only keep
  the list of track ids they contain. Tags are read once per file and cached by size/mtime.
- **Genre tags from the internet**: every library track gets its genre/style tags looked up in
  the background (MusicBrainz by default, no key needed; Last.fm if you set an API key), and each
  playlist gets `#tags` derived from the tracks it holds.
- **Sync status everywhere**: the header shows what's running (tag lookups, Tidal checks,
  imports, downloads, cloud backup); the dashboard's SYNC column shows it per playlist, and each
  track shows its tag state (`⟳` looking up, `…` queued, `✕` failed).
- **Library explorer** (`b` or `/`): browse every track by artist or by title, search by artist,
  title, album or tag, and see all of a track's metadata and every playlist it's in.
- **Cloud backup**: point Settings at a cloud-synced folder (iCloud Drive, Dropbox, Google
  Drive…) and a full snapshot of the library + every playlist's tracklist is written there
  (`crate-rat-backup.json`, no credentials). A new machine with an empty library restores from it
  automatically; `s` → `B` merges it in manually.
- Playlists that only have imported metadata (no downloaded audio) are shown with a ☁ marker and
  can't be played — only ones with real local files can.

## Run

```sh
cargo run
```

### Dashboard

- `j`/`k` or arrow keys — move between playlists
- `Tab` — change crate
- `Enter` — open the selected playlist (browse/play its tracks)
- `r` — rescan playlists from disk (also refreshes crate/drive availability)
- `t` — browse tags, `Enter` on one to see which playlists have it (and open one from there)
- `T` — edit tags (comma separated) on the selected playlist
- `b` — explore the library (`/` opens it straight in search)
- `c` — manage crates and their paths
- `n` — new crate
- `i` — import: link a playlist to Spotify/SoundCloud/Tidal, or create a new one
- `s` — settings (Spotify + Tidal credentials, connect/disconnect, overview)
- `q` — quit

### Crate paths (`c`)

Every row (crate name, then each path) is listed and editable directly:

- `↑`/`↓` or `j`/`k` — select a row, `Enter` to edit it, `Enter` again to save
- `←`/`→` or `h`/`l` — switch crate
- `a` — add a path · `x` — remove the selected path
- `e` — mark/unmark the selected path as a removable/external drive (shown differently when
  disconnected instead of as an error)
- `n` — new crate · `X` — delete the selected crate (asks for confirmation)

Saving a path rescans that crate's folders and updates its playlists.

### Playing a playlist (`Enter` on one)

- `j`/`k` — scroll tracks (shows cover art + tags for the highlighted one)
- `Enter` / `p` — play/pause the selected track · `x` — stop · `Esc` — back
- `D` — download real audio for this playlist: pulls from SoundCloud directly if it's
  SoundCloud-linked, finds + downloads via Tidal (`tidal-dl-ng`) track by track if it's
  Spotify-linked, or downloads the whole playlist in one `tidal-dl-ng dl <link>` call if it's
  Tidal-linked (can take several minutes for a big playlist). Runs in the background with a
  status message; `Esc` cancels.

The track details panel shows the track's library tags, whether they're synced, and **every
playlist the track is in** (`●` the current one). `g` re-fetches the selected track's tags. A
track that's metadata-only here but downloaded in another playlist plays from that copy. When a
track ends, playback moves on to the next downloaded track.

Playback decodes MP3, FLAC, WAV, OGG/Vorbis and AAC/ALAC `.m4a` (what `tidal-dl-ng` saves).
Anything the audio backend prints goes to `crate-rat.log` next to the config instead of over the
UI.

### Library (`b`)

- `/` — search (artist, title, album, tag — all words must match), `Enter` done, `Esc` clear
- `Tab` — group by artist ↔ list by title · `J`/`K` (or `→`/`←`) — next/previous artist
- `Enter` / `p` — play/pause · `x` — stop · `g` — re-fetch tags
- `o` — open the (first) playlist containing the track · `Esc` — back

### Tags and backup (Settings, `s`)

- **Last.fm API Key** (optional) — get one at [last.fm/api](https://www.last.fm/api/account/create);
  much denser genre tags than MusicBrainz. Without it MusicBrainz is used (1 request/second, so
  a big first-time library takes a while — it runs in the background and resumes next launch).
- **Cloud backup folder** — any folder your cloud client syncs. `b` writes a backup now, `B`
  restores tags/metadata from it. Backups are otherwise written on save, at most once a minute,
  and always on quit.

### Import (`i`)

Pick a service → new playlist or link to an existing one → pick a crate (and playlist, if
linking) → paste a link (Spotify/SoundCloud/Tidal) or type a name (manual). Fetches run in the
background so the UI never freezes; `Esc` cancels one in progress.

### Spotify setup (one-time)

1. Create an app at [developer.spotify.com/dashboard](https://developer.spotify.com/dashboard),
   add redirect URI `http://127.0.0.1:8888/callback`, enable the Web API.
2. Copy the **Client ID** (no secret needed — Crate Rat uses PKCE) into `s` → select the field →
   `Enter`/`e` in Crate Rat.
3. `s` → `l` to log in (opens your browser). `L` disconnects.
4. Note: reading a private playlist requires the account that *owns the Spotify app* to have an
   active Premium subscription and to be added under the app's User Management if it's still in
   Development Mode — this is a Spotify-side restriction, not a Crate Rat one.

### Tidal setup (one-time)

1. Create an app at [developer.tidal.com](https://developer.tidal.com) and copy its **Client ID**
   and **Client Secret**.
2. Set both in `s` (Settings) — used for catalog search only (app-only Client Credentials flow,
   no user login).
3. To actually download audio (`D` on a Tidal- or Spotify-linked playlist), separately install and
   log into [`tidal-dl-ng`](https://github.com/exislow/tidal-dl-ng) (`pip install tidal-dl-ng`) —
   that needs your own Tidal subscription and its own login, unrelated to the Client ID/Secret
   above.

## Direction

- Check playlist: which songs are missing / which don't exist on the service
- Search by playlist name

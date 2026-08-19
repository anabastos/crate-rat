# ratcrate

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
  every track's title/artist/album/duration into a local manifest. Requires your own Spotify
  Developer app (Client ID, set from Settings) — see below.
- **SoundCloud import**: paste a link to a *public* playlist — no login or app registration
  needed (uses the same public-client-id approach as most open-source SoundCloud tools).
- **Tidal import**: not implemented yet (wizard exists, fetch doesn't).
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
- `c` — manage crates and their paths
- `n` — new crate
- `i` — import: link a playlist to Spotify/SoundCloud/Tidal, or create a new one
- `s` — settings (Spotify Client ID, connect/disconnect, overview)
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

### Import (`i`)

Pick a service → new playlist or link to an existing one → pick a crate (and playlist, if
linking) → paste a link (Spotify/SoundCloud) or type a name (manual). Spotify/SoundCloud fetches
run in the background so the UI never freezes; `Esc` cancels a fetch in progress.

### Spotify setup (one-time)

1. Create an app at [developer.spotify.com/dashboard](https://developer.spotify.com/dashboard),
   add redirect URI `http://127.0.0.1:8888/callback`, enable the Web API.
2. Copy the **Client ID** (no secret needed — RatCrate uses PKCE) into `s` → `e` in RatCrate.
3. `s` → `l` to log in (opens your browser). `L` disconnects.
4. Note: reading a private playlist requires the account that *owns the Spotify app* to have an
   active Premium subscription and to be added under the app's User Management if it's still in
   Development Mode — this is a Spotify-side restriction, not a RatCrate one.

## Direction

- Tidal import (auth + API calls)
- Actually downloading tracks, not just metadata
- Check playlist: which songs are missing / which don't exist on the service
- Search by tags, artists, track name, playlist name

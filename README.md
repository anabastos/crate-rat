# ratcrate

A terminal crate manager for DJs who keep the same playlists mirrored across multiple drives.

## Status

A runnable ratatui app. A "crate" is a named set of local folder paths that are meant to hold the
same content; each subfolder found under a crate's paths is treated as a playlist, and the files
inside it are its tracks. Tags and service links (Tidal/Spotify/SoundCloud) can be attached to
playlists. Actual API integration with those services is not implemented yet — Import only records
the link locally.

## Run

```sh
cargo run
```

### Dashboard

- `j`/`k` or arrow keys — move between playlists
- `Tab` — change crate
- `r` — rescan playlists from disk (also refreshes crate availability)
- `t` — browse tags, `Enter` on one to see which playlists have it
- `T` — edit tags (comma separated) on the selected playlist
- `c` — manage crate paths
- `i` — import: link a playlist to a Tidal/Spotify/SoundCloud playlist, or create a new one
- `s` — settings
- `q` — quit

### Crate paths (`c`)

Every row (crate name, then each path) is listed and editable directly:

- `↑`/`↓` or `j`/`k` — select a row, `Enter` to edit it, `Enter` again to save
- `←`/`→` or `h`/`l` — switch crate
- `a` — add a path to the selected crate
- `x` — remove the selected path
- `n` — add a new crate
- `X` — delete the selected crate (asks for confirmation)

Saving a path rescans that crate's folders and updates its playlists.

## Direction

- can import a spotify/tidal/soundcloud playlist for real (auth + API calls)
- check playlist: which songs are missing / which don't exist on the service
- search by tags, artists, track name, playlist name
- should work on Windows and Linux

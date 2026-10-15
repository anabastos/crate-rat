use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::model::{CrateLocation, Playlist};

#[derive(Clone)]
pub struct TrackFile {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub modified: Option<SystemTime>,
    pub remote_metadata: Option<RemoteTrackMetadata>,
}

/// Metadata pulled from an imported service manifest (`*-tracks.json`) rather than read from
/// a local audio file's tags — used for playlists imported without downloading actual audio.
#[derive(Clone)]
pub struct RemoteTrackMetadata {
    pub artist: String,
    pub album: String,
    pub duration_secs: u64,
    /// The track's id on the source service (currently only populated for Tidal), so a missing
    /// track can be downloaded directly by id instead of having to be searched for first.
    pub external_id: Option<String>,
}

/// Scans every path belonging to `crate_location` and builds playlists from the subfolders found:
/// each subfolder becomes a playlist, and the files directly inside it become its tracks.
/// Playlists (matched case-insensitively) and files within a playlist are deduplicated, so adding
/// another path that mirrors an existing one doesn't create duplicate entries.
pub fn scan_crate_playlists(crate_location: &CrateLocation) -> Vec<Playlist> {
    // key: lowercased playlist name -> (display name, best known track count, best known downloaded count, locations that have it)
    let mut found: BTreeMap<String, (String, usize, usize, usize)> = BTreeMap::new();

    for location in &crate_location.locations {
        let Ok(entries) = fs::read_dir(&location.path) else {
            continue;
        };
        let mut seen_here: HashSet<String> = HashSet::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().trim().to_string();
            if name.is_empty() {
                continue;
            }
            let key = name.to_lowercase();
            if !seen_here.insert(key.clone()) {
                continue;
            }
            let track_count = track_count_for(&path);
            let downloaded_count = count_unique_files(&path);
            let slot = found.entry(key).or_insert_with(|| (name.clone(), 0, 0, 0));
            slot.1 = slot.1.max(track_count);
            slot.2 = slot.2.max(downloaded_count);
            slot.3 += 1;
        }
    }

    let location_count = crate_location.locations.len().max(1);
    found
        .into_values()
        .map(|(name, track_count, downloaded_count, seen_in)| {
            let synced = if seen_in >= location_count { downloaded_count.min(track_count) } else { 0 };
            Playlist { name, track_count, synced, tags: Vec::new(), link: None, track_ids: Vec::new(), auto_tags: Vec::new() }
        })
        .collect()
}

/// A manifest file left by an online import (e.g. `spotify-tracks.json`) that lists tracks
/// pulled from a streaming service without downloading actual audio.
fn find_manifest(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    entries.flatten().map(|entry| entry.path()).find(|path| path.is_file() && path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with("-tracks.json")))
}

fn track_count_for(dir: &Path) -> usize {
    if let Some(manifest) = find_manifest(dir) {
        if let Some(count) = read_manifest(&manifest).map(|tracks| tracks.len()) {
            return count;
        }
    }
    count_unique_files(dir)
}

fn count_unique_files(dir: &Path) -> usize {
    playlist_files(dir).iter().map(|file| file.strip_prefix(dir).unwrap_or(file).to_string_lossy().to_lowercase()).collect::<HashSet<_>>().len()
}

/// Audio formats picked up from a playlist's subfolders.
const AUDIO_EXTENSIONS: [&str; 11] = ["mp3", "flac", "wav", "aif", "aiff", "ogg", "opus", "m4a", "aac", "alac", "wma"];

/// How deep to look inside a playlist folder (`playlist/artist/album/disc/...`).
const MAX_DEPTH: usize = 6;

/// Every track file in a playlist folder: the files directly inside it (minus import
/// manifests), plus audio files in any subfolder, so an album crate laid out as
/// `artist/album/track.mp3` still counts and lists its tracks. Subfolders only contribute audio
/// files, so cover art and rip logs kept next to an album don't show up as tracks.
fn playlist_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < MAX_DEPTH {
                    walk(&path, depth + 1, out);
                }
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let keep = if depth == 0 {
                !name.ends_with("-tracks.json")
            } else {
                path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| AUDIO_EXTENSIONS.contains(&extension.to_lowercase().as_str()))
            };
            if keep {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, 0, &mut files);
    files
}

/// Lists the tracks inside `playlist_name`'s folder, taken from the first path in
/// `crate_location` that has it (matched case-insensitively). If the folder has no real audio
/// files but does have an import manifest (`*-tracks.json`), the tracks are built from that
/// instead — this is how playlists imported from Spotify/SoundCloud (metadata only, no
/// downloaded audio) show their track list.
pub fn list_playlist_tracks(crate_location: &CrateLocation, playlist_name: &str) -> Vec<TrackFile> {
    let key = playlist_name.to_lowercase();
    for location in &crate_location.locations {
        let Ok(entries) = fs::read_dir(&location.path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || entry.file_name().to_string_lossy().to_lowercase() != key {
                continue;
            }
            let mut tracks: Vec<TrackFile> = playlist_files(&path)
                .into_iter()
                .map(|file| {
                    let metadata = fs::metadata(&file).ok();
                    TrackFile {
                        name: file.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default(),
                        path: file,
                        size_bytes: metadata.as_ref().map_or(0, fs::Metadata::len),
                        modified: metadata.and_then(|metadata| metadata.modified().ok()),
                        remote_metadata: None,
                    }
                })
                .collect();

            // Add back manifest entries for tracks that haven't been downloaded yet (matched by
            // whether any local filename contains the manifest title), so a playlist that's
            // partially downloaded still shows what's missing instead of losing the rest of the
            // manifest the moment the first real file lands.
            if let Some(manifest) = find_manifest(&path) {
                if let Some(manifest_tracks) = read_manifest(&manifest) {
                    let local_names: Vec<String> = tracks.iter().map(|track| normalize_for_match(&track.name)).collect();
                    let missing = manifest_tracks.into_iter().filter(|manifest_track| {
                        let title = normalize_for_match(&manifest_track.name);
                        !local_names.iter().any(|local_name| local_name.contains(&title))
                    });
                    tracks.extend(missing);
                }
            }

            // Path order keeps an album's tracks together and in disc/track order.
            tracks.sort_unstable_by(|a, b| a.path.cmp(&b.path));
            return tracks;
        }
    }
    Vec::new()
}

/// Loosened comparison for matching a manifest title against a local filename: lowercased, with
/// punctuation stripped and whitespace collapsed, since downloaders routinely rename tracks just
/// enough (dropping quotes/colons, tweaking "Remaster" formatting, etc.) that a literal substring
/// match on the raw title fails even though it's clearly the same track.
pub fn normalize_for_match(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = false;
    for character in text.to_lowercase().chars() {
        if character.is_alphanumeric() {
            out.push(character);
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    out.trim().to_string()
}

/// Removes the entry matching `title` (exact, case-sensitive — titles come from the same
/// manifest this reads/writes, so no fuzzy matching is needed) from `dir`'s import manifest, if
/// one exists. Called right after a track is confirmed downloaded, so the manifest stops
/// claiming a track is "not downloaded yet" the moment a real file exists for it — more reliable
/// than re-deriving that from filename matching on every scan.
pub fn remove_manifest_entry(dir: &Path, title: &str) -> std::io::Result<()> {
    let Some(manifest) = find_manifest(dir) else { return Ok(()) };
    let contents = fs::read_to_string(&manifest)?;
    let Ok(mut entries) = serde_json::from_str::<Vec<serde_json::Value>>(&contents) else { return Ok(()) };
    entries.retain(|entry| entry["title"].as_str() != Some(title));
    let contents = serde_json::to_string_pretty(&entries)?;
    fs::write(&manifest, contents)
}

fn read_manifest(manifest: &Path) -> Option<Vec<TrackFile>> {
    let contents = fs::read_to_string(manifest).ok()?;
    let entries: Vec<serde_json::Value> = serde_json::from_str(&contents).ok()?;
    Some(
        entries
            .into_iter()
            .map(|entry| TrackFile {
                name: entry["title"].as_str().unwrap_or("Unknown track").to_string(),
                path: manifest.to_path_buf(),
                size_bytes: 0,
                modified: None,
                remote_metadata: Some(RemoteTrackMetadata {
                    artist: entry["artist"].as_str().unwrap_or("").to_string(),
                    album: entry["album"].as_str().unwrap_or("").to_string(),
                    duration_secs: entry["duration_secs"].as_u64().unwrap_or(0),
                    external_id: entry["id"].as_str().map(str::to_string),
                }),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Location;

    #[test]
    fn playlists_include_audio_in_subfolders() {
        let root = std::env::temp_dir().join(format!("crate-rat-sync-test-{}", std::process::id()));
        let album = root.join("Albuns").join("Artist").join("Album");
        fs::create_dir_all(&album).unwrap();
        fs::write(root.join("Albuns").join("loose.mp3"), b"").unwrap();
        fs::write(album.join("01 - One.flac"), b"").unwrap();
        fs::write(album.join("02 - Two.mp3"), b"").unwrap();
        fs::write(album.join("cover.jpg"), b"").unwrap();
        let crate_location = CrateLocation { name: "c".into(), locations: vec![Location { path: root.to_string_lossy().to_string(), removable: false }], available: true, playlists: vec![], rekordbox: false };
        let playlists = scan_crate_playlists(&crate_location);
        let tracks = list_playlist_tracks(&crate_location, "Albuns");
        let _ = fs::remove_dir_all(&root);
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].track_count, 3);
        let names: Vec<&str> = tracks.iter().map(|track| track.name.as_str()).collect();
        assert_eq!(names, ["01 - One.flac", "02 - Two.mp3", "loose.mp3"]);
    }
}

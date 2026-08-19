use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::model::{CrateLocation, Playlist};

pub struct TrackFile {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub modified: Option<SystemTime>,
}

/// Scans every path belonging to `crate_location` and builds playlists from the subfolders found:
/// each subfolder becomes a playlist, and the files directly inside it become its tracks.
/// Playlists (matched case-insensitively) and files within a playlist are deduplicated, so adding
/// another path that mirrors an existing one doesn't create duplicate entries.
pub fn scan_crate_playlists(crate_location: &CrateLocation) -> Vec<Playlist> {
    // key: lowercased playlist name -> (display name, best known track count, locations that have it)
    let mut found: BTreeMap<String, (String, usize, usize)> = BTreeMap::new();

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
            let track_count = count_unique_files(&path);
            let slot = found.entry(key).or_insert_with(|| (name.clone(), 0, 0));
            slot.1 = slot.1.max(track_count);
            slot.2 += 1;
        }
    }

    let location_count = crate_location.locations.len().max(1);
    found
        .into_values()
        .map(|(name, track_count, seen_in)| {
            let synced = if seen_in >= location_count { track_count } else { 0 };
            Playlist { name, track_count, synced, tags: Vec::new(), link: None }
        })
        .collect()
}

fn count_unique_files(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().to_lowercase())
        .collect::<HashSet<_>>()
        .len()
}

/// Lists the tracks (with file metadata) inside `playlist_name`'s folder, taken from
/// the first path in `crate_location` that has it (matched case-insensitively).
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
            let Ok(files) = fs::read_dir(&path) else {
                return Vec::new();
            };
            let mut tracks: Vec<TrackFile> = files
                .flatten()
                .filter(|file| file.path().is_file())
                .map(|file| {
                    let metadata = file.metadata().ok();
                    TrackFile {
                        name: file.file_name().to_string_lossy().to_string(),
                        path: file.path(),
                        size_bytes: metadata.as_ref().map_or(0, fs::Metadata::len),
                        modified: metadata.and_then(|metadata| metadata.modified().ok()),
                    }
                })
                .collect();
            tracks.sort_unstable_by(|a, b| a.name.cmp(&b.name));
            return tracks;
        }
    }
    Vec::new()
}

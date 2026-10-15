//! Rescanning every crate on disk, off the UI thread. Startup shows the crates and playlists
//! saved last time straight away; this scan then catches up with what changed on disk (new
//! folders and files, tags of new or edited files, rekordbox edits, files that are gone) and
//! the app applies it when it's done.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    library::FileEntry,
    metadata::{self, TrackMetadata},
    model::{CrateLocation, Playlist},
    rekordbox, sync,
    sync::TrackFile,
};

pub enum CrateScan {
    /// Playlists found in the crate's folders, plus each one's track listing (keyed by
    /// lowercased playlist name).
    Folders { playlists: Vec<Playlist>, listings: HashMap<String, Vec<TrackFile>> },
    Rekordbox { stamp: Option<SystemTime>, playlists: Vec<rekordbox::RekordboxPlaylist> },
    /// rekordbox's library hasn't changed since it was last read.
    RekordboxUnchanged,
    Failed(String),
}

pub struct ScanResult {
    /// (crate name, whether all its paths exist, what was found).
    pub crates: Vec<(String, bool, CrateScan)>,
    /// Library file paths that no longer exist.
    pub missing_files: Vec<String>,
    /// Tags of files that are new or changed since they were last read (cover art dropped).
    pub tags: HashMap<String, Option<TrackMetadata>>,
}

/// When rekordbox last wrote its library (master.db or its WAL).
pub fn rekordbox_stamp(db_path: &Path) -> Option<SystemTime> {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok();
    modified(db_path).max(modified(&PathBuf::from(format!("{}-wal", db_path.display()))))
}

/// Starts a scan of `crates` on its own thread. `known_files` is the library's file cache, so
/// only files that are new or changed get their tags read; `rekordbox_cached` is the stamp of
/// the rekordbox library already in memory, if any.
pub fn start(crates: Vec<CrateLocation>, known_files: BTreeMap<String, FileEntry>, rekordbox_cached: Option<Option<SystemTime>>) -> Receiver<ScanResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(scan(&crates, &known_files, rekordbox_cached));
    });
    rx
}

fn scan(crates: &[CrateLocation], known_files: &BTreeMap<String, FileEntry>, rekordbox_cached: Option<Option<SystemTime>>) -> ScanResult {
    let missing_files = known_files.keys().filter(|path| !Path::new(path).exists()).cloned().collect();
    let mut tags = HashMap::new();
    let crates = crates
        .iter()
        .map(|crate_location| {
            let available = crate_location.locations.iter().all(|location| Path::new(&location.path).exists());
            let found = if crate_location.rekordbox {
                scan_rekordbox(crate_location, rekordbox_cached)
            } else {
                scan_folders(crate_location, known_files, &mut tags)
            };
            (crate_location.name.clone(), available, found)
        })
        .collect();
    ScanResult { crates, missing_files, tags }
}

fn scan_rekordbox(crate_location: &CrateLocation, cached: Option<Option<SystemTime>>) -> CrateScan {
    let Some(location) = crate_location.locations.first() else { return CrateScan::Failed("no master.db path".into()) };
    let db_path = PathBuf::from(&location.path);
    let stamp = rekordbox_stamp(&db_path);
    if cached == Some(stamp) {
        return CrateScan::RekordboxUnchanged;
    }
    match rekordbox::load_playlists(&db_path) {
        Ok(playlists) => CrateScan::Rekordbox { stamp, playlists },
        Err(error) => CrateScan::Failed(format!("Could not read rekordbox's library ({}): {error}", db_path.display())),
    }
}

fn scan_folders(crate_location: &CrateLocation, known_files: &BTreeMap<String, FileEntry>, tags: &mut HashMap<String, Option<TrackMetadata>>) -> CrateScan {
    let playlists = sync::scan_crate_playlists(crate_location);
    let mut listings = HashMap::new();
    for playlist in &playlists {
        let tracks = sync::list_playlist_tracks(crate_location, &playlist.name);
        for track in tracks.iter().filter(|track| track.remote_metadata.is_none()) {
            let key = track.path.to_string_lossy().to_string();
            let modified = track.modified.and_then(|modified| modified.duration_since(UNIX_EPOCH).ok()).map_or(0, |elapsed| elapsed.as_secs());
            let unchanged = known_files.get(&key).is_some_and(|entry| entry.size_bytes == track.size_bytes && entry.modified_secs == modified);
            if !unchanged && !tags.contains_key(&key) {
                let read = metadata::read_track_metadata(&track.path).map(|mut found| {
                    found.cover = None;
                    found
                });
                tags.insert(key, read);
            }
        }
        listings.insert(playlist.name.to_lowercase(), tracks);
    }
    CrateScan::Folders { playlists, listings }
}

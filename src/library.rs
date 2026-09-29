//! The one place track metadata lives. Every playlist (in every crate, on every path) only
//! references tracks by id; the same song sitting in five playlist folders is one library entry
//! with five file copies and five playlist memberships, not five copies of its metadata.

use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::metadata;
use crate::model::{CrateLocation, PlaylistLink};
use crate::sync::{normalize_for_match, TrackFile};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub tracks: BTreeMap<String, LibraryTrack>,
    /// Local audio file path -> the track it holds. Doubles as a cache so rescans only re-read
    /// the tags of files whose size or modification time changed.
    #[serde(default)]
    pub files: BTreeMap<String, FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub track_id: String,
    pub size_bytes: u64,
    pub modified_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibraryTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub year: Option<u32>,
    #[serde(default)]
    pub duration_secs: u64,
    /// Genres read from the audio file's own tags.
    #[serde(default)]
    pub genres: Vec<String>,
    /// Genre/style tags looked up online (MusicBrainz or Last.fm).
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub tag_status: TagStatus,
    #[serde(default)]
    pub tags_source: Option<String>,
    #[serde(default)]
    pub tags_updated_secs: Option<u64>,
    /// Service name (lowercase, e.g. "tidal") -> the track's id there.
    #[serde(default)]
    pub external_ids: BTreeMap<String, String>,
    #[serde(default)]
    pub added_secs: u64,
}

impl LibraryTrack {
    /// File genres + online tags, deduplicated, file genres first.
    pub fn all_tags(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for tag in self.genres.iter().chain(self.tags.iter()) {
            let tag = tag.trim().to_lowercase();
            if !tag.is_empty() && !out.contains(&tag) {
                out.push(tag);
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagStatus {
    /// Not looked up online yet.
    #[default]
    Pending,
    Done,
    /// Looked up, but no source had tags for it.
    NotFound,
}

/// What we know about a track from wherever we saw it (a file's tags, an import manifest).
#[derive(Debug, Default, Clone)]
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<u32>,
    pub duration_secs: u64,
    pub genre: Option<String>,
    pub external_id: Option<(String, String)>,
}

/// Library id for a track: normalized artist + title, so the same song found under slightly
/// different file names/punctuation still lands on one entry.
pub fn track_key(artist: &str, title: &str) -> String {
    format!("{}|{}", normalize_for_match(artist), normalize_for_match(title))
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

/// Best-effort artist/title from a file name like `01 - Artist - Title.flac` or `Title.mp3`.
pub fn info_from_filename(file_name: &str) -> (String, String) {
    let stem = Path::new(file_name).file_stem().and_then(|stem| stem.to_str()).unwrap_or(file_name).trim();
    // Drop a leading track number ("01. ", "01 - ", "01_") — but not a name that merely starts
    // with digits ("808 State - Pacific").
    let without_number = {
        let digits = stem.chars().take_while(char::is_ascii_digit).count();
        let rest = &stem[digits..];
        let numbered = digits > 0 && digits <= 3 && (rest.starts_with(['.', '-', '_']) || rest.starts_with(" - ") || rest.starts_with(" . "));
        let trimmed = rest.trim_start_matches(['.', '-', '_', ' ']);
        if numbered && !trimmed.is_empty() { trimmed } else { stem }
    };
    match without_number.split_once(" - ") {
        Some((artist, title)) => (artist.trim().to_string(), title.trim().to_string()),
        None => (String::new(), without_number.to_string()),
    }
}

impl Library {
    /// Adds a sighting of a track, merging it into the existing entry if one matches, and
    /// returns its id. Tracks seen without an artist attach to a same-titled entry that has one
    /// (and get promoted when the artist later turns up), so a manifest entry and the file
    /// downloaded for it end up as the same track.
    pub fn upsert(&mut self, mut info: TrackInfo) -> String {
        if normalize_for_match(&info.title).is_empty() {
            info.title = "Unknown track".into();
        }
        let title_key = normalize_for_match(&info.title);
        let mut id = track_key(&info.artist, &info.title);

        if info.artist.trim().is_empty() {
            let suffix = format!("|{title_key}");
            let mut candidates = self.tracks.keys().filter(|key| key.ends_with(&suffix) && !key.starts_with('|'));
            if let (Some(only), None) = (candidates.next(), candidates.next()) {
                id = only.clone();
            }
        } else {
            let artistless = format!("|{title_key}");
            if id != artistless && self.tracks.contains_key(&artistless) {
                self.rename(&artistless, &id);
            }
        }

        let entry = self.tracks.entry(id.clone()).or_insert_with(|| LibraryTrack {
            id: id.clone(),
            title: info.title.trim().to_string(),
            artist: info.artist.trim().to_string(),
            added_secs: now_secs(),
            ..LibraryTrack::default()
        });
        if entry.artist.is_empty() && !info.artist.trim().is_empty() {
            entry.artist = info.artist.trim().to_string();
        }
        if entry.album.is_empty() && !info.album.trim().is_empty() {
            entry.album = info.album.trim().to_string();
        }
        if entry.year.is_none() {
            entry.year = info.year;
        }
        if entry.duration_secs == 0 {
            entry.duration_secs = info.duration_secs;
        }
        if let Some(genre) = info.genre {
            for genre in genre.split([';', ',', '/']).map(|genre| genre.trim().to_lowercase()).filter(|genre| !genre.is_empty()) {
                if !entry.genres.contains(&genre) {
                    entry.genres.push(genre);
                }
            }
        }
        if let Some((service, external_id)) = info.external_id {
            entry.external_ids.insert(service, external_id);
        }
        id
    }

    /// Moves an entry to a new id, merging into the target if it already exists.
    fn rename(&mut self, from: &str, to: &str) {
        let Some(mut old) = self.tracks.remove(from) else { return };
        match self.tracks.get_mut(to) {
            Some(target) => {
                if target.album.is_empty() {
                    target.album = std::mem::take(&mut old.album);
                }
                target.year = target.year.or(old.year);
                if target.duration_secs == 0 {
                    target.duration_secs = old.duration_secs;
                }
                for genre in old.genres {
                    if !target.genres.contains(&genre) {
                        target.genres.push(genre);
                    }
                }
                if target.tags.is_empty() && !old.tags.is_empty() {
                    target.tags = old.tags;
                    target.tag_status = old.tag_status;
                    target.tags_source = old.tags_source;
                    target.tags_updated_secs = old.tags_updated_secs;
                }
                for (service, external_id) in old.external_ids {
                    target.external_ids.entry(service).or_insert(external_id);
                }
            }
            None => {
                old.id = to.to_string();
                self.tracks.insert(to.to_string(), old);
            }
        }
        for file in self.files.values_mut() {
            if file.track_id == from {
                file.track_id = to.to_string();
            }
        }
    }

    /// Library id for a local audio file, reading its tags only if it's new or changed.
    pub fn resolve_file(&mut self, track: &TrackFile) -> String {
        let key = track.path.to_string_lossy().to_string();
        let modified_secs = track.modified.and_then(|modified| modified.duration_since(UNIX_EPOCH).ok()).map_or(0, |elapsed| elapsed.as_secs());
        if let Some(entry) = self.files.get(&key) {
            if entry.size_bytes == track.size_bytes && entry.modified_secs == modified_secs && self.tracks.contains_key(&entry.track_id) {
                return entry.track_id.clone();
            }
        }

        let (file_artist, file_title) = info_from_filename(&track.name);
        let tags = metadata::read_track_metadata(&track.path);
        let pick = |tag: Option<String>, fallback: &str| tag.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()).unwrap_or_else(|| fallback.to_string());
        let info = match tags {
            Some(tags) => TrackInfo {
                title: pick(tags.title, &file_title),
                artist: pick(tags.artist, &file_artist),
                album: tags.album.unwrap_or_default(),
                year: tags.year,
                duration_secs: tags.duration_secs,
                genre: tags.genre,
                external_id: None,
            },
            None => TrackInfo { title: file_title, artist: file_artist, ..TrackInfo::default() },
        };
        let id = self.upsert(info);
        self.files.insert(key, FileEntry { track_id: id.clone(), size_bytes: track.size_bytes, modified_secs });
        id
    }

    /// Library id for any entry of a playlist listing — a local file or an import-manifest entry.
    pub fn resolve(&mut self, track: &TrackFile, service: Option<&PlaylistLink>) -> String {
        match &track.remote_metadata {
            None => self.resolve_file(track),
            Some(remote) => self.upsert(TrackInfo {
                title: track.name.clone(),
                artist: remote.artist.clone(),
                album: remote.album.clone(),
                year: None,
                duration_secs: remote.duration_secs,
                genre: None,
                external_id: remote.external_id.clone().zip(service.map(|link| link.service.label().to_lowercase())).map(|(id, service)| (service, id)),
            }),
        }
    }

    /// Local files (that still exist) holding this track.
    pub fn files_for(&self, id: &str) -> Vec<PathBuf> {
        self.files.iter().filter(|(_, entry)| entry.track_id == id).map(|(path, _)| PathBuf::from(path)).filter(|path| path.exists()).collect()
    }

    /// Forgets cached files that no longer exist on disk (the tracks themselves stay).
    pub fn prune_missing_files(&mut self) {
        self.files.retain(|path, _| Path::new(path).exists());
    }

    /// Tags that describe a playlist as a whole: the ones shared by a meaningful slice of its
    /// tracks, most common first.
    pub fn playlist_tags(&self, track_ids: &[String]) -> Vec<String> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut first_seen: HashMap<String, usize> = HashMap::new();
        let mut tagged = 0usize;
        for id in track_ids {
            let Some(track) = self.tracks.get(id) else { continue };
            let tags = track.all_tags();
            if tags.is_empty() {
                continue;
            }
            tagged += 1;
            for tag in tags {
                let order = first_seen.len();
                first_seen.entry(tag.clone()).or_insert(order);
                *counts.entry(tag).or_default() += 1;
            }
        }
        if tagged == 0 {
            return Vec::new();
        }
        let threshold = if tagged < 4 { 1 } else { ((tagged as f64) * 0.15).ceil().max(2.0) as usize };
        let mut ranked: Vec<(String, usize)> = counts.into_iter().filter(|(_, count)| *count >= threshold).collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| first_seen[&a.0].cmp(&first_seen[&b.0])));
        ranked.into_iter().take(6).map(|(tag, _)| tag).collect()
    }
}

fn library_path() -> io::Result<PathBuf> {
    Ok(config::config_dir()?.join("library.json"))
}

pub fn load() -> io::Result<Option<Library>> {
    let path = library_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(path)?;
    serde_json::from_str(&contents).map(Some).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn save(library: &Library) -> io::Result<()> {
    let path = library_path()?;
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let contents = serde_json::to_string(library).map_err(io::Error::other)?;
    // Write-then-rename so a crash mid-save never leaves a truncated library behind.
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, contents)?;
    fs::rename(temp, path)
}

pub const BACKUP_FILE_NAME: &str = "crate-rat-backup.json";

/// Everything needed to rebuild the library and playlists elsewhere — no credentials.
#[derive(Serialize, Deserialize)]
pub struct Backup {
    pub version: u32,
    pub saved_secs: u64,
    pub crates: Vec<BackupCrate>,
    pub tracks: BTreeMap<String, LibraryTrack>,
}

#[derive(Serialize, Deserialize)]
pub struct BackupCrate {
    pub name: String,
    pub paths: Vec<String>,
    pub playlists: Vec<BackupPlaylist>,
}

#[derive(Serialize, Deserialize)]
pub struct BackupPlaylist {
    pub name: String,
    pub tags: Vec<String>,
    pub auto_tags: Vec<String>,
    pub link: Option<PlaylistLink>,
    /// Track ids, in order; each one is a key of `Backup::tracks`.
    pub tracks: Vec<String>,
}

pub fn write_backup(dir: &str, crates: &[CrateLocation], library: &Library) -> io::Result<PathBuf> {
    let dir = PathBuf::from(dir);
    fs::create_dir_all(&dir)?;
    let backup = Backup {
        version: 1,
        saved_secs: now_secs(),
        crates: crates
            .iter()
            .map(|crate_location| BackupCrate {
                name: crate_location.name.clone(),
                paths: crate_location.locations.iter().map(|location| location.path.clone()).collect(),
                playlists: crate_location
                    .playlists
                    .iter()
                    .map(|playlist| BackupPlaylist { name: playlist.name.clone(), tags: playlist.tags.clone(), auto_tags: playlist.auto_tags.clone(), link: playlist.link.clone(), tracks: playlist.track_ids.clone() })
                    .collect(),
            })
            .collect(),
        tracks: library.tracks.clone(),
    };
    let path = dir.join(BACKUP_FILE_NAME);
    let contents = serde_json::to_string_pretty(&backup).map_err(io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, contents)?;
    fs::rename(&temp, &path)?;
    Ok(path)
}

pub fn read_backup(dir: &str) -> io::Result<Backup> {
    let contents = fs::read_to_string(PathBuf::from(dir).join(BACKUP_FILE_NAME))?;
    serde_json::from_str(&contents).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_parsing() {
        assert_eq!(info_from_filename("01 - Aphex Twin - Windowlicker.flac"), ("Aphex Twin".into(), "Windowlicker".into()));
        assert_eq!(info_from_filename("Aphex Twin - Xtal.mp3"), ("Aphex Twin".into(), "Xtal".into()));
        assert_eq!(info_from_filename("Xtal.mp3"), (String::new(), "Xtal".into()));
        assert_eq!(info_from_filename("808 State - Pacific.m4a"), ("808 State".into(), "Pacific".into()));
        assert_eq!(info_from_filename("03. Burial - Archangel.mp3"), ("Burial".into(), "Archangel".into()));
    }

    #[test]
    fn same_song_is_one_entry() {
        let mut library = Library::default();
        let a = library.upsert(TrackInfo { title: "Windowlicker".into(), artist: "Aphex Twin".into(), ..TrackInfo::default() });
        let b = library.upsert(TrackInfo { title: "windowlicker!".into(), artist: "APHEX TWIN".into(), album: "Windowlicker EP".into(), ..TrackInfo::default() });
        assert_eq!(a, b);
        assert_eq!(library.tracks.len(), 1);
        assert_eq!(library.tracks[&a].album, "Windowlicker EP");
    }

    #[test]
    fn artistless_sighting_merges() {
        let mut library = Library::default();
        let bare = library.upsert(TrackInfo { title: "Xtal".into(), ..TrackInfo::default() });
        let full = library.upsert(TrackInfo { title: "Xtal".into(), artist: "Aphex Twin".into(), ..TrackInfo::default() });
        assert_ne!(bare, full);
        assert_eq!(library.tracks.len(), 1, "artistless entry is promoted into the full one");
        let again = library.upsert(TrackInfo { title: "Xtal".into(), ..TrackInfo::default() });
        assert_eq!(again, full);
    }

    #[test]
    fn playlist_tags_from_tracks() {
        let mut library = Library::default();
        let mut ids = Vec::new();
        for (index, tags) in [vec!["techno", "acid"], vec!["techno"], vec!["techno", "house"], vec!["acid", "techno"], vec!["ambient"]].into_iter().enumerate() {
            let id = library.upsert(TrackInfo { title: format!("t{index}"), artist: "a".into(), ..TrackInfo::default() });
            library.tracks.get_mut(&id).unwrap().tags = tags.into_iter().map(String::from).collect();
            ids.push(id);
        }
        assert_eq!(library.playlist_tags(&ids), vec!["techno".to_string(), "acid".to_string()]);
    }
}

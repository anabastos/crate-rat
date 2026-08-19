use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateLocation {
    pub name: String,
    pub locations: Vec<Location>,
    pub available: bool,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub path: String,
    #[serde(default)]
    pub removable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub track_count: usize,
    pub synced: usize,
    pub tags: Vec<String>,
    #[serde(default)]
    pub link: Option<PlaylistLink>,
}

impl Playlist {
    pub fn status(&self) -> &'static str {
        if self.synced == self.track_count { "synced" } else { "needs sync" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistLink {
    pub service: ImportService,
    pub external_name: String,
    #[serde(default)]
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportService {
    Tidal,
    Spotify,
    SoundCloud,
}

impl ImportService {
    pub const ALL: [ImportService; 3] = [ImportService::Tidal, ImportService::Spotify, ImportService::SoundCloud];

    pub fn label(self) -> &'static str {
        match self {
            ImportService::Tidal => "Tidal",
            ImportService::Spotify => "Spotify",
            ImportService::SoundCloud => "SoundCloud",
        }
    }
}
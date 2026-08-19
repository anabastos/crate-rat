use std::{env, fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::CrateLocation;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub crates: Vec<CrateLocation>,
    #[serde(default)]
    pub spotify_client_id: Option<String>,
    #[serde(default)]
    pub spotify_refresh_token: Option<String>,
    #[serde(default)]
    pub tidal_client_id: Option<String>,
    #[serde(default)]
    pub tidal_client_secret: Option<String>,
}

pub fn load() -> io::Result<Option<AppConfig>> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(path)?;
    let config = serde_json::from_str(&contents).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Some(config))
}

pub fn save(config: &AppConfig) -> io::Result<()> {
    let path = config_path()?;
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }

    let contents = serde_json::to_string_pretty(config).map_err(io::Error::other)?;
    fs::write(path, contents)
}

pub fn display_path() -> String {
    config_path().map_or_else(|_| "platform config directory".into(), |path| path.display().to_string())
}

fn config_path() -> io::Result<PathBuf> {
    if let Some(app_data) = env::var_os("APPDATA") {
        return Ok(PathBuf::from(app_data).join("ratcrate").join("config.json"));
    }

    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(config_home).join("ratcrate").join("config.json"));
    }

    env::var_os("HOME").map_or_else(
        || Err(io::Error::new(io::ErrorKind::NotFound, "could not find a user config directory")),
        |home| Ok(PathBuf::from(home).join(".config").join("ratcrate").join("config.json")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Location, Playlist};

    #[test]
    fn missing_config_is_not_an_error() {
        let path = config_path().expect("config path should be available");
        if !path.exists() {
            assert!(load().expect("missing config should load cleanly").is_none());
        }
    }

    #[test]
    fn config_round_trips_as_json() {
        let crates = vec![CrateLocation {
            name: "Test crate".into(),
            locations: vec![Location { path: "/music/test".into(), removable: false }],
            available: true,
            playlists: vec![Playlist { name: "Test playlist".into(), track_count: 10, synced: 5, tags: vec!["test".into()], link: None }],
        }];
        let config = AppConfig { crates: crates.clone(), spotify_client_id: None, spotify_refresh_token: None, tidal_client_id: None, tidal_client_secret: None };
        let json = serde_json::to_string(&config).expect("app config should serialize");
        let restored: AppConfig = serde_json::from_str(&json).expect("app config should deserialize");
        assert_eq!(restored.crates[0].name, crates[0].name);
        assert_eq!(restored.crates[0].locations[0].path, crates[0].locations[0].path);
        assert_eq!(restored.crates[0].playlists[0].name, crates[0].playlists[0].name);
    }
}

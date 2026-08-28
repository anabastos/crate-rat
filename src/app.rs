use std::path::PathBuf;

use crossterm::event::KeyCode;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;

use crate::config;
use crate::metadata::{self, TrackMetadata};
use crate::model::{CrateLocation, ImportService, Location, Playlist, PlaylistLink};
use crate::soundcloud;
use crate::spotify;
use crate::sync;
use crate::sync::TrackFile;
use crate::tidal;

/// Max tracks searched/downloaded from Tidal per press of `D`, so a huge playlist doesn't turn
/// into an unattended hours-long batch — press `D` again to work through the rest.
const TIDAL_DOWNLOAD_BATCH_LIMIT: usize = 50;

pub struct App {
    pub should_quit: bool,
    pub screen: Screen,
    pub selected_crate: usize,
    pub selected_playlist: usize,
    pub config_field: usize,
    pub editing_config: bool,
    pub config_buffer: String,
    pub config_cursor: usize,
    pub editing_tags: bool,
    pub tag_buffer: String,
    pub tag_cursor: usize,
    pub crates: Vec<CrateLocation>,
    pub message: String,
    pub pending_delete: Option<usize>,
    pub tag_browser: Option<TagBrowser>,
    pub tidal_log: Option<DownloadLog>,
    pub import: Option<ImportState>,
    pub tracks: Option<TrackView>,
    picker: Option<Picker>,
    pub spotify_client_id: Option<String>,
    pub spotify_refresh_token: Option<String>,
    pub tidal_client_id: Option<String>,
    pub tidal_client_secret: Option<String>,
    pub tidal_country_code: Option<String>,
    pub settings_field: usize,
    pub editing_settings: bool,
    pub settings_buffer: String,
    pub settings_cursor: usize,
    spotify_login_rx: Option<std::sync::mpsc::Receiver<Result<(String, String), String>>>,
    audio_stream: Option<rodio::OutputStream>,
    audio_handle: Option<rodio::OutputStreamHandle>,
    audio_sink: Option<rodio::Sink>,
    pub now_playing: Option<usize>,
    pub audio_paused: bool,
    import_fetch_rx: Option<std::sync::mpsc::Receiver<Result<ImportedPlaylist, String>>>,
    pending_spotify_import: Option<PendingImport>,
    tidal_search_rx: Option<std::sync::mpsc::Receiver<Result<Vec<bool>, String>>>,
    soundcloud_download_rx: Option<std::sync::mpsc::Receiver<Result<soundcloud::FetchResult, String>>>,
    spotify_tidal_download_rx: Option<std::sync::mpsc::Receiver<TidalDownloadUpdate>>,
    tidal_playlist_download_rx: Option<std::sync::mpsc::Receiver<TidalPlaylistDownloadUpdate>>,
    tidal_crate_refresh_rx: Option<std::sync::mpsc::Receiver<(String, String, Result<tidal::PlaylistInfo, String>)>>,
    /// Crates already auto-checked against Tidal this session, so switching back and forth
    /// between crates doesn't re-hit Tidal's API (which is both rate-limited and, per track,
    /// slow — `MIN_REQUEST_INTERVAL` in tidal.rs paces requests ~5s apart) every single time.
    /// Press `R` on a specific playlist to force a fresh check regardless.
    tidal_refreshed_crates: std::collections::HashSet<String>,
}

struct PendingImport {
    service: ImportService,
    crate_index: usize,
    playlist_index: usize,
    is_new: bool,
    link: String,
}

/// Sent from the Tidal download thread as it works through the track list, so the UI can show
/// which track is currently being searched/downloaded instead of just a static "working…" message.
enum TidalDownloadUpdate {
    Progress { index: usize, total: usize, title: String },
    /// A single line of output for one track (search result or `tidal-dl-ng` output), appended
    /// to the live log as it happens.
    Log { title: String, line: String },
    /// Marks the switch from the search phase to the download phase, so subsequent `Log`
    /// entries land in the right panel.
    Phase(LogPhase),
    Done(Result<TidalDownloadOutcome, String>),
}

/// Sent from the whole-playlist Tidal download thread (linked-via-Tidal playlists, downloaded in
/// one `tidal-dl-ng dl <playlist_url>` call rather than track-by-track).
enum TidalPlaylistDownloadUpdate {
    Log(String),
    Done(Result<(), String>),
}

struct TidalDownloadOutcome {
    downloaded: usize,
    /// (track title, outcome) for every track in the batch, success or not.
    log: Vec<(String, String)>,
    /// Whether more un-synced tracks remain beyond this batch's `TIDAL_DOWNLOAD_BATCH_LIMIT` cap.
    has_more: bool,
}

/// Common shape both spotify::FetchResult and soundcloud::FetchResult get mapped into,
/// so the fetch/poll/apply pipeline doesn't care which service produced it.
struct ImportedPlaylist {
    name: String,
    tracks: Vec<ImportedTrack>,
    spotify_refresh_token: Option<String>,
}

struct ImportedTrack {
    title: String,
    artist: String,
    album: String,
    duration_secs: u64,
    /// The track's id on the source service, when known up front (currently only Tidal — its
    /// playlist fetch returns real track ids, so a missing track can be downloaded directly by
    /// id instead of searched for like Spotify's metadata-only tracks are).
    external_id: Option<String>,
}

pub struct TrackView {
    pub crate_name: String,
    pub playlist: Playlist,
    pub tracks: Vec<TrackFile>,
    pub selected: usize,
    pub metadata: Option<TrackMetadata>,
    pub cover: Option<Box<dyn Protocol>>,
    /// One slot per track: `None` = not checked, `Some(true/false)` = found/not found on Tidal.
    pub tidal_status: Vec<Option<bool>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogPhase {
    Finding,
    Downloading,
}

/// Log of every track a Tidal search/download batch processed (success or not), scrollable, so
/// the user can see what happened to each track instead of just an aggregate count. Shown
/// automatically after every batch — no key needed to reveal it. Two panels — search phase and
/// download phase — each a list of (title, detail) entries rendered as a header line per track
/// plus its indented detail lines.
pub struct DownloadLog {
    pub finding: Vec<(String, String)>,
    pub finding_selected: usize,
    pub downloading: Vec<(String, String)>,
    pub downloading_selected: usize,
    /// Which panel new entries currently get routed into.
    pub current_phase: LogPhase,
    /// Which panel j/k scrolls right now — Tab switches this independently of `current_phase`,
    /// so you can review the finding panel while downloads are still running.
    pub focused_pane: LogPhase,
    /// True for a batch that never has a search phase (Tidal-linked playlists — track ids are
    /// already known from the playlist fetch, nothing to search for), so the UI shows one
    /// full-width "downloading" panel instead of the usual finding/downloading split.
    pub single_pane: bool,
}

impl DownloadLog {
    fn new() -> Self {
        DownloadLog { finding: Vec::new(), finding_selected: 0, downloading: Vec::new(), downloading_selected: 0, current_phase: LogPhase::Finding, focused_pane: LogPhase::Finding, single_pane: false }
    }

    /// For batches with only a download phase (see `single_pane`) — starts already focused on
    /// (and logging into) the downloading panel, since the finding one will never be used.
    fn new_single_pane() -> Self {
        DownloadLog { current_phase: LogPhase::Downloading, focused_pane: LogPhase::Downloading, single_pane: true, ..DownloadLog::new() }
    }

    fn scroll_down(&mut self) {
        let (entries, selected) = match self.focused_pane {
            LogPhase::Finding => (&self.finding, &mut self.finding_selected),
            LogPhase::Downloading => (&self.downloading, &mut self.downloading_selected),
        };
        if !entries.is_empty() {
            *selected = (*selected + 1) % entries.len();
        }
    }

    fn scroll_up(&mut self) {
        let (entries, selected) = match self.focused_pane {
            LogPhase::Finding => (&self.finding, &mut self.finding_selected),
            LogPhase::Downloading => (&self.downloading, &mut self.downloading_selected),
        };
        if !entries.is_empty() {
            *selected = selected.checked_sub(1).unwrap_or(entries.len() - 1);
        }
    }

    fn toggle_focus(&mut self) {
        if self.single_pane {
            return;
        }
        self.focused_pane = match self.focused_pane {
            LogPhase::Finding => LogPhase::Downloading,
            LogPhase::Downloading => LogPhase::Finding,
        };
    }

    /// Appends an entry to whichever panel `current_phase` points at, following the tail only if
    /// that panel's view was already scrolled to the bottom — so scrolling up to read something
    /// doesn't keep getting yanked back down by new lines arriving (the "logging and erasing"
    /// feeling from before).
    fn push(&mut self, title: String, line: String) {
        let (entries, selected) = match self.current_phase {
            LogPhase::Finding => (&mut self.finding, &mut self.finding_selected),
            LogPhase::Downloading => (&mut self.downloading, &mut self.downloading_selected),
        };
        let was_at_bottom = *selected + 1 >= entries.len();
        entries.push((title, line));
        if was_at_bottom {
            *selected = entries.len() - 1;
        }
    }
}

pub struct TagBrowser {
    pub tags: Vec<String>,
    pub selected: usize,
    pub filter: Option<String>,
    pub matches: Vec<(String, String)>,
    pub match_selected: usize,
}

#[derive(PartialEq, Eq)]
pub enum ImportStep {
    Service,
    Mode,
    Crate,
    Playlist,
    Name,
}

pub struct ImportState {
    pub step: ImportStep,
    pub service_index: usize,
    pub service: Option<ImportService>,
    pub is_new: Option<bool>,
    pub mode_index: usize,
    pub crate_index: usize,
    pub playlist_index: usize,
    pub name_buffer: String,
    pub name_cursor: usize,
}

impl ImportState {
    fn new() -> Self {
        Self { step: ImportStep::Service, service_index: 0, service: None, is_new: None, mode_index: 0, crate_index: 0, playlist_index: 0, name_buffer: String::new(), name_cursor: 0 }
    }
}

impl App {
    pub fn demo() -> Self {
        Self {
            should_quit: false,
            screen: Screen::Dashboard,
            selected_crate: 0,
            selected_playlist: 0,
            config_field: 1,
            editing_config: false,
            config_buffer: String::new(),
            config_cursor: 0,
            editing_tags: false,
            tag_buffer: String::new(),
            tag_cursor: 0,
            crates: vec![],
            message: "No crates yet. Press c, then n to add one.".into(),
            pending_delete: None,
            tag_browser: None,
            tidal_log: None,
            import: None,
            tracks: None,
            picker: None,
            spotify_client_id: None,
            spotify_refresh_token: None,
            tidal_client_id: None,
            tidal_client_secret: None,
            tidal_country_code: None,
            settings_field: 0,
            editing_settings: false,
            settings_buffer: String::new(),
            settings_cursor: 0,
            spotify_login_rx: None,
            audio_stream: None,
            audio_handle: None,
            audio_sink: None,
            now_playing: None,
            audio_paused: false,
            import_fetch_rx: None,
            pending_spotify_import: None,
            tidal_search_rx: None,
            soundcloud_download_rx: None,
            spotify_tidal_download_rx: None,
            tidal_playlist_download_rx: None,
            tidal_crate_refresh_rx: None,
            tidal_refreshed_crates: std::collections::HashSet::new(),
        }
    }

    fn play_track(&mut self, path: &std::path::Path, index: usize) {
        if self.audio_handle.is_none() {
            match rodio::OutputStream::try_default() {
                Ok((stream, handle)) => {
                    self.audio_stream = Some(stream);
                    self.audio_handle = Some(handle);
                }
                Err(error) => {
                    self.message = format!("Could not open audio output: {error}");
                    return;
                }
            }
        }
        let Some(handle) = &self.audio_handle else { return };

        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) => {
                self.message = format!("Could not open that track: {error}");
                return;
            }
        };
        let source = match rodio::Decoder::new(std::io::BufReader::new(file)) {
            Ok(source) => source,
            Err(error) => {
                self.message = format!("Could not decode that track: {error}");
                return;
            }
        };
        match rodio::Sink::try_new(handle) {
            Ok(sink) => {
                sink.append(source);
                self.audio_sink = Some(sink);
                self.now_playing = Some(index);
                self.audio_paused = false;
                self.message = "▶ playing".into();
            }
            Err(error) => self.message = format!("Could not play that track: {error}"),
        }
    }

    fn toggle_pause(&mut self) {
        let Some(sink) = &self.audio_sink else { return };
        if self.audio_paused {
            sink.play();
            self.message = "▶ playing".into();
        } else {
            sink.pause();
            self.message = "⏸ paused".into();
        }
        self.audio_paused = !self.audio_paused;
    }

    fn stop_playback(&mut self) {
        self.audio_sink = None;
        self.now_playing = None;
        self.audio_paused = false;
    }

    fn save_config(&self) -> std::io::Result<()> {
        config::save(&config::AppConfig {
            crates: self.crates.clone(),
            spotify_client_id: self.spotify_client_id.clone(),
            spotify_refresh_token: self.spotify_refresh_token.clone(),
            tidal_client_id: self.tidal_client_id.clone(),
            tidal_client_secret: self.tidal_client_secret.clone(),
            tidal_country_code: self.tidal_country_code.clone(),
        })
    }

    fn ensure_picker(&mut self) -> &mut Picker {
        if self.picker.is_none() {
            let mut picker = Picker::new((8, 12));
            picker.protocol_type = ProtocolType::Halfblocks;
            self.picker = Some(picker);
        }
        self.picker.as_mut().expect("picker was just initialized")
    }

    /// (Re)builds the metadata + cover art preview for whichever track is currently selected.
    fn refresh_selected_track(&mut self) {
        let Some(view) = &self.tracks else { return };
        let Some(track) = view.tracks.get(view.selected) else { return };
        let metadata = if let Some(remote) = &track.remote_metadata {
            Some(TrackMetadata {
                title: Some(track.name.clone()),
                artist: (!remote.artist.is_empty()).then(|| remote.artist.clone()),
                album: (!remote.album.is_empty()).then(|| remote.album.clone()),
                year: None,
                genre: None,
                duration_secs: remote.duration_secs,
                cover: None,
            })
        } else {
            metadata::read_track_metadata(&track.path)
        };
        let cover_bytes = metadata.as_ref().and_then(|metadata| metadata.cover.clone());

        let cover = cover_bytes.and_then(|bytes| image::load_from_memory(&bytes).ok()).and_then(|image| {
            let picker = self.ensure_picker();
            let area = ratatui::layout::Rect::new(0, 0, 32, 16);
            picker.new_protocol(image, area, Resize::Fit(None)).ok()
        });

        if let Some(view) = &mut self.tracks {
            view.metadata = metadata;
            view.cover = cover;
        }
    }

    pub fn load() -> Self {
        let mut app = Self::demo();
        match config::load() {
            Ok(Some(loaded)) if !loaded.crates.is_empty() => {
                app.spotify_client_id = loaded.spotify_client_id;
                app.spotify_refresh_token = loaded.spotify_refresh_token;
                app.tidal_client_id = loaded.tidal_client_id;
                app.tidal_client_secret = loaded.tidal_client_secret;
                app.tidal_country_code = loaded.tidal_country_code;
                app.crates = loaded.crates;
                app.message = "Welcome back. Your crate map is loaded.".into();
            }
            Ok(Some(loaded)) => {
                app.spotify_client_id = loaded.spotify_client_id;
                app.spotify_refresh_token = loaded.spotify_refresh_token;
                app.tidal_client_id = loaded.tidal_client_id;
                app.tidal_client_secret = loaded.tidal_client_secret;
                app.tidal_country_code = loaded.tidal_country_code;
                app.message = "No crates yet. Press c, then n to add one.".into();
            }
            Ok(None) => app.message = format!("No config yet. Press c, then n to add a crate. ({})", config::display_path()),
            Err(error) => app.message = format!("Could not load config: {}. Starting with no crates.", error),
        }
        app.refresh_availability();
        app.rescan_playlists();
        let _ = app.save_config();
        app.start_tidal_crate_refresh(app.selected_crate);
        app
    }

    fn current_playlists(&self) -> &[Playlist] {
        self.crates.get(self.selected_crate).map_or(&[], |crate_location| crate_location.playlists.as_slice())
    }

    pub fn handle_key(&mut self, key: KeyCode) {
        if self.import_fetch_rx.is_some() {
            match key {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Esc | KeyCode::Char('c') => {
                    self.import_fetch_rx = None;
                    self.pending_spotify_import = None;
                    self.message = "Import cancelled.".into();
                }
                _ => {}
            }
            return;
        }
        if self.soundcloud_download_rx.is_some() {
            match key {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Esc | KeyCode::Char('c') => {
                    self.soundcloud_download_rx = None;
                    self.message = "Download cancelled.".into();
                }
                _ => {}
            }
            return;
        }
        if self.spotify_tidal_download_rx.is_some() {
            match key {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Esc | KeyCode::Char('c') => {
                    self.spotify_tidal_download_rx = None;
                    self.message = "Download cancelled.".into();
                }
                // Let the log scroll while a download is still running, so you can watch it live.
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Some(log) = &mut self.tidal_log {
                        log.scroll_down();
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Some(log) = &mut self.tidal_log {
                        log.scroll_up();
                    }
                }
                KeyCode::Tab => {
                    if let Some(log) = &mut self.tidal_log {
                        log.toggle_focus();
                    }
                }
                _ => {}
            }
            return;
        }
        if self.tidal_playlist_download_rx.is_some() {
            match key {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Esc | KeyCode::Char('c') => {
                    self.tidal_playlist_download_rx = None;
                    self.message = "Download cancelled.".into();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Some(log) = &mut self.tidal_log {
                        log.scroll_down();
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Some(log) = &mut self.tidal_log {
                        log.scroll_up();
                    }
                }
                _ => {}
            }
            return;
        }
        if self.tidal_log.is_some() {
            self.handle_download_log_key(key);
            return;
        }
        if let Some(index) = self.pending_delete {
            self.handle_confirm_delete(key, index);
            return;
        }
        if self.editing_tags {
            self.handle_tag_edit_key(key);
            return;
        }
        if self.tag_browser.is_some() {
            self.handle_tag_browser_key(key);
            return;
        }
        if self.tracks.is_some() {
            self.handle_tracks_key(key);
            return;
        }
        match self.screen {
            Screen::Config => self.handle_config_key(key),
            Screen::Import => self.handle_import_key(key),
            Screen::Settings => self.handle_settings_key(key),
            Screen::Dashboard => self.handle_dashboard_key(key),
        }
    }

    fn handle_dashboard_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => {
                let count = self.current_playlists().len();
                if count > 0 {
                    self.selected_playlist = (self.selected_playlist + 1) % count;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let count = self.current_playlists().len();
                if count > 0 {
                    self.selected_playlist = self.selected_playlist.checked_sub(1).unwrap_or(count - 1);
                }
            }
            KeyCode::Tab => {
                if !self.crates.is_empty() {
                    self.selected_crate = (self.selected_crate + 1) % self.crates.len();
                    self.selected_playlist = 0;
                    self.start_tidal_crate_refresh(self.selected_crate);
                }
            }
            KeyCode::Char('r') => {
                self.refresh_availability();
                self.rescan_playlists();
                self.selected_playlist = self.selected_playlist.min(self.current_playlists().len().saturating_sub(1));
                let missing: Vec<&str> = self.crates.iter().filter(|crate_location| !crate_location.available).map(|crate_location| crate_location.name.as_str()).collect();
                let save_note = if self.save_config().is_ok() { "" } else { " (could not save)" };
                self.message = if missing.is_empty() {
                    format!("Playlists rescanned from disk.{save_note}")
                } else {
                    format!("Playlists rescanned. Missing paths: {}.{save_note}", missing.join(", "))
                };
            }
            KeyCode::Char('c') => {
                self.screen = Screen::Config;
                self.config_field = if self.crates.is_empty() { 0 } else { 1 };
                self.message = "Manage crates. Press n for a new one, Enter to edit a field.".into();
            }
            KeyCode::Char('n') => {
                self.crates.push(CrateLocation { name: "New crate".into(), locations: vec![Location { path: String::new(), removable: false }], available: true, playlists: vec![] });
                self.selected_crate = self.crates.len() - 1;
                self.config_field = 0;
                self.screen = Screen::Config;
                self.message = "New crate added. Name it, then Enter to save.".into();
            }
            KeyCode::Char('i') => {
                self.import = Some(ImportState::new());
                self.screen = Screen::Import;
                self.message = "Choose a service to import from.".into();
            }
            KeyCode::Char('s') => {
                self.screen = Screen::Settings;
            }
            KeyCode::Enter => {
                let Some(crate_location) = self.crates.get(self.selected_crate) else {
                    self.message = "No crates yet. Press c, then n to add one.".into();
                    return;
                };
                let Some(playlist) = crate_location.playlists.get(self.selected_playlist) else {
                    self.message = "No playlist selected.".into();
                    return;
                };
                let tracks = sync::list_playlist_tracks(crate_location, &playlist.name);
                let tidal_status = vec![None; tracks.len()];
                self.tracks = Some(TrackView { crate_name: crate_location.name.clone(), playlist: playlist.clone(), tracks, selected: 0, metadata: None, cover: None, tidal_status });
                self.refresh_selected_track();
            }
            KeyCode::Char('T') => {
                let Some(playlist) = self.current_playlists().get(self.selected_playlist) else {
                    self.message = "No playlist selected.".into();
                    return;
                };
                self.tag_buffer = playlist.tags.join(", ");
                self.tag_cursor = self.tag_buffer.chars().count();
                self.editing_tags = true;
                self.message = "Editing tags (comma separated). Enter to save, Esc to cancel.".into();
            }
            KeyCode::Char('t') => {
                let mut tags: Vec<String> = self.crates.iter().flat_map(|crate_location| crate_location.playlists.iter()).flat_map(|playlist| playlist.tags.iter().cloned()).collect();
                tags.sort_unstable();
                tags.dedup();
                if tags.is_empty() {
                    self.message = "No tags yet. Press T on a playlist to add some.".into();
                    return;
                }
                self.tag_browser = Some(TagBrowser { tags, selected: 0, filter: None, matches: Vec::new(), match_selected: 0 });
            }
            _ => {}
        }
    }

    fn handle_settings_key(&mut self, key: KeyCode) {
        if self.spotify_login_rx.is_some() {
            match key {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Esc | KeyCode::Char('c') => {
                    self.spotify_login_rx = None;
                    self.message = "Spotify login cancelled.".into();
                }
                _ => {}
            }
            return;
        }
        if self.editing_settings {
            match key {
                KeyCode::Esc => {
                    self.editing_settings = false;
                    self.settings_buffer.clear();
                    self.settings_cursor = 0;
                }
                KeyCode::Enter => {
                    let value = self.settings_buffer.trim().to_string();
                    let value = if value.is_empty() { None } else { Some(value) };
                    let label = match self.settings_field {
                        0 => {
                            self.spotify_client_id = value;
                            "Spotify Client ID"
                        }
                        1 => {
                            self.tidal_client_id = value;
                            "Tidal Client ID"
                        }
                        2 => {
                            self.tidal_client_secret = value;
                            "Tidal Client Secret"
                        }
                        _ => {
                            self.tidal_country_code = value.map(|country| country.to_uppercase());
                            "Tidal Country Code"
                        }
                    };
                    self.editing_settings = false;
                    self.settings_buffer.clear();
                    self.settings_cursor = 0;
                    self.message = match self.save_config() {
                        Ok(()) => format!("{label} saved."),
                        Err(error) => format!("Saved for this session, but could not write config: {error}"),
                    };
                }
                KeyCode::Left => self.settings_cursor = self.settings_cursor.saturating_sub(1),
                KeyCode::Right => self.settings_cursor = (self.settings_cursor + 1).min(self.settings_buffer.chars().count()),
                KeyCode::Home => self.settings_cursor = 0,
                KeyCode::End => self.settings_cursor = self.settings_buffer.chars().count(),
                KeyCode::Backspace => {
                    if self.settings_cursor > 0 {
                        let byte_index = char_byte_index(&self.settings_buffer, self.settings_cursor - 1);
                        self.settings_buffer.remove(byte_index);
                        self.settings_cursor -= 1;
                    }
                }
                KeyCode::Delete => {
                    if self.settings_cursor < self.settings_buffer.chars().count() {
                        let byte_index = char_byte_index(&self.settings_buffer, self.settings_cursor);
                        self.settings_buffer.remove(byte_index);
                    }
                }
                KeyCode::Char(character) => {
                    let byte_index = char_byte_index(&self.settings_buffer, self.settings_cursor);
                    self.settings_buffer.insert(byte_index, character);
                    self.settings_cursor += 1;
                }
                _ => {}
            }
            return;
        }

        match key {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc | KeyCode::Char('s') => {
                self.screen = Screen::Dashboard;
            }
            KeyCode::Up | KeyCode::Char('k') => self.settings_field = self.settings_field.checked_sub(1).unwrap_or(3),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.settings_field = (self.settings_field + 1) % 4,
            KeyCode::Enter | KeyCode::Char('e') => {
                self.settings_buffer = self.settings_field_value(self.settings_field).unwrap_or_default();
                self.settings_cursor = self.settings_buffer.chars().count();
                self.editing_settings = true;
            }
            KeyCode::Char('l') => self.spotify_login(),
            KeyCode::Char('L') if self.spotify_refresh_token.is_some() => {
                self.spotify_refresh_token = None;
                self.message = match self.save_config() {
                    Ok(()) => "Disconnected from Spotify.".into(),
                    Err(error) => format!("Disconnected, but could not save config: {error}"),
                };
            }
            _ => {}
        }
    }

    fn settings_field_value(&self, field: usize) -> Option<String> {
        match field {
            0 => self.spotify_client_id.clone(),
            1 => self.tidal_client_id.clone(),
            2 => self.tidal_client_secret.clone(),
            _ => self.tidal_country_code.clone(),
        }
    }

    /// ISO 3166-1 alpha-2 country code used for Tidal catalog lookups — defaults to BR (this
    /// account's region) if unset. The Tidal catalog is region-specific, so a mismatched country
    /// code can make artist/track ids resolved under it come back invalid.
    fn tidal_country_code(&self) -> String {
        self.tidal_country_code.clone().unwrap_or_else(|| "BR".to_string())
    }

    /// Runs the actual login (browser + local callback server + token exchange) on a
    /// background thread, so the UI keeps redrawing and responding to keys (q to quit, etc.)
    /// while it waits. `poll_spotify_login` picks up the result once it's ready.
    fn spotify_login(&mut self) {
        let Some(client_id) = self.spotify_client_id.clone() else {
            self.message = "Set a Spotify Client ID first (press e).".into();
            return;
        };
        if self.spotify_login_rx.is_some() {
            self.message = "Already waiting on a Spotify login.".into();
            return;
        }
        self.message = "Opening your browser to log in to Spotify… (up to 2 min — Esc to cancel)".into();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(spotify::login(&client_id));
        });
        self.spotify_login_rx = Some(rx);
    }

    pub fn spotify_login_pending(&self) -> bool {
        self.spotify_login_rx.is_some()
    }

    /// Call once per frame; non-blocking. Applies the login result as soon as the
    /// background thread finishes.
    pub fn poll_spotify_login(&mut self) {
        let Some(rx) = &self.spotify_login_rx else { return };
        match rx.try_recv() {
            Ok(Ok((_access_token, refresh_token))) => {
                self.spotify_refresh_token = Some(refresh_token);
                self.message = match self.save_config() {
                    Ok(()) => "Connected to Spotify.".into(),
                    Err(error) => format!("Connected, but could not save config: {error}"),
                };
                self.spotify_login_rx = None;
            }
            Ok(Err(error)) => {
                self.message = format!("Spotify login failed: {error}");
                self.spotify_login_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.message = "Spotify login thread crashed unexpectedly.".into();
                self.spotify_login_rx = None;
            }
        }
    }

    fn handle_import_key(&mut self, key: KeyCode) {
        let Some(is_name_step) = self.import.as_ref().map(|import| import.step == ImportStep::Name) else {
            return;
        };

        if is_name_step {
            if key == KeyCode::Enter {
                self.finalize_import();
                return;
            }
            let Some(import) = &mut self.import else { return };
            match key {
                KeyCode::Esc => {
                    import.step = if import.is_new == Some(true) { ImportStep::Crate } else { ImportStep::Playlist };
                }
                KeyCode::Left => import.name_cursor = import.name_cursor.saturating_sub(1),
                KeyCode::Right => import.name_cursor = (import.name_cursor + 1).min(import.name_buffer.chars().count()),
                KeyCode::Home => import.name_cursor = 0,
                KeyCode::End => import.name_cursor = import.name_buffer.chars().count(),
                KeyCode::Backspace => {
                    if import.name_cursor > 0 {
                        let byte_index = char_byte_index(&import.name_buffer, import.name_cursor - 1);
                        import.name_buffer.remove(byte_index);
                        import.name_cursor -= 1;
                    }
                }
                KeyCode::Delete => {
                    if import.name_cursor < import.name_buffer.chars().count() {
                        let byte_index = char_byte_index(&import.name_buffer, import.name_cursor);
                        import.name_buffer.remove(byte_index);
                    }
                }
                KeyCode::Char(character) => {
                    let byte_index = char_byte_index(&import.name_buffer, import.name_cursor);
                    import.name_buffer.insert(byte_index, character);
                    import.name_cursor += 1;
                }
                _ => {}
            }
            return;
        }

        match key {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => {
                let Some(import) = &mut self.import else { return };
                match import.step {
                    ImportStep::Service => {
                        self.import = None;
                        self.screen = Screen::Dashboard;
                        self.message = "Import cancelled.".into();
                    }
                    ImportStep::Mode => import.step = ImportStep::Service,
                    ImportStep::Crate => import.step = ImportStep::Mode,
                    ImportStep::Playlist => import.step = ImportStep::Crate,
                    ImportStep::Name => unreachable!(),
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.import_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.import_move(1),
            KeyCode::Enter => self.import_confirm_step(),
            _ => {}
        }
    }

    fn import_move(&mut self, delta: i32) {
        let Some(import) = &mut self.import else { return };
        match import.step {
            ImportStep::Service => {
                let len = ImportService::ALL.len();
                import.service_index = wrap_index(import.service_index, delta, len);
            }
            ImportStep::Mode => {
                import.mode_index = wrap_index(import.mode_index, delta, 2);
            }
            ImportStep::Crate => {
                if !self.crates.is_empty() {
                    import.crate_index = wrap_index(import.crate_index, delta, self.crates.len());
                }
            }
            ImportStep::Playlist => {
                let count = self.crates.get(import.crate_index).map_or(0, |crate_location| crate_location.playlists.len());
                if count > 0 {
                    import.playlist_index = wrap_index(import.playlist_index, delta, count);
                }
            }
            ImportStep::Name => {}
        }
    }

    fn import_confirm_step(&mut self) {
        let Some(import) = &mut self.import else { return };
        match import.step {
            ImportStep::Service => {
                import.service = Some(ImportService::ALL[import.service_index]);
                import.step = ImportStep::Mode;
            }
            ImportStep::Mode => {
                import.is_new = Some(import.mode_index == 0);
                import.crate_index = 0;
                import.step = ImportStep::Crate;
            }
            ImportStep::Crate => {
                if self.crates.is_empty() {
                    self.message = "No crates yet. Press c, then n to add one first.".into();
                    return;
                }
                if import.is_new == Some(true) {
                    import.name_buffer.clear();
                    import.name_cursor = 0;
                    import.step = ImportStep::Name;
                } else {
                    let has_playlists = self.crates.get(import.crate_index).is_some_and(|crate_location| !crate_location.playlists.is_empty());
                    if !has_playlists {
                        self.message = "That crate has no playlists to link yet.".into();
                        return;
                    }
                    import.playlist_index = 0;
                    import.step = ImportStep::Playlist;
                }
            }
            ImportStep::Playlist => {
                import.name_buffer.clear();
                import.name_cursor = 0;
                import.step = ImportStep::Name;
            }
            ImportStep::Name => {}
        }
    }

    fn finalize_import(&mut self) {
        let Some(current) = &self.import else { return };
        let fetches_online = matches!(current.service, Some(ImportService::Spotify) | Some(ImportService::SoundCloud) | Some(ImportService::Tidal));
        if current.name_buffer.trim().is_empty() {
            self.message = if fetches_online { "Paste a playlist link.".into() } else { "Playlist name can't be empty.".into() };
            return;
        }
        if let Some(service) = current.service {
            if fetches_online {
                let link = current.name_buffer.trim().to_string();
                let is_new = current.is_new.unwrap_or(true);
                let crate_index = current.crate_index;
                let playlist_index = current.playlist_index;
                self.import = None;
                self.start_online_import(service, crate_index, playlist_index, is_new, &link);
                return;
            }
        }

        let Some(import) = self.import.take() else { return };
        let Some(service) = import.service else { return };
        let external_name = import.name_buffer.trim().to_string();
        let is_new = import.is_new.unwrap_or(true);
        let crate_index = import.crate_index;

        if is_new {
            let Some(crate_location) = self.crates.get(crate_index) else {
                self.screen = Screen::Dashboard;
                return;
            };
            let Some(primary) = crate_location.locations.first() else {
                self.message = "That crate has no paths yet.".into();
                self.screen = Screen::Dashboard;
                return;
            };
            let folder = std::path::Path::new(&primary.path).join(&external_name);
            if let Err(error) = std::fs::create_dir_all(&folder) {
                self.message = format!("Could not create playlist folder: {error}");
                self.screen = Screen::Dashboard;
                return;
            }
            if let Some(crate_location) = self.crates.get_mut(crate_index) {
                let scanned = sync::scan_crate_playlists(crate_location);
                crate_location.playlists = merge_playlists(&crate_location.playlists, scanned);
                if let Some(playlist) = crate_location.playlists.iter_mut().find(|playlist| playlist.name.eq_ignore_ascii_case(&external_name)) {
                    playlist.link = Some(PlaylistLink { service, external_name: external_name.clone(), source_url: None });
                }
            }
            self.message = match self.save_config() {
                Ok(()) => format!("Created \"{external_name}\" and linked it to {}.", service.label()),
                Err(error) => format!("Created the playlist, but could not save config: {error}"),
            };
        } else {
            let playlist_index = import.playlist_index;
            if let Some(crate_location) = self.crates.get_mut(crate_index) {
                if let Some(playlist) = crate_location.playlists.get_mut(playlist_index) {
                    playlist.link = Some(PlaylistLink { service, external_name: external_name.clone(), source_url: None });
                }
            }
            self.message = match self.save_config() {
                Ok(()) => format!("Linked to {} playlist \"{external_name}\".", service.label()),
                Err(error) => format!("Linked for this session, but could not save config: {error}"),
            };
        }
        self.screen = Screen::Dashboard;
    }

    /// Kicks off the track-metadata fetch (Spotify login happens on that thread too, if
    /// needed) on a background thread and returns to the dashboard immediately.
    /// `poll_spotify_import` picks up the result once it's ready.
    fn start_online_import(&mut self, service: ImportService, crate_index: usize, playlist_index: usize, is_new: bool, link: &str) {
        let link = link.to_string();
        self.message = format!("Fetching tracks from {}… (up to 2 min — Esc to cancel)", service.label());
        self.screen = Screen::Dashboard;
        self.pending_spotify_import = Some(PendingImport { service, crate_index, playlist_index, is_new, link: link.clone() });

        let (tx, rx) = std::sync::mpsc::channel();
        match service {
            ImportService::Spotify => {
                let Some(client_id) = self.spotify_client_id.clone() else {
                    self.message = "Set a Spotify Client ID in Settings (s) first.".into();
                    self.pending_spotify_import = None;
                    return;
                };
                let refresh_token = self.spotify_refresh_token.clone();
                std::thread::spawn(move || {
                    let result = spotify::fetch_playlist(&client_id, refresh_token.as_deref(), &link).map(|fetched| ImportedPlaylist {
                        name: fetched.name,
                        tracks: fetched.tracks.into_iter().map(|track| ImportedTrack { title: track.title, artist: track.artist, album: track.album, duration_secs: track.duration_secs, external_id: None }).collect(),
                        spotify_refresh_token: Some(fetched.refresh_token),
                    });
                    let _ = tx.send(result);
                });
            }
            ImportService::SoundCloud => {
                let folder_hint = if is_new { None } else { self.crates.get(crate_index).and_then(|crate_location| crate_location.playlists.get(playlist_index)).map(|playlist| playlist.name.clone()) };
                let Some(primary_path) = self.crates.get(crate_index).and_then(|crate_location| crate_location.locations.first()).map(|location| location.path.clone()) else {
                    self.message = "That crate has no paths yet.".into();
                    self.pending_spotify_import = None;
                    return;
                };
                self.message = "Downloading tracks from SoundCloud… (up to 2 min — Esc to cancel)".into();
                std::thread::spawn(move || {
                    let result = soundcloud::download_playlist(&link, &primary_path, folder_hint.as_deref()).map(|fetched| ImportedPlaylist {
                        name: fetched.name,
                        tracks: fetched.tracks.into_iter().map(|track| ImportedTrack { title: track.title, artist: track.artist, album: String::new(), duration_secs: track.duration_secs, external_id: None }).collect(),
                        spotify_refresh_token: None,
                    });
                    let _ = tx.send(result);
                });
            }
            ImportService::Tidal => {
                let (Some(client_id), Some(client_secret)) = (self.tidal_client_id.clone(), self.tidal_client_secret.clone()) else {
                    self.message = "Set your Tidal Client ID and Client Secret in Settings (s) first.".into();
                    self.pending_spotify_import = None;
                    return;
                };
                let country_code = self.tidal_country_code();
                std::thread::spawn(move || {
                    let result = tidal::fetch_playlist(&client_id, &client_secret, &country_code, &link).map(|fetched| ImportedPlaylist {
                        name: fetched.name,
                        tracks: fetched.tracks.into_iter().map(|track| ImportedTrack { title: track.title, artist: track.artist, album: String::new(), duration_secs: 0, external_id: Some(track.id) }).collect(),
                        spotify_refresh_token: None,
                    });
                    let _ = tx.send(result);
                });
            }
        }
        self.import_fetch_rx = Some(rx);
    }

    /// Call once per frame; non-blocking.
    pub fn poll_spotify_import(&mut self) {
        let Some(rx) = &self.import_fetch_rx else { return };
        match rx.try_recv() {
            Ok(Ok(result)) => {
                self.import_fetch_rx = None;
                if let Some(pending) = self.pending_spotify_import.take() {
                    self.apply_online_import(pending, result);
                }
            }
            Ok(Err(error)) => {
                self.import_fetch_rx = None;
                self.pending_spotify_import = None;
                self.message = format!("Import failed: {error}");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.import_fetch_rx = None;
                self.pending_spotify_import = None;
                self.message = "Import thread crashed unexpectedly.".into();
            }
        }
    }

    fn apply_online_import(&mut self, pending: PendingImport, result: ImportedPlaylist) {
        if let Some(refresh_token) = &result.spotify_refresh_token {
            self.spotify_refresh_token = Some(refresh_token.clone());
        }

        let folder_name = if pending.is_new {
            result.name.clone()
        } else {
            self.crates.get(pending.crate_index).and_then(|crate_location| crate_location.playlists.get(pending.playlist_index)).map_or_else(|| result.name.clone(), |playlist| playlist.name.clone())
        };

        // SoundCloud tracks are already downloaded as real audio files by the time we get here
        // (soundcloud::download_playlist wrote them straight into the playlist folder); only
        // metadata-only services (Spotify — no downloadable stream via their API — and Tidal,
        // whose tracklist we fetch for bookkeeping but whose actual audio still comes from a
        // separate `D` via tidal-dl-ng) need the manifest fallback so the playlist's real track
        // count and "what's still missing" are known before anything is downloaded. Once a real
        // file lands, `sync::list_playlist_tracks` prefers it and drops the matching manifest
        // entry, so downloaded tracks always show their real (local file) data, not the manifest's.
        if pending.service != ImportService::SoundCloud {
            if let Some(crate_location) = self.crates.get(pending.crate_index) {
                if let Some(primary) = crate_location.locations.first() {
                    let folder = std::path::Path::new(&primary.path).join(&folder_name);
                    if let Err(error) = std::fs::create_dir_all(&folder) {
                        self.message = format!("Could not create playlist folder: {error}");
                        return;
                    }
                    let manifest: Vec<serde_json::Value> = result.tracks.iter().map(|track| serde_json::json!({"title": track.title, "artist": track.artist, "album": track.album, "duration_secs": track.duration_secs, "id": track.external_id})).collect();
                    if let Ok(contents) = serde_json::to_string_pretty(&manifest) {
                        let filename = format!("{}-tracks.json", pending.service.label().to_lowercase());
                        let _ = std::fs::write(folder.join(filename), contents);
                    }
                } else {
                    self.message = "That crate has no paths yet.".into();
                    return;
                }
            }
        }

        let track_count = result.tracks.len();
        if let Some(crate_location) = self.crates.get_mut(pending.crate_index) {
            let scanned = sync::scan_crate_playlists(crate_location);
            crate_location.playlists = merge_playlists(&crate_location.playlists, scanned);
            if let Some(playlist) = crate_location.playlists.iter_mut().find(|playlist| playlist.name.eq_ignore_ascii_case(&folder_name)) {
                playlist.link = Some(PlaylistLink { service: pending.service, external_name: result.name.clone(), source_url: Some(pending.link.clone()) });
            }
        }

        self.message = if pending.service == ImportService::Tidal {
            match self.save_config() {
                Ok(()) => format!("Linked to Tidal playlist \"{}\" ({track_count} tracks). Press D to download what's missing via tidal-dl-ng.", result.name),
                Err(error) => format!("Linked to Tidal playlist \"{}\" ({track_count} tracks), but could not save config: {error}", result.name),
            }
        } else {
            match self.save_config() {
                Ok(()) => format!("Pulled {track_count} tracks from {} playlist \"{}\".", pending.service.label(), result.name),
                Err(error) => format!("Pulled {track_count} tracks, but could not save config: {error}"),
            }
        };
    }

    fn handle_tracks_key(&mut self, key: KeyCode) {
        let Some(view) = &mut self.tracks else { return };
        match key {
            KeyCode::Esc => {
                self.stop_playback();
                self.tracks = None;
            }
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => {
                if !view.tracks.is_empty() {
                    view.selected = (view.selected + 1) % view.tracks.len();
                    self.refresh_selected_track();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !view.tracks.is_empty() {
                    view.selected = view.selected.checked_sub(1).unwrap_or(view.tracks.len() - 1);
                    self.refresh_selected_track();
                }
            }
            KeyCode::Enter | KeyCode::Char('p') => {
                let Some(track) = view.tracks.get(view.selected) else { return };
                let selected = view.selected;
                if track.remote_metadata.is_some() {
                    self.message = "This track is metadata only (no local audio file) — nothing to play.".into();
                } else if self.now_playing == Some(selected) {
                    self.toggle_pause();
                } else {
                    let path = track.path.clone();
                    self.play_track(&path, selected);
                }
            }
            KeyCode::Char('x') => self.stop_playback(),
            KeyCode::Char('F') => self.start_tidal_search(),
            KeyCode::Char('D') => self.start_download(),
            KeyCode::Char('R') => self.refresh_tidal_link(),
            _ => {}
        }
    }

    /// Re-fetches this playlist's metadata from Tidal (name, tracklist, track count) and
    /// rewrites its manifest — for playlists that got linked before playlist-tracklist fetching
    /// existed (so they never got a manifest and their track count/sync status/metadata never
    /// reflected Tidal at all), or whose Tidal playlist has changed since it was linked. Reuses
    /// the same fetch+apply path as linking fresh, just pointed at the playlist that's already
    /// open — goes back to the dashboard while it fetches, same as any other import.
    fn refresh_tidal_link(&mut self) {
        let Some(view) = &self.tracks else { return };
        let Some(link) = view.playlist.link.as_ref() else {
            self.message = "This playlist isn't linked to Tidal yet. Use i to import/link one.".into();
            return;
        };
        if link.service != ImportService::Tidal {
            self.message = "R (refresh from Tidal) only applies to Tidal-linked playlists.".into();
            return;
        }
        let Some(source_url) = link.source_url.clone() else {
            self.message = "This playlist's Tidal link is missing — re-link it via i.".into();
            return;
        };
        let crate_name = view.crate_name.clone();
        let playlist_name = view.playlist.name.clone();
        let Some(crate_index) = self.crates.iter().position(|crate_location| crate_location.name == crate_name) else { return };
        let Some(playlist_index) = self.crates[crate_index].playlists.iter().position(|playlist| playlist.name == playlist_name) else { return };
        self.start_online_import(ImportService::Tidal, crate_index, playlist_index, false, &source_url);
    }

    /// Checks every Tidal-linked playlist in a crate against Tidal as soon as that crate comes
    /// into view (on startup, and on switching to it with Tab), so track counts/sync status/
    /// metadata are already up to date without needing to open each playlist and press `R`
    /// manually. Runs once per crate per session (see `tidal_refreshed_crates`) — silent no-op
    /// if there's no Tidal credentials, no Tidal-linked playlists, or a refresh is already in
    /// flight (never overlaps requests, since Tidal's API is both rate-limited and slow per
    /// request — see `MIN_REQUEST_INTERVAL` in tidal.rs).
    fn start_tidal_crate_refresh(&mut self, crate_index: usize) {
        let Some(crate_location) = self.crates.get(crate_index) else { return };
        let crate_name = crate_location.name.clone();
        if self.tidal_refreshed_crates.contains(&crate_name) {
            return;
        }
        self.tidal_refreshed_crates.insert(crate_name.clone());
        if self.tidal_crate_refresh_rx.is_some() {
            return;
        }
        let (Some(client_id), Some(client_secret)) = (self.tidal_client_id.clone(), self.tidal_client_secret.clone()) else {
            return;
        };
        let country_code = self.tidal_country_code();
        let targets: Vec<(String, String)> = crate_location
            .playlists
            .iter()
            .filter_map(|playlist| {
                let link = playlist.link.as_ref()?;
                if link.service != ImportService::Tidal {
                    return None;
                }
                Some((playlist.name.clone(), link.source_url.clone()?))
            })
            .collect();
        if targets.is_empty() {
            return;
        }

        self.message = format!("Checking Tidal sync status… ({} playlist{})", targets.len(), if targets.len() == 1 { "" } else { "s" });
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (playlist_name, source_url) in targets {
                let result = tidal::fetch_playlist(&client_id, &client_secret, &country_code, &source_url);
                let _ = tx.send((crate_name.clone(), playlist_name, result));
            }
        });
        self.tidal_crate_refresh_rx = Some(rx);
    }

    /// Call once per frame; non-blocking.
    pub fn poll_tidal_crate_refresh(&mut self) {
        let Some(rx) = &self.tidal_crate_refresh_rx else { return };
        match rx.try_recv() {
            Ok((crate_name, playlist_name, Ok(fetched))) => self.apply_tidal_refresh(&crate_name, &playlist_name, fetched),
            Ok((_, _, Err(_))) => {}
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.tidal_crate_refresh_rx = None;
                if self.message.starts_with("Checking Tidal sync status") {
                    self.message = "Tidal sync status updated.".into();
                }
            }
        }
    }

    /// Writes a fetched Tidal tracklist as that playlist's manifest and rescans, so track
    /// count/sync status/metadata reflect it — same effect as linking fresh (`apply_online_import`)
    /// but without going through the import wizard, since this is triggered automatically rather
    /// than by the user pasting a link. Also refreshes the open track view if it's this playlist.
    fn apply_tidal_refresh(&mut self, crate_name: &str, playlist_name: &str, fetched: tidal::PlaylistInfo) {
        let Some(crate_index) = self.crates.iter().position(|crate_location| crate_location.name == crate_name) else { return };
        let Some(primary_path) = self.crates[crate_index].locations.first().map(|location| location.path.clone()) else { return };
        let folder = std::path::Path::new(&primary_path).join(playlist_name);
        if std::fs::create_dir_all(&folder).is_err() {
            return;
        }
        let manifest: Vec<serde_json::Value> = fetched.tracks.iter().map(|track| serde_json::json!({"title": track.title, "artist": track.artist, "album": "", "duration_secs": 0, "id": track.id})).collect();
        if let Ok(contents) = serde_json::to_string_pretty(&manifest) {
            let _ = std::fs::write(folder.join("tidal-tracks.json"), contents);
        }

        if let Some(crate_location) = self.crates.get_mut(crate_index) {
            let scanned = sync::scan_crate_playlists(crate_location);
            crate_location.playlists = merge_playlists(&crate_location.playlists, scanned);
        }
        let _ = self.save_config();

        if let Some(view) = &self.tracks {
            if view.crate_name == crate_name && view.playlist.name == playlist_name {
                let refreshed = self.crates.get(crate_index).and_then(|crate_location| {
                    let tracks = sync::list_playlist_tracks(crate_location, playlist_name);
                    crate_location.playlists.iter().find(|playlist| playlist.name == playlist_name).map(|playlist| (playlist.clone(), tracks))
                });
                if let Some((playlist, tracks)) = refreshed {
                    if let Some(view) = &mut self.tracks {
                        view.playlist = playlist;
                        view.tracks = tracks;
                        view.tidal_status = vec![None; view.tracks.len()];
                    }
                }
            }
        }
    }

    /// Downloads the real audio for the currently open playlist from SoundCloud (needs the
    /// playlist to be linked to a SoundCloud playlist first, via `i`).
    fn start_download(&mut self) {
        let Some(view) = &self.tracks else { return };
        let Some(service) = view.playlist.link.as_ref().map(|link| link.service) else {
            self.message = "This playlist isn't linked to a service yet. Use i to import/link one.".into();
            return;
        };
        match service {
            ImportService::SoundCloud => self.start_soundcloud_download(),
            ImportService::Spotify => self.start_spotify_tidal_download(),
            ImportService::Tidal => self.start_tidal_playlist_download(),
        }
    }

    /// Downloads the real audio for the currently open playlist straight from Tidal (needs the
    /// playlist to be linked to a Tidal playlist first, via `i`, and `tidal-dl-ng` installed +
    /// logged into a real Tidal subscription). When only some tracks are missing, downloads just
    /// those, one by one, using the track ids the playlist fetch already resolved (no search
    /// needed, unlike Spotify's metadata-only tracks) — far cheaper than re-running `dl
    /// <playlist_url>`, which walks the whole playlist again every time. Only falls back to the
    /// full-playlist call when nothing has been downloaded yet (first time) or a track's id is
    /// missing (an older manifest written before ids were tracked).
    fn start_tidal_playlist_download(&mut self) {
        let Some(view) = &self.tracks else { return };
        let Some(link) = view.playlist.link.as_ref() else {
            self.message = "This playlist isn't linked to Tidal yet. Use i to import/link one.".into();
            return;
        };
        let Some(source_url) = link.source_url.clone() else {
            self.message = "This playlist's Tidal link is missing — re-link it via i.".into();
            return;
        };
        if self.tidal_playlist_download_rx.is_some() {
            self.message = "Already downloading.".into();
            return;
        }
        let Some(primary_path) = self.crates.iter().find(|crate_location| crate_location.name == view.crate_name).and_then(|crate_location| crate_location.locations.first()).map(|location| location.path.clone()) else {
            self.message = "That crate has no paths yet.".into();
            return;
        };
        let folder_name = view.playlist.name.clone();

        let total = view.tracks.len();
        let missing: Vec<(String, Option<String>)> = view.tracks.iter().filter(|track| track.remote_metadata.is_some()).map(|track| (track.name.clone(), track.remote_metadata.as_ref().and_then(|metadata| metadata.external_id.clone()))).collect();
        if missing.is_empty() {
            self.message = "Nothing left to download — every track already has a local file.".into();
            return;
        }
        let use_bulk = missing.len() == total || missing.iter().any(|(_, id)| id.is_none());

        self.message = if use_bulk {
            "Downloading playlist via tidal-dl-ng… (can take several minutes — Esc to cancel)".into()
        } else {
            format!("Downloading {} missing track(s) via tidal-dl-ng… (Esc to cancel)", missing.len())
        };
        let log = self.tidal_log.get_or_insert_with(DownloadLog::new_single_pane);
        log.single_pane = true;
        log.current_phase = LogPhase::Downloading;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dest_dir = std::path::Path::new(&primary_path).join(&folder_name);
            let mut cfg_log = |line: &str| { let _ = tx.send(TidalPlaylistDownloadUpdate::Log(line.to_string())); };
            if let Err(error) = tidal::set_tidal_dl_ng_download_path(&dest_dir, &mut cfg_log) {
                let _ = tx.send(TidalPlaylistDownloadUpdate::Done(Err(format!("could not set tidal-dl-ng download path: {error}"))));
                return;
            }
            let result = if use_bulk {
                let line_tx = tx.clone();
                tidal::download_playlist_via_tidal_dl_ng(&source_url, move |line| {
                    let _ = line_tx.send(TidalPlaylistDownloadUpdate::Log(line.to_string()));
                })
            } else {
                let mut succeeded = 0usize;
                let mut last_error = None;
                for (title, id) in &missing {
                    let Some(id) = id else { continue };
                    let url = tidal::track_url(id);
                    let _ = tx.send(TidalPlaylistDownloadUpdate::Log(format!("→ {title}")));
                    let line_tx = tx.clone();
                    match tidal::download_via_tidal_dl_ng(&url, move |line| {
                        let _ = line_tx.send(TidalPlaylistDownloadUpdate::Log(line.to_string()));
                    }) {
                        Ok(()) => succeeded += 1,
                        Err(error) => {
                            let _ = tx.send(TidalPlaylistDownloadUpdate::Log(format!("  failed: {error}")));
                            last_error = Some(error);
                        }
                    }
                }
                if succeeded == 0 { Err(last_error.unwrap_or_else(|| "no tracks downloaded".to_string())) } else { Ok(()) }
            };
            let _ = tx.send(TidalPlaylistDownloadUpdate::Done(result));
        });
        self.tidal_playlist_download_rx = Some(rx);
    }

    /// Call once per frame; non-blocking.
    pub fn poll_tidal_playlist_download(&mut self) {
        let Some(rx) = &self.tidal_playlist_download_rx else { return };
        let mut new_lines: Vec<String> = Vec::new();
        let mut done: Option<Result<(), String>> = None;
        loop {
            match rx.try_recv() {
                Ok(TidalPlaylistDownloadUpdate::Log(line)) => new_lines.push(line),
                Ok(TidalPlaylistDownloadUpdate::Done(result)) => {
                    done = Some(result);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.tidal_playlist_download_rx = None;
                    self.message = "Download thread crashed unexpectedly.".into();
                    return;
                }
            }
        }
        if !new_lines.is_empty() {
            let folder_name = self.tracks.as_ref().map(|view| view.playlist.name.clone()).unwrap_or_default();
            if let Some(log) = &mut self.tidal_log {
                for line in new_lines {
                    log.push(folder_name.clone(), line);
                }
            }
        }
        let Some(result) = done else { return };
        self.tidal_playlist_download_rx = None;
        match result {
            Ok(()) => {
                self.message = "Downloaded playlist via tidal-dl-ng.".into();
                let names = self.tracks.as_ref().map(|view| (view.crate_name.clone(), view.playlist.name.clone()));
                if let Some((crate_name, playlist_name)) = names {
                    if let Some(crate_location) = self.crates.iter_mut().find(|crate_location| crate_location.name == crate_name) {
                        let scanned = sync::scan_crate_playlists(crate_location);
                        crate_location.playlists = merge_playlists(&crate_location.playlists, scanned);
                    }
                    let _ = self.save_config();

                    let refreshed = self.crates.iter().find(|crate_location| crate_location.name == crate_name).and_then(|crate_location| {
                        let tracks = sync::list_playlist_tracks(crate_location, &playlist_name);
                        crate_location.playlists.iter().find(|playlist| playlist.name == playlist_name).map(|playlist| (playlist.clone(), tracks))
                    });

                    if let Some((playlist, tracks)) = refreshed {
                        self.message = format!("Downloaded via tidal-dl-ng — now {}/{} tracks synced.", playlist.synced, playlist.track_count);
                        let tidal_status = vec![None; tracks.len()];
                        if let Some(view) = &mut self.tracks {
                            view.playlist = playlist;
                            view.tracks = tracks;
                            view.tidal_status = tidal_status;
                            view.selected = 0;
                        }
                    }
                    self.refresh_selected_track();
                }
                // Close the log automatically once the download finishes cleanly — nothing left
                // to watch. Left open on failure so the error/what-happened trail stays visible
                // instead of vanishing the moment something goes wrong.
                self.tidal_log = None;
            }
            Err(error) => self.message = format!("Tidal playlist download failed: {error}"),
        }
    }

    fn start_soundcloud_download(&mut self) {
        let Some(view) = &self.tracks else { return };
        let Some(link) = view.playlist.link.as_ref() else {
            self.message = "This playlist isn't linked to SoundCloud yet. Use i to import/link one.".into();
            return;
        };
        let Some(source_url) = link.source_url.clone() else {
            self.message = "This playlist's SoundCloud link is missing — re-link it via i.".into();
            return;
        };
        if self.soundcloud_download_rx.is_some() {
            self.message = "Already downloading.".into();
            return;
        }
        let Some(primary_path) = self.crates.iter().find(|crate_location| crate_location.name == view.crate_name).and_then(|crate_location| crate_location.locations.first()).map(|location| location.path.clone()) else {
            self.message = "That crate has no paths yet.".into();
            return;
        };
        let folder_name = view.playlist.name.clone();

        self.message = "Downloading tracks from SoundCloud… (up to 2 min — Esc to cancel)".into();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(soundcloud::download_playlist(&source_url, &primary_path, Some(&folder_name)));
        });
        self.soundcloud_download_rx = Some(rx);
    }

    /// Call once per frame; non-blocking.
    pub fn poll_soundcloud_download(&mut self) {
        let Some(rx) = &self.soundcloud_download_rx else { return };
        match rx.try_recv() {
            Ok(Ok(result)) => {
                self.soundcloud_download_rx = None;
                let downloaded = result.tracks.len();
                self.message = if result.failed == 0 { format!("Downloaded {downloaded} track(s) from SoundCloud.") } else { format!("Downloaded {downloaded} track(s), {} failed (likely Go+ exclusive).", result.failed) };

                // Rescan so the crate's playlist reflects the newly downloaded files, and
                // refresh the open track view so the ☁ metadata-only markers go away.
                let names = self.tracks.as_ref().map(|view| (view.crate_name.clone(), view.playlist.name.clone()));
                if let Some((crate_name, playlist_name)) = names {
                    if let Some(crate_location) = self.crates.iter_mut().find(|crate_location| crate_location.name == crate_name) {
                        let scanned = sync::scan_crate_playlists(crate_location);
                        crate_location.playlists = merge_playlists(&crate_location.playlists, scanned);
                    }
                    let _ = self.save_config();

                    let refreshed = self.crates.iter().find(|crate_location| crate_location.name == crate_name).and_then(|crate_location| {
                        let tracks = sync::list_playlist_tracks(crate_location, &playlist_name);
                        crate_location.playlists.iter().find(|playlist| playlist.name == playlist_name).map(|playlist| (playlist.clone(), tracks))
                    });

                    if let Some((playlist, tracks)) = refreshed {
                        let tidal_status = vec![None; tracks.len()];
                        if let Some(view) = &mut self.tracks {
                            view.playlist = playlist;
                            view.tracks = tracks;
                            view.tidal_status = tidal_status;
                            view.selected = 0;
                        }
                    }
                    self.refresh_selected_track();
                }
            }
            Ok(Err(error)) => {
                self.soundcloud_download_rx = None;
                self.message = format!("SoundCloud download failed: {error}");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.soundcloud_download_rx = None;
                self.message = "Download thread crashed unexpectedly.".into();
            }
        }
    }

    /// For a Spotify-linked playlist (metadata only — Spotify's API has no downloadable
    /// stream): finds each track on Tidal's catalog, then hands the matches off to the
    /// externally-installed `tidal-dl-ng` (which needs its own real Tidal login) to download
    /// them straight into the playlist's folder.
    fn start_spotify_tidal_download(&mut self) {
        let Some(view) = &self.tracks else { return };
        let (Some(client_id), Some(client_secret)) = (self.tidal_client_id.clone(), self.tidal_client_secret.clone()) else {
            self.message = "Set your Tidal Client ID and Client Secret in Settings (s) first.".into();
            return;
        };
        let country_code = self.tidal_country_code();
        if self.spotify_tidal_download_rx.is_some() {
            self.message = "Already downloading.".into();
            return;
        }
        let Some(primary_path) = self.crates.iter().find(|crate_location| crate_location.name == view.crate_name).and_then(|crate_location| crate_location.locations.first()).map(|location| location.path.clone()) else {
            self.message = "That crate has no paths yet.".into();
            return;
        };
        let folder_name = view.playlist.name.clone();
        // Only tracks that don't already have a local file need to be found/downloaded.
        let remaining: Vec<(String, String)> = view.tracks.iter().filter(|track| track.remote_metadata.is_some()).map(|track| (track.name.clone(), track.remote_metadata.as_ref().map(|remote| remote.artist.clone()).unwrap_or_default())).collect();
        if remaining.is_empty() {
            self.message = "Nothing left to download — every track already has a local file.".into();
            return;
        }
        // Capped per run so one press of D doesn't hammer the Tidal API for hours unattended —
        // press D again afterward to pick up the next batch of un-synced tracks.
        let has_more = remaining.len() > TIDAL_DOWNLOAD_BATCH_LIMIT;
        let pairs: Vec<(String, String)> = remaining.into_iter().take(TIDAL_DOWNLOAD_BATCH_LIMIT).collect();

        self.message = format!("Searching Tidal and downloading via tidal-dl-ng… (Esc to cancel){}", if has_more { format!(" — first {TIDAL_DOWNLOAD_BATCH_LIMIT}, press D again for more") } else { String::new() });
        // Keep prior batches' log entries around (don't wipe them) so pressing D again to work
        // through the next chunk still lets you scroll back through everything that happened —
        // it just keeps appending, no separator needed.
        self.tidal_log.get_or_insert_with(DownloadLog::new);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dest_dir = std::path::Path::new(&primary_path).join(&folder_name);
            let total = pairs.len();
            let _ = tx.send(TidalDownloadUpdate::Phase(LogPhase::Finding));
            let _ = tx.send(TidalDownloadUpdate::Log { title: "batch".to_string(), line: format!("finding {total} track(s)…") });
            let progress_tx = tx.clone();
            let log_tx = tx.clone();
            let result = tidal::find_track_urls(
                &client_id,
                &client_secret,
                &country_code,
                &pairs,
                |index, title| {
                    let _ = progress_tx.send(TidalDownloadUpdate::Progress { index, total, title: title.to_string() });
                },
                |title, line| {
                    let _ = log_tx.send(TidalDownloadUpdate::Log { title: title.to_string(), line: line.to_string() });
                },
            )
            .map(|matches| {
                let found = matches.iter().filter(|url| url.is_some()).count();
                let _ = tx.send(TidalDownloadUpdate::Phase(LogPhase::Downloading));
                let _ = tx.send(TidalDownloadUpdate::Log { title: "batch".to_string(), line: format!("{found} found — downloading via tidal-dl-ng…") });
                {
                    let mut cfg_log = |line: &str| { let _ = tx.send(TidalDownloadUpdate::Log { title: "batch".to_string(), line: line.to_string() }); };
                    if let Err(error) = tidal::set_tidal_dl_ng_download_path(&dest_dir, &mut cfg_log) {
                        cfg_log(&format!("could not set tidal-dl-ng download path: {error}"));
                    }
                }
                let mut downloaded = 0usize;
                let mut log = Vec::with_capacity(pairs.len());
                for ((title, _artist), maybe_url) in pairs.iter().zip(matches) {
                    match maybe_url {
                        Some(url) => {
                            let line_tx = tx.clone();
                            let outcome = tidal::download_via_tidal_dl_ng(&url, |line| {
                                let _ = line_tx.send(TidalDownloadUpdate::Log { title: title.clone(), line: line.to_string() });
                            });
                            let status = match outcome {
                                Ok(()) => {
                                    downloaded += 1;
                                    // The manifest is the only thing driving "still needs
                                    // downloading" — clear the entry now that a real file
                                    // exists, instead of relying on filename-matching it later.
                                    let _ = sync::remove_manifest_entry(&dest_dir, title);
                                    "downloaded".to_string()
                                }
                                Err(error) => format!("download failed: {error}"),
                            };
                            let _ = tx.send(TidalDownloadUpdate::Log { title: title.clone(), line: status.clone() });
                            log.push((title.clone(), status));
                        }
                        None => {
                            let _ = tx.send(TidalDownloadUpdate::Log { title: title.clone(), line: "not found on Tidal".to_string() });
                            log.push((title.clone(), "not found on Tidal".to_string()));
                        }
                    }
                }
                TidalDownloadOutcome { downloaded, log, has_more }
            });
            let _ = tx.send(TidalDownloadUpdate::Done(result));
        });
        self.spotify_tidal_download_rx = Some(rx);
    }

    /// Call once per frame; non-blocking.
    pub fn poll_spotify_tidal_download(&mut self) {
        let Some(rx) = &self.spotify_tidal_download_rx else { return };
        // Drain every update queued since the last frame so we always end up showing the most
        // recent progress, not one that's several tracks stale.
        let mut latest_progress: Option<(usize, usize, String)> = None;
        // Order matters here (a Phase switch must apply before the Log lines that follow it), so
        // collect both kinds into one ordered list rather than draining them into separate bins.
        enum LogEvent {
            Phase(LogPhase),
            Line(String, String),
        }
        let mut new_log_events: Vec<LogEvent> = Vec::new();
        let mut done: Option<Result<TidalDownloadOutcome, String>> = None;
        loop {
            match rx.try_recv() {
                Ok(TidalDownloadUpdate::Progress { index, total, title }) => latest_progress = Some((index, total, title)),
                Ok(TidalDownloadUpdate::Log { title, line }) => new_log_events.push(LogEvent::Line(title, line)),
                Ok(TidalDownloadUpdate::Phase(phase)) => new_log_events.push(LogEvent::Phase(phase)),
                Ok(TidalDownloadUpdate::Done(result)) => {
                    done = Some(result);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.spotify_tidal_download_rx = None;
                    self.message = "Download thread crashed unexpectedly.".into();
                    return;
                }
            }
        }
        if let Some((index, total, title)) = latest_progress {
            self.message = format!("→ ({index}/{total}) {title}… (Esc to cancel)");
        }
        if !new_log_events.is_empty() {
            let log = self.tidal_log.get_or_insert_with(DownloadLog::new);
            for event in new_log_events {
                match event {
                    LogEvent::Phase(phase) => {
                        log.current_phase = phase;
                        log.focused_pane = phase;
                    }
                    LogEvent::Line(title, line) => log.push(title, line),
                }
            }
        }
        match done {
            Some(Ok(TidalDownloadOutcome { downloaded, log, has_more })) => {
                self.spotify_tidal_download_rx = None;
                let failed = log.len() - downloaded;
                let more_suffix = if has_more { " Press D again for the next batch." } else { "" };
                self.message = if failed == 0 { format!("Downloaded {downloaded} track(s) via Tidal.{more_suffix}") } else { format!("Downloaded {downloaded} track(s) via Tidal, {failed} not found or failed.{more_suffix}") };
                self.rescan_current_playlist();
                self.tidal_log = None;
            }
            Some(Err(error)) => {
                self.spotify_tidal_download_rx = None;
                self.message = format!("Tidal download failed: {error}");
            }
            None => {}
        }
    }

    /// Re-scans the crate's paths and re-reads the currently open playlist's track list, so
    /// synced counts and the track view reflect whatever's actually on disk right now.
    fn rescan_current_playlist(&mut self) {
        let names = self.tracks.as_ref().map(|view| (view.crate_name.clone(), view.playlist.name.clone()));
        let Some((crate_name, playlist_name)) = names else { return };

        if let Some(crate_location) = self.crates.iter_mut().find(|crate_location| crate_location.name == crate_name) {
            let scanned = sync::scan_crate_playlists(crate_location);
            crate_location.playlists = merge_playlists(&crate_location.playlists, scanned);
        }
        let _ = self.save_config();

        let refreshed = self.crates.iter().find(|crate_location| crate_location.name == crate_name).and_then(|crate_location| {
            let tracks = sync::list_playlist_tracks(crate_location, &playlist_name);
            crate_location.playlists.iter().find(|playlist| playlist.name == playlist_name).map(|playlist| (playlist.clone(), tracks))
        });

        if let Some((playlist, tracks)) = refreshed {
            let tidal_status = vec![None; tracks.len()];
            if let Some(view) = &mut self.tracks {
                view.playlist = playlist;
                view.tracks = tracks;
                view.tidal_status = tidal_status;
                view.selected = 0;
            }
        }
        self.refresh_selected_track();
    }

    /// Secret: checks every track in the currently open playlist against Tidal's catalog
    /// (title + artist match), so you can see which ones aren't available there.
    fn start_tidal_search(&mut self) {
        let Some(view) = &self.tracks else { return };
        if view.tracks.is_empty() {
            return;
        }
        let (Some(client_id), Some(client_secret)) = (self.tidal_client_id.clone(), self.tidal_client_secret.clone()) else {
            self.message = "Set your Tidal Client ID and Client Secret in Settings (s) first.".into();
            return;
        };
        if self.tidal_search_rx.is_some() {
            self.message = "Already searching Tidal.".into();
            return;
        }

        let entries: Vec<(String, PathBuf, Option<String>)> = view.tracks.iter().map(|track| (track.name.clone(), track.path.clone(), track.remote_metadata.as_ref().map(|remote| remote.artist.clone()))).collect();
        let country_code = self.tidal_country_code();
        self.message = "Searching Tidal for these tracks…".into();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let pairs: Vec<(String, String)> = entries
                .into_iter()
                .map(|(title, path, remote_artist)| {
                    let artist = remote_artist.unwrap_or_else(|| metadata::read_track_metadata(&path).and_then(|metadata| metadata.artist).unwrap_or_default());
                    (title, artist)
                })
                .collect();
            let _ = tx.send(tidal::find_tracks(&client_id, &client_secret, &country_code, &pairs));
        });
        self.tidal_search_rx = Some(rx);
    }

    /// Call once per frame; non-blocking.
    pub fn poll_tidal_search(&mut self) {
        let Some(rx) = &self.tidal_search_rx else { return };
        match rx.try_recv() {
            Ok(Ok(results)) => {
                self.tidal_search_rx = None;
                let missing = results.iter().filter(|found| !**found).count();
                if let Some(view) = &mut self.tracks {
                    view.tidal_status = results.into_iter().map(Some).collect();
                }
                self.message = if missing == 0 { "All tracks found on Tidal.".into() } else { format!("{missing} track(s) not found on Tidal (marked ✕).") };
            }
            Ok(Err(error)) => {
                self.tidal_search_rx = None;
                self.message = format!("Tidal search failed: {error}");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.tidal_search_rx = None;
                self.message = "Tidal search thread crashed unexpectedly.".into();
            }
        }
    }

    fn handle_confirm_delete(&mut self, key: KeyCode, index: usize) {
        match key {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if index < self.crates.len() {
                    let removed = self.crates.remove(index);
                    self.selected_crate = self.selected_crate.min(self.crates.len().saturating_sub(1));
                    self.config_field = 0;
                    self.message = format!("Removed crate {}.", removed.name);
                    if let Err(error) = self.save_config() {
                        self.message = format!("Removed crate {}, but could not save config: {}", removed.name, error);
                    }
                }
                self.pending_delete = None;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.message = "Crate deletion cancelled.".into();
                self.pending_delete = None;
            }
            _ => {}
        }
    }

    fn handle_download_log_key(&mut self, key: KeyCode) {
        let Some(log) = &mut self.tidal_log else { return };
        match key {
            KeyCode::Esc => {
                self.tidal_log = None;
                self.rescan_current_playlist();
            }
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => log.scroll_down(),
            KeyCode::Up | KeyCode::Char('k') => log.scroll_up(),
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => log.toggle_focus(),
            _ => {}
        }
    }

    fn handle_tag_browser_key(&mut self, key: KeyCode) {
        let Some(browser) = &mut self.tag_browser else { return };
        if browser.filter.is_some() {
            match key {
                KeyCode::Esc | KeyCode::Char('t') => browser.filter = None,
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Down | KeyCode::Char('j') => {
                    if !browser.matches.is_empty() {
                        browser.match_selected = (browser.match_selected + 1) % browser.matches.len();
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if !browser.matches.is_empty() {
                        browser.match_selected = browser.match_selected.checked_sub(1).unwrap_or(browser.matches.len() - 1);
                    }
                }
                KeyCode::Enter => self.open_selected_match(),
                _ => {}
            }
            return;
        }
        match key {
            KeyCode::Esc => self.tag_browser = None,
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => browser.selected = (browser.selected + 1) % browser.tags.len(),
            KeyCode::Up | KeyCode::Char('k') => browser.selected = browser.selected.checked_sub(1).unwrap_or(browser.tags.len() - 1),
            KeyCode::Enter => {
                if let Some(tag) = browser.tags.get(browser.selected).cloned() {
                    let matches: Vec<(String, String)> = self
                        .crates
                        .iter()
                        .flat_map(|crate_location| crate_location.playlists.iter().map(move |playlist| (crate_location.name.clone(), playlist)))
                        .filter(|(_, playlist)| playlist.tags.iter().any(|t| t == &tag))
                        .map(|(crate_name, playlist)| (crate_name, playlist.name.clone()))
                        .collect();
                    browser.filter = Some(tag);
                    browser.matches = matches;
                    browser.match_selected = 0;
                }
            }
            _ => {}
        }
    }

    fn open_selected_match(&mut self) {
        let Some(browser) = &self.tag_browser else { return };
        let Some((crate_name, playlist_name)) = browser.matches.get(browser.match_selected).cloned() else { return };
        let Some(crate_location) = self.crates.iter().find(|crate_location| crate_location.name == crate_name) else { return };
        let Some(playlist) = crate_location.playlists.iter().find(|playlist| playlist.name == playlist_name) else { return };
        let tracks = sync::list_playlist_tracks(crate_location, &playlist_name);
        let tidal_status = vec![None; tracks.len()];
        self.tracks = Some(TrackView { crate_name, playlist: playlist.clone(), tracks, selected: 0, metadata: None, cover: None, tidal_status });
        self.tag_browser = None;
        self.refresh_selected_track();
    }

    fn handle_tag_edit_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.editing_tags = false;
                self.tag_buffer.clear();
                self.tag_cursor = 0;
                self.message = "Tag edit cancelled.".into();
            }
            KeyCode::Enter => self.save_tags(),
            KeyCode::Left => self.tag_cursor = self.tag_cursor.saturating_sub(1),
            KeyCode::Right => self.tag_cursor = (self.tag_cursor + 1).min(self.tag_buffer.chars().count()),
            KeyCode::Home => self.tag_cursor = 0,
            KeyCode::End => self.tag_cursor = self.tag_buffer.chars().count(),
            KeyCode::Backspace => {
                if self.tag_cursor > 0 {
                    let byte_index = char_byte_index(&self.tag_buffer, self.tag_cursor - 1);
                    self.tag_buffer.remove(byte_index);
                    self.tag_cursor -= 1;
                }
            }
            KeyCode::Delete => {
                if self.tag_cursor < self.tag_buffer.chars().count() {
                    let byte_index = char_byte_index(&self.tag_buffer, self.tag_cursor);
                    self.tag_buffer.remove(byte_index);
                }
            }
            KeyCode::Char(character) => {
                let byte_index = char_byte_index(&self.tag_buffer, self.tag_cursor);
                self.tag_buffer.insert(byte_index, character);
                self.tag_cursor += 1;
            }
            _ => {}
        }
    }

    fn save_tags(&mut self) {
        let tags: Vec<String> = self.tag_buffer.split(',').map(|tag| tag.trim().to_string()).filter(|tag| !tag.is_empty()).collect();
        let Some(crate_location) = self.crates.get_mut(self.selected_crate) else {
            self.editing_tags = false;
            return;
        };
        if let Some(playlist) = crate_location.playlists.get_mut(self.selected_playlist) {
            playlist.tags = tags;
        }
        self.editing_tags = false;
        self.tag_buffer.clear();
        self.tag_cursor = 0;
        self.message = match self.save_config() {
            Ok(()) => "Tags saved.".into(),
            Err(error) => format!("Tags updated for this session, but could not save config: {}", error),
        };
    }

    fn handle_config_key(&mut self, key: KeyCode) {
        if self.editing_config {
            match key {
                KeyCode::Esc => {
                    self.editing_config = false;
                    self.config_buffer.clear();
                    self.config_cursor = 0;
                }
                KeyCode::Enter => self.save_config_field(),
                KeyCode::Left => self.config_cursor = self.config_cursor.saturating_sub(1),
                KeyCode::Right => self.config_cursor = (self.config_cursor + 1).min(self.config_buffer.chars().count()),
                KeyCode::Home => self.config_cursor = 0,
                KeyCode::End => self.config_cursor = self.config_buffer.chars().count(),
                KeyCode::Backspace => {
                    if self.config_cursor > 0 {
                        let byte_index = char_byte_index(&self.config_buffer, self.config_cursor - 1);
                        self.config_buffer.remove(byte_index);
                        self.config_cursor -= 1;
                    }
                }
                KeyCode::Delete => {
                    if self.config_cursor < self.config_buffer.chars().count() {
                        let byte_index = char_byte_index(&self.config_buffer, self.config_cursor);
                        self.config_buffer.remove(byte_index);
                    }
                }
                KeyCode::Char(character) => {
                    let byte_index = char_byte_index(&self.config_buffer, self.config_cursor);
                    self.config_buffer.insert(byte_index, character);
                    self.config_cursor += 1;
                }
                _ => {}
            }
            return;
        }

        match key {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc | KeyCode::Char('c') => {
                self.screen = Screen::Dashboard;
                self.message = "Back at the crate desk.".into();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.config_field = self.config_field.checked_sub(1).unwrap_or(self.field_count().saturating_sub(1));
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.config_field = (self.config_field + 1) % self.field_count();
            }
            KeyCode::Left | KeyCode::Char('h') if !self.crates.is_empty() => {
                self.selected_crate = self.selected_crate.checked_sub(1).unwrap_or(self.crates.len() - 1);
                self.config_field = self.config_field.min(self.field_count().saturating_sub(1));
            }
            KeyCode::Right | KeyCode::Char('l') if !self.crates.is_empty() => {
                self.selected_crate = (self.selected_crate + 1) % self.crates.len();
                self.config_field = self.config_field.min(self.field_count().saturating_sub(1));
            }
            KeyCode::Char('a') if !self.crates.is_empty() => {
                self.crates[self.selected_crate].locations.push(Location { path: String::new(), removable: false });
                self.config_field = self.crates[self.selected_crate].locations.len();
                self.message = "New path added. Type it in, then Enter to save.".into();
            }
            KeyCode::Char('x') if !self.crates.is_empty() && self.config_field > 0 => {
                let locations = &mut self.crates[self.selected_crate].locations;
                if locations.len() > 1 {
                    locations.remove(self.config_field - 1);
                    self.config_field = self.config_field.min(locations.len());
                    self.message = "Path removed.".into();
                } else {
                    self.message = "A crate needs at least one path.".into();
                }
            }
            KeyCode::Char('e') if !self.crates.is_empty() && self.config_field > 0 => {
                let field = self.config_field;
                if let Some(location) = self.crates[self.selected_crate].locations.get_mut(field - 1) {
                    location.removable = !location.removable;
                    let state = if location.removable { "external drive" } else { "regular path" };
                    self.message = format!("Marked as {state}.");
                }
                self.refresh_availability();
                let _ = self.save_config();
            }
            KeyCode::Char('n') => {
                self.crates.push(CrateLocation { name: "New crate".into(), locations: vec![Location { path: String::new(), removable: false }], available: true, playlists: vec![] });
                self.selected_crate = self.crates.len() - 1;
                self.config_field = 0;
                self.message = "New crate added. Name it, then Enter to save.".into();
            }
            KeyCode::Char('X') if !self.crates.is_empty() => {
                self.pending_delete = Some(self.selected_crate);
            }
            KeyCode::Enter if !self.crates.is_empty() => {
                self.config_buffer = self.config_field_value();
                self.config_cursor = self.config_buffer.chars().count();
                self.editing_config = true;
            }
            _ => {}
        }
    }

    fn field_count(&self) -> usize {
        self.crates.get(self.selected_crate).map_or(1, |crate_location| 1 + crate_location.locations.len())
    }

    pub fn config_field_value(&self) -> String {
        self.config_value(self.config_field)
    }

    pub fn config_value(&self, field: usize) -> String {
        let Some(crate_location) = self.crates.get(self.selected_crate) else {
            return String::new();
        };
        if field == 0 {
            return crate_location.name.clone();
        }
        crate_location.locations.get(field - 1).map_or_else(String::new, |location| location.path.clone())
    }

    fn save_config_field(&mut self) {
        let value = std::mem::take(&mut self.config_buffer);
        let Some(crate_location) = self.crates.get_mut(self.selected_crate) else {
            self.editing_config = false;
            return;
        };
        let mut path_not_found: Option<String> = None;
        let path_changed = self.config_field > 0;
        if self.config_field == 0 {
            crate_location.name = value;
        } else if let Some(location) = crate_location.locations.get_mut(self.config_field - 1) {
            location.path = value;
            if !std::path::Path::new(&location.path).exists() {
                path_not_found = Some(location.path.clone());
            }
        }
        self.refresh_availability();
        if path_changed {
            if let Some(crate_location) = self.crates.get_mut(self.selected_crate) {
                let scanned = sync::scan_crate_playlists(crate_location);
                crate_location.playlists = merge_playlists(&crate_location.playlists, scanned);
            }
            self.selected_playlist = self.selected_playlist.min(self.current_playlists().len().saturating_sub(1));
        }
        self.editing_config = false;
        self.message = match (self.save_config(), path_not_found) {
            (Ok(()), Some(path)) => format!("Saved, but that path doesn't exist: {path}"),
            (Ok(()), None) if path_changed => "Saved. Playlists scanned from that path's folders.".into(),
            (Ok(()), None) => format!("Saved. Your crate map is looking lovely. ({})", config::display_path()),
            (Err(error), _) => format!("Updated for this session, but could not save config: {}", error),
        };
    }

    fn refresh_availability(&mut self) {
        for crate_location in &mut self.crates {
            crate_location.available = crate_location.locations.iter().all(|location| std::path::Path::new(&location.path).exists());
        }
    }

    fn rescan_playlists(&mut self) {
        for crate_location in &mut self.crates {
            let scanned = sync::scan_crate_playlists(crate_location);
            crate_location.playlists = merge_playlists(&crate_location.playlists, scanned);
        }
    }
}

/// Applies a fresh disk scan on top of what we already knew, keeping user-entered
/// metadata (tags, service links) for playlists that still exist.
fn merge_playlists(existing: &[Playlist], scanned: Vec<Playlist>) -> Vec<Playlist> {
    scanned
        .into_iter()
        .map(|mut playlist| {
            if let Some(previous) = existing.iter().find(|previous| previous.name.eq_ignore_ascii_case(&playlist.name)) {
                playlist.tags = previous.tags.clone();
                playlist.link = previous.link.clone();
            }
            playlist
        })
        .collect()
}

fn wrap_index(current: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let signed_len = len as i32;
    (((current as i32 + delta) % signed_len + signed_len) % signed_len) as usize
}

fn char_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices().nth(char_index).map_or(text.len(), |(byte_index, _)| byte_index)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Config,
    Import,
    Settings,
}

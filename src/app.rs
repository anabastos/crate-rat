use crossterm::event::KeyCode;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;

use crate::config;
use crate::metadata::{self, TrackMetadata};
use crate::model::{CrateLocation, ImportService, Location, Playlist, PlaylistLink};
use crate::sync;
use crate::sync::TrackFile;

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
    pub import: Option<ImportState>,
    pub tracks: Option<TrackView>,
    picker: Option<Picker>,
}

pub struct TrackView {
    pub crate_name: String,
    pub playlist: Playlist,
    pub tracks: Vec<TrackFile>,
    pub selected: usize,
    pub metadata: Option<TrackMetadata>,
    pub cover: Option<Box<dyn Protocol>>,
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
            import: None,
            tracks: None,
            picker: None,
        }
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
        let metadata = metadata::read_track_metadata(&track.path);
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
                app.crates = loaded.crates;
                app.message = "Welcome back. Your crate map is loaded.".into();
            }
            Ok(Some(_)) => app.message = "No crates yet. Press c, then n to add one.".into(),
            Ok(None) => app.message = format!("No config yet. Press c, then n to add a crate. ({})", config::display_path()),
            Err(error) => app.message = format!("Could not load config: {}. Starting with no crates.", error),
        }
        app.refresh_availability();
        app.rescan_playlists();
        let _ = config::save(&app.crates);
        app
    }

    fn current_playlists(&self) -> &[Playlist] {
        self.crates.get(self.selected_crate).map_or(&[], |crate_location| crate_location.playlists.as_slice())
    }

    pub fn handle_key(&mut self, key: KeyCode) {
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
                }
            }
            KeyCode::Char('r') => {
                self.refresh_availability();
                self.rescan_playlists();
                self.selected_playlist = self.selected_playlist.min(self.current_playlists().len().saturating_sub(1));
                let missing: Vec<&str> = self.crates.iter().filter(|crate_location| !crate_location.available).map(|crate_location| crate_location.name.as_str()).collect();
                let save_note = if config::save(&self.crates).is_ok() { "" } else { " (could not save)" };
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
                self.tracks = Some(TrackView { crate_name: crate_location.name.clone(), playlist: playlist.clone(), tracks, selected: 0, metadata: None, cover: None });
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
        match key {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc | KeyCode::Char('s') => {
                self.screen = Screen::Dashboard;
            }
            _ => {}
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
        if current.name_buffer.trim().is_empty() {
            self.message = "Playlist name can't be empty.".into();
            return;
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
                    playlist.link = Some(PlaylistLink { service, external_name: external_name.clone() });
                }
            }
            self.message = match config::save(&self.crates) {
                Ok(()) => format!("Created \"{external_name}\" and linked it to {}.", service.label()),
                Err(error) => format!("Created the playlist, but could not save config: {error}"),
            };
        } else {
            let playlist_index = import.playlist_index;
            if let Some(crate_location) = self.crates.get_mut(crate_index) {
                if let Some(playlist) = crate_location.playlists.get_mut(playlist_index) {
                    playlist.link = Some(PlaylistLink { service, external_name: external_name.clone() });
                }
            }
            self.message = match config::save(&self.crates) {
                Ok(()) => format!("Linked to {} playlist \"{external_name}\".", service.label()),
                Err(error) => format!("Linked for this session, but could not save config: {error}"),
            };
        }
        self.screen = Screen::Dashboard;
    }

    fn handle_tracks_key(&mut self, key: KeyCode) {
        let Some(view) = &mut self.tracks else { return };
        match key {
            KeyCode::Esc | KeyCode::Enter => self.tracks = None,
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
            _ => {}
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
                    if let Err(error) = config::save(&self.crates) {
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
        self.tracks = Some(TrackView { crate_name, playlist: playlist.clone(), tracks, selected: 0, metadata: None, cover: None });
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
        self.message = match config::save(&self.crates) {
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
                let _ = config::save(&self.crates);
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
        self.message = match (config::save(&self.crates), path_not_found) {
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

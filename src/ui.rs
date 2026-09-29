use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Padding, Paragraph, Row, Table, TableState},
    Frame,
};

use crate::app::{Activity, App, LibraryRow, Screen, TagState};

const INK: Color = Color::Rgb(245, 255, 255);
const CYAN: Color = Color::Rgb(0, 245, 255);
const PINK: Color = Color::Rgb(255, 0, 170);
const LIME: Color = Color::Rgb(130, 255, 0);
const YELLOW: Color = Color::Rgb(255, 235, 0);
const BLUE: Color = Color::Rgb(90, 120, 255);
const PANEL: Color = Color::Rgb(3, 3, 8);
const BACKDROP: Color = Color::Black;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BACKDROP)), area);
    if let Some(view) = &app.tracks {
        draw_playlist_page(frame, app, view, area);
        if let Some(index) = app.pending_delete {
            draw_confirm_delete(frame, app, index, area);
        }
        if let Some(log) = &app.tidal_log {
            draw_download_log(frame, log, area);
        }
        return;
    }
    match app.screen {
        Screen::Library => draw_library(frame, app, area),
        Screen::Config => draw_config(frame, app, area),
        Screen::Import => draw_import(frame, app, area),
        Screen::Settings => draw_settings(frame, app, area),
        Screen::Dashboard => {
            let vertical = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(5), Constraint::Min(8), Constraint::Length(2), Constraint::Length(2)]).split(area);
            draw_header(frame, app, vertical[0]);
            let body = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(38), Constraint::Percentage(62)]).split(vertical[1]);
            draw_crates(frame, app, body[0]);
            draw_playlists(frame, app, body[1]);
            draw_status(frame, app, vertical[2]);
            frame.render_widget(Paragraph::new(Line::from(vec![
                pill("q", BACKDROP, PINK),
                Span::styled(" quit   ", Style::default().fg(CYAN)),
                pill("j/k", PANEL, INK),
                Span::styled(" navigate   ", Style::default().fg(CYAN)),
                pill("tab", PANEL, INK),
                Span::styled(" change crate   ", Style::default().fg(CYAN)),
                pill("c", PANEL, INK),
                Span::styled(" crates   ", Style::default().fg(CYAN)),
                pill("n", PANEL, INK),
                Span::styled(" new crate   ", Style::default().fg(CYAN)),
                pill("Enter", PANEL, INK),
                Span::styled(" view tracks   ", Style::default().fg(CYAN)),
                pill("b", PANEL, INK),
                Span::styled(" library   ", Style::default().fg(CYAN)),
                pill("i", PANEL, INK),
                Span::styled(" import   ", Style::default().fg(CYAN)),
                pill("t", PANEL, INK),
                Span::styled(" browse tags   ", Style::default().fg(CYAN)),
                pill("T", PANEL, INK),
                Span::styled(" edit tags   ", Style::default().fg(CYAN)),
                pill("s", PANEL, INK),
                Span::styled(" settings", Style::default().fg(CYAN)),
            ])), vertical[3]);
        }
    }
    if let Some(index) = app.pending_delete {
        draw_confirm_delete(frame, app, index, area);
    }
    if let Some(browser) = &app.tag_browser {
        draw_tag_browser(frame, browser, area);
    }
    if let Some(log) = &app.tidal_log {
        draw_download_log(frame, log, area);
    }
}

fn draw_download_log(frame: &mut Frame, log: &crate::app::DownloadLog, area: Rect) {
    let popup = centered_rect(86, 76, area);
    frame.render_widget(Clear, popup);

    let sections = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(3), Constraint::Length(1)]).split(popup);

    if log.single_pane {
        draw_log_pane(frame, "DOWNLOADING TRACKS", &log.downloading, log.downloading_selected, true, sections[0]);
    } else {
        let panes = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(50), Constraint::Percentage(50)]).split(sections[0]);
        let finding_focused = log.focused_pane == crate::app::LogPhase::Finding;
        draw_log_pane(frame, "FINDING TRACKS", &log.finding, log.finding_selected, finding_focused, panes[0]);
        draw_log_pane(frame, "DOWNLOADING TRACKS", &log.downloading, log.downloading_selected, !finding_focused, panes[1]);
    }

    let mut footer = vec![pill("j/k", PANEL, INK), Span::styled(" scroll   ", Style::default().fg(CYAN))];
    if !log.single_pane {
        footer.push(pill("Tab", PANEL, INK));
        footer.push(Span::styled(" switch pane   ", Style::default().fg(CYAN)));
    }
    footer.push(pill("Esc", PANEL, INK));
    footer.push(Span::styled(" close", Style::default().fg(CYAN)));
    frame.render_widget(Paragraph::new(Line::from(footer)).style(Style::default().bg(PANEL)), sections[1]);
}

fn draw_log_pane(frame: &mut Frame, title_label: &str, entries: &[(String, String)], selected: usize, focused: bool, area: Rect) {
    let border_color = if focused { LIME } else { BLUE };
    let block = Block::default()
        .title(Span::styled(format!(" ♫ {title_label} ({}) ♫ ", entries.len()), Style::default().fg(PANEL).bg(border_color).add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(1))
        .style(Style::default().bg(PANEL));

    if entries.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled("  (nothing logged yet)", Style::default().fg(BLUE))).block(block), area);
        return;
    }

    // borders (2) + horizontal padding (2)
    let inner_width = area.width.saturating_sub(4) as usize;
    let items: Vec<ListItem> = entries.iter().enumerate().map(|(index, (title, outcome))| {
        let row_selected = index == selected;
        let prefix = if row_selected { "› " } else { "  " };
        if title == "batch" {
            let lines: Vec<Line> = wrap_text(&format!("{prefix}▸ {outcome}"), inner_width).into_iter().map(|chunk| Line::from(Span::styled(chunk, Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)))).collect();
            return ListItem::new(lines);
        }
        let outcome_color = if outcome == "downloaded" { LIME } else { PINK };
        let mut lines: Vec<Line> = wrap_text(&format!("{prefix}{title}"), inner_width).into_iter().map(|chunk| Line::from(Span::styled(chunk, Style::default().fg(if row_selected { INK } else { CYAN })))).collect();
        for chunk in wrap_text(outcome, inner_width.saturating_sub(4)) {
            lines.push(Line::from(Span::styled(format!("    {chunk}"), Style::default().fg(outcome_color))));
        }
        ListItem::new(lines)
    }).collect();
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(List::new(items).block(block).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK).add_modifier(Modifier::BOLD)), area, &mut state);
}

fn draw_playlist_page(frame: &mut Frame, app: &App, view: &crate::app::TrackView, area: Rect) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Length(6), Constraint::Length(1), Constraint::Min(6), Constraint::Length(2)])
        .split(area);
    draw_header(frame, app, vertical[0]);

    let playlist = &view.playlist;
    let status_color = if playlist.link.is_some() && playlist.track_count == 0 { PINK } else if playlist.synced == playlist.track_count { LIME } else { PINK };
    let mut tag_spans = vec![Span::styled("tags: ", Style::default().fg(YELLOW))];
    tag_spans.extend(tag_spans_for(&playlist.tags, &playlist.auto_tags));
    let link_line = playlist.link.as_ref().map_or("—".to_string(), |link| format!("{} · \"{}\"", link.service.label(), link.external_name));

    let details = vec![
        Line::from(vec![
            Span::styled("♫ ", Style::default().fg(LIME)),
            Span::styled(playlist.name.clone(), Style::default().fg(INK).add_modifier(Modifier::BOLD)),
            Span::styled(format!("   [{}]", view.crate_name), Style::default().fg(CYAN)),
        ]),
        Line::from(vec![
            Span::styled("status: ", Style::default().fg(YELLOW)),
            Span::styled(format!("{} ({}/{} tracks)", playlist.status(), playlist.synced, playlist.track_count), Style::default().fg(status_color)),
        ]),
        Line::from(tag_spans),
        Line::from(vec![Span::styled("linked service: ", Style::default().fg(YELLOW)), Span::styled(link_line, Style::default().fg(INK))]),
    ];
    frame.render_widget(Paragraph::new(details).block(panel("♡ PLAYLIST DETAILS ♡")), vertical[1]);
    frame.render_widget(Paragraph::new(Span::styled(format!("  {}", app.message), Style::default().fg(YELLOW))), vertical[2]);

    let track_block = panel("♫ TRACKS ♫");
    if view.tracks.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled("  No track files found in this playlist's folder.", Style::default().fg(INK))).block(track_block), vertical[3]);
    } else {
        let columns = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(55), Constraint::Percentage(45)]).split(vertical[3]);
        let items: Vec<ListItem> = view.tracks.iter().enumerate().map(|(index, track)| {
            let selected = index == view.selected;
            let prefix = if selected { "› " } else { "  " };
            let track_id = view.track_ids.get(index).map(String::as_str).unwrap_or("");
            let is_playing = app.now_playing.as_ref().is_some_and(|path| {
                if track.remote_metadata.is_none() { *path == track.path } else { app.library.files.get(&path.to_string_lossy().to_string()).is_some_and(|entry| entry.track_id == track_id) }
            });
            let playing_marker = if is_playing {
                if app.audio_paused { "⏸ " } else { "▶ " }
            } else {
                ""
            };
            let in_playlists = app.playlists_for(track_id).len();
            let name_color = if track.remote_metadata.is_some() { BLUE } else if selected { INK } else { CYAN };
            let tidal_marker = match view.tidal_status.get(index).copied().flatten() {
                Some(true) => Span::styled("  ✓ tidal", Style::default().fg(LIME)),
                Some(false) => Span::styled("  ✕ tidal", Style::default().fg(PINK)),
                None => Span::raw(""),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{prefix}{}. {playing_marker}", index + 1), Style::default().fg(if is_playing { LIME } else { BLUE })),
                Span::styled(track.name.clone(), Style::default().fg(name_color)),
                Span::styled(if track.remote_metadata.is_some() { "  ☁" } else { "" }, Style::default().fg(YELLOW)),
                tidal_marker,
                tag_state_span(app.tag_state(track_id)),
                Span::styled(if in_playlists > 1 { format!("  ×{in_playlists}") } else { String::new() }, Style::default().fg(PINK)),
            ]))
        }).collect();
        let mut state = ListState::default();
        state.select(Some(view.selected));
        frame.render_stateful_widget(List::new(items).block(track_block).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK).add_modifier(Modifier::BOLD)), columns[0], &mut state);
        draw_track_details(frame, app, view, columns[1]);
    }

    let link_service = view.playlist.link.as_ref().map(|link| link.service);
    let mut footer = vec![
        pill("Esc", BACKDROP, PINK),
        Span::styled(" back   ", Style::default().fg(CYAN)),
        pill("j/k", PANEL, INK),
        Span::styled(" scroll   ", Style::default().fg(CYAN)),
        pill("Enter / p", PANEL, INK),
        Span::styled(" play / pause   ", Style::default().fg(CYAN)),
        pill("x", PANEL, INK),
        Span::styled(" stop   ", Style::default().fg(CYAN)),
        pill("g", PANEL, INK),
        Span::styled(" re-fetch tags", Style::default().fg(CYAN)),
    ];
    if let Some(label) = match link_service {
        Some(crate::model::ImportService::SoundCloud) => Some("download audio"),
        Some(crate::model::ImportService::Spotify) => Some("find + download via Tidal"),
        Some(crate::model::ImportService::Tidal) => Some("download via tidal-dl-ng"),
        _ => None,
    } {
        footer.push(Span::styled("   ", Style::default()));
        footer.push(pill("D", PANEL, INK));
        footer.push(Span::styled(format!(" {label}"), Style::default().fg(CYAN)));
    }
    if link_service == Some(crate::model::ImportService::Tidal) {
        footer.push(Span::styled("   ", Style::default()));
        footer.push(pill("R", PANEL, INK));
        footer.push(Span::styled(" refresh from Tidal", Style::default().fg(CYAN)));
    }
    frame.render_widget(Paragraph::new(Line::from(footer)), vertical[4]);
}

fn draw_track_details(frame: &mut Frame, app: &App, view: &crate::app::TrackView, area: Rect) {
    let block = panel("♡ TRACK DETAILS ♡");
    let Some(track) = view.tracks.get(view.selected) else {
        frame.render_widget(Paragraph::new("").block(block), area);
        return;
    };

    let has_cover = view.cover.is_some();
    let sections = if has_cover {
        Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(9), Constraint::Min(6)]).split(block.inner(area))
    } else {
        Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(6)]).split(block.inner(area))
    };
    frame.render_widget(block, area);

    let text_area = if has_cover {
        if let Some(cover) = &view.cover {
            frame.render_widget(ratatui_image::Image::new(cover.as_ref()), sections[0]);
        }
        sections[1]
    } else {
        sections[0]
    };

    let is_remote = track.remote_metadata.is_some();
    let extension = std::path::Path::new(&track.name).extension().and_then(|extension| extension.to_str()).unwrap_or("—").to_uppercase();
    let modified = track.modified.map_or("—".to_string(), format_relative_time);
    let metadata = view.metadata.as_ref();
    let field = |value: Option<&String>| value.map_or("—".to_string(), |value| value.clone());

    let mut lines = vec![Line::from(Span::styled(track.name.clone(), Style::default().fg(INK).add_modifier(Modifier::BOLD)))];
    lines.push(if is_remote {
        Line::from(Span::styled("☁ metadata only — no local file", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)))
    } else {
        Line::from(Span::styled("♪ downloaded", Style::default().fg(LIME).add_modifier(Modifier::BOLD)))
    });
    lines.push(Line::from(""));
    if let Some(metadata) = metadata {
        lines.push(Line::from(vec![Span::styled("title: ", Style::default().fg(YELLOW)), Span::styled(field(metadata.title.as_ref()), Style::default().fg(INK))]));
        lines.push(Line::from(vec![Span::styled("artist: ", Style::default().fg(YELLOW)), Span::styled(field(metadata.artist.as_ref()), Style::default().fg(INK))]));
        lines.push(Line::from(vec![Span::styled("album: ", Style::default().fg(YELLOW)), Span::styled(field(metadata.album.as_ref()), Style::default().fg(INK))]));
        let extras = vec![metadata.genre.clone(), metadata.year.map(|year| year.to_string())].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        lines.push(Line::from(vec![Span::styled("genre / year: ", Style::default().fg(YELLOW)), Span::styled(if extras.is_empty() { "—".to_string() } else { extras }, Style::default().fg(INK))]));
        lines.push(Line::from(vec![Span::styled("duration: ", Style::default().fg(YELLOW)), Span::styled(format_duration(metadata.duration_secs), Style::default().fg(INK))]));
    } else {
        lines.push(Line::from(Span::styled("no audio tags found", Style::default().fg(BLUE))));
    }
    if let Some(id) = view.track_ids.get(view.selected) {
        lines.extend(library_track_lines(app, id, Some((&view.crate_name, &view.playlist.name))));
    }
    if !is_remote {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled("format: ", Style::default().fg(YELLOW)), Span::styled(extension, Style::default().fg(CYAN))]));
        lines.push(Line::from(vec![Span::styled("size: ", Style::default().fg(YELLOW)), Span::styled(format_size(track.size_bytes), Style::default().fg(CYAN))]));
        lines.push(Line::from(vec![Span::styled("modified: ", Style::default().fg(YELLOW)), Span::styled(modified, Style::default().fg(CYAN))]));
    }

    frame.render_widget(Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }), text_area);
}

fn format_duration(total_secs: u64) -> String {
    format!("{}:{:02}", total_secs / 60, total_secs % 60)
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;
    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }
    if unit_index == 0 { format!("{bytes} B") } else { format!("{size:.1} {}", UNITS[unit_index]) }
}

fn format_relative_time(modified: std::time::SystemTime) -> String {
    match std::time::SystemTime::now().duration_since(modified) {
        Ok(elapsed) => {
            let seconds = elapsed.as_secs();
            if seconds < 60 {
                "just now".to_string()
            } else if seconds < 3600 {
                format!("{} min ago", seconds / 60)
            } else if seconds < 86400 {
                format!("{} h ago", seconds / 3600)
            } else if seconds < 86400 * 30 {
                format!("{} days ago", seconds / 86400)
            } else {
                format!("{} months ago", seconds / (86400 * 30))
            }
        }
        Err(_) => "in the future".to_string(),
    }
}

fn draw_tag_browser(frame: &mut Frame, browser: &crate::app::TagBrowser, area: Rect) {
    let popup = centered_rect(60, 60, area);
    frame.render_widget(Clear, popup);

    if let Some(tag) = &browser.filter {
        let sections = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(3), Constraint::Length(1)]).split(popup);
        let block = Block::default()
            .title(Span::styled(format!(" ♪ TAGGED \"{tag}\" ♪ "), Style::default().fg(PANEL).bg(LIME).add_modifier(Modifier::BOLD)))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(LIME))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(PANEL));

        if browser.matches.is_empty() {
            frame.render_widget(Paragraph::new(Span::styled("  (no playlists with this tag)", Style::default().fg(BLUE))).block(block), sections[0]);
        } else {
            let items: Vec<ListItem> = browser.matches.iter().enumerate().map(|(index, (crate_name, playlist_name))| {
                let selected = index == browser.match_selected;
                let prefix = if selected { "› " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{prefix}{playlist_name}"), Style::default().fg(if selected { INK } else { CYAN })),
                    Span::styled(format!("  [{crate_name}]"), Style::default().fg(BLUE)),
                ]))
            }).collect();
            let mut state = ListState::default();
            state.select(Some(browser.match_selected));
            frame.render_stateful_widget(List::new(items).block(block).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK).add_modifier(Modifier::BOLD)), sections[0], &mut state);
        }
        frame.render_widget(Paragraph::new(Line::from(vec![
            pill("Enter", BACKDROP, PINK),
            Span::styled(" open playlist   ", Style::default().fg(CYAN)),
            pill("Esc / t", PANEL, INK),
            Span::styled(" back to tags", Style::default().fg(CYAN)),
        ])).style(Style::default().bg(PANEL)), sections[1]);
        return;
    }

    let items: Vec<ListItem> = browser.tags.iter().enumerate().map(|(index, tag)| {
        let selected = index == browser.selected;
        let prefix = if selected { "› " } else { "  " };
        ListItem::new(Line::from(Span::styled(format!("{prefix}{tag}"), Style::default().fg(if selected { INK } else { CYAN }))))
    }).collect();
    let mut state = ListState::default();
    state.select(Some(browser.selected));
    let block = Block::default()
        .title(Span::styled(" ♪ TAGS ♪ ", Style::default().fg(PANEL).bg(LIME).add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(LIME))
        .padding(Padding::horizontal(1))
        .style(Style::default().bg(PANEL));
    frame.render_stateful_widget(List::new(items).block(block).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK).add_modifier(Modifier::BOLD)), popup, &mut state);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage((100 - percent_y) / 2), Constraint::Percentage(percent_y), Constraint::Percentage((100 - percent_y) / 2)])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage((100 - percent_x) / 2), Constraint::Percentage(percent_x), Constraint::Percentage((100 - percent_x) / 2)])
        .split(vertical[1])[1]
}

fn draw_confirm_delete(frame: &mut Frame, app: &App, index: usize, area: Rect) {
    let Some(crate_location) = app.crates.get(index) else { return };
    let popup = centered_rect(56, 40, area);
    frame.render_widget(Clear, popup);

    let mut lines = vec![
        Line::from(Span::styled(format!("Remove \"{}\"?", crate_location.name), Style::default().fg(INK).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("You will lose all its paths and playlists:", Style::default().fg(YELLOW))),
        Line::from(""),
    ];
    for location in &crate_location.locations {
        lines.push(Line::from(Span::styled(format!("  • {}", location.path), Style::default().fg(CYAN))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        pill("y", BACKDROP, PINK),
        Span::styled(" delete it   ", Style::default().fg(CYAN)),
        pill("n / Esc", PANEL, INK),
        Span::styled(" cancel", Style::default().fg(CYAN)),
    ]));

    let block = Block::default()
        .title(Span::styled(" ⚠ CONFIRM DELETE ⚠ ", Style::default().fg(PANEL).bg(PINK).add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(PINK))
        .padding(Padding::uniform(1))
        .style(Style::default().bg(PANEL));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

fn draw_config(frame: &mut Frame, app: &App, area: Rect) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(10), Constraint::Length(2), Constraint::Length(2)])
        .split(area);
    draw_header(frame, app, vertical[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(vertical[1]);

    let crate_items = app.crates.iter().enumerate().map(|(index, item)| {
        let selected = index == app.selected_crate;
        let marker = if item.available { "♥" } else { "♡" };
        let marker_color = if item.available { LIME } else { PINK };
        let name_color = if selected { INK } else { CYAN };
        let mut spans = vec![
            Span::styled(format!("{} ", marker), Style::default().fg(marker_color)),
            Span::styled(&item.name, Style::default().fg(name_color)),
        ];
        if !item.available {
            spans.push(Span::styled("  ✕", Style::default().fg(PINK)));
        }
        ListItem::new(Line::from(spans))
    });
    let mut crate_state = ListState::default();
    crate_state.select(Some(app.selected_crate));
    frame.render_stateful_widget(List::new(crate_items).block(panel("✦ CRATES ✦")).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK)), body[0], &mut crate_state);

    if let Some(selected_crate) = app.crates.get(app.selected_crate) {
        let location_count = selected_crate.locations.len();
        let mut labels = vec!["crate name".to_string()];
        for index in 0..location_count {
            let removable = selected_crate.locations[index].removable;
            labels.push(format!("path {}{}", index + 1, if removable { " [ext]" } else { "" }));
        }
        const LABEL_COLUMN: usize = 18;
        let value_width = (body[1].width as usize).saturating_sub(4 + LABEL_COLUMN).max(8);
        let mut rows = Vec::new();
        for (index, label) in labels.iter().enumerate() {
            let selected = app.config_field == index;
            let is_editing_here = selected && app.editing_config;
            let removable = index > 0 && selected_crate.locations.get(index - 1).is_some_and(|location| location.removable);
            let (value, value_color) = if is_editing_here {
                let chars: Vec<char> = app.config_buffer.chars().collect();
                let cursor = app.config_cursor.min(chars.len());
                let before: String = chars[..cursor].iter().collect();
                let after: String = chars[cursor..].iter().collect();
                (format!("{before}█{after}"), INK)
            } else {
                let committed = app.config_value(index);
                let missing = index > 0 && !committed.is_empty() && !std::path::Path::new(&committed).exists();
                let suffix = match (missing, removable) {
                    (true, true) => "  🔌 disconnected",
                    (true, false) => "  ✕ not found",
                    _ => "",
                };
                let color = match (missing, removable, selected) {
                    (true, true, _) => YELLOW,
                    (true, false, _) => PINK,
                    (false, _, true) => INK,
                    _ => BLUE,
                };
                (format!("{committed}{suffix}"), color)
            };
            let prefix = if selected { "›" } else { " " };
            let value_lines = wrap_text(&value, value_width);
            let mut lines = vec![Line::from(vec![
                Span::styled(format!("{} {:<16}", prefix, label), Style::default().fg(if selected { LIME } else { CYAN }).add_modifier(Modifier::BOLD)),
                Span::styled(value_lines.first().cloned().unwrap_or_default(), Style::default().fg(value_color)),
            ])];
            for continuation in value_lines.iter().skip(1) {
                lines.push(Line::from(Span::styled(format!("{:>width$}{}", "", continuation, width = LABEL_COLUMN), Style::default().fg(value_color))));
            }
            rows.push(ListItem::new(lines));
        }
        let hint = if app.editing_config {
            "typing... Enter save · Esc cancel".to_string()
        } else {
            "↑/↓ select · Enter edit · ←/→ crate · a add · x remove · e toggle external drive · n new crate · X delete crate".to_string()
        };
        let mut state = ListState::default();
        state.select(Some(app.config_field));
        frame.render_stateful_widget(List::new(rows).block(panel("♡ CRATE DETAILS ♡")).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK)), body[1], &mut state);
        frame.render_widget(Paragraph::new(Span::styled(format!("  {}", hint), Style::default().fg(YELLOW))), vertical[2]);
    } else {
        frame.render_widget(Paragraph::new(Span::styled("  No crates yet.", Style::default().fg(INK))).block(panel("♡ CRATE DETAILS ♡")), body[1]);
        frame.render_widget(Paragraph::new(Span::styled("  n new crate", Style::default().fg(YELLOW))), vertical[2]);
    }
    frame.render_widget(Paragraph::new(Line::from(vec![
        pill("Esc", BACKDROP, PINK),
        Span::styled(" back   ", Style::default().fg(CYAN)),
        pill("c", PANEL, INK),
        Span::styled(" dashboard", Style::default().fg(CYAN)),
    ])), vertical[3]);
}

fn draw_import(frame: &mut Frame, app: &App, area: Rect) {
    use crate::app::ImportStep;
    use crate::model::ImportService;

    let vertical = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(5), Constraint::Length(2), Constraint::Min(8), Constraint::Length(2)]).split(area);
    draw_header(frame, app, vertical[0]);

    let Some(import) = &app.import else {
        frame.render_widget(Paragraph::new(""), vertical[1]);
        return;
    };

    let breadcrumb = vec![
        Span::styled("Import", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)),
        Span::styled(" › ", Style::default().fg(BLUE)),
        Span::styled(import.service.map_or("service?".to_string(), |service| service.label().to_string()), Style::default().fg(CYAN)),
        Span::styled(" › ", Style::default().fg(BLUE)),
        Span::styled(import.is_new.map_or("new or existing?".to_string(), |is_new| if is_new { "new playlist".into() } else { "existing playlist".into() }), Style::default().fg(CYAN)),
    ];
    frame.render_widget(Paragraph::new(Line::from(breadcrumb)), vertical[1]);

    match import.step {
        ImportStep::Service => {
            let items: Vec<ListItem> = ImportService::ALL.iter().enumerate().map(|(index, service)| {
                let selected = index == import.service_index;
                let prefix = if selected { "› " } else { "  " };
                ListItem::new(Line::from(Span::styled(format!("{prefix}{}", service.label()), Style::default().fg(if selected { INK } else { CYAN }))))
            }).collect();
            let mut state = ListState::default();
            state.select(Some(import.service_index));
            frame.render_stateful_widget(List::new(items).block(panel("✦ CHOOSE SERVICE ✦")).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK)), vertical[2], &mut state);
        }
        ImportStep::Mode => {
            let options = ["This is a new playlist", "Link to an existing playlist"];
            let items: Vec<ListItem> = options.iter().enumerate().map(|(index, label)| {
                let selected = index == import.mode_index;
                let prefix = if selected { "› " } else { "  " };
                ListItem::new(Line::from(Span::styled(format!("{prefix}{label}"), Style::default().fg(if selected { INK } else { CYAN }))))
            }).collect();
            let mut state = ListState::default();
            state.select(Some(import.mode_index));
            frame.render_stateful_widget(List::new(items).block(panel("✦ NEW OR EXISTING? ✦")).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK)), vertical[2], &mut state);
        }
        ImportStep::Crate => {
            if app.crates.is_empty() {
                frame.render_widget(Paragraph::new(Span::styled("  No crates yet. Esc back, then c to add one.", Style::default().fg(INK))).block(panel("✦ CHOOSE CRATE ✦")), vertical[2]);
            } else {
                let items: Vec<ListItem> = app.crates.iter().enumerate().map(|(index, crate_location)| {
                    let selected = index == import.crate_index;
                    let prefix = if selected { "› " } else { "  " };
                    ListItem::new(Line::from(Span::styled(format!("{prefix}{}", crate_location.name), Style::default().fg(if selected { INK } else { CYAN }))))
                }).collect();
                let mut state = ListState::default();
                state.select(Some(import.crate_index));
                frame.render_stateful_widget(List::new(items).block(panel("✦ CHOOSE CRATE ✦")).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK)), vertical[2], &mut state);
            }
        }
        ImportStep::Playlist => {
            let playlists = app.crates.get(import.crate_index).map(|crate_location| crate_location.playlists.as_slice()).unwrap_or(&[]);
            if playlists.is_empty() {
                frame.render_widget(Paragraph::new(Span::styled("  This crate has no playlists yet.", Style::default().fg(INK))).block(panel("✦ CHOOSE PLAYLIST ✦")), vertical[2]);
            } else {
                let items: Vec<ListItem> = playlists.iter().enumerate().map(|(index, playlist)| {
                    let selected = index == import.playlist_index;
                    let prefix = if selected { "› " } else { "  " };
                    ListItem::new(Line::from(Span::styled(format!("{prefix}{}", playlist.name), Style::default().fg(if selected { INK } else { CYAN }))))
                }).collect();
                let mut state = ListState::default();
                state.select(Some(import.playlist_index));
                frame.render_stateful_widget(List::new(items).block(panel("✦ CHOOSE PLAYLIST ✦")).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK)), vertical[2], &mut state);
            }
        }
        ImportStep::Name => {
            let chars: Vec<char> = import.name_buffer.chars().collect();
            let cursor = import.name_cursor.min(chars.len());
            let before: String = chars[..cursor].iter().collect();
            let after: String = chars[cursor..].iter().collect();
            let fetches_online = matches!(import.service, Some(ImportService::Spotify) | Some(ImportService::SoundCloud));
            let label = match import.service {
                Some(ImportService::Spotify) => "Paste the Spotify playlist link (open.spotify.com/playlist/...):",
                Some(ImportService::SoundCloud) => "Paste the SoundCloud playlist link (public playlists only):",
                _ if import.is_new == Some(true) => "New playlist name (also used as the folder name):",
                _ => "Playlist name on the service:",
            };
            let title = if fetches_online { "✦ PLAYLIST LINK ✦" } else { "✦ PLAYLIST NAME ✦" };
            let lines = vec![
                Line::from(Span::styled(label, Style::default().fg(YELLOW))),
                Line::from(""),
                Line::from(vec![Span::styled(before, Style::default().fg(INK)), Span::styled("█", Style::default().fg(INK)), Span::styled(after, Style::default().fg(INK))]),
            ];
            frame.render_widget(Paragraph::new(lines).block(panel(title)), vertical[2]);
        }
    }

    let hint = match import.step {
        ImportStep::Name if matches!(import.service, Some(ImportService::Spotify) | Some(ImportService::SoundCloud)) => "paste the link · Enter fetch tracks · Esc back",
        ImportStep::Name => "type the name · Enter confirm · Esc back",
        _ => "↑/↓ select · Enter confirm · Esc back",
    };
    frame.render_widget(Paragraph::new(Line::from(vec![
        pill("Esc", BACKDROP, PINK),
        Span::styled(" back   ", Style::default().fg(CYAN)),
        Span::styled(hint, Style::default().fg(YELLOW)),
    ])), vertical[3]);
}

fn draw_settings(frame: &mut Frame, app: &App, area: Rect) {
    let vertical = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(5), Constraint::Min(8), Constraint::Length(2)]).split(area);
    draw_header(frame, app, vertical[0]);

    let crate_count = app.crates.len();
    let playlist_count: usize = app.crates.iter().map(|crate_location| crate_location.playlists.len()).sum();
    let path_count: usize = app.crates.iter().map(|crate_location| crate_location.locations.len()).sum();

    let field_labels = ["Spotify Client ID", "Tidal Client ID", "Tidal Client Secret", "Tidal Country Code", "Last.fm API Key (optional, better genre tags)", "Cloud backup folder (iCloud/Dropbox/Drive synced dir)"];
    let field_values = [app.spotify_client_id.as_deref(), app.tidal_client_id.as_deref(), app.tidal_client_secret.as_deref(), app.tidal_country_code.as_deref(), app.lastfm_api_key.as_deref(), app.cloud_backup_dir.as_deref()];
    let mut credential_lines = Vec::new();
    for (index, label) in field_labels.iter().enumerate() {
        let selected = app.settings_field == index;
        let prefix = if selected { "› " } else { "  " };
        credential_lines.push(Line::from(Span::styled(format!("{prefix}{label}"), Style::default().fg(if selected { LIME } else { YELLOW }).add_modifier(Modifier::BOLD))));
        if selected && app.editing_settings {
            let chars: Vec<char> = app.settings_buffer.chars().collect();
            let cursor = app.settings_cursor.min(chars.len());
            let before: String = chars[..cursor].iter().collect();
            let after: String = chars[cursor..].iter().collect();
            credential_lines.push(Line::from(vec![Span::styled("    ", Style::default()), Span::styled(before, Style::default().fg(INK)), Span::styled("█", Style::default().fg(INK)), Span::styled(after, Style::default().fg(INK))]));
        } else {
            // Mask the secret so it's not shown in plain text on screen.
            let is_secret = index == 2 || index == 4;
            let display_value = match field_values[index] {
                Some(value) if is_secret => "•".repeat(value.chars().count().min(24)),
                Some(value) => value.to_string(),
                None if index == 3 => "(not set, defaults to BR)".to_string(),
                None if index == 4 => "(not set — using MusicBrainz)".to_string(),
                None if index == 5 => "(not set — no cloud backup)".to_string(),
                None => "(not set)".to_string(),
            };
            let color = if field_values[index].is_some() { CYAN } else { BLUE };
            credential_lines.push(Line::from(Span::styled(format!("    {display_value}"), Style::default().fg(color))));
        }
    }

    let session_line = if app.spotify_refresh_token.is_some() {
        Line::from(vec![Span::styled("  ● connected", Style::default().fg(LIME).add_modifier(Modifier::BOLD)), Span::styled("   (press L to disconnect)", Style::default().fg(BLUE))])
    } else {
        Line::from(vec![Span::styled("  ○ not connected", Style::default().fg(PINK).add_modifier(Modifier::BOLD)), Span::styled("   (press l to log in)", Style::default().fg(BLUE))])
    };

    let mut lines = vec![
        Line::from(Span::styled("Status", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled(format!("  {}", app.message), Style::default().fg(INK))),
        Line::from(""),
        Line::from(Span::styled("Config file", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled(format!("  {}", crate::config::display_path()), Style::default().fg(CYAN))),
        Line::from(""),
    ];
    lines.extend(credential_lines);
    lines.push(Line::from(""));
    lines.extend(vec![
        Line::from(Span::styled("Spotify session", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))),
        session_line,
        Line::from(""),
        Line::from(Span::styled("Cloud backup", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled(
            match (&app.cloud_backup_dir, &app.backup_status) {
                (None, _) => "  off — set a folder above (b backup now · B restore)".to_string(),
                (Some(_), None) => "  not written yet this session (b backup now · B restore)".to_string(),
                (Some(dir), Some(Ok(at))) => format!("  ☁ saved {} → {dir}/{}", format_age(*at), crate::library::BACKUP_FILE_NAME),
                (Some(_), Some(Err(error))) => format!("  ✕ failed: {error}"),
            },
            Style::default().fg(if matches!(app.backup_status, Some(Err(_))) { PINK } else { CYAN }),
        )),
        Line::from(""),
        Line::from(Span::styled("Overview", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled(format!("  {crate_count} crates · {path_count} paths · {playlist_count} playlists · {} library tracks", app.library.tracks.len()), Style::default().fg(INK))),
        Line::from(""),
        Line::from(Span::styled("Keybindings", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  j/k, ↑/↓    navigate playlists", Style::default().fg(INK))),
        Line::from(Span::styled("  tab         change crate", Style::default().fg(INK))),
        Line::from(Span::styled("  c           manage crates", Style::default().fg(INK))),
        Line::from(Span::styled("  n           new crate", Style::default().fg(INK))),
        Line::from(Span::styled("  i           import / link a playlist", Style::default().fg(INK))),
        Line::from(Span::styled("  b  or  /    explore the library (search by artist / title)", Style::default().fg(INK))),
        Line::from(Span::styled("  t / T       browse tags / edit tags", Style::default().fg(INK))),
        Line::from(Span::styled("  r           rescan playlists from disk", Style::default().fg(INK))),
        Line::from(Span::styled("  s           this screen", Style::default().fg(INK))),
        Line::from(Span::styled("  q           quit", Style::default().fg(INK))),
    ]);
    frame.render_widget(Paragraph::new(lines).block(panel("⚙ SETTINGS ⚙")), vertical[1]);
    let hint = if app.spotify_login_pending() {
        Line::from(vec![Span::styled("waiting for Spotify login…   ", Style::default().fg(YELLOW)), pill("Esc / c", BACKDROP, PINK), Span::styled(" cancel   ", Style::default().fg(CYAN)), pill("q", PANEL, INK), Span::styled(" quit", Style::default().fg(CYAN))])
    } else if app.editing_settings {
        Line::from(vec![pill("Enter", BACKDROP, PINK), Span::styled(" save   ", Style::default().fg(CYAN)), pill("Esc", PANEL, INK), Span::styled(" cancel", Style::default().fg(CYAN))])
    } else {
        Line::from(vec![
            pill("Esc / s", BACKDROP, PINK),
            Span::styled(" back   ", Style::default().fg(CYAN)),
            pill("j/k", PANEL, INK),
            Span::styled(" select   ", Style::default().fg(CYAN)),
            pill("Enter / e", PANEL, INK),
            Span::styled(" edit   ", Style::default().fg(CYAN)),
            pill("l / L", PANEL, INK),
            Span::styled(" connect / disconnect Spotify   ", Style::default().fg(CYAN)),
            pill("b / B", PANEL, INK),
            Span::styled(" backup now / restore", Style::default().fg(CYAN)),
        ])
    };
    frame.render_widget(Paragraph::new(hint), vertical[2]);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let title = Line::from(vec![
        Span::styled("crate", Style::default().fg(PANEL).bg(LIME).add_modifier(Modifier::BOLD)),
        Span::styled("rat", Style::default().fg(PANEL).bg(PINK).add_modifier(Modifier::BOLD)),
        Span::styled(" crate manager   ", Style::default().fg(CYAN)),
        Span::styled("🐀 🐁 🐭 🐹", Style::default().fg(YELLOW)),
    ]);
    let subtitle = activity_line(app);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(PINK))
        .padding(Padding::horizontal(1))
        .style(Style::default().bg(PANEL));
    frame.render_widget(Paragraph::new(vec![title, subtitle]).block(block), area);
}

fn draw_crates(frame: &mut Frame, app: &App, area: Rect) {
    // panel() adds a 1-cell border and 1-cell horizontal padding on each side.
    let inner_width = area.width.saturating_sub(4).max(8) as usize;
    let items = app.crates.iter().map(|item| {
        let marker = if item.available { "♥" } else { "♡" };
        let color = if item.available { LIME } else { BLUE };
        let mut lines = vec![Line::from(Span::styled(format!("{} {}", marker, item.name), Style::default().fg(color)))];
        for location in &item.locations {
            let found = std::path::Path::new(&location.path).exists();
            let path_color = if found { CYAN } else if location.removable { YELLOW } else { PINK };
            let wrapped_lines = wrap_text(&location.path, inner_width.saturating_sub(2));
            for (index, wrapped) in wrapped_lines.iter().enumerate() {
                let is_last = index == wrapped_lines.len() - 1;
                let suffix = match (found, location.removable, is_last) {
                    (false, true, true) => "  🔌 disconnected",
                    (false, false, true) => "  ✕ not found",
                    _ => "",
                };
                lines.push(Line::from(Span::styled(format!("  {}{}", wrapped, suffix), Style::default().fg(path_color))));
            }
        }
        ListItem::new(lines)
    });
    let mut state = ListState::default();
    state.select(Some(app.selected_crate));
    frame.render_stateful_widget(List::new(items).block(panel("✦ CRATES ✦")).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK).add_modifier(Modifier::BOLD)), area, &mut state);
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }
    text.chars().collect::<Vec<_>>().chunks(width).map(|chunk| chunk.iter().collect()).collect()
}

fn draw_playlists(frame: &mut Frame, app: &App, area: Rect) {
    let Some(crate_location) = app.crates.get(app.selected_crate) else {
        frame.render_widget(Paragraph::new(Span::styled("  Add a crate with c, then n.", Style::default().fg(INK))).block(panel("♫ PLAYLISTS ♫")), area);
        return;
    };
    let rows = crate_location.playlists.iter().enumerate().map(|(playlist_index, playlist)| {
        let status_color = if playlist.link.is_some() && playlist.track_count == 0 { PINK } else if playlist.synced == playlist.track_count { LIME } else { PINK };
        let name = playlist.link.as_ref().map_or_else(|| playlist.name.clone(), |link| format!("{} [{}]", playlist.name, link.service.label()));
        Row::new(vec![
            Cell::from(name),
            Cell::from(format!("{}/{}", playlist.synced, playlist.track_count)),
            Cell::from(playlist.status().to_string()),
            Cell::from(Line::from(activity_spans(&app.playlist_activity(app.selected_crate, playlist_index)))),
            Cell::from(Line::from(tag_spans_for(&playlist.tags, &playlist.auto_tags))),
        ])
        .style(Style::default().fg(status_color))
    });
    let title: &'static str = if crate_location.locations.len() > 1 { "♫ PLAYLISTS (shared across paths) ♫" } else { "♫ PLAYLISTS ♫" };
    let table = Table::new(rows, [Constraint::Percentage(26), Constraint::Length(9), Constraint::Length(11), Constraint::Length(16), Constraint::Min(10)])
        .header(Row::new(vec!["PLAYLIST", "TRACKS", "STATUS", "SYNC", "TAGS"]).style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)))
        .block(panel(title))
        .row_highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK).add_modifier(Modifier::BOLD));
    let mut state = TableState::default();
    state.select(Some(app.selected_playlist));
    frame.render_stateful_widget(table, area, &mut state);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    if app.editing_tags {
        let chars: Vec<char> = app.tag_buffer.chars().collect();
        let cursor = app.tag_cursor.min(chars.len());
        let before: String = chars[..cursor].iter().collect();
        let after: String = chars[cursor..].iter().collect();
        let spans = vec![
            Span::styled("  🏷 ", Style::default().fg(YELLOW)),
            Span::styled("tags (comma separated): ", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)),
            Span::styled(before, Style::default().fg(INK)),
            Span::styled("█", Style::default().fg(INK)),
            Span::styled(after, Style::default().fg(INK)),
            Span::styled("   (Enter save · Esc cancel)", Style::default().fg(CYAN)),
        ];
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    let mut spans = vec![Span::styled("  ✦ ", Style::default().fg(LIME)), Span::styled(app.message.clone(), Style::default().fg(INK))];
    if let Some(crate_location) = app.crates.get(app.selected_crate) {
        spans.push(Span::styled("   ", Style::default()));
        spans.push(pill(&crate_location.name, PANEL, PINK));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn panel(title: &'static str) -> Block<'static> {
    Block::default()
        .title(Span::styled(format!(" {} ", title), Style::default().fg(PANEL).bg(LIME).add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(CYAN))
        .padding(Padding::horizontal(1))
        .style(Style::default().bg(PANEL))
}

fn pill(label: &str, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(format!(" {} ", label), Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD))
}
/// One-line summary of everything running in the background, shown in the header on every
/// screen so it's always clear what's being synced.
fn activity_line(app: &App) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let push = |spans: &mut Vec<Span<'static>>, text: String, color: Color| {
        if !spans.is_empty() {
            spans.push(Span::styled("   ", Style::default()));
        }
        spans.push(Span::styled(text, Style::default().fg(color)));
    };
    let queued = app.tag_queue_len();
    if queued > 0 {
        let now = app.tag_in_flight.as_ref().and_then(|id| app.library.tracks.get(id)).map(|track| format!(" · now: {} — {}", if track.artist.is_empty() { "?" } else { &track.artist }, track.title)).unwrap_or_default();
        push(&mut spans, format!("⟳ tags {queued} left{now}"), YELLOW);
    }
    if !app.tidal_refresh_queue.is_empty() {
        push(&mut spans, format!("⟳ tidal: checking {} playlist(s)", app.tidal_refresh_queue.len()), CYAN);
    }
    if app.is_importing_new() {
        push(&mut spans, "⟳ importing playlist".to_string(), CYAN);
    }
    if app.is_downloading() {
        push(&mut spans, "⬇ downloading".to_string(), CYAN);
    }
    if !app.tag_errors.is_empty() {
        push(&mut spans, format!("✕ {} tag lookup(s) failed (offline?)", app.tag_errors.len()), PINK);
    }
    match (&app.cloud_backup_dir, &app.backup_status) {
        (Some(_), Some(Ok(at))) => push(&mut spans, format!("☁ backup {}", format_age(*at)), BLUE),
        (Some(_), Some(Err(_))) => push(&mut spans, "☁ backup failed".to_string(), PINK),
        _ => {}
    }
    let busy = queued > 0 || !app.tidal_refresh_queue.is_empty() || app.is_downloading() || app.is_importing_new();
    if !busy {
        spans.insert(0, Span::styled(format!("✓ up to date · {} tracks in library", app.library.tracks.len()), Style::default().fg(LIME)));
        if spans.len() > 1 {
            spans.insert(1, Span::styled("   ", Style::default()));
        }
    }
    Line::from(spans)
}

fn activity_spans(activities: &[Activity]) -> Vec<Span<'static>> {
    if activities.is_empty() {
        return vec![Span::styled("✓", Style::default().fg(LIME))];
    }
    let mut spans = Vec::new();
    for activity in activities {
        let (text, color) = match activity {
            Activity::Tidal(true) => ("⟳ tidal".to_string(), CYAN),
            Activity::Tidal(false) => ("… tidal".to_string(), BLUE),
            Activity::Importing => ("⟳ import".to_string(), CYAN),
            Activity::Downloading => ("⬇ dl".to_string(), CYAN),
            Activity::Tags { left, active: true } => (format!("⟳ tags {left}"), YELLOW),
            Activity::Tags { left, active: false } => (format!("… tags {left}"), BLUE),
        };
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(text, Style::default().fg(color)));
    }
    spans
}

/// Hand-written tags in white, tags derived from the tracks as `#tag` in blue.
fn tag_spans_for(user_tags: &[String], auto_tags: &[String]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for tag in user_tags {
        spans.push(Span::styled(format!("{tag}  "), Style::default().fg(INK)));
    }
    for tag in auto_tags.iter().filter(|tag| !user_tags.iter().any(|user| user.eq_ignore_ascii_case(tag))) {
        spans.push(Span::styled(format!("#{tag} "), Style::default().fg(BLUE)));
    }
    if spans.is_empty() {
        spans.push(Span::styled("—", Style::default().fg(BLUE)));
    }
    spans
}

fn tag_state_span(state: TagState) -> Span<'static> {
    match state {
        TagState::Syncing => Span::styled("  ⟳ tags", Style::default().fg(YELLOW)),
        TagState::Queued => Span::styled("  …", Style::default().fg(BLUE)),
        TagState::Failed => Span::styled("  ✕ tags", Style::default().fg(PINK)),
        TagState::Done | TagState::NotFound | TagState::Waiting => Span::raw(""),
    }
}

fn tag_state_label(state: TagState) -> (&'static str, Color) {
    match state {
        TagState::Syncing => ("⟳ looking up now…", YELLOW),
        TagState::Queued => ("… queued for lookup", BLUE),
        TagState::Failed => ("✕ lookup failed (offline?) — g to retry", PINK),
        TagState::Done => ("✓ synced", LIME),
        TagState::NotFound => ("✓ looked up — nothing found online", BLUE),
        TagState::Waiting => ("pending", BLUE),
    }
}

/// Library facts for a track: online tags + sync state, service ids, local copies, and every
/// playlist it's in (the current one, if given, marked).
fn library_track_lines(app: &App, id: &str, current: Option<(&str, &str)>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let Some(track) = app.library.tracks.get(id) else { return lines };
    lines.push(Line::from(""));
    let tags = if track.tags.is_empty() { "—".to_string() } else { track.tags.join(", ") };
    lines.push(Line::from(vec![Span::styled("tags: ", Style::default().fg(YELLOW)), Span::styled(tags, Style::default().fg(INK))]));
    let (label, color) = tag_state_label(app.tag_state(id));
    let source = match (&track.tags_source, track.tags_updated_secs) {
        (Some(source), Some(at)) if app.tag_state(id) == TagState::Done => format!("  ({source}, {})", format_age(at)),
        _ => String::new(),
    };
    lines.push(Line::from(vec![Span::styled("tag sync: ", Style::default().fg(YELLOW)), Span::styled(format!("{label}{source}"), Style::default().fg(color))]));
    if !track.external_ids.is_empty() {
        let ids = track.external_ids.iter().map(|(service, external_id)| format!("{service}:{external_id}")).collect::<Vec<_>>().join("  ");
        lines.push(Line::from(vec![Span::styled("ids: ", Style::default().fg(YELLOW)), Span::styled(ids, Style::default().fg(CYAN))]));
    }
    let copies = app.library.files_for(id).len();
    lines.push(Line::from(vec![Span::styled("local copies: ", Style::default().fg(YELLOW)), Span::styled(copies.to_string(), Style::default().fg(if copies > 0 { LIME } else { BLUE }))]));
    let playlists = app.playlists_for(id);
    lines.push(Line::from(Span::styled(format!("in {} playlist(s):", playlists.len()), Style::default().fg(YELLOW))));
    for (crate_name, playlist_name) in playlists {
        let is_current = current.is_some_and(|(current_crate, current_playlist)| current_crate == crate_name && current_playlist == playlist_name);
        lines.push(Line::from(vec![
            Span::styled(if is_current { "  ● " } else { "  ○ " }, Style::default().fg(if is_current { LIME } else { PINK })),
            Span::styled(playlist_name.clone(), Style::default().fg(if is_current { INK } else { CYAN })),
            Span::styled(format!("  [{crate_name}]"), Style::default().fg(BLUE)),
        ]));
    }
    lines
}

fn format_age(secs: u64) -> String {
    format_relative_time(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

fn draw_library(frame: &mut Frame, app: &App, area: Rect) {
    let Some(browser) = &app.library_browser else { return };
    let vertical = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(5), Constraint::Length(3), Constraint::Min(6), Constraint::Length(1), Constraint::Length(2)]).split(area);
    draw_header(frame, app, vertical[0]);

    let search_style = Style::default().fg(if browser.editing { INK } else { CYAN });
    let mut search = vec![Span::styled("🔎 ", Style::default().fg(YELLOW)), Span::styled(browser.query.clone(), search_style)];
    if browser.editing {
        search.push(Span::styled("█", Style::default().fg(INK)));
    } else if browser.query.is_empty() {
        search.push(Span::styled("press / to search by artist, title, album or tag", Style::default().fg(BLUE)));
    }
    let track_count = browser.rows.iter().filter(|row| matches!(row, LibraryRow::Track(_))).count();
    search.push(Span::styled(format!("   {track_count}/{} tracks · {}", app.library.tracks.len(), if browser.by_artist { "by artist" } else { "by title" }), Style::default().fg(BLUE)));
    frame.render_widget(Paragraph::new(Line::from(search)).block(panel("✦ SEARCH ✦")), vertical[1]);

    let columns = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(52), Constraint::Percentage(48)]).split(vertical[2]);
    let with_files: std::collections::HashSet<&str> = app.library.files.values().map(|entry| entry.track_id.as_str()).collect();
    let items: Vec<ListItem> = browser
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let selected = index == browser.selected;
            match row {
                LibraryRow::Artist { name, count } => ListItem::new(Line::from(vec![
                    Span::styled(format!("{}♪ {name}", if selected { "› " } else { "" }), Style::default().fg(PINK).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("  ({count})"), Style::default().fg(BLUE)),
                ])),
                LibraryRow::Track(id) => {
                    let Some(track) = app.library.tracks.get(id) else { return ListItem::new("") };
                    let has_file = with_files.contains(id.as_str());
                    let is_playing = app.now_playing.as_ref().is_some_and(|path| app.library.files.get(&path.to_string_lossy().to_string()).is_some_and(|entry| entry.track_id == *id));
                    let indent = if browser.by_artist { "    " } else { "  " };
                    let mut spans = vec![
                        Span::styled(format!("{indent}{}", if is_playing { if app.audio_paused { "⏸ " } else { "▶ " } } else { "" }), Style::default().fg(LIME)),
                        Span::styled(track.title.clone(), Style::default().fg(if !has_file { BLUE } else if selected { INK } else { CYAN })),
                    ];
                    if !browser.by_artist {
                        spans.push(Span::styled(format!(" — {}", if track.artist.is_empty() { "?" } else { &track.artist }), Style::default().fg(BLUE)));
                    }
                    if !has_file {
                        spans.push(Span::styled("  ☁", Style::default().fg(YELLOW)));
                    }
                    let in_playlists = app.playlists_for(id).len();
                    if in_playlists > 1 {
                        spans.push(Span::styled(format!("  ×{in_playlists}"), Style::default().fg(PINK)));
                    }
                    spans.push(tag_state_span(app.tag_state(id)));
                    ListItem::new(Line::from(spans))
                }
            }
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(browser.selected));
    let title: &'static str = if browser.by_artist { "♫ LIBRARY · BY ARTIST ♫" } else { "♫ LIBRARY · BY TITLE ♫" };
    frame.render_stateful_widget(List::new(items).block(panel(title)).highlight_style(Style::default().bg(Color::Rgb(40, 0, 35)).fg(INK).add_modifier(Modifier::BOLD)), columns[0], &mut state);

    let details = match browser.rows.get(browser.selected) {
        Some(LibraryRow::Track(id)) => library_track_details(app, id),
        Some(LibraryRow::Artist { name, count }) => artist_details(app, browser, name, *count),
        None => vec![Line::from(Span::styled("No tracks match.", Style::default().fg(BLUE)))],
    };
    frame.render_widget(Paragraph::new(details).wrap(ratatui::widgets::Wrap { trim: false }).block(panel("♡ METADATA ♡")), columns[1]);

    frame.render_widget(Paragraph::new(Span::styled(format!("  {}", app.message), Style::default().fg(YELLOW))), vertical[3]);
    let footer = if browser.editing {
        Line::from(vec![pill("Enter", BACKDROP, PINK), Span::styled(" done   ", Style::default().fg(CYAN)), pill("Esc", PANEL, INK), Span::styled(" clear search", Style::default().fg(CYAN))])
    } else {
        Line::from(vec![
            pill("Esc", BACKDROP, PINK),
            Span::styled(" back   ", Style::default().fg(CYAN)),
            pill("/", PANEL, INK),
            Span::styled(" search   ", Style::default().fg(CYAN)),
            pill("Tab", PANEL, INK),
            Span::styled(" artist/title   ", Style::default().fg(CYAN)),
            pill("J/K", PANEL, INK),
            Span::styled(" next/prev artist   ", Style::default().fg(CYAN)),
            pill("Enter / p", PANEL, INK),
            Span::styled(" play   ", Style::default().fg(CYAN)),
            pill("x", PANEL, INK),
            Span::styled(" stop   ", Style::default().fg(CYAN)),
            pill("o", PANEL, INK),
            Span::styled(" open playlist   ", Style::default().fg(CYAN)),
            pill("g", PANEL, INK),
            Span::styled(" re-fetch tags", Style::default().fg(CYAN)),
        ])
    };
    frame.render_widget(Paragraph::new(footer), vertical[4]);
}

fn library_track_details(app: &App, id: &str) -> Vec<Line<'static>> {
    let Some(track) = app.library.tracks.get(id) else { return Vec::new() };
    let field = |label: &str, value: String| Line::from(vec![Span::styled(format!("{label}: "), Style::default().fg(YELLOW)), Span::styled(if value.is_empty() { "—".to_string() } else { value }, Style::default().fg(INK))]);
    let mut lines = vec![
        Line::from(Span::styled(track.title.clone(), Style::default().fg(INK).add_modifier(Modifier::BOLD))),
        field("artist", track.artist.clone()),
        field("album", track.album.clone()),
        field("year", track.year.map(|year| year.to_string()).unwrap_or_default()),
        field("duration", if track.duration_secs > 0 { format_duration(track.duration_secs) } else { String::new() }),
        field("genre (file)", track.genres.join(", ")),
    ];
    lines.extend(library_track_lines(app, id, None));
    if let Some(path) = app.library.files_for(id).first() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(path.display().to_string(), Style::default().fg(BLUE))));
    }
    lines
}

fn artist_details(app: &App, browser: &crate::app::LibraryBrowser, name: &str, count: usize) -> Vec<Line<'static>> {
    // The artist's tracks are the rows right after its header.
    let position = browser.selected;
    let ids: Vec<String> = browser.rows.iter().skip(position + 1).take(count).filter_map(|row| match row {
        LibraryRow::Track(id) => Some(id.clone()),
        LibraryRow::Artist { .. } => None,
    }).collect();
    let tags = app.library.playlist_tags(&ids);
    let mut playlists: Vec<(String, String)> = ids.iter().flat_map(|id| app.playlists_for(id).iter().cloned()).collect();
    playlists.sort();
    playlists.dedup();
    let mut lines = vec![
        Line::from(Span::styled(name.to_string(), Style::default().fg(PINK).add_modifier(Modifier::BOLD))),
        Line::from(vec![Span::styled("tracks: ", Style::default().fg(YELLOW)), Span::styled(count.to_string(), Style::default().fg(INK))]),
        Line::from(vec![Span::styled("tags: ", Style::default().fg(YELLOW)), Span::styled(if tags.is_empty() { "—".to_string() } else { tags.join(", ") }, Style::default().fg(INK))]),
        Line::from(""),
        Line::from(Span::styled(format!("in {} playlist(s):", playlists.len()), Style::default().fg(YELLOW))),
    ];
    for (crate_name, playlist_name) in playlists {
        lines.push(Line::from(vec![Span::styled("  ○ ", Style::default().fg(PINK)), Span::styled(playlist_name, Style::default().fg(CYAN)), Span::styled(format!("  [{crate_name}]"), Style::default().fg(BLUE))]));
    }
    lines
}

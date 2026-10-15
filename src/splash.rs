//! Startup splash: a rainbow rat and a spinner while `App::load` scans the crates.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

const RAT: &str = r#"
          __             _,-"~^"-.
        _// )      _,-"~`         `.
      ." ( /`"-,-"`                 ;
     / 6                             ;
    /           ,             ,-"     ;
   (,__.--.      \           /        ;
    //'   /`-.\   |          |        `._________
      _.-'_/`  )  )--...,,,___\     \-----------,)
    ((("~` _.-'.-'           __`-.   )         //
          ((("`             (((---~"`         //
                                             ((________________
                                             `----""""~~~~^^^```"#;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const PINK: Color = Color::Rgb(255, 0, 170);
const DIM: Color = Color::Rgb(120, 120, 140);

pub fn draw(frame: &mut Frame, tick: usize) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(Color::Black)), area);

    let art: Vec<&str> = RAT.lines().skip(1).collect();
    let art_width = art.iter().map(|line| line.chars().count()).max().unwrap_or(0) as u16;
    // Small terminals just get the spinner; a clipped rat looks broken.
    let show_art = area.width > art_width && area.height > art.len() as u16 + 4;
    let art_height = if show_art { art.len() as u16 } else { 0 };

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(art_height), Constraint::Length(1), Constraint::Length(2), Constraint::Fill(1)])
        .split(area);

    if show_art {
        let lines: Vec<Line> = art
            .iter()
            .enumerate()
            .map(|(row, line)| {
                // Left-pad every row to the same width so the art stays aligned when centered.
                let padded = format!("{line:<width$}", width = art_width as usize);
                Line::from(padded.chars().enumerate().map(|(col, ch)| Span::styled(ch.to_string(), Style::default().fg(rainbow(col + row * 2, tick)))).collect::<Vec<_>>())
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), vertical[1]);
    }

    let spinner = SPINNER[tick % SPINNER.len()];
    let dots = ".".repeat(tick / 4 % 4);
    let status = Line::from(vec![
        Span::styled(format!("{spinner} "), Style::default().fg(PINK).add_modifier(Modifier::BOLD)),
        Span::styled("crate rat", Style::default().fg(rainbow(0, tick)).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" is sniffing your crates{dots:<3}"), Style::default().fg(DIM)),
    ]);
    frame.render_widget(Paragraph::new(status).alignment(Alignment::Center), vertical[3]);
}

/// A hue that drifts left to right as `tick` advances.
fn rainbow(position: usize, tick: usize) -> Color {
    let hue = ((position * 6 + 360 * 4 - tick * 12 % 360) % 360) as f32;
    let (r, g, b) = hsv_to_rgb(hue, 0.85, 1.0);
    Color::Rgb(r, g, b)
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> (u8, u8, u8) {
    let chroma = value * saturation;
    let x = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = value - chroma;
    let (r, g, b) = match hue as u32 / 60 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let scale = |channel: f32| ((channel + m) * 255.0).round() as u8;
    (scale(r), scale(g), scale(b))
}

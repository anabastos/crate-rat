//! Public-playlist track downloading via SoundCloud's undocumented api-v2, the same approach
//! used by most open-source SoundCloud tools (scdl, yt-dlp, etc.) since SoundCloud's official
//! API registration is manual-approval-only. Only touches public data — no login required.

use std::{fs, io, path::Path};

pub struct SoundCloudTrack {
    pub title: String,
    pub artist: String,
    pub duration_secs: u64,
}

pub struct FetchResult {
    pub name: String,
    pub tracks: Vec<SoundCloudTrack>,
    pub failed: usize,
}

/// Resolves a public playlist link and downloads every track's actual audio (not just
/// metadata) into `<crate_path>/<folder_name>/`, where `folder_name` is `existing_folder_name`
/// if linking to an already-known local playlist, or the SoundCloud playlist's own title
/// otherwise.
pub fn download_playlist(playlist_link: &str, crate_path: &str, existing_folder_name: Option<&str>) -> Result<FetchResult, String> {
    let client_id = fetch_public_client_id()?;
    let resolve_url = format!("https://api-v2.soundcloud.com/resolve?url={}&client_id={client_id}", percent_encode(playlist_link.trim()));

    let response: serde_json::Value = ureq::get(&resolve_url).call().map_err(|error| format!("could not resolve that link: {}", describe_error(error)))?.into_json().map_err(|error| format!("bad response: {error}"))?;

    if response["kind"].as_str() != Some("playlist") {
        return Err("that link doesn't point to a playlist".into());
    }
    let name = response["title"].as_str().unwrap_or("SoundCloud Playlist").to_string();
    let raw_tracks = response["tracks"].as_array().cloned().unwrap_or_default();

    let mut full_tracks = Vec::new();
    let mut stub_ids = Vec::new();
    for track in raw_tracks {
        if track.get("media").is_some() {
            full_tracks.push(track);
        } else if let Some(id) = track["id"].as_u64() {
            stub_ids.push(id);
        }
    }
    // Large playlists come back with "stub" entries (just an id) for tracks past the first
    // page; fetch those in batches to get the full track object (with its transcodings).
    for chunk in stub_ids.chunks(50) {
        let ids = chunk.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let url = format!("https://api-v2.soundcloud.com/tracks?ids={ids}&client_id={client_id}");
        if let Ok(response) = ureq::get(&url).call() {
            if let Ok(list) = response.into_json::<Vec<serde_json::Value>>() {
                full_tracks.extend(list);
            }
        }
    }

    let folder_name = existing_folder_name.unwrap_or(&name);
    let dest_dir = Path::new(crate_path).join(folder_name);
    fs::create_dir_all(&dest_dir).map_err(|error| format!("could not create playlist folder: {error}"))?;

    let mut tracks = Vec::new();
    let mut failed = 0;
    for track in &full_tracks {
        let title = track["title"].as_str().unwrap_or("Unknown track").to_string();
        let artist = track["user"]["username"].as_str().unwrap_or("").to_string();
        let duration_secs = track["duration"].as_u64().unwrap_or(0) / 1000;
        match download_track(&client_id, track, &dest_dir, &title) {
            Ok(()) => tracks.push(SoundCloudTrack { title, artist, duration_secs }),
            Err(_) => failed += 1,
        }
    }

    if tracks.is_empty() && failed > 0 {
        return Err(format!("could not download any of the {failed} track(s) (Go+ exclusive tracks aren't downloadable this way)"));
    }
    Ok(FetchResult { name, tracks, failed })
}

fn download_track(client_id: &str, track: &serde_json::Value, dest_dir: &Path, title: &str) -> Result<(), String> {
    let transcodings = track["media"]["transcodings"].as_array().cloned().unwrap_or_default();
    let chosen = transcodings
        .iter()
        .find(|transcoding| transcoding["format"]["protocol"].as_str() == Some("progressive"))
        .ok_or_else(|| "no downloadable (progressive) stream — likely Go+ exclusive".to_string())?;

    let transcoding_url = chosen["url"].as_str().ok_or("missing transcoding url")?;
    let resolved: serde_json::Value = ureq::get(&format!("{transcoding_url}?client_id={client_id}")).call().map_err(|error| format!("could not resolve stream: {}", describe_error(error)))?.into_json().map_err(|error| format!("bad stream response: {error}"))?;
    let stream_url = resolved["url"].as_str().ok_or("no stream url in response")?;

    let mime = chosen["format"]["mime_type"].as_str().unwrap_or("audio/mpeg");
    let extension = if mime.contains("ogg") || mime.contains("opus") { "opus" } else { "mp3" };

    let response = ureq::get(stream_url).call().map_err(|error| format!("stream fetch failed: {}", describe_error(error)))?;
    let mut reader = response.into_reader();
    let path = dest_dir.join(format!("{}.{extension}", sanitize_filename(title)));
    let mut file = fs::File::create(&path).map_err(|error| format!("could not write file: {error}"))?;
    io::copy(&mut reader, &mut file).map_err(|error| format!("download failed: {error}"))?;
    Ok(())
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name.chars().map(|character| if "\\/:*?\"<>|".contains(character) { '_' } else { character }).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() { "track".to_string() } else { trimmed.to_string() }
}

/// SoundCloud's public web player embeds a `client_id` in its JS bundles; scrape it the same
/// way most unofficial SoundCloud tools do, since there's no self-serve way to get one.
fn fetch_public_client_id() -> Result<String, String> {
    let html = ureq::get("https://soundcloud.com/").call().map_err(|error| format!("could not reach soundcloud.com: {}", describe_error(error)))?.into_string().map_err(|error| format!("bad response: {error}"))?;

    let mut script_urls = Vec::new();
    let mut rest = html.as_str();
    while let Some(start) = rest.find("src=\"https://a-v2.sndcdn.com/assets/") {
        rest = &rest[start + 5..];
        if let Some(end) = rest.find('"') {
            script_urls.push(rest[..end].to_string());
            rest = &rest[end..];
        } else {
            break;
        }
    }

    for url in script_urls {
        let Ok(response) = ureq::get(&url).call() else { continue };
        let Ok(js) = response.into_string() else { continue };
        if let Some(start) = js.find("client_id:\"") {
            let after = &js[start + "client_id:\"".len()..];
            if let Some(end) = after.find('"') {
                return Ok(after[..end].to_string());
            }
        }
    }

    Err("could not find a public client_id on soundcloud.com (their site may have changed)".into())
}

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn describe_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            format!("HTTP {code}: {body}")
        }
        other => other.to_string(),
    }
}

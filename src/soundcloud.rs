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
    /// The first per-track failure ("title: reason"), so the UI can say why.
    pub first_error: Option<String>,
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
    let raw_track_count = raw_tracks.len();

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
    let mut lookup_failures = Vec::new();
    for chunk in stub_ids.chunks(50) {
        let ids = chunk.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let url = format!("https://api-v2.soundcloud.com/tracks?ids={ids}&client_id={client_id}");
        let fetched = ureq::get(&url).call().map_err(describe_error).and_then(|response| response.into_json::<Vec<serde_json::Value>>().map_err(|error| error.to_string()));
        match fetched {
            Ok(list) => {
                if list.len() < chunk.len() {
                    lookup_failures.push(format!("{} track(s) unavailable: SoundCloud didn't return their details", chunk.len() - list.len()));
                }
                full_tracks.extend(list);
            }
            Err(error) => lookup_failures.push(format!("{} track(s) unavailable: track lookup failed: {error}", chunk.len())),
        }
    }
    let lookup_failed = raw_track_count.saturating_sub(full_tracks.len());

    let folder_name = existing_folder_name.unwrap_or(&name);
    let dest_dir = Path::new(crate_path).join(folder_name);
    fs::create_dir_all(&dest_dir).map_err(|error| format!("could not create playlist folder: {error}"))?;

    let mut tracks = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for track in &full_tracks {
        let title = track["title"].as_str().unwrap_or("Unknown track").to_string();
        let artist = track["user"]["username"].as_str().unwrap_or("").to_string();
        let duration_secs = track["duration"].as_u64().unwrap_or(0) / 1000;
        match download_track(&client_id, track, &dest_dir, &title) {
            Ok(()) => tracks.push(SoundCloudTrack { title, artist, duration_secs }),
            Err(error) => failures.push(format!("{title}: {error}")),
        }
    }
    let failed = failures.len() + lookup_failed;
    let first_error = lookup_failures.into_iter().chain(failures).next();

    if tracks.is_empty() && failed > 0 {
        return Err(format!("could not download any of the {failed} track(s); first error: {}", first_error.unwrap_or_default()));
    }
    Ok(FetchResult { name, tracks, failed, first_error })
}

fn download_track(client_id: &str, track: &serde_json::Value, dest_dir: &Path, title: &str) -> Result<(), String> {
    // A "SNIP" policy means anonymous listeners only get a ~30s preview: the full track is
    // Go+ only. Skip those instead of saving a preview that looks like the real thing.
    if track["policy"].as_str() == Some("SNIP") {
        return Err("Go+ exclusive (only a 30s preview is public)".into());
    }

    let transcodings = track["media"]["transcodings"].as_array().cloned().unwrap_or_default();
    if transcodings.is_empty() {
        return Err("SoundCloud returned no streams for this track".into());
    }
    let protocol = |transcoding: &serde_json::Value| transcoding["format"]["protocol"].as_str().unwrap_or("").to_string();
    let is_preview = |transcoding: &serde_json::Value| transcoding["snipped"].as_bool() == Some(true);
    // Prefer a direct file; fall back to plain (unencrypted) HLS, which is all many tracks
    // offer now. Encrypted HLS variants are DRM and can't be saved this way.
    let chosen = transcodings
        .iter()
        .filter(|transcoding| !is_preview(transcoding))
        .find(|transcoding| protocol(transcoding) == "progressive")
        .or_else(|| transcodings.iter().filter(|transcoding| !is_preview(transcoding)).find(|transcoding| protocol(transcoding) == "hls"))
        .ok_or_else(|| {
            let available: Vec<String> = transcodings.iter().map(|transcoding| protocol(transcoding)).collect();
            format!("no downloadable stream (only: {})", available.join(", "))
        })?;

    let transcoding_url = chosen["url"].as_str().ok_or("missing transcoding url")?;
    // SoundCloud rejects stream lookups that don't carry the track's own authorization token.
    let mut lookup_url = format!("{transcoding_url}?client_id={client_id}");
    if let Some(token) = track["track_authorization"].as_str() {
        lookup_url.push_str(&format!("&track_authorization={}", percent_encode(token)));
    }
    let resolved: serde_json::Value = ureq::get(&lookup_url).call().map_err(|error| format!("could not resolve stream: {}", describe_error(error)))?.into_json().map_err(|error| format!("bad stream response: {error}"))?;
    let stream_url = resolved["url"].as_str().ok_or("no stream url in response")?;

    let mime = chosen["format"]["mime_type"].as_str().unwrap_or("audio/mpeg");
    let extension = extension_for_mime(mime);
    let path = dest_dir.join(format!("{}.{extension}", sanitize_filename(title)));
    // Write to a temporary name first so a failed download never leaves a truncated file
    // that a later rescan would pick up as a real track.
    let partial_path = path.with_extension(format!("{extension}.part"));
    let result = if protocol(chosen) == "hls" { download_hls(stream_url, &partial_path) } else { download_file(stream_url, &partial_path) };
    match result {
        Ok(()) => fs::rename(&partial_path, &path).map_err(|error| format!("could not write file: {error}")),
        Err(error) => {
            let _ = fs::remove_file(&partial_path);
            Err(error)
        }
    }
}

fn extension_for_mime(mime: &str) -> &'static str {
    if mime.contains("ogg") || mime.contains("opus") {
        "opus"
    } else if mime.contains("mp4") || mime.contains("aac") {
        "m4a"
    } else {
        "mp3"
    }
}

fn download_file(url: &str, path: &Path) -> Result<(), String> {
    let response = ureq::get(url).call().map_err(|error| format!("stream fetch failed: {}", describe_error(error)))?;
    let mut reader = response.into_reader();
    let mut file = fs::File::create(path).map_err(|error| format!("could not write file: {error}"))?;
    io::copy(&mut reader, &mut file).map_err(|error| format!("download failed: {error}"))?;
    Ok(())
}

/// Downloads an HLS media playlist by concatenating its init segment (if any) and every
/// media segment, in order, into one file. Works for SoundCloud's unencrypted MP3, Opus and
/// AAC streams; encrypted playlists are refused.
fn download_hls(playlist_url: &str, path: &Path) -> Result<(), String> {
    let playlist = ureq::get(playlist_url).call().map_err(|error| format!("playlist fetch failed: {}", describe_error(error)))?.into_string().map_err(|error| format!("bad playlist: {error}"))?;
    let segment_urls = parse_hls_segments(&playlist, playlist_url)?;
    if segment_urls.is_empty() {
        return Err("stream playlist has no segments".into());
    }
    let mut file = fs::File::create(path).map_err(|error| format!("could not write file: {error}"))?;
    for url in segment_urls {
        let response = ureq::get(&url).call().map_err(|error| format!("segment fetch failed: {}", describe_error(error)))?;
        io::copy(&mut response.into_reader(), &mut file).map_err(|error| format!("download failed: {error}"))?;
    }
    Ok(())
}

fn parse_hls_segments(playlist: &str, playlist_url: &str) -> Result<Vec<String>, String> {
    let mut urls = Vec::new();
    for line in playlist.lines().map(str::trim) {
        if let Some(key) = line.strip_prefix("#EXT-X-KEY:") {
            if !key.contains("METHOD=NONE") {
                return Err("stream is encrypted (DRM)".into());
            }
        } else if let Some(map) = line.strip_prefix("#EXT-X-MAP:") {
            if let Some(uri) = map.split("URI=\"").nth(1).and_then(|rest| rest.split('"').next()) {
                urls.push(resolve_relative(playlist_url, uri));
            }
        } else if !line.is_empty() && !line.starts_with('#') {
            urls.push(resolve_relative(playlist_url, line));
        }
    }
    Ok(urls)
}

fn resolve_relative(base: &str, reference: &str) -> String {
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return reference.to_string();
    }
    let without_query = base.split('?').next().unwrap_or(base);
    if let Some(rest) = reference.strip_prefix('/') {
        // Absolute path: keep only scheme://host from the base.
        let after_scheme = without_query.find("://").map(|index| index + 3).unwrap_or(0);
        let host_end = without_query[after_scheme..].find('/').map(|index| after_scheme + index).unwrap_or(without_query.len());
        return format!("{}/{rest}", &without_query[..host_end]);
    }
    let dir_end = without_query.rfind('/').map(|index| index + 1).unwrap_or(without_query.len());
    format!("{}{reference}", &without_query[..dir_end])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hls_segments_with_init_map_and_relative_paths() {
        let playlist = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:10,\nseg/1.m4s\n#EXTINF:10,\nhttps://cdn.example/seg2.m4s\n#EXTINF:5,\n/abs/3.m4s\n#EXT-X-ENDLIST\n";
        let urls = parse_hls_segments(playlist, "https://host.example/media/x/playlist.m3u8?sig=1").unwrap();
        assert_eq!(urls, vec!["https://host.example/media/x/init.mp4", "https://host.example/media/x/seg/1.m4s", "https://cdn.example/seg2.m4s", "https://host.example/abs/3.m4s"]);
    }

    #[test]
    fn refuses_encrypted_hls() {
        let playlist = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://x\"\n#EXTINF:10,\n1.ts\n";
        assert!(parse_hls_segments(playlist, "https://h/p.m3u8").is_err());
    }

    #[test]
    fn picks_extension_from_mime() {
        assert_eq!(extension_for_mime("audio/mpeg"), "mp3");
        assert_eq!(extension_for_mime("audio/ogg; codecs=\"opus\""), "opus");
        assert_eq!(extension_for_mime("audio/mp4; codecs=\"mp4a.40.2\""), "m4a");
    }
}

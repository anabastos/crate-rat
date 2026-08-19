//! Public-playlist metadata via SoundCloud's undocumented api-v2, the same approach used by
//! most open-source SoundCloud tools (scdl, yt-dlp, etc.) since SoundCloud's official API
//! registration is manual-approval-only. Only reads public data — no login, no private access.

pub struct SoundCloudTrack {
    pub title: String,
    pub artist: String,
    pub duration_secs: u64,
}

pub struct FetchResult {
    pub name: String,
    pub tracks: Vec<SoundCloudTrack>,
}

pub fn fetch_public_playlist(playlist_link: &str) -> Result<FetchResult, String> {
    let client_id = fetch_public_client_id()?;
    let resolve_url = format!("https://api-v2.soundcloud.com/resolve?url={}&client_id={client_id}", percent_encode(playlist_link.trim()));

    let response: serde_json::Value = ureq::get(&resolve_url).call().map_err(|error| format!("could not resolve that link: {}", describe_error(error)))?.into_json().map_err(|error| format!("bad response: {error}"))?;

    if response["kind"].as_str() != Some("playlist") {
        return Err("that link doesn't point to a playlist".into());
    }
    let name = response["title"].as_str().unwrap_or("SoundCloud Playlist").to_string();
    let raw_tracks = response["tracks"].as_array().cloned().unwrap_or_default();

    let mut tracks = Vec::new();
    let mut stub_ids = Vec::new();
    for track in &raw_tracks {
        if track.get("title").is_some() {
            tracks.push(parse_track(track));
        } else if let Some(id) = track["id"].as_u64() {
            stub_ids.push(id);
        }
    }

    // Large playlists come back with "stub" entries (just an id) for tracks past the first page;
    // fetch those in batches to get their real title/artist/duration.
    for chunk in stub_ids.chunks(50) {
        let ids = chunk.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let url = format!("https://api-v2.soundcloud.com/tracks?ids={ids}&client_id={client_id}");
        if let Ok(response) = ureq::get(&url).call() {
            if let Ok(list) = response.into_json::<Vec<serde_json::Value>>() {
                for track in &list {
                    tracks.push(parse_track(track));
                }
            }
        }
    }

    Ok(FetchResult { name, tracks })
}

fn parse_track(track: &serde_json::Value) -> SoundCloudTrack {
    SoundCloudTrack {
        title: track["title"].as_str().unwrap_or("Unknown track").to_string(),
        artist: track["user"]["username"].as_str().unwrap_or("").to_string(),
        duration_secs: track["duration"].as_u64().unwrap_or(0) / 1000,
    }
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

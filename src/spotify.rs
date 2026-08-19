use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

const REDIRECT_URI: &str = "http://127.0.0.1:8888/callback";
const REDIRECT_URI_ENCODED: &str = "http%3A%2F%2F127.0.0.1%3A8888%2Fcallback";
const SCOPE_ENCODED: &str = "playlist-read-private%20playlist-read-collaborative";

pub struct SpotifyTrack {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: u64,
}

pub struct FetchResult {
    pub name: String,
    pub tracks: Vec<SpotifyTrack>,
    pub refresh_token: String,
}

/// Logs in (if needed) and pulls every track's metadata from a Spotify playlist link.
/// Blocking: may open a browser window and wait for the user to authorize.
pub fn fetch_playlist(client_id: &str, stored_refresh_token: Option<&str>, playlist_link: &str) -> Result<FetchResult, String> {
    let playlist_id = extract_playlist_id(playlist_link).ok_or_else(|| "couldn't find a playlist id in that link".to_string())?;

    let (access_token, refresh_token) = match stored_refresh_token {
        Some(token) => match refresh_access_token(client_id, token) {
            Ok(tokens) => tokens,
            Err(_) => login(client_id)?,
        },
        None => login(client_id)?,
    };

    let name = fetch_playlist_name(&access_token, &playlist_id)?;
    let tracks = fetch_all_tracks(&access_token, &playlist_id)?;
    Ok(FetchResult { name, tracks, refresh_token })
}

fn extract_playlist_id(link: &str) -> Option<String> {
    let trimmed = link.trim();
    if let Some(rest) = trimmed.strip_prefix("spotify:playlist:") {
        return Some(rest.to_string());
    }
    let after_marker = trimmed.split("/playlist/").nth(1)?;
    let id = after_marker.split(['?', '#']).next().unwrap_or(after_marker);
    if id.is_empty() { None } else { Some(id.to_string()) }
}

/// Runs the PKCE login flow (opens a browser, waits for the redirect) and returns
/// (access_token, refresh_token). Useful to establish a session ahead of time, from Settings.
pub fn login(client_id: &str) -> Result<(String, String), String> {
    let mut verifier_bytes = [0u8; 64];
    rand::thread_rng().fill_bytes(&mut verifier_bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());

    // show_dialog=true forces Spotify to always re-show the consent screen instead of silently
    // reusing whatever scopes were granted the first time this app was authorized — otherwise a
    // stale/under-scoped authorization from an earlier attempt can linger indefinitely.
    let auth_url = format!(
        "https://accounts.spotify.com/authorize?client_id={client_id}&response_type=code&redirect_uri={REDIRECT_URI_ENCODED}&code_challenge_method=S256&code_challenge={challenge}&scope={SCOPE_ENCODED}&show_dialog=true"
    );
    open_browser(&auth_url);

    let code = wait_for_callback()?;

    let response: serde_json::Value = ureq::post("https://accounts.spotify.com/api/token")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_form(&[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", REDIRECT_URI), ("client_id", client_id), ("code_verifier", &verifier)])
        .map_err(|error| format!("token exchange failed: {}", describe_error(error)))?
        .into_json()
        .map_err(|error| format!("bad token response: {error}"))?;

    let access_token = response["access_token"].as_str().ok_or("no access_token in response")?.to_string();
    let refresh_token = response["refresh_token"].as_str().ok_or("no refresh_token in response")?.to_string();
    Ok((access_token, refresh_token))
}

fn refresh_access_token(client_id: &str, refresh_token: &str) -> Result<(String, String), String> {
    let response: serde_json::Value = ureq::post("https://accounts.spotify.com/api/token")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", client_id)])
        .map_err(|error| format!("refresh failed: {}", describe_error(error)))?
        .into_json()
        .map_err(|error| format!("bad refresh response: {error}"))?;

    let access_token = response["access_token"].as_str().ok_or("no access_token in refresh response")?.to_string();
    let new_refresh_token = response["refresh_token"].as_str().map(str::to_string).unwrap_or_else(|| refresh_token.to_string());
    Ok((access_token, new_refresh_token))
}

fn open_browser(url: &str) {
    // On Windows, `cmd /C start <url>` breaks because cmd.exe treats `&` (which separates
    // query params) as a command separator, and `explorer.exe <url>` can misinterpret it as a
    // file/folder path instead of launching the default browser. Going through url.dll's
    // FileProtocolHandler (the same entry point `start` itself uses internally) is reliable.
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// Starts a one-shot local server on 127.0.0.1:8888, waits (up to 2 minutes) for Spotify's
/// redirect, and returns the authorization `code` query parameter.
fn wait_for_callback() -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:8888").map_err(|error| format!("could not open local callback server: {error}"))?;
    listener.set_nonblocking(true).map_err(|error| format!("could not configure callback server: {error}"))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err("timed out waiting for the Spotify login (2 min) — did the browser open?".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(error) => return Err(format!("callback server error: {error}")),
        }
    };
    stream.set_nonblocking(false).map_err(|error| format!("callback server error: {error}"))?;
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).map_err(|error| format!("callback read error: {error}"))?;

    let path = request_line.split_whitespace().nth(1).ok_or("malformed callback request")?;
    let query = path.split('?').nth(1).unwrap_or("");
    let code = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("code="))
        .ok_or("no authorization code in callback — did you cancel the login?")?
        .to_string();

    let mut stream = stream;
    let body = "<html><body>RatCrate: Spotify login complete, you can close this tab.</body></html>";
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = stream.write_all(response.as_bytes());

    Ok(code)
}

/// ureq's `{error}` Display doesn't include the response body, which is where Spotify puts the
/// actual reason for 4xx/5xx errors (e.g. `{"error":{"status":403,"message":"..."}}`). Pull
/// that out so failures are diagnosable instead of a bare "status code 403".
fn describe_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            format!("HTTP {code}: {body}")
        }
        other => other.to_string(),
    }
}

fn fetch_playlist_name(access_token: &str, playlist_id: &str) -> Result<String, String> {
    let response: serde_json::Value = ureq::get(&format!("https://api.spotify.com/v1/playlists/{playlist_id}?fields=name"))
        .set("Authorization", &format!("Bearer {access_token}"))
        .call()
        .map_err(|error| format!("could not fetch playlist: {}", describe_error(error)))?
        .into_json()
        .map_err(|error| format!("bad playlist response: {error}"))?;
    Ok(response["name"].as_str().unwrap_or("Spotify Playlist").to_string())
}

fn fetch_all_tracks(access_token: &str, playlist_id: &str) -> Result<Vec<SpotifyTrack>, String> {
    let mut url = format!(
        "https://api.spotify.com/v1/playlists/{playlist_id}/tracks?fields=next,items(track(name,duration_ms,artists(name),album(name)))&limit=50"
    );
    let mut tracks = Vec::new();

    loop {
        let response: serde_json::Value = ureq::get(&url).set("Authorization", &format!("Bearer {access_token}")).call().map_err(|error| format!("could not fetch tracks: {}", describe_error(error)))?.into_json().map_err(|error| format!("bad tracks response: {error}"))?;

        let items = response["items"].as_array().cloned().unwrap_or_default();
        for item in items {
            let track = &item["track"];
            if track.is_null() {
                continue;
            }
            let title = track["name"].as_str().unwrap_or("Unknown track").to_string();
            let artist = track["artists"].as_array().map(|artists| artists.iter().filter_map(|artist| artist["name"].as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            let album = track["album"]["name"].as_str().unwrap_or("").to_string();
            let duration_secs = track["duration_ms"].as_u64().unwrap_or(0) / 1000;
            tracks.push(SpotifyTrack { title, artist, album, duration_secs });
        }

        match response["next"].as_str() {
            Some(next) => url = next.to_string(),
            None => break,
        }
    }

    Ok(tracks)
}

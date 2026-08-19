//! Tidal catalog search via the Client Credentials flow (app-only, no user login) — used both
//! to check whether tracks from a local playlist exist on Tidal, and (combined with the
//! externally-installed `tidal-dl-ng` tool, which handles its own real user login) to actually
//! download matches for playlists imported from Spotify (whose own API has no downloadable
//! stream at all).

use std::path::Path;
use std::process::Command;

pub fn find_tracks(client_id: &str, client_secret: &str, tracks: &[(String, String)]) -> Result<Vec<bool>, String> {
    let access_token = get_app_token(client_id, client_secret)?;
    let mut results = Vec::with_capacity(tracks.len());
    for (title, artist) in tracks {
        results.push(search_track(&access_token, title, artist).unwrap_or(false));
    }
    Ok(results)
}

/// Same search, but returns each match's Tidal track URL (when found) instead of just a bool,
/// so the caller can hand it off to `tidal-dl-ng`.
pub fn find_track_urls(client_id: &str, client_secret: &str, tracks: &[(String, String)]) -> Result<Vec<Option<String>>, String> {
    let access_token = get_app_token(client_id, client_secret)?;
    let mut results = Vec::with_capacity(tracks.len());
    for (title, artist) in tracks {
        results.push(search_track_url(&access_token, title, artist).unwrap_or(None));
    }
    Ok(results)
}

fn search_track_url(access_token: &str, title: &str, artist: &str) -> Result<Option<String>, String> {
    let query = format!("{title} {artist}");
    let url = format!("https://openapi.tidal.com/v2/searchresults/{}?countryCode=US&include=tracks", percent_encode(&query));

    let response: serde_json::Value = ureq::get(&url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Accept", "application/vnd.api+json")
        .call()
        .map_err(|error| format!("search failed: {}", describe_error(error)))?
        .into_json()
        .map_err(|error| format!("bad search response: {error}"))?;

    let direct_id = response["data"]["relationships"]["tracks"]["data"][0]["id"].as_str();
    let included_id = response["included"].as_array().and_then(|list| list.iter().find(|item| item["type"].as_str() == Some("tracks"))).and_then(|item| item["id"].as_str());
    Ok(direct_id.or(included_id).map(|id| format!("https://tidal.com/browse/track/{id}")))
}

/// Downloads one track via the externally-installed `tidal-dl-ng` CLI (must already be
/// installed and logged in with a Tidal subscription — that's a separate, real user login,
/// distinct from the app-only Client ID/Secret used for search above).
pub fn download_via_tidal_dl_ng(track_url: &str, dest_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|error| format!("could not create playlist folder: {error}"))?;
    let output = Command::new("tidal-dl-ng")
        .args(["dl", "-o", &dest_dir.to_string_lossy(), track_url])
        .output()
        .map_err(|error| format!("could not run tidal-dl-ng (is it installed and on PATH?): {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tidal-dl-ng exited with {}: {}", output.status, stderr.trim()));
    }
    Ok(())
}

fn get_app_token(client_id: &str, client_secret: &str) -> Result<String, String> {
    let response: serde_json::Value = ureq::post("https://auth.tidal.com/v1/oauth2/token")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_form(&[("grant_type", "client_credentials"), ("client_id", client_id), ("client_secret", client_secret)])
        .map_err(|error| format!("could not authenticate with Tidal: {}", describe_error(error)))?
        .into_json()
        .map_err(|error| format!("bad token response: {error}"))?;

    response["access_token"].as_str().map(str::to_string).ok_or_else(|| "no access_token in Tidal's response".to_string())
}

fn search_track(access_token: &str, title: &str, artist: &str) -> Result<bool, String> {
    let query = format!("{title} {artist}");
    let url = format!("https://openapi.tidal.com/v2/searchresults/{}?countryCode=US&include=tracks", percent_encode(&query));

    let response: serde_json::Value = ureq::get(&url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Accept", "application/vnd.api+json")
        .call()
        .map_err(|error| format!("search failed: {}", describe_error(error)))?
        .into_json()
        .map_err(|error| format!("bad search response: {error}"))?;

    let has_direct_hits = response["data"]["relationships"]["tracks"]["data"].as_array().is_some_and(|list| !list.is_empty());
    let has_included = response["included"].as_array().is_some_and(|list| list.iter().any(|item| item["type"].as_str() == Some("tracks")));
    Ok(has_direct_hits || has_included)
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

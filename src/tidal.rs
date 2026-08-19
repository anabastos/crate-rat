//! Tidal catalog search via the Client Credentials flow (app-only, no user login) — just used
//! to check whether tracks from a local playlist exist on Tidal, nothing is downloaded.

pub fn find_tracks(client_id: &str, client_secret: &str, tracks: &[(String, String)]) -> Result<Vec<bool>, String> {
    let access_token = get_app_token(client_id, client_secret)?;
    let mut results = Vec::with_capacity(tracks.len());
    for (title, artist) in tracks {
        results.push(search_track(&access_token, title, artist).unwrap_or(false));
    }
    Ok(results)
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

//! Tidal catalog search via the Client Credentials flow (app-only, no user login) — used both
//! to check whether tracks from a local playlist exist on Tidal, and (combined with the
//! externally-installed `tidal-dl-ng` tool, which handles its own real user login) to actually
//! download matches for playlists imported from Spotify (whose own API has no downloadable
//! stream at all).

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Caches artist lookups across a whole batch, since many tracks in a playlist share an artist
/// and the artist-fallback search would otherwise re-fetch the same artist id / track list
/// once per track.
#[derive(Default)]
struct ArtistCache {
    artist_ids: HashMap<String, Option<String>>,
    /// artist id -> (lowercased track title, track id)
    artist_tracks: HashMap<String, Vec<(String, String)>>,
}

pub fn find_tracks(client_id: &str, client_secret: &str, country_code: &str, tracks: &[(String, String)]) -> Result<Vec<bool>, String> {
    let mut access_token = get_app_token(client_id, client_secret, &mut |_| {})?;
    let mut results = Vec::with_capacity(tracks.len());
    for (title, artist) in tracks {
        match search_track(&access_token, country_code, title, artist) {
            Ok(found) => results.push(found),
            // The client-credentials token expires after a while (long batches routinely
            // outlast it); get a fresh one and retry this track once before giving up.
            Err(error) if is_auth_error(&error) => {
                access_token = get_app_token(client_id, client_secret, &mut |_| {})?;
                results.push(search_track(&access_token, country_code, title, artist).unwrap_or(false));
            }
            Err(_) => results.push(false),
        }
    }
    Ok(results)
}

/// Same search as `find_tracks`, but returns each match's Tidal track URL (when found) instead
/// of just a bool, so the caller can hand it off to `tidal-dl-ng`. Calls `on_progress(1-based
/// index, title)` right before each track is looked up, so a caller polling from another thread
/// can show which track is in flight. Calls `on_log(title, line)` for every Tidal API request
/// made while resolving that track, so the caller can show a full request-by-request trace.
pub fn find_track_urls(client_id: &str, client_secret: &str, country_code: &str, tracks: &[(String, String)], mut on_progress: impl FnMut(usize, &str), mut on_log: impl FnMut(&str, &str)) -> Result<Vec<Option<String>>, String> {
    let mut access_token = get_app_token(client_id, client_secret, &mut |line| on_log("auth", line))?;
    let mut cache = ArtistCache::default();
    let mut results = Vec::with_capacity(tracks.len());
    for (index, (title, artist)) in tracks.iter().enumerate() {
        on_progress(index + 1, title);
        let mut log = |line: &str| on_log(title, line);
        match search_track_url(&access_token, country_code, title, artist, &mut cache, &mut log) {
            Ok(url) => results.push(url),
            // Same expiring-token situation as above: refresh once and retry this track
            // in place, instead of aborting the whole batch and making the user press D again.
            Err(error) if is_auth_error(&error) => {
                log(&format!("auth error, refreshing token: {error}"));
                access_token = get_app_token(client_id, client_secret, &mut log)?;
                results.push(search_track_url(&access_token, country_code, title, artist, &mut cache, &mut log).unwrap_or(None));
            }
            Err(error) => {
                log(&format!("search error: {error}"));
                results.push(None);
            }
        }
    }
    Ok(results)
}

/// Whether a search failure looks like an auth/authorization problem (bad or expired
/// credentials, insufficient scope) rather than a per-track "no match" — these are worth
/// aborting the whole batch for and surfacing immediately, since every remaining request
/// would fail the same way.
fn is_auth_error(error: &str) -> bool {
    error.contains("HTTP 401") || error.contains("HTTP 403")
}

/// Tidal's search API enforces a token-bucket rate limit (small burst, ~1 sustained request per
/// several seconds) — hammering it sequentially reliably produces 429s. Retry that many times,
/// honoring `Retry-After` when present and falling back to a fixed backoff otherwise.
const RATE_LIMIT_RETRIES: u32 = 5;
const RATE_LIMIT_FALLBACK_BACKOFF: Duration = Duration::from_secs(3);
/// Proactive spacing between requests to openapi.tidal.com, matched to the documented bucket
/// (10 tokens, 5 spent per request, refilling 1/sec — i.e. ~1 sustainable request per 5s) so we
/// mostly avoid 429s instead of just retrying after the fact.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(5);
static LAST_REQUEST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

fn throttle() {
    let mut last = LAST_REQUEST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(previous) = *last {
        let elapsed = previous.elapsed();
        if elapsed < MIN_REQUEST_INTERVAL {
            std::thread::sleep(MIN_REQUEST_INTERVAL - elapsed);
        }
    }
    *last = Some(Instant::now());
}

/// GETs `url` with bearer auth, logging the exact request line and the exact response line
/// (status + full body, whether it succeeded or not) through `log` before returning the parsed
/// JSON — so a caller can show a complete request/response trace, not just a summary.
fn get_json(url: &str, access_token: &str, log: &mut dyn FnMut(&str)) -> Result<serde_json::Value, String> {
    for attempt in 0..=RATE_LIMIT_RETRIES {
        throttle();
        log(&format!("GET {url}"));
        match ureq::get(url).set("Authorization", &format!("Bearer {access_token}")).set("Accept", "application/vnd.api+json").call() {
            Ok(response) => {
                let status = response.status();
                log(&format!("  <- {status}"));
                let body = match response.into_string() {
                    Ok(body) => body,
                    Err(error) => return Err(format!("HTTP {status}: bad response body: {error}")),
                };
                log(&format!("     {body}"));
                return serde_json::from_str(&body).map_err(|error| format!("HTTP {status}: bad json ({error}): {body}"));
            }
            Err(ureq::Error::Status(429, response)) => {
                let retry_after = response.header("Retry-After").and_then(|value| value.parse::<u64>().ok()).map(Duration::from_secs).unwrap_or(RATE_LIMIT_FALLBACK_BACKOFF);
                let body = response.into_string().unwrap_or_default();
                if attempt == RATE_LIMIT_RETRIES {
                    log(&format!("  <- 429 {body} (out of retries, giving up)"));
                    return Err(format!("HTTP 429: {body}"));
                }
                log(&format!("  <- 429 {body} (rate limited, retrying in {}s)", retry_after.as_secs()));
                std::thread::sleep(retry_after);
            }
            Err(ureq::Error::Status(code, response)) => {
                let body = response.into_string().unwrap_or_default();
                log(&format!("  <- {code} {body}"));
                return Err(format!("HTTP {code}: {body}"));
            }
            Err(other) => {
                log(&format!("  <- error: {other}"));
                return Err(other.to_string());
            }
        }
    }
    unreachable!()
}

fn search_track_url(access_token: &str, country_code: &str, title: &str, artist: &str, cache: &mut ArtistCache, log: &mut dyn FnMut(&str)) -> Result<Option<String>, String> {
    if let Some(url) = search_track_url_direct(access_token, country_code, title, artist, log)? {
        return Ok(Some(url));
    }
    // The combined "title artist" query sometimes misses (odd punctuation, "feat." formatting,
    // remaster suffixes, etc). Fall back to finding the artist first, then matching the track
    // title against that artist's own catalog — a looser but often more forgiving path.
    search_track_url_via_artist(access_token, country_code, title, artist, cache, log)
}

fn search_track_url_direct(access_token: &str, country_code: &str, title: &str, artist: &str, log: &mut dyn FnMut(&str)) -> Result<Option<String>, String> {
    let query = format!("{title} {artist}");
    // The query is a `filter[query]` QUERY PARAMETER on the /searchResults collection endpoint —
    // NOT free text embedded in the URL path as a resource id (that's what was producing
    // INVALID_RESOURCE_ID / "Invalid resource ID" on every single search, confirmed against the
    // real Swagger docs for GET /searchResults).
    let url = format!("https://openapi.tidal.com/v2/searchResults?filter%5Bquery%5D={}&countryCode={country_code}&include=tracks", percent_encode(&query));
    let response = get_json(&url, access_token, log)?;

    // The endpoint "returns a collection containing exactly one search results resource", so
    // `data` is an array with one element, not a bare object.
    let direct_id = response["data"][0]["relationships"]["tracks"]["data"][0]["id"].as_str();
    let included_id = response["included"].as_array().and_then(|list| list.iter().find(|item| item["type"].as_str() == Some("tracks"))).and_then(|item| item["id"].as_str());
    Ok(direct_id.or(included_id).map(|id| format!("https://tidal.com/browse/track/{id}")))
}

fn search_track_url_via_artist(access_token: &str, country_code: &str, title: &str, artist: &str, cache: &mut ArtistCache, log: &mut dyn FnMut(&str)) -> Result<Option<String>, String> {
    if artist.trim().is_empty() {
        return Ok(None);
    }
    let Some(artist_id) = search_artist_id(access_token, country_code, artist, cache, log)? else {
        log(&format!("no artist id found for artist=\"{artist}\""));
        return Ok(None);
    };
    log(&format!("resolved artist_id={artist_id} for artist=\"{artist}\""));
    let tracks = artist_tracks(access_token, country_code, &artist_id, cache, log)?;

    let target = title.trim().to_lowercase();
    let matched_id = tracks.iter().find(|(found, _id)| found.contains(&target) || target.contains(found.as_str())).map(|(_found, id)| id.clone());
    log(&match &matched_id {
        Some(id) => format!("matched track_id={id} for title=\"{title}\" in artist's catalog ({} tracks)", tracks.len()),
        None => format!("no title match for \"{title}\" among {} tracks in artist's catalog", tracks.len()),
    });

    Ok(matched_id.map(|id| format!("https://tidal.com/browse/track/{id}")))
}

fn search_artist_id(access_token: &str, country_code: &str, artist: &str, cache: &mut ArtistCache, log: &mut dyn FnMut(&str)) -> Result<Option<String>, String> {
    if let Some(cached) = cache.artist_ids.get(artist) {
        log(&format!("(cached artist id for \"{artist}\")"));
        return Ok(cached.clone());
    }

    let url = format!("https://openapi.tidal.com/v2/searchResults?filter%5Bquery%5D={}&countryCode={country_code}&include=artists", percent_encode(artist));
    let response = get_json(&url, access_token, log)?;

    let direct_id = response["data"][0]["relationships"]["artists"]["data"][0]["id"].as_str();
    let included_id = response["included"].as_array().and_then(|list| list.iter().find(|item| item["type"].as_str() == Some("artists"))).and_then(|item| item["id"].as_str());
    let artist_id = direct_id.or(included_id).map(str::to_string);

    cache.artist_ids.insert(artist.to_string(), artist_id.clone());
    Ok(artist_id)
}

/// Every track title (lowercased) + id for an artist, fetched once per artist per batch.
fn artist_tracks(access_token: &str, country_code: &str, artist_id: &str, cache: &mut ArtistCache, log: &mut dyn FnMut(&str)) -> Result<Vec<(String, String)>, String> {
    if let Some(cached) = cache.artist_tracks.get(artist_id) {
        log(&format!("(cached track list for artist {artist_id})"));
        return Ok(cached.clone());
    }

    let url = format!("https://openapi.tidal.com/v2/artists/{artist_id}?countryCode={country_code}&include=tracks");
    let response = get_json(&url, access_token, log)?;

    let tracks: Vec<(String, String)> = response["included"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"].as_str() == Some("tracks"))
        .filter_map(|item| Some((item["attributes"]["title"].as_str()?.trim().to_lowercase(), item["id"].as_str()?.to_string())))
        .collect();

    cache.artist_tracks.insert(artist_id.to_string(), tracks.clone());
    Ok(tracks)
}

/// How long a single-track `tidal-dl-ng` invocation gets before we assume it's stuck (e.g. hung
/// waiting on a prompt it'll never receive, since stdin is closed) and kill it.
const TIDAL_DL_NG_TIMEOUT: Duration = Duration::from_secs(180);

/// A whole-playlist `dl` gets a much longer leash than a single track, since one invocation has
/// to work through every track in the playlist before it exits.
const TIDAL_DL_NG_PLAYLIST_TIMEOUT: Duration = Duration::from_secs(60 * 30);

/// One track as listed in a Tidal playlist's own tracklist (as opposed to a search match).
pub struct PlaylistTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
}

pub struct PlaylistInfo {
    pub name: String,
    pub tracks: Vec<PlaylistTrack>,
}

/// Looks up a Tidal playlist's name and full tracklist from its link
/// (`https://tidal.com/browse/playlist/<uuid>` or `https://tidal.com/playlist/<uuid>`, with or
/// without a query string) via the same app-only Client Credentials flow used for catalog search.
/// The tracklist is what lets a linked playlist know how many tracks it *should* have and which
/// ones are still missing locally — `tidal-dl-ng` handles the actual download, this is just for
/// bookkeeping. If the name resolves but the tracklist doesn't parse (API shape surprise, empty
/// playlist, etc.), this still succeeds with an empty tracklist rather than failing the whole
/// link — the count/missing-tracking is a bonus, not a requirement for linking to work.
pub fn fetch_playlist(client_id: &str, client_secret: &str, country_code: &str, playlist_url: &str) -> Result<PlaylistInfo, String> {
    let playlist_id = extract_playlist_id(playlist_url).ok_or_else(|| format!("could not find a playlist id in \"{playlist_url}\""))?;
    let access_token = get_app_token(client_id, client_secret, &mut |_| {})?;

    // `items.artists` (not just `items`) so each included track also gets a `relationships.
    // artists` pointer and the artist resources themselves land in `included` too — a track on
    // its own has no artist name at all, just its own attributes.
    let mut next_url = Some(format!("https://openapi.tidal.com/v2/playlists/{playlist_id}?countryCode={country_code}&include=items.artists"));
    let mut name: Option<String> = None;
    let mut tracks = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();
    // The `items` relationship (and the tracks it pulls into `included`) comes back paginated —
    // one response only had the first 20 of this playlist's 37 tracks — so keep following
    // `links.next` until Tidal stops giving us one. Capped so a malformed/cyclic link can't loop
    // forever.
    for page in 0..200 {
        let Some(url) = next_url.take() else { break };
        let response = match get_json(&url, &access_token, &mut |_| {}) {
            Ok(response) => response,
            // The first page failing means the playlist/link itself is bad — a real error. A
            // later page failing shouldn't blow away the tracks already collected — stop
            // paginating and return what we have instead.
            Err(error) if page == 0 => return Err(error),
            Err(_) => break,
        };
        if name.is_none() {
            name = response["data"]["attributes"]["name"].as_str().map(str::to_string);
        }

        let artist_names: std::collections::HashMap<String, String> = response["included"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item["type"].as_str() == Some("artists"))
            .filter_map(|item| Some((item["id"].as_str()?.to_string(), item["attributes"]["name"].as_str()?.to_string())))
            .collect();
        for item in response["included"].as_array().into_iter().flatten().filter(|item| item["type"].as_str() == Some("tracks")) {
            let Some(id) = item["id"].as_str() else { continue };
            if !seen_ids.insert(id.to_string()) {
                continue;
            }
            let Some(title) = item["attributes"]["title"].as_str() else { continue };
            let artist = item["relationships"]["artists"]["data"][0]["id"].as_str().and_then(|artist_id| artist_names.get(artist_id)).cloned().unwrap_or_default();
            tracks.push(PlaylistTrack { id: id.to_string(), title: title.to_string(), artist });
        }

        // Confirmed against the real API: this link is a *relative path missing the `/v2`
        // prefix* (e.g. `/playlists/<id>/relationships/items?countryCode=...&page[cursor]=...`)
        // and never carries its own `include` param — resolve both, or the next page 404s and
        // (even once fixed) comes back as bare track ids with no title/artist at all.
        let next = response["data"]["relationships"]["items"]["links"]["next"].as_str().or_else(|| response["links"]["next"].as_str());
        next_url = next.map(|next| {
            let mut resolved = resolve_tidal_url(next);
            if !resolved.contains("include=") {
                resolved.push_str("&include=items.artists");
            }
            resolved
        });
    }

    let name = name.ok_or_else(|| "Tidal's response had no playlist name".to_string())?;
    Ok(PlaylistInfo { name, tracks })
}

/// A JSON:API `links.next` value from Tidal is a path relative to `openapi.tidal.com` but,
/// confirmed against the real API, omits the `/v2` every other endpoint here uses — add it back.
fn resolve_tidal_url(next: &str) -> String {
    if next.starts_with("http") {
        next.to_string()
    } else if let Some(rest) = next.strip_prefix('/') {
        format!("https://openapi.tidal.com/v2/{rest}")
    } else {
        format!("https://openapi.tidal.com/v2/{next}")
    }
}

/// Builds the same `tidal.com/browse/track/<id>` URL shape `tidal-dl-ng` expects, from a track
/// id already known (e.g. from a playlist's own fetched tracklist) — no search needed.
pub fn track_url(id: &str) -> String {
    format!("https://tidal.com/browse/track/{id}")
}

fn extract_playlist_id(playlist_url: &str) -> Option<String> {
    let without_query = playlist_url.split(['?', '#']).next().unwrap_or(playlist_url);
    without_query.trim_end_matches('/').rsplit('/').next().filter(|segment| !segment.is_empty()).map(str::to_string)
}

/// Points `tidal-dl-ng` at the right output folder for this playlist and makes it drop files
/// directly there instead of nested under its own subfolder. The CLI no longer takes an `-o`/
/// output flag on `dl` (that option is gone in the currently installed version — passing it
/// fails with "No such option: -o") — both of these are persistent config values instead, set
/// once via `tidal-dl-ng cfg <key> <value>` before downloading:
/// - `download_base_path` — the destination folder.
/// - `format_track` — defaults to `"Tracks/{artist_name} - {track_title}{track_explicit}"`,
///   which nests every track under a `Tracks/` subfolder of `download_base_path`. That subfolder
///   is invisible to our own sync scan (which only looks at files directly inside the playlist
///   folder), so downloads silently never counted as synced. Stripping the `Tracks/` prefix
///   makes files land straight in `download_base_path`.
/// - `format_playlist` — same deal but for `dl <playlist_url>`: defaults to nesting under
///   `Playlists/{playlist_title}/...`, which (since `download_base_path` is already the specific
///   playlist's own folder) produced a redundant `<playlist folder>/Playlists/<playlist
///   name>/...` on disk instead of files landing directly in the playlist folder. Stripped for
///   the same reason as `format_track`.
/// - `extract_flac` — set to `False` so lossless tracks stay in their original MP4/M4A container
///   instead of being extracted to standalone `.flac` (which needs FFmpeg, not always installed).
pub fn set_tidal_dl_ng_download_path(dest_dir: &Path, mut on_line: impl FnMut(&str)) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|error| format!("could not create playlist folder: {error}"))?;
    let dest_arg = dest_dir.to_string_lossy().to_string();
    run_tidal_dl_ng(&["cfg", "download_base_path", &dest_arg], TIDAL_DL_NG_TIMEOUT, &mut on_line)?;
    run_tidal_dl_ng(&["cfg", "format_track", "{artist_name} - {track_title}{track_explicit}"], TIDAL_DL_NG_TIMEOUT, &mut on_line)?;
    run_tidal_dl_ng(&["cfg", "format_playlist", "{artist_name} - {track_title}{track_explicit}"], TIDAL_DL_NG_TIMEOUT, &mut on_line)?;
    run_tidal_dl_ng(&["cfg", "extract_flac", "False"], TIDAL_DL_NG_TIMEOUT, &mut on_line)
}

/// Downloads one track via the externally-installed `tidal-dl-ng` CLI (must already be
/// installed and logged in with a Tidal subscription — that's a separate, real user login,
/// distinct from the app-only Client ID/Secret used for search above). Calls `on_line` for every
/// line of stdout/stderr as it's produced, so a caller can stream progress live instead of only
/// seeing output after the process exits. Call `set_tidal_dl_ng_download_path` first — this just
/// downloads to whatever `download_base_path` is currently configured to.
pub fn download_via_tidal_dl_ng(track_url: &str, on_line: impl FnMut(&str)) -> Result<(), String> {
    run_tidal_dl_ng(&["dl", track_url], TIDAL_DL_NG_TIMEOUT, on_line)
}

/// Downloads an entire Tidal playlist in one `tidal-dl-ng dl <playlist_url>` call — the CLI
/// resolves the playlist's own track list itself, so no separate track lookup is needed. Call
/// `set_tidal_dl_ng_download_path` first, same as `download_via_tidal_dl_ng`.
pub fn download_playlist_via_tidal_dl_ng(playlist_url: &str, on_line: impl FnMut(&str)) -> Result<(), String> {
    run_tidal_dl_ng(&["dl", playlist_url], TIDAL_DL_NG_PLAYLIST_TIMEOUT, on_line)
}

/// `tidal-dl-ng` prints exception tracebacks via `rich`, which (besides the actual error)
/// dumps every local variable in every stack frame as its own line — `file_template = '...'`,
/// `fn_logger = <LoggerWrapped object at 0x...>`, plus blank/box-drawing filler — dozens of
/// near-unreadable lines for one real error. This collapses consecutive lines that look like
/// that dump into a single placeholder, so the log stays scannable; the actual exception message
/// (never shaped like `name = value`) still comes through untouched.
struct NoiseFilter {
    hidden: usize,
}

impl NoiseFilter {
    fn new() -> Self {
        NoiseFilter { hidden: 0 }
    }

    fn filter(&mut self, line: &str, on_line: &mut dyn FnMut(&str)) {
        if is_locals_dump_line(line) {
            self.hidden += 1;
            return;
        }
        self.flush(on_line);
        on_line(line);
    }

    fn flush(&mut self, on_line: &mut dyn FnMut(&str)) {
        if self.hidden > 0 {
            on_line(&format!("  … ({} internal debug line(s) hidden)", self.hidden));
            self.hidden = 0;
        }
    }
}

/// A rich locals-dump line, once its leading box-drawing border is stripped, looks like a bare
/// `name = value` Python assignment (or is just whitespace/border — a blank panel row). Real log
/// messages from the CLI are sentences, never shaped like that.
fn is_locals_dump_line(line: &str) -> bool {
    let trimmed = line.trim_start_matches(|c: char| c.is_whitespace() || "│┃|┌└├─╭╰┐┘".contains(c)).trim();
    if trimmed.is_empty() {
        return true;
    }
    let Some(eq_pos) = trimmed.find(" = ") else { return false };
    let name = &trimmed[..eq_pos];
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn run_tidal_dl_ng(args: &[&str], timeout: Duration, mut on_line: impl FnMut(&str)) -> Result<(), String> {
    on_line(&format!("$ tidal-dl-ng {}", args.join(" ")));
    let mut child = Command::new("tidal-dl-ng")
        .args(args)
        // Closed, not inherited: if the CLI ever prompts for input (overwrite confirmation,
        // first-run login, etc), it gets EOF immediately instead of blocking forever — this app
        // has no way to answer an interactive prompt since it owns the terminal in raw mode.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not run tidal-dl-ng (is it installed and on PATH?): {error}"))?;

    // Two reader threads drain stdout/stderr into a shared channel as lines arrive (both — not
    // just one — because reading only one pipe while the other fills up is a classic way to
    // deadlock a child process). The main flow below just polls that channel plus the child's
    // exit status, so lines show up live instead of only after the process ends.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let stdout_tx = tx.clone();
    let stdout_handle = child.stdout.take().map(|stdout| std::thread::spawn(move || for line in BufReader::new(stdout).lines().map_while(Result::ok) { let _ = stdout_tx.send(line); }));
    let stderr_handle = child.stderr.take().map(|stderr| std::thread::spawn(move || for line in BufReader::new(stderr).lines().map_while(Result::ok) { let _ = tx.send(line); }));

    let mut noise = NoiseFilter::new();
    let deadline = Instant::now() + timeout;
    let status = loop {
        while let Ok(line) = rx.try_recv() {
            noise.filter(&line, &mut on_line);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => break None,
        }
    };
    while let Ok(line) = rx.try_recv() {
        noise.filter(&line, &mut on_line);
    }
    noise.flush(&mut on_line);
    if let Some(handle) = stdout_handle {
        let _ = handle.join();
    }
    if let Some(handle) = stderr_handle {
        let _ = handle.join();
    }

    match status {
        Some(status) if status.success() => Ok(()),
        Some(status) => Err(format!("tidal-dl-ng exited with {status}")),
        None => Err(format!("tidal-dl-ng timed out after {} minutes (likely stuck waiting on a prompt)", timeout.as_secs() / 60)),
    }
}

fn get_app_token(client_id: &str, client_secret: &str, log: &mut dyn FnMut(&str)) -> Result<String, String> {
    throttle();
    log("POST https://auth.tidal.com/v1/oauth2/token (grant_type=client_credentials)");
    let result = ureq::post("https://auth.tidal.com/v1/oauth2/token").set("Content-Type", "application/x-www-form-urlencoded").send_form(&[("grant_type", "client_credentials"), ("client_id", client_id), ("client_secret", client_secret)]);
    let response: serde_json::Value = match result {
        Ok(response) => {
            let status = response.status();
            log(&format!("  <- {status}"));
            let body = match response.into_string() {
                Ok(body) => body,
                Err(error) => return Err(format!("HTTP {status}: bad response body: {error}")),
            };
            // Never log the access_token itself — only that a token was granted.
            log("     (token granted)");
            serde_json::from_str(&body).map_err(|error| format!("HTTP {status}: bad token response: {error}"))?
        }
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            log(&format!("  <- {code} {body}"));
            return Err(format!("could not authenticate with Tidal: HTTP {code}: {body}"));
        }
        Err(other) => {
            log(&format!("  <- error: {other}"));
            return Err(format!("could not authenticate with Tidal: {other}"));
        }
    };

    response["access_token"].as_str().map(str::to_string).ok_or_else(|| "no access_token in Tidal's response".to_string())
}

fn search_track(access_token: &str, country_code: &str, title: &str, artist: &str) -> Result<bool, String> {
    let query = format!("{title} {artist}");
    let url = format!("https://openapi.tidal.com/v2/searchResults?filter%5Bquery%5D={}&countryCode={country_code}&include=tracks", percent_encode(&query));

    let response: serde_json::Value = ureq::get(&url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Accept", "application/vnd.api+json")
        .call()
        .map_err(|error| format!("search failed: {}", describe_error(error)))?
        .into_json()
        .map_err(|error| format!("bad search response: {error}"))?;

    let has_direct_hits = response["data"][0]["relationships"]["tracks"]["data"].as_array().is_some_and(|list| !list.is_empty());
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

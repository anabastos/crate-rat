//! Looks up genre tags for library tracks online, on a long-lived background thread fed from a
//! shared queue — so new tracks can be queued (or bumped to the front) at any time without
//! restarting anything. MusicBrainz is used by default (no key needed, 1 request/second);
//! with a Last.fm API key set, Last.fm's much denser tags are used instead.

use std::{
    collections::{HashMap, VecDeque},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

const USER_AGENT: &str = "crate-rat/0.1 ( https://github.com/anabastos/crate-rat )";
const MUSICBRAINZ_INTERVAL: Duration = Duration::from_millis(1100);
const LASTFM_INTERVAL: Duration = Duration::from_millis(250);
const MAX_TAGS: usize = 6;
const OFFLINE_BACKOFF: Duration = Duration::from_secs(60);

/// Tags nobody wants as a genre.
const JUNK_TAGS: &[&str] = &["seen live", "favorites", "favourite", "favorite", "my favorites", "awesome", "love", "beautiful", "albums i own", "spotify", "under 2000 listeners", "check out", "fip"];

pub struct TagJob {
    pub id: String,
    pub artist: String,
    pub title: String,
}

pub enum TagUpdate {
    Started(String),
    /// `Ok(Some((tags, source)))` found, `Ok(None)` nothing found, `Err` network/API trouble.
    Finished(String, Result<Option<(Vec<String>, &'static str)>, String>),
}

pub struct TagWorker {
    queue: Arc<Mutex<VecDeque<TagJob>>>,
    lastfm_key: Arc<Mutex<Option<String>>>,
    pub updates: mpsc::Receiver<TagUpdate>,
}

impl TagWorker {
    pub fn spawn(lastfm_key: Option<String>) -> Self {
        let queue: Arc<Mutex<VecDeque<TagJob>>> = Arc::default();
        let lastfm_key = Arc::new(Mutex::new(lastfm_key));
        let (tx, updates) = mpsc::channel();
        let worker_queue = Arc::clone(&queue);
        let worker_key = Arc::clone(&lastfm_key);
        thread::spawn(move || {
            let mut artist_cache: HashMap<String, Vec<String>> = HashMap::new();
            let mut consecutive_errors = 0u32;
            loop {
                // Probably offline or rate-limited: back off instead of burning through the
                // whole queue with failures.
                if consecutive_errors >= 5 {
                    thread::sleep(OFFLINE_BACKOFF);
                    consecutive_errors = 0;
                }
                let job = worker_queue.lock().ok().and_then(|mut queue| queue.pop_front());
                let Some(job) = job else {
                    thread::sleep(Duration::from_millis(400));
                    continue;
                };
                if tx.send(TagUpdate::Started(job.id.clone())).is_err() {
                    return;
                }
                let key = worker_key.lock().ok().and_then(|key| key.clone());
                let result = lookup(&job, key.as_deref(), &mut artist_cache);
                consecutive_errors = if result.is_err() { consecutive_errors + 1 } else { 0 };
                if tx.send(TagUpdate::Finished(job.id, result)).is_err() {
                    return;
                }
            }
        });
        TagWorker { queue, lastfm_key, updates }
    }

    pub fn push_back(&self, job: TagJob) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.push_back(job);
        }
    }

    /// Jumps the queue — for a track the user explicitly asked to refresh.
    pub fn push_front(&self, job: TagJob) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.retain(|queued| queued.id != job.id);
            queue.push_front(job);
        }
    }

    pub fn set_lastfm_key(&self, key: Option<String>) {
        if let Ok(mut current) = self.lastfm_key.lock() {
            *current = key;
        }
    }
}

fn lookup(job: &TagJob, lastfm_key: Option<&str>, artist_cache: &mut HashMap<String, Vec<String>>) -> Result<Option<(Vec<String>, &'static str)>, String> {
    let (source, interval): (&'static str, Duration) = if lastfm_key.is_some() { ("last.fm", LASTFM_INTERVAL) } else { ("musicbrainz", MUSICBRAINZ_INTERVAL) };
    let track_tags = match lastfm_key {
        Some(key) => lastfm_track_tags(key, &job.artist, &job.title),
        None => musicbrainz_recording_tags(&job.artist, &job.title),
    };
    thread::sleep(interval);
    let track_tags = clean_tags(track_tags?, &job.artist);
    if !track_tags.is_empty() {
        return Ok(Some((track_tags, source)));
    }
    if job.artist.trim().is_empty() {
        return Ok(None);
    }

    // Fall back to the artist's tags — much better coverage than per-recording tags.
    let artist_key = job.artist.trim().to_lowercase();
    let artist_tags = match artist_cache.get(&artist_key) {
        Some(tags) => tags.clone(),
        None => {
            let fetched = match lastfm_key {
                Some(key) => lastfm_artist_tags(key, &job.artist),
                None => musicbrainz_artist_tags(&job.artist),
            };
            thread::sleep(interval);
            let fetched = clean_tags(fetched?, &job.artist);
            artist_cache.insert(artist_key, fetched.clone());
            fetched
        }
    };
    Ok((!artist_tags.is_empty()).then_some((artist_tags, source)))
}

/// (tag, weight) pairs -> the strongest few, lowercased, junk and artist-name tags removed.
fn clean_tags(mut tags: Vec<(String, i64)>, artist: &str) -> Vec<String> {
    tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let artist = artist.trim().to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for (tag, weight) in tags {
        let tag = tag.trim().to_lowercase();
        if tag.is_empty() || weight < 1 || tag == artist || JUNK_TAGS.contains(&tag.as_str()) || tag.chars().all(|character| character.is_ascii_digit() || character == 's') {
            continue;
        }
        if !out.contains(&tag) {
            out.push(tag);
        }
        if out.len() >= MAX_TAGS {
            break;
        }
    }
    out
}

fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let response = ureq::get(url).set("User-Agent", USER_AGENT).timeout(Duration::from_secs(20)).call();
    match response {
        Ok(response) => response.into_json().map_err(|error| format!("bad response: {error}")),
        // 404 = the service doesn't know this track/artist: not an error, just no tags.
        Err(ureq::Error::Status(404, _)) => Ok(serde_json::Value::Null),
        Err(ureq::Error::Status(code, _)) => Err(format!("HTTP {code}")),
        Err(error) => Err(error.to_string()),
    }
}

/// Escapes Lucene query syntax characters for MusicBrainz search.
fn lucene_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if "+-&|!(){}[]^\"~*?:\\/".contains(character) {
            out.push('\\');
        }
        out.push(character);
    }
    out
}

fn musicbrainz_recording_tags(artist: &str, title: &str) -> Result<Vec<(String, i64)>, String> {
    let mut query = format!("recording:\"{}\"", lucene_escape(title));
    if !artist.trim().is_empty() {
        query.push_str(&format!(" AND artist:\"{}\"", lucene_escape(artist)));
    }
    let url = format!("https://musicbrainz.org/ws/2/recording?query={}&fmt=json&limit=5", percent_encode(&query));
    Ok(parse_musicbrainz_recordings(&get_json(&url)?))
}

fn parse_musicbrainz_recordings(response: &serde_json::Value) -> Vec<(String, i64)> {
    let Some(recordings) = response["recordings"].as_array() else { return Vec::new() };
    // Several recordings of the same song (album/single/compilation) — merge the tags of the
    // good matches rather than betting on the first one having any.
    let mut merged: HashMap<String, i64> = HashMap::new();
    for recording in recordings.iter().filter(|recording| recording["score"].as_i64().unwrap_or(0) >= 85) {
        for (tag, count) in parse_tag_array(&recording["tags"]).into_iter().chain(parse_tag_array(&recording["genres"])) {
            *merged.entry(tag).or_default() += count.max(1);
        }
    }
    merged.into_iter().collect()
}

fn musicbrainz_artist_tags(artist: &str) -> Result<Vec<(String, i64)>, String> {
    let query = format!("artist:\"{}\"", lucene_escape(artist));
    let url = format!("https://musicbrainz.org/ws/2/artist?query={}&fmt=json&limit=1", percent_encode(&query));
    let response = get_json(&url)?;
    let Some(best) = response["artists"].as_array().and_then(|artists| artists.first()) else { return Ok(Vec::new()) };
    if best["score"].as_i64().unwrap_or(0) < 90 {
        return Ok(Vec::new());
    }
    Ok(parse_tag_array(&best["tags"]))
}

fn parse_tag_array(tags: &serde_json::Value) -> Vec<(String, i64)> {
    tags.as_array().map_or_else(Vec::new, |tags| tags.iter().filter_map(|tag| Some((tag["name"].as_str()?.to_string(), tag["count"].as_i64().unwrap_or(1)))).collect())
}

fn lastfm_track_tags(key: &str, artist: &str, title: &str) -> Result<Vec<(String, i64)>, String> {
    let url = format!("https://ws.audioscrobbler.com/2.0/?method=track.gettoptags&artist={}&track={}&autocorrect=1&api_key={}&format=json", percent_encode(artist), percent_encode(title), percent_encode(key));
    Ok(parse_lastfm_tags(&get_json(&url)?))
}

fn lastfm_artist_tags(key: &str, artist: &str) -> Result<Vec<(String, i64)>, String> {
    let url = format!("https://ws.audioscrobbler.com/2.0/?method=artist.gettoptags&artist={}&autocorrect=1&api_key={}&format=json", percent_encode(artist), percent_encode(key));
    Ok(parse_lastfm_tags(&get_json(&url)?))
}

fn parse_lastfm_tags(response: &serde_json::Value) -> Vec<(String, i64)> {
    // Last.fm weights are 0–100 relative to the top tag; below ~10 it's mostly noise.
    response["toptags"]["tag"].as_array().map_or_else(Vec::new, |tags| {
        tags.iter().filter_map(|tag| Some((tag["name"].as_str()?.to_string(), tag["count"].as_i64().unwrap_or(0)))).filter(|(_, count)| *count >= 10).collect()
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_musicbrainz_recordings() {
        let response = serde_json::json!({"recordings": [
            {"score": 100, "tags": [{"count": 3, "name": "IDM"}, {"count": 1, "name": "electronic"}]},
            {"score": 95, "tags": [{"count": 1, "name": "electronic"}]},
            {"score": 40, "tags": [{"count": 9, "name": "polka"}]},
        ]});
        let tags = clean_tags(parse_musicbrainz_recordings(&response), "Aphex Twin");
        assert_eq!(tags, vec!["idm".to_string(), "electronic".to_string()]);
    }

    #[test]
    fn parses_lastfm_and_drops_junk() {
        let response = serde_json::json!({"toptags": {"tag": [
            {"name": "Techno", "count": 100}, {"name": "seen live", "count": 80}, {"name": "Jeff Mills", "count": 50}, {"name": "detroit techno", "count": 40}, {"name": "90s", "count": 30}, {"name": "rare", "count": 2},
        ]}});
        assert_eq!(clean_tags(parse_lastfm_tags(&response), "Jeff Mills"), vec!["techno".to_string(), "detroit techno".to_string()]);
    }

    #[test]
    fn escapes_lucene() {
        assert_eq!(lucene_escape("AC/DC: \"Hi\""), "AC\\/DC\\: \\\"Hi\\\"");
    }
}

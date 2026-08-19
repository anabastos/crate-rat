use std::path::Path;

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::Accessor;

pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub duration_secs: u64,
    pub cover: Option<Vec<u8>>,
}

/// Reads audio tags (artist/album/title/etc.) and embedded cover art from a track file.
/// Returns `None` for files without readable audio metadata (wrong format, corrupt, etc.).
pub fn read_track_metadata(path: &Path) -> Option<TrackMetadata> {
    let tagged_file = Probe::open(path).ok()?.read().ok()?;
    let duration_secs = tagged_file.properties().duration().as_secs();
    let tag = tagged_file.primary_tag().or_else(|| tagged_file.first_tag())?;
    let cover = tag.pictures().first().map(|picture| picture.data().to_vec());

    Some(TrackMetadata {
        title: tag.title().map(|value| value.to_string()),
        artist: tag.artist().map(|value| value.to_string()),
        album: tag.album().map(|value| value.to_string()),
        year: tag.year(),
        genre: tag.genre().map(|value| value.to_string()),
        duration_secs,
        cover,
    })
}

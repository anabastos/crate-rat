//! Read-only access to a rekordbox 6/7 library (`master.db`), so its playlists can show up in
//! Crate Rat as a crate of their own without copying any audio.
//!
//! `master.db` is a SQLCipher 4 database. Rather than linking SQLCipher (and OpenSSL, which is
//! painful to build on Windows), we decrypt a private copy page by page with pure-Rust crypto
//! and open that plain copy with SQLite. rekordbox's own file is never written to.

use std::{
    collections::HashMap,
    env, fs, io,
    path::{Path, PathBuf},
};

use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use rusqlite::{Connection, OpenFlags};

use crate::{library::TrackInfo, sync::{RemoteTrackMetadata, TrackFile}};

/// Name of the crate rekordbox playlists are shown under.
pub const CRATE_NAME: &str = "REKORD";

/// The SQLCipher passphrase rekordbox uses, obfuscated exactly like pyrekordbox ships it
/// (base85 → XOR → zlib), which is where it comes from.
const KEY_BLOB: &[u8] = b"PN_Pq^*N>(JYe*u^8;Yg76HuZ<mR13S?=>)b9;DpoTXV(6ItkU`}8*m6tx_I{Solh_N#dfe{v=";
const KEY_BLOB_XOR: &[u8] = b"657f48f84c437cc1";

const PAGE_SIZE: usize = 4096;
const SALT_LEN: usize = 16;
/// Bytes SQLCipher 4 reserves at the end of each page: a 16-byte IV + a 64-byte HMAC-SHA512.
const RESERVE: usize = 80;
const KDF_ITERATIONS: u32 = 256_000;
const WAL_HEADER: usize = 32;
const WAL_FRAME_HEADER: usize = 24;

/// One rekordbox playlist, its name prefixed with the folders it sits in ("dnb/hardcore/x").
pub struct RekordboxPlaylist {
    pub name: String,
    pub tracks: Vec<RekordboxTrack>,
}

pub struct RekordboxTrack {
    pub file: TrackFile,
    pub info: TrackInfo,
}

/// Where rekordbox keeps its library on this machine, if it's there.
pub fn default_db_path() -> Option<PathBuf> {
    let candidates = [
        env::var_os("APPDATA").map(|app_data| PathBuf::from(app_data).join("Pioneer").join("rekordbox").join("master.db")),
        env::var_os("HOME").map(|home| PathBuf::from(home).join("Library").join("Pioneer").join("rekordbox").join("master.db")),
    ];
    candidates.into_iter().flatten().find(|path| path.is_file())
}

/// Reads every regular (non-smart) playlist from rekordbox's `master.db`, in rekordbox's own order.
pub fn load_playlists(db_path: &Path) -> io::Result<Vec<RekordboxPlaylist>> {
    let plain = decrypt_to_temp(db_path)?;
    let result = read_playlists(&plain).map_err(io::Error::other);
    let _ = fs::remove_file(&plain);
    result
}

fn read_playlists(plain_db: &Path) -> rusqlite::Result<Vec<RekordboxPlaylist>> {
    let connection = Connection::open_with_flags(plain_db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    struct Node {
        name: String,
        parent: String,
        seq: i64,
        is_playlist: bool,
    }
    let mut nodes: HashMap<String, Node> = HashMap::new();
    let mut statement = connection.prepare("SELECT ID, ParentID, Name, Seq, Attribute FROM djmdPlaylist WHERE rb_local_deleted = 0")?;
    let rows = statement.query_map([], |row| {
        let id: String = row.get(0)?;
        let parent: Option<String> = row.get(1)?;
        let name: Option<String> = row.get(2)?;
        let seq: Option<i64> = row.get(3)?;
        let attribute: Option<i64> = row.get(4)?;
        // 0 = playlist, 1 = folder; smart playlists (4) are queries rather than track lists, and
        // anything else is rekordbox-internal.
        Ok((id, Node { name: name.unwrap_or_default(), parent: parent.unwrap_or_default(), seq: seq.unwrap_or(0), is_playlist: attribute == Some(0) }, attribute))
    })?;
    for row in rows {
        let (id, node, attribute) = row?;
        if matches!(attribute, Some(0) | Some(1)) {
            nodes.insert(id, node);
        }
    }

    // Folder path of a node, plus its position at every level, so playlists sort the way
    // rekordbox's tree shows them.
    let lineage = |id: &str| -> Option<(Vec<String>, Vec<i64>)> {
        let mut names = Vec::new();
        let mut order = Vec::new();
        let mut current = nodes.get(id)?;
        loop {
            names.push(current.name.trim().to_string());
            order.push(current.seq);
            match nodes.get(&current.parent) {
                Some(parent) if names.len() < 32 => current = parent,
                _ => break,
            }
        }
        names.reverse();
        order.reverse();
        Some((names, order))
    };

    let mut tracks_by_playlist: HashMap<String, Vec<RekordboxTrack>> = HashMap::new();
    let mut statement = connection.prepare(
        "SELECT sp.PlaylistID, c.FolderPath, c.Title, a.Name, al.Name, c.Length, c.ReleaseYear, g.Name, c.FileSize
         FROM djmdSongPlaylist sp
         JOIN djmdContent c ON c.ID = sp.ContentID AND c.rb_local_deleted = 0
         LEFT JOIN djmdArtist a ON a.ID = c.ArtistID
         LEFT JOIN djmdAlbum al ON al.ID = c.AlbumID
         LEFT JOIN djmdGenre g ON g.ID = c.GenreID
         WHERE sp.rb_local_deleted = 0
         ORDER BY sp.PlaylistID, sp.TrackNo",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            row.get::<_, Option<i64>>(5)?.unwrap_or(0),
            row.get::<_, Option<i64>>(6)?.unwrap_or(0),
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<i64>>(8)?.unwrap_or(0),
        ))
    })?;
    for row in rows {
        let (playlist_id, folder_path, title, artist, album, length, year, genre, file_size) = row?;
        if !nodes.get(&playlist_id).is_some_and(|node| node.is_playlist) {
            continue;
        }
        tracks_by_playlist.entry(playlist_id).or_default().push(track_from_row(folder_path, title, artist, album, length, year, genre, file_size));
    }

    let mut playlists: Vec<(Vec<i64>, RekordboxPlaylist)> = nodes
        .iter()
        .filter(|(_, node)| node.is_playlist)
        .filter_map(|(id, _)| {
            let (names, order) = lineage(id)?;
            let tracks = tracks_by_playlist.remove(id).unwrap_or_default();
            Some((order, RekordboxPlaylist { name: names.join("/"), tracks }))
        })
        .collect();
    playlists.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    Ok(playlists.into_iter().map(|(_, playlist)| playlist).collect())
}

#[allow(clippy::too_many_arguments)]
fn track_from_row(folder_path: String, title: String, artist: String, album: String, length: i64, year: i64, genre: Option<String>, file_size: i64) -> RekordboxTrack {
    let path = PathBuf::from(&folder_path);
    // Streaming entries look like `tidal:tracks:12345`; those (and files on a drive that isn't
    // plugged in right now) are listed but can't be played.
    let tidal_id = folder_path.strip_prefix("tidal:tracks:").map(str::to_string);
    let file_metadata = if tidal_id.is_none() { fs::metadata(&path).ok().filter(fs::Metadata::is_file) } else { None };
    let title = if title.trim().is_empty() { path.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_else(|| "Unknown track".into()) } else { title };
    let duration_secs = u64::try_from(length).unwrap_or(0);
    let info = TrackInfo {
        title: title.clone(),
        artist: artist.clone(),
        album: album.clone(),
        year: u32::try_from(year).ok().filter(|year| *year > 0),
        duration_secs,
        genre: genre.filter(|genre| !genre.trim().is_empty()),
        external_id: tidal_id.clone().map(|id| ("tidal".to_string(), id)),
    };
    let file = match file_metadata {
        Some(metadata) => TrackFile {
            name: path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or(title),
            path,
            size_bytes: metadata.len(),
            modified: metadata.modified().ok(),
            remote_metadata: None,
        },
        None => TrackFile {
            name: title,
            path,
            size_bytes: u64::try_from(file_size).unwrap_or(0),
            modified: None,
            remote_metadata: Some(RemoteTrackMetadata { artist, album, duration_secs, external_id: tidal_id }),
        },
    };
    RekordboxTrack { file, info }
}

/// Decrypts `db_path` (and any pending `-wal` frames) into a plain SQLite file in the temp dir.
fn decrypt_to_temp(db_path: &Path) -> io::Result<PathBuf> {
    let encrypted = fs::read(db_path)?;
    let wal = fs::read(PathBuf::from(format!("{}-wal", db_path.display()))).ok();
    let plain = decrypt_database(&encrypted, wal.as_deref(), &passphrase()?)?;
    let out = env::temp_dir().join(format!("crate-rat-rekordbox-{}.db", std::process::id()));
    fs::write(&out, plain)?;
    Ok(out)
}

fn passphrase() -> io::Result<String> {
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, format!("rekordbox key: {what}"));
    let decoded = base85_decode(KEY_BLOB).ok_or_else(|| invalid("bad base85"))?;
    let xored: Vec<u8> = decoded.iter().enumerate().map(|(index, byte)| byte ^ KEY_BLOB_XOR[index % KEY_BLOB_XOR.len()]).collect();
    let inflated = miniz_oxide::inflate::decompress_to_vec_zlib(&xored).map_err(|_| invalid("bad zlib"))?;
    String::from_utf8(inflated).map_err(|_| invalid("not utf-8"))
}

/// Python's `base64.b85decode` (RFC 1924 alphabet).
fn base85_decode(input: &[u8]) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";
    let padding = (5 - input.len() % 5) % 5;
    let mut padded = input.to_vec();
    padded.extend(std::iter::repeat_n(b'~', padding));
    let mut out = Vec::with_capacity(padded.len() / 5 * 4);
    for chunk in padded.chunks(5) {
        let mut value: u64 = 0;
        for &character in chunk {
            value = value * 85 + ALPHABET.iter().position(|&a| a == character)? as u64;
        }
        out.extend_from_slice(&u32::try_from(value).ok()?.to_be_bytes());
    }
    out.truncate(out.len() - padding);
    Some(out)
}

/// Decrypts a SQLCipher 4 database (default settings: 4096-byte pages, PBKDF2-HMAC-SHA512 with
/// 256000 iterations, AES-256-CBC) into a plain SQLite image. Committed WAL frames are applied
/// on top so changes rekordbox hasn't checkpointed yet still show up.
fn decrypt_database(encrypted: &[u8], wal: Option<&[u8]>, passphrase: &str) -> io::Result<Vec<u8>> {
    if encrypted.len() < PAGE_SIZE || !encrypted.len().is_multiple_of(PAGE_SIZE) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "master.db doesn't look like a SQLCipher database"));
    }
    let salt = &encrypted[..SALT_LEN];
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha512>(passphrase.as_bytes(), salt, KDF_ITERATIONS, &mut key);

    let mut plain: Vec<u8> = Vec::with_capacity(encrypted.len());
    for (index, page) in encrypted.chunks(PAGE_SIZE).enumerate() {
        plain.extend_from_slice(&decrypt_page(page, index == 0, &key)?);
    }
    // A wrong key decrypts to noise: check the header's page size, reserved bytes and the first
    // page's b-tree type all came out right.
    let header_ok = plain[16..18] == [0x10, 0x00] && usize::from(plain[20]) == RESERVE && matches!(plain[100], 0x05 | 0x0d);
    if !header_ok {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "couldn't decrypt master.db (unsupported rekordbox version?)"));
    }
    // rekordbox runs in WAL mode; this copy is read with a plain rollback journal.
    plain[18] = 1;
    plain[19] = 1;

    if let Some(wal) = wal.filter(|wal| wal.len() > WAL_HEADER) {
        apply_wal(&mut plain, wal, &key)?;
    }
    Ok(plain)
}

fn decrypt_page(page: &[u8], first: bool, key: &[u8; 32]) -> io::Result<Vec<u8>> {
    let start = if first { SALT_LEN } else { 0 };
    let iv = &page[PAGE_SIZE - RESERVE..PAGE_SIZE - RESERVE + 16];
    let mut body = page[start..PAGE_SIZE - RESERVE].to_vec();
    cbc::Decryptor::<aes::Aes256>::new(key.into(), iv.into())
        .decrypt_padded_mut::<NoPadding>(&mut body)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad page in master.db"))?;
    let mut out = Vec::with_capacity(PAGE_SIZE);
    if first {
        out.extend_from_slice(b"SQLite format 3\0");
    }
    out.extend_from_slice(&body);
    // Keep the reserved tail (zeroed): the page header says 80 bytes are reserved per page.
    out.resize(PAGE_SIZE, 0);
    Ok(out)
}

/// Applies the WAL's frames up to its last commit, ignoring frames left over from an older
/// generation of the WAL (their salts don't match the header's).
fn apply_wal(plain: &mut Vec<u8>, wal: &[u8], key: &[u8; 32]) -> io::Result<()> {
    let page_size = u32::from_be_bytes(wal[8..12].try_into().unwrap_or_default()) as usize;
    if page_size != PAGE_SIZE {
        return Ok(());
    }
    let salts = &wal[16..24];
    let frame_len = WAL_FRAME_HEADER + PAGE_SIZE;
    let mut pending: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut committed: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut committed_size = 0usize;
    for frame in wal[WAL_HEADER..].chunks_exact(frame_len) {
        if &frame[8..16] != salts {
            break;
        }
        let page_number = u32::from_be_bytes(frame[0..4].try_into().unwrap_or_default()) as usize;
        let db_size_after_commit = u32::from_be_bytes(frame[4..8].try_into().unwrap_or_default()) as usize;
        if page_number == 0 {
            break;
        }
        pending.push((page_number, decrypt_page(&frame[WAL_FRAME_HEADER..], page_number == 1, key)?));
        if db_size_after_commit > 0 {
            committed.append(&mut pending);
            committed_size = db_size_after_commit;
        }
    }
    for (page_number, mut page) in committed {
        if page_number == 1 {
            page[18] = 1;
            page[19] = 1;
        }
        let offset = (page_number - 1) * PAGE_SIZE;
        if plain.len() < offset + PAGE_SIZE {
            plain.resize(offset + PAGE_SIZE, 0);
        }
        plain[offset..offset + PAGE_SIZE].copy_from_slice(&page);
    }
    if committed_size > 0 {
        plain.truncate(committed_size * PAGE_SIZE);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_blob_decodes() {
        let key = passphrase().expect("embedded key should decode");
        assert_eq!(key.len(), 64);
        assert!(key.starts_with("402fd"));
    }

    #[test]
    fn base85_matches_python() {
        // base64.b85encode(b"hello world") == b"Xk~0{Zy<MXa%^M"
        assert_eq!(base85_decode(b"Xk~0{Zy<MXa%^M").as_deref(), Some(&b"hello world"[..]));
    }

    /// Point `RATCRATE_TEST_REKORDBOX_DB` at a real master.db to check the whole read.
    #[test]
    fn reads_a_real_library_when_given_one() {
        let Some(path) = env::var_os("RATCRATE_TEST_REKORDBOX_DB") else { return };
        let playlists = load_playlists(Path::new(&path)).expect("master.db should load");
        assert!(!playlists.is_empty());
        for playlist in playlists.iter().take(5) {
            eprintln!("{} ({} tracks)", playlist.name, playlist.tracks.len());
        }
    }
}

use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::fd::AsRawFd;

const AUDIO_EXTENSIONS: &[&str] = &["aac", "aiff", "flac", "m4a", "mp3", "ogg", "opus", "wav"];
const MAX_TEMP_FILE_ALLOCATION_ATTEMPTS: usize = 128;

static NEXT_TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub tracks: Vec<Track>,
    pub selected_track_id: Option<String>,
    #[serde(default)]
    pub reference_tracks: Vec<ReferenceTrack>,
    #[serde(default)]
    pub planner_order: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceTrack {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_proof: Option<crate::source::AudioSourceProof>,
    #[serde(default)]
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub original_name: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_proof: Option<crate::source::AudioSourceProof>,
    #[serde(default)]
    pub reference_path: Option<PathBuf>,
    pub size: u64,
    pub favorite: bool,
    pub stage: TrackStage,
    #[serde(default)]
    pub status: TrackStatus,
    pub notes: Vec<Note>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackStatus {
    #[default]
    #[serde(rename = "inbox")]
    Inbox,
    #[serde(rename = "refine")]
    Refine,
    #[serde(rename = "release")]
    Release,
    #[serde(rename = "archive")]
    Archive,
    #[serde(rename = "maybe")]
    Maybe,
}

impl TrackStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Inbox => "Inbox",
            Self::Refine => "Refine",
            Self::Release => "Release",
            Self::Archive => "Archive",
            Self::Maybe => "Maybe",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackStage {
    #[serde(rename = "sound-design")]
    SoundDesign,
    #[serde(rename = "production")]
    Production,
    #[serde(rename = "mixdown")]
    Mixdown,
    #[serde(rename = "mastering")]
    Mastering,
}

impl TrackStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::SoundDesign => "Sound design",
            Self::Production => "Production / arrangement",
            Self::Mixdown => "Mixdown",
            Self::Mastering => "Mastering",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub time_millis: u64,
    pub body: String,
    pub done: bool,
}

pub struct InstanceLock {
    #[cfg(not(unix))]
    path: PathBuf,
    _file: fs::File,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // Closing the file also releases the lock, but explicitly unlocking
            // makes the lifetime contract clear and keeps the target file reusable.
            let _ = unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
        }

        #[cfg(not(unix))]
        let _ = fs::remove_file(&self.path);
    }
}

pub fn acquire_instance_lock() -> Result<InstanceLock, String> {
    let path = app_data_directory().join("instance.lock");
    let directory = path
        .parent()
        .ok_or_else(|| format!("No parent directory for {}", path.display()))?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;

    #[cfg(unix)]
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("Could not open {}: {error}", path.display()))?;

    #[cfg(unix)]
    {
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            return Err(format!("another Cadence process owns {}", path.display()));
        }
    }

    #[cfg(not(unix))]
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not create {}: {error}", path.display()))?;

    file.set_len(0)
        .map_err(|error| format!("Could not initialize {}: {error}", path.display()))?;
    writeln!(file, "{}", std::process::id())
        .map_err(|error| format!("Could not initialize {}: {error}", path.display()))?;
    file.flush()
        .map_err(|error| format!("Could not initialize {}: {error}", path.display()))?;

    #[cfg(unix)]
    {
        Ok(InstanceLock { _file: file })
    }

    #[cfg(not(unix))]
    {
        Ok(InstanceLock { path, _file: file })
    }
}

pub fn load_library() -> Result<Library, String> {
    load_library_at(&library_path())
}

pub fn load_library_at(path: &Path) -> Result<Library, String> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let mut library: Library = serde_json::from_str(&contents)
                .map_err(|error| format!("Could not parse {}: {error}", path.display()))?;
            normalize_reference_tracks(&mut library);
            normalize_planner_order(&mut library);
            Ok(library)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Library::default()),
        Err(error) => Err(format!("Could not read {}: {error}", path.display())),
    }
}

/// Preserve an unreadable library before replacing it with a fresh snapshot.
///
/// The original bytes are copied to a unique same-directory backup using
/// `create_new`, flushed, synced, and closed; its directory entry is then
/// synced so the backup contents and directory entry are durable before the
/// active library is replaced. Backups are intentionally never removed by
/// this helper.
pub fn preserve_unreadable_library_and_start_fresh() -> Result<PathBuf, String> {
    preserve_unreadable_library_and_start_fresh_at(&library_path())
}

pub fn preserve_unreadable_library_and_start_fresh_at(path: &Path) -> Result<PathBuf, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("Could not preserve {}: {error}", path.display()))?;
    let directory = path
        .parent()
        .ok_or_else(|| format!("No parent directory for {}", path.display()))?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;

    let (backup_path, mut backup_file) = create_unique_recovery_backup(path)?;
    backup_file
        .write_all(&bytes)
        .map_err(|error| format!("Could not write {}: {error}", backup_path.display()))?;
    backup_file
        .flush()
        .map_err(|error| format!("Could not flush {}: {error}", backup_path.display()))?;
    backup_file
        .sync_all()
        .map_err(|error| format!("Could not sync {}: {error}", backup_path.display()))?;
    drop(backup_file);

    #[cfg(unix)]
    if let Err(error) = sync_parent_directory(directory) {
        return Err(format!(
            "Could not sync recovery backup directory {} before replacing the active library; active library was not replaced: {error}",
            directory.display()
        ));
    }

    persist_library_at(&Library::default(), path)?;
    Ok(backup_path)
}

pub fn import_into_library(
    library: Library,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Library, String> {
    import_into_library_at(library, decoded, &library_path())
}

fn import_into_library_at(
    mut library: Library,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Library, String> {
    let path = decoded.path().to_path_buf();
    validate_audio_path(&path)?;
    ensure_decoded_audio_unchanged(&decoded)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    ensure_decoded_audio_unchanged(&decoded)?;

    let original_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled track")
        .to_string();
    let title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled track")
        .replace(['_', '-'], " ");
    let id = format!("track-{}", unique_id());
    library.tracks.push(Track {
        id: id.clone(),
        title: if title.trim().is_empty() {
            String::from("Untitled track")
        } else {
            title
        },
        original_name,
        path,
        source_proof: Some(decoded.source_proof().clone()),
        reference_path: None,
        size: metadata.len(),
        favorite: false,
        stage: TrackStage::SoundDesign,
        status: TrackStatus::Inbox,
        notes: Vec::new(),
    });
    normalize_planner_order(&mut library);
    library.selected_track_id = Some(id);
    ensure_decoded_audio_unchanged(&decoded)?;
    persist_library_at(&library, library_path)?;
    Ok(library)
}

/// Replace the source file for one existing track while preserving its stable
/// identity, favorite state, and workflow stage. A replacement is a new audio
/// version, so timestamped comments are intentionally cleared.
pub fn replace_track(
    library: Library,
    track_id: &str,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Library, String> {
    replace_track_at(library, track_id, decoded, &library_path())
}

fn replace_track_at(
    mut library: Library,
    track_id: &str,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Library, String> {
    let path = decoded.path().to_path_buf();
    validate_audio_path(&path)?;
    ensure_decoded_audio_unchanged(&decoded)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }

    ensure_decoded_audio_unchanged(&decoded)?;
    replace_track_metadata_with_proof(
        &mut library,
        track_id,
        path,
        metadata.len(),
        Some(decoded.source_proof().clone()),
    )?;
    ensure_decoded_audio_unchanged(&decoded)?;
    persist_library_at(&library, library_path)?;
    Ok(library)
}

/// Associate a second audio file with one track. The reference is kept as an
/// external path, just like the primary source, so importing it never copies or
/// mutates the user's audio file.
pub fn set_reference_track(
    library: Library,
    track_id: &str,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Library, String> {
    set_reference_track_at(library, track_id, decoded, &library_path())
}

fn set_reference_track_at(
    mut library: Library,
    track_id: &str,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Library, String> {
    let path = decoded.path().to_path_buf();
    validate_audio_path(&path)?;
    ensure_decoded_audio_unchanged(&decoded)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    ensure_decoded_audio_unchanged(&decoded)?;

    set_reference_track_metadata_with_proof(
        &mut library,
        track_id,
        path,
        Some(decoded.source_proof().clone()),
    )?;
    ensure_decoded_audio_unchanged(&decoded)?;
    persist_library_at(&library, library_path)?;
    Ok(library)
}

/// Add an audio file to the global reference catalog without assigning it to
/// any main track. The catalog stores the external path only; importing a
/// reference never copies or mutates the user's audio file.
pub fn add_reference_track(
    library: Library,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Library, String> {
    add_reference_track_at(library, decoded, &library_path())
}

fn add_reference_track_at(
    mut library: Library,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Library, String> {
    let path = decoded.path().to_path_buf();
    validate_audio_path(&path)?;
    ensure_decoded_audio_unchanged(&decoded)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    ensure_decoded_audio_unchanged(&decoded)?;

    ensure_reference_track_with_proof(&mut library, path, decoded.source_proof().clone())?;
    ensure_decoded_audio_unchanged(&decoded)?;
    persist_library_at(&library, library_path)?;
    Ok(library)
}

fn ensure_decoded_audio_unchanged(decoded: &crate::audio::DecodedAudioFile) -> Result<(), String> {
    decoded
        .validate_source()
        .map_err(|error| format!("Audio source changed after preflight: {error}"))
}

#[allow(dead_code)]
fn set_reference_track_metadata(
    library: &mut Library,
    track_id: &str,
    path: PathBuf,
) -> Result<(), String> {
    set_reference_track_metadata_with_proof(library, track_id, path, None)
}

fn set_reference_track_metadata_with_proof(
    library: &mut Library,
    track_id: &str,
    path: PathBuf,
    source_proof: Option<crate::source::AudioSourceProof>,
) -> Result<(), String> {
    if let Some(source_proof) = source_proof {
        ensure_reference_track_with_proof(library, path.clone(), source_proof)?;
    } else {
        ensure_reference_track(library, path.clone());
    }
    set_reference_track_selection(library, track_id, path).map(|_| ())
}

pub fn set_reference_track_selection(
    library: &mut Library,
    track_id: &str,
    path: PathBuf,
) -> Result<bool, String> {
    if !library.tracks.iter().any(|track| track.id == track_id) {
        return Err(String::from("That track is no longer in the library."));
    }
    ensure_reference_track(library, path.clone());
    let track = library
        .tracks
        .iter_mut()
        .find(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    let changed = track.reference_path.as_ref() != Some(&path);
    track.reference_path = Some(path);
    Ok(changed)
}

fn ensure_reference_track(library: &mut Library, path: PathBuf) {
    if !library
        .reference_tracks
        .iter()
        .any(|reference| reference.path == path)
    {
        library.reference_tracks.push(ReferenceTrack {
            path,
            source_proof: None,
            notes: Vec::new(),
        });
    }
}

fn ensure_reference_track_with_proof(
    library: &mut Library,
    path: PathBuf,
    source_proof: crate::source::AudioSourceProof,
) -> Result<(), String> {
    if let Some(reference) = library
        .reference_tracks
        .iter_mut()
        .find(|reference| reference.path == path)
    {
        match reference.source_proof.as_ref() {
            Some(existing) if existing != &source_proof => Err(format!(
                "Reference source changed for {}. Please remove it from the reference catalog and re-import it.",
                path.display()
            )),
            Some(_) => Ok(()),
            None => {
                reference.source_proof = Some(source_proof);
                Ok(())
            }
        }
    } else {
        library.reference_tracks.push(ReferenceTrack {
            path,
            source_proof: Some(source_proof),
            notes: Vec::new(),
        });
        Ok(())
    }
}

fn normalize_reference_tracks(library: &mut Library) {
    let paths = library
        .tracks
        .iter()
        .filter_map(|track| track.reference_path.clone())
        .collect::<Vec<_>>();
    for path in paths {
        ensure_reference_track(library, path);
    }
}

#[allow(dead_code)]
fn replace_track_metadata(
    library: &mut Library,
    track_id: &str,
    path: PathBuf,
    size: u64,
) -> Result<(), String> {
    replace_track_metadata_with_proof(library, track_id, path, size, None)
}

fn replace_track_metadata_with_proof(
    library: &mut Library,
    track_id: &str,
    path: PathBuf,
    size: u64,
    source_proof: Option<crate::source::AudioSourceProof>,
) -> Result<(), String> {
    let original_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled track")
        .to_string();
    let title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled track")
        .replace(['_', '-'], " ");
    let track = library
        .tracks
        .iter_mut()
        .find(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    track.title = if title.trim().is_empty() {
        String::from("Untitled track")
    } else {
        title
    };
    track.original_name = original_name;
    track.path = path;
    track.source_proof = source_proof;
    track.size = size;
    track.notes.clear();
    Ok(())
}

pub fn remove_track(library: &mut Library, track_id: &str) -> Result<(usize, Track), String> {
    let index = library
        .tracks
        .iter()
        .position(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    let removed = library.tracks.remove(index);
    library.planner_order.retain(|id| id != track_id);
    Ok((index, removed))
}

/// Remove a path from the global reference catalog and clear every main-track
/// assignment that points to it. The caller owns persistence so reducer-level
/// state can be invalidated before the next whole-library snapshot is saved.
pub fn remove_reference_track(library: &mut Library, path: &Path) -> Result<usize, String> {
    let original_count = library.reference_tracks.len();
    library
        .reference_tracks
        .retain(|reference| reference.path != path);
    if library.reference_tracks.len() == original_count {
        return Err(String::from(
            "That reference track is no longer in the catalog.",
        ));
    }

    let mut cleared_assignments = 0;
    for track in &mut library.tracks {
        if track.reference_path.as_deref() == Some(path) {
            track.reference_path = None;
            cleared_assignments += 1;
        }
    }
    Ok(cleared_assignments)
}

pub fn set_track_stage(
    library: &mut Library,
    track_id: &str,
    stage: TrackStage,
) -> Result<bool, String> {
    let track = library
        .tracks
        .iter_mut()
        .find(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    if track.stage == stage {
        return Ok(false);
    }
    track.stage = stage;
    Ok(true)
}

/// Normalize the persisted Planner order against the current track catalog.
///
/// Older libraries have no Planner order. They retain the Planner's historic
/// favorite-first projection on first load; newer libraries preserve their
/// explicit order while removing stale/duplicate IDs and appending imports.
pub fn normalize_planner_order(library: &mut Library) {
    let legacy_order = library.planner_order.is_empty();
    let known_ids = library
        .tracks
        .iter()
        .map(|track| track.id.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::<String>::new();
    let mut order = Vec::with_capacity(library.tracks.len());

    if legacy_order {
        for track in library.tracks.iter().filter(|track| track.favorite) {
            order.push(track.id.clone());
            seen.insert(track.id.clone());
        }
        for track in library.tracks.iter().filter(|track| !track.favorite) {
            order.push(track.id.clone());
            seen.insert(track.id.clone());
        }
    } else {
        for id in library.planner_order.drain(..) {
            if known_ids.contains(id.as_str()) && seen.insert(id.clone()) {
                order.push(id);
            }
        }
    }

    for track in &library.tracks {
        if seen.insert(track.id.clone()) {
            order.push(track.id.clone());
        }
    }
    library.planner_order = order;
}

/// Return the effective Planner order without mutating the library.
#[allow(dead_code)]
pub fn planner_order(library: &Library) -> Vec<String> {
    planner_tracks(library)
        .into_iter()
        .map(|track| track.id.clone())
        .collect()
}

/// Return the effective Planner order as borrowed tracks without mutating the
/// library.
///
/// The first track for a duplicate ID wins, matching the historical
/// `planner_order` projection. Empty persisted orders retain the legacy
/// favorite-first ordering, including its one-entry-per-track behavior for
/// duplicate IDs. Explicit orders discard stale and duplicate IDs, then append
/// missing track IDs in library order.
pub fn planner_tracks<'a>(library: &'a Library) -> Vec<&'a Track> {
    let first_track_by_id = library.tracks.iter().fold(
        HashMap::<&str, &'a Track>::with_capacity(library.tracks.len()),
        |mut tracks, track| {
            tracks.entry(track.id.as_str()).or_insert(track);
            tracks
        },
    );
    let mut ordered = Vec::with_capacity(library.tracks.len());

    if library.planner_order.is_empty() {
        for track in library.tracks.iter().filter(|track| track.favorite) {
            ordered.push(
                *first_track_by_id
                    .get(track.id.as_str())
                    .expect("every library track must be indexed by its ID"),
            );
        }
        for track in library.tracks.iter().filter(|track| !track.favorite) {
            ordered.push(
                *first_track_by_id
                    .get(track.id.as_str())
                    .expect("every library track must be indexed by its ID"),
            );
        }
    } else {
        let mut seen = HashSet::with_capacity(library.planner_order.len() + library.tracks.len());
        for id in &library.planner_order {
            if seen.insert(id.as_str())
                && let Some(track) = first_track_by_id.get(id.as_str())
            {
                ordered.push(*track);
            }
        }
        for track in &library.tracks {
            if seen.insert(track.id.as_str()) {
                ordered.push(
                    *first_track_by_id
                        .get(track.id.as_str())
                        .expect("every library track must be indexed by its ID"),
                );
            }
        }
    }

    ordered
}

/// Move one track to a visible Planner insertion slot.
///
/// The operation is staged on a clone so invalid source, target, or slot data
/// leaves the caller's library unchanged. Hidden tracks remain in their
/// relative order when a status filter is active.
pub fn move_track_to_planner_slot(
    library: &mut Library,
    source_id: &str,
    target_stage: TrackStage,
    target_slot: usize,
    status_filter: Option<TrackStatus>,
) -> Result<bool, String> {
    let mut working = library.clone();
    normalize_planner_order(&mut working);

    let source = working
        .tracks
        .iter()
        .find(|track| track.id == source_id)
        .cloned()
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    if status_filter.is_some_and(|status| source.status != status) {
        return Err(String::from(
            "That track is not visible in the current Planner filter.",
        ));
    }

    let order_before = working.planner_order.clone();
    let visible_target_ids = planner_visible_ids(&working, target_stage, status_filter);
    if target_slot > visible_target_ids.len() {
        return Err(String::from(
            "That Planner drop target is no longer available.",
        ));
    }
    let source_visible_index = if source.stage == target_stage {
        visible_target_ids.iter().position(|id| id == source_id)
    } else {
        None
    };

    let mut order_after_source = order_before;
    let source_order_index = order_after_source
        .iter()
        .position(|id| id == source_id)
        .ok_or_else(|| String::from("That track is missing from the Planner order."))?;
    order_after_source.remove(source_order_index);

    let visible_target_ids_after_source = visible_target_ids
        .iter()
        .filter(|id| *id != source_id)
        .cloned()
        .collect::<Vec<_>>();
    let adjusted_slot = source_visible_index
        .filter(|source_index| *source_index < target_slot)
        .map_or(target_slot, |_| target_slot - 1);
    if adjusted_slot > visible_target_ids_after_source.len() {
        return Err(String::from(
            "That Planner drop target is no longer available.",
        ));
    }

    let insertion_index =
        if let Some(anchor_id) = visible_target_ids_after_source.get(adjusted_slot) {
            order_after_source
                .iter()
                .position(|id| id == anchor_id)
                .ok_or_else(|| String::from("That Planner drop target is no longer available."))?
        } else if let Some(last_visible_id) = visible_target_ids_after_source.last() {
            order_after_source
                .iter()
                .position(|id| id == last_visible_id)
                .map_or(order_after_source.len(), |index| index + 1)
        } else {
            order_after_source
                .iter()
                .position(|id| {
                    working
                        .tracks
                        .iter()
                        .find(|track| track.id == *id)
                        .is_some_and(|track| track.stage == target_stage)
                })
                .unwrap_or(order_after_source.len())
        };
    order_after_source.insert(insertion_index, source_id.to_string());

    if let Some(track) = working
        .tracks
        .iter_mut()
        .find(|track| track.id == source_id)
    {
        track.stage = target_stage;
    }
    working.planner_order = order_after_source;
    let changed = working != *library;
    if changed {
        *library = working;
    }
    Ok(changed)
}

fn planner_visible_ids(
    library: &Library,
    stage: TrackStage,
    status_filter: Option<TrackStatus>,
) -> Vec<String> {
    library
        .planner_order
        .iter()
        .filter_map(|id| library.tracks.iter().find(|track| track.id == *id))
        .filter(|track| track.stage == stage)
        .filter(|track| status_filter.is_none_or(|status| track.status == status))
        .map(|track| track.id.clone())
        .collect()
}

pub fn set_track_status(
    library: &mut Library,
    track_id: &str,
    status: TrackStatus,
) -> Result<bool, String> {
    let track = library
        .tracks
        .iter_mut()
        .find(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    if track.status == status {
        return Ok(false);
    }
    track.status = status;
    Ok(true)
}

pub fn selection_after_removal(library: &Library, removed_index: usize) -> Option<String> {
    library
        .tracks
        .get(removed_index)
        .or_else(|| {
            removed_index
                .checked_sub(1)
                .and_then(|index| library.tracks.get(index))
        })
        .map(|track| track.id.clone())
}

pub fn persist_library(library: &Library) -> Result<(), String> {
    let path = library_path();
    persist_library_at(library, &path)
}

fn persist_library_at(library: &Library, path: &Path) -> Result<(), String> {
    let encoded = serde_json::to_vec_pretty(library)
        .map_err(|error| format!("Could not encode library: {error}"))?;
    let directory = path
        .parent()
        .ok_or_else(|| format!("No parent directory for {}", path.display()))?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;

    let (temporary_path, mut file) = create_unique_temp_file(path)?;
    if let Err(error) = file.write_all(&encoded) {
        drop(file);
        return Err(with_temp_cleanup(
            format!("Could not write {}: {error}", temporary_path.display()),
            &temporary_path,
        ));
    }
    if let Err(error) = file.sync_all() {
        drop(file);
        return Err(with_temp_cleanup(
            format!("Could not sync {}: {error}", temporary_path.display()),
            &temporary_path,
        ));
    }
    drop(file);

    if let Err(error) = fs::rename(&temporary_path, path) {
        return Err(with_temp_cleanup(
            format!("Could not replace {}: {error}", path.display()),
            &temporary_path,
        ));
    }

    #[cfg(unix)]
    if let Err(error) = sync_parent_directory(directory) {
        return Err(format!(
            "Could not sync containing directory {} after replacing {}; durability is uncertain: {error}",
            directory.display(),
            path.display()
        ));
    }

    Ok(())
}

fn create_unique_temp_file(path: &Path) -> Result<(PathBuf, fs::File), String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library.json");
    let process_id = std::process::id();

    for _ in 0..MAX_TEMP_FILE_ALLOCATION_ATTEMPTS {
        let nonce = NEXT_TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let temporary_path = path.with_file_name(format!(".{file_name}.tmp-{process_id}-{nonce}"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Could not create {}: {error}",
                    temporary_path.display()
                ));
            }
        }
    }

    Err(format!(
        "Could not create a unique temporary file for {} after {MAX_TEMP_FILE_ALLOCATION_ATTEMPTS} attempts",
        path.display()
    ))
}

fn create_unique_recovery_backup(path: &Path) -> Result<(PathBuf, fs::File), String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library.json");
    let process_id = std::process::id();

    for _ in 0..MAX_TEMP_FILE_ALLOCATION_ATTEMPTS {
        let nonce = NEXT_TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let backup_path =
            path.with_file_name(format!("{file_name}.recovery-backup-{process_id}-{nonce}"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup_path)
        {
            Ok(file) => return Ok((backup_path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Could not create {}: {error}",
                    backup_path.display()
                ));
            }
        }
    }

    Err(format!(
        "Could not create a unique recovery backup for {} after {MAX_TEMP_FILE_ALLOCATION_ATTEMPTS} attempts",
        path.display()
    ))
}

fn with_temp_cleanup(primary_error: String, temporary_path: &Path) -> String {
    match fs::remove_file(temporary_path) {
        Ok(()) => primary_error,
        Err(cleanup_error) => format!(
            "{primary_error}; additionally, could not remove temporary file {}: {cleanup_error}",
            temporary_path.display()
        ),
    }
}

#[cfg(unix)]
fn sync_parent_directory(directory: &Path) -> std::io::Result<()> {
    let file = fs::File::open(directory)?;
    file.sync_all()
}

pub fn library_path() -> PathBuf {
    app_data_directory().join("library.json")
}

pub fn waveform_cache_path(source: &Path) -> PathBuf {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in source.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    app_data_directory()
        .join("waveform-cache")
        .join(format!("{hash:016x}.json"))
}

fn app_data_directory() -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join("Library/Application Support/Cadence");
    }

    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(data_home).join("cadence");
    }

    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/cadence")
}

fn validate_audio_path(path: &Path) -> Result<(), String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if extension
        .as_deref()
        .is_some_and(|extension| AUDIO_EXTENSIONS.contains(&extension))
    {
        Ok(())
    } else {
        Err(format!(
            "Unsupported audio file: {}. Choose WAV, AIFF, FLAC, M4A, MP3, OGG, OPUS, or AAC.",
            path.display()
        ))
    }
}

fn unique_id() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIRECTORY_NONCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            for _ in 0..MAX_TEMP_FILE_ALLOCATION_ATTEMPTS {
                let nonce = NEXT_TEST_DIRECTORY_NONCE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "cadence-storage-test-{}-{nonce}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self { path },
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                    Err(error) => {
                        panic!(
                            "could not create isolated test directory {}: {error}",
                            path.display()
                        )
                    }
                }
            }
            panic!("could not allocate an isolated test directory")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn temporary_paths(directory: &Path) -> Vec<PathBuf> {
        fs::read_dir(directory)
            .expect("test directory should be readable")
            .map(|entry| {
                entry
                    .expect("test directory entry should be readable")
                    .path()
            })
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".library.json.tmp-"))
            })
            .collect()
    }

    fn persistence_fixture() -> Library {
        let reference_path = PathBuf::from("/external/reference.wav");
        Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
                source_proof: None,
                reference_path: Some(reference_path.clone()),
                size: 42,
                favorite: true,
                stage: TrackStage::Mixdown,
                status: TrackStatus::Release,
                notes: vec![Note {
                    id: String::from("note-1"),
                    time_millis: 900,
                    body: String::from("Compare the low-end tail."),
                    done: false,
                }],
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: vec![ReferenceTrack {
                path: reference_path,
                source_proof: None,
                notes: vec![Note {
                    id: String::from("reference-note-1"),
                    time_millis: 1_100,
                    body: String::from("Check the reference vocal."),
                    done: true,
                }],
            }],
            planner_order: vec![String::from("track-1")],
        }
    }

    fn tiny_pcm_wav() -> Vec<u8> {
        let mut bytes = Vec::from(*b"RIFF");
        bytes.extend_from_slice(&38_u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&8_000_u32.to_le_bytes());
        bytes.extend_from_slice(&16_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&0_i16.to_le_bytes());
        bytes
    }

    fn decoded_audio_fixture(directory: &Path) -> (PathBuf, crate::audio::DecodedAudioFile) {
        let source = directory.join("source.wav");
        fs::write(&source, tiny_pcm_wav()).expect("valid audio fixture should be writable");
        let decoded = crate::audio::decode_audio_file(&source)
            .expect("valid audio fixture should pass preflight");
        (source, decoded)
    }

    #[test]
    fn persist_library_replaces_and_round_trips_a_complete_snapshot() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let library = persistence_fixture();
        let expected = serde_json::to_vec_pretty(&library).expect("fixture should encode");
        fs::write(&path, br#"{"stale":true}"#).expect("stale destination should be writable");

        persist_library_at(&library, &path).expect("library should persist");

        assert_eq!(
            fs::read(&path).expect("destination should be readable"),
            expected
        );
        let read_back: Library =
            serde_json::from_slice(&fs::read(&path).expect("destination should be readable"))
                .expect("persisted snapshot should parse");
        assert_eq!(read_back, library);
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn malformed_library_load_leaves_active_bytes_unchanged() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let malformed = b"{\"tracks\": [not valid json".to_vec();
        fs::write(&path, &malformed).expect("malformed library should be writable");

        let error = load_library_at(&path).expect_err("malformed library should fail to load");

        assert!(error.contains("Could not parse"));
        assert_eq!(
            fs::read(&path).expect("active library should remain readable"),
            malformed
        );
    }

    #[test]
    fn recovery_backup_is_exact_and_active_library_becomes_default() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let malformed = b"not-json\0with-original-bytes".to_vec();
        fs::write(&path, &malformed).expect("malformed library should be writable");

        let backup_path = preserve_unreadable_library_and_start_fresh_at(&path)
            .expect("recovery should preserve and reset the library");

        assert_ne!(backup_path, path);
        assert_eq!(backup_path.parent(), path.parent());
        assert_eq!(
            fs::read(&backup_path).expect("backup should be readable"),
            malformed
        );
        assert_eq!(
            serde_json::from_slice::<Library>(
                &fs::read(&path).expect("fresh library should exist")
            )
            .expect("fresh library should parse"),
            Library::default()
        );
        assert!(backup_path.exists(), "recovery backup must not be deleted");
    }

    #[test]
    fn temp_file_allocation_uses_distinct_create_new_paths() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let (first_path, first_file) =
            create_unique_temp_file(&path).expect("first temporary file should be created");
        let (second_path, second_file) =
            create_unique_temp_file(&path).expect("second temporary file should be created");
        let expected_prefix = format!(".library.json.tmp-{}-", std::process::id());

        assert_ne!(first_path, second_path);
        assert!(
            first_path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(&expected_prefix))
        );
        assert!(
            second_path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(&expected_prefix))
        );

        drop(first_file);
        drop(second_file);
        fs::remove_file(first_path).expect("first temporary file should be removable");
        fs::remove_file(second_path).expect("second temporary file should be removable");
    }

    #[test]
    fn persist_library_cleans_up_when_destination_cannot_be_replaced() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let library = persistence_fixture();
        fs::create_dir(&path).expect("destination directory should be creatable");

        let error =
            persist_library_at(&library, &path).expect_err("directory replacement must fail");

        assert!(error.contains("Could not replace"));
        assert!(
            path.is_dir(),
            "the failed replace must not delete the destination"
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn stage_labels_are_product_neutral_storage_values() {
        assert_eq!(TrackStage::SoundDesign.label(), "Sound design");
        assert_eq!(TrackStage::Production.label(), "Production / arrangement");
        assert_eq!(TrackStage::Mixdown.label(), "Mixdown");
        assert_eq!(TrackStage::Mastering.label(), "Mastering");
    }

    #[test]
    fn track_status_defaults_to_inbox_and_uses_workflow_labels() {
        assert_eq!(TrackStatus::default(), TrackStatus::Inbox);
        assert_eq!(TrackStatus::Inbox.label(), "Inbox");
        assert_eq!(TrackStatus::Refine.label(), "Refine");
        assert_eq!(TrackStatus::Release.label(), "Release");
        assert_eq!(TrackStatus::Archive.label(), "Archive");
        assert_eq!(TrackStatus::Maybe.label(), "Maybe");
    }

    #[test]
    fn track_status_serializes_to_canonical_values() {
        let statuses = [
            (TrackStatus::Inbox, "inbox"),
            (TrackStatus::Refine, "refine"),
            (TrackStatus::Release, "release"),
            (TrackStatus::Archive, "archive"),
            (TrackStatus::Maybe, "maybe"),
        ];

        for (status, expected) in statuses {
            assert_eq!(
                serde_json::to_string(&status).expect("status should encode"),
                format!("\"{expected}\"")
            );
        }
    }

    #[test]
    fn waveform_cache_paths_are_stable_and_source_specific() {
        let first = waveform_cache_path(Path::new("/external/first.wav"));
        assert_eq!(first, waveform_cache_path(Path::new("/external/first.wav")));
        assert_ne!(
            first,
            waveform_cache_path(Path::new("/external/second.wav"))
        );
        assert!(first.to_string_lossy().contains("waveform-cache"));
    }

    #[test]
    fn removing_a_track_only_changes_library_metadata() {
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
                source_proof: None,
                reference_path: None,
                size: 42,
                favorite: false,
                stage: TrackStage::SoundDesign,
                status: TrackStatus::Inbox,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: vec![ReferenceTrack {
                path: PathBuf::from("/tmp/reference.wav"),
                source_proof: None,
                notes: vec![Note {
                    id: String::from("reference-note-1"),
                    time_millis: 900,
                    body: String::from("Compare the low-end tail."),
                    done: false,
                }],
            }],
            planner_order: Vec::new(),
        };

        let (index, removed) = remove_track(&mut library, "track-1").expect("track should exist");

        assert_eq!(index, 0);
        assert_eq!(removed.path, PathBuf::from("/external/night-drive.wav"));
        assert!(library.tracks.is_empty());
        assert_eq!(library.selected_track_id.as_deref(), Some("track-1"));
    }

    #[test]
    fn selection_after_removal_prefers_the_same_position_then_previous() {
        let track = |id: &str| Track {
            id: id.to_string(),
            title: id.to_string(),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            source_proof: None,
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };
        let library = Library {
            tracks: vec![track("track-2"), track("track-3")],
            selected_track_id: None,
            reference_tracks: Vec::new(),
            planner_order: Vec::new(),
        };

        assert_eq!(
            selection_after_removal(&library, 0).as_deref(),
            Some("track-2")
        );
        assert_eq!(
            selection_after_removal(&library, 2).as_deref(),
            Some("track-3")
        );
        assert_eq!(selection_after_removal(&Library::default(), 0), None);
    }

    #[test]
    fn setting_a_track_stage_reports_real_changes_only() {
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
                source_proof: None,
                reference_path: None,
                size: 42,
                favorite: false,
                stage: TrackStage::SoundDesign,
                status: TrackStatus::Inbox,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new(),
            planner_order: Vec::new(),
        };

        assert!(
            !set_track_stage(&mut library, "track-1", TrackStage::SoundDesign)
                .expect("track should exist")
        );
        assert!(
            set_track_stage(&mut library, "track-1", TrackStage::Mixdown)
                .expect("track should exist")
        );
        assert_eq!(library.tracks[0].stage, TrackStage::Mixdown);
    }

    fn planner_test_track(
        id: &str,
        stage: TrackStage,
        status: TrackStatus,
        favorite: bool,
    ) -> Track {
        Track {
            id: String::from(id),
            title: String::from(id),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            source_proof: None,
            reference_path: None,
            size: 0,
            favorite,
            stage,
            status,
            notes: Vec::new(),
        }
    }

    #[test]
    fn planner_order_normalizes_legacy_and_stale_ids() {
        let mut legacy = Library {
            tracks: vec![
                planner_test_track("plain", TrackStage::Production, TrackStatus::Inbox, false),
                planner_test_track("starred", TrackStage::Production, TrackStatus::Inbox, true),
            ],
            selected_track_id: None,
            reference_tracks: Vec::new(),
            planner_order: Vec::new(),
        };
        normalize_planner_order(&mut legacy);
        assert_eq!(legacy.planner_order, ["starred", "plain"]);

        legacy.planner_order = vec![
            String::from("missing"),
            String::from("plain"),
            String::from("plain"),
        ];
        normalize_planner_order(&mut legacy);
        assert_eq!(legacy.planner_order, ["plain", "starred"]);
    }

    #[test]
    fn planner_tracks_preserve_order_semantics_and_first_match_identity() {
        let mut first_duplicate = planner_test_track(
            "duplicate",
            TrackStage::Production,
            TrackStatus::Inbox,
            false,
        );
        first_duplicate.title = String::from("first duplicate");
        let mut later_duplicate =
            planner_test_track("duplicate", TrackStage::Mixdown, TrackStatus::Refine, true);
        later_duplicate.title = String::from("later duplicate");
        let explicit = Library {
            tracks: vec![
                planner_test_track("tail", TrackStage::Mastering, TrackStatus::Release, false),
                first_duplicate.clone(),
                later_duplicate.clone(),
                planner_test_track(
                    "appended",
                    TrackStage::SoundDesign,
                    TrackStatus::Inbox,
                    false,
                ),
            ],
            selected_track_id: None,
            reference_tracks: Vec::new(),
            planner_order: vec![
                String::from("missing"),
                String::from("duplicate"),
                String::from("duplicate"),
                String::from("tail"),
            ],
        };
        let projected = planner_tracks(&explicit);

        assert_eq!(
            projected
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["duplicate", "tail", "appended"]
        );
        assert_eq!(planner_order(&explicit), ["duplicate", "tail", "appended"]);
        assert_eq!(projected[0].title, "first duplicate");
        assert!(std::ptr::eq(projected[0], &explicit.tracks[1]));
        assert!(std::ptr::eq(projected[1], &explicit.tracks[0]));

        let legacy = Library {
            tracks: vec![
                planner_test_track("plain", TrackStage::Production, TrackStatus::Inbox, false),
                planner_test_track("favorite", TrackStage::Production, TrackStatus::Inbox, true),
                first_duplicate,
                later_duplicate,
            ],
            selected_track_id: None,
            reference_tracks: Vec::new(),
            planner_order: Vec::new(),
        };
        let legacy_projected = planner_tracks(&legacy);
        assert_eq!(
            legacy_projected
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["favorite", "duplicate", "plain", "duplicate"]
        );
        assert_eq!(
            planner_order(&legacy),
            ["favorite", "duplicate", "plain", "duplicate"]
        );
        assert!(std::ptr::eq(legacy_projected[1], &legacy.tracks[2]));
        assert!(std::ptr::eq(legacy_projected[3], &legacy.tracks[2]));
    }

    #[test]
    fn planner_move_reorders_same_stage_and_changes_stage_atomically() {
        let mut library = Library {
            tracks: vec![
                planner_test_track("a", TrackStage::SoundDesign, TrackStatus::Inbox, false),
                planner_test_track("b", TrackStage::Production, TrackStatus::Inbox, false),
                planner_test_track("c", TrackStage::Production, TrackStatus::Inbox, false),
            ],
            selected_track_id: None,
            reference_tracks: Vec::new(),
            planner_order: vec![String::from("a"), String::from("b"), String::from("c")],
        };

        assert!(
            move_track_to_planner_slot(&mut library, "c", TrackStage::Production, 0, None,)
                .expect("same-stage move should validate")
        );
        assert_eq!(library.planner_order, ["a", "c", "b"]);
        assert_eq!(library.tracks[1].stage, TrackStage::Production);

        assert!(
            move_track_to_planner_slot(&mut library, "a", TrackStage::Production, 2, None,)
                .expect("cross-stage move should validate")
        );
        assert_eq!(library.planner_order, ["c", "b", "a"]);
        assert_eq!(library.tracks[0].stage, TrackStage::Production);
    }

    #[test]
    fn planner_move_adjusts_target_after_source_and_preserves_hidden_order() {
        let mut library = Library {
            tracks: vec![
                planner_test_track("a", TrackStage::Production, TrackStatus::Inbox, false),
                planner_test_track(
                    "hidden-one",
                    TrackStage::Production,
                    TrackStatus::Archive,
                    false,
                ),
                planner_test_track("b", TrackStage::Production, TrackStatus::Inbox, false),
                planner_test_track(
                    "hidden-two",
                    TrackStage::Production,
                    TrackStatus::Maybe,
                    false,
                ),
            ],
            selected_track_id: None,
            reference_tracks: Vec::new(),
            planner_order: vec![
                String::from("a"),
                String::from("hidden-one"),
                String::from("b"),
                String::from("hidden-two"),
            ],
        };

        assert!(
            move_track_to_planner_slot(
                &mut library,
                "a",
                TrackStage::Production,
                2,
                Some(TrackStatus::Inbox),
            )
            .expect("filtered end target should validate")
        );
        assert_eq!(
            library.planner_order,
            ["hidden-one", "b", "a", "hidden-two"]
        );
        assert_eq!(
            library
                .planner_order
                .iter()
                .filter(|id| id.starts_with("hidden"))
                .collect::<Vec<_>>(),
            [&String::from("hidden-one"), &String::from("hidden-two")]
        );
    }

    #[test]
    fn planner_move_rejects_stale_slot_without_mutating_library() {
        let mut library = Library {
            tracks: vec![planner_test_track(
                "a",
                TrackStage::Production,
                TrackStatus::Inbox,
                false,
            )],
            selected_track_id: None,
            reference_tracks: Vec::new(),
            planner_order: vec![String::from("a")],
        };
        let before = library.clone();

        let error = move_track_to_planner_slot(&mut library, "a", TrackStage::Production, 2, None)
            .expect_err("a slot beyond the visible list should be rejected");
        assert!(error.contains("no longer available"));
        assert_eq!(library, before);
    }

    #[test]
    fn planner_move_accepts_an_empty_stage_target() {
        let mut library = Library {
            tracks: vec![planner_test_track(
                "a",
                TrackStage::Production,
                TrackStatus::Inbox,
                false,
            )],
            selected_track_id: None,
            reference_tracks: Vec::new(),
            planner_order: vec![String::from("a")],
        };

        assert!(
            move_track_to_planner_slot(&mut library, "a", TrackStage::Mastering, 0, None)
                .expect("an empty stage target should validate")
        );
        assert_eq!(library.planner_order, ["a"]);
        assert_eq!(library.tracks[0].stage, TrackStage::Mastering);
    }

    #[test]
    fn replacing_track_metadata_updates_source_and_clears_comments() {
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
                source_proof: None,
                reference_path: None,
                size: 42,
                favorite: true,
                stage: TrackStage::Mixdown,
                status: TrackStatus::Maybe,
                notes: vec![Note {
                    id: String::from("note-1"),
                    time_millis: 1_250,
                    body: String::from("Recheck the vocal entrance."),
                    done: false,
                }],
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new(),
            planner_order: Vec::new(),
        };

        replace_track_metadata(
            &mut library,
            "track-1",
            PathBuf::from("/external/night-drive-v2.wav"),
            84,
        )
        .expect("the track should exist");

        let track = &library.tracks[0];
        assert_eq!(track.title, "night drive v2");
        assert_eq!(track.original_name, "night-drive-v2.wav");
        assert_eq!(track.path, PathBuf::from("/external/night-drive-v2.wav"));
        assert_eq!(track.size, 84);
        assert!(track.notes.is_empty());
        assert!(track.favorite);
        assert_eq!(track.stage, TrackStage::Mixdown);
        assert_eq!(track.status, TrackStatus::Maybe);
    }

    #[test]
    fn corrupt_replacement_leaves_library_and_persisted_snapshot_unchanged() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let replacement_path = directory.path.join("corrupt-replacement.wav");
        let library = persistence_fixture();
        persist_library_at(&library, &library_path).expect("original library should persist");
        let original_bytes =
            fs::read(&library_path).expect("original persisted library should be readable");
        fs::write(&replacement_path, b"this is not a wave file")
            .expect("corrupt replacement should be writable");

        let before = library.clone();
        let error = crate::audio::decode_audio_file(&replacement_path)
            .expect_err("corrupt replacement should fail preflight");

        assert!(error.contains("Could not identify") || error.contains("Could not read"));
        assert_eq!(library, before);
        assert_eq!(
            fs::read(&library_path).expect("persisted library should remain readable"),
            original_bytes
        );
        assert_eq!(
            load_library_at(&library_path).expect("original library should still reload"),
            before
        );
    }

    #[test]
    fn stale_replacement_proof_leaves_library_notes_and_persisted_snapshot_unchanged() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (source, decoded) = decoded_audio_fixture(&directory.path);
        let library = persistence_fixture();
        persist_library_at(&library, &library_path).expect("original library should persist");
        let original_bytes = fs::read(&library_path).expect("persisted library should be readable");
        fs::write(&source, b"replacement changed after preflight")
            .expect("stale replacement should be writable");

        let before = library.clone();
        let error = replace_track_at(library.clone(), "track-1", decoded, &library_path)
            .expect_err("stale replacement proof must be rejected");

        assert!(error.contains("changed after preflight"));
        assert_eq!(library, before);
        assert_eq!(library.tracks[0].notes, before.tracks[0].notes);
        assert_eq!(
            fs::read(&library_path).expect("library should remain readable"),
            original_bytes
        );
        assert_eq!(
            load_library_at(&library_path).expect("library should still reload"),
            before
        );
    }

    #[test]
    fn replacement_with_proof_preserves_workflow_semantics_and_persists() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (source, decoded) = decoded_audio_fixture(&directory.path);
        let library = persistence_fixture();
        persist_library_at(&library, &library_path).expect("original library should persist");

        let replaced = replace_track_at(library.clone(), "track-1", decoded, &library_path)
            .expect("replacement with a current proof should succeed");
        let track = &replaced.tracks[0];
        assert_eq!(track.id, "track-1");
        assert_eq!(track.title, "source");
        assert_eq!(track.original_name, "source.wav");
        assert_eq!(track.path, source);
        assert_eq!(track.size, tiny_pcm_wav().len() as u64);
        assert!(track.notes.is_empty());
        assert!(track.favorite);
        assert_eq!(track.stage, TrackStage::Mixdown);
        assert_eq!(track.status, TrackStatus::Release);
        assert_eq!(replaced.selected_track_id, Some(String::from("track-1")));
        assert_eq!(
            load_library_at(&library_path).expect("replaced library should reload"),
            replaced
        );
    }

    #[test]
    fn new_main_import_persists_encoded_source_proof_and_legacy_json_stays_optional() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (source, decoded) = decoded_audio_fixture(&directory.path);

        let imported = import_into_library_at(Library::default(), decoded, &library_path)
            .expect("main import should persist");
        let proof = imported.tracks[0]
            .source_proof
            .clone()
            .expect("new main imports should carry a proof");
        assert_eq!(proof.byte_len, tiny_pcm_wav().len() as u64);
        assert_eq!(imported.tracks[0].path, source);
        assert_eq!(
            load_library_at(&library_path).expect("persisted import should reload"),
            imported
        );
        let json = fs::read_to_string(&library_path).expect("persisted JSON should be readable");
        assert!(json.contains("\"source_proof\""));

        let legacy: Library = serde_json::from_str(
            r#"{"tracks":[{"id":"legacy","title":"Legacy","original_name":"legacy.wav","path":"/tmp/legacy.wav","reference_path":null,"size":0,"favorite":false,"stage":"sound-design","status":"inbox","notes":[]}],"selected_track_id":null}"#,
        )
        .expect("legacy JSON without a proof should load");
        assert_eq!(legacy.tracks[0].source_proof, None);
        assert!(
            !serde_json::to_string(&legacy)
                .expect("legacy library should encode")
                .contains("source_proof")
        );
    }

    #[test]
    fn reference_proof_is_idempotent_and_legacy_records_adopt_without_losing_notes() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (source, decoded) = decoded_audio_fixture(&directory.path);
        let notes = vec![Note {
            id: String::from("reference-note"),
            time_millis: 125,
            body: String::from("keep this note"),
            done: false,
        }];
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("owner"),
                title: String::from("Owner"),
                original_name: String::from("owner.wav"),
                path: PathBuf::from("/tmp/owner.wav"),
                source_proof: None,
                reference_path: Some(source.clone()),
                size: 0,
                favorite: false,
                stage: TrackStage::SoundDesign,
                status: TrackStatus::Inbox,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("owner")),
            reference_tracks: vec![ReferenceTrack {
                path: source.clone(),
                source_proof: None,
                notes: notes.clone(),
            }],
            planner_order: vec![String::from("owner")],
        };

        let adopted =
            set_reference_track_at(library.clone(), "owner", decoded.clone(), &library_path)
                .expect("legacy reference proof should be adopted");
        assert_eq!(adopted.tracks[0].reference_path, Some(source.clone()));
        assert_eq!(adopted.reference_tracks[0].notes, notes);
        assert_eq!(
            adopted.reference_tracks[0].source_proof,
            Some(decoded.source_proof().clone())
        );

        library = adopted.clone();
        let persisted = add_reference_track_at(library.clone(), decoded, &library_path)
            .expect("same path and proof should be idempotent");
        assert_eq!(persisted, library);
        assert_eq!(persisted.reference_tracks[0].notes, notes);
    }

    #[test]
    fn conflicting_reference_proof_rejects_atomically_and_preserves_notes_assignments() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let source = directory.path.join("reference.wav");
        fs::write(&source, tiny_pcm_wav()).expect("reference fixture should write");
        let first = crate::audio::decode_audio_file(&source).expect("first proof should decode");
        let library = Library {
            tracks: vec![Track {
                id: String::from("owner"),
                title: String::from("Owner"),
                original_name: String::from("owner.wav"),
                path: PathBuf::from("/tmp/owner.wav"),
                source_proof: None,
                reference_path: Some(source.clone()),
                size: 0,
                favorite: false,
                stage: TrackStage::SoundDesign,
                status: TrackStatus::Inbox,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("owner")),
            reference_tracks: vec![ReferenceTrack {
                path: source.clone(),
                source_proof: Some(first.source_proof().clone()),
                notes: vec![Note {
                    id: String::from("keep"),
                    time_millis: 1,
                    body: String::from("keep"),
                    done: true,
                }],
            }],
            planner_order: vec![String::from("owner")],
        };
        persist_library_at(&library, &library_path).expect("original library should persist");
        let original_bytes = fs::read(&library_path).expect("original snapshot should read");
        let mut replacement = tiny_pcm_wav();
        let last = replacement.len() - 2;
        replacement[last..].copy_from_slice(&1_i16.to_le_bytes());
        fs::write(&source, replacement).expect("changed reference should write");
        let second = crate::audio::decode_audio_file(&source).expect("second proof should decode");

        let error = add_reference_track_at(library.clone(), second, &library_path)
            .expect_err("different proof for one path must reject");
        assert!(error.contains("remove") && error.contains("re-import"));
        assert_eq!(
            library.reference_tracks[0].source_proof,
            Some(first.source_proof().clone())
        );
        assert_eq!(library.tracks[0].reference_path, Some(source));
        assert_eq!(
            fs::read(&library_path).expect("snapshot should stay unchanged"),
            original_bytes
        );
    }

    #[test]
    fn stale_main_import_proof_leaves_library_and_persisted_snapshot_unchanged() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (source, decoded) = decoded_audio_fixture(&directory.path);
        let library = persistence_fixture();
        persist_library_at(&library, &library_path).expect("original library should persist");
        let original_bytes = fs::read(&library_path).expect("persisted library should be readable");
        fs::write(&source, b"source replaced after preflight")
            .expect("stale source should be writable");

        let error = import_into_library_at(library.clone(), decoded, &library_path)
            .expect_err("stale main import proof must be rejected");

        assert!(error.contains("changed after preflight"));
        assert_eq!(library, persistence_fixture());
        assert_eq!(
            fs::read(&library_path).expect("library should remain readable"),
            original_bytes
        );
        assert_eq!(
            load_library_at(&library_path).expect("library should reload"),
            library
        );
    }

    #[test]
    fn stale_assigned_reference_proof_leaves_library_and_persisted_snapshot_unchanged() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (source, decoded) = decoded_audio_fixture(&directory.path);
        let library = persistence_fixture();
        persist_library_at(&library, &library_path).expect("original library should persist");
        let original_bytes = fs::read(&library_path).expect("persisted library should be readable");
        fs::write(&source, b"source replaced after preflight")
            .expect("stale source should be writable");

        let error = set_reference_track_at(library.clone(), "track-1", decoded, &library_path)
            .expect_err("stale assigned reference proof must be rejected");

        assert!(error.contains("changed after preflight"));
        assert_eq!(library, persistence_fixture());
        assert_eq!(
            fs::read(&library_path).expect("library should remain readable"),
            original_bytes
        );
        assert_eq!(
            load_library_at(&library_path).expect("library should reload"),
            library
        );
    }

    #[test]
    fn stale_catalog_proof_leaves_library_and_persisted_snapshot_unchanged() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (source, decoded) = decoded_audio_fixture(&directory.path);
        let library = persistence_fixture();
        persist_library_at(&library, &library_path).expect("original library should persist");
        let original_bytes = fs::read(&library_path).expect("persisted library should be readable");
        fs::write(&source, b"source replaced after preflight")
            .expect("stale source should be writable");

        let error = add_reference_track_at(library.clone(), decoded, &library_path)
            .expect_err("stale catalog proof must be rejected");

        assert!(error.contains("changed after preflight"));
        assert_eq!(library, persistence_fixture());
        assert_eq!(
            fs::read(&library_path).expect("library should remain readable"),
            original_bytes
        );
        assert_eq!(
            load_library_at(&library_path).expect("library should reload"),
            library
        );
    }

    #[test]
    fn library_round_trips_through_json() {
        let library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/tmp/night-drive.wav"),
                source_proof: None,
                reference_path: Some(PathBuf::from("/tmp/reference.wav")),
                size: 42,
                favorite: true,
                stage: TrackStage::Mixdown,
                status: TrackStatus::Maybe,
                notes: vec![Note {
                    id: String::from("note-1"),
                    time_millis: 1_250,
                    body: String::from("Check the kick tail."),
                    done: false,
                }],
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new(),
            planner_order: Vec::new(),
        };
        let encoded = serde_json::to_string(&library).expect("library should encode");
        assert!(encoded.contains(r#""status":"maybe""#));
        let decoded: Library = serde_json::from_str(&encoded).expect("library should decode");
        assert_eq!(decoded, library);
    }

    #[test]
    fn older_library_records_default_reference_path_to_none() {
        let encoded = r#"{
            "tracks": [{
                "id": "track-1",
                "title": "Night Drive",
                "original_name": "night-drive.wav",
                "path": "/tmp/night-drive.wav",
                "size": 42,
                "favorite": false,
                "stage": "sound-design",
                "notes": []
            }],
            "selected_track_id": "track-1"
        }"#;
        let library: Library = serde_json::from_str(encoded).expect("legacy library should decode");
        assert_eq!(library.tracks[0].reference_path, None);
        assert_eq!(library.tracks[0].status, TrackStatus::Inbox);
    }

    #[test]
    fn older_reference_catalog_records_default_comments_to_empty() {
        let encoded = r#"{
            "tracks": [],
            "selected_track_id": null,
            "reference_tracks": [{"path": "/tmp/reference.wav"}]
        }"#;
        let library: Library =
            serde_json::from_str(encoded).expect("legacy reference catalog should decode");

        assert_eq!(
            library.reference_tracks[0].path,
            PathBuf::from("/tmp/reference.wav")
        );
        assert!(library.reference_tracks[0].notes.is_empty());
    }

    #[test]
    fn loading_legacy_selected_references_normalizes_the_catalog() {
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
                source_proof: None,
                reference_path: Some(PathBuf::from("/external/reference.wav")),
                size: 0,
                favorite: false,
                stage: TrackStage::SoundDesign,
                status: TrackStatus::Inbox,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new(),
            planner_order: Vec::new(),
        };

        normalize_reference_tracks(&mut library);

        assert_eq!(library.reference_tracks.len(), 1);
        assert_eq!(
            library.reference_tracks[0].path,
            PathBuf::from("/external/reference.wav")
        );
        assert!(library.reference_tracks[0].notes.is_empty());
    }

    #[test]
    fn adding_a_reference_catalog_entry_does_not_assign_main_tracks() {
        let primary_path = PathBuf::from("/external/primary.wav");
        let reference_path = PathBuf::from("/external/catalog-reference.wav");
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Primary"),
                original_name: String::from("primary.wav"),
                path: primary_path.clone(),
                source_proof: None,
                reference_path: None,
                size: 42,
                favorite: false,
                stage: TrackStage::SoundDesign,
                status: TrackStatus::Inbox,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new(),
            planner_order: Vec::new(),
        };

        ensure_reference_track(&mut library, reference_path.clone());

        assert_eq!(library.reference_tracks.len(), 1);
        assert_eq!(library.reference_tracks[0].path, reference_path);
        assert_eq!(library.tracks[0].path, primary_path);
        assert_eq!(library.tracks[0].reference_path, None);
    }

    #[test]
    fn removing_a_reference_clears_all_matching_assignments_and_preserves_other_catalog_entries() {
        let removed_path = PathBuf::from("/external/remove-reference.wav");
        let retained_path = PathBuf::from("/external/keep-reference.wav");
        let track = |id: &str, reference_path: Option<PathBuf>| Track {
            id: id.to_string(),
            title: id.to_string(),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            source_proof: None,
            reference_path,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };
        let mut library = Library {
            tracks: vec![
                track("assigned-1", Some(removed_path.clone())),
                track("assigned-2", Some(removed_path.clone())),
                track("retained", Some(retained_path.clone())),
            ],
            selected_track_id: Some(String::from("assigned-1")),
            reference_tracks: vec![
                ReferenceTrack {
                    path: removed_path.clone(),
                    source_proof: None,
                    notes: vec![Note {
                        id: String::from("removed-note"),
                        time_millis: 100,
                        body: String::from("Discard with the catalog entry."),
                        done: false,
                    }],
                },
                ReferenceTrack {
                    path: retained_path.clone(),
                    source_proof: None,
                    notes: Vec::new(),
                },
            ],
            planner_order: Vec::new(),
        };

        assert_eq!(
            remove_reference_track(&mut library, &removed_path)
                .expect("catalog reference should exist"),
            2
        );
        assert!(
            library
                .reference_tracks
                .iter()
                .all(|reference| reference.path != removed_path)
        );
        assert_eq!(library.reference_tracks.len(), 1);
        assert_eq!(library.tracks[0].reference_path, None);
        assert_eq!(library.tracks[1].reference_path, None);
        assert_eq!(library.tracks[2].reference_path, Some(retained_path));
    }

    #[test]
    fn reference_selection_keeps_comments_bound_to_each_reference_path() {
        let first_path = PathBuf::from("/external/first-reference.wav");
        let second_path = PathBuf::from("/external/second-reference.wav");
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
                source_proof: None,
                reference_path: Some(first_path.clone()),
                size: 0,
                favorite: false,
                stage: TrackStage::SoundDesign,
                status: TrackStatus::Inbox,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: vec![
                ReferenceTrack {
                    path: first_path.clone(),
                    source_proof: None,
                    notes: vec![Note {
                        id: String::from("first-note"),
                        time_millis: 100,
                        body: String::from("First reference only."),
                        done: false,
                    }],
                },
                ReferenceTrack {
                    path: second_path.clone(),
                    source_proof: None,
                    notes: vec![Note {
                        id: String::from("second-note"),
                        time_millis: 200,
                        body: String::from("Second reference only."),
                        done: false,
                    }],
                },
            ],
            planner_order: Vec::new(),
        };

        assert!(
            set_reference_track_selection(&mut library, "track-1", second_path.clone())
                .expect("second reference should be selectable")
        );
        assert_eq!(library.tracks[0].reference_path, Some(second_path));
        assert_eq!(library.reference_tracks[0].notes[0].id, "first-note");
        assert_eq!(library.reference_tracks[1].notes[0].id, "second-note");
        assert!(
            set_reference_track_selection(&mut library, "track-1", first_path)
                .expect("first reference should be selectable")
        );
    }

    #[test]
    fn setting_a_track_status_reports_real_changes_and_preserves_track_metadata() {
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
                source_proof: None,
                reference_path: Some(PathBuf::from("/external/reference.wav")),
                size: 42,
                favorite: true,
                stage: TrackStage::Mixdown,
                status: TrackStatus::Inbox,
                notes: vec![Note {
                    id: String::from("note-1"),
                    time_millis: 1_250,
                    body: String::from("Keep the vocal entrance."),
                    done: false,
                }],
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new(),
            planner_order: Vec::new(),
        };

        assert!(
            !set_track_status(&mut library, "track-1", TrackStatus::Inbox)
                .expect("track should exist")
        );
        assert!(
            set_track_status(&mut library, "track-1", TrackStatus::Archive)
                .expect("track should exist")
        );

        let track = &library.tracks[0];
        assert_eq!(track.status, TrackStatus::Archive);
        assert_eq!(track.stage, TrackStage::Mixdown);
        assert!(track.favorite);
        assert_eq!(track.path, PathBuf::from("/external/night-drive.wav"));
        assert_eq!(
            track.reference_path,
            Some(PathBuf::from("/external/reference.wav"))
        );
        assert_eq!(track.notes.len(), 1);
    }

    #[test]
    fn setting_reference_track_metadata_preserves_primary_track_and_comments() {
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
                source_proof: None,
                reference_path: None,
                size: 42,
                favorite: true,
                stage: TrackStage::Mixdown,
                status: TrackStatus::Refine,
                notes: vec![Note {
                    id: String::from("note-1"),
                    time_millis: 1_250,
                    body: String::from("Keep the vocal entrance."),
                    done: false,
                }],
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new(),
            planner_order: Vec::new(),
        };

        set_reference_track_metadata(
            &mut library,
            "track-1",
            PathBuf::from("/external/reference.wav"),
        )
        .expect("the track should exist");

        let track = &library.tracks[0];
        assert_eq!(track.path, PathBuf::from("/external/night-drive.wav"));
        assert_eq!(
            track.reference_path,
            Some(PathBuf::from("/external/reference.wav"))
        );
        assert_eq!(track.notes.len(), 1);
        assert!(track.favorite);
        assert_eq!(track.stage, TrackStage::Mixdown);
    }
}

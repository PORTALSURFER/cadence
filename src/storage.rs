use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::fd::AsRawFd;

const AUDIO_EXTENSIONS: &[&str] = &["aac", "aiff", "flac", "m4a", "mp3", "ogg", "opus", "wav"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub tracks: Vec<Track>,
    pub selected_track_id: Option<String>,
    #[serde(default)]
    pub reference_tracks: Vec<ReferenceTrack>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceTrack {
    pub path: PathBuf,
    #[serde(default)]
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub original_name: String,
    pub path: PathBuf,
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
    let path = library_path();
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let mut library: Library = serde_json::from_str(&contents)
                .map_err(|error| format!("Could not parse {}: {error}", path.display()))?;
            normalize_reference_tracks(&mut library);
            Ok(library)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Library::default()),
        Err(error) => Err(format!("Could not read {}: {error}", path.display())),
    }
}

pub fn import_into_library(mut library: Library, path: PathBuf) -> Result<Library, String> {
    validate_audio_path(&path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }

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
        reference_path: None,
        size: metadata.len(),
        favorite: false,
        stage: TrackStage::SoundDesign,
        status: TrackStatus::Inbox,
        notes: Vec::new(),
    });
    library.selected_track_id = Some(id);
    persist_library(&library)?;
    Ok(library)
}

/// Replace the source file for one existing track while preserving its stable
/// identity, favorite state, and workflow stage. A replacement is a new audio
/// version, so timestamped comments are intentionally cleared.
pub fn replace_track(
    mut library: Library,
    track_id: &str,
    path: PathBuf,
) -> Result<Library, String> {
    validate_audio_path(&path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }

    replace_track_metadata(&mut library, track_id, path, metadata.len())?;
    persist_library(&library)?;
    Ok(library)
}

/// Associate a second audio file with one track. The reference is kept as an
/// external path, just like the primary source, so importing it never copies or
/// mutates the user's audio file.
pub fn set_reference_track(
    mut library: Library,
    track_id: &str,
    path: PathBuf,
) -> Result<Library, String> {
    validate_audio_path(&path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }

    set_reference_track_metadata(&mut library, track_id, path)?;
    persist_library(&library)?;
    Ok(library)
}

/// Add an audio file to the global reference catalog without assigning it to
/// any main track. The catalog stores the external path only; importing a
/// reference never copies or mutates the user's audio file.
pub fn add_reference_track(mut library: Library, path: PathBuf) -> Result<Library, String> {
    validate_audio_path(&path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }

    ensure_reference_track(&mut library, path);
    persist_library(&library)?;
    Ok(library)
}

fn set_reference_track_metadata(
    library: &mut Library,
    track_id: &str,
    path: PathBuf,
) -> Result<(), String> {
    ensure_reference_track(library, path.clone());
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
            notes: Vec::new(),
        });
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

fn replace_track_metadata(
    library: &mut Library,
    track_id: &str,
    path: PathBuf,
    size: u64,
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
    Ok((index, library.tracks.remove(index)))
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
    let directory = path
        .parent()
        .ok_or_else(|| format!("No parent directory for {}", path.display()))?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library.json");
    let temporary_path = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    let encoded = serde_json::to_vec_pretty(library)
        .map_err(|error| format!("Could not encode library: {error}"))?;
    fs::write(&temporary_path, encoded)
        .map_err(|error| format!("Could not write {}: {error}", temporary_path.display()))?;
    if let Err(error) = fs::rename(&temporary_path, &path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(format!("Could not replace {}: {error}", path.display()));
    }
    Ok(())
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
                notes: vec![Note {
                    id: String::from("reference-note-1"),
                    time_millis: 900,
                    body: String::from("Compare the low-end tail."),
                    done: false,
                }],
            }],
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
                reference_path: None,
                size: 42,
                favorite: false,
                stage: TrackStage::SoundDesign,
                status: TrackStatus::Inbox,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new(),
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

    #[test]
    fn replacing_track_metadata_updates_source_and_clears_comments() {
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
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
    fn library_round_trips_through_json() {
        let library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/tmp/night-drive.wav"),
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
                reference_path: Some(PathBuf::from("/external/reference.wav")),
                size: 0,
                favorite: false,
                stage: TrackStage::SoundDesign,
                status: TrackStatus::Inbox,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new(),
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
                reference_path: None,
                size: 42,
                favorite: false,
                stage: TrackStage::SoundDesign,
                status: TrackStatus::Inbox,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new(),
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
                    notes: vec![Note {
                        id: String::from("removed-note"),
                        time_millis: 100,
                        body: String::from("Discard with the catalog entry."),
                        done: false,
                    }],
                },
                ReferenceTrack {
                    path: retained_path.clone(),
                    notes: Vec::new(),
                },
            ],
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
                    notes: vec![Note {
                        id: String::from("first-note"),
                        time_millis: 100,
                        body: String::from("First reference only."),
                        done: false,
                    }],
                },
                ReferenceTrack {
                    path: second_path.clone(),
                    notes: vec![Note {
                        id: String::from("second-note"),
                        time_millis: 200,
                        body: String::from("Second reference only."),
                        done: false,
                    }],
                },
            ],
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

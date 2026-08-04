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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub original_name: String,
    pub path: PathBuf,
    pub size: u64,
    pub favorite: bool,
    pub stage: TrackStage,
    pub notes: Vec<Note>,
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
        Ok(contents) => serde_json::from_str(&contents)
            .map_err(|error| format!("Could not parse {}: {error}", path.display())),
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
        size: metadata.len(),
        favorite: false,
        stage: TrackStage::SoundDesign,
        notes: Vec::new(),
    });
    library.selected_track_id = Some(id);
    persist_library(&library)?;
    Ok(library)
}

pub fn remove_track(library: &mut Library, track_id: &str) -> Result<(usize, Track), String> {
    let index = library
        .tracks
        .iter()
        .position(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    Ok((index, library.tracks.remove(index)))
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
    fn removing_a_track_only_changes_library_metadata() {
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
                size: 42,
                favorite: false,
                stage: TrackStage::SoundDesign,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("track-1")),
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
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            notes: Vec::new(),
        };
        let library = Library {
            tracks: vec![track("track-2"), track("track-3")],
            selected_track_id: None,
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
                size: 42,
                favorite: false,
                stage: TrackStage::SoundDesign,
                notes: Vec::new(),
            }],
            selected_track_id: Some(String::from("track-1")),
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
    fn library_round_trips_through_json() {
        let library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/tmp/night-drive.wav"),
                size: 42,
                favorite: true,
                stage: TrackStage::Mixdown,
                notes: vec![Note {
                    id: String::from("note-1"),
                    time_millis: 1_250,
                    body: String::from("Check the kick tail."),
                    done: false,
                }],
            }],
            selected_track_id: Some(String::from("track-1")),
        };
        let encoded = serde_json::to_string(&library).expect("library should encode");
        let decoded: Library = serde_json::from_str(&encoded).expect("library should decode");
        assert_eq!(decoded, library);
    }
}

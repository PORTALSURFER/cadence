use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::{self, ErrorKind, Read, Write},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(all(test, unix))]
use std::cell::Cell;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const AUDIO_EXTENSIONS: &[&str] = &["aac", "aiff", "flac", "m4a", "mp3", "ogg", "opus", "wav"];
const MAX_LIBRARY_BYTES: usize = 16 * 1024 * 1024;
const MAX_TEMP_FILE_ALLOCATION_ATTEMPTS: usize = 128;
const RECOVERY_COPY_BUFFER_BYTES: usize = 64 * 1024;

static NEXT_TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

#[cfg(all(test, unix))]
thread_local! {
    static FAIL_NEXT_PERSIST_PARENT_DIRECTORY_SYNC: Cell<bool> = const { Cell::new(false) };
}

/// A persistable vector whose backing allocation is shared until a mutation.
///
/// `SharedVec` serializes exactly like the contained `Vec`, so changing the
/// in-memory ownership model does not change the library JSON shape.
#[derive(Debug, PartialEq, Eq)]
pub struct SharedVec<T> {
    values: Arc<Vec<T>>,
}

impl<T> SharedVec<T> {
    pub fn new() -> Self {
        Self {
            values: Arc::new(Vec::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.values, &other.values)
    }

    pub fn as_slice(&self) -> &[T] {
        self.values.as_slice()
    }
}

impl<T: Clone> SharedVec<T> {
    pub fn as_mut_vec(&mut self) -> &mut Vec<T> {
        Arc::make_mut(&mut self.values)
    }

    pub fn push(&mut self, value: T) {
        self.as_mut_vec().push(value);
    }

    pub fn remove(&mut self, index: usize) -> T {
        self.as_mut_vec().remove(index)
    }

    pub fn retain(&mut self, f: impl FnMut(&T) -> bool) {
        self.as_mut_vec().retain(f);
    }

    pub fn clear(&mut self) {
        self.as_mut_vec().clear();
    }

    pub fn insert(&mut self, index: usize, value: T) {
        self.as_mut_vec().insert(index, value);
    }
}

impl<T> From<Vec<T>> for SharedVec<T> {
    fn from(values: Vec<T>) -> Self {
        Self {
            values: Arc::new(values),
        }
    }
}

impl<T> FromIterator<T> for SharedVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from(iter.into_iter().collect::<Vec<_>>())
    }
}

impl<T: Clone> Extend<T> for SharedVec<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.as_mut_vec().extend(iter);
    }
}

impl<T> Deref for SharedVec<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T: Clone> DerefMut for SharedVec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_vec().as_mut_slice()
    }
}

impl<T> Default for SharedVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for SharedVec<T> {
    fn clone(&self) -> Self {
        Self {
            values: Arc::clone(&self.values),
        }
    }
}

impl<'a, T> IntoIterator for &'a SharedVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T: Clone> IntoIterator for &'a mut SharedVec<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T: Serialize> Serialize for SharedVec<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_slice().serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for SharedVec<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<T>::deserialize(deserializer).map(Self::from)
    }
}

/// A persistable entity collection that shares the immutable entity payloads
/// as well as the collection allocation.  Mutating the collection clones the
/// collection, while mutating one indexed entity clones only that entity.
/// This keeps background save snapshots isolated without copying unrelated
/// tracks or reference tracks.
#[derive(Debug, PartialEq, Eq)]
pub struct SharedEntityVec<T> {
    values: Arc<Vec<Arc<T>>>,
}

impl<T> SharedEntityVec<T> {
    pub fn new() -> Self {
        Self {
            values: Arc::new(Vec::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.values, &other.values)
    }

    #[cfg(test)]
    pub(crate) fn shares_entity_storage_with(&self, other: &Self, index: usize) -> bool {
        self.values
            .get(index)
            .zip(other.values.get(index))
            .is_some_and(|(left, right)| Arc::ptr_eq(left, right))
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn iter(&self) -> SharedEntityVecIter<'_, T> {
        SharedEntityVecIter {
            inner: self.values.iter(),
        }
    }

    pub fn position(&self, predicate: impl FnMut(&T) -> bool) -> Option<usize> {
        self.iter().position(predicate)
    }

    pub fn iter_mut(&mut self) -> SharedEntityVecIterMut<'_, T>
    where
        T: Clone,
    {
        SharedEntityVecIterMut {
            inner: self.as_mut_vec().iter_mut(),
        }
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.values.get(index).map(Arc::as_ref)
    }

    pub fn first(&self) -> Option<&T> {
        self.get(0)
    }

    #[cfg(test)]
    pub(crate) fn entity_pointer(&self, index: usize) -> Option<*const T> {
        self.get(index).map(std::ptr::from_ref)
    }
}

impl<T: Clone> SharedEntityVec<T> {
    fn as_mut_vec(&mut self) -> &mut Vec<Arc<T>> {
        Arc::make_mut(&mut self.values)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.as_mut_vec().get_mut(index).map(Arc::make_mut)
    }

    pub fn find_mut(&mut self, predicate: impl FnMut(&T) -> bool) -> Option<&mut T> {
        let index = self.position(predicate)?;
        self.get_mut(index)
    }

    pub fn first_mut(&mut self) -> Option<&mut T> {
        self.get_mut(0)
    }

    pub fn push(&mut self, value: T) {
        self.as_mut_vec().push(Arc::new(value));
    }

    pub fn remove(&mut self, index: usize) -> T {
        let value = self.as_mut_vec().remove(index);
        Arc::try_unwrap(value).unwrap_or_else(|value| (*value).clone())
    }

    pub fn retain(&mut self, mut f: impl FnMut(&T) -> bool) {
        self.as_mut_vec().retain(|value| f(value));
    }

    pub fn clear(&mut self) {
        self.as_mut_vec().clear();
    }

    pub fn insert(&mut self, index: usize, value: T) {
        self.as_mut_vec().insert(index, Arc::new(value));
    }
}

impl<T> From<Vec<T>> for SharedEntityVec<T> {
    fn from(values: Vec<T>) -> Self {
        Self {
            values: Arc::new(values.into_iter().map(Arc::new).collect()),
        }
    }
}

impl<T> FromIterator<T> for SharedEntityVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from(iter.into_iter().collect::<Vec<_>>())
    }
}

impl<T: Clone> Extend<T> for SharedEntityVec<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.as_mut_vec().extend(iter.into_iter().map(Arc::new));
    }
}

impl<T> Default for SharedEntityVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for SharedEntityVec<T> {
    fn clone(&self) -> Self {
        Self {
            values: Arc::clone(&self.values),
        }
    }
}

impl<'a, T> IntoIterator for &'a SharedEntityVec<T> {
    type Item = &'a T;
    type IntoIter = SharedEntityVecIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T: Clone> IntoIterator for &'a mut SharedEntityVec<T> {
    type Item = &'a mut T;
    type IntoIter = SharedEntityVecIterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T: Serialize> Serialize for SharedEntityVec<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for SharedEntityVec<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<T>::deserialize(deserializer).map(Self::from)
    }
}

pub struct SharedEntityVecIter<'a, T> {
    inner: std::slice::Iter<'a, Arc<T>>,
}

impl<'a, T> Iterator for SharedEntityVecIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(Arc::as_ref)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<T> ExactSizeIterator for SharedEntityVecIter<'_, T> {}

pub struct SharedEntityVecIterMut<'a, T> {
    inner: std::slice::IterMut<'a, Arc<T>>,
}

impl<'a, T: Clone> Iterator for SharedEntityVecIterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(Arc::make_mut)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<T: Clone> ExactSizeIterator for SharedEntityVecIterMut<'_, T> {}

impl<T> std::ops::Index<usize> for SharedEntityVec<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        self.values[index].as_ref()
    }
}

impl<T: Clone> std::ops::IndexMut<usize> for SharedEntityVec<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        Arc::make_mut(&mut self.as_mut_vec()[index])
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub tracks: SharedEntityVec<Track>,
    pub selected_track_id: Option<String>,
    #[serde(default)]
    pub reference_tracks: SharedEntityVec<ReferenceTrack>,
    #[serde(default)]
    pub planner_order: SharedVec<String>,
    #[serde(default)]
    pub tags: crate::tags::TagBook,
    #[serde(default)]
    pub reviewer_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceTrack {
    pub path: PathBuf,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub source_proof: crate::source::SourceProvenance,
    #[serde(default)]
    pub notes: SharedVec<Note>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BounceRevision {
    pub id: String,
    #[serde(default)]
    pub build: Option<u32>,
    pub label: String,
    pub original_name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub source_proof: crate::source::SourceProvenance,
    pub size: u64,
    #[serde(default)]
    pub notes: SharedVec<Note>,
    #[serde(default)]
    pub feedback_note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BounceCollision {
    pub id: String,
    pub build: u32,
    pub label: String,
    pub original_name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub source_proof: crate::source::SourceProvenance,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedBounceFilename {
    pub batch: String,
    pub slot: u8,
    pub group_key: String,
    pub build: u32,
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BounceImportOutcome {
    CreatedTrack {
        track_id: String,
        revision_id: String,
    },
    AddedRevision {
        track_id: String,
        revision_id: String,
        promoted_current: bool,
    },
    DuplicateIgnored {
        track_id: String,
        revision_id: String,
    },
    CollisionFlagged {
        track_id: String,
        collision_id: String,
        build: u32,
    },
}

impl BounceImportOutcome {
    pub fn track_id(&self) -> &str {
        match self {
            Self::CreatedTrack { track_id, .. }
            | Self::AddedRevision { track_id, .. }
            | Self::DuplicateIgnored { track_id, .. }
            | Self::CollisionFlagged { track_id, .. } => track_id,
        }
    }
}

/// Parse `<BATCH><SLOT>_b<BUILD>[__<track-title>].<audio extension>`.
///
/// - `<BATCH>`: one or more uppercase ASCII letters (`A`–`Z`, `AA`, `AB`, ...)
/// - `<SLOT>`: single ASCII digit `0`–`9`
/// - `_b` or legacy `_B` followed by a positive numeric build (leading zeros ignored)
/// - optional `__<track-title>` suffix; untitled files are valid
/// - audio file extension must be one of `AUDIO_EXTENSIONS`
pub fn parse_bounce_filename(path: &Path) -> Option<ParsedBounceFilename> {
    validate_audio_path(path).ok()?;
    let stem = path.file_stem()?.to_str()?;
    let bytes = stem.as_bytes();
    let mut i = 0;

    while i < bytes.len() && bytes[i].is_ascii_uppercase() {
        i += 1;
    }
    if i == 0 || i >= bytes.len() {
        return None;
    }
    let batch = stem[..i].to_string();

    if !bytes[i].is_ascii_digit() {
        return None;
    }
    let slot = bytes[i] - b'0';
    i += 1;

    if i + 2 >= bytes.len() || bytes[i] != b'_' || !matches!(bytes[i + 1], b'b' | b'B') {
        return None;
    }
    i += 2;

    let build_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == build_start {
        return None;
    }
    let build = stem[build_start..i].parse::<u32>().ok()?;
    if build == 0 {
        return None;
    }

    let title = if i == bytes.len() {
        None
    } else if stem[i..].starts_with("__") {
        let raw = &stem[i + 2..];
        let cleaned = clean_title_fragment(raw);
        if cleaned.is_empty() {
            None
        } else {
            Some(cleaned)
        }
    } else {
        return None;
    };

    let group_key = format!("{batch}{slot}");
    Some(ParsedBounceFilename {
        batch,
        slot,
        group_key,
        build,
        title,
    })
}

fn clean_title_fragment(raw: &str) -> String {
    raw.replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub original_name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub source_proof: crate::source::SourceProvenance,
    #[serde(default)]
    pub reference_path: Option<PathBuf>,
    pub size: u64,
    pub favorite: bool,
    pub stage: TrackStage,
    pub notes: SharedVec<Note>,
    #[serde(default)]
    pub group_key: Option<String>,
    #[serde(default)]
    pub next_action: String,
    #[serde(default)]
    pub revisions: SharedVec<BounceRevision>,
    #[serde(default)]
    pub active_revision_id: Option<String>,
    #[serde(default)]
    pub current_revision_id: Option<String>,
    #[serde(default)]
    pub collisions: SharedVec<BounceCollision>,
    #[serde(default)]
    pub external: bool,
}

impl Default for Track {
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            original_name: String::new(),
            path: PathBuf::new(),
            source_proof: crate::source::SourceProvenance::Unknown,
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Backlog,
            notes: SharedVec::default(),
            group_key: None,
            next_action: String::new(),
            revisions: SharedVec::default(),
            active_revision_id: None,
            current_revision_id: None,
            collisions: SharedVec::default(),
            external: false,
        }
    }
}

impl Track {
    pub fn source_provenance(&self) -> &crate::source::SourceProvenance {
        &self.source_proof
    }

    pub fn effective_group_key(&self) -> Option<String> {
        self.group_key.clone().or_else(|| {
            parse_bounce_filename(Path::new(&self.original_name))
                .or_else(|| parse_bounce_filename(&self.path))
                .map(|parsed| parsed.group_key)
        })
    }

    pub fn current_revision(&self) -> Option<&BounceRevision> {
        if let Some(id) = self.current_revision_id.as_deref()
            && let Some(rev) = self.revisions.iter().find(|rev| rev.id == id)
        {
            return Some(rev);
        }
        self.revisions.first()
    }

    pub fn active_revision(&self) -> Option<&BounceRevision> {
        if let Some(id) = self.active_revision_id.as_deref()
            && let Some(rev) = self.revisions.iter().find(|rev| rev.id == id)
        {
            return Some(rev);
        }
        self.current_revision()
    }

    pub fn current_build_label(&self) -> String {
        if let Some(rev) = self.current_revision() {
            return rev.label.clone();
        }
        if let Some(parsed) = parse_bounce_filename(Path::new(&self.original_name))
            .or_else(|| parse_bounce_filename(&self.path))
        {
            return format!("b{}", parsed.build);
        }
        String::from("v1")
    }

    pub fn active_build_label(&self) -> String {
        if let Some(rev) = self.active_revision() {
            return rev.label.clone();
        }
        self.current_build_label()
    }

    pub fn current_bounce_badge(&self) -> String {
        let build = self.current_build_label();
        match self.effective_group_key() {
            Some(group_key) => format!("{group_key} · {build}"),
            None => build,
        }
    }

    pub fn is_viewing_prior_build(&self) -> bool {
        match (
            self.active_revision_id.as_deref(),
            self.current_revision_id.as_deref(),
        ) {
            (Some(active), Some(current)) => active != current,
            _ => false,
        }
    }

    pub fn revision_count(&self) -> usize {
        self.revisions.len().max(1)
    }

    pub fn is_source_missing(&self) -> bool {
        !self.path.as_os_str().is_empty() && !self.path.is_file()
    }

    pub fn display_next_action(&self) -> Option<String> {
        let trimmed = self.next_action.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
        let open_note = self
            .current_revision()
            .map(|rev| rev.notes.as_slice())
            .unwrap_or(self.notes.as_slice())
            .iter()
            .find(|note| !note.done)?;
        let body = open_note.body.replace(['\n', '\r'], " ");
        let body = body.trim();
        if body.is_empty() {
            None
        } else {
            Some(body.to_string())
        }
    }
}

impl ReferenceTrack {
    pub fn source_provenance(&self) -> &crate::source::SourceProvenance {
        &self.source_proof
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackStage {
    #[serde(rename = "sound-design", alias = "backlog")]
    Backlog,
    #[serde(
        rename = "production",
        alias = "groove",
        alias = "arrangement",
        alias = "polish"
    )]
    Production,
    #[serde(rename = "mixdown")]
    Mixdown,
    #[serde(rename = "mastering", alias = "master", alias = "release")]
    Mastering,
}

impl TrackStage {
    pub const ALL: [Self; 4] = [
        Self::Backlog,
        Self::Production,
        Self::Mixdown,
        Self::Mastering,
    ];
    pub const COUNT: usize = Self::ALL.len();
    pub const fn label(self) -> &'static str {
        match self {
            Self::Backlog => "Backlog",
            Self::Production => "Production",
            Self::Mixdown => "Mixdown",
            Self::Mastering => "Mastering",
        }
    }
    pub const fn index(self) -> usize {
        match self {
            Self::Backlog => 0,
            Self::Production => 1,
            Self::Mixdown => 2,
            Self::Mastering => 3,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub time_millis: u64,
    pub body: String,
    pub done: bool,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub package_id: Option<String>,
}

/// The compact, source-proofed portion of a decoded import that is retained
/// while the rest of one logical batch is preflighted.  Waveform payloads are
/// deliberately not retained here; the library stores only this metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedImportCandidate {
    path: PathBuf,
    source_proof: crate::source::AudioSourceProof,
    source_stamp: crate::source::SourceFileStamp,
}

impl VerifiedImportCandidate {
    pub fn from_decoded(decoded: &crate::audio::DecodedAudioFile) -> Self {
        Self {
            path: decoded.path().to_path_buf(),
            source_proof: decoded.source_proof().clone(),
            source_stamp: decoded.source_stamp(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchImportError {
    pub path: PathBuf,
    pub error: String,
}

/// The result of the ordered persistence protocol after the temporary file has
/// been replaced. A directory-sync failure is reported as a committed result
/// because the new snapshot is already authoritative at the rename point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistenceOutcome {
    Durable,
    CommittedButDurabilityUncertain { detail: String },
}

impl PersistenceOutcome {
    pub fn is_durability_uncertain(&self) -> bool {
        matches!(self, Self::CommittedButDurabilityUncertain { .. })
    }

    pub fn durability_warning(&self) -> Option<String> {
        match self {
            Self::Durable => None,
            Self::CommittedButDurabilityUncertain { detail } => {
                Some(format!("Crash durability is unconfirmed: {detail}"))
            }
        }
    }
}

/// A value whose snapshot was committed, together with the durability outcome
/// of the final parent-directory sync.
#[derive(Clone, Debug, PartialEq)]
pub struct Persisted<T> {
    pub value: T,
    pub outcome: PersistenceOutcome,
}

impl<T> Persisted<T> {
    fn new(value: T, outcome: PersistenceOutcome) -> Self {
        Self { value, outcome }
    }
}

impl<T> Deref for Persisted<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchImportReport {
    pub library: Option<Library>,
    pub imported_paths: Vec<PathBuf>,
    pub errors: Vec<BatchImportError>,
    pub persistence_outcome: Option<PersistenceOutcome>,
}

impl BatchImportReport {
    fn failed(path: PathBuf, error: String) -> Self {
        Self {
            library: None,
            imported_paths: Vec::new(),
            errors: vec![BatchImportError { path, error }],
            persistence_outcome: None,
        }
    }
}

/// Validate the identity keys that are used to address persisted library data.
///
/// Track IDs are library-wide. Note IDs are scoped to their owning track, so
/// the same note ID may appear on a main track and a reference track (or on
/// two different owners) without becoming ambiguous.
pub fn validate_library_identity(library: &Library) -> Result<(), String> {
    library.tags.validate()?;
    let mut track_ids = HashSet::with_capacity(library.tracks.len());
    for track in &library.tracks {
        if !track_ids.insert(track.id.as_str()) {
            return Err(format!(
                "duplicate track ID '{}' across the library; remove or rename one of the duplicates before continuing",
                track.id
            ));
        }
        validate_note_ids(&format!("track '{}'", track.id), &track.notes)?;
        let mut revision_ids = HashSet::with_capacity(track.revisions.len());
        for revision in &track.revisions {
            if !revision_ids.insert(revision.id.as_str()) {
                return Err(format!(
                    "duplicate bounce revision ID '{}' in track '{}'; remove or rename one of the duplicates before continuing",
                    revision.id, track.id
                ));
            }
            validate_note_ids(
                &format!("track '{}' revision '{}'", track.id, revision.id),
                &revision.notes,
            )?;
        }
        let mut collision_ids = HashSet::with_capacity(track.collisions.len());
        for collision in &track.collisions {
            if !collision_ids.insert(collision.id.as_str()) {
                return Err(format!(
                    "duplicate bounce collision ID '{}' in track '{}'; remove or rename one of the duplicates before continuing",
                    collision.id, track.id
                ));
            }
        }
    }

    if library
        .tags
        .assignments
        .keys()
        .any(|id| !track_ids.contains(id.as_str()))
    {
        return Err("A tag assignment points to a missing track".into());
    }
    let mut reference_paths = HashSet::with_capacity(library.reference_tracks.len());
    for reference in &library.reference_tracks {
        if !reference_paths.insert(reference.path.as_path()) {
            return Err(format!(
                "duplicate reference path '{}' across the library; remove or rename one of the duplicates before continuing",
                reference.path.display()
            ));
        }
        validate_note_ids(
            &format!("reference track '{}'", reference.path.display()),
            &reference.notes,
        )?;
    }

    Ok(())
}

fn validate_note_ids(owner: &str, notes: &[Note]) -> Result<(), String> {
    let mut note_ids = HashSet::with_capacity(notes.len());
    for note in notes {
        if !note_ids.insert(note.id.as_str()) {
            return Err(format!(
                "duplicate note ID '{}' in {owner}; remove or rename one of the duplicates before continuing",
                note.id
            ));
        }
    }
    Ok(())
}

/// Allocate a new track ID without changing any existing IDs.
pub fn allocate_track_id(library: &Library) -> String {
    allocate_id("track", unique_id(), |candidate| {
        library.tracks.iter().any(|track| track.id == candidate)
    })
}

/// Allocate a new note ID within one note owner without changing any existing
/// IDs. Callers provide only the owning note collection so note IDs remain
/// valid to reuse under different owners.
pub fn allocate_note_id(notes: &[Note]) -> String {
    allocate_id("note", unique_id(), |candidate| {
        notes.iter().any(|note| note.id == candidate)
    })
}

pub fn allocate_revision_id(revisions: &[BounceRevision]) -> String {
    allocate_id("rev", unique_id(), |candidate| {
        revisions.iter().any(|rev| rev.id == candidate)
    })
}

fn allocate_collision_id(collisions: &[BounceCollision]) -> String {
    allocate_id("col", unique_id(), |candidate| {
        collisions.iter().any(|col| col.id == candidate)
    })
}

fn allocate_id(prefix: &str, epoch_nanos: u128, is_occupied: impl Fn(&str) -> bool) -> String {
    let base = format!("{prefix}-{epoch_nanos}");
    if !is_occupied(&base) {
        return base;
    }

    let mut suffix = 1_u128;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !is_occupied(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
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
    let (mut file, metadata) = match open_admitted_regular_file(path) {
        Ok(admitted) => admitted,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Library::default()),
        Err(error) => return Err(format!("Could not read {}: {error}", path.display())),
    };
    if metadata.len() > MAX_LIBRARY_BYTES as u64 {
        return Err(library_size_limit_error(path));
    }
    let bytes = read_library_bytes(&mut file)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    if bytes.len() > MAX_LIBRARY_BYTES {
        return Err(library_size_limit_error(path));
    }

    let mut library: Library = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Could not parse {}: {error}", path.display()))?;
    validate_library_identity(&library)
        .map_err(|error| format!("Could not validate {}: {error}", path.display()))?;
    normalize_reference_tracks(&mut library);
    normalize_planner_order(&mut library);
    Ok(library)
}

fn open_admitted_regular_file(path: &Path) -> io::Result<(fs::File, fs::Metadata)> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);

    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    Ok((file, metadata))
}

fn read_library_bytes<R: Read>(reader: R) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_LIBRARY_BYTES as u64) + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn library_size_limit_error(path: &Path) -> String {
    format!(
        "Could not load {}: the library exceeds the maximum supported size of {MAX_LIBRARY_BYTES} bytes; reduce or move the file before retrying",
        path.display()
    )
}

/// Preserve an unreadable library before replacing it with a fresh snapshot.
///
/// The original bytes are copied to a unique same-directory backup using
/// `create_new`, flushed, synced, and closed; its directory entry is then
/// synced so the backup contents and directory entry are durable before the
/// active library is replaced. Backups are intentionally never removed by
/// this helper.
pub fn preserve_unreadable_library_and_start_fresh() -> Result<Persisted<PathBuf>, String> {
    preserve_unreadable_library_and_start_fresh_at(&library_path())
}

pub fn preserve_unreadable_library_and_start_fresh_at(
    path: &Path,
) -> Result<Persisted<PathBuf>, String> {
    let (mut source_file, source_metadata) = open_admitted_regular_file(path)
        .map_err(|error| format!("Could not preserve {}: {error}", path.display()))?;
    let source_length = source_metadata.len();
    let directory = path
        .parent()
        .ok_or_else(|| format!("No parent directory for {}", path.display()))?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;

    let (backup_path, mut backup_file) = create_unique_recovery_backup(path)?;
    if let Err(error) = copy_exact_file_bytes(&mut source_file, &mut backup_file, source_length) {
        drop(backup_file);
        return Err(with_recovery_backup_cleanup(
            format!(
                "Could not copy {} to {}: {error}",
                path.display(),
                backup_path.display()
            ),
            &backup_path,
        ));
    }
    if let Err(error) = backup_file.flush() {
        drop(backup_file);
        return Err(with_recovery_backup_cleanup(
            format!("Could not flush {}: {error}", backup_path.display()),
            &backup_path,
        ));
    }
    if let Err(error) = backup_file.sync_all() {
        drop(backup_file);
        return Err(with_recovery_backup_cleanup(
            format!("Could not sync {}: {error}", backup_path.display()),
            &backup_path,
        ));
    }
    drop(backup_file);
    drop(source_file);

    #[cfg(unix)]
    if let Err(error) = sync_parent_directory(directory) {
        return Err(format!(
            "Could not sync recovery backup directory {} before replacing the active library; active library was not replaced: {error}",
            directory.display()
        ));
    }

    let outcome = persist_library_at(&Library::default(), path)?;
    Ok(Persisted::new(backup_path, outcome))
}

fn copy_exact_file_bytes(
    source: &mut fs::File,
    destination: &mut fs::File,
    mut length: u64,
) -> io::Result<()> {
    let mut buffer = [0_u8; RECOVERY_COPY_BUFFER_BYTES];
    while length > 0 {
        let chunk_length = length.min(buffer.len() as u64) as usize;
        source.read_exact(&mut buffer[..chunk_length])?;
        destination.write_all(&buffer[..chunk_length])?;
        length -= chunk_length as u64;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn import_into_library(
    library: Library,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Persisted<Library>, String> {
    import_into_library_at(library, decoded, &library_path())
}

#[allow(dead_code)]
fn import_into_library_at(
    library: Library,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Persisted<Library>, String> {
    let persisted = import_bounce_into_library_at(library, decoded, library_path)?;
    Ok(Persisted::new(persisted.value.0, persisted.outcome))
}

/// Import one decoded bounce into the library, grouping `<BATCH><SLOT>_b<BUILD>`
/// files onto the same track by batch + slot without renaming source audio.
pub fn import_bounce_into_library(
    library: Library,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Persisted<(Library, BounceImportOutcome)>, String> {
    import_bounce_into_library_at(library, decoded, &library_path())
}

fn import_bounce_into_library_at(
    mut library: Library,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Persisted<(Library, BounceImportOutcome)>, String> {
    let path = decoded.path().to_path_buf();
    validate_audio_path(&path)?;
    ensure_decoded_audio_unchanged(&decoded)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    ensure_decoded_audio_unchanged(&decoded)?;

    let mut occupied = occupied_track_ids(&library);
    let outcome = apply_bounce_import_to_library(
        &mut library,
        path,
        metadata.len(),
        decoded.source_proof().clone(),
        &mut occupied,
    );
    normalize_planner_order(&mut library);
    library.selected_track_id = Some(outcome.track_id().to_owned());
    ensure_decoded_audio_unchanged(&decoded)?;
    let persistence = persist_library_at(&library, library_path)?;
    Ok(Persisted::new((library, outcome), persistence))
}

/// Manually attach any decoded audio file as a revision of an existing track,
/// preserving the track's stage, next action, and prior builds' notes.
pub fn add_revision_to_track(
    library: Library,
    track_id: &str,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Persisted<(Library, BounceImportOutcome)>, String> {
    add_revision_to_track_at(library, track_id, decoded, &library_path())
}

fn add_revision_to_track_at(
    mut library: Library,
    track_id: &str,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Persisted<(Library, BounceImportOutcome)>, String> {
    let path = decoded.path().to_path_buf();
    validate_audio_path(&path)?;
    ensure_decoded_audio_unchanged(&decoded)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    ensure_decoded_audio_unchanged(&decoded)?;

    let outcome = attach_bounce_to_existing_track(
        &mut library,
        track_id,
        path,
        metadata.len(),
        decoded.source_proof().clone(),
        None,
    )?;
    library.selected_track_id = Some(track_id.to_owned());
    ensure_decoded_audio_unchanged(&decoded)?;
    let persistence = persist_library_at(&library, library_path)?;
    Ok(Persisted::new((library, outcome), persistence))
}

/// Relink a moved or renamed audio file for one revision in place without
/// duplicating the track card or losing timestamped notes or build history.
pub fn relink_track_revision(
    library: Library,
    track_id: &str,
    revision_id: Option<&str>,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Persisted<Library>, String> {
    relink_track_revision_at(library, track_id, revision_id, decoded, &library_path())
}

fn relink_track_revision_at(
    mut library: Library,
    track_id: &str,
    revision_id: Option<&str>,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Persisted<Library>, String> {
    let path = decoded.path().to_path_buf();
    validate_audio_path(&path)?;
    ensure_decoded_audio_unchanged(&decoded)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    ensure_decoded_audio_unchanged(&decoded)?;

    let original_name = file_name_string(&path);
    let track = library
        .tracks
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    ensure_track_revisions_initialized(track);
    let target_rev_id = revision_id
        .map(str::to_owned)
        .or_else(|| track.active_revision_id.clone())
        .or_else(|| track.current_revision_id.clone())
        .ok_or_else(|| String::from("That bounce revision is no longer on the track."))?;
    let rev = track
        .revisions
        .iter_mut()
        .find(|rev| rev.id == target_rev_id)
        .ok_or_else(|| String::from("That bounce revision is no longer on the track."))?;
    rev.path = path;
    rev.original_name = original_name;
    rev.size = metadata.len();
    rev.source_proof = crate::source::SourceProvenance::Verified(decoded.source_proof().clone());
    sync_track_from_active_revision(track);
    library.selected_track_id = Some(track_id.to_owned());
    ensure_decoded_audio_unchanged(&decoded)?;
    let outcome = persist_library_at(&library, library_path)?;
    Ok(Persisted::new(library, outcome))
}

fn file_name_string(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled track")
        .to_string()
}

fn fallback_track_title(path: &Path) -> String {
    let title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled track")
        .replace(['_', '-'], " ");
    if title.trim().is_empty() {
        String::from("Untitled track")
    } else {
        title
    }
}

fn ensure_track_revisions_initialized(track: &mut Track) {
    if !track.revisions.is_empty() {
        if track.current_revision_id.is_none() {
            sort_and_select_current_revision(track);
        }
        if track.active_revision_id.is_none() {
            track.active_revision_id = track.current_revision_id.clone();
        }
        return;
    }
    let parsed = parse_bounce_filename(Path::new(&track.original_name))
        .or_else(|| parse_bounce_filename(&track.path));
    if track.group_key.is_none() {
        track.group_key = parsed.as_ref().map(|p| p.group_key.clone());
    }
    let rev_id = allocate_revision_id(&[]);
    let (build, label) = match parsed {
        Some(p) => (Some(p.build), format!("b{}", p.build)),
        None => (None, String::from("v1")),
    };
    track.revisions.push(BounceRevision {
        id: rev_id.clone(),
        build,
        label,
        original_name: track.original_name.clone(),
        path: track.path.clone(),
        source_proof: track.source_proof.clone(),
        size: track.size,
        notes: track.notes.clone(),
        feedback_note: None,
    });
    track.current_revision_id = Some(rev_id.clone());
    track.active_revision_id = Some(rev_id);
}

fn sync_track_from_active_revision(track: &mut Track) {
    let active = track
        .active_revision_id
        .as_deref()
        .and_then(|id| track.revisions.iter().find(|rev| rev.id == id))
        .or_else(|| track.current_revision())
        .cloned();
    if let Some(rev) = active {
        track.active_revision_id = Some(rev.id);
        track.path = rev.path;
        track.original_name = rev.original_name;
        track.source_proof = rev.source_proof;
        track.size = rev.size;
        track.notes = rev.notes;
    }
}

/// Mirror mutations made to `track.notes` onto the active `BounceRevision`
/// so switching builds never loses timestamped feedback.
pub fn sync_active_revision_notes(library: &mut Library, track_id: &str) {
    let Some(track) = library.tracks.find_mut(|track| track.id == track_id) else {
        return;
    };
    if track.revisions.is_empty() {
        return;
    }
    let notes = track.notes.clone();
    let active_id = track
        .active_revision_id
        .clone()
        .or_else(|| track.current_revision_id.clone());
    if let Some(active_id) = active_id
        && let Some(rev) = track.revisions.iter_mut().find(|rev| rev.id == active_id)
    {
        rev.notes = notes;
    }
}

fn sort_and_select_current_revision(track: &mut Track) {
    track
        .revisions
        .as_mut_vec()
        .sort_by(|left, right| match (left.build, right.build) {
            (Some(a), Some(b)) => b.cmp(&a),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
    if let Some(highest) = track.revisions.first() {
        track.current_revision_id = Some(highest.id.clone());
    }
}

fn next_unnumbered_label(revisions: &[BounceRevision]) -> String {
    let mut n = revisions.len() + 1;
    loop {
        let candidate = format!("v{n}");
        if !revisions.iter().any(|rev| rev.label == candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn apply_bounce_import_to_library(
    library: &mut Library,
    path: PathBuf,
    size: u64,
    source_proof: crate::source::AudioSourceProof,
    occupied_track_ids: &mut HashSet<String>,
) -> BounceImportOutcome {
    let parsed = parse_bounce_filename(&path);
    if let Some(ref parsed) = parsed
        && let Some(existing_id) = library
            .tracks
            .iter()
            .find(|track| track.effective_group_key().as_deref() == Some(parsed.group_key.as_str()))
            .map(|track| track.id.clone())
    {
        return attach_bounce_to_existing_track(
            library,
            &existing_id,
            path,
            size,
            source_proof,
            None,
        )
        .expect("matched track exists in library");
    }

    let original_name = file_name_string(&path);
    let title = match &parsed {
        Some(p) => p.title.clone().unwrap_or_else(|| p.group_key.clone()),
        None => fallback_track_title(&path),
    };
    let id = allocate_track_id_from_occupied(occupied_track_ids);
    let rev_id = allocate_revision_id(&[]);
    let (build, label) = match &parsed {
        Some(p) => (Some(p.build), format!("b{}", p.build)),
        None => (None, String::from("v1")),
    };
    let proof_state = crate::source::SourceProvenance::Verified(source_proof);
    let revision = BounceRevision {
        id: rev_id.clone(),
        build,
        label,
        original_name: original_name.clone(),
        path: path.clone(),
        source_proof: proof_state.clone(),
        size,
        notes: SharedVec::default(),
        feedback_note: None,
    };
    library.tracks.push(Track {
        id: id.clone(),
        title,
        original_name,
        path,
        source_proof: proof_state,
        reference_path: None,
        size,
        favorite: false,
        stage: TrackStage::Backlog,
        notes: SharedVec::default(),
        group_key: parsed.map(|p| p.group_key),
        next_action: String::new(),
        revisions: vec![revision].into(),
        active_revision_id: Some(rev_id.clone()),
        current_revision_id: Some(rev_id.clone()),
        collisions: SharedVec::default(),
        external: false,
    });
    BounceImportOutcome::CreatedTrack {
        track_id: id,
        revision_id: rev_id,
    }
}

fn attach_bounce_to_existing_track(
    library: &mut Library,
    track_id: &str,
    path: PathBuf,
    size: u64,
    source_proof: crate::source::AudioSourceProof,
    initial_notes: Option<SharedVec<Note>>,
) -> Result<BounceImportOutcome, String> {
    let parsed = parse_bounce_filename(&path);
    let original_name = file_name_string(&path);
    let track = library
        .tracks
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    ensure_track_revisions_initialized(track);

    // Sync any direct legacy mutations on `track.notes` into the active revision first.
    if let Some(active_id) = track.active_revision_id.clone()
        && let Some(active_rev) = track.revisions.iter_mut().find(|rev| rev.id == active_id)
    {
        active_rev.notes = track.notes.clone();
    }

    if let Some(ref p) = parsed {
        if track.group_key.is_none() {
            track.group_key = Some(p.group_key.clone());
        }
        if let Some(ref parsed_title) = p.title
            && track.group_key.as_deref() == Some(track.title.as_str())
        {
            track.title = parsed_title.clone();
        }
        if let Some(existing) = track.revisions.iter().find(|rev| rev.build == Some(p.build)) {
            let same_source = match existing.source_proof.verified_proof() {
                Some(existing_proof) => existing_proof == &source_proof,
                None => existing.path == path && existing.size == size,
            };
            if same_source {
                return Ok(BounceImportOutcome::DuplicateIgnored {
                    track_id: track.id.clone(),
                    revision_id: existing.id.clone(),
                });
            }
            let proof_state = crate::source::SourceProvenance::Verified(source_proof);
            if let Some(existing_col) = track.collisions.iter().find(|col| {
                col.build == p.build
                    && (col.source_proof == proof_state || col.path == path)
            }) {
                return Ok(BounceImportOutcome::CollisionFlagged {
                    track_id: track.id.clone(),
                    collision_id: existing_col.id.clone(),
                    build: p.build,
                });
            }
            let collision_id = allocate_collision_id(&track.collisions);
            track.collisions.push(BounceCollision {
                id: collision_id.clone(),
                build: p.build,
                label: format!("b{}", p.build),
                original_name,
                path,
                source_proof: proof_state,
                size,
            });
            return Ok(BounceImportOutcome::CollisionFlagged {
                track_id: track.id.clone(),
                collision_id,
                build: p.build,
            });
        }
    } else if let Some(existing) = track.revisions.iter().find(|rev| {
        rev.source_proof
            .verified_proof()
            .is_some_and(|proof| proof == &source_proof)
            && rev.path == path
    }) {
        return Ok(BounceImportOutcome::DuplicateIgnored {
            track_id: track.id.clone(),
            revision_id: existing.id.clone(),
        });
    }

    let prev_current = track.current_revision_id.clone();
    let rev_id = allocate_revision_id(&track.revisions);
    let (build, label) = match &parsed {
        Some(p) => (Some(p.build), format!("b{}", p.build)),
        None => (None, next_unnumbered_label(&track.revisions)),
    };
    let revision = BounceRevision {
        id: rev_id.clone(),
        build,
        label,
        original_name,
        path,
        source_proof: crate::source::SourceProvenance::Verified(source_proof),
        size,
        notes: initial_notes.unwrap_or_default(),
        feedback_note: None,
    };
    if build.is_some() {
        track.revisions.push(revision);
        sort_and_select_current_revision(track);
        let promoted_current = track.current_revision_id.as_deref() == Some(rev_id.as_str())
            && prev_current.as_deref() != Some(rev_id.as_str());
        if promoted_current || track.active_revision_id.is_none() {
            track.active_revision_id = Some(rev_id.clone());
        }
        sync_track_from_active_revision(track);
        Ok(BounceImportOutcome::AddedRevision {
            track_id: track.id.clone(),
            revision_id: rev_id,
            promoted_current,
        })
    } else {
        // Manually assigned unnumbered bounce: append and select it for immediate review.
        // If no numbered builds exist, make it current too; otherwise highest numbered build stays current.
        let has_numbered = track.revisions.iter().any(|rev| rev.build.is_some());
        if has_numbered {
            track.revisions.push(revision);
        } else {
            track.revisions.insert(0, revision);
            track.current_revision_id = Some(rev_id.clone());
        }
        let promoted_current = !has_numbered;
        track.active_revision_id = Some(rev_id.clone());
        sync_track_from_active_revision(track);
        Ok(BounceImportOutcome::AddedRevision {
            track_id: track.id.clone(),
            revision_id: rev_id,
            promoted_current,
        })
    }
}

/// Switch the active bounce revision loaded in Review for one track.
pub fn select_track_revision(
    library: &mut Library,
    track_id: &str,
    revision_id: &str,
) -> Result<bool, String> {
    let track = library
        .tracks
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    ensure_track_revisions_initialized(track);
    if !track.revisions.iter().any(|rev| rev.id == revision_id) {
        return Err(String::from("That bounce revision is no longer on the track."));
    }
    // Preserve any active notes before switching.
    if let Some(active_id) = track.active_revision_id.clone()
        && let Some(active_rev) = track.revisions.iter_mut().find(|rev| rev.id == active_id)
    {
        active_rev.notes = track.notes.clone();
    }
    if track.active_revision_id.as_deref() == Some(revision_id) {
        sync_track_from_active_revision(track);
        return Ok(false);
    }
    track.active_revision_id = Some(revision_id.to_owned());
    sync_track_from_active_revision(track);
    Ok(true)
}

/// Manually assign an existing standalone track into another track as a bounce revision.
pub fn assign_standalone_track_as_revision(
    library: &mut Library,
    source_track_id: &str,
    target_track_id: &str,
) -> Result<BounceImportOutcome, String> {
    if source_track_id == target_track_id {
        return Err(String::from("Choose a different track to assign this bounce to."));
    }
    let source = library
        .tracks
        .iter()
        .find(|track| track.id == source_track_id)
        .cloned()
        .ok_or_else(|| String::from("That source track is no longer in the library."))?;
    if !library.tracks.iter().any(|track| track.id == target_track_id) {
        return Err(String::from("That target track is no longer in the library."));
    }
    let mut staged = library.clone();
    let proof = source.source_proof.verified_proof().cloned();
    let outcome = if let Some(proof) = proof {
        attach_bounce_to_existing_track(
            &mut staged,
            target_track_id,
            source.path.clone(),
            source.size,
            proof,
            Some(source.notes.clone()),
        )?
    } else {
        // Legacy track without verified proof: attach directly as an unnumbered revision.
        let target = staged
            .tracks
            .find_mut(|track| track.id == target_track_id)
            .ok_or_else(|| String::from("That target track is no longer in the library."))?;
        ensure_track_revisions_initialized(target);
        let rev_id = allocate_revision_id(&target.revisions);
        let label = next_unnumbered_label(&target.revisions);
        let has_numbered = target.revisions.iter().any(|rev| rev.build.is_some());
        let rev = BounceRevision {
            id: rev_id.clone(),
            build: None,
            label,
            original_name: source.original_name.clone(),
            path: source.path.clone(),
            source_proof: source.source_proof.clone(),
            size: source.size,
            notes: source.notes.clone(),
            feedback_note: None,
        };
        if has_numbered {
            target.revisions.push(rev);
        } else {
            target.revisions.insert(0, rev);
            target.current_revision_id = Some(rev_id.clone());
        }
        target.active_revision_id = Some(rev_id.clone());
        sync_track_from_active_revision(target);
        BounceImportOutcome::AddedRevision {
            track_id: target.id.clone(),
            revision_id: rev_id,
            promoted_current: !has_numbered,
        }
    };
    let _ = remove_track(&mut staged, source_track_id)?;
    staged.selected_track_id = Some(target_track_id.to_owned());
    normalize_planner_order(&mut staged);
    validate_library_identity(&staged)?;
    *library = staged;
    Ok(outcome)
}

/// Dismiss a flagged build collision while keeping the existing build untouched.
pub fn resolve_collision_keep_existing(
    library: &mut Library,
    track_id: &str,
    collision_id: &str,
) -> Result<(), String> {
    let track = library
        .tracks
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    let before = track.collisions.len();
    track.collisions.retain(|col| col.id != collision_id);
    if track.collisions.len() == before {
        return Err(String::from("That collision is no longer pending."));
    }
    Ok(())
}

/// Intentionally replace the colliding build revision with the flagged file,
/// preserving track stage, next action, and other builds' notes.
pub fn resolve_collision_replace_build(
    library: &mut Library,
    track_id: &str,
    collision_id: &str,
) -> Result<(), String> {
    let track = library
        .tracks
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    let col_idx = track
        .collisions
        .iter()
        .position(|col| col.id == collision_id)
        .ok_or_else(|| String::from("That collision is no longer pending."))?;
    let collision = track.collisions.remove(col_idx);
    ensure_track_revisions_initialized(track);
    let rev = track
        .revisions
        .iter_mut()
        .find(|rev| rev.build == Some(collision.build))
        .ok_or_else(|| String::from("The conflicting build is no longer on this track."))?;
    rev.path = collision.path;
    rev.original_name = collision.original_name;
    rev.size = collision.size;
    rev.source_proof = collision.source_proof;
    rev.notes.clear();
    sync_track_from_active_revision(track);
    Ok(())
}

/// Import a flagged collision as its own standalone track without modifying the
/// existing build on the original track.
pub fn resolve_collision_import_as_new_track(
    library: &mut Library,
    track_id: &str,
    collision_id: &str,
) -> Result<String, String> {
    let collision = {
        let track = library
            .tracks
            .find_mut(|track| track.id == track_id)
            .ok_or_else(|| String::from("That track is no longer in the library."))?;
        let col_idx = track
            .collisions
            .iter()
            .position(|col| col.id == collision_id)
            .ok_or_else(|| String::from("That collision is no longer pending."))?;
        track.collisions.remove(col_idx)
    };
    let new_id = allocate_track_id(library);
    let rev_id = allocate_revision_id(&[]);
    let title = fallback_track_title(&collision.path);
    let revision = BounceRevision {
        id: rev_id.clone(),
        build: Some(collision.build),
        label: collision.label,
        original_name: collision.original_name.clone(),
        path: collision.path.clone(),
        source_proof: collision.source_proof.clone(),
        size: collision.size,
        notes: SharedVec::default(),
        feedback_note: None,
    };
    library.tracks.push(Track {
        id: new_id.clone(),
        title,
        original_name: collision.original_name,
        path: collision.path,
        source_proof: collision.source_proof,
        reference_path: None,
        size: collision.size,
        favorite: false,
        stage: TrackStage::Backlog,
        notes: SharedVec::default(),
        group_key: None,
        next_action: String::new(),
        revisions: vec![revision].into(),
        active_revision_id: Some(rev_id.clone()),
        current_revision_id: Some(rev_id),
        collisions: SharedVec::default(),
        external: false,
    });
    normalize_planner_order(library);
    library.selected_track_id = Some(new_id.clone());
    Ok(new_id)
}

/// Edit a track's readable title without changing its `group_key` or splitting builds.
pub fn set_track_title(library: &mut Library, track_id: &str, title: &str) -> Result<bool, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err(String::from("Track title cannot be empty."));
    }
    let track = library
        .tracks
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    if track.title == title {
        return Ok(false);
    }
    track.title = title.to_owned();
    Ok(true)
}

/// Set or clear a track's single Planner next action. Persists across new builds and relaunch.
pub fn set_track_next_action(
    library: &mut Library,
    track_id: &str,
    next_action: &str,
) -> Result<bool, String> {
    let next_action = next_action.trim();
    let track = library
        .tracks
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    if track.next_action == next_action {
        return Ok(false);
    }
    track.next_action = next_action.to_owned();
    Ok(true)
}

/// Record or clear a short per-build feedback note (e.g. "Heard by Dylan") on one revision.
pub fn set_revision_feedback_note(
    library: &mut Library,
    track_id: &str,
    revision_id: Option<&str>,
    note: &str,
) -> Result<bool, String> {
    let note = note.trim();
    let value = if note.is_empty() {
        None
    } else {
        Some(note.to_owned())
    };
    let track = library
        .tracks
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    ensure_track_revisions_initialized(track);
    let target_id = revision_id
        .map(str::to_owned)
        .or_else(|| track.active_revision_id.clone())
        .or_else(|| track.current_revision_id.clone())
        .ok_or_else(|| String::from("That bounce revision is no longer on the track."))?;
    let rev = track
        .revisions
        .iter_mut()
        .find(|rev| rev.id == target_id)
        .ok_or_else(|| String::from("That bounce revision is no longer on the track."))?;
    if rev.feedback_note == value {
        return Ok(false);
    }
    rev.feedback_note = value;
    Ok(true)
}

/// Set or clear the user's remembered reviewer name used when exporting feedback packages.
pub fn set_reviewer_name(library: &mut Library, name: &str) -> Result<bool, String> {
    let trimmed = name.trim();
    let next = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    };
    if library.reviewer_name == next {
        return Ok(false);
    }
    library.reviewer_name = next;
    Ok(true)
}

/// Toggle whether a track is an external track imported for giving feedback
/// (kept out of personal Planner columns by default).
pub fn set_track_external(
    library: &mut Library,
    track_id: &str,
    external: bool,
) -> Result<bool, String> {
    let track = library
        .tracks
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    if track.external == external {
        return Ok(false);
    }
    track.external = external;
    Ok(true)
}

/// Import a bounce from someone else marked as `external = true` for giving feedback.
pub fn import_external_bounce_into_library(
    library: Library,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Persisted<(Library, BounceImportOutcome)>, String> {
    import_external_bounce_into_library_at(library, decoded, &library_path())
}

fn import_external_bounce_into_library_at(
    mut library: Library,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Persisted<(Library, BounceImportOutcome)>, String> {
    let path = decoded.path().to_path_buf();
    validate_audio_path(&path)?;
    ensure_decoded_audio_unchanged(&decoded)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    ensure_decoded_audio_unchanged(&decoded)?;

    let mut occupied = occupied_track_ids(&library);
    let outcome = apply_bounce_import_to_library(
        &mut library,
        path,
        metadata.len(),
        decoded.source_proof().clone(),
        &mut occupied,
    );
    if let BounceImportOutcome::CreatedTrack { ref track_id, .. } = outcome
        && let Some(track) = library.tracks.find_mut(|t| &t.id == track_id)
    {
        track.external = true;
    }
    normalize_planner_order(&mut library);
    library.selected_track_id = Some(outcome.track_id().to_owned());
    ensure_decoded_audio_unchanged(&decoded)?;
    let persistence = persist_library_at(&library, library_path)?;
    Ok(Persisted::new((library, outcome), persistence))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedbackImportResult {
    MergedIntoExisting {
        track_id: String,
        revision_id: String,
        reviewer: String,
        added_notes: usize,
        skipped_duplicates: usize,
    },
    ImportedWithAudio {
        track_id: String,
        revision_id: String,
        reviewer: String,
        added_notes: usize,
        extracted_audio_path: PathBuf,
    },
}

impl FeedbackImportResult {
    pub fn track_id(&self) -> &str {
        match self {
            Self::MergedIntoExisting { track_id, .. }
            | Self::ImportedWithAudio { track_id, .. } => track_id,
        }
    }
}

/// Export one track revision together with its audio file and timestamped comments
/// as a self-contained `.cadence-feedback` package.
pub fn export_track_feedback_package(
    library: &Library,
    track_id: &str,
    revision_id: Option<&str>,
    reviewer: &str,
    summary: Option<&str>,
    destination: &Path,
) -> Result<PathBuf, String> {
    let track = library
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;

    let (source_path, original_name, expected_proof, build, build_label, notes, rev_feedback) =
        if let Some(rev) = revision_id
            .and_then(|rid| track.revisions.iter().find(|r| r.id == rid))
            .or_else(|| track.active_revision())
        {
            (
                rev.path.as_path(),
                rev.original_name.as_str(),
                rev.source_proof.verified_proof(),
                rev.build,
                rev.label.clone(),
                rev.notes.as_slice(),
                rev.feedback_note.as_deref(),
            )
        } else {
            let parsed = parse_bounce_filename(Path::new(&track.original_name))
                .or_else(|| parse_bounce_filename(&track.path));
            (
                track.path.as_path(),
                track.original_name.as_str(),
                track.source_proof.verified_proof(),
                parsed.as_ref().map(|p| p.build),
                track.active_build_label(),
                track.notes.as_slice(),
                None,
            )
        };

    let shared_notes = notes
        .iter()
        .map(|n| crate::feedback_package::SharedNote {
            id: n.id.clone(),
            time_millis: n.time_millis,
            body: n.body.clone(),
            done: n.done,
            author: n.author.clone(),
        })
        .collect::<Vec<_>>();
    let effective_summary = summary
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(rev_feedback);

    crate::feedback_package::export_feedback_package(
        crate::feedback_package::ExportPackageRequest {
            reviewer,
            summary: effective_summary,
            track_title: &track.title,
            group_key: track.effective_group_key().as_deref(),
            build,
            build_label: &build_label,
            original_name,
            source_path,
            expected_proof,
            notes: shared_notes,
        },
        destination,
    )
}

/// Import a self-contained `.cadence-feedback` package into the library.
///
/// - If any track/revision already owns the exact audio SHA-256 + byte length,
///   merges the reviewer's attributed notes onto that exact revision in place
///   without duplicating the track or copying the audio.
/// - Otherwise verifies and extracts the bundled audio into local `shared-audio/`
///   storage, imports the track (marked external unless grouped onto an existing
///   track), and attaches all attributed notes.
pub fn import_feedback_package(
    library: Library,
    package_path: &Path,
) -> Result<Persisted<(Library, FeedbackImportResult)>, String> {
    import_feedback_package_at(
        library,
        package_path,
        &library_path(),
        &shared_audio_directory(),
    )
}

fn import_feedback_package_at(
    mut library: Library,
    package_path: &Path,
    library_path: &Path,
    shared_audio_dir: &Path,
) -> Result<Persisted<(Library, FeedbackImportResult)>, String> {
    let inspected = crate::feedback_package::inspect_feedback_package(package_path)?;
    let manifest = &inspected.manifest;

    // 1. Look for an existing track + revision with matching verified AudioSourceProof.
    let matched = find_revision_by_source_proof(&library, &manifest.source_proof);
    if let Some((track_id, matched_rev_id)) = matched {
        // Verify bundled audio integrity before trusting any manifest comments.
        crate::feedback_package::verify_bundled_audio(package_path, &inspected)?;
        let (revision_id, added_notes, skipped_duplicates) = merge_manifest_into_track(
            &mut library,
            &track_id,
            matched_rev_id.as_deref(),
            manifest,
        )?;
        library.selected_track_id = Some(track_id.clone());
        let outcome = persist_library_at(&library, library_path)?;
        return Ok(Persisted::new(
            (
                library,
                FeedbackImportResult::MergedIntoExisting {
                    track_id,
                    revision_id,
                    reviewer: manifest.reviewer.clone(),
                    added_notes,
                    skipped_duplicates,
                },
            ),
            outcome,
        ));
    }

    // 2. Audio not present locally: verify & extract bundled audio, decode to confirm
    // playability, import into library, and attach attributed notes.
    let extracted_path =
        crate::feedback_package::extract_verified_audio(package_path, &inspected, shared_audio_dir)?;
    let decoded = crate::audio::decode_audio_file(&extracted_path)
        .map_err(|err| format!("Bundled audio could not be decoded: {err}"))?;
    if decoded.source_proof() != &manifest.source_proof {
        return Err(String::from(
            "Extracted audio proof does not match the feedback package manifest.",
        ));
    }
    let metadata = fs::metadata(&extracted_path)
        .map_err(|err| format!("Could not inspect extracted audio: {err}"))?;

    let mut occupied = occupied_track_ids(&library);
    let import_outcome = apply_bounce_import_to_library(
        &mut library,
        extracted_path.clone(),
        metadata.len(),
        decoded.source_proof().clone(),
        &mut occupied,
    );
    let track_id = import_outcome.track_id().to_owned();
    let created_new = matches!(import_outcome, BounceImportOutcome::CreatedTrack { .. });
    if created_new
        && let Some(track) = library.tracks.find_mut(|t| t.id == track_id)
    {
        if !manifest.track_title.trim().is_empty() {
            track.title = manifest.track_title.trim().to_owned();
        }
        track.external = true;
    }

    let target_rev_id = match &import_outcome {
        BounceImportOutcome::CreatedTrack { revision_id, .. }
        | BounceImportOutcome::AddedRevision { revision_id, .. }
        | BounceImportOutcome::DuplicateIgnored { revision_id, .. } => Some(revision_id.clone()),
        BounceImportOutcome::CollisionFlagged { .. } => None,
    };
    let (revision_id, added_notes, _) = merge_manifest_into_track(
        &mut library,
        &track_id,
        target_rev_id.as_deref(),
        manifest,
    )?;

    normalize_planner_order(&mut library);
    library.selected_track_id = Some(track_id.clone());
    let persistence = persist_library_at(&library, library_path)?;
    Ok(Persisted::new(
        (
            library,
            FeedbackImportResult::ImportedWithAudio {
                track_id,
                revision_id,
                reviewer: manifest.reviewer.clone(),
                added_notes,
                extracted_audio_path: extracted_path,
            },
        ),
        persistence,
    ))
}

fn find_revision_by_source_proof(
    library: &Library,
    proof: &crate::source::AudioSourceProof,
) -> Option<(String, Option<String>)> {
    for track in &library.tracks {
        if let Some(rev) = track
            .revisions
            .iter()
            .find(|r| r.source_proof.verified_proof() == Some(proof))
        {
            return Some((track.id.clone(), Some(rev.id.clone())));
        }
        if track.source_proof.verified_proof() == Some(proof) {
            return Some((track.id.clone(), track.active_revision_id.clone()));
        }
    }
    None
}

fn merge_manifest_into_track(
    library: &mut Library,
    track_id: &str,
    revision_id: Option<&str>,
    manifest: &crate::feedback_package::FeedbackManifest,
) -> Result<(String, usize, usize), String> {
    let track = library
        .tracks
        .find_mut(|t| t.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    ensure_track_revisions_initialized(track);

    let target_id = revision_id
        .map(str::to_owned)
        .or_else(|| track.active_revision_id.clone())
        .or_else(|| track.current_revision_id.clone())
        .ok_or_else(|| String::from("Could not locate target revision for feedback merge."))?;

    let rev = track
        .revisions
        .iter_mut()
        .find(|r| r.id == target_id)
        .ok_or_else(|| String::from("Target revision is no longer on the track."))?;

    let mut added = 0_usize;
    let mut skipped = 0_usize;
    for shared in &manifest.notes {
        let dedupe_key = format!("{}:{}", manifest.package_id, shared.id);
        if rev
            .notes
            .iter()
            .any(|existing| existing.package_id.as_deref() == Some(dedupe_key.as_str()))
        {
            skipped += 1;
            continue;
        }
        let body = shared.body.trim();
        if body.is_empty() {
            continue;
        }
        let note_id = allocate_note_id(&rev.notes);
        let author = shared
            .author
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .unwrap_or(manifest.reviewer.trim())
            .to_owned();
        rev.notes.push(Note {
            id: note_id,
            time_millis: shared.time_millis,
            body: body.to_owned(),
            done: shared.done,
            author: Some(author),
            package_id: Some(dedupe_key),
        });
        added += 1;
    }
    rev.notes.as_mut_vec().sort_by_key(|n| n.time_millis);

    if let Some(summary) = manifest
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let entry = format!("{}: {summary}", manifest.reviewer.trim());
        match &mut rev.feedback_note {
            Some(existing) if existing.contains(&entry) => {}
            Some(existing) if !existing.trim().is_empty() => {
                existing.push_str(" · ");
                existing.push_str(&entry);
            }
            slot => *slot = Some(entry),
        }
    }

    track.active_revision_id = Some(target_id.clone());
    sync_track_from_active_revision(track);
    Ok((target_id, added, skipped))
}

/// Replace the source file for one existing track while preserving its stable
/// identity, favorite state, and workflow stage. A replacement is a new audio
/// version, so timestamped comments are intentionally cleared.
pub fn replace_track(
    library: Library,
    track_id: &str,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Persisted<Library>, String> {
    replace_track_at(library, track_id, decoded, &library_path())
}

fn replace_track_at(
    mut library: Library,
    track_id: &str,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Persisted<Library>, String> {
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
    library.selected_track_id = Some(track_id.to_owned());
    ensure_decoded_audio_unchanged(&decoded)?;
    let outcome = persist_library_at(&library, library_path)?;
    Ok(Persisted::new(library, outcome))
}

/// Associate a second audio file with one track. The reference is kept as an
/// external path, just like the primary source, so importing it never copies or
/// mutates the user's audio file.
pub fn set_reference_track(
    library: Library,
    track_id: &str,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Persisted<Library>, String> {
    set_reference_track_at(library, track_id, decoded, &library_path())
}

fn set_reference_track_at(
    mut library: Library,
    track_id: &str,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Persisted<Library>, String> {
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
    let outcome = persist_library_at(&library, library_path)?;
    Ok(Persisted::new(library, outcome))
}

/// Add an audio file to the global reference catalog without assigning it to
/// any main track. The catalog stores the external path only; importing a
/// reference never copies or mutates the user's audio file.
#[allow(dead_code)]
pub fn add_reference_track(
    library: Library,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Persisted<Library>, String> {
    add_reference_track_at(library, decoded, &library_path())
}

/// Replace one existing reference-catalog entry in place through one durable
/// library replacement. The entry's notes and any future fields remain on the
/// existing record; only its source path and proof change. Main-track
/// assignments that pointed at the old path follow the replacement.
pub fn replace_reference_track(
    library: Library,
    original_path: &Path,
    expected_proof: Option<&crate::source::AudioSourceProof>,
    decoded: crate::audio::DecodedAudioFile,
) -> Result<Persisted<Library>, String> {
    replace_reference_track_at(
        library,
        original_path,
        expected_proof,
        decoded,
        &library_path(),
    )
}

fn replace_reference_track_at(
    mut library: Library,
    original_path: &Path,
    expected_proof: Option<&crate::source::AudioSourceProof>,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Persisted<Library>, String> {
    ensure_reference_track_matches_proof(&library, original_path, expected_proof)?;
    let path = decoded.path().to_path_buf();
    validate_audio_path(&path)?;
    ensure_decoded_audio_unchanged(&decoded)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    ensure_decoded_audio_unchanged(&decoded)?;

    replace_reference_track_metadata_with_proof(
        &mut library,
        original_path,
        path,
        decoded.source_proof().clone(),
    )?;
    ensure_decoded_audio_unchanged(&decoded)?;
    let outcome = persist_library_at(&library, library_path)?;
    Ok(Persisted::new(library, outcome))
}

/// Commit one logical main-track import batch through one durable library
/// replacement. Candidates are provisionally validated without changing the
/// staged library, then revalidated in original order immediately before they
/// are applied; a changed candidate is retained as a per-file error while
/// other candidates may still commit atomically.
pub fn import_verified_batch(
    library: Library,
    candidates: Vec<VerifiedImportCandidate>,
    library_path: &Path,
) -> BatchImportReport {
    let mut occupied_track_ids = occupied_track_ids(&library);
    let mut apply_candidate =
        |library: &mut Library, candidate: VerifiedImportCandidate, metadata: fs::Metadata| {
            apply_main_track_import(library, candidate, metadata, &mut occupied_track_ids)
        };
    let mut prepared =
        import_verified_batch_inner(library, candidates, |_| {}, &mut apply_candidate);
    if !prepared.accepted_paths.is_empty() {
        normalize_planner_order(&mut prepared.library);
    }
    persist_batch_library(
        prepared.library,
        prepared.accepted_paths,
        prepared.errors,
        library_path,
    )
}

#[cfg(test)]
fn import_verified_batch_with_final_fence_hook(
    library: Library,
    candidates: Vec<VerifiedImportCandidate>,
    library_path: &Path,
    hook: impl FnMut(&VerifiedImportCandidate),
) -> BatchImportReport {
    let mut occupied_track_ids = occupied_track_ids(&library);
    let mut apply_candidate =
        |library: &mut Library, candidate: VerifiedImportCandidate, metadata: fs::Metadata| {
            apply_main_track_import(library, candidate, metadata, &mut occupied_track_ids)
        };
    let mut prepared = import_verified_batch_inner(library, candidates, hook, &mut apply_candidate);
    if !prepared.accepted_paths.is_empty() {
        normalize_planner_order(&mut prepared.library);
    }
    persist_batch_library(
        prepared.library,
        prepared.accepted_paths,
        prepared.errors,
        library_path,
    )
}

struct PreparedImportBatch {
    library: Library,
    accepted_paths: Vec<PathBuf>,
    errors: Vec<BatchImportError>,
}

fn import_verified_batch_inner(
    mut library: Library,
    candidates: Vec<VerifiedImportCandidate>,
    mut before_final_validation: impl FnMut(&VerifiedImportCandidate),
    mut apply_candidate: impl FnMut(
        &mut Library,
        VerifiedImportCandidate,
        fs::Metadata,
    ) -> Result<(), String>,
) -> PreparedImportBatch {
    let mut provisional = Vec::with_capacity(candidates.len());
    let mut errors = Vec::new();
    for candidate in candidates {
        let path = candidate.path.clone();
        match validate_import_candidate(&candidate) {
            Ok(_) => provisional.push(candidate),
            Err(error) => errors.push(BatchImportError { path, error }),
        }
    }

    let mut final_successes = Vec::with_capacity(provisional.len());
    for candidate in provisional {
        before_final_validation(&candidate);
        let path = candidate.path.clone();
        match validate_import_candidate(&candidate) {
            Ok(_) => final_successes.push(candidate),
            Err(error) => {
                errors.push(BatchImportError { path, error });
            }
        }
    }

    let mut accepted_paths = Vec::with_capacity(final_successes.len());
    for candidate in final_successes {
        let path = candidate.path.clone();
        let metadata = match validate_import_candidate(&candidate) {
            Ok(metadata) => metadata,
            Err(error) => {
                errors.push(BatchImportError { path, error });
                continue;
            }
        };
        match apply_candidate(&mut library, candidate, metadata) {
            Ok(()) => accepted_paths.push(path),
            Err(error) => errors.push(BatchImportError { path, error }),
        }
    }

    PreparedImportBatch {
        library,
        accepted_paths,
        errors,
    }
}

fn apply_main_track_import(
    library: &mut Library,
    candidate: VerifiedImportCandidate,
    metadata: fs::Metadata,
    occupied_track_ids: &mut HashSet<String>,
) -> Result<(), String> {
    let outcome = apply_bounce_import_to_library(
        library,
        candidate.path,
        metadata.len(),
        candidate.source_proof,
        occupied_track_ids,
    );
    library.selected_track_id = Some(outcome.track_id().to_owned());
    Ok(())
}

fn occupied_track_ids(library: &Library) -> HashSet<String> {
    library
        .tracks
        .iter()
        .map(|track| track.id.clone())
        .collect()
}

fn allocate_track_id_from_occupied(occupied_track_ids: &mut HashSet<String>) -> String {
    let id = allocate_id("track", unique_id(), |candidate| {
        occupied_track_ids.contains(candidate)
    });
    occupied_track_ids.insert(id.clone());
    id
}

fn apply_reference_import(
    library: &mut Library,
    candidate: VerifiedImportCandidate,
    _metadata: fs::Metadata,
) -> Result<(), String> {
    ensure_reference_track_with_proof(library, candidate.path, candidate.source_proof)
}

/// Commit one assigned-reference import batch through one durable library
/// replacement. The first accepted path becomes the assigned selection,
/// matching the historical multi-file interaction without a second save.
pub fn assign_reference_verified_batch(
    library: Library,
    track_id: &str,
    candidates: Vec<VerifiedImportCandidate>,
    library_path: &Path,
) -> BatchImportReport {
    if !library.tracks.iter().any(|track| track.id == track_id) {
        return BatchImportReport::failed(
            PathBuf::from(track_id),
            String::from("That track is no longer in the library."),
        );
    }

    let mut prepared =
        import_verified_batch_inner(library, candidates, |_| {}, apply_reference_import);

    if let Some(first_path) = prepared.accepted_paths.first().cloned()
        && let Err(error) =
            set_reference_track_selection(&mut prepared.library, track_id, first_path)
    {
        prepared.errors.push(BatchImportError {
            path: PathBuf::from(track_id),
            error,
        });
        prepared.accepted_paths.clear();
    }

    persist_batch_library(
        prepared.library,
        prepared.accepted_paths,
        prepared.errors,
        library_path,
    )
}

/// Commit one reference-catalog import batch through one durable library
/// replacement. The catalog keeps the candidate order and rejects duplicate
/// proof conflicts without persisting an all-failed batch.
pub fn add_reference_verified_batch(
    library: Library,
    candidates: Vec<VerifiedImportCandidate>,
    library_path: &Path,
) -> BatchImportReport {
    let prepared = import_verified_batch_inner(library, candidates, |_| {}, apply_reference_import);
    persist_batch_library(
        prepared.library,
        prepared.accepted_paths,
        prepared.errors,
        library_path,
    )
}

fn validate_import_candidate(candidate: &VerifiedImportCandidate) -> Result<fs::Metadata, String> {
    validate_audio_path(&candidate.path)?;
    candidate.source_proof.validate().map_err(|error| {
        format!(
            "Invalid source proof for {}: {error}",
            candidate.path.display()
        )
    })?;
    crate::source::validate_path_stamp(&candidate.path, candidate.source_stamp, || false)
        .map_err(|error| format!("Audio source changed after preflight: {error}"))?;
    let metadata = fs::metadata(&candidate.path)
        .map_err(|error| format!("Could not inspect {}: {error}", candidate.path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", candidate.path.display()));
    }
    if metadata.len() != candidate.source_stamp.len {
        return Err(format!(
            "Audio source length changed after preflight: {}",
            candidate.path.display()
        ));
    }
    Ok(metadata)
}

fn persist_batch_library(
    library: Library,
    accepted_paths: Vec<PathBuf>,
    errors: Vec<BatchImportError>,
    library_path: &Path,
) -> BatchImportReport {
    if accepted_paths.is_empty() {
        return BatchImportReport {
            library: None,
            imported_paths: Vec::new(),
            errors,
            persistence_outcome: None,
        };
    }
    let persistence_outcome = match persist_library_at(&library, library_path) {
        Ok(outcome) => outcome,
        Err(error) => {
            let mut errors = errors;
            errors.push(BatchImportError {
                path: library_path.to_path_buf(),
                error: format!("Atomic import batch was not saved: {error}"),
            });
            return BatchImportReport {
                library: None,
                imported_paths: Vec::new(),
                errors,
                persistence_outcome: None,
            };
        }
    };
    BatchImportReport {
        library: Some(library),
        imported_paths: accepted_paths,
        errors,
        persistence_outcome: Some(persistence_outcome),
    }
}

#[allow(dead_code)]
fn add_reference_track_at(
    mut library: Library,
    decoded: crate::audio::DecodedAudioFile,
    library_path: &Path,
) -> Result<Persisted<Library>, String> {
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
    let outcome = persist_library_at(&library, library_path)?;
    Ok(Persisted::new(library, outcome))
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

fn replace_reference_track_metadata_with_proof(
    library: &mut Library,
    original_path: &Path,
    replacement_path: PathBuf,
    source_proof: crate::source::AudioSourceProof,
) -> Result<(), String> {
    let reference_index = library
        .reference_tracks
        .iter()
        .position(|reference| reference.path == original_path)
        .ok_or_else(|| String::from("That reference track is no longer in the catalog."))?;
    if replacement_path != original_path
        && library
            .reference_tracks
            .iter()
            .enumerate()
            .any(|(index, reference)| {
                index != reference_index && reference.path == replacement_path
            })
    {
        return Err(format!(
            "Reference path {} is already owned by another catalog entry; choose a different file.",
            replacement_path.display()
        ));
    }

    let reference = library
        .reference_tracks
        .get_mut(reference_index)
        .expect("the reference index was found before mutation");
    reference.path = replacement_path.clone();
    reference.source_proof = crate::source::SourceProvenance::Verified(source_proof);

    for track in &mut library.tracks {
        if track.reference_path.as_deref() == Some(original_path) {
            track.reference_path = Some(replacement_path.clone());
        }
    }
    Ok(())
}

fn ensure_reference_track_matches_proof(
    library: &Library,
    original_path: &Path,
    expected_proof: Option<&crate::source::AudioSourceProof>,
) -> Result<(), String> {
    let reference = library
        .reference_tracks
        .iter()
        .find(|reference| reference.path == original_path)
        .ok_or_else(|| String::from("That reference track is no longer in the catalog."))?;
    if reference.source_provenance().verified_proof() != expected_proof {
        return Err(format!(
            "Reference catalog changed for {}; replacement was not applied.",
            original_path.display()
        ));
    }
    Ok(())
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
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    let changed = track.reference_path.as_ref() != Some(&path);
    track.reference_path = Some(path);
    Ok(changed)
}

/// Update a catalog entry's optional presentation name without changing its
/// path-based identity or any of its source/annotation metadata.
pub fn rename_reference_track(
    library: &mut Library,
    path: &Path,
    display_name: &str,
) -> Result<bool, String> {
    let display_name = display_name.trim();
    if display_name.is_empty() {
        return Err(String::from("Reference track name cannot be empty."));
    }

    let reference = library
        .reference_tracks
        .find_mut(|reference| reference.path == path)
        .ok_or_else(|| String::from("That reference track is no longer in the catalog."))?;
    if reference.display_name.as_deref() == Some(display_name) {
        return Ok(false);
    }
    reference.display_name = Some(display_name.to_owned());
    Ok(true)
}

fn ensure_reference_track(library: &mut Library, path: PathBuf) {
    if !library
        .reference_tracks
        .iter()
        .any(|reference| reference.path == path)
    {
        library.reference_tracks.push(ReferenceTrack {
            path,
            display_name: None,
            source_proof: crate::source::SourceProvenance::Unknown,
            notes: SharedVec::default(),
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
        .find_mut(|reference| reference.path == path)
    {
        match &reference.source_proof {
            crate::source::SourceProvenance::Verified(existing) if existing != &source_proof => {
                Err(format!(
                    "Reference source changed for {}. Please remove it from the reference catalog and re-import it.",
                    path.display()
                ))
            }
            crate::source::SourceProvenance::Verified(_) => Ok(()),
            // A legacy catalog entry owns historical notes without a proof.
            // Import/selection may use the decoded proof ephemerally, but only
            // explicit binding may promote this persisted owner.
            crate::source::SourceProvenance::Unknown => Ok(()),
        }
    } else {
        library.reference_tracks.push(ReferenceTrack {
            path,
            display_name: None,
            source_proof: crate::source::SourceProvenance::Verified(source_proof),
            notes: SharedVec::default(),
        });
        Ok(())
    }
}

/// Bind a legacy main owner to a freshly verified source without touching any
/// of its historical note fields.
pub fn bind_main_source_proof(
    library: &mut Library,
    track_id: &str,
    path: &Path,
    source_proof: crate::source::AudioSourceProof,
) -> Result<(), String> {
    source_proof.validate()?;
    let track = library
        .tracks
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    if track.path != path {
        return Err(String::from(
            "The main source path changed while binding comments.",
        ));
    }
    if track.source_provenance().verified_proof().is_some() {
        return Err(String::from("The main source is already bound to a proof."));
    }
    track.source_proof = crate::source::SourceProvenance::Verified(source_proof.clone());
    if let Some(active_id) = track.active_revision_id.clone()
        && let Some(rev) = track.revisions.iter_mut().find(|rev| rev.id == active_id)
    {
        rev.source_proof = crate::source::SourceProvenance::Verified(source_proof);
    }
    Ok(())
}

/// Bind a legacy reference owner to a freshly verified source without
/// touching any of its historical note fields.
pub fn bind_reference_source_proof(
    library: &mut Library,
    path: &Path,
    source_proof: crate::source::AudioSourceProof,
) -> Result<(), String> {
    source_proof.validate()?;
    let reference = library
        .reference_tracks
        .find_mut(|reference| reference.path == path)
        .ok_or_else(|| String::from("That reference is no longer in the catalog."))?;
    if reference.source_provenance().verified_proof().is_some() {
        return Err(String::from(
            "The reference source is already bound to a proof.",
        ));
    }
    reference.source_proof = crate::source::SourceProvenance::Verified(source_proof);
    Ok(())
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
        .find_mut(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    track.title = if title.trim().is_empty() {
        String::from("Untitled track")
    } else {
        title
    };
    track.original_name = original_name.clone();
    track.path = path.clone();
    track.source_proof = crate::source::SourceProvenance::from_optional(source_proof.clone());
    track.size = size;
    track.notes.clear();
    if let Some(active_id) = track.active_revision_id.clone()
        && let Some(rev) = track.revisions.iter_mut().find(|rev| rev.id == active_id)
    {
        rev.original_name = original_name;
        rev.path = path;
        rev.source_proof = crate::source::SourceProvenance::from_optional(source_proof);
        rev.size = size;
        rev.notes.clear();
    }
    Ok(())
}

pub fn remove_track(library: &mut Library, track_id: &str) -> Result<(usize, Track), String> {
    let index = library
        .tracks
        .iter()
        .position(|track| track.id == track_id)
        .ok_or_else(|| String::from("That track is no longer in the library."))?;
    let removed = library.tracks.remove(index);
    library.tags.assignments.remove(track_id);
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

    let affected_track_indices = library
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| track.reference_path.as_deref() == Some(path))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut cleared_assignments = 0;
    for index in affected_track_indices {
        let track = library
            .tracks
            .get_mut(index)
            .expect("affected track index remains valid");
        track.reference_path = None;
        cleared_assignments += 1;
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
        .find_mut(|track| track.id == track_id)
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
    library.planner_order = normalized_planner_order(library).into();
}

fn normalized_planner_order(library: &Library) -> Vec<String> {
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
        for id in &library.planner_order {
            if known_ids.contains(id.as_str()) && seen.insert(id.clone()) {
                order.push(id.clone());
            }
        }
    }

    for track in &library.tracks {
        if seen.insert(track.id.clone()) {
            order.push(track.id.clone());
        }
    }
    order
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

/// Move one track to a Planner insertion slot.
///
/// The operation stages only the normalized ID order and the source stage, so
/// invalid source, target, or slot data leaves the caller's library unchanged.
pub fn move_track_to_planner_slot(
    library: &mut Library,
    source_id: &str,
    target_stage: TrackStage,
    target_slot: usize,
) -> Result<bool, String> {
    let first_track_by_id = library.tracks.iter().fold(
        HashMap::<&str, &Track>::with_capacity(library.tracks.len()),
        |mut tracks, track| {
            tracks.entry(track.id.as_str()).or_insert(track);
            tracks
        },
    );
    let source = first_track_by_id
        .get(source_id)
        .copied()
        .ok_or_else(|| String::from("That track is no longer in the library."))?;

    let order_before = normalized_planner_order(library);
    let visible_target_ids = order_before
        .iter()
        .filter_map(|id| first_track_by_id.get(id.as_str()).copied())
        .filter(|track| track.stage == target_stage)
        .map(|track| track.id.as_str())
        .collect::<Vec<_>>();
    if target_slot > visible_target_ids.len() {
        return Err(String::from(
            "That Planner drop target is no longer available.",
        ));
    }
    let source_visible_index = if source.stage == target_stage {
        visible_target_ids.iter().position(|id| *id == source_id)
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
        .filter(|id| **id != source_id)
        .copied()
        .collect::<Vec<_>>();
    let adjusted_slot = source_visible_index
        .filter(|source_index| *source_index < target_slot)
        .map_or(target_slot, |_| target_slot - 1);
    if adjusted_slot > visible_target_ids_after_source.len() {
        return Err(String::from(
            "That Planner drop target is no longer available.",
        ));
    }

    let order_index = order_after_source.iter().enumerate().fold(
        HashMap::<&str, usize>::with_capacity(order_after_source.len()),
        |mut indexes, (index, id)| {
            indexes.entry(id.as_str()).or_insert(index);
            indexes
        },
    );
    let insertion_index =
        if let Some(anchor_id) = visible_target_ids_after_source.get(adjusted_slot) {
            *order_index
                .get(anchor_id)
                .ok_or_else(|| String::from("That Planner drop target is no longer available."))?
        } else if let Some(last_visible_id) = visible_target_ids_after_source.last() {
            order_index
                .get(last_visible_id)
                .map_or(order_after_source.len(), |index| index + 1)
        } else {
            order_after_source
                .iter()
                .position(|id| {
                    first_track_by_id
                        .get(id.as_str())
                        .is_some_and(|track| track.stage == target_stage)
                })
                .unwrap_or(order_after_source.len())
        };
    order_after_source.insert(insertion_index, source_id.to_string());

    let order_changed = order_after_source.as_slice() != library.planner_order.as_slice();
    let stage_changed = source.stage != target_stage;
    if order_changed || stage_changed {
        library.planner_order = order_after_source.into();
        if stage_changed && let Some(track) = library.tracks.find_mut(|track| track.id == source_id)
        {
            track.stage = target_stage;
        }
    }
    Ok(order_changed || stage_changed)
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

pub fn persist_library(library: &Library) -> Result<PersistenceOutcome, String> {
    let path = library_path();
    persist_library_at(library, &path)
}

fn persist_library_at(library: &Library, path: &Path) -> Result<PersistenceOutcome, String> {
    validate_library_identity(library)
        .map_err(|error| format!("Could not validate library before saving: {error}"))?;
    let encoded = serde_json::to_vec_pretty(library)
        .map_err(|error| format!("Could not encode library: {error}"))?;
    if encoded.len() > MAX_LIBRARY_BYTES {
        return Err(format!(
            "Could not save {}: the encoded library exceeds the maximum supported size of {MAX_LIBRARY_BYTES} bytes; reduce the library before saving",
            path.display()
        ));
    }
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
    if let Err(error) = sync_persist_parent_directory(directory) {
        return Ok(PersistenceOutcome::CommittedButDurabilityUncertain {
            detail: format!(
                "could not sync containing directory {} after replacing {}: {error}",
                directory.display(),
                path.display()
            ),
        });
    }

    Ok(PersistenceOutcome::Durable)
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

fn with_recovery_backup_cleanup(primary_error: String, backup_path: &Path) -> String {
    match fs::remove_file(backup_path) {
        Ok(()) => primary_error,
        Err(error) if error.kind() == ErrorKind::NotFound => primary_error,
        Err(cleanup_error) => format!(
            "{primary_error}; additionally, could not remove incomplete recovery backup {}: {cleanup_error}",
            backup_path.display()
        ),
    }
}

#[cfg(unix)]
fn sync_parent_directory(directory: &Path) -> std::io::Result<()> {
    let file = fs::File::open(directory)?;
    file.sync_all()
}

#[cfg(all(test, unix))]
pub(crate) fn fail_next_persist_parent_directory_sync_for_test() {
    FAIL_NEXT_PERSIST_PARENT_DIRECTORY_SYNC.with(|should_fail| should_fail.set(true));
}

#[cfg(unix)]
fn sync_persist_parent_directory(directory: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if FAIL_NEXT_PERSIST_PARENT_DIRECTORY_SYNC.with(Cell::get) {
        FAIL_NEXT_PERSIST_PARENT_DIRECTORY_SYNC.with(|should_fail| should_fail.set(false));
        return Err(std::io::Error::other(
            "injected post-rename parent-directory sync failure",
        ));
    }

    sync_parent_directory(directory)
}

pub fn library_path() -> PathBuf {
    app_data_directory().join("library.json")
}

pub fn waveform_cache_path(source: &Path, proof: &crate::source::AudioSourceProof) -> PathBuf {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in source.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // The proof type validates this value at every persistence boundary. Keep
    // the fallback deliberately fixed as an additional path-traversal guard
    // for callers constructing a malformed runtime value.
    let digest = if proof.validate().is_ok() {
        proof.sha256.as_str()
    } else {
        "invalid-proof"
    };
    app_data_directory()
        .join("waveform-cache-v2")
        .join(format!("{hash:016x}-{digest}.json"))
}

fn app_data_directory() -> PathBuf {
    // Isolate local review and automation from the user's active library.
    if let Some(directory) = env::var_os("CADENCE_DATA_DIR")
        && !directory.is_empty()
    {
        return PathBuf::from(directory);
    }

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

pub fn shared_audio_directory() -> PathBuf {
    app_data_directory().join("shared-audio")
}

pub(crate) fn validate_audio_path(path: &Path) -> Result<(), String> {
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
    use std::{
        io::{self, Read},
        sync::atomic::{AtomicU64, Ordering},
    };

    #[cfg(unix)]
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

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
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: Some(reference_path.clone()),
                size: 42,
                favorite: true,
                stage: TrackStage::Mixdown,
                notes: vec![Note {
                    id: String::from("note-1"),
                    time_millis: 900,
                    body: String::from("Compare the low-end tail."),
                    done: false,
                    ..Default::default()
                }]
                .into(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: vec![ReferenceTrack {
                path: reference_path,
                display_name: Some(String::from("Reference vocal")),
                source_proof: crate::source::SourceProvenance::Unknown,
                notes: vec![Note {
                    id: String::from("reference-note-1"),
                    time_millis: 1_100,
                    body: String::from("Check the reference vocal."),
                    done: true,
                    ..Default::default()
                }]
                .into(),
            }]
            .into(),
            planner_order: vec![String::from("track-1")].into(),
            tags: Default::default(),
            reviewer_name: None,
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
        decoded_audio_fixture_named(directory, "source.wav")
    }

    fn decoded_audio_fixture_named(
        directory: &Path,
        file_name: &str,
    ) -> (PathBuf, crate::audio::DecodedAudioFile) {
        let source = directory.join(file_name);
        fs::write(&source, tiny_pcm_wav()).expect("valid audio fixture should be writable");
        let decoded = crate::audio::decode_audio_file(&source)
            .expect("valid audio fixture should pass preflight");
        (source, decoded)
    }

    #[test]
    fn bounded_library_reader_reads_only_maximum_plus_one_bytes() {
        struct CountingReader {
            remaining: usize,
            bytes_read: usize,
        }

        impl Read for CountingReader {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let count = self.remaining.min(buffer.len());
                buffer[..count].fill(b'x');
                self.remaining -= count;
                self.bytes_read += count;
                Ok(count)
            }
        }

        let mut reader = CountingReader {
            remaining: MAX_LIBRARY_BYTES + 1_024,
            bytes_read: 0,
        };
        let bytes = read_library_bytes(&mut reader).expect("bounded reader should succeed");

        assert_eq!(bytes.len(), MAX_LIBRARY_BYTES + 1);
        assert_eq!(reader.bytes_read, MAX_LIBRARY_BYTES + 1);

        let mut normal_reader = CountingReader {
            remaining: 128,
            bytes_read: 0,
        };
        let normal_bytes =
            read_library_bytes(&mut normal_reader).expect("normal reader should succeed");
        assert_eq!(normal_bytes.len(), 128);
        assert!(normal_bytes.capacity() < MAX_LIBRARY_BYTES);
    }

    #[test]
    fn oversized_library_load_is_rejected_before_json_parse() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let oversized = vec![b'x'; MAX_LIBRARY_BYTES + 1];
        fs::write(&path, &oversized).expect("oversized library should be writable");

        let error = load_library_at(&path).expect_err("oversized library should fail to load");

        assert!(error.contains("maximum supported size"));
        assert!(error.contains(&MAX_LIBRARY_BYTES.to_string()));
        assert_eq!(
            fs::read(&path).expect("oversized library should remain readable"),
            oversized
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_regular_library_sources_are_rejected_without_blocking() {
        let directory = TestDirectory::new();
        let directory_path = directory.path.join("library-directory");
        fs::create_dir(&directory_path).expect("library directory should be creatable");

        let fifo_path = directory.path.join("library-fifo");
        let fifo_path_c = CString::new(fifo_path.as_os_str().as_bytes())
            .expect("test FIFO path should not contain NUL");
        let result = unsafe { libc::mkfifo(fifo_path_c.as_ptr(), 0o600) };
        assert_eq!(result, 0, "test FIFO should be creatable");

        for path in [directory_path.as_path(), fifo_path.as_path()] {
            let load_error = load_library_at(path).expect_err("non-file load should fail");
            assert!(load_error.contains("regular file"), "{load_error}");

            let recovery_error = preserve_unreadable_library_and_start_fresh_at(path)
                .expect_err("non-file recovery should fail");
            assert!(recovery_error.contains("regular file"), "{recovery_error}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_to_regular_library_files_are_admitted() {
        let directory = TestDirectory::new();
        let target_path = directory.path.join("library-target.json");
        fs::write(
            &target_path,
            serde_json::to_vec(&Library::default()).expect("default library should encode"),
        )
        .expect("library target should be writable");
        let symlink_path = directory.path.join("library.json");
        std::os::unix::fs::symlink(&target_path, &symlink_path)
            .expect("library symlink should be creatable");

        assert_eq!(
            load_library_at(&symlink_path).expect("symlink target should load"),
            Library::default()
        );
    }

    #[test]
    fn identity_allocator_preserves_base_shape_and_uses_deterministic_suffixes() {
        let occupied = ["track-42", "track-42-1", "track-42-2"];
        assert_eq!(
            allocate_id("track", 42, |candidate| occupied.contains(&candidate)),
            "track-42-3"
        );
        assert_eq!(allocate_id("note", 900, |_| false), "note-900");
    }

    #[test]
    fn duplicate_track_ids_are_rejected_on_load_without_rewriting_bytes() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let mut duplicate = persistence_fixture();
        duplicate.tracks.push(duplicate.tracks[0].clone());
        let bytes = serde_json::to_vec_pretty(&duplicate).expect("duplicate library should encode");
        fs::write(&path, &bytes).expect("duplicate library should be writable");

        let error = load_library_at(&path).expect_err("duplicate track IDs should fail to load");

        assert!(error.contains("duplicate track ID"));
        assert_eq!(
            fs::read(&path).expect("library should remain readable"),
            bytes
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn duplicate_note_ids_are_rejected_on_load_without_rewriting_bytes() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let mut duplicate = persistence_fixture();
        let note = duplicate.tracks[0].notes[0].clone();
        duplicate.tracks[0].notes.push(note);
        let bytes = serde_json::to_vec_pretty(&duplicate).expect("duplicate library should encode");
        fs::write(&path, &bytes).expect("duplicate library should be writable");

        let error = load_library_at(&path).expect_err("duplicate note IDs should fail to load");

        assert!(error.contains("duplicate note ID"));
        assert_eq!(
            fs::read(&path).expect("library should remain readable"),
            bytes
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn duplicate_note_ids_in_reference_track_are_rejected_on_load_without_rewriting_bytes() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let mut duplicate = persistence_fixture();
        let note = duplicate.reference_tracks[0].notes[0].clone();
        duplicate.reference_tracks[0].notes.push(note);
        let bytes = serde_json::to_vec_pretty(&duplicate).expect("duplicate library should encode");
        fs::write(&path, &bytes).expect("duplicate library should be writable");

        let error =
            load_library_at(&path).expect_err("duplicate reference note IDs should fail to load");

        assert!(error.contains("duplicate note ID"));
        assert_eq!(
            fs::read(&path).expect("library should remain readable"),
            bytes
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn duplicate_reference_paths_are_rejected_on_load_before_normalization() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let mut duplicate = persistence_fixture();
        duplicate
            .reference_tracks
            .push(duplicate.reference_tracks[0].clone());
        let bytes = serde_json::to_vec_pretty(&duplicate).expect("duplicate library should encode");
        fs::write(&path, &bytes).expect("duplicate library should be writable");

        let error =
            load_library_at(&path).expect_err("duplicate reference paths should fail to load");

        assert!(error.contains("duplicate reference path"));
        assert!(error.contains("remove or rename"));
        assert_eq!(
            fs::read(&path).expect("library should remain readable"),
            bytes
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn duplicate_note_ids_are_rejected_before_save_and_temp_creation() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let original = persistence_fixture();
        persist_library_at(&original, &path).expect("valid library should persist");
        let original_bytes = fs::read(&path).expect("original library should be readable");

        let mut duplicate = original.clone();
        let note = duplicate.tracks[0].notes[0].clone();
        duplicate.tracks[0].notes.push(note);
        let error = persist_library_at(&duplicate, &path)
            .expect_err("duplicate note IDs should fail before saving");

        assert!(error.contains("duplicate note ID"));
        assert_eq!(
            fs::read(&path).expect("original library should remain readable"),
            original_bytes
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn duplicate_reference_paths_are_rejected_before_save_and_temp_creation() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let original = persistence_fixture();
        persist_library_at(&original, &path).expect("valid library should persist");
        let original_bytes = fs::read(&path).expect("original library should be readable");

        let mut duplicate = original.clone();
        duplicate
            .reference_tracks
            .push(duplicate.reference_tracks[0].clone());
        let error = persist_library_at(&duplicate, &path)
            .expect_err("duplicate reference paths should fail before saving");

        assert!(error.contains("duplicate reference path"));
        assert!(error.contains("remove or rename"));
        assert_eq!(
            fs::read(&path).expect("original library should remain readable"),
            original_bytes
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn identical_note_ids_in_different_owners_are_valid() {
        let mut library = persistence_fixture();
        let note_id = library.tracks[0].notes[0].id.clone();
        library.reference_tracks[0].notes[0].id = note_id.clone();
        let mut second_reference = library.reference_tracks[0].clone();
        second_reference.path = PathBuf::from("/external/second-reference.wav");
        second_reference.notes[0].id = note_id;
        library.reference_tracks.push(second_reference);

        validate_library_identity(&library).expect("note IDs may be reused by different owners");
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
    fn reference_display_name_round_trips_and_legacy_records_default_to_none() {
        let library = persistence_fixture();
        let encoded = serde_json::to_string(&library).expect("named library should encode");
        assert!(encoded.contains(r#""display_name":"Reference vocal""#));
        let round_trip: Library =
            serde_json::from_str(&encoded).expect("named library should decode");
        assert_eq!(
            round_trip.reference_tracks[0].display_name.as_deref(),
            Some("Reference vocal")
        );

        let legacy: Library = serde_json::from_str(
            r#"{"tracks":[],"selected_track_id":null,"reference_tracks":[{"path":"/tmp/legacy-reference.wav"}]}"#,
        )
        .expect("legacy reference catalog should decode without display_name");
        assert_eq!(legacy.reference_tracks[0].display_name, None);
    }

    #[test]
    fn rename_reference_track_trims_rejects_blank_and_allows_duplicate_names() {
        let mut library = persistence_fixture();
        let original_path = library.reference_tracks[0].path.clone();
        let original_notes = library.reference_tracks[0].notes.clone();
        let original_assignment = library.tracks[0].reference_path.clone();
        library.reference_tracks[0].source_proof =
            crate::source::SourceProvenance::Verified(crate::source::AudioSourceProof {
                sha256: "a".repeat(64),
                byte_len: 42,
            });
        let original_proof = library.reference_tracks[0].source_proof.clone();

        assert!(
            rename_reference_track(&mut library, &original_path, "  Shared name  ")
                .expect("a non-blank name should be accepted")
        );
        assert_eq!(
            library.reference_tracks[0].display_name.as_deref(),
            Some("Shared name")
        );
        assert_eq!(library.reference_tracks[0].path, original_path);
        assert_eq!(library.reference_tracks[0].notes, original_notes);
        assert_eq!(library.reference_tracks[0].source_proof, original_proof);
        assert_eq!(library.tracks[0].reference_path, original_assignment);

        let duplicate_path = PathBuf::from("/external/duplicate-reference.wav");
        let mut duplicate = library.reference_tracks[0].clone();
        duplicate.path = duplicate_path.clone();
        duplicate.display_name = None;
        library.reference_tracks.push(duplicate);
        assert!(
            rename_reference_track(&mut library, &duplicate_path, " Shared name ")
                .expect("duplicate display names should be accepted")
        );
        assert_eq!(
            library.reference_tracks[0].display_name,
            library.reference_tracks[1].display_name
        );
        assert_eq!(library.reference_tracks[0].path, original_path);
        assert_eq!(library.reference_tracks[1].path, duplicate_path);
        assert_eq!(library.reference_tracks[1].notes, original_notes);
        assert_eq!(library.reference_tracks[1].source_proof, original_proof);
        assert_eq!(library.tracks[0].reference_path, original_assignment);

        let before_blank = library.clone();
        let error = rename_reference_track(&mut library, &original_path, " \t\n")
            .expect_err("whitespace-only names should be rejected");
        assert!(error.contains("cannot be empty"));
        assert_eq!(library, before_blank);
    }

    #[cfg(unix)]
    #[test]
    fn post_rename_parent_sync_failure_reports_a_committed_uncertain_outcome() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let original = persistence_fixture();
        persist_library_at(&original, &path).expect("original library should persist");

        let mut replacement = original.clone();
        replacement.tracks[0].title = String::from("Committed replacement");
        fail_next_persist_parent_directory_sync_for_test();

        let outcome = persist_library_at(&replacement, &path)
            .expect("a post-rename directory-sync failure must not report an ordinary error");

        assert!(matches!(
            &outcome,
            PersistenceOutcome::CommittedButDurabilityUncertain { detail }
                if detail.contains("injected post-rename parent-directory sync failure")
        ));
        assert_eq!(
            load_library_at(&path).expect("the committed replacement should reload"),
            replacement
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn shared_entity_snapshot_cow_clones_only_the_changed_entity_and_round_trips_json() {
        let second_reference_path = PathBuf::from("/external/reference-2.wav");
        let mut library = persistence_fixture();
        let mut second_track = library.tracks[0].clone();
        second_track.id = String::from("track-2");
        second_track.title = String::from("Second Track");
        second_track.path = PathBuf::from("/external/second-track.wav");
        second_track.reference_path = Some(second_reference_path.clone());
        second_track.notes[0].id = String::from("note-2");
        library.tracks.push(second_track);

        let mut second_reference = library.reference_tracks[0].clone();
        second_reference.path = second_reference_path;
        second_reference.notes[0].id = String::from("reference-note-2");
        library.reference_tracks.push(second_reference);
        library.planner_order.push(String::from("track-2"));

        let snapshot = library.clone();
        assert!(snapshot.tracks.shares_storage_with(&library.tracks));
        assert!(
            snapshot
                .tracks
                .shares_entity_storage_with(&library.tracks, 0)
        );
        assert!(
            snapshot
                .tracks
                .shares_entity_storage_with(&library.tracks, 1)
        );
        assert!(
            snapshot
                .reference_tracks
                .shares_storage_with(&library.reference_tracks)
        );
        assert!(
            snapshot
                .reference_tracks
                .shares_entity_storage_with(&library.reference_tracks, 1)
        );

        library.tracks[0].title = String::from("Mutated Track");

        assert_eq!(snapshot.tracks[0].title, "Night Drive");
        assert_eq!(library.tracks[0].title, "Mutated Track");
        assert!(!snapshot.tracks.shares_storage_with(&library.tracks));
        assert!(
            !snapshot
                .tracks
                .shares_entity_storage_with(&library.tracks, 0)
        );
        assert!(
            snapshot
                .tracks
                .shares_entity_storage_with(&library.tracks, 1)
        );
        assert!(
            snapshot
                .reference_tracks
                .shares_storage_with(&library.reference_tracks)
        );
        assert!(
            snapshot
                .reference_tracks
                .shares_entity_storage_with(&library.reference_tracks, 1)
        );

        let late_snapshot = library.clone();
        library
            .tracks
            .find_mut(|track| track.id == "track-2")
            .expect("the late main target should exist")
            .title = String::from("Late Track Mutation");
        assert!(
            late_snapshot
                .tracks
                .shares_entity_storage_with(&library.tracks, 0),
            "mutating a late main target must leave preceding tracks shared"
        );
        assert!(
            !late_snapshot
                .tracks
                .shares_entity_storage_with(&library.tracks, 1)
        );

        let late_reference_snapshot = library.clone();
        library
            .reference_tracks
            .find_mut(|reference| reference.path == Path::new("/external/reference-2.wav"))
            .expect("the late reference target should exist")
            .notes[0]
            .id = String::from("late-reference-note");
        assert!(
            late_reference_snapshot
                .reference_tracks
                .shares_entity_storage_with(&library.reference_tracks, 0),
            "mutating a late reference target must leave preceding references shared"
        );
        assert!(
            !late_reference_snapshot
                .reference_tracks
                .shares_entity_storage_with(&library.reference_tracks, 1)
        );

        let encoded = serde_json::to_vec(&library).expect("mutated library should encode");
        let round_tripped: Library =
            serde_json::from_slice(&encoded).expect("mutated library should decode");
        assert_eq!(round_tripped, library);
    }

    #[test]
    fn import_verified_batch_persists_two_valid_candidates_as_one_snapshot() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (first_path, first_decoded) = decoded_audio_fixture(&directory.path);
        let second_path = directory.path.join("second.wav");
        fs::write(&second_path, tiny_pcm_wav()).expect("second audio fixture should be writable");
        let second_decoded = crate::audio::decode_audio_file(&second_path)
            .expect("second valid audio should pass preflight");
        let original = persistence_fixture();

        let report = import_verified_batch(
            original.clone(),
            vec![
                VerifiedImportCandidate::from_decoded(&first_decoded),
                VerifiedImportCandidate::from_decoded(&second_decoded),
            ],
            &library_path,
        );

        let library = report.library.expect("a valid batch should persist");
        assert!(report.errors.is_empty());
        assert_eq!(
            report.imported_paths,
            vec![first_path.clone(), second_path.clone()]
        );
        assert_eq!(library.tracks.len(), original.tracks.len() + 2);
        assert!(library.tracks.iter().any(|track| track.path == first_path));
        assert!(library.tracks.iter().any(|track| track.path == second_path));
        let imported_ids = [first_path.as_path(), second_path.as_path()]
            .into_iter()
            .map(|path| {
                library
                    .tracks
                    .iter()
                    .find(|track| track.path == path)
                    .expect("each accepted path should have a track ID")
                    .id
                    .clone()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            library.planner_order.iter().skip(1).collect::<Vec<_>>(),
            imported_ids.iter().collect::<Vec<_>>()
        );
        assert_eq!(
            library.selected_track_id.as_deref(),
            imported_ids.last().map(String::as_str)
        );
        assert_eq!(
            load_library_at(&library_path).expect("batch should reload"),
            library
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn import_batch_keeps_accepted_paths_when_post_rename_sync_is_uncertain() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (source, decoded) = decoded_audio_fixture(&directory.path);
        let original = persistence_fixture();
        persist_library_at(&original, &library_path).expect("original library should persist");
        fail_next_persist_parent_directory_sync_for_test();

        let report = import_verified_batch(
            original,
            vec![VerifiedImportCandidate::from_decoded(&decoded)],
            &library_path,
        );

        let library = report
            .library
            .as_ref()
            .expect("the renamed batch snapshot should remain accepted");
        assert_eq!(report.imported_paths, vec![source]);
        assert!(report.errors.is_empty());
        assert!(matches!(
            report.persistence_outcome.as_ref(),
            Some(PersistenceOutcome::CommittedButDurabilityUncertain { detail })
                if detail.contains("injected post-rename parent-directory sync failure")
        ));
        assert_eq!(
            load_library_at(&library_path).expect("the committed batch should reload"),
            *library
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn import_verified_batch_reports_changed_candidate_while_another_succeeds() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (changed_path, changed_decoded) = decoded_audio_fixture(&directory.path);
        let valid_path = directory.path.join("valid.wav");
        fs::write(&valid_path, tiny_pcm_wav()).expect("valid audio fixture should be writable");
        let valid_decoded = crate::audio::decode_audio_file(&valid_path)
            .expect("valid audio should pass preflight");

        fs::write(&changed_path, b"source changed after preflight")
            .expect("changed candidate should be writable");

        let report = import_verified_batch(
            persistence_fixture(),
            vec![
                VerifiedImportCandidate::from_decoded(&changed_decoded),
                VerifiedImportCandidate::from_decoded(&valid_decoded),
            ],
            &library_path,
        );

        let library = report
            .library
            .as_ref()
            .expect("the valid candidate should still persist");
        assert_eq!(report.imported_paths, vec![valid_path.clone()]);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].path, changed_path);
        assert!(report.errors[0].error.contains("changed after preflight"));
        assert!(library.tracks.iter().any(|track| track.path == valid_path));
        assert_eq!(
            load_library_at(&library_path).expect("batch should reload"),
            *library
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn import_verified_batch_final_fence_rejects_changed_early_candidate() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (early_path, early_decoded) = decoded_audio_fixture_named(&directory.path, "early.wav");
        let (late_path, late_decoded) = decoded_audio_fixture_named(&directory.path, "late.wav");
        let early_path_for_hook = early_path.clone();
        let mut final_fence_paths = Vec::new();

        let report = import_verified_batch_with_final_fence_hook(
            persistence_fixture(),
            vec![
                VerifiedImportCandidate::from_decoded(&early_decoded),
                VerifiedImportCandidate::from_decoded(&late_decoded),
            ],
            &library_path,
            |candidate| {
                final_fence_paths.push(candidate.path.clone());
                if candidate.path == early_path_for_hook {
                    // This runs after the complete provisional pass and before
                    // the early candidate's final source fence.
                    fs::write(&early_path_for_hook, b"source changed before final fence")
                        .expect("the early source should be replaceable by the test hook");
                }
            },
        );

        assert_eq!(
            final_fence_paths,
            vec![early_path.clone(), late_path.clone()]
        );
        let library = report
            .library
            .as_ref()
            .expect("the unchanged later candidate should persist");
        assert_eq!(report.imported_paths, vec![late_path.clone()]);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].path, early_path);
        assert!(report.errors[0].error.contains("changed after preflight"));
        assert!(!library.tracks.iter().any(|track| track.path == early_path));
        assert!(library.tracks.iter().any(|track| track.path == late_path));
        assert_eq!(
            load_library_at(&library_path).expect("the final snapshot should reload"),
            *library
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn import_verified_batch_rechecks_early_candidate_after_late_final_fence() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (early_path, early_decoded) = decoded_audio_fixture_named(&directory.path, "early.wav");
        let (late_path, late_decoded) = decoded_audio_fixture_named(&directory.path, "late.wav");
        let early_path_for_hook = early_path.clone();
        let mut final_fence_paths = Vec::new();

        let report = import_verified_batch_with_final_fence_hook(
            persistence_fixture(),
            vec![
                VerifiedImportCandidate::from_decoded(&early_decoded),
                VerifiedImportCandidate::from_decoded(&late_decoded),
            ],
            &library_path,
            |candidate| {
                final_fence_paths.push(candidate.path.clone());
                if candidate.path == late_path {
                    // The early candidate has already passed its final fence
                    // when this hook runs for the late candidate.
                    fs::write(
                        &early_path_for_hook,
                        b"source changed after early final fence",
                    )
                    .expect("the early source should be replaceable by the test hook");
                }
            },
        );

        assert_eq!(
            final_fence_paths,
            vec![early_path.clone(), late_path.clone()]
        );
        let library = report
            .library
            .as_ref()
            .expect("the unchanged later candidate should persist");
        assert_eq!(report.imported_paths, vec![late_path.clone()]);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].path, early_path);
        assert!(report.errors[0].error.contains("changed after preflight"));
        assert!(!library.tracks.iter().any(|track| track.path == early_path));
        assert!(library.tracks.iter().any(|track| track.path == late_path));
        assert_eq!(
            load_library_at(&library_path).expect("the final snapshot should reload"),
            *library
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn assign_reference_verified_batch_reloads_partial_success_in_catalog_order() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (changed_path, changed_decoded) =
            decoded_audio_fixture_named(&directory.path, "changed-reference.wav");
        let (first_path, first_decoded) =
            decoded_audio_fixture_named(&directory.path, "first-reference.wav");
        let (second_path, second_decoded) =
            decoded_audio_fixture_named(&directory.path, "second-reference.wav");
        let original = persistence_fixture();
        persist_library_at(&original, &library_path).expect("the original snapshot should save");
        fs::write(&changed_path, b"reference source changed before import")
            .expect("the changed reference should be replaceable");

        let report = assign_reference_verified_batch(
            original,
            "track-1",
            vec![
                VerifiedImportCandidate::from_decoded(&changed_decoded),
                VerifiedImportCandidate::from_decoded(&first_decoded),
                VerifiedImportCandidate::from_decoded(&second_decoded),
            ],
            &library_path,
        );

        let library = report
            .library
            .as_ref()
            .expect("valid references should persist atomically");
        assert_eq!(
            report.imported_paths,
            vec![first_path.clone(), second_path.clone()]
        );
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].path, changed_path);
        assert!(report.errors[0].error.contains("changed after preflight"));
        assert_eq!(library.tracks[0].reference_path, Some(first_path.clone()));
        assert_eq!(
            library
                .reference_tracks
                .iter()
                .map(|reference| reference.path.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from("/external/reference.wav"),
                first_path,
                second_path,
            ]
        );
        assert_eq!(
            load_library_at(&library_path).expect("the assigned snapshot should reload"),
            *library
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn assign_reference_verified_batch_keeps_existing_selection_when_all_candidates_fail() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (changed_path, changed_decoded) =
            decoded_audio_fixture_named(&directory.path, "changed-reference.wav");
        let (deleted_path, deleted_decoded) =
            decoded_audio_fixture_named(&directory.path, "deleted-reference.wav");
        let original = persistence_fixture();
        persist_library_at(&original, &library_path).expect("the original snapshot should save");
        let original_bytes = fs::read(&library_path).expect("the original bytes should read");
        fs::write(&changed_path, b"reference source changed before import")
            .expect("the changed reference should be replaceable");
        fs::remove_file(&deleted_path).expect("the deleted reference should be removable");

        let report = assign_reference_verified_batch(
            original.clone(),
            "track-1",
            vec![
                VerifiedImportCandidate::from_decoded(&changed_decoded),
                VerifiedImportCandidate::from_decoded(&deleted_decoded),
            ],
            &library_path,
        );

        assert!(report.library.is_none());
        assert!(report.imported_paths.is_empty());
        assert_eq!(report.errors.len(), 2);
        assert_eq!(
            load_library_at(&library_path).expect("the unchanged snapshot should reload"),
            original
        );
        assert_eq!(
            fs::read(&library_path).expect("the unchanged bytes should read"),
            original_bytes
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn add_reference_verified_batch_reloads_partial_success_in_catalog_order() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (changed_path, changed_decoded) =
            decoded_audio_fixture_named(&directory.path, "changed-catalog-reference.wav");
        let (first_path, first_decoded) =
            decoded_audio_fixture_named(&directory.path, "first-catalog-reference.wav");
        let (second_path, second_decoded) =
            decoded_audio_fixture_named(&directory.path, "second-catalog-reference.wav");
        let original = persistence_fixture();
        persist_library_at(&original, &library_path).expect("the original snapshot should save");
        fs::write(&changed_path, b"reference source changed before import")
            .expect("the changed reference should be replaceable");

        let report = add_reference_verified_batch(
            original,
            vec![
                VerifiedImportCandidate::from_decoded(&changed_decoded),
                VerifiedImportCandidate::from_decoded(&first_decoded),
                VerifiedImportCandidate::from_decoded(&second_decoded),
            ],
            &library_path,
        );

        let library = report
            .library
            .as_ref()
            .expect("valid catalog references should persist atomically");
        assert_eq!(
            report.imported_paths,
            vec![first_path.clone(), second_path.clone()]
        );
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].path, changed_path);
        assert!(report.errors[0].error.contains("changed after preflight"));
        assert_eq!(
            library.tracks[0].reference_path,
            Some(PathBuf::from("/external/reference.wav"))
        );
        assert_eq!(
            library
                .reference_tracks
                .iter()
                .map(|reference| reference.path.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from("/external/reference.wav"),
                first_path,
                second_path,
            ]
        );
        assert_eq!(
            load_library_at(&library_path).expect("the catalog snapshot should reload"),
            *library
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn import_verified_batch_reports_deleted_candidate_while_another_succeeds() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (deleted_path, deleted_decoded) = decoded_audio_fixture(&directory.path);
        let valid_path = directory.path.join("valid.wav");
        fs::write(&valid_path, tiny_pcm_wav()).expect("valid audio fixture should be writable");
        let valid_decoded = crate::audio::decode_audio_file(&valid_path)
            .expect("valid audio should pass preflight");

        fs::remove_file(&deleted_path).expect("deleted candidate should be removable");

        let report = import_verified_batch(
            persistence_fixture(),
            vec![
                VerifiedImportCandidate::from_decoded(&deleted_decoded),
                VerifiedImportCandidate::from_decoded(&valid_decoded),
            ],
            &library_path,
        );

        let library = report
            .library
            .as_ref()
            .expect("the valid candidate should still persist");
        assert_eq!(report.imported_paths, vec![valid_path.clone()]);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].path, deleted_path);
        assert!(report.errors[0].error.contains("changed after preflight"));
        assert!(library.tracks.iter().any(|track| track.path == valid_path));
        assert_eq!(
            load_library_at(&library_path).expect("batch should reload"),
            *library
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn import_verified_batch_all_failed_leaves_existing_snapshot_unchanged() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (changed_path, changed_decoded) = decoded_audio_fixture(&directory.path);
        let deleted_path = directory.path.join("deleted.wav");
        fs::write(&deleted_path, tiny_pcm_wav()).expect("deleted fixture should be writable");
        let deleted_decoded = crate::audio::decode_audio_file(&deleted_path)
            .expect("deleted fixture should pass preflight");
        let original = persistence_fixture();
        persist_library_at(&original, &library_path).expect("original library should persist");
        let original_bytes = fs::read(&library_path).expect("original bytes should be readable");

        fs::write(&changed_path, b"source changed after preflight")
            .expect("changed candidate should be writable");
        fs::remove_file(&deleted_path).expect("deleted candidate should be removable");

        let report = import_verified_batch(
            original.clone(),
            vec![
                VerifiedImportCandidate::from_decoded(&changed_decoded),
                VerifiedImportCandidate::from_decoded(&deleted_decoded),
            ],
            &library_path,
        );

        assert!(report.library.is_none());
        assert!(report.imported_paths.is_empty());
        assert_eq!(report.errors.len(), 2);
        assert!(report.errors.iter().any(|error| {
            error.path == changed_path && error.error.contains("changed after preflight")
        }));
        assert!(report.errors.iter().any(|error| {
            error.path == deleted_path && error.error.contains("changed after preflight")
        }));
        assert_eq!(
            fs::read(&library_path).expect("snapshot should remain readable"),
            original_bytes
        );
        assert_eq!(
            load_library_at(&library_path).expect("snapshot should reload"),
            original
        );
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
    fn oversized_recovery_streams_an_exact_backup_and_resets_the_library() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let mut oversized = Vec::with_capacity(MAX_LIBRARY_BYTES + 4_096);
        oversized.extend_from_slice(b"not-json\n");
        oversized.resize(MAX_LIBRARY_BYTES + 4_096, b'x');
        fs::write(&path, &oversized).expect("oversized library should be writable");

        let backup_path = preserve_unreadable_library_and_start_fresh_at(&path)
            .expect("oversized recovery should preserve and reset the library");

        assert_eq!(
            fs::read(&backup_path.value).expect("backup should be readable"),
            oversized
        );
        assert_eq!(
            load_library_at(&path).expect("fresh library should load"),
            Library::default()
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn oversized_persistence_preserves_old_snapshot_without_a_temp_file() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let original = persistence_fixture();
        persist_library_at(&original, &path).expect("original library should persist");
        let original_bytes = fs::read(&path).expect("original snapshot should be readable");

        let mut oversized = original.clone();
        oversized
            .tracks
            .get_mut(0)
            .expect("fixture should contain a track")
            .title = "x".repeat(MAX_LIBRARY_BYTES);
        assert!(
            serde_json::to_vec_pretty(&oversized)
                .expect("oversized library should encode")
                .len()
                > MAX_LIBRARY_BYTES
        );

        let error = persist_library_at(&oversized, &path)
            .expect_err("oversized library should fail before temp creation");

        assert!(error.contains("maximum supported size"));
        assert_eq!(
            fs::read(&path).expect("old snapshot should remain readable"),
            original_bytes
        );
        assert!(temporary_paths(&directory.path).is_empty());
    }

    #[test]
    fn recovery_backup_is_exact_and_active_library_becomes_default() {
        let directory = TestDirectory::new();
        let path = directory.path.join("library.json");
        let malformed = b"not-json\0with-original-bytes".to_vec();
        fs::write(&path, &malformed).expect("malformed library should be writable");

        let backup_path = preserve_unreadable_library_and_start_fresh_at(&path)
            .expect("recovery should preserve and reset the library");

        assert_ne!(backup_path.value, path);
        assert_eq!(backup_path.parent(), path.parent());
        assert_eq!(
            fs::read(&backup_path.value).expect("backup should be readable"),
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
    fn stage_labels_and_wire_order_are_stable() {
        assert_eq!(
            TrackStage::ALL.map(TrackStage::label),
            ["Backlog", "Production", "Mixdown", "Mastering"]
        );
        assert_eq!(
            TrackStage::ALL
                .into_iter()
                .map(|stage| serde_json::to_string(&stage).unwrap())
                .collect::<Vec<_>>(),
            [
                "\"sound-design\"",
                "\"production\"",
                "\"mixdown\"",
                "\"mastering\""
            ]
        );
    }

    #[test]
    fn seven_stage_libraries_migrate_without_losing_tracks() {
        let stages = [
            "sound-design",
            "groove",
            "arrangement",
            "polish",
            "mixdown",
            "master",
            "release",
        ];
        let expected = [
            TrackStage::Backlog,
            TrackStage::Production,
            TrackStage::Production,
            TrackStage::Production,
            TrackStage::Mixdown,
            TrackStage::Mastering,
            TrackStage::Mastering,
        ];
        let tracks: Vec<_> = stages
            .iter()
            .enumerate()
            .map(|(i, stage)| {
                let mut value = serde_json::to_value(planner_test_track(
                    &format!("track-{i}"),
                    TrackStage::Backlog,
                    false,
                ))
                .unwrap();
                value["stage"] = serde_json::Value::String((*stage).into());
                value
            })
            .collect();
        let value = serde_json::json!({"tracks": tracks, "selected_track_id": "track-3", "reference_tracks": []});
        let library: Library = serde_json::from_value(value).unwrap();
        assert_eq!(library.tracks.len(), stages.len());
        assert_eq!(library.selected_track_id.as_deref(), Some("track-3"));
        assert_eq!(
            library.tracks.iter().map(|t| t.stage).collect::<Vec<_>>(),
            expected
        );
        let saved = serde_json::to_value(&library).unwrap();
        assert_eq!(saved["tracks"][3]["stage"], "production");
        assert_eq!(saved["tracks"][6]["stage"], "mastering");
    }

    #[test]
    fn legacy_status_fields_load_and_are_omitted_on_save() {
        let encoded = r#"{
            "tracks": [
                {"id":"legacy-inbox","title":"Inbox","original_name":"inbox.wav","path":"/tmp/inbox.wav","size":0,"favorite":false,"stage":"sound-design","status":"inbox","notes":[]},
                {"id":"legacy-refine","title":"Refine","original_name":"refine.wav","path":"/tmp/refine.wav","size":0,"favorite":false,"stage":"sound-design","status":"refine","notes":[]},
                {"id":"legacy-release","title":"Release","original_name":"release.wav","path":"/tmp/release.wav","size":0,"favorite":false,"stage":"sound-design","status":"release","notes":[]},
                {"id":"legacy-archive","title":"Archive","original_name":"archive.wav","path":"/tmp/archive.wav","size":0,"favorite":false,"stage":"sound-design","status":"archive","notes":[]},
                {"id":"legacy-maybe","title":"Maybe","original_name":"maybe.wav","path":"/tmp/maybe.wav","size":0,"favorite":false,"stage":"sound-design","status":"maybe","notes":[]}
            ],
            "selected_track_id": null
        }"#;

        let library: Library = serde_json::from_str(encoded).expect("legacy statuses should load");
        assert_eq!(library.tracks.len(), 5);
        assert!(
            library
                .tracks
                .iter()
                .all(|track| track.stage == TrackStage::Backlog)
        );

        let reserialized = serde_json::to_string(&library).expect("library should reserialize");
        assert!(!reserialized.contains("\"status\""));
        assert!(reserialized.contains("\"stage\":\"sound-design\""));
    }

    #[test]
    fn waveform_cache_paths_are_stable_and_source_specific() {
        let proof = crate::source::AudioSourceProof {
            sha256: "a".repeat(64),
            byte_len: 42,
        };
        let first = waveform_cache_path(Path::new("/external/first.wav"), &proof);
        assert_eq!(
            first,
            waveform_cache_path(Path::new("/external/first.wav"), &proof)
        );
        assert_ne!(
            first,
            waveform_cache_path(Path::new("/external/second.wav"), &proof)
        );
        assert!(first.to_string_lossy().contains("waveform-cache-v2"));
        let other = crate::source::AudioSourceProof {
            sha256: "b".repeat(64),
            byte_len: 42,
        };
        assert_ne!(
            first,
            waveform_cache_path(Path::new("/external/first.wav"), &other)
        );
    }

    #[test]
    fn removing_a_track_only_changes_library_metadata() {
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: None,
                size: 42,
                favorite: false,
                stage: TrackStage::Backlog,
                notes: SharedVec::default(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: vec![ReferenceTrack {
                path: PathBuf::from("/tmp/reference.wav"),
                display_name: None,
                source_proof: crate::source::SourceProvenance::Unknown,
                notes: vec![Note {
                    id: String::from("reference-note-1"),
                    time_millis: 900,
                    body: String::from("Compare the low-end tail."),
                    done: false,
                    ..Default::default()
                }]
                .into(),
            }]
            .into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            reviewer_name: None,
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
            source_proof: crate::source::SourceProvenance::Unknown,
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Backlog,
            notes: SharedVec::default(),
            ..Default::default()
        };
        let library = Library {
            tracks: vec![track("track-2"), track("track-3")].into(),
            selected_track_id: None,
            reference_tracks: Vec::new().into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            ..Default::default()
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
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: None,
                size: 42,
                favorite: false,
                stage: TrackStage::Backlog,
                notes: SharedVec::default(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new().into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            reviewer_name: None,
        };

        assert!(
            !set_track_stage(&mut library, "track-1", TrackStage::Backlog)
                .expect("track should exist")
        );
        assert!(
            set_track_stage(&mut library, "track-1", TrackStage::Mixdown)
                .expect("track should exist")
        );
        assert_eq!(library.tracks[0].stage, TrackStage::Mixdown);
    }

    fn planner_test_track(id: &str, stage: TrackStage, favorite: bool) -> Track {
        Track {
            id: String::from(id),
            title: String::from(id),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            source_proof: crate::source::SourceProvenance::Unknown,
            reference_path: None,
            size: 0,
            favorite,
            stage,
            notes: SharedVec::default(),
            ..Default::default()
        }
    }

    #[test]
    fn planner_order_normalizes_legacy_and_stale_ids() {
        let mut legacy = Library {
            tracks: vec![
                planner_test_track("plain", TrackStage::Production, false),
                planner_test_track("starred", TrackStage::Production, true),
            ]
            .into(),
            selected_track_id: None,
            reference_tracks: Vec::new().into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            ..Default::default()
        };
        normalize_planner_order(&mut legacy);
        assert_eq!(legacy.planner_order.as_slice(), ["starred", "plain"]);

        legacy.planner_order = vec![
            String::from("missing"),
            String::from("plain"),
            String::from("plain"),
        ]
        .into();
        normalize_planner_order(&mut legacy);
        assert_eq!(legacy.planner_order.as_slice(), ["plain", "starred"]);
    }

    #[test]
    fn planner_tracks_preserve_order_semantics_and_first_match_identity() {
        let mut first_duplicate = planner_test_track("duplicate", TrackStage::Production, false);
        first_duplicate.title = String::from("first duplicate");
        let mut later_duplicate = planner_test_track("duplicate", TrackStage::Mixdown, true);
        later_duplicate.title = String::from("later duplicate");
        let explicit = Library {
            tracks: vec![
                planner_test_track("tail", TrackStage::Mastering, false),
                first_duplicate.clone(),
                later_duplicate.clone(),
                planner_test_track("appended", TrackStage::Backlog, false),
            ]
            .into(),
            selected_track_id: None,
            reference_tracks: Vec::new().into(),
            planner_order: vec![
                String::from("missing"),
                String::from("duplicate"),
                String::from("duplicate"),
                String::from("tail"),
            ]
            .into(),
            tags: Default::default(),
            ..Default::default()
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
                planner_test_track("plain", TrackStage::Production, false),
                planner_test_track("favorite", TrackStage::Production, true),
                first_duplicate,
                later_duplicate,
            ]
            .into(),
            selected_track_id: None,
            reference_tracks: Vec::new().into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            ..Default::default()
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
                planner_test_track("a", TrackStage::Backlog, false),
                planner_test_track("b", TrackStage::Production, false),
                planner_test_track("c", TrackStage::Production, false),
            ]
            .into(),
            selected_track_id: None,
            reference_tracks: Vec::new().into(),
            planner_order: vec![String::from("a"), String::from("b"), String::from("c")].into(),
            tags: Default::default(),
            ..Default::default()
        };

        assert!(
            move_track_to_planner_slot(&mut library, "c", TrackStage::Production, 0)
                .expect("same-stage move should validate")
        );
        assert_eq!(library.planner_order.as_slice(), ["a", "c", "b"]);
        assert_eq!(library.tracks[1].stage, TrackStage::Production);

        assert!(
            move_track_to_planner_slot(&mut library, "a", TrackStage::Production, 2)
                .expect("cross-stage move should validate")
        );
        assert_eq!(library.planner_order.as_slice(), ["c", "b", "a"]);
        assert_eq!(library.tracks[0].stage, TrackStage::Production);
    }

    #[test]
    fn planner_move_keeps_unrelated_track_storage_in_place() {
        let mut library = Library {
            tracks: vec![
                planner_test_track("source", TrackStage::Production, false),
                planner_test_track("unrelated", TrackStage::Production, false),
            ]
            .into(),
            selected_track_id: None,
            reference_tracks: Vec::new().into(),
            planner_order: vec![String::from("source"), String::from("unrelated")].into(),
            tags: Default::default(),
            ..Default::default()
        };
        let unrelated_pointer = std::ptr::from_ref(&library.tracks[1]);

        assert!(
            move_track_to_planner_slot(&mut library, "source", TrackStage::Production, 2)
                .expect("same-stage reorder should validate")
        );
        assert_eq!(library.planner_order.as_slice(), ["unrelated", "source"]);
        assert_eq!(std::ptr::from_ref(&library.tracks[1]), unrelated_pointer);
    }

    #[test]
    fn planner_move_adjusts_target_after_source_and_preserves_order() {
        let mut library = Library {
            tracks: vec![
                planner_test_track("a", TrackStage::Production, false),
                planner_test_track("hidden-one", TrackStage::Production, false),
                planner_test_track("b", TrackStage::Production, false),
                planner_test_track("hidden-two", TrackStage::Production, false),
            ]
            .into(),
            selected_track_id: None,
            reference_tracks: Vec::new().into(),
            planner_order: vec![
                String::from("a"),
                String::from("hidden-one"),
                String::from("b"),
                String::from("hidden-two"),
            ]
            .into(),
            tags: Default::default(),
            ..Default::default()
        };

        assert!(
            move_track_to_planner_slot(&mut library, "a", TrackStage::Production, 2,)
                .expect("end target should validate")
        );
        assert_eq!(
            library.planner_order.as_slice(),
            ["hidden-one", "a", "b", "hidden-two"]
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
            tracks: vec![planner_test_track("a", TrackStage::Production, false)].into(),
            selected_track_id: None,
            reference_tracks: Vec::new().into(),
            planner_order: vec![String::from("a")].into(),
            tags: Default::default(),
            ..Default::default()
        };
        let before = library.clone();

        let error = move_track_to_planner_slot(&mut library, "a", TrackStage::Production, 2)
            .expect_err("a slot beyond the visible list should be rejected");
        assert!(error.contains("no longer available"));
        assert_eq!(library, before);
    }

    #[test]
    fn planner_move_accepts_an_empty_stage_target() {
        let mut library = Library {
            tracks: vec![planner_test_track("a", TrackStage::Production, false)].into(),
            selected_track_id: None,
            reference_tracks: Vec::new().into(),
            planner_order: vec![String::from("a")].into(),
            tags: Default::default(),
            ..Default::default()
        };

        assert!(
            move_track_to_planner_slot(&mut library, "a", TrackStage::Mastering, 0)
                .expect("an empty stage target should validate")
        );
        assert_eq!(library.planner_order.as_slice(), ["a"]);
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
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: None,
                size: 42,
                favorite: true,
                stage: TrackStage::Mixdown,
                notes: vec![Note {
                    id: String::from("note-1"),
                    time_millis: 1_250,
                    body: String::from("Recheck the vocal entrance."),
                    done: false,
                    ..Default::default()
                }]
                .into(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new().into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            reviewer_name: None,
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
        let mut library = persistence_fixture();
        library.tracks.push(Track {
            id: String::from("track-2"),
            title: String::from("Other Track"),
            original_name: String::from("other.wav"),
            path: PathBuf::from("/external/other.wav"),
            source_proof: crate::source::SourceProvenance::Unknown,
            reference_path: None,
            size: 84,
            favorite: false,
            stage: TrackStage::Backlog,
            notes: SharedVec::default(),
            ..Default::default()
        });
        library.planner_order.push(String::from("track-2"));
        library.selected_track_id = Some(String::from("track-2"));
        assert_eq!(library.selected_track_id.as_deref(), Some("track-2"));
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
        assert_eq!(replaced.selected_track_id.as_deref(), Some("track-1"));
        let reloaded = load_library_at(&library_path).expect("replaced library should reload");
        assert_eq!(reloaded.selected_track_id.as_deref(), Some("track-1"));
        assert_eq!(reloaded, replaced.value);
    }

    #[test]
    fn replacing_reference_catalog_entry_preserves_order_notes_assignments_and_reloads() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let replacement_path = directory.path.join("replacement.wav");
        fs::write(&replacement_path, tiny_pcm_wav()).expect("replacement fixture should write");
        let decoded = crate::audio::decode_audio_file(&replacement_path)
            .expect("replacement fixture should decode");
        let original_path = PathBuf::from("/external/original-reference.wav");
        let other_path = PathBuf::from("/external/other-reference.wav");
        let original_note = Note {
            id: String::from("reference-note"),
            time_millis: 250,
            body: String::from("Keep this note on the catalog entry."),
            done: false,
            ..Default::default()
        };
        let track = |id: &str, reference_path: Option<PathBuf>| Track {
            id: id.to_owned(),
            title: id.to_owned(),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            source_proof: crate::source::SourceProvenance::Unknown,
            reference_path,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            notes: SharedVec::default(),
            ..Default::default()
        };
        let library = Library {
            tracks: vec![
                track("first", Some(original_path.clone())),
                track("second", Some(original_path.clone())),
                track("other", Some(other_path.clone())),
            ]
            .into(),
            selected_track_id: Some(String::from("second")),
            reference_tracks: vec![
                ReferenceTrack {
                    path: original_path.clone(),
                    display_name: Some(String::from("Original reference")),
                    source_proof: crate::source::SourceProvenance::Unknown,
                    notes: vec![original_note.clone()].into(),
                },
                ReferenceTrack {
                    path: other_path.clone(),
                    display_name: None,
                    source_proof: crate::source::SourceProvenance::Unknown,
                    notes: SharedVec::default(),
                },
            ]
            .into(),
            planner_order: vec![
                String::from("other"),
                String::from("first"),
                String::from("second"),
            ]
            .into(),
            tags: Default::default(),
            ..Default::default()
        };
        persist_library_at(&library, &library_path).expect("original library should persist");

        let replaced = replace_reference_track_at(
            library.clone(),
            &original_path,
            None,
            decoded,
            &library_path,
        )
        .expect("reference replacement should persist");

        assert_eq!(
            replaced
                .reference_tracks
                .iter()
                .map(|reference| reference.path.clone())
                .collect::<Vec<_>>(),
            vec![replacement_path.clone(), other_path]
        );
        assert_eq!(
            replaced.reference_tracks[0].notes,
            vec![original_note].into()
        );
        assert_eq!(
            replaced.reference_tracks[0].display_name.as_deref(),
            Some("Original reference")
        );
        assert_eq!(
            replaced
                .tracks
                .iter()
                .map(|track| track.reference_path.clone())
                .collect::<Vec<_>>(),
            vec![
                Some(replacement_path.clone()),
                Some(replacement_path.clone()),
                Some(PathBuf::from("/external/other-reference.wav")),
            ]
        );
        assert_eq!(
            replaced.planner_order.as_slice(),
            ["other", "first", "second"]
        );
        assert!(
            replaced.reference_tracks[0]
                .source_provenance()
                .verified_proof()
                .is_some()
        );
        assert_eq!(
            load_library_at(&library_path).expect("replaced library should reload"),
            replaced.value
        );
    }

    #[test]
    fn stale_reference_catalog_proof_rejects_before_persistence() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let replacement_path = directory.path.join("replacement.wav");
        fs::write(&replacement_path, tiny_pcm_wav()).expect("replacement fixture should write");
        let decoded = crate::audio::decode_audio_file(&replacement_path)
            .expect("replacement fixture should decode");
        let original_path = PathBuf::from("/external/original-reference.wav");
        let library = Library {
            tracks: vec![Track {
                id: String::from("owner"),
                title: String::from("Owner"),
                original_name: String::from("owner.wav"),
                path: PathBuf::from("/external/owner.wav"),
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: Some(original_path.clone()),
                size: 0,
                favorite: false,
                stage: TrackStage::Backlog,
                notes: SharedVec::default(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("owner")),
            reference_tracks: vec![ReferenceTrack {
                path: original_path.clone(),
                display_name: None,
                source_proof: crate::source::SourceProvenance::Verified(
                    decoded.source_proof().clone(),
                ),
                notes: vec![Note {
                    id: String::from("keep-note"),
                    time_millis: 100,
                    body: String::from("Keep this note."),
                    done: false,
                    ..Default::default()
                }]
                .into(),
            }]
            .into(),
            planner_order: vec![String::from("owner")].into(),
            tags: Default::default(),
            reviewer_name: None,
        };
        persist_library_at(&library, &library_path).expect("original library should persist");
        let original_bytes = fs::read(&library_path).expect("original snapshot should read");
        let stale_proof = crate::source::AudioSourceProof {
            sha256: "a".repeat(64),
            byte_len: decoded.source_proof().byte_len,
        };

        let error = replace_reference_track_at(
            library.clone(),
            &original_path,
            Some(&stale_proof),
            decoded,
            &library_path,
        )
        .expect_err("a stale catalog proof must reject the worker commit");

        assert!(error.contains("Reference catalog changed"));
        assert_eq!(
            library,
            load_library_at(&library_path).expect("library should reload")
        );
        assert_eq!(
            fs::read(&library_path).expect("stale replacement must not rewrite the snapshot"),
            original_bytes
        );
    }

    #[test]
    fn replacing_reference_catalog_entry_refreshes_same_path_proof_and_preserves_notes() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let source = directory.path.join("same-reference.wav");
        fs::write(&source, tiny_pcm_wav()).expect("reference fixture should write");
        let first = crate::audio::decode_audio_file(&source).expect("first proof should decode");
        let note = Note {
            id: String::from("same-path-note"),
            time_millis: 100,
            body: String::from("Preserve across a same-path re-import."),
            done: true,
            ..Default::default()
        };
        let library = Library {
            tracks: vec![Track {
                id: String::from("owner"),
                title: String::from("Owner"),
                original_name: String::from("owner.wav"),
                path: PathBuf::from("/external/owner.wav"),
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: Some(source.clone()),
                size: 0,
                favorite: false,
                stage: TrackStage::Backlog,
                notes: SharedVec::default(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("owner")),
            reference_tracks: vec![ReferenceTrack {
                path: source.clone(),
                display_name: None,
                source_proof: crate::source::SourceProvenance::Verified(
                    first.source_proof().clone(),
                ),
                notes: vec![note.clone()].into(),
            }]
            .into(),
            planner_order: vec![String::from("owner")].into(),
            tags: Default::default(),
            reviewer_name: None,
        };
        persist_library_at(&library, &library_path).expect("original library should persist");

        let mut changed = tiny_pcm_wav();
        let last = changed.len() - 1;
        changed[last] = 1;
        fs::write(&source, changed).expect("changed reference should write");
        let second = crate::audio::decode_audio_file(&source).expect("second proof should decode");
        assert_ne!(first.source_proof(), second.source_proof());
        let second_proof = second.source_proof().clone();

        let replaced = replace_reference_track_at(
            library,
            &source,
            Some(first.source_proof()),
            second,
            &library_path,
        )
        .expect("same-path reference replacement should persist");

        assert_eq!(replaced.reference_tracks[0].path, source);
        assert_eq!(
            replaced.reference_tracks[0]
                .source_provenance()
                .verified_proof(),
            Some(&second_proof)
        );
        assert_eq!(replaced.reference_tracks[0].notes, vec![note].into());
        assert_eq!(replaced.tracks[0].reference_path, Some(source.clone()));
        assert_eq!(
            load_library_at(&library_path).expect("same-path replacement should reload"),
            replaced.value
        );
    }

    #[test]
    fn replacing_reference_catalog_entry_rejects_another_entry_path_atomically() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let occupied_path = directory.path.join("occupied.wav");
        fs::write(&occupied_path, tiny_pcm_wav()).expect("occupied fixture should write");
        let decoded = crate::audio::decode_audio_file(&occupied_path)
            .expect("occupied fixture should decode");
        let original_path = PathBuf::from("/external/original-reference.wav");
        let library = Library {
            tracks: vec![Track {
                id: String::from("owner"),
                title: String::from("Owner"),
                original_name: String::from("owner.wav"),
                path: PathBuf::from("/external/owner.wav"),
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: Some(original_path.clone()),
                size: 0,
                favorite: false,
                stage: TrackStage::Backlog,
                notes: SharedVec::default(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("owner")),
            reference_tracks: vec![
                ReferenceTrack {
                    path: original_path.clone(),
                    display_name: None,
                    source_proof: crate::source::SourceProvenance::Unknown,
                    notes: SharedVec::default(),
                },
                ReferenceTrack {
                    path: occupied_path.clone(),
                    display_name: None,
                    source_proof: crate::source::SourceProvenance::Unknown,
                    notes: SharedVec::default(),
                },
            ]
            .into(),
            planner_order: vec![String::from("owner")].into(),
            tags: Default::default(),
            reviewer_name: None,
        };
        persist_library_at(&library, &library_path).expect("original library should persist");
        let original_bytes = fs::read(&library_path).expect("original snapshot should read");

        let error = replace_reference_track_at(
            library.clone(),
            &original_path,
            None,
            decoded,
            &library_path,
        )
        .expect_err("replacement must reject another catalog owner's path");

        assert!(error.contains("already owned"));
        assert_eq!(
            library,
            load_library_at(&library_path).expect("library should remain intact")
        );
        assert_eq!(
            fs::read(&library_path).expect("snapshot should remain intact"),
            original_bytes
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
            .source_provenance()
            .verified_proof()
            .cloned()
            .expect("new main imports should carry a proof");
        assert_eq!(proof.byte_len, tiny_pcm_wav().len() as u64);
        assert_eq!(imported.tracks[0].path, source);
        assert_eq!(
            load_library_at(&library_path).expect("persisted import should reload"),
            imported.value
        );
        let json = fs::read_to_string(&library_path).expect("persisted JSON should be readable");
        assert!(json.contains("\"source_proof\""));

        let legacy: Library = serde_json::from_str(
            r#"{"tracks":[{"id":"legacy","title":"Legacy","original_name":"legacy.wav","path":"/tmp/legacy.wav","reference_path":null,"size":0,"favorite":false,"stage":"sound-design","status":"inbox","notes":[]}],"selected_track_id":null}"#,
        )
        .expect("legacy JSON without a proof should load");
        assert_eq!(
            legacy.tracks[0].source_proof,
            crate::source::SourceProvenance::Unknown
        );
        assert!(
            serde_json::to_string(&legacy)
                .expect("legacy library should encode")
                .contains(r#""source_proof":null"#)
        );
    }

    #[test]
    fn legacy_reference_records_remain_unknown_through_selection_and_import() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (source, decoded) = decoded_audio_fixture(&directory.path);
        let notes: SharedVec<Note> = vec![Note {
            id: String::from("reference-note"),
            time_millis: 125,
            body: String::from("keep this note"),
            done: false,
            ..Default::default()
        }]
        .into();
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("owner"),
                title: String::from("Owner"),
                original_name: String::from("owner.wav"),
                path: PathBuf::from("/tmp/owner.wav"),
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: Some(source.clone()),
                size: 0,
                favorite: false,
                stage: TrackStage::Backlog,
                notes: SharedVec::default(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("owner")),
            reference_tracks: vec![ReferenceTrack {
                path: source.clone(),
                display_name: None,
                source_proof: crate::source::SourceProvenance::Unknown,
                notes: notes.clone(),
            }]
            .into(),
            planner_order: vec![String::from("owner")].into(),
            tags: Default::default(),
            reviewer_name: None,
        };

        let selected =
            set_reference_track_at(library.clone(), "owner", decoded.clone(), &library_path)
                .expect("legacy reference selection should remain usable");
        assert_eq!(selected.tracks[0].reference_path, Some(source.clone()));
        assert_eq!(selected.reference_tracks[0].notes, notes);
        assert_eq!(
            selected.reference_tracks[0].source_proof,
            crate::source::SourceProvenance::Unknown
        );
        assert!(
            selected.reference_tracks[0]
                .source_provenance()
                .is_unknown()
        );

        library = selected.value.clone();
        let persisted = add_reference_track_at(library.clone(), decoded, &library_path)
            .expect("same path and proof should remain idempotent");
        assert_eq!(persisted.value, library);
        assert_eq!(persisted.reference_tracks[0].notes, notes);
        assert!(
            persisted.reference_tracks[0]
                .source_provenance()
                .is_unknown()
        );
    }

    #[test]
    fn source_provenance_migrates_missing_null_and_verified_proofs_and_rejects_malformed_data() {
        let missing: Library = serde_json::from_str(
            r#"{"tracks":[{"id":"legacy-missing","title":"Legacy","original_name":"legacy.wav","path":"/tmp/legacy.wav","reference_path":null,"size":0,"favorite":false,"stage":"sound-design","status":"inbox","notes":[]}],"selected_track_id":null}"#,
        )
        .expect("missing source proof should load");
        assert!(missing.tracks[0].source_provenance().is_unknown());

        let null: Library = serde_json::from_str(
            r#"{"tracks":[{"id":"legacy-null","title":"Legacy","original_name":"legacy.wav","path":"/tmp/legacy.wav","source_proof":null,"reference_path":null,"size":0,"favorite":false,"stage":"sound-design","status":"inbox","notes":[]}],"selected_track_id":null}"#,
        )
        .expect("null source proof should load");
        assert!(null.tracks[0].source_provenance().is_unknown());
        assert!(
            serde_json::to_string(&null)
                .expect("unknown provenance should encode")
                .contains(r#""source_proof":null"#)
        );

        let verified: Library = serde_json::from_str(
            r#"{"tracks":[{"id":"verified","title":"Verified","original_name":"verified.wav","path":"/tmp/verified.wav","source_proof":{"sha256":"0000000000000000000000000000000000000000000000000000000000000000","byte_len":12},"reference_path":null,"size":12,"favorite":false,"stage":"sound-design","status":"inbox","notes":[]}],"selected_track_id":null}"#,
        )
        .expect("proof object should load");
        assert!(matches!(
            verified.tracks[0].source_provenance(),
            crate::source::SourceProvenance::Verified(_)
        ));
        let verified_json = serde_json::to_string(&verified).expect("verified should encode");
        assert!(verified_json.contains(
            r#""source_proof":{"sha256":"0000000000000000000000000000000000000000000000000000000000000000","byte_len":12}"#
        ));

        let malformed: Result<Library, _> = serde_json::from_str(
            r#"{"tracks":[{"id":"malformed","title":"Malformed","original_name":"bad.wav","path":"/tmp/bad.wav","source_proof":{"sha256":"not-a-proof","byte_len":12},"reference_path":null,"size":12,"favorite":false,"stage":"sound-design","status":"inbox","notes":[]}],"selected_track_id":null}"#,
        );
        assert!(malformed.is_err(), "malformed proof data must fail closed");
    }

    #[test]
    fn explicit_source_binding_changes_only_provenance_and_persists_notes() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let main_path = directory.path.join("main.wav");
        let reference_path = directory.path.join("reference.wav");
        let proof = crate::source::AudioSourceProof {
            sha256: "1".repeat(64),
            byte_len: 46,
        };
        let main_note = Note {
            id: String::from("main-note"),
            time_millis: 10,
            body: String::from("preserve main bytes"),
            done: true,
            ..Default::default()
        };
        let reference_note = Note {
            id: String::from("reference-note"),
            time_millis: 20,
            body: String::from("preserve reference bytes"),
            done: false,
            ..Default::default()
        };
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("owner"),
                title: String::from("Owner"),
                original_name: String::from("owner.wav"),
                path: main_path.clone(),
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: Some(reference_path.clone()),
                size: 46,
                favorite: true,
                stage: TrackStage::Mixdown,
                notes: vec![main_note.clone()].into(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("owner")),
            reference_tracks: vec![ReferenceTrack {
                path: reference_path.clone(),
                display_name: None,
                source_proof: crate::source::SourceProvenance::Unknown,
                notes: vec![reference_note.clone()].into(),
            }]
            .into(),
            planner_order: vec![String::from("owner")].into(),
            tags: Default::default(),
            reviewer_name: None,
        };
        persist_library_at(&library, &library_path).expect("unknown library should persist");

        bind_main_source_proof(&mut library, "owner", &main_path, proof.clone())
            .expect("main binding should succeed");
        bind_reference_source_proof(&mut library, &reference_path, proof.clone())
            .expect("reference binding should succeed");

        assert_eq!(library.tracks[0].notes, vec![main_note].into());
        assert_eq!(
            library.reference_tracks[0].notes,
            vec![reference_note].into()
        );
        assert_eq!(
            library.tracks[0].source_proof,
            crate::source::SourceProvenance::Verified(proof.clone())
        );
        assert_eq!(
            library.reference_tracks[0].source_proof,
            crate::source::SourceProvenance::Verified(proof)
        );
        persist_library_at(&library, &library_path).expect("bound library should persist");
        assert_eq!(
            load_library_at(&library_path).expect("bound library should reload"),
            library
        );
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
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: Some(source.clone()),
                size: 0,
                favorite: false,
                stage: TrackStage::Backlog,
                notes: SharedVec::default(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("owner")),
            reference_tracks: vec![ReferenceTrack {
                path: source.clone(),
                display_name: None,
                source_proof: crate::source::SourceProvenance::Verified(
                    first.source_proof().clone(),
                ),
                notes: vec![Note {
                    id: String::from("keep"),
                    time_millis: 1,
                    body: String::from("keep"),
                    done: true,
                    ..Default::default()
                }]
                .into(),
            }]
            .into(),
            planner_order: vec![String::from("owner")].into(),
            tags: Default::default(),
            reviewer_name: None,
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
            crate::source::SourceProvenance::Verified(first.source_proof().clone())
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
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: Some(PathBuf::from("/tmp/reference.wav")),
                size: 42,
                favorite: true,
                stage: TrackStage::Mixdown,
                notes: vec![Note {
                    id: String::from("note-1"),
                    time_millis: 1_250,
                    body: String::from("Check the kick tail."),
                    done: false,
                    ..Default::default()
                }]
                .into(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new().into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            reviewer_name: None,
        };
        let encoded = serde_json::to_string(&library).expect("library should encode");
        assert!(encoded.contains(r#""stage":"mixdown""#));
        assert!(!encoded.contains(r#""status""#));
        let decoded: Library = serde_json::from_str(&encoded).expect("library should decode");
        assert_eq!(decoded, library);
    }

    #[test]
    fn shared_vec_serializes_as_an_array_and_detaches_on_mutation() {
        let mut values = SharedVec::from(vec![String::from("first")]);
        let snapshot = values.clone();

        assert!(values.shares_storage_with(&snapshot));
        assert_eq!(
            serde_json::to_string(&values).expect("values should encode"),
            r#"["first"]"#
        );

        values.push(String::from("second"));

        assert_eq!(snapshot.as_slice(), ["first"]);
        assert_eq!(values.as_slice(), ["first", "second"]);
        assert!(!values.shares_storage_with(&snapshot));
        let round_trip: SharedVec<String> =
            serde_json::from_str(r#"["first","second"]"#).expect("values should decode");
        assert_eq!(round_trip, values);
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
        assert_eq!(library.tracks[0].stage, TrackStage::Backlog);
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
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: Some(PathBuf::from("/external/reference.wav")),
                size: 0,
                favorite: false,
                stage: TrackStage::Backlog,
                notes: SharedVec::default(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new().into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            reviewer_name: None,
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
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: None,
                size: 42,
                favorite: false,
                stage: TrackStage::Backlog,
                notes: SharedVec::default(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new().into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            reviewer_name: None,
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
            source_proof: crate::source::SourceProvenance::Unknown,
            reference_path,
            size: 0,
            favorite: false,
            stage: TrackStage::Backlog,
            notes: SharedVec::default(),
            ..Default::default()
        };
        let mut library = Library {
            tracks: vec![
                track("assigned-1", Some(removed_path.clone())),
                track("assigned-2", Some(removed_path.clone())),
                track("retained", Some(retained_path.clone())),
            ]
            .into(),
            selected_track_id: Some(String::from("assigned-1")),
            reference_tracks: vec![
                ReferenceTrack {
                    path: removed_path.clone(),
                    display_name: None,
                    source_proof: crate::source::SourceProvenance::Unknown,
                    notes: vec![Note {
                        id: String::from("removed-note"),
                        time_millis: 100,
                        body: String::from("Discard with the catalog entry."),
                        done: false,
                        ..Default::default()
                    }]
                    .into(),
                },
                ReferenceTrack {
                    path: retained_path.clone(),
                    display_name: None,
                    source_proof: crate::source::SourceProvenance::Unknown,
                    notes: SharedVec::default(),
                },
            ]
            .into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            ..Default::default()
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
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: Some(first_path.clone()),
                size: 0,
                favorite: false,
                stage: TrackStage::Backlog,
                notes: SharedVec::default(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: vec![
                ReferenceTrack {
                    path: first_path.clone(),
                    display_name: None,
                    source_proof: crate::source::SourceProvenance::Unknown,
                    notes: vec![Note {
                        id: String::from("first-note"),
                        time_millis: 100,
                        body: String::from("First reference only."),
                        done: false,
                        ..Default::default()
                    }]
                    .into(),
                },
                ReferenceTrack {
                    path: second_path.clone(),
                    display_name: None,
                    source_proof: crate::source::SourceProvenance::Unknown,
                    notes: vec![Note {
                        id: String::from("second-note"),
                        time_millis: 200,
                        body: String::from("Second reference only."),
                        done: false,
                        ..Default::default()
                    }]
                    .into(),
                },
            ]
            .into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            reviewer_name: None,
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
    fn setting_reference_track_metadata_preserves_primary_track_and_comments() {
        let mut library = Library {
            tracks: vec![Track {
                id: String::from("track-1"),
                title: String::from("Night Drive"),
                original_name: String::from("night-drive.wav"),
                path: PathBuf::from("/external/night-drive.wav"),
                source_proof: crate::source::SourceProvenance::Unknown,
                reference_path: None,
                size: 42,
                favorite: true,
                stage: TrackStage::Mixdown,
                notes: vec![Note {
                    id: String::from("note-1"),
                    time_millis: 1_250,
                    body: String::from("Keep the vocal entrance."),
                    done: false,
                    ..Default::default()
                }]
                .into(),
                ..Default::default()
            }]
            .into(),
            selected_track_id: Some(String::from("track-1")),
            reference_tracks: Vec::new().into(),
            planner_order: Vec::new().into(),
            tags: Default::default(),
            reviewer_name: None,
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

    fn distinct_pcm_wav(amplitude: i16) -> Vec<u8> {
        let samples = vec![amplitude; 80];
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::from(*b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&8_000_u32.to_le_bytes());
        bytes.extend_from_slice(&16_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    fn write_decoded_bounce(
        directory: &Path,
        filename: &str,
        amplitude: i16,
    ) -> (PathBuf, crate::audio::DecodedAudioFile) {
        let path = directory.join(filename);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create bounce parent directory");
        }
        fs::write(&path, distinct_pcm_wav(amplitude)).expect("write bounce fixture");
        let decoded = crate::audio::decode_audio_file(&path).expect("decode bounce fixture");
        (path, decoded)
    }

    #[test]
    fn parse_bounce_filename_handles_canonical_legacy_multi_letter_untitled_and_invalid() {
        let a1 = parse_bounce_filename(Path::new("/bounces/A1_b1__track-title.wav"))
            .expect("canonical titled bounce should parse");
        assert_eq!(a1.batch, "A");
        assert_eq!(a1.slot, 1);
        assert_eq!(a1.group_key, "A1");
        assert_eq!(a1.build, 1);
        assert_eq!(a1.title.as_deref(), Some("track title"));

        let untitled = parse_bounce_filename(Path::new("A1_b2.wav"))
            .expect("untitled canonical bounce should parse");
        assert_eq!(untitled.group_key, "A1");
        assert_eq!(untitled.build, 2);
        assert_eq!(untitled.title, None);

        let multi_letter = parse_bounce_filename(Path::new("AA1_b1.wav"))
            .expect("multi-letter batch should parse");
        assert_eq!(multi_letter.batch, "AA");
        assert_eq!(multi_letter.slot, 1);
        assert_eq!(multi_letter.group_key, "AA1");
        assert_eq!(multi_letter.build, 1);

        let legacy_padded = parse_bounce_filename(Path::new("A1_B02.wav"))
            .expect("legacy uppercase B with zero padding should parse");
        assert_eq!(legacy_padded.group_key, "A1");
        assert_eq!(legacy_padded.build, 2);
        assert_eq!(legacy_padded.build, untitled.build);

        let legacy_titled = parse_bounce_filename(Path::new("A1_B2__new-title.wav"))
            .expect("legacy titled bounce should parse");
        assert_eq!(legacy_titled.build, 2);
        assert_eq!(legacy_titled.title.as_deref(), Some("new title"));

        let slot_zero = parse_bounce_filename(Path::new("AB0_b10.flac"))
            .expect("slot 0 and multi-digit build should parse");
        assert_eq!(slot_zero.group_key, "AB0");
        assert_eq!(slot_zero.build, 10);

        for invalid in [
            "a1_b1.wav",
            "A10_b1.wav",
            "A1_b0.wav",
            "A1_b00.wav",
            "A1_b2_extra.wav",
            "my-demo-song.wav",
            "A1_b1.txt",
        ] {
            assert!(
                parse_bounce_filename(Path::new(invalid)).is_none(),
                "{invalid} must not match bounce convention"
            );
        }
    }

    #[test]
    fn drop_order_grouping_keeps_highest_build_current_and_never_renames_audio() {
        for reverse in [false, true] {
            let directory = TestDirectory::new();
            let library_path = directory.path.join("library.json");
            let mut files = vec![
                write_decoded_bounce(&directory.path, "A1_b10.wav", 1000),
                write_decoded_bounce(&directory.path, "A2_b1.wav", 2000),
                write_decoded_bounce(&directory.path, "AA1_b1.wav", 3000),
                write_decoded_bounce(&directory.path, "A1_B2__new-title.wav", 4000),
            ];
            if reverse {
                files.reverse();
            }

            let mut library = Library::default();
            for (_, decoded) in &files {
                let saved = import_bounce_into_library_at(library, decoded.clone(), &library_path)
                    .expect("import should succeed");
                library = saved.value.0;
            }

            assert_eq!(library.tracks.len(), 3);
            let a1 = library
                .tracks
                .iter()
                .find(|t| t.group_key.as_deref() == Some("A1"))
                .expect("A1 track should exist");
            assert_eq!(a1.revisions.len(), 2);
            assert_eq!(a1.current_build_label(), "b10");
            assert_eq!(a1.active_build_label(), "b10");
            assert_eq!(a1.title, "new title");
            let a1_id = a1.id.clone();

            // Title edits never split the track on subsequent builds.
            assert!(set_track_title(&mut library, &a1_id, "Custom Arrangement").unwrap());
            let (_, b11) = write_decoded_bounce(&directory.path, "A1_b11__other-name.wav", 5000);
            let saved = import_bounce_into_library_at(library, b11, &library_path)
                .expect("b11 import should succeed");
            library = saved.value.0;
            assert_eq!(library.tracks.len(), 3);
            let a1 = library
                .tracks
                .iter()
                .find(|t| t.group_key.as_deref() == Some("A1"))
                .expect("A1 track should remain unified");
            assert_eq!(a1.title, "Custom Arrangement");
            assert_eq!(a1.revisions.len(), 3);
            assert_eq!(a1.current_build_label(), "b11");

            // Source audio files on disk are never renamed.
            for (path, _) in &files {
                assert!(path.is_file(), "source file {} must stay in place", path.display());
            }
        }
    }

    #[test]
    fn duplicate_redrop_is_ignored_and_conflicting_build_flags_collision_without_overwrite() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (path_b2, b2) = write_decoded_bounce(&directory.path, "A1_b2.wav", 1200);
        let saved = import_bounce_into_library_at(Library::default(), b2, &library_path)
            .expect("initial b2 import should succeed");
        let mut library = saved.value.0;
        let track_id = saved.value.1.track_id().to_owned();

        // Identical bytes under legacy padded name A1_B02.wav -> ignored duplicate.
        let (_, b02_same) = write_decoded_bounce(&directory.path, "A1_B02.wav", 1200);
        let saved = import_bounce_into_library_at(library, b02_same, &library_path)
            .expect("duplicate drop should succeed cleanly");
        library = saved.value.0;
        assert!(matches!(
            saved.value.1,
            BounceImportOutcome::DuplicateIgnored { .. }
        ));
        assert_eq!(library.tracks[0].revisions.len(), 1);
        assert!(library.tracks[0].collisions.is_empty());

        // Different bytes under A1_B02.wav -> collision flagged, original b2 preserved.
        let (conflict_path, b02_conflict) =
            write_decoded_bounce(&directory.path, "conflict/A1_B02.wav", 9500);
        let saved = import_bounce_into_library_at(library, b02_conflict, &library_path)
            .expect("collision drop should flag without error");
        library = saved.value.0;
        let collision_id = match saved.value.1 {
            BounceImportOutcome::CollisionFlagged {
                collision_id,
                build,
                ..
            } => {
                assert_eq!(build, 2);
                collision_id
            }
            other => panic!("expected CollisionFlagged, got {other:?}"),
        };
        assert_eq!(library.tracks[0].path, path_b2);
        assert_eq!(library.tracks[0].revisions.len(), 1);
        assert_eq!(library.tracks[0].collisions.len(), 1);

        // Keep existing dismisses collision without changing b2.
        let mut kept = library.clone();
        resolve_collision_keep_existing(&mut kept, &track_id, &collision_id).unwrap();
        assert!(kept.tracks[0].collisions.is_empty());
        assert_eq!(kept.tracks[0].path, path_b2);

        // Replace build updates b2 intentionally.
        let mut replaced = library.clone();
        resolve_collision_replace_build(&mut replaced, &track_id, &collision_id).unwrap();
        assert!(replaced.tracks[0].collisions.is_empty());
        assert_eq!(replaced.tracks[0].path, conflict_path);

        // Import as separate track leaves original b2 untouched and creates a new track.
        let mut split = library.clone();
        let new_id =
            resolve_collision_import_as_new_track(&mut split, &track_id, &collision_id).unwrap();
        assert!(split.tracks[0].collisions.is_empty());
        assert_eq!(split.tracks[0].path, path_b2);
        assert_eq!(split.tracks.len(), 2);
        assert_eq!(split.tracks[1].id, new_id);
        assert_eq!(split.tracks[1].path, conflict_path);
    }

    #[test]
    fn manual_assignment_attaches_non_matching_files_to_existing_track() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (_, a1_b1) = write_decoded_bounce(&directory.path, "A1_b1__starter.wav", 1000);
        let saved = import_bounce_into_library_at(Library::default(), a1_b1, &library_path)
            .expect("import A1");
        let mut library = saved.value.0;
        let a1_id = library.tracks[0].id.clone();
        set_track_stage(&mut library, &a1_id, TrackStage::Production).unwrap();
        set_track_next_action(&mut library, &a1_id, "Tighten snare").unwrap();

        // Direct attach of a non-matching filename via add_revision_to_track.
        let (rough_path, rough) = write_decoded_bounce(&directory.path, "rough-mix.wav", 2200);
        let saved = add_revision_to_track_at(library, &a1_id, rough, &library_path)
            .expect("manual attach should succeed");
        library = saved.value.0;
        assert_eq!(library.tracks.len(), 1);
        assert_eq!(library.tracks[0].revisions.len(), 2);
        assert_eq!(library.tracks[0].stage, TrackStage::Production);
        assert_eq!(library.tracks[0].next_action, "Tighten snare");
        assert!(rough_path.is_file());

        // Standalone non-matching track can also be manually assigned into A1.
        let (_, standalone) = write_decoded_bounce(&directory.path, "extra-take.wav", 3300);
        let saved = import_bounce_into_library_at(library, standalone, &library_path)
            .expect("standalone import should succeed");
        library = saved.value.0;
        assert_eq!(library.tracks.len(), 2);
        let standalone_id = library.tracks[1].id.clone();
        assign_standalone_track_as_revision(&mut library, &standalone_id, &a1_id)
            .expect("manual assignment from standalone should succeed");
        assert_eq!(library.tracks.len(), 1);
        assert_eq!(library.tracks[0].revisions.len(), 3);
        assert_eq!(library.tracks[0].stage, TrackStage::Production);
        assert_eq!(library.tracks[0].next_action, "Tighten snare");
    }

    #[test]
    fn per_build_notes_stage_next_action_and_relink_survive_relaunch() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let (_, b1) = write_decoded_bounce(&directory.path, "A1_b1__groove.wav", 1500);
        let saved = import_bounce_into_library_at(Library::default(), b1, &library_path)
            .expect("import b1");
        let mut library = saved.value.0;
        let track_id = library.tracks[0].id.clone();
        let b1_rev_id = library.tracks[0].active_revision_id.clone().unwrap();

        set_track_stage(&mut library, &track_id, TrackStage::Mixdown).unwrap();
        set_track_next_action(&mut library, &track_id, "Trim intro by 4 bars").unwrap();
        set_revision_feedback_note(&mut library, &track_id, Some(&b1_rev_id), "Heard by Dylan")
            .unwrap();
        library.tracks[0].notes.push(Note {
            id: String::from("note-b1"),
            time_millis: 12_500,
            body: String::from("Kick dips at drop"),
            done: false,
            ..Default::default()
        });
        sync_active_revision_notes(&mut library, &track_id);

        // Adding b2 preserves stage and next action, keeps b1 notes on b1.
        let (b2_path, b2) = write_decoded_bounce(&directory.path, "A1_b2__groove.wav", 2500);
        let saved = import_bounce_into_library_at(library, b2, &library_path).expect("import b2");
        library = saved.value.0;
        assert_eq!(library.tracks.len(), 1);
        assert_eq!(library.tracks[0].stage, TrackStage::Mixdown);
        assert_eq!(library.tracks[0].next_action, "Trim intro by 4 bars");
        assert_eq!(library.tracks[0].current_build_label(), "b2");
        assert!(library.tracks[0].notes.is_empty());

        library.tracks[0].notes.push(Note {
            id: String::from("note-b2"),
            time_millis: 45_000,
            body: String::from("Outro transition smoother"),
            done: false,
            ..Default::default()
        });
        sync_active_revision_notes(&mut library, &track_id);
        let b2_rev_id = library.tracks[0].active_revision_id.clone().unwrap();

        // Switching to b1 shows b1's notes at their original timestamp and Dylan feedback note.
        assert!(select_track_revision(&mut library, &track_id, &b1_rev_id).unwrap());
        assert!(library.tracks[0].is_viewing_prior_build());
        assert_eq!(library.tracks[0].notes.len(), 1);
        assert_eq!(library.tracks[0].notes[0].time_millis, 12_500);
        assert_eq!(
            library.tracks[0]
                .active_revision()
                .unwrap()
                .feedback_note
                .as_deref(),
            Some("Heard by Dylan")
        );

        // Switch back to b2, move b2 on disk, confirm missing-source state, relink in place.
        assert!(select_track_revision(&mut library, &track_id, &b2_rev_id).unwrap());
        let moved_path = directory.path.join("moved/A1_b2__groove.wav");
        fs::create_dir_all(moved_path.parent().unwrap()).unwrap();
        fs::rename(&b2_path, &moved_path).unwrap();
        assert!(library.tracks[0].is_source_missing());

        let moved_decoded =
            crate::audio::decode_audio_file(&moved_path).expect("decode moved bounce");
        let relinked = relink_track_revision_at(
            library,
            &track_id,
            Some(&b2_rev_id),
            moved_decoded,
            &library_path,
        )
        .expect("relink should succeed");
        assert_eq!(relinked.tracks.len(), 1, "relink must not duplicate the card");
        assert!(!relinked.tracks[0].is_source_missing());
        assert_eq!(relinked.tracks[0].path, moved_path);
        assert_eq!(relinked.tracks[0].notes.len(), 1);
        assert_eq!(relinked.tracks[0].notes[0].id, "note-b2");
        assert_eq!(relinked.tracks[0].revisions.len(), 2);

        // Reload from disk (relaunch) and verify everything survives.
        let reloaded = load_library_at(&library_path).expect("relaunch load should succeed");
        let track = &reloaded.tracks[0];
        assert_eq!(track.stage, TrackStage::Mixdown);
        assert_eq!(track.next_action, "Trim intro by 4 bars");
        assert_eq!(track.path, moved_path);
        assert_eq!(track.current_build_label(), "b2");
        assert_eq!(track.revisions.len(), 2);
        let rev_b1 = track
            .revisions
            .iter()
            .find(|r| r.build == Some(1))
            .expect("b1 history preserved");
        assert_eq!(rev_b1.notes.len(), 1);
        assert_eq!(rev_b1.notes[0].time_millis, 12_500);
        assert_eq!(rev_b1.feedback_note.as_deref(), Some("Heard by Dylan"));
    }

    #[test]
    fn five_track_baseline_and_two_bounce_dylan_round_survive_relaunch() {
        let directory = TestDirectory::new();
        let library_path = directory.path.join("library.json");
        let fixtures = [
            ("A1_b1__opener.wav", 1000_i16, TrackStage::Backlog, "Cut 8-bar intro"),
            ("A2_b1__peak.wav", 2000, TrackStage::Production, "Check break energy with Dylan"),
            ("A3_b1__bridge.wav", 3000, TrackStage::Mixdown, "Tame 250Hz mud"),
            ("A4_b1__roller.wav", 4000, TrackStage::Production, "Bring hats up 1dB"),
            ("A5_b1__closer.wav", 5000, TrackStage::Mastering, "Final loudness pass"),
        ];

        let mut library = Library::default();
        for (name, amp, stage, action) in fixtures {
            let (_, decoded) = write_decoded_bounce(&directory.path, name, amp);
            let saved =
                import_bounce_into_library_at(library, decoded, &library_path).expect("import");
            library = saved.value.0;
            let id = saved.value.1.track_id().to_owned();
            set_track_stage(&mut library, &id, stage).unwrap();
            set_track_next_action(&mut library, &id, action).unwrap();
            library
                .tracks
                .find_mut(|t| t.id == id)
                .unwrap()
                .notes
                .push(Note {
                    id: format!("n-{id}"),
                    time_millis: 5_000,
                    body: format!("Baseline note for {name}"),
                    done: false,
                    ..Default::default()
                });
            sync_active_revision_notes(&mut library, &id);
        }

        // Two-bounce Dylan feedback round on A2.
        let a2_id = library
            .tracks
            .iter()
            .find(|t| t.group_key.as_deref() == Some("A2"))
            .unwrap()
            .id
            .clone();
        set_revision_feedback_note(&mut library, &a2_id, None, "Heard by Dylan — extend DJ exit")
            .unwrap();
        let (_, a2_b2) = write_decoded_bounce(&directory.path, "A2_b2__peak.wav", 2600);
        let saved =
            import_bounce_into_library_at(library, a2_b2, &library_path).expect("import A2_b2");
        library = saved.value.0;
        set_track_next_action(&mut library, &a2_id, "Verify 16-bar DJ outro after Dylan notes")
            .unwrap();
        persist_library_at(&library, &library_path).expect("persist five-track baseline");

        let reloaded = load_library_at(&library_path).expect("relaunch load");
        assert_eq!(reloaded.tracks.len(), 5);
        for track in &reloaded.tracks {
            assert!(!track.next_action.is_empty());
            assert!(!track.is_source_missing());
        }
        let a2 = reloaded
            .tracks
            .iter()
            .find(|t| t.group_key.as_deref() == Some("A2"))
            .unwrap();
        assert_eq!(a2.current_build_label(), "b2");
        assert_eq!(a2.revisions.len(), 2);
        assert_eq!(
            a2.next_action,
            "Verify 16-bar DJ outro after Dylan notes"
        );
        let a2_b1 = a2.revisions.iter().find(|r| r.build == Some(1)).unwrap();
        assert_eq!(
            a2_b1.feedback_note.as_deref(),
            Some("Heard by Dylan — extend DJ exit")
        );
        assert_eq!(a2_b1.notes.len(), 1);
    }

    #[test]
    fn feedback_package_exports_merges_by_sha256_extracts_when_new_and_is_idempotent() {
        let reviewer_dir = TestDirectory::new();
        let owner_dir = TestDirectory::new();
        let recipient_dir = TestDirectory::new();

        // 1. Reviewer (Wanja) imports someone else's WAV as an external track, adds comments, and exports package.
        let (friend_wav, decoded) =
            write_decoded_bounce(&reviewer_dir.path, "A1_b2__club-tool.wav", 2400);
        let reviewer_lib_path = reviewer_dir.path.join("library.json");
        let saved = import_external_bounce_into_library_at(
            Library::default(),
            decoded,
            &reviewer_lib_path,
        )
        .expect("external import should succeed");
        let mut reviewer_lib = saved.value.0;
        let reviewer_track_id = saved.value.1.track_id().to_owned();
        assert!(reviewer_lib.tracks[0].external);
        reviewer_lib.tracks[0].notes.push(Note {
            id: String::from("rev-n1"),
            time_millis: 15_000,
            body: String::from("Sub drops 2dB at the break"),
            done: false,
            ..Default::default()
        });
        sync_active_revision_notes(&mut reviewer_lib, &reviewer_track_id);

        let pkg_path = reviewer_dir.path.join("club-tool-review.cadence-feedback");
        let exported = export_track_feedback_package(
            &reviewer_lib,
            &reviewer_track_id,
            None,
            "Wanja",
            Some("Strong groove, check the break transition"),
            &pkg_path,
        )
        .expect("export feedback package should succeed");
        assert_eq!(exported, pkg_path);
        assert!(friend_wav.is_file(), "source audio must never be moved or renamed");

        // 2. Owner (Dylan) ALREADY has the exact same bounce under a different filename/path + personal notes.
        let owner_lib_path = owner_dir.path.join("library.json");
        let owner_shared_dir = owner_dir.path.join("shared-audio");
        let (_, owner_decoded) =
            write_decoded_bounce(&owner_dir.path, "my_local_master_copy.wav", 2400);
        let owner_saved =
            import_bounce_into_library_at(Library::default(), owner_decoded, &owner_lib_path)
                .expect("owner import");
        let mut owner_lib = owner_saved.value.0;
        let owner_track_id = owner_saved.value.1.track_id().to_owned();
        owner_lib.tracks[0].notes.push(Note {
            id: String::from("own-n1"),
            time_millis: 5_000,
            body: String::from("My own intro note"),
            done: false,
            ..Default::default()
        });
        sync_active_revision_notes(&mut owner_lib, &owner_track_id);

        let merged = import_feedback_package_at(
            owner_lib,
            &exported,
            &owner_lib_path,
            &owner_shared_dir,
        )
        .expect("SHA-256 merge should succeed");
        let (owner_lib, result) = merged.value;
        match result {
            FeedbackImportResult::MergedIntoExisting {
                track_id,
                reviewer,
                added_notes,
                skipped_duplicates,
                ..
            } => {
                assert_eq!(track_id, owner_track_id);
                assert_eq!(reviewer, "Wanja");
                assert_eq!(added_notes, 1);
                assert_eq!(skipped_duplicates, 0);
            }
            other => panic!("expected MergedIntoExisting, got {other:?}"),
        }
        assert_eq!(owner_lib.tracks.len(), 1, "must not duplicate owner track");
        assert!(!owner_lib.tracks[0].external, "owner track stays own");
        assert!(!owner_shared_dir.exists(), "must not copy audio when matched locally");
        assert_eq!(owner_lib.tracks[0].notes.len(), 2);
        assert_eq!(owner_lib.tracks[0].notes[0].author, None);
        assert_eq!(owner_lib.tracks[0].notes[1].author.as_deref(), Some("Wanja"));
        assert_eq!(owner_lib.tracks[0].notes[1].time_millis, 15_000);
        assert!(
            owner_lib.tracks[0]
                .active_revision()
                .unwrap()
                .feedback_note
                .as_deref()
                .unwrap()
                .contains("Wanja: Strong groove, check the break transition")
        );

        // Re-importing the exact same package is idempotent (0 duplicates added).
        let reimported = import_feedback_package_at(
            owner_lib,
            &exported,
            &owner_lib_path,
            &owner_shared_dir,
        )
        .expect("idempotent re-import should succeed");
        assert!(matches!(
            reimported.value.1,
            FeedbackImportResult::MergedIntoExisting {
                added_notes: 0,
                skipped_duplicates: 1,
                ..
            }
        ));
        assert_eq!(reimported.value.0.tracks[0].notes.len(), 2);

        // 3. Fresh recipient without the audio locally extracts the bundled audio and marks external.
        let recipient_lib_path = recipient_dir.path.join("library.json");
        let recipient_shared_dir = recipient_dir.path.join("shared-audio");
        let imported = import_feedback_package_at(
            Library::default(),
            &exported,
            &recipient_lib_path,
            &recipient_shared_dir,
        )
        .expect("fresh recipient import should extract bundled audio");
        let (recipient_lib, result) = imported.value;
        match result {
            FeedbackImportResult::ImportedWithAudio {
                reviewer,
                added_notes,
                extracted_audio_path,
                ..
            } => {
                assert_eq!(reviewer, "Wanja");
                assert_eq!(added_notes, 1);
                assert!(extracted_audio_path.is_file());
            }
            other => panic!("expected ImportedWithAudio, got {other:?}"),
        }
        assert_eq!(recipient_lib.tracks.len(), 1);
        assert!(recipient_lib.tracks[0].external);
        assert_eq!(recipient_lib.tracks[0].notes.len(), 1);
        assert_eq!(recipient_lib.tracks[0].notes[0].author.as_deref(), Some("Wanja"));

        // Reload survives relaunch.
        let reloaded = load_library_at(&recipient_lib_path).expect("relaunch load");
        assert_eq!(reloaded.tracks[0].notes[0].author.as_deref(), Some("Wanja"));
        assert!(reloaded.tracks[0].external);
    }
}

//! Strong identity proofs for user-owned audio files.
//!
//! A path is only a locator.  The import path uses this module to bind the
//! bytes that were inspected and decoded to the same file that is accepted by
//! persistence.  Hashing is deliberately performed over the encoded file
//! bytes; decoded PCM is not part of source identity.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as DeError};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, File, Metadata},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[cfg(not(unix))]
use std::time::{SystemTime, UNIX_EPOCH};

/// The maximum amount of encoded source data read per hashing operation.
///
/// Keeping this buffer fixed prevents a large source file from turning an
/// import preflight into an unbounded allocation.
pub const HASH_BUFFER_SIZE: usize = 64 * 1024;
const SHA256_HEX_LENGTH: usize = 64;

/// A stable proof for one encoded audio file.
///
/// `sha256` is always a lowercase ASCII hexadecimal digest.  The manual
/// serde implementation validates both incoming and outgoing values so a
/// malformed persisted proof fails closed instead of becoming a weak cache
/// key or comparison value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSourceProof {
    pub sha256: String,
    pub byte_len: u64,
}

/// The persisted provenance state for an audio owner.
///
/// The wire representation remains the historical `source_proof` field: an
/// object for a verified owner and `null` for an unknown legacy owner. This
/// keeps old snapshots compatible while making the unknown state explicit in
/// memory so a decode ticket cannot be mistaken for durable ownership.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SourceProvenance {
    #[default]
    Unknown,
    Verified(AudioSourceProof),
}

impl Serialize for SourceProvenance {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Unknown => serializer.serialize_none(),
            Self::Verified(proof) => proof.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for SourceProvenance {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<AudioSourceProof>::deserialize(deserializer)
            .map(Self::from_optional)
            .map_err(D::Error::custom)
    }
}

impl SourceProvenance {
    pub fn from_optional(proof: Option<AudioSourceProof>) -> Self {
        proof.map_or(Self::Unknown, Self::Verified)
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }

    pub fn verified_proof(&self) -> Option<&AudioSourceProof> {
        match self {
            Self::Unknown => None,
            Self::Verified(proof) => Some(proof),
        }
    }
}

impl AudioSourceProof {
    pub fn from_digest(digest: [u8; 32], byte_len: u64) -> Self {
        Self {
            sha256: hex_digest(&digest),
            byte_len,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_digest(&self.sha256)
    }
}

impl Serialize for AudioSourceProof {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate().map_err(serde::ser::Error::custom)?;
        #[derive(Serialize)]
        struct Wire<'a> {
            sha256: &'a str,
            byte_len: u64,
        }
        Wire {
            sha256: &self.sha256,
            byte_len: self.byte_len,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AudioSourceProof {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            sha256: String,
            byte_len: u64,
        }

        let wire = Wire::deserialize(deserializer)?;
        validate_digest(&wire.sha256).map_err(D::Error::custom)?;
        Ok(Self {
            sha256: wire.sha256,
            byte_len: wire.byte_len,
        })
    }
}

/// Metadata identity captured for an opened source file.
///
/// Unix targets use the filesystem's device, inode, length, modification
/// time, and change time.  Other targets retain length and portable clock
/// values while leaving device/inode at zero; the proof hash remains the
/// authoritative fallback when those filesystem identities are unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFileStamp {
    pub dev: u64,
    pub inode: u64,
    pub len: u64,
    pub mtime_nanos: i128,
    pub ctime_nanos: i128,
}

/// Runtime proof that one path was observed with one encoded-byte digest and
/// filesystem stamp.  A ticket is cloneable so the background decoder can
/// return the same verified identity to the UI without reopening or hashing
/// the source again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VerifiedSourceTicket {
    path: PathBuf,
    proof: AudioSourceProof,
    stamp: SourceFileStamp,
}

impl<'de> Deserialize<'de> for VerifiedSourceTicket {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            path: PathBuf,
            proof: AudioSourceProof,
            stamp: SourceFileStamp,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.path, wire.proof, wire.stamp).map_err(D::Error::custom)
    }
}

impl VerifiedSourceTicket {
    /// Build a ticket from proof material that has already been verified by a
    /// source worker.  Reject malformed proofs so cache records and runtime
    /// state cannot carry a weak identity.
    pub fn new(
        path: PathBuf,
        proof: AudioSourceProof,
        stamp: SourceFileStamp,
    ) -> Result<Self, String> {
        proof.validate()?;
        if path.as_os_str().is_empty() {
            return Err(String::from("source ticket path must not be empty"));
        }
        if proof.byte_len != stamp.len {
            return Err(String::from(
                "source ticket proof length must match its filesystem stamp",
            ));
        }
        Ok(Self { path, proof, stamp })
    }

    pub fn from_verified_file(verified: &VerifiedSourceFile) -> Self {
        Self {
            path: verified.path.clone(),
            proof: verified.proof.clone(),
            stamp: verified.stamp,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn proof(&self) -> &AudioSourceProof {
        &self.proof
    }

    #[allow(dead_code)]
    pub fn stamp(&self) -> SourceFileStamp {
        self.stamp
    }

    /// Validate only the current filesystem stamp.  Callers that already
    /// possess the ticket deliberately avoid re-reading encoded bytes here.
    pub fn validate_current(
        &self,
        should_cancel: impl Fn() -> bool,
    ) -> Result<(), SourceProofError> {
        validate_path_stamp(&self.path, self.stamp, should_cancel)
    }
}

/// Open the path named by a verified ticket and validate the metadata on that
/// exact handle before handing it to a decoder.
///
/// The handle metadata is the authority here.  Do not add a path-level stat
/// before opening: an atomic replacement can otherwise land between the stat
/// and open and silently bind playback to a different inode.  Once the handle
/// matches the ticket, later path replacement does not affect the bytes read
/// by that playback session; a future reload opens a fresh handle and must
/// match the ticket again.
pub fn open_for_ticket(ticket: &VerifiedSourceTicket) -> Result<File, SourceProofError> {
    let file = File::open(ticket.path()).map_err(|error| open_failure(ticket.path(), error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_failure(ticket.path(), error))?;
    if !metadata.is_file() {
        return Err(SourceProofError::Changed {
            path: ticket.path().to_path_buf(),
            detail: String::from("opened source is no longer a regular file"),
        });
    }
    if SourceFileStamp::from_metadata(&metadata) != ticket.stamp() {
        return Err(changed_ticket_stamp(ticket.path()));
    }
    Ok(file)
}

impl SourceFileStamp {
    pub fn from_metadata(metadata: &Metadata) -> Self {
        platform_stamp(metadata)
    }

    #[allow(dead_code)]
    pub fn byte_len(self) -> u64 {
        self.len
    }
}

/// Typed failures used by source-proof open/hash/validation helpers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceProofError {
    Missing {
        path: PathBuf,
    },
    Changed {
        path: PathBuf,
        detail: String,
    },
    Io {
        path: PathBuf,
        kind: io::ErrorKind,
        detail: String,
    },
    Cancelled {
        path: PathBuf,
    },
}

impl SourceProofError {
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        match self {
            Self::Missing { path }
            | Self::Changed { path, .. }
            | Self::Io { path, .. }
            | Self::Cancelled { path } => path,
        }
    }
}

impl fmt::Display for SourceProofError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { path } => {
                write!(formatter, "Audio source is missing: {}", path.display())
            }
            Self::Changed { path, detail } => {
                write!(
                    formatter,
                    "Audio source changed at {}: {detail}",
                    path.display()
                )
            }
            Self::Io {
                path,
                kind: _,
                detail,
            } => write!(
                formatter,
                "Could not inspect audio source {}: {detail}",
                path.display()
            ),
            Self::Cancelled { path } => {
                write!(
                    formatter,
                    "Audio source preflight cancelled: {}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for SourceProofError {}

/// An opened source whose bytes were hashed and whose cursor is rewound for
/// decoding.  The file handle stays owned by the preflight worker so decoding
/// can use a clone of this same opened source rather than reopening by path.
#[derive(Debug)]
pub struct VerifiedSourceFile {
    path: PathBuf,
    file: File,
    proof: AudioSourceProof,
    stamp: SourceFileStamp,
}

impl VerifiedSourceFile {
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn proof(&self) -> &AudioSourceProof {
        &self.proof
    }

    #[allow(dead_code)]
    pub fn stamp(&self) -> SourceFileStamp {
        self.stamp
    }

    pub fn ticket(&self) -> VerifiedSourceTicket {
        VerifiedSourceTicket::from_verified_file(self)
    }

    /// Clone the already-opened source handle for a decoder.  The original
    /// handle remains available for the post-decode validation pass.
    pub fn try_clone_for_decode(&self) -> Result<File, SourceProofError> {
        self.file
            .try_clone()
            .map_err(|error| io_failure(&self.path, error))
    }

    /// Rewind the retained handle and verify the path/handle metadata after
    /// decoding has finished. The encoded digest was already captured before
    /// decode; this fence intentionally stays O(1) and does not rehash bytes.
    pub fn validate_after_decode(
        &mut self,
        should_cancel: impl Fn() -> bool,
    ) -> Result<(), SourceProofError> {
        validate_open_metadata(&self.path, &self.file, self.stamp, &should_cancel)?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|error| io_failure(&self.path, error))?;
        Ok(())
    }
}

/// Open, stamp, hash, and rewind one encoded source using a bounded buffer.
pub fn open_and_hash(
    path: &Path,
    should_cancel: impl Fn() -> bool,
) -> Result<VerifiedSourceFile, SourceProofError> {
    open_and_hash_inner(path, should_cancel, || {})
}

fn open_and_hash_inner(
    path: &Path,
    should_cancel: impl Fn() -> bool,
    mut on_hash: impl FnMut(),
) -> Result<VerifiedSourceFile, SourceProofError> {
    check_cancelled(path, &should_cancel)?;
    let file = File::open(path).map_err(|error| open_failure(path, error))?;
    let metadata = file.metadata().map_err(|error| io_failure(path, error))?;
    if !metadata.is_file() {
        return Err(SourceProofError::Changed {
            path: path.to_path_buf(),
            detail: String::from("source is not a regular file"),
        });
    }
    let mut verified = VerifiedSourceFile {
        stamp: SourceFileStamp::from_metadata(&metadata),
        path: path.to_path_buf(),
        file,
        proof: AudioSourceProof {
            sha256: String::new(),
            byte_len: 0,
        },
    };
    validate_path_stamp(path, verified.stamp, || false)?;
    on_hash();
    let proof = hash_open_file(&mut verified.file, path, &should_cancel)?;
    verified.proof = proof;
    validate_open_metadata(path, &verified.file, verified.stamp, &should_cancel)?;
    verified
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|error| io_failure(path, error))?;
    Ok(verified)
}

/// Hash a path's encoded bytes and return a strong source proof.
pub fn hash_file(
    path: &Path,
    should_cancel: impl Fn() -> bool,
) -> Result<AudioSourceProof, SourceProofError> {
    Ok(open_and_hash(path, should_cancel)?.proof().clone())
}

/// Alias that makes call sites explicit about the source-level identity.
#[allow(dead_code)]
pub fn hash_source_file(
    path: &Path,
    should_cancel: impl Fn() -> bool,
) -> Result<AudioSourceProof, SourceProofError> {
    hash_file(path, should_cancel)
}

/// Capture the current portable/filesystem stamp for a path.
#[allow(dead_code)]
pub fn stamp_file(path: &Path) -> Result<SourceFileStamp, SourceProofError> {
    let metadata = fs::metadata(path).map_err(|error| metadata_failure(path, error))?;
    if !metadata.is_file() {
        return Err(SourceProofError::Changed {
            path: path.to_path_buf(),
            detail: String::from("source is not a regular file"),
        });
    }
    Ok(SourceFileStamp::from_metadata(&metadata))
}

/// Validate the path against a previously captured stamp and proof.
#[allow(dead_code)]
pub fn validate_path(
    path: &Path,
    expected_stamp: SourceFileStamp,
    expected_proof: &AudioSourceProof,
    should_cancel: impl Fn() -> bool,
) -> Result<(), SourceProofError> {
    let mut verified = open_and_hash(path, should_cancel)?;
    if verified.stamp != expected_stamp {
        return Err(changed_stamp(path));
    }
    if verified.proof != *expected_proof {
        return Err(changed_digest(path));
    }
    verified
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|error| io_failure(path, error))?;
    Ok(())
}

/// Validate the current path against a previously captured filesystem stamp.
/// This intentionally does not read encoded bytes; callers that already hold
/// a proof ticket use it for O(1) commit/preflight fences.
pub fn validate_path_stamp(
    path: &Path,
    expected_stamp: SourceFileStamp,
    should_cancel: impl Fn() -> bool,
) -> Result<(), SourceProofError> {
    check_cancelled(path, &should_cancel)?;
    validate_path_stamp_inner(path, expected_stamp)
}

fn hash_open_file(
    file: &mut File,
    path: &Path,
    should_cancel: &impl Fn() -> bool,
) -> Result<AudioSourceProof, SourceProofError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| io_failure(path, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; HASH_BUFFER_SIZE];
    let mut byte_len = 0_u64;
    loop {
        check_cancelled(path, should_cancel)?;
        let read = file
            .read(&mut buffer)
            .map_err(|error| io_failure(path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        byte_len = byte_len
            .checked_add(read as u64)
            .ok_or_else(|| SourceProofError::Changed {
                path: path.to_path_buf(),
                detail: String::from("source length overflowed proof capacity"),
            })?;
    }
    check_cancelled(path, should_cancel)?;
    Ok(AudioSourceProof::from_digest(
        hasher.finalize().into(),
        byte_len,
    ))
}

fn validate_open_metadata(
    path: &Path,
    file: &File,
    expected_stamp: SourceFileStamp,
    should_cancel: &impl Fn() -> bool,
) -> Result<(), SourceProofError> {
    check_cancelled(path, should_cancel)?;
    let file_metadata = file.metadata().map_err(|error| io_failure(path, error))?;
    if !file_metadata.is_file() {
        return Err(SourceProofError::Changed {
            path: path.to_path_buf(),
            detail: String::from("opened source is no longer a regular file"),
        });
    }
    if SourceFileStamp::from_metadata(&file_metadata) != expected_stamp {
        return Err(changed_stamp(path));
    }
    validate_path_stamp_inner(path, expected_stamp)
}

fn validate_path_stamp_inner(
    path: &Path,
    expected_stamp: SourceFileStamp,
) -> Result<(), SourceProofError> {
    let metadata = fs::metadata(path).map_err(|error| metadata_failure(path, error))?;
    if !metadata.is_file() {
        return Err(SourceProofError::Changed {
            path: path.to_path_buf(),
            detail: String::from("source is not a regular file"),
        });
    }
    if SourceFileStamp::from_metadata(&metadata) != expected_stamp {
        return Err(changed_stamp(path));
    }
    Ok(())
}

fn changed_stamp(path: &Path) -> SourceProofError {
    SourceProofError::Changed {
        path: path.to_path_buf(),
        detail: String::from("filesystem stamp no longer matches preflight"),
    }
}

fn changed_ticket_stamp(path: &Path) -> SourceProofError {
    SourceProofError::Changed {
        path: path.to_path_buf(),
        detail: String::from("opened handle stamp no longer matches verified source"),
    }
}

#[allow(dead_code)]
fn changed_digest(path: &Path) -> SourceProofError {
    SourceProofError::Changed {
        path: path.to_path_buf(),
        detail: String::from("encoded byte digest no longer matches preflight"),
    }
}

fn check_cancelled(path: &Path, should_cancel: &impl Fn() -> bool) -> Result<(), SourceProofError> {
    if should_cancel() {
        Err(SourceProofError::Cancelled {
            path: path.to_path_buf(),
        })
    } else {
        Ok(())
    }
}

fn open_failure(path: &Path, error: io::Error) -> SourceProofError {
    if error.kind() == io::ErrorKind::NotFound {
        SourceProofError::Missing {
            path: path.to_path_buf(),
        }
    } else {
        io_failure(path, error)
    }
}

fn metadata_failure(path: &Path, error: io::Error) -> SourceProofError {
    if error.kind() == io::ErrorKind::NotFound {
        SourceProofError::Missing {
            path: path.to_path_buf(),
        }
    } else {
        io_failure(path, error)
    }
}

fn io_failure(path: &Path, error: io::Error) -> SourceProofError {
    SourceProofError::Io {
        path: path.to_path_buf(),
        kind: error.kind(),
        detail: error.to_string(),
    }
}

fn validate_digest(digest: &str) -> Result<(), String> {
    if digest.len() != SHA256_HEX_LENGTH
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(String::from(
            "sha256 must be exactly 64 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

fn hex_digest(digest: &[u8; 32]) -> String {
    let mut encoded = String::with_capacity(SHA256_HEX_LENGTH);
    for byte in digest {
        use fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(not(unix))]
fn system_time_nanos(time: io::Result<SystemTime>) -> i128 {
    match time {
        Ok(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX),
            Err(error) => -i128::try_from(error.duration().as_nanos()).unwrap_or(i128::MAX),
        },
        Err(_) => 0,
    }
}

#[cfg(unix)]
fn platform_stamp(metadata: &Metadata) -> SourceFileStamp {
    use std::os::unix::fs::MetadataExt;

    SourceFileStamp {
        dev: metadata.dev(),
        inode: metadata.ino(),
        len: metadata.len(),
        mtime_nanos: i128::from(metadata.mtime()) * 1_000_000_000
            + i128::from(metadata.mtime_nsec()),
        ctime_nanos: i128::from(metadata.ctime()) * 1_000_000_000
            + i128::from(metadata.ctime_nsec()),
    }
}

#[cfg(not(unix))]
fn platform_stamp(metadata: &Metadata) -> SourceFileStamp {
    SourceFileStamp {
        dev: 0,
        inode: 0,
        len: metadata.len(),
        mtime_nanos: system_time_nanos(metadata.modified()),
        ctime_nanos: system_time_nanos(metadata.created().or_else(|_| metadata.modified())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, OpenOptions},
        io::{Read, Write},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU32, Ordering},
        },
    };

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cadence-source-{name}-{}", std::process::id()))
    }

    #[test]
    fn proof_is_deterministic_and_serde_is_strict() {
        let path = temp_path("proof");
        fs::write(&path, b"encoded bytes").expect("fixture should write");
        let proof = hash_file(&path, || false).expect("hash should succeed");
        assert_eq!(proof.byte_len, 13);
        assert_eq!(
            proof.sha256,
            "7eee90892f592f39c45930a78428a4f5363e46fc37de88ee07deb5e22790a3cc"
        );
        assert_eq!(proof.sha256.len(), 64);
        assert!(
            proof
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        let json = serde_json::to_string(&proof).expect("proof should encode");
        assert_eq!(
            serde_json::from_str::<AudioSourceProof>(&json).expect("proof should decode"),
            proof
        );
        for digest in ["", &"A".repeat(64), &"g".repeat(64)] {
            let json = format!(r#"{{"sha256":"{digest}","byte_len":13}}"#);
            assert!(serde_json::from_str::<AudioSourceProof>(&json).is_err());
        }
        let _ = fs::remove_file(path);
    }

    #[test]
    fn hashing_uses_a_bounded_buffer_and_honors_cancellation() {
        let path = temp_path("bounded");
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .expect("fixture should open");
        file.write_all(&vec![7_u8; HASH_BUFFER_SIZE * 2 + 3])
            .expect("fixture should write");
        drop(file);
        let cancelled = Arc::new(AtomicBool::new(false));
        let first_check = Arc::clone(&cancelled);
        let checks = Arc::new(AtomicU32::new(0));
        let checks_for_callback = Arc::clone(&checks);
        let error = hash_file(&path, || {
            let count = checks_for_callback.fetch_add(1, Ordering::AcqRel) + 1;
            if count > 1 {
                first_check.store(true, Ordering::Release);
            }
            first_check.load(Ordering::Acquire)
        })
        .expect_err("cancellation should stop the bounded hash");
        assert!(matches!(error, SourceProofError::Cancelled { .. }));
        assert!(checks.load(Ordering::Acquire) >= 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn verified_ticket_hashes_once_then_uses_stamp_fences() {
        let path = temp_path("stamp-fence");
        fs::write(&path, b"original").expect("fixture should write");
        let mut hash_calls = 0;
        let mut verified =
            open_and_hash_inner(&path, || false, || hash_calls += 1).expect("source should hash");
        assert_eq!(hash_calls, 1);
        verified
            .validate_after_decode(|| false)
            .expect("unchanged source should pass its stamp fence");
        assert_eq!(hash_calls, 1);

        fs::write(&path, b"replaced").expect("same-size replacement should write");
        let error = verified
            .validate_after_decode(|| false)
            .expect_err("changed stamp must reject without another hash");
        assert!(matches!(error, SourceProofError::Changed { .. }));
        assert_eq!(hash_calls, 1);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn same_size_bytes_with_restored_metadata_are_rejected_by_digest() {
        let path = temp_path("restored-mtime");
        fs::write(&path, b"original").expect("fixture should write");
        let verified = open_and_hash(&path, || false).expect("initial hash should succeed");
        let original_stamp = verified.stamp();
        fs::write(&path, b"replaced").expect("replacement should write");
        let error = validate_path(&path, original_stamp, verified.proof(), || false)
            .expect_err("changed bytes must fail even when metadata could be restored");
        assert!(matches!(error, SourceProofError::Changed { .. }));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn open_for_ticket_rejects_replacement_before_transport_load() {
        let path = temp_path("ticket-replacement");
        fs::write(&path, b"original").expect("fixture should write");
        let verified = open_and_hash(&path, || false).expect("source should hash");
        let ticket = verified.ticket();
        drop(verified);

        // Keep the replacement the same size so this exercises the identity
        // fence rather than a decoder byte-length mismatch.
        fs::write(&path, b"replaced").expect("replacement should write");
        let error = open_for_ticket(&ticket).expect_err("replacement must not load");
        assert!(matches!(error, SourceProofError::Changed { .. }));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn open_for_ticket_keeps_the_verified_inode_after_atomic_replacement() {
        let path = temp_path("ticket-open-handle");
        let replacement = temp_path("ticket-open-handle-replacement");
        fs::write(&path, b"original").expect("fixture should write");
        let verified = open_and_hash(&path, || false).expect("source should hash");
        let ticket = verified.ticket();
        drop(verified);
        let mut opened = open_for_ticket(&ticket).expect("unchanged ticket should open");

        fs::write(&replacement, b"replacement").expect("replacement should write");
        fs::rename(&replacement, &path).expect("replacement should be atomic");

        let mut bytes = Vec::new();
        opened
            .read_to_end(&mut bytes)
            .expect("the already-open handle should remain readable");
        assert_eq!(bytes, b"original");
        assert!(matches!(
            open_for_ticket(&ticket),
            Err(SourceProofError::Changed { .. })
        ));
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(replacement);
    }
}

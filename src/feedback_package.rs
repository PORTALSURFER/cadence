//! Self-contained `.cadence-feedback` package codec (stored-mode ZIP + SHA-256 integrity).
//!
//! A feedback package is a standard uncompressed (STORE, method 0) ZIP archive containing:
//! - `feedback.json`: schema-versioned manifest with reviewer identity, optional summary,
//!   source audio SHA-256 proof, and timestamped comments.
//! - `audio/<filename>`: exact encoded bytes of the reviewed audio bounce.
//!
//! Security & integrity boundaries:
//! - Zero external archive dependencies; streaming 64 KiB I/O bounds memory usage.
//! - Rejects path traversal (`..`, absolute paths, backslashes, NUL bytes), duplicate entries,
//!   non-STORE entries, oversized payloads, CRC-32 mismatches, and SHA-256 digest mismatches.
//! - Never renames or mutates the caller's source audio file.

use crate::source::{self, AudioSourceProof};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, ErrorKind, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const FEEDBACK_PACKAGE_EXTENSION: &str = "cadence-feedback";
pub const SCHEMA_VERSION: u32 = 1;
const MANIFEST_ENTRY_NAME: &str = "feedback.json";
const AUDIO_ENTRY_PREFIX: &str = "audio/";
const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_AUDIO_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PACKAGE_ENTRIES: usize = 8;
const STREAM_BUFFER_BYTES: usize = 64 * 1024;

const SIG_LOCAL_FILE_HEADER: u32 = 0x0403_4b50;
const SIG_CENTRAL_DIR_HEADER: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const ZIP_VERSION_NEEDED: u16 = 20;
const ZIP_FLAG_UTF8: u16 = 0x0800;
const ZIP_METHOD_STORE: u16 = 0;

static NEXT_PACKAGE_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedNote {
    pub id: String,
    pub time_millis: u64,
    pub body: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub author: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackManifest {
    pub schema_version: u32,
    pub package_id: String,
    pub exported_at_unix_secs: u64,
    pub reviewer: String,
    #[serde(default)]
    pub summary: Option<String>,
    pub track_title: String,
    #[serde(default)]
    pub group_key: Option<String>,
    #[serde(default)]
    pub build: Option<u32>,
    pub build_label: String,
    pub audio_entry: String,
    pub original_name: String,
    pub source_proof: AudioSourceProof,
    pub notes: Vec<SharedNote>,
}

pub struct ExportPackageRequest<'a> {
    pub reviewer: &'a str,
    pub summary: Option<&'a str>,
    pub track_title: &'a str,
    pub group_key: Option<&'a str>,
    pub build: Option<u32>,
    pub build_label: &'a str,
    pub original_name: &'a str,
    pub source_path: &'a Path,
    pub expected_proof: Option<&'a AudioSourceProof>,
    pub notes: Vec<SharedNote>,
}

#[derive(Clone, Debug)]
struct CentralEntry {
    name: String,
    crc32: u32,
    size: u64,
    local_header_offset: u64,
}

#[derive(Clone, Debug)]
pub struct InspectedPackage {
    pub manifest: FeedbackManifest,
    audio_entry: CentralEntry,
}

pub fn is_feedback_package_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(FEEDBACK_PACKAGE_EXTENSION))
}

pub fn suggested_package_filename(track_title: &str, build_label: &str, reviewer: &str) -> String {
    let clean = |s: &str| {
        let mut out = String::new();
        let mut last_dash = false;
        for ch in s.chars() {
            if ch.is_ascii_alphanumeric() {
                out.push(ch);
                last_dash = false;
            } else if !last_dash && !out.is_empty() {
                out.push('-');
                last_dash = true;
            }
        }
        out.trim_matches('-').to_string()
    };
    let t = clean(track_title);
    let b = clean(build_label);
    let r = clean(reviewer);
    let stem = match (t.is_empty(), b.is_empty(), r.is_empty()) {
        (false, false, false) => format!("{t}_{b}_feedback-by-{r}"),
        (false, false, true) => format!("{t}_{b}_feedback"),
        (false, true, false) => format!("{t}_feedback-by-{r}"),
        (false, true, true) => format!("{t}_feedback"),
        (true, _, false) => format!("track_feedback-by-{r}"),
        (true, _, true) => String::from("track_feedback"),
    };
    format!("{stem}.{FEEDBACK_PACKAGE_EXTENSION}")
}

/// Export a self-contained `.cadence-feedback` archive bundling `feedback.json`
/// and the exact verified source audio bytes.
pub fn export_feedback_package(
    request: ExportPackageRequest<'_>,
    destination: &Path,
) -> Result<PathBuf, String> {
    let reviewer = request.reviewer.trim();
    if reviewer.is_empty() {
        return Err(String::from("Enter your name before exporting feedback."));
    }
    let destination = ensure_package_extension(destination);

    let verified = source::open_and_hash(request.source_path, || false)
        .map_err(|err| format!("Could not read source audio for export: {err}"))?;
    if let Some(expected) = request.expected_proof
        && verified.proof() != expected
    {
        return Err(String::from(
            "Audio source changed on disk; reload or relink it before exporting feedback.",
        ));
    }
    if verified.proof().byte_len > MAX_AUDIO_BYTES {
        return Err(format!(
            "Audio file exceeds the maximum feedback package size of {} MiB.",
            MAX_AUDIO_BYTES / (1024 * 1024)
        ));
    }

    let safe_audio_name = sanitize_audio_filename(request.original_name, request.source_path)?;
    let audio_entry = format!("{AUDIO_ENTRY_PREFIX}{safe_audio_name}");
    let summary = request
        .summary
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let nonce = NEXT_PACKAGE_NONCE.fetch_add(1, Ordering::Relaxed);
    let package_id = format!(
        "pkg-{}-{}-{nonce}",
        now_secs,
        &verified.proof().sha256[..12]
    );

    let manifest = FeedbackManifest {
        schema_version: SCHEMA_VERSION,
        package_id,
        exported_at_unix_secs: now_secs,
        reviewer: reviewer.to_owned(),
        summary,
        track_title: request.track_title.trim().to_owned(),
        group_key: request.group_key.map(str::to_owned),
        build: request.build,
        build_label: request.build_label.to_owned(),
        audio_entry: audio_entry.clone(),
        original_name: safe_audio_name,
        source_proof: verified.proof().clone(),
        notes: request.notes,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|err| format!("Could not encode feedback manifest: {err}"))?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(String::from("Feedback notes exceed maximum manifest size."));
    }

    // Compute audio CRC-32 and verify the open file handle still matches its proof length.
    let mut audio_file = verified
        .try_clone_for_decode()
        .map_err(|err| format!("Could not clone audio handle for packaging: {err}"))?;
    audio_file
        .seek(SeekFrom::Start(0))
        .map_err(|err| format!("Could not rewind audio file: {err}"))?;
    let (audio_crc32, audio_len) = crc32_and_len(&mut audio_file)
        .map_err(|err| format!("Could not checksum audio file: {err}"))?;
    if audio_len != verified.proof().byte_len {
        return Err(String::from(
            "Audio file changed while exporting feedback package.",
        ));
    }
    audio_file
        .seek(SeekFrom::Start(0))
        .map_err(|err| format!("Could not rewind audio file: {err}"))?;

    let parent = destination
        .parent()
        .ok_or_else(|| format!("No parent directory for {}", destination.display()))?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("Could not create {}: {err}", parent.display()))?;

    let (temp_path, mut out) = create_unique_temp_file(&destination)?;
    let write_res = (|| -> io::Result<()> {
        let mut records = Vec::with_capacity(2);

        // Entry 1: feedback.json
        let manifest_crc = crc32_bytes(&manifest_bytes);
        let manifest_offset = out.stream_position()?;
        write_local_header(
            &mut out,
            MANIFEST_ENTRY_NAME,
            manifest_crc,
            manifest_bytes.len() as u32,
        )?;
        out.write_all(&manifest_bytes)?;
        records.push(CentralEntry {
            name: MANIFEST_ENTRY_NAME.to_owned(),
            crc32: manifest_crc,
            size: manifest_bytes.len() as u64,
            local_header_offset: manifest_offset,
        });

        // Entry 2: audio/<filename>
        let audio_offset = out.stream_position()?;
        let audio_size_u32 = u32::try_from(audio_len).map_err(|_| {
            io::Error::new(
                ErrorKind::InvalidData,
                "audio file exceeds 32-bit ZIP entry limit",
            )
        })?;
        write_local_header(&mut out, &audio_entry, audio_crc32, audio_size_u32)?;
        copy_exact_streaming(&mut audio_file, &mut out, audio_len)?;
        records.push(CentralEntry {
            name: audio_entry,
            crc32: audio_crc32,
            size: audio_len,
            local_header_offset: audio_offset,
        });

        // Central Directory + EOCD
        let cd_offset = out.stream_position()?;
        for rec in &records {
            write_central_header(&mut out, rec)?;
        }
        let cd_end = out.stream_position()?;
        let cd_size = cd_end.saturating_sub(cd_offset);
        write_eocd(&mut out, records.len() as u16, cd_size as u32, cd_offset as u32)?;

        out.flush()?;
        out.sync_all()?;
        Ok(())
    })();

    drop(out);
    if let Err(err) = write_res {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "Could not write feedback package {}: {err}",
            destination.display()
        ));
    }
    if let Err(err) = fs::rename(&temp_path, &destination) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "Could not finalize feedback package {}: {err}",
            destination.display()
        ));
    }

    Ok(destination)
}

/// Read and strictly validate the manifest and audio entry metadata from a
/// `.cadence-feedback` archive.
pub fn inspect_feedback_package(package_path: &Path) -> Result<InspectedPackage, String> {
    let mut file = source::open_regular_file(package_path)
        .map_err(|err| format!("Could not open feedback package {}: {err}", package_path.display()))?;
    let entries = read_central_directory(&mut file)?;

    let manifest_entry = entries
        .iter()
        .find(|e| e.name == MANIFEST_ENTRY_NAME)
        .ok_or_else(|| String::from("Invalid feedback package: missing feedback.json."))?;
    if manifest_entry.size > MAX_MANIFEST_BYTES as u64 {
        return Err(String::from(
            "Invalid feedback package: feedback.json exceeds maximum supported size.",
        ));
    }

    let manifest_bytes = read_entry_bytes(&mut file, manifest_entry)?;
    let manifest: FeedbackManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|err| format!("Invalid feedback.json in package: {err}"))?;
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "Unsupported feedback package schema version {} (expected {SCHEMA_VERSION}).",
            manifest.schema_version
        ));
    }
    if manifest.reviewer.trim().is_empty() {
        return Err(String::from("Invalid feedback package: missing reviewer name."));
    }
    if manifest.package_id.trim().is_empty() {
        return Err(String::from("Invalid feedback package: missing package_id."));
    }
    manifest.source_proof.validate()?;
    validate_audio_entry_name(&manifest.audio_entry)?;

    let audio_entry = entries
        .iter()
        .find(|e| e.name == manifest.audio_entry)
        .cloned()
        .ok_or_else(|| {
            format!(
                "Invalid feedback package: bundled audio entry '{}' is missing.",
                manifest.audio_entry
            )
        })?;
    if audio_entry.size > MAX_AUDIO_BYTES {
        return Err(String::from(
            "Invalid feedback package: bundled audio exceeds maximum supported size.",
        ));
    }
    if audio_entry.size != manifest.source_proof.byte_len {
        return Err(String::from(
            "Invalid feedback package: bundled audio length does not match source proof.",
        ));
    }

    Ok(InspectedPackage {
        manifest,
        audio_entry,
    })
}

/// Verify the bundled audio CRC-32 and SHA-256 without extracting to disk
/// (used when the recipient already owns the matching bounce locally).
pub fn verify_bundled_audio(package_path: &Path, inspected: &InspectedPackage) -> Result<(), String> {
    let mut file = source::open_regular_file(package_path)
        .map_err(|err| format!("Could not open feedback package {}: {err}", package_path.display()))?;
    let data_offset = validate_local_header_and_data_offset(&mut file, &inspected.audio_entry)?;
    file.seek(SeekFrom::Start(data_offset))
        .map_err(|err| format!("Could not seek to bundled audio: {err}"))?;

    let mut hasher = Sha256::new();
    let mut crc = 0xffff_ffff_u32;
    let mut remaining = inspected.audio_entry.size;
    let mut buf = [0_u8; STREAM_BUFFER_BYTES];
    while remaining > 0 {
        let n = remaining.min(buf.len() as u64) as usize;
        file.read_exact(&mut buf[..n])
            .map_err(|err| format!("Could not read bundled audio: {err}"))?;
        crc = crc32_update(crc, &buf[..n]);
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    let actual_crc = !crc;
    if actual_crc != inspected.audio_entry.crc32 {
        return Err(String::from(
            "Feedback package audio is corrupt (CRC-32 mismatch).",
        ));
    }
    let actual_proof =
        AudioSourceProof::from_digest(hasher.finalize().into(), inspected.audio_entry.size);
    if actual_proof != inspected.manifest.source_proof {
        return Err(String::from(
            "Feedback package audio failed SHA-256 integrity verification.",
        ));
    }
    Ok(())
}

/// Extract and SHA-256-verify the bundled audio file into `dest_dir`.
/// Reuses an existing file at the destination path if its SHA-256 matches.
pub fn extract_verified_audio(
    package_path: &Path,
    inspected: &InspectedPackage,
    dest_dir: &Path,
) -> Result<PathBuf, String> {
    let safe_name = sanitize_audio_filename(
        &inspected.manifest.original_name,
        Path::new(&inspected.manifest.audio_entry),
    )?;
    fs::create_dir_all(dest_dir)
        .map_err(|err| format!("Could not create {}: {err}", dest_dir.display()))?;
    let target_name = format!(
        "{}-{safe_name}",
        &inspected.manifest.source_proof.sha256[..12]
    );
    let target_path = dest_dir.join(target_name);

    if target_path.is_file()
        && let Ok(existing_proof) = source::hash_file(&target_path, || false)
        && existing_proof == inspected.manifest.source_proof
    {
        verify_bundled_audio(package_path, inspected)?;
        return Ok(target_path);
    }

    let mut file = source::open_regular_file(package_path)
        .map_err(|err| format!("Could not open feedback package {}: {err}", package_path.display()))?;
    let data_offset = validate_local_header_and_data_offset(&mut file, &inspected.audio_entry)?;
    file.seek(SeekFrom::Start(data_offset))
        .map_err(|err| format!("Could not seek to bundled audio: {err}"))?;

    let (temp_path, mut out) = create_unique_temp_file(&target_path)?;
    let extract_res = (|| -> Result<(), String> {
        let mut hasher = Sha256::new();
        let mut crc = 0xffff_ffff_u32;
        let mut remaining = inspected.audio_entry.size;
        let mut buf = [0_u8; STREAM_BUFFER_BYTES];
        while remaining > 0 {
            let n = remaining.min(buf.len() as u64) as usize;
            file.read_exact(&mut buf[..n])
                .map_err(|err| format!("Could not read bundled audio: {err}"))?;
            crc = crc32_update(crc, &buf[..n]);
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])
                .map_err(|err| format!("Could not write extracted audio: {err}"))?;
            remaining -= n as u64;
        }
        out.flush()
            .map_err(|err| format!("Could not flush extracted audio: {err}"))?;
        out.sync_all()
            .map_err(|err| format!("Could not sync extracted audio: {err}"))?;

        let actual_crc = !crc;
        if actual_crc != inspected.audio_entry.crc32 {
            return Err(String::from(
                "Feedback package audio is corrupt (CRC-32 mismatch).",
            ));
        }
        let actual_proof =
            AudioSourceProof::from_digest(hasher.finalize().into(), inspected.audio_entry.size);
        if actual_proof != inspected.manifest.source_proof {
            return Err(String::from(
                "Feedback package audio failed SHA-256 integrity verification.",
            ));
        }
        Ok(())
    })();

    drop(out);
    if let Err(err) = extract_res {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }
    if let Err(err) = fs::rename(&temp_path, &target_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "Could not finalize extracted audio {}: {err}",
            target_path.display()
        ));
    }

    Ok(target_path)
}

fn ensure_package_extension(path: &Path) -> PathBuf {
    if is_feedback_package_path(path) {
        path.to_path_buf()
    } else {
        let mut os = path.as_os_str().to_os_string();
        os.push(".");
        os.push(FEEDBACK_PACKAGE_EXTENSION);
        PathBuf::from(os)
    }
}

fn sanitize_audio_filename(original_name: &str, fallback_path: &Path) -> Result<String, String> {
    let raw = Path::new(original_name)
        .file_name()
        .and_then(|n| n.to_str())
        .or_else(|| fallback_path.file_name().and_then(|n| n.to_str()))
        .ok_or_else(|| String::from("Invalid audio filename."))?;
    if raw.is_empty()
        || raw == "."
        || raw == ".."
        || raw.contains('/')
        || raw.contains('\\')
        || raw.contains('\0')
    {
        return Err(format!("Unsafe audio filename in feedback package: {raw:?}"));
    }
    crate::storage::validate_audio_path(Path::new(raw))?;
    Ok(raw.to_owned())
}

fn validate_entry_path(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err(format!("Unsafe archive entry path: {name:?}"));
    }
    for segment in name.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!("Unsafe archive entry path: {name:?}"));
        }
    }
    Ok(())
}

fn validate_audio_entry_name(entry: &str) -> Result<(), String> {
    validate_entry_path(entry)?;
    let filename = entry
        .strip_prefix(AUDIO_ENTRY_PREFIX)
        .ok_or_else(|| format!("Bundled audio entry must live under '{AUDIO_ENTRY_PREFIX}'"))?;
    if filename.contains('/') {
        return Err(String::from(
            "Bundled audio entry must not contain nested directories.",
        ));
    }
    crate::storage::validate_audio_path(Path::new(filename))
}

fn write_local_header(out: &mut File, name: &str, crc32: u32, size: u32) -> io::Result<()> {
    let name_bytes = name.as_bytes();
    out.write_all(&SIG_LOCAL_FILE_HEADER.to_le_bytes())?;
    out.write_all(&ZIP_VERSION_NEEDED.to_le_bytes())?;
    out.write_all(&ZIP_FLAG_UTF8.to_le_bytes())?;
    out.write_all(&ZIP_METHOD_STORE.to_le_bytes())?;
    out.write_all(&0_u16.to_le_bytes())?; // mod time
    out.write_all(&0_u16.to_le_bytes())?; // mod date
    out.write_all(&crc32.to_le_bytes())?;
    out.write_all(&size.to_le_bytes())?; // compressed size
    out.write_all(&size.to_le_bytes())?; // uncompressed size
    out.write_all(&(name_bytes.len() as u16).to_le_bytes())?;
    out.write_all(&0_u16.to_le_bytes())?; // extra len
    out.write_all(name_bytes)?;
    Ok(())
}

fn write_central_header(out: &mut File, entry: &CentralEntry) -> io::Result<()> {
    let name_bytes = entry.name.as_bytes();
    let size_u32 = u32::try_from(entry.size).map_err(|_| {
        io::Error::new(ErrorKind::InvalidData, "entry size exceeds 32-bit ZIP limit")
    })?;
    let offset_u32 = u32::try_from(entry.local_header_offset).map_err(|_| {
        io::Error::new(
            ErrorKind::InvalidData,
            "entry offset exceeds 32-bit ZIP limit",
        )
    })?;
    out.write_all(&SIG_CENTRAL_DIR_HEADER.to_le_bytes())?;
    out.write_all(&ZIP_VERSION_NEEDED.to_le_bytes())?; // version made by
    out.write_all(&ZIP_VERSION_NEEDED.to_le_bytes())?; // version needed
    out.write_all(&ZIP_FLAG_UTF8.to_le_bytes())?;
    out.write_all(&ZIP_METHOD_STORE.to_le_bytes())?;
    out.write_all(&0_u16.to_le_bytes())?; // mod time
    out.write_all(&0_u16.to_le_bytes())?; // mod date
    out.write_all(&entry.crc32.to_le_bytes())?;
    out.write_all(&size_u32.to_le_bytes())?;
    out.write_all(&size_u32.to_le_bytes())?;
    out.write_all(&(name_bytes.len() as u16).to_le_bytes())?;
    out.write_all(&0_u16.to_le_bytes())?; // extra len
    out.write_all(&0_u16.to_le_bytes())?; // comment len
    out.write_all(&0_u16.to_le_bytes())?; // disk number
    out.write_all(&0_u16.to_le_bytes())?; // internal attrs
    out.write_all(&0_u32.to_le_bytes())?; // external attrs
    out.write_all(&offset_u32.to_le_bytes())?;
    out.write_all(name_bytes)?;
    Ok(())
}

fn write_eocd(out: &mut File, entry_count: u16, cd_size: u32, cd_offset: u32) -> io::Result<()> {
    out.write_all(&SIG_EOCD.to_le_bytes())?;
    out.write_all(&0_u16.to_le_bytes())?; // disk number
    out.write_all(&0_u16.to_le_bytes())?; // cd start disk
    out.write_all(&entry_count.to_le_bytes())?;
    out.write_all(&entry_count.to_le_bytes())?;
    out.write_all(&cd_size.to_le_bytes())?;
    out.write_all(&cd_offset.to_le_bytes())?;
    out.write_all(&0_u16.to_le_bytes())?; // comment len
    Ok(())
}

fn read_central_directory(file: &mut File) -> Result<Vec<CentralEntry>, String> {
    let file_len = file
        .metadata()
        .map_err(|err| format!("Could not inspect feedback package: {err}"))?
        .len();
    if file_len < 22 {
        return Err(String::from(
            "Invalid feedback package: file is too small to be a ZIP archive.",
        ));
    }
    let tail_len = file_len.min(65_557) as usize;
    let tail_start = file_len - tail_len as u64;
    file.seek(SeekFrom::Start(tail_start))
        .map_err(|err| format!("Could not read feedback package trailer: {err}"))?;
    let mut tail = vec![0_u8; tail_len];
    file.read_exact(&mut tail)
        .map_err(|err| format!("Could not read feedback package trailer: {err}"))?;

    let eocd_sig = SIG_EOCD.to_le_bytes();
    let eocd_pos = (0..=tail.len() - 22)
        .rev()
        .find(|&pos| {
            tail[pos..pos + 4] == eocd_sig && {
                let comment_len = u16::from_le_bytes([tail[pos + 20], tail[pos + 21]]) as usize;
                pos + 22 + comment_len == tail.len()
            }
        })
        .ok_or_else(|| String::from("Invalid feedback package: missing ZIP central directory."))?;

    let eocd = &tail[eocd_pos..];
    let disk_num = u16::from_le_bytes([eocd[4], eocd[5]]);
    let cd_disk = u16::from_le_bytes([eocd[6], eocd[7]]);
    if disk_num != 0 || cd_disk != 0 {
        return Err(String::from(
            "Unsupported multi-disk feedback package archive.",
        ));
    }
    let total_entries = u16::from_le_bytes([eocd[10], eocd[11]]) as usize;
    if total_entries == 0 || total_entries > MAX_PACKAGE_ENTRIES {
        return Err(format!(
            "Invalid feedback package: unexpected entry count ({total_entries})."
        ));
    }
    let cd_size = u32::from_le_bytes([eocd[12], eocd[13], eocd[14], eocd[15]]) as u64;
    let cd_offset = u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]) as u64;
    let eocd_abs = tail_start + eocd_pos as u64;
    if cd_offset.saturating_add(cd_size) > eocd_abs || cd_size > 1024 * 1024 {
        return Err(String::from(
            "Invalid feedback package: central directory bounds are corrupt.",
        ));
    }

    file.seek(SeekFrom::Start(cd_offset))
        .map_err(|err| format!("Could not seek to central directory: {err}"))?;
    let mut cd = vec![0_u8; cd_size as usize];
    file.read_exact(&mut cd)
        .map_err(|err| format!("Could not read central directory: {err}"))?;

    let mut entries = Vec::with_capacity(total_entries);
    let mut seen = HashSet::with_capacity(total_entries);
    let mut pos = 0_usize;
    for _ in 0..total_entries {
        if pos + 46 > cd.len() {
            return Err(String::from(
                "Invalid feedback package: truncated central directory entry.",
            ));
        }
        let sig = u32::from_le_bytes([cd[pos], cd[pos + 1], cd[pos + 2], cd[pos + 3]]);
        if sig != SIG_CENTRAL_DIR_HEADER {
            return Err(String::from(
                "Invalid feedback package: bad central directory header signature.",
            ));
        }
        let method = u16::from_le_bytes([cd[pos + 10], cd[pos + 11]]);
        if method != ZIP_METHOD_STORE {
            return Err(format!(
                "Unsupported compression method ({method}) in feedback package."
            ));
        }
        let crc32 = u32::from_le_bytes([cd[pos + 16], cd[pos + 17], cd[pos + 18], cd[pos + 19]]);
        let comp_size =
            u32::from_le_bytes([cd[pos + 20], cd[pos + 21], cd[pos + 22], cd[pos + 23]]) as u64;
        let uncomp_size =
            u32::from_le_bytes([cd[pos + 24], cd[pos + 25], cd[pos + 26], cd[pos + 27]]) as u64;
        if comp_size != uncomp_size {
            return Err(String::from(
                "Invalid feedback package: stored entry size mismatch.",
            ));
        }
        let name_len = u16::from_le_bytes([cd[pos + 28], cd[pos + 29]]) as usize;
        let extra_len = u16::from_le_bytes([cd[pos + 30], cd[pos + 31]]) as usize;
        let comment_len = u16::from_le_bytes([cd[pos + 32], cd[pos + 33]]) as usize;
        let local_offset =
            u32::from_le_bytes([cd[pos + 42], cd[pos + 43], cd[pos + 44], cd[pos + 45]]) as u64;

        let name_start = pos + 46;
        let name_end = name_start + name_len;
        let next_pos = name_end + extra_len + comment_len;
        if next_pos > cd.len() {
            return Err(String::from(
                "Invalid feedback package: truncated entry metadata.",
            ));
        }
        let name = std::str::from_utf8(&cd[name_start..name_end])
            .map_err(|_| String::from("Invalid feedback package: non-UTF-8 entry name."))?
            .to_owned();
        validate_entry_path(&name)?;
        if !seen.insert(name.clone()) {
            return Err(format!("Invalid feedback package: duplicate entry {name:?}."));
        }
        if local_offset >= cd_offset {
            return Err(String::from(
                "Invalid feedback package: entry offset overlaps central directory.",
            ));
        }
        entries.push(CentralEntry {
            name,
            crc32,
            size: uncomp_size,
            local_header_offset: local_offset,
        });
        pos = next_pos;
    }

    Ok(entries)
}

fn validate_local_header_and_data_offset(
    file: &mut File,
    entry: &CentralEntry,
) -> Result<u64, String> {
    let file_len = file
        .metadata()
        .map_err(|err| format!("Could not inspect feedback package: {err}"))?
        .len();
    file.seek(SeekFrom::Start(entry.local_header_offset))
        .map_err(|err| format!("Could not seek to entry '{}': {err}", entry.name))?;
    let mut header = [0_u8; 30];
    file.read_exact(&mut header)
        .map_err(|err| format!("Could not read entry header '{}': {err}", entry.name))?;
    let sig = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    if sig != SIG_LOCAL_FILE_HEADER {
        return Err(String::from(
            "Invalid feedback package: bad local file header signature.",
        ));
    }
    let method = u16::from_le_bytes([header[8], header[9]]);
    if method != ZIP_METHOD_STORE {
        return Err(format!(
            "Unsupported compression method ({method}) for entry '{}'.",
            entry.name
        ));
    }
    let name_len = u16::from_le_bytes([header[26], header[27]]) as usize;
    let extra_len = u16::from_le_bytes([header[28], header[29]]) as u64;
    let mut local_name = vec![0_u8; name_len];
    file.read_exact(&mut local_name)
        .map_err(|err| format!("Could not read entry name '{}': {err}", entry.name))?;
    if local_name != entry.name.as_bytes() {
        return Err(String::from(
            "Invalid feedback package: local header name disagrees with central directory.",
        ));
    }
    let data_offset = entry
        .local_header_offset
        .checked_add(30)
        .and_then(|v| v.checked_add(name_len as u64))
        .and_then(|v| v.checked_add(extra_len))
        .ok_or_else(|| String::from("Invalid feedback package: entry offset overflow."))?;
    if data_offset.saturating_add(entry.size) > file_len {
        return Err(String::from(
            "Invalid feedback package: entry payload extends past end of file.",
        ));
    }
    Ok(data_offset)
}

fn read_entry_bytes(file: &mut File, entry: &CentralEntry) -> Result<Vec<u8>, String> {
    let data_offset = validate_local_header_and_data_offset(file, entry)?;
    file.seek(SeekFrom::Start(data_offset))
        .map_err(|err| format!("Could not seek to entry '{}': {err}", entry.name))?;
    let mut bytes = vec![0_u8; entry.size as usize];
    file.read_exact(&mut bytes)
        .map_err(|err| format!("Could not read entry '{}': {err}", entry.name))?;
    if crc32_bytes(&bytes) != entry.crc32 {
        return Err(format!(
            "Invalid feedback package: CRC-32 mismatch for '{}'.",
            entry.name
        ));
    }
    Ok(bytes)
}

fn create_unique_temp_file(destination: &Path) -> Result<(PathBuf, File), String> {
    let name = destination
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("package");
    let pid = std::process::id();
    for _ in 0..128 {
        let nonce = NEXT_PACKAGE_NONCE.fetch_add(1, Ordering::Relaxed);
        let temp = destination.with_file_name(format!(".{name}.tmp-{pid}-{nonce}"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(format!("Could not create temp file {}: {err}", temp.display()));
            }
        }
    }
    Err(String::from("Could not allocate unique temporary file."))
}

fn copy_exact_streaming(src: &mut File, dst: &mut File, mut remaining: u64) -> io::Result<()> {
    let mut buf = [0_u8; STREAM_BUFFER_BYTES];
    while remaining > 0 {
        let n = remaining.min(buf.len() as u64) as usize;
        src.read_exact(&mut buf[..n])?;
        dst.write_all(&buf[..n])?;
        remaining -= n as u64;
    }
    Ok(())
}

fn crc32_and_len(src: &mut File) -> io::Result<(u32, u64)> {
    let mut crc = 0xffff_ffff_u32;
    let mut total = 0_u64;
    let mut buf = [0_u8; STREAM_BUFFER_BYTES];
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        crc = crc32_update(crc, &buf[..n]);
        total += n as u64;
    }
    Ok((!crc, total))
}

fn crc32_bytes(data: &[u8]) -> u32 {
    !crc32_update(0xffff_ffff_u32, data)
}

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        let idx = ((crc ^ u32::from(byte)) & 0xff) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[idx];
    }
    crc
}

const CRC32_TABLE: [u32; 256] = {
    let mut table = [0_u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut j = 0;
        while j < 8 {
            if c & 1 != 0 {
                c = 0xedb8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            j += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let nonce = NEXT_PACKAGE_NONCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "cadence-feedback-pkg-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tiny_wav(amplitude: i16) -> Vec<u8> {
        let samples = vec![amplitude; 64];
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
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn export_inspect_verify_and_extract_round_trip_without_modifying_source() {
        let dir = test_dir("roundtrip");
        let audio_path = dir.join("A1_b2__night-drive.wav");
        let audio_bytes = tiny_wav(1500);
        fs::write(&audio_path, &audio_bytes).unwrap();
        let proof = source::hash_file(&audio_path, || false).unwrap();

        let dest = dir.join("shared/review");
        let exported = export_feedback_package(
            ExportPackageRequest {
                reviewer: "Wanja",
                summary: Some("Groove hits hard; trim intro 4 bars"),
                track_title: "Night Drive",
                group_key: Some("A1"),
                build: Some(2),
                build_label: "b2",
                original_name: "A1_b2__night-drive.wav",
                source_path: &audio_path,
                expected_proof: Some(&proof),
                notes: vec![SharedNote {
                    id: "n1".into(),
                    time_millis: 12_340,
                    body: "Bring hats up 1 dB".into(),
                    done: false,
                    author: None,
                }],
            },
            &dest,
        )
        .expect("export should succeed");

        assert!(is_feedback_package_path(&exported));
        assert!(exported.is_file());
        assert_eq!(fs::read(&audio_path).unwrap(), audio_bytes);

        let inspected = inspect_feedback_package(&exported).expect("inspect should succeed");
        assert_eq!(inspected.manifest.schema_version, SCHEMA_VERSION);
        assert_eq!(inspected.manifest.reviewer, "Wanja");
        assert_eq!(
            inspected.manifest.summary.as_deref(),
            Some("Groove hits hard; trim intro 4 bars")
        );
        assert_eq!(inspected.manifest.source_proof, proof);
        assert_eq!(inspected.manifest.notes.len(), 1);
        assert_eq!(inspected.manifest.notes[0].time_millis, 12_340);

        verify_bundled_audio(&exported, &inspected).expect("bundled audio should verify");

        let extract_dir = dir.join("extracted");
        let extracted =
            extract_verified_audio(&exported, &inspected, &extract_dir).expect("extract audio");
        assert_eq!(fs::read(&extracted).unwrap(), audio_bytes);
        // Second extraction reuses the identical verified file idempotently.
        let extracted_again =
            extract_verified_audio(&exported, &inspected, &extract_dir).expect("re-extract audio");
        assert_eq!(extracted_again, extracted);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_audio_bytes_and_unsafe_paths_fail_closed() {
        let dir = test_dir("corrupt");
        let audio_path = dir.join("song.wav");
        fs::write(&audio_path, tiny_wav(2200)).unwrap();
        let pkg = export_feedback_package(
            ExportPackageRequest {
                reviewer: "Dylan",
                summary: None,
                track_title: "Song",
                group_key: None,
                build: None,
                build_label: "v1",
                original_name: "song.wav",
                source_path: &audio_path,
                expected_proof: None,
                notes: Vec::new(),
            },
            &dir.join("song.cadence-feedback"),
        )
        .unwrap();

        // Flip a byte inside the bundled audio payload.
        let mut raw = fs::read(&pkg).unwrap();
        let data_pos = raw.windows(4).position(|w| w == b"RIFF").unwrap();
        raw[data_pos + 40] ^= 0xff;
        fs::write(&pkg, &raw).unwrap();

        let inspected = inspect_feedback_package(&pkg).unwrap();
        assert!(verify_bundled_audio(&pkg, &inspected).is_err());
        assert!(extract_verified_audio(&pkg, &inspected, &dir.join("out")).is_err());

        // Path traversal validation fails closed.
        assert!(validate_entry_path("../evil.wav").is_err());
        assert!(validate_entry_path("/etc/passwd").is_err());
        assert!(validate_audio_entry_name("audio/../escape.wav").is_err());
        assert!(validate_audio_entry_name("audio/sub/song.wav").is_err());
        assert!(validate_audio_entry_name("other/song.wav").is_err());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn suggested_filename_sanitizes_and_uses_extension() {
        assert_eq!(
            suggested_package_filename("Night Drive!", "b2", "Wanja S."),
            "Night-Drive_b2_feedback-by-Wanja-S.cadence-feedback"
        );
    }
}

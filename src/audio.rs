//! Background audio inspection for the native review surface.
//!
//! This module owns only bounded waveform analysis data. Native audition
//! playback is kept in the separate host-controlled transport module; this
//! decoder never performs output-device work on the Radiant UI path.

use ebur128::{Channel as LoudnessChannel, EbuR128, Mode};
use radiant::runtime::{GpuSignalSummary, GpuSignalSummaryBucket, GpuSignalSummaryLevel};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use symphonia::core::{
    audio::{Channels, SampleBuffer},
    codecs::{CODEC_TYPE_NULL, DecoderOptions},
    errors::Error,
    formats::FormatOptions,
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
};

const PEAK_WINDOW_FRAMES: usize = 1024;
const MAX_DISPLAY_BUCKETS: usize = 4096;
const SUMMARY_BAND_COUNT: usize = 2;
// Publish the first completed (or currently filling) peak window as soon as
// the decoder has data. The UI clips this prefix to its decoded extent.
const PREVIEW_FIRST_MILLIS: u64 = 1;
const PREVIEW_INTERVAL_MILLIS: u64 = 250;
const LOUDNESS_PROFILE_STEP_MILLIS: u64 = 100;
const MAX_LOUDNESS_PROFILE_POINTS: usize = 8192;
pub const MAX_LOUDNESS_MATCH_DB: f32 = 24.0;

#[derive(Clone, Debug, PartialEq)]
pub struct WaveformData {
    pub sample_rate: u32,
    pub channels: usize,
    pub duration_millis: u64,
    pub render_frames: usize,
    pub integrated_lufs: Option<f32>,
    pub loudness_profile: Arc<[LoudnessPoint]>,
    pub summary: Arc<GpuSignalSummary>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaveformProgress {
    pub waveform: WaveformData,
    /// A preview extent is only publishable when the source declares its
    /// total frame count. Unknown-duration streams stay final-only so a
    /// decoded prefix cannot be stretched across an invented timeline.
    pub progress: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoudnessPoint {
    pub end_frame: u64,
    pub lufs: f32,
}

const WAVEFORM_CACHE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaveformCacheFingerprint {
    size: u64,
    modified_seconds: u64,
    modified_nanos: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedWaveform {
    version: u32,
    source_path: PathBuf,
    source: WaveformCacheFingerprint,
    sample_rate: u32,
    channels: usize,
    duration_millis: u64,
    render_frames: usize,
    integrated_lufs: Option<f32>,
    loudness_profile: Vec<LoudnessPoint>,
    summary: CachedSummary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedSummary {
    frames: usize,
    band_count: usize,
    levels: Vec<CachedSummaryLevel>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedSummaryLevel {
    bucket_frames: usize,
    buckets: Vec<CachedSummaryBucket>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct CachedSummaryBucket {
    min: f32,
    max: f32,
}

/// Load a decoded waveform when the cache entry still describes the current
/// source file. Cache failures are treated as misses so a corrupt or old
/// entry never prevents the source from being decoded again.
pub fn load_waveform_cache(path: &Path, cache_path: &Path) -> Option<WaveformData> {
    let source = waveform_cache_fingerprint(path)?;
    let contents = fs::read(cache_path).ok()?;
    let cached = serde_json::from_slice::<CachedWaveform>(&contents).ok()?;
    if waveform_cache_fingerprint(path) != Some(source) {
        return None;
    }
    cached.into_waveform(path, source)
}

pub fn waveform_cache_fingerprint(path: &Path) -> Option<WaveformCacheFingerprint> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(WaveformCacheFingerprint {
        size: metadata.len(),
        modified_seconds: modified.as_secs(),
        modified_nanos: modified.subsec_nanos(),
    })
}

pub fn write_waveform_cache_if_unchanged(
    path: &Path,
    cache_path: &Path,
    source: WaveformCacheFingerprint,
    waveform: &WaveformData,
) -> Result<(), String> {
    if waveform_cache_fingerprint(path) != Some(source) {
        return Err(format!(
            "Source changed while decoding {}; skipping waveform cache",
            path.display()
        ));
    }
    let cached = CachedWaveform::from_waveform(path, source, waveform);
    let encoded = serde_json::to_vec(&cached)
        .map_err(|error| format!("Could not encode waveform cache: {error}"))?;
    let directory = cache_path
        .parent()
        .ok_or_else(|| format!("No parent directory for {}", cache_path.display()))?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;

    let file_name = cache_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("waveform.json");
    let temporary_path = cache_path.with_file_name(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        unique_timestamp()
    ));
    if let Err(error) = fs::write(&temporary_path, encoded) {
        let _ = fs::remove_file(&temporary_path);
        return Err(format!(
            "Could not write {}: {error}",
            temporary_path.display()
        ));
    }
    if waveform_cache_fingerprint(path) != Some(source) {
        let _ = fs::remove_file(&temporary_path);
        return Err(format!(
            "Source changed while writing {}; skipping waveform cache",
            path.display()
        ));
    }
    if let Err(error) = fs::rename(&temporary_path, cache_path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(format!(
            "Could not replace waveform cache {}: {error}",
            cache_path.display()
        ));
    }
    Ok(())
}

fn unique_timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

impl CachedWaveform {
    fn from_waveform(
        path: &Path,
        source: WaveformCacheFingerprint,
        waveform: &WaveformData,
    ) -> Self {
        Self {
            version: WAVEFORM_CACHE_VERSION,
            source_path: path.to_path_buf(),
            source,
            sample_rate: waveform.sample_rate,
            channels: waveform.channels,
            duration_millis: waveform.duration_millis,
            render_frames: waveform.render_frames,
            integrated_lufs: waveform.integrated_lufs,
            loudness_profile: waveform.loudness_profile.to_vec(),
            summary: CachedSummary {
                frames: waveform.summary.frames,
                band_count: waveform.summary.band_count,
                levels: waveform
                    .summary
                    .levels
                    .iter()
                    .map(|level| CachedSummaryLevel {
                        bucket_frames: level.bucket_frames,
                        buckets: level
                            .buckets
                            .iter()
                            .map(|bucket| CachedSummaryBucket {
                                min: bucket.min,
                                max: bucket.max,
                            })
                            .collect(),
                    })
                    .collect(),
            },
        }
    }

    fn into_waveform(self, path: &Path, source: WaveformCacheFingerprint) -> Option<WaveformData> {
        if self.version != WAVEFORM_CACHE_VERSION
            || self.source_path != path
            || self.source != source
            || self.sample_rate == 0
            || self.channels == 0
            || self.render_frames == 0
            || self.summary.frames != self.render_frames
            || self.summary.band_count == 0
            || self.summary.levels.is_empty()
            || self.integrated_lufs.is_some_and(|value| !value.is_finite())
            || self
                .loudness_profile
                .iter()
                .any(|point| !point.lufs.is_finite())
        {
            return None;
        }

        let CachedSummary {
            frames,
            band_count,
            levels: cached_levels,
        } = self.summary;
        let levels = cached_levels
            .into_iter()
            .map(|level| {
                if level.bucket_frames == 0 {
                    return None;
                }
                let expected_frames = frames.div_ceil(level.bucket_frames);
                let expected_buckets = expected_frames.checked_mul(band_count)?;
                if level.buckets.len() != expected_buckets
                    || level.buckets.iter().any(|bucket| {
                        !bucket.min.is_finite()
                            || !bucket.max.is_finite()
                            || bucket.min > bucket.max
                            || bucket.min < -1.0
                            || bucket.max > 1.0
                    })
                {
                    return None;
                }
                Some(GpuSignalSummaryLevel {
                    bucket_frames: level.bucket_frames,
                    buckets: Arc::from(
                        level
                            .buckets
                            .into_iter()
                            .map(|bucket| GpuSignalSummaryBucket {
                                min: bucket.min,
                                max: bucket.max,
                            })
                            .collect::<Vec<_>>()
                            .into_boxed_slice(),
                    ),
                })
            })
            .collect::<Option<Vec<_>>>()?;

        Some(WaveformData {
            sample_rate: self.sample_rate,
            channels: self.channels,
            duration_millis: self.duration_millis,
            render_frames: self.render_frames,
            integrated_lufs: self.integrated_lufs,
            loudness_profile: Arc::from(self.loudness_profile.into_boxed_slice()),
            summary: Arc::new(GpuSignalSummary {
                frames,
                band_count,
                levels,
            }),
        })
    }
}

/// Return the playback gain, in decibels, needed to bring `reference_lufs` to
/// the imported track's `target_lufs`. The bound keeps a quiet or malformed
/// file from requesting an unsafe unbounded boost.
pub fn loudness_match_gain_db(
    target_lufs: Option<f32>,
    reference_lufs: Option<f32>,
) -> Option<f32> {
    let target = target_lufs.filter(|value| value.is_finite())?;
    let reference = reference_lufs.filter(|value| value.is_finite())?;
    Some((target - reference).clamp(-MAX_LOUDNESS_MATCH_DB, MAX_LOUDNESS_MATCH_DB))
}

pub fn linear_gain_for_db(gain_db: f32) -> f32 {
    10.0_f32.powf(gain_db.clamp(-MAX_LOUDNESS_MATCH_DB, MAX_LOUDNESS_MATCH_DB) / 20.0)
}

/// Return the raw-audio momentary loudness at a playback position.
///
/// The decoded profile is sampled at the analyzer's 100 ms hop and begins
/// after the analyzer's initial 400 ms momentary window. Values outside that
/// range use the nearest profile point; values between points are interpolated
/// so the UI meter can move smoothly without depending on the audition gain.
pub fn loudness_at_position(waveform: &WaveformData, position_millis: u64) -> Option<f32> {
    let fallback = || waveform.integrated_lufs.filter(|value| value.is_finite());
    let Some(first) = waveform.loudness_profile.first() else {
        return fallback();
    };
    let position_frame = ((position_millis as u128 * waveform.sample_rate as u128) / 1_000)
        .min(u64::MAX as u128) as u64;
    let upper_index = waveform
        .loudness_profile
        .partition_point(|point| point.end_frame < position_frame);
    let value = match upper_index {
        0 => first.lufs,
        index if index >= waveform.loudness_profile.len() => waveform.loudness_profile.last()?.lufs,
        index => {
            let lower = waveform.loudness_profile[index - 1];
            let upper = waveform.loudness_profile[index];
            let span = upper.end_frame.saturating_sub(lower.end_frame);
            if span == 0 {
                upper.lufs
            } else {
                let fraction = (position_frame.saturating_sub(lower.end_frame) as f32
                    / span as f32)
                    .clamp(0.0, 1.0);
                lower.lufs + (upper.lufs - lower.lufs) * fraction
            }
        }
    };
    value.is_finite().then_some(value).or_else(fallback)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PeakMeasurement {
    min: f32,
    max: f32,
    squared_energy: f64,
    frames: usize,
}

impl PeakMeasurement {
    fn silence() -> Self {
        Self {
            min: 0.0,
            max: 0.0,
            squared_energy: 0.0,
            frames: 0,
        }
    }

    #[cfg(test)]
    fn rms(self) -> f32 {
        if self.frames == 0 {
            return 0.0;
        }
        (self.squared_energy / self.frames as f64).max(0.0).sqrt() as f32
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PeakWindow {
    min: f32,
    max: f32,
    squared_sum: f64,
    frames: usize,
}

impl PeakWindow {
    fn new() -> Self {
        Self {
            min: 1.0,
            max: -1.0,
            squared_sum: 0.0,
            frames: 0,
        }
    }

    /// Retain channel extrema for one source frame instead of downmixing it.
    /// A waveform should not erase anti-phase stereo or understate a
    /// hard-panned channel just because the audible channels average out.
    fn add_frame(&mut self, frame: &[f32]) {
        if frame.is_empty() {
            return;
        }
        let mut frame_squared_sum = 0.0_f64;
        for &sample in frame {
            let sample = sample.clamp(-1.0, 1.0);
            self.min = self.min.min(sample);
            self.max = self.max.max(sample);
            let sample = f64::from(sample);
            frame_squared_sum += sample * sample;
        }
        self.squared_sum += frame_squared_sum / frame.len() as f64;
        self.frames = self.frames.saturating_add(1);
    }

    fn finish(self) -> Option<PeakMeasurement> {
        (self.frames > 0).then_some(PeakMeasurement {
            min: self.min,
            max: self.max,
            squared_energy: self.squared_sum,
            frames: self.frames,
        })
    }

    fn snapshot(&self) -> Option<PeakMeasurement> {
        self.finish()
    }
}

/// Bounded BS.1770 integrated loudness analysis for the review meter.
///
/// `ebur128` owns the K-weighting filters, 400 ms blocks with the required
/// 100 ms hop, absolute/relative gating, and per-channel weighting. Histogram
/// mode keeps the integrated history bounded without retaining every block.
#[derive(Debug)]
struct LoudnessAccumulator {
    analyzer: EbuR128,
    channels: usize,
    frames: u64,
    profile_step_frames: usize,
    next_profile_frame: u64,
    profile: Vec<LoudnessPoint>,
}

impl LoudnessAccumulator {
    fn new(sample_rate: u32, channel_layout: Channels, channels: usize) -> Result<Self, String> {
        let channel_map = loudness_channel_map(channel_layout, channels);
        let mut analyzer = EbuR128::new(
            channel_map.len().try_into().unwrap_or(u32::MAX),
            sample_rate,
            Mode::I | Mode::M | Mode::HISTOGRAM,
        )
        .map_err(|error| format!("Could not initialize LUFS analyzer: {error}"))?;
        analyzer
            .set_channel_map(&channel_map)
            .map_err(|error| format!("Could not configure LUFS channel map: {error}"))?;

        let profile_step_frames = frames_for_millis(sample_rate, LOUDNESS_PROFILE_STEP_MILLIS);
        Ok(Self {
            analyzer,
            channels: channels.max(1),
            frames: 0,
            profile_step_frames,
            next_profile_frame: profile_step_frames.saturating_mul(4) as u64,
            profile: Vec::new(),
        })
    }

    fn add_frames(&mut self, mut samples: &[f32]) -> Result<(), String> {
        if !samples.len().is_multiple_of(self.channels) {
            return Err("Decoded audio did not contain complete channel frames".to_owned());
        }

        while !samples.is_empty() {
            let available_frames = samples.len() / self.channels;
            let frames_to_profile = self.next_profile_frame.saturating_sub(self.frames);
            let frames_to_add = if frames_to_profile == 0 {
                available_frames
            } else {
                available_frames.min(usize::try_from(frames_to_profile).unwrap_or(available_frames))
            };
            if frames_to_add == 0 {
                self.next_profile_frame = self
                    .next_profile_frame
                    .saturating_add(self.profile_step_frames as u64);
                continue;
            }

            let sample_count = frames_to_add * self.channels;
            self.analyzer
                .add_frames_f32(&samples[..sample_count])
                .map_err(|error| format!("Could not analyze decoded audio: {error}"))?;
            self.frames = self.frames.saturating_add(frames_to_add as u64);
            samples = &samples[sample_count..];

            if self.frames >= self.next_profile_frame {
                self.record_profile_point();
                self.next_profile_frame = self
                    .next_profile_frame
                    .saturating_add(self.profile_step_frames as u64);
            }
        }

        Ok(())
    }

    fn record_profile_point(&mut self) {
        let Ok(lufs) = self.analyzer.loudness_momentary() else {
            return;
        };
        if lufs.is_finite() {
            self.push_profile_point(lufs as f32);
        }
    }

    fn push_profile_point(&mut self, lufs: f32) {
        if !lufs.is_finite() {
            return;
        }
        if self.profile.len() >= MAX_LOUDNESS_PROFILE_POINTS {
            self.profile = self.profile.iter().step_by(2).copied().collect();
            self.profile_step_frames = self.profile_step_frames.saturating_mul(2).max(1);
        }
        self.profile.push(LoudnessPoint {
            end_frame: self.frames,
            lufs,
        });
    }

    fn profile(&self) -> Vec<LoudnessPoint> {
        self.profile.clone()
    }

    fn finish(self) -> Option<f32> {
        self.analyzer
            .loudness_global()
            .ok()
            .filter(|lufs| lufs.is_finite())
            .map(|lufs| lufs as f32)
    }
}

fn frames_for_millis(sample_rate: u32, millis: u64) -> usize {
    ((sample_rate as u64 * millis).div_ceil(1000))
        .try_into()
        .unwrap_or(usize::MAX)
        .max(1)
}

fn preview_progress(decoded_frames: usize, expected_frames: Option<u64>) -> Option<f32> {
    let expected_frames = expected_frames.filter(|frames| *frames > 0)?;
    Some((decoded_frames as f64 / expected_frames as f64).clamp(0.0, 1.0) as f32)
}

fn progressive_preview<T>(
    decoded_frames: usize,
    expected_frames: Option<u64>,
    build: impl FnOnce(f32) -> Option<T>,
) -> Option<T> {
    let progress = preview_progress(decoded_frames, expected_frames)?;
    build(progress)
}

fn loudness_channel_map(channel_layout: Channels, channel_count: usize) -> Vec<LoudnessChannel> {
    let mapped = channel_layout
        .iter()
        .map(map_loudness_channel)
        .collect::<Vec<_>>();
    if mapped.len() == channel_count && !mapped.is_empty() {
        return mapped;
    }

    (0..channel_count)
        .map(|channel| match channel {
            0 => LoudnessChannel::Left,
            1 => LoudnessChannel::Right,
            2 => LoudnessChannel::Center,
            3 => LoudnessChannel::Unused,
            4 => LoudnessChannel::LeftSurround,
            5 => LoudnessChannel::RightSurround,
            _ => LoudnessChannel::Unused,
        })
        .collect()
}

fn map_loudness_channel(channel: Channels) -> LoudnessChannel {
    if channel == Channels::FRONT_LEFT || channel == Channels::FRONT_LEFT_CENTRE {
        LoudnessChannel::Left
    } else if channel == Channels::FRONT_RIGHT || channel == Channels::FRONT_RIGHT_CENTRE {
        LoudnessChannel::Right
    } else if channel == Channels::FRONT_LEFT_WIDE {
        LoudnessChannel::Mp060
    } else if channel == Channels::FRONT_RIGHT_WIDE {
        LoudnessChannel::Mm060
    } else if channel == Channels::FRONT_CENTRE {
        LoudnessChannel::Center
    } else if channel == Channels::FRONT_CENTRE_HIGH {
        LoudnessChannel::Up000
    } else if channel == Channels::LFE1 || channel == Channels::LFE2 {
        LoudnessChannel::Unused
    } else if channel == Channels::REAR_LEFT {
        LoudnessChannel::LeftSurround
    } else if channel == Channels::REAR_RIGHT {
        LoudnessChannel::RightSurround
    } else if channel == Channels::SIDE_LEFT {
        LoudnessChannel::Mp090
    } else if channel == Channels::SIDE_RIGHT {
        LoudnessChannel::Mm090
    } else if channel == Channels::REAR_LEFT_CENTRE {
        LoudnessChannel::Mp135
    } else if channel == Channels::REAR_RIGHT_CENTRE {
        LoudnessChannel::Mm135
    } else if channel == Channels::REAR_CENTRE {
        LoudnessChannel::Mp180
    } else if channel == Channels::TOP_CENTRE || channel == Channels::TOP_FRONT_CENTRE {
        LoudnessChannel::Up000
    } else if channel == Channels::TOP_FRONT_LEFT || channel == Channels::FRONT_LEFT_HIGH {
        LoudnessChannel::Up030
    } else if channel == Channels::TOP_FRONT_RIGHT || channel == Channels::FRONT_RIGHT_HIGH {
        LoudnessChannel::Um030
    } else if channel == Channels::TOP_REAR_LEFT {
        LoudnessChannel::Up110
    } else if channel == Channels::TOP_REAR_RIGHT {
        LoudnessChannel::Um110
    } else if channel == Channels::TOP_REAR_CENTRE {
        LoudnessChannel::Up180
    } else {
        LoudnessChannel::Unused
    }
}

#[derive(Clone, Copy, Debug)]
struct PeakBucket {
    min: f32,
    max: f32,
    windows: usize,
    squared_energy: f64,
    frames: usize,
    start_window: usize,
    end_window: usize,
}

impl PeakBucket {
    fn empty() -> Self {
        Self {
            min: 1.0,
            max: -1.0,
            windows: 0,
            squared_energy: 0.0,
            frames: 0,
            start_window: 0,
            end_window: 0,
        }
    }

    fn add(&mut self, peak: PeakMeasurement, window_index: usize) {
        if self.windows == 0 {
            self.start_window = window_index;
            self.end_window = window_index.saturating_add(1);
        } else {
            self.start_window = self.start_window.min(window_index);
            self.end_window = self.end_window.max(window_index.saturating_add(1));
        }
        self.min = self.min.min(peak.min);
        self.max = self.max.max(peak.max);
        self.windows = self.windows.saturating_add(1);
        self.squared_energy += peak.squared_energy;
        self.frames = self.frames.saturating_add(peak.frames);
    }

    fn merge(self, other: Self) -> Self {
        if self.windows == 0 {
            return other;
        }
        if other.windows == 0 {
            return self;
        }
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
            windows: self.windows.saturating_add(other.windows),
            squared_energy: self.squared_energy + other.squared_energy,
            frames: self.frames.saturating_add(other.frames),
            start_window: self.start_window.min(other.start_window),
            end_window: self.end_window.max(other.end_window),
        }
    }

    fn finish(self) -> PeakMeasurement {
        if self.windows == 0 {
            return PeakMeasurement::silence();
        }
        PeakMeasurement {
            min: self.min,
            max: self.max,
            squared_energy: self.squared_energy,
            frames: self.frames,
        }
    }

    fn span(&self) -> Option<(usize, usize)> {
        (self.windows > 0 && self.end_window > self.start_window)
            .then_some((self.start_window, self.end_window))
    }
}

#[derive(Clone, Copy, Debug)]
struct ResampledPeak {
    min: f32,
    max: f32,
    squared_energy: f64,
    frames: f64,
    covered_span: f64,
}

impl ResampledPeak {
    fn empty() -> Self {
        Self {
            min: 1.0,
            max: -1.0,
            squared_energy: 0.0,
            frames: 0.0,
            covered_span: 0.0,
        }
    }

    fn finish(self) -> PeakMeasurement {
        if self.covered_span <= 0.0 || self.frames <= 0.0 {
            return PeakMeasurement::silence();
        }

        let frames = self.frames.round().clamp(1.0, usize::MAX as f64) as usize;
        let mean_square = (self.squared_energy / self.frames).max(0.0);
        PeakMeasurement {
            min: self.min,
            max: self.max,
            squared_energy: mean_square * frames as f64,
            frames,
        }
    }
}

fn resample_unknown_buckets(
    buckets: &[PeakBucket],
    source_windows: usize,
    maximum_buckets: usize,
) -> Vec<PeakMeasurement> {
    if source_windows == 0 {
        return Vec::new();
    }

    let slot_count = source_windows.min(maximum_buckets.max(1));
    let source_windows = source_windows as f64;
    let slot_count_as_float = slot_count as f64;
    let mut slots = vec![ResampledPeak::empty(); slot_count];

    for bucket in buckets {
        let Some((start_window, end_window)) = bucket.span() else {
            continue;
        };
        let bucket_start = start_window as f64;
        let bucket_end = end_window as f64;
        let bucket_span = bucket_end - bucket_start;
        if bucket_span <= 0.0 || bucket.frames == 0 {
            continue;
        }

        for (slot_index, slot) in slots.iter_mut().enumerate() {
            let slot_start = slot_index as f64 * source_windows / slot_count_as_float;
            let slot_end = (slot_index + 1) as f64 * source_windows / slot_count_as_float;
            let overlap = (bucket_end.min(slot_end) - bucket_start.max(slot_start)).max(0.0);
            if overlap <= 0.0 {
                continue;
            }

            let fraction = overlap / bucket_span;
            slot.min = slot.min.min(bucket.min);
            slot.max = slot.max.max(bucket.max);
            slot.squared_energy += bucket.squared_energy * fraction;
            slot.frames += bucket.frames as f64 * fraction;
            slot.covered_span += overlap;
        }
    }

    slots.into_iter().map(ResampledPeak::finish).collect()
}

/// Keep the decoded peak envelope bounded even when a source has millions of
/// frames. If the container supplies a frame count, buckets are assigned to a
/// fixed uniform target. Otherwise adjacent buckets are folded as the target
/// fills; extrema remain lossless while the fallback stays bounded.
struct PeakReducer {
    buckets: Vec<PeakBucket>,
    maximum_buckets: usize,
    expected_windows: Option<usize>,
    seen_windows: usize,
}

impl PeakReducer {
    fn new(maximum_buckets: usize, expected_windows: Option<usize>) -> Self {
        let maximum_buckets = maximum_buckets.max(1);
        let expected_windows = expected_windows.filter(|windows| *windows > 0);
        let target_buckets = expected_windows.map(|windows| windows.min(maximum_buckets));
        let buckets = target_buckets
            .map(|count| vec![PeakBucket::empty(); count])
            .unwrap_or_default();
        Self {
            buckets,
            maximum_buckets,
            expected_windows,
            seen_windows: 0,
        }
    }

    fn add(&mut self, peak: PeakMeasurement) {
        let window_index = self.seen_windows;
        self.seen_windows = self.seen_windows.saturating_add(1);
        if let Some(expected_windows) = self.expected_windows {
            let bucket_count = self.buckets.len().max(1);
            let index = self
                .seen_windows
                .saturating_sub(1)
                .saturating_mul(bucket_count)
                .checked_div(expected_windows)
                .unwrap_or_default()
                .min(bucket_count - 1);
            if self.buckets.is_empty() {
                self.buckets.push(PeakBucket::empty());
            }
            self.buckets[index].add(peak, window_index);
            return;
        }

        self.add_unknown_peak(peak, window_index);
    }

    fn add_unknown_peak(&mut self, peak: PeakMeasurement, window_index: usize) {
        let mut bucket = PeakBucket::empty();
        bucket.add(peak, window_index);
        self.buckets.push(bucket);
        if self.buckets.len() > self.maximum_buckets {
            let previous = std::mem::take(&mut self.buckets);
            let mut reduced = Vec::with_capacity(previous.len().div_ceil(2));
            for pair in previous.chunks(2) {
                let first = pair[0];
                let second = pair.get(1).copied().unwrap_or_else(PeakBucket::empty);
                reduced.push(first.merge(second));
            }
            self.buckets = reduced;
        }
    }

    fn finish(self) -> Vec<PeakMeasurement> {
        if self.expected_windows.is_some() {
            self.buckets.into_iter().map(PeakBucket::finish).collect()
        } else {
            resample_unknown_buckets(&self.buckets, self.seen_windows, self.maximum_buckets)
        }
    }

    fn snapshot(&self) -> Vec<PeakMeasurement> {
        if self.expected_windows.is_some() {
            self.buckets
                .iter()
                .copied()
                .map(PeakBucket::finish)
                .collect()
        } else {
            resample_unknown_buckets(&self.buckets, self.seen_windows, self.maximum_buckets)
        }
    }

    fn snapshot_with_partial(&self, partial: Option<PeakMeasurement>) -> Vec<PeakMeasurement> {
        let Some(partial) = partial else {
            return self.snapshot();
        };

        let mut buckets = self.buckets.clone();
        if let Some(expected_windows) = self.expected_windows {
            if buckets.is_empty() {
                buckets.resize(
                    expected_windows.min(self.maximum_buckets).max(1),
                    PeakBucket::empty(),
                );
            }
            let bucket_count = buckets.len().max(1);
            let index = self
                .seen_windows
                .saturating_mul(bucket_count)
                .checked_div(expected_windows)
                .unwrap_or_default()
                .min(bucket_count - 1);
            buckets[index].add(partial, self.seen_windows);
            buckets.into_iter().map(PeakBucket::finish).collect()
        } else {
            let mut source_windows = self.seen_windows;
            let mut bucket = PeakBucket::empty();
            bucket.add(partial, source_windows);
            buckets.push(bucket);
            source_windows = source_windows.saturating_add(1);
            if buckets.len() > self.maximum_buckets {
                let previous = buckets;
                buckets = previous
                    .chunks(2)
                    .map(|pair| {
                        let first = pair[0];
                        let second = pair.get(1).copied().unwrap_or_else(PeakBucket::empty);
                        first.merge(second)
                    })
                    .collect();
            }
            resample_unknown_buckets(&buckets, source_windows, self.maximum_buckets)
        }
    }
}

#[allow(dead_code)]
pub fn decode_waveform(path: &Path) -> Result<WaveformData, String> {
    decode_waveform_with_progress_and_cancellation(path, || false, |_| {})
}

/// Decode one waveform while allowing the business worker to abandon obsolete
/// selections at packet boundaries. The final waveform contract remains
/// unchanged; this only prevents a superseded file from occupying the single
/// blocking-I/O lane until EOF.
#[allow(dead_code)]
pub fn decode_waveform_with_cancellation(
    path: &Path,
    should_cancel: impl Fn() -> bool,
) -> Result<WaveformData, String> {
    decode_waveform_with_progress_and_cancellation(path, should_cancel, |_| {})
}

/// Decode a waveform and emit bounded peak snapshots while the complete
/// loudness analysis continues. Each snapshot is only a decoded prefix; the
/// caller must keep final-only interactions disabled until the returned
/// `WaveformData` is accepted.
pub fn decode_waveform_with_progress_and_cancellation(
    path: &Path,
    should_cancel: impl Fn() -> bool,
    mut on_progress: impl FnMut(WaveformProgress),
) -> Result<WaveformData, String> {
    if should_cancel() {
        return Err(String::from("cancelled"));
    }
    let file = File::open(path).map_err(|error| {
        format!(
            "Could not open {} for waveform analysis: {error}",
            path.display()
        )
    })?;
    let media = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(&extension.to_ascii_lowercase());
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            media,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| format!("Could not identify {}: {error}", path.display()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| format!("No decodable audio track found in {}", path.display()))?;
    let track_id = track.id;
    let codec_params = track.codec_params.clone();
    let expected_frames = codec_params.n_frames.filter(|frames| *frames > 0);
    let expected_windows = expected_frames
        .and_then(|frames| usize::try_from(frames.div_ceil(PEAK_WINDOW_FRAMES as u64)).ok());
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|error| format!("Could not decode {}: {error}", path.display()))?;

    let mut reducer = PeakReducer::new(MAX_DISPLAY_BUCKETS, expected_windows);
    let mut window = PeakWindow::new();
    let mut loudness: Option<LoudnessAccumulator> = None;
    let mut decoded_frames = 0usize;
    let mut sample_rate = None;
    let mut channels = None;
    let mut channel_layout = None;
    let mut next_preview_frame = None;

    loop {
        if should_cancel() {
            return Err(String::from("cancelled"));
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(Error::ResetRequired) => {
                return Err(format!(
                    "The audio stream changed format while reading {}",
                    path.display()
                ));
            }
            Err(error) => {
                return Err(format!("Could not read {}: {error}", path.display()));
            }
        };
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(Error::DecodeError(error)) => {
                return Err(format!("Could not decode {}: {error}", path.display()));
            }
            Err(Error::IoError(error)) => {
                return Err(format!("Could not decode {}: {error}", path.display()));
            }
            Err(error) => {
                return Err(format!("Could not decode {}: {error}", path.display()));
            }
        };

        let decoded_sample_rate = decoded.spec().rate;
        let decoded_channel_layout = decoded.spec().channels;
        let decoded_channels = decoded.spec().channels.count().max(1);
        if decoded_sample_rate == 0 {
            return Err(format!(
                "The audio stream has no valid sample rate in {}",
                path.display()
            ));
        }
        if sample_rate.is_some_and(|rate| rate != decoded_sample_rate)
            || channels.is_some_and(|count| count != decoded_channels)
            || channel_layout.is_some_and(|layout| layout != decoded_channel_layout)
        {
            return Err(format!(
                "The audio stream changed its sample format while reading {}",
                path.display()
            ));
        }
        sample_rate = Some(decoded_sample_rate);
        channels = Some(decoded_channels);
        channel_layout = Some(decoded_channel_layout);
        let preview_interval = frames_for_millis(decoded_sample_rate, PREVIEW_INTERVAL_MILLIS);
        let first_preview = frames_for_millis(decoded_sample_rate, PREVIEW_FIRST_MILLIS);
        let next_preview_frame = next_preview_frame.get_or_insert(first_preview);
        if loudness.is_none() {
            loudness = Some(LoudnessAccumulator::new(
                decoded_sample_rate,
                decoded_channel_layout,
                decoded_channels,
            )?);
        }
        let loudness = loudness.as_mut().expect("loudness analyzer initialized");

        let mut sample_buffer =
            SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        sample_buffer.copy_interleaved_ref(decoded);
        let samples = sample_buffer.samples();
        loudness.add_frames(samples)?;
        for frame in samples.chunks_exact(decoded_channels) {
            window.add_frame(frame);
            decoded_frames = decoded_frames.saturating_add(1);
            if window.frames >= PEAK_WINDOW_FRAMES {
                if let Some(peak) = window.finish() {
                    reducer.add(peak);
                }
                window = PeakWindow::new();
            }
        }
        if should_cancel() {
            return Err(String::from("cancelled"));
        }
        if decoded_frames >= *next_preview_frame {
            if let Some(progress) =
                progressive_preview(decoded_frames, expected_frames, |progress| {
                    let peaks = reducer.snapshot_with_partial(window.snapshot());
                    (!peaks.is_empty()).then(|| WaveformProgress {
                        waveform: preview_waveform(
                            peaks,
                            decoded_sample_rate,
                            decoded_channels,
                            decoded_frames,
                            expected_frames,
                        ),
                        progress: Some(progress),
                    })
                })
            {
                on_progress(progress);
            }
            *next_preview_frame = decoded_frames.saturating_add(preview_interval);
        }
    }

    if let Some(peak) = window.finish() {
        reducer.add(peak);
    }
    let peaks = reducer.finish();
    if decoded_frames == 0 || peaks.is_empty() {
        return Err(format!("No audio samples found in {}", path.display()));
    }

    let sample_rate =
        sample_rate.ok_or_else(|| format!("No sample rate found in {}", path.display()))?;
    let channels =
        channels.ok_or_else(|| format!("No channel layout found in {}", path.display()))?;
    let render_frames = peaks.len();
    let summary = Arc::new(summary_from_peaks(&peaks));
    let duration_millis = ((decoded_frames as u128 * 1000) / sample_rate.max(1) as u128) as u64;
    let loudness_profile = loudness
        .as_ref()
        .map(LoudnessAccumulator::profile)
        .unwrap_or_default();
    let integrated_lufs = loudness.and_then(LoudnessAccumulator::finish);

    Ok(WaveformData {
        sample_rate,
        channels,
        duration_millis,
        render_frames,
        integrated_lufs,
        loudness_profile: Arc::from(loudness_profile.into_boxed_slice()),
        summary,
    })
}

fn preview_waveform(
    peaks: Vec<PeakMeasurement>,
    sample_rate: u32,
    channels: usize,
    decoded_frames: usize,
    expected_frames: Option<u64>,
) -> WaveformData {
    let duration_frames = expected_frames
        .filter(|frames| *frames > 0)
        .unwrap_or(decoded_frames as u64);
    let duration_millis = ((duration_frames as u128 * 1_000) / sample_rate.max(1) as u128) as u64;
    let render_frames = peaks.len();
    WaveformData {
        sample_rate,
        channels,
        duration_millis,
        render_frames,
        integrated_lufs: None,
        loudness_profile: Arc::from([]),
        summary: Arc::new(summary_from_peaks(&peaks)),
    }
}

fn summary_from_peaks(peaks: &[PeakMeasurement]) -> GpuSignalSummary {
    // Keep both the lossless extrema and a channel-energy band. Band 0 is the
    // lossless per-window extrema; band 1 stores mean-square energy so the
    // renderer can combine it across time before taking the final square root.
    let frames = peaks.len().max(1);
    let mut levels = Vec::new();
    let mut bucket_frames = 1usize;
    let mut buckets: Vec<GpuSignalSummaryBucket> = if peaks.is_empty() {
        vec![GpuSignalSummaryBucket::default(); SUMMARY_BAND_COUNT]
    } else {
        peaks
            .iter()
            .flat_map(|peak| {
                [
                    GpuSignalSummaryBucket {
                        min: peak.min,
                        max: peak.max,
                    },
                    GpuSignalSummaryBucket {
                        min: 0.0,
                        max: (peak.squared_energy / peak.frames.max(1) as f64).max(0.0) as f32,
                    },
                ]
            })
            .collect()
    };

    loop {
        levels.push(GpuSignalSummaryLevel {
            bucket_frames,
            buckets: buckets.clone().into(),
        });
        if bucket_frames >= frames {
            break;
        }
        let frame_count = buckets.len() / SUMMARY_BAND_COUNT.max(1);
        let mut next = Vec::with_capacity(buckets.len().div_ceil(2));
        for frame in (0..frame_count).step_by(2) {
            let next_frame = frame + 1;
            for band in 0..SUMMARY_BAND_COUNT {
                let first = buckets[frame * SUMMARY_BAND_COUNT + band];
                let second = if next_frame < frame_count {
                    buckets[next_frame * SUMMARY_BAND_COUNT + band]
                } else {
                    first
                };
                next.push(GpuSignalSummaryBucket {
                    min: first.min.min(second.min),
                    max: first.max.max(second.max),
                });
            }
        }
        buckets = next;
        bucket_frames = bucket_frames.saturating_mul(2).max(bucket_frames + 1);
    }

    GpuSignalSummary {
        frames,
        band_count: SUMMARY_BAND_COUNT,
        levels,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LoudnessAccumulator, LoudnessPoint, MAX_LOUDNESS_MATCH_DB, MAX_LOUDNESS_PROFILE_POINTS,
        PeakMeasurement, PeakReducer, PeakWindow, WaveformData,
        decode_waveform_with_progress_and_cancellation, linear_gain_for_db, load_waveform_cache,
        loudness_at_position, loudness_channel_map, loudness_match_gain_db, preview_progress,
        preview_waveform, progressive_preview, summary_from_peaks, waveform_cache_fingerprint,
        write_waveform_cache_if_unchanged,
    };
    use radiant::runtime::GpuSignalSummary;
    use std::{
        fs,
        path::Path,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };
    use symphonia::core::audio::Channels;

    fn rms_peak(level: f32) -> PeakMeasurement {
        PeakMeasurement {
            min: -level,
            max: level,
            squared_energy: f64::from(level) * f64::from(level),
            frames: 1,
        }
    }

    #[test]
    fn waveform_cache_round_trips_and_rejects_stale_or_corrupt_entries() {
        let root = std::env::temp_dir().join(format!(
            "cadence-waveform-cache-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after the epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create cache test directory");
        let source = root.join("source.wav");
        let cache = root.join("waveform.json");
        fs::write(&source, b"source").expect("write source fixture");
        let waveform = WaveformData {
            sample_rate: 48_000,
            channels: 2,
            duration_millis: 100,
            render_frames: 2,
            integrated_lufs: Some(-8.0),
            loudness_profile: Arc::from([LoudnessPoint {
                end_frame: 4_800,
                lufs: -8.5,
            }]),
            summary: Arc::new(summary_from_peaks(&[rms_peak(0.5), rms_peak(0.25)])),
        };

        let source_fingerprint =
            waveform_cache_fingerprint(&source).expect("source fingerprint should exist");
        write_waveform_cache_if_unchanged(&source, &cache, source_fingerprint, &waveform)
            .expect("write waveform cache");
        let encoded = fs::read_to_string(&cache).expect("read waveform cache");
        assert!(
            !encoded.contains("spectrogram"),
            "waveform cache must not retain the removed passive spectrogram"
        );
        assert_eq!(load_waveform_cache(&source, &cache), Some(waveform.clone()));

        fs::write(&cache, b"not a waveform cache").expect("corrupt waveform cache");
        assert_eq!(load_waveform_cache(&source, &cache), None);

        let source_fingerprint =
            waveform_cache_fingerprint(&source).expect("source fingerprint should exist");
        write_waveform_cache_if_unchanged(&source, &cache, source_fingerprint, &waveform)
            .expect("rewrite waveform cache");
        let source_fingerprint =
            waveform_cache_fingerprint(&source).expect("source fingerprint should exist");
        fs::write(&source, b"changed source").expect("change source fixture");
        assert!(
            write_waveform_cache_if_unchanged(&source, &cache, source_fingerprint, &waveform)
                .is_err()
        );
        assert_eq!(load_waveform_cache(&source, &cache), None);

        fs::remove_dir_all(root).expect("remove cache test directory");
    }

    #[test]
    fn reducer_preserves_extrema_with_bounded_storage() {
        let mut reducer = PeakReducer::new(2, None);
        for index in 0..100 {
            let value = index as f32 / 100.0;
            reducer.add(PeakMeasurement {
                min: -value,
                max: value,
                squared_energy: f64::from(value) * f64::from(value),
                frames: 1,
            });
            assert!(reducer.buckets.len() <= 2);
        }
        let peaks = reducer.finish();
        assert_eq!(peaks.len(), 2);
        let minimum = peaks.iter().map(|peak| peak.min).fold(1.0, f32::min);
        let maximum = peaks.iter().map(|peak| peak.max).fold(-1.0, f32::max);
        assert_eq!(minimum, -0.99);
        assert_eq!(maximum, 0.99);
    }

    #[test]
    fn peak_window_preserves_multichannel_extrema_without_downmixing() {
        let mut anti_phase = PeakWindow::new();
        anti_phase.add_frame(&[1.0, -1.0]);
        assert_eq!(
            anti_phase.finish(),
            Some(PeakMeasurement {
                min: -1.0,
                max: 1.0,
                squared_energy: 1.0,
                frames: 1,
            })
        );

        let mut hard_panned = PeakWindow::new();
        hard_panned.add_frame(&[1.0, 0.0]);
        let hard_panned = hard_panned.finish().expect("hard-panned frame");
        assert_eq!(hard_panned.min, 0.0);
        assert_eq!(hard_panned.max, 1.0);
        assert!((hard_panned.rms() - 2.0_f32.sqrt().recip()).abs() < 1e-6);
    }

    #[test]
    fn peak_window_rms_tracks_channel_energy_without_cancellation() {
        let mut window = PeakWindow::new();
        window.add_frame(&[1.0, -1.0]);
        window.add_frame(&[0.0, 0.0]);

        let measurement = window.finish().expect("two source frames");
        assert_eq!(measurement.min, -1.0);
        assert_eq!(measurement.max, 1.0);
        assert!((measurement.rms() - 2.0_f32.sqrt().recip()).abs() < 1e-6);
    }

    #[test]
    fn reducer_uses_declared_frame_count_for_uniform_target_capacity() {
        let mut reducer = PeakReducer::new(4, Some(10));
        for index in 0..10 {
            let value = index as f32;
            reducer.add(PeakMeasurement {
                min: value,
                max: value,
                squared_energy: f64::from(value) * f64::from(value),
                frames: 1,
            });
        }
        assert_eq!(reducer.buckets.len(), 4);
        assert_eq!(reducer.finish().len(), 4);
    }

    #[test]
    fn declared_partial_target_keeps_empty_future_buckets_as_silence() {
        let mut reducer = PeakReducer::new(4, Some(4));
        reducer.add(rms_peak(0.8));

        let snapshot = reducer.snapshot();
        assert_eq!(snapshot.len(), 4);
        assert!((snapshot[0].rms() - 0.8).abs() < 1e-6);
        assert!(
            snapshot[1..]
                .iter()
                .all(|peak| peak == &PeakMeasurement::silence())
        );

        let preview = reducer.snapshot_with_partial(Some(rms_peak(0.4)));
        assert_eq!(preview.len(), 4);
        assert!((preview[1].rms() - 0.4).abs() < 1e-6);

        let finished = reducer.finish();
        assert_eq!(finished.len(), 4);
        assert!(
            finished[1..]
                .iter()
                .all(|peak| peak == &PeakMeasurement::silence())
        );
    }

    #[test]
    fn unknown_duration_resampling_keeps_a_localized_dip_in_time() {
        let mut reducer = PeakReducer::new(4, None);
        for index in 0..12 {
            reducer.add(rms_peak(if index == 10 { 0.0 } else { 1.0 }));
        }

        let snapshot = reducer.snapshot();
        assert_eq!(snapshot.len(), 4);
        assert!(snapshot[2].rms() > 0.99);
        assert!(snapshot[3].rms() < 0.9);

        let finished = reducer.finish();
        assert_eq!(finished.len(), 4);
        assert!(finished[2].rms() > 0.99);
        assert!(finished[3].rms() < 0.9);
    }

    #[test]
    fn reducer_combines_alternating_loud_and_silent_windows_by_energy() {
        let mut reducer = PeakReducer::new(1, None);
        reducer.add(rms_peak(1.0));
        reducer.add(rms_peak(0.0));

        let peak = reducer.finish().pop().expect("merged peak");
        assert!((peak.rms() - 2.0_f32.sqrt().recip()).abs() < 1e-6);
    }

    #[test]
    fn summary_preserves_each_peak_window_and_builds_a_pyramid() {
        let summary = summary_from_peaks(&[
            PeakMeasurement {
                min: -0.8,
                max: 0.4,
                squared_energy: 0.3_f64 * 0.3,
                frames: 1,
            },
            PeakMeasurement {
                min: -0.2,
                max: 0.9,
                squared_energy: 0.6_f64 * 0.6,
                frames: 1,
            },
            PeakMeasurement {
                min: -1.0,
                max: 0.7,
                squared_energy: 0.5_f64 * 0.5,
                frames: 1,
            },
        ]);
        assert_eq!(summary.frames, 3);
        assert_eq!(summary.band_count, 2);
        assert_eq!(summary.levels[0].bucket_frames, 1);
        assert_eq!(summary.levels[0].buckets.len(), 6);
        assert_eq!(summary.levels[0].buckets[0].min, -0.8);
        assert_eq!(summary.levels[0].buckets[1].max, 0.3 * 0.3);
        assert_eq!(summary.levels[1].bucket_frames, 2);
        assert_eq!(summary.levels[1].buckets[0].min, -0.8);
        assert_eq!(summary.levels[1].buckets[0].max, 0.9);
    }

    #[test]
    fn preview_snapshot_preserves_absolute_peaks_and_full_declared_duration() {
        let preview = preview_waveform(
            vec![
                PeakMeasurement {
                    min: -0.1,
                    max: 0.2,
                    squared_energy: 0.2_f64 * 0.2,
                    frames: 1,
                },
                PeakMeasurement {
                    min: -0.4,
                    max: 0.5,
                    squared_energy: 0.5_f64 * 0.5,
                    frames: 1,
                },
            ],
            48_000,
            2,
            24_000,
            Some(48_000),
        );

        assert_eq!(preview.duration_millis, 1_000);
        assert_eq!(preview.render_frames, 2);
        assert_eq!(preview.integrated_lufs, None);
        assert_eq!(preview.summary.levels[0].buckets[2].max, 0.5);
    }

    #[test]
    fn unknown_duration_does_not_publish_a_stretched_preview_extent() {
        assert_eq!(preview_progress(1_024, None), None);
        assert_eq!(preview_progress(1_024, Some(0)), None);
        assert_eq!(preview_progress(4_800, Some(9_600)), Some(0.5));
        assert_eq!(preview_progress(12_000, Some(9_600)), Some(1.0));
    }

    #[test]
    fn progressive_preview_only_builds_for_known_positive_duration() {
        let mut calls = 0;
        assert_eq!(
            progressive_preview(1_024, None, |_| {
                calls += 1;
                Some(())
            }),
            None
        );
        assert_eq!(
            progressive_preview(1_024, Some(0), |_| {
                calls += 1;
                Some(())
            }),
            None
        );
        assert_eq!(
            progressive_preview(4_800, Some(9_600), |progress| {
                calls += 1;
                Some(progress)
            }),
            Some(0.5)
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn cancelled_progressive_decode_exits_before_opening_a_file() {
        let result = decode_waveform_with_progress_and_cancellation(
            Path::new("/path/that/does/not/exist.wav"),
            || true,
            |_| {},
        );

        assert_eq!(result, Err(String::from("cancelled")));
    }

    #[test]
    fn loudness_profile_follows_position_with_interpolation_and_bounds() {
        let waveform = WaveformData {
            sample_rate: 100,
            channels: 1,
            duration_millis: 800,
            render_frames: 80,
            integrated_lufs: Some(-8.0),
            loudness_profile: std::sync::Arc::from([
                LoudnessPoint {
                    end_frame: 40,
                    lufs: -4.0,
                },
                LoudnessPoint {
                    end_frame: 80,
                    lufs: -12.0,
                },
            ]),
            summary: std::sync::Arc::new(GpuSignalSummary::from_interleaved_samples(
                &[0.1, 0.8, 0.2, 0.4],
                4,
                1,
            )),
        };

        assert_eq!(loudness_at_position(&waveform, 0), Some(-4.0));
        assert_eq!(loudness_at_position(&waveform, 400), Some(-4.0));
        assert_eq!(loudness_at_position(&waveform, 600), Some(-8.0));
        assert_eq!(loudness_at_position(&waveform, 800), Some(-12.0));
        assert_eq!(loudness_at_position(&waveform, 1_200), Some(-12.0));
    }

    #[test]
    fn loudness_at_position_falls_back_to_integrated_value_without_profile() {
        let waveform = WaveformData {
            sample_rate: 48_000,
            channels: 1,
            duration_millis: 1_000,
            render_frames: 48_000,
            integrated_lufs: Some(-14.0),
            loudness_profile: std::sync::Arc::from([]),
            summary: std::sync::Arc::new(GpuSignalSummary::from_interleaved_samples(
                &[0.1, 0.8, 0.2, 0.4],
                4,
                1,
            )),
        };

        assert_eq!(loudness_at_position(&waveform, 500), Some(-14.0));
    }

    #[test]
    fn loudness_profile_keeps_recording_after_reaching_its_bound() {
        let mut accumulator = LoudnessAccumulator::new(1_000, Channels::FRONT_LEFT, 1)
            .expect("the test analyzer should initialize");
        accumulator.profile = (0..MAX_LOUDNESS_PROFILE_POINTS)
            .map(|index| LoudnessPoint {
                end_frame: index as u64 * 100,
                lufs: -24.0 + index as f32 / 1_000.0,
            })
            .collect();
        accumulator.profile_step_frames = 100;
        accumulator.frames = MAX_LOUDNESS_PROFILE_POINTS as u64 * 100;

        accumulator.push_profile_point(-6.0);

        assert_eq!(
            accumulator.profile.len(),
            MAX_LOUDNESS_PROFILE_POINTS / 2 + 1
        );
        assert_eq!(accumulator.profile_step_frames, 200);
        assert_eq!(
            accumulator.profile.last().map(|point| point.end_frame),
            Some(accumulator.frames)
        );
        assert_eq!(
            accumulator.profile.last().map(|point| point.lufs),
            Some(-6.0)
        );
    }

    #[test]
    fn loudness_accumulator_uses_k_weighting_and_standard_1_khz_calibration() {
        let sample_rate = 48_000;
        let samples = one_khz_tone(sample_rate, 1, 5, 0.1);
        let level = analyze(sample_rate, Channels::FRONT_LEFT, 1, &samples)
            .expect("tone should have a measurable integrated level");

        assert!((level - (-23.0)).abs() < 0.1, "unexpected level: {level}");
    }

    #[test]
    fn loudness_accumulator_handles_common_sample_rates_consistently() {
        let levels = [44_100, 48_000, 96_000]
            .into_iter()
            .map(|sample_rate| {
                let samples = one_khz_tone(sample_rate, 1, 5, 0.1);
                analyze(sample_rate, Channels::FRONT_LEFT, 1, &samples)
                    .expect("tone should have a measurable integrated level")
            })
            .collect::<Vec<_>>();
        let minimum = levels.iter().copied().fold(f32::INFINITY, f32::min);
        let maximum = levels.iter().copied().fold(f32::NEG_INFINITY, f32::max);

        assert!(
            maximum - minimum < 0.15,
            "sample-rate levels diverged: {levels:?}"
        );
    }

    #[test]
    fn loudness_accumulator_applies_relative_gating_to_silence() {
        let sample_rate = 48_000;
        let tone = one_khz_tone(sample_rate, 1, 5, 0.1);
        let tone_level = analyze(sample_rate, Channels::FRONT_LEFT, 1, &tone)
            .expect("tone should have a measurable integrated level");

        let mut tone_with_silence = tone.clone();
        tone_with_silence.extend(std::iter::repeat_n(0.0, sample_rate as usize * 5));
        let gated_level = analyze(sample_rate, Channels::FRONT_LEFT, 1, &tone_with_silence)
            .expect("tone with silence should retain its measurable level");

        assert!(
            (tone_level - gated_level).abs() < 0.15,
            "silence changed gated level: {tone_level} vs {gated_level}"
        );
    }

    #[test]
    fn loudness_accumulator_has_no_level_without_a_measurable_block() {
        let mut loudness = LoudnessAccumulator::new(48_000, Channels::FRONT_LEFT, 1)
            .expect("standard mono analyzer should initialize");
        loudness
            .add_frames(&vec![0.0; 48_000 / 2])
            .expect("silence should be accepted");

        assert_eq!(loudness.finish(), None);
    }

    #[test]
    fn loudness_accumulator_records_momentary_profile_after_first_window() {
        let sample_rate = 48_000;
        let samples = one_khz_tone(sample_rate, 1, 1, 0.1);
        let mut loudness = LoudnessAccumulator::new(sample_rate, Channels::FRONT_LEFT, 1)
            .expect("standard mono analyzer should initialize");

        loudness
            .add_frames(&samples)
            .expect("tone should be accepted by the analyzer");

        let profile = loudness.profile();
        assert!(!profile.is_empty(), "momentary loudness profile was empty");
        assert!(profile.iter().all(|point| point.lufs.is_finite()));
        assert!(profile[0].end_frame >= (sample_rate as u64 * 400) / 1_000);
    }

    #[test]
    fn loudness_accumulator_uses_explicit_channel_layout_and_source_level() {
        let channel_layout = Channels::FRONT_LEFT
            | Channels::FRONT_RIGHT
            | Channels::FRONT_CENTRE
            | Channels::LFE1
            | Channels::REAR_LEFT
            | Channels::REAR_RIGHT;
        let channel_map = loudness_channel_map(channel_layout, 6);
        assert_eq!(channel_map[0], ebur128::Channel::Left);
        assert_eq!(channel_map[1], ebur128::Channel::Right);
        assert_eq!(channel_map[2], ebur128::Channel::Center);
        assert_eq!(channel_map[3], ebur128::Channel::Unused);
        assert_eq!(channel_map[4], ebur128::Channel::LeftSurround);
        assert_eq!(channel_map[5], ebur128::Channel::RightSurround);

        let samples = one_khz_tone(48_000, 1, 5, 0.1);
        let source_level = analyze(48_000, Channels::FRONT_LEFT, 1, &samples)
            .expect("source should have a measurable integrated level");
        let output_gain = linear_gain_for_db(-12.0);
        let output_samples = samples
            .iter()
            .map(|sample| sample * output_gain)
            .collect::<Vec<_>>();
        let output_level = analyze(48_000, Channels::FRONT_LEFT, 1, &output_samples)
            .expect("gained output should have a measurable integrated level");

        assert!((source_level - (-23.0)).abs() < 0.1);
        assert!((source_level - output_level - 12.0).abs() < 0.15);
    }

    #[test]
    fn loudness_accumulator_weights_front_wide_channels_as_m060() {
        let sample_rate = 48_000;
        let stereo_layout = Channels::FRONT_LEFT | Channels::FRONT_RIGHT;
        let wide_layout = Channels::FRONT_LEFT_WIDE | Channels::FRONT_RIGHT_WIDE;
        let wide_map = loudness_channel_map(wide_layout, 2);
        assert_eq!(wide_map, [ebur128::Channel::Mp060, ebur128::Channel::Mm060]);

        let samples = one_khz_tone(sample_rate, 2, 5, 0.1);
        let stereo_level = analyze(sample_rate, stereo_layout, 2, &samples)
            .expect("stereo tone should have a measurable integrated level");
        let wide_level = analyze(sample_rate, wide_layout, 2, &samples)
            .expect("wide-channel tone should have a measurable integrated level");
        let expected_weighting_db = 10.0_f32 * 1.41_f32.log10();

        assert!(
            (wide_level - stereo_level - expected_weighting_db).abs() < 0.1,
            "wide-channel weighting differed by {} dB, expected {expected_weighting_db} dB",
            wide_level - stereo_level
        );
    }

    #[test]
    fn loudness_match_targets_the_imported_track_and_is_bounded() {
        assert_eq!(loudness_match_gain_db(Some(-8.0), Some(-12.5)), Some(4.5));
        assert_eq!(loudness_match_gain_db(Some(-12.5), Some(-8.0)), Some(-4.5));
        assert_eq!(
            loudness_match_gain_db(Some(3.0), Some(-60.0)),
            Some(MAX_LOUDNESS_MATCH_DB)
        );
        assert_eq!(loudness_match_gain_db(Some(f32::NAN), Some(-8.0)), None);
    }

    #[test]
    fn loudness_match_db_converts_to_linear_gain() {
        assert!((linear_gain_for_db(6.0206) - 2.0).abs() < 0.001);
        assert!((linear_gain_for_db(-6.0206) - 0.5).abs() < 0.001);
    }

    fn one_khz_tone(sample_rate: u32, channels: usize, seconds: usize, peak: f32) -> Vec<f32> {
        let frame_count = sample_rate as usize * seconds;
        let mut samples = Vec::with_capacity(frame_count * channels);
        for frame in 0..frame_count {
            let phase = std::f32::consts::TAU * 1_000.0 * frame as f32 / sample_rate as f32;
            let sample = peak * phase.sin();
            samples.extend(std::iter::repeat_n(sample, channels));
        }
        samples
    }

    fn analyze(
        sample_rate: u32,
        channel_layout: Channels,
        channels: usize,
        samples: &[f32],
    ) -> Option<f32> {
        let mut loudness = LoudnessAccumulator::new(sample_rate, channel_layout, channels).ok()?;
        loudness.add_frames(samples).ok()?;
        loudness.finish()
    }
}

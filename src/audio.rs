//! Background audio inspection for the native review surface.
//!
//! This module owns only bounded waveform analysis data. Native audition
//! playback is kept in the separate host-controlled transport module; this
//! decoder never performs output-device work on the Radiant UI path.

use radiant::runtime::{GpuSignalSummary, GpuSignalSummaryBucket, GpuSignalSummaryLevel};
use std::{fs::File, path::Path, sync::Arc};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{CODEC_TYPE_NULL, DecoderOptions},
    errors::Error,
    formats::FormatOptions,
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
};

const PEAK_WINDOW_FRAMES: usize = 1024;
const MAX_DISPLAY_BUCKETS: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
pub struct WaveformData {
    pub sample_rate: u32,
    pub channels: usize,
    pub duration_millis: u64,
    pub render_frames: usize,
    pub summary: Arc<GpuSignalSummary>,
}

#[derive(Clone, Copy, Debug, Default)]
struct PeakWindow {
    min: f32,
    max: f32,
    frames: usize,
}

impl PeakWindow {
    fn new() -> Self {
        Self {
            min: 1.0,
            max: -1.0,
            frames: 0,
        }
    }

    fn add(&mut self, sample: f32) {
        let sample = sample.clamp(-1.0, 1.0);
        self.min = self.min.min(sample);
        self.max = self.max.max(sample);
        self.frames = self.frames.saturating_add(1);
    }

    fn finish(self) -> Option<(f32, f32)> {
        (self.frames > 0).then_some((self.min, self.max))
    }
}

#[derive(Clone, Copy, Debug)]
struct PeakBucket {
    min: f32,
    max: f32,
    windows: usize,
}

impl PeakBucket {
    fn empty() -> Self {
        Self {
            min: 1.0,
            max: -1.0,
            windows: 0,
        }
    }

    fn add(&mut self, peak: (f32, f32)) {
        self.min = self.min.min(peak.0);
        self.max = self.max.max(peak.1);
        self.windows = self.windows.saturating_add(1);
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
        }
    }

    fn finish(self) -> Option<(f32, f32)> {
        (self.windows > 0).then_some((self.min, self.max))
    }
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

    fn add(&mut self, peak: (f32, f32)) {
        if let Some(expected_windows) = self.expected_windows {
            let bucket_count = self.buckets.len().max(1);
            let index = self
                .seen_windows
                .saturating_mul(bucket_count)
                .checked_div(expected_windows)
                .unwrap_or_default()
                .min(bucket_count - 1);
            if self.buckets.is_empty() {
                self.buckets.push(PeakBucket::empty());
            }
            self.buckets[index].add(peak);
            self.seen_windows = self.seen_windows.saturating_add(1);
            return;
        }

        self.buckets.push(PeakBucket::empty());
        let last = self.buckets.len() - 1;
        self.buckets[last].add(peak);
        if self.buckets.len() > self.maximum_buckets {
            let mut reduced = Vec::with_capacity(self.buckets.len().div_ceil(2));
            for pair in self.buckets.drain(..).collect::<Vec<_>>().chunks(2) {
                let first = pair[0];
                let second = pair.get(1).copied().unwrap_or_else(PeakBucket::empty);
                reduced.push(first.merge(second));
            }
            self.buckets = reduced;
        }
    }

    fn finish(self) -> Vec<(f32, f32)> {
        self.buckets
            .into_iter()
            .filter_map(PeakBucket::finish)
            .collect()
    }
}

pub fn decode_waveform(path: &Path) -> Result<WaveformData, String> {
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
    let expected_windows = codec_params
        .n_frames
        .and_then(|frames| usize::try_from(frames.div_ceil(PEAK_WINDOW_FRAMES as u64)).ok());
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|error| format!("Could not decode {}: {error}", path.display()))?;

    let mut reducer = PeakReducer::new(MAX_DISPLAY_BUCKETS, expected_windows);
    let mut window = PeakWindow::new();
    let mut decoded_frames = 0usize;
    let mut sample_rate = None;
    let mut channels = None;

    loop {
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
            Err(Error::DecodeError(_)) | Err(Error::IoError(_)) => continue,
            Err(error) => {
                return Err(format!("Could not decode {}: {error}", path.display()));
            }
        };

        let decoded_sample_rate = decoded.spec().rate;
        let decoded_channels = decoded.spec().channels.count().max(1);
        if decoded_sample_rate == 0 {
            return Err(format!(
                "The audio stream has no valid sample rate in {}",
                path.display()
            ));
        }
        if sample_rate.is_some_and(|rate| rate != decoded_sample_rate)
            || channels.is_some_and(|count| count != decoded_channels)
        {
            return Err(format!(
                "The audio stream changed its sample format while reading {}",
                path.display()
            ));
        }
        sample_rate = Some(decoded_sample_rate);
        channels = Some(decoded_channels);

        let mut sample_buffer =
            SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        sample_buffer.copy_interleaved_ref(decoded);
        for frame in sample_buffer.samples().chunks_exact(decoded_channels) {
            let mono = frame.iter().copied().sum::<f32>() / frame.len() as f32;
            window.add(mono);
            decoded_frames = decoded_frames.saturating_add(1);
            if window.frames >= PEAK_WINDOW_FRAMES {
                if let Some(peak) = window.finish() {
                    reducer.add(peak);
                }
                window = PeakWindow::new();
            }
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

    Ok(WaveformData {
        sample_rate,
        channels,
        duration_millis,
        render_frames,
        summary,
    })
}

fn summary_from_peaks(peaks: &[(f32, f32)]) -> GpuSignalSummary {
    let frames = peaks.len().max(1);
    let mut levels = Vec::new();
    let mut bucket_frames = 1usize;
    let mut buckets: Vec<GpuSignalSummaryBucket> = if peaks.is_empty() {
        vec![GpuSignalSummaryBucket::default()]
    } else {
        peaks
            .iter()
            .map(|&(min, max)| GpuSignalSummaryBucket { min, max })
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
        let mut next = Vec::with_capacity(buckets.len().div_ceil(2));
        for pair in buckets.chunks(2) {
            let first = pair[0];
            let second = pair.get(1).copied().unwrap_or(first);
            next.push(GpuSignalSummaryBucket {
                min: first.min.min(second.min),
                max: first.max.max(second.max),
            });
        }
        buckets = next;
        bucket_frames = bucket_frames.saturating_mul(2).max(bucket_frames + 1);
    }

    GpuSignalSummary {
        frames,
        band_count: 1,
        levels,
    }
}

#[cfg(test)]
mod tests {
    use super::{PeakReducer, summary_from_peaks};

    #[test]
    fn reducer_preserves_extrema_with_bounded_storage() {
        let mut reducer = PeakReducer::new(2, None);
        for index in 0..100 {
            reducer.add((-(index as f32 / 100.0), index as f32 / 100.0));
            assert!(reducer.buckets.len() <= 2);
        }
        let peaks = reducer.finish();
        assert_eq!(peaks.len(), 2);
        let minimum = peaks.iter().map(|peak| peak.0).fold(1.0, f32::min);
        let maximum = peaks.iter().map(|peak| peak.1).fold(-1.0, f32::max);
        assert_eq!(minimum, -0.99);
        assert_eq!(maximum, 0.99);
    }

    #[test]
    fn reducer_uses_declared_frame_count_for_uniform_target_capacity() {
        let mut reducer = PeakReducer::new(4, Some(10));
        for index in 0..10 {
            reducer.add((index as f32, index as f32));
        }
        assert_eq!(reducer.buckets.len(), 4);
        assert_eq!(reducer.finish().len(), 4);
    }

    #[test]
    fn summary_preserves_each_peak_window_and_builds_a_pyramid() {
        let summary = summary_from_peaks(&[(-0.8, 0.4), (-0.2, 0.9), (-1.0, 0.7)]);
        assert_eq!(summary.frames, 3);
        assert_eq!(summary.band_count, 1);
        assert_eq!(summary.levels[0].bucket_frames, 1);
        assert_eq!(summary.levels[0].buckets.len(), 3);
        assert_eq!(summary.levels[0].buckets[0].min, -0.8);
        assert_eq!(summary.levels[0].buckets[1].max, 0.9);
        assert_eq!(summary.levels[1].bucket_frames, 2);
        assert_eq!(summary.levels[1].buckets[0].min, -0.8);
        assert_eq!(summary.levels[1].buckets[0].max, 0.9);
    }
}

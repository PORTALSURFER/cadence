//! Host-controlled audition playback for the native Cadence review surface.
//!
//! The Radiant reducer only sends small, generation-tagged commands and reads a
//! non-blocking snapshot. Output setup, decoder construction, and transport
//! control are owned by this host module. Rodio/CPAL may still pull decoder
//! data and service internal control state from the output callback, so this is
//! intentionally not a lock-free realtime or sample-accurate audio engine.

use rodio::{Decoder, DeviceSinkBuilder, Player, Source, source::SeekError};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    fs::File,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

const COMMAND_CAPACITY: usize = 32;
const CONTROL_INTERVAL: Duration = Duration::from_millis(8);
pub const CONTROLS_BUSY_ERROR: &str = "Audio controls are busy — try again shortly.";
pub const DEFAULT_VOLUME: f32 = 0.8;
pub const MAX_OUTPUT_GAIN: f32 = 16.0;

pub const LIVE_SPECTROGRAM_BAND_COUNT: usize = 128;
pub const LIVE_SPECTROGRAM_MAX_HISTORY: usize = 192;

const LIVE_CAPTURE_RING_CAPACITY: usize = 16_384;
const LIVE_SPECTRUM_FFT_SIZE: usize = 2_048;
const LIVE_SPECTRUM_HOP_SIZE: usize = 512;
pub(crate) const LIVE_SPECTRUM_DISPLAY_MIN_FREQUENCY: f32 = 20.0;
pub(crate) const LIVE_SPECTRUM_DISPLAY_MAX_FREQUENCY: f32 = 20_000.0;
pub(crate) const LIVE_SPECTRUM_DISPLAY_FLOOR_DB: f32 = -90.0;
pub(crate) const LIVE_SPECTRUM_DISPLAY_CEILING_DB: f32 = 0.0;
pub(crate) const LIVE_SPECTRUM_DISPLAY_TILT_DB_PER_OCTAVE: f32 = 4.5;
pub(crate) const LIVE_SPECTRUM_DISPLAY_TILT_REFERENCE_FREQUENCY: f32 = 1_000.0;
const LIVE_SPECTRUM_ATTACK_TIME: Duration = Duration::from_millis(60);
const LIVE_SPECTRUM_RELEASE_TIME: Duration = Duration::from_millis(240);
const LIVE_PUBLICATION_INTERVAL: Duration = Duration::from_millis(17);
const LIVE_ANALYZER_POLL_INTERVAL: Duration = Duration::from_millis(2);
static NEXT_LIVE_GPU_REVISION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveSpectrogramFrame {
    pub generation: u64,
    pub epoch: u64,
    pub revision: u64,
    pub sample_rate: u32,
    pub row_count: usize,
    pub values: Arc<[u8]>,
    /// The latest display row after the spectrum-only attack/release smoothing.
    pub spectrum_values: Arc<[u8]>,
    packed_values: Arc<[u8]>,
    gpu_revision: u64,
}

impl LiveSpectrogramFrame {
    fn new(
        generation: u64,
        epoch: u64,
        revision: u64,
        sample_rate: u32,
        rows: &[[u8; LIVE_SPECTROGRAM_BAND_COUNT]; LIVE_SPECTROGRAM_MAX_HISTORY],
        row_count: usize,
        spectrum_values: &[u8; LIVE_SPECTROGRAM_BAND_COUNT],
    ) -> Option<Self> {
        if sample_rate == 0 || row_count == 0 || row_count > LIVE_SPECTROGRAM_MAX_HISTORY {
            return None;
        }
        let mut values = Vec::with_capacity(row_count * LIVE_SPECTROGRAM_BAND_COUNT);
        for row in rows.iter().take(row_count) {
            values.extend_from_slice(row);
        }
        Self::from_values(
            generation,
            epoch,
            revision,
            sample_rate,
            row_count,
            Arc::from(values.into_boxed_slice()),
            Arc::from(spectrum_values.to_vec().into_boxed_slice()),
        )
    }

    pub(crate) fn from_values(
        generation: u64,
        epoch: u64,
        revision: u64,
        sample_rate: u32,
        row_count: usize,
        values: Arc<[u8]>,
        spectrum_values: Arc<[u8]>,
    ) -> Option<Self> {
        if sample_rate == 0
            || row_count == 0
            || row_count > LIVE_SPECTROGRAM_MAX_HISTORY
            || values.len() != row_count * LIVE_SPECTROGRAM_BAND_COUNT
            || spectrum_values.len() != LIVE_SPECTROGRAM_BAND_COUNT
        {
            return None;
        }
        Some(Self {
            generation,
            epoch,
            revision,
            sample_rate,
            row_count,
            packed_values: pack_u8_samples(&values),
            values,
            spectrum_values,
            gpu_revision: NEXT_LIVE_GPU_REVISION.fetch_add(1, Ordering::Relaxed),
        })
    }

    #[allow(dead_code)]
    pub fn value(&self, row: usize, band: usize) -> u8 {
        if row >= self.row_count || band >= LIVE_SPECTROGRAM_BAND_COUNT {
            return 0;
        }
        self.values
            .get(row * LIVE_SPECTROGRAM_BAND_COUNT + band)
            .copied()
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub fn spectrum_value(&self, band: usize) -> u8 {
        self.spectrum_values.get(band).copied().unwrap_or_default()
    }

    #[allow(dead_code)]
    pub(crate) fn packed_values(&self) -> &Arc<[u8]> {
        &self.packed_values
    }

    #[allow(dead_code)]
    pub(crate) fn gpu_revision(&self) -> u64 {
        self.gpu_revision
    }

    pub fn is_valid(&self) -> bool {
        self.sample_rate > 0
            && self.row_count > 0
            && self.row_count <= LIVE_SPECTROGRAM_MAX_HISTORY
            && self.values.len() == self.row_count * LIVE_SPECTROGRAM_BAND_COUNT
            && self.spectrum_values.len() == LIVE_SPECTROGRAM_BAND_COUNT
            && self.packed_values.len() == self.values.len().div_ceil(4) * 4
    }
}

fn pack_u8_samples(values: &[u8]) -> Arc<[u8]> {
    let mut words = vec![0_u32; values.len().div_ceil(4)];
    for (index, &value) in values.iter().enumerate() {
        words[index / 4] |= u32::from(value) << ((index % 4) * 8);
    }
    let mut packed = Vec::with_capacity(words.len() * std::mem::size_of::<u32>());
    for word in words {
        packed.extend_from_slice(&word.to_le_bytes());
    }
    Arc::from(packed.into_boxed_slice())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveFrameState {
    pub generation: u64,
    pub epoch: u64,
    pub revision: u64,
    pub pending: bool,
}

#[derive(Clone, Copy, Debug)]
struct CaptureFrame {
    sample: f32,
    epoch: u64,
}

#[derive(Debug)]
struct LiveCaptureSession {
    generation: u64,
    id: u64,
    epoch: AtomicU64,
    active: AtomicBool,
    analysis_frozen: AtomicBool,
    discontinuity_marked: AtomicBool,
}

impl LiveCaptureSession {
    fn new(generation: u64, id: u64, epoch: u64) -> Self {
        Self {
            generation,
            id,
            epoch: AtomicU64::new(epoch),
            active: AtomicBool::new(true),
            analysis_frozen: AtomicBool::new(true),
            discontinuity_marked: AtomicBool::new(false),
        }
    }

    fn current_epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    fn mark_discontinuity(&self, shared: &SharedSnapshot) -> u64 {
        self.discontinuity_marked.store(false, Ordering::Release);
        let epoch = self.epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        shared.live_epoch.store(epoch, Ordering::Release);
        epoch
    }

    fn mark_capture_drop(&self, shared: &SharedSnapshot) {
        if !self.discontinuity_marked.swap(true, Ordering::AcqRel) {
            self.mark_discontinuity(shared);
            self.discontinuity_marked.store(true, Ordering::Release);
        }
    }

    fn retire(&self) {
        self.analysis_frozen.store(true, Ordering::Release);
        self.active.store(false, Ordering::Release);
        self.discontinuity_marked.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct LiveComplexSample {
    real: f32,
    imaginary: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct LiveBandRange {
    start: usize,
    end: usize,
    start_frequency: f32,
    end_frequency: f32,
}

impl LiveBandRange {
    fn center_frequency(self) -> f32 {
        (self.start_frequency * self.end_frequency).sqrt()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub generation: u64,
    pub acknowledged_token: u64,
    pub position_millis: u64,
    pub playing: bool,
    pub ready: bool,
}

#[derive(Debug)]
struct SharedSnapshot {
    generation: AtomicU64,
    requested_generation: AtomicU64,
    acknowledged_token: AtomicU64,
    position_millis: AtomicU64,
    playing: AtomicBool,
    ready: AtomicBool,
    requested_volume: AtomicU32,
    error_available: AtomicBool,
    error: Mutex<Option<(u64, String)>>,
    analysis_warning_available: AtomicBool,
    analysis_warning: Mutex<Option<(u64, String)>>,
    live_session_id: AtomicU64,
    next_live_session_id: AtomicU64,
    live_epoch: AtomicU64,
    live_revision: AtomicU64,
    live_pending: AtomicBool,
    live_frame: Mutex<Option<Arc<LiveSpectrogramFrame>>>,
    live_session: Mutex<Option<Weak<LiveCaptureSession>>>,
}

impl SharedSnapshot {
    fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            requested_generation: AtomicU64::new(0),
            acknowledged_token: AtomicU64::new(0),
            position_millis: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            requested_volume: AtomicU32::new(DEFAULT_VOLUME.to_bits()),
            error_available: AtomicBool::new(false),
            error: Mutex::new(None),
            analysis_warning_available: AtomicBool::new(false),
            analysis_warning: Mutex::new(None),
            live_session_id: AtomicU64::new(0),
            next_live_session_id: AtomicU64::new(1),
            live_epoch: AtomicU64::new(0),
            live_revision: AtomicU64::new(0),
            live_pending: AtomicBool::new(false),
            live_frame: Mutex::new(None),
            live_session: Mutex::new(None),
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            generation: self.generation.load(Ordering::Acquire),
            acknowledged_token: self.acknowledged_token.load(Ordering::Acquire),
            position_millis: self.position_millis.load(Ordering::Acquire),
            playing: self.playing.load(Ordering::Acquire),
            ready: self.ready.load(Ordering::Acquire),
        }
    }

    fn set_error(&self, generation: u64, error: String) {
        if let Ok(mut slot) = self.error.lock() {
            *slot = Some((generation, error));
            self.error_available.store(true, Ordering::Release);
        }
    }

    fn acknowledge(&self, token: u64) {
        let mut observed = self.acknowledged_token.load(Ordering::Acquire);
        while token > observed {
            match self.acknowledged_token.compare_exchange_weak(
                observed,
                token,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => observed = current,
            }
        }
    }

    fn take_error(&self, generation: u64) -> Option<String> {
        if !self.error_available.load(Ordering::Acquire) {
            return None;
        }
        let mut slot = self.error.try_lock().ok()?;
        match slot.take() {
            Some((error_generation, error)) if error_generation == generation => {
                self.error_available.store(false, Ordering::Release);
                Some(error)
            }
            Some(_) | None => {
                self.error_available.store(false, Ordering::Release);
                None
            }
        }
    }

    fn set_analysis_warning(&self, generation: u64, warning: String) {
        if let Ok(mut slot) = self.analysis_warning.lock() {
            *slot = Some((generation, warning));
            self.analysis_warning_available
                .store(true, Ordering::Release);
        }
    }

    fn take_analysis_warning(&self, generation: u64) -> Option<String> {
        if !self.analysis_warning_available.load(Ordering::Acquire) {
            return None;
        }
        let mut slot = self.analysis_warning.try_lock().ok()?;
        match slot.take() {
            Some((warning_generation, warning)) if warning_generation == generation => {
                self.analysis_warning_available
                    .store(false, Ordering::Release);
                Some(warning)
            }
            Some(_) | None => {
                self.analysis_warning_available
                    .store(false, Ordering::Release);
                None
            }
        }
    }

    fn requested_volume(&self) -> f32 {
        normalize_output_gain(f32::from_bits(
            self.requested_volume.load(Ordering::Acquire),
        ))
    }

    fn clear_live_frame(&self) {
        if let Ok(mut frame) = self.live_frame.lock() {
            *frame = None;
        }
        self.live_revision.store(0, Ordering::Release);
        self.live_pending.store(false, Ordering::Release);
    }

    fn begin_live_session(&self, session: &Arc<LiveCaptureSession>) {
        self.live_session_id.store(session.id, Ordering::Release);
        self.live_epoch
            .store(session.current_epoch(), Ordering::Release);
        self.live_revision.store(0, Ordering::Release);
        self.live_pending.store(false, Ordering::Release);
        if let Ok(mut current) = self.live_session.lock() {
            *current = Some(Arc::downgrade(session));
        }
        self.clear_live_frame();
    }

    fn set_live_analysis_frozen(&self, session: &LiveCaptureSession, frozen: bool) -> bool {
        let Ok(_latest) = self.live_frame.lock() else {
            return false;
        };
        let current = session.active.load(Ordering::Acquire)
            && session.generation == self.requested_generation.load(Ordering::Acquire)
            && session.id == self.live_session_id.load(Ordering::Acquire);
        if !current {
            return false;
        }
        session.analysis_frozen.store(frozen, Ordering::Release);
        true
    }

    fn retire_live_session(&self, session: Option<&Arc<LiveCaptureSession>>) {
        if let Some(session) = session {
            session.retire();
            if self.live_session_id.load(Ordering::Acquire) == session.id {
                self.live_session_id.store(0, Ordering::Release);
                if let Ok(mut current) = self.live_session.lock() {
                    *current = None;
                }
            }
        }
        self.live_epoch.fetch_add(1, Ordering::AcqRel);
        self.clear_live_frame();
    }

    fn reset_live_segment(&self) {
        let session = self
            .live_session
            .lock()
            .ok()
            .and_then(|current| current.as_ref().and_then(Weak::upgrade));
        if let Some(session) = session {
            if session.active.load(Ordering::Acquire) {
                session.mark_discontinuity(self);
            }
        } else {
            self.live_epoch.fetch_add(1, Ordering::AcqRel);
        }
        self.clear_live_frame();
    }

    fn publish_live_frame(
        &self,
        session: &LiveCaptureSession,
        frame: Arc<LiveSpectrogramFrame>,
    ) -> bool {
        let Ok(mut latest) = self.live_frame.try_lock() else {
            return false;
        };
        let current = session.active.load(Ordering::Acquire)
            && !session.analysis_frozen.load(Ordering::Acquire)
            && session.generation == self.requested_generation.load(Ordering::Acquire)
            && session.id == self.live_session_id.load(Ordering::Acquire)
            && session.current_epoch() == frame.epoch;
        if !current || !frame.is_valid() {
            return false;
        }
        self.live_revision.store(frame.revision, Ordering::Release);
        *latest = Some(frame);
        true
    }

    fn latest_live_frame(&self) -> Option<Arc<LiveSpectrogramFrame>> {
        self.live_frame.try_lock().ok()?.as_ref().cloned()
    }

    fn live_frame_state(&self) -> LiveFrameState {
        LiveFrameState {
            generation: self.requested_generation.load(Ordering::Acquire),
            epoch: self.live_epoch.load(Ordering::Acquire),
            revision: self.live_revision.load(Ordering::Acquire),
            pending: self.live_pending.load(Ordering::Acquire),
        }
    }

    fn mark_capture_pending(&self) {
        self.live_pending.store(true, Ordering::Release);
    }

    fn clear_live_frame_for_session(&self, session: &LiveCaptureSession) {
        if session.id != self.live_session_id.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut frame) = self.live_frame.try_lock() {
            *frame = None;
            self.live_revision.store(0, Ordering::Release);
        }
    }
}

/// A rodio source adapter that leaves output samples untouched while copying
/// one mono value per complete interleaved frame into the live-analysis ring.
///
/// The adapter is deliberately limited to fixed arithmetic, atomics, and the
/// SPSC producer in `next()`. In particular, it never performs FFT work,
/// allocation, locking, logging, or blocking I/O. The literal per-sample
/// request is represented by every accepted mono frame; FFT rows are emitted
/// at the bounded 512-frame hop below.
struct LiveAnalysisSource<S> {
    inner: S,
    producer: Producer<CaptureFrame>,
    session: Arc<LiveCaptureSession>,
    shared: Arc<SharedSnapshot>,
    frame_channels: usize,
    frame_channel: usize,
    frame_sum: f32,
}

impl<S> LiveAnalysisSource<S>
where
    S: Source<Item = f32>,
{
    fn new(
        inner: S,
        producer: Producer<CaptureFrame>,
        session: Arc<LiveCaptureSession>,
        shared: Arc<SharedSnapshot>,
    ) -> Self {
        let frame_channels = inner.channels().get() as usize;
        Self {
            inner,
            producer,
            session,
            shared,
            frame_channels: frame_channels.max(1),
            frame_channel: 0,
            frame_sum: 0.0,
        }
    }

    fn reset_frame_accumulator(&mut self) {
        self.frame_channel = 0;
        self.frame_sum = 0.0;
        self.frame_channels = self.inner.channels().get() as usize;
        self.frame_channels = self.frame_channels.max(1);
    }

    fn push_analysis_frame(&mut self, mono: f32) {
        if !self.session.active.load(Ordering::Acquire)
            || self.session.analysis_frozen.load(Ordering::Acquire)
        {
            return;
        }
        // Analysis is intentionally pre-fader: the display follows decoder
        // audio even when the audition output volume changes.
        let sample = if mono.is_finite() { mono } else { 0.0 };
        let frame = CaptureFrame {
            sample,
            epoch: self.session.current_epoch(),
        };
        if self.producer.push(frame).is_ok() {
            self.session
                .discontinuity_marked
                .store(false, Ordering::Release);
            self.shared.mark_capture_pending();
        } else {
            // A full ring drops the analysis copy immediately. The next
            // accepted frame receives a new epoch, so no FFT window can span
            // the omitted audio.
            self.session.mark_capture_drop(&self.shared);
        }
    }
}

impl<S> Iterator for LiveAnalysisSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        let channels = self.inner.channels().get() as usize;
        if channels != self.frame_channels {
            self.frame_channels = channels.max(1);
            self.frame_channel = 0;
            self.frame_sum = 0.0;
        }

        if self.frame_channel == 0 {
            self.frame_sum = sample;
        } else {
            self.frame_sum += sample;
        }
        self.frame_channel += 1;
        if self.frame_channel >= self.frame_channels {
            let mono = if self.frame_sum.is_finite() {
                self.frame_sum / self.frame_channels as f32
            } else {
                0.0
            };
            self.frame_channel = 0;
            self.frame_sum = 0.0;
            self.push_analysis_frame(mono);
        }

        Some(sample)
    }
}

impl<S> Source for LiveAnalysisSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        let result = self.inner.try_seek(position);
        if result.is_ok() {
            self.reset_frame_accumulator();
            self.session.mark_discontinuity(&self.shared);
        }
        result
    }
}

struct LiveAnalyzer {
    sample_rate: u32,
    band_ranges: [LiveBandRange; LIVE_SPECTROGRAM_BAND_COUNT],
    attack_coefficient: f32,
    release_coefficient: f32,
    smoothed_levels: [f32; LIVE_SPECTROGRAM_BAND_COUNT],
    has_smoothed_levels: bool,
    window: [f32; LIVE_SPECTRUM_FFT_SIZE],
    window_len: usize,
    fft: [LiveComplexSample; LIVE_SPECTRUM_FFT_SIZE],
    rows: [[u8; LIVE_SPECTROGRAM_BAND_COUNT]; LIVE_SPECTROGRAM_MAX_HISTORY],
    spectrum_values: [u8; LIVE_SPECTROGRAM_BAND_COUNT],
    row_count: usize,
    revision: u64,
    fft_count: usize,
}

impl LiveAnalyzer {
    fn new(sample_rate: u32) -> Self {
        let sample_rate = sample_rate.max(1);
        Self {
            sample_rate,
            band_ranges: live_band_ranges(sample_rate),
            attack_coefficient: live_ballistic_coefficient(LIVE_SPECTRUM_ATTACK_TIME, sample_rate),
            release_coefficient: live_ballistic_coefficient(
                LIVE_SPECTRUM_RELEASE_TIME,
                sample_rate,
            ),
            smoothed_levels: [0.0; LIVE_SPECTROGRAM_BAND_COUNT],
            has_smoothed_levels: false,
            window: [0.0; LIVE_SPECTRUM_FFT_SIZE],
            window_len: 0,
            fft: [LiveComplexSample::default(); LIVE_SPECTRUM_FFT_SIZE],
            rows: [[0; LIVE_SPECTROGRAM_BAND_COUNT]; LIVE_SPECTROGRAM_MAX_HISTORY],
            spectrum_values: [0; LIVE_SPECTROGRAM_BAND_COUNT],
            row_count: 0,
            revision: 0,
            fft_count: 0,
        }
    }

    fn reset(&mut self) {
        self.smoothed_levels = [0.0; LIVE_SPECTROGRAM_BAND_COUNT];
        self.has_smoothed_levels = false;
        self.window_len = 0;
        self.spectrum_values = [0; LIVE_SPECTROGRAM_BAND_COUNT];
        self.row_count = 0;
        self.revision = 0;
        self.fft_count = 0;
    }

    fn reset_after_pause(&mut self) {
        self.smoothed_levels = [0.0; LIVE_SPECTROGRAM_BAND_COUNT];
        self.has_smoothed_levels = false;
        self.window_len = 0;
        self.spectrum_values = [0; LIVE_SPECTROGRAM_BAND_COUNT];
        self.row_count = 0;
    }

    fn push(&mut self, sample: f32) -> bool {
        self.window[self.window_len] = sample;
        self.window_len += 1;
        if self.window_len < LIVE_SPECTRUM_FFT_SIZE {
            return false;
        }

        self.analyze_window();
        self.window
            .copy_within(LIVE_SPECTRUM_HOP_SIZE..LIVE_SPECTRUM_FFT_SIZE, 0);
        self.window_len = LIVE_SPECTRUM_FFT_SIZE - LIVE_SPECTRUM_HOP_SIZE;
        true
    }

    fn analyze_window(&mut self) {
        for (index, (&sample, fft)) in self.window.iter().zip(self.fft.iter_mut()).enumerate() {
            let hann = 0.5
                - 0.5
                    * (std::f32::consts::TAU * index as f32 / LIVE_SPECTRUM_FFT_SIZE as f32).cos();
            *fft = LiveComplexSample {
                real: sample * hann,
                imaginary: 0.0,
            };
        }
        live_fft_in_place(&mut self.fft);

        let mut target_row = [0_u8; LIVE_SPECTROGRAM_BAND_COUNT];
        for (band, range) in self.band_ranges.iter().enumerate() {
            let magnitude = (range.start..range.end)
                .map(|bin| {
                    let sample = self.fft[bin];
                    (sample.real * sample.real + sample.imaginary * sample.imaginary).sqrt()
                        / LIVE_SPECTRUM_FFT_SIZE as f32
                })
                .fold(0.0_f32, f32::max);
            let decibels = 20.0 * magnitude.max(1.0e-8).log10();
            let display_decibels = (decibels + display_tilt_db(range.center_frequency())).clamp(
                LIVE_SPECTRUM_DISPLAY_FLOOR_DB,
                LIVE_SPECTRUM_DISPLAY_CEILING_DB,
            );
            let normalized = ((display_decibels - LIVE_SPECTRUM_DISPLAY_FLOOR_DB)
                / (LIVE_SPECTRUM_DISPLAY_CEILING_DB - LIVE_SPECTRUM_DISPLAY_FLOOR_DB))
                .clamp(0.0, 1.0);
            target_row[band] = (normalized * u8::MAX as f32).round() as u8;
        }
        self.record_analyzed_row(target_row);

        self.revision = self.revision.wrapping_add(1);
        self.fft_count = self.fft_count.saturating_add(1);
    }

    fn record_analyzed_row(&mut self, raw_row: [u8; LIVE_SPECTROGRAM_BAND_COUNT]) {
        self.spectrum_values = self.smooth_row(raw_row);
        if self.row_count < LIVE_SPECTROGRAM_MAX_HISTORY {
            self.rows[self.row_count] = raw_row;
            self.row_count += 1;
        } else {
            self.rows.copy_within(1..LIVE_SPECTROGRAM_MAX_HISTORY, 0);
            self.rows[LIVE_SPECTROGRAM_MAX_HISTORY - 1] = raw_row;
        }
    }

    /// Apply display-only exponential attack/release ballistics. This keeps
    /// the line readable without changing the decoder samples or audio path.
    fn smooth_row(
        &mut self,
        target_row: [u8; LIVE_SPECTROGRAM_BAND_COUNT],
    ) -> [u8; LIVE_SPECTROGRAM_BAND_COUNT] {
        let mut row = [0_u8; LIVE_SPECTROGRAM_BAND_COUNT];
        for (band, &target) in target_row.iter().enumerate() {
            let target = target as f32 / u8::MAX as f32;
            let previous = self.smoothed_levels[band];
            let level = if self.has_smoothed_levels {
                let coefficient = if target > previous {
                    self.attack_coefficient
                } else {
                    self.release_coefficient
                };
                previous + coefficient * (target - previous)
            } else {
                target
            };
            let level = level.clamp(0.0, 1.0);
            self.smoothed_levels[band] = level;
            row[band] = (level * u8::MAX as f32).round() as u8;
        }
        self.has_smoothed_levels = true;
        row
    }

    fn frame(&self, generation: u64, epoch: u64) -> Option<Arc<LiveSpectrogramFrame>> {
        LiveSpectrogramFrame::new(
            generation,
            epoch,
            self.revision,
            self.sample_rate,
            &self.rows,
            self.row_count,
            &self.spectrum_values,
        )
        .map(Arc::new)
    }
}

fn live_ballistic_coefficient(time_constant: Duration, sample_rate: u32) -> f32 {
    let hop_seconds = LIVE_SPECTRUM_HOP_SIZE as f32 / sample_rate.max(1) as f32;
    let time_constant_seconds = time_constant.as_secs_f32().max(f32::EPSILON);
    (1.0 - (-hop_seconds / time_constant_seconds).exp()).clamp(0.0, 1.0)
}

/// Return the analyzer's audible display range after clamping its upper edge
/// to the source Nyquist frequency.
pub(crate) fn live_display_frequency_bounds(sample_rate: u32) -> (f32, f32) {
    let sample_rate = sample_rate.max(1) as f32;
    let nyquist = (sample_rate * 0.5).max(f32::MIN_POSITIVE);
    let minimum = LIVE_SPECTRUM_DISPLAY_MIN_FREQUENCY.min(nyquist);
    let maximum = LIVE_SPECTRUM_DISPLAY_MAX_FREQUENCY
        .min(nyquist)
        .max(minimum);
    (minimum, maximum)
}

/// Apply the display-only analyzer tilt around 1 kHz.
///
/// The positive 4.5 dB/octave sign boosts frequencies above 1 kHz and cuts
/// frequencies below it. The result is only used for the quantized analyzer
/// frame; decoder samples, transport output, and their levels are untouched.
fn display_tilt_db(frequency: f32) -> f32 {
    LIVE_SPECTRUM_DISPLAY_TILT_DB_PER_OCTAVE
        * (frequency / LIVE_SPECTRUM_DISPLAY_TILT_REFERENCE_FREQUENCY).log2()
}

fn live_band_ranges(sample_rate: u32) -> [LiveBandRange; LIVE_SPECTROGRAM_BAND_COUNT] {
    let sample_rate = sample_rate.max(1) as f32;
    let (minimum, maximum) = live_display_frequency_bounds(sample_rate as u32);
    let ratio = (maximum / minimum.max(1.0)).max(1.0);
    let maximum_bin = LIVE_SPECTRUM_FFT_SIZE / 2 + 1;

    std::array::from_fn(|band| {
        let start_frequency =
            minimum * ratio.powf(band as f32 / LIVE_SPECTROGRAM_BAND_COUNT as f32);
        let end_frequency =
            minimum * ratio.powf((band + 1) as f32 / LIVE_SPECTROGRAM_BAND_COUNT as f32);
        let start_bin =
            ((start_frequency / sample_rate) * LIVE_SPECTRUM_FFT_SIZE as f32).floor() as usize;
        let end_bin =
            ((end_frequency / sample_rate) * LIVE_SPECTRUM_FFT_SIZE as f32).ceil() as usize;
        let start = start_bin.clamp(1, maximum_bin.saturating_sub(1));
        let end = end_bin.clamp(start.saturating_add(1), maximum_bin);
        LiveBandRange {
            start,
            end,
            start_frequency,
            end_frequency,
        }
    })
}

fn live_fft_in_place(buffer: &mut [LiveComplexSample; LIVE_SPECTRUM_FFT_SIZE]) {
    let mut reversed = 0usize;
    for index in 1..buffer.len() {
        let mut bit = buffer.len() >> 1;
        while reversed & bit != 0 {
            reversed ^= bit;
            bit >>= 1;
        }
        reversed ^= bit;
        if index < reversed {
            buffer.swap(index, reversed);
        }
    }

    let mut block_length = 2;
    while block_length <= buffer.len() {
        let angle = -std::f32::consts::TAU / block_length as f32;
        let block_rotation = LiveComplexSample {
            real: angle.cos(),
            imaginary: angle.sin(),
        };
        for block_start in (0..buffer.len()).step_by(block_length) {
            let mut rotation = LiveComplexSample {
                real: 1.0,
                imaginary: 0.0,
            };
            for offset in 0..block_length / 2 {
                let even_index = block_start + offset;
                let odd_index = even_index + block_length / 2;
                let odd = buffer[odd_index];
                let rotated = LiveComplexSample {
                    real: odd.real * rotation.real - odd.imaginary * rotation.imaginary,
                    imaginary: odd.real * rotation.imaginary + odd.imaginary * rotation.real,
                };
                let even = buffer[even_index];
                buffer[even_index] = LiveComplexSample {
                    real: even.real + rotated.real,
                    imaginary: even.imaginary + rotated.imaginary,
                };
                buffer[odd_index] = LiveComplexSample {
                    real: even.real - rotated.real,
                    imaginary: even.imaginary - rotated.imaginary,
                };
                rotation = LiveComplexSample {
                    real: rotation.real * block_rotation.real
                        - rotation.imaginary * block_rotation.imaginary,
                    imaginary: rotation.real * block_rotation.imaginary
                        + rotation.imaginary * block_rotation.real,
                };
            }
        }
        block_length <<= 1;
    }
}

fn run_live_analyzer(
    mut consumer: Consumer<CaptureFrame>,
    session: Arc<LiveCaptureSession>,
    shared: Arc<SharedSnapshot>,
    sample_rate: u32,
) {
    let mut analyzer = LiveAnalyzer::new(sample_rate);
    let mut observed_epoch = session.current_epoch();
    let mut published_revision = 0_u64;
    let mut last_publication = Instant::now()
        .checked_sub(LIVE_PUBLICATION_INTERVAL)
        .unwrap_or_else(Instant::now);

    loop {
        if session.current_epoch() != observed_epoch {
            observed_epoch = session.current_epoch();
            analyzer.reset();
            published_revision = 0;
            shared.clear_live_frame_for_session(&session);
        }

        let (consumed, frozen) = run_live_analyzer_iteration(
            &mut consumer,
            &session,
            &shared,
            &mut analyzer,
            &mut observed_epoch,
            &mut published_revision,
            &mut last_publication,
        );

        if frozen {
            let producer_gone = consumer.is_abandoned() && consumer.is_empty();
            if producer_gone {
                break;
            }
            if !consumed {
                thread::sleep(LIVE_ANALYZER_POLL_INTERVAL);
            }
            continue;
        }

        let producer_gone = consumer.is_abandoned() && consumer.is_empty();
        let retired = !session.active.load(Ordering::Acquire);
        if producer_gone && consumer.is_empty() {
            if !retired && analyzer.revision > published_revision {
                let elapsed = last_publication.elapsed();
                if elapsed < LIVE_PUBLICATION_INTERVAL {
                    thread::sleep(LIVE_PUBLICATION_INTERVAL - elapsed);
                }
                publish_live_frame_if_due(
                    &analyzer,
                    &session,
                    &shared,
                    observed_epoch,
                    &mut published_revision,
                    &mut last_publication,
                    true,
                );
            }
            break;
        }

        if !consumed {
            thread::sleep(LIVE_ANALYZER_POLL_INTERVAL);
        }
    }
    session.retire();
    if session.id == shared.live_session_id.load(Ordering::Acquire) {
        shared.live_pending.store(false, Ordering::Release);
    }
}

fn discard_live_capture(
    consumer: &mut Consumer<CaptureFrame>,
    session: &LiveCaptureSession,
    shared: &SharedSnapshot,
) -> bool {
    let mut discarded = false;
    while consumer.pop().is_ok() {
        discarded = true;
    }
    if session.id == shared.live_session_id.load(Ordering::Acquire) && consumer.is_empty() {
        shared.live_pending.store(false, Ordering::Release);
    }
    discarded
}

fn run_live_analyzer_iteration(
    consumer: &mut Consumer<CaptureFrame>,
    session: &LiveCaptureSession,
    shared: &SharedSnapshot,
    analyzer: &mut LiveAnalyzer,
    observed_epoch: &mut u64,
    published_revision: &mut u64,
    last_publication: &mut Instant,
) -> (bool, bool) {
    let current_epoch = session.current_epoch();
    if current_epoch != *observed_epoch {
        *observed_epoch = current_epoch;
        analyzer.reset();
        *published_revision = 0;
        shared.clear_live_frame_for_session(session);
    }

    if session.analysis_frozen.load(Ordering::Acquire) {
        let discarded = discard_live_capture(consumer, session, shared);
        // Work computed before the pause is intentionally not published after
        // it. Keep the last displayed frame at the pause boundary, but reset
        // any partial rolling window so resumed audio cannot bridge the pause
        // gap with discarded capture.
        if discarded || analyzer.window_len > 0 {
            analyzer.reset_after_pause();
        }
        *published_revision = analyzer.revision;
        return (discarded, true);
    }

    let mut consumed = false;
    while let Ok(capture) = consumer.pop() {
        consumed = true;
        if session.analysis_frozen.load(Ordering::Acquire) {
            let discarded = discard_live_capture(consumer, session, shared);
            if discarded || analyzer.window_len > 0 {
                analyzer.reset_after_pause();
            }
            *published_revision = analyzer.revision;
            return (consumed || discarded, true);
        }

        let current_epoch = session.current_epoch();
        if current_epoch != *observed_epoch {
            *observed_epoch = current_epoch;
            analyzer.reset();
            *published_revision = 0;
            shared.clear_live_frame_for_session(session);
        }
        if capture.epoch != *observed_epoch {
            continue;
        }
        if analyzer.push(capture.sample) {
            publish_live_frame_if_due(
                analyzer,
                session,
                shared,
                *observed_epoch,
                published_revision,
                last_publication,
                false,
            );
        }

        if session.analysis_frozen.load(Ordering::Acquire) {
            let discarded = discard_live_capture(consumer, session, shared);
            if discarded || analyzer.window_len > 0 {
                analyzer.reset_after_pause();
            }
            *published_revision = analyzer.revision;
            return (consumed || discarded, true);
        }
    }

    if session.analysis_frozen.load(Ordering::Acquire) {
        let discarded = discard_live_capture(consumer, session, shared);
        if discarded || analyzer.window_len > 0 {
            analyzer.reset_after_pause();
        }
        *published_revision = analyzer.revision;
        return (consumed || discarded, true);
    }

    if analyzer.revision > *published_revision {
        publish_live_frame_if_due(
            analyzer,
            session,
            shared,
            *observed_epoch,
            published_revision,
            last_publication,
            false,
        );
    }

    if consumer.is_empty()
        && analyzer.revision <= *published_revision
        && session.id == shared.live_session_id.load(Ordering::Acquire)
    {
        shared.live_pending.store(false, Ordering::Release);
    }
    (consumed, false)
}

fn publish_live_frame_if_due(
    analyzer: &LiveAnalyzer,
    session: &LiveCaptureSession,
    shared: &SharedSnapshot,
    observed_epoch: u64,
    published_revision: &mut u64,
    last_publication: &mut Instant,
    force: bool,
) {
    if analyzer.revision <= *published_revision {
        return;
    }
    if !force && last_publication.elapsed() < LIVE_PUBLICATION_INTERVAL {
        return;
    }
    if session.current_epoch() != observed_epoch {
        return;
    }
    let Some(frame) = analyzer.frame(session.generation, observed_epoch) else {
        return;
    };
    if session.current_epoch() != observed_epoch {
        return;
    }
    if shared.publish_live_frame(session, frame) {
        *published_revision = analyzer.revision;
        *last_publication = Instant::now();
    }
}

#[derive(Clone, Debug)]
enum Command {
    Load {
        token: u64,
        generation: u64,
        path: PathBuf,
        duration_millis: u64,
    },
    Unload {
        token: u64,
        generation: u64,
    },
    Play {
        token: u64,
        generation: u64,
    },
    Pause {
        token: u64,
        generation: u64,
    },
    Seek {
        token: u64,
        generation: u64,
        position_millis: u64,
        resume: bool,
    },
}

impl Command {
    fn load_generation(&self) -> Option<u64> {
        match self {
            Self::Load { generation, .. } => Some(*generation),
            Self::Unload { .. } | Self::Play { .. } | Self::Pause { .. } | Self::Seek { .. } => {
                None
            }
        }
    }
}

/// Single-slot, latest-wins admission for a load command when the bounded
/// control queue is full. Atomic pointer ownership keeps the UI path
/// non-blocking; only one heap allocation is retained at a time.
#[derive(Debug)]
struct PendingLoad {
    pointer: std::sync::atomic::AtomicPtr<Command>,
}

// SAFETY: Command is Send, and ownership of each boxed command moves through
// the atomic pointer exactly once via swap before it is reclaimed.
unsafe impl Send for PendingLoad {}
unsafe impl Sync for PendingLoad {}

impl PendingLoad {
    fn new() -> Self {
        Self {
            pointer: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    fn replace(&self, command: Command) {
        let replacement = Box::into_raw(Box::new(command));
        let previous = self.pointer.swap(replacement, Ordering::AcqRel);
        if !previous.is_null() {
            // SAFETY: the swap transfers exclusive ownership of the previous
            // allocation to this thread.
            unsafe { drop(Box::from_raw(previous)) };
        }
    }

    fn take(&self) -> Option<Command> {
        let pointer = self.pointer.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if pointer.is_null() {
            None
        } else {
            // SAFETY: the swap transfers exclusive ownership of this
            // allocation to this thread.
            Some(unsafe { *Box::from_raw(pointer) })
        }
    }

    fn is_pending(&self) -> bool {
        !self.pointer.load(Ordering::Acquire).is_null()
    }

    fn clear_generation(&self, generation: u64) {
        let Some(command) = self.take() else {
            return;
        };
        if command.load_generation() != Some(generation) {
            self.replace(command);
        }
    }
}

impl Drop for PendingLoad {
    fn drop(&mut self) {
        let pointer = self.pointer.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !pointer.is_null() {
            // SAFETY: Drop has exclusive access to the slot.
            unsafe { drop(Box::from_raw(pointer)) };
        }
    }
}

#[derive(Clone, Debug)]
pub struct AudioTransport {
    commands: SyncSender<Command>,
    queued_commands: Arc<AtomicUsize>,
    shared: Arc<SharedSnapshot>,
    pending_load: Arc<PendingLoad>,
    next_token: Arc<AtomicU64>,
}

impl AudioTransport {
    pub fn spawn() -> Self {
        let (commands, receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let queued_commands = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(SharedSnapshot::new());
        let pending_load = Arc::new(PendingLoad::new());
        let thread_queued_commands = Arc::clone(&queued_commands);
        let thread_shared = Arc::clone(&shared);
        let thread_pending_load = Arc::clone(&pending_load);
        thread::Builder::new()
            .name(String::from("cadence-audio-transport"))
            .spawn(move || {
                run_transport(
                    receiver,
                    thread_queued_commands,
                    thread_shared,
                    thread_pending_load,
                )
            })
            .expect("Cadence audio transport thread should spawn");
        Self {
            commands,
            queued_commands,
            shared,
            pending_load,
            next_token: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot()
    }

    pub fn latest_live_frame(&self) -> Option<Arc<LiveSpectrogramFrame>> {
        self.shared.latest_live_frame()
    }

    pub fn live_frame_state(&self) -> LiveFrameState {
        self.shared.live_frame_state()
    }

    pub fn live_analysis_pending(&self) -> bool {
        self.shared.live_frame_state().pending
    }

    pub fn clear_live_frame(&self) {
        self.shared.clear_live_frame();
    }

    pub fn reset_live_segment(&self) {
        self.shared.reset_live_segment();
    }

    pub fn take_error(&self, generation: u64) -> Option<String> {
        self.shared.take_error(generation)
    }

    pub fn take_analysis_warning(&self, generation: u64) -> Option<String> {
        self.shared.take_analysis_warning(generation)
    }

    #[cfg(test)]
    pub(crate) fn set_error_for_test(&self, generation: u64, error: String) {
        self.shared.set_error(generation, error);
    }

    #[cfg(test)]
    pub(crate) fn set_analysis_warning_for_test(&self, generation: u64, warning: String) {
        self.shared.set_analysis_warning(generation, warning);
    }

    #[cfg(test)]
    pub(crate) fn set_snapshot_for_test(&self, snapshot: Snapshot) {
        self.shared
            .generation
            .store(snapshot.generation, Ordering::Release);
        self.shared
            .acknowledged_token
            .store(snapshot.acknowledged_token, Ordering::Release);
        self.shared
            .position_millis
            .store(snapshot.position_millis, Ordering::Release);
        self.shared
            .playing
            .store(snapshot.playing, Ordering::Release);
        self.shared.ready.store(snapshot.ready, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn set_live_state_for_test(&self, state: LiveFrameState) {
        self.shared.live_epoch.store(state.epoch, Ordering::Release);
        self.shared
            .live_revision
            .store(state.revision, Ordering::Release);
        self.shared
            .live_pending
            .store(state.pending, Ordering::Release);
    }

    /// Set an output gain for a comparison source such as the reference
    /// track. This is separate from the user's 0–1 audition slider so a
    /// loudness-match offset can boost a quiet reference without changing the
    /// primary track's control value or raw LUFS analysis.
    pub fn set_output_gain(&self, gain: f32) {
        self.shared
            .requested_volume
            .store(normalize_output_gain(gain).to_bits(), Ordering::Release);
    }

    pub(crate) fn has_command_capacity(&self, required: usize) -> bool {
        required <= COMMAND_CAPACITY
            && self.queued_commands.load(Ordering::Acquire) <= COMMAND_CAPACITY - required
    }

    pub(crate) fn has_pending_load(&self) -> bool {
        self.pending_load.is_pending()
    }

    #[cfg(test)]
    pub(crate) fn force_command_queue_full_for_test(&self) {
        self.queued_commands
            .store(COMMAND_CAPACITY, Ordering::Release);
    }

    pub fn load(
        &self,
        generation: u64,
        path: PathBuf,
        duration_millis: u64,
    ) -> Result<u64, String> {
        self.shared
            .requested_generation
            .store(generation, Ordering::Release);
        let token = self.next_token();
        let command = Command::Load {
            token,
            generation,
            path,
            duration_millis,
        };
        if !self.try_reserve_command_slot() {
            self.store_pending_load(command)?;
            return Ok(token);
        }
        match self.commands.try_send(command) {
            Ok(()) => {
                self.clear_pending_load(generation);
                Ok(token)
            }
            // The transport thread will pick up the latest load intent from
            // the coalescing slot on its next control tick.
            Err(TrySendError::Full(command)) => {
                self.release_command_slot();
                self.store_pending_load(command)?;
                Ok(token)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.release_command_slot();
                self.clear_pending_load(generation);
                Err(String::from("The audio transport is no longer available."))
            }
        }
    }

    pub fn unload(&self, generation: u64) -> Result<u64, String> {
        self.shared
            .requested_generation
            .store(generation, Ordering::Release);
        let token = self.next_token();
        self.try_send(Command::Unload { token, generation })
            .map(|()| token)
    }

    pub fn play(&self, generation: u64) -> Result<u64, String> {
        if self.has_pending_load() {
            return Err(String::from(CONTROLS_BUSY_ERROR));
        }
        let token = self.next_token();
        self.try_send(Command::Play { token, generation })
            .map(|()| token)
    }

    pub fn pause(&self, generation: u64) -> Result<u64, String> {
        let token = self.next_token();
        self.try_send(Command::Pause { token, generation })
            .map(|()| token)
    }

    pub fn seek(
        &self,
        generation: u64,
        position_millis: u64,
        duration_millis: u64,
        resume: bool,
    ) -> Result<u64, String> {
        if self.has_pending_load() {
            return Err(String::from(CONTROLS_BUSY_ERROR));
        }
        let token = self.next_token();
        self.try_send(Command::Seek {
            token,
            generation,
            position_millis: clamp_position(position_millis, duration_millis),
            resume,
        })
        .map(|()| token)
    }

    fn try_send(&self, command: Command) -> Result<(), String> {
        if !self.try_reserve_command_slot() {
            return Err(String::from(CONTROLS_BUSY_ERROR));
        }
        match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.release_command_slot();
                Err(String::from(CONTROLS_BUSY_ERROR))
            }
            Err(TrySendError::Disconnected(_)) => {
                self.release_command_slot();
                Err(String::from("The audio transport is no longer available."))
            }
        }
    }

    fn next_token(&self) -> u64 {
        self.next_token.fetch_add(1, Ordering::Relaxed)
    }

    fn try_reserve_command_slot(&self) -> bool {
        let mut queued = self.queued_commands.load(Ordering::Acquire);
        loop {
            if queued >= COMMAND_CAPACITY {
                return false;
            }
            match self.queued_commands.compare_exchange_weak(
                queued,
                queued + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => queued = current,
            }
        }
    }

    fn release_command_slot(&self) {
        let previous = self.queued_commands.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
    }

    fn store_pending_load(&self, command: Command) -> Result<(), String> {
        self.pending_load.replace(command);
        Ok(())
    }

    fn clear_pending_load(&self, generation: u64) {
        self.pending_load.clear_generation(generation);
    }
}

#[derive(Clone, Debug)]
struct LoadedTrack {
    generation: u64,
    path: PathBuf,
    duration_millis: u64,
}

#[allow(clippy::too_many_arguments)]
fn finish_loaded_track<S>(
    shared: &SharedSnapshot,
    generation: u64,
    path: PathBuf,
    duration_millis: u64,
    player_handle: Player,
    source: S,
    analysis_session: Option<Arc<LiveCaptureSession>>,
    player: &mut Option<Player>,
    loaded: &mut Option<LoadedTrack>,
    live_session: &mut Option<Arc<LiveCaptureSession>>,
) where
    S: Source<Item = f32> + Send + 'static,
{
    player_handle.append(source);
    player_handle.set_volume(shared.requested_volume());
    player_handle.pause();
    *player = Some(player_handle);
    *loaded = Some(LoadedTrack {
        generation,
        path,
        duration_millis,
    });
    *live_session = analysis_session;
    shared.ready.store(true, Ordering::Release);
}

fn run_transport(
    receiver: Receiver<Command>,
    queued_commands: Arc<AtomicUsize>,
    shared: Arc<SharedSnapshot>,
    pending_load: Arc<PendingLoad>,
) {
    let output = match DeviceSinkBuilder::open_default_sink() {
        Ok(output) => {
            let mut output = output;
            output.log_on_drop(false);
            Some(output)
        }
        Err(error) => {
            shared.set_error(
                0,
                format!("Could not open the default audio output: {error}"),
            );
            None
        }
    };
    let mut player: Option<Player> = None;
    let mut loaded: Option<LoadedTrack> = None;
    let mut live_session: Option<Arc<LiveCaptureSession>> = None;
    let mut applied_volume = None;

    loop {
        if let Some(command) = take_pending_load(&pending_load) {
            handle_command(
                command,
                &shared,
                output.as_ref(),
                &mut player,
                &mut loaded,
                &mut live_session,
            );
        }
        match receiver.recv_timeout(CONTROL_INTERVAL) {
            Ok(command) => {
                release_command_slot(&queued_commands);
                handle_command(
                    command,
                    &shared,
                    output.as_ref(),
                    &mut player,
                    &mut loaded,
                    &mut live_session,
                )
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        loop {
            match receiver.try_recv() {
                Ok(command) => {
                    release_command_slot(&queued_commands);
                    handle_command(
                        command,
                        &shared,
                        output.as_ref(),
                        &mut player,
                        &mut loaded,
                        &mut live_session,
                    )
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    retire_live_session(&shared, &mut live_session);
                    return;
                }
            }
        }

        reconcile_stale_track(&shared, &mut player, &mut loaded, &mut live_session);
        apply_requested_volume(&shared, player.as_ref(), &mut applied_volume);
        publish_snapshot(&shared, player.as_ref(), loaded.as_ref());
    }

    retire_live_session(&shared, &mut live_session);
    drop(player);
    drop(loaded);
    drop(output);
}

fn release_command_slot(queued_commands: &AtomicUsize) {
    let previous = queued_commands.fetch_sub(1, Ordering::AcqRel);
    debug_assert!(previous > 0);
}

fn take_pending_load(pending_load: &PendingLoad) -> Option<Command> {
    pending_load.take()
}

fn reconcile_stale_track(
    shared: &SharedSnapshot,
    player: &mut Option<Player>,
    loaded: &mut Option<LoadedTrack>,
    live_session: &mut Option<Arc<LiveCaptureSession>>,
) {
    let requested_generation = shared.requested_generation.load(Ordering::Acquire);
    if loaded
        .as_ref()
        .is_some_and(|track| track.generation != requested_generation)
    {
        retire_live_session(shared, live_session);
        *player = None;
        *loaded = None;
        shared
            .generation
            .store(requested_generation, Ordering::Release);
        shared.position_millis.store(0, Ordering::Release);
        shared.playing.store(false, Ordering::Release);
        shared.ready.store(false, Ordering::Release);
    }
}

fn handle_command(
    command: Command,
    shared: &Arc<SharedSnapshot>,
    output: Option<&rodio::MixerDeviceSink>,
    player: &mut Option<Player>,
    loaded: &mut Option<LoadedTrack>,
    live_session: &mut Option<Arc<LiveCaptureSession>>,
) {
    let (token, acknowledged) = match command {
        Command::Load {
            token,
            generation,
            path,
            duration_millis,
        } => (
            token,
            load_track(
                generation,
                path,
                duration_millis,
                shared,
                output,
                player,
                loaded,
                live_session,
            ),
        ),
        Command::Unload { token, generation } => {
            if !is_current(shared, generation) {
                (token, false)
            } else {
                retire_live_session(shared, live_session);
                *player = None;
                *loaded = None;
                shared.generation.store(generation, Ordering::Release);
                shared.position_millis.store(0, Ordering::Release);
                shared.playing.store(false, Ordering::Release);
                shared.ready.store(false, Ordering::Release);
                (token, true)
            }
        }
        Command::Play { token, generation } => {
            if !is_current(shared, generation) {
                (token, false)
            } else {
                let reloaded = if let Some(track) = loaded.clone()
                    && player.as_ref().is_some_and(Player::empty)
                {
                    load_track(
                        track.generation,
                        track.path,
                        track.duration_millis,
                        shared,
                        output,
                        player,
                        loaded,
                        live_session,
                    )
                } else {
                    true
                };
                if !reloaded {
                    (token, false)
                } else {
                    if let Some(player) = player.as_ref()
                        && loaded
                            .as_ref()
                            .is_some_and(|track| track.generation == generation)
                    {
                        if let Some(session) = live_session.as_deref() {
                            shared.set_live_analysis_frozen(session, false);
                        }
                        player.play();
                        shared.playing.store(true, Ordering::Release);
                    }
                    (token, true)
                }
            }
        }
        Command::Pause { token, generation } => {
            if !is_current(shared, generation) {
                (token, false)
            } else {
                if let Some(session) = live_session.as_deref() {
                    // Freeze publication before pausing the player so any
                    // decoder frames already queued at this command boundary
                    // are drained without advancing the visible display.
                    shared.set_live_analysis_frozen(session, true);
                }
                if let Some(player) = player.as_ref() {
                    player.pause();
                }
                shared.playing.store(false, Ordering::Release);
                (token, true)
            }
        }
        Command::Seek {
            token,
            generation,
            position_millis,
            resume,
        } => {
            if !is_current(shared, generation) {
                (token, false)
            } else {
                match loaded.clone() {
                    None => (token, true),
                    Some(track) if track.generation != generation => (token, false),
                    Some(track) => {
                        let reloaded = if player.as_ref().is_some_and(Player::empty) {
                            load_track(
                                track.generation,
                                track.path,
                                track.duration_millis,
                                shared,
                                output,
                                player,
                                loaded,
                                live_session,
                            )
                        } else {
                            true
                        };
                        if !reloaded {
                            (token, false)
                        } else if let Some(player) = player.as_ref() {
                            let was_frozen = live_session.as_deref().is_none_or(|session| {
                                session.analysis_frozen.load(Ordering::Acquire)
                            });
                            if let Some(session) = live_session.as_deref() {
                                shared.set_live_analysis_frozen(session, true);
                            }
                            if let Err(error) =
                                player.try_seek(Duration::from_millis(position_millis))
                            {
                                if let Some(session) = live_session.as_deref() {
                                    shared.set_live_analysis_frozen(session, was_frozen);
                                }
                                shared.set_error(
                                    generation,
                                    format!("Could not seek this audio file: {error}"),
                                );
                                (token, true)
                            } else {
                                if resume {
                                    if let Some(session) = live_session.as_deref() {
                                        shared.set_live_analysis_frozen(session, false);
                                    }
                                    player.play();
                                } else {
                                    if let Some(session) = live_session.as_deref() {
                                        shared.set_live_analysis_frozen(session, true);
                                    }
                                    player.pause();
                                }
                                shared
                                    .position_millis
                                    .store(position_millis, Ordering::Release);
                                shared.playing.store(resume, Ordering::Release);
                                (token, true)
                            }
                        } else {
                            (token, true)
                        }
                    }
                }
            }
        }
    };
    if acknowledged {
        shared.acknowledge(token);
    }
}

#[allow(clippy::too_many_arguments)]
fn load_track(
    generation: u64,
    path: PathBuf,
    duration_millis: u64,
    shared: &Arc<SharedSnapshot>,
    output: Option<&rodio::MixerDeviceSink>,
    player: &mut Option<Player>,
    loaded: &mut Option<LoadedTrack>,
    live_session: &mut Option<Arc<LiveCaptureSession>>,
) -> bool {
    if !is_current(shared, generation) {
        return false;
    }

    retire_live_session(shared, live_session);
    *player = None;
    *loaded = None;
    shared.generation.store(generation, Ordering::Release);
    shared.position_millis.store(0, Ordering::Release);
    shared.playing.store(false, Ordering::Release);
    shared.ready.store(false, Ordering::Release);

    let Some(output) = output else {
        shared.set_error(
            generation,
            String::from("Could not open the default audio output."),
        );
        return true;
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) => {
            shared.set_error(
                generation,
                format!("Could not open {} for playback: {error}", path.display()),
            );
            return true;
        }
    };
    let byte_len = file.metadata().ok().map(|metadata| metadata.len());
    let mut builder = Decoder::builder().with_data(file);
    if let Some(byte_len) = byte_len {
        builder = builder.with_byte_len(byte_len);
    }
    if let Some(hint) = path.extension().and_then(|extension| extension.to_str()) {
        builder = builder.with_hint(&hint.to_ascii_lowercase());
    }
    let decoder = match builder.build() {
        Ok(decoder) => decoder,
        Err(error) => {
            shared.set_error(
                generation,
                format!("Could not decode {} for playback: {error}", path.display()),
            );
            return true;
        }
    };
    if !is_current(shared, generation) {
        return false;
    }

    let sample_rate = decoder.sample_rate().get();
    let (producer, consumer) = RingBuffer::new(LIVE_CAPTURE_RING_CAPACITY);
    let session_id = shared.next_live_session_id.fetch_add(1, Ordering::Relaxed);
    let epoch = shared
        .live_epoch
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    let session = Arc::new(LiveCaptureSession::new(generation, session_id, epoch));
    shared.begin_live_session(&session);

    let analyzer_session = Arc::clone(&session);
    let analyzer_shared = Arc::clone(shared);
    let analyzer_spawn = thread::Builder::new()
        .name(String::from("cadence-live-spectrogram"))
        .spawn(move || run_live_analyzer(consumer, analyzer_session, analyzer_shared, sample_rate));
    let player_handle = Player::connect_new(output.mixer());
    match analyzer_spawn {
        Ok(_) => finish_loaded_track(
            shared,
            generation,
            path,
            duration_millis,
            player_handle,
            LiveAnalysisSource::new(decoder, producer, Arc::clone(&session), Arc::clone(shared)),
            Some(session),
            player,
            loaded,
            live_session,
        ),
        Err(error) => {
            finish_analyzer_fallback(
                shared,
                generation,
                path,
                duration_millis,
                player_handle,
                decoder,
                &session,
                error,
                player,
                loaded,
                live_session,
            );
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn finish_analyzer_fallback<S>(
    shared: &SharedSnapshot,
    generation: u64,
    path: PathBuf,
    duration_millis: u64,
    player_handle: Player,
    source: S,
    session: &Arc<LiveCaptureSession>,
    error: std::io::Error,
    player: &mut Option<Player>,
    loaded: &mut Option<LoadedTrack>,
    live_session: &mut Option<Arc<LiveCaptureSession>>,
) where
    S: Source<Item = f32> + Send + 'static,
{
    handle_live_analyzer_spawn_error(shared, generation, session, error);
    finish_loaded_track(
        shared,
        generation,
        path,
        duration_millis,
        player_handle,
        source,
        None,
        player,
        loaded,
        live_session,
    );
}

fn handle_live_analyzer_spawn_error(
    shared: &SharedSnapshot,
    generation: u64,
    session: &Arc<LiveCaptureSession>,
    error: std::io::Error,
) {
    shared.retire_live_session(Some(session));
    shared.set_analysis_warning(
        generation,
        format!("Could not start live spectrogram analysis: {error}"),
    );
}

fn retire_live_session(
    shared: &SharedSnapshot,
    live_session: &mut Option<Arc<LiveCaptureSession>>,
) {
    let session = live_session.take();
    shared.retire_live_session(session.as_ref());
}

fn apply_requested_volume(
    shared: &SharedSnapshot,
    player: Option<&Player>,
    applied_volume: &mut Option<f32>,
) {
    let Some(player) = player else {
        *applied_volume = None;
        return;
    };
    let requested = shared.requested_volume();
    let changed = applied_volume.is_none_or(|applied| (applied - requested).abs() > f32::EPSILON);
    if changed {
        player.set_volume(requested);
        *applied_volume = Some(requested);
    }
}

fn publish_snapshot(
    shared: &SharedSnapshot,
    player: Option<&Player>,
    loaded: Option<&LoadedTrack>,
) {
    let Some(loaded) = loaded else {
        return;
    };
    let Some(player) = player else {
        return;
    };
    let ended = player.empty();
    let position_millis = if ended {
        loaded.duration_millis
    } else {
        clamp_position(player.get_pos().as_millis() as u64, loaded.duration_millis)
    };
    shared
        .position_millis
        .store(position_millis, Ordering::Release);
    shared
        .playing
        .store(!ended && !player.is_paused(), Ordering::Release);
    shared.ready.store(true, Ordering::Release);
}

fn is_current(shared: &SharedSnapshot, generation: u64) -> bool {
    shared.requested_generation.load(Ordering::Acquire) == generation
}

pub fn clamp_position(position_millis: u64, duration_millis: u64) -> u64 {
    position_millis.min(duration_millis)
}

pub fn normalize_volume(volume: f32) -> f32 {
    if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        DEFAULT_VOLUME
    }
}

pub fn normalize_output_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, MAX_OUTPUT_GAIN)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioTransport, CONTROLS_BUSY_ERROR, CaptureFrame, Command, DEFAULT_VOLUME,
        LIVE_SPECTROGRAM_BAND_COUNT, LIVE_SPECTROGRAM_MAX_HISTORY,
        LIVE_SPECTRUM_DISPLAY_MAX_FREQUENCY, LIVE_SPECTRUM_DISPLAY_MIN_FREQUENCY,
        LIVE_SPECTRUM_DISPLAY_TILT_DB_PER_OCTAVE, LIVE_SPECTRUM_DISPLAY_TILT_REFERENCE_FREQUENCY,
        LIVE_SPECTRUM_FFT_SIZE, LIVE_SPECTRUM_HOP_SIZE, LiveAnalysisSource, LiveAnalyzer,
        LiveCaptureSession, MAX_OUTPUT_GAIN, PendingLoad, SharedSnapshot, clamp_position,
        display_tilt_db, finish_analyzer_fallback, handle_command, is_current, live_band_ranges,
        live_display_frequency_bounds, normalize_output_gain, normalize_volume,
        publish_live_frame_if_due, run_live_analyzer_iteration,
    };
    use rodio::{Player, Source, buffer::SamplesBuffer, source::SeekError};
    use std::path::PathBuf;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn seek_position_is_saturated_to_track_duration() {
        assert_eq!(clamp_position(2_000, 1_000), 1_000);
        assert_eq!(clamp_position(250, 1_000), 250);
        assert_eq!(clamp_position(250, 0), 0);
    }

    #[test]
    fn volume_is_finite_and_clamped_to_the_audition_range() {
        assert_eq!(normalize_volume(-0.25), 0.0);
        assert_eq!(normalize_volume(1.25), 1.0);
        assert_eq!(normalize_volume(f32::NAN), DEFAULT_VOLUME);
        assert_eq!(SharedSnapshot::new().requested_volume(), DEFAULT_VOLUME);
    }

    #[test]
    fn comparison_output_gain_allows_a_bounded_loudness_match_boost() {
        assert_eq!(normalize_output_gain(-1.0), 0.0);
        assert_eq!(
            normalize_output_gain(MAX_OUTPUT_GAIN + 1.0),
            MAX_OUTPUT_GAIN
        );
        assert_eq!(normalize_output_gain(f32::NAN), 1.0);
    }

    #[test]
    fn shared_output_gain_preserves_above_unity_reference_match() {
        let shared = SharedSnapshot::new();
        shared
            .requested_volume
            .store(2.5_f32.to_bits(), Ordering::Release);

        assert_eq!(shared.requested_volume(), 2.5);
    }

    #[test]
    fn command_capacity_preflight_rejects_a_full_queue() {
        let transport = AudioTransport::spawn();
        assert!(transport.has_command_capacity(2));
        transport.force_command_queue_full_for_test();
        assert!(!transport.has_command_capacity(1));
        assert!(!transport.has_command_capacity(2));
    }

    #[test]
    fn pending_load_blocks_dependent_controls_until_it_is_admitted() {
        let (commands, _receiver) = mpsc::sync_channel(super::COMMAND_CAPACITY);
        let transport = AudioTransport {
            commands,
            queued_commands: Arc::new(AtomicUsize::new(super::COMMAND_CAPACITY)),
            shared: Arc::new(SharedSnapshot::new()),
            pending_load: Arc::new(PendingLoad::new()),
            next_token: Arc::new(AtomicU64::new(1)),
        };

        transport
            .load(0, PathBuf::from("pending.wav"), 1_000)
            .expect("a full queue should coalesce the load");
        assert!(transport.has_pending_load());
        assert_eq!(
            transport.seek(0, 250, 1_000, true),
            Err(String::from(CONTROLS_BUSY_ERROR))
        );
        assert_eq!(transport.play(0), Err(String::from(CONTROLS_BUSY_ERROR)));
    }

    #[test]
    fn stale_generation_is_rejected() {
        let shared = SharedSnapshot::new();
        shared.requested_generation.store(8, Ordering::Release);
        assert!(is_current(&shared, 8));
        assert!(!is_current(&shared, 7));
    }

    #[test]
    fn default_snapshot_is_idle_and_not_ready() {
        let shared = SharedSnapshot::new();
        assert_eq!(
            shared.snapshot(),
            super::Snapshot {
                generation: 0,
                acknowledged_token: 0,
                position_millis: 0,
                playing: false,
                ready: false,
            }
        );
    }

    #[test]
    fn acknowledged_tokens_never_move_backwards() {
        let shared = SharedSnapshot::new();
        shared.acknowledge(9);
        shared.acknowledge(4);
        assert_eq!(shared.snapshot().acknowledged_token, 9);
    }

    #[test]
    fn pending_load_slot_keeps_the_latest_generation() {
        let pending = PendingLoad::new();
        pending.replace(Command::Load {
            token: 1,
            generation: 1,
            path: PathBuf::from("first.wav"),
            duration_millis: 1_000,
        });
        pending.replace(Command::Load {
            token: 2,
            generation: 2,
            path: PathBuf::from("second.wav"),
            duration_millis: 2_000,
        });
        assert_eq!(
            pending.take().and_then(|command| command.load_generation()),
            Some(2)
        );
    }

    fn active_test_session(generation: u64) -> (Arc<SharedSnapshot>, Arc<LiveCaptureSession>) {
        let shared = Arc::new(SharedSnapshot::new());
        shared
            .requested_generation
            .store(generation, Ordering::Release);
        let session = Arc::new(LiveCaptureSession::new(generation, 1, 1));
        shared.begin_live_session(&session);
        assert!(shared.set_live_analysis_frozen(&session, false));
        (shared, session)
    }

    #[test]
    fn analysis_source_downmixes_complete_frames_without_output_gain() {
        let (shared, session) = active_test_session(3);
        shared
            .requested_volume
            .store(0.25_f32.to_bits(), Ordering::Release);
        let (producer, mut consumer) = super::RingBuffer::new(8);
        let source = SamplesBuffer::new(
            std::num::NonZeroU16::new(2).expect("non-zero channel count"),
            std::num::NonZeroU32::new(48_000).expect("non-zero sample rate"),
            vec![1.0, -1.0, 0.25, 0.75],
        );
        let mut adapter = LiveAnalysisSource::new(source, producer, session, shared);
        let output = adapter.by_ref().collect::<Vec<_>>();
        assert_eq!(output, vec![1.0, -1.0, 0.25, 0.75]);
        assert_eq!(consumer.pop().expect("first mono frame").sample, 0.0);
        assert_eq!(consumer.pop().expect("second mono frame").sample, 0.5);
        assert!(consumer.pop().is_err());
    }

    #[test]
    fn analyzer_display_uses_exponential_attack_and_slower_release() {
        let mut attack = LiveAnalyzer::new(48_000);
        attack.smooth_row([0_u8; LIVE_SPECTROGRAM_BAND_COUNT]);
        let rising = attack.smooth_row([u8::MAX; LIVE_SPECTROGRAM_BAND_COUNT]);

        let mut release = LiveAnalyzer::new(48_000);
        release.smooth_row([u8::MAX; LIVE_SPECTROGRAM_BAND_COUNT]);
        let falling = release.smooth_row([0_u8; LIVE_SPECTROGRAM_BAND_COUNT]);

        assert!(rising[0] > 0 && rising[0] < u8::MAX);
        assert!(falling[0] > 0 && falling[0] < u8::MAX);
        let attack_delta = rising[0] as u16;
        let release_delta = u8::MAX as u16 - falling[0] as u16;
        assert!(
            attack_delta > release_delta,
            "attack_delta={attack_delta}, release_delta={release_delta}"
        );
    }

    #[test]
    fn analyzer_history_retains_raw_step_while_spectrum_payload_is_smoothed() {
        let mut analyzer = LiveAnalyzer::new(48_000);
        let quiet = [0_u8; LIVE_SPECTROGRAM_BAND_COUNT];
        let mut loud = [0_u8; LIVE_SPECTROGRAM_BAND_COUNT];
        loud[0] = u8::MAX;

        analyzer.record_analyzed_row(quiet);
        analyzer.record_analyzed_row(loud);

        let frame = analyzer.frame(1, 1).expect("two analyzer rows");
        assert_eq!(frame.value(0, 0), 0);
        assert_eq!(frame.value(1, 0), u8::MAX);
        assert_eq!(frame.spectrum_values.len(), LIVE_SPECTROGRAM_BAND_COUNT);
        assert_eq!(frame.spectrum_value(0), frame.spectrum_values[0]);
        assert!(frame.spectrum_value(0) > 0);
        assert!(frame.spectrum_value(0) < u8::MAX);

        analyzer.reset();
        assert_eq!(analyzer.row_count, 0);
        assert!(analyzer.spectrum_values.iter().all(|&value| value == 0));
        assert!(analyzer.frame(1, 1).is_none());
    }

    #[test]
    fn analyzer_defaults_and_frame_shapes_stay_bounded() {
        assert_eq!(LIVE_SPECTROGRAM_BAND_COUNT, 128);
        assert_eq!(LIVE_SPECTROGRAM_MAX_HISTORY, 192);
        assert_eq!(LIVE_SPECTRUM_FFT_SIZE, 2_048);
        assert_eq!(LIVE_SPECTRUM_HOP_SIZE, 512);
        assert_eq!(LIVE_SPECTRUM_HOP_SIZE, LIVE_SPECTRUM_FFT_SIZE / 4);

        let mut analyzer = LiveAnalyzer::new(48_000);
        for index in 0..=LIVE_SPECTROGRAM_MAX_HISTORY {
            let mut row = [0_u8; LIVE_SPECTROGRAM_BAND_COUNT];
            row[index % LIVE_SPECTROGRAM_BAND_COUNT] = index as u8;
            analyzer.record_analyzed_row(row);
        }

        assert_eq!(analyzer.row_count, LIVE_SPECTROGRAM_MAX_HISTORY);
        let frame = analyzer.frame(1, 1).expect("bounded analyzer frame");
        assert_eq!(
            frame.values.len(),
            LIVE_SPECTROGRAM_MAX_HISTORY * LIVE_SPECTROGRAM_BAND_COUNT
        );
        assert_eq!(frame.spectrum_values.len(), LIVE_SPECTROGRAM_BAND_COUNT);
        assert!(frame.is_valid());
    }

    #[test]
    fn analyzer_display_tilt_is_four_point_five_db_per_octave_around_one_khz() {
        let reference = LIVE_SPECTRUM_DISPLAY_TILT_REFERENCE_FREQUENCY;
        assert!(display_tilt_db(reference).abs() < f32::EPSILON);
        assert!(
            (display_tilt_db(reference * 2.0) - LIVE_SPECTRUM_DISPLAY_TILT_DB_PER_OCTAVE).abs()
                < 0.001
        );
        assert!(
            (display_tilt_db(reference / 2.0) + LIVE_SPECTRUM_DISPLAY_TILT_DB_PER_OCTAVE).abs()
                < 0.001
        );
    }

    #[test]
    fn analyzer_display_frequency_range_is_logarithmic_and_nyquist_clamped() {
        assert_eq!(
            live_display_frequency_bounds(48_000),
            (
                LIVE_SPECTRUM_DISPLAY_MIN_FREQUENCY,
                LIVE_SPECTRUM_DISPLAY_MAX_FREQUENCY
            )
        );
        assert_eq!(live_display_frequency_bounds(16_000).1, 8_000.0);

        let bands = live_band_ranges(48_000);
        assert!((bands[0].start_frequency - 20.0).abs() < 0.001);
        assert!((bands[LIVE_SPECTROGRAM_BAND_COUNT - 1].end_frequency - 20_000.0).abs() < 0.001);
        assert!(bands.windows(2).all(|pair| {
            pair[0].start_frequency < pair[0].end_frequency
                && pair[0].end_frequency <= pair[1].start_frequency
        }));

        let nyquist_clamped = live_band_ranges(16_000);
        assert!(
            nyquist_clamped
                .last()
                .is_some_and(|band| band.end_frequency <= 8_000.0)
        );
    }

    #[test]
    fn full_capture_ring_marks_a_discontinuity_before_accepting_again() {
        let (shared, session) = active_test_session(4);
        let (mut producer, mut consumer) = super::RingBuffer::new(1);
        let old_epoch = session.current_epoch();
        assert!(
            producer
                .push(CaptureFrame {
                    sample: 0.1,
                    epoch: old_epoch,
                })
                .is_ok()
        );
        session.mark_capture_drop(&shared);
        let dropped_epoch = session.current_epoch();
        assert!(dropped_epoch > old_epoch);
        assert!(
            producer
                .push(CaptureFrame {
                    sample: 0.2,
                    epoch: dropped_epoch,
                })
                .is_err()
        );
        assert_eq!(
            consumer.pop().expect("old frame remains bounded").epoch,
            old_epoch
        );
        assert!(
            producer
                .push(CaptureFrame {
                    sample: 0.2,
                    epoch: session.current_epoch(),
                })
                .is_ok()
        );
        assert_eq!(consumer.pop().expect("new frame").epoch, dropped_epoch);
    }

    #[test]
    fn analyzer_emits_at_one_row_per_512_frame_hop_and_caps_history() {
        let mut analyzer = LiveAnalyzer::new(48_000);
        let mut emitted = 0;
        for index in
            0..(LIVE_SPECTRUM_FFT_SIZE + LIVE_SPECTRUM_HOP_SIZE * LIVE_SPECTROGRAM_MAX_HISTORY)
        {
            if analyzer.push((index as f32 * 0.01).sin()) {
                emitted += 1;
            }
        }
        assert_eq!(emitted, LIVE_SPECTROGRAM_MAX_HISTORY + 1);
        assert_eq!(analyzer.fft_count, LIVE_SPECTROGRAM_MAX_HISTORY + 1);
        assert_eq!(analyzer.revision, (LIVE_SPECTROGRAM_MAX_HISTORY + 1) as u64);
        assert_eq!(analyzer.row_count, LIVE_SPECTROGRAM_MAX_HISTORY);
    }

    #[test]
    fn analyzer_does_not_publish_history_after_epoch_changes_before_publication() {
        let (shared, session) = active_test_session(7);
        let observed_epoch = session.current_epoch();
        let mut analyzer = LiveAnalyzer::new(48_000);
        for _ in 0..LIVE_SPECTRUM_FFT_SIZE {
            analyzer.push(0.25);
        }
        assert!(analyzer.revision > 0);

        session.mark_discontinuity(&shared);
        let mut published_revision = 0;
        let mut last_publication = Instant::now();
        publish_live_frame_if_due(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut last_publication,
            true,
        );

        assert!(shared.latest_live_frame().is_none());
        assert_eq!(published_revision, 0);
    }

    #[test]
    fn paused_analyzer_discards_partial_window_and_resumes_same_epoch() {
        let (shared, session) = active_test_session(8);
        let (mut producer, mut consumer) = super::RingBuffer::new(4_096);
        let mut analyzer = LiveAnalyzer::new(48_000);
        let mut observed_epoch = session.current_epoch();
        for index in 0..LIVE_SPECTRUM_FFT_SIZE {
            let emitted = analyzer.push((index as f32 * 0.01).sin());
            assert_eq!(emitted, index + 1 == LIVE_SPECTRUM_FFT_SIZE);
        }

        let mut published_revision = 0;
        let mut last_publication = Instant::now();
        publish_live_frame_if_due(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut last_publication,
            true,
        );
        let displayed_frame = shared
            .latest_live_frame()
            .expect("the pre-pause frame should be visible");
        let displayed_state = shared.live_frame_state();

        for index in 0..LIVE_SPECTRUM_FFT_SIZE {
            producer
                .push(CaptureFrame {
                    sample: (index as f32 * 0.02).cos(),
                    epoch: observed_epoch,
                })
                .expect("the test capture ring should have room");
        }
        shared.mark_capture_pending();
        let mut player = None;
        let mut loaded = None;
        let mut live_session = Some(Arc::clone(&session));
        handle_command(
            Command::Pause {
                token: 1,
                generation: 8,
            },
            &shared,
            None,
            &mut player,
            &mut loaded,
            &mut live_session,
        );
        assert_eq!(shared.snapshot().acknowledged_token, 1);
        assert!(session.analysis_frozen.load(Ordering::Acquire));

        let (consumed, frozen) = run_live_analyzer_iteration(
            &mut consumer,
            &session,
            &shared,
            &mut analyzer,
            &mut observed_epoch,
            &mut published_revision,
            &mut last_publication,
        );
        assert!(consumed);
        assert!(frozen);
        assert_eq!(analyzer.revision, displayed_state.revision);
        assert_eq!(analyzer.window_len, 0);
        assert_eq!(published_revision, displayed_state.revision);
        assert!(!shared.live_frame_state().pending);
        let paused_frame = shared
            .latest_live_frame()
            .expect("pause should preserve the displayed frame");
        assert!(Arc::ptr_eq(&paused_frame, &displayed_frame));
        assert_eq!(shared.live_frame_state().epoch, displayed_state.epoch);
        assert_eq!(shared.live_frame_state().revision, displayed_state.revision);

        assert!(shared.set_live_analysis_frozen(&session, false));
        last_publication = Instant::now()
            .checked_sub(super::LIVE_PUBLICATION_INTERVAL)
            .expect("the publication interval should be representable");
        for index in 0..super::LIVE_SPECTRUM_HOP_SIZE {
            producer
                .push(CaptureFrame {
                    sample: (index as f32 * 0.03).sin(),
                    epoch: observed_epoch,
                })
                .expect("the resumed capture ring should have room");
        }
        shared.mark_capture_pending();
        let (consumed, frozen) = run_live_analyzer_iteration(
            &mut consumer,
            &session,
            &shared,
            &mut analyzer,
            &mut observed_epoch,
            &mut published_revision,
            &mut last_publication,
        );
        assert!(consumed);
        assert!(!frozen);
        assert_eq!(analyzer.revision, displayed_state.revision);
        assert_eq!(published_revision, displayed_state.revision);
        let still_displayed_frame = shared
            .latest_live_frame()
            .expect("one resumed hop should keep the frozen frame");
        assert!(Arc::ptr_eq(&still_displayed_frame, &displayed_frame));

        for index in 0..(super::LIVE_SPECTRUM_FFT_SIZE - super::LIVE_SPECTRUM_HOP_SIZE) {
            producer
                .push(CaptureFrame {
                    sample: (index as f32 * 0.04).cos(),
                    epoch: observed_epoch,
                })
                .expect("the resumed capture ring should have room");
        }
        shared.mark_capture_pending();
        let (consumed, frozen) = run_live_analyzer_iteration(
            &mut consumer,
            &session,
            &shared,
            &mut analyzer,
            &mut observed_epoch,
            &mut published_revision,
            &mut last_publication,
        );
        assert!(consumed);
        assert!(!frozen);
        assert!(published_revision > displayed_state.revision);
        let resumed_frame = shared
            .latest_live_frame()
            .expect("resume should publish newly analyzed audio");
        assert_eq!(resumed_frame.epoch, displayed_frame.epoch);
        assert_eq!(resumed_frame.revision, published_revision);
        assert!(!Arc::ptr_eq(&resumed_frame, &displayed_frame));
        assert!(!shared.live_frame_state().pending);
        session.retire();
    }

    fn strongest_band_for_tone(frequency: f32) -> usize {
        let mut analyzer = LiveAnalyzer::new(48_000);
        for index in 0..LIVE_SPECTRUM_FFT_SIZE {
            assert!(
                !analyzer.push((std::f32::consts::TAU * frequency * index as f32 / 48_000.0).sin())
                    || index == LIVE_SPECTRUM_FFT_SIZE - 1
            );
        }
        let frame = analyzer.frame(1, 1).expect("one FFT row");
        (0..LIVE_SPECTROGRAM_BAND_COUNT)
            .max_by_key(|&band| frame.value(0, band))
            .expect("spectrogram has bands")
    }

    #[test]
    fn tones_localize_from_low_to_mid_to_high_log_bands() {
        let low = strongest_band_for_tone(80.0);
        let middle = strongest_band_for_tone(1_000.0);
        let high = strongest_band_for_tone(10_000.0);
        assert!(low < middle, "low={low}, middle={middle}");
        assert!(middle < high, "middle={middle}, high={high}");
    }

    #[derive(Clone)]
    struct SeekableSource {
        samples: Vec<f32>,
        position: usize,
        sought_to_millis: Arc<AtomicUsize>,
        reject_seek: bool,
    }

    impl Iterator for SeekableSource {
        type Item = f32;

        fn next(&mut self) -> Option<Self::Item> {
            let sample = self.samples.get(self.position).copied()?;
            self.position += 1;
            Some(sample)
        }
    }

    impl Source for SeekableSource {
        fn current_span_len(&self) -> Option<usize> {
            Some(self.samples.len().saturating_sub(self.position))
        }

        fn channels(&self) -> rodio::ChannelCount {
            std::num::NonZeroU16::new(1).expect("non-zero channel count")
        }

        fn sample_rate(&self) -> rodio::SampleRate {
            std::num::NonZeroU32::new(48_000).expect("non-zero sample rate")
        }

        fn total_duration(&self) -> Option<Duration> {
            Some(Duration::from_millis(100))
        }

        fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
            if self.reject_seek {
                return Err(SeekError::NotSupported {
                    underlying_source: "test-seekable-source",
                });
            }
            self.position = position.as_millis() as usize;
            self.sought_to_millis
                .store(position.as_millis() as usize, Ordering::Release);
            Ok(())
        }
    }

    #[test]
    fn analysis_source_delegates_seek_and_starts_a_new_epoch() {
        let (shared, session) = active_test_session(5);
        let sought_to_millis = Arc::new(AtomicUsize::new(0));
        let source = SeekableSource {
            samples: vec![0.0; 128],
            position: 0,
            sought_to_millis: Arc::clone(&sought_to_millis),
            reject_seek: false,
        };
        let (producer, _consumer) = super::RingBuffer::new(8);
        let mut adapter = LiveAnalysisSource::new(source, producer, Arc::clone(&session), shared);
        let old_epoch = session.current_epoch();
        adapter
            .try_seek(Duration::from_millis(37))
            .expect("test source accepts seek");
        assert_eq!(sought_to_millis.load(Ordering::Acquire), 37);
        assert!(session.current_epoch() > old_epoch);
        assert_eq!(adapter.current_span_len(), Some(91));
    }

    #[test]
    fn analysis_source_failed_seek_preserves_epoch_and_visible_frame() {
        let (shared, session) = active_test_session(5);
        let frame = Arc::new(
            super::LiveSpectrogramFrame::from_values(
                5,
                session.current_epoch(),
                1,
                48_000,
                1,
                Arc::from(vec![1_u8; LIVE_SPECTROGRAM_BAND_COUNT].into_boxed_slice()),
                Arc::from(vec![1_u8; LIVE_SPECTROGRAM_BAND_COUNT].into_boxed_slice()),
            )
            .expect("valid live spectrogram test frame"),
        );
        assert!(shared.publish_live_frame(&session, Arc::clone(&frame)));
        let (producer, _consumer) = super::RingBuffer::new(8);
        let source = SeekableSource {
            samples: vec![0.0; 128],
            position: 0,
            sought_to_millis: Arc::new(AtomicUsize::new(0)),
            reject_seek: true,
        };
        let mut adapter = LiveAnalysisSource::new(source, producer, session, Arc::clone(&shared));
        let old_epoch = adapter.session.current_epoch();

        assert!(adapter.try_seek(Duration::from_millis(37)).is_err());
        assert_eq!(adapter.session.current_epoch(), old_epoch);
        assert_eq!(shared.live_frame_state().epoch, old_epoch);
        assert!(
            shared
                .latest_live_frame()
                .is_some_and(|latest| Arc::ptr_eq(&latest, &frame))
        );
    }

    #[test]
    fn live_frame_state_tracks_pending_work_and_clears_on_reset() {
        let (shared, session) = active_test_session(6);
        shared.mark_capture_pending();
        let pending = shared.live_frame_state();
        assert!(pending.pending);
        assert_eq!(pending.generation, 6);
        shared.reset_live_segment();
        let reset = shared.live_frame_state();
        assert!(!reset.pending);
        assert!(reset.epoch > pending.epoch);
        session.retire();
    }

    #[test]
    fn analyzer_spawn_failure_keeps_raw_player_ready_and_publishes_warning() {
        let (shared, session) = active_test_session(9);
        shared.generation.store(9, Ordering::Release);
        let frame = Arc::new(
            super::LiveSpectrogramFrame::from_values(
                9,
                1,
                1,
                48_000,
                1,
                Arc::from(vec![1_u8; LIVE_SPECTROGRAM_BAND_COUNT].into_boxed_slice()),
                Arc::from(vec![1_u8; LIVE_SPECTROGRAM_BAND_COUNT].into_boxed_slice()),
            )
            .expect("valid live spectrogram test frame"),
        );
        assert!(shared.publish_live_frame(&session, frame));

        let (player_handle, _queue) = Player::new();
        let mut player = None;
        let mut loaded = None;
        let mut live_session = Some(Arc::clone(&session));
        finish_analyzer_fallback(
            &shared,
            9,
            PathBuf::from("fallback.wav"),
            1_000,
            player_handle,
            SamplesBuffer::new(
                std::num::NonZeroU16::new(1).expect("non-zero channel count"),
                std::num::NonZeroU32::new(48_000).expect("non-zero sample rate"),
                vec![0.0; 4],
            ),
            &session,
            std::io::Error::other("thread limit reached"),
            &mut player,
            &mut loaded,
            &mut live_session,
        );
        assert!(!session.active.load(Ordering::Acquire));
        assert_eq!(shared.live_session_id.load(Ordering::Acquire), 0);
        assert!(shared.latest_live_frame().is_none());
        assert!(player.as_ref().is_some_and(|player| !player.empty()));
        assert_eq!(loaded.as_ref().map(|track| track.generation), Some(9));
        assert!(shared.snapshot().ready);
        assert!(live_session.is_none());
        assert_eq!(
            shared.take_analysis_warning(9),
            Some(String::from(
                "Could not start live spectrogram analysis: thread limit reached"
            ))
        );
        assert!(shared.take_error(9).is_none());
    }

    #[test]
    fn retired_generation_cannot_publish_into_a_new_live_session() {
        let shared = Arc::new(SharedSnapshot::new());
        shared.requested_generation.store(7, Ordering::Release);
        let old_session = Arc::new(LiveCaptureSession::new(7, 1, 1));
        shared.begin_live_session(&old_session);
        assert!(shared.set_live_analysis_frozen(&old_session, false));
        let old_frame = Arc::new(
            super::LiveSpectrogramFrame::from_values(
                7,
                1,
                1,
                48_000,
                1,
                Arc::from(vec![1_u8; LIVE_SPECTROGRAM_BAND_COUNT].into_boxed_slice()),
                Arc::from(vec![1_u8; LIVE_SPECTROGRAM_BAND_COUNT].into_boxed_slice()),
            )
            .expect("valid live spectrogram test frame"),
        );
        assert!(shared.publish_live_frame(&old_session, old_frame.clone()));

        old_session.retire();
        shared.requested_generation.store(8, Ordering::Release);
        let new_session = Arc::new(LiveCaptureSession::new(8, 2, 2));
        shared.begin_live_session(&new_session);
        assert!(!shared.publish_live_frame(&old_session, old_frame));
        assert!(shared.latest_live_frame().is_none());
    }
}

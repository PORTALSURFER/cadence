//! Host-controlled audition playback for the native Cadence review surface.
//!
//! The Radiant reducer only sends small, generation-tagged commands and reads a
//! non-blocking snapshot. Output setup, decoder construction, and transport
//! control are owned by this host module. Rodio/CPAL may still pull decoder
//! data and service internal control state from the output callback, so this is
//! intentionally not a lock-free realtime or sample-accurate audio engine.

use crate::source::{self, VerifiedSourceTicket};
use rodio::{Decoder, DeviceSinkBuilder, Player, Source, source::SeekError};
use rtrb::{Consumer, Producer, RingBuffer};
#[cfg(test)]
use std::sync::Condvar;
use std::{
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
pub const LIVE_SPECTRUM_POINT_COUNT: usize = 768;
pub const LIVE_SPECTROGRAM_MAX_HISTORY: usize = 240;

const LIVE_CAPTURE_RING_CAPACITY: usize = 16_384;
const LIVE_SPECTRUM_FFT_SIZE: usize = 2_048;
const LIVE_SPECTRUM_POSITIVE_BIN_COUNT: usize = LIVE_SPECTRUM_FFT_SIZE / 2 + 1;
const LIVE_SPECTRUM_HOP_SIZE: usize = 512;
pub(crate) const LIVE_SPECTRUM_DISPLAY_MIN_FREQUENCY: f32 = 20.0;
pub(crate) const LIVE_SPECTRUM_DISPLAY_MAX_FREQUENCY: f32 = 20_000.0;
pub(crate) const LIVE_SPECTRUM_DISPLAY_FLOOR_DB: f32 = -90.0;
pub(crate) const LIVE_SPECTRUM_DISPLAY_CEILING_DB: f32 = 0.0;
pub(crate) const LIVE_SPECTRUM_DISPLAY_TILT_DB_PER_OCTAVE: f32 = 4.5;
pub(crate) const LIVE_SPECTRUM_DISPLAY_TILT_REFERENCE_FREQUENCY: f32 = 1_000.0;
const LIVE_SPECTRUM_ATTACK_TIME: Duration = Duration::from_millis(30);
const LIVE_SPECTRUM_RELEASE_TIME: Duration = Duration::from_millis(160);
// The analyzer still consumes every captured frame and emits FFT rows at the
// configured hop. Private live-graph publication is capped at 30 Hz,
// independently of the app's 60 Hz frame clock, while the latest-only gate
// below prevents duplicate or queued frames from competing with the rest of
// the review surface.
const LIVE_PUBLICATION_FPS: u64 = 30;
const LIVE_PUBLICATION_INTERVAL: Duration =
    Duration::from_nanos(1_000_000_000 / LIVE_PUBLICATION_FPS);
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
        oldest_rows: &[[u8; LIVE_SPECTROGRAM_BAND_COUNT]],
        wrapped_rows: &[[u8; LIVE_SPECTROGRAM_BAND_COUNT]],
        spectrum_values: &[u8; LIVE_SPECTRUM_POINT_COUNT],
    ) -> Option<Self> {
        let row_count = oldest_rows.len() + wrapped_rows.len();
        if sample_rate == 0 || row_count == 0 || row_count > LIVE_SPECTROGRAM_MAX_HISTORY {
            return None;
        }
        let mut values = Vec::with_capacity(row_count * LIVE_SPECTROGRAM_BAND_COUNT);
        for row in oldest_rows.iter().chain(wrapped_rows.iter()) {
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
            || spectrum_values.len() != LIVE_SPECTRUM_POINT_COUNT
        {
            return None;
        }
        Some(Self {
            generation,
            epoch,
            revision,
            sample_rate,
            row_count,
            // The validated history length is always a multiple of four
            // because every row has 128 bands. The same immutable byte Arc
            // can therefore be used directly by the storage buffer without
            // a second full-history allocation and copy.
            packed_values: Arc::clone(&values),
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
            && self.spectrum_values.len() == LIVE_SPECTRUM_POINT_COUNT
            && self.packed_values.len() == self.values.len().div_ceil(4) * 4
    }
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
    analyzer_thread: Mutex<Option<thread::Thread>>,
    #[cfg(test)]
    analyzer_iterations: AtomicUsize,
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
            analyzer_thread: Mutex::new(None),
            #[cfg(test)]
            analyzer_iterations: AtomicUsize::new(0),
        }
    }

    fn register_analyzer_thread(&self) {
        let current = thread::current();
        if let Ok(mut analyzer_thread) = self.analyzer_thread.lock() {
            *analyzer_thread = Some(current.clone());
        }
        // A lifecycle transition can race with analyzer startup. Re-checking
        // the published state after registration preserves the wake token
        // instead of allowing the new thread to sleep past a transition.
        if !self.active.load(Ordering::Acquire) || !self.analysis_frozen.load(Ordering::Acquire) {
            current.unpark();
        }
    }

    fn wake_analyzer(&self) {
        let analyzer_thread = self
            .analyzer_thread
            .lock()
            .ok()
            .and_then(|thread| thread.as_ref().cloned());
        if let Some(analyzer_thread) = analyzer_thread {
            analyzer_thread.unpark();
        }
    }

    #[cfg(test)]
    fn analyzer_iteration_started(&self) {
        self.analyzer_iterations.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn analyzer_iteration_count(&self) -> usize {
        self.analyzer_iterations.load(Ordering::Acquire)
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
        self.wake_analyzer();
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
    display_tilt_db: f32,
}

impl LiveBandRange {
    fn center_frequency(self) -> f32 {
        (self.start_frequency * self.end_frequency).sqrt()
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct LiveSpectrumPointMapping {
    max_bin_start: usize,
    max_bin_end: usize,
    interpolation_lower_bin: usize,
    interpolation_upper_bin: usize,
    interpolation_fraction: f32,
    display_tilt_db: f32,
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
        session.wake_analyzer();
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
                session.wake_analyzer();
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
        // This lock is held only while swapping one Arc and its revision. The
        // audio callback never takes it; blocking the analyzer briefly gives
        // the UI a coherent payload/revision pair instead of dropping a
        // publication during a repaint race.
        let Ok(mut latest) = self.live_frame.lock() else {
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
        let revision = frame.revision;
        *latest = Some(frame);
        self.live_revision.store(revision, Ordering::Release);
        true
    }

    #[cfg(test)]
    fn latest_live_frame(&self) -> Option<Arc<LiveSpectrogramFrame>> {
        self.live_frame.lock().ok()?.as_ref().cloned()
    }

    fn live_frame_snapshot(&self) -> (LiveFrameState, Option<Arc<LiveSpectrogramFrame>>) {
        let Ok(latest) = self.live_frame.lock() else {
            return (self.live_frame_state(), None);
        };
        let frame = latest.as_ref().cloned();
        let state = self.live_frame_state();
        (state, frame)
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
        if let Ok(mut frame) = self.live_frame.lock() {
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
    spectrum_point_mappings: [LiveSpectrumPointMapping; LIVE_SPECTRUM_POINT_COUNT],
    window_coefficients: [f32; LIVE_SPECTRUM_FFT_SIZE],
    one_sided_bin_calibration: [f32; LIVE_SPECTRUM_POSITIVE_BIN_COUNT],
    spectrum_attack_coefficient: f32,
    spectrum_release_coefficient: f32,
    spectrum_levels: [f32; LIVE_SPECTRUM_POINT_COUNT],
    has_spectrum_levels: bool,
    window: [f32; LIVE_SPECTRUM_FFT_SIZE],
    window_len: usize,
    fft: [LiveComplexSample; LIVE_SPECTRUM_FFT_SIZE],
    positive_magnitudes: [f32; LIVE_SPECTRUM_POSITIVE_BIN_COUNT],
    rows: [[u8; LIVE_SPECTROGRAM_BAND_COUNT]; LIVE_SPECTROGRAM_MAX_HISTORY],
    spectrum_values: [u8; LIVE_SPECTRUM_POINT_COUNT],
    history_start: usize,
    row_count: usize,
    revision: u64,
    fft_count: usize,
}

impl LiveAnalyzer {
    fn new(sample_rate: u32) -> Self {
        let sample_rate = sample_rate.max(1);
        let window_coefficients = live_periodic_hann_window();
        let window_sum = window_coefficients.iter().copied().sum::<f32>();
        assert!(
            window_sum.is_finite() && window_sum > 0.0,
            "periodic Hann window must have a finite positive sum"
        );
        let one_sided_bin_calibration = std::array::from_fn(|bin| {
            let one_sided_factor = if bin == 0 || bin == LIVE_SPECTRUM_POSITIVE_BIN_COUNT - 1 {
                1.0
            } else {
                2.0
            };
            one_sided_factor / window_sum
        });
        Self {
            sample_rate,
            band_ranges: live_band_ranges(sample_rate),
            spectrum_point_mappings: live_spectrum_point_mappings(sample_rate),
            window_coefficients,
            one_sided_bin_calibration,
            spectrum_attack_coefficient: live_ballistic_coefficient(
                LIVE_SPECTRUM_ATTACK_TIME,
                sample_rate,
            ),
            spectrum_release_coefficient: live_ballistic_coefficient(
                LIVE_SPECTRUM_RELEASE_TIME,
                sample_rate,
            ),
            spectrum_levels: [0.0; LIVE_SPECTRUM_POINT_COUNT],
            has_spectrum_levels: false,
            window: [0.0; LIVE_SPECTRUM_FFT_SIZE],
            window_len: 0,
            fft: [LiveComplexSample::default(); LIVE_SPECTRUM_FFT_SIZE],
            positive_magnitudes: [0.0; LIVE_SPECTRUM_POSITIVE_BIN_COUNT],
            rows: [[0; LIVE_SPECTROGRAM_BAND_COUNT]; LIVE_SPECTROGRAM_MAX_HISTORY],
            spectrum_values: [0; LIVE_SPECTRUM_POINT_COUNT],
            history_start: 0,
            row_count: 0,
            revision: 0,
            fft_count: 0,
        }
    }

    fn reset(&mut self) {
        self.spectrum_levels = [0.0; LIVE_SPECTRUM_POINT_COUNT];
        self.has_spectrum_levels = false;
        self.window_len = 0;
        self.spectrum_values = [0; LIVE_SPECTRUM_POINT_COUNT];
        self.history_start = 0;
        self.row_count = 0;
        self.revision = 0;
        self.fft_count = 0;
    }

    fn reset_after_pause(&mut self) {
        self.spectrum_levels = [0.0; LIVE_SPECTRUM_POINT_COUNT];
        self.has_spectrum_levels = false;
        self.window_len = 0;
        self.spectrum_values = [0; LIVE_SPECTRUM_POINT_COUNT];
        self.history_start = 0;
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
            *fft = LiveComplexSample {
                real: sample * self.window_coefficients[index],
                imaginary: 0.0,
            };
        }
        live_fft_in_place(&mut self.fft);

        for (bin, magnitude) in self.positive_magnitudes.iter_mut().enumerate() {
            let sample = self.fft[bin];
            *magnitude = (sample.real * sample.real + sample.imaginary * sample.imaginary).sqrt()
                * self.one_sided_bin_calibration[bin];
        }

        let mut target_row = [0_u8; LIVE_SPECTROGRAM_BAND_COUNT];
        for (band, range) in self.band_ranges.iter().enumerate() {
            let magnitude = self.positive_magnitudes[range.start..range.end]
                .iter()
                .copied()
                .fold(0.0_f32, f32::max);
            let decibels = 20.0 * magnitude.max(1.0e-8).log10();
            let display_decibels = (decibels + range.display_tilt_db).clamp(
                LIVE_SPECTRUM_DISPLAY_FLOOR_DB,
                LIVE_SPECTRUM_DISPLAY_CEILING_DB,
            );
            let normalized = ((display_decibels - LIVE_SPECTRUM_DISPLAY_FLOOR_DB)
                / (LIVE_SPECTRUM_DISPLAY_CEILING_DB - LIVE_SPECTRUM_DISPLAY_FLOOR_DB))
                .clamp(0.0, 1.0);
            target_row[band] = (normalized * u8::MAX as f32).round() as u8;
        }
        let target_spectrum = self.spectrum_target_levels();
        self.record_analyzed_row(target_row, target_spectrum);

        self.revision = self.revision.wrapping_add(1);
        self.fft_count = self.fft_count.saturating_add(1);
    }

    fn record_analyzed_row(
        &mut self,
        raw_row: [u8; LIVE_SPECTROGRAM_BAND_COUNT],
        target_spectrum: [f32; LIVE_SPECTRUM_POINT_COUNT],
    ) {
        self.spectrum_values = self.smooth_spectrum(target_spectrum);
        let physical_row = (self.history_start + self.row_count) % LIVE_SPECTROGRAM_MAX_HISTORY;
        self.rows[physical_row] = raw_row;
        if self.row_count < LIVE_SPECTROGRAM_MAX_HISTORY {
            self.row_count += 1;
        } else {
            self.history_start = (self.history_start + 1) % LIVE_SPECTROGRAM_MAX_HISTORY;
        }
    }

    fn spectrum_target_levels(&self) -> [f32; LIVE_SPECTRUM_POINT_COUNT] {
        std::array::from_fn(|point| {
            let mapping = self.spectrum_point_mappings[point];
            let magnitude = if mapping.max_bin_start < mapping.max_bin_end {
                self.positive_magnitudes[mapping.max_bin_start..mapping.max_bin_end]
                    .iter()
                    .copied()
                    .fold(0.0_f32, f32::max)
            } else {
                let lower = self.positive_magnitudes[mapping.interpolation_lower_bin];
                let upper = self.positive_magnitudes[mapping.interpolation_upper_bin];
                lower + (upper - lower) * mapping.interpolation_fraction
            };
            let decibels = 20.0 * magnitude.max(1.0e-8).log10();
            let display_decibels = (decibels + mapping.display_tilt_db).clamp(
                LIVE_SPECTRUM_DISPLAY_FLOOR_DB,
                LIVE_SPECTRUM_DISPLAY_CEILING_DB,
            );
            ((display_decibels - LIVE_SPECTRUM_DISPLAY_FLOOR_DB)
                / (LIVE_SPECTRUM_DISPLAY_CEILING_DB - LIVE_SPECTRUM_DISPLAY_FLOOR_DB))
                .clamp(0.0, 1.0)
        })
    }

    /// Apply display-only exponential attack/release ballistics. This keeps
    /// the line readable without changing the decoder samples or audio path.
    fn smooth_spectrum(
        &mut self,
        target_spectrum: [f32; LIVE_SPECTRUM_POINT_COUNT],
    ) -> [u8; LIVE_SPECTRUM_POINT_COUNT] {
        let mut values = [0_u8; LIVE_SPECTRUM_POINT_COUNT];
        for (point, &target) in target_spectrum.iter().enumerate() {
            let target = target.clamp(0.0, 1.0);
            let previous = self.spectrum_levels[point];
            let level = if self.has_spectrum_levels {
                let coefficient = if target > previous {
                    self.spectrum_attack_coefficient
                } else {
                    self.spectrum_release_coefficient
                };
                previous + coefficient * (target - previous)
            } else {
                target
            };
            let level = level.clamp(0.0, 1.0);
            self.spectrum_levels[point] = level;
            values[point] = (level * u8::MAX as f32).round() as u8;
        }
        self.has_spectrum_levels = true;
        values
    }

    fn frame(&self, generation: u64, epoch: u64) -> Option<Arc<LiveSpectrogramFrame>> {
        debug_assert!(self.history_start < LIVE_SPECTROGRAM_MAX_HISTORY);
        let first_span_len = self
            .row_count
            .min(LIVE_SPECTROGRAM_MAX_HISTORY - self.history_start);
        let second_span_len = self.row_count - first_span_len;
        let oldest_rows = &self.rows[self.history_start..self.history_start + first_span_len];
        let wrapped_rows = &self.rows[..second_span_len];
        LiveSpectrogramFrame::new(
            generation,
            epoch,
            self.revision,
            self.sample_rate,
            oldest_rows,
            wrapped_rows,
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

fn live_periodic_hann_window() -> [f32; LIVE_SPECTRUM_FFT_SIZE] {
    std::array::from_fn(|index| {
        0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / LIVE_SPECTRUM_FFT_SIZE as f32).cos()
    })
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

pub(crate) fn live_spectrum_point_frequency(sample_rate: u32, point: usize) -> f32 {
    let (minimum, maximum) = live_display_frequency_bounds(sample_rate);
    let last_point = LIVE_SPECTRUM_POINT_COUNT - 1;
    if point == 0 {
        return minimum;
    }
    if point >= last_point {
        return maximum;
    }
    let ratio = (maximum / minimum.max(f32::MIN_POSITIVE)).max(1.0);
    if ratio == 1.0 {
        minimum
    } else {
        minimum * ratio.powf(point as f32 / last_point as f32)
    }
}

fn live_spectrum_point_mappings(
    sample_rate: u32,
) -> [LiveSpectrumPointMapping; LIVE_SPECTRUM_POINT_COUNT] {
    let sample_rate_hz = sample_rate.max(1);
    let sample_rate = sample_rate_hz as f32;
    let maximum_bin = LIVE_SPECTRUM_POSITIVE_BIN_COUNT - 1;
    let last_point = LIVE_SPECTRUM_POINT_COUNT - 1;

    std::array::from_fn(|point| {
        let display_frequency = live_spectrum_point_frequency(sample_rate_hz, point);
        let previous_frequency = if point == 0 {
            display_frequency
        } else {
            live_spectrum_point_frequency(sample_rate_hz, point - 1)
        };
        let next_frequency = if point == last_point {
            display_frequency
        } else {
            live_spectrum_point_frequency(sample_rate_hz, point + 1)
        };
        let cell_start = if point == 0 {
            display_frequency
        } else {
            (previous_frequency * display_frequency).sqrt()
        };
        let cell_end = if point == last_point {
            display_frequency
        } else {
            (display_frequency * next_frequency).sqrt()
        };

        let mut max_bin_start = LIVE_SPECTRUM_POSITIVE_BIN_COUNT;
        let mut max_bin_end = 0;
        for bin in 0..LIVE_SPECTRUM_POSITIVE_BIN_COUNT {
            let bin_frequency = bin as f32 * sample_rate / LIVE_SPECTRUM_FFT_SIZE as f32;
            let inside_cell = bin_frequency >= cell_start
                && (bin_frequency < cell_end || (point == last_point && bin_frequency <= cell_end));
            if inside_cell {
                max_bin_start = max_bin_start.min(bin);
                max_bin_end = max_bin_end.max(bin + 1);
            }
        }
        if max_bin_start == LIVE_SPECTRUM_POSITIVE_BIN_COUNT {
            max_bin_start = 0;
        }

        let bin_position = (display_frequency / sample_rate * LIVE_SPECTRUM_FFT_SIZE as f32)
            .clamp(0.0, maximum_bin as f32);
        let interpolation_lower_bin = bin_position.floor() as usize;
        let interpolation_upper_bin = bin_position.ceil() as usize;
        let interpolation_fraction =
            (bin_position - interpolation_lower_bin as f32).clamp(0.0, 1.0);

        LiveSpectrumPointMapping {
            max_bin_start,
            max_bin_end,
            interpolation_lower_bin,
            interpolation_upper_bin,
            interpolation_fraction,
            display_tilt_db: display_tilt_db(display_frequency),
        }
    })
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
        let mut band = LiveBandRange {
            start,
            end,
            start_frequency,
            end_frequency,
            display_tilt_db: 0.0,
        };
        band.display_tilt_db = display_tilt_db(band.center_frequency());
        band
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
    session.register_analyzer_thread();
    let mut analyzer = LiveAnalyzer::new(sample_rate);
    let mut observed_epoch = session.current_epoch();
    let mut published_revision = 0_u64;
    let mut next_publication_deadline = Instant::now();

    loop {
        #[cfg(test)]
        session.analyzer_iteration_started();

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
            &mut next_publication_deadline,
        );

        if !session.active.load(Ordering::Acquire) {
            break;
        }

        if frozen {
            let producer_gone = consumer.is_abandoned() && consumer.is_empty();
            let retired = !session.active.load(Ordering::Acquire);
            if producer_gone || retired {
                break;
            }
            if session.analysis_frozen.load(Ordering::Acquire) {
                // The callback never wakes this thread. A transition stores
                // its atomic state first and then unparks us, so this park is
                // indefinite while frozen without creating a periodic wake.
                thread::park();
            }
            continue;
        }

        let producer_gone = consumer.is_abandoned() && consumer.is_empty();
        let retired = !session.active.load(Ordering::Acquire);
        if producer_gone && consumer.is_empty() {
            if !retired && analyzer.revision > published_revision {
                if let Some(remaining) =
                    next_publication_deadline.checked_duration_since(Instant::now())
                {
                    thread::park_timeout(remaining);
                }
                publish_live_frame_if_due(
                    &analyzer,
                    &session,
                    &shared,
                    observed_epoch,
                    &mut published_revision,
                    &mut next_publication_deadline,
                    true,
                );
            }
            break;
        }

        if !consumed {
            thread::park_timeout(LIVE_ANALYZER_POLL_INTERVAL);
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
    next_publication_deadline: &mut Instant,
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
                next_publication_deadline,
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

    publish_live_frame_if_due(
        analyzer,
        session,
        shared,
        *observed_epoch,
        published_revision,
        next_publication_deadline,
        false,
    );

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
    next_publication_deadline: &mut Instant,
    force: bool,
) {
    publish_live_frame_if_due_at(
        analyzer,
        session,
        shared,
        observed_epoch,
        published_revision,
        next_publication_deadline,
        Instant::now(),
        force,
    );
}

#[allow(clippy::too_many_arguments)]
fn publish_live_frame_if_due_at(
    analyzer: &LiveAnalyzer,
    session: &LiveCaptureSession,
    shared: &SharedSnapshot,
    observed_epoch: u64,
    published_revision: &mut u64,
    next_publication_deadline: &mut Instant,
    now: Instant,
    force: bool,
) {
    if !force {
        if now < *next_publication_deadline {
            return;
        }
        if session.current_epoch() != observed_epoch {
            return;
        }
    }
    if analyzer.revision <= *published_revision {
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
        if !force {
            advance_live_publication_deadline(next_publication_deadline, now);
        }
    }
}

/// Advance to the first cadence boundary strictly after `now`. Remainder
/// arithmetic skips every elapsed slot in one step without retiming the phase.
fn advance_live_publication_deadline(next_publication_deadline: &mut Instant, now: Instant) {
    let elapsed = now.duration_since(*next_publication_deadline);
    let interval_nanos = LIVE_PUBLICATION_INTERVAL.as_nanos();
    let remainder_nanos = elapsed.as_nanos() % interval_nanos;
    let advance_nanos = if remainder_nanos == 0 {
        interval_nanos
    } else {
        interval_nanos - remainder_nanos
    };
    let advance = Duration::from_nanos(
        u64::try_from(advance_nanos).expect("live publication interval fits in nanoseconds"),
    );
    if let Some(deadline) = now.checked_add(advance) {
        *next_publication_deadline = deadline;
    }
}

#[derive(Clone, Debug)]
enum Command {
    Load {
        token: u64,
        generation: u64,
        ticket: VerifiedSourceTicket,
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

#[cfg(test)]
#[derive(Debug, Default)]
struct TransportParkGateState {
    park_count: usize,
    permit: bool,
    exited: bool,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct TransportParkGate {
    state: Mutex<TransportParkGateState>,
    changed: Condvar,
}

#[cfg(test)]
impl TransportParkGate {
    fn wait_for_park(&self) {
        let mut state = self
            .state
            .lock()
            .expect("transport test park state should not be poisoned");
        state.park_count += 1;
        self.changed.notify_all();
        while !state.permit {
            state = self
                .changed
                .wait(state)
                .expect("transport test park state should not be poisoned");
        }
        state.permit = false;
        self.changed.notify_all();
    }

    fn notify(&self) {
        let mut state = self
            .state
            .lock()
            .expect("transport test park state should not be poisoned");
        let observed_park_count = state.park_count;
        state.permit = true;
        self.changed.notify_all();
        while !state.exited && state.park_count == observed_park_count {
            state = self
                .changed
                .wait(state)
                .expect("transport test park state should not be poisoned");
        }
    }

    fn wait_until_parked(&self) {
        let mut state = self
            .state
            .lock()
            .expect("transport test park state should not be poisoned");
        while state.park_count == 0 {
            state = self
                .changed
                .wait(state)
                .expect("transport test park state should not be poisoned");
        }
    }

    fn wait_until_exited(&self) {
        let mut state = self
            .state
            .lock()
            .expect("transport test park state should not be poisoned");
        while !state.exited {
            state = self
                .changed
                .wait(state)
                .expect("transport test park state should not be poisoned");
        }
    }

    fn mark_exited(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.exited = true;
            self.changed.notify_all();
        }
    }
}

#[derive(Debug, Default)]
struct TransportWake {
    thread: Mutex<Option<thread::Thread>>,
    sequence: AtomicU64,
    #[cfg(test)]
    park_gate: Option<Arc<TransportParkGate>>,
}

impl TransportWake {
    #[cfg(test)]
    fn with_park_gate(park_gate: Arc<TransportParkGate>) -> Self {
        Self {
            thread: Mutex::new(None),
            sequence: AtomicU64::new(0),
            park_gate: Some(park_gate),
        }
    }

    fn register(&self) {
        if let Ok(mut thread_handle) = self.thread.lock() {
            *thread_handle = Some(thread::current());
        }
    }

    fn notify(&self) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        if let Ok(thread) = self.thread.lock()
            && let Some(thread) = thread.as_ref()
        {
            thread.unpark();
        }
        #[cfg(test)]
        if let Some(park_gate) = self.park_gate.as_ref() {
            park_gate.notify();
        }
    }

    fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }

    fn park(&self) {
        #[cfg(test)]
        if let Some(park_gate) = self.park_gate.as_ref() {
            park_gate.wait_for_park();
            return;
        }
        thread::park();
    }

    #[cfg(test)]
    fn wait_until_parked(&self) {
        self.park_gate
            .as_ref()
            .expect("transport test should have a park gate")
            .wait_until_parked();
    }

    #[cfg(test)]
    fn wait_until_exited(&self) {
        self.park_gate
            .as_ref()
            .expect("transport test should have a park gate")
            .wait_until_exited();
    }

    #[cfg(test)]
    fn mark_exited(&self) {
        if let Some(park_gate) = self.park_gate.as_ref() {
            park_gate.mark_exited();
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

#[derive(Debug)]
struct TransportCommandEndpoint {
    commands: Option<SyncSender<Command>>,
    wake: Arc<TransportWake>,
}

impl TransportCommandEndpoint {
    fn new(commands: SyncSender<Command>, wake: Arc<TransportWake>) -> Self {
        Self {
            commands: Some(commands),
            wake,
        }
    }

    #[allow(clippy::result_large_err)]
    fn try_send(&self, command: Command) -> Result<(), TrySendError<Command>> {
        let Some(commands) = self.commands.as_ref() else {
            return Err(TrySendError::Disconnected(command));
        };
        commands.try_send(command)
    }

    fn notify(&self) {
        self.wake.notify();
    }
}

impl Drop for TransportCommandEndpoint {
    fn drop(&mut self) {
        drop(self.commands.take());
        self.wake.notify();
    }
}

#[derive(Clone, Debug)]
pub struct AudioTransport {
    endpoint: Arc<TransportCommandEndpoint>,
    queued_commands: Arc<AtomicUsize>,
    shared: Arc<SharedSnapshot>,
    pending_load: Arc<PendingLoad>,
    next_token: Arc<AtomicU64>,
    #[cfg(test)]
    test_next_command_error: Arc<Mutex<Option<String>>>,
}

impl AudioTransport {
    pub fn spawn() -> Self {
        Self::spawn_with_wake(Arc::new(TransportWake::default()))
    }

    #[cfg(test)]
    fn spawn_with_park_gate() -> Self {
        Self::spawn_with_wake(Arc::new(TransportWake::with_park_gate(Arc::new(
            TransportParkGate::default(),
        ))))
    }

    fn spawn_with_wake(wake: Arc<TransportWake>) -> Self {
        let (commands, receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let queued_commands = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(SharedSnapshot::new());
        let pending_load = Arc::new(PendingLoad::new());
        let endpoint = Arc::new(TransportCommandEndpoint::new(commands, Arc::clone(&wake)));
        let thread_queued_commands = Arc::clone(&queued_commands);
        let thread_shared = Arc::clone(&shared);
        let thread_pending_load = Arc::clone(&pending_load);
        let thread_wake = Arc::clone(&wake);
        thread::Builder::new()
            .name(String::from("cadence-audio-transport"))
            .spawn(move || {
                thread_wake.register();
                run_transport(
                    receiver,
                    thread_queued_commands,
                    thread_shared,
                    thread_pending_load,
                    thread_wake,
                )
            })
            .expect("Cadence audio transport thread should spawn");
        Self {
            endpoint,
            queued_commands,
            shared,
            pending_load,
            next_token: Arc::new(AtomicU64::new(1)),
            #[cfg(test)]
            test_next_command_error: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    fn test_wake(&self) -> Arc<TransportWake> {
        Arc::clone(&self.endpoint.wake)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot()
    }

    pub(crate) fn live_frame_snapshot(
        &self,
    ) -> (LiveFrameState, Option<Arc<LiveSpectrogramFrame>>) {
        self.shared.live_frame_snapshot()
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

    #[cfg(test)]
    pub(crate) fn set_command_queue_size_for_test(&self, queued: usize) {
        self.queued_commands
            .store(queued.min(COMMAND_CAPACITY), Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_command_for_test(&self, error: String) {
        if let Ok(mut next_error) = self.test_next_command_error.lock() {
            *next_error = Some(error);
        }
    }

    #[cfg(test)]
    pub(crate) fn requested_output_gain_for_test(&self) -> f32 {
        self.shared.requested_volume()
    }

    pub fn load(
        &self,
        generation: u64,
        ticket: VerifiedSourceTicket,
        duration_millis: u64,
    ) -> Result<u64, String> {
        if let Some(error) = self.take_test_command_error() {
            return Err(error);
        }
        self.shared
            .requested_generation
            .store(generation, Ordering::Release);
        let token = self.next_token();
        let command = Command::Load {
            token,
            generation,
            ticket,
            duration_millis,
        };
        if !self.try_reserve_command_slot() {
            self.store_pending_load(command)?;
            self.endpoint.notify();
            return Ok(token);
        }
        match self.endpoint.try_send(command) {
            Ok(()) => {
                self.clear_pending_load(generation);
                self.endpoint.notify();
                Ok(token)
            }
            // Wake the transport so it can pick up the latest load intent from
            // the coalescing slot without waiting for a control tick.
            Err(TrySendError::Full(command)) => {
                self.release_command_slot();
                self.store_pending_load(command)?;
                self.endpoint.notify();
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
        if let Some(error) = self.take_test_command_error() {
            return Err(error);
        }
        self.shared
            .requested_generation
            .store(generation, Ordering::Release);
        let token = self.next_token();
        self.try_send(Command::Unload { token, generation })
            .map(|()| token)
    }

    pub fn play(&self, generation: u64) -> Result<u64, String> {
        if let Some(error) = self.take_test_command_error() {
            return Err(error);
        }
        if self.has_pending_load() {
            return Err(String::from(CONTROLS_BUSY_ERROR));
        }
        let token = self.next_token();
        self.try_send(Command::Play { token, generation })
            .map(|()| token)
    }

    pub fn pause(&self, generation: u64) -> Result<u64, String> {
        if let Some(error) = self.take_test_command_error() {
            return Err(error);
        }
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
        if let Some(error) = self.take_test_command_error() {
            return Err(error);
        }
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
        match self.endpoint.try_send(command) {
            Ok(()) => {
                self.endpoint.notify();
                Ok(())
            }
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

    #[cfg(test)]
    fn take_test_command_error(&self) -> Option<String> {
        self.test_next_command_error.lock().ok()?.take()
    }

    #[cfg(not(test))]
    fn take_test_command_error(&self) -> Option<String> {
        None
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
    ticket: VerifiedSourceTicket,
    duration_millis: u64,
}

#[allow(clippy::too_many_arguments)]
fn finish_loaded_track<S>(
    shared: &SharedSnapshot,
    generation: u64,
    ticket: VerifiedSourceTicket,
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
        ticket,
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
    wake: Arc<TransportWake>,
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
        let pending_load_consumed = if let Some(command) = take_pending_load(&pending_load) {
            handle_command(
                command,
                &shared,
                output.as_ref(),
                &mut player,
                &mut loaded,
                &mut live_session,
                &mut applied_volume,
            );
            true
        } else {
            false
        };

        if !pending_load_consumed {
            match wait_for_transport_command(
                &receiver,
                &pending_load,
                &wake,
                transport_is_actively_playing(player.as_ref(), loaded.as_ref()),
            ) {
                TransportWaitResult::Command(command) => {
                    release_command_slot(&queued_commands);
                    handle_command(
                        command,
                        &shared,
                        output.as_ref(),
                        &mut player,
                        &mut loaded,
                        &mut live_session,
                        &mut applied_volume,
                    );
                }
                TransportWaitResult::TimedOut => {}
                TransportWaitResult::Woken => continue,
                TransportWaitResult::Disconnected => break,
            }
        }

        let mut disconnected = false;
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
                        &mut applied_volume,
                    )
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        if disconnected {
            break;
        }

        reconcile_stale_track(&shared, &mut player, &mut loaded, &mut live_session);
        apply_requested_volume(&shared, player.as_ref(), &mut applied_volume);
        publish_snapshot(&shared, player.as_ref(), loaded.as_ref());
    }

    retire_live_session(&shared, &mut live_session);
    drop(player);
    drop(loaded);
    drop(output);
    #[cfg(test)]
    wake.mark_exited();
}

#[derive(Debug)]
enum TransportWaitResult {
    Command(Command),
    TimedOut,
    Woken,
    Disconnected,
}

fn wait_for_transport_command(
    receiver: &Receiver<Command>,
    pending_load: &PendingLoad,
    wake: &TransportWake,
    active_playback: bool,
) -> TransportWaitResult {
    let observed_wake = wake.sequence();
    let deadline = active_playback.then(|| Instant::now() + CONTROL_INTERVAL);

    loop {
        match receiver.try_recv() {
            Ok(command) => return TransportWaitResult::Command(command),
            Err(TryRecvError::Disconnected) => return TransportWaitResult::Disconnected,
            Err(TryRecvError::Empty) => {}
        }
        if pending_load.is_pending() || wake.sequence() != observed_wake {
            return TransportWaitResult::Woken;
        }

        match deadline {
            None => {
                // A quiescent transport has no timed polling deadline. The
                // command channel and pending-load wake both unpark this
                // thread, while the repeated checks close the race around
                // entering the park.
                wake.park();
            }
            Some(deadline) => {
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    return TransportWaitResult::TimedOut;
                };
                thread::park_timeout(remaining);
                if pending_load.is_pending() || wake.sequence() != observed_wake {
                    return TransportWaitResult::Woken;
                }
                if Instant::now() >= deadline {
                    return TransportWaitResult::TimedOut;
                }
            }
        }
    }
}

fn transport_is_actively_playing(player: Option<&Player>, loaded: Option<&LoadedTrack>) -> bool {
    loaded.is_some() && player.is_some_and(|player| !player.is_paused() && !player.empty())
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
    applied_volume: &mut Option<f32>,
) {
    let (token, acknowledged) = match command {
        Command::Load {
            token,
            generation,
            ticket,
            duration_millis,
        } => (
            token,
            load_track(
                generation,
                ticket,
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
                        track.ticket,
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
                        apply_requested_volume(shared, Some(player), applied_volume);
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
                                track.ticket,
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
                                    apply_requested_volume(shared, Some(player), applied_volume);
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
    ticket: VerifiedSourceTicket,
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

    let file = match source::open_for_ticket(&ticket) {
        Ok(file) => file,
        Err(error) => {
            shared.set_error(generation, error.to_string());
            return true;
        }
    };
    let Some(output) = output else {
        shared.set_error(
            generation,
            String::from("Could not open the default audio output."),
        );
        return true;
    };
    let byte_len = Some(ticket.proof().byte_len);
    let mut builder = Decoder::builder().with_data(file);
    if let Some(byte_len) = byte_len {
        builder = builder.with_byte_len(byte_len);
    }
    if let Some(hint) = ticket
        .path()
        .extension()
        .and_then(|extension| extension.to_str())
    {
        builder = builder.with_hint(&hint.to_ascii_lowercase());
    }
    let decoder = match builder.build() {
        Ok(decoder) => decoder,
        Err(error) => {
            shared.set_error(
                generation,
                format!(
                    "Could not decode {} for playback: {error}",
                    ticket.path().display()
                ),
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
            ticket,
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
                ticket,
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
    ticket: VerifiedSourceTicket,
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
        ticket,
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
        LIVE_PUBLICATION_FPS, LIVE_PUBLICATION_INTERVAL, LIVE_SPECTROGRAM_BAND_COUNT,
        LIVE_SPECTROGRAM_MAX_HISTORY, LIVE_SPECTRUM_DISPLAY_CEILING_DB,
        LIVE_SPECTRUM_DISPLAY_FLOOR_DB, LIVE_SPECTRUM_DISPLAY_MAX_FREQUENCY,
        LIVE_SPECTRUM_DISPLAY_MIN_FREQUENCY, LIVE_SPECTRUM_DISPLAY_TILT_DB_PER_OCTAVE,
        LIVE_SPECTRUM_DISPLAY_TILT_REFERENCE_FREQUENCY, LIVE_SPECTRUM_FFT_SIZE,
        LIVE_SPECTRUM_HOP_SIZE, LIVE_SPECTRUM_POINT_COUNT, LiveAnalysisSource, LiveAnalyzer,
        LiveCaptureSession, LiveSpectrogramFrame, LoadedTrack, MAX_OUTPUT_GAIN, PendingLoad,
        SharedSnapshot, TransportWaitResult, TransportWake, clamp_position, display_tilt_db,
        finish_analyzer_fallback, handle_command, is_current, live_band_ranges,
        live_display_frequency_bounds, live_spectrum_point_frequency, live_spectrum_point_mappings,
        load_track, normalize_output_gain, normalize_volume, publish_live_frame_if_due,
        publish_live_frame_if_due_at, publish_snapshot, run_live_analyzer,
        run_live_analyzer_iteration, take_pending_load, transport_is_actively_playing,
        wait_for_transport_command,
    };
    use crate::source::{AudioSourceProof, SourceFileStamp, VerifiedSourceTicket};
    use rodio::{Player, Source, buffer::SamplesBuffer, source::SeekError};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};
    use std::{fs, path::PathBuf};

    fn test_ticket(path: &str) -> VerifiedSourceTicket {
        VerifiedSourceTicket::new(
            PathBuf::from(path),
            AudioSourceProof {
                sha256: "0".repeat(64),
                byte_len: 0,
            },
            SourceFileStamp {
                dev: 0,
                inode: 0,
                len: 0,
                mtime_nanos: 0,
                ctime_nanos: 0,
            },
        )
        .expect("test source ticket should be valid")
    }

    fn replaced_ticket_fixture(label: &str) -> (PathBuf, VerifiedSourceTicket) {
        let path = std::env::temp_dir().join(format!(
            "cadence-transport-command-reload-{label}-{}",
            std::process::id()
        ));
        fs::write(&path, b"original").expect("fixture should write");
        let verified = crate::source::open_and_hash(&path, || false)
            .expect("source should hash before playback");
        let ticket = verified.ticket();
        drop(verified);
        fs::write(&path, b"replaced").expect("replacement should write");
        (path, ticket)
    }

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
            endpoint: Arc::new(super::TransportCommandEndpoint::new(
                commands,
                Arc::new(super::TransportWake::default()),
            )),
            queued_commands: Arc::new(AtomicUsize::new(super::COMMAND_CAPACITY)),
            shared: Arc::new(SharedSnapshot::new()),
            pending_load: Arc::new(PendingLoad::new()),
            next_token: Arc::new(AtomicU64::new(1)),
            test_next_command_error: Arc::new(Mutex::new(None)),
        };

        transport
            .load(0, test_ticket("pending.wav"), 1_000)
            .expect("a full queue should coalesce the load");
        assert!(transport.has_pending_load());
        assert_eq!(
            transport.seek(0, 250, 1_000, true),
            Err(String::from(CONTROLS_BUSY_ERROR))
        );
        assert_eq!(transport.play(0), Err(String::from(CONTROLS_BUSY_ERROR)));
    }

    #[test]
    fn dropping_the_last_transport_handle_disconnects_a_parked_worker() {
        let transport = AudioTransport::spawn_with_park_gate();
        let wake = transport.test_wake();
        wake.wait_until_parked();

        drop(transport);

        wake.wait_until_exited();
    }

    #[test]
    fn quiescent_transport_parks_until_an_accepted_command_wakes_it() {
        let (commands, receiver) = mpsc::sync_channel(1);
        let pending_load = PendingLoad::new();
        let wake = Arc::new(TransportWake::default());
        let worker_wake = Arc::clone(&wake);
        let (ready_sender, ready_receiver) = mpsc::channel();
        let (result_sender, result_receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            worker_wake.register();
            ready_sender
                .send(())
                .expect("transport wait test should start");
            let result = wait_for_transport_command(&receiver, &pending_load, &worker_wake, false);
            result_sender
                .send(result)
                .expect("transport wait test should report");
        });

        ready_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("transport wait test should register its worker");
        assert!(
            result_receiver
                .recv_timeout(Duration::from_millis(20))
                .is_err(),
            "an unloaded or paused transport must not wake on the control interval"
        );

        commands
            .send(Command::Unload {
                token: 1,
                generation: 0,
            })
            .expect("the test command should be accepted");
        wake.notify();
        assert!(matches!(
            result_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("the accepted command should wake the transport"),
            TransportWaitResult::Command(Command::Unload {
                token: 1,
                generation: 0
            })
        ));
        worker.join().expect("transport wait worker should exit");
    }

    #[test]
    fn pending_load_wakes_quiescent_transport_and_is_admitted_before_drain() {
        let (commands, receiver) = mpsc::sync_channel(1);
        let pending_load = PendingLoad::new();
        let wake = TransportWake::default();
        pending_load.replace(Command::Load {
            token: 7,
            generation: 3,
            ticket: test_ticket("pending-admission.wav"),
            duration_millis: 1_000,
        });
        wake.notify();

        assert!(matches!(
            wait_for_transport_command(&receiver, &pending_load, &wake, false),
            TransportWaitResult::Woken
        ));
        assert_eq!(
            take_pending_load(&pending_load).and_then(|command| command.load_generation()),
            Some(3)
        );

        commands
            .send(Command::Unload {
                token: 8,
                generation: 3,
            })
            .expect("the queued command should remain available for the immediate drain");
        assert!(matches!(
            wait_for_transport_command(&receiver, &pending_load, &wake, false),
            TransportWaitResult::Command(Command::Unload {
                token: 8,
                generation: 3
            })
        ));
    }

    #[test]
    fn active_transport_uses_timeout_polling_and_publishes_playback_end() {
        let (player, _queue) = Player::new();
        player.append(SamplesBuffer::new(
            std::num::NonZeroU16::new(1).expect("one channel is non-zero"),
            std::num::NonZeroU32::new(48_000).expect("sample rate is non-zero"),
            vec![0.0; 48_000],
        ));
        player.play();
        let loaded = LoadedTrack {
            generation: 3,
            ticket: test_ticket("active.wav"),
            duration_millis: 1_000,
        };
        assert!(transport_is_actively_playing(Some(&player), Some(&loaded)));

        let (_sender, receiver) = mpsc::sync_channel(1);
        let wake = TransportWake::default();
        assert!(matches!(
            wait_for_transport_command(&receiver, &PendingLoad::new(), &wake, true),
            TransportWaitResult::TimedOut
        ));

        let shared = SharedSnapshot::new();
        shared.playing.store(true, Ordering::Release);
        let (ended_player, _ended_queue) = Player::new();
        publish_snapshot(&shared, Some(&ended_player), Some(&loaded));
        assert_eq!(shared.snapshot().position_millis, 1_000);
        assert!(!shared.snapshot().playing);
        assert!(!transport_is_actively_playing(
            Some(&ended_player),
            Some(&loaded)
        ));
    }

    #[test]
    fn requested_gain_is_applied_before_play_resume() {
        let generation = 4;
        let shared = Arc::new(SharedSnapshot::new());
        shared
            .requested_generation
            .store(generation, Ordering::Release);
        shared
            .requested_volume
            .store(2.75_f32.to_bits(), Ordering::Release);
        let (player_handle, _queue) = Player::new();
        player_handle.append(SamplesBuffer::new(
            std::num::NonZeroU16::new(1).expect("one channel is non-zero"),
            std::num::NonZeroU32::new(48_000).expect("sample rate is non-zero"),
            vec![0.0; 48_000],
        ));
        player_handle.pause();
        let mut player = Some(player_handle);
        let mut loaded = Some(LoadedTrack {
            generation,
            ticket: test_ticket("gain-before-resume.wav"),
            duration_millis: 1_000,
        });
        let mut live_session = None;
        let mut applied_volume = None;

        handle_command(
            Command::Play {
                token: 9,
                generation,
            },
            &shared,
            None,
            &mut player,
            &mut loaded,
            &mut live_session,
            &mut applied_volume,
        );

        let player = player
            .as_ref()
            .expect("the resumed player should remain loaded");
        assert_eq!(player.volume(), 2.75);
        assert!(!player.is_paused());
        assert_eq!(shared.snapshot().acknowledged_token, 9);
    }

    #[test]
    fn disconnected_transport_worker_exits_after_wakeup() {
        let (commands, receiver) = mpsc::sync_channel(super::COMMAND_CAPACITY);
        let pending_load = PendingLoad::new();
        let wake = Arc::new(TransportWake::default());
        let worker_wake = Arc::clone(&wake);
        let (ready_sender, ready_receiver) = mpsc::channel();
        let (done_sender, done_receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            worker_wake.register();
            ready_sender
                .send(())
                .expect("disconnect wait test should start");
            let result = wait_for_transport_command(&receiver, &pending_load, &worker_wake, false);
            done_sender
                .send(result)
                .expect("disconnect wait test should report");
        });

        ready_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("disconnect wait test should register its worker");
        drop(commands);
        wake.notify();
        assert!(matches!(
            done_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("disconnect should wake and stop the wait"),
            TransportWaitResult::Disconnected
        ));
        worker
            .join()
            .expect("transport wait worker should cleanly exit");
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
            ticket: test_ticket("first.wav"),
            duration_millis: 1_000,
        });
        pending.replace(Command::Load {
            token: 2,
            generation: 2,
            ticket: test_ticket("second.wav"),
            duration_millis: 2_000,
        });
        assert_eq!(
            pending.take().and_then(|command| command.load_generation()),
            Some(2)
        );
    }

    #[test]
    fn reload_rejects_replaced_ticket_and_retires_playback_state() {
        let path = std::env::temp_dir().join(format!(
            "cadence-transport-ticket-reload-{}",
            std::process::id()
        ));
        fs::write(&path, b"original").expect("fixture should write");
        let verified = crate::source::open_and_hash(&path, || false)
            .expect("source should hash before playback");
        let ticket = verified.ticket();
        drop(verified);
        fs::write(&path, b"replaced").expect("replacement should write");

        let shared = Arc::new(SharedSnapshot::new());
        shared.requested_generation.store(9, Ordering::Release);
        shared.ready.store(true, Ordering::Release);
        let mut player = None;
        let mut loaded = None;
        let mut live_session = None;
        assert!(load_track(
            9,
            ticket,
            1_000,
            &shared,
            None,
            &mut player,
            &mut loaded,
            &mut live_session,
        ));
        let error = shared
            .take_error(9)
            .expect("replacement should surface a source error");
        assert!(error.contains("opened handle stamp no longer matches"));
        assert!(player.is_none());
        assert!(loaded.is_none());
        assert!(!shared.snapshot().ready);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn play_reload_command_rejects_replaced_ticket_atomically() {
        let generation = 11;
        let token = 101;
        let (path, ticket) = replaced_ticket_fixture("play");
        let (shared, session) = active_test_session(generation);
        shared.ready.store(true, Ordering::Release);
        shared.playing.store(true, Ordering::Release);
        let (empty_player, _queue) = Player::new();
        let mut player = Some(empty_player);
        let mut loaded = Some(LoadedTrack {
            generation,
            ticket,
            duration_millis: 1_000,
        });
        let mut live_session = Some(Arc::clone(&session));
        let mut applied_volume = None;

        handle_command(
            Command::Play { token, generation },
            &shared,
            None,
            &mut player,
            &mut loaded,
            &mut live_session,
            &mut applied_volume,
        );

        let error = shared
            .take_error(generation)
            .expect("play reload should publish a source error");
        assert!(error.contains("Audio source changed"));
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.generation, generation);
        assert_eq!(snapshot.acknowledged_token, token);
        assert!(!snapshot.ready);
        assert!(!snapshot.playing);
        assert!(player.is_none());
        assert!(loaded.is_none());
        assert!(live_session.is_none());
        assert!(!session.active.load(Ordering::Acquire));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn seek_reload_command_rejects_replaced_ticket_atomically() {
        let generation = 12;
        let token = 102;
        let (path, ticket) = replaced_ticket_fixture("seek");
        let (shared, session) = active_test_session(generation);
        shared.ready.store(true, Ordering::Release);
        shared.playing.store(true, Ordering::Release);
        let (empty_player, _queue) = Player::new();
        let mut player = Some(empty_player);
        let mut loaded = Some(LoadedTrack {
            generation,
            ticket,
            duration_millis: 1_000,
        });
        let mut live_session = Some(Arc::clone(&session));
        let mut applied_volume = None;

        handle_command(
            Command::Seek {
                token,
                generation,
                position_millis: 250,
                resume: true,
            },
            &shared,
            None,
            &mut player,
            &mut loaded,
            &mut live_session,
            &mut applied_volume,
        );

        let error = shared
            .take_error(generation)
            .expect("seek reload should publish a source error");
        assert!(error.contains("Audio source changed"));
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.generation, generation);
        assert_eq!(snapshot.acknowledged_token, token);
        assert!(!snapshot.ready);
        assert!(!snapshot.playing);
        assert!(player.is_none());
        assert!(loaded.is_none());
        assert!(live_session.is_none());
        assert!(!session.active.load(Ordering::Acquire));
        let _ = fs::remove_file(path);
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

    fn spectrum_levels(level: f32) -> [f32; LIVE_SPECTRUM_POINT_COUNT] {
        [level; LIVE_SPECTRUM_POINT_COUNT]
    }

    fn encoded_history_row(index: usize) -> [u8; LIVE_SPECTROGRAM_BAND_COUNT] {
        let mut row = [0_u8; LIVE_SPECTROGRAM_BAND_COUNT];
        let encoded_index = u16::try_from(index).expect("history test index fits in two bytes");
        row[..2].copy_from_slice(&encoded_index.to_le_bytes());
        for (band, value) in row.iter_mut().enumerate().skip(2) {
            *value = encoded_index.wrapping_add(band as u16).to_le_bytes()[0];
        }
        row
    }

    fn analyzer_with_one_frame() -> LiveAnalyzer {
        let mut analyzer = LiveAnalyzer::new(48_000);
        for index in 0..LIVE_SPECTRUM_FFT_SIZE {
            assert_eq!(
                analyzer.push((index as f32 * 0.01).sin()),
                index + 1 == LIVE_SPECTRUM_FFT_SIZE
            );
        }
        analyzer
    }

    fn push_analyzer_hop(analyzer: &mut LiveAnalyzer, offset: usize) {
        for index in 0..LIVE_SPECTRUM_HOP_SIZE {
            assert_eq!(
                analyzer.push(((offset + index) as f32 * 0.02).cos()),
                index + 1 == LIVE_SPECTRUM_HOP_SIZE
            );
        }
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
        attack.smooth_spectrum(spectrum_levels(0.0));
        let rising = attack.smooth_spectrum(spectrum_levels(1.0));

        let mut release = LiveAnalyzer::new(48_000);
        release.smooth_spectrum(spectrum_levels(1.0));
        let falling = release.smooth_spectrum(spectrum_levels(0.0));

        assert!(rising[0] > 0 && rising[0] < u8::MAX);
        assert!(falling[0] > 0 && falling[0] < u8::MAX);
        let attack_delta = rising[0] as u16;
        let release_delta = u8::MAX as u16 - falling[0] as u16;
        assert!(
            attack_delta > release_delta,
            "attack_delta={attack_delta}, release_delta={release_delta}"
        );
        let hop_seconds = LIVE_SPECTRUM_HOP_SIZE as f32 / 48_000.0;
        assert!(
            (attack.spectrum_attack_coefficient - (1.0 - (-hop_seconds / 0.030).exp())).abs()
                < 1.0e-6
        );
        assert!(
            (release.spectrum_release_coefficient - (1.0 - (-hop_seconds / 0.160).exp())).abs()
                < 1.0e-6
        );
    }

    #[test]
    fn analyzer_history_retains_raw_step_while_spectrum_payload_is_smoothed() {
        let mut analyzer = LiveAnalyzer::new(48_000);
        let quiet = [0_u8; LIVE_SPECTROGRAM_BAND_COUNT];
        let mut loud = [0_u8; LIVE_SPECTROGRAM_BAND_COUNT];
        loud[0] = u8::MAX;

        analyzer.record_analyzed_row(quiet, spectrum_levels(0.0));
        analyzer.record_analyzed_row(loud, spectrum_levels(1.0));

        let frame = analyzer.frame(1, 1).expect("two analyzer rows");
        assert_eq!(frame.value(0, 0), 0);
        assert_eq!(frame.value(1, 0), u8::MAX);
        assert_eq!(frame.spectrum_values.len(), LIVE_SPECTRUM_POINT_COUNT);
        assert_eq!(frame.spectrum_value(0), frame.spectrum_values[0]);
        assert!(frame.spectrum_value(0) > 0);
        assert!(frame.spectrum_value(0) < u8::MAX);

        analyzer.reset();
        assert_eq!(analyzer.row_count, 0);
        assert!(analyzer.spectrum_levels.iter().all(|&level| level == 0.0));
        assert!(analyzer.spectrum_values.iter().all(|&value| value == 0));
        assert!(analyzer.frame(1, 1).is_none());

        analyzer.record_analyzed_row([0_u8; LIVE_SPECTROGRAM_BAND_COUNT], spectrum_levels(1.0));
        analyzer.reset_after_pause();
        assert_eq!(analyzer.row_count, 0);
        assert!(analyzer.spectrum_levels.iter().all(|&level| level == 0.0));
        assert!(analyzer.spectrum_values.iter().all(|&value| value == 0));
    }

    #[test]
    fn analyzer_history_circular_buffer_preserves_newest_capacity_after_multiple_wraps() {
        let mut analyzer = LiveAnalyzer::new(48_000);
        let insertion_count = LIVE_SPECTROGRAM_MAX_HISTORY * 2 + 17;

        for index in 0..insertion_count {
            analyzer.record_analyzed_row(encoded_history_row(index), spectrum_levels(0.0));
        }

        assert_eq!(analyzer.row_count, LIVE_SPECTROGRAM_MAX_HISTORY);
        assert_eq!(
            analyzer.history_start,
            (insertion_count - LIVE_SPECTROGRAM_MAX_HISTORY) % LIVE_SPECTROGRAM_MAX_HISTORY
        );

        let frame = analyzer
            .frame(1, 1)
            .expect("wrapped analyzer history should publish");
        let mut expected =
            Vec::with_capacity(LIVE_SPECTROGRAM_MAX_HISTORY * LIVE_SPECTROGRAM_BAND_COUNT);
        for index in insertion_count - LIVE_SPECTROGRAM_MAX_HISTORY..insertion_count {
            expected.extend_from_slice(&encoded_history_row(index));
        }

        assert_eq!(frame.values.as_ref(), expected.as_slice());
        assert!(Arc::ptr_eq(frame.packed_values(), &frame.values));
    }

    #[test]
    fn analyzer_history_reset_after_wrap_drops_stale_rows_for_reset_and_pause() {
        let mut analyzer = LiveAnalyzer::new(48_000);
        for index in 0..LIVE_SPECTROGRAM_MAX_HISTORY + 9 {
            analyzer.record_analyzed_row(encoded_history_row(index), spectrum_levels(0.0));
        }
        assert_ne!(analyzer.history_start, 0);

        analyzer.reset();
        assert_eq!(analyzer.history_start, 0);
        assert_eq!(analyzer.row_count, 0);
        assert!(analyzer.frame(1, 1).is_none());

        for index in 10_000..10_003 {
            analyzer.record_analyzed_row(encoded_history_row(index), spectrum_levels(0.0));
        }
        let reset_frame = analyzer
            .frame(1, 1)
            .expect("post-reset rows should publish");
        let mut reset_values = Vec::new();
        for index in 10_000..10_003 {
            reset_values.extend_from_slice(&encoded_history_row(index));
        }
        assert_eq!(reset_frame.values.as_ref(), reset_values.as_slice());

        analyzer.reset_after_pause();
        assert_eq!(analyzer.history_start, 0);
        assert_eq!(analyzer.row_count, 0);
        assert!(analyzer.frame(1, 1).is_none());

        for index in 20_000..20_002 {
            analyzer.record_analyzed_row(encoded_history_row(index), spectrum_levels(0.0));
        }
        let pause_reset_frame = analyzer
            .frame(1, 1)
            .expect("post-pause-reset rows should publish");
        let mut pause_reset_values = Vec::new();
        for index in 20_000..20_002 {
            pause_reset_values.extend_from_slice(&encoded_history_row(index));
        }
        assert_eq!(
            pause_reset_frame.values.as_ref(),
            pause_reset_values.as_slice()
        );
    }

    #[test]
    fn analyzer_defaults_and_frame_shapes_stay_bounded() {
        assert_eq!(LIVE_SPECTROGRAM_BAND_COUNT, 128);
        assert_eq!(LIVE_SPECTRUM_POINT_COUNT, 768);
        assert_eq!(LIVE_SPECTROGRAM_MAX_HISTORY, 240);
        assert_eq!(LIVE_SPECTRUM_FFT_SIZE, 2_048);
        assert_eq!(LIVE_SPECTRUM_HOP_SIZE, 512);
        assert_eq!(LIVE_SPECTRUM_HOP_SIZE, LIVE_SPECTRUM_FFT_SIZE / 4);

        let mut analyzer = LiveAnalyzer::new(48_000);
        for index in 0..=LIVE_SPECTROGRAM_MAX_HISTORY {
            let mut row = [0_u8; LIVE_SPECTROGRAM_BAND_COUNT];
            row[index % LIVE_SPECTROGRAM_BAND_COUNT] = index as u8;
            analyzer.record_analyzed_row(row, spectrum_levels(0.0));
        }

        assert_eq!(analyzer.row_count, LIVE_SPECTROGRAM_MAX_HISTORY);
        let frame = analyzer.frame(1, 1).expect("bounded analyzer frame");
        assert_eq!(
            frame.values.len(),
            LIVE_SPECTROGRAM_MAX_HISTORY * LIVE_SPECTROGRAM_BAND_COUNT
        );
        assert_eq!(frame.spectrum_values.len(), LIVE_SPECTRUM_POINT_COUNT);
        assert!(frame.is_valid());
        assert!(
            LiveSpectrogramFrame::from_values(
                1,
                1,
                1,
                48_000,
                1,
                Arc::from(vec![0_u8; LIVE_SPECTROGRAM_BAND_COUNT].into_boxed_slice()),
                Arc::from(vec![0_u8; LIVE_SPECTROGRAM_BAND_COUNT].into_boxed_slice()),
            )
            .is_none()
        );
    }

    #[test]
    fn packed_history_bytes_preserve_quantized_band_order() {
        let values = Arc::from(
            (0..LIVE_SPECTROGRAM_BAND_COUNT)
                .map(|value| value as u8)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let spectrum = Arc::from(vec![0_u8; LIVE_SPECTRUM_POINT_COUNT].into_boxed_slice());
        let frame = LiveSpectrogramFrame::from_values(1, 1, 1, 48_000, 1, values, spectrum)
            .expect("valid packed history frame");

        assert_eq!(
            &frame.packed_values()[..LIVE_SPECTROGRAM_BAND_COUNT],
            frame.values.as_ref()
        );
        assert!(Arc::ptr_eq(frame.packed_values(), &frame.values));
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
    fn analyzer_precomputes_periodic_hann_and_one_sided_calibration() {
        let analyzer = LiveAnalyzer::new(48_000);
        let window_sum = analyzer.window_coefficients.iter().copied().sum::<f32>();
        let expected_unique_bin_calibration = 1.0 / window_sum;
        let expected_interior_bin_calibration = 2.0 / window_sum;

        assert!(window_sum.is_finite() && window_sum > 0.0);
        assert!(analyzer.window_coefficients[0].abs() < f32::EPSILON);
        assert!(
            analyzer
                .window_coefficients
                .iter()
                .all(|coefficient| coefficient.is_finite())
        );
        assert!(analyzer.window_coefficients[LIVE_SPECTRUM_FFT_SIZE - 1] > 0.0);
        assert!(
            (analyzer.one_sided_bin_calibration[0] - expected_unique_bin_calibration).abs()
                < 1.0e-9
        );
        assert!(
            (analyzer.one_sided_bin_calibration[1] - expected_interior_bin_calibration).abs()
                < 1.0e-9
        );
        assert!(
            (analyzer.one_sided_bin_calibration[analyzer.one_sided_bin_calibration.len() - 1]
                - expected_unique_bin_calibration)
                .abs()
                < 1.0e-9
        );
    }

    #[test]
    fn analyzer_calibrates_bin_centered_interior_sine_to_peak_dbfs() {
        let sample_rate = 48_000.0_f32;
        let bin = 64_usize;
        let expected_decibels = -18.0_f32;
        let amplitude = 10.0_f32.powf(expected_decibels / 20.0);
        let mut analyzer = LiveAnalyzer::new(sample_rate as u32);

        for index in 0..LIVE_SPECTRUM_FFT_SIZE {
            let phase =
                std::f32::consts::TAU * bin as f32 * index as f32 / LIVE_SPECTRUM_FFT_SIZE as f32;
            assert_eq!(
                analyzer.push(amplitude * phase.sin()),
                index + 1 == LIVE_SPECTRUM_FFT_SIZE
            );
        }

        let calibrated_magnitude = analyzer.positive_magnitudes[bin];
        let calibrated_decibels = 20.0 * calibrated_magnitude.log10();
        assert!(
            (calibrated_decibels - expected_decibels).abs() <= 0.75,
            "calibrated_decibels={calibrated_decibels}, expected_decibels={expected_decibels}"
        );
    }

    #[test]
    fn analyzer_calibration_preserves_bin_centered_amplitude_spacing() {
        let bin = 64_usize;
        let decode_bin_decibels = |expected_decibels: f32| {
            let amplitude = 10.0_f32.powf(expected_decibels / 20.0);
            let mut analyzer = LiveAnalyzer::new(48_000);
            for index in 0..LIVE_SPECTRUM_FFT_SIZE {
                let phase = std::f32::consts::TAU * bin as f32 * index as f32
                    / LIVE_SPECTRUM_FFT_SIZE as f32;
                assert_eq!(
                    analyzer.push(amplitude * phase.sin()),
                    index + 1 == LIVE_SPECTRUM_FFT_SIZE
                );
            }
            20.0 * analyzer.positive_magnitudes[bin].log10()
        };

        let quiet_decibels = decode_bin_decibels(-30.0);
        let loud_decibels = decode_bin_decibels(-12.0);
        let expected_spacing = 18.0;
        let decoded_spacing = loud_decibels - quiet_decibels;
        assert!(
            (decoded_spacing - expected_spacing).abs() <= 0.75,
            "decoded_spacing={decoded_spacing}, expected_spacing={expected_spacing}"
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
    fn spectrum_point_mapping_is_logarithmic_inclusive_and_nyquist_clamped() {
        let mappings = live_spectrum_point_mappings(48_000);
        assert_eq!(mappings.len(), LIVE_SPECTRUM_POINT_COUNT);
        assert_eq!(live_spectrum_point_frequency(48_000, 0), 20.0);
        assert_eq!(
            live_spectrum_point_frequency(48_000, LIVE_SPECTRUM_POINT_COUNT - 1),
            20_000.0
        );
        assert_eq!(live_spectrum_point_frequency(48_000, 0), 20.0);
        assert_eq!(
            live_spectrum_point_frequency(48_000, LIVE_SPECTRUM_POINT_COUNT - 1),
            20_000.0
        );
        assert!((0..LIVE_SPECTRUM_POINT_COUNT - 1).all(|point| {
            live_spectrum_point_frequency(48_000, point)
                < live_spectrum_point_frequency(48_000, point + 1)
        }));
        assert!(
            mappings
                .windows(2)
                .all(|pair| pair[0].display_tilt_db < pair[1].display_tilt_db)
        );

        let nyquist_mappings = live_spectrum_point_mappings(16_000);
        assert_eq!(
            live_spectrum_point_frequency(16_000, LIVE_SPECTRUM_POINT_COUNT - 1),
            8_000.0
        );
        assert!(nyquist_mappings.iter().all(|mapping| {
            mapping.max_bin_end <= super::LIVE_SPECTRUM_POSITIVE_BIN_COUNT
                && mapping.interpolation_upper_bin < super::LIVE_SPECTRUM_POSITIVE_BIN_COUNT
        }));
        assert!(
            nyquist_mappings[LIVE_SPECTRUM_POINT_COUNT - 1].max_bin_start
                < nyquist_mappings[LIVE_SPECTRUM_POINT_COUNT - 1].max_bin_end
        );
        assert_eq!(
            nyquist_mappings[LIVE_SPECTRUM_POINT_COUNT - 1].max_bin_end,
            super::LIVE_SPECTRUM_POSITIVE_BIN_COUNT
        );
    }

    #[test]
    fn spectrum_hybrid_mapping_preserves_bin_peaks_and_interpolates_gaps() {
        let mut analyzer = LiveAnalyzer::new(48_000);
        let max_point = analyzer
            .spectrum_point_mappings
            .iter()
            .position(|mapping| mapping.max_bin_start < mapping.max_bin_end)
            .expect("at least one spectrum point should contain a bin center");
        let max_mapping = analyzer.spectrum_point_mappings[max_point];
        analyzer.positive_magnitudes[max_mapping.max_bin_start] = 0.25;
        let max_levels = analyzer.spectrum_target_levels();
        let expected_max = ((20.0 * 0.25_f32.log10() + max_mapping.display_tilt_db
            - LIVE_SPECTRUM_DISPLAY_FLOOR_DB)
            / (LIVE_SPECTRUM_DISPLAY_CEILING_DB - LIVE_SPECTRUM_DISPLAY_FLOOR_DB))
            .clamp(0.0, 1.0);
        assert!((max_levels[max_point] - expected_max).abs() < 1.0e-6);

        let interpolation_point = analyzer
            .spectrum_point_mappings
            .iter()
            .position(|mapping| {
                mapping.max_bin_start == mapping.max_bin_end
                    && mapping.interpolation_lower_bin != mapping.interpolation_upper_bin
                    && mapping.interpolation_fraction > 0.25
                    && mapping.interpolation_fraction < 0.75
            })
            .expect("at least one spectrum point should interpolate between bins");
        let interpolation_mapping = analyzer.spectrum_point_mappings[interpolation_point];
        let lower_magnitude = 10.0_f32.powf(-30.0 / 20.0);
        let upper_magnitude = 10.0_f32.powf(-10.0 / 20.0);
        analyzer.positive_magnitudes[interpolation_mapping.interpolation_lower_bin] =
            lower_magnitude;
        analyzer.positive_magnitudes[interpolation_mapping.interpolation_upper_bin] =
            upper_magnitude;
        let interpolated_magnitude = lower_magnitude
            + (upper_magnitude - lower_magnitude) * interpolation_mapping.interpolation_fraction;
        let interpolated_decibels = 20.0 * interpolated_magnitude.log10();
        let expected_interpolated = ((interpolated_decibels
            + interpolation_mapping.display_tilt_db
            - LIVE_SPECTRUM_DISPLAY_FLOOR_DB)
            / (LIVE_SPECTRUM_DISPLAY_CEILING_DB - LIVE_SPECTRUM_DISPLAY_FLOOR_DB))
            .clamp(0.0, 1.0);
        let interpolated_levels = analyzer.spectrum_target_levels();
        assert!((interpolated_levels[interpolation_point] - expected_interpolated).abs() < 1.0e-6);

        let point_for_bin = |bin| {
            analyzer
                .spectrum_point_mappings
                .iter()
                .position(|mapping| mapping.max_bin_start <= bin && mapping.max_bin_end > bin)
                .expect("bin center should map to a spectrum point")
        };
        let nearby_low_point = point_for_bin(64);
        let nearby_high_point = point_for_bin(68);
        analyzer.positive_magnitudes[64] = 1.0;
        analyzer.positive_magnitudes[68] = 0.5;
        let nearby_levels = analyzer.spectrum_target_levels();
        assert!(nearby_levels[nearby_low_point] > 0.0);
        assert!(nearby_levels[nearby_high_point] > 0.0);
        assert_ne!(
            nearby_levels[nearby_low_point], nearby_levels[nearby_high_point],
            "nearby mapped bin peaks should remain distinct"
        );
    }

    #[test]
    fn spectrum_target_levels_apply_calibration_tilt_floor_and_ceiling() {
        let mut analyzer = LiveAnalyzer::new(48_000);
        let amplitude = 10.0_f32.powf(-18.0 / 20.0);
        analyzer.positive_magnitudes = [amplitude; super::LIVE_SPECTRUM_POSITIVE_BIN_COUNT];
        let target = analyzer.spectrum_target_levels();
        let nearest_point = |frequency: f32| {
            analyzer
                .spectrum_point_mappings
                .iter()
                .enumerate()
                .min_by(|(left_point, _), (right_point, _)| {
                    (live_spectrum_point_frequency(48_000, *left_point) - frequency)
                        .abs()
                        .total_cmp(
                            &(live_spectrum_point_frequency(48_000, *right_point) - frequency)
                                .abs(),
                        )
                })
                .map(|(point, _)| point)
                .expect("spectrum has points")
        };
        let reference_point = nearest_point(1_000.0);
        let low_point = nearest_point(500.0);
        let high_point = nearest_point(2_000.0);
        assert!((target[reference_point] - 0.8).abs() < 0.002);
        assert!((target[low_point] - 0.75).abs() < 0.003);
        assert!((target[high_point] - 0.85).abs() < 0.003);

        analyzer.positive_magnitudes = [1.0e-12; super::LIVE_SPECTRUM_POSITIVE_BIN_COUNT];
        let floor = analyzer.spectrum_target_levels();
        assert!(floor.iter().all(|&level| level == 0.0));
        analyzer.positive_magnitudes = [2.0; super::LIVE_SPECTRUM_POSITIVE_BIN_COUNT];
        let ceiling = analyzer.spectrum_target_levels();
        assert_eq!(ceiling[reference_point], 1.0);
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
    fn live_publication_advances_next_deadline_on_time() {
        assert_eq!(
            LIVE_PUBLICATION_INTERVAL,
            Duration::from_nanos(1_000_000_000 / LIVE_PUBLICATION_FPS)
        );

        let (shared, session) = active_test_session(7);
        let observed_epoch = session.current_epoch();
        let analyzer = analyzer_with_one_frame();
        let start = Instant::now();
        let mut published_revision = 0;
        let first_deadline = start + LIVE_PUBLICATION_INTERVAL;
        let mut next_publication_deadline = first_deadline;
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            first_deadline,
            false,
        );
        assert!(shared.latest_live_frame().is_some());
        assert_eq!(published_revision, analyzer.revision);
        assert_eq!(
            next_publication_deadline,
            first_deadline + LIVE_PUBLICATION_INTERVAL
        );
        session.retire();
    }

    #[test]
    fn live_publication_preserves_phase_when_late() {
        let (shared, session) = active_test_session(7);
        let observed_epoch = session.current_epoch();
        let analyzer = analyzer_with_one_frame();
        let start = Instant::now();
        let first_deadline = start + LIVE_PUBLICATION_INTERVAL;
        let late = first_deadline + Duration::from_millis(5);
        let mut published_revision = 0;
        let mut next_publication_deadline = first_deadline;
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            late,
            false,
        );
        assert_eq!(published_revision, analyzer.revision);
        assert_eq!(
            next_publication_deadline,
            first_deadline + LIVE_PUBLICATION_INTERVAL
        );
        assert_ne!(
            next_publication_deadline,
            late + LIVE_PUBLICATION_INTERVAL,
            "a late publication must not retime the cadence"
        );
        session.retire();
    }

    #[test]
    fn live_publication_coalesces_multiple_late_intervals() {
        let (shared, session) = active_test_session(7);
        let observed_epoch = session.current_epoch();
        let mut analyzer = analyzer_with_one_frame();
        for index in 0..LIVE_SPECTRUM_HOP_SIZE {
            assert_eq!(
                analyzer.push((index as f32 * 0.02).cos()),
                index + 1 == LIVE_SPECTRUM_HOP_SIZE
            );
        }
        let start = Instant::now();
        let first_deadline = start + LIVE_PUBLICATION_INTERVAL;
        let late = first_deadline
            + LIVE_PUBLICATION_INTERVAL
                .checked_mul(3)
                .expect("test interval multiplication should fit")
            + Duration::from_millis(5);
        let mut published_revision = 0;
        let mut next_publication_deadline = first_deadline;
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            late,
            false,
        );
        assert_eq!(published_revision, analyzer.revision);
        assert_eq!(
            next_publication_deadline,
            first_deadline
                + LIVE_PUBLICATION_INTERVAL
                    .checked_mul(4)
                    .expect("test interval multiplication should fit")
        );

        let published_frame = shared
            .latest_live_frame()
            .expect("the newest frame should publish once after lateness");
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            late,
            false,
        );
        let still_published_frame = shared
            .latest_live_frame()
            .expect("lateness must not cause a catch-up publication burst");
        assert!(Arc::ptr_eq(&still_published_frame, &published_frame));
        session.retire();
    }

    #[test]
    fn live_publication_caps_sustained_input_at_30_hz_without_bursting() {
        assert_eq!(LIVE_PUBLICATION_FPS, 30);
        assert_eq!(
            LIVE_PUBLICATION_INTERVAL,
            Duration::from_nanos(1_000_000_000 / LIVE_PUBLICATION_FPS)
        );

        let (shared, session) = active_test_session(7);
        let observed_epoch = session.current_epoch();
        let mut analyzer = analyzer_with_one_frame();
        let start = Instant::now();
        let first_deadline = start + LIVE_PUBLICATION_INTERVAL;
        let mut published_revision = 0;
        let mut next_publication_deadline = first_deadline;
        let mut normal_publication_count = 0;

        for slot in 0..4_u32 {
            let deadline = first_deadline
                + LIVE_PUBLICATION_INTERVAL
                    .checked_mul(slot)
                    .expect("test interval multiplication should fit");
            for hop in 0..3 {
                push_analyzer_hop(&mut analyzer, slot as usize * 3 + hop);
                let early = deadline
                    .checked_sub(Duration::from_nanos(1))
                    .expect("test deadline should be representable");
                let published_before = published_revision;
                publish_live_frame_if_due_at(
                    &analyzer,
                    &session,
                    &shared,
                    observed_epoch,
                    &mut published_revision,
                    &mut next_publication_deadline,
                    early,
                    false,
                );
                assert_eq!(
                    published_revision, published_before,
                    "sustained input must not publish before its scheduled slot"
                );
            }

            let published_before = published_revision;
            publish_live_frame_if_due_at(
                &analyzer,
                &session,
                &shared,
                observed_epoch,
                &mut published_revision,
                &mut next_publication_deadline,
                deadline,
                false,
            );
            assert!(published_revision > published_before);
            assert_eq!(published_revision, analyzer.revision);
            normal_publication_count += 1;
            assert_eq!(
                next_publication_deadline,
                first_deadline
                    + LIVE_PUBLICATION_INTERVAL
                        .checked_mul(slot + 1)
                        .expect("test interval multiplication should fit"),
                "publication deadlines must retain their original phase"
            );
        }

        assert_eq!(
            normal_publication_count, 4,
            "three sustained analyzer revisions per slot must still publish once per 30 Hz slot"
        );

        for hop in 0..6 {
            push_analyzer_hop(&mut analyzer, 12 + hop);
        }
        let missed_slots_now = first_deadline
            + LIVE_PUBLICATION_INTERVAL
                .checked_mul(7)
                .expect("test interval multiplication should fit")
            + Duration::from_nanos(1);
        let missed_slots_deadline = first_deadline
            + LIVE_PUBLICATION_INTERVAL
                .checked_mul(8)
                .expect("test interval multiplication should fit");
        let published_before_missed_slots = published_revision;
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            missed_slots_now,
            false,
        );
        assert!(published_revision > published_before_missed_slots);
        assert_eq!(published_revision, analyzer.revision);
        assert_eq!(
            next_publication_deadline, missed_slots_deadline,
            "missed slots must advance to the next original phase boundary"
        );
        let published_frame = shared
            .latest_live_frame()
            .expect("the late sustained-input frame should be visible");
        let published_after_missed_slots = published_revision;
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            missed_slots_now,
            false,
        );
        let still_published_frame = shared
            .latest_live_frame()
            .expect("a repeated late call should retain the newest frame");
        assert_eq!(published_revision, published_after_missed_slots);
        assert!(Arc::ptr_eq(&still_published_frame, &published_frame));
        assert_eq!(next_publication_deadline, missed_slots_deadline);

        push_analyzer_hop(&mut analyzer, 18);
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            missed_slots_deadline,
            false,
        );
        assert_eq!(published_revision, analyzer.revision);
        assert_eq!(
            next_publication_deadline,
            first_deadline
                + LIVE_PUBLICATION_INTERVAL
                    .checked_mul(9)
                    .expect("test interval multiplication should fit")
        );
        session.retire();
    }

    #[test]
    fn live_publication_ignores_early_and_duplicate_revisions() {
        let (shared, session) = active_test_session(7);
        let observed_epoch = session.current_epoch();
        let analyzer = analyzer_with_one_frame();
        let start = Instant::now();
        let first_deadline = start + LIVE_PUBLICATION_INTERVAL;
        let mut published_revision = 0;
        let mut next_publication_deadline = first_deadline;
        let early = first_deadline
            .checked_sub(Duration::from_nanos(1))
            .expect("test deadline should be representable");
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            early,
            false,
        );
        assert_eq!(published_revision, 0);
        assert!(shared.latest_live_frame().is_none());
        assert_eq!(next_publication_deadline, first_deadline);

        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            first_deadline,
            false,
        );
        let first_frame = shared
            .latest_live_frame()
            .expect("the revision should publish at its deadline");
        let duplicate_due = next_publication_deadline;
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            duplicate_due,
            false,
        );
        let duplicate_frame = shared
            .latest_live_frame()
            .expect("the duplicate should leave the latest frame unchanged");
        assert!(Arc::ptr_eq(&duplicate_frame, &first_frame));
        assert_eq!(published_revision, analyzer.revision);
        assert_eq!(next_publication_deadline, duplicate_due);
        session.retire();
    }

    #[test]
    fn live_publication_keeps_empty_due_slot_open_for_newer_revision() {
        let (shared, session) = active_test_session(7);
        let observed_epoch = session.current_epoch();
        let mut analyzer = LiveAnalyzer::new(48_000);
        let start = Instant::now();
        let first_deadline = start + LIVE_PUBLICATION_INTERVAL;
        let mut published_revision = 0;
        let mut next_publication_deadline = first_deadline;
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            first_deadline,
            false,
        );
        assert_eq!(published_revision, 0);
        assert!(shared.latest_live_frame().is_none());
        assert_eq!(next_publication_deadline, first_deadline);

        for index in 0..LIVE_SPECTRUM_FFT_SIZE {
            assert_eq!(
                analyzer.push((index as f32 * 0.01).sin()),
                index + 1 == LIVE_SPECTRUM_FFT_SIZE
            );
        }
        let just_after_empty_slot = first_deadline + Duration::from_nanos(1);
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            just_after_empty_slot,
            false,
        );
        assert_eq!(published_revision, analyzer.revision);
        assert!(shared.latest_live_frame().is_some());
        assert_eq!(
            next_publication_deadline,
            first_deadline + LIVE_PUBLICATION_INTERVAL
        );
        assert!(next_publication_deadline > just_after_empty_slot);
        session.retire();
    }

    #[test]
    fn forced_terminal_publication_is_newer_only_and_does_not_retime_schedule() {
        let (shared, session) = active_test_session(7);
        let observed_epoch = session.current_epoch();
        let mut analyzer = analyzer_with_one_frame();
        let start = Instant::now();
        let scheduled_deadline = start + LIVE_PUBLICATION_INTERVAL;
        let mut published_revision = 0;
        let mut next_publication_deadline = scheduled_deadline;
        let final_now = start + Duration::from_millis(1);
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            final_now,
            true,
        );
        let first_frame = shared
            .latest_live_frame()
            .expect("forced terminal publication should publish a newer frame");
        assert_eq!(published_revision, analyzer.revision);
        assert_eq!(next_publication_deadline, scheduled_deadline);

        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            final_now,
            true,
        );
        let duplicate_frame = shared
            .latest_live_frame()
            .expect("the forced terminal publication should remain visible");
        assert!(Arc::ptr_eq(&duplicate_frame, &first_frame));
        assert_eq!(published_revision, analyzer.revision);
        assert_eq!(next_publication_deadline, scheduled_deadline);

        for index in 0..LIVE_SPECTRUM_HOP_SIZE {
            assert_eq!(
                analyzer.push((index as f32 * 0.02).cos()),
                index + 1 == LIVE_SPECTRUM_HOP_SIZE
            );
        }
        publish_live_frame_if_due_at(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
            final_now,
            true,
        );
        let newer_frame = shared
            .latest_live_frame()
            .expect("a forced publication may replace the terminal frame once");
        assert!(!Arc::ptr_eq(&newer_frame, &first_frame));
        assert_eq!(published_revision, analyzer.revision);
        session.retire();
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
        let mut next_publication_deadline = Instant::now();
        publish_live_frame_if_due(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
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
        let mut next_publication_deadline = Instant::now();
        publish_live_frame_if_due(
            &analyzer,
            &session,
            &shared,
            observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
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
        let mut applied_volume = None;
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
            &mut applied_volume,
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
            &mut next_publication_deadline,
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
        next_publication_deadline = Instant::now()
            .checked_add(Duration::from_secs(1))
            .expect("the test publication deadline should be representable");
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
            &mut next_publication_deadline,
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
        next_publication_deadline = Instant::now()
            .checked_sub(super::LIVE_PUBLICATION_INTERVAL)
            .expect("the test publication deadline should be representable");
        let (consumed, frozen) = run_live_analyzer_iteration(
            &mut consumer,
            &session,
            &shared,
            &mut analyzer,
            &mut observed_epoch,
            &mut published_revision,
            &mut next_publication_deadline,
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

    #[test]
    fn frozen_live_analyzer_parks_until_resume_and_retire_wakes_it() {
        let shared = Arc::new(SharedSnapshot::new());
        shared.requested_generation.store(12, Ordering::Release);
        let session = Arc::new(LiveCaptureSession::new(12, 1, 1));
        shared.begin_live_session(&session);
        let (_producer, consumer) = super::RingBuffer::new(8);
        let (done_sender, done_receiver) = mpsc::channel();
        let analyzer_session = Arc::clone(&session);
        let analyzer_shared = Arc::clone(&shared);
        let handle = std::thread::spawn(move || {
            run_live_analyzer(consumer, analyzer_session, analyzer_shared, 48_000);
            done_sender
                .send(())
                .expect("analyzer should report retirement");
        });

        let startup_deadline = Instant::now() + Duration::from_secs(1);
        while session.analyzer_iteration_count() == 0 {
            assert!(Instant::now() < startup_deadline, "analyzer did not start");
            std::thread::yield_now();
        }
        let frozen_iterations = session.analyzer_iteration_count();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(session.analyzer_iteration_count(), frozen_iterations);
        assert_eq!(session.current_epoch(), 1);

        assert!(shared.set_live_analysis_frozen(&session, false));
        let resume_deadline = Instant::now() + Duration::from_secs(1);
        while session.analyzer_iteration_count() == frozen_iterations {
            assert!(
                Instant::now() < resume_deadline,
                "resume did not wake analyzer"
            );
            std::thread::yield_now();
        }
        assert_eq!(session.current_epoch(), 1);

        session.retire();
        assert!(done_receiver.recv_timeout(Duration::from_secs(1)).is_ok());
        handle.join().expect("analyzer thread should exit cleanly");
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
                Arc::from(vec![1_u8; LIVE_SPECTRUM_POINT_COUNT].into_boxed_slice()),
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
                Arc::from(vec![1_u8; LIVE_SPECTRUM_POINT_COUNT].into_boxed_slice()),
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
            test_ticket("fallback.wav"),
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
                Arc::from(vec![1_u8; LIVE_SPECTRUM_POINT_COUNT].into_boxed_slice()),
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

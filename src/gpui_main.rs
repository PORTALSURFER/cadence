// Shared modules still contain API used by the optional Radiant binary.
#![allow(dead_code)]

mod audio;
mod gpui_text_input;
mod signal_summary;
mod source;
mod storage;
mod transport;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::{
    App, AppContext, Application, Bounds, ClickEvent, Context, Entity, ExternalPaths, FocusHandle,
    Focusable, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Path as GpuiPath, PathBuilder, PathPromptOptions, Pixels, Render, ScrollHandle,
    StatefulInteractiveElement, Window, WindowBounds, WindowOptions, canvas, div, fill, point,
    prelude::*, px, size,
};

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 720.0;
const BACKGROUND: u32 = 0x0b0e0eff;
const SURFACE: u32 = 0x101313ff;
const SURFACE_RAISED: u32 = 0x151919ff;
const BORDER: u32 = 0x2b3030ff;
const TEXT: u32 = 0xe5e5e1ff;
const MUTED: u32 = 0xa4a6a4ff;
const CORAL: u32 = 0xff6057ff;
const CORAL_DARK: u32 = 0x2b1817ff;

fn panel() -> gpui::Div {
    div()
        .bg(gpui::rgba(SURFACE))
        .border_1()
        .border_color(gpui::rgba(BORDER))
}

fn section_title(label: impl Into<gpui::SharedString>) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .child(div().w(px(5.0)).h(px(26.0)).bg(gpui::rgba(0xd2443bff)))
        .child(
            div()
                .h(px(26.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .bg(gpui::rgba(CORAL))
                .text_color(gpui::rgba(BACKGROUND))
                .text_size(px(14.0))
                .child(label.into()),
        )
}

fn reference_display_name(reference: &storage::ReferenceTrack) -> String {
    reference.display_name.clone().unwrap_or_else(|| {
        reference
            .path
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| reference.path.display().to_string())
    })
}

fn format_timestamp(millis: u64) -> String {
    let seconds = millis / 1_000;
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes >= 60 {
        format!("{}:{:02}:{:02}", minutes / 60, minutes % 60, seconds)
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn format_precise_timestamp(millis: u64) -> String {
    format!("{}.{:03}", format_timestamp(millis), millis % 1_000)
}

struct CachedWaveformPath {
    generation: u64,
    bounds: Bounds<Pixels>,
    path: GpuiPath<Pixels>,
}

fn waveform_path(
    summary: &signal_summary::GpuSignalSummary,
    bounds: Bounds<Pixels>,
) -> Option<GpuiPath<Pixels>> {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let level = summary
        .levels
        .get(summary.level_for_frames_per_pixel(summary.frames as f32 / width))?;
    let band_count = summary.band_count.max(1);
    let bucket_count = level.buckets.len() / band_count;
    if bucket_count == 0 {
        return None;
    }
    let columns = width.ceil() as usize;
    let mut top = Vec::with_capacity(columns);
    let mut bottom = Vec::with_capacity(columns);
    let mid = f32::from(bounds.origin.y) + height * 0.5;
    for column in 0..columns {
        let bucket_index = ((column as f32 / width) * bucket_count as f32) as usize;
        let buckets = &level.buckets[bucket_index.min(bucket_count - 1) * band_count
            ..(bucket_index.min(bucket_count - 1) + 1) * band_count];
        let peak = buckets
            .iter()
            .fold(0.0_f32, |peak, bucket| {
                peak.max(bucket.min.abs()).max(bucket.max.abs())
            })
            .clamp(0.0, 1.0);
        let half = (peak * height * 0.45).max(0.5);
        let x = f32::from(bounds.origin.x) + column as f32;
        top.push(point(px(x), px(mid - half)));
        bottom.push(point(px(x), px(mid + half)));
    }
    bottom.reverse();
    top.extend(bottom);
    let mut builder = PathBuilder::fill();
    builder.add_polygon(&top, true);
    builder.build().ok()
}

fn cached_waveform_path(
    cache: &Arc<Mutex<Option<CachedWaveformPath>>>,
    generation: u64,
    summary: &signal_summary::GpuSignalSummary,
    bounds: Bounds<Pixels>,
) -> Option<GpuiPath<Pixels>> {
    let mut cache = cache.lock().ok()?;
    if let Some(cached) = cache.as_ref()
        && cached.generation == generation
        && cached.bounds == bounds
    {
        return Some(cached.path.clone());
    }
    let path = waveform_path(summary, bounds)?;
    *cache = Some(CachedWaveformPath {
        generation,
        bounds,
        path: path.clone(),
    });
    Some(path)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Review,
    Planner,
}

#[derive(Clone)]
enum WaveformDrag {
    Scrub {
        reference: bool,
        start_ratio: f32,
        moved: bool,
    },
    Note {
        reference: bool,
        id: String,
        start_ratio: f32,
        moved: bool,
    },
    Draft {
        reference: bool,
        start_ratio: f32,
        moved: bool,
    },
}

#[derive(Clone, Copy, PartialEq)]
struct WaveformHover {
    reference: bool,
    ratio: f32,
    comment: bool,
}

struct Cadence {
    library: storage::Library,
    focus_handle: FocusHandle,
    page: Page,
    status: String,
    library_ready: bool,
    busy: bool,
    confirm_remove: bool,
    confirm_remove_reference: bool,
    show_reference_picker: bool,
    transport: Option<transport::AudioTransport>,
    reference_transport: Option<transport::AudioTransport>,
    active_generation: u64,
    selected_waveform: Option<audio::WaveformData>,
    reference_waveform: Option<audio::WaveformData>,
    audible_reference: bool,
    match_loudness: bool,
    volume: f32,
    note_editor: Entity<gpui_text_input::TextInput>,
    reference_name_editor: Entity<gpui_text_input::TextInput>,
    editing_note_id: Option<String>,
    original_note_body: Option<String>,
    note_draft_time: Option<u64>,
    loop_start: f32,
    loop_end: f32,
    loop_enabled: bool,
    loop_seek_pending: bool,
    waveform_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    reference_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    main_paint_cache: Arc<Mutex<Option<CachedWaveformPath>>>,
    reference_paint_cache: Arc<Mutex<Option<CachedWaveformPath>>>,
    waveform_drag: Option<WaveformDrag>,
    waveform_hover: Option<WaveformHover>,
    hovered_note: Option<(bool, String)>,
    hovered_draft: Option<bool>,
    planner_scroll: ScrollHandle,
}

fn load_review_waveform(
    path: &std::path::Path,
    expected: Option<source::AudioSourceProof>,
) -> Result<audio::VerifiedWaveform, String> {
    if let Some(proof) = expected.as_ref() {
        let cache_path = storage::waveform_cache_path(path, proof);
        if let Some(cached) = audio::load_waveform_cache(path, &cache_path)
            && cached.ticket().proof() == proof
            && source::stamp_file(path).map_err(|error| error.to_string())?
                == cached.ticket().stamp()
        {
            cached
                .ticket()
                .validate_current(|| false)
                .map_err(|error| error.to_string())?;
            return Ok(audio::VerifiedWaveform::new(
                cached.waveform().clone(),
                cached.ticket().clone(),
            ));
        }
    }
    let decoded = audio::decode_audio_file(path)?;
    if expected
        .as_ref()
        .is_some_and(|proof| proof != decoded.source_proof())
    {
        return Err(format!(
            "Audio source changed since import: {}",
            path.display()
        ));
    }
    let ticket = decoded.source_ticket();
    let waveform = decoded.waveform().clone();
    let cache_path = storage::waveform_cache_path(path, decoded.source_proof());
    let _ = audio::write_waveform_cache_if_unchanged(path, &cache_path, &ticket, &waveform);
    Ok(audio::VerifiedWaveform::new(waveform, ticket))
}

impl Cadence {
    fn new(cx: &mut Context<Self>) -> Self {
        let (library, status, library_ready) = match storage::load_library() {
            Ok(library) => (library, String::from("Ready"), true),
            Err(error) => (
                storage::Library::default(),
                format!("Library unavailable: {error}"),
                false,
            ),
        };
        let transport = match transport::AudioTransport::try_spawn() {
            Ok(transport) => Some(transport),
            Err(error) => {
                eprintln!("Cadence audio unavailable: {error}");
                None
            }
        };
        let reference_transport = transport::AudioTransport::try_spawn().ok();
        let mut app = Self {
            library,
            focus_handle: cx.focus_handle(),
            page: Page::Review,
            status,
            library_ready,
            busy: false,
            confirm_remove: false,
            confirm_remove_reference: false,
            show_reference_picker: false,
            transport,
            reference_transport,
            active_generation: 0,
            selected_waveform: None,
            reference_waveform: None,
            audible_reference: false,
            match_loudness: false,
            volume: 0.8,
            note_editor: cx.new(|cx| gpui_text_input::TextInput::new(cx, "", "Write a comment")),
            reference_name_editor: cx
                .new(|cx| gpui_text_input::TextInput::new(cx, "", "Reference name")),
            editing_note_id: None,
            original_note_body: None,
            note_draft_time: None,
            loop_start: 0.0,
            loop_end: 1.0,
            loop_enabled: false,
            loop_seek_pending: false,
            waveform_bounds: Arc::new(Mutex::new(None)),
            reference_bounds: Arc::new(Mutex::new(None)),
            main_paint_cache: Arc::new(Mutex::new(None)),
            reference_paint_cache: Arc::new(Mutex::new(None)),
            waveform_drag: None,
            waveform_hover: None,
            hovered_note: None,
            hovered_draft: None,
            planner_scroll: ScrollHandle::new(),
        };
        app.start_monitor(cx);
        app.decode_selection(cx);
        app
    }

    fn selected_track(&self) -> Option<&storage::Track> {
        let id = self.library.selected_track_id.as_deref()?;
        self.library.tracks.iter().find(|track| track.id == id)
    }

    fn selected_reference(&self) -> Option<&storage::ReferenceTrack> {
        let path = self.selected_track()?.reference_path.as_ref()?;
        self.library
            .reference_tracks
            .iter()
            .find(|reference| &reference.path == path)
    }

    fn selected_notes_mut(&mut self) -> Option<&mut storage::SharedVec<storage::Note>> {
        if self.audible_reference {
            let path = self.selected_track()?.reference_path.clone()?;
            self.library
                .reference_tracks
                .find_mut(|reference| reference.path == path)
                .map(|reference| &mut reference.notes)
        } else {
            let id = self.library.selected_track_id.clone()?;
            self.library
                .tracks
                .find_mut(|track| track.id == id)
                .map(|track| &mut track.notes)
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.library_ready {
            self.status = String::from("Library unavailable; preserve it before making changes.");
            cx.notify();
            return;
        }
        match storage::persist_library(&self.library) {
            Ok(storage::PersistenceOutcome::Durable) => self.status = String::from("Saved"),
            Ok(storage::PersistenceOutcome::CommittedButDurabilityUncertain { detail }) => {
                self.status = format!("Saved, but durability is uncertain: {detail}");
            }
            Err(error) => self.status = format!("Save failed: {error}"),
        }
        cx.notify();
    }

    fn choose_import(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.library_ready {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import audio".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                let _ = this.update(cx, |app, cx| app.import_paths(paths, cx));
            }
        })
        .detach();
    }

    fn import_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.busy = true;
        self.status = format!("Importing {} file(s)…", paths.len());
        cx.notify();
        let library = self.library.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut candidates = Vec::new();
                    let mut errors = Vec::new();
                    for path in paths {
                        match audio::decode_audio_file(&path) {
                            Ok(decoded) => candidates
                                .push(storage::VerifiedImportCandidate::from_decoded(&decoded)),
                            Err(error) => errors.push(format!("{}: {error}", path.display())),
                        }
                    }
                    let report = storage::import_verified_batch(
                        library,
                        candidates,
                        &storage::library_path(),
                    );
                    errors.extend(
                        report
                            .errors
                            .into_iter()
                            .map(|error| format!("{}: {}", error.path.display(), error.error)),
                    );
                    (report.library, report.imported_paths.len(), errors)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if let Some(library) = result.0 {
                    app.library = library;
                    app.decode_selection(cx);
                }
                app.busy = false;
                app.status = if result.2.is_empty() {
                    format!("Imported {} file(s)", result.1)
                } else {
                    result.2.join("; ")
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy {
            return;
        }
        self.library.selected_track_id = Some(id);
        self.selected_waveform = None;
        self.reference_waveform = None;
        self.confirm_remove = false;
        self.confirm_remove_reference = false;
        self.show_reference_picker = false;
        self.editing_note_id = None;
        self.original_note_body = None;
        self.note_draft_time = None;
        self.loop_start = 0.0;
        self.loop_end = 1.0;
        self.loop_enabled = false;
        self.loop_seek_pending = false;
        self.waveform_drag = None;
        self.waveform_hover = None;
        self.hovered_note = None;
        self.hovered_draft = None;
        self.note_editor
            .update(cx, |editor, cx| editor.set_content("", cx));
        let reference_name = self
            .selected_reference()
            .map(|reference| {
                reference
                    .display_name
                    .clone()
                    .or_else(|| {
                        reference
                            .path
                            .file_stem()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        self.reference_name_editor
            .update(cx, |editor, cx| editor.set_content(reference_name, cx));
        self.save(cx);
        self.decode_selection(cx);
    }

    fn decode_selection(&mut self, cx: &mut Context<Self>) {
        self.active_generation = self.active_generation.wrapping_add(1);
        self.waveform_hover = None;
        self.hovered_note = None;
        self.hovered_draft = None;
        let generation = self.active_generation;
        if let Some(transport) = &self.transport {
            let _ = transport.unload(generation);
        }
        if let Some(transport) = &self.reference_transport {
            let _ = transport.unload(generation);
        }
        let Some(path) = self.selected_track().map(|track| track.path.clone()) else {
            return;
        };
        let expected_main_proof = self
            .selected_track()
            .and_then(|track| track.source_provenance().verified_proof().cloned());
        let reference_path = self
            .selected_track()
            .and_then(|track| track.reference_path.clone());
        let expected_reference_proof = reference_path.as_ref().and_then(|path| {
            self.library
                .reference_tracks
                .iter()
                .find(|reference| &reference.path == path)
                .and_then(|reference| reference.source_provenance().verified_proof().cloned())
        });
        self.status = format!("Loading {}…", path.display());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let main = load_review_waveform(&path, expected_main_proof);
                    let reference = reference_path
                        .map(|path| load_review_waveform(&path, expected_reference_proof));
                    (main, reference)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.active_generation != generation {
                    return;
                }
                match result.0 {
                    Ok(decoded) => {
                        if let Some(transport) = &app.transport {
                            if let Err(error) = transport.load(
                                generation,
                                decoded.ticket().clone(),
                                decoded.waveform().duration_millis,
                            ) {
                                app.status = error;
                            } else {
                                app.status = String::from("Ready to play");
                            }
                        }
                        app.selected_waveform = Some(decoded.waveform().clone());
                    }
                    Err(error) => app.status = error,
                }
                app.reference_waveform = None;
                if let Some(reference) = result.1 {
                    match reference {
                        Ok(decoded) => {
                            if let Some(transport) = &app.reference_transport
                                && let Err(error) = transport.load(
                                    generation,
                                    decoded.ticket().clone(),
                                    decoded.waveform().duration_millis,
                                )
                            {
                                app.status = error;
                            }
                            app.reference_waveform = Some(decoded.waveform().clone());
                        }
                        Err(error) => app.status = format!("Reference unavailable: {error}"),
                    }
                }
                app.sync_gains();
                cx.notify();
            });
        })
        .detach();
    }

    fn start_monitor(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut previous = transport::Snapshot::default();
            let mut previous_reference = transport::Snapshot::default();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if this
                    .update(cx, |app, cx| {
                        let snapshot = app
                            .transport
                            .as_ref()
                            .map(|transport| transport.snapshot())
                            .unwrap_or_default();
                        let reference = app
                            .reference_transport
                            .as_ref()
                            .map(|transport| transport.snapshot())
                            .unwrap_or_default();
                        if snapshot != previous || reference != previous_reference {
                            cx.notify();
                        }
                        previous = snapshot;
                        previous_reference = reference;
                        if let Some(error) = app
                            .transport
                            .as_ref()
                            .and_then(|transport| transport.take_error(app.active_generation))
                        {
                            app.status = error;
                            cx.notify();
                        }
                        if let Some(error) = app
                            .reference_transport
                            .as_ref()
                            .and_then(|transport| transport.take_error(app.active_generation))
                        {
                            app.status = format!("Reference: {error}");
                            cx.notify();
                        }
                        if app.loop_enabled
                            && snapshot.playing
                            && let Some(waveform) = &app.selected_waveform
                        {
                            let ratio = snapshot.position_millis as f32
                                / waveform.duration_millis.max(1) as f32;
                            if ratio < app.loop_end - 0.01 {
                                app.loop_seek_pending = false;
                            }
                            if ratio >= app.loop_end && !app.loop_seek_pending {
                                app.loop_seek_pending = true;
                                app.seek_ratio(app.loop_start, cx);
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn set_loop_point(&mut self, start: bool, cx: &mut Context<Self>) {
        let (Some(transport), Some(waveform)) = (&self.transport, &self.selected_waveform) else {
            return;
        };
        let ratio = (transport.snapshot().position_millis as f32
            / waveform.duration_millis.max(1) as f32)
            .clamp(0.0, 1.0);
        if start {
            self.loop_start = ratio.min(self.loop_end - 0.01);
        } else {
            self.loop_end = ratio.max(self.loop_start + 0.01);
        }
        self.loop_enabled = true;
        self.loop_seek_pending = false;
        cx.notify();
    }

    fn toggle_loop(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.loop_enabled = !self.loop_enabled;
        self.loop_seek_pending = false;
        cx.notify();
    }

    fn toggle_play(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_playback(cx);
    }

    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        if let Some(transport) = &self.transport {
            let snapshot = transport.snapshot();
            let result = if snapshot.playing {
                transport.pause(self.active_generation)
            } else {
                transport.play(self.active_generation)
            };
            if let Err(error) = result {
                self.status = error;
            } else if self.reference_waveform.is_some()
                && let Some(reference) = &self.reference_transport
            {
                let paired = if snapshot.playing {
                    reference.pause(self.active_generation)
                } else {
                    reference.play(self.active_generation)
                };
                if let Err(error) = paired {
                    let _ = transport.pause(self.active_generation);
                    let _ = reference.pause(self.active_generation);
                    self.status = format!("Reference playback failed: {error}");
                }
            }
            cx.notify();
        }
    }

    fn stop_playback(&mut self, cx: &mut Context<Self>) {
        if let Some(transport) = &self.transport
            && let Err(error) = transport.pause(self.active_generation)
        {
            self.status = error;
        }
        if let Some(transport) = &self.reference_transport
            && let Err(error) = transport.pause(self.active_generation)
        {
            self.status = format!("Reference stop failed: {error}");
        }
        cx.notify();
    }

    fn sync_gains(&self) {
        let reference_gain = if self.match_loudness {
            match (&self.selected_waveform, &self.reference_waveform) {
                (Some(main), Some(reference)) => {
                    audio::loudness_match_gain_db(main.integrated_lufs, reference.integrated_lufs)
                        .map(audio::linear_gain_for_db)
                        .unwrap_or(1.0)
                }
                _ => 1.0,
            }
        } else {
            1.0
        };
        if let Some(main) = &self.transport {
            main.set_output_gain(if self.audible_reference {
                0.0
            } else {
                self.volume
            });
        }
        if let Some(reference) = &self.reference_transport {
            reference.set_output_gain(if self.audible_reference {
                self.volume * reference_gain
            } else {
                0.0
            });
        }
    }

    fn toggle_source(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.reference_waveform.is_some() {
            self.audible_reference = !self.audible_reference;
            self.editing_note_id = None;
            self.original_note_body = None;
            self.note_draft_time = None;
            self.note_editor
                .update(cx, |editor, cx| editor.set_content("", cx));
            self.sync_gains();
            cx.notify();
        }
    }

    fn toggle_match(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.match_loudness = !self.match_loudness;
        self.sync_gains();
        cx.notify();
    }

    fn change_volume(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.volume = (self.volume + delta).clamp(0.0, 1.0);
        self.sync_gains();
        cx.notify();
    }

    fn capture_note(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_note(window, cx);
    }

    fn begin_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if (if self.audible_reference {
            &self.reference_waveform
        } else {
            &self.selected_waveform
        })
        .is_none()
        {
            return;
        }
        self.editing_note_id = None;
        self.original_note_body = None;
        let transport = if self.audible_reference {
            &self.reference_transport
        } else {
            &self.transport
        };
        self.note_draft_time = transport
            .as_ref()
            .map(|transport| transport.snapshot().position_millis);
        self.note_editor
            .update(cx, |editor, cx| editor.set_content("", cx));
        window.focus(&self.note_editor.read(cx).focus_handle());
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key.eq_ignore_ascii_case("escape") && !event.is_held {
            self.stop_playback(cx);
            cx.stop_propagation();
            return;
        }
        if self.note_editor.read(cx).focus_handle().is_focused(window)
            || self
                .reference_name_editor
                .read(cx)
                .focus_handle()
                .is_focused(window)
            || event.keystroke.modifiers.platform
            || event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
        {
            return;
        }
        match event.keystroke.key.as_str() {
            "n" if !event.is_held => {
                self.begin_note(window, cx);
                cx.stop_propagation();
            }
            "space" | " " if !event.is_held => {
                self.toggle_playback(cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn edit_note(&mut self, id: String, resume: bool, window: &mut Window, cx: &mut Context<Self>) {
        let notes = if self.audible_reference {
            self.selected_reference()
                .map(|reference| reference.notes.as_slice())
        } else {
            self.selected_track().map(|track| track.notes.as_slice())
        };
        let Some(note) = notes
            .and_then(|notes| notes.iter().find(|note| note.id == id))
            .cloned()
        else {
            return;
        };
        self.editing_note_id = Some(id);
        self.original_note_body = Some(note.body.clone());
        self.note_draft_time = Some(note.time_millis);
        self.note_editor
            .update(cx, |editor, cx| editor.set_content(note.body, cx));
        let transport = if self.audible_reference {
            &self.reference_transport
        } else {
            &self.transport
        };
        let waveform = if self.audible_reference {
            &self.reference_waveform
        } else {
            &self.selected_waveform
        };
        if let (Some(_), Some(waveform)) = (transport, waveform) {
            let ratio = note.time_millis as f32 / waveform.duration_millis.max(1) as f32;
            self.seek_ratio_with_resume(ratio, resume, cx);
        }
        window.focus(&self.note_editor.read(cx).focus_handle());
        cx.notify();
    }

    fn save_note(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.library_ready
            || self.busy
            || (if self.audible_reference {
                &self.reference_waveform
            } else {
                &self.selected_waveform
            })
            .is_none()
        {
            return;
        }
        let text = self.note_editor.read(cx).content().trim().to_string();
        let text = self
            .original_note_body
            .as_ref()
            .filter(|original| original.replace(['\n', '\r'], " ").trim() == text)
            .cloned()
            .unwrap_or(text);
        if text.is_empty() {
            self.status = String::from("Write a comment first.");
            cx.notify();
            return;
        }
        let notes = if self.audible_reference {
            let Some(path) = self
                .selected_track()
                .and_then(|track| track.reference_path.clone())
            else {
                return;
            };
            let Some(reference) = self
                .library
                .reference_tracks
                .find_mut(|reference| reference.path == path)
            else {
                return;
            };
            if reference.source_provenance().verified_proof().is_none() {
                self.status =
                    String::from("Reference comments require a verified source. Reimport it.");
                cx.notify();
                return;
            }
            &mut reference.notes
        } else {
            let Some(track_id) = self.library.selected_track_id.clone() else {
                return;
            };
            let Some(track) = self.library.tracks.find_mut(|track| track.id == track_id) else {
                return;
            };
            if track.source_provenance().verified_proof().is_none() {
                self.status =
                    String::from("Comments require a verified source. Reimport this track.");
                cx.notify();
                return;
            }
            &mut track.notes
        };
        if let Some(id) = self.editing_note_id.take() {
            if let Some(note) = notes.iter_mut().find(|note| note.id == id) {
                note.body = text;
            }
        } else {
            let id = storage::allocate_note_id(notes);
            let transport = if self.audible_reference {
                &self.reference_transport
            } else {
                &self.transport
            };
            let position = self
                .note_draft_time
                .take()
                .or_else(|| {
                    transport
                        .as_ref()
                        .map(|transport| transport.snapshot().position_millis)
                })
                .unwrap_or(0);
            notes.push(storage::Note {
                id,
                time_millis: position,
                body: text,
                done: false,
            });
        }
        self.note_editor
            .update(cx, |editor, cx| editor.set_content("", cx));
        self.original_note_body = None;
        self.save(cx);
    }

    fn delete_note(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy {
            return;
        }
        let notes = if self.audible_reference {
            let Some(path) = self
                .selected_track()
                .and_then(|track| track.reference_path.clone())
            else {
                return;
            };
            self.library
                .reference_tracks
                .find_mut(|reference| reference.path == path)
                .map(|reference| &mut reference.notes)
        } else {
            let Some(track_id) = self.library.selected_track_id.clone() else {
                return;
            };
            self.library
                .tracks
                .find_mut(|track| track.id == track_id)
                .map(|track| &mut track.notes)
        };
        if let Some(notes) = notes {
            notes.retain(|note| note.id != id);
            if self.editing_note_id.as_deref() == Some(id) {
                self.editing_note_id = None;
            }
            self.save(cx);
        }
    }

    fn move_note_to_playhead(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy {
            return;
        }
        let Some(id) = self.editing_note_id.clone() else {
            return;
        };
        let transport = if self.audible_reference {
            &self.reference_transport
        } else {
            &self.transport
        };
        let Some(position) = transport
            .as_ref()
            .map(|transport| transport.snapshot().position_millis)
        else {
            return;
        };
        if let Some(note) = self
            .selected_notes_mut()
            .and_then(|notes| notes.iter_mut().find(|note| note.id == id))
        {
            note.time_millis = position;
            self.note_draft_time = Some(position);
            self.save(cx);
        }
    }

    fn toggle_note_done(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy {
            return;
        }
        let Some(id) = self.editing_note_id.clone() else {
            return;
        };
        if let Some(note) = self
            .selected_notes_mut()
            .and_then(|notes| notes.iter_mut().find(|note| note.id == id))
        {
            note.done = !note.done;
            self.save(cx);
        }
    }

    fn choose_reference(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy || self.selected_track().is_none() {
            return;
        }
        self.show_reference_picker = false;
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose reference audio".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |app, cx| app.assign_reference(path, cx));
            }
        })
        .detach();
    }

    fn choose_main_replacement(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy || self.selected_track().is_none() {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Replace main audio".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |app, cx| app.replace_main(path, cx));
            }
        })
        .detach();
    }

    fn replace_main(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(id) = self.library.selected_track_id.clone() else {
            return;
        };
        let library = self.library.clone();
        self.busy = true;
        self.status = String::from("Replacing main audio…");
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    audio::decode_audio_file(&path)
                        .and_then(|decoded| storage::replace_track(library, &id, decoded))
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.busy = false;
                match result {
                    Ok(saved) => {
                        app.library = saved.value;
                        app.decode_selection(cx);
                    }
                    Err(error) => app.status = error,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_reference_replacement(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.library_ready || self.busy || self.selected_reference().is_none() {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Replace reference audio".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |app, cx| app.replace_reference(path, cx));
            }
        })
        .detach();
    }

    fn replace_reference(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(reference) = self.selected_reference() else {
            return;
        };
        let original_path = reference.path.clone();
        let expected_proof = reference.source_provenance().verified_proof().cloned();
        let library = self.library.clone();
        self.busy = true;
        self.status = String::from("Replacing reference audio…");
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    audio::decode_audio_file(&path).and_then(|decoded| {
                        storage::replace_reference_track(
                            library,
                            &original_path,
                            expected_proof.as_ref(),
                            decoded,
                        )
                    })
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.busy = false;
                match result {
                    Ok(saved) => {
                        app.library = saved.value;
                        app.decode_selection(cx);
                    }
                    Err(error) => app.status = error,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn rename_reference(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy {
            return;
        }
        let Some(path) = self
            .selected_reference()
            .map(|reference| reference.path.clone())
        else {
            return;
        };
        let name = self.reference_name_editor.read(cx).content().to_owned();
        match storage::rename_reference_track(&mut self.library, &path, &name) {
            Ok(_) => self.save(cx),
            Err(error) => {
                self.status = error;
                cx.notify();
            }
        }
    }

    fn select_reference(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy {
            return;
        }
        self.show_reference_picker = false;
        let Some(id) = self.library.selected_track_id.clone() else {
            return;
        };
        match storage::set_reference_track_selection(&mut self.library, &id, path) {
            Ok(_) => {
                self.save(cx);
                self.decode_selection(cx);
            }
            Err(error) => {
                self.status = error;
                cx.notify();
            }
        }
    }

    fn unassign_reference(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy {
            return;
        }
        self.show_reference_picker = false;
        let Some(id) = self.library.selected_track_id.clone() else {
            return;
        };
        if let Some(track) = self.library.tracks.find_mut(|track| track.id == id) {
            track.reference_path = None;
            self.reference_waveform = None;
            self.audible_reference = false;
            self.save(cx);
            self.decode_selection(cx);
        }
    }

    fn remove_reference(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy {
            return;
        }
        let Some(path) = self
            .selected_reference()
            .map(|reference| reference.path.clone())
        else {
            return;
        };
        if !self.confirm_remove_reference {
            self.confirm_remove_reference = true;
            cx.notify();
            return;
        }
        self.confirm_remove_reference = false;
        match storage::remove_reference_track(&mut self.library, &path) {
            Ok(_) => {
                self.audible_reference = false;
                self.save(cx);
                self.decode_selection(cx);
            }
            Err(error) => {
                self.status = error;
                cx.notify();
            }
        }
    }

    fn assign_reference(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(id) = self.library.selected_track_id.clone() else {
            return;
        };
        let library = self.library.clone();
        self.busy = true;
        self.status = String::from("Loading reference…");
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    audio::decode_audio_file(&path)
                        .and_then(|decoded| storage::set_reference_track(library, &id, decoded))
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.busy = false;
                match result {
                    Ok(saved) => {
                        app.library = saved.value;
                        app.decode_selection(cx);
                    }
                    Err(error) => app.status = error,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn waveform_ratio(&self, position: gpui::Point<Pixels>, reference: bool) -> Option<(f32, f32)> {
        let bounds_cell = if reference {
            &self.reference_bounds
        } else {
            &self.waveform_bounds
        };
        let bounds = bounds_cell.lock().ok().and_then(|bounds| *bounds)?;
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return None;
        }
        let ratio = ((f32::from(position.x) - f32::from(bounds.origin.x)) / width).clamp(0.0, 1.0);
        let y_ratio = ((f32::from(position.y) - f32::from(bounds.origin.y))
            / f32::from(bounds.size.height).max(1.0))
        .clamp(0.0, 1.0);
        Some((ratio, y_ratio))
    }

    fn waveform_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        reference: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((ratio, y_ratio)) = self.waveform_ratio(event.position, reference) else {
            return;
        };
        let Some(duration_millis) = (if reference {
            &self.reference_waveform
        } else {
            &self.selected_waveform
        })
        .as_ref()
        .map(|waveform| waveform.duration_millis) else {
            return;
        };
        let width = (if reference {
            &self.reference_bounds
        } else {
            &self.waveform_bounds
        })
        .lock()
        .ok()
        .and_then(|bounds| *bounds)
        .map(|bounds| f32::from(bounds.size.width))
        .unwrap_or(0.0);
        let notes = if reference {
            self.selected_reference()
                .map(|reference| reference.notes.as_slice())
        } else {
            self.selected_track().map(|track| track.notes.as_slice())
        };
        if (0.42..=0.68).contains(&y_ratio)
            && let Some(notes) = notes
            && let Some(note) = notes.iter().find(|note| {
                let note_ratio = note.time_millis as f32 / duration_millis.max(1) as f32;
                (note_ratio - ratio).abs() * width <= 7.0
            })
        {
            let id = note.id.clone();
            self.select_waveform_source(reference);
            self.hovered_note = Some((reference, id.clone()));
            self.hovered_draft = None;
            self.waveform_drag = Some(WaveformDrag::Note {
                reference,
                id,
                start_ratio: ratio,
                moved: false,
            });
            return;
        }
        if (0.42..=0.68).contains(&y_ratio)
            && self
                .draft_ratio(reference)
                .is_some_and(|draft| (draft - ratio).abs() * width <= 10.0)
        {
            self.select_waveform_source(reference);
            self.hovered_draft = Some(reference);
            self.hovered_note = None;
            self.waveform_drag = Some(WaveformDrag::Draft {
                reference,
                start_ratio: ratio,
                moved: false,
            });
            return;
        }
        self.select_waveform_source(reference);
        if y_ratio >= 0.5 {
            self.editing_note_id = None;
            self.original_note_body = None;
            self.note_draft_time = Some((duration_millis as f64 * ratio as f64).round() as u64);
            self.hovered_draft = Some(reference);
            self.hovered_note = None;
            self.waveform_drag = Some(WaveformDrag::Draft {
                reference,
                start_ratio: ratio,
                moved: false,
            });
            self.note_editor
                .update(cx, |editor, cx| editor.set_content("", cx));
            cx.notify();
            return;
        }
        self.waveform_drag = Some(WaveformDrag::Scrub {
            reference,
            start_ratio: ratio,
            moved: false,
        });
        self.seek_ratio_with_resume(ratio, true, cx);
    }

    fn select_waveform_source(&mut self, reference: bool) {
        if self.audible_reference != reference {
            self.audible_reference = reference;
            self.sync_gains();
        }
    }

    fn move_comment_node(&mut self, reference: bool, id: &str, ratio: f32) {
        let duration = (if reference {
            &self.reference_waveform
        } else {
            &self.selected_waveform
        })
        .as_ref()
        .map(|waveform| waveform.duration_millis);
        let Some(duration) = duration else { return };
        let position = (duration as f64 * ratio as f64).round() as u64;
        if let Some(note) = self
            .selected_notes_mut()
            .and_then(|notes| notes.iter_mut().find(|note| note.id == id))
        {
            note.time_millis = position;
            if self.editing_note_id.as_deref() == Some(id) {
                self.note_draft_time = Some(position);
            }
        }
    }

    fn waveform_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        match self.waveform_drag.clone() {
            Some(WaveformDrag::Scrub {
                reference,
                start_ratio,
                moved,
            }) => {
                if let Some((ratio, _)) = self.waveform_ratio(event.position, reference)
                    && (moved || (ratio - start_ratio).abs() >= 0.003)
                {
                    if let Some(WaveformDrag::Scrub { moved, .. }) = &mut self.waveform_drag {
                        *moved = true;
                    }
                    self.seek_ratio_with_resume(ratio, false, cx);
                }
            }
            Some(WaveformDrag::Note {
                reference,
                id,
                start_ratio,
                moved,
            }) => {
                if let Some((ratio, _)) = self.waveform_ratio(event.position, reference)
                    && (moved
                        || (ratio - start_ratio).abs() * self.waveform_width(reference) >= 3.0)
                {
                    self.move_comment_node(reference, &id, ratio);
                    if let Some(WaveformDrag::Note { moved, .. }) = &mut self.waveform_drag {
                        *moved = true;
                    }
                    cx.notify();
                }
            }
            Some(WaveformDrag::Draft {
                reference,
                start_ratio,
                moved,
            }) => {
                if let Some((ratio, _)) = self.waveform_ratio(event.position, reference)
                    && (moved
                        || (ratio - start_ratio).abs() * self.waveform_width(reference) >= 3.0)
                {
                    let duration = (if reference {
                        &self.reference_waveform
                    } else {
                        &self.selected_waveform
                    })
                    .as_ref()
                    .map(|waveform| waveform.duration_millis)
                    .unwrap_or(0);
                    self.note_draft_time = Some((duration as f64 * ratio as f64).round() as u64);
                    if let Some(WaveformDrag::Draft { moved, .. }) = &mut self.waveform_drag {
                        *moved = true;
                    }
                    cx.notify();
                }
            }
            None => {}
        }
    }

    fn draft_ratio(&self, reference: bool) -> Option<f32> {
        if self.audible_reference != reference || self.editing_note_id.is_some() {
            return None;
        }
        let duration = (if reference {
            &self.reference_waveform
        } else {
            &self.selected_waveform
        })
        .as_ref()?
        .duration_millis
        .max(1);
        self.note_draft_time
            .map(|time| time as f32 / duration as f32)
    }

    fn waveform_width(&self, reference: bool) -> f32 {
        (if reference {
            &self.reference_bounds
        } else {
            &self.waveform_bounds
        })
        .lock()
        .ok()
        .and_then(|bounds| *bounds)
        .map(|bounds| f32::from(bounds.size.width))
        .unwrap_or(0.0)
    }

    fn waveform_hover_move(
        &mut self,
        event: &MouseMoveEvent,
        reference: bool,
        cx: &mut Context<Self>,
    ) {
        let has_waveform = if reference {
            self.reference_waveform.is_some()
        } else {
            self.selected_waveform.is_some()
        };
        let next = has_waveform
            .then(|| self.waveform_ratio(event.position, reference))
            .flatten()
            .map(|(ratio, y_ratio)| {
                let width = self.waveform_width(reference).max(1.0);
                WaveformHover {
                    reference,
                    ratio: (ratio * width).round() / width,
                    comment: y_ratio >= 0.5,
                }
            });
        let next_note = match self.waveform_drag.as_ref() {
            Some(WaveformDrag::Note {
                reference: drag_reference,
                id,
                ..
            }) if *drag_reference == reference => Some((reference, id.clone())),
            _ => next.and_then(|hover| {
                let (_, y_ratio) = self.waveform_ratio(event.position, reference)?;
                if !(0.42..=0.68).contains(&y_ratio) {
                    return None;
                }
                let duration = (if reference {
                    &self.reference_waveform
                } else {
                    &self.selected_waveform
                })
                .as_ref()?
                .duration_millis
                .max(1);
                let notes = if reference {
                    self.selected_reference()
                        .map(|track| track.notes.as_slice())
                } else {
                    self.selected_track().map(|track| track.notes.as_slice())
                }?;
                let width = self.waveform_width(reference);
                notes
                    .iter()
                    .find(|note| {
                        let note_ratio = note.time_millis as f32 / duration as f32;
                        (note_ratio - hover.ratio).abs() * width <= 7.0
                    })
                    .map(|note| (reference, note.id.clone()))
            }),
        };
        let next_draft = match self.waveform_drag.as_ref() {
            Some(WaveformDrag::Draft {
                reference: drag_reference,
                ..
            }) if *drag_reference == reference => Some(reference),
            _ if next_note.is_none() => next.and_then(|hover| {
                let (_, y_ratio) = self.waveform_ratio(event.position, reference)?;
                ((0.42..=0.68).contains(&y_ratio)
                    && self.draft_ratio(reference).is_some_and(|draft| {
                        (draft - hover.ratio).abs() * self.waveform_width(reference) <= 10.0
                    }))
                .then_some(reference)
            }),
            _ => None,
        };
        if self.waveform_hover != next
            || self.hovered_note != next_note
            || self.hovered_draft != next_draft
        {
            self.waveform_hover = next;
            self.hovered_note = next_note;
            self.hovered_draft = next_draft;
            cx.notify();
        }
    }

    fn waveform_hover_changed(&mut self, hovered: &bool, reference: bool, cx: &mut Context<Self>) {
        if !hovered
            && self
                .waveform_hover
                .is_some_and(|hover| hover.reference == reference)
        {
            self.waveform_hover = None;
            if !matches!(self.waveform_drag, Some(WaveformDrag::Note { reference: drag_reference, .. }) if drag_reference == reference)
            {
                self.hovered_note = None;
            }
            if !matches!(self.waveform_drag, Some(WaveformDrag::Draft { reference: drag_reference, .. }) if drag_reference == reference)
            {
                self.hovered_draft = None;
            }
            cx.notify();
        }
    }

    fn waveform_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.waveform_drag.take() else {
            return;
        };
        match drag {
            WaveformDrag::Scrub {
                reference, moved, ..
            } => {
                if moved && let Some((ratio, _)) = self.waveform_ratio(event.position, reference) {
                    self.seek_ratio_with_resume(ratio, true, cx);
                }
            }
            WaveformDrag::Note {
                reference,
                id,
                moved,
                ..
            } => {
                if moved {
                    if let Some((ratio, _)) = self.waveform_ratio(event.position, reference) {
                        self.move_comment_node(reference, &id, ratio);
                    }
                    self.save(cx);
                } else {
                    self.edit_note(id, true, window, cx);
                }
            }
            WaveformDrag::Draft {
                reference, moved, ..
            } => {
                if moved && let Some((ratio, _)) = self.waveform_ratio(event.position, reference) {
                    let duration = (if reference {
                        &self.reference_waveform
                    } else {
                        &self.selected_waveform
                    })
                    .as_ref()
                    .map(|waveform| waveform.duration_millis)
                    .unwrap_or(0);
                    self.note_draft_time = Some((duration as f64 * ratio as f64).round() as u64);
                }
                window.focus(&self.note_editor.read(cx).focus_handle());
                cx.notify();
            }
        }
    }

    fn seek_ratio(&mut self, ratio: f32, cx: &mut Context<Self>) {
        let resume = self
            .transport
            .as_ref()
            .is_some_and(|transport| transport.snapshot().playing);
        self.seek_ratio_with_resume(ratio, resume, cx);
    }

    fn seek_ratio_with_resume(&mut self, ratio: f32, resume: bool, cx: &mut Context<Self>) {
        if let (Some(transport), Some(waveform)) = (&self.transport, &self.selected_waveform) {
            let position = (waveform.duration_millis as f64 * ratio as f64).round() as u64;
            if let Err(error) = transport.seek(
                self.active_generation,
                position,
                waveform.duration_millis,
                resume,
            ) {
                self.status = error;
            }
        }
        if let (Some(transport), Some(waveform)) =
            (&self.reference_transport, &self.reference_waveform)
        {
            let position = (waveform.duration_millis as f64 * ratio as f64).round() as u64;
            if let Err(error) = transport.seek(
                self.active_generation,
                position,
                waveform.duration_millis,
                resume,
            ) {
                self.status = format!("Reference seek failed: {error}");
            }
        }
        cx.notify();
    }

    fn waveform_view(&self, reference: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let waveform = if reference {
            self.reference_waveform.as_ref()
        } else {
            self.selected_waveform.as_ref()
        };
        let summary = waveform.map(|waveform| waveform.summary.clone());
        let markers: Vec<(String, f32)> = match (reference, waveform) {
            (false, Some(waveform)) => self
                .selected_track()
                .map(|track| {
                    track
                        .notes
                        .iter()
                        .map(|note| {
                            (
                                note.id.clone(),
                                note.time_millis as f32 / waveform.duration_millis.max(1) as f32,
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            (true, Some(waveform)) => self
                .selected_reference()
                .map(|track| {
                    track
                        .notes
                        .iter()
                        .map(|note| {
                            (
                                note.id.clone(),
                                note.time_millis as f32 / waveform.duration_millis.max(1) as f32,
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let loop_selection = (reference && self.loop_end > self.loop_start)
            .then_some((self.loop_start, self.loop_end));
        let hover_ratio = self
            .waveform_hover
            .filter(|hover| hover.reference == reference)
            .map(|hover| hover.ratio);
        let hovered_note_id = self
            .hovered_note
            .as_ref()
            .filter(|(source, _)| *source == reference)
            .map(|(_, id)| id.clone());
        let draft_ratio = self.draft_ratio(reference);
        let draft_highlighted = self.hovered_draft == Some(reference);
        let play_ratio = waveform
            .map(|waveform| {
                let transport = if reference {
                    &self.reference_transport
                } else {
                    &self.transport
                };
                let position = transport
                    .as_ref()
                    .map(|transport| transport.snapshot().position_millis)
                    .unwrap_or(0);
                position as f32 / waveform.duration_millis.max(1) as f32
            })
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let bounds_cell = if reference {
            self.reference_bounds.clone()
        } else {
            self.waveform_bounds.clone()
        };
        let paint_cache = if reference {
            self.reference_paint_cache.clone()
        } else {
            self.main_paint_cache.clone()
        };
        let generation = self.active_generation;
        let drawing = canvas(
            move |_, _, _| (),
            move |bounds, _, window, _| {
                if let Ok(mut cell) = bounds_cell.lock() {
                    *cell = Some(bounds);
                }
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                if width <= 0.0 || height <= 0.0 {
                    return;
                }
                let Some(summary) = &summary else {
                    return;
                };
                if let Some(path) = cached_waveform_path(&paint_cache, generation, summary, bounds)
                {
                    window.paint_path(path, gpui::rgba(0xd67972dd));
                }
                let mid = f32::from(bounds.origin.y) + height * 0.5;
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(bounds.origin.x, px(mid)),
                        point(bounds.right(), bounds.bottom()),
                    ),
                    gpui::rgba(0x0b0e0e99),
                ));
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(bounds.origin.x, px(mid)),
                        point(bounds.right(), px(mid + 1.0)),
                    ),
                    gpui::rgba(0xe5e5e199),
                ));
                if let Some((start, end)) = loop_selection {
                    let left = f32::from(bounds.origin.x) + width * start;
                    let right = f32::from(bounds.origin.x) + width * end;
                    window.paint_quad(fill(
                        Bounds::from_corners(
                            point(px(left), bounds.origin.y),
                            point(px(right), bounds.bottom()),
                        ),
                        gpui::rgba(0xff605744),
                    ));
                }
                if let Some(ratio) = hover_ratio {
                    let x = f32::from(bounds.origin.x) + width * ratio;
                    for y in (0..height.ceil() as usize).step_by(9) {
                        let top = f32::from(bounds.origin.y) + y as f32;
                        let bottom = (top + 5.0).min(f32::from(bounds.bottom()));
                        window.paint_quad(fill(
                            Bounds::from_corners(
                                point(px(x), px(top)),
                                point(px(x + 1.0), px(bottom)),
                            ),
                            gpui::rgba(0xe5e5e1bb),
                        ));
                    }
                    window.paint_quad(
                        fill(
                            Bounds::from_corners(
                                point(px(x - 4.0), px(mid - 4.0)),
                                point(px(x + 4.0), px(mid + 4.0)),
                            ),
                            gpui::rgba(TEXT),
                        )
                        .corner_radii(px(4.0)),
                    );
                }
                for (id, ratio) in &markers {
                    let x = f32::from(bounds.origin.x) + width * ratio.clamp(0.0, 1.0);
                    let highlighted = hovered_note_id.as_deref() == Some(id.as_str());
                    let outer = if highlighted { 9.0 } else { 6.0 };
                    let inner = if highlighted { 5.0 } else { 4.0 };
                    window.paint_quad(
                        fill(
                            Bounds::from_corners(
                                point(px(x - outer), px(mid - outer)),
                                point(px(x + outer), px(mid + outer)),
                            ),
                            gpui::rgba(if highlighted { CORAL } else { SURFACE_RAISED }),
                        )
                        .corner_radii(px(outer)),
                    );
                    window.paint_quad(
                        fill(
                            Bounds::from_corners(
                                point(px(x - inner), px(mid - inner)),
                                point(px(x + inner), px(mid + inner)),
                            ),
                            gpui::rgba(TEXT),
                        )
                        .corner_radii(px(inner)),
                    );
                }
                if let Some(ratio) = draft_ratio {
                    let x = f32::from(bounds.origin.x) + width * ratio.clamp(0.0, 1.0);
                    let radius = if draft_highlighted { 9.0 } else { 5.0 };
                    window.paint_quad(
                        fill(
                            Bounds::from_corners(
                                point(px(x - radius), px(mid - radius)),
                                point(px(x + radius), px(mid + radius)),
                            ),
                            gpui::rgba(CORAL),
                        )
                        .corner_radii(px(radius)),
                    );
                    if draft_highlighted {
                        window.paint_quad(
                            fill(
                                Bounds::from_corners(
                                    point(px(x - 5.0), px(mid - 5.0)),
                                    point(px(x + 5.0), px(mid + 5.0)),
                                ),
                                gpui::rgba(TEXT),
                            )
                            .corner_radii(px(5.0)),
                        );
                    }
                }
                let play_x = f32::from(bounds.origin.x) + width * play_ratio;
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(px(play_x), bounds.origin.y),
                        point(px(play_x + 2.0), bounds.bottom()),
                    ),
                    gpui::rgba(CORAL),
                ));
            },
        )
        .size_full();
        let id = if reference {
            "reference-waveform"
        } else {
            "main-waveform"
        };
        div()
            .id(id)
            .h(px(140.0))
            .flex_shrink_0()
            .w_full()
            .bg(gpui::rgba(SURFACE_RAISED))
            .border_1()
            .border_color(gpui::rgba(BORDER))
            .cursor_pointer()
            .on_mouse_move(cx.listener(move |app, event: &MouseMoveEvent, _, cx| {
                app.waveform_hover_move(event, reference, cx);
            }))
            .on_hover(cx.listener(move |app, hovered: &bool, _, cx| {
                app.waveform_hover_changed(hovered, reference, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |app, event: &MouseDownEvent, window, cx| {
                    app.waveform_mouse_down(event, reference, window, cx);
                }),
            )
            .child(drawing)
    }

    fn comments_view(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let source = if self.audible_reference {
            "REFERENCE"
        } else {
            "MAIN"
        };
        let mut notes: Vec<&storage::Note> = if self.audible_reference {
            self.selected_reference()
                .map(|reference| reference.notes.iter().collect())
                .unwrap_or_default()
        } else {
            self.selected_track()
                .map(|track| track.notes.iter().collect())
                .unwrap_or_default()
        };
        notes.sort_by_key(|note| note.time_millis);
        let position = self.note_draft_time.unwrap_or_else(|| {
            let transport = if self.audible_reference {
                &self.reference_transport
            } else {
                &self.transport
            };
            transport
                .as_ref()
                .map(|transport| transport.snapshot().position_millis)
                .unwrap_or(0)
        });
        let count = notes.len();
        let hover_time = self.waveform_hover.and_then(|hover| {
            let waveform = if hover.reference {
                &self.reference_waveform
            } else {
                &self.selected_waveform
            };
            waveform.as_ref().map(|waveform| {
                (
                    (waveform.duration_millis as f64 * hover.ratio as f64).round() as u64,
                    hover.comment,
                )
            })
        });
        let mut list = div()
            .id("comments-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(5.0));
        if notes.is_empty() {
            list = list.child(
                div()
                    .text_color(gpui::rgba(MUTED))
                    .child("No comments yet. Click below the waveform's center line to add one."),
            );
        }
        for note in notes {
            let edit_id = note.id.clone();
            let delete_id = note.id.clone();
            let label = format!(
                "{}   {}{}",
                format_timestamp(note.time_millis),
                if note.done { "✓  " } else { "" },
                note.body.replace(['\n', '\r'], " ")
            );
            list = list.child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(
                        self.button_with_id_active(
                            format!("comment-row-{edit_id}"),
                            label,
                            self.editing_note_id.as_deref() == Some(&edit_id),
                        )
                        .flex_1()
                        .justify_start()
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.edit_note(edit_id.clone(), false, window, cx)
                            },
                        )),
                    )
                    .child(
                        self.button_with_id(format!("delete-comment-{delete_id}"), "DELETE")
                            .on_click(
                                cx.listener(move |app, _, _, cx| app.delete_note(&delete_id, cx)),
                            ),
                    ),
            );
        }
        let mut comments = panel()
            .id("comments-panel")
            .h(px(170.0))
            .flex_shrink_0()
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(section_title("COMMENTS"))
                    .child(
                        div()
                            .ml(px(12.0))
                            .text_color(gpui::rgba(MUTED))
                            .child(format!("{count} · {source}")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .child(
                        div()
                            .text_color(gpui::rgba(MUTED))
                            .child(format!("AT {}", format_precise_timestamp(position))),
                    )
                    .when_some(hover_time, |row, (time, comment)| {
                        row.child(div().text_color(gpui::rgba(MUTED)).child(format!(
                            "{} {}",
                            if comment { "COMMENT" } else { "PLAY" },
                            format_precise_timestamp(time)
                        )))
                    })
                    .child(div().flex_1().child(self.note_editor.clone()))
                    .child(self.button("SAVE").on_click(cx.listener(Self::save_note)))
                    .child(self.button("NEW").on_click(cx.listener(Self::capture_note))),
            );
        if self.editing_note_id.is_some() {
            comments = comments.child(
                div()
                    .flex()
                    .gap(px(7.0))
                    .child(
                        self.button("MOVE TO PLAYHEAD")
                            .on_click(cx.listener(Self::move_note_to_playhead)),
                    )
                    .child(
                        self.button("TOGGLE DONE")
                            .on_click(cx.listener(Self::toggle_note_done)),
                    ),
            );
        }
        comments.child(list)
    }

    fn toggle_favorite(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy {
            return;
        }
        if let Some(track) = self.library.tracks.find_mut(|track| track.id == id) {
            track.favorite = !track.favorite;
            self.save(cx);
        }
    }

    fn advance_stage(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy {
            return;
        }
        if let Some(track) = self.library.tracks.iter().find(|track| track.id == id) {
            let next =
                storage::TrackStage::ALL[(track.stage.index() + 1) % storage::TrackStage::COUNT];
            if let Err(error) = storage::set_track_stage(&mut self.library, id, next) {
                self.status = error;
            } else {
                self.save(cx);
            }
        }
    }

    fn move_in_planner(
        &mut self,
        id: &str,
        stage: storage::TrackStage,
        slot: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.library_ready || self.busy {
            return;
        }
        match storage::move_track_to_planner_slot(&mut self.library, id, stage, slot) {
            Ok(_) => {
                self.planner_scroll.scroll_to_item(stage.index());
                self.save(cx);
            }
            Err(error) => {
                self.status = error;
                cx.notify();
            }
        }
    }

    fn scroll_planner(&mut self, direction: f32, cx: &mut Context<Self>) {
        let offset = self.planner_scroll.offset();
        let max = f32::from(self.planner_scroll.max_offset().width);
        let x = (f32::from(offset.x) + direction * 440.0).clamp(-max, 0.0);
        self.planner_scroll.set_offset(point(px(x), offset.y));
        cx.notify();
    }

    fn remove_selected(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.library_ready || self.busy {
            return;
        }
        if !self.confirm_remove {
            self.confirm_remove = true;
            cx.notify();
            return;
        }
        self.confirm_remove = false;
        let Some(id) = self.library.selected_track_id.clone() else {
            return;
        };
        match storage::remove_track(&mut self.library, &id) {
            Ok((index, _)) => {
                self.library.selected_track_id =
                    storage::selection_after_removal(&self.library, index);
                self.selected_waveform = None;
                self.save(cx);
                self.decode_selection(cx);
            }
            Err(error) => self.status = error,
        }
    }

    fn preserve_and_start_fresh(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        match storage::preserve_unreadable_library_and_start_fresh() {
            Ok(saved) => {
                self.library = storage::Library::default();
                self.library_ready = true;
                self.status = format!("Previous library preserved at {}", saved.value.display());
            }
            Err(error) => self.status = error,
        }
        cx.notify();
    }

    fn button_with_id(
        &self,
        id: impl Into<gpui::SharedString>,
        label: impl Into<gpui::SharedString>,
    ) -> gpui::Stateful<gpui::Div> {
        self.button_with_id_active(id, label, false)
    }

    fn button_with_id_active(
        &self,
        id: impl Into<gpui::SharedString>,
        label: impl Into<gpui::SharedString>,
        active: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let label = label.into();
        div()
            .id(id.into())
            .min_h(px(31.0))
            .px(px(10.0))
            .py(px(6.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(if active { CORAL_DARK } else { SURFACE_RAISED }))
            .border_1()
            .border_color(gpui::rgba(if active { CORAL } else { BORDER }))
            .text_color(gpui::rgba(if active { CORAL } else { TEXT }))
            .text_size(px(12.0))
            .cursor_pointer()
            .child(label)
    }

    fn button(&self, label: impl Into<gpui::SharedString>) -> gpui::Stateful<gpui::Div> {
        let label = label.into();
        self.button_with_id(label.clone(), label)
    }
}

impl Render for Cadence {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_track().cloned();
        let mut header = div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(48.0))
            .flex_shrink_0()
            .child(
                div()
                    .text_size(px(34.0))
                    .text_color(gpui::rgba(CORAL))
                    .child("cadence"),
            )
            .child(
                div()
                    .ml(px(8.0))
                    .text_size(px(10.0))
                    .text_color(gpui::rgba(MUTED))
                    .child("TRACK REVIEW"),
            )
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .gap_2()
                    .child(
                        self.button_with_id_active(
                            "tab-review",
                            "REVIEW",
                            self.page == Page::Review,
                        )
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.page = Page::Review;
                            app.waveform_hover = None;
                            app.show_reference_picker = false;
                            cx.notify();
                        })),
                    )
                    .child(
                        self.button_with_id_active(
                            "tab-planner",
                            "PLANNER",
                            self.page == Page::Planner,
                        )
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.page = Page::Planner;
                            app.waveform_hover = None;
                            app.show_reference_picker = false;
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .mx(px(12.0))
                    .h(px(30.0))
                    .w(px(1.0))
                    .bg(gpui::rgba(BORDER)),
            )
            .child(
                self.button_with_id("import-audio", "+ IMPORT AUDIO")
                    .on_click(cx.listener(Self::choose_import)),
            );
        if !self.library_ready {
            header = header.child(
                self.button("Preserve library and start fresh")
                    .on_click(cx.listener(Self::preserve_and_start_fresh)),
            );
        }
        let mut toolbar = panel()
            .flex()
            .items_center()
            .gap_2()
            .p(px(9.0))
            .flex_shrink_0();
        if self.page == Page::Review {
            let playing = self
                .transport
                .as_ref()
                .is_some_and(|transport| transport.snapshot().playing);
            toolbar = toolbar.child(section_title("AUDITION"));
            toolbar = toolbar.child(
                self.button_with_id_active(
                    "transport-play",
                    if playing { "Ⅱ  PAUSE" } else { "▶  PLAY" },
                    playing,
                )
                .on_click(cx.listener(Self::toggle_play)),
            );
            toolbar = toolbar.child(
                self.button("−")
                    .on_click(cx.listener(|app, _, _, cx| app.change_volume(-0.1, cx))),
            );
            toolbar = toolbar.child(
                div()
                    .text_color(gpui::rgba(MUTED))
                    .child(format!("VOL {:.0}%", self.volume * 100.0)),
            );
            toolbar = toolbar.child(
                self.button("+")
                    .on_click(cx.listener(|app, _, _, cx| app.change_volume(0.1, cx))),
            );
            toolbar = toolbar.child(
                self.button_with_id_active(
                    "source-toggle",
                    if self.audible_reference {
                        "REFERENCE"
                    } else {
                        "MAIN"
                    },
                    self.audible_reference,
                )
                .on_click(cx.listener(Self::toggle_source)),
            );
            let selected_name = self
                .selected_reference()
                .map(reference_display_name)
                .unwrap_or_else(|| String::from("NONE"));
            let short_name: String = selected_name.chars().take(18).collect();
            let suffix = if short_name.chars().count() < selected_name.chars().count() {
                "…"
            } else {
                ""
            };
            toolbar = toolbar.child(
                self.button_with_id_active(
                    "reference-picker-toggle",
                    format!("REF: {short_name}{suffix} ▾"),
                    self.show_reference_picker,
                )
                .on_click(cx.listener(|app, _, _, cx| {
                    if app.library_ready && !app.busy && app.selected_track().is_some() {
                        app.show_reference_picker = !app.show_reference_picker;
                        app.waveform_hover = None;
                        cx.notify();
                    }
                })),
            );
            toolbar = toolbar.child(
                self.button_with_id_active(
                    "match-toggle",
                    if self.match_loudness {
                        "MATCH ON"
                    } else {
                        "MATCH OFF"
                    },
                    self.match_loudness,
                )
                .on_click(cx.listener(Self::toggle_match)),
            );
            toolbar = toolbar.child(
                self.button_with_id_active(
                    "loop-toggle",
                    if self.loop_enabled {
                        "LOOP ON"
                    } else {
                        "LOOP OFF"
                    },
                    self.loop_enabled,
                )
                .on_click(cx.listener(Self::toggle_loop)),
            );
            toolbar = toolbar.child(
                self.button("SET A")
                    .on_click(cx.listener(|app, _, _, cx| app.set_loop_point(true, cx))),
            );
            toolbar = toolbar.child(
                self.button("SET B")
                    .on_click(cx.listener(|app, _, _, cx| app.set_loop_point(false, cx))),
            );
        } else {
            toolbar = toolbar.child(section_title("PLANNER"));
            toolbar = toolbar.child(
                div()
                    .text_color(gpui::rgba(MUTED))
                    .child("SCROLL TO VIEW ALL STAGES"),
            );
            toolbar = toolbar.child(
                self.button_with_id("planner-scroll-left", "← EARLIER")
                    .on_click(cx.listener(|app, _, _, cx| app.scroll_planner(1.0, cx))),
            );
            toolbar = toolbar.child(
                self.button_with_id("planner-scroll-right", "LATER →")
                    .on_click(cx.listener(|app, _, _, cx| app.scroll_planner(-1.0, cx))),
            );
        }

        let mut reference_menu = panel()
            .id("reference-picker-menu")
            .absolute()
            .top(px(120.0))
            .left(px(390.0))
            .w(px(330.0))
            .max_h(px(360.0))
            .overflow_y_scroll()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_move(cx.listener(|app, _, _, cx| {
                if app.waveform_hover.take().is_some() {
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(7.0))
            .child(section_title("REFERENCE TRACKS"))
            .child(
                self.button_with_id_active(
                    "reference-option-none",
                    "○  NO REFERENCE",
                    self.selected_track()
                        .is_some_and(|track| track.reference_path.is_none()),
                )
                .justify_start()
                .on_click(cx.listener(Self::unassign_reference)),
            )
            .child(
                self.button_with_id("reference-option-import", "+  IMPORT REFERENCE…")
                    .justify_start()
                    .on_click(cx.listener(Self::choose_reference)),
            );
        if self.show_reference_picker && self.page == Page::Review {
            let selected_reference_path = self
                .selected_track()
                .and_then(|track| track.reference_path.as_ref());
            for reference in &self.library.reference_tracks {
                let path = reference.path.clone();
                let selected = selected_reference_path == Some(&path);
                let name = reference_display_name(reference);
                reference_menu = reference_menu.child(
                    self.button_with_id_active(
                        format!("reference-option-{}", path.display()),
                        format!("{}  {name}", if selected { "●" } else { "○" }),
                        selected,
                    )
                    .justify_start()
                    .on_click(
                        cx.listener(move |app, _, _, cx| app.select_reference(path.clone(), cx)),
                    ),
                );
            }
            reference_menu = reference_menu.child(
                self.button_with_id("reference-option-close", "CLOSE")
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.show_reference_picker = false;
                        cx.notify();
                    })),
            );
        } else {
            reference_menu = reference_menu.hidden();
        }

        let mut sidebar = panel()
            .id("sidebar")
            .w(px(235.0))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(12.0))
            .child(section_title("LIBRARY"))
            .child(
                div()
                    .mb(px(6.0))
                    .text_size(px(10.0))
                    .text_color(gpui::rgba(MUTED))
                    .child(format!("{} TRACKS", self.library.tracks.len())),
            );
        for track in self.library.tracks.iter() {
            let id = track.id.clone();
            let label = format!(
                "{}  {}",
                if track.favorite { "★" } else { "☆" },
                track.title
            );
            sidebar = sidebar.child(
                self.button_with_id_active(
                    format!("library-track-{id}"),
                    label,
                    self.library.selected_track_id.as_deref() == Some(&id),
                )
                .justify_start()
                .on_click(cx.listener(move |app, _, _, cx| app.select(id.clone(), cx))),
            );
        }

        let content = match self.page {
            Page::Review => {
                let mut body = panel()
                    .id("review-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .p(px(12.0));
                if let Some(track) = selected {
                    let id_favorite = track.id.clone();
                    let id_stage = track.id.clone();
                    body = body
                        .child(section_title("TRACK REVIEW"))
                        .child(
                            div()
                                .text_size(px(23.0))
                                .text_color(gpui::rgba(TEXT))
                                .child(track.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(gpui::rgba(MUTED))
                                .child(format!(
                                    "{} · {}",
                                    track.stage.label(),
                                    track.path.display()
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    self.button(if track.favorite {
                                        "Unfavorite"
                                    } else {
                                        "Favorite"
                                    })
                                    .on_click(cx.listener(
                                        move |app, _, _, cx| app.toggle_favorite(&id_favorite, cx),
                                    )),
                                )
                                .child(self.button("Next stage").on_click(cx.listener(
                                    move |app, _, _, cx| app.advance_stage(&id_stage, cx),
                                )))
                                .child(
                                    self.button("Replace main…")
                                        .on_click(cx.listener(Self::choose_main_replacement)),
                                )
                                .child(
                                    self.button(if self.confirm_remove {
                                        "Confirm removal"
                                    } else {
                                        "Remove from library"
                                    })
                                    .on_click(cx.listener(Self::remove_selected)),
                                ),
                        );
                    if let Some(waveform) = &self.selected_waveform {
                        let snapshot = self
                            .transport
                            .as_ref()
                            .map(|transport| transport.snapshot())
                            .unwrap_or_default();
                        body = body.child(div().text_color(gpui::rgba(MUTED)).child(format!(
                                "{} / {} ms · LUFS {}",
                                snapshot.position_millis,
                                waveform.duration_millis,
                                waveform
                                    .integrated_lufs
                                    .map(|value| format!("{value:.1}"))
                                    .unwrap_or_else(|| "—".into())
                            )));
                    }
                    body = body.child(section_title("MAIN AUDIO"));
                    body = body.child(self.waveform_view(false, cx));
                    if let Some(reference) = &self.reference_waveform {
                        body = body.child(section_title("REFERENCE AUDIO"));
                        body = body.child(div().text_color(gpui::rgba(MUTED)).child(format!(
                                "Reference: {} ms · LUFS {}",
                                reference.duration_millis,
                                reference
                                    .integrated_lufs
                                    .map(|value| format!("{value:.1}"))
                                    .unwrap_or_else(|| "—".into())
                            )));
                        body = body.child(self.waveform_view(true, cx));
                    }
                    if self.selected_reference().is_some() {
                        body = body.child(
                            div()
                                .flex()
                                .gap_2()
                                .child(self.reference_name_editor.clone())
                                .child(
                                    self.button("Rename reference")
                                        .on_click(cx.listener(Self::rename_reference)),
                                )
                                .child(
                                    self.button("Replace reference…")
                                        .on_click(cx.listener(Self::choose_reference_replacement)),
                                )
                                .child(
                                    self.button("Unassign")
                                        .on_click(cx.listener(Self::unassign_reference)),
                                )
                                .child(
                                    self.button(if self.confirm_remove_reference {
                                        "Confirm delete reference"
                                    } else {
                                        "Delete reference"
                                    })
                                    .on_click(cx.listener(Self::remove_reference)),
                                ),
                        );
                    }
                } else {
                    body = body.child(section_title("TRACK REVIEW"));
                    body = body.child(
                        div()
                            .text_color(gpui::rgba(MUTED))
                            .child("Import audio to start reviewing."),
                    );
                }
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(body)
                    .child(self.comments_view(cx))
                    .into_any_element()
            }
            Page::Planner => {
                let ordered = storage::planner_tracks(&self.library);
                let mut stage_counts = [0usize; storage::TrackStage::COUNT];
                for track in &ordered {
                    stage_counts[track.stage.index()] += 1;
                }
                let mut board = div()
                    .id("planner-board")
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .gap(px(12.0))
                    .track_scroll(&self.planner_scroll)
                    .overflow_x_scroll();
                for stage in storage::TrackStage::ALL {
                    let mut column = panel()
                        .id(gpui::SharedString::from(format!(
                            "planner-column-{}",
                            stage.index()
                        )))
                        .w(px(225.0))
                        .h_full()
                        .overflow_y_scroll()
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .p(px(11.0))
                        .child(section_title(stage.label()))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(gpui::rgba(MUTED))
                                .child(format!("{} TRACKS", stage_counts[stage.index()])),
                        );
                    let cards: Vec<_> = ordered
                        .iter()
                        .copied()
                        .filter(|track| track.stage == stage)
                        .collect();
                    let count = cards.len();
                    for (index, track) in cards.into_iter().enumerate() {
                        let id = track.id.clone();
                        let card = self
                            .button_with_id(
                                format!("planner-card-{id}"),
                                format!(
                                    "{}  {} · {} notes",
                                    if track.favorite { "★" } else { "☆" },
                                    track.title,
                                    track.notes.len(),
                                ),
                            )
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.page = Page::Review;
                                app.select(id.clone(), cx);
                            }));
                        let mut controls = div().flex().gap_1();
                        if index > 0 {
                            let id = track.id.clone();
                            controls = controls.child(
                                self.button_with_id(format!("planner-up-{id}"), "↑")
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.move_in_planner(&id, stage, index - 1, cx)
                                    })),
                            );
                        }
                        if index + 1 < count {
                            let id = track.id.clone();
                            controls = controls.child(
                                self.button_with_id(format!("planner-down-{id}"), "↓")
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.move_in_planner(&id, stage, index + 2, cx)
                                    })),
                            );
                        }
                        if stage.index() > 0 {
                            let id = track.id.clone();
                            let target = storage::TrackStage::ALL[stage.index() - 1];
                            let slot = stage_counts[target.index()];
                            controls = controls.child(
                                self.button_with_id(format!("planner-left-{id}"), "←")
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.move_in_planner(&id, target, slot, cx)
                                    })),
                            );
                        }
                        if stage.index() + 1 < storage::TrackStage::COUNT {
                            let id = track.id.clone();
                            let target = storage::TrackStage::ALL[stage.index() + 1];
                            let slot = stage_counts[target.index()];
                            controls = controls.child(
                                self.button_with_id(format!("planner-right-{id}"), "→")
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.move_in_planner(&id, target, slot, cx)
                                    })),
                            );
                        }
                        column = column.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(card.justify_start())
                                .child(controls),
                        );
                    }
                    board = board.child(column);
                }
                board.into_any_element()
            }
        };
        let drag_move = cx.listener(Self::waveform_mouse_move);
        let drag_up = cx.listener(Self::waveform_mouse_up);
        let drag_events = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        drag_move(event, window, cx);
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Capture && event.button == MouseButton::Left {
                        drag_up(event, window, cx);
                    }
                });
            },
        )
        .absolute()
        .size_full();
        div()
            .id("cadence-root")
            .relative()
            .track_focus(&self.focus_handle)
            .capture_key_down(cx.listener(Self::handle_key_down))
            .size_full()
            .flex()
            .flex_col()
            .gap(px(11.0))
            .p(px(16.0))
            .font_family("Menlo")
            .text_size(px(12.0))
            .bg(gpui::rgba(BACKGROUND))
            .text_color(gpui::rgba(TEXT))
            .on_drop(cx.listener(|app, paths: &ExternalPaths, _, cx| {
                if app.library_ready && !app.busy {
                    app.import_paths(paths.paths().to_vec(), cx);
                }
            }))
            .child(header)
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .gap(px(11.0))
                    .child(sidebar.flex_shrink_0())
                    .child(content),
            )
            .child(
                div()
                    .h(px(16.0))
                    .flex_shrink_0()
                    .text_size(px(10.0))
                    .text_color(gpui::rgba(MUTED))
                    .child(self.status.clone()),
            )
            .child(reference_menu)
            .child(drag_events)
    }
}

impl Focusable for Cadence {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn main() {
    let _instance_lock = match storage::acquire_instance_lock() {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("Could not start Cadence: {error}");
            return;
        }
    };
    Application::new().run(|cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        gpui_text_input::install_keybindings(cx);
        let bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(900.0), px(560.0))),
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title("Cadence — local track review");
                let view = cx.new(Cadence::new);
                window.focus(&view.read(cx).focus_handle(cx));
                view
            },
        )
        .expect("open Cadence GPUI window");
        cx.activate(true);
    });
}

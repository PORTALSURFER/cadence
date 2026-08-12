mod audio;
mod chrome;
mod storage;
mod transport;
mod waveform;

use radiant::{
    application::{AnchoredPopoverParts, anchored_popover_from_parts},
    gui::types::{Point, Rect, Vector2},
    layout::LayoutOutput,
    prelude as ui,
    runtime::{
        FileDialogRequest, NativeRunOptions, PaintFillPolygon, PaintFillRect, PaintPrimitive,
        PaintStrokePolygon, PlatformResponse, PlatformResult,
    },
    theme::ThemeTokens,
    widgets::{Widget, WidgetCommon, WidgetInput, WidgetOutput},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq)]
enum Message {
    ImportPressed,
    FilePicked(PlatformResult),
    ReplacePressed(String),
    ReplaceFilePicked {
        track_id: String,
        result: PlatformResult,
    },
    ReferencePressed(String),
    ReferenceFilesPicked {
        track_id: String,
        paths: Vec<PathBuf>,
    },
    FileDropped(ui::NativeFileDrop),
    LibraryLoaded(Result<storage::Library, String>),
    ImportCompleted(Result<storage::Library, String>),
    ReplaceCompleted {
        track_id: String,
        result: Result<storage::Library, String>,
    },
    ReferenceImportCompleted {
        track_id: String,
        path: PathBuf,
        result: Result<storage::Library, String>,
    },
    LibrarySaved(Result<(), String>),
    DecodeCompleted {
        track_id: String,
        generation: u64,
        result: Result<audio::WaveformData, String>,
    },
    DecodeProgress {
        track_id: String,
        generation: u64,
        progress: audio::WaveformProgress,
    },
    ReferenceDecodeCompleted {
        track_id: String,
        generation: u64,
        result: Result<audio::WaveformData, String>,
    },
    ReferenceDecodeProgress {
        track_id: String,
        generation: u64,
        progress: audio::WaveformProgress,
    },
    SelectTrack(String),
    SelectWorkspace(WorkspaceMode),
    SetAuditionFilter(storage::TrackStatus),
    SetReviewStatusFilter(Option<storage::TrackStatus>),
    SetPlannerStatusFilter(Option<storage::TrackStatus>),
    ShuffleAudition,
    ToggleFavorite(String),
    ToggleStageMenu(String),
    ToggleStageMenuAt {
        track_id: String,
        position: Point,
    },
    ToggleStatusMenuAt {
        track_id: String,
        host: StatusMenuHost,
    },
    ToggleReferenceMenu(String),
    ToggleReferenceMenuAt {
        track_id: String,
        position: Point,
    },
    SetReferenceTrack {
        track_id: String,
        path: PathBuf,
    },
    SetStage {
        track_id: String,
        stage: storage::TrackStage,
    },
    SetStatus {
        track_id: String,
        status: storage::TrackStatus,
    },
    PlannerCardDrag {
        track_id: String,
        message: ui::DragHandleMessage,
    },
    PlannerCardHandleActivated(String),
    PlannerStageHovered(storage::TrackStage),
    PlannerStageHoverCleared(storage::TrackStage),
    PlannerStageDropped(storage::TrackStage),
    RequestRemoveTrack(String),
    ConfirmRemoveTrack(String),
    CancelRemoveTrack,
    TogglePlayback,
    StopPlayback,
    NewNoteAtCurrentTime,
    AuditionPlay,
    AuditionPrevious,
    AuditionNext,
    SelectAuditionSource(AuditionSource),
    SelectCommentSource(CommentSource),
    ToggleReviewFilterMenu,
    ToggleReferenceMatch,
    AuditionVolumeChanged(f32),
    Frame,
    WaveformLoopDragStarted {
        ratio: f32,
    },
    WaveformLoopDragMoved {
        ratio: f32,
    },
    WaveformLoopDragEnded {
        start_ratio: f32,
        end_ratio: f32,
    },
    WaveformLoopDragCancelled,
    ReferenceLoopDragStarted {
        ratio: f32,
    },
    ReferenceLoopDragMoved {
        ratio: f32,
    },
    ReferenceLoopDragEnded {
        start_ratio: f32,
        end_ratio: f32,
    },
    ReferenceLoopDragCancelled,
    ReferenceWaveformClicked {
        ratio: f32,
    },
    ReferenceCommentClicked {
        ratio: f32,
    },
    WaveformClicked {
        ratio: f32,
        lower: bool,
    },
    WaveformPlayheadDragStarted {
        ratio: f32,
    },
    WaveformPlayheadDragMoved {
        ratio: f32,
    },
    WaveformPlayheadDragEnded {
        ratio: f32,
    },
    WaveformPlayheadDragCancelled,
    ReferencePlayheadDragStarted {
        ratio: f32,
    },
    ReferencePlayheadDragMoved {
        ratio: f32,
    },
    ReferencePlayheadDragEnded {
        ratio: f32,
    },
    ReferencePlayheadDragCancelled,
    CommentDragStarted {
        ratio: f32,
        note_index: Option<usize>,
    },
    CommentDragMoved {
        ratio: f32,
    },
    CommentDragEnded {
        ratio: f32,
    },
    CommentDragCancelled,
    ReferenceCommentDragStarted {
        ratio: f32,
        note_index: Option<usize>,
    },
    ReferenceCommentDragMoved {
        ratio: f32,
    },
    ReferenceCommentDragEnded {
        ratio: f32,
    },
    ReferenceCommentDragCancelled,
    DraftNoteChanged(String),
    SaveDraftNote,
    CancelDraftNote,
    SelectNote(String),
    CommentHoverStarted(String),
    CommentHoverEnded(String),
    ReferenceCommentHoverStarted(String),
    ReferenceCommentHoverEnded(String),
    EditNote(String),
    ToggleNoteDone(String),
    DeleteNote(String),
    ReferenceDraftNoteChanged(String),
    SaveReferenceDraftNote,
    CancelReferenceDraftNote,
    SelectReferenceNote(String),
    EditReferenceNote(String),
    FocusCommentEditor(u64),
    ToggleReferenceNoteDone(String),
    DeleteReferenceNote(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceMode {
    Review,
    Planner,
    Audition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusMenuHost {
    Library,
    Planner,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AuditionSource {
    #[default]
    Main,
    Reference,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CommentSource {
    #[default]
    Main,
    Reference,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LoopSelection {
    start_ratio: f32,
    end_ratio: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct LoopSelections {
    main: Option<LoopSelection>,
    reference: Option<LoopSelection>,
}

impl LoopSelections {
    fn get(self, source: AuditionSource) -> Option<LoopSelection> {
        match source {
            AuditionSource::Main => self.main,
            AuditionSource::Reference => self.reference,
        }
    }

    fn set(&mut self, source: AuditionSource, selection: Option<LoopSelection>) {
        match source {
            AuditionSource::Main => self.main = selection,
            AuditionSource::Reference => self.reference = selection,
        }
    }

    fn clear(&mut self, source: AuditionSource) {
        self.set(source, None);
    }

    fn clear_all(&mut self) {
        self.main = None;
        self.reference = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LoopBounds {
    start_millis: u64,
    end_millis: u64,
}

const LIBRARY_WIDTH: f32 = 252.0;
// Keep workspace tabs clear of the native macOS traffic-light controls in the
// integrated titlebar while leaving the right-side controls right-anchored.
const TITLEBAR_TRAFFIC_LIGHT_SAFE_GUTTER: f32 = 72.0;
// Keep the review rails compact so the comments surface owns the majority of
// the review panel while retaining enough height for the split waveform.
const WAVEFORM_HEIGHT: f32 = 124.0;
const REFERENCE_WAVEFORM_HEIGHT: f32 = WAVEFORM_HEIGHT;
const REFERENCE_HEADER_HEIGHT: f32 = 26.0;
const REFERENCE_SECTION_SPACING: f32 = 4.0;
const WAVEFORM_SECTION_SPACING: f32 = 8.0;
const AUDITION_SOURCE_SELECTOR_WIDTH: f32 = 28.0;
const FAVORITE_CONTROL_WIDTH: f32 = 28.0;
const MIN_LOOP_MILLIS: u64 = 120;
const MAIN_COMMENT_EDITOR_ID: u64 = 0xCAD3_1001;
const REFERENCE_COMMENT_EDITOR_ID: u64 = 0xCAD3_1002;
const MAIN_INLINE_COMMENT_EDITOR_SCOPE: u64 = 0xCAD3_1003;
const REFERENCE_INLINE_COMMENT_EDITOR_SCOPE: u64 = 0xCAD3_1004;
const TRACK_CARD_CHAMFER: f32 = 8.0;
const TRACK_CARD_RAIL_WIDTH: f32 = 4.0;
const TRACK_CARD_RAIL_EDGE_INSET: f32 = 1.0;
const TRACK_CARD_RAIL_VERTICAL_INSET: f32 = 3.0;
const TRACK_CARD_OUTLINE_WIDTH: f32 = 1.5;
const TRACK_CARD_CONTENT_INSET: f32 = 12.0;
const TRACK_CARD_CONTENT_SPACING: f32 = 3.0;
const LIBRARY_LIST_INSET: f32 = 6.0;
const LIBRARY_CARD_SPACING: f32 = 8.0;
const STATUS_RAIL_WIDTH: f32 = 4.0;
const STATUS_RAIL_GAP: f32 = 4.0;
const TRACK_CARD_SELECTED_CORAL: ui::Rgba8 = ui::Rgba8::new(233, 88, 67, 255);

#[derive(Clone, Debug)]
struct TrackCardChromeWidget {
    common: WidgetCommon,
    selected: bool,
}

impl TrackCardChromeWidget {
    fn new(selected: bool) -> Self {
        Self {
            common: WidgetCommon::fixed(0, 1.0, 1.0).without_default_chrome(),
            selected,
        }
    }
}

impl Widget for TrackCardChromeWidget {
    fn common(&self) -> &WidgetCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut WidgetCommon {
        &mut self.common
    }

    fn accepts_pointer_move(&self) -> bool {
        false
    }

    fn accepts_pointer_input(&self, _input: &WidgetInput) -> bool {
        false
    }

    fn handle_input(&mut self, _bounds: Rect, _input: WidgetInput) -> Option<WidgetOutput> {
        None
    }

    fn append_paint(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        bounds: Rect,
        _layout: &LayoutOutput,
        theme: &ThemeTokens,
    ) {
        if !bounds.has_finite_positive_area() {
            return;
        }

        let points = track_card_points(bounds);
        primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
            widget_id: self.common.id,
            points: Arc::clone(&points),
            color: theme.bg_primary,
        }));
        primitives.push(PaintPrimitive::StrokePolygon(PaintStrokePolygon {
            widget_id: self.common.id,
            points,
            color: if self.selected {
                TRACK_CARD_SELECTED_CORAL
            } else {
                theme.grid_strong
            },
            width: TRACK_CARD_OUTLINE_WIDTH,
        }));
        let rail_vertical_inset = TRACK_CARD_RAIL_VERTICAL_INSET.min(bounds.height() * 0.5);
        let rail_edge_inset = TRACK_CARD_RAIL_EDGE_INSET.min(bounds.width());
        let rail_width = TRACK_CARD_RAIL_WIDTH.min((bounds.width() - rail_edge_inset).max(0.0));
        let rail = Rect::from_min_max(
            Point::new(
                bounds.min.x + rail_edge_inset,
                bounds.min.y + rail_vertical_inset,
            ),
            Point::new(
                bounds.min.x + rail_edge_inset + rail_width,
                bounds.max.y - rail_vertical_inset,
            ),
        );
        if rail.has_finite_positive_area() {
            primitives.push(PaintPrimitive::FillRect(PaintFillRect {
                widget_id: self.common.id,
                rect: rail,
                color: if self.selected {
                    TRACK_CARD_SELECTED_CORAL
                } else {
                    theme.grid_strong
                },
            }));
        }
    }
}

fn track_card_points(bounds: Rect) -> Arc<[Point]> {
    let chamfer = TRACK_CARD_CHAMFER.min(bounds.width().min(bounds.height()) * 0.5);
    [
        Point::new(bounds.min.x, bounds.min.y),
        Point::new(bounds.max.x, bounds.min.y),
        Point::new(bounds.max.x, bounds.max.y - chamfer),
        Point::new(bounds.max.x - chamfer, bounds.max.y),
        Point::new(bounds.min.x, bounds.max.y),
    ]
    .into()
}

fn track_card_chrome(selected: bool) -> ui::View<Message> {
    ui::custom_widget(TrackCardChromeWidget::new(selected), |_| None).fill()
}

#[derive(Clone, Debug)]
struct StatusDropdownRailWidget {
    common: WidgetCommon,
    status: Option<storage::TrackStatus>,
}

impl StatusDropdownRailWidget {
    fn new(status: Option<storage::TrackStatus>) -> Self {
        Self {
            common: WidgetCommon::fixed(0, STATUS_RAIL_WIDTH, 1.0).without_default_chrome(),
            status,
        }
    }
}

impl Widget for StatusDropdownRailWidget {
    fn common(&self) -> &WidgetCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut WidgetCommon {
        &mut self.common
    }

    fn accepts_pointer_move(&self) -> bool {
        false
    }

    fn accepts_pointer_input(&self, _input: &WidgetInput) -> bool {
        false
    }

    fn handle_input(&mut self, _bounds: Rect, _input: WidgetInput) -> Option<WidgetOutput> {
        None
    }

    fn append_paint(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        bounds: Rect,
        _layout: &LayoutOutput,
        theme: &ThemeTokens,
    ) {
        if bounds.has_finite_positive_area() {
            primitives.push(PaintPrimitive::FillRect(PaintFillRect {
                widget_id: self.common.id,
                rect: bounds,
                color: self.status.map_or(theme.grid_strong, |status| {
                    status_visual_color(status, theme)
                }),
            }));
        }
    }
}

fn status_dropdown_rail(status: storage::TrackStatus) -> ui::View<Message> {
    status_rail(Some(status), ui::dropdown_trigger_height())
}

fn status_rail(status: Option<storage::TrackStatus>, height: f32) -> ui::View<Message> {
    ui::custom_widget(StatusDropdownRailWidget::new(status), |_| None)
        .width(STATUS_RAIL_WIDTH)
        .height(height)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ImportBatchProgress {
    total: usize,
    completed: usize,
    failed: usize,
}

#[derive(Clone, Debug)]
struct AppState {
    library: storage::Library,
    workspace_mode: WorkspaceMode,
    status: String,
    busy: bool,
    save_in_flight: bool,
    save_again: bool,
    waveform: Option<audio::WaveformData>,
    waveform_track_id: Option<String>,
    waveform_busy: bool,
    waveform_generation: u64,
    waveform_cancellation: Option<ui::CancellationToken>,
    waveform_progress: Option<f32>,
    reference_waveform: Option<audio::WaveformData>,
    reference_waveform_track_id: Option<String>,
    reference_waveform_busy: bool,
    reference_waveform_generation: u64,
    reference_waveform_cancellation: Option<ui::CancellationToken>,
    reference_waveform_progress: Option<f32>,
    loop_selections: LoopSelections,
    review_cursor_millis: u64,
    playhead_drag_active: bool,
    reference_playhead_drag_active: bool,
    transport: transport::AudioTransport,
    transport_generation: u64,
    transport_position_millis: u64,
    transport_playing: bool,
    transport_polling: bool,
    transport_waiting_token: Option<u64>,
    audition_volume: f32,
    audition_source: AuditionSource,
    reference_transport: Option<transport::AudioTransport>,
    reference_transport_generation: u64,
    reference_transport_position_millis: u64,
    reference_transport_playing: bool,
    reference_transport_polling: bool,
    reference_transport_waiting_token: Option<u64>,
    reference_transport_loaded: bool,
    reference_only_playback: bool,
    reference_match_enabled: bool,
    comment_source: CommentSource,
    comment_source_explicit: bool,
    draft_note: Option<NoteDraft>,
    reference_draft_note: Option<NoteDraft>,
    persisted_note_drag: Option<PersistedNoteDrag>,
    reference_persisted_note_drag: Option<PersistedNoteDrag>,
    selected_note_id: Option<String>,
    hovered_note_id: Option<String>,
    selected_reference_note_id: Option<String>,
    hovered_reference_note_id: Option<String>,
    stage_menu_track_id: Option<String>,
    stage_menu_anchor: Option<Point>,
    status_menu_track_id: Option<String>,
    status_menu_host: Option<StatusMenuHost>,
    remove_confirmation_track_id: Option<String>,
    planner_drag_source_track_id: Option<String>,
    planner_drag_target_stage: Option<storage::TrackStage>,
    planner_drag_pointer: Option<Point>,
    review_status_filter: Option<storage::TrackStatus>,
    review_filter_menu_open: bool,
    planner_status_filter: Option<storage::TrackStatus>,
    audition_status_filter: storage::TrackStatus,
    audition_queue: Vec<String>,
    audition_queue_index: usize,
    audition_heard: Vec<String>,
    audition_shuffle_round: u64,
    audition_auto_advance: bool,
    audition_play_token: Option<u64>,
    audition_pending_play_track_id: Option<String>,
    import_batch: Option<ImportBatchProgress>,
    pending_import_paths: Vec<PathBuf>,
    pending_reference_paths: Vec<PathBuf>,
    pending_reference_track_id: Option<String>,
    reference_import_selected_path: Option<PathBuf>,
    reference_menu_track_id: Option<String>,
    reference_menu_anchor: Option<Point>,
}

#[derive(Clone, Debug)]
struct NoteDraft {
    note_id: Option<String>,
    time_millis: u64,
    body: String,
}

#[derive(Clone, Debug)]
struct PersistedNoteDrag {
    track_id: String,
    note_id: String,
    original_time_millis: u64,
    moved: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            library: storage::Library::default(),
            workspace_mode: WorkspaceMode::Review,
            status: String::from("Loading local library…"),
            busy: true,
            save_in_flight: false,
            save_again: false,
            waveform: None,
            waveform_track_id: None,
            waveform_busy: false,
            waveform_generation: 0,
            waveform_cancellation: None,
            waveform_progress: None,
            reference_waveform: None,
            reference_waveform_track_id: None,
            reference_waveform_busy: false,
            reference_waveform_generation: 0,
            reference_waveform_cancellation: None,
            reference_waveform_progress: None,
            loop_selections: LoopSelections::default(),
            review_cursor_millis: 0,
            playhead_drag_active: false,
            reference_playhead_drag_active: false,
            transport: transport::AudioTransport::spawn(),
            transport_generation: 0,
            transport_position_millis: 0,
            transport_playing: false,
            transport_polling: false,
            transport_waiting_token: None,
            audition_volume: transport::DEFAULT_VOLUME,
            audition_source: AuditionSource::Main,
            reference_transport: None,
            reference_transport_generation: 0,
            reference_transport_position_millis: 0,
            reference_transport_playing: false,
            reference_transport_polling: false,
            reference_transport_waiting_token: None,
            reference_transport_loaded: false,
            reference_only_playback: false,
            reference_match_enabled: false,
            comment_source: CommentSource::Main,
            comment_source_explicit: false,
            draft_note: None,
            reference_draft_note: None,
            persisted_note_drag: None,
            reference_persisted_note_drag: None,
            selected_note_id: None,
            hovered_note_id: None,
            selected_reference_note_id: None,
            hovered_reference_note_id: None,
            stage_menu_track_id: None,
            stage_menu_anchor: None,
            status_menu_track_id: None,
            status_menu_host: None,
            remove_confirmation_track_id: None,
            planner_drag_source_track_id: None,
            planner_drag_target_stage: None,
            planner_drag_pointer: None,
            review_status_filter: None,
            review_filter_menu_open: false,
            planner_status_filter: None,
            audition_status_filter: storage::TrackStatus::Inbox,
            audition_queue: Vec::new(),
            audition_queue_index: 0,
            audition_heard: Vec::new(),
            audition_shuffle_round: 0,
            audition_auto_advance: false,
            audition_play_token: None,
            audition_pending_play_track_id: None,
            import_batch: None,
            pending_import_paths: Vec::new(),
            pending_reference_paths: Vec::new(),
            pending_reference_track_id: None,
            reference_import_selected_path: None,
            reference_menu_track_id: None,
            reference_menu_anchor: None,
        }
    }
}

fn playback_shortcut(state: &AppState, press: ui::KeyPress) -> ui::ShortcutResolution<Message> {
    if press == ui::KeyPress::new(ui::KeyCode::Escape) {
        ui::ShortcutResolution::action(Message::StopPlayback)
    } else if state.draft_note.is_none()
        && state.reference_draft_note.is_none()
        && press == ui::KeyPress::new(ui::KeyCode::N)
    {
        ui::ShortcutResolution::action(Message::NewNoteAtCurrentTime)
    } else if state.draft_note.is_none() && press == ui::KeyPress::new(ui::KeyCode::Space) {
        ui::ShortcutResolution::action(Message::TogglePlayback)
    } else {
        ui::ShortcutResolution::unhandled()
    }
}

fn native_launch_options() -> NativeRunOptions {
    let mut options = NativeRunOptions::default();
    options.window.behavior.maximized = true;
    options.window.behavior.integrated_titlebar = true;
    options
        .window
        .behavior
        .integrated_titlebar_drag_region_height = Some(42.0);
    options
}

fn main() -> radiant::Result {
    let _instance_lock = match storage::acquire_instance_lock() {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("Could not start Cadence: {error}");
            return Ok(());
        }
    };

    radiant::app(AppState::default())
        .options(native_launch_options())
        .title("Cadence — local track review")
        .size(1180, 720)
        .min_size(900, 560)
        .view(project_surface)
        .animation(|state| {
            state.transport_playing
                || state.transport_polling
                || state.reference_transport_playing
                || state.reference_transport_polling
                || state.audition_pending_play_track_id.is_some()
                || state.playhead_drag_active
                || state.reference_playhead_drag_active
                || state.planner_drag_source_track_id.is_some()
        })
        .on_frame(|| Message::Frame)
        .on_startup(|_state, context| schedule_library_load(context))
        .shortcuts(|state, _pending, press, _focus| playback_shortcut(state, press))
        .handle_message(update)
        .run()
}

fn schedule_library_load(context: &mut ui::UiUpdateContext<Message>) {
    context
        .business()
        .blocking_io("cadence-load-library")
        .run(|_| storage::load_library(), Message::LibraryLoaded);
}

fn decode_or_load_cached_waveform(
    path: &Path,
    cache_path: &Path,
    should_cancel: impl Fn() -> bool,
    mut on_progress: impl FnMut(audio::WaveformProgress),
) -> Result<audio::WaveformData, String> {
    let source_fingerprint = audio::waveform_cache_fingerprint(path);
    if should_cancel() {
        return Err(String::from("cancelled"));
    }
    if let Some(waveform) = audio::load_waveform_cache(path, cache_path) {
        if should_cancel() {
            return Err(String::from("cancelled"));
        }
        on_progress(audio::WaveformProgress {
            waveform: waveform.clone(),
            progress: Some(1.0),
        });
        return Ok(waveform);
    }

    let result = audio::decode_waveform_with_progress_and_cancellation(
        path,
        &should_cancel,
        &mut on_progress,
    );
    if should_cancel() {
        return Err(String::from("cancelled"));
    }
    if let Ok(waveform) = &result
        && let Some(source_fingerprint) = source_fingerprint
    {
        let _ = audio::write_waveform_cache_if_unchanged(
            path,
            cache_path,
            source_fingerprint,
            waveform,
        );
    }
    result
}

fn schedule_waveform_decode(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    track_id: String,
    path: PathBuf,
) {
    if let Some(cancellation) = state.waveform_cancellation.take() {
        cancellation.cancel();
    }
    state.waveform_busy = true;
    state.waveform_generation = state.waveform_generation.wrapping_add(1);
    let generation = state.waveform_generation;
    state.waveform = None;
    state.waveform_track_id = None;
    state.waveform_progress = None;
    state.loop_selections.clear(AuditionSource::Main);
    state.status = format!("Preparing MAIN waveform and loudness · {}…", path.display());
    let cache_path = storage::waveform_cache_path(&path);
    let progress_track_id = track_id.clone();
    let completion_track_id = track_id;
    let cancellation = context
        .business()
        .blocking_io("cadence-decode-waveform")
        .cancellable()
        .stream_latest(
            move |work, sink| {
                decode_or_load_cached_waveform(
                    &path,
                    &cache_path,
                    || work.is_cancelled(),
                    |progress| {
                        let _ = sink.emit(progress);
                    },
                )
            },
            move |progress| Message::DecodeProgress {
                track_id: progress_track_id.clone(),
                generation,
                progress,
            },
            move |result| Message::DecodeCompleted {
                track_id: completion_track_id.clone(),
                generation,
                result,
            },
        );
    state.waveform_cancellation = Some(cancellation);
    context.request_repaint();
}

fn schedule_selected_waveform_decode(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
) {
    let selected = state
        .library
        .selected_track_id
        .as_ref()
        .and_then(|id| state.library.tracks.iter().find(|track| &track.id == id))
        .map(|track| (track.id.clone(), track.path.clone()));
    if let Some((track_id, path)) = selected {
        schedule_waveform_decode(state, context, track_id, path);
    } else {
        if let Some(cancellation) = state.waveform_cancellation.take() {
            cancellation.cancel();
        }
        state.waveform_generation = state.waveform_generation.wrapping_add(1);
        state.waveform_busy = false;
        state.waveform = None;
        state.waveform_track_id = None;
        state.waveform_progress = None;
        state.loop_selections.clear(AuditionSource::Main);
        context.request_repaint();
    }
}

fn schedule_reference_waveform_decode(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    track_id: String,
    path: PathBuf,
) {
    if let Some(cancellation) = state.reference_waveform_cancellation.take() {
        cancellation.cancel();
    }
    state.reference_waveform_busy = true;
    state.reference_waveform_generation = state.reference_waveform_generation.wrapping_add(1);
    let generation = state.reference_waveform_generation;
    state.reference_waveform = None;
    state.reference_waveform_track_id = None;
    state.reference_waveform_progress = None;
    state.loop_selections.clear(AuditionSource::Reference);
    let cache_path = storage::waveform_cache_path(&path);
    let progress_track_id = track_id.clone();
    let completion_track_id = track_id;
    let cancellation = context
        .business()
        .blocking_io("cadence-decode-reference-waveform")
        .cancellable()
        .stream_latest(
            move |work, sink| {
                decode_or_load_cached_waveform(
                    &path,
                    &cache_path,
                    || work.is_cancelled(),
                    |progress| {
                        let _ = sink.emit(progress);
                    },
                )
            },
            move |progress| Message::ReferenceDecodeProgress {
                track_id: progress_track_id.clone(),
                generation,
                progress,
            },
            move |result| Message::ReferenceDecodeCompleted {
                track_id: completion_track_id.clone(),
                generation,
                result,
            },
        );
    state.reference_waveform_cancellation = Some(cancellation);
    context.request_repaint();
}

fn schedule_selected_reference_decode(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
) {
    let selected = state
        .library
        .selected_track_id
        .as_ref()
        .and_then(|id| state.library.tracks.iter().find(|track| &track.id == id))
        .and_then(|track| {
            track
                .reference_path
                .clone()
                .map(|path| (track.id.clone(), path))
        });
    if let Some((track_id, path)) = selected {
        schedule_reference_waveform_decode(state, context, track_id, path);
    } else {
        if let Some(cancellation) = state.reference_waveform_cancellation.take() {
            cancellation.cancel();
        }
        state.reference_waveform_generation = state.reference_waveform_generation.wrapping_add(1);
        state.reference_waveform_busy = false;
        state.reference_waveform = None;
        state.reference_waveform_track_id = None;
        state.reference_waveform_progress = None;
        state.loop_selections.clear(AuditionSource::Reference);
        context.request_repaint();
    }
}

fn current_loudness_match_gain_db(state: &AppState) -> Option<f32> {
    let primary_lufs = state
        .waveform
        .as_ref()
        .filter(|_| {
            state.library.selected_track_id.as_deref() == state.waveform_track_id.as_deref()
        })
        .and_then(|waveform| waveform.integrated_lufs);
    let reference_lufs = state
        .reference_waveform
        .as_ref()
        .filter(|_| {
            state.library.selected_track_id.as_deref()
                == state.reference_waveform_track_id.as_deref()
        })
        .and_then(|waveform| waveform.integrated_lufs);
    audio::loudness_match_gain_db(primary_lufs, reference_lufs)
}

fn current_lufs_meter_value(state: &AppState, track_id: &str) -> Option<f32> {
    let position_millis = state.transport_position_millis;
    state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(track_id))
        .and_then(|waveform| audio::loudness_at_position(waveform, position_millis))
}

fn current_reference_lufs_meter_value(state: &AppState, track_id: &str) -> Option<f32> {
    let position_millis = state.reference_transport_position_millis;
    state
        .reference_waveform
        .as_ref()
        .filter(|_| state.reference_waveform_track_id.as_deref() == Some(track_id))
        .and_then(|waveform| audio::loudness_at_position(waveform, position_millis))
}

fn reference_output_gain(state: &AppState) -> f32 {
    let match_gain = state
        .reference_match_enabled
        .then(|| current_loudness_match_gain_db(state))
        .flatten()
        .map_or(1.0, audio::linear_gain_for_db);
    if state.audition_source == AuditionSource::Reference {
        transport::normalize_output_gain(state.audition_volume * match_gain)
    } else {
        0.0
    }
}

fn main_output_gain(state: &AppState) -> f32 {
    if state.audition_source == AuditionSource::Main {
        state.audition_volume
    } else {
        0.0
    }
}

fn sync_audition_output_gains(state: &AppState) {
    state.transport.set_output_gain(main_output_gain(state));
    if let Some(reference_transport) = state.reference_transport.as_ref() {
        reference_transport.set_output_gain(reference_output_gain(state));
    }
}

fn update(state: &mut AppState, message: Message, context: &mut ui::UiUpdateContext<Message>) {
    match message {
        Message::ImportPressed => request_import(state, context),
        Message::FilePicked(result) => match result {
            Ok(PlatformResponse::Path(path)) => schedule_import(state, context, path),
            Ok(PlatformResponse::Canceled) => {
                state.status = String::from("Import canceled.");
                context.request_repaint();
            }
            Ok(response) => {
                state.status = format!("Unexpected file-picker response: {response:?}");
                context.request_repaint();
            }
            Err(error) => {
                state.status = format!("Could not open the file picker: {error}");
                context.request_repaint();
            }
        },
        Message::ReplacePressed(track_id) => request_replace(state, context, track_id),
        Message::ReplaceFilePicked { track_id, result } => match result {
            Ok(PlatformResponse::Path(path)) => schedule_replace(state, context, track_id, path),
            Ok(PlatformResponse::Canceled) => {
                state.status = String::from("Replace canceled.");
                context.request_repaint();
            }
            Ok(response) => {
                state.status = format!("Unexpected file-picker response: {response:?}");
                context.request_repaint();
            }
            Err(error) => {
                state.status = format!("Could not open the file picker: {error}");
                context.request_repaint();
            }
        },
        Message::ReferencePressed(track_id) => request_reference(state, context, track_id),
        Message::ReferenceFilesPicked { track_id, paths } => {
            state.busy = false;
            if paths.is_empty() {
                state.status = String::from("Reference import canceled.");
            } else {
                schedule_reference_import(state, context, track_id, paths);
            }
            context.request_repaint();
        }
        Message::FileDropped(drop) => {
            if drop.phase == ui::NativeFileDropPhase::Drop
                && let Some(path) = drop.path
            {
                schedule_import(state, context, path);
            }
        }
        Message::LibraryLoaded(result) => {
            state.busy = false;
            clear_planner_drag(state);
            match result {
                Ok(library) => {
                    state.status = if library.tracks.is_empty() {
                        String::from("Ready — import a track to begin.")
                    } else {
                        format!(
                            "{} local track{} loaded.",
                            library.tracks.len(),
                            plural(library.tracks.len())
                        )
                    };
                    state.library = library;
                    if state.workspace_mode == WorkspaceMode::Audition {
                        rebuild_audition_queue(state);
                    }
                    state.review_cursor_millis = 0;
                    state.draft_note = None;
                    state.reference_draft_note = None;
                    rollback_persisted_note_drag(state);
                    rollback_reference_persisted_note_drag(state);
                    state.reference_playhead_drag_active = false;
                    state.selected_note_id = None;
                    state.hovered_note_id = None;
                    state.selected_reference_note_id = None;
                    state.hovered_reference_note_id = None;
                    state.comment_source = CommentSource::Main;
                    state.comment_source_explicit = false;
                    close_stage_menu(state);
                    close_status_menu(state);
                    close_reference_menu(state);
                    state.remove_confirmation_track_id = None;
                    reset_transport(state);
                    reset_reference_transport(state);
                    state.reference_match_enabled = false;
                    schedule_selected_waveform_decode(state, context);
                    schedule_selected_reference_decode(state, context);
                }
                Err(error) => {
                    state.status = error;
                }
            }
            schedule_next_pending_import(state, context);
            context.request_repaint();
        }
        Message::ImportCompleted(result) => {
            let failed = result.is_err();
            state.busy = false;
            record_import_attempt(state, failed);
            clear_planner_drag(state);
            match result {
                Ok(library) => {
                    state.status = format!(
                        "{} local track{} — all changes saved.",
                        library.tracks.len(),
                        plural(library.tracks.len())
                    );
                    state.library = library;
                    if state.workspace_mode == WorkspaceMode::Audition {
                        reconcile_audition_queue(state);
                    }
                    state.review_cursor_millis = 0;
                    state.draft_note = None;
                    state.reference_draft_note = None;
                    rollback_persisted_note_drag(state);
                    rollback_reference_persisted_note_drag(state);
                    state.reference_playhead_drag_active = false;
                    state.selected_note_id = None;
                    state.hovered_note_id = None;
                    state.selected_reference_note_id = None;
                    state.hovered_reference_note_id = None;
                    state.comment_source = CommentSource::Main;
                    state.comment_source_explicit = false;
                    close_stage_menu(state);
                    close_status_menu(state);
                    close_reference_menu(state);
                    state.remove_confirmation_track_id = None;
                    reset_transport(state);
                    reset_reference_transport(state);
                    state.reference_match_enabled = false;
                    schedule_selected_waveform_decode(state, context);
                    schedule_selected_reference_decode(state, context);
                }
                Err(error) => {
                    state.status = error;
                }
            }
            finish_import_batch(state);
            schedule_next_pending_import(state, context);
            context.request_repaint();
        }
        Message::ReplaceCompleted { track_id, result } => {
            state.busy = false;
            clear_planner_drag(state);
            match result {
                Ok(library) => {
                    let title = library
                        .tracks
                        .iter()
                        .find(|track| track.id == track_id)
                        .map(|track| track.title.clone())
                        .unwrap_or_else(|| String::from("track"));
                    state.library = library;
                    state.library.selected_track_id = Some(track_id);
                    if state.workspace_mode == WorkspaceMode::Audition {
                        reconcile_audition_queue(state);
                    }
                    state.review_cursor_millis = 0;
                    state.draft_note = None;
                    state.reference_draft_note = None;
                    rollback_persisted_note_drag(state);
                    rollback_reference_persisted_note_drag(state);
                    state.reference_playhead_drag_active = false;
                    state.selected_note_id = None;
                    state.hovered_note_id = None;
                    state.selected_reference_note_id = None;
                    state.hovered_reference_note_id = None;
                    close_stage_menu(state);
                    close_status_menu(state);
                    close_reference_menu(state);
                    state.remove_confirmation_track_id = None;
                    reset_transport(state);
                    reset_reference_transport(state);
                    state.reference_match_enabled = false;
                    state.status = format!("Replaced {title}; existing comments were cleared.");
                    schedule_selected_waveform_decode(state, context);
                    schedule_selected_reference_decode(state, context);
                }
                Err(error) => state.status = error,
            }
            schedule_next_pending_import(state, context);
            context.request_repaint();
        }
        Message::ReferenceImportCompleted {
            track_id,
            path,
            result,
        } => {
            let failed = result.is_err();
            state.busy = false;
            record_import_attempt(state, failed);
            match result {
                Ok(library) => {
                    state.library = library;
                    if state.reference_import_selected_path.is_none() {
                        state.reference_import_selected_path = Some(path);
                    }
                    let title = state
                        .library
                        .tracks
                        .iter()
                        .find(|track| track.id == track_id)
                        .map(|track| track.title.clone())
                        .unwrap_or_else(|| String::from("track"));
                    state.status = format!("Reference track added for {title}.");
                }
                Err(error) => state.status = error,
            }
            let has_more = !state.pending_reference_paths.is_empty();
            if has_more {
                schedule_next_pending_reference_import(state, context);
            } else {
                let selected_path = state.reference_import_selected_path.take();
                state.pending_reference_track_id = None;
                if let Some(path) = selected_path {
                    match storage::set_reference_track_selection(
                        &mut state.library,
                        &track_id,
                        path,
                    ) {
                        Ok(changed) if changed => schedule_library_save(state, context),
                        Ok(_) => {}
                        Err(error) => state.status = error,
                    }
                }
                finish_import_batch(state);
                if state.library.selected_track_id.as_deref() == Some(track_id.as_str()) {
                    reset_reference_transport(state);
                    state.reference_match_enabled = false;
                    schedule_selected_reference_decode(state, context);
                }
                schedule_next_pending_import(state, context);
            }
            close_reference_menu(state);
            context.request_repaint();
        }
        Message::ToggleReferenceMenu(track_id) => {
            if !state.busy
                && state
                    .library
                    .tracks
                    .iter()
                    .any(|track| track.id == track_id)
            {
                if state.reference_menu_track_id.as_deref() == Some(track_id.as_str()) {
                    close_reference_menu(state);
                } else {
                    close_stage_menu(state);
                    close_status_menu(state);
                    state.reference_menu_track_id = Some(track_id);
                    state.reference_menu_anchor = Some(keyboard_reference_menu_anchor());
                }
                context.request_repaint();
            }
        }
        Message::ToggleReferenceMenuAt { track_id, position } => {
            if !state.busy
                && state
                    .library
                    .tracks
                    .iter()
                    .any(|track| track.id == track_id)
            {
                if state.reference_menu_track_id.as_deref() == Some(track_id.as_str()) {
                    close_reference_menu(state);
                } else {
                    close_stage_menu(state);
                    close_status_menu(state);
                    state.reference_menu_track_id = Some(track_id);
                    state.reference_menu_anchor =
                        Some(reference_menu_anchor_from_pointer(position));
                }
                context.request_repaint();
            }
        }
        Message::DecodeProgress {
            track_id,
            generation,
            progress,
        } => {
            if !decode_result_is_current(state, &track_id, generation) {
                return;
            }
            state.waveform_track_id = Some(track_id);
            state.waveform_progress = progress.progress;
            state.waveform = Some(progress.waveform);
            context.request_repaint();
        }
        Message::DecodeCompleted {
            track_id,
            generation,
            result,
        } => {
            if !decode_result_is_current(state, &track_id, generation) {
                return;
            }
            state.waveform_cancellation = None;
            state.waveform_progress = None;
            state.waveform_busy = false;
            let pending_audition = state.workspace_mode == WorkspaceMode::Audition
                && state.audition_pending_play_track_id.as_deref() == Some(track_id.as_str());
            match result {
                Ok(waveform) => {
                    state.waveform_track_id = Some(track_id.clone());
                    state.status = format!(
                        "Waveform ready · {} Hz · {} channel{} · {}.",
                        waveform.sample_rate,
                        waveform.channels,
                        if waveform.channels == 1 { "" } else { "s" },
                        format_duration(waveform.duration_millis),
                    );
                    state.waveform = Some(waveform);
                    if let Some(path) = selected_track(state)
                        .filter(|track| track.id == track_id)
                        .map(|track| track.path.clone())
                    {
                        match state.transport.load(
                            state.transport_generation,
                            path,
                            state
                                .waveform
                                .as_ref()
                                .map_or(0, |waveform| waveform.duration_millis),
                        ) {
                            Ok(token) => begin_transport_polling(state, token),
                            Err(error) => {
                                state.status = error;
                                if pending_audition {
                                    state.audition_auto_advance = false;
                                    state.audition_play_token = None;
                                    state.audition_pending_play_track_id = None;
                                    advance_audition(state, context);
                                }
                            }
                        }
                    }
                }
                Err(error) if error == "cancelled" => {
                    state.waveform = None;
                    state.waveform_track_id = None;
                }
                Err(error) => {
                    state.waveform = None;
                    state.waveform_track_id = None;
                    state.status = format!("Waveform unavailable: {error}");
                    if pending_audition {
                        state.audition_auto_advance = false;
                        state.audition_play_token = None;
                        state.audition_pending_play_track_id = None;
                        advance_audition(state, context);
                    }
                }
            }
            context.request_repaint();
        }
        Message::ReferenceDecodeProgress {
            track_id,
            generation,
            progress,
        } => {
            if !reference_decode_result_is_current(state, &track_id, generation) {
                return;
            }
            state.reference_waveform_track_id = Some(track_id);
            state.reference_waveform_progress = progress.progress;
            state.reference_waveform = Some(progress.waveform);
            context.request_repaint();
        }
        Message::ReferenceDecodeCompleted {
            track_id,
            generation,
            result,
        } => {
            if !reference_decode_result_is_current(state, &track_id, generation) {
                return;
            }
            state.reference_waveform_cancellation = None;
            state.reference_waveform_progress = None;
            state.reference_waveform_busy = false;
            match result {
                Ok(waveform) => {
                    state.reference_waveform_track_id = Some(track_id);
                    state.reference_waveform = Some(waveform);
                }
                Err(error) if error == "cancelled" => {
                    state.reference_waveform = None;
                    state.reference_waveform_track_id = None;
                }
                Err(error) => {
                    state.reference_waveform = None;
                    state.reference_waveform_track_id = None;
                    state.status = format!("Reference waveform unavailable: {error}");
                }
            }
            context.request_repaint();
        }
        Message::Frame => {
            if state.planner_drag_source_track_id.is_some() {
                state.planner_drag_pointer = context.current_pointer_position();
            }
            let was_audition_playing = state.workspace_mode == WorkspaceMode::Audition
                && state.audition_auto_advance
                && state.transport_playing;
            let was_main_playing = !state.reference_only_playback && state.transport_playing;
            let was_reference_playing = state.reference_transport_playing;
            update_reference_transport(state);
            let snapshot = state.transport.snapshot();
            let audition_play_acknowledged = state.workspace_mode == WorkspaceMode::Audition
                && state.audition_auto_advance
                && state
                    .audition_play_token
                    .is_some_and(|token| transport_command_is_confirmed(snapshot, token));
            let pending_audition = state.workspace_mode == WorkspaceMode::Audition
                && state.audition_pending_play_track_id.is_some();
            let mut main_snapshot_applied = false;
            if snapshot.generation == state.transport_generation {
                if let Some(error) = state.transport.take_error(state.transport_generation) {
                    state.playhead_drag_active = false;
                    state.transport_playing = false;
                    state.transport_polling = false;
                    state.transport_waiting_token = None;
                    state.audition_auto_advance = false;
                    state.audition_play_token = None;
                    state.audition_pending_play_track_id = None;
                    state.status = error;
                    if pending_audition {
                        advance_audition(state, context);
                    }
                } else if state
                    .transport_waiting_token
                    .is_none_or(|token| transport_command_is_confirmed(snapshot, token))
                {
                    state.transport_waiting_token = None;
                    apply_transport_snapshot(state, snapshot);
                    main_snapshot_applied = true;
                }
            }
            let natural_audition_completion = main_snapshot_applied
                && (was_audition_playing || audition_play_acknowledged)
                && state.workspace_mode == WorkspaceMode::Audition
                && !state.transport_playing
                && !state.transport_polling
                && state.transport_waiting_token.is_none()
                && snapshot.ready
                && !snapshot.playing
                && state.waveform_track_id.as_deref() == state.library.selected_track_id.as_deref()
                && state
                    .waveform
                    .as_ref()
                    .is_some_and(|waveform| snapshot.position_millis >= waveform.duration_millis);
            if natural_audition_completion && state.loop_selections.main.is_none() {
                advance_audition(state, context);
            } else if state.loop_selections.main.is_none() {
                maybe_start_pending_audition(state, context);
            }
            enforce_loop(state, was_main_playing, was_reference_playing);
            context.request_repaint();
        }
        Message::LibrarySaved(result) => {
            state.save_in_flight = false;
            let save_again = state.save_again;
            state.save_again = false;
            state.status = match result {
                Ok(()) => String::from("All changes saved locally."),
                Err(error) => error,
            };
            if save_again {
                schedule_library_save(state, context);
            } else {
                schedule_next_pending_import(state, context);
                schedule_next_pending_reference_import(state, context);
            }
            context.request_repaint();
        }
        Message::SelectTrack(id) => {
            if !state.busy && state.library.tracks.iter().any(|track| track.id == id) {
                if state.workspace_mode == WorkspaceMode::Audition {
                    state.audition_heard.retain(|heard_id| heard_id != &id);
                }
                select_track_internal(state, context, id, false);
            }
        }
        Message::SelectWorkspace(mode) => {
            set_workspace_mode(state, context, mode);
        }
        Message::SetAuditionFilter(status) => {
            if state.busy || state.workspace_mode != WorkspaceMode::Audition {
                return;
            }
            state.audition_status_filter = status;
            state.audition_shuffle_round = 0;
            state.audition_auto_advance = false;
            state.audition_play_token = None;
            state.audition_pending_play_track_id = None;
            rebuild_audition_queue(state);
            if let Some(track_id) = state.audition_queue.first().cloned() {
                select_track_internal(state, context, track_id, false);
                state.status = format!(
                    "Auditioning {} tracks in {}.",
                    state.audition_queue.len(),
                    status.label()
                );
            } else {
                reset_transport(state);
                reset_reference_transport(state);
                state.status = format!("No tracks in {}.", status.label());
            }
            context.request_repaint();
        }
        Message::SetReviewStatusFilter(status) => {
            if state.review_status_filter != status {
                state.review_status_filter = status;
                close_stage_menu(state);
                close_status_menu(state);
            }
            state.review_filter_menu_open = false;
            context.request_repaint();
        }
        Message::ToggleReviewFilterMenu => {
            state.review_filter_menu_open = !state.review_filter_menu_open;
            context.request_repaint();
        }
        Message::SetPlannerStatusFilter(status) => {
            if state.planner_status_filter != status {
                state.planner_status_filter = status;
                close_stage_menu(state);
                close_status_menu(state);
                clear_planner_drag(state);
            }
            context.request_repaint();
        }
        Message::ShuffleAudition => {
            shuffle_audition(state, context);
        }
        Message::ToggleFavorite(id) => {
            if !state.busy
                && let Some(track) = state.library.tracks.iter_mut().find(|track| track.id == id)
            {
                track.favorite = !track.favorite;
                schedule_library_save(state, context);
            }
        }
        Message::ToggleStageMenu(id) => {
            if !state.busy && state.library.tracks.iter().any(|track| track.id == id) {
                if state.stage_menu_track_id.as_deref() == Some(id.as_str()) {
                    close_stage_menu(state);
                } else {
                    close_status_menu(state);
                    state.stage_menu_track_id = Some(id);
                    state.stage_menu_anchor = Some(keyboard_stage_menu_anchor(state));
                }
                context.request_repaint();
            }
        }
        Message::ToggleStageMenuAt { track_id, position } => {
            if !state.busy
                && state
                    .library
                    .tracks
                    .iter()
                    .any(|track| track.id == track_id)
            {
                if state.stage_menu_track_id.as_deref() == Some(track_id.as_str()) {
                    close_stage_menu(state);
                } else {
                    close_status_menu(state);
                    state.stage_menu_track_id = Some(track_id);
                    state.stage_menu_anchor = Some(stage_menu_anchor_from_pointer(position));
                }
                context.request_repaint();
            }
        }
        Message::SetStage { track_id, stage } => {
            if !state.busy {
                let changed = match storage::set_track_stage(&mut state.library, &track_id, stage) {
                    Ok(changed) => changed,
                    Err(error) => {
                        state.status = error;
                        context.request_repaint();
                        return;
                    }
                };
                close_stage_menu(state);
                close_status_menu(state);
                clear_planner_drag(state);
                if changed {
                    state.status = format!("Stage set to {}.", stage.label());
                    schedule_library_save(state, context);
                }
                context.request_repaint();
            }
        }
        Message::ToggleStatusMenuAt { track_id, host } => {
            toggle_status_menu(state, track_id, host, context);
        }
        Message::SetReferenceTrack { track_id, path } => {
            if !state.busy {
                let selected =
                    state.library.selected_track_id.as_deref() == Some(track_id.as_str());
                let changed = match storage::set_reference_track_selection(
                    &mut state.library,
                    &track_id,
                    path.clone(),
                ) {
                    Ok(changed) => changed,
                    Err(error) => {
                        state.status = error;
                        close_reference_menu(state);
                        context.request_repaint();
                        return;
                    }
                };
                close_reference_menu(state);
                if changed {
                    if selected {
                        reset_reference_transport(state);
                        state.reference_match_enabled = false;
                        schedule_selected_reference_decode(state, context);
                    }
                    state.reference_draft_note = None;
                    state.selected_reference_note_id = None;
                    state.hovered_reference_note_id = None;
                    state.status = format!("Reference set to {}.", reference_track_name(&path));
                    schedule_library_save(state, context);
                }
                context.request_repaint();
            }
        }
        Message::SetStatus { track_id, status } => {
            if !state.busy {
                let pending_selected_audition = state.workspace_mode == WorkspaceMode::Audition
                    && state.library.selected_track_id.as_deref() == Some(track_id.as_str())
                    && state.audition_pending_play_track_id.as_deref() == Some(track_id.as_str())
                    && status != state.audition_status_filter;
                let changed = match storage::set_track_status(&mut state.library, &track_id, status)
                {
                    Ok(changed) => changed,
                    Err(error) => {
                        state.status = error;
                        close_status_menu(state);
                        context.request_repaint();
                        return;
                    }
                };
                close_status_menu(state);
                if changed {
                    if state.workspace_mode == WorkspaceMode::Audition {
                        sync_audition_queue_after_status_change(state, &track_id);
                        if pending_selected_audition {
                            disarm_audition_auto_advance(state);
                            advance_audition(state, context);
                        }
                    }
                    state.status = format!("Status set to {}.", status.label());
                    schedule_library_save(state, context);
                }
                context.request_repaint();
            }
        }
        Message::PlannerCardDrag { track_id, message } => {
            if state.busy || state.workspace_mode != WorkspaceMode::Planner {
                return;
            }
            if !state
                .library
                .tracks
                .iter()
                .any(|track| track.id == track_id)
            {
                clear_planner_drag(state);
                state.status = String::from("That track is no longer in the library.");
                context.request_repaint();
                return;
            }
            match message {
                ui::DragHandleMessage::Started { .. } => {
                    state.planner_drag_source_track_id = Some(track_id.clone());
                    state.planner_drag_target_stage = None;
                    close_stage_menu(state);
                    close_status_menu(state);
                    state.remove_confirmation_track_id = None;
                    state.planner_drag_pointer = drag_message_position(message);
                    let title = state
                        .library
                        .tracks
                        .iter()
                        .find(|track| track.id == track_id)
                        .map_or_else(|| String::from("track"), |track| track.title.clone());
                    state.status = format!("Dragging {title}…");
                }
                ui::DragHandleMessage::Moved { .. } => {
                    if state.planner_drag_source_track_id.as_deref() != Some(track_id.as_str()) {
                        return;
                    }
                    state.planner_drag_pointer = drag_message_position(message);
                }
                ui::DragHandleMessage::Ended { .. } => {
                    if state.planner_drag_source_track_id.as_deref() != Some(track_id.as_str()) {
                        return;
                    }
                    if state.planner_drag_target_stage.is_none() {
                        clear_planner_drag(state);
                        state.status = String::from("Drag canceled.");
                    } else {
                        state.planner_drag_pointer = drag_message_position(message);
                    }
                }
                ui::DragHandleMessage::Cancelled { .. } => {
                    if state.planner_drag_source_track_id.as_deref() != Some(track_id.as_str()) {
                        return;
                    }
                    clear_planner_drag(state);
                    state.status = String::from("Drag canceled.");
                }
                ui::DragHandleMessage::DoubleActivate { .. } => {}
            }
            context.request_repaint();
        }
        Message::PlannerCardHandleActivated(_) => {}
        Message::PlannerStageHovered(stage) => {
            let Some(source_id) = state.planner_drag_source_track_id.as_deref() else {
                return;
            };
            let Some(source) = state
                .library
                .tracks
                .iter()
                .find(|track| track.id == source_id)
            else {
                clear_planner_drag(state);
                context.request_repaint();
                return;
            };
            if planner_drop_is_valid(Some(source.stage), stage) {
                state.planner_drag_target_stage = Some(stage);
                state.status = format!("Release to move to {}.", stage.label());
            }
            context.request_repaint();
        }
        Message::PlannerStageHoverCleared(stage) => {
            if state.planner_drag_target_stage == Some(stage) {
                state.planner_drag_target_stage = None;
                context.request_repaint();
            }
        }
        Message::PlannerStageDropped(stage) => {
            if state.busy || state.workspace_mode != WorkspaceMode::Planner {
                return;
            }
            let Some(source_id) = state.planner_drag_source_track_id.clone() else {
                return;
            };
            let source_stage = state
                .library
                .tracks
                .iter()
                .find(|track| track.id == source_id)
                .map(|track| track.stage);
            clear_planner_drag(state);
            let Some(source_stage) = source_stage else {
                state.status = String::from("That track is no longer in the library.");
                context.request_repaint();
                return;
            };
            if !planner_drop_is_valid(Some(source_stage), stage) {
                state.status = format!("Track is already in {}.", stage.label());
                context.request_repaint();
                return;
            }
            match storage::set_track_stage(&mut state.library, &source_id, stage) {
                Ok(true) => {
                    close_stage_menu(state);
                    close_status_menu(state);
                    state.status = format!("Moved track to {}.", stage.label());
                    schedule_library_save(state, context);
                }
                Ok(false) => {
                    state.status = format!("Track is already in {}.", stage.label());
                }
                Err(error) => state.status = error,
            }
            context.request_repaint();
        }
        Message::RequestRemoveTrack(id) => {
            if !state.busy && state.library.tracks.iter().any(|track| track.id == id) {
                close_stage_menu(state);
                close_status_menu(state);
                state.remove_confirmation_track_id = Some(id);
                state.status = String::from(
                    "Confirm removal from the library. The source audio file will stay on disk.",
                );
                context.request_repaint();
            }
        }
        Message::ConfirmRemoveTrack(id) => {
            if state.busy || state.remove_confirmation_track_id.as_deref() != Some(id.as_str()) {
                return;
            }
            let selected = state.library.selected_track_id.as_deref() == Some(id.as_str());
            let removed = match storage::remove_track(&mut state.library, &id) {
                Ok(removed) => removed,
                Err(error) => {
                    state.remove_confirmation_track_id = None;
                    state.status = error;
                    context.request_repaint();
                    return;
                }
            };
            state.remove_confirmation_track_id = None;
            close_stage_menu(state);
            close_status_menu(state);
            clear_planner_drag(state);
            if state.workspace_mode == WorkspaceMode::Audition {
                reconcile_audition_queue(state);
            }
            if selected {
                state.draft_note = None;
                rollback_persisted_note_drag(state);
                rollback_reference_persisted_note_drag(state);
                state.reference_playhead_drag_active = false;
                state.selected_note_id = None;
                state.hovered_note_id = None;
                state.library.selected_track_id =
                    storage::selection_after_removal(&state.library, removed.0);
                if let Some(cancellation) = state.waveform_cancellation.take() {
                    cancellation.cancel();
                }
                if let Some(cancellation) = state.reference_waveform_cancellation.take() {
                    cancellation.cancel();
                }
                state.waveform = None;
                state.waveform_track_id = None;
                state.waveform_busy = false;
                state.waveform_progress = None;
                state.reference_waveform = None;
                state.reference_waveform_track_id = None;
                state.reference_waveform_busy = false;
                state.reference_waveform_progress = None;
                reset_transport(state);
                reset_reference_transport(state);
                state.reference_match_enabled = false;
            }
            state.status = format!(
                "Removed {} from the library. The source audio file remains on disk.",
                removed.1.title
            );
            schedule_library_save(state, context);
            if selected {
                schedule_selected_waveform_decode(state, context);
                schedule_selected_reference_decode(state, context);
            }
            context.request_repaint();
        }
        Message::CancelRemoveTrack => {
            state.remove_confirmation_track_id = None;
            state.status = String::from("Track kept in the library.");
            context.request_repaint();
        }
        Message::TogglePlayback => toggle_playback(state, context),
        Message::StopPlayback => stop_playback(state, context),
        Message::NewNoteAtCurrentTime => start_note_at_current_time(state, context),
        Message::AuditionPlay => play_audition(state, context),
        Message::AuditionPrevious => previous_audition(state, context),
        Message::AuditionNext => next_audition(state, context),
        Message::SelectAuditionSource(source) => select_audition_source(state, context, source),
        Message::SelectCommentSource(source) => {
            state.comment_source = source;
            state.comment_source_explicit = true;
            context.request_repaint();
        }
        Message::ToggleReferenceMatch => {
            let Some(gain_db) = current_loudness_match_gain_db(state) else {
                state.status = String::from(
                    "Reference matching needs LUFS analysis for both the imported and reference tracks.",
                );
                context.request_repaint();
                return;
            };
            state.reference_match_enabled = !state.reference_match_enabled;
            sync_audition_output_gains(state);
            state.status = if state.reference_match_enabled {
                format!("Reference matched to the imported track · {gain_db:+.1} dB.")
            } else {
                String::from("Reference loudness matching disabled.")
            };
            context.request_repaint();
        }
        Message::AuditionVolumeChanged(volume) => {
            let volume = transport::normalize_volume(volume);
            state.audition_volume = volume;
            // This gain is applied only by Rodio's output player. The
            // decoder's integrated LUFS value is computed from raw samples.
            sync_audition_output_gains(state);
            context.request_repaint();
        }
        Message::WaveformLoopDragStarted { .. } => {
            if state.busy || state.waveform_busy || state.waveform.is_none() {
                return;
            }
            disarm_audition_auto_advance(state);
            set_audition_source(state, AuditionSource::Main);
            state.loop_selections.clear(AuditionSource::Main);
            state.status = String::from("Paint a loop across the main waveform…");
            context.request_repaint();
        }
        Message::WaveformLoopDragMoved { .. } => {
            if !state.busy && !state.waveform_busy && state.waveform.is_some() {
                state.status = String::from("Selecting a main loop…");
                context.request_repaint();
            }
        }
        Message::WaveformLoopDragEnded {
            start_ratio,
            end_ratio,
        } => finish_loop_selection(state, context, AuditionSource::Main, start_ratio, end_ratio),
        Message::WaveformLoopDragCancelled => {
            state.loop_selections.clear(AuditionSource::Main);
            state.status = String::from("Loop selection canceled.");
            context.request_repaint();
        }
        Message::ReferenceLoopDragStarted { .. } => {
            if state.busy || state.reference_waveform_busy || state.waveform_busy {
                return;
            }
            if selected_reference_details(state).is_none() {
                return;
            }
            disarm_audition_auto_advance(state);
            set_audition_source(state, AuditionSource::Reference);
            state.loop_selections.clear(AuditionSource::Reference);
            state.status = String::from("Paint a loop across the reference waveform…");
            context.request_repaint();
        }
        Message::ReferenceLoopDragMoved { .. } => {
            if !state.busy
                && !state.waveform_busy
                && !state.reference_waveform_busy
                && selected_reference_details(state).is_some()
            {
                state.status = String::from("Selecting a reference loop…");
                context.request_repaint();
            }
        }
        Message::ReferenceLoopDragEnded {
            start_ratio,
            end_ratio,
        } => finish_loop_selection(
            state,
            context,
            AuditionSource::Reference,
            start_ratio,
            end_ratio,
        ),
        Message::ReferenceLoopDragCancelled => {
            state.loop_selections.clear(AuditionSource::Reference);
            state.status = String::from("Reference loop selection canceled.");
            context.request_repaint();
        }
        Message::ReferenceWaveformClicked { ratio } => {
            seek_reference_waveform_position(state, context, ratio, true)
        }
        Message::ReferenceCommentClicked { ratio } => {
            state.comment_source = CommentSource::Reference;
            state.comment_source_explicit = true;
            start_reference_comment_draft(state, context, ratio)
        }
        Message::WaveformClicked { ratio, lower } => {
            if state.busy || state.waveform_busy {
                return;
            }
            let Some(waveform) = state.waveform.as_ref() else {
                return;
            };
            let duration_millis = waveform.duration_millis;
            let time_millis = waveform::millis_for_ratio(ratio, duration_millis);
            if lower {
                state.comment_source = CommentSource::Main;
                state.comment_source_explicit = true;
            }
            state.hovered_note_id = None;
            if lower {
                start_main_note_draft(state, context, time_millis);
            } else {
                set_audition_source(state, AuditionSource::Main);
                state.loop_selections.clear(AuditionSource::Main);
                state.draft_note = None;
                state.selected_note_id = None;
                disarm_audition_auto_advance(state);
                seek_review_position(state, context, ratio, true);
            }
            context.request_repaint();
        }
        Message::WaveformPlayheadDragStarted { ratio } => {
            if state.busy || state.waveform_busy || state.waveform.is_none() {
                return;
            }
            rollback_persisted_note_drag(state);
            set_audition_source(state, AuditionSource::Main);
            state.loop_selections.clear(AuditionSource::Main);
            state.playhead_drag_active = true;
            state.draft_note = None;
            state.hovered_note_id = None;
            seek_review_position(state, context, ratio, false);
        }
        Message::WaveformPlayheadDragMoved { ratio } => {
            if !state.playhead_drag_active {
                return;
            }
            seek_review_position(state, context, ratio, false);
        }
        Message::WaveformPlayheadDragEnded { ratio } => {
            if !state.playhead_drag_active {
                return;
            }
            state.playhead_drag_active = false;
            seek_review_position(state, context, ratio, true);
        }
        Message::WaveformPlayheadDragCancelled => {
            state.playhead_drag_active = false;
            context.request_repaint();
        }
        Message::ReferencePlayheadDragStarted { ratio } => {
            if state.busy || state.waveform_busy || state.reference_waveform_busy {
                return;
            }
            if selected_reference_details(state).is_none() {
                return;
            }
            rollback_reference_persisted_note_drag(state);
            set_audition_source(state, AuditionSource::Reference);
            state.loop_selections.clear(AuditionSource::Reference);
            state.reference_playhead_drag_active = true;
            state.reference_draft_note = None;
            state.hovered_reference_note_id = None;
            seek_reference_waveform_position(state, context, ratio, false);
        }
        Message::ReferencePlayheadDragMoved { ratio } => {
            if !state.reference_playhead_drag_active {
                return;
            }
            seek_reference_waveform_position(state, context, ratio, false);
        }
        Message::ReferencePlayheadDragEnded { ratio } => {
            if !state.reference_playhead_drag_active {
                return;
            }
            state.reference_playhead_drag_active = false;
            seek_reference_waveform_position(state, context, ratio, true);
        }
        Message::ReferencePlayheadDragCancelled => {
            state.reference_playhead_drag_active = false;
            context.request_repaint();
        }
        Message::CommentDragStarted { ratio, note_index } => {
            if state.busy || state.waveform_busy {
                return;
            }
            if let Some(note_index) = note_index {
                start_persisted_note_drag(state, context, note_index);
            } else {
                rollback_persisted_note_drag(state);
                move_draft_note(state, context, ratio);
            }
        }
        Message::CommentDragMoved { ratio } => {
            if state.busy || state.waveform_busy {
                return;
            }
            if state.persisted_note_drag.is_some() {
                move_persisted_note(state, context, ratio);
            } else {
                move_draft_note(state, context, ratio);
            }
        }
        Message::CommentDragEnded { ratio } => {
            if state.busy || state.waveform_busy {
                return;
            }
            if state.persisted_note_drag.is_some() {
                finish_persisted_note_drag(state, context, ratio);
            } else {
                move_draft_note(state, context, ratio);
            }
        }
        Message::CommentDragCancelled => {
            if state.persisted_note_drag.is_some() {
                rollback_persisted_note_drag(state);
            } else {
                state.draft_note = None;
            }
            state.status = String::from("Comment canceled.");
            context.request_repaint();
        }
        Message::ReferenceCommentDragStarted { ratio, note_index } => {
            if state.busy || state.waveform_busy || state.reference_waveform_busy {
                return;
            }
            if let Some(note_index) = note_index {
                start_reference_persisted_note_drag(state, context, note_index);
            } else {
                rollback_reference_persisted_note_drag(state);
                move_reference_draft_note(state, context, ratio);
            }
        }
        Message::ReferenceCommentDragMoved { ratio } => {
            if state.busy || state.waveform_busy || state.reference_waveform_busy {
                return;
            }
            if state.reference_persisted_note_drag.is_some() {
                move_reference_persisted_note(state, context, ratio);
            } else {
                move_reference_draft_note(state, context, ratio);
            }
        }
        Message::ReferenceCommentDragEnded { ratio } => {
            if state.busy || state.waveform_busy || state.reference_waveform_busy {
                return;
            }
            if state.reference_persisted_note_drag.is_some() {
                finish_reference_persisted_note_drag(state, context, ratio);
            } else {
                move_reference_draft_note(state, context, ratio);
            }
        }
        Message::ReferenceCommentDragCancelled => {
            if state.reference_persisted_note_drag.is_some() {
                rollback_reference_persisted_note_drag(state);
            } else {
                state.reference_draft_note = None;
            }
            state.status = String::from("Reference comment canceled.");
            context.request_repaint();
        }
        Message::DraftNoteChanged(body) => {
            if let Some(draft) = state.draft_note.as_mut() {
                draft.body = body;
                context.request_repaint();
            }
        }
        Message::SaveDraftNote => save_draft_note(state, context),
        Message::CancelDraftNote => {
            state.draft_note = None;
            rollback_persisted_note_drag(state);
            state.status = String::from("Comment canceled.");
            context.request_repaint();
        }
        Message::SelectNote(id) => {
            if state.busy {
                return;
            }
            state.comment_source = CommentSource::Main;
            state.comment_source_explicit = true;
            rollback_persisted_note_drag(state);
            let note = selected_track(state)
                .and_then(|track| track.notes.iter().find(|note| note.id == id))
                .cloned();
            if let Some(note) = note {
                state.selected_note_id = Some(note.id);
                state.review_cursor_millis = note.time_millis;
                state.draft_note = None;
                state.status = format!(
                    "Selected comment at {}.",
                    format_timestamp(note.time_millis)
                );
                context.request_repaint();
            }
        }
        Message::CommentHoverStarted(id) => {
            if state.busy {
                return;
            }
            let is_current_note = selected_track(state)
                .is_some_and(|track| track.notes.iter().any(|note| note.id == id));
            if is_current_note && state.hovered_note_id.as_deref() != Some(id.as_str()) {
                state.hovered_note_id = Some(id);
                context.request_repaint();
            }
        }
        Message::CommentHoverEnded(id) => {
            if state.hovered_note_id.as_deref() == Some(id.as_str()) {
                state.hovered_note_id = None;
                context.request_repaint();
            }
        }
        Message::ReferenceCommentHoverStarted(id) => {
            if state.busy {
                return;
            }
            let is_current_note = selected_reference_notes(state)
                .iter()
                .any(|note| note.id == id);
            if is_current_note && state.hovered_reference_note_id.as_deref() != Some(id.as_str()) {
                state.hovered_reference_note_id = Some(id);
                context.request_repaint();
            }
        }
        Message::ReferenceCommentHoverEnded(id) => {
            if state.hovered_reference_note_id.as_deref() == Some(id.as_str()) {
                state.hovered_reference_note_id = None;
                context.request_repaint();
            }
        }
        Message::FocusCommentEditor(editor_id) => {
            context.focus(editor_id);
            context.request_repaint();
        }
        Message::EditNote(id) => {
            if state.busy {
                return;
            }
            state.comment_source = CommentSource::Main;
            state.comment_source_explicit = true;
            rollback_persisted_note_drag(state);
            let note = selected_track(state)
                .and_then(|track| track.notes.iter().find(|note| note.id == id))
                .cloned();
            if let Some(note) = note {
                let editor_id = main_inline_comment_editor_id(&note.id);
                state.review_cursor_millis = note.time_millis;
                state.selected_note_id = Some(note.id.clone());
                state.draft_note = Some(NoteDraft {
                    note_id: Some(note.id),
                    time_millis: note.time_millis,
                    body: note.body,
                });
                state.status =
                    format!("Editing comment at {}.", format_timestamp(note.time_millis));
                context.request_repaint();
                context.focus(editor_id);
                context.after(
                    Duration::from_millis(1),
                    Message::FocusCommentEditor(editor_id),
                );
            }
        }
        Message::ToggleNoteDone(id) => {
            if state.busy {
                return;
            }
            if let Some(track) = selected_track_mut(state)
                && let Some(note) = track.notes.iter_mut().find(|note| note.id == id)
            {
                note.done = !note.done;
                schedule_library_save(state, context);
                context.request_repaint();
            }
        }
        Message::DeleteNote(id) => {
            if state.busy {
                return;
            }
            rollback_persisted_note_drag(state);
            let removed = selected_track_mut(state).and_then(|track| {
                track
                    .notes
                    .iter()
                    .position(|note| note.id == id)
                    .map(|index| track.notes.remove(index))
            });
            if removed.is_some() {
                if state.hovered_note_id.as_deref() == Some(id.as_str()) {
                    state.hovered_note_id = None;
                }
                if state
                    .selected_note_id
                    .as_ref()
                    .is_some_and(|selected_id| selected_id == &id)
                {
                    state.selected_note_id = None;
                }
                if state
                    .draft_note
                    .as_ref()
                    .is_some_and(|draft| draft.note_id.as_deref() == Some(id.as_str()))
                {
                    state.draft_note = None;
                }
                state.status = String::from("Comment deleted locally.");
                schedule_library_save(state, context);
            } else {
                state.status = String::from("That comment no longer exists.");
            }
            context.request_repaint();
        }
        Message::ReferenceDraftNoteChanged(body) => {
            if let Some(draft) = state.reference_draft_note.as_mut() {
                draft.body = body;
                context.request_repaint();
            }
        }
        Message::SaveReferenceDraftNote => save_reference_draft_note(state, context),
        Message::CancelReferenceDraftNote => {
            state.reference_draft_note = None;
            state.status = String::from("Reference comment canceled.");
            context.request_repaint();
        }
        Message::SelectReferenceNote(id) => {
            if state.busy {
                return;
            }
            state.comment_source = CommentSource::Reference;
            state.comment_source_explicit = true;
            let note = selected_reference_notes(state)
                .iter()
                .find(|note| note.id == id)
                .cloned();
            if let Some(note) = note {
                state.selected_reference_note_id = Some(note.id);
                state.reference_draft_note = None;
                state.status = format!(
                    "Selected reference comment at {}.",
                    format_timestamp(note.time_millis)
                );
                context.request_repaint();
            }
        }
        Message::EditReferenceNote(id) => {
            if state.busy {
                return;
            }
            state.comment_source = CommentSource::Reference;
            state.comment_source_explicit = true;
            let note = selected_reference_notes(state)
                .iter()
                .find(|note| note.id == id)
                .cloned();
            if let Some(note) = note {
                let editor_id = reference_inline_comment_editor_id(&note.id);
                state.selected_reference_note_id = Some(note.id.clone());
                state.reference_draft_note = Some(NoteDraft {
                    note_id: Some(note.id),
                    time_millis: note.time_millis,
                    body: note.body,
                });
                state.status = format!(
                    "Editing reference comment at {}.",
                    format_timestamp(note.time_millis)
                );
                context.request_repaint();
                context.focus(editor_id);
                context.after(
                    Duration::from_millis(1),
                    Message::FocusCommentEditor(editor_id),
                );
            }
        }
        Message::ToggleReferenceNoteDone(id) => {
            if state.busy {
                return;
            }
            if let Some(note) = selected_reference_note_mut(state, &id) {
                note.done = !note.done;
                schedule_library_save(state, context);
                context.request_repaint();
            }
        }
        Message::DeleteReferenceNote(id) => {
            if state.busy {
                return;
            }
            let removed = selected_reference_track_mut(state).and_then(|reference| {
                reference
                    .notes
                    .iter()
                    .position(|note| note.id == id)
                    .map(|index| reference.notes.remove(index))
            });
            if removed.is_some() {
                if state.selected_reference_note_id.as_deref() == Some(id.as_str()) {
                    state.selected_reference_note_id = None;
                }
                if state.hovered_reference_note_id.as_deref() == Some(id.as_str()) {
                    state.hovered_reference_note_id = None;
                }
                if state
                    .reference_draft_note
                    .as_ref()
                    .is_some_and(|draft| draft.note_id.as_deref() == Some(id.as_str()))
                {
                    state.reference_draft_note = None;
                }
                state.status = String::from("Reference comment deleted locally.");
                schedule_library_save(state, context);
            } else {
                state.status = String::from("That reference comment no longer exists.");
            }
            context.request_repaint();
        }
    }
}

fn selected_reference_details(state: &AppState) -> Option<(PathBuf, u64)> {
    selected_track(state).and_then(|track| {
        let path = track.reference_path.clone()?;
        let duration_millis = state
            .reference_waveform
            .as_ref()
            .filter(|_| {
                !state.reference_waveform_busy
                    && state.reference_waveform_track_id.as_deref() == Some(track.id.as_str())
            })
            .map(|waveform| waveform.duration_millis)?;
        Some((path, duration_millis))
    })
}

fn selected_main_duration(state: &AppState) -> Option<u64> {
    state
        .waveform
        .as_ref()
        .filter(|_| {
            state.library.selected_track_id.as_deref() == state.waveform_track_id.as_deref()
        })
        .map(|waveform| waveform.duration_millis)
}

fn projected_loop_bounds(selection: LoopSelection, duration_millis: u64) -> Option<(u64, u64)> {
    let start_millis = waveform::millis_for_ratio(selection.start_ratio, duration_millis);
    let end_millis = waveform::millis_for_ratio(selection.end_ratio, duration_millis);
    (end_millis > start_millis).then_some((start_millis, end_millis))
}

fn selected_duration_for_source(state: &AppState, source: AuditionSource) -> Option<u64> {
    match source {
        AuditionSource::Main => selected_main_duration(state),
        AuditionSource::Reference => {
            selected_reference_details(state).map(|(_, duration)| duration)
        }
    }
}

fn loop_bounds_for_selection(
    state: &AppState,
    source: AuditionSource,
    selection: LoopSelection,
) -> Option<LoopBounds> {
    let (start_millis, end_millis) =
        projected_loop_bounds(selection, selected_duration_for_source(state, source)?)?;
    Some(LoopBounds {
        start_millis,
        end_millis,
    })
}

fn loop_bounds_for_source(state: &AppState, source: AuditionSource) -> Option<LoopBounds> {
    loop_bounds_for_selection(state, source, state.loop_selections.get(source)?)
}

#[cfg(test)]
fn loop_bounds(state: &AppState) -> Option<LoopBounds> {
    loop_bounds_for_source(state, state.audition_source)
}

fn loop_bounds_meet_minimum(bounds: LoopBounds) -> bool {
    bounds.end_millis.saturating_sub(bounds.start_millis) >= MIN_LOOP_MILLIS
}

fn seek_synchronized_positions(
    state: &mut AppState,
    main_position_millis: u64,
    reference_position_millis: u64,
    resume: bool,
) -> Result<(), String> {
    if state.waveform_busy || state.reference_waveform_busy {
        return Err(String::from("Audio analysis is still building."));
    }
    let Some(main_duration_millis) = state
        .waveform
        .as_ref()
        .filter(|_| {
            state.library.selected_track_id.as_deref() == state.waveform_track_id.as_deref()
        })
        .map(|waveform| waveform.duration_millis)
    else {
        return Err(String::from("Audio analysis is still pending."));
    };
    let reference_details = selected_reference_details(state);
    if !state.transport.has_command_capacity(1) {
        return Err(String::from(transport::CONTROLS_BUSY_ERROR));
    }
    let reference_was_loaded = state.reference_transport_loaded;
    if reference_details.is_some() {
        let reference_transport = state
            .reference_transport
            .get_or_insert_with(transport::AudioTransport::spawn);
        if reference_transport.has_pending_load() {
            return Err(String::from(transport::CONTROLS_BUSY_ERROR));
        }
        let required_slots = if reference_was_loaded { 1 } else { 2 };
        if !reference_transport.has_command_capacity(required_slots) {
            return Err(String::from(transport::CONTROLS_BUSY_ERROR));
        }
    }
    let main_token = state.transport.seek(
        state.transport_generation,
        main_position_millis,
        main_duration_millis,
        resume,
    )?;
    begin_transport_polling(state, main_token);
    if let Some((reference_path, reference_duration_millis)) = reference_details {
        let reference_gain = reference_output_gain(state);
        let reference_transport = state
            .reference_transport
            .get_or_insert_with(transport::AudioTransport::spawn);
        reference_transport.set_output_gain(reference_gain);
        if !reference_was_loaded {
            reference_transport.load(
                state.reference_transport_generation,
                reference_path,
                reference_duration_millis,
            )?;
            if reference_transport.has_pending_load() {
                return Err(String::from(transport::CONTROLS_BUSY_ERROR));
            }
            state.reference_transport_loaded = true;
        }
        let reference_token = reference_transport.seek(
            state.reference_transport_generation,
            reference_position_millis,
            reference_duration_millis,
            resume,
        )?;
        state.reference_transport_waiting_token = Some(reference_token);
        state.reference_transport_polling = true;
    }
    state.reference_only_playback = false;
    state.transport_position_millis = main_position_millis;
    state.review_cursor_millis = main_position_millis;
    state.reference_transport_position_millis = reference_position_millis;
    Ok(())
}

fn resume_main_at_position_with_reference(
    state: &mut AppState,
    main_position_millis: u64,
) -> Result<(), String> {
    let reference_position_millis = selected_reference_details(state)
        .map(|(_, duration_millis)| {
            state
                .reference_transport_position_millis
                .min(duration_millis)
        })
        .unwrap_or(state.reference_transport_position_millis);
    seek_synchronized_positions(state, main_position_millis, reference_position_millis, true)
}

fn seek_reference_waveform_position(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    ratio: f32,
    resume: bool,
) {
    if state.busy || state.waveform_busy || state.reference_waveform_busy {
        return;
    }
    let Some((path, reference_duration_millis)) = selected_reference_details(state) else {
        return;
    };
    state.loop_selections.clear(AuditionSource::Reference);
    disarm_audition_auto_advance(state);
    set_audition_source(state, AuditionSource::Reference);
    if let Some(reference_transport) = state.reference_transport.as_ref()
        && !state.reference_transport_loaded
        && reference_transport.has_pending_load()
    {
        state.status = String::from(transport::CONTROLS_BUSY_ERROR);
        context.request_repaint();
        return;
    }
    let ratio = waveform::clamp_ratio(ratio);
    let reference_position_millis = waveform::millis_for_ratio(ratio, reference_duration_millis);
    state.draft_note = None;
    rollback_persisted_note_drag(state);
    state.selected_note_id = None;
    if !resume {
        state.reference_transport_position_millis = reference_position_millis;
        state.reference_only_playback = false;
        state.status = format!(
            "Scrubbing reference at {}.",
            format_timestamp(reference_position_millis)
        );
        context.request_repaint();
        return;
    }
    let reference_gain = reference_output_gain(state);
    let reference_transport = state
        .reference_transport
        .get_or_insert_with(transport::AudioTransport::spawn);
    reference_transport.set_output_gain(reference_gain);
    if !state.reference_transport_loaded {
        if let Err(error) = reference_transport.load(
            state.reference_transport_generation,
            path,
            reference_duration_millis,
        ) {
            state.status = error;
            context.request_repaint();
            return;
        }
        if reference_transport.has_pending_load() {
            state.status = String::from(transport::CONTROLS_BUSY_ERROR);
            context.request_repaint();
            return;
        }
        state.reference_transport_loaded = true;
    }
    match reference_transport.seek(
        state.reference_transport_generation,
        reference_position_millis,
        reference_duration_millis,
        true,
    ) {
        Ok(token) => {
            state.reference_transport_position_millis = reference_position_millis;
            state.reference_transport_waiting_token = Some(token);
            state.reference_transport_polling = true;
            state.reference_only_playback = true;
            state.status = format!(
                "Playing reference from {}.",
                format_timestamp(reference_position_millis)
            );
        }
        Err(error) => state.status = error,
    }
    context.request_repaint();
}

fn source_transport_is_active(state: &AppState, source: AuditionSource) -> bool {
    match source {
        AuditionSource::Main => {
            state.transport_playing
                || state.transport_polling
                || state.transport_waiting_token.is_some()
        }
        AuditionSource::Reference => {
            state.reference_transport_playing
                || state.reference_transport_polling
                || state.reference_transport_waiting_token.is_some()
        }
    }
}

fn set_source_position(state: &mut AppState, source: AuditionSource, position_millis: u64) {
    match source {
        AuditionSource::Main => {
            state.transport_position_millis = position_millis;
            state.review_cursor_millis = position_millis;
        }
        AuditionSource::Reference => {
            state.reference_transport_position_millis = position_millis;
        }
    }
}

fn seek_loop_owner(
    state: &mut AppState,
    source: AuditionSource,
    bounds: LoopBounds,
) -> Result<(), String> {
    let duration_millis = selected_duration_for_source(state, source)
        .ok_or_else(|| String::from("Audio analysis is still pending."))?;
    let position_millis = bounds.start_millis.min(duration_millis);
    match source {
        AuditionSource::Main => {
            if state.transport_polling
                || state.transport_waiting_token.is_some()
                || state.transport.has_pending_load()
                || !state.transport.has_command_capacity(1)
            {
                return Err(String::from(transport::CONTROLS_BUSY_ERROR));
            }
            let token = state.transport.seek(
                state.transport_generation,
                position_millis,
                duration_millis,
                true,
            )?;
            set_source_position(state, source, position_millis);
            begin_transport_polling(state, token);
        }
        AuditionSource::Reference => {
            let Some(reference_transport) = state.reference_transport.as_ref() else {
                return Err(String::from(transport::CONTROLS_BUSY_ERROR));
            };
            if !state.reference_transport_loaded
                || state.reference_transport_polling
                || state.reference_transport_waiting_token.is_some()
                || reference_transport.has_pending_load()
                || !reference_transport.has_command_capacity(1)
            {
                return Err(String::from(transport::CONTROLS_BUSY_ERROR));
            }
            let token = reference_transport.seek(
                state.reference_transport_generation,
                position_millis,
                duration_millis,
                true,
            )?;
            set_source_position(state, source, position_millis);
            state.reference_transport_waiting_token = Some(token);
            state.reference_transport_polling = true;
        }
    }
    Ok(())
}

fn finish_loop_selection(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    owner: AuditionSource,
    start_ratio: f32,
    end_ratio: f32,
) {
    if state.busy
        || state.waveform_busy
        || (owner == AuditionSource::Reference && state.reference_waveform_busy)
    {
        return;
    }
    if owner == AuditionSource::Reference && selected_reference_details(state).is_none() {
        return;
    }
    disarm_audition_auto_advance(state);
    set_audition_source(state, owner);
    let start_ratio = waveform::clamp_ratio(start_ratio);
    let end_ratio = waveform::clamp_ratio(end_ratio);
    let (start_ratio, end_ratio) = if start_ratio <= end_ratio {
        (start_ratio, end_ratio)
    } else {
        (end_ratio, start_ratio)
    };
    let candidate_selection = LoopSelection {
        start_ratio,
        end_ratio,
    };
    let Some(bounds) = loop_bounds_for_selection(state, owner, candidate_selection) else {
        state.loop_selections.clear(owner);
        state.status = String::from("Loop cleared.");
        context.request_repaint();
        return;
    };
    if !loop_bounds_meet_minimum(bounds) {
        state.loop_selections.clear(owner);
        state.status = String::from("Loop cleared — select at least 120 ms.");
        context.request_repaint();
        return;
    }

    let owner_was_active = source_transport_is_active(state, owner);
    if owner_was_active && let Err(error) = seek_loop_owner(state, owner, bounds) {
        state.status = error;
        context.request_repaint();
        return;
    }
    state.loop_selections.set(owner, Some(candidate_selection));
    if !owner_was_active {
        set_source_position(state, owner, bounds.start_millis);
    }
    let label = match owner {
        AuditionSource::Main => "Main",
        AuditionSource::Reference => "Reference",
    };
    state.status = format!(
        "{label} loop {}–{}.",
        format_timestamp(bounds.start_millis),
        format_timestamp(bounds.end_millis),
    );
    context.request_repaint();
}

fn select_audition_source(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    source: AuditionSource,
) {
    if state.busy {
        return;
    }
    if source == AuditionSource::Reference {
        if state.reference_waveform_busy {
            state.status = String::from("Reference analysis is still pending.");
            context.request_repaint();
            return;
        }
        if selected_reference_details(state).is_none() {
            state.status = String::from("Import and analyze a reference track first.");
            context.request_repaint();
            return;
        }
    }
    if state.audition_source != source {
        set_audition_source(state, source);
        state.status = match source {
            AuditionSource::Main => String::from("Now hearing the imported track."),
            AuditionSource::Reference => String::from("Now hearing the reference track."),
        };
    }
    context.request_repaint();
}

fn set_audition_source(state: &mut AppState, source: AuditionSource) {
    if state.audition_source == source {
        return;
    }
    state.audition_source = source;
    sync_audition_output_gains(state);
}

fn play_audition(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    if state.busy || state.workspace_mode != WorkspaceMode::Audition {
        return;
    }
    if state.transport_playing || state.reference_transport_playing {
        state.status = String::from("Audition playback is already active.");
        context.request_repaint();
        return;
    }
    let Some(track_id) = state.library.selected_track_id.clone() else {
        state.status = String::from("Select an audition track before playing.");
        context.request_repaint();
        return;
    };
    if !state
        .audition_queue
        .iter()
        .any(|queued_id| queued_id == &track_id)
    {
        state.status = String::from("Select an audition track before playing.");
        context.request_repaint();
        return;
    }

    let waveform_ready = !state.waveform_busy
        && state.waveform_track_id.as_deref() == Some(track_id.as_str())
        && state.waveform.is_some();
    let transport_pending = state.transport_polling
        || state.transport_waiting_token.is_some()
        || state.transport.has_pending_load()
        || state.reference_transport_polling
        || state.reference_transport_waiting_token.is_some()
        || state
            .reference_transport
            .as_ref()
            .is_some_and(transport::AudioTransport::has_pending_load);
    if !waveform_ready || state.reference_waveform_busy || transport_pending {
        state.audition_auto_advance = true;
        state.audition_play_token = None;
        state.audition_pending_play_track_id = Some(track_id.clone());
        let title = state
            .library
            .tracks
            .iter()
            .find(|track| track.id == track_id)
            .map_or(track_id, |track| track.title.clone());
        state.status = format!("Loading audition track: {title}…");
        context.request_repaint();
        return;
    }

    // The existing toggle path owns paired main/reference admission. Calling it
    // only after the active-playback guard makes this a one-way Play command.
    state.audition_pending_play_track_id = None;
    toggle_playback(state, context);
}

fn source_position(state: &AppState, source: AuditionSource) -> u64 {
    match source {
        AuditionSource::Main => state.transport_position_millis,
        AuditionSource::Reference => state.reference_transport_position_millis,
    }
}

fn playback_start_position(
    state: &AppState,
    source: AuditionSource,
    duration_millis: u64,
    loop_bounds: Option<LoopBounds>,
) -> u64 {
    let stored_position_millis = source_position(state, source);
    if let Some(bounds) = loop_bounds {
        if stored_position_millis >= bounds.start_millis
            && stored_position_millis < bounds.end_millis
        {
            stored_position_millis.min(duration_millis)
        } else {
            bounds.start_millis.min(duration_millis)
        }
    } else if stored_position_millis >= duration_millis {
        0
    } else {
        stored_position_millis
    }
}

fn previous_audition(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    move_audition(state, context, AuditionMove::Previous);
}

fn next_audition(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    move_audition(state, context, AuditionMove::Next);
}

#[derive(Clone, Copy)]
enum AuditionMove {
    Previous,
    Next,
}

fn move_audition(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    direction: AuditionMove,
) {
    if state.busy || state.workspace_mode != WorkspaceMode::Audition {
        return;
    }
    let Some(current_id) = state.library.selected_track_id.clone() else {
        state.status = match direction {
            AuditionMove::Previous => {
                String::from("Already at the beginning of the audition queue.")
            }
            AuditionMove::Next => String::from("Already at the end of the audition queue."),
        };
        context.request_repaint();
        return;
    };
    let Some(current_index) = state
        .audition_queue
        .iter()
        .position(|track_id| track_id == &current_id)
    else {
        state.status = match direction {
            AuditionMove::Previous => {
                String::from("Already at the beginning of the audition queue.")
            }
            AuditionMove::Next => String::from("Already at the end of the audition queue."),
        };
        context.request_repaint();
        return;
    };
    let destination_index = match direction {
        AuditionMove::Previous => current_index.checked_sub(1),
        AuditionMove::Next => current_index
            .checked_add(1)
            .filter(|index| *index < state.audition_queue.len()),
    };
    let Some(destination_index) = destination_index else {
        state.status = match direction {
            AuditionMove::Previous => {
                String::from("Already at the beginning of the audition queue.")
            }
            AuditionMove::Next => String::from("Already at the end of the audition queue."),
        };
        context.request_repaint();
        return;
    };
    let Some(destination_id) = state.audition_queue.get(destination_index).cloned() else {
        return;
    };

    if matches!(direction, AuditionMove::Next)
        && !state
            .audition_heard
            .iter()
            .any(|heard_id| heard_id == &current_id)
    {
        state.audition_heard.push(current_id);
    }
    state
        .audition_heard
        .retain(|heard_id| heard_id != &destination_id);
    let title = state
        .library
        .tracks
        .iter()
        .find(|track| track.id == destination_id)
        .map_or_else(|| destination_id.clone(), |track| track.title.clone());
    select_track_internal(state, context, destination_id, true);
    state.status = match direction {
        AuditionMove::Previous => format!("Loading previous audition track: {title}…"),
        AuditionMove::Next => format!("Loading next audition track: {title}…"),
    };
    context.request_repaint();
}

fn stop_playback(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    state.audition_auto_advance = false;
    state.audition_play_token = None;
    state.audition_pending_play_track_id = None;
    let main_active = state.transport_playing
        || state.transport_polling
        || state.transport_waiting_token.is_some();
    let reference_active = state.reference_transport.as_ref().is_some_and(|_| {
        state.reference_transport_playing
            || state.reference_transport_polling
            || state.reference_transport_waiting_token.is_some()
    });
    if !main_active && !reference_active {
        return;
    }

    let reference_capacity_available = !reference_active
        || state.reference_transport.as_ref().is_some_and(|transport| {
            !transport.has_pending_load() && transport.has_command_capacity(1)
        });
    if (main_active && !state.transport.has_command_capacity(1)) || !reference_capacity_available {
        state.status = String::from(transport::CONTROLS_BUSY_ERROR);
        context.request_repaint();
        return;
    }

    let main_token = if main_active {
        match state.transport.pause(state.transport_generation) {
            Ok(token) => Some(token),
            Err(error) => {
                state.status = error;
                context.request_repaint();
                return;
            }
        }
    } else {
        None
    };
    let reference_token = if reference_active {
        let Some(reference_transport) = state.reference_transport.as_ref() else {
            state.status = String::from(transport::CONTROLS_BUSY_ERROR);
            context.request_repaint();
            return;
        };
        match reference_transport.pause(state.reference_transport_generation) {
            Ok(token) => Some(token),
            Err(error) => {
                if let Some(token) = main_token {
                    begin_transport_polling(state, token);
                }
                state.status = error;
                context.request_repaint();
                return;
            }
        }
    } else {
        None
    };

    if let Some(token) = main_token {
        begin_transport_polling(state, token);
    }
    if let Some(token) = reference_token {
        state.reference_transport_waiting_token = Some(token);
        state.reference_transport_polling = true;
    }
    state.reference_only_playback = false;
    state.status = String::from("Stopping playback…");
    context.request_repaint();
}

fn toggle_playback(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    if state.busy || state.waveform_busy || state.reference_waveform_busy {
        if state.waveform_busy || state.reference_waveform_busy {
            state.status = String::from("Audio analysis is still building.");
            context.request_repaint();
        }
        return;
    }
    if state.transport_polling || state.reference_transport_polling {
        return;
    }
    let Some(waveform) = state
        .waveform
        .as_ref()
        .filter(|_| selected_track(state).is_some())
    else {
        state.status = String::from("Audio analysis is still pending.");
        context.request_repaint();
        return;
    };

    let reference_details = selected_reference_details(state);
    if let Some(reference_transport) = state.reference_transport.as_ref()
        && !state.reference_transport_loaded
        && reference_transport.has_pending_load()
    {
        state.status = String::from(transport::CONTROLS_BUSY_ERROR);
        context.request_repaint();
        return;
    }
    let reference_pending = selected_track(state)
        .and_then(|track| track.reference_path.as_ref())
        .is_some()
        && state.reference_waveform_busy;
    if !state.transport_playing && !state.reference_transport_playing && reference_pending {
        state.status = String::from("Reference analysis is still pending.");
        context.request_repaint();
        return;
    }

    if state.transport_playing || state.reference_transport_playing {
        if state.workspace_mode == WorkspaceMode::Audition {
            state.audition_auto_advance = false;
            state.audition_play_token = None;
        }
        let reference_will_pause =
            state.reference_transport_loaded && state.reference_transport.is_some();
        let reference_capacity_available =
            state
                .reference_transport
                .as_ref()
                .is_none_or(|reference_transport| {
                    !reference_will_pause
                        || (!reference_transport.has_pending_load()
                            && reference_transport.has_command_capacity(1))
                });
        if !state.transport.has_command_capacity(1) || !reference_capacity_available {
            state.status = String::from(transport::CONTROLS_BUSY_ERROR);
            context.request_repaint();
            return;
        }
        let main_result = state.transport.pause(state.transport_generation);
        let reference_result = state
            .reference_transport
            .as_ref()
            .filter(|_| state.reference_transport_loaded)
            .map(|reference_transport| {
                reference_transport.pause(state.reference_transport_generation)
            });
        match main_result {
            Ok(token) => begin_transport_polling(state, token),
            Err(error) => state.status = error,
        }
        if let Some(result) = reference_result {
            match result {
                Ok(token) => {
                    state.reference_transport_waiting_token = Some(token);
                    state.reference_transport_polling = true;
                }
                Err(error) => state.status = error,
            }
        }
        state.status = String::from("Pausing playback…");
        state.reference_only_playback = false;
    } else {
        let duration_millis = waveform.duration_millis;
        let main_loop_bounds = loop_bounds_for_source(state, AuditionSource::Main);
        let reference_loop_bounds = loop_bounds_for_source(state, AuditionSource::Reference);
        let main_position_millis = playback_start_position(
            state,
            AuditionSource::Main,
            duration_millis,
            main_loop_bounds,
        );
        let reference_position_millis = reference_details
            .as_ref()
            .map(|(_, duration_millis)| {
                playback_start_position(
                    state,
                    AuditionSource::Reference,
                    *duration_millis,
                    reference_loop_bounds,
                )
            })
            .unwrap_or(state.reference_transport_position_millis);
        if !state.transport.has_command_capacity(1) {
            state.status = String::from(transport::CONTROLS_BUSY_ERROR);
            context.request_repaint();
            return;
        }
        if reference_details.is_some() {
            let reference_transport = state
                .reference_transport
                .get_or_insert_with(transport::AudioTransport::spawn);
            if reference_transport.has_pending_load() {
                state.status = String::from(transport::CONTROLS_BUSY_ERROR);
                context.request_repaint();
                return;
            }
            let required_reference_slots = if state.reference_transport_loaded {
                2
            } else {
                3
            };
            if !reference_transport.has_command_capacity(required_reference_slots) {
                state.status = String::from(transport::CONTROLS_BUSY_ERROR);
                context.request_repaint();
                return;
            }
        }
        state.reference_only_playback = false;

        let main_result = if main_loop_bounds.is_some()
            || main_position_millis != state.transport_position_millis
        {
            state.transport.seek(
                state.transport_generation,
                main_position_millis,
                duration_millis,
                true,
            )
        } else {
            state.transport.play(state.transport_generation)
        };
        let main_token = match main_result {
            Ok(token) => token,
            Err(error) => {
                if state.workspace_mode == WorkspaceMode::Audition {
                    state.audition_auto_advance = false;
                    state.audition_play_token = None;
                }
                state.status = error;
                context.request_repaint();
                return;
            }
        };
        set_source_position(state, AuditionSource::Main, main_position_millis);
        if state.workspace_mode == WorkspaceMode::Audition {
            state.audition_auto_advance = true;
            state.audition_play_token = Some(main_token);
        }
        begin_transport_polling(state, main_token);

        if let Some((path, reference_duration_millis)) = reference_details {
            let reference_gain = reference_output_gain(state);
            let reference_transport = state
                .reference_transport
                .get_or_insert_with(transport::AudioTransport::spawn);
            reference_transport.set_output_gain(reference_gain);
            if !state.reference_transport_loaded {
                if let Err(error) = reference_transport.load(
                    state.reference_transport_generation,
                    path,
                    reference_duration_millis,
                ) {
                    state.status = error;
                    context.request_repaint();
                    return;
                }
                if reference_transport.has_pending_load() {
                    state.status = String::from(transport::CONTROLS_BUSY_ERROR);
                    context.request_repaint();
                    return;
                }
                state.reference_transport_loaded = true;
            }
            if let Err(error) = reference_transport.seek(
                state.reference_transport_generation,
                reference_position_millis,
                reference_duration_millis,
                false,
            ) {
                state.status = error;
                context.request_repaint();
                return;
            }
            let reference_token =
                match reference_transport.play(state.reference_transport_generation) {
                    Ok(token) => token,
                    Err(error) => {
                        state.status = error;
                        context.request_repaint();
                        return;
                    }
                };
            set_source_position(state, AuditionSource::Reference, reference_position_millis);
            state.reference_transport_waiting_token = Some(reference_token);
            state.reference_transport_polling = true;
            state.status = if state.audition_source == AuditionSource::Reference {
                String::from("Playing reference and imported track…")
            } else {
                String::from("Playing imported and reference tracks…")
            };
        } else {
            state.status = String::from("Playing imported track…");
        }
    }
    context.request_repaint();
}

fn start_note_at_current_time(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    if state.busy || state.waveform_busy {
        return;
    }
    let Some(track_id) = state.library.selected_track_id.as_deref() else {
        state.status = String::from("Select a track before adding a comment.");
        context.request_repaint();
        return;
    };
    let Some(duration_millis) = state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(track_id))
        .map(|waveform| waveform.duration_millis)
    else {
        state.status = String::from("Audio analysis is still pending.");
        context.request_repaint();
        return;
    };
    let time_millis = state.review_cursor_millis.min(duration_millis);
    start_main_note_draft(state, context, time_millis);
}

fn start_main_note_draft(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    time_millis: u64,
) {
    rollback_persisted_note_drag(state);
    state.comment_source = CommentSource::Main;
    state.comment_source_explicit = true;
    set_audition_source(state, AuditionSource::Main);
    state.review_cursor_millis = time_millis;
    state.draft_note = Some(NoteDraft {
        note_id: None,
        time_millis,
        body: String::new(),
    });
    state.selected_note_id = None;
    state.status = format!(
        "Comment at {} — type a note below.",
        format_timestamp(time_millis)
    );
    context.focus(MAIN_COMMENT_EDITOR_ID);
    context.request_repaint();
}

fn save_draft_note(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    if state.busy {
        state.status = String::from("Finish importing before saving a comment.");
        context.request_repaint();
        return;
    }
    let Some(draft) = state.draft_note.clone() else {
        return;
    };
    let body = draft.body.trim().to_string();
    if body.is_empty() {
        state.status = String::from("Write a comment before saving.");
        context.request_repaint();
        return;
    }
    let Some(track) = selected_track_mut(state) else {
        state.status = String::from("Select a track before saving a comment.");
        context.request_repaint();
        return;
    };
    if let Some(note_id) = draft.note_id {
        if let Some(note) = track.notes.iter_mut().find(|note| note.id == note_id) {
            note.body = body;
        } else {
            state.status = String::from("That comment no longer exists.");
            context.request_repaint();
            return;
        }
    } else {
        track.notes.push(storage::Note {
            id: unique_note_id(),
            time_millis: draft.time_millis,
            body,
            done: false,
        });
        track.notes.sort_by_key(|note| note.time_millis);
    }
    state.draft_note = None;
    state.status = String::from("Comment saved locally.");
    schedule_library_save(state, context);
    context.request_repaint();
}

fn start_reference_comment_draft(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    ratio: f32,
) {
    if state.busy || state.reference_waveform_busy {
        return;
    }
    let Some(waveform) = state
        .reference_waveform
        .as_ref()
        .filter(|_| !state.reference_waveform_busy)
    else {
        return;
    };
    if selected_track(state)
        .and_then(|track| track.reference_path.as_ref())
        .is_none()
    {
        return;
    }
    let time_millis = waveform::millis_for_ratio(ratio, waveform.duration_millis);
    state.reference_draft_note = Some(NoteDraft {
        note_id: None,
        time_millis,
        body: String::new(),
    });
    state.selected_reference_note_id = None;
    state.hovered_reference_note_id = None;
    state.status = format!(
        "Reference comment at {} — type a note below.",
        format_timestamp(time_millis)
    );
    context.focus(REFERENCE_COMMENT_EDITOR_ID);
    context.request_repaint();
}

fn save_reference_draft_note(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    if state.busy {
        state.status = String::from("Finish importing before saving a reference comment.");
        context.request_repaint();
        return;
    }
    let Some(draft) = state.reference_draft_note.clone() else {
        return;
    };
    let body = draft.body.trim().to_owned();
    if body.is_empty() {
        state.status = String::from("Write a reference comment before saving.");
        context.request_repaint();
        return;
    }
    let Some(reference) = selected_reference_track_mut(state) else {
        state.status = String::from("Select a reference track before saving a comment.");
        context.request_repaint();
        return;
    };
    if let Some(note_id) = draft.note_id {
        if let Some(note) = reference.notes.iter_mut().find(|note| note.id == note_id) {
            note.body = body;
        } else {
            state.status = String::from("That reference comment no longer exists.");
            context.request_repaint();
            return;
        }
    } else {
        reference.notes.push(storage::Note {
            id: unique_note_id(),
            time_millis: draft.time_millis,
            body,
            done: false,
        });
        reference.notes.sort_by_key(|note| note.time_millis);
    }
    state.reference_draft_note = None;
    state.status = String::from("Reference comment saved locally.");
    schedule_library_save(state, context);
    context.request_repaint();
}

fn move_draft_note(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>, ratio: f32) {
    let Some(duration_millis) = state
        .waveform
        .as_ref()
        .map(|waveform| waveform.duration_millis)
    else {
        return;
    };
    let Some(draft) = state
        .draft_note
        .as_mut()
        .filter(|draft| draft.note_id.is_none())
    else {
        return;
    };
    let time_millis = waveform::millis_for_ratio(ratio, duration_millis);
    draft.time_millis = time_millis;
    state.review_cursor_millis = time_millis;
    state.status = format!("Comment at {}.", format_timestamp(time_millis));
    context.request_repaint();
}

fn start_persisted_note_drag(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    note_index: usize,
) {
    rollback_persisted_note_drag(state);
    if state.busy {
        return;
    }
    let Some(track_id) = state.library.selected_track_id.clone() else {
        return;
    };
    let waveform_is_current =
        state.waveform.is_some() && state.waveform_track_id.as_deref() == Some(track_id.as_str());
    let Some((note_id, time_millis)) = state
        .library
        .tracks
        .iter()
        .find(|track| track.id == track_id)
        .filter(|_| waveform_is_current)
        .and_then(|track| {
            track
                .notes
                .get(note_index)
                .map(|note| (note.id.clone(), note.time_millis))
        })
    else {
        state.status = String::from("That comment no longer exists.");
        context.request_repaint();
        return;
    };

    if state
        .draft_note
        .as_ref()
        .is_some_and(|draft| draft.note_id.as_deref() == Some(note_id.as_str()))
    {
        if let Some(draft) = state.draft_note.as_mut() {
            draft.time_millis = time_millis;
        }
    } else {
        // Picking up a saved marker supersedes an empty draft or an editor for
        // another note; it must never create a second draft for this marker.
        state.draft_note = None;
    }
    state.persisted_note_drag = Some(PersistedNoteDrag {
        track_id,
        note_id: note_id.clone(),
        original_time_millis: time_millis,
        moved: false,
    });
    state.selected_note_id = Some(note_id);
    state.hovered_note_id = None;
    state.review_cursor_millis = time_millis;
    state.status = format!("Dragging comment at {}…", format_timestamp(time_millis));
    context.request_repaint();
}

fn move_persisted_note(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    ratio: f32,
) {
    let Some(drag) = state.persisted_note_drag.clone() else {
        return;
    };
    let Some(duration_millis) = state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(drag.track_id.as_str()))
        .map(|waveform| waveform.duration_millis)
    else {
        rollback_persisted_note_drag(state);
        return;
    };
    let time_millis = waveform::millis_for_ratio(ratio, duration_millis);
    let note_exists = state
        .library
        .tracks
        .iter_mut()
        .find(|track| track.id == drag.track_id)
        .and_then(|track| track.notes.iter_mut().find(|note| note.id == drag.note_id))
        .map(|note| note.time_millis = time_millis)
        .is_some();
    if !note_exists {
        rollback_persisted_note_drag(state);
        state.status = String::from("That comment no longer exists.");
        context.request_repaint();
        return;
    }
    if let Some(active_drag) = state.persisted_note_drag.as_mut() {
        active_drag.moved |= time_millis != active_drag.original_time_millis;
    }
    if let Some(draft) = state
        .draft_note
        .as_mut()
        .filter(|draft| draft.note_id.as_deref() == Some(drag.note_id.as_str()))
    {
        draft.time_millis = time_millis;
    }
    state.review_cursor_millis = time_millis;
    state.status = format!("Comment at {}.", format_timestamp(time_millis));
    context.request_repaint();
}

fn finish_persisted_note_drag(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    ratio: f32,
) {
    let Some(drag) = state.persisted_note_drag.take() else {
        return;
    };
    let Some(duration_millis) = state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(drag.track_id.as_str()))
        .map(|waveform| waveform.duration_millis)
    else {
        return;
    };
    let time_millis = waveform::millis_for_ratio(ratio, duration_millis);
    let Some(track) = state
        .library
        .tracks
        .iter_mut()
        .find(|track| track.id == drag.track_id)
    else {
        state.status = String::from("That track is no longer in the library.");
        context.request_repaint();
        return;
    };
    let Some(note) = track.notes.iter_mut().find(|note| note.id == drag.note_id) else {
        state.status = String::from("That comment no longer exists.");
        context.request_repaint();
        return;
    };
    if drag.moved {
        note.time_millis = time_millis;
    }
    let final_time_millis = note.time_millis;
    let changed = final_time_millis != drag.original_time_millis;
    track.notes.sort_by_key(|note| note.time_millis);
    if let Some(draft) = state
        .draft_note
        .as_mut()
        .filter(|draft| draft.note_id.as_deref() == Some(drag.note_id.as_str()))
    {
        draft.time_millis = final_time_millis;
    }
    state.review_cursor_millis = final_time_millis;
    state.status = if changed {
        format!(
            "Comment moved to {} and saved locally.",
            format_timestamp(final_time_millis)
        )
    } else {
        format!(
            "Selected comment at {}.",
            format_timestamp(final_time_millis)
        )
    };
    if changed {
        schedule_library_save(state, context);
    }
    context.request_repaint();
}

fn move_reference_draft_note(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    ratio: f32,
) {
    let Some(duration_millis) = state
        .reference_waveform
        .as_ref()
        .filter(|_| !state.reference_waveform_busy)
        .map(|waveform| waveform.duration_millis)
    else {
        return;
    };
    let Some(draft) = state
        .reference_draft_note
        .as_mut()
        .filter(|draft| draft.note_id.is_none())
    else {
        return;
    };
    let time_millis = waveform::millis_for_ratio(ratio, duration_millis);
    draft.time_millis = time_millis;
    state.reference_transport_position_millis = time_millis;
    state.status = format!("Reference comment at {}.", format_timestamp(time_millis));
    context.request_repaint();
}

fn start_reference_persisted_note_drag(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    note_index: usize,
) {
    rollback_reference_persisted_note_drag(state);
    if state.busy {
        return;
    }
    let Some(track_id) = state.library.selected_track_id.clone() else {
        return;
    };
    let waveform_is_current = state.reference_waveform.is_some()
        && state.reference_waveform_track_id.as_deref() == Some(track_id.as_str())
        && !state.reference_waveform_busy;
    let Some((note_id, time_millis)) = state
        .library
        .tracks
        .iter()
        .find(|track| track.id == track_id)
        .filter(|_| waveform_is_current)
        .and_then(|track| {
            reference_notes_for_track(&state.library, track)
                .get(note_index)
                .map(|note| (note.id.clone(), note.time_millis))
        })
    else {
        state.status = String::from("That reference comment no longer exists.");
        context.request_repaint();
        return;
    };

    if state
        .reference_draft_note
        .as_ref()
        .is_some_and(|draft| draft.note_id.as_deref() == Some(note_id.as_str()))
    {
        if let Some(draft) = state.reference_draft_note.as_mut() {
            draft.time_millis = time_millis;
        }
    } else {
        state.reference_draft_note = None;
    }
    state.reference_persisted_note_drag = Some(PersistedNoteDrag {
        track_id,
        note_id: note_id.clone(),
        original_time_millis: time_millis,
        moved: false,
    });
    state.selected_reference_note_id = Some(note_id);
    state.hovered_reference_note_id = None;
    state.reference_transport_position_millis = time_millis;
    state.status = format!(
        "Dragging reference comment at {}…",
        format_timestamp(time_millis)
    );
    context.request_repaint();
}

fn move_reference_persisted_note(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    ratio: f32,
) {
    let Some(drag) = state.reference_persisted_note_drag.clone() else {
        return;
    };
    let Some(duration_millis) = state
        .reference_waveform
        .as_ref()
        .filter(|_| {
            !state.reference_waveform_busy
                && state.reference_waveform_track_id.as_deref() == Some(drag.track_id.as_str())
        })
        .map(|waveform| waveform.duration_millis)
    else {
        rollback_reference_persisted_note_drag(state);
        return;
    };
    let time_millis = waveform::millis_for_ratio(ratio, duration_millis);
    let note_exists = selected_reference_track_mut(state)
        .and_then(|reference| {
            reference
                .notes
                .iter_mut()
                .find(|note| note.id == drag.note_id)
        })
        .map(|note| note.time_millis = time_millis)
        .is_some();
    if !note_exists {
        rollback_reference_persisted_note_drag(state);
        state.status = String::from("That reference comment no longer exists.");
        context.request_repaint();
        return;
    }
    if let Some(active_drag) = state.reference_persisted_note_drag.as_mut() {
        active_drag.moved |= time_millis != active_drag.original_time_millis;
    }
    if let Some(draft) = state
        .reference_draft_note
        .as_mut()
        .filter(|draft| draft.note_id.as_deref() == Some(drag.note_id.as_str()))
    {
        draft.time_millis = time_millis;
    }
    state.reference_transport_position_millis = time_millis;
    state.status = format!("Reference comment at {}.", format_timestamp(time_millis));
    context.request_repaint();
}

fn finish_reference_persisted_note_drag(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    ratio: f32,
) {
    let Some(drag) = state.reference_persisted_note_drag.take() else {
        return;
    };
    let Some(duration_millis) = state
        .reference_waveform
        .as_ref()
        .filter(|_| {
            !state.reference_waveform_busy
                && state.reference_waveform_track_id.as_deref() == Some(drag.track_id.as_str())
        })
        .map(|waveform| waveform.duration_millis)
    else {
        return;
    };
    let time_millis = waveform::millis_for_ratio(ratio, duration_millis);
    let Some(reference) = selected_reference_track_mut(state) else {
        state.status = String::from("That reference track is no longer available.");
        context.request_repaint();
        return;
    };
    let Some(note) = reference
        .notes
        .iter_mut()
        .find(|note| note.id == drag.note_id)
    else {
        state.status = String::from("That reference comment no longer exists.");
        context.request_repaint();
        return;
    };
    if drag.moved {
        note.time_millis = time_millis;
    }
    let final_time_millis = note.time_millis;
    let changed = final_time_millis != drag.original_time_millis;
    reference.notes.sort_by_key(|note| note.time_millis);
    if let Some(draft) = state
        .reference_draft_note
        .as_mut()
        .filter(|draft| draft.note_id.as_deref() == Some(drag.note_id.as_str()))
    {
        draft.time_millis = final_time_millis;
    }
    state.reference_transport_position_millis = final_time_millis;
    state.status = if changed {
        format!(
            "Reference comment moved to {} and saved locally.",
            format_timestamp(final_time_millis)
        )
    } else {
        format!(
            "Selected reference comment at {}.",
            format_timestamp(final_time_millis)
        )
    };
    if changed {
        schedule_library_save(state, context);
    }
    context.request_repaint();
}

fn request_import(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    if state.busy {
        state.status = String::from("The library is still loading.");
        context.request_repaint();
        return;
    }
    if state.save_in_flight {
        state.status = String::from("Saving the library — try importing again in a moment.");
        context.request_repaint();
        return;
    }
    context.pick_file(audio_file_dialog("Import audio track"), Message::FilePicked);
}

fn request_replace(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    track_id: String,
) {
    if state.busy {
        state.status = String::from("The library is still loading.");
        context.request_repaint();
        return;
    }
    if state.save_in_flight {
        state.status = String::from("Saving the library — try replacing again in a moment.");
        context.request_repaint();
        return;
    }
    if !state
        .library
        .tracks
        .iter()
        .any(|track| track.id == track_id)
    {
        state.status = String::from("That track is no longer in the library.");
        context.request_repaint();
        return;
    }
    context.pick_file(audio_file_dialog("Replace audio track"), move |result| {
        Message::ReplaceFilePicked { track_id, result }
    });
}

fn request_reference(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    track_id: String,
) {
    if state.busy {
        state.status = String::from("The library is still loading.");
        context.request_repaint();
        return;
    }
    if state.save_in_flight {
        state.status =
            String::from("Saving the library — try importing a reference again in a moment.");
        context.request_repaint();
        return;
    }
    if !state
        .library
        .tracks
        .iter()
        .any(|track| track.id == track_id)
    {
        state.status = String::from("That track is no longer in the library.");
        context.request_repaint();
        return;
    }
    state.busy = true;
    state.status = String::from("Choosing reference tracks…");
    context
        .business()
        .blocking_io("cadence-pick-reference-tracks")
        .run(
            |_| pick_reference_tracks(),
            move |paths| Message::ReferenceFilesPicked { track_id, paths },
        );
    context.request_repaint();
}

fn audio_file_dialog(title: &str) -> FileDialogRequest {
    FileDialogRequest::new().title(title).filter(
        "Audio",
        vec![
            String::from("wav"),
            String::from("aiff"),
            String::from("flac"),
            String::from("m4a"),
            String::from("mp3"),
            String::from("ogg"),
            String::from("opus"),
            String::from("aac"),
        ],
    )
}

fn pick_reference_tracks() -> Vec<PathBuf> {
    // Radiant's pinned platform picker intentionally returns one path. Keep
    // the multi-selection interaction local to Cadence until that API grows a
    // native multi-path response.
    rfd::FileDialog::new()
        .set_title("Import reference tracks")
        .add_filter(
            "Audio",
            &["wav", "aiff", "flac", "m4a", "mp3", "ogg", "opus", "aac"],
        )
        .pick_files()
        .unwrap_or_default()
}

fn record_import_attempt(state: &mut AppState, failed: bool) {
    if let Some(batch) = state.import_batch.as_mut() {
        batch.completed = batch.completed.saturating_add(1).min(batch.total);
        if failed {
            batch.failed = batch.failed.saturating_add(1).min(batch.completed);
        }
    }
}

fn finish_import_batch(state: &mut AppState) {
    let should_finish = match state.import_batch.as_ref() {
        Some(batch) => {
            batch.completed >= batch.total
                && state.pending_import_paths.is_empty()
                && state.pending_reference_paths.is_empty()
        }
        None => false,
    };
    if !should_finish {
        return;
    }

    let Some(batch) = state.import_batch.take() else {
        return;
    };
    if batch.total > 1 {
        state.status = format!(
            "Imported {} of {} files; {} failed.",
            batch.total - batch.failed,
            batch.total,
            batch.failed
        );
    }
}

fn start_import(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>, path: PathBuf) {
    state.busy = true;
    state.status = format!("Importing {}…", path.display());
    let library = state.library.clone();
    context.business().blocking_io("cadence-import-track").run(
        move |_| storage::import_into_library(library, path),
        Message::ImportCompleted,
    );
    context.request_repaint();
}

fn schedule_import(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    path: PathBuf,
) {
    let batch = state.import_batch.get_or_insert_with(Default::default);
    batch.total = batch.total.saturating_add(1);
    if state.busy || state.save_in_flight {
        let display_name = path.display().to_string();
        state.pending_import_paths.push(path);
        state.status = if state.busy {
            format!(
                "Queued {} for import · {} file{} waiting.",
                display_name,
                state.pending_import_paths.len(),
                plural(state.pending_import_paths.len())
            )
        } else {
            String::from("Saving the library — the dropped file is queued for import.")
        };
        context.request_repaint();
        return;
    }
    start_import(state, context, path);
}

fn schedule_next_pending_import(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    if state.busy || state.save_in_flight {
        return;
    }
    let Some(path) =
        (!state.pending_import_paths.is_empty()).then(|| state.pending_import_paths.remove(0))
    else {
        return;
    };
    start_import(state, context, path);
}

fn schedule_reference_import(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    track_id: String,
    paths: Vec<PathBuf>,
) {
    if paths.is_empty() {
        state.status = String::from("Reference import canceled.");
        context.request_repaint();
        return;
    }
    if !state
        .library
        .tracks
        .iter()
        .any(|track| track.id == track_id)
    {
        state.status = String::from("That track is no longer in the library.");
        context.request_repaint();
        return;
    }
    state.pending_reference_paths = paths;
    state.pending_reference_track_id = Some(track_id);
    state.reference_import_selected_path = None;
    state.reference_draft_note = None;
    state.selected_reference_note_id = None;
    state.hovered_reference_note_id = None;
    state.import_batch = Some(ImportBatchProgress {
        total: state.pending_reference_paths.len(),
        completed: 0,
        failed: 0,
    });
    schedule_next_pending_reference_import(state, context);
}

fn schedule_next_pending_reference_import(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
) {
    if state.busy || state.save_in_flight {
        return;
    }
    let Some(track_id) = state.pending_reference_track_id.clone() else {
        return;
    };
    let Some(path) = (!state.pending_reference_paths.is_empty())
        .then(|| state.pending_reference_paths.remove(0))
    else {
        return;
    };
    schedule_reference(state, context, track_id, path);
}

fn schedule_replace(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    track_id: String,
    path: PathBuf,
) {
    if state.busy {
        return;
    }
    if state.save_in_flight {
        state.status = String::from("Saving the library — try replacing again in a moment.");
        context.request_repaint();
        return;
    }
    if !state
        .library
        .tracks
        .iter()
        .any(|track| track.id == track_id)
    {
        state.status = String::from("That track is no longer in the library.");
        context.request_repaint();
        return;
    }
    state.busy = true;
    state.status = format!("Replacing with {}…", path.display());
    let library = state.library.clone();
    let completion_track_id = track_id.clone();
    context.business().blocking_io("cadence-replace-track").run(
        move |_| storage::replace_track(library, &track_id, path),
        move |result| Message::ReplaceCompleted {
            track_id: completion_track_id,
            result,
        },
    );
    context.request_repaint();
}

fn schedule_reference(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    track_id: String,
    path: PathBuf,
) {
    if state.busy {
        return;
    }
    if state.save_in_flight {
        state.status =
            String::from("Saving the library — try importing a reference again in a moment.");
        context.request_repaint();
        return;
    }
    if !state
        .library
        .tracks
        .iter()
        .any(|track| track.id == track_id)
    {
        state.status = String::from("That track is no longer in the library.");
        context.request_repaint();
        return;
    }
    state.busy = true;
    state.status = format!("Importing reference {}…", path.display());
    let library = state.library.clone();
    let completion_track_id = track_id.clone();
    let completion_path = path.clone();
    context
        .business()
        .blocking_io("cadence-import-reference")
        .run(
            move |_| storage::set_reference_track(library, &track_id, path),
            move |result| Message::ReferenceImportCompleted {
                track_id: completion_track_id,
                path: completion_path,
                result,
            },
        );
    context.request_repaint();
}

fn schedule_library_save(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    if state.save_in_flight {
        // The next completion will schedule one save from the newest in-memory snapshot.
        // This keeps the blocking-I/O lane from receiving stale whole-library writes.
        state.save_again = true;
        return;
    }
    let library = state.library.clone();
    state.save_in_flight = true;
    context.business().blocking_io("cadence-save-library").run(
        move |_| storage::persist_library(&library),
        Message::LibrarySaved,
    );
}

fn reset_transport(state: &mut AppState) {
    state.transport_generation = state.transport_generation.wrapping_add(1);
    state.audition_play_token = None;
    state.transport_position_millis = 0;
    state.review_cursor_millis = 0;
    state.loop_selections.clear(AuditionSource::Main);
    state.playhead_drag_active = false;
    state.transport_playing = false;
    state.transport_polling = false;
    state.transport_waiting_token = None;
    let _ = state.transport.unload(state.transport_generation);
}

fn reset_reference_transport(state: &mut AppState) {
    state.reference_transport_generation = state.reference_transport_generation.wrapping_add(1);
    state.reference_playhead_drag_active = false;
    rollback_reference_persisted_note_drag(state);
    state.reference_transport_position_millis = 0;
    state.loop_selections.clear(AuditionSource::Reference);
    state.audition_source = AuditionSource::Main;
    state.reference_transport_playing = false;
    state.reference_transport_polling = false;
    state.reference_transport_waiting_token = None;
    state.reference_transport_loaded = false;
    state.reference_only_playback = false;
    if let Some(reference_transport) = state.reference_transport.as_ref() {
        let _ = reference_transport.unload(state.reference_transport_generation);
    }
    sync_audition_output_gains(state);
}

fn begin_transport_polling(state: &mut AppState, token: u64) {
    state.transport_waiting_token = Some(token);
    state.transport_polling = true;
}

fn apply_transport_snapshot(state: &mut AppState, snapshot: transport::Snapshot) {
    if snapshot.ready {
        if !state.playhead_drag_active {
            state.transport_position_millis = snapshot.position_millis;
            state.review_cursor_millis = snapshot.position_millis;
        }
        state.transport_playing = snapshot.playing;
        state.transport_polling = false;
    } else {
        state.transport_playing = false;
        state.transport_polling = false;
    }
}

fn update_reference_transport(state: &mut AppState) {
    let Some(snapshot) = state
        .reference_transport
        .as_ref()
        .map(transport::AudioTransport::snapshot)
    else {
        return;
    };
    if snapshot.generation != state.reference_transport_generation {
        return;
    }
    if let Some(error) = state
        .reference_transport
        .as_ref()
        .and_then(|reference_transport| {
            reference_transport.take_error(state.reference_transport_generation)
        })
    {
        state.reference_transport_playing = false;
        state.reference_transport_polling = false;
        state.reference_transport_waiting_token = None;
        state.reference_transport_loaded = false;
        state.reference_only_playback = false;
        state.reference_playhead_drag_active = false;
        rollback_reference_persisted_note_drag(state);
        state.status = error;
    } else if state
        .reference_transport_waiting_token
        .is_none_or(|token| transport_command_is_confirmed(snapshot, token))
    {
        state.reference_transport_waiting_token = None;
        apply_reference_transport_snapshot(state, snapshot);
    }
}

fn apply_reference_transport_snapshot(state: &mut AppState, snapshot: transport::Snapshot) {
    if snapshot.ready {
        if !state.reference_playhead_drag_active {
            state.reference_transport_position_millis = snapshot.position_millis;
        }
        state.reference_transport_playing = snapshot.playing;
        state.reference_transport_polling = false;
    } else {
        state.reference_transport_playing = false;
        state.reference_transport_polling = false;
    }
}

fn enforce_loop(state: &mut AppState, was_main_playing: bool, was_reference_playing: bool) {
    enforce_loop_for_source(state, AuditionSource::Main, was_main_playing);
    enforce_loop_for_source(state, AuditionSource::Reference, was_reference_playing);
}

fn enforce_loop_for_source(state: &mut AppState, source: AuditionSource, was_playing: bool) {
    if source == AuditionSource::Main && state.reference_only_playback {
        return;
    }
    let Some(bounds) = loop_bounds_for_source(state, source) else {
        return;
    };
    let source_is_playing = match source {
        AuditionSource::Main => state.transport_playing,
        AuditionSource::Reference => state.reference_transport_playing,
    };
    let source_is_polling = match source {
        AuditionSource::Main => state.transport_polling || state.transport_waiting_token.is_some(),
        AuditionSource::Reference => {
            state.reference_transport_polling || state.reference_transport_waiting_token.is_some()
        }
    };
    if source_is_polling || !(was_playing || source_is_playing) {
        return;
    }
    if source == AuditionSource::Reference && !state.reference_transport_loaded {
        return;
    }
    if source_position(state, source) < bounds.end_millis {
        return;
    }
    match seek_loop_owner(state, source, bounds) {
        Ok(()) => state.status = String::from("Looping the selected section…"),
        Err(error) => state.status = error,
    }
}

fn seek_review_position(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    ratio: f32,
    resume: bool,
) {
    if state.waveform_busy || state.reference_waveform_busy {
        return;
    }
    let Some(duration_millis) = state
        .waveform
        .as_ref()
        .map(|waveform| waveform.duration_millis)
    else {
        return;
    };
    disarm_audition_auto_advance(state);
    let time_millis = waveform::millis_for_ratio(ratio, duration_millis);
    state.reference_only_playback = false;
    state.review_cursor_millis = time_millis;
    state.transport_position_millis = time_millis;
    if resume {
        match resume_main_at_position_with_reference(state, time_millis) {
            Ok(()) => {
                state.status = format!("Playing from {}.", format_timestamp(time_millis));
            }
            Err(error) => state.status = error,
        }
    } else {
        state.status = format!("Scrubbing at {}.", format_timestamp(time_millis));
    }
    context.request_repaint();
}

fn close_stage_menu(state: &mut AppState) {
    state.stage_menu_track_id = None;
    state.stage_menu_anchor = None;
}

fn close_reference_menu(state: &mut AppState) {
    state.reference_menu_track_id = None;
    state.reference_menu_anchor = None;
}

fn select_track_internal(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    id: String,
    pending_play: bool,
) {
    if state.busy || !state.library.tracks.iter().any(|track| track.id == id) {
        return;
    }
    let in_audition = state.workspace_mode == WorkspaceMode::Audition;
    let previous_selected_id = state.library.selected_track_id.clone();
    if !in_audition {
        state.workspace_mode = WorkspaceMode::Review;
    }
    if in_audition
        && let Some(previous_id) = previous_selected_id.as_deref()
        && previous_id != id
    {
        remove_audition_queue_entry_if_outside_filter(state, previous_id);
    }
    state.library.selected_track_id = Some(id.clone());
    state.loop_selections.clear_all();
    if in_audition {
        if let Some(index) = state
            .audition_queue
            .iter()
            .position(|track_id| track_id == &id)
        {
            state.audition_queue_index = index;
        }
        state.audition_auto_advance = pending_play;
        state.audition_play_token = None;
        state.audition_pending_play_track_id = pending_play.then_some(id);
    } else {
        state.audition_auto_advance = false;
        state.audition_play_token = None;
        state.audition_pending_play_track_id = None;
    }
    close_stage_menu(state);
    close_status_menu(state);
    close_reference_menu(state);
    state.remove_confirmation_track_id = None;
    clear_planner_drag(state);
    if let Some(cancellation) = state.waveform_cancellation.take() {
        cancellation.cancel();
    }
    if let Some(cancellation) = state.reference_waveform_cancellation.take() {
        cancellation.cancel();
    }
    state.waveform = None;
    state.waveform_track_id = None;
    state.waveform_busy = false;
    state.waveform_progress = None;
    state.reference_waveform = None;
    state.reference_waveform_track_id = None;
    state.reference_waveform_busy = false;
    state.reference_waveform_progress = None;
    state.review_cursor_millis = 0;
    state.draft_note = None;
    state.reference_draft_note = None;
    rollback_persisted_note_drag(state);
    rollback_reference_persisted_note_drag(state);
    state.playhead_drag_active = false;
    state.reference_playhead_drag_active = false;
    state.selected_note_id = None;
    state.hovered_note_id = None;
    state.selected_reference_note_id = None;
    state.hovered_reference_note_id = None;
    state.comment_source = CommentSource::Main;
    state.comment_source_explicit = false;
    reset_transport(state);
    reset_reference_transport(state);
    state.reference_match_enabled = false;
    schedule_library_save(state, context);
    schedule_selected_waveform_decode(state, context);
    schedule_selected_reference_decode(state, context);
}

fn set_workspace_mode(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    mode: WorkspaceMode,
) {
    if state.workspace_mode == mode {
        return;
    }
    state.audition_auto_advance = false;
    state.audition_play_token = None;
    state.audition_pending_play_track_id = None;
    state.workspace_mode = mode;
    if mode == WorkspaceMode::Audition {
        rebuild_audition_queue(state);
        if let Some(track_id) = state.audition_queue.first().cloned() {
            select_track_internal(state, context, track_id, false);
        } else {
            reset_transport(state);
            reset_reference_transport(state);
        }
    }
    close_stage_menu(state);
    close_status_menu(state);
    close_reference_menu(state);
    state.remove_confirmation_track_id = None;
    clear_planner_drag(state);
    context.request_repaint();
}

fn disarm_audition_auto_advance(state: &mut AppState) {
    if state.workspace_mode == WorkspaceMode::Audition {
        state.audition_auto_advance = false;
        state.audition_play_token = None;
        state.audition_pending_play_track_id = None;
    }
}

fn rebuild_audition_queue(state: &mut AppState) {
    let mut queue = state
        .library
        .tracks
        .iter()
        .filter(|track| track.status == state.audition_status_filter)
        .map(|track| track.id.clone())
        .collect::<Vec<_>>();
    let seed = audition_shuffle_seed(
        state.audition_status_filter,
        &queue,
        state.audition_shuffle_round,
    );
    deterministic_shuffle(&mut queue, seed);
    let selected_id = state.library.selected_track_id.as_deref();
    state.audition_queue_index = selected_id
        .and_then(|selected_id| queue.iter().position(|id| id == selected_id))
        .unwrap_or(0)
        .min(queue.len());
    state.audition_queue = queue;
    state.audition_heard.clear();
}

fn shuffle_audition(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    if state.busy || state.workspace_mode != WorkspaceMode::Audition {
        return;
    }
    let previous_queue = state.audition_queue.clone();
    let previous_selected = state.library.selected_track_id.clone();
    state.audition_shuffle_round = state.audition_shuffle_round.wrapping_add(1);
    rebuild_audition_queue(state);
    ensure_audition_shuffle_change(
        &mut state.audition_queue,
        &previous_queue,
        previous_selected.as_deref(),
    );
    state.audition_heard.clear();

    if let Some(track_id) = state.audition_queue.first().cloned() {
        select_track_internal(state, context, track_id, true);
        state.status = format!(
            "New {} audition order ready — loading first track…",
            state.audition_status_filter.label()
        );
    } else {
        state.audition_auto_advance = false;
        state.audition_play_token = None;
        state.audition_pending_play_track_id = None;
        reset_transport(state);
        reset_reference_transport(state);
        state.status = format!("No tracks in {}.", state.audition_status_filter.label());
    }
    context.request_repaint();
}

fn ensure_audition_shuffle_change(
    queue: &mut Vec<String>,
    previous_queue: &[String],
    previous_selected: Option<&str>,
) {
    if queue.len() < 2 {
        return;
    }
    let shuffled = queue.clone();
    let order_changed = |candidate: &[String]| candidate != previous_queue;
    let first_is_new = |candidate: &[String]| {
        previous_selected.is_none_or(|selected| candidate.first().is_none_or(|id| id != selected))
    };

    for rotation in 0..queue.len() {
        let mut candidate = shuffled.clone();
        candidate.rotate_left(rotation);
        if order_changed(&candidate) && first_is_new(&candidate) {
            *queue = candidate;
            return;
        }
    }
    for rotation in 0..queue.len() {
        let mut candidate = shuffled.clone();
        candidate.rotate_left(rotation);
        if order_changed(&candidate) {
            *queue = candidate;
            return;
        }
    }
}

fn reconcile_audition_queue(state: &mut AppState) {
    let old_queue = std::mem::take(&mut state.audition_queue);
    let old_index = state.audition_queue_index;
    let current_id = old_queue.get(old_index).cloned();
    let active_anchor_id = audition_navigation_anchor(state);
    let mut queue = old_queue
        .into_iter()
        .filter(|track_id| {
            state
                .library
                .tracks
                .iter()
                .find(|track| &track.id == track_id)
                .is_some_and(|track| {
                    track.status == state.audition_status_filter
                        || active_anchor_id == Some(track_id.as_str())
                })
        })
        .collect::<Vec<_>>();
    for track in state
        .library
        .tracks
        .iter()
        .filter(|track| track.status == state.audition_status_filter)
    {
        if !queue.iter().any(|track_id| track_id == &track.id) {
            queue.push(track.id.clone());
        }
    }
    state.audition_queue_index = current_id
        .and_then(|current_id| queue.iter().position(|id| id == &current_id))
        .unwrap_or(old_index.min(queue.len()));
    state.audition_queue = queue;
    state.audition_heard.retain(|heard_id| {
        state
            .library
            .tracks
            .iter()
            .any(|track| &track.id == heard_id)
    });
}

fn audition_navigation_anchor(state: &AppState) -> Option<&str> {
    if state.workspace_mode != WorkspaceMode::Audition
        || state.audition_pending_play_track_id.is_some()
        || !(state.transport_playing
            || state.transport_polling
            || state.transport_waiting_token.is_some()
            || state.reference_transport_playing
            || state.reference_transport_polling
            || state.reference_transport_waiting_token.is_some())
    {
        return None;
    }
    state.library.selected_track_id.as_deref()
}

fn remove_audition_queue_entry_if_outside_filter(state: &mut AppState, track_id: &str) {
    let matches_filter = state
        .library
        .tracks
        .iter()
        .find(|track| track.id == track_id)
        .is_some_and(|track| track.status == state.audition_status_filter);
    if matches_filter {
        return;
    }
    let Some(position) = state.audition_queue.iter().position(|id| id == track_id) else {
        return;
    };
    state.audition_queue.remove(position);
    if position < state.audition_queue_index {
        state.audition_queue_index = state.audition_queue_index.saturating_sub(1);
    }
    if state.audition_queue.is_empty() {
        state.audition_queue_index = 0;
    } else {
        state.audition_queue_index = state.audition_queue_index.min(state.audition_queue.len());
    }
}

fn sync_audition_queue_after_status_change(state: &mut AppState, track_id: &str) {
    let matches_filter = state
        .library
        .tracks
        .iter()
        .find(|track| track.id == track_id)
        .is_some_and(|track| track.status == state.audition_status_filter);
    let is_navigation_anchor = audition_navigation_anchor(state) == Some(track_id);
    let Some(_) = state.audition_queue.iter().position(|id| id == track_id) else {
        if matches_filter {
            state.audition_queue.push(track_id.to_owned());
        }
        return;
    };
    if !matches_filter && !is_navigation_anchor {
        remove_audition_queue_entry_if_outside_filter(state, track_id);
    }
}

fn audition_shuffle_seed(status: storage::TrackStatus, track_ids: &[String], round: u64) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64 ^ round;
    for byte in status.label().bytes().chain([0]) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for id in track_ids {
        for byte in id.bytes().chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

fn deterministic_shuffle(items: &mut [String], mut seed: u64) {
    for index in (1..items.len()).rev() {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        seed = seed.wrapping_mul(0x2545f4914f6cdd1d);
        items.swap(index, (seed as usize) % (index + 1));
    }
}

fn next_audition_track_index(state: &AppState) -> Option<usize> {
    let selected_id = state.library.selected_track_id.as_deref();
    let mut index = state.audition_queue_index;
    if state.audition_queue.get(index).map(String::as_str) == selected_id {
        index += 1;
    }
    while let Some(track_id) = state.audition_queue.get(index) {
        if !state
            .audition_heard
            .iter()
            .any(|heard_id| heard_id == track_id)
            && state
                .library
                .tracks
                .iter()
                .find(|track| track.id == *track_id)
                .is_some_and(|track| track.status == state.audition_status_filter)
        {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn advance_audition(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    if let Some(current_id) = state.library.selected_track_id.as_ref()
        && !state
            .audition_heard
            .iter()
            .any(|heard_id| heard_id == current_id)
    {
        state.audition_heard.push(current_id.clone());
    }
    let Some(next_index) = next_audition_track_index(state) else {
        state.audition_auto_advance = false;
        state.audition_play_token = None;
        state.audition_pending_play_track_id = None;
        state.status = String::from("Audition complete.");
        return;
    };
    let Some(track_id) = state.audition_queue.get(next_index).cloned() else {
        return;
    };
    let title = state
        .library
        .tracks
        .iter()
        .find(|track| track.id == track_id)
        .map_or_else(|| track_id.clone(), |track| track.title.clone());
    select_track_internal(state, context, track_id, true);
    state.status = format!("Loading next audition track: {title}…");
}

fn maybe_start_pending_audition(state: &mut AppState, context: &mut ui::UiUpdateContext<Message>) {
    let Some(track_id) = state.audition_pending_play_track_id.clone() else {
        return;
    };
    if state.workspace_mode != WorkspaceMode::Audition
        || state.busy
        || state.waveform_busy
        || state.transport_polling
        || state.transport_waiting_token.is_some()
        || state.transport.has_pending_load()
        || state.reference_waveform_busy
        || state.library.selected_track_id.as_deref() != Some(track_id.as_str())
        || state.waveform_track_id.as_deref() != Some(track_id.as_str())
        || state.waveform.is_none()
    {
        return;
    }
    if !state.transport.has_command_capacity(1) {
        return;
    }
    if selected_reference_details(state).is_some() {
        let reference_transport = state
            .reference_transport
            .get_or_insert_with(transport::AudioTransport::spawn);
        let required_slots = if state.reference_transport_loaded {
            2
        } else {
            3
        };
        if reference_transport.has_pending_load()
            || !reference_transport.has_command_capacity(required_slots)
        {
            return;
        }
    }
    state.audition_pending_play_track_id = None;
    toggle_playback(state, context);
}

fn close_status_menu(state: &mut AppState) {
    state.status_menu_track_id = None;
    state.status_menu_host = None;
}

fn toggle_status_menu(
    state: &mut AppState,
    track_id: String,
    host: StatusMenuHost,
    context: &mut ui::UiUpdateContext<Message>,
) {
    if !state.busy
        && state
            .library
            .tracks
            .iter()
            .any(|track| track.id == track_id)
    {
        if state.status_menu_track_id.as_deref() == Some(track_id.as_str())
            && state.status_menu_host == Some(host)
        {
            close_status_menu(state);
        } else {
            close_stage_menu(state);
            state.status_menu_track_id = Some(track_id);
            state.status_menu_host = Some(host);
        }
        context.request_repaint();
    }
}

fn clear_planner_drag(state: &mut AppState) {
    state.planner_drag_source_track_id = None;
    state.planner_drag_target_stage = None;
    state.planner_drag_pointer = None;
}

fn rollback_persisted_note_drag(state: &mut AppState) {
    let Some(drag) = state.persisted_note_drag.take() else {
        return;
    };

    let mut note_restored = false;
    if let Some(track) = state
        .library
        .tracks
        .iter_mut()
        .find(|track| track.id == drag.track_id)
        && let Some(note) = track.notes.iter_mut().find(|note| note.id == drag.note_id)
    {
        note.time_millis = drag.original_time_millis;
        track.notes.sort_by_key(|note| note.time_millis);
        note_restored = true;
    }
    if note_restored && state.library.selected_track_id.as_deref() == Some(drag.track_id.as_str()) {
        state.review_cursor_millis = drag.original_time_millis;
    }
    if let Some(draft) = state
        .draft_note
        .as_mut()
        .filter(|draft| draft.note_id.as_deref() == Some(drag.note_id.as_str()))
    {
        draft.time_millis = drag.original_time_millis;
    }
}

fn rollback_reference_persisted_note_drag(state: &mut AppState) {
    let Some(drag) = state.reference_persisted_note_drag.take() else {
        return;
    };

    let mut note_restored = false;
    if let Some(reference) = selected_reference_track_mut(state)
        && let Some(note) = reference
            .notes
            .iter_mut()
            .find(|note| note.id == drag.note_id)
    {
        note.time_millis = drag.original_time_millis;
        reference.notes.sort_by_key(|note| note.time_millis);
        note_restored = true;
    }
    if note_restored && state.library.selected_track_id.as_deref() == Some(drag.track_id.as_str()) {
        state.reference_transport_position_millis = drag.original_time_millis;
    }
    if let Some(draft) = state
        .reference_draft_note
        .as_mut()
        .filter(|draft| draft.note_id.as_deref() == Some(drag.note_id.as_str()))
    {
        draft.time_millis = drag.original_time_millis;
    }
}

fn drag_message_position(message: ui::DragHandleMessage) -> Option<Point> {
    Some(match message {
        ui::DragHandleMessage::Started { position, .. }
        | ui::DragHandleMessage::Moved { position, .. }
        | ui::DragHandleMessage::Ended { position, .. }
        | ui::DragHandleMessage::DoubleActivate { position, .. }
        | ui::DragHandleMessage::Cancelled { position } => position,
    })
}

fn main_inline_comment_editor_id(note_id: &str) -> u64 {
    ui::stable_widget_id(MAIN_INLINE_COMMENT_EDITOR_SCOPE, note_id)
}

fn reference_inline_comment_editor_id(note_id: &str) -> u64 {
    ui::stable_widget_id(REFERENCE_INLINE_COMMENT_EDITOR_SCOPE, note_id)
}

fn project_surface(state: &AppState) -> ui::View<Message> {
    let workspace = match state.workspace_mode {
        WorkspaceMode::Review => ui::row([
            library_panel(state).width(LIBRARY_WIDTH).fill_height(),
            review_panel(state).fill(),
        ])
        .spacing(10.0)
        .fill(),
        WorkspaceMode::Planner => planner_panel(state).fill(),
        WorkspaceMode::Audition => ui::row([
            audition_panel(state).width(LIBRARY_WIDTH).fill_height(),
            review_panel(state).fill(),
        ])
        .spacing(10.0)
        .fill(),
    };

    let drag_preview = state
        .planner_drag_source_track_id
        .as_deref()
        .and_then(|track_id| {
            let title = state
                .library
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .map(|track| track.title.clone())?;
            let pointer = state.planner_drag_pointer?;
            Some(ui::drag_preview(format!("↕ {title}"), pointer).key("planner-card-drag-preview"))
        });
    let stage_menu = state
        .stage_menu_track_id
        .as_deref()
        .zip(state.stage_menu_anchor)
        .and_then(|(track_id, anchor)| {
            state
                .library
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .map(|track| stage_menu_popover(track, anchor))
        });
    let reference_menu = state
        .reference_menu_track_id
        .as_deref()
        .zip(state.reference_menu_anchor)
        .and_then(|(track_id, anchor)| {
            state
                .library
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .filter(|track| !reference_dropdown_paths(state, track).is_empty())
                .map(|track| reference_menu_popover(state, track, anchor))
        });
    let workspace_tabs = [
        WorkspaceMode::Review,
        WorkspaceMode::Planner,
        WorkspaceMode::Audition,
    ]
    .into_iter()
    .map(|mode| {
        let label = workspace_mode_label(mode);
        ui::button(label)
            .selected(state.workspace_mode == mode)
            .message(Message::SelectWorkspace(mode))
            .key(format!("workspace-tab-{}", label.to_ascii_lowercase()))
            .width(82.0)
            .height(28.0)
    })
    .collect::<Vec<_>>();
    let global_review_controls = match state.workspace_mode {
        WorkspaceMode::Review | WorkspaceMode::Audition => selected_track(state)
            .map(|track| review_global_controls(state, track))
            .unwrap_or_else(|| ui::spacer().width(0.0)),
        WorkspaceMode::Planner => ui::spacer().width(0.0),
    };
    let header = ui::row([
        ui::spacer().width(TITLEBAR_TRAFFIC_LIGHT_SAFE_GUTTER),
        ui::row(workspace_tabs)
            .spacing(4.0)
            .width(254.0)
            .height(28.0),
        ui::spacer().fill_width(),
        global_review_controls,
    ])
    .fill_width()
    .height(36.0)
    .spacing(12.0);
    let status_bar =
        if let Some(batch) = state.import_batch.as_ref().filter(|batch| batch.total > 1) {
            let current = batch.completed.saturating_add(1).min(batch.total);
            let remaining = batch.total.saturating_sub(batch.completed);
            let progress = ui::column([
                ui::row([
                    ui::text(format!(
                        "Importing {current} of {} · {remaining} remaining · {} failed",
                        batch.total, batch.failed
                    ))
                    .truncate()
                    .height(20.0)
                    .fill_width(),
                    ui::text("SPACE  play · ESC  stop · N  note")
                        .height(20.0)
                        .width(280.0)
                        .subtle(),
                ])
                .fill_width()
                .height(20.0)
                .spacing(12.0),
                ui::determinate_progress_bar(batch.completed as f32 / batch.total as f32)
                    .passive::<Message>()
                    .fill_width()
                    .height(10.0),
            ])
            .padding_x(8.0)
            .padding_y(4.0)
            .fill_width()
            .height(38.0)
            .spacing(2.0);
            ui::stack([ui::card().fill(), progress])
                .fill_width()
                .height(46.0)
        } else {
            ui::stack([
                ui::card().fill(),
                ui::row([
                    ui::text(state.status.clone())
                        .truncate()
                        .height(24.0)
                        .fill_width(),
                    ui::text("SPACE  play · ESC  stop · N  note")
                        .height(24.0)
                        .width(280.0)
                        .subtle(),
                ])
                .padding_x(8.0)
                .fill_width()
                .height(24.0)
                .spacing(12.0),
            ])
            .fill_width()
            .height(30.0)
        };

    let content = ui::column([
        header,
        workspace
            .accepts_native_file_drop()
            .on_native_file_drop(Message::FileDropped),
        status_bar,
    ])
    .padding(18.0)
    .spacing(12.0)
    .fill();

    ui::scene(
        ui::stack([content]).fill().overlays(
            ui::overlays()
                .popover_opt(stage_menu)
                .popover_opt(reference_menu)
                .drag_preview_opt(drag_preview),
        ),
    )
    .into_view()
}

fn audition_panel(state: &AppState) -> ui::View<Message> {
    let selected_id = state.library.selected_track_id.as_deref();
    let queue_tracks = state
        .audition_queue
        .iter()
        .enumerate()
        .filter_map(|(index, track_id)| {
            state
                .library
                .tracks
                .iter()
                .find(|track| &track.id == track_id)
                .cloned()
                .map(|track| (index, track))
        })
        .collect::<Vec<_>>();
    let queue_count = queue_tracks.len();
    let filter_buttons = audition_statuses()
        .into_iter()
        .map(|status| {
            status_filter_button(
                Some(state.audition_status_filter),
                "audition",
                Some(status),
                audition_status_filter_message,
                true,
            )
        })
        .collect::<Vec<_>>();
    let queue = if queue_tracks.is_empty() {
        ui::column([
            ui::text("No matching tracks.").height(26.0).fill_width(),
            ui::text(format!(
                "Move a track into {} or choose another filter.",
                state.audition_status_filter.label()
            ))
            .wrap()
            .height(44.0)
            .fill_width()
            .subtle(),
        ])
        .padding(12.0)
        .spacing(6.0)
        .fill_width()
    } else {
        ui::list(queue_tracks, move |(index, track)| {
            audition_queue_row(index, track, selected_id)
        })
        .without_chrome()
        .fill_height()
    };
    let progress = if queue_count == 0 {
        String::from("0 tracks")
    } else {
        format!(
            "{} of {} track{}",
            state
                .audition_queue_index
                .saturating_add(1)
                .min(queue_count),
            queue_count,
            plural(queue_count)
        )
    };
    let audition_controls = ui::row([
        ui::button("Previous")
            .message(Message::AuditionPrevious)
            .key("audition-previous")
            .height(30.0)
            .width(76.0),
        ui::button("Play")
            .message(Message::AuditionPlay)
            .key("audition-play")
            .height(30.0)
            .width(42.0),
        ui::button("Stop")
            .message(Message::StopPlayback)
            .key("audition-stop")
            .height(30.0)
            .width(42.0),
        ui::button("Next")
            .message(Message::AuditionNext)
            .key("audition-next")
            .height(30.0)
            .width(50.0),
    ])
    .spacing(4.0)
    .fill_width();
    let content = ui::column([
        ui::row([
            ui::column([
                ui::text("AUDITION / PLAYLIST")
                    .height(18.0)
                    .fill_width()
                    .subtle(),
                ui::text("Fixed shuffle · one pass")
                    .height(26.0)
                    .fill_width(),
            ])
            .fill_width(),
            ui::button("Shuffle")
                .primary()
                .message(Message::ShuffleAudition)
                .key("audition-shuffle")
                .height(26.0)
                .width(80.0),
        ])
        .fill_width()
        .spacing(8.0),
        audition_controls,
        ui::text("PLAY STATUS").height(18.0).fill_width().subtle(),
        ui::column(filter_buttons).spacing(3.0).fill_width(),
        ui::row([ui::text(format!(
            "{} queue · {}",
            state.audition_status_filter.label(),
            progress
        ))
        .truncate()
        .height(22.0)
        .fill_width()
        .subtle()])
        .fill_width()
        .height(22.0),
        queue,
    ])
    .padding(14.0)
    .spacing(8.0)
    .fill_height();
    ui::stack([ui::card().fill(), content]).fill_height()
}

fn audition_queue_row(
    index: usize,
    track: storage::Track,
    selected_id: Option<&str>,
) -> ui::View<Message> {
    let selected = selected_id == Some(track.id.as_str());
    let track_id = track.id.clone();
    let title = format!("{:02}  {}", index + 1, track.title);
    let favorite_control =
        favorite_toggle(&track, selected, format!("audition-favorite-{}", track.id));
    let input = ui::button(title.clone())
        .selected(selected)
        .message(Message::SelectTrack(track_id.clone()))
        .key(format!("audition-queue-track-{track_id}"))
        .fill_width()
        .height(28.0);
    let row = ui::column([
        ui::row([input.fill_width().height(28.0), favorite_control])
            .fill_width()
            .spacing(6.0)
            .height(28.0),
        ui::text(track.original_name)
            .truncate()
            .height(18.0)
            .fill_width()
            .subtle(),
    ])
    .padding(7.0)
    .spacing(2.0)
    .fill_width();
    ui::stack([ui::card().fill(), row])
        .key(format!("audition-queue-row-{track_id}"))
        .fill_width()
}

fn audition_statuses() -> [storage::TrackStatus; 5] {
    [
        storage::TrackStatus::Inbox,
        storage::TrackStatus::Refine,
        storage::TrackStatus::Release,
        storage::TrackStatus::Archive,
        storage::TrackStatus::Maybe,
    ]
}

fn status_filter_options() -> [Option<storage::TrackStatus>; 6] {
    [
        None,
        Some(storage::TrackStatus::Inbox),
        Some(storage::TrackStatus::Refine),
        Some(storage::TrackStatus::Release),
        Some(storage::TrackStatus::Archive),
        Some(storage::TrackStatus::Maybe),
    ]
}

fn status_filter_label(status: Option<storage::TrackStatus>) -> &'static str {
    status.map_or("All", storage::TrackStatus::label)
}

fn status_filter_button(
    selected: Option<storage::TrackStatus>,
    key_prefix: &str,
    status: Option<storage::TrackStatus>,
    message: fn(Option<storage::TrackStatus>) -> Message,
    fill_width: bool,
) -> ui::View<Message> {
    let width = (!fill_width).then_some(76.0);
    colored_status_option(
        status,
        selected == status,
        message(status),
        format!(
            "{key_prefix}-status-filter-{}",
            status_filter_label(status).to_ascii_lowercase()
        ),
        width,
        26.0,
    )
}

fn audition_status_filter_message(status: Option<storage::TrackStatus>) -> Message {
    Message::SetAuditionFilter(status.expect("audition filters always select a status"))
}

fn status_filter_controls(
    selected: Option<storage::TrackStatus>,
    key_prefix: &str,
    message: fn(Option<storage::TrackStatus>) -> Message,
) -> ui::View<Message> {
    ui::row(
        status_filter_options()
            .into_iter()
            .map(|status| status_filter_button(selected, key_prefix, status, message, false))
            .collect::<Vec<_>>(),
    )
    .spacing(3.0)
    .fill_width()
}

fn status_filter_dropdown(
    selected: Option<storage::TrackStatus>,
    key_prefix: &str,
    message: fn(Option<storage::TrackStatus>) -> Message,
    open: bool,
) -> ui::View<Message> {
    let trigger = ui::row([
        status_rail(selected, ui::dropdown_trigger_height()),
        ui::dropdown_trigger(status_filter_label(selected), open)
            .toggle_message(Message::ToggleReviewFilterMenu)
            .build()
            .key(format!("{key_prefix}-status-filter"))
            .fill_width(),
    ])
    .spacing(STATUS_RAIL_GAP)
    .fill_width()
    .height(ui::dropdown_trigger_height());
    if open {
        ui::column([
            trigger,
            status_filter_menu(selected, key_prefix, message)
                .fill_width()
                .height(ui::dropdown_menu_height(status_filter_options().len())),
        ])
        .spacing(3.0)
        .fill_width()
    } else {
        ui::column([trigger]).fill_width()
    }
}

fn status_filter_menu(
    selected: Option<storage::TrackStatus>,
    key_prefix: &str,
    message: fn(Option<storage::TrackStatus>) -> Message,
) -> ui::View<Message> {
    colored_status_menu(
        format!("{key_prefix}-status-filter-menu"),
        status_filter_options()
            .into_iter()
            .map(|status| {
                (
                    status,
                    selected == status,
                    message(status),
                    format!(
                        "{key_prefix}-status-filter-option-{}",
                        status_filter_label(status).to_ascii_lowercase()
                    ),
                )
            })
            .collect(),
    )
}

fn colored_status_option(
    status: Option<storage::TrackStatus>,
    selected: bool,
    message: Message,
    key: String,
    width: Option<f32>,
    height: f32,
) -> ui::View<Message> {
    let button_width = width.map(|width| (width - STATUS_RAIL_WIDTH - STATUS_RAIL_GAP).max(0.0));
    let button = ui::button(status_filter_label(status))
        .selected(selected)
        .message(message)
        .key(key)
        .fill_width()
        .height(height);
    let button = if let Some(width) = button_width {
        button.width(width)
    } else {
        button
    };
    let row = ui::row([status_rail(status, height), button]).spacing(STATUS_RAIL_GAP);
    if let Some(width) = width {
        row.width(width).height(height)
    } else {
        row.fill_width().height(height)
    }
}

fn colored_status_menu(
    key: String,
    options: Vec<(Option<storage::TrackStatus>, bool, Message, String)>,
) -> ui::View<Message> {
    let option_count = options.len();
    ui::column(
        options
            .into_iter()
            .map(|(status, selected, message, option_key)| {
                colored_status_option(status, selected, message, option_key, None, 22.0)
            })
            .collect::<Vec<_>>(),
    )
    .key(key)
    .style(ui::WidgetStyle::strong(ui::WidgetTone::Neutral))
    .padding(4.0)
    .spacing(3.0)
    .fill_width()
    .height(ui::dropdown_menu_height(option_count))
}

fn review_status_filter_message(status: Option<storage::TrackStatus>) -> Message {
    Message::SetReviewStatusFilter(status)
}

fn planner_status_filter_message(status: Option<storage::TrackStatus>) -> Message {
    Message::SetPlannerStatusFilter(status)
}

const fn workspace_mode_label(mode: WorkspaceMode) -> &'static str {
    match mode {
        WorkspaceMode::Review => "Review",
        WorkspaceMode::Planner => "Planner",
        WorkspaceMode::Audition => "Audition",
    }
}

fn planner_panel(state: &AppState) -> ui::View<Message> {
    let stages = [
        storage::TrackStage::SoundDesign,
        storage::TrackStage::Production,
        storage::TrackStage::Mixdown,
        storage::TrackStage::Mastering,
    ];
    let filtered_tracks = tracks_with_status(&state.library.tracks, state.planner_status_filter);
    let drag_source_track_id = state.planner_drag_source_track_id.as_deref();
    let drag_source_stage = drag_source_track_id.and_then(|track_id| {
        state
            .library
            .tracks
            .iter()
            .find(|track| track.id == track_id)
            .map(|track| track.stage)
    });
    let drag_active = drag_source_track_id.is_some();
    let drag_target_stage = state.planner_drag_target_stage;
    let columns = stages.into_iter().map(|stage| {
        planner_column(
            stage,
            tracks_in_stage(&filtered_tracks, stage),
            state.planner_status_filter,
            PlannerColumnContext {
                selected_id: state.library.selected_track_id.as_deref(),
                stage_menu_track_id: state.stage_menu_track_id.as_deref(),
                status_menu_track_id: state.status_menu_track_id.as_deref(),
                status_menu_host: state.status_menu_host,
            },
            drag_active,
            drag_source_stage,
            drag_target_stage,
        )
    });
    let track_count = filtered_tracks.len();
    ui::column([
        ui::row([
            ui::column([
                ui::text("FINISHING BOARD")
                    .height(18.0)
                    .fill_width()
                    .subtle(),
                ui::text("Move every track toward release.")
                    .height(30.0)
                    .fill_width(),
            ])
            .fill_width(),
            status_filter_controls(
                state.planner_status_filter,
                "planner",
                planner_status_filter_message,
            )
            .width(480.0),
            ui::text(format!(
                "{} track{} · derived from the library",
                track_count,
                plural(track_count)
            ))
            .height(24.0)
            .subtle(),
        ])
        .fill_width()
        .spacing(12.0),
        ui::row(columns).spacing(10.0).fill(),
    ])
    .padding(18.0)
    .spacing(14.0)
    .fill()
}

struct PlannerColumnContext<'a> {
    selected_id: Option<&'a str>,
    stage_menu_track_id: Option<&'a str>,
    status_menu_track_id: Option<&'a str>,
    status_menu_host: Option<StatusMenuHost>,
}

fn planner_column(
    stage: storage::TrackStage,
    tracks: Vec<storage::Track>,
    status_filter: Option<storage::TrackStatus>,
    context: PlannerColumnContext<'_>,
    drag_active: bool,
    drag_source_stage: Option<storage::TrackStage>,
    drag_target_stage: Option<storage::TrackStage>,
) -> ui::View<Message> {
    let PlannerColumnContext {
        selected_id,
        stage_menu_track_id,
        status_menu_track_id,
        status_menu_host,
    } = context;
    let count = tracks.len();
    let candidate = drag_active && planner_drop_is_valid(drag_source_stage, stage);
    let current_target = drag_target_stage == Some(stage);
    let mut children = vec![
        ui::row([
            ui::text(if current_target {
                "DROP HERE"
            } else {
                stage.label()
            })
            .height(24.0)
            .fill_width(),
            ui::badge(count.to_string())
                .passive()
                .subtle()
                .width(32.0)
                .height(24.0),
        ])
        .fill_width()
        .spacing(8.0),
    ];
    if tracks.is_empty() {
        children.push(if status_filter.is_some() {
            ui::column([
                ui::text("No matching tracks.").height(24.0).fill_width(),
                ui::text(format!(
                    "No tracks in the {} status.",
                    status_filter_label(status_filter)
                ))
                .wrap()
                .height(44.0)
                .fill_width()
                .subtle(),
            ])
            .padding(10.0)
            .spacing(6.0)
            .fill_width()
        } else {
            ui::column([
                ui::text("No tracks here yet.").height(24.0).fill_width(),
                ui::text("Choose this stage from a card when it is ready.")
                    .wrap()
                    .height(44.0)
                    .fill_width()
                    .subtle(),
            ])
            .padding(10.0)
            .spacing(6.0)
            .fill_width()
        });
    } else {
        children.push(
            ui::list(tracks, move |track| {
                planner_card(
                    track,
                    selected_id,
                    stage_menu_track_id,
                    status_menu_track_id,
                    status_menu_host,
                )
            })
            .without_chrome()
            .fill_height(),
        );
    }
    let content = ui::stack([
        ui::card().fill(),
        ui::column(children).padding(12.0).spacing(8.0).fill(),
    ])
    .fill();
    let actions = ui::InteractiveRowActions::new().tracked_drop_candidate_key(
        stage,
        Message::PlannerStageDropped,
        |stage, _position| Message::PlannerStageHovered(stage),
        |stage, _position| Message::PlannerStageHoverCleared(stage),
    );
    let drop_target = ui::interactive_row()
        .tracked_drop_candidate(drag_active, current_target, candidate, current_target)
        .actions(actions)
        .key(format!("planner-column-drop-{}", stage.label()))
        .fill()
        .input_only();
    if drag_active {
        ui::stack([content, drop_target]).fill()
    } else {
        content
    }
}

fn planner_card(
    track: storage::Track,
    selected_id: Option<&str>,
    stage_menu_track_id: Option<&str>,
    status_menu_track_id: Option<&str>,
    status_menu_host: Option<StatusMenuHost>,
) -> ui::View<Message> {
    let selected = selected_id == Some(track.id.as_str());
    let stage_menu_open = stage_menu_track_id == Some(track.id.as_str());
    let status_menu_open = status_menu_host == Some(StatusMenuHost::Planner)
        && status_menu_track_id == Some(track.id.as_str());
    let title_track_id = track.id.clone();
    let drag_track_id = track.id.clone();
    let favorite_control =
        favorite_toggle(&track, selected, format!("planner-favorite-{}", track.id));
    let open_comments = track.notes.iter().filter(|note| !note.done).count();
    let card_content = ui::column([
        ui::row([
            card_control(
                selected,
                "↕",
                ui::button("↕")
                    .click_or_drag(
                        Message::PlannerCardHandleActivated(track.id.clone()),
                        move |message| Message::PlannerCardDrag {
                            track_id: drag_track_id.clone(),
                            message,
                        },
                    )
                    .key(format!("planner-card-drag-{}", track.id))
                    .size(22.0, 22.0),
            )
            .width(22.0)
            .height(22.0),
            card_control(
                selected,
                track.title.clone(),
                ui::button(track.title.clone())
                    .style(ui::WidgetStyle::strong(ui::WidgetTone::Neutral))
                    .selected(selected)
                    .message(Message::SelectTrack(title_track_id))
                    .fill_width()
                    .height(28.0),
            )
            .fill_width()
            .height(28.0),
            favorite_control,
        ])
        .fill_width()
        .spacing(6.0),
        card_muted_text(selected, track.original_name.clone())
            .truncate()
            .height(20.0)
            .fill_width()
            .subtle(),
        card_muted_text(
            selected,
            format!("{} open comment{}", open_comments, plural(open_comments)),
        )
        .height(22.0)
        .fill_width()
        .subtle(),
        stage_dropdown(&track, stage_menu_open, selected),
        status_dropdown_for_host(&track, status_menu_open, selected, StatusMenuHost::Planner),
    ])
    .padding(10.0)
    .spacing(5.0)
    .fill_width();
    let card_background = ui::card()
        .style(ui::WidgetStyle::normal(ui::WidgetTone::Neutral))
        .fill();
    ui::stack([card_background, card_content])
        .key(format!("planner-card-{}", track.id))
        .fill_width()
}

fn tracks_with_status(
    tracks: &[storage::Track],
    status: Option<storage::TrackStatus>,
) -> Vec<storage::Track> {
    tracks
        .iter()
        .filter(|track| status.is_none_or(|status| track.status == status))
        .cloned()
        .collect()
}

fn tracks_in_stage(tracks: &[storage::Track], stage: storage::TrackStage) -> Vec<storage::Track> {
    tracks
        .iter()
        .filter(|track| track.stage == stage)
        .cloned()
        .collect()
}

fn planner_drop_is_valid(
    source_stage: Option<storage::TrackStage>,
    target_stage: storage::TrackStage,
) -> bool {
    source_stage.is_some_and(|source_stage| source_stage != target_stage)
}

const STAGE_MENU_WIDTH: f32 = 174.0;

fn keyboard_stage_menu_anchor(state: &AppState) -> Point {
    match state.workspace_mode {
        WorkspaceMode::Review => Point::new(LIBRARY_WIDTH, 150.0),
        WorkspaceMode::Planner => Point::new(18.0 + STAGE_MENU_WIDTH * 0.5, 96.0),
        WorkspaceMode::Audition => Point::new(LIBRARY_WIDTH, 150.0),
    }
}

fn stage_menu_anchor_from_pointer(position: Point) -> Point {
    Point::new(
        position.x - STAGE_MENU_WIDTH * 0.5,
        position.y + ui::dropdown_trigger_height() * 0.5,
    )
}

fn stage_dropdown_options(track: &storage::Track) -> Vec<ui::DropdownOption<Message>> {
    let stage_id = track.id.clone();
    [
        storage::TrackStage::SoundDesign,
        storage::TrackStage::Production,
        storage::TrackStage::Mixdown,
        storage::TrackStage::Mastering,
    ]
    .into_iter()
    .map(|stage| {
        ui::DropdownOption::new(
            stage.label(),
            track.stage == stage,
            Message::SetStage {
                track_id: stage_id.clone(),
                stage,
            },
        )
    })
    .collect()
}

fn status_visual_color(status: storage::TrackStatus, theme: &ThemeTokens) -> ui::Rgba8 {
    match status {
        storage::TrackStatus::Inbox => TRACK_CARD_SELECTED_CORAL,
        storage::TrackStatus::Refine => theme.accent_warning,
        storage::TrackStatus::Release => theme.highlight_cyan,
        storage::TrackStatus::Archive => theme.text_muted,
        storage::TrackStatus::Maybe => theme.accent_danger,
    }
}

fn card_control(
    _selected: bool,
    _value: impl Into<ui::TextContent>,
    input: ui::View<Message>,
) -> ui::View<Message> {
    input
}

fn card_muted_text(_selected: bool, value: impl Into<ui::TextContent>) -> ui::View<Message> {
    ui::text(value)
}

fn favorite_toggle(track: &storage::Track, selected: bool, key: String) -> ui::View<Message> {
    ui::button(if track.favorite { "★" } else { "☆" })
        .subtle()
        .active(track.favorite)
        .selected(selected)
        .message(Message::ToggleFavorite(track.id.clone()))
        .key(key)
        .tooltip(if track.favorite {
            "Remove favorite"
        } else {
            "Mark favorite"
        })
        .size(FAVORITE_CONTROL_WIDTH, 24.0)
}

fn stage_dropdown(track: &storage::Track, open: bool, _selected: bool) -> ui::View<Message> {
    let stage_id = track.id.clone();
    let label = track.stage.label().to_owned();
    ui::dropdown_trigger(label, open)
        .toggle_message(Message::ToggleStageMenu(stage_id.clone()))
        .build()
        .fill_width()
        .height(24.0)
        .pointer_target(
            ui::pointer_target(true)
                .pointer_move(false)
                .pointer_press(true)
                .pointer_release(false)
                .pointer_drop(false)
                .wheel(false)
                .filter_map(move |message| match message {
                    ui::PointerShieldMessage::PointerPress { position, .. } => {
                        Some(Message::ToggleStageMenuAt {
                            track_id: stage_id.clone(),
                            position,
                        })
                    }
                    _ => None,
                }),
        )
        .key(format!("stage-dropdown-{}", track.id))
}

fn stage_menu_popover(track: &storage::Track, anchor: Point) -> ui::View<Message> {
    let options = stage_dropdown_options(track);
    let size = Vector2::new(STAGE_MENU_WIDTH, ui::dropdown_menu_height(options.len()));
    anchored_popover_from_parts(AnchoredPopoverParts::below(
        ui::dropdown_menu(options),
        ui::AnchoredPopoverAnchor::pointer(anchor),
        size,
    ))
}

const REFERENCE_MENU_WIDTH: f32 = 190.0;

fn reference_menu_anchor_from_pointer(position: Point) -> Point {
    Point::new(
        position.x - REFERENCE_MENU_WIDTH * 0.5,
        position.y + ui::dropdown_trigger_height() * 0.5,
    )
}

fn keyboard_reference_menu_anchor() -> Point {
    Point::new(700.0, 42.0)
}

fn reference_menu_popover(
    state: &AppState,
    track: &storage::Track,
    anchor: Point,
) -> ui::View<Message> {
    let options = reference_dropdown_options(state, track);
    let size = Vector2::new(
        REFERENCE_MENU_WIDTH,
        ui::dropdown_menu_height(options.len()),
    );
    anchored_popover_from_parts(AnchoredPopoverParts::below(
        ui::dropdown_menu(options).key(format!("reference-menu-{}", track.id)),
        ui::AnchoredPopoverAnchor::pointer(anchor),
        size,
    ))
}

fn status_dropdown_trigger(
    track: &storage::Track,
    open: bool,
    host: StatusMenuHost,
) -> ui::View<Message> {
    let status_id = track.id.clone();
    let label = track.status.label().to_owned();
    let trigger = ui::dropdown_trigger(label, open)
        .toggle_message(Message::ToggleStatusMenuAt {
            track_id: status_id,
            host,
        })
        .build()
        .style(ui::WidgetStyle::strong(ui::WidgetTone::Neutral))
        .key(format!("status-dropdown-{}", track.id))
        .fill_width()
        .height(ui::dropdown_trigger_height());
    ui::row([status_dropdown_rail(track.status), trigger])
        .spacing(STATUS_RAIL_GAP)
        .fill_width()
        .height(ui::dropdown_trigger_height())
}

fn status_dropdown_for_host(
    track: &storage::Track,
    open: bool,
    _selected: bool,
    host: StatusMenuHost,
) -> ui::View<Message> {
    let trigger = status_dropdown_trigger(track, open, host);
    if open {
        let menu = status_menu(track, host);
        ui::column([trigger, menu]).spacing(3.0).fill_width()
    } else {
        ui::column([trigger])
            .fill_width()
            .height(ui::dropdown_trigger_height())
    }
}

fn status_menu(track: &storage::Track, host: StatusMenuHost) -> ui::View<Message> {
    let track_id = track.id.clone();
    colored_status_menu(
        format!("status-menu-{}-{}", track.id, status_menu_host_key(host)),
        [
            storage::TrackStatus::Inbox,
            storage::TrackStatus::Refine,
            storage::TrackStatus::Release,
            storage::TrackStatus::Archive,
            storage::TrackStatus::Maybe,
        ]
        .into_iter()
        .map(|status| {
            (
                Some(status),
                track.status == status,
                Message::SetStatus {
                    track_id: track_id.clone(),
                    status,
                },
                format!(
                    "status-menu-option-{}-{}",
                    track.id,
                    status.label().to_ascii_lowercase()
                ),
            )
        })
        .collect(),
    )
}

const fn status_menu_host_key(host: StatusMenuHost) -> &'static str {
    match host {
        StatusMenuHost::Library => "library",
        StatusMenuHost::Planner => "planner",
    }
}

fn library_panel(state: &AppState) -> ui::View<Message> {
    let selected_id = state.library.selected_track_id.clone();
    let tracks = tracks_with_status(&state.library.tracks, state.review_status_filter);
    let total_track_count = state.library.tracks.len();
    let content = ui::column([
        ui::button("Import")
            .primary()
            .message(Message::ImportPressed)
            .fill_width()
            .height(34.0),
        status_filter_dropdown(
            state.review_status_filter,
            "review",
            review_status_filter_message,
            state.review_filter_menu_open,
        ),
        if tracks.is_empty() {
            if total_track_count == 0 {
                ui::column([
                    ui::text("No tracks yet.").height(28.0).fill_width(),
                    ui::text("Choose a file or drop audio onto the workspace.")
                        .wrap()
                        .height(48.0)
                        .fill_width()
                        .subtle(),
                ])
                .padding(12.0)
                .spacing(6.0)
                .fill_width()
            } else {
                ui::column([
                    ui::text("No matching tracks.").height(28.0).fill_width(),
                    ui::text(format!(
                        "No tracks in the {} status.",
                        status_filter_label(state.review_status_filter)
                    ))
                    .wrap()
                    .height(48.0)
                    .fill_width()
                    .subtle(),
                ])
                .padding(12.0)
                .spacing(6.0)
                .fill_width()
            }
        } else {
            ui::list(tracks.into_iter().enumerate(), move |(index, track)| {
                track_row(
                    index,
                    track,
                    selected_id.as_deref(),
                    state.stage_menu_track_id.as_deref(),
                    state.status_menu_track_id.as_deref(),
                    state.status_menu_host,
                    state.remove_confirmation_track_id.as_deref(),
                )
            })
            .without_chrome()
            .padding_x(LIBRARY_LIST_INSET)
            .spacing(LIBRARY_CARD_SPACING)
            .fill_height()
        },
    ])
    .padding(10.0)
    .spacing(8.0)
    .fill_height();
    ui::stack([ui::card().fill(), content]).fill_height()
}

fn track_row(
    _index: usize,
    track: storage::Track,
    selected_id: Option<&str>,
    stage_menu_track_id: Option<&str>,
    status_menu_track_id: Option<&str>,
    status_menu_host: Option<StatusMenuHost>,
    remove_confirmation_track_id: Option<&str>,
) -> ui::View<Message> {
    let selected = selected_id == Some(track.id.as_str());
    let id = track.id.clone();
    let stage_menu_open = stage_menu_track_id == Some(track.id.as_str());
    let status_menu_open = status_menu_host == Some(StatusMenuHost::Library)
        && status_menu_track_id == Some(track.id.as_str());
    let remove_confirmation_open = remove_confirmation_track_id == Some(track.id.as_str());
    let remove_id = track.id.clone();
    let favorite_control =
        favorite_toggle(&track, selected, format!("library-favorite-{}", track.id));
    let replace_id = track.id.clone();
    let replace_control = ui::button("↻")
        .subtle()
        .message(Message::ReplacePressed(replace_id))
        .key(format!("library-replace-{}", track.id))
        .tooltip("Replace track")
        .size(28.0, 24.0);
    let remove_control = ui::close_button()
        .subtle()
        .message(Message::RequestRemoveTrack(remove_id.clone()))
        .key(format!("library-remove-{}", track.id))
        .tooltip("Remove track")
        .size(28.0, 24.0);
    let stage_control = stage_dropdown(&track, stage_menu_open, selected);
    let status_control =
        status_dropdown_for_host(&track, status_menu_open, selected, StatusMenuHost::Library);
    let removal_controls = if remove_confirmation_open {
        ui::row([
            card_control(
                selected,
                "Confirm",
                ui::button("Confirm")
                    .message(Message::ConfirmRemoveTrack(remove_id.clone()))
                    .height(20.0),
            )
            .height(20.0),
            card_control(
                selected,
                "Cancel",
                ui::button("Cancel")
                    .message(Message::CancelRemoveTrack)
                    .height(20.0),
            )
            .height(20.0),
        ])
        .spacing(4.0)
        .fill_width()
    } else {
        ui::spacer().fill_width().height(0.0)
    };
    let row_select_id = track.id.clone();
    let row_background = ui::interactive_row_underlay(ui::spacer().fill())
        .selected(selected)
        .style(ui::WidgetStyle::normal(ui::WidgetTone::Neutral))
        .dense_chrome_palette(ui::DenseRowPalette::new())
        .actions(ui::row_actions().primary(move || Message::SelectTrack(row_select_id.clone())))
        .key(format!("library-track-input-{}", track.id))
        .fill();
    ui::stack([
        row_background,
        track_card_chrome(selected),
        ui::column([
            ui::row([
                card_control(
                    selected,
                    track.title.clone(),
                    ui::button(track.title)
                        .style(ui::WidgetStyle::strong(ui::WidgetTone::Neutral))
                        .selected(selected)
                        .message(Message::SelectTrack(id))
                        .fill_width()
                        .height(28.0),
                )
                .fill_width()
                .height(26.0),
                favorite_control,
                replace_control,
                remove_control,
            ])
            .spacing(3.0)
            .fill_width()
            .height(26.0),
            removal_controls,
            stage_control,
            status_control,
        ])
        .padding(TRACK_CARD_CONTENT_INSET)
        .fill_width()
        .spacing(TRACK_CARD_CONTENT_SPACING),
    ])
    .key(format!("library-track-{}", track.id))
    .fill_width()
}

fn audition_source_choice(
    label: &'static str,
    source: AuditionSource,
    active: bool,
) -> ui::View<Message> {
    let input = ui::button(if active { "●" } else { "○" })
        .active(active)
        .message(Message::SelectAuditionSource(source))
        .key(format!("audition-source-{}", label.to_ascii_lowercase()))
        .tooltip(format!("Audition {label}"))
        .height(28.0);
    input.width(AUDITION_SOURCE_SELECTOR_WIDTH).height(28.0)
}

const REVIEW_TRANSPORT_ICON_TINTS: ui::SvgIconTintPalette = ui::SvgIconTintPalette::new(
    ui::Rgba8::new(216, 215, 211, 255),
    ui::Rgba8::new(233, 88, 67, 255),
    ui::Rgba8::new(153, 155, 154, 255),
);

static REVIEW_PLAY_ICON: ui::SvgIconTintCache = ui::SvgIconTintCache::new(
    r#"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg" fill="currentColor">
  <path d="M4 2.5 13 8l-9 5.5z"/>
</svg>"#,
);

static REVIEW_PAUSE_ICON: ui::SvgIconTintCache = ui::SvgIconTintCache::new(
    r#"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg" fill="currentColor">
  <rect x="3" y="2.5" width="3" height="11"/>
  <rect x="10" y="2.5" width="3" height="11"/>
</svg>"#,
);

static REVIEW_VOLUME_ICON: ui::SvgIconTintCache = ui::SvgIconTintCache::new(
    r#"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg" fill="currentColor">
  <path d="M2 6h3l4-3v10l-4-3H2z"/>
  <path d="M12 5.5a3.7 3.7 0 0 1 0 5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
  <path d="M14.2 3.5a6.7 6.7 0 0 1 0 9" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
</svg>"#,
);

fn review_transport_icon(icon: &'static ui::SvgIconTintCache, active: bool) -> ui::SvgIcon {
    icon.icon_for_state(REVIEW_TRANSPORT_ICON_TINTS, true, active)
}

fn review_panel(state: &AppState) -> ui::View<Message> {
    let Some(track) = selected_track(state).cloned() else {
        return ui::column([
            ui::text("Your review desk").height(30.0).fill_width(),
            ui::text("Import a track to begin reviewing.")
                .height(28.0)
                .fill_width(),
            ui::spacer().fill(),
        ])
        .padding(18.0)
        .spacing(12.0)
        .fill();
    };

    let note_ratios = state
        .waveform
        .as_ref()
        .filter(|_| {
            !state.waveform_busy && state.waveform_track_id.as_deref() == Some(track.id.as_str())
        })
        .map(|waveform| {
            track
                .notes
                .iter()
                .filter_map(|note| {
                    waveform::ratio_for_millis(note.time_millis, waveform.duration_millis)
                        .map(|ratio| (ratio, note.done))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let hovered_note_ratio = note_ratio_for_id(state, &track, state.hovered_note_id.as_deref());
    let selected_note_ratio = note_ratio_for_id(state, &track, state.selected_note_id.as_deref());
    let cursor_ratio = state
        .waveform
        .as_ref()
        .filter(|_| {
            !state.waveform_busy && state.waveform_track_id.as_deref() == Some(track.id.as_str())
        })
        .and_then(|waveform| {
            waveform::ratio_for_millis(state.review_cursor_millis, waveform.duration_millis)
        });
    let draft_ratio = state
        .draft_note
        .as_ref()
        .filter(|draft| draft.note_id.is_none())
        .and_then(|draft| {
            state
                .waveform
                .as_ref()
                .filter(|_| {
                    !state.waveform_busy
                        && state.waveform_track_id.as_deref() == Some(track.id.as_str())
                })
                .and_then(|waveform| {
                    waveform::ratio_for_millis(draft.time_millis, waveform.duration_millis)
                })
        });
    let loop_selection = state
        .loop_selections
        .get(AuditionSource::Main)
        .map(|selection| (selection.start_ratio, selection.end_ratio));
    let waveform_view = if let Some(waveform) = state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(track.id.as_str()))
    {
        waveform::view_with_source_progress_and_loop(
            waveform::WaveformSource::Main,
            state.waveform_generation,
            Arc::new(waveform.clone()),
            cursor_ratio,
            draft_ratio,
            note_ratios,
            hovered_note_ratio,
            selected_note_ratio,
            loop_selection,
            state
                .waveform_busy
                .then_some(state.waveform_progress)
                .flatten(),
            |interaction| match interaction {
                waveform::WaveformInteraction::LoopDragStarted { ratio } => {
                    Message::WaveformLoopDragStarted { ratio }
                }
                waveform::WaveformInteraction::LoopDragMoved { ratio } => {
                    Message::WaveformLoopDragMoved { ratio }
                }
                waveform::WaveformInteraction::LoopDragEnded {
                    start_ratio,
                    end_ratio,
                } => Message::WaveformLoopDragEnded {
                    start_ratio,
                    end_ratio,
                },
                waveform::WaveformInteraction::LoopDragCancelled => {
                    Message::WaveformLoopDragCancelled
                }
                waveform::WaveformInteraction::Clicked { ratio, lower } => {
                    Message::WaveformClicked { ratio, lower }
                }
                waveform::WaveformInteraction::PlayheadDragStarted { ratio } => {
                    Message::WaveformPlayheadDragStarted { ratio }
                }
                waveform::WaveformInteraction::PlayheadDragMoved { ratio } => {
                    Message::WaveformPlayheadDragMoved { ratio }
                }
                waveform::WaveformInteraction::PlayheadDragEnded { ratio } => {
                    Message::WaveformPlayheadDragEnded { ratio }
                }
                waveform::WaveformInteraction::PlayheadDragCancelled => {
                    Message::WaveformPlayheadDragCancelled
                }
                waveform::WaveformInteraction::CommentDragStarted { ratio, note_index } => {
                    Message::CommentDragStarted { ratio, note_index }
                }
                waveform::WaveformInteraction::CommentDragMoved { ratio } => {
                    Message::CommentDragMoved { ratio }
                }
                waveform::WaveformInteraction::CommentDragEnded { ratio } => {
                    Message::CommentDragEnded { ratio }
                }
                waveform::WaveformInteraction::CommentDragCancelled => {
                    Message::CommentDragCancelled
                }
            },
        )
        .fill_width()
        .height(WAVEFORM_HEIGHT)
    } else {
        ui::column([
            ui::text(if state.waveform_busy {
                "Analyzing the real audio file…"
            } else {
                "Waveform unavailable for this file."
            })
            .height(28.0)
            .fill_width(),
            ui::text(
                "The imported path remains external to the native library; if it moved, re-import it.",
            )
                .wrap()
                .height(42.0)
                .fill_width()
                .subtle(),
        ])
        .padding(12.0)
        .spacing(8.0)
        .fill_width()
        .height(WAVEFORM_HEIGHT)
    };
    let (metadata, duration_millis) = state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(track.id.as_str()))
        .map_or_else(
            || (String::from("Audio analysis pending"), 0),
            |waveform| {
                (
                    format!(
                        "{} Hz · {} channel{} · {}",
                        waveform.sample_rate,
                        waveform.channels,
                        if waveform.channels == 1 { "" } else { "s" },
                        format_duration(waveform.duration_millis),
                    ),
                    waveform.duration_millis,
                )
            },
        );
    let meter_lufs = current_lufs_meter_value(state, &track.id);
    let waveform_with_meter = ui::row([
        chrome::lufs_meter(meter_lufs, state.waveform_busy)
            .width(68.0)
            .height(WAVEFORM_HEIGHT),
        waveform_view,
    ])
    .spacing(8.0)
    .fill_width()
    .height(WAVEFORM_HEIGHT);
    let reference_height = reference_section_height(state, &track);
    let waveform_pair_height = WAVEFORM_HEIGHT + WAVEFORM_SECTION_SPACING + reference_height;
    let waveform_pair = ui::column([
        waveform_with_meter,
        reference_waveform_section(state, &track),
    ])
    .spacing(WAVEFORM_SECTION_SPACING)
    .fill_width()
    .height(waveform_pair_height);
    let audition_source_control = if track.reference_path.is_some() {
        let main_choice = audition_source_choice(
            "MAIN",
            AuditionSource::Main,
            state.audition_source == AuditionSource::Main,
        );
        let reference_choice = audition_source_choice(
            "REF",
            AuditionSource::Reference,
            state.audition_source == AuditionSource::Reference,
        );
        ui::column([
            ui::column([ui::spacer().fill(), main_choice, ui::spacer().fill()])
                .height(WAVEFORM_HEIGHT),
            ui::spacer()
                .height(WAVEFORM_SECTION_SPACING + reference_height - REFERENCE_WAVEFORM_HEIGHT),
            ui::column([ui::spacer().fill(), reference_choice, ui::spacer().fill()])
                .height(REFERENCE_WAVEFORM_HEIGHT),
        ])
        .width(AUDITION_SOURCE_SELECTOR_WIDTH)
        .height(waveform_pair_height)
    } else {
        ui::spacer()
            .width(AUDITION_SOURCE_SELECTOR_WIDTH)
            .height(waveform_pair_height)
    };
    let waveform_with_source = ui::row([audition_source_control, waveform_pair])
        .spacing(8.0)
        .fill_width()
        .height(waveform_pair_height);

    let waveform_status = ui::text(format!(
        "{} · {metadata} · {} / {}",
        track.title,
        format_timestamp(state.transport_position_millis.min(duration_millis)),
        format_duration(duration_millis),
    ))
    .key(format!("review-track-status-{}", track.id))
    .truncate()
    .height(18.0)
    .fill_width()
    .subtle();
    let waveform_section = ui::column([waveform_with_source, waveform_status])
        .spacing(4.0)
        .fill_width();

    let content = ui::column([waveform_section, comments_panel(state, &track)])
        .padding(8.0)
        .spacing(8.0)
        .fill();
    ui::stack([ui::card().fill(), content]).fill()
}

fn reference_dropdown_paths(state: &AppState, track: &storage::Track) -> Vec<PathBuf> {
    let mut paths = state
        .library
        .reference_tracks
        .iter()
        .map(|reference| reference.path.clone())
        .collect::<Vec<_>>();
    if let Some(path) = track.reference_path.as_ref()
        && !paths.iter().any(|candidate| candidate == path)
    {
        paths.push(path.clone());
    }
    paths
}

fn review_reference_controls(state: &AppState, track: &storage::Track) -> ui::View<Message> {
    let menu_open = reference_menu_is_open(state, track);
    let selector_label = track
        .reference_path
        .as_ref()
        .map_or("Choose reference".to_owned(), |path| {
            reference_track_name(path)
        });
    let reference_id = track.id.clone();
    let selector = ui::dropdown_trigger(selector_label, menu_open)
        .toggle_message(Message::ToggleReferenceMenu(reference_id.clone()))
        .build()
        .key(format!("reference-dropdown-{}", track.id))
        .width(REFERENCE_MENU_WIDTH)
        .height(26.0)
        .pointer_target(
            ui::pointer_target(true)
                .pointer_move(false)
                .pointer_press(true)
                .pointer_release(false)
                .pointer_drop(false)
                .wheel(false)
                .filter_map(move |message| match message {
                    ui::PointerShieldMessage::PointerPress { position, .. } => {
                        Some(Message::ToggleReferenceMenuAt {
                            track_id: reference_id.clone(),
                            position,
                        })
                    }
                    _ => None,
                }),
        );
    let has_reference = track.reference_path.is_some();
    let action = ui::button(if has_reference {
        "Replace reference"
    } else {
        "Import reference"
    })
    .primary()
    .message(Message::ReferencePressed(track.id.clone()))
    .width(142.0)
    .height(26.0);
    let match_control = if state.reference_waveform.is_some() && !state.reference_waveform_busy {
        ui::button("MATCH REF")
            .active(state.reference_match_enabled)
            .message(Message::ToggleReferenceMatch)
            .width(112.0)
            .height(26.0)
    } else {
        ui::button("MATCH REF")
            .subtle()
            .message(Message::ToggleReferenceMatch)
            .width(112.0)
            .height(26.0)
    };
    ui::row([
        ui::text("REF")
            .style(ui::WidgetStyle::strong(ui::WidgetTone::Accent))
            .width(28.0)
            .height(26.0),
        selector,
        match_control,
        action,
    ])
    .spacing(8.0)
    .height(26.0)
}

fn review_global_controls(state: &AppState, track: &storage::Track) -> ui::View<Message> {
    let duration_millis = state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(track.id.as_str()))
        .map_or(0, |waveform| waveform.duration_millis);
    let shared_playing = state.transport_playing || state.reference_transport_playing;
    let play_control = ui::icon_button(review_transport_icon(
        if shared_playing {
            &REVIEW_PAUSE_ICON
        } else {
            &REVIEW_PLAY_ICON
        },
        shared_playing,
    ))
    .enabled(duration_millis > 0 && !state.waveform_busy)
    .active(shared_playing)
    .message(Message::TogglePlayback)
    .key("review-transport-play")
    .tooltip(if shared_playing {
        "Pause playback"
    } else {
        "Play track"
    })
    .size(30.0, 26.0);
    ui::row([
        review_reference_controls(state, track),
        ui::icon_button(review_transport_icon(&REVIEW_VOLUME_ICON, false))
            .bare()
            .focus(ui::FocusBehavior::None)
            .passive::<Message>()
            .key("review-transport-volume")
            .tooltip(format!(
                "Volume {:02}",
                (state.audition_volume * 100.0).round() as u32
            ))
            .size(20.0, 26.0),
        ui::slider(state.audition_volume)
            .primary()
            .compact()
            .track_height(5.0)
            .track_border()
            .message(Message::AuditionVolumeChanged)
            .key("native-audition-volume")
            .height(26.0)
            .width(180.0),
        play_control,
    ])
    .spacing(6.0)
    .height(26.0)
}

fn reference_dropdown_options(
    state: &AppState,
    track: &storage::Track,
) -> Vec<ui::DropdownOption<Message>> {
    let selected_path = track.reference_path.as_ref();
    let track_id = track.id.clone();
    reference_dropdown_paths(state, track)
        .into_iter()
        .map(|path| {
            let selected = selected_path == Some(&path);
            ui::DropdownOption::new(
                reference_track_name(&path),
                selected,
                Message::SetReferenceTrack {
                    track_id: track_id.clone(),
                    path,
                },
            )
        })
        .collect()
}

fn reference_menu_is_open(state: &AppState, track: &storage::Track) -> bool {
    state.reference_menu_track_id.as_deref() == Some(track.id.as_str())
        && !reference_dropdown_paths(state, track).is_empty()
}

fn reference_section_height(state: &AppState, track: &storage::Track) -> f32 {
    let _ = (state, track);
    REFERENCE_HEADER_HEIGHT + REFERENCE_SECTION_SPACING + REFERENCE_WAVEFORM_HEIGHT
}

fn reference_waveform_section(state: &AppState, track: &storage::Track) -> ui::View<Message> {
    let has_reference = track.reference_path.is_some();
    let reference_waveform = state
        .reference_waveform
        .as_ref()
        .filter(|_| state.reference_waveform_track_id.as_deref() == Some(track.id.as_str()));
    let reference_cursor_ratio = reference_waveform
        .filter(|_| !state.reference_waveform_busy)
        .and_then(|waveform| {
            waveform::ratio_for_millis(
                state.reference_transport_position_millis,
                waveform.duration_millis,
            )
        });
    let reference_integrated_lufs =
        reference_waveform.and_then(|waveform| waveform.integrated_lufs);
    let reference_loop_bounds = reference_waveform.and_then(|waveform| {
        state
            .loop_selections
            .get(AuditionSource::Reference)
            .and_then(|selection| projected_loop_bounds(selection, waveform.duration_millis))
    });
    let reference_meter_lufs = current_reference_lufs_meter_value(state, &track.id);
    let reference_name = track.reference_path.as_ref().map_or_else(
        || String::from("No reference track"),
        |path| reference_track_name(path),
    );
    let reference_label = if let Some(reference_lufs) = reference_integrated_lufs {
        format!("REFERENCE · {reference_name} · {reference_lufs:.1} LUFS")
    } else if has_reference && state.reference_waveform_busy {
        format!("REFERENCE · {reference_name} · BUILDING")
    } else if has_reference {
        format!("REFERENCE · {reference_name}")
    } else {
        String::from("REFERENCE TRACK")
    };
    let reference_label = if let Some((start_millis, end_millis)) = reference_loop_bounds {
        format!(
            "{reference_label} · LOOP {}–{}",
            format_timestamp(start_millis),
            format_timestamp(end_millis),
        )
    } else {
        reference_label
    };
    let reference_body = if let Some(waveform) = reference_waveform {
        let loop_selection = state
            .loop_selections
            .get(AuditionSource::Reference)
            .map(|selection| (selection.start_ratio, selection.end_ratio));
        let notes = reference_notes_for_track(&state.library, track);
        let note_ratios = notes
            .iter()
            .filter_map(|note| {
                waveform::ratio_for_millis(note.time_millis, waveform.duration_millis)
                    .map(|ratio| (ratio, note.done))
            })
            .collect::<Vec<_>>();
        let draft_ratio = state
            .reference_draft_note
            .as_ref()
            .filter(|draft| draft.note_id.is_none())
            .and_then(|draft| {
                waveform::ratio_for_millis(draft.time_millis, waveform.duration_millis)
            });
        let hovered_note_ratio =
            reference_note_ratio_for_id(state, track, state.hovered_reference_note_id.as_deref());
        waveform::view_with_source_progress_and_loop(
            waveform::WaveformSource::Reference,
            state.reference_waveform_generation,
            Arc::new(waveform.clone()),
            reference_cursor_ratio,
            draft_ratio,
            note_ratios,
            hovered_note_ratio,
            reference_note_ratio_for_id(state, track, state.selected_reference_note_id.as_deref()),
            loop_selection,
            state
                .reference_waveform_busy
                .then_some(state.reference_waveform_progress)
                .flatten(),
            |interaction| match interaction {
                waveform::WaveformInteraction::LoopDragStarted { ratio } => {
                    Message::ReferenceLoopDragStarted { ratio }
                }
                waveform::WaveformInteraction::LoopDragMoved { ratio } => {
                    Message::ReferenceLoopDragMoved { ratio }
                }
                waveform::WaveformInteraction::LoopDragEnded {
                    start_ratio,
                    end_ratio,
                } => Message::ReferenceLoopDragEnded {
                    start_ratio,
                    end_ratio,
                },
                waveform::WaveformInteraction::LoopDragCancelled => {
                    Message::ReferenceLoopDragCancelled
                }
                waveform::WaveformInteraction::Clicked { ratio, lower } => {
                    if lower {
                        Message::ReferenceCommentClicked { ratio }
                    } else {
                        Message::ReferenceWaveformClicked { ratio }
                    }
                }
                waveform::WaveformInteraction::PlayheadDragStarted { ratio } => {
                    Message::ReferencePlayheadDragStarted { ratio }
                }
                waveform::WaveformInteraction::PlayheadDragMoved { ratio } => {
                    Message::ReferencePlayheadDragMoved { ratio }
                }
                waveform::WaveformInteraction::PlayheadDragEnded { ratio } => {
                    Message::ReferencePlayheadDragEnded { ratio }
                }
                waveform::WaveformInteraction::PlayheadDragCancelled => {
                    Message::ReferencePlayheadDragCancelled
                }
                waveform::WaveformInteraction::CommentDragStarted { ratio, note_index } => {
                    Message::ReferenceCommentDragStarted { ratio, note_index }
                }
                waveform::WaveformInteraction::CommentDragMoved { ratio } => {
                    Message::ReferenceCommentDragMoved { ratio }
                }
                waveform::WaveformInteraction::CommentDragEnded { ratio } => {
                    Message::ReferenceCommentDragEnded { ratio }
                }
                waveform::WaveformInteraction::CommentDragCancelled => {
                    Message::ReferenceCommentDragCancelled
                }
            },
        )
        .fill_width()
        .height(REFERENCE_WAVEFORM_HEIGHT)
    } else {
        let message = if state.reference_waveform_busy && state.waveform_busy {
            "Reference queued behind main analysis…"
        } else if state.reference_waveform_busy {
            "Analyzing reference waveform…"
        } else if has_reference {
            "Reference waveform unavailable for this file."
        } else {
            "Import a reference track to compare it below."
        };
        ui::column([ui::text(message).height(24.0).fill_width()])
            .padding(10.0)
            .fill_width()
            .height(REFERENCE_WAVEFORM_HEIGHT)
    };
    let reference_meter = if has_reference {
        chrome::lufs_meter(reference_meter_lufs, state.reference_waveform_busy)
            .width(68.0)
            .height(REFERENCE_WAVEFORM_HEIGHT)
    } else {
        ui::spacer().width(68.0).height(REFERENCE_WAVEFORM_HEIGHT)
    };
    let body = ui::row([reference_meter, reference_body])
        .spacing(0.0)
        .fill_width()
        .height(REFERENCE_WAVEFORM_HEIGHT);
    let header = ui::row([
        ui::spacer().width(68.0).height(REFERENCE_HEADER_HEIGHT),
        ui::text(reference_label)
            .truncate()
            .height(REFERENCE_HEADER_HEIGHT)
            .fill_width()
            .subtle(),
    ])
    .spacing(8.0)
    .fill_width()
    .height(REFERENCE_HEADER_HEIGHT);
    ui::column([header, body])
        .spacing(REFERENCE_SECTION_SPACING)
        .fill_width()
        .height(reference_section_height(state, track))
}

fn comments_panel(state: &AppState, track: &storage::Track) -> ui::View<Message> {
    let reference_available = track.reference_path.is_some();
    let reference_notes = reference_notes_for_track(&state.library, track);
    let source = if reference_available {
        if state.comment_source == CommentSource::Reference
            || (!state.comment_source_explicit
                && track.notes.is_empty()
                && !reference_notes.is_empty())
        {
            CommentSource::Reference
        } else {
            CommentSource::Main
        }
    } else {
        CommentSource::Main
    };
    let (notes, selected_note_id, empty_message) = match source {
        CommentSource::Main => (
            track.notes.clone(),
            state.selected_note_id.clone(),
            "Click the lower main waveform rail to add a comment for this file.",
        ),
        CommentSource::Reference => (
            reference_notes.to_vec(),
            state.selected_reference_note_id.clone(),
            "Click the lower reference waveform rail to add a comment for this file.",
        ),
    };
    let open_count = notes.iter().filter(|note| !note.done).count();
    let tabs = ui::row([
        ui::button("MAIN")
            .selected(source == CommentSource::Main)
            .message(Message::SelectCommentSource(CommentSource::Main))
            .key("comments-tab-main")
            .width(76.0)
            .height(28.0),
        if reference_available {
            ui::button("REFERENCE")
                .selected(source == CommentSource::Reference)
                .message(Message::SelectCommentSource(CommentSource::Reference))
                .key("comments-tab-reference")
                .width(112.0)
                .height(28.0)
        } else {
            ui::text("REFERENCE").subtle().width(112.0).height(28.0)
        },
    ])
    .spacing(4.0)
    .height(28.0);
    let mut children = vec![
        ui::row([
            ui::text("COMMENTS")
                .style(ui::WidgetStyle::strong(ui::WidgetTone::Neutral))
                .width(104.0)
                .height(28.0),
            tabs,
            ui::spacer().fill_width(),
            ui::text(format!("{} total · {} open", notes.len(), open_count))
                .height(28.0)
                .subtle(),
        ])
        .fill_width()
        .spacing(8.0),
    ];
    if notes.is_empty() {
        children.push(
            ui::text(empty_message)
                .wrap()
                .height(42.0)
                .fill_width()
                .subtle(),
        );
    } else {
        let note_count = notes.len();
        let selected_note_id = selected_note_id.clone();
        let source_for_rows = source;
        let editing_note = match source {
            CommentSource::Main => state
                .draft_note
                .clone()
                .filter(|draft| draft.note_id.is_some()),
            CommentSource::Reference => state
                .reference_draft_note
                .clone()
                .filter(|draft| draft.note_id.is_some()),
        };
        let list = ui::list(notes.into_iter().enumerate(), move |(index, note)| {
            if source_for_rows == CommentSource::Main {
                note_row(
                    index,
                    note,
                    selected_note_id.as_deref(),
                    editing_note.as_ref(),
                )
            } else {
                reference_note_row(
                    index,
                    note,
                    selected_note_id.as_deref(),
                    editing_note.as_ref(),
                )
            }
        })
        .without_chrome()
        .fill_width()
        .height(note_count as f32 * 44.0);
        children.push(list);
    }
    if source == CommentSource::Main {
        if let Some(draft) = state
            .draft_note
            .as_ref()
            .filter(|draft| draft.note_id.is_none())
        {
            children.push(note_editor(draft));
        }
    } else if let Some(draft) = state
        .reference_draft_note
        .as_ref()
        .filter(|draft| draft.note_id.is_none())
    {
        children.push(reference_note_editor(draft));
    }
    let content = ui::column(children)
        .padding(12.0)
        .spacing(8.0)
        .fill_width()
        .fill_height();
    ui::stack([ui::card().fill(), content])
        .fill_width()
        .fill_height()
}

fn note_editor(draft: &NoteDraft) -> ui::View<Message> {
    ui::column([
        ui::text(format!(
            "COMMENT AT {}",
            format_timestamp(draft.time_millis)
        ))
        .height(20.0)
        .fill_width()
        .subtle(),
        ui::text_input(draft.body.clone())
            .placeholder("Write a comment…")
            .message_event(|input| {
                let submitted = input.is_submitted();
                let value = input.into_value();
                if submitted {
                    Message::SaveDraftNote
                } else {
                    Message::DraftNoteChanged(value)
                }
            })
            .id(MAIN_COMMENT_EDITOR_ID)
            .fill_width()
            .height(38.0),
        ui::row([
            ui::button("Save comment")
                .primary()
                .message(Message::SaveDraftNote)
                .height(28.0),
            ui::button("Cancel")
                .subtle()
                .message(Message::CancelDraftNote)
                .height(28.0),
        ])
        .spacing(8.0)
        .fill_width(),
    ])
    .padding(10.0)
    .spacing(8.0)
    .fill_width()
}

fn reference_note_editor(draft: &NoteDraft) -> ui::View<Message> {
    ui::column([
        ui::text(format!(
            "REFERENCE COMMENT AT {}",
            format_timestamp(draft.time_millis)
        ))
        .height(20.0)
        .fill_width()
        .subtle(),
        ui::text_input(draft.body.clone())
            .placeholder("Write a reference comment…")
            .message_event(|input| {
                let submitted = input.is_submitted();
                let value = input.into_value();
                if submitted {
                    Message::SaveReferenceDraftNote
                } else {
                    Message::ReferenceDraftNoteChanged(value)
                }
            })
            .id(REFERENCE_COMMENT_EDITOR_ID)
            .fill_width()
            .height(38.0),
        ui::row([
            ui::button("Save reference comment")
                .primary()
                .message(Message::SaveReferenceDraftNote)
                .height(28.0),
            ui::button("Cancel")
                .subtle()
                .message(Message::CancelReferenceDraftNote)
                .height(28.0),
        ])
        .spacing(8.0)
        .fill_width(),
    ])
    .padding(10.0)
    .spacing(8.0)
    .fill_width()
}

fn note_row(
    index: usize,
    note: storage::Note,
    selected_note_id: Option<&str>,
    editing_note: Option<&NoteDraft>,
) -> ui::View<Message> {
    let selected = selected_note_id == Some(note.id.as_str());
    let editing =
        editing_note.is_some_and(|draft| draft.note_id.as_deref() == Some(note.id.as_str()));
    let note_id = note.id.clone();
    let note_body = note.body.clone();
    let note_time_millis = note.time_millis;
    let note_done = note.done;
    let body = if editing {
        let draft = editing_note.expect("an editing row should have a matching draft");
        ui::text_input(draft.body.clone())
            .message_event(|input| {
                if input.is_submitted() {
                    Message::SaveDraftNote
                } else {
                    Message::DraftNoteChanged(input.into_value())
                }
            })
            .id(main_inline_comment_editor_id(&note_id))
            .fill_width()
            .height(30.0)
    } else {
        ui::text(note_body).wrap().height(30.0).fill_width()
    };
    let edit_or_save = if editing {
        ui::button("Save")
            .primary()
            .message(Message::SaveDraftNote)
            .height(28.0)
    } else {
        ui::button("Edit")
            .selected(selected)
            .message(Message::EditNote(note_id.clone()))
            .height(28.0)
    };
    let cancel_or_delete = if editing {
        ui::button("Cancel")
            .subtle()
            .message(Message::CancelDraftNote)
            .height(28.0)
    } else {
        ui::button("Delete")
            .selected(selected)
            .message(Message::DeleteNote(note_id.clone()))
            .height(28.0)
    };
    let row = ui::list_row(
        index,
        [
            ui::text(format_timestamp(note_time_millis))
                .height(30.0)
                .width(68.0)
                .subtle(),
            body,
            ui::button(if note_done { "Done" } else { "Open" })
                .selected(selected)
                .message(Message::ToggleNoteDone(note_id.clone()))
                .height(28.0),
            edit_or_save,
            cancel_or_delete,
        ],
    )
    .fill_width();
    let row_key = note_id.clone();
    let hover_id = note_id.clone();
    let double_edit_id = note_id.clone();
    let row_actions = ui::row_actions()
        .primary(move || Message::SelectNote(row_key.clone()))
        .double_activate(move || Message::EditNote(double_edit_id.clone()));
    let row_surface = ui::interactive_row_underlay(row)
        .selected(selected)
        .stable_row_identity(0xCAD3_0002, note_id.clone())
        .actions(row_actions)
        .fill_width()
        .height(44.0);
    ui::stack([
        row_surface,
        chrome::comment_hover(
            Message::CommentHoverStarted(hover_id.clone()),
            Message::CommentHoverEnded(hover_id),
        )
        .key(format!("comment-hover-{}", note_id))
        .fill(),
    ])
    .fill_width()
    .height(44.0)
}

fn reference_note_row(
    index: usize,
    note: storage::Note,
    selected_note_id: Option<&str>,
    editing_note: Option<&NoteDraft>,
) -> ui::View<Message> {
    let selected = selected_note_id == Some(note.id.as_str());
    let editing =
        editing_note.is_some_and(|draft| draft.note_id.as_deref() == Some(note.id.as_str()));
    let note_id = note.id.clone();
    let note_body = note.body.clone();
    let note_time_millis = note.time_millis;
    let note_done = note.done;
    let body = if editing {
        let draft = editing_note.expect("an editing reference row should have a matching draft");
        ui::text_input(draft.body.clone())
            .message_event(|input| {
                if input.is_submitted() {
                    Message::SaveReferenceDraftNote
                } else {
                    Message::ReferenceDraftNoteChanged(input.into_value())
                }
            })
            .id(reference_inline_comment_editor_id(&note_id))
            .fill_width()
            .height(30.0)
    } else {
        ui::text(note_body).wrap().height(30.0).fill_width()
    };
    let edit_or_save = if editing {
        ui::button("Save")
            .primary()
            .message(Message::SaveReferenceDraftNote)
            .height(28.0)
    } else {
        ui::button("Edit")
            .selected(selected)
            .message(Message::EditReferenceNote(note_id.clone()))
            .height(28.0)
    };
    let cancel_or_delete = if editing {
        ui::button("Cancel")
            .subtle()
            .message(Message::CancelReferenceDraftNote)
            .height(28.0)
    } else {
        ui::button("Delete")
            .selected(selected)
            .message(Message::DeleteReferenceNote(note_id.clone()))
            .height(28.0)
    };
    let row = ui::list_row(
        index,
        [
            ui::text(format_timestamp(note_time_millis))
                .height(30.0)
                .width(68.0)
                .subtle(),
            body,
            ui::button(if note_done { "Done" } else { "Open" })
                .selected(selected)
                .message(Message::ToggleReferenceNoteDone(note_id.clone()))
                .height(28.0),
            edit_or_save,
            cancel_or_delete,
        ],
    )
    .fill_width();
    let row_key = note_id.clone();
    let double_edit_id = note_id.clone();
    let hover_id = note_id.clone();
    let hover_key = note_id.clone();
    let row_actions = ui::row_actions()
        .primary(move || Message::SelectReferenceNote(row_key.clone()))
        .double_activate(move || Message::EditReferenceNote(double_edit_id.clone()));
    let row_surface = ui::interactive_row_underlay(row)
        .selected(selected)
        .stable_row_identity(0xCAD3_0003, note_id)
        .actions(row_actions)
        .fill_width()
        .height(44.0);
    ui::stack([
        row_surface,
        chrome::comment_hover(
            Message::ReferenceCommentHoverStarted(hover_id.clone()),
            Message::ReferenceCommentHoverEnded(hover_id),
        )
        .key(format!("reference-comment-hover-{hover_key}"))
        .fill(),
    ])
    .fill_width()
    .height(44.0)
}

fn selected_track(state: &AppState) -> Option<&storage::Track> {
    state
        .library
        .selected_track_id
        .as_ref()
        .and_then(|id| state.library.tracks.iter().find(|track| &track.id == id))
}

fn reference_notes_for_track<'a>(
    library: &'a storage::Library,
    track: &storage::Track,
) -> &'a [storage::Note] {
    let Some(path) = track.reference_path.as_ref() else {
        return &[];
    };
    library
        .reference_tracks
        .iter()
        .find(|reference| reference.path == *path)
        .map_or(&[], |reference| reference.notes.as_slice())
}

fn selected_reference_notes(state: &AppState) -> &[storage::Note] {
    selected_track(state)
        .map(|track| reference_notes_for_track(&state.library, track))
        .unwrap_or(&[])
}

fn selected_reference_track_mut(state: &mut AppState) -> Option<&mut storage::ReferenceTrack> {
    let selected_id = state.library.selected_track_id.as_ref()?.clone();
    let path = state
        .library
        .tracks
        .iter()
        .find(|track| track.id == selected_id)
        .and_then(|track| track.reference_path.clone())?;
    state
        .library
        .reference_tracks
        .iter_mut()
        .find(|reference| reference.path == path)
}

fn selected_reference_note_mut<'a>(
    state: &'a mut AppState,
    note_id: &str,
) -> Option<&'a mut storage::Note> {
    selected_reference_track_mut(state)?
        .notes
        .iter_mut()
        .find(|note| note.id == note_id)
}

fn reference_track_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| path.to_string_lossy().into_owned(), ToOwned::to_owned)
}

fn note_ratio_for_id(
    state: &AppState,
    track: &storage::Track,
    note_id: Option<&str>,
) -> Option<f32> {
    let note = note_id.and_then(|id| track.notes.iter().find(|note| note.id == id))?;
    let waveform = state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(track.id.as_str()))?;
    waveform::ratio_for_millis(note.time_millis, waveform.duration_millis)
}

fn reference_note_ratio_for_id(
    state: &AppState,
    track: &storage::Track,
    note_id: Option<&str>,
) -> Option<f32> {
    let note = note_id.and_then(|id| {
        reference_notes_for_track(&state.library, track)
            .iter()
            .find(|note| note.id == id)
    })?;
    let waveform = state
        .reference_waveform
        .as_ref()
        .filter(|_| state.reference_waveform_track_id.as_deref() == Some(track.id.as_str()))?;
    waveform::ratio_for_millis(note.time_millis, waveform.duration_millis)
}

fn selected_track_mut(state: &mut AppState) -> Option<&mut storage::Track> {
    let selected_id = state.library.selected_track_id.as_ref()?.clone();
    state
        .library
        .tracks
        .iter_mut()
        .find(|track| track.id == selected_id)
}

fn decode_result_is_current(state: &AppState, track_id: &str, generation: u64) -> bool {
    state.library.selected_track_id.as_deref() == Some(track_id)
        && state.waveform_generation == generation
}

fn reference_decode_result_is_current(state: &AppState, track_id: &str, generation: u64) -> bool {
    state.library.selected_track_id.as_deref() == Some(track_id)
        && state.reference_waveform_generation == generation
}

fn transport_command_is_confirmed(snapshot: transport::Snapshot, token: u64) -> bool {
    snapshot.acknowledged_token >= token
}

fn unique_note_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("note-{nanos}")
}

fn format_timestamp(time_millis: u64) -> String {
    let total_seconds = time_millis / 1_000;
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

fn format_duration(duration_millis: u64) -> String {
    let total_seconds = duration_millis / 1_000;
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::{
        AppState, AuditionSource, ImportBatchProgress, LoopBounds, LoopSelection, LoopSelections,
        Message, NoteDraft, REFERENCE_MENU_WIDTH, StatusMenuHost,
        TITLEBAR_TRAFFIC_LIGHT_SAFE_GUTTER, WAVEFORM_HEIGHT, WorkspaceMode,
        apply_transport_snapshot, audition_panel, audition_shuffle_seed, audition_statuses,
        current_loudness_match_gain_db, current_lufs_meter_value,
        current_reference_lufs_meter_value, decode_result_is_current, deterministic_shuffle,
        enforce_loop, loop_bounds, main_output_gain, native_launch_options, note_editor,
        note_ratio_for_id, planner_drop_is_valid, playback_shortcut, project_surface,
        rebuild_audition_queue, reconcile_audition_queue, reference_decode_result_is_current,
        reference_output_gain, review_status_filter_message, selected_reference_notes,
        selected_track, stage_dropdown, stage_menu_anchor_from_pointer, stage_menu_popover,
        status_dropdown_for_host, status_filter_dropdown, sync_audition_queue_after_status_change,
        tracks_in_stage, tracks_with_status, transport_command_is_confirmed, update,
    };
    use crate::transport::Snapshot;
    use crate::{
        audio::{LoudnessPoint, WaveformData},
        storage::{Library, Note, ReferenceTrack, Track, TrackStage, TrackStatus},
        transport, waveform,
    };
    use radiant::{
        application::IntoView,
        gui::types::{Point, Rect, Vector2},
        prelude as ui,
        runtime::{
            DeclarativeOwnedRuntimeBridge, Event, FocusTraversal, PaintPrimitive,
            RuntimeUpdateSnapshot, SurfaceRuntime,
        },
        theme::ThemeTokens,
    };
    use std::{path::PathBuf, sync::Arc, time::Duration};

    fn planner_drag_state(pointer: Point) -> AppState {
        let mut state = AppState {
            busy: false,
            workspace_mode: super::WorkspaceMode::Planner,
            planner_drag_source_track_id: Some(String::from("drag")),
            planner_drag_pointer: Some(pointer),
            ..AppState::default()
        };
        state.library.tracks.push(Track {
            id: String::from("drag"),
            title: String::from("Preview me"),
            original_name: String::from("preview.wav"),
            path: PathBuf::from("/external/preview.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        state
    }

    fn audition_track(id: &str) -> Track {
        Track {
            id: String::from(id),
            title: String::from(id),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            reference_path: Some(PathBuf::from(format!("/external/{id}-reference.wav"))),
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        }
    }

    fn audition_waveform() -> WaveformData {
        WaveformData {
            sample_rate: 48_000,
            channels: 1,
            duration_millis: 1_000,
            render_frames: 48_000,
            integrated_lufs: Some(-7.0),
            loudness_profile: Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.0, 0.5, 0.0, 0.5],
                    4,
                    1,
                ),
            ),
        }
    }

    fn audition_state(ids: &[&str]) -> AppState {
        let queue = ids.iter().map(|id| String::from(*id)).collect::<Vec<_>>();
        let selected_id = queue.first().cloned();
        let mut state = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Audition,
            audition_queue: queue,
            audition_queue_index: 0,
            waveform: selected_id.as_ref().map(|_| audition_waveform()),
            waveform_track_id: selected_id.clone(),
            ..AppState::default()
        };
        state.library.selected_track_id = selected_id;
        state.library.tracks = ids.iter().map(|id| audition_track(id)).collect();
        state
    }

    fn dropdown_surface_rect(primitives: &[PaintPrimitive], anchor: Point) -> Option<Rect> {
        primitives.iter().find_map(|primitive| match primitive {
            PaintPrimitive::FillPolygon(fill)
                if fill.color == ThemeTokens::default().surface_overlay =>
            {
                let min_x = fill
                    .points
                    .iter()
                    .map(|point| point.x)
                    .fold(f32::INFINITY, f32::min);
                let min_y = fill
                    .points
                    .iter()
                    .map(|point| point.y)
                    .fold(f32::INFINITY, f32::min);
                let max_x = fill
                    .points
                    .iter()
                    .map(|point| point.x)
                    .fold(f32::NEG_INFINITY, f32::max);
                let max_y = fill
                    .points
                    .iter()
                    .map(|point| point.y)
                    .fold(f32::NEG_INFINITY, f32::max);
                let rect = Rect::from_min_max(Point::new(min_x, min_y), Point::new(max_x, max_y));
                ((rect.min.x - anchor.x).abs() < 0.01 && (rect.min.y - anchor.y).abs() < 0.01)
                    .then_some(rect)
            }
            PaintPrimitive::FillRect(fill)
                if fill.color == ThemeTokens::default().surface_overlay
                    && (fill.rect.min.x - anchor.x).abs() < 0.01
                    && (fill.rect.min.y - anchor.y).abs() < 0.01 =>
            {
                Some(fill.rect)
            }
            _ => None,
        })
    }

    fn planner_drag_preview_rect(state: &AppState) -> Option<Rect> {
        project_surface(state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0))
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::Text(text) if text.text.as_str() == "↕ Preview me" => {
                    Some(text.rect)
                }
                _ => None,
            })
    }

    #[test]
    fn unmodified_space_maps_to_toggle_playback() {
        let state = AppState::default();

        assert_eq!(
            playback_shortcut(&state, ui::KeyPress::new(ui::KeyCode::Space)),
            ui::ShortcutResolution::action(Message::TogglePlayback)
        );
    }

    #[test]
    fn unmodified_n_maps_to_a_new_note_at_the_current_time() {
        let state = AppState::default();

        assert_eq!(
            playback_shortcut(&state, ui::KeyPress::new(ui::KeyCode::N)),
            ui::ShortcutResolution::action(Message::NewNoteAtCurrentTime)
        );
    }

    #[test]
    fn note_shortcut_creates_a_draft_for_the_current_audition_track_and_focuses_editor() {
        let mut state = audition_state(&["note-hotkey-track"]);
        state.review_cursor_millis = 640;
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::NewNoteAtCurrentTime, &mut context);

        let focus_command = context.into_command();
        assert!(matches!(
            focus_command,
            radiant::runtime::Command::Batch(commands)
                if commands.iter().any(|command| matches!(
                    command,
                    radiant::runtime::Command::Focus(id)
                        if *id == super::MAIN_COMMENT_EDITOR_ID
                ))
        ));
        assert_eq!(
            state.draft_note.as_ref().map(|draft| draft.time_millis),
            Some(640)
        );
        assert_eq!(state.status, "Comment at 00:00 — type a note below.");
    }

    #[test]
    fn editing_existing_comment_populates_draft_and_focuses_main_editor() {
        let track_id = String::from("edit-track");
        let note_id = String::from("edit-note");
        let mut state = AppState {
            busy: false,
            review_cursor_millis: 3_000,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Edit track"),
            original_name: String::from("edit-track.wav"),
            path: PathBuf::from("/external/edit-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: vec![Note {
                id: note_id.clone(),
                time_millis: 1_250,
                body: String::from("Existing comment body"),
                done: false,
            }],
        });
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::EditNote(note_id.clone()), &mut context);

        assert_eq!(state.selected_note_id.as_deref(), Some(note_id.as_str()));
        assert_eq!(state.review_cursor_millis, 1_250);
        let draft = state
            .draft_note
            .as_ref()
            .expect("editing an existing comment should open the draft");
        assert_eq!(draft.note_id.as_deref(), Some(note_id.as_str()));
        assert_eq!(draft.time_millis, 1_250);
        assert_eq!(draft.body, "Existing comment body");

        let frame = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 1000.0));
        assert!(frame.paint_plan.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                PaintPrimitive::TextInput(input)
                    if input.widget_id == super::main_inline_comment_editor_id(&note_id)
                        && input.state.value.as_str() == "Existing comment body"
            )
        }));
        let labels = frame.paint_plan.text_label_strings();
        assert!(!labels.iter().any(|label| label.starts_with("COMMENT AT ")));
        assert!(labels.iter().any(|label| label == "Save"));
        assert!(labels.iter().any(|label| label == "Cancel"));

        let focus_command = context.into_command();
        assert!(matches!(
            focus_command,
            radiant::runtime::Command::Batch(commands)
                if commands.iter().any(|command| matches!(
                    command,
                    radiant::runtime::Command::Focus(id)
                        if *id == super::main_inline_comment_editor_id(&note_id)
                ))
        ));
    }

    #[test]
    fn editing_existing_reference_comment_populates_draft_and_focuses_reference_editor() {
        let track_id = String::from("reference-edit-track");
        let note_id = String::from("reference-edit-note");
        let reference_path = PathBuf::from("/external/reference-edit.wav");
        let mut state = AppState {
            busy: false,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Reference edit track"),
            original_name: String::from("reference-edit-main.wav"),
            path: PathBuf::from("/external/reference-edit-main.wav"),
            reference_path: Some(reference_path.clone()),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        state.library.reference_tracks.push(ReferenceTrack {
            path: reference_path,
            notes: vec![Note {
                id: note_id.clone(),
                time_millis: 875,
                body: String::from("Existing reference body"),
                done: true,
            }],
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::EditReferenceNote(note_id.clone()),
            &mut context,
        );

        assert_eq!(
            state.selected_reference_note_id.as_deref(),
            Some(note_id.as_str())
        );
        let draft = state
            .reference_draft_note
            .as_ref()
            .expect("editing an existing reference comment should open the draft");
        assert_eq!(draft.note_id.as_deref(), Some(note_id.as_str()));
        assert_eq!(draft.time_millis, 875);
        assert_eq!(draft.body, "Existing reference body");

        let frame = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 1100.0));
        assert!(frame.paint_plan.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                PaintPrimitive::TextInput(input)
                    if input.widget_id == super::reference_inline_comment_editor_id(&note_id)
                        && input.state.value.as_str() == "Existing reference body"
            )
        }));
        let labels = frame.paint_plan.text_label_strings();
        assert!(
            !labels
                .iter()
                .any(|label| label.starts_with("REFERENCE COMMENT AT "))
        );
        assert!(labels.iter().any(|label| label == "Save"));
        assert!(labels.iter().any(|label| label == "Cancel"));

        let focus_command = context.into_command();
        assert!(matches!(
            focus_command,
            radiant::runtime::Command::Batch(commands)
                if commands.iter().any(|command| matches!(
                    command,
                    radiant::runtime::Command::Focus(id)
                        if *id == super::reference_inline_comment_editor_id(&note_id)
                ))
        ));
    }

    #[test]
    fn project_surface_exposes_shell_context_and_playback_hints() {
        let labels = project_surface(&AppState::default())
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0))
            .paint_plan
            .text_label_strings();

        assert!(!labels.iter().any(|label| label == "PORTALSURFER / CADENCE"));
        assert!(!labels.iter().any(|label| label == "LOCAL REVIEW DESK"));

        for label in [
            "Review",
            "Planner",
            "Audition",
            "SPACE  play · ESC  stop · N  note",
        ] {
            assert!(
                labels.iter().any(|painted| painted == label),
                "missing {label:?}"
            );
        }
        assert!(!labels.iter().any(|label| label == "NATIVE · RADIANT"));
    }

    #[test]
    fn playback_shortcut_maps_unmodified_escape_to_stop_playback() {
        let state = AppState::default();

        assert_eq!(
            playback_shortcut(&state, ui::KeyPress::new(ui::KeyCode::Escape)),
            ui::ShortcutResolution::action(Message::StopPlayback)
        );
    }

    #[test]
    fn playhead_drag_tracks_pointer_and_starts_playback_on_release() {
        let mut state = AppState {
            busy: false,
            waveform: Some(WaveformData {
                sample_rate: 48_000,
                channels: 1,
                duration_millis: 1_000,
                render_frames: 48_000,
                integrated_lufs: Some(-7.0),
                loudness_profile: std::sync::Arc::from([]),
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.1, 0.8, 0.2, 0.4],
                        4,
                        1,
                    ),
                ),
            }),
            draft_note: Some(NoteDraft {
                note_id: None,
                time_millis: 100,
                body: String::from("old draft"),
            }),
            ..AppState::default()
        };
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformPlayheadDragStarted { ratio: 0.25 },
            &mut context,
        );
        assert!(state.playhead_drag_active);
        assert_eq!(state.review_cursor_millis, 250);
        assert_eq!(state.transport_position_millis, 250);
        assert!(state.draft_note.is_none());

        update(
            &mut state,
            Message::WaveformPlayheadDragMoved { ratio: 0.75 },
            &mut context,
        );
        assert_eq!(state.review_cursor_millis, 750);
        assert_eq!(state.transport_position_millis, 750);

        update(
            &mut state,
            Message::WaveformPlayheadDragEnded { ratio: 0.5 },
            &mut context,
        );
        assert!(!state.playhead_drag_active);
        assert_eq!(state.review_cursor_millis, 500);
        assert_eq!(state.transport_position_millis, 500);
        assert!(state.transport_polling);
        assert!(state.transport_waiting_token.is_some());
        assert_eq!(state.status, "Playing from 00:00.");
    }

    #[test]
    fn playhead_drag_at_ratio_zero_starts_playback_from_the_start() {
        let mut state = AppState {
            busy: false,
            waveform: Some(WaveformData {
                sample_rate: 48_000,
                channels: 1,
                duration_millis: 1_000,
                render_frames: 48_000,
                integrated_lufs: Some(-7.0),
                loudness_profile: std::sync::Arc::from([]),
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.1, 0.8, 0.2, 0.4],
                        4,
                        1,
                    ),
                ),
            }),
            ..AppState::default()
        };
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformPlayheadDragStarted { ratio: 0.0 },
            &mut context,
        );
        assert!(state.playhead_drag_active);
        assert_eq!(state.review_cursor_millis, 0);
        assert_eq!(state.transport_position_millis, 0);

        update(
            &mut state,
            Message::WaveformPlayheadDragEnded { ratio: 0.0 },
            &mut context,
        );
        assert!(!state.playhead_drag_active);
        assert_eq!(state.review_cursor_millis, 0);
        assert_eq!(state.transport_position_millis, 0);
        assert!(state.transport_polling);
        assert!(state.transport_waiting_token.is_some());
        assert_eq!(state.status, "Playing from 00:00.");
    }

    #[test]
    fn playhead_drag_preserves_cursor_when_a_ready_snapshot_is_stale() {
        let mut state = AppState {
            playhead_drag_active: true,
            review_cursor_millis: 750,
            transport_position_millis: 750,
            ..AppState::default()
        };

        apply_transport_snapshot(
            &mut state,
            Snapshot {
                generation: 0,
                acknowledged_token: 0,
                position_millis: 125,
                playing: true,
                ready: true,
            },
        );

        assert_eq!(state.review_cursor_millis, 750);
        assert_eq!(state.transport_position_millis, 750);
        assert!(state.transport_playing);
    }

    fn persisted_comment_drag_state() -> (AppState, String, String) {
        let track_id = String::from("comment-drag-track");
        let note_id = String::from("move-me");
        let waveform = WaveformData {
            sample_rate: 48_000,
            channels: 1,
            duration_millis: 2_000,
            render_frames: 96_000,
            integrated_lufs: Some(-7.0),
            loudness_profile: Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.1, 0.8, 0.2, 0.4],
                    4,
                    1,
                ),
            ),
        };
        let mut state = AppState {
            busy: false,
            waveform: Some(waveform),
            waveform_track_id: Some(track_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id.clone(),
            title: String::from("Comment drag track"),
            original_name: String::from("comment-drag-track.wav"),
            path: PathBuf::from("/external/comment-drag-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: vec![
                Note {
                    id: note_id.clone(),
                    time_millis: 500,
                    body: String::from("move this"),
                    done: false,
                },
                Note {
                    id: String::from("anchor"),
                    time_millis: 1_000,
                    body: String::from("keep this"),
                    done: false,
                },
            ],
        });
        (state, track_id, note_id)
    }

    #[test]
    fn persisted_comment_drag_moves_and_saves_the_selected_note() {
        let (mut state, _track_id, note_id) = persisted_comment_drag_state();
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::CommentDragStarted {
                ratio: 0.25,
                note_index: Some(0),
            },
            &mut context,
        );
        assert_eq!(state.selected_note_id.as_deref(), Some(note_id.as_str()));
        assert!(state.draft_note.is_none());
        assert!(state.persisted_note_drag.is_some());

        update(
            &mut state,
            Message::CommentDragMoved { ratio: 0.75 },
            &mut context,
        );
        assert_eq!(state.review_cursor_millis, 1_500);
        assert_eq!(
            selected_track(&state)
                .and_then(|track| track.notes.iter().find(|note| note.id == note_id))
                .map(|note| note.time_millis),
            Some(1_500)
        );

        update(
            &mut state,
            Message::CommentDragEnded { ratio: 0.75 },
            &mut context,
        );
        assert!(state.persisted_note_drag.is_none());
        assert!(state.save_in_flight);
        let note_ids = selected_track(&state).map(|track| {
            track
                .notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>()
        });
        assert_eq!(note_ids, Some(vec!["anchor", "move-me"]));
        assert_eq!(state.status, "Comment moved to 00:01 and saved locally.");
    }

    #[test]
    fn persisted_comment_drag_cancellation_rolls_back_without_saving() {
        let (mut state, track_id, note_id) = persisted_comment_drag_state();
        state.draft_note = Some(NoteDraft {
            note_id: Some(note_id.clone()),
            time_millis: 500,
            body: String::from("edit this"),
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::CommentDragStarted {
                ratio: 0.25,
                note_index: Some(0),
            },
            &mut context,
        );
        update(
            &mut state,
            Message::CommentDragMoved { ratio: 0.75 },
            &mut context,
        );
        assert_eq!(state.review_cursor_millis, 1_500);
        assert_eq!(
            state
                .library
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .and_then(|track| track.notes.iter().find(|note| note.id == note_id))
                .map(|note| note.time_millis),
            Some(1_500)
        );
        assert_eq!(
            state.draft_note.as_ref().map(|draft| draft.time_millis),
            Some(1_500)
        );

        update(&mut state, Message::CommentDragCancelled, &mut context);
        assert_eq!(
            selected_track(&state)
                .and_then(|track| track.notes.iter().find(|note| note.id == note_id))
                .map(|note| note.time_millis),
            Some(500)
        );
        assert_eq!(state.review_cursor_millis, 500);
        assert_eq!(
            state.draft_note.as_ref().map(|draft| draft.time_millis),
            Some(500)
        );
        assert!(state.persisted_note_drag.is_none());
        assert!(!state.save_in_flight);

        update(
            &mut state,
            Message::CommentDragMoved { ratio: 0.25 },
            &mut context,
        );
        update(
            &mut state,
            Message::CommentDragEnded { ratio: 0.25 },
            &mut context,
        );
        assert_eq!(
            selected_track(&state)
                .and_then(|track| track.notes.iter().find(|note| note.id == note_id))
                .map(|note| note.time_millis),
            Some(500)
        );
        assert!(state.persisted_note_drag.is_none());
        assert!(!state.save_in_flight);
    }

    #[test]
    fn modified_space_is_unhandled() {
        let state = AppState::default();

        for press in [
            ui::KeyPress::with_command(ui::KeyCode::Space),
            ui::KeyPress::with_control(ui::KeyCode::Space),
            ui::KeyPress::with_shift(ui::KeyCode::Space),
            ui::KeyPress::with_alt(ui::KeyCode::Space),
        ] {
            assert_eq!(
                playback_shortcut(&state, press),
                ui::ShortcutResolution::unhandled()
            );
        }
    }

    #[test]
    fn modified_escape_is_unhandled() {
        let state = AppState::default();

        for press in [
            ui::KeyPress::with_command(ui::KeyCode::Escape),
            ui::KeyPress::with_control(ui::KeyCode::Escape),
            ui::KeyPress::with_shift(ui::KeyCode::Escape),
            ui::KeyPress::with_alt(ui::KeyCode::Escape),
        ] {
            assert_eq!(
                playback_shortcut(&state, press),
                ui::ShortcutResolution::unhandled()
            );
        }
    }

    #[test]
    fn draft_note_space_is_unhandled() {
        let state = AppState {
            draft_note: Some(NoteDraft {
                note_id: None,
                time_millis: 0,
                body: String::new(),
            }),
            ..AppState::default()
        };

        assert_eq!(
            playback_shortcut(&state, ui::KeyPress::new(ui::KeyCode::Space)),
            ui::ShortcutResolution::unhandled()
        );
        assert_eq!(
            playback_shortcut(&state, ui::KeyPress::new(ui::KeyCode::N)),
            ui::ShortcutResolution::unhandled()
        );
        assert_eq!(
            playback_shortcut(&state, ui::KeyPress::new(ui::KeyCode::Escape)),
            ui::ShortcutResolution::action(Message::StopPlayback)
        );
    }

    #[test]
    fn note_editor_submits_on_enter() {
        #[derive(Clone)]
        struct NoteEditorState {
            draft: NoteDraft,
            submitted: bool,
        }

        let bridge = DeclarativeOwnedRuntimeBridge::new(
            NoteEditorState {
                draft: NoteDraft {
                    note_id: None,
                    time_millis: 1_000,
                    body: String::from("Submit me"),
                },
                submitted: false,
            },
            |state| {
                ui::scene(note_editor(&state.draft))
                    .into_view()
                    .into_surface()
            },
            |state, message| {
                if message == Message::SaveDraftNote {
                    state.submitted = true;
                }
            },
        );
        let mut runtime = SurfaceRuntime::new(bridge, Vector2::new(640.0, 180.0));
        let focused = runtime
            .traverse_focus(FocusTraversal::Forward)
            .expect("the comment input should participate in keyboard focus");

        assert_eq!(
            runtime.dispatch_event(Event::key_press(ui::WidgetKey::Enter)),
            Some(focused)
        );
        assert!(runtime.bridge().state().submitted);
    }

    #[test]
    fn native_launch_starts_maximized() {
        let options = native_launch_options();
        assert!(options.window.behavior.maximized);
        assert!(options.window.behavior.integrated_titlebar);
        assert_eq!(
            options
                .window
                .behavior
                .integrated_titlebar_drag_region_height,
            Some(42.0)
        );
    }

    #[test]
    fn audition_volume_changes_output_gain_without_changing_raw_lufs() {
        let mut state = AppState {
            busy: false,
            waveform: Some(WaveformData {
                sample_rate: 48_000,
                channels: 2,
                duration_millis: 1_000,
                render_frames: 48_000,
                integrated_lufs: Some(-7.0),
                loudness_profile: std::sync::Arc::from([]),
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.1, 0.8, 0.2, 0.4],
                        4,
                        2,
                    ),
                ),
            }),
            ..AppState::default()
        };
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::AuditionVolumeChanged(0.25),
            &mut context,
        );

        assert_eq!(state.audition_volume, 0.25);
        assert_eq!(
            state
                .waveform
                .as_ref()
                .and_then(|waveform| waveform.integrated_lufs),
            Some(-7.0)
        );
    }

    #[test]
    fn lufs_meter_follows_playback_position_without_volume_influence() {
        let track_id = String::from("meter-track");
        let mut state = AppState {
            waveform: Some(WaveformData {
                sample_rate: 100,
                channels: 2,
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
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.1, 0.8, 0.2, 0.4],
                        4,
                        2,
                    ),
                ),
            }),
            waveform_track_id: Some(track_id.clone()),
            transport_playing: true,
            transport_position_millis: 400,
            audition_volume: 0.1,
            ..AppState::default()
        };

        assert_eq!(current_lufs_meter_value(&state, &track_id), Some(-4.0));

        state.transport_position_millis = 600;
        assert_eq!(current_lufs_meter_value(&state, &track_id), Some(-8.0));

        state.audition_volume = 0.9;
        assert_eq!(current_lufs_meter_value(&state, &track_id), Some(-8.0));

        state.transport_position_millis = 800;
        state.transport_playing = false;
        assert_eq!(current_lufs_meter_value(&state, &track_id), Some(-12.0));
    }

    #[test]
    fn reference_lufs_meter_follows_its_own_playback_position() {
        let track_id = String::from("reference-meter-track");
        let mut state = AppState {
            waveform: Some(WaveformData {
                sample_rate: 100,
                channels: 1,
                duration_millis: 800,
                render_frames: 80,
                integrated_lufs: Some(-8.0),
                loudness_profile: std::sync::Arc::from([]),
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.1, 0.8, 0.2, 0.4],
                        4,
                        1,
                    ),
                ),
            }),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(WaveformData {
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
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.1, 0.8, 0.2, 0.4],
                        4,
                        1,
                    ),
                ),
            }),
            reference_waveform_track_id: Some(track_id.clone()),
            transport_position_millis: 800,
            reference_transport_position_millis: 400,
            ..AppState::default()
        };

        assert_eq!(
            current_reference_lufs_meter_value(&state, &track_id),
            Some(-4.0)
        );

        state.reference_transport_position_millis = 600;
        assert_eq!(
            current_reference_lufs_meter_value(&state, &track_id),
            Some(-8.0)
        );
        assert_eq!(current_lufs_meter_value(&state, &track_id), Some(-8.0));
    }

    #[test]
    fn reference_match_uses_raw_lufs_without_changing_primary_volume() {
        let track_id = String::from("match-track");
        let primary = WaveformData {
            sample_rate: 48_000,
            channels: 2,
            duration_millis: 1_000,
            render_frames: 48_000,
            integrated_lufs: Some(-8.0),
            loudness_profile: std::sync::Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.1, 0.8, 0.2, 0.4],
                    4,
                    2,
                ),
            ),
        };
        let mut reference = primary.clone();
        reference.integrated_lufs = Some(-14.0);
        let mut state = AppState {
            busy: false,
            waveform: Some(primary),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(reference),
            reference_waveform_track_id: Some(track_id.clone()),
            audition_volume: 0.5,
            audition_source: AuditionSource::Reference,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Match track"),
            original_name: String::from("match-track.wav"),
            path: PathBuf::from("/external/match-track.wav"),
            reference_path: Some(PathBuf::from("/external/reference.wav")),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        let mut context = ui::UiUpdateContext::default();

        assert_eq!(current_loudness_match_gain_db(&state), Some(6.0));
        update(&mut state, Message::ToggleReferenceMatch, &mut context);

        assert!(state.reference_match_enabled);
        assert_eq!(state.audition_volume, 0.5);
        assert_eq!(
            state
                .waveform
                .as_ref()
                .and_then(|waveform| waveform.integrated_lufs),
            Some(-8.0)
        );
        assert!((reference_output_gain(&state) - 0.9976).abs() < 0.002);
    }

    #[test]
    fn audition_source_toggle_switches_the_audible_synchronized_track() {
        let track_id = String::from("audition-track");
        let waveform = WaveformData {
            sample_rate: 48_000,
            channels: 2,
            duration_millis: 1_000,
            render_frames: 48_000,
            integrated_lufs: Some(-8.0),
            loudness_profile: std::sync::Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.1, 0.8, 0.2, 0.4],
                    4,
                    2,
                ),
            ),
        };
        let mut state = AppState {
            busy: false,
            waveform: Some(waveform.clone()),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(waveform),
            reference_waveform_track_id: Some(track_id.clone()),
            audition_volume: 0.5,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Audition track"),
            original_name: String::from("audition.wav"),
            path: PathBuf::from("/external/audition.wav"),
            reference_path: Some(PathBuf::from("/external/reference.wav")),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        let mut context = ui::UiUpdateContext::default();

        assert_eq!(main_output_gain(&state), 0.5);
        assert_eq!(reference_output_gain(&state), 0.0);
        update(
            &mut state,
            Message::SelectAuditionSource(AuditionSource::Reference),
            &mut context,
        );
        assert_eq!(state.audition_source, AuditionSource::Reference);
        assert_eq!(main_output_gain(&state), 0.0);
        assert_eq!(reference_output_gain(&state), 0.5);
        update(
            &mut state,
            Message::SelectAuditionSource(AuditionSource::Main),
            &mut context,
        );
        assert_eq!(state.audition_source, AuditionSource::Main);

        update(
            &mut state,
            Message::SelectAuditionSource(AuditionSource::Reference),
            &mut context,
        );
        assert_eq!(state.audition_source, AuditionSource::Reference);
        update(
            &mut state,
            Message::SelectAuditionSource(AuditionSource::Reference),
            &mut context,
        );
        assert_eq!(state.audition_source, AuditionSource::Reference);
    }

    #[test]
    fn main_waveform_click_switches_audition_to_the_imported_track() {
        let mut state = shared_reference_playback_state();
        state.audition_source = AuditionSource::Reference;
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformClicked {
                ratio: 0.25,
                lower: false,
            },
            &mut context,
        );

        assert_eq!(state.audition_source, AuditionSource::Main);
        assert!(main_output_gain(&state) > 0.0);
        assert_eq!(reference_output_gain(&state), 0.0);
    }

    #[test]
    fn stopped_main_upper_click_seeks_and_resumes_playback() {
        let mut state = main_only_loop_state();
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformClicked {
                ratio: 0.25,
                lower: false,
            },
            &mut context,
        );

        assert_eq!(state.transport_position_millis, 500);
        assert_eq!(state.review_cursor_millis, 500);
        assert!(state.transport_polling);
        assert!(state.transport_waiting_token.is_some());
        assert_eq!(state.status, "Playing from 00:00.");
    }

    #[test]
    fn playing_main_upper_click_keeps_paired_transport_in_sync() {
        let mut state = shared_reference_playback_state();
        state.loop_selections = LoopSelections {
            main: Some(LoopSelection {
                start_ratio: 0.1,
                end_ratio: 0.2,
            }),
            reference: Some(LoopSelection {
                start_ratio: 0.7,
                end_ratio: 0.8,
            }),
        };
        state.transport_playing = true;
        state.reference_transport_playing = true;
        state.reference_transport_position_millis = 1_000;
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformClicked {
                ratio: 0.25,
                lower: false,
            },
            &mut context,
        );

        assert!(state.loop_selections.main.is_none());
        assert!(state.loop_selections.reference.is_some());
        assert_eq!(state.transport_position_millis, 500);
        assert_eq!(state.review_cursor_millis, 500);
        assert_eq!(state.reference_transport_position_millis, 1_000);
        assert!(state.transport_polling);
        assert!(state.reference_transport_polling);
        assert!(state.transport_waiting_token.is_some());
        assert!(state.reference_transport_waiting_token.is_some());
    }

    #[test]
    fn reference_only_loop_selection_seeks_only_the_reference_source() {
        let track_id = String::from("reference-click-track");
        let main_waveform = WaveformData {
            sample_rate: 48_000,
            channels: 2,
            duration_millis: 2_000,
            render_frames: 96_000,
            integrated_lufs: Some(-8.0),
            loudness_profile: std::sync::Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.1, 0.8, 0.2, 0.4],
                    4,
                    2,
                ),
            ),
        };
        let reference_waveform = WaveformData {
            duration_millis: 4_000,
            render_frames: 192_000,
            ..main_waveform.clone()
        };
        let mut state = AppState {
            busy: false,
            waveform: Some(main_waveform),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(reference_waveform),
            reference_waveform_track_id: Some(track_id.clone()),
            reference_transport: Some(transport::AudioTransport::spawn()),
            reference_transport_loaded: true,
            loop_selections: LoopSelections {
                main: Some(LoopSelection {
                    start_ratio: 0.7,
                    end_ratio: 0.8,
                }),
                reference: Some(LoopSelection {
                    start_ratio: 0.1,
                    end_ratio: 0.2,
                }),
            },
            review_cursor_millis: 600,
            transport_position_millis: 600,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Reference click track"),
            original_name: String::from("reference-click.wav"),
            path: PathBuf::from("/external/reference-click.wav"),
            reference_path: Some(PathBuf::from("/external/reference.wav")),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::ReferenceWaveformClicked { ratio: 0.25 },
            &mut context,
        );

        assert!(state.loop_selections.main.is_some());
        assert!(state.loop_selections.reference.is_none());
        assert_eq!(state.review_cursor_millis, 600);
        assert_eq!(state.transport_position_millis, 600);
        assert_eq!(state.reference_transport_position_millis, 1_000);
        assert!(!state.transport_polling);
        assert!(state.transport_waiting_token.is_none());
        assert!(state.reference_transport_polling);
        assert!(state.reference_transport_waiting_token.is_some());
        assert!(state.reference_only_playback);
        assert_eq!(state.audition_source, AuditionSource::Reference);
        assert_eq!(main_output_gain(&state), 0.0);
        assert_eq!(
            waveform::ratio_for_millis(state.review_cursor_millis, 2_000),
            Some(0.3)
        );
        assert_eq!(
            waveform::ratio_for_millis(state.reference_transport_position_millis, 4_000),
            Some(0.25)
        );

        state.reference_transport_playing = true;
        state.reference_transport_polling = false;
        state.reference_transport_waiting_token = None;

        update(
            &mut state,
            Message::ReferenceLoopDragEnded {
                start_ratio: 0.25,
                end_ratio: 0.75,
            },
            &mut context,
        );

        assert!(state.reference_only_playback);
        assert!(!state.transport_polling);
        assert!(state.transport_waiting_token.is_none());
        assert!(state.reference_transport_polling);
        assert!(state.reference_transport_waiting_token.is_some());
        assert_eq!(state.review_cursor_millis, 600);
        assert_eq!(state.transport_position_millis, 600);
        assert_eq!(state.reference_transport_position_millis, 1_000);

        update(&mut state, Message::Frame, &mut context);
        assert_eq!(state.review_cursor_millis, 600);
        assert_eq!(state.transport_position_millis, 600);
    }

    #[test]
    fn reference_waveform_click_keeps_active_main_playback_independent() {
        let track_id = String::from("active-reference-click-track");
        let main_waveform = WaveformData {
            sample_rate: 48_000,
            channels: 2,
            duration_millis: 2_000,
            render_frames: 96_000,
            integrated_lufs: Some(-8.0),
            loudness_profile: std::sync::Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.1, 0.8, 0.2, 0.4],
                    4,
                    2,
                ),
            ),
        };
        let reference_waveform = WaveformData {
            duration_millis: 4_000,
            render_frames: 192_000,
            ..main_waveform.clone()
        };
        let main_position_millis = 600;
        let mut state = AppState {
            busy: false,
            waveform: Some(main_waveform),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(reference_waveform),
            reference_waveform_track_id: Some(track_id.clone()),
            reference_transport: Some(transport::AudioTransport::spawn()),
            reference_transport_loaded: true,
            reference_transport_playing: true,
            review_cursor_millis: main_position_millis,
            transport_position_millis: main_position_millis,
            transport_playing: true,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Active reference click track"),
            original_name: String::from("active-reference-click.wav"),
            path: PathBuf::from("/external/active-reference-click.wav"),
            reference_path: Some(PathBuf::from("/external/reference.wav")),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::ReferenceWaveformClicked { ratio: 0.25 },
            &mut context,
        );

        assert!(state.reference_only_playback);
        assert!(state.transport_playing);
        assert!(!state.transport_polling);
        assert!(state.transport_waiting_token.is_none());
        assert_eq!(state.transport_position_millis, main_position_millis);
        assert_eq!(state.review_cursor_millis, main_position_millis);
        assert!(state.reference_transport_polling);
        assert!(state.reference_transport_waiting_token.is_some());
        let generation = state.transport_generation;
        apply_transport_snapshot(
            &mut state,
            Snapshot {
                generation,
                acknowledged_token: 0,
                position_millis: main_position_millis + 8,
                playing: true,
                ready: true,
            },
        );
        assert_eq!(state.transport_position_millis, main_position_millis + 8);
        assert_eq!(state.review_cursor_millis, main_position_millis + 8);
    }

    #[test]
    fn main_waveform_playhead_resume_starts_reference_at_its_stored_position() {
        let track_id = String::from("main-seek-track");
        let main_waveform = WaveformData {
            sample_rate: 48_000,
            channels: 2,
            duration_millis: 2_000,
            render_frames: 96_000,
            integrated_lufs: Some(-8.0),
            loudness_profile: std::sync::Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.1, 0.8, 0.2, 0.4],
                    4,
                    2,
                ),
            ),
        };
        let reference_waveform = WaveformData {
            duration_millis: 4_000,
            render_frames: 192_000,
            ..main_waveform.clone()
        };
        let mut state = AppState {
            busy: false,
            waveform: Some(main_waveform),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(reference_waveform),
            reference_waveform_track_id: Some(track_id.clone()),
            reference_transport: Some(transport::AudioTransport::spawn()),
            reference_transport_loaded: true,
            reference_transport_position_millis: 1_500,
            ..AppState::default()
        };
        state.loop_selections = LoopSelections {
            main: Some(LoopSelection {
                start_ratio: 0.1,
                end_ratio: 0.2,
            }),
            reference: Some(LoopSelection {
                start_ratio: 0.7,
                end_ratio: 0.8,
            }),
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Main seek track"),
            original_name: String::from("main-seek.wav"),
            path: PathBuf::from("/external/main-seek.wav"),
            reference_path: Some(PathBuf::from("/external/reference.wav")),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformPlayheadDragStarted { ratio: 0.25 },
            &mut context,
        );
        update(
            &mut state,
            Message::WaveformPlayheadDragEnded { ratio: 0.25 },
            &mut context,
        );

        assert!(state.loop_selections.main.is_none());
        assert!(state.loop_selections.reference.is_some());
        assert!(state.transport_polling);
        assert!(state.transport_waiting_token.is_some());
        assert!(state.reference_transport_polling);
        assert!(state.reference_transport_waiting_token.is_some());
        let main_ratio = waveform::ratio_for_millis(state.review_cursor_millis, 2_000)
            .expect("the main waveform duration is nonzero");
        let reference_ratio =
            waveform::ratio_for_millis(state.reference_transport_position_millis, 4_000)
                .expect("the reference waveform duration is nonzero");
        assert_eq!(main_ratio, 0.25);
        assert_eq!(reference_ratio, 0.375);
    }

    fn shared_reference_playback_state() -> AppState {
        let track_id = String::from("paired-admission-track");
        let main_waveform = WaveformData {
            sample_rate: 48_000,
            channels: 2,
            duration_millis: 2_000,
            render_frames: 96_000,
            integrated_lufs: Some(-8.0),
            loudness_profile: std::sync::Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.1, 0.8, 0.2, 0.4],
                    4,
                    2,
                ),
            ),
        };
        let reference_waveform = WaveformData {
            duration_millis: 4_000,
            render_frames: 192_000,
            ..main_waveform.clone()
        };
        let mut state = AppState {
            busy: false,
            waveform: Some(main_waveform),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(reference_waveform),
            reference_waveform_track_id: Some(track_id.clone()),
            reference_transport: Some(transport::AudioTransport::spawn()),
            reference_transport_loaded: true,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Paired admission track"),
            original_name: String::from("paired-admission.wav"),
            path: PathBuf::from("/external/paired-admission.wav"),
            reference_path: Some(PathBuf::from("/external/paired-reference.wav")),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        state
    }

    fn reference_comment_drag_state() -> (AppState, String) {
        let mut state = shared_reference_playback_state();
        let note_id = String::from("reference-move-me");
        let reference_path = state.library.tracks[0]
            .reference_path
            .clone()
            .expect("the paired state should have a reference path");
        state.library.reference_tracks.push(ReferenceTrack {
            path: reference_path,
            notes: vec![
                Note {
                    id: note_id.clone(),
                    time_millis: 500,
                    body: String::from("move this reference comment"),
                    done: false,
                },
                Note {
                    id: String::from("reference-anchor"),
                    time_millis: 1_000,
                    body: String::from("keep this reference comment"),
                    done: false,
                },
            ],
        });
        (state, note_id)
    }

    #[test]
    fn reference_playhead_drag_tracks_reference_position_and_preserves_main_transport() {
        let mut state = shared_reference_playback_state();
        state.loop_selections = LoopSelections {
            main: Some(LoopSelection {
                start_ratio: 0.1,
                end_ratio: 0.2,
            }),
            reference: Some(LoopSelection {
                start_ratio: 0.7,
                end_ratio: 0.8,
            }),
        };
        state.review_cursor_millis = 600;
        state.transport_position_millis = 600;
        state.reference_transport_position_millis = 400;
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::ReferencePlayheadDragStarted { ratio: 0.25 },
            &mut context,
        );
        assert!(state.loop_selections.main.is_some());
        assert!(state.loop_selections.reference.is_none());
        assert!(state.reference_playhead_drag_active);
        assert_eq!(state.reference_transport_position_millis, 1_000);
        assert_eq!(state.review_cursor_millis, 600);
        assert_eq!(state.transport_position_millis, 600);
        assert_eq!(state.audition_source, AuditionSource::Reference);

        update(
            &mut state,
            Message::ReferencePlayheadDragMoved { ratio: 0.75 },
            &mut context,
        );
        assert_eq!(state.reference_transport_position_millis, 3_000);
        assert_eq!(state.review_cursor_millis, 600);
        assert_eq!(state.transport_position_millis, 600);

        update(
            &mut state,
            Message::ReferencePlayheadDragEnded { ratio: 0.5 },
            &mut context,
        );
        assert!(!state.reference_playhead_drag_active);
        assert_eq!(state.reference_transport_position_millis, 2_000);
        assert_eq!(state.review_cursor_millis, 600);
        assert_eq!(state.transport_position_millis, 600);
        assert!(state.reference_transport_polling);
        assert!(state.reference_transport_waiting_token.is_some());
        assert!(state.reference_only_playback);
    }

    #[test]
    fn reference_draft_comment_drag_updates_reference_timestamp_without_main_cursor() {
        let mut state = shared_reference_playback_state();
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::ReferenceCommentClicked { ratio: 0.25 },
            &mut context,
        );
        assert_eq!(
            state
                .reference_draft_note
                .as_ref()
                .map(|draft| draft.time_millis),
            Some(1_000)
        );

        update(
            &mut state,
            Message::ReferenceCommentDragStarted {
                ratio: 0.25,
                note_index: None,
            },
            &mut context,
        );
        update(
            &mut state,
            Message::ReferenceCommentDragMoved { ratio: 0.75 },
            &mut context,
        );
        assert_eq!(
            state
                .reference_draft_note
                .as_ref()
                .map(|draft| draft.time_millis),
            Some(3_000)
        );
        assert_eq!(state.reference_transport_position_millis, 3_000);
        assert_eq!(state.review_cursor_millis, 0);
        assert_eq!(state.transport_position_millis, 0);

        update(
            &mut state,
            Message::ReferenceCommentDragEnded { ratio: 0.5 },
            &mut context,
        );
        assert_eq!(
            state
                .reference_draft_note
                .as_ref()
                .map(|draft| draft.time_millis),
            Some(2_000)
        );
        assert_eq!(state.reference_transport_position_millis, 2_000);
    }

    #[test]
    fn reference_persisted_comment_drag_moves_and_saves_reference_note() {
        let (mut state, note_id) = reference_comment_drag_state();
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::ReferenceCommentDragStarted {
                ratio: 0.125,
                note_index: Some(0),
            },
            &mut context,
        );
        assert_eq!(
            state.selected_reference_note_id.as_deref(),
            Some(note_id.as_str())
        );
        assert!(state.reference_persisted_note_drag.is_some());
        assert_eq!(state.reference_transport_position_millis, 500);

        update(
            &mut state,
            Message::ReferenceCommentDragMoved { ratio: 0.75 },
            &mut context,
        );
        assert_eq!(
            selected_reference_notes(&state)
                .iter()
                .find(|note| note.id == note_id)
                .map(|note| note.time_millis),
            Some(3_000)
        );
        assert_eq!(state.reference_transport_position_millis, 3_000);

        update(
            &mut state,
            Message::ReferenceCommentDragEnded { ratio: 0.75 },
            &mut context,
        );
        assert!(state.reference_persisted_note_drag.is_none());
        assert!(state.save_in_flight);
        assert_eq!(
            state.status,
            "Reference comment moved to 00:03 and saved locally."
        );
        assert_eq!(state.review_cursor_millis, 0);
        assert_eq!(state.transport_position_millis, 0);
    }

    #[test]
    fn reference_persisted_comment_drag_cancellation_rolls_back_without_saving() {
        let (mut state, note_id) = reference_comment_drag_state();
        state.reference_draft_note = Some(NoteDraft {
            note_id: Some(note_id.clone()),
            time_millis: 500,
            body: String::from("edit this reference comment"),
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::ReferenceCommentDragStarted {
                ratio: 0.125,
                note_index: Some(0),
            },
            &mut context,
        );
        update(
            &mut state,
            Message::ReferenceCommentDragMoved { ratio: 0.75 },
            &mut context,
        );
        assert_eq!(
            selected_reference_notes(&state)
                .iter()
                .find(|note| note.id == note_id)
                .map(|note| note.time_millis),
            Some(3_000)
        );

        update(
            &mut state,
            Message::ReferenceCommentDragCancelled,
            &mut context,
        );
        assert_eq!(
            selected_reference_notes(&state)
                .iter()
                .find(|note| note.id == note_id)
                .map(|note| note.time_millis),
            Some(500)
        );
        assert_eq!(state.reference_transport_position_millis, 500);
        assert_eq!(
            state
                .reference_draft_note
                .as_ref()
                .map(|draft| draft.time_millis),
            Some(500)
        );
        assert!(state.reference_persisted_note_drag.is_none());
        assert!(!state.save_in_flight);
    }

    fn main_only_loop_state() -> AppState {
        let mut state = shared_reference_playback_state();
        state.reference_waveform = None;
        state.reference_waveform_track_id = None;
        state.reference_transport = None;
        state.reference_transport_loaded = false;
        state.library.tracks[0].reference_path = None;
        state
    }

    #[test]
    fn shared_play_rejects_before_main_when_reference_queue_is_full() {
        let mut state = shared_reference_playback_state();
        let initial_position = state.transport_position_millis;
        state
            .reference_transport
            .as_ref()
            .expect("the paired state should have a reference transport")
            .force_command_queue_full_for_test();
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::TogglePlayback, &mut context);

        assert_eq!(state.status, transport::CONTROLS_BUSY_ERROR);
        assert!(!state.transport_polling);
        assert!(!state.transport_playing);
        assert_eq!(state.transport_position_millis, initial_position);
    }

    #[test]
    fn shared_pause_rejects_before_main_when_reference_queue_is_full() {
        let mut state = shared_reference_playback_state();
        state.transport_playing = true;
        state.reference_transport_playing = true;
        state
            .reference_transport
            .as_ref()
            .expect("the paired state should have a reference transport")
            .force_command_queue_full_for_test();
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::TogglePlayback, &mut context);

        assert_eq!(state.status, transport::CONTROLS_BUSY_ERROR);
        assert!(!state.transport_polling);
        assert!(state.transport_playing);
        assert!(state.reference_transport_playing);
    }

    #[test]
    fn stop_playback_pauses_both_active_transports() {
        let mut state = shared_reference_playback_state();
        state.transport_playing = true;
        state.reference_transport_playing = true;
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::StopPlayback, &mut context);

        assert_eq!(state.status, "Stopping playback…");
        assert!(state.transport_playing);
        assert!(state.transport_polling);
        assert!(state.transport_waiting_token.is_some());
        assert!(state.reference_transport_playing);
        assert!(state.reference_transport_polling);
        assert!(state.reference_transport_waiting_token.is_some());
        assert!(!state.reference_only_playback);
    }

    #[test]
    fn stop_playback_pauses_reference_only_playback() {
        let mut state = shared_reference_playback_state();
        state.reference_only_playback = true;
        state.reference_transport_playing = true;
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::StopPlayback, &mut context);

        assert_eq!(state.status, "Stopping playback…");
        assert!(!state.transport_polling);
        assert!(state.transport_waiting_token.is_none());
        assert!(state.reference_transport_playing);
        assert!(state.reference_transport_polling);
        assert!(state.reference_transport_waiting_token.is_some());
        assert!(!state.reference_only_playback);
    }

    #[test]
    fn stop_playback_rejects_before_main_when_reference_queue_is_full() {
        let mut state = shared_reference_playback_state();
        state.transport_playing = true;
        state.reference_transport_playing = true;
        state
            .reference_transport
            .as_ref()
            .expect("the paired state should have a reference transport")
            .force_command_queue_full_for_test();
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::StopPlayback, &mut context);

        assert_eq!(state.status, transport::CONTROLS_BUSY_ERROR);
        assert!(!state.transport_polling);
        assert!(state.transport_waiting_token.is_none());
        assert!(state.transport_playing);
        assert!(state.reference_transport_playing);
    }

    #[test]
    fn toggle_playback_resumes_unloaded_reference_at_stored_normalized_position() {
        let track_id = String::from("resume-reference-track");
        let main_waveform = WaveformData {
            sample_rate: 48_000,
            channels: 2,
            duration_millis: 2_000,
            render_frames: 96_000,
            integrated_lufs: Some(-8.0),
            loudness_profile: std::sync::Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.1, 0.8, 0.2, 0.4],
                    4,
                    2,
                ),
            ),
        };
        let reference_waveform = WaveformData {
            duration_millis: 4_000,
            render_frames: 192_000,
            ..main_waveform.clone()
        };
        let mut state = AppState {
            busy: false,
            waveform: Some(main_waveform),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(reference_waveform),
            reference_waveform_track_id: Some(track_id.clone()),
            reference_transport: Some(transport::AudioTransport::spawn()),
            reference_transport_loaded: false,
            transport_playing: true,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Resume reference track"),
            original_name: String::from("resume-reference.wav"),
            path: PathBuf::from("/external/resume-reference.wav"),
            reference_path: Some(PathBuf::from("/external/reference.wav")),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });

        let main_position_millis = 500;
        state.transport_position_millis = main_position_millis;
        state.review_cursor_millis = main_position_millis;
        state.reference_transport_position_millis = 1_000;
        assert_eq!(state.reference_transport_position_millis, 1_000);
        assert!(!state.reference_transport_loaded);
        assert!(state.loop_selections == LoopSelections::default());

        let mut context = ui::UiUpdateContext::default();
        update(&mut state, Message::TogglePlayback, &mut context);
        assert!(state.transport_polling);

        // Acknowledge the pause without waiting for the background transport.
        state.transport_polling = false;
        state.transport_waiting_token = None;
        state.transport_playing = false;
        update(&mut state, Message::TogglePlayback, &mut context);

        assert_eq!(state.reference_transport_position_millis, 1_000);
        assert_eq!(
            waveform::ratio_for_millis(state.transport_position_millis, 2_000),
            waveform::ratio_for_millis(state.reference_transport_position_millis, 4_000)
        );
    }

    #[test]
    fn reference_loop_selection_stays_on_the_reference_timeline() {
        let track_id = String::from("loop-track");
        let primary = WaveformData {
            sample_rate: 48_000,
            channels: 2,
            duration_millis: 2_000,
            render_frames: 96_000,
            integrated_lufs: Some(-8.0),
            loudness_profile: std::sync::Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.1, 0.8, 0.2, 0.4],
                    4,
                    2,
                ),
            ),
        };
        let reference = WaveformData {
            duration_millis: 4_000,
            render_frames: 192_000,
            ..primary.clone()
        };
        let mut state = AppState {
            busy: false,
            waveform: Some(primary),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(reference),
            reference_waveform_track_id: Some(track_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Loop track"),
            original_name: String::from("loop.wav"),
            path: PathBuf::from("/external/loop.wav"),
            reference_path: Some(PathBuf::from("/external/reference.wav")),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        state.transport_position_millis = 500;
        state.review_cursor_millis = 500;
        state.reference_transport_position_millis = 1_400;
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::ReferenceLoopDragEnded {
                start_ratio: 0.25,
                end_ratio: 0.75,
            },
            &mut context,
        );

        assert_eq!(
            state.loop_selections.reference,
            Some(LoopSelection {
                start_ratio: 0.25,
                end_ratio: 0.75,
            })
        );
        assert!(state.loop_selections.main.is_none());
        assert_eq!(state.transport_position_millis, 500);
        assert_eq!(state.review_cursor_millis, 500);
        assert_eq!(state.reference_transport_position_millis, 1_000);
        assert_eq!(
            loop_bounds(&state),
            Some(LoopBounds {
                start_millis: 1_000,
                end_millis: 3_000,
            })
        );

        update(
            &mut state,
            Message::ReferenceLoopDragEnded {
                start_ratio: 0.25,
                end_ratio: 0.25,
            },
            &mut context,
        );
        assert!(state.loop_selections.reference.is_none());
    }

    #[test]
    fn main_loop_selection_supports_main_only_tracks_and_normalizes_direction() {
        let mut state = main_only_loop_state();
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformLoopDragStarted { ratio: 0.8 },
            &mut context,
        );
        update(
            &mut state,
            Message::WaveformLoopDragEnded {
                start_ratio: 0.8,
                end_ratio: 0.2,
            },
            &mut context,
        );

        assert_eq!(state.audition_source, AuditionSource::Main);
        assert_eq!(
            state.loop_selections.main,
            Some(LoopSelection {
                start_ratio: 0.2,
                end_ratio: 0.8,
            })
        );
        assert_eq!(
            loop_bounds(&state),
            Some(LoopBounds {
                start_millis: 400,
                end_millis: 1_600,
            })
        );
        assert_eq!(state.transport_position_millis, 400);
        assert_eq!(state.review_cursor_millis, 400);
    }

    #[test]
    fn main_loop_selection_stays_on_the_main_timeline() {
        let mut state = shared_reference_playback_state();
        state.reference_transport_position_millis = 2_800;
        state.loop_selections.reference = Some(LoopSelection {
            start_ratio: 0.6,
            end_ratio: 0.9,
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformLoopDragEnded {
                start_ratio: 0.75,
                end_ratio: 0.25,
            },
            &mut context,
        );

        assert_eq!(
            state.loop_selections.main,
            Some(LoopSelection {
                start_ratio: 0.25,
                end_ratio: 0.75,
            })
        );
        assert_eq!(
            loop_bounds(&state),
            Some(LoopBounds {
                start_millis: 500,
                end_millis: 1_500,
            })
        );
        assert_eq!(state.transport_position_millis, 500);
        assert_eq!(state.review_cursor_millis, 500);
        assert_eq!(state.reference_transport_position_millis, 2_800);
        assert!(state.loop_selections.reference.is_some());
    }

    #[test]
    fn main_loop_selection_enforces_120_millisecond_minimum_on_main_duration() {
        let mut state = shared_reference_playback_state();
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformLoopDragEnded {
                start_ratio: 0.25,
                end_ratio: 0.30,
            },
            &mut context,
        );

        assert!(state.loop_selections.main.is_none());
        assert!(state.status.contains("120 ms"));

        update(
            &mut state,
            Message::WaveformLoopDragEnded {
                start_ratio: 0.25,
                end_ratio: 0.31,
            },
            &mut context,
        );

        assert!(state.loop_selections.main.is_some());
        assert_eq!(
            loop_bounds(&state),
            Some(LoopBounds {
                start_millis: 500,
                end_millis: 620,
            })
        );
    }

    #[test]
    fn reference_loop_minimum_is_validated_without_projecting_to_main_duration() {
        let mut state = shared_reference_playback_state();
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::ReferenceLoopDragEnded {
                start_ratio: 0.25,
                end_ratio: 0.28,
            },
            &mut context,
        );

        assert!(state.loop_selections.main.is_none());
        assert!(state.loop_selections.reference.is_some());
        assert_eq!(
            loop_bounds(&state),
            Some(LoopBounds {
                start_millis: 1_000,
                end_millis: 1_120,
            })
        );
    }

    #[test]
    fn simultaneous_distinct_loops_wrap_independently_on_different_durations() {
        let mut state = shared_reference_playback_state();
        state.loop_selections = LoopSelections {
            main: Some(LoopSelection {
                start_ratio: 0.25,
                end_ratio: 0.5,
            }),
            reference: Some(LoopSelection {
                start_ratio: 0.5,
                end_ratio: 0.75,
            }),
        };
        state.transport_playing = true;
        state.reference_transport_playing = true;
        state.transport_position_millis = 1_000;
        state.review_cursor_millis = 1_000;
        state.reference_transport_position_millis = 3_000;

        enforce_loop(&mut state, true, true);

        assert_eq!(state.transport_position_millis, 500);
        assert_eq!(state.review_cursor_millis, 500);
        assert_eq!(state.reference_transport_position_millis, 2_000);
        assert!(state.transport_polling);
        assert!(state.reference_transport_polling);
    }

    #[test]
    fn reference_only_playback_enforces_reference_loop_without_touching_main() {
        let mut state = shared_reference_playback_state();
        state.loop_selections = LoopSelections {
            main: Some(LoopSelection {
                start_ratio: 0.1,
                end_ratio: 0.2,
            }),
            reference: Some(LoopSelection {
                start_ratio: 0.5,
                end_ratio: 0.75,
            }),
        };
        state.reference_only_playback = true;
        state.transport_playing = true;
        state.reference_transport_playing = true;
        state.transport_position_millis = 250;
        state.review_cursor_millis = 250;
        state.reference_transport_position_millis = 3_000;

        enforce_loop(&mut state, false, true);

        assert_eq!(state.transport_position_millis, 250);
        assert_eq!(state.review_cursor_millis, 250);
        assert!(!state.transport_polling);
        assert_eq!(state.reference_transport_position_millis, 2_000);
        assert!(state.reference_transport_polling);
    }

    #[test]
    fn global_play_resumes_each_source_from_its_own_loop_or_stored_position() {
        let mut state = shared_reference_playback_state();
        state.loop_selections = LoopSelections {
            main: Some(LoopSelection {
                start_ratio: 0.25,
                end_ratio: 0.5,
            }),
            reference: Some(LoopSelection {
                start_ratio: 0.5,
                end_ratio: 0.75,
            }),
        };
        state.transport_position_millis = 700;
        state.review_cursor_millis = 700;
        state.reference_transport_position_millis = 3_500;
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::TogglePlayback, &mut context);

        assert_eq!(state.transport_position_millis, 700);
        assert_eq!(state.review_cursor_millis, 700);
        assert_eq!(state.reference_transport_position_millis, 2_000);
        assert!(state.transport_polling);
        assert!(state.reference_transport_polling);
        assert!(state.transport_waiting_token.is_some());
        assert!(state.reference_transport_waiting_token.is_some());
    }

    #[test]
    fn each_loop_controls_only_its_own_completion_when_durations_differ() {
        let mut main_driver_state = shared_reference_playback_state();
        main_driver_state.loop_selections.main = Some(LoopSelection {
            start_ratio: 0.25,
            end_ratio: 0.75,
        });
        main_driver_state.transport_playing = true;
        main_driver_state.reference_transport_playing = true;
        main_driver_state.transport_position_millis = 1_500;
        main_driver_state.reference_transport_position_millis = 2_000;
        enforce_loop(&mut main_driver_state, true, true);

        assert_eq!(main_driver_state.transport_position_millis, 500);
        assert_eq!(main_driver_state.reference_transport_position_millis, 2_000);
        assert!(main_driver_state.transport_polling);
        assert!(!main_driver_state.reference_transport_polling);

        let mut reference_driver_state = shared_reference_playback_state();
        reference_driver_state.loop_selections.reference = Some(LoopSelection {
            start_ratio: 0.25,
            end_ratio: 0.75,
        });
        reference_driver_state.audition_source = AuditionSource::Reference;
        reference_driver_state.transport_playing = true;
        reference_driver_state.reference_transport_playing = true;
        reference_driver_state.transport_position_millis = 1_000;
        reference_driver_state.reference_transport_position_millis = 3_000;
        enforce_loop(&mut reference_driver_state, true, true);

        assert_eq!(reference_driver_state.transport_position_millis, 1_000);
        assert_eq!(
            reference_driver_state.reference_transport_position_millis,
            1_000
        );
        assert!(!reference_driver_state.transport_polling);
        assert!(reference_driver_state.reference_transport_polling);
    }

    #[test]
    fn loop_source_switching_preserves_ranges_and_active_source_controls_transport() {
        let mut state = shared_reference_playback_state();
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformLoopDragEnded {
                start_ratio: 0.2,
                end_ratio: 0.4,
            },
            &mut context,
        );
        assert!(state.loop_selections.main.is_some());

        update(
            &mut state,
            Message::ReferenceLoopDragEnded {
                start_ratio: 0.6,
                end_ratio: 0.9,
            },
            &mut context,
        );
        assert_eq!(state.audition_source, AuditionSource::Reference);
        assert!(state.loop_selections.main.is_some());
        assert!(state.loop_selections.reference.is_some());
        assert_eq!(
            loop_bounds(&state),
            Some(LoopBounds {
                start_millis: 2_400,
                end_millis: 3_600,
            })
        );

        update(
            &mut state,
            Message::SelectAuditionSource(AuditionSource::Main),
            &mut context,
        );
        assert_eq!(state.audition_source, AuditionSource::Main);
        assert_eq!(
            loop_bounds(&state),
            Some(LoopBounds {
                start_millis: 400,
                end_millis: 800,
            })
        );

        update(
            &mut state,
            Message::SelectAuditionSource(AuditionSource::Reference),
            &mut context,
        );
        assert!(state.loop_selections.main.is_some());
        assert!(state.loop_selections.reference.is_some());

        state.audition_source = AuditionSource::Main;
        state.loop_selections.clear(AuditionSource::Main);
        assert!(loop_bounds(&state).is_none());
        assert!(state.loop_selections.reference.is_some());
    }

    #[test]
    fn loop_drag_cancellation_and_undersized_ranges_clear_only_the_initiating_source() {
        let mut state = shared_reference_playback_state();
        state.loop_selections = LoopSelections {
            main: Some(LoopSelection {
                start_ratio: 0.2,
                end_ratio: 0.4,
            }),
            reference: Some(LoopSelection {
                start_ratio: 0.6,
                end_ratio: 0.9,
            }),
        };
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformLoopDragStarted { ratio: 0.4 },
            &mut context,
        );
        assert!(state.loop_selections.main.is_none());
        assert!(state.loop_selections.reference.is_some());
        update(&mut state, Message::WaveformLoopDragCancelled, &mut context);
        assert!(state.loop_selections.main.is_none());
        assert!(state.loop_selections.reference.is_some());

        update(
            &mut state,
            Message::ReferenceLoopDragStarted { ratio: 0.6 },
            &mut context,
        );
        assert!(state.loop_selections.main.is_none());
        assert!(state.loop_selections.reference.is_none());
        update(
            &mut state,
            Message::ReferenceLoopDragEnded {
                start_ratio: 0.2,
                end_ratio: 0.22,
            },
            &mut context,
        );
        assert!(state.loop_selections.main.is_none());
        assert!(state.loop_selections.reference.is_none());
    }

    #[test]
    fn active_main_loop_creation_ignores_reference_queue_failure() {
        let mut state = shared_reference_playback_state();
        state.transport_playing = true;
        state.reference_transport_playing = true;
        state.transport_position_millis = 700;
        state.reference_transport_position_millis = 1_400;
        state
            .reference_transport
            .as_ref()
            .expect("the paired state should have a reference transport")
            .force_command_queue_full_for_test();
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformLoopDragEnded {
                start_ratio: 0.25,
                end_ratio: 0.75,
            },
            &mut context,
        );

        assert!(state.loop_selections.main.is_some());
        assert_ne!(state.status, transport::CONTROLS_BUSY_ERROR);
        assert_eq!(state.transport_position_millis, 500);
        assert_eq!(state.review_cursor_millis, 500);
        assert_eq!(state.reference_transport_position_millis, 1_400);
        assert!(state.transport_polling);
        assert!(!state.reference_transport_polling);
    }

    #[test]
    fn lower_waveform_click_projects_draft_editor_beneath_waveform() {
        let track_id = String::from("review-track");
        let mut state = AppState {
            busy: false,
            waveform: Some(WaveformData {
                sample_rate: 48_000,
                channels: 1,
                duration_millis: 2_000,
                render_frames: 96_000,
                integrated_lufs: Some(-7.0),
                loudness_profile: std::sync::Arc::from([]),
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.1, 0.8, 0.2, 0.4],
                        4,
                        1,
                    ),
                ),
            }),
            waveform_track_id: Some(track_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Review track"),
            original_name: String::from("review-track.wav"),
            path: PathBuf::from("/external/review-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::WaveformClicked {
                ratio: 0.5,
                lower: true,
            },
            &mut context,
        );

        let focus_command = context.into_command();
        assert!(matches!(
            focus_command,
            radiant::runtime::Command::Batch(commands)
                if commands.iter().any(|command| matches!(
                    command,
                    radiant::runtime::Command::Focus(id)
                        if *id == super::MAIN_COMMENT_EDITOR_ID
                ))
        ));
        let mut context = ui::UiUpdateContext::default();

        let draft = state
            .draft_note
            .as_ref()
            .expect("a lower waveform click should create a draft");
        assert_eq!(draft.time_millis, 1_000);

        let frame = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 1000.0));
        assert!(
            frame.paint_plan.primitives.iter().any(|primitive| {
                matches!(
                    primitive,
                    PaintPrimitive::StrokePolygon(stroke)
                        if stroke.color == ThemeTokens::default().text_primary
                            && (stroke.width - 3.0).abs() < f32::EPSILON
                )
            }),
            "the draft comment node should be visible on the waveform rail"
        );
        let editor_rect = frame
            .paint_plan
            .first_text_rect("COMMENT AT 00:01")
            .expect("the draft editor should be visible after a lower click");
        let comments_rect = frame
            .paint_plan
            .first_text_rect("COMMENTS")
            .expect("the lower comments panel should remain visible");
        let labels = frame.paint_plan.text_label_strings();

        assert!(
            !labels
                .iter()
                .any(|label| label == "COMMENTS / CLICK TO PIN"),
            "the waveform helper label should stay hidden"
        );

        assert_eq!(
            labels
                .iter()
                .filter(|label| label.as_str() == "COMMENT AT 00:01")
                .count(),
            1,
            "the draft editor should have one visible timestamp header"
        );
        assert!(
            comments_rect.min.y < editor_rect.min.y,
            "the draft editor should be rendered inside the unified comments surface"
        );
        assert!(labels.iter().any(|label| label == "Save comment"));
        assert!(labels.iter().any(|label| label == "Cancel"));
        assert!(
            frame.paint_plan.primitives.iter().any(|primitive| {
                matches!(
                    primitive,
                    PaintPrimitive::TextInput(input)
                        if input.placeholder.as_ref().is_some_and(|placeholder| {
                            placeholder.as_str() == "Write a comment…"
                        })
                )
            }),
            "the rendered draft editor should expose its input field"
        );

        update(
            &mut state,
            Message::CommentDragMoved { ratio: 0.75 },
            &mut context,
        );
        assert_eq!(
            state
                .draft_note
                .as_ref()
                .expect("the draft should remain open while dragging")
                .time_millis,
            1_500
        );
        assert_eq!(state.review_cursor_millis, 1_500);

        state
            .draft_note
            .as_mut()
            .expect("the draft should still be open")
            .note_id = Some(String::from("existing-note"));
        update(
            &mut state,
            Message::CommentDragMoved { ratio: 0.25 },
            &mut context,
        );
        assert_eq!(
            state
                .draft_note
                .as_ref()
                .expect("the edit draft should remain open")
                .time_millis,
            1_500,
            "editing a saved comment must keep its fixed timestamp"
        );
    }

    #[test]
    fn selected_reference_track_projects_a_matching_waveform_below_the_main_review() {
        let track_id = String::from("reference-track");
        let waveform = WaveformData {
            sample_rate: 48_000,
            channels: 2,
            duration_millis: 2_000,
            render_frames: 96_000,
            integrated_lufs: Some(-8.0),
            loudness_profile: std::sync::Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.1, 0.8, 0.2, 0.4],
                    4,
                    2,
                ),
            ),
        };
        let mut state = AppState {
            busy: false,
            waveform: Some(waveform.clone()),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(waveform),
            reference_waveform_track_id: Some(track_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Review track"),
            original_name: String::from("review-track.wav"),
            path: PathBuf::from("/external/review-track.wav"),
            reference_path: Some(PathBuf::from("/external/reference.wav")),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });

        let frame = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 1_000.0));
        let reference_rect = frame
            .paint_plan
            .first_text_rect("reference.wav")
            .expect("the global reference selector should be visible");
        let title_rect = frame
            .paint_plan
            .first_text_rect("Review track")
            .expect("the primary track title should be visible");
        let metadata_rect = frame
            .paint_plan
            .text_runs()
            .find(|run| run.text.as_str().contains("48000 Hz"))
            .map(|run| run.rect)
            .expect("the transport metadata should be visible");
        let main_waveform_rect = frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if fill.color == ThemeTokens::default().bg_secondary
                        && fill.rect.width() > 300.0
                        && fill.rect.height() > 20.0
                        && fill.rect.height() <= WAVEFORM_HEIGHT =>
                {
                    Some(fill.rect)
                }
                _ => None,
            })
            .expect("the main waveform should paint its lower rail");
        let svg_count = frame
            .paint_plan
            .primitives
            .iter()
            .filter(|primitive| matches!(primitive, PaintPrimitive::Svg(_)))
            .count();
        let labels = frame.paint_plan.text_label_strings();

        assert!(labels.iter().any(|label| label == "Replace reference"));
        assert!(labels.iter().any(|label| label == "●"));
        assert!(labels.iter().any(|label| label == "○"));
        assert!(labels.iter().any(|label| label == "MATCH REF"));
        assert!(
            svg_count >= 3,
            "status, play, and volume icons should paint"
        );
        assert!(!labels.iter().any(|label| label == "Play"));
        assert!(!labels.iter().any(|label| label == "VOL 80"));
        assert!(
            !labels.iter().any(|label| label == "LOCAL TRACK"),
            "the redundant review-card section label should stay removed"
        );
        assert!(
            !labels
                .iter()
                .any(|label| label == "01  WAVEFORM / TOP TO PLAY"),
            "the redundant waveform heading should stay removed"
        );
        assert!(
            labels
                .iter()
                .any(|label| label.contains("48000 Hz") && label.contains("00:00 / 00:02")),
            "metadata and transport time should share the compact waveform status line"
        );
        assert!(reference_rect.min.y < main_waveform_rect.min.y);
        assert!(
            main_waveform_rect.min.y < metadata_rect.min.y,
            "the transport metadata should sit below the compact waveform pair"
        );
        assert!(reference_rect.min.y < title_rect.min.y);
    }

    #[test]
    fn reference_header_uses_the_reference_loop_when_main_loop_is_active() {
        let track_id = String::from("reference-loop-header-track");
        let main_waveform = audition_waveform();
        let reference_waveform = WaveformData {
            duration_millis: 4_000,
            render_frames: 192_000,
            ..main_waveform.clone()
        };
        let mut state = AppState {
            busy: false,
            waveform: Some(main_waveform),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(reference_waveform),
            reference_waveform_track_id: Some(track_id.clone()),
            loop_selections: LoopSelections {
                main: Some(LoopSelection {
                    start_ratio: 0.2,
                    end_ratio: 0.4,
                }),
                reference: Some(LoopSelection {
                    start_ratio: 0.7,
                    end_ratio: 0.9,
                }),
            },
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Reference loop header track"),
            original_name: String::from("reference-loop-header.wav"),
            path: PathBuf::from("/external/reference-loop-header.wav"),
            reference_path: Some(PathBuf::from("/external/reference-header.wav")),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });

        let frame = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 1_000.0));
        let labels = frame.paint_plan.text_label_strings();

        assert!(labels.iter().any(|label| {
            label == "REFERENCE · reference-header.wav · -7.0 LUFS · LOOP 00:02–00:03"
        }));
        assert!(!labels.iter().any(|label| {
            label == "REFERENCE · reference-header.wav · -7.0 LUFS · LOOP 00:00–00:01"
        }));
    }

    #[test]
    fn reference_comment_draft_saves_to_its_catalog_entry_and_switching_keeps_comments_separate() {
        let track_id = String::from("reference-comment-track");
        let first_path = PathBuf::from("/external/first-reference.wav");
        let second_path = PathBuf::from("/external/second-reference.wav");
        let waveform = audition_waveform();
        let mut state = AppState {
            busy: false,
            waveform: Some(waveform.clone()),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(waveform),
            reference_waveform_track_id: Some(track_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id.clone(),
            title: String::from("Reference comment track"),
            original_name: String::from("reference-comment.wav"),
            path: PathBuf::from("/external/reference-comment.wav"),
            reference_path: Some(first_path.clone()),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: vec![Note {
                id: String::from("main-note"),
                time_millis: 100,
                body: String::from("Main track note"),
                done: false,
            }],
        });
        state.library.reference_tracks = vec![
            ReferenceTrack {
                path: first_path.clone(),
                notes: Vec::new(),
            },
            ReferenceTrack {
                path: second_path.clone(),
                notes: vec![Note {
                    id: String::from("second-reference-note"),
                    time_millis: 700,
                    body: String::from("Only on the second reference."),
                    done: false,
                }],
            },
        ];
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::ReferenceCommentClicked { ratio: 0.25 },
            &mut context,
        );
        let focus_command = context.into_command();
        assert!(matches!(
            focus_command,
            radiant::runtime::Command::Batch(commands)
                if commands.iter().any(|command| matches!(
                    command,
                    radiant::runtime::Command::Focus(id)
                        if *id == super::REFERENCE_COMMENT_EDITOR_ID
                ))
        ));
        let mut context = ui::UiUpdateContext::default();
        assert_eq!(
            state
                .reference_draft_note
                .as_ref()
                .map(|draft| draft.time_millis),
            Some(250)
        );
        update(
            &mut state,
            Message::ReferenceDraftNoteChanged(String::from("Check the reference kick.")),
            &mut context,
        );
        update(&mut state, Message::SaveReferenceDraftNote, &mut context);

        assert!(state.reference_draft_note.is_none());
        assert_eq!(state.library.tracks[0].notes[0].body, "Main track note");
        assert_eq!(state.library.reference_tracks[0].notes.len(), 1);
        assert_eq!(
            state.library.reference_tracks[0].notes[0].body,
            "Check the reference kick."
        );

        update(
            &mut state,
            Message::SetReferenceTrack {
                track_id: track_id.clone(),
                path: second_path.clone(),
            },
            &mut context,
        );
        assert_eq!(state.library.tracks[0].reference_path, Some(second_path));
        assert_eq!(state.library.reference_tracks[0].notes.len(), 1);
        assert_eq!(state.library.reference_tracks[1].notes.len(), 1);
        assert_eq!(
            selected_reference_notes(&state)[0].body,
            "Only on the second reference."
        );
    }

    #[test]
    fn deleting_one_note_retains_other_notes_and_clears_its_active_draft() {
        let track_id = String::from("review-track");
        let mut state = AppState {
            busy: false,
            draft_note: Some(NoteDraft {
                note_id: Some(String::from("target-note")),
                time_millis: 1_000,
                body: String::from("edit this"),
            }),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Review track"),
            original_name: String::from("review-track.wav"),
            path: PathBuf::from("/external/review-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: vec![
                Note {
                    id: String::from("target-note"),
                    time_millis: 1_000,
                    body: String::from("edit this"),
                    done: false,
                },
                Note {
                    id: String::from("keep-note"),
                    time_millis: 2_000,
                    body: String::from("keep this"),
                    done: true,
                },
            ],
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::DeleteNote(String::from("target-note")),
            &mut context,
        );

        let track = state
            .library
            .tracks
            .first()
            .expect("the track should remain");
        assert_eq!(track.notes.len(), 1);
        assert_eq!(track.notes[0].id, "keep-note");
        assert!(state.draft_note.is_none());
        assert!(state.save_in_flight);
        assert_eq!(state.status, "Comment deleted locally.");
    }

    #[test]
    fn main_comment_draft_remains_visible_when_reference_comments_exist() {
        let track_id = String::from("main-draft-with-reference-comments");
        let reference_path = PathBuf::from("/external/reference-with-comments.wav");
        let waveform = audition_waveform();
        let mut state = AppState {
            busy: false,
            waveform: Some(waveform.clone()),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(waveform),
            reference_waveform_track_id: Some(track_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id.clone(),
            title: String::from("Main draft track"),
            original_name: String::from("main-draft.wav"),
            path: PathBuf::from("/external/main-draft.wav"),
            reference_path: Some(reference_path.clone()),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        state.library.reference_tracks.push(ReferenceTrack {
            path: reference_path,
            notes: vec![Note {
                id: String::from("reference-note"),
                time_millis: 250,
                body: String::from("Reference note"),
                done: false,
            }],
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::SelectCommentSource(super::CommentSource::Main),
            &mut context,
        );
        update(
            &mut state,
            Message::WaveformClicked {
                ratio: 0.5,
                lower: true,
            },
            &mut context,
        );

        let labels = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1_180.0, 1_000.0))
            .paint_plan
            .text_label_strings();
        assert_eq!(state.comment_source, super::CommentSource::Main);
        assert!(labels.iter().any(|label| label == "MAIN"));
        assert!(labels.iter().any(|label| label == "COMMENT AT 00:00"));
        assert!(labels.iter().any(|label| label == "Save comment"));
        assert!(
            !labels
                .iter()
                .any(|label| label == "REFERENCE COMMENT AT 00:00")
        );
    }

    #[test]
    fn rendered_comments_panel_shows_all_comments_and_delete_controls() {
        let track_id = String::from("review-track");
        let mut state = AppState {
            busy: false,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Review track"),
            original_name: String::from("review-track.wav"),
            path: PathBuf::from("/external/review-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: vec![
                Note {
                    id: String::from("open-note"),
                    time_millis: 1_000,
                    body: String::from("first comment"),
                    done: false,
                },
                Note {
                    id: String::from("done-note"),
                    time_millis: 2_000,
                    body: String::from("second comment"),
                    done: true,
                },
            ],
        });

        let labels = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 1000.0))
            .paint_plan
            .text_label_strings();

        assert!(labels.iter().any(|label| label == "COMMENTS"));
        assert!(labels.iter().any(|label| label == "first comment"));
        assert!(labels.iter().any(|label| label == "second comment"));
        assert_eq!(
            labels
                .iter()
                .filter(|label| label.as_str() == "Delete")
                .count(),
            2,
            "each rendered comment row should expose a Delete control"
        );
    }

    #[test]
    fn comment_row_hover_tracks_and_clears_the_linked_note() {
        let track_id = String::from("hover-track");
        let mut state = AppState {
            busy: false,
            waveform: Some(WaveformData {
                sample_rate: 48_000,
                channels: 1,
                duration_millis: 2_000,
                render_frames: 96_000,
                integrated_lufs: Some(-7.0),
                loudness_profile: Arc::from([]),
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.1, 0.8, 0.2, 0.4],
                        4,
                        1,
                    ),
                ),
            }),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.waveform_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Hover track"),
            original_name: String::from("hover-track.wav"),
            path: PathBuf::from("/external/hover-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: vec![Note {
                id: String::from("hover-note"),
                time_millis: 1_000,
                body: String::from("hover me"),
                done: false,
            }],
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::CommentHoverStarted(String::from("hover-note")),
            &mut context,
        );
        assert_eq!(state.hovered_note_id.as_deref(), Some("hover-note"));
        assert_eq!(
            note_ratio_for_id(
                &state,
                state
                    .library
                    .tracks
                    .first()
                    .expect("the track should exist"),
                state.hovered_note_id.as_deref(),
            ),
            Some(0.5)
        );

        update(
            &mut state,
            Message::CommentHoverEnded(String::from("hover-note")),
            &mut context,
        );
        assert_eq!(state.hovered_note_id, None);
    }

    #[test]
    fn reference_comment_row_hover_and_click_route_to_the_reference_marker() {
        let track_id = String::from("reference-hover-track");
        let note_id = String::from("reference-hover-note");
        let reference_path = PathBuf::from("/external/reference-hover.wav");
        let mut state = AppState {
            busy: false,
            reference_waveform: Some(audition_waveform()),
            reference_waveform_track_id: Some(track_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id.clone(),
            title: String::from("Reference hover track"),
            original_name: String::from("reference-hover.wav"),
            path: PathBuf::from("/external/reference-hover-main.wav"),
            reference_path: Some(reference_path.clone()),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        state.library.reference_tracks.push(ReferenceTrack {
            path: reference_path,
            notes: vec![Note {
                id: note_id.clone(),
                time_millis: 500,
                body: String::from("reference hover me"),
                done: false,
            }],
        });

        let bridge = DeclarativeOwnedRuntimeBridge::new(
            state,
            |state| project_surface(state).into_surface(),
            |state, message| {
                let mut context = ui::UiUpdateContext::default();
                update(state, message, &mut context);
            },
        );
        let mut runtime = SurfaceRuntime::new(bridge, Vector2::new(1_180.0, 1_000.0));
        let frame = runtime.frame_with_default_theme();
        let comment_rect = frame
            .paint_plan
            .first_text_rect("reference hover me")
            .expect("the reference comment row should paint its body");
        let comment_point = Point::new(
            comment_rect.min.x + comment_rect.width() * 0.5,
            comment_rect.min.y + comment_rect.height() * 0.5,
        );

        assert!(runtime.widget_at(comment_point).is_some());
        runtime.dispatch_event(Event::pointer_move(comment_point));
        assert_eq!(
            runtime
                .bridge()
                .state()
                .hovered_reference_note_id
                .as_deref(),
            Some(note_id.as_str())
        );
        runtime.dispatch_primary_click(comment_point);
        assert_eq!(
            runtime
                .bridge()
                .state()
                .selected_reference_note_id
                .as_deref(),
            Some(note_id.as_str())
        );
        assert!(
            runtime.bridge().state().reference_draft_note.is_none(),
            "a single click should select the reference comment without opening its editor"
        );

        runtime.dispatch_event(Event::primary_double_click(comment_point));
        let draft = runtime
            .bridge()
            .state()
            .reference_draft_note
            .as_ref()
            .expect("a double click on the reference comment body should open its editor");
        assert_eq!(draft.note_id.as_deref(), Some(note_id.as_str()));
        assert_eq!(draft.time_millis, 500);
        assert_eq!(draft.body, "reference hover me");

        runtime.dispatch_event(Event::pointer_move(Point::new(10.0, 10.0)));
        assert_eq!(runtime.bridge().state().hovered_reference_note_id, None);
    }

    #[test]
    fn reference_waveform_comment_rail_is_a_single_interactive_surface() {
        let track_id = String::from("reference-rail-track");
        let reference_path = PathBuf::from("/external/reference-rail.wav");
        let waveform = audition_waveform();
        let mut state = AppState {
            busy: false,
            waveform: Some(waveform.clone()),
            waveform_track_id: Some(track_id.clone()),
            reference_waveform: Some(waveform),
            reference_waveform_track_id: Some(track_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Reference rail track"),
            original_name: String::from("reference-rail-main.wav"),
            path: PathBuf::from("/external/reference-rail-main.wav"),
            reference_path: Some(reference_path.clone()),
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        state.library.reference_tracks.push(ReferenceTrack {
            path: reference_path,
            notes: Vec::new(),
        });

        let bridge = DeclarativeOwnedRuntimeBridge::new(
            state,
            |state| project_surface(state).into_surface(),
            |state, message| {
                let mut context = ui::UiUpdateContext::default();
                update(state, message, &mut context);
            },
        );
        let mut runtime = SurfaceRuntime::new(bridge, Vector2::new(1_180.0, 1_000.0));
        let frame = runtime.frame_with_default_theme();
        let reference_bars = frame
            .paint_plan
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if fill.color == ThemeTokens::default().text_primary =>
                {
                    Some(fill.rect)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            !reference_bars.is_empty(),
            "the reference waveform should paint once"
        );
        let lower_bars = frame
            .paint_plan
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if fill.color == ThemeTokens::default().text_muted.with_alpha(160) =>
                {
                    Some(fill.rect)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!lower_bars.is_empty(), "the lower waveform should paint");
        let reference_lower_background = frame
            .paint_plan
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if fill.color == ThemeTokens::default().bg_secondary
                        && fill.rect.width() > 300.0
                        && fill.rect.height() <= WAVEFORM_HEIGHT =>
                {
                    Some(fill.rect)
                }
                _ => None,
            })
            .filter(|rect| {
                lower_bars
                    .iter()
                    .any(|bar| bar.min.y >= rect.min.y && bar.max.y <= rect.max.y)
            })
            .max_by(|left, right| left.min.y.total_cmp(&right.min.y))
            .expect("the reference waveform should paint its lower background");
        let reference_rail_y = reference_lower_background.min.y - 1.0;
        assert!(
            reference_bars
                .iter()
                .all(|rect| rect.max.y <= reference_rail_y),
            "reference bars should stay in the upper waveform band"
        );
        let reference_lower_bars = lower_bars
            .iter()
            .filter(|rect| rect.min.y >= reference_lower_background.min.y)
            .collect::<Vec<_>>();
        assert!(
            !reference_lower_bars.is_empty(),
            "the reference lower waveform should paint"
        );
        assert!(
            reference_lower_bars
                .iter()
                .all(|rect| rect.min.y > reference_rail_y),
            "reference lower bars should stay below the comment rail"
        );
        let distinct_lower_bar_starts =
            reference_lower_bars
                .iter()
                .fold(Vec::<f32>::new(), |mut starts, rect| {
                    if !starts
                        .iter()
                        .any(|start| (*start - rect.min.x).abs() < f32::EPSILON)
                    {
                        starts.push(rect.min.x);
                    }
                    starts
                });
        assert_eq!(
            distinct_lower_bar_starts.len(),
            reference_lower_bars.len(),
            "the reference lower signal should paint once per bar"
        );
        let reference_center_x = reference_lower_background.min.x
            + waveform::REFERENCE_START_HIT_SLOP
            + (reference_lower_background.width() - waveform::REFERENCE_START_HIT_SLOP) * 0.5;
        let rail_point = Point::new(reference_center_x, reference_rail_y + 8.0);

        assert!(
            runtime.widget_at(rail_point).is_some(),
            "the reference comment rail must remain in the pointer hit-test surface"
        );
        assert!(
            runtime
                .dispatch_pointer_move_with_outcome(rail_point)
                .routed(),
            "hovering the reference comment rail must reach the reference widget"
        );
        runtime.dispatch_primary_click(rail_point);
        let reference_time = runtime
            .bridge()
            .state()
            .reference_draft_note
            .as_ref()
            .map(|draft| draft.time_millis);
        assert!(
            reference_time.is_some_and(|time| time.abs_diff(500) <= 10),
            "clicking the reference rail should create a timestamped reference comment, got {reference_time:?}"
        );
    }

    #[test]
    fn composed_comment_pointer_routing_preserves_waveform_marker_highlights() {
        let track_id = String::from("composed-comment-track");
        let note_id = String::from("composed-comment");
        let waveform = WaveformData {
            sample_rate: 48_000,
            channels: 1,
            duration_millis: 2_000,
            render_frames: 96_000,
            integrated_lufs: Some(-7.0),
            loudness_profile: Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                    &[0.1, 0.8, 0.2, 0.4],
                    4,
                    1,
                ),
            ),
        };
        let mut state = AppState {
            busy: false,
            waveform: Some(waveform),
            waveform_track_id: Some(track_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Composed comment track"),
            original_name: String::from("composed-comment-track.wav"),
            path: PathBuf::from("/external/composed-comment-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: vec![Note {
                id: note_id.clone(),
                time_millis: 1_000,
                body: String::from("hover and select me"),
                done: false,
            }],
        });

        let bridge = DeclarativeOwnedRuntimeBridge::new(
            state,
            |state| project_surface(state).into_surface(),
            |state, message| {
                let mut context = ui::UiUpdateContext::default();
                update(state, message, &mut context);
            },
        );
        let mut runtime = SurfaceRuntime::new(bridge, Vector2::new(1_180.0, 1_000.0));
        let initial_frame = runtime.frame_with_default_theme();
        let comment_rect = initial_frame
            .paint_plan
            .first_text_rect("hover and select me")
            .expect("the composed comment row should paint its body");
        let comment_point = Point::new(
            comment_rect.min.x + comment_rect.width() * 0.5,
            comment_rect.min.y + comment_rect.height() * 0.5,
        );
        let lower_waveform_rect = initial_frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if fill.color == ThemeTokens::default().bg_secondary
                        && fill.rect.width() > 300.0
                        && fill.rect.height() > 20.0
                        && fill.rect.height() <= WAVEFORM_HEIGHT =>
                {
                    Some(fill.rect)
                }
                _ => None,
            })
            .expect("the decoded main waveform should paint its lower rail");
        let marker_center = Point::new(
            lower_waveform_rect.min.x
                + waveform::REFERENCE_START_HIT_SLOP
                + (lower_waveform_rect.width() - waveform::REFERENCE_START_HIT_SLOP) * 0.5,
            lower_waveform_rect.min.y - 1.0,
        );
        let highlighted_marker_count = |primitives: &[PaintPrimitive], center: Point| {
            primitives
                .iter()
                .filter(|primitive| {
                    let PaintPrimitive::StrokePolygon(stroke) = primitive else {
                        return false;
                    };
                    if stroke.color != ThemeTokens::default().text_primary
                        || (stroke.width - 3.0).abs() >= f32::EPSILON
                    {
                        return false;
                    }
                    let min_x = stroke
                        .points
                        .iter()
                        .map(|point| point.x)
                        .fold(f32::INFINITY, f32::min);
                    let max_x = stroke
                        .points
                        .iter()
                        .map(|point| point.x)
                        .fold(f32::NEG_INFINITY, f32::max);
                    let min_y = stroke
                        .points
                        .iter()
                        .map(|point| point.y)
                        .fold(f32::INFINITY, f32::min);
                    let max_y = stroke
                        .points
                        .iter()
                        .map(|point| point.y)
                        .fold(f32::NEG_INFINITY, f32::max);
                    ((min_x + max_x) * 0.5 - center.x).abs() < 0.5
                        && ((min_y + max_y) * 0.5 - center.y).abs() < 0.5
                        && (max_x - min_x - 9.0).abs() < 0.5
                        && (max_y - min_y - 9.0).abs() < 0.5
                })
                .count()
        };

        assert!(runtime.widget_at(comment_point).is_some());
        runtime.dispatch_event(Event::pointer_move(comment_point));
        assert_eq!(
            runtime.bridge().state().hovered_note_id.as_deref(),
            Some(note_id.as_str()),
            "a live comment-row pointer move should reach CommentHoverStarted"
        );
        let hovered_frame = runtime.frame_with_default_theme();
        assert_eq!(
            highlighted_marker_count(&hovered_frame.paint_plan.primitives, marker_center),
            1,
            "hovering the composed comment row should highlight its linked waveform marker"
        );

        runtime.dispatch_primary_click(comment_point);
        assert_eq!(
            runtime.bridge().state().selected_note_id.as_deref(),
            Some(note_id.as_str())
        );
        assert!(
            runtime.bridge().state().draft_note.is_none(),
            "a single click should select the comment without opening its editor"
        );

        runtime.dispatch_event(Event::primary_double_click(comment_point));
        let draft = runtime
            .bridge()
            .state()
            .draft_note
            .as_ref()
            .expect("a double click on the comment body should open its editor");
        assert_eq!(draft.note_id.as_deref(), Some(note_id.as_str()));
        assert_eq!(draft.time_millis, 1_000);
        assert_eq!(draft.body, "hover and select me");
        let editing_frame = runtime.frame_with_default_theme();
        assert!(editing_frame.paint_plan.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                PaintPrimitive::TextInput(input)
                    if input.widget_id == super::main_inline_comment_editor_id(&note_id)
                        && input.state.value.as_str() == "hover and select me"
            )
        }));
        assert!(
            !editing_frame
                .paint_plan
                .text_label_strings()
                .iter()
                .any(|label| label.starts_with("COMMENT AT "))
        );

        let waveform_away_point = Point::new(
            lower_waveform_rect.min.x + lower_waveform_rect.width() * 0.1,
            lower_waveform_rect.min.y - 20.0,
        );
        assert!(runtime.widget_at(waveform_away_point).is_some());
        runtime.dispatch_event(Event::pointer_move(waveform_away_point));
        assert_eq!(runtime.bridge().state().hovered_note_id, None);
        assert_eq!(
            runtime.bridge().state().selected_note_id.as_deref(),
            Some(note_id.as_str())
        );
        assert_eq!(
            highlighted_marker_count(
                &runtime.frame_with_default_theme().paint_plan.primitives,
                marker_center,
            ),
            1,
            "moving over the waveform away from the node should preserve the selected marker highlight"
        );
    }

    #[test]
    fn comment_row_selection_tracks_the_linked_note_for_waveform_highlight() {
        let track_id = String::from("selected-track");
        let mut state = AppState {
            busy: false,
            waveform: Some(WaveformData {
                sample_rate: 48_000,
                channels: 1,
                duration_millis: 2_000,
                render_frames: 96_000,
                integrated_lufs: Some(-7.0),
                loudness_profile: Arc::from([]),
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.1, 0.8, 0.2, 0.4],
                        4,
                        1,
                    ),
                ),
            }),
            waveform_track_id: Some(track_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Selected track"),
            original_name: String::from("selected-track.wav"),
            path: PathBuf::from("/external/selected-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Inbox,
            notes: vec![Note {
                id: String::from("selected-note"),
                time_millis: 1_000,
                body: String::from("select me"),
                done: false,
            }],
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::SelectNote(String::from("selected-note")),
            &mut context,
        );

        assert_eq!(state.selected_note_id.as_deref(), Some("selected-note"));
        assert_eq!(
            note_ratio_for_id(
                &state,
                state
                    .library
                    .tracks
                    .first()
                    .expect("the track should exist"),
                state.selected_note_id.as_deref(),
            ),
            Some(0.5)
        );
    }

    #[test]
    fn waveform_completion_requires_the_current_generation_and_selection() {
        let mut state = AppState::default();
        state.library.selected_track_id = Some(String::from("track-a"));
        state.waveform_generation = 7;
        assert!(decode_result_is_current(&state, "track-a", 7));
        assert!(!decode_result_is_current(&state, "track-a", 6));
        assert!(!decode_result_is_current(&state, "track-b", 7));
    }

    #[test]
    fn reference_waveform_completion_requires_the_current_generation_and_selection() {
        let mut state = AppState::default();
        state.library.selected_track_id = Some(String::from("track-a"));
        state.reference_waveform_generation = 4;
        assert!(reference_decode_result_is_current(&state, "track-a", 4));
        assert!(!reference_decode_result_is_current(&state, "track-a", 3));
        assert!(!reference_decode_result_is_current(&state, "track-b", 4));
    }

    #[test]
    fn waveform_progress_publishes_only_for_the_current_selection() {
        let waveform = WaveformData {
            sample_rate: 48_000,
            channels: 1,
            duration_millis: 2_000,
            render_frames: 2,
            integrated_lufs: None,
            loudness_profile: Arc::from([]),
            summary: Arc::new(
                radiant::runtime::GpuSignalSummary::from_interleaved_samples(&[0.2, 0.4], 2, 1),
            ),
        };
        let mut state = AppState {
            busy: false,
            waveform_busy: true,
            waveform_generation: 7,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(String::from("track-a"));
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::DecodeProgress {
                track_id: String::from("track-a"),
                generation: 7,
                progress: crate::audio::WaveformProgress {
                    waveform: waveform.clone(),
                    progress: Some(0.4),
                },
            },
            &mut context,
        );
        assert_eq!(state.waveform_track_id.as_deref(), Some("track-a"));
        assert_eq!(state.waveform_progress, Some(0.4));
        assert_eq!(state.waveform.as_ref(), Some(&waveform));

        update(
            &mut state,
            Message::DecodeProgress {
                track_id: String::from("track-b"),
                generation: 7,
                progress: crate::audio::WaveformProgress {
                    waveform: WaveformData {
                        duration_millis: 4_000,
                        ..waveform.clone()
                    },
                    progress: Some(0.8),
                },
            },
            &mut context,
        );
        assert_eq!(state.waveform_track_id.as_deref(), Some("track-a"));
        assert_eq!(state.waveform_progress, Some(0.4));
    }

    #[test]
    fn transport_confirmation_accepts_host_ack_before_ui_observes_it() {
        let snapshot = Snapshot {
            generation: 3,
            acknowledged_token: 9,
            position_millis: 1_250,
            playing: false,
            ready: true,
        };
        assert!(transport_command_is_confirmed(snapshot, 9));
        assert!(transport_command_is_confirmed(snapshot, 8));
        assert!(!transport_command_is_confirmed(snapshot, 10));
    }

    #[test]
    fn planner_groups_tracks_by_their_persisted_stage() {
        let track = |id: &str, stage: TrackStage| Track {
            id: String::from(id),
            title: String::from(id),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            reference_path: None,
            size: 0,
            favorite: false,
            stage,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };
        let tracks = vec![
            track("sound", TrackStage::SoundDesign),
            track("mix", TrackStage::Mixdown),
            track("production", TrackStage::Production),
        ];

        let production = tracks_in_stage(&tracks, TrackStage::Production);
        let mastering = tracks_in_stage(&tracks, TrackStage::Mastering);

        assert_eq!(production.len(), 1);
        assert_eq!(production[0].id, "production");
        assert!(mastering.is_empty());
    }

    #[test]
    fn review_and_planner_status_filters_only_project_matching_tracks() {
        let track = |id: &str, status: TrackStatus, stage: TrackStage| Track {
            id: String::from(id),
            title: format!("{id} track"),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            reference_path: None,
            size: 0,
            favorite: false,
            stage,
            status,
            notes: Vec::new(),
        };
        let tracks = vec![
            track("inbox", TrackStatus::Inbox, TrackStage::SoundDesign),
            track("refine", TrackStatus::Refine, TrackStage::Production),
            track("release", TrackStatus::Release, TrackStage::Mixdown),
        ];

        let refined = tracks_with_status(&tracks, Some(TrackStatus::Refine));
        assert_eq!(
            refined
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["refine"]
        );
        assert_eq!(
            tracks_in_stage(&refined, TrackStage::Production)
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["refine"]
        );

        let mut review = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Review,
            review_status_filter: Some(TrackStatus::Refine),
            ..AppState::default()
        };
        review.library.tracks = tracks.clone();
        let review_labels = project_surface(&review)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0))
            .paint_plan
            .text_label_strings();
        assert!(review_labels.iter().any(|label| label == "refine track"));
        assert!(!review_labels.iter().any(|label| label == "inbox track"));
        assert!(!review_labels.iter().any(|label| label == "release track"));

        let mut planner = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Planner,
            planner_status_filter: Some(TrackStatus::Refine),
            ..AppState::default()
        };
        planner.library.tracks = tracks;
        let planner_labels = project_surface(&planner)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0))
            .paint_plan
            .text_label_strings();
        assert!(planner_labels.iter().any(|label| label == "refine track"));
        assert!(!planner_labels.iter().any(|label| label == "inbox track"));
        assert!(!planner_labels.iter().any(|label| label == "release track"));

        planner.planner_status_filter = Some(TrackStatus::Archive);
        let empty_planner_labels = project_surface(&planner)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0))
            .paint_plan
            .text_label_strings();
        assert!(
            empty_planner_labels
                .iter()
                .any(|label| label == "No tracks in the Archive status.")
        );
        assert!(
            !empty_planner_labels
                .iter()
                .any(|label| label == "No tracks here yet.")
        );
    }

    #[test]
    fn changing_status_filters_closes_hidden_card_controls() {
        let mut state = AppState {
            busy: false,
            stage_menu_track_id: Some(String::from("hidden-track")),
            stage_menu_anchor: Some(Point::new(40.0, 80.0)),
            status_menu_track_id: Some(String::from("hidden-track")),
            status_menu_host: Some(StatusMenuHost::Library),
            ..AppState::default()
        };
        state.library.tracks.push(Track {
            id: String::from("hidden-track"),
            title: String::from("Hidden track"),
            original_name: String::from("hidden-track.wav"),
            path: PathBuf::from("/external/hidden-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::SetReviewStatusFilter(Some(TrackStatus::Refine)),
            &mut context,
        );
        assert!(state.stage_menu_track_id.is_none());
        assert!(state.stage_menu_anchor.is_none());
        assert!(state.status_menu_track_id.is_none());
        assert!(state.status_menu_host.is_none());

        state.stage_menu_track_id = Some(String::from("hidden-track"));
        state.stage_menu_anchor = Some(Point::new(40.0, 80.0));
        state.status_menu_track_id = Some(String::from("hidden-track"));
        state.status_menu_host = Some(StatusMenuHost::Planner);
        state.planner_drag_source_track_id = Some(String::from("hidden-track"));
        state.planner_drag_target_stage = Some(TrackStage::Mixdown);
        state.planner_drag_pointer = Some(Point::new(100.0, 120.0));

        update(
            &mut state,
            Message::SetPlannerStatusFilter(Some(TrackStatus::Archive)),
            &mut context,
        );
        assert!(state.stage_menu_track_id.is_none());
        assert!(state.stage_menu_anchor.is_none());
        assert!(state.status_menu_track_id.is_none());
        assert!(state.status_menu_host.is_none());
        assert!(state.planner_drag_source_track_id.is_none());
        assert!(state.planner_drag_target_stage.is_none());
        assert!(state.planner_drag_pointer.is_none());
    }

    #[test]
    fn planner_drop_requires_a_different_known_stage() {
        assert!(planner_drop_is_valid(
            Some(TrackStage::SoundDesign),
            TrackStage::Production
        ));
        assert!(!planner_drop_is_valid(
            Some(TrackStage::Mixdown),
            TrackStage::Mixdown
        ));
        assert!(!planner_drop_is_valid(None, TrackStage::Mastering));
    }

    #[test]
    fn active_planner_drag_projects_a_visible_preview() {
        let state = planner_drag_state(Point::new(120.0, 90.0));

        let labels = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0))
            .paint_plan
            .text_label_strings();

        assert!(
            labels.iter().any(|label| label == "↕ Preview me"),
            "active planner drags must add their preview to the paint plan"
        );
    }

    #[test]
    fn planner_drag_frame_syncs_preview_to_native_pointer() {
        let previous_pointer = Point::new(120.0, 90.0);
        let current_pointer = Point::new(460.0, 310.0);
        let mut state = planner_drag_state(previous_pointer);
        let previous_preview = planner_drag_preview_rect(&state)
            .expect("an active planner drag should paint its preview before frame sync");
        let mut context = ui::UiUpdateContext::from_runtime_snapshot(
            RuntimeUpdateSnapshot::with_current_pointer_position(Some(current_pointer)),
        );

        update(&mut state, Message::Frame, &mut context);

        assert_eq!(state.planner_drag_pointer, Some(current_pointer));
        let current_preview = planner_drag_preview_rect(&state)
            .expect("an active planner drag should paint its preview after frame sync");
        assert_ne!(
            previous_preview.min, current_preview.min,
            "native pointer changes must move the rendered drag preview"
        );
    }

    #[test]
    fn planner_drag_cancel_removes_the_rendered_preview() {
        let pointer = Point::new(120.0, 90.0);
        let mut state = planner_drag_state(pointer);
        assert!(planner_drag_preview_rect(&state).is_some());
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::PlannerCardDrag {
                track_id: String::from("drag"),
                message: ui::DragHandleMessage::Cancelled { position: pointer },
            },
            &mut context,
        );

        assert!(state.planner_drag_source_track_id.is_none());
        assert!(
            planner_drag_preview_rect(&state).is_none(),
            "canceling a planner drag must remove its rendered preview"
        );
    }

    #[test]
    fn successful_planner_stage_drop_removes_the_rendered_preview() {
        let mut state = planner_drag_state(Point::new(120.0, 90.0));
        assert!(planner_drag_preview_rect(&state).is_some());
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::PlannerStageDropped(TrackStage::Production),
            &mut context,
        );

        assert_eq!(state.library.tracks[0].stage, TrackStage::Production);
        assert!(
            planner_drag_preview_rect(&state).is_none(),
            "a successful stage drop must remove its rendered preview"
        );
    }

    #[test]
    fn open_stage_dropdown_projects_all_selectable_stages() {
        let track = Track {
            id: String::from("stage-menu"),
            title: String::from("Stage menu"),
            original_name: String::from("stage-menu.wav"),
            path: PathBuf::from("/external/stage-menu.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };

        let labels = ui::scene(stage_menu_popover(&track, Point::new(80.0, 60.0)))
            .into_view()
            .view_frame_at_size_with_default_theme(Vector2::new(240.0, 180.0))
            .paint_plan
            .text_label_strings();

        for stage in [
            TrackStage::SoundDesign,
            TrackStage::Production,
            TrackStage::Mixdown,
            TrackStage::Mastering,
        ] {
            assert!(
                labels.iter().any(|label| label == stage.label()),
                "open stage dropdown must paint {} as an option",
                stage.label()
            );
        }
    }

    #[test]
    fn clicking_stage_trigger_opens_a_pointer_anchored_popover() {
        let track = Track {
            id: String::from("stage-trigger"),
            title: String::from("Stage trigger"),
            original_name: String::from("stage-trigger.wav"),
            path: PathBuf::from("/external/stage-trigger.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };
        #[derive(Clone)]
        struct TriggerState {
            track: Track,
            open: bool,
            anchor: Option<Point>,
        }
        let bridge = DeclarativeOwnedRuntimeBridge::new(
            TriggerState {
                track,
                open: false,
                anchor: None,
            },
            |state| {
                let trigger = stage_dropdown(&state.track, state.open, false);
                let trigger = if let Some(anchor) = state.anchor {
                    trigger
                        .overlays(ui::overlays().popover(stage_menu_popover(&state.track, anchor)))
                } else {
                    trigger
                };
                ui::scene(trigger).into_view().into_surface()
            },
            |state, message| {
                if let Message::ToggleStageMenuAt { position, .. } = message {
                    state.open = true;
                    state.anchor = Some(stage_menu_anchor_from_pointer(position));
                }
            },
        );
        let mut runtime = SurfaceRuntime::new(bridge, Vector2::new(240.0, 180.0));
        let click = Point::new(120.0, 12.0);

        assert!(runtime.widget_at(click).is_some());
        runtime.dispatch_event(Event::primary_press(click));
        runtime.dispatch_event(Event::primary_release(click));

        assert_eq!(
            runtime.bridge().state().anchor,
            Some(stage_menu_anchor_from_pointer(click))
        );
        let labels = runtime
            .frame(&ThemeTokens::default())
            .paint_plan
            .text_label_strings();
        assert!(
            labels
                .iter()
                .any(|label| label == TrackStage::Mastering.label()),
            "clicking the trigger should project the anchored menu"
        );
    }

    #[test]
    fn keyboard_stage_trigger_uses_a_visible_fallback_anchor() {
        let track = Track {
            id: String::from("keyboard-stage"),
            title: String::from("Keyboard stage"),
            original_name: String::from("keyboard-stage.wav"),
            path: PathBuf::from("/external/keyboard-stage.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };
        let track_id = track.id.clone();
        let mut state = AppState {
            busy: false,
            workspace_mode: super::WorkspaceMode::Planner,
            ..AppState::default()
        };
        state.library.tracks.push(track);
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::ToggleStageMenu(track_id), &mut context);

        assert!(state.stage_menu_anchor.is_some());
        let labels = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0))
            .paint_plan
            .text_label_strings();
        assert!(
            labels
                .iter()
                .any(|label| label == TrackStage::Mastering.label()),
            "keyboard activation should still project the stage menu"
        );
    }

    #[test]
    fn focused_stage_trigger_routes_keyboard_activation() {
        let track = Track {
            id: String::from("focused-stage"),
            title: String::from("Focused stage"),
            original_name: String::from("focused-stage.wav"),
            path: PathBuf::from("/external/focused-stage.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };
        #[derive(Clone)]
        struct KeyboardState {
            track: Track,
            open: bool,
            anchor: Option<Point>,
        }
        let bridge = DeclarativeOwnedRuntimeBridge::new(
            KeyboardState {
                track,
                open: false,
                anchor: None,
            },
            |state| {
                let trigger = stage_dropdown(&state.track, state.open, false);
                let trigger = if let Some(anchor) = state.anchor {
                    trigger
                        .overlays(ui::overlays().popover(stage_menu_popover(&state.track, anchor)))
                } else {
                    trigger
                };
                ui::scene(trigger).into_view().into_surface()
            },
            |state, message| {
                if let Message::ToggleStageMenu(_) = message {
                    state.open = true;
                    state.anchor = Some(Point::new(105.0, 96.0));
                }
            },
        );
        let mut runtime = SurfaceRuntime::new(bridge, Vector2::new(240.0, 180.0));
        let focused = runtime
            .traverse_focus(FocusTraversal::Forward)
            .expect("stage trigger should participate in keyboard focus");

        assert_eq!(
            runtime.dispatch_event(Event::key_press(ui::WidgetKey::Enter)),
            Some(focused)
        );
        assert!(runtime.bridge().state().open);
        let labels = runtime
            .frame(&ThemeTokens::default())
            .paint_plan
            .text_label_strings();
        assert!(
            labels
                .iter()
                .any(|label| label == TrackStage::Mastering.label()),
            "focused keyboard activation should project the stage menu"
        );
    }

    #[test]
    fn pointer_opening_projects_and_activates_menus_in_both_contexts() {
        let track = Track {
            id: String::from("context-stage"),
            title: String::from("Context stage"),
            original_name: String::from("context-stage.wav"),
            path: PathBuf::from("/external/context-stage.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };

        for workspace_mode in [super::WorkspaceMode::Planner, super::WorkspaceMode::Review] {
            let mut state = AppState {
                busy: false,
                workspace_mode,
                ..AppState::default()
            };
            state.library.tracks.push(track.clone());
            let bridge = DeclarativeOwnedRuntimeBridge::new(
                state,
                |state| project_surface(state).into_surface(),
                |state, message| match message {
                    Message::ToggleStageMenuAt { track_id, position } => {
                        state.stage_menu_track_id = Some(track_id);
                        state.stage_menu_anchor = Some(stage_menu_anchor_from_pointer(position));
                    }
                    Message::SetStage { stage, .. } => {
                        if let Some(track) = state.library.tracks.first_mut() {
                            track.stage = stage;
                        }
                        state.stage_menu_track_id = None;
                        state.stage_menu_anchor = None;
                    }
                    _ => {}
                },
            );
            let mut runtime = SurfaceRuntime::new(bridge, Vector2::new(1180.0, 720.0));
            let frame = runtime.frame(&ThemeTokens::default());
            let trigger_rect = frame
                .paint_plan
                .primitives
                .iter()
                .find_map(|primitive| match primitive {
                    PaintPrimitive::Text(text)
                        if text.text.as_str() == TrackStage::Production.label()
                            && text.rect.min.y > 190.0 =>
                    {
                        Some(text.rect)
                    }
                    _ => None,
                })
                .expect("context should paint a stage trigger");
            let trigger_point = Point::new(
                trigger_rect.min.x + trigger_rect.width() * 0.5,
                trigger_rect.min.y + trigger_rect.height() * 0.5,
            );

            assert!(runtime.widget_at(trigger_point).is_some());
            runtime.dispatch_event(Event::primary_press(trigger_point));
            runtime.dispatch_event(Event::primary_release(trigger_point));
            assert_eq!(
                runtime.bridge().state().stage_menu_anchor,
                Some(stage_menu_anchor_from_pointer(trigger_point))
            );

            let frame = runtime.frame(&ThemeTokens::default());
            let option_rect = frame
                .paint_plan
                .primitives
                .iter()
                .find_map(|primitive| match primitive {
                    PaintPrimitive::Text(text)
                        if text.text.as_str() == TrackStage::Mastering.label()
                            && text.rect.min.y > trigger_point.y =>
                    {
                        Some(text.rect)
                    }
                    _ => None,
                })
                .expect("opened context menu should paint a mastering option below the trigger");
            let anchor = stage_menu_anchor_from_pointer(trigger_point);
            let menu_surface = dropdown_surface_rect(&frame.paint_plan.primitives, anchor)
                .expect("opened context menu should paint its surface at the pointer anchor");
            assert!((menu_surface.min.x - anchor.x).abs() < 0.01);
            assert!((menu_surface.min.y - anchor.y).abs() < 0.01);
            let option_point = Point::new(
                option_rect.min.x + option_rect.width() * 0.5,
                option_rect.min.y + option_rect.height() * 0.5,
            );
            assert!(runtime.widget_at(option_point).is_some());
            runtime.dispatch_event(Event::primary_press(option_point));
            runtime.dispatch_event(Event::primary_release(option_point));
            assert!(runtime.bridge().state().stage_menu_track_id.is_none());
            assert_eq!(
                runtime.bridge().state().library.tracks[0].stage,
                TrackStage::Mastering
            );
            let labels_after_selection = runtime
                .frame(&ThemeTokens::default())
                .paint_plan
                .text_label_strings();
            assert!(
                labels_after_selection
                    .iter()
                    .any(|label| label == TrackStage::Mastering.label()),
                "selecting a stage should update the trigger label"
            );
        }
    }

    #[test]
    fn open_stage_dropdown_projects_an_interactive_popover() {
        let track = Track {
            id: String::from("stage-popover"),
            title: String::from("Stage popover"),
            original_name: String::from("stage-popover.wav"),
            path: PathBuf::from("/external/stage-popover.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };
        #[derive(Clone)]
        struct DropdownState {
            app: AppState,
        }
        let mut app = AppState {
            busy: false,
            workspace_mode: super::WorkspaceMode::Planner,
            stage_menu_track_id: Some(track.id.clone()),
            stage_menu_anchor: Some(Point::new(180.0, 117.0)),
            ..AppState::default()
        };
        app.library.tracks.push(track);
        let bridge = ui::app(DropdownState { app })
            .view(|state| project_surface(&state.app))
            .update(|state, message| {
                if let Message::SetStage { stage, .. } = message {
                    if let Some(track) = state.app.library.tracks.first_mut() {
                        track.stage = stage;
                    }
                    state.app.stage_menu_track_id = None;
                    state.app.stage_menu_anchor = None;
                }
            })
            .into_bridge();
        let mut runtime = SurfaceRuntime::new(bridge, Vector2::new(1180.0, 720.0));

        let frame = runtime.frame(&ThemeTokens::default());
        let sound_design_count_before = frame
            .paint_plan
            .text_label_strings()
            .iter()
            .filter(|label| *label == TrackStage::SoundDesign.label())
            .count();
        let option_rect = frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::Text(text)
                    if text.text.as_str() == TrackStage::Mastering.label()
                        && text.rect.min.x < 400.0
                        && text.rect.min.y > 117.0 =>
                {
                    Some(text.rect)
                }
                _ => None,
            })
            .expect("expanded dropdown should paint its popover options");
        let option_point = Point::new(
            option_rect.min.x + option_rect.width() * 0.5,
            option_rect.min.y + option_rect.height() * 0.5,
        );

        assert!(
            runtime.widget_at(option_point).is_some(),
            "expanded dropdown options should remain interactive outside the trigger box"
        );

        runtime.dispatch_event(Event::primary_press(option_point));
        runtime.dispatch_event(Event::primary_release(option_point));
        let labels_after_selection = runtime
            .frame(&ThemeTokens::default())
            .paint_plan
            .text_label_strings();
        assert!(
            labels_after_selection
                .iter()
                .any(|label| label == TrackStage::Mastering.label()),
            "selecting an option should update the trigger label"
        );
        let sound_design_count_after = labels_after_selection
            .iter()
            .filter(|label| *label == TrackStage::SoundDesign.label())
            .count();
        assert_eq!(
            sound_design_count_after,
            sound_design_count_before - 1,
            "selecting an option should close the menu"
        );
    }

    #[test]
    fn stage_dropdown_popover_stays_below_its_planner_and_library_trigger() {
        let track = Track {
            id: String::from("stage-context"),
            title: String::from("Stage context"),
            original_name: String::from("stage-context.wav"),
            path: PathBuf::from("/external/stage-context.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };

        let planner_anchor = Point::new(220.0, 140.0);
        let mut planner_state = AppState {
            busy: false,
            workspace_mode: super::WorkspaceMode::Planner,
            stage_menu_track_id: Some(track.id.clone()),
            stage_menu_anchor: Some(planner_anchor),
            ..AppState::default()
        };
        planner_state.library.tracks.push(track.clone());
        let planner_frame = project_surface(&planner_state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0));
        let planner_menu_rect =
            dropdown_surface_rect(&planner_frame.paint_plan.primitives, planner_anchor)
                .expect("planner stage menu should paint its anchored surface");

        let library_anchor = Point::new(180.0, 140.0);
        let mut library_state = AppState {
            busy: false,
            workspace_mode: super::WorkspaceMode::Review,
            stage_menu_track_id: Some(track.id.clone()),
            stage_menu_anchor: Some(library_anchor),
            ..AppState::default()
        };
        library_state.library.tracks.push(track);
        let library_frame = project_surface(&library_state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0));
        let library_menu_rect =
            dropdown_surface_rect(&library_frame.paint_plan.primitives, library_anchor)
                .expect("library stage menu should paint its anchored surface");

        assert!(
            (planner_menu_rect.min.x - planner_anchor.x).abs() < 0.01
                && (planner_menu_rect.min.y - planner_anchor.y).abs() < 0.01,
            "planner menu should use its supplied root anchor"
        );
        assert!(
            (library_menu_rect.min.x - library_anchor.x).abs() < 0.01
                && (library_menu_rect.min.y - library_anchor.y).abs() < 0.01,
            "library menu should use its supplied root anchor"
        );
    }

    #[test]
    fn expanded_reference_selector_is_clamped_and_interactive_at_1180px() {
        let track_id = String::from("reference-selector-track");
        let first_path = PathBuf::from("/external/first-reference.wav");
        let second_path = PathBuf::from("/external/second-reference.wav");
        let mut state = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Review,
            reference_menu_track_id: Some(track_id.clone()),
            reference_menu_anchor: Some(Point::new(1_080.0, 42.0)),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id.clone(),
            title: String::from("Reference selector track"),
            original_name: String::from("main.wav"),
            path: PathBuf::from("/external/main.wav"),
            reference_path: Some(first_path.clone()),
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        state.library.reference_tracks = vec![
            ReferenceTrack {
                path: first_path,
                notes: Vec::new(),
            },
            ReferenceTrack {
                path: second_path.clone(),
                notes: Vec::new(),
            },
        ];
        let bridge = DeclarativeOwnedRuntimeBridge::new(
            state,
            |state| project_surface(state).into_surface(),
            |state, message| {
                if let Message::SetReferenceTrack { track_id, path } = message {
                    assert_eq!(track_id, "reference-selector-track");
                    state.library.tracks[0].reference_path = Some(path);
                    state.reference_menu_track_id = None;
                    state.reference_menu_anchor = None;
                }
            },
        );
        let mut runtime = SurfaceRuntime::new(bridge, Vector2::new(1_180.0, 1_000.0));
        let frame = runtime.frame(&ThemeTokens::default());
        let menu_rect = frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if fill.color == ThemeTokens::default().surface_overlay
                        && fill.rect.width() >= REFERENCE_MENU_WIDTH - 1.0 =>
                {
                    Some(fill.rect)
                }
                _ => None,
            })
            .expect("expanded reference selector should paint a menu surface");
        assert!(menu_rect.min.x >= 0.0);
        assert!(menu_rect.max.x <= 1_180.0);
        assert!(menu_rect.min.y >= 0.0);
        assert!(menu_rect.max.y <= 1_000.0);

        let option_rect = frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::Text(text) if text.text.as_str() == "second-reference.wav" => {
                    Some(text.rect)
                }
                _ => None,
            })
            .expect("expanded reference selector should paint the second option");
        let option_point = Point::new(
            option_rect.min.x + option_rect.width() * 0.5,
            option_rect.min.y + option_rect.height() * 0.5,
        );
        assert!(runtime.widget_at(option_point).is_some());
        runtime.dispatch_event(Event::primary_press(option_point));
        runtime.dispatch_event(Event::primary_release(option_point));

        assert_eq!(
            runtime.bridge().state().library.tracks[0].reference_path,
            Some(second_path)
        );
        assert!(runtime.bridge().state().reference_menu_track_id.is_none());
    }

    #[test]
    fn open_status_dropdown_projects_statuses_in_product_order() {
        let track = Track {
            id: String::from("status-menu"),
            title: String::from("Status menu"),
            original_name: String::from("status-menu.wav"),
            path: PathBuf::from("/external/status-menu.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Refine,
            notes: Vec::new(),
        };
        let expected = ["Inbox", "Refine", "Release", "Archive", "Maybe"];
        let actual = ui::scene(status_dropdown_for_host(
            &track,
            true,
            false,
            StatusMenuHost::Library,
        ))
        .into_view()
        .view_frame_at_size_with_default_theme(Vector2::new(240.0, 220.0))
        .paint_plan
        .text_runs()
        .filter(|run| run.rect.min.y > ui::dropdown_trigger_height())
        .filter(|run| !run.text.is_empty())
        .map(|run| run.text.as_str().to_owned())
        .collect::<Vec<_>>();

        assert_eq!(actual, expected.map(String::from).to_vec());
    }

    #[test]
    fn status_transition_preserves_stage_and_favorite_and_saves_only_on_change() {
        let mut state = AppState {
            busy: false,
            status_menu_track_id: Some(String::from("status-track")),
            ..AppState::default()
        };
        state.library.tracks.push(Track {
            id: String::from("status-track"),
            title: String::from("Status track"),
            original_name: String::from("status-track.wav"),
            path: PathBuf::from("/external/status-track.wav"),
            reference_path: Some(PathBuf::from("/external/reference.wav")),
            size: 42,
            favorite: true,
            stage: TrackStage::Mixdown,
            status: TrackStatus::Inbox,
            notes: vec![Note {
                id: String::from("status-note"),
                time_millis: 500,
                body: String::from("Keep this note."),
                done: false,
            }],
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::SetStatus {
                track_id: String::from("status-track"),
                status: TrackStatus::Release,
            },
            &mut context,
        );

        let track = &state.library.tracks[0];
        assert_eq!(track.status, TrackStatus::Release);
        assert_eq!(track.stage, TrackStage::Mixdown);
        assert!(track.favorite);
        assert_eq!(track.notes.len(), 1);
        assert!(state.status_menu_track_id.is_none());
        assert!(state.save_in_flight);
        assert_eq!(state.status, "Status set to Release.");
    }

    #[test]
    fn no_op_status_transition_closes_menu_without_scheduling_a_save() {
        let mut state = AppState {
            busy: false,
            status_menu_track_id: Some(String::from("status-track")),
            ..AppState::default()
        };
        state.library.tracks.push(Track {
            id: String::from("status-track"),
            title: String::from("Status track"),
            original_name: String::from("status-track.wav"),
            path: PathBuf::from("/external/status-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            status: TrackStatus::Maybe,
            notes: Vec::new(),
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::SetStatus {
                track_id: String::from("status-track"),
                status: TrackStatus::Maybe,
            },
            &mut context,
        );

        assert_eq!(state.library.tracks[0].status, TrackStatus::Maybe);
        assert!(!state.save_in_flight);
        assert!(!state.save_again);
        assert!(state.status_menu_track_id.is_none());
    }

    #[test]
    fn opening_stage_and_status_menus_closes_the_other_menu() {
        let mut state = AppState {
            busy: false,
            ..AppState::default()
        };
        state.library.tracks.push(Track {
            id: String::from("menu-track"),
            title: String::from("Menu track"),
            original_name: String::from("menu-track.wav"),
            path: PathBuf::from("/external/menu-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::ToggleStatusMenuAt {
                track_id: String::from("menu-track"),
                host: StatusMenuHost::Library,
            },
            &mut context,
        );
        assert!(state.stage_menu_track_id.is_none());
        assert_eq!(state.status_menu_track_id.as_deref(), Some("menu-track"));

        update(
            &mut state,
            Message::ToggleStageMenu(String::from("menu-track")),
            &mut context,
        );
        assert_eq!(state.stage_menu_track_id.as_deref(), Some("menu-track"));
        assert!(state.status_menu_track_id.is_none());
    }

    #[test]
    fn keyboard_status_dropdown_expands_selected_review_header() {
        let mut state = AppState {
            busy: false,
            workspace_mode: super::WorkspaceMode::Review,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(String::from("selected-header"));
        state.library.tracks.push(Track {
            id: String::from("selected-header"),
            title: String::from("Selected header"),
            original_name: String::from("selected-header.wav"),
            path: PathBuf::from("/external/selected-header.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });
        let bridge = DeclarativeOwnedRuntimeBridge::new(
            state,
            |state| project_surface(state).into_surface(),
            |state, message| {
                let mut context = ui::UiUpdateContext::default();
                update(state, message, &mut context);
            },
        );
        let mut runtime = SurfaceRuntime::new(bridge, Vector2::new(1180.0, 720.0));
        let _ = runtime.frame(&ThemeTokens::default());
        let frame = runtime.frame(&ThemeTokens::default());
        let (trigger, trigger_rect) = frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::Text(text)
                    if text.text.as_str() == TrackStatus::Inbox.label()
                        && text.rect.min.x < super::LIBRARY_WIDTH
                        && text.rect.min.y > 40.0 =>
                {
                    Some((text.widget_id, text.rect))
                }
                _ => None,
            })
            .expect("the selected library card should paint its status trigger");
        assert!(runtime.focus_widget(trigger));
        assert_eq!(
            runtime.dispatch_event(Event::key_press(ui::WidgetKey::Enter)),
            Some(trigger)
        );
        assert_eq!(
            runtime.bridge().state().status_menu_track_id.as_deref(),
            Some("selected-header")
        );

        let opened_frame = runtime.frame(&ThemeTokens::default());
        let release_option = opened_frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::Text(text)
                    if text.text.as_str() == TrackStatus::Release.label()
                        && text.rect.min.y > trigger_rect.min.y + trigger_rect.height()
                        && text.rect.min.x >= trigger_rect.min.x
                        && text.rect.min.x < trigger_rect.min.x + trigger_rect.width() =>
                {
                    Some(text.rect)
                }
                _ => None,
            })
            .expect("keyboard activation should project status options below the header trigger");
        runtime.dispatch_primary_click(Point::new(
            release_option.min.x + release_option.width() * 0.5,
            release_option.min.y + release_option.height() * 0.5,
        ));
        assert_eq!(
            runtime.bridge().state().library.tracks[0].status,
            TrackStatus::Release
        );
    }

    #[test]
    fn keyboard_status_dropdown_follows_lower_library_and_planner_triggers() {
        let track = |id: &str, status: TrackStatus| Track {
            id: String::from(id),
            title: String::from(id),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status,
            notes: Vec::new(),
        };

        let mut review_state = AppState {
            busy: false,
            workspace_mode: super::WorkspaceMode::Review,
            ..AppState::default()
        };
        review_state.library.selected_track_id = Some(String::from("library-target"));
        review_state
            .library
            .tracks
            .push(track("library-first", TrackStatus::Maybe));
        review_state
            .library
            .tracks
            .push(track("library-target", TrackStatus::Inbox));

        let review_bridge = DeclarativeOwnedRuntimeBridge::new(
            review_state,
            |state| project_surface(state).into_surface(),
            |state, message| {
                let mut context = ui::UiUpdateContext::default();
                update(state, message, &mut context);
            },
        );
        let mut review_runtime = SurfaceRuntime::new(review_bridge, Vector2::new(1180.0, 720.0));
        let review_frame = review_runtime.frame(&ThemeTokens::default());
        let (review_trigger, review_trigger_rect) = review_frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::Text(text)
                    if text.text.as_str() == TrackStatus::Inbox.label()
                        && text.rect.min.x < super::LIBRARY_WIDTH
                        && text.rect.min.y > 250.0 =>
                {
                    Some((text.widget_id, text.rect))
                }
                _ => None,
            })
            .expect("the lower library row should paint its status trigger");
        assert!(review_runtime.focus_widget(review_trigger));
        assert_eq!(
            review_runtime.dispatch_event(Event::key_press(ui::WidgetKey::Enter)),
            Some(review_trigger)
        );
        assert_eq!(
            review_runtime
                .bridge()
                .state()
                .status_menu_track_id
                .as_deref(),
            Some("library-target")
        );
        let review_option = review_runtime
            .frame(&ThemeTokens::default())
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::Text(text)
                    if text.text.as_str() == TrackStatus::Release.label()
                        && text.rect.min.y
                            > review_trigger_rect.min.y + review_trigger_rect.height() =>
                {
                    Some(text.rect)
                }
                _ => None,
            })
            .expect("keyboard activation should project status options below the library trigger");
        assert!(
            review_option.min.x < review_trigger_rect.max.x
                && review_option.max.x > review_trigger_rect.min.x,
            "library status options should remain horizontally attached to their trigger"
        );
        review_runtime.dispatch_primary_click(Point::new(
            review_option.min.x + review_option.width() * 0.5,
            review_option.min.y + review_option.height() * 0.5,
        ));
        assert_eq!(
            review_runtime.bridge().state().library.tracks[1].status,
            TrackStatus::Release
        );

        let mut planner_state = AppState {
            busy: false,
            workspace_mode: super::WorkspaceMode::Planner,
            ..AppState::default()
        };
        planner_state
            .library
            .tracks
            .push(track("planner-first", TrackStatus::Maybe));
        planner_state
            .library
            .tracks
            .push(track("planner-target", TrackStatus::Inbox));

        let planner_bridge = DeclarativeOwnedRuntimeBridge::new(
            planner_state,
            |state| project_surface(state).into_surface(),
            |state, message| {
                let mut context = ui::UiUpdateContext::default();
                update(state, message, &mut context);
            },
        );
        let mut planner_runtime = SurfaceRuntime::new(planner_bridge, Vector2::new(1180.0, 720.0));
        let planner_frame = planner_runtime.frame(&ThemeTokens::default());
        let (planner_trigger, planner_trigger_rect) = planner_frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::Text(text)
                    if text.text.as_str() == TrackStatus::Inbox.label()
                        && text.rect.min.y > 250.0 =>
                {
                    Some((text.widget_id, text.rect))
                }
                _ => None,
            })
            .expect("the non-first planner card should paint its status trigger");
        assert!(planner_runtime.focus_widget(planner_trigger));
        assert_eq!(
            planner_runtime.dispatch_event(Event::key_press(ui::WidgetKey::Enter)),
            Some(planner_trigger)
        );
        assert_eq!(
            planner_runtime
                .bridge()
                .state()
                .status_menu_track_id
                .as_deref(),
            Some("planner-target")
        );
        let planner_option = planner_runtime
            .frame(&ThemeTokens::default())
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::Text(text)
                    if text.text.as_str() == TrackStatus::Release.label()
                        && text.rect.min.y
                            > planner_trigger_rect.min.y + planner_trigger_rect.height() =>
                {
                    Some(text.rect)
                }
                _ => None,
            })
            .expect("keyboard activation should project status options below the planner trigger");
        assert!(
            planner_option.min.x < planner_trigger_rect.max.x
                && planner_option.max.x > planner_trigger_rect.min.x,
            "planner status options should remain horizontally attached to their trigger"
        );
        planner_runtime.dispatch_primary_click(Point::new(
            planner_option.min.x + planner_option.width() * 0.5,
            planner_option.min.y + planner_option.height() * 0.5,
        ));
        assert_eq!(
            planner_runtime.bridge().state().library.tracks[1].status,
            TrackStatus::Release
        );
    }

    #[test]
    fn audition_play_starts_a_ready_track_without_toggle_pause() {
        let mut state = audition_state(&["ready"]);
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::AuditionPlay, &mut context);

        assert!(state.transport_polling);
        assert!(state.transport_waiting_token.is_some());
        assert!(state.audition_auto_advance);
        assert!(state.audition_pending_play_track_id.is_none());
    }

    #[test]
    fn audition_play_arms_pending_autoplay_while_the_track_loads() {
        let mut state = audition_state(&["pending"]);
        state.waveform = None;
        state.waveform_track_id = None;
        state.waveform_busy = true;
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::AuditionPlay, &mut context);

        assert!(state.audition_auto_advance);
        assert_eq!(
            state.audition_pending_play_track_id.as_deref(),
            Some("pending")
        );
        assert!(!state.transport_playing);
        assert!(!state.transport_polling);
    }

    #[test]
    fn audition_play_does_not_pause_active_playback() {
        let mut state = audition_state(&["active"]);
        state.transport_playing = true;
        state.audition_auto_advance = true;
        state.audition_play_token = Some(7);
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::AuditionPlay, &mut context);

        assert!(state.transport_playing);
        assert!(!state.transport_polling);
        assert_eq!(state.audition_play_token, Some(7));
        assert_eq!(state.status, "Audition playback is already active.");
    }

    #[test]
    fn audition_controls_are_inert_outside_audition_and_while_busy() {
        for mut state in [
            AppState {
                workspace_mode: WorkspaceMode::Review,
                ..audition_state(&["review"])
            },
            AppState {
                busy: true,
                ..audition_state(&["busy"])
            },
        ] {
            let selected_before = state.library.selected_track_id.clone();
            let queue_before = state.audition_queue.clone();
            let round_before = state.audition_shuffle_round;
            let mut context = ui::UiUpdateContext::default();

            for message in [
                Message::AuditionPlay,
                Message::AuditionPrevious,
                Message::AuditionNext,
                Message::ShuffleAudition,
            ] {
                update(&mut state, message, &mut context);
            }

            assert_eq!(state.library.selected_track_id, selected_before);
            assert_eq!(state.audition_queue, queue_before);
            assert_eq!(state.audition_shuffle_round, round_before);
            assert!(state.audition_pending_play_track_id.is_none());
        }
    }

    #[test]
    fn audition_stop_clears_pending_autoplay_without_active_transport() {
        let mut state = audition_state(&["pending-stop"]);
        state.audition_auto_advance = true;
        state.audition_pending_play_track_id = Some(String::from("pending-stop"));
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::StopPlayback, &mut context);

        assert!(!state.audition_auto_advance);
        assert!(state.audition_play_token.is_none());
        assert!(state.audition_pending_play_track_id.is_none());
        assert!(!state.transport_playing);
        assert!(!state.transport_polling);
    }

    #[test]
    fn audition_next_resolves_selected_id_marks_current_heard_and_arms_destination() {
        let mut state = audition_state(&["a", "b", "c"]);
        state.library.selected_track_id = Some(String::from("b"));
        state.audition_queue_index = 0;
        state.audition_heard = vec![String::from("a"), String::from("c")];
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::AuditionNext, &mut context);

        assert_eq!(state.library.selected_track_id.as_deref(), Some("c"));
        assert_eq!(state.audition_queue_index, 2);
        assert!(state.audition_heard.iter().any(|id| id == "a"));
        assert!(state.audition_heard.iter().any(|id| id == "b"));
        assert!(!state.audition_heard.iter().any(|id| id == "c"));
        assert_eq!(state.audition_pending_play_track_id.as_deref(), Some("c"));
        assert!(state.audition_auto_advance);
    }

    #[test]
    fn audition_previous_resolves_selected_id_without_marking_interrupted_current() {
        let mut state = audition_state(&["a", "b", "c"]);
        state.library.selected_track_id = Some(String::from("b"));
        state.audition_queue_index = 2;
        state.audition_heard.clear();
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::AuditionPrevious, &mut context);

        assert_eq!(state.library.selected_track_id.as_deref(), Some("a"));
        assert_eq!(state.audition_queue_index, 0);
        assert!(!state.audition_heard.iter().any(|id| id == "a"));
        assert!(!state.audition_heard.iter().any(|id| id == "b"));
        assert_eq!(state.audition_pending_play_track_id.as_deref(), Some("a"));
        assert!(state.audition_auto_advance);
    }

    #[test]
    fn audition_next_keeps_pool_anchor_when_current_track_changes_status() {
        let mut state = audition_state(&["inbox-a", "inbox-b"]);
        state.transport_playing = true;
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::SetStatus {
                track_id: String::from("inbox-a"),
                status: TrackStatus::Archive,
            },
            &mut context,
        );
        assert_eq!(
            state.library.tracks[0].status,
            TrackStatus::Archive,
            "the current track should still move out of the selected pool"
        );
        assert_eq!(
            state.audition_queue,
            vec![String::from("inbox-a"), String::from("inbox-b")]
        );

        update(&mut state, Message::AuditionNext, &mut context);

        assert_eq!(state.library.selected_track_id.as_deref(), Some("inbox-b"));
        assert_eq!(state.audition_queue, vec![String::from("inbox-b")]);
        assert_eq!(state.audition_queue_index, 0);
        assert_eq!(
            state.audition_pending_play_track_id.as_deref(),
            Some("inbox-b")
        );

        update(&mut state, Message::AuditionPrevious, &mut context);

        assert_eq!(state.library.selected_track_id.as_deref(), Some("inbox-b"));
        assert_eq!(state.audition_queue, vec![String::from("inbox-b")]);
        assert!(state.status.contains("beginning"));
    }

    #[test]
    fn audition_next_keeps_pool_anchor_during_reference_only_playback() {
        let mut state = audition_state(&["inbox-a", "inbox-b"]);
        state.reference_transport = Some(transport::AudioTransport::spawn());
        state.reference_transport_playing = true;
        state.reference_only_playback = true;
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::SetStatus {
                track_id: String::from("inbox-a"),
                status: TrackStatus::Archive,
            },
            &mut context,
        );
        assert_eq!(
            state.audition_queue,
            vec![String::from("inbox-a"), String::from("inbox-b")]
        );

        update(&mut state, Message::AuditionNext, &mut context);

        assert_eq!(state.library.selected_track_id.as_deref(), Some("inbox-b"));
        assert_eq!(state.audition_queue, vec![String::from("inbox-b")]);
    }

    #[test]
    fn audition_navigation_boundaries_leave_selection_and_playback_unchanged() {
        let mut next_state = audition_state(&["a", "b", "c"]);
        next_state.library.selected_track_id = Some(String::from("c"));
        next_state.audition_queue_index = 0;
        next_state.transport_playing = true;
        let mut context = ui::UiUpdateContext::default();

        update(&mut next_state, Message::AuditionNext, &mut context);

        assert_eq!(next_state.library.selected_track_id.as_deref(), Some("c"));
        assert_eq!(next_state.audition_queue_index, 0);
        assert!(next_state.transport_playing);
        assert!(next_state.audition_pending_play_track_id.is_none());
        assert!(next_state.status.contains("end"));

        let mut previous_state = audition_state(&["a", "b", "c"]);
        previous_state.library.selected_track_id = Some(String::from("a"));
        previous_state.audition_queue_index = 2;
        previous_state.transport_playing = true;

        update(&mut previous_state, Message::AuditionPrevious, &mut context);

        assert_eq!(
            previous_state.library.selected_track_id.as_deref(),
            Some("a")
        );
        assert_eq!(previous_state.audition_queue_index, 2);
        assert!(previous_state.transport_playing);
        assert!(previous_state.audition_pending_play_track_id.is_none());
        assert!(previous_state.status.contains("beginning"));
    }

    #[test]
    fn audition_shuffle_restarts_active_playback_with_a_new_order_and_current() {
        let mut state = audition_state(&["a", "b", "c"]);
        state.library.selected_track_id = Some(String::from("b"));
        state.audition_queue = vec![String::from("a"), String::from("b"), String::from("c")];
        state.audition_queue_index = 1;
        state.audition_heard = vec![String::from("a")];
        state.audition_shuffle_round = 4;
        state.transport_playing = true;
        let previous_order = state.audition_queue.clone();
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::ShuffleAudition, &mut context);

        assert_eq!(state.audition_shuffle_round, 5);
        assert_ne!(state.audition_queue, previous_order);
        assert_eq!(
            state.library.selected_track_id.as_deref(),
            state.audition_queue.first().map(String::as_str)
        );
        assert_ne!(state.audition_queue.first().map(String::as_str), Some("b"));
        assert!(state.audition_heard.is_empty());
        assert_eq!(
            state.audition_pending_play_track_id.as_deref(),
            state.audition_queue.first().map(String::as_str)
        );
        assert!(state.audition_auto_advance);
        assert!(!state.transport_playing);
    }

    #[test]
    fn audition_shuffle_replays_single_entry_and_resets_empty_queue() {
        let mut single_state = audition_state(&["only"]);
        single_state.audition_heard = vec![String::from("only")];
        single_state.audition_shuffle_round = 2;
        let mut context = ui::UiUpdateContext::default();

        update(&mut single_state, Message::ShuffleAudition, &mut context);

        assert_eq!(single_state.audition_shuffle_round, 3);
        assert_eq!(single_state.audition_queue, vec![String::from("only")]);
        assert_eq!(
            single_state.library.selected_track_id.as_deref(),
            Some("only")
        );
        assert!(single_state.audition_heard.is_empty());
        assert_eq!(
            single_state.audition_pending_play_track_id.as_deref(),
            Some("only")
        );

        let mut empty_state = audition_state(&[]);
        empty_state.transport_playing = true;
        empty_state.reference_transport = Some(transport::AudioTransport::spawn());
        empty_state.reference_transport_playing = true;
        empty_state.audition_heard = vec![String::from("gone")];

        update(&mut empty_state, Message::ShuffleAudition, &mut context);

        assert!(empty_state.audition_queue.is_empty());
        assert!(empty_state.library.selected_track_id.is_none());
        assert!(empty_state.audition_heard.is_empty());
        assert!(!empty_state.transport_playing);
        assert!(!empty_state.reference_transport_playing);
        assert!(empty_state.audition_pending_play_track_id.is_none());
        assert_eq!(empty_state.status, "No tracks in Inbox.");
    }

    #[test]
    fn audition_stale_decode_generation_does_not_complete_pending_playback() {
        let mut state = audition_state(&["stale"]);
        state.waveform = None;
        state.waveform_track_id = None;
        state.waveform_busy = true;
        state.waveform_generation = 2;
        state.audition_auto_advance = true;
        state.audition_pending_play_track_id = Some(String::from("stale"));
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::DecodeCompleted {
                track_id: String::from("stale"),
                generation: 1,
                result: Ok(audition_waveform()),
            },
            &mut context,
        );

        assert!(state.waveform.is_none());
        assert!(state.waveform_busy);
        assert_eq!(state.waveform_generation, 2);
        assert_eq!(
            state.audition_pending_play_track_id.as_deref(),
            Some("stale")
        );
    }

    #[test]
    fn audition_shuffle_is_deterministic_and_preserves_the_filtered_tracks() {
        let ids = vec![
            String::from("track-a"),
            String::from("track-b"),
            String::from("track-c"),
            String::from("track-d"),
        ];
        let seed = audition_shuffle_seed(TrackStatus::Inbox, &ids, 0);
        let mut first = ids.clone();
        let mut second = ids.clone();
        deterministic_shuffle(&mut first, seed);
        deterministic_shuffle(&mut second, seed);

        assert_eq!(first, second);
        let mut sorted = first;
        sorted.sort();
        assert_eq!(sorted, ids);
        assert_ne!(
            audition_shuffle_seed(TrackStatus::Inbox, &ids, 0),
            audition_shuffle_seed(TrackStatus::Inbox, &ids, 1)
        );
        assert_eq!(audition_statuses().len(), 5);
    }

    #[test]
    fn audition_queue_filters_and_updates_membership_without_reordering() {
        let track = |id: &str, status: TrackStatus| Track {
            id: String::from(id),
            title: String::from(id),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status,
            notes: Vec::new(),
        };
        let mut state = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Audition,
            ..AppState::default()
        };
        state.library.tracks = vec![
            track("inbox-a", TrackStatus::Inbox),
            track("refine-a", TrackStatus::Refine),
            track("inbox-b", TrackStatus::Inbox),
        ];
        rebuild_audition_queue(&mut state);
        assert_eq!(state.audition_queue.len(), 2);
        assert!(state.audition_queue.iter().all(|id| {
            state
                .library
                .tracks
                .iter()
                .find(|track| &track.id == id)
                .is_some_and(|track| track.status == TrackStatus::Inbox)
        }));
        let original_order = state.audition_queue.clone();

        state.library.tracks[0].status = TrackStatus::Archive;
        sync_audition_queue_after_status_change(&mut state, "inbox-a");
        assert!(!state.audition_queue.iter().any(|id| id == "inbox-a"));

        state.library.tracks[1].status = TrackStatus::Inbox;
        sync_audition_queue_after_status_change(&mut state, "refine-a");
        assert_eq!(state.audition_queue.len(), 2);
        assert_eq!(
            state.audition_queue.first(),
            original_order
                .iter()
                .find(|id| *id == "inbox-b")
                .or_else(|| original_order.iter().find(|id| *id == "inbox-a"))
        );
        assert!(state.audition_queue.iter().any(|id| id == "refine-a"));
    }

    #[test]
    fn status_change_advances_pending_audition_before_decode_can_play_it() {
        let pending_id = String::from("pending-audition-track");
        let next_id = String::from("next-audition-track");
        let track = |id: &str, status: TrackStatus| Track {
            id: String::from(id),
            title: String::from(id),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status,
            notes: Vec::new(),
        };
        let mut state = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Audition,
            audition_status_filter: TrackStatus::Inbox,
            audition_queue: vec![pending_id.clone(), next_id.clone()],
            audition_queue_index: 0,
            audition_auto_advance: true,
            audition_pending_play_track_id: Some(pending_id.clone()),
            waveform: Some(WaveformData {
                sample_rate: 48_000,
                channels: 1,
                duration_millis: 1_000,
                render_frames: 48_000,
                integrated_lufs: Some(-7.0),
                loudness_profile: Arc::from([]),
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.0, 0.0, 0.0, 0.0],
                        4,
                        1,
                    ),
                ),
            }),
            waveform_track_id: Some(pending_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(pending_id.clone());
        state.library.tracks = vec![
            track(&pending_id, TrackStatus::Inbox),
            track(&next_id, TrackStatus::Inbox),
        ];
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::SetStatus {
                track_id: pending_id.clone(),
                status: TrackStatus::Archive,
            },
            &mut context,
        );

        assert_eq!(
            state.library.selected_track_id.as_deref(),
            Some(next_id.as_str())
        );
        assert_eq!(state.audition_queue, vec![next_id.clone()]);
        assert_eq!(
            state.audition_pending_play_track_id.as_deref(),
            Some(next_id.as_str())
        );
        assert_ne!(
            state.audition_pending_play_track_id.as_deref(),
            Some(pending_id.as_str())
        );

        update(&mut state, Message::Frame, &mut context);

        assert!(!state.transport_playing);
        assert!(!state.transport_polling);
        assert_eq!(
            state.audition_pending_play_track_id.as_deref(),
            Some(next_id.as_str())
        );
    }

    fn schedule_async_transport_error(
        transport: transport::AudioTransport,
        generation: u64,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(4));
            transport.set_error_for_test(
                generation,
                String::from("Could not open test-audio.wav for playback: test failure"),
            );
        })
    }

    fn wait_for_frame_state(
        state: &mut AppState,
        context: &mut ui::UiUpdateContext<Message>,
        predicate: impl Fn(&AppState) -> bool,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            update(state, Message::Frame, context);
            if predicate(state) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the asynchronous transport state did not arrive"
            );
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    #[test]
    fn frame_transport_load_error_advances_pending_audition() {
        let failed_id = String::from("failed-audition-track");
        let next_id = String::from("next-audition-track");
        let track = |id: &str, path: PathBuf| Track {
            id: String::from(id),
            title: String::from(id),
            original_name: format!("{id}.wav"),
            path,
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };
        let mut state = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Audition,
            audition_queue: vec![failed_id.clone(), next_id.clone()],
            audition_queue_index: 0,
            audition_auto_advance: true,
            audition_pending_play_track_id: Some(failed_id.clone()),
            ..AppState::default()
        };
        state.library.selected_track_id = Some(failed_id.clone());
        state.library.tracks = vec![
            track(
                &failed_id,
                PathBuf::from("/external/failed-audition-track.wav"),
            ),
            track(&next_id, PathBuf::from("/external/next-audition-track.wav")),
        ];
        let error_thread =
            schedule_async_transport_error(state.transport.clone(), state.transport_generation);
        state.transport_waiting_token = Some(1);
        state.transport_polling = true;
        let mut context = ui::UiUpdateContext::default();

        wait_for_frame_state(&mut state, &mut context, |state| {
            state.library.selected_track_id.as_deref() == Some(next_id.as_str())
        });
        error_thread
            .join()
            .expect("the asynchronous transport error should publish");

        assert!(state.audition_heard.iter().any(|id| id == &failed_id));
        assert_eq!(state.audition_queue_index, 1);
        assert_eq!(
            state.audition_pending_play_track_id.as_deref(),
            Some(next_id.as_str())
        );
        assert!(state.audition_auto_advance);
        assert_eq!(
            state.status,
            format!("Loading next audition track: {next_id}…")
        );
    }

    #[test]
    fn acknowledged_audition_play_advances_without_a_playing_frame_even_with_reference_loop() {
        let finished_id = String::from("finished-audition-track");
        let next_id = String::from("next-audition-track");
        let track = |id: &str| Track {
            id: String::from(id),
            title: String::from(id),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            reference_path: Some(PathBuf::from(format!("/external/{id}-reference.wav"))),
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };
        let mut state = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Audition,
            audition_queue: vec![finished_id.clone(), next_id.clone()],
            audition_queue_index: 0,
            audition_auto_advance: true,
            audition_play_token: Some(1),
            transport_polling: true,
            transport_waiting_token: Some(1),
            waveform: Some(WaveformData {
                sample_rate: 48_000,
                channels: 1,
                duration_millis: 1,
                render_frames: 48,
                integrated_lufs: Some(-7.0),
                loudness_profile: Arc::from([]),
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.0, 0.0, 0.0, 0.0],
                        4,
                        1,
                    ),
                ),
            }),
            waveform_track_id: Some(finished_id.clone()),
            reference_waveform: Some(WaveformData {
                sample_rate: 48_000,
                channels: 1,
                duration_millis: 1_000,
                render_frames: 48_000,
                integrated_lufs: Some(-7.0),
                loudness_profile: Arc::from([]),
                summary: Arc::new(
                    radiant::runtime::GpuSignalSummary::from_interleaved_samples(
                        &[0.0, 0.0, 0.0, 0.0],
                        4,
                        1,
                    ),
                ),
            }),
            reference_waveform_track_id: Some(finished_id.clone()),
            loop_selections: LoopSelections {
                main: None,
                reference: Some(LoopSelection {
                    start_ratio: 0.25,
                    end_ratio: 0.75,
                }),
            },
            ..AppState::default()
        };
        state.library.selected_track_id = Some(finished_id.clone());
        state.library.tracks = vec![track(&finished_id), track(&next_id)];
        state.transport.set_snapshot_for_test(Snapshot {
            generation: state.transport_generation,
            acknowledged_token: 1,
            position_millis: 1,
            playing: false,
            ready: true,
        });
        let mut context = ui::UiUpdateContext::default();

        update(&mut state, Message::Frame, &mut context);

        assert_eq!(
            state.library.selected_track_id.as_deref(),
            Some(next_id.as_str())
        );
        assert_eq!(
            state.audition_pending_play_track_id.as_deref(),
            Some(next_id.as_str())
        );
        assert_eq!(state.audition_queue_index, 1);
        assert!(!state.transport_playing);
    }

    #[test]
    fn frame_transport_load_error_retains_manual_stop_and_report() {
        let mut state = AppState {
            busy: false,
            status: String::from("Preparing playback…"),
            transport_playing: true,
            transport_polling: true,
            ..AppState::default()
        };
        let error_thread =
            schedule_async_transport_error(state.transport.clone(), state.transport_generation);
        state.transport_waiting_token = Some(1);
        let mut context = ui::UiUpdateContext::default();

        wait_for_frame_state(&mut state, &mut context, |state| {
            state.status != "Preparing playback…"
        });
        error_thread
            .join()
            .expect("the asynchronous transport error should publish");

        assert!(state.status.starts_with("Could not "));
        assert!(!state.transport_playing);
        assert!(!state.transport_polling);
        assert!(state.transport_waiting_token.is_none());
        assert!(!state.audition_auto_advance);
        assert!(state.audition_pending_play_track_id.is_none());
    }

    #[test]
    fn workspace_tabs_select_review_planner_and_audition_directly() {
        let mut state = AppState {
            busy: false,
            ..AppState::default()
        };
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::SelectWorkspace(WorkspaceMode::Planner),
            &mut context,
        );
        assert_eq!(state.workspace_mode, WorkspaceMode::Planner);
        update(
            &mut state,
            Message::SelectWorkspace(WorkspaceMode::Audition),
            &mut context,
        );
        assert_eq!(state.workspace_mode, WorkspaceMode::Audition);
        update(
            &mut state,
            Message::SelectWorkspace(WorkspaceMode::Review),
            &mut context,
        );
        assert_eq!(state.workspace_mode, WorkspaceMode::Review);
    }

    #[test]
    fn integrated_titlebar_keeps_workspace_tabs_clear_of_traffic_lights_and_global_controls_visible()
     {
        let track_id = String::from("titlebar-layout-track");
        let mut state = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Review,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks.push(Track {
            id: track_id,
            title: String::from("Titlebar layout track"),
            original_name: String::from("titlebar-layout-track.wav"),
            path: PathBuf::from("/external/titlebar-layout-track.wav"),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        });

        let frame = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0));
        let text_bounds = |label: &str| {
            frame
                .paint_plan
                .primitives
                .iter()
                .find_map(|primitive| match primitive {
                    PaintPrimitive::Text(text) if text.text.as_str() == label => Some(text.rect),
                    _ => None,
                })
        };
        let first_tab = text_bounds("Review").expect("the Review workspace tab should render");
        assert!(
            first_tab.min.x >= TITLEBAR_TRAFFIC_LIGHT_SAFE_GUTTER,
            "the first workspace tab must start after the native traffic-light safe gutter: {first_tab:?}"
        );

        for label in ["MATCH REF", "Import reference"] {
            let bounds = text_bounds(label)
                .unwrap_or_else(|| panic!("the right-side global control {label:?} should render"));
            assert!(
                bounds.max.x <= 1180.0,
                "the right-side global control {label:?} must remain inside the 1180px frame: {bounds:?}"
            );
        }
    }

    #[test]
    fn audition_surface_projects_filters_queue_and_shuffle_control() {
        let track = |id: &str, status: TrackStatus| Track {
            id: String::from(id),
            title: String::from(id),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status,
            notes: Vec::new(),
        };
        let mut state = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Audition,
            ..AppState::default()
        };
        state.library.tracks = vec![
            track("inbox-track", TrackStatus::Inbox),
            track("refine-track", TrackStatus::Refine),
        ];
        rebuild_audition_queue(&mut state);
        let labels = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0))
            .paint_plan
            .text_label_strings();

        for label in [
            "AUDITION / PLAYLIST",
            "Fixed shuffle · one pass",
            "PLAY STATUS",
            "Inbox",
            "Refine",
            "Release",
            "Archive",
            "Maybe",
            "Previous",
            "Play",
            "Stop",
            "Next",
            "Shuffle",
            "01  inbox-track",
        ] {
            assert!(
                labels.iter().any(|painted| painted == label),
                "missing {label:?}"
            );
        }
        assert!(!labels.iter().any(|label| label == "refine-track"));
    }

    #[test]
    fn favorite_state_is_immediately_visible_in_all_track_lists() {
        let mut starred = audition_track("starred-track");
        starred.favorite = true;
        starred.stage = TrackStage::SoundDesign;
        let mut unstarred = audition_track("unstarred-track");
        unstarred.stage = TrackStage::Production;
        let mut state = AppState {
            busy: false,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(starred.id.clone());
        state.library.tracks = vec![starred, unstarred];
        state.audition_queue = vec![
            String::from("starred-track"),
            String::from("unstarred-track"),
        ];

        for mode in [
            WorkspaceMode::Review,
            WorkspaceMode::Planner,
            WorkspaceMode::Audition,
        ] {
            state.workspace_mode = mode;
            let labels = project_surface(&state)
                .view_frame_at_size_with_default_theme(Vector2::new(1_400.0, 900.0))
                .paint_plan
                .text_label_strings();
            assert!(
                labels.iter().any(|label| label == "★"),
                "missing starred marker in {mode:?}"
            );
            assert!(
                labels.iter().any(|label| label == "☆"),
                "missing unstarred marker in {mode:?}"
            );
        }
    }

    #[test]
    fn library_track_cards_paint_persistent_rails_and_stronger_outlines() {
        let mut state = AppState {
            busy: false,
            ..AppState::default()
        };
        let selected = audition_track("selected-track");
        let unselected = audition_track("unselected-track");
        state.library.selected_track_id = Some(selected.id.clone());
        state.library.tracks = vec![selected, unselected];

        let frame = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(800.0, 600.0));
        let theme = ThemeTokens::default();
        let is_track_card_polygon = |points: &[Point]| {
            if points.len() != 5 {
                return false;
            }
            let min_x = points
                .iter()
                .map(|point| point.x)
                .fold(f32::INFINITY, f32::min);
            let min_y = points
                .iter()
                .map(|point| point.y)
                .fold(f32::INFINITY, f32::min);
            let max_x = points
                .iter()
                .map(|point| point.x)
                .fold(f32::NEG_INFINITY, f32::max);
            let max_y = points
                .iter()
                .map(|point| point.y)
                .fold(f32::NEG_INFINITY, f32::max);
            max_x - min_x > 200.0 && max_y - min_y > 60.0
        };
        let card_fills = frame
            .paint_plan
            .fill_polygons()
            .filter(|fill| fill.color == theme.bg_primary && is_track_card_polygon(&fill.points))
            .collect::<Vec<_>>();
        assert_eq!(
            card_fills.len(),
            2,
            "selected and unselected library cards should each paint one chamfered base"
        );

        let selected_card_widget_id = card_fills
            .iter()
            .find_map(|fill| {
                frame.paint_plan.primitives.iter().find_map(|primitive| {
                    matches!(
                        primitive,
                        PaintPrimitive::StrokePolygon(stroke)
                            if stroke.widget_id == fill.widget_id
                                && stroke.color == super::TRACK_CARD_SELECTED_CORAL
                                && stroke.points.as_ref() == fill.points.as_ref()
                    )
                    .then_some(fill.widget_id)
                })
            })
            .expect("selected library card should paint a coral polygon outline");
        let unselected_card_widget_id = card_fills
            .iter()
            .find_map(|fill| {
                frame.paint_plan.primitives.iter().find_map(|primitive| {
                    matches!(
                        primitive,
                            PaintPrimitive::StrokePolygon(stroke)
                                if stroke.widget_id == fill.widget_id
                                && stroke.color == theme.grid_strong
                                && stroke.points.as_ref() == fill.points.as_ref()
                    )
                    .then_some(fill.widget_id)
                })
            })
            .expect("unselected library card should paint a neutral polygon outline");

        let selected_card_points = card_fills
            .iter()
            .find(|fill| fill.widget_id == selected_card_widget_id)
            .expect("selected card base should be paired with its outline")
            .points
            .clone();
        let unselected_card_points = card_fills
            .iter()
            .find(|fill| fill.widget_id == unselected_card_widget_id)
            .expect("unselected card base should be paired with its outline")
            .points
            .clone();
        let assert_bottom_right_chamfer = |points: &[Point]| {
            assert_eq!(points.len(), 5);
            let min_x = points
                .iter()
                .map(|point| point.x)
                .fold(f32::INFINITY, f32::min);
            let min_y = points
                .iter()
                .map(|point| point.y)
                .fold(f32::INFINITY, f32::min);
            let max_x = points
                .iter()
                .map(|point| point.x)
                .fold(f32::NEG_INFINITY, f32::max);
            let max_y = points
                .iter()
                .map(|point| point.y)
                .fold(f32::NEG_INFINITY, f32::max);
            assert!((points[0].x - min_x).abs() < 0.01);
            assert!((points[0].y - min_y).abs() < 0.01);
            assert!((points[1].x - max_x).abs() < 0.01);
            assert!((points[1].y - min_y).abs() < 0.01);
            assert!((points[2].x - max_x).abs() < 0.01);
            assert!((points[2].y - max_y).abs() > 0.01);
            assert!((points[3].x - max_x).abs() > 0.01);
            assert!((points[3].y - max_y).abs() < 0.01);
            assert!((points[4].x - min_x).abs() < 0.01);
            assert!((points[4].y - max_y).abs() < 0.01);
            assert!((points[1].x - points[3].x) > 0.0);
            assert!((points[4].y - points[2].y) > 0.0);
            assert!(((points[1].x - points[3].x) - (points[4].y - points[2].y)).abs() < 0.01);
        };
        assert_bottom_right_chamfer(&selected_card_points);
        assert_bottom_right_chamfer(&unselected_card_points);
        assert!(
            ((selected_card_points[1].x - selected_card_points[3].x)
                - (unselected_card_points[1].x - unselected_card_points[3].x))
                .abs()
                < 0.01,
            "selected and unselected cards should share one chamfer size"
        );

        let selected_card_stroke = frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::StrokePolygon(stroke)
                    if stroke.widget_id == selected_card_widget_id =>
                {
                    Some(stroke)
                }
                _ => None,
            })
            .expect("selected library card should have one polygon outline");
        assert_eq!(selected_card_stroke.color, super::TRACK_CARD_SELECTED_CORAL);
        assert_eq!(selected_card_stroke.width, super::TRACK_CARD_OUTLINE_WIDTH);
        let unselected_card_stroke = frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::StrokePolygon(stroke)
                    if stroke.widget_id == unselected_card_widget_id =>
                {
                    Some(stroke)
                }
                _ => None,
            })
            .expect("unselected library card should have one polygon outline");
        assert_eq!(unselected_card_stroke.color, theme.grid_strong);
        assert_eq!(
            unselected_card_stroke.width,
            super::TRACK_CARD_OUTLINE_WIDTH
        );
        assert!(
            !frame
                .paint_plan
                .fill_polygons_for_widget(selected_card_widget_id)
                .any(|fill| fill.color == super::TRACK_CARD_SELECTED_CORAL)
        );

        let selected_rails = frame
            .paint_plan
            .fill_rects_for_widget(selected_card_widget_id)
            .filter(|fill| fill.color == super::TRACK_CARD_SELECTED_CORAL)
            .collect::<Vec<_>>();
        assert_eq!(
            selected_rails.len(),
            1,
            "selected card should paint one coral rail"
        );
        assert_eq!(selected_rails[0].rect.width(), super::TRACK_CARD_RAIL_WIDTH);
        assert_eq!(
            selected_rails[0].rect.height(),
            selected_card_points
                .iter()
                .map(|point| point.y)
                .fold(f32::NEG_INFINITY, f32::max)
                - selected_card_points
                    .iter()
                    .map(|point| point.y)
                    .fold(f32::INFINITY, f32::min)
                - (super::TRACK_CARD_RAIL_VERTICAL_INSET * 2.0)
        );

        let unselected_rails = frame
            .paint_plan
            .fill_rects_for_widget(unselected_card_widget_id)
            .filter(|fill| fill.color == theme.grid_strong)
            .collect::<Vec<_>>();
        assert_eq!(
            unselected_rails.len(),
            1,
            "unselected card should paint one neutral strong-grid rail"
        );
        assert_eq!(
            unselected_rails[0].rect.width(),
            super::TRACK_CARD_RAIL_WIDTH
        );

        let selected_min_x = selected_card_points
            .iter()
            .map(|point| point.x)
            .fold(f32::INFINITY, f32::min);
        let selected_min_y = selected_card_points
            .iter()
            .map(|point| point.y)
            .fold(f32::INFINITY, f32::min);
        let selected_max_y = selected_card_points
            .iter()
            .map(|point| point.y)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            frame.paint_plan.fill_rects().any(|fill| {
                fill.color == super::TRACK_CARD_SELECTED_CORAL
                    && (fill.rect.width() - super::TRACK_CARD_RAIL_WIDTH).abs() < 0.01
                    && fill.rect.min.x >= selected_min_x
                    && fill.rect.min.x
                        <= selected_min_x
                            + super::TRACK_CARD_RAIL_EDGE_INSET
                            + super::TRACK_CARD_RAIL_WIDTH
                    && fill.rect.min.y >= selected_min_y
                    && fill.rect.max.y <= selected_max_y
            }),
            "the selected card should paint one leading coral rail"
        );
        assert!(!frame.paint_plan.primitives.iter().any(|primitive| {
            primitive.widget_id() == Some(unselected_card_widget_id)
                && matches!(
                    primitive,
                    PaintPrimitive::FillRect(fill)
                        if fill.color == super::TRACK_CARD_SELECTED_CORAL
                )
                || primitive.widget_id() == Some(unselected_card_widget_id)
                    && matches!(
                        primitive,
                        PaintPrimitive::FillPolygon(fill)
                            if fill.color == super::TRACK_CARD_SELECTED_CORAL
                    )
                || primitive.widget_id() == Some(unselected_card_widget_id)
                    && matches!(
                        primitive,
                    PaintPrimitive::StrokePolygon(stroke)
                            if stroke.color == super::TRACK_CARD_SELECTED_CORAL
                    )
        }));
        assert_eq!(
            frame
                .paint_plan
                .fill_rects_for_widget(selected_card_widget_id)
                .count(),
            1,
            "the interactive-row underlay must not duplicate the selected card rail"
        );
    }

    #[test]
    fn library_track_card_content_inset_grows_bounds_without_resizing_controls() {
        let track_id = String::from("card-inset-track");
        let mut state = AppState {
            busy: false,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(track_id.clone());
        state.library.tracks = vec![audition_track(&track_id)];
        let frame = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(800.0, 600.0));
        let theme = ThemeTokens::default();
        let card = frame
            .paint_plan
            .fill_polygons()
            .find(|fill| {
                fill.color == theme.bg_primary
                    && fill.points.len() == 5
                    && fill
                        .points
                        .iter()
                        .map(|point| point.x)
                        .fold(f32::NEG_INFINITY, f32::max)
                        - fill
                            .points
                            .iter()
                            .map(|point| point.x)
                            .fold(f32::INFINITY, f32::min)
                        > 200.0
                    && fill
                        .points
                        .iter()
                        .map(|point| point.y)
                        .fold(f32::NEG_INFINITY, f32::max)
                        - fill
                            .points
                            .iter()
                            .map(|point| point.y)
                            .fold(f32::INFINITY, f32::min)
                        > 60.0
            })
            .expect("the track row should paint one card base");
        let card_bounds = Rect::from_min_max(
            Point::new(
                card.points
                    .iter()
                    .map(|point| point.x)
                    .fold(f32::INFINITY, f32::min),
                card.points
                    .iter()
                    .map(|point| point.y)
                    .fold(f32::INFINITY, f32::min),
            ),
            Point::new(
                card.points
                    .iter()
                    .map(|point| point.x)
                    .fold(f32::NEG_INFINITY, f32::max),
                card.points
                    .iter()
                    .map(|point| point.y)
                    .fold(f32::NEG_INFINITY, f32::max),
            ),
        );
        let expected_height = 26.0
            + (ui::dropdown_trigger_height() * 2.0)
            + (super::TRACK_CARD_CONTENT_SPACING * 3.0)
            + (super::TRACK_CARD_CONTENT_INSET * 2.0);
        assert!(
            (card_bounds.height() - expected_height).abs() < 0.01,
            "card height should include the larger content inset: bounds={card_bounds:?}"
        );

        let control_bounds = |label: &str| {
            let run = frame
                .paint_plan
                .text_runs()
                .find(|run| {
                    run.text.as_str() == label
                        && run.rect.min.x < super::LIBRARY_WIDTH
                        && run.rect.min.y >= card_bounds.min.y
                        && run.rect.max.y <= card_bounds.max.y
                })
                .unwrap_or_else(|| panic!("missing {label:?} control label"));
            frame
                .layout
                .rects
                .get(&run.widget_id)
                .copied()
                .unwrap_or_else(|| panic!("missing {label:?} control bounds"))
        };
        let title_bounds = control_bounds(&track_id);
        let stage_bounds = control_bounds("Production / arrangement");
        let status_bounds = control_bounds("Inbox");
        for (label, bounds) in [("title", title_bounds), ("stage", stage_bounds)] {
            assert!(
                (bounds.min.x - card_bounds.min.x - super::TRACK_CARD_CONTENT_INSET).abs() < 0.01,
                "{label} should start at the card content inset: card={card_bounds:?}, control={bounds:?}"
            );
        }
        assert!(
            (status_bounds.min.x
                - card_bounds.min.x
                - super::TRACK_CARD_CONTENT_INSET
                - super::STATUS_RAIL_WIDTH
                - super::STATUS_RAIL_GAP)
                .abs()
                < 0.01,
            "status trigger should follow its inset rail and gap: card={card_bounds:?}, control={status_bounds:?}"
        );
        assert_eq!(stage_bounds.height(), 24.0);
        assert_eq!(status_bounds.height(), ui::dropdown_trigger_height());
        let status_rail = frame
            .paint_plan
            .fill_rects()
            .find(|fill| {
                fill.rect.width() == super::STATUS_RAIL_WIDTH
                    && fill.rect.height() == ui::dropdown_trigger_height()
                    && fill.rect.min.x >= card_bounds.min.x
                    && fill.rect.max.x <= card_bounds.max.x
            })
            .expect("the status dropdown should retain its compact semantic rail");
        assert!(
            (status_rail.rect.min.x - card_bounds.min.x - super::TRACK_CARD_CONTENT_INSET).abs()
                < 0.01
        );
    }

    #[test]
    fn library_cards_are_inset_from_scrollbar_and_separated() {
        let mut state = AppState {
            busy: false,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(String::from("track-0"));
        state.library.tracks = (0..12)
            .map(|index| audition_track(&format!("track-{index}")))
            .collect();

        let frame = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(800.0, 600.0));
        let theme = ThemeTokens::default();
        let mut card_rects = frame
            .paint_plan
            .fill_polygons()
            .filter(|fill| {
                fill.color == theme.bg_primary
                    && fill.points.len() == 5
                    && fill
                        .points
                        .iter()
                        .map(|point| point.x)
                        .fold(f32::NEG_INFINITY, f32::max)
                        - fill
                            .points
                            .iter()
                            .map(|point| point.x)
                            .fold(f32::INFINITY, f32::min)
                        > 200.0
                    && fill
                        .points
                        .iter()
                        .map(|point| point.y)
                        .fold(f32::NEG_INFINITY, f32::max)
                        - fill
                            .points
                            .iter()
                            .map(|point| point.y)
                            .fold(f32::INFINITY, f32::min)
                        > 60.0
            })
            .map(|fill| {
                Rect::from_min_max(
                    Point::new(
                        fill.points
                            .iter()
                            .map(|point| point.x)
                            .fold(f32::INFINITY, f32::min),
                        fill.points
                            .iter()
                            .map(|point| point.y)
                            .fold(f32::INFINITY, f32::min),
                    ),
                    Point::new(
                        fill.points
                            .iter()
                            .map(|point| point.x)
                            .fold(f32::NEG_INFINITY, f32::max),
                        fill.points
                            .iter()
                            .map(|point| point.y)
                            .fold(f32::NEG_INFINITY, f32::max),
                    ),
                )
            })
            .collect::<Vec<_>>();
        card_rects.sort_by(|left, right| left.min.y.total_cmp(&right.min.y));
        assert!(
            card_rects.len() >= 2,
            "the frame should expose multiple library cards"
        );

        let first = card_rects[0];
        let library_origin_x = 18.0;
        assert!(
            (first.min.x - (library_origin_x + 10.0 + super::LIBRARY_LIST_INSET)).abs() < 0.01,
            "unexpected library card left inset: first={first:?} expected={}",
            library_origin_x + 10.0 + super::LIBRARY_LIST_INSET
        );
        assert!(
            (first.max.x
                - (library_origin_x + super::LIBRARY_WIDTH - 10.0 - super::LIBRARY_LIST_INSET))
                .abs()
                < 0.01
        );
        for pair in card_rects.windows(2) {
            assert!(
                pair[1].min.y - pair[0].max.y >= super::LIBRARY_CARD_SPACING - 0.01,
                "library cards should retain a visible vertical gap: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }

        let scrollbar = frame
            .paint_plan
            .fill_rects()
            .find(|fill| {
                (fill.rect.width() - 3.0).abs() < 0.01
                    && fill.rect.min.x > super::LIBRARY_WIDTH - 6.0
            })
            .expect("overflowing library content should paint a virtual scrollbar thumb");
        assert!(
            first.max.x < scrollbar.rect.min.x,
            "card right border must remain left of the scrollbar: card={first:?}, scrollbar={:?}",
            scrollbar.rect
        );
    }

    #[test]
    fn status_dropdown_uses_neutral_body_and_compact_semantic_rail() {
        let theme = ThemeTokens::default();
        let statuses = [
            (TrackStatus::Inbox, super::TRACK_CARD_SELECTED_CORAL),
            (TrackStatus::Refine, theme.accent_warning),
            (TrackStatus::Release, theme.highlight_cyan),
            (TrackStatus::Archive, theme.text_muted),
            (TrackStatus::Maybe, theme.accent_danger),
        ];

        for (status, expected_rail_color) in statuses {
            let mut track = audition_track("status-rail");
            track.status = status;
            let frame = ui::scene(status_dropdown_for_host(
                &track,
                false,
                false,
                StatusMenuHost::Library,
            ))
            .into_view()
            .view_frame_at_size_with_default_theme(Vector2::new(240.0, 80.0));
            let label = frame
                .paint_plan
                .text_runs()
                .find(|run| run.text.as_str() == status.label())
                .expect("status trigger should paint its selected label");
            let body = frame
                .paint_plan
                .fill_polygons()
                .find(|fill| {
                    fill.widget_id == label.widget_id && fill.color == theme.surface_overlay
                })
                .expect("status trigger should use a neutral dark body");
            let body_min_x = body
                .points
                .iter()
                .map(|point| point.x)
                .fold(f32::INFINITY, f32::min);
            let rail = frame
                .paint_plan
                .fill_rects()
                .find(|fill| {
                    fill.color == expected_rail_color
                        && (fill.rect.width() - super::STATUS_RAIL_WIDTH).abs() < 0.01
                })
                .expect("status trigger should paint one semantic color rail");
            assert!(rail.rect.max.x <= body_min_x);
            assert_eq!(rail.rect.height(), ui::dropdown_trigger_height());
            assert!(!frame.paint_plan.fill_polygons().any(|fill| {
                fill.widget_id == label.widget_id
                    && [
                        super::TRACK_CARD_SELECTED_CORAL,
                        theme.accent_warning,
                        theme.highlight_cyan,
                        theme.accent_danger,
                    ]
                    .contains(&fill.color)
            }));
        }
    }

    #[test]
    fn status_filter_picker_and_audition_rows_paint_semantic_rails() {
        let theme = ThemeTokens::default();
        let statuses = [
            (Some(TrackStatus::Inbox), super::TRACK_CARD_SELECTED_CORAL),
            (Some(TrackStatus::Refine), theme.accent_warning),
            (Some(TrackStatus::Release), theme.highlight_cyan),
            (Some(TrackStatus::Archive), theme.text_muted),
            (Some(TrackStatus::Maybe), theme.accent_danger),
        ];
        let picker = ui::scene(status_filter_dropdown(
            Some(TrackStatus::Refine),
            "review",
            review_status_filter_message,
            true,
        ))
        .into_view()
        .view_frame_at_size_with_default_theme(Vector2::new(260.0, 220.0));
        for (status, expected_color) in statuses {
            assert!(picker.paint_plan.fill_rects().any(|fill| {
                fill.color == expected_color
                    && (fill.rect.width() - super::STATUS_RAIL_WIDTH).abs() < 0.01
                    && fill.rect.height() > 20.0
            }));
            let label = status.expect("filter option status").label();
            assert!(
                picker
                    .paint_plan
                    .text_label_strings()
                    .iter()
                    .any(|text| text == label)
            );
        }
        assert!(
            picker
                .paint_plan
                .text_label_strings()
                .iter()
                .any(|text| text == "All")
        );
        assert!(picker.paint_plan.fill_rects().any(|fill| {
            fill.color == theme.grid_strong
                && (fill.rect.width() - super::STATUS_RAIL_WIDTH).abs() < 0.01
        }));

        let state = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Audition,
            ..AppState::default()
        };
        let audition = ui::scene(audition_panel(&state))
            .into_view()
            .view_frame_at_size_with_default_theme(Vector2::new(420.0, 720.0));
        for (_, expected_color) in statuses {
            assert!(audition.paint_plan.fill_rects().any(|fill| {
                fill.color == expected_color
                    && (fill.rect.width() - super::STATUS_RAIL_WIDTH).abs() < 0.01
                    && fill.rect.height() >= 24.0
            }));
        }
    }

    #[test]
    fn successive_native_file_drop_events_queue_every_path_while_importing() {
        let mut state = AppState {
            busy: true,
            ..AppState::default()
        };
        let mut context = ui::UiUpdateContext::default();
        let first = PathBuf::from("/external/first.wav");
        let second = PathBuf::from("/external/second.wav");

        update(
            &mut state,
            Message::FileDropped(ui::NativeFileDrop::dropped(first.clone(), None, None)),
            &mut context,
        );
        update(
            &mut state,
            Message::FileDropped(ui::NativeFileDrop::dropped(second.clone(), None, None)),
            &mut context,
        );

        assert_eq!(state.pending_import_paths, vec![first, second]);
        assert_eq!(
            state.import_batch,
            Some(ImportBatchProgress {
                total: 2,
                completed: 0,
                failed: 0,
            })
        );
    }

    #[test]
    fn import_completion_advances_progress_continues_after_failure_and_clears() {
        let mut state = AppState {
            busy: true,
            import_batch: Some(ImportBatchProgress {
                total: 3,
                completed: 0,
                failed: 0,
            }),
            pending_import_paths: vec![
                PathBuf::from("/external/second.wav"),
                PathBuf::from("/external/third.wav"),
            ],
            ..AppState::default()
        };
        let mut context = ui::UiUpdateContext::default();

        update(
            &mut state,
            Message::ImportCompleted(Ok(Library::default())),
            &mut context,
        );
        assert_eq!(
            state.import_batch,
            Some(ImportBatchProgress {
                total: 3,
                completed: 1,
                failed: 0,
            })
        );
        assert_eq!(
            state.pending_import_paths,
            vec![PathBuf::from("/external/third.wav")]
        );
        assert!(state.busy);

        update(
            &mut state,
            Message::ImportCompleted(Err(String::from("second failed"))),
            &mut context,
        );
        assert_eq!(
            state.import_batch,
            Some(ImportBatchProgress {
                total: 3,
                completed: 2,
                failed: 1,
            })
        );
        assert!(state.pending_import_paths.is_empty());
        assert!(state.busy);

        update(
            &mut state,
            Message::ImportCompleted(Ok(Library::default())),
            &mut context,
        );
        assert!(state.import_batch.is_none());
        assert_eq!(state.status, "Imported 2 of 3 files; 1 failed.");
        assert!(!state.busy);
    }

    #[test]
    fn multi_file_import_status_projects_counter_and_determinate_bar() {
        let state = AppState {
            busy: true,
            import_batch: Some(ImportBatchProgress {
                total: 3,
                completed: 1,
                failed: 1,
            }),
            ..AppState::default()
        };
        let frame = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 720.0));
        let labels = frame.paint_plan.text_label_strings();

        assert!(
            labels
                .iter()
                .any(|label| { label == "Importing 2 of 3 · 2 remaining · 1 failed" })
        );

        let fills = frame.paint_plan.fill_rects().collect::<Vec<_>>();
        assert!(fills.windows(2).any(|pair| {
            let track = pair[0];
            let fill = pair[1];
            track.widget_id == fill.widget_id
                && (track.rect.min.y - fill.rect.min.y).abs() < f32::EPSILON
                && (track.rect.height() - fill.rect.height()).abs() < f32::EPSILON
                && (fill.rect.width() - track.rect.width() / 3.0).abs() < 0.01
        }));
    }

    #[test]
    fn audition_library_reconciliation_preserves_order_and_progress_cursor() {
        let track = |id: &str| Track {
            id: String::from(id),
            title: String::from(id),
            original_name: format!("{id}.wav"),
            path: PathBuf::from(format!("/external/{id}.wav")),
            reference_path: None,
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
            status: TrackStatus::Inbox,
            notes: Vec::new(),
        };
        let mut state = AppState {
            busy: false,
            workspace_mode: WorkspaceMode::Audition,
            audition_queue: vec![String::from("a"), String::from("b")],
            audition_queue_index: 1,
            ..AppState::default()
        };
        state.library.selected_track_id = Some(String::from("b"));
        state.library.tracks = vec![track("a"), track("b"), track("c")];

        reconcile_audition_queue(&mut state);
        assert_eq!(
            state.audition_queue,
            vec![String::from("a"), String::from("b"), String::from("c")]
        );
        assert_eq!(state.audition_queue_index, 1);

        state.library.tracks.retain(|track| track.id != "b");
        reconcile_audition_queue(&mut state);
        assert_eq!(
            state.audition_queue,
            vec![String::from("a"), String::from("c")]
        );
        assert_eq!(state.audition_queue_index, 1);
    }
}

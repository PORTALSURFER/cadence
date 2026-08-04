mod audio;
mod chrome;
mod storage;
mod transport;
mod waveform;

use radiant::{
    gui::types::{Point, Vector2},
    prelude as ui,
    runtime::{FileDialogRequest, NativeRunOptions, PlatformResponse, PlatformResult},
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq)]
enum Message {
    ImportPressed,
    FilePicked(PlatformResult),
    FileDropped(ui::NativeFileDrop),
    LibraryLoaded(Result<storage::Library, String>),
    ImportCompleted(Result<storage::Library, String>),
    LibrarySaved(Result<(), String>),
    DecodeCompleted {
        track_id: String,
        generation: u64,
        result: Result<audio::WaveformData, String>,
    },
    SelectTrack(String),
    ToggleWorkspace,
    ToggleFavorite(String),
    ToggleStageMenu(String),
    ToggleStageMenuAt {
        track_id: String,
        position: Point,
    },
    SetStage {
        track_id: String,
        stage: storage::TrackStage,
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
    Frame,
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
    DraftNoteChanged(String),
    SaveDraftNote,
    CancelDraftNote,
    EditNote(String),
    ToggleNoteDone(String),
    DeleteNote(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceMode {
    Review,
    Planner,
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
    review_cursor_millis: u64,
    playhead_drag_active: bool,
    transport: transport::AudioTransport,
    transport_generation: u64,
    transport_position_millis: u64,
    transport_playing: bool,
    transport_polling: bool,
    transport_waiting_token: Option<u64>,
    draft_note: Option<NoteDraft>,
    stage_menu_track_id: Option<String>,
    stage_menu_anchor: Option<Point>,
    remove_confirmation_track_id: Option<String>,
    planner_drag_source_track_id: Option<String>,
    planner_drag_target_stage: Option<storage::TrackStage>,
    planner_drag_pointer: Option<Point>,
}

#[derive(Clone, Debug)]
struct NoteDraft {
    note_id: Option<String>,
    time_millis: u64,
    body: String,
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
            review_cursor_millis: 0,
            playhead_drag_active: false,
            transport: transport::AudioTransport::spawn(),
            transport_generation: 0,
            transport_position_millis: 0,
            transport_playing: false,
            transport_polling: false,
            transport_waiting_token: None,
            draft_note: None,
            stage_menu_track_id: None,
            stage_menu_anchor: None,
            remove_confirmation_track_id: None,
            planner_drag_source_track_id: None,
            planner_drag_target_stage: None,
            planner_drag_pointer: None,
        }
    }
}

fn playback_shortcut(state: &AppState, press: ui::KeyPress) -> ui::ShortcutResolution<Message> {
    if state.draft_note.is_none() && press == ui::KeyPress::new(ui::KeyCode::Space) {
        ui::ShortcutResolution::action(Message::TogglePlayback)
    } else {
        ui::ShortcutResolution::unhandled()
    }
}

fn native_launch_options() -> NativeRunOptions {
    let mut options = NativeRunOptions::default();
    options.window.behavior.maximized = true;
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
                || state.playhead_drag_active
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

fn schedule_waveform_decode(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    track_id: String,
    path: PathBuf,
) {
    state.waveform_busy = true;
    state.waveform_generation = state.waveform_generation.wrapping_add(1);
    let generation = state.waveform_generation;
    state.waveform = None;
    state.waveform_track_id = None;
    state.status = format!("Analyzing waveform for {}…", path.display());
    let completion_track_id = track_id;
    context
        .business()
        .blocking_io("cadence-decode-waveform")
        .run(
            move |_| audio::decode_waveform(&path),
            move |result| Message::DecodeCompleted {
                track_id: completion_track_id.clone(),
                generation,
                result,
            },
        );
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
        state.waveform_generation = state.waveform_generation.wrapping_add(1);
        state.waveform_busy = false;
        state.waveform = None;
        state.waveform_track_id = None;
        context.request_repaint();
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
                    state.review_cursor_millis = 0;
                    state.draft_note = None;
                    close_stage_menu(state);
                    state.remove_confirmation_track_id = None;
                    reset_transport(state);
                    schedule_selected_waveform_decode(state, context);
                }
                Err(error) => {
                    state.status = error;
                }
            }
            context.request_repaint();
        }
        Message::ImportCompleted(result) => {
            state.busy = false;
            clear_planner_drag(state);
            match result {
                Ok(library) => {
                    state.status = format!(
                        "{} local track{} — all changes saved.",
                        library.tracks.len(),
                        plural(library.tracks.len())
                    );
                    state.library = library;
                    state.review_cursor_millis = 0;
                    state.draft_note = None;
                    close_stage_menu(state);
                    state.remove_confirmation_track_id = None;
                    reset_transport(state);
                    schedule_selected_waveform_decode(state, context);
                }
                Err(error) => {
                    state.status = error;
                }
            }
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
            state.waveform_busy = false;
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
                            Err(error) => state.status = error,
                        }
                    }
                }
                Err(error) => {
                    state.waveform = None;
                    state.waveform_track_id = None;
                    state.status = format!("Waveform unavailable: {error}");
                }
            }
            context.request_repaint();
        }
        Message::Frame => {
            if state.planner_drag_source_track_id.is_some() {
                state.planner_drag_pointer = context.current_pointer_position();
            }
            let snapshot = state.transport.snapshot();
            if snapshot.generation != state.transport_generation {
                context.request_repaint();
                return;
            }
            if let Some(error) = state.transport.take_error(state.transport_generation) {
                state.playhead_drag_active = false;
                state.transport_playing = false;
                state.transport_polling = false;
                state.transport_waiting_token = None;
                state.status = error;
            } else if state
                .transport_waiting_token
                .is_some_and(|token| !transport_command_is_confirmed(snapshot, token))
            {
                context.request_repaint();
                return;
            } else {
                state.transport_waiting_token = None;
                apply_transport_snapshot(state, snapshot);
            }
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
            }
            context.request_repaint();
        }
        Message::SelectTrack(id) => {
            if !state.busy && state.library.tracks.iter().any(|track| track.id == id) {
                state.workspace_mode = WorkspaceMode::Review;
                state.library.selected_track_id = Some(id);
                close_stage_menu(state);
                state.remove_confirmation_track_id = None;
                clear_planner_drag(state);
                state.waveform = None;
                state.waveform_track_id = None;
                state.waveform_busy = false;
                state.review_cursor_millis = 0;
                state.draft_note = None;
                reset_transport(state);
                schedule_library_save(state, context);
                schedule_selected_waveform_decode(state, context);
            }
        }
        Message::ToggleWorkspace => {
            state.workspace_mode = match state.workspace_mode {
                WorkspaceMode::Review => WorkspaceMode::Planner,
                WorkspaceMode::Planner => WorkspaceMode::Review,
            };
            close_stage_menu(state);
            state.remove_confirmation_track_id = None;
            clear_planner_drag(state);
            context.request_repaint();
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
                clear_planner_drag(state);
                if changed {
                    state.status = format!("Stage set to {}.", stage.label());
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
            clear_planner_drag(state);
            if selected {
                state.draft_note = None;
                state.library.selected_track_id =
                    storage::selection_after_removal(&state.library, removed.0);
                state.waveform = None;
                state.waveform_track_id = None;
                state.waveform_busy = false;
                reset_transport(state);
            }
            state.status = format!(
                "Removed {} from the library. The source audio file remains on disk.",
                removed.1.title
            );
            schedule_library_save(state, context);
            if selected {
                schedule_selected_waveform_decode(state, context);
            }
            context.request_repaint();
        }
        Message::CancelRemoveTrack => {
            state.remove_confirmation_track_id = None;
            state.status = String::from("Track kept in the library.");
            context.request_repaint();
        }
        Message::TogglePlayback => {
            if state.busy {
                return;
            }
            let Some(_waveform) = state
                .waveform
                .as_ref()
                .filter(|_| selected_track(state).is_some())
            else {
                state.status = String::from("Audio analysis is still pending.");
                context.request_repaint();
                return;
            };
            let result = if state.transport_playing {
                state.transport.pause(state.transport_generation)
            } else {
                state.transport.play(state.transport_generation)
            };
            match result {
                Ok(token) => {
                    begin_transport_polling(state, token);
                    state.status = if state.transport_playing {
                        String::from("Pausing playback…")
                    } else {
                        String::from("Preparing playback…")
                    };
                }
                Err(error) => state.status = error,
            }
            context.request_repaint();
        }
        Message::WaveformClicked { ratio, lower } => {
            if state.busy {
                return;
            }
            let Some(waveform) = state.waveform.as_ref() else {
                return;
            };
            let time_millis = waveform::millis_for_ratio(ratio, waveform.duration_millis);
            if lower {
                state.review_cursor_millis = time_millis;
                state.draft_note = Some(NoteDraft {
                    note_id: None,
                    time_millis,
                    body: String::new(),
                });
                state.status = format!(
                    "Comment at {} — type a note below.",
                    format_timestamp(time_millis)
                );
            } else {
                state.draft_note = None;
                match state.transport.seek(
                    state.transport_generation,
                    time_millis,
                    waveform.duration_millis,
                    state.transport_playing,
                ) {
                    Ok(token) => {
                        begin_transport_polling(state, token);
                        state.status =
                            format!("Review cursor at {}.", format_timestamp(time_millis));
                    }
                    Err(error) => state.status = error,
                }
            }
            context.request_repaint();
        }
        Message::WaveformPlayheadDragStarted { ratio } => {
            if state.busy || state.waveform.is_none() {
                return;
            }
            state.playhead_drag_active = true;
            state.draft_note = None;
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
        Message::DraftNoteChanged(body) => {
            if let Some(draft) = state.draft_note.as_mut() {
                draft.body = body;
                context.request_repaint();
            }
        }
        Message::SaveDraftNote => save_draft_note(state, context),
        Message::CancelDraftNote => {
            state.draft_note = None;
            state.status = String::from("Comment canceled.");
            context.request_repaint();
        }
        Message::EditNote(id) => {
            if state.busy {
                return;
            }
            let note = selected_track(state)
                .and_then(|track| track.notes.iter().find(|note| note.id == id))
                .cloned();
            if let Some(note) = note {
                state.review_cursor_millis = note.time_millis;
                state.draft_note = Some(NoteDraft {
                    note_id: Some(note.id),
                    time_millis: note.time_millis,
                    body: note.body,
                });
                state.status =
                    format!("Editing comment at {}.", format_timestamp(note.time_millis));
                context.request_repaint();
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
            let removed = selected_track_mut(state).and_then(|track| {
                track
                    .notes
                    .iter()
                    .position(|note| note.id == id)
                    .map(|index| track.notes.remove(index))
            });
            if removed.is_some() {
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
    }
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
    context.pick_file(
        FileDialogRequest::new().title("Import audio track").filter(
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
        ),
        Message::FilePicked,
    );
}

fn schedule_import(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    path: PathBuf,
) {
    if state.busy {
        return;
    }
    if state.save_in_flight {
        state.status = String::from("Saving the library — try importing again in a moment.");
        context.request_repaint();
        return;
    }
    state.busy = true;
    state.status = format!("Importing {}…", path.display());
    let library = state.library.clone();
    context.business().blocking_io("cadence-import-track").run(
        move |_| storage::import_into_library(library, path),
        Message::ImportCompleted,
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
    state.transport_position_millis = 0;
    state.review_cursor_millis = 0;
    state.playhead_drag_active = false;
    state.transport_playing = false;
    state.transport_polling = false;
    state.transport_waiting_token = None;
    let _ = state.transport.unload(state.transport_generation);
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

fn seek_review_position(
    state: &mut AppState,
    context: &mut ui::UiUpdateContext<Message>,
    ratio: f32,
    resume: bool,
) {
    let Some(duration_millis) = state
        .waveform
        .as_ref()
        .map(|waveform| waveform.duration_millis)
    else {
        return;
    };
    let time_millis = waveform::millis_for_ratio(ratio, duration_millis);
    state.review_cursor_millis = time_millis;
    state.transport_position_millis = time_millis;
    if resume {
        match state.transport.seek(
            state.transport_generation,
            time_millis,
            duration_millis,
            true,
        ) {
            Ok(token) => {
                begin_transport_polling(state, token);
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

fn clear_planner_drag(state: &mut AppState) {
    state.planner_drag_source_track_id = None;
    state.planner_drag_target_stage = None;
    state.planner_drag_pointer = None;
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

fn project_surface(state: &AppState) -> ui::View<Message> {
    let workspace = match state.workspace_mode {
        WorkspaceMode::Review => ui::row([
            library_panel(state).width(310.0).fill_height(),
            review_panel(state).fill(),
        ])
        .spacing(14.0)
        .fill(),
        WorkspaceMode::Planner => planner_panel(state).fill(),
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
    let content = ui::column([
        ui::row([
            chrome::text("PORTALSURFER / CADENCE")
                .height(24.0)
                .fill_width(),
            ui::button(if state.workspace_mode == WorkspaceMode::Planner {
                "Review desk"
            } else {
                "Planner"
            })
            .primary()
            .message(Message::ToggleWorkspace)
            .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY))
            .height(28.0),
            ui::badge("NATIVE / RADIANT").subtle().passive(),
        ])
        .fill_width()
        .spacing(12.0),
        workspace
            .accepts_native_file_drop()
            .on_native_file_drop(Message::FileDropped),
        chrome::muted_text(state.status.clone())
            .height(24.0)
            .fill_width(),
    ])
    .padding(18.0)
    .spacing(12.0)
    .fill();

    ui::scene(
        ui::stack([chrome::background().fill(), content])
            .fill()
            .overlays(
                ui::overlays()
                    .popover_opt(stage_menu)
                    .drag_preview_opt(drag_preview),
            ),
    )
    .into_view()
}

fn planner_panel(state: &AppState) -> ui::View<Message> {
    let stages = [
        storage::TrackStage::SoundDesign,
        storage::TrackStage::Production,
        storage::TrackStage::Mixdown,
        storage::TrackStage::Mastering,
    ];
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
            tracks_in_stage(&state.library.tracks, stage),
            state.library.selected_track_id.as_deref(),
            state.stage_menu_track_id.as_deref(),
            drag_active,
            drag_source_stage,
            drag_target_stage,
        )
    });
    let track_count = state.library.tracks.len();
    ui::column([
        ui::row([
            ui::column([
                chrome::muted_text("FINISHING BOARD")
                    .height(18.0)
                    .fill_width()
                    .subtle(),
                chrome::text("Move every track toward release.")
                    .height(30.0)
                    .fill_width(),
            ])
            .fill_width(),
            chrome::muted_text(format!(
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

fn planner_column(
    stage: storage::TrackStage,
    tracks: Vec<storage::Track>,
    selected_id: Option<&str>,
    stage_menu_track_id: Option<&str>,
    drag_active: bool,
    drag_source_stage: Option<storage::TrackStage>,
    drag_target_stage: Option<storage::TrackStage>,
) -> ui::View<Message> {
    let count = tracks.len();
    let candidate = drag_active && planner_drop_is_valid(drag_source_stage, stage);
    let current_target = drag_target_stage == Some(stage);
    let mut children = vec![
        ui::row([
            chrome::text(if current_target {
                "DROP HERE"
            } else {
                stage.label()
            })
            .height(24.0)
            .fill_width(),
            ui::badge(count.to_string()).subtle().passive(),
        ])
        .fill_width()
        .spacing(8.0),
    ];
    if tracks.is_empty() {
        children.push(
            ui::column([
                chrome::text("No tracks here yet.")
                    .height(24.0)
                    .fill_width(),
                chrome::muted_text("Choose this stage from a card when it is ready.")
                    .wrap()
                    .height(44.0)
                    .fill_width()
                    .subtle(),
            ])
            .padding(10.0)
            .spacing(6.0)
            .fill_width(),
        );
    } else {
        children.push(
            ui::list(tracks, move |track| {
                planner_card(track, selected_id, stage_menu_track_id)
            })
            .fill_height(),
        );
    }
    let content = ui::stack([
        chrome::panel().fill(),
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
) -> ui::View<Message> {
    let selected = selected_id == Some(track.id.as_str());
    let stage_menu_open = stage_menu_track_id == Some(track.id.as_str());
    let title_track_id = track.id.clone();
    let drag_track_id = track.id.clone();
    let favorite_id = track.id.clone();
    let open_comments = track.notes.iter().filter(|note| !note.done).count();
    let card_content = ui::column([
        ui::row([
            ui::button("↕")
                .subtle()
                .hover_chrome_only()
                .click_or_drag(
                    Message::PlannerCardHandleActivated(track.id.clone()),
                    move |message| Message::PlannerCardDrag {
                        track_id: drag_track_id.clone(),
                        message,
                    },
                )
                .key(format!("planner-card-drag-{}", track.id))
                .size(22.0, 22.0)
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
            ui::button(track.title.clone())
                .message(Message::SelectTrack(title_track_id))
                .fill_width()
                .height(28.0)
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
        ])
        .fill_width()
        .spacing(6.0),
        chrome::muted_text(track.original_name.clone())
            .truncate()
            .height(20.0)
            .fill_width()
            .subtle(),
        ui::row([
            chrome::muted_text(format!(
                "{} open comment{}",
                open_comments,
                plural(open_comments)
            ))
            .height(22.0)
            .fill_width()
            .subtle(),
            ui::button(if track.favorite { "★" } else { "☆" })
                .message(Message::ToggleFavorite(favorite_id))
                .subtle()
                .height(22.0)
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
        ])
        .fill_width()
        .spacing(6.0),
        stage_dropdown(&track, stage_menu_open),
    ])
    .padding(10.0)
    .spacing(5.0)
    .fill_width();
    let mut card = ui::stack([chrome::panel().fill(), card_content])
        .key(format!("planner-card-{}", track.id))
        .fill_width();
    if selected {
        card = card.primary();
    }
    card
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
        WorkspaceMode::Review => Point::new(310.0, 150.0),
        WorkspaceMode::Planner => Point::new(18.0 + STAGE_MENU_WIDTH * 0.5, 96.0),
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

fn stage_dropdown(track: &storage::Track, open: bool) -> ui::View<Message> {
    let stage_id = track.id.clone();
    ui::dropdown_trigger(track.stage.label(), open)
        .toggle_message(Message::ToggleStageMenu(stage_id.clone()))
        .build()
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
        .fill_width()
}

fn stage_menu_popover(track: &storage::Track, anchor: Point) -> ui::View<Message> {
    let options = stage_dropdown_options(track);
    ui::anchored_dropdown_menu_popover(
        ui::AnchoredPopoverAnchor::pointer(anchor),
        Vector2::new(STAGE_MENU_WIDTH, ui::dropdown_menu_height(options.len())),
        options,
    )
}

fn library_panel(state: &AppState) -> ui::View<Message> {
    let selected_id = state.library.selected_track_id.clone();
    let tracks = state.library.tracks.clone();
    let track_count = tracks.len();
    ui::column([
        chrome::muted_text("private studio")
            .height(18.0)
            .fill_width()
            .subtle(),
        chrome::text("cadence").height(34.0).fill_width(),
        ui::button("＋ Import track")
            .primary()
            .message(Message::ImportPressed)
            .fill_width()
            .height(36.0)
            .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
        chrome::muted_text(format!(
            "YOUR LIBRARY  ·  {} track{}",
            track_count,
            plural(track_count)
        ))
        .height(24.0)
        .fill_width()
        .subtle(),
        if tracks.is_empty() {
            ui::column([
                chrome::text("No tracks yet.").height(28.0).fill_width(),
                chrome::muted_text("Choose a file or drop audio onto the workspace.")
                    .wrap()
                    .height(48.0)
                    .fill_width()
                    .subtle(),
            ])
            .padding(12.0)
            .spacing(6.0)
            .fill_width()
        } else {
            ui::list(tracks.into_iter().enumerate(), move |(index, track)| {
                track_row(
                    index,
                    track,
                    selected_id.as_deref(),
                    state.stage_menu_track_id.as_deref(),
                    state.remove_confirmation_track_id.as_deref(),
                )
            })
            .fill_height()
        },
    ])
    .padding(14.0)
    .spacing(10.0)
    .fill_height()
}

fn track_row(
    _index: usize,
    track: storage::Track,
    selected_id: Option<&str>,
    stage_menu_track_id: Option<&str>,
    remove_confirmation_track_id: Option<&str>,
) -> ui::View<Message> {
    let selected = selected_id == Some(track.id.as_str());
    let id = track.id.clone();
    let favorite_id = track.id.clone();
    let stage_menu_open = stage_menu_track_id == Some(track.id.as_str());
    let remove_confirmation_open = remove_confirmation_track_id == Some(track.id.as_str());
    let remove_id = track.id.clone();
    let stage_control = stage_dropdown(&track, stage_menu_open);
    let removal_controls = if remove_confirmation_open {
        ui::row([
            ui::button("Confirm")
                .primary()
                .message(Message::ConfirmRemoveTrack(remove_id.clone()))
                .height(20.0)
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
            ui::button("Keep")
                .subtle()
                .message(Message::CancelRemoveTrack)
                .height(20.0)
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
        ])
        .spacing(4.0)
        .fill_width()
    } else {
        ui::row([
            ui::button(if track.favorite { "★" } else { "☆" })
                .message(Message::ToggleFavorite(favorite_id))
                .subtle()
                .height(20.0)
                .fill_width()
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
            ui::button("Remove")
                .message(Message::RequestRemoveTrack(remove_id))
                .subtle()
                .height(20.0)
                .fill_width()
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
        ])
        .spacing(4.0)
        .fill_width()
    };
    let mut row = ui::stack([
        chrome::panel().fill(),
        ui::row([
            ui::button(track.title)
                .message(Message::SelectTrack(id))
                .fill_width()
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
            ui::column([removal_controls, stage_control])
                .width(186.0)
                .spacing(4.0)
                .padding_x(6.0),
        ])
        .padding_y(6.0)
        .fill_width()
        .spacing(4.0),
    ])
    .key(format!("library-track-{}", track.id))
    .fill_width();
    if selected {
        row = row.primary();
    }
    row
}

fn review_panel(state: &AppState) -> ui::View<Message> {
    let Some(track) = selected_track(state).cloned() else {
        return ui::column([
            chrome::text("Your review desk").height(30.0).fill_width(),
            chrome::text("Import a track to begin reviewing.")
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
        .filter(|_| state.waveform_track_id.as_deref() == Some(track.id.as_str()))
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
    let cursor_ratio = state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(track.id.as_str()))
        .and_then(|waveform| {
            waveform::ratio_for_millis(state.review_cursor_millis, waveform.duration_millis)
        });
    let waveform_view = if let Some(waveform) = state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(track.id.as_str()))
    {
        waveform::view(
            Arc::new(waveform.clone()),
            cursor_ratio,
            note_ratios,
            |interaction| match interaction {
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
            },
        )
        .fill_width()
        .height(250.0)
    } else {
        ui::column([
            chrome::text(if state.waveform_busy {
                "Analyzing the real audio file…"
            } else {
                "Waveform unavailable for this file."
            })
            .height(28.0)
            .fill_width(),
            chrome::muted_text(
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
        .height(250.0)
    };
    let metadata = state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(track.id.as_str()))
        .map(|waveform| {
            format!(
                "{} Hz · {} channel{} · {}",
                waveform.sample_rate,
                waveform.channels,
                if waveform.channels == 1 { "" } else { "s" },
                format_duration(waveform.duration_millis),
            )
        })
        .unwrap_or_else(|| String::from("Audio analysis pending"));
    let duration_millis = state
        .waveform
        .as_ref()
        .filter(|_| state.waveform_track_id.as_deref() == Some(track.id.as_str()))
        .map_or(0, |waveform| waveform.duration_millis);
    let transport_controls = if duration_millis > 0 {
        ui::row([
            ui::button(if state.transport_playing {
                "❚❚ Pause"
            } else {
                "▶ Play"
            })
            .primary()
            .message(Message::TogglePlayback)
            .height(32.0)
            .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
            chrome::muted_text(format!(
                "{} / {}",
                format_timestamp(state.transport_position_millis.min(duration_millis)),
                format_duration(duration_millis)
            ))
            .height(32.0)
            .fill_width()
            .subtle(),
        ])
        .spacing(10.0)
        .fill_width()
    } else {
        chrome::muted_text("Playback controls will appear after audio analysis.")
            .height(28.0)
            .fill_width()
            .subtle()
    };

    let mut waveform_section = vec![
        ui::row([
            chrome::muted_text("01  WAVEFORM / TOP TO PLAY")
                .height(18.0)
                .fill_width()
                .subtle(),
            chrome::muted_text("02  BOTTOM TO COMMENT")
                .height(18.0)
                .subtle(),
        ])
        .fill_width()
        .spacing(10.0),
        chrome::muted_text(metadata)
            .height(22.0)
            .fill_width()
            .subtle(),
        transport_controls,
        waveform_view,
    ];
    if let Some(draft) = state.draft_note.as_ref() {
        waveform_section.push(note_editor(draft));
    }

    ui::column([
        ui::row([
            ui::column([
                chrome::muted_text("LOCAL TRACK")
                    .height(18.0)
                    .fill_width()
                    .subtle(),
                chrome::text(track.title.clone()).height(34.0).fill_width(),
                chrome::muted_text(track.original_name.clone())
                    .height(22.0)
                    .fill_width()
                    .subtle(),
            ])
            .fill_width(),
            ui::badge(track.stage.label()).primary().passive(),
        ])
        .fill_width()
        .spacing(12.0),
        ui::column(waveform_section)
            .padding(16.0)
            .spacing(10.0)
            .fill_width(),
        comments_panel(&track),
        ui::spacer().fill(),
    ])
    .padding(18.0)
    .spacing(14.0)
    .fill()
}

fn comments_panel(track: &storage::Track) -> ui::View<Message> {
    let open_count = track.notes.iter().filter(|note| !note.done).count();
    let mut children = vec![
        ui::row([
            chrome::text("ALL COMMENTS").height(24.0).fill_width(),
            chrome::muted_text(format!("{} total · {} open", track.notes.len(), open_count))
                .height(24.0)
                .subtle(),
        ])
        .fill_width()
        .spacing(10.0),
    ];
    if track.notes.is_empty() {
        children.push(
            chrome::muted_text(
                "Hover the lower waveform rail and click to place a timestamped comment.",
            )
            .wrap()
            .height(42.0)
            .fill_width()
            .subtle(),
        );
    } else {
        children.push(
            ui::list(
                track.notes.clone().into_iter().enumerate(),
                |(index, note)| note_row(index, note),
            )
            .fill_width()
            .fill_height(),
        );
    }
    ui::column(children)
        .padding(12.0)
        .spacing(8.0)
        .fill_width()
        .fill_height()
}

fn note_editor(draft: &NoteDraft) -> ui::View<Message> {
    ui::column([
        chrome::muted_text(format!(
            "COMMENT AT {}",
            format_timestamp(draft.time_millis)
        ))
        .height(20.0)
        .fill_width()
        .subtle(),
        ui::text_input(draft.body.clone())
            .placeholder("Write a comment…")
            .message(Message::DraftNoteChanged)
            .key("native-note-draft")
            .fill_width()
            .height(38.0),
        ui::row([
            ui::button("Save comment")
                .primary()
                .message(Message::SaveDraftNote)
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
            ui::button("Cancel")
                .subtle()
                .message(Message::CancelDraftNote)
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
        ])
        .spacing(8.0)
        .fill_width(),
    ])
    .padding(10.0)
    .spacing(8.0)
    .fill_width()
}

fn note_row(index: usize, note: storage::Note) -> ui::View<Message> {
    let note_id = note.id.clone();
    let edit_id = note.id.clone();
    let delete_id = note.id.clone();
    ui::list_row(
        index,
        [
            chrome::muted_text(format_timestamp(note.time_millis))
                .height(30.0)
                .width(68.0)
                .subtle(),
            chrome::text(note.body).wrap().height(30.0).fill_width(),
            ui::button(if note.done { "Done" } else { "Open" })
                .subtle()
                .message(Message::ToggleNoteDone(note_id))
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
            ui::button("Edit")
                .subtle()
                .message(Message::EditNote(edit_id))
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
            ui::button("Delete")
                .subtle()
                .message(Message::DeleteNote(delete_id))
                .text_color(ui::TextColorRole::Custom(chrome::TEXT_PRIMARY)),
        ],
    )
    .fill_width()
}

fn selected_track(state: &AppState) -> Option<&storage::Track> {
    state
        .library
        .selected_track_id
        .as_ref()
        .and_then(|id| state.library.tracks.iter().find(|track| &track.id == id))
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
        AppState, Message, NoteDraft, apply_transport_snapshot, decode_result_is_current,
        native_launch_options, planner_drop_is_valid, playback_shortcut, project_surface,
        stage_dropdown, stage_menu_anchor_from_pointer, stage_menu_popover, tracks_in_stage,
        transport_command_is_confirmed, update,
    };
    use crate::transport::Snapshot;
    use crate::{
        audio::WaveformData,
        storage::{Note, Track, TrackStage},
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
    use std::{path::PathBuf, sync::Arc};

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
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
            notes: Vec::new(),
        });
        state
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
    fn playhead_drag_tracks_pointer_and_starts_playback_on_release() {
        let mut state = AppState {
            busy: false,
            waveform: Some(WaveformData {
                sample_rate: 48_000,
                channels: 1,
                duration_millis: 1_000,
                render_frames: 48_000,
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
    }

    #[test]
    fn native_launch_starts_maximized() {
        assert!(native_launch_options().window.behavior.maximized);
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
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
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

        let draft = state
            .draft_note
            .as_ref()
            .expect("a lower waveform click should create a draft");
        assert_eq!(draft.time_millis, 1_000);

        let frame = project_surface(&state)
            .view_frame_at_size_with_default_theme(Vector2::new(1180.0, 1000.0));
        let waveform_label_rect = frame
            .paint_plan
            .first_text_rect("COMMENTS / CLICK TO PIN")
            .expect("the waveform comment rail should be visible");
        let editor_rect = frame
            .paint_plan
            .first_text_rect("COMMENT AT 00:01")
            .expect("the draft editor should be visible after a lower click");
        let comments_rect = frame
            .paint_plan
            .first_text_rect("ALL COMMENTS")
            .expect("the lower comments panel should remain visible");
        let labels = frame.paint_plan.text_label_strings();

        assert_eq!(
            labels
                .iter()
                .filter(|label| label.as_str() == "COMMENT AT 00:01")
                .count(),
            1,
            "the draft editor should have one visible timestamp header"
        );
        assert!(
            editor_rect.min.y >= waveform_label_rect.max.y
                && editor_rect.min.y < comments_rect.min.y,
            "the draft editor should be rendered beneath the waveform and before the lower comments list"
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
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
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
            size: 0,
            favorite: false,
            stage: TrackStage::SoundDesign,
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

        assert!(labels.iter().any(|label| label == "ALL COMMENTS"));
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
    fn waveform_completion_requires_the_current_generation_and_selection() {
        let mut state = AppState::default();
        state.library.selected_track_id = Some(String::from("track-a"));
        state.waveform_generation = 7;
        assert!(decode_result_is_current(&state, "track-a", 7));
        assert!(!decode_result_is_current(&state, "track-a", 6));
        assert!(!decode_result_is_current(&state, "track-b", 7));
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
            size: 0,
            favorite: false,
            stage,
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
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
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
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
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
                let trigger = stage_dropdown(&state.track, state.open);
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
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
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
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
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
                let trigger = stage_dropdown(&state.track, state.open);
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
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
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
            let menu_surface = frame
                .paint_plan
                .primitives
                .iter()
                .find_map(|primitive| match primitive {
                    PaintPrimitive::FillRect(fill)
                        if (fill.rect.min.x - anchor.x).abs() < 0.01
                            && (fill.rect.min.y - anchor.y).abs() < 0.01 =>
                    {
                        Some(fill.rect)
                    }
                    _ => None,
                })
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
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
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
            size: 0,
            favorite: false,
            stage: TrackStage::Production,
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
        let planner_menu_rect = planner_frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if (fill.rect.min.x - planner_anchor.x).abs() < 0.01
                        && (fill.rect.min.y - planner_anchor.y).abs() < 0.01 =>
                {
                    Some(fill.rect)
                }
                _ => None,
            })
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
        let library_menu_rect = library_frame
            .paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if (fill.rect.min.x - library_anchor.x).abs() < 0.01
                        && (fill.rect.min.y - library_anchor.y).abs() < 0.01 =>
                {
                    Some(fill.rect)
                }
                _ => None,
            })
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
}

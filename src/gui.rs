//! GPUI implementation of the Cadence Frame design.
use crate::{
    controller::{Controller, ImportKind, Source, TICK},
    storage::{self, Track, TrackStage},
    tags::Tag,
};
use gpui::{prelude::*, *};
use gpui_component::{
    IconName, Theme, ThemeMode,
    input::{Input, InputEvent, InputState},
    slider::{Slider, SliderEvent, SliderState},
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    rc::Rc,
};

#[cfg(target_os = "macos")]
const MONO: &str = "Menlo";
#[cfg(target_os = "windows")]
const MONO: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const MONO: &str = "Liberation Mono";

const BG: u32 = 0x080808;
const WELL: u32 = 0x040404;
const SURFACE: u32 = 0x111111;
const SURFACE_ALT: u32 = 0x151515;
const SURFACE_HOVER: u32 = 0x1c1c1c;
const ACTIVE_BG: u32 = 0x1a0b0c;
const ACTIVE_HOVER: u32 = 0x261012;
const BORDER: u32 = 0x262626;
const BORDER_SUBTLE: u32 = 0x1a1a1a;
const BORDER_STRONG: u32 = 0x404040;
const TEXT: u32 = 0xf0f0f0;
const TEXT_BRIGHT: u32 = 0xffffff;
const MUTED: u32 = 0x8a8a8a;
const DIM: u32 = 0x525252;
const WAVE_UNPLAYED: u32 = 0xa8a8a8;
const WAVE_PEAK_UNPLAYED: u32 = 0x3b3b3b;
const WAVE_PEAK_PLAYED: u32 = 0x731919;
const WAVE_LOWER_BG: u32 = 0x020202;
const WAVE_LOWER_UNPLAYED: u32 = 0x454545;
const WAVE_LOWER_PEAK_UNPLAYED: u32 = 0x1b1b1b;
const WAVE_LOWER_PLAYED: u32 = 0x6b1515;
const WAVE_LOWER_PEAK_PLAYED: u32 = 0x360c0c;
const ACCENT: u32 = 0xe02424;
const ACCENT_HOVER: u32 = 0xf23636;
const ACCENT_TEXT: u32 = 0xff4747;
const ACCENT_DIM: u32 = 0x7a1818;
const COLORS: [u32; 5] = [0xe02424, 0xf0f0f0, 0xa3a3a3, 0x6e6e6e, 0x991b1b];

pub fn apply_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let theme = Theme::global_mut(cx);
    theme.font_family = MONO.into();
    theme.mono_font_family = MONO.into();
    theme.radius = px(0.);
    theme.radius_lg = px(0.);
    theme.shadow = false;
    let colors = &mut theme.colors;
    colors.background = rgb(BG).into();
    colors.foreground = rgb(TEXT).into();
    colors.border = rgb(BORDER).into();
    colors.input = rgb(BORDER).into();
    colors.popover = rgb(SURFACE).into();
    colors.popover_foreground = rgb(TEXT).into();
    colors.primary = rgb(SURFACE).into();
    colors.primary_hover = rgb(SURFACE_HOVER).into();
    colors.primary_active = rgb(ACTIVE_BG).into();
    colors.primary_foreground = rgb(TEXT).into();
    colors.secondary = rgb(SURFACE).into();
    colors.secondary_hover = rgb(SURFACE_HOVER).into();
    colors.secondary_foreground = rgb(TEXT).into();
    colors.secondary_active = rgb(ACTIVE_BG).into();
    colors.muted = rgb(SURFACE_ALT).into();
    colors.muted_foreground = rgb(MUTED).into();
    colors.accent = rgb(ACTIVE_BG).into();
    colors.accent_foreground = rgb(TEXT_BRIGHT).into();
    colors.danger = rgb(ACCENT).into();
    colors.danger_hover = rgb(ACCENT_HOVER).into();
    colors.danger_active = rgb(ACCENT_DIM).into();
    colors.danger_foreground = rgb(TEXT_BRIGHT).into();
    colors.ring = rgb(ACCENT).into();
    colors.selection = rgba(0xe0242444).into();
    colors.slider_bar = rgb(ACCENT).into();
    colors.slider_thumb = rgb(TEXT_BRIGHT).into();
    colors.caret = rgb(ACCENT).into();
    colors.scrollbar = rgb(BG).into();
    colors.scrollbar_thumb = rgb(0x2e2e2e).into();
    colors.scrollbar_thumb_hover = rgb(0x4a4a4a).into();
    colors.list = rgb(BG).into();
    colors.list_hover = rgb(SURFACE_HOVER).into();
    colors.list_active = rgb(ACTIVE_BG).into();
    colors.list_active_border = rgb(ACCENT).into();
    colors.overlay = rgba(0x000000cc).into();
}
actions!(
    cadence,
    [
        PlayPause,
        Stop,
        NewNote,
        CloseOverlay,
        Quit,
        NextControl,
        PreviousControl,
        VolumeUp,
        VolumeDown,
        VolumeMin,
        VolumeMax,
        SeekForward,
        SeekBackward,
        PageForward,
        PageBackward
    ]
);
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Review,
    Planner,
    Tags,
    Tracks,
    References,
}
#[derive(Clone, PartialEq, Eq)]
enum Confirmation {
    Track(String),
    Tracks(Vec<String>),
    Tag(String),
    Group(String),
    Reference(PathBuf),
    Bind(Source),
    Recovery,
}
#[derive(Clone, PartialEq, Eq)]
enum Modal {
    Stage(String),
    BulkStage,
    FilterStage,
    BulkTags,
    BulkAlbums,
    Assign(String),
    EditTrack(String),
    AssignToTrack(String),
    References,
    TagGroup,
    Group(Option<String>),
    Confirm(Confirmation),
}
#[derive(Clone)]
enum Command {
    Mode(Mode),
    ManagerToggle(String),
    ManagerSelectAll,
    ManagerClear,
    ManagerFilter(Option<String>, Option<String>, bool),
    ManagerSort,
    ManagerStageMenu(bool),
    ManagerStage(Option<TrackStage>, bool),
    ManagerTags,
    ManagerAlbums,
    ManagerAlbum(String),
    ManagerTag(String, bool),
    ManagerApply,
    ManagerRemove,
    Import(ImportKind),
    Select(String),
    Favorite(String),
    StageMenu(String),
    Stage(String, TrackStage),
    EditTrack(String, bool),
    SaveTrackDetails,
    SelectRevision(String, String),
    AssignToTrackMenu(String),
    AssignToTrack(String, String),
    ResolveCollisionKeep(String, String),
    ResolveCollisionReplace(String, String),
    ResolveCollisionSplit(String, String),
    RemoveTrack(String),
    Assign(String),
    ToggleTag(String, String),
    Play(Source),
    Match,
    Mute,
    References,
    Reference(PathBuf),
    ReferenceRename(PathBuf),
    ReferenceRenameSave,
    RemoveReference(PathBuf),
    ClearReference,
    TagsGroup(Option<String>),
    NewTag,
    NewAlbum,
    EditTag(String),
    SaveTag,
    DeleteTag(String),
    TagGroupMenu,
    TagGroup(String),
    Color(u32),
    NewGroup,
    EditGroup(String),
    SaveGroup,
    AlbumGroup,
    DeleteGroup(String),
    Comments(Source),
    NewNote(Source, u64),
    EditNote(Source, String),
    SaveNote,
    DeleteNote(Source, String),
    ResolveNote(Source, String),
    PlayNote(Source, u64),
    Bind(Source),
    Confirm,
    Cancel,
    RetrySave,
    RetryLoad,
    Recover,
    Spectrum(u8),
    ClearLoop(Source),
    Stop,
    SeekBy(Source, i64),
    Volume(f32),
    DismissError,
}
#[derive(Clone)]
struct TrackDrag {
    id: String,
    title: String,
}
impl Render for TrackDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_3()
            .font_family(MONO)
            .text_size(px(12.))
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(ACCENT))
            .rounded(px(0.))
            .text_color(rgb(TEXT_BRIGHT))
            .child(self.title.clone())
    }
}
#[derive(Clone)]
struct NoteDraft {
    source: Source,
    id: Option<String>,
    time: u64,
    generation: u64,
}
#[derive(Clone)]
struct FocusTarget {
    handle: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    scrolls: Vec<ScrollHandle>,
    seen: u64,
    in_modal: bool,
}
struct ScrollScope<'a>(&'a RefCell<Vec<(&'static str, ScrollHandle)>>);
impl Drop for ScrollScope<'_> {
    fn drop(&mut self) {
        self.0.borrow_mut().pop();
    }
}
pub struct Cadence {
    pub core: Controller,
    library_scroll: UniformListScrollHandle,
    mode: Mode,
    focus: FocusHandle,
    modal_focus: FocusHandle,
    modal_start: FocusHandle,
    modal_end: FocusHandle,
    return_focus: Option<FocusHandle>,
    targets: RefCell<HashMap<ElementId, FocusTarget>>,
    focus_frame: Cell<u64>,
    scroll_handles: RefCell<HashMap<&'static str, ScrollHandle>>,
    scroll_context: RefCell<Vec<(&'static str, ScrollHandle)>>,
    modal: Option<Modal>,
    modal_error: Option<String>,
    closing: bool,
    search: Entity<InputState>,
    track_search: Entity<InputState>,
    reference_search: Entity<InputState>,
    manager_filter: crate::track_manager::Filter,
    manager_selected: BTreeSet<String>,
    manager_edit: crate::track_manager::BulkEdit,
    name: Entity<InputState>,
    group_name: Entity<InputState>,
    custom_color: Entity<InputState>,
    comment: Entity<InputState>,
    reference_name: Entity<InputState>,
    track_title_input: Entity<InputState>,
    next_action_input: Entity<InputState>,
    revision_feedback_input: Entity<InputState>,
    edit_track_focus_next_action: bool,
    volume: Entity<SliderState>,
    group_filter: Option<String>,
    tag_id: Option<String>,
    tag_group: String,
    tag_color: u32,
    group_album: bool,
    comment_source: Source,
    note_draft: Option<NoteDraft>,
    // Marker movement is a draft until Save; cancellation leaves the stored note intact.
    note_dragging: bool,
    note_error: Option<String>,
    reference_edit: Option<PathBuf>,
    spectrum: u8,
    waveform_bounds: [Rc<Cell<Bounds<Pixels>>>; 2],
    waveform_bars: [Rc<RefCell<Option<WaveformBarsCache>>>; 2],
    waveform_hover: [Option<WaveformHover>; 2],
    gesture: Option<(Source, f32, bool)>,
    spectrogram_image: RefCell<crate::live_visual::SpectrogramImageCache>,
    #[cfg(target_os = "macos")]
    spectrogram_gpu: RefCell<Option<crate::live_visual_metal::GpuSpectrogramRenderer>>,
    _subscriptions: Vec<Subscription>,
}
impl Cadence {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search tags"));
        let track_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search tracks"));
        let reference_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search reference catalog"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Tag name"));
        let group_name = cx.new(|cx| InputState::new(window, cx).placeholder("Group name"));
        let custom_color = cx.new(|cx| InputState::new(window, cx).placeholder("#e02424"));
        let comment = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .auto_grow(3, 6)
                .placeholder("Write a comment…")
        });
        let reference_name = cx.new(|cx| InputState::new(window, cx).placeholder("Reference name"));
        let track_title_input = cx.new(|cx| InputState::new(window, cx).placeholder("Track title"));
        let next_action_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("One next edit or action…"));
        let revision_feedback_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("e.g. Heard by Dylan — tighten intro 4 bars")
        });
        let volume = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(1.0)
                .step(0.01)
                .default_value(0.8)
        });
        let mut subscriptions = Vec::new();
        for input in [
            &search,
            &track_search,
            &reference_search,
            &name,
            &group_name,
            &custom_color,
            &comment,
            &reference_name,
            &track_title_input,
            &next_action_input,
            &revision_feedback_input,
        ] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |_, _, _: &InputEvent, window, cx| {
                    window.refresh();
                    cx.notify();
                },
            ));
        }
        for input in [
            &track_title_input,
            &next_action_input,
            &revision_feedback_input,
        ] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. })
                        && matches!(this.modal, Some(Modal::EditTrack(_)))
                    {
                        this.act(Command::SaveTrackDetails, window, cx);
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe(&volume, |this, _, event: &SliderEvent, cx| {
            let SliderEvent::Change(value) = event;
            this.core.volume = value.start();
            this.core.apply_gain();
            cx.notify();
        }));
        subscriptions.push(cx.on_app_quit(|this, cx| {
            let done = this.core.shutdown_barrier();
            let executor = cx.background_executor().clone();
            async move {
                while !done.load(std::sync::atomic::Ordering::Acquire) {
                    executor.timer(std::time::Duration::from_millis(10)).await;
                }
            }
        }));
        let focus = cx.focus_handle();
        focus.focus(window);
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |this, cx| {
                if this.core.importing || this.core.dirty() {
                    this.closing = true;
                    this.core.error = Some(
                        "Finishing library save before closing. Retry if saving failed.".into(),
                    );
                    cx.notify();
                    false
                } else {
                    true
                }
            })
            .unwrap_or(true)
        });
        cx.spawn(async move |entity, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if entity
                    .update(cx, |this, cx| {
                        let changed = this.core.poll();
                        #[cfg(target_os = "macos")]
                        let gpu_changed = if matches!(this.mode, Mode::Review) && this.spectrum == 2
                        {
                            let source = this.core.source.index();
                            let (_, frame) = this.core.transports[source].live_frame_snapshot();
                            this.spectrogram_gpu
                                .borrow_mut()
                                .as_mut()
                                .is_some_and(|gpu| gpu.poll(source, frame.as_deref()))
                        } else {
                            false
                        };
                        #[cfg(not(target_os = "macos"))]
                        let gpu_changed = false;
                        if changed || gpu_changed {
                            cx.notify();
                        }
                        if this.closing && !this.core.dirty() && !this.core.importing {
                            cx.quit();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            core: Controller::new(),
            library_scroll: UniformListScrollHandle::new(),
            mode: Mode::Review,
            focus,
            modal_focus: cx.focus_handle().tab_stop(false),
            modal_start: cx.focus_handle().tab_index(-1).tab_stop(false),
            modal_end: cx.focus_handle().tab_index(isize::MAX).tab_stop(false),
            return_focus: None,
            targets: Default::default(),
            focus_frame: Cell::new(0),
            scroll_handles: Default::default(),
            scroll_context: Default::default(),
            modal: None,
            modal_error: None,
            closing: false,
            search,
            track_search,
            reference_search,
            manager_filter: Default::default(),
            manager_selected: Default::default(),
            manager_edit: Default::default(),
            name,
            group_name,
            custom_color,
            comment,
            reference_name,
            track_title_input,
            next_action_input,
            revision_feedback_input,
            edit_track_focus_next_action: false,
            volume,
            group_filter: Some("tasks".into()),
            tag_id: None,
            tag_group: "tasks".into(),
            tag_color: COLORS[0],
            group_album: false,
            comment_source: Source::Main,
            note_draft: None,
            note_dragging: false,
            note_error: None,
            reference_edit: None,
            spectrum: 0,
            waveform_bounds: [
                Rc::new(Cell::new(Bounds::default())),
                Rc::new(Cell::new(Bounds::default())),
            ],
            waveform_bars: [Rc::new(RefCell::new(None)), Rc::new(RefCell::new(None))],
            waveform_hover: [None, None],
            gesture: None,
            spectrogram_image: Default::default(),
            #[cfg(target_os = "macos")]
            spectrogram_gpu: RefCell::new(None),
            _subscriptions: subscriptions,
        }
    }
    fn pick(&mut self, kind: ImportKind, cx: &mut Context<Self>) {
        let prompt = match &kind {
            ImportKind::Relink(_, _) => "Locate moved audio file to relink",
            ImportKind::AddRevision(_) => "Choose bounce revision audio file(s)",
            _ => "Choose audio files",
        };
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: matches!(
                kind,
                ImportKind::Main | ImportKind::Catalog | ImportKind::AddRevision(_)
            ),
            prompt: Some(prompt.into()),
        });
        cx.spawn(async move |entity, cx| match picker.await {
            Ok(Ok(Some(paths))) => {
                let _ = entity.update(cx, |this, cx| {
                    this.core.import(paths, kind);
                    cx.notify();
                });
            }
            Ok(Err(error)) => {
                let _ = entity.update(cx, |this, cx| {
                    this.core.error = Some(error.to_string());
                    cx.notify();
                });
            }
            _ => {}
        })
        .detach();
    }
    fn set_input(
        input: &Entity<InputState>,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        input.update(cx, |i, cx| i.set_value(value, window, cx));
    }
    fn act(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let previous_modal = self.modal.clone();
        let previous_mode = self.mode;
        let opener = window.focused(cx);
        let completed_confirmation = matches!(&command, Command::Confirm);
        let previous_error = self.core.error.clone();
        let validation = matches!(
            &command,
            Command::SaveGroup
                | Command::SaveNote
                | Command::SaveTrackDetails
                | Command::Confirm
                | Command::ToggleTag(_, _)
        );
        match command {
            Command::ManagerToggle(id) => {
                if !self.manager_selected.remove(&id) {
                    self.manager_selected.insert(id);
                }
                self.manager_edit = Default::default();
            }
            Command::ManagerSelectAll => {
                self.manager_selected = self.manager_ids(cx).into_iter().collect();
                self.manager_edit = Default::default();
            }
            Command::ManagerClear => {
                self.manager_selected.clear();
                self.manager_edit = Default::default();
            }
            Command::ManagerFilter(tag, group, favorites) => {
                self.manager_filter.tag = tag;
                self.manager_filter.group = group;
                self.manager_filter.favorites = favorites;
            }
            Command::ManagerSort => self.manager_filter.reverse = !self.manager_filter.reverse,
            Command::ManagerStageMenu(filter) => {
                self.modal = Some(if filter {
                    Modal::FilterStage
                } else {
                    Modal::BulkStage
                })
            }
            Command::ManagerStage(stage, filter) => {
                if filter {
                    self.manager_filter.stage = stage;
                } else {
                    self.manager_edit.stage = stage;
                }
                self.modal = None;
            }
            Command::ManagerTags => self.modal = Some(Modal::BulkTags),
            Command::ManagerAlbums => self.modal = Some(Modal::BulkAlbums),
            Command::ManagerAlbum(id) => {
                self.manager_edit.remove.remove(&id);
                self.manager_edit.add.insert(id);
                self.modal = None;
            }
            Command::ManagerTag(id, add) => {
                let (target, other) = if add {
                    (&mut self.manager_edit.add, &mut self.manager_edit.remove)
                } else {
                    (&mut self.manager_edit.remove, &mut self.manager_edit.add)
                };
                other.remove(&id);
                if !target.remove(&id) {
                    target.insert(id);
                }
            }
            Command::ManagerApply => {
                if self.manager_edit.is_empty() {
                    return;
                }
                if self.core.can_edit() {
                    match crate::track_manager::apply(
                        &mut self.core.library,
                        &self.manager_selected,
                        &self.manager_edit,
                    ) {
                        Ok(changed) => {
                            if changed {
                                self.core.persist();
                            }
                            self.manager_edit = Default::default();
                        }
                        Err(e) => self.core.error = Some(e),
                    }
                }
            }
            Command::ManagerRemove => {
                if !self.manager_selected.is_empty() {
                    self.modal = Some(Modal::Confirm(Confirmation::Tracks(
                        self.manager_selected.iter().cloned().collect(),
                    )));
                }
            }
            Command::Mode(mode) => {
                self.mode = mode;
                self.modal = None;
                let on_tab = (0usize..5).any(|i| {
                    self.targets
                        .borrow()
                        .get(&ElementId::from(("workspace-tab", i)))
                        .is_some_and(|t| t.handle.is_focused(window))
                });
                if !on_tab {
                    self.focus.focus(window);
                }
            }
            Command::Import(kind) => self.pick(kind, cx),
            Command::Select(id) => {
                self.mode = Mode::Review;
                self.comment_source = Source::Main;
                if let Some(index) = self.core.library.tracks.iter().position(|t| t.id == id) {
                    self.library_scroll
                        .scroll_to_item(index, ScrollStrategy::Center);
                }
                self.core.select(id);
                self.note_draft = None;
                self.modal = None;
            }
            Command::Favorite(id) => self.core.favorite(&id),
            Command::StageMenu(id) => self.modal = Some(Modal::Stage(id)),
            Command::Stage(id, stage) => {
                let slot = self
                    .core
                    .library
                    .tracks
                    .iter()
                    .filter(|t| t.stage == stage)
                    .count();
                self.core.stage(&id, stage, slot);
                self.modal = None;
            }
            Command::EditTrack(id, focus_next_action) => {
                if let Some(track) = self
                    .core
                    .library
                    .tracks
                    .iter()
                    .find(|t| t.id == id)
                    .cloned()
                {
                    let feedback = track
                        .active_revision()
                        .and_then(|rev| rev.feedback_note.clone())
                        .unwrap_or_default();
                    Self::set_input(&self.track_title_input, track.title, window, cx);
                    Self::set_input(&self.next_action_input, track.next_action, window, cx);
                    Self::set_input(&self.revision_feedback_input, feedback, window, cx);
                    self.edit_track_focus_next_action = focus_next_action;
                    self.modal = Some(Modal::EditTrack(id));
                }
            }
            Command::SaveTrackDetails => {
                if let Some(Modal::EditTrack(id)) = self.modal.clone() {
                    let title = self.track_title_input.read(cx).value().to_string();
                    let next_action = self.next_action_input.read(cx).value().to_string();
                    let feedback = self.revision_feedback_input.read(cx).value().to_string();
                    let active_rev_id = self
                        .core
                        .library
                        .tracks
                        .iter()
                        .find(|t| t.id == id)
                        .and_then(|t| t.active_revision_id.clone());
                    if self.core.set_title(&id, &title)
                        && self.core.set_next_action(&id, &next_action)
                        && self
                            .core
                            .set_revision_feedback(&id, active_rev_id.as_deref(), &feedback)
                    {
                        self.core.error = None;
                        self.modal = None;
                    }
                }
            }
            Command::SelectRevision(track_id, revision_id) => {
                self.note_draft = None;
                self.core.select_revision(&track_id, &revision_id);
            }
            Command::AssignToTrackMenu(id) => self.modal = Some(Modal::AssignToTrack(id)),
            Command::AssignToTrack(source_id, target_id) => {
                if self.core.assign_track_as_revision(&source_id, &target_id) {
                    self.modal = None;
                }
            }
            Command::ResolveCollisionKeep(track_id, collision_id) => {
                self.core.resolve_collision_keep(&track_id, &collision_id);
            }
            Command::ResolveCollisionReplace(track_id, collision_id) => {
                self.core.resolve_collision_replace(&track_id, &collision_id);
            }
            Command::ResolveCollisionSplit(track_id, collision_id) => {
                self.core.resolve_collision_split(&track_id, &collision_id);
            }
            Command::RemoveTrack(id) => self.modal = Some(Modal::Confirm(Confirmation::Track(id))),
            Command::Assign(id) => self.modal = Some(Modal::Assign(id)),
            Command::ToggleTag(track, id) => {
                if self.core.can_edit() {
                    match self.core.library.tags.toggle(&track, &id) {
                        Ok(()) => self.core.persist(),
                        Err(e) => self.core.error = Some(e),
                    }
                }
            }
            Command::Play(src) => self.core.toggle_play(src),
            Command::Stop => self.core.stop(),
            Command::Comments(src) => self.comment_source = src,
            Command::Match => {
                self.core.matching = !self.core.matching;
                self.core.apply_gain();
            }
            Command::Mute => {
                self.core.muted = !self.core.muted;
                self.core.apply_gain();
            }
            Command::References => self.modal = Some(Modal::References),
            Command::Reference(path) => {
                self.core.assign_reference(path);
                self.modal = None;
            }
            Command::ClearReference => {
                if self.core.can_edit()
                    && let Some(id) = self.core.library.selected_track_id.clone()
                    && let Some(t) = self.core.library.tracks.find_mut(|t| t.id == id)
                {
                    t.reference_path = None;
                    self.core.load_selected();
                    self.core.persist();
                }
                self.modal = None;
            }
            Command::ReferenceRename(path) => {
                let label = self.reference_label(&path);
                self.reference_edit = Some(path);
                Self::set_input(&self.reference_name, label, window, cx);
                self.reference_name
                    .update(cx, |input, cx| input.focus(window, cx));
            }
            Command::ReferenceRenameSave => {
                if self.core.can_edit()
                    && let Some(path) = self.reference_edit.clone()
                {
                    let name = self.reference_name.read(cx).value();
                    match storage::rename_reference_track(&mut self.core.library, &path, &name) {
                        Ok(_) => {
                            self.core.persist();
                            self.reference_edit = None;
                        }
                        Err(e) => self.core.error = Some(e),
                    }
                }
            }
            Command::RemoveReference(path) => {
                self.modal = Some(Modal::Confirm(Confirmation::Reference(path)))
            }
            Command::TagsGroup(group) => {
                self.tag_group = group
                    .clone()
                    .or_else(|| self.core.library.tags.groups.first().map(|g| g.id.clone()))
                    .unwrap_or_default();
                self.group_filter = group;
                self.tag_id = None;
                Self::set_input(&self.name, String::new(), window, cx);
            }
            Command::NewTag | Command::NewAlbum => {
                let album = matches!(command, Command::NewAlbum);
                if album {
                    let Some(group) = self.core.library.tags.groups.iter().find(|g| g.album) else {
                        self.core.error =
                            Some("Create an album group before adding an album.".into());
                        cx.notify();
                        return;
                    };
                    self.group_filter = Some(group.id.clone());
                }
                self.mode = Mode::Tags;
                self.modal = None;
                self.tag_id = None;
                self.tag_group = self
                    .group_filter
                    .clone()
                    .or_else(|| self.core.library.tags.groups.first().map(|g| g.id.clone()))
                    .unwrap_or_default();
                Self::set_input(&self.name, String::new(), window, cx);
                self.name.update(cx, |input, cx| input.focus(window, cx));
            }
            Command::EditTag(id) => {
                if let Some(tag) = self
                    .core
                    .library
                    .tags
                    .tags
                    .iter()
                    .find(|t| t.id == id)
                    .cloned()
                {
                    self.tag_id = Some(id);
                    self.tag_group = tag.group;
                    self.tag_color = tag.color;
                    Self::set_input(&self.name, tag.name, window, cx);
                    Self::set_input(
                        &self.custom_color,
                        format!("#{:06x}", tag.color),
                        window,
                        cx,
                    );
                    self.name.update(cx, |input, cx| input.focus(window, cx));
                }
            }
            Command::SaveTag => {
                if self.core.can_edit() {
                    let color_text = self.custom_color.read(cx).value();
                    let color = if color_text.trim().is_empty() {
                        Some(self.tag_color)
                    } else {
                        let hex = color_text.trim().trim_start_matches('#');
                        (hex.len() == 6)
                            .then(|| u32::from_str_radix(hex, 16).ok())
                            .flatten()
                    };
                    if let Some(color) = color {
                        let name = self.name.read(cx).value();
                        match self.core.library.tags.save_tag(
                            self.tag_id.as_deref(),
                            &name,
                            &self.tag_group,
                            color,
                        ) {
                            Ok(id) => {
                                self.tag_id = Some(id);
                                self.tag_color = color;
                                self.core.persist();
                            }
                            Err(e) => self.core.error = Some(e),
                        }
                    } else {
                        self.core.error = Some("Enter a six-digit hex color.".into());
                    }
                }
            }
            Command::DeleteTag(id) => self.modal = Some(Modal::Confirm(Confirmation::Tag(id))),
            Command::TagGroupMenu => self.modal = Some(Modal::TagGroup),
            Command::TagGroup(id) => {
                self.tag_group = id;
                self.modal = None;
            }
            Command::Color(color) => {
                self.tag_color = color;
                Self::set_input(&self.custom_color, format!("#{color:06x}"), window, cx);
            }
            Command::NewGroup => {
                Self::set_input(&self.group_name, String::new(), window, cx);
                self.group_album = false;
                self.modal = Some(Modal::Group(None));
            }
            Command::EditGroup(id) => {
                if let Some(g) = self
                    .core
                    .library
                    .tags
                    .groups
                    .iter()
                    .find(|g| g.id == id)
                    .cloned()
                {
                    Self::set_input(&self.group_name, g.name, window, cx);
                    self.group_album = g.album;
                    self.modal = Some(Modal::Group(Some(id)));
                }
            }
            Command::AlbumGroup => {
                self.group_album = !self.group_album;
            }
            Command::SaveGroup => {
                if self.core.can_edit() {
                    let name = self.group_name.read(cx).value();
                    let existing = match &self.modal {
                        Some(Modal::Group(id)) => id.clone(),
                        _ => None,
                    };
                    let result = if let Some(id) = existing {
                        self.core
                            .library
                            .tags
                            .update_group(&id, &name, self.group_album)
                            .map(|_| id)
                    } else {
                        self.core.library.tags.create_group(&name, self.group_album)
                    };
                    match result {
                        Ok(id) => {
                            self.group_filter = Some(id.clone());
                            self.tag_group = id;
                            self.core.persist();
                            self.modal = None;
                        }
                        Err(e) => self.core.error = Some(e),
                    }
                }
            }
            Command::DeleteGroup(id) => self.modal = Some(Modal::Confirm(Confirmation::Group(id))),
            Command::NewNote(src, time) => {
                self.note_draft = Some(NoteDraft {
                    source: src,
                    id: None,
                    time,
                    generation: self.core.generation,
                });
                Self::set_input(&self.comment, String::new(), window, cx);
                self.note_error = None;
                self.comment.update(cx, |input, cx| input.focus(window, cx));
            }
            Command::EditNote(src, id) => {
                if let Some(note) = self.core.notes(src).iter().find(|n| n.id == id).cloned() {
                    self.note_draft = Some(NoteDraft {
                        source: src,
                        id: Some(id),
                        time: note.time_millis,
                        generation: self.core.generation,
                    });
                    Self::set_input(&self.comment, note.body, window, cx);
                    self.note_error = None;
                    self.comment.update(cx, |input, cx| input.focus(window, cx));
                }
            }
            Command::SaveNote => {
                if let Some(draft) = &self.note_draft {
                    if draft.generation != self.core.generation {
                        self.core.error = Some("The track changed while this comment was open. Reopen the comment before saving.".into());
                    } else {
                        let text = self.comment.read(cx).value();
                        if self
                            .core
                            .save_note(draft.source, draft.id.as_deref(), draft.time, &text)
                        {
                            self.core.error = None;
                            self.modal = None;
                            self.note_draft = None;
                            self.note_error = None;
                            self.focus.focus(window);
                        }
                    }
                }
            }
            Command::DeleteNote(src, id) => {
                if self
                    .note_draft
                    .as_ref()
                    .is_some_and(|d| d.source == src && d.id.as_ref() == Some(&id))
                {
                    self.note_draft = None;
                }
                self.core.delete_note(src, &id);
            }
            Command::ResolveNote(src, id) => self.core.resolve_note(src, &id),
            Command::PlayNote(src, time) => {
                if self.core.source != src {
                    self.core.switch_source(src);
                }
                self.core.seek(time, true);
            }
            Command::Bind(src) => self.modal = Some(Modal::Confirm(Confirmation::Bind(src))),
            Command::Confirm => {
                if let Some(Modal::Confirm(kind)) = self.modal.clone()
                    && (self.core.can_edit() || matches!(kind, Confirmation::Recovery))
                {
                    self.modal = None;
                    match kind {
                        Confirmation::Track(id) => self.core.remove(&id),
                        Confirmation::Tracks(ids) => {
                            self.core.remove_bulk(&ids);
                            self.manager_selected.clear();
                            self.manager_edit = Default::default();
                        }
                        Confirmation::Tag(id) => {
                            self.core.library.tags.remove_tag(&id);
                            self.tag_id = None;
                            self.core.persist();
                        }
                        Confirmation::Group(id) => {
                            self.core.library.tags.remove_group(&id);
                            self.group_filter = None;
                            self.tag_group = self
                                .core
                                .library
                                .tags
                                .groups
                                .first()
                                .map(|g| g.id.clone())
                                .unwrap_or_default();
                            self.tag_id = None;
                            self.core.persist();
                        }
                        Confirmation::Reference(path) => {
                            match storage::remove_reference_track(&mut self.core.library, &path) {
                                Ok(_) => {
                                    self.core.load_selected();
                                    self.core.persist();
                                }
                                Err(e) => self.core.error = Some(e),
                            }
                        }
                        Confirmation::Bind(src) => self.core.bind_historical(src),
                        Confirmation::Recovery => self.core.recover(),
                    }
                }
            }
            Command::Cancel => {
                if self.modal.is_none() {
                    self.reference_edit = None;
                }
                self.modal = None;
                self.note_draft = None;
                self.note_dragging = false;
                self.note_error = None;
                self.focus.focus(window);
            }
            Command::RetrySave => self.core.retry_save(),
            Command::RetryLoad => self.core.retry_load(),
            Command::Recover => self.modal = Some(Modal::Confirm(Confirmation::Recovery)),
            Command::Spectrum(mode) => self.spectrum = mode,
            Command::ClearLoop(src) => self.core.loops[src.index()] = None,
            Command::SeekBy(src, delta) => {
                self.core.source = src;
                self.core.seek(
                    self.core.position.saturating_add_signed(delta),
                    self.core.playing,
                );
            }
            Command::Volume(value) => {
                self.core.volume = value.clamp(0., 1.);
                self.volume.update(cx, |slider, cx| {
                    slider.set_value(self.core.volume, window, cx)
                });
                self.core.apply_gain();
            }
            Command::DismissError => {
                self.core.error = None;
                self.core.warning = None;
            }
        }
        if self.modal != previous_modal {
            self.modal_error = None;
            self.gesture = None;
            if self.modal.is_some() {
                if previous_modal.is_none() {
                    self.return_focus = opener;
                }
                match self.modal {
                    Some(Modal::Group(_)) => self
                        .group_name
                        .update(cx, |input, cx| input.focus(window, cx)),
                    Some(Modal::EditTrack(_)) => {
                        if self.edit_track_focus_next_action {
                            self.next_action_input
                                .update(cx, |input, cx| input.focus(window, cx));
                        } else {
                            self.track_title_input
                                .update(cx, |input, cx| input.focus(window, cx));
                        }
                    }
                    _ => self.modal_start.focus(window),
                }
            } else if previous_modal.is_some() {
                if self.mode == previous_mode && !completed_confirmation {
                    self.return_focus
                        .take()
                        .unwrap_or_else(|| self.focus.clone())
                        .focus(window);
                } else {
                    self.return_focus = None;
                    self.focus.focus(window);
                }
            }
        }
        if self.note_draft.is_some() && validation {
            self.note_error = self.core.error.clone();
        }
        if self
            .note_draft
            .as_ref()
            .is_some_and(|d| d.generation != self.core.generation)
            || self.mode != Mode::Review
        {
            self.note_draft = None;
            self.note_dragging = false;
        }
        if self.modal.is_some() && (validation || self.core.error != previous_error) {
            self.modal_error = self.core.error.clone();
        }
        window.refresh();
        cx.notify();
    }
    fn reference_label(&self, path: &std::path::Path) -> String {
        self.core
            .library
            .reference_tracks
            .iter()
            .find(|r| r.path == path)
            .and_then(|r| r.display_name.clone())
            .unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
    }
    fn scroll_handle(&self, id: &'static str) -> ScrollHandle {
        self.scroll_handles
            .borrow_mut()
            .entry(id)
            .or_default()
            .clone()
    }
    fn scroll_scope(&self, id: &'static str) -> ScrollScope<'_> {
        self.scroll_context
            .borrow_mut()
            .push((id, self.scroll_handle(id)));
        ScrollScope(&self.scroll_context)
    }
    fn focus_target(
        &self,
        id: ElementId,
        make_handle: impl FnOnce() -> FocusHandle,
    ) -> FocusTarget {
        let mut targets = self.targets.borrow_mut();
        let target = targets.entry(id).or_insert_with(|| FocusTarget {
            handle: make_handle(),
            bounds: Rc::new(Cell::new(Bounds::default())),
            scrolls: Vec::new(),
            seen: 0,
            in_modal: false,
        });
        target.seen = self.focus_frame.get();
        let scrolls = self.scroll_context.borrow();
        target.scrolls = scrolls.iter().map(|(_, handle)| handle.clone()).collect();
        target.in_modal = scrolls.iter().any(|(id, _)| *id == "modal-body");
        target.clone()
    }
    fn watched_input(&self, input: &Entity<InputState>, cx: &mut Context<Self>) -> Div {
        let id = ElementId::from(("input-watch", input.entity_id()));
        let target = self.focus_target(id, || input.read(cx).focus_handle(cx));
        let bounds = target.bounds;
        div().relative().w_full().child(Input::new(input)).child(
            div()
                .absolute()
                .inset_0()
                .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).size_full()),
        )
    }
    fn key_control(&self, id: impl Into<ElementId>, cx: &mut Context<Self>) -> Stateful<Div> {
        let id = id.into();
        let target =
            self.focus_target(id.clone(), || cx.focus_handle().tab_index(0).tab_stop(true));
        let bounds = target.bounds;
        div()
            .id(id)
            .relative()
            .track_focus(&target.handle)
            .key_context("CadenceControl")
            .focus(|d| {
                d.bg(rgb(SURFACE_ALT)).shadow(vec![BoxShadow {
                    color: rgb(ACCENT).into(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(1.),
                }])
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).size_full()),
            )
    }
    fn traverse(&mut self, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        if reverse {
            window.focus_prev();
        } else {
            window.focus_next();
        }
        let in_modal = self
            .targets
            .borrow()
            .values()
            .find(|t| t.handle.is_focused(window))
            .is_some_and(|t| t.in_modal);
        if self.modal.is_some() && !in_modal && !self.modal_focus.contains_focused(window, cx) {
            if reverse {
                self.modal_end.focus(window);
                window.focus_prev();
            } else {
                self.modal_start.focus(window);
                window.focus_next();
            }
        }
        self.reveal_focus(window);
        window.refresh();
        cx.notify();
    }
    fn reveal_focus(&self, window: &mut Window) {
        let target = self
            .targets
            .borrow()
            .values()
            .find(|t| t.handle.is_focused(window))
            .cloned();
        if let Some(target) = target {
            let mut bounds = target.bounds.get();
            for scroll in target.scrolls.iter().rev() {
                let viewport = scroll.bounds();
                if viewport.size.height <= px(0.) {
                    continue;
                }
                let delta = if bounds.top() < viewport.top() {
                    viewport.top() - bounds.top()
                } else if bounds.bottom() > viewport.bottom() {
                    viewport.bottom() - bounds.bottom()
                } else {
                    px(0.)
                };
                let before = scroll.offset();
                scroll.set_offset(point(before.x, before.y + delta));
                bounds.origin.y += scroll.offset().y - before.y;
            }
        }
    }
    fn page_scroll(&self, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self
            .targets
            .borrow()
            .values()
            .find(|t| t.handle.is_focused(window))
            && let Some(scroll) = target.scrolls.last()
        {
            let offset = scroll.offset();
            let step = scroll.bounds().size.height * if reverse { 0.8 } else { -0.8 };
            scroll.set_offset(point(offset.x, offset.y + step));
            cx.notify();
        }
    }
    #[allow(clippy::too_many_arguments)] // Shared geometry and visual states mirror Frame's control primitive.
    fn control(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        cmd: Command,
        width: f32,
        active: bool,
        primary: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label: SharedString = label.into();
        let dropdown = label.ends_with("  ⌄");
        let text = label.trim_end_matches("  ⌄").to_uppercase();
        let height = match &cmd {
            Command::TagGroupMenu | Command::ManagerAlbums | Command::ManagerStageMenu(false) => {
                34.
            }
            Command::SaveTag => 30.,
            Command::ManagerApply | Command::ManagerRemove => 32.,
            Command::Comments(_) => 26.,
            Command::Import(ImportKind::Main) if width == 164. => 32.,
            _ => 28.,
        };
        self.key_control(id, cx)
            .h(px(height))
            .flex_shrink_0()
            .px(px(12.))
            .flex()
            .items_center()
            .justify_center()
            .when(width > 0., |d| d.w(px(width)))
            .font_family(MONO)
            .text_size(px(10.))
            .line_height(px(12.))
            .rounded(px(0.))
            .bg(rgb(if primary {
                ACCENT
            } else if active {
                ACTIVE_BG
            } else {
                SURFACE
            }))
            .border_1()
            .border_color(rgb(if active || primary { ACCENT } else { BORDER }))
            .text_color(rgb(if primary {
                TEXT_BRIGHT
            } else if active {
                ACCENT_TEXT
            } else {
                TEXT
            }))
            .cursor_pointer()
            .hover(move |d| {
                if primary {
                    d.bg(rgb(ACCENT_HOVER)).border_color(rgb(ACCENT_HOVER))
                } else if active {
                    d.bg(rgb(ACTIVE_HOVER))
                } else {
                    d.bg(rgb(SURFACE_HOVER)).border_color(rgb(BORDER_STRONG))
                }
            })
            .tooltip({
                let label = label.clone();
                move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                }
            })
            .child(div().min_w_0().truncate().child(text))
            .when(dropdown, |d| {
                d.gap(px(8.)).child(
                    gpui_component::Icon::new(IconName::ChevronDown)
                        .size(px(10.))
                        .flex_shrink_0(),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| this.act(cmd.clone(), window, cx)))
            .into_any_element()
    }
    fn button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        cmd: Command,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.control(id, label, cmd, 0., false, false, cx)
    }
    fn icon_button(
        &self,
        id: impl Into<ElementId>,
        icon: IconName,
        tip: &'static str,
        cmd: Command,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.key_control(id, cx)
            .size(px(20.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(MUTED))
            .cursor_pointer()
            .hover(|d| d.text_color(rgb(TEXT_BRIGHT)))
            .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip).build(window, cx))
            .child(gpui_component::Icon::new(icon).size(px(14.)))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.act(cmd.clone(), window, cx);
            }))
            .into_any_element()
    }
    fn comment_source_button(&self, src: Source, cx: &mut Context<Self>) -> AnyElement {
        let active = self.comment_source == src;
        let main = src == Source::Main;
        self.key_control(
            if main {
                "notes-main"
            } else {
                "notes-reference"
            },
            cx,
        )
        .w(px(32.))
        .h(px(26.))
        .rounded(px(0.))
        .border_1()
        .border_color(rgb(if active { ACCENT } else { BORDER }))
        .bg(rgb(if active { ACTIVE_BG } else { SURFACE }))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(rgb(if active { ACCENT_TEXT } else { MUTED }))
        .hover(move |d| {
            if active {
                d.bg(rgb(ACTIVE_HOVER))
            } else {
                d.bg(rgb(SURFACE_HOVER))
                    .border_color(rgb(BORDER_STRONG))
                    .text_color(rgb(TEXT))
            }
        })
        .child(
            gpui_component::Icon::default()
                .path(if main {
                    "cadence/comments-main.svg"
                } else {
                    "cadence/comments-reference.svg"
                })
                .size(px(16.)),
        )
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(if main {
                "Main comments"
            } else {
                "Reference comments"
            })
            .build(window, cx)
        })
        .on_click(
            cx.listener(move |this, _, window, cx| this.act(Command::Comments(src), window, cx)),
        )
        .into_any_element()
    }
    fn chip(&self, tag: &Tag) -> AnyElement {
        let _album = self
            .core
            .library
            .tags
            .groups
            .iter()
            .any(|g| g.id == tag.group && g.album);
        div()
            .id(SharedString::from(format!("chip-{}", tag.id)))
            .tooltip({
                let name = tag.name.clone();
                move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(name.clone()).build(window, cx)
                }
            })
            .relative()
            .pl(px(9.))
            .pr(px(7.))
            .h(px(18.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .font_family(MONO)
            .text_size(px(10.))
            .bg(tint(tag.color, 0.08))
            .text_color(rgb(tag.color))
            .border_1()
            .border_color(tint(tag.color, 0.35))
            .rounded(px(0.))
            .max_w(px(110.))
            .truncate()
            .child(tag.name.clone())
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(rgb(tag.color)),
            )
            .into_any_element()
    }
    fn cards_list(
        &self,
        id: impl Into<ElementId>,
        ids: Vec<String>,
        stage: Option<TrackStage>,
        cx: &mut Context<Self>,
    ) -> UniformList {
        let count = ids.len();
        uniform_list(
            id,
            count + usize::from(stage.is_some()),
            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                let width = match this.mode {
                    Mode::Review => 224.,
                    Mode::Planner => (f32::from(window.viewport_size().width) - 160.) / 4. - 16.,
                    _ => 328.,
                };
                range
                    .filter_map(|slot| {
                        let mut row = div().id(slot).h(px(132.)).pb_2();
                        if let Some(stage) = stage {
                            row = row.on_drop(cx.listener(move |this, drag: &TrackDrag, _, cx| {
                                this.core.stage(&drag.id, stage, slot.min(count));
                                cx.stop_propagation();
                                cx.notify();
                            }));
                        }
                        if let Some(id) = ids.get(slot) {
                            let track = this.core.library.tracks.iter().find(|t| &t.id == id)?;
                            row = row.child(this.track_card(track, width, cx));
                        }
                        if stage.is_some() && slot == count {
                            row = row.child(div().mt(px(12.)).h(px(1.)).bg(rgb(BORDER_SUBTLE)));
                        }
                        Some(row)
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .w_full()
    }
    fn track_card(&self, track: &Track, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let id = track.id.clone();
        let selected = if self.mode == Mode::Tracks {
            self.manager_selected.contains(&id)
        } else {
            self.core.library.selected_track_id.as_ref() == Some(&id)
        };
        let drag = TrackDrag {
            id: id.clone(),
            title: track.title.clone(),
        };
        let bounce_badge = track.current_bounce_badge();
        let missing = track.is_source_missing();
        let has_collision = !track.collisions.is_empty();
        let explicit_next_action = !track.next_action.trim().is_empty();
        let next_action_text = track.display_next_action();
        let active_rev_id = track.active_revision_id.clone();
        let assigned = self.core.library.tags.for_track(&id);
        // The chip uses a 10px monospace font, 16px padding, and a 2px border.
        let widths = assigned
            .iter()
            .map(|t| (t.name.chars().count() as f32 * 6.1 + 18.).min(110.))
            .collect::<Vec<_>>();
        let visible = fitting_chips((width - 48.).max(0.), &widths);
        let mut tags = assigned
            .iter()
            .take(visible)
            .map(|t| self.chip(t))
            .collect::<Vec<_>>();
        if visible < assigned.len() {
            let labels = assigned[visible..]
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            tags.push(
                div()
                    .id(SharedString::from(format!("extra-labels-{id}")))
                    .h(px(18.))
                    .flex_shrink_0()
                    .font_family(MONO)
                    .text_size(px(10.))
                    .line_height(px(18.))
                    .text_color(rgb(MUTED))
                    .child(format!("+{}", assigned.len() - visible))
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(labels.clone()).build(window, cx)
                    })
                    .into_any_element(),
            );
        }
        div()
            .id(SharedString::from(format!("card-{id}")))
            .w_full()
            .h(px(124.))
            .relative()
            .cursor_pointer()
            .when(selected, |d| {
                d.child(div().absolute().inset_0().child(corner_frame(ACCENT)))
            })
            .flex_shrink_0()
            .bg(rgb(if selected { ACTIVE_BG } else { SURFACE }))
            .border_1()
            .border_color(rgb(if selected {
                ACCENT_DIM
            } else if missing || has_collision {
                ACCENT
            } else {
                BORDER
            }))
            .hover(move |d| {
                if selected {
                    d.bg(rgb(ACTIVE_HOVER))
                } else {
                    d.bg(rgb(SURFACE_HOVER)).border_color(rgb(BORDER_STRONG))
                }
            })
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, window, cx| {
                    this.act(
                        if this.mode == Mode::Tracks {
                            Command::ManagerToggle(id.clone())
                        } else {
                            Command::Select(id.clone())
                        },
                        window,
                        cx,
                    )
                }
            }))
            .when(selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(rgb(ACCENT)),
                )
            })
            .child(
                div()
                    .absolute()
                    .left(px(10.))
                    .top(px(12.))
                    .w(px(12.))
                    .h(px(18.))
                    .text_size(px(10.))
                    .text_color(rgb(DIM))
                    .cursor_move()
                    .child("⠿")
                    .id(SharedString::from(format!("drag-{id}")))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone())),
            )
            .child(
                div()
                    .w_full()
                    .h(px(30.))
                    .pt(px(12.))
                    .pl(px(28.))
                    .pr(px(if self.mode == Mode::Tracks { 36. } else { 10. }))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        self.key_control(SharedString::from(format!("title-{id}")), cx)
                            .flex_1()
                            .min_w_0()
                            .h(px(18.))
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(rgb(if selected { TEXT_BRIGHT } else { TEXT }))
                            .truncate()
                            .cursor_pointer()
                            .tooltip({
                                let title = track.title.clone();
                                move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(title.clone())
                                        .build(window, cx)
                                }
                            })
                            .child(track.title.clone())
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.act(
                                        if this.mode == Mode::Tracks {
                                            Command::ManagerToggle(id.clone())
                                        } else {
                                            Command::Select(id.clone())
                                        },
                                        window,
                                        cx,
                                    )
                                }
                            })),
                    )
                    .when(missing, |d| {
                        d.child(
                            div()
                                .px(px(5.))
                                .h(px(16.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .bg(rgb(ACCENT))
                                .text_color(rgb(TEXT_BRIGHT))
                                .font_family(MONO)
                                .text_size(px(9.))
                                .child("MISSING"),
                        )
                    })
                    .when(!missing && has_collision, |d| {
                        d.child(
                            div()
                                .px(px(5.))
                                .h(px(16.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .bg(rgb(ACTIVE_BG))
                                .border_1()
                                .border_color(rgb(ACCENT))
                                .text_color(rgb(ACCENT_TEXT))
                                .font_family(MONO)
                                .text_size(px(9.))
                                .child("!COL"),
                        )
                    })
                    .child(
                        div()
                            .px(px(5.))
                            .h(px(16.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .bg(rgb(WELL))
                            .border_1()
                            .border_color(rgb(BORDER_STRONG))
                            .text_color(rgb(TEXT))
                            .font_family(MONO)
                            .text_size(px(9.))
                            .child(bounce_badge),
                    ),
            )
            .when(self.mode == Mode::Tracks, |d| {
                d.child(
                    self.key_control(SharedString::from(format!("manager-select-{id}")), cx)
                        // Keep the selected fill visible while this checkbox has focus.
                        .focus(|style| {
                            style.shadow(vec![BoxShadow {
                                color: rgb(ACCENT).into(),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(0.),
                                spread_radius: px(1.),
                            }])
                        })
                        .absolute()
                        .right(px(14.))
                        .top(px(14.))
                        .size(px(14.))
                        .border_1()
                        .border_color(rgb(if selected { ACCENT } else { BORDER_STRONG }))
                        .bg(rgb(if selected { ACCENT } else { WELL }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .when(selected, |d| {
                            d.child(
                                gpui_component::Icon::new(IconName::Check)
                                    .size(px(12.))
                                    .text_color(rgb(TEXT_BRIGHT)),
                            )
                        })
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.act(Command::ManagerToggle(id.clone()), window, cx)
                            }
                        })),
                )
            })
            .child(
                self.key_control(SharedString::from(format!("next-action-{id}")), cx)
                    .absolute()
                    .left(px(12.))
                    .right(px(10.))
                    .top(px(36.))
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .font_family(MONO)
                    .text_size(px(10.))
                    .line_height(px(16.))
                    .text_color(rgb(if explicit_next_action {
                        TEXT
                    } else if next_action_text.is_some() {
                        MUTED
                    } else {
                        DIM
                    }))
                    .truncate()
                    .cursor_pointer()
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new(
                            "Click to edit title, next action, or build feedback",
                        )
                        .build(window, cx)
                    })
                    .child(match next_action_text {
                        Some(action) => format!("→ {action}"),
                        None => String::from("→ + Set next action"),
                    })
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.act(Command::EditTrack(id.clone(), true), window, cx);
                        }
                    })),
            )
            .child(
                div()
                    .absolute()
                    .left(px(12.))
                    .right(px(36.))
                    .top(px(62.))
                    .h(px(18.))
                    .flex()
                    .gap(px(6.))
                    .overflow_hidden()
                    .children(tags),
            )
            .child(
                div()
                    .absolute()
                    .right(px(7.))
                    .top(px(61.))
                    .child(self.icon_button(
                        SharedString::from(format!("label-{id}")),
                        IconName::Plus,
                        "Assign tags",
                        Command::Assign(id.clone()),
                        cx,
                    )),
            )
            .child(
                self.key_control(SharedString::from(format!("stage-{id}")), cx)
                    .absolute()
                    .left(px(12.))
                    .top(px(92.))
                    .h(px(18.))
                    .font_family(MONO)
                    .text_size(px(10.))
                    .line_height(px(18.))
                    .text_color(rgb(MUTED))
                    .cursor_pointer()
                    .child(format!("{}  ⌄", track.stage.label().to_uppercase()))
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.act(Command::StageMenu(id.clone()), window, cx)
                        }
                    })),
            )
            .child(
                div()
                    .absolute()
                    .right(px(7.))
                    .top(px(91.))
                    .flex()
                    .gap(px(3.))
                    .child(if missing {
                        self.key_control(SharedString::from(format!("relink-{id}")), cx)
                            .px(px(5.))
                            .h(px(20.))
                            .bg(rgb(ACCENT))
                            .text_color(rgb(TEXT_BRIGHT))
                            .font_family(MONO)
                            .text_size(px(9.))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(
                                    "Relink moved audio file in place",
                                )
                                .build(window, cx)
                            })
                            .child("RELINK")
                            .on_click(cx.listener({
                                let id = id.clone();
                                let active_rev_id = active_rev_id.clone();
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.act(
                                        Command::Import(ImportKind::Relink(
                                            id.clone(),
                                            active_rev_id.clone(),
                                        )),
                                        window,
                                        cx,
                                    );
                                }
                            }))
                            .into_any_element()
                    } else {
                        self.icon_button(
                            SharedString::from(format!("add-bounce-{id}")),
                            IconName::Redo,
                            "Add bounce revision",
                            Command::Import(ImportKind::AddRevision(id.clone())),
                            cx,
                        )
                    })
                    .child(
                        self.key_control(SharedString::from(format!("favorite-{id}")), cx)
                            .size(px(20.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(if track.favorite { ACCENT_TEXT } else { MUTED }))
                            .cursor_pointer()
                            .hover(|d| d.text_color(rgb(TEXT_BRIGHT)))
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new("Favorite").build(window, cx)
                            })
                            .child(
                                gpui_component::Icon::new(if track.favorite {
                                    IconName::Star
                                } else {
                                    IconName::StarOff
                                })
                                .size(px(14.)),
                            )
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.act(Command::Favorite(id.clone()), window, cx)
                                }
                            })),
                    )
                    .child(self.icon_button(
                        SharedString::from(format!("remove-{id}")),
                        IconName::Close,
                        "Remove track",
                        Command::RemoveTrack(id),
                        cx,
                    )),
            )
            .into_any_element()
    }
    fn waveform(&self, src: Source, cx: &mut Context<Self>) -> AnyElement {
        let waveform = self.core.waveform(src).cloned();
        let position = self.core.position;
        let mut notes = self.core.notes(src);
        if let Some(draft) = &self.note_draft
            && draft.source == src
            && draft.generation == self.core.generation
        {
            if let Some(note) = notes.iter_mut().find(|n| Some(&n.id) == draft.id.as_ref()) {
                note.time_millis = draft.time;
            } else if draft.id.is_none() {
                notes.push(crate::storage::Note {
                    id: String::new(),
                    time_millis: draft.time,
                    body: String::new(),
                    done: false,
                });
            }
        }
        let loop_range = self.core.loops[src.index()];
        let hover_state = self.waveform_hover[src.index()];
        let duration_millis = waveform.as_ref().map_or(0, |w| w.duration_millis);
        let bounds = self.waveform_bounds[src.index()].clone();
        let hover_badge = match hover_state {
            Some(WaveformHover::Seek(hr)) if duration_millis > 0 => {
                let b = bounds.get();
                let w = f32::from(b.size.width);
                if w > 0. {
                    let hx = hr.clamp(0., 1.) * w;
                    let badge_w = 42.;
                    let left = if hx + 6. + badge_w <= w - 4. {
                        hx + 6.
                    } else {
                        (hx - 6. - badge_w).max(4.)
                    };
                    let time_ms = (hr.clamp(0., 1.) * duration_millis as f32) as u64;
                    Some(
                        div()
                            .absolute()
                            .left(px(left))
                            .top(px(4.))
                            .h(px(15.))
                            .px(px(4.))
                            .flex()
                            .items_center()
                            .bg(rgb(SURFACE))
                            .border_1()
                            .border_color(rgb(BORDER_STRONG))
                            .font_family(MONO)
                            .text_size(px(9.))
                            .line_height(px(11.))
                            .text_color(rgb(TEXT_BRIGHT))
                            .child(timestamp(time_ms)),
                    )
                } else {
                    None
                }
            }
            _ => None,
        };
        let paint_bounds = bounds.clone();
        let bars_cache = self.waveform_bars[src.index()].clone();
        let surface = canvas(
            move |b, _, _| {
                paint_bounds.set(b);
            },
            move |b, _, window, _| {
                let width = f32::from(b.size.width);
                let height = f32::from(b.size.height);
                let x = f32::from(b.origin.x);
                let y = f32::from(b.origin.y);
                let rail_offset = waveform_rail_offset(height);
                let rail = y + rail_offset;
                let upper_max = (rail_offset - 6.).max(1.);
                let lower_max = (height - rail_offset - 6.).max(1.);
                window.paint_quad(fill(
                    Bounds::new(b.origin, size(b.size.width, px(rail_offset))),
                    rgb(WELL),
                ));
                window.paint_quad(fill(
                    Bounds::new(
                        point(px(x), px(rail)),
                        size(b.size.width, px((height - rail_offset).max(0.))),
                    ),
                    rgb(WAVE_LOWER_BG),
                ));
                for i in 0..=8 {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(px(x + width * i as f32 / 8.), px(y)),
                            size(px(1.), px(height)),
                        ),
                        rgb(BORDER_SUBTLE),
                    ));
                }
                if let Some(wave) = waveform {
                    let progress = if wave.duration_millis > 0 {
                        position as f32 / wave.duration_millis as f32
                    } else {
                        0.
                    };
                    let count = waveform_bar_count(width);
                    let mut cache = bars_cache.borrow_mut();
                    let stale = cache.as_ref().is_none_or(|cached| {
                        !std::sync::Arc::ptr_eq(&cached.summary, &wave.summary)
                            || cached.count != count
                    });
                    if stale {
                        *cache = Some(WaveformBarsCache {
                            summary: std::sync::Arc::clone(&wave.summary),
                            count,
                            bars: waveform_bars(&wave.summary, count),
                        });
                    }
                    let bars = &cache.as_ref().expect("populated above").bars;
                    let pitch = width / count as f32;
                    let bar_w = (pitch * 0.55).clamp(2., 3.).floor();
                    for (i, bar) in bars.iter().enumerate() {
                        let bx = x + i as f32 * pitch + (pitch - bar_w) * 0.5;
                        let played = (i as f32 + 0.5) / count as f32 <= progress;

                        // Upper 2/3 waveform (grows upward from the comment rail).
                        let peak_up = (bar.peak * upper_max).max(2.);
                        window.paint_quad(fill(
                            Bounds::new(
                                point(px(bx), px(rail - peak_up)),
                                size(px(bar_w), px(peak_up)),
                            ),
                            rgb(if played {
                                WAVE_PEAK_PLAYED
                            } else {
                                WAVE_PEAK_UNPLAYED
                            }),
                        ));
                        if bar.rms > 0.005 {
                            let rms_up = (bar.rms * upper_max).clamp(2., peak_up);
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(px(bx), px(rail - rms_up)),
                                    size(px(bar_w), px(rms_up)),
                                ),
                                rgb(if played { ACCENT } else { WAVE_UNPLAYED }),
                            ));
                        }

                        // Lower 1/3 darkened reflection (grows downward beneath the comment rail).
                        let peak_down = (bar.peak * lower_max).max(1.);
                        window.paint_quad(fill(
                            Bounds::new(
                                point(px(bx), px(rail + 1.)),
                                size(px(bar_w), px(peak_down)),
                            ),
                            rgb(if played {
                                WAVE_LOWER_PEAK_PLAYED
                            } else {
                                WAVE_LOWER_PEAK_UNPLAYED
                            }),
                        ));
                        if bar.rms > 0.005 {
                            let rms_down = (bar.rms * lower_max).clamp(1., peak_down);
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(px(bx), px(rail + 1.)),
                                    size(px(bar_w), px(rms_down)),
                                ),
                                rgb(if played {
                                    WAVE_LOWER_PLAYED
                                } else {
                                    WAVE_LOWER_UNPLAYED
                                }),
                            ));
                        }
                    }
                    if let Some((a, z)) = loop_range {
                        let loop_x = x + a * width;
                        let loop_w = (z - a) * width;
                        window.paint_quad(fill(
                            Bounds::new(point(px(loop_x), px(y)), size(px(loop_w), px(height))),
                            tint(ACCENT, 0.14),
                        ));
                        window.paint_quad(fill(
                            Bounds::new(point(px(loop_x), px(y)), size(px(1.), px(height))),
                            rgb(ACCENT_DIM),
                        ));
                        window.paint_quad(fill(
                            Bounds::new(
                                point(px(loop_x + loop_w), px(y)),
                                size(px(1.), px(height)),
                            ),
                            rgb(ACCENT_DIM),
                        ));
                    }
                    window.paint_quad(fill(
                        Bounds::new(point(px(x), px(rail)), size(px(width), px(1.))),
                        rgb(BORDER_STRONG),
                    ));
                    let head_x = x + progress.clamp(0., 1.) * width;
                    window.paint_quad(fill(
                        Bounds::new(point(px(head_x), px(y)), size(px(1.), px(height))),
                        rgb(ACCENT),
                    ));
                    window.paint_quad(fill(
                        Bounds::new(point(px(head_x - 2.), px(y)), size(px(5.), px(3.))),
                        rgb(ACCENT),
                    ));
                    for note in notes {
                        let ratio = note.time_millis as f32 / wave.duration_millis.max(1) as f32;
                        let xx = x + ratio * width;
                        let mut path = PathBuilder::fill();
                        path.move_to(point(px(xx), px(rail)));
                        path.line_to(point(px(xx + 4.5), px(rail + 7.)));
                        path.line_to(point(px(xx - 4.5), px(rail + 7.)));
                        path.line_to(point(px(xx), px(rail)));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, rgb(if note.done { DIM } else { ACCENT }));
                        }
                    }
                    match hover_state {
                        Some(WaveformHover::Seek(hr)) => {
                            let hx = x + hr.clamp(0., 1.) * width;
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(px(hx), px(y)),
                                    size(px(1.), px(rail_offset.max(0.))),
                                ),
                                rgba(0xffffffaa),
                            ));
                            let mut path = PathBuilder::fill();
                            path.move_to(point(px(hx - 4.), px(y)));
                            path.line_to(point(px(hx + 4.), px(y)));
                            path.line_to(point(px(hx), px(y + 5.)));
                            path.line_to(point(px(hx - 4.), px(y)));
                            if let Ok(path) = path.build() {
                                window.paint_path(path, rgb(TEXT_BRIGHT));
                            }
                        }
                        Some(WaveformHover::Comment(hr)) => {
                            let hx = x + hr.clamp(0., 1.) * width;
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(px(hx), px(rail + 1.)),
                                    size(px(1.), px((height - rail_offset - 1.).max(0.))),
                                ),
                                rgba(0xffffff28),
                            ));
                            let mut path = PathBuilder::fill();
                            path.move_to(point(px(hx), px(rail)));
                            path.line_to(point(px(hx + 4.5), px(rail + 7.)));
                            path.line_to(point(px(hx - 4.5), px(rail + 7.)));
                            path.line_to(point(px(hx), px(rail)));
                            if let Ok(path) = path.build() {
                                window.paint_path(path, rgb(TEXT_BRIGHT));
                            }
                        }
                        None => {}
                    }
                } else {
                    window.paint_quad(fill(
                        Bounds::new(point(px(x), px(rail)), size(px(width), px(1.))),
                        rgb(BORDER_STRONG),
                    ));
                }
            },
        )
        .size_full();
        let waveform_focus = self
            .focus_target(
                ElementId::from(if src == Source::Main {
                    "main-waveform"
                } else {
                    "reference-waveform"
                }),
                || cx.focus_handle().tab_index(0).tab_stop(true),
            )
            .handle;
        self.key_control(
            if src == Source::Main {
                "main-waveform"
            } else {
                "reference-waveform"
            },
            cx,
        )
        .on_click(cx.listener(move |this, event, window, cx| {
            if matches!(event, ClickEvent::Keyboard(_)) {
                this.act(Command::Play(src), window, cx);
            }
        }))
        .key_context("CadenceControl CadenceWaveform")
        .when(self.note_draft.is_none(), |d| {
            d.tooltip(|window, cx| {
                gpui_component::tooltip::Tooltip::new(
                    "Upper waveform: seek or Shift-drag to loop · Lower rail: click to comment",
                )
                .build(window, cx)
            })
        })
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if !hovered && this.waveform_hover[src.index()].take().is_some() {
                cx.notify();
            }
        }))
        .on_action(cx.listener(move |this, _: &SeekForward, window, cx| {
            cx.stop_propagation();
            this.act(Command::SeekBy(src, 1000), window, cx);
        }))
        .on_action(cx.listener(move |this, _: &SeekBackward, window, cx| {
            cx.stop_propagation();
            this.act(Command::SeekBy(src, -1000), window, cx);
        }))
        .h(px(136.))
        .w_full()
        .overflow_hidden()
        .border_1()
        .border_color(rgb(BORDER))
        .rounded(px(0.))
        .cursor_crosshair()
        .child(surface)
        .children(hover_badge)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                waveform_focus.focus(window);
                let b = this.waveform_bounds[src.index()].get();
                let ratio = ((event.position.x - b.origin.x) / b.size.width).clamp(0., 1.);
                let time = (ratio * this.core.duration(src) as f32) as u64;
                if is_comment_zone(
                    f32::from(event.position.y - b.origin.y),
                    f32::from(b.size.height),
                ) {
                    if this.note_draft.as_ref().is_some_and(|d| {
                        d.source == src
                            && (d.time as f32 / this.core.duration(src).max(1) as f32 - ratio).abs()
                                < 0.008
                    }) {
                        this.note_dragging = true;
                        cx.notify();
                    } else if let Some(note) = this.core.notes(src).into_iter().find(|n| {
                        (n.time_millis as f32 / this.core.duration(src).max(1) as f32 - ratio).abs()
                            < 0.008
                    }) {
                        this.act(Command::EditNote(src, note.id), window, cx);
                    } else {
                        this.act(Command::NewNote(src, time), window, cx);
                    }
                    this.note_dragging = true;
                } else {
                    this.gesture = Some((src, ratio, event.modifiers.shift));
                    this.core.source = src;
                    this.core.seek(time, false);
                    cx.notify();
                }
                cx.stop_propagation();
            }),
        )
        .into_any_element()
    }
    fn source_panel(&self, src: Source, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let reference = src == Source::Reference;
        let compact = f32::from(window.viewport_size().width) < 1280.;
        let ready = self.core.waveform(src).is_some();
        let active = self.core.source == src;
        let duration = self.core.duration(src);
        let lufs_value = self
            .core
            .waveform(src)
            .and_then(|w| w.integrated_lufs)
            .filter(|v| v.is_finite());
        let lufs = lufs_value
            .map(|v| format!("{v:.1}"))
            .unwrap_or_else(|| "—".into());
        let lit_segments = lufs_lit_segments(lufs_value);
        let missing = self.core.missing_source[src.index()];
        let waveform = if ready {
            self.waveform(src, cx)
        } else if !self.core.analyzing[src.index()] && missing {
            let mut well = div()
                .h(px(136.))
                .px(px(16.))
                .bg(rgb(WELL))
                .border_1()
                .border_color(rgb(ACCENT))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.));
            if !reference {
                if let Some(track) = self.core.selected().cloned() {
                    let missing_path = track.path.display().to_string();
                    well = well
                        .child(
                            div()
                                .font_family(MONO)
                                .text_size(px(11.))
                                .text_color(rgb(ACCENT_TEXT))
                                .child("MISSING AUDIO SOURCE"),
                        )
                        .child(
                            div()
                                .max_w_full()
                                .truncate()
                                .font_family(MONO)
                                .text_size(px(10.))
                                .text_color(rgb(MUTED))
                                .child(missing_path),
                        )
                        .child(self.control(
                            "relink-main-source",
                            "Relink audio file",
                            Command::Import(ImportKind::Relink(
                                track.id,
                                track.active_revision_id,
                            )),
                            0.,
                            false,
                            true,
                            cx,
                        ));
                }
            } else if let Some(ref_path) = self
                .core
                .selected()
                .and_then(|t| t.reference_path.clone())
            {
                let missing_path = ref_path.display().to_string();
                well = well
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(rgb(ACCENT_TEXT))
                            .child("MISSING REFERENCE SOURCE"),
                    )
                    .child(
                        div()
                            .max_w_full()
                            .truncate()
                            .font_family(MONO)
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(missing_path),
                    )
                    .child(self.control(
                        "relink-reference-source",
                        "Relink reference",
                        Command::Import(ImportKind::ReplaceReference(ref_path)),
                        0.,
                        false,
                        true,
                        cx,
                    ));
            }
            well.into_any_element()
        } else {
            div()
                .h(px(136.))
                .bg(rgb(WELL))
                .border_1()
                .border_color(rgb(BORDER))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED))
                .child(if self.core.analyzing[src.index()] {
                    "Analyzing audio…"
                } else if reference {
                    "Choose a reference"
                } else {
                    "Waveform unavailable"
                })
                .into_any_element()
        };
        let mut panel = div()
            .relative()
            .w_full()
            .h(px(if reference { 240. } else { 256. }))
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .absolute()
                    .left(px(8.))
                    .top(px(16.))
                    .child(caption(if reference { "Reference" } else { "Main" })),
            )
            .child(
                self.key_control(
                    if reference {
                        "reference-play"
                    } else {
                        "main-play"
                    },
                    cx,
                )
                .absolute()
                .left(px(8.))
                .top(px(94.))
                .size(px(32.))
                .bg(rgb(if active { ACCENT } else { SURFACE }))
                .border_1()
                .border_color(rgb(if active { ACCENT } else { BORDER }))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(if active { TEXT_BRIGHT } else { TEXT }))
                .cursor_pointer()
                .hover(move |d| {
                    if active {
                        d.bg(rgb(ACCENT_HOVER)).border_color(rgb(ACCENT_HOVER))
                    } else {
                        d.bg(rgb(SURFACE_HOVER)).border_color(rgb(BORDER_STRONG))
                    }
                })
                .tooltip({
                    let playing = active && self.core.playing;
                    move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(match (reference, playing) {
                            (false, false) => "Play main track",
                            (false, true) => "Pause main track",
                            (true, false) => "Play reference",
                            (true, true) => "Pause reference",
                        })
                        .build(window, cx)
                    }
                })
                .child(if active && self.core.playing {
                    "Ⅱ"
                } else {
                    "▶"
                })
                .on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.act(Command::Play(src), window, cx)
                    }),
                ),
            )
            .child(
                div()
                    .absolute()
                    .left(px(64.))
                    .right(px(96.))
                    .top(px(56.))
                    .child(waveform)
                    .child(
                        div()
                            .mt(px(12.))
                            .flex()
                            .justify_between()
                            .font_family(MONO)
                            .text_size(px(9.))
                            .text_color(rgb(MUTED))
                            .children((0..=6).map(|i| div().child(timestamp(duration * i / 6)))),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .right(px(0.))
                    .top(px(56.))
                    .w(px(80.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.))
                    .child(caption("LUFS"))
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(24.))
                            .line_height(px(28.))
                            .text_color(rgb(TEXT_BRIGHT))
                            .child(lufs),
                    )
                    .child(div().flex().gap(px(1.5)).children((0..14).map(|i| {
                        let color = if i < lit_segments {
                            if i >= 9 { ACCENT } else { TEXT }
                        } else {
                            BORDER_SUBTLE
                        };
                        div().w(px(2.)).h(px(10.)).bg(rgb(color))
                    }))),
            );
        if !reference {
            panel = panel
                .child(
                    div()
                        .absolute()
                        .left(px(88.))
                        .right(px(if compact { 320. } else { 440. }))
                        .top(px(14.))
                        .truncate()
                        .text_size(px(13.))
                        .text_color(rgb(TEXT_BRIGHT))
                        .child(
                            self.core
                                .selected()
                                .map(|t| t.title.clone())
                                .unwrap_or_default(),
                        ),
                )
                .when(!compact, |d| {
                    d.child(
                        div()
                            .absolute()
                            .right(px(456.))
                            .top(px(16.))
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(format!(
                                "{} / {}",
                                timestamp(self.core.position),
                                timestamp(duration)
                            )),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .right(px(16.))
                        .top(px(10.))
                        .flex()
                        .gap(px(8.))
                        .child(self.control(
                            "spectrum-none",
                            "None",
                            Command::Spectrum(0),
                            62.,
                            self.spectrum == 0,
                            false,
                            cx,
                        ))
                        .child(self.control(
                            "spectrum-on",
                            "Spectrum",
                            Command::Spectrum(1),
                            100.,
                            self.spectrum == 1,
                            false,
                            cx,
                        ))
                        .child(self.control(
                            "waterfall-on",
                            "Spectrogram",
                            Command::Spectrum(2),
                            114.,
                            self.spectrum == 2,
                            false,
                            cx,
                        )),
                );
        }
        if self.core.loops[src.index()].is_some() {
            panel = panel.child(
                div()
                    .absolute()
                    .left(px(64.))
                    .top(px(224.))
                    .child(self.button(
                        if reference {
                            "clear-ref-loop"
                        } else {
                            "clear-main-loop"
                        },
                        "Clear loop",
                        Command::ClearLoop(src),
                        cx,
                    )),
            );
        }
        panel.into_any_element()
    }
    fn live_spectrum(&self, window: &mut Window) -> AnyElement {
        let source = self.core.source.index();
        let (_, frame) = self.core.transports[source].live_frame_snapshot();
        if self.spectrum == 2 {
            #[cfg(target_os = "macos")]
            let surface = {
                let mut gpu = self.spectrogram_gpu.borrow_mut();
                let gpu = gpu.get_or_insert_with(Default::default);
                gpu.poll(source, frame.as_deref());
                gpu.surface()
            };
            let use_gpu = {
                #[cfg(target_os = "macos")]
                {
                    surface.is_some()
                }
                #[cfg(not(target_os = "macos"))]
                {
                    false
                }
            };
            let gpu_failed = {
                #[cfg(target_os = "macos")]
                {
                    self.spectrogram_gpu
                        .borrow()
                        .as_ref()
                        .is_some_and(|gpu| gpu.is_failed())
                }
                #[cfg(not(target_os = "macos"))]
                {
                    true
                }
            };
            let image = if !use_gpu || gpu_failed {
                let mut cache = self.spectrogram_image.borrow_mut();
                if let Some(retired) = cache.update(frame.as_deref()) {
                    let _ = window.drop_image(retired);
                }
                cache.image()
            } else {
                let mut cache = self.spectrogram_image.borrow_mut();
                if let Some(retired) = cache.update(None) {
                    let _ = window.drop_image(retired);
                }
                None
            };
            let sample_rate = frame.as_ref().map_or(48_000, |f| f.sample_rate);
            let maximum = (sample_rate / 2).min(20_000);
            let history_seconds = crate::transport::LIVE_SPECTROGRAM_MAX_HISTORY as f32
                * crate::transport::LIVE_SPECTRUM_HOP_SIZE as f32
                / sample_rate as f32;
            return div()
                .relative()
                .h(px(136.))
                .flex_shrink_0()
                .w_full()
                .bg(rgb(WELL))
                .border_1()
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .absolute()
                        .left(px(56.))
                        .right(px(12.))
                        .top(px(16.))
                        .bottom(px(16.))
                        .child(
                            canvas(
                                |_, _, _| (),
                                move |b, _, window, _| {
                                    #[cfg(target_os = "macos")]
                                    if let Some(surface) = surface {
                                        window.paint_surface(b, surface);
                                    }
                                    if let Some(image) = image {
                                        let _ = window.paint_image(
                                            b,
                                            Corners::default(),
                                            image,
                                            0,
                                            false,
                                        );
                                    }
                                },
                            )
                            .size_full(),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(6.))
                        .top(px(16.))
                        .font_family(MONO)
                        .text_size(px(9.))
                        .text_color(rgb(MUTED))
                        .child(format!("{}k Hz", maximum / 1_000)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(6.))
                        .bottom(px(16.))
                        .font_family(MONO)
                        .text_size(px(9.))
                        .text_color(rgb(MUTED))
                        .child("20 Hz"),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(56.))
                        .bottom(px(0.))
                        .font_family(MONO)
                        .text_size(px(9.))
                        .text_color(rgb(MUTED))
                        .child(format!("−{history_seconds:.1}s")),
                )
                .child(
                    div()
                        .absolute()
                        .right(px(12.))
                        .bottom(px(0.))
                        .font_family(MONO)
                        .text_size(px(9.))
                        .text_color(rgb(MUTED))
                        .child("Now"),
                )
                .child(
                    div()
                        .absolute()
                        .right(px(12.))
                        .top(px(0.))
                        .flex()
                        .items_center()
                        .gap_1()
                        .font_family(MONO)
                        .text_size(px(9.))
                        .text_color(rgb(MUTED))
                        .child("Quiet")
                        .children((0..9).map(|i| {
                            let [b, g, r, _] =
                                crate::live_visual::magnitude_color((i * 255 / 8) as u8);
                            div()
                                .w(px(12.))
                                .h(px(6.))
                                .bg(rgb((r as u32) << 16 | (g as u32) << 8 | b as u32))
                        }))
                        .child("Loud"),
                )
                .into_any_element();
        }
        canvas(
            |_, _, _| (),
            move |b, _, window, _| {
                window.paint_quad(fill(b, rgb(WELL)));
                let x = f32::from(b.origin.x);
                let y = f32::from(b.origin.y);
                let w = f32::from(b.size.width);
                let h = f32::from(b.size.height);
                for i in 1..4 {
                    window.paint_quad(fill(
                        Bounds::new(point(px(x), px(y + h * i as f32 / 4.)), size(px(w), px(1.))),
                        rgb(BORDER_SUBTLE),
                    ));
                }
                let Some(frame) = frame else {
                    return;
                };
                for (i, value) in frame.spectrum_values.iter().enumerate() {
                    let height = *value as f32 / 255. * h;
                    if height <= 0.5 {
                        continue;
                    }
                    let bar_x = x + i as f32 * w / frame.spectrum_values.len() as f32;
                    let bar_w = w / frame.spectrum_values.len() as f32 + 0.3;
                    window.paint_quad(fill(
                        Bounds::new(point(px(bar_x), px(y + h - height)), size(px(bar_w), px(height))),
                        rgb(ACCENT),
                    ));
                    if height >= 3. {
                        window.paint_quad(fill(
                            Bounds::new(point(px(bar_x), px(y + h - height)), size(px(bar_w), px(2.))),
                            rgb(TEXT_BRIGHT),
                        ));
                    }
                }
            },
        )
        .h(px(120.))
        .w_full()
        .border_1()
        .border_color(rgb(BORDER))
        .into_any_element()
    }
    fn comments_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let _scroll_scope = self.scroll_scope("comments-scroll");
        let src = self.comment_source;
        let mut rows = Vec::new();
        for (i, note) in self.core.notes(src).into_iter().enumerate() {
            let id = note.id.clone();
            let time = note.time_millis;
            rows.push(
                div()
                    .id(SharedString::from(format!("note-{id}")))
                    .relative()
                    .flex()
                    .items_center()
                    .pb(px(6.))
                    .h(px(48.))
                    .w_full()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(rgb(BORDER_SUBTLE))
                    .bg(rgb(if i % 2 == 0 { SURFACE } else { BG }))
                    .child(
                        self.key_control(SharedString::from(format!("resolve-{id}")), cx)
                            .absolute()
                            .left(px(8.))
                            .top(px(11.))
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(if note.done {
                                    "Reopen comment"
                                } else {
                                    "Resolve comment"
                                })
                                .build(window, cx)
                            })
                            .when(note.done, |d| {
                                d.child(
                                    gpui_component::Icon::new(IconName::Check)
                                        .size(px(14.))
                                        .text_color(rgb(DIM)),
                                )
                            })
                            .when(!note.done, |d| d.child(div().size(px(5.)).bg(rgb(ACCENT))))
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, window, cx| {
                                    this.act(Command::ResolveNote(src, id.clone()), window, cx)
                                }
                            })),
                    )
                    .child(
                        self.key_control(SharedString::from(format!("time-{id}")), cx)
                            .absolute()
                            .left(px(40.))
                            .top(px(13.))
                            .font_family(MONO)
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(rgb(MUTED))
                            .cursor_pointer()
                            .child(timestamp(time))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.act(Command::PlayNote(src, time), window, cx)
                            })),
                    )
                    .child(
                        self.key_control(SharedString::from(format!("note-body-{id}")), cx)
                            .ml(px(120.))
                            .mr(px(80.))
                            .flex_1()
                            .min_w_0()
                            .h(px(18.))
                            .line_height(px(18.))
                            .text_size(px(13.))
                            .truncate()
                            .text_color(rgb(if note.done { MUTED } else { TEXT }))
                            .cursor_pointer()
                            .child(note.body.replace(['\n', '\r'], " "))
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, window, cx| {
                                    this.act(Command::EditNote(src, id.clone()), window, cx)
                                }
                            })),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(46.))
                            .top(px(11.))
                            .child(self.icon_button(
                                SharedString::from(format!("play-note-{id}")),
                                IconName::ArrowRight,
                                "Play comment",
                                Command::PlayNote(src, time),
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(6.))
                            .top(px(11.))
                            .child(self.icon_button(
                                SharedString::from(format!("delete-note-{id}")),
                                IconName::Close,
                                "Delete comment",
                                Command::DeleteNote(src, id),
                                cx,
                            )),
                    )
                    .into_any_element(),
            );
        }
        if !self.core.provenance_known(src) && self.core.waveform(src).is_some() {
            rows.push(self.button(
                "bind-comments",
                "Bind historical comments",
                Command::Bind(src),
                cx,
            ));
        }
        div()
            .flex_1()
            .min_h(px(140.))
            .border_t_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .h(px(60.))
                    .flex_shrink_0()
                    .child(
                        div()
                            .absolute()
                            .left(px(8.))
                            .top(px(18.))
                            .child(caption(match src {
                                Source::Main => match self.core.selected() {
                                    Some(track) if track.is_viewing_prior_build() => format!(
                                        "Comments · {} (prior build)",
                                        track.active_build_label()
                                    ),
                                    Some(track) => {
                                        format!("Comments · {}", track.active_build_label())
                                    }
                                    None => String::from("Comments"),
                                },
                                Source::Reference => String::from("Comments · Reference"),
                            })),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(48.))
                            .top(px(10.))
                            .flex()
                            .gap(px(8.))
                            .child(self.comment_source_button(Source::Main, cx))
                            .child(self.comment_source_button(Source::Reference, cx)),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(6.))
                            .top(px(14.))
                            .child(self.icon_button(
                                "add-comment",
                                IconName::Plus,
                                "New comment",
                                Command::NewNote(src, self.core.position),
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .id("comments-scroll")
                    .track_scroll(&self.scroll_handle("comments-scroll"))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
    fn track_revision_strip(&self, track: &Track, cx: &mut Context<Self>) -> AnyElement {
        let id = track.id.clone();
        let active_rev_id = track.active_revision_id.clone();
        let current_rev_id = track.current_revision_id.clone();
        let current_label = track.current_build_label();
        let active_label = track.active_build_label();
        let viewing_prior = track.is_viewing_prior_build();
        let explicit_next_action = !track.next_action.trim().is_empty();
        let next_action = track.display_next_action();
        let active_feedback = track
            .active_revision()
            .and_then(|rev| rev.feedback_note.clone());
        let can_assign_to_track =
            track.group_key.is_none() && self.core.library.tracks.len() > 1;
        let missing = track.is_source_missing();

        let header_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .when_some(track.effective_group_key(), |d, key| {
                        d.child(
                            div()
                                .px(px(6.))
                                .h(px(22.))
                                .flex()
                                .items_center()
                                .bg(rgb(WELL))
                                .border_1()
                                .border_color(rgb(BORDER_STRONG))
                                .font_family(MONO)
                                .text_size(px(10.))
                                .text_color(rgb(TEXT_BRIGHT))
                                .child(key),
                        )
                    })
                    .child(self.control(
                        "review-stage-menu",
                        format!("{}  ⌄", track.stage.label()),
                        Command::StageMenu(id.clone()),
                        0.,
                        false,
                        false,
                        cx,
                    ))
                    .child(
                        self.key_control("review-next-action", cx)
                            .h(px(28.))
                            .px(px(10.))
                            .max_w(px(360.))
                            .bg(rgb(SURFACE))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .hover(|d| d.bg(rgb(SURFACE_HOVER)).border_color(rgb(BORDER_STRONG)))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .cursor_pointer()
                            .font_family(MONO)
                            .text_size(px(10.))
                            .text_color(rgb(if explicit_next_action { TEXT } else { MUTED }))
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(
                                    "Edit title, next action, or who heard this build",
                                )
                                .build(window, cx)
                            })
                            .child(div().text_color(rgb(ACCENT_TEXT)).child("NEXT:"))
                            .child(div().min_w_0().truncate().child(match next_action {
                                Some(action) => action,
                                None => String::from("Set next action"),
                            }))
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, window, cx| {
                                    this.act(Command::EditTrack(id.clone(), true), window, cx)
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(self.button(
                        "review-edit-details",
                        "Edit details",
                        Command::EditTrack(id.clone(), false),
                        cx,
                    ))
                    .when(can_assign_to_track, |d| {
                        d.child(self.button(
                            "review-assign-track",
                            "Assign to track",
                            Command::AssignToTrackMenu(id.clone()),
                            cx,
                        ))
                    })
                    .when(missing, |d| {
                        d.child(self.control(
                            "review-relink-top",
                            "Relink audio",
                            Command::Import(ImportKind::Relink(
                                id.clone(),
                                active_rev_id.clone(),
                            )),
                            0.,
                            false,
                            true,
                            cx,
                        ))
                    })
                    .child(self.control(
                        "review-add-bounce",
                        "+ Bounce",
                        Command::Import(ImportKind::AddRevision(id.clone())),
                        0.,
                        false,
                        false,
                        cx,
                    )),
            );

        let mut rev_pills = Vec::new();
        if track.revisions.is_empty() {
            rev_pills.push(
                div()
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .bg(rgb(ACTIVE_BG))
                    .border_1()
                    .border_color(rgb(ACCENT))
                    .font_family(MONO)
                    .text_size(px(10.))
                    .text_color(rgb(TEXT_BRIGHT))
                    .child(format!("{current_label} ● CURRENT"))
                    .into_any_element(),
            );
        } else {
            for (i, rev) in track.revisions.iter().enumerate() {
                let is_active = active_rev_id.as_deref() == Some(rev.id.as_str());
                let is_current = current_rev_id.as_deref() == Some(rev.id.as_str());
                let mut label = rev.label.clone();
                if is_current {
                    label.push_str(" ●");
                }
                if !rev.notes.is_empty() {
                    label.push_str(&format!(" ({})", rev.notes.len()));
                }
                let rev_id = rev.id.clone();
                let rev_name = rev.original_name.clone();
                let track_id = id.clone();
                rev_pills.push(
                    self.key_control(("revision-pill", i), cx)
                        .h(px(24.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .bg(rgb(if is_active { ACTIVE_BG } else { SURFACE }))
                        .border_1()
                        .border_color(rgb(if is_active { ACCENT } else { BORDER }))
                        .text_color(rgb(if is_active { TEXT_BRIGHT } else { MUTED }))
                        .font_family(MONO)
                        .text_size(px(10.))
                        .cursor_pointer()
                        .hover(move |d| {
                            if is_active {
                                d.bg(rgb(ACTIVE_HOVER))
                            } else {
                                d.bg(rgb(SURFACE_HOVER))
                                    .border_color(rgb(BORDER_STRONG))
                                    .text_color(rgb(TEXT))
                            }
                        })
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(rev_name.clone())
                                .build(window, cx)
                        })
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.act(
                                Command::SelectRevision(track_id.clone(), rev_id.clone()),
                                window,
                                cx,
                            )
                        }))
                        .into_any_element(),
                );
            }
        }

        let mut rev_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(caption("Bounces"))
            .children(rev_pills);

        if viewing_prior
            && let Some(cur_id) = current_rev_id.clone()
        {
            rev_row = rev_row
                .child(
                    div()
                        .ml(px(4.))
                        .font_family(MONO)
                        .text_size(px(10.))
                        .text_color(rgb(ACCENT_TEXT))
                        .child(format!(
                            "VIEWING PRIOR {active_label} · CURRENT IS {current_label}"
                        )),
                )
                .child(self.control(
                    "jump-current-bounce",
                    format!("Open {current_label}"),
                    Command::SelectRevision(id.clone(), cur_id),
                    0.,
                    false,
                    false,
                    cx,
                ));
        }

        if let Some(feedback) = active_feedback {
            rev_row = rev_row.child(
                div()
                    .ml(px(6.))
                    .px(px(6.))
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .bg(rgb(WELL))
                    .border_1()
                    .border_color(rgb(BORDER_STRONG))
                    .font_family(MONO)
                    .text_size(px(10.))
                    .text_color(rgb(TEXT))
                    .child(format!("BUILD NOTE: {feedback}")),
            );
        }

        let mut strip = div()
            .w_full()
            .flex_shrink_0()
            .pb(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(header_row)
            .child(rev_row);

        for (ci, col) in track.collisions.iter().enumerate() {
            let col_id = col.id.clone();
            let tid = id.clone();
            strip = strip.child(
                div()
                    .id(("collision-banner", ci))
                    .p(px(10.))
                    .bg(rgb(ACTIVE_BG))
                    .border_1()
                    .border_color(rgb(ACCENT))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(10.))
                            .text_color(rgb(TEXT_BRIGHT))
                            .child(format!(
                                "COLLISION ON {}: {} differs from existing build — original kept",
                                col.label.to_uppercase(),
                                col.original_name
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(self.button(
                                ("collision-keep", ci),
                                "Keep existing",
                                Command::ResolveCollisionKeep(tid.clone(), col_id.clone()),
                                cx,
                            ))
                            .child(self.button(
                                ("collision-replace", ci),
                                format!("Replace {}", col.label),
                                Command::ResolveCollisionReplace(tid.clone(), col_id.clone()),
                                cx,
                            ))
                            .child(self.button(
                                ("collision-split", ci),
                                "Separate track",
                                Command::ResolveCollisionSplit(tid, col_id),
                                cx,
                            )),
                    ),
            );
        }

        strip.into_any_element()
    }
    fn review(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ids = self
            .core
            .library
            .tracks
            .iter()
            .map(|t| t.id.clone())
            .collect::<Vec<_>>();
        let cards = self
            .cards_list("library-scroll", ids, None, cx)
            .track_scroll(self.library_scroll.clone());
        let library = div()
            .w(px(273.))
            .flex_shrink_0()
            .pl(px(16.))
            .pr(px(32.))
            .border_r_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
            .rounded(px(0.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .h(px(40.))
                    .flex_shrink_0()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(caption("Library"))
                    .child(
                        self.key_control("library-manager", cx)
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new("Track Manager")
                                    .build(window, cx)
                            })
                            .size(px(28.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(SURFACE))
                            .text_color(rgb(TEXT))
                            .hover(|d| d.bg(rgb(SURFACE_HOVER)).border_color(rgb(BORDER_STRONG)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .child(
                                gpui_component::Icon::default()
                                    .path("cadence/add-plus.svg")
                                    .size(px(14.)),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.act(Command::Mode(Mode::Tracks), window, cx)
                            })),
                    ),
            )
            .child(cards.flex_1().min_h_0());
        let _scroll_scope = self.scroll_scope("review-workspace");
        let mut workspace = div()
            .id("review-workspace")
            .track_scroll(&self.scroll_handle("review-workspace"))
            .overflow_y_scroll()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_0();
        if let Some(selected_track) = self.core.selected().cloned() {
            workspace = workspace.child(self.track_revision_strip(&selected_track, cx));
            if self.spectrum > 0 {
                workspace = workspace.child(self.live_spectrum(window));
            }
            workspace = workspace
                .child(self.source_panel(Source::Main, window, cx))
                .child(self.source_panel(Source::Reference, window, cx))
                .child(self.comments_panel(cx));
        } else {
            workspace = workspace.relative().child(
                div()
                    .absolute()
                    .left(px(64.))
                    .right(px(96.))
                    .top(px(168.))
                    .h(px(280.))
                    .bg(rgb(SURFACE))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .flex_col()
                    .items_center()
                    .pt(px(88.))
                    .child(div().absolute().inset_0().child(corner_frame(ACCENT_DIM)))
                    .child(caption("Your review desk"))
                    .child(
                        div()
                            .mt(px(20.))
                            .h(px(30.))
                            .text_size(px(22.))
                            .line_height(px(30.))
                            .text_color(rgb(TEXT_BRIGHT))
                            .child("Start with a track."),
                    )
                    .child(div().mt(px(40.)).child(self.control(
                        "empty-import",
                        "+ Import audio",
                        Command::Import(ImportKind::Main),
                        164.,
                        false,
                        true,
                        cx,
                    ))),
            );
        }
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .gap(px(31.))
            .child(library)
            .child(workspace)
            .into_any_element()
    }
    fn planner(&self, cx: &mut Context<Self>) -> AnyElement {
        let stages = [
            TrackStage::Backlog,
            TrackStage::Production,
            TrackStage::Mixdown,
            TrackStage::Mastering,
        ];
        let mut columns = Vec::new();
        for (i, stage) in stages.into_iter().enumerate() {
            let ids = storage::planner_tracks(&self.core.library)
                .into_iter()
                .filter(|t| t.stage == stage)
                .map(|t| t.id.clone())
                .collect::<Vec<_>>();
            let count = ids.len();
            let cards = self.cards_list(
                SharedString::from(format!("column-scroll-{i}")),
                ids,
                Some(stage),
                cx,
            );
            columns.push(
                div()
                    .id(("planner-column", i))
                    .relative()
                    .when(i < 3, |d| {
                        d.child(
                            div()
                                .absolute()
                                .right(px(-16.))
                                .top_0()
                                .bottom_0()
                                .w(px(1.))
                                .bg(rgb(BORDER_SUBTLE)),
                        )
                    })
                    .flex_1()
                    .min_w_0()
                    .bg(rgb(BG))
                    .flex()
                    .flex_col()
                    .gap(px(24.))
                    .child(
                        div()
                            .h(px(40.))
                            .flex_shrink_0()
                            .pl(px(16.))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(caption(stage.label())),
                    )
                    .child(
                        div()
                            .px(px(8.))
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(cards.flex_1().min_h_0()),
                    )
                    .on_drop(cx.listener(move |this, drag: &TrackDrag, _, cx| {
                        this.core.stage(&drag.id, stage, count);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .gap(px(32.))
            .children(columns)
            .into_any_element()
    }
    fn manager_ids(&self, cx: &Context<Self>) -> Vec<String> {
        let mut filter = self.manager_filter.clone();
        filter.query = self.track_search.read(cx).value().to_string();
        crate::track_manager::filtered_ids(&self.core.library, &filter)
    }
    fn track_manager(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let sidebar_scope = self.scroll_scope("manager-sidebar-scroll");
        let all = !self.manager_filter.favorites
            && self.manager_filter.tag.is_none()
            && self.manager_filter.group.is_none();
        let mut sidebar = div()
            .w(px(273.))
            .flex_shrink_0()
            .pl(px(16.))
            .pr(px(16.))
            .border_r_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(40.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .child(caption("Library")),
            )
            .child(
                self.key_control("manager-all", cx)
                    .mt(px(12.))
                    .h(px(36.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .font_family(MONO)
                    .text_size(px(12.))
                    .text_color(rgb(if all { TEXT_BRIGHT } else { MUTED }))
                    .cursor_pointer()
                    .bg(rgb(if all { ACTIVE_BG } else { BG }))
                    .when(all, |d| d.border_l_2().border_color(rgb(ACCENT)))
                    .hover(move |d| {
                        if all {
                            d.bg(rgb(ACTIVE_HOVER))
                        } else {
                            d.bg(rgb(SURFACE)).text_color(rgb(TEXT))
                        }
                    })
                    .child("All tracks")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.act(Command::ManagerFilter(None, None, false), window, cx)
                    })),
            )
            .child(
                self.key_control("manager-favorites", cx)
                    .mt(px(8.))
                    .h(px(32.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .font_family(MONO)
                    .text_size(px(12.))
                    .text_color(rgb(if self.manager_filter.favorites {
                        TEXT_BRIGHT
                    } else {
                        MUTED
                    }))
                    .bg(rgb(if self.manager_filter.favorites {
                        ACTIVE_BG
                    } else {
                        BG
                    }))
                    .when(self.manager_filter.favorites, |d| {
                        d.border_l_2().border_color(rgb(ACCENT))
                    })
                    .hover({
                        let fav = self.manager_filter.favorites;
                        move |d| {
                            if fav {
                                d.bg(rgb(ACTIVE_HOVER))
                            } else {
                                d.bg(rgb(SURFACE)).text_color(rgb(TEXT))
                            }
                        }
                    })
                    .cursor_pointer()
                    .child("Favorites")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.act(Command::ManagerFilter(None, None, true), window, cx)
                    })),
            )
            .child(div().mt(px(16.)).h(px(1.)).bg(rgb(BORDER)));
        let mut groups = self.core.library.tags.groups.iter().collect::<Vec<_>>();
        groups.sort_by_key(|g| !g.album);
        for (index, group) in groups.into_iter().enumerate() {
            sidebar =
                sidebar.child(
                    div()
                        .relative()
                        .h(px(55.))
                        .flex_shrink_0()
                        .child(
                            self.key_control(
                                SharedString::from(format!("manager-group-{}", group.id)),
                                cx,
                            )
                            .absolute()
                            .left(px(16.))
                            .top(px(23.))
                            .cursor_pointer()
                            .child(caption(group.name.clone()))
                            .on_click(cx.listener({
                                let id = group.id.clone();
                                move |this, _, window, cx| {
                                    this.act(
                                        Command::ManagerFilter(None, Some(id.clone()), false),
                                        window,
                                        cx,
                                    )
                                }
                            })),
                        )
                        .child(div().absolute().right(px(6.)).top(px(21.)).child(
                            self.icon_button(
                                SharedString::from(format!("manager-edit-group-{}", group.id)),
                                IconName::Ellipsis,
                                "Edit group",
                                Command::EditGroup(group.id.clone()),
                                cx,
                            ),
                        )),
                );
            for tag in self
                .core
                .library
                .tags
                .tags
                .iter()
                .filter(|t| t.group == group.id)
            {
                sidebar = sidebar.child(
                    div().h(px(40.)).flex_shrink_0().pl(px(16.)).child(
                        self.key_control(
                            SharedString::from(format!("manager-filter-{}", tag.id)),
                            cx,
                        )
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .child(self.chip(tag))
                        .on_click(cx.listener({
                            let id = tag.id.clone();
                            move |this, _, window, cx| {
                                this.act(
                                    Command::ManagerFilter(Some(id.clone()), None, false),
                                    window,
                                    cx,
                                )
                            }
                        })),
                    ),
                );
            }
            if index == 0 {
                sidebar = sidebar
                    .child(
                        self.key_control(("manager-new-group", index), cx)
                            .h(px(28.))
                            .mt(px(2.))
                            .pl(px(16.))
                            .font_family(MONO)
                            .text_size(px(11.))
                            .line_height(px(28.))
                            .text_color(rgb(MUTED))
                            .cursor_pointer()
                            .child("+ New group")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.act(Command::NewGroup, window, cx)
                            })),
                    )
                    .child(div().h(px(1.)).mt(px(18.)).bg(rgb(BORDER)));
            }
        }
        drop(sidebar_scope);
        let ids = self.manager_ids(cx);
        let count = ids.len();
        let all_visible_selected =
            count > 0 && ids.iter().all(|id| self.manager_selected.contains(id));
        let columns = if f32::from(window.viewport_size().width) < 1280. {
            1
        } else {
            2
        };
        let cards = uniform_list(
            "manager-track-scroll",
            count.div_ceil(columns),
            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                let width =
                    (f32::from(window.viewport_size().width) - 760. - 24. * (columns - 1) as f32)
                        / columns as f32;
                range
                    .map(|row| {
                        let mut line = div().w_full().h(px(128.)).flex().gap(px(24.));
                        for col in 0..columns {
                            let mut cell = div().flex_1().min_w_0();
                            if let Some(id) = ids.get(row * columns + col)
                                && let Some(track) =
                                    this.core.library.tracks.iter().find(|t| &t.id == id)
                            {
                                cell = cell.child(this.track_card(track, width, cx));
                            }
                            line = line.child(cell);
                        }
                        line
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0();
        let center = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .h(px(40.))
                    .flex_shrink_0()
                    .pl(px(8.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(caption("All tracks"))
                    .child(self.control(
                        "manager-import",
                        "+ Import tracks",
                        Command::Import(ImportKind::Main),
                        160.,
                        false,
                        true,
                        cx,
                    )),
            )
            .child(
                div()
                    .mx(px(8.))
                    .h(px(32.))
                    .mt(px(4.))
                    .child(Input::new(&self.track_search).h(px(32.))),
            )
            .child(
                div()
                    .h(px(28.))
                    .mx(px(8.))
                    .mt(px(-2.))
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        self.key_control("manager-select-all", cx)
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .cursor_pointer()
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(
                                div()
                                    .size(px(14.))
                                    .border_1()
                                    .border_color(rgb(if all_visible_selected {
                                        ACCENT
                                    } else {
                                        BORDER_STRONG
                                    }))
                                    .bg(rgb(if all_visible_selected { ACCENT } else { WELL }))
                                    .when(all_visible_selected, |d| {
                                        d.child(
                                            gpui_component::Icon::new(IconName::Check)
                                                .size(px(12.))
                                                .text_color(rgb(TEXT_BRIGHT)),
                                        )
                                    }),
                            )
                            .child("Select all")
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(format!(
                                    "Select the {count} visible tracks"
                                ))
                                .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.act(Command::ManagerSelectAll, window, cx)
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                self.button(
                                    "manager-stage-filter",
                                    self.manager_filter
                                        .stage
                                        .map(|s| s.label())
                                        .unwrap_or("All stages"),
                                    Command::ManagerStageMenu(true),
                                    cx,
                                ),
                            )
                            .child(self.button(
                                "manager-sort",
                                if self.manager_filter.reverse {
                                    "Sort: Z–A"
                                } else {
                                    "Sort: A–Z"
                                },
                                Command::ManagerSort,
                                cx,
                            )),
                    ),
            )
            .when(count == 0, |d| {
                d.child(
                    div()
                        .p_6()
                        .text_color(rgb(MUTED))
                        .child("No matching tracks"),
                )
            })
            .child(
                div()
                    .mt(px(6.))
                    .pr(px(8.))
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(cards),
            );
        let _editor_scope = self.scroll_scope("manager-editor-scroll");
        let selected = self
            .manager_selected
            .iter()
            .filter(|id| self.core.library.tracks.iter().any(|t| &t.id == *id))
            .count();
        let mut editor = div()
            .w(px(360.))
            .flex_shrink_0()
            .pl(px(32.))
            .border_l_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(40.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(caption(format!("{selected} selected")))
                    .child(self.control(
                        "manager-clear",
                        "Clear",
                        Command::ManagerClear,
                        80.,
                        false,
                        false,
                        cx,
                    )),
            );
        if selected > 0 {
            let album = self
                .core
                .library
                .tags
                .tags
                .iter()
                .find(|t| {
                    self.core
                        .library
                        .tags
                        .groups
                        .iter()
                        .any(|g| g.id == t.group && g.album)
                        && (self.manager_edit.add.contains(&t.id)
                            || (!self.manager_edit.remove.contains(&t.id)
                                && self.manager_selected.iter().all(|id| {
                                    self.core
                                        .library
                                        .tags
                                        .for_track(id)
                                        .iter()
                                        .any(|tag| tag.id == t.id)
                                })))
                })
                .map(|t| t.name.clone())
                .unwrap_or_else(|| "Mixed".into());
            let stages = self
                .core
                .library
                .tracks
                .iter()
                .filter(|t| self.manager_selected.contains(&t.id))
                .map(|t| t.stage)
                .collect::<Vec<_>>();
            let stage = self
                .manager_edit
                .stage
                .or_else(|| {
                    stages
                        .first()
                        .copied()
                        .filter(|s| stages.iter().all(|t| t == s))
                })
                .map(|s| s.label())
                .unwrap_or("Mixed");
            let mut tag_chips = Vec::new();
            for tag in &self.core.library.tags.tags {
                if self
                    .core
                    .library
                    .tags
                    .groups
                    .iter()
                    .any(|g| g.id == tag.group && g.album)
                {
                    continue;
                }
                if self.manager_edit.add.contains(&tag.id)
                    || (!self.manager_edit.remove.contains(&tag.id)
                        && self.manager_selected.iter().any(|id| {
                            self.core
                                .library
                                .tags
                                .for_track(id)
                                .iter()
                                .any(|t| t.id == tag.id)
                        }))
                {
                    tag_chips.push(self.chip(tag));
                }
            }
            editor =
                editor.child(
                    div()
                        .relative()
                        .min_h(px(480.))
                        .flex_shrink_0()
                        .child(div().absolute().top(px(28.)).child(caption("Group")))
                        .child(div().absolute().left_0().right_0().top(px(52.)).child(
                            self.control(
                                "manager-album",
                                format!("{album}  ⌄"),
                                Command::ManagerAlbums,
                                328.,
                                false,
                                false,
                                cx,
                            ),
                        ))
                        .child(
                            self.key_control("manager-create-group", cx)
                                .absolute()
                                .top(px(100.))
                                .font_family(MONO)
                                .text_size(px(11.))
                                .text_color(rgb(MUTED))
                                .cursor_pointer()
                                .child("+ New group")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.act(Command::NewGroup, window, cx)
                                })),
                        )
                        .child(div().absolute().top(px(148.)).child(caption("Stage")))
                        .child(div().absolute().left_0().right_0().top(px(172.)).child(
                            self.control(
                                "manager-stage",
                                format!("{stage}  ⌄"),
                                Command::ManagerStageMenu(false),
                                328.,
                                false,
                                false,
                                cx,
                            ),
                        ))
                        .child(div().absolute().top(px(240.)).child(caption("Tags")))
                        .child(div().absolute().right_0().top(px(234.)).child(self.control(
                            "manager-edit-tags",
                            "+ Add tags",
                            Command::ManagerTags,
                            112.,
                            false,
                            false,
                            cx,
                        )))
                        .child(
                            div()
                                .pt(px(280.))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .min_h(px(48.))
                                        .flex()
                                        .flex_wrap()
                                        .gap(px(12.))
                                        .children(tag_chips),
                                )
                                .child(div().h(px(1.)).bg(rgb(BORDER)))
                                .child(
                                    div()
                                        .mt(px(23.))
                                        .opacity(if self.manager_edit.is_empty() {
                                            0.4
                                        } else {
                                            1.
                                        })
                                        .child(self.control(
                                            "manager-apply",
                                            "Apply changes",
                                            Command::ManagerApply,
                                            328.,
                                            false,
                                            true,
                                            cx,
                                        )),
                                )
                                .child(div().mt(px(48.)).pb(px(16.)).child(self.control(
                                    "manager-remove",
                                    "Remove tracks",
                                    Command::ManagerRemove,
                                    328.,
                                    false,
                                    false,
                                    cx,
                                ))),
                        ),
                );
        }
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .gap(px(24.))
            .child(
                div()
                    .id("manager-sidebar-scroll")
                    .track_scroll(&self.scroll_handle("manager-sidebar-scroll"))
                    .w(px(280.))
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .child(sidebar),
            )
            .child(center)
            .child(
                div()
                    .id("manager-editor-scroll")
                    .track_scroll(&self.scroll_handle("manager-editor-scroll"))
                    .overflow_y_scroll()
                    .child(editor),
            )
            .into_any_element()
    }
    fn tags(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let compact = f32::from(window.viewport_size().width) < 1280.;
        let sidebar_scope = self.scroll_scope("tags-sidebar-scroll");
        let book = &self.core.library.tags;
        let all_tags_active = self.group_filter.is_none();
        let mut groups = vec![
            self.key_control("all-tags", cx)
                .h(px(36.))
                .my(px(4.))
                .px(px(16.))
                .flex()
                .items_center()
                .cursor_pointer()
                .bg(rgb(if all_tags_active { ACTIVE_BG } else { BG }))
                .when(all_tags_active, |d| {
                    d.border_l_2().border_color(rgb(ACCENT))
                })
                .font_family(MONO)
                .text_size(px(12.))
                .text_color(rgb(if all_tags_active {
                    TEXT_BRIGHT
                } else {
                    MUTED
                }))
                .hover(move |d| {
                    if all_tags_active {
                        d.bg(rgb(ACTIVE_HOVER))
                    } else {
                        d.bg(rgb(SURFACE)).text_color(rgb(TEXT))
                    }
                })
                .child("All tags")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.act(Command::TagsGroup(None), window, cx)
                }))
                .into_any_element(),
        ];
        for group in &book.groups {
            let active = self.group_filter.as_ref() == Some(&group.id);
            groups.push(
                div()
                    .relative()
                    .h(px(44.))
                    .flex_shrink_0()
                    .child(
                        self.key_control(SharedString::from(format!("group-{}", group.id)), cx)
                            .absolute()
                            .inset_0()
                            .my(px(4.))
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .bg(rgb(if active { ACTIVE_BG } else { BG }))
                            .when(active, |d| d.border_l_2().border_color(rgb(ACCENT)))
                            .font_family(MONO)
                            .text_size(px(12.))
                            .text_color(rgb(if active { TEXT_BRIGHT } else { MUTED }))
                            .hover(move |d| {
                                if active {
                                    d.bg(rgb(ACTIVE_HOVER))
                                } else {
                                    d.bg(rgb(SURFACE)).text_color(rgb(TEXT))
                                }
                            })
                            .cursor_pointer()
                            .child(group.name.clone())
                            .on_click(cx.listener({
                                let id = group.id.clone();
                                move |this, _, window, cx| {
                                    this.act(Command::TagsGroup(Some(id.clone())), window, cx)
                                }
                            })),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(6.))
                            .top(px(12.))
                            .child(self.icon_button(
                                SharedString::from(format!("edit-group-{}", group.id)),
                                IconName::Ellipsis,
                                "Edit group",
                                Command::EditGroup(group.id.clone()),
                                cx,
                            )),
                    )
                    .into_any_element(),
            );
        }
        let sidebar =
            div()
                .w(px(273.))
                .flex_shrink_0()
                .pl(px(16.))
                .pr(px(16.))
                .border_r_1()
                .border_color(rgb(BORDER))
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(48.))
                        .relative()
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(px(12.))
                                .child(caption("Groups")),
                        )
                        .child(div().absolute().right(px(6.)).top(px(10.)).child(
                            self.icon_button(
                                "new-group",
                                IconName::Plus,
                                "New group",
                                Command::NewGroup,
                                cx,
                            ),
                        )),
                )
                .children(groups)
                .child(
                    div()
                        .mt(px(24.))
                        .pt(px(16.))
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .child(self.button("tags-new-group", "+ New group", Command::NewGroup, cx)),
                );
        let heading = self
            .group_filter
            .as_ref()
            .and_then(|id| book.groups.iter().find(|g| &g.id == id))
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "All tags".into());
        drop(sidebar_scope);
        let rows_scope = self.scroll_scope("tag-list-scroll");
        let query = self.search.read(cx).value().to_lowercase();
        let mut rows = Vec::new();
        for tag in &book.tags {
            if self
                .group_filter
                .as_ref()
                .is_some_and(|id| id != &tag.group)
                || !tag.name.to_lowercase().contains(&query)
            {
                continue;
            }
            let group = book
                .groups
                .iter()
                .find(|g| g.id == tag.group)
                .map(|g| g.name.clone())
                .unwrap_or_default();
            rows.push(
                div()
                    .id(SharedString::from(format!("tag-row-{}", tag.id)))
                    .relative()
                    .h(px(64.))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(rgb(BORDER_SUBTLE))
                    .when(self.tag_id.as_ref() == Some(&tag.id), |d| {
                        d.bg(rgb(ACTIVE_BG)).child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(2.))
                                .bg(rgb(ACCENT)),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .left(px(24.))
                            .top(px(18.))
                            .child(self.chip(tag)),
                    )
                    .when(!compact, |d| {
                        d.child(
                            self.key_control(
                                SharedString::from(format!("tag-group-{}", tag.id)),
                                cx,
                            )
                            .absolute()
                            .left(relative(0.43))
                            .top(px(20.))
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .cursor_pointer()
                            .child(group)
                            .on_click(cx.listener({
                                let id = tag.id.clone();
                                move |this, _, window, cx| {
                                    this.act(Command::EditTag(id.clone()), window, cx)
                                }
                            })),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .right(px(if compact { 104. } else { 172. }))
                            .top(px(21.))
                            .size(px(12.))
                            .bg(rgb(tag.color)),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(70.))
                            .top(px(18.))
                            .child(self.icon_button(
                                SharedString::from(format!("edit-tag-{}", tag.id)),
                                IconName::Replace,
                                "Edit tag",
                                Command::EditTag(tag.id.clone()),
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(18.))
                            .top(px(18.))
                            .child(self.icon_button(
                                SharedString::from(format!("delete-tag-{}", tag.id)),
                                IconName::Delete,
                                "Delete tag",
                                Command::DeleteTag(tag.id.clone()),
                                cx,
                            )),
                    )
                    .into_any_element(),
            );
        }
        drop(rows_scope);
        let center = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(40.))
                    .flex_shrink_0()
                    .pl(px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(caption(heading))
                    .child(self.control(
                        "new-tag",
                        "+ Tag",
                        Command::NewTag,
                        80.,
                        false,
                        false,
                        cx,
                    )),
            )
            .child(
                div()
                    .mx(px(8.))
                    .mt(px(20.))
                    .h(px(32.))
                    .flex_shrink_0()
                    .child(Input::new(&self.search).h(px(32.))),
            )
            .child(
                div()
                    .relative()
                    .mx(px(8.))
                    .mt(px(16.))
                    .h(px(40.))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .absolute()
                            .left(px(16.))
                            .top(px(16.))
                            .child(caption("Tag")),
                    )
                    .when(!compact, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left(relative(0.43))
                                .top(px(16.))
                                .child(caption("Group")),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .right(px(if compact { 68. } else { 136. }))
                            .top(px(16.))
                            .child(caption("Color")),
                    ),
            )
            .child(
                div()
                    .id("tag-list-scroll")
                    .track_scroll(&self.scroll_handle("tag-list-scroll"))
                    .mt(px(16.))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            );
        let group = book
            .groups
            .iter()
            .find(|g| g.id == self.tag_group)
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "Choose group".into());
        let mut editor =
            div()
                .w(px(360.))
                .flex_shrink_0()
                .pl(px(32.))
                .border_l_1()
                .border_color(rgb(BORDER))
                .flex()
                .flex_col()
                .child(
                    div()
                        .relative()
                        .h(px(40.))
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .child(div().absolute().top(px(12.)).child(caption(
                            if self.tag_id.is_some() {
                                "Edit tag"
                            } else {
                                "New tag"
                            },
                        ))),
                )
                .child(
                    div()
                        .relative()
                        .h(px(368.))
                        .flex_shrink_0()
                        .child(div().absolute().top(px(24.)).child(caption("Name")))
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .top(px(48.))
                                .h(px(34.))
                                .child(Input::new(&self.name).h(px(34.))),
                        )
                        .child(div().absolute().top(px(106.)).child(caption("Group")))
                        .child(div().absolute().left_0().right_0().top(px(130.)).child(
                            self.control(
                                "tag-group-picker",
                                format!("{group}  ⌄"),
                                Command::TagGroupMenu,
                                328.,
                                false,
                                false,
                                cx,
                            ),
                        ))
                        .child(div().absolute().top(px(188.)).child(caption("Color")))
                        .child(div().absolute().top(px(218.)).flex().gap(px(16.)).children(
                            COLORS.into_iter().enumerate().map(|(i, color)| {
                                div()
                                    .id(("tag-color", i))
                                    .size(px(24.))
                                    .bg(rgb(color))
                                    .border_1()
                                    .border_color(rgb(if color == self.tag_color {
                                        TEXT_BRIGHT
                                    } else {
                                        BORDER_STRONG
                                    }))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.act(Command::Color(color), window, cx)
                                    }))
                            }),
                        ))
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(px(264.))
                                .w(px(152.))
                                .h(px(30.))
                                .child(Input::new(&self.custom_color).h(px(30.))),
                        )
                        .child(div().absolute().right_0().top(px(264.)).child(self.control(
                            "save-tag",
                            "Save",
                            Command::SaveTag,
                            112.,
                            false,
                            true,
                            cx,
                        )))
                        .child(
                            div()
                                .absolute()
                                .top(px(340.))
                                .child(caption("Assigned tracks")),
                        ),
                );
        let assigned = self
            .tag_id
            .as_ref()
            .map(|id| {
                self.core
                    .library
                    .tracks
                    .iter()
                    .filter(|t| {
                        book.assignments
                            .get(&t.id)
                            .is_some_and(|ids| ids.contains(id))
                    })
                    .map(|t| t.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        editor = editor.child(
            div()
                .pt(px(24.))
                .border_t_1()
                .border_color(rgb(BORDER))
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    self.cards_list("tag-tracks-scroll", assigned, None, cx)
                        .flex_1()
                        .min_h_0(),
                ),
        );
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .gap(px(24.))
            .child(
                div()
                    .id("tags-sidebar-scroll")
                    .track_scroll(&self.scroll_handle("tags-sidebar-scroll"))
                    .w(px(280.))
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .child(sidebar),
            )
            .child(center)
            .child(editor)
            .into_any_element()
    }
    fn reference_manager(&self, cx: &mut Context<Self>) -> AnyElement {
        let references = &self.core.library.reference_tracks;
        let query = self.reference_search.read(cx).value().to_lowercase();
        let assigned_tracks = self
            .core
            .library
            .tracks
            .iter()
            .filter(|track| track.reference_path.is_some())
            .count();
        let unassigned_references = references
            .iter()
            .filter(|reference| {
                !self
                    .core
                    .library
                    .tracks
                    .iter()
                    .any(|track| track.reference_path.as_ref() == Some(&reference.path))
            })
            .count();

        let rows_scope = self.scroll_scope("reference-catalog-scroll");
        let mut rows = Vec::new();
        for reference in references {
            let path = reference.path.clone();
            let name = self.reference_label(&path);
            if !query.is_empty()
                && !name.to_lowercase().contains(&query)
                && !path.display().to_string().to_lowercase().contains(&query)
            {
                continue;
            }
            let assigned = self
                .core
                .library
                .tracks
                .iter()
                .filter(|track| track.reference_path.as_ref() == Some(&path))
                .count();
            let editing = self.reference_edit.as_ref() == Some(&path);
            let full_path = path.display().to_string();
            let mut details = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().font_family(MONO).text_size(px(12.)).child(name))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_normal()
                        .font_family(MONO)
                        .text_size(px(10.))
                        .text_color(rgb(MUTED))
                        .child(full_path),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(if assigned == 0 { MUTED } else { TEXT }))
                        .child(if assigned == 0 {
                            "Not assigned to any tracks".to_owned()
                        } else {
                            format!(
                                "Assigned to {assigned} {} · removing clears those assignments",
                                if assigned == 1 { "track" } else { "tracks" }
                            )
                        }),
                );
            if editing {
                details = details.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(Input::new(&self.reference_name).h(px(30.)).flex_1())
                        .child(self.button(
                            "save-reference-name",
                            "Save",
                            Command::ReferenceRenameSave,
                            cx,
                        ))
                        .child(self.button("cancel-reference-name", "Cancel", Command::Cancel, cx)),
                );
            }
            let actions = div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_end()
                .gap_2()
                .flex_shrink_0()
                .child(self.button(
                    SharedString::from(format!("rename-{}", path.display())),
                    "Rename",
                    Command::ReferenceRename(path.clone()),
                    cx,
                ))
                .child(self.button(
                    SharedString::from(format!("ref-replace-{}", path.display())),
                    "Replace file",
                    Command::Import(ImportKind::ReplaceReference(path.clone())),
                    cx,
                ))
                .child(self.button(
                    SharedString::from(format!("ref-delete-{}", path.display())),
                    "Remove",
                    Command::RemoveReference(path),
                    cx,
                ));
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .p_4()
                    .bg(rgb(SURFACE))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .rounded(px(0.))
                    .child(details)
                    .child(actions)
                    .into_any_element(),
            );
        }
        drop(rows_scope);
        let rows_empty = rows.is_empty();
        let catalog_list = if references.is_empty() {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(caption("Your reference catalog is empty"))
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("Add reference audio here, then assign it to tracks from Review."),
                )
                .child(self.button(
                    "empty-import-reference-catalog",
                    "+ Add references",
                    Command::Import(ImportKind::Catalog),
                    cx,
                ))
                .into_any_element()
        } else if rows_empty {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED))
                .child("No references match this search.")
                .into_any_element()
        } else {
            div()
                .id("reference-catalog-scroll")
                .track_scroll(&self.scroll_handle("reference-catalog-scroll"))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_3()
                .children(rows)
                .into_any_element()
        };

        let metric = |label: &'static str, value: usize| {
            div()
                .flex_1()
                .min_w(px(120.))
                .p_4()
                .bg(rgb(SURFACE))
                .border_1()
                .border_color(rgb(BORDER))
                .rounded(px(0.))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .font_family(MONO)
                        .text_size(px(18.))
                        .text_color(rgb(TEXT_BRIGHT))
                        .child(value.to_string()),
                )
                .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(label))
        };
        let content = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(caption("Reference Manager"))
                            .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(
                                "Build and maintain the audio catalog used for comparison.",
                            )),
                    )
                    .child(self.button(
                        "import-reference-catalog",
                        "+ Add references",
                        Command::Import(ImportKind::Catalog),
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(metric("REFERENCE FILES", references.len()))
                    .child(metric("TRACK ASSIGNMENTS", assigned_tracks))
                    .child(metric("UNASSIGNED FILES", unassigned_references)),
            )
            .child(
                div()
                    .h(px(34.))
                    .flex_shrink_0()
                    .child(Input::new(&self.reference_search).h(px(34.))),
            )
            .child(catalog_list);
        content.into_any_element()
    }
    fn drag_note(&mut self, x: Pixels) {
        if let Some(draft) = &mut self.note_draft {
            let b = self.waveform_bounds[draft.source.index()].get();
            if b.size.width > px(0.) && draft.generation == self.core.generation {
                let ratio = ((x - b.origin.x) / b.size.width).clamp(0., 1.);
                draft.time = (ratio * self.core.duration(draft.source) as f32) as u64;
            }
        }
    }
    fn note_editor(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let draft = self.note_draft.as_ref()?;
        if self.mode != Mode::Review || draft.generation != self.core.generation {
            return None;
        }
        let b = self.waveform_bounds[draft.source.index()].get();
        let viewport = window.viewport_size();
        let width = 300_f32.min((f32::from(viewport.width) - 24.).max(0.));
        let ratio = draft.time as f32 / self.core.duration(draft.source).max(1) as f32;
        let left = (f32::from(b.origin.x) + ratio * f32::from(b.size.width) - width / 2.)
            .clamp(12., (f32::from(viewport.width) - width - 12.).max(12.));
        let below = f32::from(b.bottom()) + 8.;
        // Keep the rail exposed when a short window has no room below it.
        let top = if f32::from(viewport.height) - below < 200. {
            (f32::from(b.bottom()) - 248.).max(12.)
        } else {
            below
        };
        let mut editor = div()
            .id("note-editor")
            .occlude()
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .max_h(px((f32::from(viewport.height) - top - 12.).max(80.)))
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(ACCENT))
            .rounded(px(0.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(caption(format!("Comment · {}", timestamp(draft.time))))
                    .child(self.icon_button(
                        "close-note",
                        IconName::Close,
                        "Cancel comment",
                        Command::Cancel,
                        cx,
                    )),
            )
            .child(self.watched_input(&self.comment, cx))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child("Drag the marker to adjust its position"),
            );
        if let Some(error) = &self.note_error {
            editor = editor.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(ACCENT_TEXT))
                    .child(error.clone()),
            );
        }
        Some(
            editor
                .child(self.button("save-note", "Save comment", Command::SaveNote, cx))
                .into_any_element(),
        )
    }
    fn overlay(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let modal = self.modal.clone()?;
        let _scroll_scope = self.scroll_scope("modal-body");
        let mut body = div()
            .id("modal-body")
            .track_focus(&self.modal_focus)
            .tab_group()
            .track_scroll(&self.scroll_handle("modal-body"))
            .child(
                div()
                    .absolute()
                    .id("modal-tab-start")
                    .track_focus(&self.modal_start)
                    .size(px(0.)),
            )
            .overflow_y_scroll()
            .relative()
            .w(px(460.))
            .max_h(px(
                (f32::from(window.viewport_size().height) - 64.).min(680.)
            ))
            .p_6()
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(BORDER_STRONG))
            .rounded(px(0.))
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .h(px(2.))
                    .bg(rgb(ACCENT)),
            );
        let title = match &modal {
            Modal::Stage(_) | Modal::BulkStage => "Workflow stage",
            Modal::FilterStage => "Filter stage",
            Modal::BulkTags => "Bulk tags and album groups",
            Modal::BulkAlbums => "Album group",
            Modal::Assign(_) => "Track tags",
            Modal::EditTrack(_) => "Track details",
            Modal::AssignToTrack(_) => "Assign bounce to track",
            Modal::References => "Reference track",
            Modal::TagGroup => "Tag group",
            Modal::Group(_) => "Tag group",
            Modal::Confirm(_) => "Confirm",
        };
        body = body.child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .font_family(MONO)
                        .text_size(px(12.))
                        .text_color(rgb(TEXT_BRIGHT))
                        .child(title.to_uppercase()),
                )
                .child(self.icon_button(
                    "modal-close",
                    IconName::Close,
                    "Close",
                    Command::Cancel,
                    cx,
                )),
        );
        if let Some(error) = &self.modal_error {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(ACCENT_TEXT))
                    .child(error.clone()),
            );
        }
        match modal {
            Modal::BulkAlbums => {
                for tag in &self.core.library.tags.tags {
                    if self
                        .core
                        .library
                        .tags
                        .groups
                        .iter()
                        .any(|g| g.id == tag.group && g.album)
                    {
                        body = body.child(self.button(
                            SharedString::from(format!("bulk-album-{}", tag.id)),
                            tag.name.clone(),
                            Command::ManagerAlbum(tag.id.clone()),
                            cx,
                        ));
                    }
                }
                body = body.child(self.button(
                    "manager-create-album",
                    "+ Create album",
                    Command::NewAlbum,
                    cx,
                ));
            }
            Modal::BulkStage | Modal::FilterStage => {
                let filter = matches!(modal, Modal::FilterStage);
                body = body.child(self.button(
                    "manager-stage-reset",
                    if filter {
                        "All stages"
                    } else {
                        "Keep current stages"
                    },
                    Command::ManagerStage(None, filter),
                    cx,
                ));
                for (i, stage) in [
                    TrackStage::Backlog,
                    TrackStage::Production,
                    TrackStage::Mixdown,
                    TrackStage::Mastering,
                ]
                .into_iter()
                .enumerate()
                {
                    body = body.child(self.button(
                        ("manager-stage-choice", i),
                        stage.label(),
                        Command::ManagerStage(Some(stage), filter),
                        cx,
                    ));
                }
            }
            Modal::BulkTags => {
                for group in &self.core.library.tags.groups {
                    body = body.child(div().text_color(rgb(MUTED)).child(group.name.clone()));
                    for tag in self
                        .core
                        .library
                        .tags
                        .tags
                        .iter()
                        .filter(|t| t.group == group.id)
                    {
                        body = body.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().child(self.chip(tag)))
                                .child(self.control(
                                    SharedString::from(format!("bulk-add-{}", tag.id)),
                                    "Add",
                                    Command::ManagerTag(tag.id.clone(), true),
                                    0.,
                                    self.manager_edit.add.contains(&tag.id),
                                    false,
                                    cx,
                                ))
                                .child(self.control(
                                    SharedString::from(format!("bulk-remove-{}", tag.id)),
                                    "Remove",
                                    Command::ManagerTag(tag.id.clone(), false),
                                    0.,
                                    self.manager_edit.remove.contains(&tag.id),
                                    false,
                                    cx,
                                )),
                        );
                    }
                }
                body = body
                    .child(self.button("bulk-tags-done", "Done", Command::Cancel, cx))
                    .child(self.button("bulk-new-tag", "Create tag / album", Command::NewTag, cx))
                    .child(self.button("bulk-new-group", "New group", Command::NewGroup, cx));
            }
            Modal::Stage(id) => {
                for (i, stage) in [
                    TrackStage::Backlog,
                    TrackStage::Production,
                    TrackStage::Mixdown,
                    TrackStage::Mastering,
                ]
                .into_iter()
                .enumerate()
                {
                    body = body.child(self.button(
                        ("stage-option", i),
                        stage.label(),
                        Command::Stage(id.clone(), stage),
                        cx,
                    ));
                }
            }
            Modal::Assign(track) => {
                let _assign_scope = self.scroll_scope("assign-tags-scroll");
                let book = &self.core.library.tags;
                let mut options = Vec::new();
                for group in &book.groups {
                    options.push(
                        div()
                            .text_color(rgb(MUTED))
                            .text_size(px(12.))
                            .pt_3()
                            .child(group.name.clone())
                            .into_any_element(),
                    );
                    for tag in book.tags.iter().filter(|t| t.group == group.id) {
                        let assigned = book
                            .assignments
                            .get(&track)
                            .is_some_and(|ids| ids.contains(&tag.id));
                        options.push(self.control(
                            SharedString::from(format!("assign-{}", tag.id)),
                            tag.name.clone(),
                            Command::ToggleTag(track.clone(), tag.id.clone()),
                            0.,
                            assigned,
                            false,
                            cx,
                        ));
                    }
                }
                body = body
                    .child(
                        div()
                            .id("assign-tags-scroll")
                            .track_scroll(&self.scroll_handle("assign-tags-scroll"))
                            .max_h(px(450.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .children(options),
                    )
                    .child(self.button(
                        "manage-tags",
                        "Manage tags",
                        Command::Mode(Mode::Tags),
                        cx,
                    ));
            }
            Modal::EditTrack(track_id) => {
                let active_label = self
                    .core
                    .library
                    .tracks
                    .iter()
                    .find(|t| t.id == track_id)
                    .map(|t| t.active_build_label())
                    .unwrap_or_else(|| String::from("current"));
                body = body
                    .child(field("Title (keeps builds grouped)", self.watched_input(&self.track_title_input, cx)))
                    .child(field("Next action (shown on Planner card)", self.watched_input(&self.next_action_input, cx)))
                    .child(field(
                        "Build feedback / shared note (for active bounce)",
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .font_family(MONO)
                                    .text_size(px(10.))
                                    .text_color(rgb(MUTED))
                                    .child(format!("ACTIVE BOUNCE: {}", active_label.to_uppercase())),
                            )
                            .child(self.watched_input(&self.revision_feedback_input, cx)),
                    ))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(self.button("cancel-track-details", "Cancel", Command::Cancel, cx))
                            .child(self.control(
                                "save-track-details",
                                "Save",
                                Command::SaveTrackDetails,
                                84.,
                                false,
                                true,
                                cx,
                            )),
                    );
            }
            Modal::AssignToTrack(source_id) => {
                body = body.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child("Attach this bounce to an existing track as a revision without renaming the audio file."),
                );
                for (i, target) in self
                    .core
                    .library
                    .tracks
                    .iter()
                    .filter(|t| t.id != source_id)
                    .enumerate()
                {
                    let label = format!("{} [{}]", target.title, target.current_bounce_badge());
                    body = body.child(self.button(
                        ("assign-target-track", i),
                        label,
                        Command::AssignToTrack(source_id.clone(), target.id.clone()),
                        cx,
                    ));
                }
            }
            Modal::References => {
                body = body.child(self.button(
                    "manage-references",
                    "Manage references",
                    Command::Mode(Mode::References),
                    cx,
                ));
                for (i, reference) in self.core.library.reference_tracks.iter().enumerate() {
                    body = body.child(self.button(
                        ("reference-option", i),
                        self.reference_label(&reference.path),
                        Command::Reference(reference.path.clone()),
                        cx,
                    ));
                }
                if let Some(track) = self.core.selected() {
                    body = body
                        .child(self.button(
                            "import-track-reference",
                            "Import reference",
                            Command::Import(ImportKind::Reference(track.id.clone())),
                            cx,
                        ))
                        .child(self.button(
                            "clear-reference",
                            "Remove assignment",
                            Command::ClearReference,
                            cx,
                        ));
                }
            }
            Modal::TagGroup => {
                for (i, group) in self.core.library.tags.groups.iter().enumerate() {
                    body = body.child(self.button(
                        ("choose-group", i),
                        group.name.clone(),
                        Command::TagGroup(group.id.clone()),
                        cx,
                    ));
                }
                body = body.child(self.button(
                    "group-picker-new",
                    "+ New group",
                    Command::NewGroup,
                    cx,
                ));
            }
            Modal::Group(id) => {
                body = body
                    .child(field("Name", self.watched_input(&self.group_name, cx)))
                    .child(self.control(
                        "album-group-toggle",
                        "Album group",
                        Command::AlbumGroup,
                        0.,
                        self.group_album,
                        false,
                        cx,
                    ))
                    .child(self.button("save-group", "Save group", Command::SaveGroup, cx));
                if let Some(id) = id {
                    body = body.child(self.button(
                        "delete-group",
                        "Delete group",
                        Command::DeleteGroup(id),
                        cx,
                    ));
                }
            }
            Modal::Confirm(kind) => {
                let message = match kind {
                    Confirmation::Tracks(_) => String::from(
                        "Remove the selected tracks from the library? Audio files stay in place.",
                    ),
                    Confirmation::Track(_) => String::from(
                        "Remove this track from the library? The audio file stays in place.",
                    ),
                    Confirmation::Tag(_) => {
                        String::from("Delete this tag and remove its track assignments?")
                    }
                    Confirmation::Group(_) => {
                        String::from("Delete this group, its tags, and their track assignments?")
                    }
                    Confirmation::Reference(path) => {
                        let assigned = self
                            .core
                            .library
                            .tracks
                            .iter()
                            .filter(|track| track.reference_path.as_ref() == Some(&path))
                            .count();
                        format!(
                            "Remove {} and clear its {} assigned {}? The audio file stays in place.",
                            self.reference_label(&path),
                            assigned,
                            if assigned == 1 { "track" } else { "tracks" }
                        )
                    }
                    Confirmation::Bind(_) => String::from(
                        "Associate historical comments with the currently verified audio file? Confirm that this is the original recording.",
                    ),
                    Confirmation::Recovery => {
                        String::from("Back up the unreadable library and start fresh?")
                    }
                };
                body = body.child(message).child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(self.button("cancel-confirm", "Cancel", Command::Cancel, cx))
                        .child(self.button("confirm-action", "Confirm", Command::Confirm, cx)),
                );
            }
        }
        body = body.child(
            div()
                .absolute()
                .id("modal-tab-end")
                .track_focus(&self.modal_end)
                .size(px(0.)),
        );
        Some(
            div()
                .id("modal-overlay")
                .occlude()
                .absolute()
                .inset_0()
                .bg(rgba(0x000000cc))
                .flex()
                .items_center()
                .justify_center()
                .child(body)
                .into_any_element(),
        )
    }
}
impl Render for Cadence {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.focus_frame.get().saturating_add(1);
        self.focus_frame.set(frame);
        self.targets
            .borrow_mut()
            .retain(|_, t| t.seen.saturating_add(1) >= frame);
        self.manager_selected
            .retain(|id| self.core.library.tracks.iter().any(|t| &t.id == id));
        self.manager_edit
            .add
            .retain(|id| self.core.library.tags.tags.iter().any(|t| &t.id == id));
        self.manager_edit
            .remove
            .retain(|id| self.core.library.tags.tags.iter().any(|t| &t.id == id));
        let compact = f32::from(window.viewport_size().width) < 1400.;
        let stacked_controls = self.mode == Mode::Review
            && self.core.selected().is_some()
            && f32::from(window.viewport_size().width) < 1600.;
        let mut tabs = div()
            .absolute()
            .left(px(if compact { 160. } else { 352. }))
            .top(px(-4.))
            .flex()
            .gap(px(if compact { 8. } else { 12. }));
        for (i, (name, mode, width)) in [
            ("Review", Mode::Review, if compact { 72. } else { 84. }),
            ("Planner", Mode::Planner, if compact { 72. } else { 84. }),
            (
                if compact { "Tracks" } else { "Track Manager" },
                Mode::Tracks,
                if compact { 72. } else { 126. },
            ),
            ("Tags", Mode::Tags, if compact { 64. } else { 72. }),
            (
                if compact {
                    "References"
                } else {
                    "Reference Manager"
                },
                Mode::References,
                if compact { 96. } else { 136. },
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let active = self.mode == mode;
            tabs = tabs.child(
                self.key_control(("workspace-tab", i), cx)
                    .relative()
                    .w(px(width))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(active, |d| d.bg(rgb(ACTIVE_BG)))
                    .hover(move |d| {
                        if active {
                            d.bg(rgb(ACTIVE_HOVER))
                        } else {
                            d.bg(rgb(SURFACE)).text_color(rgb(TEXT))
                        }
                    })
                    .text_color(rgb(if active { TEXT_BRIGHT } else { MUTED }))
                    .child(caption(name).text_color(rgb(if active {
                        TEXT_BRIGHT
                    } else {
                        MUTED
                    })))
                    .when(active, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left(px(5.))
                                .top(px(11.))
                                .size(px(4.))
                                .bg(rgb(ACCENT)),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(0.))
                                .right(px(0.))
                                .top(px(35.))
                                .h(px(2.))
                                .bg(rgb(ACCENT)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.act(Command::Mode(mode), window, cx)
                    })),
            );
        }
        let logo = canvas(
            |_, _, _| (),
            |b, _, window, _| {
                for (i, h) in [12., 24., 18., 8.].into_iter().enumerate() {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                b.origin.x + px(i as f32 * 5.),
                                b.origin.y + px((24. - h) / 2.),
                            ),
                            size(px(2.), px(h)),
                        ),
                        rgb(ACCENT),
                    ));
                }
            },
        )
        .w(px(20.))
        .h(px(24.));
        let mut header = div()
            .relative()
            .h(px(if stacked_controls { 80. } else { 48. }))
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(-2.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(logo)
                    .when(!compact, |d| {
                        d.child(
                            div()
                                .font_family(MONO)
                                .font_weight(FontWeight::BOLD)
                                .text_size(px(15.))
                                .text_color(rgb(TEXT_BRIGHT))
                                .child("CADENCE"),
                        )
                    }),
            )
            .child(tabs);
        if self.mode == Mode::Review && self.core.selected().is_some() {
            let label = self
                .core
                .selected()
                .and_then(|t| t.reference_path.as_ref())
                .map(|p| self.reference_label(p))
                .unwrap_or_else(|| "Reference master".into());
            let controls = div()
                .absolute()
                .right(px(if compact { 0. } else { 48. }))
                .top(px(if stacked_controls { 36. } else { -4. }))
                .flex()
                .items_center()
                .gap(px(12.))
                .child(self.control(
                    "reference-picker",
                    format!("{label}  ⌄"),
                    Command::References,
                    if compact { 112. } else { 164. },
                    false,
                    false,
                    cx,
                ))
                .child(self.control(
                    "match-reference",
                    "Match",
                    Command::Match,
                    72.,
                    self.core.matching,
                    false,
                    cx,
                ))
                .child(
                    self.key_control("mute", cx)
                        .tooltip({
                            let muted = self.core.muted;
                            move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(if muted {
                                    "Unmute"
                                } else {
                                    "Mute"
                                })
                                .build(window, cx)
                            }
                        })
                        .ml(px(4.))
                        .size(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(rgb(if self.core.muted {
                            ACCENT_TEXT
                        } else {
                            MUTED
                        }))
                        .hover(|d| d.text_color(rgb(TEXT_BRIGHT)))
                        .child(
                            gpui_component::Icon::default()
                                .path(if self.core.muted {
                                    "cadence/volume-off.svg"
                                } else {
                                    "cadence/volume.svg"
                                })
                                .size(px(16.)),
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.act(Command::Mute, window, cx)),
                        ),
                )
                .child(
                    self.key_control("volume-control", cx)
                        .on_click(cx.listener(|this, event, window, cx| {
                            if matches!(event, ClickEvent::Keyboard(_)) {
                                this.act(Command::Mute, window, cx);
                            }
                        }))
                        .key_context("CadenceControl CadenceVolume")
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                "Volume: arrow keys adjust; Home/End set minimum/maximum",
                            )
                            .build(window, cx)
                        })
                        .on_action(cx.listener(|this, _: &VolumeUp, window, cx| {
                            cx.stop_propagation();
                            this.act(Command::Volume(this.core.volume + 0.01), window, cx);
                        }))
                        .on_action(cx.listener(|this, _: &VolumeDown, window, cx| {
                            cx.stop_propagation();
                            this.act(Command::Volume(this.core.volume - 0.01), window, cx);
                        }))
                        .on_action(cx.listener(|this, _: &VolumeMin, window, cx| {
                            cx.stop_propagation();
                            this.act(Command::Volume(0.), window, cx);
                        }))
                        .on_action(cx.listener(|this, _: &VolumeMax, window, cx| {
                            cx.stop_propagation();
                            this.act(Command::Volume(1.), window, cx);
                        }))
                        .w(px(if compact { 80. } else { 140. }))
                        .h(px(28.))
                        .child(
                            canvas(|_, _, _| (), {
                                let value = self.core.volume;
                                let muted = self.core.muted;
                                move |b, _, window, _| {
                                    let y = b.origin.y + px(14.);
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(b.origin.x, y),
                                            size(b.size.width, px(1.)),
                                        ),
                                        rgb(BORDER),
                                    ));
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(b.origin.x, y - px(0.5)),
                                            size(b.size.width * value, px(2.)),
                                        ),
                                        rgb(if muted { DIM } else { ACCENT }),
                                    ));
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(
                                                b.origin.x + b.size.width * value - px(2.),
                                                y - px(6.),
                                            ),
                                            size(px(4.), px(12.)),
                                        ),
                                        rgb(if muted { MUTED } else { TEXT_BRIGHT }),
                                    ));
                                }
                            })
                            .size_full(),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .opacity(0.)
                                .child(Slider::new(&self.volume)),
                        ),
                );
            header = header.child(controls);
        }
        let content = match self.mode {
            Mode::Review => self.review(window, cx),
            Mode::Planner => self.planner(cx),
            Mode::Tracks => self.track_manager(window, cx),
            Mode::Tags => self.tags(window, cx),
            Mode::References => self.reference_manager(cx),
        };
        let mut root = div()
            .id("cadence-root")
            .key_context("Cadence")
            .track_focus(&self.focus)
            .tab_group()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .p(px(32.))
            .gap(px(24.))
            .font_family(MONO)
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .text_size(px(12.))
            .child(header);
        if let Some(error) = self
            .core
            .error
            .clone()
            .or_else(|| self.core.warning.clone())
        {
            let mut bar = div()
                .relative()
                .flex()
                .items_center()
                .gap_3()
                .p_3()
                .pl(px(14.))
                .bg(rgb(ACTIVE_BG))
                .border_1()
                .border_color(rgb(ACCENT_DIM))
                .rounded(px(0.))
                .text_color(rgb(TEXT_BRIGHT))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(rgb(ACCENT)),
                )
                .child(div().flex_1().child(error));
            if !self.core.ready {
                bar = bar
                    .child(self.button("retry-load", "Retry", Command::RetryLoad, cx))
                    .child(self.button(
                        "recover-library",
                        "Back up and reset",
                        Command::Recover,
                        cx,
                    ));
            } else if self.core.dirty() {
                bar = bar.child(self.button("retry-save", "Retry save", Command::RetrySave, cx));
            }
            bar = bar.child(self.icon_button(
                "dismiss-error",
                IconName::Close,
                "Dismiss message",
                Command::DismissError,
                cx,
            ));
            root = root.child(bar);
        }
        if self.core.importing {
            root = root.child(div().text_color(rgb(MUTED)).child("Importing audio…"));
        }
        root = root.child(content);
        if let Some(editor) = self.note_editor(window, cx) {
            root = root.child(editor);
        }
        if let Some(overlay) = self.overlay(window, cx) {
            root = root.child(overlay);
        }
        root.on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
            let kind = match this.mode {
                Mode::References => ImportKind::Catalog,
                Mode::Review | Mode::Planner | Mode::Tags | Mode::Tracks => ImportKind::Main,
            };
            this.core.import(paths.paths().to_vec(), kind);
            cx.notify();
        }))
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
            let mut changed = false;
            if this.note_dragging {
                this.drag_note(event.position.x);
                changed = true;
            }
            if let Some((src, start, is_loop)) = this.gesture {
                let b = this.waveform_bounds[src.index()].get();
                let ratio = ((event.position.x - b.origin.x) / b.size.width).clamp(0., 1.);
                if is_loop {
                    this.core.set_loop(src, start, ratio);
                } else {
                    this.core
                        .seek((ratio * this.core.duration(src) as f32) as u64, false);
                }
                changed = true;
            }
            for src_idx in 0..2 {
                let next = if this.mode == Mode::Review
                    && this.modal.is_none()
                    && !this.note_dragging
                    && this.gesture.is_none()
                {
                    let b = this.waveform_bounds[src_idx].get();
                    if b.contains(&event.position) {
                        waveform_hover_at(
                            f32::from(event.position.x - b.origin.x),
                            f32::from(event.position.y - b.origin.y),
                            f32::from(b.size.width),
                            f32::from(b.size.height),
                        )
                    } else {
                        None
                    }
                } else {
                    None
                };
                if this.waveform_hover[src_idx] != next {
                    this.waveform_hover[src_idx] = next;
                    changed = true;
                }
            }
            if changed {
                cx.notify();
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, event: &MouseUpEvent, window, cx| {
                if this.note_dragging {
                    this.drag_note(event.position.x);
                    this.note_dragging = false;
                    this.comment.update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                }
                if let Some((src, start, is_loop)) = this.gesture.take() {
                    let b = this.waveform_bounds[src.index()].get();
                    let ratio = ((event.position.x - b.origin.x) / b.size.width).clamp(0., 1.);
                    if is_loop {
                        this.core.set_loop(src, start, ratio);
                    } else {
                        this.core
                            .seek((ratio * this.core.duration(src) as f32) as u64, true);
                    }
                    cx.notify();
                }
            }),
        )
        .on_action(cx.listener(|this, _: &NextControl, window, cx| {
            cx.stop_propagation();
            this.traverse(false, window, cx);
        }))
        .on_action(cx.listener(|this, _: &PreviousControl, window, cx| {
            cx.stop_propagation();
            this.traverse(true, window, cx);
        }))
        .on_action(cx.listener(|this, _: &PageForward, window, cx| {
            cx.stop_propagation();
            this.page_scroll(false, window, cx);
        }))
        .on_action(cx.listener(|this, _: &PageBackward, window, cx| {
            cx.stop_propagation();
            this.page_scroll(true, window, cx);
        }))
        .on_action(cx.listener(|this, _: &PlayPause, window, cx| {
            if this.modal.is_none() {
                this.act(Command::Play(this.core.source), window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &Stop, window, cx| {
            if this.modal.is_none() {
                this.act(Command::Stop, window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &NewNote, window, cx| {
            if this.modal.is_none() {
                this.act(
                    Command::NewNote(this.core.source, this.core.position),
                    window,
                    cx,
                );
            }
        }))
        .on_action(
            cx.listener(|this, _: &CloseOverlay, window, cx| this.act(Command::Cancel, window, cx)),
        )
        .on_action(cx.listener(|this, _: &Quit, _, cx| {
            if this.core.dirty() || this.core.importing {
                this.closing = true;
            } else {
                cx.quit();
            }
        }))
    }
}
fn corner_frame(color: u32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            for (x, y, sx, sy) in [
                (b.left(), b.top(), 1., 1.),
                (b.right(), b.top(), -1., 1.),
                (b.left(), b.bottom(), 1., -1.),
                (b.right(), b.bottom(), -1., -1.),
            ] {
                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            if sx > 0. { x } else { x - px(8.) },
                            if sy > 0. { y } else { y - px(1.) },
                        ),
                        size(px(8.), px(1.)),
                    ),
                    rgb(color),
                ));
                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            if sx > 0. { x } else { x - px(1.) },
                            if sy > 0. { y } else { y - px(8.) },
                        ),
                        size(px(1.), px(8.)),
                    ),
                    rgb(color),
                ));
            }
        },
    )
    .size_full()
}
fn caption(label: impl Into<String>) -> Div {
    div()
        .font_family(MONO)
        .text_size(px(10.))
        .line_height(px(16.))
        .text_color(rgb(MUTED))
        .child(label.into().to_uppercase())
}
fn field(label: &'static str, element: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_color(rgb(MUTED)).text_size(px(12.)).child(label))
        .child(element)
        .into_any_element()
}
fn timestamp(ms: u64) -> String {
    format!("{:02}:{:02}", ms / 60000, (ms / 1000) % 60)
}
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", NextControl, Some("Cadence")),
        KeyBinding::new("tab", NextControl, Some("Cadence && Input")),
        KeyBinding::new("shift-tab", PreviousControl, Some("Cadence && Input")),
        KeyBinding::new("escape", CloseOverlay, Some("Cadence && Input")),
        KeyBinding::new("shift-tab", PreviousControl, Some("Cadence")),
        KeyBinding::new("pagedown", PageForward, Some("Cadence && !Input")),
        KeyBinding::new("pageup", PageBackward, Some("Cadence && !Input")),
        KeyBinding::new("right", VolumeUp, Some("CadenceVolume")),
        KeyBinding::new("up", VolumeUp, Some("CadenceVolume")),
        KeyBinding::new("left", VolumeDown, Some("CadenceVolume")),
        KeyBinding::new("down", VolumeDown, Some("CadenceVolume")),
        KeyBinding::new("home", VolumeMin, Some("CadenceVolume")),
        KeyBinding::new("end", VolumeMax, Some("CadenceVolume")),
        KeyBinding::new("right", SeekForward, Some("CadenceWaveform")),
        KeyBinding::new("left", SeekBackward, Some("CadenceWaveform")),
        KeyBinding::new(
            "space",
            PlayPause,
            Some("Cadence && !Input && !CadenceControl"),
        ),
        KeyBinding::new("escape", CloseOverlay, Some("Cadence")),
        KeyBinding::new("n", NewNote, Some("Cadence && !Input")),
        KeyBinding::new("cmd-q", Quit, Some("Cadence")),
        KeyBinding::new("ctrl-q", Quit, Some("Cadence")),
        KeyBinding::new("s", Stop, Some("Cadence && !Input")),
    ]);
}

fn tint(color: u32, alpha: f32) -> Hsla {
    let mut color = Hsla::from(rgb(color));
    color.a = alpha;
    color
}

// Leave room for the hidden-label count rather than clipping a partial chip.
fn fitting_chips(available: f32, widths: &[f32]) -> usize {
    if widths.iter().sum::<f32>() + widths.len().saturating_sub(1) as f32 * 6. <= available {
        return widths.len();
    }
    let mut used = 0.;
    for (i, width) in widths.iter().enumerate() {
        let next = used + if i == 0 { 0. } else { 6. } + width;
        let remaining = widths.len() - i - 1;
        let counter = if remaining == 0 {
            0.
        } else {
            6. + (remaining.to_string().len() + 1) as f32 * 6.1
        };
        if next + counter > available {
            return i;
        }
        used = next;
    }
    widths.len()
}

const WAVEFORM_BAR_PITCH: f32 = 4.0;
const WAVEFORM_MAX_BARS: usize = 320;
const WAVEFORM_SPLIT_RATIO: f32 = 2.0 / 3.0;
const WAVEFORM_RAIL_HIT_SLOP: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum WaveformHover {
    Seek(f32),
    Comment(f32),
}

fn waveform_rail_offset(height: f32) -> f32 {
    if !height.is_finite() || height <= 0.0 {
        return 0.0;
    }
    (height * WAVEFORM_SPLIT_RATIO).round()
}

fn is_comment_zone(local_y: f32, height: f32) -> bool {
    height.is_finite()
        && height > 0.0
        && local_y.is_finite()
        && local_y >= waveform_rail_offset(height) - WAVEFORM_RAIL_HIT_SLOP
}

fn waveform_hover_at(local_x: f32, local_y: f32, width: f32, height: f32) -> Option<WaveformHover> {
    if !width.is_finite()
        || !height.is_finite()
        || !local_x.is_finite()
        || !local_y.is_finite()
        || width <= 0.0
        || height <= 0.0
        || local_x < 0.0
        || local_x > width
        || local_y < 0.0
        || local_y > height
    {
        return None;
    }
    let ratio = (local_x / width).clamp(0.0, 1.0);
    Some(if is_comment_zone(local_y, height) {
        WaveformHover::Comment(ratio)
    } else {
        WaveformHover::Seek(ratio)
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct WaveformBar {
    peak: f32,
    rms: f32,
}

struct WaveformBarsCache {
    summary: std::sync::Arc<crate::signal_summary::GpuSignalSummary>,
    count: usize,
    bars: Vec<WaveformBar>,
}

fn waveform_bar_count(width: f32) -> usize {
    if !width.is_finite() || width <= 0.0 {
        return 1;
    }
    ((width / WAVEFORM_BAR_PITCH).floor() as usize).clamp(1, WAVEFORM_MAX_BARS)
}

fn waveform_bars(
    summary: &crate::signal_summary::GpuSignalSummary,
    count: usize,
) -> Vec<WaveformBar> {
    let count = count.max(1);
    let Some(level) = summary.levels.first() else {
        return vec![WaveformBar::default(); count];
    };
    let bands = summary.band_count.max(1);
    let has_energy = bands > 1;
    let bucket_count = level.buckets.len() / bands;
    if bucket_count == 0 {
        return vec![WaveformBar::default(); count];
    }

    let mut raw = Vec::with_capacity(count);
    let mut peak_ceiling = 0.0f32;
    let mut rms_ceiling = 0.0f32;

    for i in 0..count {
        let (start, end) = if bucket_count < count {
            let idx = if count == 1 {
                0
            } else {
                (i * (bucket_count - 1)) / (count - 1)
            };
            (idx, idx + 1)
        } else {
            let start = i * bucket_count / count;
            let end = ((i + 1) * bucket_count / count)
                .max(start + 1)
                .min(bucket_count);
            (start, end)
        };
        let mut max_p = 0.0f32;
        let mut sum_p = 0.0f32;
        let mut sum_e = 0.0f32;
        let n = (end - start).max(1) as f32;
        for j in start..end {
            let base = j * bands;
            let p = level
                .buckets
                .get(base)
                .map(|b| b.min.abs().max(b.max.abs()))
                .filter(|v| v.is_finite())
                .unwrap_or(0.0);
            max_p = max_p.max(p);
            sum_p += p;
            if has_energy {
                let e = level
                    .buckets
                    .get(base + 1)
                    .map(|b| b.max.max(0.0))
                    .filter(|v| v.is_finite())
                    .unwrap_or(0.0);
                sum_e += e;
            }
        }
        let mean_p = sum_p / n;
        // Blend mean peak and max peak so transients read clearly without a
        // single 0 dBFS hit flattening an entire quieter section.
        let peak = 0.55 * mean_p + 0.45 * max_p;
        let rms = if has_energy {
            (sum_e / n).max(0.0).sqrt()
        } else {
            mean_p * std::f32::consts::FRAC_1_SQRT_2
        };
        peak_ceiling = peak_ceiling.max(peak);
        rms_ceiling = rms_ceiling.max(rms);
        raw.push(WaveformBar { peak, rms });
    }

    if peak_ceiling <= 1e-6 {
        return vec![WaveformBar::default(); count];
    }
    let rms_scale = rms_ceiling.max(peak_ceiling * 0.25);
    raw.into_iter()
        .map(|bar| {
            let peak = (bar.peak / peak_ceiling).clamp(0.0, 1.0);
            let norm_rms = (bar.rms / rms_scale).clamp(0.0, 1.0);
            // Expand upper RMS contrast slightly so loud mastered tracks still
            // separate drops, verses, and breakdowns while keeping peak tips visible.
            let rms = (norm_rms.powf(1.25) * 0.82).min(peak * 0.94).clamp(0.0, 1.0);
            WaveformBar { peak, rms }
        })
        .collect()
}

fn lufs_lit_segments(lufs: Option<f32>) -> usize {
    let Some(value) = lufs.filter(|v| v.is_finite()) else {
        return 0;
    };
    let fraction = ((value + 24.0) / 24.0).clamp(0.0, 1.0);
    (fraction * 14.0).round() as usize
}

#[cfg(test)]
mod ui_tests {
    use super::{
        ACCENT, BG, BORDER, BORDER_STRONG, BORDER_SUBTLE, DIM, MUTED, SURFACE, SURFACE_ALT,
        SURFACE_HOVER, TEXT, TEXT_BRIGHT, WAVE_LOWER_BG, WAVE_LOWER_PEAK_PLAYED,
        WAVE_LOWER_PEAK_UNPLAYED, WAVE_LOWER_PLAYED, WAVE_LOWER_UNPLAYED, WAVE_PEAK_PLAYED,
        WAVE_PEAK_UNPLAYED, WAVE_UNPLAYED, WELL, WaveformHover, fitting_chips, is_comment_zone,
        lufs_lit_segments, waveform_bar_count, waveform_bars, waveform_hover_at,
        waveform_rail_offset,
    };
    use crate::signal_summary::{GpuSignalSummary, GpuSignalSummaryBucket, GpuSignalSummaryLevel};
    use std::sync::Arc;

    fn two_band_summary(windows: &[(f32, f32)]) -> GpuSignalSummary {
        let buckets: Vec<GpuSignalSummaryBucket> = windows
            .iter()
            .flat_map(|&(peak, rms)| {
                [
                    GpuSignalSummaryBucket {
                        min: -peak,
                        max: peak,
                    },
                    GpuSignalSummaryBucket {
                        min: 0.0,
                        max: rms * rms,
                    },
                ]
            })
            .collect();
        GpuSignalSummary {
            frames: windows.len(),
            band_count: 2,
            levels: vec![GpuSignalSummaryLevel {
                bucket_frames: 1,
                buckets: Arc::from(buckets),
            }],
        }
    }

    fn rgb_channels(hex: u32) -> (u8, u8, u8) {
        (
            ((hex >> 16) & 0xff) as u8,
            ((hex >> 8) & 0xff) as u8,
            (hex & 0xff) as u8,
        )
    }

    #[test]
    fn terminal_palette_uses_neutral_black_greys_white_and_signal_red() {
        for neutral in [
            BG,
            WELL,
            SURFACE,
            SURFACE_ALT,
            SURFACE_HOVER,
            BORDER,
            BORDER_SUBTLE,
            BORDER_STRONG,
            TEXT,
            TEXT_BRIGHT,
            MUTED,
            DIM,
            WAVE_UNPLAYED,
            WAVE_PEAK_UNPLAYED,
            WAVE_LOWER_BG,
            WAVE_LOWER_UNPLAYED,
            WAVE_LOWER_PEAK_UNPLAYED,
        ] {
            let (r, g, b) = rgb_channels(neutral);
            assert_eq!(r, g, "neutral token 0x{neutral:06x} must stay achromatic");
            assert_eq!(g, b, "neutral token 0x{neutral:06x} must stay achromatic");
        }
        for red in [
            ACCENT,
            WAVE_PEAK_PLAYED,
            WAVE_LOWER_PLAYED,
            WAVE_LOWER_PEAK_PLAYED,
        ] {
            let (r, g, b) = rgb_channels(red);
            assert!(r > g * 3 && r > b * 3, "red token 0x{red:06x} must stay red");
        }
        assert_eq!(TEXT_BRIGHT, 0xffffff);
    }

    #[test]
    fn waveform_comment_rail_sits_at_bottom_third_with_clean_hit_boundary() {
        assert_eq!(waveform_rail_offset(136.0), 91.0);
        assert_eq!(waveform_rail_offset(0.0), 0.0);
        // Upper 2/3 seeks/loops; rail line and lower 1/3 place/edit comments.
        assert!(!is_comment_zone(40.0, 136.0));
        assert!(!is_comment_zone(86.0, 136.0));
        assert!(is_comment_zone(91.0, 136.0));
        assert!(is_comment_zone(120.0, 136.0));
        assert_eq!(
            waveform_hover_at(250.0, 40.0, 1000.0, 136.0),
            Some(WaveformHover::Seek(0.25))
        );
        assert_eq!(
            waveform_hover_at(750.0, 95.0, 1000.0, 136.0),
            Some(WaveformHover::Comment(0.75))
        );
        assert_eq!(waveform_hover_at(-1.0, 40.0, 1000.0, 136.0), None);
        assert_eq!(waveform_hover_at(500.0, 140.0, 1000.0, 136.0), None);
    }

    #[test]
    fn waveform_bars_reveal_sections_on_loud_limited_tracks_and_keep_silence_quiet() {
        // Simulate a loud mastered track:
        // - Section 0 (drop): dense peaks near 1.0, high RMS 0.50
        // - Section 1 (breakdown): sparse clap peaking at 0.95 once, mostly 0.25, low RMS 0.14
        // - Section 2 (cut): silence
        let mut windows = Vec::new();
        for _ in 0..20 {
            windows.push((0.98, 0.50));
        }
        for i in 0..20 {
            let peak = if i == 10 { 0.95 } else { 0.25 };
            windows.push((peak, 0.14));
        }
        for _ in 0..20 {
            windows.push((0.0, 0.0));
        }
        let summary = two_band_summary(&windows);
        let bars = waveform_bars(&summary, 3);

        // Drop is tall with distinct transient headroom above the RMS core.
        assert!((bars[0].peak - 1.0).abs() < 0.01);
        assert!(bars[0].rms > 0.70 && bars[0].rms < bars[0].peak);

        // Breakdown separates dramatically from drop despite the 0.95 clap transient.
        assert!(bars[1].peak < 0.65);
        assert!(bars[1].rms < 0.25);
        assert!(bars[1].rms < bars[1].peak);

        // Silent cut stays silent.
        assert_eq!(bars[2].peak, 0.0);
        assert_eq!(bars[2].rms, 0.0);
    }

    #[test]
    fn waveform_bars_handle_empty_single_band_and_non_finite_summaries() {
        assert_eq!(waveform_bar_count(0.0), 1);
        assert_eq!(waveform_bar_count(980.0), 245);
        assert_eq!(waveform_bar_count(4000.0), 320);

        let empty = GpuSignalSummary {
            frames: 0,
            band_count: 2,
            levels: Vec::new(),
        };
        assert_eq!(waveform_bars(&empty, 4).len(), 4);

        let single_band = GpuSignalSummary::from_interleaved_samples(
            &[0.0, 0.25, f32::NAN, 1.0],
            4,
            1,
        );
        let bars = waveform_bars(&single_band, 8);
        assert_eq!(bars.len(), 8);
        for bar in bars {
            assert!(bar.peak.is_finite() && (0.0..=1.0).contains(&bar.peak));
            assert!(bar.rms.is_finite() && bar.rms <= bar.peak);
        }
    }

    #[test]
    fn lufs_segments_map_silence_mid_and_hot_loudness_bounds() {
        assert_eq!(lufs_lit_segments(None), 0);
        assert_eq!(lufs_lit_segments(Some(f32::NAN)), 0);
        assert_eq!(lufs_lit_segments(Some(-30.0)), 0);
        assert_eq!(lufs_lit_segments(Some(-12.0)), 7);
        assert_eq!(lufs_lit_segments(Some(-6.0)), 11);
        assert_eq!(lufs_lit_segments(Some(0.0)), 14);
        assert_eq!(lufs_lit_segments(Some(3.0)), 14);
    }

    #[test]
    fn crowded_cards_reserve_space_for_the_hidden_label_count() {
        assert_eq!(fitting_chips(176., &[55., 67., 85.]), 2);
        assert_eq!(fitting_chips(176., &[110., 110.]), 1);
        assert_eq!(fitting_chips(176., &[55., 67.]), 2);
        assert_eq!(fitting_chips(176., &[]), 0);
        assert_eq!(fitting_chips(110., &[100., 4.]), 2);
        assert_eq!(fitting_chips(20., &[110.]), 0);
        assert_eq!(fitting_chips(84., &[55.; 100]), 1);
        assert_eq!(fitting_chips(84., &[55.; 101]), 0);
    }

    #[test]
    fn planner_card_displays_current_bounce_stage_and_next_action_with_note_fallback() {
        let mut track = crate::storage::Track {
            id: String::from("t1"),
            title: String::from("Peak Hour"),
            original_name: String::from("A2_b2__peak-hour.wav"),
            path: std::path::PathBuf::from("/bounces/A2_b2__peak-hour.wav"),
            stage: crate::storage::TrackStage::Mixdown,
            group_key: Some(String::from("A2")),
            notes: vec![crate::storage::Note {
                id: String::from("n1"),
                time_millis: 10_000,
                body: String::from("Check DJ outro bars"),
                done: false,
            }]
            .into(),
            ..Default::default()
        };
        assert_eq!(track.current_bounce_badge(), "A2 · b2");
        assert_eq!(
            track.display_next_action().as_deref(),
            Some("Check DJ outro bars")
        );

        track.next_action = String::from("Trim 4 bars before drop");
        assert_eq!(
            track.display_next_action().as_deref(),
            Some("Trim 4 bars before drop")
        );

        track.next_action.clear();
        track.notes[0].done = true;
        assert_eq!(track.display_next_action(), None);
    }
}

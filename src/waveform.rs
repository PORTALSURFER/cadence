//! Retained native review waveform and its timestamp interaction surface.
//!
//! The audio decoder owns the immutable signal summary. This module owns only
//! the lightweight Radiant widget that paints that summary and reports a
//! normalized review position back to the Cadence reducer.

use crate::audio::WaveformData;
use radiant::{
    gui::types::{Point, Rect, Rgba8},
    layout::LayoutOutput,
    prelude as ui,
    runtime::{PaintFillPolygon, PaintFillRect, PaintPrimitive, PaintStrokePolygon},
    theme::ThemeTokens,
    widgets::{
        FocusBehavior, PaintBounds, PointerButton, PointerCapturePolicy, Widget, WidgetCommon,
        WidgetInput, WidgetOutput,
    },
};
use std::sync::Arc;

const COMMENT_RAIL_RATIO: f32 = 0.74;
const MARKER_RADIUS: f32 = 4.5;
const DRAFT_MARKER_RADIUS: f32 = 5.5;
const COMMENT_DRAG_HIT_RADIUS: f32 = 7.0;
const NOTE_HOVER_RADIUS: f32 = MARKER_RADIUS + COMMENT_DRAG_HIT_RADIUS;
const NOTE_RATIO_MATCH_EPSILON: f32 = 0.0001;
const CURSOR_WIDTH: f32 = 2.0;
const CURSOR_GAP_ABOVE_RAIL: f32 = 7.0;
const MAX_DISPLAY_BAR_COUNT: usize = 512;
const BAR_PITCH: f32 = 4.0;
const BAR_GAP: f32 = 0.0;
const REFERENCE_DRAG_THRESHOLD: f32 = 4.0;
const TIMELINE_START_HIT_SLOP: f32 = 10.0;
pub const REFERENCE_START_HIT_SLOP: f32 = TIMELINE_START_HIT_SLOP;
const EXTREMA_DISPLAY_BAND_INDEX: usize = 0;
const RMS_DISPLAY_BAND_INDEX: usize = 1;

const NOTE_HOVER_OUTLINE_WIDTH: f32 = 3.0;

#[derive(Clone, Copy)]
enum BarTone {
    Primary,
    Lower,
    Reference,
}

#[derive(Clone, Copy)]
struct BarPaintStyle {
    cursor_ratio: Option<f32>,
    hovered: bool,
    clip: Rect,
    tone: BarTone,
}

#[derive(Clone, Copy)]
struct WaveformColors {
    lower_background: Rgba8,
    bar: Rgba8,
    bar_hover: Rgba8,
    bar_played: Rgba8,
    bar_played_hover: Rgba8,
    lower: Rgba8,
    lower_hover: Rgba8,
    lower_played: Rgba8,
    lower_played_hover: Rgba8,
    reference: Rgba8,
    reference_selection_fill: Rgba8,
    reference_selection_edge: Rgba8,
    rail: Rgba8,
    note_fill: Rgba8,
    note_outline: Rgba8,
    note_hover_fill: Rgba8,
    note_hover_outline: Rgba8,
    cursor: Rgba8,
}

impl WaveformColors {
    fn from_theme(theme: &ThemeTokens) -> Self {
        Self {
            lower_background: theme.bg_secondary,
            bar: theme.highlight_orange_soft,
            bar_hover: theme.text_muted,
            bar_played: theme.highlight_orange,
            bar_played_hover: theme.accent_mint,
            lower: theme.text_muted.with_alpha(160),
            lower_hover: theme.text_muted,
            lower_played: theme.highlight_orange_soft,
            lower_played_hover: theme.accent_mint,
            reference: theme.highlight_blue_soft,
            reference_selection_fill: theme.accent_mint.with_alpha(72),
            reference_selection_edge: theme.accent_mint,
            rail: theme.grid_strong,
            note_fill: theme.bg_primary,
            note_outline: theme.text_primary,
            note_hover_fill: theme.accent_warning,
            note_hover_outline: theme.text_primary,
            cursor: theme.highlight_orange_soft,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct TimelineSurface {
    start_hit_slop: f32,
}

impl TimelineSurface {
    fn new() -> Self {
        Self {
            start_hit_slop: REFERENCE_START_HIT_SLOP,
        }
    }

    fn plot_bounds(self, bounds: Rect) -> Rect {
        if !bounds.is_finite() {
            return bounds;
        }
        let start_x = (bounds.min.x + self.start_hit_slop).min(bounds.max.x);
        Rect::from_min_max(Point::new(start_x, bounds.min.y), bounds.max)
    }

    fn start_edge_contains(self, bounds: Rect, position: Point) -> bool {
        let plot_bounds = self.plot_bounds(bounds);
        position.y >= bounds.min.y
            && position.y <= bounds.max.y
            && position.x >= bounds.min.x
            && position.x < plot_bounds.min.x
    }

    fn interactive_contains(self, bounds: Rect, position: Point) -> bool {
        if !bounds.is_finite() {
            return false;
        }
        self.plot_bounds(bounds).contains(position) || self.start_edge_contains(bounds, position)
    }

    fn ratio_at(self, bounds: Rect, position: Point) -> f32 {
        let plot_bounds = self.plot_bounds(bounds);
        let width = plot_bounds.width();
        if !plot_bounds.is_finite() || !position.x.is_finite() || width <= 0.0 {
            return 0.0;
        }
        clamp_ratio((position.x - plot_bounds.min.x) / width)
    }

    fn x_at(self, bounds: Rect, ratio: f32) -> f32 {
        let plot_bounds = self.plot_bounds(bounds);
        if !plot_bounds.is_finite() {
            return 0.0;
        }
        plot_bounds.x_for_ratio(clamp_ratio(ratio))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaveformInteraction {
    PlayheadDragStarted {
        ratio: f32,
    },
    PlayheadDragMoved {
        ratio: f32,
    },
    PlayheadDragEnded {
        ratio: f32,
    },
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
    Clicked {
        ratio: f32,
        lower: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReferenceWaveformInteraction {
    Clicked { ratio: f32 },
    CommentClicked { ratio: f32 },
    Started { ratio: f32 },
    Moved { ratio: f32 },
    Ended { start_ratio: f32, end_ratio: f32 },
}

#[allow(dead_code)]
pub fn view<Message: 'static>(
    waveform: Arc<WaveformData>,
    cursor_ratio: Option<f32>,
    draft_ratio: Option<f32>,
    note_ratios: Vec<(f32, bool)>,
    hovered_note_ratio: Option<f32>,
    selected_note_ratio: Option<f32>,
    map: impl Fn(WaveformInteraction) -> Message + 'static,
) -> ui::View<Message> {
    view_with_progress(
        waveform,
        cursor_ratio,
        draft_ratio,
        note_ratios,
        hovered_note_ratio,
        selected_note_ratio,
        None,
        map,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn view_with_progress<Message: 'static>(
    waveform: Arc<WaveformData>,
    cursor_ratio: Option<f32>,
    draft_ratio: Option<f32>,
    note_ratios: Vec<(f32, bool)>,
    hovered_note_ratio: Option<f32>,
    selected_note_ratio: Option<f32>,
    visible_ratio: Option<f32>,
    map: impl Fn(WaveformInteraction) -> Message + 'static,
) -> ui::View<Message> {
    ui::custom_widget_mapped(
        WaveformWidget::new(waveform, cursor_ratio, note_ratios)
            .with_draft_ratio(draft_ratio)
            .with_external_hovered_note_ratio(hovered_note_ratio)
            .with_external_selected_note_ratio(selected_note_ratio)
            .with_visible_ratio(visible_ratio),
        map,
    )
}

/// Build a reference waveform for a track's external reference.
///
/// The reference's shared cursor and range can be painted to define a
/// synchronized loop for the shared transport. Use
/// [`reference_view_with_comments`] when the reference comment rail is needed.
#[allow(dead_code)]
pub fn reference_view<Message: 'static>(
    waveform: Arc<WaveformData>,
    cursor_ratio: Option<f32>,
    loop_selection: Option<(f32, f32)>,
    map: impl Fn(ReferenceWaveformInteraction) -> Message + 'static,
) -> ui::View<Message> {
    reference_view_with_progress(waveform, cursor_ratio, loop_selection, None, map)
}

pub fn reference_view_with_progress<Message: 'static>(
    waveform: Arc<WaveformData>,
    cursor_ratio: Option<f32>,
    loop_selection: Option<(f32, f32)>,
    visible_ratio: Option<f32>,
    map: impl Fn(ReferenceWaveformInteraction) -> Message + 'static,
) -> ui::View<Message> {
    reference_view_with_comments(
        waveform,
        cursor_ratio,
        loop_selection,
        visible_ratio,
        Vec::new(),
        None,
        None,
        None,
        map,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn reference_view_with_comments<Message: 'static>(
    waveform: Arc<WaveformData>,
    cursor_ratio: Option<f32>,
    loop_selection: Option<(f32, f32)>,
    visible_ratio: Option<f32>,
    note_ratios: Vec<(f32, bool)>,
    draft_ratio: Option<f32>,
    hovered_note_ratio: Option<f32>,
    selected_note_ratio: Option<f32>,
    map: impl Fn(ReferenceWaveformInteraction) -> Message + 'static,
) -> ui::View<Message> {
    ui::custom_widget_mapped(
        ReferenceWaveformWidget::new(waveform, cursor_ratio, loop_selection)
            .with_note_ratios(note_ratios)
            .with_draft_ratio(draft_ratio)
            .with_external_hovered_note_ratio(hovered_note_ratio)
            .with_selected_note_ratio(selected_note_ratio)
            .with_visible_ratio(visible_ratio),
        map,
    )
}

#[derive(Clone, Debug)]
struct WaveformWidget {
    common: WidgetCommon,
    timeline: TimelineSurface,
    summary: Arc<radiant::runtime::GpuSignalSummary>,
    cursor_ratio: Option<f32>,
    note_ratios: Vec<(f32, bool)>,
    draft_ratio: Option<f32>,
    external_hovered_note_ratio: Option<f32>,
    external_selected_note_ratio: Option<f32>,
    hover_ratio: Option<f32>,
    hover_lower: bool,
    hovered_note_ratio: Option<f32>,
    playhead_dragging: bool,
    comment_dragging: bool,
    comment_drag_note_index: Option<usize>,
    visible_ratio: Option<f32>,
}

impl WaveformWidget {
    fn new(
        waveform: Arc<WaveformData>,
        cursor_ratio: Option<f32>,
        note_ratios: Vec<(f32, bool)>,
    ) -> Self {
        let mut common = WidgetCommon::fixed(0, 640.0, 240.0);
        common.focus = FocusBehavior::Pointer;
        common.paint.bounds = PaintBounds::ClipToRect;
        common.paint.paints_focus = false;
        common.paint.paints_state_layers = false;
        Self {
            common,
            timeline: TimelineSurface::new(),
            summary: Arc::clone(&waveform.summary),
            cursor_ratio: cursor_ratio.map(clamp_ratio),
            note_ratios,
            draft_ratio: None,
            external_hovered_note_ratio: None,
            external_selected_note_ratio: None,
            hover_ratio: None,
            hover_lower: false,
            hovered_note_ratio: None,
            playhead_dragging: false,
            comment_dragging: false,
            comment_drag_note_index: None,
            visible_ratio: None,
        }
    }

    fn with_draft_ratio(mut self, draft_ratio: Option<f32>) -> Self {
        self.draft_ratio = draft_ratio.map(clamp_ratio);
        self
    }

    fn with_external_hovered_note_ratio(mut self, ratio: Option<f32>) -> Self {
        self.external_hovered_note_ratio = ratio.map(clamp_ratio);
        self
    }

    fn with_visible_ratio(mut self, ratio: Option<f32>) -> Self {
        self.visible_ratio = ratio.map(clamp_ratio);
        self
    }

    fn with_external_selected_note_ratio(mut self, ratio: Option<f32>) -> Self {
        self.external_selected_note_ratio = ratio.map(clamp_ratio);
        self
    }

    fn lower_from_position(bounds: Rect, position: Point) -> bool {
        if bounds.height() <= 0.0 {
            return false;
        }
        position.y >= comment_rail_y(bounds)
    }

    fn draft_marker_hit(&self, bounds: Rect, position: Point) -> bool {
        let Some(ratio) = self.draft_ratio else {
            return false;
        };
        if !Self::lower_from_position(bounds, position) {
            return false;
        }
        let marker_x = self.timeline.x_at(bounds, ratio);
        let rail_y = comment_rail_y(bounds);
        let hit_radius = DRAFT_MARKER_RADIUS + COMMENT_DRAG_HIT_RADIUS;
        (position.x - marker_x).abs() <= hit_radius && (position.y - rail_y).abs() <= hit_radius
    }

    fn persisted_note_hit(&self, bounds: Rect, position: Point) -> Option<(usize, f32)> {
        if !bounds.contains(position) || !Self::lower_from_position(bounds, position) {
            return None;
        }

        let rail_y = comment_rail_y(bounds);
        let hover_radius_squared = NOTE_HOVER_RADIUS * NOTE_HOVER_RADIUS;
        self.note_ratios
            .iter()
            .enumerate()
            .filter_map(|(index, (ratio, _))| {
                let ratio = clamp_ratio(*ratio);
                let dx = position.x - self.timeline.x_at(bounds, ratio);
                let dy = position.y - rail_y;
                let distance_squared = dx * dx + dy * dy;
                (distance_squared <= hover_radius_squared).then_some((
                    distance_squared,
                    index,
                    ratio,
                ))
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, index, ratio)| (index, ratio))
    }

    fn persisted_note_near_position(&self, bounds: Rect, position: Point) -> Option<f32> {
        self.persisted_note_hit(bounds, position)
            .map(|(_, ratio)| ratio)
    }

    fn matching_note_ratio(&self, target: Option<f32>) -> Option<f32> {
        let target = target?;
        self.note_ratios.iter().find_map(|(ratio, _)| {
            let ratio = clamp_ratio(*ratio);
            ((ratio - target).abs() <= NOTE_RATIO_MATCH_EPSILON).then_some(ratio)
        })
    }

    fn local_hovered_note_ratio(&self) -> Option<f32> {
        self.hover_ratio
            .is_some()
            .then(|| self.matching_note_ratio(self.hovered_note_ratio))
            .flatten()
    }

    fn current_external_hovered_note_ratio(&self) -> Option<f32> {
        self.matching_note_ratio(self.external_hovered_note_ratio)
    }

    fn current_selected_note_ratio(&self) -> Option<f32> {
        self.matching_note_ratio(self.external_selected_note_ratio)
    }

    fn same_note_ratio(target: Option<f32>, ratio: f32) -> bool {
        target.is_some_and(|target| (target - clamp_ratio(ratio)).abs() <= NOTE_RATIO_MATCH_EPSILON)
    }

    fn clear_pointer_state(&mut self) {
        self.playhead_dragging = false;
        self.comment_dragging = false;
        self.comment_drag_note_index = None;
        self.common.state.hovered = false;
        self.hover_ratio = None;
        self.hover_lower = false;
        self.hovered_note_ratio = None;
    }
}

impl Widget for WaveformWidget {
    fn common(&self) -> &WidgetCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut WidgetCommon {
        &mut self.common
    }

    fn accepts_pointer_move(&self) -> bool {
        self.visible_ratio.is_none()
    }

    fn accepts_pointer_input(&self, _input: &WidgetInput) -> bool {
        self.visible_ratio.is_none()
    }

    fn handle_input(&mut self, bounds: Rect, input: WidgetInput) -> Option<WidgetOutput> {
        if self.visible_ratio.is_some() {
            self.clear_pointer_state();
            return None;
        }
        match input {
            WidgetInput::PointerMove { position, .. } => {
                let inside = self.timeline.interactive_contains(bounds, position);
                let ratio = self.timeline.ratio_at(bounds, position);
                self.common.state.hovered = inside;
                if self.comment_dragging {
                    self.hover_ratio = Some(ratio);
                    self.hover_lower = true;
                    self.hovered_note_ratio = None;
                    Some(WidgetOutput::typed(WaveformInteraction::CommentDragMoved {
                        ratio,
                    }))
                } else if self.playhead_dragging {
                    self.hover_ratio = Some(ratio);
                    self.hover_lower = false;
                    self.hovered_note_ratio = None;
                    Some(WidgetOutput::typed(
                        WaveformInteraction::PlayheadDragMoved { ratio },
                    ))
                } else {
                    self.hover_ratio = inside.then_some(ratio);
                    self.hover_lower = inside && Self::lower_from_position(bounds, position);
                    self.hovered_note_ratio = self.persisted_note_near_position(bounds, position);
                    None
                }
            }
            WidgetInput::PointerPress {
                position,
                button: PointerButton::Primary,
                ..
            } if self.timeline.interactive_contains(bounds, position) => {
                let ratio = self.timeline.ratio_at(bounds, position);
                if self.draft_marker_hit(bounds, position) {
                    self.comment_dragging = true;
                    self.comment_drag_note_index = None;
                    self.hover_ratio = Some(ratio);
                    self.hover_lower = true;
                    self.hovered_note_ratio = None;
                    Some(WidgetOutput::typed(
                        WaveformInteraction::CommentDragStarted {
                            ratio,
                            note_index: None,
                        },
                    ))
                } else if let Some((note_index, note_ratio)) =
                    self.persisted_note_hit(bounds, position)
                {
                    self.comment_dragging = true;
                    self.comment_drag_note_index = Some(note_index);
                    self.hover_ratio = Some(ratio);
                    self.hover_lower = true;
                    self.hovered_note_ratio = Some(note_ratio);
                    Some(WidgetOutput::typed(
                        WaveformInteraction::CommentDragStarted {
                            ratio,
                            note_index: Some(note_index),
                        },
                    ))
                } else if Self::lower_from_position(bounds, position) {
                    self.comment_dragging = true;
                    self.comment_drag_note_index = None;
                    self.hover_ratio = Some(ratio);
                    self.hover_lower = true;
                    self.hovered_note_ratio = self.persisted_note_near_position(bounds, position);
                    Some(WidgetOutput::typed(WaveformInteraction::Clicked {
                        ratio,
                        lower: true,
                    }))
                } else {
                    self.playhead_dragging = true;
                    self.hover_ratio = Some(ratio);
                    self.hover_lower = false;
                    self.hovered_note_ratio = None;
                    Some(WidgetOutput::typed(
                        WaveformInteraction::PlayheadDragStarted { ratio },
                    ))
                }
            }
            WidgetInput::PointerRelease {
                position,
                button: PointerButton::Primary,
                ..
            }
            | WidgetInput::PointerDrop {
                position,
                button: PointerButton::Primary,
                ..
            } if self.comment_dragging => {
                let ratio = self.timeline.ratio_at(bounds, position);
                self.comment_dragging = false;
                self.comment_drag_note_index = None;
                self.common.state.hovered = self.timeline.interactive_contains(bounds, position);
                self.hover_ratio = self.common.state.hovered.then_some(ratio);
                self.hover_lower = false;
                self.hovered_note_ratio = self.persisted_note_near_position(bounds, position);
                Some(WidgetOutput::typed(WaveformInteraction::CommentDragEnded {
                    ratio,
                }))
            }
            WidgetInput::PointerRelease {
                position,
                button: PointerButton::Primary,
                ..
            }
            | WidgetInput::PointerDrop {
                position,
                button: PointerButton::Primary,
                ..
            } if self.playhead_dragging => {
                let ratio = self.timeline.ratio_at(bounds, position);
                self.playhead_dragging = false;
                self.common.state.hovered = self.timeline.interactive_contains(bounds, position);
                self.hover_ratio = self.common.state.hovered.then_some(ratio);
                self.hover_lower = false;
                self.hovered_note_ratio = self.persisted_note_near_position(bounds, position);
                Some(WidgetOutput::typed(
                    WaveformInteraction::PlayheadDragEnded { ratio },
                ))
            }
            _ => None,
        }
    }

    fn handle_pointer_capture_cancelled(&mut self, _bounds: Rect) -> Option<WidgetOutput> {
        let comment_was_active = self.comment_dragging;
        self.clear_pointer_state();
        comment_was_active.then(|| WidgetOutput::typed(WaveformInteraction::CommentDragCancelled))
    }

    fn pointer_capture_policy(&self) -> PointerCapturePolicy {
        PointerCapturePolicy::Exclusive
    }

    fn synchronize_from_previous(&mut self, previous: &dyn Widget) {
        let Some(previous) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.common.state = previous.common.state;
        self.hover_ratio = previous.hover_ratio;
        self.hover_lower = previous.hover_lower;
        self.hovered_note_ratio = previous.hovered_note_ratio.and_then(|ratio| {
            self.note_ratios.iter().find_map(|(candidate, _)| {
                let candidate = clamp_ratio(*candidate);
                ((candidate - ratio).abs() <= NOTE_RATIO_MATCH_EPSILON).then_some(candidate)
            })
        });
        self.playhead_dragging = previous.playhead_dragging;
        self.comment_dragging = previous.comment_dragging;
        self.comment_drag_note_index = previous
            .comment_drag_note_index
            .filter(|index| *index < self.note_ratios.len());
        if self.visible_ratio.is_some() {
            self.clear_pointer_state();
        }
    }

    fn prefers_pointer_move_paint_only(&self) -> bool {
        true
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

        let colors = WaveformColors::from_theme(theme);
        let plot_bounds = self.timeline.plot_bounds(bounds);
        let rail_y = comment_rail_y(bounds);
        let bar_bounds = visible_bounds(plot_bounds, self.visible_ratio);
        let upper_bounds = Rect::from_min_max(bar_bounds.min, Point::new(bar_bounds.max.x, rail_y));
        let lower_bounds =
            Rect::from_min_max(Point::new(bar_bounds.min.x, rail_y + 1.0), bar_bounds.max);
        let bar_levels = display_bar_levels(&self.summary, display_bar_count(bar_bounds.width()));
        fill_rect(
            primitives,
            self.common.id,
            Rect::from_min_max(Point::new(bounds.min.x, rail_y + 1.0), bounds.max),
            colors.lower_background,
        );
        paint_bars(
            primitives,
            self.common.id,
            bar_bounds,
            &bar_levels,
            colors,
            BarPaintStyle {
                cursor_ratio: self.cursor_ratio,
                hovered: self.common.state.hovered,
                clip: upper_bounds,
                tone: BarTone::Primary,
            },
        );
        paint_bars(
            primitives,
            self.common.id,
            bar_bounds,
            &bar_levels,
            colors,
            BarPaintStyle {
                cursor_ratio: self.cursor_ratio,
                hovered: self.common.state.hovered,
                clip: lower_bounds,
                tone: BarTone::Lower,
            },
        );

        fill_rect(
            primitives,
            self.common.id,
            Rect::from_min_max(
                Point::new(bounds.min.x, rail_y - 1.0),
                Point::new(bounds.max.x, rail_y + 1.0),
            ),
            colors.rail,
        );

        if self.visible_ratio.is_some() {
            return;
        }

        let local_hovered_note_ratio = self.local_hovered_note_ratio();
        let external_hovered_note_ratio = if self.hover_ratio.is_some() {
            None
        } else {
            self.current_external_hovered_note_ratio()
        };
        let selected_note_ratio = self.current_selected_note_ratio();
        let mut coalesced_notes: Vec<(f32, bool)> = Vec::with_capacity(self.note_ratios.len());
        for (ratio, done) in &self.note_ratios {
            let ratio = clamp_ratio(*ratio);
            if let Some((_, coalesced_done)) = coalesced_notes
                .iter_mut()
                .find(|(candidate, _)| (ratio - *candidate).abs() <= NOTE_RATIO_MATCH_EPSILON)
            {
                // A collocated marker is done only when every note is done, so
                // an open note always wins regardless of input order.
                *coalesced_done &= *done;
            } else {
                coalesced_notes.push((ratio, *done));
            }
        }
        for (ratio, _) in coalesced_notes {
            let externally_highlighted = Self::same_note_ratio(external_hovered_note_ratio, ratio)
                || (Self::same_note_ratio(selected_note_ratio, ratio)
                    && !Self::same_note_ratio(local_hovered_note_ratio, ratio));
            paint_note_marker(
                primitives,
                self.common.id,
                plot_bounds,
                rail_y,
                NoteMarkerStyle {
                    ratio,
                    radius: MARKER_RADIUS,
                    fill_color: if externally_highlighted {
                        colors.note_hover_fill
                    } else {
                        colors.note_fill
                    },
                    outline_color: colors.note_outline,
                    outline_width: if externally_highlighted {
                        NOTE_HOVER_OUTLINE_WIDTH
                    } else {
                        2.0
                    },
                },
            );
        }

        if let Some(ratio) = self.draft_ratio {
            paint_note_marker(
                primitives,
                self.common.id,
                plot_bounds,
                rail_y,
                NoteMarkerStyle {
                    ratio,
                    radius: DRAFT_MARKER_RADIUS,
                    fill_color: colors.note_fill,
                    outline_color: colors.note_outline,
                    outline_width: 3.0,
                },
            );
        }

        if let Some(ratio) = self.cursor_ratio {
            paint_cursor(
                primitives,
                self.common.id,
                plot_bounds,
                ratio,
                rail_y,
                colors.cursor,
            );
        }
    }

    fn append_runtime_overlay_paint(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        bounds: Rect,
        _layout: &LayoutOutput,
        theme: &ThemeTokens,
    ) {
        if self.visible_ratio.is_some() {
            return;
        }
        let colors = WaveformColors::from_theme(theme);
        let plot_bounds = self.timeline.plot_bounds(bounds);
        let rail_y = comment_rail_y(bounds);
        let hovered_note_ratio = self.local_hovered_note_ratio();
        if let Some(ratio) = self.hover_ratio {
            let x = self.timeline.x_at(bounds, ratio);
            let line_bottom = rail_y - CURSOR_GAP_ABOVE_RAIL;
            fill_rect(
                primitives,
                self.common.id,
                Rect::from_min_max(
                    Point::new(x - CURSOR_WIDTH * 0.5, bounds.min.y),
                    Point::new(x + CURSOR_WIDTH * 0.5, line_bottom),
                ),
                colors.cursor,
            );
            if self.hover_lower && hovered_note_ratio.is_none() {
                fill_rect(
                    primitives,
                    self.common.id,
                    marker_rect(x, rail_y, MARKER_RADIUS),
                    colors.note_outline,
                );
            }
        }
        if let Some(ratio) = hovered_note_ratio {
            paint_highlighted_note_marker(
                primitives,
                self.common.id,
                plot_bounds,
                rail_y,
                ratio,
                colors,
            );
        }
    }
}

#[derive(Clone, Copy)]
struct NoteMarkerStyle {
    ratio: f32,
    radius: f32,
    fill_color: Rgba8,
    outline_color: Rgba8,
    outline_width: f32,
}

fn paint_note_marker(
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    bounds: Rect,
    rail_y: f32,
    style: NoteMarkerStyle,
) {
    let x = bounds.x_for_ratio(style.ratio);
    let marker_points = rounded_corner_points(marker_rect(x, rail_y, style.radius), 3.0);
    primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
        widget_id,
        points: marker_points.clone(),
        color: style.fill_color,
    }));
    primitives.push(PaintPrimitive::StrokePolygon(PaintStrokePolygon {
        widget_id,
        points: marker_points,
        color: style.outline_color,
        width: style.outline_width,
    }));
}

fn paint_highlighted_note_marker(
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    bounds: Rect,
    rail_y: f32,
    ratio: f32,
    colors: WaveformColors,
) {
    paint_note_marker(
        primitives,
        widget_id,
        bounds,
        rail_y,
        NoteMarkerStyle {
            ratio,
            radius: MARKER_RADIUS,
            fill_color: colors.note_hover_fill,
            outline_color: colors.note_hover_outline,
            outline_width: NOTE_HOVER_OUTLINE_WIDTH,
        },
    );
}

fn rounded_corner_points(bounds: Rect, radius: f32) -> std::sync::Arc<[Point]> {
    let radius = radius
        .max(0.0)
        .min(bounds.width().min(bounds.height()) * 0.5);
    [
        Point::new(bounds.min.x + radius, bounds.min.y),
        Point::new(bounds.max.x - radius, bounds.min.y),
        Point::new(bounds.max.x, bounds.min.y + radius),
        Point::new(bounds.max.x, bounds.max.y - radius),
        Point::new(bounds.max.x - radius, bounds.max.y),
        Point::new(bounds.min.x + radius, bounds.max.y),
        Point::new(bounds.min.x, bounds.max.y - radius),
        Point::new(bounds.min.x, bounds.min.y + radius),
    ]
    .into()
}

#[derive(Clone, Debug)]
struct ReferenceWaveformWidget {
    common: WidgetCommon,
    timeline: TimelineSurface,
    summary: Arc<radiant::runtime::GpuSignalSummary>,
    cursor_ratio: Option<f32>,
    loop_selection: Option<(f32, f32)>,
    pointer_down_position: Option<Point>,
    pointer_down_ratio: Option<f32>,
    loop_drag_start_ratio: Option<f32>,
    loop_drag_current_ratio: Option<f32>,
    external_hovered_note_ratio: Option<f32>,
    hover_ratio: Option<f32>,
    hover_lower: bool,
    hovered_note_ratio: Option<f32>,
    note_ratios: Vec<(f32, bool)>,
    draft_ratio: Option<f32>,
    selected_note_ratio: Option<f32>,
    comment_pointer_down: bool,
    visible_ratio: Option<f32>,
}

impl ReferenceWaveformWidget {
    fn new(
        waveform: Arc<WaveformData>,
        cursor_ratio: Option<f32>,
        loop_selection: Option<(f32, f32)>,
    ) -> Self {
        let mut common = WidgetCommon::fixed(0, 640.0, 76.0);
        common.focus = FocusBehavior::Pointer;
        common.paint.bounds = PaintBounds::ClipToRect;
        common.paint.paints_focus = false;
        common.paint.paints_state_layers = false;
        Self {
            common,
            timeline: TimelineSurface::new(),
            summary: Arc::clone(&waveform.summary),
            cursor_ratio: cursor_ratio.map(clamp_ratio),
            loop_selection: loop_selection.map(normalize_range),
            pointer_down_position: None,
            pointer_down_ratio: None,
            loop_drag_start_ratio: None,
            loop_drag_current_ratio: None,
            external_hovered_note_ratio: None,
            hover_ratio: None,
            hover_lower: false,
            hovered_note_ratio: None,
            note_ratios: Vec::new(),
            draft_ratio: None,
            selected_note_ratio: None,
            comment_pointer_down: false,
            visible_ratio: None,
        }
    }

    fn with_note_ratios(mut self, note_ratios: Vec<(f32, bool)>) -> Self {
        self.note_ratios = note_ratios;
        self
    }

    fn with_draft_ratio(mut self, ratio: Option<f32>) -> Self {
        self.draft_ratio = ratio.map(clamp_ratio);
        self
    }

    fn with_external_hovered_note_ratio(mut self, ratio: Option<f32>) -> Self {
        self.external_hovered_note_ratio = ratio.map(clamp_ratio);
        self
    }

    fn with_selected_note_ratio(mut self, ratio: Option<f32>) -> Self {
        self.selected_note_ratio = ratio.map(clamp_ratio);
        self
    }

    fn comment_rail_contains(bounds: Rect, position: Point) -> bool {
        position.y >= comment_rail_y(bounds) && bounds.contains(position)
    }

    fn persisted_note_hit(&self, bounds: Rect, position: Point) -> Option<(usize, f32)> {
        if !bounds.contains(position) {
            return None;
        }

        let rail_y = comment_rail_y(bounds);
        let hover_radius_squared = NOTE_HOVER_RADIUS * NOTE_HOVER_RADIUS;
        self.note_ratios
            .iter()
            .enumerate()
            .filter_map(|(index, (ratio, _))| {
                let ratio = clamp_ratio(*ratio);
                let dx = position.x - self.timeline.x_at(bounds, ratio);
                let dy = position.y - rail_y;
                let distance_squared = dx * dx + dy * dy;
                (distance_squared <= hover_radius_squared).then_some((
                    distance_squared,
                    index,
                    ratio,
                ))
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, index, ratio)| (index, ratio))
    }

    fn persisted_note_near_position(&self, bounds: Rect, position: Point) -> Option<f32> {
        self.persisted_note_hit(bounds, position)
            .map(|(_, ratio)| ratio)
    }

    fn matching_note_ratio(&self, target: Option<f32>) -> Option<f32> {
        let target = target?;
        self.note_ratios.iter().find_map(|(ratio, _)| {
            let ratio = clamp_ratio(*ratio);
            ((ratio - target).abs() <= NOTE_RATIO_MATCH_EPSILON).then_some(ratio)
        })
    }

    fn local_hovered_note_ratio(&self) -> Option<f32> {
        self.hover_ratio
            .is_some()
            .then(|| self.matching_note_ratio(self.hovered_note_ratio))
            .flatten()
    }

    fn current_external_hovered_note_ratio(&self) -> Option<f32> {
        self.matching_note_ratio(self.external_hovered_note_ratio)
    }

    fn same_note_ratio(target: Option<f32>, ratio: f32) -> bool {
        target.is_some_and(|target| (target - clamp_ratio(ratio)).abs() <= NOTE_RATIO_MATCH_EPSILON)
    }

    fn active_loop_selection(&self) -> Option<(f32, f32)> {
        self.loop_drag_start_ratio
            .zip(self.loop_drag_current_ratio)
            .map(normalize_range)
            .or(self.loop_selection)
    }

    fn movement_exceeded(start: Point, current: Point) -> bool {
        let delta_x = current.x - start.x;
        let delta_y = current.y - start.y;
        delta_x * delta_x + delta_y * delta_y > REFERENCE_DRAG_THRESHOLD.powi(2)
    }

    fn with_visible_ratio(mut self, ratio: Option<f32>) -> Self {
        self.visible_ratio = ratio.map(clamp_ratio).filter(|ratio| *ratio < 1.0);
        self
    }

    fn clear_pointer_state(&mut self) {
        self.pointer_down_position = None;
        self.pointer_down_ratio = None;
        self.loop_drag_start_ratio = None;
        self.loop_drag_current_ratio = None;
        self.comment_pointer_down = false;
        self.common.state.hovered = false;
        self.hover_ratio = None;
        self.hover_lower = false;
        self.hovered_note_ratio = None;
    }
}

impl Widget for ReferenceWaveformWidget {
    fn common(&self) -> &WidgetCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut WidgetCommon {
        &mut self.common
    }

    fn accepts_pointer_move(&self) -> bool {
        self.visible_ratio.is_none()
    }

    fn accepts_pointer_input(&self, _input: &WidgetInput) -> bool {
        self.visible_ratio.is_none()
    }

    fn handle_input(&mut self, bounds: Rect, input: WidgetInput) -> Option<WidgetOutput> {
        if self.visible_ratio.is_some() {
            self.clear_pointer_state();
            return None;
        }
        match input {
            WidgetInput::PointerMove { position, .. } => {
                let ratio = self.timeline.ratio_at(bounds, position);
                let inside = self.timeline.interactive_contains(bounds, position);
                let hover_lower = inside && Self::comment_rail_contains(bounds, position);
                self.common.state.hovered = inside;
                if self.comment_pointer_down {
                    self.hover_ratio = inside.then_some(ratio);
                    self.hover_lower = hover_lower;
                    self.hovered_note_ratio = self.persisted_note_near_position(bounds, position);
                    None
                } else if self.loop_drag_start_ratio.is_some() {
                    self.loop_drag_current_ratio = Some(ratio);
                    self.hover_ratio = Some(ratio);
                    self.hover_lower = false;
                    self.hovered_note_ratio = None;
                    Some(WidgetOutput::typed(ReferenceWaveformInteraction::Moved {
                        ratio,
                    }))
                } else if self
                    .pointer_down_position
                    .is_some_and(|start| Self::movement_exceeded(start, position))
                {
                    let start_ratio = self.pointer_down_ratio.unwrap_or(ratio);
                    self.loop_drag_start_ratio = Some(start_ratio);
                    self.loop_drag_current_ratio = Some(ratio);
                    self.hover_ratio = Some(ratio);
                    self.hover_lower = false;
                    self.hovered_note_ratio = None;
                    Some(WidgetOutput::typed(ReferenceWaveformInteraction::Started {
                        ratio: start_ratio,
                    }))
                } else {
                    self.hover_ratio = inside.then_some(ratio);
                    self.hover_lower = hover_lower;
                    self.hovered_note_ratio = self.persisted_note_near_position(bounds, position);
                    None
                }
            }
            WidgetInput::PointerPress {
                position,
                button: PointerButton::Primary,
                ..
            } if self.timeline.interactive_contains(bounds, position) => {
                let ratio = self.timeline.ratio_at(bounds, position);
                self.pointer_down_position = Some(position);
                self.pointer_down_ratio = Some(ratio);
                self.comment_pointer_down = Self::comment_rail_contains(bounds, position);
                self.loop_drag_start_ratio = None;
                self.loop_drag_current_ratio = None;
                self.common.state.hovered = true;
                self.hover_ratio = Some(ratio);
                self.hover_lower = self.comment_pointer_down;
                self.hovered_note_ratio = self.persisted_note_near_position(bounds, position);
                None
            }
            WidgetInput::PointerRelease {
                position,
                button: PointerButton::Primary,
                ..
            }
            | WidgetInput::PointerDrop {
                position,
                button: PointerButton::Primary,
                ..
            } if self.pointer_down_position.is_some() => {
                let end_ratio = self.timeline.ratio_at(bounds, position);
                let pointer_down_position = self.pointer_down_position.take();
                let pointer_down_ratio = self.pointer_down_ratio.take();
                let start_ratio = self.loop_drag_start_ratio.take();
                let comment_pointer_down = self.comment_pointer_down;
                self.comment_pointer_down = false;
                self.loop_drag_current_ratio = None;
                self.common.state.hovered = self.timeline.interactive_contains(bounds, position);
                self.hover_ratio = self.common.state.hovered.then_some(end_ratio);
                self.hover_lower = false;
                self.hovered_note_ratio = self.persisted_note_near_position(bounds, position);
                if comment_pointer_down {
                    Some(WidgetOutput::typed(
                        ReferenceWaveformInteraction::CommentClicked {
                            ratio: pointer_down_ratio.unwrap_or(end_ratio),
                        },
                    ))
                } else if let Some(start_ratio) = start_ratio {
                    Some(WidgetOutput::typed(ReferenceWaveformInteraction::Ended {
                        start_ratio,
                        end_ratio,
                    }))
                } else if pointer_down_position
                    .is_some_and(|start| Self::movement_exceeded(start, position))
                {
                    Some(WidgetOutput::typed(ReferenceWaveformInteraction::Ended {
                        start_ratio: pointer_down_ratio.unwrap_or(end_ratio),
                        end_ratio,
                    }))
                } else {
                    Some(WidgetOutput::typed(ReferenceWaveformInteraction::Clicked {
                        ratio: pointer_down_ratio.unwrap_or(end_ratio),
                    }))
                }
            }
            _ => None,
        }
    }

    fn pointer_capture_policy(&self) -> PointerCapturePolicy {
        PointerCapturePolicy::Exclusive
    }

    fn prefers_pointer_move_paint_only(&self) -> bool {
        true
    }

    fn handle_pointer_capture_cancelled(&mut self, _bounds: Rect) -> Option<WidgetOutput> {
        self.clear_pointer_state();
        None
    }

    fn synchronize_from_previous(&mut self, previous: &dyn Widget) {
        let Some(previous) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.common.state = previous.common.state;
        self.pointer_down_position = previous.pointer_down_position;
        self.pointer_down_ratio = previous.pointer_down_ratio;
        self.loop_drag_start_ratio = previous.loop_drag_start_ratio;
        self.loop_drag_current_ratio = previous.loop_drag_current_ratio;
        self.hover_ratio = previous.hover_ratio;
        self.hover_lower = previous.hover_lower;
        self.hovered_note_ratio = previous.hovered_note_ratio.and_then(|ratio| {
            self.note_ratios.iter().find_map(|candidate| {
                let candidate = clamp_ratio(candidate.0);
                ((candidate - ratio).abs() <= NOTE_RATIO_MATCH_EPSILON).then_some(candidate)
            })
        });
        self.comment_pointer_down = previous.comment_pointer_down;
        if self.visible_ratio.is_some() {
            self.clear_pointer_state();
        }
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
        let colors = WaveformColors::from_theme(theme);
        fill_rect(primitives, self.common.id, bounds, theme.surface_base);
        let plot_bounds = self.timeline.plot_bounds(bounds);
        let bar_bounds = visible_bounds(plot_bounds, self.visible_ratio);
        let rail_y = comment_rail_y(bounds);
        let upper_bounds = Rect::from_min_max(bar_bounds.min, Point::new(bar_bounds.max.x, rail_y));
        let levels = display_bar_levels(&self.summary, display_bar_count(bar_bounds.width()));
        fill_rect(
            primitives,
            self.common.id,
            Rect::from_min_max(Point::new(bounds.min.x, rail_y + 1.0), bounds.max),
            colors.lower_background,
        );
        paint_bars(
            primitives,
            self.common.id,
            upper_bounds,
            &levels,
            colors,
            BarPaintStyle {
                cursor_ratio: self.cursor_ratio,
                hovered: false,
                clip: upper_bounds,
                tone: BarTone::Reference,
            },
        );
        fill_rect(
            primitives,
            self.common.id,
            Rect::from_min_max(
                Point::new(bounds.min.x, rail_y - 1.0),
                Point::new(bounds.max.x, rail_y + 1.0),
            ),
            colors.rail,
        );
        if self.visible_ratio.is_some() {
            return;
        }
        if plot_bounds.has_finite_positive_area()
            && let Some((start_ratio, end_ratio)) = self.active_loop_selection()
        {
            let start_x = self.timeline.x_at(bounds, start_ratio);
            let end_x = self.timeline.x_at(bounds, end_ratio);
            let selection_end_x = end_x.max(start_x + 1.0).min(plot_bounds.max.x);
            if selection_end_x > start_x {
                fill_rect(
                    primitives,
                    self.common.id,
                    Rect::from_min_max(
                        Point::new(start_x, plot_bounds.min.y),
                        Point::new(selection_end_x, plot_bounds.max.y),
                    ),
                    colors.reference_selection_fill,
                );
            }
            let start_edge_end = (start_x + 2.0).min(plot_bounds.max.x);
            if start_edge_end > start_x {
                fill_rect(
                    primitives,
                    self.common.id,
                    Rect::from_min_max(
                        Point::new(start_x, plot_bounds.min.y),
                        Point::new(start_edge_end, plot_bounds.max.y),
                    ),
                    colors.reference_selection_edge,
                );
            }
            let end_edge_start = (end_x - 2.0).max(plot_bounds.min.x);
            if end_x > end_edge_start {
                fill_rect(
                    primitives,
                    self.common.id,
                    Rect::from_min_max(
                        Point::new(end_edge_start, plot_bounds.min.y),
                        Point::new(end_x, plot_bounds.max.y),
                    ),
                    colors.reference_selection_edge,
                );
            }
        }
        if let Some(ratio) = self.cursor_ratio {
            paint_cursor(
                primitives,
                self.common.id,
                plot_bounds,
                ratio,
                plot_bounds.max.y + CURSOR_GAP_ABOVE_RAIL,
                colors.cursor,
            );
        }

        let local_hovered_note_ratio = self.local_hovered_note_ratio();
        let external_hovered_note_ratio = if self.hover_ratio.is_some() {
            None
        } else {
            self.current_external_hovered_note_ratio()
        };
        let mut coalesced_notes: Vec<(f32, bool)> = Vec::with_capacity(self.note_ratios.len());
        for (ratio, done) in &self.note_ratios {
            let ratio = clamp_ratio(*ratio);
            if let Some((_, coalesced_done)) = coalesced_notes
                .iter_mut()
                .find(|(candidate, _)| (ratio - *candidate).abs() <= NOTE_RATIO_MATCH_EPSILON)
            {
                *coalesced_done &= *done;
            } else {
                coalesced_notes.push((ratio, *done));
            }
        }
        for (ratio, _) in coalesced_notes {
            let highlighted = Self::same_note_ratio(external_hovered_note_ratio, ratio)
                || (Self::same_note_ratio(self.selected_note_ratio, ratio)
                    && !Self::same_note_ratio(local_hovered_note_ratio, ratio));
            paint_note_marker(
                primitives,
                self.common.id,
                plot_bounds,
                rail_y,
                NoteMarkerStyle {
                    ratio,
                    radius: MARKER_RADIUS,
                    fill_color: if highlighted {
                        colors.note_hover_fill
                    } else {
                        colors.note_fill
                    },
                    outline_color: colors.note_outline,
                    outline_width: if highlighted {
                        NOTE_HOVER_OUTLINE_WIDTH
                    } else {
                        2.0
                    },
                },
            );
        }
        if let Some(ratio) = self.draft_ratio {
            paint_note_marker(
                primitives,
                self.common.id,
                plot_bounds,
                rail_y,
                NoteMarkerStyle {
                    ratio,
                    radius: DRAFT_MARKER_RADIUS,
                    fill_color: colors.note_fill,
                    outline_color: colors.note_outline,
                    outline_width: 3.0,
                },
            );
        }
    }

    fn append_runtime_overlay_paint(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        bounds: Rect,
        _layout: &LayoutOutput,
        theme: &ThemeTokens,
    ) {
        if self.visible_ratio.is_some() {
            return;
        }
        let colors = WaveformColors::from_theme(theme);
        let plot_bounds = self.timeline.plot_bounds(bounds);
        if let Some(ratio) = self.hover_ratio {
            paint_cursor(
                primitives,
                self.common.id,
                plot_bounds,
                ratio,
                plot_bounds.max.y + CURSOR_GAP_ABOVE_RAIL,
                colors.cursor,
            );
            let rail_y = comment_rail_y(bounds);
            if let Some(ratio) = self.local_hovered_note_ratio() {
                paint_highlighted_note_marker(
                    primitives,
                    self.common.id,
                    plot_bounds,
                    rail_y,
                    ratio,
                    colors,
                );
            } else if self.hover_lower {
                fill_rect(
                    primitives,
                    self.common.id,
                    marker_rect(self.timeline.x_at(bounds, ratio), rail_y, MARKER_RADIUS),
                    colors.note_outline,
                );
            }
        }
    }
}

fn fill_rect(primitives: &mut Vec<PaintPrimitive>, widget_id: u64, rect: Rect, color: Rgba8) {
    if !rect.has_finite_positive_area() {
        return;
    }
    primitives.push(PaintPrimitive::FillRect(PaintFillRect {
        widget_id,
        rect,
        color,
    }));
}

fn visible_bounds(bounds: Rect, visible_ratio: Option<f32>) -> Rect {
    let Some(ratio) = visible_ratio else {
        return bounds;
    };
    let end_x = bounds
        .x_for_ratio(clamp_ratio(ratio))
        .max(bounds.min.x + 1.0)
        .min(bounds.max.x);
    Rect::from_min_max(bounds.min, Point::new(end_x, bounds.max.y))
}

fn display_bar_levels(
    summary: &radiant::runtime::GpuSignalSummary,
    bar_count: usize,
) -> Arc<[f32]> {
    let bar_count = bar_count.max(1);
    // Start from the finest retained level. Selecting a pre-merged max level
    // before display reduction turns a short loud section into a full-height
    // bar and hides the quieter gaps that follow it.
    let Some(level) = summary.levels.first() else {
        return Arc::from(vec![0.0; bar_count]);
    };
    let band_count = summary.band_count.max(1);
    // Decoded summaries use band 0 for lossless extrema and band 1 for
    // per-window mean-square energy. The energy band is the display signal;
    // extrema remain available for detail/peak consumers. One-band synthetic
    // summaries keep their historical amplitude display fallback.
    let display_band = if band_count > RMS_DISPLAY_BAND_INDEX {
        RMS_DISPLAY_BAND_INDEX
    } else {
        EXTREMA_DISPLAY_BAND_INDEX
    };
    let display_band_is_energy = band_count > RMS_DISPLAY_BAND_INDEX;
    let mut source_levels = level
        .buckets
        .chunks(band_count)
        .map(|buckets| {
            buckets
                .get(display_band)
                .or_else(|| buckets.first())
                .map(|bucket| bucket.min.abs().max(bucket.max.abs()))
                .filter(|peak| peak.is_finite())
                .unwrap_or(0.0)
        })
        .collect::<Vec<_>>();
    if source_levels.is_empty() {
        source_levels.push(0.0);
    }

    let source_bucket_count = source_levels.len();
    let raw_levels = if source_bucket_count < bar_count {
        if source_bucket_count == 1 || bar_count == 1 {
            let level = if display_band_is_energy {
                source_levels[0].max(0.0).sqrt()
            } else {
                source_levels[0]
            };
            vec![level; bar_count]
        } else {
            let last_source_index = (source_bucket_count - 1) as f32;
            let last_bar_index = (bar_count - 1) as f32;
            (0..bar_count)
                .map(|bar_index| {
                    let source_position = bar_index as f32 * last_source_index / last_bar_index;
                    let lower_index = source_position.floor() as usize;
                    let upper_index = source_position.ceil() as usize;
                    let fraction = source_position - lower_index as f32;
                    let interpolated = source_levels[lower_index]
                        + (source_levels[upper_index] - source_levels[lower_index]) * fraction;
                    if display_band_is_energy {
                        interpolated.max(0.0).sqrt()
                    } else {
                        interpolated
                    }
                })
                .collect::<Vec<_>>()
        }
    } else {
        (0..bar_count)
            .map(|bar_index| {
                let start = bar_index.saturating_mul(source_bucket_count) / bar_count;
                let end = bar_index
                    .saturating_add(1)
                    .saturating_mul(source_bucket_count)
                    .div_euclid(bar_count)
                    .max(start.saturating_add(1))
                    .min(source_bucket_count);
                let samples = &source_levels[start..end];
                let mean = samples.iter().copied().sum::<f32>() / samples.len() as f32;
                if display_band_is_energy {
                    mean.max(0.0).sqrt()
                } else {
                    mean
                }
            })
            .collect::<Vec<_>>()
    };
    // Keep the display tied to the source amplitude. Combining RMS windows in
    // mean-square space preserves their energy; per-track percentile
    // normalization would instead stretch small differences into artificial
    // spikes. Do not add a display floor: silence stays silent.
    Arc::from(
        raw_levels
            .into_iter()
            .map(|level| level.clamp(0.0, 1.0))
            .collect::<Vec<_>>(),
    )
}

fn display_bar_count(width: f32) -> usize {
    (width.max(1.0) / BAR_PITCH)
        .floor()
        .clamp(1.0, MAX_DISPLAY_BAR_COUNT as f32) as usize
}

fn paint_bars(
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    bounds: Rect,
    levels: &[f32],
    colors: WaveformColors,
    style: BarPaintStyle,
) {
    let count = levels.len().max(1);
    let pitch = bounds.width() / count as f32;
    let width = (pitch - BAR_GAP).max(0.75);
    let bottom = bounds.max.y - 1.0;
    let maximum_height = (bounds.height() - 2.0).max(1.0);
    for (index, level) in levels.iter().enumerate() {
        let x = bounds.min.x + index as f32 * pitch + (pitch - width) * 0.5;
        let height = maximum_height * level.clamp(0.0, 1.0);
        let played = style
            .cursor_ratio
            .is_some_and(|ratio| (index as f32 / count as f32) <= clamp_ratio(ratio));
        let bar_rect = Rect::from_min_max(
            Point::new(x, bottom - height),
            Point::new(x + width, bottom),
        );
        let clipped = Rect::from_min_max(
            Point::new(
                bar_rect.min.x.max(style.clip.min.x),
                bar_rect.min.y.max(style.clip.min.y),
            ),
            Point::new(
                bar_rect.max.x.min(style.clip.max.x),
                bar_rect.max.y.min(style.clip.max.y),
            ),
        );
        if !clipped.has_finite_positive_area() {
            continue;
        }
        let color = match (style.tone, played, style.hovered) {
            (BarTone::Primary, true, true) => colors.bar_played_hover,
            (BarTone::Primary, true, false) => colors.bar_played,
            (BarTone::Primary, false, true) => colors.bar_hover,
            (BarTone::Primary, false, false) => colors.bar,
            (BarTone::Lower, true, true) => colors.lower_played_hover,
            (BarTone::Lower, true, false) => colors.lower_played,
            (BarTone::Lower, false, true) => colors.lower_hover,
            (BarTone::Lower, false, false) => colors.lower,
            (BarTone::Reference, _, _) => colors.reference,
        };
        fill_rect(primitives, widget_id, clipped, color);
    }
}

fn comment_rail_y(bounds: Rect) -> f32 {
    bounds.y_for_ratio(COMMENT_RAIL_RATIO)
}

fn marker_rect(x: f32, y: f32, radius: f32) -> Rect {
    Rect::from_min_max(
        Point::new(x - radius, y - radius),
        Point::new(x + radius, y + radius),
    )
}

fn paint_cursor(
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    bounds: Rect,
    ratio: f32,
    rail_y: f32,
    color: Rgba8,
) {
    let x = bounds.x_for_ratio(ratio);
    fill_rect(
        primitives,
        widget_id,
        Rect::from_min_max(
            Point::new(x - CURSOR_WIDTH * 0.5, bounds.min.y),
            Point::new(x + CURSOR_WIDTH * 0.5, rail_y - CURSOR_GAP_ABOVE_RAIL),
        ),
        color,
    );
}

pub fn clamp_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn normalize_range(range: (f32, f32)) -> (f32, f32) {
    let first = clamp_ratio(range.0);
    let second = clamp_ratio(range.1);
    if first <= second {
        (first, second)
    } else {
        (second, first)
    }
}

pub fn ratio_for_millis(time_millis: u64, duration_millis: u64) -> Option<f32> {
    (duration_millis > 0).then(|| clamp_ratio((time_millis as f64 / duration_millis as f64) as f32))
}

pub fn millis_for_ratio(ratio: f32, duration_millis: u64) -> u64 {
    (clamp_ratio(ratio) as f64 * duration_millis as f64).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::WaveformData;
    use radiant::{
        gui::types::{Point, Rect},
        runtime::{GpuSignalSummary, PaintPrimitive},
        theme::ThemeTokens,
        widgets::{Widget, WidgetInput},
    };
    use std::sync::Arc;

    fn test_waveform() -> WaveformData {
        WaveformData {
            sample_rate: 48_000,
            channels: 1,
            duration_millis: 1_000,
            render_frames: 48_000,
            integrated_lufs: Some(-7.0),
            loudness_profile: Arc::from([]),
            summary: Arc::new(GpuSignalSummary::from_interleaved_samples(
                &[0.1, 0.8, 0.2, 0.4],
                4,
                1,
            )),
        }
    }

    fn interaction(output: Option<radiant::widgets::WidgetOutput>) -> WaveformInteraction {
        output
            .and_then(|output| output.typed_copied())
            .expect("waveform input should emit an interaction")
    }

    fn reference_interaction(
        output: Option<radiant::widgets::WidgetOutput>,
    ) -> ReferenceWaveformInteraction {
        output
            .and_then(|output| output.typed_copied())
            .expect("reference waveform input should emit an interaction")
    }

    fn colors() -> WaveformColors {
        WaveformColors::from_theme(&ThemeTokens::default())
    }

    fn timeline_x(bounds: Rect, ratio: f32) -> f32 {
        TimelineSurface::new().x_at(bounds, ratio)
    }

    fn generic_lower_marker_count(primitives: &[PaintPrimitive], rail_y: f32) -> usize {
        primitives
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    PaintPrimitive::FillRect(fill)
                        if fill.color == colors().note_outline
                            && (fill.rect.width() - MARKER_RADIUS * 2.0).abs() < f32::EPSILON
                            && (fill.rect.height() - MARKER_RADIUS * 2.0).abs() < f32::EPSILON
                            && (fill.rect.min.y - (rail_y - MARKER_RADIUS)).abs() < f32::EPSILON
                )
            })
            .count()
    }

    fn highlighted_note_marker_count(primitives: &[PaintPrimitive]) -> usize {
        primitives
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    PaintPrimitive::StrokePolygon(stroke)
                        if stroke.color == colors().note_hover_outline
                            && (stroke.width - NOTE_HOVER_OUTLINE_WIDTH).abs() < f32::EPSILON
                )
            })
            .count()
    }

    #[test]
    fn comment_zone_is_darker_below_the_raised_rail() {
        let bounds = Rect::from_size(100.0, 100.0);
        let widget = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());
        let paint_plan = widget.paint_plan_with_defaults(bounds);
        let rail_y = comment_rail_y(bounds);

        assert!(rail_y < bounds.max.y);
        assert!(paint_plan.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                radiant::runtime::PaintPrimitive::FillRect(fill)
                    if fill.color == colors().lower_background
                        && fill.rect.min.y > rail_y
                        && fill.rect.max.y == bounds.max.y
            )
        }));
        assert!(paint_plan.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                radiant::runtime::PaintPrimitive::FillRect(fill)
                    if fill.color == colors().lower
                        && fill.rect.min.y > rail_y
                        && fill.rect.max.y < bounds.max.y
            )
        }));
    }

    #[test]
    fn comment_markers_and_cursor_follow_default_theme_tokens() {
        let theme = ThemeTokens::default();
        let colors = colors();
        assert_eq!(colors.note_outline, theme.text_primary);
        assert_eq!(colors.note_fill, theme.bg_primary);
        assert_eq!(colors.note_hover_fill, theme.accent_warning);
        assert_eq!(colors.note_hover_outline, theme.text_primary);
        assert_eq!(colors.cursor, theme.highlight_orange_soft);

        let widget = WaveformWidget::new(
            Arc::new(test_waveform()),
            None,
            vec![(0.2, false), (0.7, true)],
        )
        .with_external_selected_note_ratio(Some(0.7));
        let paint_plan = widget.paint_plan_with_defaults(Rect::from_size(320.0, 100.0));

        let normal_marker_fills = paint_plan
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                radiant::runtime::PaintPrimitive::FillPolygon(fill)
                    if fill.color == colors.note_fill =>
                {
                    Some(fill)
                }
                _ => None,
            })
            .count();
        let highlighted_marker_fills = paint_plan
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                radiant::runtime::PaintPrimitive::FillPolygon(fill)
                    if fill.color == colors.note_hover_fill =>
                {
                    Some(fill)
                }
                _ => None,
            })
            .count();
        let normal_marker_strokes = paint_plan
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                radiant::runtime::PaintPrimitive::StrokePolygon(stroke)
                    if stroke.color == colors.note_outline
                        && (stroke.width - 2.0).abs() < f32::EPSILON =>
                {
                    Some(stroke)
                }
                _ => None,
            })
            .count();
        let highlighted_marker_strokes = paint_plan
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                radiant::runtime::PaintPrimitive::StrokePolygon(stroke)
                    if stroke.color == colors.note_hover_outline
                        && (stroke.width - NOTE_HOVER_OUTLINE_WIDTH).abs() < f32::EPSILON =>
                {
                    Some(stroke)
                }
                _ => None,
            })
            .count();

        assert_eq!(normal_marker_fills, 1);
        assert_eq!(highlighted_marker_fills, 1);
        assert_eq!(normal_marker_strokes, 1);
        assert_eq!(highlighted_marker_strokes, 1);
    }

    #[test]
    fn persisted_comment_hover_highlights_the_nearest_node_without_a_duplicate_marker() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let rail_y = comment_rail_y(bounds);
        let mut widget = WaveformWidget::new(
            Arc::new(test_waveform()),
            None,
            vec![(0.25, false), (0.75, true)],
        );

        assert!(
            widget
                .handle_input(
                    bounds,
                    WidgetInput::pointer_move(Point::new(
                        timeline_x(bounds, 0.25) + 3.0,
                        rail_y + 3.0,
                    )),
                )
                .is_none()
        );
        assert_eq!(widget.hovered_note_ratio, Some(0.25));
        assert!(widget.hover_lower);

        let mut overlay = Vec::new();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(
            overlay
                .iter()
                .filter(|primitive| matches!(
                    primitive,
                    PaintPrimitive::FillPolygon(fill) if fill.color == colors().note_hover_fill
                ))
                .count(),
            1
        );
        assert_eq!(
            overlay
                .iter()
                .filter(|primitive| matches!(
                    primitive,
                    PaintPrimitive::StrokePolygon(stroke)
                        if stroke.color == colors().note_hover_outline
                            && (stroke.width - NOTE_HOVER_OUTLINE_WIDTH).abs() < f32::EPSILON
                ))
                .count(),
            1
        );
        assert_eq!(generic_lower_marker_count(&overlay, rail_y), 0);

        widget.handle_input(
            bounds,
            WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.5), rail_y + 3.0)),
        );
        assert_eq!(widget.hovered_note_ratio, None);
        overlay.clear();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(generic_lower_marker_count(&overlay, rail_y), 1);
    }

    #[test]
    fn persisted_comment_hover_state_survives_retained_widget_rebuilds() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let rail_y = comment_rail_y(bounds);
        let pointer = Point::new(timeline_x(bounds, 0.4), rail_y);
        let mut previous = WaveformWidget::new(Arc::new(test_waveform()), None, vec![(0.4, false)]);
        previous.handle_input(bounds, WidgetInput::pointer_move(pointer));

        let mut current = WaveformWidget::new(Arc::new(test_waveform()), None, vec![(0.4, true)]);
        current.synchronize_from_previous(&previous);
        assert_eq!(current.hovered_note_ratio, Some(0.4));

        let mut overlay = Vec::new();
        current.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert!(overlay.iter().any(|primitive| matches!(
            primitive,
            PaintPrimitive::FillPolygon(fill) if fill.color == colors().note_hover_fill
        )));

        let mut missing = WaveformWidget::new(Arc::new(test_waveform()), None, vec![(0.8, false)]);
        missing.synchronize_from_previous(&previous);
        assert_eq!(missing.hovered_note_ratio, None);
    }

    #[test]
    fn externally_hovered_comment_highlights_its_existing_node() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let widget = WaveformWidget::new(
            Arc::new(test_waveform()),
            None,
            vec![(0.25, false), (0.75, true)],
        )
        .with_external_hovered_note_ratio(Some(0.75));

        let paint_plan = widget.paint_plan_with_defaults(bounds);
        assert_eq!(
            highlighted_note_marker_count(&paint_plan.primitives),
            1,
            "external hover should be part of the base paint"
        );

        let mut overlay = Vec::new();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(highlighted_note_marker_count(&overlay), 0);
    }

    #[test]
    fn externally_selected_comment_highlights_its_existing_node() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let widget = WaveformWidget::new(
            Arc::new(test_waveform()),
            None,
            vec![(0.25, false), (0.75, true)],
        )
        .with_external_selected_note_ratio(Some(0.25));

        let paint_plan = widget.paint_plan_with_defaults(bounds);
        assert_eq!(
            highlighted_note_marker_count(&paint_plan.primitives),
            1,
            "external selection should be part of the base paint"
        );

        let mut overlay = Vec::new();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(highlighted_note_marker_count(&overlay), 0);
    }

    #[test]
    fn collocated_comments_paint_once_and_open_note_wins() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let second_ratio = 0.4 + NOTE_RATIO_MATCH_EPSILON * 0.5;
        let collocated_notes = vec![(0.4, true), (second_ratio, false)];

        let normal_widget =
            WaveformWidget::new(Arc::new(test_waveform()), None, collocated_notes.clone());
        let normal_paint = normal_widget.paint_plan_with_defaults(bounds);
        assert_eq!(
            normal_paint
                .primitives
                .iter()
                .filter(|primitive| matches!(
                    primitive,
                    PaintPrimitive::FillPolygon(fill) if fill.color == colors().note_fill
                ))
                .count(),
            1,
            "collocated notes should paint one normal marker"
        );
        assert_eq!(
            normal_paint
                .primitives
                .iter()
                .filter(|primitive| matches!(
                    primitive,
                    PaintPrimitive::FillPolygon(fill) if fill.color == colors().note_hover_fill
                ))
                .count(),
            0,
            "an open collocated note should keep the marker open"
        );

        let selected_widget =
            WaveformWidget::new(Arc::new(test_waveform()), None, collocated_notes)
                .with_external_selected_note_ratio(Some(second_ratio));
        let selected_paint = selected_widget.paint_plan_with_defaults(bounds);
        assert_eq!(
            selected_paint
                .primitives
                .iter()
                .filter(|primitive| matches!(
                    primitive,
                    PaintPrimitive::FillPolygon(fill) if fill.color == colors().note_hover_fill
                ))
                .count(),
            1,
            "external selection should highlight one collocated marker"
        );
        assert_eq!(
            selected_paint
                .primitives
                .iter()
                .filter(|primitive| matches!(
                    primitive,
                    PaintPrimitive::FillPolygon(fill)
                        if fill.color == colors().note_fill
                            || fill.color == colors().note_hover_fill
                ))
                .count(),
            1,
            "external selection should not duplicate the collocated marker"
        );
        assert_eq!(highlighted_note_marker_count(&selected_paint.primitives), 1);
    }

    #[test]
    fn external_hover_and_selection_share_one_highlighted_marker() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let widget = WaveformWidget::new(
            Arc::new(test_waveform()),
            None,
            vec![(0.25, false), (0.75, true)],
        )
        .with_external_hovered_note_ratio(Some(0.25))
        .with_external_selected_note_ratio(Some(0.25));

        let paint_plan = widget.paint_plan_with_defaults(bounds);
        assert_eq!(highlighted_note_marker_count(&paint_plan.primitives), 1);
    }

    #[test]
    fn pointer_hover_takes_precedence_over_external_comment_hover() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let mut widget = WaveformWidget::new(
            Arc::new(test_waveform()),
            None,
            vec![(0.25, false), (0.75, true)],
        )
        .with_external_hovered_note_ratio(Some(0.25));

        widget.handle_input(
            bounds,
            WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.5), bounds.min.y)),
        );
        let paint_plan = widget.paint_plan_with_defaults(bounds);
        assert_eq!(highlighted_note_marker_count(&paint_plan.primitives), 0);

        let mut overlay = Vec::new();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );

        assert_eq!(highlighted_note_marker_count(&overlay), 0);
    }

    #[test]
    fn draft_comment_marker_is_visible_and_draggable_on_the_comment_rail() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let rail_y = comment_rail_y(bounds);
        let mut upper_widget = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new())
            .with_draft_ratio(Some(0.4));

        assert_eq!(
            interaction(upper_widget.handle_input(
                bounds,
                WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.4), rail_y - 1.0)),
            )),
            WaveformInteraction::PlayheadDragStarted { ratio: 0.4 }
        );

        let mut widget = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new())
            .with_draft_ratio(Some(0.4));
        let paint_plan = widget.paint_plan_with_defaults(bounds);

        assert!(paint_plan.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                radiant::runtime::PaintPrimitive::StrokePolygon(stroke)
                    if stroke.color == colors().note_outline
                        && (stroke.width - 3.0).abs() < f32::EPSILON
            )
        }));
        assert_eq!(
            interaction(widget.handle_input(
                bounds,
                WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.4), rail_y)),
            )),
            WaveformInteraction::CommentDragStarted {
                ratio: 0.4,
                note_index: None,
            }
        );
        assert_eq!(
            interaction(widget.handle_input(
                bounds,
                WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.7), rail_y)),
            )),
            WaveformInteraction::CommentDragMoved { ratio: 0.7 }
        );
        assert_eq!(
            interaction(widget.handle_input(
                bounds,
                WidgetInput::primary_release(Point::new(timeline_x(bounds, 0.8), rail_y)),
            )),
            WaveformInteraction::CommentDragEnded { ratio: 0.8 }
        );
        assert!(!widget.comment_dragging);
    }

    #[test]
    fn persisted_comment_marker_starts_a_targeted_drag() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let rail_y = comment_rail_y(bounds);
        let mut widget = WaveformWidget::new(
            Arc::new(test_waveform()),
            None,
            vec![(0.3, false), (0.8, true)],
        );

        assert_eq!(
            interaction(widget.handle_input(
                bounds,
                WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.3), rail_y)),
            )),
            WaveformInteraction::CommentDragStarted {
                ratio: 0.3,
                note_index: Some(0),
            }
        );
        assert_eq!(
            interaction(widget.handle_input(
                bounds,
                WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.6), rail_y)),
            )),
            WaveformInteraction::CommentDragMoved { ratio: 0.6 }
        );
        assert_eq!(
            interaction(widget.handle_input(
                bounds,
                WidgetInput::primary_release(Point::new(timeline_x(bounds, 0.75), rail_y)),
            )),
            WaveformInteraction::CommentDragEnded { ratio: 0.75 }
        );
        assert!(!widget.comment_dragging);
    }

    #[test]
    fn pointer_capture_cancellation_clears_comment_drag_state_and_emits_cancel() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let rail_y = comment_rail_y(bounds);
        let mut comment_widget =
            WaveformWidget::new(Arc::new(test_waveform()), None, vec![(0.3, false)]);
        assert_eq!(
            interaction(comment_widget.handle_input(
                bounds,
                WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.3), rail_y)),
            )),
            WaveformInteraction::CommentDragStarted {
                ratio: 0.3,
                note_index: Some(0),
            }
        );
        let _ = comment_widget.handle_input(
            bounds,
            WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.6), rail_y)),
        );

        assert_eq!(
            Widget::handle_pointer_capture_cancelled(&mut comment_widget, bounds)
                .and_then(|output| output.typed_copied()),
            Some(WaveformInteraction::CommentDragCancelled)
        );
        assert!(!comment_widget.comment_dragging);
        assert!(!comment_widget.playhead_dragging);
        assert!(!comment_widget.common.state.hovered);
        assert_eq!(comment_widget.hover_ratio, None);
        assert!(!comment_widget.hover_lower);
        assert_eq!(comment_widget.hovered_note_ratio, None);

        let mut playhead_widget = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());
        assert_eq!(
            interaction(playhead_widget.handle_input(
                bounds,
                WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.3), bounds.min.y)),
            )),
            WaveformInteraction::PlayheadDragStarted { ratio: 0.3 }
        );
        assert!(
            Widget::handle_pointer_capture_cancelled(&mut playhead_widget, bounds).is_none(),
            "playhead cancellation must not emit a comment cancellation"
        );
        assert!(!playhead_widget.playhead_dragging);
        assert!(!playhead_widget.common.state.hovered);
    }

    #[test]
    fn reference_waveform_is_compact_and_supports_loop_selection() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 96.0));
        let mut widget = ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None);
        let paint_plan = widget.paint_plan_with_defaults(bounds);
        let rail_y = comment_rail_y(bounds);
        let reference_bars = paint_plan
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill) if fill.color == colors().reference => Some(fill),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(widget.accepts_pointer_move());
        assert!(widget.accepts_pointer_input(&WidgetInput::primary_press(Point::new(20.0, 20.0))));
        assert!(paint_plan.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                radiant::runtime::PaintPrimitive::FillRect(fill)
                    if fill.rect == bounds && fill.color == ThemeTokens::default().surface_base
            )
        }));
        assert!(paint_plan.primitives.iter().any(|primitive| {
            matches!(
            primitive,
            radiant::runtime::PaintPrimitive::FillRect(fill)
                if fill.color == colors().reference
                    && fill.rect.max.y <= bounds.max.y
            )
        }));
        assert_eq!(
            reference_bars.len(),
            display_bar_count(widget.timeline.plot_bounds(bounds).width()),
            "the reference signal should be painted once above the comment rail"
        );
        assert!(reference_bars.iter().all(|bar| bar.rect.max.y <= rail_y));

        assert_eq!(
            widget.handle_input(
                bounds,
                WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.2), 20.0)),
            ),
            None
        );
        assert_eq!(
            reference_interaction(widget.handle_input(
                bounds,
                WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.8), 20.0)),
            )),
            ReferenceWaveformInteraction::Started { ratio: 0.2 }
        );
        assert_eq!(
            reference_interaction(widget.handle_input(
                bounds,
                WidgetInput::primary_release(Point::new(timeline_x(bounds, 0.8), 20.0)),
            )),
            ReferenceWaveformInteraction::Ended {
                start_ratio: 0.2,
                end_ratio: 0.8,
            }
        );

        let selected_paint =
            ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, Some((0.2, 0.8)))
                .paint_plan_with_defaults(bounds);
        assert!(selected_paint.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                radiant::runtime::PaintPrimitive::FillRect(fill)
                    if fill.color == colors().reference_selection_fill
            )
        }));
    }

    #[test]
    fn reference_waveform_click_emits_clicked_without_starting_a_loop_drag() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 96.0));
        let mut widget = ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None);

        assert!(
            widget
                .handle_input(
                    bounds,
                    WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.2), 20.0)),
                )
                .is_none()
        );
        assert_eq!(
            reference_interaction(widget.handle_input(
                bounds,
                WidgetInput::primary_release(Point::new(timeline_x(bounds, 0.2), 20.0)),
            )),
            ReferenceWaveformInteraction::Clicked { ratio: 0.2 }
        );
        assert!(widget.loop_drag_start_ratio.is_none());
        assert!(widget.pointer_down_position.is_none());
    }

    #[test]
    fn reference_waveform_lower_rail_emits_a_comment_click_without_starting_a_loop() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 96.0));
        let rail_y = comment_rail_y(bounds);
        let mut widget = ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None)
            .with_note_ratios(vec![(0.2, false)]);

        assert!(
            widget
                .handle_input(
                    bounds,
                    WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.2), rail_y + 3.0)),
                )
                .is_none()
        );
        let mut overlay = Vec::new();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(highlighted_note_marker_count(&overlay), 1);
        assert_eq!(generic_lower_marker_count(&overlay, rail_y), 0);

        widget.handle_input(
            bounds,
            WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.5), rail_y + 3.0)),
        );
        overlay.clear();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(highlighted_note_marker_count(&overlay), 0);
        assert_eq!(generic_lower_marker_count(&overlay, rail_y), 1);

        assert!(
            widget
                .handle_input(
                    bounds,
                    WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.2), rail_y + 4.0))
                )
                .is_none()
        );
        assert_eq!(
            reference_interaction(widget.handle_input(
                bounds,
                WidgetInput::primary_release(Point::new(timeline_x(bounds, 0.2), rail_y + 4.0))
            )),
            ReferenceWaveformInteraction::CommentClicked { ratio: 0.2 }
        );
        assert!(widget.loop_drag_start_ratio.is_none());
        assert!(!widget.comment_pointer_down);
    }

    #[test]
    fn reference_waveform_paints_persisted_and_draft_comment_markers() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 96.0));
        let widget = ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None)
            .with_note_ratios(vec![(0.2, false), (0.8, true)])
            .with_draft_ratio(Some(0.5))
            .with_selected_note_ratio(Some(0.8));
        let paint_plan = widget.paint_plan_with_defaults(bounds);

        assert!(paint_plan.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                PaintPrimitive::FillPolygon(fill) if fill.color == colors().note_fill
            )
        }));
        assert!(paint_plan.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                PaintPrimitive::StrokePolygon(stroke)
                    if stroke.color == colors().note_hover_outline
                        && (stroke.width - NOTE_HOVER_OUTLINE_WIDTH).abs() < f32::EPSILON
            )
        }));
    }

    #[test]
    fn reference_waveform_external_comment_hover_highlights_one_existing_marker() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 96.0));
        let widget = ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None)
            .with_note_ratios(vec![(0.2, false), (0.8, true)])
            .with_external_hovered_note_ratio(Some(0.8));
        let paint_plan = widget.paint_plan_with_defaults(bounds);

        assert_eq!(highlighted_note_marker_count(&paint_plan.primitives), 1);
    }

    #[test]
    fn reference_waveform_pointer_state_survives_widget_synchronization() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 96.0));
        let mut previous = ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None);
        assert!(
            previous
                .handle_input(
                    bounds,
                    WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.2), 20.0)),
                )
                .is_none()
        );

        let mut current = ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None);
        current.synchronize_from_previous(&previous);

        assert_eq!(
            reference_interaction(current.handle_input(
                bounds,
                WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.8), 20.0)),
            )),
            ReferenceWaveformInteraction::Started { ratio: 0.2 }
        );
        assert_eq!(
            reference_interaction(current.handle_input(
                bounds,
                WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.9), 20.0)),
            )),
            ReferenceWaveformInteraction::Moved { ratio: 0.9 }
        );
        assert_eq!(
            reference_interaction(current.handle_input(
                bounds,
                WidgetInput::primary_release(Point::new(timeline_x(bounds, 0.9), 20.0))
            )),
            ReferenceWaveformInteraction::Ended {
                start_ratio: 0.2,
                end_ratio: 0.9,
            }
        );
    }

    #[test]
    fn reference_waveform_paints_the_shared_cursor() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 96.0));
        let widget = ReferenceWaveformWidget::new(Arc::new(test_waveform()), Some(0.5), None);
        let expected_x = timeline_x(bounds, 0.5);
        let paint_plan = widget.paint_plan_with_defaults(bounds);

        assert!(paint_plan.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                radiant::runtime::PaintPrimitive::FillRect(fill)
                    if fill.color == colors().cursor
                        && (fill.rect.min.x - (expected_x - 1.0)).abs() < f32::EPSILON
                        && (fill.rect.max.x - (expected_x + 1.0)).abs() < f32::EPSILON
                        && fill.rect.min.y == bounds.min.y
                        && fill.rect.max.y == bounds.max.y
            )
        }));
    }

    #[test]
    fn reference_comment_markers_share_plot_coordinates_with_cursor_and_hit_testing() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 96.0));
        let ratio = 0.5;
        let mut widget = ReferenceWaveformWidget::new(Arc::new(test_waveform()), Some(ratio), None)
            .with_note_ratios(vec![(ratio, false)]);

        let expected_x = timeline_x(bounds, ratio);
        let rail_y = comment_rail_y(bounds);
        let paint_plan = widget.paint_plan_with_defaults(bounds);
        let cursor_center_x = paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if fill.color == colors().cursor
                        && (fill.rect.width() - CURSOR_WIDTH).abs() < f32::EPSILON
                        && fill.rect.min.y == bounds.min.y =>
                {
                    Some((fill.rect.min.x + fill.rect.max.x) * 0.5)
                }
                _ => None,
            })
            .expect("reference cursor should be painted");
        let marker_center_x = paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillPolygon(fill) if fill.color == colors().note_fill => {
                    let (min_x, max_x) = fill
                        .points
                        .iter()
                        .map(|point| point.x)
                        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min_x, max_x), x| {
                            (min_x.min(x), max_x.max(x))
                        });
                    Some((min_x + max_x) * 0.5)
                }
                _ => None,
            })
            .expect("persisted reference marker should be painted");

        assert!((cursor_center_x - expected_x).abs() < 1e-6);
        assert!((marker_center_x - expected_x).abs() < 1e-6);
        assert!((cursor_center_x - marker_center_x).abs() < 1e-6);

        let marker_position = Point::new(expected_x, rail_y + 3.0);
        assert!(
            widget
                .handle_input(bounds, WidgetInput::pointer_move(marker_position))
                .is_none()
        );
        assert_eq!(widget.hovered_note_ratio, Some(ratio));

        let mut overlay = Vec::new();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(highlighted_note_marker_count(&overlay), 1);
        let highlighted_center_x = overlay
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillPolygon(fill) if fill.color == colors().note_hover_fill => {
                    let (min_x, max_x) = fill
                        .points
                        .iter()
                        .map(|point| point.x)
                        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min_x, max_x), x| {
                            (min_x.min(x), max_x.max(x))
                        });
                    Some((min_x + max_x) * 0.5)
                }
                _ => None,
            })
            .expect("hovered reference marker should be highlighted");
        assert!((highlighted_center_x - expected_x).abs() < 1e-6);

        assert!(
            widget
                .handle_input(bounds, WidgetInput::primary_press(marker_position))
                .is_none()
        );
        match reference_interaction(
            widget.handle_input(bounds, WidgetInput::primary_release(marker_position)),
        ) {
            ReferenceWaveformInteraction::CommentClicked {
                ratio: clicked_ratio,
            } => {
                assert!((clicked_ratio - ratio).abs() < 1e-6);
            }
            interaction => panic!("expected comment click, got {interaction:?}"),
        }

        let temporary_ratio = 0.75;
        let temporary_position = Point::new(timeline_x(bounds, temporary_ratio), rail_y + 3.0);
        widget.handle_input(bounds, WidgetInput::pointer_move(temporary_position));
        overlay.clear();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(generic_lower_marker_count(&overlay, rail_y), 1);
        let temporary_marker_center_x = overlay
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if fill.color == colors().note_outline
                        && (fill.rect.width() - MARKER_RADIUS * 2.0).abs() < f32::EPSILON
                        && (fill.rect.height() - MARKER_RADIUS * 2.0).abs() < f32::EPSILON =>
                {
                    Some((fill.rect.min.x + fill.rect.max.x) * 0.5)
                }
                _ => None,
            })
            .expect("temporary lower-rail marker should be painted");
        assert!((temporary_marker_center_x - timeline_x(bounds, temporary_ratio)).abs() < 1e-6);
    }

    #[test]
    fn reference_waveform_paints_a_cursor_at_mouse_hover_ratio() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 96.0));
        let mut widget = ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None);
        assert!(
            widget
                .handle_input(bounds, WidgetInput::pointer_move(Point::new(70.0, 50.0)))
                .is_none()
        );

        let mut overlay = Vec::new();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert!(overlay.iter().any(|primitive| {
            matches!(
                primitive,
                PaintPrimitive::FillRect(fill)
                    if fill.color == colors().cursor
                        && (fill.rect.min.x - 69.0).abs() < f32::EPSILON
                        && (fill.rect.max.x - 71.0).abs() < f32::EPSILON
                        && fill.rect.min.y == bounds.min.y
                        && fill.rect.max.y == bounds.max.y
            )
        }));

        widget.handle_input(
            bounds,
            WidgetInput::pointer_move(Point::new(bounds.max.x + 10.0, 50.0)),
        );
        overlay.clear();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert!(overlay.is_empty());
    }

    #[test]
    fn reference_waveform_start_edge_clamps_hover_and_click_to_zero() {
        let bounds = Rect::from_min_max(Point::new(0.0, 20.0), Point::new(100.0, 96.0));
        let mut widget = ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None);

        assert!(
            widget
                .handle_input(bounds, WidgetInput::pointer_move(Point::new(5.0, 50.0)))
                .is_none()
        );
        let mut overlay = Vec::new();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert!(overlay.iter().any(|primitive| {
            matches!(
                primitive,
                PaintPrimitive::FillRect(fill)
                    if fill.color == colors().cursor
                        && (fill.rect.min.x - 9.0).abs() < f32::EPSILON
                        && (fill.rect.max.x - 11.0).abs() < f32::EPSILON
                        && fill.rect.min.y == bounds.min.y
                        && fill.rect.max.y == bounds.max.y
            )
        }));

        assert!(
            widget
                .handle_input(bounds, WidgetInput::primary_press(Point::new(5.0, 50.0)))
                .is_none()
        );
        assert_eq!(
            reference_interaction(
                widget.handle_input(bounds, WidgetInput::primary_release(Point::new(5.0, 50.0)))
            ),
            ReferenceWaveformInteraction::Clicked { ratio: 0.0 }
        );

        widget.handle_input(
            bounds,
            WidgetInput::pointer_move(Point::new(bounds.min.x - 1.0, 50.0)),
        );
        overlay.clear();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert!(overlay.is_empty());
    }

    #[test]
    fn main_waveform_start_edge_hover_and_playhead_drag_use_ratio_zero() {
        let bounds = Rect::from_min_max(Point::new(0.0, 20.0), Point::new(100.0, 120.0));
        let gutter = Point::new(5.0, 50.0);
        let mut widget = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());

        assert!(
            widget
                .handle_input(bounds, WidgetInput::pointer_move(gutter))
                .is_none()
        );
        assert_eq!(widget.hover_ratio, Some(0.0));
        assert!(widget.common.state.hovered);

        let mut overlay = Vec::new();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        let start_x = timeline_x(bounds, 0.0);
        assert!(overlay.iter().any(|primitive| {
            matches!(
                primitive,
                PaintPrimitive::FillRect(fill)
                    if fill.color == colors().cursor
                        && (fill.rect.min.x - (start_x - 1.0)).abs() < f32::EPSILON
                        && (fill.rect.max.x - (start_x + 1.0)).abs() < f32::EPSILON
                        && fill.rect.min.y == bounds.min.y
            )
        }));

        assert_eq!(
            interaction(widget.handle_input(bounds, WidgetInput::primary_press(gutter))),
            WaveformInteraction::PlayheadDragStarted { ratio: 0.0 }
        );
        assert_eq!(
            interaction(widget.handle_input(bounds, WidgetInput::primary_release(gutter))),
            WaveformInteraction::PlayheadDragEnded { ratio: 0.0 }
        );
    }

    #[test]
    fn upper_start_gutter_keeps_playhead_semantics_for_ratio_zero_persisted_marker() {
        let bounds = Rect::from_min_max(Point::new(0.0, 20.0), Point::new(100.0, 120.0));
        let rail_y = comment_rail_y(bounds);
        let upper_gutter = Point::new(bounds.min.x + REFERENCE_START_HIT_SLOP * 0.5, rail_y - 1.0);
        let mut widget = WaveformWidget::new(Arc::new(test_waveform()), None, vec![(0.0, false)]);

        assert_eq!(
            interaction(widget.handle_input(bounds, WidgetInput::primary_press(upper_gutter),)),
            WaveformInteraction::PlayheadDragStarted { ratio: 0.0 }
        );
        assert_eq!(
            interaction(widget.handle_input(bounds, WidgetInput::primary_release(upper_gutter),)),
            WaveformInteraction::PlayheadDragEnded { ratio: 0.0 }
        );
        assert!(!widget.playhead_dragging);
        assert!(!widget.comment_dragging);
    }

    #[test]
    fn main_comment_markers_share_plot_coordinates_with_cursor_and_hit_testing() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let ratio = 0.5;
        let expected_x = timeline_x(bounds, ratio);
        let rail_y = comment_rail_y(bounds);
        let mut widget =
            WaveformWidget::new(Arc::new(test_waveform()), Some(ratio), vec![(ratio, false)]);
        let paint_plan = widget.paint_plan_with_defaults(bounds);
        let cursor_center_x = paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if fill.color == colors().cursor
                        && (fill.rect.width() - CURSOR_WIDTH).abs() < f32::EPSILON
                        && fill.rect.min.y == bounds.min.y =>
                {
                    Some((fill.rect.min.x + fill.rect.max.x) * 0.5)
                }
                _ => None,
            })
            .expect("main cursor should be painted");
        let marker_center_x = paint_plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillPolygon(fill) if fill.color == colors().note_fill => {
                    let (min_x, max_x) = fill
                        .points
                        .iter()
                        .map(|point| point.x)
                        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min_x, max_x), x| {
                            (min_x.min(x), max_x.max(x))
                        });
                    Some((min_x + max_x) * 0.5)
                }
                _ => None,
            })
            .expect("persisted main marker should be painted");
        assert!((cursor_center_x - expected_x).abs() < 1e-6);
        assert!((marker_center_x - expected_x).abs() < 1e-6);

        let marker_position = Point::new(expected_x, rail_y + 3.0);
        assert!(
            widget
                .handle_input(bounds, WidgetInput::pointer_move(marker_position))
                .is_none()
        );
        assert_eq!(widget.hovered_note_ratio, Some(ratio));

        let mut overlay = Vec::new();
        widget.append_runtime_overlay_paint(
            &mut overlay,
            bounds,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(highlighted_note_marker_count(&overlay), 1);
        let highlighted_center_x = overlay
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillPolygon(fill) if fill.color == colors().note_hover_fill => {
                    let (min_x, max_x) = fill
                        .points
                        .iter()
                        .map(|point| point.x)
                        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min_x, max_x), x| {
                            (min_x.min(x), max_x.max(x))
                        });
                    Some((min_x + max_x) * 0.5)
                }
                _ => None,
            })
            .expect("hovered main marker should be highlighted");
        assert!((highlighted_center_x - expected_x).abs() < 1e-6);

        assert_eq!(
            interaction(widget.handle_input(bounds, WidgetInput::primary_press(marker_position),)),
            WaveformInteraction::CommentDragStarted {
                ratio,
                note_index: Some(0),
            }
        );
        assert_eq!(
            interaction(
                widget.handle_input(bounds, WidgetInput::primary_release(marker_position),)
            ),
            WaveformInteraction::CommentDragEnded { ratio }
        );

        let mut draft_widget = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new())
            .with_draft_ratio(Some(ratio));
        assert_eq!(
            interaction(
                draft_widget.handle_input(bounds, WidgetInput::primary_press(marker_position),)
            ),
            WaveformInteraction::CommentDragStarted {
                ratio,
                note_index: None,
            }
        );
    }

    #[test]
    fn main_and_reference_share_the_same_start_gutter_boundary() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let gutter = Point::new(15.0, 40.0);
        let plot_start = timeline_x(bounds, 0.0);
        let mut main = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());
        let mut reference = ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None);

        assert_eq!(plot_start, 20.0);
        assert!(main.timeline.interactive_contains(bounds, gutter));
        assert!(reference.timeline.interactive_contains(bounds, gutter));
        assert_eq!(main.timeline.ratio_at(bounds, gutter), 0.0);
        assert_eq!(reference.timeline.ratio_at(bounds, gutter), 0.0);
        assert!(
            !main
                .timeline
                .interactive_contains(bounds, Point::new(bounds.min.x - 1.0, gutter.y))
        );

        assert!(
            main.handle_input(bounds, WidgetInput::pointer_move(gutter))
                .is_none()
        );
        assert!(
            reference
                .handle_input(bounds, WidgetInput::pointer_move(gutter))
                .is_none()
        );
        assert_eq!(main.hover_ratio, Some(0.0));
        assert_eq!(reference.hover_ratio, Some(0.0));
    }

    #[test]
    fn narrow_timeline_bounds_do_not_emit_invalid_fill_geometry() {
        let bounds = Rect::from_min_max(Point::new(0.0, 0.0), Point::new(4.0, 20.0));
        let timeline = TimelineSurface::new();
        assert_eq!(timeline.plot_bounds(bounds).width(), 0.0);
        assert_eq!(timeline.ratio_at(bounds, Point::new(2.0, 10.0)), 0.0);
        assert_eq!(timeline.x_at(bounds, 0.0), 4.0);

        let main_plan = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new())
            .paint_plan_with_defaults(bounds);
        let reference_plan = ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None)
            .paint_plan_with_defaults(bounds);
        for plan in [main_plan, reference_plan] {
            assert!(plan.primitives.iter().all(|primitive| match primitive {
                PaintPrimitive::FillRect(fill) => fill.rect.has_finite_positive_area(),
                _ => true,
            }));
        }
    }

    #[test]
    fn upper_waveform_emits_a_captured_playhead_drag_with_clamped_ratios() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let mut widget = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());

        assert_eq!(
            interaction(widget.handle_input(
                bounds,
                WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.25), 40.0)),
            )),
            WaveformInteraction::PlayheadDragStarted { ratio: 0.25 }
        );
        assert_eq!(
            interaction(
                widget.handle_input(bounds, WidgetInput::pointer_move(Point::new(-40.0, 40.0)),)
            ),
            WaveformInteraction::PlayheadDragMoved { ratio: 0.0 }
        );
        assert_eq!(
            interaction(
                widget.handle_input(bounds, WidgetInput::pointer_move(Point::new(180.0, 40.0)),)
            ),
            WaveformInteraction::PlayheadDragMoved { ratio: 1.0 }
        );
        assert_eq!(
            interaction(widget.handle_input(
                bounds,
                WidgetInput::primary_release(Point::new(timeline_x(bounds, 0.5), 40.0)),
            )),
            WaveformInteraction::PlayheadDragEnded { ratio: 0.5 }
        );
        assert!(!widget.playhead_dragging);
    }

    #[test]
    fn playhead_drag_state_survives_widget_synchronization() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let mut previous = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());
        interaction(previous.handle_input(
            bounds,
            WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.25), 40.0)),
        ));

        let mut current = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());
        current.synchronize_from_previous(&previous);

        assert_eq!(
            interaction(current.handle_input(
                bounds,
                WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.75), 40.0)),
            )),
            WaveformInteraction::PlayheadDragMoved { ratio: 0.75 }
        );
    }

    #[test]
    fn preview_waveforms_reject_pointer_input_until_finalized() {
        let main_bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let mut previous_main = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());
        interaction(previous_main.handle_input(
            main_bounds,
            WidgetInput::primary_press(Point::new(timeline_x(main_bounds, 0.25), 40.0)),
        ));

        let mut preview_main = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new())
            .with_visible_ratio(Some(0.4));
        preview_main.synchronize_from_previous(&previous_main);
        assert!(!preview_main.accepts_pointer_move());
        assert!(
            !preview_main.accepts_pointer_input(&WidgetInput::primary_press(Point::new(
                timeline_x(main_bounds, 0.25),
                40.0,
            )))
        );
        assert!(!preview_main.playhead_dragging);
        assert!(
            preview_main
                .handle_input(
                    main_bounds,
                    WidgetInput::primary_release(Point::new(timeline_x(main_bounds, 0.75), 40.0))
                )
                .is_none()
        );

        let mut finalized_main = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());
        finalized_main.synchronize_from_previous(&preview_main);
        assert!(finalized_main.accepts_pointer_move());
        assert_eq!(
            interaction(finalized_main.handle_input(
                main_bounds,
                WidgetInput::primary_press(Point::new(timeline_x(main_bounds, 0.25), 40.0)),
            )),
            WaveformInteraction::PlayheadDragStarted { ratio: 0.25 }
        );

        let reference_bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 96.0));
        let mut previous_reference =
            ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None);
        assert!(
            previous_reference
                .handle_input(
                    reference_bounds,
                    WidgetInput::primary_press(Point::new(30.0, 20.0)),
                )
                .is_none()
        );

        let mut preview_reference =
            ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None)
                .with_visible_ratio(Some(0.4));
        preview_reference.synchronize_from_previous(&previous_reference);
        assert!(!preview_reference.accepts_pointer_move());
        assert!(
            !preview_reference
                .accepts_pointer_input(&WidgetInput::primary_press(Point::new(30.0, 20.0),))
        );
        assert!(
            preview_reference
                .handle_input(
                    reference_bounds,
                    WidgetInput::pointer_move(Point::new(90.0, 20.0)),
                )
                .is_none()
        );

        let complete_reference =
            ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None)
                .with_visible_ratio(Some(1.0));
        assert!(
            complete_reference.accepts_pointer_move(),
            "a fully painted reference preview must be hoverable before completion dispatch"
        );
        assert!(
            complete_reference
                .accepts_pointer_input(&WidgetInput::primary_press(Point::new(30.0, 20.0),))
        );

        let mut finalized_reference =
            ReferenceWaveformWidget::new(Arc::new(test_waveform()), None, None);
        finalized_reference.synchronize_from_previous(&preview_reference);
        assert!(finalized_reference.accepts_pointer_move());
        assert!(
            finalized_reference
                .handle_input(
                    reference_bounds,
                    WidgetInput::primary_press(Point::new(30.0, 20.0)),
                )
                .is_none()
        );
        assert!(
            finalized_reference
                .handle_input(
                    reference_bounds,
                    WidgetInput::pointer_move(Point::new(90.0, 20.0)),
                )
                .is_some()
        );
    }

    #[test]
    fn lower_waveform_press_remains_a_comment_click() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let mut widget = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());

        assert_eq!(
            interaction(widget.handle_input(
                bounds,
                WidgetInput::primary_press(Point::new(timeline_x(bounds, 0.5), 105.0)),
            )),
            WaveformInteraction::Clicked {
                ratio: 0.5,
                lower: true,
            }
        );
        assert!(!widget.playhead_dragging);
        assert!(widget.comment_dragging);
        assert_eq!(
            interaction(widget.handle_input(
                bounds,
                WidgetInput::pointer_move(Point::new(timeline_x(bounds, 0.75), 105.0)),
            )),
            WaveformInteraction::CommentDragMoved { ratio: 0.75 }
        );
        assert_eq!(
            interaction(widget.handle_input(
                bounds,
                WidgetInput::primary_release(Point::new(timeline_x(bounds, 0.8), 105.0))
            )),
            WaveformInteraction::CommentDragEnded { ratio: 0.8 }
        );
        assert!(!widget.comment_dragging);
    }

    #[test]
    fn ratios_are_clamped_and_non_finite_values_are_safe() {
        assert_eq!(clamp_ratio(-0.5), 0.0);
        assert_eq!(clamp_ratio(1.5), 1.0);
        assert_eq!(clamp_ratio(f32::NAN), 0.0);
    }

    #[test]
    fn timestamps_round_trip_through_normalized_positions() {
        assert_eq!(ratio_for_millis(500, 1_000), Some(0.5));
        assert_eq!(ratio_for_millis(2_000, 1_000), Some(1.0));
        assert_eq!(ratio_for_millis(500, 0), None);
        assert_eq!(millis_for_ratio(0.501, 1_000), 501);
    }

    #[test]
    fn bar_count_is_derived_from_screen_width() {
        assert_eq!(display_bar_count(640.0), 160);
        assert_eq!(display_bar_count(640.0), display_bar_count(640.0));
        assert!(display_bar_count(1_200.0) > display_bar_count(640.0));
    }

    #[test]
    fn short_and_long_tracks_use_the_same_requested_bar_count() {
        let short = GpuSignalSummary::from_interleaved_samples(&[0.1, 0.8, 0.2, 0.4], 4, 1);
        let long = GpuSignalSummary::from_interleaved_samples(
            &(0..4_096)
                .map(|index| ((index % 17) as f32 / 17.0) * 2.0 - 1.0)
                .collect::<Vec<_>>(),
            4_096,
            1,
        );

        let short_levels = display_bar_levels(&short, 128);
        let long_levels = display_bar_levels(&long, 128);

        assert_eq!(short_levels.len(), 128);
        assert_eq!(long_levels.len(), 128);
        assert!(short_levels.iter().all(|level| level.is_finite()));
        assert!(long_levels.iter().all(|level| level.is_finite()));
    }

    #[test]
    fn short_waveforms_interpolate_between_source_buckets() {
        let summary = GpuSignalSummary::from_interleaved_samples(&[0.1, 0.8, 0.2], 3, 1);
        let levels = display_bar_levels(&summary, 9);

        assert_eq!(levels.len(), 9);
        assert!(levels[1] < levels[2]);
        assert!(levels[2] < levels[3]);
    }

    #[test]
    fn downsampling_combines_rms_windows_by_energy_without_stretching_them() {
        let summary = GpuSignalSummary::from_interleaved_samples(
            &[
                0.0, 0.01, 0.0, 0.01, 0.0, 0.04, 0.0, 0.04, 0.0, 0.09, 0.0, 0.09, 0.0, 0.81, 0.0,
                0.81, 0.0, 0.81, 0.0, 0.81, 0.0, 0.01, 0.0, 0.01,
            ],
            12,
            2,
        );
        let levels = display_bar_levels(&summary, 5);

        assert_eq!(levels.len(), 5);
        assert!(levels[3] > 0.85);
        assert!(levels[4] < levels[3]);
        assert!((levels[4] - (0.83_f32 / 3.0).sqrt()).abs() < 1e-6);
    }

    #[test]
    fn downsampling_alternating_loud_and_silent_rms_windows_preserves_energy() {
        let summary = GpuSignalSummary::from_interleaved_samples(
            &[0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            4,
            2,
        );
        let levels = display_bar_levels(&summary, 2);

        assert_eq!(levels.len(), 2);
        assert!(
            levels
                .iter()
                .all(|level| { (*level - 2.0_f32.sqrt().recip()).abs() < 1e-6 })
        );
    }

    #[test]
    fn downsampling_preserves_two_local_dips_between_loud_sections() {
        let summary = GpuSignalSummary::from_interleaved_samples(
            &[
                0.99, 0.81, 0.99, 0.81, 0.99, 0.01, 0.99, 0.01, 0.99, 0.81, 0.99, 0.81, 0.99, 0.01,
                0.99, 0.01,
            ],
            8,
            2,
        );
        let levels = display_bar_levels(&summary, 4);

        assert_eq!(levels.len(), 4);
        assert!(levels[0] > 0.85);
        assert!(levels[1] < 0.2);
        assert!(levels[2] > 0.85);
        assert!(levels[3] < 0.2);
    }

    #[test]
    fn display_bar_levels_preserve_absolute_dynamics() {
        let summary = GpuSignalSummary::from_interleaved_samples(
            &[0.4, 0.45, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
            8,
            1,
        );
        let levels = display_bar_levels(&summary, 8);

        assert!(levels[1] < levels[4]);
        assert!(levels[4] < levels[7]);
        assert!((levels[0] - 0.4).abs() < f32::EPSILON);
        assert!((levels[7] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn display_bar_levels_keep_flat_data_finite_and_stable() {
        let summary = GpuSignalSummary::from_interleaved_samples(&[0.5; 8], 8, 1);
        let levels = display_bar_levels(&summary, 8);

        assert!(levels.iter().all(|level| level.is_finite()));
        assert!(
            levels
                .iter()
                .all(|level| (*level - 0.5).abs() < f32::EPSILON)
        );

        let silence = GpuSignalSummary::from_interleaved_samples(&[0.0; 8], 8, 1);
        let silence_levels = display_bar_levels(&silence, 8);
        assert!(silence_levels.iter().all(|level| *level == 0.0));
    }
}

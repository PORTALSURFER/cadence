//! Retained native review waveform and its timestamp interaction surface.
//!
//! The audio decoder owns the immutable signal summary. This module owns only
//! the lightweight Radiant widget that paints that summary and reports a
//! normalized review position back to the Cadence reducer.

use crate::{audio::WaveformData, chrome};
use radiant::{
    gui::types::{Point, Rect, Rgba8},
    layout::LayoutOutput,
    prelude as ui,
    runtime::{
        PaintFillRect, PaintPrimitive, PaintTextAlign, PaintTextMetrics, push_text_run_with_metrics,
    },
    theme::ThemeTokens,
    widgets::{
        FocusBehavior, PaintBounds, PointerButton, PointerCapturePolicy, Widget, WidgetCommon,
        WidgetInput, WidgetOutput,
    },
};
use std::sync::Arc;

const COMMENT_RAIL_RATIO: f32 = 0.78;
const MARKER_RADIUS: f32 = 4.5;
const CURSOR_WIDTH: f32 = 2.0;
const CURSOR_GAP_ABOVE_RAIL: f32 = 7.0;
const MAX_DISPLAY_BAR_COUNT: usize = 512;
const BAR_PITCH: f32 = 4.0;
const BAR_GAP: f32 = 1.0;

const BAR_COLOR: Rgba8 = chrome::ACCENT_ORANGE_SOFT;
const BAR_HOVER_COLOR: Rgba8 = chrome::TEXT_DIM;
const BAR_PLAYED_COLOR: Rgba8 = chrome::TEXT_PRIMARY;
const BAR_PLAYED_HOVER_COLOR: Rgba8 = chrome::ACCENT_ORANGE;
const RAIL_COLOR: Rgba8 = chrome::RULE_SOFT;
const NOTE_COLOR: Rgba8 = chrome::ACCENT_ORANGE;
const DONE_NOTE_COLOR: Rgba8 = chrome::TEXT_DIM;
const CURSOR_COLOR: Rgba8 = chrome::ACCENT_ORANGE;
const COMMENT_LABEL_COLOR: Rgba8 = chrome::TEXT_MUTED;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaveformInteraction {
    PlayheadDragStarted { ratio: f32 },
    PlayheadDragMoved { ratio: f32 },
    PlayheadDragEnded { ratio: f32 },
    Clicked { ratio: f32, lower: bool },
}

pub fn view<Message: 'static>(
    waveform: Arc<WaveformData>,
    cursor_ratio: Option<f32>,
    note_ratios: Vec<(f32, bool)>,
    map: impl Fn(WaveformInteraction) -> Message + 'static,
) -> ui::View<Message> {
    ui::custom_widget_mapped(
        WaveformWidget::new(waveform, cursor_ratio, note_ratios),
        map,
    )
}

#[derive(Clone, Debug)]
struct WaveformWidget {
    common: WidgetCommon,
    summary: Arc<radiant::runtime::GpuSignalSummary>,
    cursor_ratio: Option<f32>,
    note_ratios: Vec<(f32, bool)>,
    hover_ratio: Option<f32>,
    hover_lower: bool,
    playhead_dragging: bool,
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
            summary: Arc::clone(&waveform.summary),
            cursor_ratio: cursor_ratio.map(clamp_ratio),
            note_ratios,
            hover_ratio: None,
            hover_lower: false,
            playhead_dragging: false,
        }
    }

    fn ratio_from_position(bounds: Rect, position: Point) -> f32 {
        if bounds.width() <= 0.0 {
            return 0.0;
        }
        clamp_ratio((position.x - bounds.min.x) / bounds.width())
    }

    fn lower_from_position(bounds: Rect, position: Point) -> bool {
        if bounds.height() <= 0.0 {
            return false;
        }
        position.y >= comment_rail_y(bounds)
    }
}

impl Widget for WaveformWidget {
    fn common(&self) -> &WidgetCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut WidgetCommon {
        &mut self.common
    }

    fn handle_input(&mut self, bounds: Rect, input: WidgetInput) -> Option<WidgetOutput> {
        match input {
            WidgetInput::PointerMove { position, .. } => {
                let inside = bounds.contains(position);
                let ratio = Self::ratio_from_position(bounds, position);
                self.common.state.hovered = inside;
                if self.playhead_dragging {
                    self.hover_ratio = Some(ratio);
                    self.hover_lower = false;
                    Some(WidgetOutput::typed(
                        WaveformInteraction::PlayheadDragMoved { ratio },
                    ))
                } else {
                    self.hover_ratio = inside.then_some(ratio);
                    self.hover_lower = inside && Self::lower_from_position(bounds, position);
                    None
                }
            }
            WidgetInput::PointerPress {
                position,
                button: PointerButton::Primary,
                ..
            } if bounds.contains(position) => {
                let ratio = Self::ratio_from_position(bounds, position);
                if Self::lower_from_position(bounds, position) {
                    Some(WidgetOutput::typed(WaveformInteraction::Clicked {
                        ratio,
                        lower: true,
                    }))
                } else {
                    self.playhead_dragging = true;
                    self.hover_ratio = Some(ratio);
                    self.hover_lower = false;
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
            } if self.playhead_dragging => {
                let ratio = Self::ratio_from_position(bounds, position);
                self.playhead_dragging = false;
                self.common.state.hovered = bounds.contains(position);
                self.hover_ratio = bounds.contains(position).then_some(ratio);
                self.hover_lower = false;
                Some(WidgetOutput::typed(
                    WaveformInteraction::PlayheadDragEnded { ratio },
                ))
            }
            _ => None,
        }
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
        self.playhead_dragging = previous.playhead_dragging;
    }

    fn prefers_pointer_move_paint_only(&self) -> bool {
        true
    }

    fn append_paint(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        bounds: Rect,
        _layout: &LayoutOutput,
        _theme: &ThemeTokens,
    ) {
        if !bounds.has_finite_positive_area() {
            return;
        }

        let rail_y = comment_rail_y(bounds);
        let waveform_bounds = Rect::from_min_max(bounds.min, Point::new(bounds.max.x, rail_y));
        let bar_levels = display_bar_levels(&self.summary, display_bar_count(bounds.width()));
        paint_bars(
            primitives,
            self.common.id,
            waveform_bounds,
            &bar_levels,
            self.cursor_ratio,
            self.common.state.hovered,
        );

        fill_rect(
            primitives,
            self.common.id,
            Rect::from_min_max(
                Point::new(bounds.min.x, rail_y - 1.0),
                Point::new(bounds.max.x, rail_y + 1.0),
            ),
            RAIL_COLOR,
        );

        push_text_run_with_metrics(
            primitives,
            self.common.id,
            "COMMENTS / CLICK TO PIN",
            Rect::from_min_max(
                Point::new(bounds.min.x, rail_y + 8.0),
                Point::new(bounds.max.x, rail_y + 24.0),
            ),
            COMMENT_LABEL_COLOR,
            PaintTextAlign::Left,
            PaintTextMetrics::new(8.0, Some(10.0)),
        );

        for (ratio, done) in &self.note_ratios {
            let x = bounds.x_for_ratio(*ratio);
            fill_rect(
                primitives,
                self.common.id,
                marker_rect(x, rail_y, MARKER_RADIUS),
                if *done { DONE_NOTE_COLOR } else { NOTE_COLOR },
            );
        }

        if let Some(ratio) = self.cursor_ratio {
            paint_cursor(primitives, self.common.id, bounds, ratio, rail_y);
        }
    }

    fn append_runtime_overlay_paint(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        bounds: Rect,
        _layout: &LayoutOutput,
        _theme: &ThemeTokens,
    ) {
        let Some(ratio) = self.hover_ratio else {
            return;
        };
        let rail_y = comment_rail_y(bounds);
        let x = bounds.x_for_ratio(ratio);
        let line_bottom = rail_y - CURSOR_GAP_ABOVE_RAIL;
        fill_rect(
            primitives,
            self.common.id,
            Rect::from_min_max(
                Point::new(x - CURSOR_WIDTH * 0.5, bounds.min.y),
                Point::new(x + CURSOR_WIDTH * 0.5, line_bottom),
            ),
            CURSOR_COLOR,
        );
        if self.hover_lower {
            fill_rect(
                primitives,
                self.common.id,
                marker_rect(x, rail_y, MARKER_RADIUS),
                NOTE_COLOR,
            );
        }
    }
}

fn fill_rect(primitives: &mut Vec<PaintPrimitive>, widget_id: u64, rect: Rect, color: Rgba8) {
    primitives.push(PaintPrimitive::FillRect(PaintFillRect {
        widget_id,
        rect,
        color,
    }));
}

fn display_bar_levels(
    summary: &radiant::runtime::GpuSignalSummary,
    bar_count: usize,
) -> Arc<[f32]> {
    let bar_count = bar_count.max(1);
    let target_frames_per_bar = summary.frames.max(1) as f32 / bar_count as f32;
    let level_index = summary.level_for_frames_per_pixel(target_frames_per_bar);
    let Some(level) = summary.levels.get(level_index) else {
        return Arc::from(vec![0.04; bar_count]);
    };
    let band_count = summary.band_count.max(1);
    let mut source_peaks = level
        .buckets
        .chunks(band_count)
        .map(|buckets| {
            buckets
                .first()
                .map(|bucket| bucket.min.abs().max(bucket.max.abs()))
                .filter(|peak| peak.is_finite())
                .unwrap_or(0.0)
        })
        .collect::<Vec<_>>();
    if source_peaks.is_empty() {
        source_peaks.push(0.0);
    }

    let source_bucket_count = source_peaks.len();
    let raw_levels = if source_bucket_count < bar_count {
        if source_bucket_count == 1 || bar_count == 1 {
            vec![source_peaks[0]; bar_count]
        } else {
            let last_source_index = (source_bucket_count - 1) as f32;
            let last_bar_index = (bar_count - 1) as f32;
            (0..bar_count)
                .map(|bar_index| {
                    let source_position = bar_index as f32 * last_source_index / last_bar_index;
                    let lower_index = source_position.floor() as usize;
                    let upper_index = source_position.ceil() as usize;
                    let fraction = source_position - lower_index as f32;
                    source_peaks[lower_index]
                        + (source_peaks[upper_index] - source_peaks[lower_index]) * fraction
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
                source_peaks[start..end].iter().copied().fold(0.0, f32::max)
            })
            .collect::<Vec<_>>()
    };
    let mut sorted = raw_levels.clone();
    sorted.sort_by(f32::total_cmp);
    let percentile_index = sorted.len().saturating_sub(1).saturating_mul(95) / 100;
    let ceiling = sorted
        .get(percentile_index)
        .copied()
        .unwrap_or(1.0)
        .max(0.0001);
    Arc::from(
        raw_levels
            .into_iter()
            .map(|peak| ((peak / ceiling).clamp(0.0, 1.0).powf(0.75)).max(0.04))
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
    cursor_ratio: Option<f32>,
    hovered: bool,
) {
    let count = levels.len().max(1);
    let pitch = bounds.width() / count as f32;
    let width = (pitch - BAR_GAP).max(0.75);
    let bottom = bounds.max.y - 1.0;
    let maximum_height = (bounds.height() - 2.0).max(1.0);
    for (index, level) in levels.iter().enumerate() {
        let x = bounds.min.x + index as f32 * pitch + (pitch - width) * 0.5;
        let height = (maximum_height * level.clamp(0.0, 1.0)).max(2.0);
        let played =
            cursor_ratio.is_some_and(|ratio| (index as f32 / count as f32) <= clamp_ratio(ratio));
        let color = match (played, hovered) {
            (true, true) => BAR_PLAYED_HOVER_COLOR,
            (true, false) => BAR_PLAYED_COLOR,
            (false, true) => BAR_HOVER_COLOR,
            (false, false) => BAR_COLOR,
        };
        fill_rect(
            primitives,
            widget_id,
            Rect::from_min_max(
                Point::new(x, bottom - height),
                Point::new(x + width, bottom),
            ),
            color,
        );
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
) {
    let x = bounds.x_for_ratio(ratio);
    fill_rect(
        primitives,
        widget_id,
        Rect::from_min_max(
            Point::new(x - CURSOR_WIDTH * 0.5, bounds.min.y),
            Point::new(x + CURSOR_WIDTH * 0.5, rail_y - CURSOR_GAP_ABOVE_RAIL),
        ),
        CURSOR_COLOR,
    );
}

pub fn clamp_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
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
    use super::{
        WaveformInteraction, WaveformWidget, clamp_ratio, display_bar_count, display_bar_levels,
        millis_for_ratio, ratio_for_millis,
    };
    use crate::audio::WaveformData;
    use radiant::{
        gui::types::{Point, Rect},
        runtime::GpuSignalSummary,
        widgets::{Widget, WidgetInput},
    };
    use std::sync::Arc;

    fn test_waveform() -> WaveformData {
        WaveformData {
            sample_rate: 48_000,
            channels: 1,
            duration_millis: 1_000,
            render_frames: 48_000,
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

    #[test]
    fn upper_waveform_emits_a_captured_playhead_drag_with_clamped_ratios() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let mut widget = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());

        assert_eq!(
            interaction(
                widget.handle_input(bounds, WidgetInput::primary_press(Point::new(35.0, 40.0)),)
            ),
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
            interaction(
                widget.handle_input(bounds, WidgetInput::primary_release(Point::new(60.0, 40.0)),)
            ),
            WaveformInteraction::PlayheadDragEnded { ratio: 0.5 }
        );
        assert!(!widget.playhead_dragging);
    }

    #[test]
    fn playhead_drag_state_survives_widget_synchronization() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let mut previous = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());
        interaction(
            previous.handle_input(bounds, WidgetInput::primary_press(Point::new(35.0, 40.0))),
        );

        let mut current = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());
        current.synchronize_from_previous(&previous);

        assert_eq!(
            interaction(
                current.handle_input(bounds, WidgetInput::pointer_move(Point::new(85.0, 40.0)),)
            ),
            WaveformInteraction::PlayheadDragMoved { ratio: 0.75 }
        );
    }

    #[test]
    fn lower_waveform_press_remains_a_comment_click() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let mut widget = WaveformWidget::new(Arc::new(test_waveform()), None, Vec::new());

        assert_eq!(
            interaction(
                widget.handle_input(bounds, WidgetInput::primary_press(Point::new(60.0, 105.0)),)
            ),
            WaveformInteraction::Clicked {
                ratio: 0.5,
                lower: true,
            }
        );
        assert!(!widget.playhead_dragging);
        assert!(
            widget
                .handle_input(
                    bounds,
                    WidgetInput::primary_release(Point::new(60.0, 105.0))
                )
                .is_none()
        );
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
    fn denser_summaries_preserve_peak_when_downsampling() {
        let summary = GpuSignalSummary::from_interleaved_samples(
            &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.9, 0.9, 0.9, 0.9, 0.1, 0.1],
            12,
            1,
        );
        let levels = display_bar_levels(&summary, 5);

        assert_eq!(levels.len(), 5);
        assert!(levels[3] > 0.99);
        assert!(levels[4] > 0.99);
        assert!((levels[3] - levels[4]).abs() < 1e-6);
    }
}

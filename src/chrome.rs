//! Cadence-owned domain widgets that are not generic Radiant chrome.
//!
//! Cadence uses Radiant's default surfaces, controls, text, and scrolling. The
//! two widgets in this module remain because they carry Cadence-specific
//! behavior: the LUFS meter paints domain data, and comment hover emits the
//! reducer's enter/exit messages without painting a second hover surface.

use radiant::{
    gui::types::{Point, Rect},
    layout::LayoutOutput,
    prelude as ui,
    runtime::{
        PaintFillPolygon, PaintFillRect, PaintPrimitive, PaintStrokePolygon, PaintTextAlign,
        PaintTextMetrics, push_text_run_with_metrics,
    },
    theme::ThemeTokens,
    widgets::{Widget, WidgetCommon, WidgetInput, WidgetOutput},
};

const METER_RADIUS: f32 = 8.0;
const METER_SEGMENTS: usize = 12;
const METER_MIN_LUFS: f32 = -24.0;
const METER_MAX_LUFS: f32 = 0.0;
// A club-oriented techno heuristic, not a delivery standard. The indicator
// leaves room for more dynamic or streaming-first masters below this band.
const TECHNO_LOUDNESS_MIN_LUFS: f32 = -9.0;
const TECHNO_LOUDNESS_MAX_LUFS: f32 = -6.0;

/// Pointer-move-only hover surface for a comment row.
///
/// The mapper remains an input layer so the waveform can highlight the
/// corresponding timestamped comment node. It intentionally contributes no
/// paint; the row's native Radiant controls own all visible hover treatment.
pub fn comment_hover<Message: Clone + 'static>(
    entered: Message,
    exited: Message,
) -> ui::View<Message> {
    ui::custom_widget_mapped(
        CommentHoverWidget::new(),
        move |interaction| match interaction {
            CommentHoverInteraction::Entered => entered.clone(),
            CommentHoverInteraction::Exited => exited.clone(),
        },
    )
}

/// Build the passive playback loudness meter shown beside a waveform.
pub fn lufs_meter<Message: 'static>(value: Option<f32>, analyzing: bool) -> ui::View<Message> {
    ui::custom_widget(LufsMeterWidget::new(value, analyzing), |_| None)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentHoverInteraction {
    Entered,
    Exited,
}

#[derive(Clone, Debug)]
struct CommentHoverWidget {
    common: WidgetCommon,
}

impl CommentHoverWidget {
    fn new() -> Self {
        Self {
            common: WidgetCommon::fixed(0, 1.0, 1.0)
                .with_pointer_focus()
                .without_default_chrome(),
        }
    }
}

impl Widget for CommentHoverWidget {
    fn common(&self) -> &WidgetCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut WidgetCommon {
        &mut self.common
    }

    fn accepts_pointer_move(&self) -> bool {
        true
    }

    fn accepts_pointer_input(&self, input: &WidgetInput) -> bool {
        matches!(input, WidgetInput::PointerMove { .. })
    }

    fn handle_input(&mut self, bounds: Rect, input: WidgetInput) -> Option<WidgetOutput> {
        let WidgetInput::PointerMove { position, .. } = input else {
            return None;
        };

        let hovered = bounds.contains(position);
        if hovered == self.common.state.hovered {
            return None;
        }
        self.common.state.hovered = hovered;
        Some(WidgetOutput::typed(if hovered {
            CommentHoverInteraction::Entered
        } else {
            CommentHoverInteraction::Exited
        }))
    }

    fn synchronize_from_previous(&mut self, previous: &dyn Widget) {
        let Some(previous) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.common.state.hovered = previous.common.state.hovered;
    }

    fn append_paint(
        &self,
        _primitives: &mut Vec<PaintPrimitive>,
        _bounds: Rect,
        _layout: &LayoutOutput,
        _theme: &ThemeTokens,
    ) {
        // This widget is an interaction mapper only. Native Radiant controls
        // below it remain the sole owner of visible hover chrome.
    }
}

#[derive(Clone, Debug)]
struct LufsMeterWidget {
    common: WidgetCommon,
    value: Option<f32>,
    analyzing: bool,
}

impl LufsMeterWidget {
    fn new(value: Option<f32>, analyzing: bool) -> Self {
        Self {
            common: WidgetCommon::fixed(0, 76.0, 250.0),
            value: value.filter(|value| value.is_finite()),
            analyzing,
        }
    }
}

impl Widget for LufsMeterWidget {
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
        paint_lufs_meter(
            primitives,
            self.common.id,
            bounds,
            self.value,
            self.analyzing,
            theme,
        );
    }
}

/// Paint the complete meter over a retained frame at the frame's cached bounds.
pub(crate) fn paint_lufs_meter_overlay(
    primitives: &mut Vec<PaintPrimitive>,
    bounds: Rect,
    widget_id: u64,
    value: Option<f32>,
    analyzing: bool,
    theme: &ThemeTokens,
) {
    paint_lufs_meter(primitives, widget_id, bounds, value, analyzing, theme);
}

/// Recover the complete cached bounds from the meter's retained background.
pub(crate) fn lufs_meter_bounds(
    plan: &radiant::runtime::SurfacePaintPlan,
    widget_id: u64,
) -> Option<Rect> {
    plan.primitives.iter().find_map(|primitive| {
        let PaintPrimitive::FillPolygon(fill) = primitive else {
            return None;
        };
        if fill.widget_id != widget_id {
            return None;
        }
        let first = fill.points.first()?;
        let (min_x, max_x, min_y, max_y) = fill.points.iter().skip(1).fold(
            (first.x, first.x, first.y, first.y),
            |(min_x, max_x, min_y, max_y), point| {
                (
                    min_x.min(point.x),
                    max_x.max(point.x),
                    min_y.min(point.y),
                    max_y.max(point.y),
                )
            },
        );
        let bounds = Rect::from_min_max(Point::new(min_x, min_y), Point::new(max_x, max_y));
        bounds.has_finite_positive_area().then_some(bounds)
    })
}

fn paint_lufs_meter(
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    bounds: Rect,
    value: Option<f32>,
    analyzing: bool,
    theme: &ThemeTokens,
) {
    if !bounds.has_finite_positive_area() {
        return;
    }

    let value = value.filter(|value| value.is_finite());
    let fraction = value.map(lufs_fraction).unwrap_or(0.0);
    let value_label = match value {
        Some(value) if value <= -59.9 => String::from("-∞"),
        Some(value) => format!("{value:.1}"),
        None if analyzing => String::from("…"),
        None => String::from("—"),
    };

    primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
        widget_id,
        points: rounded_corner_points(bounds, METER_RADIUS),
        color: theme.surface_overlay,
    }));

    push_text_run_with_metrics(
        primitives,
        widget_id,
        "LUFS",
        Rect::from_min_max(
            Point::new(bounds.min.x + 6.0, bounds.min.y + 8.0),
            Point::new(bounds.max.x - 6.0, bounds.min.y + 24.0),
        ),
        theme.text_muted,
        PaintTextAlign::Center,
        PaintTextMetrics::new(9.0, Some(11.0)),
    );
    push_text_run_with_metrics(
        primitives,
        widget_id,
        value_label,
        Rect::from_min_max(
            Point::new(bounds.min.x + 4.0, bounds.min.y + 26.0),
            Point::new(bounds.max.x - 4.0, bounds.min.y + 46.0),
        ),
        theme.text_primary,
        PaintTextAlign::Center,
        PaintTextMetrics::new(12.0, Some(15.0)),
    );

    let track = Rect::from_min_max(
        Point::new(bounds.min.x + 27.0, bounds.min.y + 56.0),
        Point::new(bounds.max.x - 27.0, bounds.max.y - 18.0),
    );
    if !track.has_finite_positive_area() {
        return;
    }
    let track_points = rounded_corner_points(track, 4.0);
    primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
        widget_id,
        points: track_points.clone(),
        color: theme.bg_tertiary,
    }));
    primitives.push(PaintPrimitive::StrokePolygon(PaintStrokePolygon {
        widget_id,
        points: track_points,
        color: theme.border,
        width: 2.0,
    }));

    let techno_range = techno_range_rect(track);
    if techno_range.has_finite_positive_area() {
        primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
            widget_id,
            points: rounded_corner_points(techno_range, 3.0),
            color: theme.grid_strong,
        }));
    }

    let fill_height = track.height() * fraction;
    if fill_height > 0.0 {
        let fill = Rect::from_min_max(
            Point::new(track.min.x, track.max.y - fill_height),
            track.max,
        );
        primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
            widget_id,
            points: rounded_corner_points(fill, 3.0),
            color: value
                .filter(|value| *value > -6.0)
                .map_or(theme.highlight_orange_soft, |_| theme.highlight_orange),
        }));
    }

    if techno_range.has_finite_positive_area() {
        primitives.push(PaintPrimitive::StrokePolygon(PaintStrokePolygon {
            widget_id,
            points: rounded_corner_points(techno_range, 3.0),
            color: theme.border_emphasis,
            width: 1.0,
        }));
    }

    for fraction in [0.0_f32, 0.5, 1.0] {
        let y = track.max.y - track.height() * fraction;
        for rect in [
            Rect::from_min_max(
                Point::new(bounds.min.x + 14.0, y - 0.5),
                Point::new(bounds.min.x + 22.0, y + 0.5),
            ),
            Rect::from_min_max(
                Point::new(bounds.max.x - 22.0, y - 0.5),
                Point::new(bounds.max.x - 14.0, y + 0.5),
            ),
        ] {
            primitives.push(PaintPrimitive::FillRect(PaintFillRect {
                widget_id,
                rect,
                color: theme.grid_soft,
            }));
        }
    }

    push_text_run_with_metrics(
        primitives,
        widget_id,
        "TECHNO",
        Rect::from_min_max(
            Point::new(bounds.min.x + 4.0, bounds.max.y - 16.0),
            Point::new(bounds.max.x - 4.0, bounds.max.y - 3.0),
        ),
        theme.text_muted,
        PaintTextAlign::Center,
        PaintTextMetrics::new(7.0, Some(9.0)),
    );
}

fn lufs_fraction(value: f32) -> f32 {
    ((value - METER_MIN_LUFS) / (METER_MAX_LUFS - METER_MIN_LUFS)).clamp(0.0, 1.0)
}

fn techno_range_rect(track: Rect) -> Rect {
    let top = track.max.y - track.height() * lufs_fraction(TECHNO_LOUDNESS_MAX_LUFS);
    let bottom = track.max.y - track.height() * lufs_fraction(TECHNO_LOUDNESS_MIN_LUFS);
    Rect::from_min_max(
        Point::new(track.min.x, top),
        Point::new(track.max.x, bottom),
    )
}

pub(crate) fn rounded_corner_points(bounds: Rect, radius: f32) -> std::sync::Arc<[Point]> {
    let radius = radius
        .max(0.0)
        .min(bounds.width().min(bounds.height()) * 0.5);
    let mut points = Vec::with_capacity(METER_SEGMENTS * 4 + 1);
    let corners = [
        (
            Point::new(bounds.max.x - radius, bounds.min.y + radius),
            -std::f32::consts::FRAC_PI_2,
        ),
        (
            Point::new(bounds.max.x - radius, bounds.max.y - radius),
            0.0,
        ),
        (
            Point::new(bounds.min.x + radius, bounds.max.y - radius),
            std::f32::consts::FRAC_PI_2,
        ),
        (
            Point::new(bounds.min.x + radius, bounds.min.y + radius),
            std::f32::consts::PI,
        ),
    ];

    for (corner_index, (center, start_angle)) in corners.into_iter().enumerate() {
        for segment in 0..=METER_SEGMENTS {
            if corner_index > 0 && segment == 0 {
                continue;
            }
            let angle =
                start_angle + std::f32::consts::FRAC_PI_2 * segment as f32 / METER_SEGMENTS as f32;
            points.push(Point::new(
                center.x + radius * angle.cos(),
                center.y + radius * angle.sin(),
            ));
        }
    }

    points.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use radiant::{
        runtime::PaintPrimitive,
        theme::ThemeTokens,
        widgets::{Widget, WidgetInput},
    };

    #[test]
    fn lufs_meter_paints_value_and_supplied_theme_colors() {
        let theme = ThemeTokens::light();
        let bounds = Rect::from_size(68.0, 250.0);
        let meter = LufsMeterWidget::new(Some(-4.0), false).paint_plan(
            bounds,
            &LayoutOutput::default(),
            &theme,
        );
        let labels = meter.text_label_strings();

        assert!(labels.iter().any(|label| label == "LUFS"));
        assert!(labels.iter().any(|label| label == "-4.0"));
        assert!(labels.iter().any(|label| label == "TECHNO"));
        assert!(meter.primitives.iter().any(|primitive| matches!(
            primitive,
            PaintPrimitive::FillPolygon(fill) if fill.color == theme.surface_overlay
        )));
        assert!(meter.primitives.iter().any(|primitive| matches!(
            primitive,
            PaintPrimitive::FillPolygon(fill) if fill.color == theme.highlight_orange
        )));
    }

    #[test]
    fn lufs_meter_paints_the_techno_loudness_target_band() {
        let theme = ThemeTokens::light();
        let meter = LufsMeterWidget::new(Some(-7.0), false).paint_plan(
            Rect::from_size(68.0, 160.0),
            &LayoutOutput::default(),
            &theme,
        );

        assert!(meter.primitives.iter().any(|primitive| matches!(
            primitive,
            PaintPrimitive::FillPolygon(fill) if fill.color == theme.grid_strong
        )));
        let target_outline = meter
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::StrokePolygon(stroke)
                    if stroke.color == theme.border_emphasis
                        && (stroke.width - 1.0).abs() < f32::EPSILON =>
                {
                    Some(stroke)
                }
                _ => None,
            })
            .expect("the TECHNO target outline should be painted");
        let min_y = target_outline
            .points
            .iter()
            .map(|point| point.y)
            .fold(f32::INFINITY, f32::min);
        let max_y = target_outline
            .points
            .iter()
            .map(|point| point.y)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((min_y - 77.5).abs() < 0.01);
        assert!((max_y - 88.25).abs() < 0.01);
    }

    #[test]
    fn comment_hover_emits_transitions_without_paint() {
        let bounds = Rect::from_size(120.0, 44.0);
        let mut hover = CommentHoverWidget::new();

        assert!(hover.paint_plan_with_defaults(bounds).primitives.is_empty());
        assert_eq!(
            hover
                .handle_input(bounds, WidgetInput::pointer_move(Point::new(20.0, 12.0)))
                .and_then(|output| output.typed_copied::<CommentHoverInteraction>()),
            Some(CommentHoverInteraction::Entered)
        );
        assert_eq!(
            hover
                .handle_input(bounds, WidgetInput::pointer_move(Point::new(140.0, 16.0)))
                .and_then(|output| output.typed_copied::<CommentHoverInteraction>()),
            Some(CommentHoverInteraction::Exited)
        );
    }
}

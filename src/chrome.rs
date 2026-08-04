//! Cadence-owned visual chrome.
//!
//! Radiant's controls remain responsible for their interaction and state
//! treatment. This module only supplies the application shell, passive panel
//! surfaces, and the palette used by Cadence-owned paint and text.

use radiant::{
    gui::types::{Point, Rect, Rgba8},
    layout::LayoutOutput,
    prelude as ui,
    runtime::{
        PaintFillPolygon, PaintFillRect, PaintPrimitive, PaintStrokePolygon, PaintStrokePolyline,
    },
    theme::ThemeTokens,
    widgets::{Widget, WidgetCommon, WidgetInput, WidgetOutput},
};

pub const CANVAS: Rgba8 = Rgba8::new(31, 9, 5, 255);
pub const PANEL: Rgba8 = Rgba8::new(46, 13, 7, 245);
pub const RULE: Rgba8 = Rgba8::new(143, 50, 20, 220);
pub const RULE_SOFT: Rgba8 = Rgba8::new(99, 33, 17, 180);
pub const DIAGONAL: Rgba8 = Rgba8::new(105, 35, 18, 120);
pub const TEXT_PRIMARY: Rgba8 = Rgba8::new(220, 78, 28, 255);
pub const TEXT_MUTED: Rgba8 = Rgba8::new(153, 56, 28, 235);
pub const TEXT_DIM: Rgba8 = Rgba8::new(112, 40, 24, 220);
pub const ACCENT_ORANGE: Rgba8 = Rgba8::new(244, 91, 25, 255);
pub const ACCENT_ORANGE_SOFT: Rgba8 = Rgba8::new(204, 67, 24, 245);

const FRAME_INSET: f32 = 1.5;
const FRAME_MARGIN_RATIO: f32 = 0.035;
const FRAME_MARGIN_MIN: f32 = 18.0;
const FRAME_MARGIN_MAX: f32 = 32.0;
const FRAME_BAND: f32 = 32.0;
const PANEL_CUT: f32 = 7.0;
const PANEL_INNER_INSET: f32 = 4.0;

/// Build Cadence's passive canvas background. It is intentionally input-free,
/// so it can sit beneath the existing interactive view tree.
pub fn background<Message: 'static>() -> ui::View<Message> {
    ui::custom_widget(BackgroundWidget::new(), |_| None)
}

/// Build a passive outlined Cadence panel for use behind app-owned content.
pub fn panel<Message: 'static>() -> ui::View<Message> {
    ui::custom_widget(PanelWidget::new(), |_| None)
}

/// Apply Cadence's explicit primary text role to an app-owned label.
pub fn text<Message: 'static>(value: impl Into<ui::TextContent>) -> ui::View<Message> {
    ui::text(value).text_color(ui::TextColorRole::Custom(TEXT_PRIMARY))
}

/// Apply Cadence's muted burnt-orange text role to an app-owned label.
pub fn muted_text<Message: 'static>(value: impl Into<ui::TextContent>) -> ui::View<Message> {
    ui::text(value).text_color(ui::TextColorRole::Custom(TEXT_MUTED))
}

#[derive(Clone, Debug)]
struct BackgroundWidget {
    common: WidgetCommon,
}

impl BackgroundWidget {
    fn new() -> Self {
        Self {
            common: WidgetCommon::fixed(0, 1.0, 1.0),
        }
    }
}

impl Widget for BackgroundWidget {
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
        _theme: &ThemeTokens,
    ) {
        if !bounds.has_finite_positive_area() {
            return;
        }

        primitives.push(PaintPrimitive::FillRect(PaintFillRect {
            widget_id: self.common.id,
            rect: bounds,
            color: CANVAS,
        }));

        let frame = inset_rect(bounds, frame_margin(bounds));
        primitives.push(PaintPrimitive::StrokePolygon(PaintStrokePolygon {
            widget_id: self.common.id,
            points: cut_corner_points(frame, PANEL_CUT),
            color: RULE,
            width: 1.0,
        }));

        let band = frame.height().min(FRAME_BAND);
        let header_y = frame.min.y + band;
        let footer_y = frame.max.y - band;
        push_bounded_rule(
            primitives,
            self.common.id,
            frame,
            Rect::from_min_max(
                Point::new(frame.min.x + FRAME_INSET, header_y),
                Point::new(frame.max.x - FRAME_INSET, header_y + 1.0),
            ),
            RULE_SOFT,
        );
        push_bounded_rule(
            primitives,
            self.common.id,
            frame,
            Rect::from_min_max(
                Point::new(frame.min.x + FRAME_INSET, footer_y),
                Point::new(frame.max.x - FRAME_INSET, footer_y + 1.0),
            ),
            RULE_SOFT,
        );

        let capsule_width = (frame.width() * 0.42).min(frame.width());
        let capsule_height = 16.0_f32.min(frame.height() * 0.25);
        let capsule_offset_y = (frame.height() - capsule_height).clamp(0.0, 8.0);
        let capsule = Rect::from_min_max(
            Point::new(
                frame.min.x + (frame.width() - capsule_width) * 0.5,
                frame.min.y + capsule_offset_y,
            ),
            Point::new(
                frame.max.x - (frame.width() - capsule_width) * 0.5,
                frame.min.y + capsule_offset_y + capsule_height,
            ),
        );
        primitives.push(PaintPrimitive::StrokePolygon(PaintStrokePolygon {
            widget_id: self.common.id,
            points: rounded_corner_points(capsule, capsule_height * 0.5),
            color: RULE,
            width: 1.0,
        }));
        let capsule_center = (capsule.min.x + capsule.max.x) * 0.5;
        for offset in [-12.0, 0.0, 12.0] {
            let marker_x = capsule_center + offset;
            push_bounded_rule(
                primitives,
                self.common.id,
                frame,
                Rect::from_min_max(
                    Point::new(marker_x - 2.0, capsule.min.y + 5.0),
                    Point::new(marker_x + 2.0, capsule.min.y + 9.0),
                ),
                ACCENT_ORANGE_SOFT,
            );
        }

        let divider_x = frame.min.x + frame.width() * 0.27;
        primitives.push(PaintPrimitive::StrokePolyline(PaintStrokePolyline {
            widget_id: self.common.id,
            points: [
                clamp_point(frame, Point::new(divider_x, header_y + 2.0)),
                clamp_point(frame, Point::new(divider_x, footer_y - 2.0)),
            ]
            .into(),
            color: DIAGONAL,
            width: 1.0,
        }));

        let diagonal_start =
            clamp_point(frame, Point::new(frame.min.x + FRAME_INSET, header_y + 2.0));
        let diagonal_end =
            clamp_point(frame, Point::new(frame.max.x - FRAME_INSET, footer_y - 2.0));
        primitives.push(PaintPrimitive::StrokePolyline(PaintStrokePolyline {
            widget_id: self.common.id,
            points: [diagonal_start, diagonal_end].into(),
            color: DIAGONAL,
            width: 1.0,
        }));
        primitives.push(PaintPrimitive::StrokePolyline(PaintStrokePolyline {
            widget_id: self.common.id,
            points: [
                clamp_point(
                    frame,
                    Point::new(frame.max.x - FRAME_INSET, header_y + 26.0),
                ),
                clamp_point(
                    frame,
                    Point::new(frame.min.x + FRAME_INSET, footer_y - 26.0),
                ),
            ]
            .into(),
            color: DIAGONAL,
            width: 1.0,
        }));
    }
}

#[derive(Clone, Debug)]
struct PanelWidget {
    common: WidgetCommon,
}

impl PanelWidget {
    fn new() -> Self {
        Self {
            common: WidgetCommon::fixed(0, 1.0, 1.0),
        }
    }
}

impl Widget for PanelWidget {
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
        _theme: &ThemeTokens,
    ) {
        if !bounds.has_finite_positive_area() {
            return;
        }

        let points = cut_corner_points(bounds, PANEL_CUT);
        primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
            widget_id: self.common.id,
            points: points.clone(),
            color: PANEL,
        }));
        primitives.push(PaintPrimitive::StrokePolygon(PaintStrokePolygon {
            widget_id: self.common.id,
            points,
            color: RULE,
            width: 1.0,
        }));

        let inner = inset_rect(bounds, PANEL_INNER_INSET);
        if inner.has_finite_positive_area() {
            primitives.push(PaintPrimitive::StrokePolygon(PaintStrokePolygon {
                widget_id: self.common.id,
                points: cut_corner_points(inner, PANEL_CUT * 0.55),
                color: RULE_SOFT,
                width: 0.7,
            }));
        }

        let content_width = (bounds.width() - PANEL_CUT * 2.0).max(0.0);
        if content_width > 0.0 {
            let accent_width = content_width * 0.46;
            push_bounded_rule(
                primitives,
                self.common.id,
                bounds,
                Rect::from_min_max(
                    Point::new(bounds.min.x + PANEL_CUT, bounds.min.y + 2.0),
                    Point::new(bounds.min.x + PANEL_CUT + accent_width, bounds.min.y + 3.0),
                ),
                RULE_SOFT,
            );
            push_bounded_rule(
                primitives,
                self.common.id,
                bounds,
                Rect::from_min_max(
                    Point::new(
                        bounds.max.x - PANEL_CUT - content_width * 0.18,
                        bounds.min.y + 2.0,
                    ),
                    Point::new(bounds.max.x - PANEL_CUT, bounds.min.y + 3.0),
                ),
                RULE_SOFT,
            );
            push_bounded_rule(
                primitives,
                self.common.id,
                bounds,
                Rect::from_min_max(
                    Point::new(bounds.min.x + PANEL_CUT, bounds.max.y - 3.0),
                    Point::new(bounds.max.x - PANEL_CUT, bounds.max.y - 2.0),
                ),
                RULE_SOFT,
            );
            let marker_x = (bounds.min.x + bounds.max.x) * 0.5;
            push_bounded_rule(
                primitives,
                self.common.id,
                bounds,
                Rect::from_min_max(
                    Point::new(marker_x - 3.0, bounds.max.y - 4.0),
                    Point::new(marker_x + 3.0, bounds.max.y - 2.0),
                ),
                ACCENT_ORANGE_SOFT,
            );
        }
    }
}

fn push_rule(primitives: &mut Vec<PaintPrimitive>, widget_id: u64, rect: Rect, color: Rgba8) {
    primitives.push(PaintPrimitive::FillRect(PaintFillRect {
        widget_id,
        rect,
        color,
    }));
}

fn push_bounded_rule(
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    bounds: Rect,
    rect: Rect,
    color: Rgba8,
) {
    if let Some(rect) = clipped_rect(bounds, rect) {
        push_rule(primitives, widget_id, rect, color);
    }
}

fn clipped_rect(bounds: Rect, rect: Rect) -> Option<Rect> {
    if !bounds.has_finite_positive_area()
        || !rect.min.x.is_finite()
        || !rect.min.y.is_finite()
        || !rect.max.x.is_finite()
        || !rect.max.y.is_finite()
    {
        return None;
    }

    let min_x = rect.min.x.clamp(bounds.min.x, bounds.max.x);
    let min_y = rect.min.y.clamp(bounds.min.y, bounds.max.y);
    let max_x = rect.max.x.clamp(bounds.min.x, bounds.max.x);
    let max_y = rect.max.y.clamp(bounds.min.y, bounds.max.y);
    if min_x >= max_x || min_y >= max_y {
        return None;
    }
    Some(Rect::from_min_max(
        Point::new(min_x, min_y),
        Point::new(max_x, max_y),
    ))
}

fn clamp_point(bounds: Rect, point: Point) -> Point {
    Point::new(
        point.x.clamp(bounds.min.x, bounds.max.x),
        point.y.clamp(bounds.min.y, bounds.max.y),
    )
}

fn inset_rect(bounds: Rect, inset: f32) -> Rect {
    Rect::from_min_max(
        Point::new(bounds.min.x + inset, bounds.min.y + inset),
        Point::new(bounds.max.x - inset, bounds.max.y - inset),
    )
}

fn frame_margin(bounds: Rect) -> f32 {
    let smallest_dimension = bounds.width().min(bounds.height()).max(0.0);
    (smallest_dimension * FRAME_MARGIN_RATIO)
        .clamp(FRAME_MARGIN_MIN, FRAME_MARGIN_MAX)
        .min(smallest_dimension * 0.45)
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

pub(crate) fn cut_corner_points(bounds: Rect, requested_cut: f32) -> std::sync::Arc<[Point]> {
    let cut = requested_cut
        .max(0.0)
        .min(bounds.width().min(bounds.height()) * 0.5);
    [
        Point::new(bounds.min.x + cut, bounds.min.y),
        Point::new(bounds.max.x - cut, bounds.min.y),
        Point::new(bounds.max.x, bounds.min.y + cut),
        Point::new(bounds.max.x, bounds.max.y - cut),
        Point::new(bounds.max.x - cut, bounds.max.y),
        Point::new(bounds.min.x + cut, bounds.max.y),
        Point::new(bounds.min.x, bounds.max.y - cut),
        Point::new(bounds.min.x, bounds.min.y + cut),
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use radiant::{application::IntoView, gui::types::Vector2, widgets::Widget};

    #[test]
    fn chrome_paint_emits_cadence_palette_colors() {
        let bounds = Rect::from_size(320.0, 180.0);
        let background = BackgroundWidget::new().paint_plan_with_defaults(bounds);
        assert!(background.primitives.iter().any(|primitive| matches!(
            primitive,
            PaintPrimitive::FillRect(fill) if fill.rect == bounds && fill.color == CANVAS
        )));
        assert!(background.primitives.iter().any(|primitive| matches!(
            primitive,
            PaintPrimitive::StrokePolygon(stroke) if stroke.color == RULE
        )));

        let panel = PanelWidget::new().paint_plan_with_defaults(bounds);
        assert!(panel.primitives.iter().any(|primitive| matches!(
            primitive,
            PaintPrimitive::FillPolygon(fill) if fill.color == PANEL
        )));
        assert!(panel.primitives.iter().any(|primitive| matches!(
            primitive,
            PaintPrimitive::StrokePolygon(stroke) if stroke.color == RULE
        )));

        let primary = text::<()>("Primary")
            .view_frame_at_size_with_default_theme(Vector2::new(120.0, 24.0))
            .paint_plan;
        assert_eq!(primary.first_text_color("Primary"), Some(TEXT_PRIMARY));

        let muted = muted_text::<()>("Muted")
            .view_frame_at_size_with_default_theme(Vector2::new(120.0, 24.0))
            .paint_plan;
        assert_eq!(muted.first_text_color("Muted"), Some(TEXT_MUTED));
    }

    #[test]
    fn chrome_surfaces_are_passive_to_pointer_routing() {
        let input = WidgetInput::primary_press(Point::new(10.0, 10.0));

        assert!(!BackgroundWidget::new().accepts_pointer_move());
        assert!(!BackgroundWidget::new().accepts_pointer_input(&input));
        assert!(!PanelWidget::new().accepts_pointer_move());
        assert!(!PanelWidget::new().accepts_pointer_input(&input));
    }

    fn assert_point_is_bounded(bounds: Rect, point: Point) {
        assert!(point.x.is_finite() && point.y.is_finite());
        assert!(bounds.contains(point));
    }

    fn assert_rect_is_bounded(bounds: Rect, rect: Rect) {
        assert!(
            rect.min.x.is_finite()
                && rect.min.y.is_finite()
                && rect.max.x.is_finite()
                && rect.max.y.is_finite()
        );
        assert!(
            rect.min.x >= bounds.min.x
                && rect.min.y >= bounds.min.y
                && rect.max.x <= bounds.max.x
                && rect.max.y <= bounds.max.y
        );
    }

    fn assert_paint_plan_is_bounded(bounds: Rect, primitives: &[PaintPrimitive]) {
        for primitive in primitives {
            match primitive {
                PaintPrimitive::FillRect(fill) => assert_rect_is_bounded(bounds, fill.rect),
                PaintPrimitive::FillPolygon(fill) => {
                    for point in fill.points.iter().copied() {
                        assert_point_is_bounded(bounds, point);
                    }
                }
                PaintPrimitive::StrokePolygon(stroke) => {
                    for point in stroke.points.iter().copied() {
                        assert_point_is_bounded(bounds, point);
                    }
                }
                PaintPrimitive::StrokePolyline(stroke) => {
                    for point in stroke.points.iter().copied() {
                        assert_point_is_bounded(bounds, point);
                    }
                }
                _ => {}
            }
        }
    }

    #[test]
    fn chrome_geometry_stays_within_tiny_bounds() {
        for (width, height) in [
            (320.0, 180.0),
            (20.0, 10.0),
            (20.0, 2.0),
            (2.0, 40.0),
            (1.0, 1.0),
        ] {
            let bounds = Rect::from_size(width, height);
            assert_paint_plan_is_bounded(
                bounds,
                &BackgroundWidget::new()
                    .paint_plan_with_defaults(bounds)
                    .primitives,
            );
            assert_paint_plan_is_bounded(
                bounds,
                &PanelWidget::new()
                    .paint_plan_with_defaults(bounds)
                    .primitives,
            );
        }
    }

    #[test]
    fn cut_corner_points_stay_on_panel_bounds() {
        let bounds = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 80.0));
        let points = cut_corner_points(bounds, PANEL_CUT);

        assert_eq!(points.len(), 8);
        assert!(points.iter().all(|point| bounds.contains(*point)));
        assert_eq!(points[0].y, bounds.min.y);
        assert_eq!(points[2].x, bounds.max.x);
        assert_eq!(points[4].y, bounds.max.y);
        assert_eq!(points[6].x, bounds.min.x);
    }

    #[test]
    fn background_frame_is_inset_and_has_instrument_markers() {
        let bounds = Rect::from_size(480.0, 640.0);
        let background = BackgroundWidget::new().paint_plan_with_defaults(bounds);

        let frame = background
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::StrokePolygon(stroke)
                    if stroke.color == RULE && stroke.points.len() == 8 =>
                {
                    Some(stroke.points.clone())
                }
                _ => None,
            })
            .expect("the background should paint its inset outer frame");
        assert!(frame.iter().all(|point| bounds.contains(*point)));
        assert!(frame.iter().any(|point| point.x > bounds.min.x));
        assert!(frame.iter().any(|point| point.y > bounds.min.y));

        assert!(background.primitives.iter().any(|primitive| matches!(
            primitive,
            PaintPrimitive::StrokePolygon(stroke)
                if stroke.color == RULE && stroke.points.len() == 8
        )));
        assert!(background.primitives.iter().any(|primitive| matches!(
            primitive,
            PaintPrimitive::FillRect(fill) if fill.color == ACCENT_ORANGE_SOFT
        )));
    }
}

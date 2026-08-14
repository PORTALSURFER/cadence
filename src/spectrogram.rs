//! Passive live-spectrogram heatmap for the native Review/Audition surface.
//!
//! The transport owns capture and analysis. This module only paints the latest
//! immutable, quantized frame: frequency increases from left to right, while
//! the oldest retained row is above the newest row at the bottom edge.

use crate::transport::{
    LIVE_SPECTROGRAM_BAND_COUNT, LIVE_SPECTROGRAM_MAX_HISTORY, LiveSpectrogramFrame,
};
use radiant::{
    gui::types::{Point, Rect, Rgba8},
    layout::LayoutOutput,
    prelude as ui,
    runtime::{
        PaintFillRect, PaintFillRectBatch, PaintPrimitive, PaintStrokePolyline, PaintStrokeRect,
    },
    theme::ThemeTokens,
    widgets::{FocusBehavior, PaintBounds, Widget, WidgetCommon, WidgetInput, WidgetOutput},
};
use std::sync::Arc;

pub const HEIGHT: f32 = 78.0;

const PALETTE: [Rgba8; 8] = [
    Rgba8::new(8, 15, 25, 255),
    Rgba8::new(13, 39, 63, 255),
    Rgba8::new(18, 77, 105, 255),
    Rgba8::new(27, 132, 112, 255),
    Rgba8::new(71, 182, 99, 255),
    Rgba8::new(211, 177, 67, 255),
    Rgba8::new(246, 125, 53, 255),
    Rgba8::new(255, 226, 151, 255),
];
const SPECTRUM_PLOT_BACKGROUND: Rgba8 = Rgba8::new(10, 17, 27, 255);

#[derive(Clone, Debug)]
struct SpectrogramWidget {
    common: WidgetCommon,
    frame: Arc<LiveSpectrogramFrame>,
    mode: crate::LiveSpectrogramMode,
}

impl SpectrogramWidget {
    fn new(frame: Arc<LiveSpectrogramFrame>, mode: crate::LiveSpectrogramMode) -> Self {
        let mut common = WidgetCommon::fixed(0, 640.0, HEIGHT).without_default_chrome();
        common.focus = FocusBehavior::None;
        common.paint.bounds = PaintBounds::ClipToRect;
        common.paint.paints_focus = false;
        common.paint.paints_state_layers = false;
        Self {
            common,
            frame,
            mode,
        }
    }

    fn plot_rect(bounds: Rect) -> Rect {
        bounds.inset(1.0, 1.0, 1.0, 1.0)
    }

    fn append_heatmap(&self, primitives: &mut Vec<PaintPrimitive>, plot: Rect) {
        if !plot.has_finite_positive_area() || !self.frame.is_valid() {
            return;
        }
        let rows = self.frame.row_count.min(LIVE_SPECTROGRAM_MAX_HISTORY);
        let row_height = plot.height() / LIVE_SPECTROGRAM_MAX_HISTORY as f32;
        let mut batches: [Vec<Rect>; PALETTE.len()] = std::array::from_fn(|_| Vec::new());
        for row in 0..rows {
            // The frame stores rows oldest-to-newest. Anchor the newest row at
            // the bottom and leave unused history space above it.
            let rows_from_bottom = rows - 1 - row;
            let y1 = plot.max.y - row_height * rows_from_bottom as f32;
            let y0 = (y1 - row_height).max(plot.min.y);
            for band in 0..LIVE_SPECTROGRAM_BAND_COUNT {
                // Band zero is deliberately the leftmost cell: x is frequency.
                let x0 =
                    plot.min.x + plot.width() * band as f32 / LIVE_SPECTROGRAM_BAND_COUNT as f32;
                let x1 = plot.min.x
                    + plot.width() * (band + 1) as f32 / LIVE_SPECTROGRAM_BAND_COUNT as f32;
                let rect = Rect::from_min_max(Point::new(x0, y0), Point::new(x1, y1));
                if !rect.has_finite_positive_area() {
                    continue;
                }
                let palette_index = (self.frame.value(row, band) as usize * (PALETTE.len() - 1)
                    + (u8::MAX as usize / 2))
                    / u8::MAX as usize;
                batches[palette_index].push(rect);
            }
        }
        for (rects, color) in batches.into_iter().zip(PALETTE) {
            if !rects.is_empty() {
                primitives.push(PaintPrimitive::FillRectBatch(PaintFillRectBatch {
                    widget_id: self.common.id,
                    rects: Arc::from(rects),
                    color,
                }));
            }
        }
    }

    fn append_spectrum(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        plot: Rect,
        theme: &ThemeTokens,
    ) {
        if !plot.has_finite_positive_area() || !self.frame.is_valid() {
            return;
        }
        let latest_row = self.frame.row_count.saturating_sub(1);
        let points = (0..LIVE_SPECTROGRAM_BAND_COUNT)
            .map(|band| {
                let level = self.frame.value(latest_row, band) as f32 / u8::MAX as f32;
                let x = plot.min.x
                    + plot.width() * band as f32 / (LIVE_SPECTROGRAM_BAND_COUNT - 1) as f32;
                let y = plot.max.y - plot.height() * level;
                Point::new(x, y)
            })
            .collect::<Vec<_>>();
        if points.len() >= 2 {
            primitives.push(PaintPrimitive::StrokePolyline(PaintStrokePolyline {
                widget_id: self.common.id,
                points: Arc::from(points),
                color: theme.highlight_orange,
                width: 1.5,
            }));
        }
    }
}

impl Widget for SpectrogramWidget {
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
        let plot = Self::plot_rect(bounds);
        primitives.push(PaintPrimitive::FillRect(PaintFillRect {
            widget_id: self.common.id,
            rect: bounds,
            color: theme.bg_primary.blend_toward(theme.surface_overlay, 0.35),
        }));
        if plot.has_finite_positive_area() {
            primitives.push(PaintPrimitive::FillRect(PaintFillRect {
                widget_id: self.common.id,
                rect: plot,
                color: match self.mode {
                    crate::LiveSpectrogramMode::Waterfall => PALETTE[0],
                    crate::LiveSpectrogramMode::Spectrum => SPECTRUM_PLOT_BACKGROUND,
                },
            }));
            match self.mode {
                crate::LiveSpectrogramMode::Waterfall => self.append_heatmap(primitives, plot),
                crate::LiveSpectrogramMode::Spectrum => {
                    self.append_spectrum(primitives, plot, theme)
                }
            }
            primitives.push(PaintPrimitive::StrokeRect(PaintStrokeRect {
                widget_id: self.common.id,
                rect: plot,
                color: theme.border_emphasis,
                width: 1.0,
            }));
        }
    }

    fn synchronize_from_previous(&mut self, previous: &dyn Widget) {
        let Some(previous) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        if Arc::ptr_eq(&self.frame, &previous.frame) {
            self.common.state = previous.common.state;
        }
    }
}

pub fn view<Message: 'static>(
    frame: Arc<LiveSpectrogramFrame>,
    mode: crate::LiveSpectrogramMode,
) -> ui::View<Message> {
    ui::custom_widget(SpectrogramWidget::new(frame, mode), |_| None)
        .height(HEIGHT)
        .fill_width()
}

#[cfg(test)]
mod tests {
    use super::{PALETTE, SPECTRUM_PLOT_BACKGROUND, SpectrogramWidget};
    use crate::LiveSpectrogramMode;
    use crate::transport::{
        LIVE_SPECTROGRAM_BAND_COUNT, LIVE_SPECTROGRAM_MAX_HISTORY, LiveSpectrogramFrame,
    };
    use radiant::{
        gui::types::{Point, Rect, Vector2},
        layout::LayoutOutput,
        runtime::PaintPrimitive,
        theme::ThemeTokens,
        widgets::Widget,
    };
    use std::sync::Arc;

    fn test_frame() -> Arc<LiveSpectrogramFrame> {
        let row_count = 2;
        let mut values = vec![0_u8; row_count * LIVE_SPECTROGRAM_BAND_COUNT];
        values[0] = u8::MAX;
        values[(row_count - 1) * LIVE_SPECTROGRAM_BAND_COUNT + LIVE_SPECTROGRAM_BAND_COUNT - 1] =
            u8::MAX;
        Arc::new(LiveSpectrogramFrame {
            generation: 4,
            epoch: 2,
            revision: 1,
            sample_rate: 48_000,
            row_count,
            values: Arc::from(values.into_boxed_slice()),
        })
    }

    #[test]
    fn live_frame_paints_frequency_left_to_right_and_newest_at_bottom() {
        let widget = SpectrogramWidget::new(test_frame(), LiveSpectrogramMode::Waterfall);
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let primitives =
            widget.paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());
        let high_cells = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillRectBatch(batch)
                    if batch.color == PALETTE[PALETTE.len() - 1] =>
                {
                    Some(batch.rects.iter().copied().collect::<Vec<_>>())
                }
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(high_cells.len(), 2);
        let left = high_cells
            .iter()
            .min_by(|a, b| a.min.x.total_cmp(&b.min.x))
            .expect("low band cell");
        let right = high_cells
            .iter()
            .max_by(|a, b| a.min.x.total_cmp(&b.min.x))
            .expect("high band cell");
        assert!(left.min.x < right.min.x);
        assert!(left.min.y < right.min.y);
        assert!(right.max.y > left.max.y);
    }

    #[test]
    fn live_frame_paint_is_batched_and_bounded() {
        let mut values = vec![0_u8; LIVE_SPECTROGRAM_MAX_HISTORY * LIVE_SPECTROGRAM_BAND_COUNT];
        let last = values.len() - 1;
        values[last] = u8::MAX;
        let frame = Arc::new(LiveSpectrogramFrame {
            generation: 1,
            epoch: 1,
            revision: 1,
            sample_rate: 48_000,
            row_count: LIVE_SPECTROGRAM_MAX_HISTORY,
            values: Arc::from(values.into_boxed_slice()),
        });
        let widget = SpectrogramWidget::new(frame, LiveSpectrogramMode::Waterfall);
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let primitives =
            widget.paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());
        let batched_cells = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillRectBatch(batch) => Some(batch.rects.len()),
                _ => None,
            })
            .sum::<usize>();
        assert_eq!(
            batched_cells,
            LIVE_SPECTROGRAM_MAX_HISTORY * LIVE_SPECTROGRAM_BAND_COUNT
        );
        assert!(primitives.len() <= PALETTE.len() + 3);
    }

    #[test]
    fn mode_selects_waterfall_or_spectrum_paint_path() {
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let waterfall = SpectrogramWidget::new(test_frame(), LiveSpectrogramMode::Waterfall)
            .paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());
        let spectrum = SpectrogramWidget::new(test_frame(), LiveSpectrogramMode::Spectrum)
            .paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());

        assert!(
            waterfall
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::FillRectBatch(_)))
        );
        assert!(
            !waterfall
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::StrokePolyline(_)))
        );
        assert!(spectrum.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill) if fill.color == SPECTRUM_PLOT_BACKGROUND)
        }));
        assert!(
            spectrum
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::StrokePolyline(_)))
        );
        assert!(
            !spectrum
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::FillRectBatch(_)))
        );
    }

    #[test]
    fn spectrum_uses_latest_row_with_low_to_high_frequency_geometry() {
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let primitives = SpectrogramWidget::new(test_frame(), LiveSpectrogramMode::Spectrum)
            .paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());
        let line = primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::StrokePolyline(line) => Some(line),
                _ => None,
            })
            .expect("spectrum mode should paint one line");
        let plot = SpectrogramWidget::plot_rect(bounds);

        assert_eq!(line.points.len(), LIVE_SPECTROGRAM_BAND_COUNT);
        assert_eq!(line.color, ThemeTokens::default().highlight_orange);
        assert_eq!(line.points.first().expect("low band").x, plot.min.x);
        assert_eq!(line.points.last().expect("high band").x, plot.max.x);
        assert_eq!(line.points.first().expect("latest low band").y, plot.max.y);
        assert_eq!(line.points.last().expect("latest high band").y, plot.min.y);
        assert!(
            line.points
                .windows(2)
                .all(|points| points[0].x < points[1].x)
        );
        assert!(primitives.len() <= 4);
    }
}

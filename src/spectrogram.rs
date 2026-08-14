//! Passive spectrogram heatmap for the native Review/Audition surface.
//!
//! Audio analysis is owned by [`crate::audio`]. This module only maps the
//! compact, immutable decoded representation to dense Radiant paint batches;
//! it never performs DSP or retains source samples.

use crate::audio::SpectrogramData;
use radiant::{
    gui::types::{Point, Rect, Rgba8},
    layout::LayoutOutput,
    prelude as ui,
    runtime::{PaintFillRect, PaintFillRectBatch, PaintPrimitive, PaintStrokeRect},
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

#[derive(Clone, Debug)]
struct SpectrogramWidget {
    common: WidgetCommon,
    data: Arc<SpectrogramData>,
    cursor_ratio: Option<f32>,
}

impl SpectrogramWidget {
    fn new(data: Arc<SpectrogramData>, cursor_ratio: Option<f32>) -> Self {
        let mut common = WidgetCommon::fixed(0, 640.0, HEIGHT).without_default_chrome();
        common.focus = FocusBehavior::None;
        common.paint.bounds = PaintBounds::ClipToRect;
        common.paint.paints_focus = false;
        common.paint.paints_state_layers = false;
        Self {
            common,
            data,
            cursor_ratio: cursor_ratio.map(crate::waveform::clamp_ratio),
        }
    }

    fn plot_rect(bounds: Rect) -> Rect {
        bounds.inset(1.0, 1.0, 1.0, 1.0)
    }

    fn append_heatmap(&self, primitives: &mut Vec<PaintPrimitive>, plot: Rect) {
        if !plot.has_finite_positive_area() || self.data.column_count == 0 {
            return;
        }
        let columns = self
            .data
            .column_count
            .min(crate::audio::MAX_SPECTROGRAM_COLUMNS);
        let mut batches: [Vec<Rect>; PALETTE.len()] = std::array::from_fn(|_| Vec::new());
        for column in 0..columns {
            let x0 = plot.min.x + plot.width() * column as f32 / columns as f32;
            let x1 = plot.min.x + plot.width() * (column + 1) as f32 / columns as f32;
            for band in 0..crate::audio::SPECTROGRAM_BAND_COUNT {
                let y0 = plot.max.y
                    - plot.height() * (band + 1) as f32
                        / crate::audio::SPECTROGRAM_BAND_COUNT as f32;
                let y1 = plot.max.y
                    - plot.height() * band as f32 / crate::audio::SPECTROGRAM_BAND_COUNT as f32;
                let rect = Rect::from_min_max(Point::new(x0, y0), Point::new(x1, y1));
                if !rect.has_finite_positive_area() {
                    continue;
                }
                let palette_index = (self.data.value(column, band) as usize * (PALETTE.len() - 1)
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
                color: PALETTE[0],
            }));
            self.append_heatmap(primitives, plot);
            if let Some(ratio) = self.cursor_ratio {
                let x = plot.min.x + plot.width() * ratio;
                primitives.push(PaintPrimitive::FillRect(PaintFillRect {
                    widget_id: self.common.id,
                    rect: Rect::from_min_max(
                        Point::new((x - 1.0).max(plot.min.x), plot.min.y),
                        Point::new((x + 1.0).min(plot.max.x), plot.max.y),
                    ),
                    color: theme.highlight_orange_soft,
                }));
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
        if Arc::ptr_eq(&self.data, &previous.data) {
            self.common.state = previous.common.state;
        }
    }
}

pub fn view<Message: 'static>(
    data: Arc<SpectrogramData>,
    cursor_ratio: Option<f32>,
) -> ui::View<Message> {
    ui::custom_widget(SpectrogramWidget::new(data, cursor_ratio), |_| None)
        .height(HEIGHT)
        .fill_width()
}

#[cfg(test)]
mod tests {
    use super::SpectrogramWidget;
    use crate::audio::{SPECTROGRAM_BAND_COUNT, SpectrogramData};
    use radiant::{
        gui::types::{Point, Rect, Vector2},
        layout::LayoutOutput,
        runtime::PaintPrimitive,
        theme::ThemeTokens,
        widgets::Widget,
    };
    use std::sync::Arc;

    #[test]
    fn spectrogram_widget_paints_batched_heatmap_at_compact_size() {
        let magnitudes = (0..(4 * SPECTROGRAM_BAND_COUNT))
            .map(|index| (index % 8 * 36) as u8)
            .collect::<Vec<_>>();
        let data = Arc::new(SpectrogramData {
            column_count: 4,
            values: Arc::from(magnitudes.into_boxed_slice()),
        });
        let widget = SpectrogramWidget::new(data, Some(0.5));
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
        assert_eq!(batched_cells, 4 * SPECTROGRAM_BAND_COUNT);
        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::StrokeRect(stroke) if stroke.rect.width() > 700.0)
        }));
        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill) if fill.color == ThemeTokens::default().highlight_orange_soft)
        }));
    }
}

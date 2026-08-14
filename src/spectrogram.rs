//! Passive live-spectrogram heatmap for the native Review/Audition surface.
//!
//! The transport owns capture and analysis. This module only paints the latest
//! immutable, quantized frame: frequency increases from left to right, while
//! the oldest retained row is above the newest row at the bottom edge.

use crate::transport::{
    LIVE_SPECTROGRAM_BAND_COUNT, LIVE_SPECTROGRAM_MAX_HISTORY, LIVE_SPECTRUM_DISPLAY_CEILING_DB,
    LIVE_SPECTRUM_DISPLAY_FLOOR_DB, LiveSpectrogramFrame, live_display_frequency_bounds,
};
use radiant::{
    gui::types::{Point, Rect, Rgba8},
    layout::LayoutOutput,
    prelude as ui,
    runtime::{
        GpuShaderSurfaceDescriptor, GpuShaderSurfaceDescriptorParts, GpuSurfaceCapabilities,
        GpuSurfaceContent, PaintFillRect, PaintGpuSurface, PaintPrimitive, PaintStrokePolyline,
        PaintStrokeRect, PaintTextAlign, PaintTextMetrics, push_text_run_with_metrics,
    },
    theme::ThemeTokens,
    widgets::{FocusBehavior, PaintBounds, Widget, WidgetCommon, WidgetInput, WidgetOutput},
};
use std::sync::Arc;

pub const HEIGHT: f32 = 78.0;
pub const MIN_HEIGHT: f32 = 72.0;
pub const MAX_HEIGHT: f32 = 240.0;
pub const RESIZE_HANDLE_HEIGHT: f32 = 8.0;

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
const FREQUENCY_GRID: [(f32, &str); 5] = [
    (20.0, "20 Hz"),
    (100.0, "100"),
    (1_000.0, "1 kHz"),
    (10_000.0, "10 kHz"),
    (20_000.0, "20 kHz"),
];
const DECIBEL_GRID: [(f32, &str); 4] = [
    (0.0, "0 dB"),
    (-30.0, "-30"),
    (-60.0, "-60"),
    (-90.0, "-90"),
];
const GRID_LINE_ALPHA: u8 = 82;
const GRID_LABEL_ALPHA: u8 = 200;
const GRID_LABEL_FONT_SIZE: f32 = 9.0;
const WATERFALL_SURFACE_KEY: u64 = 0x4341_4445_4e43_4553;
const WATERFALL_SHADER_KEY: &str = "cadence/live-spectrogram-waterfall";

const WATERFALL_SHADER_WGSL: &str = r#"
struct SurfaceParams {
    dest: vec4<f32>,
    source: vec4<f32>,
    target_size: vec2<f32>,
    _padding: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> surface: SurfaceParams;

struct WaterfallParams {
    band_count: u32,
    row_count: u32,
    max_history: u32,
    _padding: u32,
};

@group(0) @binding(1)
var<uniform> params: WaterfallParams;

@group(0) @binding(2)
var<storage, read> history: array<u32>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
};

@vertex
fn vertex_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let local = corners[vertex_index];
    let pixel = surface.dest.xy + local * surface.dest.zw;
    let clip = vec2<f32>(
        pixel.x / surface.target_size.x * 2.0 - 1.0,
        1.0 - pixel.y / surface.target_size.y * 2.0,
    );
    var output: VertexOut;
    output.position = vec4<f32>(clip, 0.0, 1.0);
    output.local = local;
    return output;
}

fn palette_color(level: f32) -> vec3<f32> {
    var palette = array<vec3<f32>, 8>(
        vec3<f32>(0.03137255, 0.05882353, 0.09803922),
        vec3<f32>(0.05098039, 0.15294118, 0.24705882),
        vec3<f32>(0.07058824, 0.30196080, 0.41176471),
        vec3<f32>(0.10588235, 0.51764706, 0.43921569),
        vec3<f32>(0.27843138, 0.71372549, 0.38823529),
        vec3<f32>(0.82745098, 0.69411765, 0.26274510),
        vec3<f32>(0.96470588, 0.49019608, 0.20784314),
        vec3<f32>(1.00000000, 0.88627451, 0.59215686),
    );
    let scaled = clamp(level, 0.0, 1.0) * 7.0;
    let lower = u32(floor(scaled));
    let upper = min(lower + 1u, 7u);
    let blend = scaled - f32(lower);
    return palette[lower] + (palette[upper] - palette[lower]) * blend;
}

fn history_sample(sample_index: u32) -> f32 {
    let word = history[sample_index / 4u];
    let shift = (sample_index % 4u) * 8u;
    let value = (word >> shift) & 0xffu;
    return f32(value) / 255.0;
}

@fragment
fn fragment_main(input: VertexOut) -> @location(0) vec4<f32> {
    if params.band_count == 0u || params.row_count == 0u || params.max_history == 0u {
        return vec4<f32>(palette_color(0.0), 1.0);
    }

    let max_history = max(params.max_history, 1u);
    let active_height = min(f32(params.row_count) / f32(max_history), 1.0);
    let active_top = 1.0 - active_height;
    if input.local.y < active_top {
        return vec4<f32>(palette_color(0.0), 1.0);
    }

    // Rows are stored oldest-to-newest. The newest row is anchored at the
    // bottom, while vertical sampling remains nearest-row for a sharp scanline.
    let history_y = clamp((input.local.y - active_top) / active_height, 0.0, 1.0);
    let last_row = max(params.row_count, 1u) - 1u;
    let row_index = min(u32(floor(history_y * f32(params.row_count))), last_row);

    // Analyzer bands are logarithmically spaced; interpolate between adjacent
    // bands in that already-produced frequency layout.
    let last_band = max(params.band_count, 1u) - 1u;
    let band_position = clamp(input.local.x * f32(last_band), 0.0, f32(last_band));
    let lower_band = min(u32(floor(band_position)), last_band);
    let upper_band = min(lower_band + 1u, last_band);
    let band_blend = band_position - f32(lower_band);
    let row_offset = row_index * params.band_count;
    let lower = history_sample(row_offset + lower_band);
    let upper = history_sample(row_offset + upper_band);
    let level = lower + (upper - lower) * band_blend;
    return vec4<f32>(palette_color(level), 1.0);
}
"#;

pub(crate) fn clamp_height(height: f32) -> f32 {
    if height.is_finite() {
        height.clamp(MIN_HEIGHT, MAX_HEIGHT)
    } else {
        HEIGHT
    }
}

fn waterfall_uniform_bytes(frame: &LiveSpectrogramFrame) -> [u8; 16] {
    let values = [
        LIVE_SPECTROGRAM_BAND_COUNT as u32,
        frame.row_count as u32,
        LIVE_SPECTROGRAM_MAX_HISTORY as u32,
        0,
    ];
    let mut bytes = [0_u8; 16];
    for (index, value) in values.into_iter().enumerate() {
        let start = index * std::mem::size_of::<u32>();
        bytes[start..start + std::mem::size_of::<u32>()].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn waterfall_shader_descriptor(frame: &LiveSpectrogramFrame) -> Arc<GpuShaderSurfaceDescriptor> {
    let uniform_bytes = waterfall_uniform_bytes(frame);
    Arc::new(GpuShaderSurfaceDescriptor::from_parts(
        GpuShaderSurfaceDescriptorParts {
            shader_key: String::from(WATERFALL_SHADER_KEY),
            wgsl_source: Some(Arc::<str>::from(WATERFALL_SHADER_WGSL)),
            entry_point: String::from("vertex_main"),
            fragment_entry_point: Some(String::from("fragment_main")),
            uniform_bytes: Arc::<[u8]>::from(uniform_bytes.as_slice()),
            storage_bytes: Arc::clone(frame.packed_values()),
            vertex_count: 6,
        },
    ))
}

#[derive(Clone, Debug)]
struct SpectrogramWidget {
    common: WidgetCommon,
    frame: Arc<LiveSpectrogramFrame>,
    mode: crate::LiveSpectrogramMode,
}

impl SpectrogramWidget {
    fn new(frame: Arc<LiveSpectrogramFrame>, mode: crate::LiveSpectrogramMode) -> Self {
        let mut common = WidgetCommon::fixed(0, 1.0, 1.0).without_default_chrome();
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

    fn x_for_frequency(&self, plot: Rect, frequency: f32) -> f32 {
        let (minimum, maximum) = live_display_frequency_bounds(self.frame.sample_rate);
        let ratio = (maximum / minimum.max(f32::MIN_POSITIVE)).max(1.0);
        let position = if ratio > 1.0 {
            (frequency.clamp(minimum, maximum) / minimum).ln() / ratio.ln()
        } else {
            0.0
        };
        plot.min.x + plot.width() * position.clamp(0.0, 1.0)
    }

    fn y_for_decibels(plot: Rect, decibels: f32) -> f32 {
        let position = (LIVE_SPECTRUM_DISPLAY_CEILING_DB - decibels)
            / (LIVE_SPECTRUM_DISPLAY_CEILING_DB - LIVE_SPECTRUM_DISPLAY_FLOOR_DB);
        plot.min.y + plot.height() * position.clamp(0.0, 1.0)
    }

    fn append_grid(&self, primitives: &mut Vec<PaintPrimitive>, plot: Rect, theme: &ThemeTokens) {
        let grid_color = theme.border.with_alpha(GRID_LINE_ALPHA);
        let label_color = theme.text_muted.with_alpha(GRID_LABEL_ALPHA);

        for &(decibels, label) in &DECIBEL_GRID {
            let y = Self::y_for_decibels(plot, decibels);
            primitives.push(PaintPrimitive::StrokePolyline(PaintStrokePolyline {
                widget_id: self.common.id,
                points: [Point::new(plot.min.x, y), Point::new(plot.max.x, y)].into(),
                color: grid_color,
                width: 1.0,
            }));
            let label_rect = Rect::from_min_max(
                Point::new(plot.min.x + 4.0, (y - 8.0).max(plot.min.y)),
                Point::new(plot.min.x + 42.0, (y + 8.0).min(plot.max.y)),
            );
            if label_rect.has_finite_positive_area() {
                push_text_run_with_metrics(
                    primitives,
                    self.common.id,
                    label,
                    label_rect,
                    label_color,
                    PaintTextAlign::Left,
                    PaintTextMetrics::new(GRID_LABEL_FONT_SIZE, Some(10.0)),
                );
            }
        }

        let mut previous_x: Option<f32> = None;
        for &(frequency, label) in &FREQUENCY_GRID {
            let x = self.x_for_frequency(plot, frequency);
            if previous_x.is_some_and(|previous| (x - previous).abs() < 2.0) {
                continue;
            }
            previous_x = Some(x);
            primitives.push(PaintPrimitive::StrokePolyline(PaintStrokePolyline {
                widget_id: self.common.id,
                points: [Point::new(x, plot.min.y), Point::new(x, plot.max.y)].into(),
                color: grid_color,
                width: 1.0,
            }));
            let label_rect = Rect::from_min_max(
                Point::new(
                    (x - 24.0).max(plot.min.x),
                    (plot.max.y - 16.0).max(plot.min.y),
                ),
                Point::new((x + 24.0).min(plot.max.x), plot.max.y),
            );
            if label_rect.has_finite_positive_area() {
                push_text_run_with_metrics(
                    primitives,
                    self.common.id,
                    label,
                    label_rect,
                    label_color,
                    PaintTextAlign::Center,
                    PaintTextMetrics::new(GRID_LABEL_FONT_SIZE, Some(10.0)),
                );
            }
        }
    }

    fn append_waterfall(&self, primitives: &mut Vec<PaintPrimitive>, plot: Rect) {
        if !plot.has_finite_positive_area() || !self.frame.is_valid() {
            return;
        }
        primitives.push(PaintPrimitive::GpuSurface(PaintGpuSurface {
            widget_id: self.common.id,
            key: WATERFALL_SURFACE_KEY,
            revision: self.frame.gpu_revision(),
            rect: plot,
            content: GpuSurfaceContent::CustomShader {
                descriptor: waterfall_shader_descriptor(&self.frame),
            },
            capabilities: GpuSurfaceCapabilities::default(),
            overlays: Vec::new(),
        }));
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
        let points = (0..LIVE_SPECTROGRAM_BAND_COUNT)
            .map(|band| {
                let level = self.frame.spectrum_value(band) as f32 / u8::MAX as f32;
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
                crate::LiveSpectrogramMode::Waterfall => self.append_waterfall(primitives, plot),
                crate::LiveSpectrogramMode::Spectrum => {
                    self.append_spectrum(primitives, plot, theme)
                }
            }
            // Both modes consume the same frame, already normalized to the
            // display-only -90..0 dB range with the signed +4.5 dB/octave tilt.
            self.append_grid(primitives, plot, theme);
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
    height: f32,
) -> ui::View<Message> {
    let height = clamp_height(height);
    ui::custom_widget(SpectrogramWidget::new(frame, mode), |_| None)
        .height(height)
        .fill_width()
}

#[cfg(test)]
mod tests {
    use super::{SPECTRUM_PLOT_BACKGROUND, SpectrogramWidget};
    use crate::LiveSpectrogramMode;
    use crate::transport::{
        LIVE_SPECTROGRAM_BAND_COUNT, LIVE_SPECTROGRAM_MAX_HISTORY, LiveSpectrogramFrame,
    };
    use radiant::{
        gui::types::{Point, Rect, Vector2},
        layout::LayoutOutput,
        runtime::{GpuSurfaceContent, PaintPrimitive},
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
        let mut spectrum_values = vec![0_u8; LIVE_SPECTROGRAM_BAND_COUNT];
        spectrum_values[LIVE_SPECTROGRAM_BAND_COUNT - 1] = u8::MAX;
        Arc::new(
            LiveSpectrogramFrame::from_values(
                4,
                2,
                1,
                48_000,
                row_count,
                Arc::from(values.into_boxed_slice()),
                Arc::from(spectrum_values.into_boxed_slice()),
            )
            .expect("valid live spectrogram test frame"),
        )
    }

    #[test]
    fn live_frame_paints_frequency_left_to_right_and_newest_at_bottom() {
        let widget = SpectrogramWidget::new(test_frame(), LiveSpectrogramMode::Waterfall);
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let primitives =
            widget.paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());
        let surfaces = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::GpuSurface(surface) => Some(surface),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(surfaces.len(), 1);
        let surface = surfaces[0];
        assert_eq!(surface.rect, SpectrogramWidget::plot_rect(bounds));
        let GpuSurfaceContent::CustomShader { descriptor } = &surface.content else {
            panic!("waterfall should use a custom GPU shader");
        };
        assert_eq!(descriptor.shader_key, "cadence/live-spectrogram-waterfall");
        assert_eq!(descriptor.entry_point, "vertex_main");
        assert_eq!(
            descriptor.fragment_entry_point.as_deref(),
            Some("fragment_main")
        );
        assert_eq!(descriptor.vertex_count, 6);
        assert_eq!(descriptor.uniform_bytes.len(), 16);
        assert_eq!(
            descriptor.storage_bytes.len(),
            (2 * LIVE_SPECTROGRAM_BAND_COUNT).div_ceil(4) * 4
        );
        let first_word = u32::from_le_bytes(
            descriptor.storage_bytes[0..4]
                .try_into()
                .expect("packed first word"),
        );
        let last_index = 2 * LIVE_SPECTROGRAM_BAND_COUNT - 1;
        let last_word_start = (last_index / 4) * 4;
        let last_word = u32::from_le_bytes(
            descriptor.storage_bytes[last_word_start..last_word_start + 4]
                .try_into()
                .expect("packed last word"),
        );
        assert_eq!(first_word & 0xff, u32::from(u8::MAX));
        assert_eq!(
            (last_word >> ((last_index % 4) * 8)) & 0xff,
            u32::from(u8::MAX)
        );
        assert!(descriptor.wgsl_source.as_deref().is_some_and(
            |source| source.contains("row_index")
                && source.contains("active_top")
                && source.contains("band_position")
                && source.contains("oldest-to-newest")
                && source.contains("newest row is anchored at the")
        ));
        assert!(
            !primitives
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::FillRectBatch(_)))
        );
    }

    #[test]
    fn live_frame_paint_is_batched_and_bounded() {
        let mut values = vec![0_u8; LIVE_SPECTROGRAM_MAX_HISTORY * LIVE_SPECTROGRAM_BAND_COUNT];
        let last = values.len() - 1;
        values[last] = u8::MAX;
        let expected_storage_len = values.len().div_ceil(4) * 4;
        let frame = Arc::new(
            LiveSpectrogramFrame::from_values(
                1,
                1,
                1,
                48_000,
                LIVE_SPECTROGRAM_MAX_HISTORY,
                Arc::from(values.into_boxed_slice()),
                Arc::from(vec![0_u8; LIVE_SPECTROGRAM_BAND_COUNT].into_boxed_slice()),
            )
            .expect("valid live spectrogram test frame"),
        );
        let widget = SpectrogramWidget::new(frame, LiveSpectrogramMode::Waterfall);
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let primitives =
            widget.paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());
        let surfaces = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::GpuSurface(surface) => Some(surface),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(surfaces.len(), 1);
        assert_eq!(surfaces[0].rect, SpectrogramWidget::plot_rect(bounds));
        let GpuSurfaceContent::CustomShader { descriptor } = &surfaces[0].content else {
            panic!("waterfall should use a custom GPU shader");
        };
        assert_eq!(descriptor.storage_bytes.len(), expected_storage_len);
        assert!(
            !primitives
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::FillRectBatch(_)))
        );
        assert!(primitives.len() <= 32);
    }

    #[test]
    fn mode_selects_waterfall_or_spectrum_paint_path() {
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let waterfall = SpectrogramWidget::new(test_frame(), LiveSpectrogramMode::Waterfall)
            .paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());
        let spectrum = SpectrogramWidget::new(test_frame(), LiveSpectrogramMode::Spectrum)
            .paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());

        assert!(waterfall.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::GpuSurface(surface) if matches!(&surface.content, GpuSurfaceContent::CustomShader { .. }))
        }));
        assert!(
            !waterfall
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::FillRectBatch(_)))
        );
        assert!(!waterfall.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::StrokePolyline(line) if line.points.len() == LIVE_SPECTROGRAM_BAND_COUNT)
        }));
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
                PaintPrimitive::StrokePolyline(line)
                    if line.points.len() == LIVE_SPECTROGRAM_BAND_COUNT =>
                {
                    Some(line)
                }
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
        assert!(primitives.len() <= 24);
    }

    #[test]
    fn display_grid_labels_expose_frequency_and_db_conventions() {
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let primitives = SpectrogramWidget::new(test_frame(), LiveSpectrogramMode::Waterfall)
            .paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());
        let labels = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();

        for label in ["20 Hz", "1 kHz", "20 kHz", "0 dB", "-90"] {
            assert!(labels.contains(&label), "missing grid label {label:?}");
        }
    }

    #[test]
    fn frequency_grid_is_logarithmic_and_clamps_to_frame_nyquist() {
        let frame = test_frame();
        let widget = SpectrogramWidget::new(frame, LiveSpectrogramMode::Spectrum);
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, 240.0));
        let plot = SpectrogramWidget::plot_rect(bounds);

        assert_eq!(widget.x_for_frequency(plot, 20.0), plot.min.x);
        assert_eq!(widget.x_for_frequency(plot, 20_000.0), plot.max.x);
        let reference_position = (1_000.0_f32 / 20.0).ln() / (20_000.0_f32 / 20.0).ln();
        let expected_reference_x = plot.min.x + plot.width() * reference_position;
        assert!((widget.x_for_frequency(plot, 1_000.0) - expected_reference_x).abs() < 0.001);

        let mut low_rate_frame = (*test_frame()).clone();
        low_rate_frame.sample_rate = 16_000;
        let low_rate_frame = Arc::new(low_rate_frame);
        let low_rate_widget = SpectrogramWidget::new(low_rate_frame, LiveSpectrogramMode::Spectrum);
        assert_eq!(low_rate_widget.x_for_frequency(plot, 20_000.0), plot.max.x);
    }

    #[test]
    fn height_clamping_keeps_the_plot_in_a_sensible_range() {
        assert_eq!(super::clamp_height(-1.0), super::MIN_HEIGHT);
        assert_eq!(
            super::clamp_height(super::MAX_HEIGHT + 1.0),
            super::MAX_HEIGHT
        );
        assert_eq!(super::clamp_height(f32::NAN), super::HEIGHT);
    }
}

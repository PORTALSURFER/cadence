//! Passive live-spectrogram heatmap for the native Review/Audition surface.
//!
//! The transport owns capture and analysis. This module only paints the latest
//! immutable, quantized frame: frequency increases from left to right, while
//! the oldest retained row is above the newest row at the bottom edge.

use crate::transport::{
    LIVE_SPECTROGRAM_BAND_COUNT, LIVE_SPECTRUM_DISPLAY_CEILING_DB, LIVE_SPECTRUM_DISPLAY_FLOOR_DB,
    LIVE_SPECTRUM_POINT_COUNT, LiveSpectrogramFrame, live_display_frequency_bounds,
    live_spectrum_point_frequency,
};
use radiant::{
    gui::types::{Point, Rect, Rgba8},
    layout::LayoutOutput,
    prelude as ui,
    runtime::{
        GpuShaderSurfaceDescriptor, GpuShaderSurfaceDescriptorParts, GpuSurfaceCapabilities,
        GpuSurfaceContent, PaintBrush, PaintFillPath, PaintFillPolygon, PaintFillRect,
        PaintFillRectBatch, PaintGpuSurface, PaintLinearGradient, PaintPath, PaintPathCommand,
        PaintPrimitive, PaintRectList, PaintStrokePolyline, PaintStrokeRect, PaintTextAlign,
        PaintTextMetrics, push_text_run_with_metrics,
    },
    theme::ThemeTokens,
    widgets::{FocusBehavior, PaintBounds, Widget, WidgetCommon, WidgetInput, WidgetOutput},
};
use std::sync::{Arc, OnceLock};

pub const HEIGHT: f32 = 78.0;
pub const MIN_HEIGHT: f32 = 72.0;
pub const MAX_HEIGHT: f32 = 240.0;
pub const RESIZE_HANDLE_HEIGHT: f32 = 8.0;
pub(crate) const DEFAULT_HISTORY_SCALE: f32 = 1.0;
pub(crate) const MIN_HISTORY_SCALE: f32 = 1.0;
pub(crate) const MAX_HISTORY_SCALE: f32 = 4.0;

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
pub const LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID: u64 = 0xCAD3_2201;
const WATERFALL_SHADER_KEY: &str = "cadence/live-spectrogram-waterfall";
const OVERLAY_COLOR_LEVELS: usize = 24;
const SPECTRUM_RIBBON_WIDTH: f32 = 1.5;
const SPECTRUM_AREA_ALPHA: u8 = 48;

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
    row_step: f32,
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
    if params.band_count == 0u || params.row_count == 0u || params.row_step <= 0.0 {
        return vec4<f32>(palette_color(0.0), 1.0);
    }

    // The row step is normalized by the logical plot height on the CPU. It
    // therefore remains stable when the renderer targets a Retina surface.
    // Rows are stored oldest-to-newest and age zero is anchored at the bottom.
    let row_age_position = max((1.0 - input.local.y) / params.row_step, 0.0);
    let row_age = u32(floor(row_age_position));
    if row_age >= params.row_count {
        return vec4<f32>(palette_color(0.0), 1.0);
    }
    let row_index = params.row_count - 1u - row_age;

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

pub(crate) fn clamp_history_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(MIN_HISTORY_SCALE, MAX_HISTORY_SCALE)
    } else {
        DEFAULT_HISTORY_SCALE
    }
}

pub(crate) fn history_scale_from_normalized(normalized: f32) -> f32 {
    let normalized = if normalized.is_finite() {
        normalized.clamp(0.0, 1.0)
    } else {
        0.0
    };
    clamp_history_scale(MIN_HISTORY_SCALE + (MAX_HISTORY_SCALE - MIN_HISTORY_SCALE) * normalized)
}

pub(crate) fn history_scale_to_normalized(scale: f32) -> f32 {
    (clamp_history_scale(scale) - MIN_HISTORY_SCALE) / (MAX_HISTORY_SCALE - MIN_HISTORY_SCALE)
}

fn waterfall_row_y_interval(
    plot: Rect,
    row_count: usize,
    row: usize,
    history_scale: f32,
) -> Option<(f32, f32)> {
    if row_count == 0 || row >= row_count || !plot.has_finite_positive_area() {
        return None;
    }
    let history_scale = clamp_history_scale(history_scale);
    let age = row_count - 1 - row;
    let bottom = plot.max.y - age as f32 * history_scale;
    let top = bottom - history_scale;
    Some((top, bottom))
}

fn waterfall_row_rect(
    plot: Rect,
    row_count: usize,
    row: usize,
    history_scale: f32,
) -> Option<Rect> {
    let (top, bottom) = waterfall_row_y_interval(plot, row_count, row, history_scale)?;
    let rect = Rect::from_min_max(Point::new(plot.min.x, top), Point::new(plot.max.x, bottom))
        .intersection(plot)?;
    rect.has_finite_positive_area().then_some(rect)
}

fn visible_waterfall_rows(plot: Rect, row_count: usize, history_scale: f32) -> Vec<(usize, Rect)> {
    (0..row_count)
        .filter_map(|row| {
            waterfall_row_rect(plot, row_count, row, history_scale).map(|rect| (row, rect))
        })
        .collect()
}

fn waterfall_uniform_bytes(
    frame: &LiveSpectrogramFrame,
    plot_height: f32,
    history_scale: f32,
) -> [u8; 16] {
    let row_step = clamp_history_scale(history_scale) / plot_height.max(f32::MIN_POSITIVE);
    let values = [
        LIVE_SPECTROGRAM_BAND_COUNT as u32,
        frame.row_count as u32,
        0_u32,
        0_u32,
    ];
    let mut bytes = [0_u8; 16];
    for (index, value) in values.into_iter().enumerate() {
        let start = index * std::mem::size_of::<u32>();
        bytes[start..start + std::mem::size_of::<u32>()].copy_from_slice(&value.to_le_bytes());
    }
    bytes[8..12].copy_from_slice(&row_step.to_le_bytes());
    bytes
}

fn waterfall_shader_descriptor(
    frame: &LiveSpectrogramFrame,
    plot: Rect,
    history_scale: f32,
) -> Arc<GpuShaderSurfaceDescriptor> {
    static SHADER_SOURCE: OnceLock<Arc<str>> = OnceLock::new();
    let uniform_bytes = waterfall_uniform_bytes(frame, plot.height(), history_scale);
    Arc::new(GpuShaderSurfaceDescriptor::from_parts(
        GpuShaderSurfaceDescriptorParts {
            shader_key: String::from(WATERFALL_SHADER_KEY),
            wgsl_source: Some(Arc::clone(
                SHADER_SOURCE.get_or_init(|| Arc::<str>::from(WATERFALL_SHADER_WGSL)),
            )),
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
    frame: Option<Arc<LiveSpectrogramFrame>>,
    display_sample_rate: u32,
    mode: crate::LiveSpectrogramMode,
    history_scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct SpectrumPaintGeometry {
    ribbon_points: Arc<[Point]>,
    area_path: PaintPath,
}

impl SpectrogramWidget {
    #[cfg(test)]
    fn new(frame: Arc<LiveSpectrogramFrame>, mode: crate::LiveSpectrogramMode) -> Self {
        Self::new_with_id(0, Some(frame), 48_000, mode)
    }

    #[cfg(test)]
    fn new_with_scale(
        frame: Arc<LiveSpectrogramFrame>,
        mode: crate::LiveSpectrogramMode,
        history_scale: f32,
    ) -> Self {
        Self::new_with_history_scale_with_id(0, Some(frame), 48_000, mode, history_scale)
    }

    #[cfg(test)]
    fn empty(display_sample_rate: u32, mode: crate::LiveSpectrogramMode) -> Self {
        Self::new_with_id(0, None, display_sample_rate, mode)
    }

    #[cfg(test)]
    fn new_with_id(
        id: u64,
        frame: Option<Arc<LiveSpectrogramFrame>>,
        display_sample_rate: u32,
        mode: crate::LiveSpectrogramMode,
    ) -> Self {
        Self::new_with_history_scale_with_id(
            id,
            frame,
            display_sample_rate,
            mode,
            DEFAULT_HISTORY_SCALE,
        )
    }

    fn new_with_history_scale_with_id(
        id: u64,
        frame: Option<Arc<LiveSpectrogramFrame>>,
        display_sample_rate: u32,
        mode: crate::LiveSpectrogramMode,
        history_scale: f32,
    ) -> Self {
        let mut common = WidgetCommon::fixed(id, 1.0, 1.0).without_default_chrome();
        common.focus = FocusBehavior::None;
        common.paint.bounds = PaintBounds::ClipToRect;
        common.paint.paints_focus = false;
        common.paint.paints_state_layers = false;
        Self {
            common,
            frame,
            display_sample_rate,
            mode,
            history_scale: clamp_history_scale(history_scale),
        }
    }

    fn plot_rect(bounds: Rect) -> Rect {
        bounds.inset(1.0, 1.0, 1.0, 1.0)
    }

    fn sample_rate(&self) -> u32 {
        self.frame
            .as_ref()
            .map_or(self.display_sample_rate, |frame| frame.sample_rate)
    }

    fn x_for_frequency(&self, plot: Rect, frequency: f32) -> f32 {
        let (minimum, maximum) = live_display_frequency_bounds(self.sample_rate());
        let ratio = (maximum / minimum.max(f32::MIN_POSITIVE)).max(1.0);
        let position = if ratio > 1.0 {
            (frequency.clamp(minimum, maximum) / minimum).ln() / ratio.ln()
        } else {
            0.0
        };
        plot.min.x + plot.width() * position.clamp(0.0, 1.0)
    }

    fn frequency_grid(&self) -> Vec<(f32, String)> {
        let (minimum, maximum) = live_display_frequency_bounds(self.sample_rate());
        let mut entries = Vec::with_capacity(FREQUENCY_GRID.len() + 1);
        entries.push((minimum, format_frequency_label(minimum)));
        for &(frequency, label) in &FREQUENCY_GRID {
            if frequency > minimum && frequency < maximum {
                entries.push((frequency, String::from(label)));
            }
        }
        if maximum > minimum {
            entries.push((maximum, format_frequency_label(maximum)));
        }
        entries
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
        for (frequency, label) in self.frequency_grid() {
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
                    &label,
                    label_rect,
                    label_color,
                    PaintTextAlign::Center,
                    PaintTextMetrics::new(GRID_LABEL_FONT_SIZE, Some(10.0)),
                );
            }
        }
    }

    fn append_waterfall(&self, primitives: &mut Vec<PaintPrimitive>, plot: Rect) {
        let Some(frame) = self.frame.as_ref() else {
            return;
        };
        if !plot.has_finite_positive_area() || !frame.is_valid() {
            return;
        }
        let history_scale = self.history_scale;
        primitives.push(PaintPrimitive::GpuSurface(PaintGpuSurface {
            widget_id: self.common.id,
            key: WATERFALL_SURFACE_KEY,
            revision: frame.gpu_revision() ^ u64::from(history_scale.to_bits()),
            rect: plot,
            content: GpuSurfaceContent::CustomShader {
                descriptor: waterfall_shader_descriptor(frame, plot, history_scale),
            },
            capabilities: GpuSurfaceCapabilities::default(),
            overlays: Vec::new(),
        }));
    }

    fn spectrum_geometry(&self, plot: Rect) -> Option<SpectrumPaintGeometry> {
        let frame = self.frame.as_ref()?;
        if !plot.has_finite_positive_area() || !frame.is_valid() {
            return None;
        }
        let last_point = LIVE_SPECTRUM_POINT_COUNT - 1;
        let mut centerline = Vec::with_capacity(LIVE_SPECTRUM_POINT_COUNT);
        let mut upper_offsets = Vec::with_capacity(LIVE_SPECTRUM_POINT_COUNT);
        let mut lower_offsets = Vec::with_capacity(LIVE_SPECTRUM_POINT_COUNT);
        let half_width = SPECTRUM_RIBBON_WIDTH * 0.5;
        for point in 0..LIVE_SPECTRUM_POINT_COUNT {
            let frequency = live_spectrum_point_frequency(self.sample_rate(), point);
            let x = if point == 0 {
                plot.min.x
            } else if point == last_point {
                plot.max.x
            } else {
                self.x_for_frequency(plot, frequency)
            };
            let level = frame.spectrum_value(point) as f32 / u8::MAX as f32;
            let y = (plot.max.y - plot.height() * level).clamp(plot.min.y, plot.max.y);
            let center = Point::new(x.clamp(plot.min.x, plot.max.x), y);
            centerline.push(center);
            upper_offsets.push(Point::new(
                center.x,
                (center.y - half_width).clamp(plot.min.y, plot.max.y),
            ));
            lower_offsets.push(Point::new(
                center.x,
                (center.y + half_width).clamp(plot.min.y, plot.max.y),
            ));
        }

        let mut ribbon_points = Vec::with_capacity(LIVE_SPECTRUM_POINT_COUNT * 2);
        ribbon_points.extend(upper_offsets);
        ribbon_points.extend(lower_offsets.into_iter().rev());

        let mut area_commands = Vec::with_capacity(LIVE_SPECTRUM_POINT_COUNT + 3);
        area_commands.push(PaintPathCommand::MoveTo(centerline[0]));
        for point in centerline.iter().skip(1).copied() {
            area_commands.push(PaintPathCommand::LineTo(point));
        }
        area_commands.push(PaintPathCommand::LineTo(Point::new(plot.max.x, plot.max.y)));
        area_commands.push(PaintPathCommand::LineTo(Point::new(plot.min.x, plot.max.y)));
        area_commands.push(PaintPathCommand::Close);

        Some(SpectrumPaintGeometry {
            ribbon_points: Arc::from(ribbon_points.into_boxed_slice()),
            area_path: PaintPath::from(area_commands),
        })
    }

    fn overlay_color(level: usize) -> Rgba8 {
        let level = level.min(OVERLAY_COLOR_LEVELS - 1);
        let normalized = level as f32 / (OVERLAY_COLOR_LEVELS - 1) as f32;
        let scaled = normalized * (PALETTE.len() - 1) as f32;
        let lower = scaled.floor() as usize;
        let upper = (lower + 1).min(PALETTE.len() - 1);
        PALETTE[lower].blend_toward(PALETTE[upper], scaled - lower as f32)
    }

    fn overlay_grid(&self, primitives: &mut Vec<PaintPrimitive>, plot: Rect, theme: &ThemeTokens) {
        let grid_color = theme.border.with_alpha(GRID_LINE_ALPHA);
        let label_color = theme.text_muted.with_alpha(GRID_LABEL_ALPHA);

        for &(decibels, label) in &DECIBEL_GRID {
            let y = Self::y_for_decibels(plot, decibels);
            let line_bottom = (y + 1.0).min(plot.max.y);
            if line_bottom > y {
                primitives.push(PaintPrimitive::StrokeRect(PaintStrokeRect {
                    widget_id: self.common.id,
                    rect: Rect::from_min_max(
                        Point::new(plot.min.x, y),
                        Point::new(plot.max.x, line_bottom),
                    ),
                    color: grid_color,
                    width: 1.0,
                }));
            }
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
        for (frequency, label) in self.frequency_grid() {
            let x = self.x_for_frequency(plot, frequency);
            if previous_x.is_some_and(|previous| (x - previous).abs() < 2.0) {
                continue;
            }
            previous_x = Some(x);
            let line_right = (x + 1.0).min(plot.max.x);
            if line_right > x {
                primitives.push(PaintPrimitive::StrokeRect(PaintStrokeRect {
                    widget_id: self.common.id,
                    rect: Rect::from_min_max(
                        Point::new(x, plot.min.y),
                        Point::new(line_right, plot.max.y),
                    ),
                    color: grid_color,
                    width: 1.0,
                }));
            }
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
                    &label,
                    label_rect,
                    label_color,
                    PaintTextAlign::Center,
                    PaintTextMetrics::new(GRID_LABEL_FONT_SIZE, Some(10.0)),
                );
            }
        }
    }

    fn append_overlay_waterfall(&self, primitives: &mut Vec<PaintPrimitive>, plot: Rect) {
        let Some(frame) = self.frame.as_ref() else {
            return;
        };
        if !frame.is_valid() {
            return;
        }

        let visible_rows = visible_waterfall_rows(plot, frame.row_count, self.history_scale);
        if visible_rows.is_empty() {
            return;
        }
        let mut rects_by_level: Vec<Vec<Rect>> =
            (0..OVERLAY_COLOR_LEVELS).map(|_| Vec::new()).collect();

        for (row, row_rect) in visible_rows {
            let start = row * LIVE_SPECTROGRAM_BAND_COUNT;
            let end = start + LIVE_SPECTROGRAM_BAND_COUNT;
            append_overlay_row_runs(
                &mut rects_by_level,
                plot,
                row_rect,
                &frame.values[start..end],
            );
        }

        for (level, rects) in rects_by_level.into_iter().enumerate() {
            if rects.is_empty() {
                continue;
            }
            primitives.push(PaintPrimitive::FillRectBatch(PaintFillRectBatch {
                widget_id: self.common.id,
                rects: PaintRectList::from(Arc::from(rects.into_boxed_slice())),
                color: Self::overlay_color(level),
            }));
        }
    }

    fn append_overlay_paint(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        bounds: Rect,
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
        if !plot.has_finite_positive_area() {
            return;
        }
        primitives.push(PaintPrimitive::FillRect(PaintFillRect {
            widget_id: self.common.id,
            rect: plot,
            color: match self.mode {
                crate::LiveSpectrogramMode::Waterfall => PALETTE[0],
                crate::LiveSpectrogramMode::Spectrum => SPECTRUM_PLOT_BACKGROUND,
            },
        }));
        let spectrum_geometry = match self.mode {
            crate::LiveSpectrogramMode::Waterfall => None,
            crate::LiveSpectrogramMode::Spectrum => self.spectrum_geometry(plot),
        };
        match self.mode {
            crate::LiveSpectrogramMode::Waterfall => {
                self.append_overlay_waterfall(primitives, plot)
            }
            crate::LiveSpectrogramMode::Spectrum => {
                if let Some(geometry) = spectrum_geometry.as_ref() {
                    append_spectrum_area(primitives, self.common.id, plot, geometry, theme);
                }
            }
        }
        self.overlay_grid(primitives, plot, theme);
        if let Some(geometry) = spectrum_geometry.as_ref() {
            append_spectrum_ribbon(primitives, self.common.id, geometry, theme);
        }
        primitives.push(PaintPrimitive::StrokeRect(PaintStrokeRect {
            widget_id: self.common.id,
            rect: plot,
            color: theme.border_emphasis,
            width: 1.0,
        }));
    }
}

fn append_overlay_row_runs(
    rects_by_level: &mut [Vec<Rect>],
    plot: Rect,
    row_rect: Rect,
    row: &[u8],
) {
    debug_assert_eq!(rects_by_level.len(), OVERLAY_COLOR_LEVELS);
    debug_assert_eq!(row.len(), LIVE_SPECTROGRAM_BAND_COUNT);

    let mut run_level = None;
    let mut run_start = 0usize;
    for (band, value) in row
        .iter()
        .copied()
        .enumerate()
        .chain(std::iter::once((LIVE_SPECTROGRAM_BAND_COUNT, 0)))
    {
        let level = (band < LIVE_SPECTROGRAM_BAND_COUNT).then(|| overlay_level(value));
        if level == run_level {
            continue;
        }
        if let Some(run_level) = run_level {
            let x0 =
                plot.min.x + plot.width() * run_start as f32 / LIVE_SPECTROGRAM_BAND_COUNT as f32;
            let x1 = plot.min.x + plot.width() * band as f32 / LIVE_SPECTROGRAM_BAND_COUNT as f32;
            if x1 > x0 && row_rect.has_finite_positive_area() {
                rects_by_level[run_level].push(Rect::from_min_max(
                    Point::new(x0, row_rect.min.y),
                    Point::new(x1, row_rect.max.y),
                ));
            }
        }
        run_level = level;
        run_start = band;
    }
}

fn append_spectrum_area(
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    plot: Rect,
    geometry: &SpectrumPaintGeometry,
    theme: &ThemeTokens,
) {
    let gradient = PaintLinearGradient::vertical(
        plot,
        theme.highlight_orange.with_alpha(SPECTRUM_AREA_ALPHA),
        theme.highlight_orange.with_alpha(0),
    );
    primitives.push(PaintPrimitive::FillPath(PaintFillPath::new(
        widget_id,
        geometry.area_path.clone(),
        PaintBrush::linear_gradient(gradient),
    )));
}

fn append_spectrum_ribbon(
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    geometry: &SpectrumPaintGeometry,
    theme: &ThemeTokens,
) {
    primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
        widget_id,
        points: Arc::clone(&geometry.ribbon_points),
        color: theme.highlight_orange,
    }));
}

fn format_frequency_label(frequency: f32) -> String {
    if frequency < 1_000.0 {
        format!("{frequency:.0} Hz")
    } else {
        let kilohertz = frequency / 1_000.0;
        if (kilohertz - kilohertz.round()).abs() < 0.05 {
            format!("{kilohertz:.0} kHz")
        } else {
            format!("{kilohertz:.1} kHz")
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
            let spectrum_geometry = match self.mode {
                crate::LiveSpectrogramMode::Waterfall => None,
                crate::LiveSpectrogramMode::Spectrum => self.spectrum_geometry(plot),
            };
            match self.mode {
                crate::LiveSpectrogramMode::Waterfall => self.append_waterfall(primitives, plot),
                crate::LiveSpectrogramMode::Spectrum => {
                    if let Some(geometry) = spectrum_geometry.as_ref() {
                        append_spectrum_area(primitives, self.common.id, plot, geometry, theme);
                    }
                }
            }
            // Both modes consume the same frame, already normalized to the
            // display-only -90..0 dB range with the signed +4.5 dB/octave tilt.
            self.append_grid(primitives, plot, theme);
            if let Some(geometry) = spectrum_geometry.as_ref() {
                append_spectrum_ribbon(primitives, self.common.id, geometry, theme);
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
        let same_frame = match (&self.frame, &previous.frame) {
            (Some(current), Some(previous)) => Arc::ptr_eq(current, previous),
            (None, None) => true,
            _ => false,
        };
        if same_frame && self.display_sample_rate == previous.display_sample_rate {
            self.common.state = previous.common.state;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverlayRectKey {
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
}

impl OverlayRectKey {
    fn from_rect(rect: Rect) -> Self {
        Self {
            min_x: rect.min.x.to_bits(),
            min_y: rect.min.y.to_bits(),
            max_x: rect.max.x.to_bits(),
            max_y: rect.max.y.to_bits(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SpectrogramOverlayThemeKey {
    bg_primary: Rgba8,
    surface_overlay: Rgba8,
    border: Rgba8,
    text_muted: Rgba8,
    border_emphasis: Rgba8,
    highlight_orange: Rgba8,
}

impl SpectrogramOverlayThemeKey {
    fn from_theme(theme: &ThemeTokens) -> Self {
        Self {
            bg_primary: theme.bg_primary,
            surface_overlay: theme.surface_overlay,
            border: theme.border,
            text_muted: theme.text_muted,
            border_emphasis: theme.border_emphasis,
            highlight_orange: theme.highlight_orange,
        }
    }
}

#[derive(Clone, Debug)]
struct SpectrogramOverlayPaintCacheKey {
    frame: Arc<LiveSpectrogramFrame>,
    mode: crate::LiveSpectrogramMode,
    history_scale_bits: Option<u32>,
    outer_bounds: OverlayRectKey,
    plot_bounds: OverlayRectKey,
    theme: SpectrogramOverlayThemeKey,
}

impl SpectrogramOverlayPaintCacheKey {
    fn new(
        frame: Arc<LiveSpectrogramFrame>,
        mode: crate::LiveSpectrogramMode,
        history_scale: f32,
        outer_bounds: Rect,
        plot_bounds: Rect,
        theme: &ThemeTokens,
    ) -> Self {
        Self {
            frame,
            mode,
            history_scale_bits: (mode == crate::LiveSpectrogramMode::Waterfall)
                .then_some(clamp_history_scale(history_scale).to_bits()),
            outer_bounds: OverlayRectKey::from_rect(outer_bounds),
            plot_bounds: OverlayRectKey::from_rect(plot_bounds),
            theme: SpectrogramOverlayThemeKey::from_theme(theme),
        }
    }

    fn matches(
        &self,
        frame: &Arc<LiveSpectrogramFrame>,
        mode: crate::LiveSpectrogramMode,
        history_scale: f32,
        outer_bounds: Rect,
        plot_bounds: Rect,
        theme: &ThemeTokens,
    ) -> bool {
        Arc::ptr_eq(&self.frame, frame)
            && self.mode == mode
            && self.history_scale_bits
                == (mode == crate::LiveSpectrogramMode::Waterfall)
                    .then_some(clamp_history_scale(history_scale).to_bits())
            && self.outer_bounds == OverlayRectKey::from_rect(outer_bounds)
            && self.plot_bounds == OverlayRectKey::from_rect(plot_bounds)
            && self.theme == SpectrogramOverlayThemeKey::from_theme(theme)
    }
}

#[derive(Clone, Debug)]
struct SpectrogramOverlayPaintCacheEntry {
    key: SpectrogramOverlayPaintCacheKey,
    primitives: Arc<[PaintPrimitive]>,
}

/// Bounded, presentation-local replay storage for one live spectrogram overlay.
///
/// The transient compositor clears its primitive list every frame, so cache hits
/// always clone the complete immutable sequence back into the current output.
#[derive(Clone, Debug, Default)]
pub(crate) struct SpectrogramOverlayPaintCache {
    entry: Option<SpectrogramOverlayPaintCacheEntry>,
    #[cfg(test)]
    rebuild_count: usize,
}

impl SpectrogramOverlayPaintCache {
    fn paint(
        &mut self,
        frame: Option<Arc<LiveSpectrogramFrame>>,
        mode: crate::LiveSpectrogramMode,
        history_scale: f32,
        bounds: Rect,
        primitives: &mut Vec<PaintPrimitive>,
        theme: &ThemeTokens,
    ) {
        let Some(frame) = frame else {
            self.entry = None;
            return;
        };
        if !bounds.has_finite_positive_area() {
            return;
        }

        let history_scale = clamp_history_scale(history_scale);
        let plot = SpectrogramWidget::plot_rect(bounds);
        if !plot.is_finite() {
            let display_sample_rate = frame.sample_rate;
            SpectrogramWidget::new_with_history_scale_with_id(
                LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
                Some(frame),
                display_sample_rate,
                mode,
                history_scale,
            )
            .append_overlay_paint(primitives, bounds, theme);
            return;
        }

        if let Some(entry) = self.entry.as_ref()
            && entry
                .key
                .matches(&frame, mode, history_scale, bounds, plot, theme)
        {
            primitives.extend(entry.primitives.iter().cloned());
            return;
        }

        let display_sample_rate = frame.sample_rate;
        let mut built = Vec::new();
        SpectrogramWidget::new_with_history_scale_with_id(
            LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
            Some(frame.clone()),
            display_sample_rate,
            mode,
            history_scale,
        )
        .append_overlay_paint(&mut built, bounds, theme);
        let entry = SpectrogramOverlayPaintCacheEntry {
            key: SpectrogramOverlayPaintCacheKey::new(
                frame,
                mode,
                history_scale,
                bounds,
                plot,
                theme,
            ),
            primitives: Arc::from(built.into_boxed_slice()),
        };
        #[cfg(test)]
        {
            self.rebuild_count += 1;
        }
        self.entry = Some(entry);
        primitives.extend(
            self.entry
                .as_ref()
                .expect("spectrogram overlay cache entry was just stored")
                .primitives
                .iter()
                .cloned(),
        );
    }

    #[cfg(test)]
    fn rebuild_count(&self) -> usize {
        self.rebuild_count
    }
}

fn overlay_level(value: u8) -> usize {
    usize::from(value) * (OVERLAY_COLOR_LEVELS - 1) / usize::from(u8::MAX)
}

pub fn view<Message: 'static>(
    frame: Option<Arc<LiveSpectrogramFrame>>,
    display_sample_rate: u32,
    mode: crate::LiveSpectrogramMode,
    height: f32,
    history_scale: f32,
) -> ui::View<Message> {
    let height = clamp_height(height);
    ui::custom_widget(
        SpectrogramWidget::new_with_history_scale_with_id(
            LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
            frame,
            display_sample_rate,
            mode,
            clamp_history_scale(history_scale),
        ),
        |_| None,
    )
    .height(height)
    .fill_width()
}

/// Paint the current live frame over the retained review surface using only
/// primitives that the native transient compositor can replay after the base
/// scene's GPU surfaces.
pub(crate) fn paint_overlay(
    cache: &mut SpectrogramOverlayPaintCache,
    frame: Option<Arc<LiveSpectrogramFrame>>,
    mode: crate::LiveSpectrogramMode,
    history_scale: f32,
    bounds: Rect,
    primitives: &mut Vec<PaintPrimitive>,
    theme: &ThemeTokens,
) {
    cache.paint(frame, mode, history_scale, bounds, primitives, theme);
}

#[cfg(test)]
mod tests {
    use super::{
        SPECTRUM_PLOT_BACKGROUND, SPECTRUM_RIBBON_WIDTH, SpectrogramOverlayPaintCache,
        SpectrogramWidget, paint_overlay,
    };
    use crate::LiveSpectrogramMode;
    use crate::transport::{
        LIVE_SPECTROGRAM_BAND_COUNT, LIVE_SPECTROGRAM_MAX_HISTORY, LIVE_SPECTRUM_POINT_COUNT,
        LiveSpectrogramFrame,
    };
    use radiant::{
        gui::types::{Point, Rect, Vector2},
        layout::LayoutOutput,
        runtime::{GpuSurfaceContent, PaintBrush, PaintPathCommand, PaintPrimitive},
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
        let mut spectrum_values = vec![0_u8; LIVE_SPECTRUM_POINT_COUNT];
        spectrum_values[LIVE_SPECTRUM_POINT_COUNT - 1] = u8::MAX;
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

    fn overlay_bounds() -> Rect {
        Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT))
    }

    fn cached_overlay(
        cache: &mut SpectrogramOverlayPaintCache,
        frame: Option<Arc<LiveSpectrogramFrame>>,
        mode: LiveSpectrogramMode,
        bounds: Rect,
        theme: &ThemeTokens,
    ) -> Vec<PaintPrimitive> {
        cached_overlay_with_scale(
            cache,
            frame,
            mode,
            super::DEFAULT_HISTORY_SCALE,
            bounds,
            theme,
        )
    }

    fn cached_overlay_with_scale(
        cache: &mut SpectrogramOverlayPaintCache,
        frame: Option<Arc<LiveSpectrogramFrame>>,
        mode: LiveSpectrogramMode,
        history_scale: f32,
        bounds: Rect,
        theme: &ThemeTokens,
    ) -> Vec<PaintPrimitive> {
        let mut primitives = Vec::new();
        paint_overlay(
            cache,
            frame,
            mode,
            history_scale,
            bounds,
            &mut primitives,
            theme,
        );
        primitives
    }

    #[test]
    fn empty_frame_keeps_the_spectrogram_shell_visible() {
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let primitives = SpectrogramWidget::empty(16_000, LiveSpectrogramMode::Waterfall)
            .paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());

        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill) if fill.rect == bounds)
        }));
        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill) if fill.rect == SpectrogramWidget::plot_rect(bounds))
        }));
        assert!(
            primitives
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::StrokePolyline(_)))
        );
        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::Text(text) if text.text == "8 kHz")
        }));
        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::StrokeRect(border) if border.rect == SpectrogramWidget::plot_rect(bounds))
        }));
        assert!(
            !primitives
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::GpuSurface(_)))
        );
        assert!(
            !primitives
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::FillRectBatch(_)))
        );
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
                && source.contains("row_step")
                && source.contains("row_age")
                && source.contains("band_position")
                && source.contains("oldest-to-newest")
                && source.contains("age zero is anchored at the bottom")
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
                Arc::from(vec![0_u8; LIVE_SPECTRUM_POINT_COUNT].into_boxed_slice()),
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
    fn overlay_paint_uses_replayable_batches_for_the_pixel_budget() {
        let row_count = LIVE_SPECTROGRAM_MAX_HISTORY;
        let mut values = vec![0_u8; row_count * LIVE_SPECTROGRAM_BAND_COUNT];
        for row in 0..row_count {
            let level = row % (super::OVERLAY_COLOR_LEVELS - 1) + 1;
            let value =
                (level * usize::from(u8::MAX)).div_ceil(super::OVERLAY_COLOR_LEVELS - 1) as u8;
            for band in 0..LIVE_SPECTROGRAM_BAND_COUNT {
                values[row * LIVE_SPECTROGRAM_BAND_COUNT + band] = value;
            }
        }
        let frame = Arc::new(
            LiveSpectrogramFrame::from_values(
                1,
                1,
                1,
                48_000,
                row_count,
                Arc::from(values.into_boxed_slice()),
                Arc::from(vec![0_u8; LIVE_SPECTRUM_POINT_COUNT].into_boxed_slice()),
            )
            .expect("valid full-history frame"),
        );
        let widget = SpectrogramWidget::new(frame, LiveSpectrogramMode::Waterfall);
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let mut primitives = Vec::new();
        widget.append_overlay_paint(&mut primitives, bounds, &ThemeTokens::default());

        assert!(
            !primitives
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::GpuSurface(_)))
        );
        let batched_rects = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillRectBatch(batch) => Some(batch.rects.len()),
                _ => None,
            })
            .sum::<usize>();
        let plot = SpectrogramWidget::plot_rect(bounds);
        assert_eq!(
            batched_rects,
            super::visible_waterfall_rows(plot, row_count, super::DEFAULT_HISTORY_SCALE).len()
        );
        assert!(
            primitives
                .iter()
                .filter(|primitive| matches!(primitive, PaintPrimitive::FillRectBatch(_)))
                .count()
                <= super::OVERLAY_COLOR_LEVELS
        );
    }

    #[test]
    fn overlay_cache_reuses_waterfall_batches_for_same_frame() {
        let frame = test_frame();
        let bounds = overlay_bounds();
        let theme = ThemeTokens::default();
        let mut cache = SpectrogramOverlayPaintCache::default();

        let first = cached_overlay(
            &mut cache,
            Some(frame.clone()),
            LiveSpectrogramMode::Waterfall,
            bounds,
            &theme,
        );
        let second = cached_overlay(
            &mut cache,
            Some(frame),
            LiveSpectrogramMode::Waterfall,
            bounds,
            &theme,
        );

        assert_eq!(cache.rebuild_count(), 1);
        assert_eq!(first, second);
        let fresh = {
            let mut primitives = Vec::new();
            SpectrogramWidget::new_with_id(
                super::LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
                Some(test_frame()),
                48_000,
                LiveSpectrogramMode::Waterfall,
            )
            .append_overlay_paint(&mut primitives, bounds, &theme);
            primitives
        };
        assert_eq!(first, fresh);

        let first_batch = first.iter().find_map(|primitive| match primitive {
            PaintPrimitive::FillRectBatch(batch) => Some(batch),
            _ => None,
        });
        let second_batch = second.iter().find_map(|primitive| match primitive {
            PaintPrimitive::FillRectBatch(batch) => Some(batch),
            _ => None,
        });
        let (Some(first_batch), Some(second_batch)) = (first_batch, second_batch) else {
            panic!("waterfall overlay should contain a rectangle batch");
        };
        assert!(Arc::ptr_eq(&first_batch.rects, &second_batch.rects));
    }

    #[test]
    fn waterfall_cache_reuses_same_scale_and_invalidates_on_scale_change() {
        let frame = test_frame();
        let bounds = overlay_bounds();
        let theme = ThemeTokens::default();
        let mut cache = SpectrogramOverlayPaintCache::default();

        cached_overlay_with_scale(
            &mut cache,
            Some(frame.clone()),
            LiveSpectrogramMode::Waterfall,
            1.0,
            bounds,
            &theme,
        );
        cached_overlay_with_scale(
            &mut cache,
            Some(frame.clone()),
            LiveSpectrogramMode::Waterfall,
            1.0,
            bounds,
            &theme,
        );
        assert_eq!(cache.rebuild_count(), 1);

        cached_overlay_with_scale(
            &mut cache,
            Some(frame.clone()),
            LiveSpectrogramMode::Waterfall,
            2.0,
            bounds,
            &theme,
        );
        assert_eq!(cache.rebuild_count(), 2);
        cached_overlay_with_scale(
            &mut cache,
            Some(frame),
            LiveSpectrogramMode::Waterfall,
            2.0,
            bounds,
            &theme,
        );
        assert_eq!(cache.rebuild_count(), 2);
    }

    #[test]
    fn spectrum_cache_identity_ignores_history_scale() {
        let frame = test_frame();
        let bounds = overlay_bounds();
        let theme = ThemeTokens::default();
        let mut cache = SpectrogramOverlayPaintCache::default();

        cached_overlay_with_scale(
            &mut cache,
            Some(frame.clone()),
            LiveSpectrogramMode::Spectrum,
            1.0,
            bounds,
            &theme,
        );
        cached_overlay_with_scale(
            &mut cache,
            Some(frame),
            LiveSpectrogramMode::Spectrum,
            4.0,
            bounds,
            &theme,
        );

        assert_eq!(cache.rebuild_count(), 1);
    }

    #[test]
    fn retained_uniform_encodes_normalized_row_step() {
        let bounds = overlay_bounds();
        let plot = SpectrogramWidget::plot_rect(bounds);
        let widget =
            SpectrogramWidget::new_with_scale(test_frame(), LiveSpectrogramMode::Waterfall, 2.5);
        let primitives =
            widget.paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());
        let surface = primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::GpuSurface(surface) => Some(surface),
                _ => None,
            })
            .expect("waterfall should retain a GPU surface");
        let GpuSurfaceContent::CustomShader { descriptor } = &surface.content else {
            panic!("waterfall should use a custom shader");
        };
        let row_step = f32::from_le_bytes(
            descriptor.uniform_bytes[8..12]
                .try_into()
                .expect("row step bytes"),
        );
        assert_eq!(row_step, 2.5 / plot.height());
        assert_eq!(
            u32::from_le_bytes(
                descriptor.uniform_bytes[4..8]
                    .try_into()
                    .expect("row count bytes")
            ),
            2
        );
    }

    #[test]
    fn retained_and_transient_waterfall_use_the_same_visible_row_intervals() {
        let bounds = overlay_bounds();
        let plot = SpectrogramWidget::plot_rect(bounds);
        let scale = 2.0;
        let widget =
            SpectrogramWidget::new_with_scale(test_frame(), LiveSpectrogramMode::Waterfall, scale);
        let mut transient = Vec::new();
        widget.append_overlay_waterfall(&mut transient, plot);
        let visible = super::visible_waterfall_rows(plot, 2, scale);
        let transient_rects = transient
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillRectBatch(batch) => Some(batch.rects.iter()),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();

        assert_eq!(visible.len(), 2);
        assert!(visible.iter().all(|(_, row)| {
            transient_rects
                .iter()
                .any(|rect| rect.min.y == row.min.y && rect.max.y == row.max.y)
        }));
        assert!(transient_rects.iter().all(|rect| {
            rect.min.y >= plot.min.y && rect.max.y <= plot.max.y && rect.height() <= scale
        }));
    }

    #[test]
    fn overlay_cache_reuses_spectrum_geometry_for_same_frame() {
        let frame = test_frame();
        let bounds = overlay_bounds();
        let theme = ThemeTokens::default();
        let mut cache = SpectrogramOverlayPaintCache::default();

        let first = cached_overlay(
            &mut cache,
            Some(frame.clone()),
            LiveSpectrogramMode::Spectrum,
            bounds,
            &theme,
        );
        let second = cached_overlay(
            &mut cache,
            Some(frame),
            LiveSpectrogramMode::Spectrum,
            bounds,
            &theme,
        );

        assert_eq!(cache.rebuild_count(), 1);
        assert_eq!(first, second);
        let fresh = {
            let mut primitives = Vec::new();
            SpectrogramWidget::new_with_id(
                super::LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
                Some(test_frame()),
                48_000,
                LiveSpectrogramMode::Spectrum,
            )
            .append_overlay_paint(&mut primitives, bounds, &theme);
            primitives
        };
        assert_eq!(first, fresh);

        let first_polygon = first.iter().find_map(|primitive| match primitive {
            PaintPrimitive::FillPolygon(polygon) => Some(polygon),
            _ => None,
        });
        let second_polygon = second.iter().find_map(|primitive| match primitive {
            PaintPrimitive::FillPolygon(polygon) => Some(polygon),
            _ => None,
        });
        let (Some(first_polygon), Some(second_polygon)) = (first_polygon, second_polygon) else {
            panic!("spectrum overlay should contain a polygon");
        };
        assert!(Arc::ptr_eq(&first_polygon.points, &second_polygon.points));

        let first_path = first.iter().find_map(|primitive| match primitive {
            PaintPrimitive::FillPath(path) => Some(path),
            _ => None,
        });
        let second_path = second.iter().find_map(|primitive| match primitive {
            PaintPrimitive::FillPath(path) => Some(path),
            _ => None,
        });
        let (Some(first_path), Some(second_path)) = (first_path, second_path) else {
            panic!("spectrum overlay should contain an area path");
        };
        assert_eq!(
            first_path.path.commands().as_ptr(),
            second_path.path.commands().as_ptr()
        );
    }

    #[test]
    fn overlay_cache_misses_for_distinct_equal_metadata_frames() {
        let frame = test_frame();
        let distinct_frame = Arc::new((*frame).clone());
        assert!(!Arc::ptr_eq(&frame, &distinct_frame));
        let theme = ThemeTokens::default();
        let mut cache = SpectrogramOverlayPaintCache::default();

        cached_overlay(
            &mut cache,
            Some(frame),
            LiveSpectrogramMode::Waterfall,
            overlay_bounds(),
            &theme,
        );
        cached_overlay(
            &mut cache,
            Some(distinct_frame),
            LiveSpectrogramMode::Waterfall,
            overlay_bounds(),
            &theme,
        );

        assert_eq!(cache.rebuild_count(), 2);
    }

    #[test]
    fn overlay_cache_misses_for_mode_bounds_and_style_changes() {
        let frame = test_frame();
        let bounds = overlay_bounds();
        let resized_bounds =
            Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::MIN_HEIGHT));
        let theme = ThemeTokens::default();
        let mut changed_theme = theme;
        changed_theme.highlight_orange = changed_theme.highlight_orange.with_alpha(254);
        let mut cache = SpectrogramOverlayPaintCache::default();

        cached_overlay(
            &mut cache,
            Some(frame.clone()),
            LiveSpectrogramMode::Waterfall,
            bounds,
            &theme,
        );
        cached_overlay(
            &mut cache,
            Some(frame.clone()),
            LiveSpectrogramMode::Spectrum,
            bounds,
            &theme,
        );
        cached_overlay(
            &mut cache,
            Some(frame.clone()),
            LiveSpectrogramMode::Spectrum,
            resized_bounds,
            &theme,
        );
        cached_overlay(
            &mut cache,
            Some(frame),
            LiveSpectrogramMode::Spectrum,
            resized_bounds,
            &changed_theme,
        );

        assert_eq!(cache.rebuild_count(), 4);
    }

    #[test]
    fn absent_frame_does_not_replay_stale_overlay_data() {
        let frame = test_frame();
        let bounds = overlay_bounds();
        let theme = ThemeTokens::default();
        let mut cache = SpectrogramOverlayPaintCache::default();

        let first = cached_overlay(
            &mut cache,
            Some(frame.clone()),
            LiveSpectrogramMode::Spectrum,
            bounds,
            &theme,
        );
        assert!(!first.is_empty());

        let absent = cached_overlay(
            &mut cache,
            None,
            LiveSpectrogramMode::Spectrum,
            bounds,
            &theme,
        );
        assert!(absent.is_empty());
        assert_eq!(cache.rebuild_count(), 1);

        cached_overlay(
            &mut cache,
            Some(frame),
            LiveSpectrogramMode::Spectrum,
            bounds,
            &theme,
        );
        assert_eq!(cache.rebuild_count(), 2);
    }

    #[test]
    fn overlay_cache_replaces_its_single_entry() {
        let first_frame = test_frame();
        let second_frame = Arc::new((*first_frame).clone());
        let bounds = overlay_bounds();
        let theme = ThemeTokens::default();
        let mut cache = SpectrogramOverlayPaintCache::default();

        cached_overlay(
            &mut cache,
            Some(first_frame.clone()),
            LiveSpectrogramMode::Spectrum,
            bounds,
            &theme,
        );
        cached_overlay(
            &mut cache,
            Some(second_frame),
            LiveSpectrogramMode::Spectrum,
            bounds,
            &theme,
        );
        cached_overlay(
            &mut cache,
            Some(first_frame),
            LiveSpectrogramMode::Spectrum,
            bounds,
            &theme,
        );

        assert_eq!(cache.rebuild_count(), 3);
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
            matches!(primitive, PaintPrimitive::FillPolygon(polygon) if polygon.points.len() == LIVE_SPECTROGRAM_BAND_COUNT + 2)
        }));
        assert!(spectrum.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill) if fill.color == SPECTRUM_PLOT_BACKGROUND)
        }));
        assert!(
            spectrum
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::FillPolygon(polygon) if polygon.points.len() == LIVE_SPECTRUM_POINT_COUNT * 2))
        );
        assert_eq!(
            spectrum
                .iter()
                .filter(|primitive| matches!(primitive, PaintPrimitive::FillPath(_)))
                .count(),
            1
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
        let ribbon = primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillPolygon(area)
                    if area.points.len() == LIVE_SPECTRUM_POINT_COUNT * 2 =>
                {
                    Some(area)
                }
                _ => None,
            })
            .expect("spectrum mode should paint one filled ribbon");
        let area = primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillPath(area) => Some(area),
                _ => None,
            })
            .expect("spectrum mode should paint one subtle area");
        let plot = SpectrogramWidget::plot_rect(bounds);

        assert_eq!(ribbon.points.len(), LIVE_SPECTRUM_POINT_COUNT * 2);
        assert_eq!(
            primitives
                .iter()
                .filter(|primitive| matches!(primitive, PaintPrimitive::FillPolygon(_)))
                .count(),
            1
        );
        assert_eq!(
            primitives
                .iter()
                .filter(|primitive| matches!(primitive, PaintPrimitive::FillPath(_)))
                .count(),
            1
        );
        assert_eq!(ribbon.color, ThemeTokens::default().highlight_orange);
        assert_eq!(ribbon.points.first().expect("low band").x, plot.min.x);
        assert_eq!(ribbon.points[LIVE_SPECTRUM_POINT_COUNT - 1].x, plot.max.x);
        assert_eq!(ribbon.points[LIVE_SPECTRUM_POINT_COUNT].x, plot.max.x);
        assert_eq!(
            ribbon.points.last().expect("low band lower edge").x,
            plot.min.x
        );
        assert!(
            ribbon.points[..LIVE_SPECTRUM_POINT_COUNT]
                .windows(2)
                .all(|points| points[0].x < points[1].x)
        );
        assert_eq!(area.path.commands().len(), LIVE_SPECTRUM_POINT_COUNT + 3);
        assert!(matches!(
            area.path.commands().first(),
            Some(PaintPathCommand::MoveTo(point)) if *point == Point::new(plot.min.x, plot.max.y)
        ));
        assert!(matches!(
            area.path.commands().get(LIVE_SPECTRUM_POINT_COUNT - 1),
            Some(PaintPathCommand::LineTo(point)) if *point == Point::new(plot.max.x, plot.min.y)
        ));
        assert!(matches!(
            area.path.commands().last(),
            Some(PaintPathCommand::Close)
        ));
        assert!(matches!(
            area.brush,
            PaintBrush::LinearGradient(gradient)
                if gradient.start_color == ThemeTokens::default().highlight_orange.with_alpha(48)
                    && gradient.end_color == ThemeTokens::default().highlight_orange.with_alpha(0)
        ));
        let area_index = primitives
            .iter()
            .position(|primitive| matches!(primitive, PaintPrimitive::FillPath(_)))
            .expect("area index");
        let ribbon_index = primitives
            .iter()
            .position(|primitive| matches!(primitive, PaintPrimitive::FillPolygon(_)))
            .expect("ribbon index");
        assert!(area_index < ribbon_index);
        assert!(primitives.len() <= 24);
    }

    #[test]
    fn spectrum_layers_area_under_grid_ribbon_above_grid_and_border_last() {
        let bounds = overlay_bounds();
        let plot = SpectrogramWidget::plot_rect(bounds);
        let theme = ThemeTokens::default();
        let widget = SpectrogramWidget::new(test_frame(), LiveSpectrogramMode::Spectrum);
        let retained = widget.paint_primitives(bounds, &LayoutOutput::default(), &theme);
        let mut transient = Vec::new();
        widget.append_overlay_paint(&mut transient, bounds, &theme);

        let assert_order = |label: &str, primitives: &[PaintPrimitive]| {
            let area = primitives
                .iter()
                .position(|primitive| matches!(primitive, PaintPrimitive::FillPath(_)))
                .expect("spectrum area");
            let ribbon = primitives
                .iter()
                .position(|primitive| {
                    matches!(primitive, PaintPrimitive::FillPolygon(polygon) if polygon.points.len() == LIVE_SPECTRUM_POINT_COUNT * 2)
                })
                .expect("spectrum ribbon");
            let grid = primitives
                .iter()
                .enumerate()
                .find_map(|(index, primitive)| {
                    if index > area
                        && index < ribbon
                        && matches!(
                            primitive,
                            PaintPrimitive::StrokePolyline(_) | PaintPrimitive::StrokeRect(_)
                        )
                    {
                        Some(index)
                    } else {
                        None
                    }
                })
                .expect("spectrum grid");
            let border = primitives
                .iter()
                .enumerate()
                .find_map(|(index, primitive)| {
                    if index > ribbon
                        && matches!(
                            primitive,
                            PaintPrimitive::StrokeRect(border)
                                if border.rect == plot && border.color == theme.border_emphasis
                        )
                    {
                        Some(index)
                    } else {
                        None
                    }
                })
                .expect("spectrum plot border");

            assert!(
                area < grid && grid < ribbon && ribbon < border,
                "{label} spectrum order was area={area}, grid={grid}, ribbon={ribbon}, border={border}"
            );
        };

        assert_order("retained", &retained);
        assert_order("transient", &transient);
    }

    #[test]
    fn spectrum_ribbon_is_clamped_and_keeps_nominal_width_inside_plot() {
        let mut frame = (*test_frame()).clone();
        let mut spectrum_values = vec![0_u8; LIVE_SPECTRUM_POINT_COUNT];
        spectrum_values[LIVE_SPECTRUM_POINT_COUNT / 2] = 128;
        frame.spectrum_values = Arc::from(spectrum_values.into_boxed_slice());
        let widget = SpectrogramWidget::new(Arc::new(frame), LiveSpectrogramMode::Spectrum);
        let bounds = overlay_bounds();
        let plot = SpectrogramWidget::plot_rect(bounds);
        let geometry = widget
            .spectrum_geometry(plot)
            .expect("valid detailed spectrum geometry");
        assert_eq!(geometry.ribbon_points.len(), LIVE_SPECTRUM_POINT_COUNT * 2);

        for point in geometry.ribbon_points.iter() {
            assert!(point.x >= plot.min.x && point.x <= plot.max.x);
            assert!(point.y >= plot.min.y && point.y <= plot.max.y);
        }
        for point in 0..LIVE_SPECTRUM_POINT_COUNT {
            let upper = geometry.ribbon_points[point];
            let lower = geometry.ribbon_points[2 * LIVE_SPECTRUM_POINT_COUNT - point - 1];
            let width = lower.y - upper.y;
            assert!((0.0..=SPECTRUM_RIBBON_WIDTH).contains(&width));
        }
        let middle = LIVE_SPECTRUM_POINT_COUNT / 2;
        let middle_upper = geometry.ribbon_points[middle];
        let middle_lower = geometry.ribbon_points[2 * LIVE_SPECTRUM_POINT_COUNT - middle - 1];
        assert!((middle_lower.y - middle_upper.y - SPECTRUM_RIBBON_WIDTH).abs() < 0.001);
        let low_width = geometry.ribbon_points[2 * LIVE_SPECTRUM_POINT_COUNT - 1].y
            - geometry.ribbon_points[0].y;
        let high_width = geometry.ribbon_points[LIVE_SPECTRUM_POINT_COUNT].y
            - geometry.ribbon_points[LIVE_SPECTRUM_POINT_COUNT - 1].y;
        assert!(low_width <= SPECTRUM_RIBBON_WIDTH * 0.5 + 0.001);
        assert!(high_width <= SPECTRUM_RIBBON_WIDTH * 0.5 + 0.001);
    }

    #[test]
    fn waterfall_scale_helpers_clamp_and_round_trip_normalized_values() {
        assert_eq!(super::clamp_history_scale(f32::NAN), 1.0);
        assert_eq!(super::clamp_history_scale(f32::NEG_INFINITY), 1.0);
        assert_eq!(super::clamp_history_scale(0.5), 1.0);
        assert_eq!(super::clamp_history_scale(8.0), 4.0);
        assert_eq!(super::history_scale_from_normalized(-1.0), 1.0);
        assert_eq!(super::history_scale_from_normalized(2.0), 4.0);
        assert_eq!(super::history_scale_from_normalized(f32::NAN), 1.0);

        for normalized in [0.0, 0.125, 0.5, 0.875, 1.0] {
            let scale = super::history_scale_from_normalized(normalized);
            assert!((super::history_scale_to_normalized(scale) - normalized).abs() < 1.0e-6);
        }
    }

    #[test]
    fn waterfall_row_intervals_use_exact_age_and_stable_height() {
        let plot = Rect::from_min_max(Point::new(10.0, 20.0), Point::new(110.0, 120.0));
        let scale = 2.5;

        assert_eq!(
            super::waterfall_row_y_interval(plot, 4, 3, scale),
            Some((117.5, 120.0))
        );
        assert_eq!(
            super::waterfall_row_y_interval(plot, 4, 0, scale),
            Some((110.0, 112.5))
        );

        for row_count in [1, 4, LIVE_SPECTROGRAM_MAX_HISTORY] {
            let rows = super::visible_waterfall_rows(plot, row_count, scale);
            for pair in rows.windows(2) {
                if pair[0].1.height() == scale && pair[1].1.height() == scale {
                    assert_eq!(pair[0].1.height(), pair[1].1.height());
                }
            }
        }
    }

    #[test]
    fn waterfall_crops_oldest_rows_without_aggregation() {
        let row_count = 5;
        let mut values = vec![0_u8; row_count * LIVE_SPECTROGRAM_BAND_COUNT];
        for (row, value) in [10_u8, 20, 30, 40, 50].into_iter().enumerate() {
            for band in 0..LIVE_SPECTROGRAM_BAND_COUNT {
                values[row * LIVE_SPECTROGRAM_BAND_COUNT + band] = value;
            }
        }
        let frame = LiveSpectrogramFrame::from_values(
            1,
            1,
            1,
            48_000,
            row_count,
            Arc::from(values.into_boxed_slice()),
            Arc::from(vec![0_u8; LIVE_SPECTRUM_POINT_COUNT].into_boxed_slice()),
        )
        .expect("valid exact-row frame");

        let plot = Rect::from_min_max(Point::new(0.0, 0.0), Point::new(100.0, 5.0));
        let visible = super::visible_waterfall_rows(plot, row_count, 2.0);
        assert_eq!(
            visible.iter().map(|(row, _)| *row).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        assert_eq!(
            visible[0].1,
            Rect::from_min_max(Point::new(0.0, 0.0), Point::new(100.0, 1.0))
        );
        assert_eq!(
            visible[1].1,
            Rect::from_min_max(Point::new(0.0, 1.0), Point::new(100.0, 3.0))
        );
        assert_eq!(
            visible[2].1,
            Rect::from_min_max(Point::new(0.0, 3.0), Point::new(100.0, 5.0))
        );

        let widget =
            SpectrogramWidget::new_with_scale(Arc::new(frame), LiveSpectrogramMode::Waterfall, 2.0);
        let mut primitives = Vec::new();
        widget.append_overlay_waterfall(&mut primitives, plot);
        let rects = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillRectBatch(batch) => Some(batch.rects.iter()),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(rects.len(), 3);
        assert!(rects.iter().all(|rect| rect.height() <= 2.0));
        assert!(
            rects
                .iter()
                .any(|rect| rect.min.y == 0.0 && rect.max.y == 1.0)
        );
        assert!(
            rects
                .iter()
                .any(|rect| rect.min.y == 1.0 && rect.max.y == 3.0)
        );
        assert!(
            rects
                .iter()
                .any(|rect| rect.min.y == 3.0 && rect.max.y == 5.0)
        );
    }

    #[test]
    fn spectrum_area_geometry_is_shared_by_retained_and_transient_paint() {
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let frame = test_frame();
        let widget = SpectrogramWidget::new(frame, LiveSpectrogramMode::Spectrum);
        let retained =
            widget.paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());
        let mut transient = Vec::new();
        widget.append_overlay_paint(&mut transient, bounds, &ThemeTokens::default());

        let retained_area = retained.iter().find_map(|primitive| match primitive {
            PaintPrimitive::FillPolygon(area) => Some(area),
            _ => None,
        });
        let transient_area = transient.iter().find_map(|primitive| match primitive {
            PaintPrimitive::FillPolygon(area) => Some(area),
            _ => None,
        });
        assert_eq!(retained_area, transient_area);
        assert!(
            retained_area.is_some_and(|area| area.points.len() == LIVE_SPECTRUM_POINT_COUNT * 2)
        );

        let retained_path = retained.iter().find_map(|primitive| match primitive {
            PaintPrimitive::FillPath(path) => Some(path),
            _ => None,
        });
        let transient_path = transient.iter().find_map(|primitive| match primitive {
            PaintPrimitive::FillPath(path) => Some(path),
            _ => None,
        });
        assert_eq!(retained_path, transient_path);
    }

    #[test]
    fn transient_paint_contains_only_replayable_primitives() {
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));
        let mut primitives = Vec::new();
        SpectrogramWidget::new(test_frame(), LiveSpectrogramMode::Spectrum).append_overlay_paint(
            &mut primitives,
            bounds,
            &ThemeTokens::default(),
        );

        assert!(primitives.iter().all(|primitive| {
            matches!(
                primitive,
                PaintPrimitive::FillRect(_)
                    | PaintPrimitive::FillRectBatch(_)
                    | PaintPrimitive::FillPath(_)
                    | PaintPrimitive::FillPolygon(_)
                    | PaintPrimitive::StrokeRect(_)
                    | PaintPrimitive::StrokeRectBatch(_)
                    | PaintPrimitive::StrokePolygon(_)
                    | PaintPrimitive::Svg(_)
                    | PaintPrimitive::Text(_)
            )
        }));
        assert!(
            !primitives
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::StrokePolyline(_)))
        );
        assert!(
            !primitives
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::GpuSurface(_)))
        );
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
    fn frequency_grid_labels_follow_source_nyquist() {
        let bounds = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT));

        for (sample_rate, expected_endpoint, forbidden_endpoint) in [
            (16_000, "8 kHz", ["10 kHz", "20 kHz"]),
            (22_050, "11 kHz", ["20 kHz", ""]),
        ] {
            let mut frame = (*test_frame()).clone();
            frame.sample_rate = sample_rate;
            let primitives =
                SpectrogramWidget::new(Arc::new(frame), LiveSpectrogramMode::Waterfall)
                    .paint_primitives(bounds, &LayoutOutput::default(), &ThemeTokens::default());
            let labels = primitives
                .iter()
                .filter_map(|primitive| match primitive {
                    PaintPrimitive::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();

            assert!(
                labels.contains(&expected_endpoint),
                "missing {expected_endpoint} for {sample_rate} Hz"
            );
            for forbidden in forbidden_endpoint {
                if !forbidden.is_empty() {
                    assert!(!labels.contains(&forbidden), "unexpected {forbidden}");
                }
            }
        }
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

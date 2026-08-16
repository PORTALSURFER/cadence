//! Passive live-spectrogram heatmap for the native Review/Audition surface.
//!
//! The transport owns capture and analysis. This module only paints the latest
//! immutable, quantized frame: frequency increases from left to right, while
//! the oldest retained row is above the newest row at the bottom edge.

use crate::transport::{
    LIVE_SPECTROGRAM_BAND_COUNT, LIVE_SPECTRUM_DISPLAY_CEILING_DB, LIVE_SPECTRUM_DISPLAY_FLOOR_DB,
    LIVE_SPECTRUM_POINT_COUNT, LiveSpectrogramFrame, live_display_frequency_bounds,
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
const SPECTRUM_SURFACE_KEY: u64 = 0x4341_4445_4e43_5350;
const SPECTRUM_STORAGE_IDENTITY: u64 = 0x4341_4445_4e43_5356;
pub const LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID: u64 = 0xCAD3_2201;
const WATERFALL_SHADER_KEY: &str = "cadence/live-spectrogram-waterfall";
const SPECTRUM_SHADER_KEY: &str = "cadence/live-spectrogram-spectrum";
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

const SPECTRUM_SHADER_WGSL: &str = r#"
struct SurfaceParams {
    dest: vec4<f32>,
    source: vec4<f32>,
    target_size: vec2<f32>,
    _padding: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> surface: SurfaceParams;

struct SpectrumParams {
    point_count: u32,
    half_ribbon_width: f32,
    area_alpha: f32,
    _padding: u32,
    orange: vec4<f32>,
};

@group(0) @binding(1)
var<uniform> params: SpectrumParams;

@group(0) @binding(2)
var<storage, read> spectrum: array<u32>;

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

fn spectrum_sample(sample_index: u32) -> f32 {
    let word = spectrum[sample_index / 4u];
    let shift = (sample_index % 4u) * 8u;
    let value = (word >> shift) & 0xffu;
    return f32(value) / 255.0;
}

fn spectrum_level(local_x: f32) -> f32 {
    // The 768 values are logarithmically distributed in frequency. Their
    // normalized x coordinates are therefore evenly spaced in log-frequency;
    // interpolate linearly between the adjacent quantized display values.
    let last_point = params.point_count - 1u;
    let sample_position = clamp(local_x, 0.0, 1.0) * f32(last_point);
    let lower_point = min(u32(floor(sample_position)), last_point);
    let upper_point = min(lower_point + 1u, last_point);
    let blend = sample_position - f32(lower_point);
    let lower = spectrum_sample(lower_point);
    let upper = spectrum_sample(upper_point);
    return lower + (upper - lower) * blend;
}

@fragment
fn fragment_main(input: VertexOut) -> @location(0) vec4<f32> {
    if params.point_count < 2u {
        return vec4<f32>(0.0);
    }

    let center_y = 1.0 - spectrum_level(input.local.x);
    let ribbon_top = max(center_y - params.half_ribbon_width, 0.0);
    let ribbon_bottom = min(center_y + params.half_ribbon_width, 1.0);
    if input.local.y >= ribbon_top && input.local.y <= ribbon_bottom {
        return params.orange;
    }

    // Match the existing vertical gradient: the orange area begins at the
    // centerline and fades from alpha 48 at the plot top to zero at bottom.
    if input.local.y >= center_y {
        let alpha = params.area_alpha * (1.0 - input.local.y);
        return vec4<f32>(params.orange.rgb, alpha);
    }
    return vec4<f32>(0.0);
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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
            storage_identity: 0,
            storage_revision: 0,
            presentation_uniform_bytes: None,
            presentation_uniform_revision: None,
            vertex_count: 6,
        },
    ))
}

fn spectrum_uniform_bytes(plot: Rect, orange: Rgba8) -> [u8; 32] {
    let values = [LIVE_SPECTRUM_POINT_COUNT as u32, 0_u32, 0_u32, 0_u32];
    let half_ribbon_width = (SPECTRUM_RIBBON_WIDTH * 0.5) / plot.height().max(f32::MIN_POSITIVE);
    let float_values = [
        half_ribbon_width,
        SPECTRUM_AREA_ALPHA as f32 / u8::MAX as f32,
    ];
    let color = [
        orange.r as f32 / u8::MAX as f32,
        orange.g as f32 / u8::MAX as f32,
        orange.b as f32 / u8::MAX as f32,
        orange.a as f32 / u8::MAX as f32,
    ];
    let mut bytes = [0_u8; 32];
    bytes[0..4].copy_from_slice(&values[0].to_le_bytes());
    bytes[4..8].copy_from_slice(&float_values[0].to_le_bytes());
    bytes[8..12].copy_from_slice(&float_values[1].to_le_bytes());
    for (index, value) in color.into_iter().enumerate() {
        let start = 16 + index * std::mem::size_of::<f32>();
        bytes[start..start + std::mem::size_of::<f32>()].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn spectrum_storage_identity(plot: Rect, orange: Rgba8) -> u64 {
    // Radiant's immutable-payload fence covers both uniform and storage
    // bytes. Include the static plot/style inputs so a resize or theme change
    // refreshes those bytes without changing the frame data revision.
    let color = u32::from(orange.r) << 24
        | u32::from(orange.g) << 16
        | u32::from(orange.b) << 8
        | u32::from(orange.a);
    let identity = SPECTRUM_STORAGE_IDENTITY
        ^ u64::from(plot.height().to_bits()).rotate_left(17)
        ^ u64::from(color).rotate_left(41);
    if identity == 0 { 1 } else { identity }
}

fn spectrum_shader_descriptor(
    frame: &LiveSpectrogramFrame,
    plot: Rect,
    theme: &ThemeTokens,
) -> Arc<GpuShaderSurfaceDescriptor> {
    static SHADER_SOURCE: OnceLock<Arc<str>> = OnceLock::new();
    let uniform_bytes = spectrum_uniform_bytes(plot, theme.highlight_orange);
    Arc::new(GpuShaderSurfaceDescriptor::from_parts(
        GpuShaderSurfaceDescriptorParts {
            shader_key: String::from(SPECTRUM_SHADER_KEY),
            wgsl_source: Some(Arc::clone(
                SHADER_SOURCE.get_or_init(|| Arc::<str>::from(SPECTRUM_SHADER_WGSL)),
            )),
            entry_point: String::from("vertex_main"),
            fragment_entry_point: Some(String::from("fragment_main")),
            uniform_bytes: Arc::<[u8]>::from(uniform_bytes.as_slice()),
            storage_bytes: Arc::clone(&frame.spectrum_values),
            storage_identity: spectrum_storage_identity(plot, theme.highlight_orange),
            storage_revision: frame.gpu_revision(),
            presentation_uniform_bytes: None,
            presentation_uniform_revision: None,
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

    fn append_spectrum(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        plot: Rect,
        theme: &ThemeTokens,
    ) {
        let Some(frame) = self.frame.as_ref() else {
            return;
        };
        if !plot.has_finite_positive_area() || !frame.is_valid() {
            return;
        }
        primitives.push(PaintPrimitive::GpuSurface(PaintGpuSurface {
            widget_id: self.common.id,
            key: SPECTRUM_SURFACE_KEY,
            revision: frame.gpu_revision(),
            rect: plot,
            content: GpuSurfaceContent::CustomShader {
                descriptor: spectrum_shader_descriptor(frame, plot, theme),
            },
            capabilities: GpuSurfaceCapabilities::default(),
            overlays: Vec::new(),
        }));
    }
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

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_HISTORY_SCALE, SPECTRUM_AREA_ALPHA, SPECTRUM_PLOT_BACKGROUND,
        SPECTRUM_RIBBON_WIDTH, SPECTRUM_SHADER_KEY, SPECTRUM_SHADER_WGSL, SPECTRUM_SURFACE_KEY,
        SpectrogramWidget, clamp_height, clamp_history_scale, history_scale_from_normalized,
        history_scale_to_normalized, visible_waterfall_rows, waterfall_row_rect,
        waterfall_row_y_interval,
    };
    use crate::LiveSpectrogramMode;
    use crate::transport::{
        LIVE_SPECTROGRAM_BAND_COUNT, LIVE_SPECTROGRAM_MAX_HISTORY, LIVE_SPECTRUM_POINT_COUNT,
        LiveSpectrogramFrame,
    };
    use radiant::{
        gui::types::{Point, Rect, Vector2},
        layout::LayoutOutput,
        runtime::{GpuShaderSurfaceDescriptor, GpuSurfaceContent, PaintGpuSurface, PaintPrimitive},
        theme::ThemeTokens,
        widgets::Widget,
    };
    use std::sync::Arc;

    fn bounds() -> Rect {
        Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(720.0, super::HEIGHT))
    }

    fn test_frame() -> Arc<LiveSpectrogramFrame> {
        let row_count = 2;
        let mut values = vec![0_u8; row_count * LIVE_SPECTROGRAM_BAND_COUNT];
        values[0] = u8::MAX;
        values[(row_count - 1) * LIVE_SPECTROGRAM_BAND_COUNT + LIVE_SPECTROGRAM_BAND_COUNT - 1] =
            u8::MAX;
        let mut spectrum_values = vec![0_u8; LIVE_SPECTRUM_POINT_COUNT];
        spectrum_values[1] = 64;
        spectrum_values[2] = 128;
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

    fn paint(widget: SpectrogramWidget) -> Vec<PaintPrimitive> {
        widget.paint_primitives(bounds(), &LayoutOutput::default(), &ThemeTokens::default())
    }

    fn gpu_surfaces(primitives: &[PaintPrimitive]) -> Vec<&PaintGpuSurface> {
        primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::GpuSurface(surface) => Some(surface),
                _ => None,
            })
            .collect()
    }

    fn custom_descriptor(surface: &PaintGpuSurface) -> &GpuShaderSurfaceDescriptor {
        let GpuSurfaceContent::CustomShader { descriptor } = &surface.content else {
            panic!("spectrogram surface should use a custom shader");
        };
        descriptor.as_ref()
    }

    fn uniform_f32(bytes: &[u8], offset: usize) -> f32 {
        f32::from_le_bytes(
            bytes[offset..offset + std::mem::size_of::<f32>()]
                .try_into()
                .expect("uniform f32 bytes"),
        )
    }

    #[test]
    fn empty_frame_keeps_shell_grid_labels_and_border_visible() {
        let primitives = paint(SpectrogramWidget::empty(
            16_000,
            LiveSpectrogramMode::Spectrum,
        ));
        let plot = SpectrogramWidget::plot_rect(bounds());

        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill) if fill.rect == bounds())
        }));
        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill) if fill.rect == plot)
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
            matches!(primitive, PaintPrimitive::StrokeRect(border) if border.rect == plot)
        }));
        assert!(gpu_surfaces(&primitives).is_empty());
    }

    #[test]
    fn retained_waterfall_stays_on_its_existing_gpu_contract() {
        let frame = test_frame();
        let primitives = paint(SpectrogramWidget::new(
            frame,
            LiveSpectrogramMode::Waterfall,
        ));
        let surfaces = gpu_surfaces(&primitives);
        assert_eq!(surfaces.len(), 1);
        assert_eq!(surfaces[0].rect, SpectrogramWidget::plot_rect(bounds()));

        let descriptor = custom_descriptor(surfaces[0]);
        assert_eq!(descriptor.shader_key, "cadence/live-spectrogram-waterfall");
        assert_eq!(descriptor.entry_point, "vertex_main");
        assert_eq!(
            descriptor.fragment_entry_point.as_deref(),
            Some("fragment_main")
        );
        assert_eq!(descriptor.storage_identity, 0);
        assert_eq!(descriptor.storage_revision, 0);
        assert_eq!(descriptor.presentation_uniform_bytes, None);
        assert_eq!(descriptor.presentation_uniform_revision, None);
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
        let source = descriptor.wgsl_source.as_deref().expect("waterfall shader");
        assert!(
            source.contains("row_index")
                && source.contains("row_step")
                && source.contains("row_age")
                && source.contains("band_position")
                && source.contains("oldest-to-newest")
                && source.contains("age zero is anchored at the bottom")
        );
        assert!(
            !primitives
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::FillRectBatch(_)))
        );
    }

    #[test]
    fn retained_waterfall_uniform_keeps_normalized_row_step() {
        let plot = SpectrogramWidget::plot_rect(bounds());
        let primitives = paint(SpectrogramWidget::new_with_scale(
            test_frame(),
            LiveSpectrogramMode::Waterfall,
            2.5,
        ));
        let surface = gpu_surfaces(&primitives)
            .into_iter()
            .next()
            .expect("waterfall should retain a GPU surface");
        let descriptor = custom_descriptor(surface);

        assert_eq!(
            uniform_f32(&descriptor.uniform_bytes, 8),
            2.5 / plot.height()
        );
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
    fn retained_spectrum_uses_one_gpu_surface_with_direct_revisioned_payload() {
        let frame = test_frame();
        let primitives = paint(SpectrogramWidget::new(
            Arc::clone(&frame),
            LiveSpectrogramMode::Spectrum,
        ));
        let surfaces = gpu_surfaces(&primitives);
        assert_eq!(surfaces.len(), 1);
        let surface = surfaces[0];
        let descriptor = custom_descriptor(surface);

        assert_eq!(surface.key, SPECTRUM_SURFACE_KEY);
        assert_ne!(surface.key, super::WATERFALL_SURFACE_KEY);
        assert_eq!(surface.revision, frame.gpu_revision());
        assert_eq!(descriptor.shader_key, SPECTRUM_SHADER_KEY);
        assert_ne!(descriptor.shader_key, "cadence/live-spectrogram-waterfall");
        assert_eq!(descriptor.storage_bytes.len(), LIVE_SPECTRUM_POINT_COUNT);
        assert!(Arc::ptr_eq(
            &descriptor.storage_bytes,
            &frame.spectrum_values
        ));
        assert_ne!(descriptor.storage_identity, 0);
        assert_eq!(descriptor.storage_revision, frame.gpu_revision());
        assert_eq!(descriptor.entry_point, "vertex_main");
        assert_eq!(
            descriptor.fragment_entry_point.as_deref(),
            Some("fragment_main")
        );
        assert_eq!(descriptor.presentation_uniform_bytes, None);
        assert_eq!(descriptor.presentation_uniform_revision, None);
        assert_eq!(descriptor.vertex_count, 6);
        assert_eq!(descriptor.uniform_bytes.len(), 32);
        assert!(!primitives.iter().any(|primitive| {
            matches!(
                primitive,
                PaintPrimitive::FillPath(_) | PaintPrimitive::FillPolygon(_)
            )
        }));
    }

    #[test]
    fn spectrum_uniform_preserves_area_ribbon_and_color_parameters() {
        let theme = ThemeTokens::default();
        let plot = SpectrogramWidget::plot_rect(bounds());
        let primitives = SpectrogramWidget::new(test_frame(), LiveSpectrogramMode::Spectrum)
            .paint_primitives(bounds(), &LayoutOutput::default(), &theme);
        let surface = gpu_surfaces(&primitives)
            .into_iter()
            .next()
            .expect("spectrum should retain a GPU surface");
        let descriptor = custom_descriptor(surface);

        assert_eq!(
            u32::from_le_bytes(
                descriptor.uniform_bytes[0..4]
                    .try_into()
                    .expect("point count bytes")
            ),
            LIVE_SPECTRUM_POINT_COUNT as u32
        );
        assert_eq!(
            uniform_f32(&descriptor.uniform_bytes, 4),
            (SPECTRUM_RIBBON_WIDTH * 0.5) / plot.height()
        );
        assert_eq!(
            uniform_f32(&descriptor.uniform_bytes, 8),
            SPECTRUM_AREA_ALPHA as f32 / u8::MAX as f32
        );
        for (index, channel) in [
            theme.highlight_orange.r,
            theme.highlight_orange.g,
            theme.highlight_orange.b,
            theme.highlight_orange.a,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                uniform_f32(&descriptor.uniform_bytes, 16 + index * 4),
                channel as f32 / u8::MAX as f32
            );
        }
    }

    #[test]
    fn spectrum_surface_keeps_normal_paint_order_and_has_no_transient_graph() {
        let primitives = paint(SpectrogramWidget::new(
            test_frame(),
            LiveSpectrogramMode::Spectrum,
        ));
        let plot = SpectrogramWidget::plot_rect(bounds());
        let surface_index = primitives
            .iter()
            .position(|primitive| matches!(primitive, PaintPrimitive::GpuSurface(_)))
            .expect("spectrum GPU surface");
        let first_grid_index = primitives
            .iter()
            .position(|primitive| {
                matches!(
                    primitive,
                    PaintPrimitive::StrokePolyline(_) | PaintPrimitive::Text(_)
                )
            })
            .expect("normal spectrum grid");
        let border_index = primitives
            .iter()
            .rposition(|primitive| {
                matches!(primitive, PaintPrimitive::StrokeRect(border) if border.rect == plot)
            })
            .expect("spectrum border");

        assert_eq!(surface_index, 2);
        assert!(surface_index < first_grid_index);
        assert!(first_grid_index < border_index);
        assert_eq!(border_index, primitives.len() - 1);
        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill) if fill.rect == bounds())
        }));
        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill) if fill.rect == plot && fill.color == SPECTRUM_PLOT_BACKGROUND)
        }));
        assert!(!primitives.iter().any(|primitive| {
            matches!(
                primitive,
                PaintPrimitive::FillPath(_) | PaintPrimitive::FillPolygon(_)
            )
        }));
    }

    #[test]
    fn spectrum_shader_declares_log_linear_interpolation_and_clamped_ribbon() {
        assert!(SPECTRUM_SHADER_WGSL.contains("spectrum[sample_index / 4u]"));
        assert!(SPECTRUM_SHADER_WGSL.contains("sample_position"));
        assert!(SPECTRUM_SHADER_WGSL.contains("lower + (upper - lower) * blend"));
        assert!(SPECTRUM_SHADER_WGSL.contains("half_ribbon_width"));
        assert!(SPECTRUM_SHADER_WGSL.contains("ribbon_top"));
        assert!(SPECTRUM_SHADER_WGSL.contains("ribbon_bottom"));
        assert!(SPECTRUM_SHADER_WGSL.contains("params.area_alpha * (1.0 - input.local.y)"));
        assert!(SPECTRUM_SHADER_WGSL.contains("logarithmically distributed"));
    }

    #[test]
    fn spectrum_surface_revision_follows_frame_gpu_revision() {
        let first = test_frame();
        let second = Arc::new(
            LiveSpectrogramFrame::from_values(
                first.generation,
                first.epoch,
                first.revision + 1,
                first.sample_rate,
                first.row_count,
                Arc::clone(&first.values),
                Arc::clone(&first.spectrum_values),
            )
            .expect("valid second frame"),
        );
        let first_primitives = paint(SpectrogramWidget::new(
            Arc::clone(&first),
            LiveSpectrogramMode::Spectrum,
        ));
        let first_surface = gpu_surfaces(&first_primitives)
            .into_iter()
            .next()
            .expect("first spectrum surface");
        let second_primitives = paint(SpectrogramWidget::new(
            Arc::clone(&second),
            LiveSpectrogramMode::Spectrum,
        ));
        let second_surface = gpu_surfaces(&second_primitives)
            .into_iter()
            .next()
            .expect("second spectrum surface");

        assert_ne!(first.gpu_revision(), second.gpu_revision());
        assert_eq!(first_surface.revision, first.gpu_revision());
        assert_eq!(second_surface.revision, second.gpu_revision());
        assert_eq!(
            custom_descriptor(first_surface).storage_revision,
            first.gpu_revision()
        );
        assert_eq!(
            custom_descriptor(second_surface).storage_revision,
            second.gpu_revision()
        );
    }

    #[test]
    fn waterfall_storage_stays_bounded_for_maximum_history() {
        let values = vec![0_u8; LIVE_SPECTROGRAM_MAX_HISTORY * LIVE_SPECTROGRAM_BAND_COUNT];
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
            .expect("valid maximum-history frame"),
        );
        let primitives = paint(SpectrogramWidget::new(
            frame,
            LiveSpectrogramMode::Waterfall,
        ));
        let surface = gpu_surfaces(&primitives)
            .into_iter()
            .next()
            .expect("waterfall surface");
        assert_eq!(
            custom_descriptor(surface).storage_bytes.len(),
            expected_storage_len
        );
        assert!(
            !primitives
                .iter()
                .any(|primitive| matches!(primitive, PaintPrimitive::FillRectBatch(_)))
        );
        assert!(primitives.len() <= 32);
    }

    #[test]
    fn waterfall_row_geometry_is_clipped_and_newest_row_is_at_bottom() {
        let plot = Rect::from_min_size(Point::new(10.0, 20.0), Vector2::new(100.0, 40.0));
        assert_eq!(
            waterfall_row_y_interval(plot, 3, 2, 2.5),
            Some((57.5, 60.0))
        );
        assert_eq!(
            waterfall_row_y_interval(plot, 3, 0, 2.5),
            Some((52.5, 55.0))
        );
        assert_eq!(
            waterfall_row_rect(plot, 20, 4, 2.5),
            Some(Rect::from_min_max(
                Point::new(10.0, 20.0),
                Point::new(110.0, 22.5)
            ))
        );
        assert_eq!(visible_waterfall_rows(plot, 3, 2.5).len(), 3);
    }

    #[test]
    fn scale_helpers_clamp_and_round_trip() {
        assert_eq!(clamp_height(f32::NAN), super::HEIGHT);
        assert_eq!(clamp_height(1.0), super::MIN_HEIGHT);
        assert_eq!(clamp_height(999.0), super::MAX_HEIGHT);
        assert_eq!(clamp_history_scale(f32::NAN), DEFAULT_HISTORY_SCALE);
        assert_eq!(clamp_history_scale(0.0), super::MIN_HISTORY_SCALE);
        assert_eq!(clamp_history_scale(99.0), super::MAX_HISTORY_SCALE);
        assert_eq!(history_scale_from_normalized(0.5), 2.5);
        assert_eq!(history_scale_to_normalized(2.5), 0.5);
    }
}

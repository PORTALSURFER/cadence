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
        GpuSurfaceContent, PaintClipEnd, PaintClipStart, PaintFillPolygon, PaintFillRect,
        PaintFillRectBatch, PaintGpuSurface, PaintPrimitive, PaintStrokePolyline, PaintStrokeRect,
        PaintTextAlign, PaintTextMetrics, push_text_run_with_metrics,
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
const WATERFALL_STORAGE_IDENTITY: u64 = 0x4341_4445_4e43_5756;
const SPECTRUM_AREA_SURFACE_KEY: u64 = 0x4341_4445_4e43_5350;
const SPECTRUM_RIBBON_SURFACE_KEY: u64 = 0x4341_4445_4e43_5352;
const SPECTRUM_STORAGE_IDENTITY: u64 = 0x4341_4445_4e43_5356;
pub const LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID: u64 = 0xCAD3_2201;
const OVERLAY_MAX_WATERFALL_COLUMNS: usize = 160;
const OVERLAY_MAX_WATERFALL_ROWS: usize = 64;
const OVERLAY_MAX_SPECTRUM_POINTS: usize = 256;
const WATERFALL_SHADER_KEY: &str = "cadence/live-spectrogram-waterfall";
const SPECTRUM_SHADER_KEY: &str = "cadence/live-spectrogram-spectrum";
const SPECTRUM_RIBBON_WIDTH: f32 = 1.5;
const SPECTRUM_AREA_ALPHA: u8 = 48;
const SPECTRUM_AREA_RENDER_KIND: u32 = 0;
const SPECTRUM_RIBBON_RENDER_KIND: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverlayGeometryKey {
    generation: u64,
    epoch: u64,
    revision: u64,
    gpu_revision: u64,
    mode: crate::LiveSpectrogramMode,
    history_scale_bits: u32,
    bounds: [u32; 4],
    theme: [[u8; 4]; 5],
}

#[derive(Clone, Debug, Default)]
struct OverlayGeometryStorage {
    waterfall_rects: [Vec<Rect>; PALETTE.len()],
    waterfall_batches: [Option<Arc<[Rect]>>; PALETTE.len()],
    spectrum_curve: Vec<Point>,
    spectrum_area: Vec<Point>,
    spectrum_ribbon: Vec<Point>,
    spectrum_area_points: Option<Arc<[Point]>>,
    spectrum_ribbon_points: Option<Arc<[Point]>>,
}

fn refresh_arc_buffer<T: Clone>(slot: &mut Option<Arc<[T]>>, values: &[T]) -> Arc<[T]> {
    if let Some(buffer) = slot.as_mut()
        && buffer.len() == values.len()
        && let Some(slice) = Arc::get_mut(buffer)
    {
        slice.clone_from_slice(values);
        return Arc::clone(buffer);
    }

    let replacement = Arc::from(values.to_vec().into_boxed_slice());
    *slot = Some(Arc::clone(&replacement));
    replacement
}

/// Reusable shape geometry for the playback-owned live overlay.
///
/// The retained spectrogram widget has its own GPU surface cache. This cache is
/// deliberately separate: it stores only the bounded replayable overlay
/// primitives and is invalidated whenever the frame, display settings, bounds,
/// or colors change.
#[derive(Clone, Debug, Default)]
pub(crate) struct OverlayGeometryCache {
    key: Option<OverlayGeometryKey>,
    primitives: Vec<PaintPrimitive>,
    storage: OverlayGeometryStorage,
}

impl OverlayGeometryCache {
    pub(crate) fn clear(&mut self) {
        self.key = None;
        self.primitives.clear();
    }

    fn primitives_for(
        &mut self,
        frame: &LiveSpectrogramFrame,
        mode: crate::LiveSpectrogramMode,
        history_scale: f32,
        bounds: Rect,
        theme: &ThemeTokens,
    ) -> Option<&[PaintPrimitive]> {
        if !frame.is_valid() || !bounds.has_finite_positive_area() {
            self.clear();
            return None;
        }

        let key = OverlayGeometryKey {
            generation: frame.generation,
            epoch: frame.epoch,
            revision: frame.revision,
            gpu_revision: frame.gpu_revision(),
            mode,
            history_scale_bits: clamp_history_scale(history_scale).to_bits(),
            bounds: [
                bounds.min.x.to_bits(),
                bounds.min.y.to_bits(),
                bounds.max.x.to_bits(),
                bounds.max.y.to_bits(),
            ],
            theme: [
                color_key(theme.bg_primary),
                color_key(theme.surface_overlay),
                color_key(theme.highlight_orange),
                color_key(theme.border),
                color_key(theme.border_emphasis),
            ],
        };
        if self.key != Some(key) {
            self.primitives.clear();
            append_overlay_primitives(
                frame,
                mode,
                history_scale,
                bounds,
                &mut self.primitives,
                theme,
                &mut self.storage,
            );
            self.key = Some(key);
        }
        Some(self.primitives.as_slice())
    }

    #[cfg(test)]
    fn primitives_ptr(&self) -> Option<*const PaintPrimitive> {
        self.primitives.first().map(std::ptr::from_ref)
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.key.is_none() && self.primitives.is_empty()
    }
}

fn color_key(color: Rgba8) -> [u8; 4] {
    [color.r, color.g, color.b, color.a]
}

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
    render_kind: u32,
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
    if params.render_kind == 1u {
        let ribbon_top = max(center_y - params.half_ribbon_width, 0.0);
        let ribbon_bottom = min(center_y + params.half_ribbon_width, 1.0);
        if input.local.y >= ribbon_top && input.local.y <= ribbon_bottom {
            return params.orange;
        }
    } else if params.render_kind == 0u {
        // Match the existing vertical gradient: the orange area begins at the
        // centerline and fades from alpha 48 at the plot top to zero at bottom.
        if input.local.y >= center_y {
            let alpha = params.area_alpha * (1.0 - input.local.y);
            return vec4<f32>(params.orange.rgb, alpha);
        }
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

fn waterfall_storage_identity(plot: Rect, history_scale: f32) -> u64 {
    let identity = WATERFALL_STORAGE_IDENTITY
        ^ u64::from(plot.height().to_bits()).rotate_left(17)
        ^ u64::from(clamp_history_scale(history_scale).to_bits()).rotate_left(41);
    if identity == 0 { 1 } else { identity }
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
            storage_identity: waterfall_storage_identity(plot, history_scale),
            storage_revision: frame.gpu_revision(),
            presentation_uniform_bytes: None,
            presentation_uniform_revision: None,
            vertex_count: 6,
        },
    ))
}

fn spectrum_uniform_bytes(plot: Rect, orange: Rgba8, render_kind: u32) -> [u8; 32] {
    let values = [LIVE_SPECTRUM_POINT_COUNT as u32, 0_u32, 0_u32, render_kind];
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
    bytes[12..16].copy_from_slice(&values[3].to_le_bytes());
    for (index, value) in color.into_iter().enumerate() {
        let start = 16 + index * std::mem::size_of::<f32>();
        bytes[start..start + std::mem::size_of::<f32>()].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn spectrum_storage_identity(plot: Rect, orange: Rgba8, render_kind: u32) -> u64 {
    // Radiant's immutable-payload fence covers both uniform and storage
    // bytes. Include the static plot/style/render inputs so a resize, theme
    // change, or surface-kind change refreshes those bytes without changing
    // the frame data revision.
    let color = u32::from(orange.r) << 24
        | u32::from(orange.g) << 16
        | u32::from(orange.b) << 8
        | u32::from(orange.a);
    let identity = SPECTRUM_STORAGE_IDENTITY
        ^ u64::from(plot.height().to_bits()).rotate_left(17)
        ^ u64::from(color).rotate_left(41)
        ^ u64::from(render_kind).rotate_left(59);
    if identity == 0 { 1 } else { identity }
}

fn spectrum_shader_descriptor(
    frame: &LiveSpectrogramFrame,
    plot: Rect,
    theme: &ThemeTokens,
    render_kind: u32,
) -> Arc<GpuShaderSurfaceDescriptor> {
    static SHADER_SOURCE: OnceLock<Arc<str>> = OnceLock::new();
    let uniform_bytes = spectrum_uniform_bytes(plot, theme.highlight_orange, render_kind);
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
            storage_identity: spectrum_storage_identity(plot, theme.highlight_orange, render_kind),
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
        key: u64,
        render_kind: u32,
    ) {
        let Some(frame) = self.frame.as_ref() else {
            return;
        };
        if !plot.has_finite_positive_area() || !frame.is_valid() {
            return;
        }
        primitives.push(PaintPrimitive::GpuSurface(PaintGpuSurface {
            widget_id: self.common.id,
            key,
            revision: frame.gpu_revision(),
            rect: plot,
            content: GpuSurfaceContent::CustomShader {
                descriptor: spectrum_shader_descriptor(frame, plot, theme, render_kind),
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
        self.append_spectrogram_paint(primitives, bounds, theme);
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

impl SpectrogramWidget {
    fn append_spectrogram_paint(
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
                crate::LiveSpectrogramMode::Spectrum => self.append_spectrum(
                    primitives,
                    plot,
                    theme,
                    SPECTRUM_AREA_SURFACE_KEY,
                    SPECTRUM_AREA_RENDER_KIND,
                ),
            }
            // Both modes consume the same frame, already normalized to the
            // display-only -90..0 dB range with the signed +4.5 dB/octave tilt.
            self.append_grid(primitives, plot, theme);
            if self.mode == crate::LiveSpectrogramMode::Spectrum {
                self.append_spectrum(
                    primitives,
                    plot,
                    theme,
                    SPECTRUM_RIBBON_SURFACE_KEY,
                    SPECTRUM_RIBBON_RENDER_KIND,
                );
            }
            primitives.push(PaintPrimitive::StrokeRect(PaintStrokeRect {
                widget_id: self.common.id,
                rect: plot,
                color: theme.border_emphasis,
                width: 1.0,
            }));
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

fn overlay_palette_index(value: u8) -> usize {
    ((usize::from(value) * (PALETTE.len() - 1) + (u8::MAX as usize / 2)) / u8::MAX as usize)
        .min(PALETTE.len() - 1)
}

fn append_overlay_waterfall(
    frame: &LiveSpectrogramFrame,
    plot: Rect,
    history_scale: f32,
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    storage: &mut OverlayGeometryStorage,
) {
    if frame.row_count == 0 || !plot.has_finite_positive_area() {
        return;
    }

    let columns = (plot.width().round().max(1.0) as usize).min(OVERLAY_MAX_WATERFALL_COLUMNS);
    let scale = clamp_history_scale(history_scale);
    let visible_rows = ((plot.height() / scale).ceil() as usize)
        .max(1)
        .min(frame.row_count);
    let rows = visible_rows.min(OVERLAY_MAX_WATERFALL_ROWS);
    let rendered_height = (visible_rows as f32 * scale).min(plot.height());
    let rendered_top = plot.max.y - rendered_height;
    for rectangles in &mut storage.waterfall_rects {
        rectangles.clear();
    }

    for display_row in 0..rows {
        let source_row = frame.row_count - visible_rows
            + if rows == 1 {
                visible_rows - 1
            } else {
                display_row * (visible_rows - 1) / (rows - 1)
            };
        let top = rendered_top + rendered_height * display_row as f32 / rows as f32;
        let bottom = rendered_top + rendered_height * (display_row + 1) as f32 / rows as f32;
        let mut run_bucket = overlay_palette_index(
            frame
                .values
                .get(source_row * LIVE_SPECTROGRAM_BAND_COUNT)
                .copied()
                .unwrap_or_default(),
        );
        let mut run_start = 0;

        for column in 1..=columns {
            let bucket = if column == columns {
                usize::MAX
            } else {
                let source_band = (column * LIVE_SPECTROGRAM_BAND_COUNT / columns)
                    .min(LIVE_SPECTROGRAM_BAND_COUNT - 1);
                overlay_palette_index(
                    frame
                        .values
                        .get(source_row * LIVE_SPECTROGRAM_BAND_COUNT + source_band)
                        .copied()
                        .unwrap_or_default(),
                )
            };
            if bucket != run_bucket {
                let left = plot.min.x + plot.width() * run_start as f32 / columns as f32;
                let right = plot.min.x + plot.width() * column as f32 / columns as f32;
                storage.waterfall_rects[run_bucket].push(Rect::from_min_max(
                    Point::new(left, top),
                    Point::new(right, bottom),
                ));
                run_bucket = bucket;
                run_start = column;
            }
        }
    }

    for (palette_index, color) in PALETTE.iter().copied().enumerate() {
        let rects = storage.waterfall_rects[palette_index].as_slice();
        if rects.is_empty() {
            continue;
        }
        let batch = refresh_arc_buffer(&mut storage.waterfall_batches[palette_index], rects);
        primitives.push(PaintPrimitive::FillRectBatch(PaintFillRectBatch {
            widget_id,
            rects: batch,
            color,
        }));
    }
}

fn append_overlay_spectrum(
    frame: &LiveSpectrogramFrame,
    plot: Rect,
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    theme: &ThemeTokens,
    storage: &mut OverlayGeometryStorage,
) -> bool {
    storage.spectrum_curve.clear();
    storage.spectrum_area.clear();
    storage.spectrum_ribbon.clear();
    if frame.spectrum_values.is_empty() || !plot.has_finite_positive_area() {
        return false;
    }

    let point_count = (plot.width().round().max(2.0) as usize)
        .min(OVERLAY_MAX_SPECTRUM_POINTS)
        .min(frame.spectrum_values.len())
        .max(2);
    storage.spectrum_curve.reserve(point_count);
    for point in 0..point_count {
        let source_index = point * (frame.spectrum_values.len() - 1) / (point_count - 1);
        let level = frame.spectrum_values[source_index] as f32 / u8::MAX as f32;
        let x = plot.min.x + plot.width() * point as f32 / (point_count - 1) as f32;
        let y = plot.max.y - plot.height() * level.clamp(0.0, 1.0);
        storage
            .spectrum_curve
            .push(Point::new(x, y.clamp(plot.min.y, plot.max.y)));
    }

    storage
        .spectrum_area
        .extend_from_slice(&storage.spectrum_curve);
    storage.spectrum_area.extend([
        Point::new(plot.max.x, plot.max.y),
        Point::new(plot.min.x, plot.max.y),
    ]);
    let area = refresh_arc_buffer(
        &mut storage.spectrum_area_points,
        storage.spectrum_area.as_slice(),
    );
    primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
        widget_id,
        points: area,
        color: theme.highlight_orange.with_alpha(SPECTRUM_AREA_ALPHA),
    }));
    true
}

fn append_overlay_spectrum_ribbon(
    plot: Rect,
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    theme: &ThemeTokens,
    storage: &mut OverlayGeometryStorage,
) {
    let curve = storage.spectrum_curve.as_slice();
    if curve.is_empty() {
        return;
    }

    let half_width = SPECTRUM_RIBBON_WIDTH * 0.5;
    storage.spectrum_ribbon.reserve(curve.len() * 2);
    for point in curve {
        storage.spectrum_ribbon.push(Point::new(
            point.x,
            (point.y - half_width).clamp(plot.min.y, plot.max.y),
        ));
    }
    for point in curve.iter().rev() {
        storage.spectrum_ribbon.push(Point::new(
            point.x,
            (point.y + half_width).clamp(plot.min.y, plot.max.y),
        ));
    }
    let ribbon = refresh_arc_buffer(
        &mut storage.spectrum_ribbon_points,
        storage.spectrum_ribbon.as_slice(),
    );
    primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
        widget_id,
        points: ribbon,
        color: theme.highlight_orange,
    }));
}

fn append_overlay_grid(
    frame: &LiveSpectrogramFrame,
    plot: Rect,
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: u64,
    theme: &ThemeTokens,
) {
    let grid_color = theme.border.with_alpha(GRID_LINE_ALPHA);
    for &decibels in DECIBEL_GRID.iter().map(|(value, _)| value) {
        let y = SpectrogramWidget::y_for_decibels(plot, decibels);
        let rect = Rect::from_min_max(
            Point::new(plot.min.x, y.min(plot.max.y - 1.0)),
            Point::new(plot.max.x, (y + 1.0).min(plot.max.y)),
        );
        if rect.has_finite_positive_area() {
            primitives.push(PaintPrimitive::FillRect(PaintFillRect {
                widget_id,
                rect,
                color: grid_color,
            }));
        }
    }

    let (minimum, maximum) = live_display_frequency_bounds(frame.sample_rate);
    let ratio = (maximum / minimum.max(f32::MIN_POSITIVE)).max(1.0);
    let frequencies = [20.0, 100.0, 1_000.0, 10_000.0, 20_000.0];
    let mut previous_x: Option<f32> = None;
    for frequency in frequencies {
        if !(frequency >= minimum && frequency <= maximum) {
            continue;
        }
        let x = plot.min.x
            + plot.width()
                * ((frequency / minimum).ln() / ratio.ln().max(f32::MIN_POSITIVE)).clamp(0.0, 1.0);
        if previous_x.is_some_and(|previous| (x - previous).abs() < 2.0) {
            continue;
        }
        previous_x = Some(x);
        let rect = Rect::from_min_max(
            Point::new(x.min(plot.max.x - 1.0), plot.min.y),
            Point::new((x + 1.0).min(plot.max.x), plot.max.y),
        );
        if rect.has_finite_positive_area() {
            primitives.push(PaintPrimitive::FillRect(PaintFillRect {
                widget_id,
                rect,
                color: grid_color,
            }));
        }
    }
}

/// Paint one validated live frame over the retained spectrogram body.
///
/// The transient overlay cannot replay retained GPU surfaces on all native
/// backends, so its data path is deliberately bounded and shape-based. The
/// regular projected widget retains the higher-resolution GPU path.
fn append_overlay_primitives(
    frame: &LiveSpectrogramFrame,
    mode: crate::LiveSpectrogramMode,
    history_scale: f32,
    bounds: Rect,
    primitives: &mut Vec<PaintPrimitive>,
    theme: &ThemeTokens,
    storage: &mut OverlayGeometryStorage,
) {
    if !frame.is_valid() || !bounds.has_finite_positive_area() {
        return;
    }

    primitives.push(PaintPrimitive::ClipStart(PaintClipStart {
        node_id: LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
        rect: bounds,
    }));
    let plot = SpectrogramWidget::plot_rect(bounds);
    primitives.push(PaintPrimitive::FillRect(PaintFillRect {
        widget_id: LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
        rect: bounds,
        color: theme.bg_primary.blend_toward(theme.surface_overlay, 0.35),
    }));
    if plot.has_finite_positive_area() {
        primitives.push(PaintPrimitive::FillRect(PaintFillRect {
            widget_id: LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
            rect: plot,
            color: match mode {
                crate::LiveSpectrogramMode::Waterfall => PALETTE[0],
                crate::LiveSpectrogramMode::Spectrum => SPECTRUM_PLOT_BACKGROUND,
            },
        }));
        let spectrum_available = match mode {
            crate::LiveSpectrogramMode::Waterfall => {
                append_overlay_waterfall(
                    frame,
                    plot,
                    history_scale,
                    primitives,
                    LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
                    storage,
                );
                false
            }
            crate::LiveSpectrogramMode::Spectrum => append_overlay_spectrum(
                frame,
                plot,
                primitives,
                LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
                theme,
                storage,
            ),
        };
        append_overlay_grid(
            frame,
            plot,
            primitives,
            LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
            theme,
        );
        if spectrum_available {
            append_overlay_spectrum_ribbon(
                plot,
                primitives,
                LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
                theme,
                storage,
            );
        }
        primitives.push(PaintPrimitive::StrokeRect(PaintStrokeRect {
            widget_id: LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
            rect: plot,
            color: theme.border_emphasis,
            width: 1.0,
        }));
    }
    primitives.push(PaintPrimitive::ClipEnd(PaintClipEnd {
        node_id: LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID,
    }));
}

/// Paint one validated live frame over the retained spectrogram body using a
/// reusable bounded geometry cache.
pub(crate) fn paint_overlay_cached(
    frame: Arc<LiveSpectrogramFrame>,
    mode: crate::LiveSpectrogramMode,
    history_scale: f32,
    bounds: Rect,
    primitives: &mut Vec<PaintPrimitive>,
    theme: &ThemeTokens,
    cache: &mut OverlayGeometryCache,
) {
    if let Some(cached) = cache.primitives_for(frame.as_ref(), mode, history_scale, bounds, theme) {
        primitives.extend(cached.iter().cloned());
    }
}

/// Paint one frame without retaining geometry between calls.
///
/// This compatibility wrapper keeps the pure helper convenient for tests and
/// callers that do not own an overlay lifetime.
#[cfg(test)]
pub(crate) fn paint_overlay(
    frame: Arc<LiveSpectrogramFrame>,
    mode: crate::LiveSpectrogramMode,
    history_scale: f32,
    bounds: Rect,
    primitives: &mut Vec<PaintPrimitive>,
    theme: &ThemeTokens,
) {
    paint_overlay_cached(
        frame,
        mode,
        history_scale,
        bounds,
        primitives,
        theme,
        &mut OverlayGeometryCache::default(),
    );
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_HISTORY_SCALE, OVERLAY_MAX_SPECTRUM_POINTS, OVERLAY_MAX_WATERFALL_COLUMNS,
        OVERLAY_MAX_WATERFALL_ROWS, OverlayGeometryCache, PALETTE, SPECTRUM_AREA_ALPHA,
        SPECTRUM_AREA_RENDER_KIND, SPECTRUM_AREA_SURFACE_KEY, SPECTRUM_PLOT_BACKGROUND,
        SPECTRUM_RIBBON_RENDER_KIND, SPECTRUM_RIBBON_SURFACE_KEY, SPECTRUM_RIBBON_WIDTH,
        SPECTRUM_SHADER_KEY, SPECTRUM_SHADER_WGSL, SpectrogramWidget, WATERFALL_STORAGE_IDENTITY,
        clamp_height, clamp_history_scale, history_scale_from_normalized,
        history_scale_to_normalized, paint_overlay, paint_overlay_cached, visible_waterfall_rows,
        waterfall_row_rect, waterfall_row_y_interval, waterfall_shader_descriptor,
        waterfall_storage_identity,
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

    fn test_frame_revision(revision: u64) -> Arc<LiveSpectrogramFrame> {
        let frame = test_frame();
        Arc::new(
            LiveSpectrogramFrame::from_values(
                frame.generation,
                frame.epoch,
                revision,
                frame.sample_rate,
                frame.row_count,
                Arc::clone(&frame.values),
                Arc::clone(&frame.spectrum_values),
            )
            .expect("valid live spectrogram test frame revision"),
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
            Arc::clone(&frame),
            LiveSpectrogramMode::Waterfall,
        ));
        let surfaces = gpu_surfaces(&primitives);
        assert_eq!(surfaces.len(), 1);
        assert_eq!(surfaces[0].key, super::WATERFALL_SURFACE_KEY);
        assert_eq!(
            surfaces[0].revision,
            frame.gpu_revision() ^ u64::from(DEFAULT_HISTORY_SCALE.to_bits())
        );
        assert_eq!(surfaces[0].rect, SpectrogramWidget::plot_rect(bounds()));

        let descriptor = custom_descriptor(surfaces[0]);
        assert_eq!(descriptor.shader_key, "cadence/live-spectrogram-waterfall");
        assert_eq!(descriptor.entry_point, "vertex_main");
        assert_eq!(
            descriptor.fragment_entry_point.as_deref(),
            Some("fragment_main")
        );
        assert_eq!(
            descriptor.storage_identity,
            waterfall_storage_identity(surfaces[0].rect, DEFAULT_HISTORY_SCALE)
        );
        assert_ne!(descriptor.storage_identity, 0);
        assert_eq!(descriptor.storage_revision, frame.gpu_revision());
        assert!(Arc::ptr_eq(
            &descriptor.storage_bytes,
            frame.packed_values()
        ));
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
    fn waterfall_storage_identity_is_nonzero_and_stable_for_same_frame_geometry() {
        let frame = test_frame();
        let plot = SpectrogramWidget::plot_rect(bounds());
        let first = waterfall_shader_descriptor(frame.as_ref(), plot, 2.5);
        let second = waterfall_shader_descriptor(frame.as_ref(), plot, 2.5);

        assert_ne!(WATERFALL_STORAGE_IDENTITY, 0);
        assert_ne!(first.storage_identity, 0);
        assert_eq!(first.storage_identity, second.storage_identity);
        assert_eq!(
            first.storage_identity,
            waterfall_storage_identity(plot, 2.5)
        );
        assert_eq!(first.storage_revision, frame.gpu_revision());
        assert!(Arc::ptr_eq(&first.storage_bytes, &second.storage_bytes));
    }

    #[test]
    fn waterfall_storage_revision_changes_for_new_frames_but_identity_stays_stable() {
        let first_frame = test_frame();
        let second_frame = test_frame();
        let plot = SpectrogramWidget::plot_rect(bounds());
        let first = waterfall_shader_descriptor(first_frame.as_ref(), plot, 2.5);
        let second = waterfall_shader_descriptor(second_frame.as_ref(), plot, 2.5);

        assert_ne!(first_frame.gpu_revision(), second_frame.gpu_revision());
        assert_ne!(first.storage_revision, second.storage_revision);
        assert_eq!(first.storage_revision, first_frame.gpu_revision());
        assert_eq!(second.storage_revision, second_frame.gpu_revision());
        assert_eq!(first.storage_identity, second.storage_identity);
    }

    #[test]
    fn waterfall_storage_identity_changes_with_height_and_history_scale() {
        let plot = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(100.0, 72.0));
        let taller_plot = Rect::from_min_size(Point::new(0.0, 0.0), Vector2::new(100.0, 73.0));
        let base = waterfall_storage_identity(plot, 1.0);

        assert_ne!(base, waterfall_storage_identity(taller_plot, 1.0));
        assert_ne!(base, waterfall_storage_identity(plot, 2.0));
        assert_eq!(base, waterfall_storage_identity(plot, 0.5));
        assert_eq!(
            waterfall_storage_identity(plot, 99.0),
            waterfall_storage_identity(plot, super::MAX_HISTORY_SCALE)
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
    fn retained_spectrum_uses_ordered_gpu_surfaces_with_shared_direct_revisioned_payload() {
        let frame = test_frame();
        let primitives = paint(SpectrogramWidget::new(
            Arc::clone(&frame),
            LiveSpectrogramMode::Spectrum,
        ));
        let surfaces = gpu_surfaces(&primitives);
        assert_eq!(surfaces.len(), 2);
        assert_eq!(surfaces[0].key, SPECTRUM_AREA_SURFACE_KEY);
        assert_eq!(surfaces[1].key, SPECTRUM_RIBBON_SURFACE_KEY);
        assert_ne!(surfaces[0].key, surfaces[1].key);
        assert_ne!(surfaces[0].key, super::WATERFALL_SURFACE_KEY);

        let area_descriptor = custom_descriptor(surfaces[0]);
        let ribbon_descriptor = custom_descriptor(surfaces[1]);
        for (surface, descriptor) in [
            (surfaces[0], area_descriptor),
            (surfaces[1], ribbon_descriptor),
        ] {
            assert_eq!(surface.rect, SpectrogramWidget::plot_rect(bounds()));
            assert_eq!(surface.revision, frame.gpu_revision());
            assert_eq!(descriptor.shader_key, SPECTRUM_SHADER_KEY);
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
        }
        assert!(Arc::ptr_eq(
            &area_descriptor.storage_bytes,
            &ribbon_descriptor.storage_bytes
        ));
        assert_ne!(
            area_descriptor.storage_identity,
            ribbon_descriptor.storage_identity
        );
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
        let surfaces = gpu_surfaces(&primitives);
        assert_eq!(surfaces.len(), 2);
        let area_descriptor = custom_descriptor(surfaces[0]);
        let ribbon_descriptor = custom_descriptor(surfaces[1]);

        assert_eq!(
            u32::from_le_bytes(
                area_descriptor.uniform_bytes[0..4]
                    .try_into()
                    .expect("point count bytes")
            ),
            LIVE_SPECTRUM_POINT_COUNT as u32
        );
        assert_eq!(
            uniform_f32(&area_descriptor.uniform_bytes, 4),
            (SPECTRUM_RIBBON_WIDTH * 0.5) / plot.height()
        );
        assert_eq!(
            uniform_f32(&area_descriptor.uniform_bytes, 8),
            SPECTRUM_AREA_ALPHA as f32 / u8::MAX as f32
        );
        assert_eq!(
            u32::from_le_bytes(
                area_descriptor.uniform_bytes[12..16]
                    .try_into()
                    .expect("area render kind bytes")
            ),
            SPECTRUM_AREA_RENDER_KIND
        );
        assert_eq!(
            u32::from_le_bytes(
                ribbon_descriptor.uniform_bytes[12..16]
                    .try_into()
                    .expect("ribbon render kind bytes")
            ),
            SPECTRUM_RIBBON_RENDER_KIND
        );
        assert_eq!(
            &area_descriptor.uniform_bytes[0..12],
            &ribbon_descriptor.uniform_bytes[0..12]
        );
        assert_eq!(
            &area_descriptor.uniform_bytes[16..],
            &ribbon_descriptor.uniform_bytes[16..]
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
                uniform_f32(&area_descriptor.uniform_bytes, 16 + index * 4),
                channel as f32 / u8::MAX as f32
            );
        }
    }

    #[test]
    fn spectrum_surfaces_restore_area_grid_ribbon_border_order() {
        let primitives = paint(SpectrogramWidget::new(
            test_frame(),
            LiveSpectrogramMode::Spectrum,
        ));
        let plot = SpectrogramWidget::plot_rect(bounds());
        let area_index = primitives
            .iter()
            .position(|primitive| {
                matches!(
                    primitive,
                    PaintPrimitive::GpuSurface(surface)
                        if surface.key == SPECTRUM_AREA_SURFACE_KEY
                )
            })
            .expect("spectrum area surface");
        let grid_index = primitives
            .iter()
            .position(|primitive| matches!(primitive, PaintPrimitive::StrokePolyline(_)))
            .expect("normal spectrum grid");
        let ribbon_index = primitives
            .iter()
            .position(|primitive| {
                matches!(
                    primitive,
                    PaintPrimitive::GpuSurface(surface)
                        if surface.key == SPECTRUM_RIBBON_SURFACE_KEY
                )
            })
            .expect("spectrum ribbon surface");
        let border_index = primitives
            .iter()
            .rposition(|primitive| {
                matches!(primitive, PaintPrimitive::StrokeRect(border) if border.rect == plot)
            })
            .expect("spectrum border");

        assert!(area_index < grid_index && grid_index < ribbon_index);
        assert!(ribbon_index < border_index);
        assert_eq!(border_index, primitives.len() - 1);
        assert_eq!(
            gpu_surfaces(&primitives)
                .into_iter()
                .map(|surface| surface.key)
                .collect::<Vec<_>>(),
            vec![SPECTRUM_AREA_SURFACE_KEY, SPECTRUM_RIBBON_SURFACE_KEY]
        );
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
    fn overlay_paint_uses_bounded_replayable_shapes_for_both_modes() {
        let bounds = Rect::from_min_size(Point::new(18.0, 24.0), Vector2::new(720.0, 102.0));
        let plot = SpectrogramWidget::plot_rect(bounds);
        for mode in [
            LiveSpectrogramMode::Waterfall,
            LiveSpectrogramMode::Spectrum,
        ] {
            let mut primitives = Vec::new();
            paint_overlay(
                test_frame(),
                mode,
                DEFAULT_HISTORY_SCALE,
                bounds,
                &mut primitives,
                &ThemeTokens::default(),
            );

            assert!(matches!(
                primitives.first(),
                Some(PaintPrimitive::ClipStart(clip)) if clip.rect == bounds
            ));
            assert!(matches!(
                primitives.last(),
                Some(PaintPrimitive::ClipEnd(clip))
                    if clip.node_id == super::LIVE_SPECTROGRAM_OVERLAY_WIDGET_ID
            ));
            assert!(matches!(
                primitives.get(1),
                Some(PaintPrimitive::FillRect(fill)) if fill.rect == bounds
            ));
            assert!(matches!(
                primitives.get(2),
                Some(PaintPrimitive::FillRect(fill)) if fill.rect == plot
            ));

            let body = &primitives[1..primitives.len() - 1];
            assert!(!body.iter().any(|primitive| {
                matches!(
                    primitive,
                    PaintPrimitive::GpuSurface(_)
                        | PaintPrimitive::Image(_)
                        | PaintPrimitive::CustomSurface(_)
                        | PaintPrimitive::StrokePolyline(_)
                )
            }));
            match mode {
                LiveSpectrogramMode::Waterfall => assert!(
                    body.iter()
                        .any(|primitive| matches!(primitive, PaintPrimitive::FillRectBatch(_)))
                ),
                LiveSpectrogramMode::Spectrum => {
                    assert!(
                        body.iter()
                            .any(|primitive| matches!(primitive, PaintPrimitive::FillPolygon(_)))
                    );
                    assert!(
                        !body
                            .iter()
                            .any(|primitive| matches!(primitive, PaintPrimitive::StrokePolygon(_)))
                    );
                }
            }
            let grid = body
                .iter()
                .position(|primitive| matches!(primitive, PaintPrimitive::FillRect(fill) if fill.rect != bounds && fill.rect != plot))
                .expect("overlay grid");
            let fill_polygons = body
                .iter()
                .enumerate()
                .filter_map(|(index, primitive)| {
                    matches!(primitive, PaintPrimitive::FillPolygon(_)).then_some(index)
                })
                .collect::<Vec<_>>();
            let area = fill_polygons.first().copied();
            let ribbon = fill_polygons.get(1).copied();
            let border = body
                .iter()
                .rposition(|primitive| {
                    matches!(primitive, PaintPrimitive::StrokeRect(stroke) if stroke.rect == plot)
                })
                .expect("overlay border");
            assert!(grid < border);
            if mode == LiveSpectrogramMode::Spectrum {
                let area = area.expect("overlay spectrum area");
                let ribbon = ribbon.expect("overlay spectrum ribbon");
                assert!(area < grid && grid < ribbon && ribbon < border);
                let PaintPrimitive::FillPolygon(ribbon) = &body[ribbon] else {
                    unreachable!();
                };
                assert_eq!(ribbon.points.len(), 2 * OVERLAY_MAX_SPECTRUM_POINTS);
                assert!(
                    ribbon
                        .points
                        .first()
                        .is_some_and(|point| point.y < plot.max.y)
                );
                assert!(ribbon.points[OVERLAY_MAX_SPECTRUM_POINTS].y < plot.max.y);
            }
            for primitive in body {
                match primitive {
                    PaintPrimitive::FillRect(fill) => {
                        if fill.rect != bounds {
                            assert!(fill.rect.min.x >= plot.min.x - f32::EPSILON);
                            assert!(fill.rect.max.x <= plot.max.x + f32::EPSILON);
                            assert!(fill.rect.min.y >= plot.min.y - f32::EPSILON);
                            assert!(fill.rect.max.y <= plot.max.y + f32::EPSILON);
                        }
                    }
                    PaintPrimitive::FillRectBatch(batch) => {
                        assert!(
                            batch.rects.len()
                                <= OVERLAY_MAX_WATERFALL_COLUMNS * OVERLAY_MAX_WATERFALL_ROWS
                        );
                        assert!(batch.rects.iter().all(|rect| {
                            rect.min.x >= plot.min.x - f32::EPSILON
                                && rect.max.x <= plot.max.x + f32::EPSILON
                                && rect.min.y >= plot.min.y - f32::EPSILON
                                && rect.max.y <= plot.max.y + f32::EPSILON
                        }));
                    }
                    PaintPrimitive::FillPolygon(fill) => {
                        assert!(fill.points.iter().all(|point| {
                            point.x >= plot.min.x - f32::EPSILON
                                && point.x <= plot.max.x + f32::EPSILON
                                && point.y >= plot.min.y - f32::EPSILON
                                && point.y <= plot.max.y + f32::EPSILON
                        }));
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn overlay_geometry_cache_reuses_and_invalidates_by_stable_inputs() {
        let bounds = bounds();
        let theme = ThemeTokens::default();
        let frame = test_frame();
        let mut cache = OverlayGeometryCache::default();
        let mut primitives = Vec::new();

        paint_overlay_cached(
            Arc::clone(&frame),
            LiveSpectrogramMode::Waterfall,
            DEFAULT_HISTORY_SCALE,
            bounds,
            &mut primitives,
            &theme,
            &mut cache,
        );
        let first = cache.primitives_ptr().expect("cached overlay geometry");
        let first_key = cache.key;
        primitives.clear();
        paint_overlay_cached(
            Arc::clone(&frame),
            LiveSpectrogramMode::Waterfall,
            DEFAULT_HISTORY_SCALE,
            bounds,
            &mut primitives,
            &theme,
            &mut cache,
        );
        assert_eq!(cache.primitives_ptr(), Some(first));

        primitives.clear();
        paint_overlay_cached(
            Arc::clone(&frame),
            LiveSpectrogramMode::Spectrum,
            DEFAULT_HISTORY_SCALE,
            bounds,
            &mut primitives,
            &theme,
            &mut cache,
        );
        let mode_changed = cache.primitives_ptr().expect("mode cache entry");
        assert_eq!(
            mode_changed, first,
            "key changes should reuse the top-level Vec"
        );
        assert_ne!(cache.key, first_key);

        primitives.clear();
        paint_overlay_cached(
            test_frame(),
            LiveSpectrogramMode::Spectrum,
            DEFAULT_HISTORY_SCALE,
            bounds,
            &mut primitives,
            &theme,
            &mut cache,
        );
        assert_eq!(cache.primitives_ptr(), Some(mode_changed));
        assert_ne!(cache.key, first_key);
    }

    #[test]
    fn overlay_geometry_cache_reuses_nested_waterfall_backing_across_revisions() {
        let bounds = bounds();
        let theme = ThemeTokens::default();
        let mut cache = OverlayGeometryCache::default();
        let mut primitives = Vec::new();

        paint_overlay_cached(
            test_frame_revision(1),
            LiveSpectrogramMode::Waterfall,
            DEFAULT_HISTORY_SCALE,
            bounds,
            &mut primitives,
            &theme,
            &mut cache,
        );
        let first_batch = primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRectBatch(batch) => Some(batch.rects.as_ptr()),
                _ => None,
            })
            .expect("waterfall batch");
        primitives.clear();

        paint_overlay_cached(
            test_frame_revision(2),
            LiveSpectrogramMode::Waterfall,
            DEFAULT_HISTORY_SCALE,
            bounds,
            &mut primitives,
            &theme,
            &mut cache,
        );
        let second_batch = primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRectBatch(batch) => Some(batch.rects.as_ptr()),
                _ => None,
            })
            .expect("waterfall batch after revision");
        assert_eq!(first_batch, second_batch);
    }

    #[test]
    fn overlay_geometry_cache_reuses_nested_spectrum_backing_across_revisions() {
        let bounds = bounds();
        let theme = ThemeTokens::default();
        let mut cache = OverlayGeometryCache::default();
        let mut primitives = Vec::new();

        paint_overlay_cached(
            test_frame_revision(1),
            LiveSpectrogramMode::Spectrum,
            DEFAULT_HISTORY_SCALE,
            bounds,
            &mut primitives,
            &theme,
            &mut cache,
        );
        let first_polygons = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillPolygon(fill) => Some(fill.points.as_ptr()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(first_polygons.len(), 2);
        primitives.clear();

        paint_overlay_cached(
            test_frame_revision(2),
            LiveSpectrogramMode::Spectrum,
            DEFAULT_HISTORY_SCALE,
            bounds,
            &mut primitives,
            &theme,
            &mut cache,
        );
        let second_polygons = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillPolygon(fill) => Some(fill.points.as_ptr()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(second_polygons, first_polygons);
    }

    #[test]
    fn overlay_geometry_cache_uses_cow_for_outstanding_waterfall_submission() {
        let bounds = bounds();
        let theme = ThemeTokens::default();
        let mut cache = OverlayGeometryCache::default();
        let mut primitives = Vec::new();

        paint_overlay_cached(
            test_frame_revision(1),
            LiveSpectrogramMode::Waterfall,
            DEFAULT_HISTORY_SCALE,
            bounds,
            &mut primitives,
            &theme,
            &mut cache,
        );
        let prior_submission = primitives.clone();
        let prior_batch = prior_submission
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRectBatch(batch) => Some(batch.rects.as_ptr()),
                _ => None,
            })
            .expect("prior waterfall batch");
        primitives.clear();

        paint_overlay_cached(
            test_frame_revision(2),
            LiveSpectrogramMode::Waterfall,
            DEFAULT_HISTORY_SCALE,
            bounds,
            &mut primitives,
            &theme,
            &mut cache,
        );
        let refreshed_batch = primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRectBatch(batch) => Some(batch.rects.as_ptr()),
                _ => None,
            })
            .expect("refreshed waterfall batch");
        assert_ne!(prior_batch, refreshed_batch);
        assert!(prior_submission.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRectBatch(batch) if !batch.rects.is_empty())
        }));
    }

    #[test]
    fn overlay_waterfall_keeps_newest_row_when_rows_are_capped() {
        let bounds = Rect::from_min_size(Point::new(18.0, 24.0), Vector2::new(420.0, 140.0));
        let plot = SpectrogramWidget::plot_rect(bounds);
        let row_count = 128;
        let mut values = vec![0_u8; row_count * LIVE_SPECTROGRAM_BAND_COUNT];
        values[(row_count - 1) * LIVE_SPECTROGRAM_BAND_COUNT..].fill(u8::MAX);
        let frame = Arc::new(
            LiveSpectrogramFrame::from_values(
                0,
                0,
                1,
                48_000,
                row_count,
                Arc::from(values.into_boxed_slice()),
                Arc::from(vec![0_u8; LIVE_SPECTRUM_POINT_COUNT].into_boxed_slice()),
            )
            .expect("valid capped waterfall frame"),
        );
        let mut primitives = Vec::new();
        paint_overlay(
            frame,
            LiveSpectrogramMode::Waterfall,
            DEFAULT_HISTORY_SCALE,
            bounds,
            &mut primitives,
            &ThemeTokens::default(),
        );

        assert!(primitives.iter().any(|primitive| {
            let PaintPrimitive::FillRectBatch(batch) = primitive else {
                return false;
            };
            batch.color == PALETTE[PALETTE.len() - 1]
                && batch
                    .rects
                    .iter()
                    .any(|rect| (rect.max.y - plot.max.y).abs() < f32::EPSILON)
        }));
    }

    #[test]
    fn spectrum_shader_declares_log_linear_interpolation_and_clamped_ribbon() {
        assert!(SPECTRUM_SHADER_WGSL.contains("spectrum[sample_index / 4u]"));
        assert!(SPECTRUM_SHADER_WGSL.contains("sample_position"));
        assert!(SPECTRUM_SHADER_WGSL.contains("lower + (upper - lower) * blend"));
        assert!(SPECTRUM_SHADER_WGSL.contains("half_ribbon_width"));
        assert!(SPECTRUM_SHADER_WGSL.contains("render_kind"));
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
        let first_surfaces = gpu_surfaces(&first_primitives);
        let second_primitives = paint(SpectrogramWidget::new(
            Arc::clone(&second),
            LiveSpectrogramMode::Spectrum,
        ));
        let second_surfaces = gpu_surfaces(&second_primitives);

        assert_ne!(first.gpu_revision(), second.gpu_revision());
        assert_eq!(first_surfaces.len(), 2);
        assert_eq!(second_surfaces.len(), 2);
        for (first_surface, second_surface) in first_surfaces.iter().zip(second_surfaces) {
            assert_eq!(first_surface.key, second_surface.key);
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

#include <metal_stdlib>
using namespace metal;

// One GPU thread handles one spectrogram cell. Every cell expands to 2x2
// luma pixels and one matching NV12 chroma sample, so each visual bin remains
// crisp while GPUI imports the IOSurface directly.
kernel void cadence_spectrogram(
    device const uchar *magnitudes [[buffer(0)]],
    constant uint &row_count [[buffer(1)]],
    texture2d<float, access::write> y_plane [[texture(0)]],
    texture2d<float, access::write> uv_plane [[texture(1)]],
    uint2 cell [[thread_position_in_grid]]) {
    constexpr uint width = 240;
    constexpr uint height = 128;
    if (cell.x >= width || cell.y >= height) return;

    const uint history_start = width - row_count;
    const uint band = height - 1 - cell.y;
    const uchar magnitude = cell.x < history_start
        ? uchar(0)
        : magnitudes[(cell.x - history_start) * height + band];

    // Modern terminal palette anchors (black -> grey -> crimson -> signal red -> white),
    // interpolated in linear RGB by the shader.
    constexpr float3 stops[9] = {
        float3(5.0, 5.0, 5.0), float3(24.0, 24.0, 24.0),
        float3(52.0, 52.0, 52.0), float3(92.0, 28.0, 30.0),
        float3(148.0, 24.0, 28.0), float3(208.0, 30.0, 34.0),
        float3(236.0, 66.0, 66.0), float3(248.0, 158.0, 158.0),
        float3(255.0, 255.0, 255.0)
    };
    const float position = float(magnitude) * 8.0 / 255.0;
    const uint lower = min(uint(position), 7u);
    const float3 rgb = round(mix(stops[lower], stops[lower + 1], position - float(lower))) / 255.0;

    // Invert GPUI's ycbcr_to_RGB matrix from shaders.wgsl (BT.601-style,
    // full range): R=Y+1.402(Cr-.5), G=Y-.3441(Cb-.5)-.7141(Cr-.5),
    // B=Y+1.772(Cb-.5). All channels are normalized to [0,1].
    const float y = dot(rgb, float3(0.299, 0.587, 0.114));
    const float cb = clamp((rgb.b - y) / 1.772 + 0.5, 0.0, 1.0);
    const float cr = clamp((rgb.r - y) / 1.402 + 0.5, 0.0, 1.0);
    const uint2 pixel = cell * 2;
    y_plane.write(float4(y), pixel);
    y_plane.write(float4(y), pixel + uint2(1, 0));
    y_plane.write(float4(y), pixel + uint2(0, 1));
    y_plane.write(float4(y), pixel + uint2(1, 1));
    uv_plane.write(float4(cb, cr, 0.0, 1.0), cell);
}

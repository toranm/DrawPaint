// Applies the stroke in progress to its layer: paint over it, or erase it.

struct Style {
    // Straight sRGB-encoded brush colour.
    color: vec3<f32>,
    // Ceiling for the stroke's opacity.
    opacity: f32,
    erase: u32,
}

@group(0) @binding(0) var layer: texture_2d<f32>;
@group(0) @binding(1) var coverage: texture_2d<f32>;
@group(0) @binding(2) var<uniform> style: Style;

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let below = textureLoad(layer, pixel, 0);
    let alpha = clamp(textureLoad(coverage, pixel, 0).r, 0.0, 1.0) * style.opacity;
    if style.erase != 0u {
        return below * (1.0 - alpha);
    }
    return vec4(style.color * alpha, alpha) + below * (1.0 - alpha);
}

// Composites one layer over the layers below it. All colours are
// premultiplied; blend functions work on straight colour.

struct Params {
    opacity: f32,
    mode: u32,
}

const MODE_NORMAL: u32 = 0u;
const MODE_MULTIPLY: u32 = 1u;
const MODE_SCREEN: u32 = 2u;
const MODE_OVERLAY: u32 = 3u;
const MODE_ADD: u32 = 4u;

@group(0) @binding(0) var backdrop: texture_2d<f32>;
@group(0) @binding(1) var layer: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: Params;

fn straight(color: vec4<f32>) -> vec3<f32> {
    if color.a <= 0.0 {
        return vec3(0.0);
    }
    return color.rgb / color.a;
}

fn blend(below: vec3<f32>, above: vec3<f32>, mode: u32) -> vec3<f32> {
    switch mode {
        case MODE_MULTIPLY: {
            return below * above;
        }
        case MODE_SCREEN: {
            return below + above - below * above;
        }
        case MODE_OVERLAY: {
            let dark = 2.0 * below * above;
            let light = 1.0 - 2.0 * (1.0 - below) * (1.0 - above);
            return select(light, dark, below <= vec3(0.5));
        }
        case MODE_ADD: {
            return min(below + above, vec3(1.0));
        }
        default: {
            return above;
        }
    }
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let below = textureLoad(backdrop, pixel, 0);
    let above = textureLoad(layer, pixel, 0) * params.opacity;
    let mixed = blend(straight(below), straight(above), params.mode);
    // Where only one of the two has coverage it shows unchanged; the blend
    // function applies where both do.
    let rgb = above.rgb * (1.0 - below.a) + mixed * (above.a * below.a) + below.rgb * (1.0 - above.a);
    return vec4(rgb, above.a + below.a * (1.0 - above.a));
}

// Stamps round brush dabs into the stroke coverage buffer, one instanced
// quad each. The blend state accumulates coverage as c' = s + c * (1 - s).

struct Canvas {
    size: vec2<f32>,
}

@group(0) @binding(0) var<uniform> canvas: Canvas;

struct Instance {
    @location(0) center: vec2<f32>,
    @location(1) radius: f32,
    @location(2) hardness: f32,
    @location(3) flow: f32,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // Offset from the dab centre in canvas pixels.
    @location(0) offset: vec2<f32>,
    @location(1) @interpolate(flat) radius: f32,
    @location(2) @interpolate(flat) hardness: f32,
    @location(3) @interpolate(flat) flow: f32,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32, instance: Instance) -> VertexOutput {
    // Two triangles covering [-1, 1]^2.
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    );
    // One extra pixel leaves room for the antialiased edge.
    let offset = corners[index] * (instance.radius + 1.0);
    let pixel = instance.center + offset;
    let ndc = vec2(pixel.x / canvas.size.x * 2.0 - 1.0, 1.0 - pixel.y / canvas.size.y * 2.0);

    var out: VertexOutput;
    out.position = vec4(ndc, 0.0, 1.0);
    out.offset = offset;
    out.radius = instance.radius;
    out.hardness = instance.hardness;
    out.flow = instance.flow;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let distance = length(in.offset);
    // The falloff starts at `hardness * radius` and always spans at least one
    // pixel, so hard brushes stay antialiased.
    let soft_width = max(in.radius * (1.0 - in.hardness), 1.0);
    let outer = in.radius + 0.5;
    let coverage = 1.0 - smoothstep(outer - soft_width, outer, distance);
    return vec4(coverage * in.flow);
}

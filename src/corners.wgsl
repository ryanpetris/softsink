struct Corner {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

// Covers the viewport, a square whose side is the corner radius, with uv
// running from 0 at the window edge to 1 at the arc's centre. The second
// instance mirrors uv so one shader serves both top corners.
@vertex
fn vertex(@builtin(vertex_index) index: u32, @builtin(instance_index) mirror: u32) -> Corner {
    let uv = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    let position = vec4f(uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0);
    return Corner(position, select(uv, vec2f(1.0 - uv.x, uv.y), mirror == 1u));
}

// Returns the share of the pixel inside the arc. The blend state multiplies
// the colour and alpha already drawn by it.
@fragment
fn fragment(input: Corner) -> @location(0) vec4f {
    let distance = length(1.0 - input.uv);
    return vec4f(0.0, 0.0, 0.0, clamp(0.5 + (1.0 - distance) / fwidth(distance), 0.0, 1.0));
}

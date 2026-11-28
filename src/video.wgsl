struct Vertex {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

struct Video {
    kr: f32,
    kb: f32,
    y_offset: f32,
    y_scale: f32,
    chroma_scale: f32,
    kind: u32,
    width: u32,
    height: u32,
    uv_rows: vec4f,
    crop: vec4f,
}

@group(0) @binding(0) var luma: texture_2d<f32>;
@group(0) @binding(1) var chroma: texture_2d<f32>;
@group(0) @binding(2) var cr_plane: texture_2d<f32>;
@group(0) @binding(3) var linear_sampler: sampler;
@group(0) @binding(4) var<uniform> video: Video;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let uv = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    let centered = video.crop.xy + uv * video.crop.zw - 0.5;
    let source = vec2f(dot(video.uv_rows.xy, centered), dot(video.uv_rows.zw, centered)) + 0.5;
    return Vertex(vec4f(uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0), source);
}

fn packed_luma(pixel: vec2i) -> f32 {
    let pair = textureLoad(luma, vec2i(pixel.x / 2, pixel.y), 0);
    return select(pair.r, pair.b, (pixel.x & 1) == 1);
}

fn yuyv_luma(uv: vec2f) -> f32 {
    // Filtering packed YUYV directly would mix even and odd luma separately.
    let last = vec2i(i32(video.width) - 1, i32(video.height) - 1);
    let position = uv * vec2f(f32(video.width), f32(video.height)) - 0.5;
    let first = vec2i(floor(position));
    let left = clamp(first, vec2i(0), last);
    let right = clamp(first + 1, vec2i(0), last);
    let weight = fract(position);
    let top = mix(packed_luma(left), packed_luma(vec2i(right.x, left.y)), weight.x);
    let bottom = mix(packed_luma(vec2i(left.x, right.y)), packed_luma(right), weight.x);
    return mix(top, bottom, weight.y);
}

@fragment
fn fragment(input: Vertex) -> @location(0) vec4f {
    var y: f32;
    var uv: vec2f;
    if video.kind == 2u {
        y = yuyv_luma(input.uv);
        uv = textureSampleLevel(luma, linear_sampler, input.uv, 0.0).ga;
    } else {
        y = textureSampleLevel(luma, linear_sampler, input.uv, 0.0).r;
        if video.kind == 1u {
            uv = textureSampleLevel(chroma, linear_sampler, input.uv, 0.0).rg;
        } else {
            uv = vec2f(
                textureSampleLevel(chroma, linear_sampler, input.uv, 0.0).r,
                textureSampleLevel(cr_plane, linear_sampler, input.uv, 0.0).r,
            );
        }
    }

    y = (y - video.y_offset) * video.y_scale;
    let cbcr = (uv - vec2f(128.0 / 255.0)) * video.chroma_scale;
    let kg = 1.0 - video.kr - video.kb;
    let r = 2.0 * (1.0 - video.kr) * cbcr.y;
    let b = 2.0 * (1.0 - video.kb) * cbcr.x;
    let rgb = vec3f(y + r, y - (video.kr * r + video.kb * b) / kg, y + b);
    return vec4f(clamp(rgb, vec3f(0.0), vec3f(1.0)), 1.0);
}

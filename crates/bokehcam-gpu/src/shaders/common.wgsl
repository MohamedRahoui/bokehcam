// Shared by every pass: a full-screen triangle, sixteen float parameters,
// and one linear clamping sampler. Each pass declares its textures from
// binding 2 upward and documents which parameter slots it reads.

struct Params { v: array<vec4f, 4> }

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4f,
    // (0, 0) is the top-left corner of the target.
    @location(0) uv: vec2f,
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return VsOut(vec4f(p * 2.0 - 1.0, 0.0, 1.0), vec2f(p.x, 1.0 - p.y));
}

const LUMA = vec3f(0.299, 0.587, 0.114);

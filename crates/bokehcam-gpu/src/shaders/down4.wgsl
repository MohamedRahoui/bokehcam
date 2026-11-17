// Exact 4x box reduction: four bilinear taps, each averaging a 2x2 block.
//   v[0].xy   one source texel, in uv
@group(0) @binding(2) var src: texture_2d<f32>;

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    let t = P.v[0].xy;
    let sum = textureSampleLevel(src, samp, in.uv + vec2f(-t.x, -t.y), 0.0)
            + textureSampleLevel(src, samp, in.uv + vec2f(t.x, -t.y), 0.0)
            + textureSampleLevel(src, samp, in.uv + vec2f(-t.x, t.y), 0.0)
            + textureSampleLevel(src, samp, in.uv + vec2f(t.x, t.y), 0.0);
    return sum * 0.25;
}

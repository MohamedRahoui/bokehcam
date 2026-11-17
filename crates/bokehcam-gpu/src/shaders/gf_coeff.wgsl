// Guided filter, step 2: from the local means of (I, p, I*p, I*I), fit
// p = a*I + b around every pixel. The cutoff ramp is folded into (a, b) so
// the final full-resolution step is a single multiply-add and clamp.
//   v[0].x    eps (edge sensitivity)
//   v[0].y    cutoff_low
//   v[0].z    cutoff_high
@group(0) @binding(2) var means: texture_2d<f32>;

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    let m = textureLoad(means, vec2i(in.pos.xy), 0);
    let variance = m.w - m.x * m.x;
    let covariance = m.z - m.x * m.y;
    let a = covariance / (variance + P.v[0].x);
    let b = m.y - a * m.x;
    let gain = 1.0 / (P.v[0].z - P.v[0].y);
    return vec4f(a * gain, (b - P.v[0].y) * gain, 0.0, 1.0);
}

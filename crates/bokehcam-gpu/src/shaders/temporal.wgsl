// Motion-adaptive smoothing of the person mask over time. Small changes
// (model noise, visible as shimmer) are damped; large changes (movement)
// pass through at once, so the mask never trails a moving person.
//   v[0].x    smoothing, 0..1
//   v[0].y    change treated as pure noise
//   v[0].z    change treated as pure motion
//   v[0].w    1 = a previous mask exists
//   v[1].xy   one mask texel, in uv
@group(0) @binding(2) var current: texture_2d<f32>;
@group(0) @binding(3) var previous: texture_2d<f32>;

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    let cur = textureSampleLevel(current, samp, in.uv, 0.0).r;
    if P.v[0].w < 0.5 { return vec4f(cur, 0.0, 0.0, 1.0); }
    let prev = textureSampleLevel(previous, samp, in.uv, 0.0).r;
    // A moving outline drags its 3x3 neighbourhood along.
    var motion = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let uv = in.uv + vec2f(f32(x), f32(y)) * P.v[1].xy;
            let change = abs(textureSampleLevel(current, samp, uv, 0.0).r
                           - textureSampleLevel(previous, samp, uv, 0.0).r);
            motion = max(motion, clamp((change - P.v[0].y) / (P.v[0].z - P.v[0].y), 0.0, 1.0));
        }
    }
    let follow = (1.0 - P.v[0].x) + P.v[0].x * motion;
    return vec4f(prev + follow * (cur - prev), 0.0, 0.0, 1.0);
}

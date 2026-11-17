// Crop, scale and mirror the camera frame to the output size.
//   v[0]      crop rectangle in source uv: x, y, width, height
//   v[1].x    1 = mirror horizontally
//   v[1].y    1 = shrinking: average four taps (covers up to 4x reduction)
//   v[1].zw   a quarter of an output pixel, in source uv
@group(0) @binding(2) var src: texture_2d<f32>;

fn tap(uv: vec2f) -> vec3f {
    return textureSampleLevel(src, samp, uv, 0.0).rgb;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    var u = in.uv;
    if P.v[1].x > 0.5 { u.x = 1.0 - u.x; }
    let uv = P.v[0].xy + u * P.v[0].zw;
    if P.v[1].y < 0.5 { return vec4f(tap(uv), 1.0); }
    let d = P.v[1].zw;
    let sum = tap(uv + vec2f(-d.x, -d.y)) + tap(uv + vec2f(d.x, -d.y))
            + tap(uv + vec2f(-d.x, d.y)) + tap(uv + vec2f(d.x, d.y));
    return vec4f(sum * 0.25, 1.0);
}

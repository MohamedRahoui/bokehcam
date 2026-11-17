// Guided filter, step 1: per-pixel products of the guide I (picture
// luminance) and the mask p. Output: (I, p, I*p, I*I).
//   v[0].x    grow (+) or shrink (-) radius for the mask, in target pixels
//   v[0].zw   one target texel, in uv
@group(0) @binding(2) var picture: texture_2d<f32>;
@group(0) @binding(3) var mask: texture_2d<f32>;

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    let i = dot(textureSampleLevel(picture, samp, in.uv, 0.0).rgb, LUMA);
    var p = textureSampleLevel(mask, samp, in.uv, 0.0).r;
    let r = i32(round(abs(P.v[0].x)));
    if r > 0 {
        let grow = P.v[0].x > 0.0;
        for (var y = -r; y <= r; y++) {
            for (var x = -r; x <= r; x++) {
                if x * x + y * y > r * r { continue; }
                let s = textureSampleLevel(mask, samp, in.uv + vec2f(f32(x), f32(y)) * P.v[0].zw, 0.0).r;
                if grow { p = max(p, s); } else { p = min(p, s); }
            }
        }
    }
    return vec4f(i, p, i * p, i * i);
}

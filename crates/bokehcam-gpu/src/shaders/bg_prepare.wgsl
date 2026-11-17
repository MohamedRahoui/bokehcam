// Background blur, step 1: weight every pixel by "how much background it
// is", so the blur ignores the person and no halo of their colours appears.
// Output: (rgb * weight, weight).
//   v[0].xy   one texel of the small picture, in uv
@group(0) @binding(2) var picture: texture_2d<f32>;
@group(0) @binding(3) var coeff: texture_2d<f32>;

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    // The person, slightly enlarged (5x5 maximum), to be safe at soft edges.
    var person = 0.0;
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            let uv = in.uv + vec2f(f32(x), f32(y)) * P.v[0].xy;
            let i = dot(textureSampleLevel(picture, samp, uv, 0.0).rgb, LUMA);
            let ab = textureSampleLevel(coeff, samp, uv, 0.0).rg;
            person = max(person, clamp(ab.x * i + ab.y, 0.0, 1.0));
        }
    }
    let weight = 1.0 - person;
    return vec4f(textureSampleLevel(picture, samp, in.uv, 0.0).rgb * weight, weight);
}

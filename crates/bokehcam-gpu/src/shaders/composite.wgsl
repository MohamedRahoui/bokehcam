// Final picture: person over blurred background, then colour.
//   v[0]      brightness, contrast, saturation, 1 / gamma
//   v[1].x    1 = blur the background, 0 = pass the picture through
@group(0) @binding(2) var picture: texture_2d<f32>;
@group(0) @binding(3) var coeff: texture_2d<f32>;
@group(0) @binding(4) var blurred: texture_2d<f32>;

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    var rgb = textureSampleLevel(picture, samp, in.uv, 0.0).rgb;
    if P.v[1].x > 0.5 {
        // The guided-filter model, applied to the full-resolution luminance:
        // this is what gives the outline full-resolution edges.
        let ab = textureSampleLevel(coeff, samp, in.uv, 0.0).rg;
        let alpha = clamp(ab.x * dot(rgb, LUMA) + ab.y, 0.0, 1.0);
        let bg = textureSampleLevel(blurred, samp, in.uv, 0.0);
        rgb = mix(bg.rgb / max(bg.a, 0.001), rgb, alpha);
    }
    rgb = mix(vec3f(dot(rgb, LUMA)), rgb, P.v[0].z);
    rgb = (rgb - 0.5) * P.v[0].y + 0.5 + P.v[0].x;
    rgb = pow(clamp(rgb, vec3f(0.0), vec3f(1.0)), vec3f(P.v[0].w));
    return vec4f(rgb, 1.0);
}

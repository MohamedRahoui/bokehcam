// One direction of a Gaussian blur.
//   v[0].xy   one texel along the blur direction, in uv
//   v[0].z    sigma in texels
@group(0) @binding(2) var src: texture_2d<f32>;

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    let sigma = max(P.v[0].z, 0.01);
    let r = i32(ceil(sigma * 3.0));
    var sum = vec4f(0.0);
    var total = 0.0;
    for (var k = -r; k <= r; k++) {
        let w = exp(-f32(k * k) / (2.0 * sigma * sigma));
        sum += textureSampleLevel(src, samp, in.uv + P.v[0].xy * f32(k), 0.0) * w;
        total += w;
    }
    return sum / total;
}

// Bilinear resample (model input, preview).
@group(0) @binding(2) var src: texture_2d<f32>;

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    return textureSampleLevel(src, samp, in.uv, 0.0);
}

// Pack the final picture as YUYV 4:2:2 (BT.601, limited range). The target
// is half as wide; each texel holds two pixels as (Y0, U, Y1, V), which is
// exactly the byte order the virtual camera expects.
@group(0) @binding(2) var picture: texture_2d<f32>;

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(in.pos.xy);
    let c0 = textureLoad(picture, vec2i(p.x * 2, p.y), 0).rgb;
    let c1 = textureLoad(picture, vec2i(p.x * 2 + 1, p.y), 0).rgb;
    let y = vec3f(0.257, 0.504, 0.098);
    let mid = (c0 + c1) * 0.5;
    let u = dot(mid, vec3f(-0.148, -0.291, 0.439)) + 128.0 / 255.0;
    let v = dot(mid, vec3f(0.439, -0.368, -0.071)) + 128.0 / 255.0;
    return vec4f(dot(c0, y) + 16.0 / 255.0, u, dot(c1, y) + 16.0 / 255.0, v);
}

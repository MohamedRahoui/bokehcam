// One direction of a box blur, on exact texel values (no filtering), so it
// works on 32-bit float targets.
//   v[0].xy   direction: (1, 0) or (0, 1)
//   v[0].z    radius in texels
@group(0) @binding(2) var src: texture_2d<f32>;

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    let size = vec2i(textureDimensions(src));
    let here = vec2i(in.pos.xy);
    let dir = vec2i(P.v[0].xy);
    let r = i32(P.v[0].z);
    var sum = vec4f(0.0);
    for (var k = -r; k <= r; k++) {
        sum += textureLoad(src, clamp(here + dir * k, vec2i(0), size - vec2i(1)), 0);
    }
    return sum / f32(2 * r + 1);
}

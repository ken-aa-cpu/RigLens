struct JfaUniforms {
    width: u32,
    height: u32,
    step_px: i32,
    _pad: u32,
};

@group(0) @binding(0) var tex_in: texture_2d<f32>;
@group(0) @binding(1) var jfa_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<uniform> u: JfaUniforms;

fn is_inside(coord: vec2<i32>, max_c: vec2<i32>) -> bool {
    let clamped = clamp(coord, vec2<i32>(0), max_c);
    return textureLoad(tex_in, clamped, 0).a > 0.15;
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= u.width || gid.y >= u.height) {
        return;
    }
    let pos = vec2<i32>(gid.xy);
    let max_c = vec2<i32>(i32(u.width) - 1, i32(u.height) - 1);
    let inside = is_inside(pos, max_c);
    let inside_f = select(0.0, 1.0, inside);

    let diff_l = is_inside(pos + vec2<i32>(-1, 0), max_c) != inside;
    let diff_r = is_inside(pos + vec2<i32>(1, 0), max_c) != inside;
    let diff_u = is_inside(pos + vec2<i32>(0, -1), max_c) != inside;
    let diff_d = is_inside(pos + vec2<i32>(0, 1), max_c) != inside;

    if (diff_l || diff_r || diff_u || diff_d) {
        textureStore(jfa_out, pos, vec4<f32>(f32(pos.x) + 0.5, f32(pos.y) + 0.5, inside_f, 1.0));
    } else {
        textureStore(jfa_out, pos, vec4<f32>(-1e6, -1e6, inside_f, 0.0));
    }
}

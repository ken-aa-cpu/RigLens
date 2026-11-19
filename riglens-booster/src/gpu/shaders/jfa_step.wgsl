struct JfaUniforms {
    width: u32,
    height: u32,
    step_px: i32,
    _pad: u32,
};

@group(0) @binding(0) var jfa_in: texture_2d<f32>;
@group(0) @binding(1) var jfa_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<uniform> u: JfaUniforms;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= u.width || gid.y >= u.height) {
        return;
    }
    let pos = vec2<i32>(gid.xy);
    let max_c = vec2<i32>(i32(u.width) - 1, i32(u.height) - 1);
    let self_val = textureLoad(jfa_in, pos, 0);
    let inside_f = self_val.z;
    let p = vec2<f32>(pos) + 0.5;

    var best_seed = self_val.xy;
    var best_valid = self_val.w;
    var best_d2: f32 = 1e20;
    if (best_valid > 0.5) {
        let d = best_seed - p;
        best_d2 = dot(d, d);
    }

    let s = u.step_px;
    for (var dy: i32 = -1; dy <= 1; dy = dy + 1) {
        for (var dx: i32 = -1; dx <= 1; dx = dx + 1) {
            if (dx == 0 && dy == 0) {
                continue;
            }
            let npos = pos + vec2<i32>(dx * s, dy * s);
            if (npos.x < 0 || npos.y < 0 || npos.x > max_c.x || npos.y > max_c.y) {
                continue;
            }
            let cand = textureLoad(jfa_in, npos, 0);
            if (cand.w > 0.5) {
                let d = cand.xy - p;
                let d2 = dot(d, d);
                if (d2 < best_d2) {
                    best_d2 = d2;
                    best_seed = cand.xy;
                    best_valid = 1.0;
                }
            }
        }
    }

    textureStore(jfa_out, pos, vec4<f32>(best_seed, inside_f, best_valid));
}

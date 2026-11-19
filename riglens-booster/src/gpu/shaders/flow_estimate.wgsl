struct FlowUniforms {
    dims: vec4<f32>,   // (width, height, prior_vx, prior_vy)
    params: vec4<f32>, // (angular_rad, search_radius, eye_open_min, 0)
};

@group(0) @binding(0) var tex_prev: texture_2d<f32>;
@group(0) @binding(1) var tex_curr: texture_2d<f32>;
@group(0) @binding(2) var flow_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(3) var<uniform> u: FlowUniforms;

fn load_pm(tex: texture_2d<f32>, coord: vec2<i32>, max_c: vec2<i32>) -> vec4<f32> {
    let c = textureLoad(tex, clamp(coord, vec2<i32>(0), max_c), 0);
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn patch_sad(pos: vec2<i32>, vel: vec2<i32>, max_c: vec2<i32>) -> f32 {
    var sad: f32 = 0.0;
    for (var dy: i32 = -2; dy <= 2; dy = dy + 2) {
        for (var dx: i32 = -2; dx <= 2; dx = dx + 2) {
            let off = vec2<i32>(dx, dy);
            let p_curr = load_pm(tex_curr, pos + off, max_c);
            let p_prev = load_pm(tex_prev, pos + off - vel, max_c);
            let d = abs(p_curr - p_prev);
            sad = sad + d.r + d.g + d.b + d.a * 1.5;
        }
    }
    return sad / 9.0;
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = u32(u.dims.x);
    let h = u32(u.dims.y);
    if (gid.x >= w || gid.y >= h) {
        return;
    }

    let pos = vec2<i32>(gid.xy);
    let max_c = vec2<i32>(i32(w) - 1, i32(h) - 1);
    let curr_px = textureLoad(tex_curr, pos, 0);

    if (curr_px.a <= 0.008) {
        textureStore(flow_out, pos, vec4<f32>(0.0, 0.0, 0.0, 0.0));
        return;
    }

    let rel = vec2<f32>(pos) + 0.5 - u.dims.xy * 0.5;
    let rot_v = vec2<f32>(-rel.y, rel.x) * u.params.x;
    let radius = i32(clamp(u.params.y, 4.0, 32.0));
    let prior = vec2<i32>(clamp(
        round(u.dims.zw + rot_v),
        vec2<f32>(f32(-radius)),
        vec2<f32>(f32(radius)),
    ));

    var best_v = prior;
    var best_sad = patch_sad(pos, prior, max_c);

    // Also evaluate zero-velocity candidate for stationary torso/background edges
    let zero_sad = patch_sad(pos, vec2<i32>(0, 0), max_c);
    if (zero_sad < best_sad) {
        best_sad = zero_sad;
        best_v = vec2<i32>(0, 0);
    }

    // Coarse-to-fine block matching (step = 4, then 2, then 1)
    let steps = array<i32, 3>(4, 2, 1);
    for (var si: i32 = 0; si < 3; si = si + 1) {
        let s = steps[si];
        let range = select(1, 2, s == 4);
        let center_v = best_v;
        for (var oy: i32 = -range; oy <= range; oy = oy + 1) {
            for (var ox: i32 = -range; ox <= range; ox = ox + 1) {
                if (ox == 0 && oy == 0) {
                    continue;
                }
                let cand = center_v + vec2<i32>(ox * s, oy * s);
                if (abs(cand.x) > radius || abs(cand.y) > radius) {
                    continue;
                }
                let sad = patch_sad(pos, cand, max_c);
                if (sad + 0.001 < best_sad) {
                    best_sad = sad;
                    best_v = cand;
                }
            }
        }
    }

    let blink_gate = mix(0.65, 1.0, clamp(u.params.z, 0.0, 1.0));
    let conf = clamp(exp(-best_sad * 2.2) * blink_gate, 0.0, 1.0);
    textureStore(flow_out, pos, vec4<f32>(f32(best_v.x), f32(best_v.y), conf, 1.0));
}

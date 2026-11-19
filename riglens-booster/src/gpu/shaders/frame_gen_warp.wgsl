struct FrameGenUniforms {
    dims: vec4<f32>, // (width, height, tau, 0)
};

@group(0) @binding(0) var tex_prev: texture_2d<f32>;
@group(0) @binding(1) var tex_curr: texture_2d<f32>;
@group(0) @binding(2) var flow_tex: texture_2d<f32>;
@group(0) @binding(3) var tex_out: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(4) var<uniform> u: FrameGenUniforms;

fn sample_pm_bilinear(tex: texture_2d<f32>, p: vec2<f32>, max_c: vec2<i32>) -> vec4<f32> {
    let clamped_p = clamp(p, vec2<f32>(0.0), vec2<f32>(max_c));
    let i0 = vec2<i32>(floor(clamped_p));
    let f = clamped_p - vec2<f32>(i0);

    let c00 = textureLoad(tex, clamp(i0, vec2<i32>(0), max_c), 0);
    let c10 = textureLoad(tex, clamp(i0 + vec2<i32>(1, 0), vec2<i32>(0), max_c), 0);
    let c01 = textureLoad(tex, clamp(i0 + vec2<i32>(0, 1), vec2<i32>(0), max_c), 0);
    let c11 = textureLoad(tex, clamp(i0 + vec2<i32>(1, 1), vec2<i32>(0), max_c), 0);

    let w00 = (1.0 - f.x) * (1.0 - f.y);
    let w10 = f.x * (1.0 - f.y);
    let w01 = (1.0 - f.x) * f.y;
    let w11 = f.x * f.y;

    let pm_rgb = c00.rgb * (c00.a * w00)
               + c10.rgb * (c10.a * w10)
               + c01.rgb * (c01.a * w01)
               + c11.rgb * (c11.a * w11);
    let a = c00.a * w00 + c10.a * w10 + c01.a * w01 + c11.a * w11;
    return vec4<f32>(pm_rgb, a);
}

fn eval_candidate(p: vec2<f32>, vel: vec2<f32>, tau: f32, max_c: vec2<i32>) -> vec4<f32> {
    let sp = sample_pm_bilinear(tex_prev, p - vel * tau, max_c);
    let sc = sample_pm_bilinear(tex_curr, p + vel * (1.0 - tau), max_c);
    let diff = abs(sp - sc);
    let err = diff.r + diff.g + diff.b + diff.a * 1.5;
    // Prefer matching opaque moving foreground over empty transparent background on error ties
    return vec4<f32>(err, min(sp.a, sc.a), 0.0, 0.0);
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
    let p = vec2<f32>(pos);
    let tau = clamp(u.dims.z, 0.0, 1.0);

    let f_here = textureLoad(flow_tex, pos, 0);
    var best_vel = select(vec2<f32>(0.0), f_here.xy, f_here.w > 0.5);
    let init_res = eval_candidate(p, best_vel, tau, max_c);
    var best_score = init_res.x - 0.15 * init_res.y;

    // Resolve moved foreground edges & uncovered background cleanly by testing neighbor flow candidates
    let probe_offs = array<vec2<i32>, 13>(
        vec2<i32>(0, 0),
        vec2<i32>(5, 0),
        vec2<i32>(-5, 0),
        vec2<i32>(0, 5),
        vec2<i32>(0, -5),
        vec2<i32>(10, 0),
        vec2<i32>(-10, 0),
        vec2<i32>(0, 10),
        vec2<i32>(0, -10),
        vec2<i32>(20, 0),
        vec2<i32>(-20, 0),
        vec2<i32>(0, 20),
        vec2<i32>(0, -20),
    );
    for (var i: i32 = 0; i < 13; i = i + 1) {
        let cand_pos = clamp(pos + probe_offs[i], vec2<i32>(0), max_c);
        let f_cand = textureLoad(flow_tex, cand_pos, 0);
        let v_cand = select(vec2<f32>(0.0), f_cand.xy, f_cand.w > 0.5);
        let cand_res = eval_candidate(p, v_cand, tau, max_c);
        let e = cand_res.x - 0.15 * cand_res.y + length(v_cand - best_vel) * 0.0005;
        if (e + 0.01 < best_score) {
            best_score = e;
            best_vel = v_cand;
        }
    }

    let sp = sample_pm_bilinear(tex_prev, p - best_vel * tau, max_c);
    let sc = sample_pm_bilinear(tex_curr, p + best_vel * (1.0 - tau), max_c);

    // Bidirectional premultiplied blend with occlusion fallback when one side reveals background
    var pm_rgb: vec3<f32>;
    var out_a: f32;
    if (abs(sp.a - sc.a) > 0.35 && best_score > 0.3) {
        if (tau < 0.5) {
            pm_rgb = sp.rgb;
            out_a = sp.a;
        } else {
            pm_rgb = sc.rgb;
            out_a = sc.a;
        }
    } else {
        pm_rgb = mix(sp.rgb, sc.rgb, tau);
        out_a = mix(sp.a, sc.a, tau);
    }

    out_a = clamp(out_a, 0.0, 1.0);
    if (out_a <= 0.004) {
        textureStore(tex_out, pos, vec4<f32>(0.0));
        return;
    }

    let straight_rgb = clamp(pm_rgb / max(out_a, 1e-4), vec3<f32>(0.0), vec3<f32>(1.0));
    textureStore(tex_out, pos, vec4<f32>(straight_rgb, out_a));
}

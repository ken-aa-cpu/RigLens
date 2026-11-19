struct UpscaleUniforms {
    dims: vec4<f32>,   // (src_w, src_h, dst_w, dst_h)
    params: vec4<f32>, // (sharpness, edge_thin, 0, 0)
};

@group(0) @binding(0) var tex_in: texture_2d<f32>;
@group(0) @binding(1) var tex_out: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> u: UpscaleUniforms;

fn catmull_weight(x: f32) -> vec4<f32> {
    let f = x;
    let f2 = f * f;
    let f3 = f2 * f;
    return vec4<f32>(
        -0.5 * f + f2 - 0.5 * f3,
        1.0 - 2.5 * f2 + 1.5 * f3,
        0.5 * f + 2.0 * f2 - 1.5 * f3,
        -0.5 * f2 + 0.5 * f3,
    );
}

fn luma(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dst_w = u32(u.dims.z);
    let dst_h = u32(u.dims.w);
    if (gid.x >= dst_w || gid.y >= dst_h) {
        return;
    }

    let max_c = vec2<i32>(i32(u.dims.x) - 1, i32(u.dims.y) - 1);
    let src_p = (vec2<f32>(gid.xy) + 0.5) * (u.dims.xy / u.dims.zw) - 0.5;
    let base = vec2<i32>(floor(src_p));
    let f = src_p - vec2<f32>(base);

    let wx = catmull_weight(f.x);
    let wy = catmull_weight(f.y);
    let wxa = array<f32, 4>(wx.x, wx.y, wx.z, wx.w);
    let wya = array<f32, 4>(wy.x, wy.y, wy.z, wy.w);

    var pm_sum = vec3<f32>(0.0);
    var a_sum: f32 = 0.0;
    var nb_min = vec3<f32>(1.0);
    var nb_max = vec3<f32>(0.0);
    var nb_any = false;

    for (var dy: i32 = -1; dy <= 2; dy = dy + 1) {
        let wy_v = wya[dy + 1];
        for (var dx: i32 = -1; dx <= 2; dx = dx + 1) {
            let wx_v = wxa[dx + 1];
            let w = wx_v * wy_v;
            let c = textureLoad(tex_in, clamp(base + vec2<i32>(dx, dy), vec2<i32>(0), max_c), 0);
            pm_sum = pm_sum + c.rgb * (c.a * w);
            a_sum = a_sum + c.a * w;

            if (dx >= 0 && dx <= 1 && dy >= 0 && dy <= 1 && c.a > 0.01) {
                nb_min = min(nb_min, c.rgb);
                nb_max = max(nb_max, c.rgb);
                nb_any = true;
            }
        }
    }

    let out_a = clamp(a_sum, 0.0, 1.0);
    if (out_a <= 0.004) {
        textureStore(tex_out, vec2<i32>(gid.xy), vec4<f32>(0.0));
        return;
    }

    var rgb = clamp(pm_sum / max(a_sum, 1e-4), vec3<f32>(0.0), vec3<f32>(1.0));
    if (nb_any) {
        rgb = clamp(rgb, nb_min, nb_max);
    }

    // Alpha-aware 2D anime line-art edge contrast sharpening & anti-ringing
    let sharp = clamp(u.params.x, 0.0, 2.0);
    if (sharp > 0.01) {
        let center_c = vec2<i32>(clamp(round(src_p), vec2<f32>(0.0), vec2<f32>(max_c)));
        let tl = textureLoad(tex_in, clamp(center_c + vec2<i32>(-1, -1), vec2<i32>(0), max_c), 0);
        let tc = textureLoad(tex_in, clamp(center_c + vec2<i32>( 0, -1), vec2<i32>(0), max_c), 0);
        let tr = textureLoad(tex_in, clamp(center_c + vec2<i32>( 1, -1), vec2<i32>(0), max_c), 0);
        let ml = textureLoad(tex_in, clamp(center_c + vec2<i32>(-1,  0), vec2<i32>(0), max_c), 0);
        let mr = textureLoad(tex_in, clamp(center_c + vec2<i32>( 1,  0), vec2<i32>(0), max_c), 0);
        let bl = textureLoad(tex_in, clamp(center_c + vec2<i32>(-1,  1), vec2<i32>(0), max_c), 0);
        let bc = textureLoad(tex_in, clamp(center_c + vec2<i32>( 0,  1), vec2<i32>(0), max_c), 0);
        let br = textureLoad(tex_in, clamp(center_c + vec2<i32>( 1,  1), vec2<i32>(0), max_c), 0);

        let l_tl = luma(tl.rgb) * tl.a;
        let l_tc = luma(tc.rgb) * tc.a;
        let l_tr = luma(tr.rgb) * tr.a;
        let l_ml = luma(ml.rgb) * ml.a;
        let l_mr = luma(mr.rgb) * mr.a;
        let l_bl = luma(bl.rgb) * bl.a;
        let l_bc = luma(bc.rgb) * bc.a;
        let l_br = luma(br.rgb) * br.a;

        let gx = (l_tr + 2.0 * l_mr + l_br) - (l_tl + 2.0 * l_ml + l_bl);
        let gy = (l_bl + 2.0 * l_bc + l_br) - (l_tl + 2.0 * l_tc + l_tr);
        let grad = sqrt(gx * gx + gy * gy);

        let avg_nb = (tl.rgb * tl.a + tc.rgb * tc.a + tr.rgb * tr.a
                    + ml.rgb * ml.a + mr.rgb * mr.a
                    + bl.rgb * bl.a + bc.rgb * bc.a + br.rgb * br.a)
                   / max(tl.a + tc.a + tr.a + ml.a + mr.a + bl.a + bc.a + br.a, 1e-4);

        let edge_gate = smoothstep(0.06, 0.45, grad) * smoothstep(0.25, 0.85, out_a);
        rgb = clamp(rgb + (rgb - avg_nb) * (sharp * 0.65 * edge_gate), vec3<f32>(0.0), vec3<f32>(1.0));
    }

    textureStore(tex_out, vec2<i32>(gid.xy), vec4<f32>(rgb, out_a));
}

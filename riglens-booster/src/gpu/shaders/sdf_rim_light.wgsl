struct RimUniforms {
    // x: width, y: height, z: rim_width_px, w: rim_intensity
    dims: vec4<f32>,
    left_rgb: vec4<f32>,
    right_rgb: vec4<f32>,
    top_rgb: vec4<f32>,
    bottom_rgb: vec4<f32>,
};

@group(0) @binding(0) var tex_color: texture_2d<f32>;
@group(0) @binding(1) var jfa_tex: texture_2d<f32>;
@group(0) @binding(2) var tex_out: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var sdf_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var<uniform> u: RimUniforms;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = u32(u.dims.x);
    let h = u32(u.dims.y);
    if (gid.x >= w || gid.y >= h) {
        return;
    }
    let pos = vec2<i32>(gid.xy);
    let max_c = vec2<i32>(i32(w) - 1, i32(h) - 1);
    let src = textureLoad(tex_color, pos, 0);
    let j = textureLoad(jfa_tex, pos, 0);
    let inside = j.z > 0.5;
    let p = vec2<f32>(pos) + 0.5;

    // Alpha gradient provides smooth outward normal right at distance ~ 0
    let ax_p = textureLoad(tex_color, clamp(pos + vec2<i32>(1, 0), vec2<i32>(0), max_c), 0).a;
    let ax_n = textureLoad(tex_color, clamp(pos + vec2<i32>(-1, 0), vec2<i32>(0), max_c), 0).a;
    let ay_p = textureLoad(tex_color, clamp(pos + vec2<i32>(0, 1), vec2<i32>(0), max_c), 0).a;
    let ay_n = textureLoad(tex_color, clamp(pos + vec2<i32>(0, -1), vec2<i32>(0), max_c), 0).a;
    let grad_out = vec2<f32>(ax_n - ax_p, ay_n - ay_p);

    var dist: f32 = 1e4;
    var n = vec2<f32>(0.0, -1.0);
    if (j.w > 0.5) {
        let to_seed = j.xy - p;
        dist = length(to_seed);
        if (dist > 0.75) {
            n = normalize(select(-to_seed, to_seed, inside));
        } else if (dot(grad_out, grad_out) > 1e-4) {
            n = normalize(grad_out);
        }
    } else if (dot(grad_out, grad_out) > 1e-4) {
        n = normalize(grad_out);
    }

    let sd = select(dist, -dist, inside);
    textureStore(sdf_out, pos, vec4<f32>(n.x, n.y, sd, src.a));

    if (src.a <= 0.004) {
        textureStore(tex_out, pos, vec4<f32>(0.0, 0.0, 0.0, 0.0));
        return;
    }

    let rim_w = max(u.dims.z, 1.0);
    let rim_k = u.dims.w;
    var out_rgb = src.rgb;
    if (inside && rim_k > 0.0) {
        let rim_mask = smoothstep(rim_w, 0.0, -sd);
        let wx_r = max(n.x, 0.0);
        let wx_l = max(-n.x, 0.0);
        let wy_b = max(n.y, 0.0);
        let wy_t = max(-n.y, 0.0);
        let w_sum = wx_r + wx_l + wy_b + wy_t + 1e-5;
        let rim_tint = (
            u.right_rgb.rgb * wx_r
            + u.left_rgb.rgb * wx_l
            + u.bottom_rgb.rgb * wy_b
            + u.top_rgb.rgb * wy_t
        ) / w_sum;

        // Screen-like additive rim highlight modulated cleanly by source alpha
        let boost = rim_tint * (rim_mask * rim_k * src.a);
        out_rgb = clamp(src.rgb + boost * (1.0 - src.rgb * 0.45), vec3<f32>(0.0), vec3<f32>(1.0));
    }

    textureStore(tex_out, pos, vec4<f32>(out_rgb, src.a));
}

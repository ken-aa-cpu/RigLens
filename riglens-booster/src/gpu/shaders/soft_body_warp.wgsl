struct WarpUniforms {
    width: u32,
    height: u32,
    zone_count: u32,
    enabled: u32,
};

struct SoftBodyZoneGpu {
    anchor_radius: vec4<f32>, // (anchor_x, anchor_y, radius_x, radius_y)
    params: vec4<f32>,        // (max_strain, active_flag, 0, 0)
    offsets: array<vec4<f32>, 41>,
};

@group(0) @binding(0) var tex_in: texture_2d<f32>;
@group(0) @binding(1) var tex_out: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> u: WarpUniforms;
@group(0) @binding(3) var<storage, read> zones: array<SoftBodyZoneGpu>;

fn read_zone_disp(zi: u32, idx: u32) -> vec2<f32> {
    let pair = idx / 2u;
    let v = zones[zi].offsets[pair];
    if ((idx & 1u) == 0u) {
        return v.xy;
    } else {
        return v.zw;
    }
}

fn sample_zone_bilinear(zi: u32, local: vec2<f32>) -> vec2<f32> {
    // local in [-1, 1] x [-1, 1] maps to 9x9 grid coordinates [0, 8] x [0, 8]
    let g = clamp((local * 0.5 + 0.5) * 8.0, vec2<f32>(0.0), vec2<f32>(8.0));
    let i0 = vec2<u32>(min(vec2<u32>(floor(g)), vec2<u32>(7u, 7u)));
    let i1 = i0 + vec2<u32>(1u, 1u);
    let f = g - vec2<f32>(i0);

    let d00 = read_zone_disp(zi, i0.y * 9u + i0.x);
    let d10 = read_zone_disp(zi, i0.y * 9u + i1.x);
    let d01 = read_zone_disp(zi, i1.y * 9u + i0.x);
    let d11 = read_zone_disp(zi, i1.y * 9u + i1.x);

    let top = mix(d00, d10, f.x);
    let bot = mix(d01, d11, f.x);
    return mix(top, bot, f.y);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= u.width || gid.y >= u.height) {
        return;
    }
    let dims_f = vec2<f32>(f32(u.width), f32(u.height));
    let uv = (vec2<f32>(gid.xy) + 0.5) / dims_f;

    var total_disp = vec2<f32>(0.0, 0.0);
    if (u.enabled != 0u) {
        let count = min(u.zone_count, 8u);
        for (var zi: u32 = 0u; zi < count; zi = zi + 1u) {
            if (zones[zi].params.y < 0.5) {
                continue;
            }
            let center = zones[zi].anchor_radius.xy;
            let rad = max(zones[zi].anchor_radius.zw, vec2<f32>(0.005));
            let local = (uv - center) / rad;
            let dist2 = dot(local, local);
            if (dist2 < 1.0) {
                let win = smoothstep(1.0, 0.0, sqrt(dist2));
                total_disp = total_disp + sample_zone_bilinear(zi, local) * win;
            }
        }
    }

    let src_uv = clamp(uv - total_disp, vec2<f32>(0.0), vec2<f32>(1.0));
    let p = src_uv * dims_f - 0.5;
    let i0 = vec2<i32>(floor(p));
    let f = p - vec2<f32>(i0);
    let max_c = vec2<i32>(i32(u.width) - 1, i32(u.height) - 1);

    let c00 = textureLoad(tex_in, clamp(i0, vec2<i32>(0), max_c), 0);
    let c10 = textureLoad(tex_in, clamp(i0 + vec2<i32>(1, 0), vec2<i32>(0), max_c), 0);
    let c01 = textureLoad(tex_in, clamp(i0 + vec2<i32>(0, 1), vec2<i32>(0), max_c), 0);
    let c11 = textureLoad(tex_in, clamp(i0 + vec2<i32>(1, 1), vec2<i32>(0), max_c), 0);

    let w00 = (1.0 - f.x) * (1.0 - f.y);
    let w10 = f.x * (1.0 - f.y);
    let w01 = (1.0 - f.x) * f.y;
    let w11 = f.x * f.y;

    let a = c00.a * w00 + c10.a * w10 + c01.a * w01 + c11.a * w11;
    var out_col = vec4<f32>(0.0);
    if (a > 0.002) {
        let pm = c00.rgb * (c00.a * w00)
               + c10.rgb * (c10.a * w10)
               + c01.rgb * (c01.a * w01)
               + c11.rgb * (c11.a * w11);
        out_col = vec4<f32>(clamp(pm / a, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(a, 0.0, 1.0));
    }
    textureStore(tex_out, vec2<i32>(gid.xy), out_col);
}

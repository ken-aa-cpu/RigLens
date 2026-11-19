struct ParticleRenderUniforms {
    dims: vec4<f32>, // (width, height, 0, 0)
};

struct GpuParticle {
    pos_vel: vec4<f32>,
    color: vec4<f32>,
    life_size: vec4<f32>, // (life, max_life, size_px, seed)
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local_uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@group(0) @binding(0) var<storage, read> particles: array<GpuParticle>;
@group(0) @binding(1) var<uniform> u: ParticleRenderUniforms;

@vertex
fn vs_main(
    @builtin(vertex_index) vid: u32,
    @builtin(instance_index) iid: u32,
) -> VsOut {
    let p = particles[iid];
    var out: VsOut;
    if (p.life_size.x <= 0.0 || p.color.a <= 0.004) {
        out.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        out.local_uv = vec2<f32>(0.0);
        out.color = vec4<f32>(0.0);
        return out;
    }

    var corner = vec2<f32>(-1.0, -1.0);
    if (vid == 1u) {
        corner = vec2<f32>(1.0, -1.0);
    } else if (vid == 2u) {
        corner = vec2<f32>(-1.0, 1.0);
    } else if (vid == 3u) {
        corner = vec2<f32>(1.0, 1.0);
    }

    let size_px = p.life_size.z;
    let px = p.pos_vel.xy + corner * size_px;
    let ndc_x = (px.x / max(u.dims.x, 1.0)) * 2.0 - 1.0;
    let ndc_y = 1.0 - (px.y / max(u.dims.y, 1.0)) * 2.0;

    out.pos = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.local_uv = corner;
    out.color = p.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let r2 = dot(in.local_uv, in.local_uv);
    if (r2 > 1.0) {
        discard;
    }
    let glow = exp(-r2 * 4.2);
    let a = clamp(in.color.a * glow, 0.0, 1.0);
    return vec4<f32>(in.color.rgb, a);
}

struct ParticleSimUniforms {
    dims_dt: vec4<f32>,      // (width, height, dt, time_s)
    motion: vec4<f32>,       // (wind_x, wind_y, motion_speed, particle_count)
    accent_spawn: vec4<f32>, // (accent_r, accent_g, accent_b, spawn_rate)
};

struct GpuParticle {
    pos_vel: vec4<f32>,   // (px, py, vx, vy)
    color: vec4<f32>,     // (r, g, b, alpha)
    life_size: vec4<f32>, // (life, max_life, size_px, seed)
};

@group(0) @binding(0) var sdf_normals: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> particles: array<GpuParticle>;
@group(0) @binding(2) var<uniform> u: ParticleSimUniforms;

fn pcg_hash(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn rand01(seed: u32) -> f32 {
    return f32(pcg_hash(seed) & 0x00FFFFFFu) / 16777216.0;
}

@compute @workgroup_size(64, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let count = u32(u.motion.w);
    if (gid.x >= count) {
        return;
    }

    let w = max(u.dims_dt.x, 1.0);
    let h = max(u.dims_dt.y, 1.0);
    let dt = clamp(u.dims_dt.z, 0.0, 0.05);
    let time_s = u.dims_dt.w;

    var p = particles[gid.x];
    var life = p.life_size.x - dt;

    if (life <= 0.0) {
        let base_seed = u32(p.life_size.w) ^ (gid.x * 1973u) ^ u32(time_s * 1000.0);
        let should_spawn = u.accent_spawn.w + u.motion.z * 0.4 > 0.02;
        if (!should_spawn) {
            p.life_size.x = 0.0;
            p.color.a = 0.0;
            particles[gid.x] = p;
            return;
        }
        // Sample candidate position & bias toward silhouette rim using SDF
        var rx = rand01(base_seed + 11u) * w;
        var ry = rand01(base_seed + 29u) * h;
        let sample_c = vec2<i32>(clamp(i32(rx), 0, i32(w) - 1), clamp(i32(ry), 0, i32(h) - 1));
        let sdf = textureLoad(sdf_normals, sample_c, 0);
        // Push candidate inward/outward along SDF gradient toward edge (sd ~ 0)
        rx = clamp(rx - sdf.x * sdf.z * 0.85, 0.0, w - 1.0);
        ry = clamp(ry - sdf.y * sdf.z * 0.85, 0.0, h - 1.0);

        let max_life = 0.8 + rand01(base_seed + 43u) * 1.4;
        let vx = u.motion.x * 45.0 + (rand01(base_seed + 61u) - 0.5) * 32.0;
        let vy = u.motion.y * 45.0 - 18.0 - rand01(base_seed + 79u) * 28.0;
        let size_px = 3.0 + rand01(base_seed + 97u) * 5.0;

        p.pos_vel = vec4<f32>(rx, ry, vx, vy);
        p.color = vec4<f32>(u.accent_spawn.rgb, 0.75);
        p.life_size = vec4<f32>(max_life, max_life, size_px, f32(pcg_hash(base_seed) & 0xFFFFu));
        particles[gid.x] = p;
        return;
    }

    let c = vec2<i32>(
        clamp(i32(p.pos_vel.x), 0, i32(w) - 1),
        clamp(i32(p.pos_vel.y), 0, i32(h) - 1),
    );
    let sdf = textureLoad(sdf_normals, c, 0);

    // Gently deflect velocity along silhouette tangent when skimming near character edge
    var force = vec2<f32>(u.motion.x * 65.0, u.motion.y * 65.0 - 14.0);
    if (abs(sdf.z) < 20.0) {
        let tangent = vec2<f32>(-sdf.y, sdf.x);
        force = force + tangent * (12.0 + u.motion.z * 18.0);
    }

    var vel = p.pos_vel.zw * exp(-1.25 * dt) + force * dt;
    var pos = p.pos_vel.xy + vel * dt;

    let life_ratio = clamp(life / max(p.life_size.y, 0.01), 0.0, 1.0);
    p.pos_vel = vec4<f32>(pos, vel);
    p.color.a = sin(life_ratio * 3.14159265) * 0.78;
    p.life_size.x = life;
    particles[gid.x] = p;
}

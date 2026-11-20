use glam::{vec2, vec3, Vec2};

use crate::physics::MotionHint;
use crate::transport::{FrameBuffer, PixelFormat};

use super::ambient::{AmbientColors, AmbientTheme};
use super::mesh_velocity::MotionVectorField;
use super::soft_body::SoftBodyZoneConfig;
use super::{GpuEffectsConfig, GpuFrameProcessor};

fn paint_opaque_disc(w: u32, h: u32, radius: f32) -> FrameBuffer {
	let mut rgba = vec![0u8; (w * h * 4) as usize];
	let cx = w as f32 * 0.5;
	let cy = h as f32 * 0.5;
	for y in 0..h {
		for x in 0..w {
			let dx = x as f32 + 0.5 - cx;
			let dy = y as f32 + 0.5 - cy;
			if dx * dx + dy * dy <= radius * radius {
				let idx = ((y * w + x) * 4) as usize;
				rgba[idx] = 64;
				rgba[idx + 1] = 64;
				rgba[idx + 2] = 64;
				rgba[idx + 3] = 255;
			}
		}
	}
	FrameBuffer::new(w, h, PixelFormat::Rgba8, rgba).unwrap()
}

#[test]
fn wgpu_m2_rim_light_respects_alpha_and_directional_sdf_normals() {
	let mut cfg = GpuEffectsConfig {
		enable_soft_body: false,
		soft_body_zones: Vec::new(),
		enable_rim_light: true,
		rim_width_px: 12.0,
		rim_intensity: 1.0,
		ambient_theme: AmbientTheme::Custom(AmbientColors {
			left: vec3(0.0, 0.9, 1.0),   // Cyan on left edge
			right: vec3(1.0, 0.1, 0.6),  // Magenta on right edge
			top: vec3(0.5, 0.5, 0.5),
			bottom: vec3(0.5, 0.5, 0.5),
		}),
		particle_count: 0,
		particle_spawn_rate: 0.0,
		..Default::default()
	};
	cfg.particle_count = 0;

	let mut gpu = GpuFrameProcessor::new_blocking(cfg).expect("init wgpu GPU processor");
	let disc = paint_opaque_disc(128, 128, 40.0);
	let out = gpu
		.process_frame(&disc, &MotionHint::default(), 1.0 / 60.0)
		.expect("process disc");

	let px = |x: u32, y: u32| -> [u8; 4] {
		let i = ((y * 128 + x) * 4) as usize;
		[out.data[i], out.data[i + 1], out.data[i + 2], out.data[i + 3]]
	};

	// Outside silhouette must remain strictly transparent & zero color leak
	assert_eq!(px(4, 4), [0, 0, 0, 0]);
	assert_eq!(px(120, 120), [0, 0, 0, 0]);

	// Deep center stays near base gray (rim faded out inside)
	let center = px(64, 64);
	assert_eq!(center[3], 255);
	assert!(center[0] < 80 && center[1] < 80 && center[2] < 80, "center shouldn't glow: {center:?}");

	// Left silhouette inner edge (x=27, y=64 -> dist ~3px from left rim x=24) receives cyan (B > R)
	let left_edge = px(27, 64);
	assert_eq!(left_edge[3], 255);
	assert!(
		left_edge[2] > left_edge[0] + 60,
		"expected cyan left rim (B >> R), got {left_edge:?}"
	);

	// Right silhouette inner edge (x=101, y=64 -> dist ~3px from right rim x=104) receives magenta (R > G)
	let right_edge = px(101, 64);
	assert_eq!(right_edge[3], 255);
	assert!(
		right_edge[0] > right_edge[1] + 60,
		"expected magenta right rim (R >> G), got {right_edge:?}"
	);
}

#[test]
fn wgpu_m2_soft_body_and_particles_execute_cleanly() {
	let cfg = GpuEffectsConfig {
		enable_soft_body: true,
		soft_body_zones: vec![SoftBodyZoneConfig {
			name: "CenterJelly".to_owned(),
			anchor_uv: vec2(0.5, 0.5),
			radius_uv: vec2(0.30, 0.30),
			follow_head: 2.0,
			wind_sensitivity: 2.0,
			stiffness: 110.0,
			damping: 4.5,
			max_strain: 0.45,
		}],
		enable_rim_light: true,
		rim_width_px: 8.0,
		rim_intensity: 0.6,
		ambient_theme: AmbientTheme::Cyberpunk,
		particle_count: 128,
		particle_spawn_rate: 1.0,
		..Default::default()
	};
	let mut gpu = GpuFrameProcessor::new_blocking(cfg).expect("init wgpu GPU processor");

	// Paint checkerboard pattern inside disc so UV warp visibly shifts pixels
	let w = 128u32;
	let h = 128u32;
	let mut rgba = vec![0u8; (w * h * 4) as usize];
	for y in 0..h {
		for x in 0..w {
			let dx = x as f32 + 0.5 - 64.0;
			let dy = y as f32 + 0.5 - 64.0;
			if dx * dx + dy * dy <= 44.0 * 44.0 {
				let i = ((y * w + x) * 4) as usize;
				let checker = ((x / 8 + y / 8) % 2) == 0;
				rgba[i] = if checker { 220 } else { 40 };
				rgba[i + 1] = 120;
				rgba[i + 2] = 160;
				rgba[i + 3] = 255;
			}
		}
	}
	let input = FrameBuffer::new(w, h, PixelFormat::Rgba8, rgba).unwrap();

	let hint_impulse = MotionHint {
		face_found: true,
		head_velocity: vec2(25.0, -10.0),
		overall_motion: 1.5,
		wind_vec: vec2(1.0, 0.2),
		..Default::default()
	};
	let out1 = gpu.process_frame(&input, &hint_impulse, 1.0 / 60.0).unwrap();
	let mut out_late = out1.clone();
	for f in 0..8 {
		let hint = MotionHint {
			face_found: true,
			head_velocity: if f == 0 { vec2(-35.0, 15.0) } else { Vec2::ZERO },
			overall_motion: 1.2,
			wind_vec: vec2(1.0, 0.3),
			..Default::default()
		};
		out_late = gpu.process_frame(&input, &hint, 1.0 / 60.0).unwrap();
	}
	assert_ne!(out1.data, out_late.data, "expected soft-body + particles to change output over time");
}

fn paint_disc_at(w: u32, h: u32, cx: f32, cy: f32, radius: f32, color: [u8; 4]) -> FrameBuffer {
	let mut rgba = vec![0u8; (w * h * 4) as usize];
	for y in 0..h {
		for x in 0..w {
			let dx = x as f32 + 0.5 - cx;
			let dy = y as f32 + 0.5 - cy;
			if dx * dx + dy * dy <= radius * radius {
				let i = ((y * w + x) * 4) as usize;
				rgba[i] = color[0];
				rgba[i + 1] = color[1];
				rgba[i + 2] = color[2];
				rgba[i + 3] = color[3];
			}
		}
	}
	FrameBuffer::new(w, h, PixelFormat::Rgba8, rgba).unwrap()
}

fn px_rgba(fb: &FrameBuffer, x: u32, y: u32) -> [u8; 4] {
	let i = ((y * fb.width + x) * 4) as usize;
	[fb.data[i], fb.data[i + 1], fb.data[i + 2], fb.data[i + 3]]
}

#[test]
fn wgpu_m3_alpha_aware_upscale_2x_preserves_transparency_and_sharpens_edges() {
	let cfg = GpuEffectsConfig {
		enable_soft_body: false,
		soft_body_zones: Vec::new(),
		enable_rim_light: false,
		particle_count: 0,
		upscale_factor: 2.0,
		upscale_sharpness: 0.85,
		frame_gen_multiplier: 1,
		..Default::default()
	};
	let mut gpu = GpuFrameProcessor::new_blocking(cfg).expect("init wgpu GPU processor");

	let (w, h) = (64u32, 64u32);
	let mut rgba = vec![0u8; (w * h * 4) as usize];
	for y in 0..h {
		for x in 0..w {
			let dx = x as f32 + 0.5 - 32.0;
			let dy = y as f32 + 0.5 - 32.0;
			if dx * dx + dy * dy <= 18.0 * 18.0 {
				let i = ((y * w + x) * 4) as usize;
				if x < 32 {
					rgba[i] = 0;
					rgba[i + 1] = 230;
					rgba[i + 2] = 255;
					rgba[i + 3] = 255;
				} else {
					rgba[i] = 255;
					rgba[i + 1] = 40;
					rgba[i + 2] = 40;
					rgba[i + 3] = 255;
				}
			}
		}
	}
	let input = FrameBuffer::new(w, h, PixelFormat::Rgba8, rgba).unwrap();
	let out = gpu
		.process_frame(&input, &MotionHint::default(), 1.0 / 60.0)
		.expect("upscale 2x frame");

	assert_eq!((out.width, out.height), (128, 128));
	assert_eq!(px_rgba(&out, 4, 4), [0, 0, 0, 0]);
	assert_eq!(px_rgba(&out, 123, 123), [0, 0, 0, 0]);

	let cyan = px_rgba(&out, 52, 64);
	assert!(cyan[3] == 255 && cyan[2] > 200 && cyan[0] < 40, "expected cyan edge interior, got {cyan:?}");
	let red = px_rgba(&out, 76, 64);
	assert!(red[3] == 255 && red[0] > 200 && red[2] < 60, "expected red edge interior, got {red:?}");
}

#[test]
fn wgpu_m3_frame_gen_tier1_mesh_velocity_places_disc_at_exact_midpoint() {
	let cfg = GpuEffectsConfig {
		enable_soft_body: false,
		soft_body_zones: Vec::new(),
		enable_rim_light: false,
		particle_count: 0,
		upscale_factor: 1.0,
		frame_gen_multiplier: 2,
		..Default::default()
	};
	let mut gpu = GpuFrameProcessor::new_blocking(cfg).expect("init wgpu GPU processor");

	let f1 = paint_disc_at(128, 128, 48.0, 64.0, 7.5, [240, 180, 60, 255]);
	let f2 = paint_disc_at(128, 128, 68.0, 64.0, 7.5, [240, 180, 60, 255]);

	let prev_verts = vec![
		vec2(34.0, 50.0),
		vec2(62.0, 50.0),
		vec2(62.0, 78.0),
		vec2(34.0, 78.0),
	];
	let curr_verts = vec![
		vec2(54.0, 50.0),
		vec2(82.0, 50.0),
		vec2(82.0, 78.0),
		vec2(54.0, 78.0),
	];
	let mv = MotionVectorField::rasterize_mesh_motion(128, 128, &prev_verts, &curr_verts, &[0, 1, 2, 0, 2, 3]);

	let out1 = gpu
		.process_frame_with_gen(&f1, &MotionHint::default(), 1.0 / 60.0, None)
		.unwrap();
	assert_eq!(out1.len(), 1);

	let out2 = gpu
		.process_frame_with_gen(&f2, &MotionHint::default(), 1.0 / 60.0, Some(&mv))
		.unwrap();
	assert_eq!(out2.len(), 2);

	let mid = &out2[0];
	let mid_center = px_rgba(mid, 58, 64);
	let old_center = px_rgba(mid, 46, 64);
	let new_center = px_rgba(mid, 70, 64);
	assert!(mid_center[3] >= 230, "expected solid interpolated disc at midpoint (58,64), got {mid_center:?}");
	assert!(old_center[3] < 20, "expected old position (46,64) cleared, got {old_center:?}");
	assert!(new_center[3] < 20, "expected new position (70,64) cleared, got {new_center:?}");
}

#[test]
fn wgpu_m3_frame_gen_tier2_optical_flow_interpolates_moving_disc() {
	let cfg = GpuEffectsConfig {
		enable_soft_body: false,
		soft_body_zones: Vec::new(),
		enable_rim_light: false,
		particle_count: 0,
		upscale_factor: 1.0,
		frame_gen_multiplier: 2,
		..Default::default()
	};
	let mut gpu = GpuFrameProcessor::new_blocking(cfg).expect("init wgpu GPU processor");

	let f1 = paint_disc_at(128, 128, 48.0, 64.0, 5.5, [220, 240, 255, 255]);
	let f2 = paint_disc_at(128, 128, 64.0, 64.0, 5.5, [220, 240, 255, 255]);
	let hint = MotionHint {
		face_found: true,
		head_velocity: vec2(131.25, 0.0), // prior ~+14px refined by Tier 2 optical flow to +16px
		eye_open_min: 1.0,
		..Default::default()
	};

	let out1 = gpu
		.process_frame_with_gen(&f1, &MotionHint::default(), 1.0 / 60.0, None)
		.unwrap();
	assert_eq!(out1.len(), 1);

	let out2 = gpu
		.process_frame_with_gen(&f2, &hint, 1.0 / 60.0, None)
		.unwrap();
	assert_eq!(out2.len(), 2);

	let mid = &out2[0];
	let mid_center = px_rgba(mid, 56, 64);
	let old_center = px_rgba(mid, 46, 64);
	let new_center = px_rgba(mid, 66, 64);
	assert!(mid_center[3] >= 230, "expected Tier 2 optical-flow disc at midpoint (56,64), got {mid_center:?}");
	assert!(old_center[3] < 20, "expected old position (46,64) cleared, got {old_center:?}");
	assert!(new_center[3] < 20, "expected new position (66,64) cleared, got {new_center:?}");
}

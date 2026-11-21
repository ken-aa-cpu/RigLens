use glam::{vec2, vec3};
use image::{Rgba, RgbaImage};
use riglens_core::math::transform::TransformOffset;
use riglens_core::model::Model;
use riglens_core::node::components::PhysicsModel;
use riglens_core::node::InoxNode;
use riglens_core::puppet::Puppet;

use super::bindings::{
	add_standard_vtuber_params, apply_blink_binding, apply_breath_binding, apply_head_perspective_binding,
	apply_head_roll_binding, apply_mouth_form_binding, apply_mouth_open_binding, apply_sway_binding,
};
use super::mesh::add_grid_part;
use super::physics_anim::{add_pendulum_physics, create_idle_animation};

// --- Procedural Texture Helpers for Sample VTuber Model ---

fn paint_ellipse_part(w: u32, h: u32, fill: [u8; 4], border: [u8; 4], highlight: Option<[u8; 4]>) -> RgbaImage {
	let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
	let cx = w as f32 * 0.5;
	let cy = h as f32 * 0.5;
	let rx = w as f32 * 0.46;
	let ry = h as f32 * 0.46;
	for y in 0..h {
		for x in 0..w {
			let dx = (x as f32 + 0.5 - cx) / rx;
			let dy = (y as f32 + 0.5 - cy) / ry;
			let d = (dx * dx + dy * dy).sqrt();
			if d <= 1.0 {
				let c = if d > 0.90 {
					border
				} else if let Some(hl) = highlight {
					if dy < -0.25 && dx.abs() < 0.55 {
						hl
					} else {
						fill
					}
				} else {
					fill
				};
				img.put_pixel(x, y, Rgba(c));
			}
		}
	}
	img
}

fn paint_eye_texture(w: u32, h: u32) -> RgbaImage {
	let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
	let cx = w as f32 * 0.5;
	let cy = h as f32 * 0.52;
	let rx = w as f32 * 0.44;
	let ry = h as f32 * 0.40;
	for y in 0..h {
		for x in 0..w {
			let dx = (x as f32 + 0.5 - cx) / rx;
			let dy = (y as f32 + 0.5 - cy) / ry;
			let d = (dx * dx + dy * dy).sqrt();
			if d <= 1.0 {
				// Sclera
				let mut col = [250, 250, 255, 255];
				// Iris
				let iris_d = ((dx / 0.56).powi(2) + ((dy - 0.05) / 0.72).powi(2)).sqrt();
				if iris_d < 1.0 {
					col = if dy < 0.0 {
						[55, 115, 215, 255]
					} else {
						[105, 185, 245, 255]
					};
					// Pupil
					if iris_d < 0.42 {
						col = [25, 30, 65, 255];
					}
				}
				// Specular highlights
				let hl1 = ((dx + 0.22).powi(2) + (dy + 0.24).powi(2)).sqrt();
				let hl2 = ((dx - 0.20).powi(2) + (dy - 0.20).powi(2)).sqrt();
				if hl1 < 0.16 || hl2 < 0.09 {
					col = [255, 255, 255, 255];
				}
				// Upper eyelash
				if dy < -0.58 || d > 0.88 && dy < 0.1 {
					col = [42, 32, 48, 255];
				}
				img.put_pixel(x, y, Rgba(col));
			}
		}
	}
	img
}

fn paint_mouth_texture(w: u32, h: u32) -> RgbaImage {
	let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
	let cx = w as f32 * 0.5;
	let cy = h as f32 * 0.5;
	let rx = w as f32 * 0.42;
	let ry = h as f32 * 0.36;
	for y in 0..h {
		for x in 0..w {
			let dx = (x as f32 + 0.5 - cx) / rx;
			let dy = (y as f32 + 0.5 - cy) / ry;
			let d = (dx * dx + dy * dy).sqrt();
			if d <= 1.0 {
				let c = if d > 0.80 {
					[145, 65, 78, 255]
				} else if dy > 0.25 {
					[235, 125, 145, 255]
				} else {
					[180, 65, 85, 255]
				};
				img.put_pixel(x, y, Rgba(c));
			}
		}
	}
	img
}

fn paint_hair_strand(w: u32, h: u32, base: [u8; 4], tip: [u8; 4]) -> RgbaImage {
	let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
	let cx = w as f32 * 0.5;
	for y in 0..h {
		let ty = y as f32 / (h as f32 - 1.0);
		// Taper width near bottom tip
		let half_w = w as f32 * 0.45 * (1.0 - ty.powf(1.8) * 0.85);
		for x in 0..w {
			let dx = ((x as f32 + 0.5) - cx).abs();
			if dx <= half_w {
				let edge = dx / half_w.max(1.0);
				let r = ((1.0 - ty) * base[0] as f32 + ty * tip[0] as f32) as u8;
				let g = ((1.0 - ty) * base[1] as f32 + ty * tip[1] as f32) as u8;
				let b = ((1.0 - ty) * base[2] as f32 + ty * tip[2] as f32) as u8;
				let c = if edge > 0.88 { [48, 38, 72, 255] } else { [r, g, b, 255] };
				img.put_pixel(x, y, Rgba(c));
			}
		}
	}
	img
}

/// Build a complete, rigged sample VTuber Model ready for `.inp` export,
/// headless rendering, and interactive MCP manipulation.
pub fn build_sample_vtuber_model() -> Model {
	let mut model = Model {
		puppet: Puppet::new_empty("InochiSampleVTuber"),
		textures: Vec::new(),
		vendors: Vec::new(),
	};

	let root_id = model.puppet.nodes.root_node_id;

	// Create Body & Head hierarchy
	let body_group = model.puppet.next_node_uuid();
	model.puppet.add_node(
		root_id,
		InoxNode {
			uuid: body_group,
			name: "Body".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset::default(),
			lock_to_root: false,
		},
	);

	let torso_img = paint_ellipse_part(160, 140, [70, 95, 165, 255], [40, 45, 85, 255], Some([110, 140, 210, 255]));
	let _torso = add_grid_part(
		&mut model,
		body_group,
		"Torso",
		10.0,
		vec2(0.0, 130.0),
		180.0,
		150.0,
		4,
		4,
		torso_img,
	);

	let head_group = model.puppet.next_node_uuid();
	model.puppet.add_node(
		body_group,
		InoxNode {
			uuid: head_group,
			name: "Head".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset {
				translation: vec3(0.0, -25.0, 0.0),
				..TransformOffset::default()
			},
			lock_to_root: false,
		},
	);

	// Back Hair (zsort higher = drawn behind in Inochi2D descending zsort order)
	let hair_back_img = paint_hair_strand(200, 220, [75, 65, 130, 255], [115, 95, 185, 255]);
	let hair_back = add_grid_part(
		&mut model,
		head_group,
		"Hair_Back",
		20.0,
		vec2(0.0, 35.0),
		230.0,
		250.0,
		6,
		8,
		hair_back_img,
	);

	// Face Base
	let face_img = paint_ellipse_part(180, 180, [255, 228, 212, 255], [185, 135, 125, 255], None);
	let face = add_grid_part(
		&mut model,
		head_group,
		"Face",
		0.0,
		vec2(0.0, 0.0),
		170.0,
		175.0,
		6,
		6,
		face_img,
	);

	// Left & Right Eyes (drawn in front of Face => lower zsort)
	let eye_l_img = paint_eye_texture(64, 56);
	let eye_l = add_grid_part(
		&mut model,
		head_group,
		"Eye_L",
		-5.0,
		vec2(-38.0, -6.0),
		46.0,
		40.0,
		4,
		4,
		eye_l_img,
	);
	let eye_r_img = paint_eye_texture(64, 56);
	let eye_r = add_grid_part(
		&mut model,
		head_group,
		"Eye_R",
		-5.0,
		vec2(38.0, -6.0),
		46.0,
		40.0,
		4,
		4,
		eye_r_img,
	);

	// Mouth
	let mouth_img = paint_mouth_texture(48, 32);
	let mouth = add_grid_part(
		&mut model,
		head_group,
		"Mouth",
		-6.0,
		vec2(0.0, 42.0),
		36.0,
		22.0,
		4,
		4,
		mouth_img,
	);

	// Front Hair Strands (drawn in front of face & eyes)
	let hair_fl_img = paint_hair_strand(72, 130, [92, 78, 155, 255], [140, 118, 215, 255]);
	let hair_fl = add_grid_part(
		&mut model,
		head_group,
		"Hair_Front_L",
		-10.0,
		vec2(-56.0, -18.0),
		64.0,
		135.0,
		4,
		6,
		hair_fl_img,
	);
	let hair_fr_img = paint_hair_strand(72, 130, [92, 78, 155, 255], [140, 118, 215, 255]);
	let hair_fr = add_grid_part(
		&mut model,
		head_group,
		"Hair_Front_R",
		-10.0,
		vec2(56.0, -18.0),
		64.0,
		135.0,
		4,
		6,
		hair_fr_img,
	);

	// Create Standard VTuber Parameters
	add_standard_vtuber_params(&mut model.puppet);

	// Apply 2.5D Head Yaw-Pitch bindings across head parts with layered depths
	let _ = apply_head_perspective_binding(&mut model.puppet, hair_back, "Head:: Yaw-Pitch", -0.8, 28.0, 20.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, face, "Head:: Yaw-Pitch", 0.25, 24.0, 18.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, eye_l, "Head:: Yaw-Pitch", 1.0, 26.0, 20.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, eye_r, "Head:: Yaw-Pitch", 1.0, 26.0, 20.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, mouth, "Head:: Yaw-Pitch", 1.1, 26.0, 20.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, hair_fl, "Head:: Yaw-Pitch", 1.25, 26.0, 20.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, hair_fr, "Head:: Yaw-Pitch", 1.25, 26.0, 20.0);

	// Bind blinks & mouth open
	let _ = apply_blink_binding(&mut model.puppet, eye_l, "Eye:: Left:: Open", 0.65);
	let _ = apply_blink_binding(&mut model.puppet, eye_r, "Eye:: Right:: Open", 0.65);
	let _ = apply_mouth_open_binding(&mut model.puppet, mouth, "Mouth:: Open", 12.0);

	// Bind Hair Sway + SimplePhysics Pendulum
	let _ = apply_sway_binding(&mut model.puppet, hair_back, "Hair:: Sway", 24.0);
	let _ = apply_sway_binding(&mut model.puppet, hair_fl, "Hair:: Sway", 18.0);
	let _ = apply_sway_binding(&mut model.puppet, hair_fr, "Hair:: Sway", 18.0);
	let _ = add_pendulum_physics(
		&mut model.puppet,
		head_group,
		"Hair_Physics",
		"Hair:: Sway",
		PhysicsModel::RigidPendulum,
		80.0,
		2.2,
		0.45,
		vec2(1.0, 1.0),
	);

	let _ = apply_mouth_form_binding(&mut model.puppet, mouth, "Mouth:: Form", 6.0, 5.0);
	let _ = apply_head_roll_binding(&mut model.puppet, head_group, "Head:: Roll", 15.0);
	let _ = apply_breath_binding(&mut model.puppet, body_group, "Breath", 0.02, 4.0);
	create_idle_animation(&mut model.puppet);
	crate::audit::auto_fix_puppet(&mut model.puppet);

	model
}

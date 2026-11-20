use glam::vec2;
use image::{Rgba, RgbaImage};
use inox2d::model::Model;
use inox2d::node::components::{BlendMode, Drawable, Mesh, PhysicsModel, SimplePhysics};
use inox2d::node::InoxNodeUuid;
use inox2d::puppet::Puppet;

use super::bindings::{
	add_standard_vtuber_params, apply_blink_binding, apply_breath_binding, apply_brow_binding,
	apply_eye_gaze_binding, apply_head_perspective_binding, apply_head_roll_binding, apply_mouth_form_binding,
	apply_mouth_open_binding, apply_opacity_binding, apply_sway_binding,
};
use super::mesh::{add_grid_part, mesh_bounds};
use super::physics_anim::{add_pendulum_physics, create_idle_animation};

/// Automatically rig all Part/Group nodes in `puppet` by semantic layer naming (EN/ZH/JP-romaji).
pub fn auto_rig_vtuber(puppet: &mut Puppet) -> Vec<String> {
	add_standard_vtuber_params(puppet);

	let nodes: Vec<(InoxNodeUuid, String, bool)> = puppet
		.nodes
		.pre_order_iter()
		.skip(1)
		.map(|n| {
			let is_part = puppet.node_comps.get::<Mesh>(n.uuid).is_some();
			(n.uuid, n.name.clone(), is_part)
		})
		.collect();

	let mut actions = Vec::new();
	let mut head_anchor: Option<InoxNodeUuid> = None;
	let mut body_anchor: Option<InoxNodeUuid> = None;
	let has_physics = puppet
		.nodes
		.iter()
		.any(|n| puppet.node_comps.get::<SimplePhysics>(n.uuid).is_some());

	// Measure Face bounding box for proportional displacement scaling
	let (face_w, face_h) = crate::audit::find_face_node(puppet)
		.and_then(|fid| puppet.node_comps.get::<Mesh>(fid))
		.map(|m| {
			let (mn, mx) = mesh_bounds(m);
			((mx.x - mn.x).max(40.0), (mx.y - mn.y).max(40.0))
		})
		.unwrap_or((170.0, 175.0));
	let yaw_px = (face_w * 0.14).clamp(10.0, 120.0);
	let pitch_px = (face_h * 0.11).clamp(8.0, 100.0);
	let mouth_open_px = (face_h * 0.068).clamp(6.0, 48.0);
	let sway_front = (face_h * 0.10).clamp(8.0, 60.0);
	let sway_back = (face_h * 0.14).clamp(12.0, 80.0);

	for &(id, ref raw_name, is_part) in &nodes {
		let lower = raw_name.to_lowercase();
		if head_anchor.is_none() && (lower == "head" || lower.contains("頭部") || lower.contains("head_group")) {
			head_anchor = Some(id);
		}
		if body_anchor.is_none() && (lower == "body" || lower.contains("torso") || lower.contains("身體")) {
			body_anchor = Some(id);
		}
		if !is_part {
			continue;
		}

		if lower.contains("hair_back") || lower.contains("back_hair") || lower.contains("後髮") || lower.contains("后发") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", -0.85, yaw_px * 1.05, pitch_px);
			let _ = apply_sway_binding(puppet, id, "Hair:: Sway", sway_back);
			actions.push(format!("Rigged back hair '{raw_name}' (depth_z=-0.85 + sway)"));
		} else if lower.contains("hair_front") || lower.contains("bang") || lower.contains("ahoge") || lower.contains("前髮") || lower.contains("劉海") || lower.contains("刘海") || lower.contains("呆毛") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.25, yaw_px, pitch_px);
			let _ = apply_sway_binding(puppet, id, "Hair:: Sway", sway_front);
			actions.push(format!("Rigged front hair '{raw_name}' (depth_z=1.25 + sway)"));
		} else if lower.contains("hair_side") || lower.contains("side_hair") || lower.contains("側髮") || lower.contains("鬢角") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 0.65, yaw_px, pitch_px);
			let _ = apply_sway_binding(puppet, id, "Hair:: Sway", sway_front * 1.1);
			actions.push(format!("Rigged side hair '{raw_name}' (depth_z=0.65 + sway)"));
		} else if lower.contains("eye_l") || lower.contains("left_eye") || lower.contains("左眼") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.0, yaw_px, pitch_px);
			let _ = apply_blink_binding(puppet, id, "Eye:: Left:: Open", 0.65);
			let _ = apply_eye_gaze_binding(puppet, id, "Eye:: Gaze", yaw_px * 0.2, pitch_px * 0.2);
			actions.push(format!("Rigged left eye '{raw_name}' (perspective + blink + gaze)"));
		} else if lower.contains("eye_r") || lower.contains("right_eye") || lower.contains("右眼") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.0, yaw_px, pitch_px);
			let _ = apply_blink_binding(puppet, id, "Eye:: Right:: Open", 0.65);
			let _ = apply_eye_gaze_binding(puppet, id, "Eye:: Gaze", yaw_px * 0.2, pitch_px * 0.2);
			actions.push(format!("Rigged right eye '{raw_name}' (perspective + blink + gaze)"));
		} else if lower.contains("mouth") || lower.contains("lip") || lower.contains("嘴") || lower.contains("口") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.1, yaw_px, pitch_px);
			let _ = apply_mouth_open_binding(puppet, id, "Mouth:: Open", mouth_open_px);
			let _ = apply_mouth_form_binding(puppet, id, "Mouth:: Form", mouth_open_px * 0.5, mouth_open_px * 0.4);
			actions.push(format!("Rigged mouth '{raw_name}' (perspective + open + form)"));
		} else if lower.contains("nose") || lower.contains("鼻") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.35, yaw_px, pitch_px);
			actions.push(format!("Rigged nose '{raw_name}' (depth_z=1.35)"));
		} else if lower.contains("brow") || lower.contains("眉") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.05, yaw_px, pitch_px);
			let is_right = lower.contains("_r") || lower.contains("right") || lower.contains("右");
			let brow_param = if is_right { "Brow:: Right" } else { "Brow:: Left" };
			let _ = apply_brow_binding(puppet, id, brow_param, is_right);
			actions.push(format!("Rigged brow '{raw_name}' (depth_z=1.05 + {brow_param})"));
		} else if lower.contains("ear") || lower.contains("耳") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", -0.55, yaw_px * 0.9, pitch_px * 0.9);
			actions.push(format!("Rigged ear '{raw_name}' (depth_z=-0.55)"));
		} else if lower.contains("face") || lower.contains("head") || lower.contains("臉") || lower.contains("脸") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 0.25, yaw_px * 0.9, pitch_px * 0.9);
			if head_anchor.is_none() {
				head_anchor = Some(id);
			}
			actions.push(format!("Rigged face '{raw_name}' (depth_z=0.25)"));
		} else if lower.contains("torso") || lower.contains("body") || lower.contains("身體") || lower.contains("衣") {
			let _ = apply_breath_binding(puppet, id, "Breath", 0.02, 4.0);
			actions.push(format!("Rigged body '{raw_name}' (breath)"));
		}
	}

	if let Some(head_id) = head_anchor {
		let _ = apply_head_roll_binding(puppet, head_id, "Head:: Roll", 15.0);
		if !has_physics {
			let _ = add_pendulum_physics(
				puppet,
				head_id,
				"Hair_Physics",
				"Hair:: Sway",
				PhysicsModel::RigidPendulum,
				80.0,
				2.2,
				0.45,
				vec2(1.0, 1.0),
			);
			actions.push("Added Hair_Physics SimplePendulum".to_owned());
		}
	} else if !has_physics {
		let root_id = puppet.nodes.root_node_id;
		let _ = add_pendulum_physics(
			puppet,
			root_id,
			"Hair_Physics",
			"Hair:: Sway",
			PhysicsModel::RigidPendulum,
			80.0,
			2.2,
			0.45,
			vec2(1.0, 1.0),
		);
		actions.push("Added Hair_Physics SimplePendulum under Root".to_owned());
	}

	if let Some(body_id) = body_anchor {
		let _ = apply_breath_binding(puppet, body_id, "Breath", 0.02, 4.0);
	}

	create_idle_animation(puppet);
	actions.push("Created 'Idle' animation (120 frames @ 30fps)".to_owned());
	let fix_res = crate::audit::auto_fix_puppet(puppet);
	actions.push(format!("Auto-fixed constraints: {}", json::stringify(fix_res)));
	actions
}

/// Procedurally paint and attach an expression overlay Part (`blush`, `gloom`, `tears`, `angry`, `shock`)
/// positioned relative to Face bounds, and bind a 0..1 Opacity parameter (`Expr:: <Kind>`).
pub fn synthesize_expression_overlay(
	model: &mut Model,
	kind: &str,
	parent_id: Option<InoxNodeUuid>,
) -> Result<(InoxNodeUuid, String), String> {
	model.puppet.init_all();
	model.puppet.begin_frame();
	model.puppet.end_frame(0.0);

	let face_id = crate::audit::find_face_node(&model.puppet);
	let (face_w, face_h) = face_id
		.and_then(|fid| model.puppet.node_comps.get::<Mesh>(fid))
		.map(|m| {
			let (mn, mx) = mesh_bounds(m);
			((mx.x - mn.x).max(60.0), (mx.y - mn.y).max(60.0))
		})
		.unwrap_or((170.0, 175.0));

	let target_parent = parent_id
		.or_else(|| {
			model
				.puppet
				.nodes
				.iter()
				.find(|n| n.name.eq_ignore_ascii_case("Head") || n.name.contains("頭"))
				.map(|n| n.uuid)
		})
		.or_else(|| face_id.map(|fid| model.puppet.nodes.get_parent(fid).uuid))
		.unwrap_or(model.puppet.nodes.root_node_id);

	let lower = kind.to_lowercase();
	let (part_name, param_name, zsort, offset, pw, ph, blend, img) = match lower.as_str() {
		"blush" | "cheek" | "臉紅" => {
			let w = (face_w * 0.78).round().max(48.0) as u32;
			let h = (face_h * 0.22).round().max(24.0) as u32;
			let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
			for side in [0.22f32, 0.78f32] {
				let cx = w as f32 * side;
				let cy = h as f32 * 0.5;
				let rx = w as f32 * 0.16;
				let ry = h as f32 * 0.40;
				for y in 0..h {
					for x in 0..w {
						let dx = (x as f32 + 0.5 - cx) / rx;
						let dy = (y as f32 + 0.5 - cy) / ry;
						let d2 = dx * dx + dy * dy;
						if d2 < 1.0 {
							let falloff = (1.0 - d2).powf(1.4);
							let slash = if ((x as i32 + y as i32) % 9).abs() <= 1 && d2 < 0.55 {
								45u8
							} else {
								0
							};
							let alpha = (falloff * 185.0 + slash as f32).clamp(0.0, 230.0) as u8;
							img.put_pixel(x, y, Rgba([255, 110, 145, alpha]));
						}
					}
				}
			}
			(
				"Expr_Blush",
				"Expr:: Blush",
				-2.5,
				vec2(0.0, face_h * 0.08),
				w as f32,
				h as f32,
				BlendMode::Normal,
				img,
			)
		}
		"gloom" | "shadow" | "陰沉" | "黑臉" => {
			let w = (face_w * 0.92).round().max(64.0) as u32;
			let h = (face_h * 0.55).round().max(48.0) as u32;
			let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
			let cx = w as f32 * 0.5;
			for y in 0..h {
				let ty = 1.0 - (y as f32 / (h as f32 - 1.0));
				for x in 0..w {
					let dx = ((x as f32 + 0.5 - cx) / (w as f32 * 0.48)).abs();
					if dx <= 1.0 {
						let edge = (1.0 - dx * dx).max(0.0);
						let alpha = (ty.powf(1.2) * edge * 190.0).clamp(0.0, 210.0) as u8;
						img.put_pixel(x, y, Rgba([58, 52, 110, alpha]));
					}
				}
			}
			(
				"Expr_Gloom",
				"Expr:: Gloom",
				-7.5,
				vec2(0.0, -face_h * 0.22),
				w as f32,
				h as f32,
				BlendMode::Multiply,
				img,
			)
		}
		"tears" | "cry" | "眼淚" => {
			let w = (face_w * 0.72).round().max(48.0) as u32;
			let h = (face_h * 0.26).round().max(24.0) as u32;
			let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
			for side in [0.22f32, 0.78f32] {
				let cx = w as f32 * side;
				let cy = h as f32 * 0.55;
				let rx = w as f32 * 0.11;
				let ry = h as f32 * 0.36;
				for y in 0..h {
					for x in 0..w {
						let dx = (x as f32 + 0.5 - cx) / rx;
						let dy = (y as f32 + 0.5 - cy) / ry;
						let d2 = dx * dx + dy * dy;
						if d2 < 1.0 {
							let is_hl = ((dx + 0.25).powi(2) + (dy + 0.25).powi(2)) < 0.18;
							let col = if is_hl {
								[245, 252, 255, 240]
							} else {
								[115, 200, 255, ((1.0 - d2 * 0.5) * 215.0) as u8]
							};
							img.put_pixel(x, y, Rgba(col));
						}
					}
				}
			}
			(
				"Expr_Tears",
				"Expr:: Tears",
				-6.5,
				vec2(0.0, face_h * 0.04),
				w as f32,
				h as f32,
				BlendMode::Normal,
				img,
			)
		}
		"angry" | "angry_mark" | "青筋" | "生氣" => {
			let sz = (face_w * 0.22).round().max(24.0) as u32;
			let mut img = RgbaImage::from_pixel(sz, sz, Rgba([0, 0, 0, 0]));
			let c = sz as f32 * 0.5;
			let r = sz as f32 * 0.36;
			for y in 0..sz {
				for x in 0..sz {
					let dx = x as f32 + 0.5 - c;
					let dy = y as f32 + 0.5 - c;
					// Four curved anime cross-vein corners
					let corner = ((dx.abs() - r * 0.55).powi(2) + (dy.abs() - r * 0.55).powi(2)).sqrt();
					if (corner - r * 0.32).abs() < 2.2 && dx.abs() < r && dy.abs() < r {
						img.put_pixel(x, y, Rgba([235, 55, 70, 240]));
					}
				}
			}
			(
				"Expr_Angry",
				"Expr:: Angry",
				-11.0,
				vec2(face_w * 0.30, -face_h * 0.28),
				sz as f32,
				sz as f32,
				BlendMode::Normal,
				img,
			)
		}
		"shock" | "驚嚇" => {
			let w = (face_w * 0.72).round().max(48.0) as u32;
			let h = (face_h * 0.30).round().max(24.0) as u32;
			let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
			for x in (4..w - 4).step_by(8) {
				for y in 0..h {
					let ty = y as f32 / h as f32;
					let alpha = ((1.0 - ty) * 210.0) as u8;
					img.put_pixel(x, y, Rgba([210, 225, 255, alpha]));
					if x + 1 < w {
						img.put_pixel(x + 1, y, Rgba([210, 225, 255, alpha / 2]));
					}
				}
			}
			(
				"Expr_Shock",
				"Expr:: Shock",
				-8.0,
				vec2(0.0, -face_h * 0.25),
				w as f32,
				h as f32,
				BlendMode::Normal,
				img,
			)
		}
		other => {
			return Err(format!(
				"Unknown overlay kind '{other}' (expected blush, gloom, tears, angry, shock)"
			));
		}
	};

	let id = add_grid_part(model, target_parent, part_name, zsort, offset, pw, ph, 4, 4, img);
	if let Some(d) = model.puppet.node_comps.get_mut::<Drawable>(id) {
		d.blending.mode = blend;
	}

	if model.puppet.params.contains_key("Head:: Yaw-Pitch") {
		let _ = apply_head_perspective_binding(&mut model.puppet, id, "Head:: Yaw-Pitch", 0.75, face_w * 0.14, face_h * 0.11);
	}
	apply_opacity_binding(&mut model.puppet, id, param_name, 0.0, 1.0)?;
	model.puppet.reset_contexts();

	Ok((id, param_name.to_owned()))
}

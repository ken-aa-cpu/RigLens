use glam::vec2;
use image::{Rgba, RgbaImage};
use riglens_core::model::Model;
use riglens_core::math::interp::InterpolateMode;
use riglens_core::math::matrix::Matrix2d;
use riglens_core::node::components::{BlendMode, Drawable, Mask, MaskMode, Masks, Mesh, PhysicsModel, SimplePhysics};
use riglens_core::node::InoxNodeUuid;
use riglens_core::params::{AxisPoints, Binding, BindingValues};
use riglens_core::puppet::Puppet;

use super::bindings::{
	add_standard_vtuber_params, apply_blink_binding, apply_breath_binding, apply_brow_binding,
	apply_eye_gaze_binding, apply_head_perspective_binding, apply_head_roll_binding, apply_mouth_form_binding,
	apply_mouth_open_binding, apply_opacity_binding, apply_sway_binding, ensure_param,
};
use super::mesh::{add_grid_part, mesh_bounds};
use super::physics_anim::{add_pendulum_physics, create_idle_animation};

/// Detect left/right side from common English, suffix, arrow, and CJK naming (`Some(false)` = Left, `Some(true)` = Right).
fn detect_side(lower: &str) -> Option<bool> {
	let is_left = lower.ends_with("_l")
		|| lower.ends_with(" l")
		|| lower.contains("_l_")
		|| lower.contains(" l ")
		|| lower.starts_with("l_")
		|| lower.contains("left")
		|| lower.contains("左")
		|| lower.contains('←');
	let is_right = lower.ends_with("_r")
		|| lower.ends_with(" r")
		|| lower.contains("_r_")
		|| lower.contains(" r ")
		|| lower.starts_with("r_")
		|| lower.contains("right")
		|| lower.contains("右")
		|| lower.contains('→');
	match (is_left, is_right) {
		(true, false) => Some(false),
		(false, true) => Some(true),
		_ => None,
	}
}

/// Detect left/right side from `lower`, falling back to ancestor group names up to root.
fn detect_side_with_ancestors(puppet: &Puppet, id: InoxNodeUuid, lower: &str) -> Option<bool> {
	if let Some(side) = detect_side(lower) {
		return Some(side);
	}
	let mut cur = id;
	let root = puppet.nodes.root_node_id;
	for _ in 0..64 {
		if cur == root {
			break;
		}
		let parent = puppet.nodes.get_parent(cur);
		if let Some(side) = detect_side(&parent.name.to_lowercase()) {
			return Some(side);
		}
		cur = parent.uuid;
	}
	None
}

/// Bind uniform rigid parallax translation (no quadratic vertex shear) for small facial parts such as `nose`.
fn apply_rigid_parallax_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	depth_z: f32,
	yaw_px: f32,
	pitch_px: f32,
) -> Result<(), String> {
	let mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();

	ensure_param(
		puppet,
		param_name,
		true,
		vec2(-1.0, -1.0),
		vec2(1.0, 1.0),
		vec2(0.0, 0.0),
		AxisPoints {
			x: vec![0.0, 0.5, 1.0],
			y: vec![0.0, 0.5, 1.0],
		},
	);

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;
	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let vert_len = mesh.vertices.len();

	let values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, iy| {
		let tx = param.axis_points.x[ix];
		let ty = param.axis_points.y[iy];
		let yaw = param.min.x + (param.max.x - param.min.x) * tx;
		let pitch = param.min.y + (param.max.y - param.min.y) * ty;
		let shift = vec2(yaw * yaw_px * depth_z, pitch * pitch_px * depth_z);
		vec![shift; vert_len]
	});
	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	let binding = Binding {
		node: node_id,
		is_set,
		interpolate_mode: InterpolateMode::Linear,
		values: BindingValues::Deform(values),
	};
	if let Some(existing) = param
		.bindings
		.iter_mut()
		.find(|b| b.node == binding.node && std::mem::discriminant(&b.values) == std::mem::discriminant(&binding.values))
	{
		*existing = binding;
	} else {
		param.bindings.push(binding);
	}
	Ok(())
}

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
	let mut left_eyeball_id: Option<InoxNodeUuid> = None;
	let mut right_eyeball_id: Option<InoxNodeUuid> = None;
	let mut mask_targets: Vec<(InoxNodeUuid, String, Option<bool>)> = Vec::new();
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

	// Pre-discover eyeball/sclera nodes for automatic pupil/highlight masking
	for &(id, ref raw_name, is_part) in &nodes {
		if !is_part {
			continue;
		}
		let lower = raw_name.to_lowercase();
		if lower.contains("eyeball")
			|| lower.contains("eye_white")
			|| lower.contains("sclera")
			|| lower.contains("眼白")
			|| lower.contains("白目")
			|| lower.contains("白眼")
			|| lower.contains("眼球")
		{
			match detect_side_with_ancestors(puppet, id, &lower) {
				Some(false) if left_eyeball_id.is_none() => left_eyeball_id = Some(id),
				Some(true) if right_eyeball_id.is_none() => right_eyeball_id = Some(id),
				_ => {}
			}
		}
	}

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

		let is_tiny = puppet
			.node_comps
			.get::<Mesh>(id)
			.map(|m| {
				let (mn, mx) = mesh_bounds(m);
				(mx.x - mn.x).min(mx.y - mn.y) < face_w * 0.16
			})
			.unwrap_or(false);

		if lower.contains("hair_back")
			|| lower.contains("back_hair")
			|| lower.contains("後髮")
			|| lower.contains("后发")
			|| lower.contains("後ろ髪")
		{
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", -0.85, yaw_px * 1.05, pitch_px);
			let _ = apply_sway_binding(puppet, id, "Hair:: Sway", sway_back);
			actions.push(format!("Rigged back hair '{raw_name}' (depth_z=-0.85 + sway)"));
		} else if lower.contains("hair_front")
			|| lower.contains("bang")
			|| lower.contains("ahoge")
			|| lower.contains("前髮")
			|| lower.contains("前髪")
			|| lower.contains("劉海")
			|| lower.contains("刘海")
			|| lower.contains("呆毛")
		{
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.25, yaw_px, pitch_px);
			let _ = apply_sway_binding(puppet, id, "Hair:: Sway", sway_front);
			actions.push(format!("Rigged front hair '{raw_name}' (depth_z=1.25 + sway)"));
		} else if lower.contains("hair_side")
			|| lower.contains("side_hair")
			|| lower.contains("側髮")
			|| lower.contains("鬢角")
			|| lower.contains("横髪")
			|| lower.contains("もみあげ")
		{
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 0.65, yaw_px, pitch_px);
			let _ = apply_sway_binding(puppet, id, "Hair:: Sway", sway_front * 1.1);
			actions.push(format!("Rigged side hair '{raw_name}' (depth_z=0.65 + sway)"));
		} else if lower.contains("pupil") || lower.contains("iris") || lower.contains("highlight") || lower.contains("瞳") {
			let side = detect_side_with_ancestors(puppet, id, &lower);
			let _ = apply_rigid_parallax_binding(puppet, id, "Head:: Yaw-Pitch", 1.0, yaw_px, pitch_px);
			match side {
				Some(false) => {
					let _ = apply_blink_binding(puppet, id, "Eye:: Left:: Open", 0.65);
				}
				Some(true) => {
					let _ = apply_blink_binding(puppet, id, "Eye:: Right:: Open", 0.65);
				}
				None => {}
			}
			let _ = apply_eye_gaze_binding(puppet, id, "Eye:: Gaze", yaw_px * 0.25, pitch_px * 0.25);
			mask_targets.push((id, raw_name.clone(), side));
			actions.push(format!("Rigged eye iris/highlight '{raw_name}' (parallax + gaze)"));
		} else if lower.contains("eyelid") || lower.contains("eyelash") || lower.contains("lash") {
			let side = detect_side_with_ancestors(puppet, id, &lower);
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.0, yaw_px, pitch_px);
			match side {
				Some(false) => {
					let _ = apply_blink_binding(puppet, id, "Eye:: Left:: Open", 0.65);
				}
				Some(true) => {
					let _ = apply_blink_binding(puppet, id, "Eye:: Right:: Open", 0.65);
				}
				None => {}
			}
			actions.push(format!("Rigged eyelid/lash '{raw_name}' (perspective + blink)"));
		} else if lower.contains("eye_l")
			|| lower.contains("left_eye")
			|| lower.contains("左眼")
			|| ((lower.contains("eyeball")
				|| lower.contains("eye_white")
				|| lower.contains("sclera")
				|| lower.contains("白目")
				|| lower.contains("白眼")
				|| lower.contains("眼球"))
				&& detect_side_with_ancestors(puppet, id, &lower) == Some(false))
		{
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.0, yaw_px, pitch_px);
			let _ = apply_blink_binding(puppet, id, "Eye:: Left:: Open", 0.65);
			let _ = apply_eye_gaze_binding(puppet, id, "Eye:: Gaze", yaw_px * 0.2, pitch_px * 0.2);
			actions.push(format!("Rigged left eye '{raw_name}' (perspective + blink + gaze)"));
		} else if lower.contains("eye_r")
			|| lower.contains("right_eye")
			|| lower.contains("右眼")
			|| ((lower.contains("eyeball")
				|| lower.contains("eye_white")
				|| lower.contains("sclera")
				|| lower.contains("白目")
				|| lower.contains("白眼")
				|| lower.contains("眼球"))
				&& detect_side_with_ancestors(puppet, id, &lower) == Some(true))
		{
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.0, yaw_px, pitch_px);
			let _ = apply_blink_binding(puppet, id, "Eye:: Right:: Open", 0.65);
			let _ = apply_eye_gaze_binding(puppet, id, "Eye:: Gaze", yaw_px * 0.2, pitch_px * 0.2);
			actions.push(format!("Rigged right eye '{raw_name}' (perspective + blink + gaze)"));
		} else if lower.contains("mouth") || lower.contains("mouse") || lower.contains("lip") || lower.contains("嘴") || lower.contains("口") {
			if is_tiny {
				let _ = apply_rigid_parallax_binding(puppet, id, "Head:: Yaw-Pitch", 1.1, yaw_px, pitch_px);
			} else {
				let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.1, yaw_px, pitch_px);
			}
			let _ = apply_mouth_open_binding(puppet, id, "Mouth:: Open", mouth_open_px);
			let _ = apply_mouth_form_binding(puppet, id, "Mouth:: Form", mouth_open_px * 0.5, mouth_open_px * 0.4);
			actions.push(format!("Rigged mouth '{raw_name}' (perspective + open + form)"));
		} else if lower.contains("nose") || lower.contains("鼻") {
			let _ = apply_rigid_parallax_binding(puppet, id, "Head:: Yaw-Pitch", 1.35, yaw_px, pitch_px);
			actions.push(format!("Rigged nose '{raw_name}' (rigid parallax depth_z=1.35)"));
		} else if lower.contains("brow") || lower.contains("眉") {
			if is_tiny {
				let _ = apply_rigid_parallax_binding(puppet, id, "Head:: Yaw-Pitch", 1.05, yaw_px, pitch_px);
			} else {
				let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.05, yaw_px, pitch_px);
			}
			let is_right = detect_side_with_ancestors(puppet, id, &lower).unwrap_or(false);
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

	// Auto-attach eyeball masks onto `Drawable.masks` for pupils/highlights with no masks yet
	for (id, raw_name, side) in mask_targets {
		let mask_src = match side {
			Some(false) => left_eyeball_id,
			Some(true) => right_eyeball_id,
			None => None,
		};
		let Some(source) = mask_src else {
			continue;
		};
		if let Some(d) = puppet.node_comps.get_mut::<Drawable>(id) {
			let already_masked = d.masks.as_ref().map(|m| !m.masks.is_empty()).unwrap_or(false);
			if !already_masked {
				d.masks = Some(Masks {
					threshold: 0.5,
					masks: vec![Mask {
						source,
						mode: MaskMode::Mask,
					}],
				});
				actions.push(format!("Auto-masked '{raw_name}' to eyeball node {}", source.0));
			}
		}
	}

	let has_hair_bindings = puppet
		.params
		.get("Hair:: Sway")
		.map(|p| !p.bindings.is_empty())
		.unwrap_or(false);

	if let Some(head_id) = head_anchor {
		let _ = apply_head_roll_binding(puppet, head_id, "Head:: Roll", 15.0);
		if !has_physics && has_hair_bindings {
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
	} else if !has_physics && has_hair_bindings {
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

	actions.extend(auto_bind_hidden_expression_groups(puppet));

	create_idle_animation(puppet);
	actions.push("Created 'Idle' animation (120 frames @ 30fps)".to_owned());
	let fix_res = crate::audit::auto_fix_puppet(puppet);
	actions.push(format!("Auto-fixed constraints: {}", json::stringify(fix_res)));
	actions
}

/// Convert hidden PSD differential expression groups (`sweat`, `annoyance`, `heart`, `star`, etc.)
/// into 0..1 `Expr:: <Group>` opacity parameters with default opacity `0.0`.
pub fn auto_bind_hidden_expression_groups(puppet: &mut Puppet) -> Vec<String> {
	const EXPR_KEYS: &[&str] = &[
		"sweat",
		"annoyance",
		"heart",
		"star",
		"spin",
		"serious",
		"blush",
		"tear",
		"angry",
		"shock",
		"汗",
		"怒",
		"淚",
		"愛心",
		"星星",
	];
	let hidden_groups: Vec<(InoxNodeUuid, String)> = puppet
		.nodes
		.pre_order_iter()
		.skip(1)
		.filter_map(|n| {
			if n.enabled || puppet.node_comps.get::<Mesh>(n.uuid).is_some() {
				return None;
			}
			let lower = n.name.to_lowercase();
			if EXPR_KEYS.iter().any(|k| lower.contains(k)) {
				Some((n.uuid, n.name.clone()))
			} else {
				None
			}
		})
		.collect();

	let mut actions = Vec::new();
	for (gid, gname) in hidden_groups {
		let part_ids: Vec<InoxNodeUuid> = puppet
			.nodes
			.pre_order_iter()
			.filter_map(|n| {
				if puppet.nodes.is_descendant(n.uuid, gid)
					&& puppet.node_comps.get::<Mesh>(n.uuid).is_some()
				{
					Some(n.uuid)
				} else {
					None
				}
			})
			.collect();
		if part_ids.is_empty() {
			continue;
		}
		if let Some(g) = puppet.nodes.get_node_mut(gid) {
			g.enabled = true;
		}
		let param_name = format!("Expr:: {gname}");
		for pid in &part_ids {
			let _ = apply_opacity_binding(puppet, *pid, &param_name, 0.0, 1.0);
		}
		actions.push(format!(
			"Converted hidden expression group '{gname}' ({} parts) into '{param_name}'",
			part_ids.len()
		));
	}
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::rig::sample_model::build_sample_vtuber_model;

	#[test]
	fn auto_rig_handles_romaji_mouth_split_eyes_automask_and_tiny_nose() {
		let mut model = build_sample_vtuber_model();
		let root = model.puppet.nodes.root_node_id;
		let img_small = RgbaImage::from_pixel(12, 12, Rgba([255, 255, 255, 255]));
		let img_eye = RgbaImage::from_pixel(40, 24, Rgba([255, 255, 255, 255]));
		let img_mouth = RgbaImage::from_pixel(36, 20, Rgba([255, 180, 180, 255]));

		let ew_l = add_grid_part(&mut model, root, "eye_white_L", 0.0, vec2(-30.0, -10.0), 40.0, 24.0, 3, 3, img_eye);
		let pupil_l = add_grid_part(&mut model, root, "pupil_L", -0.1, vec2(-30.0, -10.0), 12.0, 12.0, 3, 3, img_small.clone());
		let hl_l = add_grid_part(&mut model, root, "highlight_L", -0.2, vec2(-28.0, -12.0), 12.0, 12.0, 3, 3, img_small.clone());
		let mouth = add_grid_part(&mut model, root, "mouse_inside", 0.0, vec2(0.0, 35.0), 36.0, 20.0, 3, 3, img_mouth);
		let nose = add_grid_part(&mut model, root, "nose", -0.3, vec2(0.0, 5.0), 10.0, 10.0, 3, 3, img_small);

		let _actions = auto_rig_vtuber(&mut model.puppet);

		// Mouth:: Open should include mouse_inside
		let mouth_open = &model.puppet.params["Mouth:: Open"];
		assert!(mouth_open.bindings.iter().any(|b| b.node == mouth));

		// Eye:: Left:: Open should include eye_white_L and pupil_L
		let eye_l = &model.puppet.params["Eye:: Left:: Open"];
		assert!(eye_l.bindings.iter().any(|b| b.node == ew_l));
		assert!(eye_l.bindings.iter().any(|b| b.node == pupil_l));

		// pupil_L and highlight_L should be auto-masked by eye_white_L via Drawable.masks
		let m_pupil = model
			.puppet
			.node_comps
			.get::<Drawable>(pupil_l)
			.and_then(|d| d.masks.as_ref())
			.expect("pupil_L Drawable.masks");
		assert!(m_pupil.masks.iter().any(|m| m.source == ew_l));
		let m_hl = model
			.puppet
			.node_comps
			.get::<Drawable>(hl_l)
			.and_then(|d| d.masks.as_ref())
			.expect("highlight_L Drawable.masks");
		assert!(m_hl.masks.iter().any(|m| m.source == ew_l));

		// Tiny nose binding should be rigid translation (all vertices in Deform keypoint have identical offset)
		let head_yp = &model.puppet.params["Head:: Yaw-Pitch"];
		let nose_binding = head_yp
			.bindings
			.iter()
			.find(|b| b.node == nose)
			.expect("nose bound to Head:: Yaw-Pitch");
		if let BindingValues::Deform(ref mat) = nose_binding.values {
			let corner = &mat[(0, 0)];
			assert!(corner.windows(2).all(|w| (w[0] - w[1]).length() < 1e-4));
		} else {
			panic!("expected Deform binding for nose");
		}
	}

	#[test]
	fn hidden_expression_group_converts_to_opacity_toggle() {
		let mut model = build_sample_vtuber_model();
		let root = model.puppet.nodes.root_node_id;
		let grp_id = InoxNodeUuid(9001);
		model.puppet.nodes.add(
			root,
			grp_id,
			riglens_core::node::InoxNode {
				uuid: grp_id,
				name: "sweat".to_owned(),
				enabled: false,
				zsort: 0.0,
				trans_offset: Default::default(),
				lock_to_root: false,
			},
		);
		let img = RgbaImage::from_pixel(16, 16, Rgba([100, 180, 255, 255]));
		let drop_id = add_grid_part(&mut model, grp_id, "sweat1", -1.0, vec2(-20.0, -20.0), 16.0, 16.0, 2, 2, img);

		let _ = auto_rig_vtuber(&mut model.puppet);

		let p = model.puppet.params.get("Expr:: sweat").expect("Expr:: sweat created");
		assert!(p.bindings.iter().any(|b| b.node == drop_id));
		assert!(model.puppet.nodes.get_node(grp_id).unwrap().enabled);
	}

	#[test]
	fn ancestor_side_detection_and_no_orphan_hair_physics() {
		let mut model = riglens_core::model::Model {
			puppet: Puppet::new_empty("JpEyeNoHair"),
			textures: Vec::new(),
			vendors: Vec::new(),
		};
		let root = model.puppet.nodes.root_node_id;
		let left_eye_grp = InoxNodeUuid(9100);
		model.puppet.nodes.add(
			root,
			left_eye_grp,
			riglens_core::node::InoxNode {
				uuid: left_eye_grp,
				name: "左目".to_owned(),
				enabled: true,
				zsort: 0.0,
				trans_offset: Default::default(),
				lock_to_root: false,
			},
		);
		let img = RgbaImage::from_pixel(20, 20, Rgba([255, 255, 255, 255]));
		let sclera = add_grid_part(&mut model, left_eye_grp, "白目", 0.0, vec2(-20.0, -10.0), 20.0, 20.0, 2, 2, img.clone());
		let pupil = add_grid_part(&mut model, left_eye_grp, "瞳", -0.1, vec2(-20.0, -10.0), 10.0, 10.0, 2, 2, img);

		let _ = auto_rig_vtuber(&mut model.puppet);

		let d_masks = model
			.puppet
			.node_comps
			.get::<Drawable>(pupil)
			.and_then(|d| d.masks.as_ref())
			.expect("pupil auto-masked by ancestor side");
		assert!(d_masks.masks.iter().any(|m| m.source == sclera));
		assert!(
			!model
				.puppet
				.nodes
				.iter()
				.any(|n| model.puppet.node_comps.get::<SimplePhysics>(n.uuid).is_some()),
			"must not attach Hair_Physics when no hair parts exist"
		);
	}
}

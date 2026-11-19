use std::collections::{HashMap, HashSet};

use glam::{vec2, vec3, Mat4, Vec2};
use inox2d::model::Model;
use inox2d::node::components::{
	Composite, Drawable, GridDeformer, Mask, MaskMode, Masks, Mesh, MeshGroup, PathDeformer, SimplePhysics, TexturedMesh,
};
use inox2d::node::InoxNodeUuid;
use inox2d::params::BindingValues;
use inox2d::puppet::Puppet;
use inox2d::render::TexturedMeshRenderCtx;
use json::JsonValue;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FeatureRole {
	Face,
	EyeLeft,
	EyeRight,
	Mouth,
	Nose,
	Brow,
	HairBack,
	HairFront,
	HairSide,
	Ear,
	PupilOrIris,
	Tongue,
	Body,
	Other,
}

impl FeatureRole {
	pub fn from_name(name: &str) -> Self {
		let l = name.to_lowercase();
		if l.contains("pupil") || l.contains("iris") || l.contains("highlight") || l.contains("瞳") || l.contains("眼珠") || l.contains("高光") {
			Self::PupilOrIris
		} else if l.contains("tongue") || l.contains("舌") {
			Self::Tongue
		} else if l.contains("hair_back") || l.contains("back_hair") || l.contains("後髮") || l.contains("后发") {
			Self::HairBack
		} else if l.contains("hair_front") || l.contains("bang") || l.contains("ahoge") || l.contains("前髮") || l.contains("劉海") || l.contains("刘海") || l.contains("呆毛") {
			Self::HairFront
		} else if l.contains("hair_side") || l.contains("side_hair") || l.contains("側髮") || l.contains("鬢角") {
			Self::HairSide
		} else if l.contains("eye_l") || l.contains("left_eye") || l.contains("左眼") {
			Self::EyeLeft
		} else if l.contains("eye_r") || l.contains("right_eye") || l.contains("右眼") {
			Self::EyeRight
		} else if l.contains("mouth") || l.contains("lip") || l.contains("嘴") || l.contains("口") {
			Self::Mouth
		} else if l.contains("nose") || l.contains("鼻") {
			Self::Nose
		} else if l.contains("brow") || l.contains("眉") {
			Self::Brow
		} else if l.contains("ear") || l.contains("耳") {
			Self::Ear
		} else if l.contains("face") || l.contains("head_base") || l.contains("臉") || l.contains("脸") {
			Self::Face
		} else if l.contains("torso") || l.contains("body") || l.contains("身體") || l.contains("衣") {
			Self::Body
		} else {
			Self::Other
		}
	}

	pub fn is_facial_feature(self) -> bool {
		matches!(
			self,
			Self::EyeLeft | Self::EyeRight | Self::Mouth | Self::Nose | Self::Brow | Self::PupilOrIris | Self::Tongue
		)
	}

	pub fn as_str(self) -> &'static str {
		match self {
			Self::Face => "face",
			Self::EyeLeft => "eye_left",
			Self::EyeRight => "eye_right",
			Self::Mouth => "mouth",
			Self::Nose => "nose",
			Self::Brow => "brow",
			Self::HairBack => "hair_back",
			Self::HairFront => "hair_front",
			Self::HairSide => "hair_side",
			Self::Ear => "ear",
			Self::PupilOrIris => "pupil_or_iris",
			Self::Tongue => "tongue",
			Self::Body => "body",
			Self::Other => "other",
		}
	}
}

fn eval_puppet_at(puppet: &mut Puppet, overrides: &HashMap<String, Vec2>) {
	if puppet.render_ctx.is_none()
		|| puppet
			.param_ctx
			.as_ref()
			.map(|c| c.values().len() != puppet.params.len())
			.unwrap_or(true)
	{
		puppet.init_all();
	}
	puppet.begin_frame();
	if let Some(pctx) = puppet.param_ctx.as_mut() {
		for (k, v) in overrides {
			let _ = pctx.set(k, *v);
		}
	}
	puppet.end_frame(0.0);
}

fn world_vertices_for_node(puppet: &Puppet, id: InoxNodeUuid, with_deform: bool) -> Option<(Vec<Vec2>, Vec<u32>, Mat4)> {
	let mesh = puppet.node_comps.get::<Mesh>(id)?;
	let abs = puppet.node_transform(id)?.absolute;
	let deforms: Option<&[Vec2]> = if with_deform {
		if let (Some(rc), Some(tm_rc)) = (
			puppet.render_ctx.as_ref(),
			puppet.node_comps.get::<TexturedMeshRenderCtx>(id),
		) {
			let off = tm_rc.vert_offset as usize;
			let len = tm_rc.vert_len;
			rc.vertex_buffers.deforms.get(off..off + len)
		} else {
			None
		}
	} else {
		None
	};

	let verts = mesh
		.vertices
		.iter()
		.enumerate()
		.map(|(i, &v)| {
			let d = deforms.and_then(|ds| ds.get(i)).copied().unwrap_or(Vec2::ZERO);
			abs.transform_point3(vec3(v.x - mesh.origin.x + d.x, v.y - mesh.origin.y + d.y, 0.0))
				.truncate()
		})
		.collect();

	Some((verts, mesh.indices.clone(), abs))
}

fn bbox_of(pts: &[Vec2]) -> (Vec2, Vec2) {
	let mut mn = vec2(f32::INFINITY, f32::INFINITY);
	let mut mx = vec2(f32::NEG_INFINITY, f32::NEG_INFINITY);
	for &p in pts {
		mn = mn.min(p);
		mx = mx.max(p);
	}
	(mn, mx)
}

fn tri_signed_area(a: Vec2, b: Vec2, c: Vec2) -> f32 {
	0.5 * ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x))
}

pub fn find_face_node(puppet: &Puppet) -> Option<InoxNodeUuid> {
	let mut best: Option<(InoxNodeUuid, f32)> = None;
	for node in puppet.nodes.iter() {
		if puppet.node_comps.get::<Mesh>(node.uuid).is_none() {
			continue;
		}
		if FeatureRole::from_name(&node.name) == FeatureRole::Face {
			return Some(node.uuid);
		}
		if let Some(m) = puppet.node_comps.get::<Mesh>(node.uuid) {
			let (mn, mx) = bbox_of(&m.vertices);
			let area = (mx.x - mn.x) * (mx.y - mn.y);
			if best.map_or(true, |(_, a)| area > a) {
				best = Some((node.uuid, area));
			}
		}
	}
	best.map(|(id, _)| id)
}

/// Analyze all Part nodes at rest pose relative to the Face bounding box.
pub fn analyze_anatomy(puppet: &mut Puppet) -> JsonValue {
	eval_puppet_at(puppet, &HashMap::new());
	let face_id = find_face_node(puppet);

	let (face_mn, face_mx, face_name) = if let Some(fid) = face_id {
		let (pts, _, _) = world_vertices_for_node(puppet, fid, false).unwrap();
		let (mn, mx) = bbox_of(&pts);
		let name = puppet
			.nodes
			.get_node(fid)
			.map(|n| n.name.trim_end_matches('\0').to_owned())
			.unwrap_or_default();
		(mn, mx, name)
	} else {
		(vec2(-100.0, -100.0), vec2(100.0, 100.0), "<none>".to_owned())
	};

	let face_c = (face_mn + face_mx) * 0.5;
	let face_size = (face_mx - face_mn).max(vec2(1.0, 1.0));
	let face_half = face_size * 0.5;

	let mut parts_json = Vec::new();
	let mut rest_warnings = Vec::new();

	let part_ids: Vec<(InoxNodeUuid, String, bool)> = puppet
		.nodes
		.pre_order_iter()
		.filter_map(|n| {
			if puppet.node_comps.get::<TexturedMesh>(n.uuid).is_some() {
				Some((n.uuid, n.name.trim_end_matches('\0').to_owned(), n.enabled))
			} else {
				None
			}
		})
		.collect();

	for (id, name, enabled) in part_ids {
		let Some((pts, _, _)) = world_vertices_for_node(puppet, id, false) else {
			continue;
		};
		let (mn, mx) = bbox_of(&pts);
		let c = (mn + mx) * 0.5;
		let sz = mx - mn;
		let role = FeatureRole::from_name(&name);

		let rel_x = (c.x - face_c.x) / face_half.x;
		let rel_y = (c.y - face_c.y) / face_half.y;
		let margin_left = mn.x - face_mn.x;
		let margin_right = face_mx.x - mx.x;
		let margin_top = mn.y - face_mn.y;
		let margin_bottom = face_mx.y - mx.y;

		if role.is_facial_feature() {
			let min_margin = margin_left.min(margin_right).min(margin_top).min(margin_bottom);
			if min_margin < 4.0 {
				rest_warnings.push(format!(
					"Facial feature '{name}' is close to/outside '{face_name}' boundary at rest (min margin {min_margin:.1}px)"
				));
			}
		}

		parts_json.push(json::object! {
			uuid: id.0,
			name: name,
			role: role.as_str(),
			enabled: enabled,
			center: [c.x, c.y],
			size: [sz.x, sz.y],
			face_normalized_pos: [rel_x, rel_y],
			face_margins_px: {
				left: margin_left,
				right: margin_right,
				top: margin_top,
				bottom: margin_bottom
			}
		});
	}

	json::object! {
		face_anchor: {
			name: face_name,
			center: [face_c.x, face_c.y],
			size: [face_size.x, face_size.y]
		},
		parts: parts_json,
		rest_warnings: rest_warnings
	}
}

/// Perform a 6-point deterministic visual/geometric rig audit without guessing.
pub fn audit_rig(model: &mut Model) -> JsonValue {
	let puppet = &mut model.puppet;
	eval_puppet_at(puppet, &HashMap::new());

	let face_id = find_face_node(puppet);
	let part_nodes: Vec<(InoxNodeUuid, String)> = puppet
		.nodes
		.pre_order_iter()
		.filter_map(|n| {
			if n.enabled && puppet.node_comps.get::<TexturedMesh>(n.uuid).is_some() {
				Some((n.uuid, n.name.trim_end_matches('\0').to_owned()))
			} else {
				None
			}
		})
		.collect();

	// Record rest triangle signed areas & edge lengths, plus triangles already crushed at neutral rest
	struct RestGeom {
		verts: Vec<Vec2>,
		indices: Vec<u32>,
		tri_areas: Vec<f32>,
	}
	let mut rest_map: HashMap<InoxNodeUuid, RestGeom> = HashMap::new();
	let mut neutral_crushed: HashMap<InoxNodeUuid, std::collections::HashSet<usize>> = HashMap::new();
	for &(id, _) in &part_nodes {
		if let Some((verts, indices, _)) = world_vertices_for_node(puppet, id, false) {
			let mut tri_areas = Vec::with_capacity(indices.len() / 3);
			for tri in indices.chunks_exact(3) {
				let a = tri_signed_area(verts[tri[0] as usize], verts[tri[1] as usize], verts[tri[2] as usize]);
				tri_areas.push(a);
			}
			if let Some((nverts, _, _)) = world_vertices_for_node(puppet, id, true) {
				let mut set = std::collections::HashSet::new();
				for (ti, tri) in indices.chunks_exact(3).enumerate() {
					let ra = tri_areas[ti];
					if ra.abs() < 1e-4 {
						continue;
					}
					let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
					let na = tri_signed_area(nverts[i0], nverts[i1], nverts[i2]);
					if na * ra >= 0.0 && na.abs() < ra.abs() * 0.025 {
						set.insert(ti);
					}
				}
				if !set.is_empty() {
					neutral_crushed.insert(id, set);
				}
			}
			rest_map.insert(id, RestGeom { verts, indices, tri_areas });
		}
	}

	let mut containment_leaks = Vec::new();
	let mut mesh_foldovers = Vec::new();
	let mut mesh_stretch = Vec::new();
	let mut unmasked_parts = Vec::new();
	let mut recommendations = Vec::new();

	// Build pose sweep list
	let mut poses: Vec<(String, HashMap<String, Vec2>, bool)> = Vec::new();
	if let Some(hp) = puppet.params.get("Head:: Yaw-Pitch").cloned() {
		for iy in [-1.0f32, 0.0, 1.0] {
			for ix in [-1.0f32, 0.0, 1.0] {
				let mut m = HashMap::new();
				let vx = if ix < 0.0 { hp.min.x } else if ix > 0.0 { hp.max.x } else { hp.defaults.x };
				let vy = if iy < 0.0 { hp.min.y } else if iy > 0.0 { hp.max.y } else { hp.defaults.y };
				m.insert("Head:: Yaw-Pitch".to_owned(), vec2(vx, vy));
				poses.push((format!("Head({vx:.1},{vy:.1})"), m, false));
			}
		}
	} else {
		poses.push(("Rest".to_owned(), HashMap::new(), false));
	}

	if puppet.params.contains_key("Eye:: Left:: Open") || puppet.params.contains_key("Eye:: Right:: Open") {
		let mut m = HashMap::new();
		if puppet.params.contains_key("Eye:: Left:: Open") {
			m.insert("Eye:: Left:: Open".to_owned(), vec2(0.0, 0.0));
		}
		if puppet.params.contains_key("Eye:: Right:: Open") {
			m.insert("Eye:: Right:: Open".to_owned(), vec2(0.0, 0.0));
		}
		poses.push(("Blink(0.0)".to_owned(), m, true));
	}

	if puppet.params.contains_key("Mouth:: Open") {
		let mut m = HashMap::new();
		m.insert("Mouth:: Open".to_owned(), vec2(1.0, 0.0));
		poses.push(("MouthOpen(1.0)".to_owned(), m, false));
	}

	let mut left_face_shift_x = None;
	let mut right_face_shift_x = None;

	for (pose_name, pose_params, is_blink) in &poses {
		eval_puppet_at(puppet, pose_params);

		let face_box = face_id.and_then(|fid| {
			let (pts, _, _) = world_vertices_for_node(puppet, fid, true)?;
			let (mn, mx) = bbox_of(&pts);
			Some(((mn + mx) * 0.5, ((mx - mn) * 0.5).max(vec2(1.0, 1.0))))
		});

		if let Some((fc, _)) = face_box {
			if pose_name.starts_with("Head(-1.") {
				left_face_shift_x = Some(fc.x);
			} else if pose_name.starts_with("Head(1.") {
				right_face_shift_x = Some(fc.x);
			}
		}

		for &(id, ref name) in &part_nodes {
			let Some(rest) = rest_map.get(&id) else {
				continue;
			};
			let Some((verts, _, _)) = world_vertices_for_node(puppet, id, true) else {
				continue;
			};
			let role = FeatureRole::from_name(name);

			// Check 1: Face containment for facial features
			if role.is_facial_feature() {
				if let Some((fc, fh)) = face_box {
					let safe_rx = fh.x * 0.96;
					let safe_ry = fh.y * 0.98;
					let mut max_overflow = 0.0f32;
					let mut worst_dir = "";
					for &v in &verts {
						let dx = v.x - fc.x;
						let dy = v.y - fc.y;
						let norm = (dx / safe_rx).powi(2) + (dy / safe_ry).powi(2);
						if norm > 1.0 {
							let scale_back = 1.0 / norm.sqrt();
							let over = ((dx * (1.0 - scale_back)).powi(2) + (dy * (1.0 - scale_back)).powi(2)).sqrt();
							if over > max_overflow {
								max_overflow = over;
								worst_dir = if dx.abs() > dy.abs() {
									if dx < 0.0 { "left" } else { "right" }
								} else if dy < 0.0 {
									"top"
								} else {
									"bottom"
								};
							}
						}
					}
					if max_overflow > 2.0 {
						containment_leaks.push(json::object! {
							pose: pose_name.as_str(),
							node: name.as_str(),
							overflow_px: max_overflow,
							direction: worst_dir
						});
					}
				}
			}

			// Check 2 & 3: Mesh foldovers & extreme edge stretching
			let mut fold_count = 0usize;
			let mut max_stretch = 1.0f32;
			let crush_thresh = if *is_blink { 0.002 } else { 0.025 };

			for (ti, tri) in rest.indices.chunks_exact(3).enumerate() {
				let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
				let ra = rest.tri_areas[ti];
				if ra.abs() < 1e-4 {
					continue;
				}
				let da = tri_signed_area(verts[i0], verts[i1], verts[i2]);
				let is_neutral_crushed = neutral_crushed
					.get(&id)
					.map(|s| s.contains(&ti))
					.unwrap_or(false);
				if da * ra < 0.0 || (!is_neutral_crushed && da.abs() < ra.abs() * crush_thresh) {
					fold_count += 1;
				}
				for (a, b) in [(i0, i1), (i1, i2), (i2, i0)] {
					let rl = (rest.verts[b] - rest.verts[a]).length();
					if rl > 1.0 {
						let dl = (verts[b] - verts[a]).length();
						max_stretch = max_stretch.max(dl / rl);
					}
				}
			}

			if fold_count > 0 {
				mesh_foldovers.push(json::object! {
					pose: pose_name.as_str(),
					node: name.as_str(),
					flipped_or_crushed_tris: fold_count
				});
			}
			if max_stretch > 2.8 {
				mesh_stretch.push(json::object! {
					pose: pose_name.as_str(),
					node: name.as_str(),
					max_stretch_ratio: max_stretch
				});
			}
		}
	}

	// Check 4: Blink closure & symmetry
	let mut blink_issues = Vec::new();
	let mut closed_cy_l = None;
	let mut closed_cy_r = None;
	for (param_name, target_role, label) in [
		("Eye:: Left:: Open", FeatureRole::EyeLeft, "left_eye"),
		("Eye:: Right:: Open", FeatureRole::EyeRight, "right_eye"),
	] {
		if puppet.params.contains_key(param_name) {
			let mut m = HashMap::new();
			m.insert(param_name.to_owned(), vec2(0.0, 0.0));
			eval_puppet_at(puppet, &m);

			for &(id, ref name) in &part_nodes {
				if FeatureRole::from_name(name) == target_role {
					if let (Some(rest), Some((verts, _, _))) =
						(rest_map.get(&id), world_vertices_for_node(puppet, id, true))
					{
						let (rmn, rmx) = bbox_of(&rest.verts);
						let (dmn, dmx) = bbox_of(&verts);
						let rest_h = (rmx.y - rmn.y).max(1.0);
						let def_h = (dmx.y - dmn.y).max(0.0);
						let ratio = def_h / rest_h;
						let cy = (dmn.y + dmx.y) * 0.5;
						if target_role == FeatureRole::EyeLeft {
							closed_cy_l = Some(cy);
						} else {
							closed_cy_r = Some(cy);
						}
						if ratio > 0.35 {
							blink_issues.push(json::object! {
								eye: label,
								node: name.as_str(),
								closed_height_ratio: ratio,
								issue: "Eyelid does not compress enough when Open=0.0 (expected <= 0.35)"
							});
						}
					}
				}
			}
		}
	}
	if let (Some(yl), Some(yr)) = (closed_cy_l, closed_cy_r) {
		if (yl - yr).abs() > 8.0 {
			blink_issues.push(json::object! {
				issue: format!("Closed eye height mismatch between L ({yl:.1}) and R ({yr:.1})")
			});
		}
	}

	// Check 5: Symmetry check
	let mut symmetry_defects = Vec::new();
	if let (Some(lx), Some(rx)) = (left_face_shift_x, right_face_shift_x) {
		eval_puppet_at(puppet, &HashMap::new());
		if let Some(cx) = face_id
			.and_then(|fid| world_vertices_for_node(puppet, fid, false))
			.map(|(pts, _, _)| {
				let (mn, mx) = bbox_of(&pts);
				(mn.x + mx.x) * 0.5
			}) {
			let asym = ((lx - cx) + (rx - cx)).abs();
			if asym > 6.0 {
				symmetry_defects.push(json::object! {
					node: "Face",
					yaw_asymmetry_px: asym
				});
			}
		}
	}

	// Check 6: Unmasked pupils/iris/tongue
	for &(id, ref name) in &part_nodes {
		let role = FeatureRole::from_name(name);
		if matches!(role, FeatureRole::PupilOrIris | FeatureRole::Tongue) {
			let has_mask = puppet
				.node_comps
				.get::<Drawable>(id)
				.and_then(|d| d.masks.as_ref())
				.map(|m| !m.masks.is_empty())
				.unwrap_or(false);
			if !has_mask {
				unmasked_parts.push(name.clone());
			}
		}
	}

	// Check 7: Deformer structural integrity (GridDeformer / PathDeformer / MeshGroup)
	let mut deformer_issues: Vec<JsonValue> = Vec::new();
	for node in puppet.nodes.pre_order_iter() {
		if !node.enabled {
			continue;
		}
		if let Some(grid) = puppet.node_comps.get::<GridDeformer>(node.uuid) {
			if grid.axis_x.len() < 2 || grid.axis_y.len() < 2 {
				deformer_issues.push(json::object! {
					node: node.name.trim_end_matches('\0').to_owned(),
					node_uuid: node.uuid.0,
					deformer_type: "GridDeformer",
					issue: "degenerate_grid_axes"
				});
			} else if !grid.axis_x.windows(2).all(|w| w[0].is_finite() && w[1].is_finite() && w[0] < w[1])
				|| !grid.axis_y.windows(2).all(|w| w[0].is_finite() && w[1].is_finite() && w[0] < w[1])
			{
				deformer_issues.push(json::object! {
					node: node.name.trim_end_matches('\0').to_owned(),
					node_uuid: node.uuid.0,
					deformer_type: "GridDeformer",
					issue: "non_monotonic_axes"
				});
			}
			if !has_deformable_descendant(puppet, node.uuid) {
				deformer_issues.push(json::object! {
					node: node.name.trim_end_matches('\0').to_owned(),
					node_uuid: node.uuid.0,
					deformer_type: "GridDeformer",
					issue: "no_deformable_children"
				});
			}
		}
		if let Some(path) = puppet.node_comps.get::<PathDeformer>(node.uuid) {
			if path.vertices.len() < 2 {
				deformer_issues.push(json::object! {
					node: node.name.trim_end_matches('\0').to_owned(),
					node_uuid: node.uuid.0,
					deformer_type: "PathDeformer",
					issue: "degenerate_path_vertices"
				});
			} else if path.vertices.iter().any(|v| !v.is_finite()) {
				deformer_issues.push(json::object! {
					node: node.name.trim_end_matches('\0').to_owned(),
					node_uuid: node.uuid.0,
					deformer_type: "PathDeformer",
					issue: "non_finite_vertices"
				});
			}
			if !has_deformable_descendant(puppet, node.uuid) {
				deformer_issues.push(json::object! {
					node: node.name.trim_end_matches('\0').to_owned(),
					node_uuid: node.uuid.0,
					deformer_type: "PathDeformer",
					issue: "no_deformable_children"
				});
			}
		}
		if puppet.node_comps.get::<MeshGroup>(node.uuid).is_some() {
			let bad_mesh = puppet
				.node_comps
				.get::<Mesh>(node.uuid)
				.map_or(true, |m| m.vertices.len() < 3 || m.indices.len() < 3);
			if bad_mesh {
				deformer_issues.push(json::object! {
					node: node.name.trim_end_matches('\0').to_owned(),
					node_uuid: node.uuid.0,
					deformer_type: "MeshGroup",
					issue: "missing_or_empty_mesh"
				});
			}
			if !has_deformable_descendant(puppet, node.uuid) {
				deformer_issues.push(json::object! {
					node: node.name.trim_end_matches('\0').to_owned(),
					node_uuid: node.uuid.0,
					deformer_type: "MeshGroup",
					issue: "no_deformable_children"
				});
			}
		}
	}

	// Check 8: Binding coverage & integrity
	let mut binding_coverage: Vec<JsonValue> = Vec::new();
	let mut bound_nodes: HashSet<InoxNodeUuid> = HashSet::new();
	let mut param_names: Vec<&String> = puppet.params.keys().collect();
	param_names.sort();
	for param_name in param_names {
		let param = &puppet.params[param_name];
		for binding in &param.bindings {
			bound_nodes.insert(binding.node);
			let target_node = puppet.nodes.get_node(binding.node);
			let Some(target) = target_node else {
				binding_coverage.push(json::object! {
					param: param_name.clone(),
					node_uuid: binding.node.0,
					issue: "dangling_target_node"
				});
				continue;
			};

			let is_set_nx = if binding.is_set.is_transposed() {
				binding.is_set.height()
			} else {
				binding.is_set.width()
			};
			let is_set_ny = if binding.is_set.is_transposed() {
				binding.is_set.width()
			} else {
				binding.is_set.height()
			};
			let mut any_set = false;
			for ix in 0..is_set_nx {
				for iy in 0..is_set_ny {
					if binding.is_set.get(ix, iy) == Some(&true) {
						any_set = true;
						break;
					}
				}
				if any_set {
					break;
				}
			}
			if !any_set {
				binding_coverage.push(json::object! {
					param: param_name.clone(),
					node: target.name.trim_end_matches('\0').to_owned(),
					node_uuid: binding.node.0,
					issue: "unset_binding"
				});
			}

			if let BindingValues::Deform(matrix) = &binding.values {
				let expected = puppet
					.node_comps
					.get::<Mesh>(binding.node)
					.map(|m| m.vertices.len())
					.or_else(|| {
						puppet
							.node_comps
							.get::<GridDeformer>(binding.node)
							.map(|g| g.vertex_count())
					})
					.or_else(|| {
						puppet
							.node_comps
							.get::<PathDeformer>(binding.node)
							.map(|p| p.vertex_count())
					});

				match expected {
					None => {
						binding_coverage.push(json::object! {
							param: param_name.clone(),
							node: target.name.trim_end_matches('\0').to_owned(),
							node_uuid: binding.node.0,
							issue: "deform_on_non_deformable_node"
						});
					}
					Some(exp_len) => {
						let mut bad_vec: Option<usize> = None;
						for ix in 0..is_set_nx {
							for iy in 0..is_set_ny {
								if binding.is_set.get(ix, iy) == Some(&true) {
									if let Some(v) = matrix.get(ix, iy) {
										if v.len() != exp_len {
											bad_vec = Some(v.len());
											break;
										}
									}
								}
							}
							if bad_vec.is_some() {
								break;
							}
						}
						if let Some(actual_len) = bad_vec {
							binding_coverage.push(json::object! {
								param: param_name.clone(),
								node: target.name.trim_end_matches('\0').to_owned(),
								node_uuid: binding.node.0,
								expected_vertices: exp_len,
								actual_vertices: actual_len,
								issue: "deform_vertex_mismatch"
							});
						}
					}
				}
			}
		}
	}

	// Deformers with zero direct parameter bindings have no effect
	for node in puppet.nodes.pre_order_iter() {
		if !node.enabled {
			continue;
		}
		let is_deformer = puppet.node_comps.get::<GridDeformer>(node.uuid).is_some()
			|| puppet.node_comps.get::<PathDeformer>(node.uuid).is_some()
			|| puppet.node_comps.get::<MeshGroup>(node.uuid).is_some();
		if is_deformer && !bound_nodes.contains(&node.uuid) {
			binding_coverage.push(json::object! {
				node: node.name.trim_end_matches('\0').to_owned(),
				node_uuid: node.uuid.0,
				issue: "unbound_deformer"
			});
		}
		if let Some(sp) = puppet.node_comps.get::<SimplePhysics>(node.uuid) {
			match puppet.params.values().find(|p| p.uuid == sp.param) {
				None => {
					binding_coverage.push(json::object! {
						node: node.name.trim_end_matches('\0').to_owned(),
						node_uuid: node.uuid.0,
						param_uuid: sp.param.0,
						issue: "dangling_physics_param"
					});
				}
				Some(p) if p.bindings.is_empty() => {
					binding_coverage.push(json::object! {
						node: node.name.trim_end_matches('\0').to_owned(),
						node_uuid: node.uuid.0,
						param: p.name.clone(),
						issue: "physics_drives_unbound_param"
					});
				}
				Some(_) => {}
			}
		}
	}

	// Restore rest state before checking draw order
	eval_puppet_at(puppet, &HashMap::new());

	// Check 9: Draw-order conflicts (overlapping equal zsort, anatomical face zsort inversions)
	let mut draw_order_conflicts: Vec<JsonValue> = Vec::new();
	struct DrawableZInfo {
		uuid: InoxNodeUuid,
		name: String,
		zsort: f32,
		min: Vec2,
		max: Vec2,
		is_mask_helper: bool,
	}
	let mut drawable_z: Vec<DrawableZInfo> = Vec::new();
	for node in puppet.nodes.pre_order_iter() {
		if !node.enabled || puppet.node_comps.get::<Drawable>(node.uuid).is_none() {
			continue;
		}
		if let Some((pts, _, _)) = world_vertices_for_node(puppet, node.uuid, false) {
			if pts.is_empty() {
				continue;
			}
			let mut mn = vec2(f32::INFINITY, f32::INFINITY);
			let mut mx = vec2(f32::NEG_INFINITY, f32::NEG_INFINITY);
			for p in &pts {
				mn = mn.min(*p);
				mx = mx.max(*p);
			}
			let zsort = puppet.node_zsort(node.uuid).unwrap_or(node.zsort);
			let lower_name = node.name.to_ascii_lowercase();
			let opacity = puppet.node_opacity(node.uuid).unwrap_or(1.0);
			let is_mask_helper = opacity <= 1e-4
				|| lower_name.contains("mask")
				|| lower_name.contains("stencil")
				|| lower_name.contains("clip");
			drawable_z.push(DrawableZInfo {
				uuid: node.uuid,
				name: node.name.trim_end_matches('\0').to_owned(),
				zsort,
				min: mn,
				max: mx,
				is_mask_helper,
			});
		}
	}

	for i in 0..drawable_z.len() {
		for j in (i + 1)..drawable_z.len() {
			let a = &drawable_z[i];
			let b = &drawable_z[j];
			if a.is_mask_helper || b.is_mask_helper {
				continue;
			}
			if (a.zsort - b.zsort).abs() <= 1e-4 {
				let ox = (a.max.x.min(b.max.x) - a.min.x.max(b.min.x)).max(0.0);
				let oy = (a.max.y.min(b.max.y) - a.min.y.max(b.min.y)).max(0.0);
				let overlap_area = ox * oy;
				if overlap_area > 16.0 {
					draw_order_conflicts.push(json::object! {
						node_a: a.name.clone(),
						node_b: b.name.clone(),
						zsort: a.zsort,
						overlap_area: overlap_area,
						issue: "overlapping_identical_zsort"
					});
				}
			}
		}
	}

	if let Some(fid) = face_id {
		let face_z = puppet.node_zsort(fid).unwrap_or(0.0);
		for d in &drawable_z {
			if d.uuid == fid || d.is_mask_helper {
				continue;
			}
			let lower = d.name.to_ascii_lowercase();
			let is_posterior = lower.contains("hair_back")
				|| lower.contains("back_hair")
				|| lower.contains("backhair")
				|| lower.contains("rear_hair")
				|| lower.contains("rearhair");
			let is_anterior_face_feature = !lower.contains("back")
				&& !lower.contains("rear")
				&& (lower.contains("eye")
					|| lower.contains("iris")
					|| lower.contains("pupil")
					|| lower.contains("lash")
					|| lower.contains("brow")
					|| lower.contains("mouth")
					|| lower.contains("lip")
					|| lower.contains("nose")
					|| lower.contains("blush")
					|| lower.contains("bang")
					|| lower.contains("hair_front")
					|| lower.contains("front_hair")
					|| lower.contains("fronthair"));
			if is_anterior_face_feature && d.zsort > face_z + 1e-3 {
				draw_order_conflicts.push(json::object! {
					node: d.name.trim_end_matches('\0').to_owned(),
					node_uuid: d.uuid.0,
					node_zsort: d.zsort,
					face_zsort: face_z,
					issue: "occluded_behind_face"
				});
			} else if is_posterior && d.zsort < face_z - 1e-3 {
				draw_order_conflicts.push(json::object! {
					node: d.name.trim_end_matches('\0').to_owned(),
					node_uuid: d.uuid.0,
					node_zsort: d.zsort,
					face_zsort: face_z,
					issue: "back_hair_in_front_of_face"
				});
			}
		}
	}

	// Check 10: Mask dependency cycles & invalid mask sources
	let mut mask_cycles: Vec<JsonValue> = Vec::new();
	let mut mask_adj: HashMap<InoxNodeUuid, Vec<InoxNodeUuid>> = HashMap::new();
	let mut mask_nodes: Vec<InoxNodeUuid> = Vec::new();
	for node in puppet.nodes.pre_order_iter() {
		if let Some(masks) = puppet.node_comps.get::<Masks>(node.uuid) {
			mask_nodes.push(node.uuid);
			let entry = mask_adj.entry(node.uuid).or_default();
			for m in &masks.masks {
				if m.source == node.uuid {
					mask_cycles.push(json::object! {
						node: node.name.trim_end_matches('\0').to_owned(),
						node_uuid: node.uuid.0,
						source_uuid: m.source.0,
						issue: "self_mask"
					});
				} else if puppet.nodes.get_node(m.source).is_none() {
					mask_cycles.push(json::object! {
						node: node.name.trim_end_matches('\0').to_owned(),
						node_uuid: node.uuid.0,
						source_uuid: m.source.0,
						issue: "dangling_mask_source"
					});
				} else if puppet.node_comps.get::<Drawable>(m.source).is_none() {
					mask_cycles.push(json::object! {
						node: node.name.trim_end_matches('\0').to_owned(),
						node_uuid: node.uuid.0,
						source_uuid: m.source.0,
						issue: "non_drawable_mask_source"
					});
				} else if !entry.contains(&m.source) {
					entry.push(m.source);
				}
			}
		}
	}

	let mut dfs_state: HashMap<InoxNodeUuid, u8> = HashMap::new();
	let mut dfs_stack: Vec<InoxNodeUuid> = Vec::new();
	let mut seen_cycles: HashSet<Vec<u32>> = HashSet::new();
	for start in mask_nodes {
		if dfs_state.get(&start).copied().unwrap_or(0) == 0 {
			detect_mask_cycles_dfs(
				start,
				puppet,
				&mask_adj,
				&mut dfs_state,
				&mut dfs_stack,
				&mut seen_cycles,
				&mut mask_cycles,
			);
		}
	}

	let mut score: i32 = 100;
	score -= (containment_leaks.len() as i32) * 8;
	let distinct_foldover_nodes: std::collections::HashSet<&str> = mesh_foldovers
		.iter()
		.filter_map(|e| e["node"].as_str())
		.collect();
	score -= (distinct_foldover_nodes.len() as i32) * 15;
	score -= (mesh_stretch.len() as i32) * 5;
	score -= (blink_issues.len() as i32) * 10;
	score -= (symmetry_defects.len() as i32) * 8;
	score -= (unmasked_parts.len() as i32) * 5;
	score -= (deformer_issues.len() as i32) * 8;
	score -= (binding_coverage.len() as i32) * 5;
	score -= (draw_order_conflicts.len() as i32) * 6;
	score -= (mask_cycles.len() as i32) * 15;
	let score = score.clamp(0, 100) as u32;

	let passed = containment_leaks.is_empty()
		&& mesh_foldovers.is_empty()
		&& blink_issues.is_empty()
		&& deformer_issues.is_empty()
		&& mask_cycles.is_empty()
		&& score >= 80;

	if !containment_leaks.is_empty() || !mesh_foldovers.is_empty() || !unmasked_parts.is_empty() {
		recommendations.push("Run `rig.auto_fix` to clamp facial feature containment, relax mesh fold-overs, and attach missing masks.".to_owned());
	}
	if !blink_issues.is_empty() {
		recommendations.push("Re-run `rig.auto_blink` or refine eyelid with `deform.brush` at (x=0.0).".to_owned());
	}
	if !mesh_stretch.is_empty() {
		recommendations.push("Increase mesh subdivision via `mesh.auto_contour` or `mesh.auto_grid` on stretched parts.".to_owned());
	}
	if !deformer_issues.is_empty() {
		recommendations.push("Repair invalid GridDeformer/PathDeformer/MeshGroup hierarchies or remove empty deformers.".to_owned());
	}
	if !binding_coverage.is_empty() {
		recommendations.push("Fix dangling or mismatched Deform bindings and bind orphan deformers/physics targets.".to_owned());
	}
	if !draw_order_conflicts.is_empty() {
		recommendations.push("Separate overlapping zsort values and ensure anterior facial parts have lower zsort than Face.".to_owned());
	}
	if !mask_cycles.is_empty() {
		recommendations.push("Break cyclic mask dependencies and replace non-drawable or dangling mask sources.".to_owned());
	}
	if passed && recommendations.is_empty() {
		recommendations.push("Rig passed all 10 geometric, deformer, binding, draw-order & mask checks. Ready for `render.grid` / `puppet.save`.".to_owned());
	}

	// Restore rest state
	eval_puppet_at(puppet, &HashMap::new());

	json::object! {
		score: score,
		passed: passed,
		summary: {
			containment_leaks: containment_leaks.len(),
			mesh_foldovers: mesh_foldovers.len(),
			mesh_stretch: mesh_stretch.len(),
			blink_issues: blink_issues.len(),
			symmetry_defects: symmetry_defects.len(),
			unmasked_parts: unmasked_parts.len(),
			deformer_issues: deformer_issues.len(),
			binding_coverage: binding_coverage.len(),
			draw_order_conflicts: draw_order_conflicts.len(),
			mask_cycles: mask_cycles.len()
		},
		details: {
			containment_leaks: containment_leaks,
			mesh_foldovers: mesh_foldovers,
			mesh_stretch: mesh_stretch,
			blink_issues: blink_issues,
			symmetry_defects: symmetry_defects,
			unmasked_parts: unmasked_parts,
			deformer_issues: deformer_issues,
			binding_coverage: binding_coverage,
			draw_order_conflicts: draw_order_conflicts,
			mask_cycles: mask_cycles
		},
		recommendations: recommendations
	}
}

fn has_deformable_descendant(puppet: &Puppet, root: InoxNodeUuid) -> bool {
	puppet.nodes.pre_order_iter().any(|n| {
		n.uuid != root
			&& n.enabled
			&& puppet.nodes.is_descendant(n.uuid, root)
			&& (puppet.node_comps.get::<Mesh>(n.uuid).is_some()
				|| puppet.node_comps.get::<Drawable>(n.uuid).is_some()
				|| puppet.node_comps.get::<Composite>(n.uuid).is_some())
	})
}

fn detect_mask_cycles_dfs(
	node: InoxNodeUuid,
	puppet: &Puppet,
	adj: &HashMap<InoxNodeUuid, Vec<InoxNodeUuid>>,
	state: &mut HashMap<InoxNodeUuid, u8>,
	stack: &mut Vec<InoxNodeUuid>,
	seen_cycles: &mut HashSet<Vec<u32>>,
	out: &mut Vec<JsonValue>,
) {
	state.insert(node, 1);
	stack.push(node);

	if let Some(neighbors) = adj.get(&node) {
		for &next in neighbors {
			match state.get(&next).copied().unwrap_or(0) {
				0 => detect_mask_cycles_dfs(next, puppet, adj, state, stack, seen_cycles, out),
				1 => {
					if let Some(pos) = stack.iter().position(|&u| u == next) {
						let raw_cycle = &stack[pos..];
						if raw_cycle.len() >= 2 {
							let min_idx = raw_cycle
								.iter()
								.enumerate()
								.min_by_key(|&(_, u)| u.0)
								.map(|(i, _)| i)
								.unwrap_or(0);
							let mut canonical = Vec::with_capacity(raw_cycle.len());
							for k in 0..raw_cycle.len() {
								canonical.push(raw_cycle[(min_idx + k) % raw_cycle.len()].0);
							}
							if seen_cycles.insert(canonical.clone()) {
								let names: Vec<String> = canonical
									.iter()
									.map(|&uid| {
										puppet
											.nodes
											.get_node(InoxNodeUuid(uid))
											.map(|n| n.name.trim_end_matches('\0').to_owned())
											.unwrap_or_else(|| format!("Node({uid})"))
									})
									.collect();
								out.push(json::object! {
									issue: "mask_cycle",
									cycle_uuids: canonical,
									cycle_names: names
								});
							}
						}
					}
				}
				_ => {}
			}
		}
	}

	stack.pop();
	state.insert(node, 2);
}

/// Deterministically fix containment leaks, mesh fold-overs, and missing masks.
pub fn auto_fix_rig(model: &mut Model) -> JsonValue {
	auto_fix_puppet(&mut model.puppet)
}

/// Deterministically fix containment leaks, mesh fold-overs, and missing masks on a `Puppet`.
pub fn auto_fix_puppet(puppet: &mut Puppet) -> JsonValue {
	eval_puppet_at(puppet, &HashMap::new());

	let mut clamped_vertices = 0usize;
	let mut relaxed_cells = 0usize;
	let mut attached_masks = 0usize;

	// Fix 1: Containment clamping for Head:: Yaw-Pitch facial feature bindings
	if let (Some(face_id), Some(head_param)) = (find_face_node(puppet), puppet.params.get("Head:: Yaw-Pitch").cloned()) {
		if let Some((face_rest, _, face_abs)) = world_vertices_for_node(puppet, face_id, false) {
			let face_deform_binding = head_param.bindings.iter().find(|b| {
				b.node == face_id && matches!(b.values, BindingValues::Deform(_))
			});

			let x_len = head_param.axis_points.x.len();
			let y_len = head_param.axis_points.y.len();

			// Precompute deformed Face safe ellipse per (ix, iy)
			let mut face_ellipses = vec![vec![(Vec2::ZERO, vec2(50.0, 50.0)); y_len]; x_len];
			for ix in 0..x_len {
				for iy in 0..y_len {
					let pts: Vec<Vec2> = if let Some(fb) = face_deform_binding {
						if let BindingValues::Deform(ref mat) = fb.values {
							face_rest
								.iter()
								.enumerate()
								.map(|(vi, &w)| {
									let d = mat[(ix, iy)].get(vi).copied().unwrap_or(Vec2::ZERO);
									w + face_abs.transform_vector3(d.extend(0.0)).truncate()
								})
								.collect()
						} else {
							face_rest.clone()
						}
					} else {
						face_rest.clone()
					};
					let (mn, mx) = bbox_of(&pts);
					let fc = (mn + mx) * 0.5;
					let fh = ((mx - mn) * 0.5 * vec2(0.88, 0.90)).max(vec2(8.0, 8.0));
					face_ellipses[ix][iy] = (fc, fh);
				}
			}

			// Collect world transforms & rest positions for each Part
			let mut part_info: HashMap<InoxNodeUuid, (Vec<Vec2>, Mat4)> = HashMap::new();
			for node in puppet.nodes.iter() {
				if FeatureRole::from_name(&node.name).is_facial_feature() {
					if let Some((wverts, _, abs)) = world_vertices_for_node(puppet, node.uuid, false) {
						part_info.insert(node.uuid, (wverts, abs));
					}
				}
			}

			if let Some(hp_mut) = puppet.params.get_mut("Head:: Yaw-Pitch") {
				for binding in &mut hp_mut.bindings {
					let Some((wverts, abs)) = part_info.get(&binding.node) else {
						continue;
					};
					let abs_inv = abs.inverse();
					if let BindingValues::Deform(ref mut mat) = binding.values {
						for ix in 0..x_len {
							for iy in 0..y_len {
								let (fc, fh) = face_ellipses[ix][iy];
								if let Some(cell) = mat.get_mut(ix, iy) {
									for (vi, d) in cell.iter_mut().enumerate() {
										let Some(&rest_w) = wverts.get(vi) else { continue };
										let dw = abs.transform_vector3(d.extend(0.0)).truncate();
										let pw = rest_w + dw;
										let rel = pw - fc;
										let norm = (rel.x / fh.x).powi(2) + (rel.y / fh.y).powi(2);
										if norm > 1.0 {
											let clamped_w = fc + rel / norm.sqrt();
											let new_dw = clamped_w - rest_w;
											*d = abs_inv.transform_vector3(new_dw.extend(0.0)).truncate();
											clamped_vertices += 1;
										}
									}
								}
							}
						}
					}
				}
			}
		}
	}

	// Fix 2: Laplacian relaxation for any flipped triangles across all Deform bindings
	let mut meshes_adj: HashMap<InoxNodeUuid, (Mesh, Vec<Vec<usize>>, Vec<f32>)> = HashMap::new();
	for node in puppet.nodes.iter() {
		if let Some(mesh) = puppet.node_comps.get::<Mesh>(node.uuid) {
			let mut adj: Vec<HashSet<usize>> = vec![HashSet::new(); mesh.vertices.len()];
			let mut rest_areas = Vec::with_capacity(mesh.indices.len() / 3);
			for tri in mesh.indices.chunks_exact(3) {
				let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
				if i0 < mesh.vertices.len() && i1 < mesh.vertices.len() && i2 < mesh.vertices.len() {
					adj[i0].extend([i1, i2]);
					adj[i1].extend([i0, i2]);
					adj[i2].extend([i0, i1]);
					rest_areas.push(tri_signed_area(mesh.vertices[i0], mesh.vertices[i1], mesh.vertices[i2]));
				} else {
					rest_areas.push(0.0);
				}
			}
			let adj_vec = adj.into_iter().map(|s| s.into_iter().collect()).collect();
			meshes_adj.insert(node.uuid, (mesh.clone(), adj_vec, rest_areas));
		}
	}

	for param in puppet.params.values_mut() {
		let is_blink_param = param.name.to_lowercase().contains("open");
		for binding in &mut param.bindings {
			let Some((mesh, adj, rest_areas)) = meshes_adj.get(&binding.node) else {
				continue;
			};
			let BindingValues::Deform(ref mut mat) = binding.values else {
				continue;
			};
			let w = param.axis_points.x.len();
			let h = param.axis_points.y.len();
			for ix in 0..w {
				for iy in 0..h {
					let Some(cell) = mat.get_mut(ix, iy) else { continue };
					if cell.len() != mesh.vertices.len() {
						continue;
					}
					let mut fixed_this_cell = false;
					for _iter in 0..24 {
						let mut bad_verts = HashSet::new();
						for (ti, tri) in mesh.indices.chunks_exact(3).enumerate() {
							let ra = rest_areas[ti];
							if ra.abs() < 1e-4 {
								continue;
							}
							let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
							let p0 = mesh.vertices[i0] + cell[i0];
							let p1 = mesh.vertices[i1] + cell[i1];
							let p2 = mesh.vertices[i2] + cell[i2];
							let da = tri_signed_area(p0, p1, p2);
							let crushed = !is_blink_param && da.abs() < ra.abs() * 0.05;
							if da * ra <= 0.0 || crushed {
								bad_verts.extend([i0, i1, i2]);
							}
						}
						if bad_verts.is_empty() {
							break;
						}
						fixed_this_cell = true;
						let prev = cell.clone();
						for vi in bad_verts {
							if adj[vi].is_empty() {
								continue;
							}
							let sum: Vec2 = adj[vi].iter().map(|&n| prev[n]).sum();
							let avg = sum / adj[vi].len() as f32;
							cell[vi] = prev[vi] * 0.35 + avg * 0.55;
						}
					}
					if fixed_this_cell {
						relaxed_cells += 1;
					}
				}
			}
		}
	}

	// Fix 3: Auto-attach clipping masks for unmasked iris/pupil/tongue parts
	let mut eye_l_id = None;
	let mut eye_r_id = None;
	let mut mouth_id = None;
	let mut candidates = Vec::new();
	for node in puppet.nodes.iter() {
		match FeatureRole::from_name(&node.name) {
			FeatureRole::EyeLeft => eye_l_id = Some(node.uuid),
			FeatureRole::EyeRight => eye_r_id = Some(node.uuid),
			FeatureRole::Mouth => mouth_id = Some(node.uuid),
			FeatureRole::PupilOrIris | FeatureRole::Tongue => {
				candidates.push((node.uuid, node.name.trim_end_matches('\0').to_owned()))
			}
			_ => {}
		}
	}
	for (cid, cname) in candidates {
		let role = FeatureRole::from_name(&cname);
		let target_mask = if role == FeatureRole::Tongue {
			mouth_id
		} else if cname.to_lowercase().contains("_r") || cname.contains("右") {
			eye_r_id.or(eye_l_id)
		} else {
			eye_l_id.or(eye_r_id)
		};
		if let (Some(src), Some(drawable)) = (target_mask, puppet.node_comps.get_mut::<Drawable>(cid)) {
			if drawable.masks.as_ref().map_or(true, |m| m.masks.is_empty()) {
				drawable.masks = Some(Masks {
					threshold: 0.1,
					masks: vec![Mask {
						source: src,
						mode: MaskMode::Mask,
					}],
				});
				attached_masks += 1;
			}
		}
	}

	puppet.reset_contexts();
	json::object! {
		clamped_vertices: clamped_vertices,
		relaxed_cells: relaxed_cells,
		attached_masks: attached_masks
	}
}

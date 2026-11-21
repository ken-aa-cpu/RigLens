use std::collections::HashSet;

use glam::{vec2, vec3, Vec2};
use image::{Rgba, RgbaImage};
use riglens_core::math::curve::CurveType;
use riglens_core::math::transform::TransformOffset;
use riglens_core::node::components::{
	BlendMode, Drawable, GridDeformer, GridFormation, Mask, MaskMode, Masks, Mesh, MeshGroup, PathDeformer,
	TexturedMesh,
};
use riglens_core::node::{InoxNode, InoxNodeUuid};
use riglens_core::texture::decode_model_textures;
use json::JsonValue;

use crate::rig::{
	add_grid_part, generate_contour_mesh, generate_delaunay_mesh_with_options, generate_grid_mesh,
	retriangulate_points_preserving_deforms, synthesize_expression_overlay, DelaunayMeshOptions, RefinementZone,
};

use super::Session;

impl Session {
	pub(super) fn exec_node_mesh_cmd(&mut self, cmd: &str, params: &JsonValue) -> Result<JsonValue, String> {
		match cmd {
			"node.add_part" => {
				let name = params["name"]
					.as_str()
					.ok_or_else(|| "Missing 'name'".to_owned())?;
				let parent = if !params["parent_uuid"].is_null() || !params["parent_name"].is_null() {
					self.resolve_node(params, "parent_uuid", "parent_name")?
				} else {
					self.model.puppet.nodes.root_node_id
				};
				let zsort = params["zsort"].as_f32().unwrap_or(0.0);
				let x = params["x"].as_f32().unwrap_or(0.0);
				let y = params["y"].as_f32().unwrap_or(0.0);
				let cols = params["cols"].as_usize().unwrap_or(4);
				let rows = params["rows"].as_usize().unwrap_or(4);

				let img = if let Some(img_path) = params["image_path"].as_str() {
					image::open(img_path)
						.map_err(|e| format!("Failed to open image '{img_path}': {e}"))?
						.to_rgba8()
				} else {
					let w = params["width"].as_f32().unwrap_or(120.0).max(8.0) as u32;
					let h = params["height"].as_f32().unwrap_or(120.0).max(8.0) as u32;
					let rgba = if let JsonValue::Array(arr) = &params["color"] {
						[
							arr.first().and_then(JsonValue::as_u8).unwrap_or(230),
							arr.get(1).and_then(JsonValue::as_u8).unwrap_or(180),
							arr.get(2).and_then(JsonValue::as_u8).unwrap_or(190),
							arr.get(3).and_then(JsonValue::as_u8).unwrap_or(255),
						]
					} else {
						[230, 180, 190, 255]
					};
					RgbaImage::from_pixel(w, h, Rgba(rgba))
				};
				let width = params["width"].as_f32().unwrap_or(img.width() as f32);
				let height = params["height"].as_f32().unwrap_or(img.height() as f32);

				let uuid = add_grid_part(
					&mut self.model,
					parent,
					name,
					zsort,
					vec2(x, y),
					width,
					height,
					cols,
					rows,
					img,
				);
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					uuid: uuid.0,
					name: name,
					parent_uuid: parent.0
				})
			}
			"mesh.auto_grid" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let cols = params["cols"].as_usize().unwrap_or(6).max(1);
				let rows = params["rows"].as_usize().unwrap_or(6).max(1);

				let existing = self
					.model
					.puppet
					.node_comps
					.get::<Mesh>(id)
					.ok_or_else(|| format!("Node {} has no Mesh", id.0))?
					.clone();

				let (min_v, max_v) = existing.vertices.iter().fold(
					(vec2(f32::INFINITY, f32::INFINITY), vec2(f32::NEG_INFINITY, f32::NEG_INFINITY)),
					|(mn, mx), &v| (mn.min(v), mx.max(v)),
				);
				let cur_size = (max_v - min_v).max(vec2(16.0, 16.0));
				let width = params["width"].as_f32().unwrap_or(cur_size.x);
				let height = params["height"].as_f32().unwrap_or(cur_size.y);

				let new_mesh = generate_grid_mesh(width, height, cols, rows, existing.origin);
				let vcount = new_mesh.vertices.len();
				let tcount = new_mesh.indices.len() / 3;
				self.model.puppet.node_comps.set(id, new_mesh);
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					vertices: vcount,
					triangles: tcount
				})
			}
			"mesh.auto_contour" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let cols = params["cols"].as_usize().unwrap_or(8).max(2);
				let rows = params["rows"].as_usize().unwrap_or(8).max(2);
				let alpha_threshold = params["alpha_threshold"].as_u8().unwrap_or(4);

				let existing = self
					.model
					.puppet
					.node_comps
					.get::<Mesh>(id)
					.ok_or_else(|| format!("Node {} has no Mesh", id.0))?
					.clone();
				let tm = self
					.model
					.puppet
					.node_comps
					.get::<TexturedMesh>(id)
					.ok_or_else(|| format!("Node {} has no TexturedMesh", id.0))?
					.clone();
				let decoded = decode_model_textures(self.model.textures.iter());
				let tex = decoded
					.get(tm.tex_albedo.0)
					.ok_or_else(|| format!("Texture {} missing", tm.tex_albedo.0))?;
				let img = RgbaImage::from_raw(tex.width(), tex.height(), tex.pixels().to_vec())
					.ok_or_else(|| "Failed to construct RgbaImage from texture".to_owned())?;

				let (min_v, max_v) = existing.vertices.iter().fold(
					(vec2(f32::INFINITY, f32::INFINITY), vec2(f32::NEG_INFINITY, f32::NEG_INFINITY)),
					|(mn, mx), &v| (mn.min(v), mx.max(v)),
				);
				let cur_size = (max_v - min_v).max(vec2(16.0, 16.0));
				let width = params["width"].as_f32().unwrap_or(cur_size.x);
				let height = params["height"].as_f32().unwrap_or(cur_size.y);

				let new_mesh = generate_contour_mesh(&img, width, height, cols, rows, alpha_threshold);
				let vcount = new_mesh.vertices.len();
				let tcount = new_mesh.indices.len() / 3;
				self.model.puppet.node_comps.set(id, new_mesh);
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					vertices: vcount,
					triangles: tcount
				})
			}
			"mesh.auto_delaunay" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let step_px = params["step_px"].as_f32().unwrap_or(22.0);
				let alpha_threshold = params["alpha_threshold"].as_u8().unwrap_or(4);
				let boundary_offset_px = params["boundary_offset_px"].as_f32().unwrap_or(2.0);
				let constrained = params["constrained"].as_bool().unwrap_or(true);
				let refine_near_corners = params["refine_near_corners"].as_bool().unwrap_or(true);

				let mut refinement_zones = Vec::new();
				for z in params["refinement_zones"].members() {
					let cx = z["center"][0]
						.as_f32()
						.or_else(|| z["x"].as_f32())
						.unwrap_or(0.0);
					let cy = z["center"][1]
						.as_f32()
						.or_else(|| z["y"].as_f32())
						.unwrap_or(0.0);
					let radius = z["radius"].as_f32().unwrap_or(24.0);
					let density_scale = z["density_scale"].as_f32().unwrap_or(0.35);
					refinement_zones.push(RefinementZone {
						center: vec2(cx, cy),
						radius,
						density_scale,
					});
				}

				let existing = self
					.model
					.puppet
					.node_comps
					.get::<Mesh>(id)
					.ok_or_else(|| format!("Node {} has no Mesh", id.0))?
					.clone();
				let tm = self
					.model
					.puppet
					.node_comps
					.get::<TexturedMesh>(id)
					.ok_or_else(|| format!("Node {} has no TexturedMesh", id.0))?
					.clone();
				let decoded = decode_model_textures(self.model.textures.iter());
				let tex = decoded
					.get(tm.tex_albedo.0)
					.ok_or_else(|| format!("Texture {} missing", tm.tex_albedo.0))?;
				let img = RgbaImage::from_raw(tex.width(), tex.height(), tex.pixels().to_vec())
					.ok_or_else(|| "Failed to construct RgbaImage from texture".to_owned())?;

				let (min_v, max_v) = existing.vertices.iter().fold(
					(vec2(f32::INFINITY, f32::INFINITY), vec2(f32::NEG_INFINITY, f32::NEG_INFINITY)),
					|(mn, mx), &v| (mn.min(v), mx.max(v)),
				);
				let cur_size = (max_v - min_v).max(vec2(16.0, 16.0));
				let width = params["width"].as_f32().unwrap_or(cur_size.x);
				let height = params["height"].as_f32().unwrap_or(cur_size.y);

				let opts = DelaunayMeshOptions {
					step_px,
					alpha_threshold,
					boundary_offset_px,
					constrained,
					refine_near_corners,
					refinement_zones,
				};
				let new_mesh = generate_delaunay_mesh_with_options(&img, width, height, &opts);
				let vcount = new_mesh.vertices.len();
				let tcount = new_mesh.indices.len() / 3;
				self.model.puppet.node_comps.set(id, new_mesh);
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					triangulation: "bowyer_watson_delaunay",
					constrained: opts.constrained,
					vertices: vcount,
					triangles: tcount
				})
			}
			"mesh.edit_topology" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let action = params["action"]
					.as_str()
					.ok_or_else(|| "Missing 'action' (move|insert|remove)".to_owned())?;
				let mut verts = self
					.model
					.puppet
					.node_comps
					.get::<Mesh>(id)
					.ok_or_else(|| format!("Node {} has no Mesh", id.0))?
					.vertices
					.clone();

				let mut inserted_index: Option<usize> = None;
				match action {
					"move" => {
						let dx = params["dx"].as_f32();
						let dy = params["dy"].as_f32();
						let abs_x = params["x"].as_f32();
						let abs_y = params["y"].as_f32();
						let mut idx_list = Vec::new();
						if let Some(single) = params["vertex_index"].as_usize() {
							idx_list.push(single);
						}
						if let JsonValue::Array(arr) = &params["indices"] {
							for item in arr {
								if let Some(vi) = item.as_usize() {
									idx_list.push(vi);
								}
							}
						}
						if idx_list.is_empty() {
							return Err("Action 'move' requires 'vertex_index' or 'indices'".to_owned());
						}
						for vi in idx_list {
							let Some(v) = verts.get_mut(vi) else {
								return Err(format!("Vertex index {vi} out of bounds ({})", verts.len()));
							};
							if let Some(x) = abs_x {
								v.x = x;
							} else if let Some(d) = dx {
								v.x += d;
							}
							if let Some(y) = abs_y {
								v.y = y;
							} else if let Some(d) = dy {
								v.y += d;
							}
						}
					}
					"insert" => {
						let x = params["x"].as_f32().ok_or_else(|| "Action 'insert' requires 'x'".to_owned())?;
						let y = params["y"].as_f32().ok_or_else(|| "Action 'insert' requires 'y'".to_owned())?;
						let pt = vec2(x, y);
						if verts.iter().any(|&v| (v - pt).length_squared() < 1.0) {
							return Err("A vertex already exists within 1px of that coordinate".to_owned());
						}
						verts.push(pt);
						inserted_index = Some(verts.len() - 1);
					}
					"remove" => {
						let mut del = HashSet::new();
						if let Some(single) = params["vertex_index"].as_usize() {
							del.insert(single);
						}
						if let JsonValue::Array(arr) = &params["indices"] {
							for item in arr {
								if let Some(vi) = item.as_usize() {
									del.insert(vi);
								}
							}
						}
						let kept: Vec<Vec2> = verts
							.into_iter()
							.enumerate()
							.filter(|(i, _)| !del.contains(i))
							.map(|(_, v)| v)
							.collect();
						if kept.len() < 3 {
							return Err("Cannot delete vertices: mesh must retain at least 3 vertices".to_owned());
						}
						verts = kept;
					}
					other => return Err(format!("Unknown topology action '{other}'")),
				}

				let (vc, tc) = retriangulate_points_preserving_deforms(&mut self.model.puppet, id, verts)?;
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					action: action,
					inserted_index: inserted_index.map(|i| JsonValue::from(i as u64)).unwrap_or(JsonValue::Null),
					vertices: vc,
					triangles: tc
				})
			}
			"node.configure" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				if !params["parent_uuid"].is_null() || !params["parent_name"].is_null() {
					let new_parent = self.resolve_node(params, "parent_uuid", "parent_name")?;
					self.model.puppet.nodes.reparent(id, new_parent)?;
				}
				if let Some(node) = self.model.puppet.nodes.get_node_mut(id) {
					if let Some(new_name) = params["name"].as_str() {
						node.name = new_name.to_owned();
					}
					if let Some(z) = params["zsort"].as_f32() {
						node.zsort = z;
					}
					if let Some(en) = params["enabled"].as_bool() {
						node.enabled = en;
					}
					if let Some(ltr) = params["lock_to_root"].as_bool() {
						node.lock_to_root = ltr;
					}
					if let Some(x) = params["x"].as_f32() {
						node.trans_offset.translation.x = x;
					}
					if let Some(y) = params["y"].as_f32() {
						node.trans_offset.translation.y = y;
					}
					if let Some(deg) = params["rot_z_deg"].as_f32() {
						node.trans_offset.rotation.z = deg.to_radians();
					}
					if let Some(sx) = params["scale_x"].as_f32() {
						node.trans_offset.scale.x = sx;
					}
					if let Some(sy) = params["scale_y"].as_f32() {
						node.trans_offset.scale.y = sy;
					}
				}
				if let Some(drawable) = self.model.puppet.node_comps.get_mut::<Drawable>(id) {
					if let Some(op) = params["opacity"].as_f32() {
						drawable.blending.opacity = op.clamp(0.0, 1.0);
					}
					if let Some(bm) = params["blend_mode"].as_str() {
						if let Some(parsed) = BlendMode::from_name(bm) {
							drawable.blending.mode = parsed;
						} else {
							return Err(format!("Unknown blend_mode '{bm}'"));
						}
					}
					if let JsonValue::Array(arr) = &params["tint"] {
						if arr.len() >= 3 {
							drawable.blending.tint = vec3(
								arr[0].as_f32().unwrap_or(1.0),
								arr[1].as_f32().unwrap_or(1.0),
								arr[2].as_f32().unwrap_or(1.0),
							);
						}
					}
				}
				self.model.puppet.reset_contexts();
				self.revision += 1;
				let name = self.model.puppet.nodes.get_node(id).map(|n| n.name.clone()).unwrap_or_default();
				Ok(json::object! {
					uuid: id.0,
					name: name,
					updated: true
				})
			}
			"node.set_mask" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let threshold = params["threshold"].as_f32().unwrap_or(0.1);
				let mut masks_vec = Vec::new();
				if let JsonValue::Array(arr) = &params["sources"] {
					for item in arr {
						let src_id = if item.is_object() {
							self.resolve_node(item, "uuid", "name")?
						} else {
							self.resolve_node_by_ref(item)?
						};
						let mode = match item["mode"].as_str().unwrap_or("Mask") {
							"Dodge" | "DodgeMask" => MaskMode::Dodge,
							_ => MaskMode::Mask,
						};
						masks_vec.push(Mask { source: src_id, mode });
					}
				}
				let drawable = self
					.model
					.puppet
					.node_comps
					.get_mut::<Drawable>(id)
					.ok_or_else(|| format!("Node {} does not have a Drawable component", id.0))?;
				let mask_count = masks_vec.len();
				drawable.masks = if masks_vec.is_empty() {
					None
				} else {
					Some(Masks {
						threshold,
						masks: masks_vec,
					})
				};
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					mask_count: mask_count
				})
			}
			"node.add_meshgroup" => {
				let name = params["name"].as_str().unwrap_or("MeshGroup");
				let parent = self.resolve_optional_parent(params)?;
				let cols = params["cols"].as_usize().unwrap_or(4).max(1);
				let rows = params["rows"].as_usize().unwrap_or(4).max(1);
				let padding = params["padding"].as_f32().unwrap_or(16.0);
				let child_ids = self.resolve_child_list(params)?;
				let (center, w, h) = self.compute_children_bounds_in_parent(parent, &child_ids, padding);

				let mg_id = self.model.puppet.next_node_uuid();
				self.model.puppet.add_node(
					parent,
					InoxNode {
						uuid: mg_id,
						name: name.to_owned(),
						enabled: true,
						zsort: 0.0,
						trans_offset: TransformOffset {
							translation: vec3(center.x, center.y, 0.0),
							..TransformOffset::default()
						},
						lock_to_root: false,
					},
				);
				self.model.puppet.node_comps.add(
					mg_id,
					MeshGroup {
						dynamic: true,
						translate_children: true,
					},
				);
				self.model
					.puppet
					.node_comps
					.add(mg_id, generate_grid_mesh(w, h, cols, rows, Vec2::ZERO));

				self.reparent_into_deformer(mg_id, parent, &child_ids, center)?;
				Ok(json::object! {
					uuid: mg_id.0,
					name: name,
					reparented_children: child_ids.len(),
					size: [w, h]
				})
			}
			"node.add_grid_deformer" => {
				let name = params["name"].as_str().unwrap_or("GridDeformer");
				let parent = self.resolve_optional_parent(params)?;
				let cols = params["cols"].as_usize().unwrap_or(3).clamp(1, 32);
				let rows = params["rows"].as_usize().unwrap_or(3).clamp(1, 32);
				let padding = params["padding"].as_f32().unwrap_or(16.0);
				let child_ids = self.resolve_child_list(params)?;
				let (center, w, h) = self.compute_children_bounds_in_parent(parent, &child_ids, padding);

				let axis_x: Vec<f32> = (0..=cols)
					.map(|i| ((i as f32 / cols as f32) - 0.5) * w)
					.collect();
				let axis_y: Vec<f32> = (0..=rows)
					.map(|i| ((i as f32 / rows as f32) - 0.5) * h)
					.collect();

				let gd_id = self.model.puppet.next_node_uuid();
				self.model.puppet.add_node(
					parent,
					InoxNode {
						uuid: gd_id,
						name: name.to_owned(),
						enabled: true,
						zsort: 0.0,
						trans_offset: TransformOffset {
							translation: vec3(center.x, center.y, 0.0),
							..TransformOffset::default()
						},
						lock_to_root: false,
					},
				);
				self.model.puppet.node_comps.add(
					gd_id,
					GridDeformer {
						axis_x,
						axis_y,
						formation: GridFormation::Bilinear,
						dynamic: true,
						translate_children: true,
					},
				);

				self.reparent_into_deformer(gd_id, parent, &child_ids, center)?;
				Ok(json::object! {
					uuid: gd_id.0,
					name: name,
					reparented_children: child_ids.len(),
					cols: cols,
					rows: rows,
					size: [w, h]
				})
			}
			"node.add_path_deformer" => {
				let name = params["name"].as_str().unwrap_or("PathDeformer");
				let parent = self.resolve_optional_parent(params)?;
				let points = params["points"].as_usize().unwrap_or(4).clamp(2, 16);
				let curve_type = match params["curve_type"].as_str().unwrap_or("Spline") {
					"Bezier" => CurveType::Bezier,
					_ => CurveType::Spline,
				};
				let horizontal = params["orientation"].as_str().unwrap_or("vertical").eq_ignore_ascii_case("horizontal");
				let padding = params["padding"].as_f32().unwrap_or(16.0);
				let child_ids = self.resolve_child_list(params)?;
				let (center, w, h) = self.compute_children_bounds_in_parent(parent, &child_ids, padding);

				let vertices: Vec<Vec2> = (0..points)
					.map(|i| {
						let t = (i as f32 / (points - 1) as f32) - 0.5;
						if horizontal {
							vec2(t * w, 0.0)
						} else {
							vec2(0.0, t * h)
						}
					})
					.collect();

				let pd_id = self.model.puppet.next_node_uuid();
				self.model.puppet.add_node(
					parent,
					InoxNode {
						uuid: pd_id,
						name: name.to_owned(),
						enabled: true,
						zsort: 0.0,
						trans_offset: TransformOffset {
							translation: vec3(center.x, center.y, 0.0),
							..TransformOffset::default()
						},
						lock_to_root: false,
					},
				);
				self.model.puppet.node_comps.add(
					pd_id,
					PathDeformer {
						curve_type,
						vertices,
						dynamic: true,
						physics_only: false,
					},
				);

				self.reparent_into_deformer(pd_id, parent, &child_ids, center)?;
				Ok(json::object! {
					uuid: pd_id.0,
					name: name,
					curve_type: curve_type.as_str(),
					points: points,
					reparented_children: child_ids.len()
				})
			}
			"node.remove" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let removed = self.model.puppet.nodes.remove_subtree(id)?;
				let removed_set: HashSet<InoxNodeUuid> = removed.iter().copied().collect();
				for p in self.model.puppet.params.values_mut() {
					p.bindings.retain(|b| !removed_set.contains(&b.node));
				}
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					removed_count: removed.len(),
					removed_uuids: removed.iter().map(|u| u.0).collect::<Vec<_>>()
				})
			}
			"node.synthesize_overlay" => {
				let kind = params["kind"]
					.as_str()
					.ok_or_else(|| "Missing 'kind' (blush, gloom, tears, angry, shock)".to_owned())?;
				let parent = if !params["parent_uuid"].is_null() || !params["parent_name"].is_null() {
					Some(self.resolve_node(params, "parent_uuid", "parent_name")?)
				} else {
					None
				};
				let (uuid, param_name) = synthesize_expression_overlay(&mut self.model, kind, parent)?;
				self.revision += 1;
				Ok(json::object! {
					uuid: uuid.0,
					kind: kind,
					param: param_name
				})
			}
			_ => Err(format!("Unknown command: '{cmd}'")),
		}
	}

	fn resolve_optional_parent(&self, params: &JsonValue) -> Result<InoxNodeUuid, String> {
		if !params["parent_uuid"].is_null() || !params["parent_name"].is_null() {
			self.resolve_node(params, "parent_uuid", "parent_name")
		} else {
			Ok(self.model.puppet.nodes.root_node_id)
		}
	}

	fn resolve_child_list(&self, params: &JsonValue) -> Result<Vec<InoxNodeUuid>, String> {
		let mut child_ids = Vec::new();
		if let JsonValue::Array(arr) = &params["children"] {
			for item in arr {
				child_ids.push(self.resolve_node_by_ref(item)?);
			}
		}
		Ok(child_ids)
	}

	fn compute_children_bounds_in_parent(
		&mut self,
		parent: InoxNodeUuid,
		child_ids: &[InoxNodeUuid],
		padding: f32,
	) -> (Vec2, f32, f32) {
		self.model.puppet.init_all();
		self.model.puppet.begin_frame();
		self.model.puppet.end_frame(0.0);
		let parent_inv = self
			.model
			.puppet
			.node_transform(parent)
			.map(|t| t.absolute.inverse())
			.unwrap_or_default();
		let mut mn = vec2(f32::INFINITY, f32::INFINITY);
		let mut mx = vec2(f32::NEG_INFINITY, f32::NEG_INFINITY);
		for &cid in child_ids {
			if let (Some(cmesh), Some(cts)) = (
				self.model.puppet.node_comps.get::<Mesh>(cid),
				self.model.puppet.node_transform(cid),
			) {
				let to_parent = parent_inv * cts.absolute;
				for &v in &cmesh.vertices {
					let p = to_parent
						.transform_point3(vec3(v.x - cmesh.origin.x, v.y - cmesh.origin.y, 0.0))
						.truncate();
					mn = mn.min(p);
					mx = mx.max(p);
				}
			}
		}
		if mn.x.is_finite() {
			((mn + mx) * 0.5, (mx.x - mn.x) + padding * 2.0, (mx.y - mn.y) + padding * 2.0)
		} else {
			(Vec2::ZERO, 160.0, 160.0)
		}
	}

	fn reparent_into_deformer(
		&mut self,
		deformer_id: InoxNodeUuid,
		parent: InoxNodeUuid,
		child_ids: &[InoxNodeUuid],
		center: Vec2,
	) -> Result<(), String> {
		for &cid in child_ids {
			let same_parent = self.model.puppet.nodes.get_parent(cid).uuid == parent;
			if same_parent {
				if let Some(child_node) = self.model.puppet.nodes.get_node_mut(cid) {
					child_node.trans_offset.translation.x -= center.x;
					child_node.trans_offset.translation.y -= center.y;
				}
			}
			self.model.puppet.nodes.reparent(cid, deformer_id)?;
		}
		self.model.puppet.reset_contexts();
		self.revision += 1;
		Ok(())
	}
}

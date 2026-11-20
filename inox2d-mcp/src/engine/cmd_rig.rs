use glam::{vec2, Vec2};
use inox2d::animation::PuppetAnimation;
use inox2d::math::interp::InterpolateMode;
use inox2d::node::components::PhysicsModel;
use inox2d::params::AxisPoints;
use json::object::Object;
use json::JsonValue;

use crate::audit::{analyze_anatomy, audit_rig, auto_fix_rig};
use crate::rig::{
	add_pendulum_physics, add_standard_vtuber_params, apply_blink_binding, apply_breath_binding,
	apply_brow_binding, apply_deform_brush_mode, apply_head_perspective_binding, apply_head_roll_binding,
	apply_mouth_form_binding, apply_mouth_visemes_2d, apply_opacity_binding, apply_vertex_deform_delta,
	auto_rig_vtuber, clear_binding_keypoint, configure_pendulum_physics, create_idle_animation, ensure_param,
	mirror_deform_keypoint, remove_animation_keyframe, remove_param_binding, set_animation_keyframe,
	set_scalar_binding_keypoint, synthesize_2d_corners, weld_seam_vertices, DeformBrushMode,
};

use super::Session;

impl Session {
	pub(super) fn exec_rig_cmd(&mut self, cmd: &str, params: &JsonValue) -> Result<JsonValue, String> {
		match cmd {
			"param.add_standard_vtuber" => {
				let names = add_standard_vtuber_params(&mut self.model.puppet);
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					added_or_existing: names
				})
			}
			"param.set" => {
				if let JsonValue::Object(map) = &params["params"] {
					for (k, v) in map.iter() {
						if let JsonValue::Array(arr) = v {
							let x = arr.first().and_then(JsonValue::as_f32).unwrap_or(0.0);
							let y = arr.get(1).and_then(JsonValue::as_f32).unwrap_or(0.0);
							self.param_overrides.insert(k.to_owned(), vec2(x, y));
						} else if let Some(x) = v.as_f32() {
							self.param_overrides.insert(k.to_owned(), vec2(x, 0.0));
						}
					}
				}
				if let Some(p) = params["param"].as_str() {
					let x = params["x"].as_f32().unwrap_or(0.0);
					let y = params["y"].as_f32().unwrap_or(0.0);
					self.param_overrides.insert(p.to_owned(), vec2(x, y));
				}
				self.revision += 1;
				let mut out = Object::new();
				for (k, v) in &self.param_overrides {
					out.insert(k, JsonValue::Array(vec![JsonValue::from(v.x), JsonValue::from(v.y)]));
				}
				Ok(JsonValue::Object(out))
			}
			"rig.auto_head_perspective" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"].as_str().unwrap_or("Head:: Yaw-Pitch");
				let depth_z = params["depth_z"].as_f32().unwrap_or(0.5);
				let yaw_px = params["yaw_px"].as_f32().unwrap_or(26.0);
				let pitch_px = params["pitch_px"].as_f32().unwrap_or(20.0);

				apply_head_perspective_binding(&mut self.model.puppet, id, param_name, depth_z, yaw_px, pitch_px)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name,
					depth_z: depth_z
				})
			}
			"rig.auto_blink" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"].as_str().unwrap_or("Eye:: Left:: Open");
				let close_y_ratio = params["close_y_ratio"].as_f32().unwrap_or(0.65);

				apply_blink_binding(&mut self.model.puppet, id, param_name, close_y_ratio)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name,
					close_y_ratio: close_y_ratio
				})
			}
			"rig.auto_rig_vtuber" => {
				let actions = auto_rig_vtuber(&mut self.model.puppet);
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					actions: actions,
					params_count: self.model.puppet.params.len()
				})
			}
			"rig.auto_mouth_form" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"].as_str().unwrap_or("Mouth:: Form");
				let width_px = params["width_px"].as_f32().unwrap_or(6.0);
				let smile_y_px = params["smile_y_px"].as_f32().unwrap_or(5.0);
				apply_mouth_form_binding(&mut self.model.puppet, id, param_name, width_px, smile_y_px)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! { node_uuid: id.0, param: param_name })
			}
			"rig.auto_head_roll" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"].as_str().unwrap_or("Head:: Roll");
				let max_deg = params["max_deg"].as_f32().unwrap_or(15.0);
				apply_head_roll_binding(&mut self.model.puppet, id, param_name, max_deg)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! { node_uuid: id.0, param: param_name, max_deg: max_deg })
			}
			"rig.auto_breath" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"].as_str().unwrap_or("Breath");
				let scale_amp = params["scale_amp"].as_f32().unwrap_or(0.02);
				let y_amp = params["y_amp"].as_f32().unwrap_or(4.0);
				apply_breath_binding(&mut self.model.puppet, id, param_name, scale_amp, y_amp)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! { node_uuid: id.0, param: param_name })
			}
			"anim.create_idle" => {
				let anim_name = create_idle_animation(&mut self.model.puppet);
				let lanes = self
					.model
					.puppet
					.animations
					.get(&anim_name)
					.map(|a| a.lanes.len())
					.unwrap_or(0);
				self.revision += 1;
				Ok(json::object! {
					animation: anim_name,
					lanes: lanes
				})
			}
			"physics.add_pendulum" => {
				let name = params["name"].as_str().unwrap_or("Pendulum");
				let parent = if !params["parent_uuid"].is_null() || !params["parent_name"].is_null() {
					self.resolve_node(params, "parent_uuid", "parent_name")?
				} else {
					self.model.puppet.nodes.root_node_id
				};
				let param_name = params["param"]
					.as_str()
					.ok_or_else(|| "Missing 'param'".to_owned())?;
				let model_type = match params["model_type"].as_str().unwrap_or("Pendulum") {
					"SpringPendulum" => PhysicsModel::SpringPendulum,
					_ => PhysicsModel::RigidPendulum,
				};
				let length = params["length"].as_f32().unwrap_or(80.0);
				let frequency = params["frequency"].as_f32().unwrap_or(2.0);
				let angle_damping = params["angle_damping"].as_f32().unwrap_or(0.45);
				let output_scale = if let JsonValue::Array(arr) = &params["output_scale"] {
					vec2(
						arr.first().and_then(JsonValue::as_f32).unwrap_or(1.0),
						arr.get(1).and_then(JsonValue::as_f32).unwrap_or(1.0),
					)
				} else {
					vec2(1.0, 1.0)
				};

				let pid = add_pendulum_physics(
					&mut self.model.puppet,
					parent,
					name,
					param_name,
					model_type,
					length,
					frequency,
					angle_damping,
					output_scale,
				)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					uuid: pid.0,
					name: name,
					param: param_name
				})
			}
			"rig.bind_opacity" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"]
					.as_str()
					.ok_or_else(|| "Missing 'param'".to_owned())?;
				let op0 = params["opacity_at_0"].as_f32().unwrap_or(0.0);
				let op1 = params["opacity_at_1"].as_f32().unwrap_or(1.0);
				apply_opacity_binding(&mut self.model.puppet, id, param_name, op0, op1)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name,
					opacity_at_0: op0,
					opacity_at_1: op1
				})
			}
			"rig.analyze_anatomy" => Ok(analyze_anatomy(&mut self.model.puppet)),
			"rig.audit" => Ok(audit_rig(&mut self.model)),
			"rig.auto_fix" => {
				let res = auto_fix_rig(&mut self.model);
				self.revision += 1;
				Ok(res)
			}
			"deform.brush" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"]
					.as_str()
					.unwrap_or("Head:: Yaw-Pitch");
				let kx = params["kx"].as_f32().unwrap_or(0.0);
				let ky = params["ky"].as_f32().unwrap_or(0.0);
				let anchor = params["anchor"].as_str();
				let local_xy = if let JsonValue::Array(arr) = &params["local_xy"] {
					Some(vec2(
						arr.first().and_then(JsonValue::as_f32).unwrap_or(0.0),
						arr.get(1).and_then(JsonValue::as_f32).unwrap_or(0.0),
					))
				} else {
					None
				};
				let radius_ratio = params["radius_ratio"].as_f32().unwrap_or(0.45);
				let dx = params["dx"].as_f32().unwrap_or(0.0);
				let dy = params["dy"].as_f32().unwrap_or(0.0);
				let mode = params["mode"]
					.as_str()
					.map(DeformBrushMode::parse)
					.unwrap_or(DeformBrushMode::Grab);
				let (ix, iy, affected) = apply_deform_brush_mode(
					&mut self.model.puppet,
					id,
					param_name,
					vec2(kx, ky),
					anchor,
					local_xy,
					radius_ratio,
					vec2(dx, dy),
					mode,
				)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name,
					mode: mode.as_str(),
					grid_cell: [ix, iy],
					affected_vertices: affected
				})
			}
			"deform.set_vertices" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"].as_str().unwrap_or("Head:: Yaw-Pitch");
				let kx = params["kx"].as_f32().unwrap_or(0.0);
				let ky = params["ky"].as_f32().unwrap_or(0.0);
				let dx = params["dx"].as_f32().unwrap_or(0.0);
				let dy = params["dy"].as_f32().unwrap_or(0.0);
				let reset = params["reset"].as_bool().unwrap_or(false);
				let mut indices = Vec::new();
				if let JsonValue::Array(arr) = &params["indices"] {
					for item in arr {
						if let Some(vi) = item.as_u32() {
							indices.push(vi);
						}
					}
				}
				if indices.is_empty() {
					return Err("Missing non-empty 'indices' array".to_owned());
				}
				let (ix, iy, updated) = apply_vertex_deform_delta(
					&mut self.model.puppet,
					id,
					param_name,
					vec2(kx, ky),
					&indices,
					vec2(dx, dy),
					reset,
				)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name,
					grid_cell: [ix, iy],
					updated_vertices: updated
				})
			}
			"physics.configure" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let model_type = params["model_type"].as_str().map(|s| match s {
					"SpringPendulum" => PhysicsModel::SpringPendulum,
					_ => PhysicsModel::RigidPendulum,
				});
				let output_scale = if let JsonValue::Array(arr) = &params["output_scale"] {
					Some(vec2(
						arr.first().and_then(JsonValue::as_f32).unwrap_or(1.0),
						arr.get(1).and_then(JsonValue::as_f32).unwrap_or(1.0),
					))
				} else {
					None
				};
				configure_pendulum_physics(
					&mut self.model.puppet,
					id,
					model_type,
					params["length"].as_f32(),
					params["frequency"].as_f32(),
					params["angle_damping"].as_f32(),
					params["length_damping"].as_f32(),
					params["gravity"].as_f32(),
					output_scale,
				)?;
				self.revision += 1;
				Ok(json::object! { node_uuid: id.0, configured: true })
			}
			"anim.set_keyframe" => {
				let anim_name = params["anim"].as_str().unwrap_or("Idle");
				let param_name = params["param"]
					.as_str()
					.ok_or_else(|| "Missing 'param'".to_owned())?;
				let target = params["target"].as_u8().unwrap_or(0);
				let frame = params["frame"]
					.as_i32()
					.ok_or_else(|| "Missing 'frame'".to_owned())?;
				let value = params["value"]
					.as_f32()
					.ok_or_else(|| "Missing 'value'".to_owned())?;
				let tension = params["tension"].as_f32().unwrap_or(0.5);
				let interpolation = params["interpolation"].as_str().map(|s| {
					let mut name = s.to_owned();
					if let Some(first) = name.get_mut(0..1) {
						first.make_ascii_uppercase();
					}
					InterpolateMode::from_name(&name).unwrap_or(InterpolateMode::Linear)
				});
				set_animation_keyframe(
					&mut self.model.puppet,
					anim_name,
					param_name,
					target,
					frame,
					value,
					tension,
					interpolation,
				)?;
				self.revision += 1;
				Ok(json::object! { anim: anim_name, param: param_name, frame: frame, value: value })
			}
			"anim.remove_keyframe" => {
				let anim_name = params["anim"].as_str().unwrap_or("Idle");
				let param_name = params["param"]
					.as_str()
					.ok_or_else(|| "Missing 'param'".to_owned())?;
				let target = params["target"].as_u8().unwrap_or(0);
				let frame = params["frame"]
					.as_i32()
					.ok_or_else(|| "Missing 'frame'".to_owned())?;
				let removed = remove_animation_keyframe(&mut self.model.puppet, anim_name, param_name, target, frame)?;
				if removed {
					self.revision += 1;
				}
				Ok(json::object! { anim: anim_name, param: param_name, frame: frame, removed: removed })
			}
			"rig.auto_visemes" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"].as_str().unwrap_or("Mouth:: Viseme");
				let open_px = params["open_px"].as_f32().unwrap_or(12.0);
				let width_px = params["width_px"].as_f32().unwrap_or(8.0);
				apply_mouth_visemes_2d(&mut self.model.puppet, id, param_name, open_px, width_px)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name
				})
			}
			"rig.auto_brow" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let is_right = params["is_right"].as_bool().unwrap_or_else(|| {
					self.model
						.puppet
						.nodes
						.get_node(id)
						.map(|n| n.name.to_lowercase().contains("_r") || n.name.contains("右"))
						.unwrap_or(false)
				});
				let default_param = if is_right { "Brow:: Right" } else { "Brow:: Left" };
				let param_name = params["param"].as_str().unwrap_or(default_param);
				apply_brow_binding(&mut self.model.puppet, id, param_name, is_right)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name,
					is_right: is_right
				})
			}
			"param.create" => {
				let name = params["name"]
					.as_str()
					.map(str::trim)
					.filter(|s| !s.is_empty())
					.ok_or_else(|| "Missing non-empty 'name'".to_owned())?;
				if self.model.puppet.params.contains_key(name) {
					return Err(format!("Parameter '{name}' already exists"));
				}
				let is_vec2 = params["is_vec2"].as_bool().unwrap_or(false);
				let parse_v2 = |val: &JsonValue, def: Vec2| -> Vec2 {
					if let JsonValue::Array(arr) = val {
						vec2(
							arr.first().and_then(JsonValue::as_f32).unwrap_or(def.x),
							arr.get(1).and_then(JsonValue::as_f32).unwrap_or(def.y),
						)
					} else {
						def
					}
				};
				let min = parse_v2(&params["min"], Vec2::ZERO);
				let max = parse_v2(&params["max"], if is_vec2 { vec2(1.0, 1.0) } else { vec2(1.0, 0.0) });
				let defaults = parse_v2(&params["defaults"], Vec2::ZERO).clamp(min.min(max), min.max(max));
				let normalize_axis = |val: &JsonValue, fallback: Vec<f32>| -> Vec<f32> {
					let mut pts: Vec<f32> = if let JsonValue::Array(arr) = val {
						arr.iter()
							.filter_map(JsonValue::as_f32)
							.filter(|v| v.is_finite())
							.map(|v| v.clamp(0.0, 1.0))
							.collect()
					} else {
						fallback
					};
					pts.sort_by(f32::total_cmp);
					pts.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
					if pts.is_empty() {
						vec![0.0]
					} else {
						pts
					}
				};
				let axis_x = normalize_axis(&params["axis_x"], vec![0.0, 1.0]);
				let axis_y = if is_vec2 {
					normalize_axis(&params["axis_y"], vec![0.0, 1.0])
				} else {
					normalize_axis(&params["axis_y"], vec![0.0])
				};
				let uuid = ensure_param(
					&mut self.model.puppet,
					name,
					is_vec2,
					min,
					max,
					defaults,
					AxisPoints { x: axis_x, y: axis_y },
				);
				if !self.model.puppet.param_order.iter().any(|p| p == name) {
					self.model.puppet.param_order.push(name.to_owned());
				}
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					uuid: uuid.0,
					name: name,
					is_vec2: is_vec2
				})
			}
			"param.set_binding_keypoint" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"]
					.as_str()
					.ok_or_else(|| "Missing 'param'".to_owned())?;
				let kind = params["kind"]
					.as_str()
					.ok_or_else(|| "Missing 'kind'".to_owned())?;
				let ix = params["ix"].as_usize().unwrap_or(0);
				let iy = params["iy"].as_usize().unwrap_or(0);
				let value = params["value"]
					.as_f32()
					.ok_or_else(|| "Missing numeric 'value'".to_owned())?;
				set_scalar_binding_keypoint(&mut self.model.puppet, id, param_name, ix, iy, kind, value)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name,
					kind: kind,
					ix: ix,
					iy: iy,
					value: value
				})
			}
			"param.clear_binding_keypoint" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"]
					.as_str()
					.ok_or_else(|| "Missing 'param'".to_owned())?;
				let kind = params["kind"]
					.as_str()
					.ok_or_else(|| "Missing 'kind'".to_owned())?;
				let ix = params["ix"].as_usize().unwrap_or(0);
				let iy = params["iy"].as_usize().unwrap_or(0);
				let cleared = clear_binding_keypoint(&mut self.model.puppet, id, param_name, ix, iy, kind)?;
				self.model.puppet.reset_contexts();
				if cleared {
					self.revision += 1;
				}
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name,
					kind: kind,
					ix: ix,
					iy: iy,
					cleared: cleared
				})
			}
			"param.remove_binding" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"]
					.as_str()
					.ok_or_else(|| "Missing 'param'".to_owned())?;
				let kind = params["kind"].as_str();
				let removed = remove_param_binding(&mut self.model.puppet, id, param_name, kind)?;
				self.model.puppet.reset_contexts();
				if removed > 0 {
					self.revision += 1;
				}
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name,
					removed: removed
				})
			}
			"deform.mirror_keypoint" => {
				let id = self.resolve_node(params, "node_uuid", "node_name")?;
				let param_name = params["param"]
					.as_str()
					.ok_or_else(|| "Missing 'param'".to_owned())?;
				let src_ix = params["src_ix"].as_usize().unwrap_or(0);
				let src_iy = params["src_iy"].as_usize().unwrap_or(0);
				let dst_ix = params["dst_ix"].as_usize().unwrap_or(0);
				let dst_iy = params["dst_iy"].as_usize().unwrap_or(0);
				let mirror_x = params["mirror_x"].as_bool().unwrap_or(true);
				let verts = mirror_deform_keypoint(
					&mut self.model.puppet,
					id,
					param_name,
					src_ix,
					src_iy,
					dst_ix,
					dst_iy,
					mirror_x,
				)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name,
					mirrored_vertices: verts,
					mirror_x: mirror_x
				})
			}
			"param.synthesize_corners" => {
				let param_name = params["param"]
					.as_str()
					.ok_or_else(|| "Missing 'param'".to_owned())?;
				let target_node = if !params["node_uuid"].is_null() || !params["node_name"].is_null() {
					Some(self.resolve_node(params, "node_uuid", "node_name")?)
				} else {
					None
				};
				let (bindings_updated, cells_synthesized) =
					synthesize_2d_corners(&mut self.model.puppet, param_name, target_node)?;
				self.model.puppet.reset_contexts();
				if cells_synthesized > 0 {
					self.revision += 1;
				}
				Ok(json::object! {
					param: param_name,
					bindings_updated: bindings_updated,
					cells_synthesized: cells_synthesized
				})
			}
			"deform.weld_seam" => {
				let id_a = self.resolve_node(params, "node_a_uuid", "node_a_name")?;
				let id_b = self.resolve_node(params, "node_b_uuid", "node_b_name")?;
				let param_name = params["param"].as_str().unwrap_or("Head:: Yaw-Pitch");
				let kx = params["kx"].as_f32().unwrap_or(0.0);
				let ky = params["ky"].as_f32().unwrap_or(0.0);
				let max_dist_px = params["max_dist_px"].as_f32().unwrap_or(14.0);
				let weight = params["weight"].as_f32().unwrap_or(0.5);
				let (ix, iy, pairs) = weld_seam_vertices(
					&mut self.model.puppet,
					id_a,
					id_b,
					param_name,
					vec2(kx, ky),
					max_dist_px,
					weight,
				)?;
				self.model.puppet.reset_contexts();
				if pairs > 0 {
					self.revision += 1;
				}
				Ok(json::object! {
					node_a_uuid: id_a.0,
					node_b_uuid: id_b.0,
					param: param_name,
					grid_cell: [ix, iy],
					welded_pairs: pairs
				})
			}
			"anim.create" => {
				let name = params["name"]
					.as_str()
					.map(str::trim)
					.filter(|s| !s.is_empty())
					.ok_or_else(|| "Missing non-empty 'name'".to_owned())?;
				if self.model.puppet.animations.contains_key(name) {
					return Err(format!("Animation '{name}' already exists"));
				}
				let length = params["length"].as_i32().unwrap_or(120).clamp(1, 36000);
				let fps = params["fps"].as_f32().unwrap_or(30.0).clamp(1.0, 240.0);
				self.model.puppet.animations.insert(
					name.to_owned(),
					PuppetAnimation {
						timestep: 1.0 / fps,
						additive: false,
						length,
						lead_in: 0,
						lead_out: 0,
						lanes: Vec::new(),
					},
				);
				self.revision += 1;
				Ok(json::object! {
					name: name,
					length: length,
					fps: fps
				})
			}
			_ => Err(format!("Unknown command: '{cmd}'")),
		}
	}
}

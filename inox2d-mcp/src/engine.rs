use std::collections::HashMap;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

use glam::{vec2, Vec2};
use image::{Rgba, RgbaImage};
use inox2d::formats::inp::write_inp;
use inox2d::model::Model;
use inox2d::node::components::{Composite, Mesh, MeshGroup, PhysicsModel, SimplePhysics, TexturedMesh};
use inox2d::node::InoxNodeUuid;
use inox2d::texture::decode_model_textures;
use json::object::Object;
use json::JsonValue;

use crate::audit::{analyze_anatomy, audit_rig, auto_fix_rig};
use crate::headless_render::{render_anim_sheet, render_diagnostic, render_frame, render_grid, RenderOptions};
use crate::psd::{
	export_model_to_psd, import_psd_to_model, parse_psd_bytes, photocraft_convert, photocraft_inspect,
};
use crate::rig::{
	add_grid_part, add_pendulum_physics, add_standard_vtuber_params, apply_blink_binding, apply_breath_binding,
	apply_deform_brush, apply_head_perspective_binding, apply_head_roll_binding, apply_mouth_form_binding,
	apply_opacity_binding, auto_rig_vtuber, build_sample_vtuber_model, create_idle_animation,
	generate_contour_mesh, generate_grid_mesh,
};

pub struct CommandSpec {
	pub id: &'static str,
	pub description: &'static str,
	pub params_schema: &'static str,
}

pub fn command_registry() -> &'static [CommandSpec] {
	&[
		CommandSpec {
			id: "puppet.info",
			description: "Inspect puppet metadata, node hierarchy, mesh stats, parameters, and textures",
			params_schema: "{}",
		},
		CommandSpec {
			id: "puppet.new_sample",
			description: "Reset session to the built-in rigged sample VTuber puppet",
			params_schema: "{}",
		},
		CommandSpec {
			id: "puppet.save",
			description: "Save the current puppet model to an .inp binary file",
			params_schema: r#"{"path":"string"}"#,
		},
		CommandSpec {
			id: "puppet.import_psd",
			description: "Import a layered .psd artwork file into an Inochi2D puppet with optional contour meshing and auto-rigging",
			params_schema: r#"{"path":"string","name?":"string","contour_mesh?":true,"auto_rig?":true,"cols?":8,"rows?":8}"#,
		},
		CommandSpec {
			id: "puppet.export_psd",
			description: "Export the current puppet parts as a layered .psd file",
			params_schema: r#"{"path":"string","width?":1024,"height?":1024}"#,
		},
		CommandSpec {
			id: "photocraft.inspect",
			description: "Inspect a PSD/PCRAFT/image file using D:\\app\\PhotoCraft\\photocraft-cli.exe",
			params_schema: r#"{"path":"string"}"#,
		},
		CommandSpec {
			id: "photocraft.convert",
			description: "Convert between formats (.pcraft, .psd, .png, etc.) using D:\\app\\PhotoCraft\\photocraft-cli.exe",
			params_schema: r#"{"in_path":"string","out_path":"string"}"#,
		},
		CommandSpec {
			id: "node.add_part",
			description: "Add a textured Part node with an auto-generated subdivided grid mesh",
			params_schema: r#"{"name":"string","parent_uuid?":0,"parent_name?":"string","zsort?":0.0,"x?":0.0,"y?":0.0,"width?":120.0,"height?":120.0,"cols?":4,"rows?":4,"image_path?":"string","color?":[255,200,200,255]}"#,
		},
		CommandSpec {
			id: "mesh.auto_grid",
			description: "Rebuild an existing Part node's mesh into a subdivided cols x rows grid",
			params_schema: r#"{"node_uuid?":1,"node_name?":"string","width?":120.0,"height?":120.0,"cols":6,"rows":6}"#,
		},
		CommandSpec {
			id: "mesh.auto_contour",
			description: "Rebuild a Part node's mesh by culling transparent cells around its alpha contour",
			params_schema: r#"{"node_uuid?":1,"node_name?":"string","cols?":8,"rows?":8,"alpha_threshold?":4}"#,
		},
		CommandSpec {
			id: "param.add_standard_vtuber",
			description: "Add standard Inochi Session VTuber tracking parameters (Head:: Yaw-Pitch, Eye Open, Mouth Open/Form, Gaze, Breath, Hair Sway)",
			params_schema: "{}",
		},
		CommandSpec {
			id: "param.set",
			description: "Set parameter override values for rendering and inspection",
			params_schema: r#"{"param?":"Head:: Yaw-Pitch","x?":0.5,"y?":0.0,"params?":{"Head:: Yaw-Pitch":[0.5,0.0]}}"#,
		},
		CommandSpec {
			id: "rig.auto_rig_vtuber",
			description: "Automatically rig all Part/Group nodes by semantic names (EN/ZH) and create an Idle animation",
			params_schema: "{}",
		},
		CommandSpec {
			id: "rig.auto_head_perspective",
			description: "Bind 3x3 2.5D yaw-pitch perspective vertex deformation onto a Part node",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Face","param?":"Head:: Yaw-Pitch","depth_z?":0.5,"yaw_px?":26.0,"pitch_px?":20.0}"#,
		},
		CommandSpec {
			id: "rig.auto_blink",
			description: "Bind eyelid-closing deformation onto an Eye Part node",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Eye_L","param?":"Eye:: Left:: Open","close_y_ratio?":0.65}"#,
		},
		CommandSpec {
			id: "rig.auto_mouth_form",
			description: "Bind Mouth:: Form smile/frown vertex deformation onto a Mouth Part node",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Mouth","param?":"Mouth:: Form","width_px?":6.0,"smile_y_px?":5.0}"#,
		},
		CommandSpec {
			id: "rig.auto_head_roll",
			description: "Bind Head:: Roll Z-rotation onto a Head node",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Head","param?":"Head:: Roll","max_deg?":15.0}"#,
		},
		CommandSpec {
			id: "rig.auto_breath",
			description: "Bind Breath scaling/bobbing onto a Body or Torso node",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Body","param?":"Breath","scale_amp?":0.02,"y_amp?":4.0}"#,
		},
		CommandSpec {
			id: "rig.bind_opacity",
			description: "Bind a 0..1 opacity toggle/slider parameter onto a node (blush, tears, expressions)",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Blush","param":"Expr:: Blush","opacity_at_0?":0.0,"opacity_at_1?":1.0}"#,
		},
		CommandSpec {
			id: "rig.analyze_anatomy",
			description: "Measure all Parts in Face-normalized coordinates and margins so AI never guesses layout",
			params_schema: "{}",
		},
		CommandSpec {
			id: "rig.audit",
			description: "Run 6-point deterministic geometric/visual rig audit (containment, foldovers, stretch, blink, symmetry, masks)",
			params_schema: "{}",
		},
		CommandSpec {
			id: "rig.auto_fix",
			description: "Clamp facial feature containment leaks, relax triangle fold-overs, and attach missing masks",
			params_schema: "{}",
		},
		CommandSpec {
			id: "deform.brush",
			description: "Apply a Gaussian soft-brush deform to a parameter keyframe via semantic anchor or local coordinates",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Mouth","param":"Head:: Yaw-Pitch","kx?":1.0,"ky?":0.0,"anchor?":"center","local_xy?":[0.0,0.0],"radius_ratio?":0.45,"dx":4.0,"dy":-2.0}"#,
		},
		CommandSpec {
			id: "physics.add_pendulum",
			description: "Attach a SimplePhysics pendulum driver node targeting a parameter",
			params_schema: r#"{"name":"Hair_Physics","parent_uuid?":0,"parent_name?":"Head","param":"Hair:: Sway","model_type?":"Pendulum","length?":80.0,"frequency?":2.0,"angle_damping?":0.45,"output_scale?":[1.0,1.0]}"#,
		},
		CommandSpec {
			id: "anim.create_idle",
			description: "Create a 120-frame @ 30fps Idle animation (breath, blinks, head sway)",
			params_schema: "{}",
		},
		CommandSpec {
			id: "render.frame",
			description: "Render a single PNG frame using current parameter overrides",
			params_schema: r#"{"out":"frame.png","width?":512,"height?":512,"scale?":1.0,"wireframe?":false,"dt?":0.0}"#,
		},
		CommandSpec {
			id: "render.grid",
			description: "Render a 3x3 contact sheet PNG sweeping a parameter across its min..max range",
			params_schema: r#"{"out":"grid.png","param?":"Head:: Yaw-Pitch","tile_size?":220,"wireframe?":true}"#,
		},
		CommandSpec {
			id: "render.anim_sheet",
			description: "Render sampled animation or motion-test frames into a sprite contact sheet PNG",
			params_schema: r#"{"out":"anim_sheet.png","anim?":"Idle","frames?":8,"tile_size?":180,"wireframe?":false}"#,
		},
		CommandSpec {
			id: "render.diagnostic",
			description: "Render X-ray diagnostic PNG (50px coordinate grid, foldover/stretch heatmap, Face containment contour, pivots)",
			params_schema: r#"{"out":"diagnostic.png","width?":512,"height?":512,"scale?":1.0}"#,
		},
	]
}

pub struct Session {
	pub model: Model,
	pub param_overrides: HashMap<String, Vec2>,
	pub revision: u64,
}

impl Session {
	pub fn new(model: Model) -> Self {
		Self {
			model,
			param_overrides: HashMap::new(),
			revision: 0,
		}
	}

	pub fn new_sample() -> Self {
		Self::new(build_sample_vtuber_model())
	}

	pub fn resolve_node(&self, params: &JsonValue, uuid_key: &str, name_key: &str) -> Result<InoxNodeUuid, String> {
		if let Some(id) = params[uuid_key].as_u32() {
			if self.model.puppet.nodes.get_node(InoxNodeUuid(id)).is_some() {
				return Ok(InoxNodeUuid(id));
			}
			return Err(format!("Node UUID {id} not found"));
		}
		if let Some(name) = params[name_key].as_str() {
			for node in self.model.puppet.nodes.iter() {
				if node.name == name {
					return Ok(node.uuid);
				}
			}
			return Err(format!("Node '{name}' not found"));
		}
		Err(format!("Provide either '{uuid_key}' or '{name_key}'"))
	}

	pub fn exec(&mut self, cmd: &str, params: &JsonValue) -> Result<JsonValue, String> {
		match cmd {
			"puppet.info" => Ok(self.cmd_info()),
			"puppet.new_sample" => {
				self.model = build_sample_vtuber_model();
				self.param_overrides.clear();
				self.revision += 1;
				Ok(self.cmd_info())
			}
			"puppet.save" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				let file = File::create(Path::new(path)).map_err(|e| format!("Failed to create {path}: {e}"))?;
				let mut writer = BufWriter::new(file);
				write_inp(&self.model, &mut writer).map_err(|e| format!("Failed to write .inp: {e}"))?;
				Ok(json::object! {
					saved: path,
					textures: self.model.textures.len(),
					nodes: self.model.puppet.nodes.iter().count(),
					params: self.model.puppet.params.len()
				})
			}
			"puppet.import_psd" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				let bytes = fs::read(Path::new(path)).map_err(|e| format!("Failed to read '{path}': {e}"))?;
				let psd = parse_psd_bytes(&bytes)?;
				let model_name = params["name"]
					.as_str()
					.map(str::to_owned)
					.unwrap_or_else(|| {
						Path::new(path)
							.file_stem()
							.and_then(|s| s.to_str())
							.unwrap_or("PsdPuppet")
							.to_owned()
					});
				let contour_mesh = params["contour_mesh"].as_bool().unwrap_or(true);
				let auto_rig = params["auto_rig"].as_bool().unwrap_or(true);
				let cols = params["cols"].as_usize().unwrap_or(8);
				let rows = params["rows"].as_usize().unwrap_or(8);
				self.model = import_psd_to_model(&psd, &model_name, contour_mesh, auto_rig, cols, rows);
				self.param_overrides.clear();
				self.revision += 1;
				Ok(self.cmd_info())
			}
			"puppet.export_psd" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				let width = params["width"].as_u32().unwrap_or(1024);
				let height = params["height"].as_u32().unwrap_or(1024);
				let psd_bytes = export_model_to_psd(&mut self.model, width, height)?;
				fs::write(Path::new(path), &psd_bytes).map_err(|e| format!("Failed to write '{path}': {e}"))?;
				Ok(json::object! {
					saved: path,
					width: width,
					height: height,
					bytes: psd_bytes.len()
				})
			}
			"photocraft.inspect" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				photocraft_inspect(Path::new(path))
			}
			"photocraft.convert" => {
				let in_path = params["in_path"]
					.as_str()
					.ok_or_else(|| "Missing 'in_path'".to_owned())?;
				let out_path = params["out_path"]
					.as_str()
					.ok_or_else(|| "Missing 'out_path'".to_owned())?;
				let msg = photocraft_convert(Path::new(in_path), Path::new(out_path))?;
				Ok(json::object! { status: msg })
			}
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
			"render.frame" => {
				let out = params["out"].as_str().unwrap_or("frame.png");
				let mut opts = RenderOptions::default();
				if let Some(w) = params["width"].as_u32() {
					opts.width = w;
				}
				if let Some(h) = params["height"].as_u32() {
					opts.height = h;
				}
				if let Some(s) = params["scale"].as_f32() {
					opts.scale = s;
				}
				if let Some(wf) = params["wireframe"].as_bool() {
					opts.wireframe = wf;
				}
				if let Some(dt) = params["dt"].as_f32() {
					opts.dt = dt;
				}
				let img = render_frame(&mut self.model, &self.param_overrides, &opts)?;
				img.save(out).map_err(|e| format!("Failed to save '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					width: opts.width,
					height: opts.height,
					wireframe: opts.wireframe
				})
			}
			"render.grid" => {
				let out = params["out"].as_str().unwrap_or("grid.png");
				let param_name = params["param"].as_str().unwrap_or("Head:: Yaw-Pitch");
				let tile_size = params["tile_size"].as_u32().unwrap_or(220);
				let wireframe = params["wireframe"].as_bool().unwrap_or(true);
				let img = render_grid(
					&mut self.model,
					param_name,
					tile_size,
					wireframe,
					&self.param_overrides,
				)?;
				let (w, h) = img.dimensions();
				img.save(out).map_err(|e| format!("Failed to save '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					param: param_name,
					width: w,
					height: h,
					wireframe: wireframe
				})
			}
			"render.anim_sheet" => {
				let out = params["out"].as_str().unwrap_or("anim_sheet.png");
				let anim = params["anim"].as_str();
				let frames = params["frames"].as_u32().unwrap_or(8);
				let tile_size = params["tile_size"].as_u32().unwrap_or(180);
				let wireframe = params["wireframe"].as_bool().unwrap_or(false);
				let img = render_anim_sheet(&mut self.model, anim, frames, tile_size, wireframe)?;
				let (w, h) = img.dimensions();
				img.save(out).map_err(|e| format!("Failed to save '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					frames: frames,
					width: w,
					height: h
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
				let (ix, iy, affected) = apply_deform_brush(
					&mut self.model.puppet,
					id,
					param_name,
					vec2(kx, ky),
					anchor,
					local_xy,
					radius_ratio,
					vec2(dx, dy),
				)?;
				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					node_uuid: id.0,
					param: param_name,
					grid_cell: [ix, iy],
					affected_vertices: affected
				})
			}
			"render.diagnostic" => {
				let out = params["out"].as_str().unwrap_or("diagnostic.png");
				let width = params["width"].as_u32().unwrap_or(512);
				let height = params["height"].as_u32().unwrap_or(512);
				let scale = params["scale"].as_f32().unwrap_or(1.0);
				let img = render_diagnostic(&mut self.model, &self.param_overrides, width, height, scale)?;
				img.save(out).map_err(|e| format!("Failed to save '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					width: width,
					height: height
				})
			}
			other => Err(format!("Unknown command: '{other}'")),
		}
	}

	pub fn cmd_info(&self) -> JsonValue {
		let nodes: Vec<JsonValue> = self
			.model
			.puppet
			.nodes
			.pre_order_iter()
			.map(|n| {
				let comps = &self.model.puppet.node_comps;
				let kind = if comps.get::<TexturedMesh>(n.uuid).is_some() {
					"Part"
				} else if comps.get::<MeshGroup>(n.uuid).is_some() {
					"MeshGroup"
				} else if comps.get::<Composite>(n.uuid).is_some() {
					"Composite"
				} else if comps.get::<SimplePhysics>(n.uuid).is_some() {
					"SimplePhysics"
				} else {
					"Node"
				};
				let (verts, tris) = comps
					.get::<Mesh>(n.uuid)
					.map(|m| (m.vertices.len(), m.indices.len() / 3))
					.unwrap_or((0, 0));
				json::object! {
					uuid: n.uuid.0,
					name: n.name.clone(),
					type: kind,
					enabled: n.enabled,
					zsort: n.zsort,
					vertices: verts,
					triangles: tris
				}
			})
			.collect();

		let mut params_list: Vec<_> = self.model.puppet.params.values().collect();
		params_list.sort_by_key(|p| p.uuid);
		let params_json: Vec<JsonValue> = params_list
			.into_iter()
			.map(|p| {
				json::object! {
					uuid: p.uuid.0,
					name: p.name.clone(),
					is_vec2: p.is_vec2,
					min: [p.min.x, p.min.y],
					max: [p.max.x, p.max.y],
					defaults: [p.defaults.x, p.defaults.y],
					bindings: p.bindings.len()
				}
			})
			.collect();

		let mut anim_names: Vec<_> = self.model.puppet.animations.keys().cloned().collect();
		anim_names.sort();

		json::object! {
			name: self.model.puppet.meta.name.clone().unwrap_or_default(),
			version: self.model.puppet.meta.version.clone(),
			textures: self.model.textures.len(),
			node_count: nodes.len(),
			nodes: nodes,
			params: params_json,
			animations: anim_names
		}
	}
}

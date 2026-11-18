use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

use glam::{vec2, vec3, Vec2};
use image::{Rgba, RgbaImage};
use inox2d::formats::inp::write_inp;
use inox2d::math::transform::TransformOffset;
use inox2d::model::{Model, ModelTexture, VendorData};
use inox2d::node::components::{
	BlendMode, Composite, Drawable, Mask, MaskMode, Masks, Mesh, MeshGroup, PhysicsModel, SimplePhysics, TexturedMesh,
};
use inox2d::node::{InoxNode, InoxNodeUuid};
use inox2d::puppet::Puppet;
use inox2d::texture::decode_model_textures;
use json::object::Object;
use json::JsonValue;

use crate::audit::{analyze_anatomy, audit_rig, auto_fix_rig};
use crate::headless_render::{
	render_anim_gif, render_anim_sheet, render_diagnostic, render_diff, render_frame, render_grid, RenderOptions,
};
use crate::psd::{
	export_model_to_psd, import_psd_to_model, parse_psd_bytes, photocraft_convert, photocraft_inspect,
};
use crate::rig::{
	add_grid_part, add_pendulum_physics, add_standard_vtuber_params, apply_blink_binding, apply_breath_binding,
	apply_brow_binding, apply_deform_brush, apply_head_perspective_binding, apply_head_roll_binding,
	apply_mouth_form_binding, apply_mouth_visemes_2d, apply_opacity_binding, auto_rig_vtuber,
	build_sample_vtuber_model, create_idle_animation, generate_contour_mesh, generate_grid_mesh,
	synthesize_expression_overlay,
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
		CommandSpec {
			id: "render.diff",
			description: "Render side-by-side [Before | After | Pixel Heatmap] visual diff PNG against a checkpoint or previous undo state",
			params_schema: r#"{"out":"diff.png","checkpoint?":"string","tile_size?":256,"scale?":0.75}"#,
		},
		CommandSpec {
			id: "render.gif",
			description: "Render a looping animated .gif of an animation or continuous physics motion test",
			params_schema: r#"{"out":"preview.gif","anim?":"Idle","frames?":24,"fps?":15,"width?":320,"height?":320,"scale?":0.85,"wireframe?":false}"#,
		},
		CommandSpec {
			id: "edit.undo",
			description: "Undo the most recent mutating command",
			params_schema: "{}",
		},
		CommandSpec {
			id: "edit.redo",
			description: "Redo a previously undone command",
			params_schema: "{}",
		},
		CommandSpec {
			id: "session.checkpoint",
			description: "Save a named snapshot checkpoint of the current model",
			params_schema: r#"{"name":"before_sculpt"}"#,
		},
		CommandSpec {
			id: "session.restore",
			description: "Restore the model to a previously saved named checkpoint",
			params_schema: r#"{"name":"before_sculpt"}"#,
		},
		CommandSpec {
			id: "session.batch",
			description: "Execute multiple commands atomically in one call (rolls back all on error)",
			params_schema: r#"{"commands":[{"cmd":"string","params":{}}]}"#,
		},
		CommandSpec {
			id: "node.configure",
			description: "Update a node's name, parent, zsort, enabled, lock_to_root, transform, blend_mode, opacity, or tint",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Face","name?":"string","parent_name?":"string","zsort?":0.0,"enabled?":true,"lock_to_root?":false,"x?":0.0,"y?":0.0,"rot_z_deg?":0.0,"scale_x?":1.0,"scale_y?":1.0,"blend_mode?":"Normal","opacity?":1.0,"tint?":[1.0,1.0,1.0]}"#,
		},
		CommandSpec {
			id: "node.set_mask",
			description: "Configure or clear clipping masks on a Drawable node",
			params_schema: r#"{"node_name":"Pupil_L","sources":[{"name":"Eye_L","mode":"Mask"}],"threshold?":0.1}"#,
		},
		CommandSpec {
			id: "node.add_meshgroup",
			description: "Create a MeshGroup deformation cage over existing child nodes and reparent them into it",
			params_schema: r#"{"name":"EyeCage","parent_name?":"Head","children":["Eye_L"],"cols?":4,"rows?":4,"padding?":16.0}"#,
		},
		CommandSpec {
			id: "node.remove",
			description: "Remove a node subtree and clean up any parameter bindings referencing removed nodes",
			params_schema: r#"{"node_uuid?":1,"node_name?":"string"}"#,
		},
		CommandSpec {
			id: "node.synthesize_overlay",
			description: "Procedurally paint & attach an expression overlay (blush, gloom, tears, angry, shock) with 0..1 Opacity parameter",
			params_schema: r#"{"kind":"blush|gloom|tears|angry|shock","parent_name?":"Head"}"#,
		},
		CommandSpec {
			id: "rig.auto_visemes",
			description: "Bind 3x3 2D A/I/U/E/O vowel visemes (Mouth:: Viseme) onto a Mouth Part node",
			params_schema: r#"{"node_name":"Mouth","param?":"Mouth:: Viseme","open_px?":12.0,"width_px?":8.0}"#,
		},
		CommandSpec {
			id: "rig.auto_brow",
			description: "Bind 2D eyebrow expression deformations (Angle x Height) onto a Brow Part node",
			params_schema: r#"{"node_name":"Brow_L","param?":"Brow:: Left","is_right?":false}"#,
		},
	]
}

#[derive(Clone)]
pub struct ModelSnapshot {
	pub puppet_json: JsonValue,
	pub textures: Vec<ModelTexture>,
	pub vendors: Vec<VendorData>,
	pub param_overrides: HashMap<String, Vec2>,
}

pub struct Session {
	pub model: Model,
	pub param_overrides: HashMap<String, Vec2>,
	pub revision: u64,
	pub undo_stack: Vec<ModelSnapshot>,
	pub redo_stack: Vec<ModelSnapshot>,
	pub checkpoints: HashMap<String, ModelSnapshot>,
}

impl Session {
	pub fn new(model: Model) -> Self {
		Self {
			model,
			param_overrides: HashMap::new(),
			revision: 0,
			undo_stack: Vec::new(),
			redo_stack: Vec::new(),
			checkpoints: HashMap::new(),
		}
	}

	pub fn new_sample() -> Self {
		Self::new(build_sample_vtuber_model())
	}

	pub fn snapshot(&self) -> ModelSnapshot {
		ModelSnapshot {
			puppet_json: self.model.puppet.to_json(),
			textures: self.model.textures.clone(),
			vendors: self.model.vendors.clone(),
			param_overrides: self.param_overrides.clone(),
		}
	}

	pub fn restore_snapshot(&mut self, snap: &ModelSnapshot) -> Result<(), String> {
		let puppet = Puppet::new_from_json(&snap.puppet_json)
			.map_err(|e| format!("Failed to restore snapshot: {e}"))?;
		self.model = Model {
			puppet,
			textures: snap.textures.clone(),
			vendors: snap.vendors.clone(),
		};
		self.param_overrides = snap.param_overrides.clone();
		Ok(())
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

	fn resolve_node_by_ref(&self, val: &JsonValue) -> Result<InoxNodeUuid, String> {
		if let Some(id) = val.as_u32() {
			if self.model.puppet.nodes.get_node(InoxNodeUuid(id)).is_some() {
				return Ok(InoxNodeUuid(id));
			}
			return Err(format!("Node UUID {id} not found"));
		}
		if let Some(name) = val.as_str() {
			for node in self.model.puppet.nodes.iter() {
				if node.name == name {
					return Ok(node.uuid);
				}
			}
			return Err(format!("Node '{name}' not found"));
		}
		Err("Expected node name string or numeric UUID".to_owned())
	}

	fn is_mutating_cmd(cmd: &str) -> bool {
		!matches!(
			cmd,
			"puppet.info"
				| "puppet.save"
				| "puppet.export_psd"
				| "photocraft.inspect"
				| "photocraft.convert"
				| "rig.analyze_anatomy"
				| "rig.audit"
				| "render.frame"
				| "render.grid"
				| "render.anim_sheet"
				| "render.diagnostic"
				| "render.diff"
				| "render.gif"
				| "edit.undo"
				| "edit.redo"
				| "session.checkpoint"
				| "session.restore"
				| "session.batch"
		)
	}

	pub fn exec(&mut self, cmd: &str, params: &JsonValue) -> Result<JsonValue, String> {
		if cmd == "edit.undo" {
			let Some(prev) = self.undo_stack.pop() else {
				return Err("Nothing to undo".to_owned());
			};
			let current = self.snapshot();
			self.restore_snapshot(&prev)?;
			self.redo_stack.push(current);
			self.revision += 1;
			return Ok(json::object! {
				undone: true,
				revision: self.revision,
				undo_remaining: self.undo_stack.len(),
				redo_available: self.redo_stack.len()
			});
		}
		if cmd == "edit.redo" {
			let Some(next) = self.redo_stack.pop() else {
				return Err("Nothing to redo".to_owned());
			};
			let current = self.snapshot();
			self.restore_snapshot(&next)?;
			self.undo_stack.push(current);
			self.revision += 1;
			return Ok(json::object! {
				redone: true,
				revision: self.revision,
				undo_remaining: self.undo_stack.len(),
				redo_available: self.redo_stack.len()
			});
		}
		if cmd == "session.checkpoint" {
			let name = params["name"]
				.as_str()
				.ok_or_else(|| "Missing 'name'".to_owned())?;
			self.checkpoints.insert(name.to_owned(), self.snapshot());
			return Ok(json::object! {
				checkpoint: name,
				revision: self.revision,
				total_checkpoints: self.checkpoints.len()
			});
		}
		if cmd == "session.restore" {
			let name = params["name"]
				.as_str()
				.ok_or_else(|| "Missing 'name'".to_owned())?;
			let snap = self
				.checkpoints
				.get(name)
				.cloned()
				.ok_or_else(|| format!("Checkpoint '{name}' not found"))?;
			let current = self.snapshot();
			self.restore_snapshot(&snap)?;
			self.undo_stack.push(current);
			self.redo_stack.clear();
			self.revision += 1;
			return Ok(json::object! {
				restored: name,
				revision: self.revision
			});
		}
		if cmd == "session.batch" {
			let items = match &params["commands"] {
				JsonValue::Array(arr) => arr.clone(),
				_ => return Err("Missing 'commands' array".to_owned()),
			};
			let pre_batch = self.snapshot();
			let mut results = Vec::with_capacity(items.len());
			for (idx, item) in items.iter().enumerate() {
				let sub_cmd = item["cmd"]
					.as_str()
					.ok_or_else(|| format!("commands[{idx}] missing 'cmd'"))?;
				let sub_params = &item["params"];
				match self.exec_inner(sub_cmd, sub_params) {
					Ok(r) => results.push(json::object! { cmd: sub_cmd, ok: true, result: r }),
					Err(e) => {
						let _ = self.restore_snapshot(&pre_batch);
						return Err(format!("Batch aborted at step {idx} ({sub_cmd}): {e}"));
					}
				}
			}
			self.undo_stack.push(pre_batch);
			if self.undo_stack.len() > 40 {
				self.undo_stack.remove(0);
			}
			self.redo_stack.clear();
			return Ok(json::object! {
				executed: results.len(),
				revision: self.revision,
				results: results
			});
		}

		let snap = if Self::is_mutating_cmd(cmd) {
			Some(self.snapshot())
		} else {
			None
		};
		let res = self.exec_inner(cmd, params)?;
		if let Some(s) = snap {
			self.undo_stack.push(s);
			if self.undo_stack.len() > 40 {
				self.undo_stack.remove(0);
			}
			self.redo_stack.clear();
		}
		Ok(res)
	}

	fn exec_inner(&mut self, cmd: &str, params: &JsonValue) -> Result<JsonValue, String> {
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
			"render.diff" => {
				let out = params["out"].as_str().unwrap_or("diff.png");
				let tile_size = params["tile_size"].as_u32().unwrap_or(256);
				let scale = params["scale"].as_f32().unwrap_or(0.75);
				let snap = if let Some(cp_name) = params["checkpoint"].as_str() {
					self.checkpoints
						.get(cp_name)
						.cloned()
						.ok_or_else(|| format!("Checkpoint '{cp_name}' not found"))?
				} else if let Some(prev) = self.undo_stack.last() {
					prev.clone()
				} else {
					return Err("No checkpoint specified and undo history is empty".to_owned());
				};
				let before_puppet = Puppet::new_from_json(&snap.puppet_json)
					.map_err(|e| format!("Failed to restore baseline: {e}"))?;
				let mut before_model = Model {
					puppet: before_puppet,
					textures: snap.textures,
					vendors: snap.vendors,
				};
				let (img, changed_pct) =
					render_diff(&mut before_model, &mut self.model, &self.param_overrides, tile_size, scale)?;
				let (w, h) = img.dimensions();
				img.save(out).map_err(|e| format!("Failed to save '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					width: w,
					height: h,
					changed_pixel_pct: changed_pct
				})
			}
			"render.gif" => {
				let out = params["out"].as_str().unwrap_or("preview.gif");
				let anim = params["anim"].as_str();
				let frames = params["frames"].as_u32().unwrap_or(24);
				let fps = params["fps"].as_u32().unwrap_or(15);
				let width = params["width"].as_u32().unwrap_or(320);
				let height = params["height"].as_u32().unwrap_or(320);
				let scale = params["scale"].as_f32().unwrap_or(0.85);
				let wireframe = params["wireframe"].as_bool().unwrap_or(false);
				let gif_bytes = render_anim_gif(&mut self.model, anim, frames, fps, width, height, scale, wireframe)?;
				fs::write(out, &gif_bytes).map_err(|e| format!("Failed to write '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					frames: frames,
					fps: fps,
					bytes: gif_bytes.len()
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
						drawable.blending.mode = match bm {
							"Multiply" => BlendMode::Multiply,
							"ColorDodge" => BlendMode::ColorDodge,
							"LinearDodge" => BlendMode::LinearDodge,
							"Screen" => BlendMode::Screen,
							"ClipToLower" => BlendMode::ClipToLower,
							"SliceFromLower" => BlendMode::SliceFromLower,
							_ => BlendMode::Normal,
						};
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
				let parent = if !params["parent_uuid"].is_null() || !params["parent_name"].is_null() {
					self.resolve_node(params, "parent_uuid", "parent_name")?
				} else {
					self.model.puppet.nodes.root_node_id
				};
				let cols = params["cols"].as_usize().unwrap_or(4).max(1);
				let rows = params["rows"].as_usize().unwrap_or(4).max(1);
				let padding = params["padding"].as_f32().unwrap_or(16.0);

				let mut child_ids = Vec::new();
				if let JsonValue::Array(arr) = &params["children"] {
					for item in arr {
						child_ids.push(self.resolve_node_by_ref(item)?);
					}
				}

				// Compute union bounding box of target children relative to parent
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
				for &cid in &child_ids {
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
				let (center, w, h) = if mn.x.is_finite() {
					((mn + mx) * 0.5, (mx.x - mn.x) + padding * 2.0, (mx.y - mn.y) + padding * 2.0)
				} else {
					(Vec2::ZERO, 160.0, 160.0)
				};

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

				for &cid in &child_ids {
					let same_parent = self.model.puppet.nodes.get_parent(cid).uuid == parent;
					if same_parent {
						if let Some(child_node) = self.model.puppet.nodes.get_node_mut(cid) {
							child_node.trans_offset.translation.x -= center.x;
							child_node.trans_offset.translation.y -= center.y;
						}
					}
					self.model.puppet.nodes.reparent(cid, mg_id)?;
				}

				self.model.puppet.reset_contexts();
				self.revision += 1;
				Ok(json::object! {
					uuid: mg_id.0,
					name: name,
					reparented_children: child_ids.len(),
					size: [w, h]
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

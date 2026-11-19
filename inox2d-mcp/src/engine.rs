use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

use glam::{vec2, vec3, Vec2};
use image::{Rgba, RgbaImage};
use inox2d::animation::PuppetAnimation;
use inox2d::formats::inp::write_inp;
use inox2d::formats::ir_bridge::{ir_to_model, model_to_ir};
use inox2d::math::curve::CurveType;
use inox2d::math::interp::InterpolateMode;
use riglens_format::{Diagnostic, ExportOptions, FormatCapabilities, ImportOptions, Severity};
use inox2d::math::transform::TransformOffset;
use inox2d::model::{Model, ModelTexture, VendorData};
use inox2d::node::components::{
	BlendMode, Composite, Drawable, GridDeformer, GridFormation, Mask, MaskMode, Masks, Mesh, MeshGroup,
	PathDeformer, PhysicsModel, RawNodeData, SimplePhysics, TexturedMesh,
};
use inox2d::node::{InoxNode, InoxNodeUuid};
use inox2d::params::AxisPoints;
use inox2d::puppet::Puppet;
use inox2d::texture::decode_model_textures;
use json::object::Object;
use json::JsonValue;

use crate::audit::{analyze_anatomy, audit_rig, auto_fix_rig};
use crate::craft_ui::generate_rig_spec_card;
use crate::headless_render::{
	render_anim_gif, render_anim_sheet, render_diagnostic, render_diff, render_frame, render_grid, RenderOptions,
};
use crate::psd::{
	export_model_to_psd, import_psd_to_model_with_mode, parse_psd_bytes, photocraft_convert, photocraft_inspect,
};
use crate::rig::{
	add_grid_part, add_pendulum_physics, add_standard_vtuber_params, apply_blink_binding, apply_breath_binding,
	apply_brow_binding, apply_deform_brush, apply_head_perspective_binding, apply_head_roll_binding,
	apply_mouth_form_binding, apply_mouth_visemes_2d, apply_opacity_binding, apply_vertex_deform_delta,
	auto_rig_vtuber, build_sample_vtuber_model, clear_binding_keypoint, configure_pendulum_physics,
	create_idle_animation, ensure_param, generate_contour_mesh, generate_delaunay_mesh_with_options,
	generate_grid_mesh, mirror_deform_keypoint, remove_animation_keyframe, remove_param_binding,
	retriangulate_points_preserving_deforms, set_animation_keyframe, set_scalar_binding_keypoint,
	synthesize_expression_overlay, DelaunayMeshOptions, RefinementZone,
};

use riglens_booster::pipeline::{BoosterController, BoosterControllerConfig};
use riglens_booster::transport::spout2::{get_active_spout_sender, list_spout_senders};

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
			description: "Import a layered .psd artwork file into an Inochi2D puppet with optional Delaunay/contour meshing and auto-rigging",
			params_schema: r#"{"path":"string","name?":"string","mesh_mode?":"delaunay|contour|grid","contour_mesh?":true,"auto_rig?":true,"cols?":8,"rows?":8}"#,
		},
		CommandSpec {
			id: "puppet.export_psd",
			description: "Export the current puppet parts as a layered .psd file",
			params_schema: r#"{"path":"string","width?":1024,"height?":1024}"#,
		},
		CommandSpec {
			id: "format.list",
			description: "List all registered format importers and exporters with their capabilities",
			params_schema: "{}",
		},
		CommandSpec {
			id: "puppet.export",
			description: "Export the current puppet through the format plugin registry (.inp, .rigjson, .psd, or custom plugins)",
			params_schema: r#"{"path":"string","format?":"inp|rig-json|psd","strict?":false,"pretty?":true}"#,
		},
		CommandSpec {
			id: "puppet.import",
			description: "Import a puppet through the format plugin registry (.inp, .inx, .rigjson, .psd, or custom plugins)",
			params_schema: r#"{"path":"string","format?":"inp|rig-json|psd"}"#,
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
			id: "mesh.auto_delaunay",
			description: "Rebuild a Part node's mesh using constrained Bowyer-Watson Delaunay triangulation, closed-loop Douglas-Peucker contours, and adaptive corner/joint refinement",
			params_schema: r#"{"node_uuid?":1,"node_name?":"string","width?":100.0,"height?":100.0,"step_px?":22.0,"alpha_threshold?":4,"boundary_offset_px?":2.0,"constrained?":true,"refine_near_corners?":true,"refinement_zones?":[{"center":[0.0,0.0],"radius":24.0,"density_scale?":0.35}]}"#,
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
			description: "Run 10-point deterministic geometric/visual rig audit (containment, foldovers, stretch, blink, symmetry, masks, deformers, binding coverage, draw order, mask cycles)",
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
			id: "deform.set_vertices",
			description: "Move or reset explicit mesh vertex deform offsets on an active parameter keypoint",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Face","param":"Head:: Yaw-Pitch","kx?":0.0,"ky?":0.0,"indices":[0,1],"dx?":2.0,"dy?":-1.0,"reset?":false}"#,
		},
		CommandSpec {
			id: "mesh.edit_topology",
			description: "Manually move, insert, or remove rest mesh vertices and retriangulate via Delaunay while preserving Deform bindings",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Face","action":"move|insert|remove","indices?":[0],"x?":0.0,"y?":0.0,"dx?":0.0,"dy?":0.0}"#,
		},
		CommandSpec {
			id: "physics.add_pendulum",
			description: "Attach a SimplePhysics pendulum driver node targeting a parameter",
			params_schema: r#"{"name":"Hair_Physics","parent_uuid?":0,"parent_name?":"Head","param":"Hair:: Sway","model_type?":"Pendulum","length?":80.0,"frequency?":2.0,"angle_damping?":0.45,"output_scale?":[1.0,1.0]}"#,
		},
		CommandSpec {
			id: "physics.configure",
			description: "Modify length, frequency, damping, gravity, model type, or output scale of an existing SimplePhysics node",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Hair_Physics","model_type?":"Pendulum|SpringPendulum","length?":80.0,"frequency?":2.0,"angle_damping?":0.45,"length_damping?":0.5,"gravity?":1.0,"output_scale?":[1.0,1.0]}"#,
		},
		CommandSpec {
			id: "anim.create_idle",
			description: "Create a 120-frame @ 30fps Idle animation (breath, blinks, head sway)",
			params_schema: "{}",
		},
		CommandSpec {
			id: "anim.set_keyframe",
			description: "Insert or update a keyframe (frame, value, tension, interpolation) on an animation lane",
			params_schema: r#"{"anim?":"Idle","param":"Head:: Roll","target?":0,"frame":30,"value":0.4,"tension?":0.5,"interpolation?":"Linear|Nearest|Stepped|Cubic|Bezier"}"#,
		},
		CommandSpec {
			id: "anim.remove_keyframe",
			description: "Delete a keyframe at a specific frame from an animation lane",
			params_schema: r#"{"anim?":"Idle","param":"Head:: Roll","target?":0,"frame":30}"#,
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
			id: "craft.figcraft_card",
			description: "Generate a FigCraft SVG+PNG Rigging Spec Card showing puppet render preview, 6-point audit breakdown, mesh stats, and parameter pills",
			params_schema: r#"{"out":"spec_card.png"}"#,
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
		CommandSpec {
			id: "puppet.diagnostics",
			description: "Report Phase 0 load warnings, preserved opaque nijilive nodes, and passthrough metadata",
			params_schema: "{}",
		},
		CommandSpec {
			id: "node.add_grid_deformer",
			description: "Create a GridDeformer lattice over target children and reparent them into it",
			params_schema: r#"{"name":"HeadGrid","parent_name?":"Head","children":["Face"],"cols?":3,"rows?":3,"padding?":16.0}"#,
		},
		CommandSpec {
			id: "node.add_path_deformer",
			description: "Create a PathDeformer spline/bezier spine over target children and reparent them into it",
			params_schema: r#"{"name":"HairSpline","parent_name?":"Head","children":["Hair"],"points?":4,"curve_type?":"Spline|Bezier","orientation?":"vertical|horizontal","padding?":16.0}"#,
		},
		CommandSpec {
			id: "param.create",
			description: "Create a new 1D or 2D animation/rigging parameter with custom range and axis keypoints",
			params_schema: r#"{"name":"Expr:: Custom","is_vec2?":false,"min?":[0.0,0.0],"max?":[1.0,1.0],"defaults?":[0.0,0.0],"axis_x?":[0.0,1.0],"axis_y?":[0.0,1.0]}"#,
		},
		CommandSpec {
			id: "param.set_binding_keypoint",
			description: "Create or update a scalar binding keypoint (TransformTX/TY/SX/SY/RX/RY/RZ, ZSort, Opacity)",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Face","param":"Head:: Yaw-Pitch","kind":"TransformTX","ix":0,"iy":1,"value":12.0}"#,
		},
		CommandSpec {
			id: "param.clear_binding_keypoint",
			description: "Clear (unset & reset to neutral) a binding keypoint cell for Deform or a scalar binding",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Face","param":"Head:: Yaw-Pitch","kind":"Deform|TransformTX|...","ix":0,"iy":0}"#,
		},
		CommandSpec {
			id: "param.remove_binding",
			description: "Remove parameter bindings targeting a node (optionally filtered by kind)",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Face","param":"Head:: Yaw-Pitch","kind?":"Deform"}"#,
		},
		CommandSpec {
			id: "deform.mirror_keypoint",
			description: "Copy or X-mirror a Deform keypoint across the node's local center X",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Face","param":"Head:: Yaw-Pitch","src_ix":0,"src_iy":1,"dst_ix":2,"dst_iy":1,"mirror_x?":true}"#,
		},
		CommandSpec {
			id: "anim.create",
			description: "Create an empty named animation timeline container",
			params_schema: r#"{"name":"Wave","length?":120,"fps?":30}"#,
		},
		CommandSpec {
			id: "booster.status",
			description: "Query live RigLens Booster status, VTS connection, GPU adapter, Spout2 channels, and telemetry",
			params_schema: "{}",
		},
		CommandSpec {
			id: "booster.list_spout",
			description: "List active Windows Spout2 shared-texture senders and active sender name",
			params_schema: "{}",
		},
		CommandSpec {
			id: "booster.configure",
			description: "Configure RigLens Booster VTS, Spout2, physics, GPU rim/soft-body, and 2D-DLSS settings",
			params_schema: r#"{"vts_url?":"ws://127.0.0.1:8001","enable_vts?":true,"spout_in?":"VTubeStudioSpout","spout_out?":"RigLensBooster","tick_hz?":120.0,"wind_turbulence?":0.35,"enable_gpu?":true,"enable_soft_body?":true,"rim_intensity?":0.75,"rim_theme?":"cyberpunk","particle_count?":384,"upscale_factor?":1.0,"upscale_sharpness?":0.65,"frame_gen_multiplier?":1}"#,
		},
		CommandSpec {
			id: "booster.start",
			description: "Start the background RigLens Booster 120Hz physics + GPU/Spout2 pipeline (optionally applying config overrides)",
			params_schema: r#"{"vts_url?":"ws://127.0.0.1:8001","enable_vts?":true,"spout_in?":"VTubeStudioSpout","spout_out?":"RigLensBooster","tick_hz?":120.0,"wind_turbulence?":0.35,"enable_gpu?":true,"enable_soft_body?":true,"rim_intensity?":0.75,"rim_theme?":"cyberpunk","particle_count?":384,"upscale_factor?":1.0,"upscale_sharpness?":0.65,"frame_gen_multiplier?":1}"#,
		},
		CommandSpec {
			id: "booster.stop",
			description: "Stop the running RigLens Booster background pipeline",
			params_schema: "{}",
		},
	]
}

fn serde_to_json(val: &serde_json::Value) -> Result<JsonValue, String> {
	let serialized = serde_json::to_string(val).map_err(|e| format!("JSON serialize error: {e}"))?;
	json::parse(&serialized).map_err(|e| format!("JSON parse error: {e}"))
}

fn apply_booster_config_patch(base: &BoosterControllerConfig, params: &JsonValue) -> BoosterControllerConfig {
	let mut cfg = base.clone();
	if let Some(s) = params["vts_url"].as_str() {
		cfg.vts_url = s.to_owned();
	}
	if let Some(v) = params["enable_vts"].as_bool() {
		cfg.enable_vts = v;
	}
	if params.has_key("spout_in") {
		if params["spout_in"].is_null() {
			cfg.spout_in = None;
		} else if let Some(s) = params["spout_in"].as_str() {
			let t = s.trim();
			cfg.spout_in = if t.is_empty() { None } else { Some(t.to_owned()) };
		}
	}
	if let Some(s) = params["spout_out"].as_str() {
		cfg.spout_out = s.to_owned();
	}
	if let Some(v) = params["tick_hz"].as_f32() {
		cfg.tick_hz = v;
	}
	if let Some(v) = params["wind_turbulence"].as_f32() {
		cfg.wind_turbulence = v;
	}
	if let Some(v) = params["enable_gpu"].as_bool() {
		cfg.enable_gpu = v;
	}
	if let Some(v) = params["enable_soft_body"].as_bool() {
		cfg.enable_soft_body = v;
	}
	if let Some(v) = params["rim_intensity"].as_f32() {
		cfg.rim_intensity = v;
	}
	if let Some(s) = params["rim_theme"].as_str() {
		cfg.rim_theme = s.to_owned();
	}
	if let Some(v) = params["particle_count"].as_u32() {
		cfg.particle_count = v;
	}
	if let Some(v) = params["upscale_factor"].as_f32() {
		cfg.upscale_factor = v;
	}
	if let Some(v) = params["upscale_sharpness"].as_f32() {
		cfg.upscale_sharpness = v;
	}
	if let Some(v) = params["frame_gen_multiplier"].as_u32() {
		cfg.frame_gen_multiplier = v;
	}
	cfg
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
	pub booster: BoosterController,
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
			booster: BoosterController::default(),
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
				| "puppet.diagnostics"
				| "puppet.save"
				| "puppet.export_psd"
				| "puppet.export"
				| "format.list"
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
				| "craft.figcraft_card"
				| "edit.undo"
				| "edit.redo"
				| "session.checkpoint"
				| "session.restore"
				| "session.batch"
				| "booster.status"
				| "booster.list_spout"
				| "booster.configure"
				| "booster.start"
				| "booster.stop"
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
				let mesh_mode = if let Some(m) = params["mesh_mode"].as_str() {
					m
				} else if params["contour_mesh"].as_bool().unwrap_or(true) {
					"contour"
				} else {
					"grid"
				};
				let auto_rig = params["auto_rig"].as_bool().unwrap_or(true);
				let cols = params["cols"].as_usize().unwrap_or(8);
				let rows = params["rows"].as_usize().unwrap_or(8);
				self.model = import_psd_to_model_with_mode(&psd, &model_name, mesh_mode, auto_rig, cols, rows);
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
			"format.list" => {
				let reg = crate::formats::default_registry();
				Ok(json::object! {
					importers: caps_list_to_json(&reg.list_importers()),
					exporters: caps_list_to_json(&reg.list_exporters()),
				})
			}
			"puppet.export" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				let fmt_key = params["format"]
					.as_str()
					.or_else(|| Path::new(path).extension().and_then(|s| s.to_str()))
					.unwrap_or("inp");
				let reg = crate::formats::default_registry();
				let resolved_id = reg
					.resolve_exporter(fmt_key)
					.ok_or_else(|| format!("Unsupported export format '{fmt_key}'"))?
					.info()
					.id;
				let mut options = ExportOptions {
					pretty: params["pretty"].as_bool().unwrap_or(true),
					strict: params["strict"].as_bool().unwrap_or(false),
					..ExportOptions::default()
				};
				if let Some(w) = params["width"].as_u64() {
					options.custom.insert("width".to_owned(), serde_json::json!(w));
				}
				if let Some(h) = params["height"].as_u64() {
					options.custom.insert("height".to_owned(), serde_json::json!(h));
				}
				let doc = model_to_ir(&self.model);
				let file = File::create(Path::new(path)).map_err(|e| format!("Failed to create '{path}': {e}"))?;
				let mut writer = BufWriter::new(file);
				let report = reg
					.export_to_writer(&resolved_id, &doc, &mut writer, &options)
					.map_err(|e| e.to_string())?;
				Ok(json::object! {
					saved: path,
					format: resolved_id,
					bytes_written: report.bytes_written,
					warnings: diagnostics_to_json(&report.warnings),
				})
			}
			"puppet.import" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				let fmt_key = params["format"]
					.as_str()
					.or_else(|| Path::new(path).extension().and_then(|s| s.to_str()))
					.unwrap_or("inp");
				let reg = crate::formats::default_registry();
				let file = File::open(Path::new(path)).map_err(|e| format!("Failed to open '{path}': {e}"))?;
				let mut reader = std::io::BufReader::new(file);
				let report = reg
					.import_from_reader(fmt_key, &mut reader, &ImportOptions::default())
					.map_err(|e| e.to_string())?;
				let model = ir_to_model(&report.document).map_err(|e| e.to_string())?;
				self.model = model;
				self.param_overrides.clear();
				self.revision += 1;
				Ok(json::object! {
					info: self.cmd_info(),
					warnings: diagnostics_to_json(&report.warnings),
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
			"craft.figcraft_card" => {
				let out = params["out"].as_str().unwrap_or("spec_card.png");
				generate_rig_spec_card(&mut self.model, Path::new(out))
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
			"puppet.diagnostics" => {
				let raw_nodes: Vec<JsonValue> = self
					.model
					.puppet
					.nodes
					.pre_order_iter()
					.filter_map(|n| {
						self.model.puppet.node_comps.get::<RawNodeData>(n.uuid).map(|raw| {
							json::object! {
								uuid: n.uuid.0,
								name: n.name.clone(),
								type_name: raw.type_name.clone(),
								field_count: raw.fields.len(),
							}
						})
					})
					.collect();
				let top_keys: Vec<String> = self
					.model
					.puppet
					.top_extras
					.iter()
					.map(|(k, _)| k.to_owned())
					.collect();
				Ok(json::object! {
					warnings: self.model.puppet.diagnostics.clone(),
					raw_nodes: raw_nodes,
					param_extra_count: self.model.puppet.param_extras.len(),
					top_extra_keys: top_keys,
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
			"booster.status" => {
				let st = self.booster.status();
				let v = serde_json::to_value(&st).map_err(|e| format!("Status serialize error: {e}"))?;
				serde_to_json(&v)
			}
			"booster.list_spout" => Ok(json::object! {
				active_sender: get_active_spout_sender(),
				senders: list_spout_senders()
			}),
			"booster.configure" => {
				let cfg = apply_booster_config_patch(self.booster.config(), params);
				let st = self.booster.configure(cfg)?;
				let v = serde_json::to_value(&st).map_err(|e| format!("Status serialize error: {e}"))?;
				serde_to_json(&v)
			}
			"booster.start" => {
				let next_cfg = if params.is_object() && params.entries().next().is_some() {
					Some(apply_booster_config_patch(self.booster.config(), params))
				} else {
					None
				};
				let st = self.booster.start(next_cfg)?;
				let v = serde_json::to_value(&st).map_err(|e| format!("Status serialize error: {e}"))?;
				serde_to_json(&v)
			}
			"booster.stop" => {
				let st = self.booster.stop();
				let v = serde_json::to_value(&st).map_err(|e| format!("Status serialize error: {e}"))?;
				serde_to_json(&v)
			}
			other => Err(format!("Unknown command: '{other}'")),
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

	pub fn cmd_info(&self) -> JsonValue {
		let nodes: Vec<JsonValue> = self
			.model
			.puppet
			.nodes
			.pre_order_iter()
			.map(|n| {
				let comps = &self.model.puppet.node_comps;
				let kind: &str = if let Some(raw) = comps.get::<RawNodeData>(n.uuid) {
					raw.type_name.as_str()
				} else if comps.get::<TexturedMesh>(n.uuid).is_some() {
					"Part"
				} else if comps.get::<MeshGroup>(n.uuid).is_some() {
					"MeshGroup"
				} else if comps.get::<GridDeformer>(n.uuid).is_some() {
					"GridDeformer"
				} else if comps.get::<PathDeformer>(n.uuid).is_some() {
					"PathDeformer"
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
					.or_else(|| comps.get::<GridDeformer>(n.uuid).map(|g| (g.vertex_count(), 0)))
					.or_else(|| comps.get::<PathDeformer>(n.uuid).map(|p| (p.vertex_count(), 0)))
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
			animations: anim_names,
			warnings: self.model.puppet.diagnostics.clone()
		}
	}
}

pub fn caps_list_to_json(caps: &[FormatCapabilities]) -> JsonValue {
	JsonValue::Array(
		caps.iter()
			.map(|c| {
				json::object! {
					id: c.id.clone(),
					display_name: c.display_name.clone(),
					extensions: c.extensions.clone(),
					supports_grid_deformer: c.supports_grid_deformer,
					supports_path_deformer: c.supports_path_deformer,
					supports_cubic_interpolation: c.supports_cubic_interpolation,
					supports_animations: c.supports_animations,
					supports_opaque_passthrough: c.supports_opaque_passthrough,
					supports_textures: c.supports_textures,
					supports_vendor_data: c.supports_vendor_data,
				}
			})
			.collect(),
	)
}

fn diagnostics_to_json(diags: &[Diagnostic]) -> JsonValue {
	JsonValue::Array(
		diags
			.iter()
			.map(|d| {
				json::object! {
					severity: match d.severity {
						Severity::Info => "info",
						Severity::Warning => "warning",
					},
					code: d.code.clone(),
					node_id: d.node_id,
					message: d.message.clone(),
				}
			})
			.collect(),
	)
}

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
			description: "Apply a multi-mode soft deform brush (grab, smooth, bulge, twist) onto a parameter keyframe",
			params_schema: r#"{"node_uuid?":1,"node_name?":"Mouth","param":"Head:: Yaw-Pitch","mode?":"grab|smooth|bulge|twist","kx?":1.0,"ky?":0.0,"anchor?":"center","local_xy?":[0.0,0.0],"radius_ratio?":0.45,"dx":4.0,"dy":-2.0}"#,
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
			id: "param.synthesize_corners",
			description: "Synthesize 2D corner keypoints from cardinal X/Y axes (Live2D Cubism bilinear additive synthesis)",
			params_schema: r#"{"param":"Head:: Yaw-Pitch","node_uuid?":1,"node_name?":"Face"}"#,
		},
		CommandSpec {
			id: "deform.weld_seam",
			description: "Glue-style cross-node seam welding between two adjacent Part/Deformer meshes at a parameter keypoint",
			params_schema: r#"{"node_a_name":"Face","node_b_name":"Neck","param?":"Head:: Yaw-Pitch","kx?":0.0,"ky?":0.0,"max_dist_px?":14.0,"weight?":0.5}"#,
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

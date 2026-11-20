use super::*;
use inox2d_mcp::psd::import_psd_to_model;

#[test]
fn sample_vtuber_roundtrip_and_render_grid() {
	let model = build_sample_vtuber_model();
	let mut bytes = Vec::new();
	write_inp(&model, &mut bytes).expect("write sample .inp");
	let mut parsed = parse_inp(bytes.as_slice()).expect("parse sample .inp");

	let grid = render_grid(&mut parsed, "Head:: Yaw-Pitch", 128, true, &HashMap::new())
		.expect("render 3x3 head grid");
	assert_eq!(grid.width(), 128 * 3 + 4 * 4);
	assert_eq!(grid.height(), 128 * 3 + 4 * 4);

	// Verify blink actually deforms Eye_L when set to 0.0
	let mut open_params = HashMap::new();
	open_params.insert("Eye:: Left:: Open".to_owned(), vec2(1.0, 0.0));
	let frame_open = render_frame(&mut parsed, &open_params, &RenderOptions::default()).unwrap();

	let mut closed_params = HashMap::new();
	closed_params.insert("Eye:: Left:: Open".to_owned(), vec2(0.0, 0.0));
	let frame_closed = render_frame(&mut parsed, &closed_params, &RenderOptions::default()).unwrap();

	assert_ne!(frame_open.as_raw(), frame_closed.as_raw());
}

#[test]
fn engine_and_mcp_protocol_flow() {
	let mut session = Session::new_sample();

	// Add part, re-grid, bind perspective, step physics
	let add_res = session
		.exec(
			"node.add_part",
			&json::object! {
				name: "Ahoge",
				parent_name: "Head",
				zsort: -12.0,
				x: 0.0,
				y: -95.0,
				width: 40.0,
				height: 50.0,
				cols: 2,
				rows: 2
			},
		)
		.unwrap();
	assert_eq!(add_res["name"].as_str(), Some("Ahoge"));

	let grid_res = session
		.exec(
			"mesh.auto_grid",
			&json::object! {
				node_name: "Ahoge",
				cols: 4,
				rows: 5
			},
		)
		.unwrap();
	assert_eq!(grid_res["vertices"].as_u32(), Some(30));

	let rig_res = session
		.exec(
			"rig.auto_head_perspective",
			&json::object! {
				node_name: "Ahoge",
				depth_z: 1.4
			},
		)
		.unwrap();
	assert_eq!(rig_res["param"].as_str(), Some("Head:: Yaw-Pitch"));

	// Verify MCP initialize + tools/list + tools/call
	let init_resp = handle_mcp_request(
		&mut session,
		&json::object! {
			jsonrpc: "2.0",
			id: 1,
			method: "initialize",
			params: {}
		},
	)
	.unwrap();
	assert_eq!(init_resp["result"]["serverInfo"]["name"].as_str(), Some("inox2d-mcp"));

	let list_resp = handle_mcp_request(
		&mut session,
		&json::object! {
			jsonrpc: "2.0",
			id: 2,
			method: "tools/list",
			params: {}
		},
	)
	.unwrap();
	assert!(list_resp["result"]["tools"].members().any(|t| t["name"] == "rig_auto_head_perspective"));

	let call_resp = handle_mcp_request(
		&mut session,
		&json::object! {
			jsonrpc: "2.0",
			id: 3,
			method: "tools/call",
			params: {
				name: "rig_auto_head_perspective",
				arguments: {
					node_name: "Ahoge",
					depth_z: 1.4
				}
			}
		},
	)
	.unwrap();
	assert!(!call_resp["result"]["isError"].as_bool().unwrap_or(false));
	assert!(call_resp["result"]["content"][0]["text"]
		.as_str()
		.unwrap()
		.contains("Head:: Yaw-Pitch"));
}

#[test]
fn psd_roundtrip_contour_mesh_auto_rig_and_anim_sheet() {
	let mut sample = build_sample_vtuber_model();
	let psd_bytes = export_model_to_psd(&mut sample, 512, 512).expect("export sample to psd");
	let parsed_psd = parse_psd_bytes(&psd_bytes).expect("parse exported psd");
	assert_eq!(parsed_psd.width, 512);
	assert!(parsed_psd.layers.len() >= 8);

	let mut imported = import_psd_to_model(&parsed_psd, "PsdAutoRigged", true, true, 8, 8);
	assert!(imported.puppet.params.contains_key("Head:: Yaw-Pitch"));
	assert!(imported.puppet.params.contains_key("Mouth:: Form"));
	assert!(imported.puppet.animations.contains_key("Idle"));

	// Contour meshing should cull transparent corner cells on Face
	let face_id = imported
		.puppet
		.nodes
		.iter()
		.find(|n| n.name == "Face")
		.unwrap()
		.uuid;
	let face_mesh = imported.puppet.node_comps.get::<inox2d::node::components::Mesh>(face_id).unwrap();
	assert!(face_mesh.indices.len() / 3 <= 8 * 8 * 2);

	let sheet = render_anim_sheet(&mut imported, Some("Idle"), 8, 96, false).expect("render Idle sheet");
	assert_eq!(sheet.width(), 4 * 96 + 5 * 4);
	assert_eq!(sheet.height(), 2 * 96 + 3 * 4);
}

#[test]
fn meshgroup_deform_propagation_and_animation_roundtrip() {
	use inox2d::math::interp::InterpolateMode;
	use inox2d::math::matrix::Matrix2d;
	use inox2d::math::transform::TransformOffset;
	use inox2d::node::components::{BlendMode, Blending, Drawable, MeshGroup, TexturedMesh};
	use inox2d::node::InoxNode;
	use inox2d::params::{AxisPoints, Binding, BindingValues, Param};
	use inox2d::texture::TextureId;
	use inox2d_mcp::rig::{encode_rgba_png, generate_grid_mesh};

	let mut puppet = Puppet::new_empty("MeshGroupTest");
	let root_id = puppet.nodes.root_node_id;

	let mg_id = puppet.next_node_uuid();
	puppet.add_node(
		root_id,
		InoxNode {
			uuid: mg_id,
			name: "DeformBox".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset::default(),
			lock_to_root: false,
		},
	);
	puppet.node_comps.add(
		mg_id,
		MeshGroup {
			dynamic: true,
			translate_children: true,
		},
	);
	let mg_mesh = generate_grid_mesh(100.0, 100.0, 2, 2, vec2(0.0, 0.0));
	let mg_vert_count = mg_mesh.vertices.len();
	puppet.node_comps.add(mg_id, mg_mesh);

	let child_id = puppet.next_node_uuid();
	puppet.add_node(
		mg_id,
		InoxNode {
			uuid: child_id,
			name: "ChildPart".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset::default(),
			lock_to_root: false,
		},
	);
	puppet.node_comps.add(
		child_id,
		Drawable {
			blending: Blending {
				mode: BlendMode::Normal,
				tint: glam::vec3(1.0, 1.0, 1.0),
				screen_tint: glam::vec3(0.0, 0.0, 0.0),
				opacity: 1.0,
			},
			masks: None,
		},
	);
	puppet.node_comps.add(
		child_id,
		TexturedMesh {
			tex_albedo: TextureId(0),
			tex_emissive: TextureId(0),
			tex_bumpmap: TextureId(0),
		},
	);
	puppet
		.node_comps
		.add(child_id, generate_grid_mesh(40.0, 40.0, 2, 2, vec2(0.0, 0.0)));

	let p_uuid = puppet.next_param_uuid();
	let values = Matrix2d::from_grid_transposed(2, 1, |ix, _| {
		if ix == 0 {
			vec![vec2(0.0, 0.0); mg_vert_count]
		} else {
			vec![vec2(12.0, -6.0); mg_vert_count]
		}
	});
	let is_set = Matrix2d::from_grid_transposed(2, 1, |_, _| true);
	puppet.params.insert(
		"Warp".to_owned(),
		Param {
			uuid: p_uuid,
			name: "Warp".to_owned(),
			is_vec2: false,
			min: vec2(0.0, 0.0),
			max: vec2(1.0, 0.0),
			defaults: vec2(0.0, 0.0),
			axis_points: AxisPoints {
				x: vec![0.0, 1.0],
				y: vec![0.0],
			},
			bindings: vec![Binding {
				node: mg_id,
				is_set,
				interpolate_mode: InterpolateMode::Linear,
				values: BindingValues::Deform(values),
			}],
		},
	);
	inox2d_mcp::rig::create_idle_animation(&mut puppet);

	let img = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 100, 100, 255]));
	let model = Model {
		puppet,
		textures: vec![encode_rgba_png(&img)],
		vendors: Vec::new(),
	};

	let mut bytes = Vec::new();
	write_inp(&model, &mut bytes).expect("write meshgroup .inp");
	let mut parsed = parse_inp(bytes.as_slice()).expect("parse meshgroup .inp");

	assert!(parsed.puppet.animations.contains_key("Idle"));
	parsed.puppet.init_all();
	parsed.puppet.begin_frame();
	parsed.puppet.param_ctx.as_mut().unwrap().set("Warp", vec2(1.0, 0.0)).unwrap();
	parsed.puppet.end_frame(0.0);

	let child_deforms = &parsed.puppet.render_ctx.as_ref().unwrap().vertex_buffers.deforms;
	assert!(
		child_deforms.iter().any(|d| (d.x - 12.0).abs() < 0.2 && (d.y + 6.0).abs() < 0.2),
		"Expected MeshGroup deform (12, -6) to propagate to child vertices, got {child_deforms:?}"
	);
}

#[test]
fn opacity_binding_roundtrip_and_render() {
	let mut session = Session::new_sample();
	session
		.exec(
			"node.add_part",
			&json::object! {
				name: "Blush",
				parent_name: "Head",
				zsort: -2.0,
				x: 0.0,
				y: 12.0,
				width: 100.0,
				height: 30.0,
				color: [255, 90, 120, 220]
			},
		)
		.unwrap();
	session
		.exec(
			"rig.bind_opacity",
			&json::object! {
				node_name: "Blush",
				param: "Expr:: Blush",
				opacity_at_0: 0.0,
				opacity_at_1: 1.0
			},
		)
		.unwrap();

	let mut bytes = Vec::new();
	write_inp(&session.model, &mut bytes).expect("write opacity .inp");
	let mut parsed = parse_inp(bytes.as_slice()).expect("parse opacity .inp");

	let mut off_map = HashMap::new();
	off_map.insert("Expr:: Blush".to_owned(), vec2(0.0, 0.0));
	let img_off = render_frame(&mut parsed, &off_map, &RenderOptions::default()).unwrap();

	let mut on_map = HashMap::new();
	on_map.insert("Expr:: Blush".to_owned(), vec2(1.0, 0.0));
	let img_on = render_frame(&mut parsed, &on_map, &RenderOptions::default()).unwrap();

	assert_ne!(img_off.as_raw(), img_on.as_raw());
}

#[test]
fn audit_detects_leaks_and_foldovers_then_auto_fix_repairs() {
	let mut session = Session::new_sample();
	let anatomy = session.exec("rig.analyze_anatomy", &json::object! {}).unwrap();
	assert_eq!(anatomy["face_anchor"]["name"].as_str(), Some("Face"));

	// Intentionally push Eye_L's right edge far left across its own mesh and out of the face boundary
	session
		.exec(
			"deform.brush",
			&json::object! {
				node_name: "Eye_L",
				param: "Head:: Yaw-Pitch",
				kx: -1.0,
				ky: 0.0,
				anchor: "right",
				radius_ratio: 0.25,
				dx: -160.0,
				dy: 0.0
			},
		)
		.unwrap();

	let bad_report = session.exec("rig.audit", &json::object! {}).unwrap();
	assert_eq!(bad_report["passed"].as_bool(), Some(false));
	assert!(bad_report["summary"]["containment_leaks"].as_u32().unwrap() > 0);
	assert!(bad_report["summary"]["mesh_foldovers"].as_u32().unwrap() > 0);

	let fixed = session.exec("rig.auto_fix", &json::object! {}).unwrap();
	assert!(fixed["clamped_vertices"].as_u32().unwrap() > 0);
	assert!(fixed["relaxed_cells"].as_u32().unwrap() > 0);

	let clean_report = session.exec("rig.audit", &json::object! {}).unwrap();
	assert_eq!(clean_report["summary"]["containment_leaks"].as_u32(), Some(0));
	assert_eq!(clean_report["summary"]["mesh_foldovers"].as_u32(), Some(0));
	assert_eq!(clean_report["passed"].as_bool(), Some(true));

	let diag = render_diagnostic(&mut session.model, &HashMap::new(), 256, 256, 0.7).unwrap();
	assert_eq!(diag.dimensions(), (256, 256));
}

#[test]
fn undo_redo_checkpoint_batch_and_diff() {
	let mut session = Session::new_sample();
	session
		.exec("session.checkpoint", &json::object! { name: "base" })
		.unwrap();

	// Atomic batch succeeds and wraps into a single undo entry
	let batch_res = session
		.exec(
			"session.batch",
			&json::object! {
				commands: [
					{ cmd: "node.synthesize_overlay", params: { kind: "blush" } },
					{ cmd: "param.set", params: { param: "Expr:: Blush", x: 1.0 } }
				]
			},
		)
		.unwrap();
	assert_eq!(batch_res["executed"].as_u32(), Some(2));
	assert!(session.model.puppet.nodes.iter().any(|n| n.name == "Expr_Blush"));

	// Render side-by-side Before | After | Diff Heatmap
	let diff_path = std::env::temp_dir().join("inox2d_test_diff.png");
	let diff_res = session
		.exec(
			"render.diff",
			&json::object! {
				out: diff_path.to_string_lossy().as_ref(),
				checkpoint: "base",
				tile_size: 160
			},
		)
		.unwrap();
	assert!(diff_res["changed_pixel_pct"].as_f32().unwrap() > 0.1);
	let _ = fs::remove_file(&diff_path);

	// Failed batch rolls back cleanly
	let fail_batch = session.exec(
		"session.batch",
		&json::object! {
			commands: [
				{ cmd: "node.synthesize_overlay", params: { kind: "angry" } },
				{ cmd: "unknown.command", params: {} }
			]
		},
	);
	assert!(fail_batch.is_err());
	assert!(!session.model.puppet.nodes.iter().any(|n| n.name == "Expr_Angry"));

	// Undo removes the whole blush batch; redo restores it; restore resets to base
	session.exec("edit.undo", &json::object! {}).unwrap();
	assert!(!session.model.puppet.nodes.iter().any(|n| n.name == "Expr_Blush"));
	session.exec("edit.redo", &json::object! {}).unwrap();
	assert!(session.model.puppet.nodes.iter().any(|n| n.name == "Expr_Blush"));
	session
		.exec("session.restore", &json::object! { name: "base" })
		.unwrap();
	assert!(!session.model.puppet.nodes.iter().any(|n| n.name == "Expr_Blush"));
}

#[test]
fn node_crud_meshgroup_visemes_overlays_and_gif() {
	let mut session = Session::new_sample();

	// Configure Face opacity & Mouth visemes
	session
		.exec(
			"rig.auto_visemes",
			&json::object! { node_name: "Mouth", open_px: 14.0, width_px: 9.0 },
		)
		.unwrap();
	assert!(session.model.puppet.params.contains_key("Mouth:: Viseme"));

	// Add Pupil_L, clip it to Eye_L via node.set_mask
	session
		.exec(
			"node.add_part",
			&json::object! {
				name: "Pupil_L",
				parent_name: "Head",
				zsort: -5.5,
				x: -38.0,
				y: -6.0,
				width: 24.0,
				height: 24.0,
				color: [30, 30, 90, 255]
			},
		)
		.unwrap();
	let mask_res = session
		.exec(
			"node.set_mask",
			&json::object! {
				node_name: "Pupil_L",
				sources: [{ name: "Eye_L", mode: "Mask" }]
			},
		)
		.unwrap();
	assert_eq!(mask_res["mask_count"].as_u32(), Some(1));

	// Wrap Eye_L + Pupil_L inside a MeshGroup cage
	let mg_res = session
		.exec(
			"node.add_meshgroup",
			&json::object! {
				name: "LeftEyeCage",
				parent_name: "Head",
				children: ["Eye_L", "Pupil_L"],
				cols: 3,
				rows: 3
			},
		)
		.unwrap();
	assert_eq!(mg_res["reparented_children"].as_u32(), Some(2));

	// Synthesize tears + remove Pupil_L cleanly
	session
		.exec("node.synthesize_overlay", &json::object! { kind: "tears" })
		.unwrap();
	assert!(session.model.puppet.params.contains_key("Expr:: Tears"));
	session
		.exec("node.remove", &json::object! { node_name: "Pupil_L" })
		.unwrap();
	assert!(!session.model.puppet.nodes.iter().any(|n| n.name == "Pupil_L"));

	// Render animated GIF and verify GIF89a magic & decodable frames
	let gif_path = std::env::temp_dir().join("inox2d_test_idle.gif");
	let gif_res = session
		.exec(
			"render.gif",
			&json::object! {
				out: gif_path.to_string_lossy().as_ref(),
				anim: "Idle",
				frames: 6,
				fps: 10,
				width: 128,
				height: 128
			},
		)
		.unwrap();
	assert!(gif_res["bytes"].as_u32().unwrap() > 100);
	let gif_bytes = fs::read(&gif_path).unwrap();
	assert!(gif_bytes.starts_with(b"GIF89a"));
	let _ = fs::remove_file(&gif_path);
}

#[test]
fn delaunay_mesh_and_figcraft_card() {
	use image::{Rgba, RgbaImage};
	use inox2d_mcp::rig::generate_delaunay_mesh;

	// Concave L-shaped silhouette: top-right quadrant (x >= 64 && y < 64) is transparent.
	let mut img = RgbaImage::from_pixel(128, 128, Rgba([0, 0, 0, 0]));
	for y in 8..120 {
		for x in 8..120 {
			if !(x >= 68 && y <= 60) {
				img.put_pixel(x, y, Rgba([220, 180, 190, 255]));
			}
		}
	}

	let mesh = generate_delaunay_mesh(&img, 128.0, 128.0, 18.0, 4);
	assert!(mesh.vertices.len() >= 12);
	assert!(mesh.indices.len() >= 12 && mesh.indices.len() % 3 == 0);
	for &uv in &mesh.uvs {
		assert!(uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0);
	}
	// Ensure no triangle centroid falls deep inside the empty top-right bay (x > 84, y < 44 => local x > 20, y < -20).
	for tri in mesh.indices.chunks_exact(3) {
		let a = mesh.vertices[tri[0] as usize];
		let b = mesh.vertices[tri[1] as usize];
		let c = mesh.vertices[tri[2] as usize];
		let g = (a + b + c) / 3.0;
		assert!(
			!(g.x > 20.0 && g.y < -20.0),
			"Delaunay culling leaked triangle into concave bay at {:?}",
			g
		);
	}

	let mut session = Session::new_sample();
	let res = session
		.exec(
			"mesh.auto_delaunay",
			&json::object! { node_name: "Face", step_px: 20.0 },
		)
		.unwrap();
	assert_eq!(res["triangulation"].as_str(), Some("bowyer_watson_delaunay"));
	assert!(res["triangles"].as_u32().unwrap() >= 8);

	let card_png = std::env::temp_dir().join("inox2d_test_spec_card.png");
	let card_svg = card_png.with_extension("svg");
	let card_res = session
		.exec(
			"craft.figcraft_card",
			&json::object! { out: card_png.to_string_lossy().as_ref() },
		)
		.unwrap();
	assert!(card_png.exists());
	assert!(card_svg.exists());
	let png_bytes = fs::read(&card_png).unwrap();
	assert!(png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
	let svg_text = fs::read_to_string(&card_svg).unwrap();
	assert!(svg_text.contains("6-POINT RIG AUDIT BREAKDOWN"));
	assert!(card_res["audit_score"].as_u32().is_some());
	let _ = fs::remove_file(&card_png);
	let _ = fs::remove_file(&card_svg);
}

#[test]
fn manual_vertex_topology_physics_and_keyframe_commands() {
	use inox2d::node::components::{Mesh, SimplePhysics};
	use inox2d::params::BindingValues;

	let mut session = Session::new_sample();
	let face_id = session
		.model
		.puppet
		.nodes
		.iter()
		.find(|n| n.name == "Face")
		.unwrap()
		.uuid;

	let orig_verts = session
		.model
		.puppet
		.node_comps
		.get::<Mesh>(face_id)
		.unwrap()
		.vertices
		.len();

	// 1) Move explicit vertices in deform keypoint (kx = 1.0, ky = 0.0)
	let vres = session
		.exec(
			"deform.set_vertices",
			&json::object! {
				node_name: "Face",
				param: "Head:: Yaw-Pitch",
				kx: 1.0,
				ky: 0.0,
				indices: [0, 1],
				dx: 4.5,
				dy: -2.0
			},
		)
		.unwrap();
	assert_eq!(vres["updated_vertices"].as_u32(), Some(2));

	// 2) Insert a vertex via mesh.edit_topology & verify existing Face deform bindings resize & preserve!
	let ins = session
		.exec(
			"mesh.edit_topology",
			&json::object! { node_name: "Face", action: "insert", x: 5.0, y: 7.0 },
		)
		.unwrap();
	let new_verts = ins["vertices"].as_usize().unwrap();
	assert_eq!(new_verts, orig_verts + 1);

	let head_param = session.model.puppet.params.get("Head:: Yaw-Pitch").unwrap();
	let face_binding = head_param
		.bindings
		.iter()
		.find(|b| b.node == face_id && matches!(b.values, BindingValues::Deform(_)))
		.unwrap();
	if let BindingValues::Deform(ref mat) = face_binding.values {
		assert_eq!(mat.get(0, 0).unwrap().len(), new_verts);
		// Ensure non-zero head perspective binding survived retriangulation
		assert!(mat.get(2, 1).unwrap().iter().any(|v| v.length() > 0.5));
	}

	// 3) Remove the inserted vertex cleanly
	let inserted_idx = ins["inserted_index"].as_u32().unwrap();
	let rem = session
		.exec(
			"mesh.edit_topology",
			&json::object! { node_name: "Face", action: "remove", indices: [inserted_idx] },
		)
		.unwrap();
	assert_eq!(rem["vertices"].as_usize(), Some(orig_verts));

	// 4) Configure existing SimplePhysics pendulum
	let phys_id = session
		.model
		.puppet
		.nodes
		.iter()
		.find(|n| session.model.puppet.node_comps.get::<SimplePhysics>(n.uuid).is_some())
		.unwrap()
		.uuid;
	session
		.exec(
			"physics.configure",
			&json::object! { node_uuid: phys_id.0, length: 125.0, frequency: 3.2, angle_damping: 0.65 },
		)
		.unwrap();
	let phys = session.model.puppet.node_comps.get::<SimplePhysics>(phys_id).unwrap();
	assert!((phys.props.length - 125.0).abs() < 0.01);
	assert!((phys.props.frequency - 3.2).abs() < 0.01);

	// 5) Insert and remove animation keyframe
	session
		.exec(
			"anim.set_keyframe",
			&json::object! { anim: "Idle", param: "Head:: Roll", target: 0, frame: 45, value: 0.55, tension: 0.8 },
		)
		.unwrap();
	let rm_kf = session
		.exec(
			"anim.remove_keyframe",
			&json::object! { anim: "Idle", param: "Head:: Roll", target: 0, frame: 45 },
		)
		.unwrap();
	assert_eq!(rm_kf["removed"].as_bool(), Some(true));
}

#[test]
fn format_registry_export_import_and_strict_warnings() {
	let mut session = Session::new(build_sample_vtuber_model());
	session.exec("anim.create_idle", &json::object! {}).unwrap();

	// 1) format.list includes inp, rig-json, and psd
	let listed = session.exec("format.list", &json::object! {}).unwrap();
	let exp_ids: Vec<&str> = listed["exporters"]
		.members()
		.filter_map(|c| c["id"].as_str())
		.collect();
	assert!(exp_ids.contains(&"inp"));
	assert!(exp_ids.contains(&"rig-json"));
	assert!(exp_ids.contains(&"psd"));

	let dir = std::env::temp_dir().join("riglens_mcp_format_test");
	let _ = fs::create_dir_all(&dir);
	let rigjson_path = dir.join("sample.rigjson");
	let psd_path = dir.join("sample.psd");

	// 2) Export to .rigjson cleanly with zero warnings
	let exp_json = session
		.exec(
			"puppet.export",
			&json::object! { path: rigjson_path.to_str().unwrap(), pretty: true },
		)
		.unwrap();
	assert_eq!(exp_json["format"].as_str(), Some("rig-json"));
	assert!(exp_json["bytes_written"].as_usize().unwrap() > 100);
	assert_eq!(exp_json["warnings"].len(), 0);

	// 3) Non-strict export to .psd succeeds and reports structured capability warnings
	let exp_psd = session
		.exec(
			"puppet.export",
			&json::object! { path: psd_path.to_str().unwrap(), width: 256, height: 256 },
		)
		.unwrap();
	assert_eq!(exp_psd["format"].as_str(), Some("psd"));
	assert!(exp_psd["bytes_written"].as_usize().unwrap() > 0);
	let has_anim_warn = exp_psd["warnings"]
		.members()
		.any(|w| w["code"].as_str() == Some("UNSUPPORTED_ANIMATIONS"));
	assert!(has_anim_warn, "expected UNSUPPORTED_ANIMATIONS warning: {:?}", exp_psd["warnings"]);

	// 4) Strict export to .psd rejects because animations are dropped
	let strict_err = session
		.exec(
			"puppet.export",
			&json::object! { path: psd_path.to_str().unwrap(), strict: true },
		)
		.unwrap_err();
	assert!(strict_err.contains("strict mode"), "unexpected error: {strict_err}");

	// 5) Re-import .rigjson back into a fresh session
	let orig_nodes = session.model.puppet.nodes.iter().count();
	let mut fresh = Session::new(Model {
		puppet: Puppet::new_empty("Empty"),
		textures: Vec::new(),
		vendors: Vec::new(),
	});
	let imp = fresh
		.exec("puppet.import", &json::object! { path: rigjson_path.to_str().unwrap() })
		.unwrap();
	assert_eq!(imp["info"]["node_count"].as_usize(), Some(orig_nodes));
	assert!(fresh.model.puppet.animations.contains_key("Idle"));

	let _ = fs::remove_dir_all(&dir);
}

#[test]
fn audit_ten_point_checks_detect_deformers_bindings_draw_order_and_mask_cycles() {
	use inox2d::math::curve::CurveType;
	use inox2d::math::interp::InterpolateMode;
	use inox2d::math::matrix::Matrix2d;
	use inox2d::math::transform::TransformOffset;
	use inox2d::node::components::{
		GridDeformer, GridFormation, Mask, MaskMode, Masks, PathDeformer,
	};
	use inox2d::node::{InoxNode, InoxNodeUuid};
	use inox2d::params::{Binding, BindingValues};

	let mut session = Session::new_sample();

	// Clean sample passes all 10 audit checks
	let clean = session.exec("rig.audit", &json::object! {}).unwrap();
	assert_eq!(clean["passed"].as_bool(), Some(true), "clean sample failed audit: {clean}");
	assert_eq!(clean["summary"]["deformer_issues"].as_u32(), Some(0));
	assert_eq!(clean["summary"]["binding_coverage"].as_u32(), Some(0));
	assert_eq!(clean["summary"]["draw_order_conflicts"].as_u32(), Some(0));
	assert_eq!(clean["summary"]["mask_cycles"].as_u32(), Some(0));

	// Inject Check 7 defects: degenerate GridDeformer + degenerate PathDeformer (both unbound)
	let root = session.model.puppet.nodes.root_node_id;
	let bad_grid_id = session.model.puppet.next_node_uuid();
	session.model.puppet.add_node(
		root,
		InoxNode {
			uuid: bad_grid_id,
			name: "BrokenGrid".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset::default(),
			lock_to_root: false,
		},
	);
	session.model.puppet.node_comps.set(
		bad_grid_id,
		GridDeformer {
			axis_x: vec![0.0],
			axis_y: vec![-10.0, 10.0],
			formation: GridFormation::Bilinear,
			dynamic: false,
			translate_children: true,
		},
	);

	let bad_path_id = session.model.puppet.next_node_uuid();
	session.model.puppet.add_node(
		root,
		InoxNode {
			uuid: bad_path_id,
			name: "BrokenPath".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset::default(),
			lock_to_root: false,
		},
	);
	session.model.puppet.node_comps.set(
		bad_path_id,
		PathDeformer {
			curve_type: CurveType::Spline,
			vertices: vec![glam::vec2(0.0, 0.0)],
			dynamic: false,
			physics_only: false,
		},
	);

	// Inject Check 8 defects: dangling target UUID & wrong deform vertex count
	let face_id = session
		.model
		.puppet
		.nodes
		.iter()
		.find(|n| n.name == "Face")
		.unwrap()
		.uuid;
	let eye_l_id = session
		.model
		.puppet
		.nodes
		.iter()
		.find(|n| n.name == "Eye_L")
		.unwrap()
		.uuid;
	let mouth_id = session
		.model
		.puppet
		.nodes
		.iter()
		.find(|n| n.name == "Mouth")
		.unwrap()
		.uuid;

	if let Some(roll) = session.model.puppet.params.get_mut("Head:: Roll") {
		roll.bindings.push(Binding {
			node: InoxNodeUuid(99999),
			is_set: Matrix2d::from_grid_transposed(3, 1, |_, _| false),
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::ZSort(Matrix2d::from_grid_transposed(3, 1, |_, _| 0.0)),
		});
		roll.bindings.push(Binding {
			node: face_id,
			is_set: Matrix2d::from_grid_transposed(3, 1, |ix, _| ix == 0),
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Deform(Matrix2d::from_grid_transposed(3, 1, |_, _| {
				vec![glam::vec2(0.0, 0.0); 2]
			})),
		});
	}

	// Inject Check 9 defects: move Eye_L behind Face and collide Mouth zsort with Face
	session.model.puppet.nodes.get_node_mut(eye_l_id).unwrap().zsort = 5.0;
	session.model.puppet.nodes.get_node_mut(mouth_id).unwrap().zsort = 0.0;

	// Inject Check 10 defects: 2-node mask cycle between Eye_L and Face
	session.model.puppet.node_comps.set(
		eye_l_id,
		Masks {
			threshold: 0.5,
			masks: vec![Mask {
				source: face_id,
				mode: MaskMode::Mask,
			}],
		},
	);
	session.model.puppet.node_comps.set(
		face_id,
		Masks {
			threshold: 0.5,
			masks: vec![Mask {
				source: eye_l_id,
				mode: MaskMode::Mask,
			}],
		},
	);

	session.model.puppet.reset_contexts();
	let dirty = session.exec("rig.audit", &json::object! {}).unwrap();
	assert_eq!(dirty["passed"].as_bool(), Some(false));
	assert!(dirty["summary"]["deformer_issues"].as_u32().unwrap() >= 2, "{dirty}");
	assert!(dirty["summary"]["binding_coverage"].as_u32().unwrap() >= 2, "{dirty}");
	assert!(dirty["summary"]["draw_order_conflicts"].as_u32().unwrap() >= 2, "{dirty}");
	assert!(dirty["summary"]["mask_cycles"].as_u32().unwrap() >= 1, "{dirty}");
}

#[test]
fn constrained_delaunay_respects_concave_bay_offset_and_joint_refinement() {
	use image::{Rgba, RgbaImage};
	use inox2d_mcp::rig::{generate_delaunay_mesh_with_options, DelaunayMeshOptions, RefinementZone};

	// Paint a crisp 160x160 C-shaped silhouette with a wide open concave bay on the right.
	let mut img = RgbaImage::from_pixel(160, 160, Rgba([0, 0, 0, 0]));
	let solid = Rgba([220, 180, 160, 255]);
	for y in 20..140 {
		for x in 20..140 {
			let in_spine = x < 56;
			let in_top = y < 52;
			let in_bot = y >= 108;
			if in_spine || in_top || in_bot {
				img.put_pixel(x, y, solid);
			}
		}
	}

	// 1) Constrained Delaunay with 0px offset must not bridge across the empty concave bay.
	let c_mesh = generate_delaunay_mesh_with_options(
		&img,
		160.0,
		160.0,
		&DelaunayMeshOptions {
			step_px: 18.0,
			alpha_threshold: 4,
			boundary_offset_px: 0.0,
			constrained: true,
			refine_near_corners: true,
			refinement_zones: Vec::new(),
		},
	);
	let tri_count = c_mesh.indices.len() / 3;
	assert!(tri_count >= 20, "expected >= 20 triangles, got {tri_count}");

	for tri in c_mesh.indices.chunks_exact(3) {
		let a = c_mesh.vertices[tri[0] as usize];
		let b = c_mesh.vertices[tri[1] as usize];
		let c = c_mesh.vertices[tri[2] as usize];
		let area2 = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
		assert!(area2 > 0.0, "triangle not CCW or degenerate: {area2}");
		let centroid = (a + b + c) / 3.0;
		let in_empty_bay = centroid.x > -18.0 && centroid.x < 56.0 && centroid.y.abs() < 22.0;
		assert!(
			!in_empty_bay,
			"constrained Delaunay bridged empty concave bay at {centroid:?}"
		);
	}

	// 2) boundary_offset_px > 0 expands mesh bounds outward cleanly.
	let expanded = generate_delaunay_mesh_with_options(
		&img,
		160.0,
		160.0,
		&DelaunayMeshOptions {
			step_px: 18.0,
			alpha_threshold: 4,
			boundary_offset_px: 6.0,
			constrained: true,
			refine_near_corners: false,
			refinement_zones: Vec::new(),
		},
	);
	let span = |m: &inox2d::node::components::Mesh| -> (f32, f32) {
		let mut mn = glam::vec2(f32::INFINITY, f32::INFINITY);
		let mut mx = glam::vec2(f32::NEG_INFINITY, f32::NEG_INFINITY);
		for &v in &m.vertices {
			mn = mn.min(v);
			mx = mx.max(v);
		}
		(mx.x - mn.x, mx.y - mn.y)
	};
	let (w0, h0) = span(&c_mesh);
	let (w6, h6) = span(&expanded);
	assert!(w6 > w0 + 4.0 && h6 > h0 + 4.0, "expected outward offset ({w0},{h0}) -> ({w6},{h6})");

	// 3) Explicit RefinementZone densifies joint region without exploding distant regions.
	let joint_center = glam::vec2(-42.0, 0.0);
	let coarse = generate_delaunay_mesh_with_options(
		&img,
		160.0,
		160.0,
		&DelaunayMeshOptions {
			step_px: 20.0,
			alpha_threshold: 4,
			boundary_offset_px: 0.0,
			constrained: true,
			refine_near_corners: false,
			refinement_zones: Vec::new(),
		},
	);
	let refined = generate_delaunay_mesh_with_options(
		&img,
		160.0,
		160.0,
		&DelaunayMeshOptions {
			step_px: 20.0,
			alpha_threshold: 4,
			boundary_offset_px: 0.0,
			constrained: true,
			refine_near_corners: false,
			refinement_zones: vec![RefinementZone {
				center: joint_center,
				radius: 26.0,
				density_scale: 0.3,
			}],
		},
	);
	let count_near = |m: &inox2d::node::components::Mesh| {
		m.vertices
			.iter()
			.filter(|&&v| (v - joint_center).length() <= 26.0)
			.count()
	};
	let c_cnt = count_near(&coarse);
	let r_cnt = count_near(&refined);
	assert!(
		r_cnt > c_cnt * 2,
		"expected refinement zone to multiply local vertices: coarse={c_cnt}, refined={r_cnt}"
	);

	// 4) Exercise MCP command `mesh.auto_delaunay` with new parameters end-to-end.
	let mut session = Session::new_sample();
	let res = session
		.exec(
			"mesh.auto_delaunay",
			&json::object! {
				node_name: "Face",
				step_px: 20.0,
				boundary_offset_px: 3.0,
				constrained: true,
				refine_near_corners: true,
				refinement_zones: [
					{ center: [0.0, 0.0], radius: 28.0, density_scale: 0.4 }
				]
			},
		)
		.unwrap();
	assert_eq!(res["triangulation"].as_str(), Some("bowyer_watson_delaunay"));
	assert_eq!(res["constrained"].as_bool(), Some(true));
	assert!(res["triangles"].as_u32().unwrap() >= 20);
}

#[test]
fn editor_command_layer_deformers_bindings_blendmodes_and_diagnostics() {
	let mut session = Session::new_sample();

	// 1) All 19 nijilive blend modes accepted by node.configure
	for bm in inox2d::node::components::BlendMode::VALUES {
		session
			.exec(
				"node.configure",
				&json::object! { node_name: "Face", blend_mode: bm.as_str() },
			)
			.unwrap();
	}
	assert!(session
		.exec("node.configure", &json::object! { node_name: "Face", blend_mode: "BogusMode" })
		.is_err());

	// 2) Add GridDeformer over Face and sculpt via deform.set_vertices + deform.mirror_keypoint
	let gd = session
		.exec(
			"node.add_grid_deformer",
			&json::object! {
				name: "FaceGrid",
				parent_name: "Head",
				children: ["Face"],
				cols: 2,
				rows: 2
			},
		)
		.unwrap();
	let gd_id = gd["uuid"].as_u32().unwrap();
	assert_eq!(gd["reparented_children"].as_u32(), Some(1));

	session
		.exec(
			"deform.set_vertices",
			&json::object! {
				node_uuid: gd_id,
				param: "Head:: Yaw-Pitch",
				kx: -1.0,
				ky: 0.0,
				indices: [0, 3],
				dx: 10.0,
				dy: 2.0
			},
		)
		.unwrap();
	let mir = session
		.exec(
			"deform.mirror_keypoint",
			&json::object! {
				node_uuid: gd_id,
				param: "Head:: Yaw-Pitch",
				src_ix: 0,
				src_iy: 1,
				dst_ix: 2,
				dst_iy: 1,
				mirror_x: true
			},
		)
		.unwrap();
	assert_eq!(mir["mirrored_vertices"].as_u32(), Some(9));

	// 3) Add PathDeformer and deform with brush
	let pd = session
		.exec(
			"node.add_path_deformer",
			&json::object! {
				name: "FacePath",
				parent_name: "Head",
				children: ["FaceGrid"],
				points: 4,
				curve_type: "Spline"
			},
		)
		.unwrap();
	let pd_id = pd["uuid"].as_u32().unwrap();
	session
		.exec(
			"deform.brush",
			&json::object! {
				node_uuid: pd_id,
				param: "Head:: Yaw-Pitch",
				kx: 0.0,
				ky: 0.0,
				anchor: "top",
				dx: 4.0,
				dy: 0.0
			},
		)
		.unwrap();

	// 4) param.create + scalar binding CRUD + anim.create + puppet.diagnostics (read-only)
	session
		.exec(
			"param.create",
			&json::object! {
				name: "Custom:: Slide",
				is_vec2: false,
				min: [-1.0, 0.0],
				max: [1.0, 0.0],
				axis_x: [0.0, 1.0]
			},
		)
		.unwrap();
	session
		.exec(
			"param.set_binding_keypoint",
			&json::object! {
				node_name: "Face",
				param: "Custom:: Slide",
				kind: "TransformTX",
				ix: 1,
				iy: 0,
				value: 15.0
			},
		)
		.unwrap();
	let clr = session
		.exec(
			"param.clear_binding_keypoint",
			&json::object! {
				node_name: "Face",
				param: "Custom:: Slide",
				kind: "TransformTX",
				ix: 1,
				iy: 0
			},
		)
		.unwrap();
	assert_eq!(clr["cleared"].as_bool(), Some(true));
	let rem = session
		.exec(
			"param.remove_binding",
			&json::object! { node_name: "Face", param: "Custom:: Slide" },
		)
		.unwrap();
	assert_eq!(rem["removed"].as_u32(), Some(1));

	session
		.exec("anim.create", &json::object! { name: "Wave", length: 90, fps: 30 })
		.unwrap();
	assert!(session.model.puppet.animations.contains_key("Wave"));

	let rev_before = session.revision;
	let diag = session.exec("puppet.diagnostics", &json::object! {}).unwrap();
	assert_eq!(session.revision, rev_before);
	assert!(diag["warnings"].is_array());
	assert!(diag["raw_nodes"].is_array());
}

#[test]
fn mcp_booster_commands_configure_start_status_and_stop() {
	let mut session = Session::new_sample();

	let list = session.exec("booster.list_spout", &json::object! {}).unwrap();
	assert!(list["senders"].is_array());

	let cfg_res = session
		.exec(
			"booster.configure",
			&json::object! {
				enable_vts: false,
				enable_gpu: false,
				spout_in: "",
				spout_out: "RigLensTestOut",
				tick_hz: 120.0,
				wind_turbulence: 0.42
			},
		)
		.unwrap();
	assert_eq!(cfg_res["running"].as_bool(), Some(false));
	assert_eq!(cfg_res["config"]["enable_vts"].as_bool(), Some(false));
	assert!(cfg_res["config"]["spout_in"].is_null());

	let start_res = session.exec("booster.start", &json::object! {}).unwrap();
	assert_eq!(start_res["running"].as_bool(), Some(true));

	let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
	let mut status = session.exec("booster.status", &json::object! {}).unwrap();
	while status["ticks"].as_u64().unwrap_or(0) < 1 && std::time::Instant::now() < deadline {
		std::thread::sleep(std::time::Duration::from_millis(10));
		status = session.exec("booster.status", &json::object! {}).unwrap();
	}
	assert_eq!(status["running"].as_bool(), Some(true));
	assert!(status["ticks"].as_u64().unwrap_or(0) >= 1);

	let stop_res = session.exec("booster.stop", &json::object! {}).unwrap();
	assert_eq!(stop_res["running"].as_bool(), Some(false));
}

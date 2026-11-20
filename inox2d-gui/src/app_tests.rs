use std::path::Path;

use eframe::egui::{pos2, Rect};
use glam::{vec2 as gvec2, vec3 as gvec3};
use inox2d::node::components::RawNodeData;
use inox2d_mcp::audit::audit_rig;

use crate::stage::{find_folded_triangles, pose25d_norm_from_screen, xy_pad_norm_from_pointer, xy_pad_screen_from_norm};
use crate::stage_render::StageOverlay;
use crate::startup::{clean_display_name, find_example_models, format_clean_zsort, load_initial_session};

use super::*;

#[test]
fn sample_studio_session_loads_cleanly() {
	let mut s = load_initial_session("--sample").expect("load sample studio session");
	assert!(s.model.puppet.params.contains_key("Mouth:: Viseme"));
	let rep = audit_rig(&mut s.model);
	assert!(rep["score"].as_u32().unwrap_or(0) >= 80);
}

#[test]
fn studio_app_10pt_audit_deformers_and_real_inx_diagnostics() {
	let mut app = Inox2dStudioApp::new(
		load_initial_session("--sample").unwrap(),
		None,
		None,
		None,
	);
	for key in [
		"containment_leaks",
		"mesh_foldovers",
		"mesh_stretch",
		"blink_issues",
		"symmetry_defects",
		"unmasked_parts",
		"deformer_issues",
		"binding_coverage",
		"draw_order_conflicts",
		"mask_cycles",
	] {
		assert!(app.audit_cache["summary"][key].as_usize().is_some(), "missing 10pt key {key}");
	}

	// Wrap Face in GridDeformer + PathDeformer and verify runtime deform offsets + stage overlays
	app.exec_cmd(
		"node.add_grid_deformer",
		json::object! { name: "HeadGrid", parent_name: "Head", children: ["Face"], cols: 2, rows: 2 },
		"Added GridDeformer",
	);
	let gd_id = app
		.session
		.model
		.puppet
		.nodes
		.iter()
		.find(|n| n.name == "HeadGrid")
		.unwrap()
		.uuid;
	app.exec_cmd(
		"deform.set_vertices",
		json::object! {
			node_uuid: gd_id.0,
			param: "Head:: Yaw-Pitch",
			kx: -1.0,
			ky: 0.0,
			indices: [0],
			dx: 8.0,
			dy: 0.0
		},
		"Deformed grid vertex",
	);
	app.session
		.param_overrides
		.insert("Head:: Yaw-Pitch".to_owned(), gvec2(-1.0, 0.0));
	app.session.model.puppet.init_all();
	app.session.model.puppet.begin_frame();
	if let Some(pctx) = app.session.model.puppet.param_ctx.as_mut() {
		let _ = pctx.set("Head:: Yaw-Pitch", gvec2(-1.0, 0.0));
	}
	app.session.model.puppet.end_frame(0.0);

	let offsets = app.session.model.puppet.node_deform_offsets(gd_id).unwrap();
	assert!((offsets[0].x - 8.0).abs() < 1e-3);

	app.selected_node = Some(gd_id);
	let canvas = Rect::from_min_max(pos2(0.0, 0.0), pos2(512.0, 512.0));
	assert!(matches!(app.selected_node_stage_overlay(canvas), Some(StageOverlay::Grid { .. })));

	// Verify Phase 0 diagnostics surface for opaque RawNodeData nodes
	let opaque_id = app.session.model.puppet.next_node_uuid();
	app.session.model.puppet.add_node(
		app.session.model.puppet.nodes.root_node_id,
		inox2d::node::InoxNode {
			uuid: opaque_id,
			name: "OpaquePlugin".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: Default::default(),
			lock_to_root: false,
		},
	);
	app.session.model.puppet.node_comps.add(
		opaque_id,
		RawNodeData {
			type_name: "AnimatedPart".to_owned(),
			fields: json::object::Object::new(),
		},
	);
	let diag_res = app.session.exec("puppet.diagnostics", &json::object! {}).unwrap();
	assert_eq!(diag_res["raw_nodes"].len(), 1);
	assert_eq!(diag_res["raw_nodes"][0]["type_name"].as_str(), Some("AnimatedPart"));

	// Check real nijigenerate model loads cleanly in studio when present
	if let Ok(dir) = std::env::var("RIGLENS_EXAMPLE_MODELS") {
		let midori = Path::new(&dir).join("Midori.inx");
		if midori.is_file() {
			app.open_model_from_path(&midori);
			assert!(app.session.model.puppet.nodes.iter().count() >= 10);
		}
	}
}

#[test]
fn real_model_framing_dynamic_pads_and_clean_labels() {
	assert_eq!(clean_display_name("Leg:: Right\0\0 "), "Leg:: Right");
	assert_eq!(format_clean_zsort(-0.0), 0.0);
	assert_eq!(format_clean_zsort(-0.02), 0.0);
	assert_eq!(format_clean_zsort(-1.5), -1.5);

	// Verify TransformStore::absolute offset is respected in world-space framing
	let mut app = Inox2dStudioApp::new(
		load_initial_session("--sample").unwrap(),
		None,
		None,
		None,
	);
	let root_id = app.session.model.puppet.nodes.root_node_id;
	let grp_id = app.session.model.puppet.next_node_uuid();
	app.session.model.puppet.add_node(
		root_id,
		inox2d::node::InoxNode {
			uuid: grp_id,
			name: "OffsetGroup\0\0".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: inox2d::math::transform::TransformOffset {
				translation: gvec3(0.0, 600.0, 0.0),
				..Default::default()
			},
			lock_to_root: false,
		},
	);
	let face_id = app
		.session
		.model
		.puppet
		.nodes
		.iter()
		.find(|n| n.name == "Face")
		.map(|n| n.uuid);
	if let Some(fid) = face_id {
		let _ = app
			.session
			.model
			.puppet
			.nodes
			.reparent(fid, grp_id);
		app.session.model.puppet.reset_contexts();
		app.fit_puppet_scale();
		assert!(app.puppet_center.y > 100.0, "expected puppet_center shifted by parent TransformStore, got {:?}", app.puppet_center);
	}

	// Verify example-model discovery, framing, clean title & active 2D pads on Midori/Aka if present
	let examples = find_example_models();
	for (label, path) in examples {
		if label.eq_ignore_ascii_case("Midori") || label.eq_ignore_ascii_case("Aka") {
			app.open_model_from_path(&path);
			assert!(!app.model_display_name().is_empty());
			assert!(!app.model_display_name().contains('\0'));
			assert!(app.puppet_scale > 0.05 && app.puppet_scale < 1.0, "{label} scale={}", app.puppet_scale);
			assert!(
				app.session.model.puppet.params.contains_key(&app.pad_params[0]),
				"{label} missing pad_params[0]={}",
				app.pad_params[0]
			);
			assert!(
				app.session.model.puppet.params.contains_key(&app.pad_params[1]),
				"{label} missing pad_params[1]={}",
				app.pad_params[1]
			);
		}
	}
}

#[test]
fn xy_pad_and_pose25d_y_up_convention() {
	let pad_rect = egui::Rect::from_min_size(egui::pos2(100.0, 200.0), egui::vec2(120.0, 120.0));
	// Dragging to top-right of pad -> tx=1.0, ty=1.0 (max.x, max.y = looking up-right)
	let (tx, ty) = xy_pad_norm_from_pointer(egui::pos2(220.0, 200.0), pad_rect);
	assert!((tx - 1.0).abs() < 1e-5 && (ty - 1.0).abs() < 1e-5);
	// Dragging to bottom-left of pad -> tx=0.0, ty=0.0 (min.x, min.y = looking down-left)
	let (tx_b, ty_b) = xy_pad_norm_from_pointer(egui::pos2(100.0, 320.0), pad_rect);
	assert!(tx_b.abs() < 1e-5 && ty_b.abs() < 1e-5);
	// Puck round-trip: ny=1.0 renders at rect.top(), ny=0.0 renders at rect.bottom()
	let puck_top = xy_pad_screen_from_norm(0.5, 1.0, pad_rect);
	assert!((puck_top.y - pad_rect.top()).abs() < 1e-5);
	let puck_bot = xy_pad_screen_from_norm(0.5, 0.0, pad_rect);
	assert!((puck_bot.y - pad_rect.bottom()).abs() < 1e-5);

	let stage_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
	// Moving pointer above stage center -> positive ny (head/gaze up)
	let (px, py_up) = pose25d_norm_from_screen(egui::pos2(400.0, 30.0), stage_rect);
	assert!(px.abs() < 1e-5 && (py_up - 1.0).abs() < 1e-5);
	// Moving pointer below stage center -> negative ny (head/gaze down)
	let (_, py_down) = pose25d_norm_from_screen(egui::pos2(400.0, 570.0), stage_rect);
	assert!((py_down + 1.0).abs() < 1e-5);

	// Foldover detector catches flipped/inverted triangles relative to majority winding
	let pts = vec![
		pos2(0.0, 0.0),
		pos2(40.0, 0.0),
		pos2(0.0, 40.0),
		pos2(40.0, 40.0),
		pos2(15.0, 10.0), // pushed right across left edge, flipping triangle (2, 4, 0)
	];
	let indices = vec![0, 1, 2, 1, 3, 2, 2, 4, 0];
	let folded = find_folded_triangles(&pts, &indices);
	assert_eq!(folded.len(), 1);
	assert_eq!(folded[0], [2, 4, 0]);
}

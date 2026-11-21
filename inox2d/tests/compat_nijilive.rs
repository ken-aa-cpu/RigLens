//! Regression tests: files written by nijilive-family tools must survive a load/save round trip.

use inox2d::formats::inp::{parse_inp, write_inp};
use inox2d::node::components::BlendMode;

fn node(uuid: u32, name: &str, ty: &str, extra: &str, children: &str) -> String {
	format!(
		r#"{{"uuid":{uuid},"name":"{name}","type":"{ty}","enabled":true,"zsort":0.0,
		"transform":{{"trans":[0,0,0],"rot":[0,0,0],"scale":[1,1]}},"lockToRoot":false{extra},"children":[{children}]}}"#
	)
}

fn part(uuid: u32, name: &str, blend: &str) -> String {
	node(
		uuid,
		name,
		"Part",
		&format!(
			r#","mesh":{{"verts":[-10,-10,10,-10,10,10,-10,10],"uvs":[0,0,1,0,1,1,0,1],"indices":[0,1,2,0,2,3],"origin":[0,0]}},
			"textures":[0,4294967295,4294967295],"blend_mode":"{blend}","tint":[1,1,1],"screenTint":[0,0,0],
			"mask_threshold":0.5,"opacity":1.0,"masks":[]"#
		),
		"",
	)
}

fn build_inp(root_children: &str) -> Vec<u8> {
	let root = node(1, "Root", "Node", "", root_children);
	let json = format!(
		r#"{{"meta":{{"name":"compat","version":"1.0-alpha","rigger":"t","artist":"t","rights":null,"copyright":"","licenseURL":"","contact":"","reference":"","thumbnailId":4294967295,"preservePixels":false}},
		"physics":{{"pixelsPerMeter":1000.0,"gravity":9.8}},"nodes":{root},"param":[],"automation":[],"animations":{{}}}}"#
	);
	let mut out = b"TRNSRTS\0".to_vec();
	out.extend((json.len() as u32).to_be_bytes());
	out.extend(json.as_bytes());
	out.extend(b"TEX_SECT");
	out.extend(0u32.to_be_bytes());
	out
}

fn roundtrip(bytes: &[u8]) -> (inox2d::model::Model, inox2d::model::Model) {
	let a = parse_inp(bytes).expect("first parse");
	let mut buf = Vec::new();
	write_inp(&a, &mut buf).expect("write");
	let b = parse_inp(&buf[..]).expect("second parse");
	(a, b)
}

fn type_and_fields(puppet: &inox2d::puppet::Puppet) -> Vec<(String, String, String)> {
	use inox2d::node::components::RawNodeData;
	puppet
		.nodes
		.iter()
		.filter_map(|n| {
			puppet
				.node_comps
				.get::<RawNodeData>(n.uuid)
				.map(|r| (n.name.clone(), r.type_name.clone(), r.fields.dump()))
		})
		.collect()
}

#[test]
fn unknown_node_types_survive_round_trip() {
	let children = [
		node(4, "Fx", "CustomPluginFx", r#","intensity":0.75"#, &part(5, "InShape", "Normal")),
		node(6, "Cl", "ClothSpring", r#","stiffness":0.4"#, ""),
		node(8, "Tr", "TrailEmitter", r#","samples":16"#, ""),
	]
	.join(",");
	let (a, b) = roundtrip(&build_inp(&children));

	let before = type_and_fields(&a.puppet);
	let after = type_and_fields(&b.puppet);
	assert_eq!(before.len(), 3, "all three unknown plugin nodes captured");
	assert_eq!(before, after, "type names and payload fields must be unchanged");
	assert!(a.puppet.diagnostics.len() >= 3, "loading must report each unsupported node");
	// Children of an unknown node are still loaded.
	assert!(b.puppet.nodes.iter().any(|n| n.name == "InShape"));
}

#[test]
fn p1_remaining_node_types_round_trip_and_render() {
	use inox2d::formats::ir_bridge::{ir_to_model, model_to_ir};
	use inox2d::node::components::{
		AnimatedPart, Composite, Driver, DynamicComposite, MaskNode, RawNodeData, Shapes, TexturedMesh,
	};

	let anim_extra = r#","blend_mode":"Normal","opacity":0.9,"frame_rate":12.0,"loop":false,"textures":[0],"mesh":{"verts":[0,0,10,0,0,10],"uvs":[0,0,1,0,0,1],"indices":[0,1,2],"origin":[0,0]},"sheet":{"rows":2,"cols":2}"#;
	let mask_extra = r#","inverted":false,"mesh":{"verts":[0,0,8,0,0,8],"uvs":[0,0,1,0,0,1],"indices":[0,1,2],"origin":[0,0]}"#;
	let dyn_extra = r#","blend_mode":"Multiply","opacity":0.8,"auto_resized":true"#;
	let shp_extra = r#","closed":true,"vertices":[0,0,10,0,10,10],"shape":"polygon""#;
	let drv_extra = r#","driver_type":"ParamBridge","param_id":7"#;

	let children = [
		node(4, "Shp", "Shapes", shp_extra, &part(5, "InShape", "Normal")),
		node(6, "Msk", "Mask", mask_extra, ""),
		node(8, "Anim", "AnimatedPart", anim_extra, ""),
		node(9, "Dyn", "DynamicComposite", dyn_extra, &part(10, "InDyn", "Normal")),
		node(11, "Drv", "Driver", drv_extra, ""),
	]
	.join(",");
	let (mut a, mut b) = roundtrip(&build_inp(&children));

	assert!(
		!a.puppet.nodes.iter().any(|n| a.puppet.node_comps.get::<RawNodeData>(n.uuid).is_some()),
		"all 5 P1 node types must parse as typed components, not RawNodeData: {:?}",
		a.puppet.diagnostics
	);
	assert!(a.puppet.diagnostics.is_empty(), "unexpected diagnostics: {:?}", a.puppet.diagnostics);

	// Verify IR RigDocument roundtrip preserves all 5 node types + extras cleanly.
	let ir = model_to_ir(&a);
	let mut c = ir_to_model(&ir).expect("ir_to_model");

	for puppet in [&mut a.puppet, &mut b.puppet, &mut c.puppet] {
		let find = |name: &str| puppet.nodes.iter().find(|n| n.name == name).unwrap().uuid;

		let shp = puppet.node_comps.get::<Shapes>(find("Shp")).expect("Shapes");
		assert!(shp.closed);
		assert_eq!(shp.vertices.len(), 3);

		assert!(puppet.node_comps.get::<MaskNode>(find("Msk")).is_some());

		let ap_id = find("Anim");
		let ap = puppet.node_comps.get::<AnimatedPart>(ap_id).expect("AnimatedPart");
		assert!((ap.frame_rate - 12.0).abs() < 1e-4);
		assert!(!ap.loop_anim);
		assert!(puppet.node_comps.get::<TexturedMesh>(ap_id).is_some());

		let dc_id = find("Dyn");
		assert!(puppet.node_comps.get::<Composite>(dc_id).is_some());
		let dc = puppet.node_comps.get::<DynamicComposite>(dc_id).expect("DynamicComposite");
		assert!(dc.auto_resized);

		let drv = puppet.node_comps.get::<Driver>(find("Drv")).expect("Driver");
		assert_eq!(drv.driver_type, "ParamBridge");

		puppet.init_all();
		puppet.begin_frame();
		puppet.end_frame(0.0);
	}
}

#[test]
fn all_blend_modes_round_trip() {
	let parts: Vec<String> = BlendMode::VALUES
		.iter()
		.enumerate()
		.map(|(i, m)| part(10 + i as u32, &format!("p{i}"), m.as_str()))
		.collect();
	let (_, b) = roundtrip(&build_inp(&parts.join(",")));

	use inox2d::node::components::Drawable;
	for (i, expected) in BlendMode::VALUES.iter().enumerate() {
		let node = b.puppet.nodes.iter().find(|n| n.name == format!("p{i}")).unwrap();
		let d = b.puppet.node_comps.get::<Drawable>(node.uuid).unwrap();
		assert_eq!(d.blending.mode, *expected);
	}
}

fn payload_json(bytes: &[u8]) -> json::JsonValue {
	assert_eq!(&bytes[..8], b"TRNSRTS\0");
	let n = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
	json::parse(std::str::from_utf8(&bytes[12..12 + n]).unwrap()).unwrap()
}

fn all_nodes<'a>(n: &'a json::JsonValue, out: &mut Vec<&'a json::JsonValue>) {
	out.push(n);
	for c in n["children"].members() {
		all_nodes(c, out);
	}
}

/// Fields and values this crate does not model must be written back, not dropped.
#[test]
fn unmodelled_fields_survive_round_trip() {
	let part_with_extras = node(
		5,
		"P",
		"Part",
		r#","psdLayerPath":"/Body","emissionStrength":0.7,
		"mesh":{"verts":[0,0,1,0,1,1],"uvs":[0,0,1,0,1,1],"indices":[0,1,2],"origin":[0,0],"grid_axes":[[0,1],[0,1]]},
		"textures":[0],"blend_mode":"Normal","tint":[1,1,1],"screenTint":[0,0,0],"mask_threshold":0.5,"opacity":1.0,"masks":[]"#,
		"",
	);
	let root = node(1, "Root", "Node", "", &part_with_extras);
	let src = format!(
		r#"{{"meta":{{"name":"x","version":"1.0-alpha","rigger":"t","artist":"t","rights":null,"copyright":"","licenseURL":"","contact":"","reference":"","thumbnailId":4294967295,"preservePixels":false}},
		"physics":{{"pixelsPerMeter":1000.0,"gravity":9.8}},"nodes":{root},
		"param":[
		 {{"uuid":9,"name":"B","is_vec2":false,"min":[0,0],"max":[1,0],"defaults":[0,0],"axis_points":[[0,1],[0]],"merge_mode":"Additive","parentUUID":77,"bindings":[
		   {{"node":5,"param_name":"transform.t.x","isSet":[[true],[true]],"interpolate_mode":"Cubic","values":[[0],[1]]}}]}},
		 {{"uuid":3,"name":"A","is_vec2":false,"min":[0,0],"max":[1,0],"defaults":[0,0],"axis_points":[[0,1],[0]],"bindings":[]}}],
		"automation":[],"groups":[{{"groupUUID":77,"name":"G","color":[1,0,0]}}],"animations":{{}}}}"#
	);
	let mut bytes = b"TRNSRTS\0".to_vec();
	bytes.extend((src.len() as u32).to_be_bytes());
	bytes.extend(src.as_bytes());
	bytes.extend(b"TEX_SECT");
	bytes.extend(0u32.to_be_bytes());

	let a = parse_inp(&bytes[..]).expect("parse");
	let mut buf = Vec::new();
	write_inp(&a, &mut buf).expect("write");
	let out = payload_json(&buf);

	let mut nodes = Vec::new();
	all_nodes(&out["nodes"], &mut nodes);
	let p = nodes.iter().find(|n| n["name"] == "P").unwrap();
	assert_eq!(p["psdLayerPath"], "/Body");
	assert!((p["emissionStrength"].as_f32().unwrap() - 0.7).abs() < 1e-6);
	assert!(!p["mesh"]["grid_axes"].is_null(), "mesh.grid_axes must be preserved");

	let params: Vec<&str> = out["param"].members().map(|p| p["name"].as_str().unwrap()).collect();
	assert_eq!(params, ["B", "A"], "parameter order must follow the file, not uuid");
	let b = &out["param"][0];
	assert_eq!(b["parentUUID"], 77);
	assert_eq!(b["merge_mode"], "Additive");
	assert_eq!(b["bindings"][0]["interpolate_mode"], "Cubic", "Cubic bindings must not be dropped");
	assert_eq!(out["groups"][0]["groupUUID"], 77, "parameter groups must be preserved");
}

/// Optional: real-world files. Set `RIGLENS_EXAMPLE_MODELS` to a folder containing
/// `Aka.inx` / `Midori.inx` (https://github.com/Inochi2D/example-models, CC BY 4.0).
/// Skipped when the variable is not set; the models are deliberately not part of this repo.
#[test]
fn real_models_round_trip_without_losing_structure() {
	let Ok(dir) = std::env::var("RIGLENS_EXAMPLE_MODELS") else {
		eprintln!("RIGLENS_EXAMPLE_MODELS not set, skipping");
		return;
	};
	for name in ["Aka.inx", "Midori.inx"] {
		let path = std::path::Path::new(&dir).join(name);
		let Ok(bytes) = std::fs::read(&path) else {
			eprintln!("{path:?} not found, skipping");
			continue;
		};
		let model = parse_inp(&bytes[..]).expect("parse real model");
		let mut buf = Vec::new();
		write_inp(&model, &mut buf).expect("write");

		let (a, b) = (payload_json(&bytes), payload_json(&buf));
		let (mut na, mut nb) = (Vec::new(), Vec::new());
		all_nodes(&a["nodes"], &mut na);
		all_nodes(&b["nodes"], &mut nb);
		assert_eq!(na.len(), nb.len(), "{name}: node count");
		let names = |v: &json::JsonValue| v["param"].members().map(|p| p["name"].to_string()).collect::<Vec<_>>();
		assert_eq!(names(&a), names(&b), "{name}: parameter order");
		for (pa, pb) in a["param"].members().zip(b["param"].members()) {
			assert_eq!(pa["bindings"].len(), pb["bindings"].len(), "{name}: bindings of {}", pa["name"]);
			for (ba, bb) in pa["bindings"].members().zip(pb["bindings"].members()) {
				assert_eq!(ba["interpolate_mode"], bb["interpolate_mode"], "{name}: interpolation");
			}
		}
		let count = |v: &[&json::JsonValue], key: &str| v.iter().filter(|n| !n[key].is_null()).count();
		for key in ["psdLayerPath", "emissionStrength"] {
			assert_eq!(count(&na, key), count(&nb, key), "{name}: {key}");
		}
		assert_eq!(a["groups"].len(), b["groups"].len(), "{name}: groups");
	}
}

#[test]
fn cubic_param_bindings_1d_and_2d_match_catmull_rom() {
	use glam::Vec2;
	use inox2d::math::interp::cubic_cell;
	use inox2d::node::InoxNodeUuid;
	use inox2d::render::TexturedMeshRenderCtx;

	// Build a minimal puppet with one Part (4 verts) and two parameters:
	// - 1D param "P1" with 3 axis points [0.0, 0.5, 1.0] driving TransformTX ([0, 10, 0]) and Deform (v0 += [0,0], [0,10], [0,0])
	// - 2D param "P2" with 3x3 axis points driving TransformTX with non-collinear grid
	let p_node = part(5, "P", "Normal");
	let root = node(1, "Root", "Node", "", &p_node);
	let mk_inp = |mode: &str| {
		let zero_4 = "[[0,0],[0,0],[0,0],[0,0]]";
		let mid_4 = "[[0,10],[0,0],[0,0],[0,0]]";
		let json = format!(
			r#"{{"meta":{{"name":"cubic","version":"1.0-alpha","thumbnailId":4294967295,"preservePixels":false}},
			"physics":{{"pixelsPerMeter":1000.0,"gravity":9.8}},"nodes":{root},
			"param":[
			  {{"uuid":10,"name":"P1","is_vec2":false,"min":[0,0],"max":[1,0],"defaults":[0,0],
			    "axis_points":[[0.0,0.5,1.0],[0.0]],"bindings":[
			      {{"node":5,"param_name":"transform.t.x","isSet":[[true],[true],[true]],"interpolate_mode":"{mode}","values":[[0.0],[10.0],[0.0]]}},
			      {{"node":5,"param_name":"deform","isSet":[[true],[true],[true]],"interpolate_mode":"{mode}","values":[[{zero_4}],[{mid_4}],[{zero_4}]]}}
			    ]}},
			  {{"uuid":11,"name":"P2","is_vec2":true,"min":[0,0],"max":[1,1],"defaults":[0,0],
			    "axis_points":[[0.0,0.5,1.0],[0.0,0.5,1.0]],"bindings":[
			      {{"node":5,"param_name":"transform.t.y","isSet":[[true,true,true],[true,true,true],[true,true,true]],
			        "interpolate_mode":"{mode}",
			        "values":[[0.0,0.0,0.0],[0.0,12.0,0.0],[0.0,0.0,0.0]]}}
			    ]}}
			],
			"automation":[],"groups":[],"animations":{{}}}}"#
		);
		let mut out = b"TRNSRTS\0".to_vec();
		out.extend((json.len() as u32).to_be_bytes());
		out.extend(json.as_bytes());
		out.extend(b"TEX_SECT");
		out.extend(0u32.to_be_bytes());
		out
	};

	let mut m_cubic = parse_inp(&mk_inp("Cubic")[..]).unwrap();
	let mut m_linear = parse_inp(&mk_inp("Linear")[..]).unwrap();
	m_cubic.puppet.init_all();
	m_linear.puppet.init_all();

	for m in [&mut m_cubic, &mut m_linear] {
		m.puppet.begin_frame();
		let pc = m.puppet.param_ctx.as_mut().unwrap();
		// Sample at cell midpoint x = 0.25 (t = 0.5 between 0.0 and 0.5) and y = 0.25.
		pc.set("P1", Vec2::new(0.25, 0.0)).unwrap();
		pc.set("P2", Vec2::new(0.25, 0.25)).unwrap();
		m.puppet.end_frame(0.0);
	}

	let pid = InoxNodeUuid(5);
	let tc = m_cubic.puppet.node_transform(pid).unwrap().relative.translation;
	let tl = m_linear.puppet.node_transform(pid).unwrap().relative.translation;

	// Expected 1D Catmull-Rom on [0, 10, 0] in cell (0, 1) at t = 0.5:
	let grid_1d = [0.0f32, 10.0, 0.0];
	let exp_tx = cubic_cell(|x, _| grid_1d[x], (0, 1), (0, 0), 2, 0, Vec2::new(0.5, 0.0));
	assert!((tc.x - exp_tx).abs() < 1e-5, "1D cubic tx={} expected={}", tc.x, exp_tx);
	assert!((tc.x - tl.x).abs() > 0.5, "1D cubic {} must differ from linear {}", tc.x, tl.x);

	// Expected 2D bicubic on [[0,0,0],[0,12,0],[0,0,0]] in cell (0,1)x(0,1) at (0.5, 0.5):
	let grid_2d = [[0.0f32, 0.0, 0.0], [0.0, 12.0, 0.0], [0.0, 0.0, 0.0]];
	let exp_ty = cubic_cell(|x, y| grid_2d[x][y], (0, 1), (0, 1), 2, 2, Vec2::new(0.5, 0.5));
	assert!((tc.y - exp_ty).abs() < 1e-5, "2D cubic ty={} expected={}", tc.y, exp_ty);
	assert!((tc.y - tl.y).abs() > 0.2, "2D cubic {} must differ from linear {}", tc.y, tl.y);

	// Check 1D Deform on vertex 0:
	let rc_c = m_cubic.puppet.render_ctx.as_ref().unwrap();
	let tm_c = m_cubic.puppet.node_comps.get::<TexturedMeshRenderCtx>(pid).unwrap();
	let def_c = rc_c.vertex_buffers.deforms[tm_c.vert_offset as usize];
	let rc_l = m_linear.puppet.render_ctx.as_ref().unwrap();
	let tm_l = m_linear.puppet.node_comps.get::<TexturedMeshRenderCtx>(pid).unwrap();
	let def_l = rc_l.vertex_buffers.deforms[tm_l.vert_offset as usize];
	assert!((def_c.y - exp_tx).abs() < 1e-5);
	assert!((def_c.y - def_l.y).abs() > 0.5);
}

#[test]
fn real_midori_cubic_vs_linear_evaluation() {
	use glam::Vec2;
	use inox2d::math::interp::InterpolateMode;
	use inox2d::node::components::TransformStore;

	let Ok(dir) = std::env::var("RIGLENS_EXAMPLE_MODELS") else {
		eprintln!("RIGLENS_EXAMPLE_MODELS not set, skipping");
		return;
	};
	let path = std::path::Path::new(&dir).join("Midori.inx");
	let Ok(bytes) = std::fs::read(&path) else {
		eprintln!("{path:?} not found, skipping");
		return;
	};

	let mut m_cubic = parse_inp(&bytes[..]).expect("parse Midori");
	let mut m_linear = parse_inp(&bytes[..]).expect("parse Midori");
	for param in m_linear.puppet.params.values_mut() {
		for b in &mut param.bindings {
			if b.interpolate_mode == InterpolateMode::Cubic {
				b.interpolate_mode = InterpolateMode::Linear;
			}
		}
	}

	m_cubic.puppet.init_all();
	m_linear.puppet.init_all();

	for m in [&mut m_cubic, &mut m_linear] {
		m.puppet.begin_frame();
		let pc = m.puppet.param_ctx.as_mut().unwrap();
		pc.set("Head:: Yaw-Pitch", Vec2::new(0.35, 0.35)).unwrap();
		pc.set("Body:: Yaw-Pitch", Vec2::new(0.35, 0.35)).unwrap();
		m.puppet.end_frame(0.0);
	}

	let deforms_c = &m_cubic.puppet.render_ctx.as_ref().unwrap().vertex_buffers.deforms;
	let deforms_l = &m_linear.puppet.render_ctx.as_ref().unwrap().vertex_buffers.deforms;
	assert!(deforms_c.iter().all(|d| d.is_finite()), "no NaN/Inf in cubic deforms");

	let max_deform_diff = deforms_c
		.iter()
		.zip(deforms_l.iter())
		.map(|(a, b)| (*a - *b).length())
		.fold(0.0f32, f32::max);
	let max_trans_diff = m_cubic
		.puppet
		.nodes
		.iter()
		.filter_map(|n| {
			let a = m_cubic.puppet.node_comps.get::<TransformStore>(n.uuid)?;
			let b = m_linear.puppet.node_comps.get::<TransformStore>(n.uuid)?;
			Some((a.relative.translation - b.relative.translation).length())
		})
		.fold(0.0f32, f32::max);

	assert!(
		max_deform_diff > 1e-3 || max_trans_diff > 1e-3,
		"expected non-zero delta between Cubic and Linear on Midori (deform={max_deform_diff}, trans={max_trans_diff})"
	);
}

fn wrap_inp(json: &str) -> Vec<u8> {
	let mut out = b"TRNSRTS\0".to_vec();
	out.extend((json.len() as u32).to_be_bytes());
	out.extend(json.as_bytes());
	out.extend(b"TEX_SECT");
	out.extend(0u32.to_be_bytes());
	out
}

#[test]
fn grid_deformer_round_trip_and_bilinear_propagation() {
	use glam::{vec2, Vec2};
	use inox2d::node::components::{GridDeformer, GridFormation};
	use inox2d::node::InoxNodeUuid;
	use inox2d::render::TexturedMeshRenderCtx;

	// Part vertices: (-10,-10), (0,0), (10,10), (20,0) — last vertex is outside [-10,10] to verify clamping.
	let child = node(
		5,
		"ChildPart",
		"Part",
		r#","mesh":{"verts":[-10,-10,0,0,10,10,20,0],"uvs":[0,0,0.5,0.5,1,1,1,0.5],"indices":[0,1,2,0,2,3],"origin":[0,0]},
		"textures":[0],"blend_mode":"Normal","tint":[1,1,1],"screenTint":[0,0,0],"mask_threshold":0.5,"opacity":1.0,"masks":[]"#,
		"",
	);
	let grid = node(
		4,
		"Grid",
		"GridDeformer",
		r#","grid_axis_x":[-10.0,10.0],"grid_axis_y":[-10.0,10.0],"formation":"Bilinear","dynamic":false"#,
		&child,
	);
	let root = node(1, "Root", "Node", "", &grid);
	// 2x2 corners row-major: (x=-10,y=-10), (10,-10), (-10,10), (10,10)
	// Deforms at p=1.0: [(0,0), (10,0), (0,20), (10,20)]
	let src = format!(
		r#"{{"meta":{{"name":"grid","version":"1.0-alpha","thumbnailId":4294967295,"preservePixels":false}},
		"physics":{{"pixelsPerMeter":1000.0,"gravity":9.8}},"nodes":{root},
		"param":[{{"uuid":10,"name":"MoveGrid","is_vec2":false,"min":[0,0],"max":[1,0],"defaults":[0,0],
		  "axis_points":[[0.0,1.0],[0.0]],"bindings":[
		    {{"node":4,"param_name":"deform","isSet":[[true],[true]],"interpolate_mode":"Linear",
		      "values":[[[[0,0],[0,0],[0,0],[0,0]]],[[[0,0],[10,0],[0,20],[10,20]]]]}}
		  ]}}],
		"automation":[],"groups":[],"animations":{{}}}}"#
	);

	let (a, mut b) = roundtrip(&wrap_inp(&src));
	assert!(a.puppet.diagnostics.is_empty(), "GridDeformer must load cleanly without warnings: {:?}", a.puppet.diagnostics);
	let g = b.puppet.node_comps.get::<GridDeformer>(InoxNodeUuid(4)).expect("GridDeformer component");
	assert_eq!(g.axis_x, vec![-10.0, 10.0]);
	assert_eq!(g.axis_y, vec![-10.0, 10.0]);
	assert_eq!(g.formation, GridFormation::Bilinear);
	assert!(!g.dynamic);

	b.puppet.init_all();
	b.puppet.begin_frame();
	b.puppet.param_ctx.as_mut().unwrap().set("MoveGrid", Vec2::new(1.0, 0.0)).unwrap();
	b.puppet.end_frame(0.0);

	let rc = b.puppet.render_ctx.as_ref().unwrap();
	let tm = b.puppet.node_comps.get::<TexturedMeshRenderCtx>(InoxNodeUuid(5)).unwrap();
	let off = tm.vert_offset as usize;
	let deforms = &rc.vertex_buffers.deforms[off..off + tm.vert_len];
	let expected = [vec2(0.0, 0.0), vec2(5.0, 10.0), vec2(10.0, 20.0), vec2(10.0, 10.0)];
	for (i, (got, exp)) in deforms.iter().zip(expected).enumerate() {
		assert!((*got - exp).length() < 1e-4, "vert {i}: got={got:?} expected={exp:?}");
	}
}

#[test]
fn path_deformer_round_trip_and_frame_propagation() {
	use glam::{vec2, Vec2};
	use inox2d::math::curve::CurveType;
	use inox2d::node::components::PathDeformer;
	use inox2d::node::InoxNodeUuid;
	use inox2d::render::TexturedMeshRenderCtx;

	// Part vertices: (10, 0) on the spine, (10, 5) offset 5 units along +Y normal, (0, 0) at origin.
	let child = node(
		5,
		"Arm",
		"Part",
		r#","mesh":{"verts":[10,0,10,5,0,0],"uvs":[0,0,1,0,0,1],"indices":[0,1,2],"origin":[0,0]},
		"textures":[0],"blend_mode":"Normal","tint":[1,1,1],"screenTint":[0,0,0],"mask_threshold":0.5,"opacity":1.0,"masks":[]"#,
		"",
	);
	let path = node(
		4,
		"Spine",
		"PathDeformer",
		r#","curve_type":"Spline","vertices":[0,0,10,0,20,0],"dynamic_deformation":false,"physics":{"type":"Pendulum","gravity":9.8}"#,
		&child,
	);
	let alias = node(
		8,
		"LegacyBezier",
		"BezierDeformer",
		r#","vertices":[0,0,5,5,10,0],"dynamic_deformation":true"#,
		"",
	);
	let root = node(1, "Root", "Node", "", &format!("{path},{alias}"));
	// Rotate path 90 deg CCW around origin: [0,0, 10,0, 20,0] -> [0,0, 0,10, 0,20]
	// Deltas: [(0,0), (-10,10), (-20,20)]
	let src = format!(
		r#"{{"meta":{{"name":"path","version":"1.0-alpha","thumbnailId":4294967295,"preservePixels":false}},
		"physics":{{"pixelsPerMeter":1000.0,"gravity":9.8}},"nodes":{root},
		"param":[{{"uuid":10,"name":"Bend","is_vec2":false,"min":[0,0],"max":[1,0],"defaults":[0,0],
		  "axis_points":[[0.0,1.0],[0.0]],"bindings":[
		    {{"node":4,"param_name":"deform","isSet":[[true],[true]],"interpolate_mode":"Linear",
		      "values":[[[[0,0],[0,0],[0,0]]],[[[0,0],[-10,10],[-20,20]]]]}}
		  ]}}],
		"automation":[],"groups":[],"animations":{{}}}}"#
	);

	let (a, mut b) = roundtrip(&wrap_inp(&src));
	assert_eq!(a.puppet.diagnostics.len(), 1, "expected 1 diagnostic for attached path physics: {:?}", a.puppet.diagnostics);
	assert!(a.puppet.diagnostics[0].contains("physics"));

	// Verify BezierDeformer alias decoded as PathDeformer with CurveType::Bezier.
	let bez = b.puppet.node_comps.get::<PathDeformer>(InoxNodeUuid(8)).expect("BezierDeformer -> PathDeformer");
	assert_eq!(bez.curve_type, CurveType::Bezier);
	assert!(bez.dynamic);

	// Verify round-tripped PathDeformer fields and preserved physics ExtraFields.
	let mut buf = Vec::new();
	write_inp(&b, &mut buf).unwrap();
	let out_json = payload_json(&buf);
	let mut nodes = Vec::new();
	all_nodes(&out_json["nodes"], &mut nodes);
	let pj = nodes.iter().find(|n| n["name"] == "Spine").unwrap();
	assert_eq!(pj["curve_type"], "Spline");
	assert_eq!(pj["physics"]["type"], "Pendulum");

	b.puppet.init_all();
	b.puppet.begin_frame();
	b.puppet.param_ctx.as_mut().unwrap().set("Bend", Vec2::new(1.0, 0.0)).unwrap();
	b.puppet.end_frame(0.0);

	let rc = b.puppet.render_ctx.as_ref().unwrap();
	let tm = b.puppet.node_comps.get::<TexturedMeshRenderCtx>(InoxNodeUuid(5)).unwrap();
	let off = tm.vert_offset as usize;
	let deforms = &rc.vertex_buffers.deforms[off..off + tm.vert_len];
	// (10,0) -> (0,10): delta (-10,10); (10,5) -> (-5,10): delta (-15,5); (0,0) -> (0,0): delta (0,0)
	assert!((deforms[0] - vec2(-10.0, 10.0)).length() < 1e-2, "v0 got {:?}", deforms[0]);
	assert!((deforms[1] - vec2(-15.0, 5.0)).length() < 1e-2, "v1 got {:?}", deforms[1]);
	assert!(deforms[2].length() < 1e-2, "v2 got {:?}", deforms[2]);
}

#[test]
fn nested_path_to_grid_does_not_double_deform() {
	use glam::{vec2, Vec2};
	use inox2d::node::InoxNodeUuid;
	use inox2d::render::TexturedMeshRenderCtx;

	let leaf = node(
		6,
		"Leaf",
		"Part",
		r#","mesh":{"verts":[0,0,5,0,5,5],"uvs":[0,0,1,0,1,1],"indices":[0,1,2],"origin":[0,0]},
		"textures":[0],"blend_mode":"Normal","tint":[1,1,1],"screenTint":[0,0,0],"mask_threshold":0.5,"opacity":1.0,"masks":[]"#,
		"",
	);
	let grid = node(
		5,
		"MidGrid",
		"GridDeformer",
		r#","grid_axis_x":[-10.0,10.0],"grid_axis_y":[-10.0,10.0],"formation":"Bilinear","dynamic":false"#,
		&leaf,
	);
	let path = node(
		4,
		"OuterPath",
		"PathDeformer",
		r#","curve_type":"Spline","vertices":[0,0,10,0],"dynamic_deformation":false"#,
		&grid,
	);
	let root = node(1, "Root", "Node", "", &path);
	let src = format!(
		r#"{{"meta":{{"name":"nested","version":"1.0-alpha","thumbnailId":4294967295,"preservePixels":false}},
		"physics":{{"pixelsPerMeter":1000.0,"gravity":9.8}},"nodes":{root},
		"param":[{{"uuid":10,"name":"Shift","is_vec2":false,"min":[0,0],"max":[1,0],"defaults":[0,0],
		  "axis_points":[[0.0,1.0],[0.0]],"bindings":[
		    {{"node":4,"param_name":"deform","isSet":[[true],[true]],"interpolate_mode":"Linear",
		      "values":[[[[0,0],[0,0]]],[[[10,0],[10,0]]]]}}
		  ]}}],
		"automation":[],"groups":[],"animations":{{}}}}"#
	);

	let mut model = parse_inp(&wrap_inp(&src)[..]).unwrap();
	model.puppet.init_all();
	model.puppet.begin_frame();
	model.puppet.param_ctx.as_mut().unwrap().set("Shift", Vec2::new(1.0, 0.0)).unwrap();
	model.puppet.end_frame(0.0);

	let rc = model.puppet.render_ctx.as_ref().unwrap();
	let tm = model.puppet.node_comps.get::<TexturedMeshRenderCtx>(InoxNodeUuid(6)).unwrap();
	let off = tm.vert_offset as usize;
	for (i, d) in rc.vertex_buffers.deforms[off..off + tm.vert_len].iter().enumerate() {
		assert!((*d - vec2(10.0, 0.0)).length() < 1e-4, "vert {i}: got {d:?} (must be (10,0), not doubled)");
	}
}

#[test]
fn inp_format_passes_conformance_kit() {
	use inox2d::formats::ir_bridge::InpFormat;
	riglens_format::conformance::assert_roundtrip_conformance(&InpFormat, &InpFormat);
}

#[test]
fn real_models_survive_neutral_ir_json_round_trip() {
	use inox2d::formats::ir_bridge::{ir_to_model, model_to_ir};
	use riglens_format::{ExportOptions, Exporter, ImportOptions, Importer, RigJsonFormat};

	let Ok(dir) = std::env::var("RIGLENS_EXAMPLE_MODELS") else {
		eprintln!("RIGLENS_EXAMPLE_MODELS not set, skipping");
		return;
	};
	for name in ["Aka.inx", "Midori.inx"] {
		let path = std::path::Path::new(&dir).join(name);
		let Ok(bytes) = std::fs::read(&path) else {
			eprintln!("{path:?} not found, skipping");
			continue;
		};
		let model = parse_inp(&bytes[..]).expect("parse real model");
		let ir = model_to_ir(&model);
		ir.validate().expect("real model IR must validate");

		let (json_bytes, _) = RigJsonFormat
			.export_bytes(&ir, &ExportOptions::default())
			.expect("export to rig-json");
		let reimported = RigJsonFormat
			.import_bytes(&json_bytes, &ImportOptions::default())
			.expect("import from rig-json");
		let round_model = ir_to_model(&reimported.document).expect("convert back to Model");
		let mut buf = Vec::new();
		write_inp(&round_model, &mut buf).expect("write .inp");

		assert_eq!(model.textures.len(), round_model.textures.len(), "{name}: texture count");
		let (a, b) = (payload_json(&bytes), payload_json(&buf));
		let (mut na, mut nb) = (Vec::new(), Vec::new());
		all_nodes(&a["nodes"], &mut na);
		all_nodes(&b["nodes"], &mut nb);
		assert_eq!(na.len(), nb.len(), "{name}: node count");

		let names = |v: &json::JsonValue| v["param"].members().map(|p| p["name"].to_string()).collect::<Vec<_>>();
		assert_eq!(names(&a), names(&b), "{name}: parameter order");
		for (pa, pb) in a["param"].members().zip(b["param"].members()) {
			assert_eq!(pa["bindings"].len(), pb["bindings"].len(), "{name}: bindings of {}", pa["name"]);
		}
		let count = |v: &[&json::JsonValue], key: &str| v.iter().filter(|n| !n[key].is_null()).count();
		for key in ["psdLayerPath", "emissionStrength"] {
			assert_eq!(count(&na, key), count(&nb, key), "{name}: {key}");
		}
		assert_eq!(a["groups"].len(), b["groups"].len(), "{name}: groups");
	}
}

#[test]
fn dynamic_composite_collected_in_render_ctx() {
	let child = part(3, "LayerA", "Normal");
	let dyn_comp = node(
		2,
		"DynComp",
		"DynamicComposite",
		r#","blend_mode":"Normal","opacity":0.9,"tint":[1,1,1],"screenTint":[0,0,0],"mask_threshold":0.5,"masks":[],"auto_resized":true"#,
		&child,
	);
	let root = node(1, "Root", "Node", "", &dyn_comp);
	let src = format!(
		r#"{{"meta":{{"name":"DynTest","version":"1.0-alpha","preservePixels":false}},"physics":{{"pixelsPerMeter":1000,"gravity":9.8}},"nodes":{root},"param":[]}}"#
	);
	let mut model = parse_inp(&wrap_inp(&src)[..]).expect("parse DynamicComposite");
	model.puppet.init_all();
	model.puppet.begin_frame();
	model.puppet.end_frame(0.0);

	let rc = model.puppet.render_ctx.as_ref().expect("RenderCtx initialized");
	assert!(
		rc.root_drawables_zsorted.contains(&inox2d::node::InoxNodeUuid(2)),
		"DynamicComposite must appear in root_drawables_zsorted"
	);
	assert!(
		!rc.root_drawables_zsorted.contains(&inox2d::node::InoxNodeUuid(3)),
		"DynamicComposite child should be composited offscreen under parent"
	);
	let comp_ctx = model
		.puppet
		.node_comps
		.get::<inox2d::render::CompositeRenderCtx>(inox2d::node::InoxNodeUuid(2))
		.expect("CompositeRenderCtx attached");
	assert_eq!(comp_ctx.zsorted_children_list, vec![inox2d::node::InoxNodeUuid(3)]);
}

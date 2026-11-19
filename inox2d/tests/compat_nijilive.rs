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
		node(4, "Grid", "GridDeformer", r#","dynamic_deformation":true,"vertices":[0,0,1,0,0,1,1,1]"#, &part(5, "InGrid", "Normal")),
		node(6, "Path", "PathDeformer", r#","vertices":[0,0,1,1],"curveType":1"#, ""),
		node(8, "Anim", "AnimatedPart", r#","sheet":{"rows":2,"cols":2}"#, ""),
		node(9, "Dyn", "DynamicComposite", r#","opacity":1.0"#, ""),
		node(11, "Drv", "Driver", r#","param_id":1"#, ""),
	]
	.join(",");
	let (a, b) = roundtrip(&build_inp(&children));

	let before = type_and_fields(&a.puppet);
	let after = type_and_fields(&b.puppet);
	assert_eq!(before.len(), 5, "all five unknown nodes captured");
	assert_eq!(before, after, "type names and payload fields must be unchanged");
	assert!(a.puppet.diagnostics.len() >= 5, "loading must report each unsupported node");
	// Children of an unknown node are still loaded.
	assert!(b.puppet.nodes.iter().any(|n| n.name == "InGrid"));
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

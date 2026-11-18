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

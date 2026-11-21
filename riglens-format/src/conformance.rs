use std::collections::BTreeMap;

use serde_json::json;

use crate::ir::{
	BindingGrid, BindingTarget, BlendData, CompositeData, GridDeformerData, MaskData, MaskEntry, MeshData,
	MeshGroupData, NodeKind, PartData, PathDeformerData, PhysicsWorld, RigAnimation, RigAnimationLane, RigBinding,
	RigDocument, RigKeyframe, RigMeta, RigNode, RigParam, RigTransform, SimplePhysicsData, TextureBlob,
	TextureEncoding, TextureSlots, VendorExtension, IR_SCHEMA_VERSION,
};
use crate::plugin::{export_checked, ExportOptions, Exporter, FormatError, ImportOptions, Importer};

const TINY_PNG: &[u8] = &[
	0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
	0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49,
	0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xdf, 0xe0, 0xf0, 0x1f, 0x00, 0x07, 0x00, 0x02, 0xbf, 0x2b, 0xd7, 0xc7,
	0xe2, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn quad_mesh() -> MeshData {
	MeshData {
		vertices: vec![[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0]],
		uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
		indices: vec![0, 1, 2, 0, 2, 3],
		origin: [0.0, 0.0],
		extras: BTreeMap::new(),
	}
}

fn minimal_empty_rig() -> RigDocument {
	RigDocument {
		schema_version: IR_SCHEMA_VERSION,
		meta: RigMeta {
			name: Some("Minimal".to_owned()),
			..RigMeta::default()
		},
		physics_world: PhysicsWorld::default(),
		nodes: vec![RigNode {
			id: 1,
			parent: None,
			name: "Root".to_owned(),
			enabled: true,
			zsort: 0.0,
			transform: RigTransform::default(),
			lock_to_root: false,
			kind: NodeKind::Group,
			extras: BTreeMap::new(),
		}],
		params: Vec::new(),
		pose_groups: Vec::new(),
		animations: BTreeMap::new(),
		textures: Vec::new(),
		vendors: Vec::new(),
		top_extras: BTreeMap::new(),
		diagnostics: Vec::new(),
	}
}

/// Build a canonical sample [`RigDocument`] exercising all neutral IR node kinds,
/// parameters (Linear + Cubic), animations, textures, vendor extensions and extras.
pub fn sample_conformance_rig() -> RigDocument {
	let mut part_extras = BTreeMap::new();
	part_extras.insert("psdLayerPath".to_owned(), json!("/Body/OverlayPart"));

	let mut opaque_fields = BTreeMap::new();
	opaque_fields.insert("flag".to_owned(), json!(true));

	let mut param_extras = BTreeMap::new();
	param_extras.insert("parentUUID".to_owned(), json!(77));

	let mut top_extras = BTreeMap::new();
	top_extras.insert(
		"groups".to_owned(),
		json!([{"groupUUID": 77, "name": "DeformGroup", "color": [0.2, 0.6, 1.0]}]),
	);

	let mut animations = BTreeMap::new();
	animations.insert(
		"Idle".to_owned(),
		RigAnimation {
			timestep: 1.0 / 30.0,
			additive: false,
			length: 60,
			lead_in: 0,
			lead_out: 0,
			lanes: vec![RigAnimationLane {
				interpolation: "Cubic".to_owned(),
				param_id: 10,
				target: 0,
				merge_mode: "Weighted".to_owned(),
				keyframes: vec![
					RigKeyframe {
						frame: 0,
						value: 0.0,
						tension: 0.5,
					},
					RigKeyframe {
						frame: 30,
						value: 1.0,
						tension: 0.5,
					},
					RigKeyframe {
						frame: 60,
						value: 0.0,
						tension: 0.5,
					},
				],
			}],
		},
	);

	RigDocument {
		schema_version: IR_SCHEMA_VERSION,
		meta: RigMeta {
			name: Some("ConformanceSample".to_owned()),
			version: "1.0-alpha".to_owned(),
			rigger: Some("RigLens".to_owned()),
			artist: Some("RigLens".to_owned()),
			..RigMeta::default()
		},
		physics_world: PhysicsWorld::default(),
		nodes: vec![
			RigNode {
				id: 1,
				parent: None,
				name: "Root".to_owned(),
				enabled: true,
				zsort: 0.0,
				transform: RigTransform::default(),
				lock_to_root: false,
				kind: NodeKind::Group,
				extras: BTreeMap::new(),
			},
			RigNode {
				id: 2,
				parent: Some(1),
				name: "OverlayPart".to_owned(),
				enabled: true,
				zsort: 0.0,
				transform: RigTransform::default(),
				lock_to_root: false,
				kind: NodeKind::Part(PartData {
					mesh: quad_mesh(),
					textures: TextureSlots {
						albedo: 0,
						emissive: None,
						bumpmap: None,
					},
					blending: BlendData {
						mode: "Overlay".to_owned(),
						tint: [1.0, 1.0, 1.0],
						screen_tint: [0.0, 0.0, 0.0],
						opacity: 0.9,
					},
					masks: Some(MaskData {
						threshold: 0.5,
						masks: vec![MaskEntry {
							source_node: 3,
							mode: "Mask".to_owned(),
						}],
					}),
				}),
				extras: part_extras,
			},
			RigNode {
				id: 3,
				parent: Some(1),
				name: "MaskSource".to_owned(),
				enabled: true,
				zsort: 1.0,
				transform: RigTransform::default(),
				lock_to_root: false,
				kind: NodeKind::Part(PartData {
					mesh: quad_mesh(),
					textures: TextureSlots {
						albedo: 0,
						emissive: None,
						bumpmap: None,
					},
					blending: BlendData::default(),
					masks: None,
				}),
				extras: BTreeMap::new(),
			},
			RigNode {
				id: 4,
				parent: Some(1),
				name: "Comp".to_owned(),
				enabled: true,
				zsort: 2.0,
				transform: RigTransform::default(),
				lock_to_root: false,
				kind: NodeKind::Composite(CompositeData {
					blending: BlendData::default(),
					masks: None,
				}),
				extras: BTreeMap::new(),
			},
			RigNode {
				id: 5,
				parent: Some(4),
				name: "CompChild".to_owned(),
				enabled: true,
				zsort: 0.0,
				transform: RigTransform::default(),
				lock_to_root: false,
				kind: NodeKind::Part(PartData {
					mesh: quad_mesh(),
					textures: TextureSlots {
						albedo: 0,
						emissive: None,
						bumpmap: None,
					},
					blending: BlendData::default(),
					masks: None,
				}),
				extras: BTreeMap::new(),
			},
			RigNode {
				id: 6,
				parent: Some(1),
				name: "Cage".to_owned(),
				enabled: true,
				zsort: 3.0,
				transform: RigTransform::default(),
				lock_to_root: false,
				kind: NodeKind::MeshGroup(MeshGroupData {
					mesh: quad_mesh(),
					dynamic: false,
					translate_children: true,
				}),
				extras: BTreeMap::new(),
			},
			RigNode {
				id: 7,
				parent: Some(1),
				name: "Lattice".to_owned(),
				enabled: true,
				zsort: 4.0,
				transform: RigTransform::default(),
				lock_to_root: false,
				kind: NodeKind::GridDeformer(GridDeformerData {
					axis_x: vec![-10.0, 10.0],
					axis_y: vec![-10.0, 10.0],
					formation: "Bilinear".to_owned(),
					dynamic: false,
					translate_children: true,
				}),
				extras: BTreeMap::new(),
			},
			RigNode {
				id: 8,
				parent: Some(1),
				name: "Ribbon".to_owned(),
				enabled: true,
				zsort: 5.0,
				transform: RigTransform::default(),
				lock_to_root: false,
				kind: NodeKind::PathDeformer(PathDeformerData {
					curve_type: "Spline".to_owned(),
					vertices: vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]],
					dynamic: false,
					physics_only: false,
				}),
				extras: BTreeMap::new(),
			},
			RigNode {
				id: 9,
				parent: Some(1),
				name: "HairPhys".to_owned(),
				enabled: true,
				zsort: 6.0,
				transform: RigTransform::default(),
				lock_to_root: false,
				kind: NodeKind::SimplePhysics(SimplePhysicsData {
					param_id: 10,
					model_type: "Pendulum".to_owned(),
					map_mode: "AngleLength".to_owned(),
					gravity: 1.0,
					length: 50.0,
					frequency: 2.0,
					angle_damping: 0.5,
					length_damping: 0.5,
					output_scale: [1.0, 1.0],
					local_only: false,
				}),
				extras: BTreeMap::new(),
			},
			RigNode {
				id: 10,
				parent: Some(1),
				name: "ExtPlug".to_owned(),
				enabled: true,
				zsort: 7.0,
				transform: RigTransform::default(),
				lock_to_root: false,
				kind: NodeKind::Opaque {
					type_name: "CustomPlugNode".to_owned(),
					fields: opaque_fields,
				},
				extras: BTreeMap::new(),
			},
		],
		params: vec![
			RigParam {
				id: 10,
				name: "HeadX".to_owned(),
				is_vec2: false,
				min: [-1.0, 0.0],
				max: [1.0, 0.0],
				defaults: [0.0, 0.0],
				axis_x: vec![0.0, 1.0],
				axis_y: vec![0.0],
				additive: false,
				bindings: vec![RigBinding {
					node_id: 2,
					target: BindingTarget::TransformTX,
					interpolate_mode: "Linear".to_owned(),
					is_set: vec![vec![true], vec![true]],
					values: BindingGrid::Scalar(vec![vec![-5.0], vec![5.0]]),
				}],
				extras: BTreeMap::new(),
			},
			RigParam {
				id: 11,
				name: "Warp".to_owned(),
				is_vec2: true,
				min: [0.0, 0.0],
				max: [1.0, 1.0],
				defaults: [0.0, 0.0],
				axis_x: vec![0.0, 1.0],
				axis_y: vec![0.0, 1.0],
				additive: true,
				bindings: vec![RigBinding {
					node_id: 7,
					target: BindingTarget::Deform,
					interpolate_mode: "Cubic".to_owned(),
					is_set: vec![vec![true, true], vec![true, true]],
					values: BindingGrid::Deform(vec![
						vec![vec![[0.0, 0.0]; 4], vec![[0.0, 2.0]; 4]],
						vec![vec![[2.0, 0.0]; 4], vec![[2.0, 2.0]; 4]],
					]),
				}],
				extras: param_extras,
			},
		],
		pose_groups: vec![crate::ir::PoseGroup {
			id: "Arms".to_owned(),
			parts: vec![2],
			fade_seconds: 0.2,
		}],
		animations,
		textures: vec![TextureBlob {
			format: TextureEncoding::Png,
			bytes: TINY_PNG.to_vec(),
		}],
		vendors: vec![VendorExtension {
			name: "example.vendor".to_owned(),
			payload: json!({"v": 1}),
		}],
		top_extras,
		diagnostics: Vec::new(),
	}
}

/// Run standard conformance assertions on an [`Exporter`] plugin.
pub fn assert_exporter_conformance(exporter: &dyn Exporter) {
	let sample = sample_conformance_rig();
	sample.validate().expect("sample conformance rig must be valid");

	let mut buf = Vec::new();
	let report = export_checked(exporter, &sample, &mut buf, &ExportOptions::default())
		.expect("non-strict conformance export must succeed");
	assert!(report.bytes_written > 0, "exporter reported 0 bytes written");
	assert_eq!(buf.len(), report.bytes_written, "bytes_written mismatch");

	let caps = exporter.info();
	let all_supported = caps.supports_grid_deformer
		&& caps.supports_path_deformer
		&& caps.supports_cubic_interpolation
		&& caps.supports_animations
		&& caps.supports_opaque_passthrough
		&& caps.supports_textures
		&& caps.supports_vendor_data;
	if !all_supported {
		let mut strict_buf = Vec::new();
		let strict_opts = ExportOptions {
			strict: true,
			..ExportOptions::default()
		};
		let err = export_checked(exporter, &sample, &mut strict_buf, &strict_opts)
			.expect_err("strict mode must reject when capabilities are missing");
		assert!(matches!(err, FormatError::CapabilityRejected(_)), "unexpected error: {err:?}");
	}

	let minimal = minimal_empty_rig();
	let mut min_buf = Vec::new();
	export_checked(exporter, &minimal, &mut min_buf, &ExportOptions::default())
		.expect("exporter must handle minimal 1-node rig");
}

/// Run full export + re-import round-trip conformance assertions, gated on
/// the capabilities declared by `exporter`.
pub fn assert_roundtrip_conformance(importer: &dyn Importer, exporter: &dyn Exporter) {
	assert_exporter_conformance(exporter);

	let sample = sample_conformance_rig();
	let (bytes, _) = exporter
		.export_bytes(&sample, &ExportOptions::default())
		.expect("export sample");
	let rep = importer
		.import_bytes(&bytes, &ImportOptions::default())
		.expect("re-import exported sample");
	let doc = &rep.document;
	doc.validate().expect("re-imported document must validate");

	let root_count = doc.nodes.iter().filter(|n| n.parent.is_none()).count();
	assert_eq!(root_count, 1, "re-imported rig must have exactly one root");

	let overlay = doc
		.nodes
		.iter()
		.find(|n| n.name == "OverlayPart")
		.expect("OverlayPart must survive round-trip");
	match &overlay.kind {
		NodeKind::Part(p) => {
			assert_eq!(p.mesh.vertices.len(), 4);
			assert_eq!(p.blending.mode, "Overlay");
		}
		other => panic!("expected OverlayPart to be Part, got {other:?}"),
	}

	let caps = exporter.info();
	if caps.supports_grid_deformer {
		let grid = doc
			.nodes
			.iter()
			.find(|n| n.name == "Lattice")
			.expect("GridDeformer must survive");
		match &grid.kind {
			NodeKind::GridDeformer(g) => {
				assert_eq!(g.axis_x, vec![-10.0, 10.0]);
				assert_eq!(g.axis_y, vec![-10.0, 10.0]);
			}
			other => panic!("expected Lattice to be GridDeformer, got {other:?}"),
		}
	}
	if caps.supports_path_deformer {
		let path = doc
			.nodes
			.iter()
			.find(|n| n.name == "Ribbon")
			.expect("PathDeformer must survive");
		match &path.kind {
			NodeKind::PathDeformer(p) => {
				assert_eq!(p.curve_type, "Spline");
				assert_eq!(p.vertices.len(), 3);
			}
			other => panic!("expected Ribbon to be PathDeformer, got {other:?}"),
		}
	}
	if caps.supports_cubic_interpolation {
		let warp = doc
			.params
			.iter()
			.find(|p| p.name == "Warp")
			.expect("Warp param must survive");
		assert_eq!(warp.bindings[0].interpolate_mode, "Cubic");
	}
	if caps.supports_animations {
		let idle = doc.animations.get("Idle").expect("Idle animation must survive");
		assert_eq!(idle.lanes.len(), 1);
		assert_eq!(idle.lanes[0].interpolation, "Cubic");
	}
	if caps.supports_opaque_passthrough {
		let ext = doc
			.nodes
			.iter()
			.find(|n| n.name == "ExtPlug")
			.expect("Opaque node must survive");
		match &ext.kind {
			NodeKind::Opaque { type_name, .. } => assert_eq!(type_name, "CustomPlugNode"),
			other => panic!("expected ExtPlug to be Opaque, got {other:?}"),
		}
	}
	if caps.supports_textures {
		assert_eq!(doc.textures.len(), 1);
		assert_eq!(doc.textures[0].bytes, TINY_PNG);
	}
	if caps.supports_vendor_data {
		assert!(doc.vendors.iter().any(|v| v.name == "example.vendor"));
	}
}

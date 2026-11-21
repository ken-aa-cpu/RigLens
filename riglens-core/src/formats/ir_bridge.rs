use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::sync::Arc;

use glam::{vec2, vec3, Vec2};
use image::ImageFormat;
use riglens_format::{
	AnimatedPartData, BindingGrid, BindingTarget, BlendData, CompositeData, Diagnostic, DriverData,
	DynamicCompositeData, ExportOptions, ExportReport, Exporter, FormatCapabilities, FormatError, GridDeformerData,
	ImportOptions, ImportReport, Importer, MaskData, MaskEntry, MaskNodeData, MeshData, MeshGroupData, NodeKind,
	PartData, PathDeformerData, PhysicsWorld, PoseGroup as IrPoseGroup, RigAnimation, RigAnimationLane, RigBinding,
	RigDocument, RigKeyframe, RigMeta, RigNode, RigParam, RigTransform, RigUsageRights, ShapesData, SimplePhysicsData,
	TextureBlob, TextureEncoding, TextureSlots, VendorExtension, IR_SCHEMA_VERSION,
};

use crate::animation::{AnimationLane, Keyframe, PuppetAnimation};
use crate::formats::inp::{parse_inp, write_inp};
use crate::math::curve::CurveType;
use crate::math::interp::InterpolateMode;
use crate::math::matrix::Matrix2d;
use crate::math::transform::TransformOffset;
use crate::model::{Model, ModelTexture, VendorData};
use crate::node::components::{
	AnimatedPart, BlendMode, Blending, Composite, Drawable, Driver, DynamicComposite, ExtraFields, GridDeformer,
	GridFormation, Mask, MaskMode, MaskNode, Masks, Mesh, MeshExtraFields, MeshGroup, PathDeformer, PhysicsModel,
	PhysicsParamMapMode, PhysicsProps, RawNodeData, Shapes, SimplePhysics, TexturedMesh,
};
use crate::node::{InoxNode, InoxNodeUuid};
use crate::params::{AxisPoints, Binding, BindingValues, Param, ParamUuid};
use crate::physics::PuppetPhysics;
use crate::puppet::meta::{
	PuppetAllowedModification, PuppetAllowedRedistribution, PuppetAllowedUsers, PuppetMeta, PuppetUsageRights,
};
use crate::puppet::Puppet;
use crate::texture::TextureId;

pub fn json_to_serde(v: &json::JsonValue) -> serde_json::Value {
	match v {
		json::JsonValue::Null => serde_json::Value::Null,
		json::JsonValue::Short(s) => serde_json::Value::String(s.as_str().to_owned()),
		json::JsonValue::String(s) => serde_json::Value::String(s.clone()),
		json::JsonValue::Number(_) => {
			if let Some(i) = v.as_i64() {
				serde_json::Value::from(i)
			} else if let Some(u) = v.as_u64() {
				serde_json::Value::from(u)
			} else {
				v.as_f64()
					.and_then(serde_json::Number::from_f64)
					.map(serde_json::Value::Number)
					.unwrap_or(serde_json::Value::Null)
			}
		}
		json::JsonValue::Boolean(b) => serde_json::Value::Bool(*b),
		json::JsonValue::Array(arr) => serde_json::Value::Array(arr.iter().map(json_to_serde).collect()),
		json::JsonValue::Object(obj) => {
			let map: serde_json::Map<String, serde_json::Value> =
				obj.iter().map(|(k, val)| (k.to_owned(), json_to_serde(val))).collect();
			serde_json::Value::Object(map)
		}
	}
}

pub fn serde_to_json(v: &serde_json::Value) -> json::JsonValue {
	match v {
		serde_json::Value::Null => json::JsonValue::Null,
		serde_json::Value::Bool(b) => json::JsonValue::Boolean(*b),
		serde_json::Value::Number(n) => {
			if let Some(i) = n.as_i64() {
				json::JsonValue::from(i)
			} else if let Some(u) = n.as_u64() {
				json::JsonValue::from(u)
			} else {
				json::JsonValue::from(n.as_f64().unwrap_or(0.0))
			}
		}
		serde_json::Value::String(s) => json::JsonValue::String(s.clone()),
		serde_json::Value::Array(arr) => json::JsonValue::Array(arr.iter().map(serde_to_json).collect()),
		serde_json::Value::Object(map) => {
			let mut obj = json::object::Object::new();
			for (k, val) in map {
				obj.insert(k, serde_to_json(val));
			}
			json::JsonValue::Object(obj)
		}
	}
}

fn json_obj_to_map(obj: &json::object::Object) -> BTreeMap<String, serde_json::Value> {
	obj.iter().map(|(k, v)| (k.to_owned(), json_to_serde(v))).collect()
}

fn map_to_json_obj(map: &BTreeMap<String, serde_json::Value>) -> json::object::Object {
	let mut obj = json::object::Object::new();
	for (k, v) in map {
		obj.insert(k, serde_to_json(v));
	}
	obj
}

fn drawable_to_ir(d: &Drawable) -> (BlendData, Option<MaskData>) {
	let blending = BlendData {
		mode: d.blending.mode.as_str().to_owned(),
		tint: d.blending.tint.to_array(),
		screen_tint: d.blending.screen_tint.to_array(),
		opacity: d.blending.opacity,
	};
	let masks = d.masks.as_ref().map(|ms| MaskData {
		threshold: ms.threshold,
		masks: ms
			.masks
			.iter()
			.map(|m| MaskEntry {
				source_node: m.source.0,
				mode: match m.mode {
					MaskMode::Mask => "Mask".to_owned(),
					MaskMode::Dodge => "DodgeMask".to_owned(),
				},
			})
			.collect(),
	});
	(blending, masks)
}

fn mesh_to_ir(m: &Mesh, extras: Option<&MeshExtraFields>) -> MeshData {
	MeshData {
		vertices: m.vertices.iter().map(|v| [v.x, v.y]).collect(),
		uvs: m.uvs.iter().map(|uv| [uv.x, uv.y]).collect(),
		indices: m.indices.clone(),
		origin: [m.origin.x, m.origin.y],
		extras: extras.map(|e| json_obj_to_map(&e.0)).unwrap_or_default(),
	}
}

fn param_to_ir(p: &Param, extras: Option<&json::object::Object>) -> RigParam {
	let bindings = p
		.bindings
		.iter()
		.map(|b| {
			let (target, values) = match &b.values {
				BindingValues::ZSort(m) => (BindingTarget::ZSort, BindingGrid::Scalar(m.to_vec_vecs())),
				BindingValues::TransformTX(m) => (BindingTarget::TransformTX, BindingGrid::Scalar(m.to_vec_vecs())),
				BindingValues::TransformTY(m) => (BindingTarget::TransformTY, BindingGrid::Scalar(m.to_vec_vecs())),
				BindingValues::TransformSX(m) => (BindingTarget::TransformSX, BindingGrid::Scalar(m.to_vec_vecs())),
				BindingValues::TransformSY(m) => (BindingTarget::TransformSY, BindingGrid::Scalar(m.to_vec_vecs())),
				BindingValues::TransformRX(m) => (BindingTarget::TransformRX, BindingGrid::Scalar(m.to_vec_vecs())),
				BindingValues::TransformRY(m) => (BindingTarget::TransformRY, BindingGrid::Scalar(m.to_vec_vecs())),
				BindingValues::TransformRZ(m) => (BindingTarget::TransformRZ, BindingGrid::Scalar(m.to_vec_vecs())),
				BindingValues::Opacity(m) => (BindingTarget::Opacity, BindingGrid::Scalar(m.to_vec_vecs())),
				BindingValues::Deform(m) => {
					let rows = m
						.to_vec_vecs()
						.into_iter()
						.map(|row| {
							row.into_iter()
								.map(|verts| verts.into_iter().map(|v| [v.x, v.y]).collect())
								.collect()
						})
						.collect();
					(BindingTarget::Deform, BindingGrid::Deform(rows))
				}
			};
			RigBinding {
				node_id: b.node.0,
				target,
				interpolate_mode: b.interpolate_mode.as_str().to_owned(),
				is_set: b.is_set.to_vec_vecs(),
				values,
			}
		})
		.collect();

	let additive = p.additive;

	RigParam {
		id: p.uuid.0,
		name: p.name.clone(),
		is_vec2: p.is_vec2,
		min: [p.min.x, p.min.y],
		max: [p.max.x, p.max.y],
		defaults: [p.defaults.x, p.defaults.y],
		axis_x: p.axis_points.x.clone(),
		axis_y: p.axis_points.y.clone(),
		additive,
		bindings,
		extras: extras.map(json_obj_to_map).unwrap_or_default(),
	}
}

/// Convert an in-memory [`Model`] into the format-neutral [`RigDocument`] IR.
pub fn model_to_ir(model: &Model) -> RigDocument {
	let puppet = &model.puppet;
	let root_id = puppet.nodes.root_node_id;

	let meta = RigMeta {
		name: puppet.meta.name.clone(),
		version: puppet.meta.version.clone(),
		rigger: puppet.meta.rigger.clone(),
		artist: puppet.meta.artist.clone(),
		rights: puppet.meta.rights.as_ref().map(|r| RigUsageRights {
			allowed_users: match r.allowed_users {
				PuppetAllowedUsers::OnlyAuthor => "OnlyAuthor".to_owned(),
				PuppetAllowedUsers::OnlyLicensee => "OnlyLicensee".to_owned(),
				PuppetAllowedUsers::Everyone => "Everyone".to_owned(),
			},
			allow_violence: r.allow_violence,
			allow_sexual: r.allow_sexual,
			allow_commercial: r.allow_commercial,
			allow_redistribution: match r.allow_redistribution {
				PuppetAllowedRedistribution::Prohibited => "Prohibited".to_owned(),
				PuppetAllowedRedistribution::ViralLicense => "ViralLicense".to_owned(),
				PuppetAllowedRedistribution::CopyleftLicense => "CopyleftLicense".to_owned(),
			},
			allow_modification: match r.allow_modification {
				PuppetAllowedModification::Prohibited => "Prohibited".to_owned(),
				PuppetAllowedModification::AllowPersonal => "AllowPersonal".to_owned(),
				PuppetAllowedModification::AllowRedistribute => "AllowRedistribute".to_owned(),
			},
			require_attribution: r.require_attribution,
		}),
		copyright: puppet.meta.copyright.clone(),
		license_url: puppet.meta.license_url.clone(),
		contact: puppet.meta.contact.clone(),
		reference: puppet.meta.reference.clone(),
		thumbnail_texture: puppet.meta.thumbnail_id,
		preserve_pixels: puppet.meta.preserve_pixels,
	};

	let physics_world = PhysicsWorld {
		pixels_per_meter: puppet.physics.pixels_per_meter,
		gravity: puppet.physics.gravity,
	};

	let comps = &puppet.node_comps;
	let nodes = puppet
		.nodes
		.pre_order_iter()
		.map(|n| {
			let parent = if n.uuid == root_id {
				None
			} else {
				Some(puppet.nodes.get_parent(n.uuid).uuid.0)
			};

			let mesh_extras = comps.get::<MeshExtraFields>(n.uuid);
			let kind = if let Some(raw) = comps.get::<RawNodeData>(n.uuid) {
				NodeKind::Opaque {
					type_name: raw.type_name.clone(),
					fields: json_obj_to_map(&raw.fields),
				}
			} else if let Some(ap) = comps.get::<AnimatedPart>(n.uuid) {
				let (blending, masks) = comps
					.get::<Drawable>(n.uuid)
					.map(drawable_to_ir)
					.unwrap_or_default();
				let mesh = comps.get::<Mesh>(n.uuid).map(|m| mesh_to_ir(m, mesh_extras));
				let textures = comps.get::<TexturedMesh>(n.uuid).map(|tm| TextureSlots {
					albedo: tm.tex_albedo.0 as u32,
					emissive: (tm.tex_emissive.0 != 0).then_some(tm.tex_emissive.0 as u32),
					bumpmap: (tm.tex_bumpmap.0 != 0).then_some(tm.tex_bumpmap.0 as u32),
				});
				NodeKind::AnimatedPart(AnimatedPartData {
					blending,
					masks,
					mesh,
					textures,
					frame_rate: ap.frame_rate,
					loop_anim: ap.loop_anim,
				})
			} else if comps.get::<MaskNode>(n.uuid).is_some() {
				let mesh = comps.get::<Mesh>(n.uuid).map(|m| mesh_to_ir(m, mesh_extras));
				let drawable_pair = comps.get::<Drawable>(n.uuid).map(drawable_to_ir);
				let (blending, masks) = match drawable_pair {
					Some((b, m)) => (Some(b), m),
					None => (None, None),
				};
				NodeKind::Mask(MaskNodeData { mesh, blending, masks })
			} else if let (Some(tm), Some(mesh)) = (comps.get::<TexturedMesh>(n.uuid), comps.get::<Mesh>(n.uuid)) {
				let (blending, masks) = comps
					.get::<Drawable>(n.uuid)
					.map(drawable_to_ir)
					.unwrap_or_default();
				NodeKind::Part(PartData {
					mesh: mesh_to_ir(mesh, mesh_extras),
					textures: TextureSlots {
						albedo: tm.tex_albedo.0 as u32,
						emissive: (tm.tex_emissive.0 != 0).then_some(tm.tex_emissive.0 as u32),
						bumpmap: (tm.tex_bumpmap.0 != 0).then_some(tm.tex_bumpmap.0 as u32),
					},
					blending,
					masks,
				})
			} else if let (Some(mg), Some(mesh)) = (comps.get::<MeshGroup>(n.uuid), comps.get::<Mesh>(n.uuid)) {
				NodeKind::MeshGroup(MeshGroupData {
					mesh: mesh_to_ir(mesh, mesh_extras),
					dynamic: mg.dynamic,
					translate_children: mg.translate_children,
				})
			} else if let Some(grid) = comps.get::<GridDeformer>(n.uuid) {
				NodeKind::GridDeformer(GridDeformerData {
					axis_x: grid.axis_x.clone(),
					axis_y: grid.axis_y.clone(),
					formation: grid.formation.as_str().to_owned(),
					dynamic: grid.dynamic,
					translate_children: grid.translate_children,
				})
			} else if let Some(path) = comps.get::<PathDeformer>(n.uuid) {
				NodeKind::PathDeformer(PathDeformerData {
					curve_type: path.curve_type.as_str().to_owned(),
					vertices: path.vertices.iter().map(|v| [v.x, v.y]).collect(),
					dynamic: path.dynamic,
					physics_only: path.physics_only,
				})
			} else if let Some(dc) = comps.get::<DynamicComposite>(n.uuid) {
				let (blending, masks) = comps
					.get::<Drawable>(n.uuid)
					.map(drawable_to_ir)
					.unwrap_or_default();
				NodeKind::DynamicComposite(DynamicCompositeData {
					blending,
					masks,
					auto_resized: dc.auto_resized,
				})
			} else if comps.get::<Composite>(n.uuid).is_some() {
				let (blending, masks) = comps
					.get::<Drawable>(n.uuid)
					.map(drawable_to_ir)
					.unwrap_or_default();
				NodeKind::Composite(CompositeData { blending, masks })
			} else if let Some(sh) = comps.get::<Shapes>(n.uuid) {
				NodeKind::Shapes(ShapesData {
					closed: sh.closed,
					vertices: sh.vertices.iter().map(|v| [v.x, v.y]).collect(),
				})
			} else if let Some(drv) = comps.get::<Driver>(n.uuid) {
				NodeKind::Driver(DriverData {
					driver_type: drv.driver_type.clone(),
				})
			} else if let Some(sp) = comps.get::<SimplePhysics>(n.uuid) {
				NodeKind::SimplePhysics(SimplePhysicsData {
					param_id: sp.param.0,
					model_type: match sp.model_type {
						PhysicsModel::RigidPendulum => "Pendulum".to_owned(),
						PhysicsModel::SpringPendulum => "SpringPendulum".to_owned(),
					},
					map_mode: match sp.map_mode {
						PhysicsParamMapMode::AngleLength => "AngleLength".to_owned(),
						PhysicsParamMapMode::LengthAngle => "LengthAngle".to_owned(),
						PhysicsParamMapMode::XY => "XY".to_owned(),
						PhysicsParamMapMode::YX => "YX".to_owned(),
					},
					gravity: sp.props.gravity,
					length: sp.props.length,
					frequency: sp.props.frequency,
					angle_damping: sp.props.angle_damping,
					length_damping: sp.props.length_damping,
					output_scale: sp.props.output_scale.to_array(),
					local_only: sp.local_only,
				})
			} else {
				NodeKind::Group
			};

			RigNode {
				id: n.uuid.0,
				parent,
				name: n.name.clone(),
				enabled: n.enabled,
				zsort: n.zsort,
				transform: RigTransform {
					translation: n.trans_offset.translation.to_array(),
					rotation: n.trans_offset.rotation.to_array(),
					scale: n.trans_offset.scale.to_array(),
					pixel_snap: n.trans_offset.pixel_snap,
				},
				lock_to_root: n.lock_to_root,
				kind,
				extras: comps
					.get::<ExtraFields>(n.uuid)
					.map(|e| json_obj_to_map(&e.0))
					.unwrap_or_default(),
			}
		})
		.collect();

	let mut param_refs: Vec<&Param> = puppet.params.values().collect();
	param_refs.sort_by_key(|p| {
		(
			puppet.param_order.iter().position(|n| *n == p.name).unwrap_or(usize::MAX),
			p.uuid,
		)
	});
	let params = param_refs
		.into_iter()
		.map(|p| param_to_ir(p, puppet.param_extras.get(&p.name)))
		.collect();

	let animations = puppet
		.animations
		.iter()
		.map(|(name, anim)| {
			let lanes = anim
				.lanes
				.iter()
				.map(|l| RigAnimationLane {
					interpolation: l.interpolation.as_str().to_owned(),
					param_id: l.param_uuid.0,
					target: l.target,
					merge_mode: l.merge_mode.clone(),
					keyframes: l
						.keyframes
						.iter()
						.map(|k| RigKeyframe {
							frame: k.frame,
							value: k.value,
							tension: k.tension,
						})
						.collect(),
				})
				.collect();
			(
				name.clone(),
				RigAnimation {
					timestep: anim.timestep,
					additive: anim.additive,
					length: anim.length,
					lead_in: anim.lead_in,
					lead_out: anim.lead_out,
					lanes,
				},
			)
		})
		.collect();

	let textures = model
		.textures
		.iter()
		.map(|t| TextureBlob {
			format: match t.format {
				ImageFormat::Png => TextureEncoding::Png,
				ImageFormat::Jpeg => TextureEncoding::Jpeg,
				ImageFormat::Tga => TextureEncoding::Tga,
				other => TextureEncoding::Unknown(format!("{other:?}")),
			},
			bytes: t.data.to_vec(),
		})
		.collect();

	let vendors = model
		.vendors
		.iter()
		.map(|v| VendorExtension {
			name: v.name.clone(),
			payload: json_to_serde(&v.payload),
		})
		.collect();

	let diagnostics = puppet
		.diagnostics
		.iter()
		.map(|msg| Diagnostic::warning("PUPPET_DIAGNOSTIC", None, msg.clone()))
		.collect();

	let pose_groups = puppet
		.pose_groups
		.iter()
		.map(|pg| IrPoseGroup {
			id: pg.id.clone(),
			parts: pg.parts.iter().map(|id| id.0).collect(),
			fade_seconds: pg.fade_seconds,
		})
		.collect();

	RigDocument {
		schema_version: IR_SCHEMA_VERSION,
		meta,
		physics_world,
		nodes,
		params,
		pose_groups,
		animations,
		textures,
		vendors,
		top_extras: json_obj_to_map(&puppet.top_extras),
		diagnostics,
	}
}

fn ir_to_drawable(blending: &BlendData, masks: Option<&MaskData>, diagnostics: &mut Vec<String>, node_id: u32) -> Result<Drawable, FormatError> {
	let mode = BlendMode::from_name(&blending.mode).unwrap_or_else(|| {
		diagnostics.push(format!(
			"node {node_id} uses unknown blend mode {:?}; falling back to Normal",
			blending.mode
		));
		BlendMode::Normal
	});
	let parsed_masks = if let Some(ms) = masks {
		let mut list = Vec::with_capacity(ms.masks.len());
		for entry in &ms.masks {
			let m = match entry.mode.as_str() {
				"Mask" => MaskMode::Mask,
				"DodgeMask" | "Dodge" => MaskMode::Dodge,
				other => return Err(FormatError::InvalidInput(format!("unknown mask mode {other:?}"))),
			};
			list.push(Mask {
				source: InoxNodeUuid(entry.source_node),
				mode: m,
			});
		}
		Some(Masks {
			threshold: ms.threshold,
			masks: list,
		})
	} else {
		None
	};
	Ok(Drawable {
		blending: Blending {
			mode,
			tint: vec3(blending.tint[0], blending.tint[1], blending.tint[2]),
			screen_tint: vec3(blending.screen_tint[0], blending.screen_tint[1], blending.screen_tint[2]),
			opacity: blending.opacity,
		},
		masks: parsed_masks,
	})
}

fn ir_to_mesh(m: &MeshData) -> Mesh {
	Mesh {
		vertices: m.vertices.iter().map(|v| vec2(v[0], v[1])).collect(),
		uvs: m.uvs.iter().map(|uv| vec2(uv[0], uv[1])).collect(),
		indices: m.indices.clone(),
		origin: vec2(m.origin[0], m.origin[1]),
	}
}

fn ir_to_inox_node(n: &RigNode) -> InoxNode {
	InoxNode {
		uuid: InoxNodeUuid(n.id),
		name: n.name.clone(),
		enabled: n.enabled,
		zsort: n.zsort,
		trans_offset: TransformOffset {
			translation: vec3(
				n.transform.translation[0],
				n.transform.translation[1],
				n.transform.translation[2],
			),
			rotation: vec3(
				n.transform.rotation[0],
				n.transform.rotation[1],
				n.transform.rotation[2],
			),
			scale: vec2(n.transform.scale[0], n.transform.scale[1]),
			pixel_snap: n.transform.pixel_snap,
		},
		lock_to_root: n.lock_to_root,
	}
}

fn attach_node_components(puppet: &mut Puppet, n: &RigNode) -> Result<(), FormatError> {
	let id = InoxNodeUuid(n.id);
	if !n.extras.is_empty() {
		puppet.node_comps.add(id, ExtraFields(map_to_json_obj(&n.extras)));
	}
	match &n.kind {
		NodeKind::Group => {}
		NodeKind::Part(p) => {
			let d = ir_to_drawable(&p.blending, p.masks.as_ref(), &mut puppet.diagnostics, n.id)?;
			puppet.node_comps.add(id, d);
			puppet.node_comps.add(
				id,
				TexturedMesh {
					tex_albedo: TextureId(p.textures.albedo as usize),
					tex_emissive: TextureId(p.textures.emissive.unwrap_or(0) as usize),
					tex_bumpmap: TextureId(p.textures.bumpmap.unwrap_or(0) as usize),
				},
			);
			if !p.mesh.extras.is_empty() {
				puppet.node_comps.add(id, MeshExtraFields(map_to_json_obj(&p.mesh.extras)));
			}
			puppet.node_comps.add(id, ir_to_mesh(&p.mesh));
		}
		NodeKind::Composite(c) => {
			let d = ir_to_drawable(&c.blending, c.masks.as_ref(), &mut puppet.diagnostics, n.id)?;
			puppet.node_comps.add(id, d);
			puppet.node_comps.add(id, Composite {});
		}
		NodeKind::DynamicComposite(dc) => {
			let d = ir_to_drawable(&dc.blending, dc.masks.as_ref(), &mut puppet.diagnostics, n.id)?;
			puppet.node_comps.add(id, d);
			puppet.node_comps.add(id, Composite {});
			puppet.node_comps.add(
				id,
				DynamicComposite {
					auto_resized: dc.auto_resized,
				},
			);
		}
		NodeKind::AnimatedPart(ap) => {
			let d = ir_to_drawable(&ap.blending, ap.masks.as_ref(), &mut puppet.diagnostics, n.id)?;
			puppet.node_comps.add(id, d);
			if let Some(tex) = &ap.textures {
				puppet.node_comps.add(
					id,
					TexturedMesh {
						tex_albedo: TextureId(tex.albedo as usize),
						tex_emissive: TextureId(tex.emissive.unwrap_or(0) as usize),
						tex_bumpmap: TextureId(tex.bumpmap.unwrap_or(0) as usize),
					},
				);
			}
			if let Some(mesh) = &ap.mesh {
				if !mesh.extras.is_empty() {
					puppet.node_comps.add(id, MeshExtraFields(map_to_json_obj(&mesh.extras)));
				}
				puppet.node_comps.add(id, ir_to_mesh(mesh));
			}
			puppet.node_comps.add(
				id,
				AnimatedPart {
					frame_rate: ap.frame_rate,
					loop_anim: ap.loop_anim,
				},
			);
		}
		NodeKind::Mask(m) => {
			if let Some(blending) = &m.blending {
				let d = ir_to_drawable(blending, m.masks.as_ref(), &mut puppet.diagnostics, n.id)?;
				puppet.node_comps.add(id, d);
			}
			if let Some(mesh) = &m.mesh {
				if !mesh.extras.is_empty() {
					puppet.node_comps.add(id, MeshExtraFields(map_to_json_obj(&mesh.extras)));
				}
				puppet.node_comps.add(id, ir_to_mesh(mesh));
			}
			puppet.node_comps.add(id, MaskNode);
		}
		NodeKind::Shapes(s) => {
			puppet.node_comps.add(
				id,
				Shapes {
					closed: s.closed,
					vertices: s.vertices.iter().map(|v| vec2(v[0], v[1])).collect(),
				},
			);
		}
		NodeKind::Driver(drv) => {
			puppet.node_comps.add(
				id,
				Driver {
					driver_type: drv.driver_type.clone(),
				},
			);
		}
		NodeKind::MeshGroup(mg) => {
			puppet.node_comps.add(
				id,
				MeshGroup {
					dynamic: mg.dynamic,
					translate_children: mg.translate_children,
				},
			);
			if !mg.mesh.extras.is_empty() {
				puppet.node_comps.add(id, MeshExtraFields(map_to_json_obj(&mg.mesh.extras)));
			}
			puppet.node_comps.add(id, ir_to_mesh(&mg.mesh));
		}
		NodeKind::GridDeformer(g) => {
			let formation = GridFormation::from_name(&g.formation).unwrap_or_default();
			puppet.node_comps.add(
				id,
				GridDeformer {
					axis_x: GridDeformer::normalize_axis(g.axis_x.clone()),
					axis_y: GridDeformer::normalize_axis(g.axis_y.clone()),
					formation,
					dynamic: g.dynamic,
					translate_children: g.translate_children,
				},
			);
		}
		NodeKind::PathDeformer(p) => {
			let curve_type = CurveType::from_name(&p.curve_type).unwrap_or_default();
			puppet.node_comps.add(
				id,
				PathDeformer {
					curve_type,
					vertices: p.vertices.iter().map(|v| vec2(v[0], v[1])).collect(),
					dynamic: p.dynamic,
					physics_only: p.physics_only,
				},
			);
		}
		NodeKind::SimplePhysics(sp) => {
			let model_type = match sp.model_type.as_str() {
				"Pendulum" | "RigidPendulum" => PhysicsModel::RigidPendulum,
				"SpringPendulum" => PhysicsModel::SpringPendulum,
				other => return Err(FormatError::InvalidInput(format!("unknown physics model {other:?}"))),
			};
			let map_mode = match sp.map_mode.as_str() {
				"AngleLength" => PhysicsParamMapMode::AngleLength,
				"LengthAngle" => PhysicsParamMapMode::LengthAngle,
				"XY" => PhysicsParamMapMode::XY,
				"YX" => PhysicsParamMapMode::YX,
				other => return Err(FormatError::InvalidInput(format!("unknown physics map_mode {other:?}"))),
			};
			puppet.node_comps.add(
				id,
				SimplePhysics {
					param: ParamUuid(sp.param_id),
					model_type,
					map_mode,
					props: PhysicsProps {
						gravity: sp.gravity,
						length: sp.length,
						frequency: sp.frequency,
						angle_damping: sp.angle_damping,
						length_damping: sp.length_damping,
						output_scale: vec2(sp.output_scale[0], sp.output_scale[1]),
					},
					local_only: sp.local_only,
				},
			);
		}
		NodeKind::Opaque { type_name, fields } => {
			puppet.node_comps.add(
				id,
				RawNodeData {
					type_name: type_name.clone(),
					fields: map_to_json_obj(fields),
				},
			);
		}
	}
	Ok(())
}

/// Convert a format-neutral [`RigDocument`] IR into an in-memory [`Model`].
pub fn ir_to_model(doc: &RigDocument) -> Result<Model, FormatError> {
	doc.validate().map_err(FormatError::ValidationFailed)?;

	let rights = if let Some(r) = &doc.meta.rights {
		Some(PuppetUsageRights {
			allowed_users: match r.allowed_users.as_str() {
				"OnlyAuthor" => PuppetAllowedUsers::OnlyAuthor,
				"OnlyLicensee" => PuppetAllowedUsers::OnlyLicensee,
				"Everyone" => PuppetAllowedUsers::Everyone,
				other => return Err(FormatError::InvalidInput(format!("unknown allowed_users {other:?}"))),
			},
			allow_violence: r.allow_violence,
			allow_sexual: r.allow_sexual,
			allow_commercial: r.allow_commercial,
			allow_redistribution: match r.allow_redistribution.as_str() {
				"Prohibited" => PuppetAllowedRedistribution::Prohibited,
				"ViralLicense" => PuppetAllowedRedistribution::ViralLicense,
				"CopyleftLicense" => PuppetAllowedRedistribution::CopyleftLicense,
				other => return Err(FormatError::InvalidInput(format!("unknown allow_redistribution {other:?}"))),
			},
			allow_modification: match r.allow_modification.as_str() {
				"Prohibited" => PuppetAllowedModification::Prohibited,
				"AllowPersonal" => PuppetAllowedModification::AllowPersonal,
				"AllowRedistribute" => PuppetAllowedModification::AllowRedistribute,
				other => return Err(FormatError::InvalidInput(format!("unknown allow_modification {other:?}"))),
			},
			require_attribution: r.require_attribution,
		})
	} else {
		None
	};

	let meta = PuppetMeta {
		name: doc.meta.name.clone(),
		version: doc.meta.version.clone(),
		rigger: doc.meta.rigger.clone(),
		artist: doc.meta.artist.clone(),
		rights,
		copyright: doc.meta.copyright.clone(),
		license_url: doc.meta.license_url.clone(),
		contact: doc.meta.contact.clone(),
		reference: doc.meta.reference.clone(),
		thumbnail_id: doc.meta.thumbnail_texture,
		preserve_pixels: doc.meta.preserve_pixels,
	};

	let physics = PuppetPhysics {
		pixels_per_meter: doc.physics_world.pixels_per_meter,
		gravity: doc.physics_world.gravity,
	};

	let mut params = HashMap::with_capacity(doc.params.len());
	let mut param_order = Vec::with_capacity(doc.params.len());
	let mut param_extras = HashMap::new();
	let mut param_warnings = Vec::new();

	for rp in &doc.params {
		param_order.push(rp.name.clone());
		if !rp.extras.is_empty() {
			param_extras.insert(rp.name.clone(), map_to_json_obj(&rp.extras));
		}
		let mut bindings = Vec::with_capacity(rp.bindings.len());
		for rb in &rp.bindings {
			let interpolate_mode = InterpolateMode::from_name(&rb.interpolate_mode)
				.ok_or_else(|| FormatError::InvalidInput(format!("unknown interpolate_mode {:?}", rb.interpolate_mode)))?;
			let is_set = Matrix2d::from_slice_vecs(&rb.is_set, true)
				.map_err(|e| FormatError::InvalidInput(format!("invalid is_set matrix: {e}")))?;

			let values = match (&rb.target, &rb.values) {
				(BindingTarget::Deform, BindingGrid::Deform(grid)) => {
					let mapped: Vec<Vec<Vec<Vec2>>> = grid
						.iter()
						.map(|row| {
							row.iter()
								.map(|verts| verts.iter().map(|v| vec2(v[0], v[1])).collect())
								.collect()
						})
						.collect();
					BindingValues::Deform(
						Matrix2d::from_slice_vecs(&mapped, true)
							.map_err(|e| FormatError::InvalidInput(format!("invalid deform matrix: {e}")))?,
					)
				}
				(BindingTarget::Custom(name), _) => {
					param_warnings.push(format!(
						"param {:?} binding on node {} uses unsupported target {name:?}; skipped",
						rp.name, rb.node_id
					));
					continue;
				}
				(target, BindingGrid::Scalar(grid)) => {
					let m = Matrix2d::from_slice_vecs(grid, true)
						.map_err(|e| FormatError::InvalidInput(format!("invalid scalar matrix: {e}")))?;
					match target {
						BindingTarget::ZSort => BindingValues::ZSort(m),
						BindingTarget::TransformTX => BindingValues::TransformTX(m),
						BindingTarget::TransformTY => BindingValues::TransformTY(m),
						BindingTarget::TransformSX => BindingValues::TransformSX(m),
						BindingTarget::TransformSY => BindingValues::TransformSY(m),
						BindingTarget::TransformRX => BindingValues::TransformRX(m),
						BindingTarget::TransformRY => BindingValues::TransformRY(m),
						BindingTarget::TransformRZ => BindingValues::TransformRZ(m),
						BindingTarget::Opacity => BindingValues::Opacity(m),
						BindingTarget::Deform | BindingTarget::Custom(_) => unreachable!(),
					}
				}
				(target, grid) => {
					return Err(FormatError::InvalidInput(format!(
						"mismatched binding target {target:?} and grid {grid:?}"
					)))
				}
			};

			bindings.push(Binding {
				node: InoxNodeUuid(rb.node_id),
				is_set,
				interpolate_mode,
				values,
			});
		}

		params.insert(
			rp.name.clone(),
			Param {
				uuid: ParamUuid(rp.id),
				name: rp.name.clone(),
				is_vec2: rp.is_vec2,
				min: vec2(rp.min[0], rp.min[1]),
				max: vec2(rp.max[0], rp.max[1]),
				defaults: vec2(rp.defaults[0], rp.defaults[1]),
				axis_points: AxisPoints {
					x: rp.axis_x.clone(),
					y: rp.axis_y.clone(),
				},
				additive: rp.additive,
				bindings,
			},
		);
	}

	let root_ir = doc
		.nodes
		.iter()
		.find(|n| n.parent.is_none())
		.ok_or_else(|| FormatError::InvalidInput("missing root node".to_owned()))?;
	let mut puppet = Puppet::new(meta, physics, ir_to_inox_node(root_ir), params);
	puppet.param_order = param_order;
	puppet.param_extras = param_extras;
	puppet.top_extras = map_to_json_obj(&doc.top_extras);
	puppet.diagnostics = doc.diagnostics.iter().map(|d| d.message.clone()).collect();
	puppet.diagnostics.extend(param_warnings);

	attach_node_components(&mut puppet, root_ir)?;

	let mut children_by_parent: HashMap<u32, Vec<&RigNode>> = HashMap::new();
	for n in &doc.nodes {
		if let Some(pid) = n.parent {
			children_by_parent.entry(pid).or_default().push(n);
		}
	}
	let mut queue = VecDeque::new();
	queue.push_back(root_ir.id);
	while let Some(pid) = queue.pop_front() {
		if let Some(kids) = children_by_parent.get(&pid) {
			for &kid in kids {
				puppet.nodes.add(InoxNodeUuid(pid), InoxNodeUuid(kid.id), ir_to_inox_node(kid));
				attach_node_components(&mut puppet, kid)?;
				queue.push_back(kid.id);
			}
		}
	}

	for (name, anim) in &doc.animations {
		let lanes = anim
			.lanes
			.iter()
			.map(|l| AnimationLane {
				interpolation: InterpolateMode::from_name(&l.interpolation).unwrap_or(InterpolateMode::Linear),
				param_uuid: ParamUuid(l.param_id),
				target: l.target,
				keyframes: l
					.keyframes
					.iter()
					.map(|k| Keyframe {
						frame: k.frame,
						value: k.value,
						tension: k.tension,
					})
					.collect(),
				merge_mode: l.merge_mode.clone(),
			})
			.collect();
		puppet.animations.insert(
			name.clone(),
			PuppetAnimation {
				timestep: anim.timestep,
				additive: anim.additive,
				length: anim.length,
				lead_in: anim.lead_in,
				lead_out: anim.lead_out,
				lanes,
			},
		);
	}

	puppet.pose_groups = doc
		.pose_groups
		.iter()
		.map(|pg| crate::puppet::PoseGroup {
			id: pg.id.clone(),
			parts: pg.parts.iter().copied().map(InoxNodeUuid).collect(),
			fade_seconds: pg.fade_seconds,
		})
		.collect();

	let mut textures = Vec::with_capacity(doc.textures.len());
	for t in &doc.textures {
		let format = match &t.format {
			TextureEncoding::Png => ImageFormat::Png,
			TextureEncoding::Jpeg => ImageFormat::Jpeg,
			TextureEncoding::Tga => ImageFormat::Tga,
			TextureEncoding::Unknown(ext) => ImageFormat::from_extension(ext)
				.or_else(|| image::guess_format(&t.bytes).ok())
				.ok_or_else(|| FormatError::InvalidInput(format!("unsupported texture encoding {ext:?}")))?,
		};
		textures.push(ModelTexture {
			format,
			data: Arc::from(t.bytes.as_slice()),
		});
	}

	let vendors = doc
		.vendors
		.iter()
		.map(|v| VendorData {
			name: v.name.clone(),
			payload: serde_to_json(&v.payload),
		})
		.collect();

	Ok(Model {
		puppet,
		textures,
		vendors,
	})
}

/// Built-in format plugin for Inochi2D / nijilive binary puppets (`.inp`, `.inx`).
pub struct InpFormat;

impl InpFormat {
	pub fn capabilities() -> FormatCapabilities {
		FormatCapabilities {
			id: "inp".to_owned(),
			display_name: "Inochi2D / nijilive Binary Puppet (.inp/.inx)".to_owned(),
			extensions: vec!["inp".to_owned(), "inx".to_owned()],
			supports_grid_deformer: true,
			supports_path_deformer: true,
			supports_cubic_interpolation: true,
			supports_animations: true,
			supports_opaque_passthrough: true,
			supports_textures: true,
			supports_vendor_data: true,
		}
	}
}

struct CountingWriter<W> {
	inner: W,
	written: usize,
}

impl<W: Write> Write for CountingWriter<W> {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		let n = self.inner.write(buf)?;
		self.written += n;
		Ok(n)
	}

	fn flush(&mut self) -> io::Result<()> {
		self.inner.flush()
	}
}

impl Importer for InpFormat {
	fn info(&self) -> FormatCapabilities {
		Self::capabilities()
	}

	fn import(&self, reader: &mut dyn Read, _options: &ImportOptions) -> Result<ImportReport, FormatError> {
		let model = parse_inp(reader).map_err(|e| FormatError::InvalidInput(e.to_string()))?;
		let document = model_to_ir(&model);
		let warnings = document.diagnostics.clone();
		Ok(ImportReport { document, warnings })
	}
}

impl Exporter for InpFormat {
	fn info(&self) -> FormatCapabilities {
		Self::capabilities()
	}

	fn export(
		&self,
		doc: &RigDocument,
		writer: &mut dyn Write,
		_options: &ExportOptions,
	) -> Result<ExportReport, FormatError> {
		let model = ir_to_model(doc)?;
		let mut cw = CountingWriter {
			inner: writer,
			written: 0,
		};
		write_inp(&model, &mut cw)?;
		Ok(ExportReport {
			bytes_written: cw.written,
			warnings: Vec::new(),
		})
	}
}

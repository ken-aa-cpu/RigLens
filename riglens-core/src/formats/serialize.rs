use glam::{Vec2, Vec3};
use json::object::Object;
use json::JsonValue;

use crate::animation::PuppetAnimation;
use crate::math::transform::TransformOffset;
use crate::node::components::{
	AnimatedPart, BlendMode, Composite, Drawable, Driver, DynamicComposite, ExtraFields, GridDeformer, MaskMode,
	MaskNode, Masks, Mesh, MeshExtraFields, MeshGroup, PathDeformer, PhysicsModel, PhysicsParamMapMode, RawNodeData,
	Shapes, SimplePhysics, TexturedMesh,
};
use crate::node::InoxNodeUuid;
use crate::params::{Binding, BindingValues, Param};
use crate::physics::PuppetPhysics;
use crate::puppet::meta::{
	PuppetAllowedModification, PuppetAllowedRedistribution, PuppetAllowedUsers, PuppetMeta, PuppetUsageRights,
};
use crate::puppet::Puppet;

fn opt_str_empty(v: &Option<String>) -> JsonValue {
	match v {
		Some(s) => JsonValue::String(s.clone()),
		None => JsonValue::String(String::new()),
	}
}

fn vec2_json(v: Vec2) -> JsonValue {
	JsonValue::Array(vec![JsonValue::from(v.x), JsonValue::from(v.y)])
}

fn vec3_json(v: Vec3) -> JsonValue {
	JsonValue::Array(vec![JsonValue::from(v.x), JsonValue::from(v.y), JsonValue::from(v.z)])
}

fn serialize_meta(meta: &PuppetMeta) -> JsonValue {
	let mut obj = Object::new();
	obj.insert("name", opt_str_empty(&meta.name));
	obj.insert("version", JsonValue::String(meta.version.clone()));
	obj.insert("rigger", opt_str_empty(&meta.rigger));
	obj.insert("artist", opt_str_empty(&meta.artist));
	if let Some(ref rights) = meta.rights {
		obj.insert("rights", serialize_rights(rights));
	}
	obj.insert("copyright", opt_str_empty(&meta.copyright));
	obj.insert("licenseURL", opt_str_empty(&meta.license_url));
	obj.insert("contact", opt_str_empty(&meta.contact));
	obj.insert("reference", opt_str_empty(&meta.reference));
	obj.insert("thumbnailId", JsonValue::from(meta.thumbnail_id.unwrap_or(u32::MAX)));
	obj.insert("preservePixels", JsonValue::Boolean(meta.preserve_pixels));
	JsonValue::Object(obj)
}

fn serialize_rights(rights: &PuppetUsageRights) -> JsonValue {
	let mut obj = Object::new();
	obj.insert(
		"allowed_users",
		JsonValue::String(
			match rights.allowed_users {
				PuppetAllowedUsers::OnlyAuthor => "OnlyAuthor",
				PuppetAllowedUsers::OnlyLicensee => "OnlyLicensee",
				PuppetAllowedUsers::Everyone => "Everyone",
			}
			.to_owned(),
		),
	);
	obj.insert("allow_violence", JsonValue::Boolean(rights.allow_violence));
	obj.insert("allow_sexual", JsonValue::Boolean(rights.allow_sexual));
	obj.insert("allow_commercial", JsonValue::Boolean(rights.allow_commercial));
	obj.insert(
		"allow_redistribution",
		JsonValue::String(
			match rights.allow_redistribution {
				PuppetAllowedRedistribution::Prohibited => "Prohibited",
				PuppetAllowedRedistribution::ViralLicense => "ViralLicense",
				PuppetAllowedRedistribution::CopyleftLicense => "CopyleftLicense",
			}
			.to_owned(),
		),
	);
	obj.insert(
		"allow_modification",
		JsonValue::String(
			match rights.allow_modification {
				PuppetAllowedModification::Prohibited => "Prohibited",
				PuppetAllowedModification::AllowPersonal => "AllowPersonal",
				PuppetAllowedModification::AllowRedistribute => "AllowRedistribute",
			}
			.to_owned(),
		),
	);
	obj.insert("require_attribution", JsonValue::Boolean(rights.require_attribution));
	JsonValue::Object(obj)
}

fn serialize_puppet_physics(physics: &PuppetPhysics) -> JsonValue {
	let mut obj = Object::new();
	obj.insert("pixelsPerMeter", JsonValue::from(physics.pixels_per_meter));
	obj.insert("gravity", JsonValue::from(physics.gravity));
	JsonValue::Object(obj)
}

fn serialize_transform(t: &TransformOffset) -> JsonValue {
	let mut obj = Object::new();
	obj.insert("trans", vec3_json(t.translation));
	obj.insert("rot", vec3_json(t.rotation));
	obj.insert("scale", vec2_json(t.scale));
	obj.insert("pixel_snap", JsonValue::Boolean(t.pixel_snap));
	JsonValue::Object(obj)
}

fn blend_mode_str(mode: BlendMode) -> &'static str {
	mode.as_str()
}

fn serialize_masks(masks: &Masks, obj: &mut Object) {
	let arr = masks
		.masks
		.iter()
		.map(|m| {
			let mut mobj = Object::new();
			mobj.insert("source", JsonValue::from(m.source.0));
			mobj.insert(
				"mode",
				JsonValue::String(
					match m.mode {
						MaskMode::Mask => "Mask",
						MaskMode::Dodge => "DodgeMask",
					}
					.to_owned(),
				),
			);
			JsonValue::Object(mobj)
		})
		.collect();
	obj.insert("masks", JsonValue::Array(arr));
}

fn serialize_drawable(drawable: &Drawable, obj: &mut Object) {
	obj.insert(
		"blend_mode",
		JsonValue::String(blend_mode_str(drawable.blending.mode).to_owned()),
	);
	obj.insert("tint", vec3_json(drawable.blending.tint));
	obj.insert("screenTint", vec3_json(drawable.blending.screen_tint));
	let threshold = drawable.masks.as_ref().map_or(0.5, |m| m.threshold);
	obj.insert("mask_threshold", JsonValue::from(threshold));
	obj.insert("opacity", JsonValue::from(drawable.blending.opacity));
	if let Some(ref masks) = drawable.masks {
		if !masks.masks.is_empty() {
			serialize_masks(masks, obj);
		}
	}
}

fn serialize_mesh(mesh: &Mesh) -> JsonValue {
	let mut obj = Object::new();
	let mut verts = Vec::with_capacity(mesh.vertices.len() * 2);
	for v in &mesh.vertices {
		verts.push(JsonValue::from(v.x));
		verts.push(JsonValue::from(v.y));
	}
	let mut uvs = Vec::with_capacity(mesh.uvs.len() * 2);
	for uv in &mesh.uvs {
		uvs.push(JsonValue::from(uv.x));
		uvs.push(JsonValue::from(uv.y));
	}
	let indices = mesh.indices.iter().map(|&i| JsonValue::from(i)).collect();
	obj.insert("verts", JsonValue::Array(verts));
	obj.insert("uvs", JsonValue::Array(uvs));
	obj.insert("indices", JsonValue::Array(indices));
	obj.insert("origin", vec2_json(mesh.origin));
	JsonValue::Object(obj)
}

fn serialize_textured_mesh(tm: &TexturedMesh) -> JsonValue {
	let emissive = if tm.tex_emissive.0 == 0 {
		u32::MAX
	} else {
		tm.tex_emissive.0 as u32
	};
	let bumpmap = if tm.tex_bumpmap.0 == 0 {
		u32::MAX
	} else {
		tm.tex_bumpmap.0 as u32
	};
	JsonValue::Array(vec![
		JsonValue::from(tm.tex_albedo.0 as u32),
		JsonValue::from(emissive),
		JsonValue::from(bumpmap),
	])
}

fn serialize_simple_physics(sp: &SimplePhysics, obj: &mut Object) {
	obj.insert("param", JsonValue::from(sp.param.0));
	obj.insert(
		"model_type",
		JsonValue::String(
			match sp.model_type {
				PhysicsModel::RigidPendulum => "Pendulum",
				PhysicsModel::SpringPendulum => "SpringPendulum",
			}
			.to_owned(),
		),
	);
	obj.insert(
		"map_mode",
		JsonValue::String(
			match sp.map_mode {
				PhysicsParamMapMode::AngleLength => "AngleLength",
				PhysicsParamMapMode::LengthAngle => "LengthAngle",
				PhysicsParamMapMode::XY => "XY",
				PhysicsParamMapMode::YX => "YX",
			}
			.to_owned(),
		),
	);
	obj.insert("gravity", JsonValue::from(sp.props.gravity));
	obj.insert("length", JsonValue::from(sp.props.length));
	obj.insert("frequency", JsonValue::from(sp.props.frequency));
	obj.insert("angle_damping", JsonValue::from(sp.props.angle_damping));
	obj.insert("length_damping", JsonValue::from(sp.props.length_damping));
	obj.insert("output_scale", vec2_json(sp.props.output_scale));
	obj.insert("local_only", JsonValue::Boolean(sp.local_only));
}

fn serialize_node(puppet: &Puppet, id: InoxNodeUuid) -> JsonValue {
	let node = puppet.nodes.get_node(id).expect("node uuid must exist in tree");
	let mut obj = Object::new();
	obj.insert("uuid", JsonValue::from(node.uuid.0));
	obj.insert("name", JsonValue::String(node.name.clone()));

	let has_drawable = puppet.node_comps.get::<Drawable>(id);
	let has_tm = puppet.node_comps.get::<TexturedMesh>(id);
	let has_mesh = puppet.node_comps.get::<Mesh>(id);
	let has_comp = puppet.node_comps.get::<Composite>(id);
	let has_dyn_comp = puppet.node_comps.get::<DynamicComposite>(id);
	let has_anim_part = puppet.node_comps.get::<AnimatedPart>(id);
	let has_mask_node = puppet.node_comps.get::<MaskNode>(id);
	let has_shapes = puppet.node_comps.get::<Shapes>(id);
	let has_driver = puppet.node_comps.get::<Driver>(id);
	let has_phys = puppet.node_comps.get::<SimplePhysics>(id);
	let has_mg = puppet.node_comps.get::<MeshGroup>(id);
	let has_grid = puppet.node_comps.get::<GridDeformer>(id);
	let has_path = puppet.node_comps.get::<PathDeformer>(id);

	let has_raw = puppet.node_comps.get::<RawNodeData>(id);

	let ty: &str = if let Some(raw) = has_raw {
		raw.type_name.as_str()
	} else if has_anim_part.is_some() {
		"AnimatedPart"
	} else if has_mask_node.is_some() {
		"Mask"
	} else if has_tm.is_some() && has_mesh.is_some() {
		"Part"
	} else if has_mg.is_some() && has_mesh.is_some() {
		"MeshGroup"
	} else if has_grid.is_some() {
		"GridDeformer"
	} else if has_path.is_some() {
		"PathDeformer"
	} else if has_dyn_comp.is_some() {
		"DynamicComposite"
	} else if has_comp.is_some() {
		"Composite"
	} else if has_shapes.is_some() {
		"Shapes"
	} else if has_driver.is_some() {
		"Driver"
	} else if has_phys.is_some() {
		"SimplePhysics"
	} else {
		"Node"
	};

	obj.insert("type", JsonValue::String(ty.to_owned()));
	obj.insert("enabled", JsonValue::Boolean(node.enabled));
	obj.insert("zsort", JsonValue::from(node.zsort));
	obj.insert("transform", serialize_transform(&node.trans_offset));
	obj.insert("lockToRoot", JsonValue::Boolean(node.lock_to_root));

	match ty {
		"Part" => {
			if let Some(d) = has_drawable {
				serialize_drawable(d, &mut obj);
			}
			if let Some(tm) = has_tm {
				obj.insert("textures", serialize_textured_mesh(tm));
			}
			if let Some(mesh) = has_mesh {
				obj.insert("mesh", serialize_mesh(mesh));
			}
		}
		"Composite" => {
			if let Some(d) = has_drawable {
				serialize_drawable(d, &mut obj);
			}
		}
		"DynamicComposite" => {
			if let Some(d) = has_drawable {
				serialize_drawable(d, &mut obj);
			}
			if let Some(dc) = has_dyn_comp {
				obj.insert("auto_resized", JsonValue::Boolean(dc.auto_resized));
			}
		}
		"AnimatedPart" => {
			if let Some(d) = has_drawable {
				serialize_drawable(d, &mut obj);
			}
			if let Some(tm) = has_tm {
				obj.insert("textures", serialize_textured_mesh(tm));
			}
			if let Some(mesh) = has_mesh {
				obj.insert("mesh", serialize_mesh(mesh));
			}
			if let Some(ap) = has_anim_part {
				obj.insert("frame_rate", JsonValue::from(ap.frame_rate));
				obj.insert("loop", JsonValue::Boolean(ap.loop_anim));
			}
		}
		"Mask" => {
			if let Some(d) = has_drawable {
				serialize_drawable(d, &mut obj);
			}
			if let Some(mesh) = has_mesh {
				obj.insert("mesh", serialize_mesh(mesh));
			}
		}
		"Shapes" => {
			if let Some(s) = has_shapes {
				obj.insert("closed", JsonValue::Boolean(s.closed));
				let mut verts = Vec::with_capacity(s.vertices.len() * 2);
				for v in &s.vertices {
					verts.push(JsonValue::from(v.x));
					verts.push(JsonValue::from(v.y));
				}
				obj.insert("vertices", JsonValue::Array(verts));
			}
		}
		"Driver" => {
			if let Some(drv) = has_driver {
				obj.insert("driver_type", JsonValue::String(drv.driver_type.clone()));
			}
		}
		"SimplePhysics" => {
			if let Some(sp) = has_phys {
				serialize_simple_physics(sp, &mut obj);
			}
		}
		"MeshGroup" => {
			if let Some(mg) = has_mg {
				obj.insert("dynamic_deformation", JsonValue::Boolean(mg.dynamic));
				obj.insert("translate_children", JsonValue::Boolean(mg.translate_children));
			}
			if let Some(mesh) = has_mesh {
				obj.insert("mesh", serialize_mesh(mesh));
			}
		}
		"GridDeformer" => {
			if let Some(g) = has_grid {
				obj.insert(
					"grid_axis_x",
					JsonValue::Array(g.axis_x.iter().copied().map(JsonValue::from).collect()),
				);
				obj.insert(
					"grid_axis_y",
					JsonValue::Array(g.axis_y.iter().copied().map(JsonValue::from).collect()),
				);
				obj.insert("formation", JsonValue::String(g.formation.as_str().to_owned()));
				obj.insert("dynamic", JsonValue::Boolean(g.dynamic));
			}
		}
		"PathDeformer" => {
			if let Some(p) = has_path {
				if p.physics_only {
					obj.insert("physics_only", JsonValue::Boolean(true));
				}
				obj.insert("curve_type", JsonValue::String(p.curve_type.as_str().to_owned()));
				let mut verts = Vec::with_capacity(p.vertices.len() * 2);
				for v in &p.vertices {
					verts.push(JsonValue::from(v.x));
					verts.push(JsonValue::from(v.y));
				}
				obj.insert("vertices", JsonValue::Array(verts));
				obj.insert("dynamic_deformation", JsonValue::Boolean(p.dynamic));
			}
		}
		_ => {
			if let Some(raw) = has_raw {
				for (k, v) in raw.fields.iter() {
					if !obj.iter().any(|(ek, _)| ek == k) {
						obj.insert(k, v.clone());
					}
				}
			}
		}
	}

	if let Some(extra) = puppet.node_comps.get::<MeshExtraFields>(id) {
		if let Some(JsonValue::Object(mesh_obj)) = obj.get_mut("mesh") {
			for (k, v) in extra.0.iter() {
				if mesh_obj.get(k).is_none() {
					mesh_obj.insert(k, v.clone());
				}
			}
		}
	}
	if let Some(extra) = puppet.node_comps.get::<ExtraFields>(id) {
		for (k, v) in extra.0.iter() {
			if !obj.iter().any(|(ek, _)| ek == k) {
				obj.insert(k, v.clone());
			}
		}
	}

	let children: Vec<JsonValue> = puppet
		.nodes
		.get_children(id)
		.map(|child| serialize_node(puppet, child.uuid))
		.collect();
	if !children.is_empty() {
		obj.insert("children", JsonValue::Array(children));
	}

	JsonValue::Object(obj)
}

fn serialize_f32_matrix(m: &crate::math::matrix::Matrix2d<f32>) -> JsonValue {
	JsonValue::Array(
		m.to_vec_vecs()
			.into_iter()
			.map(|row| JsonValue::Array(row.into_iter().map(JsonValue::from).collect()))
			.collect(),
	)
}

fn serialize_binding(b: &Binding) -> JsonValue {
	let mut obj = Object::new();
	obj.insert("node", JsonValue::from(b.node.0));

	let is_set = b
		.is_set
		.to_vec_vecs()
		.into_iter()
		.map(|row| JsonValue::Array(row.into_iter().map(JsonValue::Boolean).collect()))
		.collect();
	obj.insert("isSet", JsonValue::Array(is_set));

	obj.insert(
		"interpolate_mode",
		JsonValue::String(
			b.interpolate_mode.as_str().to_owned(),
		),
	);

	let (param_name, values) = match &b.values {
		BindingValues::ZSort(m) => ("zSort", serialize_f32_matrix(m)),
		BindingValues::TransformTX(m) => ("transform.t.x", serialize_f32_matrix(m)),
		BindingValues::TransformTY(m) => ("transform.t.y", serialize_f32_matrix(m)),
		BindingValues::TransformSX(m) => ("transform.s.x", serialize_f32_matrix(m)),
		BindingValues::TransformSY(m) => ("transform.s.y", serialize_f32_matrix(m)),
		BindingValues::TransformRX(m) => ("transform.r.x", serialize_f32_matrix(m)),
		BindingValues::TransformRY(m) => ("transform.r.y", serialize_f32_matrix(m)),
		BindingValues::TransformRZ(m) => ("transform.r.z", serialize_f32_matrix(m)),
		BindingValues::Deform(m) => {
			let rows = m
				.to_vec_vecs()
				.into_iter()
				.map(|row| {
					JsonValue::Array(
						row.into_iter()
							.map(|verts| JsonValue::Array(verts.into_iter().map(vec2_json).collect()))
							.collect(),
					)
				})
				.collect();
			("deform", JsonValue::Array(rows))
		}
		BindingValues::Opacity(m) => ("opacity", serialize_f32_matrix(m)),
	};

	obj.insert("param_name", JsonValue::String(param_name.to_owned()));
	obj.insert("values", values);
	JsonValue::Object(obj)
}

fn serialize_param(p: &Param) -> JsonValue {
	let mut obj = Object::new();
	obj.insert("uuid", JsonValue::from(p.uuid.0));
	obj.insert("name", JsonValue::String(p.name.clone()));
	obj.insert("is_vec2", JsonValue::Boolean(p.is_vec2));
	obj.insert("min", vec2_json(p.min));
	obj.insert("max", vec2_json(p.max));
	obj.insert("defaults", vec2_json(p.defaults));
	obj.insert(
		"axis_points",
		JsonValue::Array(vec![
			JsonValue::Array(p.axis_points.x.iter().copied().map(JsonValue::from).collect()),
			JsonValue::Array(p.axis_points.y.iter().copied().map(JsonValue::from).collect()),
		]),
	);
	obj.insert("merge_mode", JsonValue::String("Additive".to_owned()));
	obj.insert(
		"bindings",
		JsonValue::Array(p.bindings.iter().map(serialize_binding).collect()),
	);
	JsonValue::Object(obj)
}

fn serialize_animation(anim: &PuppetAnimation) -> JsonValue {
	let mut obj = Object::new();
	obj.insert("timestep", JsonValue::from(anim.timestep));
	obj.insert("additive", JsonValue::Boolean(anim.additive));
	obj.insert("length", JsonValue::from(anim.length));
	obj.insert("leadIn", JsonValue::from(anim.lead_in));
	obj.insert("leadOut", JsonValue::from(anim.lead_out));
	obj.insert("animationWeight", JsonValue::from(1.0f32));

	let lanes = anim
		.lanes
		.iter()
		.map(|lane| {
			let mut lobj = Object::new();
			lobj.insert(
				"interpolation",
				JsonValue::String(
					lane.interpolation.as_str().to_owned(),
				),
			);
			lobj.insert("uuid", JsonValue::from(lane.param_uuid.0));
			lobj.insert("target", JsonValue::from(lane.target));
			lobj.insert("merge_mode", JsonValue::String(lane.merge_mode.clone()));
			let kfs = lane
				.keyframes
				.iter()
				.map(|kf| {
					let mut kobj = Object::new();
					kobj.insert("frame", JsonValue::from(kf.frame));
					kobj.insert("value", JsonValue::from(kf.value));
					kobj.insert("tension", JsonValue::from(kf.tension));
					JsonValue::Object(kobj)
				})
				.collect();
			lobj.insert("keyframes", JsonValue::Array(kfs));
			JsonValue::Object(lobj)
		})
		.collect();
	obj.insert("lanes", JsonValue::Array(lanes));
	JsonValue::Object(obj)
}

impl Puppet {
	/// Serialize this puppet to the official Inochi2D JSON payload format.
	pub fn to_json(&self) -> JsonValue {
		let mut obj = Object::new();
		obj.insert("meta", serialize_meta(&self.meta));
		obj.insert("physics", serialize_puppet_physics(&self.physics));
		obj.insert("nodes", serialize_node(self, self.nodes.root_node_id));

		let mut params: Vec<&Param> = self.params.values().collect();
		// Keep file order for known params; new params follow, ordered by uuid.
		params.sort_by_key(|p| {
			(
				self.param_order.iter().position(|n| *n == p.name).unwrap_or(usize::MAX),
				p.uuid,
			)
		});
		let param_json: Vec<JsonValue> = params
			.into_iter()
			.map(|p| {
				let mut v = serialize_param(p);
				if let (JsonValue::Object(o), Some(extra)) = (&mut v, self.param_extras.get(&p.name)) {
					for (k, ev) in extra.iter() {
						if o.get(k).is_none() {
							o.insert(k, ev.clone());
						}
					}
				}
				v
			})
			.collect();
		obj.insert("param", JsonValue::Array(param_json));
		for (k, v) in self.top_extras.iter() {
			if obj.get(k).is_none() {
				obj.insert(k, v.clone());
			}
		}
		if obj.get("automation").is_none() {
			obj.insert("automation", JsonValue::Array(Vec::new()));
		}
		if obj.get("groups").is_none() {
			obj.insert("groups", JsonValue::Array(Vec::new()));
		}

		if !self.animations.is_empty() {
			let mut anim_names: Vec<&String> = self.animations.keys().collect();
			anim_names.sort();
			let mut anims_obj = Object::new();
			for name in anim_names {
				anims_obj.insert(name, serialize_animation(&self.animations[name]));
			}
			obj.insert("animations", JsonValue::Object(anims_obj));
		}

		JsonValue::Object(obj)
	}
}

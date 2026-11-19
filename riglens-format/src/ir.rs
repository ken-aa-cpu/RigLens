use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

pub const IR_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
	Info,
	Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
	pub severity: Severity,
	pub code: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub node_id: Option<u32>,
	pub message: String,
}

impl Diagnostic {
	pub fn warning(code: impl Into<String>, node_id: Option<u32>, message: impl Into<String>) -> Self {
		Self {
			severity: Severity::Warning,
			code: code.into(),
			node_id,
			message: message.into(),
		}
	}

	pub fn info(code: impl Into<String>, node_id: Option<u32>, message: impl Into<String>) -> Self {
		Self {
			severity: Severity::Info,
			code: code.into(),
			node_id,
			message: message.into(),
		}
	}
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RigUsageRights {
	pub allowed_users: String,
	pub allow_violence: bool,
	pub allow_sexual: bool,
	pub allow_commercial: bool,
	pub allow_redistribution: String,
	pub allow_modification: String,
	pub require_attribution: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RigMeta {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
	pub version: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub rigger: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub artist: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub rights: Option<RigUsageRights>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub copyright: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub license_url: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub contact: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reference: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thumbnail_texture: Option<u32>,
	#[serde(default)]
	pub preserve_pixels: bool,
}

impl Default for RigMeta {
	fn default() -> Self {
		Self {
			name: None,
			version: "1.0-alpha".to_owned(),
			rigger: None,
			artist: None,
			rights: None,
			copyright: None,
			license_url: None,
			contact: None,
			reference: None,
			thumbnail_texture: None,
			preserve_pixels: false,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhysicsWorld {
	pub pixels_per_meter: f32,
	pub gravity: f32,
}

impl Default for PhysicsWorld {
	fn default() -> Self {
		Self {
			pixels_per_meter: 1000.0,
			gravity: 9.8,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigTransform {
	pub translation: [f32; 3],
	pub rotation: [f32; 3],
	pub scale: [f32; 2],
	#[serde(default)]
	pub pixel_snap: bool,
}

impl Default for RigTransform {
	fn default() -> Self {
		Self {
			translation: [0.0, 0.0, 0.0],
			rotation: [0.0, 0.0, 0.0],
			scale: [1.0, 1.0],
			pixel_snap: false,
		}
	}
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshData {
	pub vertices: Vec<[f32; 2]>,
	pub uvs: Vec<[f32; 2]>,
	pub indices: Vec<u32>,
	pub origin: [f32; 2],
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub extras: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextureSlots {
	pub albedo: u32,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub emissive: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub bumpmap: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlendData {
	pub mode: String,
	pub tint: [f32; 3],
	pub screen_tint: [f32; 3],
	pub opacity: f32,
}

impl Default for BlendData {
	fn default() -> Self {
		Self {
			mode: "Normal".to_owned(),
			tint: [1.0, 1.0, 1.0],
			screen_tint: [0.0, 0.0, 0.0],
			opacity: 1.0,
		}
	}
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaskEntry {
	pub source_node: u32,
	pub mode: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskData {
	pub threshold: f32,
	pub masks: Vec<MaskEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartData {
	pub mesh: MeshData,
	pub textures: TextureSlots,
	pub blending: BlendData,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub masks: Option<MaskData>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompositeData {
	pub blending: BlendData,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub masks: Option<MaskData>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshGroupData {
	pub mesh: MeshData,
	pub dynamic: bool,
	pub translate_children: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridDeformerData {
	pub axis_x: Vec<f32>,
	pub axis_y: Vec<f32>,
	pub formation: String,
	pub dynamic: bool,
	pub translate_children: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathDeformerData {
	pub curve_type: String,
	pub vertices: Vec<[f32; 2]>,
	pub dynamic: bool,
	pub physics_only: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimplePhysicsData {
	pub param_id: u32,
	pub model_type: String,
	pub map_mode: String,
	pub gravity: f32,
	pub length: f32,
	pub frequency: f32,
	pub angle_damping: f32,
	pub length_damping: f32,
	pub output_scale: [f32; 2],
	pub local_only: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum NodeKind {
	Group,
	Part(PartData),
	Composite(CompositeData),
	MeshGroup(MeshGroupData),
	GridDeformer(GridDeformerData),
	PathDeformer(PathDeformerData),
	SimplePhysics(SimplePhysicsData),
	Opaque {
		type_name: String,
		#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
		fields: BTreeMap<String, serde_json::Value>,
	},
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigNode {
	pub id: u32,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub parent: Option<u32>,
	pub name: String,
	pub enabled: bool,
	pub zsort: f32,
	pub transform: RigTransform,
	pub lock_to_root: bool,
	pub kind: NodeKind,
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub extras: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BindingTarget {
	ZSort,
	TransformTX,
	TransformTY,
	TransformSX,
	TransformSY,
	TransformRX,
	TransformRY,
	TransformRZ,
	Opacity,
	Deform,
	Custom(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "grid_type", content = "data")]
pub enum BindingGrid {
	Scalar(Vec<Vec<f32>>),
	Deform(Vec<Vec<Vec<[f32; 2]>>>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigBinding {
	pub node_id: u32,
	pub target: BindingTarget,
	pub interpolate_mode: String,
	pub is_set: Vec<Vec<bool>>,
	pub values: BindingGrid,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigParam {
	pub id: u32,
	pub name: String,
	pub is_vec2: bool,
	pub min: [f32; 2],
	pub max: [f32; 2],
	pub defaults: [f32; 2],
	pub axis_x: Vec<f32>,
	pub axis_y: Vec<f32>,
	pub bindings: Vec<RigBinding>,
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub extras: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigKeyframe {
	pub frame: i32,
	pub value: f32,
	pub tension: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigAnimationLane {
	pub interpolation: String,
	pub param_id: u32,
	pub target: u8,
	pub merge_mode: String,
	pub keyframes: Vec<RigKeyframe>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigAnimation {
	pub timestep: f32,
	pub additive: bool,
	pub length: i32,
	pub lead_in: i32,
	pub lead_out: i32,
	pub lanes: Vec<RigAnimationLane>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureEncoding {
	Png,
	Jpeg,
	Tga,
	Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextureBlob {
	pub format: TextureEncoding,
	#[serde(with = "crate::base64")]
	pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VendorExtension {
	pub name: String,
	pub payload: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigDocument {
	pub schema_version: u32,
	pub meta: RigMeta,
	pub physics_world: PhysicsWorld,
	pub nodes: Vec<RigNode>,
	pub params: Vec<RigParam>,
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub animations: BTreeMap<String, RigAnimation>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub textures: Vec<TextureBlob>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub vendors: Vec<VendorExtension>,
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub top_extras: BTreeMap<String, serde_json::Value>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub diagnostics: Vec<Diagnostic>,
}

impl RigDocument {
	/// Validate structural invariants of this rig document.
	pub fn validate(&self) -> Result<(), Vec<String>> {
		let mut errors = Vec::new();

		if self.nodes.is_empty() {
			errors.push("document contains no nodes".to_owned());
			return Err(errors);
		}

		let mut by_id: HashMap<u32, Option<u32>> = HashMap::with_capacity(self.nodes.len());
		let mut roots = Vec::new();
		for n in &self.nodes {
			if by_id.insert(n.id, n.parent).is_some() {
				errors.push(format!("duplicate node id {}", n.id));
			}
			if n.parent.is_none() {
				roots.push(n.id);
			}
		}

		if roots.len() != 1 {
			errors.push(format!("expected exactly 1 root node, found {}", roots.len()));
		}

		for n in &self.nodes {
			if let Some(pid) = n.parent {
				if !by_id.contains_key(&pid) {
					errors.push(format!("node {} references missing parent {}", n.id, pid));
					continue;
				}
				let mut cur = Some(pid);
				let mut steps = 0usize;
				let mut seen = HashSet::new();
				while let Some(id) = cur {
					if id == n.id || !seen.insert(id) || steps > self.nodes.len() {
						errors.push(format!("cycle in parent chain at node {}", n.id));
						break;
					}
					steps += 1;
					cur = by_id.get(&id).copied().flatten();
				}
			}

			let mesh_opt = match &n.kind {
				NodeKind::Part(p) => Some(&p.mesh),
				NodeKind::MeshGroup(mg) => Some(&mg.mesh),
				_ => None,
			};
			if let Some(mesh) = mesh_opt {
				if !mesh.uvs.is_empty() && mesh.uvs.len() != mesh.vertices.len() {
					errors.push(format!(
						"node {} mesh uv count {} != vertex count {}",
						n.id,
						mesh.uvs.len(),
						mesh.vertices.len()
					));
				}
				for &idx in &mesh.indices {
					if idx as usize >= mesh.vertices.len() {
						errors.push(format!(
							"node {} mesh index {} out of bounds ({} vertices)",
							n.id,
							idx,
							mesh.vertices.len()
						));
						break;
					}
				}
			}
		}

		for p in &self.params {
			if p.axis_x.is_empty() || p.axis_y.is_empty() {
				errors.push(format!("param {:?} ({}) has empty axis_points", p.name, p.id));
				continue;
			}
			let nx = p.axis_x.len();
			let ny = p.axis_y.len();
			for (bi, b) in p.bindings.iter().enumerate() {
				if b.is_set.len() != nx || b.is_set.iter().any(|row| row.len() != ny) {
					errors.push(format!(
						"param {:?} binding #{bi} is_set dimensions do not match {nx}x{ny}",
						p.name
					));
				}
				let (gx, bad_inner) = match &b.values {
					BindingGrid::Scalar(rows) => (rows.len(), rows.iter().any(|r| r.len() != ny)),
					BindingGrid::Deform(rows) => (rows.len(), rows.iter().any(|r| r.len() != ny)),
				};
				if gx != nx || bad_inner {
					errors.push(format!(
						"param {:?} binding #{bi} value grid dimensions do not match {nx}x{ny}",
						p.name
					));
				}
			}
		}

		if errors.is_empty() {
			Ok(())
		} else {
			Err(errors)
		}
	}
}

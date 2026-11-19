/*!
Inochi2D node types to Inox2D components:
- Node -> (Nothing)
- Part -> Drawable + TexturedMesh + Mesh
- Composite -> Drawable + Composite
- SimplePhysics -> SimplePhysics
- Custom nodes by inheritance -> Custom nodes by composition
*/

use glam::{vec2, Mat4, Vec2, Vec3};

use crate::math::curve::CurveType;
use crate::math::deform::Deform;
use crate::node::{InoxNodeUuid, TransformOffset};
use crate::params::ParamUuid;
use crate::physics::{
	pendulum::{rigid::RigidPendulum, spring::SpringPendulum},
	runge_kutta::PhysicsState,
};
use crate::texture::TextureId;

/* --- RAW (UNKNOWN) NODE --- */

/// Preserves a node whose `type` this crate does not understand (e.g. nijilive's
/// `AnimatedPart`, `DynamicComposite`, `Driver`, `Shapes`, `Mask`).
///
/// The original type name and all type-specific JSON fields are kept verbatim so
/// that saving the puppet again does not destroy data written by other tools.
/// The node is otherwise treated as a plain transform node.
#[derive(Clone, Debug)]
pub struct RawNodeData {
	pub type_name: String,
	/// Type-specific fields only. Common fields (`uuid`, `name`, `type`, `enabled`,
	/// `zsort`, `transform`, `lockToRoot`, `children`) are handled by the node itself.
	pub fields: json::object::Object,
}

/// Fields of a *known* node type that this crate does not model (e.g. `psdLayerPath`,
/// `emissionStrength`, `mesh.grid_axes` parents). Kept verbatim and written back on save
/// unless the serializer already emits a field with the same key.
#[derive(Clone, Debug)]
pub struct ExtraFields(pub json::object::Object);

/// Unmodelled fields inside a node's `mesh` object (e.g. `grid_axes`), kept verbatim.
#[derive(Clone, Debug)]
pub struct MeshExtraFields(pub json::object::Object);

/* --- COMPOSITE --- */

/// If has this as a component, the node should composite all children
///
/// Empty as only a marker, zsorted children list constructed later on demand
#[derive(Clone, Debug)]
pub struct Composite {}

/* --- MESH GROUP --- */

/// If has this as a component, the node deforms its descendant drawables using its own Mesh.
#[derive(Clone, Debug)]
pub struct MeshGroup {
	pub dynamic: bool,
	pub translate_children: bool,
}

/* --- GRID DEFORMER --- */

/// Grid interpolation formation for [`GridDeformer`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GridFormation {
	#[default]
	Bilinear,
}

impl GridFormation {
	/// Name used in `.inp` / `.inx` files.
	pub fn as_str(self) -> &'static str {
		match self {
			GridFormation::Bilinear => "Bilinear",
		}
	}

	/// Parse a grid formation name from JSON.
	pub fn from_name(name: &str) -> Option<Self> {
		match name {
			"Bilinear" => Some(GridFormation::Bilinear),
			_ => None,
		}
	}

	/// Parse a grid formation ordinal (0 = Bilinear).
	pub fn from_ordinal(ord: i64) -> Option<Self> {
		match ord {
			0 => Some(GridFormation::Bilinear),
			_ => None,
		}
	}
}

/// Lattice deformer node (`GridDeformer` in nijilive) defined by 1D `axis_x` and `axis_y` ticks.
#[derive(Clone, Debug)]
pub struct GridDeformer {
	pub axis_x: Vec<f32>,
	pub axis_y: Vec<f32>,
	pub formation: GridFormation,
	pub dynamic: bool,
	pub translate_children: bool,
}

impl GridDeformer {
	/// Number of columns along the X axis.
	pub fn cols(&self) -> usize {
		self.axis_x.len()
	}

	/// Number of rows along the Y axis.
	pub fn rows(&self) -> usize {
		self.axis_y.len()
	}

	/// Total number of grid control vertices (`cols * rows`).
	pub fn vertex_count(&self) -> usize {
		self.cols() * self.rows()
	}

	/// Returns `true` if the grid has at least a 2x2 lattice.
	pub fn has_valid_grid(&self) -> bool {
		self.cols() >= 2 && self.rows() >= 2
	}

	/// Row-major index into grid vertices / deformations (matches nijilive `y * cols + x`).
	pub fn grid_index(&self, x: usize, y: usize) -> usize {
		y * self.cols() + x
	}

	/// Rest grid control point coordinates in row-major order.
	pub fn rest_vertices(&self) -> Vec<Vec2> {
		let mut out = Vec::with_capacity(self.vertex_count());
		for &y in &self.axis_y {
			for &x in &self.axis_x {
				out.push(vec2(x, y));
			}
		}
		out
	}

	/// Sort and deduplicate axis ticks; keeps strictly ascending slices verbatim.
	pub fn normalize_axis(mut values: Vec<f32>) -> Vec<f32> {
		values.retain(|v| v.is_finite());
		if values.len() < 2 {
			return vec![-0.5, 0.5];
		}
		if values.windows(2).all(|w| w[1] - w[0] > 1e-4) {
			return values;
		}
		values.sort_by(f32::total_cmp);
		let mut out: Vec<f32> = Vec::with_capacity(values.len());
		for v in values {
			if out.last().is_none_or(|&prev| (v - prev).abs() > 1e-4) {
				out.push(v);
			}
		}
		if out.len() < 2 {
			return vec![-0.5, 0.5];
		}
		out
	}

	/// Sample the bilinear deformation vector at `local_pt` (in GridDeformer's local space).
	pub fn sample_deform(&self, grid_deforms: &[Vec2], local_pt: Vec2) -> Vec2 {
		if !self.has_valid_grid() || grid_deforms.len() < self.vertex_count() || !local_pt.is_finite() {
			return Vec2::ZERO;
		}
		let x = local_pt.x.clamp(self.axis_x[0], *self.axis_x.last().unwrap());
		let y = local_pt.y.clamp(self.axis_y[0], *self.axis_y.last().unwrap());
		let (cx, u) = locate_grid_interval(&self.axis_x, x);
		let (cy, v) = locate_grid_interval(&self.axis_y, y);

		let d00 = grid_deforms[self.grid_index(cx, cy)];
		let d10 = grid_deforms[self.grid_index(cx + 1, cy)];
		let d01 = grid_deforms[self.grid_index(cx, cy + 1)];
		let d11 = grid_deforms[self.grid_index(cx + 1, cy + 1)];

		let wu0 = 1.0 - u;
		let wv0 = 1.0 - v;
		d00 * (wu0 * wv0) + d10 * (u * wv0) + d01 * (wu0 * v) + d11 * (u * v)
	}
}

fn locate_grid_interval(axis: &[f32], value: f32) -> (usize, f32) {
	let mut idx = axis.len().saturating_sub(2);
	for i in 0..axis.len().saturating_sub(1) {
		if value <= axis[i + 1] {
			idx = i;
			break;
		}
	}
	let a = axis[idx];
	let b = axis[idx + 1];
	let span = b - a;
	let mut u = if span.abs() <= 1e-8 {
		0.0
	} else {
		((value - a) / span).clamp(0.0, 1.0)
	};
	if u <= 1e-4 {
		u = 0.0;
	} else if u >= 1.0 - 1e-4 {
		u = 1.0;
	}
	(idx, u)
}

/* --- PATH DEFORMER --- */

/// Curve deformer node (`PathDeformer` / `BezierDeformer` in nijilive).
#[derive(Clone, Debug)]
pub struct PathDeformer {
	pub curve_type: CurveType,
	pub vertices: Vec<Vec2>,
	pub dynamic: bool,
	pub physics_only: bool,
}

impl PathDeformer {
	/// Number of curve control points.
	pub fn vertex_count(&self) -> usize {
		self.vertices.len()
	}
}

/* --- DRAWABLE --- */

/// If has this as a component, the node should render something
#[derive(Clone, Debug)]
pub struct Drawable {
	pub blending: Blending,
	/// If Some, the node should consider masking when rendering
	pub masks: Option<Masks>,
}

#[derive(Clone, Debug)]
pub struct Blending {
	pub mode: BlendMode,
	pub tint: Vec3,
	pub screen_tint: Vec3,
	pub opacity: f32,
}

#[derive(Default, PartialEq, Eq, Clone, Copy, Debug)]
pub enum BlendMode {
	/// Normal blending mode.
	#[default]
	Normal,
	/// Multiply.
	Multiply,
	/// Screen.
	Screen,
	/// Overlay.
	Overlay,
	/// Darken.
	Darken,
	/// Lighten.
	Lighten,
	/// ColorDodge.
	ColorDodge,
	/// LinearDodge.
	LinearDodge,
	/// AddGlow.
	AddGlow,
	/// ColorBurn.
	ColorBurn,
	/// HardLight.
	HardLight,
	/// SoftLight.
	SoftLight,
	/// Difference.
	Difference,
	/// Exclusion.
	Exclusion,
	/// Subtract.
	Subtract,
	/// Inverse.
	Inverse,
	/// DestinationIn.
	DestinationIn,
	/// Clip to Lower.
	/// Special blending mode that clips the drawable
	/// to a lower rendered area.
	ClipToLower,
	/// Slice from Lower.
	/// Special blending mode that slices the drawable
	/// via a lower rendered area.
	/// (Basically inverse ClipToLower.)
	SliceFromLower,
}

impl BlendMode {
	/// All blend modes known to the nijilive / Inochi2D family of formats.
	pub const VALUES: [BlendMode; 19] = [
		BlendMode::Normal,
		BlendMode::Multiply,
		BlendMode::Screen,
		BlendMode::Overlay,
		BlendMode::Darken,
		BlendMode::Lighten,
		BlendMode::ColorDodge,
		BlendMode::LinearDodge,
		BlendMode::AddGlow,
		BlendMode::ColorBurn,
		BlendMode::HardLight,
		BlendMode::SoftLight,
		BlendMode::Difference,
		BlendMode::Exclusion,
		BlendMode::Subtract,
		BlendMode::Inverse,
		BlendMode::DestinationIn,
		BlendMode::ClipToLower,
		BlendMode::SliceFromLower,
	];

	/// Name used in `.inp` files.
	pub fn as_str(self) -> &'static str {
		match self {
			BlendMode::Normal => "Normal",
			BlendMode::Multiply => "Multiply",
			BlendMode::Screen => "Screen",
			BlendMode::Overlay => "Overlay",
			BlendMode::Darken => "Darken",
			BlendMode::Lighten => "Lighten",
			BlendMode::ColorDodge => "ColorDodge",
			BlendMode::LinearDodge => "LinearDodge",
			BlendMode::AddGlow => "AddGlow",
			BlendMode::ColorBurn => "ColorBurn",
			BlendMode::HardLight => "HardLight",
			BlendMode::SoftLight => "SoftLight",
			BlendMode::Difference => "Difference",
			BlendMode::Exclusion => "Exclusion",
			BlendMode::Subtract => "Subtract",
			BlendMode::Inverse => "Inverse",
			BlendMode::DestinationIn => "DestinationIn",
			BlendMode::ClipToLower => "ClipToLower",
			BlendMode::SliceFromLower => "SliceFromLower",
		}
	}

	/// Parse an `.inp` blend mode name. Returns `None` for names we do not know.
	pub fn from_name(name: &str) -> Option<BlendMode> {
		match name {
			"Normal" => Some(BlendMode::Normal),
			"Multiply" => Some(BlendMode::Multiply),
			"Screen" => Some(BlendMode::Screen),
			"Overlay" => Some(BlendMode::Overlay),
			"Darken" => Some(BlendMode::Darken),
			"Lighten" => Some(BlendMode::Lighten),
			"ColorDodge" => Some(BlendMode::ColorDodge),
			"LinearDodge" => Some(BlendMode::LinearDodge),
			"AddGlow" => Some(BlendMode::AddGlow),
			"ColorBurn" => Some(BlendMode::ColorBurn),
			"HardLight" => Some(BlendMode::HardLight),
			"SoftLight" => Some(BlendMode::SoftLight),
			"Difference" => Some(BlendMode::Difference),
			"Exclusion" => Some(BlendMode::Exclusion),
			"Subtract" => Some(BlendMode::Subtract),
			"Inverse" => Some(BlendMode::Inverse),
			"DestinationIn" => Some(BlendMode::DestinationIn),
			"ClipToLower" => Some(BlendMode::ClipToLower),
			"SliceFromLower" => Some(BlendMode::SliceFromLower),
			_ => None,
		}
	}
}

#[derive(Clone, Debug)]
pub struct Masks {
	pub threshold: f32,
	pub masks: Vec<Mask>,
}

impl Masks {
	/// Checks whether has masks of mode `MaskMode::Mask`.
	pub fn has_masks(&self) -> bool {
		self.masks.iter().any(|mask| mask.mode == MaskMode::Mask)
	}

	/// Checks whether has masks of mode `MaskMode::Dodge`.
	pub fn has_dodge_masks(&self) -> bool {
		self.masks.iter().any(|mask| mask.mode == MaskMode::Dodge)
	}
}

#[derive(Clone, Debug)]
pub struct Mask {
	pub source: InoxNodeUuid,
	pub mode: MaskMode,
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum MaskMode {
	/// The part should be masked by the drawables specified.
	Mask,
	/// The path should be dodge-masked by the drawables specified.
	Dodge,
}

/* --- SIMPLE PHYSICS --- */

/// If has this as a component, the node is capable of doing Inochi2D SimplePhysics simulations
#[derive(Clone, Debug)]
pub struct SimplePhysics {
	pub param: ParamUuid,
	pub model_type: PhysicsModel,
	pub map_mode: PhysicsParamMapMode,
	pub props: PhysicsProps,
	/// Whether physics system listens to local transform only.
	pub local_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicsModel {
	RigidPendulum,
	SpringPendulum,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicsParamMapMode {
	AngleLength,
	LengthAngle,
	XY,
	YX,
}

#[derive(Clone, Debug)]
pub struct PhysicsProps {
	/// Gravity scale (1.0 = puppet gravity)
	pub gravity: f32,
	/// Pendulum/spring rest length (pixels)
	pub length: f32,
	/// Resonant frequency (Hz)
	pub frequency: f32,
	/// Angular damping ratio
	pub angle_damping: f32,
	/// Length damping ratio
	pub length_damping: f32,
	pub output_scale: Vec2,
}

impl Default for PhysicsProps {
	fn default() -> Self {
		Self {
			gravity: 1.,
			length: 1.,
			frequency: 1.,
			angle_damping: 0.5,
			length_damping: 0.5,
			output_scale: Vec2::ONE,
		}
	}
}

/// Physical states for simulating a rigid pendulum.
#[derive(Default)]
pub(crate) struct RigidPendulumCtx {
	pub bob: Vec2,
	pub state: PhysicsState<2, RigidPendulum>,
	pub initialized: bool,
}

/// Physical states for simulating a spring pendulum.
#[derive(Default)]
pub(crate) struct SpringPendulumCtx {
	pub state: PhysicsState<4, SpringPendulum>,
	pub initialized: bool,
}

/* --- TEXTURED MESH --- */

/// If has this as a component, the node should render a deformed texture
#[derive(Clone, Debug)]
pub struct TexturedMesh {
	pub tex_albedo: TextureId,
	pub tex_emissive: TextureId,
	pub tex_bumpmap: TextureId,
}

/* --- MESH --- */

/// A deformable mesh, deforming either textures (TexturedMesh nodes), or children (MeshGroup nodes)
#[derive(Clone, Debug)]
pub struct Mesh {
	/// Vertices in the mesh.
	pub vertices: Vec<Vec2>,
	/// Base UVs.
	pub uvs: Vec<Vec2>,
	/// Indices in the mesh.
	pub indices: Vec<u32>,
	/// Origin of the mesh.
	pub origin: Vec2,
}

/* --- DEFORM STACK --- */

/// Source of a deform.
#[derive(Hash, PartialEq, Eq, Copy, Clone)]
#[allow(unused)]
pub(crate) enum DeformSource {
	Param(ParamUuid),
	Node(InoxNodeUuid),
}

/// Internal component solving for deforms of a node.
/// Storing deforms specified by multiple sources to apply on one node for one frame.
///
/// Despite the name (this is respecting the ref impl), this is not in any way a stack.
/// The order of deforms being applied, or more generally speaking, the way multiple deforms adds up to be a single one, needs to be defined according to the spec.
pub(crate) struct DeformStack {
	/// this is a component so cannot use generics for the length.
	pub(crate) deform_len: usize,
	/// map of (src, (enabled, Deform)).
	/// On reset, only set enabled to false instead of clearing the map, as deforms from same sources tend to come in every frame.
	pub(crate) stack: std::collections::HashMap<DeformSource, (bool, Deform)>,
}

/* --- TRANSFORM STORE --- */

/// Internal component storing:
/// - Relative transform being determined in between frames.
/// - Absolute transform prepared from all relative transforms just before rendering.
#[derive(Default, Clone)]
pub struct TransformStore {
	pub absolute: Mat4,
	pub relative: TransformOffset,
}

/* --- ZSORT --- */

/// Component holding zsort values that may be modified across frames.
// only one value instead of absolute + relative as in TransformStore, cause inheritance of zsort (+) is commutative
#[derive(Default)]
pub struct ZSort(pub f32);

// so ZSort automatically gets the `.total_cmp()` of `f32`
impl std::ops::Deref for ZSort {
	type Target = f32;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

/* --- OPACITY STORE --- */

/// Component holding evaluated opacity values across frames.
#[derive(Default, Clone, Debug)]
pub struct OpacityStore(pub f32);

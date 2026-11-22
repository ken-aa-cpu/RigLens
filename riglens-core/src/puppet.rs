pub mod meta;
mod transforms;
mod tree;
mod world;

use std::collections::HashMap;

use glam::Vec2;

use crate::animation::PuppetAnimation;
use crate::math::transform::TransformOffset;
use crate::node::components::{DeformStack, OpacityStore, TransformStore, ZSort};
use crate::node::{InoxNode, InoxNodeUuid};
use crate::params::{Param, ParamCtx, ParamUuid};
use crate::physics::{PhysicsCtx, PuppetPhysics};
use crate::render::RenderCtx;
use crate::INOCHI2D_SPEC_VERSION;

use meta::PuppetMeta;
pub use transforms::TransformCtx;
pub use tree::InoxNodeTree;
pub use world::World;

/// Mutually exclusive part-visibility group (matching Cubism `.pose3.json` / PSD differential groups).
#[derive(Clone, Debug, PartialEq)]
pub struct PoseGroup {
	pub id: String,
	pub parts: Vec<InoxNodeUuid>,
	pub fade_seconds: f32,
}

/// Inochi2D puppet.
pub struct Puppet {
	pub meta: PuppetMeta,
	pub physics: PuppetPhysics,
	physics_ctx: Option<PhysicsCtx>,
	pub nodes: InoxNodeTree,
	pub node_comps: World,
	/// Currently only a marker for if transform/zsort components are initialized.
	pub transform_ctx: Option<TransformCtx>,
	/// Context for rendering this puppet. See `.init_rendering()`.
	pub render_ctx: Option<RenderCtx>,
	pub params: HashMap<String, Param>,
	/// Context for animating puppet with parameters. See `.init_params()`
	pub param_ctx: Option<ParamCtx>,
	/// Named animations stored in the puppet.
	pub animations: HashMap<String, PuppetAnimation>,
	/// Mutually exclusive part-visibility groups (evaluated at end of frame).
	pub pose_groups: Vec<PoseGroup>,
	/// Non-fatal problems found while loading (unknown node types, lossy conversions, ...).
	pub diagnostics: Vec<String>,
	/// Parameter names in file order (the `params` map itself is unordered).
	pub param_order: Vec<String>,
	/// Unmodelled per-parameter fields (e.g. `parentUUID`, `merge_mode`), keyed by parameter name.
	pub param_extras: HashMap<String, json::object::Object>,
	/// Unmodelled top-level keys (e.g. `groups`, `automation`), preserved verbatim.
	pub top_extras: json::object::Object,
}

impl Puppet {
	pub fn new(
		meta: PuppetMeta,
		physics: PuppetPhysics,
		root: InoxNode,
		params: HashMap<String, Param>,
	) -> Self {
		Self {
			meta,
			physics,
			physics_ctx: None,
			nodes: InoxNodeTree::new_with_root(root),
			node_comps: World::new(),
			transform_ctx: None,
			render_ctx: None,
			params,
			param_ctx: None,
			animations: HashMap::new(),
			pose_groups: Vec::new(),
			diagnostics: Vec::new(),
			param_order: Vec::new(),
			param_extras: HashMap::new(),
			top_extras: json::object::Object::new(),
		}
	}

	/// Create an empty puppet with a Root node and default metadata/physics.
	pub fn new_empty(name: impl Into<String>) -> Self {
		let meta = PuppetMeta {
			name: Some(name.into()),
			version: INOCHI2D_SPEC_VERSION.to_owned(),
			rigger: Some("inox2d-mcp".to_owned()),
			artist: Some("inox2d-mcp".to_owned()),
			rights: None,
			copyright: None,
			license_url: None,
			contact: None,
			reference: None,
			thumbnail_id: None,
			preserve_pixels: false,
		};
		let physics = PuppetPhysics {
			pixels_per_meter: 1000.0,
			gravity: 9.8,
		};
		let root = InoxNode {
			uuid: InoxNodeUuid(0),
			name: "Root".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset::default(),
			lock_to_root: false,
		};
		Self::new(meta, physics, root, HashMap::new())
	}

	/// Allocate the next unused node UUID.
	pub fn next_node_uuid(&self) -> InoxNodeUuid {
		let max_id = self.nodes.iter().map(|n| n.uuid.0).max().unwrap_or(0);
		InoxNodeUuid(max_id + 1)
	}

	/// Allocate the next unused parameter UUID.
	pub fn next_param_uuid(&self) -> ParamUuid {
		let max_id = self.params.values().map(|p| p.uuid.0).max().unwrap_or(0);
		ParamUuid(max_id + 1)
	}

	/// Add a child node under `parent` and return its UUID.
	pub fn add_node(&mut self, parent: InoxNodeUuid, node: InoxNode) -> InoxNodeUuid {
		let id = node.uuid;
		self.nodes.add(parent, id, node);
		id
	}

	/// Clear runtime evaluation contexts so the puppet can be re-initialized after edits.
	pub fn reset_contexts(&mut self) {
		self.physics_ctx = None;
		self.param_ctx = None;
		self.render_ctx = None;
		self.transform_ctx = None;
	}

	/// Initialize all runtime contexts (transforms, rendering, params, physics) idempotently.
	pub fn init_all(&mut self) {
		if self.transform_ctx.is_none() {
			self.init_transforms();
		}
		if self.render_ctx.is_none() {
			self.init_rendering();
		}
		if self.param_ctx.is_none() {
			self.init_params();
		}
		if self.physics_ctx.is_none() {
			self.init_physics();
		}
	}

	/// Get evaluated zsort for a node after `end_frame()`.
	pub fn node_zsort(&self, id: InoxNodeUuid) -> Option<f32> {
		self.node_comps.get::<ZSort>(id).map(|z| z.0)
	}

	/// Get evaluated TransformStore for a node after `end_frame()`.
	pub fn node_transform(&self, id: InoxNodeUuid) -> Option<&TransformStore> {
		self.node_comps.get::<TransformStore>(id)
	}

	/// Get evaluated OpacityStore for a node after `end_frame()`.
	pub fn node_opacity(&self, id: InoxNodeUuid) -> Option<f32> {
		self.node_comps.get::<OpacityStore>(id).map(|o| o.0)
	}

	/// Evaluate the current combined deform offsets for a node (`Mesh`, `MeshGroup`, `GridDeformer`, or `PathDeformer`)
	/// after `end_frame()`.
	pub fn node_deform_offsets(&self, id: InoxNodeUuid) -> Option<Vec<Vec2>> {
		let stack = self.node_comps.get::<DeformStack>(id)?;
		let mut out = vec![Vec2::ZERO; stack.deform_len];
		stack.combine(&self.nodes, &self.node_comps, &mut out);
		Some(out)
	}

	/// Create a copy of node transform/zsort for modification. Panicks on second call.
	pub fn init_transforms(&mut self) {
		if self.transform_ctx.is_some() {
			panic!("Puppet transforms already initialized.")
		}

		let transform_ctx = TransformCtx::new(self);
		self.transform_ctx = Some(transform_ctx);
	}

	/// Call this on a freshly loaded puppet if rendering is needed. Panicks:
	/// - if transforms are not initialized.
	/// - on second call.
	pub fn init_rendering(&mut self) {
		if self.transform_ctx.is_none() {
			panic!("Puppet rendering depends on initialized puppet transforms.");
		}
		if self.render_ctx.is_some() {
			panic!("Puppet already initialized for rendering.");
		}

		let render_ctx = RenderCtx::new(self);
		self.render_ctx = Some(render_ctx);
	}

	/// Call this on a puppet if params are going to be used. Panicks:
	/// - if rendering is not initialized.
	/// - on second call.
	pub fn init_params(&mut self) {
		if self.render_ctx.is_none() {
			panic!("Only a puppet initialized for rendering can be animated by params.");
		}
		if self.param_ctx.is_some() {
			panic!("Puppet already initialized for params.");
		}

		let param_ctx = ParamCtx::new(self);
		self.param_ctx = Some(param_ctx);
	}

	/// Call this on a puppet if physics are going to be simulated. Panicks:
	/// - if params is not initialized.
	/// - on second call.
	pub fn init_physics(&mut self) {
		if self.param_ctx.is_none() {
			panic!("Puppet physics depends on initialized puppet params.");
		}
		if self.physics_ctx.is_some() {
			panic!("Puppet already initialized for physics.");
		}

		let physics_ctx = PhysicsCtx::new(self);
		self.physics_ctx = Some(physics_ctx);
	}

	/// Prepare the puppet for a new frame. User may set params afterwards.
	pub fn begin_frame(&mut self) {
		if let Some(render_ctx) = self.render_ctx.as_mut() {
			render_ctx.reset(&self.nodes, &mut self.node_comps);
		}

		if let Some(transform_ctx) = self.transform_ctx.as_mut() {
			transform_ctx.reset(&self.nodes, &mut self.node_comps);
		}

		if let Some(param_ctx) = self.param_ctx.as_mut() {
			param_ctx.reset(&self.params);
		}
	}

	/// Freeze puppet for one frame. Rendering, if initialized, may follow.
	///
	/// Provide elapsed time for physics, if initialized, to run. Provide `0` for the first call.
	pub fn end_frame(&mut self, dt: f32) {
		if let Some(param_ctx) = self.param_ctx.as_mut() {
			param_ctx.apply(&self.params, &self.nodes, &mut self.node_comps);
		}

		if let Some(transform_ctx) = self.transform_ctx.as_mut() {
			transform_ctx.update(&self.nodes, &mut self.node_comps);
		}

		if let Some(physics_ctx) = self.physics_ctx.as_mut() {
			let values_to_apply = physics_ctx.step(&self.physics, &self.nodes, &mut self.node_comps, dt);

			// TODO: Think about separating DeformStack reset and RenderCtx reset?
			self.render_ctx
				.as_mut()
				.expect("If physics is initialized, so does params, so does rendering.")
				.reset(&self.nodes, &mut self.node_comps);

			// TODO: Fewer repeated calculations of a same transform?
			let transform_ctx = self
				.transform_ctx
				.as_mut()
				.expect("If physics is initialized, so does transforms.");
			transform_ctx.reset(&self.nodes, &mut self.node_comps);

			let param_ctx = self
				.param_ctx
				.as_mut()
				.expect("If physics is initialized, so does params.");
			for (param_name, value) in &values_to_apply {
				param_ctx
					.set(param_name, *value)
					.expect("Param name returned by .step() must exist.");
			}
			param_ctx.apply(&self.params, &self.nodes, &mut self.node_comps);

			transform_ctx.update(&self.nodes, &mut self.node_comps);
			physics_ctx.apply_path_deforms(&self.nodes, &mut self.node_comps);
		}

		self.apply_pose_groups();

		if let Some(render_ctx) = self.render_ctx.as_mut() {
			render_ctx.update(&self.nodes, &mut self.node_comps);
		}
	}

	fn apply_pose_groups(&mut self) {
		use crate::node::components::{Drawable, OpacityStore};
		for group in &self.pose_groups {
			if group.parts.len() < 2 {
				continue;
			}
			let mut best: Option<(InoxNodeUuid, f32)> = None;
			for &pid in &group.parts {
				if !self.nodes.is_effectively_enabled(pid) {
					continue;
				}
				let op = if self.node_comps.get::<Drawable>(pid).is_some() {
					self.node_comps.get::<OpacityStore>(pid).map(|o| o.0).unwrap_or(0.0)
				} else {
					let mut max_child_op = None::<f32>;
					for n in self.nodes.pre_order_iter() {
						if n.uuid != pid
							&& !n.lock_to_root
							&& self.nodes.is_descendant(n.uuid, pid)
							&& self.nodes.is_effectively_enabled(n.uuid)
							&& self.node_comps.get::<Drawable>(n.uuid).is_some()
						{
							let cop = self.node_comps.get::<OpacityStore>(n.uuid).map(|o| o.0).unwrap_or(0.0);
							max_child_op = Some(max_child_op.map_or(cop, |m| m.max(cop)));
						}
					}
					max_child_op.unwrap_or_else(|| {
						self.node_comps.get::<OpacityStore>(pid).map(|o| o.0).unwrap_or(0.0)
					})
				};
				if op > 0.01 && best.map(|(_, b)| op > b).unwrap_or(true) {
					best = Some((pid, op));
				}
			}
			if let Some((win_id, _)) = best {
				for &pid in &group.parts {
					if pid != win_id {
						let descendants: Vec<InoxNodeUuid> = self
							.nodes
							.pre_order_iter()
							.filter(|n| {
								n.uuid == pid || (!n.lock_to_root && self.nodes.is_descendant(n.uuid, pid))
							})
							.map(|n| n.uuid)
							.collect();
						for uid in descendants {
							if let Some(op) = self.node_comps.get_mut::<OpacityStore>(uid) {
								op.0 = 0.0;
							}
						}
					}
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::math::interp::InterpolateMode;
	use crate::math::matrix::Matrix2d;
	use crate::math::transform::TransformOffset;
	use crate::node::components::{BlendMode, Blending, Drawable};
	use crate::params::{AxisPoints, Binding, BindingValues, Param, ParamUuid};
	use glam::{vec2, Vec3};

	#[test]
	fn pose_groups_and_additive_params_evaluate_and_roundtrip_ir() {
		let mut puppet = Puppet::new_empty("PoseAdditive");
		let root = puppet.nodes.root_node_id;
		let a = InoxNodeUuid(10);
		let b = InoxNodeUuid(11);
		for (id, name, init_op) in [(a, "ArmA", 0.3_f32), (b, "ArmB", 0.2_f32)] {
			puppet.nodes.add(
				root,
				id,
				InoxNode {
					uuid: id,
					name: name.to_owned(),
					enabled: true,
					zsort: 0.0,
					trans_offset: TransformOffset::default(),
					lock_to_root: false,
				},
			);
			puppet.node_comps.add(
				id,
				Drawable {
					blending: Blending {
						mode: BlendMode::Normal,
						tint: Vec3::ONE,
						screen_tint: Vec3::ZERO,
						opacity: init_op,
					},
					masks: None,
				},
			);
		}
		puppet.pose_groups.push(PoseGroup {
			id: "Arms".to_owned(),
			parts: vec![a, b],
			fade_seconds: 0.1,
		});

		let vals = Matrix2d::from_grid_transposed(2, 1, |ix, _| if ix == 1 { 0.5 } else { 0.0 });
		let is_set = Matrix2d::from_grid_transposed(2, 1, |_, _| true);
		puppet.params.insert(
			"AddArmA".to_owned(),
			Param {
				uuid: ParamUuid(100),
				name: "AddArmA".to_owned(),
				is_vec2: false,
				min: vec2(0.0, 0.0),
				max: vec2(1.0, 0.0),
				defaults: vec2(0.0, 0.0),
				axis_points: AxisPoints {
					x: vec![0.0, 1.0],
					y: vec![0.0],
				},
				additive: true,
				bindings: vec![Binding {
					node: a,
					is_set,
					interpolate_mode: InterpolateMode::Linear,
					values: BindingValues::Opacity(vals),
				}],
			},
		);
		puppet.init_all();

		puppet.begin_frame();
		puppet.param_ctx.as_mut().unwrap().set("AddArmA", vec2(1.0, 0.0)).unwrap();
		puppet.end_frame(0.0);

		// Additive param adds +0.5 onto ArmA's initial 0.3 -> 0.8; PoseGroup suppresses ArmB to 0.0
		assert!((puppet.node_opacity(a).unwrap() - 0.8).abs() < 1e-4);
		assert!((puppet.node_opacity(b).unwrap() - 0.0).abs() < 1e-4);

		// Verify IR bridge roundtrip preserves additive and pose_groups
		let model = crate::model::Model {
			puppet,
			textures: Vec::new(),
			vendors: Vec::new(),
		};
		let ir = crate::formats::ir_bridge::model_to_ir(&model);
		assert!(ir.params[0].additive);
		assert_eq!(ir.pose_groups.len(), 1);
		let back = crate::formats::ir_bridge::ir_to_model(&ir).expect("ir_to_model");
		assert!(back.puppet.params["AddArmA"].additive);
		assert_eq!(back.puppet.pose_groups.len(), 1);
	}

	#[test]
	fn group_opacity_propagates_through_drawable_parent_and_drives_pose_group() {
		let mut puppet = Puppet::new_empty("GroupHierarchy");
		let root = puppet.nodes.root_node_id;
		let grp_a = InoxNodeUuid(20);
		let parent_part = InoxNodeUuid(21);
		let child_part = InoxNodeUuid(22);
		let grp_b = InoxNodeUuid(23);
		let b_part = InoxNodeUuid(24);

		for (parent, id, name) in [
			(root, grp_a, "GroupA"),
			(grp_a, parent_part, "ParentPart"),
			(parent_part, child_part, "ChildPart"),
			(root, grp_b, "GroupB"),
			(grp_b, b_part, "BPart"),
		] {
			puppet.nodes.add(
				parent,
				id,
				InoxNode {
					uuid: id,
					name: name.to_owned(),
					enabled: true,
					zsort: 0.0,
					trans_offset: TransformOffset::default(),
					lock_to_root: false,
				},
			);
		}
		for (id, op) in [(parent_part, 0.8_f32), (child_part, 0.5_f32), (b_part, 0.6_f32)] {
			puppet.node_comps.add(
				id,
				Drawable {
					blending: Blending {
						mode: BlendMode::Normal,
						tint: Vec3::ONE,
						screen_tint: Vec3::ZERO,
						opacity: op,
					},
					masks: None,
				},
			);
		}
		puppet.pose_groups.push(PoseGroup {
			id: "GroupSwitch".to_owned(),
			parts: vec![grp_a, grp_b],
			fade_seconds: 0.0,
		});

		// Parameter scales GroupA opacity to 0.5 and ParentPart additive SX by +0.25.
		puppet.params.insert(
			"FadeGroupA".to_owned(),
			Param {
				uuid: ParamUuid(200),
				name: "FadeGroupA".to_owned(),
				is_vec2: false,
				min: vec2(0.0, 0.0),
				max: vec2(1.0, 0.0),
				defaults: vec2(1.0, 0.0),
				axis_points: AxisPoints {
					x: vec![0.0, 1.0],
					y: vec![0.0],
				},
				additive: false,
				bindings: vec![Binding {
					node: grp_a,
					is_set: Matrix2d::from_grid_transposed(2, 1, |_, _| true),
					interpolate_mode: InterpolateMode::Linear,
					values: BindingValues::Opacity(Matrix2d::from_grid_transposed(2, 1, |ix, _| {
						if ix == 0 { 0.0 } else { 0.5 }
					})),
				}],
			},
		);
		puppet.init_all();

		// At val=1.0: GroupA effective opacity = 0.5 -> ParentPart = 0.8*0.5 = 0.4, ChildPart = 0.5*0.5 = 0.25.
		// GroupB's BPart has 0.6 > 0.4 so GroupB wins PoseGroup and suppresses GroupA descendants to 0.0.
		puppet.begin_frame();
		puppet.param_ctx.as_mut().unwrap().set("FadeGroupA", vec2(1.0, 0.0)).unwrap();
		puppet.end_frame(0.0);
		assert!((puppet.node_opacity(b_part).unwrap() - 0.6).abs() < 1e-4);
		assert!((puppet.node_opacity(parent_part).unwrap() - 0.0).abs() < 1e-4);
		assert!((puppet.node_opacity(child_part).unwrap() - 0.0).abs() < 1e-4);

		// Remove PoseGroup to check raw Group -> ParentPart -> ChildPart inheritance directly.
		puppet.pose_groups.clear();
		puppet.begin_frame();
		puppet.param_ctx.as_mut().unwrap().set("FadeGroupA", vec2(1.0, 0.0)).unwrap();
		puppet.end_frame(0.0);
		assert!((puppet.node_opacity(parent_part).unwrap() - 0.4).abs() < 1e-4);
		assert!((puppet.node_opacity(child_part).unwrap() - 0.25).abs() < 1e-4);
	}
}

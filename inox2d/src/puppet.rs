pub mod meta;
mod transforms;
mod tree;
mod world;

use std::collections::HashMap;

use crate::animation::PuppetAnimation;
use crate::math::transform::TransformOffset;
use crate::node::components::{OpacityStore, TransformStore, ZSort};
use crate::node::{InoxNode, InoxNodeUuid};
use crate::params::{Param, ParamCtx, ParamUuid};
use crate::physics::{PhysicsCtx, PuppetPhysics};
use crate::render::RenderCtx;
use crate::INOCHI2D_SPEC_VERSION;

use meta::PuppetMeta;
pub use transforms::TransformCtx;
pub use tree::InoxNodeTree;
pub use world::World;

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
		}

		if let Some(render_ctx) = self.render_ctx.as_mut() {
			render_ctx.update(&self.nodes, &mut self.node_comps);
		}
	}
}

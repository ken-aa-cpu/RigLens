pub mod pendulum;
pub(crate) mod runge_kutta;

use std::collections::HashMap;

use glam::{Vec2, Vec3};

use crate::math::deform::Deform;
use crate::node::components::{
	DeformSource, DeformStack, ExtraFields, PathDeformer, PhysicsModel, RigidPendulumCtx, SimplePhysics,
	SpringPendulumCtx, TransformStore,
};
use crate::params::ParamUuid;
use crate::puppet::{InoxNodeTree, Puppet, World};

/// Global physics parameters for the puppet.
#[derive(Clone, Debug)]
pub struct PuppetPhysics {
	pub pixels_per_meter: f32,
	pub gravity: f32,
}

type SimplePhysicsProps<'a> = (&'a PuppetPhysics, &'a SimplePhysics);

/// Components implementing this will be able to yield a parameter value every frame based on
/// - time history of transforms of the associated node.
/// - Physics simulation.
pub trait SimplePhysicsCtx {
	/// Type of input to the simulation.
	type Anchor;

	/// Convert node transform to input.
	fn calc_anchor(&self, props: &SimplePhysicsProps, transform: &TransformStore) -> Self::Anchor;
	/// Run one step of simulation given input.
	fn tick(&mut self, props: &SimplePhysicsProps, anchor: &Self::Anchor, t: f32, dt: f32);
	/// Convert simulation result into a parameter value to set.
	fn calc_output(&self, props: &SimplePhysicsProps, transform: &TransformStore, anchor: Self::Anchor) -> Vec2;
}

/// Auto implemented trait for all `impl SimplePhysicsCtx`.
trait SimplePhysicsCtxCommon {
	fn update(&mut self, props: &SimplePhysicsProps, transform: &TransformStore, t: f32, dt: f32) -> Vec2;
}

impl<T: SimplePhysicsCtx> SimplePhysicsCtxCommon for T {
	/// Run physics simulation for one frame given provided methods. Handle big `dt` problems.
	fn update(&mut self, props: &SimplePhysicsProps, transform: &TransformStore, t: f32, dt: f32) -> Vec2 {
		// Timestep is limited to 10 seconds.
		// If you're getting 0.1 FPS, you have bigger issues to deal with.
		let mut dt = dt.min(10.);

		let anchor = self.calc_anchor(props, transform);

		// Minimum physics timestep: 0.01s. If not satisfied, break simulation into steps.
		let mut t = t;
		while dt > 0. {
			self.tick(props, &anchor, t, dt.min(0.01));
			t += 0.01;
			dt -= 0.01;
		}

		self.calc_output(props, transform, anchor)
	}
}

/// Additional struct attached to a puppet for executing all physics nodes.
pub(crate) struct PhysicsCtx {
	/// Time since first simulation step.
	t: f32,
	param_uuid_to_name: HashMap<ParamUuid, String>,
}

impl PhysicsCtx {
	/// MODIFIES puppet. In addition to initializing self, installs physics contexts in the World of components
	pub fn new(puppet: &mut Puppet) -> Self {
		for node in puppet.nodes.iter() {
			if let Some(simple_physics) = puppet.node_comps.get::<SimplePhysics>(node.uuid) {
				match simple_physics.model_type {
					PhysicsModel::RigidPendulum => puppet.node_comps.set(node.uuid, RigidPendulumCtx::default()),
					PhysicsModel::SpringPendulum => puppet.node_comps.set(node.uuid, SpringPendulumCtx::default()),
				}
			}
			if let Some(path) = puppet.node_comps.get::<PathDeformer>(node.uuid) {
				let has_extra_phys = puppet
					.node_comps
					.get::<ExtraFields>(node.uuid)
					.map(|ef| ef.0.get("physics").is_some())
					.unwrap_or(false);
				if path.vertices.len() >= 2 && (path.physics_only || path.dynamic || has_extra_phys) {
					let ctx = PathPhysicsCtx::new(path.vertices.clone());
					puppet.node_comps.set(node.uuid, ctx);
				}
			}
		}

		Self {
			t: 0.,
			param_uuid_to_name: puppet.params.iter().map(|p| (p.1.uuid, p.0.to_owned())).collect(),
		}
	}

	pub fn step(
		&mut self,
		puppet_physics: &PuppetPhysics,
		nodes: &InoxNodeTree,
		comps: &mut World,
		dt: f32,
	) -> HashMap<String, Vec2> {
		let mut values_to_apply = HashMap::new();

		if !dt.is_finite() || dt <= 0. {
			return values_to_apply;
		}

		for node in nodes.iter() {
			if !nodes.is_effectively_enabled(node.uuid) {
				continue;
			}
			if let Some(transform) = comps.get::<TransformStore>(node.uuid).cloned() {
				if let Some(path_ctx) = comps.get_mut::<PathPhysicsCtx>(node.uuid) {
					path_ctx.update(&transform, dt);
				}
			}
			if let Some(simple_physics) = comps.get::<SimplePhysics>(node.uuid) {
				// before we use some Rust dark magic so that two components can be mutably borrowed at the same time,
				// need to clone to workaround comps ownership problem
				let simple_physics = simple_physics.clone();
				let props = &(puppet_physics, &simple_physics);
				let Some(transform) = comps.get::<TransformStore>(node.uuid).cloned() else {
					tracing::warn!(
						uuid = ?node.uuid,
						name = %node.name,
						"SimplePhysics node is missing TransformStore; skipping."
					);
					continue;
				};

				let param_value = if let Some(rigid_pendulum_ctx) = comps.get_mut::<RigidPendulumCtx>(node.uuid) {
					Some(rigid_pendulum_ctx.update(props, &transform, self.t, dt))
				} else if let Some(spring_pendulum_ctx) = comps.get_mut::<SpringPendulumCtx>(node.uuid) {
					Some(spring_pendulum_ctx.update(props, &transform, self.t, dt))
				} else {
					None
				};

				if let Some(param_value) = param_value {
					let safe_value = if param_value.is_finite() {
						param_value
					} else {
						tracing::warn!(
							uuid = ?node.uuid,
							name = %node.name,
							"SimplePhysics node produced non-finite output; resetting to zero."
						);
						Vec2::ZERO
					};

					if let Some(param_name) = self.param_uuid_to_name.get(&simple_physics.param) {
						if values_to_apply.contains_key(param_name) {
							tracing::warn!(
								uuid = ?node.uuid,
								name = %node.name,
								param = %param_name,
								"Multiple SimplePhysics nodes reference the same param; overwriting with latest."
							);
						}
						values_to_apply.insert(param_name.to_owned(), safe_value);
					} else {
						tracing::warn!(
							uuid = ?node.uuid,
							name = %node.name,
							param_uuid = ?simple_physics.param,
							"SimplePhysics node references a missing param; skipping."
						);
					}
				}
			}
		}

		self.t += dt;

		values_to_apply
	}

	/// Push dynamic chain offsets from [`PathPhysicsCtx`] into each node's [`DeformStack`].
	pub fn apply_path_deforms(&self, nodes: &InoxNodeTree, comps: &mut World) {
		for node in nodes.iter() {
			if !nodes.is_effectively_enabled(node.uuid) {
				continue;
			}
			let Some(offsets) = comps
				.get::<PathPhysicsCtx>(node.uuid)
				.map(|p| p.last_offsets.clone())
			else {
				continue;
			};
			if let Some(stack) = comps.get_mut::<DeformStack>(node.uuid) {
				if offsets.len() == stack.deform_len {
					stack.push(DeformSource::Node(node.uuid), Deform::Direct(offsets));
				}
			}
		}
	}
}

/// Damped Verlet / spring-chain solver for `PathDeformer` control points.
pub(crate) struct PathPhysicsCtx {
	rest_verts: Vec<Vec2>,
	positions: Vec<Vec2>,
	velocities: Vec<Vec2>,
	last_offsets: Vec<Vec2>,
	initialized: bool,
}

impl PathPhysicsCtx {
	pub fn new(rest_verts: Vec<Vec2>) -> Self {
		let n = rest_verts.len();
		Self {
			rest_verts,
			positions: vec![Vec2::ZERO; n],
			velocities: vec![Vec2::ZERO; n],
			last_offsets: vec![Vec2::ZERO; n],
			initialized: false,
		}
	}

	pub fn update(&mut self, transform: &TransformStore, dt: f32) {
		let n = self.rest_verts.len();
		if n < 2 || !dt.is_finite() || dt <= 0.0 {
			return;
		}
		let world_targets: Vec<Vec2> = self
			.rest_verts
			.iter()
			.map(|v| transform.absolute.transform_point3(Vec3::new(v.x, v.y, 0.0)).truncate())
			.collect();
		if !self.initialized {
			self.positions = world_targets;
			self.velocities.fill(Vec2::ZERO);
			self.last_offsets.fill(Vec2::ZERO);
			self.initialized = true;
			return;
		}

		let mut rem = dt.min(0.25);
		while rem > 0.0 {
			let sub_dt = rem.min(0.01);
			rem -= sub_dt;

			self.positions[0] = world_targets[0];
			self.velocities[0] = Vec2::ZERO;

			for i in 1..n {
				let spring_k = 52.0;
				let damp = 7.5;
				let force = (world_targets[i] - self.positions[i]) * spring_k;
				self.velocities[i] = (self.velocities[i] + force * sub_dt) * (-damp * sub_dt).exp();
				if !self.velocities[i].is_finite() {
					self.velocities[i] = Vec2::ZERO;
				}
				self.positions[i] += self.velocities[i] * sub_dt;

				// Soft segment-length preservation relative to previous chain node
				let seg_rest = (self.rest_verts[i] - self.rest_verts[i - 1]).length().max(1.0);
				let delta = self.positions[i] - self.positions[i - 1];
				let dist = delta.length();
				if dist > 1e-4 && dist.is_finite() {
					let target_dist = dist + (seg_rest - dist) * 0.65;
					self.positions[i] = self.positions[i - 1] + delta * (target_dist / dist);
				}
			}
		}

		let inv = transform.absolute.inverse();
		self.last_offsets[0] = Vec2::ZERO;
		for i in 1..n {
			let local_pos = inv
				.transform_point3(Vec3::new(self.positions[i].x, self.positions[i].y, 0.0))
				.truncate();
			let delta = local_pos - self.rest_verts[i];
			let max_r = (self.rest_verts[i] - self.rest_verts[0]).length().max(16.0) * 1.5;
			self.last_offsets[i] = if delta.is_finite() {
				delta.clamp_length_max(max_r)
			} else {
				Vec2::ZERO
			};
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use glam::{vec3, Mat4};
	use crate::math::transform::TransformOffset;
	use crate::node::components::{PhysicsParamMapMode, PhysicsProps};
	use crate::node::{InoxNode, InoxNodeUuid};

	fn make_node(uuid: u32, name: &str, enabled: bool) -> InoxNode {
		InoxNode {
			uuid: InoxNodeUuid(uuid),
			name: name.to_owned(),
			enabled,
			zsort: 0.0,
			trans_offset: TransformOffset::default(),
			lock_to_root: false,
		}
	}

	fn make_physics(param: u32, model_type: PhysicsModel) -> SimplePhysics {
		SimplePhysics {
			param: ParamUuid(param),
			model_type,
			map_mode: PhysicsParamMapMode::XY,
			props: PhysicsProps {
				gravity: 1.0,
				length: 100.0,
				frequency: 2.0,
				angle_damping: 0.6,
				length_damping: 0.6,
				output_scale: Vec2::ONE,
			},
			local_only: false,
		}
	}

	fn transform_at(pos: Vec2) -> TransformStore {
		TransformStore {
			absolute: Mat4::from_translation(vec3(pos.x, pos.y, 0.0)),
			relative: TransformOffset {
				translation: vec3(pos.x, pos.y, 0.0),
				..Default::default()
			},
		}
	}

	#[test]
	fn rest_stability_for_rigid_and_spring_pendulum() {
		let pp = PuppetPhysics {
			pixels_per_meter: 1000.0,
			gravity: 9.8,
		};
		for anchor in [Vec2::ZERO, Vec2::new(150.0, -45.0)] {
			let transform = transform_at(anchor);
			let rigid_props = make_physics(1, PhysicsModel::RigidPendulum);
			let mut rigid_ctx = RigidPendulumCtx::default();
			for frame in 0..60 {
				let out = rigid_ctx.update(&(&pp, &rigid_props), &transform, frame as f32 / 60.0, 1.0 / 60.0);
				assert!(out.is_finite(), "rigid output non-finite at frame {frame}");
				assert!(
					out.length() < 1e-4,
					"rigid pendulum jumped at rest (anchor={anchor:?}, frame={frame}, out={out:?})"
				);
			}

			let spring_props = make_physics(2, PhysicsModel::SpringPendulum);
			let mut spring_ctx = SpringPendulumCtx::default();
			for frame in 0..60 {
				let out = spring_ctx.update(&(&pp, &spring_props), &transform, frame as f32 / 60.0, 1.0 / 60.0);
				assert!(out.is_finite(), "spring output non-finite at frame {frame}");
				assert!(
					out.length() < 1e-4,
					"spring pendulum jumped at rest (anchor={anchor:?}, frame={frame}, out={out:?})"
				);
			}
		}
	}

	#[test]
	fn displaced_anchor_oscillates_and_damps_toward_rest() {
		let pp = PuppetPhysics {
			pixels_per_meter: 1000.0,
			gravity: 9.8,
		};
		let dt = 1.0 / 60.0;
		for model in [PhysicsModel::RigidPendulum, PhysicsModel::SpringPendulum] {
			let props = make_physics(1, model);
			let mut rigid = RigidPendulumCtx::default();
			let mut spring = SpringPendulumCtx::default();

			let step_model = |t_transform: &TransformStore,
			                  frame: usize,
			                  r: &mut RigidPendulumCtx,
			                  s: &mut SpringPendulumCtx| {
				match model {
					PhysicsModel::RigidPendulum => r.update(&(&pp, &props), t_transform, frame as f32 * dt, dt),
					PhysicsModel::SpringPendulum => s.update(&(&pp, &props), t_transform, frame as f32 * dt, dt),
				}
			};

			// First settle at origin
			let t0 = transform_at(Vec2::ZERO);
			step_model(&t0, 0, &mut rigid, &mut spring);

			// Displace anchor right by 40px -> bob trails left relative to anchor
			let t1 = transform_at(Vec2::new(40.0, 0.0));
			let first = step_model(&t1, 1, &mut rigid, &mut spring);
			assert!(first.is_finite() && first.x.abs() > 0.01, "{model:?} expected sway, got {first:?}");

			// Hold steady at new anchor and confirm damping settles cleanly
			let mut last = first;
			for frame in 2..240 {
				last = step_model(&t1, frame, &mut rigid, &mut spring);
				assert!(last.is_finite(), "{model:?} became non-finite at frame {frame}");
			}
			assert!(
				last.length() < 0.05,
				"{model:?} failed to damp toward rest: {last:?}"
			);
		}
	}

	#[test]
	fn negative_gravity_and_zero_length_produce_finite_outputs() {
		let pp = PuppetPhysics {
			pixels_per_meter: 1000.0,
			gravity: -9.8,
		};
		let singular_transform = TransformStore {
			absolute: Mat4::ZERO,
			relative: TransformOffset::default(),
		};

		for model in [PhysicsModel::RigidPendulum, PhysicsModel::SpringPendulum] {
			for length in [80.0_f32, 0.0_f32] {
				let mut props = make_physics(1, model);
				props.props.length = length;
				props.props.gravity = -1.0;

				let mut rigid = RigidPendulumCtx::default();
				let mut spring = SpringPendulumCtx::default();
				for frame in 0..30 {
					let out = match model {
						PhysicsModel::RigidPendulum => {
							rigid.update(&(&pp, &props), &singular_transform, frame as f32 / 60.0, 1.0 / 60.0)
						}
						PhysicsModel::SpringPendulum => {
							spring.update(&(&pp, &props), &singular_transform, frame as f32 / 60.0, 1.0 / 60.0)
						}
					};
					assert!(
						out.is_finite(),
						"non-finite output for {model:?} with negative gravity and length={length}: {out:?}"
					);
				}
			}
		}
	}

	#[test]
	fn step_skips_disabled_dangling_and_overwrites_duplicate_physics_nodes() {
		let pp = PuppetPhysics {
			pixels_per_meter: 1000.0,
			gravity: 9.8,
		};
		let mut tree = InoxNodeTree::new_with_root(make_node(0, "Root", true));
		let mut comps = World::new();

		// 1: Disabled physics node targeting ParamUuid(1)
		tree.add(InoxNodeUuid(0), InoxNodeUuid(1), make_node(1, "DisabledPhys", false));
		comps.set(InoxNodeUuid(1), make_physics(1, PhysicsModel::RigidPendulum));
		comps.set(InoxNodeUuid(1), transform_at(Vec2::new(50.0, 0.0)));
		comps.set(InoxNodeUuid(1), RigidPendulumCtx::default());

		// 2: Dangling param UUID (999 not registered)
		tree.add(InoxNodeUuid(0), InoxNodeUuid(2), make_node(2, "DanglingParam", true));
		comps.set(InoxNodeUuid(2), make_physics(999, PhysicsModel::SpringPendulum));
		comps.set(InoxNodeUuid(2), transform_at(Vec2::ZERO));
		comps.set(InoxNodeUuid(2), SpringPendulumCtx::default());

		// 3: Missing TransformStore
		tree.add(InoxNodeUuid(0), InoxNodeUuid(3), make_node(3, "MissingTransform", true));
		comps.set(InoxNodeUuid(3), make_physics(1, PhysicsModel::RigidPendulum));
		comps.set(InoxNodeUuid(3), RigidPendulumCtx::default());

		// 4 & 5: Two active physics nodes targeting the same param ParamUuid(1)
		tree.add(InoxNodeUuid(0), InoxNodeUuid(4), make_node(4, "DupA", true));
		comps.set(InoxNodeUuid(4), make_physics(1, PhysicsModel::RigidPendulum));
		comps.set(InoxNodeUuid(4), transform_at(Vec2::ZERO));
		comps.set(InoxNodeUuid(4), RigidPendulumCtx::default());

		tree.add(InoxNodeUuid(0), InoxNodeUuid(5), make_node(5, "DupB", true));
		comps.set(InoxNodeUuid(5), make_physics(1, PhysicsModel::SpringPendulum));
		comps.set(InoxNodeUuid(5), transform_at(Vec2::new(10.0, 0.0)));
		comps.set(InoxNodeUuid(5), SpringPendulumCtx::default());

		let mut map = HashMap::new();
		map.insert(ParamUuid(1), "Hair".to_owned());
		let mut ctx = PhysicsCtx {
			t: 0.0,
			param_uuid_to_name: map,
		};

		// Non-positive and NaN dt must not panic
		assert!(ctx.step(&pp, &tree, &mut comps, -0.016).is_empty());
		assert!(ctx.step(&pp, &tree, &mut comps, f32::NAN).is_empty());

		let out = ctx.step(&pp, &tree, &mut comps, 1.0 / 60.0);
		assert_eq!(out.len(), 1);
		let val = out.get("Hair").copied().expect("Hair param output present");
		assert!(val.is_finite());

		// Disabled node should never have been ticked/initialized
		let disabled_ctx = comps.get::<RigidPendulumCtx>(InoxNodeUuid(1)).unwrap();
		assert!(!disabled_ctx.initialized);
	}

	#[test]
	fn path_deformer_chain_physics_sways_on_motion_and_settles() {
		let mut puppet = Puppet::new_empty("PathPhys");
		let root = puppet.nodes.root_node_id;
		let path_id = InoxNodeUuid(20);
		puppet.nodes.add(root, path_id, make_node(20, "HairPath", true));
		puppet.node_comps.add(
			path_id,
			PathDeformer {
				curve_type: crate::math::curve::CurveType::Spline,
				vertices: vec![Vec2::ZERO, Vec2::new(0.0, 40.0), Vec2::new(0.0, 80.0)],
				dynamic: true,
				physics_only: true,
			},
		);
		puppet.init_all();

		// First frame initializes chain at rest with zero offset
		puppet.begin_frame();
		puppet.end_frame(1.0 / 60.0);
		let off0 = puppet.node_deform_offsets(path_id).expect("PathDeformer stack");
		assert!(off0.iter().all(|v| v.length() < 1e-5));

		// Displace root anchor -> chain lags behind with finite non-zero sway on tips
		puppet.nodes.get_node_mut(path_id).unwrap().trans_offset.translation.x = 24.0;
		puppet.begin_frame();
		puppet.end_frame(1.0 / 60.0);
		let off1 = puppet.node_deform_offsets(path_id).expect("PathDeformer stack");
		assert!(off1[0].length() < 1e-5, "root control point must stay anchored");
		assert!(off1[2].is_finite() && off1[2].x.abs() > 0.5, "tip should sway: {:?}", off1);

		// Hold anchor stationary for several seconds -> chain settles back toward rest
		for _ in 0..180 {
			puppet.begin_frame();
			puppet.end_frame(1.0 / 60.0);
		}
		let settled = puppet.node_deform_offsets(path_id).unwrap();
		assert!(settled[2].length() < 0.5, "expected settled sway, got {:?}", settled[2]);
	}
}

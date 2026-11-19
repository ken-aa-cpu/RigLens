pub mod rigid;
pub mod spring;

use std::f32::consts::PI;

use glam::{Vec2, Vec4};

use crate::node::components::{PhysicsParamMapMode, TransformStore};

use super::{SimplePhysicsCtx, SimplePhysicsProps};

/// All pendulum-like physical systems have a bob.
///
/// For such systems, auto implement input and parameter mapping for `SimplePhysicsCtx`.
trait Pendulum {
	fn get_bob(&self) -> Vec2;
	fn set_bob(&mut self, bob: Vec2);
	fn is_initialized(&self) -> bool;
	fn mark_initialized(&mut self);
	/// Compute new anchor position give current anchor and time.
	fn tick(&mut self, props: &SimplePhysicsProps, anchor: Vec2, t: f32, dt: f32) -> Vec2;
}

impl<T: Pendulum> SimplePhysicsCtx for T {
	type Anchor = Vec2;

	fn calc_anchor(&self, props: &SimplePhysicsProps, transform: &TransformStore) -> Vec2 {
		let anchor = match props.1.local_only {
			true => transform.relative.translation.extend(1.0),
			false => transform.absolute * Vec4::new(0.0, 0.0, 0.0, 1.0),
		};

		Vec2::new(anchor.x, anchor.y)
	}

	fn tick(&mut self, props: &SimplePhysicsProps, anchor: &Vec2, t: f32, dt: f32) {
		if !anchor.is_finite() {
			return;
		}
		if !self.is_initialized() || !self.get_bob().is_finite() {
			let length = if props.1.props.length.is_finite() {
				props.1.props.length
			} else {
				0.0
			};
			self.set_bob(*anchor + Vec2::new(0.0, length));
			self.mark_initialized();
		}
		let bob = Pendulum::tick(self, props, *anchor, t, dt);
		if bob.is_finite() {
			self.set_bob(bob);
		}
	}

	fn calc_output(&self, props: &SimplePhysicsProps, transform: &TransformStore, anchor: Vec2) -> Vec2 {
		let oscale = props.1.props.output_scale;
		let bob = self.get_bob();
		let length = props.1.props.length;

		if !bob.is_finite()
			|| !anchor.is_finite()
			|| !length.is_finite()
			|| length.abs() <= 1e-6
			|| !oscale.is_finite()
		{
			return Vec2::ZERO;
		}

		// "Okay, so this is confusing. We want to translate the angle back to local space, but not the coordinates."
		// - Asahi Lina

		// Transform the physics output back into local space.
		// The origin here is the anchor. This gives us the local angle.
		let local_pos4 = match props.1.local_only {
			true => Vec4::new(bob.x - anchor.x, bob.y - anchor.y, 0.0, 1.0),
			false => {
				let inv = transform.absolute.inverse();
				if inv.is_finite() {
					inv * Vec4::new(bob.x, bob.y, 0.0, 1.0)
				} else {
					Vec4::new(bob.x - anchor.x, bob.y - anchor.y, 0.0, 1.0)
				}
			}
		};

		let local_angle = Vec2::new(local_pos4.x, local_pos4.y)
			.try_normalize()
			.unwrap_or(Vec2::Y);

		// Figure out the relative length. We can work this out directly in global space.
		let relative_length = bob.distance(anchor) / length;
		let relative_length = if relative_length.is_finite() {
			relative_length
		} else {
			1.0
		};

		let param_value = match props.1.map_mode {
			PhysicsParamMapMode::XY => {
				let local_pos_norm = local_angle * relative_length;
				let mut result = local_pos_norm - Vec2::Y;
				result.y = -result.y; // Y goes up for params
				result
			}
			PhysicsParamMapMode::YX => {
				let local_pos_norm = local_angle * relative_length;
				let mut result = local_pos_norm - Vec2::Y;
				result.y = -result.y; // Y goes up for params

				use glam::Vec2Swizzles;
				result.yx()
			}
			PhysicsParamMapMode::AngleLength => {
				let a = f32::atan2(-local_angle.x, local_angle.y) / PI;
				Vec2::new(a, relative_length)
			}
			PhysicsParamMapMode::LengthAngle => {
				let a = f32::atan2(-local_angle.x, local_angle.y) / PI;
				Vec2::new(relative_length, a)
			},
		};

		let out = param_value * oscale;
		if out.is_finite() {
			out
		} else {
			Vec2::ZERO
		}
	}
}

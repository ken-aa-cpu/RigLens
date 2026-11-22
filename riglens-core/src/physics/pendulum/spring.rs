use std::f32::consts::PI;

use glam::{vec2, Vec2};

use crate::node::components::{PhysicsProps, SpringPendulumCtx};
use crate::physics::{
	pendulum::Pendulum,
	runge_kutta::{IsPhysicsVars, PhysicsState},
	PuppetPhysics, SimplePhysicsProps,
};

/// Variables for Runge-Kutta method.
#[derive(Default)]
pub(crate) struct SpringPendulum {
	pub bob_pos: Vec2,
	pub bob_vel: Vec2,
}

impl IsPhysicsVars<4> for SpringPendulum {
	fn get_f32s(&self) -> [f32; 4] {
		[self.bob_pos.x, self.bob_pos.y, self.bob_vel.x, self.bob_vel.y]
	}

	fn set_f32s(&mut self, f32s: [f32; 4]) {
		[self.bob_pos.x, self.bob_pos.y, self.bob_vel.x, self.bob_vel.y] = f32s;
	}
}

impl Pendulum for SpringPendulumCtx {
	fn get_bob(&self) -> Vec2 {
		self.state.vars.bob_pos
	}

	fn set_bob(&mut self, bob: Vec2) {
		self.state.vars.bob_pos = bob;
	}

	fn is_initialized(&self) -> bool {
		self.initialized
	}

	fn mark_initialized(&mut self) {
		self.initialized = true;
	}

	fn tick(&mut self, props: &SimplePhysicsProps, anchor: Vec2, t: f32, dt: f32) -> Vec2 {
		let length = if props.1.props.length.is_finite() {
			props.1.props.length
		} else {
			0.0
		};
		if !self.state.vars.bob_pos.is_finite() || !self.state.vars.bob_vel.is_finite() {
			self.state.vars.bob_pos = anchor + Vec2::new(0.0, length);
			self.state.vars.bob_vel = Vec2::ZERO;
		}

		// Run the spring pendulum simulation
		self.state.tick(&eval, (props.0, &props.1.props), &anchor, t, dt);

		if !self.state.vars.bob_pos.is_finite() || !self.state.vars.bob_vel.is_finite() {
			self.state.vars.bob_pos = anchor + Vec2::new(0.0, length);
			self.state.vars.bob_vel = Vec2::ZERO;
		}

		self.state.vars.bob_pos
	}
}

/// Acceleration of bob caused by both
/// - gravity.
/// - damped oscillation of the spring-bob system in the radial direction.
fn eval(
	state: &mut PhysicsState<4, SpringPendulum>,
	&(puppet_physics, props): &(&PuppetPhysics, &PhysicsProps),
	anchor: &Vec2,
	_t: f32,
) {
	if !state.vars.bob_pos.is_finite() || !state.vars.bob_vel.is_finite() || !anchor.is_finite() {
		state.derivatives.bob_pos = Vec2::ZERO;
		state.derivatives.bob_vel = Vec2::ZERO;
		return;
	}

	state.derivatives.bob_pos = state.vars.bob_vel;

	// These are normalized vs. mass
	let spring_ksqrt = props.frequency * 2. * PI;
	let spring_k = spring_ksqrt.powi(2);

	let g = props.gravity * puppet_physics.pixels_per_meter * puppet_physics.gravity;
	if !props.frequency.is_finite()
		|| !spring_ksqrt.is_finite()
		|| !spring_k.is_finite()
		|| spring_k.abs() <= 1e-6
		|| !g.is_finite()
		|| !props.length.is_finite()
		|| props.length.abs() <= 1e-6
		|| !props.angle_damping.is_finite()
		|| !props.length_damping.is_finite()
	{
		state.derivatives.bob_pos = Vec2::ZERO;
		state.derivatives.bob_vel = Vec2::ZERO;
		return;
	}

	let rest_length = props.length - g / spring_k;

	let off_pos = state.vars.bob_pos - *anchor;
	let off_pos_norm = off_pos.try_normalize().unwrap_or(Vec2::Y);

	let length_ratio = (g / props.length).max(0.0);
	let crit_damp_angle = 2. * length_ratio.sqrt();
	let crit_damp_length = 2. * spring_ksqrt.abs();

	let dist = anchor.distance(state.vars.bob_pos).abs();
	let force = vec2(0., g) - (off_pos_norm * (dist - rest_length) * spring_k);

	let d_bob = state.vars.bob_vel;
	let d_bob_rot = vec2(
		d_bob.x * off_pos_norm.y + d_bob.y * off_pos_norm.x,
		d_bob.y * off_pos_norm.y - d_bob.x * off_pos_norm.x,
	);

	let dd_bob_rot = -vec2(
		d_bob_rot.x * props.angle_damping * crit_damp_angle,
		d_bob_rot.y * props.length_damping * crit_damp_length,
	);

	let dd_bob_damping = vec2(
		dd_bob_rot.x * off_pos_norm.y - dd_bob_rot.y * off_pos_norm.x,
		dd_bob_rot.y * off_pos_norm.y + dd_bob_rot.x * off_pos_norm.x,
	);

	let dd_bob = force + dd_bob_damping;

	state.derivatives.bob_vel = if dd_bob.is_finite() { dd_bob } else { Vec2::ZERO };
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::physics::PuppetPhysics;

	#[test]
	fn spring_pendulum_damping_opposes_horizontal_velocity() {
		let mut state = PhysicsState::<4, SpringPendulum>::default();
		let anchor = vec2(0.0, 0.0);
		state.vars.bob_pos = vec2(0.0, 100.0);
		state.vars.bob_vel = vec2(50.0, 0.0);
		let props = PhysicsProps {
			gravity: 1.0,
			length: 100.0,
			frequency: 2.0,
			angle_damping: 0.8,
			length_damping: 0.8,
			output_scale: vec2(1.0, 1.0),
		};
		let puppet_physics = PuppetPhysics {
			pixels_per_meter: 1000.0,
			gravity: 9.8,
		};
		eval(&mut state, &(&puppet_physics, &props), &anchor, 0.0);
		assert!(
			state.derivatives.bob_vel.x < 0.0,
			"expected horizontal damping deceleration (< 0), got {}",
			state.derivatives.bob_vel.x
		);
	}
}

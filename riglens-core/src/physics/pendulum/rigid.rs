use glam::{vec2, Vec2};

use crate::node::components::{PhysicsProps, RigidPendulumCtx};
use crate::physics::{
	pendulum::Pendulum,
	runge_kutta::{IsPhysicsVars, PhysicsState},
	PuppetPhysics, SimplePhysicsProps,
};

/// Variables for Runge-Kutta method.
#[derive(Default)]
pub(crate) struct RigidPendulum {
	pub θ: f32,
	pub ω: f32,
}

impl IsPhysicsVars<2> for RigidPendulum {
	fn get_f32s(&self) -> [f32; 2] {
		[self.θ, self.ω]
	}

	fn set_f32s(&mut self, f32s: [f32; 2]) {
		[self.θ, self.ω] = f32s;
	}
}

impl Pendulum for RigidPendulumCtx {
	fn get_bob(&self) -> Vec2 {
		self.bob
	}

	fn set_bob(&mut self, bob: Vec2) {
		self.bob = bob;
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

		// Compute the angle against the updated anchor position
		let d_bob = self.bob - anchor;
		self.state.vars.θ = if d_bob.length_squared() > 1e-12 {
			f32::atan2(-d_bob.x, d_bob.y)
		} else {
			0.0
		};
		if !self.state.vars.ω.is_finite() {
			self.state.vars.ω = 0.0;
		}

		// Run the pendulum simulation in terms of angle
		self.state.tick(&eval, (props.0, &props.1.props), &anchor, t, dt);

		if !self.state.vars.θ.is_finite() || !self.state.vars.ω.is_finite() {
			self.state.vars.θ = 0.0;
			self.state.vars.ω = 0.0;
		}

		// Update the bob position at the new angle
		let angle = self.state.vars.θ;
		let d_bob = vec2(-angle.sin(), angle.cos());

		anchor + d_bob * length
	}
}

/// Acceleration of bob caused by gravity.
fn eval(
	state: &mut PhysicsState<2, RigidPendulum>,
	(puppet_physics, props): &(&PuppetPhysics, &PhysicsProps),
	_anchor: &Vec2,
	_t: f32,
) {
	// https://www.myphysicslab.com/pendulum/pendulum-en.html

	let g = props.gravity * puppet_physics.pixels_per_meter * puppet_physics.gravity;
	let r = props.length;

	if !g.is_finite()
		|| !r.is_finite()
		|| r.abs() <= 1e-6
		|| !props.angle_damping.is_finite()
		|| !state.vars.θ.is_finite()
		|| !state.vars.ω.is_finite()
	{
		state.derivatives.θ = 0.0;
		state.derivatives.ω = 0.0;
		return;
	}

	// θ' = ω
	state.derivatives.θ = state.vars.ω;

	// ω' = -(g/R) sin θ
	let dω = -(g / r) * state.vars.θ.sin();

	// critical damp: that way a damping value of 1 corresponds to no bouncing
	let crit_damp = 2. * (g / r).max(0.0).sqrt();

	let damping = -state.vars.ω * props.angle_damping * crit_damp;

	let deriv_ω = dω + damping;
	state.derivatives.ω = if deriv_ω.is_finite() { deriv_ω } else { 0.0 };
}

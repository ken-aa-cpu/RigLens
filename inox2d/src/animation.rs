use std::collections::HashMap;

use glam::Vec2;

use crate::math::interp::InterpolateMode;
use crate::params::{Param, ParamUuid};

#[derive(Clone, Debug)]
pub struct Keyframe {
	pub frame: i32,
	pub value: f32,
	pub tension: f32,
}

#[derive(Clone, Debug)]
pub struct AnimationLane {
	pub interpolation: InterpolateMode,
	pub param_uuid: ParamUuid,
	pub target: u8,
	pub keyframes: Vec<Keyframe>,
	pub merge_mode: String,
}

#[derive(Clone, Debug)]
pub struct PuppetAnimation {
	pub timestep: f32,
	pub additive: bool,
	pub length: i32,
	pub lead_in: i32,
	pub lead_out: i32,
	pub lanes: Vec<AnimationLane>,
}

impl AnimationLane {
	pub fn eval_at(&self, frame: f32) -> Option<f32> {
		if self.keyframes.is_empty() {
			return None;
		}
		if self.keyframes.len() == 1 || frame <= self.keyframes[0].frame as f32 {
			return Some(self.keyframes[0].value);
		}
		let last = &self.keyframes[self.keyframes.len() - 1];
		if frame >= last.frame as f32 {
			return Some(last.value);
		}
		for win in self.keyframes.windows(2) {
			let a = &win[0];
			let b = &win[1];
			let af = a.frame as f32;
			let bf = b.frame as f32;
			if frame >= af && frame <= bf {
				let span = bf - af;
				if span.abs() < 1e-6 {
					return Some(a.value);
				}
				let t = (frame - af) / span;
				return Some(match self.interpolation {
					InterpolateMode::Nearest => {
						if t < 0.5 {
							a.value
						} else {
							b.value
						}
					}
					InterpolateMode::Linear => {
						// Smooth Hermite-like curve when tension != 0.5, or standard linear
						let s = if (a.tension - 0.5).abs() > 1e-3 {
							let t2 = t * t;
							let t3 = t2 * t;
							3.0 * t2 - 2.0 * t3
						} else {
							t
						};
						a.value + (b.value - a.value) * s
					}
				});
			}
		}
		Some(last.value)
	}
}

impl PuppetAnimation {
	/// Evaluate parameter values at a given animation frame (supports looping over `length`).
	pub fn eval_at_frame(&self, frame: f32, params: &HashMap<String, Param>) -> HashMap<String, Vec2> {
		let mut uuid_to_param: HashMap<ParamUuid, (&str, Vec2)> = HashMap::new();
		for (name, p) in params {
			uuid_to_param.insert(p.uuid, (name.as_str(), p.defaults));
		}

		let len = self.length.max(1) as f32;
		let wrapped = frame.rem_euclid(len);

		let mut out: HashMap<String, Vec2> = HashMap::new();
		for lane in &self.lanes {
			let Some(&(name, default_val)) = uuid_to_param.get(&lane.param_uuid) else {
				continue;
			};
			let Some(v) = lane.eval_at(wrapped) else {
				continue;
			};
			let entry = out.entry(name.to_owned()).or_insert(default_val);
			if lane.target == 0 {
				entry.x = v;
			} else {
				entry.y = v;
			}
		}
		out
	}
}

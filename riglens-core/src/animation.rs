use std::collections::HashMap;

use glam::Vec2;

use crate::math::interp::{cubic, hermite, InterpolateMode};
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
		for i in 1..self.keyframes.len() {
			let curr = &self.keyframes[i - 1];
			let next1 = &self.keyframes[i];
			let af = curr.frame as f32;
			let bf = next1.frame as f32;
			if frame >= af && frame <= bf {
				let span = bf - af;
				if span.abs() < 1e-6 {
					return Some(curr.value);
				}
				let t = (frame - af) / span;
				return Some(match self.interpolation {
					InterpolateMode::Stepped => curr.value,
					InterpolateMode::Nearest => {
						if t > 0.5 {
							next1.value
						} else {
							curr.value
						}
					}
					InterpolateMode::Linear => curr.value + (next1.value - curr.value) * t,
					InterpolateMode::Cubic => {
						let prev = self.keyframes[i.saturating_sub(2)].value;
						let next2 = self.keyframes[(i + 1).min(self.keyframes.len() - 1)].value;
						cubic(prev, curr.value, next1.value, next2, t)
					}
					InterpolateMode::Bezier => {
						let tension = next1.tension;
						let s = hermite(0.0, 2.0 * tension, 1.0, 2.0 * tension, t).clamp(0.0, 1.0);
						curr.value + (next1.value - curr.value) * s
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

#[cfg(test)]
mod tests {
	use super::*;

	fn lane(interpolation: InterpolateMode, kfs: &[(i32, f32, f32)]) -> AnimationLane {
		AnimationLane {
			interpolation,
			param_uuid: ParamUuid(1),
			target: 0,
			merge_mode: "Weighted".to_owned(),
			keyframes: kfs
				.iter()
				.map(|&(frame, value, tension)| Keyframe { frame, value, tension })
				.collect(),
		}
	}

	#[test]
	fn stepped_nearest_and_linear_eval() {
		let s = lane(InterpolateMode::Stepped, &[(0, 1.0, 0.5), (10, 5.0, 0.5)]);
		assert_eq!(s.eval_at(0.0), Some(1.0));
		assert_eq!(s.eval_at(7.5), Some(1.0));
		assert_eq!(s.eval_at(10.0), Some(5.0));

		let n = lane(InterpolateMode::Nearest, &[(0, 1.0, 0.5), (10, 5.0, 0.5)]);
		assert_eq!(n.eval_at(4.0), Some(1.0));
		assert_eq!(n.eval_at(5.0), Some(1.0));
		assert_eq!(n.eval_at(6.0), Some(5.0));

		// Linear ignores tension (matches nijilive).
		let l = lane(InterpolateMode::Linear, &[(0, 0.0, 0.9), (10, 10.0, 0.9)]);
		assert!((l.eval_at(2.5).unwrap() - 2.5).abs() < 1e-6);
	}

	#[test]
	fn cubic_animation_eval() {
		let kfs = [(0, 0.0, 0.5), (10, 0.0, 0.5), (20, 10.0, 0.5), (30, 0.0, 0.5)];
		let c = lane(InterpolateMode::Cubic, &kfs);
		assert!((c.eval_at(10.0).unwrap() - 0.0).abs() < 1e-6);
		assert!((c.eval_at(20.0).unwrap() - 10.0).abs() < 1e-6);
		let mid = c.eval_at(15.0).unwrap();
		assert!((mid - cubic(0.0, 0.0, 10.0, 0.0, 0.5)).abs() < 1e-6);
		assert!((mid - 5.0).abs() > 0.5, "cubic midpoint should overshoot/curve vs linear");
	}

	#[test]
	fn bezier_animation_eval_tension() {
		let l0 = lane(InterpolateMode::Bezier, &[(0, 0.0, 0.0), (10, 10.0, 0.0)]);
		let l5 = lane(InterpolateMode::Bezier, &[(0, 0.0, 0.5), (10, 10.0, 0.5)]);
		let l1 = lane(InterpolateMode::Bezier, &[(0, 0.0, 1.0), (10, 10.0, 1.0)]);

		// Tension 0.5 produces exact linear hermite(0, 1, 1, 1, t) == t.
		assert!((l5.eval_at(2.5).unwrap() - 2.5).abs() < 1e-6);
		// Tension 0.0 eases in/out slowly near endpoints (hermite(0,0,1,0,0.25) < 0.25).
		assert!(l0.eval_at(2.5).unwrap() < 2.0);
		// Tension 1.0 has steeper tangents near endpoints (hermite(0,2,1,2,0.25) > 2.5).
		assert!(l1.eval_at(2.5).unwrap() > 3.0);
	}
}

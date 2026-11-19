//! High-frequency (120Hz+) advanced GPU-assistant physics engine:
//! - Smooth deterministic 2D Perlin/gradient turbulence wind field
//! - Multi-segment coupled damped mass-spring chains with squash compression & limit clamping
//! - Audio RMS envelope follower & harmonic voice micro-tremor

use glam::{vec2, Vec2, Vec3};

/// Maximum stable integration substep (seconds).
const MAX_SUBSTEP_S: f32 = 1.0 / 240.0;

/// Metadata about a custom parameter exposed to VTube Studio (`ParameterCreationRequest`).
#[derive(Clone, Copy, Debug)]
pub struct VtsParameterSpec {
	pub name: &'static str,
	pub explanation: &'static str,
	pub min: f64,
	pub max: f64,
	pub default_value: f64,
}

/// 2D procedural wind field with base flow, sinusoidal gusts, and 2-octave gradient noise turbulence.
#[derive(Clone, Debug)]
pub struct WindField {
	pub direction_rad: f32,
	pub base_strength: f32,
	pub gust_strength: f32,
	pub gust_hz: f32,
	pub turbulence: f32,
	pub spatial_scale: f32,
	pub seed: u64,
}

impl Default for WindField {
	fn default() -> Self {
		Self {
			direction_rad: 0.15,
			base_strength: 0.28,
			gust_strength: 0.25,
			gust_hz: 0.45,
			turbulence: 0.35,
			spatial_scale: 0.015,
			seed: 0x5249_474C_454E_5331,
		}
	}
}

impl WindField {
	/// Evaluate wind force vector at position `pos` and simulation time `time_s`.
	/// Always returns finite components even if inputs are NaN or Inf.
	pub fn sample(&self, pos: Vec2, time_s: f32) -> Vec2 {
		if !pos.is_finite() || !time_s.is_finite() {
			return Vec2::ZERO;
		}
		let dir = if self.direction_rad.is_finite() {
			vec2(self.direction_rad.cos(), self.direction_rad.sin())
		} else {
			Vec2::X
		};
		let perp = vec2(-dir.y, dir.x);

		let base = self.base_strength.clamp(-4.0, 4.0);
		let gust_hz = self.gust_hz.clamp(0.0, 10.0);
		let gust_phase = time_s * gust_hz * std::f32::consts::TAU;
		let gust = self.gust_strength.clamp(0.0, 4.0) * (0.5 + 0.5 * gust_phase.sin());

		let sc = self.spatial_scale.clamp(1e-4, 2.0);
		let nx = pos.x * sc + time_s * 0.75;
		let ny = pos.y * sc + time_s * 0.35;

		let n1 = fbm2d(nx, ny, self.seed);
		let n2 = fbm2d(nx + 31.7, ny - 19.3, self.seed ^ 0x9E37_79B9_7F4A_7C15);
		let turb = self.turbulence.clamp(0.0, 4.0);

		let out = dir * (base + gust + turb * 0.65 * n1) + perp * (turb * 0.85 * n2);
		if out.is_finite() {
			out
		} else {
			Vec2::ZERO
		}
	}
}

#[inline]
fn hash_grid(ix: i32, iy: i32, seed: u64) -> Vec2 {
	let mut h = (ix as i64 as u64).wrapping_mul(0x9E37_79B1_85EB_CA87)
		^ (iy as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
		^ seed;
	h ^= h >> 33;
	h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
	h ^= h >> 33;
	h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
	h ^= h >> 33;
	let angle = ((h & 0xFFFF) as f32 / 65536.0) * std::f32::consts::TAU;
	vec2(angle.cos(), angle.sin())
}

#[inline]
fn quintic_fade(t: f32) -> f32 {
	t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn gradient_noise2d(x: f32, y: f32, seed: u64) -> f32 {
	let ix0 = x.floor() as i32;
	let iy0 = y.floor() as i32;
	let fx = x - ix0 as f32;
	let fy = y - iy0 as f32;

	let g00 = hash_grid(ix0, iy0, seed);
	let g10 = hash_grid(ix0 + 1, iy0, seed);
	let g01 = hash_grid(ix0, iy0 + 1, seed);
	let g11 = hash_grid(ix0 + 1, iy0 + 1, seed);

	let d00 = g00.dot(vec2(fx, fy));
	let d10 = g10.dot(vec2(fx - 1.0, fy));
	let d01 = g01.dot(vec2(fx, fy - 1.0));
	let d11 = g11.dot(vec2(fx - 1.0, fy - 1.0));

	let u = quintic_fade(fx);
	let v = quintic_fade(fy);

	let a = d00 + (d10 - d00) * u;
	let b = d01 + (d11 - d01) * u;
	(a + (b - a) * v).clamp(-1.0, 1.0)
}

fn fbm2d(x: f32, y: f32, seed: u64) -> f32 {
	let o1 = gradient_noise2d(x, y, seed);
	let o2 = gradient_noise2d(x * 2.03 + 7.1, y * 2.03 - 5.3, seed ^ 0xA24B_AED4_963E_E407);
	((o1 * 0.68) + (o2 * 0.32)).clamp(-1.0, 1.0)
}

/// Single node inside a [`CoupledSpringChain`].
#[derive(Clone, Debug)]
pub struct ChainSegment {
	pub pos: Vec2,
	pub vel: Vec2,
	pub stiffness: f32,
	pub damping: f32,
	pub mass: f32,
	pub max_displacement: f32,
}

impl Default for ChainSegment {
	fn default() -> Self {
		Self {
			pos: Vec2::ZERO,
			vel: Vec2::ZERO,
			stiffness: 95.0,
			damping: 8.5,
			mass: 1.0,
			max_displacement: 1.0,
		}
	}
}

/// Coupled multi-segment damped mass-spring chain with limit projection and NaN protection.
#[derive(Clone, Debug)]
pub struct CoupledSpringChain {
	pub segments: Vec<ChainSegment>,
	pub parent_coupling: f32,
	pub child_pull: f32,
}

impl CoupledSpringChain {
	/// Build an N-segment chain where deeper segments progressively soften and lag behind the root.
	pub fn new(
		segment_count: usize,
		base_stiffness: f32,
		base_damping: f32,
		max_displacement: f32,
	) -> Self {
		let count = segment_count.max(1);
		let mut segments = Vec::with_capacity(count);
		for i in 0..count {
			let factor = 1.0 / (1.0 + 0.32 * i as f32);
			segments.push(ChainSegment {
				pos: Vec2::ZERO,
				vel: Vec2::ZERO,
				stiffness: (base_stiffness * factor).max(4.0),
				damping: (base_damping * (0.85 + 0.15 * factor)).max(0.5),
				mass: 1.0 + 0.15 * i as f32,
				max_displacement: max_displacement.max(0.05),
			});
		}
		Self {
			segments,
			parent_coupling: 0.72,
			child_pull: 0.18,
		}
	}

	/// Advance the coupled chain using symplectic Euler substeps (`h <= 1/240s`).
	pub fn step(&mut self, external_accel: Vec2, dt: f32) {
		if self.segments.is_empty() || !dt.is_finite() || dt <= 0.0 {
			return;
		}
		let ext = if external_accel.is_finite() {
			external_accel.clamp_length_max(120.0)
		} else {
			Vec2::ZERO
		};
		let total_dt = dt.min(0.25);
		let substeps = (total_dt / MAX_SUBSTEP_S).ceil().clamp(1.0, 64.0) as usize;
		let h = total_dt / substeps as f32;

		let n = self.segments.len();
		for _ in 0..substeps {
			let snapshot: Vec<Vec2> = self
				.segments
				.iter()
				.map(|s| if s.pos.is_finite() { s.pos } else { Vec2::ZERO })
				.collect();

			for i in 0..n {
				let seg = &mut self.segments[i];
				if !seg.pos.is_finite() || !seg.vel.is_finite() {
					seg.pos = Vec2::ZERO;
					seg.vel = Vec2::ZERO;
				}
				let parent_pos = if i > 0 { snapshot[i - 1] } else { Vec2::ZERO };
				let target = parent_pos * self.parent_coupling;
				let mut force = -seg.stiffness * (seg.pos - target) - seg.damping * seg.vel;
				if i + 1 < n {
					force += (snapshot[i + 1] - seg.pos) * (seg.stiffness * self.child_pull);
				}

				let inv_mass = 1.0 / seg.mass.max(0.05);
				let acc = force * inv_mass + ext * (0.75 + 0.25 * (i as f32 + 1.0) / n as f32);
				seg.vel += acc * h;
				seg.pos += seg.vel * h;

				if !seg.pos.is_finite() || !seg.vel.is_finite() {
					seg.pos = Vec2::ZERO;
					seg.vel = Vec2::ZERO;
					continue;
				}

				// Soft collision limit clamp: remove outward radial velocity component when clamped
				let max_r = seg.max_displacement.max(0.05);
				let len_sq = seg.pos.length_squared();
				if len_sq > max_r * max_r {
					let len = len_sq.sqrt();
					let normal = seg.pos / len;
					seg.pos = normal * max_r;
					let radial_v = seg.vel.dot(normal);
					if radial_v > 0.0 {
						seg.vel -= normal * (radial_v * 1.15);
					}
				}
			}
		}
	}

	/// Blended displacement across all chain links normalized to `[-1, 1]`.
	pub fn tip_normalized(&self) -> Vec2 {
		if self.segments.is_empty() {
			return Vec2::ZERO;
		}
		let tip = self.segments.last().unwrap();
		let mid = &self.segments[self.segments.len() / 2];
		let blended = (tip.pos * 0.65 + mid.pos * 0.35) / tip.max_displacement.max(0.05);
		if blended.is_finite() {
			blended.clamp(Vec2::splat(-1.0), Vec2::splat(1.0))
		} else {
			Vec2::ZERO
		}
	}
}

/// Real-time pose and audio snapshot consumed by [`BoosterPhysicsEngine`].
#[derive(Clone, Debug, Default)]
pub struct MotionPoseInput {
	pub face_found: bool,
	pub head_pos: Vec2,
	pub head_angle: Vec3,
	pub body_angle: Vec3,
	pub eye_open_l: f32,
	pub eye_open_r: f32,
	pub mouth_open: f32,
	pub audio_rms: f32,
}

/// Compact motion metadata passed from physics/VTS readback to M2/M3 GPU passes.
#[derive(Clone, Copy, Debug, Default)]
pub struct MotionHint {
	pub face_found: bool,
	pub head_velocity: Vec2,
	pub head_angular_vel_z: f32,
	pub eye_open_min: f32,
	pub overall_motion: f32,
	pub wind_vec: Vec2,
}

/// Output produced per tick by [`BoosterPhysicsEngine`].
#[derive(Clone, Debug, Default)]
pub struct BoosterPhysicsOutput {
	pub time_s: f32,
	pub wind: Vec2,
	pub hair: Vec2,
	pub accessory: Vec2,
	pub body_bounce_y: f32,
	pub squash: f32,
	pub voice_tremor: f32,
	pub motion_hint: MotionHint,
}

impl BoosterPhysicsOutput {
	/// Export all 9 RigLens parameters formatted cleanly for VTube Studio `InjectParameterDataRequest`.
	pub fn to_vts_parameters(&self) -> Vec<(String, f32)> {
		let clamp1 = |v: f32| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
		let clamp01 = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
		vec![
			("RigLensWindX".to_owned(), clamp1(self.wind.x)),
			("RigLensWindY".to_owned(), clamp1(self.wind.y)),
			("RigLensHairX".to_owned(), clamp1(self.hair.x)),
			("RigLensHairY".to_owned(), clamp1(self.hair.y)),
			("RigLensAccX".to_owned(), clamp1(self.accessory.x)),
			("RigLensAccY".to_owned(), clamp1(self.accessory.y)),
			("RigLensBounceY".to_owned(), clamp1(self.body_bounce_y)),
			("RigLensSquash".to_owned(), clamp1(self.squash)),
			("RigLensVoiceTremor".to_owned(), clamp01(self.voice_tremor)),
		]
	}
}

/// Configuration for [`BoosterPhysicsEngine`].
#[derive(Clone, Debug)]
pub struct BoosterPhysicsConfig {
	pub wind: WindField,
	pub gravity: f32,
	pub motion_gain: f32,
	pub audio_gain: f32,
	pub audio_attack_hz: f32,
	pub audio_release_hz: f32,
}

impl Default for BoosterPhysicsConfig {
	fn default() -> Self {
		Self {
			wind: WindField::default(),
			gravity: 0.15,
			motion_gain: 1.0,
			audio_gain: 1.0,
			audio_attack_hz: 28.0,
			audio_release_hz: 6.5,
		}
	}
}

/// Stateful 120Hz secondary-motion physics simulator emitting VTS parameters and GPU motion hints.
pub struct BoosterPhysicsEngine {
	pub config: BoosterPhysicsConfig,
	pub hair_chain: CoupledSpringChain,
	pub accessory_chain: CoupledSpringChain,
	pub bounce_chain: CoupledSpringChain,
	time_s: f32,
	has_prev: bool,
	prev_head_pos: Vec2,
	prev_head_angle: Vec3,
	prev_head_vel: Vec2,
	audio_env: f32,
}

impl Default for BoosterPhysicsEngine {
	fn default() -> Self {
		Self::new(BoosterPhysicsConfig::default())
	}
}

impl BoosterPhysicsEngine {
	pub fn new(config: BoosterPhysicsConfig) -> Self {
		Self {
			config,
			hair_chain: CoupledSpringChain::new(3, 88.0, 6.2, 1.0),
			accessory_chain: CoupledSpringChain::new(2, 135.0, 8.8, 1.0),
			bounce_chain: CoupledSpringChain::new(2, 115.0, 9.2, 1.0),
			time_s: 0.0,
			has_prev: false,
			prev_head_pos: Vec2::ZERO,
			prev_head_angle: Vec3::ZERO,
			prev_head_vel: Vec2::ZERO,
			audio_env: 0.0,
		}
	}

	/// Canonical parameter specifications registered in VTube Studio on startup.
	pub fn parameter_specs() -> &'static [VtsParameterSpec] {
		&[
			VtsParameterSpec {
				name: "RigLensWindX",
				explanation: "RigLens GPU booster 2D turbulence wind X",
				min: -1.0,
				max: 1.0,
				default_value: 0.0,
			},
			VtsParameterSpec {
				name: "RigLensWindY",
				explanation: "RigLens GPU booster 2D turbulence wind Y",
				min: -1.0,
				max: 1.0,
				default_value: 0.0,
			},
			VtsParameterSpec {
				name: "RigLensHairX",
				explanation: "RigLens coupled multi-link hair sway X",
				min: -1.0,
				max: 1.0,
				default_value: 0.0,
			},
			VtsParameterSpec {
				name: "RigLensHairY",
				explanation: "RigLens coupled multi-link hair bounce Y",
				min: -1.0,
				max: 1.0,
				default_value: 0.0,
			},
			VtsParameterSpec {
				name: "RigLensAccX",
				explanation: "RigLens secondary accessory inertia X",
				min: -1.0,
				max: 1.0,
				default_value: 0.0,
			},
			VtsParameterSpec {
				name: "RigLensAccY",
				explanation: "RigLens secondary accessory inertia Y",
				min: -1.0,
				max: 1.0,
				default_value: 0.0,
			},
			VtsParameterSpec {
				name: "RigLensBounceY",
				explanation: "RigLens voice and step vertical bounce",
				min: -1.0,
				max: 1.0,
				default_value: 0.0,
			},
			VtsParameterSpec {
				name: "RigLensSquash",
				explanation: "RigLens soft-body compression squash factor",
				min: -1.0,
				max: 1.0,
				default_value: 0.0,
			},
			VtsParameterSpec {
				name: "RigLensVoiceTremor",
				explanation: "RigLens vocal RMS harmonic micro-tremor",
				min: 0.0,
				max: 1.0,
				default_value: 0.0,
			},
		]
	}

	/// Step physics forward by `dt` seconds given the latest pose/audio sample.
	pub fn step(&mut self, pose: &MotionPoseInput, dt: f32) -> BoosterPhysicsOutput {
		if !dt.is_finite() || dt <= 0.0 {
			return BoosterPhysicsOutput {
				time_s: self.time_s,
				..Default::default()
			};
		}
		let dt = dt.min(0.25);
		self.time_s += dt;

		let head_pos = if pose.head_pos.is_finite() {
			pose.head_pos
		} else {
			Vec2::ZERO
		};
		let head_angle = if pose.head_angle.is_finite() {
			pose.head_angle
		} else {
			Vec3::ZERO
		};

		let (head_vel, ang_vel_z, head_accel) = if self.has_prev {
			let hv = (head_pos - self.prev_head_pos) / dt;
			let az = (head_angle.z - self.prev_head_angle.z) / dt;
			let pitch_yaw_vel = vec2(
				(head_angle.x - self.prev_head_angle.x) / dt,
				(head_angle.y - self.prev_head_angle.y) / dt,
			) * 0.12;
			let combined_vel = hv + pitch_yaw_vel;
			let accel = (combined_vel - self.prev_head_vel) / dt;
			self.prev_head_vel = combined_vel.clamp_length_max(40.0);
			(
				combined_vel.clamp_length_max(40.0),
				az.clamp(-360.0, 360.0),
				accel.clamp_length_max(120.0),
			)
		} else {
			self.has_prev = true;
			(Vec2::ZERO, 0.0, Vec2::ZERO)
		};

		self.prev_head_pos = head_pos;
		self.prev_head_angle = head_angle;

		// Smooth audio RMS envelope follower
		let raw_rms = if pose.audio_rms.is_finite() {
			pose.audio_rms.clamp(0.0, 1.0)
		} else {
			0.0
		};
		let rate = if raw_rms > self.audio_env {
			self.config.audio_attack_hz.max(1.0)
		} else {
			self.config.audio_release_hz.max(0.5)
		};
		let alpha = (1.0 - (-rate * dt).exp()).clamp(0.0, 1.0);
		self.audio_env = (self.audio_env + (raw_rms - self.audio_env) * alpha).clamp(0.0, 1.0);

		let voice_harmonic = (self.time_s * 14.0 * std::f32::consts::TAU).sin() * 0.25 + 0.75;
		let voice_tremor = (self.audio_env * self.config.audio_gain.max(0.0) * voice_harmonic).clamp(0.0, 1.0);

		let wind = self.config.wind.sample(head_pos, self.time_s);
		let grav = vec2(0.0, self.config.gravity.clamp(-4.0, 4.0) * 10.0);
		let mg = self.config.motion_gain.clamp(0.0, 5.0);

		// Inertia drags secondary chains in opposite direction of head acceleration
		let inertia = -head_accel * (0.28 * mg) - vec2(ang_vel_z * 0.02 * mg, 0.0);
		let voice_impulse = vec2(0.0, -voice_tremor * 18.0);

		self.hair_chain.step(wind * 26.0 + inertia + grav, dt);
		self.accessory_chain
			.step(wind * 18.0 + inertia * 1.2 + grav * 0.8 + voice_impulse * 0.4, dt);
		self.bounce_chain
			.step(vec2(inertia.x * 0.25, inertia.y * 0.65) + voice_impulse + grav * 0.3, dt);

		let hair = self.hair_chain.tip_normalized();
		let accessory = self.accessory_chain.tip_normalized();
		let bounce = self.bounce_chain.tip_normalized();
		let squash = (-bounce.y * 0.75 + voice_tremor * 0.35).clamp(-1.0, 1.0);

		let eye_l = if pose.eye_open_l.is_finite() {
			pose.eye_open_l.clamp(0.0, 1.0)
		} else {
			1.0
		};
		let eye_r = if pose.eye_open_r.is_finite() {
			pose.eye_open_r.clamp(0.0, 1.0)
		} else {
			1.0
		};
		let overall_motion = (head_vel.length() * 0.2 + hair.length() * 0.5 + voice_tremor * 0.4).min(2.0);

		BoosterPhysicsOutput {
			time_s: self.time_s,
			wind: wind.clamp(Vec2::splat(-1.0), Vec2::splat(1.0)),
			hair,
			accessory,
			body_bounce_y: bounce.y.clamp(-1.0, 1.0),
			squash,
			voice_tremor,
			motion_hint: MotionHint {
				face_found: pose.face_found,
				head_velocity: head_vel,
				head_angular_vel_z: ang_vel_z,
				eye_open_min: eye_l.min(eye_r),
				overall_motion,
				wind_vec: wind,
			},
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn wind_field_is_deterministic_smooth_and_nan_safe() {
		let wf = WindField::default();
		let a = wf.sample(vec2(10.0, 20.0), 1.5);
		let b = wf.sample(vec2(10.0, 20.0), 1.5);
		assert_eq!(a, b);
		assert!(a.is_finite());

		let c = wf.sample(vec2(10.05, 20.02), 1.505);
		assert!((c - a).length() < 0.05, "wind field should vary smoothly: {a:?} vs {c:?}");

		let bad = wf.sample(vec2(f32::NAN, f32::INFINITY), f32::NAN);
		assert_eq!(bad, Vec2::ZERO);
	}

	#[test]
	fn coupled_spring_damps_and_handles_extreme_or_nan_dt() {
		let mut chain = CoupledSpringChain::new(3, 90.0, 7.5, 1.0);
		for _ in 0..6 {
			chain.step(vec2(80.0, 0.0), 1.0 / 60.0);
		}
		let peak = chain.tip_normalized().x;
		assert!(peak.abs() > 0.08, "expected chain displacement under impulse, got {peak}");

		for _ in 0..360 {
			chain.step(Vec2::ZERO, 1.0 / 120.0);
			let tip = chain.tip_normalized();
			assert!(tip.is_finite() && tip.length() <= 1.0001);
		}
		assert!(
			chain.tip_normalized().length() < 0.05,
			"expected chain to settle near rest, got {:?}",
			chain.tip_normalized()
		);

		// Extreme & non-finite inputs must never panic or poison state
		chain.step(vec2(f32::NAN, f32::INFINITY), 10.0);
		chain.step(vec2(1e9, -1e9), f32::NAN);
		chain.step(Vec2::X, -0.5);
		assert!(chain.tip_normalized().is_finite());
	}

	#[test]
	fn booster_engine_emits_valid_vts_params_and_reacts_to_audio_and_motion() {
		for spec in BoosterPhysicsEngine::parameter_specs() {
			assert!((4..=32).contains(&spec.name.len()), "bad param len: {}", spec.name);
			assert!(
				spec.name.chars().all(|c| c.is_ascii_alphanumeric()),
				"VTS requires alphanumeric param names: {}",
				spec.name
			);
		}

		let mut engine = BoosterPhysicsEngine::default();
		let idle = MotionPoseInput {
			face_found: true,
			eye_open_l: 1.0,
			eye_open_r: 1.0,
			..Default::default()
		};
		for _ in 0..30 {
			let out = engine.step(&idle, 1.0 / 120.0);
			for (name, val) in out.to_vts_parameters() {
				assert!(val.is_finite(), "param {name} non-finite: {val}");
				assert!((-1.0..=1.0).contains(&val), "param {name} out of range: {val}");
			}
		}

		// Speak / head snap -> voice tremor + bounce should rise significantly
		let loud = MotionPoseInput {
			face_found: true,
			head_pos: vec2(2.5, 0.0),
			head_angle: Vec3::new(15.0, 0.0, 6.0),
			eye_open_l: 1.0,
			eye_open_r: 1.0,
			mouth_open: 0.85,
			audio_rms: 0.9,
			..Default::default()
		};
		let out_loud = engine.step(&loud, 1.0 / 120.0);
		assert!(out_loud.voice_tremor > 0.1, "expected voice tremor > 0.1, got {}", out_loud.voice_tremor);
		assert!(out_loud.hair.is_finite());

		// After audio drops to 0, envelope should decay cleanly
		let mut last_tremor = out_loud.voice_tremor;
		for _ in 0..180 {
			last_tremor = engine.step(&idle, 1.0 / 120.0).voice_tremor;
		}
		assert!(last_tremor < 0.02, "voice tremor failed to decay: {last_tremor}");
	}
}

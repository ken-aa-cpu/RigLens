//! Screen-Space XPBD Soft-Body Mesh Solver for unrigged Live2D / RigLens regions
//! (cheeks, bangs/ahoge, chest/plushie) driven by head inertia & wind hints.

use bytemuck::{Pod, Zeroable};
use glam::{vec2, Vec2};

use crate::physics::MotionHint;

pub const MAX_SOFT_BODY_ZONES: usize = 8;
pub const SOFT_BODY_GRID_N: usize = 9;
pub const SOFT_BODY_VERT_COUNT: usize = SOFT_BODY_GRID_N * SOFT_BODY_GRID_N; // 81

/// User / AI-configurable screen-space soft-body patch.
#[derive(Clone, Debug)]
pub struct SoftBodyZoneConfig {
	pub name: String,
	/// Center of patch in normalized image UVs `[0, 1] x [0, 1]`.
	pub anchor_uv: Vec2,
	/// Half-width / half-height radii in normalized UV coordinates.
	pub radius_uv: Vec2,
	/// Sensitivity to head translation/rotation inertia.
	pub follow_head: f32,
	/// Sensitivity to turbulence wind vector.
	pub wind_sensitivity: f32,
	/// XPBD structural stiffness (higher = crisper snap-back).
	pub stiffness: f32,
	/// Exponential velocity damping per second.
	pub damping: f32,
	/// Maximum normalized displacement inside the zone (`[0.02 .. 0.6]`).
	pub max_strain: f32,
}

impl Default for SoftBodyZoneConfig {
	fn default() -> Self {
		Self {
			name: "Zone".to_owned(),
			anchor_uv: vec2(0.5, 0.5),
			radius_uv: vec2(0.14, 0.14),
			follow_head: 1.0,
			wind_sensitivity: 0.4,
			stiffness: 180.0,
			damping: 7.5,
			max_strain: 0.28,
		}
	}
}

/// Sensible default VTuber screen-space soft-body zones (cheeks, front bangs, torso/plushie).
pub fn default_vtuber_zones() -> Vec<SoftBodyZoneConfig> {
	vec![
		SoftBodyZoneConfig {
			name: "Cheek_L".to_owned(),
			anchor_uv: vec2(0.42, 0.44),
			radius_uv: vec2(0.09, 0.08),
			follow_head: 1.15,
			wind_sensitivity: 0.15,
			stiffness: 220.0,
			damping: 8.5,
			max_strain: 0.24,
		},
		SoftBodyZoneConfig {
			name: "Cheek_R".to_owned(),
			anchor_uv: vec2(0.58, 0.44),
			radius_uv: vec2(0.09, 0.08),
			follow_head: 1.15,
			wind_sensitivity: 0.15,
			stiffness: 220.0,
			damping: 8.5,
			max_strain: 0.24,
		},
		SoftBodyZoneConfig {
			name: "Bangs_Ahoge".to_owned(),
			anchor_uv: vec2(0.50, 0.26),
			radius_uv: vec2(0.16, 0.12),
			follow_head: 0.9,
			wind_sensitivity: 1.25,
			stiffness: 140.0,
			damping: 6.2,
			max_strain: 0.32,
		},
		SoftBodyZoneConfig {
			name: "Torso_Soft".to_owned(),
			anchor_uv: vec2(0.50, 0.68),
			radius_uv: vec2(0.18, 0.16),
			follow_head: 0.85,
			wind_sensitivity: 0.55,
			stiffness: 155.0,
			damping: 7.0,
			max_strain: 0.28,
		},
	]
}

/// 16-byte-aligned GPU uniform/storage representation matching `soft_body_warp.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SoftBodyZoneGpu {
	/// `(anchor_x, anchor_y, radius_x, radius_y)`
	pub anchor_radius: [f32; 4],
	/// `(max_strain, active_flag, 0.0, 0.0)`
	pub params: [f32; 4],
	/// 81 `(dx, dy)` displacements packed into 41 `vec4<f32>` (`xy` = even, `zw` = odd).
	pub offsets: [[f32; 4]; 41],
}

impl Default for SoftBodyZoneGpu {
	fn default() -> Self {
		Self::zeroed()
	}
}

#[derive(Clone, Debug)]
struct XpbdConstraint {
	a: usize,
	b: usize,
	rest_len: f32,
	compliance_scale: f32,
}

/// Single 9x9 XPBD patch with pinned outer border (`ix/iy in {0, 8}`).
#[derive(Clone, Debug)]
pub struct XpbdSoftBodyGrid {
	pub config: SoftBodyZoneConfig,
	/// Displacements in normalized local patch space `[-1, 1] x [-1, 1]`.
	pub disp: [Vec2; SOFT_BODY_VERT_COUNT],
	prev_disp: [Vec2; SOFT_BODY_VERT_COUNT],
	pub vel: [Vec2; SOFT_BODY_VERT_COUNT],
	rest_pos: [Vec2; SOFT_BODY_VERT_COUNT],
	inv_mass: [f32; SOFT_BODY_VERT_COUNT],
	constraints: Vec<XpbdConstraint>,
}

impl XpbdSoftBodyGrid {
	pub fn new(config: SoftBodyZoneConfig) -> Self {
		let mut rest_pos = [Vec2::ZERO; SOFT_BODY_VERT_COUNT];
		let mut inv_mass = [0.0f32; SOFT_BODY_VERT_COUNT];
		let step = 2.0 / (SOFT_BODY_GRID_N - 1) as f32;

		for iy in 0..SOFT_BODY_GRID_N {
			for ix in 0..SOFT_BODY_GRID_N {
				let idx = iy * SOFT_BODY_GRID_N + ix;
				let px = -1.0 + ix as f32 * step;
				let py = -1.0 + iy as f32 * step;
				rest_pos[idx] = vec2(px, py);
				let is_border = ix == 0 || ix == SOFT_BODY_GRID_N - 1 || iy == 0 || iy == SOFT_BODY_GRID_N - 1;
				inv_mass[idx] = if is_border {
					0.0
				} else {
					let r = (px * px + py * py).sqrt().clamp(0.0, 1.0);
					// Core nodes move more freely than near-border collar nodes
					(1.25 - 0.65 * r).max(0.25)
				};
			}
		}

		let mut constraints = Vec::new();
		let add_edge = |c: &mut Vec<XpbdConstraint>, a: usize, b: usize, scale: f32| {
			if inv_mass[a] == 0.0 && inv_mass[b] == 0.0 {
				return;
			}
			let rest_len = (rest_pos[a] - rest_pos[b]).length();
			c.push(XpbdConstraint {
				a,
				b,
				rest_len,
				compliance_scale: scale,
			});
		};

		for iy in 0..SOFT_BODY_GRID_N {
			for ix in 0..SOFT_BODY_GRID_N {
				let idx = iy * SOFT_BODY_GRID_N + ix;
				if ix + 1 < SOFT_BODY_GRID_N {
					add_edge(&mut constraints, idx, idx + 1, 1.0);
				}
				if iy + 1 < SOFT_BODY_GRID_N {
					add_edge(&mut constraints, idx, idx + SOFT_BODY_GRID_N, 1.0);
				}
				if ix + 1 < SOFT_BODY_GRID_N && iy + 1 < SOFT_BODY_GRID_N {
					add_edge(&mut constraints, idx, idx + SOFT_BODY_GRID_N + 1, 1.35);
				}
				if ix > 0 && iy + 1 < SOFT_BODY_GRID_N {
					add_edge(&mut constraints, idx, idx + SOFT_BODY_GRID_N - 1, 1.35);
				}
			}
		}

		Self {
			config,
			disp: [Vec2::ZERO; SOFT_BODY_VERT_COUNT],
			prev_disp: [Vec2::ZERO; SOFT_BODY_VERT_COUNT],
			vel: [Vec2::ZERO; SOFT_BODY_VERT_COUNT],
			rest_pos,
			inv_mass,
			constraints,
		}
	}

	/// Step the XPBD solver (`substeps = 4`) under inertial acceleration + wind force.
	pub fn step(&mut self, accel: Vec2, dt: f32) {
		if !dt.is_finite() || dt <= 0.0 {
			return;
		}
		let dt = dt.min(0.05);
		let accel = if accel.is_finite() {
			accel.clamp_length_max(60.0)
		} else {
			Vec2::ZERO
		};

		const SUBSTEPS: usize = 4;
		let h = dt / SUBSTEPS as f32;
		let k = self.config.stiffness.clamp(20.0, 4000.0);
		let damping = self.config.damping.clamp(0.5, 40.0);
		let damp_factor = (-damping * h).exp().clamp(0.0, 1.0);
		let max_disp = (self.config.max_strain.clamp(0.02, 0.75) * 2.2).max(0.05);

		for _ in 0..SUBSTEPS {
			// Predict positions
			for i in 0..SOFT_BODY_VERT_COUNT {
				let w = self.inv_mass[i];
				if w == 0.0 {
					self.disp[i] = Vec2::ZERO;
					self.prev_disp[i] = Vec2::ZERO;
					self.vel[i] = Vec2::ZERO;
					continue;
				}
				if !self.disp[i].is_finite() || !self.vel[i].is_finite() {
					self.disp[i] = Vec2::ZERO;
					self.vel[i] = Vec2::ZERO;
				}
				self.prev_disp[i] = self.disp[i];
				// Soft anchor spring pulls displacement toward zero rest shape
				let restore = -self.disp[i] * (k * 0.22);
				self.vel[i] = (self.vel[i] + (accel * w + restore) * h) * damp_factor;
				self.disp[i] += self.vel[i] * h;
			}

			// XPBD distance constraint solve
			let base_alpha = 1.0 / (k * h * h);
			for c in &self.constraints {
				let wa = self.inv_mass[c.a];
				let wb = self.inv_mass[c.b];
				let w_sum = wa + wb;
				if w_sum <= 1e-8 {
					continue;
				}
				let pa = self.rest_pos[c.a] + self.disp[c.a];
				let pb = self.rest_pos[c.b] + self.disp[c.b];
				let diff = pa - pb;
				let dist = diff.length();
				if dist <= 1e-6 {
					continue;
				}
				let c_val = dist - c.rest_len;
				let alpha_tilde = base_alpha * c.compliance_scale;
				let lambda = -c_val / (w_sum + alpha_tilde);
				let corr = diff * (lambda / dist);
				if wa > 0.0 {
					self.disp[c.a] += corr * wa;
				}
				if wb > 0.0 {
					self.disp[c.b] -= corr * wb;
				}
			}

			// Update velocities & clamp max strain cleanly
			let inv_h = 1.0 / h;
			for i in 0..SOFT_BODY_VERT_COUNT {
				if self.inv_mass[i] == 0.0 {
					self.disp[i] = Vec2::ZERO;
					self.vel[i] = Vec2::ZERO;
					continue;
				}
				if self.disp[i].length_squared() > max_disp * max_disp {
					self.disp[i] = self.disp[i].normalize() * max_disp;
				}
				self.vel[i] = (self.disp[i] - self.prev_disp[i]) * inv_h;
				if !self.disp[i].is_finite() || !self.vel[i].is_finite() {
					self.disp[i] = Vec2::ZERO;
					self.vel[i] = Vec2::ZERO;
				}
			}
		}
	}

	/// Export displaced UV deltas scaled into image UV units for the WGSL compute shader.
	pub fn to_gpu(&self) -> SoftBodyZoneGpu {
		let mut out = SoftBodyZoneGpu::zeroed();
		out.anchor_radius = [
			self.config.anchor_uv.x,
			self.config.anchor_uv.y,
			self.config.radius_uv.x.max(0.01),
			self.config.radius_uv.y.max(0.01),
		];
		out.params = [self.config.max_strain, 1.0, 0.0, 0.0];

		for i in 0..SOFT_BODY_VERT_COUNT {
			// Scale local [-1, 1] grid displacement into UV-space displacement
			let d = vec2(
				self.disp[i].x * self.config.radius_uv.x * 0.45,
				self.disp[i].y * self.config.radius_uv.y * 0.45,
			);
			let pair = i / 2;
			if i % 2 == 0 {
				out.offsets[pair][0] = d.x;
				out.offsets[pair][1] = d.y;
			} else {
				out.offsets[pair][2] = d.x;
				out.offsets[pair][3] = d.y;
			}
		}
		out
	}
}

/// Manages up to [`MAX_SOFT_BODY_ZONES`] coupled XPBD patches synchronized with [`MotionHint`].
#[derive(Clone, Debug)]
pub struct XpbdSoftBodySystem {
	pub grids: Vec<XpbdSoftBodyGrid>,
	prev_head_vel: Vec2,
	has_prev: bool,
}

impl XpbdSoftBodySystem {
	pub fn new(configs: Vec<SoftBodyZoneConfig>) -> Self {
		let grids = configs
			.into_iter()
			.take(MAX_SOFT_BODY_ZONES)
			.map(XpbdSoftBodyGrid::new)
			.collect();
		Self {
			grids,
			prev_head_vel: Vec2::ZERO,
			has_prev: false,
		}
	}

	pub fn step(&mut self, motion: &MotionHint, dt: f32) {
		if !dt.is_finite() || dt <= 0.0 {
			return;
		}
		let hv = if motion.head_velocity.is_finite() {
			motion.head_velocity
		} else {
			Vec2::ZERO
		};
		let head_accel = if self.has_prev {
			((hv - self.prev_head_vel) / dt.max(1e-4)).clamp_length_max(80.0)
		} else {
			self.has_prev = true;
			Vec2::ZERO
		};
		self.prev_head_vel = hv;

		let wind = if motion.wind_vec.is_finite() {
			motion.wind_vec
		} else {
			Vec2::ZERO
		};

		for grid in &mut self.grids {
			let fg = grid.config.follow_head;
			let ws = grid.config.wind_sensitivity;
			let ang_push = vec2(-motion.head_angular_vel_z * 0.025, 0.0);
			let accel = -head_accel * (0.45 * fg) + wind * (5.5 * ws) + ang_push * fg;
			grid.step(accel, dt);
		}
	}

	pub fn pack_gpu_zones(&self) -> [SoftBodyZoneGpu; MAX_SOFT_BODY_ZONES] {
		let mut zones = [SoftBodyZoneGpu::default(); MAX_SOFT_BODY_ZONES];
		for (i, g) in self.grids.iter().take(MAX_SOFT_BODY_ZONES).enumerate() {
			zones[i] = g.to_gpu();
		}
		zones
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn xpbd_grid_pins_border_damps_and_never_nans() {
		let mut grid = XpbdSoftBodyGrid::new(SoftBodyZoneConfig::default());
		let center_idx = 4 * SOFT_BODY_GRID_N + 4;

		for _ in 0..8 {
			grid.step(vec2(18.0, -9.0), 1.0 / 60.0);
		}
		assert!(
			grid.disp[center_idx].length() > 0.02,
			"expected interior soft-body node displacement, got {:?}",
			grid.disp[center_idx]
		);

		// Verify all perimeter vertices remain strictly pinned to zero
		for iy in 0..SOFT_BODY_GRID_N {
			for ix in 0..SOFT_BODY_GRID_N {
				if ix == 0 || ix == SOFT_BODY_GRID_N - 1 || iy == 0 || iy == SOFT_BODY_GRID_N - 1 {
					let idx = iy * SOFT_BODY_GRID_N + ix;
					assert_eq!(grid.disp[idx], Vec2::ZERO, "border vertex {ix},{iy} moved!");
				}
			}
		}

		// Settle under zero load
		for _ in 0..240 {
			grid.step(Vec2::ZERO, 1.0 / 60.0);
		}
		assert!(
			grid.disp[center_idx].length() < 0.02,
			"expected XPBD grid to settle, got {:?}",
			grid.disp[center_idx]
		);

		// NaN/Inf and extreme dt guard
		grid.step(vec2(f32::NAN, f32::INFINITY), 10.0);
		grid.step(Vec2::X, f32::NAN);
		for d in grid.disp {
			assert!(d.is_finite());
		}
	}
}

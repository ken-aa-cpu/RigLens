//! Tier 1 Ground-Truth Mesh Velocity Field rasterization (matching Inochi2D / RigLens 2D meshes).

use glam::Vec2;

/// Dense per-pixel motion vector buffer formatted for GPU upload as `TextureFormat::Rgba32Float`:
/// `(vx_px, vy_px, confidence, valid)` where `v = curr_pos - prev_pos`.
#[derive(Clone, Debug, PartialEq)]
pub struct MotionVectorField {
	pub width: u32,
	pub height: u32,
	pub data: Vec<[f32; 4]>,
}

impl MotionVectorField {
	pub fn new(width: u32, height: u32) -> Self {
		let len = (width as usize).saturating_mul(height as usize);
		Self {
			width,
			height,
			data: vec![[0.0, 0.0, 0.0, 0.0]; len],
		}
	}

	/// Fill masked pixels with a uniform velocity vector (useful for rigid sprites & tests).
	pub fn fill_uniform(
		width: u32,
		height: u32,
		vel: Vec2,
		mut mask_fn: impl FnMut(u32, u32) -> bool,
	) -> Self {
		let mut field = Self::new(width, height);
		let vx = if vel.x.is_finite() { vel.x } else { 0.0 };
		let vy = if vel.y.is_finite() { vel.y } else { 0.0 };
		for y in 0..height {
			for x in 0..width {
				if mask_fn(x, y) {
					field.data[(y * width + x) as usize] = [vx, vy, 1.0, 1.0];
				}
			}
		}
		field
	}

	/// Rasterize exact triangle-mesh displacement vectors (`curr_verts[i] - prev_verts[i]`)
	/// onto pixel centers `[0..width] x [0..height]` using barycentric interpolation.
	pub fn rasterize_mesh_motion(
		width: u32,
		height: u32,
		prev_verts: &[Vec2],
		curr_verts: &[Vec2],
		indices: &[u32],
	) -> Self {
		let mut field = Self::new(width, height);
		if width == 0 || height == 0 || prev_verts.len() != curr_verts.len() {
			return field;
		}
		let vlen = curr_verts.len();
		let wf = width as f32;
		let hf = height as f32;

		for tri in indices.chunks_exact(3) {
			let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
			if i0 >= vlen || i1 >= vlen || i2 >= vlen {
				continue;
			}
			let a = curr_verts[i0];
			let b = curr_verts[i1];
			let c = curr_verts[i2];
			if !a.is_finite() || !b.is_finite() || !c.is_finite() {
				continue;
			}
			let area2 = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
			if area2.abs() <= 1e-5 {
				continue;
			}

			let va = a - prev_verts[i0];
			let vb = b - prev_verts[i1];
			let vc = c - prev_verts[i2];
			if !va.is_finite() || !vb.is_finite() || !vc.is_finite() {
				continue;
			}

			let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as u32;
			let max_x = a.x.max(b.x).max(c.x).ceil().min(wf - 1.0).max(0.0) as u32;
			let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as u32;
			let max_y = a.y.max(b.y).max(c.y).ceil().min(hf - 1.0).max(0.0) as u32;
			if min_x > max_x || min_y > max_y {
				continue;
			}

			let inv_area2 = 1.0 / area2;
			for y in min_y..=max_y {
				let py = y as f32 + 0.5;
				for x in min_x..=max_x {
					let px = x as f32 + 0.5;
					let w0 = ((c.x - b.x) * (py - b.y) - (c.y - b.y) * (px - b.x)) * inv_area2;
					let w1 = ((a.x - c.x) * (py - c.y) - (a.y - c.y) * (px - c.x)) * inv_area2;
					let w2 = 1.0 - w0 - w1;
					if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
						let v = va * w0 + vb * w1 + vc * w2;
						field.data[(y * width + x) as usize] = [v.x, v.y, 1.0, 1.0];
					}
				}
			}
		}

		field
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use glam::vec2;

	#[test]
	fn mesh_velocity_field_rasterizes_exact_triangle_displacement() {
		let prev = [vec2(10.0, 10.0), vec2(40.0, 10.0), vec2(10.0, 40.0)];
		let delta = vec2(6.0, -4.0);
		let curr = [prev[0] + delta, prev[1] + delta, prev[2] + delta];
		let indices = [0, 1, 2];

		let mv = MotionVectorField::rasterize_mesh_motion(64, 64, &prev, &curr, &indices);
		// Interior pixel (20, 16) lies inside moved triangle [16..46] x [6..36]
		let sample = mv.data[(16 * 64 + 20) as usize];
		assert!((sample[0] - 6.0).abs() < 1e-3, "expected vx=6, got {}", sample[0]);
		assert!((sample[1] + 4.0).abs() < 1e-3, "expected vy=-4, got {}", sample[1]);
		assert_eq!(sample[3], 1.0);

		// Outside pixel stays zero/invalid
		let empty = mv.data[0];
		assert_eq!(empty, [0.0, 0.0, 0.0, 0.0]);
	}
}

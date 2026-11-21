use glam::Vec2;

/// Curve interpolation type for [`crate::node::components::PathDeformer`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CurveType {
	Bezier,
	#[default]
	Spline,
}

impl CurveType {
	/// Name used in `.inp` / `.inx` files.
	pub fn as_str(self) -> &'static str {
		match self {
			CurveType::Bezier => "Bezier",
			CurveType::Spline => "Spline",
		}
	}

	/// Parse a curve type name from JSON.
	pub fn from_name(name: &str) -> Option<Self> {
		match name {
			"Bezier" => Some(CurveType::Bezier),
			"Spline" => Some(CurveType::Spline),
			_ => None,
		}
	}

	/// Parse a curve type ordinal (0 = Bezier, 1 = Spline).
	pub fn from_ordinal(ord: i64) -> Option<Self> {
		match ord {
			0 => Some(CurveType::Bezier),
			1 => Some(CurveType::Spline),
			_ => None,
		}
	}
}

/// Evaluate both point and derivative of a Bezier curve via de Casteljau's algorithm.
/// Using de Casteljau avoids factorial overflow for high control-point counts.
fn evaluate_bezier_point_and_deriv(pts: &[Vec2], t: f32) -> (Vec2, Vec2) {
	match pts.len() {
		0 => (Vec2::ZERO, Vec2::ZERO),
		1 => (pts[0], Vec2::ZERO),
		n => {
			let t = t.clamp(0.0, 1.0);
			let mut work = pts.to_vec();
			let mut deriv = Vec2::ZERO;
			for len in (2..=n).rev() {
				if len == 2 {
					deriv = (work[1] - work[0]) * (n - 1) as f32;
				}
				for i in 0..len - 1 {
					work[i] = work[i].lerp(work[i + 1], t);
				}
			}
			(work[0], deriv)
		}
	}
}

/// Evaluate a point on a Catmull-Rom spline at `t` in `0..=1`.
fn evaluate_spline_point(pts: &[Vec2], t: f32) -> Vec2 {
	match pts.len() {
		0 => Vec2::ZERO,
		1 => pts[0],
		2 => {
			let t = t.clamp(0.0, 1.0);
			pts[0].lerp(pts[1], t)
		}
		len => {
			let t = t.clamp(0.0, 1.0);
			let segment = t * (len - 1) as f32;
			let p1 = (segment.floor() as usize).min(len - 2);
			let lt = (segment - p1 as f32).clamp(0.0, 1.0);
			let p0 = p1.saturating_sub(1);
			let p2 = (p1 + 1).min(len - 1);
			let p3 = (p2 + 1).min(len - 1);

			let lt2 = lt * lt;
			let lt3 = lt2 * lt;
			let a = 2.0 * pts[p1];
			let b = pts[p2] - pts[p0];
			let c = 2.0 * pts[p0] - 5.0 * pts[p1] + 4.0 * pts[p2] - pts[p3];
			let d = -pts[p0] + 3.0 * pts[p1] - 3.0 * pts[p2] + pts[p3];
			0.5 * (a + b * lt + c * lt2 + d * lt3)
		}
	}
}

/// Evaluate the tangent derivative on a Catmull-Rom spline at `t` in `0..=1`.
fn evaluate_spline_derivative(pts: &[Vec2], t: f32) -> Vec2 {
	match pts.len() {
		0 | 1 => Vec2::ZERO,
		2 => pts[1] - pts[0],
		len => {
			let t = t.clamp(0.0, 1.0);
			let segment = t * (len - 1) as f32;
			let p1 = (segment.floor() as usize).min(len - 2);
			let lt = (segment - p1 as f32).clamp(0.0, 1.0);
			let p0 = p1.saturating_sub(1);
			let p2 = (p1 + 1).min(len - 1);
			let p3 = (p2 + 1).min(len - 1);

			let lt2 = lt * lt;
			let b = pts[p2] - pts[p0];
			let c = 2.0 * pts[p0] - 5.0 * pts[p1] + 4.0 * pts[p2] - pts[p3];
			let d = -pts[p0] + 3.0 * pts[p1] - 3.0 * pts[p2] + pts[p3];
			0.5 * (b + 2.0 * c * lt + 3.0 * d * lt2)
		}
	}
}

/// Evaluate a point on the curve at parameter `t` in `0..=1`.
pub fn evaluate_point(curve_type: CurveType, pts: &[Vec2], t: f32) -> Vec2 {
	match curve_type {
		CurveType::Bezier => evaluate_bezier_point_and_deriv(pts, t).0,
		CurveType::Spline => evaluate_spline_point(pts, t),
	}
}

/// Evaluate the curve tangent vector at parameter `t` in `0..=1`.
pub fn evaluate_derivative(curve_type: CurveType, pts: &[Vec2], t: f32) -> Vec2 {
	match curve_type {
		CurveType::Bezier => evaluate_bezier_point_and_deriv(pts, t).1,
		CurveType::Spline => evaluate_spline_derivative(pts, t),
	}
}

/// Find the curve parameter `t` in `0..=1` whose curve point is closest to `target`.
pub fn closest_point(curve_type: CurveType, pts: &[Vec2], target: Vec2, samples: usize) -> f32 {
	if pts.len() < 2 {
		return 0.0;
	}
	let n = samples.max(16);
	let mut best_k = 0;
	let mut best_dist = f32::INFINITY;
	for k in 0..=n {
		let t = k as f32 / n as f32;
		let dist = (evaluate_point(curve_type, pts, t) - target).length_squared();
		if dist < best_dist {
			best_dist = dist;
			best_k = k;
		}
	}

	let mut lo = best_k.saturating_sub(1) as f32 / n as f32;
	let mut hi = (best_k + 1).min(n) as f32 / n as f32;
	let phi = 0.618_034_f32;
	for _ in 0..16 {
		let x1 = hi - phi * (hi - lo);
		let x2 = lo + phi * (hi - lo);
		let d1 = (evaluate_point(curve_type, pts, x1) - target).length_squared();
		let d2 = (evaluate_point(curve_type, pts, x2) - target).length_squared();
		if d1 < d2 {
			hi = x2;
		} else {
			lo = x1;
		}
	}
	0.5 * (lo + hi)
}

#[cfg(test)]
mod tests {
	use glam::vec2;

	use super::*;

	#[test]
	fn bezier_endpoints_midpoint_and_derivative() {
		let pts = vec![
			vec2(0.0, 0.0),
			vec2(10.0, 20.0),
			vec2(30.0, -10.0),
			vec2(40.0, 30.0),
			vec2(50.0, 0.0),
			vec2(60.0, 10.0),
		];
		assert!((evaluate_point(CurveType::Bezier, &pts, 0.0) - pts[0]).length() < 1e-5);
		assert!((evaluate_point(CurveType::Bezier, &pts, 1.0) - pts[5]).length() < 1e-5);

		// Compare midpoint against direct Bernstein evaluation for quadratic and derivative vs FD.
		let quad = [vec2(0.0, 0.0), vec2(10.0, 20.0), vec2(20.0, 0.0)];
		assert!((evaluate_point(CurveType::Bezier, &quad, 0.5) - vec2(10.0, 10.0)).length() < 1e-5);

		let eps = 1e-3;
		for t in [0.15, 0.5, 0.85] {
			let fd = (evaluate_point(CurveType::Bezier, &pts, t + eps)
				- evaluate_point(CurveType::Bezier, &pts, t - eps))
				/ (2.0 * eps);
			let ana = evaluate_derivative(CurveType::Bezier, &pts, t);
			assert!((fd - ana).length() < 1e-2, "t={t}: fd={fd:?} ana={ana:?}");
		}
	}

	#[test]
	fn spline_linear_and_catmull_rom() {
		let line = [vec2(0.0, 0.0), vec2(10.0, 20.0)];
		assert!((evaluate_point(CurveType::Spline, &line, 0.25) - vec2(2.5, 5.0)).length() < 1e-5);
		assert!((evaluate_derivative(CurveType::Spline, &line, 0.5) - vec2(10.0, 20.0)).length() < 1e-5);

		let pts = [
			vec2(0.0, 0.0),
			vec2(10.0, 15.0),
			vec2(25.0, -5.0),
			vec2(40.0, 10.0),
		];
		assert!((evaluate_point(CurveType::Spline, &pts, 0.0) - pts[0]).length() < 1e-5);
		assert!((evaluate_point(CurveType::Spline, &pts, 1.0) - pts[3]).length() < 1e-5);

		let eps = 1e-3;
		let seg_scale = (pts.len() - 1) as f32;
		for t in [0.1, 0.45, 0.8] {
			let fd = (evaluate_point(CurveType::Spline, &pts, t + eps)
				- evaluate_point(CurveType::Spline, &pts, t - eps))
				/ (2.0 * eps * seg_scale);
			let ana = evaluate_derivative(CurveType::Spline, &pts, t);
			assert!((fd - ana).length() < 1e-2, "t={t}: fd={fd:?} ana={ana:?}");
		}
	}

	#[test]
	fn closest_point_finds_nearest_param() {
		let two_pt = [vec2(0.0, 0.0), vec2(20.0, 0.0)];
		let t2 = closest_point(CurveType::Spline, &two_pt, vec2(7.0, 4.0), 100);
		assert!((t2 - 0.35).abs() < 1e-3, "got t={t2}");

		let three_pt = [vec2(0.0, 0.0), vec2(10.0, 0.0), vec2(20.0, 0.0)];
		let t3 = closest_point(CurveType::Spline, &three_pt, vec2(7.0, 4.0), 100);
		let p3 = evaluate_point(CurveType::Spline, &three_pt, t3);
		assert!((p3 - vec2(7.0, 0.0)).length() < 1e-3, "got p={p3:?} at t={t3}");

		let s_curve = [
			vec2(0.0, 0.0),
			vec2(10.0, 20.0),
			vec2(20.0, -20.0),
			vec2(30.0, 0.0),
		];
		let expected_t = 0.72_f32;
		let pt = evaluate_point(CurveType::Bezier, &s_curve, expected_t);
		let got = closest_point(CurveType::Bezier, &s_curve, pt, 100);
		assert!((got - expected_t).abs() < 1e-3, "got={got} expected={expected_t}");
	}
}

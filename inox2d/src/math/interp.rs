use glam::Vec2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpolateMode {
	/// Round to nearest
	Nearest,
	/// Linear interpolation
	Linear,
	/// Hold the left keypoint value (nijilive `Stepped`).
	Stepped,
	/// Cubic interpolation (nijilive `Cubic`: Catmull-Rom for parameters and animations).
	Cubic,
	/// Bezier interpolation (nijilive `Bezier`: Hermite tension curves in animations; Linear fallback in parameters).
	Bezier,
}

impl InterpolateMode {
	/// Name used in `.inp` files.
	pub fn as_str(self) -> &'static str {
		match self {
			InterpolateMode::Nearest => "Nearest",
			InterpolateMode::Linear => "Linear",
			InterpolateMode::Stepped => "Stepped",
			InterpolateMode::Cubic => "Cubic",
			InterpolateMode::Bezier => "Bezier",
		}
	}

	/// Parse an `.inp` interpolation mode name.
	pub fn from_name(name: &str) -> Option<Self> {
		Some(match name {
			"Nearest" => InterpolateMode::Nearest,
			"Linear" => InterpolateMode::Linear,
			"Stepped" => InterpolateMode::Stepped,
			"Cubic" => InterpolateMode::Cubic,
			"Bezier" => InterpolateMode::Bezier,
			_ => return None,
		})
	}
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InterpRange<T> {
	pub beg: T,
	pub end: T,
}

impl<T> InterpRange<T> {
	#[inline]
	pub fn new(beg: T, end: T) -> Self {
		Self { beg, end }
	}
}

impl InterpRange<Vec2> {
	#[inline]
	pub fn to_x(self) -> InterpRange<f32> {
		InterpRange {
			beg: self.beg.x,
			end: self.end.x,
		}
	}

	#[inline]
	pub fn to_y(self) -> InterpRange<f32> {
		InterpRange {
			beg: self.beg.y,
			end: self.end.y,
		}
	}
}

#[inline]
fn interpolate_nearest(t: f32, range_in: InterpRange<f32>, range_out: InterpRange<f32>) -> f32 {
	debug_assert!(
		range_in.beg <= t && t <= range_in.end,
		"{} <= {} <= {}",
		range_in.beg,
		t,
		range_in.end
	);

	if (range_in.end - t) < (t - range_in.beg) {
		range_out.end
	} else {
		range_out.beg
	}
}

#[inline]
fn interpolate_linear(t: f32, range_in: InterpRange<f32>, range_out: InterpRange<f32>) -> f32 {
	// A degenerate input range happens for single-point axes, e.g. the Y axis
	// of 1D parameters (axis_points `[0]`). There is nothing to interpolate
	// along such an axis, so return the start value instead of dividing by
	// zero (which would produce NaN and e.g. make driven parts disappear).
	if range_in.end == range_in.beg {
		return range_out.beg;
	}

	debug_assert!(
		range_in.beg <= t && t <= range_in.end,
		"{} is out of input range [{}, {}]",
		t,
		range_in.beg,
		range_in.end,
	);

	(t - range_in.beg) * (range_out.end - range_out.beg) / (range_in.end - range_in.beg) + range_out.beg
}

#[inline]
pub fn interpolate_f32(t: f32, range_in: InterpRange<f32>, range_out: InterpRange<f32>, mode: InterpolateMode) -> f32 {
	match mode {
		InterpolateMode::Nearest => interpolate_nearest(t, range_in, range_out),
		InterpolateMode::Stepped => range_out.beg,
		// TODO: true cubic/bezier needs neighbouring keypoints; approximate with linear for now.
		InterpolateMode::Linear | InterpolateMode::Cubic | InterpolateMode::Bezier => {
			interpolate_linear(t, range_in, range_out)
		}
	}
}

#[inline]
pub fn interpolate_vec2(
	t: f32,
	range_in: InterpRange<f32>,
	range_out: InterpRange<Vec2>,
	mode: InterpolateMode,
) -> Vec2 {
	let x = interpolate_f32(t, range_in, range_out.to_x(), mode);
	let y = interpolate_f32(t, range_in, range_out.to_y(), mode);
	Vec2 { x, y }
}

pub fn interpolate_f32s_additive(
	t: f32,
	range_in: InterpRange<f32>,
	range_out: InterpRange<&[f32]>,
	mode: InterpolateMode,
	out: &mut [f32],
) {
	for ((&ob, &oe), o) in range_out.beg.iter().zip(range_out.end).zip(out) {
		*o += interpolate_f32(t, range_in, InterpRange::new(ob, oe), mode);
	}
}

pub fn interpolate_vec2s_additive(
	t: f32,
	range_in: InterpRange<f32>,
	range_out: InterpRange<&[Vec2]>,
	mode: InterpolateMode,
	out: &mut [Vec2],
) {
	for ((&ob, &oe), o) in range_out.beg.iter().zip(range_out.end).zip(out) {
		*o += interpolate_vec2(t, range_in, InterpRange::new(ob, oe), mode);
	}
}

#[inline]
pub fn bi_interpolate_f32(
	t: Vec2,
	range_in: InterpRange<Vec2>,
	out_top: InterpRange<f32>,
	out_bottom: InterpRange<f32>,
	mode: InterpolateMode,
) -> f32 {
	let beg = interpolate_f32(t.x, range_in.to_x(), out_top, mode);
	let end = interpolate_f32(t.x, range_in.to_x(), out_bottom, mode);
	interpolate_f32(t.y, range_in.to_y(), InterpRange::new(beg, end), mode)
}

#[inline]
pub fn bi_interpolate_vec2(
	t: Vec2,
	range_in: InterpRange<Vec2>,
	out_top: InterpRange<Vec2>,
	out_bottom: InterpRange<Vec2>,
	mode: InterpolateMode,
) -> Vec2 {
	let beg = interpolate_vec2(t.x, range_in.to_x(), out_top, mode);
	let end = interpolate_vec2(t.x, range_in.to_x(), out_bottom, mode);
	interpolate_vec2(t.y, range_in.to_y(), InterpRange::new(beg, end), mode)
}

pub fn bi_interpolate_f32s_additive(
	t: Vec2,
	range_in: InterpRange<Vec2>,
	out_top: InterpRange<&[f32]>,
	out_bottom: InterpRange<&[f32]>,
	mode: InterpolateMode,
	out: &mut [f32],
) {
	for (((&otb, &ote), (&obb, &obe)), o) in (out_top.beg.iter().zip(out_top.end))
		.zip(out_bottom.beg.iter().zip(out_bottom.end))
		.zip(out)
	{
		*o += bi_interpolate_f32(
			t,
			range_in,
			InterpRange::new(otb, ote),
			InterpRange::new(obb, obe),
			mode,
		)
	}
}

pub fn bi_interpolate_vec2s_additive(
	t: Vec2,
	range_in: InterpRange<Vec2>,
	out_top: InterpRange<&[Vec2]>,
	out_bottom: InterpRange<&[Vec2]>,
	mode: InterpolateMode,
	out: &mut [Vec2],
) {
	for (((&otb, &ote), (&obb, &obe)), o) in (out_top.beg.iter().zip(out_top.end))
		.zip(out_bottom.beg.iter().zip(out_bottom.end))
		.zip(out)
	{
		*o += bi_interpolate_vec2(
			t,
			range_in,
			InterpRange::new(otb, ote),
			InterpRange::new(obb, obe),
			mode,
		)
	}
}

/// Catmull-Rom segment between `p1` (t = 0) and `p2` (t = 1), with `p0`/`p3` as neighbours.
/// Catmull-Rom segment between `p1` (t = 0) and `p2` (t = 1), with `p0`/`p3` as neighbours.
/// Matches nijilive's `cubic` (inmath).
#[inline]
pub fn cubic<T>(p0: T, p1: T, p2: T, p3: T, t: f32) -> T
where
	T: Copy + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>,
{
	let a = p0 * -0.5 + p1 * 1.5 + p2 * -1.5 + p3 * 0.5;
	let b = p0 + p1 * -2.5 + p2 * 2.0 + p3 * -0.5;
	let c = p0 * -0.5 + p2 * 0.5;
	let d = p1;
	a * (t * t * t) + b * (t * t) + c * t + d
}

/// Cubic Hermite spline between `x` (t = 0) and `y` (t = 1) with tangents `tx`, `ty`.
/// Matches nijilive's `hermite` (inmath).
#[inline]
pub fn hermite(x: f32, tx: f32, y: f32, ty: f32, t: f32) -> f32 {
	let t2 = t * t;
	let t3 = t2 * t;
	let h1 = 2.0 * t3 - 3.0 * t2 + 1.0;
	let h2 = -2.0 * t3 + 3.0 * t2;
	let h3 = t3 - 2.0 * t2 + t;
	let h4 = t3 - t2;
	h1 * x + h3 * tx + h2 * y + h4 * ty
}

/// Neighbour indices (and in-cell offset) along one axis, following nijilive's clamping:
/// `[left - 1, left, right, right + 1]`, each clamped to the axis. Single-point axes are constant.
#[inline]
fn cubic_axis(left: usize, right: usize, last: usize, offset: f32) -> ([usize; 4], f32) {
	if left == right {
		([left; 4], 0.0)
	} else {
		([left.saturating_sub(1), left, right, (right + 1).min(last)], offset)
	}
}

/// Bicubic sample of a keypoint grid inside the cell `[xs.0, xs.1] x [ys.0, ys.1]`.
///
/// `offset` is the position inside the cell, 0..=1 per axis. `x_last`/`y_last` are the last valid indices.
pub fn cubic_cell<T>(
	get: impl Fn(usize, usize) -> T,
	xs: (usize, usize),
	ys: (usize, usize),
	x_last: usize,
	y_last: usize,
	offset: Vec2,
) -> T
where
	T: Copy + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>,
{
	let (xi, tx) = cubic_axis(xs.0, xs.1, x_last, offset.x);
	let (yi, ty) = cubic_axis(ys.0, ys.1, y_last, offset.y);
	let row = |y: usize| cubic(get(xi[0], y), get(xi[1], y), get(xi[2], y), get(xi[3], y), tx);
	cubic(row(yi[0]), row(yi[1]), row(yi[2]), row(yi[3]), ty)
}

/// Where a parameter value falls inside the keypoint grid; picks Cubic or the pairwise modes.
pub struct Cell {
	pub t: Vec2,
	pub range_in: InterpRange<Vec2>,
	pub xs: (usize, usize),
	pub ys: (usize, usize),
	pub x_last: usize,
	pub y_last: usize,
}

impl Cell {
	fn offset(&self) -> Vec2 {
		let span = self.range_in.end - self.range_in.beg;
		Vec2::new(
			if span.x.abs() > 1e-8 { (self.t.x - self.range_in.beg.x) / span.x } else { 0.0 },
			if span.y.abs() > 1e-8 { (self.t.y - self.range_in.beg.y) / span.y } else { 0.0 },
		)
	}

	pub fn is_cubic(mode: InterpolateMode) -> bool {
		mode == InterpolateMode::Cubic
	}

	/// Interpolate a scalar binding.
	pub fn f32(
		&self,
		matrix: &crate::math::matrix::Matrix2d<f32>,
		out_top: InterpRange<f32>,
		out_bottom: InterpRange<f32>,
		mode: InterpolateMode,
	) -> f32 {
		if Self::is_cubic(mode) {
			cubic_cell(|x, y| matrix[(x, y)], self.xs, self.ys, self.x_last, self.y_last, self.offset())
		} else {
			bi_interpolate_f32(self.t, self.range_in, out_top, out_bottom, mode)
		}
	}

	/// Add an interpolated per-vertex deformation into `out`.
	pub fn vec2s_additive(
		&self,
		matrix: &crate::math::matrix::Matrix2d<Vec<Vec2>>,
		out_top: InterpRange<&[Vec2]>,
		out_bottom: InterpRange<&[Vec2]>,
		mode: InterpolateMode,
		out: &mut [Vec2],
	) {
		if Self::is_cubic(mode) {
			let off = self.offset();
			for (i, o) in out.iter_mut().enumerate() {
				let get = |x: usize, y: usize| matrix[(x, y)].get(i).copied().unwrap_or(Vec2::ZERO);
				*o += cubic_cell(get, self.xs, self.ys, self.x_last, self.y_last, off);
			}
		} else {
			bi_interpolate_vec2s_additive(self.t, self.range_in, out_top, out_bottom, mode, out);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_linear_interpolation() {
		assert_eq!(
			interpolate_linear(0.0, InterpRange::new(0.0, 1.0), InterpRange::new(-5.0, 5.0)),
			-5.0
		);
		assert_eq!(
			interpolate_linear(1.0, InterpRange::new(0.0, 1.0), InterpRange::new(-5.0, 5.0)),
			5.0
		);
		assert_eq!(
			interpolate_linear(0.5, InterpRange::new(0.0, 1.0), InterpRange::new(-5.0, 5.0)),
			0.0
		);
		assert_eq!(
			interpolate_linear(0.0, InterpRange::new(-0.5, 0.0), InterpRange::new(-5.0, 5.0)),
			5.0
		);
	}

	#[test]
	fn test_degenerate_input_range() {
		// Single-point axes (e.g. the Y axis `[0]` of 1D parameters) produce a
		// zero-length input range. Interpolating over it must not divide by
		// zero (NaN); the start value is returned instead.
		let r = interpolate_linear(0.0, InterpRange::new(0.0, 0.0), InterpRange::new(3.0, 3.0));
		assert_eq!(r, 3.0);
		assert!(r.is_finite());

		// Same for the bilinear entry points used by param bindings.
		let b = bi_interpolate_f32(
			Vec2::new(0.6, 0.0),
			InterpRange::new(Vec2::new(0.5, 0.0), Vec2::new(1.0, 0.0)),
			InterpRange::new(0.0, 15.0),
			InterpRange::new(0.0, 15.0),
			InterpolateMode::Linear,
		);
		assert!(b.is_finite());
		assert!((b - 3.0).abs() < 1e-5);
	}

	#[test]
	fn cubic_matches_catmull_rom_endpoints_and_line() {
		// Segment endpoints are interpolated exactly.
		assert!((cubic(0.0f32, 1.0, 2.0, 5.0, 0.0) - 1.0).abs() < 1e-6);
		assert!((cubic(0.0f32, 1.0, 2.0, 5.0, 1.0) - 2.0).abs() < 1e-6);
		// Collinear, evenly spaced points give a straight line.
		assert!((cubic(0.0f32, 1.0, 2.0, 3.0, 0.5) - 1.5).abs() < 1e-6);
		// Non-trivial curvature: p0=0,p1=0,p2=1,p3=1 at t=.5 -> 0.5 (symmetric)
		assert!((cubic(0.0f32, 0.0, 1.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
	}

	#[test]
	fn cubic_cell_clamps_at_edges() {
		// 1D grid [0, 1, 4]; sample in the first cell: left neighbour clamps to index 0.
		let vals = [0.0f32, 1.0, 4.0];
		let v = cubic_cell(|x, _| vals[x], (0, 1), (0, 0), 2, 0, Vec2::new(0.5, 0.0));
		let expect = cubic(0.0, 0.0, 1.0, 4.0, 0.5);
		assert!((v - expect).abs() < 1e-6);
	}
}

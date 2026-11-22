use glam::{vec2, Vec2};
use riglens_core::math::interp::InterpolateMode;
use riglens_core::math::matrix::Matrix2d;
use riglens_core::node::components::{GridDeformer, Mesh, PathDeformer, TransformStore};
use riglens_core::node::InoxNodeUuid;
use riglens_core::params::{AxisPoints, Binding, BindingValues};
use riglens_core::puppet::Puppet;

use super::bindings::ensure_param;

/// Apply a Gaussian soft-brush deform to a Part node's parameter keyframe using either
/// a semantic anchor (`center`, `left`, `right`, `top`, `bottom`, `top_left`, `top_right`, `bottom_left`, `bottom_right`)
/// or explicit local `(x, y)` coordinates.
/// Ensure `param_name` and a `BindingValues::Deform` binding exist for `node_id`,
/// returning `(binding_idx, ix, iy)` snapped to the closest axis grid keypoint.
fn ensure_deform_binding_keypoint(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	param_val: Vec2,
	vert_count: usize,
) -> Result<(usize, usize, usize), String> {
	if !puppet.params.contains_key(param_name) {
		ensure_param(
			puppet,
			param_name,
			true,
			vec2(-1.0, -1.0),
			vec2(1.0, 1.0),
			vec2(0.0, 0.0),
			AxisPoints {
				x: vec![0.0, 0.5, 1.0],
				y: vec![0.0, 0.5, 1.0],
			},
		);
	}

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;

	let span = param.max - param.min;
	let norm_x = if span.x.abs() > 1e-6 {
		((param_val.x - param.min.x) / span.x).clamp(0.0, 1.0)
	} else {
		0.0
	};
	let norm_y = if span.y.abs() > 1e-6 {
		((param_val.y - param.min.y) / span.y).clamp(0.0, 1.0)
	} else {
		0.0
	};

	let ix = param
		.axis_points
		.x
		.iter()
		.enumerate()
		.min_by(|a, b| (a.1 - norm_x).abs().total_cmp(&(b.1 - norm_x).abs()))
		.map(|(i, _)| i)
		.unwrap_or(0);
	let iy = param
		.axis_points
		.y
		.iter()
		.enumerate()
		.min_by(|a, b| (a.1 - norm_y).abs().total_cmp(&(b.1 - norm_y).abs()))
		.map(|(i, _)| i)
		.unwrap_or(0);

	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let existing_idx = param
		.bindings
		.iter()
		.position(|b| b.node == node_id && matches!(b.values, BindingValues::Deform(_)));

	if existing_idx.is_none() {
		let zero_mat = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| vec![Vec2::ZERO; vert_count]);
		let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
		param.bindings.push(Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Deform(zero_mat),
		});
	}

	let b_idx = existing_idx.unwrap_or(param.bindings.len() - 1);
	Ok((b_idx, ix, iy))
}

/// Return the rest-pose deform control vertices for `Mesh`, `GridDeformer`, or `PathDeformer`.
pub fn node_deform_rest_vertices(puppet: &Puppet, node_id: InoxNodeUuid) -> Result<Vec<Vec2>, String> {
	if let Some(mesh) = puppet.node_comps.get::<Mesh>(node_id) {
		if mesh.vertices.is_empty() {
			return Err(format!("Node {} Mesh has 0 vertices", node_id.0));
		}
		return Ok(mesh.vertices.clone());
	}
	if let Some(grid) = puppet.node_comps.get::<GridDeformer>(node_id) {
		if !grid.has_valid_grid() {
			return Err(format!("Node {} GridDeformer has invalid grid", node_id.0));
		}
		return Ok(grid.rest_vertices());
	}
	if let Some(path) = puppet.node_comps.get::<PathDeformer>(node_id) {
		if path.vertices.is_empty() {
			return Err(format!("Node {} PathDeformer has 0 vertices", node_id.0));
		}
		return Ok(path.vertices.clone());
	}
	Err(format!(
		"Node {} does not have a Mesh, GridDeformer, or PathDeformer component",
		node_id.0
	))
}

/// Sculpting brush mode matching mainstream 2D rig editors (Grab / Smooth / Bulge / Twist).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeformBrushMode {
	#[default]
	Grab,
	Smooth,
	Bulge,
	Twist,
}

impl DeformBrushMode {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Grab => "grab",
			Self::Smooth => "smooth",
			Self::Bulge => "bulge",
			Self::Twist => "twist",
		}
	}

	pub fn label(self) -> &'static str {
		match self {
			Self::Grab => "Grab",
			Self::Smooth => "Smooth",
			Self::Bulge => "Bulge",
			Self::Twist => "Twist",
		}
	}

	pub fn parse(s: &str) -> Self {
		match s.trim().to_ascii_lowercase().as_str() {
			"smooth" | "relax" => Self::Smooth,
			"bulge" | "pinch" | "inflate" => Self::Bulge,
			"twist" | "rotate" => Self::Twist,
			_ => Self::Grab,
		}
	}
}

fn node_vertex_neighbors(puppet: &Puppet, node_id: InoxNodeUuid, vert_count: usize) -> Vec<Vec<usize>> {
	let mut adj = vec![Vec::<usize>::new(); vert_count];
	let mut add_edge = |a: usize, b: usize| {
		if a < vert_count && b < vert_count && a != b {
			if !adj[a].contains(&b) {
				adj[a].push(b);
			}
			if !adj[b].contains(&a) {
				adj[b].push(a);
			}
		}
	};
	if let Some(mesh) = puppet.node_comps.get::<Mesh>(node_id) {
		for tri in mesh.indices.chunks_exact(3) {
			let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
			add_edge(a, b);
			add_edge(b, c);
			add_edge(c, a);
		}
	} else if let Some(grid) = puppet.node_comps.get::<GridDeformer>(node_id) {
		let cols = grid.cols();
		let rows = grid.rows();
		for r in 0..rows {
			for c in 0..cols {
				let i = r * cols + c;
				if c + 1 < cols {
					add_edge(i, i + 1);
				}
				if r + 1 < rows {
					add_edge(i, i + cols);
				}
			}
		}
	} else if let Some(path) = puppet.node_comps.get::<PathDeformer>(node_id) {
		for i in 0..path.vertices.len().saturating_sub(1) {
			add_edge(i, i + 1);
		}
	}
	adj
}

/// Apply a Grab deform brush (backwards-compatible wrapper for [`apply_deform_brush_mode`]).
pub fn apply_deform_brush(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	param_val: Vec2,
	anchor: Option<&str>,
	local_xy: Option<Vec2>,
	radius_ratio: f32,
	delta: Vec2,
) -> Result<(usize, usize, usize), String> {
	apply_deform_brush_mode(
		puppet,
		node_id,
		param_name,
		param_val,
		anchor,
		local_xy,
		radius_ratio,
		delta,
		DeformBrushMode::Grab,
	)
}

/// Apply a multi-mode soft deform brush (`Grab`, `Smooth`, `Bulge`, `Twist`) with smooth cubic falloff.
pub fn apply_deform_brush_mode(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	param_val: Vec2,
	anchor: Option<&str>,
	local_xy: Option<Vec2>,
	radius_ratio: f32,
	delta: Vec2,
	mode: DeformBrushMode,
) -> Result<(usize, usize, usize), String> {
	let verts = node_deform_rest_vertices(puppet, node_id)?;
	let neighbors = if mode == DeformBrushMode::Smooth {
		node_vertex_neighbors(puppet, node_id, verts.len())
	} else {
		Vec::new()
	};

	let (b_idx, ix, iy) = ensure_deform_binding_keypoint(puppet, node_id, param_name, param_val, verts.len())?;

	let mut mn = Vec2::splat(f32::INFINITY);
	let mut mx = Vec2::splat(f32::NEG_INFINITY);
	for &v in &verts {
		mn = mn.min(v);
		mx = mx.max(v);
	}
	let center = (mn + mx) * 0.5;
	let size = (mx - mn).max(vec2(4.0, 4.0));
	let brush_center = if let Some(xy) = local_xy {
		xy
	} else {
		match anchor.unwrap_or("center").to_lowercase().as_str() {
			"left" => vec2(mn.x, center.y),
			"right" => vec2(mx.x, center.y),
			"top" => vec2(center.x, mn.y),
			"bottom" => vec2(center.x, mx.y),
			"top_left" => vec2(mn.x, mn.y),
			"top_right" => vec2(mx.x, mn.y),
			"bottom_left" => vec2(mn.x, mx.y),
			"bottom_right" => vec2(mx.x, mx.y),
			_ => center,
		}
	};

	let radius = (size.length() * radius_ratio.clamp(0.05, 3.0)).max(2.0);
	let sigma2 = 2.0 * (radius * 0.5).powi(2);
	let inv_r2 = 1.0 / (radius * radius);

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;
	let mut affected = 0usize;
	if let BindingValues::Deform(ref mut mat) = param.bindings[b_idx].values {
		if let Some(cell) = mat.get_mut(ix, iy) {
			match mode {
				DeformBrushMode::Grab => {
					for (vi, &v) in verts.iter().enumerate() {
						let d2 = (v - brush_center).length_squared();
						let w = (-d2 / sigma2).exp();
						if w > 0.02 {
							if let Some(slot) = cell.get_mut(vi) {
								*slot += delta * w;
								affected += 1;
							}
						}
					}
				}
				DeformBrushMode::Smooth => {
					if delta.length_squared() >= 1e-6 {
						let prev = cell.clone();
						let strength = (delta.length().clamp(1.0, 14.0) / radius).clamp(0.10, 0.55);
						for (vi, &v) in verts.iter().enumerate() {
							let nbrs = &neighbors[vi];
							if nbrs.is_empty() {
								continue;
							}
							let cur_pos = v + prev[vi];
							let t2 = ((cur_pos - brush_center).length_squared() * inv_r2).clamp(0.0, 1.0);
							let w = (1.0 - t2) * (1.0 - t2);
							if w > 0.01 {
								let mut sum = Vec2::ZERO;
								for &ni in nbrs {
									sum += verts[ni] + prev[ni];
								}
								let avg = sum / (nbrs.len() as f32);
								cell[vi] += (avg - cur_pos) * (strength * w);
								affected += 1;
							}
						}
					}
				}
				DeformBrushMode::Bulge => {
					let diff = delta.x - delta.y;
					let mag = if diff.abs() > delta.length() * 0.15 {
						diff
					} else if delta.x.abs() >= delta.y.abs() {
						delta.x
					} else {
						-delta.y
					};
					for (vi, &v) in verts.iter().enumerate() {
						let cur_pos = v + cell.get(vi).copied().unwrap_or(Vec2::ZERO);
						let rel = cur_pos - brush_center;
						let t2 = (rel.length_squared() * inv_r2).clamp(0.0, 1.0);
						let w = (1.0 - t2) * (1.0 - t2);
						if w > 0.01 && rel.length_squared() > 1e-4 {
							let dir = rel.normalize();
							if let Some(slot) = cell.get_mut(vi) {
								*slot += dir * (mag * w);
								affected += 1;
							}
						}
					}
				}
				DeformBrushMode::Twist => {
					let angle = ((delta.x - delta.y) / radius).clamp(-0.6, 0.6);
					for (vi, &v) in verts.iter().enumerate() {
						let cur_pos = v + cell.get(vi).copied().unwrap_or(Vec2::ZERO);
						let rel = cur_pos - brush_center;
						let t2 = (rel.length_squared() * inv_r2).clamp(0.0, 1.0);
						let w = (1.0 - t2) * (1.0 - t2);
						if w > 0.01 {
							let a = angle * w;
							let (s, c) = a.sin_cos();
							let rotated = vec2(rel.x * c - rel.y * s, rel.x * s + rel.y * c);
							if let Some(slot) = cell.get_mut(vi) {
								*slot += rotated - rel;
								affected += 1;
							}
						}
					}
				}
			}
		}
	}

	Ok((ix, iy, affected))
}

/// Apply an exact deform delta `(dx, dy)` (or reset to `0`) to explicitly selected vertex `indices`
/// at the active keypoint of `param_name`.
pub fn apply_vertex_deform_delta(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	param_val: Vec2,
	indices: &[u32],
	delta: Vec2,
	reset: bool,
) -> Result<(usize, usize, usize), String> {
	let vert_count = node_deform_rest_vertices(puppet, node_id)?.len();

	let (b_idx, ix, iy) = ensure_deform_binding_keypoint(puppet, node_id, param_name, param_val, vert_count)?;
	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;

	let mut updated = 0usize;
	if let BindingValues::Deform(ref mut mat) = param.bindings[b_idx].values {
		if let Some(cell) = mat.get_mut(ix, iy) {
			for &vi in indices {
				if let Some(slot) = cell.get_mut(vi as usize) {
					if reset {
						*slot = Vec2::ZERO;
					} else {
						*slot += delta;
					}
					updated += 1;
				}
			}
		}
	}
	Ok((ix, iy, updated))
}

pub fn binding_kind_name(values: &BindingValues) -> &'static str {
	match values {
		BindingValues::ZSort(_) => "ZSort",
		BindingValues::TransformTX(_) => "TransformTX",
		BindingValues::TransformTY(_) => "TransformTY",
		BindingValues::TransformSX(_) => "TransformSX",
		BindingValues::TransformSY(_) => "TransformSY",
		BindingValues::TransformRX(_) => "TransformRX",
		BindingValues::TransformRY(_) => "TransformRY",
		BindingValues::TransformRZ(_) => "TransformRZ",
		BindingValues::Deform(_) => "Deform",
		BindingValues::Opacity(_) => "Opacity",
	}
}

fn scalar_neutral_value(kind: &str) -> Result<f32, String> {
	match kind {
		"TransformSX" | "TransformSY" | "Opacity" => Ok(1.0),
		"TransformTX" | "TransformTY" | "TransformRX" | "TransformRY" | "TransformRZ" | "ZSort" => Ok(0.0),
		_ => Err(format!(
			"Unsupported scalar binding kind '{kind}' (expected TransformTX/TY/SX/SY/RX/RY/RZ, ZSort, or Opacity)"
		)),
	}
}

fn scalar_matrix_mut<'a>(values: &'a mut BindingValues, kind: &str) -> Option<&'a mut Matrix2d<f32>> {
	match (values, kind) {
		(BindingValues::ZSort(m), "ZSort")
		| (BindingValues::TransformTX(m), "TransformTX")
		| (BindingValues::TransformTY(m), "TransformTY")
		| (BindingValues::TransformSX(m), "TransformSX")
		| (BindingValues::TransformSY(m), "TransformSY")
		| (BindingValues::TransformRX(m), "TransformRX")
		| (BindingValues::TransformRY(m), "TransformRY")
		| (BindingValues::TransformRZ(m), "TransformRZ")
		| (BindingValues::Opacity(m), "Opacity") => Some(m),
		_ => None,
	}
}

/// Set a scalar binding value at keypoint `(ix, iy)` for `(node_id, param_name, binding_kind)`,
/// creating the binding matrix if it does not yet exist.
pub fn set_scalar_binding_keypoint(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	ix: usize,
	iy: usize,
	binding_kind: &str,
	value: f32,
) -> Result<(), String> {
	if puppet.nodes.get_node(node_id).is_none() {
		return Err(format!("Node {} not found", node_id.0));
	}
	if !value.is_finite() {
		return Err("Binding value must be finite".to_owned());
	}
	let neutral = scalar_neutral_value(binding_kind)?;
	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;
	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	if ix >= x_len || iy >= y_len {
		return Err(format!(
			"Keypoint ({ix}, {iy}) out of bounds for axis grid ({x_len}x{y_len})"
		));
	}

	let idx = param
		.bindings
		.iter()
		.position(|b| b.node == node_id && binding_kind_name(&b.values) == binding_kind);
	let b_idx = match idx {
		Some(i) => i,
		None => {
			let mat = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| neutral);
			let values = match binding_kind {
				"ZSort" => BindingValues::ZSort(mat),
				"TransformTX" => BindingValues::TransformTX(mat),
				"TransformTY" => BindingValues::TransformTY(mat),
				"TransformSX" => BindingValues::TransformSX(mat),
				"TransformSY" => BindingValues::TransformSY(mat),
				"TransformRX" => BindingValues::TransformRX(mat),
				"TransformRY" => BindingValues::TransformRY(mat),
				"TransformRZ" => BindingValues::TransformRZ(mat),
				"Opacity" => BindingValues::Opacity(mat),
				_ => unreachable!(),
			};
			param.bindings.push(Binding {
				node: node_id,
				is_set: Matrix2d::from_grid_transposed(x_len, y_len, |_, _| false),
				interpolate_mode: InterpolateMode::Linear,
				values,
			});
			param.bindings.len() - 1
		}
	};

	let binding = &mut param.bindings[b_idx];
	if let Some(m) = scalar_matrix_mut(&mut binding.values, binding_kind) {
		if let Some(slot) = m.get_mut(ix, iy) {
			*slot = value;
		}
	}
	if let Some(flag) = binding.is_set.get_mut(ix, iy) {
		*flag = true;
	}
	Ok(())
}

/// Clear (`is_set = false` + reset to neutral) a keypoint cell for either `"Deform"` or any scalar binding kind.
pub fn clear_binding_keypoint(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	ix: usize,
	iy: usize,
	binding_kind: &str,
) -> Result<bool, String> {
	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;
	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	if ix >= x_len || iy >= y_len {
		return Err(format!(
			"Keypoint ({ix}, {iy}) out of bounds for axis grid ({x_len}x{y_len})"
		));
	}

	let Some(binding) = param
		.bindings
		.iter_mut()
		.find(|b| b.node == node_id && binding_kind_name(&b.values) == binding_kind)
	else {
		return Ok(false);
	};

	match &mut binding.values {
		BindingValues::Deform(mat) => {
			if let Some(cell) = mat.get_mut(ix, iy) {
				for v in cell.iter_mut() {
					*v = Vec2::ZERO;
				}
			}
		}
		other => {
			let neutral = scalar_neutral_value(binding_kind)?;
			if let Some(m) = scalar_matrix_mut(other, binding_kind) {
				if let Some(slot) = m.get_mut(ix, iy) {
					*slot = neutral;
				}
			}
		}
	}
	if let Some(flag) = binding.is_set.get_mut(ix, iy) {
		*flag = false;
	}
	Ok(true)
}

/// Remove bindings targeting `node_id` on `param_name` (all kinds if `binding_kind` is `None`).
pub fn remove_param_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	binding_kind: Option<&str>,
) -> Result<usize, String> {
	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;
	let before = param.bindings.len();
	param.bindings.retain(|b| {
		b.node != node_id || binding_kind.is_some_and(|k| binding_kind_name(&b.values) != k)
	});
	Ok(before - param.bindings.len())
}

/// Copy or X-mirror a `Deform` keypoint `(src_ix, src_iy)` onto `(dst_ix, dst_iy)` across rest-vertex center X.
pub fn mirror_deform_keypoint(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	src_ix: usize,
	src_iy: usize,
	dst_ix: usize,
	dst_iy: usize,
	mirror_x: bool,
) -> Result<usize, String> {
	let verts = node_deform_rest_vertices(puppet, node_id)?;
	let mut min_x = f32::INFINITY;
	let mut max_x = f32::NEG_INFINITY;
	for &v in &verts {
		min_x = min_x.min(v.x);
		max_x = max_x.max(v.x);
	}
	let cx = (min_x + max_x) * 0.5;

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;
	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	if src_ix >= x_len || src_iy >= y_len || dst_ix >= x_len || dst_iy >= y_len {
		return Err(format!("Keypoint index out of bounds for {x_len}x{y_len} grid"));
	}

	let binding = param
		.bindings
		.iter_mut()
		.find(|b| b.node == node_id && matches!(b.values, BindingValues::Deform(_)))
		.ok_or_else(|| format!("Node {} has no Deform binding on '{param_name}'", node_id.0))?;

	let BindingValues::Deform(ref mut mat) = binding.values else {
		unreachable!();
	};
	let src_offsets = mat
		.get(src_ix, src_iy)
		.ok_or_else(|| "Source keypoint missing".to_owned())?
		.clone();
	if src_offsets.len() != verts.len() {
		return Err(format!(
			"Deform buffer length {} mismatches rest vertices {}",
			src_offsets.len(),
			verts.len()
		));
	}

	let new_dst: Vec<Vec2> = if mirror_x {
		verts
			.iter()
			.map(|&v| {
				let target = vec2(2.0 * cx - v.x, v.y);
				let nearest = verts
					.iter()
					.enumerate()
					.min_by(|(_, a), (_, b)| (**a - target).length_squared().total_cmp(&(**b - target).length_squared()))
					.map(|(j, _)| j)
					.unwrap_or(0);
				let d = src_offsets[nearest];
				vec2(-d.x, d.y)
			})
			.collect()
	} else {
		src_offsets
	};

	if let Some(dst_slot) = mat.get_mut(dst_ix, dst_iy) {
		*dst_slot = new_dst;
	}
	if let Some(flag) = binding.is_set.get_mut(dst_ix, dst_iy) {
		*flag = true;
	}
	Ok(verts.len())
}

/// Synthesize non-cardinal 2D corner keypoints from orthogonal cross axes using additive
/// Live2D-style corner synthesis: `Corner(ix, iy) = AxisX(ix, cy) + AxisY(cx, iy) - Center(cx, cy)`.
pub fn synthesize_2d_corners(
	puppet: &mut Puppet,
	param_name: &str,
	target_node: Option<InoxNodeUuid>,
) -> Result<(usize, usize), String> {
	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;
	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	if x_len < 2 || y_len < 2 {
		return Err(format!(
			"Parameter '{param_name}' must be at least 2x2 to synthesize corners (got {x_len}x{y_len})"
		));
	}

	let span = param.max - param.min;
	let def_nx = if span.x.abs() > 1e-6 {
		((param.defaults.x - param.min.x) / span.x).clamp(0.0, 1.0)
	} else {
		0.5
	};
	let def_ny = if span.y.abs() > 1e-6 {
		((param.defaults.y - param.min.y) / span.y).clamp(0.0, 1.0)
	} else {
		0.5
	};
	let cx = param
		.axis_points
		.x
		.iter()
		.enumerate()
		.min_by(|a, b| (a.1 - def_nx).abs().total_cmp(&(b.1 - def_nx).abs()))
		.map(|(i, _)| i)
		.unwrap_or(0);
	let cy = param
		.axis_points
		.y
		.iter()
		.enumerate()
		.min_by(|a, b| (a.1 - def_ny).abs().total_cmp(&(b.1 - def_ny).abs()))
		.map(|(i, _)| i)
		.unwrap_or(0);

	let mut bindings_updated = 0usize;
	let mut cells_synthesized = 0usize;

	for binding in &mut param.bindings {
		if target_node.is_some_and(|nid| binding.node != nid) {
			continue;
		}
		let mut touched_binding = false;
		for iy in 0..y_len {
			for ix in 0..x_len {
				if ix == cx || iy == cy {
					continue;
				}
				let set_x = binding.is_set.get(ix, cy).copied().unwrap_or(false);
				let set_y = binding.is_set.get(cx, iy).copied().unwrap_or(false);
				if !set_x && !set_y {
					continue;
				}
				match &mut binding.values {
					BindingValues::Deform(mat) => {
						let row_c = mat.get(ix, cy).cloned().unwrap_or_default();
						let col_c = mat.get(cx, iy).cloned().unwrap_or_default();
						let ctr = mat.get(cx, cy).cloned().unwrap_or_default();
						if let Some(dst) = mat.get_mut(ix, iy) {
							let n = dst.len().min(row_c.len()).min(col_c.len());
							for vi in 0..n {
								let c = ctr.get(vi).copied().unwrap_or(Vec2::ZERO);
								dst[vi] = row_c[vi] + col_c[vi] - c;
							}
							touched_binding = true;
							cells_synthesized += 1;
						}
					}
					other => {
						let kind = binding_kind_name(other);
						if let Some(mat) = scalar_matrix_mut(other, kind) {
							let vx = mat.get(ix, cy).copied().unwrap_or(0.0);
							let vy = mat.get(cx, iy).copied().unwrap_or(0.0);
							let vc = mat.get(cx, cy).copied().unwrap_or(0.0);
							let mut val = vx + vy - vc;
							if kind == "Opacity" {
								val = val.clamp(0.0, 1.0);
							} else if kind == "TransformSX" || kind == "TransformSY" {
								val = val.max(0.01);
							}
							if let Some(dst) = mat.get_mut(ix, iy) {
								*dst = val;
								touched_binding = true;
								cells_synthesized += 1;
							}
						}
					}
				}
				if touched_binding {
					if let Some(flag) = binding.is_set.get_mut(ix, iy) {
						*flag = true;
					}
				}
			}
		}
		if touched_binding {
			bindings_updated += 1;
		}
	}

	Ok((bindings_updated, cells_synthesized))
}

/// Live2D Glue-style cross-node seam welding at a parameter keypoint: finds vertex pairs between
/// `node_a` and `node_b` within `max_dist_px` in world space and pulls them toward their weighted
/// midpoint `mix(world_a, world_b, weight)` in each node's local `Deform` binding.
pub fn weld_seam_vertices(
	puppet: &mut Puppet,
	node_a: InoxNodeUuid,
	node_b: InoxNodeUuid,
	param_name: &str,
	param_val: Vec2,
	max_dist_px: f32,
	weight: f32,
) -> Result<(usize, usize, usize), String> {
	if node_a == node_b {
		return Err("node_a and node_b must be distinct nodes".to_owned());
	}
	if puppet.render_ctx.is_none() {
		puppet.init_all();
	}
	puppet.begin_frame();
	if let Some(pctx) = puppet.param_ctx.as_mut() {
		let _ = pctx.set(param_name, param_val);
	}
	puppet.end_frame(0.0);

	let verts_a = node_deform_rest_vertices(puppet, node_a)?;
	let verts_b = node_deform_rest_vertices(puppet, node_b)?;
	let origin_a = puppet
		.node_comps
		.get::<Mesh>(node_a)
		.map(|m| m.origin)
		.unwrap_or(Vec2::ZERO);
	let origin_b = puppet
		.node_comps
		.get::<Mesh>(node_b)
		.map(|m| m.origin)
		.unwrap_or(Vec2::ZERO);
	let abs_a = puppet
		.node_comps
		.get::<TransformStore>(node_a)
		.map(|t| t.absolute)
		.unwrap_or(glam::Mat4::IDENTITY);
	let abs_b = puppet
		.node_comps
		.get::<TransformStore>(node_b)
		.map(|t| t.absolute)
		.unwrap_or(glam::Mat4::IDENTITY);

	let (ba, ix, iy) = ensure_deform_binding_keypoint(puppet, node_a, param_name, param_val, verts_a.len())?;
	let (bb, _, _) = ensure_deform_binding_keypoint(puppet, node_b, param_name, param_val, verts_b.len())?;

	let offs_a = {
		let param = puppet.params.get(param_name).unwrap();
		if let BindingValues::Deform(ref m) = param.bindings[ba].values {
			m.get(ix, iy).cloned().unwrap_or_else(|| vec![Vec2::ZERO; verts_a.len()])
		} else {
			vec![Vec2::ZERO; verts_a.len()]
		}
	};
	let offs_b = {
		let param = puppet.params.get(param_name).unwrap();
		if let BindingValues::Deform(ref m) = param.bindings[bb].values {
			m.get(ix, iy).cloned().unwrap_or_else(|| vec![Vec2::ZERO; verts_b.len()])
		} else {
			vec![Vec2::ZERO; verts_b.len()]
		}
	};

	let world_a: Vec<Vec2> = verts_a
		.iter()
		.enumerate()
		.map(|(i, &v)| {
			let d = offs_a.get(i).copied().unwrap_or(Vec2::ZERO);
			abs_a
				.transform_point3(glam::vec3(v.x - origin_a.x + d.x, v.y - origin_a.y + d.y, 0.0))
				.truncate()
		})
		.collect();
	let world_b: Vec<Vec2> = verts_b
		.iter()
		.enumerate()
		.map(|(i, &v)| {
			let d = offs_b.get(i).copied().unwrap_or(Vec2::ZERO);
			abs_b
				.transform_point3(glam::vec3(v.x - origin_b.x + d.x, v.y - origin_b.y + d.y, 0.0))
				.truncate()
		})
		.collect();

	let inv_a = abs_a.inverse();
	let inv_b = abs_b.inverse();
	let w = weight.clamp(0.0, 1.0);
	let max_d2 = max_dist_px.max(0.5).powi(2);

	let mut delta_a = vec![Vec2::ZERO; verts_a.len()];
	let mut delta_b = vec![Vec2::ZERO; verts_b.len()];
	let mut count_b = vec![0u32; verts_b.len()];
	let mut pairs = 0usize;

	for (ia, &wa) in world_a.iter().enumerate() {
		let mut best: Option<(usize, f32)> = None;
		for (ib, &wb) in world_b.iter().enumerate() {
			let d2 = (wa - wb).length_squared();
			if d2 <= max_d2 && best.is_none_or(|(_, bd2)| d2 < bd2) {
				best = Some((ib, d2));
			}
		}
		if let Some((ib, _)) = best {
			let wb = world_b[ib];
			let target_w = wa.lerp(wb, w);
			let target_la = inv_a.transform_point3(target_w.extend(0.0)).truncate() + origin_a;
			let target_lb = inv_b.transform_point3(target_w.extend(0.0)).truncate() + origin_b;
			let cur_la = verts_a[ia] + offs_a[ia];
			let cur_lb = verts_b[ib] + offs_b[ib];
			delta_a[ia] = target_la - cur_la;
			delta_b[ib] += target_lb - cur_lb;
			count_b[ib] += 1;
			pairs += 1;
		}
	}

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;
	if let BindingValues::Deform(ref mut mat) = param.bindings[ba].values {
		if let Some(cell) = mat.get_mut(ix, iy) {
			for (i, d) in delta_a.into_iter().enumerate() {
				if let Some(slot) = cell.get_mut(i) {
					*slot += d;
				}
			}
		}
	}
	if let BindingValues::Deform(ref mut mat) = param.bindings[bb].values {
		if let Some(cell) = mat.get_mut(ix, iy) {
			for (i, d) in delta_b.into_iter().enumerate() {
				if count_b[i] > 0 {
					if let Some(slot) = cell.get_mut(i) {
						*slot += d / (count_b[i] as f32);
					}
				}
			}
		}
	}

	Ok((ix, iy, pairs))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn smooth_zero_delta_is_noop_and_bulge_handles_diagonal_drag() {
		let mut puppet = crate::engine::Session::new_sample().model.puppet;
		let node_id = puppet
			.nodes
			.iter()
			.find(|n| puppet.node_comps.get::<Mesh>(n.uuid).is_some())
			.map(|n| n.uuid)
			.expect("sample puppet has a Mesh node");
		let param_name = puppet
			.params
			.keys()
			.next()
			.cloned()
			.expect("sample puppet has a parameter");

		let (_, _, affected_smooth0) = apply_deform_brush_mode(
			&mut puppet,
			node_id,
			&param_name,
			Vec2::ZERO,
			Some("center"),
			None,
			0.5,
			Vec2::ZERO,
			DeformBrushMode::Smooth,
		)
		.unwrap();
		assert_eq!(affected_smooth0, 0);

		let (_, _, affected_diag) = apply_deform_brush_mode(
			&mut puppet,
			node_id,
			&param_name,
			Vec2::ZERO,
			Some("center"),
			None,
			0.8,
			vec2(10.0, 10.0),
			DeformBrushMode::Bulge,
		)
		.unwrap();
		assert!(affected_diag > 0);
	}
}

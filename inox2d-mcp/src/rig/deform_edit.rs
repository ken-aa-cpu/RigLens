use glam::{vec2, Vec2};
use inox2d::math::interp::InterpolateMode;
use inox2d::math::matrix::Matrix2d;
use inox2d::node::components::{GridDeformer, Mesh, PathDeformer};
use inox2d::node::InoxNodeUuid;
use inox2d::params::{AxisPoints, Binding, BindingValues};
use inox2d::puppet::Puppet;

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
	let verts = node_deform_rest_vertices(puppet, node_id)?;

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

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;
	let mut affected = 0usize;
	if let BindingValues::Deform(ref mut mat) = param.bindings[b_idx].values {
		if let Some(cell) = mat.get_mut(ix, iy) {
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

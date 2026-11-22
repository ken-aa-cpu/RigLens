use std::collections::HashMap;

use glam::{vec2, Vec2};

use crate::math::{
	deform::Deform,
	interp::{Cell, InterpRange, InterpolateMode},
	matrix::Matrix2d,
};
use crate::node::{
	components::{DeformSource, DeformStack, GridDeformer, Mesh, OpacityStore, PathDeformer, TransformStore, ZSort},
	InoxNodeUuid,
};
use crate::puppet::{InoxNodeTree, Puppet, World};

/// Parameter binding to a node. This allows to animate a node based on the value of the parameter that owns it.
#[derive(Debug, Clone)]
pub struct Binding {
	pub node: InoxNodeUuid,
	pub is_set: Matrix2d<bool>,
	pub interpolate_mode: InterpolateMode,
	pub values: BindingValues,
}

#[derive(Debug, Clone)]
pub enum BindingValues {
	ZSort(Matrix2d<f32>),
	TransformTX(Matrix2d<f32>),
	TransformTY(Matrix2d<f32>),
	TransformSX(Matrix2d<f32>),
	TransformSY(Matrix2d<f32>),
	TransformRX(Matrix2d<f32>),
	TransformRY(Matrix2d<f32>),
	TransformRZ(Matrix2d<f32>),
	Deform(Matrix2d<Vec<Vec2>>),
	Opacity(Matrix2d<f32>),
}

#[derive(Debug, Clone)]
pub struct AxisPoints {
	pub x: Vec<f32>,
	pub y: Vec<f32>,
}

fn find_axis_cell(axis: &[f32], t: f32) -> (usize, usize) {
	let last_idx = axis.len().saturating_sub(1);
	if last_idx == 0 {
		return (0, 0);
	}
	match axis.binary_search_by(|a| a.total_cmp(&t)) {
		Ok(ind) if ind >= last_idx => (last_idx - 1, last_idx),
		Ok(ind) => (ind, ind + 1),
		Err(0) => (0, 0),
		Err(ind) if ind > last_idx => (last_idx, last_idx),
		Err(ind) => (ind - 1, ind),
	}
}

fn deform_vertex_count(comps: &World, id: InoxNodeUuid) -> Option<usize> {
	if let Some(m) = comps.get::<Mesh>(id) {
		return Some(m.vertices.len());
	}
	if let Some(g) = comps.get::<GridDeformer>(id) {
		return Some(g.vertex_count());
	}
	if let Some(p) = comps.get::<PathDeformer>(id) {
		return Some(p.vertex_count());
	}
	None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamUuid(pub u32);

/// Parameter. A simple bounded value that is used to animate nodes through bindings.
#[derive(Debug, Clone)]
pub struct Param {
	pub uuid: ParamUuid,
	pub name: String,
	pub is_vec2: bool,
	pub min: Vec2,
	pub max: Vec2,
	pub defaults: Vec2,
	pub axis_points: AxisPoints,
	/// True when this parameter behaves as an additive Blend Shape (Live2D Cubism 5 style).
	pub additive: bool,
	pub bindings: Vec<Binding>,
}

impl Param {
	/// Internal function that modifies puppet components according to one param set.
	/// Must be only called ONCE per frame to ensure correct behavior.
	///
	/// End users may repeatedly apply a same parameter for multiple times in between frames,
	/// but other facilities should be present to make sure this `apply()` is only called once per parameter.
	pub(crate) fn apply(&self, val: Vec2, nodes: &InoxNodeTree, comps: &mut World) {
		if self.axis_points.x.is_empty() || self.axis_points.y.is_empty() {
			return;
		}
		let val = val.clamp(self.min, self.max);
		let span = self.max - self.min;
		let val_normed = vec2(
			if span.x.abs() > 1e-8 { (val.x - self.min.x) / span.x } else { 0.0 },
			if span.y.abs() > 1e-8 { (val.y - self.min.y) / span.y } else { 0.0 },
		);

		// calculate axis point indexes
		let (x_mindex, x_maxdex) = find_axis_cell(&self.axis_points.x, val_normed.x);
		let (y_mindex, y_maxdex) = find_axis_cell(&self.axis_points.y, val_normed.y);
		let x_last = self.axis_points.x.len() - 1;
		let y_last = self.axis_points.y.len() - 1;

		// Apply offset on each binding
		for binding in &self.bindings {
			if !binding.is_set.any(|&v| v) {
				continue;
			}
			let range_in = InterpRange::new(
				vec2(self.axis_points.x[x_mindex], self.axis_points.y[y_mindex]),
				vec2(self.axis_points.x[x_maxdex], self.axis_points.y[y_maxdex]),
			);

			let val_normed = val_normed.clamp(range_in.beg, range_in.end);

			let cell = Cell {
				t: val_normed,
				range_in,
				xs: (x_mindex, x_maxdex),
				ys: (y_mindex, y_maxdex),
				x_last,
				y_last,
			};

			let eval_scalar = |matrix: &Matrix2d<f32>, neutral: f32| -> f32 {
				cell.f32_sampled(
					|x, y| {
						if binding.is_set.get(x, y).copied().unwrap_or(false) {
							matrix[(x, y)]
						} else {
							neutral
						}
					},
					binding.interpolate_mode,
				)
			};

			match binding.values {
				BindingValues::ZSort(ref matrix) => {
					if matrix.get(x_last, y_last).is_none() {
						tracing::warn!(param = %self.name, node_uuid = binding.node.0, "Param ZSort matrix smaller than axis_points; skipping.");
						continue;
					}
					let val = eval_scalar(matrix, 0.0);
					if let Some(zsort) = comps.get_mut::<ZSort>(binding.node) {
						zsort.0 += val;
					} else {
						tracing::warn!(
							param = %self.name,
							node_uuid = binding.node.0,
							"Param ZSort binding targets missing node/component; skipping."
						);
					}
				}
				BindingValues::TransformTX(ref matrix)
				| BindingValues::TransformTY(ref matrix)
				| BindingValues::TransformSX(ref matrix)
				| BindingValues::TransformSY(ref matrix)
				| BindingValues::TransformRX(ref matrix)
				| BindingValues::TransformRY(ref matrix)
				| BindingValues::TransformRZ(ref matrix) => {
					if matrix.get(x_last, y_last).is_none() {
						tracing::warn!(param = %self.name, node_uuid = binding.node.0, "Param Transform matrix smaller than axis_points; skipping.");
						continue;
					}
					let is_scale = matches!(
						binding.values,
						BindingValues::TransformSX(_) | BindingValues::TransformSY(_)
					);
					let neutral = if is_scale && !self.additive { 1.0 } else { 0.0 };
					let val = eval_scalar(matrix, neutral);
					let Some(ts) = comps.get_mut::<TransformStore>(binding.node) else {
						tracing::warn!(
							param = %self.name,
							node_uuid = binding.node.0,
							"Param transform binding targets missing node/TransformStore; skipping."
						);
						continue;
					};
					match binding.values {
						BindingValues::TransformTX(_) => ts.relative.translation.x += val,
						BindingValues::TransformTY(_) => ts.relative.translation.y += val,
						BindingValues::TransformSX(_) => {
							if self.additive {
								ts.relative.scale.x += val;
							} else {
								ts.relative.scale.x *= val;
							}
						}
						BindingValues::TransformSY(_) => {
							if self.additive {
								ts.relative.scale.y += val;
							} else {
								ts.relative.scale.y *= val;
							}
						}
						BindingValues::TransformRX(_) => ts.relative.rotation.x += val,
						BindingValues::TransformRY(_) => ts.relative.rotation.y += val,
						BindingValues::TransformRZ(_) => ts.relative.rotation.z += val,
						_ => unreachable!(),
					}
				}
				BindingValues::Deform(ref matrix) => {
					if matrix.get(x_last, y_last).is_none() {
						tracing::warn!(param = %self.name, node_uuid = binding.node.0, "Param Deform matrix smaller than axis_points; skipping.");
						continue;
					}

					// deform specified by a parameter must be direct, i.e., in the form of displacements of all vertices
					let direct_deform = {
						let Some(vert_len) = deform_vertex_count(comps, binding.node) else {
							let target_name = nodes.get_node(binding.node).map(|n| n.name.as_str());
							tracing::error!(
								"Deform param target must have an associated Mesh or deformer. (Param: {}, Binding Node: {} ({:?}))",
								self.name,
								target_name.unwrap_or("<NO NAME>"),
								binding.node.0
							);
							continue;
						};
						if vert_len == 0 {
							continue;
						}

						let mut direct_deform: Vec<Vec2> = vec![Vec2::ZERO; vert_len];

						cell.vec2s_additive_sampled(
							|x, y, i| {
								if binding.is_set.get(x, y).copied().unwrap_or(false) {
									matrix[(x, y)].get(i).copied().unwrap_or(Vec2::ZERO)
								} else {
									Vec2::ZERO
								}
							},
							binding.interpolate_mode,
							&mut direct_deform,
						);

						direct_deform
					};

					if let Some(stack) = comps.get_mut::<DeformStack>(binding.node) {
						stack.push(DeformSource::Param(self.uuid), Deform::Direct(direct_deform));
					}
				}
				BindingValues::Opacity(ref matrix) => {
					if matrix.get(x_last, y_last).is_none() {
						tracing::warn!(param = %self.name, node_uuid = binding.node.0, "Param Opacity matrix smaller than axis_points; skipping.");
						continue;
					}
					let neutral = if self.additive { 0.0 } else { 1.0 };
					let val = eval_scalar(matrix, neutral);
					if let Some(op) = comps.get_mut::<OpacityStore>(binding.node) {
						if self.additive {
							op.0 = (op.0 + val).clamp(0.0, 1.0);
						} else {
							op.0 *= val;
						}
					}
				}
			}
		}
	}
}

/// Additional struct attached to a puppet for animating through params.
pub struct ParamCtx {
	values: HashMap<String, Vec2>,
}

impl ParamCtx {
	pub(crate) fn new(puppet: &Puppet) -> Self {
		Self {
			values: puppet.params.iter().map(|p| (p.0.to_owned(), p.1.defaults)).collect(),
		}
	}

	/// Reset all params to default value.
	pub(crate) fn reset(&mut self, params: &HashMap<String, Param>) {
		for (name, value) in self.values.iter_mut() {
			*value = params.get(name).unwrap().defaults;
		}
	}

	/// Set param with name to value `val`.
	pub fn set(&mut self, param_name: &str, val: Vec2) -> Result<(), SetParamError> {
		if let Some(value) = self.values.get_mut(param_name) {
			*value = val;
			Ok(())
		} else {
			Err(SetParamError::NoParameterNamed(param_name.to_string()))
		}
	}

	/// Get current parameter values.
	pub fn values(&self) -> &HashMap<String, Vec2> {
		&self.values
	}

	/// Modify components as specified by all params. Must be called ONCE per frame.
	pub(crate) fn apply(&self, params: &HashMap<String, Param>, nodes: &InoxNodeTree, comps: &mut World) {
		// a correct implementation should not care about the order of `.apply()`
		for (param_name, val) in self.values.iter() {
			if let Some(param) = params.get(param_name) {
				param.apply(*val, nodes, comps);
			}
		}
	}
}

/// Possible errors setting a param.
#[derive(Debug, thiserror::Error)]
pub enum SetParamError {
	#[error("No parameter named {0}")]
	NoParameterNamed(String),
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::math::transform::TransformOffset;
	use crate::node::InoxNode;

	#[test]
	fn param_apply_clamps_unanchored_axis_points_and_guards_matrix_mismatch() {
		let mut puppet = Puppet::new_empty("AxisClampTest");
		let root = puppet.nodes.root_node_id;
		let child = InoxNodeUuid(5);
		puppet.nodes.add(
			root,
			child,
			InoxNode {
				uuid: child,
				name: "Child".to_owned(),
				enabled: true,
				zsort: 0.0,
				trans_offset: TransformOffset::default(),
				lock_to_root: false,
			},
		);
		puppet.init_transforms();
		puppet.transform_ctx.as_mut().unwrap().reset(&puppet.nodes, &mut puppet.node_comps);

		// axis_points do not start at 0.0 or end at 1.0 -> binary_search_by returns Err(0) and Err(len)
		let param = Param {
			uuid: ParamUuid(1),
			name: "Clamped".to_owned(),
			is_vec2: false,
			min: vec2(0.0, 0.0),
			max: vec2(1.0, 0.0),
			defaults: vec2(0.5, 0.0),
			axis_points: AxisPoints {
				x: vec![0.2, 0.8],
				y: vec![0.25],
			},
			additive: false,
			bindings: vec![
				Binding {
					node: child,
					is_set: Matrix2d::from_grid_transposed(2, 1, |_, _| true),
					interpolate_mode: InterpolateMode::Linear,
					values: BindingValues::TransformTX(Matrix2d::from_grid_transposed(2, 1, |ix, _| {
						if ix == 0 { 10.0 } else { 20.0 }
					})),
				},
				// Mismatched smaller matrix (1x1) must be skipped safely without panicking
				Binding {
					node: child,
					is_set: Matrix2d::from_grid_transposed(1, 1, |_, _| true),
					interpolate_mode: InterpolateMode::Linear,
					values: BindingValues::TransformTY(Matrix2d::from_grid_transposed(1, 1, |_, _| 99.0)),
				},
			],
		};

		param.apply(vec2(0.0, 0.0), &puppet.nodes, &mut puppet.node_comps);
		assert!((puppet.node_transform(child).unwrap().relative.translation.x - 10.0).abs() < 1e-4);

		puppet.transform_ctx.as_mut().unwrap().reset(&puppet.nodes, &mut puppet.node_comps);
		param.apply(vec2(1.0, 0.0), &puppet.nodes, &mut puppet.node_comps);
		assert!((puppet.node_transform(child).unwrap().relative.translation.x - 20.0).abs() < 1e-4);
	}

	#[test]
	fn param_apply_handles_additive_scale_and_unset_is_set_corners() {
		let mut puppet = Puppet::new_empty("AdditiveAndIsSet");
		let root = puppet.nodes.root_node_id;
		let child = InoxNodeUuid(7);
		puppet.nodes.add(
			root,
			child,
			InoxNode {
				uuid: child,
				name: "Child".to_owned(),
				enabled: true,
				zsort: 0.0,
				trans_offset: TransformOffset::default(),
				lock_to_root: false,
			},
		);
		puppet.init_transforms();
		puppet.transform_ctx.as_mut().unwrap().reset(&puppet.nodes, &mut puppet.node_comps);

		// 1) All-false is_set binding must be skipped completely even if matrix holds 99.0.
		let skip_param = Param {
			uuid: ParamUuid(1),
			name: "AllUnset".to_owned(),
			is_vec2: false,
			min: vec2(0.0, 0.0),
			max: vec2(1.0, 0.0),
			defaults: vec2(0.0, 0.0),
			axis_points: AxisPoints {
				x: vec![0.0, 1.0],
				y: vec![0.0],
			},
			additive: false,
			bindings: vec![Binding {
				node: child,
				is_set: Matrix2d::from_grid_transposed(2, 1, |_, _| false),
				interpolate_mode: InterpolateMode::Linear,
				values: BindingValues::TransformTX(Matrix2d::from_grid_transposed(2, 1, |_, _| 99.0)),
			}],
		};
		skip_param.apply(vec2(1.0, 0.0), &puppet.nodes, &mut puppet.node_comps);
		assert_eq!(puppet.node_transform(child).unwrap().relative.translation.x, 0.0);

		// 2) Non-additive TransformSX with corner 0 unset (matrix holds 99.0) and corner 1 = 3.0 ->
		//    midpoint (0.5) interpolates neutral 1.0 and 3.0 -> 2.0.
		let scale_partial = Param {
			uuid: ParamUuid(2),
			name: "ScalePartial".to_owned(),
			is_vec2: false,
			min: vec2(0.0, 0.0),
			max: vec2(1.0, 0.0),
			defaults: vec2(0.0, 0.0),
			axis_points: AxisPoints {
				x: vec![0.0, 1.0],
				y: vec![0.0],
			},
			additive: false,
			bindings: vec![Binding {
				node: child,
				is_set: Matrix2d::from_grid_transposed(2, 1, |ix, _| ix == 1),
				interpolate_mode: InterpolateMode::Linear,
				values: BindingValues::TransformSX(Matrix2d::from_grid_transposed(2, 1, |ix, _| {
					if ix == 0 { 99.0 } else { 3.0 }
				})),
			}],
		};
		scale_partial.apply(vec2(0.5, 0.0), &puppet.nodes, &mut puppet.node_comps);
		assert!((puppet.node_transform(child).unwrap().relative.scale.x - 2.0).abs() < 1e-4);

		// 3) Additive TransformSX adds val onto existing scale (2.0 + 0.5 = 2.5).
		let add_scale = Param {
			uuid: ParamUuid(3),
			name: "AddScale".to_owned(),
			is_vec2: false,
			min: vec2(0.0, 0.0),
			max: vec2(1.0, 0.0),
			defaults: vec2(0.0, 0.0),
			axis_points: AxisPoints {
				x: vec![0.0, 1.0],
				y: vec![0.0],
			},
			additive: true,
			bindings: vec![Binding {
				node: child,
				is_set: Matrix2d::from_grid_transposed(2, 1, |_, _| true),
				interpolate_mode: InterpolateMode::Linear,
				values: BindingValues::TransformSX(Matrix2d::from_grid_transposed(2, 1, |ix, _| {
					if ix == 0 { 0.0 } else { 0.5 }
				})),
			}],
		};
		add_scale.apply(vec2(1.0, 0.0), &puppet.nodes, &mut puppet.node_comps);
		assert!((puppet.node_transform(child).unwrap().relative.scale.x - 2.5).abs() < 1e-4);
	}
}

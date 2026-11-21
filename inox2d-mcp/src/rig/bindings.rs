use glam::{vec2, Vec2};
use inox2d::math::interp::InterpolateMode;
use inox2d::math::matrix::Matrix2d;
use inox2d::node::components::Mesh;
use inox2d::node::InoxNodeUuid;
use inox2d::params::{AxisPoints, Binding, BindingValues, Param, ParamUuid};
use inox2d::puppet::Puppet;

use super::mesh::mesh_bounds;

/// Ensure a parameter exists in `puppet.params` and return its UUID.
pub fn ensure_param(
	puppet: &mut Puppet,
	name: &str,
	is_vec2: bool,
	min: Vec2,
	max: Vec2,
	defaults: Vec2,
	axis_points: AxisPoints,
) -> ParamUuid {
	if let Some(existing) = puppet.params.get(name) {
		return existing.uuid;
	}
	let uuid = puppet.next_param_uuid();
	puppet.params.insert(
		name.to_owned(),
		Param {
			uuid,
			name: name.to_owned(),
			is_vec2,
			min,
			max,
			defaults,
			axis_points,
			additive: false,
			bindings: Vec::new(),
		},
	);
	uuid
}

/// Add the standard Inochi Session VTuber tracking parameter set.
pub fn add_standard_vtuber_params(puppet: &mut Puppet) -> Vec<String> {
	let specs = [
		(
			"Head:: Yaw-Pitch",
			true,
			vec2(-1.0, -1.0),
			vec2(1.0, 1.0),
			vec2(0.0, 0.0),
			AxisPoints {
				x: vec![0.0, 0.5, 1.0],
				y: vec![0.0, 0.5, 1.0],
			},
		),
		(
			"Head:: Roll",
			false,
			vec2(-1.0, 0.0),
			vec2(1.0, 0.0),
			vec2(0.0, 0.0),
			AxisPoints {
				x: vec![0.0, 0.5, 1.0],
				y: vec![0.0],
			},
		),
		(
			"Eye:: Left:: Open",
			false,
			vec2(0.0, 0.0),
			vec2(1.0, 0.0),
			vec2(1.0, 0.0),
			AxisPoints {
				x: vec![0.0, 1.0],
				y: vec![0.0],
			},
		),
		(
			"Eye:: Right:: Open",
			false,
			vec2(0.0, 0.0),
			vec2(1.0, 0.0),
			vec2(1.0, 0.0),
			AxisPoints {
				x: vec![0.0, 1.0],
				y: vec![0.0],
			},
		),
		(
			"Mouth:: Open",
			false,
			vec2(0.0, 0.0),
			vec2(1.0, 0.0),
			vec2(0.0, 0.0),
			AxisPoints {
				x: vec![0.0, 1.0],
				y: vec![0.0],
			},
		),
		(
			"Mouth:: Form",
			false,
			vec2(-1.0, 0.0),
			vec2(1.0, 0.0),
			vec2(0.0, 0.0),
			AxisPoints {
				x: vec![0.0, 0.5, 1.0],
				y: vec![0.0],
			},
		),
		(
			"Eye:: Gaze",
			true,
			vec2(-1.0, -1.0),
			vec2(1.0, 1.0),
			vec2(0.0, 0.0),
			AxisPoints {
				x: vec![0.0, 0.5, 1.0],
				y: vec![0.0, 0.5, 1.0],
			},
		),
		(
			"Breath",
			false,
			vec2(0.0, 0.0),
			vec2(1.0, 0.0),
			vec2(0.0, 0.0),
			AxisPoints {
				x: vec![0.0, 1.0],
				y: vec![0.0],
			},
		),
		(
			"Hair:: Sway",
			true,
			vec2(-1.0, -1.0),
			vec2(1.0, 1.0),
			vec2(0.0, 0.0),
			AxisPoints {
				x: vec![0.0, 0.5, 1.0],
				y: vec![0.0, 0.5, 1.0],
			},
		),
	];

	let mut names = Vec::with_capacity(specs.len());
	for (name, is_vec2, min, max, defaults, axis_points) in specs {
		ensure_param(puppet, name, is_vec2, min, max, defaults, axis_points);
		names.push(name.to_owned());
	}
	names
}

pub(super) fn upsert_binding(param: &mut Param, new_binding: Binding) {
	let target_node = new_binding.node;
	let same_kind = |a: &BindingValues, b: &BindingValues| {
		std::mem::discriminant(a) == std::mem::discriminant(b)
	};
	if let Some(existing) = param
		.bindings
		.iter_mut()
		.find(|b| b.node == target_node && same_kind(&b.values, &new_binding.values))
	{
		*existing = new_binding;
	} else {
		param.bindings.push(new_binding);
	}
}

/// Bind a 3x3 2.5D perspective yaw-pitch deformation onto a Part mesh.
/// Positive `depth_z` moves with the turn direction (e.g. nose/eyes/front hair > 0),
/// negative `depth_z` moves opposite (e.g. back hair/ears < 0).
pub fn apply_head_perspective_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	depth_z: f32,
	yaw_px: f32,
	pitch_px: f32,
) -> Result<(), String> {
	let mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();

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

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;

	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let (min_v, max_v) = mesh_bounds(&mesh);
	let center = (min_v + max_v) * 0.5;
	let half = ((max_v - min_v) * 0.5).max(vec2(1.0, 1.0));

	let values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, iy| {
		let tx = param.axis_points.x[ix];
		let ty = param.axis_points.y[iy];
		let yaw = param.min.x + (param.max.x - param.min.x) * tx;
		let pitch = param.min.y + (param.max.y - param.min.y) * ty;

		mesh.vertices
			.iter()
			.map(|&v| {
				let nx = ((v.x - center.x) / half.x).clamp(-1.0, 1.0);
				let ny = ((v.y - center.y) / half.y).clamp(-1.0, 1.0);

				// Parallax shift + size-aware spherical 2.5D foreshortening
				let parallax_x = yaw * yaw_px * depth_z;
				let parallax_y = pitch * pitch_px * depth_z;
				let shear_x = (yaw_px * 0.12).min(half.x * 0.22);
				let curve_y_amp = (pitch_px * 0.22).min(half.y * 0.25);
				let foreshorten_x = -yaw.abs() * (v.x - center.x) * 0.14 - yaw * (nx * nx) * shear_x;
				let curve_y = pitch * curve_y_amp * (1.0 - nx * nx * 0.65)
					+ yaw * ny * (half.y * 0.07) * nx;
				let foreshorten_y = -pitch.abs() * (v.y - center.y) * 0.10;

				vec2(parallax_x + foreshorten_x, parallax_y + curve_y + foreshorten_y)
			})
			.collect()
	});

	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Deform(values),
		},
	);
	Ok(())
}

/// Bind an eyelid-closure deform onto an Eye Part mesh (`0.0 = closed`, `1.0 = open`).
pub fn apply_blink_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	close_y_ratio: f32,
) -> Result<(), String> {
	let mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();

	ensure_param(
		puppet,
		param_name,
		false,
		vec2(0.0, 0.0),
		vec2(1.0, 0.0),
		vec2(1.0, 0.0),
		AxisPoints {
			x: vec![0.0, 1.0],
			y: vec![0.0],
		},
	);

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;

	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let (min_v, max_v) = mesh_bounds(&mesh);
	let center_x = (min_v.x + max_v.x) * 0.5;
	let half_x = ((max_v.x - min_v.x) * 0.5).max(1.0);
	let y_span = (max_v.y - min_v.y).max(1.0);
	let target_y_base = min_v.y + y_span * close_y_ratio.clamp(0.1, 0.9);

	let values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, _| {
		let tx = param.axis_points.x[ix];
		let open = param.min.x + (param.max.x - param.min.x) * tx;
		let closedness = (1.0 - open).clamp(0.0, 1.0);

		mesh.vertices
			.iter()
			.map(|&v| {
				let nx = ((v.x - center_x) / half_x).clamp(-1.0, 1.0);
				// Gentle downward arc when eye is closed
				let arc_y = target_y_base + (1.0 - nx * nx) * (y_span * 0.06);
				let dy = (arc_y - v.y) * 0.92 * closedness;
				vec2(0.0, dy)
			})
			.collect()
	});

	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Deform(values),
		},
	);
	Ok(())
}

/// Bind a mouth-open deform onto a Mouth Part mesh (`0.0 = closed`, `1.0 = open`).
pub fn apply_mouth_open_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	open_px: f32,
) -> Result<(), String> {
	let mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();

	ensure_param(
		puppet,
		param_name,
		false,
		vec2(0.0, 0.0),
		vec2(1.0, 0.0),
		vec2(0.0, 0.0),
		AxisPoints {
			x: vec![0.0, 1.0],
			y: vec![0.0],
		},
	);

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;

	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let (min_v, max_v) = mesh_bounds(&mesh);
	let center_y = (min_v.y + max_v.y) * 0.5;
	let half_y = ((max_v.y - min_v.y) * 0.5).max(1.0);

	let values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, _| {
		let tx = param.axis_points.x[ix];
		let open = (param.min.x + (param.max.x - param.min.x) * tx).clamp(0.0, 1.0);

		mesh.vertices
			.iter()
			.map(|&v| {
				let ny = ((v.y - center_y) / half_y).clamp(-1.0, 1.0);
				// Upper lip lifts slightly, lower lip drops open
				let dy = if ny >= 0.0 {
					ny * open_px * open
				} else {
					ny * open_px * 0.35 * open
				};
				vec2(0.0, dy)
			})
			.collect()
	});

	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Deform(values),
		},
	);
	Ok(())
}

/// Bind a quadratic pendulum sway deform onto hair/cloth meshes (`min_y` pinned, `max_y` sways).
pub fn apply_sway_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	sway_px: f32,
) -> Result<(), String> {
	let mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();

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

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;

	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let (min_v, max_v) = mesh_bounds(&mesh);
	let y_span = (max_v.y - min_v.y).max(1.0);

	let values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, iy| {
		let tx = param.axis_points.x[ix];
		let ty = param.axis_points.y[iy];
		let sx = param.min.x + (param.max.x - param.min.x) * tx;
		let sy = param.min.y + (param.max.y - param.min.y) * ty;

		mesh.vertices
			.iter()
			.map(|&v| {
				let t = ((v.y - min_v.y) / y_span).clamp(0.0, 1.0);
				let weight = t * t;
				vec2(sx * sway_px * weight, sy * (sway_px * 0.35) * weight)
			})
			.collect()
	});

	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Deform(values),
		},
	);
	Ok(())
}

/// Bind `Mouth:: Form` (`-1.0 = frown/pout`, `1.0 = smile`) onto a Mouth Part mesh.
pub fn apply_mouth_form_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	width_px: f32,
	smile_y_px: f32,
) -> Result<(), String> {
	let mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();

	ensure_param(
		puppet,
		param_name,
		false,
		vec2(-1.0, 0.0),
		vec2(1.0, 0.0),
		vec2(0.0, 0.0),
		AxisPoints {
			x: vec![0.0, 0.5, 1.0],
			y: vec![0.0],
		},
	);

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;

	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let (min_v, max_v) = mesh_bounds(&mesh);
	let center_x = (min_v.x + max_v.x) * 0.5;
	let half_x = ((max_v.x - min_v.x) * 0.5).max(1.0);

	let values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, _| {
		let tx = param.axis_points.x[ix];
		let form = param.min.x + (param.max.x - param.min.x) * tx;
		mesh.vertices
			.iter()
			.map(|&v| {
				let nx = ((v.x - center_x) / half_x).clamp(-1.0, 1.0);
				let corner_weight = nx * nx;
				let dx = nx * width_px * form;
				let dy = -corner_weight * smile_y_px * form;
				vec2(dx, dy)
			})
			.collect()
	});

	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Deform(values),
		},
	);
	Ok(())
}

/// Bind `Eye:: Gaze` (`[-1..1] x [-1..1]`) onto an Eye or Iris Part node.
pub fn apply_eye_gaze_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	gaze_x_px: f32,
	gaze_y_px: f32,
) -> Result<(), String> {
	let mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();

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

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;

	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let (min_v, max_v) = mesh_bounds(&mesh);
	let center = (min_v + max_v) * 0.5;
	let half = ((max_v - min_v) * 0.5).max(vec2(1.0, 1.0));

	let values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, iy| {
		let tx = param.axis_points.x[ix];
		let ty = param.axis_points.y[iy];
		let gx = param.min.x + (param.max.x - param.min.x) * tx;
		let gy = param.min.y + (param.max.y - param.min.y) * ty;
		mesh.vertices
			.iter()
			.map(|&v| {
				let nx = ((v.x - center.x) / half.x).clamp(-1.0, 1.0);
				let ny = ((v.y - center.y) / half.y).clamp(-1.0, 1.0);
				// Center iris vertices shift more than outer sclera boundary
				let w = ((1.0 - nx * nx * 0.7) * (1.0 - ny * ny * 0.7)).max(0.0);
				vec2(gx * gaze_x_px * w, gy * gaze_y_px * w)
			})
			.collect()
	});

	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Deform(values),
		},
	);
	Ok(())
}

/// Bind `Head:: Roll` (`-1..1`) onto a Head group/node via `transform.r.z`.
pub fn apply_head_roll_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	max_deg: f32,
) -> Result<(), String> {
	if puppet.nodes.get_node(node_id).is_none() {
		return Err(format!("Node {} not found", node_id.0));
	}
	ensure_param(
		puppet,
		param_name,
		false,
		vec2(-1.0, 0.0),
		vec2(1.0, 0.0),
		vec2(0.0, 0.0),
		AxisPoints {
			x: vec![0.0, 0.5, 1.0],
			y: vec![0.0],
		},
	);
	let param = puppet.params.get_mut(param_name).unwrap();
	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let max_rad = max_deg.to_radians();

	let values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, _| {
		let tx = param.axis_points.x[ix];
		let roll = param.min.x + (param.max.x - param.min.x) * tx;
		roll * max_rad
	});
	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::TransformRZ(values),
		},
	);
	Ok(())
}

/// Bind `Breath` (`0..1`) onto a Body/Torso node via `transform.s.y` and `transform.t.y`.
pub fn apply_breath_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	scale_amp: f32,
	y_amp: f32,
) -> Result<(), String> {
	if puppet.nodes.get_node(node_id).is_none() {
		return Err(format!("Node {} not found", node_id.0));
	}
	ensure_param(
		puppet,
		param_name,
		false,
		vec2(0.0, 0.0),
		vec2(1.0, 0.0),
		vec2(0.0, 0.0),
		AxisPoints {
			x: vec![0.0, 1.0],
			y: vec![0.0],
		},
	);
	let param = puppet.params.get_mut(param_name).unwrap();
	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();

	let sy_values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, _| {
		let tx = param.axis_points.x[ix];
		let b = param.min.x + (param.max.x - param.min.x) * tx;
		1.0 + b * scale_amp
	});
	let ty_values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, _| {
		let tx = param.axis_points.x[ix];
		let b = param.min.x + (param.max.x - param.min.x) * tx;
		-b * y_amp
	});
	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set: is_set.clone(),
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::TransformSY(sy_values),
		},
	);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::TransformTY(ty_values),
		},
	);
	Ok(())
}

/// Bind a 0..1 opacity toggle/slider parameter onto a node (`BindingValues::Opacity`).
pub fn apply_opacity_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	opacity_at_0: f32,
	opacity_at_1: f32,
) -> Result<(), String> {
	if puppet.nodes.get_node(node_id).is_none() {
		return Err(format!("Node {} not found", node_id.0));
	}
	ensure_param(
		puppet,
		param_name,
		false,
		vec2(0.0, 0.0),
		vec2(1.0, 0.0),
		vec2(0.0, 0.0),
		AxisPoints {
			x: vec![0.0, 1.0],
			y: vec![0.0],
		},
	);
	let param = puppet.params.get_mut(param_name).unwrap();
	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, _| {
		let tx = param.axis_points.x[ix];
		opacity_at_0 + (opacity_at_1 - opacity_at_0) * tx
	});
	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Opacity(values),
		},
	);
	Ok(())
}

/// Bind a 3x3 2D vowel-viseme parameter (`Mouth:: Viseme`, `x=Form [-1..1]`, `y=Open [0..1]`)
/// producing distinct A/I/U/E/O shapes, alongside standard `Mouth:: Open` and `Mouth:: Form`.
pub fn apply_mouth_visemes_2d(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	open_px: f32,
	width_px: f32,
) -> Result<(), String> {
	let mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();

	ensure_param(
		puppet,
		param_name,
		true,
		vec2(-1.0, 0.0),
		vec2(1.0, 1.0),
		vec2(0.0, 0.0),
		AxisPoints {
			x: vec![0.0, 0.5, 1.0],
			y: vec![0.0, 0.5, 1.0],
		},
	);

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;

	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let (min_v, max_v) = mesh_bounds(&mesh);
	let center = (min_v + max_v) * 0.5;
	let half = ((max_v - min_v) * 0.5).max(vec2(1.0, 1.0));

	let values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, iy| {
		let tx = param.axis_points.x[ix];
		let ty = param.axis_points.y[iy];
		let form = param.min.x + (param.max.x - param.min.x) * tx; // -1=U/pucker .. 0=A/O .. +1=I/E smile
		let open = (param.min.y + (param.max.y - param.min.y) * ty).clamp(0.0, 1.0);

		mesh.vertices
			.iter()
			.map(|&v| {
				let nx = ((v.x - center.x) / half.x).clamp(-1.0, 1.0);
				let ny = ((v.y - center.y) / half.y).clamp(-1.0, 1.0);

				// Horizontal stretch for I/E (+form), narrow pucker for U/O (-form)
				let dx = nx * width_px * (form * (1.0 - 0.25 * open) - 0.35 * open * (1.0 - form.max(0.0)));
				// Vertical opening: O/U stay rounder in center, A drops full jaw, I stays wide/thin
				let round_center = 1.0 - 0.35 * nx * nx * (1.0 - form.max(0.0));
				let jaw = if ny >= 0.0 {
					ny * open_px * open * round_center
				} else {
					ny * open_px * 0.35 * open
				};
				// Corner lift on smile (+form)
				let smile = -(nx * nx) * (width_px * 0.55) * form * (1.0 - 0.5 * open);
				vec2(dx, jaw + smile)
			})
			.collect()
	});

	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Deform(values),
		},
	);
	Ok(())
}

/// Bind a 2D eyebrow expression parameter (`x=Angle [-1 angry .. +1 sad]`, `y=Height [-1..1]`).
pub fn apply_brow_binding(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	param_name: &str,
	is_right: bool,
) -> Result<(), String> {
	let mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();

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

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;

	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let (min_v, max_v) = mesh_bounds(&mesh);
	let center = (min_v + max_v) * 0.5;
	let half = ((max_v - min_v) * 0.5).max(vec2(1.0, 1.0));
	let inner_sign = if is_right { -1.0 } else { 1.0 };

	let values = Matrix2d::from_grid_transposed(x_len, y_len, |ix, iy| {
		let tx = param.axis_points.x[ix];
		let ty = param.axis_points.y[iy];
		let angle = param.min.x + (param.max.x - param.min.x) * tx;
		let height = param.min.y + (param.max.y - param.min.y) * ty;

		mesh.vertices
			.iter()
			.map(|&v| {
				let nx = ((v.x - center.x) / half.x).clamp(-1.0, 1.0);
				// inner_nx > 0 means closer to nose bridge
				let inner_nx = nx * inner_sign;
				// +angle (sad) raises inner end, drops outer end; -angle (angry) drops inner end, raises outer
				let dy = -height * (half.y.max(6.0) * 0.9) - angle * inner_nx * (half.y.max(6.0) * 0.85);
				let arch = -(1.0 - nx * nx) * height.max(0.0) * 3.0;
				vec2(0.0, dy + arch)
			})
			.collect()
	});

	let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
	upsert_binding(
		param,
		Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Deform(values),
		},
	);
	Ok(())
}

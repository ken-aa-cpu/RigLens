use std::collections::HashMap;
use std::io::Cursor;

use glam::{vec2, vec3, Vec2};
use image::{ImageFormat, Rgba, RgbaImage};
use inox2d::animation::{AnimationLane, Keyframe, PuppetAnimation};
use inox2d::math::interp::InterpolateMode;
use inox2d::math::matrix::Matrix2d;
use inox2d::math::transform::TransformOffset;
use inox2d::model::{Model, ModelTexture};
use inox2d::node::components::{
	BlendMode, Blending, Drawable, Mesh, PhysicsModel, PhysicsParamMapMode, PhysicsProps, SimplePhysics, TexturedMesh,
};
use inox2d::node::{InoxNode, InoxNodeUuid};
use inox2d::params::{AxisPoints, Binding, BindingValues, Param, ParamUuid};
use inox2d::puppet::Puppet;
use inox2d::texture::TextureId;

/// Generate a subdivided rectangular triangle mesh centered at `(0, 0)` (relative to `origin`).
pub fn generate_grid_mesh(width: f32, height: f32, cols: usize, rows: usize, origin: Vec2) -> Mesh {
	let cols = cols.max(1);
	let rows = rows.max(1);
	let mut vertices = Vec::with_capacity((cols + 1) * (rows + 1));
	let mut uvs = Vec::with_capacity((cols + 1) * (rows + 1));
	let mut indices = Vec::with_capacity(cols * rows * 6);

	for r in 0..=rows {
		let v = r as f32 / rows as f32;
		let y = (v - 0.5) * height;
		for c in 0..=cols {
			let u = c as f32 / cols as f32;
			let x = (u - 0.5) * width;
			vertices.push(vec2(x, y));
			uvs.push(vec2(u, v));
		}
	}

	let stride = cols + 1;
	for r in 0..rows {
		for c in 0..cols {
			let i0 = (r * stride + c) as u32;
			let i1 = i0 + 1;
			let i2 = ((r + 1) * stride + c) as u32;
			let i3 = i2 + 1;
			indices.extend_from_slice(&[i0, i1, i2, i1, i3, i2]);
		}
	}

	Mesh {
		vertices,
		uvs,
		indices,
		origin,
	}
}

/// Generate a contour-aware triangle mesh by keeping only cells that overlap or border
/// non-transparent pixels in `img`, then compacting unused vertices.
pub fn generate_contour_mesh(
	img: &RgbaImage,
	width: f32,
	height: f32,
	cols: usize,
	rows: usize,
	alpha_threshold: u8,
) -> Mesh {
	let cols = cols.max(1);
	let rows = rows.max(1);
	let iw = img.width().max(1);
	let ih = img.height().max(1);

	let mut active = vec![false; cols * rows];
	for r in 0..rows {
		let y0 = ((r as f32 / rows as f32) * ih as f32).floor() as u32;
		let y1 = ((((r + 1) as f32 / rows as f32) * ih as f32).ceil() as u32).min(ih);
		for c in 0..cols {
			let x0 = ((c as f32 / cols as f32) * iw as f32).floor() as u32;
			let x1 = ((((c + 1) as f32 / cols as f32) * iw as f32).ceil() as u32).min(iw);
			let mut hit = false;
			'cell: for py in y0..y1 {
				for px in x0..x1 {
					if img.get_pixel(px, py).0[3] > alpha_threshold {
						hit = true;
						break 'cell;
					}
				}
			}
			active[r * cols + c] = hit;
		}
	}

	// Dilate active mask by 1 cell so anti-aliased edges keep full margin
	let mut dilated = active.clone();
	for r in 0..rows {
		for c in 0..cols {
			if active[r * cols + c] {
				for dr in -1isize..=1 {
					for dc in -1isize..=1 {
						let nr = r as isize + dr;
						let nc = c as isize + dc;
						if nr >= 0 && nr < rows as isize && nc >= 0 && nc < cols as isize {
							dilated[nr as usize * cols + nc as usize] = true;
						}
					}
				}
			}
		}
	}

	// Fallback if image is completely transparent
	if !dilated.iter().any(|&v| v) {
		return generate_grid_mesh(width, height, cols, rows, Vec2::ZERO);
	}

	let stride = cols + 1;
	let mut grid_verts = Vec::with_capacity((cols + 1) * (rows + 1));
	let mut grid_uvs = Vec::with_capacity((cols + 1) * (rows + 1));
	for r in 0..=rows {
		let v = r as f32 / rows as f32;
		let y = (v - 0.5) * height;
		for c in 0..=cols {
			let u = c as f32 / cols as f32;
			let x = (u - 0.5) * width;
			grid_verts.push(vec2(x, y));
			grid_uvs.push(vec2(u, v));
		}
	}

	let mut remap: HashMap<usize, u32> = HashMap::new();
	let mut vertices = Vec::new();
	let mut uvs = Vec::new();
	let mut indices = Vec::new();

	let mut get_or_insert = |idx: usize| -> u32 {
		if let Some(&mapped) = remap.get(&idx) {
			return mapped;
		}
		let new_idx = vertices.len() as u32;
		vertices.push(grid_verts[idx]);
		uvs.push(grid_uvs[idx]);
		remap.insert(idx, new_idx);
		new_idx
	};

	for r in 0..rows {
		for c in 0..cols {
			if !dilated[r * cols + c] {
				continue;
			}
			let i0 = get_or_insert(r * stride + c);
			let i1 = get_or_insert(r * stride + c + 1);
			let i2 = get_or_insert((r + 1) * stride + c);
			let i3 = get_or_insert((r + 1) * stride + c + 1);
			indices.extend_from_slice(&[i0, i1, i2, i1, i3, i2]);
		}
	}

	Mesh {
		vertices,
		uvs,
		indices,
		origin: Vec2::ZERO,
	}
}

/// Encode an `RgbaImage` as PNG bytes for `ModelTexture`.
pub fn encode_rgba_png(img: &RgbaImage) -> ModelTexture {
	let mut buf = Vec::new();
	img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
		.expect("PNG encoding in memory cannot fail");
	ModelTexture {
		format: ImageFormat::Png,
		data: buf.into(),
	}
}

/// Add a textured mesh `Part` node with an auto-generated subdivided grid mesh.
pub fn add_grid_part(
	model: &mut Model,
	parent: InoxNodeUuid,
	name: impl Into<String>,
	zsort: f32,
	translation: Vec2,
	width: f32,
	height: f32,
	cols: usize,
	rows: usize,
	img: RgbaImage,
) -> InoxNodeUuid {
	let tex_idx = model.textures.len();
	model.textures.push(encode_rgba_png(&img));

	let id = model.puppet.next_node_uuid();
	model.puppet.add_node(
		parent,
		InoxNode {
			uuid: id,
			name: name.into(),
			enabled: true,
			zsort,
			trans_offset: TransformOffset {
				translation: vec3(translation.x, translation.y, 0.0),
				..TransformOffset::default()
			},
			lock_to_root: false,
		},
	);

	model.puppet.node_comps.add(
		id,
		Drawable {
			blending: Blending {
				mode: BlendMode::Normal,
				tint: vec3(1.0, 1.0, 1.0),
				screen_tint: vec3(0.0, 0.0, 0.0),
				opacity: 1.0,
			},
			masks: None,
		},
	);
	model.puppet.node_comps.add(
		id,
		TexturedMesh {
			tex_albedo: TextureId(tex_idx),
			tex_emissive: TextureId(0),
			tex_bumpmap: TextureId(0),
		},
	);
	model
		.puppet
		.node_comps
		.add(id, generate_grid_mesh(width, height, cols, rows, Vec2::ZERO));

	id
}

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

fn upsert_binding(param: &mut Param, new_binding: Binding) {
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

fn mesh_bounds(mesh: &Mesh) -> (Vec2, Vec2) {
	let mut min_v = vec2(f32::INFINITY, f32::INFINITY);
	let mut max_v = vec2(f32::NEG_INFINITY, f32::NEG_INFINITY);
	for &v in &mesh.vertices {
		min_v = min_v.min(v);
		max_v = max_v.max(v);
	}
	(min_v, max_v)
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

				// Parallax shift + spherical 2.5D foreshortening
				let parallax_x = yaw * yaw_px * depth_z;
				let parallax_y = pitch * pitch_px * depth_z;
				let foreshorten_x = -yaw.abs() * (v.x - center.x) * 0.14 - yaw * (nx * nx) * yaw_px * 0.12;
				let curve_y = pitch * pitch_px * 0.22 * (1.0 - nx * nx * 0.65)
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

/// Attach a `SimplePhysics` pendulum driver node under `parent` targeting `param_name`.
pub fn add_pendulum_physics(
	puppet: &mut Puppet,
	parent: InoxNodeUuid,
	name: impl Into<String>,
	param_name: &str,
	model_type: PhysicsModel,
	length: f32,
	frequency: f32,
	angle_damping: f32,
	output_scale: Vec2,
) -> Result<InoxNodeUuid, String> {
	let param_uuid = puppet
		.params
		.get(param_name)
		.map(|p| p.uuid)
		.ok_or_else(|| format!("Parameter '{param_name}' does not exist"))?;

	let id = puppet.next_node_uuid();
	puppet.add_node(
		parent,
		InoxNode {
			uuid: id,
			name: name.into(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset::default(),
			lock_to_root: false,
		},
	);
	puppet.node_comps.add(
		id,
		SimplePhysics {
			param: param_uuid,
			model_type,
			map_mode: PhysicsParamMapMode::XY,
			props: PhysicsProps {
				gravity: 1.0,
				length,
				frequency,
				angle_damping,
				length_damping: 0.5,
				output_scale,
			},
			local_only: false,
		},
	);
	Ok(id)
}

// --- Procedural Texture Helpers for Sample VTuber Model ---

fn paint_ellipse_part(w: u32, h: u32, fill: [u8; 4], border: [u8; 4], highlight: Option<[u8; 4]>) -> RgbaImage {
	let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
	let cx = w as f32 * 0.5;
	let cy = h as f32 * 0.5;
	let rx = w as f32 * 0.46;
	let ry = h as f32 * 0.46;
	for y in 0..h {
		for x in 0..w {
			let dx = (x as f32 + 0.5 - cx) / rx;
			let dy = (y as f32 + 0.5 - cy) / ry;
			let d = (dx * dx + dy * dy).sqrt();
			if d <= 1.0 {
				let c = if d > 0.90 {
					border
				} else if let Some(hl) = highlight {
					if dy < -0.25 && dx.abs() < 0.55 {
						hl
					} else {
						fill
					}
				} else {
					fill
				};
				img.put_pixel(x, y, Rgba(c));
			}
		}
	}
	img
}

fn paint_eye_texture(w: u32, h: u32) -> RgbaImage {
	let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
	let cx = w as f32 * 0.5;
	let cy = h as f32 * 0.52;
	let rx = w as f32 * 0.44;
	let ry = h as f32 * 0.40;
	for y in 0..h {
		for x in 0..w {
			let dx = (x as f32 + 0.5 - cx) / rx;
			let dy = (y as f32 + 0.5 - cy) / ry;
			let d = (dx * dx + dy * dy).sqrt();
			if d <= 1.0 {
				// Sclera
				let mut col = [250, 250, 255, 255];
				// Iris
				let iris_d = ((dx / 0.56).powi(2) + ((dy - 0.05) / 0.72).powi(2)).sqrt();
				if iris_d < 1.0 {
					col = if dy < 0.0 {
						[55, 115, 215, 255]
					} else {
						[105, 185, 245, 255]
					};
					// Pupil
					if iris_d < 0.42 {
						col = [25, 30, 65, 255];
					}
				}
				// Specular highlights
				let hl1 = ((dx + 0.22).powi(2) + (dy + 0.24).powi(2)).sqrt();
				let hl2 = ((dx - 0.20).powi(2) + (dy - 0.20).powi(2)).sqrt();
				if hl1 < 0.16 || hl2 < 0.09 {
					col = [255, 255, 255, 255];
				}
				// Upper eyelash
				if dy < -0.58 || d > 0.88 && dy < 0.1 {
					col = [42, 32, 48, 255];
				}
				img.put_pixel(x, y, Rgba(col));
			}
		}
	}
	img
}

fn paint_mouth_texture(w: u32, h: u32) -> RgbaImage {
	let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
	let cx = w as f32 * 0.5;
	let cy = h as f32 * 0.5;
	let rx = w as f32 * 0.42;
	let ry = h as f32 * 0.36;
	for y in 0..h {
		for x in 0..w {
			let dx = (x as f32 + 0.5 - cx) / rx;
			let dy = (y as f32 + 0.5 - cy) / ry;
			let d = (dx * dx + dy * dy).sqrt();
			if d <= 1.0 {
				let c = if d > 0.80 {
					[145, 65, 78, 255]
				} else if dy > 0.25 {
					[235, 125, 145, 255]
				} else {
					[180, 65, 85, 255]
				};
				img.put_pixel(x, y, Rgba(c));
			}
		}
	}
	img
}

fn paint_hair_strand(w: u32, h: u32, base: [u8; 4], tip: [u8; 4]) -> RgbaImage {
	let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
	let cx = w as f32 * 0.5;
	for y in 0..h {
		let ty = y as f32 / (h as f32 - 1.0);
		// Taper width near bottom tip
		let half_w = w as f32 * 0.45 * (1.0 - ty.powf(1.8) * 0.85);
		for x in 0..w {
			let dx = ((x as f32 + 0.5) - cx).abs();
			if dx <= half_w {
				let edge = dx / half_w.max(1.0);
				let r = ((1.0 - ty) * base[0] as f32 + ty * tip[0] as f32) as u8;
				let g = ((1.0 - ty) * base[1] as f32 + ty * tip[1] as f32) as u8;
				let b = ((1.0 - ty) * base[2] as f32 + ty * tip[2] as f32) as u8;
				let c = if edge > 0.88 { [48, 38, 72, 255] } else { [r, g, b, 255] };
				img.put_pixel(x, y, Rgba(c));
			}
		}
	}
	img
}

/// Build a complete, rigged sample VTuber Model ready for `.inp` export,
/// headless rendering, and interactive MCP manipulation.
pub fn build_sample_vtuber_model() -> Model {
	let mut model = Model {
		puppet: Puppet::new_empty("InochiSampleVTuber"),
		textures: Vec::new(),
		vendors: Vec::new(),
	};

	let root_id = model.puppet.nodes.root_node_id;

	// Create Body & Head hierarchy
	let body_group = model.puppet.next_node_uuid();
	model.puppet.add_node(
		root_id,
		InoxNode {
			uuid: body_group,
			name: "Body".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset::default(),
			lock_to_root: false,
		},
	);

	let torso_img = paint_ellipse_part(160, 140, [70, 95, 165, 255], [40, 45, 85, 255], Some([110, 140, 210, 255]));
	let _torso = add_grid_part(
		&mut model,
		body_group,
		"Torso",
		10.0,
		vec2(0.0, 130.0),
		180.0,
		150.0,
		4,
		4,
		torso_img,
	);

	let head_group = model.puppet.next_node_uuid();
	model.puppet.add_node(
		body_group,
		InoxNode {
			uuid: head_group,
			name: "Head".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset {
				translation: vec3(0.0, -25.0, 0.0),
				..TransformOffset::default()
			},
			lock_to_root: false,
		},
	);

	// Back Hair (zsort higher = drawn behind in Inochi2D descending zsort order)
	let hair_back_img = paint_hair_strand(200, 220, [75, 65, 130, 255], [115, 95, 185, 255]);
	let hair_back = add_grid_part(
		&mut model,
		head_group,
		"Hair_Back",
		20.0,
		vec2(0.0, 35.0),
		230.0,
		250.0,
		6,
		8,
		hair_back_img,
	);

	// Face Base
	let face_img = paint_ellipse_part(180, 180, [255, 228, 212, 255], [185, 135, 125, 255], None);
	let face = add_grid_part(
		&mut model,
		head_group,
		"Face",
		0.0,
		vec2(0.0, 0.0),
		170.0,
		175.0,
		6,
		6,
		face_img,
	);

	// Left & Right Eyes (drawn in front of Face => lower zsort)
	let eye_l_img = paint_eye_texture(64, 56);
	let eye_l = add_grid_part(
		&mut model,
		head_group,
		"Eye_L",
		-5.0,
		vec2(-38.0, -6.0),
		46.0,
		40.0,
		4,
		4,
		eye_l_img,
	);
	let eye_r_img = paint_eye_texture(64, 56);
	let eye_r = add_grid_part(
		&mut model,
		head_group,
		"Eye_R",
		-5.0,
		vec2(38.0, -6.0),
		46.0,
		40.0,
		4,
		4,
		eye_r_img,
	);

	// Mouth
	let mouth_img = paint_mouth_texture(48, 32);
	let mouth = add_grid_part(
		&mut model,
		head_group,
		"Mouth",
		-6.0,
		vec2(0.0, 42.0),
		36.0,
		22.0,
		4,
		4,
		mouth_img,
	);

	// Front Hair Strands (drawn in front of face & eyes)
	let hair_fl_img = paint_hair_strand(72, 130, [92, 78, 155, 255], [140, 118, 215, 255]);
	let hair_fl = add_grid_part(
		&mut model,
		head_group,
		"Hair_Front_L",
		-10.0,
		vec2(-56.0, -18.0),
		64.0,
		135.0,
		4,
		6,
		hair_fl_img,
	);
	let hair_fr_img = paint_hair_strand(72, 130, [92, 78, 155, 255], [140, 118, 215, 255]);
	let hair_fr = add_grid_part(
		&mut model,
		head_group,
		"Hair_Front_R",
		-10.0,
		vec2(56.0, -18.0),
		64.0,
		135.0,
		4,
		6,
		hair_fr_img,
	);

	// Create Standard VTuber Parameters
	add_standard_vtuber_params(&mut model.puppet);

	// Apply 2.5D Head Yaw-Pitch bindings across head parts with layered depths
	let _ = apply_head_perspective_binding(&mut model.puppet, hair_back, "Head:: Yaw-Pitch", -0.8, 28.0, 20.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, face, "Head:: Yaw-Pitch", 0.25, 24.0, 18.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, eye_l, "Head:: Yaw-Pitch", 1.0, 26.0, 20.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, eye_r, "Head:: Yaw-Pitch", 1.0, 26.0, 20.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, mouth, "Head:: Yaw-Pitch", 1.1, 26.0, 20.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, hair_fl, "Head:: Yaw-Pitch", 1.25, 26.0, 20.0);
	let _ = apply_head_perspective_binding(&mut model.puppet, hair_fr, "Head:: Yaw-Pitch", 1.25, 26.0, 20.0);

	// Bind blinks & mouth open
	let _ = apply_blink_binding(&mut model.puppet, eye_l, "Eye:: Left:: Open", 0.65);
	let _ = apply_blink_binding(&mut model.puppet, eye_r, "Eye:: Right:: Open", 0.65);
	let _ = apply_mouth_open_binding(&mut model.puppet, mouth, "Mouth:: Open", 12.0);

	// Bind Hair Sway + SimplePhysics Pendulum
	let _ = apply_sway_binding(&mut model.puppet, hair_back, "Hair:: Sway", 24.0);
	let _ = apply_sway_binding(&mut model.puppet, hair_fl, "Hair:: Sway", 18.0);
	let _ = apply_sway_binding(&mut model.puppet, hair_fr, "Hair:: Sway", 18.0);
	let _ = add_pendulum_physics(
		&mut model.puppet,
		head_group,
		"Hair_Physics",
		"Hair:: Sway",
		PhysicsModel::RigidPendulum,
		80.0,
		2.2,
		0.45,
		vec2(1.0, 1.0),
	);

	let _ = apply_mouth_form_binding(&mut model.puppet, mouth, "Mouth:: Form", 6.0, 5.0);
	let _ = apply_head_roll_binding(&mut model.puppet, head_group, "Head:: Roll", 15.0);
	let _ = apply_breath_binding(&mut model.puppet, body_group, "Breath", 0.02, 4.0);
	create_idle_animation(&mut model.puppet);
	crate::audit::auto_fix_puppet(&mut model.puppet);

	model
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

/// Create a 120-frame @ 30fps looping `"Idle"` animation driving breathing, blinking, and gentle head sway.
pub fn create_idle_animation(puppet: &mut Puppet) -> String {
	let mut lanes = Vec::new();

	if let Some(p) = puppet.params.get("Breath") {
		lanes.push(AnimationLane {
			interpolation: InterpolateMode::Linear,
			param_uuid: p.uuid,
			target: 0,
			merge_mode: "Additive".to_owned(),
			keyframes: vec![
				Keyframe { frame: 0, value: 0.0, tension: 0.6 },
				Keyframe { frame: 60, value: 1.0, tension: 0.6 },
				Keyframe { frame: 120, value: 0.0, tension: 0.6 },
			],
		});
	}

	if let Some(p) = puppet.params.get("Head:: Yaw-Pitch") {
		lanes.push(AnimationLane {
			interpolation: InterpolateMode::Linear,
			param_uuid: p.uuid,
			target: 0,
			merge_mode: "Additive".to_owned(),
			keyframes: vec![
				Keyframe { frame: 0, value: 0.0, tension: 0.6 },
				Keyframe { frame: 30, value: 0.35, tension: 0.6 },
				Keyframe { frame: 60, value: 0.0, tension: 0.6 },
				Keyframe { frame: 90, value: -0.35, tension: 0.6 },
				Keyframe { frame: 120, value: 0.0, tension: 0.6 },
			],
		});
		lanes.push(AnimationLane {
			interpolation: InterpolateMode::Linear,
			param_uuid: p.uuid,
			target: 1,
			merge_mode: "Additive".to_owned(),
			keyframes: vec![
				Keyframe { frame: 0, value: 0.0, tension: 0.6 },
				Keyframe { frame: 45, value: -0.20, tension: 0.6 },
				Keyframe { frame: 90, value: 0.20, tension: 0.6 },
				Keyframe { frame: 120, value: 0.0, tension: 0.6 },
			],
		});
	}

	for eye_param in ["Eye:: Left:: Open", "Eye:: Right:: Open"] {
		if let Some(p) = puppet.params.get(eye_param) {
			lanes.push(AnimationLane {
				interpolation: InterpolateMode::Linear,
				param_uuid: p.uuid,
				target: 0,
				merge_mode: "Additive".to_owned(),
				keyframes: vec![
					Keyframe { frame: 0, value: 1.0, tension: 0.5 },
					Keyframe { frame: 46, value: 1.0, tension: 0.5 },
					Keyframe { frame: 50, value: 0.0, tension: 0.5 },
					Keyframe { frame: 54, value: 1.0, tension: 0.5 },
					Keyframe { frame: 120, value: 1.0, tension: 0.5 },
				],
			});
		}
	}

	puppet.animations.insert(
		"Idle".to_owned(),
		PuppetAnimation {
			timestep: 1.0 / 30.0,
			additive: false,
			length: 120,
			lead_in: 0,
			lead_out: 0,
			lanes,
		},
	);
	"Idle".to_owned()
}

/// Automatically rig all Part/Group nodes in `puppet` by semantic layer naming (EN/ZH/JP-romaji).
pub fn auto_rig_vtuber(puppet: &mut Puppet) -> Vec<String> {
	add_standard_vtuber_params(puppet);

	let nodes: Vec<(InoxNodeUuid, String, bool)> = puppet
		.nodes
		.pre_order_iter()
		.skip(1)
		.map(|n| {
			let is_part = puppet.node_comps.get::<Mesh>(n.uuid).is_some();
			(n.uuid, n.name.clone(), is_part)
		})
		.collect();

	let mut actions = Vec::new();
	let mut head_anchor: Option<InoxNodeUuid> = None;
	let mut body_anchor: Option<InoxNodeUuid> = None;
	let has_physics = puppet
		.nodes
		.iter()
		.any(|n| puppet.node_comps.get::<SimplePhysics>(n.uuid).is_some());

	// Measure Face bounding box for proportional displacement scaling
	let (face_w, face_h) = crate::audit::find_face_node(puppet)
		.and_then(|fid| puppet.node_comps.get::<Mesh>(fid))
		.map(|m| {
			let (mn, mx) = mesh_bounds(m);
			((mx.x - mn.x).max(40.0), (mx.y - mn.y).max(40.0))
		})
		.unwrap_or((170.0, 175.0));
	let yaw_px = (face_w * 0.14).clamp(10.0, 120.0);
	let pitch_px = (face_h * 0.11).clamp(8.0, 100.0);
	let mouth_open_px = (face_h * 0.068).clamp(6.0, 48.0);
	let sway_front = (face_h * 0.10).clamp(8.0, 60.0);
	let sway_back = (face_h * 0.14).clamp(12.0, 80.0);

	for &(id, ref raw_name, is_part) in &nodes {
		let lower = raw_name.to_lowercase();
		if head_anchor.is_none() && (lower == "head" || lower.contains("頭部") || lower.contains("head_group")) {
			head_anchor = Some(id);
		}
		if body_anchor.is_none() && (lower == "body" || lower.contains("torso") || lower.contains("身體")) {
			body_anchor = Some(id);
		}
		if !is_part {
			continue;
		}

		if lower.contains("hair_back") || lower.contains("back_hair") || lower.contains("後髮") || lower.contains("后发") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", -0.85, yaw_px * 1.05, pitch_px);
			let _ = apply_sway_binding(puppet, id, "Hair:: Sway", sway_back);
			actions.push(format!("Rigged back hair '{raw_name}' (depth_z=-0.85 + sway)"));
		} else if lower.contains("hair_front") || lower.contains("bang") || lower.contains("ahoge") || lower.contains("前髮") || lower.contains("劉海") || lower.contains("刘海") || lower.contains("呆毛") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.25, yaw_px, pitch_px);
			let _ = apply_sway_binding(puppet, id, "Hair:: Sway", sway_front);
			actions.push(format!("Rigged front hair '{raw_name}' (depth_z=1.25 + sway)"));
		} else if lower.contains("hair_side") || lower.contains("side_hair") || lower.contains("側髮") || lower.contains("鬢角") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 0.65, yaw_px, pitch_px);
			let _ = apply_sway_binding(puppet, id, "Hair:: Sway", sway_front * 1.1);
			actions.push(format!("Rigged side hair '{raw_name}' (depth_z=0.65 + sway)"));
		} else if lower.contains("eye_l") || lower.contains("left_eye") || lower.contains("左眼") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.0, yaw_px, pitch_px);
			let _ = apply_blink_binding(puppet, id, "Eye:: Left:: Open", 0.65);
			let _ = apply_eye_gaze_binding(puppet, id, "Eye:: Gaze", yaw_px * 0.2, pitch_px * 0.2);
			actions.push(format!("Rigged left eye '{raw_name}' (perspective + blink + gaze)"));
		} else if lower.contains("eye_r") || lower.contains("right_eye") || lower.contains("右眼") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.0, yaw_px, pitch_px);
			let _ = apply_blink_binding(puppet, id, "Eye:: Right:: Open", 0.65);
			let _ = apply_eye_gaze_binding(puppet, id, "Eye:: Gaze", yaw_px * 0.2, pitch_px * 0.2);
			actions.push(format!("Rigged right eye '{raw_name}' (perspective + blink + gaze)"));
		} else if lower.contains("mouth") || lower.contains("lip") || lower.contains("嘴") || lower.contains("口") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.1, yaw_px, pitch_px);
			let _ = apply_mouth_open_binding(puppet, id, "Mouth:: Open", mouth_open_px);
			let _ = apply_mouth_form_binding(puppet, id, "Mouth:: Form", mouth_open_px * 0.5, mouth_open_px * 0.4);
			actions.push(format!("Rigged mouth '{raw_name}' (perspective + open + form)"));
		} else if lower.contains("nose") || lower.contains("鼻") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.35, yaw_px, pitch_px);
			actions.push(format!("Rigged nose '{raw_name}' (depth_z=1.35)"));
		} else if lower.contains("brow") || lower.contains("眉") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 1.05, yaw_px, pitch_px);
			let is_right = lower.contains("_r") || lower.contains("right") || lower.contains("右");
			let brow_param = if is_right { "Brow:: Right" } else { "Brow:: Left" };
			let _ = apply_brow_binding(puppet, id, brow_param, is_right);
			actions.push(format!("Rigged brow '{raw_name}' (depth_z=1.05 + {brow_param})"));
		} else if lower.contains("ear") || lower.contains("耳") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", -0.55, yaw_px * 0.9, pitch_px * 0.9);
			actions.push(format!("Rigged ear '{raw_name}' (depth_z=-0.55)"));
		} else if lower.contains("face") || lower.contains("head") || lower.contains("臉") || lower.contains("脸") {
			let _ = apply_head_perspective_binding(puppet, id, "Head:: Yaw-Pitch", 0.25, yaw_px * 0.9, pitch_px * 0.9);
			if head_anchor.is_none() {
				head_anchor = Some(id);
			}
			actions.push(format!("Rigged face '{raw_name}' (depth_z=0.25)"));
		} else if lower.contains("torso") || lower.contains("body") || lower.contains("身體") || lower.contains("衣") {
			let _ = apply_breath_binding(puppet, id, "Breath", 0.02, 4.0);
			actions.push(format!("Rigged body '{raw_name}' (breath)"));
		}
	}

	if let Some(head_id) = head_anchor {
		let _ = apply_head_roll_binding(puppet, head_id, "Head:: Roll", 15.0);
		if !has_physics {
			let _ = add_pendulum_physics(
				puppet,
				head_id,
				"Hair_Physics",
				"Hair:: Sway",
				PhysicsModel::RigidPendulum,
				80.0,
				2.2,
				0.45,
				vec2(1.0, 1.0),
			);
			actions.push("Added Hair_Physics SimplePendulum".to_owned());
		}
	} else if !has_physics {
		let root_id = puppet.nodes.root_node_id;
		let _ = add_pendulum_physics(
			puppet,
			root_id,
			"Hair_Physics",
			"Hair:: Sway",
			PhysicsModel::RigidPendulum,
			80.0,
			2.2,
			0.45,
			vec2(1.0, 1.0),
		);
		actions.push("Added Hair_Physics SimplePendulum under Root".to_owned());
	}

	if let Some(body_id) = body_anchor {
		let _ = apply_breath_binding(puppet, body_id, "Breath", 0.02, 4.0);
	}

	create_idle_animation(puppet);
	actions.push("Created 'Idle' animation (120 frames @ 30fps)".to_owned());
	let fix_res = crate::audit::auto_fix_puppet(puppet);
	actions.push(format!("Auto-fixed constraints: {}", json::stringify(fix_res)));
	actions
}

/// Apply a Gaussian soft-brush deform to a Part node's parameter keyframe using either
/// a semantic anchor (`center`, `left`, `right`, `top`, `bottom`, `top_left`, `top_right`, `bottom_left`, `bottom_right`)
/// or explicit local `(x, y)` coordinates.
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
	let mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();

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

	let (mn, mx) = mesh_bounds(&mesh);
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

	let x_len = param.axis_points.x.len();
	let y_len = param.axis_points.y.len();
	let existing_idx = param
		.bindings
		.iter()
		.position(|b| b.node == node_id && matches!(b.values, BindingValues::Deform(_)));

	if existing_idx.is_none() {
		let zero_mat = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| vec![Vec2::ZERO; mesh.vertices.len()]);
		let is_set = Matrix2d::from_grid_transposed(x_len, y_len, |_, _| true);
		param.bindings.push(Binding {
			node: node_id,
			is_set,
			interpolate_mode: InterpolateMode::Linear,
			values: BindingValues::Deform(zero_mat),
		});
	}

	let b_idx = existing_idx.unwrap_or(param.bindings.len() - 1);
	let mut affected = 0usize;
	if let BindingValues::Deform(ref mut mat) = param.bindings[b_idx].values {
		if let Some(cell) = mat.get_mut(ix, iy) {
			for (vi, &v) in mesh.vertices.iter().enumerate() {
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

/// Procedurally paint and attach an expression overlay Part (`blush`, `gloom`, `tears`, `angry`, `shock`)
/// positioned relative to Face bounds, and bind a 0..1 Opacity parameter (`Expr:: <Kind>`).
pub fn synthesize_expression_overlay(
	model: &mut Model,
	kind: &str,
	parent_id: Option<InoxNodeUuid>,
) -> Result<(InoxNodeUuid, String), String> {
	model.puppet.init_all();
	model.puppet.begin_frame();
	model.puppet.end_frame(0.0);

	let face_id = crate::audit::find_face_node(&model.puppet);
	let (face_w, face_h) = face_id
		.and_then(|fid| model.puppet.node_comps.get::<Mesh>(fid))
		.map(|m| {
			let (mn, mx) = mesh_bounds(m);
			((mx.x - mn.x).max(60.0), (mx.y - mn.y).max(60.0))
		})
		.unwrap_or((170.0, 175.0));

	let target_parent = parent_id
		.or_else(|| {
			model
				.puppet
				.nodes
				.iter()
				.find(|n| n.name.eq_ignore_ascii_case("Head") || n.name.contains("頭"))
				.map(|n| n.uuid)
		})
		.or_else(|| face_id.map(|fid| model.puppet.nodes.get_parent(fid).uuid))
		.unwrap_or(model.puppet.nodes.root_node_id);

	let lower = kind.to_lowercase();
	let (part_name, param_name, zsort, offset, pw, ph, blend, img) = match lower.as_str() {
		"blush" | "cheek" | "臉紅" => {
			let w = (face_w * 0.78).round().max(48.0) as u32;
			let h = (face_h * 0.22).round().max(24.0) as u32;
			let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
			for side in [0.22f32, 0.78f32] {
				let cx = w as f32 * side;
				let cy = h as f32 * 0.5;
				let rx = w as f32 * 0.16;
				let ry = h as f32 * 0.40;
				for y in 0..h {
					for x in 0..w {
						let dx = (x as f32 + 0.5 - cx) / rx;
						let dy = (y as f32 + 0.5 - cy) / ry;
						let d2 = dx * dx + dy * dy;
						if d2 < 1.0 {
							let falloff = (1.0 - d2).powf(1.4);
							let slash = if ((x as i32 + y as i32) % 9).abs() <= 1 && d2 < 0.55 {
								45u8
							} else {
								0
							};
							let alpha = (falloff * 185.0 + slash as f32).clamp(0.0, 230.0) as u8;
							img.put_pixel(x, y, Rgba([255, 110, 145, alpha]));
						}
					}
				}
			}
			(
				"Expr_Blush",
				"Expr:: Blush",
				-2.5,
				vec2(0.0, face_h * 0.08),
				w as f32,
				h as f32,
				BlendMode::Normal,
				img,
			)
		}
		"gloom" | "shadow" | "陰沉" | "黑臉" => {
			let w = (face_w * 0.92).round().max(64.0) as u32;
			let h = (face_h * 0.55).round().max(48.0) as u32;
			let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
			let cx = w as f32 * 0.5;
			for y in 0..h {
				let ty = 1.0 - (y as f32 / (h as f32 - 1.0));
				for x in 0..w {
					let dx = ((x as f32 + 0.5 - cx) / (w as f32 * 0.48)).abs();
					if dx <= 1.0 {
						let edge = (1.0 - dx * dx).max(0.0);
						let alpha = (ty.powf(1.2) * edge * 190.0).clamp(0.0, 210.0) as u8;
						img.put_pixel(x, y, Rgba([58, 52, 110, alpha]));
					}
				}
			}
			(
				"Expr_Gloom",
				"Expr:: Gloom",
				-7.5,
				vec2(0.0, -face_h * 0.22),
				w as f32,
				h as f32,
				BlendMode::Multiply,
				img,
			)
		}
		"tears" | "cry" | "眼淚" => {
			let w = (face_w * 0.72).round().max(48.0) as u32;
			let h = (face_h * 0.26).round().max(24.0) as u32;
			let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
			for side in [0.22f32, 0.78f32] {
				let cx = w as f32 * side;
				let cy = h as f32 * 0.55;
				let rx = w as f32 * 0.11;
				let ry = h as f32 * 0.36;
				for y in 0..h {
					for x in 0..w {
						let dx = (x as f32 + 0.5 - cx) / rx;
						let dy = (y as f32 + 0.5 - cy) / ry;
						let d2 = dx * dx + dy * dy;
						if d2 < 1.0 {
							let is_hl = ((dx + 0.25).powi(2) + (dy + 0.25).powi(2)) < 0.18;
							let col = if is_hl {
								[245, 252, 255, 240]
							} else {
								[115, 200, 255, ((1.0 - d2 * 0.5) * 215.0) as u8]
							};
							img.put_pixel(x, y, Rgba(col));
						}
					}
				}
			}
			(
				"Expr_Tears",
				"Expr:: Tears",
				-6.5,
				vec2(0.0, face_h * 0.04),
				w as f32,
				h as f32,
				BlendMode::Normal,
				img,
			)
		}
		"angry" | "angry_mark" | "青筋" | "生氣" => {
			let sz = (face_w * 0.22).round().max(24.0) as u32;
			let mut img = RgbaImage::from_pixel(sz, sz, Rgba([0, 0, 0, 0]));
			let c = sz as f32 * 0.5;
			let r = sz as f32 * 0.36;
			for y in 0..sz {
				for x in 0..sz {
					let dx = x as f32 + 0.5 - c;
					let dy = y as f32 + 0.5 - c;
					// Four curved anime cross-vein corners
					let corner = ((dx.abs() - r * 0.55).powi(2) + (dy.abs() - r * 0.55).powi(2)).sqrt();
					if (corner - r * 0.32).abs() < 2.2 && dx.abs() < r && dy.abs() < r {
						img.put_pixel(x, y, Rgba([235, 55, 70, 240]));
					}
				}
			}
			(
				"Expr_Angry",
				"Expr:: Angry",
				-11.0,
				vec2(face_w * 0.30, -face_h * 0.28),
				sz as f32,
				sz as f32,
				BlendMode::Normal,
				img,
			)
		}
		"shock" | "驚嚇" => {
			let w = (face_w * 0.72).round().max(48.0) as u32;
			let h = (face_h * 0.30).round().max(24.0) as u32;
			let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
			for x in (4..w - 4).step_by(8) {
				for y in 0..h {
					let ty = y as f32 / h as f32;
					let alpha = ((1.0 - ty) * 210.0) as u8;
					img.put_pixel(x, y, Rgba([210, 225, 255, alpha]));
					if x + 1 < w {
						img.put_pixel(x + 1, y, Rgba([210, 225, 255, alpha / 2]));
					}
				}
			}
			(
				"Expr_Shock",
				"Expr:: Shock",
				-8.0,
				vec2(0.0, -face_h * 0.25),
				w as f32,
				h as f32,
				BlendMode::Normal,
				img,
			)
		}
		other => {
			return Err(format!(
				"Unknown overlay kind '{other}' (expected blush, gloom, tears, angry, shock)"
			));
		}
	};

	let id = add_grid_part(model, target_parent, part_name, zsort, offset, pw, ph, 4, 4, img);
	if let Some(d) = model.puppet.node_comps.get_mut::<Drawable>(id) {
		d.blending.mode = blend;
	}

	if model.puppet.params.contains_key("Head:: Yaw-Pitch") {
		let _ = apply_head_perspective_binding(&mut model.puppet, id, "Head:: Yaw-Pitch", 0.75, face_w * 0.14, face_h * 0.11);
	}
	apply_opacity_binding(&mut model.puppet, id, param_name, 0.0, 1.0)?;
	model.puppet.reset_contexts();

	Ok((id, param_name.to_owned()))
}

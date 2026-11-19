use std::collections::{HashMap, HashSet, VecDeque};
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

	let (b_idx, ix, iy) = ensure_deform_binding_keypoint(puppet, node_id, param_name, param_val, mesh.vertices.len())?;

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

	let param = puppet
		.params
		.get_mut(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' missing"))?;
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
	let vert_count = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.vertices
		.len();

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

#[derive(Clone, Copy)]
struct DelaunayPt {
	x: f64,
	y: f64,
}

#[inline]
fn orient2d_f64(a: DelaunayPt, b: DelaunayPt, c: DelaunayPt) -> f64 {
	(b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

#[inline]
fn in_circumcircle_ccw(a: DelaunayPt, b: DelaunayPt, c: DelaunayPt, p: DelaunayPt) -> bool {
	let ax = a.x - p.x;
	let ay = a.y - p.y;
	let bx = b.x - p.x;
	let by = b.y - p.y;
	let cx = c.x - p.x;
	let cy = c.y - p.y;
	let det = (ax * ax + ay * ay) * (bx * cy - by * cx)
		- (bx * bx + by * by) * (ax * cy - ay * cx)
		+ (cx * cx + cy * cy) * (ax * by - ay * bx);
	det > 0.0
}

fn bowyer_watson_tris(pts: &[DelaunayPt], min_scale: f64) -> Vec<[usize; 3]> {
	let n = pts.len();
	if n < 3 {
		return Vec::new();
	}

	// Perturb predicates infinitesimally (Simulation of Simplicity) so exact circles never degenerate.
	let mut bw_pts: Vec<DelaunayPt> = pts
		.iter()
		.enumerate()
		.map(|(i, p)| {
			let jx = (((i.wrapping_mul(2654435761) & 1023) as f64) / 1023.0 - 0.5) * 1e-4;
			let jy = (((i.wrapping_mul(1597334677) & 1023) as f64) / 1023.0 - 0.5) * 1e-4;
			DelaunayPt {
				x: p.x + jx,
				y: p.y + jy,
			}
		})
		.collect();

	let (mut bx0, mut bx1) = (f64::INFINITY, f64::NEG_INFINITY);
	let (mut by0, mut by1) = (f64::INFINITY, f64::NEG_INFINITY);
	for p in &bw_pts {
		bx0 = bx0.min(p.x);
		bx1 = bx1.max(p.x);
		by0 = by0.min(p.y);
		by1 = by1.max(p.y);
	}
	let dmax = (bx1 - bx0).max(by1 - by0).max(min_scale).max(16.0);
	let mid_x = (bx0 + bx1) * 0.5;
	let mid_y = (by0 + by1) * 0.5;

	let s0 = DelaunayPt {
		x: mid_x - 20.0 * dmax,
		y: mid_y - dmax,
	};
	let s1 = DelaunayPt {
		x: mid_x + 20.0 * dmax,
		y: mid_y - dmax,
	};
	let s2 = DelaunayPt {
		x: mid_x,
		y: mid_y + 20.0 * dmax,
	};
	bw_pts.push(s0);
	bw_pts.push(s1);
	bw_pts.push(s2);

	let mut tris: Vec<[usize; 3]> = if orient2d_f64(bw_pts[n], bw_pts[n + 1], bw_pts[n + 2]) > 0.0 {
		vec![[n, n + 1, n + 2]]
	} else {
		vec![[n, n + 2, n + 1]]
	};

	for pi in 0..n {
		let p = bw_pts[pi];
		let mut edge_counts: HashMap<(usize, usize), (usize, (usize, usize))> = HashMap::new();
		let mut keep_tris = Vec::with_capacity(tris.len());

		for tri in tris {
			if in_circumcircle_ccw(bw_pts[tri[0]], bw_pts[tri[1]], bw_pts[tri[2]], p) {
				let edges = [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])];
				for (u, v) in edges {
					let key = (u.min(v), u.max(v));
					edge_counts
						.entry(key)
						.and_modify(|e| e.0 += 1)
						.or_insert((1usize, (u, v)));
				}
			} else {
				keep_tris.push(tri);
			}
		}

		for (_, (count, (u, v))) in edge_counts {
			if count == 1 {
				let area = orient2d_f64(bw_pts[u], bw_pts[v], p);
				if area > 1e-9 {
					keep_tris.push([u, v, pi]);
				} else if area < -1e-9 {
					keep_tris.push([v, u, pi]);
				}
			}
		}
		tris = keep_tris;
	}

	tris.into_iter()
		.filter(|t| t[0] < n && t[1] < n && t[2] < n)
		.collect()
}

/// Replace `node_id`'s vertex array with `new_verts`, recompute UVs smoothly from existing
/// vertex->UV correspondence, retriangulate with Bowyer-Watson Delaunay, and remap all
/// existing `BindingValues::Deform` keyframes onto the new topology via IDW interpolation.
pub fn retriangulate_points_preserving_deforms(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	new_verts: Vec<Vec2>,
) -> Result<(usize, usize), String> {
	if new_verts.len() < 3 {
		return Err("Mesh requires at least 3 vertices".to_owned());
	}
	let old_mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();
	if old_mesh.vertices.is_empty() {
		return Err("Existing mesh has no vertices".to_owned());
	}

	// Derive linear fallback UV mapping from existing mesh bbox so added outer vertices map accurately.
	let (old_min, old_max) = mesh_bounds(&old_mesh);
	let (mut umin, mut vmin) = (f32::INFINITY, f32::INFINITY);
	let (mut umax, mut vmax) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
	for &uv in &old_mesh.uvs {
		umin = umin.min(uv.x);
		vmin = vmin.min(uv.y);
		umax = umax.max(uv.x);
		vmax = vmax.max(uv.y);
	}
	let span_v = (old_max - old_min).max(vec2(1.0, 1.0));
	let span_uv = vec2((umax - umin).max(1e-4), (vmax - vmin).max(1e-4));

	let mut new_uvs = Vec::with_capacity(new_verts.len());
	for &nv in &new_verts {
		if let Some((oi, _)) = old_mesh
			.vertices
			.iter()
			.enumerate()
			.find(|(_, &ov)| (ov - nv).length_squared() < 0.01)
		{
			new_uvs.push(*old_mesh.uvs.get(oi).unwrap_or(&vec2(0.5, 0.5)));
		} else {
			let tx = ((nv.x - old_min.x) / span_v.x).clamp(-0.2, 1.2);
			let ty = ((nv.y - old_min.y) / span_v.y).clamp(-0.2, 1.2);
			new_uvs.push(vec2((umin + tx * span_uv.x).clamp(0.0, 1.0), (vmin + ty * span_uv.y).clamp(0.0, 1.0)));
		}
	}

	let pts64: Vec<DelaunayPt> = new_verts
		.iter()
		.map(|v| DelaunayPt {
			x: v.x as f64,
			y: v.y as f64,
		})
		.collect();
	let raw_tris = bowyer_watson_tris(&pts64, (span_v.x.max(span_v.y)) as f64);
	let mut indices = Vec::with_capacity(raw_tris.len() * 3);
	for tri in raw_tris {
		let area = orient2d_f64(pts64[tri[0]], pts64[tri[1]], pts64[tri[2]]);
		if area.abs() <= 1e-6 {
			continue;
		}
		if area > 0.0 {
			indices.extend_from_slice(&[tri[0] as u32, tri[1] as u32, tri[2] as u32]);
		} else {
			indices.extend_from_slice(&[tri[0] as u32, tri[2] as u32, tri[1] as u32]);
		}
	}
	if indices.len() < 3 {
		return Err("Delaunay retriangulation produced no valid triangles (points may be collinear)".to_owned());
	}

	// Precompute IDW weights from old vertices -> new vertices so deform bindings survive edits.
	let map_weights: Vec<Vec<(usize, f32)>> = new_verts
		.iter()
		.map(|&nv| {
			let mut dists: Vec<(usize, f32)> = old_mesh
				.vertices
				.iter()
				.enumerate()
				.map(|(oi, &ov)| (oi, (ov - nv).length_squared()))
				.collect();
			dists.sort_by(|a, b| a.1.total_cmp(&b.1));
			if let Some(&(oi, d2)) = dists.first() {
				if d2 < 0.01 {
					return vec![(oi, 1.0)];
				}
			}
			let top: Vec<(usize, f32)> = dists
				.into_iter()
				.take(3)
				.map(|(oi, d2)| (oi, 1.0 / (d2 + 1e-4)))
				.collect();
			let sum_w: f32 = top.iter().map(|&(_, w)| w).sum::<f32>().max(1e-6);
			top.into_iter().map(|(oi, w)| (oi, w / sum_w)).collect()
		})
		.collect();

	for param in puppet.params.values_mut() {
		for binding in &mut param.bindings {
			if binding.node != node_id {
				continue;
			}
			if let BindingValues::Deform(ref mut mat) = binding.values {
				let old_mat = mat.clone();
				let (x_len, y_len) = (old_mat.width(), old_mat.height());
				let new_mat = Matrix2d::from_grid_transposed(x_len, y_len, |ix, iy| {
					let old_cell = old_mat.get(ix, iy);
					map_weights
						.iter()
						.map(|weights| {
							let Some(cell) = old_cell else {
								return Vec2::ZERO;
							};
							let mut acc = Vec2::ZERO;
							for &(oi, w) in weights {
								if let Some(&d) = cell.get(oi) {
									acc += d * w;
								}
							}
							acc
						})
						.collect()
				});
				*mat = new_mat;
			}
		}
	}

	let vert_count = new_verts.len();
	let tri_count = indices.len() / 3;
	puppet.node_comps.set(
		node_id,
		Mesh {
			vertices: new_verts,
			uvs: new_uvs,
			indices,
			origin: old_mesh.origin,
		},
	);
	puppet.reset_contexts();
	Ok((vert_count, tri_count))
}

/// Configure an existing `SimplePhysics` pendulum node.
pub fn configure_pendulum_physics(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	model_type: Option<PhysicsModel>,
	length: Option<f32>,
	frequency: Option<f32>,
	angle_damping: Option<f32>,
	length_damping: Option<f32>,
	gravity: Option<f32>,
	output_scale: Option<Vec2>,
) -> Result<(), String> {
	let phys = puppet
		.node_comps
		.get_mut::<SimplePhysics>(node_id)
		.ok_or_else(|| format!("Node {} does not have a SimplePhysics component", node_id.0))?;

	if let Some(mt) = model_type {
		phys.model_type = mt;
	}
	if let Some(l) = length {
		phys.props.length = l.clamp(5.0, 800.0);
	}
	if let Some(f) = frequency {
		phys.props.frequency = f.clamp(0.1, 20.0);
	}
	if let Some(ad) = angle_damping {
		phys.props.angle_damping = ad.clamp(0.0, 10.0);
	}
	if let Some(ld) = length_damping {
		phys.props.length_damping = ld.clamp(0.0, 10.0);
	}
	if let Some(g) = gravity {
		phys.props.gravity = g.clamp(-5.0, 10.0);
	}
	if let Some(os) = output_scale {
		phys.props.output_scale = os;
	}
	puppet.reset_contexts();
	Ok(())
}

/// Insert or update a keyframe on `anim_name` for `(param_name, target)` (creating the lane/animation if needed).
pub fn set_animation_keyframe(
	puppet: &mut Puppet,
	anim_name: &str,
	param_name: &str,
	target: u8,
	frame: i32,
	value: f32,
	tension: f32,
	interpolation: Option<InterpolateMode>,
) -> Result<(), String> {
	let param_uuid = puppet
		.params
		.get(param_name)
		.map(|p| p.uuid)
		.ok_or_else(|| format!("Parameter '{param_name}' does not exist"))?;

	let anim = puppet.animations.entry(anim_name.to_owned()).or_insert_with(|| PuppetAnimation {
		timestep: 1.0 / 30.0,
		additive: false,
		length: 120,
		lead_in: 0,
		lead_out: 0,
		lanes: Vec::new(),
	});
	let frame = frame.max(0);
	if frame + 1 > anim.length {
		anim.length = frame + 1;
	}

	let lane = if let Some(existing) = anim
		.lanes
		.iter_mut()
		.find(|l| l.param_uuid == param_uuid && l.target == target)
	{
		existing
	} else {
		anim.lanes.push(AnimationLane {
			interpolation: interpolation.unwrap_or(InterpolateMode::Linear),
			param_uuid,
			target,
			keyframes: Vec::new(),
			merge_mode: "Forced".to_owned(),
		});
		anim.lanes.last_mut().unwrap()
	};

	if let Some(mode) = interpolation {
		lane.interpolation = mode;
	}

	let kf = Keyframe {
		frame,
		value,
		tension: tension.clamp(0.0, 1.0),
	};
	if let Some(pos) = lane.keyframes.iter().position(|k| k.frame == frame) {
		lane.keyframes[pos] = kf;
	} else {
		lane.keyframes.push(kf);
		lane.keyframes.sort_by_key(|k| k.frame);
	}
	Ok(())
}

/// Remove a keyframe at `frame` from `(anim_name, param_name, target)`. Returns true if removed.
pub fn remove_animation_keyframe(
	puppet: &mut Puppet,
	anim_name: &str,
	param_name: &str,
	target: u8,
	frame: i32,
) -> Result<bool, String> {
	let param_uuid = puppet
		.params
		.get(param_name)
		.map(|p| p.uuid)
		.ok_or_else(|| format!("Parameter '{param_name}' does not exist"))?;

	let Some(anim) = puppet.animations.get_mut(anim_name) else {
		return Err(format!("Animation '{anim_name}' not found"));
	};
	let Some(lane) = anim
		.lanes
		.iter_mut()
		.find(|l| l.param_uuid == param_uuid && l.target == target)
	else {
		return Ok(false);
	};
	let before = lane.keyframes.len();
	lane.keyframes.retain(|k| k.frame != frame);
	Ok(lane.keyframes.len() < before)
}

/// High-density adaptive sampling circle (e.g. eye corners, mouth corners, elbow/knee joints)
/// passed to [`generate_delaunay_mesh_with_options`].
#[derive(Clone, Debug)]
pub struct RefinementZone {
	/// Local mesh coordinate center (same space as `Mesh.vertices`).
	pub center: Vec2,
	/// Influence radius in local mesh units.
	pub radius: f32,
	/// Local spacing factor relative to `step_px` (e.g. `0.35` produces ~3x finer triangles).
	pub density_scale: f32,
}

/// Options controlling boundary-constrained Bowyer-Watson Delaunay meshing.
#[derive(Clone, Debug)]
pub struct DelaunayMeshOptions {
	pub step_px: f32,
	pub alpha_threshold: u8,
	/// Signed pixel offset around alpha silhouette (`>= 0` expands outward margin).
	pub boundary_offset_px: f32,
	/// Enforce exact closed contour segments via Sloan edge recovery and polygon-parity culling.
	pub constrained: bool,
	/// Automatically refine triangle density around sharp boundary corners (>= 35 deg turn).
	pub refine_near_corners: bool,
	/// Explicit localized high-density regions (joints, expression anchor areas).
	pub refinement_zones: Vec<RefinementZone>,
}

impl Default for DelaunayMeshOptions {
	fn default() -> Self {
		Self {
			step_px: 22.0,
			alpha_threshold: 4,
			boundary_offset_px: 2.0,
			constrained: true,
			refine_near_corners: true,
			refinement_zones: Vec::new(),
		}
	}
}

/// Generate a boundary-conforming Bowyer-Watson Delaunay triangle mesh from an RGBA texture.
pub fn generate_delaunay_mesh(
	img: &RgbaImage,
	width: f32,
	height: f32,
	step_px: f32,
	alpha_threshold: u8,
) -> Mesh {
	generate_delaunay_mesh_with_options(
		img,
		width,
		height,
		&DelaunayMeshOptions {
			step_px,
			alpha_threshold,
			..Default::default()
		},
	)
}

/// Generate a constrained, adaptively refined Bowyer-Watson Delaunay mesh:
/// 1) Euclidean-chamfer signed boundary offset (`boundary_offset_px`).
/// 2) Ordered pixel-edge alpha-contour loop tracing + closed-loop Douglas-Peucker simplification.
/// 3) Corner detection + explicit `RefinementZone` adaptive hex interior lattice.
/// 4) Sloan edge-flipping constrained Delaunay recovery + exact polygon-parity silhouette culling.
pub fn generate_delaunay_mesh_with_options(
	img: &RgbaImage,
	width: f32,
	height: f32,
	opts: &DelaunayMeshOptions,
) -> Mesh {
	let iw = img.width() as usize;
	let ih = img.height() as usize;
	if iw < 3 || ih < 3 || width <= 1.0 || height <= 1.0 {
		return generate_grid_mesh(width.max(1.0), height.max(1.0), 2, 2, Vec2::ZERO);
	}

	let mut raw = vec![false; iw * ih];
	let mut any_opaque = false;
	for y in 0..ih {
		for x in 0..iw {
			if img.get_pixel(x as u32, y as u32).0[3] > opts.alpha_threshold {
				raw[y * iw + x] = true;
				any_opaque = true;
			}
		}
	}
	if !any_opaque {
		return generate_grid_mesh(width, height, 2, 2, Vec2::ZERO);
	}

	let offset = opts.boundary_offset_px;
	let mut mask = if offset >= 0.0 {
		let dist = chamfer_distance_to_mask(&raw, iw, ih, true);
		let thresh = offset + 1e-4;
		dist.into_iter().map(|d| d <= thresh).collect::<Vec<bool>>()
	} else {
		let dist = chamfer_distance_to_mask(&raw, iw, ih, false);
		let thresh = -offset + 1e-4;
		let eroded = dist.into_iter().map(|d| d > thresh).collect::<Vec<bool>>();
		if eroded.iter().any(|&v| v) {
			eroded
		} else {
			raw.clone()
		}
	};

	if !mask.iter().any(|&v| v) {
		mask = raw;
	}

	let w64 = width as f64;
	let h64 = height as f64;
	let sx = (w64 / iw as f64).max(0.01);
	let sy = (h64 / ih as f64).max(0.01);
	let spacing = (opts.step_px.clamp(4.0, 256.0) as f64) * ((sx + sy) * 0.5);

	let grid_to_local = |vx: usize, vy: usize| -> DelaunayPt {
		DelaunayPt {
			x: (vx as f64 / iw as f64 - 0.5) * w64,
			y: (vy as f64 / ih as f64 - 0.5) * h64,
		}
	};

	let raw_loops = extract_alpha_contour_loops(&mask, iw, ih);
	if raw_loops.is_empty() {
		return generate_contour_mesh(img, width, height, 4, 4, opts.alpha_threshold);
	}

	let dp_eps = (spacing * 0.12).clamp(0.4, 3.0);
	let mut simplified_loops: Vec<Vec<DelaunayPt>> = raw_loops
		.into_iter()
		.filter_map(|loop_grid| {
			let local: Vec<DelaunayPt> = loop_grid
				.into_iter()
				.map(|(vx, vy)| grid_to_local(vx, vy))
				.collect();
			let clean = collapse_collinear_closed(&local, 1e-6);
			if clean.len() < 3 {
				return None;
			}
			let simp = douglas_peucker_closed(&clean, dp_eps);
			if simp.len() < 3 || polygon_signed_area(&simp).abs() < (spacing * spacing * 0.05).max(1.0) {
				None
			} else {
				Some(simp)
			}
		})
		.collect();

	if simplified_loops.is_empty() {
		return generate_contour_mesh(img, width, height, 4, 4, opts.alpha_threshold);
	}

	// Collect refinement zones (explicit caller joints first, then sharp boundary corners)
	let mut active_zones: Vec<RefinementZone> = opts.refinement_zones.clone();
	if opts.refine_near_corners {
		let mut corner_cands: Vec<(f64, Vec2)> = Vec::new();
		for lp in &simplified_loops {
			let m = lp.len();
			for i in 0..m {
				let prev = lp[(i + m - 1) % m];
				let curr = lp[i];
				let next = lp[(i + 1) % m];
				let d1x = curr.x - prev.x;
				let d1y = curr.y - prev.y;
				let d2x = next.x - curr.x;
				let d2y = next.y - curr.y;
				let l1 = (d1x * d1x + d1y * d1y).sqrt();
				let l2 = (d2x * d2x + d2y * d2y).sqrt();
				if l1 > 1e-6 && l2 > 1e-6 {
					let cos_ang = (d1x * d2x + d1y * d2y) / (l1 * l2);
					if cos_ang <= 0.81915 {
						corner_cands.push((cos_ang, vec2(curr.x as f32, curr.y as f32)));
					}
				}
			}
		}
		corner_cands.sort_by(|a, b| a.0.total_cmp(&b.0));
		let min_sep_sq = (spacing * 0.65).powi(2) as f32;
		let mut added_corners = 0usize;
		for (_, cpos) in corner_cands {
			if added_corners >= 64 {
				break;
			}
			let too_close = active_zones
				.iter()
				.any(|z| (z.center - cpos).length_squared() < min_sep_sq);
			if !too_close {
				active_zones.push(RefinementZone {
					center: cpos,
					radius: (spacing * 1.35) as f32,
					density_scale: 0.45,
				});
				added_corners += 1;
			}
		}
	}

	let local_spacing_at = |x: f64, y: f64| -> f64 {
		let mut scale = 1.0f64;
		let p = vec2(x as f32, y as f32);
		for z in &active_zones {
			if z.radius > 0.0 && (z.center - p).length_squared() <= z.radius * z.radius {
				scale = scale.min((z.density_scale as f64).clamp(0.2, 1.0));
			}
		}
		(spacing * scale).max(2.0)
	};

	// Resample long boundary segments adaptively so edges near joints match interior density.
	for lp in &mut simplified_loops {
		let m = lp.len();
		let mut resampled = Vec::with_capacity(m * 2);
		for i in 0..m {
			let a = lp[i];
			let b = lp[(i + 1) % m];
			let dx = b.x - a.x;
			let dy = b.y - a.y;
			let seg_len = (dx * dx + dy * dy).sqrt();
			let target = local_spacing_at((a.x + b.x) * 0.5, (a.y + b.y) * 0.5).max(2.0);
			let steps = (seg_len / target).ceil().max(1.0).min(128.0) as usize;
			for s in 0..steps {
				let t = s as f64 / steps as f64;
				resampled.push(DelaunayPt {
					x: a.x + dx * t,
					y: a.y + dy * t,
				});
			}
		}
		*lp = resampled;
	}

	// Build point array: boundary loop vertices first (exact index rings), then adaptive interior hex points.
	let mut pts: Vec<DelaunayPt> = Vec::new();
	let cell_size = spacing.max(4.0);
	let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::new();

	let bucket_insert = |pt: DelaunayPt, pts: &mut Vec<DelaunayPt>, buckets: &mut HashMap<(i32, i32), Vec<usize>>| -> usize {
		let cx = (pt.x / cell_size).floor() as i32;
		let cy = (pt.y / cell_size).floor() as i32;
		for dy in -1..=1 {
			for dx in -1..=1 {
				if let Some(list) = buckets.get(&(cx + dx, cy + dy)) {
					for &idx in list {
						let ex = pts[idx].x - pt.x;
						let ey = pts[idx].y - pt.y;
						if ex * ex + ey * ey < 1e-6 {
							return idx;
						}
					}
				}
			}
		}
		let idx = pts.len();
		pts.push(pt);
		buckets.entry((cx, cy)).or_default().push(idx);
		idx
	};

	let mut constrained_edges: Vec<(usize, usize)> = Vec::new();
	let mut constrained_seen: HashSet<(usize, usize)> = HashSet::new();
	let mut segs: Vec<(DelaunayPt, DelaunayPt)> = Vec::new();

	for lp in &simplified_loops {
		let ring_idxs: Vec<usize> = lp.iter().map(|&p| bucket_insert(p, &mut pts, &mut buckets)).collect();
		let m = ring_idxs.len();
		for i in 0..m {
			let u = ring_idxs[i];
			let v = ring_idxs[(i + 1) % m];
			if u != v {
				let key = (u.min(v), u.max(v));
				if constrained_seen.insert(key) {
					constrained_edges.push((u, v));
				}
				segs.push((pts[u], pts[v]));
			}
		}
	}

	let try_insert_interior = |pt: DelaunayPt,
	                           local_step: f64,
	                           pts: &mut Vec<DelaunayPt>,
	                           buckets: &mut HashMap<(i32, i32), Vec<usize>>| {
		if !point_in_loops(pt, &simplified_loops) {
			return;
		}
		let clear_sq = (local_step * 0.34).max(0.75).powi(2);
		for &(a, b) in &segs {
			if point_to_segment_dist_sq(pt, a, b) < clear_sq {
				return;
			}
		}
		let min_d2 = (local_step * 0.68).max(1.25).powi(2);
		let search_cells = ((local_step / cell_size).ceil() as i32).max(1);
		let cx = (pt.x / cell_size).floor() as i32;
		let cy = (pt.y / cell_size).floor() as i32;
		for dy in -search_cells..=search_cells {
			for dx in -search_cells..=search_cells {
				if let Some(list) = buckets.get(&(cx + dx, cy + dy)) {
					for &idx in list {
						let ex = pts[idx].x - pt.x;
						let ey = pts[idx].y - pt.y;
						if ex * ex + ey * ey < min_d2 {
							return;
						}
					}
				}
			}
		}
		let idx = pts.len();
		pts.push(pt);
		buckets.entry((cx, cy)).or_default().push(idx);
	};

	// Pass 1: Dense hex lattice inside refinement zones (finest zones first)
	let mut zones_sorted = active_zones.clone();
	zones_sorted.sort_by(|a, b| a.density_scale.total_cmp(&b.density_scale));
	for z in &zones_sorted {
		if z.radius <= 1.0 {
			continue;
		}
		let z_step = (spacing * (z.density_scale as f64).clamp(0.2, 1.0)).max(2.0);
		let r = (z.radius as f64).min(w64.max(h64));
		let r2 = r * r;
		let h_dy = z_step * 0.8660254037844386;
		let mut row = 0usize;
		let mut ly = z.center.y as f64 - r + h_dy * 0.5;
		while ly <= z.center.y as f64 + r {
			let off = if row % 2 == 1 { z_step * 0.5 } else { 0.0 };
			let mut lx = z.center.x as f64 - r + off;
			while lx <= z.center.x as f64 + r {
				let dx = lx - z.center.x as f64;
				let dy = ly - z.center.y as f64;
				if dx * dx + dy * dy <= r2 {
					try_insert_interior(DelaunayPt { x: lx, y: ly }, z_step, &mut pts, &mut buckets);
				}
				lx += z_step;
			}
			row += 1;
			ly += h_dy;
		}
	}

	// Pass 2: Global interior staggered hex lattice across silhouette bbox
	let (mut bmin_x, mut bmax_x) = (f64::INFINITY, f64::NEG_INFINITY);
	let (mut bmin_y, mut bmax_y) = (f64::INFINITY, f64::NEG_INFINITY);
	for lp in &simplified_loops {
		for &p in lp {
			bmin_x = bmin_x.min(p.x);
			bmax_x = bmax_x.max(p.x);
			bmin_y = bmin_y.min(p.y);
			bmax_y = bmax_y.max(p.y);
		}
	}
	let hex_dx = spacing.max(2.0);
	let hex_dy = hex_dx * 0.8660254037844386;
	let mut row_idx = 0usize;
	let mut ly = bmin_y + hex_dy * 0.5;
	while ly <= bmax_y - hex_dy * 0.25 {
		let offset_x = if row_idx % 2 == 1 { hex_dx * 0.5 } else { 0.0 };
		let mut lx = bmin_x + offset_x;
		while lx <= bmax_x {
			let l_step = local_spacing_at(lx, ly);
			try_insert_interior(DelaunayPt { x: lx, y: ly }, l_step, &mut pts, &mut buckets);
			lx += hex_dx;
		}
		row_idx += 1;
		ly += hex_dy;
	}

	let n = pts.len();
	if n < 3 {
		return generate_contour_mesh(img, width, height, 4, 4, opts.alpha_threshold);
	}

	let mut tris = bowyer_watson_tris(&pts, w64.max(h64));
	if opts.constrained && !constrained_edges.is_empty() {
		let sos_pts: Vec<DelaunayPt> = pts
			.iter()
			.enumerate()
			.map(|(i, p)| {
				let jx = (((i.wrapping_mul(2654435761) & 1023) as f64) / 1023.0 - 0.5) * 1e-4;
				let jy = (((i.wrapping_mul(1597334677) & 1023) as f64) / 1023.0 - 0.5) * 1e-4;
				DelaunayPt {
					x: p.x + jx,
					y: p.y + jy,
				}
			})
			.collect();
		enforce_constrained_edges(&mut tris, &sos_pts, &constrained_edges);
	}

	let sample_mask = |lx: f64, ly: f64| -> bool {
		let u = lx / w64 + 0.5;
		let v = ly / h64 + 0.5;
		let ix = (u * iw as f64).floor() as isize;
		let iy = (v * ih as f64).floor() as isize;
		if ix < 0 || iy < 0 || ix >= iw as isize || iy >= ih as isize {
			return false;
		}
		mask[iy as usize * iw + ix as usize]
	};

	let mut clean_tris: Vec<[usize; 3]> = Vec::new();
	for tri in tris {
		let pa = pts[tri[0]];
		let pb = pts[tri[1]];
		let pc = pts[tri[2]];
		let area = orient2d_f64(pa, pb, pc);
		if area.abs() <= 1e-6 {
			continue;
		}
		let gx = (pa.x + pb.x + pc.x) / 3.0;
		let gy = (pa.y + pb.y + pc.y) / 3.0;
		let inside = if opts.constrained {
			point_in_loops(DelaunayPt { x: gx, y: gy }, &simplified_loops)
		} else {
			let m0 = sample_mask((pa.x + pb.x) * 0.5, (pa.y + pb.y) * 0.5) as u8;
			let m1 = sample_mask((pb.x + pc.x) * 0.5, (pb.y + pc.y) * 0.5) as u8;
			let m2 = sample_mask((pc.x + pa.x) * 0.5, (pc.y + pa.y) * 0.5) as u8;
			sample_mask(gx, gy) && (m0 + m1 + m2 >= 2)
		};
		if !inside {
			continue;
		}
		if area > 0.0 {
			clean_tris.push(tri);
		} else {
			clean_tris.push([tri[0], tri[2], tri[1]]);
		}
	}

	if clean_tris.is_empty() {
		return generate_contour_mesh(img, width, height, 4, 4, opts.alpha_threshold);
	}

	// Compact unused vertices.
	let mut remap: HashMap<usize, u32> = HashMap::new();
	let mut vertices: Vec<Vec2> = Vec::new();
	let mut uvs: Vec<Vec2> = Vec::new();
	let mut indices: Vec<u32> = Vec::with_capacity(clean_tris.len() * 3);

	for tri in clean_tris {
		for old_idx in tri {
			let new_idx = *remap.entry(old_idx).or_insert_with(|| {
				let idx = vertices.len() as u32;
				let p = pts[old_idx];
				vertices.push(vec2(p.x as f32, p.y as f32));
				let u = (p.x / w64 + 0.5).clamp(0.0, 1.0) as f32;
				let v = (p.y / h64 + 0.5).clamp(0.0, 1.0) as f32;
				uvs.push(vec2(u, v));
				idx
			});
			indices.push(new_idx);
		}
	}

	Mesh {
		vertices,
		uvs,
		indices,
		origin: Vec2::ZERO,
	}
}

fn chamfer_distance_to_mask(raw: &[bool], iw: usize, ih: usize, to_foreground: bool) -> Vec<f32> {
	const INF: f32 = 1e6;
	const SQRT2: f32 = 1.4142135;
	let mut dist = vec![INF; iw * ih];
	for y in 0..ih {
		for x in 0..iw {
			let i = y * iw + x;
			if to_foreground {
				if raw[i] {
					dist[i] = 0.0;
				}
			} else if !raw[i] {
				dist[i] = 0.0;
			} else {
				let edge_d = ((x + 1).min(iw - x).min(y + 1).min(ih - y)) as f32;
				dist[i] = edge_d;
			}
		}
	}

	// Forward pass
	for y in 0..ih {
		for x in 0..iw {
			let i = y * iw + x;
			let mut d = dist[i];
			if x > 0 {
				d = d.min(dist[i - 1] + 1.0);
			}
			if y > 0 {
				d = d.min(dist[(y - 1) * iw + x] + 1.0);
				if x > 0 {
					d = d.min(dist[(y - 1) * iw + x - 1] + SQRT2);
				}
				if x + 1 < iw {
					d = d.min(dist[(y - 1) * iw + x + 1] + SQRT2);
				}
			}
			dist[i] = d;
		}
	}

	// Backward pass
	for y in (0..ih).rev() {
		for x in (0..iw).rev() {
			let i = y * iw + x;
			let mut d = dist[i];
			if x + 1 < iw {
				d = d.min(dist[i + 1] + 1.0);
			}
			if y + 1 < ih {
				d = d.min(dist[(y + 1) * iw + x] + 1.0);
				if x > 0 {
					d = d.min(dist[(y + 1) * iw + x - 1] + SQRT2);
				}
				if x + 1 < iw {
					d = d.min(dist[(y + 1) * iw + x + 1] + SQRT2);
				}
			}
			dist[i] = d;
		}
	}
	dist
}

fn extract_alpha_contour_loops(mask: &[bool], iw: usize, ih: usize) -> Vec<Vec<(usize, usize)>> {
	let gw = iw + 1;
	let mut out_dirs = vec![0u8; gw * (ih + 1)];

	for y in 0..ih {
		for x in 0..iw {
			if !mask[y * iw + x] {
				continue;
			}
			if y == 0 || !mask[(y - 1) * iw + x] {
				out_dirs[y * gw + x] |= 1 << 0; // +X
			}
			if x + 1 == iw || !mask[y * iw + x + 1] {
				out_dirs[y * gw + (x + 1)] |= 1 << 1; // +Y
			}
			if y + 1 == ih || !mask[(y + 1) * iw + x] {
				out_dirs[(y + 1) * gw + (x + 1)] |= 1 << 2; // -X
			}
			if x == 0 || !mask[y * iw + x - 1] {
				out_dirs[(y + 1) * gw + x] |= 1 << 3; // -Y
			}
		}
	}

	let step_dir = |vx: usize, vy: usize, dir: u8| -> (usize, usize) {
		match dir {
			0 => (vx + 1, vy),
			1 => (vx, vy + 1),
			2 => (vx - 1, vy),
			_ => (vx, vy - 1),
		}
	};

	let mut loops = Vec::new();
	for sy in 0..=ih {
		for sx in 0..=iw {
			loop {
				let bits = out_dirs[sy * gw + sx];
				if bits == 0 {
					break;
				}
				let start_dir = (0..4u8).find(|&d| (bits & (1 << d)) != 0).unwrap();
				let mut cx = sx;
				let mut cy = sy;
				let mut dir = start_dir;
				let mut ring = Vec::new();
				let max_steps = (gw * (ih + 1)) * 2 + 4;

				for _ in 0..max_steps {
					let slot = &mut out_dirs[cy * gw + cx];
					if (*slot & (1 << dir)) == 0 {
						break;
					}
					*slot &= !(1 << dir);
					ring.push((cx, cy));
					let (nx, ny) = step_dir(cx, cy, dir);
					cx = nx;
					cy = ny;
					let next_bits = out_dirs[cy * gw + cx];
					// Keep filled interior on the right side of edge while closing saddle loops on start_dir.
					let prefs = [(dir + 1) % 4, dir, (dir + 3) % 4, (dir + 2) % 4];
					let next_dir = prefs.iter().copied().find(|&d| {
						(cx == sx && cy == sy && d == start_dir) || (next_bits & (1 << d)) != 0
					});
					match next_dir {
						Some(nd) if cx == sx && cy == sy && nd == start_dir => break,
						Some(nd) => dir = nd,
						None => break,
					}
				}
				if ring.len() >= 3 {
					loops.push(ring);
				}
			}
		}
	}
	loops
}

fn collapse_collinear_closed(pts: &[DelaunayPt], tol: f64) -> Vec<DelaunayPt> {
	let n = pts.len();
	if n < 3 {
		return pts.to_vec();
	}
	let mut out = Vec::with_capacity(n);
	for i in 0..n {
		let prev = pts[(i + n - 1) % n];
		let curr = pts[i];
		let next = pts[(i + 1) % n];
		let area = orient2d_f64(prev, curr, next);
		let d2_prev = (curr.x - prev.x).powi(2) + (curr.y - prev.y).powi(2);
		let d2_next = (next.x - curr.x).powi(2) + (next.y - curr.y).powi(2);
		let d_span = ((next.x - prev.x).powi(2) + (next.y - prev.y).powi(2)).sqrt();
		if d2_prev > 1e-12 && d2_next > 1e-12 && (d_span <= 1e-12 || area.abs() > tol * d_span) {
			out.push(curr);
		}
	}
	out
}

fn perp_dist_point_segment(p: DelaunayPt, a: DelaunayPt, b: DelaunayPt) -> f64 {
	let dx = b.x - a.x;
	let dy = b.y - a.y;
	let len = (dx * dx + dy * dy).sqrt();
	if len <= 1e-12 {
		return ((p.x - a.x).powi(2) + (p.y - a.y).powi(2)).sqrt();
	}
	((dy * p.x - dx * p.y + b.x * a.y - b.y * a.x).abs()) / len
}

fn dp_open_recursive(pts: &[DelaunayPt], eps: f64, out: &mut Vec<DelaunayPt>) {
	if pts.len() <= 2 {
		out.push(pts[0]);
		return;
	}
	let a = pts[0];
	let b = pts[pts.len() - 1];
	let mut best_idx = 0usize;
	let mut best_d = -1.0f64;
	for (i, &p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
		let d = perp_dist_point_segment(p, a, b);
		if d > best_d {
			best_d = d;
			best_idx = i;
		}
	}
	if best_d > eps && best_idx > 0 {
		dp_open_recursive(&pts[..=best_idx], eps, out);
		dp_open_recursive(&pts[best_idx..], eps, out);
	} else {
		out.push(a);
	}
}

fn douglas_peucker_closed(pts: &[DelaunayPt], eps: f64) -> Vec<DelaunayPt> {
	let n = pts.len();
	if n <= 4 {
		return pts.to_vec();
	}
	let mut i_a = 0usize;
	let mut best_d2 = -1.0f64;
	for (i, &p) in pts.iter().enumerate().skip(1) {
		let d2 = (p.x - pts[0].x).powi(2) + (p.y - pts[0].y).powi(2);
		if d2 > best_d2 {
			best_d2 = d2;
			i_a = i;
		}
	}
	let mut i_b = (i_a + 1) % n;
	let mut best_d2_b = -1.0f64;
	for (i, &p) in pts.iter().enumerate() {
		let d2 = (p.x - pts[i_a].x).powi(2) + (p.y - pts[i_a].y).powi(2);
		if d2 > best_d2_b {
			best_d2_b = d2;
			i_b = i;
		}
	}
	if i_a == i_b {
		return pts.to_vec();
	}

	let mut chain1 = Vec::new();
	let mut k = i_a;
	loop {
		chain1.push(pts[k]);
		if k == i_b {
			break;
		}
		k = (k + 1) % n;
	}
	let mut chain2 = Vec::new();
	let mut k = i_b;
	loop {
		chain2.push(pts[k]);
		if k == i_a {
			break;
		}
		k = (k + 1) % n;
	}

	let mut res = Vec::new();
	dp_open_recursive(&chain1, eps, &mut res);
	dp_open_recursive(&chain2, eps, &mut res);
	collapse_collinear_closed(&res, 1e-6)
}

fn polygon_signed_area(pts: &[DelaunayPt]) -> f64 {
	let n = pts.len();
	let mut s = 0.0f64;
	for i in 0..n {
		let a = pts[i];
		let b = pts[(i + 1) % n];
		s += a.x * b.y - b.x * a.y;
	}
	0.5 * s
}

fn point_in_loops(p: DelaunayPt, loops: &[Vec<DelaunayPt>]) -> bool {
	let mut inside = false;
	for lp in loops {
		let m = lp.len();
		if m < 3 {
			continue;
		}
		for i in 0..m {
			let a = lp[i];
			let b = lp[(i + 1) % m];
			let cond = (a.y > p.y) != (b.y > p.y);
			if cond {
				let x_int = (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x;
				if p.x < x_int {
					inside = !inside;
				}
			}
		}
	}
	inside
}

fn point_to_segment_dist_sq(p: DelaunayPt, a: DelaunayPt, b: DelaunayPt) -> f64 {
	let abx = b.x - a.x;
	let aby = b.y - a.y;
	let len2 = abx * abx + aby * aby;
	if len2 <= 1e-12 {
		return (p.x - a.x).powi(2) + (p.y - a.y).powi(2);
	}
	let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0);
	let px = a.x + abx * t;
	let py = a.y + aby * t;
	(p.x - px).powi(2) + (p.y - py).powi(2)
}

fn segments_properly_intersect(a: DelaunayPt, b: DelaunayPt, c: DelaunayPt, d: DelaunayPt) -> bool {
	let o1 = orient2d_f64(a, b, c);
	let o2 = orient2d_f64(a, b, d);
	let o3 = orient2d_f64(c, d, a);
	let o4 = orient2d_f64(c, d, b);
	let opp12 = (o1 > 1e-8 && o2 < -1e-8) || (o1 < -1e-8 && o2 > 1e-8);
	let opp34 = (o3 > 1e-8 && o4 < -1e-8) || (o3 < -1e-8 && o4 > 1e-8);
	opp12 && opp34
}

#[inline]
fn edge_key_uv(u: usize, v: usize) -> (usize, usize) {
	if u < v {
		(u, v)
	} else {
		(v, u)
	}
}

fn remove_tri_from_map(tris: &[[usize; 3]], edge_tris: &mut HashMap<(usize, usize), Vec<usize>>, ti: usize) {
	let t = tris[ti];
	for &(a, b) in &[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
		let k = edge_key_uv(a, b);
		if let Some(list) = edge_tris.get_mut(&k) {
			list.retain(|&x| x != ti);
			if list.is_empty() {
				edge_tris.remove(&k);
			}
		}
	}
}

fn add_tri_to_map(tris: &[[usize; 3]], edge_tris: &mut HashMap<(usize, usize), Vec<usize>>, ti: usize) {
	let t = tris[ti];
	for &(a, b) in &[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
		let k = edge_key_uv(a, b);
		let list = edge_tris.entry(k).or_default();
		if !list.contains(&ti) {
			list.push(ti);
		}
	}
}

fn extract_directed_opposite(tri: [usize; 3], p: usize, q: usize) -> Option<usize> {
	if tri[0] == p && tri[1] == q {
		Some(tri[2])
	} else if tri[1] == p && tri[2] == q {
		Some(tri[0])
	} else if tri[2] == p && tri[0] == q {
		Some(tri[1])
	} else {
		None
	}
}

fn enforce_constrained_edges(
	tris: &mut Vec<[usize; 3]>,
	pts: &[DelaunayPt],
	constrained_edges: &[(usize, usize)],
) {
	let mut edge_tris: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
	for ti in 0..tris.len() {
		add_tri_to_map(tris, &mut edge_tris, ti);
	}
	let constrained_set: HashSet<(usize, usize)> = constrained_edges
		.iter()
		.map(|&(u, v)| edge_key_uv(u, v))
		.collect();

	for &(u, v) in constrained_edges {
		let target_key = edge_key_uv(u, v);
		if edge_tris.contains_key(&target_key) {
			continue;
		}
		let mut crossing: VecDeque<(usize, usize)> = edge_tris
			.iter()
			.filter_map(|(&ek, tlist)| {
				if tlist.len() == 2
					&& !constrained_set.contains(&ek)
					&& ek.0 != u
					&& ek.0 != v
					&& ek.1 != u
					&& ek.1 != v
					&& segments_properly_intersect(pts[u], pts[v], pts[ek.0], pts[ek.1])
				{
					Some(ek)
				} else {
					None
				}
			})
			.collect();

		let mut new_edges: Vec<(usize, usize)> = Vec::new();
		let mut streak = 0usize;
		while let Some(ek) = crossing.pop_front() {
			let Some(tlist) = edge_tris.get(&ek) else {
				continue;
			};
			if tlist.len() != 2 {
				continue;
			}
			let t1 = tlist[0];
			let t2 = tlist[1];
			let (p, q, r, s) = if let (Some(r), Some(s)) = (
				extract_directed_opposite(tris[t1], ek.0, ek.1),
				extract_directed_opposite(tris[t2], ek.1, ek.0),
			) {
				(ek.0, ek.1, r, s)
			} else if let (Some(r), Some(s)) = (
				extract_directed_opposite(tris[t1], ek.1, ek.0),
				extract_directed_opposite(tris[t2], ek.0, ek.1),
			) {
				(ek.1, ek.0, r, s)
			} else {
				continue;
			};

			if r == s {
				continue;
			}

			// Quad (p, s, q, r) is strictly convex iff both flipped triangles are CCW.
			if orient2d_f64(pts[r], pts[p], pts[s]) > 1e-8 && orient2d_f64(pts[s], pts[q], pts[r]) > 1e-8 {
				remove_tri_from_map(tris, &mut edge_tris, t1);
				remove_tri_from_map(tris, &mut edge_tris, t2);
				tris[t1] = [r, p, s];
				tris[t2] = [s, q, r];
				add_tri_to_map(tris, &mut edge_tris, t1);
				add_tri_to_map(tris, &mut edge_tris, t2);
				streak = 0;

				let nk = edge_key_uv(r, s);
				if r != u && r != v && s != u && s != v && segments_properly_intersect(pts[u], pts[v], pts[r], pts[s]) {
					crossing.push_back(nk);
				} else {
					new_edges.push(nk);
				}
			} else {
				crossing.push_back(ek);
				streak += 1;
				if streak > crossing.len() * 2 + 16 {
					break;
				}
			}
		}

		// Step 2: Restore Delaunay condition on newly created unconstrained edges.
		let max_swaps = new_edges.len() * 12 + 64;
		let mut swaps = 0usize;
		while let Some(ek) = new_edges.pop() {
			if swaps >= max_swaps || ek == target_key || constrained_set.contains(&ek) {
				continue;
			}
			let Some(tlist) = edge_tris.get(&ek) else {
				continue;
			};
			if tlist.len() != 2 {
				continue;
			}
			let t1 = tlist[0];
			let t2 = tlist[1];
			let (p, q, r, s) = if let (Some(r), Some(s)) = (
				extract_directed_opposite(tris[t1], ek.0, ek.1),
				extract_directed_opposite(tris[t2], ek.1, ek.0),
			) {
				(ek.0, ek.1, r, s)
			} else if let (Some(r), Some(s)) = (
				extract_directed_opposite(tris[t1], ek.1, ek.0),
				extract_directed_opposite(tris[t2], ek.0, ek.1),
			) {
				(ek.1, ek.0, r, s)
			} else {
				continue;
			};

			if r != s
				&& orient2d_f64(pts[r], pts[p], pts[s]) > 1e-8
				&& orient2d_f64(pts[s], pts[q], pts[r]) > 1e-8
				&& in_circumcircle_ccw(pts[p], pts[q], pts[r], pts[s])
			{
				remove_tri_from_map(tris, &mut edge_tris, t1);
				remove_tri_from_map(tris, &mut edge_tris, t2);
				tris[t1] = [r, p, s];
				tris[t2] = [s, q, r];
				add_tri_to_map(tris, &mut edge_tris, t1);
				add_tri_to_map(tris, &mut edge_tris, t2);
				swaps += 1;
				for (ea, eb) in [(p, s), (s, q), (q, r), (r, p)] {
					let nek = edge_key_uv(ea, eb);
					if nek != target_key && !constrained_set.contains(&nek) {
						new_edges.push(nek);
					}
				}
			}
		}
	}
}

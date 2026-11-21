use std::collections::HashMap;
use std::io::Cursor;

use glam::{vec2, vec3, Vec2};
use image::{ImageFormat, RgbaImage};
use riglens_core::math::transform::TransformOffset;
use riglens_core::model::{Model, ModelTexture};
use riglens_core::node::components::{BlendMode, Blending, Drawable, Mesh, TexturedMesh};
use riglens_core::node::{InoxNode, InoxNodeUuid};
use riglens_core::texture::TextureId;

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

pub(super) fn mesh_bounds(mesh: &Mesh) -> (Vec2, Vec2) {
	let mut min_v = vec2(f32::INFINITY, f32::INFINITY);
	let mut max_v = vec2(f32::NEG_INFINITY, f32::NEG_INFINITY);
	for &v in &mesh.vertices {
		min_v = min_v.min(v);
		max_v = max_v.max(v);
	}
	(min_v, max_v)
}

use std::cell::RefCell;
use std::collections::HashMap;

use glam::{vec2, vec3, Mat4, Vec2, Vec4};
use image::{Rgba, RgbaImage};
use inox2d::math::camera::Camera;
use inox2d::model::Model;
use inox2d::node::components::{BlendMode, Mask, MaskMode, Masks};
use inox2d::node::drawables::{CompositeComponents, TexturedMeshComponents};
use inox2d::node::InoxNodeUuid;
use inox2d::puppet::Puppet;
use inox2d::render::{CompositeRenderCtx, InoxRenderer, InoxRendererExt, TexturedMeshRenderCtx};
use inox2d::texture::{decode_model_textures, ShallowTexture};

#[derive(Clone, Debug)]
pub struct RenderOptions {
	pub width: u32,
	pub height: u32,
	pub scale: f32,
	pub wireframe: bool,
	pub dt: f32,
	pub background: [u8; 4],
}

impl Default for RenderOptions {
	fn default() -> Self {
		Self {
			width: 512,
			height: 512,
			scale: 1.0,
			wireframe: false,
			dt: 0.0,
			background: [26, 28, 36, 255],
		}
	}
}

struct MaskState {
	active: bool,
	has_normal_mask: bool,
	threshold: f32,
	current_mode: MaskMode,
	stencil: Vec<u8>,
}

struct RenderState {
	canvas: RgbaImage,
	saved_canvas: Option<RgbaImage>,
	mask: MaskState,
}

struct SoftwareRenderer<'a> {
	puppet: &'a Puppet,
	textures: &'a [ShallowTexture],
	mvp_cam: Mat4,
	width: u32,
	height: u32,
	wireframe: bool,
	state: RefCell<RenderState>,
}

impl<'a> SoftwareRenderer<'a> {
	fn draw_line(&self, p0: Vec2, p1: Vec2, color: [u8; 4]) {
		let mut st = self.state.borrow_mut();
		let steps = (p1 - p0).length().ceil().max(1.0) as i32;
		for i in 0..=steps {
			let t = i as f32 / steps as f32;
			let p = p0.lerp(p1, t);
			let ix = p.x.round() as i32;
			let iy = p.y.round() as i32;
			if ix >= 0 && iy >= 0 && (ix as u32) < self.width && (iy as u32) < self.height {
				let px = st.canvas.get_pixel_mut(ix as u32, iy as u32);
				let a = color[3] as f32 / 255.0;
				for c in 0..3 {
					px.0[c] = (px.0[c] as f32 * (1.0 - a) + color[c] as f32 * a).clamp(0.0, 255.0) as u8;
				}
				px.0[3] = 255;
			}
		}
	}
}

fn edge_fn(a: Vec2, b: Vec2, c: Vec2) -> f32 {
	(b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn sample_bilinear(tex: &ShallowTexture, uv: Vec2) -> [f32; 4] {
	let w = tex.width();
	let h = tex.height();
	if w == 0 || h == 0 {
		return [0.0; 4];
	}
	let u = uv.x.clamp(0.0, 1.0) * (w as f32 - 1.0);
	let v = uv.y.clamp(0.0, 1.0) * (h as f32 - 1.0);
	let x0 = (u.floor() as u32).min(w - 1);
	let y0 = (v.floor() as u32).min(h - 1);
	let x1 = (x0 + 1).min(w - 1);
	let y1 = (y0 + 1).min(h - 1);
	let fx = u - x0 as f32;
	let fy = v - y0 as f32;

	let px = tex.pixels();
	let fetch = |x: u32, y: u32| -> [f32; 4] {
		let idx = ((y * w + x) * 4) as usize;
		if idx + 3 < px.len() {
			[
				px[idx] as f32 / 255.0,
				px[idx + 1] as f32 / 255.0,
				px[idx + 2] as f32 / 255.0,
				px[idx + 3] as f32 / 255.0,
			]
		} else {
			[0.0; 4]
		}
	};

	let c00 = fetch(x0, y0);
	let c10 = fetch(x1, y0);
	let c01 = fetch(x0, y1);
	let c11 = fetch(x1, y1);

	let mut out = [0.0; 4];
	for i in 0..4 {
		let top = c00[i] * (1.0 - fx) + c10[i] * fx;
		let bot = c01[i] * (1.0 - fx) + c11[i] * fx;
		out[i] = top * (1.0 - fy) + bot * fy;
	}
	out
}

fn blend_pixel(dst: &mut [u8; 4], src: [f32; 4], mode: BlendMode) {
	let sa = src[3].clamp(0.0, 1.0);
	if sa <= 0.001 {
		return;
	}
	let dr = dst[0] as f32 / 255.0;
	let dg = dst[1] as f32 / 255.0;
	let db = dst[2] as f32 / 255.0;
	let da = dst[3] as f32 / 255.0;

	let (sr, sg, sb) = match mode {
		BlendMode::Normal | BlendMode::ClipToLower | BlendMode::SliceFromLower => (src[0], src[1], src[2]),
		BlendMode::Multiply => (src[0] * dr, src[1] * dg, src[2] * db),
		BlendMode::Screen => (
			1.0 - (1.0 - src[0]) * (1.0 - dr),
			1.0 - (1.0 - src[1]) * (1.0 - dg),
			1.0 - (1.0 - src[2]) * (1.0 - db),
		),
		BlendMode::LinearDodge => ((src[0] + dr).min(1.0), (src[1] + dg).min(1.0), (src[2] + db).min(1.0)),
		BlendMode::ColorDodge => (
			if src[0] >= 0.999 { 1.0 } else { (dr / (1.0 - src[0])).min(1.0) },
			if src[1] >= 0.999 { 1.0 } else { (dg / (1.0 - src[1])).min(1.0) },
			if src[2] >= 0.999 { 1.0 } else { (db / (1.0 - src[2])).min(1.0) },
		),
	};

	let out_a = sa + da * (1.0 - sa);
	if out_a <= 1e-6 {
		return;
	}
	let or = (sr * sa + dr * da * (1.0 - sa)) / out_a;
	let og = (sg * sa + dg * da * (1.0 - sa)) / out_a;
	let ob = (sb * sa + db * da * (1.0 - sa)) / out_a;

	dst[0] = (or * 255.0).clamp(0.0, 255.0) as u8;
	dst[1] = (og * 255.0).clamp(0.0, 255.0) as u8;
	dst[2] = (ob * 255.0).clamp(0.0, 255.0) as u8;
	dst[3] = (out_a * 255.0).clamp(0.0, 255.0) as u8;
}

impl<'a> InoxRenderer for SoftwareRenderer<'a> {
	fn on_begin_masks(&self, masks: &Masks) {
		let mut st = self.state.borrow_mut();
		let has_normal = masks.has_masks();
		st.mask.active = true;
		st.mask.has_normal_mask = has_normal;
		st.mask.threshold = masks.threshold;
		let fill = if has_normal { 0 } else { 1 };
		st.mask.stencil.fill(fill);
	}

	fn on_begin_mask(&self, mask: &Mask) {
		let mut st = self.state.borrow_mut();
		st.mask.current_mode = match mask.mode {
			MaskMode::Mask => MaskMode::Mask,
			MaskMode::Dodge => MaskMode::Dodge,
		};
	}

	fn on_begin_masked_content(&self) {}

	fn on_end_mask(&self) {
		let mut st = self.state.borrow_mut();
		st.mask.active = false;
	}

	fn draw_textured_mesh_content(
		&self,
		as_mask: bool,
		components: &TexturedMeshComponents,
		render_ctx: &TexturedMeshRenderCtx,
		id: InoxNodeUuid,
	) {
		if let Some(node) = self.puppet.nodes.get_node(id) {
			if !node.enabled {
				return;
			}
		}
		let tex_id = components.texture.tex_albedo.0;
		let Some(tex) = self.textures.get(tex_id) else {
			return;
		};
		let Some(puppet_render_ctx) = self.puppet.render_ctx.as_ref() else {
			return;
		};

		let mesh = components.mesh;
		let vert_offset = render_ctx.vert_offset as usize;
		let vert_len = render_ctx.vert_len;
		let deforms = &puppet_render_ctx.vertex_buffers.deforms[vert_offset..vert_offset + vert_len];

		let mvp = self.mvp_cam * *components.transform;
		let wf = self.width as f32;
		let hf = self.height as f32;

		let screen_verts: Vec<Vec2> = mesh
			.vertices
			.iter()
			.zip(deforms.iter())
			.map(|(&v, &d)| {
				let local = vec3(v.x - mesh.origin.x + d.x, v.y - mesh.origin.y + d.y, 0.0);
				let ndc: Vec4 = mvp * local.extend(1.0);
				let x = (ndc.x * 0.5 + 0.5) * wf;
				let y = (0.5 - ndc.y * 0.5) * hf;
				vec2(x, y)
			})
			.collect();

		let tint = components.drawable.blending.tint;
		let screen_tint = components.drawable.blending.screen_tint;
		let opacity = components.drawable.blending.opacity;
		let mode = components.drawable.blending.mode;

		{
			let mut st = self.state.borrow_mut();
			let w = self.width;
			let h = self.height;

			for tri in mesh.indices.chunks_exact(3) {
				let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
				if i0 >= screen_verts.len() || i1 >= screen_verts.len() || i2 >= screen_verts.len() {
					continue;
				}
				let p0 = screen_verts[i0];
				let mut p1 = screen_verts[i1];
				let mut p2 = screen_verts[i2];
				let uv0 = mesh.uvs[i0];
				let mut uv1 = mesh.uvs[i1];
				let mut uv2 = mesh.uvs[i2];

				let mut area = edge_fn(p0, p1, p2);
				if area.abs() < 1e-4 {
					continue;
				}
				if area < 0.0 {
					std::mem::swap(&mut p1, &mut p2);
					std::mem::swap(&mut uv1, &mut uv2);
					area = -area;
				}

				let min_x = p0.x.min(p1.x).min(p2.x).floor().max(0.0) as u32;
				let min_y = p0.y.min(p1.y).min(p2.y).floor().max(0.0) as u32;
				let max_x = p0.x.max(p1.x).max(p2.x).ceil().min(w as f32 - 1.0).max(0.0) as u32;
				let max_y = p0.y.max(p1.y).max(p2.y).ceil().min(h as f32 - 1.0).max(0.0) as u32;

				for py in min_y..=max_y {
					for px in min_x..=max_x {
						let p = vec2(px as f32 + 0.5, py as f32 + 0.5);
						let w0 = edge_fn(p1, p2, p);
						let w1 = edge_fn(p2, p0, p);
						let w2 = edge_fn(p0, p1, p);
						if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
							continue;
						}
						let b0 = w0 / area;
						let b1 = w1 / area;
						let b2 = w2 / area;
						let uv = uv0 * b0 + uv1 * b1 + uv2 * b2;
						let tc = sample_bilinear(tex, uv);

						let idx = (py * w + px) as usize;
						if as_mask {
							if tc[3] > st.mask.threshold {
								st.mask.stencil[idx] = match st.mask.current_mode {
									MaskMode::Mask => 1,
									MaskMode::Dodge => 0,
								};
							}
							continue;
						}

						if st.mask.active && st.mask.stencil[idx] == 0 {
							continue;
						}

						// Apply screenTint + tint + opacity (matches basic.frag)
						let a = tc[3];
						let sr = 1.0 - (1.0 - tc[0]) * (1.0 - screen_tint.x * a);
						let sg = 1.0 - (1.0 - tc[1]) * (1.0 - screen_tint.y * a);
						let sb = 1.0 - (1.0 - tc[2]) * (1.0 - screen_tint.z * a);
						let col = [
							(sr * tint.x).clamp(0.0, 1.0),
							(sg * tint.y).clamp(0.0, 1.0),
							(sb * tint.z).clamp(0.0, 1.0),
							(a * opacity).clamp(0.0, 1.0),
						];

						let dst = st.canvas.get_pixel_mut(px, py);
						blend_pixel(&mut dst.0, col, mode);
					}
				}
			}
		}

		if self.wireframe && !as_mask {
			for tri in mesh.indices.chunks_exact(3) {
				let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
				if i0 >= screen_verts.len() || i1 >= screen_verts.len() || i2 >= screen_verts.len() {
					continue;
				}
				let wire_col = [0, 230, 190, 130];
				self.draw_line(screen_verts[i0], screen_verts[i1], wire_col);
				self.draw_line(screen_verts[i1], screen_verts[i2], wire_col);
				self.draw_line(screen_verts[i2], screen_verts[i0], wire_col);
			}
		}
	}

	fn begin_composite_content(
		&self,
		_as_mask: bool,
		_components: &CompositeComponents,
		_render_ctx: &CompositeRenderCtx,
		_id: InoxNodeUuid,
	) {
		let mut st = self.state.borrow_mut();
		let old = std::mem::replace(&mut st.canvas, RgbaImage::from_pixel(self.width, self.height, Rgba([0, 0, 0, 0])));
		st.saved_canvas = Some(old);
	}

	fn finish_composite_content(
		&self,
		_as_mask: bool,
		components: &CompositeComponents,
		_render_ctx: &CompositeRenderCtx,
		_id: InoxNodeUuid,
	) {
		let mut st = self.state.borrow_mut();
		if let Some(mut parent) = st.saved_canvas.take() {
			let layer = std::mem::replace(&mut st.canvas, RgbaImage::new(0, 0));
			let op = components.drawable.blending.opacity;
			let mode = components.drawable.blending.mode;
			for y in 0..self.height {
				for x in 0..self.width {
					let sp = layer.get_pixel(x, y).0;
					if sp[3] == 0 {
						continue;
					}
					let src = [
						sp[0] as f32 / 255.0,
						sp[1] as f32 / 255.0,
						sp[2] as f32 / 255.0,
						(sp[3] as f32 / 255.0) * op,
					];
					blend_pixel(&mut parent.get_pixel_mut(x, y).0, src, mode);
				}
			}
			st.canvas = parent;
		}
	}
}

/// Evaluate `model` at the given parameter overrides and render a frame to `RgbaImage`.
pub fn render_frame(
	model: &mut Model,
	params: &HashMap<String, Vec2>,
	opts: &RenderOptions,
) -> Result<RgbaImage, String> {
	model.puppet.init_all();
	model.puppet.begin_frame();
	if let Some(pctx) = model.puppet.param_ctx.as_mut() {
		for (name, val) in params {
			pctx.set(name, *val).map_err(|e| e.to_string())?;
		}
	}
	model.puppet.end_frame(opts.dt);

	let decoded = decode_model_textures(model.textures.iter());
	let camera = Camera {
		position: vec2(0.0, -35.0),
		rotation: 0.0,
		scale: vec2(opts.scale, opts.scale),
	};
	let viewport = vec2(opts.width as f32, opts.height as f32);
	let mvp_cam = camera.matrix(viewport);

	let renderer = SoftwareRenderer {
		puppet: &model.puppet,
		textures: &decoded,
		mvp_cam,
		width: opts.width,
		height: opts.height,
		wireframe: opts.wireframe,
		state: RefCell::new(RenderState {
			canvas: RgbaImage::from_pixel(opts.width, opts.height, Rgba(opts.background)),
			saved_canvas: None,
			mask: MaskState {
				active: false,
				has_normal_mask: false,
				threshold: 0.5,
				current_mode: MaskMode::Mask,
				stencil: vec![0; (opts.width * opts.height) as usize],
			},
		}),
	};

	renderer.draw(&model.puppet);
	Ok(renderer.state.into_inner().canvas)
}

/// Render a 3x3 contact sheet sweeping a 2D parameter across `[min..max]` in X and Y
/// (or 3 samples along X if 1D) for AI multimodal self-inspection.
pub fn render_grid(
	model: &mut Model,
	param_name: &str,
	tile_size: u32,
	wireframe: bool,
	base_params: &HashMap<String, Vec2>,
) -> Result<RgbaImage, String> {
	let param = model
		.puppet
		.params
		.get(param_name)
		.ok_or_else(|| format!("Parameter '{param_name}' not found"))?;
	let min = param.min;
	let max = param.max;
	let is_vec2 = param.is_vec2;

	let gap = 4u32;
	let cols = 3u32;
	let rows = if is_vec2 { 3u32 } else { 1u32 };
	let total_w = cols * tile_size + (cols + 1) * gap;
	let total_h = rows * tile_size + (rows + 1) * gap;

	let mut sheet = RgbaImage::from_pixel(total_w, total_h, Rgba([16, 18, 24, 255]));
	let tile_scale = tile_size as f32 / 384.0;

	for ry in 0..rows {
		let ty = if rows == 1 { 0.0 } else { ry as f32 / 2.0 };
		let py_val = min.y + (max.y - min.y) * ty;
		for rx in 0..cols {
			let tx = rx as f32 / 2.0;
			let px_val = min.x + (max.x - min.x) * tx;

			let mut frame_params = base_params.clone();
			frame_params.insert(param_name.to_owned(), vec2(px_val, py_val));

			let opts = RenderOptions {
				width: tile_size,
				height: tile_size,
				scale: tile_scale,
				wireframe,
				dt: 0.0,
				background: [30, 33, 43, 255],
			};
			let tile = render_frame(model, &frame_params, &opts)?;

			let ox = gap + rx * (tile_size + gap);
			let oy = gap + ry * (tile_size + gap);
			for y in 0..tile_size {
				for x in 0..tile_size {
					sheet.put_pixel(ox + x, oy + y, *tile.get_pixel(x, y));
				}
			}
		}
	}

	Ok(sheet)
}

/// Render `frames` sampled frames of a named animation (or a synthetic VTuber motion test if `anim_name` is `None`)
/// into a contact sheet PNG so AI can inspect motion, blinks, and physics behavior over time.
pub fn render_anim_sheet(
	model: &mut Model,
	anim_name: Option<&str>,
	frames: u32,
	tile_size: u32,
	wireframe: bool,
) -> Result<RgbaImage, String> {
	let frames = frames.clamp(1, 24);
	let cols = frames.min(4);
	let rows = frames.div_ceil(cols);
	let gap = 4u32;
	let total_w = cols * tile_size + (cols + 1) * gap;
	let total_h = rows * tile_size + (rows + 1) * gap;

	let mut sheet = RgbaImage::from_pixel(total_w, total_h, Rgba([16, 18, 24, 255]));
	let tile_scale = tile_size as f32 / 384.0;

	let anim = match anim_name {
		Some(name) => Some(
			model
				.puppet
				.animations
				.get(name)
				.cloned()
				.ok_or_else(|| format!("Animation '{name}' not found"))?,
		),
		None => model.puppet.animations.values().next().cloned(),
	};

	for i in 0..frames {
		let t = if frames == 1 {
			0.0
		} else {
			i as f32 / (frames - 1) as f32
		};
		let (frame_params, dt) = if let Some(ref a) = anim {
			let f = t * a.length.max(1) as f32;
			(a.eval_at_frame(f, &model.puppet.params), a.timestep * 4.0)
		} else {
			let mut m = HashMap::new();
			let angle = t * std::f32::consts::TAU;
			m.insert("Head:: Yaw-Pitch".to_owned(), vec2(angle.sin() * 0.85, angle.cos() * 0.45));
			let blink = if (t - 0.5).abs() < 0.12 { 0.0 } else { 1.0 };
			m.insert("Eye:: Left:: Open".to_owned(), vec2(blink, 0.0));
			m.insert("Eye:: Right:: Open".to_owned(), vec2(blink, 0.0));
			m.insert("Mouth:: Open".to_owned(), vec2((angle * 2.0).sin().abs(), 0.0));
			(m, 0.06)
		};

		let opts = RenderOptions {
			width: tile_size,
			height: tile_size,
			scale: tile_scale,
			wireframe,
			dt,
			background: [30, 33, 43, 255],
		};
		let tile = render_frame(model, &frame_params, &opts)?;

		let rx = i % cols;
		let ry = i / cols;
		let ox = gap + rx * (tile_size + gap);
		let oy = gap + ry * (tile_size + gap);
		for y in 0..tile_size {
			for x in 0..tile_size {
				sheet.put_pixel(ox + x, oy + y, *tile.get_pixel(x, y));
			}
		}
	}

	Ok(sheet)
}

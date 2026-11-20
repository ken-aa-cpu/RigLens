use std::cell::RefCell;
use std::collections::HashMap;

use glam::{vec2, vec3, Mat4, Vec2, Vec3, Vec4};
use image::{Rgba, RgbaImage};
use inox2d::math::camera::Camera;
use inox2d::model::Model;
use inox2d::node::components::{BlendMode, Mask, MaskMode, Masks, Mesh, TexturedMesh};
use inox2d::node::drawables::{CompositeComponents, TexturedMeshComponents};
use inox2d::node::InoxNodeUuid;
use inox2d::puppet::Puppet;
use inox2d::render::{CompositeRenderCtx, InoxRenderer, InoxRendererExt, TexturedMeshRenderCtx};
use inox2d::texture::{decode_model_textures, ShallowTexture};

use crate::audit::{find_face_node, FeatureRole};
use crate::gif_encode::encode_animated_gif;

#[derive(Clone, Debug)]
pub struct RenderOptions {
	pub width: u32,
	pub height: u32,
	pub scale: f32,
	pub camera_pos: Vec2,
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
			camera_pos: vec2(0.0, -35.0),
			wireframe: false,
			dt: 0.0,
			background: [26, 28, 36, 255],
		}
	}
}

#[derive(Clone)]
struct MaskState {
	active: bool,
	has_normal_mask: bool,
	threshold: f32,
	current_mode: MaskMode,
	stencil: Vec<u8>,
}

struct RenderState {
	canvas: RgbaImage,
	canvas_stack: Vec<(RgbaImage, MaskState)>,
	mask: MaskState,
}

struct SoftwareRenderer<'a> {
	puppet: &'a Puppet,
	textures: &'a [ShallowTexture],
	tex_is_premul: Vec<bool>,
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
	if w == 0 || h == 0 || !uv.is_finite() {
		return [0.0; 4];
	}
	// Software GL_CLAMP_TO_BORDER([0,0,0,0]) so meshes with padded OOB UVs don't streak edge pixels.
	let wf = w as f32;
	let hf = h as f32;
	let u = uv.x * wf - 0.5;
	let v = uv.y * hf - 0.5;
	if u < -1.0 || v < -1.0 || u > wf || v > hf {
		return [0.0; 4];
	}
	let x0 = u.floor() as i32;
	let y0 = v.floor() as i32;
	let x1 = x0 + 1;
	let y1 = y0 + 1;
	let fx = u - x0 as f32;
	let fy = v - y0 as f32;

	let px = tex.pixels();
	let fetch = |x: i32, y: i32| -> [f32; 4] {
		if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
			return [0.0; 4];
		}
		let idx = ((y as u32 * w + x as u32) * 4) as usize;
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

/// Separable per-channel blend function (source `s`, destination `d`, both 0..=1).
fn blend_channel(mode: BlendMode, s: f32, d: f32) -> f32 {
	let v = match mode {
		BlendMode::Normal | BlendMode::ClipToLower | BlendMode::SliceFromLower | BlendMode::DestinationIn => s,
		BlendMode::Multiply => s * d,
		BlendMode::Screen => 1.0 - (1.0 - s) * (1.0 - d),
		BlendMode::Overlay => {
			if d < 0.5 {
				2.0 * s * d
			} else {
				1.0 - 2.0 * (1.0 - s) * (1.0 - d)
			}
		}
		BlendMode::HardLight => {
			if s < 0.5 {
				2.0 * s * d
			} else {
				1.0 - 2.0 * (1.0 - s) * (1.0 - d)
			}
		}
		BlendMode::SoftLight => {
			if s <= 0.5 {
				d - (1.0 - 2.0 * s) * d * (1.0 - d)
			} else {
				let g = if d <= 0.25 {
					((16.0 * d - 12.0) * d + 4.0) * d
				} else {
					d.sqrt()
				};
				d + (2.0 * s - 1.0) * (g - d)
			}
		}
		BlendMode::Darken => s.min(d),
		BlendMode::Lighten => s.max(d),
		BlendMode::ColorDodge => {
			if s >= 0.999 {
				1.0
			} else {
				d / (1.0 - s)
			}
		}
		BlendMode::LinearDodge | BlendMode::AddGlow => s + d,
		BlendMode::ColorBurn => {
			if s <= 0.001 {
				0.0
			} else {
				1.0 - (1.0 - d) / s
			}
		}
		BlendMode::Difference => (s - d).abs(),
		BlendMode::Exclusion => s + d - 2.0 * s * d,
		BlendMode::Subtract => d - s,
		BlendMode::Inverse => 1.0 - d,
	};
	v.clamp(0.0, 1.0)
}

fn blend_pixel(dst: &mut [u8; 4], src: [f32; 4], mode: BlendMode) {
	let sa = src[3].clamp(0.0, 1.0);
	let dr = dst[0] as f32 / 255.0;
	let dg = dst[1] as f32 / 255.0;
	let db = dst[2] as f32 / 255.0;
	let da = dst[3] as f32 / 255.0;

	match mode {
		BlendMode::ClipToLower => {
			if sa <= 0.001 || da <= 0.001 {
				return;
			}
			let or = src[0].clamp(0.0, 1.0) * sa + dr * (1.0 - sa);
			let og = src[1].clamp(0.0, 1.0) * sa + dg * (1.0 - sa);
			let ob = src[2].clamp(0.0, 1.0) * sa + db * (1.0 - sa);
			dst[0] = (or * 255.0).clamp(0.0, 255.0) as u8;
			dst[1] = (og * 255.0).clamp(0.0, 255.0) as u8;
			dst[2] = (ob * 255.0).clamp(0.0, 255.0) as u8;
			return;
		}
		BlendMode::SliceFromLower => {
			if sa <= 0.001 {
				return;
			}
			let eff_sa = sa * (1.0 - da);
			let out_a = eff_sa + da;
			if out_a <= 0.001 {
				return;
			}
			let or = (src[0].clamp(0.0, 1.0) * eff_sa + dr * da) / out_a;
			let og = (src[1].clamp(0.0, 1.0) * eff_sa + dg * da) / out_a;
			let ob = (src[2].clamp(0.0, 1.0) * eff_sa + db * da) / out_a;
			dst[0] = (or * 255.0).clamp(0.0, 255.0) as u8;
			dst[1] = (og * 255.0).clamp(0.0, 255.0) as u8;
			dst[2] = (ob * 255.0).clamp(0.0, 255.0) as u8;
			dst[3] = (out_a * 255.0).clamp(0.0, 255.0) as u8;
			return;
		}
		BlendMode::DestinationIn => {
			let out_a = (da * sa).clamp(0.0, 1.0);
			if out_a <= 0.001 {
				*dst = [0, 0, 0, 0];
			} else {
				dst[3] = (out_a * 255.0).clamp(0.0, 255.0) as u8;
			}
			return;
		}
		_ => {}
	}

	if sa <= 0.001 {
		return;
	}

	let sr = blend_channel(mode, src[0], dr);
	let sg = blend_channel(mode, src[1], dg);
	let sb = blend_channel(mode, src[2], db);

	let out_a = sa + da * (1.0 - sa);
	if out_a <= 1e-6 {
		return;
	}
	let or = (src[0] * sa * (1.0 - da) + dr * da * (1.0 - sa) + sr * sa * da) / out_a;
	let og = (src[1] * sa * (1.0 - da) + dg * da * (1.0 - sa) + sg * sa * da) / out_a;
	let ob = (src[2] * sa * (1.0 - da) + db * da * (1.0 - sa) + sb * sa * da) / out_a;

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
		let opacity = self
			.puppet
			.node_opacity(id)
			.unwrap_or(components.drawable.blending.opacity);
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
						let mut tc = sample_bilinear(tex, uv);

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

						let a = tc[3].clamp(0.0, 1.0);
						if a <= 0.0005 {
							continue;
						}
						let is_premul = self.tex_is_premul.get(components.texture.tex_albedo.0).copied().unwrap_or(false);
						if is_premul {
							tc[0] = (tc[0] / a).clamp(0.0, 1.0);
							tc[1] = (tc[1] / a).clamp(0.0, 1.0);
							tc[2] = (tc[2] / a).clamp(0.0, 1.0);
						}

						// Apply screenTint + tint + opacity on straight RGB before blend_pixel
						let sr = 1.0 - (1.0 - tc[0]) * (1.0 - screen_tint.x);
						let sg = 1.0 - (1.0 - tc[1]) * (1.0 - screen_tint.y);
						let sb = 1.0 - (1.0 - tc[2]) * (1.0 - screen_tint.z);
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
		let old_canvas = std::mem::replace(
			&mut st.canvas,
			RgbaImage::from_pixel(self.width, self.height, Rgba([0, 0, 0, 0])),
		);
		let old_mask = std::mem::replace(
			&mut st.mask,
			MaskState {
				active: false,
				has_normal_mask: false,
				threshold: 0.5,
				current_mode: MaskMode::Mask,
				stencil: vec![0; (self.width * self.height) as usize],
			},
		);
		st.canvas_stack.push((old_canvas, old_mask));
	}

	fn finish_composite_content(
		&self,
		as_mask: bool,
		components: &CompositeComponents,
		_render_ctx: &CompositeRenderCtx,
		_id: InoxNodeUuid,
	) {
		let mut st = self.state.borrow_mut();
		if let Some((mut parent, mut parent_mask)) = st.canvas_stack.pop() {
			let layer = std::mem::replace(&mut st.canvas, RgbaImage::new(0, 0));
			let op = components.opacity.clamp(0.0, 1.0);
			if as_mask {
				for y in 0..self.height {
					for x in 0..self.width {
						let idx = (y * self.width + x) as usize;
						let a = (layer.get_pixel(x, y).0[3] as f32 / 255.0) * op;
						if a > parent_mask.threshold {
							parent_mask.stencil[idx] = match parent_mask.current_mode {
								MaskMode::Mask => 1,
								MaskMode::Dodge => 0,
							};
						}
					}
				}
				st.canvas = parent;
				st.mask = parent_mask;
				return;
			}
			let mode = components.drawable.blending.mode;
			let tint = components.drawable.blending.tint.clamp(Vec3::ZERO, Vec3::ONE);
			let screen_tint = components.drawable.blending.screen_tint.clamp(Vec3::ZERO, Vec3::ONE);
			for y in 0..self.height {
				for x in 0..self.width {
					let idx = (y * self.width + x) as usize;
					if parent_mask.active && parent_mask.stencil[idx] == 0 {
						continue;
					}
					let sp = layer.get_pixel(x, y).0;
					let a = sp[3] as f32 / 255.0;
					if sp[3] == 0 && mode != BlendMode::DestinationIn {
						continue;
					}
					let tr = sp[0] as f32 / 255.0;
					let tg = sp[1] as f32 / 255.0;
					let tb = sp[2] as f32 / 255.0;
					let sr = 1.0 - (1.0 - tr) * (1.0 - screen_tint.x * a);
					let sg = 1.0 - (1.0 - tg) * (1.0 - screen_tint.y * a);
					let sb = 1.0 - (1.0 - tb) * (1.0 - screen_tint.z * a);
					let src = [
						(sr * tint.x).clamp(0.0, 1.0),
						(sg * tint.y).clamp(0.0, 1.0),
						(sb * tint.z).clamp(0.0, 1.0),
						(a * op).clamp(0.0, 1.0),
					];
					blend_pixel(&mut parent.get_pixel_mut(x, y).0, src, mode);
				}
			}
			st.canvas = parent;
			st.mask = parent_mask;
		}
	}
}

/// Evaluate `model` at the given parameter overrides and render a frame using caller-decoded textures.
pub fn render_frame_with_textures(
	model: &mut Model,
	decoded: &[ShallowTexture],
	params: &HashMap<String, Vec2>,
	opts: &RenderOptions,
) -> Result<RgbaImage, String> {
	if model.puppet.render_ctx.is_none()
		|| model
			.puppet
			.param_ctx
			.as_ref()
			.map(|c| c.values().len() != model.puppet.params.len())
			.unwrap_or(true)
	{
		model.puppet.init_all();
	}
	model.puppet.begin_frame();
	if let Some(pctx) = model.puppet.param_ctx.as_mut() {
		for (name, val) in params {
			pctx.set(name, *val).map_err(|e| e.to_string())?;
		}
	}
	model.puppet.end_frame(opts.dt);

	let camera = Camera {
		position: opts.camera_pos,
		rotation: 0.0,
		scale: vec2(opts.scale, opts.scale),
	};
	let viewport = vec2(opts.width as f32, opts.height as f32);
	let mvp_cam = camera.matrix(viewport);

	let tex_is_premul: Vec<bool> = decoded
		.iter()
		.map(|t| {
			let mut has_semi = false;
			let mut exceeds = false;
			for px in t.pixels().chunks_exact(4) {
				if px[3] > 0 && px[3] < 255 {
					has_semi = true;
				}
				if px[3] > 0 && (px[0] > px[3] || px[1] > px[3] || px[2] > px[3]) {
					exceeds = true;
					break;
				}
			}
			has_semi && !exceeds
		})
		.collect();

	let renderer = SoftwareRenderer {
		puppet: &model.puppet,
		textures: decoded,
		tex_is_premul,
		mvp_cam,
		width: opts.width,
		height: opts.height,
		wireframe: opts.wireframe,
		state: RefCell::new(RenderState {
			canvas: RgbaImage::from_pixel(opts.width, opts.height, Rgba([0, 0, 0, 0])),
			canvas_stack: Vec::new(),
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
	let rendered = renderer.state.into_inner().canvas;
	if opts.background[3] == 0 {
		return Ok(rendered);
	}
	let mut out = RgbaImage::from_pixel(opts.width, opts.height, Rgba(opts.background));
	for y in 0..opts.height {
		for x in 0..opts.width {
			let sp = rendered.get_pixel(x, y).0;
			if sp[3] == 0 {
				continue;
			}
			let src = [
				sp[0] as f32 / 255.0,
				sp[1] as f32 / 255.0,
				sp[2] as f32 / 255.0,
				sp[3] as f32 / 255.0,
			];
			blend_pixel(&mut out.get_pixel_mut(x, y).0, src, BlendMode::Normal);
		}
	}
	Ok(out)
}

/// Evaluate `model` at the given parameter overrides and render a frame to `RgbaImage`.
pub fn render_frame(
	model: &mut Model,
	params: &HashMap<String, Vec2>,
	opts: &RenderOptions,
) -> Result<RgbaImage, String> {
	let decoded = decode_model_textures(model.textures.iter());
	render_frame_with_textures(model, &decoded, params, opts)
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
	let decoded = decode_model_textures(model.textures.iter());

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
				..RenderOptions::default()
			};
			let tile = render_frame_with_textures(model, &decoded, &frame_params, &opts)?;

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
			..RenderOptions::default()
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

/// Render a looping animated `.gif` of `anim_name` (or synthetic VTuber physics motion if none),
/// stepping physics continuously at `1.0 / fps` per frame.
pub fn render_anim_gif(
	model: &mut Model,
	anim_name: Option<&str>,
	frames: u32,
	fps: u32,
	width: u32,
	height: u32,
	scale: f32,
	wireframe: bool,
) -> Result<Vec<u8>, String> {
	let frames = frames.clamp(2, 120);
	let fps = fps.clamp(4, 60);
	let dt = 1.0 / fps as f32;
	let delay_cs = ((100.0 / fps as f32).round() as u16).max(2);

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

	let mut rendered = Vec::with_capacity(frames as usize);
	for i in 0..frames {
		let t = i as f32 / frames as f32;
		let frame_params = if let Some(ref a) = anim {
			let f = t * a.length.max(1) as f32;
			a.eval_at_frame(f, &model.puppet.params)
		} else {
			let mut m = HashMap::new();
			let angle = t * std::f32::consts::TAU;
			m.insert("Head:: Yaw-Pitch".to_owned(), vec2(angle.sin() * 0.85, angle.cos() * 0.45));
			let blink = if (t - 0.5).abs() < 0.08 { 0.0 } else { 1.0 };
			m.insert("Eye:: Left:: Open".to_owned(), vec2(blink, 0.0));
			m.insert("Eye:: Right:: Open".to_owned(), vec2(blink, 0.0));
			m.insert("Mouth:: Open".to_owned(), vec2((angle * 2.0).sin().abs(), 0.0));
			m
		};

		let opts = RenderOptions {
			width,
			height,
			scale,
			wireframe,
			dt,
			background: [26, 28, 36, 255],
			..RenderOptions::default()
		};
		rendered.push(render_frame(model, &frame_params, &opts)?);
	}

	let mut out = Vec::new();
	encode_animated_gif(&rendered, delay_cs, &mut out).map_err(|e| format!("GIF encoding error: {e}"))?;
	Ok(out)
}

/// Render a 3-panel visual diff (`Before | After | Pixel Diff Heatmap`) and return `(image, changed_pct)`.
pub fn render_diff(
	before_model: &mut Model,
	after_model: &mut Model,
	params: &HashMap<String, Vec2>,
	tile_size: u32,
	scale: f32,
) -> Result<(RgbaImage, f32), String> {
	let opts = RenderOptions {
		width: tile_size,
		height: tile_size,
		scale,
		wireframe: false,
		dt: 0.0,
		background: [26, 28, 36, 255],
		..RenderOptions::default()
	};
	let before_params: HashMap<String, Vec2> = params
		.iter()
		.filter(|(k, _)| before_model.puppet.params.contains_key(*k))
		.map(|(k, v)| (k.clone(), *v))
		.collect();
	let after_params: HashMap<String, Vec2> = params
		.iter()
		.filter(|(k, _)| after_model.puppet.params.contains_key(*k))
		.map(|(k, v)| (k.clone(), *v))
		.collect();

	let before_img = render_frame(before_model, &before_params, &opts)?;
	let after_img = render_frame(after_model, &after_params, &opts)?;

	let gap = 4u32;
	let total_w = tile_size * 3 + gap * 4;
	let total_h = tile_size + gap * 2;
	let mut out = RgbaImage::from_pixel(total_w, total_h, Rgba([16, 18, 24, 255]));

	let mut changed_pixels = 0u64;
	let total_pixels = (tile_size * tile_size).max(1) as u64;

	for y in 0..tile_size {
		for x in 0..tile_size {
			let bp = before_img.get_pixel(x, y).0;
			let ap = after_img.get_pixel(x, y).0;
			out.put_pixel(gap + x, gap + y, Rgba(bp));
			out.put_pixel(gap * 2 + tile_size + x, gap + y, Rgba(ap));

			let diff = (bp[0] as i32 - ap[0] as i32).abs()
				+ (bp[1] as i32 - ap[1] as i32).abs()
				+ (bp[2] as i32 - ap[2] as i32).abs();
			if diff > 12 {
				changed_pixels += 1;
				let heat = (diff.min(255)) as u8;
				out.put_pixel(gap * 3 + tile_size * 2 + x, gap + y, Rgba([255, 255 - heat / 2, 40, 255]));
			} else {
				out.put_pixel(
					gap * 3 + tile_size * 2 + x,
					gap + y,
					Rgba([ap[0] / 3, ap[1] / 3, ap[2] / 3, 255]),
				);
			}
		}
	}

	let changed_pct = (changed_pixels as f32 / total_pixels as f32) * 100.0;
	Ok((out, changed_pct))
}

fn draw_canvas_line(img: &mut RgbaImage, p0: Vec2, p1: Vec2, color: [u8; 4]) {
	let w = img.width() as i32;
	let h = img.height() as i32;
	let steps = (p1 - p0).length().ceil().max(1.0) as i32;
	let a = color[3] as f32 / 255.0;
	for i in 0..=steps {
		let t = i as f32 / steps as f32;
		let p = p0.lerp(p1, t);
		let ix = p.x.round() as i32;
		let iy = p.y.round() as i32;
		if ix >= 0 && iy >= 0 && ix < w && iy < h {
			let px = img.get_pixel_mut(ix as u32, iy as u32);
			for c in 0..3 {
				px.0[c] = (px.0[c] as f32 * (1.0 - a) + color[c] as f32 * a).clamp(0.0, 255.0) as u8;
			}
			px.0[3] = 255;
		}
	}
}

fn draw_crosshair(img: &mut RgbaImage, center: Vec2, radius: f32, color: [u8; 4]) {
	draw_canvas_line(img, center - vec2(radius, 0.0), center + vec2(radius, 0.0), color);
	draw_canvas_line(img, center - vec2(0.0, radius), center + vec2(0.0, radius), color);
}

/// Render an X-ray diagnostic PNG featuring:
/// 1) 50px world-space coordinate grid + origin axes
/// 2) Mesh wireframe with red/orange heatmap highlighting flipped or over-stretched triangles
/// 3) Green Face containment ellipse, yellow/red overflow markers, and Part pivot crosshairs
pub fn render_diagnostic(
	model: &mut Model,
	params: &HashMap<String, Vec2>,
	width: u32,
	height: u32,
	scale: f32,
) -> Result<RgbaImage, String> {
	let opts = RenderOptions {
		width,
		height,
		scale,
		wireframe: true,
		dt: 0.0,
		background: [20, 22, 30, 255],
		..RenderOptions::default()
	};
	let mut img = render_frame(model, params, &opts)?;

	let camera = Camera {
		position: vec2(0.0, -35.0),
		rotation: 0.0,
		scale: vec2(scale, scale),
	};
	let viewport = vec2(width as f32, height as f32);
	let mvp_cam = camera.matrix(viewport);
	let world_to_screen = |wpos: Vec2| -> Vec2 {
		let ndc = mvp_cam * vec3(wpos.x, wpos.y, 0.0).extend(1.0);
		vec2((ndc.x * 0.5 + 0.5) * width as f32, (0.5 - ndc.y * 0.5) * height as f32)
	};

	// 1. 50px coordinate grid & center axes
	for gx in (-400..=400).step_by(50) {
		let is_axis = gx == 0;
		let col = if is_axis { [140, 160, 210, 120] } else { [80, 90, 120, 55] };
		draw_canvas_line(&mut img, world_to_screen(vec2(gx as f32, -400.0)), world_to_screen(vec2(gx as f32, 400.0)), col);
		draw_canvas_line(&mut img, world_to_screen(vec2(-400.0, gx as f32)), world_to_screen(vec2(400.0, gx as f32)), col);
	}

	// Collect Face containment ellipse in deformed world space
	let face_ellipse = find_face_node(&model.puppet).and_then(|fid| {
		let mesh = model.puppet.node_comps.get::<Mesh>(fid)?;
		let abs = model.puppet.node_transform(fid)?.absolute;
		let rc = model.puppet.render_ctx.as_ref()?;
		let tm_rc = model.puppet.node_comps.get::<TexturedMeshRenderCtx>(fid)?;
		let off = tm_rc.vert_offset as usize;
		let deforms = &rc.vertex_buffers.deforms[off..off + tm_rc.vert_len];
		let mut mn = vec2(f32::INFINITY, f32::INFINITY);
		let mut mx = vec2(f32::NEG_INFINITY, f32::NEG_INFINITY);
		for (i, &v) in mesh.vertices.iter().enumerate() {
			let d = deforms.get(i).copied().unwrap_or(Vec2::ZERO);
			let w = abs.transform_point3(vec3(v.x - mesh.origin.x + d.x, v.y - mesh.origin.y + d.y, 0.0)).truncate();
			mn = mn.min(w);
			mx = mx.max(w);
		}
		let fc = (mn + mx) * 0.5;
		let fh = ((mx - mn) * 0.5).max(vec2(1.0, 1.0));
		Some((fc, fh))
	});

	if let Some((fc, fh)) = face_ellipse {
		let rx = fh.x * 0.96;
		let ry = fh.y * 0.98;
		let steps = 48;
		for i in 0..steps {
			let a0 = (i as f32 / steps as f32) * std::f32::consts::TAU;
			let a1 = ((i + 1) as f32 / steps as f32) * std::f32::consts::TAU;
			let p0 = world_to_screen(fc + vec2(a0.cos() * rx, a0.sin() * ry));
			let p1 = world_to_screen(fc + vec2(a1.cos() * rx, a1.sin() * ry));
			draw_canvas_line(&mut img, p0, p1, [60, 240, 120, 210]);
		}
	}

	// 2 & 3. Anomaly triangle heatmap + overflow markers + pivot crosshairs
	let part_ids: Vec<(InoxNodeUuid, String)> = model
		.puppet
		.nodes
		.pre_order_iter()
		.filter_map(|n| {
			if n.enabled && model.puppet.node_comps.get::<TexturedMesh>(n.uuid).is_some() {
				Some((n.uuid, n.name.clone()))
			} else {
				None
			}
		})
		.collect();

	if let Some(rc) = model.puppet.render_ctx.as_ref() {
		for (id, name) in part_ids {
			let Some(mesh) = model.puppet.node_comps.get::<Mesh>(id) else { continue };
			let Some(ts) = model.puppet.node_transform(id) else { continue };
			let Some(tm_rc) = model.puppet.node_comps.get::<TexturedMeshRenderCtx>(id) else { continue };

			let pivot = world_to_screen(ts.absolute.transform_point3(vec3(0.0, 0.0, 0.0)).truncate());
			draw_crosshair(&mut img, pivot, 5.0, [255, 220, 80, 220]);

			let off = tm_rc.vert_offset as usize;
			let deforms = &rc.vertex_buffers.deforms[off..off + tm_rc.vert_len];
			let rest_w: Vec<Vec2> = mesh
				.vertices
				.iter()
				.map(|&v| ts.absolute.transform_point3(vec3(v.x - mesh.origin.x, v.y - mesh.origin.y, 0.0)).truncate())
				.collect();
			let def_w: Vec<Vec2> = mesh
				.vertices
				.iter()
				.enumerate()
				.map(|(i, &v)| {
					let d = deforms.get(i).copied().unwrap_or(Vec2::ZERO);
					ts.absolute
						.transform_point3(vec3(v.x - mesh.origin.x + d.x, v.y - mesh.origin.y + d.y, 0.0))
						.truncate()
				})
				.collect();

			for tri in mesh.indices.chunks_exact(3) {
				let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
				let ra = (rest_w[i1].x - rest_w[i0].x) * (rest_w[i2].y - rest_w[i0].y)
					- (rest_w[i1].y - rest_w[i0].y) * (rest_w[i2].x - rest_w[i0].x);
				let da = (def_w[i1].x - def_w[i0].x) * (def_w[i2].y - def_w[i0].y)
					- (def_w[i1].y - def_w[i0].y) * (def_w[i2].x - def_w[i0].x);

				let mut max_stretch = 1.0f32;
				for (a, b) in [(i0, i1), (i1, i2), (i2, i0)] {
					let rl = (rest_w[b] - rest_w[a]).length();
					if rl > 1.0 {
						max_stretch = max_stretch.max((def_w[b] - def_w[a]).length() / rl);
					}
				}

				let highlight = if ra.abs() > 1e-4 && da * ra <= 0.0 {
					Some([255, 45, 55, 240]) // Flipped fold-over in RED
				} else if max_stretch > 2.5 {
					Some([255, 165, 30, 220]) // Stretched in ORANGE
				} else {
					None
				};

				if let Some(col) = highlight {
					let s0 = world_to_screen(def_w[i0]);
					let s1 = world_to_screen(def_w[i1]);
					let s2 = world_to_screen(def_w[i2]);
					draw_canvas_line(&mut img, s0, s1, col);
					draw_canvas_line(&mut img, s1, s2, col);
					draw_canvas_line(&mut img, s2, s0, col);
				}
			}

			if FeatureRole::from_name(&name).is_facial_feature() {
				if let Some((fc, fh)) = face_ellipse {
					let rx = fh.x * 0.96;
					let ry = fh.y * 0.98;
					for &w in &def_w {
						let norm = ((w.x - fc.x) / rx).powi(2) + ((w.y - fc.y) / ry).powi(2);
						if norm > 1.0 {
							draw_crosshair(&mut img, world_to_screen(w), 4.0, [255, 70, 70, 245]);
						}
					}
				}
			}
		}
	}

	Ok(img)
}

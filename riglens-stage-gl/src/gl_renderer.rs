use std::mem;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use egui_glow::glow::{self, HasContext};
use inox2d::node::components::{BlendMode, MaskMode};

use crate::recorder::{StageDrawOp, StageFrame, StageGeometry};
use crate::shaders::{is_gles, StagePrograms};

/// Shared handle held by `Inox2dStudioApp` for lazy GL resource initialization and cleanup.
pub type SharedGlStage = Arc<Mutex<Option<StageGlRenderer>>>;

/// Straight-alpha RGBA8 texture payload passed for on-demand GL upload.
#[derive(Clone)]
pub struct StageTextureInput {
	pub width: u32,
	pub height: u32,
	pub pixels: Vec<u8>,
}

struct OffscreenTarget {
	fbo: glow::Framebuffer,
	color_tex: glow::Texture,
	stencil_rbo: glow::Renderbuffer,
}

impl OffscreenTarget {
	unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
		Ok(Self {
			fbo: gl.create_framebuffer()?,
			color_tex: gl.create_texture()?,
			stencil_rbo: gl.create_renderbuffer()?,
		})
	}

	unsafe fn resize(&self, gl: &glow::Context, w: i32, h: i32) {
		gl.bind_texture(glow::TEXTURE_2D, Some(self.color_tex));
		gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
		gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
		gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
		gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
		gl.tex_image_2d(
			glow::TEXTURE_2D,
			0,
			glow::RGBA8 as i32,
			w,
			h,
			0,
			glow::RGBA,
			glow::UNSIGNED_BYTE,
			None,
		);

		gl.bind_renderbuffer(glow::RENDERBUFFER, Some(self.stencil_rbo));
		gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH24_STENCIL8, w, h);

		gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
		gl.framebuffer_texture_2d(
			glow::FRAMEBUFFER,
			glow::COLOR_ATTACHMENT0,
			glow::TEXTURE_2D,
			Some(self.color_tex),
			0,
		);
		gl.framebuffer_renderbuffer(
			glow::FRAMEBUFFER,
			glow::DEPTH_STENCIL_ATTACHMENT,
			glow::RENDERBUFFER,
			Some(self.stencil_rbo),
		);
		gl.bind_renderbuffer(glow::RENDERBUFFER, None);
		gl.bind_texture(glow::TEXTURE_2D, None);
	}

	unsafe fn destroy(&self, gl: &glow::Context) {
		gl.delete_framebuffer(self.fbo);
		gl.delete_texture(self.color_tex);
		gl.delete_renderbuffer(self.stencil_rbo);
	}
}

/// Native OpenGL stage renderer with offscreen FBO compositing and 8-bit stencil masking.
pub struct StageGlRenderer {
	programs: StagePrograms,
	is_es: bool,
	max_tex_size: i32,

	mesh_vao: glow::VertexArray,
	vbo_verts: glow::Buffer,
	vbo_uvs: glow::Buffer,
	vbo_deforms: glow::Buffer,
	ebo_indices: glow::Buffer,

	quad_vao: glow::VertexArray,
	quad_vbo: glow::Buffer,

	puppet_target: OffscreenTarget,
	composite_target: OffscreenTarget,
	backdrop_tex: glow::Texture,
	target_size: (i32, i32),

	textures: Vec<glow::Texture>,
	geo_key: u64,
	tex_key: u64,
}

impl StageGlRenderer {
	fn init(gl: &glow::Context) -> Result<Self, String> {
		unsafe {
			let programs = StagePrograms::new(gl)?;
			let is_es = is_gles(gl);
			let max_tex_size = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE).clamp(512, 4096);

			let mesh_vao = gl.create_vertex_array()?;
			let vbo_verts = gl.create_buffer()?;
			let vbo_uvs = gl.create_buffer()?;
			let vbo_deforms = gl.create_buffer()?;
			let ebo_indices = gl.create_buffer()?;

			gl.bind_vertex_array(Some(mesh_vao));
			gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo_verts));
			gl.enable_vertex_attrib_array(0);
			gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);

			gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo_uvs));
			gl.enable_vertex_attrib_array(1);
			gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 0, 0);

			gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo_deforms));
			gl.enable_vertex_attrib_array(2);
			gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, 0, 0);

			gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo_indices));
			gl.bind_vertex_array(None);

			let quad_vao = gl.create_vertex_array()?;
			let quad_vbo = gl.create_buffer()?;
			#[rustfmt::skip]
			let quad_data: [f32; 24] = [
				-1.0, -1.0, 0.0, 0.0,
				 1.0, -1.0, 1.0, 0.0,
				 1.0,  1.0, 1.0, 1.0,
				-1.0, -1.0, 0.0, 0.0,
				 1.0,  1.0, 1.0, 1.0,
				-1.0,  1.0, 0.0, 1.0,
			];
			gl.bind_vertex_array(Some(quad_vao));
			gl.bind_buffer(glow::ARRAY_BUFFER, Some(quad_vbo));
			let quad_bytes = std::slice::from_raw_parts(quad_data.as_ptr() as *const u8, mem::size_of_val(&quad_data));
			gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, quad_bytes, glow::STATIC_DRAW);
			gl.enable_vertex_attrib_array(0);
			gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 16, 0);
			gl.enable_vertex_attrib_array(1);
			gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 16, 8);
			gl.bind_vertex_array(None);

			let puppet_target = OffscreenTarget::new(gl)?;
			let composite_target = OffscreenTarget::new(gl)?;
			let backdrop_tex = gl.create_texture()?;

			Ok(Self {
				programs,
				is_es,
				max_tex_size,
				mesh_vao,
				vbo_verts,
				vbo_uvs,
				vbo_deforms,
				ebo_indices,
				quad_vao,
				quad_vbo,
				puppet_target,
				composite_target,
				backdrop_tex,
				target_size: (0, 0),
				textures: Vec::new(),
				geo_key: u64::MAX,
				tex_key: u64::MAX,
			})
		}
	}

	/// Release all OpenGL buffers, textures, framebuffers, and shaders.
	pub fn destroy(self, gl: &glow::Context) {
		unsafe {
			self.programs.destroy(gl);
			gl.delete_vertex_array(self.mesh_vao);
			gl.delete_buffer(self.vbo_verts);
			gl.delete_buffer(self.vbo_uvs);
			gl.delete_buffer(self.vbo_deforms);
			gl.delete_buffer(self.ebo_indices);
			gl.delete_vertex_array(self.quad_vao);
			gl.delete_buffer(self.quad_vbo);
			self.puppet_target.destroy(gl);
			self.composite_target.destroy(gl);
			gl.delete_texture(self.backdrop_tex);
			for t in self.textures {
				gl.delete_texture(t);
			}
		}
	}

	unsafe fn ensure_targets(&mut self, gl: &glow::Context, w: i32, h: i32) {
		let w = w.clamp(1, self.max_tex_size);
		let h = h.clamp(1, self.max_tex_size);
		if self.target_size == (w, h) {
			return;
		}
		self.puppet_target.resize(gl, w, h);
		self.composite_target.resize(gl, w, h);

		gl.bind_texture(glow::TEXTURE_2D, Some(self.backdrop_tex));
		gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
		gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
		gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
		gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
		gl.tex_image_2d(
			glow::TEXTURE_2D,
			0,
			glow::RGBA8 as i32,
			w,
			h,
			0,
			glow::RGBA,
			glow::UNSIGNED_BYTE,
			None,
		);
		gl.bind_texture(glow::TEXTURE_2D, None);
		self.target_size = (w, h);
	}

	unsafe fn sync_textures(&mut self, gl: &glow::Context, tex_key: u64, inputs: &[StageTextureInput]) {
		if self.tex_key == tex_key && self.textures.len() == inputs.len() {
			return;
		}
		for t in self.textures.drain(..) {
			gl.delete_texture(t);
		}
		for inp in inputs {
			if let Ok(tex) = gl.create_texture() {
				gl.bind_texture(glow::TEXTURE_2D, Some(tex));
				gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
				gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
				gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
				gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
				gl.tex_image_2d(
					glow::TEXTURE_2D,
					0,
					glow::RGBA8 as i32,
					inp.width.max(1) as i32,
					inp.height.max(1) as i32,
					0,
					glow::RGBA,
					glow::UNSIGNED_BYTE,
					Some(&inp.pixels),
				);
				gl.bind_texture(glow::TEXTURE_2D, None);
				self.textures.push(tex);
			}
		}
		self.tex_key = tex_key;
	}

	unsafe fn sync_geometry(&mut self, gl: &glow::Context, geo_key: u64, geo: &StageGeometry) {
		if self.geo_key == geo_key {
			return;
		}
		gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo_verts));
		let vbytes = std::slice::from_raw_parts(
			geo.verts.as_ptr() as *const u8,
			geo.verts.len() * mem::size_of::<glam::Vec2>(),
		);
		gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vbytes, glow::STATIC_DRAW);

		gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo_uvs));
		let ubytes = std::slice::from_raw_parts(
			geo.uvs.as_ptr() as *const u8,
			geo.uvs.len() * mem::size_of::<glam::Vec2>(),
		);
		gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, ubytes, glow::STATIC_DRAW);

		gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.ebo_indices));
		let ibytes = std::slice::from_raw_parts(
			geo.indices.as_ptr() as *const u8,
			geo.indices.len() * mem::size_of::<u32>(),
		);
		gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, ibytes, glow::STATIC_DRAW);
		self.geo_key = geo_key;
	}

	unsafe fn prepare_blend(&self, gl: &glow::Context, mode: BlendMode) -> i32 {
		match mode {
			BlendMode::Normal => {
				gl.enable(glow::BLEND);
				gl.blend_equation(glow::FUNC_ADD);
				gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
				0
			}
			BlendMode::ClipToLower => {
				gl.enable(glow::BLEND);
				gl.blend_equation(glow::FUNC_ADD);
				gl.blend_func_separate(
					glow::DST_ALPHA,
					glow::ONE_MINUS_SRC_ALPHA,
					glow::ZERO,
					glow::ONE,
				);
				0
			}
			BlendMode::SliceFromLower => {
				gl.enable(glow::BLEND);
				gl.blend_equation(glow::FUNC_ADD);
				gl.blend_func(glow::ONE_MINUS_DST_ALPHA, glow::ONE);
				0
			}
			other => {
				let id = match other {
					BlendMode::Multiply => 1,
					BlendMode::Screen => 2,
					BlendMode::Overlay => 3,
					BlendMode::HardLight => 4,
					BlendMode::SoftLight => 5,
					BlendMode::Darken => 6,
					BlendMode::Lighten => 7,
					BlendMode::ColorDodge => 8,
					BlendMode::LinearDodge | BlendMode::AddGlow => 9,
					BlendMode::ColorBurn => 10,
					BlendMode::Difference => 11,
					BlendMode::Exclusion => 12,
					BlendMode::Subtract => 13,
					BlendMode::Inverse => 14,
					BlendMode::DestinationIn => 15,
					BlendMode::Normal | BlendMode::ClipToLower | BlendMode::SliceFromLower => 0,
				};
				gl.disable(glow::BLEND);
				let (w, h) = self.target_size;
				gl.active_texture(glow::TEXTURE1);
				gl.bind_texture(glow::TEXTURE_2D, Some(self.backdrop_tex));
				gl.copy_tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, 0, 0, w, h);
				id
			}
		}
	}

	unsafe fn render_frame(
		&mut self,
		gl: &glow::Context,
		viewport_px: [i32; 4],
		frame: &StageFrame,
		geo_key: u64,
		geo: &StageGeometry,
		tex_key: u64,
		textures: &[StageTextureInput],
	) {
		let prev_draw_fbo = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32;
		let prev_read_fbo = gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING) as u32;
		let prev_draw_opt = NonZeroU32::new(prev_draw_fbo).map(glow::NativeFramebuffer);
		let prev_read_opt = NonZeroU32::new(prev_read_fbo).map(glow::NativeFramebuffer);

		self.ensure_targets(gl, viewport_px[2], viewport_px[3]);
		self.sync_textures(gl, tex_key, textures);
		self.sync_geometry(gl, geo_key, geo);

		gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo_deforms));
		let dbytes = std::slice::from_raw_parts(
			frame.deforms.as_ptr() as *const u8,
			frame.deforms.len() * mem::size_of::<glam::Vec2>(),
		);
		gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, dbytes, glow::DYNAMIC_DRAW);

		let (fbo_w, fbo_h) = self.target_size;
		gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.puppet_target.fbo));
		gl.viewport(0, 0, fbo_w, fbo_h);
		gl.disable(glow::SCISSOR_TEST);
		gl.disable(glow::DEPTH_TEST);
		gl.disable(glow::CULL_FACE);
		gl.color_mask(true, true, true, true);
		gl.stencil_mask(0xff);
		gl.disable(glow::STENCIL_TEST);
		gl.clear_color(0.0, 0.0, 0.0, 0.0);
		gl.clear_stencil(0);
		gl.clear(glow::COLOR_BUFFER_BIT | glow::STENCIL_BUFFER_BIT);

		let mut in_composite = false;
		let mut puppet_mask_active = false;
		let mut comp_mask_active = false;
		let mut puppet_mask_ref = 1i32;
		let mut comp_mask_ref = 1i32;
		let mut active_threshold = 0.5f32;

		for op in &frame.ops {
			match op {
				StageDrawOp::BeginMasks {
					threshold,
					has_normal_masks,
				} => {
					active_threshold = *threshold;
					if in_composite {
						comp_mask_active = true;
					} else {
						puppet_mask_active = true;
					}
					gl.enable(glow::STENCIL_TEST);
					gl.clear_stencil(if *has_normal_masks { 0 } else { 1 });
					gl.clear(glow::STENCIL_BUFFER_BIT);
					gl.color_mask(false, false, false, false);
					gl.stencil_op(glow::KEEP, glow::KEEP, glow::REPLACE);
					gl.stencil_mask(0xff);
				}
				StageDrawOp::BeginMask { mode } => {
					let r = if *mode == MaskMode::Mask { 1 } else { 0 };
					if in_composite {
						comp_mask_ref = r;
					} else {
						puppet_mask_ref = r;
					}
					gl.stencil_func(glow::ALWAYS, r, 0xff);
				}
				StageDrawOp::BeginMaskedContent => {
					gl.stencil_func(glow::EQUAL, 1, 0xff);
					gl.stencil_mask(0x00);
					gl.color_mask(true, true, true, true);
				}
				StageDrawOp::EndMask => {
					if in_composite {
						comp_mask_active = false;
					} else {
						puppet_mask_active = false;
					}
					gl.stencil_mask(0xff);
					gl.stencil_func(glow::ALWAYS, 1, 0xff);
					gl.disable(glow::STENCIL_TEST);
					gl.color_mask(true, true, true, true);
				}
				StageDrawOp::BeginComposite => {
					in_composite = true;
					comp_mask_active = false;
					gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.composite_target.fbo));
					gl.disable(glow::STENCIL_TEST);
					gl.color_mask(true, true, true, true);
					gl.stencil_mask(0xff);
					gl.clear_color(0.0, 0.0, 0.0, 0.0);
					gl.clear_stencil(0);
					gl.clear(glow::COLOR_BUFFER_BIT | glow::STENCIL_BUFFER_BIT);
				}
				StageDrawOp::FinishComposite {
					as_mask,
					opacity,
					tint,
					screen_tint,
					blend_mode,
				} => {
					in_composite = false;
					comp_mask_active = false;
					gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.puppet_target.fbo));
					gl.bind_vertex_array(self.quad_vao.into());

					if *as_mask {
						gl.enable(glow::STENCIL_TEST);
						gl.color_mask(false, false, false, false);
						gl.stencil_op(glow::KEEP, glow::KEEP, glow::REPLACE);
						gl.stencil_mask(0xff);
						gl.stencil_func(glow::ALWAYS, puppet_mask_ref, 0xff);
						gl.disable(glow::BLEND);

						gl.use_program(Some(self.programs.comp_mask.program));
						gl.active_texture(glow::TEXTURE0);
						gl.bind_texture(glow::TEXTURE_2D, Some(self.composite_target.color_tex));
						gl.uniform_1_i32(self.programs.comp_mask.u_fbo.as_ref(), 0);
						gl.uniform_1_f32(self.programs.comp_mask.u_threshold.as_ref(), active_threshold);
						gl.uniform_1_f32(self.programs.comp_mask.u_opacity.as_ref(), *opacity);
						gl.draw_arrays(glow::TRIANGLES, 0, 6);
					} else {
						if puppet_mask_active {
							gl.enable(glow::STENCIL_TEST);
							gl.stencil_func(glow::EQUAL, 1, 0xff);
							gl.stencil_mask(0x00);
						} else {
							gl.disable(glow::STENCIL_TEST);
						}
						gl.color_mask(true, true, true, true);

						let bm_id = self.prepare_blend(gl, *blend_mode);
						gl.use_program(Some(self.programs.comp.program));
						gl.active_texture(glow::TEXTURE0);
						gl.bind_texture(glow::TEXTURE_2D, Some(self.composite_target.color_tex));
						gl.uniform_1_i32(self.programs.comp.u_fbo.as_ref(), 0);
						gl.uniform_1_i32(self.programs.comp.u_backdrop.as_ref(), 1);
						gl.uniform_1_f32(self.programs.comp.u_opacity.as_ref(), *opacity);
						gl.uniform_3_f32_slice(self.programs.comp.u_mult_color.as_ref(), tint.as_ref());
						gl.uniform_3_f32_slice(self.programs.comp.u_screen_color.as_ref(), screen_tint.as_ref());
						gl.uniform_1_i32(self.programs.comp.u_blend_mode.as_ref(), bm_id);
						gl.draw_arrays(glow::TRIANGLES, 0, 6);
					}
				}
				StageDrawOp::DrawPart(part) => {
					let Some(&tex) = self.textures.get(part.tex_albedo) else {
						continue;
					};
					gl.bind_vertex_array(Some(self.mesh_vao));

					if part.as_mask {
						let cur_ref = if in_composite { comp_mask_ref } else { puppet_mask_ref };
						gl.enable(glow::STENCIL_TEST);
						gl.color_mask(false, false, false, false);
						gl.stencil_op(glow::KEEP, glow::KEEP, glow::REPLACE);
						gl.stencil_mask(0xff);
						gl.stencil_func(glow::ALWAYS, cur_ref, 0xff);
						gl.disable(glow::BLEND);

						gl.use_program(Some(self.programs.part_mask.program));
						gl.active_texture(glow::TEXTURE0);
						gl.bind_texture(glow::TEXTURE_2D, Some(tex));
						gl.uniform_1_i32(self.programs.part_mask.u_tex.as_ref(), 0);
						gl.uniform_matrix_4_f32_slice(
							self.programs.part_mask.u_mvp.as_ref(),
							false,
							part.mvp.as_ref(),
						);
						gl.uniform_2_f32_slice(self.programs.part_mask.u_offset.as_ref(), part.offset.as_ref());
						gl.uniform_1_f32(self.programs.part_mask.u_threshold.as_ref(), active_threshold);
					} else {
						let mask_active = if in_composite {
							comp_mask_active
						} else {
							puppet_mask_active
						};
						if mask_active {
							gl.enable(glow::STENCIL_TEST);
							gl.stencil_func(glow::EQUAL, 1, 0xff);
							gl.stencil_mask(0x00);
						} else {
							gl.disable(glow::STENCIL_TEST);
						}
						gl.color_mask(true, true, true, true);

						let bm_id = self.prepare_blend(gl, part.blend_mode);
						gl.use_program(Some(self.programs.part.program));
						gl.active_texture(glow::TEXTURE0);
						gl.bind_texture(glow::TEXTURE_2D, Some(tex));
						gl.uniform_1_i32(self.programs.part.u_albedo.as_ref(), 0);
						gl.uniform_1_i32(self.programs.part.u_backdrop.as_ref(), 1);
						gl.uniform_matrix_4_f32_slice(
							self.programs.part.u_mvp.as_ref(),
							false,
							part.mvp.as_ref(),
						);
						gl.uniform_2_f32_slice(self.programs.part.u_offset.as_ref(), part.offset.as_ref());
						gl.uniform_1_f32(self.programs.part.u_opacity.as_ref(), part.opacity);
						gl.uniform_3_f32_slice(self.programs.part.u_mult_color.as_ref(), part.tint.as_ref());
						gl.uniform_3_f32_slice(self.programs.part.u_screen_color.as_ref(), part.screen_tint.as_ref());
						gl.uniform_1_i32(self.programs.part.u_blend_mode.as_ref(), bm_id);
					}

					gl.draw_elements(
						glow::TRIANGLES,
						part.index_len as i32,
						glow::UNSIGNED_INT,
						part.index_offset as i32 * mem::size_of::<u32>() as i32,
					);
				}
			}
		}

		// Optional desktop wireframe pass over puppet_fbo
		if frame.wireframe && !self.is_es {
			gl.disable(glow::STENCIL_TEST);
			gl.color_mask(true, true, true, true);
			gl.enable(glow::BLEND);
			gl.blend_equation(glow::FUNC_ADD);
			gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
			gl.polygon_mode(glow::FRONT_AND_BACK, glow::LINE);
			gl.bind_vertex_array(Some(self.mesh_vao));
			gl.use_program(Some(self.programs.part.program));
			gl.uniform_1_i32(self.programs.part.u_blend_mode.as_ref(), 16);
			for op in &frame.ops {
				if let StageDrawOp::DrawPart(part) = op {
					if part.as_mask {
						continue;
					}
					gl.uniform_matrix_4_f32_slice(self.programs.part.u_mvp.as_ref(), false, part.mvp.as_ref());
					gl.uniform_2_f32_slice(self.programs.part.u_offset.as_ref(), part.offset.as_ref());
					gl.draw_elements(
						glow::TRIANGLES,
						part.index_len as i32,
						glow::UNSIGNED_INT,
						part.index_offset as i32 * mem::size_of::<u32>() as i32,
					);
				}
			}
			gl.polygon_mode(glow::FRONT_AND_BACK, glow::FILL);
		}

		// Restore egui's FBO & scissor, then composite puppet_fbo onto egui canvas
		gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, prev_draw_opt);
		gl.bind_framebuffer(glow::READ_FRAMEBUFFER, prev_read_opt);
		gl.enable(glow::SCISSOR_TEST);
		gl.disable(glow::STENCIL_TEST);
		gl.stencil_mask(0xff);
		gl.color_mask(true, true, true, true);
		gl.viewport(viewport_px[0], viewport_px[1], viewport_px[2], viewport_px[3]);
		gl.enable(glow::BLEND);
		gl.blend_equation(glow::FUNC_ADD);
		gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);

		gl.bind_vertex_array(Some(self.quad_vao));
		gl.use_program(Some(self.programs.blit.program));
		gl.active_texture(glow::TEXTURE0);
		gl.bind_texture(glow::TEXTURE_2D, Some(self.puppet_target.color_tex));
		gl.uniform_1_i32(self.programs.blit.u_fbo.as_ref(), 0);
		gl.draw_arrays(glow::TRIANGLES, 0, 6);

		gl.bind_vertex_array(None);
		gl.use_program(None);
	}
}

/// Build an `egui::PaintCallback` that renders `frame` via native OpenGL FBOs and stencil masks.
pub fn make_paint_callback(
	rect: egui::Rect,
	shared: SharedGlStage,
	last_gl_ms: Arc<Mutex<f32>>,
	frame: StageFrame,
	geo_key: u64,
	geo: Arc<StageGeometry>,
	tex_key: u64,
	textures: Arc<Vec<StageTextureInput>>,
) -> egui::PaintCallback {
	let cb = egui_glow::CallbackFn::new(move |info, painter| {
		let vp = info.viewport_in_pixels();
		if vp.width_px <= 0 || vp.height_px <= 0 {
			return;
		}
		let start = Instant::now();
		let gl = painter.gl();
		if let Ok(mut guard) = shared.lock() {
			if guard.is_none() {
				match StageGlRenderer::init(gl) {
					Ok(r) => *guard = Some(r),
					Err(_) => return,
				}
			}
			if let Some(ref mut renderer) = *guard {
				unsafe {
					renderer.render_frame(
						gl,
						[vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px],
						&frame,
						geo_key,
						&geo,
						tex_key,
						&textures,
					);
				}
			}
		}
		if let Ok(mut ms) = last_gl_ms.lock() {
			*ms = start.elapsed().as_secs_f32() * 1000.0;
		}
	});
	egui::PaintCallback {
		rect,
		callback: Arc::new(cb),
	}
}

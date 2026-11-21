use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, pos2, Rect};
use glam::{vec2 as gvec2, vec3 as gvec3, Mat4, Vec2 as GVec2, Vec4};
use inox2d::math::camera::Camera;
use inox2d::math::curve::evaluate_point;
use inox2d::node::components::{GridDeformer, Mesh, PathDeformer, TransformStore};
use inox2d::node::InoxNodeUuid;
use inox2d::render::TexturedMeshRenderCtx;
use inox2d::texture::decode_model_textures;
use inox2d_mcp::headless_render::{render_diagnostic, render_frame_with_textures, RenderOptions};
use riglens_booster::gpu::ambient::AmbientTheme;
use riglens_booster::gpu::{GpuEffectsConfig, GpuFrameProcessor};
use riglens_booster::physics::MotionHint;
use riglens_booster::transport::{FrameBuffer, PixelFormat};
use riglens_stage_gl::{extract_stage_geometry, make_paint_callback, record_stage_frame, StageTextureInput};

use crate::icons::StudioTool;
use crate::stage::StageDrag;
use crate::Inox2dStudioApp;

pub(crate) enum StageOverlay {
	Mesh {
		pts: Vec<egui::Pos2>,
		indices: Vec<u32>,
		bbox: Rect,
		pivot: egui::Pos2,
	},
	Grid {
		pts: Vec<egui::Pos2>,
		cols: usize,
		rows: usize,
		bbox: Rect,
		pivot: egui::Pos2,
	},
	Path {
		pts: Vec<egui::Pos2>,
		curve_pts: Vec<egui::Pos2>,
		bbox: Rect,
		pivot: egui::Pos2,
	},
}

impl StageOverlay {
	pub(crate) fn pts(&self) -> &[egui::Pos2] {
		match self {
			Self::Mesh { pts, .. } | Self::Grid { pts, .. } | Self::Path { pts, .. } => pts,
		}
	}

	pub(crate) fn bbox(&self) -> Rect {
		match self {
			Self::Mesh { bbox, .. } | Self::Grid { bbox, .. } | Self::Path { bbox, .. } => *bbox,
		}
	}

	pub(crate) fn pivot(&self) -> egui::Pos2 {
		match self {
			Self::Mesh { pivot, .. } | Self::Grid { pivot, .. } | Self::Path { pivot, .. } => *pivot,
		}
	}
}

impl Inox2dStudioApp {
	pub(crate) fn filtered_params(&self) -> HashMap<String, GVec2> {
		self.session
			.param_overrides
			.iter()
			.filter(|(k, _)| self.session.model.puppet.params.contains_key(k.as_str()))
			.map(|(k, v)| (k.clone(), *v))
			.collect()
	}

	pub(crate) fn render_signature(&self) -> u64 {
		use std::collections::hash_map::DefaultHasher;
		use std::hash::{Hash, Hasher};
		let mut h = DefaultHasher::new();
		self.session.revision.hash(&mut h);
		self.stage_res.hash(&mut h);
		self.stage_viewport_size.x.to_bits().hash(&mut h);
		self.stage_viewport_size.y.to_bits().hash(&mut h);
		self.pixels_per_point.to_bits().hash(&mut h);
		self.puppet_scale.to_bits().hash(&mut h);
		self.puppet_center.x.to_bits().hash(&mut h);
		self.puppet_center.y.to_bits().hash(&mut h);
		self.wireframe.hash(&mut h);
		self.xray_mode.hash(&mut h);
		self.exact_composite.hash(&mut h);
		self.booster_live_fx.hash(&mut h);
		if self.booster_live_fx {
			self.booster_ui.draft.enable_soft_body.hash(&mut h);
			self.booster_ui.draft.rim_intensity.to_bits().hash(&mut h);
			self.booster_ui.draft.rim_theme.hash(&mut h);
			self.booster_ui.draft.particle_count.hash(&mut h);
		}
		if self.exact_composite || self.xray_mode || self.booster_live_fx {
			self.view_zoom.to_bits().hash(&mut h);
			self.view_pan.x.to_bits().hash(&mut h);
			self.view_pan.y.to_bits().hash(&mut h);
		}
		self.live_physics.hash(&mut h);
		self.stage_drag.is_some().hash(&mut h);
		self.playing.hash(&mut h);
		let mut keys: Vec<_> = self.session.param_overrides.keys().collect();
		keys.sort();
		for k in keys {
			k.hash(&mut h);
			let v = self.session.param_overrides[k];
			v.x.to_bits().hash(&mut h);
			v.y.to_bits().hash(&mut h);
		}
		if self.playing {
			self.playhead_frame.to_bits().hash(&mut h);
		}
		h.finish()
	}

	pub(crate) fn use_exact_stage(&self) -> bool {
		self.exact_composite && !self.xray_mode && self.stage_drag.is_none() && !self.playing
	}

	pub(crate) fn ensure_gpu_textures(&mut self, _ctx: &egui::Context) {
		if !self.gpu_tex_dirty && self.gl_textures.len() == self.session.model.textures.len() {
			return;
		}
		let decoded = decode_model_textures(self.session.model.textures.iter());
		let inputs: Vec<StageTextureInput> = decoded
			.iter()
			.map(|tex| StageTextureInput {
				width: tex.width(),
				height: tex.height(),
				pixels: tex.pixels().to_vec(),
			})
			.collect();
		self.gl_textures = Arc::new(inputs);
		self.gl_tex_gen = self.gl_tex_gen.wrapping_add(1);
		self.decoded_textures = decoded;
		self.gpu_tex_dirty = false;
	}

	pub(crate) fn prepare_stage_frame(&mut self, ctx: &egui::Context) {
		self.ensure_gpu_textures(ctx);

		let use_exact = self.use_exact_stage();
		let use_booster_fx = self.booster_live_fx && !self.xray_mode;
		let sig = self.render_signature();
		if !self.live_physics
			&& self.cached_rev == self.session.revision
			&& self.cached_sig == sig
			&& ((!self.xray_mode && !use_exact && !use_booster_fx) || self.stage_tex.is_some())
		{
			return;
		}

		let start = Instant::now();
		let mut pmap = self.filtered_params();
		let booster_running = self.session.booster.is_running();
		if booster_running {
			let tel = self.session.booster.latest_telemetry();
			let lo = &tel.last_output;
			for (pname, val) in [
				("Hair:: Sway", gvec2(lo.hair.x.clamp(-1.0, 1.0), lo.hair.y.clamp(-1.0, 1.0))),
				("Breath", gvec2((0.5 + lo.body_bounce_y * 2.5).clamp(0.0, 1.0), 0.0)),
			] {
				if self.session.model.puppet.params.contains_key(pname) && !pmap.contains_key(pname) {
					pmap.insert(pname.to_owned(), val);
				}
			}
		}
		let dt = if self.playing
			|| self.live_physics
			|| booster_running
			|| self.tool == StudioTool::Physics
			|| matches!(self.stage_drag, Some(StageDrag::Pose25D))
		{
			1.0 / 60.0
		} else {
			0.0
		};

		let vp_w = self.stage_viewport_size.x.max(64.0);
		let vp_h = self.stage_viewport_size.y.max(64.0);
		let phys_w = (vp_w * self.pixels_per_point).round() as u32;
		let phys_h = (vp_h * self.pixels_per_point).round() as u32;
		let max_side = phys_w.max(phys_h).max(1);
		let cap = 1600u32;
		let down = if max_side > cap { cap as f32 / max_side as f32 } else { 1.0 };
		let render_w = ((phys_w as f32 * down).round() as u32).clamp(64, cap);
		let render_h = ((phys_h as f32 * down).round() as u32).clamp(64, cap);
		let eff = (self.puppet_scale * self.view_zoom).max(0.05);
		let render_scale = eff * (render_w as f32 / vp_w);
		let render_cam_pos = -self.puppet_center + gvec2(self.view_pan.x / eff, self.view_pan.y / eff);

		if self.xray_mode {
			if let Ok(img) = render_diagnostic(&mut self.session.model, &pmap, render_w, render_h, eff * (render_w as f32 / vp_w)) {
				let (w, h) = img.dimensions();
				let cimg = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
				if let Some(tex) = self.stage_tex.as_mut() {
					tex.set(cimg, egui::TextureOptions::LINEAR);
				} else {
					self.stage_tex = Some(ctx.load_texture("inox2d_stage", cimg, egui::TextureOptions::LINEAR));
				}
			}
		} else if use_booster_fx {
			let mut fx_ok = true;
			if self.booster_gpu.is_none() {
				let mut cfg = GpuEffectsConfig::default();
				cfg.enable_soft_body = self.booster_ui.draft.enable_soft_body;
				cfg.rim_intensity = self.booster_ui.draft.rim_intensity;
				cfg.enable_rim_light = cfg.rim_intensity > 0.01;
				cfg.ambient_theme = AmbientTheme::parse(&self.booster_ui.draft.rim_theme).unwrap_or(AmbientTheme::Cyberpunk);
				cfg.particle_count = self.booster_ui.draft.particle_count;
				match GpuFrameProcessor::new_blocking(cfg) {
					Ok(proc) => self.booster_gpu = Some(proc),
					Err(e) => {
						self.booster_live_fx = false;
						self.set_status(false, format!("Booster GPU unavailable: {e}"));
						fx_ok = false;
					}
				}
			}
			if fx_ok {
				let fx_w = render_w.min(960);
				let fx_h = render_h.min(960);
				let fx_scale = eff * (fx_w as f32 / vp_w);
				let opts = RenderOptions {
					width: fx_w,
					height: fx_h,
					scale: fx_scale,
					camera_pos: render_cam_pos,
					wireframe: self.wireframe,
					dt,
					background: [0, 0, 0, 0],
				};
				if let (Ok(img), Some(proc)) = (
					render_frame_with_textures(&mut self.session.model, &self.decoded_textures, &pmap, &opts),
					self.booster_gpu.as_mut(),
				) {
					proc.config.enable_soft_body = self.booster_ui.draft.enable_soft_body;
					proc.config.rim_intensity = self.booster_ui.draft.rim_intensity;
					proc.config.enable_rim_light = proc.config.rim_intensity > 0.01;
					proc.config.ambient_theme =
						AmbientTheme::parse(&self.booster_ui.draft.rim_theme).unwrap_or(AmbientTheme::Cyberpunk);
					proc.config.particle_count = self.booster_ui.draft.particle_count;
					proc.config.upscale_factor = 1.0;
					let (w, h) = img.dimensions();
					if let Ok(fb) = FrameBuffer::new(w, h, PixelFormat::Rgba8, img.into_raw()) {
						if let Ok(out) = proc.process_frame(&fb, &MotionHint::default(), dt.max(1.0 / 60.0)) {
							let cimg = egui::ColorImage::from_rgba_unmultiplied(
								[out.width as usize, out.height as usize],
								&out.data,
							);
							if let Some(tex) = self.stage_tex.as_mut() {
								tex.set(cimg, egui::TextureOptions::LINEAR);
							} else {
								self.stage_tex =
									Some(ctx.load_texture("inox2d_stage", cimg, egui::TextureOptions::LINEAR));
							}
						}
					}
				}
			}
		} else if use_exact {
			let opts = RenderOptions {
				width: render_w,
				height: render_h,
				scale: render_scale,
				camera_pos: render_cam_pos,
				wireframe: self.wireframe,
				dt,
				background: [0, 0, 0, 0],
			};
			if let Ok(img) = render_frame_with_textures(&mut self.session.model, &self.decoded_textures, &pmap, &opts) {
				let (w, h) = img.dimensions();
				let cimg = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
				if let Some(tex) = self.stage_tex.as_mut() {
					tex.set(cimg, egui::TextureOptions::LINEAR);
				} else {
					self.stage_tex = Some(ctx.load_texture("inox2d_stage", cimg, egui::TextureOptions::LINEAR));
				}
			}
		} else {
			let puppet = &mut self.session.model.puppet;
			if puppet.render_ctx.is_none()
				|| puppet
					.param_ctx
					.as_ref()
					.map(|c| c.values().len() != puppet.params.len())
					.unwrap_or(true)
			{
				puppet.init_all();
			}
			puppet.begin_frame();
			if let Some(pctx) = puppet.param_ctx.as_mut() {
				for (name, val) in &pmap {
					let _ = pctx.set(name, *val);
				}
			}
			puppet.end_frame(dt);
		}
		let elapsed_ms = start.elapsed().as_secs_f32() * 1000.0;
		self.eval_ms = elapsed_ms;
		if self.xray_mode || use_exact || use_booster_fx {
			self.render_ms = elapsed_ms;
		}
		self.cached_rev = self.session.revision;
		self.cached_sig = sig;
	}

	pub(crate) fn draw_puppet_gpu_meshes(&mut self, painter: &egui::Painter, canvas_rect: Rect) {
		let submit_start = Instant::now();
		let puppet = &self.session.model.puppet;
		let geo_key = (self.model_gen << 32) ^ self.session.revision;
		if self.gl_geo.is_none() || self.gl_geo_key != geo_key {
			self.gl_geo = extract_stage_geometry(puppet).map(Arc::new);
			self.gl_geo_key = geo_key;
		}
		let Some(geo) = self.gl_geo.clone() else {
			return;
		};
		let Some(frame) = record_stage_frame(puppet, self.mvp_cam(), self.wireframe) else {
			return;
		};
		painter.add(egui::Shape::Callback(make_paint_callback(
			canvas_rect,
			self.gl_stage.clone(),
			self.gl_last_ms.clone(),
			frame,
			self.gl_geo_key,
			geo,
			self.gl_tex_gen,
			self.gl_textures.clone(),
		)));

		let gl_ms = self.gl_last_ms.lock().map(|g| *g).unwrap_or(0.0);
		self.render_ms = (self.eval_ms + submit_start.elapsed().as_secs_f32() * 1000.0 + gl_ms).max(0.01);
	}

	pub(crate) fn mvp_cam(&self) -> Mat4 {
		let eff = (self.puppet_scale * self.view_zoom).max(0.05);
		let cam = Camera {
			position: -self.puppet_center + gvec2(self.view_pan.x / eff, self.view_pan.y / eff),
			rotation: 0.0,
			scale: gvec2(eff, eff),
		};
		cam.matrix(gvec2(
			self.stage_viewport_size.x.max(64.0),
			self.stage_viewport_size.y.max(64.0),
		))
	}

	pub(crate) fn world_to_ui(&self, canvas_rect: Rect, world_pt: GVec2) -> egui::Pos2 {
		let ndc = self.mvp_cam() * gvec3(world_pt.x, world_pt.y, 0.0).extend(1.0);
		let nx = ndc.x * 0.5 + 0.5;
		let ny = 0.5 - ndc.y * 0.5;
		pos2(
			canvas_rect.left() + nx * canvas_rect.width(),
			canvas_rect.top() + ny * canvas_rect.height(),
		)
	}

	pub(crate) fn selected_node_stage_overlay(&self, canvas_rect: Rect) -> Option<StageOverlay> {
		let id = self.selected_node?;
		let puppet = &self.session.model.puppet;
		let tstore = puppet.node_comps.get::<TransformStore>(id)?;
		let mvp = self.mvp_cam() * tstore.absolute;

		let to_ui = |ndc: Vec4| -> egui::Pos2 {
			let nx = ndc.x * 0.5 + 0.5;
			let ny = 0.5 - ndc.y * 0.5;
			pos2(
				canvas_rect.left() + nx * canvas_rect.width(),
				canvas_rect.top() + ny * canvas_rect.height(),
			)
		};
		let pivot = to_ui(mvp * gvec3(0.0, 0.0, 0.0).extend(1.0));

		let compute_pts_bbox = |verts: &[GVec2], origin: GVec2, deforms: &[GVec2]| -> (Vec<egui::Pos2>, Rect) {
			let mut pts = Vec::with_capacity(verts.len());
			let mut mn = pos2(f32::INFINITY, f32::INFINITY);
			let mut mx = pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
			for (i, &v) in verts.iter().enumerate() {
				let d = deforms.get(i).copied().unwrap_or(GVec2::ZERO);
				let local = gvec3(v.x - origin.x + d.x, v.y - origin.y + d.y, 0.0);
				let sp = to_ui(mvp * local.extend(1.0));
				mn.x = mn.x.min(sp.x);
				mn.y = mn.y.min(sp.y);
				mx.x = mx.x.max(sp.x);
				mx.y = mx.y.max(sp.y);
				pts.push(sp);
			}
			(pts, Rect::from_min_max(mn, mx))
		};

		if let Some(mesh) = puppet.node_comps.get::<Mesh>(id) {
			if mesh.vertices.is_empty() {
				return None;
			}
			let render_deforms: Option<&[GVec2]> = puppet
				.node_comps
				.get::<TexturedMeshRenderCtx>(id)
				.zip(puppet.render_ctx.as_ref())
				.and_then(|(rctx, prctx)| {
					let off = rctx.vert_offset as usize;
					prctx.vertex_buffers.deforms.get(off..off + rctx.vert_len)
				});
			let eval_deforms;
			let deforms: &[GVec2] = if let Some(d) = render_deforms {
				d
			} else {
				eval_deforms = puppet
					.node_deform_offsets(id)
					.unwrap_or_else(|| vec![GVec2::ZERO; mesh.vertices.len()]);
				&eval_deforms
			};
			let (pts, bbox) = compute_pts_bbox(&mesh.vertices, mesh.origin, deforms);
			return Some(StageOverlay::Mesh {
				pts,
				indices: mesh.indices.clone(),
				bbox,
				pivot,
			});
		}

		if let Some(grid) = puppet.node_comps.get::<GridDeformer>(id) {
			if !grid.has_valid_grid() {
				return None;
			}
			let rest = grid.rest_vertices();
			let deforms = puppet
				.node_deform_offsets(id)
				.unwrap_or_else(|| vec![GVec2::ZERO; rest.len()]);
			let (pts, bbox) = compute_pts_bbox(&rest, GVec2::ZERO, &deforms);
			return Some(StageOverlay::Grid {
				pts,
				cols: grid.cols(),
				rows: grid.rows(),
				bbox,
				pivot,
			});
		}

		if let Some(path) = puppet.node_comps.get::<PathDeformer>(id) {
			if path.vertices.is_empty() {
				return None;
			}
			let deforms = puppet
				.node_deform_offsets(id)
				.unwrap_or_else(|| vec![GVec2::ZERO; path.vertices.len()]);
			let deformed_local: Vec<GVec2> = path
				.vertices
				.iter()
				.enumerate()
				.map(|(i, &v)| v + deforms.get(i).copied().unwrap_or(GVec2::ZERO))
				.collect();
			let (pts, bbox) = compute_pts_bbox(&path.vertices, GVec2::ZERO, &deforms);
			let curve_pts: Vec<egui::Pos2> = if deformed_local.len() >= 2 {
				(0..=32)
					.map(|s| {
						let p = evaluate_point(path.curve_type, &deformed_local, s as f32 / 32.0);
						to_ui(mvp * gvec3(p.x, p.y, 0.0).extend(1.0))
					})
					.collect()
			} else {
				pts.clone()
			};
			return Some(StageOverlay::Path {
				pts,
				curve_pts,
				bbox,
				pivot,
			});
		}

		None
	}

	pub(crate) fn ui_to_local_on_node(&self, id: InoxNodeUuid, canvas_rect: Rect, p: egui::Pos2) -> Option<GVec2> {
		let puppet = &self.session.model.puppet;
		let tstore = puppet.node_comps.get::<TransformStore>(id)?;
		let origin = puppet.node_comps.get::<Mesh>(id).map(|m| m.origin).unwrap_or(GVec2::ZERO);
		let nx = (p.x - canvas_rect.left()) / canvas_rect.width().max(1.0);
		let ny = (p.y - canvas_rect.top()) / canvas_rect.height().max(1.0);
		let ndc = Vec4::new(nx * 2.0 - 1.0, (0.5 - ny) * 2.0, 0.0, 1.0);
		let inv = (self.mvp_cam() * tstore.absolute).inverse();
		let local = inv * ndc;
		Some(gvec2(local.x + origin.x, local.y + origin.y))
	}
}

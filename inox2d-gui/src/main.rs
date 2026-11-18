#![forbid(unsafe_code)]

mod icons;
mod theme;

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use clap::Parser;
use eframe::egui::epaint::{Mesh as EguiMesh, Vertex as EguiVertex};
use eframe::egui::{self, pos2, vec2, Align2, Color32, Frame, Key, Margin, Rect, Rounding, Sense, Stroke, Vec2};
use glam::{vec2 as gvec2, vec3 as gvec3, Mat4, Vec2 as GVec2, Vec3 as GVec3, Vec4};
use image::RgbaImage;
use inox2d::formats::inp::{parse_inp, write_inp};
use inox2d::math::camera::Camera;
use inox2d::math::interp::InterpolateMode;
use inox2d::model::Model;
use inox2d::node::components::{
	Drawable, MaskMode, Mesh, MeshGroup, SimplePhysics, TexturedMesh, TransformStore,
};
use inox2d::node::InoxNodeUuid;
use inox2d::render::{CompositeRenderCtx, TexturedMeshRenderCtx};
use inox2d::texture::decode_model_textures;
use inox2d_mcp::audit::audit_rig;
use inox2d_mcp::engine::{command_registry, ModelSnapshot, Session};
use inox2d_mcp::headless_render::render_diagnostic;
use inox2d_mcp::psd::{import_psd_to_model_with_mode, parse_psd_bytes};
use inox2d_mcp::rig::{
	apply_vertex_deform_delta, configure_pendulum_physics, retriangulate_points_preserving_deforms,
};
use json::JsonValue;

use icons::{draw_tool_button, draw_transport_play, StudioTool};
use theme::{
	configure_craft_theme, section_header, ACCENT_BLUE, AUDIT_ERR_ROSE, AUDIT_OK_GREEN, AUDIT_WARN_AMBER, BG_DOCK,
	BG_INPUT, BG_MENU, BG_STAGE, BG_WIDGET, BORDER, BTN_ACTIVE, PLAYHEAD_RED, ROW_SELECTED, TEXT_MUTED, TEXT_PRIMARY,
	TIMELINE_EMERALD, TIMELINE_VIOLET,
};

#[derive(Parser, Debug)]
#[command(name = "inox2d-gui", version, about = "Craft-style native Rust desktop studio for Inox2D")]
struct Cli {
	/// Input .inp or .psd file, or 'sample' (defaults to built-in VTuber puppet)
	#[arg(default_value = "--sample")]
	input: String,
	/// Load the built-in rigged sample VTuber puppet
	#[arg(long)]
	sample: bool,
	/// Watch/sync .inp file on disk for bidirectional hot-reload with MCP / AI agents
	#[arg(long)]
	watch: Option<PathBuf>,
	/// Render several frames, capture a PNG screenshot of the studio window, and exit
	#[arg(long)]
	screenshot: Option<PathBuf>,
}

fn compute_alpha_convex_hull_uv(pixels: &[u8], w: u32, h: u32) -> Vec<GVec2> {
	let fallback = vec![gvec2(0.0, 0.0), gvec2(1.0, 0.0), gvec2(1.0, 1.0), gvec2(0.0, 1.0)];
	if w == 0 || h == 0 || pixels.len() < (w * h * 4) as usize {
		return fallback;
	}
	let mut sum_x = 0.0f64;
	let mut sum_y = 0.0f64;
	let mut cnt = 0usize;
	for y in 0..h {
		for x in 0..w {
			if pixels[((y * w + x) * 4 + 3) as usize] > 12 {
				sum_x += x as f64 + 0.5;
				sum_y += y as f64 + 0.5;
				cnt += 1;
			}
		}
	}
	if cnt < 4 {
		return fallback;
	}
	let cx = (sum_x / cnt as f64) as f32;
	let cy = (sum_y / cnt as f64) as f32;

	const SECTORS: usize = 24;
	let mut best = [None::<(f32, GVec2)>; SECTORS];
	let inv_w = 1.0 / w as f32;
	let inv_h = 1.0 / h as f32;
	let pad = 1.5_f32;

	for y in 0..h {
		for x in 0..w {
			if pixels[((y * w + x) * 4 + 3) as usize] <= 12 {
				continue;
			}
			let px = x as f32 + 0.5 - cx;
			let py = y as f32 + 0.5 - cy;
			let dist2 = px * px + py * py;
			let mut ang = py.atan2(px);
			if ang < 0.0 {
				ang += std::f32::consts::TAU;
			}
			let bin = ((ang / std::f32::consts::TAU) * SECTORS as f32).floor() as usize % SECTORS;
			if best[bin].map_or(true, |(d, _)| dist2 > d) {
				let r = dist2.sqrt().max(1e-4);
				let ux = ((cx + px + (px / r) * pad) * inv_w).clamp(0.0, 1.0);
				let uy = ((cy + py + (py / r) * pad) * inv_h).clamp(0.0, 1.0);
				best[bin] = Some((dist2, gvec2(ux, uy)));
			}
		}
	}

	let mut pts: Vec<GVec2> = best.into_iter().flatten().map(|(_, p)| p).collect();
	if pts.len() < 3 {
		return fallback;
	}

	// Andrew's monotone chain 2D convex hull so Sutherland-Hodgman is always convex & non-self-intersecting.
	pts.sort_by(|a, b| a.x.total_cmp(&b.x).then_with(|| a.y.total_cmp(&b.y)));
	pts.dedup_by(|a, b| (*a - *b).length_squared() < 1e-6);
	if pts.len() < 3 {
		return fallback;
	}
	let cross = |o: GVec2, a: GVec2, b: GVec2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
	let mut hull: Vec<GVec2> = Vec::with_capacity(pts.len() * 2);
	for &p in &pts {
		while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
			hull.pop();
		}
		hull.push(p);
	}
	let lower_len = hull.len() + 1;
	for &p in pts.iter().rev() {
		while hull.len() >= lower_len && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
			hull.pop();
		}
		hull.push(p);
	}
	hull.pop();
	if hull.len() < 3 {
		return fallback;
	}
	hull
}

fn clip_poly_convex_uv(mut poly: Vec<(egui::Pos2, egui::Pos2)>, clip: &[egui::Pos2]) -> Vec<(egui::Pos2, egui::Pos2)> {
	if clip.len() < 3 || poly.is_empty() {
		return poly;
	}
	let mut area = 0.0_f32;
	for i in 0..clip.len() {
		let a = clip[i];
		let b = clip[(i + 1) % clip.len()];
		area += a.x * b.y - b.x * a.y;
	}
	let sign = if area >= 0.0 { 1.0_f32 } else { -1.0_f32 };
	let is_inside = |a: egui::Pos2, b: egui::Pos2, p: egui::Pos2| -> f32 {
		((b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)) * sign
	};

	for i in 0..clip.len() {
		if poly.is_empty() {
			break;
		}
		let a = clip[i];
		let b = clip[(i + 1) % clip.len()];
		let input = std::mem::take(&mut poly);
		let mut s = *input.last().unwrap();
		for e in input {
			let se = is_inside(a, b, s.0);
			let ee = is_inside(a, b, e.0);
			if ee >= 0.0 {
				if se < 0.0 {
					let denom = se - ee;
					let t = if denom.abs() > 1e-6 { (se / denom).clamp(0.0, 1.0) } else { 0.0 };
					let ip = pos2(s.0.x + (e.0.x - s.0.x) * t, s.0.y + (e.0.y - s.0.y) * t);
					let iuv = pos2(s.1.x + (e.1.x - s.1.x) * t, s.1.y + (e.1.y - s.1.y) * t);
					poly.push((ip, iuv));
				}
				poly.push(e);
			} else if se >= 0.0 {
				let denom = se - ee;
				let t = if denom.abs() > 1e-6 { (se / denom).clamp(0.0, 1.0) } else { 0.0 };
				let ip = pos2(s.0.x + (e.0.x - s.0.x) * t, s.0.y + (e.0.y - s.0.y) * t);
				let iuv = pos2(s.1.x + (e.1.x - s.1.x) * t, s.1.y + (e.1.y - s.1.y) * t);
				poly.push((ip, iuv));
			}
			s = e;
		}
	}
	poly
}

const VISEME_PRESETS: [(&str, f32, f32); 6] = [
	("Neutral", 0.0, 0.0),
	("A", 0.0, 1.0),
	("I", 1.0, 0.35),
	("U", -1.0, 0.45),
	("E", 0.75, 0.7),
	("O", -0.45, 0.9),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum BottomTab {
	Timeline,
	MotionCurves,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VertexEditMode {
	DeformKeypoint,
	BaseTopology,
}

#[derive(Clone, Debug)]
enum StageDrag {
	MoveNode,
	ScaleCorner {
		orig_scale: GVec2,
		orig_dist: f32,
	},
	VertexMove,
	VertexMarquee {
		start: egui::Pos2,
		end: egui::Pos2,
	},
	DeformBrush,
	PhysicsLength {
		phys_id: InoxNodeUuid,
		anchor_ui: egui::Pos2,
		px_per_world: f32,
	},
	Pose25D,
	HandPan,
}

#[derive(Clone, Debug)]
struct CurveDragState {
	param_name: String,
	target: u8,
	frame: i32,
}

fn load_initial_session(input: &str) -> Result<Session, String> {
	if input == "--sample" || input == "sample" {
		let mut session = Session::new_sample();
		let _ = session.exec("rig.auto_visemes", &json::object! { node_name: "Mouth" });
		let _ = session.exec("node.synthesize_overlay", &json::object! { kind: "blush" });
		let _ = session.exec("node.synthesize_overlay", &json::object! { kind: "tears" });
		session.undo_stack.clear();
		session.redo_stack.clear();
		session.revision = 0;
		return Ok(session);
	}
	let path = Path::new(input);
	let ext = path
		.extension()
		.and_then(|s| s.to_str())
		.map(str::to_ascii_lowercase)
		.unwrap_or_default();
	let bytes = fs::read(path).map_err(|e| format!("Failed to open '{}': {e}", path.display()))?;
	if ext == "psd" {
		let doc = parse_psd_bytes(&bytes)?;
		let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("PsdPuppet");
		let model = import_psd_to_model_with_mode(&doc, name, "delaunay", true, 8, 8);
		return Ok(Session::new(model));
	}
	let model = parse_inp(bytes.as_slice()).map_err(|e| format!("Failed to parse '{}': {e}", path.display()))?;
	Ok(Session::new(model))
}

fn write_inp_atomic(model: &Model, path: &Path) -> Result<(), String> {
	if let Some(parent) = path.parent() {
		if !parent.as_os_str().is_empty() {
			fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {e}"))?;
		}
	}
	let tmp_path = path.with_extension("inp.tmp");
	{
		let file = File::create(&tmp_path).map_err(|e| format!("Failed to create temp file: {e}"))?;
		let mut w = BufWriter::new(file);
		write_inp(model, &mut w).map_err(|e| format!("Failed to serialize .inp: {e}"))?;
		w.flush().map_err(|e| format!("Failed to flush .inp: {e}"))?;
	}
	fs::rename(&tmp_path, path).map_err(|e| format!("Failed to update '{}': {e}", path.display()))?;
	Ok(())
}

fn save_color_image_png(img: &egui::ColorImage, out: &Path) -> Result<(), String> {
	if let Some(parent) = out.parent() {
		if !parent.as_os_str().is_empty() {
			fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {e}"))?;
		}
	}
	let w = img.size[0] as u32;
	let h = img.size[1] as u32;
	let mut raw = Vec::with_capacity((w * h * 4) as usize);
	for p in &img.pixels {
		raw.extend_from_slice(&p.to_array());
	}
	let rgba = RgbaImage::from_raw(w, h, raw).ok_or_else(|| "Failed to convert screenshot image".to_owned())?;
	rgba.save(out)
		.map_err(|e| format!("Failed to write screenshot '{}': {e}", out.display()))
}

struct Inox2dStudioApp {
	session: Session,
	source_path: Option<PathBuf>,
	watch_path: Option<PathBuf>,
	last_watch_mtime: Option<SystemTime>,
	last_watch_check: Instant,
	last_synced_rev: u64,

	selected_node: Option<InoxNodeUuid>,
	selected_vertices: HashSet<u32>,
	selected_physics: Option<InoxNodeUuid>,
	tool: StudioTool,
	vertex_mode: VertexEditMode,

	stage_res: u32,
	puppet_scale: f32,
	view_zoom: f32,
	view_pan: Vec2,
	wireframe: bool,
	xray_mode: bool,
	show_sel_overlay: bool,

	brush_param: String,
	brush_radius_ratio: f32,

	stage_drag: Option<StageDrag>,
	gesture_snapshot: Option<ModelSnapshot>,

	bottom_tab: BottomTab,
	playing: bool,
	loop_anim: bool,
	playhead_frame: f32,
	last_tick: Instant,
	selected_curve_lane: Option<(String, u8)>,
	selected_keyframe: Option<(String, u8, i32)>,
	curve_drag: Option<CurveDragState>,

	gpu_textures: Vec<egui::TextureHandle>,
	tex_alpha_polys: Vec<Vec<GVec2>>,
	gpu_tex_dirty: bool,
	stage_tex: Option<egui::TextureHandle>,
	cached_rev: u64,
	cached_sig: u64,
	eval_ms: f32,
	render_ms: f32,

	audit_cache: JsonValue,
	audit_rev: u64,

	status_msg: String,
	status_ok: bool,

	cmd_palette_open: bool,
	cmd_query: String,

	screenshot_out: Option<PathBuf>,
	frame_counter: u32,
	screenshot_sent: bool,
}

impl Inox2dStudioApp {
	fn new(
		session: Session,
		source_path: Option<PathBuf>,
		watch_path: Option<PathBuf>,
		screenshot_out: Option<PathBuf>,
	) -> Self {
		let selected_node = session
			.model
			.puppet
			.nodes
			.iter()
			.find(|n| {
				session.model.puppet.node_comps.get::<TexturedMesh>(n.uuid).is_some()
					&& (n.name.eq_ignore_ascii_case("face") || n.name.contains("臉"))
			})
			.or_else(|| {
				session
					.model
					.puppet
					.nodes
					.iter()
					.find(|n| session.model.puppet.node_comps.get::<TexturedMesh>(n.uuid).is_some())
			})
			.map(|n| n.uuid);

		let selected_physics = session
			.model
			.puppet
			.nodes
			.iter()
			.find(|n| session.model.puppet.node_comps.get::<SimplePhysics>(n.uuid).is_some())
			.map(|n| n.uuid);

		let last_watch_mtime = watch_path
			.as_ref()
			.and_then(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
		let rev = session.revision;

		let mut app = Self {
			session,
			source_path,
			watch_path,
			last_watch_mtime,
			last_watch_check: Instant::now(),
			last_synced_rev: rev,
			selected_node,
			selected_vertices: HashSet::new(),
			selected_physics,
			tool: StudioTool::Select,
			vertex_mode: VertexEditMode::DeformKeypoint,
			stage_res: 512,
			puppet_scale: 0.92,
			view_zoom: 1.0,
			view_pan: Vec2::ZERO,
			wireframe: false,
			xray_mode: false,
			show_sel_overlay: true,
			brush_param: "Head:: Yaw-Pitch".to_owned(),
			brush_radius_ratio: 0.45,
			stage_drag: None,
			gesture_snapshot: None,
			bottom_tab: BottomTab::Timeline,
			playing: false,
			loop_anim: true,
			playhead_frame: 1.0,
			last_tick: Instant::now(),
			selected_curve_lane: Some(("Head:: Yaw-Pitch".to_owned(), 0)),
			selected_keyframe: None,
			curve_drag: None,
			gpu_textures: Vec::new(),
			tex_alpha_polys: Vec::new(),
			gpu_tex_dirty: true,
			stage_tex: None,
			cached_rev: u64::MAX,
			cached_sig: u64::MAX,
			eval_ms: 0.0,
			render_ms: 0.0,
			audit_cache: JsonValue::new_object(),
			audit_rev: u64::MAX,
			status_msg: "Ready. V=Move/Scale · A=Vertex Edit · D=Deform Brush · S=Physics Gizmo · P=Pose 2.5D".to_owned(),
			status_ok: true,
			cmd_palette_open: false,
			cmd_query: String::new(),
			screenshot_out,
			frame_counter: 0,
			screenshot_sent: false,
		};
		app.fit_puppet_scale();
		app.refresh_audit();
		app
	}

	fn set_status(&mut self, ok: bool, msg: impl Into<String>) {
		self.status_ok = ok;
		self.status_msg = msg.into();
	}

	fn begin_gesture_undo(&mut self) {
		if self.gesture_snapshot.is_none() {
			self.gesture_snapshot = Some(self.session.snapshot());
		}
	}

	fn finish_gesture_undo(&mut self) {
		if let Some(snap) = self.gesture_snapshot.take() {
			self.session.undo_stack.push(snap);
			if self.session.undo_stack.len() > 40 {
				self.session.undo_stack.remove(0);
			}
			self.session.redo_stack.clear();
			self.session.revision += 1;
			self.sync_watch_out();
			self.cached_rev = u64::MAX;
			self.audit_rev = u64::MAX;
		}
	}

	fn fit_puppet_scale(&mut self) {
		let mut min_y = f32::INFINITY;
		let mut max_y = f32::NEG_INFINITY;
		for node in self.session.model.puppet.nodes.iter() {
			if let Some(m) = self.session.model.puppet.node_comps.get::<Mesh>(node.uuid) {
				let oy = node.trans_offset.translation.y;
				for &v in &m.vertices {
					min_y = min_y.min(v.y + oy);
					max_y = max_y.max(v.y + oy);
				}
			}
		}
		if min_y.is_finite() && max_y.is_finite() && (max_y - min_y) > 8.0 {
			let span = (max_y - min_y).max(64.0);
			self.puppet_scale = ((self.stage_res as f32 * 0.72) / span).clamp(0.12, 1.5);
		} else {
			self.puppet_scale = 0.85;
		}
		self.view_zoom = 1.0;
		self.view_pan = Vec2::ZERO;
	}

	fn exec_cmd(&mut self, cmd: &str, params: JsonValue, ok_label: &str) {
		let prev = self.session.revision;
		match self.session.exec(cmd, &params) {
			Ok(_) => {
				if self.session.revision != prev {
					if matches!(
						cmd,
						"puppet.new_sample"
							| "puppet.import_psd"
							| "node.add_part"
							| "node.synthesize_overlay"
							| "edit.undo"
							| "edit.redo"
							| "session.restore"
					) {
						self.gpu_tex_dirty = true;
					}
					self.sync_watch_out();
					self.cached_rev = u64::MAX;
					self.audit_rev = u64::MAX;
				}
				self.set_status(true, ok_label.to_owned());
			}
			Err(err) => {
				self.set_status(false, format!("{cmd} error: {err}"));
			}
		}
	}

	fn sync_watch_out(&mut self) {
		if self.session.revision == self.last_synced_rev {
			return;
		}
		if let Some(ref wpath) = self.watch_path {
			match write_inp_atomic(&self.session.model, wpath) {
				Ok(()) => {
					self.last_watch_mtime = fs::metadata(wpath).and_then(|m| m.modified()).ok();
					self.last_synced_rev = self.session.revision;
				}
				Err(err) => {
					self.set_status(false, format!("Watch write failed: {err}"));
				}
			}
		} else {
			self.last_synced_rev = self.session.revision;
		}
	}

	fn poll_watch_in(&mut self) {
		let Some(ref wpath) = self.watch_path else {
			return;
		};
		if self.last_watch_check.elapsed() < Duration::from_millis(250) {
			return;
		}
		self.last_watch_check = Instant::now();
		let Ok(mtime) = fs::metadata(wpath).and_then(|m| m.modified()) else {
			return;
		};
		if Some(mtime) == self.last_watch_mtime {
			return;
		}
		let Ok(bytes) = fs::read(wpath) else {
			return;
		};
		if let Ok(model) = parse_inp(bytes.as_slice()) {
			self.session.model = model;
			self.session.revision += 1;
			self.last_synced_rev = self.session.revision;
			self.last_watch_mtime = Some(mtime);
			self.gpu_tex_dirty = true;
			self.cached_rev = u64::MAX;
			self.audit_rev = u64::MAX;
			self.set_status(true, format!("Hot-reloaded external change from {}", wpath.display()));
		}
	}

	fn refresh_audit(&mut self) {
		if self.stage_drag.is_some() || self.curve_drag.is_some() {
			return;
		}
		if self.audit_rev != self.session.revision {
			self.audit_cache = audit_rig(&mut self.session.model);
			self.audit_rev = self.session.revision;
			self.cached_rev = u64::MAX;
		}
	}

	fn filtered_params(&self) -> HashMap<String, GVec2> {
		self.session
			.param_overrides
			.iter()
			.filter(|(k, _)| self.session.model.puppet.params.contains_key(k.as_str()))
			.map(|(k, v)| (k.clone(), *v))
			.collect()
	}

	fn render_signature(&self) -> u64 {
		use std::collections::hash_map::DefaultHasher;
		use std::hash::{Hash, Hasher};
		let mut h = DefaultHasher::new();
		self.session.revision.hash(&mut h);
		self.stage_res.hash(&mut h);
		self.puppet_scale.to_bits().hash(&mut h);
		self.wireframe.hash(&mut h);
		self.xray_mode.hash(&mut h);
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

	fn ensure_gpu_textures(&mut self, ctx: &egui::Context) {
		if !self.gpu_tex_dirty && self.gpu_textures.len() == self.session.model.textures.len() {
			return;
		}
		let decoded = decode_model_textures(self.session.model.textures.iter());
		self.gpu_textures.clear();
		self.tex_alpha_polys.clear();
		for (idx, tex) in decoded.iter().enumerate() {
			let w = tex.width();
			let h = tex.height();
			let hull = compute_alpha_convex_hull_uv(tex.pixels(), w, h);
			self.tex_alpha_polys.push(hull);
			let cimg = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], tex.pixels());
			self.gpu_textures.push(ctx.load_texture(
				format!("inox2d_part_tex_{idx}"),
				cimg,
				egui::TextureOptions::LINEAR,
			));
		}
		self.gpu_tex_dirty = false;
	}

	fn prepare_stage_frame(&mut self, ctx: &egui::Context) {
		self.ensure_gpu_textures(ctx);

		let sig = self.render_signature();
		if self.cached_rev == self.session.revision && self.cached_sig == sig && (!self.xray_mode || self.stage_tex.is_some()) {
			return;
		}

		let start = Instant::now();
		if self.xray_mode {
			let pmap = self.filtered_params();
			if let Ok(img) = render_diagnostic(&mut self.session.model, &pmap, self.stage_res, self.stage_res, self.puppet_scale) {
				let (w, h) = img.dimensions();
				let cimg = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
				if let Some(tex) = self.stage_tex.as_mut() {
					tex.set(cimg, egui::TextureOptions::LINEAR);
				} else {
					self.stage_tex = Some(ctx.load_texture("inox2d_stage", cimg, egui::TextureOptions::LINEAR));
				}
			}
		} else {
			let pmap = self.filtered_params();
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
			let dt = if self.playing || self.tool == StudioTool::Physics {
				1.0 / 60.0
			} else {
				0.0
			};
			puppet.end_frame(dt);
		}
		let elapsed_ms = start.elapsed().as_secs_f32() * 1000.0;
		self.eval_ms = elapsed_ms;
		if self.xray_mode {
			self.render_ms = elapsed_ms;
		}
		self.cached_rev = self.session.revision;
		self.cached_sig = sig;
	}

	fn node_screen_mask_poly(&self, src_id: InoxNodeUuid, canvas_rect: Rect) -> Option<Vec<egui::Pos2>> {
		let puppet = &self.session.model.puppet;
		let tm = puppet.node_comps.get::<TexturedMesh>(src_id)?;
		let mesh = puppet.node_comps.get::<Mesh>(src_id)?;
		let ts = puppet.node_comps.get::<TransformStore>(src_id)?;
		let rctx = puppet.node_comps.get::<TexturedMeshRenderCtx>(src_id)?;
		let prctx = puppet.render_ctx.as_ref()?;
		let off = rctx.vert_offset as usize;
		if off + rctx.vert_len > prctx.vertex_buffers.deforms.len() || mesh.vertices.is_empty() {
			return None;
		}
		let deforms = &prctx.vertex_buffers.deforms[off..off + rctx.vert_len];
		let uv_hull = self.tex_alpha_polys.get(tm.tex_albedo.0)?;

		let mut mn = GVec2::splat(f32::INFINITY);
		let mut mx = GVec2::splat(f32::NEG_INFINITY);
		for &v in &mesh.vertices {
			mn = mn.min(v);
			mx = mx.max(v);
		}
		let span = (mx - mn).max(gvec2(1.0, 1.0));
		let mvp = self.mvp_cam() * ts.absolute;

		let mut out = Vec::with_capacity(uv_hull.len());
		for &uv in uv_hull {
			let local_rest = gvec2(mn.x + uv.x * span.x, mn.y + uv.y * span.y);
			let deform = mesh
				.vertices
				.iter()
				.zip(deforms.iter())
				.min_by(|(va, _), (vb, _)| (**va - local_rest).length_squared().total_cmp(&(**vb - local_rest).length_squared()))
				.map(|(_, d)| *d)
				.unwrap_or(GVec2::ZERO);
			let p3 = gvec3(local_rest.x - mesh.origin.x + deform.x, local_rest.y - mesh.origin.y + deform.y, 0.0);
			let ndc = mvp * p3.extend(1.0);
			out.push(pos2(
				canvas_rect.left() + (ndc.x * 0.5 + 0.5) * canvas_rect.width(),
				canvas_rect.top() + (0.5 - ndc.y * 0.5) * canvas_rect.height(),
			));
		}
		Some(out)
	}

	fn draw_puppet_gpu_meshes(&mut self, painter: &egui::Painter, canvas_rect: Rect) {
		let submit_start = Instant::now();
		let Some(prctx) = self.session.model.puppet.render_ctx.as_ref() else {
			return;
		};
		let puppet = &self.session.model.puppet;
		let mvp_cam = self.mvp_cam();

		// Flatten draw order cleanly: root drawables zsorted descending (Composite expands its zsorted children).
		let mut ordered_parts: Vec<InoxNodeUuid> = Vec::with_capacity(prctx.root_drawables_zsorted.len() + 8);
		for &root_id in &prctx.root_drawables_zsorted {
			if let Some(comp_ctx) = puppet.node_comps.get::<CompositeRenderCtx>(root_id) {
				let comp_enabled = puppet.nodes.get_node(root_id).map(|n| n.enabled).unwrap_or(true);
				if comp_enabled {
					ordered_parts.extend(comp_ctx.zsorted_children_list.iter().copied());
				}
			} else {
				ordered_parts.push(root_id);
			}
		}

		for id in ordered_parts {
			let Some(node) = puppet.nodes.get_node(id) else {
				continue;
			};
			if !node.enabled {
				continue;
			}
			let Some(drawable) = puppet.node_comps.get::<Drawable>(id) else {
				continue;
			};
			let Some(tm) = puppet.node_comps.get::<TexturedMesh>(id) else {
				continue;
			};
			let Some(mesh) = puppet.node_comps.get::<Mesh>(id) else {
				continue;
			};
			let Some(ts) = puppet.node_comps.get::<TransformStore>(id) else {
				continue;
			};
			let Some(tm_rctx) = puppet.node_comps.get::<TexturedMeshRenderCtx>(id) else {
				continue;
			};
			let Some(gpu_tex) = self.gpu_textures.get(tm.tex_albedo.0) else {
				continue;
			};

			let opacity = puppet.node_opacity(id).unwrap_or(drawable.blending.opacity).clamp(0.0, 1.0);
			if opacity <= 0.002 {
				continue;
			}
			let tint = drawable.blending.tint.clamp(GVec3::ZERO, GVec3::ONE);
			let vcol = Color32::from_rgba_unmultiplied(
				(tint.x * 255.0) as u8,
				(tint.y * 255.0) as u8,
				(tint.z * 255.0) as u8,
				(opacity * 255.0) as u8,
			);

			let off = tm_rctx.vert_offset as usize;
			if off + tm_rctx.vert_len > prctx.vertex_buffers.deforms.len() {
				continue;
			}
			let deforms = &prctx.vertex_buffers.deforms[off..off + tm_rctx.vert_len];
			let mvp = mvp_cam * ts.absolute;

			let screen_pts: Vec<egui::Pos2> = mesh
				.vertices
				.iter()
				.zip(deforms.iter())
				.map(|(&v, &d)| {
					let local = gvec3(v.x - mesh.origin.x + d.x, v.y - mesh.origin.y + d.y, 0.0);
					let ndc = mvp * local.extend(1.0);
					pos2(
						canvas_rect.left() + (ndc.x * 0.5 + 0.5) * canvas_rect.width(),
						canvas_rect.top() + (0.5 - ndc.y * 0.5) * canvas_rect.height(),
					)
				})
				.collect();

			let clip_poly: Option<Vec<egui::Pos2>> = drawable
				.masks
				.as_ref()
				.and_then(|ms| ms.masks.iter().find(|m| m.mode == MaskMode::Mask))
				.and_then(|m| self.node_screen_mask_poly(m.source, canvas_rect));

			let mut emesh = EguiMesh::with_texture(gpu_tex.id());
			if let Some(ref cpoly) = clip_poly {
				for tri in mesh.indices.chunks_exact(3) {
					let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
					if i0 >= screen_pts.len() || i1 >= screen_pts.len() || i2 >= screen_pts.len() {
						continue;
					}
					let tri_poly = vec![
						(screen_pts[i0], pos2(mesh.uvs[i0].x, mesh.uvs[i0].y)),
						(screen_pts[i1], pos2(mesh.uvs[i1].x, mesh.uvs[i1].y)),
						(screen_pts[i2], pos2(mesh.uvs[i2].x, mesh.uvs[i2].y)),
					];
					let clipped = clip_poly_convex_uv(tri_poly, cpoly);
					if clipped.len() >= 3 {
						let base = emesh.vertices.len() as u32;
						for (sp, uv) in &clipped {
							emesh.vertices.push(EguiVertex {
								pos: *sp,
								uv: *uv,
								color: vcol,
							});
						}
						for k in 1..(clipped.len() - 1) {
							emesh.indices.push(base);
							emesh.indices.push(base + k as u32);
							emesh.indices.push(base + (k as u32) + 1);
						}
					}
				}
			} else {
				emesh.vertices.reserve(screen_pts.len());
				for (&sp, &uv) in screen_pts.iter().zip(mesh.uvs.iter()) {
					emesh.vertices.push(EguiVertex {
						pos: sp,
						uv: pos2(uv.x, uv.y),
						color: vcol,
					});
				}
				emesh.indices.extend_from_slice(&mesh.indices);
			}

			if !emesh.indices.is_empty() {
				painter.add(egui::Shape::mesh(emesh));
			}

			if self.wireframe {
				let wstroke = Stroke::new(0.8_f32, Color32::from_rgba_unmultiplied(110, 231, 183, 135));
				for tri in mesh.indices.chunks_exact(3) {
					let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
					if a < screen_pts.len() && b < screen_pts.len() && c < screen_pts.len() {
						painter.line_segment([screen_pts[a], screen_pts[b]], wstroke);
						painter.line_segment([screen_pts[b], screen_pts[c]], wstroke);
						painter.line_segment([screen_pts[c], screen_pts[a]], wstroke);
					}
				}
			}
		}

		// Record combined pose eval + GPU mesh submission ms (no accumulation across cached frames)
		self.render_ms = (self.eval_ms + submit_start.elapsed().as_secs_f32() * 1000.0).max(0.01);
	}

	fn mvp_cam(&self) -> Mat4 {
		let cam = Camera {
			position: gvec2(0.0, -35.0),
			rotation: 0.0,
			scale: gvec2(self.puppet_scale, self.puppet_scale),
		};
		cam.matrix(gvec2(self.stage_res as f32, self.stage_res as f32))
	}

	fn world_to_ui(&self, canvas_rect: Rect, world_pt: GVec2) -> egui::Pos2 {
		let ndc = self.mvp_cam() * gvec3(world_pt.x, world_pt.y, 0.0).extend(1.0);
		let nx = ndc.x * 0.5 + 0.5;
		let ny = 0.5 - ndc.y * 0.5;
		pos2(
			canvas_rect.left() + nx * canvas_rect.width(),
			canvas_rect.top() + ny * canvas_rect.height(),
		)
	}

	fn selected_node_screen_geometry(&self, canvas_rect: Rect) -> Option<(Vec<egui::Pos2>, Vec<u32>, Rect, egui::Pos2)> {
		let id = self.selected_node?;
		let puppet = &self.session.model.puppet;
		let mesh = puppet.node_comps.get::<Mesh>(id)?;
		let tstore = puppet.node_comps.get::<TransformStore>(id)?;
		let rctx = puppet.node_comps.get::<TexturedMeshRenderCtx>(id)?;
		let prctx = puppet.render_ctx.as_ref()?;
		let off = rctx.vert_offset as usize;
		if off + rctx.vert_len > prctx.vertex_buffers.deforms.len() {
			return None;
		}
		let deforms = &prctx.vertex_buffers.deforms[off..off + rctx.vert_len];
		let mvp = self.mvp_cam() * tstore.absolute;

		let to_ui = |ndc: Vec4| -> egui::Pos2 {
			let nx = ndc.x * 0.5 + 0.5;
			let ny = 0.5 - ndc.y * 0.5;
			pos2(
				canvas_rect.left() + nx * canvas_rect.width(),
				canvas_rect.top() + ny * canvas_rect.height(),
			)
		};

		let mut pts = Vec::with_capacity(mesh.vertices.len());
		let mut mn = pos2(f32::INFINITY, f32::INFINITY);
		let mut mx = pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
		for (v, d) in mesh.vertices.iter().zip(deforms.iter()) {
			let local = gvec3(v.x - mesh.origin.x + d.x, v.y - mesh.origin.y + d.y, 0.0);
			let sp = to_ui(mvp * local.extend(1.0));
			mn.x = mn.x.min(sp.x);
			mn.y = mn.y.min(sp.y);
			mx.x = mx.x.max(sp.x);
			mx.y = mx.y.max(sp.y);
			pts.push(sp);
		}
		let pivot = to_ui(mvp * gvec3(0.0, 0.0, 0.0).extend(1.0));
		Some((pts, mesh.indices.clone(), Rect::from_min_max(mn, mx), pivot))
	}

	fn ui_to_local_on_node(&self, id: InoxNodeUuid, canvas_rect: Rect, p: egui::Pos2) -> Option<GVec2> {
		let puppet = &self.session.model.puppet;
		let tstore = puppet.node_comps.get::<TransformStore>(id)?;
		let mesh = puppet.node_comps.get::<Mesh>(id)?;
		let nx = (p.x - canvas_rect.left()) / canvas_rect.width().max(1.0);
		let ny = (p.y - canvas_rect.top()) / canvas_rect.height().max(1.0);
		let ndc = Vec4::new(nx * 2.0 - 1.0, (0.5 - ny) * 2.0, 0.0, 1.0);
		let inv = (self.mvp_cam() * tstore.absolute).inverse();
		let local = inv * ndc;
		Some(gvec2(local.x + mesh.origin.x, local.y + mesh.origin.y))
	}

	fn apply_anim_at_playhead(&mut self) {
		let anim_opt = self
			.session
			.model
			.puppet
			.animations
			.get("Idle")
			.or_else(|| self.session.model.puppet.animations.values().next())
			.cloned();
		if let Some(anim) = anim_opt {
			let vals = anim.eval_at_frame(self.playhead_frame, &self.session.model.puppet.params);
			for (k, v) in vals {
				self.session.param_overrides.insert(k, v);
			}
		}
	}

	fn keypoint_cell_for_param(&self, param_name: &str) -> Option<(usize, usize)> {
		let p = self.session.model.puppet.params.get(param_name)?;
		let val = self.session.param_overrides.get(param_name).copied().unwrap_or(p.defaults);
		let span = p.max - p.min;
		let nx = if span.x.abs() > 1e-6 {
			((val.x - p.min.x) / span.x).clamp(0.0, 1.0)
		} else {
			0.0
		};
		let ny = if span.y.abs() > 1e-6 {
			((val.y - p.min.y) / span.y).clamp(0.0, 1.0)
		} else {
			0.0
		};
		let ix = p
			.axis_points
			.x
			.iter()
			.enumerate()
			.min_by(|a, b| (a.1 - nx).abs().total_cmp(&(b.1 - nx).abs()))
			.map(|(i, _)| i)?;
		let iy = p
			.axis_points
			.y
			.iter()
			.enumerate()
			.min_by(|a, b| (a.1 - ny).abs().total_cmp(&(b.1 - ny).abs()))
			.map(|(i, _)| i)?;
		Some((ix, iy))
	}

	fn handle_shortcuts(&mut self, ctx: &egui::Context) {
		if ctx.wants_keyboard_input() {
			return;
		}
		let cmd_k = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::K));
		if cmd_k {
			self.cmd_palette_open = !self.cmd_palette_open;
			self.cmd_query.clear();
		}
		let ctrl_z = ctx.input(|i| i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(Key::Z));
		if ctrl_z {
			self.exec_cmd("edit.undo", json::object! {}, "Undid last edit");
		}
		let ctrl_y = ctx.input(|i| {
			i.modifiers.ctrl && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z)))
		});
		if ctrl_y {
			self.exec_cmd("edit.redo", json::object! {}, "Redid edit");
		}

		let f6_pressed = ctx.input(|i| i.key_pressed(Key::F6));
		if f6_pressed {
			self.insert_keyframe_at_playhead();
		}

		// Brush size [ / ]
		ctx.input(|i| {
			if i.key_pressed(Key::OpenBracket) {
				self.brush_radius_ratio = (self.brush_radius_ratio - 0.08).clamp(0.12, 1.5);
			} else if i.key_pressed(Key::CloseBracket) {
				self.brush_radius_ratio = (self.brush_radius_ratio + 0.08).clamp(0.12, 1.5);
			} else if i.key_pressed(Key::V) {
				self.tool = StudioTool::Select;
			} else if i.key_pressed(Key::A) {
				self.tool = StudioTool::VertexSubselect;
			} else if i.key_pressed(Key::D) {
				self.tool = StudioTool::DeformBrush;
			} else if i.key_pressed(Key::P) {
				self.tool = StudioTool::Pose25D;
			} else if i.key_pressed(Key::M) {
				self.tool = StudioTool::Delaunay;
			} else if i.key_pressed(Key::G) {
				self.tool = StudioTool::MeshGroup;
			} else if i.key_pressed(Key::S) {
				self.tool = StudioTool::Physics;
			} else if i.key_pressed(Key::H) {
				self.tool = StudioTool::HandPan;
			} else if i.key_pressed(Key::Z) {
				self.fit_puppet_scale();
				self.cached_rev = u64::MAX;
			} else if i.key_pressed(Key::Space) {
				self.playing = !self.playing;
			} else if i.key_pressed(Key::Escape) {
				self.cmd_palette_open = false;
				self.selected_vertices.clear();
			}
		});

		// VertexSubselect keyboard nudging & deletion
		if self.tool == StudioTool::VertexSubselect && !self.selected_vertices.is_empty() {
			let delete_pressed = ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace));
			if delete_pressed {
				self.delete_selected_vertices();
				return;
			}
			let shift = ctx.input(|i| i.modifiers.shift);
			let step = if shift { 5.0 } else { 1.0 };
			let mut nudge = GVec2::ZERO;
			ctx.input(|i| {
				if i.key_pressed(Key::ArrowLeft) {
					nudge.x -= step;
				}
				if i.key_pressed(Key::ArrowRight) {
					nudge.x += step;
				}
				if i.key_pressed(Key::ArrowUp) {
					nudge.y -= step;
				}
				if i.key_pressed(Key::ArrowDown) {
					nudge.y += step;
				}
			});
			if nudge.length_squared() > 0.0 {
				if let Some(id) = self.selected_node {
					let indices: Vec<u32> = self.selected_vertices.iter().copied().collect();
					self.begin_gesture_undo();
					match self.vertex_mode {
						VertexEditMode::DeformKeypoint => {
							let pval = self
								.session
								.param_overrides
								.get(&self.brush_param)
								.copied()
								.unwrap_or(GVec2::ZERO);
							let _ = apply_vertex_deform_delta(
								&mut self.session.model.puppet,
								id,
								&self.brush_param,
								pval,
								&indices,
								nudge,
								false,
							);
							self.session.model.puppet.reset_contexts();
						}
						VertexEditMode::BaseTopology => {
							if let Some(mesh) = self.session.model.puppet.node_comps.get_mut::<Mesh>(id) {
								for &vi in &indices {
									if let Some(v) = mesh.vertices.get_mut(vi as usize) {
										*v += nudge;
									}
								}
								let verts = mesh.vertices.clone();
								let _ = retriangulate_points_preserving_deforms(&mut self.session.model.puppet, id, verts);
							}
						}
					}
					self.finish_gesture_undo();
					self.set_status(true, format!("Nudged {} selected vertices", indices.len()));
				}
			}
		}
	}

	fn delete_selected_vertices(&mut self) {
		let Some(id) = self.selected_node else {
			return;
		};
		if self.selected_vertices.is_empty() {
			return;
		}
		let indices: Vec<u32> = self.selected_vertices.iter().copied().collect();
		self.exec_cmd(
			"mesh.edit_topology",
			json::object! {
				node_uuid: id.0,
				action: "remove",
				indices: indices
			},
			"Removed vertices & retriangulated cleanly",
		);
		self.selected_vertices.clear();
	}

	fn insert_keyframe_at_playhead(&mut self) {
		let frame = self.playhead_frame.round() as i32;
		let (param_name, target) = self
			.selected_curve_lane
			.clone()
			.unwrap_or_else(|| (self.brush_param.clone(), 0));
		let cur = self
			.session
			.param_overrides
			.get(&param_name)
			.copied()
			.or_else(|| self.session.model.puppet.params.get(&param_name).map(|p| p.defaults))
			.unwrap_or(GVec2::ZERO);
		let val = if target == 1 { cur.y } else { cur.x };
		self.exec_cmd(
			"anim.set_keyframe",
			json::object! {
				anim: "Idle",
				param: param_name.as_str(),
				target: target,
				frame: frame,
				value: val,
				tension: 0.5
			},
			&format!("Inserted keyframe @ frame {frame} on {param_name}"),
		);
		self.selected_keyframe = Some((param_name, target, frame));
	}

	fn remove_selected_keyframe(&mut self) {
		let Some((pname, target, frame)) = self.selected_keyframe.take() else {
			return;
		};
		self.exec_cmd(
			"anim.remove_keyframe",
			json::object! {
				anim: "Idle",
				param: pname.as_str(),
				target: target,
				frame: frame
			},
			&format!("Removed keyframe @ frame {frame}"),
		);
	}

	fn draw_menu_bar(&mut self, ui: &mut egui::Ui) {
		egui::menu::bar(ui, |ui| {
			ui.menu_button("File", |ui| {
				if ui.button("Load Sample VTuber").clicked() {
					if let Ok(s) = load_initial_session("--sample") {
						self.session = s;
						self.selected_vertices.clear();
						self.gpu_tex_dirty = true;
						self.sync_watch_out();
						self.fit_puppet_scale();
						self.cached_rev = u64::MAX;
						self.audit_rev = u64::MAX;
						self.set_status(true, "Loaded built-in sample VTuber puppet");
					}
					ui.close_menu();
				}
				if ui.button("Save .inp (to current/watch path)").clicked() {
					let out = self
						.watch_path
						.clone()
						.or_else(|| self.source_path.clone())
						.unwrap_or_else(|| PathBuf::from("output_studio.inp"));
					let out_str = out.display().to_string();
					self.exec_cmd("puppet.save", json::object! { path: out_str.as_str() }, &format!("Saved {}", out.display()));
					ui.close_menu();
				}
				if ui.button("Export Spec Card PNG (FigCraft)").clicked() {
					self.exec_cmd(
						"craft.figcraft_card",
						json::object! { out: "studio_spec_card.png" },
						"Exported studio_spec_card.png via FigCraft",
					);
					ui.close_menu();
				}
				if ui.button("Export Animated Idle GIF").clicked() {
					self.exec_cmd(
						"render.gif",
						json::object! { out: "studio_idle.gif", anim: "Idle", frames: 24, fps: 15 },
						"Exported studio_idle.gif",
					);
					ui.close_menu();
				}
			});

			ui.menu_button("Edit", |ui| {
				if ui.button("Undo (Ctrl+Z)").clicked() {
					self.exec_cmd("edit.undo", json::object! {}, "Undid last edit");
					ui.close_menu();
				}
				if ui.button("Redo (Ctrl+Y)").clicked() {
					self.exec_cmd("edit.redo", json::object! {}, "Redid edit");
					ui.close_menu();
				}
				if ui.button("Reset Parameter Pose").clicked() {
					self.session.param_overrides.clear();
					self.cached_rev = u64::MAX;
					self.set_status(true, "Reset parameter pose to neutral");
					ui.close_menu();
				}
			});

			ui.menu_button("View", |ui| {
				ui.checkbox(&mut self.wireframe, "Show Render Wireframe");
				ui.checkbox(&mut self.xray_mode, "X-Ray Geometric Diagnostic");
				ui.checkbox(&mut self.show_sel_overlay, "Selected Node Vector Overlay");
				if ui.button("Fit Puppet to Stage (Z)").clicked() {
					self.fit_puppet_scale();
					self.cached_rev = u64::MAX;
					ui.close_menu();
				}
			});

			ui.menu_button("Rig", |ui| {
				if ui.button("Auto-Rig Full Puppet (EN/ZH Semantics)").clicked() {
					self.exec_cmd("rig.auto_rig_vtuber", json::object! {}, "Auto-rigged puppet layers");
					ui.close_menu();
				}
				if ui.button("Upgrade Mouth -> 2D A/I/U/E/O Visemes").clicked() {
					self.exec_cmd("rig.auto_visemes", json::object! { node_name: "Mouth" }, "Bound 2D A/I/U/E/O visemes");
					ui.close_menu();
				}
				if ui.button("Create 120f Idle Animation").clicked() {
					self.exec_cmd("anim.create_idle", json::object! {}, "Generated Idle animation");
					ui.close_menu();
				}
			});

			ui.menu_button("Audit", |ui| {
				if ui.button("Run 6-Point Rig Audit").clicked() {
					self.audit_rev = u64::MAX;
					self.refresh_audit();
					let s = self.audit_cache["score"].as_u32().unwrap_or(0);
					self.set_status(s >= 80, format!("Rig audit score: {s}/100"));
					ui.close_menu();
				}
				if ui.button("1-Click Constraint Auto-Fix").clicked() {
					self.exec_cmd("rig.auto_fix", json::object! {}, "Ran deterministic rig constraint auto-fix");
					ui.close_menu();
				}
			});

			ui.menu_button("Overlays", |ui| {
				for kind in ["blush", "gloom", "tears", "angry", "shock"] {
					if ui.button(format!("+ Synthesize {kind} Overlay")).clicked() {
						self.exec_cmd(
							"node.synthesize_overlay",
							json::object! { kind: kind },
							&format!("Synthesized '{kind}' overlay"),
						);
						ui.close_menu();
					}
				}
			});

			if ui.button("Commands (Ctrl+K)").clicked() {
				self.cmd_palette_open = true;
				self.cmd_query.clear();
			}

			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				ui.label(egui::RichText::new("Inox2D Studio · Craft Shell").color(TEXT_MUTED).size(11.0));
			});
		});
	}

	fn draw_context_bar(&mut self, ui: &mut egui::Ui) {
		ui.horizontal(|ui| {
			let pname = self
				.session
				.model
				.puppet
				.meta
				.name
				.as_deref()
				.unwrap_or("InochiSampleVTuber");
			egui::Frame::none()
				.fill(BG_WIDGET)
				.stroke(Stroke::new(1.0_f32, BORDER))
				.rounding(Rounding::same(4.0))
				.inner_margin(Margin::symmetric(8.0, 2.0))
				.show(ui, |ui| {
					ui.label(egui::RichText::new(pname).strong().size(11.5));
				});

			let sel_name = self
				.selected_node
				.and_then(|id| self.session.model.puppet.nodes.get_node(id))
				.map(|n| n.name.as_str())
				.unwrap_or("Root");
			ui.label(egui::RichText::new(format!("> {sel_name}")).color(TEXT_MUTED).size(11.5));
			ui.separator();

			if ui.button("Auto-Rig").clicked() {
				self.exec_cmd("rig.auto_rig_vtuber", json::object! {}, "Auto-rigged puppet layers");
			}
			if ui.button("Auto-Fix").clicked() {
				self.exec_cmd("rig.auto_fix", json::object! {}, "Repaired leaks, foldovers & masks");
			}
			ui.separator();

			// Ergonomic dynamic contextual controls matching active tool
			match self.tool {
				StudioTool::Select => {
					if let Some(id) = self.selected_node {
						if self.session.model.puppet.node_comps.get::<TexturedMesh>(id).is_some()
							&& ui.button("Delaunay").clicked()
						{
							self.exec_cmd(
								"mesh.auto_delaunay",
								json::object! { node_uuid: id.0, step_px: 20.0 },
								"Rebuilt selected Part mesh with Bowyer-Watson Delaunay",
							);
						}
					}
					ui.label(
						egui::RichText::new("Drag box to move · Corners to scale · A = Vertex Edit")
							.size(10.5)
							.color(TEXT_MUTED),
					);
				}
				StudioTool::VertexSubselect => {
					ui.selectable_value(&mut self.vertex_mode, VertexEditMode::DeformKeypoint, "Deform Keypoint");
					ui.selectable_value(&mut self.vertex_mode, VertexEditMode::BaseTopology, "Base Topology");
					if self.vertex_mode == VertexEditMode::DeformKeypoint {
						let mut param_keys: Vec<String> = self.session.model.puppet.params.keys().cloned().collect();
						param_keys.sort();
						egui::ComboBox::from_id_source("ctx_vertex_param")
							.selected_text(self.brush_param.as_str())
							.width(132.0)
							.show_ui(ui, |ui| {
								for k in param_keys {
									ui.selectable_value(&mut self.brush_param, k.clone(), k);
								}
							});
						if !self.selected_vertices.is_empty() && ui.small_button("Reset Verts").clicked() {
							if let Some(id) = self.selected_node {
								let indices: Vec<u32> = self.selected_vertices.iter().copied().collect();
								let pval = self
									.session
									.param_overrides
									.get(&self.brush_param)
									.copied()
									.unwrap_or(GVec2::ZERO);
								self.exec_cmd(
									"deform.set_vertices",
									json::object! {
										node_uuid: id.0,
										param: self.brush_param.as_str(),
										kx: pval.x,
										ky: pval.y,
										indices: indices,
										reset: true
									},
									"Reset selected vertices in active keypoint",
								);
							}
						}
					} else if !self.selected_vertices.is_empty() && ui.small_button("Delete (Del)").clicked() {
						self.delete_selected_vertices();
					}
					ui.label(
						egui::RichText::new(format!("{} sel · Dbl-click=Insert", self.selected_vertices.len()))
							.monospace()
							.size(10.0)
							.color(ACCENT_BLUE),
					);
				}
				StudioTool::DeformBrush => {
					let mut param_keys: Vec<String> = self.session.model.puppet.params.keys().cloned().collect();
					param_keys.sort();
					egui::ComboBox::from_id_source("ctx_brush_param")
						.selected_text(self.brush_param.as_str())
						.width(140.0)
						.show_ui(ui, |ui| {
							for k in param_keys {
								ui.selectable_value(&mut self.brush_param, k.clone(), k);
							}
						});
					ui.add(egui::Slider::new(&mut self.brush_radius_ratio, 0.12..=1.25).text("Radius"));
				}
				StudioTool::Physics => {
					let phys_nodes: Vec<(InoxNodeUuid, String)> = self
						.session
						.model
						.puppet
						.nodes
						.iter()
						.filter(|n| self.session.model.puppet.node_comps.get::<SimplePhysics>(n.uuid).is_some())
						.map(|n| (n.uuid, n.name.clone()))
						.collect();
					if let Some((pid, pname)) = phys_nodes
						.iter()
						.find(|(id, _)| Some(*id) == self.selected_physics)
						.or_else(|| phys_nodes.first())
						.cloned()
					{
						self.selected_physics = Some(pid);
						if let Some(phys) = self.session.model.puppet.node_comps.get::<SimplePhysics>(pid).cloned() {
							let mut len = phys.props.length;
							let mut freq = phys.props.frequency;
							let mut damp = phys.props.angle_damping;
							ui.label(egui::RichText::new(format!("{pname}:")).size(10.5).color(AUDIT_WARN_AMBER));
							if ui.add(egui::DragValue::new(&mut len).speed(1.0).range(10.0..=500.0).prefix("Len:")).changed() {
								self.exec_cmd("physics.configure", json::object! { node_uuid: pid.0, length: len }, "Updated physics length");
							}
							if ui.add(egui::DragValue::new(&mut freq).speed(0.05).range(0.1..=12.0).prefix("Hz:")).changed() {
								self.exec_cmd("physics.configure", json::object! { node_uuid: pid.0, frequency: freq }, "Updated physics frequency");
							}
							if ui.add(egui::DragValue::new(&mut damp).speed(0.02).range(0.0..=5.0).prefix("Damp:")).changed() {
								self.exec_cmd("physics.configure", json::object! { node_uuid: pid.0, angle_damping: damp }, "Updated physics damping");
							}
						}
					} else {
						ui.label(egui::RichText::new("No SimplePhysics nodes — attach from S tool").size(10.5).color(TEXT_MUTED));
					}
				}
				_ => {
					ui.label(egui::RichText::new(self.tool.tooltip()).size(10.5).color(TEXT_MUTED));
				}
			}

			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				let zoom_txt = format!("{:.0}%", self.view_zoom * 100.0);
				if ui.button(&zoom_txt).on_hover_text("Click to fit stage zoom (Z)").clicked() {
					self.fit_puppet_scale();
					self.cached_rev = u64::MAX;
				}

				let score = self.audit_cache["score"].as_u32().unwrap_or(0);
				let passed = self.audit_cache["passed"].as_bool().unwrap_or(false);
				let (bg, fg, label) = if passed {
					(Color32::from_rgb(6, 78, 59), AUDIT_OK_GREEN, format!("{score}/100 PASS"))
				} else {
					(Color32::from_rgb(76, 5, 25), AUDIT_ERR_ROSE, format!("{score}/100 FIX"))
				};
				egui::Frame::none()
					.fill(bg)
					.stroke(Stroke::new(1.0_f32, fg))
					.rounding(Rounding::same(4.0))
					.inner_margin(Margin::symmetric(8.0, 2.0))
					.show(ui, |ui| {
						ui.label(egui::RichText::new(label).color(fg).monospace().strong().size(11.0));
					});

				ui.checkbox(&mut self.xray_mode, "X-Ray");
				ui.checkbox(&mut self.wireframe, "Wireframe");
			});
		});
	}

	fn draw_stage(&mut self, ui: &mut egui::Ui) {
		let rect = ui.available_rect_before_wrap();
		let painter = ui.painter_at(rect);
		painter.rect_filled(rect, 0.0, BG_STAGE);

		let resp = ui.allocate_rect(rect, Sense::click_and_drag());

		// Handle scroll-wheel zoom
		if resp.hovered() {
			let scroll = ui.ctx().input(|i| i.raw_scroll_delta.y);
			if scroll.abs() > 0.1 {
				let factor = (1.0 + scroll * 0.0015).clamp(0.85, 1.18);
				self.view_zoom = (self.view_zoom * factor).clamp(0.4, 3.5);
			}
		}

		let card_side = (rect.width().min(rect.height()) - 32.0).max(220.0) * self.view_zoom;
		let card_center = rect.center() + self.view_pan;
		let canvas_rect = Rect::from_center_size(card_center, vec2(card_side, card_side));
		let world_per_ui = (self.stage_res as f32 / canvas_rect.width().max(1.0)) / self.puppet_scale.max(0.05);
		let px_per_world = 1.0 / world_per_ui.max(1e-4);

		// Stage checkerboard card
		painter.rect_filled(canvas_rect, 2.0, Color32::from_rgb(24, 25, 30));
		let clip_painter = ui.painter_at(canvas_rect.intersect(rect));
		let tile = 16.0;
		let cols = (canvas_rect.width() / tile).ceil() as i32;
		let rows = (canvas_rect.height() / tile).ceil() as i32;
		let c1 = Color32::from_rgb(31, 33, 40);
		for r in 0..rows {
			for c in 0..cols {
				if (r + c) % 2 == 0 {
					let tr = Rect::from_min_size(
						pos2(canvas_rect.left() + c as f32 * tile, canvas_rect.top() + r as f32 * tile),
						vec2(tile, tile),
					);
					clip_painter.rect_filled(tr, 0.0, c1);
				}
			}
		}
		painter.rect_stroke(canvas_rect, 2.0, Stroke::new(1.0_f32, Color32::from_rgb(18, 18, 22)));

		if self.xray_mode {
			if let Some(tex) = &self.stage_tex {
				clip_painter.image(
					tex.id(),
					canvas_rect,
					Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
					Color32::WHITE,
				);
			}
		} else {
			self.draw_puppet_gpu_meshes(&clip_painter, canvas_rect);
		}

		let hover_pos = resp.hover_pos();
		let shift_held = ui.ctx().input(|i| i.modifiers.shift);
		let middle_down = ui.ctx().input(|i| i.pointer.middle_down());

		let sel_geom = self.selected_node_screen_geometry(canvas_rect);

		// Selected node vector bbox, wireframe, vertices & handles
		if self.show_sel_overlay {
			if let Some((ref pts, ref indices, bbox, pivot)) = sel_geom {
				let expanded = bbox.expand(2.0);
				if !self.wireframe || self.tool == StudioTool::VertexSubselect {
					let alpha = if self.tool == StudioTool::VertexSubselect { 135 } else { 90 };
					let wire_stroke = Stroke::new(0.8_f32, Color32::from_rgba_unmultiplied(59, 130, 246, alpha));
					for tri in indices.chunks_exact(3) {
						let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
						if a < pts.len() && b < pts.len() && c < pts.len() {
							clip_painter.line_segment([pts[a], pts[b]], wire_stroke);
							clip_painter.line_segment([pts[b], pts[c]], wire_stroke);
							clip_painter.line_segment([pts[c], pts[a]], wire_stroke);
						}
					}
				}
				clip_painter.rect_stroke(expanded, 0.0, Stroke::new(1.25_f32, ACCENT_BLUE));
				for corner in [
					expanded.left_top(),
					expanded.right_top(),
					expanded.right_bottom(),
					expanded.left_bottom(),
				] {
					clip_painter.rect_filled(Rect::from_center_size(corner, vec2(6.5, 6.5)), 0.0, Color32::WHITE);
					clip_painter.rect_stroke(
						Rect::from_center_size(corner, vec2(6.5, 6.5)),
						0.0,
						Stroke::new(1.0_f32, ACCENT_BLUE),
					);
				}
				clip_painter.circle_stroke(pivot, 5.0, Stroke::new(1.3_f32, Color32::from_rgb(250, 204, 21)));

				// Vertex Subselect handles (A tool)
				if self.tool == StudioTool::VertexSubselect {
					let hover_vi = hover_pos.and_then(|hp| {
						pts.iter()
							.enumerate()
							.filter(|(_, p)| p.distance(hp) < 10.0)
							.min_by(|a, b| a.1.distance(hp).total_cmp(&b.1.distance(hp)))
							.map(|(vi, _)| vi as u32)
					});
					for (vi, &sp) in pts.iter().enumerate() {
						let vi32 = vi as u32;
						let is_sel = self.selected_vertices.contains(&vi32);
						let is_hov = hover_vi == Some(vi32);
						if is_sel {
							clip_painter.circle_filled(sp, 4.2, Color32::from_rgb(56, 189, 248));
							clip_painter.circle_stroke(sp, 4.2, Stroke::new(1.3_f32, Color32::WHITE));
						} else if is_hov {
							clip_painter.circle_filled(sp, 3.8, AUDIT_WARN_AMBER);
							clip_painter.circle_stroke(sp, 3.8, Stroke::new(1.0_f32, Color32::WHITE));
						} else {
							clip_painter.circle_filled(sp, 2.7, Color32::from_rgb(30, 41, 59));
							clip_painter.circle_stroke(sp, 2.7, Stroke::new(1.0_f32, Color32::from_rgb(148, 163, 184)));
						}
					}
				}
			}
		}

		// Collect SimplePhysics pendulum gizmos for S tool (or subtle highlight)
		let mut phys_gizmos: Vec<(InoxNodeUuid, String, egui::Pos2, egui::Pos2, f32)> = Vec::new();
		if self.tool == StudioTool::Physics {
			for n in self.session.model.puppet.nodes.iter() {
				if let Some(phys) = self.session.model.puppet.node_comps.get::<SimplePhysics>(n.uuid) {
					let anchor_world = self
						.session
						.model
						.puppet
						.node_comps
						.get::<TransformStore>(n.uuid)
						.map(|ts| {
							let p = ts.absolute * gvec3(0.0, 0.0, 0.0).extend(1.0);
							gvec2(p.x, p.y)
						})
						.unwrap_or(GVec2::ZERO);
					let anchor_ui = self.world_to_ui(canvas_rect, anchor_world);
					let rod_px = phys.props.length * px_per_world;
					let bob_ui = anchor_ui + vec2(0.0, rod_px);
					phys_gizmos.push((n.uuid, n.name.clone(), anchor_ui, bob_ui, phys.props.length));
				}
			}
			for (pid, pname, a_ui, b_ui, len) in &phys_gizmos {
				let is_sel = Some(*pid) == self.selected_physics;
				let col = if is_sel { AUDIT_WARN_AMBER } else { Color32::from_rgb(52, 211, 153) };
				// Draw pendulum swing arc indicator
				let rod_px = (*len * px_per_world).max(8.0);
				let mut arc_pts = Vec::new();
				for step in -8..=8 {
					let ang = (step as f32 / 8.0) * 0.45;
					arc_pts.push(pos2(a_ui.x + rod_px * ang.sin(), a_ui.y + rod_px * ang.cos()));
				}
				clip_painter.add(egui::Shape::line(
					arc_pts,
					Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 90)),
				));
				clip_painter.line_segment([*a_ui, *b_ui], Stroke::new(1.8_f32, col));
				clip_painter.circle_stroke(*a_ui, 4.0, Stroke::new(1.4_f32, Color32::WHITE));
				clip_painter.circle_filled(*b_ui, 6.5, col);
				clip_painter.circle_stroke(*b_ui, 6.5, Stroke::new(1.4_f32, Color32::WHITE));
				clip_painter.text(
					*b_ui + vec2(10.0, 0.0),
					Align2::LEFT_CENTER,
					format!("{pname} ({:.0}px)", len),
					egui::FontId::monospace(10.0),
					col,
				);
			}
		}

		// Double-click in VertexSubselect -> insert vertex & retriangulate cleanly
		if self.tool == StudioTool::VertexSubselect && resp.double_clicked() {
			if let (Some(id), Some(pos)) = (self.selected_node, hover_pos) {
				if canvas_rect.contains(pos) {
					if let Some(local_xy) = self.ui_to_local_on_node(id, canvas_rect, pos) {
						let prev_rev = self.session.revision;
						if let Ok(res) = self.session.exec(
							"mesh.edit_topology",
							&json::object! {
								node_uuid: id.0,
								action: "insert",
								x: local_xy.x,
								y: local_xy.y
							},
						) {
							if let Some(new_vi) = res["inserted_index"].as_u32() {
								self.selected_vertices.clear();
								self.selected_vertices.insert(new_vi);
							}
							if self.session.revision != prev_rev {
								self.sync_watch_out();
								self.cached_rev = u64::MAX;
								self.audit_rev = u64::MAX;
							}
							self.set_status(true, format!("Inserted vertex at ({:.1}, {:.1}) + Delaunay", local_xy.x, local_xy.y));
						}
					}
				}
			}
		}

		// Single click handling (picking Parts, Vertices, or Physics gizmos)
		if resp.clicked() && !resp.double_clicked() {
			if let Some(pos) = resp.interact_pointer_pos() {
				match self.tool {
					StudioTool::Select => {
						let mut picked: Option<(InoxNodeUuid, f32)> = None;
						let old = self.selected_node;
						let mut best_z = f32::INFINITY;
						for node in self.session.model.puppet.nodes.iter() {
							if self.session.model.puppet.node_comps.get::<TexturedMesh>(node.uuid).is_none() {
								continue;
							}
							self.selected_node = Some(node.uuid);
							if let Some((_pts, _idx, bbox, _)) = self.selected_node_screen_geometry(canvas_rect) {
								let z = self.session.model.puppet.node_zsort(node.uuid).unwrap_or(node.zsort);
								if bbox.expand(4.0).contains(pos) && z < best_z {
									best_z = z;
									picked = Some((node.uuid, z));
								}
							}
						}
						self.selected_node = picked.map(|(id, _)| id).or(old);
						self.selected_vertices.clear();
						if let Some(id) = picked.map(|(id, _)| id) {
							if let Some(n) = self.session.model.puppet.nodes.get_node(id) {
								self.set_status(true, format!("Selected '{}' (UUID {})", n.name, id.0));
							}
						}
					}
					StudioTool::VertexSubselect => {
						if let Some((ref pts, _, _, _)) = sel_geom {
							let hit = pts
								.iter()
								.enumerate()
								.filter(|(_, p)| p.distance(pos) < 10.0)
								.min_by(|a, b| a.1.distance(pos).total_cmp(&b.1.distance(pos)))
								.map(|(vi, _)| vi as u32);
							if let Some(vi) = hit {
								if shift_held {
									if self.selected_vertices.contains(&vi) {
										self.selected_vertices.remove(&vi);
									} else {
										self.selected_vertices.insert(vi);
									}
								} else {
									self.selected_vertices.clear();
									self.selected_vertices.insert(vi);
								}
							} else if !shift_held {
								self.selected_vertices.clear();
							}
						}
					}
					StudioTool::Physics => {
						if let Some((pid, _, _, _, _)) = phys_gizmos.iter().find(|(_, _, _, b_ui, _)| b_ui.distance(pos) < 14.0) {
							self.selected_physics = Some(*pid);
						}
					}
					_ => {}
				}
			}
		}

		// Drag Start: latch exact drag intent + start coalesced Undo snapshot
		if resp.drag_started() {
			if let Some(pos) = resp.interact_pointer_pos() {
				if middle_down || self.tool == StudioTool::HandPan {
					self.stage_drag = Some(StageDrag::HandPan);
				} else {
					match self.tool {
						StudioTool::Select => {
							if let (Some(id), Some((_, _, bbox, _))) = (self.selected_node, sel_geom.as_ref()) {
								let exp = bbox.expand(2.0);
								let corners = [exp.left_top(), exp.right_top(), exp.right_bottom(), exp.left_bottom()];
								let hit_corner = corners.iter().any(|c| c.distance(pos) <= 9.0);
								if hit_corner {
									let orig_scale = self
										.session
										.model
										.puppet
										.nodes
										.get_node(id)
										.map(|n| n.trans_offset.scale)
										.unwrap_or(GVec2::ONE);
									self.begin_gesture_undo();
									self.stage_drag = Some(StageDrag::ScaleCorner {
										orig_scale,
										orig_dist: pos.distance(exp.center()).max(8.0),
									});
								} else if exp.contains(pos) {
									self.begin_gesture_undo();
									self.stage_drag = Some(StageDrag::MoveNode);
								}
							}
						}
						StudioTool::VertexSubselect => {
							if let Some((ref pts, _, _, _)) = sel_geom {
								let hit = pts
									.iter()
									.enumerate()
									.filter(|(_, p)| p.distance(pos) < 10.0)
									.min_by(|a, b| a.1.distance(pos).total_cmp(&b.1.distance(pos)))
									.map(|(vi, _)| vi as u32);
								if let Some(vi) = hit {
									if !self.selected_vertices.contains(&vi) {
										if !shift_held {
											self.selected_vertices.clear();
										}
										self.selected_vertices.insert(vi);
									}
									self.begin_gesture_undo();
									self.stage_drag = Some(StageDrag::VertexMove);
								} else {
									self.stage_drag = Some(StageDrag::VertexMarquee { start: pos, end: pos });
								}
							}
						}
						StudioTool::DeformBrush => {
							self.begin_gesture_undo();
							self.stage_drag = Some(StageDrag::DeformBrush);
						}
						StudioTool::Physics => {
							if let Some((pid, _, a_ui, b_ui, _)) =
								phys_gizmos.iter().find(|(_, _, _, b_ui, _)| b_ui.distance(pos) < 16.0)
							{
								self.selected_physics = Some(*pid);
								self.begin_gesture_undo();
								self.stage_drag = Some(StageDrag::PhysicsLength {
									phys_id: *pid,
									anchor_ui: *a_ui,
									px_per_world,
								});
								let _ = b_ui;
							}
						}
						StudioTool::Pose25D => {
							self.stage_drag = Some(StageDrag::Pose25D);
						}
						_ => {}
					}
				}
			}
		}

		// Drag Active
		if resp.dragged() {
			let delta = resp.drag_delta();
			let pos = resp.interact_pointer_pos();
			match self.stage_drag.clone() {
				Some(StageDrag::HandPan) => {
					self.view_pan += delta;
				}
				Some(StageDrag::MoveNode) => {
					if let Some(id) = self.selected_node {
						if let Some(n) = self.session.model.puppet.nodes.get_node_mut(id) {
							n.trans_offset.translation.x += delta.x * world_per_ui;
							n.trans_offset.translation.y += delta.y * world_per_ui;
							self.session.revision += 1;
							self.cached_rev = u64::MAX;
						}
					}
				}
				Some(StageDrag::ScaleCorner { orig_scale, orig_dist }) => {
					if let (Some(id), Some(p), Some((_, _, bbox, _))) = (self.selected_node, pos, sel_geom.as_ref()) {
						let ratio = (p.distance(bbox.center()) / orig_dist).clamp(0.15, 5.0);
						if let Some(n) = self.session.model.puppet.nodes.get_node_mut(id) {
							n.trans_offset.scale = orig_scale * ratio;
							self.session.revision += 1;
							self.cached_rev = u64::MAX;
						}
					}
				}
				Some(StageDrag::VertexMove) => {
					if let Some(id) = self.selected_node {
						if delta.length_sq() > 0.01 && !self.selected_vertices.is_empty() {
							let local_delta = gvec2(delta.x * world_per_ui, delta.y * world_per_ui);
							let indices: Vec<u32> = self.selected_vertices.iter().copied().collect();
							match self.vertex_mode {
								VertexEditMode::DeformKeypoint => {
									let pval = self
										.session
										.param_overrides
										.get(&self.brush_param)
										.copied()
										.unwrap_or(GVec2::ZERO);
									let _ = apply_vertex_deform_delta(
										&mut self.session.model.puppet,
										id,
										&self.brush_param,
										pval,
										&indices,
										local_delta,
										false,
									);
									self.session.model.puppet.reset_contexts();
								}
								VertexEditMode::BaseTopology => {
									if let Some(mesh) = self.session.model.puppet.node_comps.get_mut::<Mesh>(id) {
										for &vi in &indices {
											if let Some(v) = mesh.vertices.get_mut(vi as usize) {
												*v += local_delta;
											}
										}
									}
									self.session.model.puppet.reset_contexts();
								}
							}
							self.session.revision += 1;
							self.cached_rev = u64::MAX;
						}
					}
				}
				Some(StageDrag::VertexMarquee { start, .. }) => {
					if let Some(p) = pos {
						let mrect = Rect::from_two_pos(start, p);
						clip_painter.rect_filled(mrect, 0.0, Color32::from_rgba_unmultiplied(59, 130, 246, 36));
						clip_painter.rect_stroke(mrect, 0.0, Stroke::new(1.0_f32, ACCENT_BLUE));
						self.stage_drag = Some(StageDrag::VertexMarquee { start, end: p });
					}
				}
				Some(StageDrag::DeformBrush) => {
					if let (Some(id), Some(p)) = (self.selected_node, pos) {
						if delta.length() > 0.2 {
							if let Some(local_xy) = self.ui_to_local_on_node(id, canvas_rect, p) {
								let local_delta = gvec2(delta.x * world_per_ui, delta.y * world_per_ui);
								let pval = self
									.session
									.param_overrides
									.get(&self.brush_param)
									.copied()
									.unwrap_or(GVec2::ZERO);
								let _ = self.session.exec(
									"deform.brush",
									&json::object! {
										node_uuid: id.0,
										param: self.brush_param.as_str(),
										kx: pval.x,
										ky: pval.y,
										local_xy: [local_xy.x, local_xy.y],
										radius_ratio: self.brush_radius_ratio,
										dx: local_delta.x,
										dy: local_delta.y
									},
								);
								self.cached_rev = u64::MAX;
							}
						}
					}
				}
				Some(StageDrag::PhysicsLength {
					phys_id,
					anchor_ui,
					px_per_world: ppw,
				}) => {
					if let Some(p) = pos {
						let new_len = ((p.distance(anchor_ui)) / ppw.max(1e-4)).clamp(10.0, 500.0);
						let _ = configure_pendulum_physics(
							&mut self.session.model.puppet,
							phys_id,
							None,
							Some(new_len),
							None,
							None,
							None,
							None,
							None,
						);
						self.session.revision += 1;
						self.cached_rev = u64::MAX;
					}
				}
				Some(StageDrag::Pose25D) => {
					if let Some(p) = pos {
						let nx = ((p.x - canvas_rect.center().x) / (canvas_rect.width() * 0.45)).clamp(-1.0, 1.0);
						let ny = ((p.y - canvas_rect.center().y) / (canvas_rect.height() * 0.45)).clamp(-1.0, 1.0);
						self.session.param_overrides.insert("Head:: Yaw-Pitch".to_owned(), gvec2(nx, ny));
						self.session.param_overrides.insert("Head:: Roll".to_owned(), gvec2(nx * 0.35, 0.0));
						self.session.param_overrides.insert("Eye:: Gaze".to_owned(), gvec2(nx, ny));
						self.session.param_overrides.insert("Eye:: Left:: Gaze".to_owned(), gvec2(nx, ny));
						self.session.param_overrides.insert("Eye:: Right:: Gaze".to_owned(), gvec2(nx, ny));
					}
				}
				None => {}
			}
		}

		// Drag Stop: finalize marquee selection, retriangulate topology edits, commit coalesced Undo
		if resp.drag_stopped() {
			match self.stage_drag.take() {
				Some(StageDrag::VertexMarquee { start, end }) => {
					let mrect = Rect::from_two_pos(start, end);
					if let Some((ref pts, _, _, _)) = sel_geom {
						if !shift_held {
							self.selected_vertices.clear();
						}
						for (vi, &sp) in pts.iter().enumerate() {
							if mrect.contains(sp) {
								self.selected_vertices.insert(vi as u32);
							}
						}
						self.set_status(true, format!("Selected {} vertices in marquee", self.selected_vertices.len()));
					}
				}
				Some(StageDrag::VertexMove) => {
					if self.vertex_mode == VertexEditMode::BaseTopology {
						if let Some(id) = self.selected_node {
							if let Some(mesh) = self.session.model.puppet.node_comps.get::<Mesh>(id).cloned() {
								let _ = retriangulate_points_preserving_deforms(
									&mut self.session.model.puppet,
									id,
									mesh.vertices,
								);
							}
						}
					}
					self.finish_gesture_undo();
					self.set_status(true, format!("Edited {} vertices", self.selected_vertices.len()));
				}
				Some(StageDrag::MoveNode)
				| Some(StageDrag::ScaleCorner { .. })
				| Some(StageDrag::DeformBrush)
				| Some(StageDrag::PhysicsLength { .. }) => {
					self.finish_gesture_undo();
				}
				_ => {}
			}
		}

		// Deform Brush hover circle
		if self.tool == StudioTool::DeformBrush {
			if let Some(hover) = hover_pos {
				if canvas_rect.contains(hover) {
					clip_painter.circle_stroke(
						hover,
						28.0 * self.brush_radius_ratio,
						Stroke::new(1.2_f32, AUDIT_WARN_AMBER),
					);
				}
			}
		}

		// Stage Zoom indicator bottom right
		let badge = Rect::from_min_size(pos2(rect.right() - 56.0, rect.bottom() - 26.0), vec2(46.0, 18.0));
		painter.rect_filled(badge, 3.0, BG_DOCK);
		painter.rect_stroke(badge, 3.0, Stroke::new(1.0_f32, BORDER));
		painter.text(
			badge.center(),
			Align2::CENTER_CENTER,
			format!("{:.0}%", self.view_zoom * 100.0),
			egui::FontId::monospace(10.0),
			TEXT_MUTED,
		);
	}

	fn draw_xy_pad(&mut self, ui: &mut egui::Ui, param_name: &str, size: f32, puck_color: Color32) {
		let exists = self.session.model.puppet.params.get(param_name).cloned();
		let (min_v, max_v, def_v) = exists
			.as_ref()
			.map(|p| (p.min, p.max, p.defaults))
			.unwrap_or((gvec2(-1.0, -1.0), gvec2(1.0, 1.0), GVec2::ZERO));

		let is_armed = (self.tool == StudioTool::VertexSubselect && self.vertex_mode == VertexEditMode::DeformKeypoint
			|| self.tool == StudioTool::DeformBrush)
			&& self.brush_param == param_name;

		ui.vertical(|ui| {
			let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
			let painter = ui.painter();
			painter.rect_filled(rect, 5.0, BG_INPUT);
			painter.rect_stroke(
				rect,
				5.0,
				Stroke::new(if is_armed { 1.4_f32 } else { 1.0_f32 }, if is_armed { AUDIT_WARN_AMBER } else { BORDER }),
			);

			let c = rect.center();
			painter.line_segment(
				[pos2(rect.left(), c.y), pos2(rect.right(), c.y)],
				Stroke::new(1.0_f32, Color32::from_rgb(40, 42, 48)),
			);
			painter.line_segment(
				[pos2(c.x, rect.top()), pos2(c.x, rect.bottom())],
				Stroke::new(1.0_f32, Color32::from_rgb(40, 42, 48)),
			);

			if let Some(ref p) = exists {
				// Draw axis_points keypoint snap dots (3x3 lattice)
				for &ax in &p.axis_points.x {
					for &ay in &p.axis_points.y {
						let dp = pos2(rect.left() + ax * rect.width(), rect.top() + ay * rect.height());
						painter.circle_filled(dp, 2.0, Color32::from_rgb(71, 85, 105));
					}
				}

				if (resp.clicked() || resp.dragged()) && rect.width() > 2.0 {
					if let Some(ptr) = resp.interact_pointer_pos() {
						let mut tx = ((ptr.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
						let mut ty = ((ptr.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
						// Magnetic snap onto nearby axis keypoint dot (< 7px) for accurate keypoint sculpting
						for &ax in &p.axis_points.x {
							if ((ax - tx) * rect.width()).abs() < 7.0 {
								tx = ax;
								break;
							}
						}
						for &ay in &p.axis_points.y {
							if ((ay - ty) * rect.height()).abs() < 7.0 {
								ty = ay;
								break;
							}
						}
						let vx = p.min.x + (p.max.x - p.min.x) * tx;
						let vy = p.min.y + (p.max.y - p.min.y) * ty;
						self.session.param_overrides.insert(param_name.to_owned(), gvec2(vx, vy));
					}
				}
				if resp.double_clicked() {
					self.session.param_overrides.insert(param_name.to_owned(), def_v);
				}

				let cur = self.session.param_overrides.get(param_name).copied().unwrap_or(def_v);
				let span_x = (max_v.x - min_v.x).abs().max(1e-4);
				let span_y = (max_v.y - min_v.y).abs().max(1e-4);
				let nx = ((cur.x - min_v.x) / span_x).clamp(0.0, 1.0);
				let ny = ((cur.y - min_v.y) / span_y).clamp(0.0, 1.0);
				let puck = pos2(rect.left() + nx * rect.width(), rect.top() + ny * rect.height());
				painter.circle_filled(puck, 5.5, puck_color);
				painter.circle_stroke(puck, 5.5, Stroke::new(1.5_f32, Color32::WHITE));
			} else {
				painter.text(c, Align2::CENTER_CENTER, "N/A", egui::FontId::monospace(10.0), TEXT_MUTED);
			}

			if is_armed {
				let kp = self.keypoint_cell_for_param(param_name).unwrap_or((0, 0));
				ui.label(
					egui::RichText::new(format!("{param_name} [KP {},{}]", kp.0, kp.1))
						.monospace()
						.size(9.5)
						.color(AUDIT_WARN_AMBER),
				);
			} else {
				ui.label(egui::RichText::new(param_name).monospace().size(10.0).color(TEXT_MUTED));
			}
		});
	}

	fn draw_right_inspector(&mut self, ui: &mut egui::Ui) {
		egui::ScrollArea::vertical().show(ui, |ui| {
			section_header(ui, "Properties", None);
			ui.add_space(4.0);

			if let Some(id) = self.selected_node {
				if let Some(node) = self.session.model.puppet.nodes.get_node(id).cloned() {
					let is_mesh = self.session.model.puppet.node_comps.get::<TexturedMesh>(id).is_some();
					let is_mg = self.session.model.puppet.node_comps.get::<MeshGroup>(id).is_some();
					let kind_tag = if is_mg {
						"MeshGroup"
					} else if is_mesh {
						"Part"
					} else {
						"Node"
					};

					egui::Grid::new("prop_grid").num_columns(2).spacing([8.0, 5.0]).show(ui, |ui| {
						ui.label(egui::RichText::new("Name").color(TEXT_MUTED).size(11.0));
						ui.label(egui::RichText::new(format!("{}  ({} #{})", node.name, kind_tag, id.0)).strong().size(11.5));
						ui.end_row();

						let mut enabled = node.enabled;
						ui.label(egui::RichText::new("Visible").color(TEXT_MUTED).size(11.0));
						if ui.checkbox(&mut enabled, "Enabled").changed() {
							self.exec_cmd("node.configure", json::object! { node_uuid: id.0, enabled: enabled }, "Updated visibility");
						}
						ui.end_row();

						let mut tx = node.trans_offset.translation.x;
						let mut ty = node.trans_offset.translation.y;
						ui.label(egui::RichText::new("Offset").color(TEXT_MUTED).size(11.0));
						ui.horizontal(|ui| {
							let rx = ui.add(egui::DragValue::new(&mut tx).speed(0.5).prefix("X "));
							let ry = ui.add(egui::DragValue::new(&mut ty).speed(0.5).prefix("Y "));
							if rx.changed() || ry.changed() {
								self.exec_cmd("node.configure", json::object! { node_uuid: id.0, x: tx, y: ty }, "Moved node");
							}
						});
						ui.end_row();

						let mut z = node.zsort;
						ui.label(egui::RichText::new("Z-Sort").color(TEXT_MUTED).size(11.0));
						if ui.add(egui::DragValue::new(&mut z).speed(0.25)).changed() {
							self.exec_cmd("node.configure", json::object! { node_uuid: id.0, zsort: z }, "Updated Z-sort");
						}
						ui.end_row();

						if let Some(drw) = self.session.model.puppet.node_comps.get::<Drawable>(id).cloned() {
							let mut op = drw.blending.opacity;
							ui.label(egui::RichText::new("Opacity").color(TEXT_MUTED).size(11.0));
							if ui.add(egui::Slider::new(&mut op, 0.0..=1.0)).changed() {
								self.exec_cmd("node.configure", json::object! { node_uuid: id.0, opacity: op }, "Updated opacity");
							}
							ui.end_row();
						}
					});

					if let Some(m) = self.session.model.puppet.node_comps.get::<Mesh>(id) {
						let vc = m.vertices.len();
						let tc = m.indices.len() / 3;
						ui.horizontal(|ui| {
							ui.label(egui::RichText::new(format!("Mesh: {vc}v / {tc}t")).monospace().size(10.5).color(TEXT_MUTED));
							if is_mesh && ui.small_button("Delaunay").clicked() {
								self.exec_cmd(
									"mesh.auto_delaunay",
									json::object! { node_uuid: id.0, step_px: 20.0 },
									"Rebuilt Delaunay mesh",
								);
							}
							if is_mesh && ui.small_button("Contour").clicked() {
								self.exec_cmd(
									"mesh.auto_contour",
									json::object! { node_uuid: id.0, cols: 8, rows: 8 },
									"Rebuilt contour mesh",
								);
							}
							if is_mesh && ui.small_button("Grid").clicked() {
								self.exec_cmd(
									"mesh.auto_grid",
									json::object! { node_uuid: id.0, cols: 6, rows: 6 },
									"Rebuilt grid mesh",
								);
							}
						});
					}
				}
			} else {
				ui.label(egui::RichText::new("Select a node in the hierarchy or stage.").color(TEXT_MUTED).size(11.0));
			}

			ui.add_space(6.0);
			section_header(ui, "2D Parameter Pads", Some(("snap · dbl-click reset", TEXT_MUTED)));
			ui.add_space(4.0);
			ui.horizontal(|ui| {
				self.draw_xy_pad(ui, "Head:: Yaw-Pitch", 132.0, ACCENT_BLUE);
				ui.add_space(10.0);
				self.draw_xy_pad(ui, "Mouth:: Viseme", 132.0, TIMELINE_VIOLET);
			});
			ui.horizontal_wrapped(|ui| {
				for (label, vx, vy) in VISEME_PRESETS {
					if ui.small_button(label).clicked() {
						self.session.param_overrides.insert("Mouth:: Viseme".to_owned(), gvec2(vx, vy));
						self.session.param_overrides.insert("Mouth:: Form".to_owned(), gvec2(vx, 0.0));
						self.session.param_overrides.insert("Mouth:: Open".to_owned(), gvec2(vy, 0.0));
					}
				}
			});

			ui.add_space(6.0);
			section_header(ui, "Parameters & Overlays", None);
			ui.add_space(4.0);

			let mut pnames: Vec<(String, bool, GVec2, GVec2, GVec2)> = self
				.session
				.model
				.puppet
				.params
				.iter()
				.map(|(k, p)| (k.clone(), p.is_vec2, p.min, p.max, p.defaults))
				.collect();
			pnames.sort_by(|a, b| a.0.cmp(&b.0));

			for (pname, is_vec2, pmin, pmax, pdef) in pnames {
				if pname == "Head:: Yaw-Pitch" || pname == "Mouth:: Viseme" {
					continue;
				}
				let cur = self.session.param_overrides.get(&pname).copied().unwrap_or(pdef);
				let mut x = cur.x;
				let mut y = cur.y;
				let short = pname.as_str();
				if !is_vec2 {
					ui.horizontal(|ui| {
						ui.add_sized([116.0, 16.0], egui::Label::new(egui::RichText::new(short).monospace().size(10.5).color(TEXT_MUTED)));
						if ui.add(egui::Slider::new(&mut x, pmin.x..=pmax.x).show_value(true)).changed() {
							self.session.param_overrides.insert(pname.clone(), gvec2(x, 0.0));
						}
					});
				} else {
					ui.horizontal(|ui| {
						ui.add_sized([116.0, 16.0], egui::Label::new(egui::RichText::new(short).monospace().size(10.0).color(TEXT_MUTED)));
						let rx = ui.add(egui::DragValue::new(&mut x).speed(0.02).range(pmin.x..=pmax.x).prefix("X:"));
						let ry = ui.add(egui::DragValue::new(&mut y).speed(0.02).range(pmin.y..=pmax.y).prefix("Y:"));
						if rx.changed() || ry.changed() {
							self.session.param_overrides.insert(pname.clone(), gvec2(x, y));
						}
					});
				}
			}

			if ui.small_button("Reset All Parameters").clicked() {
				self.session.param_overrides.clear();
			}

			ui.add_space(6.0);
			let score = self.audit_cache["score"].as_u32().unwrap_or(0);
			let passed = self.audit_cache["passed"].as_bool().unwrap_or(false);
			let badge_col = if passed { AUDIT_OK_GREEN } else { AUDIT_WARN_AMBER };
			let badge_str = format!("{score}/100");
			section_header(ui, "6-Point Rig Audit", Some((&badge_str, badge_col)));
			ui.add_space(4.0);

			let checks = [
				("Containment Leaks", self.audit_cache["summary"]["containment_leaks"].as_usize().unwrap_or(0)),
				("Mesh Foldovers", self.audit_cache["summary"]["mesh_foldovers"].as_usize().unwrap_or(0)),
				("Extreme Stretch", self.audit_cache["summary"]["mesh_stretch"].as_usize().unwrap_or(0)),
				("Blink Symmetry", self.audit_cache["summary"]["blink_issues"].as_usize().unwrap_or(0)),
				("Yaw Symmetry", self.audit_cache["summary"]["symmetry_defects"].as_usize().unwrap_or(0)),
				("Clipping Masks", self.audit_cache["summary"]["unmasked_parts"].as_usize().unwrap_or(0)),
			];
			for (lbl, cnt) in checks {
				ui.horizontal(|ui| {
					let dot_col = if cnt == 0 { AUDIT_OK_GREEN } else { AUDIT_ERR_ROSE };
					let (txt, col) = if cnt == 0 {
						("OK".to_owned(), AUDIT_OK_GREEN)
					} else {
						(format!("{cnt} ISSUE"), AUDIT_ERR_ROSE)
					};
					let (dr, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
					ui.painter().circle_filled(dr.center(), 3.2, dot_col);
					ui.label(egui::RichText::new(lbl).size(11.0).color(TEXT_PRIMARY));
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						ui.label(egui::RichText::new(txt).monospace().size(10.5).color(col));
					});
				});
			}

			ui.add_space(4.0);
			ui.horizontal(|ui| {
				if ui.button("Run Audit").clicked() {
					self.audit_rev = u64::MAX;
					self.refresh_audit();
				}
				let fix_btn = egui::Button::new(egui::RichText::new("1-Click Auto-Fix").color(Color32::WHITE).strong())
					.fill(BTN_ACTIVE);
				if ui.add(fix_btn).clicked() {
					self.exec_cmd("rig.auto_fix", json::object! {}, "Ran 1-click Auto-Fix");
				}
			});
		});
	}

	fn draw_motion_curves(&mut self, ui: &mut egui::Ui, dock_rect: Rect, left_w: f32) {
		let left_rect = Rect::from_min_max(dock_rect.min, pos2((dock_rect.left() + left_w).min(dock_rect.right()), dock_rect.bottom()));
		let right_rect = Rect::from_min_max(pos2(left_rect.right() + 1.0, dock_rect.top()), dock_rect.max);

		let uuid_to_name: HashMap<_, _> = self
			.session
			.model
			.puppet
			.params
			.iter()
			.map(|(n, p)| (p.uuid, n.clone()))
			.collect();

		let mut lane_entries: Vec<(String, u8, usize)> = Vec::new();
		if let Some(anim) = self.session.model.puppet.animations.get("Idle") {
			for lane in &anim.lanes {
				if let Some(pname) = uuid_to_name.get(&lane.param_uuid) {
					lane_entries.push((pname.clone(), lane.target, lane.keyframes.len()));
				}
			}
		}

		// Left lane selector
		let mut left_ui = ui.child_ui(left_rect, egui::Layout::top_down(egui::Align::Min), None);
		egui::ScrollArea::vertical().id_source("curves_lanes_scroll").show(&mut left_ui, |ui| {
			for (pname, target, kcount) in &lane_entries {
				let axis = if *target == 1 { "Y" } else { "X" };
				let is_sel = self.selected_curve_lane.as_ref() == Some(&(pname.clone(), *target));
				let row_rect = ui.allocate_space(vec2(left_w - 8.0, 22.0)).1;
				let resp = ui.interact(row_rect, ui.id().with(("clane", pname.as_str(), *target)), Sense::click());
				if is_sel {
					ui.painter().rect_filled(row_rect, 2.0, ROW_SELECTED);
				} else if resp.hovered() {
					ui.painter().rect_filled(row_rect, 2.0, Color32::from_rgb(42, 42, 48));
				}
				ui.painter().text(
					pos2(row_rect.left() + 8.0, row_rect.center().y),
					Align2::LEFT_CENTER,
					format!("{pname} .{axis}"),
					egui::FontId::monospace(10.5),
					if is_sel { Color32::WHITE } else { TEXT_PRIMARY },
				);
				ui.painter().text(
					pos2(row_rect.right() - 8.0, row_rect.center().y),
					Align2::RIGHT_CENTER,
					format!("{kcount}k"),
					egui::FontId::monospace(9.5),
					TEXT_MUTED,
				);
				if resp.clicked() {
					self.selected_curve_lane = Some((pname.clone(), *target));
					self.selected_keyframe = None;
				}
			}
		});

		if right_rect.width() <= 60.0 {
			return;
		}
		let painter = ui.painter_at(right_rect);
		painter.rect_filled(right_rect, 0.0, BG_INPUT);

		let (sel_pname, sel_target) = self
			.selected_curve_lane
			.clone()
			.or_else(|| lane_entries.first().map(|e| (e.0.clone(), e.1)))
			.unwrap_or_else(|| ("Body:: Breath".to_owned(), 0));

		// Inline Tension & Interpolation Bar at top of graph
		let ctrl_h = 24.0;
		let ctrl_rect = Rect::from_min_max(right_rect.min, pos2(right_rect.right(), right_rect.top() + ctrl_h));
		painter.rect_filled(ctrl_rect, 0.0, BG_DOCK);
		painter.line_segment([ctrl_rect.left_bottom(), ctrl_rect.right_bottom()], Stroke::new(1.0_f32, BORDER));

		let mut ctrl_ui = ui.child_ui(ctrl_rect.shrink2(vec2(6.0, 2.0)), egui::Layout::left_to_right(egui::Align::Center), None);
		let anim_len = self
			.session
			.model
			.puppet
			.animations
			.get("Idle")
			.map(|a| a.length.max(30))
			.unwrap_or(120);

		let pbounds = self
			.session
			.model
			.puppet
			.params
			.get(&sel_pname)
			.map(|p| {
				if sel_target == 1 {
					(p.min.y, p.max.y)
				} else {
					(p.min.x, p.max.x)
				}
			})
			.unwrap_or((-1.0, 1.0));
		let vmin = pbounds.0;
		let vmax = if (pbounds.1 - pbounds.0).abs() < 1e-4 {
			pbounds.0 + 1.0
		} else {
			pbounds.1
		};

		let sel_uuid = self.session.model.puppet.params.get(&sel_pname).map(|p| p.uuid);
		let mut cur_tension = 0.5_f32;
		let mut cur_interp = InterpolateMode::Linear;
		if let (Some(puuid), Some(anim)) = (sel_uuid, self.session.model.puppet.animations.get("Idle")) {
			if let Some(lane) = anim.lanes.iter().find(|l| l.param_uuid == puuid && l.target == sel_target) {
				cur_interp = lane.interpolation.clone();
				if let Some((_, _, kf_f)) = self.selected_keyframe {
					if let Some(k) = lane.keyframes.iter().find(|k| k.frame == kf_f) {
						cur_tension = k.tension;
					}
				}
			}
		}

		ctrl_ui.label(egui::RichText::new(format!("{sel_pname} .{}", if sel_target == 1 { "Y" } else { "X" })).monospace().size(10.5).color(ACCENT_BLUE));
		ctrl_ui.separator();
		if let Some((ref kp_name, kp_t, kp_f)) = self.selected_keyframe.clone() {
			if kp_name == &sel_pname && kp_t == sel_target {
				let mut t_val = cur_tension;
				if ctrl_ui.add(egui::Slider::new(&mut t_val, 0.0..=1.0).text("Tension")).changed() {
					if let (Some(puuid), Some(anim)) = (sel_uuid, self.session.model.puppet.animations.get_mut("Idle")) {
						if let Some(lane) = anim.lanes.iter_mut().find(|l| l.param_uuid == puuid && l.target == sel_target) {
							if let Some(k) = lane.keyframes.iter_mut().find(|k| k.frame == kp_f) {
								k.tension = t_val;
								self.session.revision += 1;
							}
						}
					}
				}
			}
		}
		if ctrl_ui
			.small_button(match cur_interp {
				InterpolateMode::Linear => "Mode: Smooth/Linear",
				InterpolateMode::Nearest => "Mode: Stepped",
			})
			.clicked()
		{
			let next_mode = match cur_interp {
				InterpolateMode::Linear => InterpolateMode::Nearest,
				InterpolateMode::Nearest => InterpolateMode::Linear,
			};
			self.begin_gesture_undo();
			if let (Some(puuid), Some(anim)) = (sel_uuid, self.session.model.puppet.animations.get_mut("Idle")) {
				if let Some(lane) = anim.lanes.iter_mut().find(|l| l.param_uuid == puuid && l.target == sel_target) {
					lane.interpolation = next_mode;
				}
			}
			self.finish_gesture_undo();
		}

		// Graph Canvas below ctrl bar
		let graph_rect = Rect::from_min_max(pos2(right_rect.left() + 8.0, ctrl_rect.bottom() + 6.0), right_rect.max - vec2(12.0, 10.0));
		if graph_rect.height() < 30.0 {
			return;
		}
		let val_to_y = |v: f32| -> f32 {
			let t = ((v - vmin) / (vmax - vmin)).clamp(0.0, 1.0);
			graph_rect.bottom() - t * graph_rect.height()
		};
		let y_to_val = |y: f32| -> f32 {
			let t = (1.0 - (y - graph_rect.top()) / graph_rect.height().max(1.0)).clamp(0.0, 1.0);
			vmin + t * (vmax - vmin)
		};
		let f_to_x = |f: f32| -> f32 { graph_rect.left() + (f / anim_len as f32) * graph_rect.width() };
		let x_to_f = |x: f32| -> i32 {
			(((x - graph_rect.left()) / graph_rect.width().max(1.0)) * anim_len as f32)
				.round()
				.clamp(0.0, anim_len as f32) as i32
		};

		// Zero line + bounds
		if vmin < 0.0 && vmax > 0.0 {
			let zy = val_to_y(0.0);
			painter.line_segment(
				[pos2(graph_rect.left(), zy), pos2(graph_rect.right(), zy)],
				Stroke::new(1.0_f32, Color32::from_rgb(55, 58, 68)),
			);
		}
		// Playhead vertical line
		let phx = f_to_x(self.playhead_frame);
		painter.line_segment([pos2(phx, graph_rect.top()), pos2(phx, graph_rect.bottom())], Stroke::new(1.2_f32, PLAYHEAD_RED));

		let kfs = self
			.session
			.model
			.puppet
			.animations
			.get("Idle")
			.and_then(|a| {
				let puuid = sel_uuid?;
				a.lanes
					.iter()
					.find(|l| l.param_uuid == puuid && l.target == sel_target)
					.map(|l| {
						let mut poly = Vec::new();
						for f in (0..=anim_len).step_by(2) {
							if let Some(val) = l.eval_at(f as f32) {
								poly.push(pos2(f_to_x(f as f32), val_to_y(val)));
							}
						}
						(poly, l.keyframes.clone())
					})
			})
			.unwrap_or_default();

		if kfs.0.len() >= 2 {
			painter.add(egui::Shape::line(kfs.0, Stroke::new(1.8_f32, ACCENT_BLUE)));
		}

		for kf in &kfs.1 {
			let p = pos2(f_to_x(kf.frame as f32), val_to_y(kf.value));
			let is_sel = self.selected_keyframe.as_ref() == Some(&(sel_pname.clone(), sel_target, kf.frame));
			painter.circle_filled(p, 5.0, if is_sel { AUDIT_WARN_AMBER } else { Color32::WHITE });
			painter.circle_stroke(p, 5.0, Stroke::new(1.3_f32, TIMELINE_VIOLET));
		}

		// Graph interactions (drag keyframes or double-click to insert)
		let g_resp = ui.interact(graph_rect, ui.id().with("motion_graph_canvas"), Sense::click_and_drag());
		if g_resp.double_clicked() {
			if let Some(ptr) = g_resp.interact_pointer_pos() {
				let nf = x_to_f(ptr.x);
				let nv = y_to_val(ptr.y);
				self.exec_cmd(
					"anim.set_keyframe",
					json::object! { anim: "Idle", param: sel_pname.as_str(), target: sel_target, frame: nf, value: nv, tension: 0.5 },
					&format!("Added keyframe ({nf}, {nv:.2})"),
				);
				self.selected_keyframe = Some((sel_pname.clone(), sel_target, nf));
			}
		}
		if g_resp.drag_started() {
			if let Some(ptr) = g_resp.interact_pointer_pos() {
				if let Some(kf) = kfs
					.1
					.iter()
					.find(|k| pos2(f_to_x(k.frame as f32), val_to_y(k.value)).distance(ptr) < 11.0)
				{
					self.begin_gesture_undo();
					self.selected_keyframe = Some((sel_pname.clone(), sel_target, kf.frame));
					self.curve_drag = Some(CurveDragState {
						param_name: sel_pname.clone(),
						target: sel_target,
						frame: kf.frame,
					});
				}
			}
		}
		if g_resp.dragged() {
			if let (Some(ref mut cd), Some(ptr)) = (&mut self.curve_drag, g_resp.interact_pointer_pos()) {
				let nf = x_to_f(ptr.x);
				let nv = y_to_val(ptr.y);
				if let (Some(puuid), Some(anim)) = (
					self.session.model.puppet.params.get(&cd.param_name).map(|p| p.uuid),
					self.session.model.puppet.animations.get_mut("Idle"),
				) {
					if let Some(lane) = anim
						.lanes
						.iter_mut()
						.find(|l| l.param_uuid == puuid && l.target == cd.target)
					{
						if let Some(k) = lane.keyframes.iter_mut().find(|k| k.frame == cd.frame) {
							k.frame = nf;
							k.value = nv;
							cd.frame = nf;
							lane.keyframes.sort_by_key(|k| k.frame);
							self.selected_keyframe = Some((cd.param_name.clone(), cd.target, nf));
							self.playhead_frame = nf as f32;
							self.apply_anim_at_playhead();
							self.session.revision += 1;
							self.cached_rev = u64::MAX;
						}
					}
				}
			}
		}
		if g_resp.drag_stopped() && self.curve_drag.take().is_some() {
			self.finish_gesture_undo();
		}
	}

	fn draw_bottom_dock(&mut self, ui: &mut egui::Ui) {
		ui.horizontal(|ui| {
			let t_btn = egui::SelectableLabel::new(self.bottom_tab == BottomTab::Timeline, "Timeline");
			if ui.add(t_btn).clicked() {
				self.bottom_tab = BottomTab::Timeline;
			}
			let m_btn = egui::SelectableLabel::new(self.bottom_tab == BottomTab::MotionCurves, "Motion Curves");
			if ui.add(m_btn).clicked() {
				self.bottom_tab = BottomTab::MotionCurves;
			}

			ui.separator();
			if ui.small_button("<<").clicked() {
				self.playing = false;
				self.playhead_frame = 1.0;
				self.apply_anim_at_playhead();
			}
			if ui.small_button("<").clicked() {
				self.playing = false;
				self.playhead_frame = (self.playhead_frame - 1.0).max(1.0);
				self.apply_anim_at_playhead();
			}
			let (play_rect, play_resp) = ui.allocate_exact_size(vec2(24.0, 20.0), Sense::click());
			if play_resp.hovered() {
				ui.painter().rect_filled(play_rect, 3.0, BG_WIDGET);
			}
			draw_transport_play(ui.painter(), play_rect, self.playing, TEXT_PRIMARY);
			if play_resp.clicked() {
				self.playing = !self.playing;
			}
			if ui.small_button(">").clicked() {
				self.playing = false;
				self.playhead_frame += 1.0;
				self.apply_anim_at_playhead();
			}
			ui.checkbox(&mut self.loop_anim, "Loop");

			ui.label(
				egui::RichText::new(format!(
					"Frame {:>3}  30 fps  {:.2} s",
					self.playhead_frame.round() as i32,
					(self.playhead_frame - 1.0) / 30.0
				))
				.monospace()
				.size(11.0)
				.color(TEXT_MUTED),
			);
			if ui.small_button("+ Key (F6)").clicked() {
				self.insert_keyframe_at_playhead();
			}
			if self.selected_keyframe.is_some() && ui.small_button("Del Key").clicked() {
				self.remove_selected_keyframe();
			}

			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				if let Some(id) = self.selected_node {
					if ui.small_button("Delete Node").clicked() {
						self.exec_cmd("node.remove", json::object! { node_uuid: id.0 }, "Removed selected node");
						self.selected_node = None;
						self.selected_vertices.clear();
					}
				}
				if ui.small_button("+ MeshGroup").clicked() {
					if let Some(id) = self.selected_node {
						let n = self
							.session
							.model
							.puppet
							.nodes
							.get_node(id)
							.map(|n| n.name.clone())
							.unwrap_or_else(|| "Part".to_owned());
						self.exec_cmd(
							"node.add_meshgroup",
							json::object! {
								name: format!("{n}_Cage"),
								children: [n.as_str()],
								cols: 4,
								rows: 4
							},
							"Created MeshGroup deform cage",
						);
					}
				}
				if ui.small_button("+ Part").clicked() {
					let pname = format!("Part_{}", self.session.model.puppet.nodes.iter().count());
					self.exec_cmd(
						"node.add_part",
						json::object! { name: pname.as_str(), width: 80.0, height: 80.0, color: [160, 200, 255, 255] },
						"Added new Part node",
					);
				}
			});
		});

		ui.separator();

		let dock_rect = ui.available_rect_before_wrap();
		let left_w = 236.0;
		let left_rect = Rect::from_min_max(dock_rect.min, pos2((dock_rect.left() + left_w).min(dock_rect.right()), dock_rect.bottom()));
		let right_rect = Rect::from_min_max(pos2(left_rect.right() + 1.0, dock_rect.top()), dock_rect.max);

		ui.painter().line_segment(
			[pos2(left_rect.right(), dock_rect.top()), pos2(left_rect.right(), dock_rect.bottom())],
			Stroke::new(1.0_f32, BORDER),
		);

		if self.bottom_tab == BottomTab::MotionCurves {
			self.draw_motion_curves(ui, dock_rect, left_w);
			return;
		}

		let nodes_list: Vec<_> = self
			.session
			.model
			.puppet
			.nodes
			.iter()
			.filter(|n| n.uuid != self.session.model.puppet.nodes.root_node_id)
			.map(|n| {
				let is_part = self.session.model.puppet.node_comps.get::<TexturedMesh>(n.uuid).is_some();
				let is_mg = self.session.model.puppet.node_comps.get::<MeshGroup>(n.uuid).is_some();
				let has_mask = self
					.session
					.model
					.puppet
					.node_comps
					.get::<Drawable>(n.uuid)
					.and_then(|d| d.masks.as_ref())
					.map(|m| !m.masks.is_empty())
					.unwrap_or(false);
				let has_phys = self.session.model.puppet.node_comps.get::<SimplePhysics>(n.uuid).is_some();
				(n.uuid, n.name.clone(), n.enabled, n.zsort, is_part, is_mg, has_mask, has_phys)
			})
			.collect();

		let anim_len = self
			.session
			.model
			.puppet
			.animations
			.get("Idle")
			.map(|a| a.length.max(30))
			.unwrap_or(120);

		let row_h = 22.0;

		let mut left_ui = ui.child_ui(left_rect, egui::Layout::top_down(egui::Align::Min), None);
		egui::ScrollArea::vertical().id_source("layers_scroll").show(&mut left_ui, |ui| {
			for (idx, (uid, name, enabled, zsort, is_part, is_mg, has_mask, has_phys)) in nodes_list.iter().enumerate() {
				let is_sel = self.selected_node == Some(*uid);
				let tag_col = if *has_phys {
					TIMELINE_EMERALD
				} else if *is_mg {
					TIMELINE_VIOLET
				} else if *has_mask {
					AUDIT_WARN_AMBER
				} else if idx % 2 == 0 {
					ACCENT_BLUE
				} else {
					AUDIT_ERR_ROSE
				};

				let row_rect = ui.allocate_space(vec2(left_w - 8.0, row_h)).1;
				let resp = ui.interact(row_rect, ui.id().with(("layer", uid.0)), Sense::click());
				if is_sel {
					ui.painter().rect_filled(row_rect, 2.0, ROW_SELECTED);
				} else if resp.hovered() {
					ui.painter().rect_filled(row_rect, 2.0, Color32::from_rgb(42, 42, 48));
				}

				let mut icon_lbl = if *is_mg {
					"[MG] "
				} else if *has_phys {
					"[PH] "
				} else if *is_part {
					""
				} else {
					"[G] "
				};
				if *has_mask {
					icon_lbl = "[M] ";
				}
				let eye_center = pos2(row_rect.left() + 12.0, row_rect.center().y);
				if *enabled {
					ui.painter().circle_filled(eye_center, 3.2, if is_sel { Color32::WHITE } else { AUDIT_OK_GREEN });
				} else {
					ui.painter().circle_stroke(eye_center, 3.2, Stroke::new(1.1_f32, TEXT_MUTED));
				}
				ui.painter().text(
					pos2(row_rect.left() + 21.0, row_rect.center().y),
					Align2::LEFT_CENTER,
					format!("{icon_lbl}{name}"),
					egui::FontId::proportional(11.0),
					if is_sel { Color32::WHITE } else { TEXT_PRIMARY },
				);
				ui.painter().text(
					pos2(row_rect.right() - 22.0, row_rect.center().y),
					Align2::RIGHT_CENTER,
					format!("z{:.0}", zsort),
					egui::FontId::monospace(9.5),
					TEXT_MUTED,
				);
				let tag_rect = Rect::from_center_size(pos2(row_rect.right() - 10.0, row_rect.center().y), vec2(8.0, 8.0));
				ui.painter().rect_filled(tag_rect, 1.5, tag_col);

				if resp.clicked() {
					self.selected_node = Some(*uid);
					self.selected_vertices.clear();
					if *has_phys {
						self.selected_physics = Some(*uid);
					}
				}
				if resp.double_clicked() {
					let next_en = !*enabled;
					self.exec_cmd("node.configure", json::object! { node_uuid: uid.0, enabled: next_en }, "Toggled node visibility");
				}
			}
		});

		if right_rect.width() > 40.0 {
			let tr_painter = ui.painter_at(right_rect);
			tr_painter.rect_filled(right_rect, 0.0, BG_INPUT);
			let ruler_h = 20.0;
			let px_per_frame = 6.5;

			let ruler_rect = Rect::from_min_max(right_rect.min, pos2(right_rect.right(), right_rect.top() + ruler_h));
			tr_painter.rect_filled(ruler_rect, 0.0, BG_DOCK);
			tr_painter.line_segment([ruler_rect.left_bottom(), ruler_rect.right_bottom()], Stroke::new(1.0_f32, BORDER));

			let ruler_resp = ui.interact(ruler_rect, ui.id().with("timeline_ruler"), Sense::click_and_drag());
			if ruler_resp.clicked() || ruler_resp.dragged() {
				if let Some(p) = ruler_resp.interact_pointer_pos() {
					let f = (((p.x - right_rect.left()) / px_per_frame) + 1.0).clamp(1.0, anim_len as f32);
					self.playing = false;
					self.playhead_frame = f;
					self.apply_anim_at_playhead();
				}
			}

			for tick in (0..=160).step_by(5) {
				let f = if tick == 0 { 1 } else { tick };
				let x = right_rect.left() + (f - 1) as f32 * px_per_frame + 12.0;
				if x > right_rect.right() - 14.0 {
					break;
				}
				tr_painter.line_segment(
					[pos2(x, ruler_rect.bottom()), pos2(x, right_rect.bottom())],
					Stroke::new(1.0_f32, Color32::from_rgb(28, 28, 34)),
				);
				if f == 1 || f % 10 == 0 {
					tr_painter.text(
						pos2(x, ruler_rect.center().y),
						Align2::CENTER_CENTER,
						format!("{f}"),
						egui::FontId::monospace(9.5),
						TEXT_MUTED,
					);
				}
			}

			let uuid_to_name: HashMap<_, _> = self
				.session
				.model
				.puppet
				.params
				.iter()
				.map(|(n, p)| (p.uuid, n.clone()))
				.collect();
			let anim_opt = self.session.model.puppet.animations.get("Idle").cloned();
			let span_w = (anim_len as f32 * px_per_frame).min(right_rect.width() - 24.0).max(40.0);
			let mut kf_hits: Vec<(egui::Pos2, String, u8, i32)> = Vec::new();

			for (idx, (uid, name, _, _, _, _, _, _)) in nodes_list.iter().enumerate() {
				let y = right_rect.top() + ruler_h + idx as f32 * (row_h + 2.0) + 3.0;
				if y + row_h > right_rect.bottom() {
					break;
				}
				let bound_params: Vec<_> = self
					.session
					.model
					.puppet
					.params
					.values()
					.filter(|p| p.bindings.iter().any(|b| b.node == *uid))
					.map(|p| p.uuid)
					.collect();
				let bar_col = if !bound_params.is_empty() || name.contains("Head") || name.contains("Eye") {
					TIMELINE_VIOLET
				} else {
					TIMELINE_EMERALD
				};
				let bar_rect = Rect::from_min_size(pos2(right_rect.left() + 12.0, y), vec2(span_w, row_h - 4.0));
				tr_painter.rect_filled(bar_rect, 2.0, bar_col);

				if let Some(ref anim) = anim_opt {
					if let Some(lane) = anim.lanes.iter().find(|l| bound_params.contains(&l.param_uuid)) {
						let pname = uuid_to_name.get(&lane.param_uuid).cloned().unwrap_or_default();
						for kf in &lane.keyframes {
							let kx = right_rect.left() + (kf.frame.max(1) - 1) as f32 * px_per_frame + 12.0;
							if kx > bar_rect.right() - 2.0 {
								continue;
							}
							let kc = pos2(kx, bar_rect.center().y);
							kf_hits.push((kc, pname.clone(), lane.target, kf.frame));
							let is_kf_sel = self.selected_keyframe.as_ref() == Some(&(pname.clone(), lane.target, kf.frame));
							let d = if is_kf_sel { 4.8 } else { 3.6 };
							let diamond = vec![kc + vec2(0.0, -d), kc + vec2(d, 0.0), kc + vec2(0.0, d), kc + vec2(-d, 0.0)];
							tr_painter.add(egui::Shape::convex_polygon(
								diamond,
								if is_kf_sel { AUDIT_WARN_AMBER } else { Color32::WHITE },
								Stroke::new(1.0_f32, BG_INPUT),
							));
						}
					} else {
						tr_painter.circle_filled(pos2(bar_rect.left() + 6.0, bar_rect.center().y), 3.0, BG_INPUT);
					}
				}
			}

			// Interactive keyframe selection & horizontal drag on Timeline tracks
			let tracks_rect = Rect::from_min_max(pos2(right_rect.left(), ruler_rect.bottom()), right_rect.max);
			let tr_resp = ui.interact(tracks_rect, ui.id().with("timeline_tracks_drag"), Sense::click_and_drag());
			if tr_resp.clicked() {
				if let Some(ptr) = tr_resp.interact_pointer_pos() {
					if let Some((_, pname, tgt, f)) = kf_hits.iter().find(|(kc, _, _, _)| kc.distance(ptr) < 10.0) {
						self.selected_curve_lane = Some((pname.clone(), *tgt));
						self.selected_keyframe = Some((pname.clone(), *tgt, *f));
						self.playhead_frame = (*f).max(1) as f32;
						self.apply_anim_at_playhead();
					}
				}
			}
			if tr_resp.drag_started() {
				if let Some(ptr) = tr_resp.interact_pointer_pos() {
					if let Some((_, pname, tgt, f)) = kf_hits.iter().find(|(kc, _, _, _)| kc.distance(ptr) < 11.0) {
						self.begin_gesture_undo();
						self.selected_curve_lane = Some((pname.clone(), *tgt));
						self.selected_keyframe = Some((pname.clone(), *tgt, *f));
						self.curve_drag = Some(CurveDragState {
							param_name: pname.clone(),
							target: *tgt,
							frame: *f,
						});
					}
				}
			}
			if tr_resp.dragged() {
				if let (Some(ref mut cd), Some(ptr)) = (&mut self.curve_drag, tr_resp.interact_pointer_pos()) {
					let nf = (((ptr.x - right_rect.left() - 12.0) / px_per_frame) + 1.0)
						.round()
						.clamp(1.0, anim_len as f32) as i32;
					if let (Some(puuid), Some(anim)) = (
						self.session.model.puppet.params.get(&cd.param_name).map(|p| p.uuid),
						self.session.model.puppet.animations.get_mut("Idle"),
					) {
						if let Some(lane) = anim
							.lanes
							.iter_mut()
							.find(|l| l.param_uuid == puuid && l.target == cd.target)
						{
							if let Some(k) = lane.keyframes.iter_mut().find(|k| k.frame == cd.frame) {
								k.frame = nf;
								cd.frame = nf;
								lane.keyframes.sort_by_key(|k| k.frame);
								self.selected_keyframe = Some((cd.param_name.clone(), cd.target, nf));
								self.playhead_frame = nf as f32;
								self.apply_anim_at_playhead();
								self.session.revision += 1;
								self.cached_rev = u64::MAX;
							}
						}
					}
				}
			}
			if tr_resp.drag_stopped() && self.curve_drag.take().is_some() {
				self.finish_gesture_undo();
			}

			let px = right_rect.left() + (self.playhead_frame - 1.0) * px_per_frame + 12.0;
			let badge_r = Rect::from_center_size(pos2(px, ruler_rect.center().y), vec2(18.0, 15.0));
			tr_painter.rect_filled(badge_r, 2.0, PLAYHEAD_RED);
			tr_painter.text(
				badge_r.center(),
				Align2::CENTER_CENTER,
				format!("{}", self.playhead_frame.round() as i32),
				egui::FontId::monospace(9.5),
				Color32::WHITE,
			);
			tr_painter.line_segment(
				[pos2(px, badge_r.bottom()), pos2(px, right_rect.bottom())],
				Stroke::new(1.5_f32, PLAYHEAD_RED),
			);
		}
	}

	fn draw_command_palette(&mut self, ctx: &egui::Context) {
		if !self.cmd_palette_open {
			return;
		}
		let mut open = self.cmd_palette_open;
		egui::Window::new("Command Palette (Ctrl+K)")
			.open(&mut open)
			.collapsible(false)
			.resizable(false)
			.fixed_size([500.0, 320.0])
			.anchor(Align2::CENTER_CENTER, Vec2::ZERO)
			.show(ctx, |ui| {
				ui.horizontal(|ui| {
					ui.label("Search:");
					let r = ui.text_edit_singleline(&mut self.cmd_query);
					r.request_focus();
				});
				ui.separator();
				let q = self.cmd_query.to_ascii_lowercase();
				let specs = command_registry();
				egui::ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
					for spec in specs {
						if !q.is_empty()
							&& !spec.id.to_ascii_lowercase().contains(&q)
							&& !spec.description.to_ascii_lowercase().contains(&q)
						{
							continue;
						}
						let clicked = ui
							.horizontal(|ui| {
								let b = ui.button(egui::RichText::new(spec.id).monospace().color(ACCENT_BLUE));
								ui.label(egui::RichText::new(spec.description).size(11.0).color(TEXT_MUTED));
								b.clicked()
							})
							.inner;
						if clicked {
							match spec.id {
								"rig.auto_rig_vtuber"
								| "rig.auto_fix"
								| "rig.audit"
								| "anim.create_idle"
								| "edit.undo"
								| "edit.redo"
								| "puppet.new_sample" => {
									self.exec_cmd(spec.id, json::object! {}, &format!("Executed {}", spec.id));
								}
								"craft.figcraft_card" => {
									self.exec_cmd(
										"craft.figcraft_card",
										json::object! { out: "studio_spec_card.png" },
										"Exported FigCraft spec card",
									);
								}
								_ => {
									self.set_status(true, format!("{} schema: {}", spec.id, spec.params_schema));
								}
							}
							self.cmd_palette_open = false;
						}
					}
				});
			});
		self.cmd_palette_open = open;
	}
}

impl eframe::App for Inox2dStudioApp {
	fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
		self.frame_counter += 1;
		self.poll_watch_in();
		self.handle_shortcuts(ctx);

		if self.playing {
			let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
			let anim_len = self
				.session
				.model
				.puppet
				.animations
				.get("Idle")
				.map(|a| a.length.max(1) as f32)
				.unwrap_or(120.0);
			self.playhead_frame += dt * 30.0;
			if self.playhead_frame > anim_len {
				if self.loop_anim {
					self.playhead_frame = 1.0 + (self.playhead_frame - 1.0).rem_euclid(anim_len);
				} else {
					self.playhead_frame = anim_len;
					self.playing = false;
				}
			}
			self.apply_anim_at_playhead();
			ctx.request_repaint();
		}
		self.last_tick = Instant::now();

		self.refresh_audit();
		self.prepare_stage_frame(ctx);

		if let Some(ref out_path) = self.screenshot_out.clone() {
			if self.frame_counter == 4 && !self.screenshot_sent {
				self.screenshot_sent = true;
				ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
			}
			let captured = ctx.input(|i| {
				i.events.iter().find_map(|ev| match ev {
					egui::Event::Screenshot { image, .. } => Some(image.clone()),
					_ => None,
				})
			});
			if let Some(img) = captured {
				if let Err(err) = save_color_image_png(&img, out_path) {
					eprintln!("Error saving screenshot: {err}");
				}
				ctx.send_viewport_cmd(egui::ViewportCommand::Close);
			}
		}

		// 1. Top Menu Bar
		egui::TopBottomPanel::top("top_menu_bar")
			.exact_height(26.0)
			.frame(Frame::none().fill(BG_MENU).inner_margin(Margin::symmetric(8.0, 3.0)))
			.show(ctx, |ui| self.draw_menu_bar(ui));

		// 2. Dynamic Context Bar
		egui::TopBottomPanel::top("context_bar")
			.exact_height(30.0)
			.frame(
				Frame::none()
					.fill(Color32::from_rgb(26, 26, 30))
					.stroke(Stroke::new(1.0_f32, BORDER))
					.inner_margin(Margin::symmetric(8.0, 4.0)),
			)
			.show(ctx, |ui| self.draw_context_bar(ui));

		// 3. Status Bar
		egui::TopBottomPanel::bottom("status_bar")
			.exact_height(22.0)
			.frame(
				Frame::none()
					.fill(BG_MENU)
					.stroke(Stroke::new(1.0_f32, BORDER))
					.inner_margin(Margin::symmetric(10.0, 2.0)),
			)
			.show(ctx, |ui| {
				ui.horizontal(|ui| {
					let scol = if self.status_ok { AUDIT_OK_GREEN } else { AUDIT_ERR_ROSE };
					ui.label(egui::RichText::new(&self.status_msg).color(scol).size(11.0));

					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						let parts = self
							.session
							.model
							.puppet
							.nodes
							.iter()
							.filter(|n| self.session.model.puppet.node_comps.get::<TexturedMesh>(n.uuid).is_some())
							.count();
						let tris: usize = self
							.session
							.model
							.puppet
							.nodes
							.iter()
							.filter_map(|n| self.session.model.puppet.node_comps.get::<Mesh>(n.uuid))
							.map(|m| m.indices.len() / 3)
							.sum();
						let score = self.audit_cache["score"].as_u32().unwrap_or(0);
						ui.label(
							egui::RichText::new(format!(
								"render {:.1} ms  |  {} parts  |  {} tris  |  audit {}/100  |  undo {}  |  rev {}",
								self.render_ms,
								parts,
								tris,
								score,
								self.session.undo_stack.len(),
								self.session.revision
							))
							.monospace()
							.size(10.5)
							.color(TEXT_MUTED),
						);
					});
				});
			});

		// 4. Right Inspector Dock
		egui::SidePanel::right("right_inspector")
			.exact_width(328.0)
			.resizable(false)
			.frame(
				Frame::none()
					.fill(BG_DOCK)
					.stroke(Stroke::new(1.0_f32, BORDER))
					.inner_margin(Margin::symmetric(8.0, 4.0)),
			)
			.show(ctx, |ui| self.draw_right_inspector(ui));

		// 5. Left Ergonomic Grouped Tool Rail
		egui::SidePanel::left("left_tool_rail")
			.exact_width(38.0)
			.resizable(false)
			.frame(
				Frame::none()
					.fill(BG_DOCK)
					.stroke(Stroke::new(1.0_f32, BORDER))
					.inner_margin(Margin::symmetric(4.0, 6.0)),
			)
			.show(ctx, |ui| {
				ui.vertical_centered(|ui| {
					let groups = StudioTool::groups();
					for (gi, group) in groups.iter().enumerate() {
						for &t in *group {
							if draw_tool_button(ui, t, self.tool == t) {
								match t {
									StudioTool::ZoomFit => {
										self.fit_puppet_scale();
										self.cached_rev = u64::MAX;
										self.set_status(true, "Reset & fit puppet to stage");
									}
									StudioTool::Delaunay => {
										self.tool = t;
										if let Some(id) = self.selected_node {
											self.exec_cmd(
												"mesh.auto_delaunay",
												json::object! { node_uuid: id.0, step_px: 20.0 },
												"Applied Bowyer-Watson Delaunay mesh to selected node",
											);
										}
									}
									StudioTool::MeshGroup => {
										self.tool = t;
										if let Some(id) = self.selected_node {
											let name = self
												.session
												.model
												.puppet
												.nodes
												.get_node(id)
												.map(|n| n.name.clone())
												.unwrap_or_default();
											if !name.is_empty() {
												self.exec_cmd(
													"node.add_meshgroup",
													json::object! { name: format!("{name}_Cage"), children: [name.as_str()], cols: 4, rows: 4 },
													"Wrapped selected Part inside MeshGroup cage",
												);
											}
										}
									}
									StudioTool::Physics => {
										self.tool = t;
										let has_any_phys = self
											.session
											.model
											.puppet
											.nodes
											.iter()
											.any(|n| self.session.model.puppet.node_comps.get::<SimplePhysics>(n.uuid).is_some());
										if !has_any_phys {
											if let Some(id) = self.selected_node {
												let name = self
													.session
													.model
													.puppet
													.nodes
													.get_node(id)
													.map(|n| n.name.clone())
													.unwrap_or_else(|| "Part".to_owned());
												self.exec_cmd(
													"physics.add_pendulum",
													json::object! { name: format!("{name}_Pendulum"), parent_uuid: id.0, param: "Hair:: Sway" },
													"Attached SimplePhysics pendulum driver",
												);
											}
										} else {
											self.set_status(true, "Physics Gizmo active: drag Bob ● on stage to tune pendulum length");
										}
									}
									_ => {
										self.tool = t;
										self.set_status(true, t.tooltip());
									}
								}
							}
							ui.add_space(2.0);
						}
						if gi + 1 < groups.len() {
							ui.add_space(2.0);
							ui.separator();
							ui.add_space(2.0);
						}
					}
				});
			});

		// 6. Bottom Timeline & Motion Curves Dock
		egui::TopBottomPanel::bottom("bottom_timeline_dock")
			.exact_height(215.0)
			.frame(
				Frame::none()
					.fill(BG_DOCK)
					.stroke(Stroke::new(1.0_f32, BORDER))
					.inner_margin(Margin::symmetric(8.0, 4.0)),
			)
			.show(ctx, |ui| self.draw_bottom_dock(ui));

		// 7. Center Stage
		egui::CentralPanel::default()
			.frame(Frame::none().fill(BG_STAGE))
			.show(ctx, |ui| self.draw_stage(ui));

		self.draw_command_palette(ctx);
	}
}

fn main() -> eframe::Result<()> {
	let cli = Cli::parse();
	let input_arg = if cli.sample { "--sample" } else { cli.input.as_str() };
	let session = load_initial_session(input_arg).unwrap_or_else(|err| {
		eprintln!("Error loading '{input_arg}': {err}");
		std::process::exit(1);
	});
	let source_path = if input_arg == "--sample" || input_arg == "sample" {
		None
	} else {
		Some(PathBuf::from(input_arg))
	};

	if let Some(ref wpath) = cli.watch {
		if !wpath.exists() {
			let _ = write_inp_atomic(&session.model, wpath);
		}
	}

	let native_options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default()
			.with_inner_size([1280.0, 760.0])
			.with_min_inner_size([1000.0, 620.0])
			.with_title("Inox2D Studio — Craft-Style VTuber Rigger"),
		..Default::default()
	};

	eframe::run_native(
		"Inox2D Studio",
		native_options,
		Box::new(move |cc| {
			configure_craft_theme(&cc.egui_ctx);
			Ok(Box::new(Inox2dStudioApp::new(
				session,
				source_path,
				cli.watch,
				cli.screenshot,
			)))
		}),
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn sample_studio_session_loads_cleanly() {
		let mut s = load_initial_session("--sample").expect("load sample studio session");
		assert!(s.model.puppet.params.contains_key("Mouth:: Viseme"));
		let rep = audit_rig(&mut s.model);
		assert!(rep["score"].as_u32().unwrap_or(0) >= 80);
	}
}

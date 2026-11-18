#![forbid(unsafe_code)]

mod icons;
mod theme;

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use clap::Parser;
use eframe::egui::{self, pos2, vec2, Align2, Color32, Frame, Key, Margin, Rect, Rounding, Sense, Stroke, Vec2};
use glam::{vec2 as gvec2, vec3 as gvec3, Mat4, Vec2 as GVec2, Vec4};
use image::RgbaImage;
use inox2d::formats::inp::{parse_inp, write_inp};
use inox2d::math::camera::Camera;
use inox2d::model::Model;
use inox2d::node::components::{Drawable, Mesh, MeshGroup, SimplePhysics, TexturedMesh, TransformStore};
use inox2d::node::InoxNodeUuid;
use inox2d::render::TexturedMeshRenderCtx;
use inox2d_mcp::audit::audit_rig;
use inox2d_mcp::engine::{command_registry, Session};
use inox2d_mcp::headless_render::{render_diagnostic, render_frame, RenderOptions};
use inox2d_mcp::psd::{import_psd_to_model_with_mode, parse_psd_bytes};
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
	/// Input .inp or .psd file, or '--sample'
	#[arg(default_value = "--sample")]
	input: String,
	/// Watch/sync .inp file on disk for bidirectional hot-reload with MCP / AI agents
	#[arg(long)]
	watch: Option<PathBuf>,
	/// Render several frames, capture a PNG screenshot of the studio window, and exit
	#[arg(long)]
	screenshot: Option<PathBuf>,
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
	HierarchyDetail,
}

fn load_initial_session(input: &str) -> Result<Session, String> {
	if input == "--sample" || input == "sample" {
		let mut session = Session::new_sample();
		// Pre-populate 2D mouth visemes and Blush/Tears overlays so the studio showcases full controls immediately.
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
	tool: StudioTool,

	stage_res: u32,
	puppet_scale: f32,
	view_zoom: f32,
	view_pan: Vec2,
	wireframe: bool,
	xray_mode: bool,
	show_sel_overlay: bool,

	brush_param: String,
	brush_radius_ratio: f32,

	bottom_tab: BottomTab,
	playing: bool,
	loop_anim: bool,
	playhead_frame: f32,
	last_tick: Instant,

	stage_tex: Option<egui::TextureHandle>,
	cached_rev: u64,
	cached_sig: u64,
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
			tool: StudioTool::Select,
			stage_res: 512,
			puppet_scale: 0.92,
			view_zoom: 1.0,
			view_pan: Vec2::ZERO,
			wireframe: false,
			xray_mode: false,
			show_sel_overlay: true,
			brush_param: "Head:: Yaw-Pitch".to_owned(),
			brush_radius_ratio: 0.45,
			bottom_tab: BottomTab::Timeline,
			playing: false,
			loop_anim: true,
			playhead_frame: 1.0,
			last_tick: Instant::now(),
			stage_tex: None,
			cached_rev: u64::MAX,
			cached_sig: u64::MAX,
			render_ms: 0.0,
			audit_cache: JsonValue::new_object(),
			audit_rev: u64::MAX,
			status_msg: "Ready. Drag 2D XY Pads on the right or pick Pose / Deform Brush on the left.".to_owned(),
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
			self.cached_rev = u64::MAX;
			self.audit_rev = u64::MAX;
			self.set_status(true, format!("Hot-reloaded external change from {}", wpath.display()));
		}
	}

	fn refresh_audit(&mut self) {
		if self.audit_rev != self.session.revision {
			self.audit_cache = audit_rig(&mut self.session.model);
			self.audit_rev = self.session.revision;
			self.cached_rev = u64::MAX; // ensure puppet stays evaluated cleanly
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

	fn update_stage_texture(&mut self, ctx: &egui::Context) {
		let sig = self.render_signature();
		if self.stage_tex.is_some() && self.cached_rev == self.session.revision && self.cached_sig == sig {
			return;
		}
		let start = Instant::now();
		let pmap = self.filtered_params();
		let res = if self.xray_mode {
			render_diagnostic(&mut self.session.model, &pmap, self.stage_res, self.stage_res, self.puppet_scale)
		} else {
			let opts = RenderOptions {
				width: self.stage_res,
				height: self.stage_res,
				scale: self.puppet_scale,
				wireframe: self.wireframe,
				dt: if self.playing { 1.0 / 30.0 } else { 0.0 },
				background: [0, 0, 0, 0],
			};
			render_frame(&mut self.session.model, &pmap, &opts)
		};

		if let Ok(img) = res {
			self.render_ms = start.elapsed().as_secs_f32() * 1000.0;
			let (w, h) = img.dimensions();
			let cimg = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
			if let Some(tex) = self.stage_tex.as_mut() {
				tex.set(cimg, egui::TextureOptions::LINEAR);
			} else {
				self.stage_tex = Some(ctx.load_texture("inox2d_stage", cimg, egui::TextureOptions::LINEAR));
			}
			self.cached_rev = self.session.revision;
			self.cached_sig = sig;
		}
	}

	fn mvp_cam(&self) -> Mat4 {
		let cam = Camera {
			position: gvec2(0.0, -35.0),
			rotation: 0.0,
			scale: gvec2(self.puppet_scale, self.puppet_scale),
		};
		cam.matrix(gvec2(self.stage_res as f32, self.stage_res as f32))
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
		let ctrl_y = ctx.input(|i| i.modifiers.ctrl && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z))));
		if ctrl_y {
			self.exec_cmd("edit.redo", json::object! {}, "Redid edit");
		}
		ctx.input(|i| {
			if i.key_pressed(Key::V) {
				self.tool = StudioTool::Select;
			} else if i.key_pressed(Key::P) {
				self.tool = StudioTool::Pose25D;
			} else if i.key_pressed(Key::D) {
				self.tool = StudioTool::DeformBrush;
			} else if i.key_pressed(Key::M) {
				self.tool = StudioTool::Delaunay;
			} else if i.key_pressed(Key::G) {
				self.tool = StudioTool::MeshGroup;
			} else if i.key_pressed(Key::S) {
				self.tool = StudioTool::Physics;
			} else if i.key_pressed(Key::H) {
				self.tool = StudioTool::HandPan;
			} else if i.key_pressed(Key::Space) {
				self.playing = !self.playing;
			} else if i.key_pressed(Key::Escape) {
				self.cmd_palette_open = false;
			}
		});
	}

	fn draw_menu_bar(&mut self, ui: &mut egui::Ui) {
		egui::menu::bar(ui, |ui| {
			ui.menu_button("File", |ui| {
				if ui.button("Load Sample VTuber").clicked() {
					if let Ok(s) = load_initial_session("--sample") {
						self.session = s;
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
				if ui.button("Fit Puppet to Stage (F)").clicked() {
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
						self.exec_cmd("node.synthesize_overlay", json::object! { kind: kind }, &format!("Synthesized '{kind}' overlay"));
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
			if let Some(id) = self.selected_node {
				if self.session.model.puppet.node_comps.get::<TexturedMesh>(id).is_some() && ui.button("Delaunay").clicked() {
					self.exec_cmd(
						"mesh.auto_delaunay",
						json::object! { node_uuid: id.0, step_px: 20.0 },
						"Rebuilt selected Part mesh with Bowyer-Watson Delaunay",
					);
				}
			}

			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				let zoom_txt = format!("{:.0}%", self.view_zoom * 100.0);
				if ui.button(&zoom_txt).on_hover_text("Click to fit stage zoom").clicked() {
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
				ui.label(egui::RichText::new(format!("{}x{} px", self.stage_res, self.stage_res)).color(TEXT_MUTED).monospace().size(11.0));
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

		// Middle/Right drag or HandPan pans stage
		let middle_drag = ui.ctx().input(|i| i.pointer.middle_down());
		if (self.tool == StudioTool::HandPan || middle_drag) && resp.dragged() {
			self.view_pan += resp.drag_delta();
		}

		let card_side = (rect.width().min(rect.height()) - 32.0).max(220.0) * self.view_zoom;
		let card_center = rect.center() + self.view_pan;
		let canvas_rect = Rect::from_center_size(card_center, vec2(card_side, card_side));

		// Subtle stage checkerboard card backdrop
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

		if let Some(tex) = &self.stage_tex {
			clip_painter.image(
				tex.id(),
				canvas_rect,
				Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
				Color32::WHITE,
			);
		}

		// Draw selected node AniCraft-style vector bbox, wireframe & pivot crosshair
		if self.show_sel_overlay {
			if let Some((pts, indices, bbox, pivot)) = self.selected_node_screen_geometry(canvas_rect) {
				let expanded = bbox.expand(2.0);
				if !self.wireframe {
					let wire_stroke = Stroke::new(0.8_f32, Color32::from_rgba_unmultiplied(59, 130, 246, 95));
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
				for corner in [expanded.left_top(), expanded.right_top(), expanded.right_bottom(), expanded.left_bottom()] {
					clip_painter.rect_filled(
						Rect::from_center_size(corner, vec2(6.0, 6.0)),
						0.0,
						Color32::WHITE,
					);
					clip_painter.rect_stroke(
						Rect::from_center_size(corner, vec2(6.0, 6.0)),
						0.0,
						Stroke::new(1.0_f32, ACCENT_BLUE),
					);
				}
				clip_painter.circle_stroke(pivot, 5.0, Stroke::new(1.3_f32, Color32::from_rgb(250, 204, 21)));
			}
		}

		// Stage tool interactions
		if resp.clicked() && self.tool == StudioTool::Select {
			if let Some(pos) = resp.interact_pointer_pos() {
				// Pick frontmost TexturedMesh whose deformed screen bbox contains `pos`
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
				if let Some(id) = picked.map(|(id, _)| id) {
					if let Some(n) = self.session.model.puppet.nodes.get_node(id) {
						self.set_status(true, format!("Selected '{}' (UUID {})", n.name, id.0));
					}
				}
			}
		} else if self.tool == StudioTool::Pose25D && resp.dragged() {
			if let Some(pos) = resp.interact_pointer_pos() {
				let nx = ((pos.x - canvas_rect.center().x) / (canvas_rect.width() * 0.45)).clamp(-1.0, 1.0);
				let ny = ((pos.y - canvas_rect.center().y) / (canvas_rect.height() * 0.45)).clamp(-1.0, 1.0);
				self.session.param_overrides.insert("Head:: Yaw-Pitch".to_owned(), gvec2(nx, ny));
				self.session.param_overrides.insert("Head:: Roll".to_owned(), gvec2(nx * 0.35, 0.0));
				self.session.param_overrides.insert("Eye:: Gaze".to_owned(), gvec2(nx, ny));
				self.session.param_overrides.insert("Eye:: Left:: Gaze".to_owned(), gvec2(nx, ny));
				self.session.param_overrides.insert("Eye:: Right:: Gaze".to_owned(), gvec2(nx, ny));
			}
		} else if self.tool == StudioTool::DeformBrush {
			if let Some(hover) = resp.hover_pos() {
				if canvas_rect.contains(hover) {
					clip_painter.circle_stroke(
						hover,
						28.0 * self.brush_radius_ratio,
						Stroke::new(1.2_f32, Color32::from_rgb(245, 158, 11)),
					);
				}
			}
			if resp.dragged() {
				if let (Some(id), Some(pos)) = (self.selected_node, resp.interact_pointer_pos()) {
					let delta_screen = resp.drag_delta();
					if delta_screen.length() > 0.2 {
						if let Some(local_xy) = self.ui_to_local_on_node(id, canvas_rect, pos) {
							let inv_scale = (self.stage_res as f32 / canvas_rect.width().max(1.0)) / self.puppet_scale.max(0.05);
							let dx = delta_screen.x * inv_scale;
							let dy = delta_screen.y * inv_scale;
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
									dx: dx,
									dy: dy
								},
							);
							self.sync_watch_out();
							self.cached_rev = u64::MAX;
							self.audit_rev = u64::MAX;
							self.set_status(true, format!("Sculpting '{}' on {}", self.brush_param, id.0));
						}
					}
				}
			}
		}

		// Bottom-right floating stage zoom indicator matching AniCraft ("57%")
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

		ui.vertical(|ui| {
			let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
			let painter = ui.painter();
			painter.rect_filled(rect, 5.0, BG_INPUT);
			painter.rect_stroke(rect, 5.0, Stroke::new(1.0_f32, BORDER));

			// Crosshair grid
			let c = rect.center();
			painter.line_segment([pos2(rect.left(), c.y), pos2(rect.right(), c.y)], Stroke::new(1.0_f32, Color32::from_rgb(40, 42, 48)));
			painter.line_segment([pos2(c.x, rect.top()), pos2(c.x, rect.bottom())], Stroke::new(1.0_f32, Color32::from_rgb(40, 42, 48)));

			if let Some(ref p) = exists {
				if (resp.clicked() || resp.dragged()) && rect.width() > 2.0 {
					if let Some(ptr) = resp.interact_pointer_pos() {
						let tx = ((ptr.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
						let ty = ((ptr.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
						// y maps min..max bottom->top for Viseme (Open [0..1]) or top->bottom for Head?
						// For Mouth:: Viseme (0..1 open downward feels intuitive top=closed, bottom=open)
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

			ui.label(egui::RichText::new(param_name).monospace().size(10.0).color(TEXT_MUTED));
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
								self.exec_cmd("mesh.auto_delaunay", json::object! { node_uuid: id.0, step_px: 20.0 }, "Rebuilt Delaunay mesh");
							}
							if is_mesh && ui.small_button("Contour").clicked() {
								self.exec_cmd("mesh.auto_contour", json::object! { node_uuid: id.0, cols: 8, rows: 8 }, "Rebuilt contour mesh");
							}
							if is_mesh && ui.small_button("Grid").clicked() {
								self.exec_cmd("mesh.auto_grid", json::object! { node_uuid: id.0, cols: 6, rows: 6 }, "Rebuilt grid mesh");
							}
						});
					}
				}
			} else {
				ui.label(egui::RichText::new("Select a node in the hierarchy or stage.").color(TEXT_MUTED).size(11.0));
			}

			ui.add_space(6.0);
			section_header(ui, "2D Parameter Pads", Some(("drag · dbl-click reset", TEXT_MUTED)));
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

	fn draw_bottom_dock(&mut self, ui: &mut egui::Ui) {
		// Toolbar strip
		ui.horizontal(|ui| {
			let t_btn = egui::SelectableLabel::new(self.bottom_tab == BottomTab::Timeline, "Timeline");
			if ui.add(t_btn).clicked() {
				self.bottom_tab = BottomTab::Timeline;
			}
			let m_btn = egui::SelectableLabel::new(self.bottom_tab == BottomTab::HierarchyDetail, "Node Tree & Blend");
			if ui.add(m_btn).clicked() {
				self.bottom_tab = BottomTab::HierarchyDetail;
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

			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				if let Some(id) = self.selected_node {
					if ui.small_button("Delete Node").clicked() {
						self.exec_cmd("node.remove", json::object! { node_uuid: id.0 }, "Removed selected node");
						self.selected_node = None;
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

		// Split Layers column on left + Timeline Tracks / Tween spans on right
		let dock_rect = ui.available_rect_before_wrap();
		let left_w = 236.0;
		let left_rect = Rect::from_min_max(dock_rect.min, pos2((dock_rect.left() + left_w).min(dock_rect.right()), dock_rect.bottom()));
		let right_rect = Rect::from_min_max(pos2(left_rect.right() + 1.0, dock_rect.top()), dock_rect.max);

		// Paint vertical splitter
		ui.painter().line_segment(
			[pos2(left_rect.right(), dock_rect.top()), pos2(left_rect.right(), dock_rect.bottom())],
			Stroke::new(1.0_f32, BORDER),
		);

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
					Color32::from_rgb(245, 158, 11)
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
				}
				if resp.double_clicked() {
					let next_en = !*enabled;
					self.exec_cmd("node.configure", json::object! { node_uuid: uid.0, enabled: next_en }, "Toggled node visibility");
				}
			}
		});

		// Right Timeline Frame Grid (Ruler + Violet/Emerald tween bars + Red playhead)
		if right_rect.width() > 40.0 {
			let tr_painter = ui.painter_at(right_rect);
			tr_painter.rect_filled(right_rect, 0.0, BG_INPUT);
			let ruler_h = 20.0;
			let px_per_frame = 6.5;

			// Frame ruler background
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

			// Grid lines & frame numbers every 10 frames (1, 10, 20, 30, ...)
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

			// Track tween spans per node row (animated/bound layers show violet tween spans matching AniCraft)
			let anim_opt = self.session.model.puppet.animations.get("Idle");
			let span_w = (anim_len as f32 * px_per_frame).min(right_rect.width() - 24.0).max(40.0);
			for (idx, (uid, name, _, _, _, _, _, _)) in nodes_list.iter().enumerate() {
				let y = right_rect.top() + ruler_h + idx as f32 * (row_h + 2.0) + 3.0;
				if y + row_h > right_rect.bottom() {
					break;
				}
				let has_binding = self
					.session
					.model
					.puppet
					.params
					.values()
					.any(|p| p.bindings.iter().any(|b| b.node == *uid));
				let bar_col = if has_binding || name.contains("Head") || name.contains("Eye") {
					TIMELINE_VIOLET
				} else {
					TIMELINE_EMERALD
				};
				let bar_rect = Rect::from_min_size(pos2(right_rect.left() + 12.0, y), vec2(span_w, row_h - 4.0));
				tr_painter.rect_filled(bar_rect, 2.0, bar_col);
				tr_painter.circle_filled(pos2(bar_rect.left() + 6.0, bar_rect.center().y), 3.0, BG_INPUT);
				if has_binding {
					let mid_x = bar_rect.left() + span_w * 0.5;
					tr_painter.circle_filled(pos2(mid_x, bar_rect.center().y), 3.0, BG_INPUT);
					tr_painter.line_segment(
						[pos2(bar_rect.left() + 9.0, bar_rect.center().y), pos2(mid_x - 3.0, bar_rect.center().y)],
						Stroke::new(1.2_f32, BG_INPUT),
					);
				}
				if let Some(a) = anim_opt {
					let end_x = bar_rect.right() - 6.0;
					tr_painter.rect_filled(Rect::from_center_size(pos2(end_x, bar_rect.center().y), vec2(4.0, 8.0)), 0.5, BG_INPUT);
					let _ = a;
				}
			}

			// Red Playhead cursor + frame badge matching AniCraft
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
							// Run parameterless or safe default commands directly from palette
							match spec.id {
								"rig.auto_rig_vtuber" | "rig.auto_fix" | "rig.audit" | "anim.create_idle" | "edit.undo" | "edit.redo"
								| "puppet.new_sample" => {
									self.exec_cmd(spec.id, json::object! {}, &format!("Executed {}", spec.id));
								}
								"craft.figcraft_card" => {
									self.exec_cmd("craft.figcraft_card", json::object! { out: "studio_spec_card.png" }, "Exported FigCraft spec card");
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
		self.update_stage_texture(ctx);

		// Capture screenshot on frame 5 if --screenshot was specified
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

		// 1. Top Menu Bar (#141417)
		egui::TopBottomPanel::top("top_menu_bar")
			.exact_height(26.0)
			.frame(Frame::none().fill(BG_MENU).inner_margin(Margin::symmetric(8.0, 3.0)))
			.show(ctx, |ui| self.draw_menu_bar(ui));

		// 2. Context Sub-Header Bar
		egui::TopBottomPanel::top("context_bar")
			.exact_height(30.0)
			.frame(
				Frame::none()
					.fill(Color32::from_rgb(26, 26, 30))
					.stroke(Stroke::new(1.0_f32, BORDER))
					.inner_margin(Margin::symmetric(8.0, 4.0)),
			)
			.show(ctx, |ui| self.draw_context_bar(ui));

		// 3. Bottom Status Bar (#141417)
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

		// 4. Right Inspector Dock (328px wide, full height between header & status bar)
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

		// 5. Left Vector Tool Rail (38px wide)
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
					for &t in StudioTool::all() {
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
								}
								_ => self.tool = t,
							}
						}
						ui.add_space(2.0);
					}
				});
			});

		// 6. Bottom Timeline & Node Layers Dock
		egui::TopBottomPanel::bottom("bottom_timeline_dock")
			.exact_height(215.0)
			.frame(
				Frame::none()
					.fill(BG_DOCK)
					.stroke(Stroke::new(1.0_f32, BORDER))
					.inner_margin(Margin::symmetric(8.0, 4.0)),
			)
			.show(ctx, |ui| self.draw_bottom_dock(ui));

		// 7. Center Stage Canvas (#2B2D31)
		egui::CentralPanel::default()
			.frame(Frame::none().fill(BG_STAGE))
			.show(ctx, |ui| self.draw_stage(ui));

		self.draw_command_palette(ctx);
	}
}

fn main() -> eframe::Result<()> {
	let cli = Cli::parse();
	let session = load_initial_session(&cli.input).unwrap_or_else(|err| {
		eprintln!("Error loading '{}': {err}", cli.input);
		std::process::exit(1);
	});
	let source_path = if cli.input == "--sample" || cli.input == "sample" {
		None
	} else {
		Some(PathBuf::from(&cli.input))
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

#![forbid(unsafe_code)]

mod autosave;
mod bindings;
mod booster_panel;
mod dialogs;
mod hierarchy;
mod i18n;
mod icons;
mod panels;
mod project;
mod session_ops;
mod settings;
mod shortcuts;
mod stage;
mod stage_render;
mod startup;
mod theme;
mod timeline;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use clap::Parser;
use eframe::egui::{self, Color32, Frame, Margin, Stroke, Vec2};
use glam::Vec2 as GVec2;
use riglens_core::node::components::{Mesh, SimplePhysics, TexturedMesh};
use riglens_core::node::InoxNodeUuid;
use riglens_core::texture::ShallowTexture;
use riglens_mcp::engine::{ModelSnapshot, Session};
use riglens_mcp::rig::DeformBrushMode;
use json::JsonValue;
use riglens_stage_gl::{SharedGlStage, StageGeometry, StageTextureInput};

use autosave::{AutosaveManager, RecoveryInfo};
use bindings::{BindingEditorState, InspectorTab};
use dialogs::{draw_file_dialog, FileDialogResult, FileDialogState};
use hierarchy::{draw_outliner_panel, OutlinerTab};
use icons::{draw_tool_button, StudioTool};
use stage::{StageDrag, VertexEditMode};
use startup::{
	load_initial_session, pick_default_anim, pick_default_pad_params, pick_default_selected_node,
	resolve_startup_input, save_color_image_png, write_inp_atomic, Cli,
};
use theme::{
	configure_craft_theme, AUDIT_ERR_ROSE, AUDIT_OK_GREEN, AUDIT_WARN_AMBER, BG_DOCK, BG_MENU, BG_STAGE, BORDER,
	TEXT_MUTED,
};
use timeline::{BottomTab, CurveDragState};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WorkspaceMode {
	Quick,
	Pro,
}

pub(crate) struct Inox2dStudioApp {
	pub(crate) workspace_mode: WorkspaceMode,
	pub(crate) quick_show_outliner: bool,
	pub(crate) show_welcome: bool,
	pub(crate) active_project: Option<project::ProjectWorkspace>,
	pub(crate) pending_psd_project_import: bool,
	pub(crate) i18n: i18n::I18n,
	pub(crate) session: Session,
	pub(crate) source_path: Option<PathBuf>,
	pub(crate) watch_path: Option<PathBuf>,
	pub(crate) last_watch_mtime: Option<SystemTime>,
	pub(crate) last_watch_check: Instant,
	pub(crate) last_synced_rev: u64,
	pub(crate) last_saved_rev: u64,
	pub(crate) autosave: AutosaveManager,
	pub(crate) pending_recovery: Option<RecoveryInfo>,
	pub(crate) file_dialog: Option<FileDialogState>,

	pub(crate) selected_node: Option<InoxNodeUuid>,
	pub(crate) selected_vertices: HashSet<u32>,
	pub(crate) selected_physics: Option<InoxNodeUuid>,
	pub(crate) collapsed_nodes: HashSet<InoxNodeUuid>,
	pub(crate) outliner_tab: OutlinerTab,
	pub(crate) outliner_filter: String,
	pub(crate) inspector_state: BindingEditorState,
	pub(crate) tool: StudioTool,
	pub(crate) vertex_mode: VertexEditMode,

	pub(crate) stage_res: u32,
	pub(crate) stage_viewport_size: GVec2,
	pub(crate) pixels_per_point: f32,
	pub(crate) needs_initial_fit: bool,
	pub(crate) initial_zoom: Option<f32>,
	pub(crate) initial_focus_y: f32,
	pub(crate) puppet_scale: f32,
	pub(crate) puppet_center: GVec2,
	pub(crate) view_zoom: f32,
	pub(crate) view_pan: Vec2,
	pub(crate) wireframe: bool,
	pub(crate) xray_mode: bool,
	pub(crate) exact_composite: bool,
	pub(crate) live_physics: bool,
	pub(crate) show_sel_overlay: bool,

	pub(crate) brush_param: String,
	pub(crate) brush_mode: DeformBrushMode,
	pub(crate) brush_radius_ratio: f32,
	pub(crate) pad_params: [String; 2],

	pub(crate) stage_drag: Option<StageDrag>,
	pub(crate) gesture_snapshot: Option<ModelSnapshot>,

	pub(crate) bottom_tab: BottomTab,
	pub(crate) active_anim: String,
	pub(crate) new_anim_name: String,
	pub(crate) playing: bool,
	pub(crate) loop_anim: bool,
	pub(crate) playhead_frame: f32,
	pub(crate) last_tick: Instant,
	pub(crate) selected_curve_lane: Option<(String, u8)>,
	pub(crate) selected_keyframe: Option<(String, u8, i32)>,
	pub(crate) curve_drag: Option<CurveDragState>,

	pub(crate) gl_stage: SharedGlStage,
	pub(crate) gl_last_ms: Arc<Mutex<f32>>,
	pub(crate) gl_textures: Arc<Vec<StageTextureInput>>,
	pub(crate) gl_tex_gen: u64,
	pub(crate) gl_geo: Option<Arc<StageGeometry>>,
	pub(crate) gl_geo_key: u64,
	pub(crate) model_gen: u64,
	pub(crate) decoded_textures: Vec<ShallowTexture>,
	pub(crate) gpu_tex_dirty: bool,
	pub(crate) stage_tex: Option<egui::TextureHandle>,
	pub(crate) cached_rev: u64,
	pub(crate) cached_sig: u64,
	pub(crate) eval_ms: f32,
	pub(crate) render_ms: f32,

	pub(crate) audit_cache: JsonValue,
	pub(crate) audit_rev: u64,
	pub(crate) booster_ui: booster_panel::BoosterUiState,
	pub(crate) booster_live_fx: bool,
	pub(crate) booster_gpu: Option<riglens_booster::gpu::GpuFrameProcessor>,

	pub(crate) status_msg: String,
	pub(crate) status_ok: bool,

	pub(crate) cmd_palette_open: bool,
	pub(crate) cmd_query: String,

	pub(crate) screenshot_out: Option<PathBuf>,
	pub(crate) frame_counter: u32,
	pub(crate) screenshot_sent: bool,
}

impl Inox2dStudioApp {
	fn new(
		session: Session,
		source_path: Option<PathBuf>,
		watch_path: Option<PathBuf>,
		screenshot_out: Option<PathBuf>,
	) -> Self {
		let selected_node = pick_default_selected_node(&session.model.puppet);
		let active_anim = pick_default_anim(&session.model.puppet);

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
		let autosave = AutosaveManager::new();
		let pending_recovery = if screenshot_out.is_none() {
			autosave.check_pending_recovery()
		} else {
			None
		};
		let diag_count = session.model.puppet.diagnostics.len();
		let pad_params = pick_default_pad_params(&session.model.puppet);
		let brush_param = pad_params[0].clone();
		let initial_status = if diag_count > 0 {
			format!("Loaded with {diag_count} compatibility warning(s) — check Audit tab")
		} else {
			"Ready. V=Move/Scale · A=Vertex Edit · D=Deform Brush · S=Physics Gizmo · P=Pose 2.5D".to_owned()
		};

		let mut app = Self {
			workspace_mode: WorkspaceMode::Quick,
			quick_show_outliner: false,
			show_welcome: false,
			active_project: None,
			pending_psd_project_import: false,
			i18n: i18n::I18n::new("en"),
			session,
			source_path,
			watch_path,
			last_watch_mtime,
			last_watch_check: Instant::now(),
			last_synced_rev: rev,
			last_saved_rev: rev,
			autosave,
			pending_recovery,
			file_dialog: None,
			selected_node,
			selected_vertices: HashSet::new(),
			selected_physics,
			collapsed_nodes: HashSet::new(),
			outliner_tab: OutlinerTab::Tree,
			outliner_filter: String::new(),
			inspector_state: BindingEditorState::default(),
			tool: StudioTool::Select,
			vertex_mode: VertexEditMode::DeformKeypoint,
			stage_res: 1024,
			stage_viewport_size: glam::vec2(660.0, 460.0),
			pixels_per_point: 1.0,
			needs_initial_fit: true,
			initial_zoom: None,
			initial_focus_y: 0.0,
			puppet_scale: 0.92,
			puppet_center: GVec2::ZERO,
			view_zoom: 1.0,
			view_pan: Vec2::ZERO,
			wireframe: false,
			xray_mode: false,
			exact_composite: false,
			live_physics: false,
			show_sel_overlay: true,
			brush_param: brush_param.clone(),
			brush_mode: DeformBrushMode::Grab,
			brush_radius_ratio: 0.45,
			pad_params,
			stage_drag: None,
			gesture_snapshot: None,
			bottom_tab: BottomTab::Timeline,
			active_anim,
			new_anim_name: String::new(),
			playing: false,
			loop_anim: true,
			playhead_frame: 1.0,
			last_tick: Instant::now(),
			selected_curve_lane: Some((brush_param, 0)),
			selected_keyframe: None,
			curve_drag: None,
			gl_stage: Arc::new(Mutex::new(None)),
			gl_last_ms: Arc::new(Mutex::new(0.0)),
			gl_textures: Arc::new(Vec::new()),
			gl_tex_gen: 0,
			gl_geo: None,
			gl_geo_key: u64::MAX,
			model_gen: 1,
			decoded_textures: Vec::new(),
			gpu_tex_dirty: true,
			stage_tex: None,
			cached_rev: u64::MAX,
			cached_sig: u64::MAX,
			eval_ms: 0.0,
			render_ms: 0.0,
			audit_cache: JsonValue::new_object(),
			audit_rev: u64::MAX,
			booster_ui: booster_panel::BoosterUiState::default(),
			booster_live_fx: false,
			booster_gpu: None,
			status_msg: initial_status,
			status_ok: diag_count == 0,
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

	pub(crate) fn set_workspace_mode(&mut self, mode: WorkspaceMode) {
		self.workspace_mode = mode;
		if mode == WorkspaceMode::Quick {
			if matches!(
				self.tool,
				StudioTool::VertexSubselect | StudioTool::Delaunay | StudioTool::MeshGroup | StudioTool::Physics
			) {
				self.tool = StudioTool::Pose25D;
			}
			if self.inspector_state.tab == InspectorTab::Bindings {
				self.inspector_state.tab = InspectorTab::Audit;
			}
			self.set_status(true, "Switched to Quick Inspect & Edit mode (F2 for Pro)");
		} else {
			self.set_status(true, "Switched to Pro Rigger & Timeline mode (F2 for Quick)");
		}
		self.save_user_settings();
	}

	pub(crate) fn save_user_settings(&self) {
		if self.screenshot_out.is_some() {
			return;
		}
		let mode_str = match self.workspace_mode {
			WorkspaceMode::Quick => "quick",
			WorkspaceMode::Pro => "pro",
		};
		let cfg = settings::UserSettings {
			language: self.i18n.current_lang().to_owned(),
			workspace_mode: mode_str.to_owned(),
		};
		let _ = settings::save_settings_to(&settings::settings_path(), &cfg);
	}
}

impl eframe::App for Inox2dStudioApp {
	fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
		let _ = self.session.booster.stop();
		if let Some(gl) = gl {
			if let Ok(mut guard) = self.gl_stage.lock() {
				if let Some(renderer) = guard.take() {
					renderer.destroy(gl);
				}
			}
		}
		if self.session.revision == self.last_saved_rev {
			self.autosave.clear_recovery();
		}
	}

	fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
		self.frame_counter += 1;
		self.pixels_per_point = ctx.pixels_per_point().max(1.0);
		self.poll_watch_in();
		self.handle_shortcuts(ctx);

		if self.screenshot_out.is_none() {
			let _ = self.autosave.maybe_autosave(
				&self.session.model,
				self.session.revision,
				self.last_saved_rev,
				self.source_path.as_deref(),
			);
		}

		let frame_dt = self.last_tick.elapsed().as_secs_f32().clamp(1.0 / 240.0, 0.05);
		let booster_active = self.session.booster.is_running();
		if self.playing {
			let anim_len = self
				.session
				.model
				.puppet
				.animations
				.get(self.active_anim.as_str())
				.map(|a| a.length.max(1) as f32)
				.unwrap_or(120.0);
			self.playhead_frame += frame_dt * 30.0;
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
		} else if (self.live_physics || booster_active) && self.screenshot_out.is_none() {
			self.cached_sig = u64::MAX;
			ctx.request_repaint();
		} else if self.inspector_state.tab == InspectorTab::Booster && self.screenshot_out.is_none() {
			ctx.request_repaint_after(Duration::from_millis(100));
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
			.exact_width(320.0)
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
					let groups = if self.workspace_mode == WorkspaceMode::Quick {
						StudioTool::quick_groups()
					} else {
						StudioTool::groups()
					};
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

		// 5b. Left Hierarchical Outliner & Draw-Order Panel (always in Pro, optional toggle in Quick)
		if self.workspace_mode == WorkspaceMode::Pro || self.quick_show_outliner {
			let mut outliner_actions = Vec::new();
			egui::SidePanel::left("left_outliner")
				.resizable(true)
				.default_width(220.0)
				.width_range(175.0..=320.0)
				.frame(
					Frame::none()
						.fill(BG_DOCK)
						.stroke(Stroke::new(1.0_f32, BORDER))
						.inner_margin(Margin::symmetric(6.0, 4.0)),
				)
				.show(ctx, |ui| {
					outliner_actions = draw_outliner_panel(
						ui,
						&self.session.model.puppet,
						self.selected_node,
						&self.collapsed_nodes,
						&mut self.outliner_tab,
						&mut self.outliner_filter,
					);
				});
			if !outliner_actions.is_empty() {
				self.apply_outliner_actions(outliner_actions);
			}
		}

		// 6. Bottom Timeline & Motion Curves Dock (Pro mode only)
		if self.workspace_mode == WorkspaceMode::Pro {
			egui::TopBottomPanel::bottom("bottom_timeline_dock")
				.exact_height(215.0)
				.frame(
					Frame::none()
						.fill(BG_DOCK)
						.stroke(Stroke::new(1.0_f32, BORDER))
						.inner_margin(Margin::symmetric(8.0, 4.0)),
				)
				.show(ctx, |ui| self.draw_bottom_dock(ui));
		}

		// 7. Center Stage (+ optional autosave crash-recovery banner)
		egui::CentralPanel::default()
			.frame(Frame::none().fill(BG_STAGE))
			.show(ctx, |ui| {
				if let Some(rec) = self.pending_recovery.clone() {
					egui::Frame::none()
						.fill(Color32::from_rgb(66, 42, 12))
						.stroke(Stroke::new(1.0_f32, AUDIT_WARN_AMBER))
						.inner_margin(Margin::symmetric(10.0, 4.0))
						.show(ui, |ui| {
							ui.horizontal(|ui| {
								let src_txt = rec.original_path.as_deref().unwrap_or("unsaved project");
								ui.label(
									egui::RichText::new(format!("Autosave recovery available for {src_txt}"))
										.color(AUDIT_WARN_AMBER)
										.size(11.0),
								);
								if ui.small_button("Recover").clicked() {
									let orig = rec.original_path.as_ref().map(PathBuf::from);
									self.open_model_from_path(&rec.recovery_inp);
									self.source_path = orig;
									self.last_saved_rev = u64::MAX;
									self.pending_recovery = None;
									self.autosave.clear_recovery();
									self.set_status(true, "Recovered autosaved session");
								}
								if ui.small_button("Dismiss").clicked() {
									self.pending_recovery = None;
									self.autosave.clear_recovery();
								}
							});
						});
				}
				self.draw_stage(ui);
			});

		self.draw_command_palette(ctx);
		if self.show_welcome && self.file_dialog.is_none() {
			self.draw_welcome_screen(ctx);
		}
		if let Some(mut dlg) = self.file_dialog.take() {
			let res = draw_file_dialog(ctx, &mut dlg);
			if res == FileDialogResult::Pending {
				self.file_dialog = Some(dlg);
			} else {
				self.handle_file_dialog_result(res);
			}
		}
	}
}

fn main() -> eframe::Result<()> {
	let cli = Cli::parse();
	let (input_arg, source_path) = resolve_startup_input(&cli).unwrap_or_else(|err| {
		eprintln!("Error resolving startup model: {err}");
		std::process::exit(1);
	});
	let session = load_initial_session(&input_arg).unwrap_or_else(|err| {
		eprintln!("Error loading '{input_arg}': {err}");
		std::process::exit(1);
	});

	if let Some(ref wpath) = cli.watch {
		if !wpath.exists() {
			let _ = write_inp_atomic(&session.model, wpath);
		}
	}

	let open_welcome = cli.input.is_none() && cli.model.is_none() && !cli.sample && cli.screenshot.is_none();
	let native_options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default()
			.with_inner_size([1280.0, 760.0])
			.with_min_inner_size([1000.0, 620.0])
			.with_title("RigLens Editor"),
		..Default::default()
	};

	eframe::run_native(
		"RigLens Editor",
		native_options,
		Box::new(move |cc| {
			configure_craft_theme(&cc.egui_ctx);
			let mut app = Inox2dStudioApp::new(
				session,
				source_path,
				cli.watch,
				cli.screenshot,
			);
			if cli.cpu_exact {
				app.exact_composite = true;
			} else if cli.gpu_stage {
				app.exact_composite = false;
			}
			app.initial_zoom = cli.zoom;
			app.initial_focus_y = cli.focus_y;
			app.show_welcome = open_welcome;
			let saved = settings::load_settings_from(&settings::settings_path());
			app.i18n.set_language(&saved.language);
			if saved.workspace_mode.eq_ignore_ascii_case("pro") {
				app.workspace_mode = WorkspaceMode::Pro;
			}
			Ok(Box::new(app))
		}),
	)
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

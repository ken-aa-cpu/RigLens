use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::Vec2;
use glam::{vec2 as gvec2, vec3 as gvec3, Mat4, Vec2 as GVec2};
use riglens_core::formats::inp::parse_inp;
use riglens_core::node::components::{Mesh, SimplePhysics, TexturedMesh, TransformStore};
use riglens_mcp::audit::audit_rig;
use riglens_mcp::engine::Session;
use json::JsonValue;

use crate::dialogs::{FileDialogMode, FileDialogResult, FileDialogState};
use crate::startup::{
	load_initial_session, pick_default_anim, pick_default_pad_params, pick_default_selected_node, write_inp_atomic,
};
use crate::Inox2dStudioApp;

impl Inox2dStudioApp {
	pub(crate) fn set_status(&mut self, ok: bool, msg: impl Into<String>) {
		self.status_ok = ok;
		self.status_msg = msg.into();
	}

	pub(crate) fn begin_gesture_undo(&mut self) {
		if self.gesture_snapshot.is_none() {
			self.gesture_snapshot = Some(self.session.snapshot());
		}
	}

	pub(crate) fn finish_gesture_undo(&mut self) {
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

	pub(crate) fn fit_puppet_scale(&mut self) {
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
		puppet.end_frame(0.0);

		let mut min_pt = GVec2::splat(f32::INFINITY);
		let mut max_pt = GVec2::splat(f32::NEG_INFINITY);
		for node in puppet.nodes.iter() {
			if !node.enabled || puppet.node_comps.get::<TexturedMesh>(node.uuid).is_none() {
				continue;
			}
			let Some(m) = puppet.node_comps.get::<Mesh>(node.uuid) else {
				continue;
			};
			let abs = puppet
				.node_comps
				.get::<TransformStore>(node.uuid)
				.map(|ts| ts.absolute)
				.unwrap_or(Mat4::IDENTITY);
			for &v in &m.vertices {
				let w = abs
					.transform_point3(gvec3(v.x - m.origin.x, v.y - m.origin.y, 0.0))
					.truncate();
				min_pt = min_pt.min(w);
				max_pt = max_pt.max(w);
			}
		}
		if min_pt.x.is_finite()
			&& min_pt.y.is_finite()
			&& max_pt.x.is_finite()
			&& max_pt.y.is_finite()
			&& ((max_pt.x - min_pt.x) > 8.0 || (max_pt.y - min_pt.y) > 8.0)
		{
			self.puppet_center = (min_pt + max_pt) * 0.5;
			let span_x = (max_pt.x - min_pt.x).max(64.0);
			let span_y = (max_pt.y - min_pt.y).max(64.0);
			let sx = (self.stage_viewport_size.x.max(200.0) * 0.86) / span_x;
			let sy = (self.stage_viewport_size.y.max(200.0) * 0.86) / span_y;
			self.puppet_scale = sx.min(sy).clamp(0.05, 4.0);
		} else {
			self.puppet_center = gvec2(0.0, 35.0);
			self.puppet_scale = 0.85;
		}
		self.view_zoom = 1.0;
		self.view_pan = Vec2::ZERO;
	}

	pub(crate) fn load_session_into_app(&mut self, mut session: Session, source_path: Option<PathBuf>, status_msg: String) {
		// Preserve running booster lifecycle if user switches model files cleanly
		std::mem::swap(&mut session.booster, &mut self.session.booster);
		self.session = session;
		self.source_path = source_path;
		self.last_saved_rev = self.session.revision;
		self.last_synced_rev = self.session.revision;
		self.selected_node = pick_default_selected_node(&self.session.model.puppet);
		self.active_anim = pick_default_anim(&self.session.model.puppet);
		self.selected_physics = self
			.session
			.model
			.puppet
			.nodes
			.iter()
			.find(|n| self.session.model.puppet.node_comps.get::<SimplePhysics>(n.uuid).is_some())
			.map(|n| n.uuid);
		self.pad_params = pick_default_pad_params(&self.session.model.puppet);
		self.brush_param = self.pad_params[0].clone();
		self.selected_curve_lane = Some((self.brush_param.clone(), 0));
		self.selected_vertices.clear();
		self.collapsed_nodes.clear();
		self.inspector_state.rename_node_id = None;
		self.gpu_tex_dirty = true;
		self.gl_geo = None;
		self.needs_initial_fit = true;
		self.model_gen = self.model_gen.wrapping_add(1);
		self.sync_watch_out();
		self.fit_puppet_scale();
		self.cached_rev = u64::MAX;
		self.audit_rev = u64::MAX;
		self.refresh_audit();
		let warns = self.session.model.puppet.diagnostics.len();
		if warns > 0 {
			self.set_status(false, format!("{status_msg} ({warns} warning(s))"));
		} else {
			self.set_status(true, status_msg);
		}
	}

	pub(crate) fn exec_cmd(&mut self, cmd: &str, params: JsonValue, ok_label: &str) {
		let prev = self.session.revision;
		match self.session.exec(cmd, &params) {
			Ok(_) => {
				if self.session.revision != prev {
					if matches!(
						cmd,
						"puppet.new_sample"
							| "puppet.import_psd"
							| "psd.sync_textures"
							| "puppet.import"
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

	pub(crate) fn open_file_dialog(&mut self, mode: FileDialogMode) {
		let start_dir = self
			.source_path
			.as_ref()
			.and_then(|p| p.parent())
			.map(|p| p.to_path_buf())
			.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
		let suggested = if mode == FileDialogMode::SaveInpAs {
			self.source_path
				.as_ref()
				.and_then(|p| p.file_name())
				.and_then(|s| s.to_str())
		} else {
			None
		};
		self.file_dialog = Some(FileDialogState::open(mode, start_dir, suggested));
	}

	pub(crate) fn open_model_from_path(&mut self, path: &Path) {
		let path_str = path.to_string_lossy().to_string();
		match load_initial_session(&path_str) {
			Ok(session) => {
				self.load_session_into_app(
					session,
					Some(path.to_path_buf()),
					format!("Opened '{}'", path.display()),
				);
			}
			Err(err) => {
				self.set_status(false, err);
			}
		}
	}

	pub(crate) fn save_model_to_path(&mut self, path: &Path) {
		let path_str = path.to_string_lossy().to_string();
		let prev_rev = self.session.revision;
		match self.session.exec("puppet.save", &json::object! { path: path_str.as_str() }) {
			Ok(_) => {
				self.session.revision = prev_rev;
				self.source_path = Some(path.to_path_buf());
				self.last_saved_rev = self.session.revision;
				self.autosave.clear_recovery();
				self.pending_recovery = None;
				self.set_status(true, format!("Saved '{}'", path.display()));
			}
			Err(err) => {
				self.set_status(false, format!("Save failed: {err}"));
			}
		}
	}

	pub(crate) fn handle_file_dialog_result(&mut self, res: FileDialogResult) {
		match res {
			FileDialogResult::Pending => {}
			FileDialogResult::Cancelled => {
				self.file_dialog = None;
				self.pending_psd_project_import = false;
			}
			FileDialogResult::Confirmed(mode, path) => {
				self.file_dialog = None;
				let is_proj_psd = self.pending_psd_project_import;
				self.pending_psd_project_import = false;
				match mode {
					FileDialogMode::OpenModel => {
						let is_psd = path
							.extension()
							.and_then(|e| e.to_str())
							.map(|s| s.eq_ignore_ascii_case("psd"))
							.unwrap_or(false);
						if is_proj_psd && is_psd {
							match crate::project::create_project_from_psd(
								&crate::project::default_projects_root(),
								&path,
							) {
								Ok(ws) => {
									let path_str = path.to_string_lossy().to_string();
									match load_initial_session(&path_str) {
										Ok(session) => {
											let _ = write_inp_atomic(&session.model, &ws.working_inp);
											let msg = format!(
												"Project '{}' created (vault PSD backed up)",
												ws.name
											);
											let working = ws.working_inp.clone();
											self.active_project = Some(ws);
											self.show_welcome = false;
											self.load_session_into_app(session, Some(working), msg);
										}
										Err(err) => self.set_status(false, err),
									}
								}
								Err(err) => self.set_status(false, err),
							}
						} else {
							self.active_project = None;
							self.show_welcome = false;
							self.open_model_from_path(&path);
						}
					}
					FileDialogMode::SyncPsdTextures => {
						let p = path.to_string_lossy().to_string();
						self.exec_cmd(
							"psd.sync_textures",
							json::object! { path: p.as_str() },
							&format!("Synced updated textures from '{}'", path.display()),
						);
					}
					FileDialogMode::SaveInpAs => self.save_model_to_path(&path),
					FileDialogMode::ExportRigJson => {
						let p = path.to_string_lossy().to_string();
						self.exec_cmd(
							"puppet.export",
							json::object! { path: p.as_str(), format: "rig-json", pretty: true },
							&format!("Exported '{}'", path.display()),
						);
					}
					FileDialogMode::ExportPsd => {
						let p = path.to_string_lossy().to_string();
						self.exec_cmd(
							"puppet.export_psd",
							json::object! { path: p.as_str() },
							&format!("Exported PSD '{}'", path.display()),
						);
					}
				}
			}
		}
	}

	pub(crate) fn sync_watch_out(&mut self) {
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

	pub(crate) fn poll_watch_in(&mut self) {
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

	pub(crate) fn refresh_audit(&mut self) {
		if self.stage_drag.is_some() || self.curve_drag.is_some() {
			return;
		}
		if self.audit_rev != self.session.revision {
			self.audit_cache = audit_rig(&mut self.session.model);
			self.audit_rev = self.session.revision;
			self.cached_rev = u64::MAX;
		}
	}

	pub(crate) fn keypoint_cell_for_param(&self, param_name: &str) -> Option<(usize, usize)> {
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

	pub(crate) fn delete_selected_vertices(&mut self) {
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
}

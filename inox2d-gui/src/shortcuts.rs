use eframe::egui::{self, Key};
use glam::Vec2 as GVec2;
use inox2d::node::components::{Mesh, PathDeformer, TexturedMesh};
use inox2d_mcp::rig::{apply_vertex_deform_delta, retriangulate_points_preserving_deforms};

use crate::dialogs::FileDialogMode;
use crate::icons::StudioTool;
use crate::stage::VertexEditMode;
use crate::Inox2dStudioApp;

impl Inox2dStudioApp {
	pub(crate) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
		if self.file_dialog.is_some() || ctx.wants_keyboard_input() {
			return;
		}
		let ctrl_o = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::O));
		if ctrl_o {
			self.open_file_dialog(FileDialogMode::OpenModel);
			return;
		}
		let ctrl_s = ctx.input(|i| i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(Key::S));
		if ctrl_s {
			if let Some(p) = self.watch_path.clone().or_else(|| self.source_path.clone()) {
				self.save_model_to_path(&p);
			} else {
				self.open_file_dialog(FileDialogMode::SaveInpAs);
			}
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
							let has_tm = self.session.model.puppet.node_comps.get::<TexturedMesh>(id).is_some();
							if let Some(mesh) = self.session.model.puppet.node_comps.get_mut::<Mesh>(id) {
								for &vi in &indices {
									if let Some(v) = mesh.vertices.get_mut(vi as usize) {
										*v += nudge;
									}
								}
								let verts = mesh.vertices.clone();
								if has_tm {
									let _ = retriangulate_points_preserving_deforms(&mut self.session.model.puppet, id, verts);
								}
							} else if let Some(path) = self.session.model.puppet.node_comps.get_mut::<PathDeformer>(id) {
								for &vi in &indices {
									if let Some(v) = path.vertices.get_mut(vi as usize) {
										*v += nudge;
									}
								}
								self.session.model.puppet.reset_contexts();
							}
						}
					}
					self.finish_gesture_undo();
					self.set_status(true, format!("Nudged {} selected vertices", indices.len()));
				}
			}
		}
	}
}

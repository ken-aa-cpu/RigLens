use eframe::egui::{self, Align2, Color32, Margin, Rounding, Sense, Stroke, Vec2};
use glam::{vec2 as gvec2, Vec2 as GVec2};
use inox2d::node::components::{SimplePhysics, TexturedMesh};
use inox2d::node::InoxNodeUuid;
use inox2d_mcp::engine::command_registry;
use inox2d_mcp::rig::DeformBrushMode;

use crate::bindings::{draw_audit_tab, draw_bindings_tab, draw_properties_tab, InspectorAction, InspectorTab};
use crate::booster_panel;
use crate::dialogs::FileDialogMode;
use crate::hierarchy::OutlinerAction;
use crate::icons::StudioTool;
use crate::project::{default_projects_root, list_projects, open_folder_in_os};
use crate::stage::VertexEditMode;
use crate::startup::{
	clean_display_name, find_example_models, load_initial_session, pick_default_pad_params, VISEME_PRESETS,
};
use crate::theme::{
	section_header, ACCENT_BLUE, AUDIT_ERR_ROSE, AUDIT_OK_GREEN, AUDIT_WARN_AMBER, BG_WIDGET, BORDER, BTN_ACTIVE,
	TEXT_MUTED, TIMELINE_VIOLET,
};
use crate::{Inox2dStudioApp, WorkspaceMode};

impl Inox2dStudioApp {
	pub(crate) fn draw_menu_bar(&mut self, ui: &mut egui::Ui) {
		egui::menu::bar(ui, |ui| {
			let lbl_file = self.i18n.t("menu.file").to_owned();
			let lbl_welcome = self.i18n.t("menu.file.welcome").to_owned();
			let lbl_imp_proj = self.i18n.t("menu.file.import_psd_project").to_owned();
			let lbl_open_no = self.i18n.t("menu.file.open_no_project").to_owned();
			let lbl_open_vault = self.i18n.t("menu.file.open_vault").to_owned();
			let lbl_save = self.i18n.t("menu.file.save").to_owned();
			let lbl_save_as = self.i18n.t("menu.file.save_as").to_owned();

			ui.menu_button(lbl_file, |ui| {
				if ui.button(lbl_welcome).clicked() {
					self.show_welcome = true;
					ui.close_menu();
				}
				if ui.button(lbl_imp_proj).clicked() {
					self.pending_psd_project_import = true;
					self.open_file_dialog(FileDialogMode::OpenModel);
					ui.close_menu();
				}
				if ui.button(lbl_open_no).clicked() {
					self.pending_psd_project_import = false;
					self.open_file_dialog(FileDialogMode::OpenModel);
					ui.close_menu();
				}
				if ui.button(lbl_open_vault).clicked() {
					let root = default_projects_root();
					if let Err(e) = open_folder_in_os(&root) {
						self.set_status(false, e);
					} else {
						self.set_status(true, format!("Opened Backup Vault: {}", root.display()));
					}
					ui.close_menu();
				}
				ui.separator();
				if ui.button("Load Sample VTuber").clicked() {
					if let Ok(s) = load_initial_session("--sample") {
						self.load_session_into_app(s, None, "Loaded built-in sample VTuber puppet".to_owned());
					}
					ui.close_menu();
				}
				let example_models = find_example_models();
				if !example_models.is_empty() {
					ui.menu_button("Example Models", |ui| {
						for (label, path) in &example_models {
							if ui.button(label).clicked() {
								self.open_model_from_path(path);
								ui.close_menu();
							}
						}
						ui.separator();
						if ui.button("Built-in Sample VTuber").clicked() {
							if let Ok(s) = load_initial_session("--sample") {
								self.load_session_into_app(
									s,
									None,
									"Loaded built-in sample VTuber puppet".to_owned(),
								);
							}
							ui.close_menu();
						}
					});
				}
				ui.separator();
				if ui.button(lbl_save).clicked() {
					if let Some(p) = self.watch_path.clone().or_else(|| self.source_path.clone()) {
						self.save_model_to_path(&p);
					} else {
						self.open_file_dialog(FileDialogMode::SaveInpAs);
					}
					ui.close_menu();
				}
				if ui.button(lbl_save_as).clicked() {
					self.open_file_dialog(FileDialogMode::SaveInpAs);
					ui.close_menu();
				}
				if ui.button("Export .rigjson (IR)...").clicked() {
					self.open_file_dialog(FileDialogMode::ExportRigJson);
					ui.close_menu();
				}
				if ui.button("Export Layered .psd...").clicked() {
					self.open_file_dialog(FileDialogMode::ExportPsd);
					ui.close_menu();
				}
				ui.separator();
				if ui.button("Export Spec Card PNG (FigCraft)").clicked() {
					self.exec_cmd(
						"craft.figcraft_card",
						json::object! { out: "studio_spec_card.png" },
						"Exported studio_spec_card.png via FigCraft",
					);
					ui.close_menu();
				}
				if ui.button("Export Animated Idle GIF").clicked() {
					let anim_name = self.active_anim.clone();
					self.exec_cmd(
						"render.gif",
						json::object! { out: "studio_idle.gif", anim: anim_name.as_str(), frames: 24, fps: 15 },
						"Exported studio_idle.gif",
					);
					ui.close_menu();
				}
			});

			let lbl_edit = self.i18n.t("menu.edit").to_owned();
			let lbl_undo = self.i18n.t("menu.edit.undo").to_owned();
			let lbl_redo = self.i18n.t("menu.edit.redo").to_owned();
			let lbl_reset_pose = self.i18n.t("menu.edit.reset_pose").to_owned();
			ui.menu_button(lbl_edit, |ui| {
				if ui.button(lbl_undo).clicked() {
					self.exec_cmd("edit.undo", json::object! {}, "Undid last edit");
					ui.close_menu();
				}
				if ui.button(lbl_redo).clicked() {
					self.exec_cmd("edit.redo", json::object! {}, "Redid edit");
					ui.close_menu();
				}
				if ui.button(lbl_reset_pose).clicked() {
					self.session.param_overrides.clear();
					self.cached_rev = u64::MAX;
					self.set_status(true, "Reset parameter pose to neutral");
					ui.close_menu();
				}
			});

			let lbl_view = self.i18n.t("menu.view").to_owned();
			let lbl_lang_menu = self.i18n.t("menu.view.language").to_owned();
			let langs = self.i18n.available_languages();
			let cur_lang = self.i18n.current_lang().to_owned();
			ui.menu_button(lbl_view, |ui| {
				ui.checkbox(&mut self.wireframe, "Show Render Wireframe");
				ui.checkbox(&mut self.xray_mode, "X-Ray Geometric Diagnostic");
				ui.checkbox(&mut self.exact_composite, "CPU Exact FX (Reference)");
				ui.checkbox(&mut self.live_physics, "Live Physics Preview");
				ui.checkbox(&mut self.show_sel_overlay, "Selected Node Vector Overlay");
				ui.menu_button(lbl_lang_menu, |ui| {
					for (code, name) in langs {
						let selected = code == cur_lang;
						if ui.selectable_label(selected, name).clicked() {
							self.i18n.set_language(&code);
							self.save_user_settings();
							ui.close_menu();
						}
					}
				});
				if ui.button("Fit Puppet to Stage (Z)").clicked() {
					self.fit_puppet_scale();
					self.cached_rev = u64::MAX;
					ui.close_menu();
				}
			});

			let lbl_rig = self.i18n.t("menu.rig").to_owned();
			ui.menu_button(lbl_rig, |ui| {
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

			let lbl_audit = self.i18n.t("menu.audit").to_owned();
			ui.menu_button(lbl_audit, |ui| {
				if ui.button("Run 10-Point Rig Audit").clicked() {
					self.audit_rev = u64::MAX;
					self.refresh_audit();
					self.inspector_state.tab = InspectorTab::Audit;
					let s = self.audit_cache["score"].as_u32().unwrap_or(0);
					self.set_status(s >= 80, format!("10-point audit score: {s}/100"));
					ui.close_menu();
				}
				if ui.button("1-Click Constraint Auto-Fix").clicked() {
					self.exec_cmd("rig.auto_fix", json::object! {}, "Ran deterministic rig constraint auto-fix");
					ui.close_menu();
				}
			});

			let lbl_overlays = self.i18n.t("menu.overlays").to_owned();
			ui.menu_button(lbl_overlays, |ui| {
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

			let lbl_commands = self.i18n.t("menu.commands").to_owned();
			if ui.button(lbl_commands).clicked() {
				self.cmd_palette_open = true;
				self.cmd_query.clear();
			}

			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				let is_pro = self.workspace_mode == WorkspaceMode::Pro;
				let lbl_pro = self.i18n.t("mode.pro").to_owned();
				let pro_btn = egui::Button::new(
					egui::RichText::new(lbl_pro)
						.size(11.0)
						.color(if is_pro { Color32::WHITE } else { TEXT_MUTED }),
				)
				.fill(if is_pro { BTN_ACTIVE } else { BG_WIDGET });
				if ui.add(pro_btn).on_hover_text("Pro Rigger & Timeline mode (F2)").clicked() && !is_pro {
					self.set_workspace_mode(WorkspaceMode::Pro);
				}

				let is_quick = !is_pro;
				let lbl_quick = self.i18n.t("mode.quick").to_owned();
				let quick_btn = egui::Button::new(
					egui::RichText::new(lbl_quick)
						.size(11.0)
						.color(if is_quick { Color32::WHITE } else { TEXT_MUTED }),
				)
				.fill(if is_quick { BTN_ACTIVE } else { BG_WIDGET });
				if ui
					.add(quick_btn)
					.on_hover_text("Quick Inspect, Audit & Booster mode (F2)")
					.clicked() && !is_quick
				{
					self.set_workspace_mode(WorkspaceMode::Quick);
				}

				if is_quick {
					ui.separator();
					let lbl_outliner = self.i18n.t("bar.outliner").to_owned();
					let out_btn = egui::Button::new(
						egui::RichText::new(lbl_outliner)
							.size(10.5)
							.color(if self.quick_show_outliner { Color32::WHITE } else { TEXT_MUTED }),
					)
					.fill(if self.quick_show_outliner { BTN_ACTIVE } else { BG_WIDGET });
					if ui.add(out_btn).clicked() {
						self.quick_show_outliner = !self.quick_show_outliner;
					}

					let lbl_phys = self.i18n.t("bar.live_phys").to_owned();
					let phys_btn = egui::Button::new(
						egui::RichText::new(lbl_phys)
							.size(10.5)
							.color(if self.live_physics { Color32::WHITE } else { TEXT_MUTED }),
					)
					.fill(if self.live_physics { AUDIT_OK_GREEN.linear_multiply(0.5) } else { BG_WIDGET });
					if ui.add(phys_btn).clicked() {
						self.live_physics = !self.live_physics;
					}

					let play_lbl = if self.playing {
						self.i18n.t("bar.pause").to_owned()
					} else {
						self.i18n.t("bar.play_idle").to_owned()
					};
					if ui.small_button(play_lbl).clicked() {
						self.playing = !self.playing;
					}
				}

				ui.separator();
				let lbl_fx = self.i18n.t("bar.booster_fx").to_owned();
				let fx_btn = egui::Button::new(
					egui::RichText::new(lbl_fx)
						.size(10.5)
						.color(if self.booster_live_fx { Color32::WHITE } else { TEXT_MUTED }),
				)
				.fill(if self.booster_live_fx { TIMELINE_VIOLET } else { BG_WIDGET });
				if ui
					.add(fx_btn)
					.on_hover_text("Toggle live Booster wgpu 2.5D Rim Light & Soft-Body stage preview")
					.clicked()
				{
					self.booster_live_fx = !self.booster_live_fx;
					self.cached_sig = u64::MAX;
					let msg = if self.booster_live_fx {
						"Enabled live Booster GPU FX preview on stage"
					} else {
						"Disabled Booster GPU FX preview (Editor Raw stage)"
					};
					self.set_status(true, msg);
				}
			});
		});
	}

	pub(crate) fn model_display_name(&self) -> String {
		if let Some(meta_name) = self
			.session
			.model
			.puppet
			.meta
			.name
			.as_deref()
			.map(clean_display_name)
			.filter(|s| !s.is_empty())
		{
			return meta_name.to_owned();
		}
		if let Some(stem) = self
			.source_path
			.as_ref()
			.and_then(|p| p.file_stem())
			.and_then(|s| s.to_str())
			.map(clean_display_name)
			.filter(|s| !s.is_empty())
		{
			return stem.to_owned();
		}
		"SampleVTuber".to_owned()
	}

	pub(crate) fn draw_context_bar(&mut self, ui: &mut egui::Ui) {
		ui.horizontal(|ui| {
			let base_name = self.model_display_name();
			let dirty = self.session.revision != self.last_saved_rev;
			let pname = if dirty {
				format!("{base_name}*")
			} else {
				base_name
			};
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
				.map(|n| clean_display_name(&n.name))
				.filter(|s| !s.is_empty())
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
					for mode in [
						DeformBrushMode::Grab,
						DeformBrushMode::Smooth,
						DeformBrushMode::Bulge,
						DeformBrushMode::Twist,
					] {
						ui.selectable_value(&mut self.brush_mode, mode, mode.label());
					}
					ui.separator();
					let mut param_keys: Vec<String> = self.session.model.puppet.params.keys().cloned().collect();
					param_keys.sort();
					egui::ComboBox::from_id_source("ctx_brush_param")
						.selected_text(self.brush_param.as_str())
						.width(132.0)
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
						if ui
							.add(egui::Label::new(egui::RichText::new(label).color(fg).monospace().strong().size(11.0)).sense(Sense::click()))
							.on_hover_text("Open 10-Point Audit & Diagnostics")
							.clicked()
						{
							self.inspector_state.tab = InspectorTab::Audit;
						}
					});

				let warn_n = self.session.model.puppet.diagnostics.len();
				if warn_n > 0
					&& ui
						.small_button(egui::RichText::new(format!("! {warn_n} warn")).color(AUDIT_WARN_AMBER).monospace())
						.on_hover_text("View Phase 0 load & compatibility warnings")
						.clicked()
				{
					self.inspector_state.tab = InspectorTab::Audit;
				}

				ui.checkbox(&mut self.live_physics, "Live Phys");
				ui.checkbox(&mut self.exact_composite, "CPU Exact FX")
					.on_hover_text("Optional CPU reference renderer (GPU FBO+Stencil stage with mipmaps & 2x SSAA is active by default)");
				ui.checkbox(&mut self.xray_mode, "X-Ray");
				ui.checkbox(&mut self.wireframe, "Wireframe");
			});
		});
	}

	pub(crate) fn apply_inspector_actions(&mut self, actions: Vec<InspectorAction>) {
		for act in actions {
			match act {
				InspectorAction::ExecCmd { cmd, params, ok_label } => {
					self.exec_cmd(cmd, params, &ok_label);
				}
				InspectorAction::TouchRevision => {
					self.session.model.puppet.reset_contexts();
					self.session.revision += 1;
					self.cached_rev = u64::MAX;
					self.audit_rev = u64::MAX;
				}
				InspectorAction::FinishGestureUndo => {
					self.finish_gesture_undo();
				}
				InspectorAction::RefreshAudit => {
					self.audit_rev = u64::MAX;
					self.refresh_audit();
				}
				InspectorAction::SelectNode(uid) => {
					self.selected_node = Some(uid);
					self.selected_vertices.clear();
					if self.session.model.puppet.node_comps.get::<SimplePhysics>(uid).is_some() {
						self.selected_physics = Some(uid);
					}
				}
				InspectorAction::ArmDeformBrush(param_name) => {
					self.brush_param = param_name;
					self.tool = StudioTool::DeformBrush;
					self.set_status(true, format!("Armed Deform Brush on '{}'", self.brush_param));
				}
				InspectorAction::ArmVertexDeform(param_name) => {
					self.brush_param = param_name;
					self.tool = StudioTool::VertexSubselect;
					self.vertex_mode = VertexEditMode::DeformKeypoint;
					self.set_status(true, format!("Armed Vertex Sculpt on '{}'", self.brush_param));
				}
			}
		}
	}

	pub(crate) fn apply_outliner_actions(&mut self, actions: Vec<OutlinerAction>) {
		for act in actions {
			match act {
				OutlinerAction::Select(uid) => {
					self.selected_node = Some(uid);
					self.selected_vertices.clear();
					if self.session.model.puppet.node_comps.get::<SimplePhysics>(uid).is_some() {
						self.selected_physics = Some(uid);
					}
				}
				OutlinerAction::ToggleCollapse(uid) => {
					if !self.collapsed_nodes.insert(uid) {
						self.collapsed_nodes.remove(&uid);
					}
				}
				OutlinerAction::ToggleEnabled(uid, en) => {
					self.exec_cmd(
						"node.configure",
						json::object! { node_uuid: uid.0, enabled: en },
						"Toggled node visibility",
					);
				}
				OutlinerAction::SetZSort(uid, z) => {
					self.exec_cmd(
						"node.configure",
						json::object! { node_uuid: uid.0, zsort: z },
						&format!("Updated draw order z={z:.1}"),
					);
				}
				OutlinerAction::AddPart => {
					let pname = format!("Part_{}", self.session.model.puppet.nodes.iter().count());
					self.exec_cmd(
						"node.add_part",
						json::object! { name: pname.as_str(), width: 80.0, height: 80.0, color: [160, 200, 255, 255] },
						"Added new Part node",
					);
				}
				OutlinerAction::AddGridDeformer => {
					if let Some(id) = self.selected_node {
						if let Some(n) = self.session.model.puppet.nodes.get_node(id).cloned() {
							self.exec_cmd(
								"node.add_grid_deformer",
								json::object! {
									name: format!("{}_Grid", n.name),
									children: [id.0],
									cols: 3,
									rows: 3
								},
								"Wrapped selected node inside GridDeformer",
							);
						}
					} else {
						self.set_status(false, "Select a child node first to wrap in GridDeformer");
					}
				}
				OutlinerAction::AddPathDeformer => {
					if let Some(id) = self.selected_node {
						if let Some(n) = self.session.model.puppet.nodes.get_node(id).cloned() {
							self.exec_cmd(
								"node.add_path_deformer",
								json::object! {
									name: format!("{}_Spline", n.name),
									children: [id.0],
									points: 4,
									curve_type: "Spline"
								},
								"Wrapped selected node inside PathDeformer",
							);
						}
					} else {
						self.set_status(false, "Select a child node first to wrap in PathDeformer");
					}
				}
				OutlinerAction::AddMeshGroup => {
					if let Some(id) = self.selected_node {
						if let Some(n) = self.session.model.puppet.nodes.get_node(id).cloned() {
							self.exec_cmd(
								"node.add_meshgroup",
								json::object! {
									name: format!("{}_Cage", n.name),
									children: [id.0],
									cols: 4,
									rows: 4
								},
								"Wrapped selected node inside MeshGroup",
							);
						}
					} else {
						self.set_status(false, "Select a child node first to wrap in MeshGroup");
					}
				}
				OutlinerAction::DeleteNode(id) => {
					self.exec_cmd("node.remove", json::object! { node_uuid: id.0 }, "Removed selected node");
					if self.selected_node == Some(id) {
						self.selected_node = None;
					}
					self.selected_vertices.clear();
				}
				OutlinerAction::RunDelaunay(id) => {
					self.selected_node = Some(id);
					self.exec_cmd(
						"mesh.auto_delaunay",
						json::object! { node_uuid: id.0, step_px: 20.0 },
						"Rebuilt Part mesh via Bowyer-Watson Delaunay",
					);
				}
			}
		}
	}

	pub(crate) fn draw_right_inspector(&mut self, ui: &mut egui::Ui) {
		let tab_props = self.i18n.t("tab.properties").to_owned();
		let tab_binds = self.i18n.t("tab.bindings").to_owned();
		let tab_audit = self.i18n.t("tab.audit").to_owned();
		let tab_booster = self.i18n.t("tab.booster").to_owned();
		ui.horizontal_wrapped(|ui| {
			ui.selectable_value(&mut self.inspector_state.tab, InspectorTab::Properties, tab_props);
			if self.workspace_mode == WorkspaceMode::Pro {
				ui.selectable_value(&mut self.inspector_state.tab, InspectorTab::Bindings, tab_binds);
			}
			let score = self.audit_cache["score"].as_u32().unwrap_or(0);
			let warns = self.session.model.puppet.diagnostics.len();
			let audit_lbl = if warns > 0 {
				format!("{tab_audit} ({score}) !{warns}")
			} else {
				format!("{tab_audit} ({score})")
			};
			ui.selectable_value(&mut self.inspector_state.tab, InspectorTab::Audit, audit_lbl);
			ui.selectable_value(&mut self.inspector_state.tab, InspectorTab::Booster, tab_booster);
		});
		ui.separator();

		egui::ScrollArea::vertical().id_source("right_insp_scroll").show(ui, |ui| {
			match self.inspector_state.tab {
				InspectorTab::Properties => {
					let actions = draw_properties_tab(
						ui,
						&mut self.session,
						self.selected_node,
						&mut self.inspector_state,
						&mut self.gesture_snapshot,
					);
					self.apply_inspector_actions(actions);

					let mut vec2_choices: Vec<String> = self
						.session
						.model
						.puppet
						.params
						.iter()
						.filter(|(_, p)| p.is_vec2)
						.map(|(k, _)| k.clone())
						.collect();
					vec2_choices.sort();
					for slot in 0..2 {
						if !vec2_choices.is_empty() && !self.session.model.puppet.params.contains_key(&self.pad_params[slot]) {
							let defaults = pick_default_pad_params(&self.session.model.puppet);
							self.pad_params[slot] = defaults[slot].clone();
						}
					}

					ui.add_space(6.0);
					section_header(ui, "2D Parameter Pads", Some(("snap · dbl-click reset", TEXT_MUTED)));
					ui.add_space(4.0);
					ui.horizontal(|ui| {
						for (slot, pad_col) in [(0usize, ACCENT_BLUE), (1usize, TIMELINE_VIOLET)] {
							ui.vertical(|ui| {
								let combo_id = if slot == 0 { "xy_pad_param_0" } else { "xy_pad_param_1" };
								egui::ComboBox::from_id_source(combo_id)
									.selected_text(self.pad_params[slot].as_str())
									.width(124.0)
									.show_ui(ui, |ui| {
										for cname in &vec2_choices {
											ui.selectable_value(&mut self.pad_params[slot], cname.clone(), cname);
										}
									});
								let pname = self.pad_params[slot].clone();
								self.draw_xy_pad(ui, &pname, 124.0, pad_col);
							});
							if slot == 0 {
								ui.add_space(8.0);
							}
						}
					});
					ui.horizontal_wrapped(|ui| {
						for (label, vx, vy) in VISEME_PRESETS {
							if ui.small_button(label).clicked() {
								for (target, raw_val) in [
									("Mouth:: Viseme", gvec2(vx, vy)),
									("Mouth:: Shape", gvec2(vx, vy)),
									("Mouth:: Form", gvec2(vx, 0.0)),
									("Mouth:: Open", gvec2(vy, 0.0)),
								] {
									if let Some(p) = self.session.model.puppet.params.get(target) {
										let clamped = gvec2(
											raw_val.x.clamp(p.min.x, p.max.x),
											raw_val.y.clamp(p.min.y, p.max.y),
										);
										self.session.param_overrides.insert(target.to_owned(), clamped);
									}
								}
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
						if pname == self.pad_params[0] || pname == self.pad_params[1] {
							continue;
						}
						let cur = self.session.param_overrides.get(&pname).copied().unwrap_or(pdef);
						let mut x = cur.x;
						let mut y = cur.y;
						let short = pname.as_str();
						if !is_vec2 {
							ui.horizontal(|ui| {
								ui.add_sized(
									[110.0, 16.0],
									egui::Label::new(egui::RichText::new(short).monospace().size(10.5).color(TEXT_MUTED)),
								);
								if ui.add(egui::Slider::new(&mut x, pmin.x..=pmax.x).show_value(true)).changed() {
									self.session.param_overrides.insert(pname.clone(), gvec2(x, 0.0));
								}
							});
						} else {
							ui.horizontal(|ui| {
								ui.add_sized(
									[110.0, 16.0],
									egui::Label::new(egui::RichText::new(short).monospace().size(10.0).color(TEXT_MUTED)),
								);
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
				}
				InspectorTab::Bindings => {
					let actions = draw_bindings_tab(
						ui,
						&mut self.session,
						self.selected_node,
						&mut self.inspector_state,
					);
					self.apply_inspector_actions(actions);
				}
				InspectorTab::Audit => {
					let actions = draw_audit_tab(ui, &self.session, &self.audit_cache);
					self.apply_inspector_actions(actions);
				}
				InspectorTab::Booster => {
					booster_panel::draw_booster_tab(
						ui,
						&mut self.session.booster,
						&mut self.booster_ui,
						&mut self.status_msg,
					);
				}
			}
		});
	}

	pub(crate) fn draw_command_palette(&mut self, ctx: &egui::Context) {
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
								| "puppet.diagnostics"
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

	pub(crate) fn draw_welcome_screen(&mut self, ctx: &egui::Context) {
		let projects_root = default_projects_root();
		let projects = list_projects(&projects_root);
		let langs = self.i18n.available_languages();
		let mut next_lang: Option<String> = None;

		let title = self.i18n.t("welcome.title").to_owned();
		let subtitle = self.i18n.t("welcome.subtitle").to_owned();
		let lbl_lang = self.i18n.t("welcome.language").to_owned();
		let lbl_cur_lang = self.i18n.current_display_name().to_owned();
		let cur_code = self.i18n.current_lang().to_owned();
		let lbl_import_proj = self.i18n.t("welcome.import_psd_project").to_owned();
		let hint_import_proj = self.i18n.t("welcome.import_psd_hint").to_owned();
		let lbl_open_no = self.i18n.t("welcome.open_no_project").to_owned();
		let hint_open_no = self.i18n.t("welcome.open_no_project_hint").to_owned();
		let lbl_new_sample = self.i18n.t("welcome.new_sample").to_owned();
		let lbl_vault = self.i18n.t("welcome.backup_vault").to_owned();
		let hint_vault = self.i18n.t("welcome.backup_vault_hint").to_owned();
		let lbl_existing = self.i18n.t("welcome.existing_projects").to_owned();
		let lbl_no_proj = self.i18n.t("welcome.no_projects").to_owned();
		let lbl_open_btn = self.i18n.t("welcome.open_btn").to_owned();
		let lbl_folder_btn = self.i18n.t("welcome.folder_btn").to_owned();
		let lbl_continue = self.i18n.t("welcome.continue_stage").to_owned();

		egui::Window::new(title)
			.id(egui::Id::new("welcome_riglens_window"))
			.collapsible(false)
			.resizable(false)
			.anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
			.fixed_size(egui::vec2(580.0, 440.0))
			.show(ctx, |ui| {
				ui.horizontal(|ui| {
					ui.heading("RigLens Editor");
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						egui::ComboBox::from_id_source("welcome_lang_combo")
							.selected_text(lbl_cur_lang)
							.width(135.0)
							.show_ui(ui, |ui| {
								for (code, display) in langs {
									if ui.selectable_label(code == cur_code, display).clicked() {
										next_lang = Some(code);
									}
								}
							});
						ui.label(egui::RichText::new(lbl_lang).size(11.0).color(TEXT_MUTED));
					});
				});
				ui.label(egui::RichText::new(subtitle).size(11.5).color(TEXT_MUTED));
				ui.add_space(10.0);

				ui.horizontal_wrapped(|ui| {
					let proj_btn = egui::Button::new(
						egui::RichText::new(lbl_import_proj).strong().color(Color32::WHITE),
					)
					.fill(BTN_ACTIVE);
					if ui.add(proj_btn).on_hover_text(hint_import_proj).clicked() {
						self.pending_psd_project_import = true;
						self.open_file_dialog(FileDialogMode::OpenModel);
					}

					if ui.button(lbl_open_no).on_hover_text(hint_open_no).clicked() {
						self.pending_psd_project_import = false;
						self.open_file_dialog(FileDialogMode::OpenModel);
					}

					if ui.button(lbl_new_sample).clicked() {
						if let Ok(s) = load_initial_session("--sample") {
							self.active_project = None;
							self.show_welcome = false;
							self.load_session_into_app(s, None, "Started Scratch Sample VTuber (No-Project)".to_owned());
						}
					}

					let vault_btn = egui::Button::new(
						egui::RichText::new(lbl_vault).color(Color32::WHITE),
					)
					.fill(TIMELINE_VIOLET);
					if ui.add(vault_btn).on_hover_text(hint_vault).clicked() {
						if let Err(e) = open_folder_in_os(&projects_root) {
							self.set_status(false, e);
						} else {
							self.set_status(true, format!("Opened Backup Vault: {}", projects_root.display()));
						}
					}
				});

				ui.add_space(6.0);
				ui.label(
					egui::RichText::new(format!("Projects Root: {}", projects_root.display()))
						.monospace()
						.size(10.0)
						.color(TEXT_MUTED),
				);
				ui.separator();

				section_header(ui, &lbl_existing, None);
				egui::ScrollArea::vertical().max_height(210.0).show(ui, |ui| {
					if projects.is_empty() {
						ui.label(egui::RichText::new(lbl_no_proj).size(11.0).color(TEXT_MUTED));
					} else {
						for proj in projects {
							ui.horizontal(|ui| {
								ui.label(egui::RichText::new(&proj.name).strong().size(12.0));
								if proj.has_vault_psd {
									ui.label(egui::RichText::new("[vault PSD]").monospace().size(10.0).color(AUDIT_OK_GREEN));
								}
								ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
									if ui.small_button(&lbl_folder_btn).clicked() {
										let _ = open_folder_in_os(&proj.root_dir);
									}
									if let Some(ref inp_path) = proj.working_inp {
										if ui.small_button(&lbl_open_btn).clicked() {
											let ws = crate::project::ProjectWorkspace {
												name: proj.name.clone(),
												root_dir: proj.root_dir.clone(),
												production_dir: proj.root_dir.join("production"),
												vault_dir: proj.root_dir.join("vault"),
												working_inp: inp_path.clone(),
												vault_psd: None,
											};
											self.active_project = Some(ws);
											self.show_welcome = false;
											self.open_model_from_path(inp_path);
										}
									}
								});
							});
						}
					}
				});

				ui.add_space(8.0);
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if ui.button(lbl_continue).clicked() {
						self.show_welcome = false;
					}
				});
			});

		if let Some(code) = next_lang {
			if self.i18n.set_language(&code) {
				self.save_user_settings();
			}
		}
	}
}

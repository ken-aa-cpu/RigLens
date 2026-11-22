#![forbid(unsafe_code)]

use egui::{self, Color32, Stroke};
use glam::vec2;
use riglens_core::node::components::{
	BlendMode, Drawable, GridDeformer, MaskMode, Mesh, MeshGroup, PathDeformer, RawNodeData, TexturedMesh,
};
use riglens_core::node::InoxNodeUuid;
use riglens_core::params::BindingValues;
use riglens_mcp::engine::{ModelSnapshot, Session};
use riglens_mcp::rig::{binding_kind_name, set_scalar_binding_keypoint};
use json::JsonValue;

use crate::theme::{
	section_header, ACCENT_BLUE, AUDIT_ERR_ROSE, AUDIT_OK_GREEN, AUDIT_WARN_AMBER, BG_WIDGET, BTN_ACTIVE, TEXT_MUTED,
	TEXT_PRIMARY, TIMELINE_EMERALD, TIMELINE_VIOLET,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InspectorTab {
	Properties,
	Bindings,
	Audit,
	Booster,
}

pub enum InspectorAction {
	ExecCmd {
		cmd: &'static str,
		params: JsonValue,
		ok_label: String,
	},
	TouchRevision,
	RefreshView,
	FinishGestureUndo,
	RefreshAudit,
	SelectNode(InoxNodeUuid),
	ArmDeformBrush(String),
	ArmVertexDeform(String),
}

pub struct BindingEditorState {
	pub tab: InspectorTab,
	pub selected_param: String,
	pub selected_cell: (usize, usize),
	pub new_param_open: bool,
	pub new_param_name: String,
	pub new_param_vec2: bool,
	pub add_binding_kind: String,
	pub mask_add_source: Option<InoxNodeUuid>,
	pub rename_buf: String,
	pub rename_node_id: Option<InoxNodeUuid>,
	pub copied_kp: Option<(InoxNodeUuid, String, usize, usize)>,
	pub tint_editing: bool,
}

impl Default for BindingEditorState {
	fn default() -> Self {
		Self {
			tab: InspectorTab::Properties,
			selected_param: "Head:: Yaw-Pitch".to_owned(),
			selected_cell: (1, 1),
			new_param_open: false,
			new_param_name: String::new(),
			new_param_vec2: false,
			add_binding_kind: "TransformTX".to_owned(),
			mask_add_source: None,
			rename_buf: String::new(),
			rename_node_id: None,
			copied_kp: None,
			tint_editing: false,
		}
	}
}

const SCALAR_KINDS: [&str; 9] = [
	"TransformTX",
	"TransformTY",
	"TransformSX",
	"TransformSY",
	"TransformRX",
	"TransformRY",
	"TransformRZ",
	"ZSort",
	"Opacity",
];

fn begin_gesture(session: &Session, snap: &mut Option<ModelSnapshot>, start_rev: &mut u64) {
	if snap.is_none() {
		*start_rev = session.revision;
		*snap = Some(session.snapshot());
	}
}

pub fn draw_properties_tab(
	ui: &mut egui::Ui,
	session: &mut Session,
	selected_node: Option<InoxNodeUuid>,
	state: &mut BindingEditorState,
	gesture_snap: &mut Option<ModelSnapshot>,
	gesture_start_rev: &mut u64,
	i18n: &crate::i18n::I18n,
) -> Vec<InspectorAction> {
	let mut actions = Vec::new();
	section_header(ui, i18n.t("insp.node_properties"), None);
	ui.add_space(4.0);

	let Some(id) = selected_node else {
		ui.label(
			egui::RichText::new(i18n.t("insp.select_node"))
				.color(TEXT_MUTED)
				.size(11.0),
		);
		return actions;
	};

	let Some(node) = session.model.puppet.nodes.get_node(id).cloned() else {
		ui.label(egui::RichText::new(i18n.t("insp.missing_node")).color(TEXT_MUTED).size(11.0));
		return actions;
	};

	if state.rename_node_id != Some(id) {
		state.rename_node_id = Some(id);
		state.rename_buf = node.name.trim_end_matches('\0').to_owned();
	}

	let comps = &session.model.puppet.node_comps;
	let is_mesh = comps.get::<TexturedMesh>(id).is_some();
	let is_mg = comps.get::<MeshGroup>(id).is_some();
	let is_gd = comps.get::<GridDeformer>(id).is_some();
	let is_pd = comps.get::<PathDeformer>(id).is_some();
	let kind_tag: String = if let Some(raw) = comps.get::<RawNodeData>(id) {
		raw.type_name.clone()
	} else if is_mg {
		"MeshGroup".to_owned()
	} else if is_gd {
		"GridDeformer".to_owned()
	} else if is_pd {
		"PathDeformer".to_owned()
	} else if is_mesh {
		"Part".to_owned()
	} else {
		"Node".to_owned()
	};

	let is_root = id == session.model.puppet.nodes.root_node_id;
	let cur_parent = if is_root {
		id
	} else {
		session.model.puppet.nodes.get_parent(id).uuid
	};
	let cur_parent_name = session
		.model
		.puppet
		.nodes
		.get_node(cur_parent)
		.map(|p| p.name.trim_end_matches('\0').to_owned())
		.unwrap_or_else(|| "Root".to_owned());

	let parent_candidates: Vec<(InoxNodeUuid, String)> = if is_root {
		Vec::new()
	} else {
		session
			.model
			.puppet
			.nodes
			.pre_order_iter()
			.filter(|cand| {
				cand.uuid != id && !session.model.puppet.nodes.is_descendant(cand.uuid, id)
			})
			.map(|cand| (cand.uuid, cand.name.trim_end_matches('\0').to_owned()))
			.collect()
	};

	let mut tx = node.trans_offset.translation.x;
	let mut ty = node.trans_offset.translation.y;
	let mut rz_deg = node.trans_offset.rotation.z.to_degrees();
	let mut sx = node.trans_offset.scale.x;
	let mut sy = node.trans_offset.scale.y;
	let mut zsort = node.zsort;
	let drawable_opt = session.model.puppet.node_comps.get::<Drawable>(id).cloned();

	egui::Grid::new("p4_prop_grid")
		.num_columns(2)
		.spacing([8.0, 5.0])
		.show(ui, |ui| {
			ui.label(egui::RichText::new(i18n.t("insp.name")).color(TEXT_MUTED).size(11.0));
			ui.horizontal(|ui| {
				let nr = ui.add_sized([132.0, 18.0], egui::TextEdit::singleline(&mut state.rename_buf));
				let commit_rename = (nr.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)))
					&& !state.rename_buf.trim().is_empty()
					&& state.rename_buf.trim() != node.name.trim_end_matches('\0');
				if commit_rename {
					actions.push(InspectorAction::ExecCmd {
						cmd: "node.configure",
						params: json::object! { node_uuid: id.0, name: state.rename_buf.trim() },
						ok_label: format!("Renamed node to '{}'", state.rename_buf.trim()),
					});
				}
				ui.label(egui::RichText::new(format!("{kind_tag} #{}", id.0)).monospace().size(10.0).color(ACCENT_BLUE));
			});
			ui.end_row();

			if !is_root {
				ui.label(egui::RichText::new(i18n.t("insp.parent")).color(TEXT_MUTED).size(11.0));
				egui::ComboBox::from_id_source("prop_parent_combo")
					.selected_text(&cur_parent_name)
					.width(150.0)
					.show_ui(ui, |ui| {
						for (pid, pname) in &parent_candidates {
							if ui.selectable_label(*pid == cur_parent, pname).clicked() && *pid != cur_parent {
								actions.push(InspectorAction::ExecCmd {
									cmd: "node.configure",
									params: json::object! { node_uuid: id.0, parent_uuid: pid.0 },
									ok_label: format!("Reparented '{}' -> '{}'", node.name.trim_end_matches('\0'), pname),
								});
							}
						}
					});
				ui.end_row();
			}

			ui.label(egui::RichText::new(i18n.t("insp.flags")).color(TEXT_MUTED).size(11.0));
			ui.horizontal(|ui| {
				let mut enabled = node.enabled;
				if ui.checkbox(&mut enabled, i18n.t("insp.visible")).changed() {
					actions.push(InspectorAction::ExecCmd {
						cmd: "node.configure",
						params: json::object! { node_uuid: id.0, enabled: enabled },
						ok_label: "Updated visibility".to_owned(),
					});
				}
				let mut ltr = node.lock_to_root;
				if ui.checkbox(&mut ltr, i18n.t("insp.lock_root")).changed() {
					actions.push(InspectorAction::ExecCmd {
						cmd: "node.configure",
						params: json::object! { node_uuid: id.0, lock_to_root: ltr },
						ok_label: "Updated lockToRoot".to_owned(),
					});
				}
			});
			ui.end_row();

			ui.label(egui::RichText::new(i18n.t("insp.offset")).color(TEXT_MUTED).size(11.0));
			ui.horizontal(|ui| {
				let rx = ui.add(egui::DragValue::new(&mut tx).speed(0.5).prefix("X:"));
				let ry = ui.add(egui::DragValue::new(&mut ty).speed(0.5).prefix("Y:"));
				if rx.drag_started() || ry.drag_started() {
					begin_gesture(session, gesture_snap, gesture_start_rev);
				}
				if rx.changed() || ry.changed() {
					begin_gesture(session, gesture_snap, gesture_start_rev);
					if let Some(n) = session.model.puppet.nodes.get_node_mut(id) {
						n.trans_offset.translation.x = tx;
						n.trans_offset.translation.y = ty;
					}
					actions.push(InspectorAction::TouchRevision);
					if !rx.dragged() && !ry.dragged() && !rx.has_focus() && !ry.has_focus() {
						actions.push(InspectorAction::FinishGestureUndo);
					}
				}
				for r in [&rx, &ry] {
					if r.drag_stopped() || r.lost_focus() {
						actions.push(InspectorAction::FinishGestureUndo);
					}
				}
			});
			ui.end_row();

			ui.label(egui::RichText::new(i18n.t("insp.scale")).color(TEXT_MUTED).size(11.0));
			ui.horizontal(|ui| {
				let rsx = ui.add(egui::DragValue::new(&mut sx).speed(0.01).range(0.05..=10.0).prefix("X:"));
				let rsy = ui.add(egui::DragValue::new(&mut sy).speed(0.01).range(0.05..=10.0).prefix("Y:"));
				if rsx.drag_started() || rsy.drag_started() {
					begin_gesture(session, gesture_snap, gesture_start_rev);
				}
				if rsx.changed() || rsy.changed() {
					begin_gesture(session, gesture_snap, gesture_start_rev);
					if let Some(n) = session.model.puppet.nodes.get_node_mut(id) {
						n.trans_offset.scale.x = sx;
						n.trans_offset.scale.y = sy;
					}
					actions.push(InspectorAction::TouchRevision);
					if !rsx.dragged() && !rsy.dragged() && !rsx.has_focus() && !rsy.has_focus() {
						actions.push(InspectorAction::FinishGestureUndo);
					}
				}
				for r in [&rsx, &rsy] {
					if r.drag_stopped() || r.lost_focus() {
						actions.push(InspectorAction::FinishGestureUndo);
					}
				}
			});
			ui.end_row();

			ui.label(egui::RichText::new(i18n.t("insp.rot_z")).color(TEXT_MUTED).size(11.0));
			ui.horizontal(|ui| {
				let rrot = ui.add(egui::DragValue::new(&mut rz_deg).speed(0.5).suffix(" deg"));
				let rz = ui.add(egui::DragValue::new(&mut zsort).speed(0.25).prefix("Z:"));
				if rrot.drag_started() || rz.drag_started() {
					begin_gesture(session, gesture_snap, gesture_start_rev);
				}
				if rrot.changed() || rz.changed() {
					begin_gesture(session, gesture_snap, gesture_start_rev);
					if let Some(n) = session.model.puppet.nodes.get_node_mut(id) {
						n.trans_offset.rotation.z = rz_deg.to_radians();
						n.zsort = zsort;
					}
					actions.push(InspectorAction::TouchRevision);
					if !rrot.dragged() && !rz.dragged() && !rrot.has_focus() && !rz.has_focus() {
						actions.push(InspectorAction::FinishGestureUndo);
					}
				}
				for r in [&rrot, &rz] {
					if r.drag_stopped() || r.lost_focus() {
						actions.push(InspectorAction::FinishGestureUndo);
					}
				}
			});
			ui.end_row();
		});

	if let Some(drw) = drawable_opt {
		ui.add_space(4.0);
		section_header(ui, i18n.t("insp.drawable_blending"), None);
		ui.add_space(3.0);

		let mut op = drw.blending.opacity;
		ui.horizontal(|ui| {
			ui.label(egui::RichText::new(i18n.t("insp.opacity")).color(TEXT_MUTED).size(11.0));
			let rop = ui.add(egui::Slider::new(&mut op, 0.0..=1.0));
			if rop.drag_started() {
				begin_gesture(session, gesture_snap, gesture_start_rev);
			}
			if rop.changed() {
				begin_gesture(session, gesture_snap, gesture_start_rev);
				if let Some(d) = session.model.puppet.node_comps.get_mut::<Drawable>(id) {
					d.blending.opacity = op.clamp(0.0, 1.0);
				}
				actions.push(InspectorAction::TouchRevision);
				if !rop.dragged() && !rop.has_focus() {
					actions.push(InspectorAction::FinishGestureUndo);
				}
			}
			if rop.drag_stopped() || rop.lost_focus() {
				actions.push(InspectorAction::FinishGestureUndo);
			}
		});

		ui.horizontal(|ui| {
			ui.label(egui::RichText::new(i18n.t("insp.blend")).color(TEXT_MUTED).size(11.0));
			egui::ComboBox::from_id_source("prop_blend_combo")
				.selected_text(drw.blending.mode.as_str())
				.width(120.0)
				.show_ui(ui, |ui| {
					for bm in BlendMode::VALUES {
						if ui.selectable_label(drw.blending.mode == bm, bm.as_str()).clicked() && bm != drw.blending.mode {
							actions.push(InspectorAction::ExecCmd {
								cmd: "node.configure",
								params: json::object! { node_uuid: id.0, blend_mode: bm.as_str() },
								ok_label: format!("Set blend mode -> {}", bm.as_str()),
							});
						}
					}
				});
			ui.label(egui::RichText::new(i18n.t("insp.tint")).color(TEXT_MUTED).size(11.0));
			let mut rgb = [drw.blending.tint.x, drw.blending.tint.y, drw.blending.tint.z];
			let rtint = ui.color_edit_button_rgb(&mut rgb);
			if rtint.changed() {
				begin_gesture(session, gesture_snap, gesture_start_rev);
				state.tint_editing = true;
				if let Some(d) = session.model.puppet.node_comps.get_mut::<Drawable>(id) {
					d.blending.tint = glam::vec3(rgb[0], rgb[1], rgb[2]);
				}
				actions.push(InspectorAction::TouchRevision);
			}
			if state.tint_editing
				&& !ui.ctx().memory(|m| m.any_popup_open())
				&& !rtint.has_focus()
				&& !ui.ctx().input(|i| i.pointer.any_down())
			{
				state.tint_editing = false;
				actions.push(InspectorAction::FinishGestureUndo);
			}
		});

		ui.add_space(4.0);
		let masks_vec = drw.masks.as_ref().map(|m| m.masks.clone()).unwrap_or_default();
		let mut threshold = drw.masks.as_ref().map(|m| m.threshold).unwrap_or(0.1);
		let mask_badge = format!("{}", masks_vec.len());
		section_header(ui, i18n.t("insp.clipping_masks"), Some((&mask_badge, AUDIT_WARN_AMBER)));
		ui.add_space(3.0);

		let build_mask_sources_json = |items: &[(InoxNodeUuid, MaskMode)]| -> JsonValue {
			JsonValue::Array(
				items
					.iter()
					.map(|(src, mode)| {
						json::object! {
							uuid: src.0,
							mode: if *mode == MaskMode::Dodge { "Dodge" } else { "Mask" }
						}
					})
					.collect(),
			)
		};

		let pairs: Vec<(InoxNodeUuid, MaskMode)> = masks_vec.iter().map(|m| (m.source, m.mode)).collect();
		for (mi, (src_id, mode)) in pairs.iter().enumerate() {
			let sname = session
				.model
				.puppet
				.nodes
				.get_node(*src_id)
				.map(|n| n.name.trim_end_matches('\0').to_owned())
				.unwrap_or_else(|| format!("#{}", src_id.0));
			ui.horizontal(|ui| {
				ui.label(egui::RichText::new(&sname).size(11.0).color(TEXT_PRIMARY));
				let mode_lbl = if *mode == MaskMode::Dodge { "Dodge" } else { "Mask" };
				if ui.small_button(mode_lbl).on_hover_text("Toggle Mask / Dodge mode").clicked() {
					let mut updated = pairs.clone();
					updated[mi].1 = if *mode == MaskMode::Dodge {
						MaskMode::Mask
					} else {
						MaskMode::Dodge
					};
					actions.push(InspectorAction::ExecCmd {
						cmd: "node.set_mask",
						params: json::object! {
							node_uuid: id.0,
							sources: build_mask_sources_json(&updated),
							threshold: threshold
						},
						ok_label: "Toggled clipping mask mode".to_owned(),
					});
				}
				if ui.small_button("x").clicked() {
					let mut updated = pairs.clone();
					updated.remove(mi);
					actions.push(InspectorAction::ExecCmd {
						cmd: "node.set_mask",
						params: json::object! {
							node_uuid: id.0,
							sources: build_mask_sources_json(&updated),
							threshold: threshold
						},
						ok_label: format!("Removed mask source '{sname}'"),
					});
				}
			});
		}
		if !pairs.is_empty() {
			ui.horizontal(|ui| {
				ui.label(egui::RichText::new(i18n.t("insp.thresh")).color(TEXT_MUTED).size(10.5));
				let rt = ui.add(egui::Slider::new(&mut threshold, 0.0..=1.0));
				if rt.drag_started() {
					begin_gesture(session, gesture_snap, gesture_start_rev);
				}
				if rt.changed() {
					begin_gesture(session, gesture_snap, gesture_start_rev);
					if let Some(d) = session.model.puppet.node_comps.get_mut::<Drawable>(id) {
						if let Some(ref mut masks) = d.masks {
							masks.threshold = threshold.clamp(0.0, 1.0);
						}
					}
					actions.push(InspectorAction::TouchRevision);
					if !rt.dragged() && !rt.has_focus() {
						actions.push(InspectorAction::FinishGestureUndo);
					}
				}
				if rt.drag_stopped() || rt.lost_focus() {
					actions.push(InspectorAction::FinishGestureUndo);
				}
			});
		}

		let mask_cands: Vec<(InoxNodeUuid, String)> = session
			.model
			.puppet
			.nodes
			.pre_order_iter()
			.filter(|n| n.uuid != id && session.model.puppet.node_comps.get::<Drawable>(n.uuid).is_some())
			.map(|n| (n.uuid, n.name.trim_end_matches('\0').to_owned()))
			.collect();
		if state
			.mask_add_source
			.is_none_or(|sid| !mask_cands.iter().any(|(cid, _)| *cid == sid))
		{
			state.mask_add_source = mask_cands.first().map(|(cid, _)| *cid);
		}
		let sel_cand_name = state
			.mask_add_source
			.and_then(|sid| mask_cands.iter().find(|(cid, _)| *cid == sid).map(|(_, n)| n.clone()))
			.unwrap_or_else(|| i18n.t("insp.none").to_owned());
		ui.horizontal(|ui| {
			egui::ComboBox::from_id_source("mask_add_combo")
				.selected_text(sel_cand_name)
				.width(150.0)
				.show_ui(ui, |ui| {
					for (cid, cname) in &mask_cands {
						ui.selectable_value(&mut state.mask_add_source, Some(*cid), cname);
					}
				});
			if ui.small_button(i18n.t("insp.add_mask")).clicked() {
				if let Some(src) = state.mask_add_source {
					let mut updated = pairs.clone();
					if !updated.iter().any(|(s, _)| *s == src) {
						updated.push((src, MaskMode::Mask));
						actions.push(InspectorAction::ExecCmd {
							cmd: "node.set_mask",
							params: json::object! {
								node_uuid: id.0,
								sources: build_mask_sources_json(&updated),
								threshold: threshold
							},
							ok_label: "Added clipping mask".to_owned(),
						});
					}
				}
			}
		});
	}

	if let Some(m) = session.model.puppet.node_comps.get::<Mesh>(id) {
		let vc = m.vertices.len();
		let tc = m.indices.len() / 3;
		ui.add_space(4.0);
		ui.horizontal(|ui| {
			ui.label(
				egui::RichText::new(format!("Mesh: {vc}v / {tc}t"))
					.monospace()
					.size(10.5)
					.color(TEXT_MUTED),
			);
			if is_mesh && ui.small_button("Delaunay").clicked() {
				actions.push(InspectorAction::ExecCmd {
					cmd: "mesh.auto_delaunay",
					params: json::object! { node_uuid: id.0, step_px: 20.0 },
					ok_label: "Rebuilt Delaunay mesh".to_owned(),
				});
			}
			if is_mesh && ui.small_button("Contour").clicked() {
				actions.push(InspectorAction::ExecCmd {
					cmd: "mesh.auto_contour",
					params: json::object! { node_uuid: id.0, cols: 8, rows: 8 },
					ok_label: "Rebuilt contour mesh".to_owned(),
				});
			}
			if is_mesh && ui.small_button("Grid").clicked() {
				actions.push(InspectorAction::ExecCmd {
					cmd: "mesh.auto_grid",
					params: json::object! { node_uuid: id.0, cols: 6, rows: 6 },
					ok_label: "Rebuilt grid mesh".to_owned(),
				});
			}
		});
	} else if let Some(gd) = session.model.puppet.node_comps.get::<GridDeformer>(id) {
		ui.add_space(4.0);
		ui.label(
			egui::RichText::new(format!(
				"GridDeformer: {}x{} ({} pts, {}) · Use A/D tools on Stage",
				gd.cols(),
				gd.rows(),
				gd.vertex_count(),
				gd.formation.as_str()
			))
			.monospace()
			.size(10.5)
			.color(TIMELINE_VIOLET),
		);
	} else if let Some(pd) = session.model.puppet.node_comps.get::<PathDeformer>(id) {
		ui.add_space(4.0);
		ui.label(
			egui::RichText::new(format!(
				"PathDeformer: {} ({} pts) · Use A/D tools on Stage",
				pd.curve_type.as_str(),
				pd.vertex_count()
			))
			.monospace()
			.size(10.5)
			.color(TIMELINE_EMERALD),
		);
	}

	actions
}

pub fn draw_bindings_tab(
	ui: &mut egui::Ui,
	session: &mut Session,
	selected_node: Option<InoxNodeUuid>,
	state: &mut BindingEditorState,
	gesture_snap: &mut Option<ModelSnapshot>,
	gesture_start_rev: &mut u64,
) -> Vec<InspectorAction> {
	let mut actions = Vec::new();
	section_header(ui, "Bindings & Keypoints", None);
	ui.add_space(4.0);

	let mut param_names: Vec<String> = session.model.puppet.params.keys().cloned().collect();
	param_names.sort();
	if !param_names.is_empty() && !session.model.puppet.params.contains_key(&state.selected_param) {
		state.selected_param = param_names[0].clone();
	}

	ui.horizontal(|ui| {
		ui.label(egui::RichText::new("Param").color(TEXT_MUTED).size(11.0));
		egui::ComboBox::from_id_source("bind_param_combo")
			.selected_text(if state.selected_param.is_empty() { "None" } else { state.selected_param.as_str() })
			.width(180.0)
			.show_ui(ui, |ui| {
				for pn in &param_names {
					ui.selectable_value(&mut state.selected_param, pn.clone(), pn);
				}
			});
		if ui.small_button("+ New").clicked() {
			state.new_param_open = !state.new_param_open;
		}
	});

	if state.new_param_open {
		egui::Frame::none()
			.fill(BG_WIDGET)
			.inner_margin(egui::Margin::same(6.0))
			.show(ui, |ui| {
				ui.horizontal(|ui| {
					ui.add_sized([155.0, 20.0], egui::TextEdit::singleline(&mut state.new_param_name).hint_text("Expr:: Name"));
					ui.checkbox(&mut state.new_param_vec2, "2D");
					if ui.small_button("Create").clicked() && !state.new_param_name.trim().is_empty() {
						let pname = state.new_param_name.trim().to_owned();
						let is_2d = state.new_param_vec2;
						let axis = if is_2d { vec![0.0, 0.5, 1.0] } else { vec![0.0, 1.0] };
						actions.push(InspectorAction::ExecCmd {
							cmd: "param.create",
							params: json::object! {
								name: pname.as_str(),
								is_vec2: is_2d,
								min: if is_2d { vec![-1.0_f32, -1.0] } else { vec![0.0_f32, 0.0] },
								max: vec![1.0_f32, if is_2d { 1.0 } else { 0.0 }],
								axis_x: axis.clone(),
								axis_y: if is_2d { axis } else { vec![0.0_f32] },
							},
							ok_label: format!("Created parameter '{pname}'"),
						});
						state.selected_param = pname;
						state.new_param_name.clear();
						state.new_param_open = false;
					}
				});
			});
	}

	let Some(param) = session.model.puppet.params.get(&state.selected_param).cloned() else {
		ui.label(egui::RichText::new("No parameters in puppet.").color(TEXT_MUTED).size(11.0));
		return actions;
	};

	let x_len = param.axis_points.x.len().max(1);
	let y_len = param.axis_points.y.len().max(1);
	state.selected_cell.0 = state.selected_cell.0.min(x_len - 1);
	state.selected_cell.1 = state.selected_cell.1.min(y_len - 1);

	let Some(node_id) = selected_node else {
		ui.label(egui::RichText::new("Select a node to inspect/edit its bindings.").color(TEXT_MUTED).size(11.0));
		return actions;
	};
	let node_name = session
		.model
		.puppet
		.nodes
		.get_node(node_id)
		.map(|n| n.name.clone())
		.unwrap_or_else(|| format!("#{}", node_id.0));

	ui.label(
		egui::RichText::new(format!(
			"Target: {node_name} · Range X[{:.1}..{:.1}] Y[{:.1}..{:.1}]",
			param.min.x, param.max.x, param.min.y, param.max.y
		))
		.size(10.5)
		.color(TEXT_MUTED),
	);

	// Interactive Keypoint Lattice Grid
	ui.add_space(4.0);
	section_header(ui, "Keypoint Grid (click to snap stage)", None);
	let node_bindings: Vec<_> = param
		.bindings
		.iter()
		.filter(|b| b.node == node_id)
		.cloned()
		.collect();

	egui::Grid::new("kp_lattice_grid").spacing([4.0, 4.0]).show(ui, |ui| {
		for iy in 0..y_len {
			for ix in 0..x_len {
				let is_cur = state.selected_cell == (ix, iy);
				let any_set = node_bindings
					.iter()
					.any(|b| b.is_set.get(ix, iy).copied().unwrap_or(false));
				let ax = param.axis_points.x.get(ix).copied().unwrap_or(0.0);
				let ay = param.axis_points.y.get(iy).copied().unwrap_or(0.0);
				let label = if any_set {
					format!("●{ix},{iy}")
				} else {
					format!("{ix},{iy}")
				};
				let btn = egui::Button::new(
					egui::RichText::new(label)
						.monospace()
						.size(10.0)
						.color(if is_cur {
							Color32::WHITE
						} else if any_set {
							AUDIT_OK_GREEN
						} else {
							TEXT_MUTED
						}),
				)
				.fill(if is_cur { BTN_ACTIVE } else { BG_WIDGET })
				.stroke(Stroke::new(1.0_f32, if is_cur { ACCENT_BLUE } else { Color32::from_rgb(56, 56, 66) }));
				if ui.add(btn).clicked() {
					state.selected_cell = (ix, iy);
					let vx = param.min.x + (param.max.x - param.min.x) * ax;
					let vy = param.min.y + (param.max.y - param.min.y) * ay;
					session.param_overrides.insert(state.selected_param.clone(), vec2(vx, vy));
					actions.push(InspectorAction::RefreshView);
				}
			}
			ui.end_row();
		}
	});

	let (cix, ciy) = state.selected_cell;
	let is_deformable = session.model.puppet.node_comps.get::<Mesh>(node_id).is_some()
		|| session.model.puppet.node_comps.get::<GridDeformer>(node_id).is_some()
		|| session.model.puppet.node_comps.get::<PathDeformer>(node_id).is_some();

	if is_deformable {
		ui.add_space(6.0);
		let deform_b = node_bindings
			.iter()
			.find(|b| matches!(b.values, BindingValues::Deform(_)));
		let deform_set = deform_b
			.and_then(|b| b.is_set.get(cix, ciy).copied())
			.unwrap_or(false);
		let badge = if deform_set {
			("KP SET", AUDIT_OK_GREEN)
		} else {
			("Unset", TEXT_MUTED)
		};
		section_header(ui, "Mesh / Deformer Sculpt", Some(badge));
		ui.horizontal_wrapped(|ui| {
			if ui.small_button("Arm Brush (D)").clicked() {
				actions.push(InspectorAction::ArmDeformBrush(state.selected_param.clone()));
			}
			if ui.small_button("Arm Vertices (A)").clicked() {
				actions.push(InspectorAction::ArmVertexDeform(state.selected_param.clone()));
			}
			if x_len >= 2 && deform_b.is_some() {
				let mirror_dst = x_len - 1 - cix;
				if ui
					.small_button(format!("Mirror X {cix}->{mirror_dst}"))
					.on_hover_text("Symmetric X-mirror of Deform offsets at current row")
					.clicked()
				{
					actions.push(InspectorAction::ExecCmd {
						cmd: "deform.mirror_keypoint",
						params: json::object! {
							node_uuid: node_id.0,
							param: state.selected_param.as_str(),
							src_ix: cix,
							src_iy: ciy,
							dst_ix: mirror_dst,
							dst_iy: ciy,
							mirror_x: true
						},
						ok_label: format!("Mirrored keypoint ({cix},{ciy}) -> ({mirror_dst},{ciy})"),
					});
				}
			}
			if deform_b.is_some() {
				if ui.small_button("Copy KP").clicked() {
					state.copied_kp = Some((node_id, state.selected_param.clone(), cix, ciy));
				}
				if let Some((src_id, ref src_param, src_ix, src_iy)) = state.copied_kp.clone() {
					if src_id == node_id
						&& src_param == &state.selected_param
						&& (src_ix, src_iy) != (cix, ciy)
						&& ui.small_button(format!("Paste ({src_ix},{src_iy})")).clicked()
					{
						actions.push(InspectorAction::ExecCmd {
							cmd: "deform.mirror_keypoint",
							params: json::object! {
								node_uuid: node_id.0,
								param: state.selected_param.as_str(),
								src_ix: src_ix,
								src_iy: src_iy,
								dst_ix: cix,
								dst_iy: ciy,
								mirror_x: false
							},
							ok_label: format!("Copied Deform KP ({src_ix},{src_iy}) -> ({cix},{ciy})"),
						});
					}
				}
				if ui.small_button("Clear KP").clicked() {
					actions.push(InspectorAction::ExecCmd {
						cmd: "param.clear_binding_keypoint",
						params: json::object! {
							node_uuid: node_id.0,
							param: state.selected_param.as_str(),
							kind: "Deform",
							ix: cix,
							iy: ciy
						},
						ok_label: format!("Cleared Deform KP ({cix},{ciy})"),
					});
				}
				if ui.small_button("Del Binding").clicked() {
					actions.push(InspectorAction::ExecCmd {
						cmd: "param.remove_binding",
						params: json::object! {
							node_uuid: node_id.0,
							param: state.selected_param.as_str(),
							kind: "Deform"
						},
						ok_label: "Removed Deform binding".to_owned(),
					});
				}
			}
			if x_len >= 2 && y_len >= 2 && !node_bindings.is_empty() {
				if ui
					.small_button("Synthesize Corners")
					.on_hover_text("Live2D-style bilinear synthesis of 4 corner keypoints from cardinal X/Y axes")
					.clicked()
				{
					actions.push(InspectorAction::ExecCmd {
						cmd: "param.synthesize_corners",
						params: json::object! {
							node_uuid: node_id.0,
							param: state.selected_param.as_str()
						},
						ok_label: format!("Synthesized 2D corners for '{}'", state.selected_param),
					});
				}
			}
		});
	}

	ui.add_space(6.0);
	section_header(ui, &format!("Scalar Bindings @ KP({cix},{ciy})"), None);
	for b in &node_bindings {
		let kind = binding_kind_name(&b.values);
		if kind == "Deform" {
			continue;
		}
		let val_opt = match &b.values {
			BindingValues::ZSort(m)
			| BindingValues::TransformTX(m)
			| BindingValues::TransformTY(m)
			| BindingValues::TransformSX(m)
			| BindingValues::TransformSY(m)
			| BindingValues::TransformRX(m)
			| BindingValues::TransformRY(m)
			| BindingValues::TransformRZ(m)
			| BindingValues::Opacity(m) => m.get(cix, ciy).copied(),
			BindingValues::Deform(_) => None,
		};
		if let Some(mut v) = val_opt {
			let is_kp_set = b.is_set.get(cix, ciy).copied().unwrap_or(false);
			ui.horizontal(|ui| {
				ui.label(
					egui::RichText::new(kind)
						.monospace()
						.size(10.5)
						.color(if is_kp_set { AUDIT_OK_GREEN } else { TEXT_MUTED }),
				);
				let r = ui.add(egui::DragValue::new(&mut v).speed(0.05));
				if r.drag_started() {
					begin_gesture(session, gesture_snap, gesture_start_rev);
				}
				if r.changed() {
					begin_gesture(session, gesture_snap, gesture_start_rev);
					if set_scalar_binding_keypoint(
						&mut session.model.puppet,
						node_id,
						&state.selected_param,
						cix,
						ciy,
						kind,
						v,
					)
					.is_ok()
					{
						actions.push(InspectorAction::TouchRevision);
					}
					if !r.dragged() && !r.has_focus() {
						actions.push(InspectorAction::FinishGestureUndo);
					}
				}
				if r.drag_stopped() || r.lost_focus() {
					actions.push(InspectorAction::FinishGestureUndo);
				}
				if ui.small_button("Clr").clicked() {
					actions.push(InspectorAction::ExecCmd {
						cmd: "param.clear_binding_keypoint",
						params: json::object! {
							node_uuid: node_id.0,
							param: state.selected_param.as_str(),
							kind: kind,
							ix: cix,
							iy: ciy
						},
						ok_label: format!("Cleared {kind} KP"),
					});
				}
				if ui.small_button("x").clicked() {
					actions.push(InspectorAction::ExecCmd {
						cmd: "param.remove_binding",
						params: json::object! {
							node_uuid: node_id.0,
							param: state.selected_param.as_str(),
							kind: kind
						},
						ok_label: format!("Removed {kind} binding"),
					});
				}
			});
		}
	}

	ui.horizontal(|ui| {
		egui::ComboBox::from_id_source("add_scalar_bind_combo")
			.selected_text(state.add_binding_kind.as_str())
			.width(140.0)
			.show_ui(ui, |ui| {
				for k in SCALAR_KINDS {
					ui.selectable_value(&mut state.add_binding_kind, k.to_owned(), k);
				}
			});
		if ui.small_button("+ Bind @ KP").clicked() {
			let neutral = if matches!(
				state.add_binding_kind.as_str(),
				"TransformSX" | "TransformSY" | "Opacity"
			) {
				1.0
			} else {
				0.0
			};
			actions.push(InspectorAction::ExecCmd {
				cmd: "param.set_binding_keypoint",
				params: json::object! {
					node_uuid: node_id.0,
					param: state.selected_param.as_str(),
					kind: state.add_binding_kind.as_str(),
					ix: cix,
					iy: ciy,
					value: neutral
				},
				ok_label: format!("Bound {} at KP ({cix},{ciy})", state.add_binding_kind),
			});
		}
	});

	actions
}

pub fn draw_audit_tab(
	ui: &mut egui::Ui,
	session: &Session,
	audit_cache: &JsonValue,
) -> Vec<InspectorAction> {
	let mut actions = Vec::new();
	let score = audit_cache["score"].as_u32().unwrap_or(0);
	let passed = audit_cache["passed"].as_bool().unwrap_or(false);
	let badge_col = if passed { AUDIT_OK_GREEN } else { AUDIT_WARN_AMBER };
	let badge_str = format!("{score}/100");
	section_header(ui, "10-Point Rig Audit", Some((&badge_str, badge_col)));
	ui.add_space(4.0);

	const AUDIT_CHECKS: [(&str, &str); 10] = [
		("Containment Leaks", "containment_leaks"),
		("Mesh Foldovers", "mesh_foldovers"),
		("Extreme Stretch", "mesh_stretch"),
		("Blink Symmetry", "blink_issues"),
		("Yaw Symmetry", "symmetry_defects"),
		("Clipping Masks", "unmasked_parts"),
		("Deformer Hierarchy", "deformer_issues"),
		("Binding Coverage", "binding_coverage"),
		("Draw-Order Conflicts", "draw_order_conflicts"),
		("Mask Cycles", "mask_cycles"),
	];

	for (lbl, key) in AUDIT_CHECKS {
		let cnt = audit_cache["summary"][key].as_usize().unwrap_or(0);
		let dot_col = if cnt == 0 { AUDIT_OK_GREEN } else { AUDIT_ERR_ROSE };
		ui.horizontal(|ui| {
			let (dr, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), egui::Sense::hover());
			ui.painter().circle_filled(dr.center(), 3.2, dot_col);
			ui.label(egui::RichText::new(lbl).size(11.0).color(TEXT_PRIMARY));
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				let (txt, col) = if cnt == 0 {
					("OK".to_owned(), AUDIT_OK_GREEN)
				} else {
					(format!("{cnt} ISSUE"), AUDIT_ERR_ROSE)
				};
				ui.label(egui::RichText::new(txt).monospace().size(10.5).color(col));
			});
		});
		if cnt > 0 {
			if let JsonValue::Array(items) = &audit_cache["details"][key] {
				for item in items.iter().take(6) {
					let node_lbl = item["node"]
						.as_str()
						.or_else(|| item["node_a"].as_str())
						.unwrap_or("node");
					let issue = item["issue"].as_str().unwrap_or(key);
					ui.horizontal(|ui| {
						ui.add_space(14.0);
						ui.label(egui::RichText::new(format!("- {node_lbl}: {issue}")).monospace().size(9.5).color(TEXT_MUTED));
						if let Some(uid) = item["node_uuid"].as_u32() {
							if ui.small_button("Go").clicked() {
								actions.push(InspectorAction::SelectNode(InoxNodeUuid(uid)));
							}
						}
					});
				}
			}
		}
	}

	ui.add_space(4.0);
	ui.horizontal(|ui| {
		if ui.button("Run 10-Point Audit").clicked() {
			actions.push(InspectorAction::RefreshAudit);
		}
		let fix_btn = egui::Button::new(egui::RichText::new("1-Click Auto-Fix").color(Color32::WHITE).strong())
			.fill(BTN_ACTIVE);
		if ui.add(fix_btn).clicked() {
			actions.push(InspectorAction::ExecCmd {
				cmd: "rig.auto_fix",
				params: json::object! {},
				ok_label: "Ran 1-click Auto-Fix".to_owned(),
			});
		}
	});

	ui.add_space(8.0);
	let diags = &session.model.puppet.diagnostics;
	let diag_badge = if diags.is_empty() {
		("Lossless (0)".to_owned(), AUDIT_OK_GREEN)
	} else {
		(format!("{} warnings", diags.len()), AUDIT_WARN_AMBER)
	};
	section_header(ui, "Compatibility & Load Warnings", Some((&diag_badge.0, diag_badge.1)));
	ui.add_space(3.0);

	if diags.is_empty() {
		ui.label(
			egui::RichText::new("No lossy conversions or unknown structure warnings.")
				.size(10.5)
				.color(TEXT_MUTED),
		);
	} else {
		for msg in diags.iter().take(20) {
			ui.label(egui::RichText::new(format!("! {msg}")).size(10.5).color(AUDIT_WARN_AMBER));
		}
	}

	let raw_nodes: Vec<_> = session
		.model
		.puppet
		.nodes
		.pre_order_iter()
		.filter_map(|n| {
			session
				.model
				.puppet
				.node_comps
				.get::<RawNodeData>(n.uuid)
				.map(|raw| (n.uuid, n.name.trim_end_matches('\0').to_owned(), raw.type_name.clone(), raw.fields.len()))
		})
		.collect();
	if !raw_nodes.is_empty() {
		ui.add_space(4.0);
		ui.label(
			egui::RichText::new(format!("Preserved Opaque Nodes ({}):", raw_nodes.len()))
				.size(10.5)
				.color(ACCENT_BLUE),
		);
		for (uid, name, tname, fc) in raw_nodes.iter().take(12) {
			ui.horizontal(|ui| {
				if ui.small_button(format!("{name} [{tname}, {fc}f]")).clicked() {
					actions.push(InspectorAction::SelectNode(*uid));
				}
			});
		}
	}

	actions
}

#![forbid(unsafe_code)]

use std::collections::HashSet;

use egui::{self, pos2, vec2, Align2, Color32, Sense, Stroke};
use inox2d::node::components::{
	Composite, Drawable, GridDeformer, MeshGroup, PathDeformer, RawNodeData, SimplePhysics, TexturedMesh,
};
use inox2d::node::{InoxNode, InoxNodeUuid};
use inox2d::puppet::Puppet;

use crate::theme::{
	section_header, ACCENT_BLUE, AUDIT_ERR_ROSE, AUDIT_OK_GREEN, AUDIT_WARN_AMBER, BORDER, ROW_SELECTED, TEXT_MUTED,
	TEXT_PRIMARY, TIMELINE_EMERALD, TIMELINE_VIOLET,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutlinerTab {
	Tree,
	DrawOrder,
}

#[derive(Clone, Debug)]
pub struct TreeRow {
	pub uuid: InoxNodeUuid,
	pub name: String,
	pub depth: usize,
	pub has_children: bool,
	pub is_collapsed: bool,
	pub enabled: bool,
	pub zsort: f32,
	pub tag: &'static str,
	pub tag_color: Color32,
}

pub enum OutlinerAction {
	Select(InoxNodeUuid),
	ToggleCollapse(InoxNodeUuid),
	ToggleEnabled(InoxNodeUuid, bool),
	SetZSort(InoxNodeUuid, f32),
	AddPart,
	AddGridDeformer,
	AddPathDeformer,
	AddMeshGroup,
	DeleteNode(InoxNodeUuid),
}

fn node_row_meta(puppet: &Puppet, node: &InoxNode, depth: usize, has_children: bool, is_collapsed: bool) -> TreeRow {
	let comps = &puppet.node_comps;
	let is_part = comps.get::<TexturedMesh>(node.uuid).is_some();
	let is_mg = comps.get::<MeshGroup>(node.uuid).is_some();
	let is_gd = comps.get::<GridDeformer>(node.uuid).is_some();
	let is_pd = comps.get::<PathDeformer>(node.uuid).is_some();
	let is_comp = comps.get::<Composite>(node.uuid).is_some();
	let is_phys = comps.get::<SimplePhysics>(node.uuid).is_some();
	let is_raw = comps.get::<RawNodeData>(node.uuid).is_some();
	let has_mask = comps
		.get::<Drawable>(node.uuid)
		.and_then(|d| d.masks.as_ref())
		.is_some_and(|m| !m.masks.is_empty());

	let tag = if has_mask && is_part {
		"Part·M"
	} else if is_mg {
		"MG"
	} else if is_gd {
		"Grid"
	} else if is_pd {
		"Path"
	} else if is_phys {
		"Phys"
	} else if is_comp {
		"Comp"
	} else if is_raw {
		"Raw"
	} else if is_part {
		"Part"
	} else {
		"Node"
	};

	let tag_color = if is_phys {
		TIMELINE_EMERALD
	} else if is_mg || is_gd || is_pd {
		TIMELINE_VIOLET
	} else if has_mask {
		AUDIT_WARN_AMBER
	} else if is_raw {
		AUDIT_ERR_ROSE
	} else {
		ACCENT_BLUE
	};

	let zsort = puppet.node_zsort(node.uuid).unwrap_or(node.zsort);
	TreeRow {
		uuid: node.uuid,
		name: node.name.clone(),
		depth,
		has_children,
		is_collapsed,
		enabled: node.enabled,
		zsort,
		tag,
		tag_color,
	}
}

fn walk_tree(
	puppet: &Puppet,
	parent: InoxNodeUuid,
	depth: usize,
	collapsed: &HashSet<InoxNodeUuid>,
	out: &mut Vec<TreeRow>,
) {
	for child in puppet.nodes.get_children(parent) {
		let has_children = puppet.nodes.get_children(child.uuid).next().is_some();
		let is_collapsed = collapsed.contains(&child.uuid);
		out.push(node_row_meta(puppet, child, depth, has_children, is_collapsed));
		if has_children && !is_collapsed {
			walk_tree(puppet, child.uuid, depth + 1, collapsed, out);
		}
	}
}

pub fn collect_tree_rows(puppet: &Puppet, collapsed: &HashSet<InoxNodeUuid>, filter: &str) -> Vec<TreeRow> {
	let q = filter.trim().to_ascii_lowercase();
	let mut out = Vec::new();
	if q.is_empty() {
		walk_tree(puppet, puppet.nodes.root_node_id, 0, collapsed, &mut out);
	} else {
		for node in puppet.nodes.pre_order_iter() {
			if node.uuid == puppet.nodes.root_node_id {
				continue;
			}
			if node.name.to_ascii_lowercase().contains(&q) {
				let has_children = puppet.nodes.get_children(node.uuid).next().is_some();
				out.push(node_row_meta(puppet, node, 0, has_children, false));
			}
		}
	}
	out
}

pub fn collect_draw_order_rows(puppet: &Puppet, filter: &str) -> Vec<TreeRow> {
	let q = filter.trim().to_ascii_lowercase();
	let mut out: Vec<TreeRow> = puppet
		.nodes
		.pre_order_iter()
		.filter(|n| n.uuid != puppet.nodes.root_node_id && puppet.node_comps.get::<Drawable>(n.uuid).is_some())
		.filter(|n| q.is_empty() || n.name.to_ascii_lowercase().contains(&q))
		.map(|n| node_row_meta(puppet, n, 0, false, false))
		.collect();
	out.sort_by(|a, b| a.zsort.total_cmp(&b.zsort).then_with(|| a.uuid.0.cmp(&b.uuid.0)));
	out
}

pub fn draw_outliner_panel(
	ui: &mut egui::Ui,
	puppet: &Puppet,
	selected_node: Option<InoxNodeUuid>,
	collapsed: &HashSet<InoxNodeUuid>,
	tab: &mut OutlinerTab,
	filter: &mut String,
) -> Vec<OutlinerAction> {
	let mut actions = Vec::new();
	section_header(ui, "Hierarchy", None);
	ui.horizontal(|ui| {
		ui.selectable_value(tab, OutlinerTab::Tree, "Node Tree");
		ui.selectable_value(tab, OutlinerTab::DrawOrder, "Draw Order");
	});
	ui.horizontal(|ui| {
		ui.add_sized([ui.available_width() - 26.0, 20.0], egui::TextEdit::singleline(filter).hint_text("Filter nodes..."));
		if !filter.is_empty() && ui.small_button("x").clicked() {
			filter.clear();
		}
	});
	ui.separator();

	let rows = match *tab {
		OutlinerTab::Tree => collect_tree_rows(puppet, collapsed, filter),
		OutlinerTab::DrawOrder => collect_draw_order_rows(puppet, filter),
	};

	let footer_h = 30.0;
	let scroll_h = (ui.available_height() - footer_h).max(80.0);
	egui::ScrollArea::vertical()
		.id_source("outliner_scroll")
		.max_height(scroll_h)
		.show(ui, |ui| {
			for row in &rows {
				let is_sel = selected_node == Some(row.uuid);
				let row_w = ui.available_width().max(150.0);
				let row_rect = ui.allocate_space(vec2(row_w, 22.0)).1;
				let mut click_resp = ui.interact(row_rect, ui.id().with(("out_row", row.uuid.0)), Sense::click());
				if is_sel {
					ui.painter().rect_filled(row_rect, 2.0, ROW_SELECTED);
				} else if click_resp.hovered() {
					ui.painter().rect_filled(row_rect, 2.0, Color32::from_rgb(42, 42, 48));
				}

				if *tab == OutlinerTab::Tree {
					let indent = (row.depth as f32 * 11.0).min(77.0);
					let tri_c = pos2(row_rect.left() + 8.0 + indent, row_rect.center().y);
					if row.has_children {
						let tri_r = egui::Rect::from_center_size(tri_c, vec2(12.0, 16.0));
						let tri_resp = ui.interact(tri_r, ui.id().with(("out_tri", row.uuid.0)), Sense::click());
						ui.painter().text(
							tri_c,
							Align2::CENTER_CENTER,
							if row.is_collapsed { ">" } else { "v" },
							egui::FontId::monospace(10.0),
							TEXT_MUTED,
						);
						if tri_resp.clicked() {
							actions.push(OutlinerAction::ToggleCollapse(row.uuid));
							continue;
						}
					}
					let eye_c = pos2(tri_c.x + 12.0, row_rect.center().y);
					let eye_r = egui::Rect::from_center_size(eye_c, vec2(12.0, 16.0));
					let eye_resp = ui.interact(eye_r, ui.id().with(("out_eye", row.uuid.0)), Sense::click());
					if row.enabled {
						ui.painter().circle_filled(
							eye_c,
							3.0,
							if is_sel { Color32::WHITE } else { AUDIT_OK_GREEN },
						);
					} else {
						ui.painter()
							.circle_stroke(eye_c, 3.0, Stroke::new(1.0_f32, TEXT_MUTED));
					}
					if eye_resp.clicked() {
						actions.push(OutlinerAction::ToggleEnabled(row.uuid, !row.enabled));
						continue;
					}

					ui.painter().text(
						pos2(eye_c.x + 9.0, row_rect.center().y),
						Align2::LEFT_CENTER,
						&row.name,
						egui::FontId::proportional(11.0),
						if is_sel { Color32::WHITE } else { TEXT_PRIMARY },
					);
					ui.painter().text(
						pos2(row_rect.right() - 6.0, row_rect.center().y),
						Align2::RIGHT_CENTER,
						row.tag,
						egui::FontId::monospace(9.5),
						row.tag_color,
					);
				} else {
					let btn_w = 18.0;
					let up_r = egui::Rect::from_min_size(
						pos2(row_rect.right() - btn_w * 2.0 - 4.0, row_rect.top() + 2.0),
						vec2(btn_w, 18.0),
					);
					let dn_r = egui::Rect::from_min_size(
						pos2(row_rect.right() - btn_w - 2.0, row_rect.top() + 2.0),
						vec2(btn_w, 18.0),
					);
					let up_resp = ui
						.interact(up_r, ui.id().with(("z_up", row.uuid.0)), Sense::click())
						.on_hover_text("Bring forward (zsort - 1.0)");
					let dn_resp = ui
						.interact(dn_r, ui.id().with(("z_dn", row.uuid.0)), Sense::click())
						.on_hover_text("Send back (zsort + 1.0)");

					ui.painter().rect_stroke(up_r, 2.0, Stroke::new(1.0_f32, BORDER));
					ui.painter().rect_stroke(dn_r, 2.0, Stroke::new(1.0_f32, BORDER));
					ui.painter().text(up_r.center(), Align2::CENTER_CENTER, "^", egui::FontId::monospace(10.0), TEXT_PRIMARY);
					ui.painter().text(dn_r.center(), Align2::CENTER_CENTER, "v", egui::FontId::monospace(10.0), TEXT_PRIMARY);

					if up_resp.clicked() {
						actions.push(OutlinerAction::SetZSort(row.uuid, row.zsort - 1.0));
						continue;
					}
					if dn_resp.clicked() {
						actions.push(OutlinerAction::SetZSort(row.uuid, row.zsort + 1.0));
						continue;
					}

					ui.painter().text(
						pos2(row_rect.left() + 6.0, row_rect.center().y),
						Align2::LEFT_CENTER,
						&row.name,
						egui::FontId::proportional(11.0),
						if is_sel { Color32::WHITE } else { TEXT_PRIMARY },
					);
					ui.painter().text(
						pos2(up_r.left() - 5.0, row_rect.center().y),
						Align2::RIGHT_CENTER,
						format!("{:.1}", row.zsort),
						egui::FontId::monospace(9.5),
						row.tag_color,
					);
				}

				click_resp = click_resp.on_hover_text(format!("{} ({}) z={:.1}", row.name, row.tag, row.zsort));
				if click_resp.clicked() {
					actions.push(OutlinerAction::Select(row.uuid));
				}
			}
		});

	ui.separator();
	ui.horizontal(|ui| {
		if ui.small_button("+Part").on_hover_text("Add Part node").clicked() {
			actions.push(OutlinerAction::AddPart);
		}
		if ui.small_button("+Grid").on_hover_text("Wrap selected in GridDeformer").clicked() {
			actions.push(OutlinerAction::AddGridDeformer);
		}
		if ui.small_button("+Path").on_hover_text("Wrap selected in PathDeformer").clicked() {
			actions.push(OutlinerAction::AddPathDeformer);
		}
		if ui.small_button("+MG").on_hover_text("Wrap selected in MeshGroup").clicked() {
			actions.push(OutlinerAction::AddMeshGroup);
		}
		if let Some(id) = selected_node {
			if id != puppet.nodes.root_node_id && ui.small_button("Del").clicked() {
				actions.push(OutlinerAction::DeleteNode(id));
			}
		}
	});

	actions
}

#[cfg(test)]
mod tests {
	use super::*;
	use inox2d_mcp::engine::Session;

	#[test]
	fn outliner_tree_collapse_and_draw_order_sort() {
		let mut session = Session::new_sample();
		let puppet = &session.model.puppet;

		let empty_collapsed = HashSet::new();
		let all_rows = collect_tree_rows(puppet, &empty_collapsed, "");
		assert!(all_rows.iter().any(|r| r.depth >= 1), "sample puppet must have nested nodes");

		if let Some(head) = all_rows.iter().find(|r| r.has_children) {
			let mut collapsed = HashSet::new();
			collapsed.insert(head.uuid);
			let collapsed_rows = collect_tree_rows(puppet, &collapsed, "");
			assert!(collapsed_rows.len() < all_rows.len());
		}

		session.model.puppet.init_all();
		session.model.puppet.begin_frame();
		session.model.puppet.end_frame(0.0);
		let z_rows = collect_draw_order_rows(&session.model.puppet, "");
		assert!(z_rows.len() >= 3);
		for pair in z_rows.windows(2) {
			assert!(pair[0].zsort <= pair[1].zsort);
		}
	}
}

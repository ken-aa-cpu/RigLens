use std::collections::HashMap;

use eframe::egui::{self, pos2, vec2, Align2, Color32, Rect, Sense, Stroke};
use glam::Vec2 as GVec2;
use riglens_core::math::interp::InterpolateMode;
use riglens_core::node::components::{Drawable, GridDeformer, MeshGroup, PathDeformer, RawNodeData, SimplePhysics, TexturedMesh};

use crate::icons::draw_transport_play;
use crate::startup::{clean_display_name, format_clean_zsort};
use crate::theme::{
	ACCENT_BLUE, AUDIT_ERR_ROSE, AUDIT_OK_GREEN, AUDIT_WARN_AMBER, BG_DOCK, BG_INPUT, BG_WIDGET, BORDER, PLAYHEAD_RED,
	ROW_SELECTED, TEXT_MUTED, TEXT_PRIMARY, TIMELINE_EMERALD, TIMELINE_VIOLET,
};
use crate::Inox2dStudioApp;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BottomTab {
	Timeline,
	MotionCurves,
}

#[derive(Clone, Debug)]
pub(crate) struct CurveDragState {
	pub(crate) param_name: String,
	pub(crate) target: u8,
	pub(crate) kf_idx: usize,
}

fn relocate_keyframe_sorted(kfs: &mut [riglens_core::animation::Keyframe], mut idx: usize) -> usize {
	let f = kfs[idx].frame;
	while idx > 0 && kfs[idx - 1].frame > f {
		kfs.swap(idx - 1, idx);
		idx -= 1;
	}
	while idx + 1 < kfs.len() && kfs[idx + 1].frame < f {
		kfs.swap(idx, idx + 1);
		idx += 1;
	}
	idx
}

impl Inox2dStudioApp {
	pub(crate) fn apply_anim_at_playhead(&mut self) {
		let anim_opt = self
			.session
			.model
			.puppet
			.animations
			.get(self.active_anim.as_str())
			.or_else(|| self.session.model.puppet.animations.values().next())
			.cloned();
		if let Some(anim) = anim_opt {
			let vals = anim.eval_at_frame(self.playhead_frame, &self.session.model.puppet.params);
			for (k, v) in vals {
				self.session.param_overrides.insert(k, v);
			}
		}
	}

	pub(crate) fn insert_keyframe_at_playhead(&mut self) {
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
		let anim_name = self.active_anim.clone();
		self.exec_cmd(
			"anim.set_keyframe",
			json::object! {
				anim: anim_name.as_str(),
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

	pub(crate) fn remove_selected_keyframe(&mut self) {
		let Some((pname, target, frame)) = self.selected_keyframe.take() else {
			return;
		};
		let anim_name = self.active_anim.clone();
		self.exec_cmd(
			"anim.remove_keyframe",
			json::object! {
				anim: anim_name.as_str(),
				param: pname.as_str(),
				target: target,
				frame: frame
			},
			&format!("Removed keyframe @ frame {frame}"),
		);
	}

	pub(crate) fn draw_motion_curves(&mut self, ui: &mut egui::Ui, dock_rect: Rect, left_w: f32) {
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
		if let Some(anim) = self.session.model.puppet.animations.get(self.active_anim.as_str()) {
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
			.get(self.active_anim.as_str())
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
		if let (Some(puuid), Some(anim)) = (
			sel_uuid,
			self.session.model.puppet.animations.get(self.active_anim.as_str()),
		) {
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
				let r_tens = ctrl_ui.add(egui::Slider::new(&mut t_val, 0.0..=1.0).text("Tension"));
				if r_tens.drag_started() || r_tens.gained_focus() {
					self.begin_gesture_undo();
				}
				if r_tens.changed() {
					self.begin_gesture_undo();
					if let (Some(puuid), Some(anim)) = (
						sel_uuid,
						self.session.model.puppet.animations.get_mut(self.active_anim.as_str()),
					) {
						if let Some(lane) = anim.lanes.iter_mut().find(|l| l.param_uuid == puuid && l.target == sel_target) {
							if let Some(k) = lane.keyframes.iter_mut().find(|k| k.frame == kp_f) {
								k.tension = t_val;
								self.apply_anim_at_playhead();
								self.session.revision += 1;
								self.cached_rev = u64::MAX;
							}
						}
					}
					if !r_tens.dragged() && !r_tens.has_focus() {
						self.finish_gesture_undo();
					}
				}
				if r_tens.drag_stopped() || r_tens.lost_focus() {
					self.finish_gesture_undo();
				}
			}
		}
		if ctrl_ui
			.small_button(match cur_interp {
				InterpolateMode::Linear => "Mode: Smooth/Linear",
				InterpolateMode::Nearest => "Mode: Nearest",
				InterpolateMode::Stepped => "Mode: Stepped",
				InterpolateMode::Cubic => "Mode: Cubic",
				InterpolateMode::Bezier => "Mode: Bezier",
			})
			.clicked()
		{
			let next_mode = match cur_interp {
				InterpolateMode::Linear => InterpolateMode::Cubic,
				InterpolateMode::Cubic => InterpolateMode::Bezier,
				InterpolateMode::Bezier => InterpolateMode::Nearest,
				InterpolateMode::Nearest => InterpolateMode::Stepped,
				InterpolateMode::Stepped => InterpolateMode::Linear,
			};
			self.begin_gesture_undo();
			if let (Some(puuid), Some(anim)) = (
				sel_uuid,
				self.session.model.puppet.animations.get_mut(self.active_anim.as_str()),
			) {
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
			.get(self.active_anim.as_str())
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
				let anim_name = self.active_anim.clone();
				self.exec_cmd(
					"anim.set_keyframe",
					json::object! { anim: anim_name.as_str(), param: sel_pname.as_str(), target: sel_target, frame: nf, value: nv, tension: 0.5 },
					&format!("Added keyframe ({nf}, {nv:.2})"),
				);
				self.selected_keyframe = Some((sel_pname.clone(), sel_target, nf));
			}
		}
		if g_resp.drag_started() {
			if let Some(ptr) = g_resp.interact_pointer_pos() {
				if let Some((idx, kf)) = kfs
					.1
					.iter()
					.enumerate()
					.find(|(_, k)| pos2(f_to_x(k.frame as f32), val_to_y(k.value)).distance(ptr) < 11.0)
				{
					self.begin_gesture_undo();
					self.selected_keyframe = Some((sel_pname.clone(), sel_target, kf.frame));
					self.curve_drag = Some(CurveDragState {
						param_name: sel_pname.clone(),
						target: sel_target,
						kf_idx: idx,
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
					self.session.model.puppet.animations.get_mut(self.active_anim.as_str()),
				) {
					if let Some(lane) = anim
						.lanes
						.iter_mut()
						.find(|l| l.param_uuid == puuid && l.target == cd.target)
					{
						if let Some(k) = lane.keyframes.get_mut(cd.kf_idx) {
							if k.frame != nf || (k.value - nv).abs() > 1e-6 {
								k.frame = nf;
								k.value = nv;
								cd.kf_idx = relocate_keyframe_sorted(&mut lane.keyframes, cd.kf_idx);
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
		}
		if g_resp.drag_stopped() && self.curve_drag.take().is_some() {
			self.finish_gesture_undo();
		}
	}

	pub(crate) fn draw_bottom_dock(&mut self, ui: &mut egui::Ui) {
		ui.horizontal(|ui| {
			let lbl_timeline = self.i18n.t("timeline.tab_timeline").to_owned();
			let lbl_curves = self.i18n.t("timeline.tab_curves").to_owned();
			let lbl_loop = self.i18n.t("timeline.loop").to_owned();
			let lbl_add_key = self.i18n.t("timeline.add_key").to_owned();
			let t_btn = egui::SelectableLabel::new(self.bottom_tab == BottomTab::Timeline, lbl_timeline);
			if ui.add(t_btn).clicked() {
				self.bottom_tab = BottomTab::Timeline;
			}
			let m_btn = egui::SelectableLabel::new(self.bottom_tab == BottomTab::MotionCurves, lbl_curves);
			if ui.add(m_btn).clicked() {
				self.bottom_tab = BottomTab::MotionCurves;
			}

			ui.separator();
			let mut anim_keys: Vec<String> = self.session.model.puppet.animations.keys().cloned().collect();
			anim_keys.sort();
			if !anim_keys.is_empty() && !self.session.model.puppet.animations.contains_key(&self.active_anim) {
				self.active_anim = anim_keys[0].clone();
			}
			egui::ComboBox::from_id_source("timeline_anim_combo")
				.selected_text(self.active_anim.as_str())
				.width(74.0)
				.show_ui(ui, |ui| {
					for ak in &anim_keys {
						if ui.selectable_label(&self.active_anim == ak, ak).clicked() {
							self.active_anim = ak.clone();
							self.playhead_frame = 1.0;
							self.selected_keyframe = None;
							self.apply_anim_at_playhead();
						}
					}
				});
			ui.add_sized([54.0, 18.0], egui::TextEdit::singleline(&mut self.new_anim_name).hint_text("Clip"));
			if ui.small_button("+").on_hover_text("Create new animation clip").clicked() {
				let aname = if self.new_anim_name.trim().is_empty() {
					format!("Clip_{}", anim_keys.len() + 1)
				} else {
					self.new_anim_name.trim().to_owned()
				};
				self.exec_cmd(
					"anim.create",
					json::object! { name: aname.as_str(), length: 120, fps: 30 },
					&format!("Created animation '{aname}'"),
				);
				self.active_anim = aname;
				self.new_anim_name.clear();
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
			let (play_rect, play_resp) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click());
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
			ui.checkbox(&mut self.loop_anim, lbl_loop);

			ui.label(
				egui::RichText::new(format!(
					"F{} {:.2}s",
					self.playhead_frame.round() as i32,
					(self.playhead_frame - 1.0) / 30.0
				))
				.monospace()
				.size(10.5)
				.color(TEXT_MUTED),
			);
			if ui
				.small_button(lbl_add_key)
				.on_hover_text("Insert keyframe at playhead (F6)")
				.clicked()
			{
				self.insert_keyframe_at_playhead();
			}
			if self.selected_keyframe.is_some() && ui.small_button("Del").on_hover_text("Remove selected keyframe").clicked() {
				self.remove_selected_keyframe();
			}
		});

		ui.separator();

		let dock_rect = ui.available_rect_before_wrap();
		let left_w = 220.0;
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
			.pre_order_iter()
			.filter(|n| n.uuid != self.session.model.puppet.nodes.root_node_id)
			.map(|n| {
				let comps = &self.session.model.puppet.node_comps;
				let is_part = comps.get::<TexturedMesh>(n.uuid).is_some();
				let is_mg = comps.get::<MeshGroup>(n.uuid).is_some();
				let is_grid = comps.get::<GridDeformer>(n.uuid).is_some();
				let is_path = comps.get::<PathDeformer>(n.uuid).is_some();
				let is_raw = comps.get::<RawNodeData>(n.uuid).is_some();
				let has_mask = comps
					.get::<Drawable>(n.uuid)
					.and_then(|d| d.masks.as_ref())
					.map(|m| !m.masks.is_empty())
					.unwrap_or(false);
				let has_phys = comps.get::<SimplePhysics>(n.uuid).is_some();
				(
					n.uuid,
					clean_display_name(&n.name).to_owned(),
					n.enabled,
					n.zsort,
					is_part,
					is_mg,
					is_grid,
					is_path,
					is_raw,
					has_mask,
					has_phys,
				)
			})
			.collect();

		let anim_len = self
			.session
			.model
			.puppet
			.animations
			.get(self.active_anim.as_str())
			.map(|a| a.length.max(30))
			.unwrap_or(120);

		let row_h = 22.0;

		let mut left_ui = ui.child_ui(left_rect, egui::Layout::top_down(egui::Align::Min), None);
		egui::ScrollArea::vertical().id_source("layers_scroll").show(&mut left_ui, |ui| {
			for (idx, (uid, name, enabled, zsort, is_part, is_mg, is_grid, is_path, is_raw, has_mask, has_phys)) in
				nodes_list.iter().enumerate()
			{
				let is_sel = self.selected_node == Some(*uid);
				let tag_col = if *has_phys {
					TIMELINE_EMERALD
				} else if *is_mg || *is_grid || *is_path {
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
				} else if *is_grid {
					"[GD] "
				} else if *is_path {
					"[PD] "
				} else if *has_phys {
					"[PH] "
				} else if *is_part {
					""
				} else if *is_raw {
					"[RAW] "
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
				let label_clip = Rect::from_min_max(
					pos2(row_rect.left() + 19.0, row_rect.top()),
					pos2((row_rect.right() - 52.0).max(row_rect.left() + 24.0), row_rect.bottom()),
				);
				ui.painter().with_clip_rect(label_clip).text(
					pos2(row_rect.left() + 21.0, row_rect.center().y),
					Align2::LEFT_CENTER,
					format!("{icon_lbl}{name}"),
					egui::FontId::proportional(11.0),
					if is_sel { Color32::WHITE } else { TEXT_PRIMARY },
				);
				let disp_z = format_clean_zsort(*zsort);
				ui.painter().text(
					pos2(row_rect.right() - 22.0, row_rect.center().y),
					Align2::RIGHT_CENTER,
					format!("z{:.0}", disp_z),
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
					let f = (((p.x - right_rect.left() - 12.0) / px_per_frame) + 1.0).clamp(1.0, anim_len as f32);
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
			let anim_opt = self.session.model.puppet.animations.get(self.active_anim.as_str()).cloned();
			let span_w = (anim_len as f32 * px_per_frame).min(right_rect.width() - 24.0).max(40.0);
			let mut kf_hits: Vec<(egui::Pos2, String, u8, i32, usize)> = Vec::new();

			for (idx, (uid, name, _, _, _, _, _, _, _, _, _)) in nodes_list.iter().enumerate() {
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
						for (k_idx, kf) in lane.keyframes.iter().enumerate() {
							let kx = right_rect.left() + (kf.frame.max(1) - 1) as f32 * px_per_frame + 12.0;
							if kx > bar_rect.right() - 2.0 {
								continue;
							}
							let kc = pos2(kx, bar_rect.center().y);
							kf_hits.push((kc, pname.clone(), lane.target, kf.frame, k_idx));
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
					if let Some((_, pname, tgt, f, _)) = kf_hits.iter().find(|(kc, _, _, _, _)| kc.distance(ptr) < 10.0) {
						self.selected_curve_lane = Some((pname.clone(), *tgt));
						self.selected_keyframe = Some((pname.clone(), *tgt, *f));
						self.playhead_frame = (*f).max(1) as f32;
						self.apply_anim_at_playhead();
					}
				}
			}
			if tr_resp.drag_started() {
				if let Some(ptr) = tr_resp.interact_pointer_pos() {
					if let Some((_, pname, tgt, f, k_idx)) = kf_hits.iter().find(|(kc, _, _, _, _)| kc.distance(ptr) < 11.0) {
						self.begin_gesture_undo();
						self.selected_curve_lane = Some((pname.clone(), *tgt));
						self.selected_keyframe = Some((pname.clone(), *tgt, *f));
						self.curve_drag = Some(CurveDragState {
							param_name: pname.clone(),
							target: *tgt,
							kf_idx: *k_idx,
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
						self.session.model.puppet.animations.get_mut(self.active_anim.as_str()),
					) {
						if let Some(lane) = anim
							.lanes
							.iter_mut()
							.find(|l| l.param_uuid == puuid && l.target == cd.target)
						{
							if let Some(k) = lane.keyframes.get_mut(cd.kf_idx) {
								if k.frame != nf {
									k.frame = nf;
									cd.kf_idx = relocate_keyframe_sorted(&mut lane.keyframes, cd.kf_idx);
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
}

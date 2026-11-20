use eframe::egui::{self, pos2, vec2, Align2, Color32, Rect, Sense, Stroke};
use glam::{vec2 as gvec2, vec3 as gvec3, Vec2 as GVec2};
use inox2d::node::components::{Mesh, PathDeformer, SimplePhysics, TexturedMesh, TransformStore};
use inox2d::node::InoxNodeUuid;
use inox2d_mcp::rig::{apply_vertex_deform_delta, configure_pendulum_physics, retriangulate_points_preserving_deforms};

use crate::icons::StudioTool;
use crate::stage_render::StageOverlay;
use crate::theme::{
	ACCENT_BLUE, AUDIT_WARN_AMBER, BG_DOCK, BG_INPUT, BG_STAGE, BORDER, TEXT_MUTED, TIMELINE_EMERALD,
	TIMELINE_VIOLET,
};
use crate::Inox2dStudioApp;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum VertexEditMode {
	DeformKeypoint,
	BaseTopology,
}

#[derive(Clone, Debug)]
pub(crate) enum StageDrag {
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

/// Maps stage pointer coordinates to normalized 2.5D pose space (`[-1, 1]`),
/// with +Y upward (head/gaze up) to match Inochi2D/nijigenerate parameter conventions.
pub(crate) fn pose25d_norm_from_screen(pointer: egui::Pos2, canvas_rect: egui::Rect) -> (f32, f32) {
	let nx = ((pointer.x - canvas_rect.center().x) / (canvas_rect.width() * 0.45)).clamp(-1.0, 1.0);
	let ny = -((pointer.y - canvas_rect.center().y) / (canvas_rect.height() * 0.45)).clamp(-1.0, 1.0);
	(nx, ny)
}

/// Maps a pointer position inside an XY pad rect to normalized `[0, 1]` parameter coordinates,
/// with `ty = 1.0` at `rect.top()` (`max.y`, up) and `ty = 0.0` at `rect.bottom()` (`min.y`, down).
pub(crate) fn xy_pad_norm_from_pointer(pointer: egui::Pos2, rect: egui::Rect) -> (f32, f32) {
	let tx = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
	let ty = (1.0_f32 - (pointer.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
	(tx, ty)
}

/// Maps normalized `[0, 1]` parameter coordinates to screen position inside an XY pad rect,
/// with `ny = 1.0` at `rect.top()` and `ny = 0.0` at `rect.bottom()`.
pub(crate) fn xy_pad_screen_from_norm(nx: f32, ny: f32, rect: egui::Rect) -> egui::Pos2 {
	egui::pos2(rect.left() + nx * rect.width(), rect.bottom() - ny * rect.height())
}

impl Inox2dStudioApp {
	pub(crate) fn draw_stage(&mut self, ui: &mut egui::Ui) {
		let rect = ui.available_rect_before_wrap();
		let painter = ui.painter_at(rect);
		painter.rect_filled(rect, 0.0, BG_STAGE);

		let resp = ui.allocate_rect(rect, Sense::click_and_drag());

		// Handle scroll-wheel zoom anchored around cursor
		if resp.hovered() {
			let scroll = ui.ctx().input(|i| i.raw_scroll_delta.y);
			if scroll.abs() > 0.1 {
				let factor = (1.0 + scroll * 0.0015).clamp(0.85, 1.18);
				let new_zoom = (self.view_zoom * factor).clamp(0.25, 12.0);
				if let Some(hp) = resp.hover_pos() {
					let rel = hp - rect.center();
					let ratio = new_zoom / self.view_zoom.max(0.05);
					self.view_pan = rel - (rel - self.view_pan) * ratio;
				}
				self.view_zoom = new_zoom;
			}
		}

		// Sync viewport size so Camera::matrix matches the full stage rect at 1:1 logical pixels
		let new_vp = gvec2(rect.width().max(64.0), rect.height().max(64.0));
		let vp_changed = (self.stage_viewport_size - new_vp).length_squared() > 1.0;
		self.stage_viewport_size = new_vp;
		if self.needs_initial_fit && rect.width() > 80.0 && rect.height() > 80.0 {
			self.fit_puppet_scale();
			if let Some(z) = self.initial_zoom.take() {
				self.view_zoom = z.clamp(0.25, 12.0);
			}
			if self.initial_focus_y.abs() > 1e-3 {
				self.puppet_center.y += self.initial_focus_y;
				self.initial_focus_y = 0.0;
			}
			self.needs_initial_fit = false;
			self.cached_rev = u64::MAX;
		} else if vp_changed && !self.live_physics {
			self.cached_rev = u64::MAX;
		}

		let canvas_rect = rect;
		let px_per_world = (self.puppet_scale * self.view_zoom).max(0.05);
		let world_per_ui = 1.0 / px_per_world;

		// Full-viewport stage checkerboard
		let clip_painter = ui.painter_at(canvas_rect);
		clip_painter.rect_filled(canvas_rect, 0.0, Color32::from_rgb(24, 25, 30));
		let tile = 20.0;
		let cols = (canvas_rect.width() / tile).ceil() as i32;
		let rows = (canvas_rect.height() / tile).ceil() as i32;
		let c1 = Color32::from_rgb(30, 32, 39);
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

		if self.xray_mode || self.use_exact_stage() {
			if let Some(tex) = &self.stage_tex {
				clip_painter.image(
					tex.id(),
					canvas_rect,
					Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
					Color32::WHITE,
				);
			} else if !self.xray_mode {
				self.draw_puppet_gpu_meshes(&clip_painter, canvas_rect);
			}
		} else {
			self.draw_puppet_gpu_meshes(&clip_painter, canvas_rect);
		}

		let hover_pos = resp.hover_pos();
		let shift_held = ui.ctx().input(|i| i.modifiers.shift);
		let middle_down = ui.ctx().input(|i| i.pointer.middle_down());

		let sel_overlay = self.selected_node_stage_overlay(canvas_rect);

		// Selected node vector bbox, wireframe, deformer lattice/spline, vertices & handles
		if self.show_sel_overlay {
			if let Some(ref ov) = sel_overlay {
				let bbox = ov.bbox();
				let pivot = ov.pivot();
				let expanded = bbox.expand(2.0);
				match ov {
					StageOverlay::Mesh { pts, indices, .. } => {
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
					}
					StageOverlay::Grid { pts, cols, rows, .. } => {
						let grid_stroke = Stroke::new(1.5_f32, TIMELINE_VIOLET);
						for r in 0..*rows {
							for c in 0..*cols {
								let i = r * *cols + c;
								if c + 1 < *cols {
									clip_painter.line_segment([pts[i], pts[i + 1]], grid_stroke);
								}
								if r + 1 < *rows {
									clip_painter.line_segment([pts[i], pts[i + *cols]], grid_stroke);
								}
							}
						}
					}
					StageOverlay::Path { pts, curve_pts, .. } => {
						let ctrl_stroke = Stroke::new(1.1_f32, Color32::from_rgba_unmultiplied(52, 211, 153, 120));
						for win in pts.windows(2) {
							clip_painter.line_segment([win[0], win[1]], ctrl_stroke);
						}
						if curve_pts.len() >= 2 {
							clip_painter.add(egui::Shape::line(
								curve_pts.clone(),
								Stroke::new(2.0_f32, TIMELINE_EMERALD),
							));
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

				// Vertex Subselect handles (A tool) — supports Mesh, MeshGroup, GridDeformer & PathDeformer
				if self.tool == StudioTool::VertexSubselect {
					let pts = ov.pts();
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
							clip_painter.circle_filled(sp, 4.4, Color32::from_rgb(56, 189, 248));
							clip_painter.circle_stroke(sp, 4.4, Stroke::new(1.3_f32, Color32::WHITE));
						} else if is_hov {
							clip_painter.circle_filled(sp, 3.8, AUDIT_WARN_AMBER);
							clip_painter.circle_stroke(sp, 3.8, Stroke::new(1.0_f32, Color32::WHITE));
						} else {
							clip_painter.circle_filled(sp, 3.0, Color32::from_rgb(30, 41, 59));
							clip_painter.circle_stroke(sp, 3.0, Stroke::new(1.0_f32, Color32::from_rgb(148, 163, 184)));
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
							if let Some(ov) = self.selected_node_stage_overlay(canvas_rect) {
								let z = self.session.model.puppet.node_zsort(node.uuid).unwrap_or(node.zsort);
								if ov.bbox().expand(4.0).contains(pos) && z < best_z {
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
						if let Some(ref ov) = sel_overlay {
							let pts = ov.pts();
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
							if let (Some(id), Some(ref ov)) = (self.selected_node, sel_overlay.as_ref()) {
								let exp = ov.bbox().expand(2.0);
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
							if let Some(ref ov) = sel_overlay {
								let pts = ov.pts();
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
					if let (Some(id), Some(p), Some(ref ov)) = (self.selected_node, pos, sel_overlay.as_ref()) {
						let ratio = (p.distance(ov.bbox().center()) / orig_dist).clamp(0.15, 5.0);
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
									} else if let Some(path) = self.session.model.puppet.node_comps.get_mut::<PathDeformer>(id) {
										for &vi in &indices {
											if let Some(v) = path.vertices.get_mut(vi as usize) {
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
						let (nx, ny) = pose25d_norm_from_screen(p, canvas_rect);
						for (pname, val) in [
							("Head:: Yaw-Pitch", gvec2(nx, ny)),
							("Head:: Roll", gvec2(nx * 0.35, 0.0)),
							("Body:: Yaw-Pitch", gvec2(nx * 0.35, ny * 0.35)),
							("Eye:: Gaze", gvec2(nx, ny)),
							("Eye:: Left:: Gaze", gvec2(nx, ny)),
							("Eye:: Right:: Gaze", gvec2(nx, ny)),
							("Eye:: Left:: Move", gvec2(nx, ny)),
							("Eye:: Right:: Move", gvec2(nx, ny)),
						] {
							if self.session.model.puppet.params.contains_key(pname) {
								self.session.param_overrides.insert(pname.to_owned(), val);
							}
						}
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
					if let Some(ref ov) = sel_overlay {
						if !shift_held {
							self.selected_vertices.clear();
						}
						for (vi, &sp) in ov.pts().iter().enumerate() {
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
							let has_tm = self.session.model.puppet.node_comps.get::<TexturedMesh>(id).is_some();
							if has_tm {
								if let Some(mesh) = self.session.model.puppet.node_comps.get::<Mesh>(id).cloned() {
									let _ = retriangulate_points_preserving_deforms(
										&mut self.session.model.puppet,
										id,
										mesh.vertices,
									);
								}
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

	pub(crate) fn draw_xy_pad(&mut self, ui: &mut egui::Ui, param_name: &str, size: f32, puck_color: Color32) {
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
						let dp = xy_pad_screen_from_norm(ax, ay, rect);
						painter.circle_filled(dp, 2.0, Color32::from_rgb(71, 85, 105));
					}
				}

				if (resp.clicked() || resp.dragged()) && rect.width() > 2.0 {
					if let Some(ptr) = resp.interact_pointer_pos() {
						let (mut tx, mut ty) = xy_pad_norm_from_pointer(ptr, rect);
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
				let puck = xy_pad_screen_from_norm(nx, ny, rect);
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
}

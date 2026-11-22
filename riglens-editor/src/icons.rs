#![forbid(unsafe_code)]

use egui::{pos2, vec2, Color32, Painter, Rect, Rounding, Sense, Stroke, Ui};

use crate::theme::{ACCENT_BLUE, BTN_ACTIVE, TEXT_MUTED};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StudioTool {
	Select,
	VertexSubselect,
	DeformBrush,
	Pose25D,
	Delaunay,
	MeshGroup,
	Physics,
	HandPan,
	ZoomFit,
}

impl StudioTool {
	/// Ordered in 3 ergonomic frequency groups matching AniCraft / Adobe Animate:
	/// [Direct Stage Manipulation] | [Auto Structure & Physics] | [Stage Navigation]
	pub fn groups() -> &'static [&'static [StudioTool]] {
		&[
			&[
				StudioTool::Select,
				StudioTool::VertexSubselect,
				StudioTool::DeformBrush,
				StudioTool::Pose25D,
			],
			&[
				StudioTool::Delaunay,
				StudioTool::MeshGroup,
				StudioTool::Physics,
			],
			&[StudioTool::HandPan, StudioTool::ZoomFit],
		]
	}

	/// Streamlined high-frequency tools for `WorkspaceMode::Quick`.
	pub fn quick_groups() -> &'static [&'static [StudioTool]] {
		&[
			&[
				StudioTool::Pose25D,
				StudioTool::Select,
				StudioTool::DeformBrush,
			],
			&[StudioTool::HandPan, StudioTool::ZoomFit],
		]
	}

	pub fn tooltip_key(self) -> &'static str {
		match self {
			StudioTool::Select => "tool.select",
			StudioTool::VertexSubselect => "tool.vertex",
			StudioTool::DeformBrush => "tool.brush",
			StudioTool::Pose25D => "tool.pose25d",
			StudioTool::Delaunay => "tool.delaunay",
			StudioTool::MeshGroup => "tool.meshgroup",
			StudioTool::Physics => "tool.physics",
			StudioTool::HandPan => "tool.pan",
			StudioTool::ZoomFit => "tool.zoom_fit",
		}
	}
}

pub fn draw_tool_button(ui: &mut Ui, tool: StudioTool, active: bool, tooltip: &str) -> bool {
	let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 28.0), Sense::click());
	let painter = ui.painter();

	if active {
		let bar = Rect::from_min_max(
			pos2(rect.left() - 4.0, rect.top() + 2.0),
			pos2(rect.left() - 1.5, rect.bottom() - 2.0),
		);
		painter.rect_filled(bar, 0.0, ACCENT_BLUE);
		painter.rect_filled(rect, Rounding::same(4.0), BTN_ACTIVE);
	} else if resp.hovered() {
		painter.rect_filled(rect, Rounding::same(4.0), Color32::from_rgb(50, 50, 58));
	}

	let fg = if active { Color32::WHITE } else { TEXT_MUTED };
	let c = rect.center();
	let st = Stroke::new(1.4_f32, fg);

	match tool {
		StudioTool::Select => {
			let pts = vec![
				c + vec2(-4.0, -7.0),
				c + vec2(-4.0, 6.0),
				c + vec2(-0.5, 2.5),
				c + vec2(3.0, 7.0),
				c + vec2(5.0, 5.5),
				c + vec2(1.5, 1.0),
				c + vec2(6.0, 1.0),
			];
			painter.add(egui::Shape::convex_polygon(pts, fg, Stroke::NONE));
		}
		StudioTool::VertexSubselect => {
			// Hollow subselection arrow + vertex node square (AniCraft `A` tool)
			let pts = vec![
				c + vec2(-5.0, -7.0),
				c + vec2(-5.0, 5.5),
				c + vec2(-1.5, 2.5),
				c + vec2(3.5, 2.0),
			];
			painter.add(egui::Shape::closed_line(pts, st));
			let node_box = Rect::from_center_size(c + vec2(4.0, 5.0), vec2(5.0, 5.0));
			painter.rect_filled(node_box, 0.5, if active { ACCENT_BLUE } else { Color32::from_rgb(56, 189, 248) });
			painter.rect_stroke(node_box, 0.5, Stroke::new(1.0_f32, Color32::WHITE));
		}
		StudioTool::DeformBrush => {
			painter.circle_stroke(c, 6.5, st);
			painter.circle_filled(c, 2.2, fg);
		}
		StudioTool::Pose25D => {
			painter.circle_stroke(c, 6.5, st);
			painter.line_segment([c - vec2(7.0, 0.0), c + vec2(7.0, 0.0)], Stroke::new(1.1_f32, fg));
			painter.line_segment([c - vec2(0.0, 7.0), c + vec2(0.0, 7.0)], Stroke::new(1.1_f32, fg));
			painter.circle_filled(c + vec2(2.0, -1.5), 2.0, ACCENT_BLUE);
		}
		StudioTool::Delaunay => {
			let a = c + vec2(0.0, -6.5);
			let b = c + vec2(6.5, 5.5);
			let d = c + vec2(-6.5, 5.5);
			painter.line_segment([a, b], st);
			painter.line_segment([b, d], st);
			painter.line_segment([d, a], st);
			painter.line_segment([a, c + vec2(0.0, 5.5)], Stroke::new(1.0_f32, fg));
		}
		StudioTool::MeshGroup => {
			let b = Rect::from_center_size(c, vec2(12.0, 12.0));
			painter.rect_stroke(b, 1.0, st);
			painter.line_segment([pos2(b.center().x, b.top()), pos2(b.center().x, b.bottom())], Stroke::new(1.0_f32, fg));
			painter.line_segment([pos2(b.left(), b.center().y), pos2(b.right(), b.center().y)], Stroke::new(1.0_f32, fg));
		}
		StudioTool::Physics => {
			let top = c + vec2(-2.0, -6.5);
			let bob = c + vec2(3.5, 4.5);
			painter.circle_stroke(top, 2.0, Stroke::new(1.1_f32, fg));
			painter.line_segment([top + vec2(0.8, 1.8), bob], st);
			painter.circle_filled(bob, 3.2, fg);
		}
		StudioTool::HandPan => {
			painter.line_segment([c - vec2(6.0, 0.0), c + vec2(6.0, 0.0)], st);
			painter.line_segment([c - vec2(0.0, 6.0), c + vec2(0.0, 6.0)], st);
			painter.circle_stroke(c, 2.5, st);
		}
		StudioTool::ZoomFit => {
			let z = Rect::from_center_size(c, vec2(12.0, 10.0));
			painter.rect_stroke(z, 1.0, st);
			painter.circle_filled(c, 1.8, ACCENT_BLUE);
		}
	}

	resp.on_hover_text(tooltip).clicked()
}

pub fn draw_transport_play(painter: &Painter, rect: Rect, playing: bool, fg: Color32) {
	let c = rect.center();
	if playing {
		painter.rect_filled(Rect::from_min_max(c + vec2(-4.0, -5.0), c + vec2(-1.2, 5.0)), 0.5, fg);
		painter.rect_filled(Rect::from_min_max(c + vec2(1.2, -5.0), c + vec2(4.0, 5.0)), 0.5, fg);
	} else {
		let pts = vec![c + vec2(-4.0, -5.5), c + vec2(5.5, 0.0), c + vec2(-4.0, 5.5)];
		painter.add(egui::Shape::convex_polygon(pts, fg, Stroke::NONE));
	}
}

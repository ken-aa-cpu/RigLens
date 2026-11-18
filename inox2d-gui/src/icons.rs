#![forbid(unsafe_code)]

use egui::{pos2, vec2, Color32, Painter, Rect, Rounding, Sense, Stroke, Ui};

use crate::theme::{ACCENT_BLUE, BTN_ACTIVE, TEXT_MUTED};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StudioTool {
	Select,
	Pose25D,
	DeformBrush,
	Delaunay,
	MeshGroup,
	Physics,
	HandPan,
	ZoomFit,
}

impl StudioTool {
	pub fn all() -> &'static [StudioTool] {
		&[
			StudioTool::Select,
			StudioTool::Pose25D,
			StudioTool::DeformBrush,
			StudioTool::Delaunay,
			StudioTool::MeshGroup,
			StudioTool::Physics,
			StudioTool::HandPan,
			StudioTool::ZoomFit,
		]
	}

	pub fn tooltip(self) -> &'static str {
		match self {
			StudioTool::Select => "Select / Move Node (V)",
			StudioTool::Pose25D => "Pose 2.5D Head & Gaze on Stage (P)",
			StudioTool::DeformBrush => "Gaussian Soft Deform Brush (D)",
			StudioTool::Delaunay => "Delaunay Contour Mesh (M)",
			StudioTool::MeshGroup => "Wrap in MeshGroup Cage (G)",
			StudioTool::Physics => "Attach SimplePhysics Pendulum (S)",
			StudioTool::HandPan => "Pan Stage View (H)",
			StudioTool::ZoomFit => "Reset & Fit Stage Zoom (Z)",
		}
	}
}

pub fn draw_tool_button(ui: &mut Ui, tool: StudioTool, active: bool) -> bool {
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
		StudioTool::Pose25D => {
			painter.circle_stroke(c, 6.5, st);
			painter.line_segment([c - vec2(7.0, 0.0), c + vec2(7.0, 0.0)], Stroke::new(1.1_f32, fg));
			painter.line_segment([c - vec2(0.0, 7.0), c + vec2(0.0, 7.0)], Stroke::new(1.1_f32, fg));
			painter.circle_filled(c + vec2(2.0, -1.5), 2.0, ACCENT_BLUE);
		}
		StudioTool::DeformBrush => {
			painter.circle_stroke(c, 6.5, st);
			painter.circle_filled(c, 2.2, fg);
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
			let top = c + vec2(0.0, -6.5);
			let bob = c + vec2(2.0, 4.5);
			painter.line_segment([top, bob], st);
			painter.circle_filled(bob, 3.0, fg);
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

	resp.on_hover_text(tool.tooltip()).clicked()
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

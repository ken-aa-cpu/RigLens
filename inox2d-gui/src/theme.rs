#![forbid(unsafe_code)]

use egui::{Color32, Margin, Rect, Rounding, Stroke, Vec2, Visuals};

// Exact Craft-series (AniCraft / FigCraft / PhotoCraft) color palette
pub const BG_MENU: Color32 = Color32::from_rgb(20, 20, 23);
pub const BG_DOCK: Color32 = Color32::from_rgb(30, 30, 34);
pub const BG_STAGE: Color32 = Color32::from_rgb(43, 45, 49);
pub const BG_WIDGET: Color32 = Color32::from_rgb(39, 39, 44);
pub const BG_WIDGET_HOVER: Color32 = Color32::from_rgb(50, 50, 56);
pub const BG_INPUT: Color32 = Color32::from_rgb(18, 18, 22);
pub const BORDER: Color32 = Color32::from_rgb(46, 46, 54);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(228, 228, 231);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(148, 152, 162);

pub const ACCENT_BLUE: Color32 = Color32::from_rgb(59, 130, 246);
pub const BTN_ACTIVE: Color32 = Color32::from_rgb(29, 78, 216);
pub const ROW_SELECTED: Color32 = Color32::from_rgb(30, 58, 138);

pub const TIMELINE_VIOLET: Color32 = Color32::from_rgb(109, 40, 217);
pub const TIMELINE_EMERALD: Color32 = Color32::from_rgb(5, 150, 105);
pub const PLAYHEAD_RED: Color32 = Color32::from_rgb(239, 68, 68);
pub const AUDIT_OK_GREEN: Color32 = Color32::from_rgb(52, 211, 153);
pub const AUDIT_WARN_AMBER: Color32 = Color32::from_rgb(245, 158, 11);
pub const AUDIT_ERR_ROSE: Color32 = Color32::from_rgb(244, 63, 94);

pub fn configure_craft_theme(ctx: &egui::Context) {
	let mut style = (*ctx.style()).clone();
	let mut visuals = Visuals::dark();

	visuals.window_fill = BG_DOCK;
	visuals.panel_fill = BG_DOCK;
	visuals.faint_bg_color = Color32::from_rgb(25, 25, 29);
	visuals.extreme_bg_color = BG_INPUT;
	visuals.code_bg_color = BG_INPUT;

	visuals.widgets.noninteractive.bg_fill = BG_DOCK;
	visuals.widgets.noninteractive.weak_bg_fill = BG_DOCK;
	visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
	visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);

	visuals.widgets.inactive.bg_fill = BG_WIDGET;
	visuals.widgets.inactive.weak_bg_fill = BG_WIDGET;
	visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(54, 54, 62));
	visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
	visuals.widgets.inactive.rounding = Rounding::same(4.0);

	visuals.widgets.hovered.bg_fill = BG_WIDGET_HOVER;
	visuals.widgets.hovered.weak_bg_fill = BG_WIDGET_HOVER;
	visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(76, 86, 106));
	visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
	visuals.widgets.hovered.rounding = Rounding::same(4.0);

	visuals.widgets.active.bg_fill = BTN_ACTIVE;
	visuals.widgets.active.weak_bg_fill = BTN_ACTIVE;
	visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT_BLUE);
	visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
	visuals.widgets.active.rounding = Rounding::same(4.0);

	visuals.selection.bg_fill = BTN_ACTIVE;
	visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT_BLUE);
	visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
	visuals.window_rounding = Rounding::same(6.0);

	style.visuals = visuals;
	style.spacing.item_spacing = Vec2::new(6.0, 5.0);
	style.spacing.button_padding = Vec2::new(8.0, 4.0);
	style.spacing.window_margin = Margin::same(8.0);
	style.spacing.slider_width = 104.0;
	ctx.set_style(style);
}

/// Draw an AniCraft-style dock section header with a 2px cobalt accent bar on the left.
pub fn section_header(ui: &mut egui::Ui, title: &str, right_badge: Option<(&str, Color32)>) {
	let desired_height = 24.0;
	let avail_w = ui.available_width().max(60.0);
	let (rect, _) = ui.allocate_exact_size(Vec2::new(avail_w, desired_height), egui::Sense::hover());
	let painter = ui.painter();

	painter.rect_filled(rect, 0.0, BG_DOCK);
	// Top subtle divider
	painter.line_segment(
		[rect.left_top(), rect.right_top()],
		Stroke::new(1.0_f32, BORDER),
	);
	// 2px left blue accent stripe matching AniCraft inspector sections
	let stripe = Rect::from_min_max(
		rect.left_top() + Vec2::new(0.0, 2.0),
		egui::pos2(rect.left() + 2.5, rect.bottom() - 2.0),
	);
	painter.rect_filled(stripe, 0.0, ACCENT_BLUE);

	painter.text(
		egui::pos2(rect.left() + 10.0, rect.center().y),
		egui::Align2::LEFT_CENTER,
		title,
		egui::FontId::proportional(12.0),
		TEXT_PRIMARY,
	);

	if let Some((badge_txt, col)) = right_badge {
		painter.text(
			egui::pos2(rect.right() - 8.0, rect.center().y),
			egui::Align2::RIGHT_CENTER,
			badge_txt,
			egui::FontId::monospace(10.5),
			col,
		);
	}
}

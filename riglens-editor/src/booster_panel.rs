use egui::{Color32, RichText};
use riglens_booster::pipeline::{BoosterController, BoosterControllerConfig};
use riglens_booster::transport::spout2::list_spout_senders;

use crate::theme;

#[derive(Clone, Debug)]
pub struct BoosterUiState {
	pub draft: BoosterControllerConfig,
	pub initialized: bool,
	pub spout_in_buf: String,
	pub senders_cache: Vec<String>,
}

impl Default for BoosterUiState {
	fn default() -> Self {
		let draft = BoosterControllerConfig::default();
		let spout_in_buf = draft.spout_in.clone().unwrap_or_default();
		Self {
			draft,
			initialized: false,
			spout_in_buf,
			senders_cache: Vec::new(),
		}
	}
}

pub fn draw_booster_tab(
	ui: &mut egui::Ui,
	booster: &mut BoosterController,
	state: &mut BoosterUiState,
	i18n: &crate::i18n::I18n,
	status_msg: &mut String,
) {
	if !state.initialized {
		state.draft = booster.config().clone();
		state.spout_in_buf = state.draft.spout_in.clone().unwrap_or_default();
		state.senders_cache = list_spout_senders();
		state.initialized = true;
	}

	let status = booster.status();
	let badge = if status.running {
		(i18n.t("booster.running"), theme::AUDIT_OK_GREEN)
	} else {
		(i18n.t("booster.stopped"), theme::TEXT_MUTED)
	};
	theme::section_header(ui, i18n.t("booster.header"), Some(badge));
	ui.add_space(4.0);

	ui.horizontal_wrapped(|ui| {
		if !status.running {
			if ui
				.add(
					egui::Button::new(RichText::new(i18n.t("booster.start")).strong().color(Color32::WHITE))
						.fill(theme::BTN_ACTIVE),
				)
				.clicked()
			{
				sync_spout_in_from_buf(state);
				match booster.start(Some(state.draft.clone())) {
					Ok(_) => *status_msg = "RigLens Booster started".to_owned(),
					Err(e) => *status_msg = format!("Booster start failed: {e}"),
				}
			}
		} else if ui
			.add(
				egui::Button::new(RichText::new(i18n.t("booster.stop")).strong().color(Color32::WHITE))
					.fill(Color32::from_rgb(153, 27, 27)),
			)
			.clicked()
		{
			let _ = booster.stop();
			*status_msg = "RigLens Booster stopped".to_owned();
		}

		if ui.button(i18n.t("booster.apply")).clicked() {
			sync_spout_in_from_buf(state);
			match booster.configure(state.draft.clone()) {
				Ok(st) => {
					state.draft = st.config;
					*status_msg = "Applied Booster configuration".to_owned();
				}
				Err(e) => *status_msg = format!("Booster configure error: {e}"),
			}
		}

		if ui.small_button(i18n.t("booster.refresh_spout")).clicked() {
			state.senders_cache = list_spout_senders();
			*status_msg = format!("Found {} active Spout2 sender(s)", state.senders_cache.len());
		}
	});

	ui.add_space(4.0);
	ui.horizontal(|ui| {
		let vts_col = if status.vts_connected {
			theme::AUDIT_OK_GREEN
		} else {
			theme::TEXT_MUTED
		};
		ui.label(RichText::new("VTS:").size(11.0).color(theme::TEXT_MUTED));
		ui.label(
			RichText::new(if status.vts_connected {
				i18n.t("booster.connected")
			} else {
				i18n.t("booster.offline")
			})
			.size(11.0)
			.strong()
			.color(vts_col),
		);
		ui.separator();
		let gpu_lbl = if status.gpu_adapter.is_empty() {
			i18n.t("booster.idle")
		} else {
			&status.gpu_adapter
		};
		ui.label(RichText::new(format!("GPU: {gpu_lbl}")).size(11.0).color(theme::TEXT_MUTED));
	});

	if let Some(ref err) = status.last_error {
		ui.label(
			RichText::new(err)
				.size(10.0)
				.color(theme::AUDIT_WARN_AMBER),
		);
	}

	ui.add_space(6.0);
	theme::section_header(ui, i18n.t("booster.bridge_header"), None);
	ui.add_space(2.0);
	ui.checkbox(&mut state.draft.enable_vts, i18n.t("booster.enable_vts"));
	ui.horizontal(|ui| {
		ui.label(RichText::new(i18n.t("booster.vts_url")).size(11.0).color(theme::TEXT_MUTED));
		ui.add_sized(
			[ui.available_width().max(80.0), 20.0],
			egui::TextEdit::singleline(&mut state.draft.vts_url),
		);
	});

	ui.horizontal(|ui| {
		ui.label(RichText::new(i18n.t("booster.spout_in")).size(11.0).color(theme::TEXT_MUTED));
		let combo_label = if state.spout_in_buf.trim().is_empty() {
			i18n.t("booster.none").to_owned()
		} else {
			state.spout_in_buf.clone()
		};
		egui::ComboBox::from_id_source("booster_spout_in_pick")
			.selected_text(combo_label)
			.width(120.0)
			.show_ui(ui, |ui| {
				if ui
					.selectable_label(state.spout_in_buf.trim().is_empty(), i18n.t("booster.none_phys_only"))
					.clicked()
				{
					state.spout_in_buf.clear();
					sync_spout_in_from_buf(state);
				}
				if ui
					.selectable_label(state.spout_in_buf == "VTubeStudioSpout", "VTubeStudioSpout")
					.clicked()
				{
					state.spout_in_buf = "VTubeStudioSpout".to_owned();
					sync_spout_in_from_buf(state);
				}
				let live_list: Vec<String> = if !status.spout_senders.is_empty() {
					status.spout_senders.clone()
				} else {
					state.senders_cache.clone()
				};
				for s in &live_list {
					if ui.selectable_label(state.spout_in_buf == *s, s).clicked() {
						state.spout_in_buf = s.clone();
						sync_spout_in_from_buf(state);
					}
				}
			});
		ui.add_sized(
			[ui.available_width().max(60.0), 20.0],
			egui::TextEdit::singleline(&mut state.spout_in_buf).hint_text(i18n.t("booster.disabled")),
		);
	});
	sync_spout_in_from_buf(state);

	ui.horizontal(|ui| {
		ui.label(RichText::new(i18n.t("booster.spout_out")).size(11.0).color(theme::TEXT_MUTED));
		ui.add_sized(
			[ui.available_width().max(80.0), 20.0],
			egui::TextEdit::singleline(&mut state.draft.spout_out),
		);
	});

	ui.add_space(6.0);
	theme::section_header(ui, i18n.t("booster.gpu_header"), None);
	ui.add_space(2.0);
	ui.checkbox(&mut state.draft.enable_gpu, i18n.t("booster.enable_gpu"));
	ui.checkbox(&mut state.draft.enable_soft_body, i18n.t("booster.enable_soft_body"));
	ui.add(egui::Slider::new(&mut state.draft.rim_intensity, 0.0..=2.0).text(i18n.t("booster.rim_intensity")));

	ui.horizontal(|ui| {
		ui.label(RichText::new(i18n.t("booster.rim_theme")).size(11.0).color(theme::TEXT_MUTED));
		egui::ComboBox::from_id_source("booster_rim_theme")
			.selected_text(&state.draft.rim_theme)
			.width(120.0)
			.show_ui(ui, |ui| {
				for theme_name in ["cyberpunk", "warm", "ocean", "screen"] {
					ui.selectable_value(&mut state.draft.rim_theme, theme_name.to_owned(), theme_name);
				}
			});
	});
	ui.add(egui::Slider::new(&mut state.draft.particle_count, 0..=2048).text(i18n.t("booster.gpu_particles")));

	ui.horizontal(|ui| {
		ui.label(RichText::new(i18n.t("booster.upscale")).size(11.0).color(theme::TEXT_MUTED));
		for (label, factor) in [("1.0x", 1.0f32), ("1.5x", 1.5), ("2.0x", 2.0)] {
			let selected = (state.draft.upscale_factor - factor).abs() < 0.05;
			if ui.selectable_label(selected, label).clicked() {
				state.draft.upscale_factor = factor;
			}
		}
	});
	ui.add(egui::Slider::new(&mut state.draft.upscale_sharpness, 0.0..=1.0).text(i18n.t("booster.dlss_sharpness")));
	ui.horizontal(|ui| {
		ui.label(RichText::new(i18n.t("booster.frame_gen")).size(11.0).color(theme::TEXT_MUTED));
		for (label, mult) in [(i18n.t("booster.frame_gen_off"), 1u32), ("2x", 2), ("3x", 3)] {
			ui.selectable_value(&mut state.draft.frame_gen_multiplier, mult, label);
		}
	});

	ui.add_space(6.0);
	theme::section_header(ui, i18n.t("booster.physics_header"), None);
	ui.add_space(2.0);
	ui.add(egui::Slider::new(&mut state.draft.tick_hz, 30.0..=240.0).text(i18n.t("booster.tick_hz")));
	ui.add(egui::Slider::new(&mut state.draft.wind_turbulence, 0.0..=2.0).text(i18n.t("booster.wind_turbulence")));

	ui.add_space(6.0);
	theme::section_header(ui, i18n.t("booster.telemetry_header"), None);
	ui.add_space(2.0);
	egui::Grid::new("booster_telemetry_grid")
		.num_columns(2)
		.spacing([12.0, 3.0])
		.show(ui, |ui| {
			telemetry_row(ui, i18n.t("booster.tel.ticks"), &format!("{}", status.ticks));
			telemetry_row(
				ui,
				i18n.t("booster.tel.frames"),
				&format!("{} / {}", status.frames_forwarded, status.generated_frames),
			);
			telemetry_row(
				ui,
				i18n.t("booster.tel.gpu"),
				&format!("{} ({:.2} ms)", status.gpu_frames_processed, status.avg_gpu_ms),
			);
			telemetry_row(
				ui,
				i18n.t("booster.tel.vts"),
				&format!("{} / {}", status.vts_injects, status.vts_errors),
			);
			telemetry_row(
				ui,
				i18n.t("booster.tel.wind"),
				&format!("{:+.2} / {:+.2}", status.wind[0], status.wind[1]),
			);
			telemetry_row(
				ui,
				i18n.t("booster.tel.hair_acc"),
				&format!(
					"({:+.2},{:+.2}) ({:+.2},{:+.2})",
					status.hair[0], status.hair[1], status.accessory[0], status.accessory[1]
				),
			);
			telemetry_row(
				ui,
				i18n.t("booster.tel.bounce"),
				&format!(
					"{:+.2} / {:+.2} / {:.2}",
					status.body_bounce_y, status.squash, status.voice_tremor
				),
			);
			let senders_lbl = if status.spout_senders.is_empty() {
				i18n.t("booster.tel.none").to_owned()
			} else {
				status.spout_senders.join(", ")
			};
			telemetry_row(ui, i18n.t("booster.tel.senders"), &senders_lbl);
		});
}

fn sync_spout_in_from_buf(state: &mut BoosterUiState) {
	let trimmed = state.spout_in_buf.trim();
	state.draft.spout_in = if trimmed.is_empty() {
		None
	} else {
		Some(trimmed.to_owned())
	};
}

fn telemetry_row(ui: &mut egui::Ui, label: &str, val: &str) {
	ui.label(RichText::new(label).size(11.0).color(theme::TEXT_MUTED));
	ui.label(RichText::new(val).size(11.0).monospace().color(theme::TEXT_PRIMARY));
	ui.end_row();
}

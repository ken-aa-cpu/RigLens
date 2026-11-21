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
		("RUNNING", theme::AUDIT_OK_GREEN)
	} else {
		("STOPPED", theme::TEXT_MUTED)
	};
	theme::section_header(ui, "RIGLENS BOOSTER M4", Some(badge));
	ui.add_space(4.0);

	ui.horizontal_wrapped(|ui| {
		if !status.running {
			if ui
				.add(
					egui::Button::new(RichText::new("Start Booster").strong().color(Color32::WHITE))
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
				egui::Button::new(RichText::new("Stop Booster").strong().color(Color32::WHITE))
					.fill(Color32::from_rgb(153, 27, 27)),
			)
			.clicked()
		{
			let _ = booster.stop();
			*status_msg = "RigLens Booster stopped".to_owned();
		}

		if ui.button("Apply Config").clicked() {
			sync_spout_in_from_buf(state);
			match booster.configure(state.draft.clone()) {
				Ok(st) => {
					state.draft = st.config;
					*status_msg = "Applied Booster configuration".to_owned();
				}
				Err(e) => *status_msg = format!("Booster configure error: {e}"),
			}
		}

		if ui.small_button("Refresh Spout").clicked() {
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
			RichText::new(if status.vts_connected { "Connected" } else { "Offline" })
				.size(11.0)
				.strong()
				.color(vts_col),
		);
		ui.separator();
		let gpu_lbl = if status.gpu_adapter.is_empty() {
			"Idle"
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
	theme::section_header(ui, "VTS & SPOUT2 BRIDGE", None);
	ui.add_space(2.0);
	ui.checkbox(&mut state.draft.enable_vts, "Enable VTS Port 8001 Physics Injection");
	ui.horizontal(|ui| {
		ui.label(RichText::new("VTS URL").size(11.0).color(theme::TEXT_MUTED));
		ui.add_sized(
			[ui.available_width().max(80.0), 20.0],
			egui::TextEdit::singleline(&mut state.draft.vts_url),
		);
	});

	ui.horizontal(|ui| {
		ui.label(RichText::new("Spout In").size(11.0).color(theme::TEXT_MUTED));
		let combo_label = if state.spout_in_buf.trim().is_empty() {
			"(None)".to_owned()
		} else {
			state.spout_in_buf.clone()
		};
		egui::ComboBox::from_id_source("booster_spout_in_pick")
			.selected_text(combo_label)
			.width(120.0)
			.show_ui(ui, |ui| {
				if ui
					.selectable_label(state.spout_in_buf.trim().is_empty(), "(None - Physics Only)")
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
			egui::TextEdit::singleline(&mut state.spout_in_buf).hint_text("Disabled"),
		);
	});
	sync_spout_in_from_buf(state);

	ui.horizontal(|ui| {
		ui.label(RichText::new("Spout Out").size(11.0).color(theme::TEXT_MUTED));
		ui.add_sized(
			[ui.available_width().max(80.0), 20.0],
			egui::TextEdit::singleline(&mut state.draft.spout_out),
		);
	});

	ui.add_space(6.0);
	theme::section_header(ui, "GPU FX & 2D-NATIVE DLSS (M2/M3)", None);
	ui.add_space(2.0);
	ui.checkbox(&mut state.draft.enable_gpu, "Enable wgpu Compute Pipeline");
	ui.checkbox(&mut state.draft.enable_soft_body, "XPBD Soft-Body Secondary Warp");
	ui.add(egui::Slider::new(&mut state.draft.rim_intensity, 0.0..=2.0).text("Rim Intensity"));

	ui.horizontal(|ui| {
		ui.label(RichText::new("Rim Theme").size(11.0).color(theme::TEXT_MUTED));
		egui::ComboBox::from_id_source("booster_rim_theme")
			.selected_text(&state.draft.rim_theme)
			.width(120.0)
			.show_ui(ui, |ui| {
				for theme_name in ["cyberpunk", "warm", "ocean", "screen"] {
					ui.selectable_value(&mut state.draft.rim_theme, theme_name.to_owned(), theme_name);
				}
			});
	});
	ui.add(egui::Slider::new(&mut state.draft.particle_count, 0..=2048).text("GPU Particles"));

	ui.horizontal(|ui| {
		ui.label(RichText::new("Upscale").size(11.0).color(theme::TEXT_MUTED));
		for (label, factor) in [("1.0x", 1.0f32), ("1.5x", 1.5), ("2.0x", 2.0)] {
			let selected = (state.draft.upscale_factor - factor).abs() < 0.05;
			if ui.selectable_label(selected, label).clicked() {
				state.draft.upscale_factor = factor;
			}
		}
	});
	ui.add(egui::Slider::new(&mut state.draft.upscale_sharpness, 0.0..=1.0).text("DLSS Sharpness"));
	ui.horizontal(|ui| {
		ui.label(RichText::new("Frame Gen").size(11.0).color(theme::TEXT_MUTED));
		for (label, mult) in [("1x Off", 1u32), ("2x", 2), ("3x", 3)] {
			ui.selectable_value(&mut state.draft.frame_gen_multiplier, mult, label);
		}
	});

	ui.add_space(6.0);
	theme::section_header(ui, "120HZ PHYSICS", None);
	ui.add_space(2.0);
	ui.add(egui::Slider::new(&mut state.draft.tick_hz, 30.0..=240.0).text("Tick Hz"));
	ui.add(egui::Slider::new(&mut state.draft.wind_turbulence, 0.0..=2.0).text("Wind Turbulence"));

	ui.add_space(6.0);
	theme::section_header(ui, "LIVE TELEMETRY", None);
	ui.add_space(2.0);
	egui::Grid::new("booster_telemetry_grid")
		.num_columns(2)
		.spacing([12.0, 3.0])
		.show(ui, |ui| {
			telemetry_row(ui, "Ticks", &format!("{}", status.ticks));
			telemetry_row(
				ui,
				"Frames Fwd / Gen",
				&format!("{} / {}", status.frames_forwarded, status.generated_frames),
			);
			telemetry_row(
				ui,
				"GPU Frames / ms",
				&format!("{} ({:.2} ms)", status.gpu_frames_processed, status.avg_gpu_ms),
			);
			telemetry_row(
				ui,
				"VTS Inj / Err",
				&format!("{} / {}", status.vts_injects, status.vts_errors),
			);
			telemetry_row(
				ui,
				"Wind X / Y",
				&format!("{:+.2} / {:+.2}", status.wind[0], status.wind[1]),
			);
			telemetry_row(
				ui,
				"Hair / Acc",
				&format!(
					"({:+.2},{:+.2}) ({:+.2},{:+.2})",
					status.hair[0], status.hair[1], status.accessory[0], status.accessory[1]
				),
			);
			telemetry_row(
				ui,
				"Bounce / Squash / Voice",
				&format!(
					"{:+.2} / {:+.2} / {:.2}",
					status.body_bounce_y, status.squash, status.voice_tremor
				),
			);
			let senders_lbl = if status.spout_senders.is_empty() {
				"none".to_owned()
			} else {
				status.spout_senders.join(", ")
			};
			telemetry_row(ui, "Spout Senders", &senders_lbl);
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

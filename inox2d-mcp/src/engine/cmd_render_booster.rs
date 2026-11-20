use std::fs;
use std::path::Path;

use inox2d::model::Model;
use inox2d::puppet::Puppet;
use json::JsonValue;
use riglens_booster::pipeline::BoosterControllerConfig;
use riglens_booster::transport::spout2::{get_active_spout_sender, list_spout_senders};

use crate::craft_ui::generate_rig_spec_card;
use crate::headless_render::{
	render_anim_gif, render_anim_sheet, render_diagnostic, render_diff, render_frame, render_grid, RenderOptions,
};

use super::Session;

fn serde_to_json(val: &serde_json::Value) -> Result<JsonValue, String> {
	let serialized = serde_json::to_string(val).map_err(|e| format!("JSON serialize error: {e}"))?;
	json::parse(&serialized).map_err(|e| format!("JSON parse error: {e}"))
}

fn apply_booster_config_patch(base: &BoosterControllerConfig, params: &JsonValue) -> BoosterControllerConfig {
	let mut cfg = base.clone();
	if let Some(s) = params["vts_url"].as_str() {
		cfg.vts_url = s.to_owned();
	}
	if let Some(v) = params["enable_vts"].as_bool() {
		cfg.enable_vts = v;
	}
	if params.has_key("spout_in") {
		if params["spout_in"].is_null() {
			cfg.spout_in = None;
		} else if let Some(s) = params["spout_in"].as_str() {
			let t = s.trim();
			cfg.spout_in = if t.is_empty() { None } else { Some(t.to_owned()) };
		}
	}
	if let Some(s) = params["spout_out"].as_str() {
		cfg.spout_out = s.to_owned();
	}
	if let Some(v) = params["tick_hz"].as_f32() {
		cfg.tick_hz = v;
	}
	if let Some(v) = params["wind_turbulence"].as_f32() {
		cfg.wind_turbulence = v;
	}
	if let Some(v) = params["enable_gpu"].as_bool() {
		cfg.enable_gpu = v;
	}
	if let Some(v) = params["enable_soft_body"].as_bool() {
		cfg.enable_soft_body = v;
	}
	if let Some(v) = params["rim_intensity"].as_f32() {
		cfg.rim_intensity = v;
	}
	if let Some(s) = params["rim_theme"].as_str() {
		cfg.rim_theme = s.to_owned();
	}
	if let Some(v) = params["particle_count"].as_u32() {
		cfg.particle_count = v;
	}
	if let Some(v) = params["upscale_factor"].as_f32() {
		cfg.upscale_factor = v;
	}
	if let Some(v) = params["upscale_sharpness"].as_f32() {
		cfg.upscale_sharpness = v;
	}
	if let Some(v) = params["frame_gen_multiplier"].as_u32() {
		cfg.frame_gen_multiplier = v;
	}
	cfg
}

impl Session {
	pub(super) fn exec_render_booster_cmd(&mut self, cmd: &str, params: &JsonValue) -> Result<JsonValue, String> {
		match cmd {
			"render.frame" => {
				let out = params["out"].as_str().unwrap_or("frame.png");
				let mut opts = RenderOptions::default();
				if let Some(w) = params["width"].as_u32() {
					opts.width = w;
				}
				if let Some(h) = params["height"].as_u32() {
					opts.height = h;
				}
				if let Some(s) = params["scale"].as_f32() {
					opts.scale = s;
				}
				if let Some(wf) = params["wireframe"].as_bool() {
					opts.wireframe = wf;
				}
				if let Some(dt) = params["dt"].as_f32() {
					opts.dt = dt;
				}
				let img = render_frame(&mut self.model, &self.param_overrides, &opts)?;
				img.save(out).map_err(|e| format!("Failed to save '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					width: opts.width,
					height: opts.height,
					wireframe: opts.wireframe
				})
			}
			"render.grid" => {
				let out = params["out"].as_str().unwrap_or("grid.png");
				let param_name = params["param"].as_str().unwrap_or("Head:: Yaw-Pitch");
				let tile_size = params["tile_size"].as_u32().unwrap_or(220);
				let wireframe = params["wireframe"].as_bool().unwrap_or(true);
				let img = render_grid(
					&mut self.model,
					param_name,
					tile_size,
					wireframe,
					&self.param_overrides,
				)?;
				let (w, h) = img.dimensions();
				img.save(out).map_err(|e| format!("Failed to save '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					param: param_name,
					width: w,
					height: h,
					wireframe: wireframe
				})
			}
			"render.anim_sheet" => {
				let out = params["out"].as_str().unwrap_or("anim_sheet.png");
				let anim = params["anim"].as_str();
				let frames = params["frames"].as_u32().unwrap_or(8);
				let tile_size = params["tile_size"].as_u32().unwrap_or(180);
				let wireframe = params["wireframe"].as_bool().unwrap_or(false);
				let img = render_anim_sheet(&mut self.model, anim, frames, tile_size, wireframe)?;
				let (w, h) = img.dimensions();
				img.save(out).map_err(|e| format!("Failed to save '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					frames: frames,
					width: w,
					height: h
				})
			}
			"render.diagnostic" => {
				let out = params["out"].as_str().unwrap_or("diagnostic.png");
				let width = params["width"].as_u32().unwrap_or(512);
				let height = params["height"].as_u32().unwrap_or(512);
				let scale = params["scale"].as_f32().unwrap_or(1.0);
				let img = render_diagnostic(&mut self.model, &self.param_overrides, width, height, scale)?;
				img.save(out).map_err(|e| format!("Failed to save '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					width: width,
					height: height
				})
			}
			"render.diff" => {
				let out = params["out"].as_str().unwrap_or("diff.png");
				let tile_size = params["tile_size"].as_u32().unwrap_or(256);
				let scale = params["scale"].as_f32().unwrap_or(0.75);
				let snap = if let Some(cp_name) = params["checkpoint"].as_str() {
					self.checkpoints
						.get(cp_name)
						.cloned()
						.ok_or_else(|| format!("Checkpoint '{cp_name}' not found"))?
				} else if let Some(prev) = self.undo_stack.last() {
					prev.clone()
				} else {
					return Err("No checkpoint specified and undo history is empty".to_owned());
				};
				let before_puppet = Puppet::new_from_json(&snap.puppet_json)
					.map_err(|e| format!("Failed to restore baseline: {e}"))?;
				let mut before_model = Model {
					puppet: before_puppet,
					textures: snap.textures,
					vendors: snap.vendors,
				};
				let (img, changed_pct) =
					render_diff(&mut before_model, &mut self.model, &self.param_overrides, tile_size, scale)?;
				let (w, h) = img.dimensions();
				img.save(out).map_err(|e| format!("Failed to save '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					width: w,
					height: h,
					changed_pixel_pct: changed_pct
				})
			}
			"render.gif" => {
				let out = params["out"].as_str().unwrap_or("preview.gif");
				let anim = params["anim"].as_str();
				let frames = params["frames"].as_u32().unwrap_or(24);
				let fps = params["fps"].as_u32().unwrap_or(15);
				let width = params["width"].as_u32().unwrap_or(320);
				let height = params["height"].as_u32().unwrap_or(320);
				let scale = params["scale"].as_f32().unwrap_or(0.85);
				let wireframe = params["wireframe"].as_bool().unwrap_or(false);
				let gif_bytes = render_anim_gif(&mut self.model, anim, frames, fps, width, height, scale, wireframe)?;
				fs::write(out, &gif_bytes).map_err(|e| format!("Failed to write '{out}': {e}"))?;
				Ok(json::object! {
					out: out,
					frames: frames,
					fps: fps,
					bytes: gif_bytes.len()
				})
			}
			"craft.figcraft_card" => {
				let out = params["out"].as_str().unwrap_or("spec_card.png");
				generate_rig_spec_card(&mut self.model, Path::new(out))
			}
			"booster.status" => {
				let st = self.booster.status();
				let v = serde_json::to_value(&st).map_err(|e| format!("Status serialize error: {e}"))?;
				serde_to_json(&v)
			}
			"booster.list_spout" => Ok(json::object! {
				active_sender: get_active_spout_sender(),
				senders: list_spout_senders()
			}),
			"booster.configure" => {
				let cfg = apply_booster_config_patch(self.booster.config(), params);
				let st = self.booster.configure(cfg)?;
				let v = serde_json::to_value(&st).map_err(|e| format!("Status serialize error: {e}"))?;
				serde_to_json(&v)
			}
			"booster.start" => {
				let next_cfg = if params.is_object() && params.entries().next().is_some() {
					Some(apply_booster_config_patch(self.booster.config(), params))
				} else {
					None
				};
				let st = self.booster.start(next_cfg)?;
				let v = serde_json::to_value(&st).map_err(|e| format!("Status serialize error: {e}"))?;
				serde_to_json(&v)
			}
			"booster.stop" => {
				let st = self.booster.stop();
				let v = serde_json::to_value(&st).map_err(|e| format!("Status serialize error: {e}"))?;
				serde_to_json(&v)
			}
			_ => Err(format!("Unknown command: '{cmd}'")),
		}
	}
}

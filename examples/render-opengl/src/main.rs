use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use std::{error::Error, fs};

use inox2d::formats::inp::parse_inp;
use inox2d::model::Model;
use inox2d::node::components::Mesh;
use inox2d::render::InoxRendererExt;
use inox2d_opengl::OpenglRenderer;

use clap::Parser;
use glam::{vec2, Vec2};
use tracing_subscriber::{filter::LevelFilter, fmt, prelude::*};

use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::EventLoopWindowTarget;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::WindowBuilder;

use common::scene::ExampleSceneController;

use crate::app_frame::{App, AppFrame};

mod app_frame;

#[derive(Parser, Debug)]
#[command(about = "Interactive Inox2D OpenGL puppet viewer with live .inp hot-reload")]
struct Cli {
	/// Path to the .inp file to render and watch for live hot-reload.
	inp_path: PathBuf,
	/// Disable automatic .inp mtime hot-reloading.
	#[arg(long)]
	no_hot_reload: bool,
	/// Initial camera zoom scale (auto-fits puppet height when omitted).
	#[arg(long)]
	scale: Option<f32>,
}

const VISEME_PRESETS: [(&str, f32, f32); 6] = [
	("Neutral", 0.0, 0.0),
	("A", 0.0, 1.0),
	("I", 1.0, 0.35),
	("U", -1.0, 0.45),
	("E", 0.75, 0.7),
	("O", -0.45, 0.9),
];

const EXPR_PARAMS: [&str; 5] = [
	"Expr:: Blush",
	"Expr:: Gloom",
	"Expr:: Tears",
	"Expr:: Angry",
	"Expr:: Shock",
];

fn load_and_init_model(path: &Path) -> Result<Model, String> {
	let data = fs::read(path).map_err(|e| format!("Failed to read '{}': {e}", path.display()))?;
	let mut model = parse_inp(data.as_slice()).map_err(|e| format!("Failed to parse '{}': {e}", path.display()))?;
	model.puppet.init_all();
	Ok(model)
}

fn fit_scale_for_model(model: &Model, width: u32, height: u32) -> f32 {
	let mut min_y = f32::INFINITY;
	let mut max_y = f32::NEG_INFINITY;
	for node in model.puppet.nodes.iter() {
		if let Some(mesh) = model.puppet.node_comps.get::<Mesh>(node.uuid) {
			let oy = node.trans_offset.translation.y;
			for &v in &mesh.vertices {
				min_y = min_y.min(v.y + oy);
				max_y = max_y.max(v.y + oy);
			}
		}
	}
	if !min_y.is_finite() || !max_y.is_finite() || (max_y - min_y) < 8.0 {
		return 0.25;
	}
	let span_y = (max_y - min_y).max(64.0);
	let target_px = (width.min(height) as f32) * 0.68;
	(target_px / span_y).clamp(0.08, 1.8)
}

fn main() -> Result<(), Box<dyn Error>> {
	tracing_subscriber::registry()
		.with(fmt::layer())
		.with(LevelFilter::INFO)
		.init();

	let cli = Cli::parse();
	let model = load_and_init_model(&cli.inp_path)?;
	let last_modified = fs::metadata(&cli.inp_path).and_then(|m| m.modified()).ok();

	tracing::info!(
		"Puppet loaded: {}",
		model
			.puppet
			.meta
			.name
			.as_deref()
			.unwrap_or("<no puppet name specified in file>")
	);
	tracing::info!(
		"Controls: Mouse=Track Head/Eyes | Drag=Pan | Wheel=Zoom | Space=Toggle Idle Anim | B=Blink | M=Viseme | 1-5=Expr | F=Fit | R=Reload | Esc=Quit"
	);

	let width = 768u32;
	let height = 768u32;
	let initial_scale = cli.scale.unwrap_or_else(|| fit_scale_for_model(&model, width, height));

	let window_builder = WindowBuilder::new()
		.with_transparent(true)
		.with_resizable(true)
		.with_inner_size(winit::dpi::PhysicalSize::new(width, height))
		.with_title(format!(
			"Inox2D Live Viewer — {}",
			cli.inp_path
				.file_name()
				.and_then(|s| s.to_str())
				.unwrap_or("puppet.inp")
		));

	let events = AppFrame::init(window_builder)?;
	let app = Inox2dOpenglExampleApp {
		on_window: None,
		model,
		inp_path: cli.inp_path,
		hot_reload: !cli.no_hot_reload,
		last_modified,
		last_reload_check: Instant::now(),
		width,
		height,
		initial_scale,
		cursor_norm: Vec2::ZERO,
		anim_mode: false,
		blink_hold: false,
		viseme_idx: 0,
		expr_toggles: [false; 5],
	};
	events.run(app)?;

	Ok(())
}

struct Inox2dOpenglExampleApp {
	on_window: Option<(OpenglRenderer, ExampleSceneController)>,
	model: Model,
	inp_path: PathBuf,
	hot_reload: bool,
	last_modified: Option<SystemTime>,
	last_reload_check: Instant,
	width: u32,
	height: u32,
	initial_scale: f32,
	cursor_norm: Vec2,
	anim_mode: bool,
	blink_hold: bool,
	viseme_idx: usize,
	expr_toggles: [bool; 5],
}

impl Inox2dOpenglExampleApp {
	fn reload_from_disk(&mut self, force: bool) {
		let mtime = match fs::metadata(&self.inp_path).and_then(|m| m.modified()) {
			Ok(t) => Some(t),
			Err(_) => return,
		};
		if !force && mtime == self.last_modified {
			return;
		}
		let new_model = match load_and_init_model(&self.inp_path) {
			Ok(m) => m,
			Err(err) => {
				// File may still be mid-write if external tool doesn't use atomic rename; try again on next tick.
				tracing::debug!("Hot-reload skipped ({err})");
				return;
			}
		};
		self.last_modified = mtime;
		self.model = new_model;

		if let Some((old_renderer, scene_ctrl)) = self.on_window.take() {
			let saved_cam = old_renderer.camera.clone();
			let gl = old_renderer.into_gl();
			match OpenglRenderer::new(gl, &self.model) {
				Ok(mut renderer) => {
					renderer.camera = saved_cam;
					renderer.resize(self.width, self.height);
					self.on_window = Some((renderer, scene_ctrl));
					tracing::info!("Hot-reloaded '{}'", self.inp_path.display());
				}
				Err(err) => {
					tracing::error!("Failed to recreate OpenGL renderer after hot-reload: {err}");
				}
			}
		}
	}
}

impl App for Inox2dOpenglExampleApp {
	fn resume_window(&mut self, gl: glow::Context) {
		match OpenglRenderer::new(gl, &self.model) {
			Ok(mut renderer) => {
				renderer.resize(self.width, self.height);
				renderer.camera.scale = Vec2::splat(self.initial_scale);
				let scene_ctrl = ExampleSceneController::new(&renderer.camera, 0.5);
				self.on_window = Some((renderer, scene_ctrl));
			}
			Err(err) => {
				eprintln!("Error initializing Inox2D renderer: {err}");
			}
		}
	}

	fn resize(&mut self, width: i32, height: i32) {
		self.width = width.max(1) as u32;
		self.height = height.max(1) as u32;
		if let Some((renderer, _)) = &mut self.on_window {
			renderer.resize(self.width, self.height);
		}
	}

	fn draw(&mut self) {
		if self.hot_reload && self.last_reload_check.elapsed() >= Duration::from_millis(250) {
			self.last_reload_check = Instant::now();
			self.reload_from_disk(false);
		}

		let Some((renderer, scene_ctrl)) = &mut self.on_window else {
			return;
		};

		scene_ctrl.update(&mut renderer.camera);
		renderer.clear();

		let puppet = &mut self.model.puppet;
		let t = scene_ctrl.current_elapsed();
		let dt = scene_ctrl.dt();

		// Evaluate parameters for current interaction mode before beginning frame.
		let mut param_map: HashMap<String, Vec2> = HashMap::new();
		if self.anim_mode {
			let anim = puppet
				.animations
				.get("Idle")
				.or_else(|| puppet.animations.values().next())
				.cloned();
			if let Some(anim) = anim {
				let step = if anim.timestep > 0.0001 { anim.timestep } else { 1.0 / 30.0 };
				let frame = t / step;
				param_map = anim.eval_at_frame(frame, &puppet.params);
			}
		} else {
			param_map.insert("Head:: Yaw-Pitch".to_owned(), self.cursor_norm);
			param_map.insert("Head:: Roll".to_owned(), vec2(self.cursor_norm.x * 0.35, 0.0));
			param_map.insert("Eye:: Left:: Gaze".to_owned(), self.cursor_norm);
			param_map.insert("Eye:: Right:: Gaze".to_owned(), self.cursor_norm);
			param_map.insert("Eye:: Gaze".to_owned(), self.cursor_norm);
			param_map.insert("Body:: Breath".to_owned(), vec2((t * 1.6).sin() * 0.5 + 0.5, 0.0));

			// Natural periodic blink every ~4.2s unless held manually.
			let cycle = t.rem_euclid(4.2);
			let auto_blink = if cycle > 4.02 { 0.0 } else { 1.0 };
			let eye_open = if self.blink_hold { 0.0 } else { auto_blink };
			param_map.insert("Eye:: Left:: Open".to_owned(), vec2(eye_open, 0.0));
			param_map.insert("Eye:: Right:: Open".to_owned(), vec2(eye_open, 0.0));

			let (_label, v_form, v_open) = VISEME_PRESETS[self.viseme_idx];
			param_map.insert("Mouth:: Viseme".to_owned(), vec2(v_form, v_open));
			param_map.insert("Mouth:: Form".to_owned(), vec2(v_form, 0.0));
			param_map.insert("Mouth:: Open".to_owned(), vec2(v_open, 0.0));
		}

		if self.blink_hold {
			param_map.insert("Eye:: Left:: Open".to_owned(), Vec2::ZERO);
			param_map.insert("Eye:: Right:: Open".to_owned(), Vec2::ZERO);
		}
		for (idx, pname) in EXPR_PARAMS.iter().enumerate() {
			if self.expr_toggles[idx] {
				param_map.insert((*pname).to_owned(), vec2(1.0, 0.0));
			}
		}

		puppet.begin_frame();
		if let Some(param_ctx) = puppet.param_ctx.as_mut() {
			for (name, val) in param_map {
				let _ = param_ctx.set(&name, val);
			}
		}
		puppet.end_frame(dt);

		renderer.on_begin_draw(puppet);
		renderer.draw(puppet);
		renderer.on_end_draw(puppet);
	}

	fn handle_window_event(&mut self, event: WindowEvent, window_target: &EventLoopWindowTarget<()>) {
		match &event {
			WindowEvent::CloseRequested => {
				window_target.exit();
				return;
			}
			WindowEvent::CursorMoved { position, .. } => {
				let w = (self.width as f32).max(1.0);
				let h = (self.height as f32).max(1.0);
				self.cursor_norm = vec2(
					((position.x as f32 / w) * 2.0 - 1.0).clamp(-1.0, 1.0),
					((position.y as f32 / h) * 2.0 - 1.0).clamp(-1.0, 1.0),
				);
			}
			WindowEvent::KeyboardInput {
				event:
					KeyEvent {
						physical_key: PhysicalKey::Code(code),
						state,
						repeat,
						..
					},
				..
			} => match (code, state, repeat) {
				(KeyCode::Escape, ElementState::Pressed, _) => {
					window_target.exit();
					return;
				}
				(KeyCode::KeyB, ElementState::Pressed, _) => {
					self.blink_hold = true;
				}
				(KeyCode::KeyB, ElementState::Released, _) => {
					self.blink_hold = false;
				}
				(KeyCode::Space, ElementState::Pressed, false) => {
					self.anim_mode = !self.anim_mode;
					tracing::info!(
						"Mode: {}",
						if self.anim_mode {
							"Animation Playback (Idle)"
						} else {
							"Interactive Cursor Tracking"
						}
					);
				}
				(KeyCode::KeyM, ElementState::Pressed, false) => {
					self.viseme_idx = (self.viseme_idx + 1) % VISEME_PRESETS.len();
					tracing::info!("Mouth Viseme -> {}", VISEME_PRESETS[self.viseme_idx].0);
				}
				(KeyCode::KeyR, ElementState::Pressed, false) => {
					self.reload_from_disk(true);
				}
				(KeyCode::KeyF, ElementState::Pressed, false) => {
					let fit = fit_scale_for_model(&self.model, self.width, self.height);
					if let Some((renderer, _)) = &mut self.on_window {
						renderer.camera.position = Vec2::ZERO;
						renderer.camera.scale = Vec2::splat(fit);
					}
				}
				(KeyCode::Digit1, ElementState::Pressed, false) => {
					self.expr_toggles[0] = !self.expr_toggles[0];
					tracing::info!("{} -> {}", EXPR_PARAMS[0], self.expr_toggles[0]);
				}
				(KeyCode::Digit2, ElementState::Pressed, false) => {
					self.expr_toggles[1] = !self.expr_toggles[1];
					tracing::info!("{} -> {}", EXPR_PARAMS[1], self.expr_toggles[1]);
				}
				(KeyCode::Digit3, ElementState::Pressed, false) => {
					self.expr_toggles[2] = !self.expr_toggles[2];
					tracing::info!("{} -> {}", EXPR_PARAMS[2], self.expr_toggles[2]);
				}
				(KeyCode::Digit4, ElementState::Pressed, false) => {
					self.expr_toggles[3] = !self.expr_toggles[3];
					tracing::info!("{} -> {}", EXPR_PARAMS[3], self.expr_toggles[3]);
				}
				(KeyCode::Digit5, ElementState::Pressed, false) => {
					self.expr_toggles[4] = !self.expr_toggles[4];
					tracing::info!("{} -> {}", EXPR_PARAMS[4], self.expr_toggles[4]);
				}
				_ => {}
			},
			_ => {}
		}

		if let Some((renderer, scene_ctrl)) = &mut self.on_window {
			scene_ctrl.interact(&event, &renderer.camera);
		}
	}
}

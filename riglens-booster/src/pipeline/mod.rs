//! Dual-channel orchestrator: 120Hz physics + VTS Port 8001 + M2/M3 GPU compute + Spout2 frame forwarding.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::gpu::ambient::AmbientTheme;
use crate::gpu::{GpuEffectsConfig, GpuError, GpuFrameProcessor};
use crate::physics::{BoosterPhysicsConfig, BoosterPhysicsEngine, BoosterPhysicsOutput, MotionPoseInput, WindField};
use crate::protocol::vts::{VtsClient, VtsClientConfig};
use crate::transport::spout2::{get_active_spout_sender, list_spout_senders, Spout2Receiver, Spout2Sender};
use crate::transport::{FrameSink, FrameSource};

#[derive(Clone, Debug)]
pub struct BoosterConfig {
	pub tick_hz: f32,
	pub vts_poll_hz: f32,
	pub enable_vts: bool,
	pub enable_gpu: bool,
	pub vts: VtsClientConfig,
	pub physics: BoosterPhysicsConfig,
	pub gpu: GpuEffectsConfig,
}

impl Default for BoosterConfig {
	fn default() -> Self {
		Self {
			tick_hz: 120.0,
			vts_poll_hz: 60.0,
			enable_vts: true,
			enable_gpu: true,
			vts: VtsClientConfig::default(),
			physics: BoosterPhysicsConfig::default(),
			gpu: GpuEffectsConfig::default(),
		}
	}
}

#[derive(Clone, Debug, Default)]
pub struct BoosterTelemetry {
	pub ticks: u64,
	pub frames_forwarded: u64,
	pub generated_frames: u64,
	pub gpu_frames_processed: u64,
	pub avg_gpu_ms: f32,
	pub vts_injects: u64,
	pub vts_errors: u64,
	pub last_output: BoosterPhysicsOutput,
}

/// Coordinates Port 8001 bidirectional VTS tracking/physics, M2 `wgpu` effects, and Spout2 transport.
pub struct BoosterPipeline {
	config: BoosterConfig,
	physics: BoosterPhysicsEngine,
	vts: Option<VtsClient>,
	gpu: Option<GpuFrameProcessor>,
	source: Option<Box<dyn FrameSource>>,
	sink: Option<Box<dyn FrameSink>>,
}

impl BoosterPipeline {
	pub fn new(
		config: BoosterConfig,
		source: Option<Box<dyn FrameSource>>,
		sink: Option<Box<dyn FrameSink>>,
	) -> Self {
		let physics = BoosterPhysicsEngine::new(config.physics.clone());
		Self {
			config,
			physics,
			vts: None,
			gpu: None,
			source,
			sink,
		}
	}

	/// Initialize the M2 `wgpu` compute/render processor asynchronously (safe inside Tokio runtime).
	pub async fn init_gpu(&mut self) -> Result<Option<String>, GpuError> {
		if !self.config.enable_gpu {
			return Ok(None);
		}
		let proc = GpuFrameProcessor::new_async(self.config.gpu.clone()).await?;
		let info = proc.adapter_info().to_owned();
		info!("Initialized M2 GPU pipeline on adapter: {info}");
		self.gpu = Some(proc);
		Ok(Some(info))
	}

	/// Connect to VTS if enabled, authenticate, and register all `RigLens*` physics parameters.
	pub async fn init_vts(&mut self) -> bool {
		if !self.config.enable_vts {
			return false;
		}
		match VtsClient::connect(self.config.vts.clone()).await {
			Ok(mut client) => {
				if let Err(e) = client.authenticate().await {
					warn!("VTS authentication failed: {e}");
					return false;
				}
				if let Err(e) = client.register_booster_parameters().await {
					warn!("VTS parameter registration failed: {e}");
				}
				info!("Connected and authenticated with VTube Studio at {}", self.config.vts.url);
				self.vts = Some(client);
				true
			}
			Err(e) => {
				warn!("Unable to connect to VTube Studio at {}: {e}", self.config.vts.url);
				false
			}
		}
	}

	/// Run the live dual-channel loop until `stop` becomes true (or `max_ticks` if provided),
	/// optionally streaming live snapshots into `live_telemetry` after every tick.
	pub async fn run_with_telemetry(
		&mut self,
		stop: Arc<AtomicBool>,
		max_ticks: Option<u64>,
		live_telemetry: Option<Arc<Mutex<BoosterTelemetry>>>,
	) -> BoosterTelemetry {
		let tick_hz = self.config.tick_hz.clamp(15.0, 240.0);
		let poll_every_n = ((tick_hz / self.config.vts_poll_hz.clamp(1.0, tick_hz)).round() as u64).max(1);
		let tick_period = Duration::from_secs_f32(1.0 / tick_hz);
		let mut interval = tokio::time::interval(tick_period);
		interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

		let mut telemetry = BoosterTelemetry::default();
		let mut last_pose = MotionPoseInput::default();
		let mut prev_t = Instant::now();
		let mut gpu_total_ms = 0.0f32;
		let mut first_frame_logged = false;

		while !stop.load(Ordering::Relaxed) {
			if let Some(limit) = max_ticks {
				if telemetry.ticks >= limit {
					break;
				}
			}
			interval.tick().await;
			let now = Instant::now();
			let dt = (now - prev_t).as_secs_f32().clamp(1.0 / 1000.0, 0.1);
			prev_t = now;

			if let Some(ref mut client) = self.vts {
				if telemetry.ticks % poll_every_n == 0 {
					match client.poll_motion_pose(0.0).await {
						Ok(p) => last_pose = p,
						Err(e) => {
							telemetry.vts_errors += 1;
							warn!("VTS poll error: {e}");
						}
					}
				}
			}

			let phys_out = self.physics.step(&last_pose, dt);
			if let Some(ref mut client) = self.vts {
				let params = phys_out.to_vts_parameters();
				if let Err(e) = client.inject_parameters(last_pose.face_found, "set", &params).await {
					telemetry.vts_errors += 1;
					warn!("VTS inject error: {e}");
				} else {
					telemetry.vts_injects += 1;
				}
			}

			if let (Some(ref mut src), Some(ref mut dst)) = (&mut self.source, &mut self.sink) {
				if let Ok(Some(frame)) = src.recv_frame() {
					if !first_frame_logged {
						first_frame_logged = true;
						println!(
							"[Spout2] Receiving {}x{} RGBA frames cleanly -> processing & forwarding.",
							frame.width, frame.height
						);
					}
					let out_frames = if let Some(ref mut gpu) = self.gpu {
						let t0 = Instant::now();
						match gpu.process_frame_with_gen(&frame, &phys_out.motion_hint, dt, None) {
							Ok(processed) => {
								gpu_total_ms += t0.elapsed().as_secs_f32() * 1000.0;
								telemetry.gpu_frames_processed += 1;
								telemetry.avg_gpu_ms = gpu_total_ms / telemetry.gpu_frames_processed as f32;
								if processed.len() > 1 {
									telemetry.generated_frames += (processed.len() - 1) as u64;
								}
								processed
							}
							Err(e) => {
								warn!("GPU process_frame_with_gen error (passing raw frame through): {e}");
								vec![frame]
							}
						}
					} else {
						vec![frame]
					};
					for out_frame in out_frames {
						if dst.send_frame(&out_frame).is_ok() {
							telemetry.frames_forwarded += 1;
						}
					}
				}
			}

			telemetry.last_output = phys_out;
			telemetry.ticks += 1;
			if let Some(ref shared) = live_telemetry {
				if let Ok(mut guard) = shared.try_lock() {
					*guard = telemetry.clone();
				}
			}
		}

		if let Some(ref shared) = live_telemetry {
			if let Ok(mut guard) = shared.lock() {
				*guard = telemetry.clone();
			}
		}

		telemetry
	}

	/// Run the live dual-channel loop until `stop` becomes true (or `max_ticks` if provided).
	pub async fn run(
		&mut self,
		stop: Arc<AtomicBool>,
		max_ticks: Option<u64>,
	) -> BoosterTelemetry {
		self.run_with_telemetry(stop, max_ticks, None).await
	}
}

/// Serializable runtime configuration for managing `riglens-booster` inside GUI/MCP hosts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoosterControllerConfig {
	pub vts_url: String,
	pub enable_vts: bool,
	pub spout_in: Option<String>,
	pub spout_out: String,
	pub tick_hz: f32,
	pub wind_turbulence: f32,
	pub enable_gpu: bool,
	pub enable_soft_body: bool,
	pub rim_intensity: f32,
	pub rim_theme: String,
	pub particle_count: u32,
	pub upscale_factor: f32,
	pub upscale_sharpness: f32,
	pub frame_gen_multiplier: u32,
}

impl Default for BoosterControllerConfig {
	fn default() -> Self {
		Self {
			vts_url: "ws://127.0.0.1:8001".to_owned(),
			enable_vts: true,
			spout_in: Some("VTubeStudioSpout".to_owned()),
			spout_out: "RigLensBooster".to_owned(),
			tick_hz: 120.0,
			wind_turbulence: 0.35,
			enable_gpu: true,
			enable_soft_body: true,
			rim_intensity: 0.75,
			rim_theme: "cyberpunk".to_owned(),
			particle_count: 384,
			upscale_factor: 1.0,
			upscale_sharpness: 0.65,
			frame_gen_multiplier: 1,
		}
	}
}

/// Live status snapshot exposed by [`BoosterController`] to desktop UI and MCP tools.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoosterStatus {
	pub running: bool,
	pub vts_connected: bool,
	pub gpu_adapter: String,
	pub spout_senders: Vec<String>,
	pub active_spout_sender: Option<String>,
	pub last_error: Option<String>,
	pub ticks: u64,
	pub frames_forwarded: u64,
	pub generated_frames: u64,
	pub gpu_frames_processed: u64,
	pub avg_gpu_ms: f32,
	pub vts_injects: u64,
	pub vts_errors: u64,
	pub wind: [f32; 2],
	pub hair: [f32; 2],
	pub accessory: [f32; 2],
	pub body_bounce_y: f32,
	pub squash: f32,
	pub voice_tremor: f32,
	pub config: BoosterControllerConfig,
}

#[derive(Default)]
struct BoosterLiveState {
	running: bool,
	vts_connected: bool,
	gpu_adapter: String,
	last_error: Option<String>,
}

/// Thread-safe background lifecycle controller for starting, stopping, and polling [`BoosterPipeline`].
pub struct BoosterController {
	config: BoosterControllerConfig,
	stop: Arc<AtomicBool>,
	live: Arc<Mutex<BoosterLiveState>>,
	telemetry: Arc<Mutex<BoosterTelemetry>>,
	worker: Option<std::thread::JoinHandle<()>>,
}

impl Default for BoosterController {
	fn default() -> Self {
		Self {
			config: BoosterControllerConfig::default(),
			stop: Arc::new(AtomicBool::new(false)),
			live: Arc::new(Mutex::new(BoosterLiveState::default())),
			telemetry: Arc::new(Mutex::new(BoosterTelemetry::default())),
			worker: None,
		}
	}
}

impl BoosterController {
	pub fn config(&self) -> &BoosterControllerConfig {
		&self.config
	}

	pub fn configure(&mut self, cfg: BoosterControllerConfig) -> Result<BoosterStatus, String> {
		let was_running = self.is_worker_running();
		self.config = cfg;
		if was_running {
			self.stop();
			return self.start(None);
		}
		Ok(self.status())
	}

	pub fn start(&mut self, cfg: Option<BoosterControllerConfig>) -> Result<BoosterStatus, String> {
		if self.worker.is_some() {
			self.stop();
		}
		if let Some(new_cfg) = cfg {
			self.config = new_cfg;
		}
		let run_cfg = self.config.clone();
		let stop_flag = Arc::new(AtomicBool::new(false));
		self.stop = stop_flag.clone();
		if let Ok(mut live) = self.live.lock() {
			live.running = true;
			live.vts_connected = false;
			live.gpu_adapter = "initializing".to_owned();
			live.last_error = None;
		}
		if let Ok(mut tel) = self.telemetry.lock() {
			*tel = BoosterTelemetry::default();
		}

		let live_ref = self.live.clone();
		let tel_ref = self.telemetry.clone();
		let handle = std::thread::Builder::new()
			.name("riglens-booster-worker".to_owned())
			.spawn(move || {
				let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
					Ok(rt) => rt,
					Err(e) => {
						if let Ok(mut live) = live_ref.lock() {
							live.running = false;
							live.last_error = Some(format!("Tokio runtime init failed: {e}"));
						}
						return;
					}
				};
				rt.block_on(async move {
					let mut errors = Vec::new();
					let spout_in_name = run_cfg
						.spout_in
						.as_deref()
						.map(str::trim)
						.filter(|s| !s.is_empty())
						.map(str::to_owned);
					let source: Option<Box<dyn FrameSource>> = match spout_in_name {
						Some(ref name) => match Spout2Receiver::new(name) {
							Ok(rx) => Some(Box::new(rx)),
							Err(e) => {
								errors.push(format!("Spout2 receiver '{name}': {e}"));
								None
							}
						},
						None => None,
					};
					let spout_out_name = run_cfg.spout_out.trim();
					let sink: Option<Box<dyn FrameSink>> = if source.is_some() && !spout_out_name.is_empty() {
						match Spout2Sender::new(spout_out_name) {
							Ok(tx) => Some(Box::new(tx)),
							Err(e) => {
								errors.push(format!("Spout2 sender '{spout_out_name}': {e}"));
								None
							}
						}
					} else {
						None
					};

					let gpu_cfg = GpuEffectsConfig {
						enable_soft_body: run_cfg.enable_soft_body,
						enable_rim_light: run_cfg.rim_intensity > 0.0,
						rim_intensity: run_cfg.rim_intensity,
						ambient_theme: AmbientTheme::parse(&run_cfg.rim_theme).unwrap_or(AmbientTheme::Cyberpunk),
						particle_count: run_cfg.particle_count,
						upscale_factor: run_cfg.upscale_factor,
						upscale_sharpness: run_cfg.upscale_sharpness,
						frame_gen_multiplier: run_cfg.frame_gen_multiplier.clamp(1, 3),
						..Default::default()
					};

					let pipeline_cfg = BoosterConfig {
						tick_hz: run_cfg.tick_hz,
						enable_vts: run_cfg.enable_vts,
						enable_gpu: run_cfg.enable_gpu && source.is_some(),
						vts: VtsClientConfig {
							url: run_cfg.vts_url.clone(),
							..Default::default()
						},
						physics: BoosterPhysicsConfig {
							wind: WindField {
								turbulence: run_cfg.wind_turbulence,
								..Default::default()
							},
							..Default::default()
						},
						gpu: gpu_cfg,
						..Default::default()
					};

					let mut pipeline = BoosterPipeline::new(pipeline_cfg, source, sink);
					let gpu_label = match pipeline.init_gpu().await {
						Ok(Some(info)) => info,
						Ok(None) => {
							if run_cfg.enable_gpu {
								"idle (no spout_in)".to_owned()
							} else {
								"disabled".to_owned()
							}
						}
						Err(e) => {
							errors.push(format!("GPU init fallback: {e}"));
							"fallback".to_owned()
						}
					};
					let vts_ok = pipeline.init_vts().await;
					if run_cfg.enable_vts && !vts_ok {
						errors.push(format!("VTS not connected at {}", run_cfg.vts_url));
					}
					if let Ok(mut live) = live_ref.lock() {
						live.gpu_adapter = gpu_label;
						live.vts_connected = vts_ok;
						live.last_error = if errors.is_empty() {
							None
						} else {
							Some(errors.join("; "))
						};
					}

					let _ = pipeline.run_with_telemetry(stop_flag, None, Some(tel_ref)).await;
					if let Ok(mut live) = live_ref.lock() {
						live.running = false;
					}
				});
			})
			.map_err(|e| {
				if let Ok(mut live) = self.live.lock() {
					live.running = false;
					live.last_error = Some(format!("Failed to spawn booster thread: {e}"));
				}
				format!("Failed to spawn booster thread: {e}")
			})?;
		self.worker = Some(handle);
		Ok(self.status())
	}

	pub fn stop(&mut self) -> BoosterStatus {
		self.stop.store(true, Ordering::Relaxed);
		if let Some(handle) = self.worker.take() {
			let _ = handle.join();
		}
		if let Ok(mut live) = self.live.lock() {
			live.running = false;
		}
		self.status()
	}

	/// Fast non-blocking check whether the background booster worker is alive and running.
	pub fn is_running(&self) -> bool {
		self.is_worker_running()
			&& self.live.lock().map(|g| g.running).unwrap_or(false)
	}

	/// Snapshot latest physics & frame telemetry without enumerating system Spout2 senders.
	pub fn latest_telemetry(&self) -> BoosterTelemetry {
		self.telemetry.lock().map(|g| g.clone()).unwrap_or_default()
	}

	pub fn status(&self) -> BoosterStatus {
		let worker_alive = self.is_worker_running();
		let live = self.live.lock().map(|g| (g.running, g.vts_connected, g.gpu_adapter.clone(), g.last_error.clone())).unwrap_or_default();
		let tel = self.telemetry.lock().map(|g| g.clone()).unwrap_or_default();
		let lo = &tel.last_output;
		BoosterStatus {
			running: worker_alive && live.0,
			vts_connected: live.1,
			gpu_adapter: live.2,
			spout_senders: list_spout_senders(),
			active_spout_sender: get_active_spout_sender(),
			last_error: live.3,
			ticks: tel.ticks,
			frames_forwarded: tel.frames_forwarded,
			generated_frames: tel.generated_frames,
			gpu_frames_processed: tel.gpu_frames_processed,
			avg_gpu_ms: tel.avg_gpu_ms,
			vts_injects: tel.vts_injects,
			vts_errors: tel.vts_errors,
			wind: [lo.wind.x, lo.wind.y],
			hair: [lo.hair.x, lo.hair.y],
			accessory: [lo.accessory.x, lo.accessory.y],
			body_bounce_y: lo.body_bounce_y,
			squash: lo.squash,
			voice_tremor: lo.voice_tremor,
			config: self.config.clone(),
		}
	}

	fn is_worker_running(&self) -> bool {
		self.worker.as_ref().is_some_and(|h| !h.is_finished())
	}
}

impl Drop for BoosterController {
	fn drop(&mut self) {
		let _ = self.stop();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::transport::{FrameBuffer, FrameSink, MemoryFrameBus, PixelFormat};

	#[tokio::test]
	async fn pipeline_steps_physics_and_forwards_frames() {
		let mut src = MemoryFrameBus::new("in");
		let dst = MemoryFrameBus::new("out");
		let sample = FrameBuffer::new(2, 2, PixelFormat::Rgba8, vec![255u8; 16]).unwrap();
		src.send_frame(&sample).unwrap();

		let cfg = BoosterConfig {
			tick_hz: 240.0,
			enable_vts: false,
			enable_gpu: false,
			..Default::default()
		};
		let mut pipeline = BoosterPipeline::new(
			cfg,
			Some(Box::new(src)),
			Some(Box::new(dst.clone())),
		);
		let stop = Arc::new(AtomicBool::new(false));
		let telemetry = pipeline.run(stop, Some(4)).await;
		assert_eq!(telemetry.ticks, 4);
		assert_eq!(telemetry.frames_forwarded, 4);
		assert!(telemetry.last_output.wind.is_finite());

		let mut dst_reader = dst;
		assert_eq!(dst_reader.recv_frame().unwrap(), Some(sample));
	}

	#[test]
	fn booster_controller_starts_updates_telemetry_and_stops_cleanly() {
		let mut ctrl = BoosterController::default();
		let cfg = BoosterControllerConfig {
			enable_vts: false,
			enable_gpu: false,
			spout_in: None,
			tick_hz: 120.0,
			..Default::default()
		};
		let started = ctrl.start(Some(cfg)).expect("start booster controller");
		assert!(started.running);

		let deadline = Instant::now() + Duration::from_millis(500);
		let mut latest = ctrl.status();
		while latest.ticks < 2 && Instant::now() < deadline {
			std::thread::sleep(Duration::from_millis(10));
			latest = ctrl.status();
		}
		assert!(latest.running, "expected controller to stay running");
		assert!(latest.ticks >= 2, "expected telemetry ticks >= 2, got {}", latest.ticks);

		let stopped = ctrl.stop();
		assert!(!stopped.running, "expected controller stopped");
	}

	#[tokio::test]
	async fn pipeline_with_live_gpu_processes_frames_and_exposes_telemetry() {
		let mut src = MemoryFrameBus::new("gpu-in");
		let dst = MemoryFrameBus::new("gpu-out");
		let w = 64u32;
		let h = 64u32;
		let mut pixels = vec![0u8; (w * h * 4) as usize];
		for y in 16..48 {
			for x in 16..48 {
				let i = ((y * w + x) as usize) * 4;
				pixels[i] = 180;
				pixels[i + 1] = 210;
				pixels[i + 2] = 255;
				pixels[i + 3] = 255;
			}
		}
		let sample = FrameBuffer::new(w, h, PixelFormat::Rgba8, pixels).unwrap();
		src.send_frame(&sample).unwrap();

		let cfg = BoosterConfig {
			tick_hz: 120.0,
			enable_vts: false,
			enable_gpu: true,
			..Default::default()
		};
		let mut pipeline = BoosterPipeline::new(cfg, Some(Box::new(src)), Some(Box::new(dst.clone())));
		let gpu_ready = pipeline.init_gpu().await.is_ok();
		let stop = Arc::new(AtomicBool::new(false));
		let shared = Arc::new(Mutex::new(BoosterTelemetry::default()));
		let telemetry = pipeline.run_with_telemetry(stop, Some(3), Some(shared.clone())).await;

		assert_eq!(telemetry.ticks, 3);
		assert_eq!(telemetry.frames_forwarded, 3);
		if gpu_ready {
			assert_eq!(telemetry.gpu_frames_processed, 3);
			assert!(telemetry.avg_gpu_ms >= 0.0);
		}
		let snap = shared.lock().unwrap().clone();
		assert_eq!(snap.ticks, 3);
		assert!(dst.clone().recv_frame().unwrap().is_some());
	}
}

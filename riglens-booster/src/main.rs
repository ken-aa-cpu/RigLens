//! CLI entrypoint for `riglens-booster`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use glam::vec2;

use riglens_booster::gpu::ambient::AmbientTheme;
use riglens_booster::gpu::{GpuEffectsConfig, GpuFrameProcessor};
use riglens_booster::physics::{BoosterPhysicsConfig, MotionHint, WindField};
use riglens_booster::pipeline::{BoosterConfig, BoosterPipeline};
use riglens_booster::protocol::vts::{VtsClient, VtsClientConfig};
use riglens_booster::transport::spout2::{
	get_active_spout_sender, list_spout_senders, Spout2Receiver, Spout2Sender,
};
use riglens_booster::transport::{FrameBuffer, FrameSink, FrameSource, PixelFormat};

#[derive(Parser)]
#[command(name = "riglens-booster")]
#[command(about = "Dual-channel GPU co-processor & 120Hz physics booster for RigLens and VTube Studio")]
struct Cli {
	#[command(subcommand)]
	command: Commands,
}

#[derive(Subcommand)]
enum Commands {
	/// List active Windows Spout2 shared-texture senders visible to OBS/VTS
	ListSpout,
	/// Connect to VTube Studio Port 8001, authenticate, inspect model/params, optionally register RigLens* params
	VtsProbe {
		#[arg(long, default_value = "ws://127.0.0.1:8001")]
		url: String,
		#[arg(long)]
		token_file: Option<PathBuf>,
		#[arg(long, default_value_t = false)]
		register_params: bool,
	},
	/// Stream a transparent animated RGBA test pattern over Windows Spout2 to verify OBS capture
	SpoutTest {
		#[arg(long, default_value = "RigLensTest")]
		sender: String,
		#[arg(long, default_value_t = 512)]
		width: u32,
		#[arg(long, default_value_t = 512)]
		height: u32,
		#[arg(long, default_value_t = 300)]
		frames: u32,
		#[arg(long, default_value_t = 60.0)]
		fps: f32,
		/// Run frames through the M2/M3 GPU compute pipeline (XPBD + JFA 2.5D rim light + particles + 2D DLSS)
		#[arg(long, default_value_t = false)]
		gpu_fx: bool,
		#[arg(long, default_value = "cyberpunk")]
		rim_theme: String,
		#[arg(long, default_value_t = 384)]
		particles: u32,
		/// M3 Alpha-aware anime super-resolution factor (1.0 - 2.0)
		#[arg(long, default_value_t = 1.0)]
		upscale: f32,
		/// M3 Alpha-aware line-art sharpness (0.0 - 2.0)
		#[arg(long, default_value_t = 0.65)]
		sharpness: f32,
		/// M3 Motion-vector frame generation multiplier (1 = off, 2 = 2x FPS)
		#[arg(long, default_value_t = 1)]
		frame_gen: u32,
	},
	/// Benchmark the M2/M3 wgpu compute/render pipeline (XPBD, JFA, Particles, 2D DLSS) on live GPU hardware
	GpuBench {
		#[arg(long, default_value_t = 1280)]
		width: u32,
		#[arg(long, default_value_t = 720)]
		height: u32,
		#[arg(long, default_value_t = 30)]
		frames: u32,
		#[arg(long, default_value_t = 512)]
		particles: u32,
		/// M3 Alpha-aware anime super-resolution factor (1.0 - 2.0)
		#[arg(long, default_value_t = 1.0)]
		upscale: f32,
		/// M3 Alpha-aware line-art sharpness (0.0 - 2.0)
		#[arg(long, default_value_t = 0.65)]
		sharpness: f32,
		/// M3 Motion-vector frame generation multiplier (1 = off, 2 = 2x FPS)
		#[arg(long, default_value_t = 1)]
		frame_gen: u32,
	},
	/// Run the live 120Hz dual-channel physics + VTS + M2/M3 GPU FX + Spout2 bridge
	Run {
		#[arg(long, default_value = "ws://127.0.0.1:8001")]
		vts_url: String,
		#[arg(long, default_value_t = false)]
		no_vts: bool,
		#[arg(long)]
		token_file: Option<PathBuf>,
		#[arg(long)]
		spout_in: Option<String>,
		#[arg(long, default_value = "RigLensBooster")]
		spout_out: String,
		#[arg(long, default_value_t = 120.0)]
		hz: f32,
		#[arg(long, default_value_t = 0.55)]
		wind: f32,
		/// Disable M2/M3 GPU effects and pass Spout2 frames straight through
		#[arg(long, default_value_t = false)]
		no_gpu: bool,
		/// Disable XPBD soft-body warp pass while keeping rim light / particles
		#[arg(long, default_value_t = false)]
		no_soft_body: bool,
		#[arg(long, default_value_t = 0.75)]
		rim_intensity: f32,
		/// Ambient theme: cyberpunk, warm, ocean, or screen (live Windows desktop edge sync)
		#[arg(long, default_value = "cyberpunk")]
		rim_theme: String,
		#[arg(long, default_value_t = 384)]
		particles: u32,
		/// M3 Alpha-aware anime super-resolution factor (1.0 - 2.0)
		#[arg(long, default_value_t = 1.0)]
		upscale: f32,
		/// M3 Alpha-aware line-art sharpness (0.0 - 2.0)
		#[arg(long, default_value_t = 0.65)]
		sharpness: f32,
		/// M3 Motion-vector frame generation multiplier (1 = off, 2 = 2x FPS)
		#[arg(long, default_value_t = 1)]
		frame_gen: u32,
	},
}

fn paint_sphere_rgba(rgba: &mut [u8], width: u32, height: u32, ox: f32, oy: f32, rad: f32) {
	for y in 0..height {
		for x in 0..width {
			let dx = x as f32 - ox;
			let dy = y as f32 - oy;
			let dist = (dx * dx + dy * dy).sqrt();
			let idx = ((y * width + x) * 4) as usize;
			if dist <= rad {
				let band = if ((x / 16 + y / 16) % 2) == 0 { 1.0 } else { 0.75 };
				rgba[idx] = (72.0 * band) as u8;
				rgba[idx + 1] = (140.0 * band) as u8;
				rgba[idx + 2] = (210.0 * band) as u8;
				rgba[idx + 3] = 240;
			} else {
				rgba[idx] = 0;
				rgba[idx + 1] = 0;
				rgba[idx + 2] = 0;
				rgba[idx + 3] = 0;
			}
		}
	}
}

#[tokio::main]
async fn main() {
	let cli = Cli::parse();
	match cli.command {
		Commands::ListSpout => {
			let senders = list_spout_senders();
			let active = get_active_spout_sender();
			println!(
				"{}",
				serde_json::to_string_pretty(&serde_json::json!({
					"active_sender": active,
					"senders": senders,
				}))
				.unwrap()
			);
		}
		Commands::VtsProbe {
			url,
			token_file,
			register_params,
		} => {
			let cfg = VtsClientConfig {
				url: url.clone(),
				token_path: token_file,
				..Default::default()
			};
			let mut client = match VtsClient::connect(cfg).await {
				Ok(c) => c,
				Err(e) => {
					eprintln!("Failed to connect to VTube Studio at {url}: {e}");
					std::process::exit(1);
				}
			};
			if let Err(e) = client.authenticate().await {
				eprintln!("Authentication failed: {e}");
				std::process::exit(1);
			}
			let registered = if register_params {
				match client.register_booster_parameters().await {
					Ok(names) => Some(names),
					Err(e) => {
						eprintln!("Parameter registration failed: {e}");
						std::process::exit(1);
					}
				}
			} else {
				None
			};
			let model = client.get_current_model().await.unwrap_or_default();
			let params = client.get_input_parameters().await.unwrap_or_default();
			println!(
				"{}",
				serde_json::to_string_pretty(&serde_json::json!({
					"connected": true,
					"url": url,
					"model_loaded": model.model_loaded,
					"model_name": model.model_name,
					"model_id": model.model_id,
					"model_position": model.model_position,
					"default_param_count": params.default_parameters.len(),
					"custom_param_count": params.custom_parameters.len(),
					"registered_riglens_params": registered,
				}))
				.unwrap()
			);
			let _ = client.close().await;
		}
		Commands::GpuBench {
			width,
			height,
			frames,
			particles,
			upscale,
			sharpness,
			frame_gen,
		} => {
			let cfg = GpuEffectsConfig {
				particle_count: particles,
				upscale_factor: upscale,
				upscale_sharpness: sharpness,
				frame_gen_multiplier: frame_gen,
				..Default::default()
			};
			let mut gpu = match GpuFrameProcessor::new_async(cfg).await {
				Ok(g) => g,
				Err(e) => {
					eprintln!("Failed to init wgpu GPU processor: {e}");
					std::process::exit(1);
				}
			};
			let (out_width, out_height) = gpu.output_dimensions(width, height);
			let mut rgba = vec![0u8; (width * height * 4) as usize];
			paint_sphere_rgba(
				&mut rgba,
				width,
				height,
				width as f32 * 0.5,
				height as f32 * 0.5,
				width.min(height) as f32 * 0.35,
			);
			let fb = FrameBuffer::new(width, height, PixelFormat::Rgba8, rgba).unwrap();
			let hint = MotionHint {
				face_found: true,
				head_velocity: vec2(8.0, -3.0),
				overall_motion: 1.0,
				wind_vec: vec2(0.7, 0.2),
				..Default::default()
			};

			// Warmup passes (compiles target cache, dynamic JFA uniform slots & prev frame)
			for _ in 0..2 {
				gpu.process_frame_with_gen(&fb, &hint, 1.0 / 60.0, None).unwrap();
			}

			let n = frames.max(1);
			let mut min_ms = f32::INFINITY;
			let mut max_ms = 0.0f32;
			let mut sum_ms = 0.0f32;
			let mut total_out_frames = 0u64;
			for i in 0..n {
				let moving_hint = MotionHint {
					face_found: true,
					head_velocity: vec2((i as f32 * 0.4).sin() * 12.0, -3.0),
					overall_motion: 1.0,
					wind_vec: vec2(0.7, 0.2),
					..Default::default()
				};
				let t0 = Instant::now();
				let produced = gpu
					.process_frame_with_gen(&fb, &moving_hint, 1.0 / 60.0, None)
					.unwrap();
				let ms = t0.elapsed().as_secs_f32() * 1000.0;
				total_out_frames += produced.len() as u64;
				min_ms = min_ms.min(ms);
				max_ms = max_ms.max(ms);
				sum_ms += ms;
			}
			let avg_ms = sum_ms / n as f32;
			println!(
				"{}",
				serde_json::to_string_pretty(&serde_json::json!({
					"adapter": gpu.adapter_info(),
					"width": width,
					"height": height,
					"out_width": out_width,
					"out_height": out_height,
					"upscale": upscale,
					"sharpness": sharpness,
					"frame_gen": frame_gen,
					"frames": n,
					"total_out_frames": total_out_frames,
					"particles": particles,
					"avg_ms": avg_ms,
					"min_ms": min_ms,
					"max_ms": max_ms,
					"effective_input_fps": 1000.0 / avg_ms.max(0.001),
					"effective_output_fps": (total_out_frames as f32 * 1000.0) / sum_ms.max(0.001),
				}))
				.unwrap()
			);
		}
		Commands::SpoutTest {
			sender,
			width,
			height,
			frames,
			fps,
			gpu_fx,
			rim_theme,
			particles,
			upscale,
			sharpness,
			frame_gen,
		} => {
			let mut sink = match Spout2Sender::new(&sender) {
				Ok(s) => s,
				Err(e) => {
					eprintln!("Failed to initialize Spout2 sender '{sender}': {e}");
					std::process::exit(1);
				}
			};
			let use_gpu = gpu_fx || upscale > 1.0 || frame_gen > 1;
			let mut gpu = if use_gpu {
				let cfg = GpuEffectsConfig {
					ambient_theme: AmbientTheme::parse(&rim_theme).unwrap_or(AmbientTheme::Cyberpunk),
					particle_count: particles,
					upscale_factor: upscale,
					upscale_sharpness: sharpness,
					frame_gen_multiplier: frame_gen,
					..Default::default()
				};
				match GpuFrameProcessor::new_async(cfg).await {
					Ok(g) => {
						println!("M2/M3 GPU FX enabled on '{}'", g.adapter_info());
						Some(g)
					}
					Err(e) => {
						eprintln!("Failed to init M2/M3 GPU FX (falling back to raw): {e}");
						None
					}
				}
			} else {
				None
			};

			println!(
				"Streaming {width}x{height} transparent RGBA test over Spout2 sender '{sender}' for {frames} frames (gpu_fx={use_gpu}, upscale={upscale}x, frame_gen={frame_gen}x)..."
			);
			let dt = 1.0 / fps.clamp(1.0, 240.0);
			let period = Duration::from_secs_f32(dt);
			let start = Instant::now();
			let mut rgba = vec![0u8; (width * height * 4) as usize];
			let cx = width as f32 * 0.5;
			let cy = height as f32 * 0.5;
			let rad = (width.min(height) as f32) * 0.36;

			for f in 0..frames {
				let t = f as f32 * 0.06;
				let ox = cx + t.cos() * (rad * 0.18);
				let oy = cy + (t * 1.3).sin() * (rad * 0.18);
				paint_sphere_rgba(&mut rgba, width, height, ox, oy, rad);

				let fb = FrameBuffer::new(width, height, PixelFormat::Rgba8, rgba.clone()).unwrap();
				let out_frames = if let Some(ref mut g) = gpu {
					let hint = MotionHint {
						face_found: true,
						head_velocity: vec2(-t.sin() * 14.0, (t * 1.3).cos() * 18.0),
						overall_motion: 1.1,
						wind_vec: vec2(0.8, (t * 0.5).sin() * 0.3),
						..Default::default()
					};
					g.process_frame_with_gen(&fb, &hint, dt, None)
						.unwrap_or_else(|_| vec![fb])
				} else {
					vec![fb]
				};
				for out_fb in out_frames {
					if let Err(e) = sink.send_frame(&out_fb) {
						eprintln!("Spout send error at frame {f}: {e}");
						break;
					}
				}
				let target = start + period * (f + 1);
				let now = Instant::now();
				if target > now {
					tokio::time::sleep(target - now).await;
				}
			}
			println!("Finished Spout2 test stream '{sender}'.");
		}
		Commands::Run {
			vts_url,
			no_vts,
			token_file,
			spout_in,
			spout_out,
			hz,
			wind,
			no_gpu,
			no_soft_body,
			rim_intensity,
			rim_theme,
			particles,
			upscale,
			sharpness,
			frame_gen,
		} => {
			if let Some(ref in_name) = spout_in {
				let senders = list_spout_senders();
				if !senders.iter().any(|s| s == in_name) {
					eprintln!(
						"[Spout2] Sender '{in_name}' is not active right now (currently visible: {senders:?}).\n\
						 -> In VTube Studio: load a model and enable Camera -> Spout2 Output (creates '{in_name}').\n\
						 -> In OBS: add a 'Spout2 Capture' source selecting '{spout_out}'."
					);
				}
			}
			let source: Option<Box<dyn FrameSource>> = match spout_in {
				Some(name) => match Spout2Receiver::new(&name) {
					Ok(rx) => Some(Box::new(rx)),
					Err(e) => {
						eprintln!("Failed to open Spout2 receiver '{name}': {e}");
						None
					}
				},
				None => None,
			};
			let sink: Option<Box<dyn FrameSink>> = if source.is_some() {
				match Spout2Sender::new(&spout_out) {
					Ok(tx) => Some(Box::new(tx)),
					Err(e) => {
						eprintln!("Failed to open Spout2 sender '{spout_out}': {e}");
						None
					}
				}
			} else {
				None
			};

			let gpu_cfg = GpuEffectsConfig {
				enable_soft_body: !no_soft_body,
				enable_rim_light: rim_intensity > 0.0,
				rim_intensity,
				ambient_theme: AmbientTheme::parse(&rim_theme).unwrap_or(AmbientTheme::Cyberpunk),
				particle_count: particles,
				upscale_factor: upscale,
				upscale_sharpness: sharpness,
				frame_gen_multiplier: frame_gen,
				..Default::default()
			};

			let cfg = BoosterConfig {
				tick_hz: hz,
				enable_vts: !no_vts,
				enable_gpu: !no_gpu && source.is_some(),
				vts: VtsClientConfig {
					url: vts_url,
					token_path: token_file,
					..Default::default()
				},
				physics: BoosterPhysicsConfig {
					wind: WindField {
						turbulence: wind,
						..Default::default()
					},
					..Default::default()
				},
				gpu: gpu_cfg,
				..Default::default()
			};

			let mut pipeline = BoosterPipeline::new(cfg, source, sink);
			let gpu_label = match pipeline.init_gpu().await {
				Ok(Some(info)) => info,
				Ok(None) => "disabled".to_owned(),
				Err(e) => {
					eprintln!("M2 GPU init failed (falling back to passthrough): {e}");
					"fallback".to_owned()
				}
			};
			let vts_ok = pipeline.init_vts().await;
			println!(
				"RigLens Booster running at {hz}Hz (VTS: {vts_ok}, GPU: {gpu_label}, upscale: {upscale}x, frame_gen: {frame_gen}x, Spout out: '{spout_out}'). Press Ctrl+C to stop."
			);

			let stop = Arc::new(AtomicBool::new(false));
			let stop_sig = stop.clone();
			tokio::spawn(async move {
				let _ = tokio::signal::ctrl_c().await;
				stop_sig.store(true, Ordering::Relaxed);
			});

			let telemetry = pipeline.run(stop, None).await;
			println!(
				"Booster stopped cleanly: ticks={}, vts_injects={}, vts_errors={}, frames_forwarded={}, generated_frames={}, gpu_frames={}, avg_gpu_ms={:.3}",
				telemetry.ticks,
				telemetry.vts_injects,
				telemetry.vts_errors,
				telemetry.frames_forwarded,
				telemetry.generated_frames,
				telemetry.gpu_frames_processed,
				telemetry.avg_gpu_ms
			);
		}
	}
}

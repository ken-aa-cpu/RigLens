//! CLI entrypoint for `riglens-booster`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};

use riglens_booster::physics::{BoosterPhysicsConfig, WindField};
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
	},
	/// Run the live 120Hz dual-channel physics + VTS + Spout2 bridge
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
	},
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
		Commands::SpoutTest {
			sender,
			width,
			height,
			frames,
			fps,
		} => {
			let mut sink = match Spout2Sender::new(&sender) {
				Ok(s) => s,
				Err(e) => {
					eprintln!("Failed to initialize Spout2 sender '{sender}': {e}");
					std::process::exit(1);
				}
			};
			println!("Streaming {width}x{height} transparent RGBA test over Spout2 sender '{sender}' for {frames} frames...");
			let period = Duration::from_secs_f32(1.0 / fps.clamp(1.0, 240.0));
			let start = Instant::now();
			let mut rgba = vec![0u8; (width * height * 4) as usize];
			let cx = width as f32 * 0.5;
			let cy = height as f32 * 0.5;
			let rad = (width.min(height) as f32) * 0.36;

			for f in 0..frames {
				let t = f as f32 * 0.06;
				let ox = cx + t.cos() * (rad * 0.18);
				let oy = cy + (t * 1.3).sin() * (rad * 0.18);
				for y in 0..height {
					for x in 0..width {
						let dx = x as f32 - ox;
						let dy = y as f32 - oy;
						let dist = (dx * dx + dy * dy).sqrt();
						let idx = ((y * width + x) * 4) as usize;
						if dist <= rad {
							let rim = ((dist / rad).powi(3) * 255.0) as u8;
							rgba[idx] = 80u8.saturating_add(rim / 2);
							rgba[idx + 1] = 180;
							rgba[idx + 2] = 255;
							rgba[idx + 3] = 235;
						} else {
							rgba[idx] = 0;
							rgba[idx + 1] = 0;
							rgba[idx + 2] = 0;
							rgba[idx + 3] = 0;
						}
					}
				}
				let fb = FrameBuffer::new(width, height, PixelFormat::Rgba8, rgba.clone()).unwrap();
				if let Err(e) = sink.send_frame(&fb) {
					eprintln!("Spout send error at frame {f}: {e}");
					break;
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
		} => {
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

			let cfg = BoosterConfig {
				tick_hz: hz,
				enable_vts: !no_vts,
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
				..Default::default()
			};

			let mut pipeline = BoosterPipeline::new(cfg, source, sink);
			let vts_ok = pipeline.init_vts().await;
			println!(
				"RigLens Booster running at {hz}Hz (VTS connected: {vts_ok}, Spout out: '{spout_out}'). Press Ctrl+C to stop."
			);

			let stop = Arc::new(AtomicBool::new(false));
			let stop_sig = stop.clone();
			tokio::spawn(async move {
				let _ = tokio::signal::ctrl_c().await;
				stop_sig.store(true, Ordering::Relaxed);
			});

			let telemetry = pipeline.run(stop, None).await;
			println!(
				"Booster stopped cleanly: ticks={}, vts_injects={}, vts_errors={}, frames_forwarded={}",
				telemetry.ticks, telemetry.vts_injects, telemetry.vts_errors, telemetry.frames_forwarded
			);
		}
	}
}

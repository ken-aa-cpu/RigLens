//! Dual-channel orchestrator: 120Hz physics + VTS Port 8001 + Spout2 frame forwarding.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::physics::{BoosterPhysicsConfig, BoosterPhysicsEngine, BoosterPhysicsOutput, MotionPoseInput};
use crate::protocol::vts::{VtsClient, VtsClientConfig};
use crate::transport::{FrameSink, FrameSource};

#[derive(Clone, Debug)]
pub struct BoosterConfig {
	pub tick_hz: f32,
	pub vts_poll_hz: f32,
	pub enable_vts: bool,
	pub vts: VtsClientConfig,
	pub physics: BoosterPhysicsConfig,
}

impl Default for BoosterConfig {
	fn default() -> Self {
		Self {
			tick_hz: 120.0,
			vts_poll_hz: 60.0,
			enable_vts: true,
			vts: VtsClientConfig::default(),
			physics: BoosterPhysicsConfig::default(),
		}
	}
}

#[derive(Clone, Debug, Default)]
pub struct BoosterTelemetry {
	pub ticks: u64,
	pub frames_forwarded: u64,
	pub vts_injects: u64,
	pub vts_errors: u64,
	pub last_output: BoosterPhysicsOutput,
}

/// Coordinates Port 8001 bidirectional VTS tracking/physics with Spout2 video transport.
pub struct BoosterPipeline {
	config: BoosterConfig,
	physics: BoosterPhysicsEngine,
	vts: Option<VtsClient>,
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
			source,
			sink,
		}
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

	/// Run the live dual-channel loop until `stop` becomes true (or `max_ticks` if provided).
	pub async fn run(
		&mut self,
		stop: Arc<AtomicBool>,
		max_ticks: Option<u64>,
	) -> BoosterTelemetry {
		let tick_hz = self.config.tick_hz.clamp(15.0, 240.0);
		let poll_every_n = ((tick_hz / self.config.vts_poll_hz.clamp(1.0, tick_hz)).round() as u64).max(1);
		let tick_period = Duration::from_secs_f32(1.0 / tick_hz);
		let mut interval = tokio::time::interval(tick_period);
		interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

		let mut telemetry = BoosterTelemetry::default();
		let mut last_pose = MotionPoseInput::default();
		let mut prev_t = Instant::now();

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
					if dst.send_frame(&frame).is_ok() {
						telemetry.frames_forwarded += 1;
					}
				}
			}

			telemetry.last_output = phys_out;
			telemetry.ticks += 1;
		}

		telemetry
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
}

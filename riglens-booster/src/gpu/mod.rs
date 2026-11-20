//! `wgpu` Compute + Render pipeline for M2/M3 visual, soft-body & 2D-DLSS effects:
//! 1) Screen-space XPBD soft-body warp (premultiplied-alpha bilinear)
//! 2) Jump Flooding Algorithm (JFA) 2D SDF + surface normals
//! 3) Dynamic directional 2.5D rim lighting (strict alpha preservation)
//! 4) Motion/SDF-driven GPU particle simulation & instanced glow render
//! 5) M3 2D-Native DLSS: Alpha-aware super-resolution + Dual-tier motion-vector frame generation

pub mod ambient;
pub mod mesh_velocity;
pub mod soft_body;

mod passes;
mod resources;
mod targets;
#[cfg(test)]
mod tests;

use std::num::NonZeroU64;

use thiserror::Error;

use crate::transport::TransportError;

use self::ambient::{AmbientColorSampler, AmbientTheme};
use self::resources::{
	bgl_storage_buf, bgl_storage_tex, bgl_tex, bgl_uniform, create_compute_pipe, create_uniform_buf, FlowUniforms,
	FrameGenUniforms, GpuParticle, JfaUniforms, ParticleRenderUniforms, ParticleSimUniforms, RimUniforms, TargetCache,
	UpscaleUniforms, WarpUniforms, JFA_MAX_POW2, JFA_STEP_STRIDE,
};
use self::soft_body::{
	default_vtuber_zones, SoftBodyZoneConfig, SoftBodyZoneGpu, XpbdSoftBodySystem, MAX_SOFT_BODY_ZONES,
};

pub const MAX_GPU_PARTICLES: u32 = 2048;

#[derive(Debug, Error)]
pub enum GpuError {
	#[error("No compatible GPU adapter found")]
	NoAdapter,
	#[error("Failed to request GPU device: {0}")]
	DeviceRequest(#[from] wgpu::RequestDeviceError),
	#[error("GPU buffer map error: {0}")]
	BufferMap(String),
	#[error("Transport/frame error: {0}")]
	Transport(#[from] TransportError),
}

#[derive(Clone, Debug)]
pub struct GpuEffectsConfig {
	pub enable_soft_body: bool,
	pub soft_body_zones: Vec<SoftBodyZoneConfig>,
	pub enable_rim_light: bool,
	pub rim_width_px: f32,
	pub rim_intensity: f32,
	pub ambient_theme: AmbientTheme,
	pub particle_count: u32,
	pub particle_spawn_rate: f32,
	pub upscale_factor: f32,
	pub upscale_sharpness: f32,
	pub frame_gen_multiplier: u32,
}

impl Default for GpuEffectsConfig {
	fn default() -> Self {
		Self {
			enable_soft_body: true,
			soft_body_zones: default_vtuber_zones(),
			enable_rim_light: true,
			rim_width_px: 10.0,
			rim_intensity: 0.75,
			ambient_theme: AmbientTheme::Cyberpunk,
			particle_count: 384,
			particle_spawn_rate: 0.65,
			upscale_factor: 1.0,
			upscale_sharpness: 0.65,
			frame_gen_multiplier: 1,
		}
	}
}

/// Real-time `wgpu` compute/render processor executing M2 effects and M3 2D-Native DLSS.
pub struct GpuFrameProcessor {
	pub config: GpuEffectsConfig,
	pub soft_body: XpbdSoftBodySystem,
	pub ambient: AmbientColorSampler,
	adapter_label: String,
	pub(in crate::gpu) device: wgpu::Device,
	pub(in crate::gpu) queue: wgpu::Queue,
	pub(in crate::gpu) time_s: f32,

	pub(in crate::gpu) warp_pipe: wgpu::ComputePipeline,
	pub(in crate::gpu) jfa_seed_pipe: wgpu::ComputePipeline,
	pub(in crate::gpu) jfa_step_pipe: wgpu::ComputePipeline,
	pub(in crate::gpu) rim_pipe: wgpu::ComputePipeline,
	pub(in crate::gpu) particle_sim_pipe: wgpu::ComputePipeline,
	pub(in crate::gpu) particle_render_pipe: wgpu::RenderPipeline,
	pub(in crate::gpu) flow_pipe: wgpu::ComputePipeline,
	pub(in crate::gpu) frame_gen_pipe: wgpu::ComputePipeline,
	pub(in crate::gpu) upscale_pipe: wgpu::ComputePipeline,

	pub(in crate::gpu) warp_bgl: wgpu::BindGroupLayout,
	pub(in crate::gpu) jfa_seed_bgl: wgpu::BindGroupLayout,
	pub(in crate::gpu) jfa_step_bgl: wgpu::BindGroupLayout,
	pub(in crate::gpu) rim_bgl: wgpu::BindGroupLayout,
	pub(in crate::gpu) particle_sim_bgl: wgpu::BindGroupLayout,
	pub(in crate::gpu) particle_render_bg: wgpu::BindGroup,
	pub(in crate::gpu) flow_bgl: wgpu::BindGroupLayout,
	pub(in crate::gpu) frame_gen_bgl: wgpu::BindGroupLayout,
	pub(in crate::gpu) upscale_bgl: wgpu::BindGroupLayout,

	pub(in crate::gpu) warp_uniform_buf: wgpu::Buffer,
	pub(in crate::gpu) warp_zones_buf: wgpu::Buffer,
	pub(in crate::gpu) jfa_step_buf: wgpu::Buffer,
	pub(in crate::gpu) rim_uniform_buf: wgpu::Buffer,
	pub(in crate::gpu) particle_sim_uniform_buf: wgpu::Buffer,
	pub(in crate::gpu) particle_render_uniform_buf: wgpu::Buffer,
	pub(in crate::gpu) particles_buf: wgpu::Buffer,
	pub(in crate::gpu) flow_uniform_buf: wgpu::Buffer,
	pub(in crate::gpu) frame_gen_uniform_buf: wgpu::Buffer,
	pub(in crate::gpu) upscale_uniform_buf: wgpu::Buffer,

	pub(in crate::gpu) targets: Option<TargetCache>,
}

impl GpuFrameProcessor {
	/// Create a GPU frame processor synchronously (blocks on adapter/device selection).
	pub fn new_blocking(config: GpuEffectsConfig) -> Result<Self, GpuError> {
		let rt = tokio::runtime::Builder::new_current_thread()
			.enable_all()
			.build()
			.map_err(|e| GpuError::BufferMap(format!("runtime init: {e}")))?;
		rt.block_on(Self::new_async(config))
	}

	/// Initialize `wgpu` compute & render pipelines using the highest-performance GPU adapter.
	pub async fn new_async(config: GpuEffectsConfig) -> Result<Self, GpuError> {
		let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
			backends: wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
			..Default::default()
		});

		let adapter = match instance
			.request_adapter(&wgpu::RequestAdapterOptions {
				power_preference: wgpu::PowerPreference::HighPerformance,
				compatible_surface: None,
				force_fallback_adapter: false,
			})
			.await
		{
			Some(a) => a,
			None => instance
				.request_adapter(&wgpu::RequestAdapterOptions::default())
				.await
				.ok_or(GpuError::NoAdapter)?,
		};

		let info = adapter.get_info();
		let adapter_label = format!("{} ({:?})", info.name, info.backend);

		let (device, queue) = adapter
			.request_device(
				&wgpu::DeviceDescriptor {
					label: Some("RigLensBoosterGpu"),
					required_features: wgpu::Features::empty(),
					required_limits: wgpu::Limits::default(),
					memory_hints: wgpu::MemoryHints::Performance,
				},
				None,
			)
			.await?;

		let shader_warp = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("soft_body_warp"),
			source: wgpu::ShaderSource::Wgsl(include_str!("shaders/soft_body_warp.wgsl").into()),
		});
		let shader_jfa_seed = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("jfa_seed"),
			source: wgpu::ShaderSource::Wgsl(include_str!("shaders/jfa_seed.wgsl").into()),
		});
		let shader_jfa_step = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("jfa_step"),
			source: wgpu::ShaderSource::Wgsl(include_str!("shaders/jfa_step.wgsl").into()),
		});
		let shader_rim = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("sdf_rim_light"),
			source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sdf_rim_light.wgsl").into()),
		});
		let shader_p_sim = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("particles_sim"),
			source: wgpu::ShaderSource::Wgsl(include_str!("shaders/particles_sim.wgsl").into()),
		});
		let shader_p_render = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("particles_render"),
			source: wgpu::ShaderSource::Wgsl(include_str!("shaders/particles_render.wgsl").into()),
		});
		let shader_flow = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("flow_estimate"),
			source: wgpu::ShaderSource::Wgsl(include_str!("shaders/flow_estimate.wgsl").into()),
		});
		let shader_frame_gen = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("frame_gen_warp"),
			source: wgpu::ShaderSource::Wgsl(include_str!("shaders/frame_gen_warp.wgsl").into()),
		});
		let shader_upscale = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("upscale_alpha_aware"),
			source: wgpu::ShaderSource::Wgsl(include_str!("shaders/upscale_alpha_aware.wgsl").into()),
		});

		let warp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("warp_bgl"),
			entries: &[
				bgl_tex(0, true),
				bgl_storage_tex(1, wgpu::TextureFormat::Rgba8Unorm),
				bgl_uniform(2, false, None),
				bgl_storage_buf(3, true),
			],
		});

		let jfa_seed_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("jfa_seed_bgl"),
			entries: &[
				bgl_tex(0, true),
				bgl_storage_tex(1, wgpu::TextureFormat::Rgba32Float),
				bgl_uniform(2, false, None),
			],
		});

		let jfa_step_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("jfa_step_bgl"),
			entries: &[
				bgl_tex(0, false),
				bgl_storage_tex(1, wgpu::TextureFormat::Rgba32Float),
				bgl_uniform(2, true, NonZeroU64::new(std::mem::size_of::<JfaUniforms>() as u64)),
			],
		});

		let rim_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("rim_bgl"),
			entries: &[
				bgl_tex(0, true),
				bgl_tex(1, false),
				bgl_storage_tex(2, wgpu::TextureFormat::Rgba8Unorm),
				bgl_storage_tex(3, wgpu::TextureFormat::Rgba16Float),
				bgl_uniform(4, false, None),
			],
		});

		let particle_sim_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("particle_sim_bgl"),
			entries: &[
				bgl_tex(0, true),
				bgl_storage_buf(1, false),
				bgl_uniform(2, false, None),
			],
		});

		let particle_render_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("particle_render_bgl"),
			entries: &[
				bgl_storage_buf(0, true),
				bgl_uniform(1, false, None),
			],
		});

		let flow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("flow_bgl"),
			entries: &[
				bgl_tex(0, true),
				bgl_tex(1, true),
				bgl_storage_tex(2, wgpu::TextureFormat::Rgba32Float),
				bgl_uniform(3, false, None),
			],
		});

		let frame_gen_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("frame_gen_bgl"),
			entries: &[
				bgl_tex(0, true),
				bgl_tex(1, true),
				bgl_tex(2, false),
				bgl_storage_tex(3, wgpu::TextureFormat::Rgba8Unorm),
				bgl_uniform(4, false, None),
			],
		});

		let upscale_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("upscale_bgl"),
			entries: &[
				bgl_tex(0, true),
				bgl_storage_tex(1, wgpu::TextureFormat::Rgba8Unorm),
				bgl_uniform(2, false, None),
			],
		});

		let warp_pipe = create_compute_pipe(&device, "warp_pipe", &warp_bgl, &shader_warp);
		let jfa_seed_pipe = create_compute_pipe(&device, "jfa_seed_pipe", &jfa_seed_bgl, &shader_jfa_seed);
		let jfa_step_pipe = create_compute_pipe(&device, "jfa_step_pipe", &jfa_step_bgl, &shader_jfa_step);
		let rim_pipe = create_compute_pipe(&device, "rim_pipe", &rim_bgl, &shader_rim);
		let particle_sim_pipe = create_compute_pipe(&device, "particle_sim_pipe", &particle_sim_bgl, &shader_p_sim);
		let flow_pipe = create_compute_pipe(&device, "flow_pipe", &flow_bgl, &shader_flow);
		let frame_gen_pipe = create_compute_pipe(&device, "frame_gen_pipe", &frame_gen_bgl, &shader_frame_gen);
		let upscale_pipe = create_compute_pipe(&device, "upscale_pipe", &upscale_bgl, &shader_upscale);

		let render_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("particle_render_pl"),
			bind_group_layouts: &[&particle_render_bgl],
			push_constant_ranges: &[],
		});
		let particle_render_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("particle_render_pipe"),
			layout: Some(&render_pl),
			vertex: wgpu::VertexState {
				module: &shader_p_render,
				entry_point: Some("vs_main"),
				buffers: &[],
				compilation_options: wgpu::PipelineCompilationOptions::default(),
			},
			fragment: Some(wgpu::FragmentState {
				module: &shader_p_render,
				entry_point: Some("fs_main"),
				targets: &[Some(wgpu::ColorTargetState {
					format: wgpu::TextureFormat::Rgba8Unorm,
					blend: Some(wgpu::BlendState {
						color: wgpu::BlendComponent {
							src_factor: wgpu::BlendFactor::SrcAlpha,
							dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
							operation: wgpu::BlendOperation::Add,
						},
						alpha: wgpu::BlendComponent {
							src_factor: wgpu::BlendFactor::One,
							dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
							operation: wgpu::BlendOperation::Add,
						},
					}),
					write_mask: wgpu::ColorWrites::ALL,
				})],
				compilation_options: wgpu::PipelineCompilationOptions::default(),
			}),
			primitive: wgpu::PrimitiveState {
				topology: wgpu::PrimitiveTopology::TriangleStrip,
				..Default::default()
			},
			depth_stencil: None,
			multisample: wgpu::MultisampleState::default(),
			multiview: None,
			cache: None,
		});

		let warp_uniform_buf = create_uniform_buf(&device, "warp_u", std::mem::size_of::<WarpUniforms>() as u64);
		let warp_zones_buf = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("warp_zones"),
			size: (MAX_SOFT_BODY_ZONES * std::mem::size_of::<SoftBodyZoneGpu>()) as u64,
			usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});
		let jfa_step_buf = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("jfa_step_u"),
			size: JFA_STEP_STRIDE * (JFA_MAX_POW2 as u64 + 1),
			usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});
		let rim_uniform_buf = create_uniform_buf(&device, "rim_u", std::mem::size_of::<RimUniforms>() as u64);
		let particle_sim_uniform_buf =
			create_uniform_buf(&device, "psim_u", std::mem::size_of::<ParticleSimUniforms>() as u64);
		let particle_render_uniform_buf =
			create_uniform_buf(&device, "prender_u", std::mem::size_of::<ParticleRenderUniforms>() as u64);
		let flow_uniform_buf = create_uniform_buf(&device, "flow_u", std::mem::size_of::<FlowUniforms>() as u64);
		let frame_gen_uniform_buf =
			create_uniform_buf(&device, "frame_gen_u", std::mem::size_of::<FrameGenUniforms>() as u64);
		let upscale_uniform_buf =
			create_uniform_buf(&device, "upscale_u", std::mem::size_of::<UpscaleUniforms>() as u64);

		let init_particles: Vec<GpuParticle> = (0..MAX_GPU_PARTICLES)
			.map(|i| GpuParticle {
				pos_vel: [0.0; 4],
				color: [1.0, 1.0, 1.0, 0.0],
				meta: [0.0, 1.0, 4.0, (i * 73 + 17) as f32],
			})
			.collect();
		let particles_buf = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("gpu_particles"),
			size: (init_particles.len() * std::mem::size_of::<GpuParticle>()) as u64,
			usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});
		queue.write_buffer(&particles_buf, 0, bytemuck::cast_slice(&init_particles));

		let particle_render_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("particle_render_bg"),
			layout: &particle_render_bgl,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: particles_buf.as_entire_binding(),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: particle_render_uniform_buf.as_entire_binding(),
				},
			],
		});

		let soft_body = XpbdSoftBodySystem::new(config.soft_body_zones.clone());
		let ambient = AmbientColorSampler::new(config.ambient_theme);

		Ok(Self {
			config,
			soft_body,
			ambient,
			adapter_label,
			device,
			queue,
			time_s: 0.0,
			warp_pipe,
			jfa_seed_pipe,
			jfa_step_pipe,
			rim_pipe,
			particle_sim_pipe,
			particle_render_pipe,
			flow_pipe,
			frame_gen_pipe,
			upscale_pipe,
			warp_bgl,
			jfa_seed_bgl,
			jfa_step_bgl,
			rim_bgl,
			particle_sim_bgl,
			particle_render_bg,
			flow_bgl,
			frame_gen_bgl,
			upscale_bgl,
			warp_uniform_buf,
			warp_zones_buf,
			jfa_step_buf,
			rim_uniform_buf,
			particle_sim_uniform_buf,
			particle_render_uniform_buf,
			particles_buf,
			flow_uniform_buf,
			frame_gen_uniform_buf,
			upscale_uniform_buf,
			targets: None,
		})
	}

	pub fn adapter_info(&self) -> &str {
		&self.adapter_label
	}

	pub fn output_dimensions(&self, src_w: u32, src_h: u32) -> (u32, u32) {
		let scale = if self.config.upscale_factor.is_finite() {
			self.config.upscale_factor.clamp(1.0, 2.0)
		} else {
			1.0
		};
		if (scale - 1.0).abs() <= 1e-3 {
			(src_w, src_h)
		} else {
			(
				((src_w as f32) * scale).round().max(1.0) as u32,
				((src_h as f32) * scale).round().max(1.0) as u32,
			)
		}
	}
}

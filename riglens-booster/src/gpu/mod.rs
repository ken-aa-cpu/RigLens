//! `wgpu` Compute + Render pipeline for M2 visual & soft-body effects:
//! 1) Screen-space XPBD soft-body warp (premultiplied-alpha bilinear)
//! 2) Jump Flooding Algorithm (JFA) 2D SDF + surface normals
//! 3) Dynamic directional 2.5D rim lighting (strict alpha preservation)
//! 4) Motion/SDF-driven GPU particle simulation & instanced glow render

pub mod ambient;
pub mod soft_body;

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use thiserror::Error;

use crate::physics::MotionHint;
use crate::transport::{FrameBuffer, PixelFormat, TransportError};

use self::ambient::{AmbientColorSampler, AmbientTheme};
use self::soft_body::{default_vtuber_zones, SoftBodyZoneConfig, SoftBodyZoneGpu, XpbdSoftBodySystem, MAX_SOFT_BODY_ZONES};

pub const MAX_GPU_PARTICLES: u32 = 2048;
const JFA_STEP_STRIDE: u64 = 256;
const JFA_MAX_POW2: usize = 14; // up to 16384px

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
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WarpUniforms {
	width: u32,
	height: u32,
	zone_count: u32,
	enabled: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct JfaUniforms {
	width: u32,
	height: u32,
	step_px: i32,
	_pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RimUniforms {
	dims: [f32; 4],
	left_rgb: [f32; 4],
	right_rgb: [f32; 4],
	top_rgb: [f32; 4],
	bottom_rgb: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ParticleSimUniforms {
	dims_dt: [f32; 4],
	motion: [f32; 4],
	accent_spawn: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ParticleRenderUniforms {
	dims: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuParticle {
	pos_vel: [f32; 4],
	color: [f32; 4],
	meta: [f32; 4],
}

struct TargetCache {
	width: u32,
	height: u32,
	tex_in: wgpu::Texture,
	_tex_a: wgpu::Texture,
	tex_b: wgpu::Texture,
	view_b: wgpu::TextureView,
	_jfa_a: wgpu::Texture,
	_jfa_b: wgpu::Texture,
	readback_buf: wgpu::Buffer,
	padded_bytes_per_row: u32,
	warp_bg: wgpu::BindGroup,
	jfa_seed_bg: wgpu::BindGroup,
	jfa_step_a_to_b_bg: wgpu::BindGroup,
	jfa_step_b_to_a_bg: wgpu::BindGroup,
	rim_bg_a: wgpu::BindGroup,
	rim_bg_b: wgpu::BindGroup,
	particle_sim_bg: wgpu::BindGroup,
}

/// Real-time `wgpu` compute/render processor executing M2 soft-body, JFA rim lighting, and GPU particles.
pub struct GpuFrameProcessor {
	pub config: GpuEffectsConfig,
	pub soft_body: XpbdSoftBodySystem,
	pub ambient: AmbientColorSampler,
	adapter_label: String,
	device: wgpu::Device,
	queue: wgpu::Queue,
	time_s: f32,

	warp_pipe: wgpu::ComputePipeline,
	jfa_seed_pipe: wgpu::ComputePipeline,
	jfa_step_pipe: wgpu::ComputePipeline,
	rim_pipe: wgpu::ComputePipeline,
	particle_sim_pipe: wgpu::ComputePipeline,
	particle_render_pipe: wgpu::RenderPipeline,

	warp_bgl: wgpu::BindGroupLayout,
	jfa_seed_bgl: wgpu::BindGroupLayout,
	jfa_step_bgl: wgpu::BindGroupLayout,
	rim_bgl: wgpu::BindGroupLayout,
	particle_sim_bgl: wgpu::BindGroupLayout,
	particle_render_bg: wgpu::BindGroup,

	warp_uniform_buf: wgpu::Buffer,
	warp_zones_buf: wgpu::Buffer,
	jfa_step_buf: wgpu::Buffer,
	rim_uniform_buf: wgpu::Buffer,
	particle_sim_uniform_buf: wgpu::Buffer,
	particle_render_uniform_buf: wgpu::Buffer,
	particles_buf: wgpu::Buffer,

	targets: Option<TargetCache>,
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

		let warp_pipe = create_compute_pipe(&device, "warp_pipe", &warp_bgl, &shader_warp);
		let jfa_seed_pipe = create_compute_pipe(&device, "jfa_seed_pipe", &jfa_seed_bgl, &shader_jfa_seed);
		let jfa_step_pipe = create_compute_pipe(&device, "jfa_step_pipe", &jfa_step_bgl, &shader_jfa_step);
		let rim_pipe = create_compute_pipe(&device, "rim_pipe", &rim_bgl, &shader_rim);
		let particle_sim_pipe = create_compute_pipe(&device, "particle_sim_pipe", &particle_sim_bgl, &shader_p_sim);

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
			warp_bgl,
			jfa_seed_bgl,
			jfa_step_bgl,
			rim_bgl,
			particle_sim_bgl,
			particle_render_bg,
			warp_uniform_buf,
			warp_zones_buf,
			jfa_step_buf,
			rim_uniform_buf,
			particle_sim_uniform_buf,
			particle_render_uniform_buf,
			particles_buf,
			targets: None,
		})
	}

	pub fn adapter_info(&self) -> &str {
		&self.adapter_label
	}

	fn ensure_targets(&mut self, width: u32, height: u32) {
		if let Some(ref tc) = self.targets {
			if tc.width == width && tc.height == height {
				return;
			}
		}

		let color_usage = wgpu::TextureUsages::TEXTURE_BINDING
			| wgpu::TextureUsages::STORAGE_BINDING
			| wgpu::TextureUsages::RENDER_ATTACHMENT
			| wgpu::TextureUsages::COPY_SRC
			| wgpu::TextureUsages::COPY_DST;
		let tex_in = create_tex2d(&self.device, "tex_in", width, height, wgpu::TextureFormat::Rgba8Unorm, color_usage);
		let tex_a = create_tex2d(&self.device, "tex_a", width, height, wgpu::TextureFormat::Rgba8Unorm, color_usage);
		let tex_b = create_tex2d(&self.device, "tex_b", width, height, wgpu::TextureFormat::Rgba8Unorm, color_usage);

		let jfa_usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
		let jfa_a = create_tex2d(&self.device, "jfa_a", width, height, wgpu::TextureFormat::Rgba32Float, jfa_usage);
		let jfa_b = create_tex2d(&self.device, "jfa_b", width, height, wgpu::TextureFormat::Rgba32Float, jfa_usage);
		let sdf_normals = create_tex2d(
			&self.device,
			"sdf_normals",
			width,
			height,
			wgpu::TextureFormat::Rgba16Float,
			jfa_usage,
		);

		let view_in = tex_in.create_view(&Default::default());
		let view_a = tex_a.create_view(&Default::default());
		let view_b = tex_b.create_view(&Default::default());
		let view_jfa_a = jfa_a.create_view(&Default::default());
		let view_jfa_b = jfa_b.create_view(&Default::default());
		let view_sdf = sdf_normals.create_view(&Default::default());

		// Populate all dynamic JFA step uniform slots (slot p => step_px = 1 << p) once for this size.
		let mut step_bytes = vec![0u8; JFA_STEP_STRIDE as usize * (JFA_MAX_POW2 + 1)];
		for p in 0..=JFA_MAX_POW2 {
			let uni = JfaUniforms {
				width,
				height,
				step_px: 1i32 << p,
				_pad: 0,
			};
			let off = p * JFA_STEP_STRIDE as usize;
			let u_bytes = bytemuck::bytes_of(&uni);
			step_bytes[off..off + u_bytes.len()].copy_from_slice(u_bytes);
		}
		self.queue.write_buffer(&self.jfa_step_buf, 0, &step_bytes);

		let jfa_binding_size = NonZeroU64::new(std::mem::size_of::<JfaUniforms>() as u64);

		let warp_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("warp_bg"),
			layout: &self.warp_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_in) },
				wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view_a) },
				wgpu::BindGroupEntry { binding: 2, resource: self.warp_uniform_buf.as_entire_binding() },
				wgpu::BindGroupEntry { binding: 3, resource: self.warp_zones_buf.as_entire_binding() },
			],
		});

		let jfa_seed_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("jfa_seed_bg"),
			layout: &self.jfa_seed_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_a) },
				wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view_jfa_a) },
				wgpu::BindGroupEntry {
					binding: 2,
					resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
						buffer: &self.jfa_step_buf,
						offset: 0,
						size: jfa_binding_size,
					}),
				},
			],
		});

		let jfa_step_a_to_b_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("jfa_a_b_bg"),
			layout: &self.jfa_step_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_jfa_a) },
				wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view_jfa_b) },
				wgpu::BindGroupEntry {
					binding: 2,
					resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
						buffer: &self.jfa_step_buf,
						offset: 0,
						size: jfa_binding_size,
					}),
				},
			],
		});

		let jfa_step_b_to_a_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("jfa_b_a_bg"),
			layout: &self.jfa_step_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_jfa_b) },
				wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view_jfa_a) },
				wgpu::BindGroupEntry {
					binding: 2,
					resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
						buffer: &self.jfa_step_buf,
						offset: 0,
						size: jfa_binding_size,
					}),
				},
			],
		});

		let rim_bg_a = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("rim_bg_a"),
			layout: &self.rim_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_a) },
				wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view_jfa_a) },
				wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view_b) },
				wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view_sdf) },
				wgpu::BindGroupEntry { binding: 4, resource: self.rim_uniform_buf.as_entire_binding() },
			],
		});

		let rim_bg_b = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("rim_bg_b"),
			layout: &self.rim_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_a) },
				wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view_jfa_b) },
				wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view_b) },
				wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view_sdf) },
				wgpu::BindGroupEntry { binding: 4, resource: self.rim_uniform_buf.as_entire_binding() },
			],
		});

		let particle_sim_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("particle_sim_bg"),
			layout: &self.particle_sim_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_sdf) },
				wgpu::BindGroupEntry { binding: 1, resource: self.particles_buf.as_entire_binding() },
				wgpu::BindGroupEntry { binding: 2, resource: self.particle_sim_uniform_buf.as_entire_binding() },
			],
		});

		let unpadded = width * 4;
		let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
		let padded_bytes_per_row = unpadded.div_ceil(align) * align;
		let readback_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("readback_buf"),
			size: padded_bytes_per_row as u64 * height as u64,
			usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});

		self.targets = Some(TargetCache {
			width,
			height,
			tex_in,
			_tex_a: tex_a,
			tex_b,
			view_b,
			_jfa_a: jfa_a,
			_jfa_b: jfa_b,
			readback_buf,
			padded_bytes_per_row,
			warp_bg,
			jfa_seed_bg,
			jfa_step_a_to_b_bg,
			jfa_step_b_to_a_bg,
			rim_bg_a,
			rim_bg_b,
			particle_sim_bg,
		});
	}

	/// Execute all M2 GPU passes on `input` synchronized with `motion` and return the processed frame.
	pub fn process_frame(
		&mut self,
		input: &FrameBuffer,
		motion: &MotionHint,
		dt: f32,
	) -> Result<FrameBuffer, GpuError> {
		let w = input.width;
		let h = input.height;
		if w == 0 || h == 0 {
			return Err(GpuError::Transport(TransportError::InvalidDimensions(w, h)));
		}

		let safe_dt = if dt.is_finite() && dt > 0.0 { dt.min(0.05) } else { 1.0 / 60.0 };
		self.time_s += safe_dt;
		self.soft_body.step(motion, safe_dt);
		let amb = self.ambient.sample();

		self.ensure_targets(w, h);
		let tc = self.targets.as_ref().unwrap();

		let rgba = input.to_rgba8();
		self.queue.write_texture(
			wgpu::TexelCopyTextureInfo {
				texture: &tc.tex_in,
				mip_level: 0,
				origin: wgpu::Origin3d::ZERO,
				aspect: wgpu::TextureAspect::All,
			},
			&rgba,
			wgpu::TexelCopyBufferLayout {
				offset: 0,
				bytes_per_row: Some(w * 4),
				rows_per_image: Some(h),
			},
			wgpu::Extent3d {
				width: w,
				height: h,
				depth_or_array_layers: 1,
			},
		);

		let zones_gpu = self.soft_body.pack_gpu_zones();
		self.queue.write_buffer(&self.warp_zones_buf, 0, bytemuck::cast_slice(&zones_gpu));
		let warp_u = WarpUniforms {
			width: w,
			height: h,
			zone_count: self.soft_body.grids.len().min(MAX_SOFT_BODY_ZONES) as u32,
			enabled: u32::from(self.config.enable_soft_body),
		};
		self.queue.write_buffer(&self.warp_uniform_buf, 0, bytemuck::bytes_of(&warp_u));

		let rim_u = RimUniforms {
			dims: [
				w as f32,
				h as f32,
				self.config.rim_width_px.max(1.0),
				if self.config.enable_rim_light {
					self.config.rim_intensity.clamp(0.0, 3.0)
				} else {
					0.0
				},
			],
			left_rgb: [amb.left.x, amb.left.y, amb.left.z, 1.0],
			right_rgb: [amb.right.x, amb.right.y, amb.right.z, 1.0],
			top_rgb: [amb.top.x, amb.top.y, amb.top.z, 1.0],
			bottom_rgb: [amb.bottom.x, amb.bottom.y, amb.bottom.z, 1.0],
		};
		self.queue.write_buffer(&self.rim_uniform_buf, 0, bytemuck::bytes_of(&rim_u));

		let p_count = self.config.particle_count.min(MAX_GPU_PARTICLES);
		let p_sim_u = ParticleSimUniforms {
			dims_dt: [w as f32, h as f32, safe_dt, self.time_s],
			motion: [
				motion.wind_vec.x,
				motion.wind_vec.y,
				motion.overall_motion,
				p_count as f32,
			],
			accent_spawn: [
				(amb.left.x + amb.right.x) * 0.5,
				(amb.left.y + amb.right.y) * 0.5,
				(amb.left.z + amb.right.z) * 0.5,
				self.config.particle_spawn_rate.clamp(0.0, 2.0),
			],
		};
		self.queue
			.write_buffer(&self.particle_sim_uniform_buf, 0, bytemuck::bytes_of(&p_sim_u));

		let p_render_u = ParticleRenderUniforms {
			dims: [w as f32, h as f32, 0.0, 0.0],
		};
		self.queue
			.write_buffer(&self.particle_render_uniform_buf, 0, bytemuck::bytes_of(&p_render_u));

		let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
			label: Some("riglens_m2_enc"),
		});
		let wg_x = w.div_ceil(8);
		let wg_y = h.div_ceil(8);

		// 1) Soft-body warp pass: tex_in -> tex_a
		{
			let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
				label: Some("soft_body_warp_pass"),
				timestamp_writes: None,
			});
			pass.set_pipeline(&self.warp_pipe);
			pass.set_bind_group(0, &tc.warp_bg, &[]);
			pass.dispatch_workgroups(wg_x, wg_y, 1);
		}

		// 2) JFA seed pass: tex_a -> jfa_a
		{
			let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
				label: Some("jfa_seed_pass"),
				timestamp_writes: None,
			});
			pass.set_pipeline(&self.jfa_seed_pipe);
			pass.set_bind_group(0, &tc.jfa_seed_bg, &[]);
			pass.dispatch_workgroups(wg_x, wg_y, 1);
		}

		// 3) JFA logarithmic step passes: start from power-of-two covering max(w, h)/2 down to 1
		let max_dim = w.max(h).max(2);
		let max_pow = ((max_dim - 1).ilog2() as usize).min(JFA_MAX_POW2);
		let mut src_is_a = true;
		{
			let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
				label: Some("jfa_steps_pass"),
				timestamp_writes: None,
			});
			pass.set_pipeline(&self.jfa_step_pipe);
			for p in (0..=max_pow).rev() {
				let dyn_off = (p as u32) * (JFA_STEP_STRIDE as u32);
				let bg = if src_is_a {
					&tc.jfa_step_a_to_b_bg
				} else {
					&tc.jfa_step_b_to_a_bg
				};
				pass.set_bind_group(0, bg, &[dyn_off]);
				pass.dispatch_workgroups(wg_x, wg_y, 1);
				src_is_a = !src_is_a;
			}
		}

		// 4) SDF normal + 2.5D directional rim lighting: (tex_a, final_jfa) -> (tex_b, sdf_normals)
		{
			let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
				label: Some("sdf_rim_pass"),
				timestamp_writes: None,
			});
			pass.set_pipeline(&self.rim_pipe);
			let rim_bg = if src_is_a { &tc.rim_bg_a } else { &tc.rim_bg_b };
			pass.set_bind_group(0, rim_bg, &[]);
			pass.dispatch_workgroups(wg_x, wg_y, 1);
		}

		// 5) GPU Particle simulation & instanced alpha glow rendering onto tex_b
		if p_count > 0 {
			{
				let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
					label: Some("particles_sim_pass"),
					timestamp_writes: None,
				});
				pass.set_pipeline(&self.particle_sim_pipe);
				pass.set_bind_group(0, &tc.particle_sim_bg, &[]);
				pass.dispatch_workgroups(p_count.div_ceil(64), 1, 1);
			}
			{
				let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
					label: Some("particles_render_pass"),
					color_attachments: &[Some(wgpu::RenderPassColorAttachment {
						view: &tc.view_b,
						resolve_target: None,
						ops: wgpu::Operations {
							load: wgpu::LoadOp::Load,
							store: wgpu::StoreOp::Store,
						},
					})],
					depth_stencil_attachment: None,
					timestamp_writes: None,
					occlusion_query_set: None,
				});
				rpass.set_pipeline(&self.particle_render_pipe);
				rpass.set_bind_group(0, &self.particle_render_bg, &[]);
				rpass.draw(0..4, 0..p_count);
			}
		}

		// 6) Copy final processed frame tex_b -> staging readback buffer
		encoder.copy_texture_to_buffer(
			wgpu::TexelCopyTextureInfo {
				texture: &tc.tex_b,
				mip_level: 0,
				origin: wgpu::Origin3d::ZERO,
				aspect: wgpu::TextureAspect::All,
			},
			wgpu::TexelCopyBufferInfo {
				buffer: &tc.readback_buf,
				layout: wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(tc.padded_bytes_per_row),
					rows_per_image: Some(h),
				},
			},
			wgpu::Extent3d {
				width: w,
				height: h,
				depth_or_array_layers: 1,
			},
		);

		self.queue.submit(Some(encoder.finish()));

		let slice = tc.readback_buf.slice(..);
		let (tx, rx) = std::sync::mpsc::channel();
		slice.map_async(wgpu::MapMode::Read, move |res| {
			let _ = tx.send(res);
		});
		self.device.poll(wgpu::Maintain::Wait);
		match rx.recv() {
			Ok(Ok(())) => {}
			Ok(Err(e)) => return Err(GpuError::BufferMap(format!("{e}"))),
			Err(e) => return Err(GpuError::BufferMap(format!("channel: {e}"))),
		}

		let mapped = slice.get_mapped_range();
		let unpadded_row = (w * 4) as usize;
		let padded_row = tc.padded_bytes_per_row as usize;
		let mut out_rgba = Vec::with_capacity(unpadded_row * h as usize);
		for y in 0..h as usize {
			let row_start = y * padded_row;
			out_rgba.extend_from_slice(&mapped[row_start..row_start + unpadded_row]);
		}
		drop(mapped);
		tc.readback_buf.unmap();

		Ok(FrameBuffer::new(w, h, PixelFormat::Rgba8, out_rgba)?)
	}
}

fn create_tex2d(
	device: &wgpu::Device,
	label: &str,
	width: u32,
	height: u32,
	format: wgpu::TextureFormat,
	usage: wgpu::TextureUsages,
) -> wgpu::Texture {
	device.create_texture(&wgpu::TextureDescriptor {
		label: Some(label),
		size: wgpu::Extent3d {
			width,
			height,
			depth_or_array_layers: 1,
		},
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format,
		usage,
		view_formats: &[],
	})
}

fn create_uniform_buf(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
	device.create_buffer(&wgpu::BufferDescriptor {
		label: Some(label),
		size,
		usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
		mapped_at_creation: false,
	})
}

fn create_compute_pipe(
	device: &wgpu::Device,
	label: &str,
	bgl: &wgpu::BindGroupLayout,
	module: &wgpu::ShaderModule,
) -> wgpu::ComputePipeline {
	let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
		label: Some(label),
		bind_group_layouts: &[bgl],
		push_constant_ranges: &[],
	});
	device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
		label: Some(label),
		layout: Some(&layout),
		module,
		entry_point: Some("main"),
		compilation_options: wgpu::PipelineCompilationOptions::default(),
		cache: None,
	})
}

fn bgl_tex(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
	wgpu::BindGroupLayoutEntry {
		binding,
		visibility: wgpu::ShaderStages::COMPUTE,
		ty: wgpu::BindingType::Texture {
			sample_type: wgpu::TextureSampleType::Float { filterable },
			view_dimension: wgpu::TextureViewDimension::D2,
			multisampled: false,
		},
		count: None,
	}
}

fn bgl_storage_tex(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
	wgpu::BindGroupLayoutEntry {
		binding,
		visibility: wgpu::ShaderStages::COMPUTE,
		ty: wgpu::BindingType::StorageTexture {
			access: wgpu::StorageTextureAccess::WriteOnly,
			format,
			view_dimension: wgpu::TextureViewDimension::D2,
		},
		count: None,
	}
}

fn bgl_uniform(binding: u32, has_dynamic_offset: bool, min_size: Option<NonZeroU64>) -> wgpu::BindGroupLayoutEntry {
	wgpu::BindGroupLayoutEntry {
		binding,
		visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::VERTEX_FRAGMENT,
		ty: wgpu::BindingType::Buffer {
			ty: wgpu::BufferBindingType::Uniform,
			has_dynamic_offset,
			min_binding_size: min_size,
		},
		count: None,
	}
}

fn bgl_storage_buf(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
	wgpu::BindGroupLayoutEntry {
		binding,
		visibility: if read_only {
			wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::VERTEX
		} else {
			wgpu::ShaderStages::COMPUTE
		},
		ty: wgpu::BindingType::Buffer {
			ty: wgpu::BufferBindingType::Storage { read_only },
			has_dynamic_offset: false,
			min_binding_size: None,
		},
		count: None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use self::ambient::AmbientColors;
	use glam::{vec2, vec3, Vec2};

	fn paint_opaque_disc(w: u32, h: u32, radius: f32) -> FrameBuffer {
		let mut rgba = vec![0u8; (w * h * 4) as usize];
		let cx = w as f32 * 0.5;
		let cy = h as f32 * 0.5;
		for y in 0..h {
			for x in 0..w {
				let dx = x as f32 + 0.5 - cx;
				let dy = y as f32 + 0.5 - cy;
				if dx * dx + dy * dy <= radius * radius {
					let idx = ((y * w + x) * 4) as usize;
					rgba[idx] = 64;
					rgba[idx + 1] = 64;
					rgba[idx + 2] = 64;
					rgba[idx + 3] = 255;
				}
			}
		}
		FrameBuffer::new(w, h, PixelFormat::Rgba8, rgba).unwrap()
	}

	#[test]
	fn wgpu_m2_rim_light_respects_alpha_and_directional_sdf_normals() {
		let mut cfg = GpuEffectsConfig {
			enable_soft_body: false,
			soft_body_zones: Vec::new(),
			enable_rim_light: true,
			rim_width_px: 12.0,
			rim_intensity: 1.0,
			ambient_theme: AmbientTheme::Custom(AmbientColors {
				left: vec3(0.0, 0.9, 1.0),   // Cyan on left edge
				right: vec3(1.0, 0.1, 0.6),  // Magenta on right edge
				top: vec3(0.5, 0.5, 0.5),
				bottom: vec3(0.5, 0.5, 0.5),
			}),
			particle_count: 0,
			particle_spawn_rate: 0.0,
		};
		cfg.particle_count = 0;

		let mut gpu = GpuFrameProcessor::new_blocking(cfg).expect("init wgpu GPU processor");
		let disc = paint_opaque_disc(128, 128, 40.0);
		let out = gpu
			.process_frame(&disc, &MotionHint::default(), 1.0 / 60.0)
			.expect("process disc");

		let px = |x: u32, y: u32| -> [u8; 4] {
			let i = ((y * 128 + x) * 4) as usize;
			[out.data[i], out.data[i + 1], out.data[i + 2], out.data[i + 3]]
		};

		// Outside silhouette must remain strictly transparent & zero color leak
		assert_eq!(px(4, 4), [0, 0, 0, 0]);
		assert_eq!(px(120, 120), [0, 0, 0, 0]);

		// Deep center stays near base gray (rim faded out inside)
		let center = px(64, 64);
		assert_eq!(center[3], 255);
		assert!(center[0] < 80 && center[1] < 80 && center[2] < 80, "center shouldn't glow: {center:?}");

		// Left silhouette inner edge (x=27, y=64 -> dist ~3px from left rim x=24) receives cyan (B > R)
		let left_edge = px(27, 64);
		assert_eq!(left_edge[3], 255);
		assert!(
			left_edge[2] > left_edge[0] + 60,
			"expected cyan left rim (B >> R), got {left_edge:?}"
		);

		// Right silhouette inner edge (x=101, y=64 -> dist ~3px from right rim x=104) receives magenta (R > G)
		let right_edge = px(101, 64);
		assert_eq!(right_edge[3], 255);
		assert!(
			right_edge[0] > right_edge[1] + 60,
			"expected magenta right rim (R >> G), got {right_edge:?}"
		);
	}

	#[test]
	fn wgpu_m2_soft_body_and_particles_execute_cleanly() {
		let cfg = GpuEffectsConfig {
			enable_soft_body: true,
			soft_body_zones: vec![SoftBodyZoneConfig {
				name: "CenterJelly".to_owned(),
				anchor_uv: vec2(0.5, 0.5),
				radius_uv: vec2(0.30, 0.30),
				follow_head: 2.0,
				wind_sensitivity: 2.0,
				stiffness: 110.0,
				damping: 4.5,
				max_strain: 0.45,
			}],
			enable_rim_light: true,
			rim_width_px: 8.0,
			rim_intensity: 0.6,
			ambient_theme: AmbientTheme::Cyberpunk,
			particle_count: 128,
			particle_spawn_rate: 1.0,
		};
		let mut gpu = GpuFrameProcessor::new_blocking(cfg).expect("init wgpu GPU processor");

		// Paint checkerboard pattern inside disc so UV warp visibly shifts pixels
		let w = 128u32;
		let h = 128u32;
		let mut rgba = vec![0u8; (w * h * 4) as usize];
		for y in 0..h {
			for x in 0..w {
				let dx = x as f32 + 0.5 - 64.0;
				let dy = y as f32 + 0.5 - 64.0;
				if dx * dx + dy * dy <= 44.0 * 44.0 {
					let i = ((y * w + x) * 4) as usize;
					let checker = ((x / 8 + y / 8) % 2) == 0;
					rgba[i] = if checker { 220 } else { 40 };
					rgba[i + 1] = 120;
					rgba[i + 2] = 160;
					rgba[i + 3] = 255;
				}
			}
		}
		let input = FrameBuffer::new(w, h, PixelFormat::Rgba8, rgba).unwrap();

		let hint_impulse = MotionHint {
			face_found: true,
			head_velocity: vec2(25.0, -10.0),
			overall_motion: 1.5,
			wind_vec: vec2(1.0, 0.2),
			..Default::default()
		};
		let out1 = gpu.process_frame(&input, &hint_impulse, 1.0 / 60.0).unwrap();
		let mut out_late = out1.clone();
		for f in 0..8 {
			let hint = MotionHint {
				face_found: true,
				head_velocity: if f == 0 { vec2(-35.0, 15.0) } else { Vec2::ZERO },
				overall_motion: 1.2,
				wind_vec: vec2(1.0, 0.3),
				..Default::default()
			};
			out_late = gpu.process_frame(&input, &hint, 1.0 / 60.0).unwrap();
		}
		assert_ne!(out1.data, out_late.data, "expected soft-body + particles to change output over time");
	}
}

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};

pub(super) const JFA_STEP_STRIDE: u64 = 256;
pub(super) const JFA_MAX_POW2: usize = 14; // up to 16384px

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct WarpUniforms {
	pub(super) width: u32,
	pub(super) height: u32,
	pub(super) zone_count: u32,
	pub(super) enabled: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct JfaUniforms {
	pub(super) width: u32,
	pub(super) height: u32,
	pub(super) step_px: i32,
	pub(super) _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct RimUniforms {
	pub(super) dims: [f32; 4],
	pub(super) left_rgb: [f32; 4],
	pub(super) right_rgb: [f32; 4],
	pub(super) top_rgb: [f32; 4],
	pub(super) bottom_rgb: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct ParticleSimUniforms {
	pub(super) dims_dt: [f32; 4],
	pub(super) motion: [f32; 4],
	pub(super) accent_spawn: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct ParticleRenderUniforms {
	pub(super) dims: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct GpuParticle {
	pub(super) pos_vel: [f32; 4],
	pub(super) color: [f32; 4],
	pub(super) meta: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct UpscaleUniforms {
	pub(super) dims: [f32; 4],
	pub(super) params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct FlowUniforms {
	pub(super) dims: [f32; 4],
	pub(super) params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct FrameGenUniforms {
	pub(super) dims: [f32; 4],
}

pub(super) struct TargetCache {
	pub(super) width: u32,
	pub(super) height: u32,
	pub(super) out_width: u32,
	pub(super) out_height: u32,
	pub(super) tex_in: wgpu::Texture,
	pub(super) _tex_a: wgpu::Texture,
	pub(super) tex_b: wgpu::Texture,
	pub(super) view_b: wgpu::TextureView,
	pub(super) _jfa_a: wgpu::Texture,
	pub(super) _jfa_b: wgpu::Texture,
	pub(super) tex_prev: wgpu::Texture,
	pub(super) has_prev: bool,
	pub(super) flow_tex: wgpu::Texture,
	pub(super) tex_interp: wgpu::Texture,
	pub(super) tex_upscaled: wgpu::Texture,
	pub(super) readback_buf: wgpu::Buffer,
	pub(super) padded_bytes_per_row: u32,
	pub(super) warp_bg: wgpu::BindGroup,
	pub(super) jfa_seed_bg: wgpu::BindGroup,
	pub(super) jfa_step_a_to_b_bg: wgpu::BindGroup,
	pub(super) jfa_step_b_to_a_bg: wgpu::BindGroup,
	pub(super) rim_bg_a: wgpu::BindGroup,
	pub(super) rim_bg_b: wgpu::BindGroup,
	pub(super) particle_sim_bg: wgpu::BindGroup,
	pub(super) flow_bg: wgpu::BindGroup,
	pub(super) frame_gen_bg: wgpu::BindGroup,
	pub(super) upscale_from_b_bg: wgpu::BindGroup,
	pub(super) upscale_from_interp_bg: wgpu::BindGroup,
}

pub(super) fn create_tex2d(
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

pub(super) fn create_uniform_buf(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
	device.create_buffer(&wgpu::BufferDescriptor {
		label: Some(label),
		size,
		usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
		mapped_at_creation: false,
	})
}

pub(super) fn create_compute_pipe(
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

pub(super) fn bgl_tex(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
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

pub(super) fn bgl_storage_tex(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
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

pub(super) fn bgl_uniform(binding: u32, has_dynamic_offset: bool, min_size: Option<NonZeroU64>) -> wgpu::BindGroupLayoutEntry {
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

pub(super) fn bgl_storage_buf(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
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

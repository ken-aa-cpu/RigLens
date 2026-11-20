use std::num::NonZeroU64;

use super::resources::{create_tex2d, JfaUniforms, TargetCache, JFA_MAX_POW2, JFA_STEP_STRIDE};
use super::GpuFrameProcessor;

impl GpuFrameProcessor {
	pub(super) fn ensure_targets(&mut self, width: u32, height: u32, out_width: u32, out_height: u32) {
		if let Some(ref tc) = self.targets {
			if tc.width == width
				&& tc.height == height
				&& tc.out_width == out_width
				&& tc.out_height == out_height
			{
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
		let tex_prev = create_tex2d(
			&self.device,
			"tex_prev",
			width,
			height,
			wgpu::TextureFormat::Rgba8Unorm,
			wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
		);
		let tex_interp = create_tex2d(
			&self.device,
			"tex_interp",
			width,
			height,
			wgpu::TextureFormat::Rgba8Unorm,
			wgpu::TextureUsages::TEXTURE_BINDING
				| wgpu::TextureUsages::STORAGE_BINDING
				| wgpu::TextureUsages::COPY_SRC,
		);
		let tex_upscaled = create_tex2d(
			&self.device,
			"tex_upscaled",
			out_width,
			out_height,
			wgpu::TextureFormat::Rgba8Unorm,
			wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
		);

		let jfa_usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
		let jfa_a = create_tex2d(&self.device, "jfa_a", width, height, wgpu::TextureFormat::Rgba32Float, jfa_usage);
		let jfa_b = create_tex2d(&self.device, "jfa_b", width, height, wgpu::TextureFormat::Rgba32Float, jfa_usage);
		let flow_tex = create_tex2d(
			&self.device,
			"flow_tex",
			width,
			height,
			wgpu::TextureFormat::Rgba32Float,
			jfa_usage | wgpu::TextureUsages::COPY_DST,
		);
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
		let view_prev = tex_prev.create_view(&Default::default());
		let view_interp = tex_interp.create_view(&Default::default());
		let view_upscaled = tex_upscaled.create_view(&Default::default());
		let view_jfa_a = jfa_a.create_view(&Default::default());
		let view_jfa_b = jfa_b.create_view(&Default::default());
		let view_flow = flow_tex.create_view(&Default::default());
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

		let flow_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("flow_bg"),
			layout: &self.flow_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_prev) },
				wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view_b) },
				wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view_flow) },
				wgpu::BindGroupEntry { binding: 3, resource: self.flow_uniform_buf.as_entire_binding() },
			],
		});

		let frame_gen_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("frame_gen_bg"),
			layout: &self.frame_gen_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_prev) },
				wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view_b) },
				wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view_flow) },
				wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view_interp) },
				wgpu::BindGroupEntry { binding: 4, resource: self.frame_gen_uniform_buf.as_entire_binding() },
			],
		});

		let upscale_from_b_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("upscale_from_b_bg"),
			layout: &self.upscale_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_b) },
				wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view_upscaled) },
				wgpu::BindGroupEntry { binding: 2, resource: self.upscale_uniform_buf.as_entire_binding() },
			],
		});

		let upscale_from_interp_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("upscale_from_interp_bg"),
			layout: &self.upscale_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_interp) },
				wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view_upscaled) },
				wgpu::BindGroupEntry { binding: 2, resource: self.upscale_uniform_buf.as_entire_binding() },
			],
		});

		let unpadded = out_width * 4;
		let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
		let padded_bytes_per_row = unpadded.div_ceil(align) * align;
		let readback_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("readback_buf"),
			size: padded_bytes_per_row as u64 * out_height as u64,
			usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});

		self.targets = Some(TargetCache {
			width,
			height,
			out_width,
			out_height,
			tex_in,
			_tex_a: tex_a,
			tex_b,
			view_b,
			_jfa_a: jfa_a,
			_jfa_b: jfa_b,
			tex_prev,
			has_prev: false,
			flow_tex,
			tex_interp,
			tex_upscaled,
			readback_buf,
			padded_bytes_per_row,
			warp_bg,
			jfa_seed_bg,
			jfa_step_a_to_b_bg,
			jfa_step_b_to_a_bg,
			rim_bg_a,
			rim_bg_b,
			particle_sim_bg,
			flow_bg,
			frame_gen_bg,
			upscale_from_b_bg,
			upscale_from_interp_bg,
		});
	}
}

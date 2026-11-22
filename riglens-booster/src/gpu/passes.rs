use crate::physics::MotionHint;
use crate::transport::{FrameBuffer, PixelFormat, TransportError};

use super::mesh_velocity::MotionVectorField;
use super::resources::{
	FlowUniforms, FrameGenUniforms, ParticleRenderUniforms, ParticleSimUniforms, RimUniforms, TargetCache,
	UpscaleUniforms, WarpUniforms, JFA_MAX_POW2, JFA_STEP_STRIDE,
};
use super::soft_body::MAX_SOFT_BODY_ZONES;
use super::{GpuError, GpuFrameProcessor, MAX_GPU_PARTICLES};

impl GpuFrameProcessor {
	fn encode_m2_passes(
		&self,
		encoder: &mut wgpu::CommandEncoder,
		tc: &TargetCache,
		w: u32,
		h: u32,
		p_count: u32,
	) {
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
	}

	fn encode_readback_and_map(
		&self,
		mut encoder: wgpu::CommandEncoder,
		src_tex: &wgpu::Texture,
		readback_buf: &wgpu::Buffer,
		padded_bytes_per_row: u32,
		out_w: u32,
		out_h: u32,
	) -> Result<FrameBuffer, GpuError> {
		encoder.copy_texture_to_buffer(
			wgpu::TexelCopyTextureInfo {
				texture: src_tex,
				mip_level: 0,
				origin: wgpu::Origin3d::ZERO,
				aspect: wgpu::TextureAspect::All,
			},
			wgpu::TexelCopyBufferInfo {
				buffer: readback_buf,
				layout: wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(padded_bytes_per_row),
					rows_per_image: Some(out_h),
				},
			},
			wgpu::Extent3d {
				width: out_w,
				height: out_h,
				depth_or_array_layers: 1,
			},
		);

		self.queue.submit(Some(encoder.finish()));

		let slice = readback_buf.slice(..);
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
		let unpadded_row = (out_w * 4) as usize;
		let padded_row = padded_bytes_per_row as usize;
		let mut out_rgba = Vec::with_capacity(unpadded_row * out_h as usize);
		for y in 0..out_h as usize {
			let row_start = y * padded_row;
			out_rgba.extend_from_slice(&mapped[row_start..row_start + unpadded_row]);
		}
		drop(mapped);
		readback_buf.unmap();

		Ok(FrameBuffer::new(out_w, out_h, PixelFormat::Rgba8, out_rgba)?)
	}

	/// Execute M2 + M3 passes on `input`, returning 1 frame (or `frame_gen_multiplier` frames).
	pub fn process_frame(
		&mut self,
		input: &FrameBuffer,
		motion: &MotionHint,
		dt: f32,
	) -> Result<FrameBuffer, GpuError> {
		let mut frames = self.process_frame_with_gen(input, motion, dt, None)?;
		frames
			.pop()
			.ok_or_else(|| GpuError::BufferMap("GPU produced zero frames".to_owned()))
	}

	/// Execute M2 + M3 GPU passes (`2D-Native DLSS` upscaling & dual-tier motion-vector frame generation).
	/// When `mesh_velocity` matches `(input.width, input.height)` it is used directly as Tier 1 ground-truth flow;
	/// otherwise Tier 2 compute optical flow (`flow_estimate.wgsl`) runs on `(tex_prev, tex_b)`.
	pub fn process_frame_with_gen(
		&mut self,
		input: &FrameBuffer,
		motion: &MotionHint,
		dt: f32,
		mesh_velocity: Option<&MotionVectorField>,
	) -> Result<Vec<FrameBuffer>, GpuError> {
		let w = input.width;
		let h = input.height;
		if w == 0 || h == 0 {
			return Err(GpuError::Transport(TransportError::InvalidDimensions(w, h)));
		}

		let safe_dt = if dt.is_finite() && dt > 0.0 { dt.min(0.05) } else { 1.0 / 60.0 };
		self.time_s += safe_dt;
		self.soft_body.step(motion, safe_dt);
		let amb = self.ambient.sample();
		let (out_w, out_h) = self.output_dimensions(w, h);
		let max_dim = self.device.limits().max_texture_dimension_2d;
		if w > max_dim || h > max_dim || out_w > max_dim || out_h > max_dim {
			return Err(GpuError::DimensionsExceedLimit {
				width: w.max(out_w),
				height: h.max(out_h),
				max: max_dim,
			});
		}
		let use_upscale = out_w != w || out_h != h;
		let mult = self.config.frame_gen_multiplier.clamp(1, 4);

		self.ensure_targets(w, h, out_w, out_h);
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

		if use_upscale {
			let upscale_u = UpscaleUniforms {
				dims: [w as f32, h as f32, out_w as f32, out_h as f32],
				params: [self.config.upscale_sharpness.clamp(0.0, 2.0), 0.0, 0.0, 0.0],
			};
			self.queue
				.write_buffer(&self.upscale_uniform_buf, 0, bytemuck::bytes_of(&upscale_u));
		}

		let wg_x = w.div_ceil(8);
		let wg_y = h.div_ceil(8);
		let out_wg_x = out_w.div_ceil(8);
		let out_wg_y = out_h.div_ceil(8);
		let tex_extent = wgpu::Extent3d {
			width: w,
			height: h,
			depth_or_array_layers: 1,
		};

		if mult > 1 && tc.has_prev {
			let has_tier1 = mesh_velocity
				.filter(|mv| mv.width == w && mv.height == h && mv.data.len() == (w * h) as usize);

			if let Some(mv) = has_tier1 {
				self.queue.write_texture(
					wgpu::TexelCopyTextureInfo {
						texture: &tc.flow_tex,
						mip_level: 0,
						origin: wgpu::Origin3d::ZERO,
						aspect: wgpu::TextureAspect::All,
					},
					bytemuck::cast_slice(&mv.data),
					wgpu::TexelCopyBufferLayout {
						offset: 0,
						bytes_per_row: Some(w * 16),
						rows_per_image: Some(h),
					},
					tex_extent,
				);
			} else {
				let prior_vx = (motion.head_velocity.x * safe_dt * w as f32 * 0.05).clamp(-16.0, 16.0);
				let prior_vy = (motion.head_velocity.y * safe_dt * h as f32 * 0.05).clamp(-16.0, 16.0);
				let angular_rad = (motion.head_angular_vel_z.to_radians() * safe_dt).clamp(-0.25, 0.25);
				let flow_u = FlowUniforms {
					dims: [w as f32, h as f32, prior_vx, prior_vy],
					params: [angular_rad, 24.0, motion.eye_open_min.clamp(0.0, 1.0), 0.0],
				};
				self.queue
					.write_buffer(&self.flow_uniform_buf, 0, bytemuck::bytes_of(&flow_u));
			}

			let mut enc_m2_flow = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
				label: Some("riglens_m2_flow_enc"),
			});
			self.encode_m2_passes(&mut enc_m2_flow, tc, w, h, p_count);
			if has_tier1.is_none() {
				let mut pass = enc_m2_flow.begin_compute_pass(&wgpu::ComputePassDescriptor {
					label: Some("flow_estimate_pass"),
					timestamp_writes: None,
				});
				pass.set_pipeline(&self.flow_pipe);
				pass.set_bind_group(0, &tc.flow_bg, &[]);
				pass.dispatch_workgroups(wg_x, wg_y, 1);
			}
			self.queue.submit(Some(enc_m2_flow.finish()));

			let mut out_frames = Vec::with_capacity(mult as usize);
			for k in 1..mult {
				let tau = k as f32 / mult as f32;
				let fg_u = FrameGenUniforms {
					dims: [w as f32, h as f32, tau, 0.0],
				};
				self.queue
					.write_buffer(&self.frame_gen_uniform_buf, 0, bytemuck::bytes_of(&fg_u));

				let mut enc_fg = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
					label: Some("riglens_m3_fg_enc"),
				});
				{
					let mut pass = enc_fg.begin_compute_pass(&wgpu::ComputePassDescriptor {
						label: Some("frame_gen_warp_pass"),
						timestamp_writes: None,
					});
					pass.set_pipeline(&self.frame_gen_pipe);
					pass.set_bind_group(0, &tc.frame_gen_bg, &[]);
					pass.dispatch_workgroups(wg_x, wg_y, 1);
				}
				let src_tex = if use_upscale {
					let mut pass = enc_fg.begin_compute_pass(&wgpu::ComputePassDescriptor {
						label: Some("upscale_interp_pass"),
						timestamp_writes: None,
					});
					pass.set_pipeline(&self.upscale_pipe);
					pass.set_bind_group(0, &tc.upscale_from_interp_bg, &[]);
					pass.dispatch_workgroups(out_wg_x, out_wg_y, 1);
					&tc.tex_upscaled
				} else {
					&tc.tex_interp
				};
				out_frames.push(self.encode_readback_and_map(
					enc_fg,
					src_tex,
					&tc.readback_buf,
					tc.padded_bytes_per_row,
					out_w,
					out_h,
				)?);
			}

			let mut enc_curr = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
				label: Some("riglens_m3_curr_enc"),
			});
			let src_tex = if use_upscale {
				let mut pass = enc_curr.begin_compute_pass(&wgpu::ComputePassDescriptor {
					label: Some("upscale_curr_pass"),
					timestamp_writes: None,
				});
				pass.set_pipeline(&self.upscale_pipe);
				pass.set_bind_group(0, &tc.upscale_from_b_bg, &[]);
				pass.dispatch_workgroups(out_wg_x, out_wg_y, 1);
				&tc.tex_upscaled
			} else {
				&tc.tex_b
			};
			enc_curr.copy_texture_to_texture(
				tc.tex_b.as_image_copy(),
				tc.tex_prev.as_image_copy(),
				tex_extent,
			);
			out_frames.push(self.encode_readback_and_map(
				enc_curr,
				src_tex,
				&tc.readback_buf,
				tc.padded_bytes_per_row,
				out_w,
				out_h,
			)?);
			Ok(out_frames)
		} else {
			let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
				label: Some("riglens_m2_m3_enc"),
			});
			self.encode_m2_passes(&mut enc, tc, w, h, p_count);
			let src_tex = if use_upscale {
				let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
					label: Some("upscale_curr_pass"),
					timestamp_writes: None,
				});
				pass.set_pipeline(&self.upscale_pipe);
				pass.set_bind_group(0, &tc.upscale_from_b_bg, &[]);
				pass.dispatch_workgroups(out_wg_x, out_wg_y, 1);
				&tc.tex_upscaled
			} else {
				&tc.tex_b
			};
			if mult > 1 {
				enc.copy_texture_to_texture(
					tc.tex_b.as_image_copy(),
					tc.tex_prev.as_image_copy(),
					tex_extent,
				);
			}
			let frame = self.encode_readback_and_map(
				enc,
				src_tex,
				&tc.readback_buf,
				tc.padded_bytes_per_row,
				out_w,
				out_h,
			)?;
			self.targets.as_mut().unwrap().has_prev = mult > 1;
			Ok(vec![frame])
		}
	}
}

use std::cell::RefCell;

use glam::{Mat4, Vec2, Vec3};
use riglens_core::node::components::{BlendMode, Mask, MaskMode, Masks};
use riglens_core::node::drawables::{CompositeComponents, TexturedMeshComponents};
use riglens_core::node::InoxNodeUuid;
use riglens_core::puppet::Puppet;
use riglens_core::render::{CompositeRenderCtx, InoxRenderer, InoxRendererExt, TexturedMeshRenderCtx};

/// Static rest-pose vertex buffers extracted from `Puppet::render_ctx`.
#[derive(Clone)]
pub struct StageGeometry {
	pub verts: Vec<Vec2>,
	pub uvs: Vec<Vec2>,
	pub indices: Vec<u32>,
}

/// Parameters for a single `TexturedMesh` draw command.
#[derive(Clone, Debug)]
pub struct PartDraw {
	pub index_offset: u32,
	pub index_len: usize,
	pub tex_albedo: usize,
	pub mvp: Mat4,
	pub offset: Vec2,
	pub opacity: f32,
	pub tint: Vec3,
	pub screen_tint: Vec3,
	pub blend_mode: BlendMode,
	pub as_mask: bool,
}

/// Recorded draw operation in Inochi2D traversal order.
#[derive(Clone, Debug)]
pub enum StageDrawOp {
	BeginMasks {
		threshold: f32,
		has_normal_masks: bool,
	},
	BeginMask {
		mode: MaskMode,
	},
	BeginMaskedContent,
	EndMask,
	DrawPart(PartDraw),
	BeginComposite,
	FinishComposite {
		as_mask: bool,
		opacity: f32,
		tint: Vec3,
		screen_tint: Vec3,
		blend_mode: BlendMode,
	},
}

/// Evaluated per-frame draw list + deformed vertex buffer ready for GPU submission.
#[derive(Clone)]
pub struct StageFrame {
	pub ops: Vec<StageDrawOp>,
	pub deforms: Vec<Vec2>,
	pub wireframe: bool,
}

struct DrawRecorder<'a> {
	puppet: &'a Puppet,
	mvp_cam: Mat4,
	ops: RefCell<Vec<StageDrawOp>>,
	skip_composite: RefCell<bool>,
}

impl<'a> InoxRenderer for DrawRecorder<'a> {
	fn on_begin_masks(&self, masks: &Masks) {
		self.ops.borrow_mut().push(StageDrawOp::BeginMasks {
			threshold: masks.threshold.clamp(0.0, 1.0),
			has_normal_masks: masks.has_masks(),
		});
	}

	fn on_begin_mask(&self, mask: &Mask) {
		self.ops.borrow_mut().push(StageDrawOp::BeginMask { mode: mask.mode });
	}

	fn on_begin_masked_content(&self) {
		self.ops.borrow_mut().push(StageDrawOp::BeginMaskedContent);
	}

	fn on_end_mask(&self) {
		self.ops.borrow_mut().push(StageDrawOp::EndMask);
	}

	fn draw_textured_mesh_content(
		&self,
		as_mask: bool,
		components: &TexturedMeshComponents,
		render_ctx: &TexturedMeshRenderCtx,
		id: InoxNodeUuid,
	) {
		if *self.skip_composite.borrow() {
			return;
		}
		let enabled = self.puppet.nodes.is_effectively_enabled(id);
		if !enabled || render_ctx.index_len == 0 {
			return;
		}
		let opacity = components.opacity.clamp(0.0, 1.0);
		if !as_mask && opacity <= 0.002 {
			return;
		}
		let tint = components.drawable.blending.tint.clamp(Vec3::ZERO, Vec3::ONE);
		let screen_tint = components.drawable.blending.screen_tint.clamp(Vec3::ZERO, Vec3::ONE);
		let mvp = self.mvp_cam * *components.transform;

		self.ops.borrow_mut().push(StageDrawOp::DrawPart(PartDraw {
			index_offset: render_ctx.index_offset,
			index_len: render_ctx.index_len,
			tex_albedo: components.texture.tex_albedo.0,
			mvp,
			offset: components.mesh.origin,
			opacity,
			tint,
			screen_tint,
			blend_mode: components.drawable.blending.mode,
			as_mask,
		}));
	}

	fn begin_composite_content(
		&self,
		_as_mask: bool,
		_components: &CompositeComponents,
		_render_ctx: &CompositeRenderCtx,
		id: InoxNodeUuid,
	) {
		let enabled = self.puppet.nodes.is_effectively_enabled(id);
		if !enabled {
			*self.skip_composite.borrow_mut() = true;
			return;
		}
		*self.skip_composite.borrow_mut() = false;
		self.ops.borrow_mut().push(StageDrawOp::BeginComposite);
	}

	fn finish_composite_content(
		&self,
		as_mask: bool,
		components: &CompositeComponents,
		_render_ctx: &CompositeRenderCtx,
		_id: InoxNodeUuid,
	) {
		if *self.skip_composite.borrow() {
			*self.skip_composite.borrow_mut() = false;
			return;
		}
		let opacity = components.opacity.clamp(0.0, 1.0);
		let tint = components.drawable.blending.tint.clamp(Vec3::ZERO, Vec3::ONE);
		let screen_tint = components.drawable.blending.screen_tint.clamp(Vec3::ZERO, Vec3::ONE);
		self.ops.borrow_mut().push(StageDrawOp::FinishComposite {
			as_mask,
			opacity,
			tint,
			screen_tint,
			blend_mode: components.drawable.blending.mode,
		});
	}
}

/// Extract static rest-pose vertex buffers (`verts`, `uvs`, `indices`) from an initialized puppet.
pub fn extract_stage_geometry(puppet: &Puppet) -> Option<StageGeometry> {
	let prctx = puppet.render_ctx.as_ref()?;
	Some(StageGeometry {
		verts: prctx.vertex_buffers.verts.clone(),
		uvs: prctx.vertex_buffers.uvs.clone(),
		indices: prctx.vertex_buffers.indices.clone(),
	})
}

/// Record the ordered GPU draw commands and current frame vertex deforms for `puppet`.
pub fn record_stage_frame(puppet: &Puppet, mvp_cam: Mat4, wireframe: bool) -> Option<StageFrame> {
	let prctx = puppet.render_ctx.as_ref()?;
	let recorder = DrawRecorder {
		puppet,
		mvp_cam,
		ops: RefCell::new(Vec::with_capacity(prctx.root_drawables_zsorted.len() * 2 + 16)),
		skip_composite: RefCell::new(false),
	};
	recorder.draw(puppet);
	Some(StageFrame {
		ops: recorder.ops.into_inner(),
		deforms: prctx.vertex_buffers.deforms.clone(),
		wireframe,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use glam::vec2;
	use riglens_core::math::transform::TransformOffset;
	use riglens_core::node::components::{Blending, Composite, Drawable, Mesh, TexturedMesh};
	use riglens_core::node::InoxNode;
	use riglens_core::texture::TextureId;

	fn make_quad_mesh() -> Mesh {
		Mesh {
			vertices: vec![vec2(-10.0, -10.0), vec2(10.0, -10.0), vec2(10.0, 10.0), vec2(-10.0, 10.0)],
			uvs: vec![vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0)],
			indices: vec![0, 1, 2, 0, 2, 3],
			origin: Vec2::ZERO,
		}
	}

	#[test]
	fn recorder_emits_mask_dodge_and_composite_sequence() {
		let mut puppet = Puppet::new_empty("RecorderTest");
		let root = puppet.nodes.root_node_id;

		let mask_id = puppet.next_node_uuid();
		puppet.add_node(
			root,
			InoxNode {
				uuid: mask_id,
				name: "MaskSrc".to_owned(),
				enabled: true,
				zsort: 10.0,
				trans_offset: TransformOffset::default(),
				lock_to_root: false,
			},
		);
		puppet.node_comps.add(
			mask_id,
			Drawable {
				blending: Blending {
					mode: BlendMode::Normal,
					tint: Vec3::ONE,
					screen_tint: Vec3::ZERO,
					opacity: 1.0,
				},
				masks: None,
			},
		);
		puppet.node_comps.add(
			mask_id,
			TexturedMesh {
				tex_albedo: TextureId(0),
				tex_emissive: TextureId(0),
				tex_bumpmap: TextureId(0),
			},
		);
		puppet.node_comps.add(mask_id, make_quad_mesh());

		let comp_id = puppet.next_node_uuid();
		puppet.add_node(
			root,
			InoxNode {
				uuid: comp_id,
				name: "MaskedComp".to_owned(),
				enabled: true,
				zsort: 0.0,
				trans_offset: TransformOffset::default(),
				lock_to_root: false,
			},
		);
		puppet.node_comps.add(
			comp_id,
			Drawable {
				blending: Blending {
					mode: BlendMode::Multiply,
					tint: Vec3::ONE,
					screen_tint: Vec3::ZERO,
					opacity: 0.8,
				},
				masks: Some(Masks {
					threshold: 0.25,
					masks: vec![Mask {
						source: mask_id,
						mode: MaskMode::Dodge,
					}],
				}),
			},
		);
		puppet.node_comps.add(comp_id, Composite {});

		let child_id = puppet.next_node_uuid();
		puppet.add_node(
			comp_id,
			InoxNode {
				uuid: child_id,
				name: "CompChild".to_owned(),
				enabled: true,
				zsort: 0.0,
				trans_offset: TransformOffset::default(),
				lock_to_root: false,
			},
		);
		puppet.node_comps.add(
			child_id,
			Drawable {
				blending: Blending {
					mode: BlendMode::Normal,
					tint: Vec3::ONE,
					screen_tint: Vec3::ZERO,
					opacity: 1.0,
				},
				masks: None,
			},
		);
		puppet.node_comps.add(
			child_id,
			TexturedMesh {
				tex_albedo: TextureId(0),
				tex_emissive: TextureId(0),
				tex_bumpmap: TextureId(0),
			},
		);
		puppet.node_comps.add(child_id, make_quad_mesh());

		puppet.init_all();
		puppet.begin_frame();
		puppet.end_frame(0.0);

		let frame = record_stage_frame(&puppet, Mat4::IDENTITY, false).expect("record frame");
		// Expect: MaskSrc standalone draw, then BeginMasks -> BeginMask(Dodge) -> DrawPart(as_mask) ->
		// BeginMaskedContent -> BeginComposite -> DrawPart(child) -> FinishComposite -> EndMask.
		assert!(matches!(frame.ops[0], StageDrawOp::DrawPart(ref p) if !p.as_mask));
		assert!(matches!(
			frame.ops[1],
			StageDrawOp::BeginMasks {
				has_normal_masks: false,
				..
			}
		));
		assert!(matches!(
			frame.ops[2],
			StageDrawOp::BeginMask {
				mode: MaskMode::Dodge
			}
		));
		assert!(matches!(frame.ops[3], StageDrawOp::DrawPart(ref p) if p.as_mask));
		assert!(matches!(frame.ops[4], StageDrawOp::BeginMaskedContent));
		assert!(matches!(frame.ops[5], StageDrawOp::BeginComposite));
		assert!(matches!(frame.ops[6], StageDrawOp::DrawPart(ref p) if !p.as_mask));
		assert!(matches!(
			frame.ops[7],
			StageDrawOp::FinishComposite {
				as_mask: false,
				blend_mode: BlendMode::Multiply,
				..
			}
		));
		assert!(matches!(frame.ops[8], StageDrawOp::EndMask));
	}
}

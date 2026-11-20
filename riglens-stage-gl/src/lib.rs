//! Native OpenGL (`egui_glow`) stage renderer for Inox2D puppets with
//! offscreen FBO `Composite` isolation, 8-bit `Stencil` masking (`Mask` & `Dodge`),
//! and full 19-mode Porter-Duff blending.

pub mod gl_renderer;
pub mod recorder;
mod shaders;

pub use gl_renderer::{make_paint_callback, SharedGlStage, StageGlRenderer, StageTextureInput};
pub use recorder::{
	extract_stage_geometry, record_stage_frame, PartDraw, StageDrawOp, StageFrame, StageGeometry,
};

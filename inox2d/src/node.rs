pub mod components;
pub mod drawables;

use crate::math::transform::TransformOffset;

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
#[repr(transparent)]
pub struct InoxNodeUuid(pub u32);

#[derive(Clone, Debug)]
pub struct InoxNode {
	pub uuid: InoxNodeUuid,
	pub name: String,
	pub enabled: bool,
	pub zsort: f32,
	pub trans_offset: TransformOffset,
	pub lock_to_root: bool,
}

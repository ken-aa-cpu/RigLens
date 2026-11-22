use std::collections::HashMap;

use crate::node::components::{Drawable, OpacityStore, TransformStore, ZSort};
use crate::node::InoxNodeUuid;

use super::{InoxNodeTree, Puppet, World};

pub struct TransformCtx {
	inherited_group_opacities: HashMap<InoxNodeUuid, f32>,
}

impl TransformCtx {
	/// Give every node a `TransformStore` and a `ZSort` component, if the puppet is going to be rendered/animated
	pub fn new(puppet: &mut Puppet) -> Self {
		for node in puppet.nodes.iter() {
			puppet.node_comps.set(node.uuid, TransformStore::default());
			puppet.node_comps.set(node.uuid, ZSort::default());
			puppet.node_comps.set(node.uuid, OpacityStore(1.0));
		}
		TransformCtx {
			inherited_group_opacities: HashMap::new(),
		}
	}

	/// Reset all transform/zsort/opacity values to default.
	pub fn reset(&mut self, nodes: &InoxNodeTree, comps: &mut World) {
		for node in nodes.iter() {
			comps.get_mut::<TransformStore>(node.uuid).unwrap().relative = node.trans_offset.clone();
			comps.get_mut::<ZSort>(node.uuid).unwrap().0 = node.zsort;
			let base_op = comps.get::<Drawable>(node.uuid).map(|d| d.blending.opacity).unwrap_or(1.0);
			comps.get_mut::<OpacityStore>(node.uuid).unwrap().0 = base_op;
		}
	}

	/// Update the puppet's nodes' absolute transforms, by combining transforms
	/// from each node's ancestors in a pre-order traversal manner.
	pub(crate) fn update(&mut self, nodes: &InoxNodeTree, comps: &mut World) {
		let root_trans_store = comps.get_mut::<TransformStore>(nodes.root_node_id).unwrap();
		// The root's absolute transform is its relative transform.
		let root_trans = root_trans_store.relative.to_matrix();
		root_trans_store.absolute = root_trans;

		let root_zsort = comps.get_mut::<ZSort>(nodes.root_node_id).unwrap().0;
		let root_group_opacity = if comps.get::<Drawable>(nodes.root_node_id).is_none() {
			comps.get::<OpacityStore>(nodes.root_node_id).map(|o| o.0).unwrap_or(1.0).clamp(0.0, 1.0)
		} else {
			1.0
		};
		self.inherited_group_opacities.clear();
		self.inherited_group_opacities.insert(nodes.root_node_id, root_group_opacity);

		// Pre-order traversal, just the order to ensure that parents are accessed earlier than children
		// Skip the root
		for node in nodes.pre_order_iter().skip(1) {
			let base = if node.lock_to_root {
				(root_trans, root_zsort, root_group_opacity)
			} else {
				let parent = nodes.get_parent(node.uuid);
				let inherited_op = self
					.inherited_group_opacities
					.get(&parent.uuid)
					.copied()
					.unwrap_or(1.0);
				(
					comps.get_mut::<TransformStore>(parent.uuid).unwrap().absolute,
					comps.get_mut::<ZSort>(parent.uuid).unwrap().0,
					inherited_op,
				)
			};

			let node_trans_store = comps.get_mut::<TransformStore>(node.uuid).unwrap();
			let node_trans = node_trans_store.relative.to_matrix();
			node_trans_store.absolute = base.0 * node_trans;

			let node_zsort = comps.get_mut::<ZSort>(node.uuid).unwrap();
			node_zsort.0 += base.1;

			if let Some(node_op) = comps.get_mut::<OpacityStore>(node.uuid) {
				node_op.0 = (node_op.0 * base.2).clamp(0.0, 1.0);
			}

			// Propagate effective group opacity to children: non-Drawable groups combine their
			// own OpacityStore, while Drawables keep only the ancestor group opacity so their
			// own blending opacity is never applied twice onto child Drawables.
			let child_inherited = if comps.get::<Drawable>(node.uuid).is_none() {
				comps
					.get::<OpacityStore>(node.uuid)
					.map(|o| o.0)
					.unwrap_or(base.2)
					.clamp(0.0, 1.0)
			} else {
				base.2
			};
			self.inherited_group_opacities.insert(node.uuid, child_inherited);
		}
	}
}

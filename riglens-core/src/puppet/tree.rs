use std::collections::HashMap;

use indextree::Arena;

use crate::node::{InoxNode, InoxNodeUuid};

pub struct InoxNodeTree {
	// make this public, instead of replicating all node methods for root, now that callers have the root id
	pub root_node_id: InoxNodeUuid,
	arena: Arena<InoxNode>,
	node_ids: HashMap<InoxNodeUuid, indextree::NodeId>,
}

impl InoxNodeTree {
	pub fn new_with_root(node: InoxNode) -> Self {
		let id = node.uuid;
		let mut node_ids = HashMap::new();
		let mut arena = Arena::new();

		let root_id = arena.new_node(node);
		node_ids.insert(id, root_id);

		Self {
			root_node_id: id,
			arena,
			node_ids,
		}
	}

	pub fn add(&mut self, parent: InoxNodeUuid, id: InoxNodeUuid, node: InoxNode) {
		let parent_id = self.node_ids.get(&parent).expect("parent should be added earlier");

		let node_id = self.arena.new_node(node);
		parent_id.append(node_id, &mut self.arena);

		let result = self.node_ids.insert(id, node_id);
		if result.is_some() {
			panic!("duplicate inox node uuid")
		}
	}

	fn get_internal_node(&self, id: InoxNodeUuid) -> Option<&indextree::Node<InoxNode>> {
		self.arena.get(*self.node_ids.get(&id)?)
	}

	fn get_internal_node_mut(&mut self, id: InoxNodeUuid) -> Option<&mut indextree::Node<InoxNode>> {
		self.arena.get_mut(*self.node_ids.get(&id)?)
	}

	pub fn get_node(&self, id: InoxNodeUuid) -> Option<&InoxNode> {
		Some(self.get_internal_node(id)?.get())
	}

	pub fn get_node_mut(&mut self, id: InoxNodeUuid) -> Option<&mut InoxNode> {
		Some(self.get_internal_node_mut(id)?.get_mut())
	}

	/// order is not guaranteed. use pre_order_iter() for pre-order traversal
	pub fn iter(&self) -> impl Iterator<Item = &InoxNode> {
		self.arena.iter().filter(|n| !n.is_removed()).map(|n| n.get())
	}

	pub fn is_descendant(&self, node: InoxNodeUuid, potential_ancestor: InoxNodeUuid) -> bool {
		let Some(&node_id) = self.node_ids.get(&node) else {
			return false;
		};
		let Some(&anc_id) = self.node_ids.get(&potential_ancestor) else {
			return false;
		};
		anc_id.descendants(&self.arena).any(|id| id == node_id)
	}

	pub fn reparent(&mut self, node: InoxNodeUuid, new_parent: InoxNodeUuid) -> Result<(), String> {
		if node == self.root_node_id {
			return Err("Cannot reparent the Root node".to_owned());
		}
		if node == new_parent || self.is_descendant(new_parent, node) {
			return Err("Cannot reparent a node into itself or one of its descendants".to_owned());
		}
		let node_id = *self
			.node_ids
			.get(&node)
			.ok_or_else(|| format!("Node {} not found", node.0))?;
		let parent_id = *self
			.node_ids
			.get(&new_parent)
			.ok_or_else(|| format!("Parent node {} not found", new_parent.0))?;
		node_id.detach(&mut self.arena);
		parent_id.append(node_id, &mut self.arena);
		Ok(())
	}

	pub fn remove_subtree(&mut self, node: InoxNodeUuid) -> Result<Vec<InoxNodeUuid>, String> {
		if node == self.root_node_id {
			return Err("Cannot remove the Root node".to_owned());
		}
		let node_id = *self
			.node_ids
			.get(&node)
			.ok_or_else(|| format!("Node {} not found", node.0))?;
		let removed_uuids: Vec<InoxNodeUuid> = node_id
			.descendants(&self.arena)
			.map(|id| self.arena.get(id).unwrap().get().uuid)
			.collect();
		node_id.remove_subtree(&mut self.arena);
		for uuid in &removed_uuids {
			self.node_ids.remove(uuid);
		}
		Ok(removed_uuids)
	}

	pub fn pre_order_iter(&self) -> impl Iterator<Item = &InoxNode> {
		let root_id = self.node_ids.get(&self.root_node_id).unwrap();
		root_id
			.descendants(&self.arena)
			.map(|id| self.arena.get(id).unwrap().get())
	}

	/// WARNING: panicks if called on root
	pub fn get_parent(&self, children: InoxNodeUuid) -> &InoxNode {
		self.arena
			.get(
				self.arena
					.get(*self.node_ids.get(&children).unwrap())
					.unwrap()
					.parent()
					.unwrap(),
			)
			.unwrap()
			.get()
	}

	pub fn get_children(&self, parent: InoxNodeUuid) -> impl Iterator<Item = &InoxNode> {
		self.node_ids
			.get(&parent)
			.unwrap()
			.children(&self.arena)
			.map(|id| self.arena.get(id).unwrap().get())
	}

	/// Returns `true` only if `id` exists and every node on its ancestor chain
	/// (including `id` itself and root) has `enabled == true`.
	pub fn is_effectively_enabled(&self, id: InoxNodeUuid) -> bool {
		let Some(&start_id) = self.node_ids.get(&id) else {
			return false;
		};
		let mut cur = Some(start_id);
		let mut depth = 0usize;
		while let Some(nid) = cur {
			let Some(node) = self.arena.get(nid) else {
				return false;
			};
			if !node.get().enabled {
				return false;
			}
			cur = node.parent();
			depth += 1;
			if depth > self.node_ids.len().saturating_add(1) {
				return false;
			}
		}
		true
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::math::transform::TransformOffset;

	#[test]
	fn effectively_enabled_inherits_disabled_parent() {
		let root = InoxNode {
			uuid: InoxNodeUuid(1),
			name: "Root".to_owned(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset::default(),
			lock_to_root: false,
		};
		let mut tree = InoxNodeTree::new_with_root(root);
		tree.add(
			InoxNodeUuid(1),
			InoxNodeUuid(2),
			InoxNode {
				uuid: InoxNodeUuid(2),
				name: "HiddenGroup".to_owned(),
				enabled: false,
				zsort: 0.0,
				trans_offset: TransformOffset::default(),
				lock_to_root: false,
			},
		);
		tree.add(
			InoxNodeUuid(2),
			InoxNodeUuid(3),
			InoxNode {
				uuid: InoxNodeUuid(3),
				name: "ChildPart".to_owned(),
				enabled: true,
				zsort: 0.0,
				trans_offset: TransformOffset::default(),
				lock_to_root: false,
			},
		);
		assert!(tree.is_effectively_enabled(InoxNodeUuid(1)));
		assert!(!tree.is_effectively_enabled(InoxNodeUuid(2)));
		assert!(!tree.is_effectively_enabled(InoxNodeUuid(3)));
		assert!(!tree.is_effectively_enabled(InoxNodeUuid(999)));
	}
}

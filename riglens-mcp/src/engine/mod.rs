mod cmd_nodes;
mod cmd_puppet;
mod cmd_render_booster;
mod cmd_rig;
mod registry;

pub use registry::{command_registry, CommandSpec};

use std::collections::HashMap;

use glam::Vec2;
use riglens_core::model::{Model, ModelTexture, VendorData};
use riglens_core::node::components::{
	Composite, GridDeformer, Mesh, MeshGroup, PathDeformer, RawNodeData, SimplePhysics, TexturedMesh,
};
use riglens_core::node::InoxNodeUuid;
use riglens_core::puppet::Puppet;
use json::JsonValue;
use riglens_booster::pipeline::BoosterController;
use riglens_format::{Diagnostic, FormatCapabilities, Severity};

use crate::rig::build_sample_vtuber_model;

#[derive(Clone)]
pub struct ModelSnapshot {
	pub puppet_json: JsonValue,
	pub textures: Vec<ModelTexture>,
	pub vendors: Vec<VendorData>,
	pub param_overrides: HashMap<String, Vec2>,
}

pub struct Session {
	pub model: Model,
	pub param_overrides: HashMap<String, Vec2>,
	pub revision: u64,
	pub undo_stack: Vec<ModelSnapshot>,
	pub redo_stack: Vec<ModelSnapshot>,
	pub checkpoints: HashMap<String, ModelSnapshot>,
	pub booster: BoosterController,
}

impl Session {
	pub fn new(model: Model) -> Self {
		Self {
			model,
			param_overrides: HashMap::new(),
			revision: 0,
			undo_stack: Vec::new(),
			redo_stack: Vec::new(),
			checkpoints: HashMap::new(),
			booster: BoosterController::default(),
		}
	}

	pub fn new_sample() -> Self {
		Self::new(build_sample_vtuber_model())
	}

	pub fn snapshot(&self) -> ModelSnapshot {
		ModelSnapshot {
			puppet_json: self.model.puppet.to_json(),
			textures: self.model.textures.clone(),
			vendors: self.model.vendors.clone(),
			param_overrides: self.param_overrides.clone(),
		}
	}

	pub fn restore_snapshot(&mut self, snap: &ModelSnapshot) -> Result<(), String> {
		let puppet = Puppet::new_from_json(&snap.puppet_json)
			.map_err(|e| format!("Failed to restore snapshot: {e}"))?;
		self.model = Model {
			puppet,
			textures: snap.textures.clone(),
			vendors: snap.vendors.clone(),
		};
		self.param_overrides = snap.param_overrides.clone();
		Ok(())
	}

	pub fn resolve_node(&self, params: &JsonValue, uuid_key: &str, name_key: &str) -> Result<InoxNodeUuid, String> {
		if let Some(id) = params[uuid_key].as_u32() {
			if self.model.puppet.nodes.get_node(InoxNodeUuid(id)).is_some() {
				return Ok(InoxNodeUuid(id));
			}
			return Err(format!("Node UUID {id} not found"));
		}
		if let Some(name) = params[name_key].as_str() {
			for node in self.model.puppet.nodes.iter() {
				if node.name == name {
					return Ok(node.uuid);
				}
			}
			return Err(format!("Node '{name}' not found"));
		}
		Err(format!("Provide either '{uuid_key}' or '{name_key}'"))
	}

	pub(super) fn resolve_node_by_ref(&self, val: &JsonValue) -> Result<InoxNodeUuid, String> {
		if let Some(id) = val.as_u32() {
			if self.model.puppet.nodes.get_node(InoxNodeUuid(id)).is_some() {
				return Ok(InoxNodeUuid(id));
			}
			return Err(format!("Node UUID {id} not found"));
		}
		if let Some(name) = val.as_str() {
			for node in self.model.puppet.nodes.iter() {
				if node.name == name {
					return Ok(node.uuid);
				}
			}
			return Err(format!("Node '{name}' not found"));
		}
		Err("Expected node name string or numeric UUID".to_owned())
	}

	fn is_mutating_cmd(cmd: &str) -> bool {
		!matches!(
			cmd,
			"puppet.info"
				| "puppet.diagnostics"
				| "puppet.save"
				| "puppet.export_psd"
				| "puppet.export"
				| "format.list"
				| "photocraft.inspect"
				| "photocraft.convert"
				| "rig.analyze_anatomy"
				| "rig.audit"
				| "render.frame"
				| "render.grid"
				| "render.anim_sheet"
				| "render.diagnostic"
				| "render.diff"
				| "render.gif"
				| "craft.figcraft_card"
				| "edit.undo"
				| "edit.redo"
				| "session.checkpoint"
				| "session.restore"
				| "session.batch"
				| "booster.status"
				| "booster.list_spout"
				| "booster.configure"
				| "booster.start"
				| "booster.stop"
		)
	}

	pub fn exec(&mut self, cmd: &str, params: &JsonValue) -> Result<JsonValue, String> {
		if cmd == "edit.undo" {
			let Some(prev) = self.undo_stack.pop() else {
				return Err("Nothing to undo".to_owned());
			};
			let current = self.snapshot();
			self.restore_snapshot(&prev)?;
			self.redo_stack.push(current);
			self.revision += 1;
			return Ok(json::object! {
				undone: true,
				revision: self.revision,
				undo_remaining: self.undo_stack.len(),
				redo_available: self.redo_stack.len()
			});
		}
		if cmd == "edit.redo" {
			let Some(next) = self.redo_stack.pop() else {
				return Err("Nothing to redo".to_owned());
			};
			let current = self.snapshot();
			self.restore_snapshot(&next)?;
			self.undo_stack.push(current);
			self.revision += 1;
			return Ok(json::object! {
				redone: true,
				revision: self.revision,
				undo_remaining: self.undo_stack.len(),
				redo_available: self.redo_stack.len()
			});
		}
		if cmd == "session.checkpoint" {
			let name = params["name"]
				.as_str()
				.ok_or_else(|| "Missing 'name'".to_owned())?;
			self.checkpoints.insert(name.to_owned(), self.snapshot());
			return Ok(json::object! {
				checkpoint: name,
				revision: self.revision,
				total_checkpoints: self.checkpoints.len()
			});
		}
		if cmd == "session.restore" {
			let name = params["name"]
				.as_str()
				.ok_or_else(|| "Missing 'name'".to_owned())?;
			let snap = self
				.checkpoints
				.get(name)
				.cloned()
				.ok_or_else(|| format!("Checkpoint '{name}' not found"))?;
			let current = self.snapshot();
			self.restore_snapshot(&snap)?;
			self.undo_stack.push(current);
			self.redo_stack.clear();
			self.revision += 1;
			return Ok(json::object! {
				restored: name,
				revision: self.revision
			});
		}
		if cmd == "session.batch" {
			let items = match &params["commands"] {
				JsonValue::Array(arr) => arr.clone(),
				_ => return Err("Missing 'commands' array".to_owned()),
			};
			let pre_batch = self.snapshot();
			let mut results = Vec::with_capacity(items.len());
			for (idx, item) in items.iter().enumerate() {
				let sub_cmd = item["cmd"]
					.as_str()
					.ok_or_else(|| format!("commands[{idx}] missing 'cmd'"))?;
				let sub_params = &item["params"];
				match self.exec_inner(sub_cmd, sub_params) {
					Ok(r) => results.push(json::object! { cmd: sub_cmd, ok: true, result: r }),
					Err(e) => {
						let _ = self.restore_snapshot(&pre_batch);
						return Err(format!("Batch aborted at step {idx} ({sub_cmd}): {e}"));
					}
				}
			}
			self.undo_stack.push(pre_batch);
			if self.undo_stack.len() > 40 {
				self.undo_stack.remove(0);
			}
			self.redo_stack.clear();
			return Ok(json::object! {
				executed: results.len(),
				revision: self.revision,
				results: results
			});
		}

		let snap = if Self::is_mutating_cmd(cmd) && params["push_undo"].as_bool().unwrap_or(true) {
			Some(self.snapshot())
		} else {
			None
		};
		let res = self.exec_inner(cmd, params)?;
		if let Some(s) = snap {
			self.undo_stack.push(s);
			if self.undo_stack.len() > 40 {
				self.undo_stack.remove(0);
			}
			self.redo_stack.clear();
		}
		Ok(res)
	}

	fn exec_inner(&mut self, cmd: &str, params: &JsonValue) -> Result<JsonValue, String> {
		match cmd.split('.').next().unwrap_or("") {
			"puppet" | "format" | "photocraft" => self.exec_puppet_cmd(cmd, params),
			"node" | "mesh" => self.exec_node_mesh_cmd(cmd, params),
			"param" | "rig" | "deform" | "physics" | "anim" => self.exec_rig_cmd(cmd, params),
			"render" | "craft" | "booster" => self.exec_render_booster_cmd(cmd, params),
			_ => Err(format!("Unknown command: '{cmd}'")),
		}
	}

	pub fn cmd_info(&self) -> JsonValue {
		let nodes: Vec<JsonValue> = self
			.model
			.puppet
			.nodes
			.pre_order_iter()
			.map(|n| {
				let comps = &self.model.puppet.node_comps;
				let kind: &str = if let Some(raw) = comps.get::<RawNodeData>(n.uuid) {
					raw.type_name.as_str()
				} else if comps.get::<TexturedMesh>(n.uuid).is_some() {
					"Part"
				} else if comps.get::<MeshGroup>(n.uuid).is_some() {
					"MeshGroup"
				} else if comps.get::<GridDeformer>(n.uuid).is_some() {
					"GridDeformer"
				} else if comps.get::<PathDeformer>(n.uuid).is_some() {
					"PathDeformer"
				} else if comps.get::<Composite>(n.uuid).is_some() {
					"Composite"
				} else if comps.get::<SimplePhysics>(n.uuid).is_some() {
					"SimplePhysics"
				} else {
					"Node"
				};
				let (verts, tris) = comps
					.get::<Mesh>(n.uuid)
					.map(|m| (m.vertices.len(), m.indices.len() / 3))
					.or_else(|| comps.get::<GridDeformer>(n.uuid).map(|g| (g.vertex_count(), 0)))
					.or_else(|| comps.get::<PathDeformer>(n.uuid).map(|p| (p.vertex_count(), 0)))
					.unwrap_or((0, 0));
				json::object! {
					uuid: n.uuid.0,
					name: n.name.clone(),
					type: kind,
					enabled: n.enabled,
					zsort: n.zsort,
					vertices: verts,
					triangles: tris
				}
			})
			.collect();

		let mut params_list: Vec<_> = self.model.puppet.params.values().collect();
		params_list.sort_by_key(|p| p.uuid);
		let params_json: Vec<JsonValue> = params_list
			.into_iter()
			.map(|p| {
				json::object! {
					uuid: p.uuid.0,
					name: p.name.clone(),
					is_vec2: p.is_vec2,
					min: [p.min.x, p.min.y],
					max: [p.max.x, p.max.y],
					defaults: [p.defaults.x, p.defaults.y],
					bindings: p.bindings.len()
				}
			})
			.collect();

		let mut anim_names: Vec<_> = self.model.puppet.animations.keys().cloned().collect();
		anim_names.sort();

		json::object! {
			name: self.model.puppet.meta.name.clone().unwrap_or_default(),
			version: self.model.puppet.meta.version.clone(),
			textures: self.model.textures.len(),
			node_count: nodes.len(),
			nodes: nodes,
			params: params_json,
			animations: anim_names,
			warnings: self.model.puppet.diagnostics.clone()
		}
	}
}

pub fn caps_list_to_json(caps: &[FormatCapabilities]) -> JsonValue {
	JsonValue::Array(
		caps.iter()
			.map(|c| {
				json::object! {
					id: c.id.clone(),
					display_name: c.display_name.clone(),
					extensions: c.extensions.clone(),
					supports_grid_deformer: c.supports_grid_deformer,
					supports_path_deformer: c.supports_path_deformer,
					supports_cubic_interpolation: c.supports_cubic_interpolation,
					supports_animations: c.supports_animations,
					supports_opaque_passthrough: c.supports_opaque_passthrough,
					supports_textures: c.supports_textures,
					supports_vendor_data: c.supports_vendor_data,
				}
			})
			.collect(),
	)
}

pub(super) fn diagnostics_to_json(diags: &[Diagnostic]) -> JsonValue {
	JsonValue::Array(
		diags
			.iter()
			.map(|d| {
				json::object! {
					severity: match d.severity {
						Severity::Info => "info",
						Severity::Warning => "warning",
					},
					code: d.code.clone(),
					node_id: d.node_id,
					message: d.message.clone(),
				}
			})
			.collect(),
	)
}

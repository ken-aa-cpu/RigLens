use inox2d_mcp::engine::{command_registry, Session};
use json::JsonValue;

fn tool_name_to_cmd(name: &str) -> String {
	command_registry()
		.iter()
		.find(|spec| cmd_to_tool_name(spec.id) == name)
		.map(|spec| spec.id.to_owned())
		.unwrap_or_else(|| name.to_owned())
}

fn cmd_to_tool_name(cmd: &str) -> String {
	cmd.replace('.', "_")
}

fn mcp_tools_list() -> JsonValue {
	let tools: Vec<JsonValue> = command_registry()
		.iter()
		.map(|spec| {
			let mut props = json::object::Object::new();
			if spec.id == "puppet.save" {
				props.insert("path", json::object! { type: "string", description: "Output .inp path" });
			} else if spec.id == "puppet.import_psd" {
				props.insert("path", json::object! { type: "string", description: "Input .psd path" });
				props.insert("name", json::object! { type: "string" });
				props.insert("mesh_mode", json::object! { type: "string", description: "delaunay | contour | grid" });
				props.insert("contour_mesh", json::object! { type: "boolean" });
				props.insert("auto_rig", json::object! { type: "boolean" });
				props.insert("cols", json::object! { type: "integer" });
				props.insert("rows", json::object! { type: "integer" });
			} else if spec.id == "puppet.export_psd" {
				props.insert("path", json::object! { type: "string", description: "Output .psd path" });
				props.insert("width", json::object! { type: "integer" });
				props.insert("height", json::object! { type: "integer" });
			} else if spec.id == "puppet.export" {
				props.insert("path", json::object! { type: "string", description: "Output file path" });
				props.insert("format", json::object! { type: "string", description: "Format id or extension (inp, rig-json, psd, ...)" });
				props.insert("strict", json::object! { type: "boolean", description: "Reject export if target format drops features" });
				props.insert("pretty", json::object! { type: "boolean" });
				props.insert("width", json::object! { type: "integer" });
				props.insert("height", json::object! { type: "integer" });
			} else if spec.id == "puppet.import" {
				props.insert("path", json::object! { type: "string", description: "Input file path" });
				props.insert("format", json::object! { type: "string", description: "Format id or extension (inp, rig-json, psd, ...)" });
			} else if spec.id == "photocraft.inspect" {
				props.insert("path", json::object! { type: "string" });
			} else if spec.id == "photocraft.convert" {
				props.insert("in_path", json::object! { type: "string" });
				props.insert("out_path", json::object! { type: "string" });
			} else if spec.id == "render.frame" {
				props.insert("out", json::object! { type: "string", description: "Output PNG file path" });
				props.insert("width", json::object! { type: "integer" });
				props.insert("height", json::object! { type: "integer" });
				props.insert("wireframe", json::object! { type: "boolean" });
				props.insert("dt", json::object! { type: "number" });
			} else if spec.id == "render.grid" {
				props.insert("out", json::object! { type: "string", description: "Output 3x3 grid PNG path" });
				props.insert("param", json::object! { type: "string", description: "Parameter name to sweep" });
				props.insert("tile_size", json::object! { type: "integer" });
				props.insert("wireframe", json::object! { type: "boolean" });
			} else if spec.id == "render.anim_sheet" {
				props.insert("out", json::object! { type: "string", description: "Output animation sheet PNG path" });
				props.insert("anim", json::object! { type: "string" });
				props.insert("frames", json::object! { type: "integer" });
				props.insert("tile_size", json::object! { type: "integer" });
				props.insert("wireframe", json::object! { type: "boolean" });
			} else if spec.id == "node.add_part" {
				props.insert("name", json::object! { type: "string" });
				props.insert("parent_name", json::object! { type: "string" });
				props.insert("zsort", json::object! { type: "number" });
				props.insert("x", json::object! { type: "number" });
				props.insert("y", json::object! { type: "number" });
				props.insert("width", json::object! { type: "number" });
				props.insert("height", json::object! { type: "number" });
				props.insert("cols", json::object! { type: "integer" });
				props.insert("rows", json::object! { type: "integer" });
				props.insert("image_path", json::object! { type: "string" });
			} else if spec.id == "mesh.auto_grid" || spec.id == "mesh.auto_contour" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("cols", json::object! { type: "integer" });
				props.insert("rows", json::object! { type: "integer" });
				props.insert("alpha_threshold", json::object! { type: "integer" });
			} else if spec.id == "mesh.auto_delaunay" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("width", json::object! { type: "number" });
				props.insert("height", json::object! { type: "number" });
				props.insert("step_px", json::object! { type: "number" });
				props.insert("alpha_threshold", json::object! { type: "integer" });
				props.insert("boundary_offset_px", json::object! { type: "number" });
				props.insert("constrained", json::object! { type: "boolean" });
				props.insert("refine_near_corners", json::object! { type: "boolean" });
				props.insert("refinement_zones", json::object! { type: "array" });
			} else if spec.id == "rig.auto_head_perspective" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("param", json::object! { type: "string" });
				props.insert("depth_z", json::object! { type: "number" });
				props.insert("yaw_px", json::object! { type: "number" });
				props.insert("pitch_px", json::object! { type: "number" });
			} else if spec.id == "rig.auto_blink" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("param", json::object! { type: "string" });
				props.insert("close_y_ratio", json::object! { type: "number" });
			} else if spec.id == "rig.auto_mouth_form" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("param", json::object! { type: "string" });
				props.insert("width_px", json::object! { type: "number" });
				props.insert("smile_y_px", json::object! { type: "number" });
			} else if spec.id == "rig.auto_head_roll" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("param", json::object! { type: "string" });
				props.insert("max_deg", json::object! { type: "number" });
			} else if spec.id == "rig.auto_breath" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("param", json::object! { type: "string" });
				props.insert("scale_amp", json::object! { type: "number" });
				props.insert("y_amp", json::object! { type: "number" });
			} else if spec.id == "rig.bind_opacity" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("param", json::object! { type: "string" });
				props.insert("opacity_at_0", json::object! { type: "number" });
				props.insert("opacity_at_1", json::object! { type: "number" });
			} else if spec.id == "deform.brush" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("param", json::object! { type: "string" });
				props.insert("kx", json::object! { type: "number" });
				props.insert("ky", json::object! { type: "number" });
				props.insert("anchor", json::object! { type: "string" });
				props.insert("radius_ratio", json::object! { type: "number" });
				props.insert("dx", json::object! { type: "number" });
				props.insert("dy", json::object! { type: "number" });
			} else if spec.id == "deform.set_vertices" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("param", json::object! { type: "string" });
				props.insert("kx", json::object! { type: "number" });
				props.insert("ky", json::object! { type: "number" });
				props.insert("indices", json::object! { type: "array" });
				props.insert("dx", json::object! { type: "number" });
				props.insert("dy", json::object! { type: "number" });
				props.insert("reset", json::object! { type: "boolean" });
			} else if spec.id == "mesh.edit_topology" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("action", json::object! { type: "string" });
				props.insert("vertex_index", json::object! { type: "integer" });
				props.insert("indices", json::object! { type: "array" });
				props.insert("x", json::object! { type: "number" });
				props.insert("y", json::object! { type: "number" });
				props.insert("dx", json::object! { type: "number" });
				props.insert("dy", json::object! { type: "number" });
			} else if spec.id == "physics.configure" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("model_type", json::object! { type: "string" });
				props.insert("length", json::object! { type: "number" });
				props.insert("frequency", json::object! { type: "number" });
				props.insert("angle_damping", json::object! { type: "number" });
				props.insert("length_damping", json::object! { type: "number" });
				props.insert("gravity", json::object! { type: "number" });
			} else if spec.id == "anim.set_keyframe" || spec.id == "anim.remove_keyframe" {
				props.insert("anim", json::object! { type: "string" });
				props.insert("param", json::object! { type: "string" });
				props.insert("target", json::object! { type: "integer" });
				props.insert("frame", json::object! { type: "integer" });
				props.insert("value", json::object! { type: "number" });
				props.insert("tension", json::object! { type: "number" });
				props.insert("interpolation", json::object! { type: "string" });
			} else if spec.id == "render.diagnostic" {
				props.insert("out", json::object! { type: "string" });
				props.insert("width", json::object! { type: "integer" });
				props.insert("height", json::object! { type: "integer" });
				props.insert("scale", json::object! { type: "number" });
			} else if spec.id == "render.diff" {
				props.insert("out", json::object! { type: "string" });
				props.insert("checkpoint", json::object! { type: "string" });
				props.insert("tile_size", json::object! { type: "integer" });
				props.insert("scale", json::object! { type: "number" });
			} else if spec.id == "render.gif" {
				props.insert("out", json::object! { type: "string" });
				props.insert("anim", json::object! { type: "string" });
				props.insert("frames", json::object! { type: "integer" });
				props.insert("fps", json::object! { type: "integer" });
				props.insert("width", json::object! { type: "integer" });
				props.insert("height", json::object! { type: "integer" });
				props.insert("scale", json::object! { type: "number" });
				props.insert("wireframe", json::object! { type: "boolean" });
			} else if spec.id == "craft.figcraft_card" {
				props.insert("out", json::object! { type: "string", description: "Output PNG path (.svg written alongside)" });
			} else if spec.id == "session.checkpoint" || spec.id == "session.restore" {
				props.insert("name", json::object! { type: "string" });
			} else if spec.id == "session.batch" {
				props.insert("commands", json::object! { type: "array" });
			} else if spec.id == "node.configure" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("name", json::object! { type: "string" });
				props.insert("parent_name", json::object! { type: "string" });
				props.insert("zsort", json::object! { type: "number" });
				props.insert("enabled", json::object! { type: "boolean" });
				props.insert("x", json::object! { type: "number" });
				props.insert("y", json::object! { type: "number" });
				props.insert("rot_z_deg", json::object! { type: "number" });
				props.insert("opacity", json::object! { type: "number" });
				props.insert("blend_mode", json::object! { type: "string" });
			} else if spec.id == "node.set_mask" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("sources", json::object! { type: "array" });
				props.insert("threshold", json::object! { type: "number" });
			} else if spec.id == "node.add_meshgroup" {
				props.insert("name", json::object! { type: "string" });
				props.insert("parent_name", json::object! { type: "string" });
				props.insert("children", json::object! { type: "array" });
				props.insert("cols", json::object! { type: "integer" });
				props.insert("rows", json::object! { type: "integer" });
				props.insert("padding", json::object! { type: "number" });
			} else if spec.id == "node.remove" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("node_uuid", json::object! { type: "integer" });
			} else if spec.id == "node.synthesize_overlay" {
				props.insert("kind", json::object! { type: "string" });
				props.insert("parent_name", json::object! { type: "string" });
			} else if spec.id == "rig.auto_visemes" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("param", json::object! { type: "string" });
				props.insert("open_px", json::object! { type: "number" });
				props.insert("width_px", json::object! { type: "number" });
			} else if spec.id == "rig.auto_brow" {
				props.insert("node_name", json::object! { type: "string" });
				props.insert("param", json::object! { type: "string" });
				props.insert("is_right", json::object! { type: "boolean" });
			} else if spec.id == "physics.add_pendulum" {
				props.insert("name", json::object! { type: "string" });
				props.insert("parent_name", json::object! { type: "string" });
				props.insert("param", json::object! { type: "string" });
				props.insert("model_type", json::object! { type: "string" });
				props.insert("length", json::object! { type: "number" });
				props.insert("frequency", json::object! { type: "number" });
				props.insert("angle_damping", json::object! { type: "number" });
			} else if spec.id == "param.set" {
				props.insert("param", json::object! { type: "string" });
				props.insert("x", json::object! { type: "number" });
				props.insert("y", json::object! { type: "number" });
			} else if spec.id == "node.add_grid_deformer" {
				props.insert("name", json::object! { type: "string" });
				props.insert("parent_name", json::object! { type: "string" });
				props.insert("children", json::object! { type: "array" });
				props.insert("cols", json::object! { type: "integer" });
				props.insert("rows", json::object! { type: "integer" });
				props.insert("padding", json::object! { type: "number" });
			} else if spec.id == "node.add_path_deformer" {
				props.insert("name", json::object! { type: "string" });
				props.insert("parent_name", json::object! { type: "string" });
				props.insert("children", json::object! { type: "array" });
				props.insert("points", json::object! { type: "integer" });
				props.insert("curve_type", json::object! { type: "string" });
				props.insert("orientation", json::object! { type: "string" });
				props.insert("padding", json::object! { type: "number" });
			} else if spec.id == "param.create" {
				props.insert("name", json::object! { type: "string" });
				props.insert("is_vec2", json::object! { type: "boolean" });
				props.insert("min", json::object! { type: "array" });
				props.insert("max", json::object! { type: "array" });
				props.insert("defaults", json::object! { type: "array" });
				props.insert("axis_x", json::object! { type: "array" });
				props.insert("axis_y", json::object! { type: "array" });
			} else if spec.id == "param.set_binding_keypoint" || spec.id == "param.clear_binding_keypoint" {
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("node_name", json::object! { type: "string" });
				props.insert("param", json::object! { type: "string" });
				props.insert("kind", json::object! { type: "string" });
				props.insert("ix", json::object! { type: "integer" });
				props.insert("iy", json::object! { type: "integer" });
				props.insert("value", json::object! { type: "number" });
			} else if spec.id == "param.remove_binding" {
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("node_name", json::object! { type: "string" });
				props.insert("param", json::object! { type: "string" });
				props.insert("kind", json::object! { type: "string" });
			} else if spec.id == "deform.mirror_keypoint" {
				props.insert("node_uuid", json::object! { type: "integer" });
				props.insert("node_name", json::object! { type: "string" });
				props.insert("param", json::object! { type: "string" });
				props.insert("src_ix", json::object! { type: "integer" });
				props.insert("src_iy", json::object! { type: "integer" });
				props.insert("dst_ix", json::object! { type: "integer" });
				props.insert("dst_iy", json::object! { type: "integer" });
				props.insert("mirror_x", json::object! { type: "boolean" });
			} else if spec.id == "anim.create" {
				props.insert("name", json::object! { type: "string" });
				props.insert("length", json::object! { type: "integer" });
				props.insert("fps", json::object! { type: "number" });
			} else if spec.id == "booster.configure" || spec.id == "booster.start" {
				props.insert("vts_url", json::object! { type: "string" });
				props.insert("enable_vts", json::object! { type: "boolean" });
				props.insert("spout_in", json::object! { type: "string" });
				props.insert("spout_out", json::object! { type: "string" });
				props.insert("tick_hz", json::object! { type: "number" });
				props.insert("wind_turbulence", json::object! { type: "number" });
				props.insert("enable_gpu", json::object! { type: "boolean" });
				props.insert("enable_soft_body", json::object! { type: "boolean" });
				props.insert("rim_intensity", json::object! { type: "number" });
				props.insert("rim_theme", json::object! { type: "string" });
				props.insert("particle_count", json::object! { type: "integer" });
				props.insert("upscale_factor", json::object! { type: "number" });
				props.insert("upscale_sharpness", json::object! { type: "number" });
				props.insert("frame_gen_multiplier", json::object! { type: "integer" });
			}

			json::object! {
				name: cmd_to_tool_name(spec.id),
				description: format!("{} (schema: {})", spec.description, spec.params_schema),
				inputSchema: {
					type: "object",
					properties: JsonValue::Object(props),
					additionalProperties: true
				}
			}
		})
		.collect();

	json::object! { tools: tools }
}

pub(crate) fn handle_mcp_request(session: &mut Session, req: &JsonValue) -> Option<JsonValue> {
	let id = req["id"].clone();
	let method = req["method"].as_str()?;

	if method == "notifications/initialized" || id.is_null() {
		return None;
	}

	let resp = match method {
		"initialize" => json::object! {
			jsonrpc: "2.0",
			id: id,
			result: {
				protocolVersion: "2024-11-05",
				capabilities: { tools: {} },
				serverInfo: {
					name: "inox2d-mcp",
					version: env!("CARGO_PKG_VERSION")
				}
			}
		},
		"ping" => json::object! {
			jsonrpc: "2.0",
			id: id,
			result: {}
		},
		"tools/list" => json::object! {
			jsonrpc: "2.0",
			id: id,
			result: mcp_tools_list()
		},
		"tools/call" => {
			let tool_name = req["params"]["name"].as_str().unwrap_or("");
			let cmd = tool_name_to_cmd(tool_name);
			let args = if req["params"]["arguments"].is_object() {
				req["params"]["arguments"].clone()
			} else {
				JsonValue::new_object()
			};
			match session.exec(&cmd, &args) {
				Ok(res) => {
					let text = json::stringify_pretty(res, 2);
					json::object! {
						jsonrpc: "2.0",
						id: id,
						result: {
							content: [
								{ type: "text", text: text.as_str() }
							]
						}
					}
				}
				Err(err) => json::object! {
					jsonrpc: "2.0",
					id: id,
					result: {
						isError: true,
						content: [
							{ type: "text", text: err.as_str() }
						]
					}
				},
			}
		}
		other => json::object! {
			jsonrpc: "2.0",
			id: id,
			error: {
				code: -32601,
				message: format!("Method not found: {other}")
			}
		},
	};
	Some(resp)
}

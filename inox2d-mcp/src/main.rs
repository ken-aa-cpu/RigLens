#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use glam::{vec2, Vec2};
use inox2d::formats::inp::{parse_inp, write_inp};
use inox2d::formats::ir_bridge::{ir_to_model, model_to_ir};
use inox2d::model::Model;
use inox2d::puppet::Puppet;
use inox2d_mcp::audit::{audit_rig, auto_fix_rig};
use inox2d_mcp::engine::{caps_list_to_json, command_registry, Session};
use inox2d_mcp::formats::default_registry;
use inox2d_mcp::headless_render::{
	render_anim_gif, render_anim_sheet, render_diagnostic, render_frame, render_grid, RenderOptions,
};
use inox2d_mcp::psd::{export_model_to_psd, import_psd_to_model_with_mode, parse_psd_bytes};
use inox2d_mcp::rig::{auto_rig_vtuber, build_sample_vtuber_model};
use json::JsonValue;
use riglens_format::{ExportOptions, ImportOptions};

#[derive(Parser)]
#[command(name = "inox2d-mcp", version, about = "Headless Inochi2D Creator engine, CLI, and MCP server")]
struct Cli {
	#[command(subcommand)]
	command: Commands,
}

#[derive(Subcommand)]
enum Commands {
	/// Print model info (nodes, meshes, parameters, textures) as JSON
	Info {
		/// Path to .inp/.inx file or '--sample'
		input: String,
	},
	/// Create a new .inp model file
	New {
		#[arg(long)]
		out: PathBuf,
		#[arg(long)]
		sample: bool,
		#[arg(long, default_value = "NewPuppet")]
		name: String,
	},
	/// Import a layered .psd file into an .inp puppet (with optional Delaunay/contour meshing & auto-rig)
	ImportPsd {
		psd: PathBuf,
		#[arg(long)]
		out: PathBuf,
		#[arg(long)]
		name: Option<String>,
		#[arg(long)]
		no_contour: bool,
		#[arg(long)]
		delaunay: bool,
		#[arg(long)]
		no_auto_rig: bool,
		#[arg(long, default_value_t = 8)]
		cols: usize,
		#[arg(long, default_value_t = 8)]
		rows: usize,
	},
	/// Export an .inp puppet's Parts into a layered .psd file
	ExportPsd {
		input: String,
		#[arg(long)]
		out: PathBuf,
		#[arg(long, default_value_t = 1024)]
		width: u32,
		#[arg(long, default_value_t = 1024)]
		height: u32,
	},
	/// Run semantic VTuber auto-rigging on an existing .inp model
	AutoRig {
		input: String,
		#[arg(long)]
		out: Option<PathBuf>,
	},
	/// Render a single frame to PNG
	Render {
		input: String,
		#[arg(long)]
		out: PathBuf,
		#[arg(long, default_value_t = 512)]
		width: u32,
		#[arg(long, default_value_t = 512)]
		height: u32,
		#[arg(long, default_value_t = 1.0)]
		scale: f32,
		#[arg(long)]
		wireframe: bool,
		#[arg(long, default_value_t = 0.0)]
		dt: f32,
		/// Parameter override as Key=x,y (or Key=x for 1D)
		#[arg(long = "param")]
		params: Vec<String>,
	},
	/// Render a 3x3 contact sheet sweeping a parameter across min..max
	RenderGrid {
		input: String,
		#[arg(long)]
		param: String,
		#[arg(long)]
		out: PathBuf,
		#[arg(long, default_value_t = 220)]
		tile_size: u32,
		#[arg(long)]
		wireframe: bool,
	},
	/// Render an animation (or physics motion test) into a contact sheet PNG
	RenderAnim {
		input: String,
		#[arg(long)]
		out: PathBuf,
		#[arg(long)]
		anim: Option<String>,
		#[arg(long, default_value_t = 8)]
		frames: u32,
		#[arg(long, default_value_t = 180)]
		tile_size: u32,
		#[arg(long)]
		wireframe: bool,
	},
	/// Run 6-point deterministic geometric & visual rig audit
	Audit {
		input: String,
	},
	/// Auto-fix containment leaks, triangle foldovers, and missing masks
	AutoFix {
		input: String,
		#[arg(long)]
		out: Option<PathBuf>,
	},
	/// Render X-ray diagnostic PNG (coordinate grid, foldover heatmap, Face containment contour)
	Diagnostic {
		input: String,
		#[arg(long)]
		out: PathBuf,
		#[arg(long, default_value_t = 512)]
		width: u32,
		#[arg(long, default_value_t = 512)]
		height: u32,
		#[arg(long, default_value_t = 1.0)]
		scale: f32,
		#[arg(long = "param")]
		params: Vec<String>,
	},
	/// Render a looping animated .gif of an animation or physics motion test
	Gif {
		input: String,
		#[arg(long)]
		out: PathBuf,
		#[arg(long)]
		anim: Option<String>,
		#[arg(long, default_value_t = 24)]
		frames: u32,
		#[arg(long, default_value_t = 15)]
		fps: u32,
		#[arg(long, default_value_t = 320)]
		width: u32,
		#[arg(long, default_value_t = 320)]
		height: u32,
		#[arg(long, default_value_t = 0.85)]
		scale: f32,
		#[arg(long)]
		wireframe: bool,
	},
	/// List all engine commands as JSON
	Commands,
	/// List all registered format importers, exporters, and capabilities as JSON
	Formats,
	/// Export a puppet via the format registry (.inp, .rigjson, .psd, or custom plugins)
	Export {
		/// Input path (.inp, .inx, .rigjson, .psd), or omit when passing --sample
		input: Option<String>,
		#[arg(long)]
		sample: bool,
		#[arg(long)]
		out: PathBuf,
		#[arg(long)]
		format: Option<String>,
		#[arg(long)]
		strict: bool,
		#[arg(long)]
		pretty: bool,
	},
	/// Execute one engine command and optionally save the modified .inp
	Exec {
		input: String,
		cmd: String,
		#[arg(default_value = "{}")]
		params_json: String,
		#[arg(long)]
		save: Option<PathBuf>,
	},
	/// Interactive JSON-lines session on stdin/stdout
	Serve {
		#[arg(default_value = "--sample")]
		input: String,
		/// Auto-save modified .inp atomically whenever the session mutates (for live render-opengl hot-reload)
		#[arg(long)]
		watch: Option<PathBuf>,
	},
	/// Run Model Context Protocol (MCP) JSON-RPC 2.0 server on stdio
	Mcp {
		#[arg(default_value = "--sample")]
		input: String,
		/// Auto-save modified .inp atomically whenever an MCP tool mutates the puppet
		#[arg(long)]
		watch: Option<PathBuf>,
	},
}

fn load_model(input: &str) -> Result<Model, String> {
	if input == "--sample" || input == "sample" {
		return Ok(build_sample_vtuber_model());
	}
	let path = Path::new(input);
	let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
	let file = File::open(path).map_err(|e| format!("Failed to open '{input}': {e}"))?;
	let mut reader = BufReader::new(file);
	if ext.is_empty() || ext.eq_ignore_ascii_case("inp") || ext.eq_ignore_ascii_case("inx") {
		let mut bytes = Vec::new();
		reader
			.read_to_end(&mut bytes)
			.map_err(|e| format!("Failed to read '{input}': {e}"))?;
		return parse_inp(bytes.as_slice()).map_err(|e| format!("Failed to parse '{input}': {e}"));
	}
	let reg = default_registry();
	let report = reg
		.import_from_reader(ext, &mut reader, &ImportOptions::default())
		.map_err(|e| format!("Failed to import '{input}': {e}"))?;
	ir_to_model(&report.document).map_err(|e| format!("Failed to convert '{input}': {e}"))
}

fn parse_param_overrides(items: &[String]) -> Result<HashMap<String, Vec2>, String> {
	let mut map = HashMap::new();
	for item in items {
		let (k, v) = item
			.split_once('=')
			.ok_or_else(|| format!("Invalid --param '{item}', expected Key=x or Key=x,y"))?;
		let (x, y) = if let Some((sx, sy)) = v.split_once(',') {
			(
				sx.trim().parse::<f32>().map_err(|e| format!("Invalid float '{sx}': {e}"))?,
				sy.trim().parse::<f32>().map_err(|e| format!("Invalid float '{sy}': {e}"))?,
			)
		} else {
			(
				v.trim().parse::<f32>().map_err(|e| format!("Invalid float '{v}': {e}"))?,
				0.0,
			)
		};
		map.insert(k.to_owned(), vec2(x, y));
	}
	Ok(map)
}

fn write_inp_atomic(model: &Model, path: &Path) -> Result<(), String> {
	if let Some(parent) = path.parent() {
		if !parent.as_os_str().is_empty() {
			fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir for '{}': {e}", path.display()))?;
		}
	}
	let tmp_path = path.with_extension("inp.tmp");
	{
		let file = File::create(&tmp_path)
			.map_err(|e| format!("Failed to create temp watch file '{}': {e}", tmp_path.display()))?;
		let mut w = BufWriter::new(file);
		write_inp(model, &mut w).map_err(|e| format!("Failed to serialize watch .inp: {e}"))?;
		w.flush().map_err(|e| format!("Failed to flush watch .inp: {e}"))?;
	}
	fs::rename(&tmp_path, path)
		.map_err(|e| format!("Failed to replace watch file '{}': {e}", path.display()))?;
	Ok(())
}

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

fn handle_mcp_request(session: &mut Session, req: &JsonValue) -> Option<JsonValue> {
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

fn run() -> Result<(), String> {
	let cli = Cli::parse();
	match cli.command {
		Commands::Info { input } => {
			let model = load_model(&input)?;
			let session = Session::new(model);
			println!("{}", json::stringify_pretty(session.cmd_info(), 2));
		}
		Commands::New { out, sample, name } => {
			let model = if sample {
				build_sample_vtuber_model()
			} else {
				Model {
					puppet: Puppet::new_empty(name),
					textures: Vec::new(),
					vendors: Vec::new(),
				}
			};
			let file = File::create(&out).map_err(|e| format!("Failed to create {}: {e}", out.display()))?;
			let mut w = BufWriter::new(file);
			write_inp(&model, &mut w).map_err(|e| format!("Failed to write .inp: {e}"))?;
			println!("Created {}", out.display());
		}
		Commands::ImportPsd {
			psd,
			out,
			name,
			no_contour,
			delaunay,
			no_auto_rig,
			cols,
			rows,
		} => {
			let bytes = fs::read(&psd).map_err(|e| format!("Failed to read '{}': {e}", psd.display()))?;
			let doc = parse_psd_bytes(&bytes)?;
			let puppet_name = name.unwrap_or_else(|| {
				psd.file_stem()
					.and_then(|s| s.to_str())
					.unwrap_or("PsdPuppet")
					.to_owned()
			});
			let mesh_mode = if delaunay {
				"delaunay"
			} else if !no_contour {
				"contour"
			} else {
				"grid"
			};
			let model = import_psd_to_model_with_mode(&doc, &puppet_name, mesh_mode, !no_auto_rig, cols, rows);
			let file = File::create(&out).map_err(|e| format!("Failed to create {}: {e}", out.display()))?;
			let mut w = BufWriter::new(file);
			write_inp(&model, &mut w).map_err(|e| format!("Failed to write .inp: {e}"))?;
			println!(
				"Imported PSD {} ({}x{}, {} layers) -> {}",
				psd.display(),
				doc.width,
				doc.height,
				doc.layers.len(),
				out.display()
			);
		}
		Commands::ExportPsd {
			input,
			out,
			width,
			height,
		} => {
			let mut model = load_model(&input)?;
			let psd_bytes = export_model_to_psd(&mut model, width, height)?;
			fs::write(&out, &psd_bytes).map_err(|e| format!("Failed to write {}: {e}", out.display()))?;
			println!("Exported PSD {} ({width}x{height}, {} bytes)", out.display(), psd_bytes.len());
		}
		Commands::AutoRig { input, out } => {
			let mut model = load_model(&input)?;
			let actions = auto_rig_vtuber(&mut model.puppet);
			let out_path = out.unwrap_or_else(|| PathBuf::from(&input));
			let file = File::create(&out_path).map_err(|e| format!("Failed to create {}: {e}", out_path.display()))?;
			let mut w = BufWriter::new(file);
			write_inp(&model, &mut w).map_err(|e| format!("Failed to write .inp: {e}"))?;
			println!("{}", json::stringify_pretty(json::object! { saved: out_path.display().to_string(), actions: actions }, 2));
		}
		Commands::Render {
			input,
			out,
			width,
			height,
			scale,
			wireframe,
			dt,
			params,
		} => {
			let mut model = load_model(&input)?;
			let param_map = parse_param_overrides(&params)?;
			let opts = RenderOptions {
				width,
				height,
				scale,
				wireframe,
				dt,
				..RenderOptions::default()
			};
			let img = render_frame(&mut model, &param_map, &opts)?;
			img.save(&out)
				.map_err(|e| format!("Failed to save '{}': {e}", out.display()))?;
			println!("Rendered {} ({width}x{height})", out.display());
		}
		Commands::RenderGrid {
			input,
			param,
			out,
			tile_size,
			wireframe,
		} => {
			let mut model = load_model(&input)?;
			let img = render_grid(&mut model, &param, tile_size, wireframe, &HashMap::new())?;
			let (w, h) = img.dimensions();
			img.save(&out)
				.map_err(|e| format!("Failed to save '{}': {e}", out.display()))?;
			println!("Rendered 3x3 grid for '{param}' to {} ({w}x{h})", out.display());
		}
		Commands::RenderAnim {
			input,
			out,
			anim,
			frames,
			tile_size,
			wireframe,
		} => {
			let mut model = load_model(&input)?;
			let img = render_anim_sheet(&mut model, anim.as_deref(), frames, tile_size, wireframe)?;
			let (w, h) = img.dimensions();
			img.save(&out)
				.map_err(|e| format!("Failed to save '{}': {e}", out.display()))?;
			println!("Rendered {frames}-frame animation sheet to {} ({w}x{h})", out.display());
		}
		Commands::Audit { input } => {
			let mut model = load_model(&input)?;
			let report = audit_rig(&mut model);
			println!("{}", json::stringify_pretty(report, 2));
		}
		Commands::AutoFix { input, out } => {
			let mut model = load_model(&input)?;
			let fixed = auto_fix_rig(&mut model);
			if let Some(out_path) = out {
				let file = File::create(&out_path)
					.map_err(|e| format!("Failed to create {}: {e}", out_path.display()))?;
				let mut w = BufWriter::new(file);
				write_inp(&model, &mut w).map_err(|e| format!("Failed to write .inp: {e}"))?;
			}
			println!("{}", json::stringify_pretty(fixed, 2));
		}
		Commands::Diagnostic {
			input,
			out,
			width,
			height,
			scale,
			params,
		} => {
			let mut model = load_model(&input)?;
			let param_map = parse_param_overrides(&params)?;
			let img = render_diagnostic(&mut model, &param_map, width, height, scale)?;
			img.save(&out)
				.map_err(|e| format!("Failed to save '{}': {e}", out.display()))?;
			println!("Rendered diagnostic X-ray to {} ({width}x{height})", out.display());
		}
		Commands::Gif {
			input,
			out,
			anim,
			frames,
			fps,
			width,
			height,
			scale,
			wireframe,
		} => {
			let mut model = load_model(&input)?;
			let gif_bytes = render_anim_gif(&mut model, anim.as_deref(), frames, fps, width, height, scale, wireframe)?;
			fs::write(&out, &gif_bytes).map_err(|e| format!("Failed to write '{}': {e}", out.display()))?;
			println!(
				"Rendered {frames}-frame animated GIF @ {fps}fps to {} ({} bytes)",
				out.display(),
				gif_bytes.len()
			);
		}
		Commands::Commands => {
			let arr: Vec<JsonValue> = command_registry()
				.iter()
				.map(|c| {
					json::object! {
						id: c.id,
						description: c.description,
						params: c.params_schema
					}
				})
				.collect();
			println!("{}", json::stringify_pretty(JsonValue::Array(arr), 2));
		}
		Commands::Formats => {
			let reg = default_registry();
			let obj = json::object! {
				importers: caps_list_to_json(&reg.list_importers()),
				exporters: caps_list_to_json(&reg.list_exporters()),
			};
			println!("{}", json::stringify_pretty(obj, 2));
		}
		Commands::Export {
			input,
			sample,
			out,
			format,
			strict,
			pretty,
		} => {
			let input_key = if sample {
				"--sample".to_owned()
			} else {
				input.ok_or_else(|| "Missing input path (or pass --sample)".to_owned())?
			};
			let model = load_model(&input_key)?;
			let doc = model_to_ir(&model);
			let fmt_key = format
				.as_deref()
				.or_else(|| out.extension().and_then(|s| s.to_str()))
				.unwrap_or("inp");
			let reg = default_registry();
			let resolved_id = reg
				.resolve_exporter(fmt_key)
				.ok_or_else(|| format!("Unsupported export format '{fmt_key}'"))?
				.info()
				.id;
			let file = File::create(&out).map_err(|e| format!("Failed to create '{}': {e}", out.display()))?;
			let mut w = BufWriter::new(file);
			let opts = ExportOptions {
				pretty,
				strict,
				..ExportOptions::default()
			};
			let report = reg
				.export_to_writer(&resolved_id, &doc, &mut w, &opts)
				.map_err(|e| e.to_string())?;
			for warn in &report.warnings {
				eprintln!("[{}] {}", warn.code, warn.message);
			}
			println!(
				"Exported {} via '{}' ({} bytes, {} warning(s))",
				out.display(),
				resolved_id,
				report.bytes_written,
				report.warnings.len()
			);
		}
		Commands::Exec {
			input,
			cmd,
			params_json,
			save,
		} => {
			let model = load_model(&input)?;
			let mut session = Session::new(model);
			let params = json::parse(&params_json).map_err(|e| format!("Invalid JSON params: {e}"))?;
			let result = session.exec(&cmd, &params)?;
			if let Some(save_path) = save {
				let file = File::create(&save_path)
					.map_err(|e| format!("Failed to create {}: {e}", save_path.display()))?;
				let mut w = BufWriter::new(file);
				write_inp(&session.model, &mut w).map_err(|e| format!("Failed to write .inp: {e}"))?;
			}
			println!("{}", json::stringify_pretty(result, 2));
		}
		Commands::Serve { input, watch } => {
			let mut session = Session::new(load_model(&input)?);
			if let Some(ref wpath) = watch {
				if !wpath.exists() {
					write_inp_atomic(&session.model, wpath)?;
				}
			}
			let stdin = io::stdin();
			let mut stdout = io::stdout();
			for line in stdin.lock().lines() {
				let line = line.map_err(|e| e.to_string())?;
				let trimmed = line.trim();
				if trimmed.is_empty() {
					continue;
				}
				let req = match json::parse(trimmed) {
					Ok(v) => v,
					Err(e) => {
						let err = json::object! { ok: false, error: format!("Invalid JSON: {e}") };
						writeln!(stdout, "{}", json::stringify(err)).map_err(|e| e.to_string())?;
						stdout.flush().map_err(|e| e.to_string())?;
						continue;
					}
				};
				let cmd = req["cmd"].as_str().unwrap_or("");
				if cmd == "quit" {
					break;
				}
				let id = req["id"].clone();
				let params = &req["params"];
				let prev_rev = session.revision;
				let resp = match session.exec(cmd, params) {
					Ok(res) => {
						if session.revision != prev_rev {
							if let Some(ref wpath) = watch {
								write_inp_atomic(&session.model, wpath)?;
							}
						}
						json::object! {
							id: id,
							ok: true,
							revision: session.revision,
							result: res
						}
					}
					Err(err) => json::object! {
						id: id,
						ok: false,
						revision: session.revision,
						error: err
					},
				};
				writeln!(stdout, "{}", json::stringify(resp)).map_err(|e| e.to_string())?;
				stdout.flush().map_err(|e| e.to_string())?;
			}
		}
		Commands::Mcp { input, watch } => {
			let mut session = Session::new(load_model(&input)?);
			if let Some(ref wpath) = watch {
				if !wpath.exists() {
					write_inp_atomic(&session.model, wpath)?;
				}
			}
			let stdin = io::stdin();
			let mut stdout = io::stdout();
			for line in stdin.lock().lines() {
				let line = line.map_err(|e| e.to_string())?;
				let trimmed = line.trim();
				if trimmed.is_empty() {
					continue;
				}
				if let Ok(req) = json::parse(trimmed) {
					let prev_rev = session.revision;
					if let Some(resp) = handle_mcp_request(&mut session, &req) {
						if session.revision != prev_rev {
							if let Some(ref wpath) = watch {
								write_inp_atomic(&session.model, wpath)?;
							}
						}
						writeln!(stdout, "{}", json::stringify(resp)).map_err(|e| e.to_string())?;
						stdout.flush().map_err(|e| e.to_string())?;
					}
				}
			}
		}
	}
	Ok(())
}

fn main() {
	if let Err(err) = run() {
		eprintln!("Error: {err}");
		std::process::exit(1);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use inox2d_mcp::psd::import_psd_to_model;

	#[test]
	fn sample_vtuber_roundtrip_and_render_grid() {
		let model = build_sample_vtuber_model();
		let mut bytes = Vec::new();
		write_inp(&model, &mut bytes).expect("write sample .inp");
		let mut parsed = parse_inp(bytes.as_slice()).expect("parse sample .inp");

		let grid = render_grid(&mut parsed, "Head:: Yaw-Pitch", 128, true, &HashMap::new())
			.expect("render 3x3 head grid");
		assert_eq!(grid.width(), 128 * 3 + 4 * 4);
		assert_eq!(grid.height(), 128 * 3 + 4 * 4);

		// Verify blink actually deforms Eye_L when set to 0.0
		let mut open_params = HashMap::new();
		open_params.insert("Eye:: Left:: Open".to_owned(), vec2(1.0, 0.0));
		let frame_open = render_frame(&mut parsed, &open_params, &RenderOptions::default()).unwrap();

		let mut closed_params = HashMap::new();
		closed_params.insert("Eye:: Left:: Open".to_owned(), vec2(0.0, 0.0));
		let frame_closed = render_frame(&mut parsed, &closed_params, &RenderOptions::default()).unwrap();

		assert_ne!(frame_open.as_raw(), frame_closed.as_raw());
	}

	#[test]
	fn engine_and_mcp_protocol_flow() {
		let mut session = Session::new_sample();

		// Add part, re-grid, bind perspective, step physics
		let add_res = session
			.exec(
				"node.add_part",
				&json::object! {
					name: "Ahoge",
					parent_name: "Head",
					zsort: -12.0,
					x: 0.0,
					y: -95.0,
					width: 40.0,
					height: 50.0,
					cols: 2,
					rows: 2
				},
			)
			.unwrap();
		assert_eq!(add_res["name"].as_str(), Some("Ahoge"));

		let grid_res = session
			.exec(
				"mesh.auto_grid",
				&json::object! {
					node_name: "Ahoge",
					cols: 4,
					rows: 5
				},
			)
			.unwrap();
		assert_eq!(grid_res["vertices"].as_u32(), Some(30));

		let rig_res = session
			.exec(
				"rig.auto_head_perspective",
				&json::object! {
					node_name: "Ahoge",
					depth_z: 1.4
				},
			)
			.unwrap();
		assert_eq!(rig_res["param"].as_str(), Some("Head:: Yaw-Pitch"));

		// Verify MCP initialize + tools/list + tools/call
		let init_resp = handle_mcp_request(
			&mut session,
			&json::object! {
				jsonrpc: "2.0",
				id: 1,
				method: "initialize",
				params: {}
			},
		)
		.unwrap();
		assert_eq!(init_resp["result"]["serverInfo"]["name"].as_str(), Some("inox2d-mcp"));

		let list_resp = handle_mcp_request(
			&mut session,
			&json::object! {
				jsonrpc: "2.0",
				id: 2,
				method: "tools/list",
				params: {}
			},
		)
		.unwrap();
		assert!(list_resp["result"]["tools"].members().any(|t| t["name"] == "rig_auto_head_perspective"));

		let call_resp = handle_mcp_request(
			&mut session,
			&json::object! {
				jsonrpc: "2.0",
				id: 3,
				method: "tools/call",
				params: {
					name: "rig_auto_head_perspective",
					arguments: {
						node_name: "Ahoge",
						depth_z: 1.4
					}
				}
			},
		)
		.unwrap();
		assert!(!call_resp["result"]["isError"].as_bool().unwrap_or(false));
		assert!(call_resp["result"]["content"][0]["text"]
			.as_str()
			.unwrap()
			.contains("Head:: Yaw-Pitch"));
	}

	#[test]
	fn psd_roundtrip_contour_mesh_auto_rig_and_anim_sheet() {
		let mut sample = build_sample_vtuber_model();
		let psd_bytes = export_model_to_psd(&mut sample, 512, 512).expect("export sample to psd");
		let parsed_psd = parse_psd_bytes(&psd_bytes).expect("parse exported psd");
		assert_eq!(parsed_psd.width, 512);
		assert!(parsed_psd.layers.len() >= 8);

		let mut imported = import_psd_to_model(&parsed_psd, "PsdAutoRigged", true, true, 8, 8);
		assert!(imported.puppet.params.contains_key("Head:: Yaw-Pitch"));
		assert!(imported.puppet.params.contains_key("Mouth:: Form"));
		assert!(imported.puppet.animations.contains_key("Idle"));

		// Contour meshing should cull transparent corner cells on Face
		let face_id = imported
			.puppet
			.nodes
			.iter()
			.find(|n| n.name == "Face")
			.unwrap()
			.uuid;
		let face_mesh = imported.puppet.node_comps.get::<inox2d::node::components::Mesh>(face_id).unwrap();
		assert!(face_mesh.indices.len() / 3 <= 8 * 8 * 2);

		let sheet = render_anim_sheet(&mut imported, Some("Idle"), 8, 96, false).expect("render Idle sheet");
		assert_eq!(sheet.width(), 4 * 96 + 5 * 4);
		assert_eq!(sheet.height(), 2 * 96 + 3 * 4);
	}

	#[test]
	fn meshgroup_deform_propagation_and_animation_roundtrip() {
		use inox2d::math::interp::InterpolateMode;
		use inox2d::math::matrix::Matrix2d;
		use inox2d::math::transform::TransformOffset;
		use inox2d::node::components::{BlendMode, Blending, Drawable, MeshGroup, TexturedMesh};
		use inox2d::node::InoxNode;
		use inox2d::params::{AxisPoints, Binding, BindingValues, Param};
		use inox2d::texture::TextureId;
		use inox2d_mcp::rig::{encode_rgba_png, generate_grid_mesh};

		let mut puppet = Puppet::new_empty("MeshGroupTest");
		let root_id = puppet.nodes.root_node_id;

		let mg_id = puppet.next_node_uuid();
		puppet.add_node(
			root_id,
			InoxNode {
				uuid: mg_id,
				name: "DeformBox".to_owned(),
				enabled: true,
				zsort: 0.0,
				trans_offset: TransformOffset::default(),
				lock_to_root: false,
			},
		);
		puppet.node_comps.add(
			mg_id,
			MeshGroup {
				dynamic: true,
				translate_children: true,
			},
		);
		let mg_mesh = generate_grid_mesh(100.0, 100.0, 2, 2, vec2(0.0, 0.0));
		let mg_vert_count = mg_mesh.vertices.len();
		puppet.node_comps.add(mg_id, mg_mesh);

		let child_id = puppet.next_node_uuid();
		puppet.add_node(
			mg_id,
			InoxNode {
				uuid: child_id,
				name: "ChildPart".to_owned(),
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
					tint: glam::vec3(1.0, 1.0, 1.0),
					screen_tint: glam::vec3(0.0, 0.0, 0.0),
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
		puppet
			.node_comps
			.add(child_id, generate_grid_mesh(40.0, 40.0, 2, 2, vec2(0.0, 0.0)));

		let p_uuid = puppet.next_param_uuid();
		let values = Matrix2d::from_grid_transposed(2, 1, |ix, _| {
			if ix == 0 {
				vec![vec2(0.0, 0.0); mg_vert_count]
			} else {
				vec![vec2(12.0, -6.0); mg_vert_count]
			}
		});
		let is_set = Matrix2d::from_grid_transposed(2, 1, |_, _| true);
		puppet.params.insert(
			"Warp".to_owned(),
			Param {
				uuid: p_uuid,
				name: "Warp".to_owned(),
				is_vec2: false,
				min: vec2(0.0, 0.0),
				max: vec2(1.0, 0.0),
				defaults: vec2(0.0, 0.0),
				axis_points: AxisPoints {
					x: vec![0.0, 1.0],
					y: vec![0.0],
				},
				bindings: vec![Binding {
					node: mg_id,
					is_set,
					interpolate_mode: InterpolateMode::Linear,
					values: BindingValues::Deform(values),
				}],
			},
		);
		inox2d_mcp::rig::create_idle_animation(&mut puppet);

		let img = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 100, 100, 255]));
		let model = Model {
			puppet,
			textures: vec![encode_rgba_png(&img)],
			vendors: Vec::new(),
		};

		let mut bytes = Vec::new();
		write_inp(&model, &mut bytes).expect("write meshgroup .inp");
		let mut parsed = parse_inp(bytes.as_slice()).expect("parse meshgroup .inp");

		assert!(parsed.puppet.animations.contains_key("Idle"));
		parsed.puppet.init_all();
		parsed.puppet.begin_frame();
		parsed.puppet.param_ctx.as_mut().unwrap().set("Warp", vec2(1.0, 0.0)).unwrap();
		parsed.puppet.end_frame(0.0);

		let child_deforms = &parsed.puppet.render_ctx.as_ref().unwrap().vertex_buffers.deforms;
		assert!(
			child_deforms.iter().any(|d| (d.x - 12.0).abs() < 0.2 && (d.y + 6.0).abs() < 0.2),
			"Expected MeshGroup deform (12, -6) to propagate to child vertices, got {child_deforms:?}"
		);
	}

	#[test]
	fn opacity_binding_roundtrip_and_render() {
		let mut session = Session::new_sample();
		session
			.exec(
				"node.add_part",
				&json::object! {
					name: "Blush",
					parent_name: "Head",
					zsort: -2.0,
					x: 0.0,
					y: 12.0,
					width: 100.0,
					height: 30.0,
					color: [255, 90, 120, 220]
				},
			)
			.unwrap();
		session
			.exec(
				"rig.bind_opacity",
				&json::object! {
					node_name: "Blush",
					param: "Expr:: Blush",
					opacity_at_0: 0.0,
					opacity_at_1: 1.0
				},
			)
			.unwrap();

		let mut bytes = Vec::new();
		write_inp(&session.model, &mut bytes).expect("write opacity .inp");
		let mut parsed = parse_inp(bytes.as_slice()).expect("parse opacity .inp");

		let mut off_map = HashMap::new();
		off_map.insert("Expr:: Blush".to_owned(), vec2(0.0, 0.0));
		let img_off = render_frame(&mut parsed, &off_map, &RenderOptions::default()).unwrap();

		let mut on_map = HashMap::new();
		on_map.insert("Expr:: Blush".to_owned(), vec2(1.0, 0.0));
		let img_on = render_frame(&mut parsed, &on_map, &RenderOptions::default()).unwrap();

		assert_ne!(img_off.as_raw(), img_on.as_raw());
	}

	#[test]
	fn audit_detects_leaks_and_foldovers_then_auto_fix_repairs() {
		let mut session = Session::new_sample();
		let anatomy = session.exec("rig.analyze_anatomy", &json::object! {}).unwrap();
		assert_eq!(anatomy["face_anchor"]["name"].as_str(), Some("Face"));

		// Intentionally push Eye_L's right edge far left across its own mesh and out of the face boundary
		session
			.exec(
				"deform.brush",
				&json::object! {
					node_name: "Eye_L",
					param: "Head:: Yaw-Pitch",
					kx: -1.0,
					ky: 0.0,
					anchor: "right",
					radius_ratio: 0.25,
					dx: -160.0,
					dy: 0.0
				},
			)
			.unwrap();

		let bad_report = session.exec("rig.audit", &json::object! {}).unwrap();
		assert_eq!(bad_report["passed"].as_bool(), Some(false));
		assert!(bad_report["summary"]["containment_leaks"].as_u32().unwrap() > 0);
		assert!(bad_report["summary"]["mesh_foldovers"].as_u32().unwrap() > 0);

		let fixed = session.exec("rig.auto_fix", &json::object! {}).unwrap();
		assert!(fixed["clamped_vertices"].as_u32().unwrap() > 0);
		assert!(fixed["relaxed_cells"].as_u32().unwrap() > 0);

		let clean_report = session.exec("rig.audit", &json::object! {}).unwrap();
		assert_eq!(clean_report["summary"]["containment_leaks"].as_u32(), Some(0));
		assert_eq!(clean_report["summary"]["mesh_foldovers"].as_u32(), Some(0));
		assert_eq!(clean_report["passed"].as_bool(), Some(true));

		let diag = render_diagnostic(&mut session.model, &HashMap::new(), 256, 256, 0.7).unwrap();
		assert_eq!(diag.dimensions(), (256, 256));
	}

	#[test]
	fn undo_redo_checkpoint_batch_and_diff() {
		let mut session = Session::new_sample();
		session
			.exec("session.checkpoint", &json::object! { name: "base" })
			.unwrap();

		// Atomic batch succeeds and wraps into a single undo entry
		let batch_res = session
			.exec(
				"session.batch",
				&json::object! {
					commands: [
						{ cmd: "node.synthesize_overlay", params: { kind: "blush" } },
						{ cmd: "param.set", params: { param: "Expr:: Blush", x: 1.0 } }
					]
				},
			)
			.unwrap();
		assert_eq!(batch_res["executed"].as_u32(), Some(2));
		assert!(session.model.puppet.nodes.iter().any(|n| n.name == "Expr_Blush"));

		// Render side-by-side Before | After | Diff Heatmap
		let diff_path = std::env::temp_dir().join("inox2d_test_diff.png");
		let diff_res = session
			.exec(
				"render.diff",
				&json::object! {
					out: diff_path.to_string_lossy().as_ref(),
					checkpoint: "base",
					tile_size: 160
				},
			)
			.unwrap();
		assert!(diff_res["changed_pixel_pct"].as_f32().unwrap() > 0.1);
		let _ = fs::remove_file(&diff_path);

		// Failed batch rolls back cleanly
		let fail_batch = session.exec(
			"session.batch",
			&json::object! {
				commands: [
					{ cmd: "node.synthesize_overlay", params: { kind: "angry" } },
					{ cmd: "unknown.command", params: {} }
				]
			},
		);
		assert!(fail_batch.is_err());
		assert!(!session.model.puppet.nodes.iter().any(|n| n.name == "Expr_Angry"));

		// Undo removes the whole blush batch; redo restores it; restore resets to base
		session.exec("edit.undo", &json::object! {}).unwrap();
		assert!(!session.model.puppet.nodes.iter().any(|n| n.name == "Expr_Blush"));
		session.exec("edit.redo", &json::object! {}).unwrap();
		assert!(session.model.puppet.nodes.iter().any(|n| n.name == "Expr_Blush"));
		session
			.exec("session.restore", &json::object! { name: "base" })
			.unwrap();
		assert!(!session.model.puppet.nodes.iter().any(|n| n.name == "Expr_Blush"));
	}

	#[test]
	fn node_crud_meshgroup_visemes_overlays_and_gif() {
		let mut session = Session::new_sample();

		// Configure Face opacity & Mouth visemes
		session
			.exec(
				"rig.auto_visemes",
				&json::object! { node_name: "Mouth", open_px: 14.0, width_px: 9.0 },
			)
			.unwrap();
		assert!(session.model.puppet.params.contains_key("Mouth:: Viseme"));

		// Add Pupil_L, clip it to Eye_L via node.set_mask
		session
			.exec(
				"node.add_part",
				&json::object! {
					name: "Pupil_L",
					parent_name: "Head",
					zsort: -5.5,
					x: -38.0,
					y: -6.0,
					width: 24.0,
					height: 24.0,
					color: [30, 30, 90, 255]
				},
			)
			.unwrap();
		let mask_res = session
			.exec(
				"node.set_mask",
				&json::object! {
					node_name: "Pupil_L",
					sources: [{ name: "Eye_L", mode: "Mask" }]
				},
			)
			.unwrap();
		assert_eq!(mask_res["mask_count"].as_u32(), Some(1));

		// Wrap Eye_L + Pupil_L inside a MeshGroup cage
		let mg_res = session
			.exec(
				"node.add_meshgroup",
				&json::object! {
					name: "LeftEyeCage",
					parent_name: "Head",
					children: ["Eye_L", "Pupil_L"],
					cols: 3,
					rows: 3
				},
			)
			.unwrap();
		assert_eq!(mg_res["reparented_children"].as_u32(), Some(2));

		// Synthesize tears + remove Pupil_L cleanly
		session
			.exec("node.synthesize_overlay", &json::object! { kind: "tears" })
			.unwrap();
		assert!(session.model.puppet.params.contains_key("Expr:: Tears"));
		session
			.exec("node.remove", &json::object! { node_name: "Pupil_L" })
			.unwrap();
		assert!(!session.model.puppet.nodes.iter().any(|n| n.name == "Pupil_L"));

		// Render animated GIF and verify GIF89a magic & decodable frames
		let gif_path = std::env::temp_dir().join("inox2d_test_idle.gif");
		let gif_res = session
			.exec(
				"render.gif",
				&json::object! {
					out: gif_path.to_string_lossy().as_ref(),
					anim: "Idle",
					frames: 6,
					fps: 10,
					width: 128,
					height: 128
				},
			)
			.unwrap();
		assert!(gif_res["bytes"].as_u32().unwrap() > 100);
		let gif_bytes = fs::read(&gif_path).unwrap();
		assert!(gif_bytes.starts_with(b"GIF89a"));
		let _ = fs::remove_file(&gif_path);
	}

	#[test]
	fn delaunay_mesh_and_figcraft_card() {
		use image::{Rgba, RgbaImage};
		use inox2d_mcp::rig::generate_delaunay_mesh;

		// Concave L-shaped silhouette: top-right quadrant (x >= 64 && y < 64) is transparent.
		let mut img = RgbaImage::from_pixel(128, 128, Rgba([0, 0, 0, 0]));
		for y in 8..120 {
			for x in 8..120 {
				if !(x >= 68 && y <= 60) {
					img.put_pixel(x, y, Rgba([220, 180, 190, 255]));
				}
			}
		}

		let mesh = generate_delaunay_mesh(&img, 128.0, 128.0, 18.0, 4);
		assert!(mesh.vertices.len() >= 12);
		assert!(mesh.indices.len() >= 12 && mesh.indices.len() % 3 == 0);
		for &uv in &mesh.uvs {
			assert!(uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0);
		}
		// Ensure no triangle centroid falls deep inside the empty top-right bay (x > 84, y < 44 => local x > 20, y < -20).
		for tri in mesh.indices.chunks_exact(3) {
			let a = mesh.vertices[tri[0] as usize];
			let b = mesh.vertices[tri[1] as usize];
			let c = mesh.vertices[tri[2] as usize];
			let g = (a + b + c) / 3.0;
			assert!(
				!(g.x > 20.0 && g.y < -20.0),
				"Delaunay culling leaked triangle into concave bay at {:?}",
				g
			);
		}

		let mut session = Session::new_sample();
		let res = session
			.exec(
				"mesh.auto_delaunay",
				&json::object! { node_name: "Face", step_px: 20.0 },
			)
			.unwrap();
		assert_eq!(res["triangulation"].as_str(), Some("bowyer_watson_delaunay"));
		assert!(res["triangles"].as_u32().unwrap() >= 8);

		let card_png = std::env::temp_dir().join("inox2d_test_spec_card.png");
		let card_svg = card_png.with_extension("svg");
		let card_res = session
			.exec(
				"craft.figcraft_card",
				&json::object! { out: card_png.to_string_lossy().as_ref() },
			)
			.unwrap();
		assert!(card_png.exists());
		assert!(card_svg.exists());
		let png_bytes = fs::read(&card_png).unwrap();
		assert!(png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
		let svg_text = fs::read_to_string(&card_svg).unwrap();
		assert!(svg_text.contains("6-POINT RIG AUDIT BREAKDOWN"));
		assert!(card_res["audit_score"].as_u32().is_some());
		let _ = fs::remove_file(&card_png);
		let _ = fs::remove_file(&card_svg);
	}

	#[test]
	fn manual_vertex_topology_physics_and_keyframe_commands() {
		use inox2d::node::components::{Mesh, SimplePhysics};
		use inox2d::params::BindingValues;

		let mut session = Session::new_sample();
		let face_id = session
			.model
			.puppet
			.nodes
			.iter()
			.find(|n| n.name == "Face")
			.unwrap()
			.uuid;

		let orig_verts = session
			.model
			.puppet
			.node_comps
			.get::<Mesh>(face_id)
			.unwrap()
			.vertices
			.len();

		// 1) Move explicit vertices in deform keypoint (kx = 1.0, ky = 0.0)
		let vres = session
			.exec(
				"deform.set_vertices",
				&json::object! {
					node_name: "Face",
					param: "Head:: Yaw-Pitch",
					kx: 1.0,
					ky: 0.0,
					indices: [0, 1],
					dx: 4.5,
					dy: -2.0
				},
			)
			.unwrap();
		assert_eq!(vres["updated_vertices"].as_u32(), Some(2));

		// 2) Insert a vertex via mesh.edit_topology & verify existing Face deform bindings resize & preserve!
		let ins = session
			.exec(
				"mesh.edit_topology",
				&json::object! { node_name: "Face", action: "insert", x: 5.0, y: 7.0 },
			)
			.unwrap();
		let new_verts = ins["vertices"].as_usize().unwrap();
		assert_eq!(new_verts, orig_verts + 1);

		let head_param = session.model.puppet.params.get("Head:: Yaw-Pitch").unwrap();
		let face_binding = head_param
			.bindings
			.iter()
			.find(|b| b.node == face_id && matches!(b.values, BindingValues::Deform(_)))
			.unwrap();
		if let BindingValues::Deform(ref mat) = face_binding.values {
			assert_eq!(mat.get(0, 0).unwrap().len(), new_verts);
			// Ensure non-zero head perspective binding survived retriangulation
			assert!(mat.get(2, 1).unwrap().iter().any(|v| v.length() > 0.5));
		}

		// 3) Remove the inserted vertex cleanly
		let inserted_idx = ins["inserted_index"].as_u32().unwrap();
		let rem = session
			.exec(
				"mesh.edit_topology",
				&json::object! { node_name: "Face", action: "remove", indices: [inserted_idx] },
			)
			.unwrap();
		assert_eq!(rem["vertices"].as_usize(), Some(orig_verts));

		// 4) Configure existing SimplePhysics pendulum
		let phys_id = session
			.model
			.puppet
			.nodes
			.iter()
			.find(|n| session.model.puppet.node_comps.get::<SimplePhysics>(n.uuid).is_some())
			.unwrap()
			.uuid;
		session
			.exec(
				"physics.configure",
				&json::object! { node_uuid: phys_id.0, length: 125.0, frequency: 3.2, angle_damping: 0.65 },
			)
			.unwrap();
		let phys = session.model.puppet.node_comps.get::<SimplePhysics>(phys_id).unwrap();
		assert!((phys.props.length - 125.0).abs() < 0.01);
		assert!((phys.props.frequency - 3.2).abs() < 0.01);

		// 5) Insert and remove animation keyframe
		session
			.exec(
				"anim.set_keyframe",
				&json::object! { anim: "Idle", param: "Head:: Roll", target: 0, frame: 45, value: 0.55, tension: 0.8 },
			)
			.unwrap();
		let rm_kf = session
			.exec(
				"anim.remove_keyframe",
				&json::object! { anim: "Idle", param: "Head:: Roll", target: 0, frame: 45 },
			)
			.unwrap();
		assert_eq!(rm_kf["removed"].as_bool(), Some(true));
	}

	#[test]
	fn format_registry_export_import_and_strict_warnings() {
		let mut session = Session::new(build_sample_vtuber_model());
		session.exec("anim.create_idle", &json::object! {}).unwrap();

		// 1) format.list includes inp, rig-json, and psd
		let listed = session.exec("format.list", &json::object! {}).unwrap();
		let exp_ids: Vec<&str> = listed["exporters"]
			.members()
			.filter_map(|c| c["id"].as_str())
			.collect();
		assert!(exp_ids.contains(&"inp"));
		assert!(exp_ids.contains(&"rig-json"));
		assert!(exp_ids.contains(&"psd"));

		let dir = std::env::temp_dir().join("riglens_mcp_format_test");
		let _ = fs::create_dir_all(&dir);
		let rigjson_path = dir.join("sample.rigjson");
		let psd_path = dir.join("sample.psd");

		// 2) Export to .rigjson cleanly with zero warnings
		let exp_json = session
			.exec(
				"puppet.export",
				&json::object! { path: rigjson_path.to_str().unwrap(), pretty: true },
			)
			.unwrap();
		assert_eq!(exp_json["format"].as_str(), Some("rig-json"));
		assert!(exp_json["bytes_written"].as_usize().unwrap() > 100);
		assert_eq!(exp_json["warnings"].len(), 0);

		// 3) Non-strict export to .psd succeeds and reports structured capability warnings
		let exp_psd = session
			.exec(
				"puppet.export",
				&json::object! { path: psd_path.to_str().unwrap(), width: 256, height: 256 },
			)
			.unwrap();
		assert_eq!(exp_psd["format"].as_str(), Some("psd"));
		assert!(exp_psd["bytes_written"].as_usize().unwrap() > 0);
		let has_anim_warn = exp_psd["warnings"]
			.members()
			.any(|w| w["code"].as_str() == Some("UNSUPPORTED_ANIMATIONS"));
		assert!(has_anim_warn, "expected UNSUPPORTED_ANIMATIONS warning: {:?}", exp_psd["warnings"]);

		// 4) Strict export to .psd rejects because animations are dropped
		let strict_err = session
			.exec(
				"puppet.export",
				&json::object! { path: psd_path.to_str().unwrap(), strict: true },
			)
			.unwrap_err();
		assert!(strict_err.contains("strict mode"), "unexpected error: {strict_err}");

		// 5) Re-import .rigjson back into a fresh session
		let orig_nodes = session.model.puppet.nodes.iter().count();
		let mut fresh = Session::new(Model {
			puppet: Puppet::new_empty("Empty"),
			textures: Vec::new(),
			vendors: Vec::new(),
		});
		let imp = fresh
			.exec("puppet.import", &json::object! { path: rigjson_path.to_str().unwrap() })
			.unwrap();
		assert_eq!(imp["info"]["node_count"].as_usize(), Some(orig_nodes));
		assert!(fresh.model.puppet.animations.contains_key("Idle"));

		let _ = fs::remove_dir_all(&dir);
	}

	#[test]
	fn audit_ten_point_checks_detect_deformers_bindings_draw_order_and_mask_cycles() {
		use inox2d::math::curve::CurveType;
		use inox2d::math::interp::InterpolateMode;
		use inox2d::math::matrix::Matrix2d;
		use inox2d::math::transform::TransformOffset;
		use inox2d::node::components::{
			GridDeformer, GridFormation, Mask, MaskMode, Masks, PathDeformer,
		};
		use inox2d::node::{InoxNode, InoxNodeUuid};
		use inox2d::params::{Binding, BindingValues};

		let mut session = Session::new_sample();

		// Clean sample passes all 10 audit checks
		let clean = session.exec("rig.audit", &json::object! {}).unwrap();
		assert_eq!(clean["passed"].as_bool(), Some(true), "clean sample failed audit: {clean}");
		assert_eq!(clean["summary"]["deformer_issues"].as_u32(), Some(0));
		assert_eq!(clean["summary"]["binding_coverage"].as_u32(), Some(0));
		assert_eq!(clean["summary"]["draw_order_conflicts"].as_u32(), Some(0));
		assert_eq!(clean["summary"]["mask_cycles"].as_u32(), Some(0));

		// Inject Check 7 defects: degenerate GridDeformer + degenerate PathDeformer (both unbound)
		let root = session.model.puppet.nodes.root_node_id;
		let bad_grid_id = session.model.puppet.next_node_uuid();
		session.model.puppet.add_node(
			root,
			InoxNode {
				uuid: bad_grid_id,
				name: "BrokenGrid".to_owned(),
				enabled: true,
				zsort: 0.0,
				trans_offset: TransformOffset::default(),
				lock_to_root: false,
			},
		);
		session.model.puppet.node_comps.set(
			bad_grid_id,
			GridDeformer {
				axis_x: vec![0.0],
				axis_y: vec![-10.0, 10.0],
				formation: GridFormation::Bilinear,
				dynamic: false,
				translate_children: true,
			},
		);

		let bad_path_id = session.model.puppet.next_node_uuid();
		session.model.puppet.add_node(
			root,
			InoxNode {
				uuid: bad_path_id,
				name: "BrokenPath".to_owned(),
				enabled: true,
				zsort: 0.0,
				trans_offset: TransformOffset::default(),
				lock_to_root: false,
			},
		);
		session.model.puppet.node_comps.set(
			bad_path_id,
			PathDeformer {
				curve_type: CurveType::Spline,
				vertices: vec![glam::vec2(0.0, 0.0)],
				dynamic: false,
				physics_only: false,
			},
		);

		// Inject Check 8 defects: dangling target UUID & wrong deform vertex count
		let face_id = session
			.model
			.puppet
			.nodes
			.iter()
			.find(|n| n.name == "Face")
			.unwrap()
			.uuid;
		let eye_l_id = session
			.model
			.puppet
			.nodes
			.iter()
			.find(|n| n.name == "Eye_L")
			.unwrap()
			.uuid;
		let mouth_id = session
			.model
			.puppet
			.nodes
			.iter()
			.find(|n| n.name == "Mouth")
			.unwrap()
			.uuid;

		if let Some(roll) = session.model.puppet.params.get_mut("Head:: Roll") {
			roll.bindings.push(Binding {
				node: InoxNodeUuid(99999),
				is_set: Matrix2d::from_grid_transposed(3, 1, |_, _| false),
				interpolate_mode: InterpolateMode::Linear,
				values: BindingValues::ZSort(Matrix2d::from_grid_transposed(3, 1, |_, _| 0.0)),
			});
			roll.bindings.push(Binding {
				node: face_id,
				is_set: Matrix2d::from_grid_transposed(3, 1, |ix, _| ix == 0),
				interpolate_mode: InterpolateMode::Linear,
				values: BindingValues::Deform(Matrix2d::from_grid_transposed(3, 1, |_, _| {
					vec![glam::vec2(0.0, 0.0); 2]
				})),
			});
		}

		// Inject Check 9 defects: move Eye_L behind Face and collide Mouth zsort with Face
		session.model.puppet.nodes.get_node_mut(eye_l_id).unwrap().zsort = 5.0;
		session.model.puppet.nodes.get_node_mut(mouth_id).unwrap().zsort = 0.0;

		// Inject Check 10 defects: 2-node mask cycle between Eye_L and Face
		session.model.puppet.node_comps.set(
			eye_l_id,
			Masks {
				threshold: 0.5,
				masks: vec![Mask {
					source: face_id,
					mode: MaskMode::Mask,
				}],
			},
		);
		session.model.puppet.node_comps.set(
			face_id,
			Masks {
				threshold: 0.5,
				masks: vec![Mask {
					source: eye_l_id,
					mode: MaskMode::Mask,
				}],
			},
		);

		session.model.puppet.reset_contexts();
		let dirty = session.exec("rig.audit", &json::object! {}).unwrap();
		assert_eq!(dirty["passed"].as_bool(), Some(false));
		assert!(dirty["summary"]["deformer_issues"].as_u32().unwrap() >= 2, "{dirty}");
		assert!(dirty["summary"]["binding_coverage"].as_u32().unwrap() >= 2, "{dirty}");
		assert!(dirty["summary"]["draw_order_conflicts"].as_u32().unwrap() >= 2, "{dirty}");
		assert!(dirty["summary"]["mask_cycles"].as_u32().unwrap() >= 1, "{dirty}");
	}

	#[test]
	fn constrained_delaunay_respects_concave_bay_offset_and_joint_refinement() {
		use image::{Rgba, RgbaImage};
		use inox2d_mcp::rig::{generate_delaunay_mesh_with_options, DelaunayMeshOptions, RefinementZone};

		// Paint a crisp 160x160 C-shaped silhouette with a wide open concave bay on the right.
		let mut img = RgbaImage::from_pixel(160, 160, Rgba([0, 0, 0, 0]));
		let solid = Rgba([220, 180, 160, 255]);
		for y in 20..140 {
			for x in 20..140 {
				let in_spine = x < 56;
				let in_top = y < 52;
				let in_bot = y >= 108;
				if in_spine || in_top || in_bot {
					img.put_pixel(x, y, solid);
				}
			}
		}

		// 1) Constrained Delaunay with 0px offset must not bridge across the empty concave bay.
		let c_mesh = generate_delaunay_mesh_with_options(
			&img,
			160.0,
			160.0,
			&DelaunayMeshOptions {
				step_px: 18.0,
				alpha_threshold: 4,
				boundary_offset_px: 0.0,
				constrained: true,
				refine_near_corners: true,
				refinement_zones: Vec::new(),
			},
		);
		let tri_count = c_mesh.indices.len() / 3;
		assert!(tri_count >= 20, "expected >= 20 triangles, got {tri_count}");

		for tri in c_mesh.indices.chunks_exact(3) {
			let a = c_mesh.vertices[tri[0] as usize];
			let b = c_mesh.vertices[tri[1] as usize];
			let c = c_mesh.vertices[tri[2] as usize];
			let area2 = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
			assert!(area2 > 0.0, "triangle not CCW or degenerate: {area2}");
			let centroid = (a + b + c) / 3.0;
			let in_empty_bay = centroid.x > -18.0 && centroid.x < 56.0 && centroid.y.abs() < 22.0;
			assert!(
				!in_empty_bay,
				"constrained Delaunay bridged empty concave bay at {centroid:?}"
			);
		}

		// 2) boundary_offset_px > 0 expands mesh bounds outward cleanly.
		let expanded = generate_delaunay_mesh_with_options(
			&img,
			160.0,
			160.0,
			&DelaunayMeshOptions {
				step_px: 18.0,
				alpha_threshold: 4,
				boundary_offset_px: 6.0,
				constrained: true,
				refine_near_corners: false,
				refinement_zones: Vec::new(),
			},
		);
		let span = |m: &inox2d::node::components::Mesh| -> (f32, f32) {
			let mut mn = glam::vec2(f32::INFINITY, f32::INFINITY);
			let mut mx = glam::vec2(f32::NEG_INFINITY, f32::NEG_INFINITY);
			for &v in &m.vertices {
				mn = mn.min(v);
				mx = mx.max(v);
			}
			(mx.x - mn.x, mx.y - mn.y)
		};
		let (w0, h0) = span(&c_mesh);
		let (w6, h6) = span(&expanded);
		assert!(w6 > w0 + 4.0 && h6 > h0 + 4.0, "expected outward offset ({w0},{h0}) -> ({w6},{h6})");

		// 3) Explicit RefinementZone densifies joint region without exploding distant regions.
		let joint_center = glam::vec2(-42.0, 0.0);
		let coarse = generate_delaunay_mesh_with_options(
			&img,
			160.0,
			160.0,
			&DelaunayMeshOptions {
				step_px: 20.0,
				alpha_threshold: 4,
				boundary_offset_px: 0.0,
				constrained: true,
				refine_near_corners: false,
				refinement_zones: Vec::new(),
			},
		);
		let refined = generate_delaunay_mesh_with_options(
			&img,
			160.0,
			160.0,
			&DelaunayMeshOptions {
				step_px: 20.0,
				alpha_threshold: 4,
				boundary_offset_px: 0.0,
				constrained: true,
				refine_near_corners: false,
				refinement_zones: vec![RefinementZone {
					center: joint_center,
					radius: 26.0,
					density_scale: 0.3,
				}],
			},
		);
		let count_near = |m: &inox2d::node::components::Mesh| {
			m.vertices
				.iter()
				.filter(|&&v| (v - joint_center).length() <= 26.0)
				.count()
		};
		let c_cnt = count_near(&coarse);
		let r_cnt = count_near(&refined);
		assert!(
			r_cnt > c_cnt * 2,
			"expected refinement zone to multiply local vertices: coarse={c_cnt}, refined={r_cnt}"
		);

		// 4) Exercise MCP command `mesh.auto_delaunay` with new parameters end-to-end.
		let mut session = Session::new_sample();
		let res = session
			.exec(
				"mesh.auto_delaunay",
				&json::object! {
					node_name: "Face",
					step_px: 20.0,
					boundary_offset_px: 3.0,
					constrained: true,
					refine_near_corners: true,
					refinement_zones: [
						{ center: [0.0, 0.0], radius: 28.0, density_scale: 0.4 }
					]
				},
			)
			.unwrap();
		assert_eq!(res["triangulation"].as_str(), Some("bowyer_watson_delaunay"));
		assert_eq!(res["constrained"].as_bool(), Some(true));
		assert!(res["triangles"].as_u32().unwrap() >= 20);
	}
}

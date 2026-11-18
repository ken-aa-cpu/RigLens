#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use glam::{vec2, Vec2};
use inox2d::formats::inp::{parse_inp, write_inp};
use inox2d::model::Model;
use inox2d::puppet::Puppet;
use inox2d_mcp::audit::{audit_rig, auto_fix_rig};
use inox2d_mcp::engine::{command_registry, Session};
use inox2d_mcp::headless_render::{render_anim_sheet, render_diagnostic, render_frame, render_grid, RenderOptions};
use inox2d_mcp::psd::{export_model_to_psd, import_psd_to_model, parse_psd_bytes};
use inox2d_mcp::rig::{auto_rig_vtuber, build_sample_vtuber_model};
use json::JsonValue;

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
	/// Import a layered .psd file into an .inp puppet (with optional contour meshing & auto-rig)
	ImportPsd {
		psd: PathBuf,
		#[arg(long)]
		out: PathBuf,
		#[arg(long)]
		name: Option<String>,
		#[arg(long)]
		no_contour: bool,
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
	/// List all engine commands as JSON
	Commands,
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
	},
	/// Run Model Context Protocol (MCP) JSON-RPC 2.0 server on stdio
	Mcp {
		#[arg(default_value = "--sample")]
		input: String,
	},
}

fn load_model(input: &str) -> Result<Model, String> {
	if input == "--sample" || input == "sample" {
		return Ok(build_sample_vtuber_model());
	}
	let file = File::open(Path::new(input)).map_err(|e| format!("Failed to open '{input}': {e}"))?;
	let mut reader = BufReader::new(file);
	let mut bytes = Vec::new();
	reader
		.read_to_end(&mut bytes)
		.map_err(|e| format!("Failed to read '{input}': {e}"))?;
	parse_inp(bytes.as_slice()).map_err(|e| format!("Failed to parse '{input}': {e}"))
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
				props.insert("contour_mesh", json::object! { type: "boolean" });
				props.insert("auto_rig", json::object! { type: "boolean" });
				props.insert("cols", json::object! { type: "integer" });
				props.insert("rows", json::object! { type: "integer" });
			} else if spec.id == "puppet.export_psd" {
				props.insert("path", json::object! { type: "string", description: "Output .psd path" });
				props.insert("width", json::object! { type: "integer" });
				props.insert("height", json::object! { type: "integer" });
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
			} else if spec.id == "render.diagnostic" {
				props.insert("out", json::object! { type: "string" });
				props.insert("width", json::object! { type: "integer" });
				props.insert("height", json::object! { type: "integer" });
				props.insert("scale", json::object! { type: "number" });
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
			let model = import_psd_to_model(&doc, &puppet_name, !no_contour, !no_auto_rig, cols, rows);
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
		Commands::Serve { input } => {
			let mut session = Session::new(load_model(&input)?);
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
				let resp = match session.exec(cmd, params) {
					Ok(res) => json::object! {
						id: id,
						ok: true,
						revision: session.revision,
						result: res
					},
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
		Commands::Mcp { input } => {
			let mut session = Session::new(load_model(&input)?);
			let stdin = io::stdin();
			let mut stdout = io::stdout();
			for line in stdin.lock().lines() {
				let line = line.map_err(|e| e.to_string())?;
				let trimmed = line.trim();
				if trimmed.is_empty() {
					continue;
				}
				if let Ok(req) = json::parse(trimmed) {
					if let Some(resp) = handle_mcp_request(&mut session, &req) {
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
}

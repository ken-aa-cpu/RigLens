#![forbid(unsafe_code)]

mod mcp_server;

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

use mcp_server::handle_mcp_request;

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
#[path = "cli_tests.rs"]
mod tests;

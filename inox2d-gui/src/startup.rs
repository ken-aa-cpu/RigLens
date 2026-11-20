use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::Parser;
use eframe::egui;
use image::RgbaImage;
use inox2d::formats::inp::{parse_inp, write_inp};
use inox2d::model::Model;
use inox2d::node::components::TexturedMesh;
use inox2d::node::InoxNodeUuid;
use inox2d::puppet::Puppet;
use inox2d_mcp::engine::Session;
use inox2d_mcp::psd::{import_psd_to_model_with_mode, parse_psd_bytes};

#[derive(Parser, Debug)]
#[command(name = "inox2d-gui", version, about = "Craft-style native Rust desktop studio for Inox2D")]
pub(crate) struct Cli {
	/// Input .inp/.inx/.psd/.rigjson file or 'sample'
	pub(crate) input: Option<String>,
	/// Select a model preset ('midori', 'aka', 'sample') or example filename/path
	#[arg(long)]
	pub(crate) model: Option<String>,
	/// Load the built-in rigged sample VTuber puppet
	#[arg(long)]
	pub(crate) sample: bool,
	/// Watch/sync .inp file on disk for bidirectional hot-reload with MCP / AI agents
	#[arg(long)]
	pub(crate) watch: Option<PathBuf>,
	/// Render several frames, capture a PNG screenshot of the studio window, and exit
	#[arg(long)]
	pub(crate) screenshot: Option<PathBuf>,
	/// Optional initial camera zoom multiplier (e.g. 4.0 to inspect face close-up)
	#[arg(long)]
	pub(crate) zoom: Option<f32>,
	/// Optional initial vertical camera pan offset in world units (negative = look higher toward head)
	#[arg(long, default_value_t = 0.0)]
	pub(crate) focus_y: f32,
	/// Start with interactive GPU FBO+Stencil stage (default)
	#[arg(long)]
	pub(crate) gpu_stage: bool,
	/// Start with static CPU Exact FX reference renderer instead of GPU stage
	#[arg(long)]
	pub(crate) cpu_exact: bool,
}

pub(crate) const VISEME_PRESETS: [(&str, f32, f32); 6] = [
	("Neutral", 0.0, 0.0),
	("A", 0.0, 1.0),
	("I", 1.0, 0.35),
	("U", -1.0, 0.45),
	("E", 0.75, 0.7),
	("O", -0.45, 0.9),
];

pub(crate) fn pick_default_selected_node(puppet: &Puppet) -> Option<InoxNodeUuid> {
	puppet
		.nodes
		.iter()
		.find(|n| {
			puppet.node_comps.get::<TexturedMesh>(n.uuid).is_some()
				&& (n.name.eq_ignore_ascii_case("face") || n.name.contains("臉"))
		})
		.or_else(|| {
			puppet
				.nodes
				.iter()
				.find(|n| puppet.node_comps.get::<TexturedMesh>(n.uuid).is_some())
		})
		.map(|n| n.uuid)
}

pub(crate) fn pick_default_anim(puppet: &Puppet) -> String {
	if puppet.animations.contains_key("Idle") {
		return "Idle".to_owned();
	}
	let mut keys: Vec<_> = puppet.animations.keys().cloned().collect();
	keys.sort();
	keys.into_iter().next().unwrap_or_else(|| "Idle".to_owned())
}

pub fn find_example_models() -> Vec<(String, PathBuf)> {
	let mut search_dirs: Vec<PathBuf> = Vec::new();
	if let Some(env_dir) = std::env::var_os("RIGLENS_EXAMPLE_MODELS") {
		search_dirs.push(PathBuf::from(env_dir));
	}
	search_dirs.push(PathBuf::from("assets/example-models"));
	search_dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../assets/example-models"));
	if let Ok(exe) = std::env::current_exe() {
		if let Some(dir) = exe.parent() {
			search_dirs.push(dir.join("assets/example-models"));
			search_dirs.push(dir.join("../../assets/example-models"));
		}
	}

	let mut by_key: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();
	for dir in search_dirs {
		let Ok(entries) = fs::read_dir(&dir) else {
			continue;
		};
		for entry in entries.flatten() {
			let path = entry.path();
			if !path.is_file() {
				continue;
			}
			let ext = path
				.extension()
				.and_then(|s| s.to_str())
				.map(str::to_ascii_lowercase)
				.unwrap_or_default();
			if ext != "inx" && ext != "inp" {
				continue;
			}
			let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
				continue;
			};
			let key = stem.to_ascii_lowercase();
			by_key.entry(key).or_insert_with(|| (stem, path));
		}
	}

	let mut out: Vec<(String, PathBuf)> = by_key.into_values().collect();
	out.sort_by(|(a, _), (b, _)| {
		let rank = |s: &str| match s.to_ascii_lowercase().as_str() {
			"midori" => 0usize,
			"aka" => 1usize,
			_ => 2usize,
		};
		rank(a)
			.cmp(&rank(b))
			.then_with(|| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()))
	});
	out
}

pub(crate) fn resolve_startup_input(cli: &Cli) -> Result<(String, Option<PathBuf>), String> {
	if cli.sample {
		return Ok(("--sample".to_owned(), None));
	}
	if let Some(ref model_arg) = cli.model {
		let trimmed = model_arg.trim();
		if trimmed.eq_ignore_ascii_case("sample") || trimmed == "--sample" {
			return Ok(("--sample".to_owned(), None));
		}
		let direct = Path::new(trimmed);
		if direct.is_file() {
			return Ok((trimmed.to_owned(), Some(direct.to_path_buf())));
		}
		let needle = trimmed.to_ascii_lowercase();
		let models = find_example_models();
		if let Some((_, p)) = models.iter().find(|(name, _)| name.eq_ignore_ascii_case(trimmed)) {
			return Ok((p.to_string_lossy().into_owned(), Some(p.clone())));
		}
		if let Some((_, p)) = models
			.iter()
			.find(|(name, _)| name.to_ascii_lowercase().contains(&needle))
		{
			return Ok((p.to_string_lossy().into_owned(), Some(p.clone())));
		}
		let available: Vec<String> = models.into_iter().map(|(n, _)| n).collect();
		return Err(format!(
			"Model '{model_arg}' not found (available example models: {available:?}, or 'sample')"
		));
	}
	if let Some(ref inp) = cli.input {
		if inp == "--sample" || inp.eq_ignore_ascii_case("sample") {
			return Ok(("--sample".to_owned(), None));
		}
		return Ok((inp.clone(), Some(PathBuf::from(inp))));
	}
	if let Some((_, p)) = find_example_models().into_iter().next() {
		return Ok((p.to_string_lossy().into_owned(), Some(p)));
	}
	Ok(("--sample".to_owned(), None))
}

pub(crate) fn pick_default_pad_params(puppet: &Puppet) -> [String; 2] {
	let mut vec2_params: Vec<String> = puppet
		.params
		.iter()
		.filter(|(_, p)| p.is_vec2)
		.map(|(k, _)| k.clone())
		.collect();
	vec2_params.sort();

	let pad1 = if puppet.params.contains_key("Head:: Yaw-Pitch") {
		"Head:: Yaw-Pitch".to_owned()
	} else if let Some(h) = vec2_params
		.iter()
		.find(|p| p.to_ascii_lowercase().contains("head"))
	{
		h.clone()
	} else {
		vec2_params
			.first()
			.cloned()
			.unwrap_or_else(|| "Head:: Yaw-Pitch".to_owned())
	};

	let pad2 = [
		"Body:: Yaw-Pitch",
		"Mouth:: Viseme",
		"Mouth:: Shape",
	]
	.into_iter()
	.find(|&cand| cand != pad1 && puppet.params.contains_key(cand))
	.map(str::to_owned)
	.or_else(|| {
		vec2_params
			.iter()
			.find(|p| **p != pad1 && p.to_ascii_lowercase().contains("mouth"))
			.cloned()
	})
	.or_else(|| {
		vec2_params
			.iter()
			.find(|p| **p != pad1 && p.to_ascii_lowercase().contains("body"))
			.cloned()
	})
	.or_else(|| vec2_params.iter().find(|p| **p != pad1).cloned())
	.unwrap_or_else(|| "Mouth:: Viseme".to_owned());

	[pad1, pad2]
}

pub(crate) fn clean_display_name(s: &str) -> &str {
	s.trim_end_matches(|c: char| c == '\0' || c.is_whitespace()).trim_start()
}

pub(crate) fn format_clean_zsort(z: f32) -> f32 {
	if z.abs() < 0.5 {
		0.0
	} else {
		z
	}
}

pub(crate) fn load_initial_session(input: &str) -> Result<Session, String> {
	if input == "--sample" || input == "sample" {
		let mut session = Session::new_sample();
		let _ = session.exec("rig.auto_visemes", &json::object! { node_name: "Mouth" });
		let _ = session.exec("node.synthesize_overlay", &json::object! { kind: "blush" });
		let _ = session.exec("node.synthesize_overlay", &json::object! { kind: "tears" });
		session.undo_stack.clear();
		session.redo_stack.clear();
		session.revision = 0;
		return Ok(session);
	}
	let path = Path::new(input);
	let ext = path
		.extension()
		.and_then(|s| s.to_str())
		.map(str::to_ascii_lowercase)
		.unwrap_or_default();
	if ext == "rigjson" {
		let mut session = Session::new_sample();
		session.exec("puppet.import", &json::object! { path: input })?;
		session.revision = 0;
		session.undo_stack.clear();
		session.redo_stack.clear();
		return Ok(session);
	}
	let bytes = fs::read(path).map_err(|e| format!("Failed to open '{}': {e}", path.display()))?;
	if ext == "psd" {
		let doc = parse_psd_bytes(&bytes)?;
		let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("PsdPuppet");
		let model = import_psd_to_model_with_mode(&doc, name, "delaunay", true, 8, 8);
		return Ok(Session::new(model));
	}
	let model = parse_inp(bytes.as_slice()).map_err(|e| format!("Failed to parse '{}': {e}", path.display()))?;
	Ok(Session::new(model))
}

pub(crate) fn write_inp_atomic(model: &Model, path: &Path) -> Result<(), String> {
	if let Some(parent) = path.parent() {
		if !parent.as_os_str().is_empty() {
			fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {e}"))?;
		}
	}
	let tmp_path = path.with_extension("inp.tmp");
	{
		let file = File::create(&tmp_path).map_err(|e| format!("Failed to create temp file: {e}"))?;
		let mut w = BufWriter::new(file);
		write_inp(model, &mut w).map_err(|e| format!("Failed to serialize .inp: {e}"))?;
		w.flush().map_err(|e| format!("Failed to flush .inp: {e}"))?;
	}
	if path.exists() {
		let _ = fs::remove_file(path);
	}
	fs::rename(&tmp_path, path).map_err(|e| format!("Failed to update '{}': {e}", path.display()))?;
	Ok(())
}

pub(crate) fn save_color_image_png(img: &egui::ColorImage, out: &Path) -> Result<(), String> {
	if let Some(parent) = out.parent() {
		if !parent.as_os_str().is_empty() {
			fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {e}"))?;
		}
	}
	let w = img.size[0] as u32;
	let h = img.size[1] as u32;
	let mut raw = Vec::with_capacity((w * h * 4) as usize);
	for p in &img.pixels {
		raw.extend_from_slice(&p.to_array());
	}
	let rgba = RgbaImage::from_raw(w, h, raw).ok_or_else(|| "Failed to convert screenshot image".to_owned())?;
	rgba.save(out)
		.map_err(|e| format!("Failed to write screenshot '{}': {e}", out.display()))
}

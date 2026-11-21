#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use riglens_core::model::Model;
use riglens_core::node::components::{Mesh, MeshGroup, SimplePhysics, TexturedMesh};
use json::JsonValue;

use crate::audit::audit_rig;
use crate::headless_render::{render_frame, RenderOptions};
use crate::rig::encode_rgba_png;

pub const DEFAULT_FIGCRAFT_CLI: &str = r"D:\app\FigCraft\figcraft-cli.exe";

fn base64_encode(input: &[u8]) -> String {
	const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
	let mut out = String::with_capacity(((input.len() + 2) / 3) * 4);
	for chunk in input.chunks(3) {
		let b0 = chunk[0] as u32;
		let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
		let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
		let n = (b0 << 16) | (b1 << 8) | b2;
		out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
		out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
		out.push(if chunk.len() > 1 {
			TABLE[((n >> 6) & 0x3f) as usize] as char
		} else {
			'='
		});
		out.push(if chunk.len() > 2 {
			TABLE[(n & 0x3f) as usize] as char
		} else {
			'='
		});
	}
	out
}

fn xml_escape(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for ch in s.chars() {
		match ch {
			'&' => out.push_str("&amp;"),
			'<' => out.push_str("&lt;"),
			'>' => out.push_str("&gt;"),
			'"' => out.push_str("&quot;"),
			'\'' => out.push_str("&apos;"),
			_ => out.push(ch),
		}
	}
	out
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
	let count = s.chars().count();
	if count <= max_chars {
		return s.to_owned();
	}
	let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
	out.push_str("...");
	out
}

fn find_figcraft_cli() -> Option<PathBuf> {
	if let Ok(custom) = std::env::var("FIGCRAFT_CLI") {
		let p = PathBuf::from(custom);
		if p.exists() {
			return Some(p);
		}
	}
	let default = Path::new(DEFAULT_FIGCRAFT_CLI);
	if default.exists() {
		return Some(default.to_path_buf());
	}
	None
}

/// Generate a rich studio SVG rig telemetry card & preview, write `.svg` next to `out_png`,
/// and rasterize it into `out_png` via `figcraft-cli convert --no-system-fonts` (with clean
/// preview PNG fallback if `figcraft-cli` is unavailable on this host).
pub fn generate_rig_spec_card(model: &mut Model, out_png: &Path) -> Result<JsonValue, String> {
	let audit = audit_rig(model);
	let score = audit["score"].as_u32().unwrap_or(0);
	let passed = audit["passed"].as_bool().unwrap_or(false);

	let preview = render_frame(
		model,
		&HashMap::new(),
		&RenderOptions {
			width: 320,
			height: 320,
			scale: 0.95,
			wireframe: false,
			dt: 0.0,
			background: [0, 0, 0, 0],
			..RenderOptions::default()
		},
	)?;
	let preview_tex = encode_rgba_png(&preview);
	let preview_b64 = base64_encode(&preview_tex.data);

	let puppet_name = model
		.puppet
		.meta
		.name
		.as_deref()
		.map(str::trim)
		.filter(|s| !s.is_empty())
		.unwrap_or("Unnamed Puppet");

	let mut part_count = 0usize;
	let mut cage_count = 0usize;
	let mut phys_count = 0usize;
	let mut vert_count = 0usize;
	let mut tri_count = 0usize;

	for n in model.puppet.nodes.iter() {
		if model.puppet.node_comps.get::<TexturedMesh>(n.uuid).is_some() {
			part_count += 1;
		}
		if model.puppet.node_comps.get::<MeshGroup>(n.uuid).is_some() {
			cage_count += 1;
		}
		if model.puppet.node_comps.get::<SimplePhysics>(n.uuid).is_some() {
			phys_count += 1;
		}
		if let Some(m) = model.puppet.node_comps.get::<Mesh>(n.uuid) {
			vert_count += m.vertices.len();
			tri_count += m.indices.len() / 3;
		}
	}

	let param_count = model.puppet.params.len();
	let anim_count = model.puppet.animations.len();

	let mut param_names: Vec<String> = model
		.puppet
		.params
		.iter()
		.map(|(name, p)| {
			if p.is_vec2 {
				format!("{name} (2D)")
			} else {
				name.clone()
			}
		})
		.collect();
	param_names.sort();

	let badge_bg = if passed { "#064E3B" } else { "#4C0519" };
	let badge_stroke = if passed { "#10B981" } else { "#F43F5E" };
	let badge_fg = if passed { "#6EE7B7" } else { "#FDA4AF" };
	let badge_label = if passed { "PASS" } else { "NEEDS FIX" };

	let audit_rows = [
		("Containment Leaks", audit["summary"]["containment_leaks"].as_usize().unwrap_or(0)),
		("Mesh Foldovers", audit["summary"]["mesh_foldovers"].as_usize().unwrap_or(0)),
		("Extreme Stretch", audit["summary"]["mesh_stretch"].as_usize().unwrap_or(0)),
		("Blink Symmetry", audit["summary"]["blink_issues"].as_usize().unwrap_or(0)),
		("Yaw Symmetry", audit["summary"]["symmetry_defects"].as_usize().unwrap_or(0)),
		("Unmasked Pupils", audit["summary"]["unmasked_parts"].as_usize().unwrap_or(0)),
	];

	let mut audit_svg = String::new();
	for (idx, (label, count)) in audit_rows.iter().enumerate() {
		let col = idx % 2;
		let row = idx / 2;
		let bx = 412 + col * 218;
		let by = 204 + row * 38;
		let ok = *count == 0;
		let dot_col = if ok { "#10B981" } else { "#F43F5E" };
		let pill_bg = if ok { "#064E3B" } else { "#4C0519" };
		let pill_fg = if ok { "#6EE7B7" } else { "#FDA4AF" };
		let pill_txt = if ok {
			"OK".to_owned()
		} else {
			format!("{count}")
		};
		audit_svg.push_str(&format!(
			"<rect x=\"{bx}\" y=\"{by}\" width=\"206\" height=\"30\" rx=\"7\" fill=\"#0F172A\" stroke=\"#334155\" stroke-width=\"1\"/>\
			 <circle cx=\"{}\" cy=\"{}\" r=\"4\" fill=\"{dot_col}\"/>\
			 <text x=\"{}\" y=\"{}\" fill=\"#E2E8F0\" font-family=\"sans-serif\" font-size=\"11\" font-weight=\"600\">{}</text>\
			 <rect x=\"{}\" y=\"{}\" width=\"44\" height=\"18\" rx=\"9\" fill=\"{pill_bg}\"/>\
			 <text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"{pill_fg}\" font-family=\"monospace\" font-size=\"10\" font-weight=\"700\">{pill_txt}</text>",
			bx + 14,
			by + 15,
			bx + 26,
			by + 19,
			xml_escape(label),
			bx + 154,
			by + 6,
			bx + 176,
			by + 19,
		));
	}

	let mut param_svg = String::new();
	for (idx, pname) in param_names.iter().take(6).enumerate() {
		let col = idx % 2;
		let row = idx / 2;
		let px = 412 + col * 218;
		let py = 376 + row * 26;
		param_svg.push_str(&format!(
			"<rect x=\"{px}\" y=\"{py}\" width=\"206\" height=\"20\" rx=\"5\" fill=\"#0F172A\" stroke=\"#334155\" stroke-width=\"1\"/>\
			 <text x=\"{}\" y=\"{}\" fill=\"#CBD5E1\" font-family=\"monospace\" font-size=\"10\">{}</text>",
			px + 10,
			py + 14,
			xml_escape(&truncate_chars(pname, 26)),
		));
	}
	let extra_params = param_names.len().saturating_sub(6);
	let param_subtitle = if extra_params > 0 {
		format!("PARAMETERS ({param_count} total, +{extra_params} more)")
	} else {
		format!("PARAMETERS ({param_count} total)")
	};

	let rec_text = audit["recommendations"]
		.members()
		.next()
		.and_then(|v| v.as_str())
		.unwrap_or("Ready for runtime export and OpenGL preview.");

	let svg = format!(
		"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"880\" height=\"520\" viewBox=\"0 0 880 520\">\
		  <defs>\
		    <linearGradient id=\"hdrAccent\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"0\">\
		      <stop offset=\"0%\" stop-color=\"#38BDF8\"/>\
		      <stop offset=\"100%\" stop-color=\"#818CF8\"/>\
		    </linearGradient>\
		    <pattern id=\"checker\" width=\"16\" height=\"16\" patternUnits=\"userSpaceOnUse\">\
		      <rect width=\"16\" height=\"16\" fill=\"#111827\"/>\
		      <rect width=\"8\" height=\"8\" fill=\"#172033\"/>\
		      <rect x=\"8\" y=\"8\" width=\"8\" height=\"8\" fill=\"#172033\"/>\
		    </pattern>\
		    <clipPath id=\"stageClip\">\
		      <rect x=\"24\" y=\"88\" width=\"350\" height=\"360\" rx=\"14\"/>\
		    </clipPath>\
		  </defs>\
		  <rect width=\"880\" height=\"520\" rx=\"16\" fill=\"#0F172A\"/>\
		  <rect x=\"1\" y=\"1\" width=\"878\" height=\"518\" rx=\"15\" fill=\"none\" stroke=\"#1E293B\" stroke-width=\"2\"/>\
		  <rect x=\"24\" y=\"16\" width=\"832\" height=\"3\" rx=\"1.5\" fill=\"url(#hdrAccent)\"/>\
		  <text x=\"24\" y=\"38\" fill=\"#38BDF8\" font-family=\"monospace\" font-size=\"12\" font-weight=\"700\">INOX2D-MCP  RIGGING SPEC CARD</text>\
		  <text x=\"24\" y=\"66\" fill=\"#F8FAFC\" font-family=\"sans-serif\" font-size=\"24\" font-weight=\"700\">{}</text>\
		  <rect x=\"672\" y=\"26\" width=\"184\" height=\"42\" rx=\"10\" fill=\"{badge_bg}\" stroke=\"{badge_stroke}\" stroke-width=\"1.5\"/>\
		  <text x=\"764\" y=\"52\" text-anchor=\"middle\" fill=\"{badge_fg}\" font-family=\"monospace\" font-size=\"14\" font-weight=\"700\">{score}/100 {badge_label}</text>\
		  <g clip-path=\"url(#stageClip)\">\
		    <rect x=\"24\" y=\"88\" width=\"350\" height=\"360\" fill=\"url(#checker)\"/>\
		    <image href=\"data:image/png;base64,{preview_b64}\" x=\"39\" y=\"96\" width=\"320\" height=\"320\"/>\
		    <rect x=\"24\" y=\"416\" width=\"350\" height=\"32\" fill=\"#0B1120\"/>\
		    <text x=\"199\" y=\"436\" text-anchor=\"middle\" fill=\"#94A3B8\" font-family=\"monospace\" font-size=\"11\">{part_count} Parts  |  {vert_count} Verts  |  {tri_count} Tris</text>\
		  </g>\
		  <rect x=\"24\" y=\"88\" width=\"350\" height=\"360\" rx=\"14\" fill=\"none\" stroke=\"#334155\" stroke-width=\"1.5\"/>\
		  <rect x=\"396\" y=\"88\" width=\"108\" height=\"64\" rx=\"10\" fill=\"#1E293B\" stroke=\"#334155\" stroke-width=\"1\"/>\
		  <text x=\"410\" y=\"110\" fill=\"#94A3B8\" font-family=\"monospace\" font-size=\"10\">PARTS / CAGES</text>\
		  <text x=\"410\" y=\"138\" fill=\"#F8FAFC\" font-family=\"monospace\" font-size=\"20\" font-weight=\"700\">{part_count} / {cage_count}</text>\
		  <rect x=\"512\" y=\"88\" width=\"108\" height=\"64\" rx=\"10\" fill=\"#1E293B\" stroke=\"#334155\" stroke-width=\"1\"/>\
		  <text x=\"526\" y=\"110\" fill=\"#94A3B8\" font-family=\"monospace\" font-size=\"10\">TRIANGLES</text>\
		  <text x=\"526\" y=\"138\" fill=\"#38BDF8\" font-family=\"monospace\" font-size=\"20\" font-weight=\"700\">{tri_count}</text>\
		  <rect x=\"628\" y=\"88\" width=\"108\" height=\"64\" rx=\"10\" fill=\"#1E293B\" stroke=\"#334155\" stroke-width=\"1\"/>\
		  <text x=\"642\" y=\"110\" fill=\"#94A3B8\" font-family=\"monospace\" font-size=\"10\">PARAMS</text>\
		  <text x=\"642\" y=\"138\" fill=\"#818CF8\" font-family=\"monospace\" font-size=\"20\" font-weight=\"700\">{param_count}</text>\
		  <rect x=\"744\" y=\"88\" width=\"112\" height=\"64\" rx=\"10\" fill=\"#1E293B\" stroke=\"#334155\" stroke-width=\"1\"/>\
		  <text x=\"758\" y=\"110\" fill=\"#94A3B8\" font-family=\"monospace\" font-size=\"10\">PHYS / ANIM</text>\
		  <text x=\"758\" y=\"138\" fill=\"#34D399\" font-family=\"monospace\" font-size=\"20\" font-weight=\"700\">{phys_count} / {anim_count}</text>\
		  <rect x=\"396\" y=\"166\" width=\"460\" height=\"162\" rx=\"12\" fill=\"#1E293B\" stroke=\"#334155\" stroke-width=\"1\"/>\
		  <text x=\"412\" y=\"190\" fill=\"#38BDF8\" font-family=\"monospace\" font-size=\"11\" font-weight=\"700\">6-POINT RIG AUDIT BREAKDOWN</text>\
		  {audit_svg}\
		  <rect x=\"396\" y=\"342\" width=\"460\" height=\"122\" rx=\"12\" fill=\"#1E293B\" stroke=\"#334155\" stroke-width=\"1\"/>\
		  <text x=\"412\" y=\"364\" fill=\"#818CF8\" font-family=\"monospace\" font-size=\"11\" font-weight=\"700\">{param_subtitle}</text>\
		  {param_svg}\
		  <rect x=\"24\" y=\"476\" width=\"832\" height=\"28\" rx=\"8\" fill=\"#1E293B\" stroke=\"#334155\" stroke-width=\"1\"/>\
		  <text x=\"36\" y=\"494\" fill=\"#CBD5E1\" font-family=\"sans-serif\" font-size=\"11\">{}</text>\
		  <text x=\"844\" y=\"494\" text-anchor=\"end\" fill=\"#64748B\" font-family=\"monospace\" font-size=\"10\">FigCraft + Inox2D</text>\
		</svg>",
		xml_escape(&truncate_chars(puppet_name, 30)),
		xml_escape(&truncate_chars(rec_text, 108)),
	);

	let svg_path = out_png.with_extension("svg");
	if let Some(parent) = out_png.parent() {
		if !parent.as_os_str().is_empty() {
			fs::create_dir_all(parent).map_err(|e| format!("Failed to create output dir: {e}"))?;
		}
	}
	fs::write(&svg_path, svg.as_bytes()).map_err(|e| format!("Failed to write SVG '{}': {e}", svg_path.display()))?;

	let mut rendered_via = "preview_fallback";
	let mut figcraft_stdout = String::new();
	if let Some(cli) = find_figcraft_cli() {
		let out = Command::new(&cli)
			.arg("convert")
			.arg("--no-system-fonts")
			.arg(&svg_path)
			.arg(out_png)
			.output()
			.map_err(|e| format!("Failed to invoke FigCraft CLI ({}): {e}", cli.display()))?;
		if out.status.success() && out_png.exists() {
			rendered_via = "figcraft-cli";
			figcraft_stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
		} else {
			let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
			return Err(format!(
				"FigCraft CLI failed (status {:?}): {stderr}",
				out.status.code()
			));
		}
	} else {
		preview
			.save(out_png)
			.map_err(|e| format!("Failed to save fallback preview PNG '{}': {e}", out_png.display()))?;
	}

	Ok(json::object! {
		out_png: out_png.display().to_string(),
		svg_path: svg_path.display().to_string(),
		puppet: puppet_name,
		audit_score: score,
		passed: passed,
		parts: part_count,
		cages: cage_count,
		vertices: vert_count,
		triangles: tri_count,
		parameters: param_count,
		rendered_via: rendered_via,
		figcraft_stdout: figcraft_stdout
	})
}

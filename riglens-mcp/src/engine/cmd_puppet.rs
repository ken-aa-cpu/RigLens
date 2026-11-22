use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

use riglens_core::formats::inp::write_inp_atomic;
use riglens_core::formats::ir_bridge::{ir_to_model, model_to_ir};
use riglens_core::node::components::RawNodeData;
use json::JsonValue;
use riglens_format::{ExportOptions, ImportOptions};

use crate::psd::{
	export_model_to_psd, import_psd_to_model_with_mode, parse_psd_bytes, photocraft_convert, photocraft_inspect,
	sync_psd_textures_into_model,
};
use crate::rig::build_sample_vtuber_model;

use super::{caps_list_to_json, diagnostics_to_json, Session};

impl Session {
	pub(super) fn exec_puppet_cmd(&mut self, cmd: &str, params: &JsonValue) -> Result<JsonValue, String> {
		match cmd {
			"puppet.info" => Ok(self.cmd_info()),
			"puppet.new_sample" => {
				self.model = build_sample_vtuber_model();
				self.param_overrides.clear();
				self.revision += 1;
				Ok(self.cmd_info())
			}
			"puppet.save" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				write_inp_atomic(&self.model, Path::new(path))?;
				Ok(json::object! {
					saved: path,
					textures: self.model.textures.len(),
					nodes: self.model.puppet.nodes.iter().count(),
					params: self.model.puppet.params.len()
				})
			}
			"puppet.import_psd" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				let bytes = fs::read(Path::new(path)).map_err(|e| format!("Failed to read '{path}': {e}"))?;
				let psd = parse_psd_bytes(&bytes)?;
				let model_name = params["name"]
					.as_str()
					.map(str::to_owned)
					.unwrap_or_else(|| {
						Path::new(path)
							.file_stem()
							.and_then(|s| s.to_str())
							.unwrap_or("PsdPuppet")
							.to_owned()
					});
				let mesh_mode = if let Some(m) = params["mesh_mode"].as_str() {
					m
				} else if params["contour_mesh"].as_bool().unwrap_or(true) {
					"contour"
				} else {
					"grid"
				};
				let auto_rig = params["auto_rig"].as_bool().unwrap_or(true);
				let cols = params["cols"].as_usize().unwrap_or(8);
				let rows = params["rows"].as_usize().unwrap_or(8);
				self.model = import_psd_to_model_with_mode(&psd, &model_name, mesh_mode, auto_rig, cols, rows);
				self.param_overrides.clear();
				self.revision += 1;
				Ok(self.cmd_info())
			}
			"puppet.export_psd" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				let width = params["width"].as_u32().unwrap_or(1024);
				let height = params["height"].as_u32().unwrap_or(1024);
				let psd_bytes = export_model_to_psd(&mut self.model, width, height)?;
				fs::write(Path::new(path), &psd_bytes).map_err(|e| format!("Failed to write '{path}': {e}"))?;
				Ok(json::object! {
					saved: path,
					width: width,
					height: height,
					bytes: psd_bytes.len()
				})
			}
			"psd.sync_textures" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				let bytes = fs::read(Path::new(path)).map_err(|e| format!("Failed to read '{path}': {e}"))?;
				let psd = parse_psd_bytes(&bytes)?;
				let updated_parts = sync_psd_textures_into_model(&mut self.model, &psd)?;
				self.revision += 1;
				Ok(json::object! {
					path: path,
					updated_parts: updated_parts
				})
			}
			"format.list" => {
				let reg = crate::formats::default_registry();
				Ok(json::object! {
					importers: caps_list_to_json(&reg.list_importers()),
					exporters: caps_list_to_json(&reg.list_exporters()),
				})
			}
			"puppet.export" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				let fmt_key = params["format"]
					.as_str()
					.or_else(|| Path::new(path).extension().and_then(|s| s.to_str()))
					.unwrap_or("inp");
				let reg = crate::formats::default_registry();
				let resolved_id = reg
					.resolve_exporter(fmt_key)
					.ok_or_else(|| format!("Unsupported export format '{fmt_key}'"))?
					.info()
					.id;
				let mut options = ExportOptions {
					pretty: params["pretty"].as_bool().unwrap_or(true),
					strict: params["strict"].as_bool().unwrap_or(false),
					..ExportOptions::default()
				};
				if let Some(w) = params["width"].as_u64() {
					options.custom.insert("width".to_owned(), serde_json::json!(w));
				}
				if let Some(h) = params["height"].as_u64() {
					options.custom.insert("height".to_owned(), serde_json::json!(h));
				}
				let doc = model_to_ir(&self.model);
				let file = File::create(Path::new(path)).map_err(|e| format!("Failed to create '{path}': {e}"))?;
				let mut writer = BufWriter::new(file);
				let report = reg
					.export_to_writer(&resolved_id, &doc, &mut writer, &options)
					.map_err(|e| e.to_string())?;
				Ok(json::object! {
					saved: path,
					format: resolved_id,
					bytes_written: report.bytes_written,
					warnings: diagnostics_to_json(&report.warnings),
				})
			}
			"puppet.import" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				let fmt_key = params["format"]
					.as_str()
					.or_else(|| Path::new(path).extension().and_then(|s| s.to_str()))
					.unwrap_or("inp");
				let reg = crate::formats::default_registry();
				let file = File::open(Path::new(path)).map_err(|e| format!("Failed to open '{path}': {e}"))?;
				let mut reader = std::io::BufReader::new(file);
				let report = reg
					.import_from_reader(fmt_key, &mut reader, &ImportOptions::default())
					.map_err(|e| e.to_string())?;
				let model = ir_to_model(&report.document).map_err(|e| e.to_string())?;
				self.model = model;
				self.param_overrides.clear();
				self.revision += 1;
				Ok(json::object! {
					info: self.cmd_info(),
					warnings: diagnostics_to_json(&report.warnings),
				})
			}
			"photocraft.inspect" => {
				let path = params["path"]
					.as_str()
					.ok_or_else(|| "Missing 'path'".to_owned())?;
				photocraft_inspect(Path::new(path))
			}
			"photocraft.convert" => {
				let in_path = params["in_path"]
					.as_str()
					.ok_or_else(|| "Missing 'in_path'".to_owned())?;
				let out_path = params["out_path"]
					.as_str()
					.ok_or_else(|| "Missing 'out_path'".to_owned())?;
				let msg = photocraft_convert(Path::new(in_path), Path::new(out_path))?;
				Ok(json::object! { status: msg })
			}
			"puppet.diagnostics" => {
				let raw_nodes: Vec<JsonValue> = self
					.model
					.puppet
					.nodes
					.pre_order_iter()
					.filter_map(|n| {
						self.model.puppet.node_comps.get::<RawNodeData>(n.uuid).map(|raw| {
							json::object! {
								uuid: n.uuid.0,
								name: n.name.clone(),
								type_name: raw.type_name.clone(),
								field_count: raw.fields.len(),
							}
						})
					})
					.collect();
				let top_keys: Vec<String> = self
					.model
					.puppet
					.top_extras
					.iter()
					.map(|(k, _)| k.to_owned())
					.collect();
				Ok(json::object! {
					warnings: self.model.puppet.diagnostics.clone(),
					raw_nodes: raw_nodes,
					param_extra_count: self.model.puppet.param_extras.len(),
					top_extra_keys: top_keys,
				})
			}
			_ => Err(format!("Unknown command: '{cmd}'")),
		}
	}
}

use std::io::{Read, Write};

use riglens_core::formats::ir_bridge::{ir_to_model, model_to_ir, InpFormat};
use riglens_format::{
	ExportOptions, ExportReport, Exporter, FormatCapabilities, FormatError, FormatRegistry, ImportOptions, ImportReport,
	Importer, RigDocument, RigJsonFormat,
};

use crate::psd::{export_model_to_psd, import_psd_to_model_with_mode, parse_psd_bytes};

/// Format plugin for layered Photoshop `.psd` artwork import and export.
pub struct PsdFormat;

impl PsdFormat {
	pub fn capabilities() -> FormatCapabilities {
		FormatCapabilities {
			id: "psd".to_owned(),
			display_name: "Layered Photoshop Document (.psd)".to_owned(),
			extensions: vec!["psd".to_owned()],
			supports_grid_deformer: false,
			supports_path_deformer: false,
			supports_cubic_interpolation: false,
			supports_animations: false,
			supports_opaque_passthrough: false,
			supports_textures: true,
			supports_vendor_data: false,
		}
	}
}

impl Importer for PsdFormat {
	fn info(&self) -> FormatCapabilities {
		Self::capabilities()
	}

	fn import(&self, reader: &mut dyn Read, options: &ImportOptions) -> Result<ImportReport, FormatError> {
		let mut buf = Vec::new();
		reader.read_to_end(&mut buf)?;
		let psd = parse_psd_bytes(&buf).map_err(FormatError::InvalidInput)?;

		let name = options
			.custom
			.get("name")
			.and_then(|v| v.as_str())
			.unwrap_or("PsdPuppet");
		let mesh_mode = options
			.custom
			.get("mesh_mode")
			.and_then(|v| v.as_str())
			.unwrap_or("contour");
		let auto_rig = options
			.custom
			.get("auto_rig")
			.and_then(|v| v.as_bool())
			.unwrap_or(true);
		let cols = options
			.custom
			.get("cols")
			.and_then(|v| v.as_u64())
			.map(|v| v as usize)
			.unwrap_or(8);
		let rows = options
			.custom
			.get("rows")
			.and_then(|v| v.as_u64())
			.map(|v| v as usize)
			.unwrap_or(8);

		let model = import_psd_to_model_with_mode(&psd, name, mesh_mode, auto_rig, cols, rows);
		let document = model_to_ir(&model);
		Ok(ImportReport {
			warnings: document.diagnostics.clone(),
			document,
		})
	}
}

impl Exporter for PsdFormat {
	fn info(&self) -> FormatCapabilities {
		Self::capabilities()
	}

	fn export(
		&self,
		doc: &RigDocument,
		writer: &mut dyn Write,
		options: &ExportOptions,
	) -> Result<ExportReport, FormatError> {
		let mut model = ir_to_model(doc)?;
		let width = options
			.custom
			.get("width")
			.and_then(|v| v.as_u64())
			.map(|v| v as u32)
			.unwrap_or(1024);
		let height = options
			.custom
			.get("height")
			.and_then(|v| v.as_u64())
			.map(|v| v as u32)
			.unwrap_or(1024);

		let bytes = export_model_to_psd(&mut model, width, height).map_err(FormatError::Custom)?;
		writer.write_all(&bytes)?;
		Ok(ExportReport {
			bytes_written: bytes.len(),
			warnings: Vec::new(),
		})
	}
}

/// Build the default [`FormatRegistry`] containing `.inp`/`.inx`, `.rigjson`, and `.psd`.
pub fn default_registry() -> FormatRegistry {
	let mut reg = FormatRegistry::new();
	reg.register_importer(Box::new(InpFormat));
	reg.register_exporter(Box::new(InpFormat));
	reg.register_importer(Box::new(RigJsonFormat));
	reg.register_exporter(Box::new(RigJsonFormat));
	reg.register_importer(Box::new(PsdFormat));
	reg.register_exporter(Box::new(PsdFormat));
	reg
}

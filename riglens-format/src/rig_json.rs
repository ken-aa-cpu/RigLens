use std::io::{Read, Write};

use crate::ir::RigDocument;
use crate::plugin::{
	ExportOptions, ExportReport, Exporter, FormatCapabilities, FormatError, ImportOptions, ImportReport, Importer,
};

/// Built-in lossless JSON codec for the neutral [`RigDocument`] intermediate representation.
pub struct RigJsonFormat;

impl RigJsonFormat {
	pub fn capabilities() -> FormatCapabilities {
		FormatCapabilities {
			id: "rig-json".to_owned(),
			display_name: "RigLens Neutral JSON IR".to_owned(),
			extensions: vec!["rigjson".to_owned(), "json".to_owned()],
			supports_grid_deformer: true,
			supports_path_deformer: true,
			supports_cubic_interpolation: true,
			supports_animations: true,
			supports_opaque_passthrough: true,
			supports_textures: true,
			supports_vendor_data: true,
		}
	}
}

impl Importer for RigJsonFormat {
	fn info(&self) -> FormatCapabilities {
		Self::capabilities()
	}

	fn import(&self, reader: &mut dyn Read, _options: &ImportOptions) -> Result<ImportReport, FormatError> {
		let mut buf = Vec::new();
		reader.read_to_end(&mut buf)?;
		let document: RigDocument =
			serde_json::from_slice(&buf).map_err(|e| FormatError::InvalidInput(format!("invalid rig-json: {e}")))?;
		document.validate().map_err(FormatError::ValidationFailed)?;
		let warnings = document.diagnostics.clone();
		Ok(ImportReport { document, warnings })
	}
}

impl Exporter for RigJsonFormat {
	fn info(&self) -> FormatCapabilities {
		Self::capabilities()
	}

	fn export(
		&self,
		doc: &RigDocument,
		writer: &mut dyn Write,
		options: &ExportOptions,
	) -> Result<ExportReport, FormatError> {
		let bytes = if options.pretty {
			serde_json::to_vec_pretty(doc)
		} else {
			serde_json::to_vec(doc)
		}
		.map_err(|e| FormatError::Custom(format!("failed to serialize rig-json: {e}")))?;
		writer.write_all(&bytes)?;
		Ok(ExportReport {
			bytes_written: bytes.len(),
			warnings: Vec::new(),
		})
	}
}

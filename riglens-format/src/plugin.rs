use std::collections::BTreeMap;
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use crate::ir::{Diagnostic, NodeKind, RigDocument};

/// Declares what a format plugin supports so exporters and callers can inspect
/// capabilities and catch lossy conversions before writing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatCapabilities {
	pub id: String,
	pub display_name: String,
	pub extensions: Vec<String>,
	pub supports_grid_deformer: bool,
	pub supports_path_deformer: bool,
	pub supports_cubic_interpolation: bool,
	pub supports_animations: bool,
	pub supports_opaque_passthrough: bool,
	pub supports_textures: bool,
	pub supports_vendor_data: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExportOptions {
	pub pretty: bool,
	/// When true, [`FormatRegistry::export_to_writer`] fails with
	/// [`FormatError::CapabilityRejected`] if any capability warning would occur.
	pub strict: bool,
	#[serde(default)]
	pub custom: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ImportOptions {
	#[serde(default)]
	pub custom: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExportReport {
	pub bytes_written: usize,
	pub warnings: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportReport {
	pub document: RigDocument,
	pub warnings: Vec<Diagnostic>,
}

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
	#[error("I/O error: {0}")]
	Io(#[from] std::io::Error),
	#[error("unsupported format {0:?}")]
	UnsupportedFormat(String),
	#[error("invalid input: {0}")]
	InvalidInput(String),
	#[error("rig validation failed: {0:?}")]
	ValidationFailed(Vec<String>),
	#[error("export rejected under strict mode due to {} capability warning(s)", .0.len())]
	CapabilityRejected(Vec<Diagnostic>),
	#[error("{0}")]
	Custom(String),
}

/// Pluggable importer reading an external file format into a neutral [`RigDocument`].
pub trait Importer: Send + Sync {
	fn info(&self) -> FormatCapabilities;

	fn import(&self, reader: &mut dyn Read, options: &ImportOptions) -> Result<ImportReport, FormatError>;

	fn import_bytes(&self, bytes: &[u8], options: &ImportOptions) -> Result<ImportReport, FormatError> {
		let mut cursor = std::io::Cursor::new(bytes);
		self.import(&mut cursor, options)
	}
}

/// Pluggable exporter emitting a neutral [`RigDocument`] into a target format.
pub trait Exporter: Send + Sync {
	fn info(&self) -> FormatCapabilities;

	fn export(
		&self,
		doc: &RigDocument,
		writer: &mut dyn Write,
		options: &ExportOptions,
	) -> Result<ExportReport, FormatError>;

	fn export_bytes(&self, doc: &RigDocument, options: &ExportOptions) -> Result<(Vec<u8>, ExportReport), FormatError> {
		let mut buf = Vec::new();
		let report = self.export(doc, &mut buf, options)?;
		Ok((buf, report))
	}
}

/// Inspect a [`RigDocument`] against an exporter's declared [`FormatCapabilities`]
/// and report every feature that would be dropped or downgraded.
pub fn check_export_compatibility(doc: &RigDocument, caps: &FormatCapabilities) -> Vec<Diagnostic> {
	let mut warnings = Vec::new();

	for node in &doc.nodes {
		match &node.kind {
			NodeKind::GridDeformer(_) if !caps.supports_grid_deformer => {
				warnings.push(Diagnostic::warning(
					"UNSUPPORTED_GRID_DEFORMER",
					Some(node.id),
					format!(
						"format {:?} does not support GridDeformer (node {:?} #{})",
						caps.id, node.name, node.id
					),
				));
			}
			NodeKind::PathDeformer(_) if !caps.supports_path_deformer => {
				warnings.push(Diagnostic::warning(
					"UNSUPPORTED_PATH_DEFORMER",
					Some(node.id),
					format!(
						"format {:?} does not support PathDeformer (node {:?} #{})",
						caps.id, node.name, node.id
					),
				));
			}
			NodeKind::Opaque { type_name, .. } if !caps.supports_opaque_passthrough => {
				warnings.push(Diagnostic::warning(
					"UNSUPPORTED_OPAQUE_NODE",
					Some(node.id),
					format!(
						"format {:?} does not preserve opaque node type {:?} ({:?} #{})",
						caps.id, type_name, node.name, node.id
					),
				));
			}
			_ => {}
		}
	}

	if !caps.supports_cubic_interpolation {
		let has_cubic_param = doc.params.iter().any(|p| {
			p.bindings
				.iter()
				.any(|b| b.interpolate_mode.eq_ignore_ascii_case("cubic") || b.interpolate_mode.eq_ignore_ascii_case("bezier"))
		});
		let has_cubic_anim = doc.animations.values().any(|a| {
			a.lanes
				.iter()
				.any(|l| l.interpolation.eq_ignore_ascii_case("cubic") || l.interpolation.eq_ignore_ascii_case("bezier"))
		});
		if has_cubic_param || has_cubic_anim {
			warnings.push(Diagnostic::warning(
				"UNSUPPORTED_CUBIC_INTERP",
				None,
				format!(
					"format {:?} does not preserve Cubic/Bezier interpolation modes",
					caps.id
				),
			));
		}
	}

	if !caps.supports_animations && !doc.animations.is_empty() {
		warnings.push(Diagnostic::warning(
			"UNSUPPORTED_ANIMATIONS",
			None,
			format!(
				"format {:?} will drop {} animation(s)",
				caps.id,
				doc.animations.len()
			),
		));
	}

	if !caps.supports_textures && !doc.textures.is_empty() {
		warnings.push(Diagnostic::warning(
			"UNSUPPORTED_TEXTURES",
			None,
			format!(
				"format {:?} will omit {} embedded texture(s)",
				caps.id,
				doc.textures.len()
			),
		));
	}

	if !caps.supports_vendor_data && !doc.vendors.is_empty() {
		warnings.push(Diagnostic::warning(
			"UNSUPPORTED_VENDOR_DATA",
			None,
			format!(
				"format {:?} will drop {} vendor extension block(s)",
				caps.id,
				doc.vendors.len()
			),
		));
	}

	warnings
}

/// Validate `doc`, check `exporter` capabilities (rejecting under `options.strict`), and write output.
pub fn export_checked(
	exporter: &dyn Exporter,
	doc: &RigDocument,
	writer: &mut dyn Write,
	options: &ExportOptions,
) -> Result<ExportReport, FormatError> {
	doc.validate().map_err(FormatError::ValidationFailed)?;
	let compat_warnings = check_export_compatibility(doc, &exporter.info());
	if options.strict && !compat_warnings.is_empty() {
		return Err(FormatError::CapabilityRejected(compat_warnings));
	}
	let mut report = exporter.export(doc, writer, options)?;
	if !compat_warnings.is_empty() {
		let mut merged = compat_warnings;
		merged.extend(report.warnings);
		report.warnings = merged;
	}
	Ok(report)
}

/// Registry holding built-in and third-party format plugins.
#[derive(Default)]
pub struct FormatRegistry {
	importers: Vec<Box<dyn Importer>>,
	exporters: Vec<Box<dyn Exporter>>,
}

fn norm_ext(ext: &str) -> &str {
	ext.trim().trim_start_matches('.')
}

impl FormatRegistry {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn register_importer(&mut self, imp: Box<dyn Importer>) {
		let id = imp.info().id;
		self.importers.retain(|existing| existing.info().id != id);
		self.importers.push(imp);
	}

	pub fn register_exporter(&mut self, exp: Box<dyn Exporter>) {
		let id = exp.info().id;
		self.exporters.retain(|existing| existing.info().id != id);
		self.exporters.push(exp);
	}

	pub fn importer_by_id(&self, id: &str) -> Option<&dyn Importer> {
		self.importers
			.iter()
			.find(|i| i.info().id.eq_ignore_ascii_case(id))
			.map(|i| i.as_ref())
	}

	pub fn exporter_by_id(&self, id: &str) -> Option<&dyn Exporter> {
		self.exporters
			.iter()
			.find(|e| e.info().id.eq_ignore_ascii_case(id))
			.map(|e| e.as_ref())
	}

	pub fn importer_for_extension(&self, ext: &str) -> Option<&dyn Importer> {
		let ext = norm_ext(ext);
		self.importers
			.iter()
			.find(|i| i.info().extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
			.map(|i| i.as_ref())
	}

	pub fn exporter_for_extension(&self, ext: &str) -> Option<&dyn Exporter> {
		let ext = norm_ext(ext);
		self.exporters
			.iter()
			.find(|e| e.info().extensions.iter().any(|x| x.eq_ignore_ascii_case(ext)))
			.map(|e| e.as_ref())
	}

	pub fn resolve_importer(&self, id_or_ext: &str) -> Option<&dyn Importer> {
		self.importer_by_id(id_or_ext)
			.or_else(|| self.importer_for_extension(id_or_ext))
	}

	pub fn resolve_exporter(&self, id_or_ext: &str) -> Option<&dyn Exporter> {
		self.exporter_by_id(id_or_ext)
			.or_else(|| self.exporter_for_extension(id_or_ext))
	}

	pub fn list_importers(&self) -> Vec<FormatCapabilities> {
		self.importers.iter().map(|i| i.info()).collect()
	}

	pub fn list_exporters(&self) -> Vec<FormatCapabilities> {
		self.exporters.iter().map(|e| e.info()).collect()
	}

	/// Validate `doc`, verify capabilities against the resolved exporter, and write output.
	pub fn export_to_writer(
		&self,
		id_or_ext: &str,
		doc: &RigDocument,
		writer: &mut dyn Write,
		options: &ExportOptions,
	) -> Result<ExportReport, FormatError> {
		let exporter = self
			.resolve_exporter(id_or_ext)
			.ok_or_else(|| FormatError::UnsupportedFormat(id_or_ext.to_owned()))?;
		export_checked(exporter, doc, writer, options)
	}

	/// Export to an in-memory buffer through [`Self::export_to_writer`].
	pub fn export_bytes(
		&self,
		id_or_ext: &str,
		doc: &RigDocument,
		options: &ExportOptions,
	) -> Result<(Vec<u8>, ExportReport), FormatError> {
		let mut buf = Vec::new();
		let report = self.export_to_writer(id_or_ext, doc, &mut buf, options)?;
		Ok((buf, report))
	}

	/// Resolve an importer and decode from `reader`, validating the resulting [`RigDocument`].
	pub fn import_from_reader(
		&self,
		id_or_ext: &str,
		reader: &mut dyn Read,
		options: &ImportOptions,
	) -> Result<ImportReport, FormatError> {
		let importer = self
			.resolve_importer(id_or_ext)
			.ok_or_else(|| FormatError::UnsupportedFormat(id_or_ext.to_owned()))?;
		let report = importer.import(reader, options)?;
		report.document.validate().map_err(FormatError::ValidationFailed)?;
		Ok(report)
	}
}

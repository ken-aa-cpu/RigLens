#![forbid(unsafe_code)]

pub mod base64;
pub mod conformance;
pub mod ir;
pub mod plugin;
pub mod rig_json;

pub use ir::{
	AnimatedPartData, BindingGrid, BindingTarget, BlendData, CompositeData, Diagnostic, DriverData,
	DynamicCompositeData, GridDeformerData, MaskData, MaskEntry, MaskNodeData, MeshData, MeshGroupData, NodeKind,
	PartData, PathDeformerData, PhysicsWorld, PoseGroup, RigAnimation, RigAnimationLane, RigBinding, RigDocument,
	RigKeyframe, RigMeta, RigNode, RigParam, RigTransform, RigUsageRights, Severity, ShapesData, SimplePhysicsData,
	TextureBlob, TextureEncoding, TextureSlots, VendorExtension, IR_SCHEMA_VERSION,
};
pub use plugin::{
	check_export_compatibility, export_checked, ExportOptions, ExportReport, Exporter, FormatCapabilities, FormatError,
	FormatRegistry, ImportOptions, ImportReport, Importer,
};
pub use rig_json::RigJsonFormat;

#[cfg(test)]
mod tests {
	use std::io::Write;

	use super::*;

	struct LossyMockExporter;

	impl Exporter for LossyMockExporter {
		fn info(&self) -> FormatCapabilities {
			FormatCapabilities {
				id: "mock-lossy".to_owned(),
				display_name: "Lossy Mock Format".to_owned(),
				extensions: vec!["mck".to_owned()],
				supports_grid_deformer: false,
				supports_path_deformer: true,
				supports_cubic_interpolation: false,
				supports_animations: true,
				supports_opaque_passthrough: false,
				supports_textures: true,
				supports_vendor_data: false,
			}
		}

		fn export(
			&self,
			doc: &RigDocument,
			writer: &mut dyn Write,
			_options: &ExportOptions,
		) -> Result<ExportReport, FormatError> {
			let payload = format!("MOCK:{}", doc.nodes.len());
			writer.write_all(payload.as_bytes())?;
			Ok(ExportReport {
				bytes_written: payload.len(),
				warnings: Vec::new(),
			})
		}
	}

	#[test]
	fn rig_json_passes_roundtrip_conformance() {
		conformance::assert_roundtrip_conformance(&RigJsonFormat, &RigJsonFormat);
	}

	#[test]
	fn registry_resolves_and_enforces_strict_mode() {
		let mut reg = FormatRegistry::new();
		reg.register_importer(Box::new(RigJsonFormat));
		reg.register_exporter(Box::new(RigJsonFormat));
		reg.register_exporter(Box::new(LossyMockExporter));

		assert!(reg.resolve_importer("rig-json").is_some());
		assert!(reg.resolve_importer(".RIGJSON").is_some());
		assert!(reg.resolve_exporter("mck").is_some());
		assert_eq!(reg.list_exporters().len(), 2);

		let sample = conformance::sample_conformance_rig();
		let (bytes, rep) = reg
			.export_bytes("mck", &sample, &ExportOptions::default())
			.expect("non-strict export succeeds with warnings");
		assert!(!bytes.is_empty());
		assert!(rep.warnings.iter().any(|w| w.code == "UNSUPPORTED_GRID_DEFORMER"));
		assert!(rep.warnings.iter().any(|w| w.code == "UNSUPPORTED_OPAQUE_NODE"));
		assert!(rep.warnings.iter().any(|w| w.code == "UNSUPPORTED_CUBIC_INTERP"));
		assert!(rep.warnings.iter().any(|w| w.code == "UNSUPPORTED_VENDOR_DATA"));

		let strict = ExportOptions {
			strict: true,
			..ExportOptions::default()
		};
		let err = reg.export_bytes("mock-lossy", &sample, &strict).unwrap_err();
		assert!(matches!(err, FormatError::CapabilityRejected(_)));

		conformance::assert_exporter_conformance(&LossyMockExporter);
	}

	#[test]
	fn document_validate_catches_broken_invariants() {
		let mut doc = conformance::sample_conformance_rig();
		doc.nodes[1].id = 1; // duplicate id
		assert!(doc.validate().is_err());

		let mut doc2 = conformance::sample_conformance_rig();
		doc2.nodes[1].parent = Some(999); // missing parent
		assert!(doc2.validate().is_err());

		let mut doc3 = conformance::sample_conformance_rig();
		if let NodeKind::Part(ref mut p) = doc3.nodes[1].kind {
			p.mesh.indices[0] = 99; // out-of-bounds vertex index
		}
		assert!(doc3.validate().is_err());
	}
}

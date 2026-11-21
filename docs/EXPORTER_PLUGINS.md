# Writing RigLens Format & Exporter Plugins

RigLens keeps its core neutral (`riglens-format`). Rather than hardcoding proprietary or studio-specific formats into the core engine, all import/export flows go through a format-neutral intermediate representation (`RigDocument`) and the `Importer` / `Exporter` traits.

This design supports two clean ways to bind custom or community formats:

1. **Rust trait plugin** (in-process via `riglens-format`)
2. **External-language CLI / SDK pipeline** (via `.rigjson` JSON IR)

---

## Neutrality Policy

Proprietary third-party runtime formats (such as Live2D `.moc3`) are **not** included in RigLens core. RigLens provides the neutral IR, trait interfaces, capability checks, and conformance kit; third parties own their own format plugins, dependencies, and licensing.

Because the neutral IR separates rig topology, deformers, bindings, and textures from any particular container format or renderer, future runtime accelerators (e.g. GPU compute or neural/surrogate deformation evaluators) can also consume `RigDocument` directly without coupling to `.inp` serialization details.

---

## Option 1: Rust Trait Plugin (`riglens-format`)

Depend on `riglens-format` in your crate's `Cargo.toml`, implement `Exporter` (and optionally `Importer`), declare `FormatCapabilities`, and register your plugin with `FormatRegistry`:

```rust
use std::io::Write;
use riglens_format::{
    ExportOptions, ExportReport, Exporter, FormatCapabilities, FormatError, FormatRegistry, RigDocument,
};

pub struct MyGameFormat;

impl Exporter for MyGameFormat {
    fn info(&self) -> FormatCapabilities {
        FormatCapabilities {
            id: "my-game".to_owned(),
            display_name: "My Game Engine Puppet (.mgp)".to_owned(),
            extensions: vec!["mgp".to_owned()],
            supports_grid_deformer: true,
            supports_path_deformer: false,
            supports_cubic_interpolation: true,
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
        let mut written = 0usize;
        // Serialize `doc.nodes`, `doc.params`, `doc.animations`, `doc.textures`...
        writer.write_all(b"MGP1")?;
        written += 4;
        Ok(ExportReport {
            bytes_written: written,
            warnings: Vec::new(),
        })
    }
}
```

### Capability Checks & Strict Mode

When callers export through `FormatRegistry::export_to_writer` (or `puppet.export` in MCP / `export` in CLI):

1. `doc.validate()` checks node tree integrity, mesh index bounds, and parameter grid dimensions.
2. `check_export_compatibility` compares the document against `FormatCapabilities` and emits structured `Diagnostic` warnings (`UNSUPPORTED_GRID_DEFORMER`, `UNSUPPORTED_PATH_DEFORMER`, `UNSUPPORTED_CUBIC_INTERP`, `UNSUPPORTED_ANIMATIONS`, `UNSUPPORTED_OPAQUE_NODE`, `UNSUPPORTED_TEXTURES`, `UNSUPPORTED_VENDOR_DATA`).
3. If `options.strict == true` and any capability warning is triggered, export is rejected with `FormatError::CapabilityRejected`.

### Conformance Test Kit

Every plugin crate can run the built-in test kit inside its own `cargo test`:

```rust
#[test]
fn my_format_conformance() {
    riglens_format::conformance::assert_exporter_conformance(&MyGameFormat);
    // Or if you also implement Importer:
    // riglens_format::conformance::assert_roundtrip_conformance(&MyGameFormat, &MyGameFormat);
}
```

`assert_roundtrip_conformance` verifies a minimal 1-node rig, strict-mode capability rejection, and capability-gated round-trip invariants across `Part`, `Composite`, `MeshGroup`, `GridDeformer`, `PathDeformer`, `SimplePhysics`, `Opaque`, `Cubic` parameters/animations, base64 textures, and vendor blocks.

---

## Option 2: External-Language Pipeline (`.rigjson`)

If your target format relies on a C++, C#, Python, or closed SDK library, export to RigLens's lossless neutral JSON IR (`.rigjson`) and feed it to your own converter binary:

```sh
# Export any .inp / .inx / .psd / --sample puppet to neutral JSON IR
cargo run -p riglens-mcp -- export character.inp --out character.rigjson --pretty

# Inspect available formats and capabilities
cargo run -p riglens-mcp -- formats
```

From MCP (`riglens-mcp mcp`), use:
- `format.list` — enumerate registered importers, exporters, and capability flags.
- `puppet.export` — `{"path":"out.rigjson","format?":"rig-json","strict?":false,"pretty?":true}`.
- `puppet.import` — `{"path":"in.rigjson","format?":"rig-json"}`.

Key `.rigjson` conventions:
- `nodes` form a single-rooted tree via `id` and `parent` (`null` on root), discriminated by `kind` (`Group`, `Part`, `Composite`, `MeshGroup`, `GridDeformer`, `PathDeformer`, `SimplePhysics`, `Opaque`).
- `params[i].bindings[j]` store `is_set[x][y]` and `values.data[x][y]` matching `axis_x.len() x axis_y.len()`.
- `textures[i].bytes` are standard RFC 4648 base64 strings tagged with `format` (`Png`, `Jpeg`, `Tga`, `Unknown`).
- Unmodelled fields (`extras`, `mesh.extras`, `top_extras`) are preserved verbatim.

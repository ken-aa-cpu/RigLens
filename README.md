<h1 align="center">RigLens</h1>

<p align="center">
  A neutral, AI-friendly toolkit for 2D puppet rigs: headless read/write of Inochi2D-style <code>.inp</code>/<code>.inx</code> files,
  rig auditing, visual QA, and an MCP server — written in Rust.
</p>

> [!NOTE]
> **Independent project.** RigLens is not affiliated with, endorsed by, or part of Inochi2D, nijigenerate, nijilive, nijiexpose or Live2D.
> It began as a fork of [Inochi2D/inox2d](https://github.com/Inochi2D/inox2d) (BSD-2-Clause) and has since diverged substantially.
> See [NOTICE](NOTICE) for attribution. All third-party names are used only to describe file-format compatibility.

> [!WARNING]
> **Status: Experimental Preview / 實驗版.** Large parts of this code base were written with AI assistance and are still undergoing active testing and polish.
> Compatibility with files produced by other editors is verified only where stated below. Back up your work (or use Protected Project Mode).
> **遇到問題、Bug 或手感建議，請不要吝嗇你的反饋！** Experimental preview — please don't hesitate to share feedback or open an [Issue](https://github.com/ken-aa-cpu/RigLens/issues), and read the **[RigLens Wiki](https://github.com/ken-aa-cpu/RigLens/wiki)** for guides and troubleshooting.

---

## Why this exists

The 2D puppet ecosystem has editors, runtimes and trackers, but few tools that work **without opening an editor**.
RigLens focuses on the layer in between:

- **Headless file layer** – read, modify and write puppets from scripts, CI or an AI agent.
- **Quality layer** – automatic rig auditing and fixing, parameter-sweep renders, before/after diffs.
- **Format-neutral by design** – a neutral rig IR and public `Importer` / `Exporter` traits let anyone plug in their own format (see [Exporter Plugins](docs/EXPORTER_PLUGINS.md)).

It is meant to *complement* editors such as [nijigenerate](https://github.com/nijigenerate/nijigenerate) and Inochi Creator, not replace them.

## RigLens Suite

| Product | Role | Workspace Crate |
| :--- | :--- | :--- |
| **RigLens Core & Format** | Format-neutral IR (`RigDocument`), `Importer`/`Exporter` plugins, `.inp`/`.inx` parser & physics | [`riglens-format`](riglens-format/), [`riglens-core`](riglens-core/) |
| **RigLens Editor** | Dual-mode desktop workbench (`Quick` inspect/audit/fix & `Pro` rigger/timeline) | [`riglens-editor`](riglens-editor/) |
| **RigLens Stage** | 2x-SSAA OpenGL FBO/Stencil stage renderer & upcoming standalone playback/live viewer | [`riglens-stage-gl`](riglens-stage-gl/), [`riglens-opengl`](riglens-opengl/) |
| **RigLens Booster** | GPU co-processor: 120Hz physics, VTS 8001, DX11 Spout2, XPBD, JFA 2.5D rim light & 2D-DLSS | [`riglens-booster`](riglens-booster/) |
| **RigLens MCP** | Headless renderer, 10-point auditor, pure-Rust PSD pipeline, CLI & bidirectional MCP | [`riglens-mcp`](riglens-mcp/) |

## Workspace overview

| Crate / Path | Role |
| :--- | :--- |
| [`riglens-format`](riglens-format/) | Format-neutral rig IR (`RigDocument`, `PoseGroup`, `additive` params), `Importer` / `Exporter` traits, `FormatRegistry`, `.rigjson` codec, and plugin conformance kit. |
| [`riglens-core`](riglens-core/) | Core `.inp`/`.inx` parser and serializer (`InpFormat` bridge, `AnimatedPart` / `DynamicComposite` / `Mask` / `Shapes` / `Driver`), mesh-group / grid / path deformation, `PathDeformer` chain physics, `PoseGroup` / `additive` runtime, ancestor visibility (`is_effectively_enabled`), and animation. |
| [`riglens-opengl`](riglens-opengl/) | OpenGL renderer with live `.inp` hot-reload and WASM/WebGL support. |
| [`riglens-stage-gl`](riglens-stage-gl/) | Native OpenGL (`glow` 0.13) interactive stage renderer for `riglens-editor`: 2x supersampled FBO `Composite`, 8-bit `Stencil` masking (`Mask`/`Dodge`), premultiplied RGBA8 mipmaps, and all 19 `BlendMode`s with zero CPU readback. |
| [`riglens-mcp`](riglens-mcp/) | Headless CPU renderer, 10-point rig auditor / auto-fixer, pure-Rust `.psd` import/export & `psd.sync_textures`, CLI, JSON-RPC 2.0 MCP server (including `booster.*`), and companion stdio MCP client (`mcp-connect`). |
| [`riglens-editor`](riglens-editor/) | Native desktop editor (`egui` + `glow`): Welcome Screen with Protected Project Vault (`production/` + verbatim `.psd` `vault/`) & No-Project mode, JSON i18n (`en`/`zh-TW`/`ja` + external locales), `Quick` / `Pro` dual workspace (`F2`), `Booster FX` live preview toggle, 2x SSAA FBO/Stencil stage, CJK font fallback, `GridDeformer` / `PathDeformer` / `MeshGroup` stage gizmos, dynamic 2D pads (`+Y = Up`), 10-point audit, multi-animation timeline, and autosave recovery. |
| [`riglens-booster`](riglens-booster/) | Dual-channel GPU co-processor & 120Hz physics booster (`WindField`, coupled spring chains, VTube Studio Port 8001 WebSocket, Windows DX11 Spout2 bridge, `wgpu` XPBD soft-body, JFA 2D SDF 2.5D rim lighting & particles with DXGI Desktop Duplication `ScreenSync`, M3 2D-Native DLSS alpha-aware super-resolution & dual-tier motion-vector frame generation). |
| [`.agent/skills/inox2d-vtuber-rigger`](.agent/skills/inox2d-vtuber-rigger/SKILL.md) | Playbook for AI agents rigging VTuber puppets through the MCP server. |
| [`docs/EXPORTER_PLUGINS.md`](docs/EXPORTER_PLUGINS.md) | Guide for writing custom Rust or `.rigjson`-based exporter plugins. |

## Key capabilities

1. **`.inp` read/write** – generates binary `.inp` files (`TRNSRTS\0` + JSON + `TEX_SECT`) that follow the Inochi2D v0.8 layout.
2. **10-point Rig audit & auto-fix** – `rig.audit` checks containment leaks, triangle fold-overs, extreme mesh stretch, blink and yaw symmetry, clipping masks, deformer hierarchy integrity, binding coverage, draw-order conflicts, and mask cycles; `rig.auto_fix` repairs what it can.
3. **Visual QA** – `render.frame`, `render.grid` (parameter sweeps, optional wireframe), `render.gif`, `render.diff` (heat-map), `render.anim_sheet`.
4. **Layered PSD pipeline** – pure-Rust 8-bit RGBA PSD reader/writer, constrained Bowyer-Watson Delaunay meshing (closed-loop Douglas-Peucker contours, signed boundary offset, Sloan edge recovery, adaptive corner/joint refinement), and automatic VTuber rig templates (`rig.auto_*`).
5. **MCP server** – node, parameter, puppet, render and rig tools over JSON-RPC 2.0, with `--watch` live sync to the GUI.

## Compatibility

| Target | Status |
| :--- | :--- |
| Inochi2D v0.8-era `.inp` | Implemented; verified against the upstream `inox2d` loader. |
| `GridDeformer` & `PathDeformer` (`BezierDeformer` alias) | Parsed, serialized, targetable by deform parameters, and propagated into rendered meshes (including nested deformers). Attached path physics drivers are preserved in files with a diagnostic warning. |
| Remaining nijilive-only nodes (`AnimatedPart`, `DynamicComposite`, `Driver`, `Shapes`, `Mask`, ...) | **Preserved, not interpreted.** Kept as opaque data and written back unchanged; not rendered or deformed. Loading reports a warning (`warnings` in `puppet.info`). Covered by synthetic regression tests. |
| All 19 nijilive blend modes | Preserved on round trip. The software renderer implements them; the OpenGL renderer approximates some (see source). |
| Real `.inx` files (Inochi2D example models Aka and Midori, made with nijigenerate-era tools) | **Round trip verified**: node and parameter counts, parameter order, every binding and its interpolation mode, `psdLayerPath`, `emissionStrength`, `mesh.grid_axes`, parameter groups. Unmodelled fields are preserved verbatim. |
| `Cubic` / `Bezier` interpolation | Evaluated with Catmull-Rom cubic (parameters and animations) and Hermite tension (animations), matching nijilive. |
| Animations, automation, `.inx` `EXT_SECT` from nijigenerate itself | Not yet verified against files saved by current nijigenerate builds. |
| Live2D `.moc3` | Not supported; proprietary format. Not planned in core. |

## Quick start

```sh
# Run tests (across the entire workspace)
cargo test --workspace

# Open RigLens Editor (Welcome Screen with Protected Project Vault & No-Project modes)
cargo run -p riglens-editor

# Open a model directly using the 2x-SSAA mipmapped GPU stage (or --cpu-exact for reference)
cargo run -p riglens-editor -- --model midori
cargo run -p riglens-editor -- --model midori --cpu-exact
cargo run -p riglens-editor -- --model aka
cargo run -p riglens-editor -- --sample

# Live-sync the editor with the MCP server
cargo run -p riglens-editor -- live.inp --watch live.inp
```

> Example `.inx` binaries (`Midori.inx`, `Aka.inx`) from `Inochi2D/example-models` are gitignored; place them in [`assets/example-models/`](assets/example-models/README.md) (or point `RIGLENS_EXAMPLE_MODELS` to their directory) for automatic loading and CC BY 4.0 attribution.

### CLI examples

```sh
cargo run -p riglens-mcp -- new --out sample_vtuber.inp --sample
cargo run -p riglens-mcp -- info sample_vtuber.inp
cargo run -p riglens-mcp -- render-grid sample_vtuber.inp --param "Head:: Yaw-Pitch" --out head_grid.png --wireframe
cargo run -p riglens-mcp -- import-psd character.psd --out character.inp --delaunay
cargo run -p riglens-mcp -- export-psd sample_vtuber.inp --out character.psd
cargo run -p riglens-mcp -- formats
cargo run -p riglens-mcp -- export sample_vtuber.inp --out sample.rigjson --pretty
cargo run -p riglens-mcp -- commands

# Companion MCP client: connect to an external MCP server (e.g. nijigenerate)
cargo run -p riglens-mcp -- mcp-connect path/to/mcp-server --call puppet_info -- mcp sample

# Dual-channel GPU & 120Hz physics booster (VTube Studio Port 8001 + Spout2 + 2D DLSS)
cargo run --release -p riglens-booster -- list-spout
cargo run --release -p riglens-booster -- gpu-bench --width 1280 --height 720 --frames 20 --upscale 2.0 --frame-gen 2
cargo run --release -p riglens-booster -- run --spout-in VTubeStudioSpout --spout-out RigLensBooster --upscale 1.5 --frame-gen 2
```

### MCP client configuration

```json
{
  "mcpServers": {
    "puppet-tools": {
      "command": "cargo",
      "args": ["run", "--quiet", "-p", "riglens-mcp", "--", "mcp", "--sample"]
    }
  }
}
```

## Contributing

Issues and pull requests are welcome. Please state clearly if a contribution was AI-generated, and only submit assets you have the right to share.

## Support

RigLens is free and open source. If it saves you rigging or streaming time, you can support ongoing development on [Ko-fi](https://ko-fi.com/kenaa45519).

## License

BSD 2-Clause. See [LICENSE](LICENSE) and [NOTICE](NOTICE). Upstream copyright notices are retained as required.

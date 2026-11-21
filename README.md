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
> **Status: early / experimental.** Large parts of this code base were written with AI assistance and are not yet battle-tested.
> Compatibility with files produced by other editors is verified only where stated below. Back up your work.

---

## Why this exists

The 2D puppet ecosystem has editors, runtimes and trackers, but few tools that work **without opening an editor**.
RigLens focuses on the layer in between:

- **Headless file layer** – read, modify and write puppets from scripts, CI or an AI agent.
- **Quality layer** – automatic rig auditing and fixing, parameter-sweep renders, before/after diffs.
- **Format-neutral by design** – a neutral rig IR and public `Importer` / `Exporter` traits let anyone plug in their own format (see [Exporter Plugins](docs/EXPORTER_PLUGINS.md)).

It is meant to *complement* editors such as [nijigenerate](https://github.com/nijigenerate/nijigenerate) and Inochi Creator, not replace them.

## Workspace overview

| Crate / Path | Role |
| :--- | :--- |
| [`riglens-format`](riglens-format/) | Format-neutral rig IR (`RigDocument`), `Importer` / `Exporter` traits, `FormatRegistry`, `.rigjson` codec, and plugin conformance kit. |
| [`inox2d`](inox2d/) | Core: `.inp`/`.inx` parser and serializer (`InpFormat` bridge, including P1 `AnimatedPart` / `DynamicComposite` / `Mask` / `Shapes` / `Driver`), mesh-group / grid / path deformation, simple physics, animation, opacity store. |
| [`inox2d-opengl`](inox2d-opengl/) | OpenGL renderer with live `.inp` hot-reload and WASM/WebGL support. |
| [`riglens-stage-gl`](riglens-stage-gl/) | Native OpenGL (`glow` 0.13) interactive stage renderer for `inox2d-gui`: 2x supersampled FBO `Composite`, 8-bit `Stencil` masking (`Mask`/`Dodge`), premultiplied RGBA8 mipmaps, and all 19 `BlendMode`s with zero CPU readback. |
| [`inox2d-mcp`](inox2d-mcp/) | Headless CPU renderer, rig auditor / auto-fixer, pure-Rust `.psd` import/export (`PsdFormat`), CLI, JSON-RPC 2.0 MCP server (including `booster.*`), and companion stdio MCP client (`mcp-connect`). |
| [`inox2d-gui`](inox2d-gui/) | Native desktop studio (`egui` + `glow`): full-bleed GPU FBO/Stencil stage with 2x SSAA + mipmaps & cursor-anchored pan/zoom (`--cpu-exact` for reference CPU renderer), `Live Phys` & Booster telemetry sync, `File -> Example Models` (`--model midori\|aka\|sample`), collapsible outliner, `GridDeformer` / `PathDeformer` / `MeshGroup` stage gizmos, dynamic 2D parameter pads (`+Y = Up`), 10-point audit & diagnostics, multi-animation timeline, file browser dialogs, autosave recovery, and integrated `Booster` M4 control panel. |
| [`riglens-booster`](riglens-booster/) | Dual-channel GPU co-processor & 120Hz physics booster (`WindField`, coupled spring chains, VTube Studio Port 8001 WebSocket, Windows DX11 Spout2 bridge, `wgpu` XPBD soft-body, JFA 2D SDF 2.5D rim lighting & particles with DXGI Desktop Duplication `ScreenSync`, M3 2D-Native DLSS alpha-aware super-resolution & dual-tier motion-vector frame generation). |
| [`.agent/skills/inox2d-vtuber-rigger`](.agent/skills/inox2d-vtuber-rigger/SKILL.md) | Playbook for AI agents rigging VTuber puppets through the MCP server. |
| [`docs/EXPORTER_PLUGINS.md`](docs/EXPORTER_PLUGINS.md) | Guide for writing custom Rust or `.rigjson`-based exporter plugins. |

> Crate names are inherited from the fork and will be renamed in a later release.

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
# Run tests (across all 7 workspace crates)
cargo test -p riglens-format -p inox2d -p inox2d-mcp -p inox2d-opengl -p riglens-stage-gl -p inox2d-gui -p riglens-booster

# Open the default real example model (Midori/Aka if staged in assets/example-models/) using the 2x-SSAA mipmapped GPU stage (or --cpu-exact for reference)
cargo run -p inox2d-gui -- --model midori
cargo run -p inox2d-gui -- --model midori --cpu-exact
cargo run -p inox2d-gui -- --model aka
cargo run -p inox2d-gui -- --sample

# Live-sync the studio with the MCP server
cargo run -p inox2d-gui -- live.inp --watch live.inp
```

> Example `.inx` binaries (`Midori.inx`, `Aka.inx`) from `Inochi2D/example-models` are gitignored; place them in [`assets/example-models/`](assets/example-models/README.md) (or point `RIGLENS_EXAMPLE_MODELS` to their directory) for automatic loading and CC BY 4.0 attribution.

### CLI examples

```sh
cargo run -p inox2d-mcp -- new --out sample_vtuber.inp --sample
cargo run -p inox2d-mcp -- info sample_vtuber.inp
cargo run -p inox2d-mcp -- render-grid sample_vtuber.inp --param "Head:: Yaw-Pitch" --out head_grid.png --wireframe
cargo run -p inox2d-mcp -- import-psd character.psd --out character.inp --delaunay
cargo run -p inox2d-mcp -- export-psd sample_vtuber.inp --out character.psd
cargo run -p inox2d-mcp -- formats
cargo run -p inox2d-mcp -- export sample_vtuber.inp --out sample.rigjson --pretty
cargo run -p inox2d-mcp -- commands

# Companion MCP client: connect to an external MCP server (e.g. nijigenerate)
cargo run -p inox2d-mcp -- mcp-connect path/to/mcp-server --call puppet_info -- mcp sample

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
      "args": ["run", "--quiet", "-p", "inox2d-mcp", "--", "mcp", "--sample"]
    }
  }
}
```

## Roadmap

- [x] Lossless round trip for unknown node types and all blend modes (synthetic tests).
- [x] Round-trip verified on real `.inx` files (Aka, Midori).
- [x] True Catmull-Rom `Cubic` & Hermite `Bezier` evaluation, matching nijilive.
- [x] P1 deformers: `GridDeformer` and `PathDeformer` (`BezierDeformer`) parse/serialize, parameter bindings, and render propagation.
- [x] P1 remaining nodes (`AnimatedPart`, `DynamicComposite`, `Driver`, `Shapes`, `Mask`) typed components, `.inp`/`.inx` + `RigDocument` IR round-trip & nijigenerate compatibility tests.
- [x] Format-neutral rig model (`riglens-format`) and public **`Importer` / `Exporter` traits** + conformance kit so users can bind their own library and emit any format.
- [x] P3 algorithm upgrades: nijilive-parity physics initialization & NaN/panic guards, 10-point rig auditor, and constrained Delaunay meshing with adaptive joint refinement.
- [x] P4 desktop editor (`inox2d-gui` + `riglens-stage-gl` + shared command layer): hierarchical node tree & draw-order outliner, full-bleed OpenGL FBO `Composite` & 8-bit `Stencil` viewport with premultiplied mipmaps, 2x SSAA & cursor-anchored camera zoom (`--cpu-exact` reference toggle), stage editing for `GridDeformer` / `PathDeformer` / `MeshGroup`, clipping mask & binding keypoint editor (`mirror`, scalar CRUD), dynamic 2D parameter pads (`+Y = Up`), 10-point audit & Phase 0 load-warning diagnostics panel, multi-animation timeline, pure-Rust Open/Save/Export dialogs, and autosave crash recovery.
- [x] Co-processor M1 (`riglens-booster`): 120Hz coupled physics/wind engine, VTube Studio Public API (Port `8001`) WebSocket bridge, and Windows DX11 Spout2 shared-texture sender/receiver.
- [x] Co-processor M2 (`riglens-booster`): `wgpu` GPU compute/render pipeline with screen-space XPBD soft-body mesh deformation, JFA 2D SDF normals & directional 2.5D rim lighting (with DXGI Desktop Duplication + GDI fallback Windows `ScreenSync`), motion-driven GPU particles, and `gpu-bench`.
- [x] Co-processor M3 (`riglens-booster`): **2D-Native DLSS** (`upscale_alpha_aware.wgsl` premultiplied-alpha Catmull-Rom super-resolution + edge sharpening, `flow_estimate.wgsl` + `MotionVectorField` dual-tier motion-vector frame generation via `frame_gen_warp.wgsl`).
- [x] Co-processor M4: integrated `Booster` tab & `BoosterController` live telemetry sync in `inox2d-gui`, plus MCP `booster.*` controls (`booster.status`, `booster.list_spout`, `booster.configure`, `booster.start`, `booster.stop`).
- [x] Companion MCP *client* (`McpClient` / `McpStdioClient` + `inox2d-mcp mcp-connect`) for connecting to external editors that expose MCP (e.g. nijigenerate).
- [x] Crate packaging metadata (`Cargo.toml` repository, README, keywords, categories) & publishable workspace documentation (`cargo doc`). (Public crate rename deferred to major v1.0 tag to preserve fork path stability.)
- [x] Core-Identity Upgrade: `Quick` / `Pro` dual workspace mode (`F2`), `is_effectively_enabled` ancestor visibility inheritance, CJK/Korean font fallback, real-world PSD semantic Auto-Rig/Auto-Mask & hidden differential expression toggles, `PathDeformer` Verlet/spring-chain physics, `DynamicComposite` offscreen render collection, `PoseGroup` + additive Blend Shape neutral IR, and live `Booster FX` stage preview.

## Contributing

Issues and pull requests are welcome. Please state clearly if a contribution was AI-generated, and only submit assets you have the right to share.

## License

BSD 2-Clause. See [LICENSE](LICENSE) and [NOTICE](NOTICE). Upstream copyright notices are retained as required.

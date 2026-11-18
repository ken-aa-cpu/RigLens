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
- **Format-neutral by design** – a planned exporter interface lets anyone plug in their own output format (see [Roadmap](#roadmap)).

It is meant to *complement* editors such as [nijigenerate](https://github.com/nijigenerate/nijigenerate) and Inochi Creator, not replace them.

## Workspace overview

| Crate / Path | Role |
| :--- | :--- |
| [`inox2d`](inox2d/) | Core: `.inp`/`.inx` parser and serializer, mesh-group deformation, simple physics, animation, opacity store. |
| [`inox2d-opengl`](inox2d-opengl/) | OpenGL renderer with live `.inp` hot-reload and WASM/WebGL support. |
| [`inox2d-mcp`](inox2d-mcp/) | Headless CPU renderer, rig auditor / auto-fixer, pure-Rust `.psd` import/export, CLI, JSON-RPC 2.0 MCP server. |
| [`inox2d-gui`](inox2d-gui/) | Native desktop studio (`egui` + `glow`): viewport, gizmos, parameter pads, timeline and curves. |
| [`.agent/skills/inox2d-vtuber-rigger`](.agent/skills/inox2d-vtuber-rigger/SKILL.md) | Playbook for AI agents rigging VTuber puppets through the MCP server. |

> Crate names are inherited from the fork and will be renamed in a later release.

## Key capabilities

1. **`.inp` read/write** – generates binary `.inp` files (`TRNSRTS\0` + JSON + `TEX_SECT`) that follow the Inochi2D v0.8 layout.
2. **Rig audit & auto-fix** – `rig.audit` checks containment leaks, triangle fold-overs, extreme mesh stretch, blink and yaw symmetry, and clipping masks; `rig.auto_fix` repairs what it can.
3. **Visual QA** – `render.frame`, `render.grid` (parameter sweeps, optional wireframe), `render.gif`, `render.diff` (heat-map), `render.anim_sheet`.
4. **Layered PSD pipeline** – pure-Rust 8-bit RGBA PSD reader/writer, Delaunay meshing, automatic VTuber rig templates (`rig.auto_*`).
5. **MCP server** – node, parameter, puppet, render and rig tools over JSON-RPC 2.0, with `--watch` live sync to the GUI.

## Compatibility

| Target | Status |
| :--- | :--- |
| Inochi2D v0.8-era `.inp` | Implemented; verified against the upstream `inox2d` loader. |
| nijilive-only nodes (`GridDeformer`, `PathDeformer`, `AnimatedPart`, `DynamicComposite`, `Driver`, ...) | **Preserved, not interpreted.** Kept as opaque data and written back unchanged; not rendered or deformed. Loading reports a warning (`warnings` in `puppet.info`). Covered by synthetic regression tests. |
| All 19 nijilive blend modes | Preserved on round trip. The software renderer implements them; the OpenGL renderer approximates some (see source). |
| nijilive / nijigenerate parameter bindings, animations, `.inx` extension data | **Not yet verified against real files.** Bindings we cannot parse are currently dropped with a log message. |
| Live2D `.moc3` | Not supported; proprietary format. Not planned in core. |

## Quick start

```sh
# Run tests
cargo test -p inox2d -p inox2d-mcp -p inox2d-opengl -p inox2d-gui

# Open the built-in sample in the desktop studio
cargo run -p inox2d-gui -- --sample

# Live-sync the studio with the MCP server
cargo run -p inox2d-gui -- live.inp --watch live.inp
```

### CLI examples

```sh
cargo run -p inox2d-mcp -- new --out sample_vtuber.inp --sample
cargo run -p inox2d-mcp -- info sample_vtuber.inp
cargo run -p inox2d-mcp -- render-grid sample_vtuber.inp --param "Head:: Yaw-Pitch" --out head_grid.png --wireframe
cargo run -p inox2d-mcp -- import-psd character.psd --out character.inp --delaunay
cargo run -p inox2d-mcp -- export-psd sample_vtuber.inp --out character.psd
cargo run -p inox2d-mcp -- commands
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
- [ ] Verify `.inp` / `.inx` round-trip with real nijigenerate / nijilive files (bindings, animations, `.inx`).
- [ ] Format-neutral rig model and a public **`Exporter` trait** so users can bind their own library and emit any format.
- [ ] Act as an MCP *client* companion for editors that already expose MCP (e.g. nijigenerate).
- [ ] Rename crates and publish documentation.

## Contributing

Issues and pull requests are welcome. Please state clearly if a contribution was AI-generated, and only submit assets you have the right to share.

## License

BSD 2-Clause. See [LICENSE](LICENSE) and [NOTICE](NOTICE). Upstream copyright notices are retained as required.

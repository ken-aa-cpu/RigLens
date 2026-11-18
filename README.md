<p align="center">
  <h1 align="center">Inox2D + inox2d-mcp (Personal AI Edition)</h1>
  <p align="center">
    <img width="200" height="256" src="inox2d_logo.svg" alt="Inox2D Logo">
  </p>
  <p align="center">
    An AI-assisted personal fork of <a href="https://github.com/Inochi2D/inox2d">Inochi2D/inox2d</a> (BSD-2-Clause) featuring a <strong>read-write Inochi2D `.inp` core</strong>, <strong>GPU-accelerated desktop studio (<code>inox2d-gui</code>)</strong>, <strong>layered PSD pipeline</strong>, <strong>rig auditor</strong>, and <strong>MCP server (<code>inox2d-mcp</code>)</strong>.
  </p>
</p>

> [!IMPORTANT]
> **AI & Personal Use Disclaimer / 聲明**
>
> - **AI-Assisted Work**：本專案（包含 `inox2d-mcp`、`inox2d-gui`、序列化相容性修正與自動綁骨流水線）主要由 AI 輔助設計與實作。
> - **Personal Use Only**：本專案僅供**個人學習、研究與私人測試使用**，與官方 **Inochi2D / Inochi Creator / Puppetstring** 及其原作者無任何關聯或背書。
> - **Respecting the Community**：考量 Inochi2D 原作者與社群對 AI 工具及 AI 內容的反對立場，請勿將本儲存庫的產出或 AI 生成內容提交至官方上游專案或社群頻道，亦不建議用於公開商業發布。
>
> *English Summary: This is an AI-built personal fork for private experimentation and learning only. It is not affiliated with or endorsed by Inochi2D or its authors. Out of respect for the Inochi2D community's stance against AI, do not upstream PRs or submit AI-generated assets from this fork to official Inochi2D spaces.*

---

## Workspace Overview

| Crate / Path | Role |
| :--- | :--- |
| [`inox2d`](inox2d/) | Read-write Inochi2D core: `.inp`/`.inx` parser & official D-compatible serializer (`TRNSRTS\0`, `TEX_SECT`, `EXT_SECT`), `MeshGroup` barycentric deform propagation, `SimplePhysics`, `PuppetAnimation`, `OpacityStore`. |
| [`inox2d-opengl`](inox2d-opengl/) | Hardware-accelerated OpenGL renderer with `OpacityStore` expression blending, live `.inp` hot-reload (`render-opengl`), and WASM/WebGL support. |
| [`inox2d-mcp`](inox2d-mcp/) | Headless CPU renderer, 6-point rig auditor/auto-fixer, pure-Rust `.psd` importer/exporter, CLI (`exec`, `serve --watch`), and JSON-RPC 2.0 MCP server (`mcp --watch`). |
| [`inox2d-gui`](inox2d-gui/) | Craft-style pure-Rust native desktop rigging studio (`egui` + `glow` GPU viewport, live gizmos, timeline & motion curves). |
| [`.agent/skills/inox2d-vtuber-rigger`](.agent/skills/inox2d-vtuber-rigger/SKILL.md) | Zero-guessing AI VTuber rigging playbook for `inox2d-mcp` and layered PSD workflows. |

---

## Key Capabilities

1. **Official Inochi2D `.inp` Compatibility**:
   - Generates binary `.inp` files (`TRNSRTS\0` + JSON + `TEX_SECT`) compatible with **Inox2D**, official **Inochi Creator (D / `fghj`)**, and **Puppetstring VTuber Tracking** (including `thumbnailId = 4294967295`, `mask_threshold` on all `Part`/`Composite` nodes, `animationWeight`, `automation`, and `groups`).
2. **Craft-Style Native Desktop Studio (`inox2d-gui`)**:
   - Hardware-accelerated GPU canvas viewport with sub-millisecond rendering, interactive gizmos (`V` Move/Scale, `A` Vertex/Topology edit + deform-preserving Delaunay retriangulation, `D` Deform Brush, `P` 2.5D Pose, `S` Physics Pendulum Gizmo), 2D Parameter Pads (`Head:: Yaw-Pitch`, `Mouth:: Viseme`), and Timeline / Motion Curves editor.
   - Supports bidirectional live sync (`--watch live.inp`) alongside `inox2d-mcp`.
3. **Zero-Guessing AI Rigging & Visual QA (`inox2d-mcp`)**:
   - **6-Point Rig Auditor (`rig.audit`) & Auto-Fixer (`rig.auto_fix`)**: Checks containment leaks, triangle fold-overs, extreme mesh stretch, blink symmetry, yaw symmetry, and clipping masks.
   - **Visual Renders**: `render.frame`, `render.grid` (3×3 parameter sweeps with optional wireframes), `render.gif` (animated GIF89a), `render.diff` (before/after heatmap), and FigCraft-style rigging spec cards (`craft.figcraft_card`).
   - **Layered `.psd` Pipeline**: Pure-Rust 8-bit RGBA `.psd` reader & writer (Raw + PackBits RLE, Unicode `luni` names, `lsct` groups, Bowyer-Watson Delaunay meshing).

---

## Quick Start

### Run Tests
```sh
cargo test -p inox2d -p inox2d-mcp -p inox2d-opengl -p inox2d-gui
```

### Launch Inox2D Studio (`inox2d-gui`)
```sh
# Open with built-in sample VTuber puppet
cargo run -p inox2d-gui -- --sample

# Open a specific .inp or .psd file
cargo run -p inox2d-gui -- path/to/puppet.inp

# Live-sync with inox2d-mcp
cargo run -p inox2d-gui -- live.inp --watch live.inp
```

### `inox2d-mcp` CLI Examples
```sh
# Generate a rigged sample VTuber puppet (.inp)
cargo run -p inox2d-mcp -- new --out sample_vtuber.inp --sample

# Inspect nodes, meshes, parameters, and animations
cargo run -p inox2d-mcp -- info sample_vtuber.inp

# Render a 3x3 Head Yaw-Pitch grid with wireframes
cargo run -p inox2d-mcp -- render-grid sample_vtuber.inp --param "Head:: Yaw-Pitch" --out head_grid.png --wireframe

# Import a layered PSD with Delaunay meshing and auto-rig
cargo run -p inox2d-mcp -- import-psd character.psd --out character.inp --delaunay

# Export an .inp puppet back to a layered PSD
cargo run -p inox2d-mcp -- export-psd sample_vtuber.inp --out character.psd

# List all available engine commands
cargo run -p inox2d-mcp -- commands
```

### MCP Client Configuration
```json
{
  "mcpServers": {
    "inox2d": {
      "command": "cargo",
      "args": ["run", "--quiet", "-p", "inox2d-mcp", "--", "mcp", "--sample"]
    }
  }
}
```

---

## License

This project is licensed under the **2-Clause BSD License**, inherited from upstream [Inochi2D/inox2d](https://github.com/Inochi2D/inox2d). See [LICENSE](LICENSE) for details.

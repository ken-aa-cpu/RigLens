<p align="center">
  <h1 align="center">Inox2D + inox2d-mcp</h1>
  <p align="center">
    <img width="200" height="256" src="inox2d_logo.svg">
  </p>
  <div align="center">

Fork of [Inochi2D/inox2d](https://github.com/Inochi2D/inox2d) (BSD-2-Clause) upgraded with a **read-write Inochi2D Creator core**, **headless CPU renderer**, **layered PSD importer/exporter**, **VTuber auto-rigger**, and **MCP (Model Context Protocol) server** (`inox2d-mcp`) for AI agents.

  </div>
</p>

&nbsp;

## What `inox2d-mcp` Adds on Top of Upstream `Inox2D`

Upstream `Inochi2D/inox2d` was a read-only runtime prototype. This fork (`ken-aa-cpu/inox2d-mcp`) turns it into an **Agent-native Inochi Creator engine** modeled after the [ArtCraft](https://getartcraft.com/) CLI/MCP architecture:

1. **Read-Write `inox2d` Core (`inox2d`)**:
   - Full `.inp` / JSON serialization (`Puppet::to_json`, `write_inp`) with round-trip fidelity (`TRNSRTS\0`, `TEX_SECT`, `EXT_SECT`).
   - Completed `MeshGroup` deformation propagation (barycentric transfer from `MeshGroup` meshes to descendant `Part` vertices), `Animations` (`PuppetAnimation` / `AnimationLane` / `Keyframe`), and dynamic `zSort` bindings.
   - Fixed `ParamCtx::apply` zero-value skipping (`Eye:: Open = 0.0` blinks now evaluate properly) and `Matrix2d::get_mut` transposition.
2. **Headless Software Renderer & Visual Inspection (`inox2d-mcp`)**:
   - Implements `inox2d::render::InoxRenderer` on CPU (`render_frame`) with bilinear UV sampling, blend modes, masks, evaluated `OpacityStore`, and optional mesh wireframes.
   - **`render-grid`**: Generates 3×3 parameter sweep contact sheets (e.g., `Head:: Yaw-Pitch`) so multimodal AI agents can visually inspect rigs.
   - **`render-anim` & `gif`**: Renders multi-frame animation/physics sprite sheets and looping **GIF89a animations** (`render.gif`).
   - **`diagnostic` & `render.diff`**: X-ray diagnostic overlay (50px world grid, red/orange fold-over & stretch heatmap, green Face containment contour) and side-by-side `[Before | After | Diff Heatmap]` comparison.
3. **Zero-Guessing Rig Audit, Constraint Solver, Visemes & Auto-Rigger**:
   - **6-Point Rig Auditor (`rig.audit`) & Auto-Fixer (`rig.auto_fix`)**: Detects facial containment leaks, triangle fold-overs, over-stretching, incomplete blink closure, yaw asymmetry, and missing clipping masks; clamps overflowing features and runs Laplacian fold-over relaxation.
   - **Anatomical Telemetry (`rig.analyze_anatomy`)**: Expresses all parts in Face-normalized coordinates (`[-1..1]`) and pixel margins.
   - Subdivided grid (`mesh.auto_grid`), alpha contour-culled meshing (`mesh.auto_contour`), and semantic Gaussian soft brush (`deform.brush`).
   - High-level parametric operators: proportional 2.5D head yaw/pitch (`rig.auto_head_perspective`), eyelid blink (`rig.auto_blink`), 2D A/I/U/E/O vowel visemes (`rig.auto_visemes`), 2D brow expressions (`rig.auto_brow`), head roll (`rig.auto_head_roll`), breathing (`rig.auto_breath`), `Opacity` expression toggles (`rig.bind_opacity`), procedural expression overlay synthesis (`node.synthesize_overlay` for `blush`, `gloom`, `tears`, `angry`, `shock`), and pendulum physics (`physics.add_pendulum`).
4. **Full Session & Node CRUD + Native `.psd` Pipeline**:
   - Undo/redo (`edit.undo`, `edit.redo`), named checkpoints (`session.checkpoint`, `session.restore`), atomic batches (`session.batch`), and full node/mask/`MeshGroup` CRUD (`node.configure`, `node.set_mask`, `node.add_meshgroup`, `node.remove`).
   - Pure-Rust 8-bit RGBA `.psd` reader & writer (Raw + PackBits RLE, `luni` Unicode layer names, `lsct` folder hierarchy, clipping masks, alpha bounding-box trimming) + `D:\app\PhotoCraft\photocraft-cli.exe` integration.
   - Includes AI operator playbook at [`.agent/skills/inox2d-vtuber-rigger/SKILL.md`](.agent/skills/inox2d-vtuber-rigger/SKILL.md).

### Quick Start (`inox2d-mcp`)

```sh
# Build & run all tests
cargo test -p inox2d -p inox2d-mcp -p inox2d-opengl

# Generate a rigged sample VTuber puppet (.inp, opens in official Inochi Creator)
cargo run -p inox2d-mcp -- new --out sample_vtuber.inp --sample

# Inspect nodes, meshes, parameters, and animations
cargo run -p inox2d-mcp -- info sample_vtuber.inp

# Render a 3x3 Head Yaw-Pitch contact sheet with mesh wireframes
cargo run -p inox2d-mcp -- render-grid sample_vtuber.inp --param "Head:: Yaw-Pitch" --out head_grid.png --wireframe

# Render an 8-frame Idle animation sheet
cargo run -p inox2d-mcp -- render-anim sample_vtuber.inp --out idle_sheet.png --anim Idle --frames 8 --wireframe

# Import a layered PSD with Bowyer-Watson Delaunay meshing and auto-rig into an .inp puppet
cargo run -p inox2d-mcp -- import-psd character.psd --out character.inp --delaunay

# Generate a studio FigCraft Rigging Spec Card (SVG + PNG)
cargo run -p inox2d-mcp -- exec sample_vtuber.inp craft.figcraft_card '{\"out\":\"spec_card.png\"}'

# Export an .inp puppet back to a layered PSD
cargo run -p inox2d-mcp -- export-psd sample_vtuber.inp --out character.psd

# List all 45 engine commands
cargo run -p inox2d-mcp -- commands

# Run as an MCP server (JSON-RPC 2.0 over stdio) with live .inp hot-reload watch
cargo run -p inox2d-mcp -- mcp --sample --watch live.inp
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

&nbsp;

## Status

Both INP and INX files get parsed **and serialized** (`write_inp`) cleanly.

### Feature tree

- [x] Parsing & Serialization
  - [x] INP format (read + write round-trip)
  - [x] INX / JSON payload (read + write)
  - [x] Layered `.psd` import & export with Bowyer-Watson Delaunay contour meshing (`inox2d-mcp`)
- [x] Rendering
  - [x] OpenGL (with `OpacityStore` expression blending + live `.inp` hot-reload in `render-opengl`)
    - [x] WASM (WebGL)
  - [x] Headless CPU Software Renderer, X-Ray Diagnostic, Diff, GIF & FigCraft Spec Card (`inox2d-mcp`)
- [x] Parameters
  - [x] Deforms (mesh vertex offsets)
  - [x] Values (node transform offsets)
  - [x] Opacity (`OpacityStore` parameter blending in both CPU & OpenGL renderers)
  - [x] Z-sort
- [x] Physics (`SimplePhysics` Rigid & Spring Pendulums)
- [x] Animations (`PuppetAnimation`, `AnimationLane`, `Keyframe`)
- [x] Mesh groups (`MeshGroup` barycentric deform propagation)
- [x] Agent / MCP Server (`inox2d-mcp` CLI, JSON-lines `serve --watch`, JSON-RPC 2.0 `mcp --watch`, 45 tools)

### INP and INX parsing

```sh
cargo run -p inox2d --features owo --example parse-inp path/to/puppet.inp
```

![Parsed foxgirl](https://fs.speykious.dev/inox2d/inox2d-parsed-foxgirl.png)

### OpenGL renderer (with live `.inp` hot-reload & interactive VTuber controls)

```sh
cargo run -p render-opengl live.inp
```

While running side-by-side with `inox2d-mcp mcp --watch live.inp`:
- **Auto hot-reload**: automatically detects atomic `.inp` updates every 250ms while preserving camera pan/zoom (`R` forces reload, `F` auto-fits puppet bounds).
- **Interactive controls**: move mouse for `Head:: Yaw-Pitch` / `Head:: Roll` / eye gaze tracking, `Space` to toggle `Idle` animation playback, `B` to blink, `M` to cycle 2D `Mouth:: Viseme` presets (`Neutral/A/I/U/E/O`), and `1`–`5` to toggle animated `OpacityStore` expression overlays (`Expr:: Blush/Gloom/Tears/Angry/Shock`).

![OpenGL-rendered Arch-chan](https://fs.speykious.dev/inox2d/inox2d-opengl-arch-chan.png)

### WebGL demo

See the [`render_webgl`](/examples/render_webgl) example.

![WebGL-rendered Aka](https://fs.speykious.dev/inox2d/inox2d-webgl-foxgirl.png)

## Implementation

Inox2D aims to support all features currently present in the standard D implementation.

Inox2D is designed to be extensible. Nodes are extensible through a generic `InoxData<T>` enum which has a `Custom(T)` variant. Every other part of the library accounts for it: the OpenGL renderer accepts any struct that implements the `CustomRenderer` trait to be able to render your custom nodes, and the deserialization functions accept generic `Fn`s for deserialization of custom nodes when it is relevant.

&nbsp;

## Optimization on OpenGL

| Implementation        | language | OpenGL calls |
| --------------------- | -------- | ------------ |
| Inochi2D reference*   | D        | 3076         |
| Link Mauve's inochi2d | Rust     | 551          |
| Inox2D                | Rust     | 1639         |

The OpenGL renderer on Inox2D has a few simple optimizations that result in fewer OpenGL calls:

- it uses a simple OpenGL cache to avoid making calls when the resulting state won't change,
- it only uploads a model's part textures once instead of every frame.

> \* Reference implementation is subject to change as optimisation passes are done, additionally code is more geared towards readability than performance for implementers to be able to more easily use it as reference.

&nbsp;

## License

This project is licensed under the 2-Clause BSD license.
See [LICENSE](LICENSE) for details.

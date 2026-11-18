---
name: inox2d-vtuber-rigger
description: Zero-guessing VTuber/Live2D rigging playbook for inox2d-mcp and PhotoCraft. Follow this manual whenever importing layered PSD artwork, creating Inochi2D (.inp) puppets, rigging facial deformations, auditing mesh foldovers/containment leaks, or sculpting expressions via MCP.
---

# Inochi2D / Live2D Zero-Guessing Rigging Manual (`inox2d-mcp`)

Never guess raw vertex coordinates or eyeball whether a 2.5D head turn leaks out of the face contour. Always use the **Analyze -> Auto-Rig -> Audit -> Auto-Fix -> Diagnostic X-Ray** loop below.

---

## Mandatory 6-Stage Workflow

### Stage 1: Artwork Inspection & PSD Import
1. If starting from a `.psd` file and `PhotoCraft` (`D:\app\PhotoCraft\photocraft-cli.exe`) is available, inspect layer structure first with `photocraft_inspect` (`path`).
2. Ensure parts follow semantic naming (EN or ZH) so proportional auto-rigging maps depths accurately:
   - `Face` / `臉` (`depth_z = 0.25`)
   - `Hair_Back` / `後髮` (`depth_z = -0.85` + hair sway)
   - `Hair_Front_L` / `Hair_Front_R` / `前髮` / `劉海` (`depth_z = 1.25` + hair sway)
   - `Hair_Side` / `側髮` (`depth_z = 0.65` + hair sway)
   - `Eye_L` / `左眼`, `Eye_R` / `右眼` (`depth_z = 1.0` + blink + gaze)
   - `Pupil` / `Iris` / `Highlight` / `瞳` / `高光` (clipped via `Mask`)
   - `Mouth` / `嘴` (`depth_z = 1.1` + `Mouth:: Open` + `Mouth:: Form`)
   - `Brow` / `眉` (`depth_z = 1.05`), `Nose` / `鼻` (`depth_z = 1.35`), `Ear` / `耳` (`depth_z = -0.55`)
   - `Torso` / `Body` / `身體` (`Breath`)
3. Import into the session using `puppet_import_psd` (`path`, `contour_mesh: true`, `auto_rig: true`).

### Stage 2: Anatomical Telemetry (`rig_analyze_anatomy`)
Call `rig_analyze_anatomy` **before** manual tweaks.
- Inspect `face_anchor` (`center`, `size`) and each feature's `face_normalized_pos` (`[-1..1]` inside face bounds) and `face_margins_px` (`left`, `right`, `top`, `bottom`).
- If `rest_warnings` shows any feature `< 4px` from the face boundary at rest, keep `yaw_px` conservative or trim/reposition the part before binding extreme turns.

### Stage 3: Base Rigging & Expression Toggles
1. Run `rig_auto_rig_vtuber` if the model was assembled manually via `node_add_part` instead of `puppet_import_psd`.
2. For toggleable expression parts (blush, tears, angry marks, accessories), bind a `0..1` opacity slider via `rig_bind_opacity`:
   ```json
   { "node_name": "Blush", "param": "Expr:: Blush", "opacity_at_0": 0.0, "opacity_at_1": 1.0 }
   ```
3. To densify a specific deformable mesh without losing transparent culling, call `mesh_auto_contour` (`node_name`, `cols: 8`, `rows: 8`).

### Stage 4: Mandatory Audit & Constraint Solver Loop (`rig_audit` -> `rig_auto_fix`)
**Do not ship or finish a model without running `rig_audit`!**
1. Call `rig_audit` to sweep all 9 `Head:: Yaw-Pitch` corners, `Blink(0.0)`, and `MouthOpen(1.0)`. It returns:
   - `score` (`0..100`) and `passed` (`true` requires `score >= 80`, `containment_leaks == 0`, `mesh_foldovers == 0`, `blink_issues == 0`).
   - Exact failure lists: `containment_leaks`, `mesh_foldovers`, `mesh_stretch`, `blink_issues`, `symmetry_defects`, `unmasked_parts`.
2. If any `containment_leaks`, `mesh_foldovers`, or `unmasked_parts` are reported, call `rig_auto_fix`:
   - Clamps overflowing facial feature vertices inside the deformed `Face` safe ellipse (`0.88x`, `0.90y`).
   - Runs iterative Laplacian relaxation on flipped/crushed triangles until fold-overs clear.
   - Attaches missing `Mask` clippings to `Pupil`/`Iris`/`Tongue` nodes.
3. Re-run `rig_audit` to verify `passed: true` and `score == 100`.

### Stage 5: X-Ray Visual Inspection & Semantic Soft-Brush Sculpting
When numbers alone are not enough or you want artistic fine-tuning:
1. Render an X-ray diagnostic image at an extreme pose using `param_set` + `render_diagnostic` (`out`, `width: 512`, `height: 512`):
   - **50px world grid + `(0,0)` axes** let you read exact pixel scales.
   - **Green ellipse** marks the `Face` containment boundary.
   - **Red triangles** mark inverted/flipped mesh fold-overs; **Orange** marks `>2.5x` stretch.
   - **Red crosshairs** mark overflowing facial feature vertices; **Yellow crosshairs** mark part pivots.
2. Render the 3×3 parameter sweep via `render_grid` (`param: "Head:: Yaw-Pitch"`, `wireframe: true`, `out: "head_grid.png"`) and inspect the PNG visually (`read_file`).
3. Sculpt local fixes using `deform_brush` with **semantic anchors** (`"center"`, `"left"`, `"right"`, `"top"`, `"bottom"`, `"top_left"`, `"top_right"`, `"bottom_left"`, `"bottom_right"`):
   ```json
   {
     "node_name": "Mouth",
     "param": "Head:: Yaw-Pitch",
     "kx": 1.0,
     "ky": 0.0,
     "anchor": "right",
     "radius_ratio": 0.4,
     "dx": -3.0,
     "dy": -1.5
   }
   ```
   Always re-run `rig_audit` after sculpting with `deform_brush`.

### Stage 6: Motion Verification & Export
1. Render the `Idle` animation + physics motion sheet via `render_anim_sheet` (`anim: "Idle"`, `frames: 8`, `out: "idle_sheet.png"`), and inspect the rendered sheet to confirm blinking, breathing, and hair physics sway look natural.
2. Save the final `.inp` model via `puppet_save` (`path: "output.inp"`). The output opens directly in official `Inochi Creator` and `Inochi Session`.

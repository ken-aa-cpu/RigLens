---
name: inox2d-vtuber-rigger
description: Zero-guessing VTuber/Live2D rigging playbook for inox2d-mcp and PhotoCraft. Follow this manual whenever importing layered PSD artwork, creating Inochi2D (.inp) puppets, rigging facial deformations, auditing mesh foldovers/containment leaks, or sculpting expressions via MCP.
---

# Inochi2D / Live2D Zero-Guessing Rigging Manual (`inox2d-mcp`)

Never guess raw vertex coordinates or eyeball whether a 2.5D head turn leaks out of the face contour. Work like an operator with checkpoints, atomic batches, deterministic audits, visual diffing, and animated GIF previews.

---

## Mandatory 6-Stage Operator Workflow

### Stage 1: Artwork Inspection, PSD Import & Tree Cleanup
1. If starting from a `.psd` file and `PhotoCraft` (`D:\app\PhotoCraft\photocraft-cli.exe`) is available, inspect layer structure first with `photocraft_inspect` (`path`).
2. Ensure parts follow semantic naming (EN or ZH) so proportional auto-rigging maps depths accurately:
   - `Face` / `臉` (`depth_z = 0.25`)
   - `Hair_Back` / `後髮` (`depth_z = -0.85` + hair sway)
   - `Hair_Front_L` / `Hair_Front_R` / `前髮` / `劉海` (`depth_z = 1.25` + hair sway)
   - `Hair_Side` / `側髮` (`depth_z = 0.65` + hair sway)
   - `Eye_L` / `左眼`, `Eye_R` / `右眼` (`depth_z = 1.0` + blink + gaze)
   - `Pupil` / `Iris` / `Highlight` / `瞳` / `高光` (clipped via `Mask`)
   - `Mouth` / `嘴` (`depth_z = 1.1` + `Mouth:: Open` + `Mouth:: Form` + `Mouth:: Viseme`)
   - `Brow` / `眉` (`depth_z = 1.05` + `Brow:: Left/Right`), `Nose` / `鼻` (`depth_z = 1.35`), `Ear` / `耳` (`depth_z = -0.55`)
   - `Torso` / `Body` / `身體` (`Breath`)
3. Import into the session using `puppet_import_psd` (`path`, `mesh_mode: "delaunay"` recommended for organic curved hair/face silhouettes or `"contour"`, `auto_rig: true`). To upgrade any existing Part mesh individually, call `mesh_auto_delaunay` (`node_name`, `step_px`).
4. Fix any hierarchy, zsort, transform, or clipping mistakes cleanly using `node_configure`, `node_set_mask`, `node_add_meshgroup`, or `node_remove`.

### Stage 2: Anatomical Telemetry (`rig_analyze_anatomy`)
Call `rig_analyze_anatomy` **before** manual tweaks.
- Inspect `face_anchor` (`center`, `size`) and each feature's `face_normalized_pos` (`[-1..1]` inside face bounds) and `face_margins_px` (`left`, `right`, `top`, `bottom`).
- If `rest_warnings` shows any feature `< 4px` from the face boundary at rest, reposition via `node_configure` or keep `yaw_px` conservative before binding extreme turns.

### Stage 3: Base Rigging, Vowel Visemes & Expression Synthesis
1. Run `rig_auto_rig_vtuber` if the model was assembled manually.
2. Upgrade mouth lip-sync with 3×3 2D A/I/U/E/O vowel visemes via `rig_auto_visemes` (`node_name: "Mouth"`), and rig eyebrows via `rig_auto_brow`.
3. Need missing anime expression layers (`blush`, `gloom`, `tears`, `angry`, `shock`)? Generate and auto-bind them proportionally to `Face` with one command:
   ```json
   { "kind": "blush" }
   ```
   via `node_synthesize_overlay` (or bind custom existing layers via `rig_bind_opacity`).
4. Use `session_checkpoint` before risky edits, `session_batch` to apply multi-step edits atomically, and `edit_undo` / `edit_redo` / `session_restore` whenever reverting an experiment.

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

### Stage 5: X-Ray Visual Inspection, Visual Diff & Semantic Soft-Brush Sculpting
1. Save a checkpoint (`session_checkpoint`, `{"name": "before_sculpt"}`) before sculpting.
2. Sculpt local refinements using `deform_brush` with **semantic anchors** (`"center"`, `"left"`, `"right"`, `"top"`, `"bottom"`, `"top_left"`, `"top_right"`, `"bottom_left"`, `"bottom_right"`).
3. Verify your exact visual delta using `render_diff` (`checkpoint: "before_sculpt"`, `out: "diff.png"`), which outputs `[Before | After | Pixel Heatmap]` + `changed_pixel_pct`.
4. Render an X-ray diagnostic image at extreme poses via `render_diagnostic` (`out: "diagnostic.png"`) and a 3×3 parameter sweep via `render_grid` (`param: "Head:: Yaw-Pitch"`, `wireframe: true`, `out: "head_grid.png"`).
5. Always re-run `rig_audit` after sculpting with `deform_brush`.

### Stage 6: Motion Verification, FigCraft Spec Card, Live OpenGL & Export
1. Render the `Idle` animation + physics motion sheet via `render_anim_sheet` (`anim: "Idle"`, `frames: 8`, `out: "idle_sheet.png"`).
2. Export a looping animated `.gif` preview via `render_gif` (`anim: "Idle"`, `frames: 24`, `fps: 15`, `out: "preview.gif"`) so humans can review blinking, breathing, and pendulum physics in one click.
3. Generate a studio FigCraft Rigging Spec Card (`craft_figcraft_card`, `out: "spec_card.png"`, rendered via `FigCraft`) summarizing the transparent puppet preview, mesh/parameter stats, and 6-point audit breakdown.
4. For human visual review and interactive GUI rigging in the Craft-style native desktop studio, launch `inox2d-mcp mcp --watch live.inp` alongside `cargo run -p inox2d-gui -- live.inp --watch live.inp` (2D XY Parameter Pads, Deform Brush `D`, Pose `P`, Timeline scrubber, `Ctrl+K` Command Palette, 1-click Auto-Fix, or `--screenshot` for visual verification) or `cargo run -p render-opengl live.inp` for GPU preview (`1`–`5` expressions, `B` blink, `M` visemes).
5. Save the final `.inp` model via `puppet_save` (`path: "output.inp"`). The output opens directly in official `Inochi Creator` and `Inochi Session`.

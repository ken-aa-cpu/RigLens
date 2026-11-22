use std::collections::{HashMap, HashSet};

use glam::{vec2, Vec2};
use image::RgbaImage;
use riglens_core::math::matrix::Matrix2d;
use riglens_core::node::components::Mesh;
use riglens_core::node::InoxNodeUuid;
use riglens_core::params::BindingValues;
use riglens_core::puppet::Puppet;

use super::contour::{
	chamfer_distance_to_mask, collapse_collinear_closed, douglas_peucker_closed, enforce_constrained_edges,
	extract_alpha_contour_loops, point_in_loops, point_to_segment_dist_sq, polygon_signed_area,
};
use super::mesh::{generate_contour_mesh, generate_grid_mesh, mesh_bounds};

#[derive(Clone, Copy)]
pub(super) struct DelaunayPt {
	pub(super) x: f64,
	pub(super) y: f64,
}

#[inline]
pub(super) fn orient2d_f64(a: DelaunayPt, b: DelaunayPt, c: DelaunayPt) -> f64 {
	(b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

#[inline]
pub(super) fn in_circumcircle_ccw(a: DelaunayPt, b: DelaunayPt, c: DelaunayPt, p: DelaunayPt) -> bool {
	let ax = a.x - p.x;
	let ay = a.y - p.y;
	let bx = b.x - p.x;
	let by = b.y - p.y;
	let cx = c.x - p.x;
	let cy = c.y - p.y;
	let det = (ax * ax + ay * ay) * (bx * cy - by * cx)
		- (bx * bx + by * by) * (ax * cy - ay * cx)
		+ (cx * cx + cy * cy) * (ax * by - ay * bx);
	det > 0.0
}

fn bowyer_watson_tris(pts: &[DelaunayPt], min_scale: f64) -> Vec<[usize; 3]> {
	let n = pts.len();
	if n < 3 {
		return Vec::new();
	}

	// Perturb predicates infinitesimally (Simulation of Simplicity) so exact circles never degenerate.
	let mut bw_pts: Vec<DelaunayPt> = pts
		.iter()
		.enumerate()
		.map(|(i, p)| {
			let jx = (((i.wrapping_mul(2654435761) & 1023) as f64) / 1023.0 - 0.5) * 1e-4;
			let jy = (((i.wrapping_mul(1597334677) & 1023) as f64) / 1023.0 - 0.5) * 1e-4;
			DelaunayPt {
				x: p.x + jx,
				y: p.y + jy,
			}
		})
		.collect();

	let (mut bx0, mut bx1) = (f64::INFINITY, f64::NEG_INFINITY);
	let (mut by0, mut by1) = (f64::INFINITY, f64::NEG_INFINITY);
	for p in &bw_pts {
		bx0 = bx0.min(p.x);
		bx1 = bx1.max(p.x);
		by0 = by0.min(p.y);
		by1 = by1.max(p.y);
	}
	let dmax = (bx1 - bx0).max(by1 - by0).max(min_scale).max(16.0);
	let mid_x = (bx0 + bx1) * 0.5;
	let mid_y = (by0 + by1) * 0.5;

	let s0 = DelaunayPt {
		x: mid_x - 20.0 * dmax,
		y: mid_y - dmax,
	};
	let s1 = DelaunayPt {
		x: mid_x + 20.0 * dmax,
		y: mid_y - dmax,
	};
	let s2 = DelaunayPt {
		x: mid_x,
		y: mid_y + 20.0 * dmax,
	};
	bw_pts.push(s0);
	bw_pts.push(s1);
	bw_pts.push(s2);

	let mut tris: Vec<[usize; 3]> = if orient2d_f64(bw_pts[n], bw_pts[n + 1], bw_pts[n + 2]) > 0.0 {
		vec![[n, n + 1, n + 2]]
	} else {
		vec![[n, n + 2, n + 1]]
	};

	for pi in 0..n {
		let p = bw_pts[pi];
		let mut edge_counts: HashMap<(usize, usize), (usize, (usize, usize))> = HashMap::new();
		let mut keep_tris = Vec::with_capacity(tris.len());

		for tri in tris {
			if in_circumcircle_ccw(bw_pts[tri[0]], bw_pts[tri[1]], bw_pts[tri[2]], p) {
				let edges = [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])];
				for (u, v) in edges {
					let key = (u.min(v), u.max(v));
					edge_counts
						.entry(key)
						.and_modify(|e| e.0 += 1)
						.or_insert((1usize, (u, v)));
				}
			} else {
				keep_tris.push(tri);
			}
		}

		for (_, (count, (u, v))) in edge_counts {
			if count == 1 {
				let area = orient2d_f64(bw_pts[u], bw_pts[v], p);
				if area > 1e-9 {
					keep_tris.push([u, v, pi]);
				} else if area < -1e-9 {
					keep_tris.push([v, u, pi]);
				}
			}
		}
		tris = keep_tris;
	}

	tris.into_iter()
		.filter(|t| t[0] < n && t[1] < n && t[2] < n)
		.collect()
}

/// Replace `node_id`'s vertex array with `new_verts`, recompute UVs smoothly from existing
/// vertex->UV correspondence, retriangulate with Bowyer-Watson Delaunay, and remap all
/// existing `BindingValues::Deform` keyframes onto the new topology via IDW interpolation.
pub fn retriangulate_points_preserving_deforms(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	new_verts: Vec<Vec2>,
) -> Result<(usize, usize), String> {
	if new_verts.len() < 3 {
		return Err("Mesh requires at least 3 vertices".to_owned());
	}
	let old_mesh = puppet
		.node_comps
		.get::<Mesh>(node_id)
		.ok_or_else(|| format!("Node {} does not have a Mesh component", node_id.0))?
		.clone();
	if old_mesh.vertices.is_empty() {
		return Err("Existing mesh has no vertices".to_owned());
	}

	// Derive linear fallback UV mapping from existing mesh bbox so added outer vertices map accurately.
	let (old_min, old_max) = mesh_bounds(&old_mesh);
	let (mut umin, mut vmin) = (f32::INFINITY, f32::INFINITY);
	let (mut umax, mut vmax) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
	for &uv in &old_mesh.uvs {
		umin = umin.min(uv.x);
		vmin = vmin.min(uv.y);
		umax = umax.max(uv.x);
		vmax = vmax.max(uv.y);
	}
	let span_v = (old_max - old_min).max(vec2(1.0, 1.0));
	let span_uv = vec2((umax - umin).max(1e-4), (vmax - vmin).max(1e-4));

	let mut new_uvs = Vec::with_capacity(new_verts.len());
	for &nv in &new_verts {
		if let Some((oi, _)) = old_mesh
			.vertices
			.iter()
			.enumerate()
			.find(|(_, &ov)| (ov - nv).length_squared() < 0.01)
		{
			new_uvs.push(*old_mesh.uvs.get(oi).unwrap_or(&vec2(0.5, 0.5)));
		} else {
			let tx = ((nv.x - old_min.x) / span_v.x).clamp(-0.2, 1.2);
			let ty = ((nv.y - old_min.y) / span_v.y).clamp(-0.2, 1.2);
			new_uvs.push(vec2((umin + tx * span_uv.x).clamp(0.0, 1.0), (vmin + ty * span_uv.y).clamp(0.0, 1.0)));
		}
	}

	let pts64: Vec<DelaunayPt> = new_verts
		.iter()
		.map(|v| DelaunayPt {
			x: v.x as f64,
			y: v.y as f64,
		})
		.collect();
	let raw_tris = bowyer_watson_tris(&pts64, (span_v.x.max(span_v.y)) as f64);
	let mut indices = Vec::with_capacity(raw_tris.len() * 3);
	for tri in raw_tris {
		let area = orient2d_f64(pts64[tri[0]], pts64[tri[1]], pts64[tri[2]]);
		if area.abs() <= 1e-6 {
			continue;
		}
		if area > 0.0 {
			indices.extend_from_slice(&[tri[0] as u32, tri[1] as u32, tri[2] as u32]);
		} else {
			indices.extend_from_slice(&[tri[0] as u32, tri[2] as u32, tri[1] as u32]);
		}
	}
	if indices.len() < 3 {
		return Err("Delaunay retriangulation produced no valid triangles (points may be collinear)".to_owned());
	}

	// Precompute IDW weights from old vertices -> new vertices so deform bindings survive edits.
	let map_weights: Vec<Vec<(usize, f32)>> = new_verts
		.iter()
		.map(|&nv| {
			let mut dists: Vec<(usize, f32)> = old_mesh
				.vertices
				.iter()
				.enumerate()
				.map(|(oi, &ov)| (oi, (ov - nv).length_squared()))
				.collect();
			dists.sort_by(|a, b| a.1.total_cmp(&b.1));
			if let Some(&(oi, d2)) = dists.first() {
				if d2 < 0.01 {
					return vec![(oi, 1.0)];
				}
			}
			let top: Vec<(usize, f32)> = dists
				.into_iter()
				.take(3)
				.map(|(oi, d2)| (oi, 1.0 / (d2 + 1e-4)))
				.collect();
			let sum_w: f32 = top.iter().map(|&(_, w)| w).sum::<f32>().max(1e-6);
			top.into_iter().map(|(oi, w)| (oi, w / sum_w)).collect()
		})
		.collect();

	for param in puppet.params.values_mut() {
		for binding in &mut param.bindings {
			if binding.node != node_id {
				continue;
			}
			if let BindingValues::Deform(ref mut mat) = binding.values {
				let old_mat = mat.clone();
				let (x_len, y_len) = (old_mat.width(), old_mat.height());
				let new_mat = Matrix2d::from_grid_transposed(x_len, y_len, |ix, iy| {
					let old_cell = old_mat.get(ix, iy);
					map_weights
						.iter()
						.map(|weights| {
							let Some(cell) = old_cell else {
								return Vec2::ZERO;
							};
							let mut acc = Vec2::ZERO;
							for &(oi, w) in weights {
								if let Some(&d) = cell.get(oi) {
									acc += d * w;
								}
							}
							acc
						})
						.collect()
				});
				*mat = new_mat;
			}
		}
	}

	let vert_count = new_verts.len();
	let tri_count = indices.len() / 3;
	puppet.node_comps.set(
		node_id,
		Mesh {
			vertices: new_verts,
			uvs: new_uvs,
			indices,
			origin: old_mesh.origin,
		},
	);
	puppet.reset_contexts();
	Ok((vert_count, tri_count))
}

/// High-density adaptive sampling circle (e.g. eye corners, mouth corners, elbow/knee joints)
/// passed to [`generate_delaunay_mesh_with_options`].
#[derive(Clone, Debug)]
pub struct RefinementZone {
	/// Local mesh coordinate center (same space as `Mesh.vertices`).
	pub center: Vec2,
	/// Influence radius in local mesh units.
	pub radius: f32,
	/// Local spacing factor relative to `step_px` (e.g. `0.35` produces ~3x finer triangles).
	pub density_scale: f32,
}

/// Options controlling boundary-constrained Bowyer-Watson Delaunay meshing.
#[derive(Clone, Debug)]
pub struct DelaunayMeshOptions {
	pub step_px: f32,
	pub alpha_threshold: u8,
	/// Signed pixel offset around alpha silhouette (`>= 0` expands outward margin).
	pub boundary_offset_px: f32,
	/// Enforce exact closed contour segments via Sloan edge recovery and polygon-parity culling.
	pub constrained: bool,
	/// Automatically refine triangle density around sharp boundary corners (>= 35 deg turn).
	pub refine_near_corners: bool,
	/// Explicit localized high-density regions (joints, expression anchor areas).
	pub refinement_zones: Vec<RefinementZone>,
}

impl Default for DelaunayMeshOptions {
	fn default() -> Self {
		Self {
			step_px: 22.0,
			alpha_threshold: 4,
			boundary_offset_px: 2.0,
			constrained: true,
			refine_near_corners: true,
			refinement_zones: Vec::new(),
		}
	}
}

/// Generate a boundary-conforming Bowyer-Watson Delaunay triangle mesh from an RGBA texture.
pub fn generate_delaunay_mesh(
	img: &RgbaImage,
	width: f32,
	height: f32,
	step_px: f32,
	alpha_threshold: u8,
) -> Mesh {
	generate_delaunay_mesh_with_options(
		img,
		width,
		height,
		&DelaunayMeshOptions {
			step_px,
			alpha_threshold,
			..Default::default()
		},
	)
}

/// Generate a constrained, adaptively refined Bowyer-Watson Delaunay mesh:
/// 1) Euclidean-chamfer signed boundary offset (`boundary_offset_px`).
/// 2) Ordered pixel-edge alpha-contour loop tracing + closed-loop Douglas-Peucker simplification.
/// 3) Corner detection + explicit `RefinementZone` adaptive hex interior lattice.
/// 4) Sloan edge-flipping constrained Delaunay recovery + exact polygon-parity silhouette culling.
pub fn generate_delaunay_mesh_with_options(
	img: &RgbaImage,
	width: f32,
	height: f32,
	opts: &DelaunayMeshOptions,
) -> Mesh {
	let iw = img.width() as usize;
	let ih = img.height() as usize;
	if iw < 3 || ih < 3 || width <= 1.0 || height <= 1.0 {
		return generate_grid_mesh(width.max(1.0), height.max(1.0), 2, 2, Vec2::ZERO);
	}

	let mut raw = vec![false; iw * ih];
	let mut any_opaque = false;
	for y in 0..ih {
		for x in 0..iw {
			if img.get_pixel(x as u32, y as u32).0[3] > opts.alpha_threshold {
				raw[y * iw + x] = true;
				any_opaque = true;
			}
		}
	}
	if !any_opaque {
		return generate_grid_mesh(width, height, 2, 2, Vec2::ZERO);
	}

	let offset = opts.boundary_offset_px;
	let mut mask = if offset >= 0.0 {
		let dist = chamfer_distance_to_mask(&raw, iw, ih, true);
		let thresh = offset + 1e-4;
		dist.into_iter().map(|d| d <= thresh).collect::<Vec<bool>>()
	} else {
		let dist = chamfer_distance_to_mask(&raw, iw, ih, false);
		let thresh = -offset + 1e-4;
		let eroded = dist.into_iter().map(|d| d > thresh).collect::<Vec<bool>>();
		if eroded.iter().any(|&v| v) {
			eroded
		} else {
			raw.clone()
		}
	};

	if !mask.iter().any(|&v| v) {
		mask = raw;
	}

	let w64 = width as f64;
	let h64 = height as f64;
	let sx = (w64 / iw as f64).max(0.01);
	let sy = (h64 / ih as f64).max(0.01);
	let step = if opts.step_px.is_finite() { opts.step_px } else { 24.0 };
	let spacing = (step.clamp(4.0, 256.0) as f64) * ((sx + sy) * 0.5);

	let grid_to_local = |vx: usize, vy: usize| -> DelaunayPt {
		DelaunayPt {
			x: (vx as f64 / iw as f64 - 0.5) * w64,
			y: (vy as f64 / ih as f64 - 0.5) * h64,
		}
	};

	let raw_loops = extract_alpha_contour_loops(&mask, iw, ih);
	if raw_loops.is_empty() {
		return generate_contour_mesh(img, width, height, 4, 4, opts.alpha_threshold);
	}

	let dp_eps = (spacing * 0.12).clamp(0.4, 3.0);
	let mut simplified_loops: Vec<Vec<DelaunayPt>> = raw_loops
		.into_iter()
		.filter_map(|loop_grid| {
			let local: Vec<DelaunayPt> = loop_grid
				.into_iter()
				.map(|(vx, vy)| grid_to_local(vx, vy))
				.collect();
			let clean = collapse_collinear_closed(&local, 1e-6);
			if clean.len() < 3 {
				return None;
			}
			let simp = douglas_peucker_closed(&clean, dp_eps);
			if simp.len() < 3 || polygon_signed_area(&simp).abs() < (spacing * spacing * 0.05).max(1.0) {
				None
			} else {
				Some(simp)
			}
		})
		.collect();

	if simplified_loops.is_empty() {
		return generate_contour_mesh(img, width, height, 4, 4, opts.alpha_threshold);
	}

	// Collect refinement zones (explicit caller joints first, then sharp boundary corners)
	let mut active_zones: Vec<RefinementZone> = opts.refinement_zones.clone();
	if opts.refine_near_corners {
		let mut corner_cands: Vec<(f64, Vec2)> = Vec::new();
		for lp in &simplified_loops {
			let m = lp.len();
			for i in 0..m {
				let prev = lp[(i + m - 1) % m];
				let curr = lp[i];
				let next = lp[(i + 1) % m];
				let d1x = curr.x - prev.x;
				let d1y = curr.y - prev.y;
				let d2x = next.x - curr.x;
				let d2y = next.y - curr.y;
				let l1 = (d1x * d1x + d1y * d1y).sqrt();
				let l2 = (d2x * d2x + d2y * d2y).sqrt();
				if l1 > 1e-6 && l2 > 1e-6 {
					let cos_ang = (d1x * d2x + d1y * d2y) / (l1 * l2);
					if cos_ang <= 0.81915 {
						corner_cands.push((cos_ang, vec2(curr.x as f32, curr.y as f32)));
					}
				}
			}
		}
		corner_cands.sort_by(|a, b| a.0.total_cmp(&b.0));
		let min_sep_sq = (spacing * 0.65).powi(2) as f32;
		let mut added_corners = 0usize;
		for (_, cpos) in corner_cands {
			if added_corners >= 64 {
				break;
			}
			let too_close = active_zones
				.iter()
				.any(|z| (z.center - cpos).length_squared() < min_sep_sq);
			if !too_close {
				active_zones.push(RefinementZone {
					center: cpos,
					radius: (spacing * 1.35) as f32,
					density_scale: 0.45,
				});
				added_corners += 1;
			}
		}
	}

	let local_spacing_at = |x: f64, y: f64| -> f64 {
		let mut scale = 1.0f64;
		let p = vec2(x as f32, y as f32);
		for z in &active_zones {
			if z.radius > 0.0 && (z.center - p).length_squared() <= z.radius * z.radius {
				scale = scale.min((z.density_scale as f64).clamp(0.2, 1.0));
			}
		}
		(spacing * scale).max(2.0)
	};

	// Resample long boundary segments adaptively so edges near joints match interior density.
	for lp in &mut simplified_loops {
		let m = lp.len();
		let mut resampled = Vec::with_capacity(m * 2);
		for i in 0..m {
			let a = lp[i];
			let b = lp[(i + 1) % m];
			let dx = b.x - a.x;
			let dy = b.y - a.y;
			let seg_len = (dx * dx + dy * dy).sqrt();
			let target = local_spacing_at((a.x + b.x) * 0.5, (a.y + b.y) * 0.5).max(2.0);
			let steps = (seg_len / target).ceil().max(1.0).min(128.0) as usize;
			for s in 0..steps {
				let t = s as f64 / steps as f64;
				resampled.push(DelaunayPt {
					x: a.x + dx * t,
					y: a.y + dy * t,
				});
			}
		}
		*lp = resampled;
	}

	// Build point array: boundary loop vertices first (exact index rings), then adaptive interior hex points.
	let mut pts: Vec<DelaunayPt> = Vec::new();
	let cell_size = spacing.max(4.0);
	let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::new();

	let bucket_insert = |pt: DelaunayPt, pts: &mut Vec<DelaunayPt>, buckets: &mut HashMap<(i32, i32), Vec<usize>>| -> usize {
		let cx = (pt.x / cell_size).floor() as i32;
		let cy = (pt.y / cell_size).floor() as i32;
		for dy in -1..=1 {
			for dx in -1..=1 {
				if let Some(list) = buckets.get(&(cx + dx, cy + dy)) {
					for &idx in list {
						let ex = pts[idx].x - pt.x;
						let ey = pts[idx].y - pt.y;
						if ex * ex + ey * ey < 1e-6 {
							return idx;
						}
					}
				}
			}
		}
		let idx = pts.len();
		pts.push(pt);
		buckets.entry((cx, cy)).or_default().push(idx);
		idx
	};

	let mut constrained_edges: Vec<(usize, usize)> = Vec::new();
	let mut constrained_seen: HashSet<(usize, usize)> = HashSet::new();
	let mut segs: Vec<(DelaunayPt, DelaunayPt)> = Vec::new();

	for lp in &simplified_loops {
		let ring_idxs: Vec<usize> = lp.iter().map(|&p| bucket_insert(p, &mut pts, &mut buckets)).collect();
		let m = ring_idxs.len();
		for i in 0..m {
			let u = ring_idxs[i];
			let v = ring_idxs[(i + 1) % m];
			if u != v {
				let key = (u.min(v), u.max(v));
				if constrained_seen.insert(key) {
					constrained_edges.push((u, v));
				}
				segs.push((pts[u], pts[v]));
			}
		}
	}

	let try_insert_interior = |pt: DelaunayPt,
	                           local_step: f64,
	                           pts: &mut Vec<DelaunayPt>,
	                           buckets: &mut HashMap<(i32, i32), Vec<usize>>| {
		if !point_in_loops(pt, &simplified_loops) {
			return;
		}
		let clear_sq = (local_step * 0.34).max(0.75).powi(2);
		for &(a, b) in &segs {
			if point_to_segment_dist_sq(pt, a, b) < clear_sq {
				return;
			}
		}
		let min_d2 = (local_step * 0.68).max(1.25).powi(2);
		let search_cells = ((local_step / cell_size).ceil() as i32).max(1);
		let cx = (pt.x / cell_size).floor() as i32;
		let cy = (pt.y / cell_size).floor() as i32;
		for dy in -search_cells..=search_cells {
			for dx in -search_cells..=search_cells {
				if let Some(list) = buckets.get(&(cx + dx, cy + dy)) {
					for &idx in list {
						let ex = pts[idx].x - pt.x;
						let ey = pts[idx].y - pt.y;
						if ex * ex + ey * ey < min_d2 {
							return;
						}
					}
				}
			}
		}
		let idx = pts.len();
		pts.push(pt);
		buckets.entry((cx, cy)).or_default().push(idx);
	};

	// Pass 1: Dense hex lattice inside refinement zones (finest zones first)
	let mut zones_sorted = active_zones.clone();
	zones_sorted.sort_by(|a, b| a.density_scale.total_cmp(&b.density_scale));
	for z in &zones_sorted {
		if z.radius <= 1.0 {
			continue;
		}
		let z_step = (spacing * (z.density_scale as f64).clamp(0.2, 1.0)).max(2.0);
		let r = (z.radius as f64).min(w64.max(h64));
		let r2 = r * r;
		let h_dy = z_step * 0.8660254037844386;
		let mut row = 0usize;
		let mut ly = z.center.y as f64 - r + h_dy * 0.5;
		while ly <= z.center.y as f64 + r {
			let off = if row % 2 == 1 { z_step * 0.5 } else { 0.0 };
			let mut lx = z.center.x as f64 - r + off;
			while lx <= z.center.x as f64 + r {
				let dx = lx - z.center.x as f64;
				let dy = ly - z.center.y as f64;
				if dx * dx + dy * dy <= r2 {
					try_insert_interior(DelaunayPt { x: lx, y: ly }, z_step, &mut pts, &mut buckets);
				}
				lx += z_step;
			}
			row += 1;
			ly += h_dy;
		}
	}

	// Pass 2: Global interior staggered hex lattice across silhouette bbox
	let (mut bmin_x, mut bmax_x) = (f64::INFINITY, f64::NEG_INFINITY);
	let (mut bmin_y, mut bmax_y) = (f64::INFINITY, f64::NEG_INFINITY);
	for lp in &simplified_loops {
		for &p in lp {
			bmin_x = bmin_x.min(p.x);
			bmax_x = bmax_x.max(p.x);
			bmin_y = bmin_y.min(p.y);
			bmax_y = bmax_y.max(p.y);
		}
	}
	let hex_dx = spacing.max(2.0);
	let hex_dy = hex_dx * 0.8660254037844386;
	let mut row_idx = 0usize;
	let mut ly = bmin_y + hex_dy * 0.5;
	while ly <= bmax_y - hex_dy * 0.25 {
		let offset_x = if row_idx % 2 == 1 { hex_dx * 0.5 } else { 0.0 };
		let mut lx = bmin_x + offset_x;
		while lx <= bmax_x {
			let l_step = local_spacing_at(lx, ly);
			try_insert_interior(DelaunayPt { x: lx, y: ly }, l_step, &mut pts, &mut buckets);
			lx += hex_dx;
		}
		row_idx += 1;
		ly += hex_dy;
	}

	let n = pts.len();
	if n < 3 {
		return generate_contour_mesh(img, width, height, 4, 4, opts.alpha_threshold);
	}

	let mut tris = bowyer_watson_tris(&pts, w64.max(h64));
	if opts.constrained && !constrained_edges.is_empty() {
		let sos_pts: Vec<DelaunayPt> = pts
			.iter()
			.enumerate()
			.map(|(i, p)| {
				let jx = (((i.wrapping_mul(2654435761) & 1023) as f64) / 1023.0 - 0.5) * 1e-4;
				let jy = (((i.wrapping_mul(1597334677) & 1023) as f64) / 1023.0 - 0.5) * 1e-4;
				DelaunayPt {
					x: p.x + jx,
					y: p.y + jy,
				}
			})
			.collect();
		enforce_constrained_edges(&mut tris, &sos_pts, &constrained_edges);
	}

	let sample_mask = |lx: f64, ly: f64| -> bool {
		let u = lx / w64 + 0.5;
		let v = ly / h64 + 0.5;
		let ix = (u * iw as f64).floor() as isize;
		let iy = (v * ih as f64).floor() as isize;
		if ix < 0 || iy < 0 || ix >= iw as isize || iy >= ih as isize {
			return false;
		}
		mask[iy as usize * iw + ix as usize]
	};

	let mut clean_tris: Vec<[usize; 3]> = Vec::new();
	for tri in tris {
		let pa = pts[tri[0]];
		let pb = pts[tri[1]];
		let pc = pts[tri[2]];
		let area = orient2d_f64(pa, pb, pc);
		if area.abs() <= 1e-6 {
			continue;
		}
		let gx = (pa.x + pb.x + pc.x) / 3.0;
		let gy = (pa.y + pb.y + pc.y) / 3.0;
		let inside = if opts.constrained {
			point_in_loops(DelaunayPt { x: gx, y: gy }, &simplified_loops)
		} else {
			let m0 = sample_mask((pa.x + pb.x) * 0.5, (pa.y + pb.y) * 0.5) as u8;
			let m1 = sample_mask((pb.x + pc.x) * 0.5, (pb.y + pc.y) * 0.5) as u8;
			let m2 = sample_mask((pc.x + pa.x) * 0.5, (pc.y + pa.y) * 0.5) as u8;
			sample_mask(gx, gy) && (m0 + m1 + m2 >= 2)
		};
		if !inside {
			continue;
		}
		if area > 0.0 {
			clean_tris.push(tri);
		} else {
			clean_tris.push([tri[0], tri[2], tri[1]]);
		}
	}

	if clean_tris.is_empty() {
		return generate_contour_mesh(img, width, height, 4, 4, opts.alpha_threshold);
	}

	// Compact unused vertices.
	let mut remap: HashMap<usize, u32> = HashMap::new();
	let mut vertices: Vec<Vec2> = Vec::new();
	let mut uvs: Vec<Vec2> = Vec::new();
	let mut indices: Vec<u32> = Vec::with_capacity(clean_tris.len() * 3);

	for tri in clean_tris {
		for old_idx in tri {
			let new_idx = *remap.entry(old_idx).or_insert_with(|| {
				let idx = vertices.len() as u32;
				let p = pts[old_idx];
				vertices.push(vec2(p.x as f32, p.y as f32));
				let u = (p.x / w64 + 0.5).clamp(0.0, 1.0) as f32;
				let v = (p.y / h64 + 0.5).clamp(0.0, 1.0) as f32;
				uvs.push(vec2(u, v));
				idx
			});
			indices.push(new_idx);
		}
	}

	Mesh {
		vertices,
		uvs,
		indices,
		origin: Vec2::ZERO,
	}
}

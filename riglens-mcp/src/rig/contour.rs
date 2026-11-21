use std::collections::{HashMap, HashSet, VecDeque};

use super::delaunay::{in_circumcircle_ccw, orient2d_f64, DelaunayPt};

pub(super) fn chamfer_distance_to_mask(raw: &[bool], iw: usize, ih: usize, to_foreground: bool) -> Vec<f32> {
	const INF: f32 = 1e6;
	const SQRT2: f32 = 1.4142135;
	let mut dist = vec![INF; iw * ih];
	for y in 0..ih {
		for x in 0..iw {
			let i = y * iw + x;
			if to_foreground {
				if raw[i] {
					dist[i] = 0.0;
				}
			} else if !raw[i] {
				dist[i] = 0.0;
			} else {
				let edge_d = ((x + 1).min(iw - x).min(y + 1).min(ih - y)) as f32;
				dist[i] = edge_d;
			}
		}
	}

	// Forward pass
	for y in 0..ih {
		for x in 0..iw {
			let i = y * iw + x;
			let mut d = dist[i];
			if x > 0 {
				d = d.min(dist[i - 1] + 1.0);
			}
			if y > 0 {
				d = d.min(dist[(y - 1) * iw + x] + 1.0);
				if x > 0 {
					d = d.min(dist[(y - 1) * iw + x - 1] + SQRT2);
				}
				if x + 1 < iw {
					d = d.min(dist[(y - 1) * iw + x + 1] + SQRT2);
				}
			}
			dist[i] = d;
		}
	}

	// Backward pass
	for y in (0..ih).rev() {
		for x in (0..iw).rev() {
			let i = y * iw + x;
			let mut d = dist[i];
			if x + 1 < iw {
				d = d.min(dist[i + 1] + 1.0);
			}
			if y + 1 < ih {
				d = d.min(dist[(y + 1) * iw + x] + 1.0);
				if x > 0 {
					d = d.min(dist[(y + 1) * iw + x - 1] + SQRT2);
				}
				if x + 1 < iw {
					d = d.min(dist[(y + 1) * iw + x + 1] + SQRT2);
				}
			}
			dist[i] = d;
		}
	}
	dist
}

pub(super) fn extract_alpha_contour_loops(mask: &[bool], iw: usize, ih: usize) -> Vec<Vec<(usize, usize)>> {
	let gw = iw + 1;
	let mut out_dirs = vec![0u8; gw * (ih + 1)];

	for y in 0..ih {
		for x in 0..iw {
			if !mask[y * iw + x] {
				continue;
			}
			if y == 0 || !mask[(y - 1) * iw + x] {
				out_dirs[y * gw + x] |= 1 << 0; // +X
			}
			if x + 1 == iw || !mask[y * iw + x + 1] {
				out_dirs[y * gw + (x + 1)] |= 1 << 1; // +Y
			}
			if y + 1 == ih || !mask[(y + 1) * iw + x] {
				out_dirs[(y + 1) * gw + (x + 1)] |= 1 << 2; // -X
			}
			if x == 0 || !mask[y * iw + x - 1] {
				out_dirs[(y + 1) * gw + x] |= 1 << 3; // -Y
			}
		}
	}

	let step_dir = |vx: usize, vy: usize, dir: u8| -> (usize, usize) {
		match dir {
			0 => (vx + 1, vy),
			1 => (vx, vy + 1),
			2 => (vx - 1, vy),
			_ => (vx, vy - 1),
		}
	};

	let mut loops = Vec::new();
	for sy in 0..=ih {
		for sx in 0..=iw {
			loop {
				let bits = out_dirs[sy * gw + sx];
				if bits == 0 {
					break;
				}
				let start_dir = (0..4u8).find(|&d| (bits & (1 << d)) != 0).unwrap();
				let mut cx = sx;
				let mut cy = sy;
				let mut dir = start_dir;
				let mut ring = Vec::new();
				let max_steps = (gw * (ih + 1)) * 2 + 4;

				for _ in 0..max_steps {
					let slot = &mut out_dirs[cy * gw + cx];
					if (*slot & (1 << dir)) == 0 {
						break;
					}
					*slot &= !(1 << dir);
					ring.push((cx, cy));
					let (nx, ny) = step_dir(cx, cy, dir);
					cx = nx;
					cy = ny;
					let next_bits = out_dirs[cy * gw + cx];
					// Keep filled interior on the right side of edge while closing saddle loops on start_dir.
					let prefs = [(dir + 1) % 4, dir, (dir + 3) % 4, (dir + 2) % 4];
					let next_dir = prefs.iter().copied().find(|&d| {
						(cx == sx && cy == sy && d == start_dir) || (next_bits & (1 << d)) != 0
					});
					match next_dir {
						Some(nd) if cx == sx && cy == sy && nd == start_dir => break,
						Some(nd) => dir = nd,
						None => break,
					}
				}
				if ring.len() >= 3 {
					loops.push(ring);
				}
			}
		}
	}
	loops
}

pub(super) fn collapse_collinear_closed(pts: &[DelaunayPt], tol: f64) -> Vec<DelaunayPt> {
	let n = pts.len();
	if n < 3 {
		return pts.to_vec();
	}
	let mut out = Vec::with_capacity(n);
	for i in 0..n {
		let prev = pts[(i + n - 1) % n];
		let curr = pts[i];
		let next = pts[(i + 1) % n];
		let area = orient2d_f64(prev, curr, next);
		let d2_prev = (curr.x - prev.x).powi(2) + (curr.y - prev.y).powi(2);
		let d2_next = (next.x - curr.x).powi(2) + (next.y - curr.y).powi(2);
		let d_span = ((next.x - prev.x).powi(2) + (next.y - prev.y).powi(2)).sqrt();
		if d2_prev > 1e-12 && d2_next > 1e-12 && (d_span <= 1e-12 || area.abs() > tol * d_span) {
			out.push(curr);
		}
	}
	out
}

fn perp_dist_point_segment(p: DelaunayPt, a: DelaunayPt, b: DelaunayPt) -> f64 {
	let dx = b.x - a.x;
	let dy = b.y - a.y;
	let len = (dx * dx + dy * dy).sqrt();
	if len <= 1e-12 {
		return ((p.x - a.x).powi(2) + (p.y - a.y).powi(2)).sqrt();
	}
	((dy * p.x - dx * p.y + b.x * a.y - b.y * a.x).abs()) / len
}

fn dp_open_recursive(pts: &[DelaunayPt], eps: f64, out: &mut Vec<DelaunayPt>) {
	if pts.len() <= 2 {
		out.push(pts[0]);
		return;
	}
	let a = pts[0];
	let b = pts[pts.len() - 1];
	let mut best_idx = 0usize;
	let mut best_d = -1.0f64;
	for (i, &p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
		let d = perp_dist_point_segment(p, a, b);
		if d > best_d {
			best_d = d;
			best_idx = i;
		}
	}
	if best_d > eps && best_idx > 0 {
		dp_open_recursive(&pts[..=best_idx], eps, out);
		dp_open_recursive(&pts[best_idx..], eps, out);
	} else {
		out.push(a);
	}
}

pub(super) fn douglas_peucker_closed(pts: &[DelaunayPt], eps: f64) -> Vec<DelaunayPt> {
	let n = pts.len();
	if n <= 4 {
		return pts.to_vec();
	}
	let mut i_a = 0usize;
	let mut best_d2 = -1.0f64;
	for (i, &p) in pts.iter().enumerate().skip(1) {
		let d2 = (p.x - pts[0].x).powi(2) + (p.y - pts[0].y).powi(2);
		if d2 > best_d2 {
			best_d2 = d2;
			i_a = i;
		}
	}
	let mut i_b = (i_a + 1) % n;
	let mut best_d2_b = -1.0f64;
	for (i, &p) in pts.iter().enumerate() {
		let d2 = (p.x - pts[i_a].x).powi(2) + (p.y - pts[i_a].y).powi(2);
		if d2 > best_d2_b {
			best_d2_b = d2;
			i_b = i;
		}
	}
	if i_a == i_b {
		return pts.to_vec();
	}

	let mut chain1 = Vec::new();
	let mut k = i_a;
	loop {
		chain1.push(pts[k]);
		if k == i_b {
			break;
		}
		k = (k + 1) % n;
	}
	let mut chain2 = Vec::new();
	let mut k = i_b;
	loop {
		chain2.push(pts[k]);
		if k == i_a {
			break;
		}
		k = (k + 1) % n;
	}

	let mut res = Vec::new();
	dp_open_recursive(&chain1, eps, &mut res);
	dp_open_recursive(&chain2, eps, &mut res);
	collapse_collinear_closed(&res, 1e-6)
}

pub(super) fn polygon_signed_area(pts: &[DelaunayPt]) -> f64 {
	let n = pts.len();
	let mut s = 0.0f64;
	for i in 0..n {
		let a = pts[i];
		let b = pts[(i + 1) % n];
		s += a.x * b.y - b.x * a.y;
	}
	0.5 * s
}

pub(super) fn point_in_loops(p: DelaunayPt, loops: &[Vec<DelaunayPt>]) -> bool {
	let mut inside = false;
	for lp in loops {
		let m = lp.len();
		if m < 3 {
			continue;
		}
		for i in 0..m {
			let a = lp[i];
			let b = lp[(i + 1) % m];
			let cond = (a.y > p.y) != (b.y > p.y);
			if cond {
				let x_int = (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x;
				if p.x < x_int {
					inside = !inside;
				}
			}
		}
	}
	inside
}

pub(super) fn point_to_segment_dist_sq(p: DelaunayPt, a: DelaunayPt, b: DelaunayPt) -> f64 {
	let abx = b.x - a.x;
	let aby = b.y - a.y;
	let len2 = abx * abx + aby * aby;
	if len2 <= 1e-12 {
		return (p.x - a.x).powi(2) + (p.y - a.y).powi(2);
	}
	let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0);
	let px = a.x + abx * t;
	let py = a.y + aby * t;
	(p.x - px).powi(2) + (p.y - py).powi(2)
}

fn segments_properly_intersect(a: DelaunayPt, b: DelaunayPt, c: DelaunayPt, d: DelaunayPt) -> bool {
	let o1 = orient2d_f64(a, b, c);
	let o2 = orient2d_f64(a, b, d);
	let o3 = orient2d_f64(c, d, a);
	let o4 = orient2d_f64(c, d, b);
	let opp12 = (o1 > 1e-8 && o2 < -1e-8) || (o1 < -1e-8 && o2 > 1e-8);
	let opp34 = (o3 > 1e-8 && o4 < -1e-8) || (o3 < -1e-8 && o4 > 1e-8);
	opp12 && opp34
}

#[inline]
fn edge_key_uv(u: usize, v: usize) -> (usize, usize) {
	if u < v {
		(u, v)
	} else {
		(v, u)
	}
}

fn remove_tri_from_map(tris: &[[usize; 3]], edge_tris: &mut HashMap<(usize, usize), Vec<usize>>, ti: usize) {
	let t = tris[ti];
	for &(a, b) in &[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
		let k = edge_key_uv(a, b);
		if let Some(list) = edge_tris.get_mut(&k) {
			list.retain(|&x| x != ti);
			if list.is_empty() {
				edge_tris.remove(&k);
			}
		}
	}
}

fn add_tri_to_map(tris: &[[usize; 3]], edge_tris: &mut HashMap<(usize, usize), Vec<usize>>, ti: usize) {
	let t = tris[ti];
	for &(a, b) in &[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
		let k = edge_key_uv(a, b);
		let list = edge_tris.entry(k).or_default();
		if !list.contains(&ti) {
			list.push(ti);
		}
	}
}

fn extract_directed_opposite(tri: [usize; 3], p: usize, q: usize) -> Option<usize> {
	if tri[0] == p && tri[1] == q {
		Some(tri[2])
	} else if tri[1] == p && tri[2] == q {
		Some(tri[0])
	} else if tri[2] == p && tri[0] == q {
		Some(tri[1])
	} else {
		None
	}
}

pub(super) fn enforce_constrained_edges(
	tris: &mut Vec<[usize; 3]>,
	pts: &[DelaunayPt],
	constrained_edges: &[(usize, usize)],
) {
	let mut edge_tris: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
	for ti in 0..tris.len() {
		add_tri_to_map(tris, &mut edge_tris, ti);
	}
	let constrained_set: HashSet<(usize, usize)> = constrained_edges
		.iter()
		.map(|&(u, v)| edge_key_uv(u, v))
		.collect();

	for &(u, v) in constrained_edges {
		let target_key = edge_key_uv(u, v);
		if edge_tris.contains_key(&target_key) {
			continue;
		}
		let mut crossing: VecDeque<(usize, usize)> = edge_tris
			.iter()
			.filter_map(|(&ek, tlist)| {
				if tlist.len() == 2
					&& !constrained_set.contains(&ek)
					&& ek.0 != u
					&& ek.0 != v
					&& ek.1 != u
					&& ek.1 != v
					&& segments_properly_intersect(pts[u], pts[v], pts[ek.0], pts[ek.1])
				{
					Some(ek)
				} else {
					None
				}
			})
			.collect();

		let mut new_edges: Vec<(usize, usize)> = Vec::new();
		let mut streak = 0usize;
		while let Some(ek) = crossing.pop_front() {
			let Some(tlist) = edge_tris.get(&ek) else {
				continue;
			};
			if tlist.len() != 2 {
				continue;
			}
			let t1 = tlist[0];
			let t2 = tlist[1];
			let (p, q, r, s) = if let (Some(r), Some(s)) = (
				extract_directed_opposite(tris[t1], ek.0, ek.1),
				extract_directed_opposite(tris[t2], ek.1, ek.0),
			) {
				(ek.0, ek.1, r, s)
			} else if let (Some(r), Some(s)) = (
				extract_directed_opposite(tris[t1], ek.1, ek.0),
				extract_directed_opposite(tris[t2], ek.0, ek.1),
			) {
				(ek.1, ek.0, r, s)
			} else {
				continue;
			};

			if r == s {
				continue;
			}

			// Quad (p, s, q, r) is strictly convex iff both flipped triangles are CCW.
			if orient2d_f64(pts[r], pts[p], pts[s]) > 1e-8 && orient2d_f64(pts[s], pts[q], pts[r]) > 1e-8 {
				remove_tri_from_map(tris, &mut edge_tris, t1);
				remove_tri_from_map(tris, &mut edge_tris, t2);
				tris[t1] = [r, p, s];
				tris[t2] = [s, q, r];
				add_tri_to_map(tris, &mut edge_tris, t1);
				add_tri_to_map(tris, &mut edge_tris, t2);
				streak = 0;

				let nk = edge_key_uv(r, s);
				if r != u && r != v && s != u && s != v && segments_properly_intersect(pts[u], pts[v], pts[r], pts[s]) {
					crossing.push_back(nk);
				} else {
					new_edges.push(nk);
				}
			} else {
				crossing.push_back(ek);
				streak += 1;
				if streak > crossing.len() * 2 + 16 {
					break;
				}
			}
		}

		// Step 2: Restore Delaunay condition on newly created unconstrained edges.
		let max_swaps = new_edges.len() * 12 + 64;
		let mut swaps = 0usize;
		while let Some(ek) = new_edges.pop() {
			if swaps >= max_swaps || ek == target_key || constrained_set.contains(&ek) {
				continue;
			}
			let Some(tlist) = edge_tris.get(&ek) else {
				continue;
			};
			if tlist.len() != 2 {
				continue;
			}
			let t1 = tlist[0];
			let t2 = tlist[1];
			let (p, q, r, s) = if let (Some(r), Some(s)) = (
				extract_directed_opposite(tris[t1], ek.0, ek.1),
				extract_directed_opposite(tris[t2], ek.1, ek.0),
			) {
				(ek.0, ek.1, r, s)
			} else if let (Some(r), Some(s)) = (
				extract_directed_opposite(tris[t1], ek.1, ek.0),
				extract_directed_opposite(tris[t2], ek.0, ek.1),
			) {
				(ek.1, ek.0, r, s)
			} else {
				continue;
			};

			if r != s
				&& orient2d_f64(pts[r], pts[p], pts[s]) > 1e-8
				&& orient2d_f64(pts[s], pts[q], pts[r]) > 1e-8
				&& in_circumcircle_ccw(pts[p], pts[q], pts[r], pts[s])
			{
				remove_tri_from_map(tris, &mut edge_tris, t1);
				remove_tri_from_map(tris, &mut edge_tris, t2);
				tris[t1] = [r, p, s];
				tris[t2] = [s, q, r];
				add_tri_to_map(tris, &mut edge_tris, t1);
				add_tri_to_map(tris, &mut edge_tris, t2);
				swaps += 1;
				for (ea, eb) in [(p, s), (s, q), (q, r), (r, p)] {
					let nek = edge_key_uv(ea, eb);
					if nek != target_key && !constrained_set.contains(&nek) {
						new_edges.push(nek);
					}
				}
			}
		}
	}
}

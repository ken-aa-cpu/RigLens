mod deform_stack;
mod vertex_buffers;

use std::collections::{HashMap, HashSet};
use std::mem::swap;

use glam::{vec2, vec3, Mat4, Vec2};

use crate::math::curve::{closest_point, evaluate_derivative, evaluate_point};
use crate::math::deform::Deform;
use crate::node::{
	components::{
		DeformSource, DeformStack, GridDeformer, Mask, Masks, Mesh, MeshGroup, PathDeformer, TransformStore, ZSort,
	},
	drawables::{CompositeComponents, DrawableKind, TexturedMeshComponents},
	InoxNodeUuid,
};
use crate::params::BindingValues;
use crate::puppet::{InoxNodeTree, Puppet, World};

pub use vertex_buffers::VertexBuffers;

/// Additional info per node for rendering a TexturedMesh:
/// - offset and length of array for mesh point coordinates
/// - offset and length of array for indices of mesh points defining the mesh
///
/// inside `puppet.render_ctx_vertex_buffers`.
pub struct TexturedMeshRenderCtx {
	pub index_offset: u32,
	pub vert_offset: u32,
	pub index_len: usize,
	pub vert_len: usize,
}

/// Additional info per node for rendering a Composite.
pub struct CompositeRenderCtx {
	pub zsorted_children_list: Vec<InoxNodeUuid>,
}

/// Additional struct attached to a puppet for rendering.
pub struct RenderCtx {
	/// General compact data buffers for interfacing with the GPU.
	pub vertex_buffers: VertexBuffers,
	/// All nodes that need respective draw method calls:
	/// - including standalone parts and composite parents,
	/// - excluding (TODO: plain mesh masks) and composite children.
	pub root_drawables_zsorted: Vec<InoxNodeUuid>,
	/// Cached closest curve parameter `t` per `(path_deformer_uuid, descendant_target_uuid)` vertex.
	path_t_cache: HashMap<(InoxNodeUuid, InoxNodeUuid), Vec<f32>>,
}

fn rest_absolute_transforms(nodes: &InoxNodeTree) -> HashMap<InoxNodeUuid, Mat4> {
	let mut out = HashMap::new();
	let root_id = nodes.root_node_id;
	let root_mat = nodes
		.get_node(root_id)
		.map(|n| n.trans_offset.to_matrix())
		.unwrap_or(Mat4::IDENTITY);
	out.insert(root_id, root_mat);
	for node in nodes.pre_order_iter().skip(1) {
		let base = if node.lock_to_root {
			root_mat
		} else {
			let parent = nodes.get_parent(node.uuid);
			out.get(&parent.uuid).copied().unwrap_or(root_mat)
		};
		out.insert(node.uuid, base * node.trans_offset.to_matrix());
	}
	out
}

fn node_must_propagate(comps: &World, id: InoxNodeUuid) -> bool {
	comps.get::<MeshGroup>(id).is_none() && comps.get::<GridDeformer>(id).is_none()
}

fn collect_propagated_descendants(nodes: &InoxNodeTree, comps: &World, root: InoxNodeUuid) -> Vec<InoxNodeUuid> {
	let mut out = Vec::new();
	let mut stack: Vec<InoxNodeUuid> = nodes.get_children(root).map(|c| c.uuid).collect();
	while let Some(cid) = stack.pop() {
		out.push(cid);
		if node_must_propagate(comps, cid) {
			for gc in nodes.get_children(cid) {
				stack.push(gc.uuid);
			}
		}
	}
	out
}

fn node_deform_target_verts(comps: &World, id: InoxNodeUuid) -> Option<(Vec<Vec2>, Vec2)> {
	if let Some(mesh) = comps.get::<Mesh>(id) {
		return Some((mesh.vertices.clone(), mesh.origin));
	}
	if let Some(grid) = comps.get::<GridDeformer>(id) {
		return Some((grid.rest_vertices(), Vec2::ZERO));
	}
	None
}

fn combine_node_deforms(nodes: &InoxNodeTree, comps: &World, id: InoxNodeUuid, len: usize) -> Option<Vec<Vec2>> {
	let stack = comps.get::<DeformStack>(id)?;
	if stack.deform_len != len {
		return None;
	}
	let mut out = vec![Vec2::ZERO; len];
	stack.combine(nodes, comps, &mut out);
	Some(out)
}

impl RenderCtx {
	/// MODIFIES puppet. In addition to initializing self, installs render contexts in the World of components
	pub(super) fn new(puppet: &mut Puppet) -> Self {
		let nodes = &puppet.nodes;
		let comps = &mut puppet.node_comps;

		let mut nodes_to_deform = HashSet::new();
		for param in &puppet.params {
			param.1.bindings.iter().for_each(|b| {
				if matches!(b.values, BindingValues::Deform(_)) {
					nodes_to_deform.insert(b.node);
				}
			});
		}

		let rest_abs = rest_absolute_transforms(nodes);
		let mut deformer_influenced = HashSet::new();
		let mut path_t_cache = HashMap::new();

		for node in nodes.iter() {
			let is_deformer = comps.get::<MeshGroup>(node.uuid).is_some()
				|| comps.get::<GridDeformer>(node.uuid).is_some()
				|| comps.get::<PathDeformer>(node.uuid).is_some();
			if !is_deformer {
				continue;
			}
			let descendants = collect_propagated_descendants(nodes, comps, node.uuid);
			for &cid in &descendants {
				deformer_influenced.insert(cid);
			}
			if let Some(path) = comps.get::<PathDeformer>(node.uuid) {
				if path.vertices.len() >= 2 {
					let path_inv = rest_abs.get(&node.uuid).copied().unwrap_or(Mat4::IDENTITY).inverse();
					if path_inv.is_finite() {
						for &cid in &descendants {
							if let Some((verts, origin)) = node_deform_target_verts(comps, cid) {
								let child_abs = rest_abs.get(&cid).copied().unwrap_or(Mat4::IDENTITY);
								let to_path = path_inv * child_abs;
								if to_path.is_finite() {
									let ts = verts
										.iter()
										.map(|&v| {
											let local = vec3(v.x - origin.x, v.y - origin.y, 0.0);
											let p_path = to_path.transform_point3(local).truncate();
											closest_point(path.curve_type, &path.vertices, p_path, 100)
										})
										.collect();
									path_t_cache.insert((node.uuid, cid), ts);
								}
							}
						}
					}
				}
			}
		}

		for node in nodes.iter() {
			let has_path_phys = comps
				.get::<PathDeformer>(node.uuid)
				.map(|p| {
					p.vertices.len() >= 2
						&& (p.physics_only
							|| p.dynamic
							|| comps
								.get::<crate::node::components::ExtraFields>(node.uuid)
								.map(|ef| ef.0.get("physics").is_some())
								.unwrap_or(false))
				})
				.unwrap_or(false);
			let needs_stack =
				nodes_to_deform.contains(&node.uuid) || deformer_influenced.contains(&node.uuid) || has_path_phys;
			if !needs_stack {
				continue;
			}
			if comps.get::<MeshGroup>(node.uuid).is_some() {
				if let Some(mesh) = comps.get::<Mesh>(node.uuid) {
					if !mesh.vertices.is_empty() {
						comps.set(node.uuid, DeformStack::new(mesh.vertices.len()));
					}
				}
			} else if let Some(grid) = comps.get::<GridDeformer>(node.uuid) {
				if grid.vertex_count() > 0 {
					comps.set(node.uuid, DeformStack::new(grid.vertex_count()));
				}
			} else if let Some(path) = comps.get::<PathDeformer>(node.uuid) {
				if path.vertex_count() > 0 {
					comps.set(node.uuid, DeformStack::new(path.vertex_count()));
				}
			}
		}

		let mut vertex_buffers = VertexBuffers::default();

		let mut root_drawables_count: usize = 0;
		for node in nodes.iter() {
			let drawable_kind = DrawableKind::new(node.uuid, comps, true);
			if let Some(drawable_kind) = drawable_kind {
				root_drawables_count += 1;

				match drawable_kind {
					DrawableKind::TexturedMesh(components) => {
						let (index_offset, vert_offset) = vertex_buffers.push(components.mesh);
						let (index_len, vert_len) = (components.mesh.indices.len(), components.mesh.vertices.len());

						comps.set(
							node.uuid,
							TexturedMeshRenderCtx {
								index_offset,
								vert_offset,
								index_len,
								vert_len,
							},
						);

						// TexturedMesh deformed by params or an ancestor deformer needs a DeformStack
						if nodes_to_deform.contains(&node.uuid) || deformer_influenced.contains(&node.uuid) {
							comps.set(node.uuid, DeformStack::new(vert_len));
						}
					}
					DrawableKind::Composite { .. } => {
						// exclude non-drawable children
						let children_list: Vec<InoxNodeUuid> = nodes
							.get_children(node.uuid)
							.filter_map(|n| {
								if DrawableKind::new(n.uuid, comps, false).is_some() {
									Some(n.uuid)
								} else {
									None
								}
							})
							.collect();

						// composite children are excluded from root_drawables_zsorted
						root_drawables_count -= children_list.len();

						comps.set(
							node.uuid,
							CompositeRenderCtx {
								// sort later, before render
								zsorted_children_list: children_list,
							},
						);
					}
				};
			}
		}

		let mut root_drawables_zsorted = Vec::new();
		// similarly, populate later, before render
		root_drawables_zsorted.resize(root_drawables_count, InoxNodeUuid(0));

		Self {
			vertex_buffers,
			root_drawables_zsorted,
			path_t_cache,
		}
	}

	/// Reset all `DeformStack`.
	pub(crate) fn reset(&mut self, nodes: &InoxNodeTree, comps: &mut World) {
		for node in nodes.iter() {
			if let Some(deform_stack) = comps.get_mut::<DeformStack>(node.uuid) {
				deform_stack.reset();
			}
		}
	}

	/// Update zsort-ordered info and deform buffer content inside self, according to updated puppet.
	pub(crate) fn update(&mut self, nodes: &InoxNodeTree, comps: &mut World) {
		// Propagate MeshGroup, GridDeformer and PathDeformer deformations top-down before combining Part deforms.
		for node in nodes.pre_order_iter().skip(1) {
			if comps.get::<MeshGroup>(node.uuid).is_some() {
				let Some(mg_mesh) = comps.get::<Mesh>(node.uuid).cloned() else {
					continue;
				};
				let Some(mg_deforms) = combine_node_deforms(nodes, comps, node.uuid, mg_mesh.vertices.len()) else {
					continue;
				};
				if !mg_deforms.iter().any(|d| d.length_squared() > 1e-8) {
					continue;
				}
				let mg_abs = comps.get::<TransformStore>(node.uuid).unwrap().absolute;
				let mg_inv = mg_abs.inverse();
				if !mg_inv.is_finite() {
					continue;
				}

				for cid in collect_propagated_descendants(nodes, comps, node.uuid) {
					if comps.get::<DeformStack>(cid).is_none() {
						continue;
					}
					let Some((verts, origin)) = node_deform_target_verts(comps, cid) else {
						continue;
					};
					let child_abs = comps.get::<TransformStore>(cid).unwrap().absolute;
					let to_mg = mg_inv * child_abs;
					let to_child = child_abs.inverse() * mg_abs;
					if !to_mg.is_finite() || !to_child.is_finite() {
						continue;
					}

					let child_deforms: Vec<Vec2> = verts
						.iter()
						.map(|&v| {
							let local = vec3(v.x - origin.x, v.y - origin.y, 0.0);
							let p_mg = to_mg.transform_point3(local).truncate() + mg_mesh.origin;
							let d_mg = sample_mesh_deform(&mg_mesh, &mg_deforms, p_mg);
							to_child.transform_vector3(d_mg.extend(0.0)).truncate()
						})
						.collect();

					comps
						.get_mut::<DeformStack>(cid)
						.unwrap()
						.push(DeformSource::Node(node.uuid), Deform::Direct(child_deforms));
				}
			} else if let Some(grid) = comps.get::<GridDeformer>(node.uuid).cloned() {
				if !grid.has_valid_grid() {
					continue;
				}
				let Some(grid_deforms) = combine_node_deforms(nodes, comps, node.uuid, grid.vertex_count()) else {
					continue;
				};
				if !grid_deforms.iter().any(|d| d.length_squared() > 1e-8) {
					continue;
				}
				let grid_abs = comps.get::<TransformStore>(node.uuid).unwrap().absolute;
				let grid_inv = grid_abs.inverse();
				if !grid_inv.is_finite() {
					continue;
				}

				for cid in collect_propagated_descendants(nodes, comps, node.uuid) {
					if comps.get::<DeformStack>(cid).is_none() {
						continue;
					}
					let Some((verts, origin)) = node_deform_target_verts(comps, cid) else {
						continue;
					};
					let child_abs = comps.get::<TransformStore>(cid).unwrap().absolute;
					let to_grid = grid_inv * child_abs;
					let to_child = child_abs.inverse() * grid_abs;
					if !to_grid.is_finite() || !to_child.is_finite() {
						continue;
					}
					let existing = if grid.dynamic {
						combine_node_deforms(nodes, comps, cid, verts.len())
					} else {
						None
					};

					let child_deforms: Vec<Vec2> = verts
						.iter()
						.enumerate()
						.map(|(i, &v)| {
							let extra = existing.as_ref().map_or(Vec2::ZERO, |e| e[i]);
							let local = vec3(v.x - origin.x + extra.x, v.y - origin.y + extra.y, 0.0);
							let p_grid = to_grid.transform_point3(local).truncate();
							let d_grid = grid.sample_deform(&grid_deforms, p_grid);
							to_child.transform_vector3(d_grid.extend(0.0)).truncate()
						})
						.collect();

					comps
						.get_mut::<DeformStack>(cid)
						.unwrap()
						.push(DeformSource::Node(node.uuid), Deform::Direct(child_deforms));
				}
			} else if let Some(path) = comps.get::<PathDeformer>(node.uuid).cloned() {
				if path.vertices.len() < 2 {
					continue;
				}
				let Some(path_deforms) = combine_node_deforms(nodes, comps, node.uuid, path.vertex_count()) else {
					continue;
				};
				if !path_deforms.iter().any(|d| d.length_squared() > 1e-8) {
					continue;
				}
				let deformed_pts: Vec<Vec2> = path
					.vertices
					.iter()
					.zip(&path_deforms)
					.map(|(&p, &d)| p + d)
					.collect();
				let path_abs = comps.get::<TransformStore>(node.uuid).unwrap().absolute;
				let path_inv = path_abs.inverse();
				if !path_inv.is_finite() {
					continue;
				}

				for cid in collect_propagated_descendants(nodes, comps, node.uuid) {
					let Some(t_samples) = self.path_t_cache.get(&(node.uuid, cid)) else {
						continue;
					};
					let Some((verts, origin)) = node_deform_target_verts(comps, cid) else {
						continue;
					};
					if t_samples.len() != verts.len() {
						continue;
					}
					let Some(existing) = combine_node_deforms(nodes, comps, cid, verts.len()) else {
						continue;
					};
					let child_abs = comps.get::<TransformStore>(cid).unwrap().absolute;
					let to_path = path_inv * child_abs;
					let to_child = child_abs.inverse() * path_abs;
					if !to_path.is_finite() || !to_child.is_finite() {
						continue;
					}

					let child_deforms: Vec<Vec2> = verts
						.iter()
						.enumerate()
						.map(|(i, &v)| {
							let local = vec3(v.x - origin.x + existing[i].x, v.y - origin.y + existing[i].y, 0.0);
							let c_vertex = to_path.transform_point3(local).truncate();
							let t = t_samples[i];

							let orig_pt = evaluate_point(path.curve_type, &path.vertices, t);
							let orig_tan_raw = evaluate_derivative(path.curve_type, &path.vertices, t);
							let orig_tan = if orig_tan_raw.length_squared() > 1e-16 {
								orig_tan_raw.normalize()
							} else {
								Vec2::X
							};
							let orig_nrm = vec2(-orig_tan.y, orig_tan.x);

							let def_pt = evaluate_point(path.curve_type, &deformed_pts, t);
							let def_tan_raw = evaluate_derivative(path.curve_type, &deformed_pts, t);
							let def_tan = if def_tan_raw.length_squared() > 1e-16 {
								def_tan_raw.normalize()
							} else {
								orig_tan
							};
							let def_nrm = vec2(-def_tan.y, def_tan.x);

							let diff = c_vertex - orig_pt;
							let n_dist = diff.dot(orig_nrm);
							let t_dist = diff.dot(orig_tan);
							let deformed_v = def_pt + def_nrm * n_dist + def_tan * t_dist;
							let offset_local = deformed_v - c_vertex;
							if offset_local.is_finite() {
								to_child.transform_vector3(offset_local.extend(0.0)).truncate()
							} else {
								Vec2::ZERO
							}
						})
						.collect();

					comps
						.get_mut::<DeformStack>(cid)
						.unwrap()
						.push(DeformSource::Node(node.uuid), Deform::Direct(child_deforms));
				}
			}
		}

		let mut root_drawable_uuid_zsort_vec = Vec::<(InoxNodeUuid, f32)>::new();

		// root is definitely not a drawable.
		for node in nodes.iter().skip(1) {
			if let Some(drawable_kind) = DrawableKind::new(node.uuid, comps, false) {
				let parent = nodes.get_parent(node.uuid);
				let node_zsort = comps.get::<ZSort>(node.uuid).unwrap().0;

				if !matches!(
					DrawableKind::new(parent.uuid, comps, false),
					Some(DrawableKind::Composite(_))
				) {
					// exclude composite children
					root_drawable_uuid_zsort_vec.push((node.uuid, node_zsort));
				}

				match drawable_kind {
					// for Composite, update zsorted children list
					DrawableKind::Composite { .. } => {
						// `swap()` usage is a trick that both:
						// - returns mut borrowed comps early
						// - does not involve any heap allocations
						let mut zsorted_children_list = Vec::new();
						swap(
							&mut zsorted_children_list,
							&mut comps
								.get_mut::<CompositeRenderCtx>(node.uuid)
								.unwrap()
								.zsorted_children_list,
						);

						zsorted_children_list.sort_by(|a, b| {
							let zsort_a = comps.get::<ZSort>(*a).unwrap();
							let zsort_b = comps.get::<ZSort>(*b).unwrap();
							zsort_a.total_cmp(zsort_b).reverse()
						});

						swap(
							&mut zsorted_children_list,
							&mut comps
								.get_mut::<CompositeRenderCtx>(node.uuid)
								.unwrap()
								.zsorted_children_list,
						);
					}
					// for TexturedMesh, obtain and write deforms into vertex_buffer
					DrawableKind::TexturedMesh(..) => {
						// A TexturedMesh not having an associated DeformStack means it will not be deformed at all, skip.
						if let Some(deform_stack) = comps.get::<DeformStack>(node.uuid) {
							let render_ctx = comps.get::<TexturedMeshRenderCtx>(node.uuid).unwrap();
							let vert_offset = render_ctx.vert_offset as usize;
							let vert_len = render_ctx.vert_len;
							deform_stack.combine(
								nodes,
								comps,
								&mut self.vertex_buffers.deforms[vert_offset..(vert_offset + vert_len)],
							);
						}
					}
				}
			}
		}

		root_drawable_uuid_zsort_vec.sort_by(|a, b| a.1.total_cmp(&b.1).reverse());
		self.root_drawables_zsorted
			.iter_mut()
			.zip(root_drawable_uuid_zsort_vec.iter())
			.for_each(|(old, new)| *old = new.0);
	}
}

fn sample_mesh_deform(mesh: &Mesh, deforms: &[Vec2], p: Vec2) -> Vec2 {
	let mut best_dist = f32::INFINITY;
	let mut best_deform = Vec2::ZERO;

	for tri in mesh.indices.chunks_exact(3) {
		let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
		if i0 >= mesh.vertices.len() || i1 >= mesh.vertices.len() || i2 >= mesh.vertices.len() {
			continue;
		}
		let a = mesh.vertices[i0];
		let b = mesh.vertices[i1];
		let c = mesh.vertices[i2];

		let v0 = b - a;
		let v1 = c - a;
		let v2 = p - a;
		let denom = v0.x * v1.y - v1.x * v0.y;
		if denom.abs() < 1e-6 {
			continue;
		}
		let v = (v2.x * v1.y - v1.x * v2.y) / denom;
		let w = (v0.x * v2.y - v2.x * v0.y) / denom;
		let u = 1.0 - v - w;

		if u >= -1e-3 && v >= -1e-3 && w >= -1e-3 {
			return deforms[i0] * u + deforms[i1] * v + deforms[i2] * w;
		}

		let centroid = (a + b + c) / 3.0;
		let dist = (p - centroid).length_squared();
		if dist < best_dist {
			best_dist = dist;
			let uc = u.clamp(0.0, 1.0);
			let vc = v.clamp(0.0, 1.0);
			let wc = w.clamp(0.0, 1.0);
			let sum = (uc + vc + wc).max(1e-6);
			best_deform = (deforms[i0] * uc + deforms[i1] * vc + deforms[i2] * wc) / sum;
		}
	}

	let _ = vec2(0.0, 0.0);
	best_deform
}

/// Same as the reference Inochi2D implementation, Inox2D also aims for a "bring your own rendering backend" design.
/// A custom backend shall implement this trait.
///
/// It is perfectly fine that the trait implementation does not contain everything needed to display a puppet as:
/// - The renderer may not be directly rendering to the screen for flexibility.
/// - The renderer may want platform-specific optimizations, e.g. batching, and the provided implementation is merely for collecting puppet info.
/// - The renderer may be a debug/just-for-fun renderer intercepting draw calls for other purposes.
///
/// Either way, the point is Inox2D will implement a `draw()` method for any `impl InoxRenderer`, dispatching calls based on puppet structure according to Inochi2D standard.
pub trait InoxRenderer {
	/// Begin masking.
	///
	/// Ref impl: Clear and start writing to the stencil buffer, lock the color buffer.
	fn on_begin_masks(&self, masks: &Masks);
	/// Get prepared for rendering a singular Mask.
	fn on_begin_mask(&self, mask: &Mask);
	/// Get prepared for rendering masked content.
	///
	/// Ref impl: Read only from the stencil buffer, unlock the color buffer.
	fn on_begin_masked_content(&self);
	/// End masking.
	///
	/// Ref impl: Disable the stencil buffer.
	fn on_end_mask(&self);

	/// Draw TexturedMesh content.
	// TODO: TexturedMesh without any texture (usually for mesh masks)?
	fn draw_textured_mesh_content(
		&self,
		as_mask: bool,
		components: &TexturedMeshComponents,
		render_ctx: &TexturedMeshRenderCtx,
		id: InoxNodeUuid,
	);

	/// Begin compositing. Get prepared for rendering children of a Composite.
	///
	/// Ref impl: Prepare composite buffers.
	fn begin_composite_content(
		&self,
		as_mask: bool,
		components: &CompositeComponents,
		render_ctx: &CompositeRenderCtx,
		id: InoxNodeUuid,
	);
	/// End compositing.
	///
	/// Ref impl: Transfer content from composite buffers to normal buffers.
	fn finish_composite_content(
		&self,
		as_mask: bool,
		components: &CompositeComponents,
		render_ctx: &CompositeRenderCtx,
		id: InoxNodeUuid,
	);
}

pub trait InoxRendererExt {
	/// Draw a Drawable, which is potentially masked.
	fn draw_drawable(&self, as_mask: bool, comps: &World, id: InoxNodeUuid);

	/// Draw one composite. `components` must be referencing `comps`.
	fn draw_composite(&self, as_mask: bool, comps: &World, components: &CompositeComponents, id: InoxNodeUuid);

	/// Iterate over top-level drawables (excluding masks) in zsort order,
	/// and make draw calls correspondingly.
	///
	/// This effectively draws the complete puppet.
	fn draw(&self, puppet: &Puppet);
}

impl<T: InoxRenderer> InoxRendererExt for T {
	fn draw_drawable(&self, as_mask: bool, comps: &World, id: InoxNodeUuid) {
		let drawable_kind = DrawableKind::new(id, comps, false).expect("Node must be a Drawable.");
		let masks = match drawable_kind {
			DrawableKind::TexturedMesh(ref components) => &components.drawable.masks,
			DrawableKind::Composite(ref components) => &components.drawable.masks,
		};

		let mut has_masks = false;
		if let Some(ref masks) = masks {
			has_masks = true;
			self.on_begin_masks(masks);
			for mask in &masks.masks {
				self.on_begin_mask(mask);

				self.draw_drawable(true, comps, mask.source);
			}
			self.on_begin_masked_content();
		}

		match drawable_kind {
			DrawableKind::TexturedMesh(ref components) => {
				self.draw_textured_mesh_content(as_mask, components, comps.get(id).unwrap(), id)
			}
			DrawableKind::Composite(ref components) => self.draw_composite(as_mask, comps, components, id),
		}

		if has_masks {
			self.on_end_mask();
		}
	}

	fn draw_composite(&self, as_mask: bool, comps: &World, components: &CompositeComponents, id: InoxNodeUuid) {
		let render_ctx = comps.get::<CompositeRenderCtx>(id).unwrap();
		if render_ctx.zsorted_children_list.is_empty() {
			// Optimization: Nothing to be drawn, skip context switching
			return;
		}

		self.begin_composite_content(as_mask, components, render_ctx, id);

		for uuid in &render_ctx.zsorted_children_list {
			let drawable_kind = DrawableKind::new(*uuid, comps, false)
				.expect("All children in zsorted_children_list should be a Drawable.");
			match drawable_kind {
				DrawableKind::TexturedMesh(components) => {
					self.draw_textured_mesh_content(as_mask, &components, comps.get(*uuid).unwrap(), *uuid)
				}
				DrawableKind::Composite { .. } => panic!("Composite inside Composite not allowed."),
			}
		}

		self.finish_composite_content(as_mask, components, render_ctx, id);
	}

	/// Dispatches draw calls for all nodes of `puppet`
	/// - with provided renderer implementation,
	/// - in Inochi2D standard defined order.
	///
	/// This does not guarantee the display of a puppet on screen due to these possible reasons:
	/// - Only provided `InoxRenderer` method implementations are called.
	/// 
	/// For example, maybe the caller still need to transfer content from a texture buffer to the screen surface buffer.
	/// - The provided `InoxRender` implementation is wrong.
	/// - `puppet` here does not belong to the `model` this `renderer` is initialized with. This will likely result in panics for non-existent node uuids.
	fn draw(&self, puppet: &Puppet) {
		for uuid in &puppet
			.render_ctx
			.as_ref()
			.expect("RenderCtx of puppet must be initialized before calling draw().")
			.root_drawables_zsorted
		{
			self.draw_drawable(false, &puppet.node_comps, *uuid);
		}
	}
}

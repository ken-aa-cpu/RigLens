use std::path::Path;
use std::process::Command;

use glam::{vec2, vec3};
use image::{Rgba, RgbaImage};
use inox2d::model::Model;
use inox2d::node::components::{BlendMode, Blending, Drawable, Mask, MaskMode, Masks, TexturedMesh};
use inox2d::node::{InoxNode, InoxNodeUuid};
use inox2d::puppet::Puppet;
use inox2d::texture::{decode_model_textures, TextureId};
use json::JsonValue;

use crate::rig::{
	auto_rig_vtuber, encode_rgba_png, generate_contour_mesh, generate_delaunay_mesh, generate_grid_mesh,
};

pub const DEFAULT_PHOTOCRAFT_CLI: &str = r"D:\app\PhotoCraft\photocraft-cli.exe";

#[derive(Clone, Debug)]
pub enum PsdSectionType {
	Normal,
	OpenFolder,
	ClosedFolder,
	BoundingDivider,
}

#[derive(Clone, Debug)]
pub struct PsdLayer {
	pub name: String,
	pub left: i32,
	pub top: i32,
	pub right: i32,
	pub bottom: i32,
	pub opacity: u8,
	pub blend_mode: BlendMode,
	pub clipping: bool,
	pub visible: bool,
	pub section: PsdSectionType,
	pub image: RgbaImage,
}

#[derive(Clone, Debug)]
pub struct PsdDocument {
	pub width: u32,
	pub height: u32,
	pub layers: Vec<PsdLayer>,
}

pub struct PsdExportLayer {
	pub name: String,
	pub left: i32,
	pub top: i32,
	pub image: RgbaImage,
	pub opacity: u8,
	pub blend_mode: BlendMode,
	pub clipping: bool,
	pub visible: bool,
}

struct CursorReader<'a> {
	data: &'a [u8],
	pos: usize,
}

impl<'a> CursorReader<'a> {
	fn new(data: &'a [u8]) -> Self {
		Self { data, pos: 0 }
	}

	fn remaining(&self) -> usize {
		self.data.len().saturating_sub(self.pos)
	}

	fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
		if self.pos + n > self.data.len() {
			return Err(format!("Unexpected EOF at offset {} (needed {n} bytes)", self.pos));
		}
		let slice = &self.data[self.pos..self.pos + n];
		self.pos += n;
		Ok(slice)
	}

	fn skip(&mut self, n: usize) -> Result<(), String> {
		if self.pos + n > self.data.len() {
			return Err(format!("Unexpected EOF skipping {n} bytes at {}", self.pos));
		}
		self.pos += n;
		Ok(())
	}

	fn read_u8(&mut self) -> Result<u8, String> {
		Ok(self.read_bytes(1)?[0])
	}

	fn read_u16(&mut self) -> Result<u16, String> {
		let b = self.read_bytes(2)?;
		Ok(u16::from_be_bytes([b[0], b[1]]))
	}

	fn read_i16(&mut self) -> Result<i16, String> {
		let b = self.read_bytes(2)?;
		Ok(i16::from_be_bytes([b[0], b[1]]))
	}

	fn read_u32(&mut self) -> Result<u32, String> {
		let b = self.read_bytes(4)?;
		Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
	}

	fn read_i32(&mut self) -> Result<i32, String> {
		let b = self.read_bytes(4)?;
		Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
	}
}

fn parse_blend_key(key: &[u8]) -> BlendMode {
	match key {
		b"mul " => BlendMode::Multiply,
		b"scrn" => BlendMode::Screen,
		b"lddg" => BlendMode::LinearDodge,
		b"div " => BlendMode::ColorDodge,
		_ => BlendMode::Normal,
	}
}

fn blend_mode_key(mode: BlendMode) -> [u8; 4] {
	match mode {
		BlendMode::Multiply => *b"mul ",
		BlendMode::Screen => *b"scrn",
		BlendMode::LinearDodge => *b"lddg",
		BlendMode::ColorDodge => *b"div ",
		_ => *b"norm",
	}
}

fn decode_packbits_scanline(src: &[u8], expected_len: usize) -> Result<Vec<u8>, String> {
	let mut out = Vec::with_capacity(expected_len);
	let mut i = 0usize;
	while i < src.len() && out.len() < expected_len {
		let n = src[i] as i8;
		i += 1;
		if n >= 0 {
			let count = (n as usize) + 1;
			if i + count > src.len() {
				return Err("PackBits literal run out of bounds".to_owned());
			}
			let take = count.min(expected_len - out.len());
			out.extend_from_slice(&src[i..i + take]);
			i += count;
		} else if n != -128 {
			let count = (1 - n as i32) as usize;
			if i >= src.len() {
				return Err("PackBits repeat run out of bounds".to_owned());
			}
			let val = src[i];
			i += 1;
			let take = count.min(expected_len - out.len());
			out.resize(out.len() + take, val);
		}
	}
	if out.len() < expected_len {
		out.resize(expected_len, 0);
	}
	Ok(out)
}

fn decode_channel_pixels(raw: &[u8], w: usize, h: usize) -> Result<Vec<u8>, String> {
	if w == 0 || h == 0 {
		return Ok(Vec::new());
	}
	if raw.len() < 2 {
		return Err("Channel data too short".to_owned());
	}
	let compression = u16::from_be_bytes([raw[0], raw[1]]);
	let body = &raw[2..];
	match compression {
		0 => {
			let expected = w * h;
			if body.len() < expected {
				return Err(format!("Raw channel truncated: got {}, expected {expected}", body.len()));
			}
			Ok(body[..expected].to_vec())
		}
		1 => {
			let counts_bytes = h * 2;
			if body.len() < counts_bytes {
				return Err("RLE scanline counts truncated".to_owned());
			}
			let mut out = Vec::with_capacity(w * h);
			let mut offset = counts_bytes;
			for row in 0..h {
				let c = u16::from_be_bytes([body[row * 2], body[row * 2 + 1]]) as usize;
				if offset + c > body.len() {
					return Err("RLE scanline payload truncated".to_owned());
				}
				let decoded = decode_packbits_scanline(&body[offset..offset + c], w)?;
				out.extend_from_slice(&decoded);
				offset += c;
			}
			Ok(out)
		}
		other => Err(format!("Unsupported PSD channel compression {other} (expected 0 Raw or 1 RLE)")),
	}
}

/// Parse an 8-bit RGB/RGBA Adobe `.psd` file into layers.
pub fn parse_psd_bytes(data: &[u8]) -> Result<PsdDocument, String> {
	let mut r = CursorReader::new(data);
	let sig = r.read_bytes(4)?;
	if sig != b"8BPS" {
		return Err("Not a PSD file (missing 8BPS header)".to_owned());
	}
	let version = r.read_u16()?;
	if version != 1 {
		return Err(format!("Unsupported PSD version {version} (PSB v2 not supported natively; convert via PhotoCraft)"));
	}
	r.skip(6)?; // reserved
	let _channels = r.read_u16()?;
	let height = r.read_u32()?;
	let width = r.read_u32()?;
	let depth = r.read_u16()?;
	if depth != 8 {
		return Err(format!("Unsupported PSD bit depth {depth} (expected 8-bit; convert via PhotoCraft)"));
	}
	let color_mode = r.read_u16()?;
	if color_mode != 3 {
		return Err(format!("Unsupported PSD color mode {color_mode} (expected RGB=3; convert via PhotoCraft)"));
	}

	// Color mode data
	let color_len = r.read_u32()? as usize;
	r.skip(color_len)?;

	// Image resources
	let res_len = r.read_u32()? as usize;
	r.skip(res_len)?;

	// Layer and mask information
	let layer_mask_len = r.read_u32()? as usize;
	if layer_mask_len == 0 {
		return Ok(PsdDocument {
			width,
			height,
			layers: Vec::new(),
		});
	}
	let layer_info_len = r.read_u32()? as usize;
	if layer_info_len == 0 {
		return Ok(PsdDocument {
			width,
			height,
			layers: Vec::new(),
		});
	}

	let raw_count = r.read_i16()?;
	let layer_count = raw_count.unsigned_abs() as usize;

	struct RawLayerMeta {
		name: String,
		left: i32,
		top: i32,
		right: i32,
		bottom: i32,
		channels: Vec<(i16, usize)>,
		opacity: u8,
		blend_mode: BlendMode,
		clipping: bool,
		visible: bool,
		section: PsdSectionType,
	}

	let mut metas = Vec::with_capacity(layer_count);
	for _ in 0..layer_count {
		let top = r.read_i32()?;
		let left = r.read_i32()?;
		let bottom = r.read_i32()?;
		let right = r.read_i32()?;
		let num_ch = r.read_u16()? as usize;
		let mut channels = Vec::with_capacity(num_ch);
		for _ in 0..num_ch {
			let ch_id = r.read_i16()?;
			let ch_len = r.read_u32()? as usize;
			channels.push((ch_id, ch_len));
		}

		let bsig = r.read_bytes(4)?;
		if bsig != b"8BIM" {
			return Err("Invalid layer blend mode signature".to_owned());
		}
		let bkey = r.read_bytes(4)?;
		let blend_mode = parse_blend_key(bkey);
		let opacity = r.read_u8()?;
		let clipping = r.read_u8()? != 0;
		let flags = r.read_u8()?;
		let visible = (flags & 0x02) == 0;
		let _filler = r.read_u8()?;

		let extra_len = r.read_u32()? as usize;
		let extra_bytes = r.read_bytes(extra_len)?;
		let mut er = CursorReader::new(extra_bytes);

		let mask_data_len = er.read_u32()? as usize;
		er.skip(mask_data_len)?;
		let range_len = er.read_u32()? as usize;
		er.skip(range_len)?;

		// Pascal layer name padded to 4-byte multiple
		let name_len = er.read_u8()? as usize;
		let pascal_bytes = er.read_bytes(name_len)?;
		let mut name = String::from_utf8_lossy(pascal_bytes).into_owned();
		let total_pascal = 1 + name_len;
		let pad = (4 - (total_pascal % 4)) % 4;
		if er.remaining() >= pad {
			er.skip(pad)?;
		}

		let mut section = PsdSectionType::Normal;
		while er.remaining() >= 12 {
			let sig = er.read_bytes(4)?;
			if sig != b"8BIM" && sig != b"8B64" {
				break;
			}
			let key = er.read_bytes(4)?;
			let block_len = er.read_u32()? as usize;
			let padded_len = (block_len + 1) & !1;
			let actual_len = block_len.min(er.remaining());
			let block = er.read_bytes(actual_len)?;
			if padded_len > actual_len && er.remaining() >= (padded_len - actual_len) {
				er.skip(padded_len - actual_len)?;
			}

			if key == b"luni" && block.len() >= 4 {
				let char_count = u32::from_be_bytes([block[0], block[1], block[2], block[3]]) as usize;
				if block.len() >= 4 + char_count * 2 {
					let utf16: Vec<u16> = block[4..4 + char_count * 2]
						.chunks_exact(2)
						.map(|c| u16::from_be_bytes([c[0], c[1]]))
						.collect();
					if let Ok(decoded) = String::from_utf16(&utf16) {
						let trimmed = decoded.trim_end_matches('\0');
						if !trimmed.is_empty() {
							name = trimmed.to_owned();
						}
					}
				}
			} else if (key == b"lsct" || key == b"lsdk") && block.len() >= 4 {
				let code = u32::from_be_bytes([block[0], block[1], block[2], block[3]]);
				section = match code {
					1 => PsdSectionType::OpenFolder,
					2 => PsdSectionType::ClosedFolder,
					3 => PsdSectionType::BoundingDivider,
					_ => PsdSectionType::Normal,
				};
			}
		}

		metas.push(RawLayerMeta {
			name,
			left,
			top,
			right,
			bottom,
			channels,
			opacity,
			blend_mode,
			clipping,
			visible,
			section,
		});
	}

	let mut layers = Vec::with_capacity(metas.len());
	for meta in metas {
		let w = (meta.right - meta.left).max(0) as usize;
		let h = (meta.bottom - meta.top).max(0) as usize;
		let mut ch_r = None;
		let mut ch_g = None;
		let mut ch_b = None;
		let mut ch_a = None;

		for (ch_id, ch_len) in meta.channels {
			let raw = r.read_bytes(ch_len)?;
			if w == 0 || h == 0 {
				continue;
			}
			match ch_id {
				0 => ch_r = Some(decode_channel_pixels(raw, w, h)?),
				1 => ch_g = Some(decode_channel_pixels(raw, w, h)?),
				2 => ch_b = Some(decode_channel_pixels(raw, w, h)?),
				-1 => ch_a = Some(decode_channel_pixels(raw, w, h)?),
				_ => {}
			}
		}

		let mut image = RgbaImage::new(w as u32, h as u32);
		if w > 0 && h > 0 {
			for y in 0..h {
				for x in 0..w {
					let idx = y * w + x;
					let rv = ch_r.as_ref().and_then(|v| v.get(idx).copied()).unwrap_or(0);
					let gv = ch_g.as_ref().and_then(|v| v.get(idx).copied()).unwrap_or(rv);
					let bv = ch_b.as_ref().and_then(|v| v.get(idx).copied()).unwrap_or(rv);
					let av = ch_a.as_ref().and_then(|v| v.get(idx).copied()).unwrap_or(255);
					image.put_pixel(x as u32, y as u32, Rgba([rv, gv, bv, av]));
				}
			}
		}

		layers.push(PsdLayer {
			name: meta.name,
			left: meta.left,
			top: meta.top,
			right: meta.right,
			bottom: meta.bottom,
			opacity: meta.opacity,
			blend_mode: meta.blend_mode,
			clipping: meta.clipping,
			visible: meta.visible,
			section: meta.section,
			image,
		});
	}

	Ok(PsdDocument {
		width,
		height,
		layers,
	})
}

/// Write a standard 8-bit RGBA layered `.psd` file.
pub fn write_layered_psd(width: u32, height: u32, layers: &[PsdExportLayer]) -> Result<Vec<u8>, String> {
	let mut out = Vec::new();
	// Header (26 bytes)
	out.extend_from_slice(b"8BPS");
	out.extend_from_slice(&1u16.to_be_bytes());
	out.extend_from_slice(&[0u8; 6]);
	out.extend_from_slice(&4u16.to_be_bytes()); // RGBA
	out.extend_from_slice(&height.to_be_bytes());
	out.extend_from_slice(&width.to_be_bytes());
	out.extend_from_slice(&8u16.to_be_bytes());
	out.extend_from_slice(&3u16.to_be_bytes()); // RGB mode

	// Color Mode Data + Image Resources (empty)
	out.extend_from_slice(&0u32.to_be_bytes());
	out.extend_from_slice(&0u32.to_be_bytes());

	let mut layer_records = Vec::new();
	let mut channel_blobs = Vec::new();

	layer_records.extend_from_slice(&(layers.len() as i16).to_be_bytes());

	for layer in layers {
		let w = layer.image.width() as i32;
		let h = layer.image.height() as i32;
		let right = layer.left + w;
		let bottom = layer.top + h;
		let px_count = (w * h) as usize;
		let ch_len = (2 + px_count) as u32;

		layer_records.extend_from_slice(&layer.top.to_be_bytes());
		layer_records.extend_from_slice(&layer.left.to_be_bytes());
		layer_records.extend_from_slice(&bottom.to_be_bytes());
		layer_records.extend_from_slice(&right.to_be_bytes());
		layer_records.extend_from_slice(&4u16.to_be_bytes());

		for ch_id in [-1i16, 0i16, 1i16, 2i16] {
			layer_records.extend_from_slice(&ch_id.to_be_bytes());
			layer_records.extend_from_slice(&ch_len.to_be_bytes());
		}

		layer_records.extend_from_slice(b"8BIM");
		layer_records.extend_from_slice(&blend_mode_key(layer.blend_mode));
		layer_records.push(layer.opacity);
		layer_records.push(if layer.clipping { 1 } else { 0 });
		layer_records.push(if layer.visible { 0 } else { 0x02 });
		layer_records.push(0); // filler

		let mut extra = Vec::new();
		extra.extend_from_slice(&0u32.to_be_bytes()); // mask data len
		extra.extend_from_slice(&0u32.to_be_bytes()); // blending ranges len

		// Pascal name (ASCII fallback, padded to 4 bytes)
		let ascii_name: Vec<u8> = layer
			.name
			.bytes()
			.map(|b| if b.is_ascii() { b } else { b'_' })
			.take(255)
			.collect();
		extra.push(ascii_name.len() as u8);
		extra.extend_from_slice(&ascii_name);
		let pad = (4 - ((1 + ascii_name.len()) % 4)) % 4;
		extra.resize(extra.len() + pad, 0);

		// luni Unicode name block
		let utf16: Vec<u16> = layer.name.encode_utf16().collect();
		let mut luni_data = Vec::with_capacity(4 + utf16.len() * 2);
		luni_data.extend_from_slice(&(utf16.len() as u32).to_be_bytes());
		for u in utf16 {
			luni_data.extend_from_slice(&u.to_be_bytes());
		}
		if luni_data.len() % 2 != 0 {
			luni_data.push(0);
		}
		extra.extend_from_slice(b"8BIMluni");
		extra.extend_from_slice(&(luni_data.len() as u32).to_be_bytes());
		extra.extend_from_slice(&luni_data);

		layer_records.extend_from_slice(&(extra.len() as u32).to_be_bytes());
		layer_records.extend_from_slice(&extra);

		// Planar channel pixels in order: -1 (A), 0 (R), 1 (G), 2 (B)
		let raw_px = layer.image.as_raw();
		for offset in [3usize, 0usize, 1usize, 2usize] {
			channel_blobs.extend_from_slice(&0u16.to_be_bytes()); // Raw
			for chunk in raw_px.chunks_exact(4) {
				channel_blobs.push(chunk[offset]);
			}
		}
	}

	let mut layer_info = Vec::new();
	layer_info.extend_from_slice(&layer_records);
	layer_info.extend_from_slice(&channel_blobs);
	if layer_info.len() % 2 != 0 {
		layer_info.push(0);
	}

	let layer_mask_len = 4 + layer_info.len();
	out.extend_from_slice(&(layer_mask_len as u32).to_be_bytes());
	out.extend_from_slice(&(layer_info.len() as u32).to_be_bytes());
	out.extend_from_slice(&layer_info);

	// Merged composite image (Raw compression, planar R, G, B, A)
	out.extend_from_slice(&0u16.to_be_bytes());
	let canvas_px = (width * height) as usize;
	let mut merged = vec![0u8; canvas_px * 4];
	for layer in layers {
		if !layer.visible {
			continue;
		}
		let lw = layer.image.width() as i32;
		let lh = layer.image.height() as i32;
		let op = layer.opacity as f32 / 255.0;
		for ly in 0..lh {
			let cy = layer.top + ly;
			if cy < 0 || cy >= height as i32 {
				continue;
			}
			for lx in 0..lw {
				let cx = layer.left + lx;
				if cx < 0 || cx >= width as i32 {
					continue;
				}
				let sp = layer.image.get_pixel(lx as u32, ly as u32).0;
				let sa = (sp[3] as f32 / 255.0) * op;
				if sa <= 0.001 {
					continue;
				}
				let didx = ((cy as u32 * width + cx as u32) * 4) as usize;
				for c in 0..3 {
					let d = merged[didx + c] as f32;
					merged[didx + c] = (sp[c] as f32 * sa + d * (1.0 - sa)).clamp(0.0, 255.0) as u8;
				}
				let da = merged[didx + 3] as f32 / 255.0;
				merged[didx + 3] = ((sa + da * (1.0 - sa)) * 255.0).clamp(0.0, 255.0) as u8;
			}
		}
	}
	for ch in 0..4 {
		for px in merged.chunks_exact(4) {
			out.push(px[ch]);
		}
	}

	Ok(out)
}

fn trim_layer_alpha(layer: &PsdLayer) -> Option<(RgbaImage, i32, i32)> {
	let w = layer.image.width();
	let h = layer.image.height();
	if w == 0 || h == 0 {
		return None;
	}
	let mut min_x = w;
	let mut min_y = h;
	let mut max_x = 0u32;
	let mut max_y = 0u32;
	let mut any = false;

	for y in 0..h {
		for x in 0..w {
			if layer.image.get_pixel(x, y).0[3] > 2 {
				any = true;
				min_x = min_x.min(x);
				min_y = min_y.min(y);
				max_x = max_x.max(x);
				max_y = max_y.max(y);
			}
		}
	}
	if !any {
		return None;
	}

	let pad = 2u32;
	let x0 = min_x.saturating_sub(pad);
	let y0 = min_y.saturating_sub(pad);
	let x1 = (max_x + 1 + pad).min(w);
	let y1 = (max_y + 1 + pad).min(h);
	let tw = x1 - x0;
	let th = y1 - y0;

	let mut cropped = RgbaImage::new(tw, th);
	for y in 0..th {
		for x in 0..tw {
			cropped.put_pixel(x, y, *layer.image.get_pixel(x0 + x, y0 + y));
		}
	}
	Some((cropped, layer.left + x0 as i32, layer.top + y0 as i32))
}

/// Convert a parsed `PsdDocument` into a rigged/structured Inochi2D `Model`
/// using the chosen meshing strategy (`"delaunay"`, `"contour"`, or `"grid"`).
pub fn import_psd_to_model_with_mode(
	psd: &PsdDocument,
	name: &str,
	mesh_mode: &str,
	auto_rig: bool,
	grid_cols: usize,
	grid_rows: usize,
) -> Model {
	let mut model = Model {
		puppet: Puppet::new_empty(name),
		textures: Vec::new(),
		vendors: Vec::new(),
	};

	let root_id = model.puppet.nodes.root_node_id;
	let mut group_stack: Vec<InoxNodeUuid> = vec![root_id];
	let mut last_base_part: Option<InoxNodeUuid> = None;
	let canvas_cx = psd.width as f32 * 0.5;
	let canvas_cy = psd.height as f32 * 0.5;

	for (idx, layer) in psd.layers.iter().enumerate() {
		match layer.section {
			PsdSectionType::BoundingDivider => {
				let parent = *group_stack.last().unwrap_or(&root_id);
				let gid = model.puppet.next_node_uuid();
				model.puppet.add_node(
					parent,
					InoxNode {
						uuid: gid,
						name: format!("Group_{}", gid.0),
						enabled: true,
						zsort: 0.0,
						trans_offset: Default::default(),
						lock_to_root: false,
					},
				);
				group_stack.push(gid);
				continue;
			}
			PsdSectionType::OpenFolder | PsdSectionType::ClosedFolder => {
				if group_stack.len() > 1 {
					let gid = group_stack.pop().unwrap();
					if let Some(node) = model.puppet.nodes.get_node_mut(gid) {
						node.name = layer.name.clone();
						node.enabled = layer.visible;
					}
				}
				continue;
			}
			PsdSectionType::Normal => {}
		}

		let Some((trimmed, abs_left, abs_top)) = trim_layer_alpha(layer) else {
			continue;
		};

		let tw = trimmed.width() as f32;
		let th = trimmed.height() as f32;
		let center_x = abs_left as f32 + tw * 0.5 - canvas_cx;
		let center_y = abs_top as f32 + th * 0.5 - canvas_cy;

		let tex_idx = model.textures.len();
		model.textures.push(encode_rgba_png(&trimmed));

		let parent = *group_stack.last().unwrap_or(&root_id);
		let id = model.puppet.next_node_uuid();
		let zsort = -(idx as f32) * 2.0;

		model.puppet.add_node(
			parent,
			InoxNode {
				uuid: id,
				name: layer.name.clone(),
				enabled: layer.visible,
				zsort,
				trans_offset: inox2d::math::transform::TransformOffset {
					translation: vec3(center_x, center_y, 0.0),
					..Default::default()
				},
				lock_to_root: false,
			},
		);

		let masks = if layer.clipping {
			last_base_part.map(|src| Masks {
				threshold: 0.1,
				masks: vec![Mask {
					source: src,
					mode: MaskMode::Mask,
				}],
			})
		} else {
			last_base_part = Some(id);
			None
		};

		model.puppet.node_comps.add(
			id,
			Drawable {
				blending: Blending {
					mode: layer.blend_mode,
					tint: vec3(1.0, 1.0, 1.0),
					screen_tint: vec3(0.0, 0.0, 0.0),
					opacity: layer.opacity as f32 / 255.0,
				},
				masks,
			},
		);
		model.puppet.node_comps.add(
			id,
			TexturedMesh {
				tex_albedo: TextureId(tex_idx),
				tex_emissive: TextureId(0),
				tex_bumpmap: TextureId(0),
			},
		);

		let mesh = match mesh_mode {
			"delaunay" => {
				let target_div = grid_cols.max(grid_rows).max(4) as f32;
				let step_px = (tw.max(th) / target_div).clamp(12.0, 44.0);
				generate_delaunay_mesh(&trimmed, tw, th, step_px, 4)
			}
			"contour" => generate_contour_mesh(&trimmed, tw, th, grid_cols.max(4), grid_rows.max(4), 4),
			_ => generate_grid_mesh(tw, th, grid_cols.max(2), grid_rows.max(2), vec2(0.0, 0.0)),
		};
		model.puppet.node_comps.add(id, mesh);
	}

	if auto_rig {
		auto_rig_vtuber(&mut model.puppet);
	}

	model
}

/// Convert a parsed `PsdDocument` into a rigged/structured Inochi2D `Model`.
pub fn import_psd_to_model(
	psd: &PsdDocument,
	name: &str,
	contour_mesh: bool,
	auto_rig: bool,
	grid_cols: usize,
	grid_rows: usize,
) -> Model {
	let mode = if contour_mesh { "contour" } else { "grid" };
	import_psd_to_model_with_mode(psd, name, mode, auto_rig, grid_cols, grid_rows)
}

/// Export all Part layers of a `Model` into a multi-layer `.psd` byte stream.
pub fn export_model_to_psd(model: &mut Model, width: u32, height: u32) -> Result<Vec<u8>, String> {
	model.puppet.init_all();
	model.puppet.begin_frame();
	model.puppet.end_frame(0.0);

	let decoded = decode_model_textures(model.textures.iter());
	let cx = width as f32 * 0.5;
	let cy = height as f32 * 0.5;

	let mut part_ids: Vec<(InoxNodeUuid, f32)> = model
		.puppet
		.nodes
		.iter()
		.filter_map(|n| {
			if model.puppet.node_comps.get::<TexturedMesh>(n.uuid).is_some() {
				let z = model.puppet.node_zsort(n.uuid).unwrap_or(n.zsort);
				Some((n.uuid, z))
			} else {
				None
			}
		})
		.collect();
	// Bottom-to-top in PSD means highest zsort first
	part_ids.sort_by(|a, b| b.1.total_cmp(&a.1));

	let mut export_layers = Vec::with_capacity(part_ids.len());
	for (id, _) in part_ids {
		let node = model.puppet.nodes.get_node(id).unwrap();
		let tm = model.puppet.node_comps.get::<TexturedMesh>(id).unwrap();
		let drawable = model.puppet.node_comps.get::<Drawable>(id).unwrap();
		let Some(tex) = decoded.get(tm.tex_albedo.0) else {
			continue;
		};
		let abs = model
			.puppet
			.node_transform(id)
			.map(|t| t.absolute)
			.unwrap_or_default();
		let world_center = abs.transform_point3(vec3(0.0, 0.0, 0.0));
		let img = RgbaImage::from_raw(tex.width(), tex.height(), tex.pixels().to_vec())
			.ok_or_else(|| "Invalid decoded texture buffer".to_owned())?;
		let left = (cx + world_center.x - tex.width() as f32 * 0.5).round() as i32;
		let top = (cy + world_center.y - tex.height() as f32 * 0.5).round() as i32;

		export_layers.push(PsdExportLayer {
			name: node.name.clone(),
			left,
			top,
			image: img,
			opacity: (drawable.blending.opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
			blend_mode: drawable.blending.mode,
			clipping: drawable.masks.is_some(),
			visible: node.enabled,
		});
	}

	write_layered_psd(width, height, &export_layers)
}

pub fn photocraft_cli_path() -> String {
	std::env::var("PHOTOCRAFT_CLI").unwrap_or_else(|_| DEFAULT_PHOTOCRAFT_CLI.to_owned())
}

/// Call `D:\app\PhotoCraft\photocraft-cli.exe info --compact <file>` to inspect a PSD/PCRAFT/image.
pub fn photocraft_inspect(file_path: &Path) -> Result<JsonValue, String> {
	let exe = photocraft_cli_path();
	if !Path::new(&exe).exists() {
		return Err(format!("PhotoCraft CLI not found at '{exe}'"));
	}
	let output = Command::new(&exe)
		.arg("info")
		.arg("--compact")
		.arg(file_path)
		.output()
		.map_err(|e| format!("Failed to execute '{exe}': {e}"))?;
	if !output.status.success() {
		return Err(format!(
			"photocraft-cli info failed: {}",
			String::from_utf8_lossy(&output.stderr)
		));
	}
	let stdout = String::from_utf8_lossy(&output.stdout);
	json::parse(stdout.trim()).map_err(|e| format!("Invalid JSON from photocraft-cli: {e}"))
}

/// Call `D:\app\PhotoCraft\photocraft-cli.exe convert <in> <out>`.
pub fn photocraft_convert(in_path: &Path, out_path: &Path) -> Result<String, String> {
	let exe = photocraft_cli_path();
	if !Path::new(&exe).exists() {
		return Err(format!("PhotoCraft CLI not found at '{exe}'"));
	}
	let output = Command::new(&exe)
		.arg("convert")
		.arg(in_path)
		.arg(out_path)
		.output()
		.map_err(|e| format!("Failed to execute '{exe}': {e}"))?;
	if !output.status.success() {
		return Err(format!(
			"photocraft-cli convert failed: {}",
			String::from_utf8_lossy(&output.stderr)
		));
	}
	Ok(format!("Converted {} -> {} via PhotoCraft", in_path.display(), out_path.display()))
}

use std::collections::HashMap;
use std::io::{self, Write};

use image::RgbaImage;

fn build_palette() -> [[u8; 3]; 256] {
	let mut pal = [[0u8; 3]; 256];
	for r in 0..6usize {
		for g in 0..6usize {
			for b in 0..6usize {
				let idx = r * 36 + g * 6 + b;
				pal[idx] = [(r * 51) as u8, (g * 51) as u8, (b * 51) as u8];
			}
		}
	}
	for i in 0..40usize {
		let v = ((i * 255) / 39) as u8;
		pal[216 + i] = [v, v, v];
	}
	pal
}

fn quantize_rgb(r: u8, g: u8, b: u8, pal: &[[u8; 3]; 256]) -> u8 {
	let ri = ((r as usize * 5 + 127) / 255).min(5);
	let gi = ((g as usize * 5 + 127) / 255).min(5);
	let bi = ((b as usize * 5 + 127) / 255).min(5);
	let cube_idx = ri * 36 + gi * 6 + bi;
	let c = pal[cube_idx];
	let err_cube = (r as i32 - c[0] as i32).pow(2)
		+ (g as i32 - c[1] as i32).pow(2)
		+ (b as i32 - c[2] as i32).pow(2);

	let lum = (r as usize * 30 + g as usize * 59 + b as usize * 11) / 100;
	let gray_i = ((lum * 39 + 127) / 255).min(39);
	let gray_idx = 216 + gray_i;
	let gc = pal[gray_idx];
	let err_gray = (r as i32 - gc[0] as i32).pow(2)
		+ (g as i32 - gc[1] as i32).pow(2)
		+ (b as i32 - gc[2] as i32).pow(2);

	if err_gray < err_cube {
		gray_idx as u8
	} else {
		cube_idx as u8
	}
}

struct BitPacker {
	bytes: Vec<u8>,
	acc: u32,
	bits: u8,
}

impl BitPacker {
	fn new() -> Self {
		Self {
			bytes: Vec::new(),
			acc: 0,
			bits: 0,
		}
	}

	fn push(&mut self, code: u16, width: u8) {
		self.acc |= (code as u32) << self.bits;
		self.bits += width;
		while self.bits >= 8 {
			self.bytes.push((self.acc & 0xFF) as u8);
			self.acc >>= 8;
			self.bits -= 8;
		}
	}

	fn finish(mut self) -> Vec<u8> {
		if self.bits > 0 {
			self.bytes.push((self.acc & 0xFF) as u8);
		}
		self.bytes
	}
}

fn lzw_encode_indices(indices: &[u8]) -> Vec<u8> {
	const CLEAR: u16 = 256;
	const EOI: u16 = 257;

	let mut packer = BitPacker::new();
	let mut code_bits: u8 = 9;
	let mut next_code: u16 = 258;
	let mut dict: HashMap<(u16, u8), u16> = HashMap::with_capacity(4096);

	packer.push(CLEAR, code_bits);
	if indices.is_empty() {
		packer.push(EOI, code_bits);
		return packer.finish();
	}

	let mut prefix = indices[0] as u16;
	for &k in &indices[1..] {
		if let Some(&existing) = dict.get(&(prefix, k)) {
			prefix = existing;
		} else {
			packer.push(prefix, code_bits);
			if next_code < 4096 {
				dict.insert((prefix, k), next_code);
				// GIF89a LZW uses deferred ("late-change") bit-width increments:
				// the decoder inserts one step behind the encoder, so `code_bits` bumps
				// only when `next_code` reaches `2^code_bits` *before* advancing.
				if next_code == (1u16 << code_bits) && code_bits < 12 {
					code_bits += 1;
				}
				next_code += 1;
			} else {
				packer.push(CLEAR, code_bits);
				dict.clear();
				code_bits = 9;
				next_code = 258;
			}
			prefix = k as u16;
		}
	}
	packer.push(prefix, code_bits);
	packer.push(EOI, code_bits);
	packer.finish()
}

/// Encode a sequence of equal-sized RGBA frames into a looping GIF89a animation.
pub fn encode_animated_gif<W: Write>(frames: &[RgbaImage], delay_cs: u16, w: &mut W) -> io::Result<()> {
	if frames.is_empty() {
		return Err(io::Error::new(io::ErrorKind::InvalidInput, "Cannot encode 0 frames"));
	}
	let (width, height) = frames[0].dimensions();
	if width == 0 || height == 0 || width > u16::MAX as u32 || height > u16::MAX as u32 {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			format!("GIF frame dimensions {width}x{height} out of valid 1..=65535 range"),
		));
	}
	let pal = build_palette();

	// Header & Logical Screen Descriptor
	w.write_all(b"GIF89a")?;
	w.write_all(&(width as u16).to_le_bytes())?;
	w.write_all(&(height as u16).to_le_bytes())?;
	w.write_all(&[0xF7, 0x00, 0x00])?; // Global palette: 256 colors (size=7, 8-bit res)

	// Global Color Table
	for c in &pal {
		w.write_all(c)?;
	}

	// Netscape 2.0 infinite loop application extension
	w.write_all(&[0x21, 0xFF, 0x0B])?;
	w.write_all(b"NETSCAPE2.0")?;
	w.write_all(&[0x03, 0x01, 0x00, 0x00, 0x00])?;

	for frame in frames {
		let (fw, fh) = frame.dimensions();
		if fw != width || fh != height {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"All GIF frames must share the same dimensions",
			));
		}

		// Graphic Control Extension (delay in 1/100s, disposal = 1)
		w.write_all(&[0x21, 0xF9, 0x04, 0x04])?;
		w.write_all(&delay_cs.to_le_bytes())?;
		w.write_all(&[0x00, 0x00])?;

		// Image Descriptor
		w.write_all(&[0x2C])?;
		w.write_all(&0u16.to_le_bytes())?;
		w.write_all(&0u16.to_le_bytes())?;
		w.write_all(&(width as u16).to_le_bytes())?;
		w.write_all(&(height as u16).to_le_bytes())?;
		w.write_all(&[0x00])?;

		// Minimum LZW code size = 8
		w.write_all(&[8u8])?;

		let mut indices = Vec::with_capacity((width * height) as usize);
		for px in frame.pixels() {
			indices.push(quantize_rgb(px.0[0], px.0[1], px.0[2], &pal));
		}
		let compressed = lzw_encode_indices(&indices);
		for chunk in compressed.chunks(255) {
			w.write_all(&[chunk.len() as u8])?;
			w.write_all(chunk)?;
		}
		w.write_all(&[0x00])?; // Block terminator
	}

	// GIF Trailer
	w.write_all(&[0x3B])?;
	w.flush()
}

#[cfg(test)]
mod tests {
	use super::*;
	use image::Rgba;

	fn lzw_decode_indices(sub_blocks: &[u8]) -> Vec<u8> {
		const CLEAR: u16 = 256;
		const EOI: u16 = 257;

		// Collect packed LZW data bytes from sub-blocks up to 0x00 terminator.
		let mut data = Vec::new();
		let mut pos = 0;
		while pos < sub_blocks.len() {
			let len = sub_blocks[pos] as usize;
			pos += 1;
			if len == 0 {
				break;
			}
			data.extend_from_slice(&sub_blocks[pos..pos + len]);
			pos += len;
		}

		let mut bit_pos = 0usize;
		let read_code = |bit_pos: &mut usize, bits: u8| -> Option<u16> {
			let mut val = 0u32;
			for i in 0..bits {
				let b_idx = (*bit_pos + i as usize) / 8;
				let b_off = (*bit_pos + i as usize) % 8;
				if b_idx >= data.len() {
					return None;
				}
				let bit = ((data[b_idx] >> b_off) & 1) as u32;
				val |= bit << i;
			}
			*bit_pos += bits as usize;
			Some(val as u16)
		};

		let mut dict: Vec<Vec<u8>> = (0..=255u8).map(|b| vec![b]).collect();
		dict.push(Vec::new()); // 256 CLEAR
		dict.push(Vec::new()); // 257 EOI
		let mut code_bits = 9u8;
		let mut prev: Option<Vec<u8>> = None;
		let mut out = Vec::new();

		while let Some(code) = read_code(&mut bit_pos, code_bits) {
			if code == CLEAR {
				dict.truncate(258);
				code_bits = 9;
				prev = None;
				continue;
			}
			if code == EOI {
				break;
			}
			let entry = if (code as usize) < dict.len() {
				dict[code as usize].clone()
			} else if code as usize == dict.len() {
				let mut e = prev.clone().expect("valid LZW KwKwK code");
				e.push(e[0]);
				e
			} else {
				panic!("invalid LZW code {code} at dict len {} (bits={code_bits})", dict.len());
			};
			out.extend_from_slice(&entry);
			if let Some(p) = &prev {
				if dict.len() < 4096 {
					let mut new_entry = p.clone();
					new_entry.push(entry[0]);
					dict.push(new_entry);
					if dict.len() == (1usize << code_bits) && code_bits < 12 {
						code_bits += 1;
					}
				}
			}
			prev = Some(entry);
		}
		out
	}

	#[test]
	fn lzw_encode_handles_width_bumps_across_512_and_1024_cleanly() {
		// Deterministic pseudo-random pattern over 216+ palette entries to force >1024 dict entries.
		let w = 64u32;
		let h = 64u32;
		let mut img = RgbaImage::new(w, h);
		let pal = build_palette();
		let mut expected_indices = Vec::with_capacity((w * h) as usize);
		for y in 0..h {
			for x in 0..w {
				let v = ((x.wrapping_mul(37) ^ y.wrapping_mul(91) ^ (x * y)) & 0xFF) as u8;
				let px = Rgba([v, v.wrapping_mul(3), v.wrapping_mul(7), 255]);
				img.put_pixel(x, y, px);
				expected_indices.push(quantize_rgb(px.0[0], px.0[1], px.0[2], &pal));
			}
		}

		let mut buf = Vec::new();
		encode_animated_gif(&[img], 4, &mut buf).expect("GIF encoding succeeds");
		assert_eq!(&buf[0..6], b"GIF89a");

		// Locate the Image Descriptor (0x2C) after Header(13) + GCT(768) + Netscape(19) + GCE(8).
		let img_desc_offset = 13 + 768 + 19 + 8;
		assert_eq!(buf[img_desc_offset], 0x2C);
		assert_eq!(buf[img_desc_offset + 10], 8); // min LZW code size = 8
		let sub_blocks = &buf[img_desc_offset + 11..buf.len() - 1];
		let decoded = lzw_decode_indices(sub_blocks);
		assert_eq!(decoded, expected_indices);
	}

	#[test]
	fn encode_animated_gif_rejects_invalid_dimensions() {
		let zero = RgbaImage::new(0, 10);
		let mut buf = Vec::new();
		assert!(encode_animated_gif(&[zero], 4, &mut buf).is_err());

		let oversized = RgbaImage::new(u16::MAX as u32 + 1, 1);
		assert!(encode_animated_gif(&[oversized], 4, &mut buf).is_err());
	}
}

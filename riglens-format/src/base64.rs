use serde::{Deserialize, Deserializer, Serializer};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes using standard RFC 4648 base64 with `=` padding.
pub fn encode(input: &[u8]) -> String {
	if input.is_empty() {
		return String::new();
	}
	let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
	for chunk in input.chunks(3) {
		let b0 = chunk[0] as u32;
		let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
		let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
		let triple = (b0 << 16) | (b1 << 8) | b2;
		out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
		out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
		out.push(if chunk.len() > 1 {
			ALPHABET[((triple >> 6) & 0x3F) as usize] as char
		} else {
			'='
		});
		out.push(if chunk.len() > 2 {
			ALPHABET[(triple & 0x3F) as usize] as char
		} else {
			'='
		});
	}
	out
}

fn decode_char(b: u8) -> Result<u8, String> {
	match b {
		b'A'..=b'Z' => Ok(b - b'A'),
		b'a'..=b'z' => Ok(b - b'a' + 26),
		b'0'..=b'9' => Ok(b - b'0' + 52),
		b'+' => Ok(62),
		b'/' => Ok(63),
		_ => Err(format!("invalid base64 byte: 0x{b:02x}")),
	}
}

/// Decode standard RFC 4648 base64 (ignoring ASCII whitespace).
pub fn decode(input: &str) -> Result<Vec<u8>, String> {
	let cleaned: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
	if cleaned.is_empty() {
		return Ok(Vec::new());
	}
	if cleaned.len() % 4 != 0 {
		return Err(format!("base64 length {} is not a multiple of 4", cleaned.len()));
	}
	let mut out = Vec::with_capacity((cleaned.len() / 4) * 3);
	let num_chunks = cleaned.len() / 4;
	for (idx, chunk) in cleaned.chunks_exact(4).enumerate() {
		let is_last = idx + 1 == num_chunks;
		let c0 = decode_char(chunk[0])?;
		let c1 = decode_char(chunk[1])?;
		let pad2 = chunk[2] == b'=';
		let pad3 = chunk[3] == b'=';
		if (pad2 && !pad3) || ((pad2 || pad3) && !is_last) {
			return Err("invalid base64 padding".to_owned());
		}
		let c2 = if pad2 { 0 } else { decode_char(chunk[2])? };
		let c3 = if pad3 { 0 } else { decode_char(chunk[3])? };
		let triple = ((c0 as u32) << 18) | ((c1 as u32) << 12) | ((c2 as u32) << 6) | (c3 as u32);
		out.push(((triple >> 16) & 0xFF) as u8);
		if !pad2 {
			out.push(((triple >> 8) & 0xFF) as u8);
		}
		if !pad3 {
			out.push((triple & 0xFF) as u8);
		}
	}
	Ok(out)
}

pub fn serialize<S: Serializer>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error> {
	serializer.serialize_str(&encode(bytes))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
	let s = String::deserialize(deserializer)?;
	decode(&s).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn base64_roundtrips() {
		let cases: &[&[u8]] = &[
			b"",
			b"a",
			b"ab",
			b"abc",
			b"abcd",
			b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\xff\x80\x01",
		];
		for &raw in cases {
			let enc = encode(raw);
			let dec = decode(&enc).expect("decode");
			assert_eq!(dec, raw);
		}
		assert_eq!(encode(b"Man"), "TWFu");
		assert_eq!(encode(b"Ma"), "TWE=");
		assert_eq!(encode(b"M"), "TQ==");
	}
}

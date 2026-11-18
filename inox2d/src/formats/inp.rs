use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;
use std::str::Utf8Error;
use std::string::FromUtf8Error;
use std::sync::Arc;

use image::ImageFormat;

use crate::model::{Model, ModelTexture, VendorData};
use crate::puppet::Puppet;

use super::json::JsonError;
use super::payload::InoxParseError;
use super::{read_be_u32, read_n, read_u8, read_vec};

#[derive(Debug, thiserror::Error)]
#[error("Could not parse INP file\n  - {0}")]
pub enum ParseInpError {
	#[error("magic bytes do not match, the file is not in the INP format")]
	IncorrectMagic,
	#[error("there is no texture section")]
	NoTexSect,
	#[error("BC7 texture encoding is not supported yet")]
	Bc7NotSupported,
	#[error("Invalid texture encoding: {0}")]
	InvalidTexEncoding(u8),
	Io(#[from] io::Error),
	Utf8(#[from] Utf8Error),
	FromUtf8(#[from] FromUtf8Error),
	JsonParse(#[from] json::Error),
	InoxParse(#[from] InoxParseError),
	Json(#[from] JsonError),
}

/// Trans rights!
const MAGIC: &[u8] = b"TRNSRTS\0";
/// Texture section header
const TEX_SECT: &[u8] = b"TEX_SECT";
/// Optional EXTended Vendor Data section for app provided settings for the puppet
const EXT_SECT: &[u8] = b"EXT_SECT";

/// Parse `.inp` and `.inx` files.
pub fn parse_inp<R: Read>(mut data: R) -> Result<Model, ParseInpError> {
	// check magic bytes
	let magic = read_n::<_, 8>(&mut data)?;
	if magic != MAGIC {
		return Err(ParseInpError::IncorrectMagic);
	}

	// parse json payload into puppet
	let length = read_be_u32(&mut data)? as usize;
	let payload = read_vec(&mut data, length)?;
	let payload = std::str::from_utf8(&payload)?;
	let payload = json::parse(payload)?;
	let puppet = Puppet::new_from_json(&payload)?;

	// check texture section header
	let tex_sect = read_n::<_, 8>(&mut data).map_err(|_| ParseInpError::NoTexSect)?;
	if tex_sect != TEX_SECT {
		return Err(ParseInpError::NoTexSect);
	}

	// retrieve textures
	let tex_count = read_be_u32(&mut data)? as usize;
	let mut textures = Vec::with_capacity(tex_count);
	for _ in 0..tex_count {
		let tex_length = read_be_u32(&mut data)? as usize;
		let tex_encoding = read_u8(&mut data)?;

		let format = match tex_encoding {
			0 => ImageFormat::Png, // PNG
			1 => ImageFormat::Tga, // TGA
			2 => return Err(ParseInpError::Bc7NotSupported),
			n => return Err(ParseInpError::InvalidTexEncoding(n)),
		};

		let data: Arc<[u8]> = read_vec(&mut data, tex_length)?.into();
		textures.push(ModelTexture { format, data });
	}

	// read extended section header if present
	let vendors = match read_n::<_, 8>(&mut data) {
		Ok(ext_sect) if ext_sect == EXT_SECT => {
			let ext_count = read_be_u32(&mut data)? as usize;
			let mut vendors = Vec::with_capacity(ext_count);
			for _ in 0..ext_count {
				let length = read_be_u32(&mut data)? as usize;
				let name = read_vec(&mut data, length)?;
				let name = String::from_utf8(name)?;

				let length = read_be_u32(&mut data)? as usize;
				let payload = read_vec(&mut data, length)?;
				let payload = std::str::from_utf8(&payload)?;
				let payload = json::parse(payload)?;

				vendors.push(VendorData { name, payload });
			}
			vendors
		}
		_ => Vec::new(),
	};

	Ok(Model {
		puppet,
		textures,
		vendors,
	})
}

/// Parse `.inp` and `.inx` files.
pub fn dump_inp<R: Read>(mut data: R, directory: &Path) -> Result<(), ParseInpError> {
	// check magic bytes
	let magic = read_n::<_, 8>(&mut data)?;
	if magic != MAGIC {
		return Err(ParseInpError::IncorrectMagic);
	}

	// parse json payload into puppet
	let length = read_be_u32(&mut data)? as usize;
	let payload = read_vec(&mut data, length)?;
	File::create(directory.join("payload.json"))?.write_all(&payload)?;

	// check texture section header
	let tex_sect = read_n::<_, 8>(&mut data).map_err(|_| ParseInpError::NoTexSect)?;
	if tex_sect != TEX_SECT {
		return Err(ParseInpError::NoTexSect);
	}

	// retrieve textures
	fs::create_dir(directory.join("textures"))?;
	let tex_count = read_be_u32(&mut data)? as usize;
	for i in 0..tex_count {
		let tex_length = read_be_u32(&mut data)? as usize;
		let tex_encoding = read_u8(&mut data)?;

		let format = match tex_encoding {
			0 => ImageFormat::Png, // PNG
			1 => ImageFormat::Tga, // TGA
			2 => return Err(ParseInpError::Bc7NotSupported),
			n => return Err(ParseInpError::InvalidTexEncoding(n)),
		};

		let extension = match format {
			ImageFormat::Png => "png",
			ImageFormat::Jpeg => "jpeg",
			ImageFormat::Gif => "gif",
			ImageFormat::WebP => "webp",
			ImageFormat::Pnm => "pnm",
			ImageFormat::Tiff => "tiff",
			ImageFormat::Tga => "tga",
			ImageFormat::Dds => "dds",
			ImageFormat::Bmp => "bmp",
			ImageFormat::Ico => "ico",
			ImageFormat::Hdr => "hdr",
			ImageFormat::OpenExr => "openexr",
			ImageFormat::Farbfeld => "farbfeld",
			ImageFormat::Avif => "avif",
			ImageFormat::Qoi => "qoi",
			_ => unreachable!(),
		};

		let data: Vec<u8> = read_vec(&mut data, tex_length)?;
		File::create(directory.join(format!("textures/tex-{:03}.{}", i, extension)))?.write_all(&data)?;
	}

	// read extended section header if present
	let Ok(ext_sect) = read_n::<_, 8>(&mut data) else {
		return Ok(());
	};

	fs::create_dir(directory.join("vendors"))?;
	if ext_sect == EXT_SECT {
		let ext_count = read_be_u32(&mut data)? as usize;

		for i in 0..ext_count {
			let length = read_be_u32(&mut data)? as usize;
			let name = read_vec(&mut data, length)?;
			let name = String::from_utf8(name)?;

			let length = read_be_u32(&mut data)? as usize;
			let payload = read_vec(&mut data, length)?;

			File::create(directory.join(format!("vendors/{:02} - {}.json", i, name)))?.write_all(&payload)?;
		}
	}

	Ok(())
}

pub fn dump_to_inp<W: Write>(directory: &Path, w: &mut W) -> io::Result<()> {
	let mut payload_file = File::open(directory.join("payload.json"))?;

	w.write_all(MAGIC)?;
	w.write_all(&(payload_file.metadata()?.len() as u32).to_be_bytes())?;
	io::copy(&mut payload_file, w)?;

	let mut texture_files = Vec::new();
	for tex_file in fs::read_dir(directory.join("textures"))? {
		let tex_file = tex_file?;
		let path = tex_file.path();
		let Some(ext) = path.extension() else {
			eprintln!("File {:?} has no extension, ignoring", tex_file.file_name());
			continue;
		};

		let Some(ext) = ext.to_str() else {
			eprintln!("File {:?} has unrecognized extension, ignoring", tex_file.file_name());
			continue;
		};

		let tex_encoding: u8 = match ext {
			"png" => 0,
			"tga" => 1,
			"bc7" => 2,
			ext => {
				eprintln!(
					"File {:?} has unsupported extension {:?}, ignoring",
					tex_file.file_name(),
					ext
				);
				continue;
			}
		};

		texture_files.push((tex_encoding, path));
	}

	w.write_all(TEX_SECT)?;
	w.write_all(&(texture_files.len() as u32).to_be_bytes())?;
	for (tex_encoding, tex_path) in texture_files {
		let mut tex_file = File::open(tex_path)?;

		let file_len = tex_file.metadata()?.len() as u32;
		println!("t {} | {:?} {} B", tex_encoding, file_len.to_be_bytes(), file_len);

		w.write_all(&file_len.to_be_bytes())?;
		w.write_all(&[tex_encoding])?;

		io::copy(&mut tex_file, w)?;
	}

	w.flush()?;
	Ok(())
}

/// Write a `Model` directly to the binary `.inp` format (`TRNSRTS\0` + JSON + `TEX_SECT` + optional `EXT_SECT`).
pub fn write_inp<W: Write>(model: &Model, w: &mut W) -> io::Result<()> {
	let payload_str = json::stringify(model.puppet.to_json());
	let payload_bytes = payload_str.as_bytes();
	let payload_len = u32::try_from(payload_bytes.len())
		.map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "JSON payload exceeds 4GiB"))?;

	w.write_all(MAGIC)?;
	w.write_all(&payload_len.to_be_bytes())?;
	w.write_all(payload_bytes)?;

	w.write_all(TEX_SECT)?;
	let tex_count = u32::try_from(model.textures.len())
		.map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Too many textures"))?;
	w.write_all(&tex_count.to_be_bytes())?;

	for tex in &model.textures {
		let encoding: u8 = match tex.format {
			ImageFormat::Png => 0,
			ImageFormat::Tga => 1,
			other => {
				return Err(io::Error::new(
					io::ErrorKind::InvalidInput,
					format!("Unsupported texture format for .inp export: {other:?}"),
				));
			}
		};
		let tex_len = u32::try_from(tex.data.len())
			.map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Texture exceeds 4GiB"))?;
		w.write_all(&tex_len.to_be_bytes())?;
		w.write_all(&[encoding])?;
		w.write_all(&tex.data)?;
	}

	if !model.vendors.is_empty() {
		w.write_all(EXT_SECT)?;
		let ext_count = u32::try_from(model.vendors.len())
			.map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Too many vendor blocks"))?;
		w.write_all(&ext_count.to_be_bytes())?;

		for vendor in &model.vendors {
			let name_bytes = vendor.name.as_bytes();
			let name_len = u32::try_from(name_bytes.len())
				.map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Vendor name exceeds 4GiB"))?;
			let v_payload = json::stringify(vendor.payload.clone());
			let v_bytes = v_payload.as_bytes();
			let v_len = u32::try_from(v_bytes.len())
				.map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Vendor payload exceeds 4GiB"))?;

			w.write_all(&name_len.to_be_bytes())?;
			w.write_all(name_bytes)?;
			w.write_all(&v_len.to_be_bytes())?;
			w.write_all(v_bytes)?;
		}
	}

	w.flush()?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::math::interp::InterpolateMode;
	use crate::math::matrix::Matrix2d;
	use crate::math::transform::TransformOffset;
	use crate::node::components::{BlendMode, Blending, Drawable, Mesh, TexturedMesh};
	use crate::node::{InoxNode, InoxNodeUuid};
	use crate::params::{AxisPoints, Binding, BindingValues, Param};
	use crate::texture::TextureId;
	use glam::{vec2, vec3};

	#[test]
	fn inp_roundtrip_and_dynamic_zsort() {
		let mut puppet = Puppet::new_empty("RoundTripTest");
		let root_id = puppet.nodes.root_node_id;
		let part_id = puppet.next_node_uuid();
		puppet.add_node(
			root_id,
			InoxNode {
				uuid: part_id,
				name: "Face".to_owned(),
				enabled: true,
				zsort: 1.0,
				trans_offset: TransformOffset::default(),
				lock_to_root: false,
			},
		);
		puppet.node_comps.add(
			part_id,
			Drawable {
				blending: Blending {
					mode: BlendMode::Normal,
					tint: vec3(1.0, 1.0, 1.0),
					screen_tint: vec3(0.0, 0.0, 0.0),
					opacity: 1.0,
				},
				masks: None,
			},
		);
		puppet.node_comps.add(
			part_id,
			TexturedMesh {
				tex_albedo: TextureId(0),
				tex_emissive: TextureId(0),
				tex_bumpmap: TextureId(0),
			},
		);
		puppet.node_comps.add(
			part_id,
			Mesh {
				vertices: vec![vec2(-10.0, -10.0), vec2(10.0, -10.0), vec2(0.0, 10.0)],
				uvs: vec![vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.5, 1.0)],
				indices: vec![0, 1, 2],
				origin: vec2(0.0, 0.0),
			},
		);

		let param_uuid = puppet.next_param_uuid();
		let mut deform_mat = Matrix2d::from_grid_transposed(2, 1, |x, _| {
			if x == 0 {
				vec![vec2(0.0, 0.0); 3]
			} else {
				vec![vec2(5.0, 0.0); 3]
			}
		});
		let _ = &mut deform_mat;
		let zsort_mat = Matrix2d::from_grid_transposed(2, 1, |x, _| if x == 0 { 0.0 } else { -10.0 });
		let is_set = Matrix2d::from_grid_transposed(2, 1, |_, _| true);

		puppet.params.insert(
			"DepthFlip".to_owned(),
			Param {
				uuid: param_uuid,
				name: "DepthFlip".to_owned(),
				is_vec2: false,
				min: vec2(0.0, 0.0),
				max: vec2(1.0, 0.0),
				defaults: vec2(0.0, 0.0),
				axis_points: AxisPoints {
					x: vec![0.0, 1.0],
					y: vec![0.0],
				},
				bindings: vec![
					Binding {
						node: part_id,
						is_set: is_set.clone(),
						interpolate_mode: InterpolateMode::Linear,
						values: BindingValues::Deform(deform_mat),
					},
					Binding {
						node: part_id,
						is_set,
						interpolate_mode: InterpolateMode::Linear,
						values: BindingValues::ZSort(zsort_mat),
					},
				],
			},
		);

		let model = Model {
			puppet,
			textures: vec![ModelTexture {
				format: ImageFormat::Png,
				data: vec![137, 80, 78, 71, 13, 10, 26, 10].into(),
			}],
			vendors: Vec::new(),
		};

		let mut bytes = Vec::new();
		write_inp(&model, &mut bytes).expect("write_inp");
		let mut parsed = parse_inp(bytes.as_slice()).expect("parse_inp");

		assert_eq!(parsed.puppet.meta.name.as_deref(), Some("RoundTripTest"));
		assert_eq!(parsed.textures.len(), 1);
		assert!(parsed.puppet.params.contains_key("DepthFlip"));

		parsed.puppet.init_all();
		parsed.puppet.begin_frame();
		parsed.puppet.param_ctx.as_mut().unwrap().set("DepthFlip", vec2(1.0, 0.0)).unwrap();
		parsed.puppet.end_frame(0.0);

		let z = parsed.puppet.node_zsort(InoxNodeUuid(1)).unwrap();
		assert!((z - (-9.0)).abs() < 1e-4, "expected zsort -9.0, got {z}");
		let deforms = &parsed.puppet.render_ctx.as_ref().unwrap().vertex_buffers.deforms;
		assert!((deforms.last().unwrap().x - 5.0).abs() < 1e-4);
	}
}

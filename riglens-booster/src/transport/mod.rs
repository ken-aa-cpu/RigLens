//! Zero-copy and memory-backed video frame transport abstractions.

pub mod spout2;

use std::sync::{Arc, Mutex};

use thiserror::Error;

/// Pixel layout of a [`FrameBuffer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
	Rgba8,
	Bgra8,
}

/// CPU/staging frame buffer exchanged across transport stages or tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameBuffer {
	pub width: u32,
	pub height: u32,
	pub format: PixelFormat,
	pub data: Vec<u8>,
}

impl FrameBuffer {
	pub fn new(width: u32, height: u32, format: PixelFormat, data: Vec<u8>) -> Result<Self, TransportError> {
		let expected = (width as usize)
			.checked_mul(height as usize)
			.and_then(|px| px.checked_mul(4))
			.ok_or(TransportError::InvalidDimensions(width, height))?;
		if width == 0 || height == 0 || data.len() != expected {
			return Err(TransportError::BufferSizeMismatch {
				expected,
				actual: data.len(),
			});
		}
		Ok(Self {
			width,
			height,
			format,
			data,
		})
	}

	/// Return a copy converted to tightly packed `Rgba8` bytes.
	pub fn to_rgba8(&self) -> Vec<u8> {
		match self.format {
			PixelFormat::Rgba8 => self.data.clone(),
			PixelFormat::Bgra8 => {
				let mut out = self.data.clone();
				swizzle_rgba_bgra(&mut out);
				out
			}
		}
	}

	/// Return a copy converted to tightly packed `Bgra8` bytes (Spout2 DX11 default).
	pub fn to_bgra8(&self) -> Vec<u8> {
		match self.format {
			PixelFormat::Bgra8 => self.data.clone(),
			PixelFormat::Rgba8 => {
				let mut out = self.data.clone();
				swizzle_rgba_bgra(&mut out);
				out
			}
		}
	}
}

/// Swap R and B channels in-place across packed 4-byte pixels (`RGBA8 <-> BGRA8`).
#[inline]
pub fn swizzle_rgba_bgra(bytes: &mut [u8]) {
	for px in bytes.chunks_exact_mut(4) {
		px.swap(0, 2);
	}
}

#[derive(Debug, Error)]
pub enum TransportError {
	#[error("Invalid frame dimensions {0}x{1}")]
	InvalidDimensions(u32, u32),
	#[error("Frame buffer size mismatch: expected {expected} bytes, got {actual}")]
	BufferSizeMismatch { expected: usize, actual: usize },
	#[error("Spout2 sender '{0}' not found or inactive")]
	SenderNotFound(String),
	#[error("Unsupported pixel/DXGI format: {0}")]
	UnsupportedFormat(u32),
	#[error("Spout2 / DX11 native transport is only supported on Windows")]
	UnsupportedPlatform,
	#[error("Windows / Direct3D 11 error: {0}")]
	Native(String),
}

/// Frame producer abstraction (Windows Spout2 receiver, in-memory bus, or future RigLens renderer).
pub trait FrameSource: Send {
	fn name(&self) -> &str;
	fn recv_frame(&mut self) -> Result<Option<FrameBuffer>, TransportError>;
}

/// Frame consumer abstraction (Windows Spout2 sender or in-memory bus).
pub trait FrameSink: Send {
	fn name(&self) -> &str;
	fn send_frame(&mut self, frame: &FrameBuffer) -> Result<(), TransportError>;
}

/// Thread-safe in-memory latest-frame transport bus for pipelines and tests.
#[derive(Clone, Default)]
pub struct MemoryFrameBus {
	name: String,
	latest: Arc<Mutex<Option<FrameBuffer>>>,
}

impl MemoryFrameBus {
	pub fn new(name: impl Into<String>) -> Self {
		Self {
			name: name.into(),
			latest: Arc::new(Mutex::new(None)),
		}
	}
}

impl FrameSink for MemoryFrameBus {
	fn name(&self) -> &str {
		&self.name
	}

	fn send_frame(&mut self, frame: &FrameBuffer) -> Result<(), TransportError> {
		let mut guard = self
			.latest
			.lock()
			.map_err(|_| TransportError::Native("MemoryFrameBus mutex poisoned".to_owned()))?;
		*guard = Some(frame.clone());
		Ok(())
	}
}

impl FrameSource for MemoryFrameBus {
	fn name(&self) -> &str {
		&self.name
	}

	fn recv_frame(&mut self) -> Result<Option<FrameBuffer>, TransportError> {
		let guard = self
			.latest
			.lock()
			.map_err(|_| TransportError::Native("MemoryFrameBus mutex poisoned".to_owned()))?;
		Ok(guard.clone())
	}
}

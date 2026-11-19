//! Directional ambient rim color presets and optional Windows screen-edge color sampling.

use glam::{vec3, Vec3};

/// Directional ambient RGB colors (`0..=1`) used by the 2.5D SDF rim-light pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmbientColors {
	pub left: Vec3,
	pub right: Vec3,
	pub top: Vec3,
	pub bottom: Vec3,
}

impl Default for AmbientColors {
	fn default() -> Self {
		AmbientTheme::Cyberpunk.colors()
	}
}

/// Ambient lighting source preset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AmbientTheme {
	Cyberpunk,
	Warm,
	Ocean,
	ScreenSync,
	Custom(AmbientColors),
}

impl AmbientTheme {
	pub fn parse(name: &str) -> Option<Self> {
		match name.trim().to_ascii_lowercase().as_str() {
			"cyberpunk" | "neon" => Some(Self::Cyberpunk),
			"warm" | "sunset" => Some(Self::Warm),
			"ocean" | "cool" => Some(Self::Ocean),
			"screen" | "screensync" | "sync" => Some(Self::ScreenSync),
			_ => None,
		}
	}

	pub fn colors(self) -> AmbientColors {
		match self {
			Self::Cyberpunk => AmbientColors {
				left: vec3(0.05, 0.88, 1.00),
				right: vec3(1.00, 0.22, 0.78),
				top: vec3(0.65, 0.85, 1.00),
				bottom: vec3(0.35, 0.15, 0.65),
			},
			Self::Warm => AmbientColors {
				left: vec3(1.00, 0.58, 0.22),
				right: vec3(1.00, 0.34, 0.48),
				top: vec3(1.00, 0.90, 0.65),
				bottom: vec3(0.45, 0.20, 0.16),
			},
			Self::Ocean => AmbientColors {
				left: vec3(0.15, 0.72, 1.00),
				right: vec3(0.28, 0.96, 0.78),
				top: vec3(0.80, 0.95, 1.00),
				bottom: vec3(0.08, 0.25, 0.55),
			},
			Self::ScreenSync => Self::Cyberpunk.colors(),
			Self::Custom(c) => c,
		}
	}
}

/// Smoothly updates directional ambient colors via EMA filtering (preventing abrupt strobing).
#[derive(Clone, Debug)]
pub struct AmbientColorSampler {
	pub theme: AmbientTheme,
	current: AmbientColors,
	ema_alpha: f32,
	frame_counter: u32,
}

impl AmbientColorSampler {
	pub fn new(theme: AmbientTheme) -> Self {
		Self {
			theme,
			current: theme.colors(),
			ema_alpha: 0.22,
			frame_counter: 0,
		}
	}

	pub fn sample(&mut self) -> AmbientColors {
		self.frame_counter = self.frame_counter.wrapping_add(1);
		let target = match self.theme {
			AmbientTheme::ScreenSync => {
				// Sample screen edges every 3 frames to keep GDI/capture overhead trivial
				if self.frame_counter == 1 || self.frame_counter % 3 == 0 {
					sample_screen_edge_colors().unwrap_or_else(|| self.current)
				} else {
					self.current
				}
			}
			other => other.colors(),
		};

		let a = self.ema_alpha.clamp(0.05, 1.0);
		self.current.left = self.current.left.lerp(target.left, a);
		self.current.right = self.current.right.lerp(target.right, a);
		self.current.top = self.current.top.lerp(target.top, a);
		self.current.bottom = self.current.bottom.lerp(target.bottom, a);
		self.current
	}
}

#[cfg(windows)]
fn sample_screen_edge_colors() -> Option<AmbientColors> {
	use std::ffi::c_void;
	use std::mem::{size_of, zeroed};
	use windows::Win32::Graphics::Gdi::{
		CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
		StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, SRCCOPY,
	};
	use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

	const GRID: i32 = 32;

	struct DcGuard(HDC);
	impl Drop for DcGuard {
		fn drop(&mut self) {
			if !self.0.is_invalid() {
				// SAFETY: Owned screen DC acquired from `GetDC(None)`.
				unsafe {
					let _ = ReleaseDC(None, self.0);
				}
			}
		}
	}

	struct MemDcGuard(HDC);
	impl Drop for MemDcGuard {
		fn drop(&mut self) {
			if !self.0.is_invalid() {
				// SAFETY: Owned memory DC created by `CreateCompatibleDC`.
				unsafe {
					let _ = DeleteDC(self.0);
				}
			}
		}
	}

	// SAFETY: Read-only 32x32 downsampled GDI copy of desktop screen edges for ambient rim sync.
	unsafe {
		let sw = GetSystemMetrics(SM_CXSCREEN);
		let sh = GetSystemMetrics(SM_CYSCREEN);
		if sw <= 0 || sh <= 0 {
			return None;
		}
		let screen_dc = GetDC(None);
		if screen_dc.is_invalid() {
			return None;
		}
		let _screen_guard = DcGuard(screen_dc);

		let mem_dc = CreateCompatibleDC(screen_dc);
		if mem_dc.is_invalid() {
			return None;
		}
		let _mem_guard = MemDcGuard(mem_dc);

		let mut bmi: BITMAPINFO = zeroed();
		bmi.bmiHeader = BITMAPINFOHEADER {
			biSize: size_of::<BITMAPINFOHEADER>() as u32,
			biWidth: GRID,
			biHeight: -GRID, // top-down
			biPlanes: 1,
			biBitCount: 32,
			biCompression: BI_RGB.0,
			..zeroed()
		};
		let mut bits_ptr: *mut c_void = std::ptr::null_mut();
		let Ok(bmp) = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits_ptr, None, 0) else {
			return None;
		};
		if bits_ptr.is_null() {
			let _ = DeleteObject(bmp);
			return None;
		}
		let old_obj = SelectObject(mem_dc, bmp);
		let ok = StretchBlt(mem_dc, 0, 0, GRID, GRID, screen_dc, 0, 0, sw, sh, SRCCOPY).as_bool();

		let result = if ok {
			let bytes = std::slice::from_raw_parts(bits_ptr as *const u8, (GRID * GRID * 4) as usize);
			let avg_band = |x0: usize, x1: usize, y0: usize, y1: usize| -> Vec3 {
				let mut acc = Vec3::ZERO;
				let mut cnt = 0.0f32;
				for y in y0..y1 {
					for x in x0..x1 {
						let i = (y * GRID as usize + x) * 4;
						// DIB 32bpp on Windows is BGRA
						let b = bytes[i] as f32 / 255.0;
						let g = bytes[i + 1] as f32 / 255.0;
						let r = bytes[i + 2] as f32 / 255.0;
						acc += vec3(r, g, b);
						cnt += 1.0;
					}
				}
				if cnt > 0.0 {
					acc / cnt
				} else {
					Vec3::splat(0.5)
				}
			};
			let g = GRID as usize;
			Some(AmbientColors {
				left: avg_band(0, 4, 0, g),
				right: avg_band(g - 4, g, 0, g),
				top: avg_band(0, g, 0, 4),
				bottom: avg_band(0, g, g - 4, g),
			})
		} else {
			None
		};

		SelectObject(mem_dc, old_obj);
		let _ = DeleteObject(bmp);
		result
	}
}

#[cfg(not(windows))]
fn sample_screen_edge_colors() -> Option<AmbientColors> {
	None
}

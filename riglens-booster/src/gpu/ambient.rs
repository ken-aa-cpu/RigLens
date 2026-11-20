//! Directional ambient rim color presets and Windows DXGI/GDI screen-edge color sampling.

use std::sync::{Arc, Mutex};

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

#[cfg(windows)]
#[derive(Clone, Default)]
struct ScreenSyncState(Arc<Mutex<Option<dxgi::DxgiState>>>);

#[cfg(windows)]
impl std::fmt::Debug for ScreenSyncState {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("ScreenSyncState(..)")
	}
}

/// Smoothly updates directional ambient colors via EMA filtering (preventing abrupt strobing).
#[derive(Clone, Debug)]
pub struct AmbientColorSampler {
	pub theme: AmbientTheme,
	current: AmbientColors,
	ema_alpha: f32,
	frame_counter: u32,
	#[cfg(windows)]
	screen_sync: ScreenSyncState,
}

impl AmbientColorSampler {
	pub fn new(theme: AmbientTheme) -> Self {
		Self {
			theme,
			current: theme.colors(),
			ema_alpha: 0.22,
			frame_counter: 0,
			#[cfg(windows)]
			screen_sync: ScreenSyncState::default(),
		}
	}

	pub fn sample(&mut self) -> AmbientColors {
		self.frame_counter = self.frame_counter.wrapping_add(1);
		let target = match self.theme {
			AmbientTheme::ScreenSync => {
				// Sample screen edges every 3 frames to keep desktop capture overhead trivial
				if self.frame_counter == 1 || self.frame_counter % 3 == 0 {
					self.sample_screen_sync().unwrap_or(self.current)
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

	#[cfg(windows)]
	fn sample_screen_sync(&self) -> Option<AmbientColors> {
		if let Ok(mut guard) = self.screen_sync.0.lock() {
			let state = guard.get_or_insert_with(dxgi::DxgiState::default);
			match state.sample_edge_colors() {
				dxgi::DxgiSample::Colors(c) => return Some(c),
				dxgi::DxgiSample::Unchanged => return None,
				dxgi::DxgiSample::Unavailable => {}
			}
		}
		sample_screen_edge_colors_gdi()
	}

	#[cfg(not(windows))]
	fn sample_screen_sync(&self) -> Option<AmbientColors> {
		None
	}
}

/// Average left/right/top/bottom 12.5% screen-edge bands from a strided RGBA/BGRA surface.
pub(crate) fn average_edge_bands(
	bytes: &[u8],
	width: usize,
	height: usize,
	row_pitch: usize,
	is_bgra: bool,
) -> Option<AmbientColors> {
	if width < 8 || height < 8 || row_pitch < width * 4 {
		return None;
	}
	const GRID: usize = 32;
	let band = (GRID / 8).max(1);
	let mut acc = [Vec3::ZERO; 4]; // left, right, top, bottom
	let mut cnt = [0.0f32; 4];

	for gy in 0..GRID {
		let py = ((gy * 2 + 1) * height) / (GRID * 2);
		let row_off = py.min(height - 1) * row_pitch;
		for gx in 0..GRID {
			let in_left = gx < band;
			let in_right = gx >= GRID - band;
			let in_top = gy < band;
			let in_bottom = gy >= GRID - band;
			if !(in_left || in_right || in_top || in_bottom) {
				continue;
			}
			let px = ((gx * 2 + 1) * width) / (GRID * 2);
			let idx = row_off + px.min(width - 1) * 4;
			if idx + 3 >= bytes.len() {
				continue;
			}
			let rgb = if is_bgra {
				vec3(
					bytes[idx + 2] as f32 / 255.0,
					bytes[idx + 1] as f32 / 255.0,
					bytes[idx] as f32 / 255.0,
				)
			} else {
				vec3(
					bytes[idx] as f32 / 255.0,
					bytes[idx + 1] as f32 / 255.0,
					bytes[idx + 2] as f32 / 255.0,
				)
			};
			if in_left {
				acc[0] += rgb;
				cnt[0] += 1.0;
			}
			if in_right {
				acc[1] += rgb;
				cnt[1] += 1.0;
			}
			if in_top {
				acc[2] += rgb;
				cnt[2] += 1.0;
			}
			if in_bottom {
				acc[3] += rgb;
				cnt[3] += 1.0;
			}
		}
	}

	let norm = |i: usize| -> Vec3 {
		if cnt[i] > 0.0 {
			(acc[i] / cnt[i]).clamp(Vec3::ZERO, Vec3::ONE)
		} else {
			Vec3::splat(0.5)
		}
	};
	Some(AmbientColors {
		left: norm(0),
		right: norm(1),
		top: norm(2),
		bottom: norm(3),
	})
}

#[cfg(windows)]
mod dxgi {
	use super::{average_edge_bands, AmbientColors};
	use std::ptr::null_mut;
	use windows::core::Interface;
	use windows::Win32::Graphics::Direct3D::{
		D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
	};
	use windows::Win32::Graphics::Direct3D11::{
		D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
		D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
		D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
	};
	use windows::Win32::Graphics::Dxgi::Common::{
		DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
	};
	use windows::Win32::Graphics::Dxgi::{
		IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT,
		DXGI_OUTDUPL_FRAME_INFO,
	};

	pub(super) enum DxgiSample {
		Colors(AmbientColors),
		Unchanged,
		Unavailable,
	}

	#[derive(Default)]
	pub(super) struct DxgiState {
		dupl: Option<DxgiDuplicator>,
		disabled: bool,
	}

	struct DxgiDuplicator {
		device: ID3D11Device,
		ctx: ID3D11DeviceContext,
		dupl: IDXGIOutputDuplication,
		staging: Option<(u32, u32, DXGI_FORMAT, ID3D11Texture2D)>,
		has_frame: bool,
	}

	// SAFETY: D3D11 device/context/duplication COM interfaces are thread-safe when serialized behind `Mutex`.
	unsafe impl Send for DxgiDuplicator {}

	impl DxgiDuplicator {
		fn create() -> Option<Self> {
			let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
			let mut device: Option<ID3D11Device> = None;
			let mut ctx: Option<ID3D11DeviceContext> = None;
			unsafe {
				D3D11CreateDevice(
					None,
					D3D_DRIVER_TYPE_HARDWARE,
					None,
					D3D11_CREATE_DEVICE_BGRA_SUPPORT,
					Some(&levels),
					D3D11_SDK_VERSION,
					Some(&mut device),
					None,
					Some(&mut ctx),
				)
				.ok()?;
				let device = device?;
				let ctx = ctx?;
				let dxgi_dev: IDXGIDevice = device.cast().ok()?;
				let adapter = dxgi_dev.GetAdapter().ok()?;
				let output = adapter.EnumOutputs(0).ok()?;
				let output1: IDXGIOutput1 = output.cast().ok()?;
				let dupl = output1.DuplicateOutput(&device).ok()?;
				Some(Self {
					device,
					ctx,
					dupl,
					staging: None,
					has_frame: false,
				})
			}
		}

		fn acquire_and_sample(&mut self) -> Result<Option<AmbientColors>, windows::core::Error> {
			unsafe {
				let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
				let mut resource = None;
				let res = self.dupl.AcquireNextFrame(0, &mut info, &mut resource);
				if let Err(err) = res {
					if err.code() == DXGI_ERROR_WAIT_TIMEOUT {
						return Ok(None);
					}
					return Err(err);
				}
				struct ReleaseGuard<'a>(&'a IDXGIOutputDuplication);
				impl Drop for ReleaseGuard<'_> {
					fn drop(&mut self) {
						unsafe {
							let _ = self.0.ReleaseFrame();
						}
					}
				}
				let _guard = ReleaseGuard(&self.dupl);

				let Some(res_obj) = resource else {
					return Ok(None);
				};
				if info.LastPresentTime == 0 && self.has_frame {
					return Ok(None);
				}
				let tex: ID3D11Texture2D = res_obj.cast()?;
				let mut desc = D3D11_TEXTURE2D_DESC::default();
				tex.GetDesc(&mut desc);
				let is_bgra = match desc.Format {
					DXGI_FORMAT_B8G8R8A8_UNORM => true,
					DXGI_FORMAT_R8G8B8A8_UNORM => false,
					_ => return Ok(None),
				};

				let need_new = match self.staging {
					Some((w, h, fmt, _)) => w != desc.Width || h != desc.Height || fmt != desc.Format,
					None => true,
				};
				if need_new {
					let st_desc = D3D11_TEXTURE2D_DESC {
						Width: desc.Width,
						Height: desc.Height,
						MipLevels: 1,
						ArraySize: 1,
						Format: desc.Format,
						SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
						Usage: D3D11_USAGE_STAGING,
						BindFlags: 0,
						CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
						MiscFlags: 0,
					};
					let mut st_tex: Option<ID3D11Texture2D> = None;
					self.device.CreateTexture2D(&st_desc, Some(null_mut()), Some(&mut st_tex))?;
					if let Some(t) = st_tex {
						self.staging = Some((desc.Width, desc.Height, desc.Format, t));
					}
				}

				let Some((_, _, _, ref st_tex)) = self.staging else {
					return Ok(None);
				};
				self.ctx.CopyResource(st_tex, &tex);

				let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
				self.ctx.Map(st_tex, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
				let total = (mapped.RowPitch as usize).saturating_mul(desc.Height as usize);
				let colors = if !mapped.pData.is_null() && total >= (desc.Width as usize) * 4 {
					let slice = std::slice::from_raw_parts(mapped.pData as *const u8, total);
					average_edge_bands(slice, desc.Width as usize, desc.Height as usize, mapped.RowPitch as usize, is_bgra)
				} else {
					None
				};
				self.ctx.Unmap(st_tex, 0);
				if colors.is_some() {
					self.has_frame = true;
				}
				Ok(colors)
			}
		}
	}

	impl DxgiState {
		pub(super) fn sample_edge_colors(&mut self) -> DxgiSample {
			if self.disabled {
				return DxgiSample::Unavailable;
			}
			if self.dupl.is_none() {
				match DxgiDuplicator::create() {
					Some(d) => self.dupl = Some(d),
					None => {
						self.disabled = true;
						return DxgiSample::Unavailable;
					}
				}
			}
			let Some(ref mut d) = self.dupl else {
				return DxgiSample::Unavailable;
			};
			match d.acquire_and_sample() {
				Ok(Some(c)) => DxgiSample::Colors(c),
				Ok(None) => DxgiSample::Unchanged,
				Err(err) => {
					self.dupl = None;
					if err.code() != DXGI_ERROR_ACCESS_LOST {
						self.disabled = true;
					}
					DxgiSample::Unavailable
				}
			}
		}
	}
}

#[cfg(windows)]
fn sample_screen_edge_colors_gdi() -> Option<AmbientColors> {
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
			let g = GRID as usize;
			let bytes = std::slice::from_raw_parts(bits_ptr as *const u8, g * g * 4);
			average_edge_bands(bytes, g, g, g * 4, true)
		} else {
			None
		};

		SelectObject(mem_dc, old_obj);
		let _ = DeleteObject(bmp);
		result
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn average_edge_bands_extracts_distinct_directional_bgra_edges() {
		let (w, h) = (32usize, 32usize);
		let mut buf = vec![0u8; w * h * 4];
		for y in 0..h {
			for x in 0..w {
				let i = (y * w + x) * 4;
				// BGRA layout: left=blue, right=red, top=green, bottom=white
				let (r, g, b) = if x < 4 {
					(0, 0, 255)
				} else if x >= w - 4 {
					(255, 0, 0)
				} else if y < 4 {
					(0, 255, 0)
				} else if y >= h - 4 {
					(255, 255, 255)
				} else {
					(10, 10, 10)
				};
				buf[i] = b;
				buf[i + 1] = g;
				buf[i + 2] = r;
				buf[i + 3] = 255;
			}
		}
		let cols = average_edge_bands(&buf, w, h, w * 4, true).expect("edge colors");
		assert!(cols.left.z > 0.7 && cols.left.x < 0.3, "left should be blue: {:?}", cols.left);
		assert!(cols.right.x > 0.7 && cols.right.z < 0.3, "right should be red: {:?}", cols.right);
		assert!(cols.top.y > 0.5, "top should be green-dominant: {:?}", cols.top);
		assert!(cols.bottom.x > 0.5 && cols.bottom.y > 0.5, "bottom should be bright: {:?}", cols.bottom);
	}

	#[test]
	fn screen_sync_sampler_returns_finite_normalized_colors() {
		let mut sampler = AmbientColorSampler::new(AmbientTheme::ScreenSync);
		let c = sampler.sample();
		for v in [c.left, c.right, c.top, c.bottom] {
			assert!(v.is_finite());
			assert!(v.x >= 0.0 && v.x <= 1.0 && v.y >= 0.0 && v.y <= 1.0 && v.z >= 0.0 && v.z <= 1.0);
		}
	}
}

//! Pure-Rust Windows Spout2 shared-texture sender/receiver via DX11 + named shared memory.
//! Matches `leadedge/Spout2` v2.007 binary wire layout (`SharedTextureInfo` = 280 bytes).

use std::collections::BTreeSet;

use super::{FrameBuffer, FrameSink, FrameSource, PixelFormat, TransportError};

pub const SPOUT_MAX_SENDER_NAME_LEN: usize = 256;
pub const SPOUT_DEFAULT_MAX_SENDERS: usize = 64;
pub const SPOUT_SENDER_NAMES_MAP: &str = "SpoutSenderNames";
pub const SPOUT_ACTIVE_SENDER_MAP: &str = "ActiveSenderName";

pub const DXGI_FORMAT_R8G8B8A8_UNORM_RAW: u32 = 28;
pub const DXGI_FORMAT_B8G8R8A8_UNORM_RAW: u32 = 87;

/// Binary-exact C++ `SpoutSenderNames.h` `SharedTextureInfo` (280 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SharedTextureInfo {
	pub share_handle: u32,
	pub width: u32,
	pub height: u32,
	pub format: u32,
	pub usage: u32,
	pub description: [u8; 256],
	pub partner_id: u32,
}

impl Default for SharedTextureInfo {
	fn default() -> Self {
		Self {
			share_handle: 0,
			width: 0,
			height: 0,
			format: DXGI_FORMAT_B8G8R8A8_UNORM_RAW,
			usage: 0,
			description: [0u8; 256],
			partner_id: 0,
		}
	}
}

/// Parse packed 256-byte NUL-terminated slots matching `spoutSenderNames::readSenderSetFromBuffer`.
pub fn parse_sender_names_buffer(buf: &[u8]) -> BTreeSet<String> {
	let mut set = BTreeSet::new();
	let max_senders = buf.len() / SPOUT_MAX_SENDER_NAME_LEN;
	for i in 0..max_senders {
		let slot = &buf[i * SPOUT_MAX_SENDER_NAME_LEN..(i + 1) * SPOUT_MAX_SENDER_NAME_LEN];
		if slot[0] == 0 {
			break;
		}
		let end = slot.iter().position(|&b| b == 0).unwrap_or(slot.len());
		let name = String::from_utf8_lossy(&slot[..end]).into_owned();
		if !name.is_empty() {
			set.insert(name);
		}
	}
	set
}

/// Serialize sender names into packed 256-byte slots matching `spoutSenderNames::writeBufferFromSenderSet`.
pub fn write_sender_names_buffer(names: &BTreeSet<String>, buf: &mut [u8]) {
	let max_senders = buf.len() / SPOUT_MAX_SENDER_NAME_LEN;
	let mut i = 0usize;
	for name in names {
		if i >= max_senders {
			break;
		}
		let slot = &mut buf[i * SPOUT_MAX_SENDER_NAME_LEN..(i + 1) * SPOUT_MAX_SENDER_NAME_LEN];
		slot.fill(0);
		let bytes = name.as_bytes();
		let copy_len = bytes.len().min(SPOUT_MAX_SENDER_NAME_LEN - 1);
		slot[..copy_len].copy_from_slice(&bytes[..copy_len]);
		i += 1;
	}
	if i < max_senders {
		buf[i * SPOUT_MAX_SENDER_NAME_LEN] = 0;
	}
}

pub fn read_cstr_bytes(buf: &[u8]) -> String {
	let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
	String::from_utf8_lossy(&buf[..end]).into_owned()
}

pub fn write_cstr_bytes(dst: &mut [u8], s: &str) {
	if dst.is_empty() {
		return;
	}
	dst.fill(0);
	let b = s.as_bytes();
	let n = b.len().min(dst.len() - 1);
	dst[..n].copy_from_slice(&b[..n]);
}

#[cfg(windows)]
mod native {
	use super::*;
	use std::ffi::{c_void, CString};
	use std::mem::{size_of, zeroed};
	use windows::core::{Interface, PCSTR};
	use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, WAIT_ABANDONED, WAIT_OBJECT_0};
	use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
	use windows::Win32::Graphics::Direct3D11::{
		D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
		D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
		D3D11_RESOURCE_MISC_SHARED, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
		D3D11_USAGE_STAGING, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
	};
	use windows::Win32::Graphics::Dxgi::Common::{
		DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
	};
	use windows::Win32::Graphics::Dxgi::IDXGIResource;
	use windows::Win32::System::Memory::{
		CreateFileMappingA, MapViewOfFile, OpenFileMappingA, UnmapViewOfFile, VirtualQuery,
		FILE_MAP_ALL_ACCESS, FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS,
		PAGE_READWRITE,
	};
	use windows::Win32::System::Threading::{
		CreateMutexA, CreateSemaphoreA, ReleaseMutex, ReleaseSemaphore, WaitForSingleObject,
	};

	const SPOUT_WAIT_TIMEOUT_MS: u32 = 100;

	fn to_cstr(name: &str) -> Result<CString, TransportError> {
		CString::new(name)
			.map_err(|_| TransportError::Native(format!("Invalid interior NUL in name '{name}'")))
	}

	struct OwnedHandle(HANDLE);
	impl Drop for OwnedHandle {
		fn drop(&mut self) {
			if !self.0.is_invalid() {
				// SAFETY: Owned OS kernel handle created/opened via Win32 APIs and closed once here.
				unsafe {
					let _ = CloseHandle(self.0);
				}
			}
		}
	}

	struct SpoutSharedMemory {
		_map_handle: OwnedHandle,
		mutex_handle: Option<OwnedHandle>,
		view: MEMORY_MAPPED_VIEW_ADDRESS,
		size: usize,
	}

	// SAFETY: Spout shared memory is synchronized cross-process via named Win32 mutex before access.
	unsafe impl Send for SpoutSharedMemory {}

	impl Drop for SpoutSharedMemory {
		fn drop(&mut self) {
			if !self.view.Value.is_null() {
				// SAFETY: `self.view` came from `MapViewOfFile` and is unmapped once on drop.
				unsafe {
					let _ = UnmapViewOfFile(self.view);
				}
				self.view.Value = std::ptr::null_mut();
			}
		}
	}

	impl SpoutSharedMemory {
		fn create(name: &str, size: usize) -> Result<Self, TransportError> {
			let c_name = to_cstr(name)?;
			let c_mutex = to_cstr(&format!("{name}_mutex"))?;
			// SAFETY: Win32 paging-file-backed shared memory map & named synchronization mutex.
			unsafe {
				let h_map = CreateFileMappingA(
					HANDLE(-1isize as *mut c_void),
					None,
					PAGE_READWRITE,
					0,
					size as u32,
					PCSTR(c_name.as_ptr() as *const u8),
				)
				.map_err(|e| TransportError::Native(format!("CreateFileMappingA({name}): {e}")))?;

				let view = MapViewOfFile(h_map, FILE_MAP_ALL_ACCESS, 0, 0, 0);
				if view.Value.is_null() {
					let _ = CloseHandle(h_map);
					return Err(TransportError::Native(format!("MapViewOfFile({name}) failed")));
				}

				let mut mbi: MEMORY_BASIC_INFORMATION = zeroed();
				let q = VirtualQuery(
					Some(view.Value as *const c_void),
					&mut mbi,
					size_of::<MEMORY_BASIC_INFORMATION>(),
				);
				let actual_size = if q > 0 && mbi.RegionSize > 0 {
					mbi.RegionSize
				} else {
					size
				};

				let mutex_handle = CreateMutexA(None, false, PCSTR(c_mutex.as_ptr() as *const u8))
					.ok()
					.map(OwnedHandle);

				Ok(Self {
					_map_handle: OwnedHandle(h_map),
					mutex_handle,
					view,
					size: actual_size,
				})
			}
		}

		fn open(name: &str, writable: bool) -> Option<Self> {
			let c_name = to_cstr(name).ok()?;
			let c_mutex = to_cstr(&format!("{name}_mutex")).ok()?;
			let map_access = if writable {
				FILE_MAP_ALL_ACCESS
			} else {
				FILE_MAP_READ
			};
			// SAFETY: Opening existing named shared memory map & creating/opening named mutex.
			unsafe {
				let h_map = OpenFileMappingA(map_access.0, false, PCSTR(c_name.as_ptr() as *const u8)).ok()?;
				let view = MapViewOfFile(h_map, map_access, 0, 0, 0);
				if view.Value.is_null() {
					let _ = CloseHandle(h_map);
					return None;
				}
				let mut mbi: MEMORY_BASIC_INFORMATION = zeroed();
				let q = VirtualQuery(
					Some(view.Value as *const c_void),
					&mut mbi,
					size_of::<MEMORY_BASIC_INFORMATION>(),
				);
				if q == 0 || mbi.RegionSize == 0 {
					let _ = UnmapViewOfFile(view);
					let _ = CloseHandle(h_map);
					return None;
				}
				let mutex_handle = CreateMutexA(None, false, PCSTR(c_mutex.as_ptr() as *const u8))
					.ok()
					.map(OwnedHandle);
				Some(Self {
					_map_handle: OwnedHandle(h_map),
					mutex_handle,
					view,
					size: mbi.RegionSize,
				})
			}
		}

		fn with_lock<R>(&mut self, f: impl FnOnce(&mut [u8]) -> R) -> Option<R> {
			let mut locked = false;
			if let Some(ref mh) = self.mutex_handle {
				// SAFETY: Valid mutex handle acquired for `SPOUT_WAIT_TIMEOUT_MS`.
				let wait = unsafe { WaitForSingleObject(mh.0, SPOUT_WAIT_TIMEOUT_MS) };
				if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
					return None;
				}
				locked = true;
			}
			// SAFETY: View pointer is valid for `self.size` bytes and guarded by mutex.
			let slice = unsafe { std::slice::from_raw_parts_mut(self.view.Value as *mut u8, self.size) };
			let res = f(slice);
			if locked {
				if let Some(ref mh) = self.mutex_handle {
					// SAFETY: Mutex was acquired above by this thread.
					unsafe {
						let _ = ReleaseMutex(mh.0);
					}
				}
			}
			Some(res)
		}

		fn with_lock_read<R>(&self, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
			let mut locked = false;
			if let Some(ref mh) = self.mutex_handle {
				// SAFETY: Valid mutex handle acquired for `SPOUT_WAIT_TIMEOUT_MS`.
				let wait = unsafe { WaitForSingleObject(mh.0, SPOUT_WAIT_TIMEOUT_MS) };
				if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
					return None;
				}
				locked = true;
			}
			// SAFETY: View pointer is valid for `self.size` bytes and guarded by mutex.
			let slice = unsafe { std::slice::from_raw_parts(self.view.Value as *const u8, self.size) };
			let res = f(slice);
			if locked {
				if let Some(ref mh) = self.mutex_handle {
					// SAFETY: Mutex was acquired above by this thread.
					unsafe {
						let _ = ReleaseMutex(mh.0);
					}
				}
			}
			Some(res)
		}
	}

	fn create_d3d11_device() -> Result<(ID3D11Device, ID3D11DeviceContext), TransportError> {
		for driver_type in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
			let mut device: Option<ID3D11Device> = None;
			let mut context: Option<ID3D11DeviceContext> = None;
			// SAFETY: Standard D3D11 device initialization with BGRA support and optional WARP fallback.
			let hr = unsafe {
				D3D11CreateDevice(
					None,
					driver_type,
					HMODULE::default(),
					D3D11_CREATE_DEVICE_BGRA_SUPPORT,
					None,
					D3D11_SDK_VERSION,
					Some(&mut device),
					None,
					Some(&mut context),
				)
			};
			if hr.is_ok() {
				if let (Some(d), Some(c)) = (device, context) {
					return Ok((d, c));
				}
			}
		}
		Err(TransportError::Native(
			"Failed to create Direct3D 11 device (Hardware and WARP both failed)".to_owned(),
		))
	}

	pub fn list_spout_senders() -> Vec<String> {
		let Some(shm) = SpoutSharedMemory::open(SPOUT_SENDER_NAMES_MAP, false) else {
			return Vec::new();
		};
		shm.with_lock_read(|buf| parse_sender_names_buffer(buf).into_iter().collect())
			.unwrap_or_default()
	}

	pub fn get_active_spout_sender() -> Option<String> {
		let shm = SpoutSharedMemory::open(SPOUT_ACTIVE_SENDER_MAP, false)?;
		let s = shm.with_lock_read(read_cstr_bytes)?;
		if s.is_empty() {
			None
		} else {
			Some(s)
		}
	}

	struct AccessSync {
		mutex: Option<OwnedHandle>,
		semaphore: Option<OwnedHandle>,
	}

	impl AccessSync {
		fn new(sender_name: &str) -> Self {
			let mutex = to_cstr(&format!("{sender_name}_SpoutAccessMutex"))
				.ok()
				.and_then(|c| {
					// SAFETY: Named access mutex per `SpoutFrameCount`.
					unsafe { CreateMutexA(None, false, PCSTR(c.as_ptr() as *const u8)) }
						.ok()
						.map(OwnedHandle)
				});
			let semaphore = to_cstr(&format!("{sender_name}_Count_Semaphore"))
				.ok()
				.and_then(|c| {
					// SAFETY: Named frame-count semaphore initialized with count 1.
					unsafe { CreateSemaphoreA(None, 1, i32::MAX, PCSTR(c.as_ptr() as *const u8)) }
						.ok()
						.map(OwnedHandle)
				});
			Self { mutex, semaphore }
		}

		fn with_access<R>(&self, f: impl FnOnce() -> R) -> R {
			let mut locked = false;
			if let Some(ref mh) = self.mutex {
				// SAFETY: Lock sender access mutex around texture GPU read/write.
				let wait = unsafe { WaitForSingleObject(mh.0, SPOUT_WAIT_TIMEOUT_MS) };
				if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
					locked = true;
				}
			}
			let res = f();
			if locked {
				if let Some(ref mh) = self.mutex {
					// SAFETY: Release sender access mutex.
					unsafe {
						let _ = ReleaseMutex(mh.0);
					}
				}
			}
			res
		}

		fn pulse_new_frame(&self) {
			if let Some(ref sem) = self.semaphore {
				// SAFETY: Increment Spout frame-count semaphore matching `SetNewFrame`.
				unsafe {
					let wait = WaitForSingleObject(sem.0, 0);
					let count = if wait == WAIT_OBJECT_0 { 2 } else { 1 };
					let _ = ReleaseSemaphore(sem.0, count, None);
				}
			}
		}
	}

	pub struct Spout2Sender {
		sender_name: String,
		device: ID3D11Device,
		context: ID3D11DeviceContext,
		shared_tex: Option<ID3D11Texture2D>,
		width: u32,
		height: u32,
		share_handle: u32,
		info_shm: SpoutSharedMemory,
		names_shm: SpoutSharedMemory,
		active_shm: SpoutSharedMemory,
		sync: AccessSync,
	}

	// SAFETY: Sender methods take `&mut self`; D3D11 immediate context is used solely by the owning instance.
	unsafe impl Send for Spout2Sender {}

	impl Spout2Sender {
		pub fn new(sender_name: impl Into<String>) -> Result<Self, TransportError> {
			let sender_name = sender_name.into();
			if sender_name.is_empty() || sender_name.len() >= SPOUT_MAX_SENDER_NAME_LEN {
				return Err(TransportError::Native(format!(
					"Sender name '{sender_name}' must be 1..{} bytes",
					SPOUT_MAX_SENDER_NAME_LEN - 1
				)));
			}
			let (device, context) = create_d3d11_device()?;
			let info_shm = SpoutSharedMemory::create(&sender_name, size_of::<SharedTextureInfo>())?;
			let mut names_shm = SpoutSharedMemory::create(
				SPOUT_SENDER_NAMES_MAP,
				SPOUT_DEFAULT_MAX_SENDERS * SPOUT_MAX_SENDER_NAME_LEN,
			)?;
			let mut active_shm = SpoutSharedMemory::create(SPOUT_ACTIVE_SENDER_MAP, SPOUT_MAX_SENDER_NAME_LEN)?;

			names_shm.with_lock(|buf| {
				let mut set = parse_sender_names_buffer(buf);
				set.insert(sender_name.clone());
				write_sender_names_buffer(&set, buf);
			});
			active_shm.with_lock(|buf| write_cstr_bytes(buf, &sender_name));

			let sync = AccessSync::new(&sender_name);
			Ok(Self {
				sender_name,
				device,
				context,
				shared_tex: None,
				width: 0,
				height: 0,
				share_handle: 0,
				info_shm,
				names_shm,
				active_shm,
				sync,
			})
		}

		fn ensure_texture(&mut self, width: u32, height: u32) -> Result<(), TransportError> {
			if self.shared_tex.is_some() && self.width == width && self.height == height {
				return Ok(());
			}
			let desc = D3D11_TEXTURE2D_DESC {
				Width: width,
				Height: height,
				MipLevels: 1,
				ArraySize: 1,
				Format: DXGI_FORMAT_B8G8R8A8_UNORM,
				SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
				Usage: D3D11_USAGE_DEFAULT,
				BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
				CPUAccessFlags: 0,
				MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
			};
			let mut tex: Option<ID3D11Texture2D> = None;
			// SAFETY: Valid D3D11 device & texture descriptor.
			unsafe {
				self.device
					.CreateTexture2D(&desc, None, Some(&mut tex))
					.map_err(|e| TransportError::Native(format!("CreateTexture2D: {e}")))?;
			}
			let tex = tex.ok_or_else(|| TransportError::Native("CreateTexture2D returned null".to_owned()))?;
			let dxgi_res: IDXGIResource = tex
				.cast()
				.map_err(|e| TransportError::Native(format!("cast IDXGIResource: {e}")))?;
			// SAFETY: Query KMT shared handle from texture created with `D3D11_RESOURCE_MISC_SHARED`.
			let raw_handle = unsafe {
				dxgi_res
					.GetSharedHandle()
					.map_err(|e| TransportError::Native(format!("GetSharedHandle: {e}")))?
			};
			let share_handle = raw_handle.0 as usize as u32;
			if share_handle == 0 {
				return Err(TransportError::Native("GetSharedHandle returned 0".to_owned()));
			}

			let mut info = SharedTextureInfo {
				share_handle,
				width,
				height,
				format: DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
				usage: 0,
				description: [0u8; 256],
				partner_id: 0,
			};
			write_cstr_bytes(&mut info.description, "RigLensBooster");

			self.info_shm.with_lock(|buf| {
				if buf.len() >= size_of::<SharedTextureInfo>() {
					// SAFETY: `SharedTextureInfo` is `#[repr(C)]` plain data of 280 bytes.
					let bytes = unsafe {
						std::slice::from_raw_parts(
							(&info as *const SharedTextureInfo) as *const u8,
							size_of::<SharedTextureInfo>(),
						)
					};
					buf[..size_of::<SharedTextureInfo>()].copy_from_slice(bytes);
				}
			});

			self.shared_tex = Some(tex);
			self.width = width;
			self.height = height;
			self.share_handle = share_handle;
			Ok(())
		}
	}

	impl Drop for Spout2Sender {
		fn drop(&mut self) {
			let mut next_active = None;
			self.names_shm.with_lock(|buf| {
				let mut set = parse_sender_names_buffer(buf);
				set.remove(&self.sender_name);
				next_active = set.iter().next().cloned();
				write_sender_names_buffer(&set, buf);
			});
			self.active_shm.with_lock(|buf| {
				if read_cstr_bytes(buf) == self.sender_name {
					if let Some(ref alt) = next_active {
						write_cstr_bytes(buf, alt);
					} else {
						buf.fill(0);
					}
				}
			});
		}
	}

	impl FrameSink for Spout2Sender {
		fn name(&self) -> &str {
			&self.sender_name
		}

		fn send_frame(&mut self, frame: &FrameBuffer) -> Result<(), TransportError> {
			self.ensure_texture(frame.width, frame.height)?;
			let bgra = frame.to_bgra8();
			let tex = self
				.shared_tex
				.as_ref()
				.ok_or_else(|| TransportError::Native("Shared texture missing".to_owned()))?
				.clone();
			let row_pitch = frame.width * 4;
			self.sync.with_access(|| {
				// SAFETY: `bgra` has exact `width * height * 4` bytes matching `tex` dimensions.
				unsafe {
					self.context.UpdateSubresource(
						&tex,
						0,
						None,
						bgra.as_ptr() as *const c_void,
						row_pitch,
						0,
					);
					self.context.Flush();
				}
				self.sync.pulse_new_frame();
			});
			Ok(())
		}
	}

	pub struct Spout2Receiver {
		requested_sender: String,
		active_name: String,
		device: ID3D11Device,
		context: ID3D11DeviceContext,
		shared_tex: Option<ID3D11Texture2D>,
		staging_tex: Option<ID3D11Texture2D>,
		last_handle: u32,
		last_width: u32,
		last_height: u32,
		last_format: u32,
	}

	// SAFETY: Receiver owns its own D3D11 device & context and synchronizes reads via named mutex.
	unsafe impl Send for Spout2Receiver {}

	impl Spout2Receiver {
		/// Open a receiver targeting `sender_name` (empty string tracks Spout's active sender).
		pub fn new(sender_name: impl Into<String>) -> Result<Self, TransportError> {
			let requested_sender = sender_name.into();
			let (device, context) = create_d3d11_device()?;
			Ok(Self {
				active_name: requested_sender.clone(),
				requested_sender,
				device,
				context,
				shared_tex: None,
				staging_tex: None,
				last_handle: 0,
				last_width: 0,
				last_height: 0,
				last_format: 0,
			})
		}

		fn resolve_sender_name(&mut self) -> Option<String> {
			if !self.requested_sender.is_empty() {
				self.active_name = self.requested_sender.clone();
				return Some(self.requested_sender.clone());
			}
			if let Some(active) = get_active_spout_sender() {
				self.active_name = active.clone();
				return Some(active);
			}
			let list = list_spout_senders();
			let first = list.into_iter().next();
			if let Some(ref f) = first {
				self.active_name = f.clone();
			}
			first
		}

		fn ensure_shared_resources(&mut self, info: &SharedTextureInfo) -> Result<(), TransportError> {
			if self.shared_tex.is_some()
				&& self.staging_tex.is_some()
				&& self.last_handle == info.share_handle
				&& self.last_width == info.width
				&& self.last_height == info.height
				&& self.last_format == info.format
			{
				return Ok(());
			}

			let handle = HANDLE(info.share_handle as usize as *mut c_void);
			let mut opened: Option<ID3D11Texture2D> = None;
			// SAFETY: Opening valid cross-process DX11 shared handle from `SharedTextureInfo`.
			unsafe {
				self.device
					.OpenSharedResource(handle, &mut opened)
					.map_err(|e| TransportError::Native(format!("OpenSharedResource: {e}")))?;
			}
			let shared = opened
				.ok_or_else(|| TransportError::Native("OpenSharedResource returned null".to_owned()))?;

			let dxgi_fmt = match info.format {
				DXGI_FORMAT_B8G8R8A8_UNORM_RAW | 0 => DXGI_FORMAT_B8G8R8A8_UNORM,
				DXGI_FORMAT_R8G8B8A8_UNORM_RAW => DXGI_FORMAT_R8G8B8A8_UNORM,
				other => DXGI_FORMAT(other as i32),
			};

			let staging_desc = D3D11_TEXTURE2D_DESC {
				Width: info.width,
				Height: info.height,
				MipLevels: 1,
				ArraySize: 1,
				Format: dxgi_fmt,
				SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
				Usage: D3D11_USAGE_STAGING,
				BindFlags: 0,
				CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
				MiscFlags: 0,
			};
			let mut staging: Option<ID3D11Texture2D> = None;
			// SAFETY: Creating CPU-readable staging texture matching shared texture format & dimensions.
			unsafe {
				self.device
					.CreateTexture2D(&staging_desc, None, Some(&mut staging))
					.map_err(|e| TransportError::Native(format!("CreateTexture2D(staging): {e}")))?;
			}

			self.shared_tex = Some(shared);
			self.staging_tex = staging;
			self.last_handle = info.share_handle;
			self.last_width = info.width;
			self.last_height = info.height;
			self.last_format = info.format;
			Ok(())
		}
	}

	impl FrameSource for Spout2Receiver {
		fn name(&self) -> &str {
			&self.active_name
		}

		fn recv_frame(&mut self) -> Result<Option<FrameBuffer>, TransportError> {
			let Some(target_name) = self.resolve_sender_name() else {
				return Ok(None);
			};
			let Some(shm) = SpoutSharedMemory::open(&target_name, false) else {
				return Ok(None);
			};
			let info: Option<SharedTextureInfo> = shm
				.with_lock_read(|buf| {
					if buf.len() < size_of::<SharedTextureInfo>() {
						return None;
					}
					// SAFETY: `SharedTextureInfo` is `#[repr(C)]` plain value type; read unaligned copy safely.
					Some(unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const SharedTextureInfo) })
				})
				.flatten();
			let Some(info) = info else {
				return Ok(None);
			};
			if info.share_handle == 0 || info.width == 0 || info.height == 0 {
				return Ok(None);
			}
			let pixel_format = match info.format {
				DXGI_FORMAT_B8G8R8A8_UNORM_RAW | 0 => PixelFormat::Bgra8,
				DXGI_FORMAT_R8G8B8A8_UNORM_RAW => PixelFormat::Rgba8,
				other => return Err(TransportError::UnsupportedFormat(other)),
			};

			self.ensure_shared_resources(&info)?;
			let shared = self.shared_tex.as_ref().unwrap().clone();
			let staging = self.staging_tex.as_ref().unwrap().clone();

			let sync = AccessSync::new(&target_name);
			let copy_res: Result<Vec<u8>, TransportError> = sync.with_access(|| {
				// SAFETY: Copy from shared GPU texture to staging under mutex, map for read, copy rows.
				unsafe {
					self.context.CopyResource(&staging, &shared);
					let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
					self.context
						.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
						.map_err(|e| TransportError::Native(format!("Map(staging): {e}")))?;

					let w = info.width as usize;
					let h = info.height as usize;
					let row_bytes = w * 4;
					let pitch = mapped.RowPitch as usize;
					let mut out = vec![0u8; row_bytes * h];
					let src_base = mapped.pData as *const u8;
					for y in 0..h {
						let src_row = std::slice::from_raw_parts(src_base.add(y * pitch), row_bytes);
						out[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(src_row);
					}
					self.context.Unmap(&staging, 0);
					Ok(out)
				}
			});
			let raw_bytes = copy_res?;
			let mut fb = FrameBuffer::new(info.width, info.height, pixel_format, raw_bytes)?;
			if fb.format == PixelFormat::Bgra8 {
				super::super::swizzle_rgba_bgra(&mut fb.data);
				fb.format = PixelFormat::Rgba8;
			}
			Ok(Some(fb))
		}
	}
}

#[cfg(not(windows))]
mod native {
	use super::*;

	pub fn list_spout_senders() -> Vec<String> {
		Vec::new()
	}

	pub fn get_active_spout_sender() -> Option<String> {
		None
	}

	pub struct Spout2Sender {
		name: String,
	}

	impl Spout2Sender {
		pub fn new(sender_name: impl Into<String>) -> Result<Self, TransportError> {
			let _ = sender_name.into();
			Err(TransportError::UnsupportedPlatform)
		}
	}

	impl FrameSink for Spout2Sender {
		fn name(&self) -> &str {
			&self.name
		}
		fn send_frame(&mut self, _frame: &FrameBuffer) -> Result<(), TransportError> {
			Err(TransportError::UnsupportedPlatform)
		}
	}

	pub struct Spout2Receiver {
		name: String,
	}

	impl Spout2Receiver {
		pub fn new(sender_name: impl Into<String>) -> Result<Self, TransportError> {
			let _ = sender_name.into();
			Err(TransportError::UnsupportedPlatform)
		}
	}

	impl FrameSource for Spout2Receiver {
		fn name(&self) -> &str {
			&self.name
		}
		fn recv_frame(&mut self) -> Result<Option<FrameBuffer>, TransportError> {
			Err(TransportError::UnsupportedPlatform)
		}
	}
}

pub use native::{get_active_spout_sender, list_spout_senders, Spout2Receiver, Spout2Sender};

#[cfg(test)]
mod tests {
	use super::*;
	use crate::transport::MemoryFrameBus;

	#[test]
	fn shared_texture_info_is_280_bytes() {
		assert_eq!(std::mem::size_of::<SharedTextureInfo>(), 280);
	}

	#[test]
	fn sender_names_buffer_roundtrip_and_selective_removal() {
		let mut buf = vec![0u8; SPOUT_DEFAULT_MAX_SENDERS * SPOUT_MAX_SENDER_NAME_LEN];
		let mut set = BTreeSet::new();
		set.insert("VTubeStudioSpout".to_owned());
		set.insert("RigLensBooster".to_owned());
		set.insert("OtherSender".to_owned());
		write_sender_names_buffer(&set, &mut buf);

		let parsed = parse_sender_names_buffer(&buf);
		assert_eq!(parsed, set);

		// Remove middle entry and verify contiguous packing with no early NUL termination gap
		set.remove("RigLensBooster");
		write_sender_names_buffer(&set, &mut buf);
		let parsed2 = parse_sender_names_buffer(&buf);
		assert_eq!(parsed2, set);
	}

	#[test]
	fn memory_frame_bus_swizzle_and_roundtrip() {
		let mut bus = MemoryFrameBus::new("TestBus");
		let frame = FrameBuffer::new(
			2,
			2,
			PixelFormat::Rgba8,
			vec![
				255, 0, 0, 255, // Red opaque
				0, 255, 0, 128, // Green semi
				0, 0, 255, 0,   // Blue transparent
				10, 20, 30, 255,
			],
		)
		.unwrap();
		let bgra = frame.to_bgra8();
		assert_eq!(&bgra[0..4], &[0, 0, 255, 255]);
		bus.send_frame(&frame).unwrap();
		let got = bus.recv_frame().unwrap().unwrap();
		assert_eq!(got, frame);
	}

	#[cfg(windows)]
	#[test]
	fn spout2_native_dx11_sender_receiver_loopback() {
		let unique_name = format!("RigLensLoopback_{}", std::process::id());
		{
			let mut sender = Spout2Sender::new(&unique_name).expect("create DX11 Spout2 sender");
			let mut receiver = Spout2Receiver::new(&unique_name).expect("create DX11 Spout2 receiver");

			assert!(
				list_spout_senders().iter().any(|s| s == &unique_name),
				"expected sender '{unique_name}' in list: {:?}",
				list_spout_senders()
			);

			let w = 64u32;
			let h = 64u32;
			let mut rgba = Vec::with_capacity((w * h * 4) as usize);
			for y in 0..h {
				for x in 0..w {
					rgba.push(((x * 4) & 255) as u8);
					rgba.push(((y * 4) & 255) as u8);
					rgba.push(180);
					rgba.push(if x < 16 && y < 16 { 0 } else { 220 });
				}
			}
			let frame = FrameBuffer::new(w, h, PixelFormat::Rgba8, rgba.clone()).unwrap();
			sender.send_frame(&frame).expect("send DX11 frame");

			let received = receiver
				.recv_frame()
				.expect("recv DX11 frame")
				.expect("expected Some(frame) from Spout2Receiver");
			assert_eq!(received.width, w);
			assert_eq!(received.height, h);
			assert_eq!(received.format, PixelFormat::Rgba8);
			assert_eq!(received.data, rgba, "RGBA+Alpha mismatch across DX11 Spout2 loopback");
		}

		assert!(
			!list_spout_senders().iter().any(|s| s == &unique_name),
			"expected '{unique_name}' unregistered after Drop"
		);
	}
}

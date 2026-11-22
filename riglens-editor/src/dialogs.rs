#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileDialogMode {
	ImportPsdProject,
	OpenModel,
	SyncPsdTextures,
	SaveInpAs,
	ExportRigJson,
	ExportPsd,
}

impl FileDialogMode {
	pub fn title_key(self) -> &'static str {
		match self {
			Self::ImportPsdProject => "dialog.import_psd_project.title",
			Self::OpenModel => "dialog.open_model.title",
			Self::SyncPsdTextures => "dialog.sync_psd.title",
			Self::SaveInpAs => "dialog.save_inp.title",
			Self::ExportRigJson => "dialog.export_rigjson.title",
			Self::ExportPsd => "dialog.export_psd.title",
		}
	}

	pub fn filter_key(self) -> &'static str {
		match self {
			Self::ImportPsdProject | Self::SyncPsdTextures | Self::ExportPsd => "dialog.filter.psd",
			Self::OpenModel => "dialog.filter.puppet",
			Self::SaveInpAs => "dialog.filter.inp",
			Self::ExportRigJson => "dialog.filter.rigjson",
		}
	}

	pub fn extensions(self) -> &'static [&'static str] {
		match self {
			Self::ImportPsdProject | Self::SyncPsdTextures | Self::ExportPsd => &["psd"],
			Self::OpenModel => &["psd", "inp", "inx", "rigjson"],
			Self::SaveInpAs => &["inp"],
			Self::ExportRigJson => &["rigjson"],
		}
	}

	pub fn default_filename(self) -> &'static str {
		match self {
			Self::ImportPsdProject | Self::OpenModel | Self::SyncPsdTextures => "",
			Self::SaveInpAs => "puppet_studio.inp",
			Self::ExportRigJson => "puppet_studio.rigjson",
			Self::ExportPsd => "puppet_studio.psd",
		}
	}

	pub fn is_save(self) -> bool {
		!matches!(self, Self::ImportPsdProject | Self::OpenModel | Self::SyncPsdTextures)
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileDialogResult {
	Cancelled,
	Confirmed(FileDialogMode, PathBuf),
}

pub fn ensure_save_extension(mode: FileDialogMode, path: PathBuf) -> PathBuf {
	if mode.is_save() {
		if let Some(req_ext) = mode.extensions().first() {
			let has_ext = path
				.extension()
				.and_then(|s| s.to_str())
				.is_some_and(|e| e.eq_ignore_ascii_case(req_ext));
			if !has_ext {
				let mut os = path.into_os_string();
				if !os.to_string_lossy().ends_with('.') {
					os.push(".");
				}
				os.push(req_ext);
				return PathBuf::from(os);
			}
		}
	}
	path
}

/// Opens the native OS file explorer picker / save dialog with localized title & filter label.
pub fn pick_native_file_localized(
	mode: FileDialogMode,
	title: &str,
	filter_name: &str,
	start_dir: &Path,
	suggested_name: Option<&str>,
) -> Option<PathBuf> {
	let mut dialog = rfd::FileDialog::new()
		.set_title(title)
		.add_filter(filter_name, mode.extensions());

	if start_dir.is_dir() {
		dialog = dialog.set_directory(start_dir);
	}

	if mode.is_save() {
		let name = suggested_name
			.filter(|s| !s.trim().is_empty())
			.unwrap_or_else(|| mode.default_filename());
		if !name.is_empty() {
			dialog = dialog.set_file_name(name);
		}
		dialog.save_file().map(|p| ensure_save_extension(mode, p))
	} else {
		dialog.pick_file()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn dialog_modes_and_save_extensions_match_contract() {
		assert_eq!(FileDialogMode::ImportPsdProject.extensions(), &["psd"]);
		assert!(!FileDialogMode::ImportPsdProject.is_save());
		assert_eq!(FileDialogMode::OpenModel.extensions(), &["psd", "inp", "inx", "rigjson"]);
		assert!(!FileDialogMode::OpenModel.is_save());
		assert!(FileDialogMode::SaveInpAs.is_save());
		assert!(FileDialogMode::ExportRigJson.is_save());
		assert!(FileDialogMode::ExportPsd.is_save());

		let resolved = ensure_save_extension(FileDialogMode::SaveInpAs, PathBuf::from("my_rig"));
		assert_eq!(resolved, PathBuf::from("my_rig.inp"));
		let dotted = ensure_save_extension(FileDialogMode::SaveInpAs, PathBuf::from("model.v2"));
		assert_eq!(dotted, PathBuf::from("model.v2.inp"));
		let unchanged = ensure_save_extension(FileDialogMode::ExportPsd, PathBuf::from("art.PSD"));
		assert_eq!(unchanged, PathBuf::from("art.PSD"));
	}
}

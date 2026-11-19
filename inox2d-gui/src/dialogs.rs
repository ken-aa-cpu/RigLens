#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

use egui::{self, Align2, Key, Sense, Vec2};

use crate::theme::{ACCENT_BLUE, AUDIT_ERR_ROSE, AUDIT_WARN_AMBER, BG_INPUT, TEXT_MUTED, TEXT_PRIMARY};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileDialogMode {
	OpenModel,
	SaveInpAs,
	ExportRigJson,
	ExportPsd,
}

impl FileDialogMode {
	pub fn title(self) -> &'static str {
		match self {
			Self::OpenModel => "Open / Import Puppet (.inp, .inx, .psd, .rigjson)",
			Self::SaveInpAs => "Save Puppet (.inp)",
			Self::ExportRigJson => "Export RigLens IR (.rigjson)",
			Self::ExportPsd => "Export Layered Artwork (.psd)",
		}
	}

	pub fn extensions(self) -> &'static [&'static str] {
		match self {
			Self::OpenModel => &["inp", "inx", "psd", "rigjson"],
			Self::SaveInpAs => &["inp"],
			Self::ExportRigJson => &["rigjson"],
			Self::ExportPsd => &["psd"],
		}
	}

	pub fn default_filename(self) -> &'static str {
		match self {
			Self::OpenModel => "",
			Self::SaveInpAs => "puppet_studio.inp",
			Self::ExportRigJson => "puppet_studio.rigjson",
			Self::ExportPsd => "puppet_studio.psd",
		}
	}

	pub fn is_save(self) -> bool {
		!matches!(self, Self::OpenModel)
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileDialogResult {
	Pending,
	Cancelled,
	Confirmed(FileDialogMode, PathBuf),
}

pub struct FileDialogState {
	pub mode: FileDialogMode,
	pub current_dir: PathBuf,
	pub source_dir: Option<PathBuf>,
	pub filename: String,
	pub entries: Vec<(String, PathBuf, bool)>,
	pub error: Option<String>,
}

impl FileDialogState {
	pub fn open(mode: FileDialogMode, start_dir: PathBuf, suggested_name: Option<&str>) -> Self {
		let dir = if start_dir.is_dir() {
			start_dir
		} else {
			std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
		};
		let entries = filter_entries(&dir, mode.extensions()).unwrap_or_default();
		Self {
			mode,
			current_dir: dir.clone(),
			source_dir: Some(dir),
			filename: suggested_name.unwrap_or(mode.default_filename()).to_owned(),
			entries,
			error: None,
		}
	}

	pub fn refresh(&mut self) {
		match filter_entries(&self.current_dir, self.mode.extensions()) {
			Ok(list) => {
				self.entries = list;
				self.error = None;
			}
			Err(e) => {
				self.entries.clear();
				self.error = Some(e);
			}
		}
	}

	pub fn resolve_target(&self) -> Option<PathBuf> {
		let trimmed = self.filename.trim();
		if trimmed.is_empty() {
			return None;
		}
		let candidate = Path::new(trimmed);
		let mut path = if candidate.is_absolute() {
			candidate.to_path_buf()
		} else {
			self.current_dir.join(candidate)
		};
		if self.mode.is_save() {
			if let Some(req_ext) = self.mode.extensions().first() {
				let has_ext = path
					.extension()
					.and_then(|s| s.to_str())
					.is_some_and(|e| e.eq_ignore_ascii_case(req_ext));
				if !has_ext {
					path.set_extension(req_ext);
				}
			}
		}
		Some(path)
	}
}

/// Read directory entries matching `extensions` (empty = all files). Directories come first.
pub fn filter_entries(dir: &Path, extensions: &[&str]) -> Result<Vec<(String, PathBuf, bool)>, String> {
	let rd = fs::read_dir(dir).map_err(|e| format!("Cannot read '{}': {e}", dir.display()))?;
	let mut dirs = Vec::new();
	let mut files = Vec::new();
	for entry in rd.flatten() {
		let path = entry.path();
		let name = entry.file_name().to_string_lossy().to_string();
		let is_dir = path.is_dir();
		if is_dir {
			dirs.push((name, path, true));
		} else {
			let keep = extensions.is_empty()
				|| path
					.extension()
					.and_then(|s| s.to_str())
					.is_some_and(|ext| extensions.iter().any(|e| ext.eq_ignore_ascii_case(e)));
			if keep {
				files.push((name, path, false));
			}
		}
	}
	dirs.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
	files.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
	dirs.extend(files);
	Ok(dirs)
}

pub fn draw_file_dialog(ctx: &egui::Context, state: &mut FileDialogState) -> FileDialogResult {
	let mut open = true;
	let mut result = FileDialogResult::Pending;

	if ctx.input(|i| i.key_pressed(Key::Escape)) {
		return FileDialogResult::Cancelled;
	}

	egui::Window::new(state.mode.title())
		.open(&mut open)
		.collapsible(false)
		.resizable(true)
		.default_size([560.0, 420.0])
		.anchor(Align2::CENTER_CENTER, Vec2::ZERO)
		.show(ctx, |ui| {
			ui.horizontal(|ui| {
				if ui.small_button("Up (..)").clicked() {
					if let Some(parent) = state.current_dir.parent() {
						state.current_dir = parent.to_path_buf();
						state.refresh();
					}
				}
				if ui.small_button("CWD").clicked() {
					if let Ok(cwd) = std::env::current_dir() {
						state.current_dir = cwd;
						state.refresh();
					}
				}
				if let Some(ref sdir) = state.source_dir.clone() {
					if ui.small_button("Start Dir").clicked() {
						state.current_dir = sdir.clone();
						state.refresh();
					}
				}
				if ui.small_button("Refresh").clicked() {
					state.refresh();
				}
			});

			let dir_str = state.current_dir.display().to_string();
			ui.label(egui::RichText::new(dir_str).monospace().size(10.5).color(ACCENT_BLUE));
			ui.separator();

			if let Some(ref err) = state.error {
				ui.label(egui::RichText::new(err).color(AUDIT_ERR_ROSE).size(11.0));
			}

			let mut nav_to: Option<PathBuf> = None;
			let mut picked_file: Option<String> = None;
			let mut double_confirmed = false;

			egui::Frame::none().fill(BG_INPUT).show(ui, |ui| {
				egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
					ui.set_min_width(520.0);
					for (name, path, is_dir) in &state.entries {
						let prefix = if *is_dir { "[DIR]" } else { "     " };
						let col = if *is_dir { AUDIT_WARN_AMBER } else { TEXT_PRIMARY };
						let selected = !*is_dir && state.filename == *name;
						let resp = ui.add(
							egui::SelectableLabel::new(
								selected,
								egui::RichText::new(format!("{prefix} {name}")).monospace().size(11.5).color(col),
							),
						);
						if resp.clicked() {
							if *is_dir {
								nav_to = Some(path.clone());
							} else {
								picked_file = Some(name.clone());
							}
						}
						if resp.double_clicked() && !*is_dir {
							picked_file = Some(name.clone());
							double_confirmed = true;
						}
					}
					if state.entries.is_empty() {
						ui.label(
							egui::RichText::new("No matching files in this directory.")
								.size(11.0)
								.color(TEXT_MUTED),
						);
					}
				});
			});

			if let Some(d) = nav_to {
				state.current_dir = d;
				state.refresh();
			}
			if let Some(f) = picked_file {
				state.filename = f;
			}

			ui.separator();
			let mut enter_pressed = false;
			ui.horizontal(|ui| {
				ui.label("File:");
				let te = ui.add_sized([340.0, 22.0], egui::TextEdit::singleline(&mut state.filename));
				if te.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
					enter_pressed = true;
				}
				let btn_lbl = if state.mode.is_save() { "Save / Export" } else { "Open" };
				if ui.add(egui::Button::new(btn_lbl).sense(Sense::click())).clicked()
					|| double_confirmed
					|| enter_pressed
				{
					if let Some(target) = state.resolve_target() {
						if target.is_dir() {
							state.current_dir = target;
							state.filename.clear();
							state.refresh();
						} else if !state.mode.is_save() && !target.is_file() {
							state.error = Some(format!("File not found: {}", target.display()));
						} else {
							result = FileDialogResult::Confirmed(state.mode, target);
						}
					} else {
						state.error = Some("Enter or pick a filename".to_owned());
					}
				}
				if ui.button("Cancel").clicked() {
					result = FileDialogResult::Cancelled;
				}
			});
		});

	if !open && result == FileDialogResult::Pending {
		return FileDialogResult::Cancelled;
	}
	result
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn dialog_filter_lists_dirs_first_and_filters_extensions() {
		let dir = std::env::temp_dir().join(format!("riglens_dialog_test_{}", std::process::id()));
		let _ = fs::remove_dir_all(&dir);
		fs::create_dir_all(dir.join("sub_folder")).unwrap();
		fs::write(dir.join("alpha.inp"), b"a").unwrap();
		fs::write(dir.join("beta.PSD"), b"b").unwrap();
		fs::write(dir.join("ignore.txt"), b"c").unwrap();

		let entries = filter_entries(&dir, FileDialogMode::OpenModel.extensions()).unwrap();
		assert_eq!(entries.len(), 3);
		assert!(entries[0].2 && entries[0].0 == "sub_folder");
		assert!(!entries[1].2 && entries[1].0 == "alpha.inp");
		assert!(!entries[2].2 && entries[2].0 == "beta.PSD");

		let state = FileDialogState::open(FileDialogMode::SaveInpAs, dir.clone(), Some("my_rig"));
		assert_eq!(state.resolve_target().unwrap(), dir.join("my_rig.inp"));

		let _ = fs::remove_dir_all(&dir);
	}
}

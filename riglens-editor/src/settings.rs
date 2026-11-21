#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserSettings {
	pub language: String,
	pub workspace_mode: String,
}

impl Default for UserSettings {
	fn default() -> Self {
		Self {
			language: "en".to_owned(),
			workspace_mode: "quick".to_owned(),
		}
	}
}

pub fn settings_path() -> PathBuf {
	if let Some(p) = std::env::var_os("RIGLENS_SETTINGS_PATH") {
		if !p.is_empty() {
			return PathBuf::from(p);
		}
	}
	crate::project::default_projects_root().join("settings.json")
}

pub fn load_settings_from(path: &Path) -> UserSettings {
	let Ok(raw) = fs::read_to_string(path) else {
		return UserSettings::default();
	};
	let Ok(val) = json::parse(&raw) else {
		return UserSettings::default();
	};
	UserSettings {
		language: val["language"].as_str().unwrap_or("en").to_owned(),
		workspace_mode: val["workspace_mode"].as_str().unwrap_or("quick").to_owned(),
	}
}

pub fn save_settings_to(path: &Path, settings: &UserSettings) -> Result<(), String> {
	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent).map_err(|e| format!("create settings dir: {e}"))?;
	}
	let obj = json::object! {
		language: settings.language.as_str(),
		workspace_mode: settings.workspace_mode.as_str(),
	};
	fs::write(path, json::stringify_pretty(obj, 2)).map_err(|e| format!("write settings: {e}"))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn settings_save_and_load_roundtrip() {
		let tmp = std::env::temp_dir().join(format!("riglens_settings_test_{}.json", std::process::id()));
		let _ = fs::remove_file(&tmp);
		let s = UserSettings {
			language: "zh-TW".to_owned(),
			workspace_mode: "pro".to_owned(),
		};
		save_settings_to(&tmp, &s).expect("save settings");
		let back = load_settings_from(&tmp);
		assert_eq!(back, s);
		let _ = fs::remove_file(&tmp);
	}
}

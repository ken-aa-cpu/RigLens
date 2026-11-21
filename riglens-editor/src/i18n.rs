#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
struct LocaleTable {
	display_name: String,
	entries: HashMap<String, String>,
}

/// JSON-backed multi-language registry with built-in embedded defaults (`en`, `zh-TW`, `ja`)
/// plus optional runtime discovery from an external `locales/` directory.
#[derive(Clone, Debug)]
pub struct I18n {
	current: String,
	tables: BTreeMap<String, LocaleTable>,
}

impl I18n {
	pub fn new(initial_lang: &str) -> Self {
		let mut i18n = Self {
			current: "en".to_owned(),
			tables: BTreeMap::new(),
		};
		i18n.insert_json_str("en", include_str!("../locales/en.json"));
		i18n.insert_json_str("zh-TW", include_str!("../locales/zh-TW.json"));
		i18n.insert_json_str("ja", include_str!("../locales/ja.json"));

		for dir in external_locale_dirs() {
			i18n.load_dir(&dir);
		}
		i18n.set_language(initial_lang);
		i18n
	}

	pub fn load_dir(&mut self, dir: &Path) {
		let Ok(rd) = fs::read_dir(dir) else {
			return;
		};
		for entry in rd.flatten() {
			let p = entry.path();
			if p.extension().and_then(|e| e.to_str()).map(|s| s.eq_ignore_ascii_case("json")).unwrap_or(false) {
				if let (Some(code), Ok(raw)) = (p.file_stem().and_then(|s| s.to_str()), fs::read_to_string(&p)) {
					self.insert_json_str(code, &raw);
				}
			}
		}
	}

	pub fn insert_json_str(&mut self, code: &str, raw: &str) {
		let Ok(json::JsonValue::Object(obj)) = json::parse(raw) else {
			return;
		};
		let mut display_name = code.to_owned();
		let mut entries = HashMap::new();
		for (k, v) in obj.iter() {
			if let Some(s) = v.as_str() {
				if k == "_name" {
					display_name = s.to_owned();
				} else {
					entries.insert(k.to_owned(), s.to_owned());
				}
			}
		}
		self.tables
			.entry(code.to_owned())
			.and_modify(|existing| {
				existing.display_name = display_name.clone();
				existing.entries.extend(entries.clone());
			})
			.or_insert(LocaleTable { display_name, entries });
	}

	pub fn current_lang(&self) -> &str {
		&self.current
	}

	pub fn current_display_name(&self) -> &str {
		self.tables
			.get(&self.current)
			.map(|t| t.display_name.as_str())
			.unwrap_or(&self.current)
	}

	pub fn available_languages(&self) -> Vec<(String, String)> {
		self.tables
			.iter()
			.map(|(k, v)| (k.clone(), v.display_name.clone()))
			.collect()
	}

	pub fn set_language(&mut self, code: &str) -> bool {
		if self.tables.contains_key(code) {
			self.current = code.to_owned();
			true
		} else {
			false
		}
	}

	pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
		if let Some(val) = self.tables.get(&self.current).and_then(|t| t.entries.get(key)) {
			return val.as_str();
		}
		if let Some(val) = self.tables.get("en").and_then(|t| t.entries.get(key)) {
			return val.as_str();
		}
		key
	}
}

fn external_locale_dirs() -> Vec<PathBuf> {
	let mut dirs = Vec::new();
	if let Some(env_dir) = std::env::var_os("RIGLENS_LOCALES_DIR") {
		if !env_dir.is_empty() {
			dirs.push(PathBuf::from(env_dir));
		}
	}
	if let Ok(exe) = std::env::current_exe() {
		if let Some(parent) = exe.parent() {
			dirs.push(parent.join("locales"));
		}
	}
	dirs
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn i18n_loads_builtin_locales_and_supports_external_json_override() {
		let mut i18n = I18n::new("zh-TW");
		assert_eq!(i18n.current_lang(), "zh-TW");
		assert_eq!(i18n.t("menu.file"), "檔案");

		assert!(i18n.set_language("ja"));
		assert_eq!(i18n.t("menu.file"), "ファイル");

		// Missing key falls back cleanly to key itself
		assert_eq!(i18n.t("nonexistent.key"), "nonexistent.key");

		// Community custom JSON locale file overrides or adds a new language
		let tmp = std::env::temp_dir().join(format!("riglens_i18n_test_{}", std::process::id()));
		let _ = fs::remove_dir_all(&tmp);
		fs::create_dir_all(&tmp).unwrap();
		fs::write(tmp.join("ko.json"), r#"{"_name":"한국어","menu.file":"파일"}"#).unwrap();

		i18n.load_dir(&tmp);
		assert!(i18n.set_language("ko"));
		assert_eq!(i18n.current_display_name(), "한국어");
		assert_eq!(i18n.t("menu.file"), "파일");
		// Partial custom locale falls back to English for untranslated keys
		assert_eq!(i18n.t("menu.edit"), "Edit");

		let _ = fs::remove_dir_all(&tmp);
	}
}

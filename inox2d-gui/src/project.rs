#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct ProjectWorkspace {
	pub name: String,
	pub root_dir: PathBuf,
	pub production_dir: PathBuf,
	pub vault_dir: PathBuf,
	pub working_inp: PathBuf,
	pub vault_psd: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct ProjectSummary {
	pub name: String,
	pub root_dir: PathBuf,
	pub working_inp: Option<PathBuf>,
	pub has_vault_psd: bool,
}

/// Default root directory holding all RigLens project folders (`production/` + `vault/`).
pub fn default_projects_root() -> PathBuf {
	if let Some(env_dir) = std::env::var_os("RIGLENS_PROJECTS_DIR") {
		if !env_dir.is_empty() {
			return PathBuf::from(env_dir);
		}
	}
	if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
		let home_p = PathBuf::from(home);
		let docs = home_p.join("Documents");
		if docs.is_dir() {
			return docs.join("RigLensProjects");
		}
		return home_p.join("RigLensProjects");
	}
	PathBuf::from("RigLensProjects")
}

fn sanitize_project_name(raw: &str) -> String {
	let cleaned: String = raw
		.chars()
		.map(|c| {
			if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
				'_'
			} else {
				c
			}
		})
		.collect();
	let trimmed = cleaned.trim();
	if trimmed.is_empty() {
		"Untitled_Project".to_owned()
	} else {
		trimmed.to_owned()
	}
}

/// Create a protected project workspace from an imported `.psd`:
/// - `production/`: working `.inp` & editor output files
/// - `vault/`: verbatim byte-for-byte copy of the original `.psd` (untouched backup)
pub fn create_project_from_psd(projects_root: &Path, psd_path: &Path) -> Result<ProjectWorkspace, String> {
	let stem = psd_path
		.file_stem()
		.and_then(|s| s.to_str())
		.unwrap_or("RigProject");
	let base_name = sanitize_project_name(stem);

	let mut name = base_name.clone();
	let mut root_dir = projects_root.join(&name);
	let mut suffix = 2u32;
	while root_dir.exists() {
		name = format!("{base_name}_{suffix}");
		root_dir = projects_root.join(&name);
		suffix += 1;
	}

	let production_dir = root_dir.join("production");
	let vault_dir = root_dir.join("vault");
	fs::create_dir_all(&production_dir).map_err(|e| format!("create production dir: {e}"))?;
	fs::create_dir_all(&vault_dir).map_err(|e| format!("create vault dir: {e}"))?;

	let psd_filename = psd_path
		.file_name()
		.and_then(|s| s.to_str())
		.unwrap_or("source.psd");
	let vault_psd = vault_dir.join(psd_filename);
	fs::copy(psd_path, &vault_psd).map_err(|e| format!("backup PSD into vault: {e}"))?;

	let working_inp = production_dir.join(format!("{name}.inp"));

	Ok(ProjectWorkspace {
		name,
		root_dir,
		production_dir,
		vault_dir,
		working_inp,
		vault_psd: Some(vault_psd),
	})
}

/// Discover existing RigLens project folders under `projects_root`.
pub fn list_projects(projects_root: &Path) -> Vec<ProjectSummary> {
	let Ok(entries) = fs::read_dir(projects_root) else {
		return Vec::new();
	};
	let mut out = Vec::new();
	for entry in entries.flatten() {
		let path = entry.path();
		if !path.is_dir() {
			continue;
		}
		let prod = path.join("production");
		let vault = path.join("vault");
		if !prod.is_dir() && !vault.is_dir() {
			continue;
		}
		let name = path
			.file_name()
			.and_then(|s| s.to_str())
			.unwrap_or("Project")
			.to_owned();
		let working_inp = fs::read_dir(&prod).ok().and_then(|rd| {
			rd.flatten()
				.map(|e| e.path())
				.find(|p| p.extension().and_then(|e| e.to_str()).map(|s| s.eq_ignore_ascii_case("inp")).unwrap_or(false))
		});
		let has_vault_psd = fs::read_dir(&vault)
			.ok()
			.map(|rd| {
				rd.flatten().any(|e| {
					e.path()
						.extension()
						.and_then(|ext| ext.to_str())
						.map(|s| s.eq_ignore_ascii_case("psd"))
						.unwrap_or(false)
				})
			})
			.unwrap_or(false);
		out.push(ProjectSummary {
			name,
			root_dir: path,
			working_inp,
			has_vault_psd,
		});
	}
	out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
	out
}

/// Open `path` in the OS file manager (creates directory first if missing).
pub fn open_folder_in_os(path: &Path) -> Result<(), String> {
	if !path.exists() {
		fs::create_dir_all(path).map_err(|e| format!("create dir '{}': {e}", path.display()))?;
	}
	#[cfg(target_os = "windows")]
	let res = Command::new("explorer").arg(path).spawn();
	#[cfg(target_os = "macos")]
	let res = Command::new("open").arg(path).spawn();
	#[cfg(all(unix, not(target_os = "macos")))]
	let res = Command::new("xdg-open").arg(path).spawn();

	res.map(|_| ()).map_err(|e| format!("open folder '{}': {e}", path.display()))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn create_project_backs_up_psd_verbatim_in_vault_and_lists_projects() {
		let tmp = std::env::temp_dir().join(format!("riglens_proj_test_{}", std::process::id()));
		let _ = fs::remove_dir_all(&tmp);
		fs::create_dir_all(&tmp).unwrap();

		let src_psd = tmp.join("my_character.psd");
		let raw_bytes = b"8BPS\x00\x01_ORIGINAL_UNTOUCHED_PSD_BYTES";
		fs::write(&src_psd, raw_bytes).unwrap();

		let root = tmp.join("Projects");
		let ws = create_project_from_psd(&root, &src_psd).expect("create project");
		assert!(ws.production_dir.is_dir());
		assert!(ws.vault_dir.is_dir());
		let backup = ws.vault_psd.expect("vault backup path");
		assert_eq!(fs::read(&backup).unwrap(), raw_bytes);

		// Creating a second project with the same PSD name keeps the first vault untouched
		let ws2 = create_project_from_psd(&root, &src_psd).expect("create second project");
		assert_ne!(ws2.root_dir, ws.root_dir);
		assert!(ws2.name.ends_with("_2"));

		let list = list_projects(&root);
		assert_eq!(list.len(), 2);
		assert!(list.iter().all(|p| p.has_vault_psd));

		let _ = fs::remove_dir_all(&tmp);
	}
}

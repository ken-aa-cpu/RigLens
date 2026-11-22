#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use riglens_core::formats::inp::write_inp_atomic;
use riglens_core::model::Model;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryInfo {
	pub recovery_inp: PathBuf,
	pub original_path: Option<String>,
	pub saved_epoch_s: u64,
}

pub struct AutosaveManager {
	dir: PathBuf,
	scan_root: Option<PathBuf>,
	interval: Duration,
	last_attempt: Instant,
	last_autosaved_rev: u64,
}

impl AutosaveManager {
	pub fn new() -> Self {
		let root = std::env::temp_dir();
		Self {
			dir: root.join(format!("riglens_autosave_{}", std::process::id())),
			scan_root: Some(root),
			interval: Duration::from_secs(30),
			last_attempt: Instant::now() - Duration::from_secs(30),
			last_autosaved_rev: 0,
		}
	}

	#[cfg(test)]
	pub fn with_dir(dir: PathBuf, interval: Duration) -> Self {
		// Initialize last_attempt in the past so tests with Duration::ZERO trigger immediately.
		Self {
			dir,
			scan_root: None,
			interval,
			last_attempt: Instant::now() - interval.min(Duration::from_secs(600)),
			last_autosaved_rev: 0,
		}
	}

	#[cfg(test)]
	pub fn with_dir_and_scan_root(dir: PathBuf, scan_root: PathBuf, interval: Duration) -> Self {
		Self {
			dir,
			scan_root: Some(scan_root),
			interval,
			last_attempt: Instant::now() - interval.min(Duration::from_secs(600)),
			last_autosaved_rev: 0,
		}
	}

	pub fn reset_for_revision(&mut self, rev: u64) {
		self.last_autosaved_rev = rev;
	}

	fn recovery_inp_path(&self) -> PathBuf {
		self.dir.join("recovery.inp")
	}

	fn recovery_meta_path(&self) -> PathBuf {
		self.dir.join("recovery_meta.json")
	}

	fn read_recovery_from_dir(dir: &Path) -> Option<(RecoveryInfo, SystemTime)> {
		let inp = dir.join("recovery.inp");
		if !inp.is_file() {
			return None;
		}
		let mut original_path = None;
		let mut saved_epoch_s = 0u64;
		if let Ok(meta_txt) = fs::read_to_string(dir.join("recovery_meta.json")) {
			if let Ok(val) = json::parse(&meta_txt) {
				original_path = val["original_path"].as_str().map(ToOwned::to_owned);
				saved_epoch_s = val["saved_epoch_s"].as_u64().unwrap_or(0);
			}
		}
		let mtime = fs::metadata(&inp)
			.and_then(|m| m.modified())
			.unwrap_or(UNIX_EPOCH);
		Some((
			RecoveryInfo {
				recovery_inp: inp,
				original_path,
				saved_epoch_s,
			},
			mtime,
		))
	}

	fn matching_autosave_dirs(root: &Path) -> Vec<PathBuf> {
		let Ok(rd) = fs::read_dir(root) else {
			return Vec::new();
		};
		rd.flatten()
			.filter_map(|entry| {
				let p = entry.path();
				if !p.is_dir() {
					return None;
				}
				let name = p.file_name()?.to_str()?;
				if name.starts_with("riglens_autosave") {
					Some(p)
				} else {
					None
				}
			})
			.collect()
	}

	pub fn check_pending_recovery(&self) -> Option<RecoveryInfo> {
		let mut best = Self::read_recovery_from_dir(&self.dir);
		if let Some(ref root) = self.scan_root {
			for dir in Self::matching_autosave_dirs(root) {
				if dir == self.dir {
					continue;
				}
				if let Some((info, mtime)) = Self::read_recovery_from_dir(&dir) {
					let replace = match &best {
						None => true,
						Some((b_info, b_mtime)) => {
							(info.saved_epoch_s, mtime) > (b_info.saved_epoch_s, *b_mtime)
						}
					};
					if replace {
						best = Some((info, mtime));
					}
				}
			}
		}
		best.map(|(info, _)| info)
	}

	pub fn maybe_autosave(
		&mut self,
		model: &Model,
		rev: u64,
		last_saved_rev: u64,
		source_path: Option<&Path>,
	) -> Result<bool, String> {
		if rev == last_saved_rev || rev == self.last_autosaved_rev {
			return Ok(false);
		}
		if self.last_attempt.elapsed() < self.interval {
			return Ok(false);
		}
		self.last_attempt = Instant::now();
		fs::create_dir_all(&self.dir).map_err(|e| format!("Failed to create autosave dir: {e}"))?;

		let dst = self.recovery_inp_path();
		write_inp_atomic(model, &dst)?;

		let epoch_s = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map(|d| d.as_secs())
			.unwrap_or(0);
		let meta = json::object! {
			original_path: source_path.map(|p| p.display().to_string()),
			saved_epoch_s: epoch_s,
			revision: rev,
		};
		fs::write(self.recovery_meta_path(), json::stringify_pretty(meta, 2))
			.map_err(|e| format!("Failed to write autosave metadata: {e}"))?;

		self.last_autosaved_rev = rev;
		Ok(true)
	}

	fn clear_recovery_dir(dir: &Path) {
		let _ = fs::remove_file(dir.join("recovery.inp"));
		let _ = fs::remove_file(dir.join("recovery_meta.json"));
		let _ = fs::remove_file(dir.join("recovery.inp.tmp"));
		let _ = fs::remove_dir(dir);
	}

	pub fn clear_recovery(&self) {
		Self::clear_recovery_dir(&self.dir);
		if let Some(ref root) = self.scan_root {
			for dir in Self::matching_autosave_dirs(root) {
				Self::clear_recovery_dir(&dir);
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use riglens_core::formats::inp::parse_inp;
	use riglens_mcp::engine::Session;

	#[test]
	fn autosave_writes_recovery_and_clears_cleanly() {
		let base = std::env::temp_dir().join(format!("riglens_unit_autosave_{}", std::process::id()));
		let dir = base.join("riglens_autosave_unit");
		let _ = fs::remove_dir_all(&base);
		let mut mgr = AutosaveManager::with_dir(dir.clone(), Duration::ZERO);
		assert!(mgr.check_pending_recovery().is_none());

		let session = Session::new_sample();
		let src = PathBuf::from("sample_vtuber.inp");
		// Same revision as last_saved_rev -> no autosave needed
		assert!(!mgr.maybe_autosave(&session.model, 0, 0, Some(&src)).unwrap());
		// Dirty revision -> autosave writes recovery.inp + recovery_meta.json
		assert!(mgr.maybe_autosave(&session.model, 3, 0, Some(&src)).unwrap());

		let rec = mgr.check_pending_recovery().expect("pending recovery should exist");
		assert_eq!(rec.original_path.as_deref(), Some("sample_vtuber.inp"));
		let bytes = fs::read(&rec.recovery_inp).unwrap();
		let parsed = parse_inp(bytes.as_slice()).expect("recovery inp parses cleanly");
		assert!(parsed.puppet.nodes.iter().count() >= 5);

		mgr.clear_recovery();
		assert!(mgr.check_pending_recovery().is_none());
		let _ = fs::remove_dir_all(&base);
	}

	#[test]
	fn autosave_discovers_newest_recovery_across_different_pid_dirs() {
		let root = std::env::temp_dir().join(format!("riglens_unit_recovery_scan_{}", std::process::id()));
		let _ = fs::remove_dir_all(&root);
		fs::create_dir_all(&root).unwrap();

		let session = Session::new_sample();
		let dir_old = root.join("riglens_autosave_10001");
		let mut old_mgr = AutosaveManager::with_dir(dir_old.clone(), Duration::ZERO);
		assert!(old_mgr.maybe_autosave(&session.model, 2, 0, Some(Path::new("older.inp"))).unwrap());
		// Force deterministic older timestamp on dir_old metadata
		fs::write(
			dir_old.join("recovery_meta.json"),
			r#"{"original_path":"older.inp","saved_epoch_s":1000,"revision":2}"#,
		)
		.unwrap();

		let dir_new = root.join("riglens_autosave_20002");
		let mut new_mgr = AutosaveManager::with_dir(dir_new.clone(), Duration::ZERO);
		assert!(new_mgr.maybe_autosave(&session.model, 5, 0, Some(Path::new("newer.inp"))).unwrap());
		fs::write(
			dir_new.join("recovery_meta.json"),
			r#"{"original_path":"newer.inp","saved_epoch_s":2000,"revision":5}"#,
		)
		.unwrap();

		// Restarted process has a different PID dir (riglens_autosave_99999) scanning root
		let restarted = AutosaveManager::with_dir_and_scan_root(
			root.join("riglens_autosave_99999"),
			root.clone(),
			Duration::from_secs(30),
		);
		let rec = restarted
			.check_pending_recovery()
			.expect("restarted manager should discover prior PID recovery");
		assert_eq!(rec.original_path.as_deref(), Some("newer.inp"));
		assert_eq!(rec.saved_epoch_s, 2000);

		restarted.clear_recovery();
		assert!(restarted.check_pending_recovery().is_none());
		assert!(!dir_old.exists());
		assert!(!dir_new.exists());
		let _ = fs::remove_dir_all(&root);
	}
}

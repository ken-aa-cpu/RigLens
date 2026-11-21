#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use riglens_core::formats::inp::write_inp;
use riglens_core::model::Model;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryInfo {
	pub recovery_inp: PathBuf,
	pub original_path: Option<String>,
	pub saved_epoch_s: u64,
}

pub struct AutosaveManager {
	dir: PathBuf,
	interval: Duration,
	last_attempt: Instant,
	last_autosaved_rev: u64,
}

impl AutosaveManager {
	pub fn new() -> Self {
		Self::with_dir(std::env::temp_dir().join("riglens_autosave"), Duration::from_secs(30))
	}

	pub fn with_dir(dir: PathBuf, interval: Duration) -> Self {
		// Initialize last_attempt in the past so tests with Duration::ZERO trigger immediately.
		Self {
			dir,
			interval,
			last_attempt: Instant::now() - interval.min(Duration::from_secs(600)),
			last_autosaved_rev: 0,
		}
	}

	fn recovery_inp_path(&self) -> PathBuf {
		self.dir.join("recovery.inp")
	}

	fn recovery_meta_path(&self) -> PathBuf {
		self.dir.join("recovery_meta.json")
	}

	pub fn check_pending_recovery(&self) -> Option<RecoveryInfo> {
		let inp = self.recovery_inp_path();
		if !inp.is_file() {
			return None;
		}
		let mut original_path = None;
		let mut saved_epoch_s = 0u64;
		if let Ok(meta_txt) = fs::read_to_string(self.recovery_meta_path()) {
			if let Ok(val) = json::parse(&meta_txt) {
				original_path = val["original_path"].as_str().map(ToOwned::to_owned);
				saved_epoch_s = val["saved_epoch_s"].as_u64().unwrap_or(0);
			}
		}
		Some(RecoveryInfo {
			recovery_inp: inp,
			original_path,
			saved_epoch_s,
		})
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
		let tmp = self.dir.join("recovery.inp.tmp");
		{
			let f = File::create(&tmp).map_err(|e| format!("Failed to create autosave temp file: {e}"))?;
			let mut w = BufWriter::new(f);
			write_inp(model, &mut w).map_err(|e| format!("Failed to serialize autosave: {e}"))?;
			w.flush().map_err(|e| format!("Failed to flush autosave: {e}"))?;
		}
		if dst.exists() {
			let _ = fs::remove_file(&dst);
		}
		fs::rename(&tmp, &dst).map_err(|e| format!("Failed to commit autosave: {e}"))?;

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

	pub fn clear_recovery(&self) {
		let _ = fs::remove_file(self.recovery_inp_path());
		let _ = fs::remove_file(self.recovery_meta_path());
		let _ = fs::remove_file(self.dir.join("recovery.inp.tmp"));
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use riglens_core::formats::inp::parse_inp;
	use riglens_mcp::engine::Session;

	#[test]
	fn autosave_writes_recovery_and_clears_cleanly() {
		let dir = std::env::temp_dir().join(format!("riglens_autosave_test_{}", std::process::id()));
		let _ = fs::remove_dir_all(&dir);
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
		let _ = fs::remove_dir_all(&dir);
	}
}

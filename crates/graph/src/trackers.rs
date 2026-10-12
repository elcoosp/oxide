//! Concrete tracker implementations (spec §5.2): real FS reads, an injectable
//! env map (testable without touching `std::env`), and an injectable config map.

use crate::{ConfigTracker, EnvTracker, FileTracker, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Real-filesystem tracker: hashes nothing itself — the graph's recording
/// wrappers compute the blake3 hashes when they record the inputs.
#[derive(Debug, Default, Clone, Copy)]
pub struct FsTrackerImpl;

impl FsTrackerImpl {
    pub fn new() -> Self {
        Self
    }
}

impl FileTracker for FsTrackerImpl {
    /// Read a file's bytes and record `Input::File`.
    fn read_file(&mut self, path: &Path) -> Result<Vec<u8>> {
        std::fs::read(path)
            .map_err(|e| crate::err("GRAPH-FS-IO", format!("read {}: {e}", path.display())))
    }

    /// List a directory and record `Input::DirListing`.
    fn list_dir(&mut self, path: &Path) -> Result<Vec<PathBuf>> {
        let rd = std::fs::read_dir(path)
            .map_err(|e| crate::err("GRAPH-FS-IO", format!("list {}: {e}", path.display())))?;
        let mut out: Vec<PathBuf> = Vec::new();
        for e in rd {
            let e =
                e.map_err(|e| crate::err("GRAPH-FS-IO", format!("list {}: {e}", path.display())))?;
            out.push(e.path());
        }
        out.sort(); // deterministic listing order (conventions: sort before hashing)
        Ok(out)
    }
}

/// Env tracker over an injectable map, so tests never depend on `std::env`.
#[derive(Debug, Clone)]
pub struct EnvTrackerImpl {
    pub(crate) vars: Arc<RwLock<BTreeMap<String, String>>>,
}

impl EnvTrackerImpl {
    pub fn new(vars: Arc<RwLock<BTreeMap<String, String>>>) -> Self {
        Self { vars }
    }
}

impl EnvTracker for EnvTrackerImpl {
    /// Read an env var and record `Input::Env`.
    fn get(&mut self, key: &str) -> Option<String> {
        self.vars.read().unwrap().get(key).cloned()
    }
}

/// Config tracker over an injectable map (dotted keys → JSON values).
#[derive(Debug, Clone)]
pub struct ConfigTrackerImpl {
    pub(crate) values: Arc<RwLock<BTreeMap<String, Value>>>,
}

impl ConfigTrackerImpl {
    pub fn new(values: Arc<RwLock<BTreeMap<String, Value>>>) -> Self {
        Self { values }
    }
}

impl ConfigTracker for ConfigTrackerImpl {
    /// Read a config key and record `Input::ConfigKey`.
    fn get(&mut self, key: &str) -> Option<Value> {
        self.values.read().unwrap().get(key).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn fs_read_missing_file_error_code() {
        let mut fs = FsTrackerImpl::new();
        let e = fs
            .read_file(Path::new("/definitely/not/here/oxide.txt"))
            .unwrap_err();
        assert_eq!(e.0.code, "GRAPH-FS-IO");
    }

    #[test]
    fn fs_list_dir_is_sorted_and_reports_missing() {
        let tmp = tempfile::tempdir().unwrap();
        for n in ["c.txt", "a.txt", "b.txt"] {
            std::fs::write(tmp.path().join(n), b"x").unwrap();
        }
        let mut fs = FsTrackerImpl::new();
        let listing = fs.list_dir(tmp.path()).unwrap();
        let names: Vec<String> = listing
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "c.txt"]);
        let e = fs.list_dir(Path::new("/definitely/not/here")).unwrap_err();
        assert_eq!(e.0.code, "GRAPH-FS-IO");
    }

    #[test]
    fn env_tracker_reads_injected_map() {
        let vars: Arc<RwLock<BTreeMap<String, String>>> =
            Arc::new(RwLock::new(BTreeMap::from([("OX_A".into(), "1".into())])));
        let mut env = EnvTrackerImpl::new(vars.clone());
        assert_eq!(env.get("OX_A").as_deref(), Some("1"));
        assert_eq!(env.get("OX_MISSING"), None);
        vars.write().unwrap().insert("OX_A".into(), "2".into());
        assert_eq!(
            env.get("OX_A").as_deref(),
            Some("2"),
            "map stays live across reads"
        );
    }

    #[test]
    fn config_tracker_reads_injected_map() {
        let values: Arc<RwLock<BTreeMap<String, Value>>> =
            Arc::new(RwLock::new(BTreeMap::from([(
                "server.port".into(),
                serde_json::json!(3000),
            )])));
        let mut cfg = ConfigTrackerImpl::new(values);
        assert_eq!(cfg.get("server.port"), Some(serde_json::json!(3000)));
        assert_eq!(cfg.get("server.host"), None);
    }
}

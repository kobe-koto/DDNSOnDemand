//! Persistent last-known-IP state used for differential updates.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::provider::RecordType;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub ip: String,
    pub updated_at: String,
}

fn key(profile: &str, domain: &str, rtype: RecordType) -> String {
    format!("{profile}|{domain}|{}", rtype.as_str())
}

impl State {
    /// Load state from disk, returning an empty state if the file is absent.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) if text.trim().is_empty() => Ok(Self::default()),
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parsing state file {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading state file {}", path.display())),
        }
    }

    pub fn get(&self, profile: &str, domain: &str, rtype: RecordType) -> Option<&Entry> {
        self.entries.get(&key(profile, domain, rtype))
    }

    pub fn set(&mut self, profile: &str, domain: &str, rtype: RecordType, ip: &str) {
        self.entries.insert(
            key(profile, domain, rtype),
            Entry {
                ip: ip.to_string(),
                updated_at: crate::now_rfc3339(),
            },
        );
    }

    /// Atomically persist the state (write temp file, then rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating state dir {}", parent.display()))?;
        }
        let tmp = tmp_path(path);
        let json = serde_json::to_string_pretty(self).context("serializing state")?;
        {
            let mut f = std::fs::File::create(&tmp)
                .with_context(|| format!("creating temp state file {}", tmp.display()))?;
            f.write_all(json.as_bytes())?;
            f.write_all(b"\n")?;
            f.sync_all().ok();
        }
        std::fs::rename(&tmp, path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".tmp");
    PathBuf::from(os)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let dir = std::env::temp_dir().join(format!("ddns-state-test-{}", std::process::id()));
        let path = dir.join("state.json");
        let mut st = State::default();
        st.set("home", "a.example.com", RecordType::A, "1.2.3.4");
        st.save(&path).unwrap();

        let loaded = State::load(&path).unwrap();
        assert_eq!(loaded.get("home", "a.example.com", RecordType::A).unwrap().ip, "1.2.3.4");
        assert!(loaded.get("home", "a.example.com", RecordType::Aaaa).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_is_empty() {
        let path = std::env::temp_dir().join("ddns-nonexistent-xyz/state.json");
        let st = State::load(&path).unwrap();
        assert!(st.get("x", "y", RecordType::A).is_none());
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

use chrono::Utc;
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub struct StateError {
    pub reason: &'static str,
    pub path: PathBuf,
    pub detail: String,
}

impl StateError {
    fn new(reason: &'static str, path: &Path, detail: impl std::fmt::Display) -> Self {
        Self {
            reason,
            path: path.to_path_buf(),
            detail: detail.to_string(),
        }
    }

    /// State errors are fatal at the runtime boundary, never panics or a fresh
    /// workspace. Existing agent APIs can stay infallible without losing writes.
    pub fn exit(self) -> ! {
        crate::print_json(serde_json::json!({
            "status": "error",
            "reason": self.reason,
            "path": self.path.to_string_lossy(),
            "error": self.detail,
            "instruction": "Restore or repair the authoritative state and its filesystem permissions, then retry. Do not delete state to bypass a gate; a failed write may have partially completed the operation."
        }));
        std::process::exit(1)
    }
}

/// The Formwork directory: `state.json` (task state and the artifact registry)
/// plus the project's JSON artifacts, which the agents write. `scratch/` holds
/// transient submission payloads and is the only git-ignored path under it.
pub const FORMWORK_DIR: &str = ".formwork";
/// Transient agent submission payloads, relative to [`FORMWORK_DIR`].
pub const SCRATCH_SUBDIR: &str = "scratch";

/// Holds ONLY metadata: task state and an artifact registry (file pointers,
/// descriptions, statuses). Artifact CONTENTS live in the workspace — under
/// `.formwork/` for the JSON spec artifacts, and in the source tree for code.
pub struct Blackboard {
    root: PathBuf,
}

impl Blackboard {
    pub fn new(root: Option<&str>) -> Self {
        // The environment variable names the workspace, not the state directory.
        let base = root
            .map(PathBuf::from)
            .or_else(|| std::env::var("FORMWORK_WORKSPACE").ok().map(PathBuf::from))
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|e| {
                    StateError::new("workspace_unreadable", Path::new("."), e).exit()
                })
            });
        Self {
            root: base.join(FORMWORK_DIR),
        }
    }

    /// Lock the stable workspace inode, not state.json (atomic replacement changes
    /// that inode). The OS releases this lock even when an invocation exits early.
    /// Opening a directory creates nothing, including for a missing workspace.
    pub fn lock_workspace(&self) -> Result<File, StateError> {
        let file = File::open(self.workspace())
            .map_err(|e| StateError::new("workspace_lock_failed", self.workspace(), e))?;
        file.lock()
            .map_err(|e| StateError::new("workspace_lock_failed", self.workspace(), e))?;
        Ok(file)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The project workspace root — the parent of `.formwork/`.
    pub fn workspace(&self) -> &Path {
        self.root.parent().unwrap_or(&self.root)
    }

    // ── state.json ──

    pub fn read_state(&self) -> Value {
        self.try_read_state().unwrap_or_else(|e| e.exit())
    }

    pub fn try_read_state(&self) -> Result<Value, StateError> {
        let current = self.root.join("state.json");
        let path = current.clone();
        let content = match std::fs::read_to_string(&current) {
            Ok(content) => Some(content),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // A dangling authoritative symlink is broken state, not absence.
                if std::fs::symlink_metadata(&current).is_ok() {
                    return Err(StateError::new("state_unreadable", &current, e));
                }
                // Check legacy locations to prevent accidental reinitialization.
                // Never read, migrate, or rewrite their contents.
                for legacy in [".vibe", ".blackboard"] {
                    let unsupported = self.workspace().join(legacy).join("state.json");
                    match std::fs::symlink_metadata(&unsupported) {
                        Ok(_) => return Err(StateError::new("unsupported_state", &unsupported,
                            "Use the matching binary or a separate workspace without overwriting existing state")),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(e) => return Err(StateError::new("state_unreadable", &unsupported, e)),
                    }
                }
                None
            }
            Err(e) => return Err(StateError::new("state_unreadable", &current, e)),
        };
        let Some(content) = content else {
            return Ok(serde_json::json!({
                "phase": "init",
                "idea": "",
                "completed": [],
                "prior_summaries": [],
                "artifacts": [],
                "blockers": [],
                "started_at": null,
                "updated_at": null
            }));
        };
        let state: Value = serde_json::from_str(&content)
            .map_err(|e| StateError::new("state_corrupt", &path, e))?;
        if state.is_object() && phase_is_known(&state) && state["pipeline_version"] != 2 {
            return Err(StateError::new("unsupported_state", &path,
                "Use the matching binary or a separate workspace without overwriting existing state"));
        }
        let phase = state.get("phase").and_then(Value::as_str);
        if !state.is_object() || !crate::state::PHASES.contains(&phase.unwrap_or("")) {
            return Err(StateError::new(
                "state_corrupt",
                &path,
                "State must be an object with a recognized phase",
            ));
        }
        for key in ["completed", "prior_summaries", "artifacts", "blockers"] {
            if let Some(value) = state.get(key) {
                if !value.is_array()
                    || (key != "prior_summaries"
                        && value.as_array().unwrap().iter().any(|v| !v.is_object()))
                {
                    return Err(StateError::new(
                        "state_corrupt",
                        &path,
                        format!("{key} must be an array of objects"),
                    ));
                }
            } else {
                return Err(StateError::new(
                    "state_corrupt",
                    &path,
                    format!("State is missing required array {key}"),
                ));
            }
        }
        // Rollback dispatches on these fields. Accepting an empty object here
        // would turn missing metadata into an implicit reset to init.
        for entry in state["completed"].as_array().unwrap() {
            let phase = entry.get("phase").and_then(Value::as_str).unwrap_or("");
            let valid = if phase == "build" {
                entry["milestone"].as_str().is_some_and(|s| !s.is_empty())
                    && entry["verification"].is_object()
            } else {
                crate::state::phase_agent(phase).is_some()
            };
            if !valid {
                return Err(StateError::new(
                    "state_corrupt",
                    &path,
                    "Completion entries need a recognized gate phase; build entries also need a milestone and verification metadata",
                ));
            }
        }
        // These containers are subsequently mutated with object indexing. A
        // scalar must be corruption, not a panic or a fresh lifecycle default.
        for key in ["outstanding_task", "design_review", "design_lint"] {
            if let Some(value) = state.get(key) {
                if !value.is_null() && !value.is_object() {
                    return Err(StateError::new(
                        "state_corrupt",
                        &path,
                        format!("{key} must be an object or null"),
                    ));
                }
            }
        }
        crate::schema::validate("state", &state)
            .map_err(|e| StateError::new("state_corrupt", &path, e))?;
        Ok(state)
    }

    pub fn write_state(&self, state: &Value) {
        self.try_write_state(state).unwrap_or_else(|e| e.exit());
    }

    pub fn try_write_state(&self, state: &Value) -> Result<(), StateError> {
        let path = self.root.join("state.json");
        if !state.is_object() {
            return Err(StateError::new(
                "state_persistence_failed",
                &path,
                "State must be an object",
            ));
        }
        let mut state = state.clone();
        // Stamp the state schema on every write so the file is self-describing
        // and validatable. Formwork is the only writer of this file; the
        // pointer exists for inspection and repair, not for agents to read.
        state["$schema"] = Value::String(
            crate::schema::state_schema_path()
                .to_string_lossy()
                .to_string(),
        );
        state["updated_at"] = Value::String(Utc::now().to_rfc3339());
        std::fs::create_dir_all(&self.root)
            .map_err(|e| StateError::new("state_persistence_failed", &path, e))?;
        // Persist the directory entry too when this is the workspace's first
        // write; syncing only the new state file cannot make its parent durable.
        File::open(self.workspace())
            .and_then(|f| f.sync_all())
            .map_err(|e| StateError::new("state_persistence_failed", &path, e))?;
        let json = serde_json::to_vec_pretty(&state)
            .map_err(|e| StateError::new("state_persistence_failed", &path, e))?;
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let (temp, mut file) = loop {
            let temp = self.root.join(format!(
                ".state-{}-{}.tmp",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match OpenOptions::new().write(true).create_new(true).open(&temp) {
                Ok(file) => break (temp, file),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(StateError::new("state_persistence_failed", &path, e)),
            }
        };
        let result = (|| -> io::Result<()> {
            file.write_all(&json)?;
            file.sync_all()?;
            std::fs::rename(&temp, &path)?;
            File::open(&self.root)?.sync_all()
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_file(&temp);
            return Err(StateError::new("state_persistence_failed", &path, e));
        }
        Ok(())
    }

    // ── artifact registry (metadata only) ──

    /// Record metadata for artifacts a coding agent wrote to the workspace.
    /// Each entry is `{path, description, status, domain, recorded_at}` plus an
    /// optional `slice` tag for build-phase artifacts. No file contents are
    /// ever stored.
    pub fn record_artifacts(&self, domain: &str, slice: Option<&str>, artifacts: &[ArtifactMeta]) {
        let mut state = self.read_state();
        if !state["artifacts"].is_array() {
            state["artifacts"] = Value::Array(vec![]);
        }
        let now = Utc::now().to_rfc3339();
        let registry = state["artifacts"].as_array_mut().unwrap();
        for a in artifacts {
            // Replace any existing entry for the same path.
            registry.retain(|e| e.get("path").and_then(|v| v.as_str()) != Some(&a.path));
            let mut entry = serde_json::json!({
                "path": a.path,
                "description": a.description,
                "status": a.status,
                "domain": domain,
                "recorded_at": now,
            });
            if let Some(slug) = slice {
                entry["slice"] = Value::String(slug.to_string());
            }
            registry.push(entry);
        }
        self.write_state(&state);
    }

    /// Return recorded artifact metadata, optionally filtered by domain.
    pub fn artifacts(&self, domain: Option<&str>) -> Vec<ArtifactMeta> {
        let state = self.read_state();
        state
            .get("artifacts")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|e| {
                        domain.is_none() || e.get("domain").and_then(|v| v.as_str()) == domain
                    })
                    .filter_map(ArtifactMeta::from_value)
                    .collect()
            })
            .unwrap_or_default()
    }

    // ── blockers ──

    pub fn add_blocker(
        &self,
        agent: &str,
        question: &str,
        context: &str,
        depends_on: Option<&[String]>,
    ) -> String {
        let mut state = self.read_state();
        let blockers = state["blockers"].as_array().map(|a| a.len()).unwrap_or(0);
        let blocker_id = format!("BLK-{:03}", blockers + 1);
        let blocker = serde_json::json!({
            "id": blocker_id,
            "agent": agent,
            "question": question,
            "context": context,
            "depends_on": depends_on.unwrap_or(&[]),
            "resolved": false,
            "resolution": null,
            "created_at": Utc::now().to_rfc3339(),
        });
        if !state["blockers"].is_array() {
            state["blockers"] = Value::Array(vec![]);
        }
        state["blockers"].as_array_mut().unwrap().push(blocker);
        self.write_state(&state);
        blocker_id
    }
}

fn phase_is_known(state: &Value) -> bool {
    state["phase"]
        .as_str()
        .is_some_and(|p| crate::state::PHASES.contains(&p))
}

/// Metadata about a single artifact — a pointer into the project workspace.
#[derive(Debug, Clone)]
pub struct ArtifactMeta {
    pub path: String,
    pub description: String,
    pub status: String,
}

impl ArtifactMeta {
    pub fn from_value(v: &Value) -> Option<Self> {
        let path = v.get("path").and_then(|p| p.as_str())?.to_string();
        let description = v
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();
        let status = v
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("complete")
            .to_string();
        Some(Self {
            path,
            description,
            status,
        })
    }

    pub fn to_context(&self) -> Value {
        serde_json::json!({
            "path": self.path,
            "description": self.description,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> io::Result<TempDir> {
        let path =
            Path::new("/tmp/opencode").join(format!("formwork-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        Ok(TempDir(path))
    }

    fn state(phase: &str) -> Value {
        serde_json::json!({"pipeline_version":2,"stage":"design","phase":phase,"completed":[],"prior_summaries":[],"artifacts":[],"blockers":[]})
    }

    #[test]
    fn state_missing_read_is_read_only() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing");
        let bb = Blackboard::new(Some(missing.to_str().unwrap()));
        assert_eq!(bb.try_read_state().unwrap()["phase"], "init");
        assert!(!missing.exists());
    }

    #[test]
    fn vibe_state_is_rejected_without_creating_formwork_state() {
        let dir = tempdir().unwrap();
        let bb = Blackboard::new(Some(dir.path().to_str().unwrap()));
        let old = dir.path().join(".vibe/state.json");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        let before = serde_json::to_vec(&state("design")).unwrap();
        std::fs::write(&old, &before).unwrap();

        let error = bb.try_read_state().unwrap_err();
        assert_eq!(error.reason, "unsupported_state");
        assert_eq!(error.path, old);
        assert_eq!(std::fs::read(old).unwrap(), before);
        assert!(!bb.root().exists());
    }

    #[test]
    fn unsupported_state_is_rejected_without_reinitializing() {
        let dir = tempdir().unwrap();
        let bb = Blackboard::new(Some(dir.path().to_str().unwrap()));
        std::fs::create_dir_all(bb.root()).unwrap();
        let unsupported = dir.path().join(".blackboard");
        std::fs::create_dir_all(&unsupported).unwrap();
        std::fs::write(
            unsupported.join("state.json"),
            serde_json::to_vec(&state("design")).unwrap(),
        )
        .unwrap();
        for invalid in ["{", "null", "[]", "{}", r#"{"phase":"unknown"}"#] {
            std::fs::write(bb.root().join("state.json"), invalid).unwrap();
            assert_eq!(bb.try_read_state().unwrap_err().reason, "state_corrupt");
        }
        for bytes in [
            r#"{"phase":"design"}"#,
            r#"{"phase":"design","pipeline_version":3}"#,
        ] {
            std::fs::write(bb.root().join("state.json"), bytes).unwrap();
            assert_eq!(bb.try_read_state().unwrap_err().reason, "unsupported_state");
            assert_eq!(
                std::fs::read_to_string(bb.root().join("state.json")).unwrap(),
                bytes
            );
        }
        std::fs::remove_file(bb.root().join("state.json")).unwrap();
        let before = std::fs::read(unsupported.join("state.json")).unwrap();
        assert_eq!(bb.try_read_state().unwrap_err().reason, "unsupported_state");
        assert_eq!(
            std::fs::read(unsupported.join("state.json")).unwrap(),
            before
        );
        assert!(!bb.root().join("state.json").exists());
    }

    #[test]
    fn state_unreadable_is_not_missing() {
        let dir = tempdir().unwrap();
        let bb = Blackboard::new(Some(dir.path().to_str().unwrap()));
        std::fs::create_dir_all(bb.root().join("state.json")).unwrap();
        assert_eq!(bb.try_read_state().unwrap_err().reason, "state_unreadable");
    }

    #[test]
    fn state_rejects_malformed_completion_discriminators() {
        let dir = tempdir().unwrap();
        let bb = Blackboard::new(Some(dir.path().to_str().unwrap()));
        for entry in [
            serde_json::json!({}),
            serde_json::json!({"phase":"unknown"}),
            serde_json::json!({"phase":"init"}),
            serde_json::json!({"phase":"build"}),
            serde_json::json!({"phase":"build","milestone":"","verification":{}}),
            serde_json::json!({"phase":"build","milestone":"mvp"}),
        ] {
            let mut value = state("design");
            value["completed"] = serde_json::json!([entry]);
            bb.try_write_state(&value).unwrap();
            assert_eq!(bb.try_read_state().unwrap_err().reason, "state_corrupt");
        }
        let mut value = state("build");
        value["completed"] = serde_json::json!([
            {"phase":"storytelling"}, {"phase":"design"},
            {"phase":"design:review"}, {"phase":"build","milestone":"mvp","verification":{}}
        ]);
        bb.try_write_state(&value).unwrap();
        assert!(bb.try_read_state().is_ok());
    }

    #[test]
    fn state_atomic_replacement_and_failed_write_preserve_original() {
        let dir = tempdir().unwrap();
        let bb = Blackboard::new(Some(dir.path().to_str().unwrap()));
        bb.try_write_state(&state("design")).unwrap();
        // A hard link retains the previous inode, proving replacement rather
        // than truncation of the state readers may already have open.
        let old = dir.path().join("old.json");
        std::fs::hard_link(bb.root().join("state.json"), &old).unwrap();
        bb.try_write_state(&state("build")).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(old).unwrap()).unwrap()["phase"],
            "design"
        );
        assert_eq!(bb.try_read_state().unwrap()["phase"], "build");
        let before = std::fs::read(bb.root().join("state.json")).unwrap();
        assert_eq!(
            bb.try_write_state(&Value::Null).unwrap_err().reason,
            "state_persistence_failed"
        );
        assert_eq!(std::fs::read(bb.root().join("state.json")).unwrap(), before);
        assert_eq!(std::fs::read_dir(bb.root()).unwrap().count(), 1);
    }

    #[test]
    fn state_failed_replacement_preserves_other_files() {
        let dir = tempdir().unwrap();
        let bb = Blackboard::new(Some(dir.path().to_str().unwrap()));
        let other = dir.path().join("other.json");
        std::fs::write(&other, r#"{"phase":"design"}"#).unwrap();
        std::fs::create_dir_all(bb.root().join("state.json")).unwrap();
        assert_eq!(
            bb.try_write_state(&serde_json::json!({"phase":"build"}))
                .unwrap_err()
                .reason,
            "state_persistence_failed"
        );
        assert_eq!(
            std::fs::read_to_string(other).unwrap(),
            r#"{"phase":"design"}"#
        );
        assert_eq!(std::fs::read_dir(bb.root()).unwrap().count(), 1);
    }

    #[test]
    fn state_workspace_lock_is_exclusive_without_creating_files() {
        let dir = tempdir().unwrap();
        let bb = Blackboard::new(Some(dir.path().to_str().unwrap()));
        let lock = bb.lock_workspace().unwrap();
        let second = File::open(dir.path()).unwrap();
        assert!(second.try_lock().is_err());
        drop(lock);
        second.try_lock().unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

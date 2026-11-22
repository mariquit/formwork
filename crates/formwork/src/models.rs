// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

//! Home directory and the harness target recorded on a project.
//!
//! Role models are not resolved here. `formwork setup` writes
//! `agents.<role>.model` in the OpenCode config, and a task omits `model`.

use std::path::PathBuf;

/// Effective target for a project: an explicit `FORMWORK_TARGET` env
/// override wins, then the target recorded at `formwork start`, then
/// `opencode`. `start` still records which harness opened the project.
/// The value does not select a model.
pub fn resolve_target(stored: Option<&str>) -> String {
    if let Ok(t) = std::env::var("FORMWORK_TARGET") {
        let t = t.trim().to_lowercase();
        if !t.is_empty() {
            return t;
        }
    }
    if let Some(t) = stored {
        let t = t.trim().to_lowercase();
        if !t.is_empty() {
            return t;
        }
    }
    "opencode".to_string()
}

pub fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "~".to_string()))
}

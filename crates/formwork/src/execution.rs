// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

//! Bounded execution and evidence identity. This is not an execution sandbox.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub fn environment() -> String {
    let vars: BTreeMap<_, _> = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    hash(&serde_json::to_vec(&vars).unwrap())
}

pub fn executable(program: &str, workspace: &Path) -> Result<PathBuf, String> {
    let candidates =
        if Path::new(program).components().count() > 1 || Path::new(program).is_absolute() {
            vec![workspace.join(program)]
        } else {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|p| {
                    if p.is_absolute() {
                        p.join(program)
                    } else {
                        workspace.join(p).join(program)
                    }
                })
                .collect()
        };
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| format!("Executable unavailable: {program}"))
}

pub fn tools(workspace: &Path, commands: &Value) -> Result<Value, String> {
    let mut tools = BTreeMap::new();
    for c in commands.as_array().ok_or("commands must be an array")? {
        let program = c["program"].as_str().ok_or("program required")?;
        let path = executable(program, workspace)?;
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        tools.insert(program.to_owned(),json!({"path":path,"target":path.canonicalize().map_err(|e|e.to_string())?,"hash":hash(&bytes)}));
    }
    Ok(json!(tools))
}

pub fn candidate(workspace: &Path) -> Result<String, String> {
    fn walk(root: &Path, rel: &Path, files: &mut BTreeMap<String, String>) -> Result<(), String> {
        for entry in std::fs::read_dir(root.join(rel)).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            // Generated outputs are not candidate source. Protected harness files
            // are hashed independently even when they live in one of these trees.
            if matches!(
                name.to_str(),
                Some(
                    ".git"
                        | ".formwork"
                        | "target"
                        | "node_modules"
                        | "dist"
                        | "build"
                        | ".venv"
                        | "__pycache__"
                        | ".pytest_cache"
                        | ".next"
                        | "coverage"
                )
            ) {
                continue;
            }
            let path = rel.join(name);
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                walk(root, &path, files)?;
            } else if kind.is_file() {
                files.insert(
                    path.to_string_lossy().into_owned(),
                    hash(&std::fs::read(entry.path()).map_err(|e| e.to_string())?),
                );
            } else {
                return Err(format!("Non-regular candidate input: {}", path.display()));
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    walk(workspace, Path::new(""), &mut files)?;
    Ok(hash(&serde_json::to_vec(&files).unwrap()))
}

pub fn run(workspace: &Path, checks: &Value) -> Result<Value, String> {
    let commands = checks
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or("Nonempty commands required")?;
    // Validate all inputs before executing any command.
    for c in commands {
        if c["program"].as_str().is_none_or(|s| s.is_empty())
            || !c["args"]
                .as_array()
                .is_some_and(|a| a.iter().all(Value::is_string))
            || c["non_interactive"] != true
            || c["timeout_seconds"]
                .as_u64()
                .is_some_and(|t| !(1..=600).contains(&t))
        {
            return Err("Invalid noninteractive command or timeout (1 to 600 seconds)".into());
        }
    }
    let dir = workspace.join(".formwork/execution");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut records = Vec::new();
    for c in commands {
        let program = c["program"].as_str().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let out = dir.join(format!("{id}.stdout"));
        let err = dir.join(format!("{id}.stderr"));
        let stdout = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out)
            .map_err(|e| e.to_string())?;
        let stderr = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&err)
            .map_err(|e| e.to_string())?;
        let args: Vec<_> = c["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        let mut command = Command::new(executable(program, workspace)?);
        command
            .args(args)
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                records.push(json!({"id":c["id"],"passed":false,"error":e.to_string()}));
                continue;
            }
        };
        let deadline =
            Instant::now() + Duration::from_secs(c["timeout_seconds"].as_u64().unwrap_or(120));
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                #[cfg(unix)]
                {
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                }
                let _ = child.kill();
                break child.wait().map_err(|e| e.to_string())?;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        records.push(json!({"id":c["id"],"command":c,"passed":status.success()&&!timed_out,"exit_code":status.code(),"timed_out":timed_out,"stdout_path":out.strip_prefix(workspace).unwrap_or(&out),"stderr_path":err.strip_prefix(workspace).unwrap_or(&err)}));
    }
    Ok(json!(records))
}

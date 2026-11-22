// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

use clap::Parser;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "formwork-report", version)]
#[command(about = "Generate an HTML status report for a Formwork workspace")]
struct Cli {
    #[arg(long, short, help = "Formwork workspace root (default: CWD)")]
    workspace: Option<String>,
    #[arg(long, short, help = "Write HTML to this file (default: stdout)")]
    output: Option<String>,
}

fn main() {
    let cli = Cli::parse();
    let root = cli.workspace.map(PathBuf::from).unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|e| {
            eprintln!("formwork-report: cannot determine workspace: {e}");
            std::process::exit(1);
        })
    });
    let html = build_report(&root).unwrap_or_else(|e| {
        eprintln!("formwork-report: {e}");
        std::process::exit(1);
    });
    match cli.output {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, &html) {
                eprintln!("formwork-report: cannot write {path}: {e}");
                std::process::exit(1);
            }
            eprintln!("Wrote {path} ({:.1} KB)", html.len() as f64 / 1024.0);
        }
        None => println!("{html}"),
    }
}

fn build_report(root: &std::path::Path) -> Result<String, String> {
    let path = root.join(".formwork/state.json");
    let bytes = std::fs::read(&path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let state: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Invalid JSON in {}: {e}", path.display()))?;
    if state["pipeline_version"] != 2 {
        return Err("Unsupported state protocol. Use the matching binary; this report does not migrate state.".into());
    }
    Ok(render_spec_report(&state))
}

fn render_spec_report(state: &Value) -> String {
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let stage = escape(state["stage"].as_str().unwrap_or("unknown"));
    let mut html = format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>Formwork status</title><style>body{{font:1rem/1.6 system-ui;max-width:70ch;margin:2rem auto;padding:1rem;color:#202020;background:#fff}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}</style><main><h1>Formwork status</h1><p>Spec-driven pipeline. Stage: <strong>{stage}</strong>.</p><p>This read-only report shows recorded state, not a fresh verification of candidate files.</p><h2>Milestone results</h2><ul>");
    for m in state["completed"].as_array().into_iter().flatten() {
        html.push_str(&format!(
            "<li>{}: recorded checks {}</li>",
            escape(m["milestone"].as_str().unwrap_or("unknown")),
            if m["verification"]["passed"] == true {
                "passed"
            } else {
                "not passed"
            }
        ));
    }
    html.push_str("</ul><h2>Latest verification</h2><pre>");
    html.push_str(&escape(
        &serde_json::to_string_pretty(&state["verification"]).unwrap(),
    ));
    html.push_str("</pre></main></html>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spec_report_escapes_state_and_labels_recorded_evidence() {
        let html = render_spec_report(
            &json!({"stage":"<script>","completed":[{"milestone":"<mvp>","verification":{"passed":true}}],"verification":{"message":"<unsafe>"}}),
        );
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&lt;mvp&gt;: recorded checks passed"));
        assert!(html.contains("&lt;unsafe&gt;"));
        assert!(html.contains("not a fresh verification"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn report_reads_only_current_protocol_without_writing_state() {
        let dir = tempfile::Builder::new()
            .prefix("report-")
            .tempdir_in("/tmp/opencode")
            .unwrap();
        let path = dir.path().join(".formwork/state.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for version in [Value::Null, json!(1), json!(3)] {
            let bytes = serde_json::to_vec(&json!({"pipeline_version":version})).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            assert!(build_report(dir.path())
                .unwrap_err()
                .contains("Unsupported state"));
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        let bytes =
            serde_json::to_vec(&json!({"pipeline_version":2,"stage":"story","completed":[]}))
                .unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(build_report(dir.path())
            .unwrap()
            .contains("Stage: <strong>story"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn report_rejects_missing_corrupt_and_unreadable_state() {
        let dir = tempfile::Builder::new()
            .prefix("report-errors-")
            .tempdir_in("/tmp/opencode")
            .unwrap();
        assert!(build_report(dir.path())
            .unwrap_err()
            .contains("Cannot read"));
        let path = dir.path().join(".formwork/state.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{").unwrap();
        assert!(build_report(dir.path())
            .unwrap_err()
            .contains("Invalid JSON"));
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(build_report(dir.path())
            .unwrap_err()
            .contains("Cannot read"));
    }
}

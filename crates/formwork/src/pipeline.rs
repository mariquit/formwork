// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

//! Four-agent protocol. Agents author specifications and tests; the binary owns approvals.
use crate::{blackboard::Blackboard, cli::Commands};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

#[derive(Debug)]
pub struct Error {
    pub reason: &'static str,
    pub message: String,
    pub instruction: &'static str,
}
type Result<T> = std::result::Result<T, Error>;
fn error(reason: &'static str, message: impl ToString) -> Error {
    Error { reason, message: message.to_string(), instruction: "Fix the reported input or explicitly reopen design with `formwork rollback`; do not edit state or weaken approved tests." }
}
fn nonempty(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.trim().is_empty())
}
fn strings(v: &Value) -> bool {
    v.as_array()
        .is_some_and(|a| !a.is_empty() && a.iter().all(nonempty))
}
fn slug(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn validate_roadmap(doc: &Value) -> std::result::Result<(), String> {
    let Some(roadmap) = doc.get("milestones") else {
        return Ok(());
    };
    let list = roadmap
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or("milestones must be a nonempty array")?;
    let mut ids = BTreeSet::new();
    let mut stretch = false;
    for (i, m) in list.iter().enumerate() {
        let id = m["id"].as_str().unwrap_or("");
        let kind = m["kind"].as_str().unwrap_or("");
        if !slug(id)
            || !ids.insert(id)
            || (i == 0 && kind != "mvp")
            || (i > 0 && !matches!(kind, "later" | "stretch"))
            || (stretch && kind != "stretch")
        {
            return Err(
                "Milestones need unique safe IDs, first mvp, then later, then stretch.".into(),
            );
        }
        stretch |= kind == "stretch";
        if ["title", "outcome", "rationale"]
            .iter()
            .any(|f| !nonempty(&m[*f]))
            || !strings(&m["exit_criteria"])
        {
            return Err(
                "Milestone outcome, rationale and observable exit criteria are required.".into(),
            );
        }
    }
    let mut used = BTreeSet::new();
    for f in doc["features"].as_array().into_iter().flatten() {
        let id = f["milestone"].as_str().unwrap_or("");
        if !ids.contains(id) {
            return Err("Every feature must name an approved milestone.".into());
        }
        used.insert(id);
    }
    if ids != used {
        return Err("Every milestone must contain feature scope.".into());
    }
    Ok(())
}

fn read(bb: &Blackboard, path: &str) -> Result<Value> {
    let p = safe_path(bb.workspace(), path)?;
    let bytes = std::fs::read(p).map_err(|e| error("artifact_unreadable", e))?;
    serde_json::from_slice(&bytes).map_err(|e| error("invalid_json", e))
}
fn safe_path(root: &Path, path: &str) -> Result<std::path::PathBuf> {
    if path.is_empty()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(error("unsafe_path", path));
    }
    let p = root.join(path);
    let canonical = p
        .canonicalize()
        .map_err(|e| error("artifact_unreadable", format!("{path}: {e}")))?;
    let workspace = root
        .canonicalize()
        .map_err(|e| error("workspace_unreadable", e))?;
    if !canonical.starts_with(workspace) || !canonical.is_file() {
        return Err(error("unsafe_path", path));
    }
    let mut cursor = root.to_path_buf();
    for part in Path::new(path).components() {
        cursor.push(part);
        if std::fs::symlink_metadata(&cursor)
            .map_err(|e| error("artifact_unreadable", e))?
            .file_type()
            .is_symlink()
        {
            return Err(error("unsafe_path", path));
        }
    }
    Ok(canonical)
}
fn digest(bb: &Blackboard, files: &[String]) -> Result<String> {
    let mut hash = Sha256::new();
    for path in files.iter().collect::<BTreeSet<_>>() {
        let bytes = std::fs::read(safe_path(bb.workspace(), path)?)
            .map_err(|e| error("artifact_unreadable", e))?;
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}
fn list(v: &Value) -> Result<Vec<String>> {
    if !strings(v) {
        return Err(error("invalid_file_list", "Expected nonempty file paths."));
    }
    Ok(v.as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect())
}
fn design_files(plan: &Value) -> Result<Vec<String>> {
    let mut files = vec![
        ".formwork/prd.json".into(),
        ".formwork/plan.json".into(),
        "SPEC.md".into(),
    ];
    files.extend(list(&plan["contracts"])?);
    if let Some(files_value) = plan.get("design_files") {
        files.extend(list(files_value)?);
    }
    Ok(files)
}
fn validate_plan(bb: &Blackboard) -> Result<(Value, String)> {
    let doc = crate::prd::load(bb.workspace()).map_err(|e| error("invalid_prd", e))?;
    if !doc["milestones"].is_array() {
        return Err(error(
            "missing_roadmap",
            "Approve explicit MVP/later/stretch scope before design.",
        ));
    }
    let plan = read(bb, ".formwork/plan.json")?;
    crate::schema::validate("plan", &plan).map_err(|e| error("invalid_plan", e))?;
    let work = plan["milestones"]
        .as_array()
        .ok_or_else(|| error("invalid_plan", "milestones required"))?;
    let roadmap = doc["milestones"].as_array().unwrap();
    if work.len() != roadmap.len() {
        return Err(error(
            "invalid_plan",
            "Work plan must cover exactly the approved milestones.",
        ));
    }
    for file in list(&plan["contracts"])? {
        if !plan["validators"].as_array().is_some_and(|a| {
            a.iter().any(|v| {
                v["contracts"]
                    .as_array()
                    .is_some_and(|c| c.contains(&json!(file)))
            })
        }) {
            return Err(error(
                "unvalidated_contract",
                format!("No validator claims contract {file}"),
            ));
        }
    }
    for (i, m) in work.iter().enumerate() {
        let features: BTreeSet<_> = doc["features"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["milestone"] == roadmap[i]["id"])
            .filter_map(|f| f["id"].as_str())
            .collect();
        let traces: BTreeSet<_> = m["features"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if m["id"] != roadmap[i]["id"]
            || features != traces
            || !nonempty(&m["work"])
            || !strings(&m["modules"])
        {
            return Err(error("invalid_plan", "Milestone order, exact feature coverage, work description and module list are required."));
        }
        for dep in m["depends_on"].as_array().into_iter().flatten() {
            if !work[..i].iter().any(|p| p["id"] == *dep) {
                return Err(error(
                    "future_dependency",
                    "Dependencies must name earlier milestones.",
                ));
            }
        }
    }
    let revision = digest(bb, &design_files(&plan)?)?;
    Ok((plan, revision))
}

/// Commands are explicitly approved executable inputs, not shell strings.
fn run(bb: &Blackboard, checks: &Value) -> Result<Value> {
    crate::execution::run(bb.workspace(), checks).map_err(|e| error("execution_unavailable", e))
}
fn passed(records: &Value) -> bool {
    records
        .as_array()
        .is_some_and(|a| !a.is_empty() && a.iter().all(|r| r["passed"] == true))
}
fn validate_contracts(bb: &Blackboard, plan: &Value) -> Result<Value> {
    // JSON Schema definitions are compiled, not merely parsed as JSON.
    for file in list(&plan["contracts"])? {
        if file.ends_with(".schema.json") {
            let schema = read(bb, &file)?;
            jsonschema::validator_for(&schema).map_err(|e| error("invalid_contract", e))?;
        }
    }
    let evidence = run(bb, &plan["validators"])?;
    if !passed(&evidence) {
        return Err(error("contract_validation_failed", evidence));
    }
    Ok(evidence)
}
fn stage(state: &Value) -> &str {
    state["stage"].as_str().unwrap_or("story")
}
fn role(state: &Value) -> Option<&'static str> {
    match stage(state) {
        "story" => Some("storyteller"),
        "design" | "refine" => Some("designer"),
        "review" | "rereview" | "tests" => Some("reviewer"),
        "implement" => Some("worker"),
        _ => None,
    }
}
fn set_stage(state: &mut Value, s: &str) {
    state["stage"] = json!(s);
    state["phase"] = json!(match s {
        "story" => "storytelling",
        "design" | "refine" => "design",
        "review" | "rereview" | "tests" => "design:review",
        "complete" => "build:complete",
        _ => "build",
    });
    state["outstanding_task"] = Value::Null;
}
fn protected(bb: &Blackboard, state: &Value) -> Result<()> {
    let (plan, revision) = validate_plan(bb)?;
    if state["design_revision"] != revision {
        return Err(error(
            "approved_design_changed",
            "Restore the reviewed design or reopen design.",
        ));
    }
    if state["test_revision"].is_string() {
        let tests = read(bb, ".formwork/tests.json")?;
        let mut files = list(&tests["files"])?;
        files.push(".formwork/tests.json".into());
        files.extend(design_files(&plan)?);
        if state["test_revision"] != digest(bb, &files)? {
            return Err(error(
                "protected_tests_changed",
                "Restore approved tests, fixtures and harness inputs.",
            ));
        }
        let tools = crate::execution::tools(bb.workspace(), &tests["checks"])
            .map_err(|e| error("validator_unavailable", e))?;
        if state["tools"] != tools {
            return Err(error("validation_environment_changed","Approved executable identities changed. Reopen design before refreshing test approval."));
        }
    }
    Ok(())
}
fn emit(bb: &Blackboard, mut state: Value) -> Result<Value> {
    if state["pipeline_version"] != 2 {
        return Err(error(
            "unsupported_state",
            "Use the matching binary or a separate workspace without overwriting existing state.",
        ));
    }
    if matches!(
        stage(&state),
        "implement" | "awaiting_approval" | "complete"
    ) {
        protected(bb, &state)?;
    }
    if stage(&state) == "complete" {
        let candidate = crate::execution::candidate(bb.workspace())
            .map_err(|e| error("candidate_unreadable", e))?;
        if state["verification"]["candidate_fingerprint"] != candidate
            || state["verification"]["environment_fingerprint"] != crate::execution::environment()
        {
            return Err(error(
                "stale_verification",
                "Candidate or execution environment changed since verification.",
            ));
        }
        if state["verification"]["evidence_fingerprint"]
            != digest(
                bb,
                &[state["verification"]["path"]
                    .as_str()
                    .ok_or_else(|| error("stale_verification", "Missing evidence path"))?
                    .into()],
            )?
        {
            return Err(error(
                "stale_verification",
                "Recorded evidence changed after verification.",
            ));
        }
        return Ok(
            json!({"status":"complete","verification":state["verification"],"limitations":"Passing configured checks is not proof of untested behavior."}),
        );
    }
    if stage(&state) == "blocked" {
        return Ok(
            json!({"status":"blocked","reason":"spec_refinement_exhausted","findings":state["findings"],"instruction":"Resolve missing product policy with the user, then explicitly reopen design."}),
        );
    }
    if stage(&state) == "awaiting_approval" {
        let plan = read(bb, ".formwork/plan.json")?;
        let i = state["milestone_index"].as_u64().unwrap_or(0) as usize;
        return Ok(
            json!({"status":"awaiting_approval","reason":"milestone_approval_required","milestone":plan["milestones"][i],"design_revision":state["design_revision"],"instruction":"Ask the user before authorizing execution. Roadmap approval is not execution approval.","next_command":format!("formwork approve-milestone {} --design-revision {}",plan["milestones"][i]["id"].as_str().unwrap(),state["design_revision"].as_str().unwrap())}),
        );
    }
    let agent = role(&state).ok_or_else(|| error("invalid_stage", stage(&state)))?;
    if !state["outstanding_task"].is_object() {
        state["outstanding_task"] =
            json!({"id":uuid::Uuid::new_v4().to_string(),"agent":agent,"stage":stage(&state)});
        bb.write_state(&state);
    }
    let registry = bb.artifacts(None);
    let mut task = json!({"status":"task","domain":agent,"task_id":state["outstanding_task"]["id"],"agent":agent,"stage":stage(&state),"phase":state["phase"],"design_revision":state["design_revision"],"findings":state["findings"],"idea":state["idea"],"context":[{"path":".formwork/prd.json","description":"Approved product scope"},{"path":"SPEC.md","description":"Canonical architecture and behavior"},{"path":".formwork/plan.json","description":"Shared contracts, validators and milestone work"}],"instruction":match stage(&state) {
        "story" => "Interview the user and obtain primary-session plan approval before writing .formwork/prd.json. Define explicit MVP/later/stretch milestones. Submit task_id and task_status: complete.",
        "design" | "refine" => "Read approved product scope. Design shared modules and canonical machine-readable contracts once, reusing existing capability. Write SPEC.md and .formwork/plan.json. For refine, fix only actionable review findings. Obtain primary-session approval before artifact writes. Submit task_id and task_status: complete.",
        "review" | "rereview" => "Fresh adversarial systems audit: find up to three actionable production failures, edge cases or missing contracts. Do not write application code. Submit verdict approve/reject, design_revision and findings array. Rereview only prior issues; unresolved blockers stop after one refinement.",
        "tests" => "Generate concrete unit tests and integration assertions from the final approved spec before application implementation. Write .formwork/tests.json with files (all protected tests/fixtures/harness), coverage (AC-ID to check IDs), readiness commands (discovery/collection, not application success), and checks (id, milestone, kind unit/integration/build, program,args,non_interactive). Require actual integration/build checks. Tests must use independent expected results, never a candidate-derived oracle. If expected behavior is ambiguous, submit task_status blocked and findings. Do not implement features.",
        _ => "Implement approved milestone using shared code and approved tests. Do not modify frozen specs, tests, fixtures or harness inputs. Submit task_id and task_status complete. The binary executes all checks through this milestone, including regression; failures return to you, not another LLM reviewer."
    }});
    task["registered_context"] = json!(registry.iter().map(|a| a.to_context()).collect::<Vec<_>>());
    task["scratch_path"] = json!(bb.root().join(crate::blackboard::SCRATCH_SUBDIR));
    if agent == "worker" {
        let p = read(bb, ".formwork/plan.json")?;
        task["milestone"] =
            p["milestones"][state["milestone_index"].as_u64().unwrap_or(0) as usize].clone();
        task["previous_verification"] = state["verification"].clone();
    }
    Ok(task)
}

fn submission(raw: Option<String>) -> Result<Value> {
    let raw = match raw {
        Some(s) if s.trim_start().starts_with('{') => s,
        Some(path) => {
            std::fs::read_to_string(path).map_err(|e| error("unreadable_submission", e))?
        }
        None => {
            use std::io::Read;
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| error("unreadable_submission", e))?;
            s
        }
    };
    serde_json::from_str(&raw).map_err(|e| error("invalid_submission", e))
}
fn submit(bb: &Blackboard, mut state: Value, payload: Value) -> Result<Value> {
    if state["pipeline_version"] != 2 {
        return Err(error(
            "unsupported_state",
            "Use the matching binary or a separate workspace without overwriting existing state.",
        ));
    }
    if !state["outstanding_task"]["id"].is_string()
        || payload["task_id"] != state["outstanding_task"]["id"]
        || state["outstanding_task"]["stage"] != stage(&state)
    {
        return Err(error("stale_task", "Submit only the current issued task."));
    }
    if payload["task_status"] == "blocked" {
        state["findings"] = payload["findings"].clone();
        set_stage(&mut state, "blocked");
        bb.write_state(&state);
        return emit(bb, state);
    }
    if payload["task_status"] != "complete" {
        return Err(error(
            "incomplete_task",
            "task_status must be complete or blocked",
        ));
    }
    match stage(&state).to_owned().as_str() {
        "story" => {
            let doc = read(bb, ".formwork/prd.json")?;
            crate::prd::validate(&doc).map_err(|e| error("invalid_prd", e))?;
            if !doc["milestones"].is_array() {
                return Err(error("missing_roadmap", "Explicit milestones required."));
            }
            state["story_revision"] = json!(digest(bb, &[".formwork/prd.json".into()])?);
            set_stage(&mut state, "design");
        }
        "design" | "refine" => {
            let refining = stage(&state) == "refine";
            if state["story_revision"] != digest(bb, &[".formwork/prd.json".into()])? {
                return Err(error(
                    "approved_scope_changed",
                    "Restore the approved PRD; design may not rewrite milestone scope.",
                ));
            }
            let (p, r) = validate_plan(bb)?;
            let contract_evidence = validate_contracts(bb, &p)?;
            let craft = match crate::design_lint::lint_workspace(
                bb.workspace(),
                crate::design_lint::Mode::Auto,
            ) {
                Ok(reports) => {
                    if reports.iter().any(|r| r.blocking_count() > 0) {
                        return Err(error(
                            "design_lint_failed",
                            json!(reports.iter().map(|r| r.to_json()).collect::<Vec<_>>()),
                        ));
                    }
                    json!(reports.iter().map(|r| r.to_json()).collect::<Vec<_>>())
                }
                Err(e) => json!({"status":"DEGRADED","error":e}),
            };
            state["design_validation"] = json!({"contracts":contract_evidence,"craft":craft});
            state["design_revision"] = json!(r);
            set_stage(&mut state, if refining { "rereview" } else { "review" });
        }
        "review" | "rereview" => {
            protected(bb, &state)?;
            if payload["design_revision"] != state["design_revision"] {
                return Err(error(
                    "stale_review",
                    "Review must echo the current design revision.",
                ));
            }
            let findings = payload["findings"]
                .as_array()
                .ok_or_else(|| error("invalid_findings", "findings array required"))?;
            if payload["verdict"] == "reject" {
                if findings.is_empty()
                    || findings.len() > 3
                    || findings
                        .iter()
                        .any(|f| !nonempty(&f["issue"]) || !nonempty(&f["remediation"]))
                {
                    return Err(error(
                        "invalid_findings",
                        "One to three actionable issue/remediation findings required.",
                    ));
                }
                state["findings"] = payload["findings"].clone();
                let next = if stage(&state) == "review" {
                    "refine"
                } else {
                    "blocked"
                };
                set_stage(&mut state, next);
            } else if payload["verdict"] == "approve" && findings.is_empty() {
                set_stage(&mut state, "tests");
            } else {
                return Err(error(
                    "invalid_verdict",
                    "Approve requires no unresolved findings.",
                ));
            }
        }
        "tests" => {
            protected(bb, &state)?;
            let tests = read(bb, ".formwork/tests.json")?;
            crate::schema::validate("tests", &tests)
                .map_err(|e| error("invalid_test_manifest", e))?;
            let files = list(&tests["files"])?;
            for f in &files {
                safe_path(bb.workspace(), f)?;
            }
            let checks = tests["checks"]
                .as_array()
                .filter(|a| !a.is_empty())
                .ok_or_else(|| error("missing_checks", "Tests must define checks."))?;
            let plan = read(bb, ".formwork/plan.json")?;
            let ids: BTreeSet<_> = checks.iter().filter_map(|c| c["id"].as_str()).collect();
            if ids.len() != checks.len() {
                return Err(error("invalid_checks", "Unique check IDs required."));
            }
            for m in plan["milestones"].as_array().unwrap() {
                let owned: Vec<_> = checks
                    .iter()
                    .filter(|c| c["milestone"] == m["id"])
                    .collect();
                if ["unit", "integration", "build"]
                    .iter()
                    .any(|kind| !owned.iter().any(|c| c["kind"] == *kind))
                {
                    return Err(error(
                        "missing_integration_checks",
                        "Every milestone requires unit, integration and build checks.",
                    ));
                }
            }
            if checks.iter().any(|c| {
                !plan["milestones"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["id"] == c["milestone"])
            }) {
                return Err(error("invalid_checks", "Unknown milestone in check."));
            }
            let doc = read(bb, ".formwork/prd.json")?;
            let index =
                crate::prd::trace_index(bb.workspace()).map_err(|e| error("invalid_prd", e))?;
            for id in tests["coverage"]
                .as_object()
                .ok_or_else(|| error("invalid_coverage", "coverage must be an object"))?
                .keys()
            {
                if !index.contains(id) {
                    return Err(error(
                        "invalid_coverage",
                        format!(
                            "Unknown criterion {id}; known requirement IDs: {:?}",
                            index.known()
                        ),
                    ));
                }
            }
            for ac in doc["acceptance_criteria"].as_array().unwrap() {
                let covered = list(&tests["coverage"][ac["id"].as_str().unwrap()])?;
                let owners: BTreeSet<_> = doc["user_stories"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| {
                        s["acceptance_criteria"]
                            .as_array()
                            .is_some_and(|a| a.contains(&ac["id"]))
                    })
                    .filter_map(|s| {
                        doc["features"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|f| f["id"] == s["feature"])
                    })
                    .filter_map(|f| f["milestone"].as_str())
                    .collect();
                if owners.len() != 1
                    || covered.iter().any(|id| {
                        !ids.contains(id.as_str())
                            || !checks.iter().any(|c| {
                                c["id"] == *id
                                    && owners.contains(c["milestone"].as_str().unwrap_or(""))
                            })
                    })
                {
                    return Err(error("invalid_coverage","Coverage references an unknown check, an orphan criterion or another milestone."));
                }
            }
            let mut protected_files = files;
            protected_files.push(".formwork/tests.json".into());
            protected_files.extend(design_files(&plan)?);
            let before_readiness = digest(bb, &protected_files)?;
            let readiness = run(bb, &tests["readiness"])?;
            if !passed(&readiness) {
                return Err(error("test_harness_failed", readiness));
            }
            if before_readiness != digest(bb, &protected_files)? {
                return Err(error(
                    "readiness_changed_inputs",
                    "Readiness commands changed the tests or specification.",
                ));
            }
            protected(bb, &state)?;
            state["tools"] = crate::execution::tools(bb.workspace(), &tests["checks"])
                .map_err(|e| error("validator_unavailable", e))?;
            state["test_revision"] = json!(digest(bb, &protected_files)?);
            state["milestone_index"] = json!(0);
            state["attempts"] = json!(0);
            set_stage(&mut state, "awaiting_approval");
        }
        "implement" => {
            protected(bb, &state)?;
            let i = state["milestone_index"].as_u64().unwrap_or(0) as usize;
            let plan = read(bb, ".formwork/plan.json")?;
            let tests = read(bb, ".formwork/tests.json")?;
            let allowed: BTreeSet<_> = plan["milestones"].as_array().unwrap()[..=i]
                .iter()
                .filter_map(|m| m["id"].as_str())
                .collect();
            let checks: Vec<_> = tests["checks"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| allowed.contains(c["milestone"].as_str().unwrap_or("")))
                .cloned()
                .collect();
            let candidate = crate::execution::candidate(bb.workspace())
                .map_err(|e| error("candidate_unreadable", e))?;
            let environment = crate::execution::environment();
            let result = run(bb, &json!(checks))?;
            // Revalidate after execution too: a check cannot silently rewrite its own oracle.
            protected(bb, &state)?;
            if candidate
                != crate::execution::candidate(bb.workspace())
                    .map_err(|e| error("candidate_unreadable", e))?
            {
                return Err(error(
                    "candidate_changed_during_checks",
                    "Checks changed application source; evidence cannot be accepted.",
                ));
            }
            let path = format!(".formwork/verification-{}-{}.json", i, uuid::Uuid::new_v4());
            let evidence = json!({"task_id":payload["task_id"],"candidate_fingerprint":candidate,"environment_fingerprint":environment,"test_revision":state["test_revision"],"design_revision":state["design_revision"],"checks":result});
            std::fs::write(
                bb.workspace().join(&path),
                serde_json::to_vec_pretty(&evidence).unwrap(),
            )
            .map_err(|e| error("evidence_write_failed", e))?;
            state["verification"] = json!({"path":path,"passed":passed(&result),"task_id":payload["task_id"],"candidate_fingerprint":candidate,"environment_fingerprint":environment,"test_revision":state["test_revision"],"recorded_at":chrono::Utc::now().to_rfc3339()});
            state["verification"]["evidence_fingerprint"] = json!(digest(bb, &[path])?);
            state["outstanding_task"] = Value::Null;
            if passed(&result) {
                let completion = json!({"phase":"build","milestone":plan["milestones"][i]["id"],"verification":state["verification"]});
                state["completed"].as_array_mut().unwrap().push(completion);
                state["attempts"] = json!(0);
                if i + 1 == plan["milestones"].as_array().unwrap().len() {
                    set_stage(&mut state, "complete");
                } else {
                    state["milestone_index"] = json!(i + 1);
                    set_stage(&mut state, "awaiting_approval");
                }
            } else {
                let attempts = state["attempts"].as_u64().unwrap_or(0) + 1;
                state["attempts"] = json!(attempts);
                if attempts >= 3 {
                    set_stage(&mut state, "blocked");
                    state["findings"] = json!([{"issue":"Implementation failed three verification attempts","remediation":"Inspect recorded execution evidence and resolve with the user."}]);
                }
            }
        }
        _ => {
            return Err(error(
                "wrong_stage",
                "No submission is allowed at this stage.",
            ))
        }
    }
    bb.write_state(&state);
    if stage(&state) == "blocked" {
        bb.add_blocker("delivery_blocked","Delivery needs user resolution","Inspect findings and recorded verification evidence before explicitly reopening design.",None);
        state = bb.read_state();
    }
    let artifacts: Vec<_> = payload["artifacts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(crate::blackboard::ArtifactMeta::from_value)
        .collect();
    if !artifacts.is_empty() {
        bb.record_artifacts("delivery", None, &artifacts);
        state = bb.read_state();
    }
    emit(bb, state)
}

pub fn dispatch(bb: &Blackboard, command: Commands) -> Result<Value> {
    let mut state = bb.read_state();
    match command {
        Commands::Start { idea } => {
            if state["phase"] != "init" {
                return Err(error("project_exists", "Resume the existing project."));
            }
            state["pipeline_version"] = json!(2);
            state["idea"] = json!(idea.join(" "));
            state["target"] = json!(crate::models::resolve_target(None));
            set_stage(&mut state, "story");
            bb.write_state(&state);
            emit(bb, state)
        }
        Commands::Status => Ok(
            json!({"phase":state["phase"],"stage":state["stage"],"pipeline_version":state["pipeline_version"],"agent":role(&state),"verification":state["verification"],"completed":state["completed"],"next_command":"formwork task"}),
        ),
        Commands::Log => Ok(json!({"completed":state["completed"]})),
        Commands::Rollback => {
            if state["pipeline_version"] != 2 {
                return Err(error("unsupported_state", "Use the matching binary or a separate workspace without overwriting existing state."));
            }
            if !state["delivery_history"].is_array() {
                state["delivery_history"] = json!([]);
            }
            let history = json!({"completed":state["completed"],"verification":state["verification"],"milestone_approvals":state["milestone_approvals"],"design_revision":state["design_revision"],"test_revision":state["test_revision"]});
            state["delivery_history"]
                .as_array_mut()
                .unwrap()
                .push(history);
            state["verification"] = Value::Null;
            state["design_revision"] = Value::Null;
            state["test_revision"] = Value::Null;
            state["milestone_approvals"] = json!({});
            state["completed"] = json!([]);
            state["findings"] = json!([]);
            set_stage(&mut state, "design");
            bb.write_state(&state);
            emit(bb, state)
        }
        Commands::ApproveMilestone {
            id,
            design_revision,
        } => {
            if state["pipeline_version"] != 2 || stage(&state) != "awaiting_approval" {
                return Err(error(
                    "milestone_not_awaiting_approval",
                    "No milestone is awaiting approval.",
                ));
            }
            protected(bb, &state)?;
            let plan = read(bb, ".formwork/plan.json")?;
            let i = state["milestone_index"].as_u64().unwrap_or(0) as usize;
            if state["design_revision"] != design_revision || plan["milestones"][i]["id"] != id {
                return Err(error(
                    "stale_milestone_approval",
                    "Authorize only the next milestone and current design revision.",
                ));
            }
            if !state["milestone_approvals"].is_object() {
                state["milestone_approvals"] = json!({});
            }
            state["milestone_approvals"][&id] = json!({"design_revision":design_revision,"test_revision":state["test_revision"],"timestamp":chrono::Utc::now().to_rfc3339()});
            set_stage(&mut state, "implement");
            bb.write_state(&state);
            emit(bb, state)
        }
        Commands::Submit { artifacts } => submit(bb, state, submission(artifacts)?),
        Commands::Subagent { name } => {
            if !crate::OPENCODE_SUBAGENTS.iter().any(|(n, _)| *n == name) {
                return Err(error(
                    "unknown_agent",
                    "Only storyteller, designer, reviewer and worker are available.",
                ));
            }
            if role(&state) != Some(name.as_str()) {
                return Err(error(
                    "wrong_task_role",
                    "Delegate the role from formwork task.",
                ));
            }
            emit(bb, state)
        }
        Commands::Task | Commands::Resume | Commands::Next => emit(bb, state),
        Commands::DesignLint { .. } | Commands::Setup { .. } => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Project {
        bb: Blackboard,
    }
    impl Project {
        fn new() -> Self {
            let root = std::path::PathBuf::from("/tmp/opencode")
                .join(format!("pivot-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(root.join(".formwork")).unwrap();
            Self {
                bb: Blackboard::new(Some(root.to_str().unwrap())),
            }
        }
        fn write(&self, path: &str, value: &str) {
            let p = self.bb.workspace().join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, value).unwrap();
        }
        fn json(&self, path: &str, value: Value) {
            self.write(path, &value.to_string());
        }
        fn task(&self) -> Value {
            dispatch(&self.bb, Commands::Task).unwrap()
        }
        fn submit(&self, extra: Value) -> Result<Value> {
            let task = self.task();
            let mut p = json!({"task_id":task["task_id"],"task_status":"complete"});
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            submit(&self.bb, self.bb.read_state(), p)
        }
        fn start_design(&self) {
            dispatch(
                &self.bb,
                Commands::Start {
                    idea: vec!["Counter".into()],
                },
            )
            .unwrap();
            self.json(".formwork/prd.json",json!({"version":1,"destination":"A usable counter","context":{"problem":"Count reliably","users":"Operators","constraints":[]},"milestones":[{"id":"mvp","kind":"mvp","title":"Counter","outcome":"Increment values","rationale":"Core outcome","exit_criteria":["Increment works"]},{"id":"later","kind":"later","title":"Reset","outcome":"Reset values","rationale":"Optional convenience","exit_criteria":["Reset works"]}],"features":[{"id":"FEAT-1","title":"Increment","milestone":"mvp"},{"id":"FEAT-2","title":"Reset","milestone":"later"}],"user_stories":[{"id":"US-1","feature":"FEAT-1","acceptance_criteria":["AC-1"]},{"id":"US-2","feature":"FEAT-2","acceptance_criteria":["AC-2"]}],"acceptance_criteria":[{"id":"AC-1","statement":"Increment returns next value","verify":["Increment 1; observe 2"]},{"id":"AC-2","statement":"Reset returns zero","verify":["Reset 2; observe 0"]}],"non_goals":[],"open_questions":[]}));
            let mut doc = read(&self.bb, ".formwork/prd.json").unwrap();
            doc["summary"] = json!("Counter operations");
            doc["existing_code"] = json!([]);
            doc["journeys"] = json!([{ "id":"JRN-1","title":"Count","steps":["Increment value"]}]);
            doc["behavior"] = json!([{ "id":"BH-1","statement":"Increment adds one"}]);
            for story in doc["user_stories"].as_array_mut().unwrap() {
                story["story"] = json!("As an operator, I can count values.");
                story["priority"] = json!("P0");
            }
            self.json(".formwork/prd.json", doc);
            self.submit(json!({})).unwrap();
            self.write("SPEC.md", "Increment adds one. Reset returns zero.");
            self.json("counter.schema.json", json!({"type":"integer"}));
            self.json(".formwork/plan.json",json!({"contracts":["counter.schema.json"],"validators":[{"program":"python3","args":["-c","import json; json.load(open('counter.schema.json'))"],"contracts":["counter.schema.json"],"non_interactive":true}],"milestones":[{"id":"mvp","features":["FEAT-1"],"work":"Implement shared counter","modules":["counter.py"]},{"id":"later","features":["FEAT-2"],"work":"Add reset using shared module","modules":["counter.py"],"depends_on":["mvp"]}]}));
        }
        fn approve_review(&self) {
            let t = self.task();
            self.submit(
                json!({"verdict":"approve","findings":[],"design_revision":t["design_revision"]}),
            )
            .unwrap();
        }
        fn prepare_tests(&self) {
            self.write("test_counter.py","import unittest\nclass CounterTests(unittest.TestCase):\n def test_increment(self):\n  from counter import increment\n  self.assertEqual(increment(1),2)\n def test_reset(self):\n  from counter import reset\n  self.assertEqual(reset(9),0)\nif __name__=='__main__': unittest.main()\n");
            let mut checks = Vec::new();
            for (m, name) in [("mvp", "increment"), ("later", "reset")] {
                for kind in ["unit", "integration", "build"] {
                    let args = if kind == "build" {
                        json!(["-c", "import ast; ast.parse(open('counter.py').read())"])
                    } else {
                        json!([
                            "-m",
                            "unittest",
                            format!("test_counter.CounterTests.test_{name}")
                        ])
                    };
                    checks.push(json!({"id":format!("{m}-{kind}"),"milestone":m,"kind":kind,"program":"python3","args":args,"non_interactive":true}));
                }
            }
            self.json(".formwork/tests.json",json!({"files":["test_counter.py"],"coverage":{"AC-1":["mvp-unit"],"AC-2":["later-unit"]},"readiness":[{"program":"python3","args":["-c","import unittest; s=unittest.defaultTestLoader.loadTestsFromName('test_counter'); assert s.countTestCases()==2"],"non_interactive":true}],"checks":checks}));
        }
        fn ready(&self) {
            self.start_design();
            self.submit(json!({})).unwrap();
            self.approve_review();
            self.prepare_tests();
            self.submit(json!({})).unwrap();
        }
        fn authorize(&self, id: &str) -> Value {
            let state = self.bb.read_state();
            dispatch(
                &self.bb,
                Commands::ApproveMilestone {
                    id: id.into(),
                    design_revision: state["design_revision"].as_str().unwrap().into(),
                },
            )
            .unwrap()
        }
    }
    impl Drop for Project {
        fn drop(&mut self) {
            std::fs::remove_dir_all(self.bb.workspace()).unwrap();
        }
    }

    #[test]
    fn pivot_happy_path_gates_each_milestone_and_runs_regressions() {
        let p = Project::new();
        p.ready();
        assert_eq!(p.task()["reason"], "milestone_approval_required");
        assert_eq!(p.authorize("mvp")["agent"], "worker");
        p.write("counter.py", "def increment(n): return n+1\n");
        let next = p.submit(json!({})).unwrap();
        assert_eq!(next["status"], "awaiting_approval");
        assert_eq!(next["milestone"]["id"], "later");
        p.authorize("later");
        p.write(
            "counter.py",
            "def increment(n): return n\ndef reset(n): return 0\n",
        );
        let failure = p.submit(json!({})).unwrap();
        assert_eq!(failure["agent"], "worker");
        assert_eq!(failure["previous_verification"]["passed"], false);
        p.write(
            "counter.py",
            "def increment(n): return n+1\ndef reset(n): return 0\n",
        );
        let done = p.submit(json!({})).unwrap();
        assert_eq!(done["status"], "complete");
        assert_eq!(p.bb.read_state()["completed"].as_array().unwrap().len(), 2);
        p.write("counter.py", "def increment(n): return 42\n");
        assert_eq!(
            dispatch(&p.bb, Commands::Resume).unwrap_err().reason,
            "stale_verification"
        );
    }

    #[test]
    fn pivot_review_allows_one_refinement_then_blocks() {
        let p = Project::new();
        p.start_design();
        p.submit(json!({})).unwrap();
        let critique =
            json!([{"issue":"Overflow unspecified","remediation":"Specify checked arithmetic"}]);
        let t = p.task();
        assert_eq!(p.submit(json!({"verdict":"reject","findings":critique,"design_revision":t["design_revision"]})).unwrap()["stage"],"refine");
        p.write("SPEC.md", "Increment checks overflow.");
        p.submit(json!({})).unwrap();
        let t = p.task();
        assert_eq!(t["stage"], "rereview");
        assert_eq!(p.submit(json!({"verdict":"reject","findings":critique,"design_revision":t["design_revision"]})).unwrap()["status"],"blocked");
        assert_eq!(p.bb.read_state()["stage"], "blocked");
    }

    #[test]
    fn pivot_stale_task_review_and_changed_product_scope_reject() {
        let p = Project::new();
        p.start_design();
        assert_eq!(
            submit(
                &p.bb,
                p.bb.read_state(),
                json!({"task_id":"old","task_status":"complete"})
            )
            .unwrap_err()
            .reason,
            "stale_task"
        );
        p.submit(json!({})).unwrap();
        assert_eq!(
            p.submit(json!({"verdict":"approve","findings":[],"design_revision":"old"}))
                .unwrap_err()
                .reason,
            "stale_review"
        );
        let mut doc = read(&p.bb, ".formwork/prd.json").unwrap();
        doc["destination"] = json!("Changed product");
        p.json(".formwork/prd.json", doc);
        assert_eq!(p.submit(json!({"verdict":"approve","findings":[],"design_revision":p.task()["design_revision"]})).unwrap_err().reason,"approved_design_changed");
    }

    #[test]
    fn pivot_missing_coverage_and_broken_harness_do_not_freeze() {
        let p = Project::new();
        p.start_design();
        p.submit(json!({})).unwrap();
        p.approve_review();
        p.prepare_tests();
        let mut t = read(&p.bb, ".formwork/tests.json").unwrap();
        t["coverage"]["AC-1"] = json!(["later-unit"]);
        p.json(".formwork/tests.json", t.clone());
        assert_eq!(p.submit(json!({})).unwrap_err().reason, "invalid_coverage");
        t["coverage"]["AC-1"] = json!(["mvp-unit"]);
        t["readiness"][0]["args"] = json!(["-c", "raise RuntimeError('broken collection')"]);
        p.json(".formwork/tests.json", t);
        assert_eq!(
            p.submit(json!({})).unwrap_err().reason,
            "test_harness_failed"
        );
        assert!(p.bb.read_state()["test_revision"].is_null());
    }

    #[test]
    fn pivot_frozen_tests_and_early_or_stale_approval_reject() {
        let p = Project::new();
        p.ready();
        let state = p.bb.read_state();
        for (id, revision) in [
            ("later", state["design_revision"].as_str().unwrap()),
            ("mvp", "old"),
        ] {
            assert_eq!(
                dispatch(
                    &p.bb,
                    Commands::ApproveMilestone {
                        id: id.into(),
                        design_revision: revision.into()
                    }
                )
                .unwrap_err()
                .reason,
                "stale_milestone_approval"
            );
        }
        p.authorize("mvp");
        p.write("test_counter.py", "# weakened tests");
        assert_eq!(
            dispatch(&p.bb, Commands::Task).unwrap_err().reason,
            "protected_tests_changed"
        );
    }

    #[test]
    fn unsupported_state_never_becomes_current_approval() {
        let p = Project::new();
        let mut state = p.bb.read_state();
        state["phase"] = json!("build");
        p.bb.write_state(&state);
        let before = std::fs::read(p.bb.root().join("state.json")).unwrap();
        assert_eq!(
            p.bb.try_read_state().unwrap_err().reason,
            "unsupported_state"
        );
        assert_eq!(
            std::fs::read(p.bb.root().join("state.json")).unwrap(),
            before
        );
        let current = Project::new();
        dispatch(
            &current.bb,
            Commands::Start {
                idea: vec!["Counter".into()],
            },
        )
        .unwrap();
        assert_eq!(
            dispatch(
                &current.bb,
                Commands::Subagent {
                    name: "unknown".into()
                }
            )
            .unwrap_err()
            .reason,
            "unknown_agent"
        );
    }

    #[test]
    fn pivot_timeout_and_missing_tool_are_not_success() {
        let p = Project::new();
        let result=run(&p.bb,&json!([{"program":"python3","args":["-c","import time;time.sleep(5)"],"non_interactive":true,"timeout_seconds":1}])).unwrap();
        assert!(!passed(&result));
        assert_eq!(result[0]["timed_out"], true);
        assert_eq!(
            run(
                &p.bb,
                &json!([{"program":"formwork-nonexistent-tool","args":[],"non_interactive":true}])
            )
            .unwrap_err()
            .reason,
            "execution_unavailable"
        );
    }

    #[test]
    fn task_omits_model() {
        let p = Project::new();
        dispatch(
            &p.bb,
            Commands::Start {
                idea: vec!["Counter".into()],
            },
        )
        .unwrap();
        let task = p.task();
        assert_eq!(task["agent"], "storyteller");
        assert!(
            task.get("model").is_none(),
            "task JSON must not stamp a role model"
        );
    }

    #[test]
    fn pivot_binary_emits_one_json_object_and_rejects_unknown_agents() {
        let p = Project::new();
        let binary =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/formwork");
        let output = std::process::Command::new(&binary)
            .args(["start", "Counter"])
            .env("FORMWORK_WORKSPACE", p.bb.workspace())
            .env("FORMWORK_DESIGN_LINT", "static")
            .env_remove("FORMWORK_TARGET")
            .output()
            .unwrap();
        assert!(output.status.success());
        let task: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(task["agent"], "storyteller");
        assert!(task.get("model").is_none());
        assert!(task.get("design_style").is_none());
        assert!(task.get("legacy_sources").is_none());
        assert!(task.get("transcription_required").is_none());
        crate::schema::validate("task", &task).unwrap();
        let output = std::process::Command::new(&binary)
            .args(["subagent", "unknown"])
            .env("FORMWORK_WORKSPACE", p.bb.workspace())
            .env("FORMWORK_DESIGN_LINT", "static")
            .env_remove("FORMWORK_TARGET")
            .output()
            .unwrap();
        let rejection: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(rejection["reason"], "unknown_agent");
        crate::schema::validate("state", &p.bb.read_state()).unwrap();
    }

    #[test]
    fn pivot_invalid_contract_and_uncovered_contract_reject() {
        let p = Project::new();
        p.start_design();
        p.json(
            "counter.schema.json",
            json!({"type":"not-a-json-schema-type"}),
        );
        assert_eq!(p.submit(json!({})).unwrap_err().reason, "invalid_contract");
        p.json("counter.schema.json", json!({"type":"integer"}));
        let mut plan = read(&p.bb, ".formwork/plan.json").unwrap();
        plan["validators"][0]["contracts"] = json!(["other.schema.json"]);
        p.json(".formwork/plan.json", plan);
        assert_eq!(
            p.submit(json!({})).unwrap_err().reason,
            "unvalidated_contract"
        );
    }

    #[test]
    fn pivot_missing_integration_and_mutating_checks_reject() {
        let p = Project::new();
        p.start_design();
        p.submit(json!({})).unwrap();
        p.approve_review();
        p.prepare_tests();
        let mut tests = read(&p.bb, ".formwork/tests.json").unwrap();
        let original = tests.clone();
        tests["checks"]
            .as_array_mut()
            .unwrap()
            .retain(|c| c["kind"] != "integration");
        p.json(".formwork/tests.json", tests);
        assert_eq!(
            p.submit(json!({})).unwrap_err().reason,
            "missing_integration_checks"
        );
        p.json(".formwork/tests.json", original);
        p.submit(json!({})).unwrap();
        p.authorize("mvp");
        // Candidate module code runs through the frozen independent harness.
        p.write(
            "counter.py",
            "open('test_counter.py','w').write('# tampered')\ndef increment(n): return n+1\n",
        );
        assert_eq!(
            p.submit(json!({})).unwrap_err().reason,
            "protected_tests_changed"
        );
    }

    #[test]
    fn pivot_rollback_preserves_prior_results_and_invalidates_approvals() {
        let p = Project::new();
        p.ready();
        p.authorize("mvp");
        p.write("counter.py", "def increment(n): return n+1\n");
        p.submit(json!({})).unwrap();
        dispatch(&p.bb, Commands::Rollback).unwrap();
        let state = p.bb.read_state();
        assert_eq!(state["stage"], "design");
        assert!(state["test_revision"].is_null());
        assert!(state["milestone_approvals"].as_object().unwrap().is_empty());
        assert_eq!(
            state["delivery_history"][0]["completed"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn pivot_three_failed_checks_block_instead_of_looping_forever() {
        let p = Project::new();
        p.ready();
        p.authorize("mvp");
        p.write("counter.py", "def increment(n): return n\n");
        for attempt in 1..=3 {
            let response = p.submit(json!({})).unwrap();
            assert_eq!(
                response["status"],
                if attempt == 3 { "blocked" } else { "task" }
            );
        }
        assert!(p.bb.read_state()["completed"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn pivot_protected_paths_reject_symlinks_and_traversal() {
        let p = Project::new();
        p.write("oracle.txt", "expected result");
        assert_eq!(
            safe_path(p.bb.workspace(), "../oracle.txt")
                .unwrap_err()
                .reason,
            "unsafe_path"
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                p.bb.workspace().join("oracle.txt"),
                p.bb.workspace().join("linked.txt"),
            )
            .unwrap();
            assert_eq!(
                safe_path(p.bb.workspace(), "linked.txt")
                    .unwrap_err()
                    .reason,
                "unsafe_path"
            );
        }
    }
}

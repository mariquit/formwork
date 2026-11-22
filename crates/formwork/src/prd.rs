// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

//! `.formwork/prd.json` — the storyteller's product requirements document.
//!
//! The stable id spaces (`FEAT-`, `US-`, `AC-`, `NG-`,
//! `UN-`) are the contract later phases cite through feature and test coverage,
//! so a design that builds something nobody asked for is
//! detectable rather than a judgement call.
//!
//! Validation here is deliberately MECHANICAL — ids unique, cross-references
//! resolve, required fields non-empty. Whether an acceptance criterion is
//! genuinely observable, or a journey actually makes sense, is the reviewer
//! agent's job. The binary checks what it can check exactly.

use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

pub const PRD_PATH: &str = ".formwork/prd.json";

fn nonempty_str(v: Option<&Value>) -> bool {
    v.and_then(|v| v.as_str())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// Collect `id` values from an array field, requiring each to be a non-empty
/// string carrying the expected prefix and to be unique across the document.
fn collect_ids(
    doc: &Value,
    field: &str,
    prefix: &str,
    seen: &mut HashSet<String>,
    out: &mut HashSet<String>,
) -> Result<(), String> {
    let arr = match doc.get(field).and_then(|v| v.as_array()) {
        Some(a) => a,
        // Absent is handled by the required-field checks; an absent optional
        // list simply contributes no ids.
        None => return Ok(()),
    };
    for (i, entry) in arr.iter().enumerate() {
        let id = match entry.get("id").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                return Err(format!(
                    "{}: entry {} in `{}` has no `id`. Every entry needs a stable id ({}N) because later phases cite it.",
                    PRD_PATH, i + 1, field, prefix
                ));
            }
        };
        if !id.starts_with(prefix) {
            return Err(format!(
                "{}: `{}` id `{}` must start with `{}` — the id spaces are how a slice's `traces_to` is resolved.",
                PRD_PATH, field, id, prefix
            ));
        }
        // The suffix must be a literal integer. Both the prompt and the schema
        // declare `<PREFIX>-<N>`, so `FEAT-X` or a bare `FEAT-` passing the
        // prefix test alone made the numbering contract unenforced — and the
        // literal `AC-N` placeholder from the instruction is exactly the value a
        // model emits when it copies the template instead of numbering.
        let suffix = &id[prefix.len()..];
        if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!(
                "{}: `{}` id `{}` must be `{}<N>` with N a literal integer numbered from 1 (e.g. `{}1`). Never emit the letter `N` itself.",
                PRD_PATH, field, id, prefix, prefix
            ));
        }
        if !seen.insert(id.clone()) {
            return Err(format!(
                "{}: duplicate id `{}`. Ids must be unique across the whole document.",
                PRD_PATH, id
            ));
        }
        out.insert(id);
    }
    Ok(())
}

/// Every requirement identifier that test coverage may reference.
pub struct TraceIndex {
    ids: HashSet<String>,
}

impl TraceIndex {
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id.trim())
    }

    /// Rendered for error messages, sorted so the output is deterministic.
    pub fn known(&self) -> Vec<String> {
        let mut v: Vec<String> = self.ids.iter().cloned().collect();
        v.sort();
        v
    }
}

/// Read and mechanically validate the PRD.
pub fn load(workspace: &Path) -> Result<Value, String> {
    let path = workspace.join(PRD_PATH);
    if !path.exists() {
        return Err(format!(
            "Missing {}. The story phase must produce the product requirements document before the design phase can start.",
            PRD_PATH
        ));
    }
    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("Cannot read {}: {}", PRD_PATH, e))?;
    let doc: Value =
        serde_json::from_str(&raw).map_err(|e| format!("{} is not valid JSON: {}", PRD_PATH, e))?;
    validate(&doc)?;
    Ok(doc)
}

/// The document-shape half of [`load`], split out so it is unit-testable
/// without touching the filesystem.
pub fn validate(doc: &Value) -> Result<(), String> {
    crate::pipeline::validate_roadmap(doc)?;
    if !doc.is_object() {
        return Err(format!("{} must be a JSON object.", PRD_PATH));
    }

    // The destination fixes the scope: it is agreed in interview round 1 and
    // every later requirement is judged against it, so it is required rather
    // than nice-to-have.
    if !nonempty_str(doc.get("destination")) {
        return Err(format!(
            "{}: missing `destination` — one or two lines stating what reaching the end of this effort looks like. It fixes the scope, so nothing else can be judged without it.",
            PRD_PATH
        ));
    }
    if !nonempty_str(doc.get("summary")) {
        return Err(format!("{}: missing `summary`.", PRD_PATH));
    }

    for field in ["features", "user_stories", "acceptance_criteria"] {
        match doc.get(field).and_then(|v| v.as_array()) {
            Some(a) if !a.is_empty() => {}
            _ => {
                return Err(format!(
                    "{}: `{}` must be a non-empty array.",
                    PRD_PATH, field
                ));
            }
        }
    }

    // An absent fog list is not the same as an empty one. Requiring the key
    // makes "nobody looked" distinguishable from "fully specified" — the same
    // reason the markdown version required the section to be explicit.
    for field in ["non_goals", "open_questions"] {
        if !doc.get(field).map(|v| v.is_array()).unwrap_or(false) {
            return Err(format!(
                "{}: `{}` must be present as an array (use `[]` when there are none, so a later phase can tell 'none' from 'nobody looked').",
                PRD_PATH, field
            ));
        }
    }

    // `context` carries the problem, the user, and the constraints. The
    // constraints in particular are the stack authority every later phase reads:
    // the designer is told not to re-litigate what the story settled, and the
    // reviewer flags a design that contradicts it. Absent, both silently fall
    // back to "nothing was settled".
    match doc.get("context") {
        Some(c) if c.is_object() => {
            for field in ["problem", "users"] {
                if !nonempty_str(c.get(field)) {
                    return Err(format!(
                        "{}: `context.{}` is missing or empty. The design phase is judged against the problem and the user; neither can be inferred later.",
                        PRD_PATH, field
                    ));
                }
            }
            if !c.get("constraints").map(|v| v.is_array()).unwrap_or(false) {
                return Err(format!(
                    "{}: `context.constraints` must be present as an array (use `[]` when nothing is constrained). It is the stack authority the designer must not re-litigate, so 'none' and 'nobody looked' cannot read the same.",
                    PRD_PATH
                ));
            }
        }
        _ => {
            return Err(format!(
                "{}: missing `context` — {{problem, users, constraints[]}}. Later phases are judged against it.",
                PRD_PATH
            ));
        }
    }

    // `existing_code` is the written output of the research step, and the
    // designer is required to anchor every slice to it. An empty array is the
    // legitimate greenfield answer; an absent key means nobody looked, and a
    // slice can then specify a parallel reimplementation of what the repo has.
    match doc.get("existing_code").and_then(|v| v.as_array()) {
        Some(entries) => {
            for (i, e) in entries.iter().enumerate() {
                for field in ["path", "why"] {
                    if !nonempty_str(e.get(field)) {
                        return Err(format!(
                            "{}: `existing_code[{}]` needs a non-empty `{}` — name the specific file and the specific reason, never a bare directory.",
                            PRD_PATH, i, field
                        ));
                    }
                }
            }
        }
        None => {
            return Err(format!(
                "{}: `existing_code` must be present as an array of {{path, why}} (use `[]` for greenfield). It is the research record every slice is anchored to, so 'greenfield' and 'nobody looked' cannot read the same.",
                PRD_PATH
            ));
        }
    }

    // Journeys and behaviour are non-empty in the prompt's contract: they are
    // what makes the acceptance criteria reviewable as a coherent product rather
    // than a checklist.
    for (field, prefix, what) in [
        ("journeys", "JRN-", "how the user discovers, onboards, uses"),
        ("behavior", "BH-", "what the system does, not how"),
    ] {
        match doc.get(field).and_then(|v| v.as_array()) {
            Some(a) if !a.is_empty() => {}
            _ => {
                return Err(format!(
                    "{}: `{}` must be a non-empty array of entries with `{}` ids — {}.",
                    PRD_PATH, field, prefix, what
                ));
            }
        }
    }

    let mut seen = HashSet::new();
    let mut features = HashSet::new();
    let mut stories = HashSet::new();
    let mut criteria = HashSet::new();
    let mut sink = HashSet::new();
    collect_ids(doc, "features", "FEAT-", &mut seen, &mut features)?;
    collect_ids(doc, "user_stories", "US-", &mut seen, &mut stories)?;
    collect_ids(doc, "acceptance_criteria", "AC-", &mut seen, &mut criteria)?;
    collect_ids(doc, "non_goals", "NG-", &mut seen, &mut sink)?;
    collect_ids(doc, "open_questions", "UN-", &mut seen, &mut sink)?;
    // Journeys and behaviour are not `traces_to` targets, but their ids are still
    // cited in prose and must be well-formed and unique like every other space.
    collect_ids(doc, "journeys", "JRN-", &mut seen, &mut sink)?;
    collect_ids(doc, "behavior", "BH-", &mut seen, &mut sink)?;

    // Cross-references inside the document must resolve, or a slice tracing to
    // a story that cites a missing criterion inherits a broken chain.
    for story in doc
        .get("user_stories")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let id = story.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if !nonempty_str(story.get("story")) {
            return Err(format!(
                "{}: user story `{}` has no `story` text.",
                PRD_PATH, id
            ));
        }
        // `feature` is REQUIRED, not merely resolvable-when-present. Checking it
        // only when present meant a story with no feature at all passed, which
        // breaks the promised FEAT → US → AC chain in the one direction nothing
        // downstream can detect: the slice traces to the story, and the story
        // traces to nothing.
        let feature = story
            .get("feature")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        if feature.is_empty() {
            return Err(format!(
                "{}: user story `{}` names no `feature`. Every story must belong to a `FEAT-N` — that is the traceability chain later phases cite.",
                PRD_PATH, id
            ));
        }
        if !features.contains(feature) {
            return Err(format!(
                "{}: user story `{}` references feature `{}`, which is not defined in `features`.",
                PRD_PATH, id, feature
            ));
        }
        match story.get("acceptance_criteria").and_then(|v| v.as_array()) {
            Some(refs) if !refs.is_empty() => {
                for r in refs {
                    let r = r.as_str().unwrap_or("").trim();
                    if !criteria.contains(r) {
                        return Err(format!(
                            "{}: user story `{}` cites acceptance criterion `{}`, which is not defined in `acceptance_criteria`.",
                            PRD_PATH, id, r
                        ));
                    }
                }
            }
            _ => {
                return Err(format!(
                    "{}: user story `{}` must cite at least one `acceptance_criteria` id — a story with no testable criterion cannot be verified.",
                    PRD_PATH, id
                ));
            }
        }
    }

    for ac in doc
        .get("acceptance_criteria")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let id = ac.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if !nonempty_str(ac.get("statement")) {
            return Err(format!(
                "{}: acceptance criterion `{}` has no `statement`.",
                PRD_PATH, id
            ));
        }
        // `verify` is the manual proof the criterion is observable. Without it a
        // criterion states an outcome nobody can check, and both the prompt and
        // the generated schema describe it as required — so enforce it here
        // rather than letting the contract be aspirational.
        match ac.get("verify").and_then(|v| v.as_array()) {
            Some(steps) if !steps.is_empty() => {
                for (i, s) in steps.iter().enumerate() {
                    if s.as_str().map(|s| s.trim().is_empty()).unwrap_or(true) {
                        return Err(format!(
                            "{}: acceptance criterion `{}` has an empty `verify` step at position {}.",
                            PRD_PATH,
                            id,
                            i + 1
                        ));
                    }
                }
            }
            _ => {
                return Err(format!(
                    "{}: acceptance criterion `{}` has no `verify` steps. State the numbered manual steps anyone can follow — where to go, what to do, exactly what should happen — or the criterion is not testable.",
                    PRD_PATH, id
                ));
            }
        }
    }

    Ok(())
}

/// Build the canonical product traceability index.
pub fn trace_index(workspace: &Path) -> Result<TraceIndex, String> {
    let doc = load(workspace)?;
    let mut ids = HashSet::new();
    let mut seen = HashSet::new();
    collect_ids(&doc, "features", "FEAT-", &mut seen, &mut ids)?;
    collect_ids(&doc, "user_stories", "US-", &mut seen, &mut ids)?;
    collect_ids(&doc, "acceptance_criteria", "AC-", &mut seen, &mut ids)?;
    Ok(TraceIndex { ids })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest document that satisfies the whole contract. "Minimal" means
    /// no optional extras, NOT missing required keys — every field here is one
    /// validation requires, so a test that removes one is testing that rule.
    fn minimal() -> Value {
        serde_json::json!({
            "version": 1,
            "destination": "A cook can buy, cook, and clean up after one meal.",
            "summary": "Kitchen workflow tracker.",
            "context": {
                "problem": "A cook loses track of what is already in the fridge.",
                "users": "One home cook, cooking for a household.",
                "constraints": []
            },
            "existing_code": [],
            "journeys": [{
                "id": "JRN-1", "title": "Buy and store a fish",
                "steps": ["Open the market", "Buy salmon", "Put it in the fridge"]
            }],
            "behavior": [{"id": "BH-1", "statement": "Checkout adds every bought item to inventory."}],
            "features": [{"id": "FEAT-1", "title": "Meal prep", "priority": "P0"}],
            "user_stories": [{
                "id": "US-1", "feature": "FEAT-1", "priority": "P0",
                "story": "As a cook, I want to buy fish, so that I can cook it.",
                "acceptance_criteria": ["AC-1"]
            }],
            "acceptance_criteria": [{
                "id": "AC-1",
                "statement": "A fresh fish object exists in inventory after checkout.",
                "verify": ["Visit the market", "Buy salmon", "Open the fridge"]
            }],
            "non_goals": [],
            "open_questions": []
        })
    }

    #[test]
    fn accepts_a_minimal_document() {
        assert_eq!(validate(&minimal()), Ok(()));
    }

    #[test]
    fn requires_the_destination() {
        let mut d = minimal();
        d["destination"] = Value::String("  ".to_string());
        assert!(validate(&d).unwrap_err().contains("destination"));
    }

    #[test]
    fn rejects_duplicate_ids_across_id_spaces() {
        let mut d = minimal();
        d["non_goals"] = serde_json::json!([{"id": "AC-1", "statement": "reused id"}]);
        let err = validate(&d).unwrap_err();
        assert!(err.contains("must start with `NG-`"), "{}", err);
    }

    #[test]
    fn rejects_duplicate_ids_within_one_space() {
        let mut d = minimal();
        d["acceptance_criteria"] = serde_json::json!([
            {"id": "AC-1", "statement": "one"},
            {"id": "AC-1", "statement": "two"},
        ]);
        assert!(validate(&d).unwrap_err().contains("duplicate id `AC-1`"));
    }

    #[test]
    fn rejects_unresolved_cross_references() {
        let mut d = minimal();
        d["user_stories"][0]["acceptance_criteria"] = serde_json::json!(["AC-9"]);
        assert!(validate(&d).unwrap_err().contains("AC-9"));

        let mut d = minimal();
        d["user_stories"][0]["feature"] = Value::String("FEAT-9".to_string());
        assert!(validate(&d).unwrap_err().contains("FEAT-9"));
    }

    #[test]
    fn requires_every_story_to_cite_a_criterion() {
        let mut d = minimal();
        d["user_stories"][0]["acceptance_criteria"] = serde_json::json!([]);
        assert!(validate(&d).unwrap_err().contains("at least one"));
    }

    /// An absent fog list must not read as "no fog" — that is the distinction
    /// the markdown version enforced by requiring an explicit section.
    #[test]
    fn requires_the_optional_lists_to_be_present_but_allows_empty() {
        let mut d = minimal();
        d.as_object_mut().unwrap().remove("open_questions");
        assert!(validate(&d).unwrap_err().contains("open_questions"));

        let mut d = minimal();
        d["open_questions"] = serde_json::json!([{"id": "UN-1", "question": "How many plates?"}]);
        assert_eq!(validate(&d), Ok(()));
    }

    /// A story with no `feature` broke the FEAT → US → AC chain and passed,
    /// because the reference was only checked when present.
    #[test]
    fn requires_every_story_to_name_a_feature() {
        let mut d = minimal();
        d["user_stories"][0]
            .as_object_mut()
            .unwrap()
            .remove("feature");
        assert!(validate(&d).unwrap_err().contains("names no `feature`"));

        let mut d = minimal();
        d["user_stories"][0]["feature"] = Value::String("   ".to_string());
        assert!(validate(&d).unwrap_err().contains("names no `feature`"));
    }

    /// A criterion with no `verify` steps states an outcome nobody can check.
    #[test]
    fn requires_every_criterion_to_carry_verify_steps() {
        let mut d = minimal();
        d["acceptance_criteria"][0]
            .as_object_mut()
            .unwrap()
            .remove("verify");
        assert!(validate(&d).unwrap_err().contains("no `verify` steps"));

        let mut d = minimal();
        d["acceptance_criteria"][0]["verify"] = serde_json::json!([]);
        assert!(validate(&d).unwrap_err().contains("no `verify` steps"));

        let mut d = minimal();
        d["acceptance_criteria"][0]["verify"] = serde_json::json!(["ok", "  "]);
        assert!(validate(&d).unwrap_err().contains("empty `verify` step"));
    }

    /// The id contract is `<PREFIX>-<integer>`. A non-numeric suffix passed the
    /// prefix test alone — including the literal `N` from the instruction's
    /// template, which is exactly what a model emits when it copies rather than
    /// numbers.
    #[test]
    fn requires_ids_to_be_numeric() {
        for bad in ["FEAT-N", "FEAT-", "FEAT-1a", "FEAT-one"] {
            let mut d = minimal();
            d["features"][0]["id"] = Value::String(bad.to_string());
            d["user_stories"][0]["feature"] = Value::String(bad.to_string());
            let err = validate(&d).unwrap_err();
            assert!(
                err.contains("literal integer"),
                "`{}` should be rejected, got: {}",
                bad,
                err
            );
        }
    }

    /// `context` is the stack authority the designer is told not to re-litigate
    /// and the reviewer checks against. Absent, both silently read "nothing was
    /// settled".
    #[test]
    fn requires_context_with_constraints_present() {
        let mut d = minimal();
        d.as_object_mut().unwrap().remove("context");
        assert!(validate(&d).unwrap_err().contains("missing `context`"));

        let mut d = minimal();
        d["context"].as_object_mut().unwrap().remove("constraints");
        assert!(validate(&d)
            .unwrap_err()
            .contains("`context.constraints` must be present"));

        // Empty is legitimate; absent is not.
        let mut d = minimal();
        d["context"]["constraints"] = serde_json::json!([]);
        assert_eq!(validate(&d), Ok(()));

        let mut d = minimal();
        d["context"]["problem"] = Value::String(String::new());
        assert!(validate(&d).unwrap_err().contains("context.problem"));
    }

    /// `existing_code` is the research record. `[]` is the greenfield answer; an
    /// absent key means nobody looked, and the two must not read the same.
    #[test]
    fn requires_existing_code_key_but_allows_empty() {
        let mut d = minimal();
        d.as_object_mut().unwrap().remove("existing_code");
        assert!(validate(&d).unwrap_err().contains("existing_code"));

        let mut d = minimal();
        d["existing_code"] = serde_json::json!([{"path": "src/a.ts"}]);
        assert!(validate(&d).unwrap_err().contains("non-empty `why`"));

        let mut d = minimal();
        d["existing_code"] = serde_json::json!([{"path": "src/a.ts", "why": "reused"}]);
        assert_eq!(validate(&d), Ok(()));
    }

    #[test]
    fn requires_journeys_and_behavior() {
        for field in ["journeys", "behavior"] {
            let mut d = minimal();
            d[field] = serde_json::json!([]);
            let err = validate(&d).unwrap_err();
            assert!(err.contains(field), "{}", err);
        }
    }

    /// Journey and behaviour ids share the one id namespace, so a collision with
    /// another space must be caught like any other duplicate.
    #[test]
    fn journey_and_behavior_ids_are_validated() {
        let mut d = minimal();
        d["journeys"][0]["id"] = Value::String("JRN-X".to_string());
        assert!(validate(&d).unwrap_err().contains("literal integer"));

        let mut d = minimal();
        d["behavior"][0]["id"] = Value::String("JRN-1".to_string());
        assert!(validate(&d).unwrap_err().contains("must start with `BH-`"));
    }

    #[test]
    fn trace_index_spans_features_stories_and_criteria() {
        let d = minimal();
        let mut ids = HashSet::new();
        let mut seen = HashSet::new();
        collect_ids(&d, "features", "FEAT-", &mut seen, &mut ids).unwrap();
        collect_ids(&d, "user_stories", "US-", &mut seen, &mut ids).unwrap();
        collect_ids(&d, "acceptance_criteria", "AC-", &mut seen, &mut ids).unwrap();
        let idx = TraceIndex { ids };
        assert!(idx.contains("FEAT-1") && idx.contains("US-1") && idx.contains("AC-1"));
        assert!(!idx.contains("NG-1"));
        assert_eq!(idx.known(), vec!["AC-1", "FEAT-1", "US-1"]);
    }
}

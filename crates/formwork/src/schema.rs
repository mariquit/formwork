// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

use std::path::PathBuf;

pub fn task_schema_path() -> PathBuf {
    crate::models::home_dir().join(".formwork/schema-task.json")
}
pub fn state_schema_path() -> PathBuf {
    crate::models::home_dir().join(".formwork/schema-state.json")
}

pub fn ensure_schema() -> std::io::Result<PathBuf> {
    let root = crate::models::home_dir().join(".formwork");
    std::fs::create_dir_all(&root)?;
    for (name, body) in documents() {
        std::fs::write(
            root.join(format!("schema-{name}.json")),
            serde_json::to_string_pretty(&body)?,
        )?;
    }
    Ok(root.join("schema-task.json"))
}

fn documents() -> Vec<(&'static str, serde_json::Value)> {
    use serde_json::json;
    let text = json!({"type":"string","minLength":1});
    let strings = json!({"type":"array","minItems":1,"uniqueItems":true,"items":text});
    let command = json!({"type":"object","required":["program","args","non_interactive"],"properties":{"program":text,"args":{"type":"array","items":{"type":"string"}},"non_interactive":{"const":true},"timeout_seconds":{"type":"integer","minimum":1,"maximum":600},"contracts":strings}});
    let mut check = command.clone();
    check["required"] = json!([
        "id",
        "milestone",
        "kind",
        "program",
        "args",
        "non_interactive"
    ]);
    check["properties"]["id"] = text.clone();
    check["properties"]["milestone"] = text.clone();
    check["properties"]["kind"] = json!({"enum":["unit","integration","build"]});
    vec![
        (
            "task",
            json!({"type":"object","required":["task_id","agent","stage","instruction"],"properties":{"task_id":text,"instruction":text,"stage":{"enum":["story","design","refine","review","rereview","tests","implement"]},"agent":{"enum":["storyteller","designer","reviewer","worker"]}}}),
        ),
        (
            "state",
            json!({"type":"object","required":["phase","stage","pipeline_version","completed","artifacts","blockers","prior_summaries"],"properties":{"pipeline_version":{"const":2},"phase":text,"stage":{"enum":["story","design","refine","review","rereview","tests","awaiting_approval","implement","complete","blocked"]},"completed":{"type":"array","items":{"type":"object"}},"artifacts":{"type":"array","items":{"type":"object"}},"blockers":{"type":"array","items":{"type":"object"}},"prior_summaries":{"type":"array"}}}),
        ),
        (
            "plan",
            json!({"type":"object","required":["milestones","contracts","validators"],"properties":{"milestones":{"type":"array","minItems":1,"items":{"type":"object","required":["id","features","work","modules"],"properties":{"id":text,"features":strings,"work":text,"modules":strings,"depends_on":{"type":"array","uniqueItems":true,"items":text}}}},"contracts":strings,"design_files":strings,"validators":{"type":"array","minItems":1,"items":command}}}),
        ),
        (
            "tests",
            json!({"type":"object","required":["files","coverage","checks","readiness"],"properties":{"files":strings,"coverage":{"type":"object","minProperties":1,"additionalProperties":strings},"checks":{"type":"array","minItems":1,"items":check},"readiness":{"type":"array","minItems":1,"items":command}}}),
        ),
        (
            "prd",
            json!({"type":"object","required":["version","destination","summary","context","existing_code","journeys","behavior","features","user_stories","acceptance_criteria","non_goals","open_questions","milestones"],"properties":{"version":{"type":"integer"},"destination":text,"summary":text,"context":{"type":"object","required":["problem","users","constraints"],"properties":{"problem":text,"users":text,"constraints":{"type":"array","items":{"type":"string"}}}},"features":{"type":"array","minItems":1,"items":{"type":"object","required":["id","title","milestone"],"properties":{"id":{"type":"string","pattern":"^FEAT-[0-9]+$"},"title":text,"milestone":text}}},"user_stories":{"type":"array","minItems":1},"acceptance_criteria":{"type":"array","minItems":1},"milestones":{"type":"array","minItems":1,"items":{"type":"object","required":["id","title","kind","outcome","rationale","exit_criteria"],"properties":{"id":{"type":"string","pattern":"^[A-Za-z0-9_-]+$"},"title":text,"kind":{"enum":["mvp","later","stretch"]},"outcome":text,"rationale":text,"exit_criteria":strings}}},"existing_code":{"type":"array"},"journeys":{"type":"array"},"behavior":{"type":"array"},"non_goals":{"type":"array"},"open_questions":{"type":"array"}}}),
        ),
    ]
}

pub fn validate(name: &str, value: &serde_json::Value) -> Result<(), String> {
    let (_, schema) = documents()
        .into_iter()
        .find(|(n, _)| *n == name)
        .ok_or("Unknown protocol schema")?;
    let validator = jsonschema::validator_for(&schema).map_err(|e| e.to_string())?;
    validator.validate(value).map_err(|e| e.to_string())
}

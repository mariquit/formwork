// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

use serde_json::Value;

/// Stamp the task-block schema onto a TASK BLOCK.
///
/// Only a task block: the pointer is a claim about shape, and the task schema
/// requires `task_id`, `domain`, `agent`, `phase`, and `instruction`. Stamping it
/// on a status dump, a rejection, or a rollback result advertised a contract those
/// objects cannot satisfy, which is worse than no pointer at all — a consumer that
/// validates would reject Formwork's own well-formed output. So the stamp is
/// applied only when the object actually carries the required fields, and
/// `print_json` is the path for everything else.
pub fn wrap_json(obj: Value) -> Value {
    // Point at the focused task-block schema rather than the whole protocol
    // bundle: the agent consuming this block only needs this one contract.
    let schema_path = crate::schema::task_schema_path()
        .to_string_lossy()
        .to_string();
    let mut wrapped = serde_json::Map::new();
    // Checked against the object itself rather than trusted from the call site,
    // because the call sites are many and a future one would otherwise
    // reintroduce a false claim silently.
    let is_task_block = matches!(&obj, Value::Object(m)
        if ["task_id", "domain", "agent", "phase", "instruction"]
            .iter()
            .all(|k| m.contains_key(*k)));
    if is_task_block {
        wrapped.insert("$schema".to_string(), Value::String(schema_path));
    }
    if let Value::Object(map) = obj {
        for (k, v) in map {
            wrapped.insert(k, v);
        }
    }
    Value::Object(wrapped)
}

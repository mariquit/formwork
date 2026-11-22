// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <[EMAIL_REDACTED]>

//! OpenCode install and live-catalog role setup.
//!
//! `--install` copies the authored Formwork agent, the four hidden role
//! prompts, the skill, and the server plugin. It does not write model keys and does not create
//! `models.ini`.
//!
//! `formwork setup` reads `opencode api get /api/model`, keeps tool-capable
//! chat models, and writes `agents.<role>.model`. `inherit` removes that key.
//! One unknown id, or a missing catalog, writes nothing. A successful setup
//! also removes retired `preflight` and `qa` entries. Every other key stays.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use crate::cli::InstallTarget;
use crate::{OPENCODE_PLUGIN, OPENCODE_PRIMARY, OPENCODE_SKILL, OPENCODE_SUBAGENTS};

const ROLES: [&str; 4] = ["storyteller", "designer", "reviewer", "worker"];
const RETIRED_AGENTS: [&str; 2] = ["preflight", "qa"];
const REQUIRED_SUBAGENT_DEPTH: u64 = 3;
const PLANNOTATOR_PKG: &str = "@plannotator/opencode";
const PLANNOTATOR_SPEC: &str = "@plannotator/opencode@latest";
const PLANNOTATOR_WORKFLOW: &str = "plan-agent";
/// `submit_plan` is a primary-session tool. Only the Formwork agent runs this
/// loop's plan gate. Plan and Build are not the coordinator, and the four roles
/// are hidden subagents that cannot call the tool.
const PLANNOTATOR_AGENTS: [&str; 1] = ["formwork"];

pub(crate) fn cmd_install(target: InstallTarget) {
    match crate::schema::ensure_schema() {
        Ok(dir) => println!("Schemas installed: {}/schema-*.json", dir.display()),
        Err(err) => {
            eprintln!("Failed to install schema: {err}");
            std::process::exit(1);
        }
    }
    match target {
        InstallTarget::Schema => {
            println!("\nSchema install complete.");
            println!(
                "Use --install=opencode to install the Formwork agents, skill, and workflow tools."
            );
        }
        InstallTarget::Opencode => install_skill(),
    }
}

pub(crate) fn cmd_setup(
    storyteller: Option<String>,
    designer: Option<String>,
    reviewer: Option<String>,
    worker: Option<String>,
    list: bool,
) {
    let flags = RoleFlags {
        storyteller,
        designer,
        reviewer,
        worker,
        list,
    };
    match run_setup(&crate::models::home_dir(), &flags, Path::new("opencode")) {
        Ok(report) => print_report(&report),
        Err(err) => {
            println!("{}", rejection_json(&err));
            std::process::exit(1);
        }
    }
}

fn install_skill() {
    let home = crate::models::home_dir();
    match install_skill_at_home(&home) {
        Ok(sync) => {
            println!("Skill installed: {}", sync.skill_file.display());
            println!("Workflow tools installed: {}", sync.plugin_dir.display());
            for agent_file in &sync.agent_files {
                println!("Agent installed: {}", agent_file.display());
            }
            if sync.wrote {
                println!("Config updated: {}", sync.config_file.display());
            } else {
                println!("Config already in sync: {}", sync.config_file.display());
            }
            report_install(&sync);
            if sync.wrote {
                println!("\nRestart OpenCode for the Formwork agent to take effect.");
            }
            println!("\nFormwork install complete.");
        }
        Err(err) => {
            eprintln!("Failed to install opencode integration: {err}");
            std::process::exit(1);
        }
    }
}

struct InstallSync {
    skill_file: PathBuf,
    plugin_dir: PathBuf,
    agent_files: Vec<PathBuf>,
    config_file: PathBuf,
    plannotator: PlannotatorStatus,
    depth: DepthOutcome,
    wrote: bool,
}

fn install_skill_at_home(home: &Path) -> io::Result<InstallSync> {
    let config_root = home.join(".config/opencode");
    let plugin_dir = config_root.join("plugins/formwork");
    fs::create_dir_all(&plugin_dir)?;
    for (name, body) in OPENCODE_PLUGIN {
        fs::write(plugin_dir.join(name), body)?;
    }

    let skill_dir = config_root.join("skills/formwork");
    fs::create_dir_all(&skill_dir)?;
    let skill_file = skill_dir.join("SKILL.md");
    fs::write(&skill_file, OPENCODE_SKILL)?;

    let agents_dir = config_root.join("agents");
    fs::create_dir_all(&agents_dir)?;
    let mut agent_files = Vec::new();
    let mut prompts = vec![OPENCODE_PRIMARY];
    prompts.extend(OPENCODE_SUBAGENTS);
    for (name, body) in prompts {
        let path = agents_dir.join(format!("{name}.md"));
        fs::write(&path, body)?;
        agent_files.push(path);
    }

    let config_file = pick_opencode_config_file(&config_root);
    let sync = merge_install_config(&config_file)?;
    Ok(InstallSync {
        skill_file,
        plugin_dir,
        agent_files,
        config_file,
        plannotator: sync.plannotator,
        depth: sync.depth,
        wrote: sync.wrote,
    })
}

fn report_install(sync: &InstallSync) {
    match sync.plannotator {
        PlannotatorStatus::Added => println!(
            "Plannotator: plugin entry added to {} (workflow={PLANNOTATOR_WORKFLOW}, planningAgents={})",
            sync.config_file.display(),
            PLANNOTATOR_AGENTS.join(",")
        ),
        PlannotatorStatus::Merged => println!(
            "Plannotator: plugin entry merged into {} (workflow={PLANNOTATOR_WORKFLOW}, planningAgents={})",
            sync.config_file.display(),
            PLANNOTATOR_AGENTS.join(",")
        ),
        PlannotatorStatus::AlreadyConfigured => {}
    }
    if matches!(
        sync.plannotator,
        PlannotatorStatus::Added | PlannotatorStatus::Merged
    ) {
        println!("  Formwork uses submit_plan in its own session for the story and design gates.");
        println!(
            "  Install the review commands:  curl -fsSL https://plannotator.ai/install.sh | bash"
        );
    }
    match sync.depth {
        DepthOutcome::Inserted => {
            println!("Config: subagent_depth set to {REQUIRED_SUBAGENT_DEPTH}")
        }
        DepthOutcome::Raised(old) => {
            println!("Config: subagent_depth raised from {old} to {REQUIRED_SUBAGENT_DEPTH}")
        }
        DepthOutcome::Satisfied(_) | DepthOutcome::NotAnInteger => {}
    }
}

struct ConfigSync {
    plannotator: PlannotatorStatus,
    depth: DepthOutcome,
    wrote: bool,
}

fn merge_install_config(config_file: &Path) -> io::Result<ConfigSync> {
    let raw = if config_file.exists() {
        fs::read_to_string(config_file)?
    } else {
        String::new()
    };
    let base = if raw.trim().is_empty() {
        "{\n  \"$schema\": \"https://opencode.ai/config.json\"\n}\n".to_string()
    } else {
        raw.clone()
    };
    let (with_plugin, plannotator) = merge_plannotator_plugin(&base).map_err(invalid_data)?;
    let (edited, depth) = match ensure_subagent_depth(&with_plugin) {
        Ok((Some(text), outcome)) => (text, outcome),
        Ok((None, outcome)) => (with_plugin, outcome),
        Err(err) => {
            eprintln!(
                "Warning: could not set subagent_depth in {}: {err}",
                config_file.display()
            );
            (with_plugin, DepthOutcome::NotAnInteger)
        }
    };
    let edited = ensure_trailing_newline(edited);
    if edited == raw {
        return Ok(ConfigSync {
            plannotator,
            depth,
            wrote: false,
        });
    }
    if let Some(parent) = config_file.parent() {
        fs::create_dir_all(parent)?;
    }
    if config_file.exists() {
        backup_config_file(config_file)?;
    }
    fs::write(config_file, edited)?;
    Ok(ConfigSync {
        plannotator,
        depth,
        wrote: true,
    })
}

fn invalid_data(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

fn pick_opencode_config_file(config_root: &Path) -> PathBuf {
    let jsonc = config_root.join("opencode.jsonc");
    if jsonc.exists() {
        return jsonc;
    }
    let json = config_root.join("opencode.json");
    if json.exists() {
        return json;
    }
    jsonc
}

fn backup_config_file(config_file: &Path) -> io::Result<Option<PathBuf>> {
    if !config_file.exists() {
        return Ok(None);
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let file_name = config_file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "config".to_string());
    let backup_name = format!("{file_name}.{stamp}.bak");
    let backup_path = match config_file.parent() {
        Some(parent) => parent.join(backup_name),
        None => PathBuf::from(backup_name),
    };
    fs::copy(config_file, &backup_path)?;
    eprintln!("Backup created: {}", backup_path.display());
    Ok(Some(backup_path))
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum PlannotatorStatus {
    AlreadyConfigured,
    Added,
    Merged,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum DepthOutcome {
    Inserted,
    Raised(u64),
    Satisfied(u64),
    NotAnInteger,
}

struct RoleFlags {
    storyteller: Option<String>,
    designer: Option<String>,
    reviewer: Option<String>,
    worker: Option<String>,
    list: bool,
}

impl RoleFlags {
    fn value(&self, role: &str) -> &Option<String> {
        match role {
            "storyteller" => &self.storyteller,
            "designer" => &self.designer,
            "reviewer" => &self.reviewer,
            "worker" => &self.worker,
            _ => unreachable!("unknown role {role}"),
        }
    }

    fn any_role(&self) -> bool {
        ROLES.iter().any(|role| self.value(role).is_some())
    }
}

enum Choice {
    Keep,
    Inherit,
    Set(String),
}

#[derive(Debug)]
struct SetupReport {
    config: PathBuf,
    wrote: bool,
    listing: Option<String>,
    notes: Vec<String>,
}

#[derive(Debug)]
struct SetupFailure {
    reason: &'static str,
    message: String,
    instruction: &'static str,
}

fn fail(
    reason: &'static str,
    message: impl Into<String>,
    instruction: &'static str,
) -> SetupFailure {
    SetupFailure {
        reason,
        message: message.into(),
        instruction,
    }
}

fn rejection_json(err: &SetupFailure) -> Value {
    json!({
        "status": "rejected",
        "reason": err.reason,
        "error": err.message,
        "instruction": err.instruction,
    })
}

fn print_report(report: &SetupReport) {
    if let Some(listing) = &report.listing {
        print!("{listing}");
    }
    if report.wrote {
        println!("Config updated: {}", report.config.display());
        for note in &report.notes {
            println!("{note}");
        }
        println!("Restart OpenCode for model changes to take effect.");
    } else if report.listing.is_none() {
        println!("Config already in sync: {}", report.config.display());
    }
}

fn run_setup(home: &Path, flags: &RoleFlags, opencode: &Path) -> Result<SetupReport, SetupFailure> {
    if flags.list && !flags.any_role() {
        let ids = load_catalog(opencode)?;
        let (path, _raw, doc) = read_config(home)?;
        return Ok(SetupReport {
            config: path,
            wrote: false,
            listing: Some(format_listing(&ids, &doc)),
            notes: Vec::new(),
        });
    }

    let choices = if flags.any_role() {
        choices_from_flags(flags)?
    } else if !io::stdin().is_terminal() {
        return Err(fail(
            "setup_needs_roles",
            "No role flags were given and stdin is not a terminal.",
            "Pass --storyteller, --designer, --reviewer, and/or --worker, or run `formwork setup --list`.",
        ));
    } else {
        let (_path, _raw, doc) = read_config(home)?;
        prompt_choices(&doc)?
    };
    let ids = load_catalog(opencode)?;
    validate_choices(&choices, &ids)?;
    let (path, raw, before) = read_config(home)?;
    let listing = if flags.list {
        Some(format_listing(&ids, &before))
    } else {
        None
    };
    if !needs_write(&before, &choices) {
        return Ok(SetupReport {
            config: path,
            wrote: false,
            listing,
            notes: Vec::new(),
        });
    }

    let new_text = apply_setup_edits(
        if raw.trim().is_empty() {
            "{\n}\n"
        } else {
            &raw
        },
        &choices,
    )
    .map_err(|err| {
        fail(
            "invalid_config",
            err,
            "Fix the OpenCode config and retry. Nothing was written.",
        )
    })?;
    let after: Value = json5::from_str(&new_text).map_err(|err| {
        fail(
            "invalid_config",
            format!("Edited config does not parse: {err}"),
            "Nothing was written.",
        )
    })?;
    if !after.is_object() {
        return Err(fail(
            "invalid_config",
            "Edited config is not a JSON object.",
            "Nothing was written.",
        ));
    }
    if new_text == raw {
        return Ok(SetupReport {
            config: path,
            wrote: false,
            listing,
            notes: Vec::new(),
        });
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            fail(
                "invalid_config",
                format!("Could not create {}: {err}", parent.display()),
                "Nothing was written.",
            )
        })?;
    }
    if path.exists() {
        backup_config_file(&path).map_err(|err| {
            fail(
                "invalid_config",
                format!("Could not back up {}: {err}", path.display()),
                "Nothing was written.",
            )
        })?;
    }
    fs::write(&path, &new_text).map_err(|err| {
        fail(
            "invalid_config",
            format!("Could not write {}: {err}", path.display()),
            "Restore the backup beside the config if the write stopped midway.",
        )
    })?;
    let listing = if flags.list {
        Some(format_listing(&ids, &after))
    } else {
        listing
    };
    Ok(SetupReport {
        config: path,
        wrote: true,
        listing,
        notes: diff_notes(&before, &after),
    })
}

fn choices_from_flags(flags: &RoleFlags) -> Result<Vec<(&'static str, Choice)>, SetupFailure> {
    let mut choices = Vec::new();
    for role in ROLES {
        let choice = match flags.value(role) {
            None => Choice::Keep,
            Some(value) => parse_flag(role, value)?,
        };
        choices.push((role, choice));
    }
    Ok(choices)
}

fn parse_flag(role: &str, value: &str) -> Result<Choice, SetupFailure> {
    let value = value.trim();
    if value.is_empty() {
        return Err(fail(
            "invalid_model",
            format!("{role} model id is empty."),
            "Pass a catalog id or inherit. Nothing was written.",
        ));
    }
    if value == "inherit" {
        return Ok(Choice::Inherit);
    }
    Ok(Choice::Set(value.to_string()))
}

fn prompt_choices(doc: &Value) -> Result<Vec<(&'static str, Choice)>, SetupFailure> {
    let stdin = io::stdin();
    let mut choices = Vec::new();
    for role in ROLES {
        let current = pin_of(doc, role);
        eprint!("{role} [{current}] catalog id, inherit, or empty to keep: ");
        let _ = io::stderr().flush();
        let mut line = String::new();
        stdin.lock().read_line(&mut line).map_err(|err| {
            fail(
                "setup_needs_roles",
                format!("Could not read a role choice: {err}"),
                "Pass the role flags instead. Nothing was written.",
            )
        })?;
        let choice = if line.trim().is_empty() {
            Choice::Keep
        } else if line.trim() == "inherit" {
            Choice::Inherit
        } else {
            Choice::Set(line.trim().to_string())
        };
        choices.push((role, choice));
    }
    Ok(choices)
}

fn run_catalog_command(program: &Path) -> Result<std::process::Output, SetupFailure> {
    // Linux returns ETXTBSY when a binary is replaced and executed in the same
    // moment, including the test fixture that has just been written. A missing
    // binary is not transient, so only that one error is retried.
    let mut attempts = 0u8;
    loop {
        match Command::new(program)
            .args(["api", "get", "/api/model"])
            .output()
        {
            Ok(output) => return Ok(output),
            Err(err) if err.kind() == io::ErrorKind::ExecutableFileBusy && attempts < 5 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(20 * u64::from(attempts)));
            }
            Err(err) => {
                return Err(fail(
                    "catalog_unavailable",
                    format!("Could not run {}: {err}", program.display()),
                    "Install OpenCode and retry. Setup reads `opencode api get /api/model` and writes nothing when that catalog is unavailable.",
                ));
            }
        }
    }
}

fn load_catalog(program: &Path) -> Result<BTreeSet<String>, SetupFailure> {
    let output = run_catalog_command(program)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let detail = if stderr.is_empty() {
            format!("{} exited {}", program.display(), output.status)
        } else {
            let stderr = stderr.chars().take(500).collect::<String>();
            format!("{} exited {}: {stderr}", program.display(), output.status)
        };
        return Err(fail(
            "catalog_unavailable",
            detail,
            "Retry when `opencode api get /api/model` prints the model catalog. Nothing was written.",
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    parse_catalog(&stdout)
}

fn parse_catalog(raw: &str) -> Result<BTreeSet<String>, SetupFailure> {
    let value: Value = json5::from_str(raw).map_err(|err| {
        fail(
            "invalid_catalog",
            format!("Catalog is not JSON: {err}"),
            "Expected the OpenCode model catalog object with a data array. Nothing was written.",
        )
    })?;
    let data = value.get("data").and_then(Value::as_array).ok_or_else(|| {
        fail(
            "invalid_catalog",
            "Catalog payload has no data array.",
            "Expected `opencode api get /api/model` to return {{\"data\": [...]}}. Nothing was written.",
        )
    })?;
    let mut ids = BTreeSet::new();
    for entry in data {
        let tools = entry
            .pointer("/capabilities/tools")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !tools {
            continue;
        }
        if let Some(kind) = entry.get("type").and_then(Value::as_str) {
            if kind != "chat" {
                continue;
            }
        }
        let Some(provider) = entry.get("providerID").and_then(Value::as_str) else {
            continue;
        };
        let Some(model) = entry.get("modelID").and_then(Value::as_str) else {
            continue;
        };
        if provider.is_empty() || model.is_empty() {
            continue;
        }
        let base = format!("{provider}/{model}");
        ids.insert(base.clone());
        let Some(variants) = entry.get("variants").and_then(Value::as_array) else {
            continue;
        };
        for variant in variants {
            let id = variant
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| variant.as_str());
            if let Some(id) = id {
                if !id.is_empty() && !id.contains('/') && !id.contains('#') {
                    ids.insert(format!("{base}#{id}"));
                }
            }
        }
    }
    Ok(ids)
}

fn validate_choices(
    choices: &[(&str, Choice)],
    ids: &BTreeSet<String>,
) -> Result<(), SetupFailure> {
    let mut unknown = Vec::new();
    for (role, choice) in choices {
        if let Choice::Set(id) = choice {
            if !ids.contains(id) {
                unknown.push(format!("{role}: {id}"));
            }
        }
    }
    if unknown.is_empty() {
        return Ok(());
    }
    Err(fail(
        "unknown_model",
        format!("Unknown model id {}", unknown.join(", ")),
        "Choose an id from `formwork setup --list`. Use `#variant` only when that variant is listed on the catalog entry. Nothing was written.",
    ))
}

fn read_config(home: &Path) -> Result<(PathBuf, String, Value), SetupFailure> {
    let path = pick_opencode_config_file(&home.join(".config/opencode"));
    if !path.exists() {
        return Ok((path, String::new(), json!({})));
    }
    let raw = fs::read_to_string(&path).map_err(|err| {
        fail(
            "invalid_config",
            format!("Could not read {}: {err}", path.display()),
            "Fix the OpenCode config and retry. Nothing was written.",
        )
    })?;
    let value: Value = json5::from_str(&raw).map_err(|err| {
        fail(
            "invalid_config",
            format!("{} is not JSON/JSONC: {err}", path.display()),
            "Fix the OpenCode config and retry. Nothing was written.",
        )
    })?;
    if !value.is_object() {
        return Err(fail(
            "invalid_config",
            format!("{} must contain a JSON object.", path.display()),
            "Fix the OpenCode config and retry. Nothing was written.",
        ));
    }
    Ok((path, raw, value))
}

fn needs_write(doc: &Value, choices: &[(&str, Choice)]) -> bool {
    let agents = doc.get("agents").and_then(Value::as_object);
    if agents.is_some_and(|map| RETIRED_AGENTS.iter().any(|name| map.contains_key(*name))) {
        return true;
    }
    for (role, choice) in choices {
        match choice {
            Choice::Keep => {}
            Choice::Inherit => {
                if agents
                    .and_then(|map| map.get(*role))
                    .and_then(|entry| entry.get("model"))
                    .is_some()
                {
                    return true;
                }
            }
            Choice::Set(id) => {
                let current = agents
                    .and_then(|map| map.get(*role))
                    .and_then(|entry| entry.get("model"))
                    .and_then(Value::as_str);
                if current != Some(id.as_str()) {
                    return true;
                }
            }
        }
    }
    false
}

fn pin_of(doc: &Value, role: &str) -> String {
    doc.get("agents")
        .and_then(|agents| agents.get(role))
        .and_then(|entry| entry.get("model"))
        .and_then(Value::as_str)
        .unwrap_or("inherit")
        .to_string()
}

fn format_listing(ids: &BTreeSet<String>, doc: &Value) -> String {
    let mut out = String::from("tool-capable:\n");
    for id in ids {
        out.push_str("  ");
        out.push_str(id);
        out.push('\n');
    }
    out.push_str("pins:\n");
    for role in ROLES {
        let pin = pin_of(doc, role);
        out.push_str(&format!("  {role}: {pin}\n"));
    }
    out
}

fn diff_notes(before: &Value, after: &Value) -> Vec<String> {
    let mut notes = Vec::new();
    for role in ROLES {
        let previous = model_of(before, role);
        let next = model_of(after, role);
        if previous == next {
            continue;
        }
        match next {
            Some(id) => notes.push(format!("{role} model set to {id}")),
            None => notes.push(format!("{role} model cleared (inherit)")),
        }
    }
    for name in RETIRED_AGENTS {
        let had = before
            .get("agents")
            .and_then(|agents| agents.get(name))
            .is_some();
        let has = after
            .get("agents")
            .and_then(|agents| agents.get(name))
            .is_some();
        if had && !has {
            notes.push(format!("removed agents.{name}"));
        }
    }
    notes
}

fn model_of(doc: &Value, role: &str) -> Option<String> {
    doc.get("agents")
        .and_then(|agents| agents.get(role))
        .and_then(|entry| entry.get("model"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn apply_setup_edits(original: &str, choices: &[(&str, Choice)]) -> Result<String, String> {
    let mut text = if original.trim().is_empty() {
        "{\n}\n".to_string()
    } else {
        original.to_string()
    };
    for name in RETIRED_AGENTS {
        if let Some(brace) = agents_brace(&text)? {
            text = remove_member(&text, brace, name)?;
        }
    }
    for (role, choice) in choices {
        match choice {
            Choice::Keep => {}
            Choice::Inherit => {
                if let Some(brace) = role_brace(&text, role)? {
                    text = remove_member(&text, brace, "model")?;
                }
            }
            Choice::Set(model) => {
                text = set_role_model(&text, role, model)?;
            }
        }
    }
    Ok(ensure_trailing_newline(text))
}

fn ensure_agents_object(text: &str) -> Result<String, String> {
    if agents_brace(text)?.is_some() {
        return Ok(text.to_string());
    }
    let root = root_brace(text)?;
    insert_member(text, root, "agents", "{}")
}

fn set_role_model(text: &str, role: &str, model: &str) -> Result<String, String> {
    let text = ensure_agents_object(text)?;
    let agents = agents_brace(&text)?.ok_or_else(|| "agents object missing".to_string())?;
    let members = object_members(&text, agents)?;
    let rendered_model = serde_json::to_string(model).map_err(|err| err.to_string())?;
    if let Some(member) = members.iter().find(|member| member.key == role) {
        let brace = object_value_brace(&text, member.value_start, member.value_end)
            .map_err(|_| format!("agents.{role} is not an object"))?;
        return set_raw_member(&text, brace, "model", &rendered_model);
    }
    let rendered = format!("{{ \"model\": {rendered_model} }}");
    insert_member(&text, agents, role, &rendered)
}

fn agents_brace(text: &str) -> Result<Option<usize>, String> {
    let root = root_brace(text)?;
    let members = object_members(text, root)?;
    let Some(member) = members.into_iter().find(|member| member.key == "agents") else {
        return Ok(None);
    };
    object_value_brace(text, member.value_start, member.value_end)
        .map(Some)
        .map_err(|_| "agents is not an object".to_string())
}

fn role_brace(text: &str, role: &str) -> Result<Option<usize>, String> {
    let Some(agents) = agents_brace(text)? else {
        return Ok(None);
    };
    let members = object_members(text, agents)?;
    let Some(member) = members.into_iter().find(|member| member.key == role) else {
        return Ok(None);
    };
    object_value_brace(text, member.value_start, member.value_end)
        .map(Some)
        .map_err(|_| format!("agents.{role} is not an object"))
}

fn ensure_subagent_depth(text: &str) -> Result<(Option<String>, DepthOutcome), String> {
    let existing: Value = json5::from_str(text).map_err(|err| err.to_string())?;
    match existing.get("subagent_depth") {
        None => {
            if let Some(depth) = existing
                .get("experimental")
                .and_then(|value| value.get("subagent_depth"))
                .and_then(Value::as_u64)
            {
                if depth >= REQUIRED_SUBAGENT_DEPTH {
                    return Ok((None, DepthOutcome::Satisfied(depth)));
                }
            }
            let root = root_brace(text)?;
            let updated = insert_member(
                text,
                root,
                "subagent_depth",
                &REQUIRED_SUBAGENT_DEPTH.to_string(),
            )?;
            Ok((Some(updated), DepthOutcome::Inserted))
        }
        Some(value) => {
            let Some(depth) = value.as_u64() else {
                return Ok((None, DepthOutcome::NotAnInteger));
            };
            if depth >= REQUIRED_SUBAGENT_DEPTH {
                return Ok((None, DepthOutcome::Satisfied(depth)));
            }
            let root = root_brace(text)?;
            let members = object_members(text, root)?;
            let member = members
                .iter()
                .find(|member| member.key == "subagent_depth")
                .ok_or_else(|| "could not locate subagent_depth".to_string())?;
            let updated = splice(
                text,
                member.value_start,
                member.value_end,
                &REQUIRED_SUBAGENT_DEPTH.to_string(),
            );
            Ok((Some(updated), DepthOutcome::Raised(depth)))
        }
    }
}

fn merge_plannotator_plugin(raw: &str) -> Result<(String, PlannotatorStatus), String> {
    let parsed: Value = json5::from_str(raw).map_err(|err| err.to_string())?;
    let root = parsed
        .as_object()
        .ok_or_else(|| "config must be a JSON object".to_string())?;
    let had_entry = contains_plannotator(root);
    let edited = if let Some((open, close)) = root_array_bounds(raw, "plugins")? {
        reconcile_plugin_array(raw, "plugins", open, close)?
    } else if let Some((open, close)) = root_array_bounds(raw, "plugin")? {
        reconcile_plugin_array(raw, "plugin", open, close)?
    } else if root.contains_key("plugins") || root.contains_key("plugin") {
        return Err("plugin list is not an array".into());
    } else {
        Some(insert_v2_plugins(raw)?)
    };
    match edited {
        None => Ok((raw.to_string(), PlannotatorStatus::AlreadyConfigured)),
        Some(text) if had_entry => Ok((text, PlannotatorStatus::Merged)),
        Some(text) => Ok((text, PlannotatorStatus::Added)),
    }
}

fn contains_plannotator(root: &serde_json::Map<String, Value>) -> bool {
    let Some(list) = root
        .get("plugins")
        .or_else(|| root.get("plugin"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    list.iter().any(entry_is_plannotator)
}

fn entry_is_plannotator(value: &Value) -> bool {
    match value {
        Value::String(spec) => is_plannotator_spec(spec),
        Value::Object(object) => object
            .get("package")
            .and_then(Value::as_str)
            .is_some_and(is_plannotator_spec),
        Value::Array(tuple) => tuple
            .first()
            .and_then(Value::as_str)
            .is_some_and(is_plannotator_spec),
        _ => false,
    }
}

fn is_plannotator_spec(spec: &str) -> bool {
    spec == PLANNOTATOR_PKG || spec.starts_with(&format!("{PLANNOTATOR_PKG}@"))
}

fn planning_json() -> String {
    format!("[\"{}\"]", PLANNOTATOR_AGENTS[0])
}

fn replacement_element(key: &str) -> String {
    let agents = planning_json();
    if key == "plugins" {
        format!(
            "{{ \"package\": \"{PLANNOTATOR_SPEC}\", \"options\": {{ \"workflow\": \"{PLANNOTATOR_WORKFLOW}\", \"planningAgents\": {agents} }} }}"
        )
    } else {
        format!(
            "[\"{PLANNOTATOR_SPEC}\", {{ \"workflow\": \"{PLANNOTATOR_WORKFLOW}\", \"planningAgents\": {agents} }}]"
        )
    }
}

fn insert_v2_plugins(text: &str) -> Result<String, String> {
    let root = root_brace(text)?;
    let rendered = format!("[{}]", replacement_element("plugins"));
    insert_member(text, root, "plugins", &rendered)
}

fn root_array_bounds(text: &str, key: &str) -> Result<Option<(usize, usize)>, String> {
    let root = root_brace(text)?;
    let members = object_members(text, root)?;
    let Some(member) = members.into_iter().find(|member| member.key == key) else {
        return Ok(None);
    };
    let slice = &text[member.value_start..member.value_end];
    if !slice.trim_start().starts_with('[') {
        return Err(format!("{key} is not an array"));
    }
    let bracket = member.value_start + slice.find('[').ok_or("plugin array missing")?;
    let end = scan_value_end(text, bracket).ok_or("unterminated plugin array")?;
    if end == 0 {
        return Err("unterminated plugin array".into());
    }
    Ok(Some((bracket + 1, end - 1)))
}

fn reconcile_plugin_array(
    text: &str,
    key: &str,
    open: usize,
    close: usize,
) -> Result<Option<String>, String> {
    let Some(quote) = find_plannotator_quote(text, open, close) else {
        return Ok(Some(append_element(
            text,
            open,
            close,
            &replacement_element(key),
        )?));
    };
    let (start, end) = element_containing(text, open, close, quote)?;
    match text.as_bytes().get(start) {
        Some(b'{') => ensure_v2_planning(text, start),
        Some(b'[') => ensure_v1_planning(text, start),
        _ => Ok(Some(splice(text, start, end, &replacement_element(key)))),
    }
}

fn ensure_v2_planning(text: &str, object: usize) -> Result<Option<String>, String> {
    let members = object_members(text, object)?;
    let Some(options) = members.iter().find(|member| member.key == "options") else {
        let rendered = format!(
            "{{ \"workflow\": \"{PLANNOTATOR_WORKFLOW}\", \"planningAgents\": {} }}",
            planning_json()
        );
        return Ok(Some(insert_member(text, object, "options", &rendered)?));
    };
    let brace = object_value_brace(text, options.value_start, options.value_end)
        .map_err(|_| "plugin options is not an object".to_string())?;
    set_planning_agents(text, brace)
}

fn ensure_v1_planning(text: &str, tuple_open: usize) -> Result<Option<String>, String> {
    let Some(brace) = v1_options_brace(text, tuple_open)? else {
        let end = scan_value_end(text, tuple_open).ok_or("unterminated plugin tuple")?;
        return Ok(Some(splice(
            text,
            tuple_open,
            end,
            &replacement_element("plugin"),
        )));
    };
    set_planning_agents(text, brace)
}

fn v1_options_brace(text: &str, tuple_open: usize) -> Result<Option<usize>, String> {
    let end = scan_value_end(text, tuple_open).ok_or("unterminated plugin tuple")?;
    let bytes = text.as_bytes();
    let mut i = skip_ws_comments(text, tuple_open + 1);
    if i >= end {
        return Ok(None);
    }
    let first_end = scan_value_end(text, i).ok_or("unterminated plugin spec")?;
    i = skip_ws_comments(text, first_end);
    if i >= end || bytes.get(i) != Some(&b',') {
        return Ok(None);
    }
    i = skip_ws_comments(text, i + 1);
    if bytes.get(i) == Some(&b'{') {
        Ok(Some(i))
    } else {
        Ok(None)
    }
}

fn set_planning_agents(text: &str, brace: usize) -> Result<Option<String>, String> {
    let members = object_members(text, brace)?;
    let rendered = planning_json();
    if let Some(member) = members
        .into_iter()
        .find(|member| member.key == "planningAgents")
    {
        let current: Value = json5::from_str(&text[member.value_start..member.value_end])
            .map_err(|err| err.to_string())?;
        if current == json!([PLANNOTATOR_AGENTS[0]]) {
            return Ok(None);
        }
        return Ok(Some(splice(
            text,
            member.value_start,
            member.value_end,
            &rendered,
        )));
    }
    Ok(Some(insert_member(
        text,
        brace,
        "planningAgents",
        &rendered,
    )?))
}

fn append_element(text: &str, open: usize, close: usize, element: &str) -> Result<String, String> {
    let indent = detect_indent_unit(text);
    let comma = if array_has_elements(text, open, close) {
        ","
    } else {
        ""
    };
    let insertion = format!("{comma}\n{indent}{indent}{element}\n{indent}");
    Ok(splice(text, close, close, &insertion))
}

fn array_has_elements(text: &str, open: usize, close: usize) -> bool {
    let bytes = text.as_bytes();
    let mut i = open;
    while i < close {
        i = skip_ws_comments(text, i);
        if i >= close {
            return false;
        }
        if bytes[i] == b',' {
            i += 1;
            continue;
        }
        return bytes[i] != b']';
    }
    false
}

fn find_plannotator_quote(text: &str, from: usize, end: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = from;
    while i < end && i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let quote = i;
                let (content, after) = parse_json_string(text, i).ok()?;
                if quote < end && is_plannotator_spec(&content) {
                    return Some(quote);
                }
                i = after;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            _ => i += 1,
        }
    }
    None
}

fn element_containing(
    text: &str,
    open: usize,
    close: usize,
    needle: usize,
) -> Result<(usize, usize), String> {
    let bytes = text.as_bytes();
    let mut i = open;
    while i < close {
        i = skip_ws_comments(text, i);
        if i >= close {
            break;
        }
        if bytes[i] == b',' {
            i += 1;
            continue;
        }
        let end =
            scan_value_end(text, i).ok_or_else(|| "unterminated plugin element".to_string())?;
        if end <= i {
            return Err("plugin scan stalled".into());
        }
        if i <= needle && needle < end {
            return Ok((i, end));
        }
        i = end;
    }
    Err("could not locate the Plannotator plugin entry".into())
}

struct Member {
    key: String,
    key_start: usize,
    value_start: usize,
    value_end: usize,
    delete_start: usize,
    delete_end: usize,
}

fn object_members(text: &str, open_brace: usize) -> Result<Vec<Member>, String> {
    let bytes = text.as_bytes();
    if bytes.get(open_brace) != Some(&b'{') {
        return Err("expected an object".into());
    }
    let mut i = open_brace + 1;
    let mut members = Vec::new();
    while i < bytes.len() {
        i = skip_ws_comments(text, i);
        if i >= bytes.len() {
            return Err("unterminated object".into());
        }
        if bytes[i] == b'}' {
            return Ok(members);
        }
        if bytes[i] == b',' {
            i += 1;
            continue;
        }
        if bytes[i] != b'"' {
            return Err("expected an object key".into());
        }
        let key_start = i;
        let (key, after_key) = parse_json_string(text, i)?;
        i = skip_ws_comments(text, after_key);
        if bytes.get(i) != Some(&b':') {
            return Err(format!("expected ':' after {key}"));
        }
        i = skip_ws_comments(text, i + 1);
        let value_start = i;
        let value_end = scan_value_end(text, value_start)
            .ok_or_else(|| format!("unterminated value for {key}"))?;
        if value_end <= value_start {
            return Err(format!("empty value for {key}"));
        }
        let (delete_start, delete_end) = deletion_bounds(text, key_start, value_end);
        members.push(Member {
            key,
            key_start,
            value_start,
            value_end,
            delete_start,
            delete_end,
        });
        i = value_end;
    }
    Err("unterminated object".into())
}

fn deletion_bounds(text: &str, key_start: usize, value_end: usize) -> (usize, usize) {
    let bytes = text.as_bytes();
    let after = skip_ws_comments(text, value_end);
    if after < bytes.len() && bytes[after] == b',' {
        // Drop the member's own indent and the newline after its comma.
        // Stopping at the key leaves `    ` behind, which is a blank line.
        let line_start = text[..key_start].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let start = if text[line_start..key_start]
            .chars()
            .all(|c| c == ' ' || c == '\t')
        {
            line_start
        } else {
            key_start
        };
        let mut end = after + 1;
        if end < bytes.len() && bytes[end] == b'\n' {
            end += 1;
        }
        return (start, end);
    }
    let mut j = key_start;
    while j > 0 && bytes[j - 1].is_ascii_whitespace() {
        j -= 1;
    }
    if j > 0 && bytes[j - 1] == b',' {
        return (j - 1, value_end);
    }
    (key_start, value_end)
}

fn set_raw_member(
    text: &str,
    open_brace: usize,
    key: &str,
    rendered: &str,
) -> Result<String, String> {
    let members = object_members(text, open_brace)?;
    if let Some(member) = members.iter().find(|member| member.key == key) {
        return Ok(splice(text, member.value_start, member.value_end, rendered));
    }
    insert_member(text, open_brace, key, rendered)
}

fn insert_member(
    text: &str,
    open_brace: usize,
    key: &str,
    rendered: &str,
) -> Result<String, String> {
    let members = object_members(text, open_brace)?;
    let indent = member_indent(text, open_brace);
    let comma = if members.is_empty() { "" } else { "," };
    let insertion = format!("\n{indent}\"{key}\": {rendered}{comma}");
    Ok(splice(text, open_brace + 1, open_brace + 1, &insertion))
}

fn remove_member(text: &str, open_brace: usize, key: &str) -> Result<String, String> {
    let members = object_members(text, open_brace)?;
    let Some(member) = members.into_iter().find(|member| member.key == key) else {
        return Ok(text.to_string());
    };
    Ok(splice(text, member.delete_start, member.delete_end, ""))
}

fn member_indent(text: &str, open_brace: usize) -> String {
    if let Ok(members) = object_members(text, open_brace) {
        if let Some(member) = members.first() {
            if let Some(line_start) = text[..member.key_start].rfind('\n') {
                let prefix = &text[line_start + 1..member.key_start];
                if prefix.chars().all(|c| c == ' ' || c == '\t') {
                    return prefix.to_string();
                }
            }
        }
    }
    let unit = detect_indent_unit(text);
    let line_start = text[..open_brace].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let base = if text[line_start..open_brace]
        .chars()
        .all(|c| c == ' ' || c == '\t')
    {
        text[line_start..open_brace].to_string()
    } else {
        String::new()
    };
    format!("{base}{unit}")
}

fn object_value_brace(text: &str, value_start: usize, value_end: usize) -> Result<usize, String> {
    let slice = &text[value_start..value_end];
    if !slice.trim_start().starts_with('{') {
        return Err("expected an object".into());
    }
    let rel = slice.find('{').ok_or("expected an object")?;
    Ok(value_start + rel)
}

fn root_brace(text: &str) -> Result<usize, String> {
    text.find('{').ok_or_else(|| "no opening brace".to_string())
}

fn detect_indent_unit(text: &str) -> String {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let indent_len = line.len() - trimmed.len();
        if indent_len > 0 {
            return line[..indent_len].to_string();
        }
    }
    "  ".to_string()
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn splice(text: &str, start: usize, end: usize, insert: &str) -> String {
    let mut out = String::with_capacity(text.len() + insert.len());
    out.push_str(&text[..start]);
    out.push_str(insert);
    out.push_str(&text[end..]);
    out
}

fn skip_ws_comments(text: &str, mut i: usize) -> usize {
    let bytes = text.as_bytes();
    loop {
        if i >= bytes.len() {
            return i;
        }
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            _ => return i,
        }
    }
}

fn parse_json_string(text: &str, start: usize) -> Result<(String, usize), String> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return Err("expected a string".into());
    }
    let mut i = start + 1;
    let mut out = String::new();
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                if i + 1 >= bytes.len() {
                    return Err("bad escape".into());
                }
                match bytes[i + 1] {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    _ => return Err("unsupported string escape".into()),
                }
                i += 2;
            }
            b'"' => return Ok((out, i + 1)),
            _ => {
                let ch = text[i..].chars().next().ok_or("bad string")?;
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    Err("unterminated string".into())
}

fn scan_value_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let (_, after) = parse_json_string(text, i).ok()?;
                if depth == 0 {
                    return Some(after);
                }
                i = after;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
                i += 1;
            }
            b',' if depth == 0 => return Some(i),
            _ => i += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const FIXTURE: &str = r#"{
  "location": {"directory": "/tmp"},
  "data": [
    {"providerID": "acme", "modelID": "good", "type": "chat", "capabilities": {"tools": true}, "variants": [{"id": "fast"}]},
    {"providerID": "acme", "modelID": "silent", "type": "chat", "capabilities": {"tools": false}},
    {"providerID": "acme", "modelID": "image", "type": "image", "capabilities": {"tools": true}},
    {"providerID": "acme", "modelID": "bare", "capabilities": {"tools": true}}
  ]
}"#;

    struct TempHome {
        path: PathBuf,
    }

    impl TempHome {
        fn new() -> Self {
            let path =
                PathBuf::from("/tmp/opencode").join(format!("vc-setup-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn flags(
        storyteller: Option<&str>,
        designer: Option<&str>,
        reviewer: Option<&str>,
        worker: Option<&str>,
        list: bool,
    ) -> RoleFlags {
        RoleFlags {
            storyteller: storyteller.map(str::to_string),
            designer: designer.map(str::to_string),
            reviewer: reviewer.map(str::to_string),
            worker: worker.map(str::to_string),
            list,
        }
    }

    fn write_config(home: &Path, body: &str) -> PathBuf {
        let root = home.join(".config/opencode");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("opencode.jsonc");
        fs::write(&path, body).unwrap();
        path
    }

    fn fake_opencode(home: &Path, catalog: &str) -> PathBuf {
        let dir = home.join("bin");
        fs::create_dir_all(&dir).unwrap();
        let catalog_path = dir.join("catalog.json");
        fs::write(&catalog_path, catalog).unwrap();
        let script = dir.join("opencode");
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = api ] && [ \"$2\" = get ] && [ \"$3\" = /api/model ]; then\n  cat \"{}\"\n  exit 0\nfi\necho \"unexpected: $*\" >&2\nexit 2\n",
            catalog_path.display()
        );
        write_executable(&script, &body);
        script
    }

    fn script(home: &Path, name: &str, body: &str) -> PathBuf {
        let path = home.join(name);
        write_executable(&path, body);
        path
    }

    /// Write the executable under a temporary name, close it, then rename.
    /// Executing a path that is still open for write returns ETXTBSY.
    fn write_executable(path: &Path, body: &str) {
        let tmp = path.with_extension("tmp");
        {
            let mut file = fs::File::create(&tmp).unwrap();
            file.write_all(body.as_bytes()).unwrap();
            file.set_permissions(fs::Permissions::from_mode(0o755))
                .unwrap();
            file.sync_all().unwrap();
        }
        fs::rename(&tmp, path).unwrap();
    }

    #[test]
    fn planning_agents_are_formwork_coder_only() {
        assert_eq!(PLANNOTATOR_AGENTS, ["formwork"]);
        for (name, _) in OPENCODE_SUBAGENTS {
            assert!(
                !PLANNOTATOR_AGENTS.contains(&name),
                "planningAgents must not list hidden role {name}"
            );
        }
    }

    #[test]
    fn install_targets_are_schema_and_opencode() {
        fn name(target: InstallTarget) -> &'static str {
            match target {
                InstallTarget::Schema => "schema",
                InstallTarget::Opencode => "opencode",
            }
        }
        assert_eq!(name(InstallTarget::Schema), "schema");
        assert_eq!(name(InstallTarget::Opencode), "opencode");
    }

    #[test]
    fn catalog_keeps_tool_capable_chat_models_and_listed_variants() {
        let ids = parse_catalog(FIXTURE).unwrap();
        assert!(ids.contains("acme/good"));
        assert!(ids.contains("acme/good#fast"));
        assert!(ids.contains("acme/bare"));
        assert!(!ids.contains("acme/silent"));
        assert!(!ids.contains("acme/image"));
        assert!(!ids.contains("acme/good#other"));
    }

    #[test]
    fn install_writes_prompts_without_model_keys_or_models_ini() {
        let home = TempHome::new();
        let sync = install_skill_at_home(&home.path).unwrap();
        assert_eq!(
            sync.plugin_dir,
            home.path.join(".config/opencode/plugins/formwork")
        );
        for (name, body) in OPENCODE_PLUGIN {
            assert_eq!(
                fs::read_to_string(sync.plugin_dir.join(name)).unwrap(),
                body
            );
        }
        assert!(!home.path.join(".formwork/models.ini").exists());
        assert_eq!(
            fs::read_to_string(&sync.skill_file).unwrap(),
            OPENCODE_SKILL
        );
        assert_eq!(sync.agent_files.len(), 5);
        let agents = home.path.join(".config/opencode/agents");
        let primary = fs::read_to_string(agents.join("formwork.md")).unwrap();
        assert_eq!(primary, OPENCODE_PRIMARY.1);
        assert!(primary.contains("mode: primary"));
        for (name, body) in OPENCODE_SUBAGENTS {
            let installed = fs::read_to_string(agents.join(format!("{name}.md"))).unwrap();
            assert_eq!(installed, body);
            assert!(installed.contains("hidden: true"));
            assert!(!installed
                .lines()
                .any(|line| line.trim_start().starts_with("model:")));
        }
        let doc: Value = json5::from_str(&fs::read_to_string(&sync.config_file).unwrap()).unwrap();
        assert!(doc.get("agent").is_none());
        assert!(doc.get("agents").is_none());
        assert_eq!(doc["subagent_depth"], 3);
        assert_eq!(
            doc["plugins"][0]["options"]["planningAgents"],
            json!(["formwork"])
        );
    }

    #[test]
    fn install_updates_v2_planning_agents_and_keeps_other_keys() {
        let home = TempHome::new();
        let raw = "{\n  // keep me\n  \"providers\": { \"local\": { \"name\": \"L\" } },\n  \"plugins\": [\n    {\n      \"package\": \"@plannotator/opencode@latest\",\n      \"options\": {\n        \"workflow\": \"plan-agent\",\n        \"planningAgents\": [\"plan\", \"build\"]\n      }\n    }\n  ],\n  \"experimental\": { \"subagent_depth\": 3 }\n}\n";
        write_config(&home.path, raw);
        install_skill_at_home(&home.path).unwrap();
        let text = fs::read_to_string(home.path.join(".config/opencode/opencode.jsonc")).unwrap();
        assert!(text.contains("// keep me"));
        let doc: Value = json5::from_str(&text).unwrap();
        assert_eq!(doc["providers"]["local"]["name"], "L");
        assert_eq!(
            doc["plugins"][0]["options"]["planningAgents"],
            json!(["formwork"])
        );
        assert_eq!(doc["plugins"][0]["options"]["workflow"], "plan-agent");
        assert_eq!(doc["experimental"]["subagent_depth"], 3);
        assert!(doc.get("subagent_depth").is_none());
        assert!(doc.get("agents").is_none());
        assert!(!home.path.join(".formwork/models.ini").exists());
    }

    #[test]
    fn v1_plugin_tuple_planning_agents_become_formwork_coder() {
        let raw = "{\n  \"plugin\": [\n    [\"@plannotator/opencode@1.2.3\", {\n      \"workflow\": \"user-managed\",\n      \"planningAgents\": [\"plan\", \"build\"]\n    }]\n  ]\n}\n";
        let (out, status) = merge_plannotator_plugin(raw).unwrap();
        assert_eq!(status, PlannotatorStatus::Merged);
        let doc: Value = json5::from_str(&out).unwrap();
        assert_eq!(doc["plugin"][0][0], "@plannotator/opencode@1.2.3");
        assert_eq!(doc["plugin"][0][1]["workflow"], "user-managed");
        assert_eq!(doc["plugin"][0][1]["planningAgents"], json!(["formwork"]));
    }

    #[test]
    fn subagent_depth_respects_experimental_floor() {
        let (text, outcome) = ensure_subagent_depth("{\n  \"shell\": \"zsh\"\n}\n").unwrap();
        assert_eq!(outcome, DepthOutcome::Inserted);
        let doc: Value = json5::from_str(&text.unwrap()).unwrap();
        assert_eq!(doc["subagent_depth"], 3);

        let (text, outcome) = ensure_subagent_depth("{\n  \"subagent_depth\": 1\n}\n").unwrap();
        assert_eq!(outcome, DepthOutcome::Raised(1));
        let doc: Value = json5::from_str(&text.unwrap()).unwrap();
        assert_eq!(doc["subagent_depth"], 3);

        let (text, outcome) =
            ensure_subagent_depth("{\n  \"experimental\": { \"subagent_depth\": 3 }\n}\n").unwrap();
        assert_eq!(outcome, DepthOutcome::Satisfied(3));
        assert!(text.is_none());
    }

    #[test]
    fn unknown_model_writes_nothing() {
        let home = TempHome::new();
        let body = "{\n  // keep\n  \"agents\": {\n    \"preflight\": { \"model\": \"drop/me\" },\n    \"worker\": { \"model\": \"keep/worker\" }\n  }\n}\n";
        let path = write_config(&home.path, body);
        let opencode = fake_opencode(&home.path, FIXTURE);
        let err = run_setup(
            &home.path,
            &flags(Some("acme/missing"), None, None, None, false),
            &opencode,
        )
        .unwrap_err();
        assert_eq!(err.reason, "unknown_model");
        assert!(err.instruction.contains("Nothing was written"));
        assert_eq!(fs::read_to_string(&path).unwrap(), body);

        let err = run_setup(
            &home.path,
            &flags(Some("acme/good#other"), None, None, None, false),
            &opencode,
        )
        .unwrap_err();
        assert_eq!(err.reason, "unknown_model");
        assert_eq!(fs::read_to_string(&path).unwrap(), body);

        let err = run_setup(
            &home.path,
            &flags(Some("acme/silent"), None, None, None, false),
            &opencode,
        )
        .unwrap_err();
        assert_eq!(err.reason, "unknown_model");
        assert_eq!(fs::read_to_string(path).unwrap(), body);
    }

    #[test]
    fn setup_sets_only_the_requested_role() {
        let home = TempHome::new();
        let body = "{\n  // keep me\n  \"shell\": \"/bin/zsh\",\n  \"agents\": {\n    \"compaction\": { \"model\": \"keep/compaction\" },\n    \"worker\": { \"model\": \"keep/worker\" }\n  }\n}\n";
        write_config(&home.path, body);
        let opencode = fake_opencode(&home.path, FIXTURE);
        let report = run_setup(
            &home.path,
            &flags(Some("acme/good"), None, None, None, false),
            &opencode,
        )
        .unwrap();
        assert!(report.wrote);
        assert!(!home.path.join(".formwork/models.ini").exists());
        let text = fs::read_to_string(&report.config).unwrap();
        assert!(text.contains("// keep me"));
        let doc: Value = json5::from_str(&text).unwrap();
        assert_eq!(doc["shell"], "/bin/zsh");
        assert_eq!(doc["agents"]["storyteller"]["model"], "acme/good");
        assert_eq!(doc["agents"]["worker"]["model"], "keep/worker");
        assert_eq!(doc["agents"]["compaction"]["model"], "keep/compaction");
        assert!(doc["agents"].get("designer").is_none());
        assert!(doc.get("agent").is_none());
        assert!(report.notes.iter().any(|note| note.contains("storyteller")));
        assert!(!report.notes.iter().any(|note| note.contains("worker")));
    }

    #[test]
    fn removing_a_middle_agent_does_not_leave_a_blank_line() {
        let raw = "{\n  \"agents\": {\n    \"compaction\": { \"model\": \"keep/compaction\" },\n    \"preflight\": { \"model\": \"drop/me\" },\n    \"qa\": { \"model\": \"drop/me\" },\n    \"worker\": { \"model\": \"keep/worker\" }\n  }\n}\n";
        let out = apply_setup_edits(
            raw,
            &[
                ("storyteller", Choice::Keep),
                ("designer", Choice::Keep),
                ("reviewer", Choice::Keep),
                ("worker", Choice::Keep),
            ],
        )
        .unwrap();
        assert!(
            out.lines()
                .all(|line| line.is_empty() || !line.trim().is_empty()),
            "removed member left a blank line:\n{out}"
        );
        let doc: Value = json5::from_str(&out).unwrap();
        assert!(doc["agents"].get("preflight").is_none());
        assert!(doc["agents"].get("qa").is_none());
        assert_eq!(doc["agents"]["compaction"]["model"], "keep/compaction");
        assert_eq!(doc["agents"]["worker"]["model"], "keep/worker");
    }

    #[test]
    fn inherit_clears_the_model_and_removes_retired_agents() {
        let home = TempHome::new();
        let body = "{\n  \"agents\": {\n    \"compaction\": { \"model\": \"keep/compaction\" },\n    \"preflight\": { \"model\": \"drop/me\" },\n    \"qa\": { \"model\": \"drop/me\" },\n    \"storyteller\": { \"temperature\": 0.2, \"model\": \"old/id\" },\n    \"worker\": { \"model\": \"keep/worker\" }\n  }\n}\n";
        write_config(&home.path, body);
        let opencode = fake_opencode(&home.path, FIXTURE);
        let report = run_setup(
            &home.path,
            &flags(Some("inherit"), None, None, None, false),
            &opencode,
        )
        .unwrap();
        assert!(report.wrote);
        let doc: Value = json5::from_str(&fs::read_to_string(&report.config).unwrap()).unwrap();
        assert!(doc["agents"]["storyteller"].get("model").is_none());
        assert_eq!(doc["agents"]["storyteller"]["temperature"], 0.2);
        assert!(doc["agents"].get("preflight").is_none());
        assert!(doc["agents"].get("qa").is_none());
        assert_eq!(doc["agents"]["compaction"]["model"], "keep/compaction");
        assert_eq!(doc["agents"]["worker"]["model"], "keep/worker");
        assert!(!home.path.join(".formwork/models.ini").exists());
    }

    #[test]
    fn list_is_read_only_and_filters_the_catalog() {
        let home = TempHome::new();
        let body = "{\n  \"agents\": { \"designer\": { \"model\": \"keep/designer\" } }\n}\n";
        let path = write_config(&home.path, body);
        let opencode = fake_opencode(&home.path, FIXTURE);
        let report =
            run_setup(&home.path, &flags(None, None, None, None, true), &opencode).unwrap();
        assert!(!report.wrote);
        let listing = report.listing.unwrap();
        assert!(listing.contains("acme/good\n") || listing.contains("acme/good"));
        assert!(listing.contains("acme/good#fast"));
        assert!(listing.contains("acme/bare"));
        assert!(!listing.contains("acme/silent"));
        assert!(!listing.contains("acme/image"));
        assert!(listing.contains("designer: keep/designer"));
        assert!(listing.contains("storyteller: inherit"));
        assert_eq!(fs::read_to_string(path).unwrap(), body);
        assert!(!home.path.join(".formwork/models.ini").exists());
    }

    #[test]
    fn catalog_failures_write_nothing() {
        let home = TempHome::new();
        let body = "{\n  \"agents\": { \"preflight\": { \"model\": \"stay\" } }\n}\n";
        let path = write_config(&home.path, body);
        let down = script(&home.path, "down", "#!/bin/sh\necho down >&2\nexit 1\n");
        let err = run_setup(
            &home.path,
            &flags(Some("acme/good"), None, None, None, false),
            &down,
        )
        .unwrap_err();
        assert_eq!(err.reason, "catalog_unavailable");
        assert_eq!(fs::read_to_string(&path).unwrap(), body);

        let bad = script(
            &home.path,
            "bad",
            "#!/bin/sh\nif [ \"$1\" = api ]; then echo '{not-json'; exit 0; fi\nexit 2\n",
        );
        let err = run_setup(
            &home.path,
            &flags(Some("acme/good"), None, None, None, false),
            &bad,
        )
        .unwrap_err();
        assert_eq!(err.reason, "invalid_catalog");
        assert_eq!(fs::read_to_string(path).unwrap(), body);
    }

    #[test]
    fn setup_without_flags_rejects_when_stdin_is_not_a_terminal() {
        let home = TempHome::new();
        let err = run_setup(
            &home.path,
            &flags(None, None, None, None, false),
            Path::new("opencode"),
        )
        .unwrap_err();
        assert_eq!(err.reason, "setup_needs_roles");
        assert!(!home.path.join(".config/opencode/opencode.jsonc").exists());
        assert!(!home.path.join(".formwork/models.ini").exists());
    }

    #[test]
    fn variant_id_is_written_only_for_the_requested_role() {
        let home = TempHome::new();
        write_config(&home.path, "{\n  \"agents\": {}\n}\n");
        let opencode = fake_opencode(&home.path, FIXTURE);
        let report = run_setup(
            &home.path,
            &flags(None, Some("acme/good#fast"), None, None, false),
            &opencode,
        )
        .unwrap();
        let doc: Value = json5::from_str(&fs::read_to_string(report.config).unwrap()).unwrap();
        assert_eq!(doc["agents"]["designer"]["model"], "acme/good#fast");
        assert!(doc["agents"].get("storyteller").is_none());
    }
}

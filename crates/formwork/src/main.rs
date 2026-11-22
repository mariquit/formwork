// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

#![recursion_limit = "512"]
#![cfg_attr(test, allow(dead_code))]

mod agents;
mod blackboard;
mod cli;
mod design_lint;
mod execution;
mod install;
mod models;
mod pipeline;
mod prd;
mod schema;
mod state;
mod version;

// The craft checker shares these paths with the authored design protocol.
mod design_paths {
    pub const DESIGN_SYSTEM_PATH: &str = ".formwork/design-system.md";
    pub const UI_DIR: &str = ".formwork/ui";
}

pub(crate) const OPENCODE_PRIMARY: (&str, &str) = (
    "formwork",
    include_str!("../../../opencode-agents/formwork.md"),
);
pub(crate) const OPENCODE_SUBAGENTS: [(&str, &str); 4] = [
    (
        "storyteller",
        include_str!("../../../opencode-agents/storyteller.md"),
    ),
    (
        "designer",
        include_str!("../../../opencode-agents/designer.md"),
    ),
    (
        "reviewer",
        include_str!("../../../opencode-agents/reviewer.md"),
    ),
    ("worker", include_str!("../../../opencode-agents/worker.md")),
];
pub(crate) const OPENCODE_SKILL: &str = include_str!("../../../opencode-skill/SKILL.md");
pub(crate) const OPENCODE_PLUGIN: [(&str, &str); 2] = [
    (
        "index.ts",
        include_str!("../../../opencode-plugin/index.ts"),
    ),
    (
        "tools.mjs",
        include_str!("../../../opencode-plugin/tools.mjs"),
    ),
];

fn print_json(value: serde_json::Value) {
    println!("{}", agents::wrap_json(value));
}

fn main() {
    use cli::Commands;
    let cli = cli::parse_args();
    if let Some(target) = cli.install {
        install::cmd_install(target);
        return;
    }
    let Some(command) = cli.command else {
        use clap::CommandFactory;
        cli::Cli::command().print_help().unwrap();
        return;
    };
    let command = match command {
        Commands::Setup {
            storyteller,
            designer,
            reviewer,
            worker,
            list,
        } => {
            install::cmd_setup(storyteller, designer, reviewer, worker, list);
            return;
        }
        other => other,
    };
    let bb = blackboard::Blackboard::new(None);
    let readonly = matches!(
        command,
        Commands::Status | Commands::Log | Commands::DesignLint { .. }
    );
    let _lock = if readonly {
        None
    } else {
        match bb.lock_workspace() {
            Ok(lock) => Some(lock),
            Err(error) => error.exit(),
        }
    };
    if let Commands::DesignLint {
        paths,
        json,
        no_browser,
    } = command
    {
        let mode = if no_browser {
            design_lint::Mode::StaticOnly
        } else {
            design_lint::Mode::Auto
        };
        let tokens = design_lint::Tokens::from_design_system(bb.workspace());
        let mut reports = if paths.is_empty() {
            match design_lint::lint_workspace(bb.workspace(), mode) {
                Ok(reports) => reports,
                Err(error) => {
                    print_json(
                        serde_json::json!({"status":"error","reason":"lint_unavailable","error":error,"instruction":"Check lint inputs."}),
                    );
                    std::process::exit(2);
                }
            }
        } else {
            Vec::new()
        };
        for path in paths {
            match design_lint::lint_file(std::path::Path::new(&path), mode, tokens.as_ref()) {
                Ok(report) => reports.push(report),
                Err(error) => {
                    print_json(
                        serde_json::json!({"status":"error","reason":"lint_unavailable","error":error,"instruction":"Check lint inputs and browser availability."}),
                    );
                    std::process::exit(2);
                }
            }
        }
        let blocking = reports.iter().map(|r| r.blocking_count()).sum::<usize>();
        if json {
            print_json(
                serde_json::json!({"reports":reports.iter().map(|r| r.to_json()).collect::<Vec<_>>(),"blocking":blocking}),
            );
        } else {
            println!("{}", design_lint::format_reports(&reports));
        }
        if blocking > 0 {
            std::process::exit(1);
        }
        return;
    }
    let result = pipeline::dispatch(&bb, command);
    match result {
        Ok(value) => print_json(value),
        Err(error) => print_json(
            serde_json::json!({"status":"rejected","reason":error.reason,"error":error.message,"instruction":error.instruction}),
        ),
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "formwork", version = crate::version::VERSION)]
#[command(about = "Spec-driven delivery for solo development with OpenCode.")]
pub struct Cli {
    #[arg(long, short = 'n', exclusive = true, value_enum)]
    pub install: Option<InstallTarget>,
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum InstallTarget {
    Schema,
    Opencode,
}

#[derive(Subcommand)]
pub enum Commands {
    Start {
        idea: Vec<String>,
    },
    Task,
    Resume,
    Next,
    Status,
    Submit {
        artifacts: Option<String>,
    },
    Subagent {
        name: String,
    },
    ApproveMilestone {
        id: String,
        #[arg(long)]
        design_revision: String,
    },
    /// Reopen design and invalidate test and execution approvals without deleting artifacts
    Rollback,
    Log,
    DesignLint {
        paths: Vec<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        no_browser: bool,
    },
    /// Assign role models from the models OpenCode can run now
    Setup {
        /// Catalog id `provider/model` or `provider/model#variant`, or `inherit`
        #[arg(long)]
        storyteller: Option<String>,
        /// Catalog id `provider/model` or `provider/model#variant`, or `inherit`
        #[arg(long)]
        designer: Option<String>,
        /// Catalog id `provider/model` or `provider/model#variant`, or `inherit`
        #[arg(long)]
        reviewer: Option<String>,
        /// Catalog id `provider/model` or `provider/model#variant`, or `inherit`
        #[arg(long)]
        worker: Option<String>,
        /// Print the tool-capable catalog and the four current pins
        #[arg(long)]
        list: bool,
    },
}

pub fn parse_args() -> Cli {
    Cli::parse()
}

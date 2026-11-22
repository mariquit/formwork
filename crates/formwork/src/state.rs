// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

pub const PHASES: &[&str] = &[
    "init",
    "storytelling",
    "storytelling:complete",
    "design",
    "design:review",
    "design:complete",
    "build",
    "build:complete",
];

pub fn phase_agent(phase: &str) -> Option<&'static str> {
    match phase {
        "storytelling" => Some("storyteller"),
        "design" => Some("designer"),
        "design:review" => Some("reviewer"),
        "build" => Some("worker"),
        _ => None,
    }
}

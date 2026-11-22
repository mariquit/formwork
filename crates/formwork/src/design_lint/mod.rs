// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

//! Design lint: a deterministic craft gate over the designer's `ui.html`
//! mockups.
//!
//! Two engines, same rule vocabulary:
//!
//! * **CDP (preferred)** — drives a headless Chrome out-of-process over the
//!   DevTools Protocol and reads *computed* styles plus real layout geometry.
//!   This is the only engine that sees through Tailwind utility classes: a
//!   `border-l-4 border-violet-500` class resolves to
//!   `border-left: 4px rgb(139,92,246)` only after the browser applies the
//!   cascade. Measured on a Tailwind mockup: 13 findings vs 4 static.
//! * **Static (fallback)** — pure text/CSS heuristics with no external
//!   process. Runs when no browser is installed or `--no-browser` is passed.
//!   It genuinely cannot resolve utility classes, so its findings are a
//!   subset and every report says so.
//!
//! No CSS engine is linked into the binary: Chrome does the cascade work in
//! its own process, so the only dependency this adds is a WebSocket client.
//!
//! The rule set and its thresholds are derived from prior art in AI-UI
//! anti-pattern detection (see NOTICE); the implementation is ours.

mod cdp;
mod color;
mod rules;
mod statik;

pub use rules::Tokens;

use serde_json::Value;
use std::path::{Path, PathBuf};

/// How a finding affects the gate. Mirrors the reviewer's vocabulary so a
/// lint finding and a review finding can be read on the same scale.
///
/// The split follows one principle: **the more objective and
/// context-independent a failure is, the more willing we are to block on it.**
///
/// * `Major` — blocks the submission. Either an objectively measurable failure
///   against a published standard (`low-contrast`, WCAG AA), or one of the
///   narrow set of patterns that are the recognizable tells of generated UI and
///   are essentially never the right answer for a bespoke design: `side-tab`,
///   `glow-shadow`, `nested-cards`, `gradient-text`, `kicker-above-heading`.
///   The design phase exists to prevent exactly these, so they are worth a hard
///   stop.
/// * `Minor` — real craft problems that a project may legitimately want. Its
///   brand face genuinely may be Inter (`default-display-face`); 13px meta text
///   may be a deliberate choice. Reported for the designer and reviewer to
///   weigh, never blocking.
/// * `Advisory` — rules with a known false-positive tail.
///
/// Because `Major` rejects a submission, a blocking rule must abstain whenever
/// its inputs are uncertain: a false positive there blocks a legitimate design,
/// which costs more than a miss. See `rules::contrast` for what that looks like
/// in practice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Blocks the design gate.
    Major,
    /// Reported, does not block.
    Minor,
    /// Reported for context only; never blocks, never counted as a failure.
    /// Used for rules with a known false-positive tail.
    Advisory,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Major => "major",
            Severity::Minor => "minor",
            Severity::Advisory => "advisory",
        }
    }

    /// Only major findings fail the gate. Minor and advisory are informational,
    /// so a design is never blocked by a rule with a soft false-positive tail.
    pub fn blocks(self) -> bool {
        matches!(self, Severity::Major)
    }
}

/// Which engine produced a report. Callers must surface this: a `Static`
/// report is a genuinely weaker check and claiming otherwise would overstate
/// the guarantee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// Headless Chrome over CDP: computed styles + layout geometry.
    Cdp,
    /// Text/CSS heuristics only. Cannot resolve Tailwind utility classes.
    Static,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Cdp => "cdp",
            Engine::Static => "static",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    /// Stable rule id, e.g. `low-contrast`.
    pub rule: &'static str,
    pub severity: Severity,
    /// Where it was found: a selector-ish path or a `line N` hint.
    pub location: String,
    /// The concrete measured evidence — never a restatement of the rule.
    /// e.g. `1.9:1 (need 4.5:1) — #b8bec7 on #ffffff`.
    pub evidence: String,
    /// What to do instead.
    pub remedy: &'static str,
}

impl Finding {
    fn to_json(&self) -> Value {
        serde_json::json!({
            "rule": self.rule,
            "severity": self.severity.as_str(),
            "location": self.location,
            "evidence": self.evidence,
            "remedy": self.remedy,
        })
    }
}

/// One linted file.
#[derive(Debug, Clone)]
pub struct Report {
    pub path: PathBuf,
    pub engine: Engine,
    pub findings: Vec<Finding>,
    /// Set when the CDP engine was wanted but unavailable, explaining why the
    /// run degraded. A silent degrade would be a false assurance.
    pub degraded_reason: Option<String>,
}

impl Report {
    pub fn blocking(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.severity.blocks())
    }

    pub fn blocking_count(&self) -> usize {
        self.blocking().count()
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "path": self.path.to_string_lossy(),
            "engine": self.engine.as_str(),
            "degraded_reason": self.degraded_reason,
            "blocking": self.blocking_count(),
            "findings": self.findings.iter().map(Finding::to_json).collect::<Vec<_>>(),
        })
    }
}

/// Engine selection for a lint run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Try CDP, fall back to static with a recorded reason.
    Auto,
    /// Never launch a browser.
    StaticOnly,
}

impl Mode {
    /// Honor `FORMWORK_DESIGN_LINT=static|off` so a CI runner (or the test
    /// suite) can keep the gate hermetic and fast without a browser, and so an
    /// operator can disable the gate outright if it misfires on their design.
    fn resolve(self) -> Option<Mode> {
        self.resolve_with(&std::env::var("FORMWORK_DESIGN_LINT").unwrap_or_default())
    }

    /// Pure half of `resolve`, so the precedence rules are testable without
    /// mutating the process environment.
    fn resolve_with(self, raw: &str) -> Option<Mode> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" => None,
            "static" | "no-browser" => Some(Mode::StaticOnly),
            _ => Some(self),
        }
    }
}

/// Lint one HTML file. Never returns `Err` for a rule failure — only for an
/// unreadable file. A missing browser degrades rather than fails, because a
/// design gate that hard-fails on a machine without Chrome would be
/// unshippable.
pub fn lint_file(
    path: &Path,
    mode: Mode,
    tokens: Option<&rules::Tokens>,
) -> Result<Report, String> {
    let html = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read {}: {}", path.display(), e))?;

    // FORMWORK_DESIGN_LINT=off disables the gate: report nothing rather than
    // pretend a check ran.
    let Some(mode) = mode.resolve() else {
        return Ok(Report {
            path: path.to_path_buf(),
            engine: Engine::Static,
            findings: Vec::new(),
            degraded_reason: Some("disabled by FORMWORK_DESIGN_LINT=off".to_string()),
        });
    };

    if mode == Mode::StaticOnly {
        return Ok(Report {
            path: path.to_path_buf(),
            engine: Engine::Static,
            findings: statik::lint(&html, tokens),
            degraded_reason: None,
        });
    }

    match cdp::probe(path) {
        Ok(elements) => Ok(Report {
            path: path.to_path_buf(),
            engine: Engine::Cdp,
            findings: rules::run(&elements, tokens),
            degraded_reason: None,
        }),
        Err(reason) => Ok(Report {
            path: path.to_path_buf(),
            engine: Engine::Static,
            findings: statik::lint(&html, tokens),
            degraded_reason: Some(reason),
        }),
    }
}

/// Lint every mockup under `.formwork/ui/`, using the design system's tokens
/// when it declares any. Mockups are one flat directory of `<slice-id>.html`
/// files rather than a folder per slice.
pub fn lint_workspace(workspace: &Path, mode: Mode) -> Result<Vec<Report>, String> {
    let tokens = rules::Tokens::from_design_system(workspace);
    let ui_root = workspace.join(crate::design_paths::UI_DIR);
    let mut out = Vec::new();
    if !ui_root.is_dir() {
        return Ok(out);
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&ui_root)
        .map_err(|e| format!("Cannot read {}: {}", ui_root.display(), e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "html"))
        .collect();
    files.sort();
    for ui in files {
        out.push(lint_file(&ui, mode, tokens.as_ref())?);
    }
    Ok(out)
}

/// Human-readable report for the terminal.
pub fn format_reports(reports: &[Report]) -> String {
    let mut lines = Vec::new();
    let mut blocking = 0usize;
    let mut total = 0usize;

    for r in reports {
        if r.findings.is_empty() {
            continue;
        }
        lines.push(String::new());
        let mut header = format!("{} [{}]", r.path.display(), r.engine.as_str());
        if let Some(reason) = &r.degraded_reason {
            header.push_str(&format!(" DEGRADED: {}", reason));
        }
        lines.push(header);
        let mut sorted: Vec<&Finding> = r.findings.iter().collect();
        sorted.sort_by_key(|f| (f.severity, f.rule));
        for f in sorted {
            total += 1;
            if f.severity.blocks() {
                blocking += 1;
            }
            lines.push(format!(
                "  [{}] {} — {}",
                f.severity.as_str(),
                f.rule,
                f.evidence
            ));
            lines.push(format!("      at {}", f.location));
            lines.push(format!("      -> {}", f.remedy));
        }
    }

    let degraded: Vec<&Report> = reports
        .iter()
        .filter(|r| r.degraded_reason.is_some())
        .collect();
    lines.push(String::new());
    if total == 0 {
        lines.push("design-lint: no findings.".to_string());
    } else {
        lines.push(format!(
            "design-lint: {} finding(s), {} blocking.",
            total, blocking
        ));
    }
    if !degraded.is_empty() {
        lines.push(format!(
            "WARNING: {} file(s) linted without a browser — Tailwind utility classes were NOT resolved, so this is a partial check.",
            degraded.len()
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str, html: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("formwork-design-lint-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, html).unwrap();
        p
    }

    /// A Tailwind mockup is the case that motivated the browser engine: the
    /// styling lives in utility classes, so only a real cascade resolves it.
    /// Skips (rather than fails) when the machine has no Chrome, so the suite
    /// stays portable.
    #[test]
    fn cdp_resolves_tailwind_utility_classes() {
        if cdp::find_browser().is_err() {
            eprintln!("skipping: no browser available");
            return;
        }
        let p = fixture(
            "tailwind.html",
            r#"<!DOCTYPE html><html><head><style>
               /* Local stand-in for the utilities, so the test never needs the network. */
               .bg-slate-950{background:#020617}
               .bg-slate-900{background:#0f172a}
               .text-slate-500{color:#64748b}
               .border-l-4{border-left-width:4px;border-left-style:solid}
               .border-violet-500{border-left-color:#8b5cf6}
               .rounded-xl{border-radius:12px}
               .shadow-glow{box-shadow:0 0 24px rgba(139,92,246,.5)}
               .text-sm{font-size:14px}
               </style></head><body class="bg-slate-950">
               <div class="rounded-xl border-l-4 border-violet-500 bg-slate-900 shadow-glow"
                    style="width:400px;height:140px">
                 <p class="text-slate-500 text-sm">Muted caption text</p>
               </div></body></html>"#,
        );
        let r = lint_file(&p, Mode::Auto, None).unwrap();
        // Skip (don't fail) when a browser is unavailable OR could not launch
        // in time. Under the full parallel suite several tests may contend for
        // resources and a launch can time out; that is an environment
        // condition, not a bug in the rules under test. The dedicated CDP path
        // is still covered whenever a browser is reachable.
        if r.engine != Engine::Cdp {
            eprintln!("skipping CDP assertions: {:?}", r.degraded_reason);
            return;
        }

        let rules: Vec<&str> = r.findings.iter().map(|f| f.rule).collect();
        // Each of these is only visible once the cascade has been applied.
        assert!(rules.contains(&"side-tab"), "got {:?}", rules);
        assert!(rules.contains(&"glow-shadow"), "got {:?}", rules);
        assert!(rules.contains(&"low-contrast"), "got {:?}", rules);

        // The contrast finding must carry measured evidence, not a restatement.
        let c = r
            .findings
            .iter()
            .find(|f| f.rule == "low-contrast")
            .unwrap();
        assert!(c.evidence.contains(":1"), "evidence was {:?}", c.evidence);
        assert!(c.evidence.contains('#'), "evidence was {:?}", c.evidence);
    }

    /// A well-made mockup must produce no blocking findings. If this ever
    /// fails, the gate has become unshippable regardless of rule quality.
    #[test]
    fn clean_mockup_passes_both_engines() {
        let p = fixture(
            "clean.html",
            r#"<!DOCTYPE html><html><head><style>
               body{background:#faf7f2;color:#1f1d1a;font-family:"Iowan Old Style",Georgia,serif;margin:0}
               .wrap{max-width:640px;margin:0 auto;padding:48px 24px}
               h1{font-size:38px;line-height:1.15}
               h2{font-size:21px}
               p{font-size:17px;line-height:1.6}
               .act{min-height:44px;padding:12px 20px;background:#7a2e1e;color:#fff;border-radius:4px;font-size:16px}
               </style></head><body><div class="wrap">
               <h1>Reconcile a print run</h1>
               <p>Match the delivered quantity against the invoice.</p>
               <h2>Open runs</h2>
               <button class="act">Post variance</button>
               </div></body></html>"#,
        );
        for mode in [Mode::StaticOnly, Mode::Auto] {
            let r = lint_file(&p, mode, None).unwrap();
            assert_eq!(
                r.blocking_count(),
                0,
                "{:?} engine blocked a clean mockup: {:?}",
                r.engine,
                r.findings
            );
        }
    }

    /// The degrade path must be explicit. A silent fallback would let a partial
    /// check read as a full one.
    #[test]
    fn static_mode_reports_its_own_limits() {
        let p = fixture(
            "degrade.html",
            r#"<div class="card"><div class="card">x</div></div>"#,
        );
        let r = lint_file(&p, Mode::StaticOnly, None).unwrap();
        assert_eq!(r.engine, Engine::Static);
        assert!(r.blocking_count() > 0);

        let text = format_reports(&[r]);
        assert!(text.contains("nested-cards"));
    }

    /// `Mode::resolve` is the whole disable mechanism, so test it directly.
    /// Mutating the process environment from a test would leak into every other
    /// test in the same binary (they share one process).
    #[test]
    fn env_override_selects_the_engine() {
        // Default: caller's choice is honored.
        assert_eq!(Mode::Auto.resolve_with(""), Some(Mode::Auto));
        assert_eq!(Mode::Auto.resolve_with("unset-garbage"), Some(Mode::Auto));
        // Forced static.
        assert_eq!(Mode::Auto.resolve_with("static"), Some(Mode::StaticOnly));
        assert_eq!(
            Mode::Auto.resolve_with("no-browser"),
            Some(Mode::StaticOnly)
        );
        // Disabled outright.
        assert_eq!(Mode::Auto.resolve_with("off"), None);
        assert_eq!(Mode::Auto.resolve_with("0"), None);
        assert_eq!(Mode::Auto.resolve_with("FALSE"), None);
    }

    /// Locks the severity policy in place. Changing a rule's severity is a
    /// deliberate product decision, so it should require editing this test.
    #[test]
    fn severity_policy_is_stable() {
        let p = fixture(
            "severity.html",
            r#"<!DOCTYPE html><html><head><style>
               body{background:#0b0b12;font-family:Inter,sans-serif}
               .tab{border-left:4px solid #8b5cf6;border-radius:12px;background:#141420;
                    box-shadow:0 0 24px rgba(139,92,246,.5);width:400px;height:160px}
               .inner{background:#141420;border-radius:12px;width:300px;height:80px}
               .muted{color:#6b7280;font-size:14px}
               .kick{font-size:12px;letter-spacing:3px;text-transform:uppercase;color:#8b5cf6}
               h1{font-size:44px;color:#f8fafc}
               </style></head><body>
               <p class="kick">INTRODUCING</p>
               <h1>Ledger</h1>
               <div class="tab"><div class="inner"><p class="muted">Muted caption</p></div></div>
               </body></html>"#,
        );
        let r = lint_file(&p, Mode::Auto, None).unwrap();
        if r.engine != Engine::Cdp {
            eprintln!("skipping: no browser available");
            return;
        }
        let sev = |rule: &str| {
            r.findings
                .iter()
                .find(|f| f.rule == rule)
                .map(|f| f.severity)
        };

        // Objective, measurable against WCAG: blocks.
        assert_eq!(
            sev("low-contrast"),
            Some(Severity::Major),
            "{:?}",
            r.findings
        );
        // Recognizable tells of generated UI: block.
        assert_eq!(sev("side-tab"), Some(Severity::Major));
        assert_eq!(sev("glow-shadow"), Some(Severity::Major));
        assert_eq!(sev("nested-cards"), Some(Severity::Major));
        assert_eq!(sev("kicker-above-heading"), Some(Severity::Major));
        // Taste: a project's brand face may legitimately be Inter.
        assert_eq!(sev("default-display-face"), Some(Severity::Minor));
    }

    /// A blocking rule must abstain when it cannot know the answer. Each case
    /// here would otherwise reject a legitimate design.
    #[test]
    fn contrast_abstains_when_the_backdrop_is_unknown() {
        let p = fixture(
            "abstain.html",
            r#"<!DOCTYPE html><html><head><style>
               body{background:#ffffff}
               /* Text over a photo: the painted backdrop is not a readable color. */
               .hero{background-image:url(data:image/gif;base64,R0lGODlhAQABAAAAACw=);padding:40px}
               .hero p{color:#f8fafc;font-size:16px}
               /* Gradient-clipped text is painted from its own background. */
               .grad{background-image:linear-gradient(90deg,#8b5cf6,#22d3ee);
                     -webkit-background-clip:text;background-clip:text;color:transparent;font-size:40px}
               /* Visually hidden: not visually present at all. */
               .sr{position:absolute;width:1px;height:1px;overflow:hidden;color:#eee}
               /* Faded out (pre-animation state). */
               .faded{opacity:.2;color:#cccccc;font-size:16px}
               </style></head><body>
               <div class="hero"><p>White text over a photograph</p></div>
               <p class="grad">Gradient headline</p>
               <span class="sr">Screen reader only</span>
               <p class="faded">Fading in</p>
               </body></html>"#,
        );
        let r = lint_file(&p, Mode::Auto, None).unwrap();
        if r.engine != Engine::Cdp {
            eprintln!("skipping: no browser available");
            return;
        }
        let contrast: Vec<&Finding> = r
            .findings
            .iter()
            .filter(|f| f.rule == "low-contrast")
            .collect();
        assert!(
            contrast.is_empty(),
            "contrast must abstain on unknown backdrops, got: {:?}",
            contrast
                .iter()
                .map(|f| (&f.location, &f.evidence))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn only_major_findings_block() {
        assert!(Severity::Major.blocks());
        assert!(!Severity::Minor.blocks());
        assert!(!Severity::Advisory.blocks());
    }

    /// Crash-only corpus: real-world-shaped adversarial `ui.html` files, run
    /// through the public `lint_file` entry point in the static engine (no
    /// browser needed, so CI-safe). A design mockup is untrusted input as far
    /// as the linter is concerned; none of these may panic, and each must yield
    /// a report rather than an error.
    #[test]
    fn lint_file_survives_adversarial_corpus() {
        let corpus: &[&str] = &[
            // Empty and near-empty.
            "",
            " ",
            "\n\n\n",
            "<!DOCTYPE html>",
            // Unbalanced / truncated tags.
            "<div",
            "<div class=",
            "<div class=\"",
            "<style>",
            "<style>.a{color:#",
            "</div></div></div>",
            "<script>while(true){}", // never executed: static engine reads text
            // Multi-byte adjacent to every scanned token (the shipped-bug class).
            "<style>border-left:4px solid #8b5cf6\u{2014}</style><div class=\"card\">\u{201c}\u{201d}</div>",
            "<h1>\u{65e5}\u{672c}</h1><h3>\u{2192}</h3>",
            "font-family:\u{2014}inter; box-shadow:0 0 8px #8b5cf6",
            // Deeply nested cards (stack depth).
            &"<div class=\"card\">".repeat(500),
            // Pathological repetition.
            &"border-l-4 ".repeat(2000),
            &"#8b5cf6 ".repeat(3000),
            // Colors in every broken form.
            "<style>color:rgb(;background:rgba(0,0,0,NaN);border-left:9px solid oklch(1 2 3)</style>",
            // Lone delimiters.
            "<<<>>>{{{}}};;;///",
            // A giant single line (line-scanning perf/bounds).
            &format!("<style>{}</style>", "a".repeat(50_000)),
        ];

        let dir = std::env::temp_dir().join("formwork-lint-corpus");
        std::fs::create_dir_all(&dir).unwrap();
        for (i, html) in corpus.iter().enumerate() {
            let p = dir.join(format!("case-{i}.html"));
            std::fs::write(&p, html).unwrap();
            // StaticOnly keeps this hermetic; the goal is crash resistance, not
            // finding accuracy.
            let r = lint_file(&p, Mode::StaticOnly, None)
                .unwrap_or_else(|e| panic!("case {i} errored instead of reporting: {e}"));
            // format_reports also walks every finding — exercise it too.
            let _ = format_reports(&[r]);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

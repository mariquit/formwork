# AGENTS.md

Guidance for coding agents working in this repository.

This repository builds **Formwork**, a Rust CLI (edition 2021, no async runtime or web server). The binary owns state and gates. The Formwork primary agent launches one hidden role at a time: Storyteller, Designer, Reviewer, or Worker. Read [WORKFLOW.md](WORKFLOW.md) before changing transitions, [README.md](README.md) for gate guarantees and limitations, and [CONTRIBUTING.md](CONTRIBUTING.md) before committing.

## Source layout

| Path | Responsibility |
| --- | --- |
| `crates/formwork/src/main.rs` | CLI entrypoint, authored integration embedding, standalone craft command |
| `crates/formwork/src/pipeline.rs` | Stage transitions, task identity, bounded audit, protected specifications/tests, milestone approval and deterministic acceptance |
| `crates/formwork/src/execution.rs` | Bounded noninteractive commands, logs, executable identities, source and environment fingerprints |
| `crates/formwork/src/blackboard.rs` | Atomic state persistence, workspace locking, metadata registry and protocol validation |
| `crates/formwork/src/cli.rs` | All clap command and flag declarations |
| `crates/formwork/src/prd.rs` | Mechanical product validation and stable FEAT/US/AC/NG/UN references |
| `crates/formwork/src/schema.rs` | Installed protocol schemas |
| `crates/formwork/src/models.rs` | Home directory and the harness target recorded on a project |
| `crates/formwork/src/install.rs` | Prompt installation, Plannotator config, and live-catalog role model setup |
| `opencode-plugin/` | OpenCode V2 task and submission tools. The adapter delegates to the Rust engine without a shell. |
| `crates/formwork/src/design_lint/` | Browser/static craft checker, rule evidence and WCAG contrast |
| `crates/formwork-report/src/main.rs` | Read-only spec-driven status reports |

## Authored prompts

`opencode-agents/formwork.md`, `opencode-agents/{storyteller,designer,reviewer,worker}.md`, and `opencode-skill/SKILL.md` are the single source of truth. Edit local files only, never installed copies in `~/.config/opencode/`. Rust embeds and copies these files verbatim; never generate or template prompt content in code. A rebuild and explicit installation are needed to update global prompts, but do not install unless the user requests it.

OpenCode is the only harness target. Do not introduce duplicate prompt sets. One visible primary agent ships, plus four hidden delivery roles. Setup removes only the retired `preflight` and `qa` agent entries. Other installed files and config keys stay.

## Load-bearing invariants

- Only the binary writes `.formwork/state.json`. State contains metadata and evidence pointers, not artifact bodies.
- Review is adversarial and fresh-context. One refinement is allowed; unresolved blockers stop, never defer into implementation. Reviewer authors tests from the final spec before Worker implementation. No separate postimplementation LLM acceptance loop exists.
- Preserve approved MVP/later/stretch feature ownership. Obtain explicit user authorization before running `formwork approve-milestone` for any milestone, including the MVP. The CLI validates the stage, milestone, and design revision and binds the approval to the current design and test revisions. It does not verify user consent. Roadmap approval is not execution approval.
- Frozen contracts, test manifests, tests, fixtures and declared harness inputs are verified before and after execution. Changed or stale evidence is not a pass. Deterministic checks do not prove semantic adequacy; disclose limits.
- Unsupported state must be rejected without rewriting files or converting approvals. Never synthesize an approval fingerprint during state reads.
- Gate infrastructure errors must not masquerade as design defects or clean results. Browser loss visibly degrades to static craft checks. Missing required compiler/test tools never passes implementation.
- Workflow commands emit one JSON object on stdout. Check `status` rather than relying on the exit code: workflow rejections can exit 0 and include `reason` and `instruction`. `design-lint` defaults to prose for reports and emits JSON reports with `--json`. It exits 1 for blocking findings and emits a JSON error with exit code 2 when linting cannot run, even without `--json`. Installation uses human-readable output.
- Installation does not write model keys and does not create `models.ini`. `formwork setup` writes `agents.<role>.model` from the live OpenCode catalog, or removes that key for `inherit`. An unknown id writes nothing. Setup removes `preflight` and `qa` from `agents`. Other config keys survive.
- The Formwork primary session coordinates. Obtain explicit user approval for story and design plans with `submit_plan` in that session. If `submit_plan` is unavailable, disclose the fallback and ask for explicit approval with `question`. Child sessions cannot call `submit_plan` or claim approval on behalf of the primary session. Do not create a coordinator subagent.

## Build and tests

```sh
make test
make
```

`make test` must retain the order: formatting, Clippy with warnings as errors, debug build, Rust tests, and Node.js adapter tests. Clippy can evict the executable used by end-to-end tests. `make` builds release binaries without installing. `make install` also installs the OpenCode integration; only run it against the live home with installation authorization.

Tests live beside source in `#[cfg(test)]` modules. Use temporary workspaces, preferably under `/tmp/opencode`. Binary subprocess tests must set `FORMWORK_DESIGN_LINT=static` and clear `FORMWORK_TARGET`. Never overwrite or discard user work.

Keep `#![recursion_limit = "512"]` and test-only dead-code allowance in `main.rs`: macro expansion and replacement of the main entrypoint under tests have concrete effects. Production dead code must still fail clippy.

Add contract tests for coordinator-visible gate vocabulary and emitted protocol shape.

## Conventions

Every Rust source opens with SPDX and copyright lines. Authored prompts put those lines as YAML comments inside frontmatter. Use Formwork in prose and preserve programmatic identifiers. Comments explain rationale, especially abstentions, integrity boundaries and failure semantics. Keep changes scoped and consistent with surrounding idioms. Commits use Conventional Commits and DCO sign-off.

If `act` is on PATH and this change would make GitHub run a test, build, or release workflow under `.github/workflows/`, load the `act` skill and run that workflow locally before finishing; if `act` is not installed, skip.

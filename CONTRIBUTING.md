# Contributing to Formwork

Thanks for your interest in the project. This document covers how to build it,
the conventions the codebase already follows, and what a reviewable change looks
like.

Use **Formwork** in prose and lowercase programmatic names: `formwork` and
`formwork-report` for the binaries, and `formwork` for the OpenCode agent ID.

## Licensing of contributions

The software is licensed under the **Apache License, Version 2.0**
(see [LICENSE](LICENSE)). By submitting a contribution you agree that it is
licensed under the same terms.

**There is no CLA.** You keep the copyright to your work. Nothing here transfers
ownership or grants the maintainer rights beyond those in the license.

### Sign your work

This project uses the **Developer Certificate of Origin 1.1** (see [DCO](DCO))
instead of a CLA. It is a lightweight statement that you wrote the patch, or
otherwise have the right to submit it under Apache-2.0.

Certify it by signing off each commit:

```bash
git commit -s
```

which appends a trailer to the commit message:

```
Signed-off-by: Your Name <your.email@example.com>
```

Use your real name and a working email address. To sign off a commit you already
made, `git commit --amend -s`; for a longer branch,
`git rebase --signoff <base>`.

### Copyright headers

Every source file carries two lines, and new files should too:

```rust
// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>
```

The authored Markdown prompt files carry the same two lines as **YAML comments
inside** the frontmatter, on the first two lines after the opening `---`:

```markdown
---
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>
description: ...
---
```

Two constraints drive that placement. The frontmatter must stay the first bytes
of the file or the coding agent will not parse it, and the notice must live
inside the frontmatter rather than in the body — frontmatter is consumed as
metadata, so a comment there costs nothing at inference time, whereas a notice
in the body would be sent to the model on every single invocation. Keep them as
YAML *comments*, not keys: comments are dropped by every conformant parser,
while unknown keys are at the mercy of whichever schema the host agent applies.

Adding your own copyright line to a file you substantially author is welcome.
Preserve applicable notices and mark modified files as required by Apache-2.0
§4. The [NOTICE](NOTICE) file carries project and third-party attribution.

### Prior art

Do **not** paste code from a proprietary, unlicensed, or license-incompatible
source. If a change is adapted from prior art under a different licence, say so
in the pull request and add an entry to [NOTICE](NOTICE) — that file
already records the design-craft guidance adapted from Impeccable
(Apache-2.0), and it is the right precedent to follow.

### The project name

See [TRADEMARK.md](TRADEMARK.md) for naming and attribution guidance when
redistributing Formwork or a fork.

## Build and test

Requires a stable Rust toolchain 1.89 or newer (edition 2021).

```bash
make          # build release binaries without installing
make test     # the full gate — run this before every commit
make install  # install both binaries and the matching OpenCode integration
make clean    # remove build artifacts
```

## GitHub releases

1. Update the version in both crate manifests and refresh `Cargo.lock`. Commit the version changes and release notes to the GitHub repository.
2. Run `make test`, then run `RELEASE_TAG=vX.Y.Z make package` with your release version. The archive and its SHA-256 checksum appear in `target/dist/`. To verify installation without changing your home directory, run `sh scripts/test-release.sh target/dist/formwork-vX.Y.Z-linux-x86_64.tar.gz`.
3. Push the matching `vX.Y.Z` tag to GitHub. The Release workflow runs the test gate, checks both binary versions against the tag, smoke-tests installation, and publishes the archive and checksum to GitHub Releases. The first supported target is Linux x86-64.

### Test the workflow with act and Podman

`act` needs Podman's Docker-compatible API socket. Podman Compose is not needed because this workflow has no service containers. On Linux with a running rootless `podman.socket`, simulate a tag push from the repository root:

```sh
event=$(mktemp)
printf '%s\n' '{"ref":"refs/tags/v0.1.0"}' > "$event"
DOCKER_HOST="unix://$XDG_RUNTIME_DIR/podman/podman.sock" \
  act push -W .github/workflows/release.yml -j package -e "$event" \
  -P ubuntu-22.04=catthehacker/ubuntu:act-22.04 --container-daemon-socket -
```

Replace `v0.1.0` with the version in both crate manifests. The publish step is skipped under `act`; do not supply a release token for the local test.

If rootless `crun` reports permission denied for `/proc/self/oom_score_adj`, retry with `--container-options "--oom-score-adj=$(cat /proc/self/oom_score_adj)"` to match the host process's setting.

## Test gate

`make test` runs, in this order:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build
cargo test
node --test opencode-plugin/tools.test.mjs
```

**The ordering is deliberate.** Clippy evicts `target/debug/formwork`, which
the suite shells out to, so the build must come *after* it. If you rearrange the
`Makefile`, preserve that.

Warnings are errors. A change that compiles but trips clippy is not ready.

Override the install prefix with `make PREFIX=/usr/local` (the binary lands in
`$PREFIX/bin`).

## Architecture in one paragraph

`formwork` is a state-machine-driven orchestrator that drives coding agents
through **Story → Design → Design Review → Build**. The binary owns the state
machine, the validation gates, and the test gate; the agents do the work. State
lives in the project's `.formwork/state.json` (metadata only — never artifact
bodies), and artifacts live in the project workspace. Read
[WORKFLOW.md](WORKFLOW.md) before changing phase transitions.

## Agent prompts and skills live in this repository

This is the rule most likely to trip up a first contribution, so it also has its
own file: [AGENTS.md](AGENTS.md).

The coding-agent integrations are authored **only** here:

| Path                  | Contents                                                    |
| --------------------- | ----------------------------------------------------------- |
| `opencode-agents/`    | Formwork primary agent and four hidden role prompts       |
| `opencode-skill/`     | the `formwork` skill prompt for OpenCode                   |
| `opencode-plugin/`    | OpenCode V2 task and submission tools backed by the CLI     |

OpenCode is the only harness target, and there is exactly one set of prompts.
Do not add a parallel set for another harness: every prompt fix would have to
land in each copy, and the copies drift. `--install` accepts only `schema`
and `opencode`.

These files are embedded into the binary at compile time via `include_str!` in
`src/main.rs`, and written out verbatim by `formwork --install=opencode`.

Therefore:

1. **Edit prompts here, never in the installed copies.** Anything under
   `~/.config/opencode/` is overwritten on the next install.
2. **Never generate prompt content in code.** The Rust side copies the authored
   Markdown verbatim. If you need new wording, edit the Markdown.
3. **Rebuild after editing.** `include_str!` registers a compile-time file
   dependency, so `cargo build` picks up Markdown changes on its own — but the
   installed copies only update when you re-run `formwork --install=opencode`.
4. **Adding a role** means: create `opencode-agents/<name>.md` and add it to
   `OPENCODE_SUBAGENTS` in `src/main.rs`. Role models are not defaults in
   Rust. `formwork setup` writes `agents.<role>.model` from the live catalog.

## Host-tool behaviour is worth verifying, not assuming

Prompt-level changes often depend on which tools a host actually exposes to a
subagent, and that is not always what the documentation implies. Two findings
already baked into this codebase:

- OpenCode denies `question`, `webfetch`, and `websearch` to Markdown-defined
  agents by default; each must be re-allowed in the agent's frontmatter
  `permission` block.
- `submit_plan` is registered as a primary-agent-only tool and is unavailable to
  subagents, which is why the plan-approval gate lives with the coordinator.

If you change something in this area, **verify it in the interactive TUI** using
a subagent plus a no-permission control agent. `opencode run` is not a valid
proxy: it does not register interactive tools at all, so everything looks denied
there regardless of configuration.

## Commit and pull request conventions

Commits follow **Conventional Commits**, as the existing history shows:

```
fix(opencode): unblock the story and design phases
feat(design): require four global anchors per slice
test(design-lint): fuzz + adversarial hardening
docs(workflow): clarify milestone execution approval
```

Common scopes: `opencode`, `design`, `design-lint`, `prompts`, `install`,
`pipeline`, `tests`, `make`, `deps`.

For the body, explain **why** rather than restating the diff. If the change fixes
non-obvious behaviour, name the mechanism — a future reader needs to know what
made the old code wrong, not just that it changed.

Before opening a pull request:

- `make test` passes.
- The change is focused. Unrelated fixes belong in their own commits.
- New behaviour has a test. Prompt/instruction contracts count: see
  `test_story_instruction_carries_staged_interview_and_id_families` for the
  pattern used to pin wording that nothing parses at runtime.
- No secrets, credentials, or absolute paths from your machine.
- Prompt changes are made in `./opencode-agents/` or `./opencode-skill/` and
  nowhere else. OpenCode is the only harness target and there is exactly one set
  of prompts — never add a parallel set, because every later fix then has to land
  in each copy.

## Reporting bugs

Open an issue with the `formwork` version (`formwork --version`), your OS, the
host coding agent and model, the phase it happened in, and the exact command
plus output. If a phase stalled, the emitted task-block JSON is the single most
useful thing you can include.

Security issues go to [SECURITY.md](SECURITY.md), not the issue tracker.

## Questions

Open an issue, or email John Anthony Mariquit <john@mariquit.com>.

# Formwork workflow

The binary owns state, task identity, fingerprints, execution and acceptance. The Formwork primary agent coordinates four hidden roles: Storyteller, Designer, Reviewer and Worker. It launches the one role the current task names. Agents must never write state directly.

```text
                    ┌──────────────────────────┐
                    │ Tier 0: Product Features │
                    │   (handled by the        │
                    │   Storyteller agent      │
                    │   -- probes/interviews   │
                    │   the human for more     │
                    │   context)               │
                    └────────────┬─────────────┘
                                 │
                                 ▼
  ┌──────────────────────────────────────────────────────────────────────┐
  │ DESIGN & SHIFT-LEFT QA (The Checks & Balances Loop)                  │
  │                                                                      │
  │                                                                      │
  │  ┌──────────────────────┐ Interface / Spec  ┌─────────────────────┐  │
  │  │ Tier 1: Architecture ├──────────────────►│ Tier 2: Red Team QA │  │
  │  │   (handled by the    │                   │   (handled by the   │  │
  │  │   Designer agent)    │                   │   Reviewer agent)   │  │
  │  └────────────▲─────────┘                   └───────────┬─────────┘  │
  │               │                                         │            │
  │               └──────── Critique / Edge Cases ──────────┘            │
  │                        (Max 1 Refinement Loop)                       │
  │                                                                      │
  └─────────────────────────────┬────────────────────────────────────────┘
                                │ Approved Spec & Test Suite
                                ▼
                      ┌─────────────────────┐
                      │ Tier 3: Code Engine │
                      │   (handled by the   │
                      │   Worker agent      │
                      └─────────────────────┘
```

Tier 0 is `story`. Tier 1 is `design` and `refine`. Tier 2 is `review`, `rereview`, and `tests`. The arrow labeled Approved Spec & Test Suite is that test stage, then `awaiting_approval`. Tier 3 is `implement`. Max 1 Refinement Loop is the real cap: a second rejection blocks.

There is no postimplementation LLM acceptance verdict. After tests freeze, the user authorizes the milestone, including the MVP. Design approval does not authorize execution. The binary then runs build, unit, and integration checks. Failure returns to the Worker. Three failures block.

## Checks and balances

The Designer does not audit its own spec. A fresh Reviewer audits the written artifacts. The first rejection allows one Designer refinement. A second rejection blocks. Up to three findings are allowed, not required.

After approval, a fresh Reviewer authors executable tests from the final spec. A spec that cannot support concrete checks does not advance. Tests, fixtures, and harness inputs freeze before Worker begins.

The binary compiles `*.schema.json` definitions and runs the project validators before implementation. Contracts may be JSON Schema, OpenAPI, or compiler-checked interfaces. No language or transport is mandatory. Passing configured checks is not proof of untested behavior.

## States

| Stage | Agent or actor | Successful result |
| --- | --- | --- |
| `story` | Storyteller and user | Approved PRD with explicit milestones |
| `design` | Designer and user | SPEC, shared contracts and work plan validate |
| `review` | Fresh Reviewer | Approve to tests, or reject to one refinement |
| `refine` | Designer and user | Revised design validates |
| `rereview` | Fresh Reviewer | Approve to tests, or unresolved blockers stop |
| `tests` | Fresh Reviewer | Concrete final-spec tests, coverage and readiness validate |
| `awaiting_approval` | User through primary coordinator | Explicit current-milestone execution authorization |
| `implement` | Worker | Binary-run checks pass, or evidence returns for correction |
| `complete` | Binary | All approved milestones passed configured checks |
| `blocked` | User resolution | No automatic advancement |

Delivery decisions use the stage, issued task identity and revision bindings. `task`, `resume` and `next` return the current task or gate; they never skip a gate. Exactly one task ID is outstanding at a time. To submit manually, run `formwork submit payload.json` with the current `task_id` and `task_status` (`"complete"` or `"blocked"`). An audit verdict also includes `design_revision`, `verdict`, and `findings`. Check the JSON `status`: stale tasks and reviews are rejected, and a rejection can return exit code 0.

In OpenCode, the primary agent and delivery roles call `formwork_task` with no arguments. The plugin uses the calling session's workspace and, for a role, requests only that role's current task. The role calls `formwork_submit` once per completed submission; the primary agent fetches the result without replaying the payload. A plan proposal awaiting user approval is a handoff, not a blocked submission. The plugin forwards structured arguments to the same Rust engine. It does not authorize milestones or write workflow state itself.

## Workspace and artifacts

Formwork stores project artifacts in `.formwork/` and installs protocol schemas in `~/.formwork/`. The CLI recognizes `FORMWORK_WORKSPACE`, `FORMWORK_TARGET`, and `FORMWORK_DESIGN_LINT`.

| Artifact | Owner | Purpose |
| --- | --- | --- |
| `.formwork/prd.json` | Storyteller | Product intent, FEAT/US/AC/NG/UN IDs, and approved milestone boundaries |
| `SPEC.md` | Designer | Canonical behavior, architecture, shared modules, and failure cases |
| `.formwork/plan.json` | Designer | Ordered milestone work, feature references, reused modules, contract paths, and validator commands |
| Project-native contracts | Designer | Machine-readable interfaces appropriate to the project |
| `.formwork/tests.json` and test files | Reviewer | Executable tests, criterion coverage, harness readiness, and milestone checks |
| `.formwork/state.json` | Binary only | Task bindings, approval metadata, and evidence pointers |
| `.formwork/verification-*.json` | Binary only | Check outcomes bound to source, environment, test revision, and task identity |

## Product scope and design

The Storyteller interviews in stages, resolves product fog and obtains primary-session plan approval before writing `.formwork/prd.json`. Milestones use safe unique IDs; the first is MVP, followed by later milestones and then stretch goals. Every feature belongs to exactly one milestone. Stories and criteria inherit that scope. MVP means a usable outcome with required failure handling and authorization, not a happy-path demonstration.

Designer inspects existing code, repository constraints and product intent. Define shared boundaries once. Write `SPEC.md`, canonical machine-readable contracts and `.formwork/plan.json`. The plan preserves exact milestone order and feature ownership. Each milestone describes cohesive `work`, affected/reused `modules` and optional earlier-milestone `depends_on` references. The binary checks structure and references; it does not prove that prose modules are optimal.

Contracts may be JSON Schema, OpenAPI or native interfaces. No HTTP or TypeScript requirement exists. `contracts` lists canonical file paths and `validators` lists project-specific executable validation. JSON Schema files ending in `.schema.json` are compiled by the binary as well. UI work retains shared design guidance, proportional mockups and craft checks. Craft infrastructure errors are disclosed rather than treated as design defects; blocking measured findings reject.

Work plans do not prescribe a UI/API/database split. Define shared modules and contracts once and reuse them. Supply styling preferences separately if needed; Formwork does not install a global styling reference. Missing browser support is reported as degraded static checking, not browser validation.

## Bounded adversarial review

A fresh Reviewer audits actual artifacts for consequential failures. Up to three actionable findings are allowed, not required. Rejecting requires issue/remediation findings; approval requires none unresolved. The first rejection permits one Designer refinement. A second rejection blocks. There is no deferred-scope approval or automatic acceptance of unresolved defects.

Once approved, a fresh Reviewer authors executable tests from the final spec before Worker implementation. `.formwork/tests.json` contains protected `files`, criterion `coverage`, harness `readiness` commands and milestone `checks`. Checks have unique IDs, milestone IDs, a `kind` of unit/integration/build, executable `program`, string `args`, `non_interactive: true` and optional `timeout_seconds` from 1 to 600 (default 120). Every milestone requires all three kinds. Coverage must resolve every criterion to its own milestone's checks. Expected values must be independent of the candidate.

Readiness checks demonstrate discovery/collection and harness viability without requiring unimplemented behavior to pass. Semantic adequacy remains the Reviewer's responsibility; an arbitrary command's zero exit status cannot itself establish meaningful coverage. Unavailable tools, ambiguous expected behavior and broken collection must be reported as blockers, not waived.

## Mechanical gates

After tests freeze, the binary pauses at `awaiting_approval`, including for MVP. The primary coordinator shows the proposed milestone and asks the user before invoking `approve-milestone <id> --design-revision <revision>`. Approval binds to both the current design and test revision. Roadmap/design approval never authorizes execution.

Worker implements only that milestone, reusing shared code. Frozen contracts, tests, fixtures, expected values and declared harness inputs cannot be modified. A defect in an approved oracle requires user resolution and explicit reopening, never silent weakening.

Before implementation, the binary fingerprints the spec, contracts, test manifest, tests, fixtures, and declared harness inputs, and records executable identities. On submission it verifies those inputs and identities, fingerprints candidate source and the execution environment, and runs configured checks for the current and earlier milestones. Commands use structured arguments, run noninteractively with timeouts, and write logs to `.formwork/execution/`. The binary revalidates protected inputs and source identity after execution. All configured checks must pass. Three failed attempts block for user resolution. Missing execution infrastructure is a rejection, never a pass. There is no postimplementation LLM acceptance verdict.

Passing moves to the next milestone's approval gate or completion. A later task or resume request rejects stale completion evidence if candidate source or the execution environment changed since verification.

**Limits:** Formwork runs trusted project commands without a sandbox. Passing checks do not prove untested behavior or catch every omitted harness dependency. Candidate fingerprints exclude generated and dependency directories such as `target`, `node_modules`, `dist`, `build`, and `.venv`. Declared protected inputs are hashed separately. External services and mutable dependencies require project-specific checks.

## Recovery

Formwork rejects unsupported state protocols without rewriting files or converting approvals. Use the matching binary or a separate workspace. Use `formwork-report` to inspect recorded state without changing it.

If a workspace has `.vibe/state.json` but no Formwork state, Formwork refuses to initialize there. Use the matching `vibe` binary to continue that workflow or start Formwork in a separate workspace. Do not rename `.vibe/` to `.formwork/`: artifact paths are part of verification fingerprints, so renaming does not preserve approvals or evidence. The Formwork installer does not remove old binaries or OpenCode entries, and installing it updates the four shared role prompts.

Obtain user consent before running `formwork rollback`. It reopens design and invalidates test revisions and milestone execution approvals while retaining artifacts and history. Do not edit state to bypass a gate. State holds metadata; artifact and evidence bodies live in separate workspace files.

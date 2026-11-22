# Security Policy

**Formwork** (`formwork`)

## Reporting a vulnerability

Report security issues privately to:

**John Anthony Mariquit — <john@mariquit.com>**

Use the subject line `SECURITY: formwork — <short summary>`.

Please do **not** open a public issue, merge request, or discussion thread for a
suspected vulnerability. Public reports are handled the same way as any other
bug, which means the details are visible before a fix exists.

If you want to encrypt your report, ask for a key in a first message containing
no details.

### What to include

- The `formwork` version (`formwork --version`) and your OS.
- The host coding agent and model, if relevant.
- What an attacker can achieve, and what they need in order to achieve it.
- Reproduction steps — a minimal project workspace or command sequence.
- Any proof-of-concept artifact, attached rather than pasted if large.

### What to expect

This project is maintained by one person, so response times are best-effort
rather than contractual:

| Stage                       | Target                   |
| --------------------------- | ------------------------ |
| Acknowledgement of report   | within 5 working days    |
| Initial assessment          | within 14 days           |
| Fix or documented mitigation| depends on severity      |

I will tell you whether the report is accepted, and credit you in the release
notes and commit message unless you ask me not to. Please give a reasonable
window for a fix before disclosing publicly; if you plan to disclose on a fixed
date, say so in your first message.

## Supported versions

The project is pre-1.0 and moves fast. Only the **latest commit on `main`**
receives security fixes. There are no backports to earlier tags.

## Scope

`formwork` is a local CLI orchestrator. It has no server, no network listener,
and no user accounts. It reads and writes files in a project workspace, runs
commands the design phase declares, shells out to coding agents, and starts a
headless browser for `design-lint`. The interesting attack surface follows from
exactly that.

### In scope

- **Path traversal or workspace escape** — writing outside the project
  workspace, or reading files a phase should not reach. Slice slugs are
  validated for this reason; a bypass is in scope.
- **Command injection via untrusted artifacts** — the build phase executes the
  `test_command` the WORKER reports on its submission payload. A crafted
  `.formwork/slices.json`, artifact path, or submission payload that achieves
  execution beyond that reported command is in scope.
- **Design-lint browser handling** — anything in the CDP transport or `ui.html`
  handling that lets a mockup escape the headless browser or reach the host.
- **Integrity bypass of a gate** — defeating the design fingerprint, the review
  verdict binding, or the per-slice test gate so unreviewed or untested work is
  accepted as approved. These are correctness controls and treated as security
  boundaries.
- **Secret leakage** — `formwork` writing credentials, tokens, or environment
  contents into `.formwork/state.json`, artifacts, checkpoints, or terminal
  output.
- **Config-editing damage** — the installer performs surgical edits on the host
  agent's config file. Corruption, or injection of content that changes the
  host's permission or plugin behaviour beyond what is documented, is in scope.

### Out of scope

- **What a coding agent chooses to do.** `formwork` orchestrates third-party
  agents and models. Their behaviour, prompt-injection susceptibility, and
  output belong to those vendors. A malicious model producing malicious code is
  a property of running agents on your machine, not a `formwork` vulnerability.
- **`test_command` executing as designed.** The build gate runs the command the
  worker reports at submit — that is its documented purpose. Note where this
  command comes from: the designer does NOT author it (a command written before
  the stack exists is a guess), so approving a design does not tell you what will
  run. The binary constrains it — it must reference a `tests/` file the same
  submission declares as an artifact, and it runs in the workspace under a
  timeout — but the command text itself originates from the coding agent during
  the build loop, after your approval. Run Formwork on a workspace whose agents
  you are willing to let execute code, and treat the build loop the way you would
  `make test` in an untrusted checkout.
- **Vulnerabilities in dependencies**, unless `formwork` uses them in a way that
  creates the exposure. Report those upstream; tell me if a version bump is
  needed.
- **Running with a deliberately hostile config**, e.g. an OpenCode provider
  pointed at an attacker-controlled endpoint you configured yourself.
- Missing hardening with no demonstrated impact, or scanner output without a
  working reproduction.

## Notes for users

- **`.formwork/state.json` holds metadata, not artifact bodies** — task state
  and an artifact registry of paths, descriptions, and statuses. It is still project
  data; treat it as you treat the rest of the repository.
- **Review designs before approving them.** Approval is what starts the build
  loop. It does not, however, let you preview the commands that loop will run:
  each slice's `test_command` is reported by the worker at submit time, after
  approval. What approval fixes is the design — the slices, the anchors, and the
  test goals — which the build loop then cannot alter.
- **Model endpoints are yours to trust.** `formwork setup` pins a role to a
  catalog id in OpenCode's `agents` config, and `/models` selects the session
  model a role inherits when it has no pin. Those endpoints receive prompts,
  including source and requirements. Point them only at providers you are
  willing to send that to.
- **Keep secrets out of the workspace.** Artifacts and prompt context are read
  from the project directory and forwarded to the configured models.

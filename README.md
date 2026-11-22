# Project: Formwork

The **Formwork** agent coordinates four roles: Storyteller, Designer, Reviewer, and Worker. 

```text
  ┌───┬───┬───┬───┬───┬───┬───┬───┐
  │ F │ O │ R │ M │ W │ O │ R │ K │ v0.1.0
  └───┴───┴───┴───┴───┴───┴───┴───┘
  Spec-driven agent for solo developers on OpenCode
```

In construction, formwork shapes concrete until it can support itself. In software, a reviewed specification gives the build its shape before implementation. Formwork helps you turn an idea into that spec and deliver one approved, verified milestone at a time. 

The Rust CLI owns project state and runs the checks.

## Related OpenCode projects

- [Trestle](https://github.com/mariquit/trestle): Software engineering skills for OpenCode
- [Truss](https://github.com/mariquit/truss): Plugins for OpenCode
- [Lattice](https://github.com/mariquit/lattice): Semantic codebase wiring for OpenCode
- [Span](https://github.com/mariquit/span): Contextual aggregate memory mesh for OpenCode
- [Catwalk](https://github.com/mariquit/catwalk): Terminal viewer for agent-owned TODO lists

## Installation

**Prebuilt (Linux x86-64):** Download the `.tar.gz` and matching `.sha256` from the repository's GitHub Releases page. Verify the archive, extract it, and run its installer. Replace `VERSION` with the release version:

```sh
bundle=formwork-vVERSION-linux-x86_64
sha256sum -c "$bundle.tar.gz.sha256" &&
  tar -xzf "$bundle.tar.gz" &&
  "$bundle/install.sh"
```

**From source:** With Rust 1.89 or newer, run `make install`. Plain `make` builds without installing. `make test` also requires Node.js and Python 3.

Both installers put `formwork` and `formwork-report` in `~/.local/bin/` by default and install the matching Formwork agent, four roles, skill, OpenCode V2 plugin, and schemas. They configure Plannotator without assigning role models. Use `PREFIX` or `BINDIR` to change the binary destination; the integration still installs into your home directory. The OpenCode server must be able to find `formwork` on its `PATH`.

**Coming from VIBE?** Use Formwork for new workflows. Keep the matching VIBE binary for existing workspaces; [Formwork does not migrate VIBE state](WORKFLOW.md#recovery).

## Use Formwork in OpenCode

1. Open your project in OpenCode V2 and select the **Formwork** agent.
2. Tell it what to build, or ask it to resume an existing Formwork project. The agent starts or retrieves the workflow and delegates to the current role. You do not need to submit a payload by hand.
3. Review the proposed story and design in the primary session. Authorize implementation separately when the agent asks, including for the MVP.

> [!WARNING]
> Formwork runs project-defined validators and tests without a sandbox. Use it only in workspaces you trust.

### Role models

By default, a role without a model pin uses the Formwork session model selected with `/models`. Run `formwork setup --list` to see the tool-capable models and current pins. Set a pin with `formwork setup --storyteller <id>` or the corresponding `--designer`, `--reviewer`, or `--worker` flag. Pass `inherit` instead of a model ID to remove a pin.

## Standalone CLI

From the project directory, use these commands to initialize a workflow, retrieve the current task, or inspect its status:

```sh
formwork start Build a project tracker
formwork task
formwork status
formwork-report --output status.html
```

`formwork-report` renders recorded state as read-only HTML. See [the workflow protocol](WORKFLOW.md) for artifact ownership, manual submissions, and verification gates, or [Recovery](WORKFLOW.md#recovery) for rollback and unsupported state.

## License

Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

This project is licensed under the [Apache License, Version 2.0](LICENSE).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for coding standards, DCO sign-off, and pull request guidelines.

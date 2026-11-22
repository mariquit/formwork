---
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>
name: formwork
description: Point at the Formwork primary agent, which coordinates the four hidden roles through OpenCode workflow tools.
---
# Formwork

Use the `formwork` primary agent. It coordinates Storyteller, Designer, Reviewer, and Worker using `formwork_task` and `formwork_submit`. Do not coordinate this loop from Build or Plan.

Role models are not in the task. Set one with `formwork setup --storyteller <id>`, or the same flag for `designer`, `reviewer`, and `worker`. `inherit` clears that role's pin so it uses the Formwork session model from `/models`.

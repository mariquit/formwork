// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { access, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { delimiter, join, resolve } from "node:path"
import { test } from "node:test"
import { registerTools } from "./tools.mjs"

test("workflow tools route calls to the Rust engine in the session workspace", async (t) => {
  const tempBase = await access("/tmp/opencode").then(() => "/tmp/opencode", () => tmpdir())
  const root = await mkdtemp(join(tempBase, "formwork-tools-"))
  t.after(() => rm(root, { recursive: true, force: true }))
  const workspace = join(root, "workspace with spaces")
  const override = join(root, "different workspace")
  await mkdir(workspace)
  await mkdir(override)
  const path = process.env.PATH
  const previousOverride = process.env.FORMWORK_WORKSPACE
  const previousLint = process.env.FORMWORK_DESIGN_LINT
  const previousTarget = process.env.FORMWORK_TARGET
  t.after(() => {
    process.env.PATH = path
    if (previousOverride === undefined) delete process.env.FORMWORK_WORKSPACE
    else process.env.FORMWORK_WORKSPACE = previousOverride
    if (previousLint === undefined) delete process.env.FORMWORK_DESIGN_LINT
    else process.env.FORMWORK_DESIGN_LINT = previousLint
    if (previousTarget === undefined) delete process.env.FORMWORK_TARGET
    else process.env.FORMWORK_TARGET = previousTarget
  })
  process.env.PATH = `${resolve("target/debug")}${delimiter}${path}`
  process.env.FORMWORK_WORKSPACE = override
  process.env.FORMWORK_DESIGN_LINT = "static"
  delete process.env.FORMWORK_TARGET
  const seeded = JSON.parse(execFileSync("formwork", ["start", "Counter"], {
    env: { ...process.env, FORMWORK_WORKSPACE: workspace },
  }))

  const tools = new Map()
  const calls = []
  let allow = true
  const controller = new AbortController()
  const signal = controller.signal
  await registerTools({
    tool: { transform: (callback) => callback({ add: (tool) => tools.set(tool.name, tool) }) },
    session: { get: (input, options) => {
      assert.deepEqual(input, { sessionID: "ses_test" })
      assert.equal(options.signal, signal)
      return { location: { directory: workspace } }
    } },
    permission: { assert: (input) => {
      if (!allow) throw new Error("Permission denied")
      assert.equal(input.action, "shell")
      calls.push(input.resources[0])
    } },
  })
  const context = { agent: "formwork", sessionID: "ses_test", messageID: "msg_test", id: "call_test", signal }
  const task = tools.get("formwork_task")
  const submit = tools.get("formwork_submit")
  assert.equal(task.input.additionalProperties, false)
  assert.deepEqual(submit.input.required, ["task_id", "task_status"])
  assert.deepEqual(submit.input.properties.task_status.enum, ["complete", "blocked"])

  assert.equal(JSON.parse((await task.execute({}, context)).content).task_id, seeded.task_id)
  assert.equal(JSON.parse((await task.execute({}, { ...context, agent: "storyteller" })).content).task_id, seeded.task_id)
  assert.equal(JSON.parse((await task.execute({}, { ...context, agent: "worker" })).content).reason, "wrong_task_role")
  await assert.rejects(task.execute({}, { ...context, agent: "build" }), /Formwork primary agent/)
  assert.deepEqual(calls, ["formwork task", "formwork subagent storyteller", "formwork subagent worker"])

  const payload = { task_id: "old", task_status: "blocked", findings: [{ issue: 'Quotes " and newline\n', remediation: "Inspect $(not a shell)." }] }
  assert.equal(JSON.parse((await submit.execute(payload, context)).content).reason, "stale_task")
  payload.task_id = seeded.task_id
  allow = false
  await assert.rejects(submit.execute(payload, context), /Permission denied/)
  allow = true
  const blocked = JSON.parse((await submit.execute(payload, context)).content)
  assert.equal(blocked.status, "blocked")
  assert.deepEqual(blocked.findings, payload.findings)
  await assert.rejects(readFile(join(override, ".formwork/state.json")), { code: "ENOENT" })

  await writeFile(join(workspace, ".formwork/state.json"), "broken state")
  const stateError = JSON.parse((await task.execute({}, context)).content)
  assert.equal(stateError.status, "error")
  assert.ok(stateError.instruction)

  process.env.PATH = root
  await assert.rejects(task.execute({}, context), /Could not run the Formwork engine \(ENOENT\)/)
  await writeFile(join(root, "formwork"), "#!/bin/sh\nprintf 'not JSON'\n", { mode: 0o755 })
  await assert.rejects(task.execute({}, context), /did not return a valid workflow response/)
})

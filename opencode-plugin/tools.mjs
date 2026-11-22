// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

import { execFile } from "node:child_process"
import { isAbsolute } from "node:path"

const roles = new Set(["storyteller", "designer", "reviewer", "worker"])
const text = { type: "string", minLength: 1 }
const object = (properties, required = []) => ({
  type: "object",
  properties,
  required,
  additionalProperties: false,
})
const array = (items) => ({ type: "array", items })

function runEngine(args, cwd, signal, submission) {
  const env = { ...process.env }
  // The session directory, not an inherited CLI override, owns tool calls.
  delete env.FORMWORK_WORKSPACE
  return new Promise((resolve, reject) => {
    const child = execFile("formwork", args, { cwd, env, signal, shell: false }, (error, stdout) => {
      // State errors exit nonzero but still use the same JSON protocol.
      if (error && typeof error.code !== "number") {
        reject(new Error(`Could not run the Formwork engine (${error.code ?? error.name}). Check that formwork is on the OpenCode server's PATH.`))
        return
      }
      let result
      try {
        result = JSON.parse(stdout)
        if (typeof result?.status !== "string" || (error && result.status !== "error" && result.status !== "rejected")) {
          throw new Error("Unexpected engine status")
        }
      } catch {
        reject(new Error("Formwork did not return a valid workflow response. Check engine diagnostics before retrying."))
        return
      }
      resolve({ content: JSON.stringify(result) })
    })
    // If the child exits before reading, its exit result is more informative.
    child.stdin.on("error", (error) => {
      if (error.code !== "EPIPE") reject(error)
    })
    child.stdin.end(submission === undefined ? undefined : JSON.stringify(submission))
  })
}

export async function registerTools(ctx) {
  async function execute(operation, context, submission) {
    const { agent, sessionID, signal } = context
    if (agent !== "formwork" && !roles.has(agent)) {
      throw new Error("Use the Formwork primary agent or one of its delivery roles.")
    }
    const session = await ctx.session.get({ sessionID }, { signal })
    const cwd = session.location?.directory
    if (typeof cwd !== "string" || !isAbsolute(cwd)) {
      throw new Error("Formwork requires an absolute directory for the calling session.")
    }
    const args = operation === "task" && roles.has(agent) ? ["subagent", agent] : [operation]
    // Keep the permission check for the CLI operation the tool replaces. Submit
    // may run project-defined validators or tests through the Rust engine.
    const command = ["formwork", ...args].join(" ")
    await ctx.permission.assert({
      sessionID,
      agent,
      source: { type: "tool", messageID: context.messageID, id: context.id },
      action: "shell",
      resources: [command],
      save: [command],
    })
    return runEngine(args, cwd, signal, submission)
  }

  await ctx.tool.transform((editor) => {
    editor.add({
      name: "formwork_task",
      description: "Get the current Formwork task or gate for this session's workspace. Delivery roles can only fetch their own issued task.",
      input: object({}),
      options: { codemode: false },
      execute: (_input, context) => execute("task", context),
    })
    editor.add({
      name: "formwork_submit",
      description: "Submit the issued Formwork task to the workflow engine. Reviews include the design revision, verdict, and findings. Do not submit while waiting for plan approval.",
      input: object({
        task_id: text,
        task_status: { enum: ["complete", "blocked"] },
        design_revision: text,
        verdict: { enum: ["approve", "reject"] },
        findings: array(object({ issue: text, remediation: text }, ["issue", "remediation"])),
        artifacts: array(object({ path: text, description: { type: "string" }, status: text }, ["path"])),
      }, ["task_id", "task_status"]),
      options: { codemode: false },
      execute: (input, context) => execute("submit", context, input),
    })
  })
}

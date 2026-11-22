// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

import { Plugin } from "@opencode/plugin"
import { registerTools } from "./tools.mjs"

export default Plugin.define({ id: "formwork", setup: registerTools })

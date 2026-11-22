#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu

archive=$1
name=$(basename "$archive" .tar.gz)
(cd "$(dirname "$archive")" && sha256sum -c "$name.tar.gz.sha256")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
tar -xzf "$archive" -C "$tmp"
repo=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cmp "$repo/NOTICE" "$tmp/$name/NOTICE"
cmp "$repo/LICENSE" "$tmp/$name/LICENSE"
test -s "$tmp/$name/third-party/INDEX.txt"
test -s "$tmp/$name/third-party/json5-0.4.1/LICENCE"
export HOME="$tmp/home"
mkdir "$HOME"
unset PREFIX BINDIR
"$tmp/$name/install.sh"
version=${name#formwork-}
version=${version%-linux-x86_64}
test "$("$HOME/.local/bin/formwork" --version)" = "formwork ${version#v}"
test "$("$HOME/.local/bin/formwork-report" --version)" = "formwork-report ${version#v}"
config="$HOME/.config/opencode"
for role in formwork storyteller designer reviewer worker; do
    test -s "$config/agents/$role.md"
done
test -s "$config/skills/formwork/SKILL.md"
test -s "$config/plugins/formwork/index.ts"
test -s "$config/plugins/formwork/tools.mjs"
test -s "$HOME/.formwork/schema-task.json"
test -s "$HOME/.formwork/schema-state.json"
grep -q '"formwork"' "$config/opencode.jsonc"

workspace="$tmp/workspace"
mkdir "$workspace"
(
    unset FORMWORK_TARGET
    export FORMWORK_WORKSPACE="$workspace" FORMWORK_DESIGN_LINT=static
    "$HOME/.local/bin/formwork" start 'Release smoke test' > "$tmp/task.json"
)
test -s "$workspace/.formwork/state.json"
python3 - "$tmp/task.json" "$HOME/.formwork/schema-task.json" <<'PY'
import json
from pathlib import Path
import sys

task = json.loads(Path(sys.argv[1]).read_text())
assert task["$schema"] == sys.argv[2], task
PY
"$HOME/.local/bin/formwork-report" --workspace "$workspace" > "$tmp/report.html"
grep -q 'Stage: <strong>story</strong>' "$tmp/report.html"

# An integration failure must make the installer fail, not report success.
export HOME="$tmp/broken-home"
mkdir "$HOME"
touch "$HOME/.config"
if "$tmp/$name/install.sh"; then
    printf 'The installer succeeded despite an invalid OpenCode config directory.\n' >&2
    exit 1
fi

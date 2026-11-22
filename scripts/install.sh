#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu

bundle=${1:-$(dirname "$0")}
bindir=${BINDIR:-${PREFIX:-"$HOME/.local"}/bin}
for binary in formwork formwork-report; do
    if [ ! -f "$bundle/$binary" ] || [ ! -x "$bundle/$binary" ]; then
        printf 'Missing executable: %s\n' "$bundle/$binary" >&2
        exit 1
    fi
done

install -d "$bindir"
install -m 0755 "$bundle/formwork" "$bundle/formwork-report" "$bindir/"
"$bindir/formwork" --install=opencode
printf 'Installed binaries in %s\n' "$bindir"

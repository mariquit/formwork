#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu

bin=$1
out=$2
cd "$(dirname "$0")/.."
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) platform=linux-x86_64 ;;
    *) printf 'Release packaging currently supports Linux x86-64 only.\n' >&2; exit 1 ;;
esac

cli=$("$bin/formwork" --version)
report=$("$bin/formwork-report" --version)
version=${cli#formwork }
if [ "$cli" != "formwork $version" ] || [ "$report" != "formwork-report $version" ] ||
    ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    printf 'Expected both binaries to report the same stable version; got %s and %s.\n' "$cli" "$report" >&2
    exit 1
fi
if [ -n "${RELEASE_TAG:-}" ] && [ "$RELEASE_TAG" != "v$version" ]; then
    printf 'Release tag %s does not match binary version %s.\n' "$RELEASE_TAG" "$version" >&2
    exit 1
fi

name=formwork-v$version-$platform
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name" "$out"
install -m 0755 "$bin/formwork" "$bin/formwork-report" "$stage/$name/"
install -m 0755 scripts/install.sh "$stage/$name/"
install -m 0644 LICENSE NOTICE TRADEMARK.md "$stage/$name/"
python3 scripts/collect-dependency-notices.py "$stage/$name/third-party"
tar -czf "$out/$name.tar.gz" -C "$stage" "$name"
(cd "$out" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
printf 'Package: %s/%s.tar.gz\n' "$out" "$name"

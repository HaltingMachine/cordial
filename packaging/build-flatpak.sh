#!/usr/bin/env bash
# Build Cordial as a Flatpak.
#
# Flatpak is the primary distribution target (spec §11). Building one on every
# change is worth the minute it costs: it is the only check that the runtime's
# dependencies are actually declared rather than merely present on the machine
# that happens to be building it. A Cordial that runs from `cargo build` but not
# from a Flatpak is a Cordial nobody else can install.
#
# Usage:
#     packaging/build-flatpak.sh [--install]

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$here")"
manifest="$here/org.cordial.Cordial.yml"
builddir="$root/target/flatpak"
repo="$root/target/flatpak-repo"

need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "error: $1 is not installed" >&2
        exit 1
    }
}
need flatpak
need flatpak-builder

# The manifest names these explicitly; installing them here rather than letting
# flatpak-builder fail halfway keeps the error legible.
runtime_version=$(sed -n "s/^runtime-version: *'\\(.*\\)'/\\1/p" "$manifest")
required=(
    "org.freedesktop.Platform//${runtime_version}"
    "org.freedesktop.Sdk//${runtime_version}"
    "org.freedesktop.Sdk.Extension.rust-stable//${runtime_version}"
    "org.freedesktop.Sdk.Extension.llvm20//${runtime_version}"
)
missing=()
for ref in "${required[@]}"; do
    flatpak info "$ref" >/dev/null 2>&1 || missing+=("$ref")
done

if (( ${#missing[@]} )); then
    echo "installing ${#missing[@]} missing runtime(s): ${missing[*]}"
    flatpak install --user --noninteractive --or-update flathub "${missing[@]}"
fi

# The bionic linker and libjnivm are submodules; without them the build script
# panics with a message about them, but only after flatpak-builder has copied
# the whole tree in. Checking first is faster and clearer.
if [[ ! -f "$root/third_party/mcpelauncher-linker/bionic/linker/linker.cpp" ]]; then
    echo "error: submodules are not checked out — run:" >&2
    echo "    git submodule update --init --recursive" >&2
    exit 1
fi

echo "building $(basename "$manifest") ..."
flatpak-builder \
    --force-clean \
    --user \
    --repo="$repo" \
    "$builddir" \
    "$manifest"

echo
echo "built into $repo"

if [[ "${1:-}" == "--install" ]]; then
    flatpak install --user --noninteractive --or-update --reinstall "$repo" org.cordial.Cordial
    echo "installed. run with: flatpak run org.cordial.Cordial"
else
    echo "install with: packaging/build-flatpak.sh --install"
fi

#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Sets up a fresh Ubuntu laptop to build Sanctm and run it in the KeyOS simulator.
#
#   ./setup.sh          install everything (safe to run again: finished steps are skipped)
#   ./setup.sh pack     on a laptop that has the local test tools: write sanctm-testkit.tar.gz
#                       (tools/ and docs/TESTING.md, which are not in git) to carry to the new one
#
# What it installs: curl, git, gpg, python3 (apt); Nix (Determinate Systems installer); the
# Foundation SDK 1.1.0 and its development shell (Rust toolchain, Slint, KeyOS targets); the
# Passport USB rules; Node.js 22 (through Nix). With the test kit present it also installs the
# test tools' npm packages and Playwright's Chromium.
#
# Needs sudo (apt, Nix, udev rules), internet, and about 25 GB of free disk.

set -euo pipefail

SDK_VERSION="1.1.0"
SDK_INSTALLER="https://sdk.foundation.xyz/v${SDK_VERSION}/install.sh"
NIX_INSTALLER="https://install.determinate.systems/nix"
KIT="sanctm-testkit.tar.gz"
REPO="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
SDK_ROOT="$HOME/.foundation/sdk/current"

step() { printf '\n\033[1;35m==> %s\033[0m\n' "$*"; }
ok() { printf '    \033[32m✓\033[0m %s\n' "$*"; }
note() { printf '    \033[33m!\033[0m %s\n' "$*"; }
die() { printf '\n\033[31mError:\033[0m %s\n' "$*" >&2; exit 1; }

# Runs a command inside the SDK's Nix development shell (the same toolchain as `foundation develop`).
in_dev_shell() {
    (cd "$SDK_ROOT" && nix develop --accept-flake-config --command bash -c "$1")
}

pack() {
    cd "$REPO"
    [ -f tools/package.json ] || die "tools/ is not here: run 'pack' on the laptop that has the test tools."
    local files=(tools docs/TESTING.md)
    [ -f docs/DEMO.md ] && files+=(docs/DEMO.md)
    tar --exclude='tools/node_modules' --exclude='target' --exclude='.peer-key' --exclude='*.key' -czf "$KIT" "${files[@]}"
    ok "Wrote $REPO/$KIT ($(du -h "$KIT" | cut -f1))"
    echo "    Copy it into the cloned repo folder on the new laptop (or into its home folder), then run ./setup.sh there."
}

if [ "${1:-}" = "pack" ]; then
    pack
    exit 0
fi
[ -z "${1:-}" ] || die "unknown argument '$1' (use no argument, or 'pack')"

[ "$(id -u)" -ne 0 ] || die "run this as your normal user, not with sudo (it asks for sudo itself)."
command -v apt-get >/dev/null || die "this script is for Ubuntu (apt-get not found)."
case "$(uname -m)" in
    x86_64 | aarch64) ;;
    *) die "unsupported CPU: $(uname -m)" ;;
esac

free_gb=$(df -Pk "$HOME" | awk 'NR == 2 { print int($4 / 1048576) }')
if [ "$free_gb" -lt 25 ]; then
    note "Only ${free_gb} GB free in $HOME. The SDK, its Nix shell and the first build need about 25 GB."
fi

step "Ubuntu packages"
sudo apt-get update -qq
sudo apt-get install -y -qq curl git gnupg ca-certificates xz-utils python3 >/dev/null
ok "curl, git, gpg, python3"

step "Nix"
if ! command -v nix >/dev/null && [ -e /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]; then
    # shellcheck disable=SC1091
    . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
fi
if command -v nix >/dev/null; then
    ok "already installed ($(nix --version))"
else
    curl --proto '=https' --tlsv1.2 -sSf -L "$NIX_INSTALLER" | sh -s -- install --no-confirm
    # shellcheck disable=SC1091
    . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
    ok "installed ($(nix --version))"
fi

step "Foundation SDK $SDK_VERSION"
if [ -f "$SDK_ROOT/manifest.toml" ] && grep -q "^version = \"$SDK_VERSION\"" "$SDK_ROOT/manifest.toml"; then
    ok "already installed in $SDK_ROOT"
else
    # The installer checks the release's GPG signature and adds ~/.foundation/sdk/bin to ~/.bashrc.
    curl -fsSL "$SDK_INSTALLER" | sh
    ok "installed in $SDK_ROOT"
fi
export PATH="$HOME/.foundation/sdk/bin:$PATH"

step "Passport USB access (only used with a real Passport Prime)"
if [ -f /etc/udev/rules.d/99-passport.rules ]; then
    ok "udev rules already installed"
else
    sudo cp "$SDK_ROOT/share/99-passport.rules" /etc/udev/rules.d/
    sudo udevadm control --reload && sudo udevadm trigger
    ok "udev rules installed"
fi

step "Node.js (for the local relay, the test peer and the end-to-end test)"
node_major=0
command -v node >/dev/null && node_major=$(node -p 'process.versions.node.split(".")[0]')
if [ "$node_major" -ge 20 ]; then
    ok "already installed ($(node --version))"
else
    nix profile add nixpkgs#nodejs_22 2>/dev/null || nix profile install nixpkgs#nodejs_22
    export PATH="$HOME/.nix-profile/bin:$PATH"
    ok "installed ($(node --version))"
fi

step "Test kit (tools/ and docs/TESTING.md, which are not in git)"
if [ ! -f "$REPO/tools/package.json" ]; then
    for kit in "$REPO/$KIT" "$HOME/$KIT" "$HOME/Downloads/$KIT"; do
        if [ -f "$kit" ]; then
            tar -xzf "$kit" -C "$REPO"
            ok "unpacked $kit"
            break
        fi
    done
fi
have_kit=0
if [ -f "$REPO/tools/package.json" ]; then
    have_kit=1
    (cd "$REPO/tools" && npm ci --no-audit --no-fund --loglevel=error)
    ok "npm packages"
    # Chromium for the end-to-end test; --with-deps installs its system libraries through apt.
    (cd "$REPO/tools" && npx --yes playwright install --with-deps chromium)
    ok "Playwright Chromium"
else
    note "No $KIT found. The app and the companion page work without it; the step-by-step test"
    note "(local relay, test peer) needs it: run './setup.sh pack' on the other laptop, copy"
    note "$KIT into $REPO, and run ./setup.sh again."
fi

step "SDK development shell (first time: downloads the toolchain, several GB)"
in_dev_shell "rustc --version && cargo --version"
ok "toolchain ready"

step "Fetch the app's crates and run the protocol tests"
in_dev_shell "cd '$REPO' && cargo fetch --quiet && cd core && cargo test --release --quiet"
ok "core tests pass"
if [ "$have_kit" = 1 ] && [ -f "$REPO/tools/headless-device/Cargo.toml" ]; then
    in_dev_shell "cd '$REPO/tools/headless-device' && cargo build --quiet"
    ok "headless device (for the end-to-end test) built"
fi

step "Check"
in_dev_shell "cd '$REPO' && foundation doctor" || note "foundation doctor reported a problem above."

cat <<EOF

$(printf '\033[1;32mDone.\033[0m') Open a NEW terminal (so PATH picks up Nix and the SDK), then:

  cd $REPO
  foundation develop        # a shell with the SDK toolchain
  foundation sim            # first build 5-15 min, then a Passport window opens with Sanctm

In a second terminal, the companion page (the "phone"):

  cd $REPO/companion && python3 -m http.server 8000
  # open http://localhost:8000 in Firefox
EOF
if [ "$have_kit" = 1 ]; then
    echo
    echo "For the full step-by-step test, follow $REPO/docs/TESTING.md."
fi
if [ ! -d "$HOME/.foundation/signing" ]; then
    cat <<EOF

Not set up: a signing identity, which only a real Passport Prime needs (not the simulator).
Copy ~/.foundation/signing/ from the laptop that already has one, or create a new one with
'foundation cert gen passport-prime-dev' (a new one must be allowed on the Passport again).
EOF
fi

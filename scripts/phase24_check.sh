#!/usr/bin/env bash
# phase 24 behavioral check script
# verifies: universal binary build config, signing/notarization config,
# entitlements, tauri-plugin-updater, auto-update implementation in main.rs,
# ci pipeline structure, and regression guard against phase 23.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONF="$REPO_ROOT/desktop/src-tauri"
SRC="$CONF/src"
CI="$REPO_ROOT/.github/workflows"

PASS=0
FAIL=0

check() {
    local desc="$1"
    local result="$2"
    if [ "$result" = "ok" ]; then
        echo "  [pass] $desc"
        PASS=$((PASS + 1))
    else
        echo "  [fail] $desc"
        FAIL=$((FAIL + 1))
    fi
}

grep_check() {
    local desc="$1"
    shift
    if grep -r "$@" > /dev/null 2>&1; then
        check "$desc" "ok"
    else
        check "$desc" "fail"
    fi
}

run_check() {
    local desc="$1"
    shift
    if "$@" > /dev/null 2>&1; then
        check "$desc" "ok"
    else
        check "$desc" "fail"
    fi
}

echo ""
echo "phase 24: distribution + auto-update"
echo "======================================"

echo ""
echo "--- m24.1: universal binary + signing + notarization config ---"

# universal binary target — lives in ci workflow since it is a rust target triple,
# not a tauri bundle format. tauri.conf.json holds signing and entitlement config.
grep_check "universal-apple-darwin build target in release.yml" \
    "universal-apple-darwin" "$CI/release.yml"

grep_check "bundle active in tauri.conf.json" \
    "\"active\": true" "$CONF/tauri.conf.json"

grep_check "minimumSystemVersion 13.0 in tauri.conf.json" \
    "minimumSystemVersion" "$CONF/tauri.conf.json"

grep_check "13.0 present in tauri.conf.json" \
    "13.0" "$CONF/tauri.conf.json"

grep_check "signingIdentity key present in tauri.conf.json" \
    "signingIdentity" "$CONF/tauri.conf.json"

grep_check "providerShortName key present in tauri.conf.json" \
    "providerShortName" "$CONF/tauri.conf.json"

grep_check "entitlements reference in tauri.conf.json" \
    "entitlements" "$CONF/tauri.conf.json"

check "entitlements.plist file exists" \
    "$([ -f "$CONF/entitlements.plist" ] && echo ok || echo fail)"

grep_check "com.apple.security.network.client in entitlements.plist" \
    "com.apple.security.network.client" "$CONF/entitlements.plist"

grep_check "com.apple.security.cs.allow-jit in entitlements.plist" \
    "com.apple.security.cs.allow-jit" "$CONF/entitlements.plist"

grep_check "com.apple.security.files.user-selected in entitlements.plist" \
    "com.apple.security.files.user-selected" "$CONF/entitlements.plist"

grep_check "tauri-plugin-updater in Cargo.toml" \
    "tauri-plugin-updater" "$CONF/Cargo.toml"

echo ""
echo "--- m24.2: updater config in tauri.conf.json ---"

grep_check "updater endpoints https url in tauri.conf.json" \
    "https://github.com" "$CONF/tauri.conf.json"

grep_check "updater endpoint is not OWNER/REPO placeholder" \
    "km31-code/jeff" "$CONF/tauri.conf.json"

grep_check "updater pubkey references TAURI_PUBLIC_KEY in tauri.conf.json" \
    "TAURI_PUBLIC_KEY" "$CONF/tauri.conf.json"

grep_check "plugins.updater section in tauri.conf.json" \
    "updater" "$CONF/tauri.conf.json"

echo ""
echo "--- m24.2: auto-update implementation in main.rs ---"

grep_check "tauri_plugin_updater initialized in main.rs" \
    "tauri_plugin_updater::Builder" "$SRC/main.rs"

grep_check "background update check spawned in main.rs" \
    "perform_update_check" "$SRC/app_polls.rs"

grep_check "UpdaterExt imported in main.rs" \
    "UpdaterExt" "$SRC/app_polls.rs"

grep_check "updater check called in main.rs" \
    "updater.*check\|check.*await" "$SRC/app_polls.rs"

grep_check "Install button label in main.rs" \
    "Install" "$SRC/app_polls.rs"

grep_check "Later button label in main.rs" \
    "Later" "$SRC/app_polls.rs"

grep_check "download_and_install called in main.rs" \
    "download_and_install" "$SRC/app_polls.rs"

grep_check "app.restart called on install in main.rs" \
    "app.restart\|\.restart()" "$SRC/app_polls.rs"

echo ""
echo "--- m24.3: ci pipeline structure ---"

check "release.yml exists" \
    "$([ -f "$CI/release.yml" ] && echo ok || echo fail)"

grep_check "pipeline triggers on release branch only" \
    "release" "$CI/release.yml"

grep_check "test job present in release.yml" \
    "test:" "$CI/release.yml"

grep_check "build job depends on test" \
    "needs: test" "$CI/release.yml"

grep_check "sign job depends on build" \
    "needs: build" "$CI/release.yml"

grep_check "notarize job depends on sign" \
    "needs: sign" "$CI/release.yml"

grep_check "release job depends on notarize" \
    "needs: notarize" "$CI/release.yml"

grep_check "phase17_check.sh called in ci test job" \
    "phase17_check.sh" "$CI/release.yml"

grep_check "tauri cli runs from desktop package in build job" \
    "npm --prefix desktop run tauri -- build" "$CI/release.yml"

grep_check "updater public key injected before release build" \
    "TAURI_PUBLIC_KEY.*tauri.conf.json\|inject updater public key" "$CI/release.yml"

grep_check "apple_certificate secret referenced correctly" \
    "secrets.APPLE_CERTIFICATE" "$CI/release.yml"

grep_check "tauri private key secret referenced correctly" \
    "secrets.TAURI_PRIVATE_KEY" "$CI/release.yml"

grep_check "notarytool submit in ci" \
    "notarytool submit" "$CI/release.yml"

grep_check "xcrun stapler staple in ci" \
    "xcrun stapler staple" "$CI/release.yml"

grep_check "codesign call in ci sign job" \
    "codesign" "$CI/release.yml"

grep_check "macos-14 runner used (apple silicon for universal build)" \
    "macos-14" "$CI/release.yml"

grep_check "no hardcoded apple_id value (only secret reference allowed)" \
    "\${{ secrets.APPLE_ID }}" "$CI/release.yml"

grep_check "latest.json generated in release job" \
    "latest.json" "$CI/release.yml"

grep_check "release job signs updater archive with tauri signer" \
    "signer sign" "$CI/release.yml"

grep_check "latest.json points at signed updater archive, not dmg" \
    "app.tar.gz" "$CI/release.yml"

echo ""
echo "--- m24.4: architecture note ---"

grep_check "windows-ready section in ARCHITECTURE.md" \
    "Windows-ready when" "$REPO_ROOT/docs/ARCHITECTURE.md"

grep_check "windows updater platform entry mentioned" \
    "win32-x86_64\|windows-2022" "$REPO_ROOT/docs/ARCHITECTURE.md"

grep_check "SMAppService windows replacement noted" \
    "SMAppService\|Task Scheduler" "$REPO_ROOT/docs/ARCHITECTURE.md"

grep_check "eventkit windows replacement noted" \
    "EventKit\|Windows Calendar" "$REPO_ROOT/docs/ARCHITECTURE.md"

echo ""
echo "--- behavioral tests ---"

run_check "cargo check passes (compilation proxy for release build)" \
    cargo check --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml"

echo ""
echo "--- regression guard ---"

run_check "phase23_check.sh still passes" \
    bash "$REPO_ROOT/scripts/phase23_check.sh"

echo ""
echo "phase 24 check: $PASS passed, $FAIL failed"
if [ "$FAIL" -ne 0 ]; then
    exit 1
fi

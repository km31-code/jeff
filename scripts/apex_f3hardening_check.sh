#!/usr/bin/env bash
# apex f3-hardening: the companion channel's Noise static PRIVATE key belongs in
# the Keychain, next to the api keys -- not in the app_settings DB. this check
# proves the key moved behind secrets.rs, that identity() migrates a pre-hardening
# store on first read and never writes the private key back to the DB, and that the
# public key / psk / rendezvous token (routing material, not secrets) stay put.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
TAURI="$ROOT_DIR/desktop/src-tauri"
COMP="$SRC/companion.rs"
SECRETS="$SRC/secrets.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f3-hardening companion-key -> keychain check ---"

# 1. the keychain surface exists in secrets.rs, mirroring the api-key pattern.
test -f "$SECRETS" || fail "secrets.rs missing"
grep -q 'COMPANION_KEY_KEYCHAIN_ACCOUNT' "$SECRETS" || fail "companion keychain account missing"
grep -q 'trait CompanionKeyStore' "$SECRETS" || fail "CompanionKeyStore trait missing"
grep -q 'struct SystemCompanionKeyStore' "$SECRETS" || fail "SystemCompanionKeyStore impl missing"
grep -q 'fn set_companion_private_key' "$SECRETS" || fail "keychain store setter missing"
grep -q 'KeyringError::NoEntry' "$SECRETS" || fail "keychain read must treat NoEntry as absent"
pass "secrets.rs exposes a Keychain-backed CompanionKeyStore"

# 2. identity() resolves the private key through the key store, not app_settings,
#    and migrates a legacy store: keychain first, migrate the DB copy, generate.
grep -q 'fn identity_with_key_store' "$COMP" || fail "identity is not backed by a key store"
grep -q 'SystemCompanionKeyStore' "$COMP" || fail "identity() must use the system keychain store"
grep -q 'enum IdentityAction' "$COMP" || fail "pure migration decision missing"
for v in UseKeychain Migrate Generate; do
  grep -q "IdentityAction::$v" "$COMP" || fail "identity action $v missing"
done

# 3. the private key is never written to the DB, and the legacy copy is cleared.
#    the only app_settings write in identity_with_key_store must be the PUBLIC key.
IDENT_BODY="$(awk '/fn identity_with_key_store/{f=1} f{print} f&&/^}/{c++} f&&c==1{exit}' "$COMP")"
printf '%s\n' "$IDENT_BODY" | grep -q 'set_companion_private_key' \
  || fail "private key is not stored in the keychain"
printf '%s\n' "$IDENT_BODY" | grep -q 'delete_app_setting(IDENTITY_PRIV_KEY)' \
  || fail "legacy DB private key is not cleared"
if printf '%s\n' "$IDENT_BODY" | grep -q 'set_app_setting(IDENTITY_PRIV_KEY'; then
  fail "identity() must never write the private key into app_settings"
fi
printf '%s\n' "$IDENT_BODY" | grep -q 'set_app_setting(IDENTITY_PUB_KEY' \
  || fail "the public key must still be persisted to app_settings"
pass "identity() keeps the private key in the keychain and clears the DB copy"

# 4. warning-free compile.
CHECK_OUT=$(cd "$TAURI" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

# 5. behavioral proof: the migration/generation/steady-state tests exist and pass,
#    and the f3a channel tests still pass (nothing regressed).
for t in \
  f3hardening_decides_use_migrate_or_generate \
  f3hardening_migrates_db_private_key_into_the_keychain_and_clears_the_db_copy \
  f3hardening_generates_into_the_keychain_never_the_db \
  f3hardening_uses_keychain_without_rewriting_the_db_private \
  f3hardening_keychain_read_failure_surfaces_rather_than_forging_an_identity; do
  grep -q "fn $t" "$COMP" || fail "expected test $t is missing"
done
T_OUT=$(cd "$TAURI" && cargo test --lib f3 --quiet 2>&1)
echo "$T_OUT" | grep -q "test result: ok" || { echo "$T_OUT"; fail "f3 tests failed"; }
echo "$T_OUT" | grep -q "FAILED" && { echo "$T_OUT"; fail "f3 tests failed"; }
pass "f3-hardening migration tests pass; f3a/f3b channel tests still green"

echo "--- apex f3-hardening check passed ---"

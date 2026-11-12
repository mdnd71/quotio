# Sprint log

## 2026-11-13 — Antigravity work-profile quotas

- Confirmed all eight work profiles return distinct quota windows through their native `agy /usage` command while the legacy Google Cloud Code quota endpoint returns `SUBSCRIPTION_REQUIRED`.
- Added explicit registered-profile CLI reads to borrowed Antigravity auth files, with Keychain email checks before and after each quota command, bounded subprocess output, and no cross-account fallback.
- Added OpenCode Go access-only auth files so the locally built helper can use the existing Go account without accessing the official CLI's shared credential vault.
- Verified a release build against all nine real Antigravity profiles: all return quota windows without an account issue. All 555 unit tests pass (two ignored), as do the preceding integration suites. The two `usage_cache_cli` tests fail unchanged on the upstream baseline; both failures were reproduced from a clean HEAD archive.
- Installed the patched helper in the existing Quotio Beta bundle as a local ad-hoc development build; automatic upstream updates are off to preserve the patch. The original signed beta is retained outside Applications as a rollback copy.
- macOS Keychain approval for the changed app signing identity is required before the GUI can finish startup; SecurityAgent cannot be controlled by the computer-use tool.

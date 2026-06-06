//! Web UI authentication: bcrypt password hashing and credential verification.
//!
//! Credentials live in `config.toml` under `[auth]` as htpasswd-style
//! `"username:bcrypt-hash"` entries (see [`crate::config::AuthConfig`]). This
//! module owns the trusted-side primitives: hashing a new password
//! ([`hash_password`]), parsing the entries ([`ParsedUser`]), verifying a
//! login attempt ([`verify_credentials`]), and deciding whether the login gate
//! is active ([`auth_enabled`]). The web crate calls into these; it never
//! touches bcrypt directly.

use bcrypt::DEFAULT_COST;

use crate::config::Config;

/// A fixed, valid bcrypt hash used to equalize response timing when a login is
/// attempted for an unknown username.
///
/// Without this, an unknown user would skip the (deliberately slow) bcrypt
/// verify entirely, and an attacker could distinguish "user exists, wrong
/// password" (slow) from "user does not exist" (fast) and enumerate valid
/// usernames. By always running one bcrypt verify against this dummy on the
/// unknown-user path, both branches do the same expensive work.
///
/// This is a real cost-12 (`DEFAULT_COST`) hash generated once, offline, of a
/// throwaway string. It is a literal (not generated at runtime) so the very
/// first unknown-user request does not pay a one-off generation cost that would
/// itself leak timing. The plaintext it hashes is irrelevant — we only ever
/// call `verify` against it to burn the same CPU as a real verify, and discard
/// the result.
const DUMMY_HASH: &str = "$2b$12$P6LY6t.tj2V/XGO3FxFCkO6LQKVpEedM1YeKqEOL8vuUkT4PdlsfC";

/// The maximum password byte length bcrypt actually hashes. bcrypt silently
/// truncates anything past 72 bytes, so two distinct passwords that share a
/// 72-byte prefix would verify against the same hash. Rather than hand the user
/// that surprise, the TUI add-user flow rejects longer passwords up front (see
/// [`password_within_bcrypt_limit`]).
pub const MAX_BCRYPT_PASSWORD_BYTES: usize = 72;

/// Hash a plaintext password with bcrypt at [`DEFAULT_COST`]. Returns the
/// htpasswd-compatible hash string suitable for storing in an `[auth]` entry.
pub fn hash_password(plain: &str) -> anyhow::Result<String> {
    let hash = bcrypt::hash(plain, DEFAULT_COST)?;
    Ok(hash)
}

/// Whether a password is short enough that bcrypt will hash it in full.
///
/// bcrypt only considers the first [`MAX_BCRYPT_PASSWORD_BYTES`] bytes of the
/// input; anything beyond is silently ignored. Callers should reject (not
/// truncate) over-long passwords so the user is never under the false
/// impression that the dropped suffix protects their account.
pub fn password_within_bcrypt_limit(password: &str) -> bool {
    password.len() <= MAX_BCRYPT_PASSWORD_BYTES
}

/// Validate a proposed `[auth]` username.
///
/// Usernames are stored htpasswd-style as the part before the FIRST `':'` in a
/// `"username:hash"` entry, so a username may not be empty and may not contain
/// `':'` (that is the field separator — see [`parse_user`]). dux also rejects
/// leading/trailing whitespace and any control or whitespace characters inside
/// the name, because such usernames are confusing to type at the login form and
/// easy to mistype in config. Usernames are case-sensitive (slice A1): `Alice`
/// and `alice` are different accounts. Returns `Ok(())` when valid, or an
/// explanatory message suitable for a status line when not.
pub fn validate_username(username: &str) -> Result<(), String> {
    if username.is_empty() {
        return Err("Username cannot be empty.".to_string());
    }
    if username.contains(':') {
        return Err(
            "Username cannot contain ':' — it separates the username from the password hash in config.".to_string(),
        );
    }
    if username != username.trim() {
        return Err("Username cannot start or end with whitespace.".to_string());
    }
    if username
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("Username cannot contain spaces or control characters.".to_string());
    }
    Ok(())
}

/// A single parsed `[auth]` entry: a username and its bcrypt hash, borrowed from
/// the original `"username:hash"` string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedUser<'a> {
    pub username: &'a str,
    pub hash: &'a str,
}

/// Parse a single `"username:hash"` entry.
///
/// The username is everything before the FIRST `':'`; the hash is everything
/// after. bcrypt hashes themselves contain `'$'` separators (and never `':'`),
/// so splitting on the first colon is unambiguous. An empty username or empty
/// hash makes the entry invalid (returns `None`); the caller logs and skips it.
fn parse_user(entry: &str) -> Option<ParsedUser<'_>> {
    let (username, hash) = entry.split_once(':')?;
    if username.is_empty() || hash.is_empty() {
        return None;
    }
    Some(ParsedUser { username, hash })
}

/// Parse all valid `[auth]` entries, skipping (and logging) malformed ones.
///
/// A malformed entry must NOT brick the whole config — explicit failure applies
/// to operator-facing actions, but a single typo in one user line should not
/// lock everyone out. Each skipped entry is logged once at warn level so the
/// operator can see (in dux.log) that an entry was ignored. Usernames are not
/// logged on the malformed path (we cannot reliably extract one) and hashes are
/// never logged.
pub fn parse_users(users: &[String]) -> Vec<ParsedUser<'_>> {
    let mut parsed = Vec::with_capacity(users.len());
    for entry in users {
        match parse_user(entry) {
            Some(user) => parsed.push(user),
            None => {
                crate::logger::warn(
                    "ignoring malformed [auth] users entry: expected \"username:bcrypt-hash\" \
                     with a non-empty username and hash",
                );
            }
        }
    }
    parsed
}

/// Verify a login attempt against the configured `[auth]` users.
///
/// Returns `true` only when `username` matches a configured user AND `password`
/// verifies against that user's bcrypt hash.
///
/// Timing-shape note: when the username is unknown we still run exactly one
/// bcrypt verify (against [`DUMMY_HASH`]) and discard the result, so the
/// known-user and unknown-user paths perform the same expensive work and do not
/// leak which usernames exist. We deliberately do NOT assert exact timing in a
/// test — bcrypt timing is environment-dependent and such a test would be
/// flaky; the mitigation is structural, not measured.
///
/// Duplicate usernames: LAST wins. When two entries share a username, the one
/// later in the list is authoritative. This matches "append to change the
/// password": the TUI's add-user command (slice A4) appends a fresh entry to
/// replace a password, and verification honors the newest one.
///
/// A malformed bcrypt hash (one bcrypt cannot parse) denies the login; the
/// error is logged once and treated as a failed verify, never a panic.
pub fn verify_credentials(users: &[String], username: &str, password: &str) -> bool {
    let parsed = parse_users(users);

    // LAST wins: scan from the end so a later duplicate entry overrides earlier
    // ones for the same username.
    let matched = parsed.iter().rev().find(|user| user.username == username);

    let Some(user) = matched else {
        // Unknown username: run one verify against the dummy hash to equalize
        // timing with the known-user path, then deny.
        let _ = bcrypt::verify(password, DUMMY_HASH);
        return false;
    };

    match bcrypt::verify(password, user.hash) {
        Ok(ok) => ok,
        Err(_) => {
            crate::logger::warn(&format!(
                "denying login for user \"{username}\": stored [auth] hash is not a valid bcrypt hash"
            ));
            false
        }
    }
}

/// Whether the web UI login gate is active.
///
/// The gate is ON when at least one VALID `[auth]` user exists AND auth is not
/// explicitly disabled (`dux server --disable-auth`). "Valid" means the entry
/// parses into a non-empty username and hash; counting only valid entries
/// prevents a config full of typo'd entries from looking like it has users
/// while leaving the gate effectively unguarded.
///
/// HAZARD for slice A2 to surface: a config that has entries but none of them
/// valid returns `false` here (gate OFF) — the operator likely intended auth to
/// be ON. The server bootstrap should WARN loudly in that
/// entries-present-but-none-valid case rather than silently serving with no
/// login. This function does not warn (it is a pure predicate); the warning
/// belongs at the call site that has the startup logging surface.
pub fn auth_enabled(config: &Config, disable_flag: bool) -> bool {
    if disable_flag {
        return false;
    }
    !parse_users(&config.auth.users).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_round_trip() {
        let hash = hash_password("correct horse battery staple").expect("hash");
        let users = vec![format!("alice:{hash}")];
        assert!(verify_credentials(
            &users,
            "alice",
            "correct horse battery staple"
        ));
    }

    #[test]
    fn hash_uses_default_cost() {
        let hash = hash_password("whatever").expect("hash");
        // bcrypt hashes encode the cost as the second `$`-delimited field, e.g.
        // `$2b$12$...`. DEFAULT_COST is 12.
        assert!(
            hash.contains(&format!("${DEFAULT_COST}$")),
            "hash should encode DEFAULT_COST: {hash}"
        );
    }

    #[test]
    fn wrong_password_is_rejected() {
        let hash = hash_password("right").expect("hash");
        let users = vec![format!("alice:{hash}")];
        assert!(!verify_credentials(&users, "alice", "wrong"));
    }

    #[test]
    fn unknown_user_is_rejected() {
        // Unknown user denies. (The structural timing mitigation runs a dummy
        // verify internally; we intentionally do not assert timing here — such a
        // test would be flaky. See verify_credentials docs.)
        let hash = hash_password("right").expect("hash");
        let users = vec![format!("alice:{hash}")];
        assert!(!verify_credentials(&users, "mallory", "right"));
    }

    #[test]
    fn empty_users_rejects_everything() {
        assert!(!verify_credentials(&[], "alice", "anything"));
    }

    #[test]
    fn malformed_hash_denies_without_panicking() {
        let users = vec!["alice:not-a-bcrypt-hash".to_string()];
        assert!(!verify_credentials(&users, "alice", "anything"));
    }

    #[test]
    fn malformed_entries_are_skipped_not_fatal() {
        // A garbage entry with no colon, plus an empty-username and empty-hash
        // entry, must be skipped while the valid one still authenticates.
        let hash = hash_password("right").expect("hash");
        let users = vec![
            "no-colon-here".to_string(),
            ":empty-username".to_string(),
            "empty-hash:".to_string(),
            format!("alice:{hash}"),
        ];
        let parsed = parse_users(&users);
        assert_eq!(parsed.len(), 1, "only the valid entry should parse");
        assert_eq!(parsed[0].username, "alice");
        assert!(verify_credentials(&users, "alice", "right"));
    }

    #[test]
    fn parse_user_splits_on_first_colon_only() {
        // bcrypt hashes never contain ':' but be defensive: the username is
        // everything before the first colon, the hash is the remainder verbatim.
        let user = parse_user("alice:$2b$12$abc:def").expect("parse");
        assert_eq!(user.username, "alice");
        assert_eq!(user.hash, "$2b$12$abc:def");
    }

    #[test]
    fn duplicate_username_last_wins() {
        // Two entries for "alice" with different passwords; the LAST one is
        // authoritative ("append to change the password").
        let old = hash_password("old-password").expect("hash old");
        let new = hash_password("new-password").expect("hash new");
        let users = vec![format!("alice:{old}"), format!("alice:{new}")];

        assert!(
            verify_credentials(&users, "alice", "new-password"),
            "the latest entry's password must verify"
        );
        assert!(
            !verify_credentials(&users, "alice", "old-password"),
            "the superseded entry's password must no longer verify"
        );
    }

    #[test]
    fn dummy_hash_is_a_valid_bcrypt_hash() {
        // The timing-mitigation dummy must be a parseable bcrypt hash so the
        // unknown-user verify actually runs the KDF (rather than erroring out
        // early, which would defeat the timing equalization).
        assert!(
            bcrypt::verify("anything-at-all", DUMMY_HASH).is_ok(),
            "DUMMY_HASH must be a valid bcrypt hash"
        );
        assert!(
            DUMMY_HASH.contains(&format!("${DEFAULT_COST}$")),
            "DUMMY_HASH should be cost {DEFAULT_COST} to match real verifies"
        );
    }

    #[test]
    fn htpasswd_prefix_variants_round_trip() {
        // Pin the advertised htpasswd compatibility: a real bcrypt fixture in
        // each of the $2a$/$2b$/$2y$ version families must verify with the right
        // password and reject the wrong one. All three are the SAME digest of
        // "htpasswd-pw" at cost 6 (cheap for a test) — only the version tag
        // differs, which is exactly what htpasswd-produced $2y$ hashes look like.
        const TWO_B: &str = "$2b$06$XKnmjYm7zibkFIc7mP7ulO5rBmEcWf6zEL/HQgbEUX9I7ue4k7NNS";
        const TWO_A: &str = "$2a$06$XKnmjYm7zibkFIc7mP7ulO5rBmEcWf6zEL/HQgbEUX9I7ue4k7NNS";
        const TWO_Y: &str = "$2y$06$XKnmjYm7zibkFIc7mP7ulO5rBmEcWf6zEL/HQgbEUX9I7ue4k7NNS";

        for (label, hash) in [("$2b$", TWO_B), ("$2a$", TWO_A), ("$2y$", TWO_Y)] {
            let users = vec![format!("alice:{hash}")];
            assert!(
                verify_credentials(&users, "alice", "htpasswd-pw"),
                "{label} hash must verify with the correct password"
            );
            assert!(
                !verify_credentials(&users, "alice", "wrong-pw"),
                "{label} hash must reject the wrong password"
            );
        }
    }

    #[test]
    fn auth_enabled_requires_a_valid_user() {
        let mut config = Config::default();
        assert!(
            !auth_enabled(&config, false),
            "no users means the gate is off"
        );

        let hash = hash_password("pw").expect("hash");
        config.auth.users = vec![format!("alice:{hash}")];
        assert!(
            auth_enabled(&config, false),
            "one valid user enables the gate"
        );

        assert!(
            !auth_enabled(&config, true),
            "--disable-auth forces the gate off even with users"
        );
    }

    #[test]
    fn validate_username_accepts_reasonable_names() {
        assert!(validate_username("alice").is_ok());
        assert!(validate_username("Bob").is_ok());
        assert!(validate_username("user_42-x").is_ok());
        assert!(validate_username("dev.lead").is_ok());
    }

    #[test]
    fn validate_username_rejects_empty() {
        assert!(validate_username("").is_err());
    }

    #[test]
    fn validate_username_rejects_colon() {
        // The colon is the username:hash separator — a username carrying one
        // would corrupt the stored entry.
        let err = validate_username("al:ice").expect_err("colon must be rejected");
        assert!(err.contains(':'), "message should explain the colon: {err}");
    }

    #[test]
    fn validate_username_rejects_whitespace_and_control() {
        assert!(validate_username("al ice").is_err());
        assert!(validate_username(" alice").is_err());
        assert!(validate_username("alice ").is_err());
        assert!(validate_username("al\tice").is_err());
        assert!(validate_username("al\nice").is_err());
    }

    #[test]
    fn validate_username_is_case_sensitive() {
        // Both forms are independently valid; the gate treats them as distinct
        // accounts (slice A1), so validation must not normalize case.
        assert!(validate_username("alice").is_ok());
        assert!(validate_username("ALICE").is_ok());
    }

    #[test]
    fn password_within_bcrypt_limit_boundary() {
        assert!(password_within_bcrypt_limit(""));
        assert!(password_within_bcrypt_limit(&"a".repeat(72)));
        assert!(!password_within_bcrypt_limit(&"a".repeat(73)));
        // Byte length, not char count: a 36-char multi-byte string is 72 bytes.
        assert!(password_within_bcrypt_limit(&"é".repeat(36)));
        assert!(!password_within_bcrypt_limit(&"é".repeat(37)));
    }

    #[test]
    fn auth_enabled_is_false_when_all_entries_malformed() {
        // Entries present but none valid: the gate is OFF (false). The startup
        // surface (A2) is responsible for warning loudly in this case; this
        // predicate just reports the effective state.
        let mut config = Config::default();
        config.auth.users = vec!["garbage".to_string(), ":nohash".to_string()];
        assert!(!auth_enabled(&config, false));
    }
}

//! The web login's sessions, as they are stored: random tokens that live in a
//! browser cookie and, on this side, only as SHA-256 digests in
//! `sessions.sqlite3`, so a quick restart of dux does not sign an open tab out
//! and a copy of the database (or of dux's memory) yields no usable cookie.
//!
//! Each session records the CREDENTIAL GENERATION it was issued under, a short
//! digest of the configured password hash ([`credential_generation`]). A session
//! is only good while that generation is the current one, so any change to the
//! password, including one made with `dux config set` while dux was stopped,
//! signs every browser out. Each record also carries when it was last used; the
//! idle timeout is applied by the reader, so a change to it applies to sessions
//! that already exist: `[server.auth] session_idle_seconds` for a browser's and
//! `[server.auth] cli_token_idle_days` for the command line's ([`SessionKind`]).
//!
//! What lives here is storage and the token arithmetic. Who may hold a session,
//! when it is refreshed, and what keeps it alive are the web layer's business.

use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine as _;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

/// Bytes of randomness in a session token: 256 bits.
pub const TOKEN_BYTES: usize = 32;

/// The SHA-256 digest a session is stored and looked up by.
pub type TokenDigest = [u8; 32];

/// A newly minted session token. `cookie_value` goes to the browser once and is
/// never kept; `digest` is what the server stores.
pub struct NewToken {
    pub cookie_value: String,
    pub digest: TokenDigest,
}

impl std::fmt::Debug for NewToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NewToken(<redacted>)")
    }
}

/// Mint a token from the operating system's random generator.
pub fn new_token() -> Result<NewToken> {
    let mut raw = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut raw)
        .map_err(|e| anyhow::anyhow!("could not read the system's random generator: {e}"))?;
    let cookie_value = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let digest = digest_raw(&raw);
    raw.fill(0);
    Ok(NewToken {
        cookie_value,
        digest,
    })
}

/// The digest of a token as a browser sent it back, or `None` when the value is
/// not one dux could have minted (wrong length, not URL-safe base64).
pub fn digest_of(cookie_value: &str) -> Option<TokenDigest> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cookie_value.as_bytes())
        .ok()?;
    if raw.len() != TOKEN_BYTES {
        return None;
    }
    Some(digest_raw(&raw))
}

fn digest_raw(raw: &[u8]) -> TokenDigest {
    Sha256::digest(raw).into()
}

/// The credential generation of a configured password hash: the first 16 bytes
/// of its SHA-256, in hex. Empty for no password. Two different hash strings
/// (a new password, or the same password hashed again with a fresh salt) are
/// two generations.
pub fn credential_generation(password_hash: &str) -> String {
    if password_hash.is_empty() {
        return String::new();
    }
    let digest = Sha256::digest(password_hash.as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// Who holds a session, which decides how long it may sit unused: a browser's
/// is `[server.auth] session_idle_seconds`, the command line's is
/// `[server.auth] cli_token_idle_days`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    Browser,
    Cli,
}

impl SessionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Cli => "cli",
        }
    }

    /// An unknown word reads as a browser session, the shorter lived of the two.
    fn from_stored(text: &str) -> Self {
        match text {
            "cli" => Self::Cli,
            _ => Self::Browser,
        }
    }
}

/// One stored session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSession {
    pub digest: TokenDigest,
    pub generation: String,
    /// When it was last used, in milliseconds since the Unix epoch.
    pub last_seen_ms: i64,
    pub kind: SessionKind,
}

/// The `web_sessions` table in `sessions.sqlite3`, on a connection of its own:
/// the engine keeps its own connection to the same file, and WAL plus a busy
/// timeout let the two write without failing each other.
pub struct WebSessionStore {
    conn: Connection,
}

impl WebSessionStore {
    /// Open the database at `path`, creating the table when it is missing.
    pub fn open(path: &Path) -> Result<Self> {
        let conn =
            Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        crate::file_modes::restrict_to_owner_best_effort(path, "session database");
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// An in-memory store, for tests.
    pub fn in_memory() -> Result<Self> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "create table if not exists web_sessions (
                digest blob primary key,
                generation text not null,
                last_seen_ms integer not null
            );",
        )?;
        let has_kind = self
            .conn
            .prepare("select 1 from pragma_table_info('web_sessions') where name = 'kind'")?
            .exists([])?;
        if !has_kind {
            self.conn.execute_batch(
                "alter table web_sessions add column kind text not null default 'browser';",
            )?;
        }
        Ok(())
    }

    /// Every stored session.
    pub fn load(&self) -> Result<Vec<StoredSession>> {
        let mut statement = self
            .conn
            .prepare("select digest, generation, last_seen_ms, kind from web_sessions")?;
        let rows = statement.query_map([], |row| {
            let digest: Vec<u8> = row.get(0)?;
            Ok((
                digest,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (digest, generation, last_seen_ms, kind) = row?;
            // A row whose key is not a digest was not written by dux; skip it.
            let Ok(digest) = TokenDigest::try_from(digest.as_slice()) else {
                continue;
            };
            out.push(StoredSession {
                digest,
                generation,
                last_seen_ms,
                kind: SessionKind::from_stored(&kind),
            });
        }
        Ok(out)
    }

    /// Insert a session, or move an existing one's last use and generation.
    pub fn upsert(&self, session: &StoredSession) -> Result<()> {
        self.conn.execute(
            "insert into web_sessions (digest, generation, last_seen_ms, kind)
             values (?1, ?2, ?3, ?4)
             on conflict(digest) do update set
                generation = excluded.generation,
                last_seen_ms = max(web_sessions.last_seen_ms, excluded.last_seen_ms)",
            params![
                session.digest.as_slice(),
                session.generation,
                session.last_seen_ms,
                session.kind.as_str()
            ],
        )?;
        Ok(())
    }

    /// Move the last use of every listed session that still exists, in one
    /// transaction. A session deleted meanwhile is not brought back.
    pub fn touch(&mut self, sessions: &[(TokenDigest, i64)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut statement = tx.prepare(
                "update web_sessions set last_seen_ms = max(last_seen_ms, ?2) where digest = ?1",
            )?;
            for (digest, last_seen_ms) in sessions {
                statement.execute(params![digest.as_slice(), last_seen_ms])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Delete one session.
    pub fn delete(&self, digest: &TokenDigest) -> Result<()> {
        self.conn.execute(
            "delete from web_sessions where digest = ?1",
            params![digest.as_slice()],
        )?;
        Ok(())
    }

    /// Delete every session not issued under `generation`.
    pub fn delete_other_generations(&self, generation: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "delete from web_sessions where generation <> ?1",
            params![generation],
        )?)
    }

    /// Delete every browser session last used before `browser_cutoff_ms` and
    /// every command-line one last used before `cli_cutoff_ms`.
    pub fn delete_idle_since(&self, browser_cutoff_ms: i64, cli_cutoff_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "delete from web_sessions
             where (kind = 'cli' and last_seen_ms < ?2)
                or (kind <> 'cli' and last_seen_ms < ?1)",
            params![browser_cutoff_ms, cli_cutoff_ms],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(byte: u8, generation: &str, last_seen_ms: i64) -> StoredSession {
        StoredSession {
            digest: [byte; 32],
            generation: generation.to_string(),
            last_seen_ms,
            kind: SessionKind::Browser,
        }
    }

    fn cli_session(byte: u8, generation: &str, last_seen_ms: i64) -> StoredSession {
        StoredSession {
            kind: SessionKind::Cli,
            ..session(byte, generation, last_seen_ms)
        }
    }

    #[test]
    fn a_token_is_256_random_bits_and_its_digest_is_what_the_browser_value_hashes_to() {
        let a = new_token().expect("token");
        let b = new_token().expect("token");
        assert_ne!(a.cookie_value, b.cookie_value);
        assert_eq!(a.cookie_value.len(), 43, "32 bytes of unpadded base64");
        assert_eq!(digest_of(&a.cookie_value), Some(a.digest));
        assert_ne!(a.digest, b.digest);
        assert!(!format!("{a:?}").contains(&a.cookie_value));
    }

    #[test]
    fn a_value_dux_could_not_have_minted_has_no_digest() {
        for bad in ["", "short", "!!!!", &"A".repeat(44), &"A".repeat(1000)] {
            assert_eq!(digest_of(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_generation_follows_the_hash_string() {
        assert_eq!(credential_generation(""), "");
        let one = credential_generation("$argon2id$v=19$m=19456,t=2,p=1$a$b");
        let two = credential_generation("$argon2id$v=19$m=19456,t=2,p=1$a$c");
        assert_eq!(one.len(), 32);
        assert_ne!(one, two);
        assert_eq!(
            one,
            credential_generation("$argon2id$v=19$m=19456,t=2,p=1$a$b")
        );
    }

    #[test]
    fn sessions_round_trip_and_a_touch_never_moves_one_backwards() {
        let mut store = WebSessionStore::in_memory().unwrap();
        store.upsert(&session(1, "g1", 1_000)).unwrap();
        store.upsert(&cli_session(2, "g1", 2_000)).unwrap();
        store
            .touch(&[([1; 32], 5_000), ([2; 32], 1_500), ([9; 32], 9_000)])
            .unwrap();
        let mut loaded = store.load().unwrap();
        loaded.sort_by_key(|s| s.digest);
        assert_eq!(
            loaded,
            vec![session(1, "g1", 5_000), cli_session(2, "g1", 2_000)]
        );
    }

    #[test]
    fn deleting_by_digest_generation_and_idleness() {
        let store = WebSessionStore::in_memory().unwrap();
        store.upsert(&session(1, "old", 1_000)).unwrap();
        store.upsert(&session(2, "new", 1_000)).unwrap();
        store.upsert(&session(3, "new", 9_000)).unwrap();
        store.upsert(&session(4, "new", 9_000)).unwrap();
        store.upsert(&cli_session(5, "new", 3_000)).unwrap();
        store.upsert(&cli_session(6, "new", 1_000)).unwrap();
        assert_eq!(store.delete_other_generations("new").unwrap(), 1);
        // Browser sessions go before 5_000, command-line ones before 2_000.
        assert_eq!(store.delete_idle_since(5_000, 2_000).unwrap(), 2);
        store.delete(&[4; 32]).unwrap();
        let mut left = store.load().unwrap();
        left.sort_by_key(|s| s.digest);
        assert_eq!(
            left,
            vec![session(3, "new", 9_000), cli_session(5, "new", 3_000)]
        );
    }

    #[test]
    fn the_table_lives_beside_the_engines_and_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        // The engine's own store opens the same file.
        let _engine = crate::storage::SessionStore::open(&path).unwrap();
        WebSessionStore::open(&path)
            .unwrap()
            .upsert(&session(7, "g", 42))
            .unwrap();
        assert_eq!(
            WebSessionStore::open(&path).unwrap().load().unwrap(),
            vec![session(7, "g", 42)]
        );
    }

    #[test]
    fn sessions_stored_before_the_kind_column_load_as_browser_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "create table web_sessions (
                    digest blob primary key,
                    generation text not null,
                    last_seen_ms integer not null
                );",
            )
            .unwrap();
            old.execute(
                "insert into web_sessions (digest, generation, last_seen_ms) values (?1, 'g', 42)",
                params![[5u8; 32].as_slice()],
            )
            .unwrap();
        }
        let store = WebSessionStore::open(&path).unwrap();
        assert_eq!(store.load().unwrap(), vec![session(5, "g", 42)]);
        // Opening again does not add the column twice.
        drop(store);
        assert_eq!(
            WebSessionStore::open(&path).unwrap().load().unwrap(),
            vec![session(5, "g", 42)]
        );
    }
}

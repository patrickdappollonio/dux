//! Signed-in browsers. A session is a random token in a cookie; dux keeps its
//! SHA-256 digest, the credential generation it was issued under, and when it
//! was last used, in memory and in `sessions.sqlite3`
//! (`dux_core::web_sessions`), so a quick restart does not sign an open tab out.
//!
//! A session is good while its generation is the current one and it is not
//! idle: idle means no request for `session_idle_seconds` (a browser's) or
//! `cli_token_idle_days` (the command line's, [`SessionKind::Cli`]) AND no open
//! socket.
//! An open socket holds a [`SessionLease`]; while any lease is held the session
//! cannot go idle, and when the last one is released the idle clock starts
//! from that moment. The socket loops keep a lease only while the peer answers
//! (`crate::server`'s pong deadline), so a half-open connection cannot keep a
//! session alive for long.
//!
//! Persistence is write-behind: issuing and revoking are written before the
//! request answers; the last-use times are written every few seconds (and the
//! leased sessions' with them), so a crash loses at most that much of a
//! session's freshness. Every SQLite call runs on a blocking thread.
//!
//! The session layer knows nothing about how a token travels: a browser sends
//! it in a cookie and the command line as a bearer header.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use dux_core::web_sessions::{NewToken, SessionKind, StoredSession, TokenDigest, WebSessionStore};

/// Milliseconds since the Unix epoch, injectable so tests can move time.
pub(crate) type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The wall clock.
pub(crate) fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    })
}

/// How long a session may sit unused before it ends, per kind, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Idle {
    pub(crate) browser_ms: i64,
    pub(crate) cli_ms: i64,
}

impl Idle {
    fn of(self, kind: SessionKind) -> i64 {
        match kind {
            SessionKind::Browser => self.browser_ms,
            SessionKind::Cli => self.cli_ms,
        }
    }
}

struct Live {
    kind: SessionKind,
    generation: String,
    last_seen_ms: i64,
    leases: u32,
    /// Used since the last write of its last-use time.
    dirty: bool,
    /// Issued before the table was open: written by the load that opens it.
    unstored: bool,
}

struct Inner {
    map: std::sync::Mutex<HashMap<TokenDigest, Live>>,
    /// The table, once opened. `None` until the load finishes, and for good
    /// when it could not be opened (sessions then last only as long as the run).
    store: std::sync::Mutex<Option<WebSessionStore>>,
    /// Whether the stored sessions have been loaded (or given up on).
    loaded: tokio::sync::watch::Sender<bool>,
    clock: Clock,
}

/// The sessions of one serve. Cloning shares them.
#[derive(Clone)]
pub(crate) struct Sessions(Arc<Inner>);

impl Sessions {
    /// Sessions whose stored copy has yet to be loaded with [`Self::load`].
    pub(crate) fn new(clock: Clock) -> Self {
        Self(Arc::new(Inner {
            map: std::sync::Mutex::default(),
            store: std::sync::Mutex::new(None),
            loaded: tokio::sync::watch::Sender::new(false),
            clock,
        }))
    }

    /// Sessions that are never stored (a router no serve owns a database for).
    pub(crate) fn in_memory(clock: Clock) -> Self {
        let sessions = Self::new(clock);
        sessions.0.loaded.send_replace(true);
        sessions
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<TokenDigest, Live>> {
        self.0
            .map
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn now(&self) -> i64 {
        (self.0.clock)()
    }

    /// Open the table at `db`, drop sessions of another generation or idle past `idle`,
    /// and adopt the rest. One that cannot be opened is logged and sessions live in memory.
    pub(crate) async fn load(&self, db: PathBuf, generation: String, idle: Idle) {
        let now = self.now();
        let browser_cutoff = now.saturating_sub(idle.browser_ms);
        let cli_cutoff = now.saturating_sub(idle.cli_ms);
        let inner = Arc::clone(&self.0);
        let opened = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let store = WebSessionStore::open(&db)?;
            store.delete_other_generations(&generation)?;
            store.delete_idle_since(browser_cutoff, cli_cutoff)?;
            let rows = store.load()?;
            // The table is put in place under its own lock, the one `issue`
            // writes under, so a session issued while this ran is either
            // written by `issue` itself or collected here: never neither.
            let mut slot = inner
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let unstored: Vec<StoredSession> = {
                let mut map = inner
                    .map
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for StoredSession {
                    digest,
                    generation,
                    last_seen_ms,
                    kind,
                } in rows
                {
                    map.entry(digest).or_insert(Live {
                        kind,
                        generation,
                        last_seen_ms,
                        leases: 0,
                        dirty: false,
                        unstored: false,
                    });
                }
                map.iter_mut()
                    .filter_map(|(digest, live)| {
                        let unstored = std::mem::take(&mut live.unstored);
                        (unstored && live.generation == generation).then(|| StoredSession {
                            digest: *digest,
                            generation: live.generation.clone(),
                            last_seen_ms: live.last_seen_ms,
                            kind: live.kind,
                        })
                    })
                    .collect()
            };
            for row in &unstored {
                store.upsert(row)?;
            }
            *slot = Some(store);
            Ok(())
        })
        .await;
        match opened {
            Ok(Ok(())) => {}
            Ok(Err(error)) => dux_core::logger::warn(&format!(
                "[server] could not open the web sessions in the session database ({error:#}); \
                 sign-ins last only until dux stops"
            )),
            Err(error) => dux_core::logger::warn(&format!(
                "[server] loading the web sessions stopped ({error}); sign-ins last only until \
                 dux stops"
            )),
        }
        self.0.loaded.send_replace(true);
    }

    /// Wait until the stored sessions are loaded, so a browser coming back
    /// right after a restart is not turned away by a session not read yet.
    pub(crate) async fn ready(&self) {
        let mut loaded = self.0.loaded.subscribe();
        let _ = loaded.wait_for(|loaded| *loaded).await;
    }

    /// Issue a session under `generation` and store it before answering. One
    /// issued before the table is open is never dropped (decided, after
    /// review): it is marked, and the load that opens the table writes it.
    pub(crate) async fn issue(
        &self,
        generation: &str,
        kind: SessionKind,
    ) -> anyhow::Result<NewToken> {
        let token = dux_core::web_sessions::new_token()?;
        let now = self.now();
        let row = StoredSession {
            digest: token.digest,
            generation: generation.to_string(),
            last_seen_ms: now,
            kind,
        };
        let inner = Arc::clone(&self.0);
        let written = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            // Under the table's lock, the same one the load puts it in place
            // under, so the session is in the map before the load collects.
            let mut slot = inner
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let loaded = *inner.loaded.borrow();
            inner
                .map
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(
                    row.digest,
                    Live {
                        kind,
                        generation: row.generation.clone(),
                        last_seen_ms: row.last_seen_ms,
                        leases: 0,
                        dirty: false,
                        unstored: slot.is_none() && !loaded,
                    },
                );
            match slot.as_mut() {
                Some(store) => store.upsert(&row),
                None if loaded => {
                    // A run with no session table (it could not be opened,
                    // which was said at the time, or none was asked for).
                    dux_core::logger::debug(
                        "[server] a web session was issued with no session table open; it \
                         lasts until dux stops",
                    );
                    Ok(())
                }
                None => Ok(()),
            }
        })
        .await;
        match written {
            Ok(Ok(())) => {}
            Ok(Err(error)) => dux_core::logger::warn(&format!(
                "[server] could not write a web session to the session database: {error:#}"
            )),
            Err(error) => {
                dux_core::logger::warn(&format!("[server] writing a web session stopped: {error}"))
            }
        }
        Ok(token)
    }

    /// Whether `digest` names a usable session under `generation`, refreshing
    /// its last use when `touch` is set. An idle one is forgotten.
    pub(crate) fn check(
        &self,
        digest: &TokenDigest,
        generation: &str,
        idle: Idle,
        touch: bool,
    ) -> bool {
        let now = self.now();
        let mut map = self.map();
        let Some(live) = map.get_mut(digest) else {
            return false;
        };
        if live.generation != generation {
            map.remove(digest);
            return false;
        }
        if live.leases == 0 && now.saturating_sub(live.last_seen_ms) > idle.of(live.kind) {
            map.remove(digest);
            return false;
        }
        if touch {
            live.last_seen_ms = live.last_seen_ms.max(now);
            live.dirty = true;
        }
        true
    }

    /// End one session, in memory and in the table, before answering.
    pub(crate) async fn revoke(&self, digest: TokenDigest) {
        self.map().remove(&digest);
        self.with_store(move |store| store.delete(&digest)).await;
    }

    /// Forget every session not issued under `generation`. Answers whether any
    /// was forgotten. The table is cleaned in the background.
    pub(crate) fn retain_generation(&self, generation: &str) -> bool {
        let removed = {
            let mut map = self.map();
            let before = map.len();
            map.retain(|_, live| live.generation == generation);
            before != map.len()
        };
        let sessions = self.clone();
        let generation = generation.to_string();
        tokio::spawn(async move {
            sessions
                .with_store(move |store| store.delete_other_generations(&generation).map(|_| ()))
                .await;
        });
        removed
    }

    /// Hold `digest` alive for as long as the lease lives. `None` when the
    /// session is not known.
    pub(crate) fn lease(&self, digest: TokenDigest) -> Option<SessionLease> {
        let now = self.now();
        {
            let mut map = self.map();
            let live = map.get_mut(&digest)?;
            live.leases += 1;
            live.last_seen_ms = live.last_seen_ms.max(now);
        }
        // An open tab is the session's activity, so its start is written now
        // rather than at the next flush: a restart right after a tab opened
        // finds a fresh last use.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let sessions = self.clone();
            runtime.spawn(async move {
                sessions
                    .with_store(move |store| store.touch(&[(digest, now)]))
                    .await;
            });
        }
        Some(SessionLease {
            sessions: self.clone(),
            digest,
        })
    }

    /// Write the last-use times that moved (and every leased session's, as of
    /// now) and forget what went idle, in memory and in the table.
    pub(crate) async fn flush(&self, idle: Idle) {
        let now = self.now();
        let touched: Vec<(TokenDigest, i64)> = {
            let mut map = self.map();
            map.retain(|_, live| {
                live.leases > 0 || now.saturating_sub(live.last_seen_ms) <= idle.of(live.kind)
            });
            map.iter_mut()
                .filter_map(|(digest, live)| {
                    if live.leases > 0 {
                        live.last_seen_ms = now;
                        live.dirty = true;
                    }
                    std::mem::take(&mut live.dirty).then_some((*digest, live.last_seen_ms))
                })
                .collect()
        };
        let browser_cutoff = now.saturating_sub(idle.browser_ms);
        let cli_cutoff = now.saturating_sub(idle.cli_ms);
        self.with_store(move |store| {
            store.touch(&touched)?;
            store
                .delete_idle_since(browser_cutoff, cli_cutoff)
                .map(|_| ())
        })
        .await;
    }

    /// How many sessions are known right now.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map().len()
    }

    /// Run one write on a blocking thread, if the table is open. A failure is
    /// said in dux.log and nothing more: the session in memory is still right.
    async fn with_store(
        &self,
        write: impl FnOnce(&mut WebSessionStore) -> anyhow::Result<()> + Send + 'static,
    ) {
        let inner = Arc::clone(&self.0);
        let done = tokio::task::spawn_blocking(move || {
            let mut slot = inner
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match slot.as_mut() {
                Some(store) => write(store),
                None => Ok(()),
            }
        })
        .await;
        match done {
            Ok(Ok(())) => {}
            Ok(Err(error)) => dux_core::logger::warn(&format!(
                "[server] could not write a web session to the session database: {error:#}"
            )),
            Err(error) => {
                dux_core::logger::warn(&format!("[server] writing a web session stopped: {error}"))
            }
        }
    }
}

/// Keeps a session from going idle while an open socket holds it.
pub(crate) struct SessionLease {
    sessions: Sessions,
    digest: TokenDigest,
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        let now = self.sessions.now();
        let mut map = self.sessions.map();
        if let Some(live) = map.get_mut(&self.digest) {
            live.leases = live.leases.saturating_sub(1);
            live.last_seen_ms = live.last_seen_ms.max(now);
            live.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    fn idle(ms: i64) -> Idle {
        Idle {
            browser_ms: ms,
            cli_ms: ms,
        }
    }

    fn clock_at(start: i64) -> (Clock, Arc<AtomicI64>) {
        let now = Arc::new(AtomicI64::new(start));
        let reads = Arc::clone(&now);
        (Arc::new(move || reads.load(Ordering::SeqCst)), now)
    }

    #[tokio::test]
    async fn a_session_is_good_until_it_goes_idle_and_any_use_refreshes_it() {
        let (clock, now) = clock_at(1_000_000);
        let sessions = Sessions::in_memory(clock);
        let token = sessions.issue("g", SessionKind::Browser).await.unwrap();
        assert!(sessions.check(&token.digest, "g", idle(60_000), true));
        now.fetch_add(59_000, Ordering::SeqCst);
        assert!(
            sessions.check(&token.digest, "g", idle(60_000), true),
            "refreshed"
        );
        now.fetch_add(59_000, Ordering::SeqCst);
        assert!(sessions.check(&token.digest, "g", idle(60_000), false));
        now.fetch_add(2_000, Ordering::SeqCst);
        assert!(
            !sessions.check(&token.digest, "g", idle(60_000), true),
            "a check that does not touch refreshes nothing"
        );
        assert_eq!(sessions.len(), 0);
    }

    #[tokio::test]
    async fn another_generation_is_never_good_and_is_forgotten() {
        let (clock, _) = clock_at(0);
        let sessions = Sessions::in_memory(clock);
        let token = sessions.issue("old", SessionKind::Browser).await.unwrap();
        assert!(!sessions.check(&token.digest, "new", idle(60_000), true));
        let token = sessions.issue("old", SessionKind::Browser).await.unwrap();
        assert!(sessions.retain_generation("new"));
        assert!(!sessions.check(&token.digest, "old", idle(60_000), true));
    }

    #[tokio::test]
    async fn a_lease_keeps_a_session_alive_and_its_release_starts_the_idle_clock() {
        let (clock, now) = clock_at(0);
        let sessions = Sessions::in_memory(clock);
        let token = sessions.issue("g", SessionKind::Browser).await.unwrap();
        let lease = sessions.lease(token.digest).expect("known");
        now.store(10 * 60_000, Ordering::SeqCst);
        assert!(
            sessions.check(&token.digest, "g", idle(3_000), false),
            "leased"
        );
        sessions.flush(idle(3_000)).await;
        assert_eq!(sessions.len(), 1, "a flush keeps a leased session");
        drop(lease);
        now.fetch_add(2_000, Ordering::SeqCst);
        assert!(
            sessions.check(&token.digest, "g", idle(3_000), false),
            "idle from the release"
        );
        now.fetch_add(2_000, Ordering::SeqCst);
        assert!(!sessions.check(&token.digest, "g", idle(3_000), false));
        assert!(sessions.lease([9; 32]).is_none());
    }

    /// An open tab is the session's activity, so the moment a socket takes its
    /// lease is written at once: a restart right after a tab opened must find a
    /// fresh last use, not the sign-in's.
    #[tokio::test]
    async fn taking_a_lease_writes_the_sessions_last_use_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.sqlite3");
        let (clock, now) = clock_at(1_000_000);
        let sessions = Sessions::new(clock);
        sessions.load(db.clone(), "g".into(), idle(60_000)).await;
        let token = sessions.issue("g", SessionKind::Browser).await.unwrap();
        now.fetch_add(5_000, Ordering::SeqCst);
        let _lease = sessions.lease(token.digest).unwrap();
        let stored = || WebSessionStore::open(&db).unwrap().load().unwrap()[0].last_seen_ms;
        for _ in 0..100 {
            if stored() == 1_005_000 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(stored(), 1_005_000);
    }

    #[tokio::test]
    async fn revoking_forgets_a_session() {
        let (clock, _) = clock_at(0);
        let sessions = Sessions::in_memory(clock);
        let token = sessions.issue("g", SessionKind::Browser).await.unwrap();
        sessions.revoke(token.digest).await;
        assert!(!sessions.check(&token.digest, "g", idle(60_000), true));
    }

    #[tokio::test]
    async fn sessions_survive_a_restart_within_the_idle_window_and_not_past_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.sqlite3");
        let (clock, now) = clock_at(1_000_000);
        let first = Sessions::new(Arc::clone(&clock));
        first.load(db.clone(), "g".into(), idle(8_000)).await;
        let kept = first.issue("g", SessionKind::Browser).await.unwrap();
        let other = first.issue("g", SessionKind::Browser).await.unwrap();
        let lease = first.lease(other.digest).unwrap();
        now.fetch_add(5_000, Ordering::SeqCst);
        first.check(&kept.digest, "g", idle(8_000), true);
        first.flush(idle(8_000)).await;
        drop(lease);
        drop(first);

        // A quick restart: both are still good.
        now.fetch_add(2_000, Ordering::SeqCst);
        let second = Sessions::new(Arc::clone(&clock));
        second.load(db.clone(), "g".into(), idle(8_000)).await;
        second.ready().await;
        assert!(second.check(&kept.digest, "g", idle(8_000), false));
        assert!(second.check(&other.digest, "g", idle(8_000), false));
        drop(second);

        // A long outage: gone, and gone from the table too.
        now.fetch_add(20_000, Ordering::SeqCst);
        let third = Sessions::new(Arc::clone(&clock));
        third.load(db.clone(), "g".into(), idle(8_000)).await;
        assert!(!third.check(&kept.digest, "g", idle(8_000), false));
        assert!(
            WebSessionStore::open(&db)
                .unwrap()
                .load()
                .unwrap()
                .is_empty(),
            "the long outage cleaned the table"
        );
    }

    #[tokio::test]
    async fn a_password_changed_while_dux_was_stopped_signs_every_stored_session_out() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.sqlite3");
        let (clock, _) = clock_at(0);
        let first = Sessions::new(Arc::clone(&clock));
        first.load(db.clone(), "before".into(), idle(60_000)).await;
        let token = first.issue("before", SessionKind::Browser).await.unwrap();
        drop(first);
        let second = Sessions::new(clock);
        second.load(db, "after".into(), idle(60_000)).await;
        assert!(!second.check(&token.digest, "after", idle(60_000), false));
        assert!(!second.check(&token.digest, "before", idle(60_000), false));
    }

    #[tokio::test]
    async fn an_unopenable_database_leaves_sessions_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let (clock, _) = clock_at(0);
        let sessions = Sessions::new(clock);
        // A directory is not a database.
        sessions
            .load(dir.path().to_path_buf(), "g".into(), idle(60_000))
            .await;
        sessions.ready().await;
        let token = sessions.issue("g", SessionKind::Browser).await.unwrap();
        assert!(sessions.check(&token.digest, "g", idle(60_000), true));
    }

    /// A session issued before the stored sessions finished loading is still
    /// written: the load that opens the table writes it.
    #[tokio::test]
    async fn a_session_issued_before_the_load_finishes_is_stored() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.sqlite3");
        let (clock, _now) = clock_at(1_000_000);
        let sessions = Sessions::new(Arc::clone(&clock));
        let early = sessions.issue("g", SessionKind::Browser).await.unwrap();
        sessions.load(db.clone(), "g".into(), idle(60_000)).await;
        assert!(sessions.check(&early.digest, "g", idle(60_000), false));
        let stored = WebSessionStore::open(&db).unwrap().load().unwrap();
        assert!(
            stored.iter().any(|row| row.digest == early.digest),
            "the session issued before the load was never written to the table"
        );
    }

    fn idle_of(browser_ms: i64, cli_ms: i64) -> Idle {
        Idle { browser_ms, cli_ms }
    }

    #[tokio::test]
    async fn a_cli_session_outlasts_the_browser_idle_window_and_a_browser_session_does_not() {
        let (clock, now) = clock_at(0);
        let sessions = Sessions::in_memory(clock);
        let browser = sessions.issue("g", SessionKind::Browser).await.unwrap();
        let cli = sessions.issue("g", SessionKind::Cli).await.unwrap();
        let windows = idle_of(60_000, 30 * 86_400_000);
        now.store(10 * 60_000, Ordering::SeqCst);
        assert!(!sessions.check(&browser.digest, "g", windows, false));
        assert!(sessions.check(&cli.digest, "g", windows, true));
        // A flush keeps what is still inside its own window.
        sessions.flush(windows).await;
        assert_eq!(sessions.len(), 1);
        // Thirty days and a millisecond with no use ends it.
        now.fetch_add(30 * 86_400_000 + 1, Ordering::SeqCst);
        assert!(!sessions.check(&cli.digest, "g", windows, false));
    }

    #[tokio::test]
    async fn stored_sessions_are_judged_by_their_kind_when_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.sqlite3");
        let (clock, now) = clock_at(1_000_000);
        let first = Sessions::new(Arc::clone(&clock));
        first
            .load(db.clone(), "g".into(), idle_of(8_000, 100_000))
            .await;
        let browser = first.issue("g", SessionKind::Browser).await.unwrap();
        let cli = first.issue("g", SessionKind::Cli).await.unwrap();
        drop(first);

        now.fetch_add(20_000, Ordering::SeqCst);
        let second = Sessions::new(clock);
        second
            .load(db.clone(), "g".into(), idle_of(8_000, 100_000))
            .await;
        assert!(!second.check(&browser.digest, "g", idle_of(8_000, 100_000), false));
        assert!(second.check(&cli.digest, "g", idle_of(8_000, 100_000), false));
        let stored = WebSessionStore::open(&db).unwrap().load().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].kind, SessionKind::Cli);
    }
}

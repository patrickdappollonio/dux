//! Shared `config.toml` writer built on `toml_edit`.
//!
//! This module owns the surgical PATCH path: given an in-memory [`Config`], it
//! updates only the keys it manages in an existing TOML document, preserving the
//! user's comments, formatting, and any unknown keys. It deliberately does NOT
//! render the fully-commented canonical template: that path needs the TUI's
//! `RuntimeBindings` for two comment strings and stays in the binary.
//!
//! Both the TUI and the web surface share this patch path so a save from either
//! preserves the same on-disk shape. The TUI keeps its own pretty
//! first-creation renderer; the web uses [`save_config`] for a plain fallback.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use toml_edit::{Array, Decor, DocumentMut, Formatted, InlineTable, Item, Key, Table, Value};

/// Permission bits for `config.toml`: owner read/write only (`0600`). The file
/// may hold secrets such as tokens under `[env]`, so it must not be group/world
/// readable.
///
/// This is the ONE rule for every file dux keeps for itself, not a rule about
/// the config file in particular. It lives in [`crate::file_modes`] alongside
/// the directory mode and the tightening pass, so the database, its sidecars,
/// and the log get the same answer rather than three separate decisions.
use crate::file_modes::PRIVATE_FILE_MODE as CONFIG_FILE_MODE;

/// Whether an atomic write fsyncs the file before the rename. Eager (critical)
/// writes use `Fsync` for power-loss durability of the file's data; lazy writes
/// use `NoFsync` (crash-safe via rename, but not power-loss-durable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    Fsync,
    NoFsync,
}

/// The cross-process lock every write of `config.toml` holds, so writers in
/// different processes (a running dux, `dux config set`, a second thread of
/// the same dux) take turns instead of interleaving.
///
/// What it protects is the READ-modify-write: a writer that re-reads the file
/// and patches it under this lock can never land on top of an update another
/// writer made between its read and its write. The rename itself was already
/// atomic; that only ever ruled out a torn file, never a lost update.
///
/// It is an advisory `flock(2)` on a lock file beside the config, kept apart
/// from `dux.lock` (which a running dux holds for its whole life). The kernel
/// releases it when the holder exits, crash included. Not reentrant: a holder
/// must not try to take it again on the same thread.
#[derive(Debug)]
pub struct ConfigFileLock {
    file: fs::File,
}

impl ConfigFileLock {
    /// How long a writer waits for another one before giving up out loud.
    pub const DEFAULT_WAIT: Duration = Duration::from_secs(10);

    /// The lock file guarding writes to `config_path`.
    pub fn lock_path(config_path: &Path) -> PathBuf {
        let dir = config_path.parent().unwrap_or_else(|| Path::new("."));
        dir.join(CONFIG_WRITE_LOCK_NAME)
    }

    /// Take the lock, waiting up to [`Self::DEFAULT_WAIT`].
    pub fn acquire(config_path: &Path) -> Result<Self> {
        Self::acquire_within(config_path, Self::DEFAULT_WAIT)
    }

    /// Take the lock, waiting up to `wait`. A lock still held after that is
    /// an error naming the lock file, never an endless wait.
    pub fn acquire_within(config_path: &Path, wait: Duration) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let lock_path = Self::lock_path(config_path);
        // flock needs only a readable descriptor, so the file is opened
        // read-only: a lock file left read-only, or owned by root after a
        // `sudo dux`, still locks. It is created (owner-only) when missing.
        if let Err(error) = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(CONFIG_FILE_MODE)
            .open(&lock_path)
            && error.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(error).with_context(|| {
                format!("failed to create the config lock {}", lock_path.display())
            });
        }
        let file = fs::File::open(&lock_path).with_context(|| {
            format!(
                "failed to open the config lock {}; it must be readable by you, its owner \
                 (if another user owns it, `chown` it back or delete it)",
                lock_path.display()
            )
        })?;
        let deadline = Instant::now() + wait;
        loop {
            let outcome = crate::io_retry::retry_on_interrupt_errno(|| {
                flock(&file, FlockOperation::NonBlockingLockExclusive)
            });
            match outcome {
                Ok(()) => return Ok(Self { file }),
                Err(err) if err == Errno::WOULDBLOCK || err == Errno::AGAIN => {
                    if Instant::now() >= deadline {
                        anyhow::bail!(
                            "another dux process has been writing {} for over {} seconds, so \
                             this change was not saved; try again in a moment (lock file {})",
                            config_path.display(),
                            wait.as_secs_f32(),
                            lock_path.display()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(err) => {
                    return Err(std::io::Error::from(err))
                        .with_context(|| format!("failed to lock {}", lock_path.display()));
                }
            }
        }
    }
}

impl Drop for ConfigFileLock {
    fn drop(&mut self) {
        let _ =
            crate::io_retry::retry_on_interrupt_errno(|| flock(&self.file, FlockOperation::Unlock));
    }
}

/// The lock file's name, in the config file's own directory. One name for
/// the whole directory, so a backup written beside the config takes turns
/// with it too and leaves no lock file of its own behind.
const CONFIG_WRITE_LOCK_NAME: &str = ".config-write.lock";

/// Atomically write `contents` to `path`: a temp file in the same directory
/// (created `0600`), optionally fsync'd, then `rename`d into place. The temp file
/// self-deletes on drop if the rename never happens, so a failed/panicking write
/// leaves no orphan and never a partial real file. Holds the
/// [`ConfigFileLock`] for the write.
pub fn write_config_atomic(path: &Path, contents: &str, durability: Durability) -> Result<()> {
    let _lock = ConfigFileLock::acquire(path)?;
    // Judged against the file as it is on disk, under the lock.
    let on_disk = std::fs::read_to_string(path).ok();
    check_auth_added_by(path, on_disk.as_deref(), contents, "write")?;
    write_config_atomic_unlocked(path, contents, durability)
}

/// Each `[server.auth]` problem `raw` has (the section's own, and a password
/// setting where dux does not read it), each known by what it is about
/// rather than by a sentence: positions move when a write patches the file,
/// and the same problem must read as the same problem before and after.
fn auth_problem_identities(raw: &str) -> std::collections::BTreeSet<String> {
    crate::config::auth_problems_of(raw)
        .into_iter()
        .map(|problem| {
            // A line the sentence names is no part of what the problem is.
            let without_positions: String = problem
                .message
                .chars()
                .filter(|c| !c.is_ascii_digit())
                .collect();
            format!("{:?}:{without_positions}", problem.keys)
        })
        .collect()
}

/// Refuse a write (`act`) that would ADD a `[server.auth]` problem to the
/// file, `before` being the file's text as the user left it (`None` for a
/// file that does not exist yet): dux refuses to start with such a file, so
/// no writer may make one. A problem the user's file already has is the
/// user's to fix, not the write's: it never blocks the write (dux's own
/// change, a preference toggled in a browser, would be lost), and it is
/// logged once, worded from the user's own file so any line it names is a
/// line of that file, never of dux's patched copy.
fn check_auth_added_by(path: &Path, before: Option<&str>, contents: &str, act: &str) -> Result<()> {
    let after = auth_problem_identities(contents);
    if after.is_empty() {
        return Ok(());
    }
    let had = before.map(auth_problem_identities).unwrap_or_default();
    if after.iter().any(|problem| !had.contains(problem)) {
        // Each problem the write would add, named by the setting it is about
        // and never by a line: the result is dux's text, which was never
        // written, so a line of it is no line of the user's file.
        let reason = crate::config::auth_problems_of(contents)
            .into_iter()
            .filter(|problem| {
                let without_positions: String = problem
                    .message
                    .chars()
                    .filter(|c| !c.is_ascii_digit())
                    .collect();
                !had.contains(&format!("{:?}:{without_positions}", problem.keys))
            })
            .map(|problem| match problem.keys.first() {
                Some(keys) => format!("a problem with {}", crate::config::shown_path("", keys)),
                None => "a problem with the section".to_string(),
            })
            .collect::<Vec<_>>()
            .join("; ");
        return Err(auth_refusal(path, contents, &reason, act));
    }
    if let Some(before) = before {
        log_existing_auth_problems_once(before);
    }
    Ok(())
}

/// Log, once per process for each, the `[server.auth]` problems the user's
/// file `raw` already has, a write having gone ahead beside them.
fn log_existing_auth_problems_once(raw: &str) {
    static LOGGED: std::sync::Mutex<std::collections::BTreeSet<String>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    let Err(problem) = crate::config::auth_section_of(raw) else {
        return;
    };
    let line = format!(
        "config.toml has a [server.auth] problem dux did not make, so dux will not start with \
         it until it is fixed; the change dux saved left it as it was: {}",
        problem.reason()
    );
    // Once per problem, known as the attribution knows it: the same mistake
    // reads as itself even when the lines around it move.
    let identity = auth_problem_identities(raw)
        .into_iter()
        .collect::<Vec<_>>()
        .join("|");
    let mut logged = LOGGED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if logged.insert(identity) {
        crate::logger::warn(&line);
    }
}

/// The refusal of a write (`act`) that would leave `contents`, whose
/// `[server.auth]` cannot be read for `reason`. It names the shape that is
/// actually wrong: a `server` or a `server.auth` that is not a table is not
/// an invalid `[server.auth]` section, because there is no such section.
fn auth_refusal(path: &Path, contents: &str, reason: &str, act: &str) -> anyhow::Error {
    let file = toml::from_str::<toml::Table>(contents).ok();
    let server = file.as_ref().and_then(|file| file.get("server"));
    let auth = server
        .and_then(toml::Value::as_table)
        .and_then(|server| server.get("auth"));
    let what = match (server, auth) {
        (Some(server), _) if !server.is_table() => {
            format!("[server] in {} is not a table", path.display())
        }
        (_, Some(auth)) if !auth.is_table() => {
            format!("server.auth in {} is not a table", path.display())
        }
        _ => format!("[server.auth] in {} is invalid", path.display()),
    };
    anyhow::anyhow!(
        "after that {act}, {what} ({reason}), and dux refuses to start when it cannot read its \
         [server.auth] settings; nothing was written"
    )
}

/// [`write_config_atomic`] for a caller already holding the lock.
fn write_config_atomic_unlocked(path: &Path, contents: &str, durability: Durability) -> Result<()> {
    let target = write_target(path)?;
    let path = target.as_path();
    let dir = path
        .parent()
        .with_context(|| format!("config path {} has no parent directory", path.display()))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".config.toml.")
        .tempfile_in(dir)
        .with_context(|| format!("failed to create temp file in {}", dir.display()))?;

    // Explicit 0600 (tempfile already defaults to this; belt-and-suspenders).
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(CONFIG_FILE_MODE))
        .with_context(|| format!("failed to chmod temp file in {}", dir.display()))?;

    tmp.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write temp config in {}", dir.display()))?;

    if durability == Durability::Fsync {
        tmp.as_file()
            .sync_all()
            .with_context(|| format!("failed to fsync temp config in {}", dir.display()))?;
    }

    tmp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("failed to rename temp config over {}", path.display()))?;
    Ok(())
}

/// Where a write of the config at `path` lands. A symbolic link is written
/// through, to the file it points at (renaming over the link would replace
/// it with a plain file). A link whose target does not exist is refused,
/// naming both: nothing is created or replaced at either path, because the
/// link says the user keeps their config somewhere dux cannot see now.
fn write_target(path: &Path) -> Result<PathBuf> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(path.to_path_buf());
    };
    if !meta.file_type().is_symlink() {
        return Ok(path.to_path_buf());
    }
    if let Some(target) = crate::config::dangling_link_target(path) {
        anyhow::bail!(
            "{} is a symbolic link to {}, which does not exist; dux writes only through the \
             link and will not create or replace either, so nothing was written. Restore {} or \
             point the link at your config file.",
            path.display(),
            target.display(),
            target.display()
        );
    }
    fs::canonicalize(path).with_context(|| format!("failed to resolve {}", path.display()))
}

/// Atomic write at the default (Fsync) durability. Kept for existing callers.
///
/// # Migration lock
///
/// This function is intentionally `#[deprecated]` so that any new unrouted caller
/// fails `cargo clippy --all-targets --all-features -- -D warnings`. This is a
/// regression guard: all runtime config writes must go through `ConfigWriteQueue`.
/// Legitimate sync-direct callers (boot, first-creation, `config regenerate`,
/// recover, bootstrap project-sync) silence the lint with `#[allow(deprecated)]`
/// and a short comment explaining why direct write is correct there.
#[deprecated(
    note = "route config writes through ConfigWriteQueue; sync-direct callers must #[allow(deprecated)]"
)]
pub fn write_config_secure(path: &Path, contents: &str) -> Result<()> {
    write_config_atomic(path, contents, Durability::Fsync)
}

use crate::config::{Config, MacrosConfig, ProjectConfig, ProvidersConfig};

/// Patch an EXISTING `config.toml` in place, preserving the user's comments,
/// formatting, and any keys this writer doesn't manage. Reads the file, applies
/// every section patch, and writes it back atomically at [`Durability::Fsync`].
#[deprecated(
    note = "route config writes through ConfigWriteQueue; sync-direct callers must #[allow(deprecated)]"
)]
pub fn patch_config_file(config_path: &Path, config: &Config) -> Result<()> {
    patch_config_file_with(config_path, config, Durability::Fsync)
}

pub fn patch_config_file_with(
    config_path: &Path,
    config: &Config,
    durability: Durability,
) -> Result<()> {
    patch_config_file_three_way(config_path, None, config, durability).map(|_| ())
}

/// What a three-way save compares memory with.
///
/// `config` is the config the file last agreed with: the config as read
/// (parsed from the text, as written, with no load corrections) or, after a
/// save, the config that save wrote. `seen` is the file's text as dux has seen
/// it since it last read it: the text it read, plus every key it has written
/// since. It answers one question: was this setting ever in the file? A
/// setting that was, and is gone now, was deleted by hand.
#[derive(Clone, Copy)]
pub struct SaveBase<'a> {
    pub config: &'a Config,
    pub seen: Option<&'a str>,
}

impl<'a> SaveBase<'a> {
    /// The base for a config just read from a file: the text it was read from
    /// (its [`Config::source_text`]).
    pub fn read(config: &'a Config) -> Self {
        Self {
            config,
            seen: config.source_text.as_str(),
        }
    }
}

/// Patch the file from memory, three ways against `base`. Setting by setting
/// (and field by field inside a project):
///
/// - memory differs from the base config: memory's value is written, even
///   over a hand edit or a hand deletion of that same setting;
/// - memory equals the base config: the file keeps whatever it has now, so a
///   setting changed or deleted on disk stays that way across any number of
///   saves;
/// - a setting the file has never had (absent from what dux has seen of it,
///   and from the file now) that memory has is new to this version of dux:
///   it is filled in.
///
/// With no base every managed setting counts as changed (the full patch).
/// Either way comments around a written value are kept. `[server.auth]` is
/// never written from memory at all (see [`mutate_config_file`]). Reading and
/// writing happen under one [`ConfigFileLock`], and the result's
/// `[server.auth]` is checked first. Returns the text written.
pub fn patch_config_file_three_way(
    config_path: &Path,
    base: Option<SaveBase<'_>>,
    ours: &Config,
    durability: Durability,
) -> Result<String> {
    let _lock = ConfigFileLock::acquire(config_path)?;
    let raw = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut doc: DocumentMut = raw.parse().map_err(|e: toml_edit::TomlError| {
        anyhow::anyhow!(
            "failed to parse {}: {}",
            config_path.display(),
            crate::config::describe_toml_edit_error(&raw, &e)
        )
    })?;
    apply_patches_three_way(&mut doc, base, ours);
    let text = doc.to_string();
    check_auth_added_by(config_path, Some(&raw), &text, "write")?;
    write_config_atomic_unlocked(config_path, &text, durability)?;
    Ok(text)
}

/// [`save_config_with`] for a writer that knows its base: patch the file
/// three ways when it exists, write the documented template when it does
/// not. Returns the text written.
pub fn save_config_three_way(
    config_path: &Path,
    base: Option<SaveBase<'_>>,
    ours: &Config,
    durability: Durability,
) -> Result<String> {
    if config_path.exists() {
        patch_config_file_three_way(config_path, base, ours, durability)
    } else {
        let text = render_config_documented(ours);
        write_config_atomic(config_path, &text, durability)?;
        Ok(text)
    }
}

/// The source of a config right after a sync wrote its projects as
/// `written`, starting from `read` (the config's source before the sync).
/// The base is what the file is known to hold: the config as it was read
/// (as loaded, its corrections included, so a correction is never a change
/// dux made), with the projects the sync wrote. What has been seen is the
/// read text and the written one together.
pub fn source_after_sync(
    read: &crate::config::SourceText,
    written: &str,
    synced: &Config,
) -> crate::config::SourceText {
    let mut base = match read.written_base() {
        Some(base) => base.clone(),
        None => read
            .as_str()
            .and_then(|text| crate::config::config_from_text_as_loaded(text).ok())
            .unwrap_or_else(|| synced.clone()),
    };
    base.projects = synced.projects.clone();
    // What the read text expressed, its carried-over keys included.
    let read_seen = read
        .as_str()
        .map(|text| seen_of_read(text, &synced.projects));
    crate::config::SourceText::written(
        &union_seen(read_seen.as_deref(), written, &synced.projects),
        base,
    )
}

/// What the writer has seen of the file after writing `written` on top of
/// what it had seen (`seen`): every key and project entry of either. Keeps a
/// setting it filled in, and later found deleted, deleted.
///
/// The result is rendered with every table in its natural place, so it
/// always parses: tables taken from two documents keep the positions they
/// had in their own, and rendering those together can put an array's
/// nested header before its parent's. A `seen` that does not parse (which
/// this never writes) is kept as it is, so every later save keeps treating
/// every setting as seen rather than as new (see
/// [`apply_patches_three_way`]).
///
/// A `[[projects]]` entry neither memory (`projects`) nor the written file
/// has any more is forgotten: no decision needs it, and keeping it would let
/// what has been seen grow with every project a session adds and removes.
/// What dux has seen of a file it just read, `text`: every key the file
/// sets, and every key a load migration writes from one of them (a
/// deprecated `[server] bind` seen as `server.host` and `server.port`, for
/// instance). A value carried over from a deprecated key came from the file,
/// so its new key counts as one the file has expressed: when the deprecated
/// key is later deleted by hand, the new keys count as deleted with it, and
/// a save never fills them in. Only a key the file has never expressed in
/// any form is filled with its default.
pub fn seen_of_read(text: &str, projects: &[ProjectConfig]) -> String {
    let Ok(mut migrated) = text.parse::<DocumentMut>() else {
        return text.to_string();
    };
    if crate::config_migrate::apply_load_migrations(&mut migrated).is_err() {
        return text.to_string();
    }
    union_seen(Some(text), &migrated.to_string(), projects)
}

pub fn union_seen(seen: Option<&str>, written: &str, projects: &[ProjectConfig]) -> String {
    let Some(seen) = seen else {
        return written.to_string();
    };
    let Ok(mut seen_doc) = seen.parse::<DocumentMut>() else {
        crate::logger::warn(
            "what dux has seen of config.toml could not be read back; settings missing from the \
             file are left out rather than filled in",
        );
        return seen.to_string();
    };
    let Ok(written_doc) = written.parse::<DocumentMut>() else {
        return seen.to_string();
    };
    union_tables(seen_doc.as_table_mut(), written_doc.as_table(), true);
    forget_gone_projects(&mut seen_doc, &written_doc, projects);
    clear_positions(seen_doc.as_table_mut());
    seen_doc.to_string()
}

/// Drop the seen `[[projects]]` entries that are in neither `projects`
/// (memory) nor `written`.
fn forget_gone_projects(seen: &mut DocumentMut, written: &DocumentMut, projects: &[ProjectConfig]) {
    let Some(Item::ArrayOfTables(seen_projects)) = seen.get_mut("projects") else {
        return;
    };
    let written_entries: Vec<&Table> = written
        .get("projects")
        .and_then(Item::as_array_of_tables)
        .map(|entries| entries.iter().collect())
        .unwrap_or_default();
    let field =
        |entry: &Table, name: &str| entry.get(name).and_then(Item::as_str).map(str::to_string);
    seen_projects.retain(|entry| {
        let in_file = written_entries
            .iter()
            .any(|written| (0..MATCH_TIERS).any(|tier| same_entry_at(tier, entry, written)));
        let in_memory = projects.iter().any(|project| {
            field(entry, "id").as_deref() == Some(project.id.as_str())
                || field(entry, "path").as_deref() == Some(project.path.as_str())
        });
        in_file || in_memory
    });
}

/// Forget where each table sat in the document it came from, so the whole
/// renders in its natural order: every header after its parent's.
fn clear_positions(table: &mut Table) {
    table.set_position(None);
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(child) => clear_positions(child),
            Item::ArrayOfTables(array) => {
                for entry in array.iter_mut() {
                    clear_positions(entry);
                }
            }
            _ => {}
        }
    }
}

/// `root` when `seen` is the file itself (see [`merges_entries`]).
fn union_tables(seen: &mut Table, written: &Table, root: bool) {
    for (key, item) in written.iter() {
        match seen.get_mut(key) {
            None => {
                seen.insert(key, item.clone());
            }
            Some(seen_item) => union_items(seen_item, item, merges_entries(key, root)),
        }
    }
}

/// Fold `written` into `seen` at one key, everywhere a key can live: inside
/// a table in either form (a subtable, an inline table, one nested inline in
/// another), and inside each entry of an array of tables matched to its
/// written entry by the same assignment the merge uses, for `[[projects]]`
/// (`by_entry`). Any other array of tables is one value to the merge, so it
/// is seen as the one written; nothing is ever appended to it, because an
/// entry with nothing to identify it by would be appended on every save. A
/// value is already seen; its text does not matter here.
fn union_items(seen: &mut Item, written: &Item, by_entry: bool) {
    if let (Item::ArrayOfTables(seen_array), Item::ArrayOfTables(written_array)) =
        (&mut *seen, written)
    {
        if by_entry {
            union_arrays(seen_array, written_array);
        } else {
            *seen_array = written_array.clone();
        }
        return;
    }
    let Some(written_table) = table_like(written) else {
        return;
    };
    match seen {
        Item::Table(seen_table) => union_tables(seen_table, &written_table, false),
        Item::Value(Value::InlineTable(inline)) => {
            let mut table = inline.clone().into_table();
            union_tables(&mut table, &written_table, false);
            let decor = inline.decor().clone();
            *inline = table.into_inline_table();
            *inline.decor_mut() = decor;
        }
        _ => {}
    }
}

/// Each written entry folded into the seen entry it is (see
/// [`assign_entries`]), and one with no seen entry added.
fn union_arrays(seen: &mut toml_edit::ArrayOfTables, written: &toml_edit::ArrayOfTables) {
    let written_entries: Vec<&Table> = written.iter().collect();
    let matched = {
        let seen_entries: Vec<&Table> = seen.iter().collect();
        let mut used = vec![false; seen_entries.len()];
        assign_entries(
            &written_entries
                .iter()
                .map(|entry| vec![*entry])
                .collect::<Vec<_>>(),
            &seen_entries,
            &mut used,
        )
    };
    for (entry, matched) in written_entries.into_iter().zip(matched) {
        match matched.and_then(|index| seen.get_mut(index)) {
            Some(seen_entry) => union_tables(seen_entry, entry, false),
            None => seen.push(entry.clone()),
        }
    }
}

fn apply_patches_three_way(disk: &mut DocumentMut, base: Option<SaveBase<'_>>, ours: &Config) {
    // A save removes the retired keys (below). Removing one that still
    // carries a value and writing that value under the key that replaced it
    // is ONE rewrite, made on the file as it is now, before anything is
    // merged: the replacement lands unless the file already sets it, and a
    // key the user deleted by hand is not there to carry anything.
    for (section, key) in RETIRED_KEYS {
        crate::config_migrate::carry_over_deprecated_key(disk, section, key);
    }
    let original = disk.clone();
    let mut with_ours = original.clone();
    apply_patches(&mut with_ours, ours);
    match base {
        Some(base) => {
            let mut with_base = original.clone();
            apply_patches(&mut with_base, base.config);
            // What dux has seen of the file. One that cannot be read back
            // fails SAFE: every setting dux writes counts as seen, so a
            // setting missing from the file stays out rather than being
            // filled in over a deletion nobody can now tell from a new key.
            let seen = match base.seen.map(str::parse::<DocumentMut>) {
                Some(Ok(seen)) => Some(seen),
                Some(Err(_)) => {
                    crate::logger::warn(
                        "what dux has seen of config.toml could not be read back; this save \
                         treats every setting as seen and fills nothing in",
                    );
                    Some(with_base.clone())
                }
                None => None,
            };
            merge_changed_at(
                disk.as_table_mut(),
                original.as_table(),
                Some(MergeBase {
                    raw: seen.as_ref().map(DocumentMut::as_table),
                    base: with_base.as_table(),
                }),
                with_ours.as_table(),
                true,
            );
        }
        None => merge_changed_at(
            disk.as_table_mut(),
            original.as_table(),
            None,
            with_ours.as_table(),
            true,
        ),
    }
    if let Some(base) = base {
        note_macro_order_limit(disk, base.config, ours);
    }
    // Retired keys still go on every save, as they always have.
    for (section, key) in RETIRED_KEYS {
        remove_table_key(disk, section, key);
    }
}

/// Say so when memory reordered its macros and the file cannot read back in
/// that order. A macro's written form is never changed, and TOML prints a
/// table's inline entries before its `[macros.<name>]` sections, so the order
/// set in dux is kept within each form (see [`merge_changed_at`]) and a
/// section can never read back ahead of an inline macro. What the file reads
/// back is taken from the file itself, parsed, never guessed from how it is
/// stored.
fn note_macro_order_limit(doc: &DocumentMut, base: &Config, ours: &Config) {
    let shared = |config: &Config, other: &Config| -> Vec<String> {
        config
            .macros
            .entries
            .keys()
            .filter(|name| other.macros.entries.contains_key(*name))
            .cloned()
            .collect()
    };
    if shared(base, ours) == shared(ours, base) {
        return;
    }
    let Ok(written) = crate::config::config_from_text_as_written(&doc.to_string()) else {
        return;
    };
    let read_back: Vec<&String> = written
        .macros
        .entries
        .keys()
        .filter(|name| ours.macros.entries.contains_key(*name))
        .collect();
    let wanted: Vec<&String> = ours
        .macros
        .entries
        .keys()
        .filter(|name| written.macros.entries.contains_key(*name))
        .collect();
    if read_back != wanted {
        crate::logger::info(
            "config.toml: the macro order set in dux is kept within each form, but macros \
             written as [macros.<name>] sections always read back after the inline ones; write \
             them all in one form to keep any order",
        );
    }
}

/// Whether `[section] key` is a key dux once wrote and every save removes.
/// The formatter's schema knows these names.
pub(crate) fn is_retired_key(section: &str, key: &str) -> bool {
    RETIRED_KEYS
        .iter()
        .any(|(retired_section, retired_key)| *retired_section == section && *retired_key == key)
}

/// Keys dux once wrote and every save removes (see `apply_patches`).
const RETIRED_KEYS: &[(&str, &str)] = &[
    ("defaults", "commit_prompt"),
    ("defaults", "prompt_for_name"),
    ("server", "tailscale_enabled"),
    ("server", "max_websocket_connections"),
];

/// The base side of a merge at one table: what dux has seen of the file
/// there (`raw`, absent where it has seen none) and the file patched with
/// the base config (`base`).
#[derive(Clone, Copy)]
struct MergeBase<'a> {
    raw: Option<&'a Table>,
    base: &'a Table,
}

/// Copy into `target` (the file) what memory changed relative to the base,
/// recursing into tables (see [`patch_config_file_three_way`] for the
/// rules). `disk` is the file before this save. With no base, everything
/// in `ours` counts as changed.
fn merge_changed(target: &mut Table, disk: &Table, base: Option<MergeBase<'_>>, ours: &Table) {
    merge_changed_at(target, disk, base, ours, false);
}

/// Whether the array of tables at `key` is one dux merges entry by entry:
/// `[[projects]]` at the top of the file, the one array dux writes and whose
/// entries it can identify (by `id` and `path`). Every other array of
/// tables (a hand-added `[[extra]]`, one nested in a section, one under a
/// provider, a newer dux's) is one value: memory's when memory changed it,
/// the file's otherwise, so an entry with nothing to identify it by is never
/// written twice.
fn merges_entries(key: &str, root: bool) -> bool {
    root && key == "projects"
}

/// [`merge_changed`] at one table, `root` when it is the file itself.
fn merge_changed_at(
    target: &mut Table,
    disk: &Table,
    base: Option<MergeBase<'_>>,
    ours: &Table,
    root: bool,
) {
    for (key, ours_item) in ours.iter() {
        let Some(base_side) = base else {
            // No base: the full patch, through the same in-place writes.
            match (ours_item, disk.get(key), target.get_mut(key)) {
                (Item::Table(ours_table), Some(Item::Table(disk_table)), Some(Item::Table(t))) => {
                    merge_changed(t, disk_table, None, ours_table)
                }
                (Item::ArrayOfTables(ours_array), _, Some(Item::ArrayOfTables(t))) => {
                    *t = ours_array.clone();
                }
                _ => put_in_place(target, key, ours_item.clone()),
            }
            continue;
        };
        let base_item = base_side.base.get(key);
        let raw_item = base_side.raw.and_then(|raw| raw.get(key));
        let unchanged = base_item.is_some_and(|b| item_text(b) == item_text(ours_item));
        // A whole table or array missing from the file is merged as an empty
        // one, so each setting (or project) in it gets the same rule as a
        // single deleted setting, and it reappears only if something in it is
        // written.
        if disk.get(key).is_none() {
            match (ours_item, base_item) {
                (Item::Table(ours_table), Some(Item::Table(base_table))) => {
                    // What dux has seen there, in either form: an inline
                    // section deleted by hand was seen, so what it held
                    // stays deleted.
                    let raw_table = raw_item.and_then(table_like);
                    let empty = Table::new();
                    let mut out = Table::new();
                    merge_changed(
                        &mut out,
                        &empty,
                        Some(MergeBase {
                            raw: raw_table.as_ref(),
                            base: base_table,
                        }),
                        ours_table,
                    );
                    if !out.is_empty() {
                        put_in_place(target, key, Item::Table(out));
                    }
                    continue;
                }
                (ours_item, Some(base_item))
                    if is_inline_table(ours_item) && table_like(base_item).is_some() =>
                {
                    let (Some(ours_table), Some(base_table)) =
                        (table_like(ours_item), table_like(base_item))
                    else {
                        continue;
                    };
                    let raw_table = raw_item.and_then(table_like);
                    let empty = Table::new();
                    let mut out = Table::new();
                    merge_changed(
                        &mut out,
                        &empty,
                        Some(MergeBase {
                            raw: raw_table.as_ref(),
                            base: &base_table,
                        }),
                        &ours_table,
                    );
                    if !out.is_empty() {
                        put_in_place(target, key, toml_edit::value(out.into_inline_table()));
                    }
                    continue;
                }
                (Item::ArrayOfTables(ours_array), Some(Item::ArrayOfTables(base_array)))
                    if merges_entries(key, root) =>
                {
                    let empty = toml_edit::ArrayOfTables::new();
                    let mut out = toml_edit::ArrayOfTables::new();
                    merge_array_of_tables(
                        &mut out,
                        &empty,
                        raw_item.and_then(Item::as_array_of_tables),
                        Some(base_array),
                        ours_array,
                    );
                    if !out.is_empty() {
                        put_in_place(target, key, Item::ArrayOfTables(out));
                    }
                    continue;
                }
                _ => {}
            }
        }
        let Some(disk_item) = disk.get(key) else {
            // Missing from the file. Memory's own change is written; an
            // unchanged setting is filled in only when the file has never had
            // it (new to this version), and otherwise stays deleted, because
            // someone deleted it by hand.
            if !unchanged || raw_item.is_none() {
                put_in_place(target, key, ours_item.clone());
            }
            continue;
        };
        match (ours_item, disk_item, target.get_mut(key)) {
            (Item::Table(ours_table), Item::Table(disk_table), Some(Item::Table(t)))
                if base_item.is_some_and(Item::is_table) =>
            {
                // What dux has seen there, in either form: a section the user
                // rewrote from inline to a table was seen with every key it
                // held, so one left out in the rewrite stays out.
                let raw_table = raw_item.and_then(table_like);
                let child = base_item.and_then(Item::as_table).map(|base| MergeBase {
                    raw: raw_table.as_ref(),
                    base,
                });
                merge_changed(t, disk_table, child, ours_table);
            }
            // An array the base never had (memory and the file each started
            // one) is merged against an empty one, so the file's own entries
            // stay beside memory's.
            (
                Item::ArrayOfTables(ours_array),
                Item::ArrayOfTables(disk_array),
                Some(Item::ArrayOfTables(t)),
            ) if merges_entries(key, root) && base_item.is_none_or(Item::is_array_of_tables) => {
                let empty = toml_edit::ArrayOfTables::new();
                merge_array_of_tables(
                    t,
                    disk_array,
                    raw_item.and_then(Item::as_array_of_tables),
                    Some(
                        base_item
                            .and_then(Item::as_array_of_tables)
                            .unwrap_or(&empty),
                    ),
                    ours_array,
                );
            }
            // A table written inline on either side (a project's `env`, as
            // dux writes it, or as the user wrote it in either form) is
            // merged entry by entry like any other table, and keeps the
            // file's form: a `[projects.env]` subtable stays one, with its
            // comments, and an inline table stays inline. A table the base
            // never had (memory and the file each started one) is merged
            // against an empty one, so the file's own entries stay.
            (ours_item, disk_item, Some(target_item))
                if table_like(ours_item).is_some()
                    && table_like(disk_item).is_some()
                    && base_item.is_none_or(|item| table_like(item).is_some()) =>
            {
                let (Some(ours_table), Some(disk_table), Some(base_table)) = (
                    table_like(ours_item),
                    table_like(disk_item),
                    base_item.map_or(Some(Table::new()), table_like),
                ) else {
                    continue;
                };
                let raw_table = raw_item.and_then(table_like);
                let entry_base = Some(MergeBase {
                    raw: raw_table.as_ref(),
                    base: &base_table,
                });
                match target_item {
                    Item::Table(t) => merge_changed(t, &disk_table, entry_base, &ours_table),
                    Item::Value(Value::InlineTable(inline)) => {
                        let mut out = disk_table.clone();
                        merge_changed(&mut out, &disk_table, entry_base, &ours_table);
                        let decor = inline.decor().clone();
                        *inline = out.into_inline_table();
                        *inline.decor_mut() = decor;
                    }
                    _ => {}
                }
            }
            _ => {
                if !unchanged {
                    put_in_place(target, key, ours_item.clone());
                }
            }
        }
    }
    // Removed in memory (an env variable, a provider): in the base, gone
    // from ours. Something the base never had stays: it was added on disk.
    // With no base, whatever the patch left out goes, as in the full patch.
    let gone: Vec<String> = disk
        .iter()
        .map(|(key, _)| key.to_string())
        .filter(|key| {
            ours.get(key).is_none() && base.is_none_or(|base| base.base.get(key).is_some())
        })
        .collect();
    for key in gone {
        // A whole array of tables memory emptied (its last project removed)
        // is merged entry by entry against an empty one, so an entry added on
        // disk by someone else stays; only the base's entries go.
        if let (Some(base_side), Some(Item::ArrayOfTables(disk_array))) = (base, disk.get(&key))
            && merges_entries(&key, root)
            && let Some(Item::ArrayOfTables(base_array)) = base_side.base.get(&key)
        {
            let raw_array = base_side
                .raw
                .and_then(|raw| raw.get(&key))
                .and_then(Item::as_array_of_tables);
            let mut out = toml_edit::ArrayOfTables::new();
            merge_array_of_tables(
                &mut out,
                disk_array,
                raw_array,
                Some(base_array),
                &toml_edit::ArrayOfTables::new(),
            );
            if !out.is_empty() {
                put_in_place(target, &key, Item::ArrayOfTables(out));
                continue;
            }
        }
        target.remove(&key);
    }
    // Order is meaningful in some tables (`[macros]` above all), so a pure
    // reorder in memory is a change: the keys ours shares with the file take
    // ours' order, and anything added on disk keeps its place after them.
    let reference: Vec<String> = match base {
        Some(base) => base.base.iter().map(|(key, _)| key.to_string()).collect(),
        None => disk.iter().map(|(key, _)| key.to_string()).collect(),
    };
    let ours_order: Vec<String> = ours.iter().map(|(key, _)| key.to_string()).collect();
    let shared = |order: &[String]| -> Vec<String> {
        order
            .iter()
            .filter(|key| ours_order.contains(key) && reference.contains(key))
            .cloned()
            .collect()
    };
    if shared(&reference) != shared(&ours_order) {
        let rank = |key: &str| {
            ours_order
                .iter()
                .position(|k| k == key)
                .unwrap_or(ours_order.len())
        };
        target.sort_values_by(|a, _, b, _| rank(a.get()).cmp(&rank(b.get())));
        // Child sections (`[macros.<name>]`) print where their position puts
        // them, not in the table's key order: they take ours' order by
        // trading the positions they already hold, so they stay in the same
        // slots of the file and keep their form and every comment.
        let mut sections: Vec<(String, isize)> = target
            .iter()
            .filter_map(|(key, item)| match item {
                Item::Table(table) if !table.is_dotted() => {
                    table.position().map(|position| (key.to_string(), position))
                }
                _ => None,
            })
            .collect();
        if sections.len() > 1 {
            let mut positions: Vec<isize> =
                sections.iter().map(|(_, position)| *position).collect();
            positions.sort_unstable();
            sections.sort_by_key(|(key, _)| rank(key));
            for ((key, _), position) in sections.iter().zip(positions) {
                if let Some(Item::Table(table)) = target.get_mut(key) {
                    table.set_position(Some(position));
                }
            }
        }
    }
}

/// Whether `item` is a table written inline (`env = { A = "1" }`).
fn is_inline_table(item: &Item) -> bool {
    matches!(item, Item::Value(Value::InlineTable(_)))
}

/// `item` as a table, whether it is written as one or inline.
fn table_like(item: &Item) -> Option<Table> {
    match item {
        Item::Table(table) => Some(table.clone()),
        Item::Value(Value::InlineTable(inline)) => Some(inline.clone().into_table()),
        _ => None,
    }
}

/// Write `item` at `key`, keeping the key itself (and so the comment above
/// it) and the comment trailing the old value. `Table::insert` on an existing
/// key would replace the key and drop its comment.
fn put_in_place(target: &mut Table, key: &str, mut item: Item) {
    match target.get_mut(key) {
        Some(existing) => {
            match (&*existing, &mut item) {
                (Item::Value(old), Item::Value(new)) => *new.decor_mut() = old.decor().clone(),
                (Item::Table(old), Item::Table(new)) => *new.decor_mut() = old.decor().clone(),
                _ => {}
            }
            *existing = item;
        }
        None => {
            target.insert(key, item);
        }
    }
}

/// How strongly two entries are the same entry, strongest first: the same
/// `id` AND the same `path`; the same `id`; the same `path`. The exact pair
/// comes first so a copy-pasted entry with the original's id but its own
/// path never takes the original's place. Falling back past a differing
/// `id` is deliberate: a hand-written project has no id of its own, so each
/// read mints a fresh one, and an id adopted from the session database
/// replaces the file's for the same path. Never by `name`: two different
/// projects can share one (two folders both called `api`), and matching them
/// would drop one or give it the other's id.
const MATCH_TIERS: usize = 3;

fn same_entry_at(tier: usize, a: &Table, b: &Table) -> bool {
    let field = |table: &Table, name: &str| table.get(name).map(item_text);
    let same =
        |name: &str| matches!((field(a, name), field(b, name)), (Some(x), Some(y)) if x == y);
    match tier {
        0 => same("id") && same("path"),
        1 => same("id"),
        _ => same("path"),
    }
}

/// Match each of `probes` (an entry, through any of the tables standing for
/// it, in order of preference) to at most one entry of `list` not yet
/// `used`, as ONE assignment: every match at a stronger tier is made, across
/// all the entries, before any at a weaker one, so a weak match (the same
/// path) never takes an entry a strong one (the same id) wants, whatever
/// order the entries come in. Marks what it matched as used.
fn assign_entries(
    probes: &[Vec<&Table>],
    list: &[&Table],
    used: &mut [bool],
) -> Vec<Option<usize>> {
    let mut matched = vec![None; probes.len()];
    for tier in 0..MATCH_TIERS {
        // Within a tier, the closest candidates are paired first, so two
        // entries that share a path (two id-less projects at one folder) are
        // told apart by what else they hold before their position decides.
        for closeness in (0..=CLOSEST).rev() {
            for (slot, tables) in probes.iter().enumerate() {
                if matched[slot].is_some() {
                    continue;
                }
                let found = tables.iter().find_map(|probe| {
                    (0..list.len()).find(|&i| {
                        !used[i]
                            && same_entry_at(tier, list[i], probe)
                            && entry_closeness(list[i], probe) >= closeness
                    })
                });
                if let Some(i) = found {
                    used[i] = true;
                    matched[slot] = Some(i);
                }
            }
        }
    }
    matched
}

/// The highest [`entry_closeness`].
const CLOSEST: u8 = 2;

/// How much more than their identity two entries share: everything but the
/// `id` (each parse mints one for an id-less entry), then the same `name`,
/// then nothing.
fn entry_closeness(a: &Table, b: &Table) -> u8 {
    if entry_text(a, true) == entry_text(b, true) {
        return CLOSEST;
    }
    let name = |table: &Table| table.get("name").map(item_text);
    match (name(a), name(b)) {
        (Some(x), Some(y)) if x == y => 1,
        _ => 0,
    }
}

/// `entry` with only the keys dux writes for a project (see
/// [`merge_array_of_tables`]).
fn managed_keys(entry: &Table) -> Table {
    let mut managed = entry.clone();
    let unmanaged: Vec<String> = managed
        .iter()
        .map(|(key, _)| key.to_string())
        .filter(|key| is_unmanaged_project_key(key))
        .collect();
    for key in unmanaged {
        managed.remove(&key);
    }
    managed
}

/// An entry's text for comparing base and memory. When the file's own entry
/// has no `id`, the id is left out: each parse mints one, so it says nothing
/// about whether memory changed the entry.
fn entry_text(entry: &Table, ignore_id: bool) -> String {
    let mut entry = entry.clone();
    if ignore_id {
        entry.remove("id");
    }
    entry.to_string()
}

/// `[[projects]]` and any other array of tables, merged entry by entry with
/// the same rules as single settings (see [`patch_config_file_three_way`]):
/// entries are matched across memory, the base and the file with
/// [`find_entry`], each used at most once. A matched entry is merged field
/// by field against its own base entry; an entry memory added is written; one
/// memory removed goes; one deleted by hand that memory did not change stays
/// deleted; one added on disk is kept after memory's entries. When memory
/// has an `id` the file's entry lacks, the id is written, so a hand-written
/// project settles on one id.
fn merge_array_of_tables(
    target: &mut toml_edit::ArrayOfTables,
    disk: &toml_edit::ArrayOfTables,
    raw: Option<&toml_edit::ArrayOfTables>,
    base: Option<&toml_edit::ArrayOfTables>,
    ours: &toml_edit::ArrayOfTables,
) {
    let Some(base) = base else {
        *target = ours.clone();
        return;
    };
    // Memory and the base are compared on the keys dux manages only. The
    // user's own keys in an entry (a note, a fork's setting) are carried into
    // memory's rendering of the file by a guess that cannot tell two id-less
    // entries at one path apart once one is renamed, so they have no say
    // here: the file's own entry keeps them, whatever the guess was.
    let ours_full: Vec<&Table> = ours.iter().collect();
    let ours_managed: Vec<Table> = ours_full.iter().map(|entry| managed_keys(entry)).collect();
    let bases_managed: Vec<Table> = base.iter().map(managed_keys).collect();
    let disks: Vec<&Table> = disk.iter().collect();
    let bases: Vec<&Table> = bases_managed.iter().collect();
    let raws: Vec<&Table> = raw.map(|raw| raw.iter().collect()).unwrap_or_default();
    let ours: Vec<&Table> = ours_managed.iter().collect();
    // Every side is matched on the keys dux manages too, so how alike two
    // entries are never depends on keys only the file has: two id-less
    // projects at one path are told apart by their own settings, and the
    // user's keys go with whichever file entry is matched.
    let raws_managed: Vec<Table> = raws.iter().map(|entry| managed_keys(entry)).collect();
    let disks_managed: Vec<Table> = disks.iter().map(|entry| managed_keys(entry)).collect();
    let raws_matched: Vec<&Table> = raws_managed.iter().collect();
    let disks_matched: Vec<&Table> = disks_managed.iter().collect();
    let mut disk_used = vec![false; disks.len()];
    let mut base_used = vec![false; bases.len()];
    let mut raw_used = vec![false; raws.len()];
    // Memory's entries against the base, then against what dux has seen
    // (through their base entry first), then against the file, through what
    // dux has seen of the entry first: that is the file's own entry as it
    // was, the user's keys included, so it finds it even when memory renamed
    // the project to the name another entry at the same path has.
    let base_of = assign_entries(
        &ours.iter().map(|entry| vec![*entry]).collect::<Vec<_>>(),
        &bases,
        &mut base_used,
    );
    let raw_of = assign_entries(
        &ours
            .iter()
            .zip(&base_of)
            .map(|(entry, b)| b.map(|b| bases[b]).into_iter().chain([*entry]).collect())
            .collect::<Vec<_>>(),
        &raws_matched,
        &mut raw_used,
    );
    let disk_of = assign_entries(
        &ours
            .iter()
            .zip(base_of.iter().zip(&raw_of))
            .map(|(entry, (b, r))| {
                r.map(|r| raws_matched[r])
                    .into_iter()
                    .chain(b.map(|b| bases[b]))
                    .chain(std::iter::once(*entry))
                    .collect()
            })
            .collect::<Vec<_>>(),
        &disks_matched,
        &mut disk_used,
    );
    let mut merged = toml_edit::ArrayOfTables::new();
    for (slot, entry) in ours.iter().copied().enumerate() {
        let base_index = base_of[slot];
        let raw_entry = raw_of[slot].map(|r| raws[r]);
        let disk_index = disk_of[slot];
        let ignore_id = raw_entry.is_some_and(|r| !r.contains_key("id"))
            || disk_index.is_some_and(|d| !disks[d].contains_key("id"));
        let unchanged = base_index
            .is_some_and(|b| entry_text(bases[b], ignore_id) == entry_text(entry, ignore_id));
        let Some(d) = disk_index else {
            // Not in the file: deleted by hand when the file once had it and
            // memory did not change it; otherwise memory's entry is written.
            if !(unchanged && raw_entry.is_some()) {
                merged.push(ours_full[slot].clone());
            }
            continue;
        };
        let mut out = disks[d].clone();
        let entry_base = base_index.map(|b| MergeBase {
            raw: raw_entry,
            base: bases[b],
        });
        merge_changed(&mut out, disks[d], entry_base, entry);
        // Keys every write drops from a project (its `leading_branch`) go
        // from the file's entry too.
        for key in PROJECT_KEYS_DROPPED_ON_WRITE {
            if !entry.contains_key(key) {
                out.remove(key);
            }
        }
        if !out.contains_key("id")
            && let Some(id) = entry.get("id")
        {
            out.insert("id", id.clone());
        }
        merged.push(out);
    }
    // In the file but matched by nothing memory has: removed in memory when
    // the base CONFIG had it (and memory no longer does), added on disk by
    // someone else otherwise. Only the base config answers this: what dux
    // has seen of the file says nothing about what memory removed, and
    // asking it would drop an entry added by hand on the save after it was
    // first written, or one dux removed and the user put back.
    // Each base entry answers for one file entry at most, matched as one
    // assignment: a file entry with the base entry's id takes it before
    // another that only shares its path (moved by hand, then a new one
    // added at the old path), in whichever order the file lists them.
    let leftovers: Vec<usize> = (0..disks.len()).filter(|&d| !disk_used[d]).collect();
    let removed = assign_entries(
        &leftovers
            .iter()
            .map(|&d| vec![disks_matched[d]])
            .collect::<Vec<_>>(),
        &bases,
        &mut base_used,
    );
    for (&d, removed) in leftovers.iter().zip(removed) {
        if removed.is_none() {
            merged.push(disks[d].clone());
        }
    }
    *target = merged;
}

/// An item's TOML text without the comments and spacing around it.
fn item_text(item: &Item) -> String {
    match item {
        Item::Value(value) => {
            let mut value = value.clone();
            value.decor_mut().clear();
            value.to_string()
        }
        other => other.to_string(),
    }
}

/// Replace the whole file with what `render` makes of its current text
/// (`None` when there is no file), reading that text only once the
/// [`ConfigFileLock`] is held, so a writer that finishes while this one waits
/// is part of what `render` sees. For the whole-file writers that depend on
/// what the file holds now: recovering the last working config, restoring the
/// documentation. The result's `[server.auth]` is checked before it lands.
pub fn replace_config_file<T>(
    config_path: &Path,
    render: impl FnOnce(Option<&str>) -> Result<(String, T)>,
) -> Result<T> {
    let _lock = ConfigFileLock::acquire(config_path)?;
    let current = match fs::read_to_string(config_path) {
        Ok(raw) => Some(raw),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", config_path.display()));
        }
    };
    let (text, outcome) = render(current.as_deref())?;
    check_auth_added_by(config_path, current.as_deref(), &text, "write")?;
    write_config_atomic_unlocked(config_path, &text, Durability::Fsync)?;
    Ok(outcome)
}

/// Read the config, decide, and write only when `decide` asks to, all under
/// the [`ConfigFileLock`], so a writer that lands while this one waits is
/// part of what `decide` reads and is never overwritten by a stale copy.
/// `decide` gets the read's own result (a missing or unreadable file is its
/// to judge) and returns the text to write, if any, with its outcome. A
/// text to write is checked for a readable `[server.auth]` first. For a
/// load-time migration, which rewrites keys of whatever the file holds.
pub fn migrate_config_file<T>(
    config_path: &Path,
    decide: impl FnOnce(std::io::Result<String>) -> Result<(Option<String>, T)>,
) -> Result<T> {
    let _lock = ConfigFileLock::acquire(config_path)?;
    let read = fs::read_to_string(config_path);
    let before = read.as_ref().ok().cloned();
    let (text, outcome) = decide(read)?;
    if let Some(text) = text {
        check_auth_added_by(config_path, before.as_deref(), &text, "write")?;
        write_config_atomic_unlocked(config_path, &text, Durability::Fsync)
            .with_context(|| format!("failed to write {}", config_path.display()))?;
    }
    Ok(outcome)
}

/// Write a private (0600) file beside the config while the caller holds the
/// [`ConfigFileLock`] inside [`replace_config_file`]'s `render`: a backup of
/// the file being replaced. Taking the lock again there would wait forever.
pub fn write_beside_config_locked(path: &Path, contents: &str) -> Result<()> {
    write_config_atomic_unlocked(path, contents, Durability::Fsync)
}

/// The ONE way to change specific keys of `config.toml` while anything else
/// may be writing it: take the [`ConfigFileLock`], re-read the file as it is
/// on disk NOW, let `change` edit only the keys it means to, check the result,
/// and write it atomically (fsync'd) before releasing the lock. Comments,
/// formatting and every other key stay exactly as the file had them.
///
/// `dux config set`, password changes and the login's ban appends all go
/// through here, so concurrent writers never lose each other's updates.
///
/// A missing file starts from the documented default template. A file that is
/// not TOML is refused (there is nothing safe to patch), and so is a result
/// whose `[server.auth]` would not load, because that would stop dux from
/// starting; in both cases nothing is written.
pub fn mutate_config_file<T>(
    config_path: &Path,
    change: impl FnOnce(&mut DocumentMut) -> Result<T>,
) -> Result<T> {
    mutate_config_file_with(config_path, MissingConfig::CreateDocumented, change)
}

/// What [`mutate_config_file_with`] does when there is no config file.
pub enum MissingConfig<'a> {
    /// Start from the documented default template.
    CreateDocumented,
    /// Run this check first, inside the config write lock, and start from the
    /// template only when it passes. `dux config set` refuses here while a
    /// dux is running: a fresh default file would drop its password.
    CheckFirst(Box<dyn Fn() -> Result<()> + 'a>),
}

/// [`mutate_config_file`] with a choice of what a missing file means.
pub fn mutate_config_file_with<T>(
    config_path: &Path,
    missing: MissingConfig<'_>,
    change: impl FnOnce(&mut DocumentMut) -> Result<T>,
) -> Result<T> {
    mutate_config_file_ruled(config_path, missing, AuthRule::Valid, change)
        .map(|(outcome, _)| outcome)
}

/// [`mutate_config_file_with`] for a change that may leave problems the file
/// ALREADY has, so a file with several can be repaired one at a time. A
/// problem is anything that stops dux starting with the file
/// ([`crate::config::start_problems_of`]: `[server.auth]` key by key, then
/// the start checks), so a change that adds one is refused with the start's
/// own words, and nothing is written. Returns the problems left, each with
/// the surfaces it stops.
pub fn mutate_config_file_repairing<T>(
    config_path: &Path,
    missing: MissingConfig<'_>,
    key: &[String],
    change: impl FnOnce(&mut DocumentMut) -> Result<T>,
) -> Result<(T, Vec<crate::config::StartProblem>)> {
    mutate_config_file_ruled(
        config_path,
        missing,
        AuthRule::NothingAddedBy(key.to_vec()),
        change,
    )
}

/// What a locked change must leave of `[server.auth]`.
enum AuthRule {
    /// A section that loads.
    Valid,
    /// Nothing that stops a start which the change of this setting is
    /// answerable for (see [`crate::config::problems_added_by_set`]).
    NothingAddedBy(Vec<String>),
}

fn mutate_config_file_ruled<T>(
    config_path: &Path,
    missing: MissingConfig<'_>,
    rule: AuthRule,
    change: impl FnOnce(&mut DocumentMut) -> Result<T>,
) -> Result<(T, Vec<crate::config::StartProblem>)> {
    let _lock = ConfigFileLock::acquire(config_path)?;
    let raw = match fs::read_to_string(config_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let MissingConfig::CheckFirst(check) = missing {
                check()?;
            }
            render_config_documented(&Config::default())
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", config_path.display()));
        }
    };
    let mut doc: DocumentMut = raw.parse().map_err(|e: toml_edit::TomlError| {
        anyhow::anyhow!(
            "{} is not valid TOML, so it cannot be changed safely; fix it by hand first.\n{}",
            config_path.display(),
            crate::config::describe_toml_edit_error(&raw, &e)
        )
    })?;
    let before = match &rule {
        AuthRule::Valid => crate::config::StartCheck::default(),
        AuthRule::NothingAddedBy(_) => crate::config::check_start(&raw),
    };
    let outcome = change(&mut doc)?;
    let text = doc.to_string();
    let remaining = match rule {
        AuthRule::Valid => {
            check_auth_added_by(config_path, Some(&raw), &text, "change")?;
            Vec::new()
        }
        AuthRule::NothingAddedBy(key) => {
            // Attributed to the setting changed: its own value, and a rule
            // spanning it that held before. Problems about other settings
            // never block it; they are listed after.
            let after = crate::config::check_start(&text);
            let added: Vec<&str> = crate::config::problems_added_by_set(&before, &after, &key)
                .into_iter()
                .map(|p| p.message.as_str())
                .collect();
            if !added.is_empty() {
                // Each sentence names the surfaces it stops, never "dux" as
                // a whole.
                anyhow::bail!(
                    "{} was not changed, because with that change {}. Nothing was written.",
                    config_path.display(),
                    added.join("; ").trim_end_matches('.')
                );
            }
            after.problems
        }
    };
    write_config_atomic_unlocked(config_path, &text, Durability::Fsync)?;
    Ok((outcome, remaining))
}

/// Write the whole `[server.auth]` section from `auth`, for a render of a
/// fresh file (first creation, recovery) and nothing else. A save from
/// memory never calls this: these keys change from outside the running dux
/// (`dux config set`, a ban, a password change), so only
/// [`mutate_config_file`] changes them in an existing file.
fn render_auth_section(doc: &mut DocumentMut, auth: &crate::config::ServerAuthConfig) {
    let Some(server) = doc
        .entry("server")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
    else {
        return;
    };
    let Some(table) = server
        .entry("auth")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
    else {
        return;
    };
    let Ok(rendered) = toml::to_string(auth) else {
        return;
    };
    let Ok(rendered) = rendered.parse::<DocumentMut>() else {
        return;
    };
    for (key, item) in rendered.iter() {
        table.insert(key, item.clone());
    }
}

/// Save config: patch in place if the file exists, otherwise write a plain
/// (uncommented) `toml_edit` serialization from scratch. Used by surfaces that
/// don't have the TUI's canonical commented renderer (e.g. the web). The TUI
/// keeps its own `save_config` for the pretty first-creation path.
#[deprecated(
    note = "route config writes through ConfigWriteQueue; sync-direct callers must #[allow(deprecated)]"
)]
pub fn save_config(config_path: &Path, config: &Config) -> Result<()> {
    save_config_with(config_path, config, Durability::Fsync)
}

pub fn save_config_with(config_path: &Path, config: &Config, durability: Durability) -> Result<()> {
    if config_path.exists() {
        patch_config_file_three_way(config_path, None, config, durability).map(|_| ())
    } else {
        // FIRST CREATION. This must emit the fully-commented template, not the
        // plain one: "the config file is the documentation" (CLAUDE.md), and the
        // patch path that runs on every later save preserves comments but never
        // ADDS them, so a config born bare stays bare forever. See
        // [`render_config_documented`].
        write_config_atomic(config_path, &render_config_documented(config), durability)
    }
}

/// The canonical fully-commented renderer, installed by the TUI at startup.
///
/// It cannot live in this crate: it needs the TUI's `RuntimeBindings` to render
/// the `[keys]` and `[macros]` comments, and `dux-core` does not depend on
/// `dux-tui`. So the binary registers it here once and every surface that
/// creates a config file (the TUI, and `dux server`'s bootstrap project-sync)
/// gets the documented output from the same source.
static CANONICAL_RENDERER: std::sync::OnceLock<fn(&Config) -> String> = std::sync::OnceLock::new();

/// Install the fully-commented renderer. Idempotent; the first caller wins.
pub fn set_canonical_renderer(renderer: fn(&Config) -> String) {
    let _ = CANONICAL_RENDERER.set(renderer);
}

/// Render `config` with comments when a canonical renderer has been installed,
/// falling back to the plain render otherwise.
///
/// The fallback is deliberate rather than a panic: a comment-free config is
/// degraded, not broken, and `dux config restore-docs` can add the comments
/// later. Refusing to write would lose the user's settings outright, which is a
/// far worse failure than losing the prose.
pub fn render_config_documented(config: &Config) -> String {
    match CANONICAL_RENDERER.get() {
        Some(render) => render(config),
        None => render_config_plain(config),
    }
}

/// Unconditionally write a fresh plain (uncommented) `toml_edit` serialization,
/// overwriting whatever is on disk. Unlike [`save_config`], this never patches
/// an existing file, so it succeeds even when the current `config.toml` is
/// corrupt or unparseable. Used by the web's "recover config" path, which must
/// overwrite a broken file from the in-memory config.
#[deprecated(
    note = "route config writes through ConfigWriteQueue; sync-direct callers must #[allow(deprecated)]"
)]
pub fn write_config_plain(config_path: &Path, config: &Config) -> Result<()> {
    write_config_plain_with(config_path, config, Durability::Fsync)
}

pub fn write_config_plain_with(
    config_path: &Path,
    config: &Config,
    durability: Durability,
) -> Result<()> {
    // Render the same plain (comment-free) document `render_config_plain`
    // produces, then write it atomically. Sharing the renderer keeps the on-disk
    // shape and the `recover_render` string byte-identical.
    write_config_atomic(config_path, &render_config_plain(config), durability)
}

/// Render `config` to a fresh, plain (comment-free) `config.toml` text using the
/// shared patch set against an empty document, with no comments (this is the plain
/// fallback, not the TUI's pretty first-creation path). The same shape
/// [`write_config_plain`] writes, but returned as a `String` instead of written.
/// Used by a surface's `recover_render` (e.g. the web's plain recovery render) so
/// the Engine can perform the atomic write through its own writer while holding
/// the quiesce barrier.
pub fn render_config_plain(config: &Config) -> String {
    let mut doc = DocumentMut::new();
    apply_patches(&mut doc, config);
    render_auth_section(&mut doc, &config.server.auth);
    doc.to_string()
}

/// Apply every section patch to `doc`. Mirrors the section sequence the TUI's
/// existing-file branch ran, so both surfaces produce the same managed shape.
///
/// A user-edited file can hold any shape. A section written inline
/// (`env = { A = "1" }` before any header, or a provider written inline) is
/// patched as a table and written back inline, so it keeps the user's form.
fn apply_patches(doc: &mut DocumentMut, config: &Config) {
    let inline = inline_sections(doc);
    apply_section_patches(doc, config);
    restore_inline_sections(doc, &inline);
}

/// The paths of the sections the patches write that the file holds inline:
/// every top-level inline table, and every inline table directly under
/// `[providers]`.
fn inline_sections(doc: &DocumentMut) -> Vec<Vec<String>> {
    let mut paths = Vec::new();
    for (key, item) in doc.iter() {
        if is_inline_table(item) {
            paths.push(vec![key.to_string()]);
        }
    }
    let providers: Option<Vec<String>> = match doc.get("providers") {
        Some(Item::Table(table)) => Some(
            table
                .iter()
                .filter(|(_, item)| is_inline_table(item))
                .map(|(key, _)| key.to_string())
                .collect(),
        ),
        Some(Item::Value(Value::InlineTable(table))) => Some(
            table
                .iter()
                .filter(|(_, value)| value.is_inline_table())
                .map(|(key, _)| key.to_string())
                .collect(),
        ),
        _ => None,
    };
    for name in providers.unwrap_or_default() {
        paths.push(vec!["providers".to_string(), name]);
    }
    // Innermost first, so a provider goes back inline before its parent.
    paths.sort_by_key(|path| std::cmp::Reverse(path.len()));
    paths
}

/// Put each section of `paths` that the patches turned into a table back
/// inline, in place, so its position and the comment above it stay.
fn restore_inline_sections(doc: &mut DocumentMut, paths: &[Vec<String>]) {
    for path in paths {
        restore_inline_at(doc.as_table_mut(), path);
    }
}

fn restore_inline_at(table: &mut Table, path: &[String]) {
    match path {
        [] => {}
        [last] => {
            if let Some(item) = table.get_mut(last)
                && let Item::Table(section) = item
            {
                let inline = std::mem::take(section).into_inline_table();
                *item = toml_edit::value(inline);
            }
        }
        [parent, rest @ ..] => {
            if let Some(Item::Table(next)) = table.get_mut(parent) {
                restore_inline_at(next, rest);
            }
        }
    }
}

fn apply_section_patches(doc: &mut DocumentMut, config: &Config) {
    // --- top-level (no table) keys ---
    // A dotless root key must render before any table header or TOML would parse
    // it as belonging to the preceding table. `patch_root_u16` positions it at
    // the front of the document, so this is order-safe whether the doc is empty
    // (plain render) or an existing user file already full of tables (patch).
    patch_root_u16(
        doc,
        "shutdown_timeout_seconds",
        config.shutdown_timeout_seconds,
    );

    // --- [defaults] ---
    patch_table_str(doc, "defaults", "provider", &config.defaults.provider);
    patch_table_opt_str(
        doc,
        "defaults",
        "start_directory",
        config.defaults.start_directory.as_deref(),
    );
    // The AI commit-message feature was removed; drop its now-obsolete prompt key
    // from any existing config so saves stop carrying it forward.
    remove_table_key(doc, "defaults", "commit_prompt");
    patch_table_bool(
        doc,
        "defaults",
        "enable_randomized_pet_name_by_default",
        config.defaults.enable_randomized_pet_name_by_default,
    );
    patch_table_bool(
        doc,
        "defaults",
        "pull_before_creating_agent_by_default",
        config.defaults.pull_before_creating_agent_by_default,
    );
    patch_table_bool(
        doc,
        "defaults",
        "copy_uncommitted_changes_by_default",
        config.defaults.copy_uncommitted_changes_by_default,
    );
    remove_table_key(doc, "defaults", "prompt_for_name");

    // --- [env] ---
    patch_env_table(doc, "env", &config.env);

    // --- [logging] ---
    patch_table_str(doc, "logging", "level", &config.logging.level);
    patch_table_str(doc, "logging", "path", &config.logging.path);
    patch_table_u64(doc, "logging", "max_bytes", config.logging.max_bytes);
    patch_table_u32(doc, "logging", "keep", config.logging.keep);
    patch_table_bool(doc, "logging", "compress", config.logging.compress);

    // --- [ui] ---
    patch_table_u16(doc, "ui", "left_width_pct", config.ui.left_width_pct);
    patch_table_u16(doc, "ui", "right_width_pct", config.ui.right_width_pct);
    patch_table_u16(
        doc,
        "ui",
        "terminal_pane_height_pct",
        config.ui.terminal_pane_height_pct,
    );
    patch_table_u16(
        doc,
        "ui",
        "empty_project_separator_min_projects",
        config.ui.empty_project_separator_min_projects,
    );
    patch_table_u16(
        doc,
        "ui",
        "staged_pane_height_pct",
        config.ui.staged_pane_height_pct,
    );
    patch_table_u16(
        doc,
        "ui",
        "commit_pane_height_pct",
        config.ui.commit_pane_height_pct,
    );
    patch_table_usize(
        doc,
        "ui",
        "agent_scrollback_lines",
        config.ui.agent_scrollback_lines,
    );
    patch_table_u16(doc, "ui", "agent_tabs_max", config.ui.agent_tabs_max);
    patch_table_u16(
        doc,
        "ui",
        "status_clear_seconds",
        config.ui.status_clear_seconds,
    );
    patch_table_u16(
        doc,
        "ui",
        "branch_sync_interval",
        config.ui.branch_sync_interval,
    );
    patch_table_bool(
        doc,
        "ui",
        "show_diff_line_numbers",
        config.ui.show_diff_line_numbers,
    );
    patch_table_u16(doc, "ui", "diff_tab_width", config.ui.diff_tab_width);
    patch_table_bool(
        doc,
        "ui",
        "github_integration",
        config.ui.github_integration,
    );
    patch_table_u16(
        doc,
        "ui",
        "pr_poll_interval_seconds",
        config.ui.pr_poll_interval_seconds,
    );
    patch_table_u32(
        doc,
        "ui",
        "pr_poll_inactive_interval_seconds",
        config.ui.pr_poll_inactive_interval_seconds,
    );
    patch_table_u16(
        doc,
        "ui",
        "github_probe_interval_secs",
        config.ui.github_probe_interval_secs,
    );
    patch_table_bool(doc, "ui", "copy_on_select", config.ui.copy_on_select);
    patch_table_str(
        doc,
        "ui",
        "terminal_font_family",
        &config.ui.terminal_font_family,
    );
    patch_table_u16(
        doc,
        "ui",
        "terminal_font_size",
        config.ui.terminal_font_size,
    );
    patch_table_str(doc, "ui", "compose_bar", &config.ui.compose_bar);
    patch_table_bool(
        doc,
        "ui",
        "mobile_accessory_bar",
        config.ui.mobile_accessory_bar,
    );
    patch_table_u64(
        doc,
        "ui",
        "attention_grace_seconds",
        config.ui.attention_grace_seconds,
    );
    patch_table_bool(
        doc,
        "ui",
        "auto_reopen_agents",
        config.ui.auto_reopen_agents,
    );
    patch_table_bool(doc, "ui", "show_changes_pane", config.ui.show_changes_pane);
    patch_table_bool(
        doc,
        "ui",
        "always_show_tab_strip",
        config.ui.always_show_tab_strip,
    );
    patch_table_bool(doc, "ui", "tab_reaches_agent", config.ui.tab_reaches_agent);
    patch_table_str(doc, "ui", "upload_directory", &config.ui.upload_directory);
    patch_table_bool(
        doc,
        "ui",
        "upload_write_gitignore",
        config.ui.upload_write_gitignore,
    );
    patch_table_usize(
        doc,
        "ui",
        "upload_pasted_text_chars",
        config.ui.upload_pasted_text_chars,
    );
    patch_table_bool(
        doc,
        "ui",
        "attention_indicator",
        config.ui.attention_indicator,
    );
    patch_table_bool(doc, "ui", "attention_on_bell", config.ui.attention_on_bell);
    patch_table_bool(
        doc,
        "ui",
        "disable_automated_welcome_screen",
        config.ui.disable_automated_welcome_screen,
    );
    patch_table_bool(
        doc,
        "ui",
        "disable_release_notes",
        config.ui.disable_release_notes,
    );
    patch_table_str(
        doc,
        "ui",
        "pr_banner_position",
        &config.ui.pr_banner_position,
    );
    patch_table_str(doc, "ui", "agent_sort", &config.ui.agent_sort);
    patch_table_str(doc, "ui", "theme", &config.ui.theme);

    // --- [capabilities] ---
    patch_table_str(
        doc,
        "capabilities",
        "terminal_identity",
        &config.capabilities.terminal_identity,
    );
    patch_table_bool(
        doc,
        "capabilities",
        "passthrough",
        config.capabilities.passthrough,
    );
    patch_table_str(
        doc,
        "capabilities",
        "clipboard_passthrough",
        &config.capabilities.clipboard_passthrough,
    );
    patch_table_bool(
        doc,
        "capabilities",
        "hyperlinks",
        config.capabilities.hyperlinks,
    );
    patch_table_bool(
        doc,
        "capabilities",
        "web_notifications",
        config.capabilities.web_notifications,
    );

    // --- [editor] ---
    patch_table_str(doc, "editor", "default", &config.editor.default);

    // --- [server] ---
    // The deprecated `bind` field is migrated away on load and is never
    // re-emitted here, so a patch/recover/plain write produces the new
    // host/port shape only.
    patch_table_str(doc, "server", "host", &config.server.host);
    patch_table_u16(doc, "server", "port", config.server.port);
    patch_table_str(doc, "server", "tailscale", &config.server.tailscale);
    // The boolean `tailscale_enabled` became the tri-state `tailscale`. Load-time
    // migration already rewrote it in memory; dropping it here is what takes it
    // OUT of the file, so a save stops carrying a key dux no longer reads (the
    // same one-line strip `max_websocket_connections` gets below).
    remove_table_key(doc, "server", "tailscale_enabled");
    patch_table_string_array(doc, "server", "allowed_hosts", &config.server.allowed_hosts);
    patch_table_str(doc, "server", "color", &config.server.color);
    patch_table_bool(doc, "server", "access_log", config.server.access_log);
    patch_table_usize(
        doc,
        "server",
        "log_viewer_lines",
        config.server.log_viewer_lines,
    );
    patch_table_bool(doc, "server", "qr_codes", config.server.qr_codes);
    patch_table_bool(
        doc,
        "server",
        "serve_while_tui",
        config.server.serve_while_tui,
    );
    // The single WebSocket cap was split into three per-class caps; drop the
    // obsolete key from any existing config block on every save so saves stop
    // carrying it (mirrors the oneshot strip in `patch_providers`). Warn when
    // the key is actually present so a TUI user (who never calls load_config on
    // the server path) still sees the migration notice in dux.log and on stderr.
    if remove_table_key_item(doc, "server", "max_websocket_connections").is_some() {
        let msg = "[server] max_websocket_connections has been removed and is being ignored. \
            It was split into max_websocket_events_connections, \
            max_websocket_agent_connections, and max_websocket_terminal_connections. \
            Set those per-class caps instead; a value of 0 still means disable \
            (refuse all new connections of that class until restart).";
        crate::logger::warn(msg);
        eprintln!("dux config migration warning: {msg}");
    }
    patch_table_usize(
        doc,
        "server",
        "max_websocket_events_connections",
        config.server.max_websocket_events_connections as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "max_websocket_agent_connections",
        config.server.max_websocket_agent_connections as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "max_websocket_terminal_connections",
        config.server.max_websocket_terminal_connections as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "max_websocket_tab_connections",
        config.server.max_websocket_tab_connections as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "max_websocket_tabs_per_agent",
        config.server.max_websocket_tabs_per_agent as usize,
    );
    patch_table_str(doc, "server", "title", &config.server.title);
    patch_table_str(doc, "server", "favicon", &config.server.favicon);
    patch_table_u16(
        doc,
        "server",
        "shutdown_timeout_seconds",
        config.server.shutdown_timeout_seconds,
    );
    patch_table_usize(
        doc,
        "server",
        "search_index_max_files",
        config.server.search_index_max_files,
    );
    patch_table_usize(
        doc,
        "server",
        "replay_wait_seconds",
        config.server.replay_wait_seconds as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "reconnect_backoff_cap_seconds",
        config.server.reconnect_backoff_cap_seconds as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "reconnect_attempts",
        config.server.reconnect_attempts as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "reconnect_attempt_timeout_seconds",
        config.server.reconnect_attempt_timeout_seconds as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "changes_request_timeout_seconds",
        config.server.changes_request_timeout_seconds as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "heartbeat_seconds",
        config.server.heartbeat_seconds as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "heartbeat_deadline_seconds",
        config.server.heartbeat_deadline_seconds as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "pty_send_timeout_seconds",
        config.server.pty_send_timeout_seconds as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "tree_list_max_concurrency",
        config.server.tree_list_max_concurrency as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "release_notes_max_concurrency",
        config.server.release_notes_max_concurrency as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "file_drop_max_bytes",
        config.server.file_drop_max_bytes,
    );
    patch_table_usize(
        doc,
        "server",
        "file_drop_max_concurrency",
        config.server.file_drop_max_concurrency as usize,
    );

    // [server.auth] is deliberately absent: see `render_auth_section`.

    // --- [terminal] ---
    patch_table_str(doc, "terminal", "command", &config.terminal.command);
    patch_table_string_array(doc, "terminal", "args", &config.terminal.args);

    // --- [startup_command_terminal] ---
    patch_table_str(
        doc,
        "startup_command_terminal",
        "command",
        &config.startup_command_terminal.command,
    );
    patch_table_string_array(
        doc,
        "startup_command_terminal",
        "args",
        &config.startup_command_terminal.args,
    );

    // --- [keys] ---
    patch_table_bool(
        doc,
        "keys",
        "show_terminal_keys",
        config.keys.show_terminal_keys,
    );
    {
        let keys_table = ensure_table(doc, "keys");
        for (action, key_strs) in &config.keys.bindings {
            let mut arr = Array::new();
            for s in key_strs {
                arr.push(s.as_str());
            }
            keys_table[action] = toml_edit::value(arr);
        }
    }

    // --- [providers.*] ---
    patch_providers(doc, &config.providers);

    // --- [[projects]] ---
    patch_projects(doc, &config.projects);

    // --- [macros] ---
    patch_macros(doc, &config.macros);
}

// ---------------------------------------------------------------------------
// toml_edit patch helpers
// ---------------------------------------------------------------------------

/// Get or create a table named `section` at the document root.
///
/// Public because the TUI's deprecation migrations reuse it.
pub fn ensure_table<'a>(doc: &'a mut DocumentMut, section: &str) -> &'a mut Table {
    table_in(doc.as_table_mut(), section)
}

/// Get or create the table at `key` of `parent`, whatever the file holds
/// there: a table written inline becomes a table with the same entries (see
/// [`apply_patches`], which puts it back inline), and any other value, which
/// no reader takes as that section anyway (a load reads it as the
/// defaults), is replaced by an empty table. Never panics on a user's shape.
fn table_in<'a>(parent: &'a mut Table, key: &str) -> &'a mut Table {
    let item = parent
        .entry(key)
        .or_insert_with(|| Item::Table(Table::new()));
    if !item.is_table() {
        let table = match std::mem::take(item) {
            Item::Value(Value::InlineTable(inline)) => inline.into_table(),
            _ => Table::new(),
        };
        *item = Item::Table(table);
    }
    match item {
        Item::Table(table) => table,
        // Set to a table just above.
        _ => unreachable!("the item was just made a table"),
    }
}

fn patch_table_str(doc: &mut DocumentMut, section: &str, key: &str, value: &str) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value);
}

fn patch_table_opt_str(doc: &mut DocumentMut, section: &str, key: &str, value: Option<&str>) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value.unwrap_or(""));
}

fn patch_table_u16(doc: &mut DocumentMut, section: &str, key: &str, value: u16) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(i64::from(value));
}

/// Set a dotless key at the document root. With the pinned `toml_edit`, a
/// table's own key/value pairs render before its child tables, so a root leaf
/// key emits at the top of the document, ahead of every `[table]` header, valid
/// TOML whether the document is empty (plain render) or an existing user file
/// already full of tables (patch). That ordering is emergent encoder behavior,
/// not a documented `toml_edit` API guarantee, so it is not assumed blindly: the
/// `root_key_renders_before_tables_and_round_trips` and
/// `patch_adds_root_key_to_existing_file_without_corruption` tests re-parse the
/// rendered output and would fail loudly if a `toml_edit` upgrade ever moved the
/// bare key after a table header (which would otherwise parse it into that
/// table). If they break, this key needs an explicit position fix-up here.
fn patch_root_u16(doc: &mut DocumentMut, key: &str, value: u16) {
    doc[key] = toml_edit::value(i64::from(value));
}

fn patch_table_u32(doc: &mut DocumentMut, section: &str, key: &str, value: u32) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(i64::from(value));
}
fn patch_table_usize(doc: &mut DocumentMut, section: &str, key: &str, value: usize) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value as i64);
}

fn patch_table_bool(doc: &mut DocumentMut, section: &str, key: &str, value: bool) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value);
}

fn patch_table_u64(doc: &mut DocumentMut, section: &str, key: &str, value: u64) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value as i64);
}

fn remove_table_key(doc: &mut DocumentMut, section: &str, key: &str) {
    let _ = remove_table_key_item(doc, section, key);
}

/// Remove `key` from the table named `section`, returning the removed item.
///
/// Public because the TUI's deprecation migrations reuse it.
pub fn remove_table_key_item(doc: &mut DocumentMut, section: &str, key: &str) -> Option<Item> {
    doc.get_mut(section)
        .and_then(Item::as_table_mut)
        .and_then(|table| table.remove(key))
}

fn patch_table_string_array(doc: &mut DocumentMut, section: &str, key: &str, values: &[String]) {
    let table = ensure_table(doc, section);
    let mut arr = Array::new();
    for v in values {
        arr.push(v.as_str());
    }
    table[key] = toml_edit::value(arr);
}

fn patch_providers(doc: &mut DocumentMut, providers: &ProvidersConfig) {
    let providers_table = ensure_table(doc, "providers");

    for (name, config) in &providers.commands {
        let tbl = table_in(providers_table, name);

        tbl["command"] = toml_edit::value(&config.command);

        let mut args = Array::new();
        for a in &config.args {
            args.push(a.as_str());
        }
        tbl["args"] = toml_edit::value(args);

        let mut resume = Array::new();
        for a in config.resume_args.as_deref().unwrap_or(&[]) {
            resume.push(a.as_str());
        }
        tbl["resume_args"] = toml_edit::value(resume);
        if let Some(timeout_ms) = config.resume_wait_timeout_ms {
            tbl["resume_wait_timeout_ms"] = toml_edit::value(timeout_ms as i64);
        }

        // The AI commit-message feature was removed; drop the obsolete oneshot
        // keys from any existing provider block so saves stop carrying them.
        tbl.remove("oneshot_args");
        tbl.remove("oneshot_output");

        if let Some(hint) = &config.install_hint {
            tbl["install_hint"] = toml_edit::value(hint.as_str());
        }

        // Tri-state: write the bool only when the user pinned a value. An
        // absent key means auto-detect (forward only to a fullscreen,
        // mouse-aware child), so omit it when `None`.
        match config.forward_scroll {
            Some(value) => tbl["forward_scroll"] = toml_edit::value(value),
            None => {
                tbl.remove("forward_scroll");
            }
        }

        // The drag-and-drop paste form for the WEB UI. Every provider dux ships
        // carries one (`ensure_defaults` fills it in), so in practice this writes;
        // a provider the user added themselves has none and an absent key means
        // `bare`, so omit it rather than inventing a value they did not choose.
        match &config.web_dragdrop_paste {
            Some(value) => tbl["web_dragdrop_paste"] = toml_edit::value(value.as_str()),
            None => {
                tbl.remove("web_dragdrop_paste");
            }
        }
    }
}

fn patch_macros(doc: &mut DocumentMut, macros: &MacrosConfig) {
    let table = ensure_table(doc, "macros");

    // Remove entries that no longer exist in config.
    let existing_keys: Vec<String> = table
        .iter()
        .filter(|(_, v)| v.is_inline_table())
        .map(|(k, _)| k.to_string())
        .collect();
    for key in &existing_keys {
        if !macros.entries.contains_key(key) {
            table.remove(key);
        }
    }

    // Add or update entries, IN `macros.entries` ORDER. Order is meaningful
    // (the macro bar, the web quick-picker, and the editor list all render in
    // declaration order, and the web editor reorders by drag-and-drop through
    // this same wholesale save), and `table[name] = ...` on an existing key
    // keeps its OLD toml_edit position, so an in-place write would round-trip a
    // pure reorder while the file kept the old order, and a dux restart would
    // silently undo the drag. Removing each key first and re-inserting
    // with `insert_formatted` appends in iteration order while carrying the
    // key's own decor (a comment the user wrote above a macro line) with it.
    for (name, entry) in &macros.entries {
        let existing_key = table.remove_entry(name).map(|(key, _)| key);
        let mut inline = InlineTable::new();
        inline.insert("text", Value::String(Formatted::new(entry.text.clone())));
        inline.insert(
            "surface",
            Value::String(Formatted::new(entry.surface.as_config_str().to_string())),
        );
        let item = toml_edit::value(Value::InlineTable(inline));
        match existing_key {
            Some(key) => {
                table.insert_formatted(&key, item);
            }
            None => {
                table[name] = item;
            }
        }
    }
}

/// The keys [`patch_projects`] writes itself. Everything else it finds in an
/// existing `[[projects]]` entry belongs to the user (a hand-added note, a key
/// from a newer dux, a fork's own setting) and is carried over verbatim, except
/// for [`PROJECT_KEYS_DROPPED_ON_WRITE`].
const PROJECT_MANAGED_KEYS: &[&str] = &[
    "id",
    "path",
    "name",
    "default_provider",
    "auto_reopen_agents",
    "startup_command",
    "env",
];

/// Project keys dux once wrote to config and now keeps in SQLite instead. These
/// are DROPPED on every write rather than carried over: `leading_branch` is
/// autodetected runtime state, and leaving it in portable config pins the
/// detection (CLAUDE.md, "Keep derived project state in SQLite"). The loader
/// still reads it to repair SQLite, which is why it stays a `ProjectConfig`
/// field; only the writer refuses to emit it.
const PROJECT_KEYS_DROPPED_ON_WRITE: &[&str] = &["leading_branch"];

/// One existing `[[projects]]` entry's user-owned material, captured before the
/// rebuild replaces it.
#[derive(Default)]
struct CarriedProject {
    /// The entry's `id`, when it wrote one. A hand-written entry may legally leave
    /// it out: `ProjectConfig::id` carries `#[serde(default = "new_project_id")]`,
    /// so the loader mints an identifier and the file works. The carry must
    /// include such an entry: skipping it deletes both its unmanaged keys and its
    /// comment on the next save (pinned by
    /// `patch_keeps_the_keys_and_comment_of_an_entry_that_carries_no_id`).
    id: Option<String>,
    /// The entry's `path` AS SPELLED IN THE FILE. It is the fallback identity for
    /// an entry with no id, and the tie-breaker between two entries that share one.
    ///
    /// The raw spelling is deliberately not normalized: the loader env-expands the
    /// path, so `$HOME/p` in the file is `/home/user/p` in memory and the two do
    /// not compare equal. A miss is the correct outcome there. Guessing past it
    /// would mean attaching one project's keys to another project's entry, which is
    /// worse than dropping them.
    path: Option<String>,
    /// The entry's `name`, which tells apart two id-less entries at the same
    /// path (two projects for one folder) before their position does.
    name: Option<String>,
    /// The comment block the user wrote ABOVE the entry, as comment lines with the
    /// surrounding whitespace already dropped (see [`carried_comment_prefix`]).
    /// `toml_edit` files it on the entry's own decor rather than on any of its keys,
    /// so carrying the keys alone deleted it.
    ///
    /// Present for BOTH spellings. The array-of-tables spelling keeps it on the
    /// table's decor; the multi-line `projects = [ ... ]` spelling can carry a
    /// comment between its elements (legal TOML, and pure user data), and
    /// `toml_edit` files that on the following element's value decor.
    header_comment: Option<String>,
    /// The comment block written between the `[[projects]]` header and the entry's
    /// FIRST key. `toml_edit` files that as the first key's prefix, and the first
    /// key is `id`, which this writer rebuilds from scratch, so it reached neither
    /// the decor carry nor the key carry and was deleted.
    first_key_comment: Option<String>,
    /// The keys this writer does not manage, each with its own [`Key`] so a
    /// comment attached to it travels along, exactly as in
    /// [`merge_unmanaged_keys`].
    keys: Vec<(Key, Item)>,
}

/// Every key and comment the user's own `[[projects]]` entries carry that this
/// writer does not manage, in FILE ORDER. Each entry keeps its own identity
/// (`id`, `path`) so it can be matched back to the right project even when
/// projects were added, removed, or reordered between saves.
///
/// Captured BEFORE the rebuild below. `patch_projects` replaces the whole array
/// (it has to: a project can be removed, and per-project keys are optional, so
/// there is no in-place edit that covers every case), so anything not captured
/// here is deleted by that replacement.
///
/// BOTH legal spellings of the array are read. `[[projects]]` (an array of tables)
/// is what dux writes, but `projects = [ { ... } ]` (an array of inline tables)
/// parses to the identical `Config`, so a user who hand-wrote that form has a
/// working config and must not lose their keys on the next save.
///
/// Every entry is captured, including one with no unmanaged keys and one with no
/// `id` at all, because the comments hang off the entry rather than off its keys.
/// [`take_carried_project`] owns the matching rules.
fn unmanaged_project_keys(doc: &DocumentMut) -> Vec<Option<CarriedProject>> {
    let mut carried: Vec<Option<CarriedProject>> = Vec::new();
    match doc.get("projects") {
        Some(Item::ArrayOfTables(existing)) => {
            for table in existing.iter() {
                let keys: Vec<(Key, Item)> = table
                    .iter()
                    .map(|(name, _)| name.to_string())
                    .filter(|name| is_unmanaged_project_key(name))
                    .filter_map(|name| {
                        table
                            .get_key_value(&name)
                            .map(|(key, item)| (key.clone(), item.clone()))
                    })
                    .collect();
                // Only when the first key is one this writer REBUILDS. An
                // unmanaged key written first keeps that comment on its own decor
                // and is carried with it, so re-homing it onto `id` as well
                // duplicated it further down the file on every save.
                let first_key_comment = table
                    .iter()
                    .next()
                    .filter(|(name, _)| !is_unmanaged_project_key(name))
                    .and_then(|(name, _)| table.get_key_value(name))
                    .and_then(|(key, _)| decor_comment(key.leaf_decor()));
                carried.push(Some(CarriedProject {
                    id: table.get("id").and_then(Item::as_str).map(str::to_string),
                    path: table.get("path").and_then(Item::as_str).map(str::to_string),
                    name: table.get("name").and_then(Item::as_str).map(str::to_string),
                    header_comment: decor_comment(table.decor()),
                    first_key_comment,
                    keys,
                }));
            }
        }
        Some(Item::Value(Value::Array(existing))) => {
            for value in existing.iter() {
                let Some(inline) = value.as_inline_table() else {
                    continue;
                };
                let keys: Vec<(Key, Item)> = inline
                    .iter()
                    .map(|(name, _)| name.to_string())
                    .filter(|name| is_unmanaged_project_key(name))
                    .filter_map(|name| {
                        inline
                            .get_key_value(&name)
                            .map(|(key, item)| (strip_inline_decor(key), item.clone()))
                    })
                    .map(|(key, item)| (key, block_spaced_item(item)))
                    .collect();
                carried.push(Some(CarriedProject {
                    id: inline.get("id").and_then(Value::as_str).map(str::to_string),
                    path: inline
                        .get("path")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    name: inline
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    // A comment written between two elements of a multi-line array
                    // is filed as the prefix of the element that FOLLOWS it, which
                    // is where an inline entry's comment lives. (A comment after the
                    // LAST element is the array's own trailing trivia and is not an
                    // entry's data, the same call the array-of-tables spelling makes
                    // for a comment below the last key.)
                    header_comment: decor_comment(value.decor()),
                    // An inline table is one line, so there is no "between the
                    // header and the first key" position to capture.
                    first_key_comment: None,
                    keys,
                }));
            }
        }
        // No projects yet, or a `projects` key of some other type entirely (a
        // string, say). Nothing to carry, and the rebuild replaces it.
        _ => {}
    }
    carried
}

/// The captured entry that belongs to `project`, removed from `carried` so no two
/// projects can claim the same one.
///
/// Identity is tried in three steps, most specific first:
///
/// 1. the PAIR of `id` and raw `path`. Matching on the id alone lets two entries
///    sharing an id (a hand-edit the project sync rejects, but that a file can
///    hold) swap their keys as soon as the projects are reordered in memory.
/// 2. the `id` alone, because the raw path is not always comparable: the loader
///    env-expands it, so a file that spells it `$HOME/p` holds `/home/ada/p` in
///    memory and never matches on the pair.
/// 3. the raw `path` alone, and only for an entry that carried NO id, whose id was
///    minted by the loader and therefore cannot match anything in the file; one
///    with the project's own `name` first, so two such entries at one path keep
///    their own keys.
///
/// Step 3 accepts a miss (an id-less entry whose path is env-expanded loses its
/// extras) rather than guessing, because the wrong guess attaches one project's
/// keys and comments to a different project's worktree.
fn take_carried_project(
    carried: &mut [Option<CarriedProject>],
    project: &ProjectConfig,
) -> Option<CarriedProject> {
    let position = |matches: &dyn Fn(&CarriedProject) -> bool| {
        carried
            .iter()
            .position(|slot| slot.as_ref().is_some_and(matches))
    };
    let found = position(&|entry: &CarriedProject| {
        entry.id.as_deref() == Some(project.id.as_str())
            && entry.path.as_deref() == Some(project.path.as_str())
    })
    .or_else(|| {
        position(&|entry: &CarriedProject| entry.id.as_deref() == Some(project.id.as_str()))
    })
    .or_else(|| {
        // Two id-less entries at one path are told apart by their name first.
        position(&|entry: &CarriedProject| {
            entry.id.is_none()
                && entry.path.as_deref() == Some(project.path.as_str())
                && entry.name.is_some()
                && entry.name == project.name
        })
    })
    .or_else(|| {
        position(&|entry: &CarriedProject| {
            entry.id.is_none() && entry.path.as_deref() == Some(project.path.as_str())
        })
    })?;
    carried[found].take()
}

/// A key carried out of an inline table, with the inline spacing dropped.
///
/// Inside `{ id = "a", note = 1 }` the key records a leading and trailing space of
/// its own; pasted into a block table those become ` note = 1 `. The document still
/// reparses, so this is cosmetic, but it accumulates over saves.
fn strip_inline_decor(key: &Key) -> Key {
    let mut key = key.clone();
    *key.leaf_decor_mut() = Decor::default();
    key
}

/// The same for the value half of a carried inline entry: back to the default
/// decor, so the block table spaces it the way it spaces everything else.
fn block_spaced_item(mut item: Item) -> Item {
    if let Some(value) = item.as_value_mut() {
        *value.decor_mut() = Decor::default();
    }
    item
}

/// Whether a key found in an existing project entry is the user's rather than
/// this writer's, and so has to be carried over.
fn is_unmanaged_project_key(name: &str) -> bool {
    !PROJECT_MANAGED_KEYS.contains(&name) && !PROJECT_KEYS_DROPPED_ON_WRITE.contains(&name)
}

/// The comment lines in a captured decor prefix, or `None` when it holds only the
/// whitespace `toml_edit` records by default.
///
/// The whitespace is deliberately NOT kept. Pasting the recorded prefix verbatim
/// re-used the source entry's own separation, and the file's FIRST entry has no
/// leading blank line because nothing precedes it, so moving that project to
/// second position ran the two entries together (measured in
/// `a_commented_project_moved_later_keeps_a_blank_line_before_its_header`). Only
/// the comment lines are the user's data; the spacing around them belongs to
/// wherever the entry ends up.
fn decor_comment(decor: &Decor) -> Option<String> {
    let prefix = decor.prefix().and_then(|prefix| prefix.as_str())?;
    let comments: Vec<&str> = prefix
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('#'))
        .collect();
    (!comments.is_empty()).then(|| comments.join("\n"))
}

fn patch_projects(doc: &mut DocumentMut, projects: &[ProjectConfig]) {
    let mut carried = unmanaged_project_keys(doc);
    let _ = doc.remove("projects");
    if projects.is_empty() {
        return;
    }

    let mut array = toml_edit::ArrayOfTables::new();
    for project in projects {
        let mut table = Table::new();
        table["id"] = toml_edit::value(project.id.as_str());
        table["path"] = toml_edit::value(project.path.as_str());
        if let Some(name) = project.name.as_deref() {
            table["name"] = toml_edit::value(name);
        }
        if let Some(provider) = project.default_provider.as_deref() {
            table["default_provider"] = toml_edit::value(provider);
        }
        if let Some(auto_reopen_agents) = project.auto_reopen_agents {
            table["auto_reopen_agents"] = toml_edit::value(auto_reopen_agents);
        }
        if let Some(command) = project.startup_command.as_deref() {
            table["startup_command"] = toml_edit::value(command);
        }
        if !project.env.is_empty() {
            let mut inline = InlineTable::new();
            for (name, value) in &project.env {
                inline.insert(name, Value::String(Formatted::new(value.clone())));
            }
            table["env"] = toml_edit::value(Value::InlineTable(inline));
        }
        // Put the user's own keys and comments back. The source entry is matched
        // by identity rather than by position (see `take_carried_project`) and is
        // consumed, so no two rebuilt entries can claim the same one.
        if let Some(extras) = take_carried_project(&mut carried, project) {
            for (key, item) in extras.keys {
                table.insert_formatted(&key, item);
            }
            // The comment block the user wrote above this entry's header, which
            // lives on the entry's decor rather than on any of its keys. The
            // leading newline is what separates it from whatever now precedes it,
            // whether or not anything did in the source file.
            if let Some(comment) = extras.header_comment {
                table.decor_mut().set_prefix(format!("\n{comment}\n"));
            }
            // ...and the comment block between the header and the first key, which
            // `toml_edit` files as the prefix of that first key. The rebuilt first
            // key is always `id`, written just above.
            if let Some(comment) = extras.first_key_comment
                && let Some(mut key) = table.key_mut("id")
            {
                key.leaf_decor_mut().set_prefix(format!("{comment}\n"));
            }
        }
        array.push(table);
    }
    doc["projects"] = Item::ArrayOfTables(array);
}

fn patch_env_table(doc: &mut DocumentMut, section: &str, env: &BTreeMap<String, String>) {
    let table = ensure_table(doc, section);
    let existing = table
        .iter()
        .map(|(key, _)| key.to_string())
        .collect::<Vec<_>>();
    for key in existing {
        table.remove(&key);
    }
    for (name, value) in env {
        table[name] = toml_edit::value(value.as_str());
    }
}

// ---------------------------------------------------------------------------
// Documentation restore: merging a user's unmanaged keys into a fresh render
// ---------------------------------------------------------------------------

/// Sections dux once wrote but no longer reads. They survive in real user files
/// only because the surgical patch path preserves unknown keys, so a config that
/// predates their removal carries them forever.
///
/// Anything listed here is REMOVED by the documentation-restore merge (and the
/// removal is reported to the user; a silent drop is data loss even when the
/// data was inert). Everything NOT listed here is preserved verbatim: a user may
/// hand-add keys, run a fork, or have keys from a newer dux.
///
/// Entries are dotted table paths. A path matches when it is equal to an entry
/// or nested beneath one.
pub const ORPHANED_CONFIG_SECTIONS: &[&[&str]] = &[
    // Removed with the HTTP-basic-auth experiment. Only this TOP-LEVEL `[auth]`
    // table: the web login's live settings are `[server.auth]`, which a dotted
    // path match never confuses with it (pinned by
    // `the_orphan_cleanup_never_touches_server_auth`).
    &["auth"],
    // Removed with the built-in ACME/TLS listener. TLS is delegated to an
    // upstream proxy or to Tailscale.
    &["server", "acme"],
];

/// One step of a path through a TOML document: a table key, or an index into an
/// array of tables (`[[projects]]`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum PathSeg {
    Key(String),
    Index(usize),
}

/// Render a path for display: `server.acme.production`, `projects[0].custom_key`.
/// Every path, an index into an array of tables included, goes through the
/// one formatter ([`crate::config::shown_parts`]) against the file's text
/// `raw`, so a name that breaks its schema is placed by its line, never printed.
fn path_display(raw: &str, path: &[PathSeg]) -> String {
    let parts: Vec<crate::config::PathPart<'_>> = path
        .iter()
        .map(|seg| match seg {
            PathSeg::Key(key) => crate::config::PathPart::Key(key),
            PathSeg::Index(index) => crate::config::PathPart::Index(*index),
        })
        .collect();
    crate::config::shown_parts(raw, &parts)
}

/// The path's keys when it holds no array index, for drop-list matching
/// segment by segment: `projects[0].auth` never collides with the top-level
/// `[auth]`, and a top-level key named `"server.acme"` never with
/// `[server.acme]`.
fn key_segments(path: &[PathSeg]) -> Option<Vec<&str>> {
    path.iter()
        .map(|seg| match seg {
            PathSeg::Key(key) => Some(key.as_str()),
            PathSeg::Index(_) => None,
        })
        .collect()
}

/// Whether `item` holds a `password_hash` key anywhere inside it.
fn holds_password_hash(item: &Item) -> bool {
    match item {
        Item::Table(table) => table
            .iter()
            .any(|(key, child)| key == "password_hash" || holds_password_hash(child)),
        Item::Value(Value::InlineTable(table)) => table.iter().any(|(key, child)| {
            key == "password_hash" || holds_password_hash(&Item::Value(child.clone()))
        }),
        Item::ArrayOfTables(array) => array.iter().any(|entry| {
            entry
                .iter()
                .any(|(key, child)| key == "password_hash" || holds_password_hash(child))
        }),
        _ => false,
    }
}

/// Whether `path` is exactly an orphaned section, segment by segment.
fn is_orphan_root(path: &[PathSeg]) -> bool {
    key_segments(path).is_some_and(|keys| ORPHANED_CONFIG_SECTIONS.contains(&keys.as_slice()))
}

/// What the documentation-restore merge did to a user's non-canonical content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreMergeReport {
    /// Dotted paths of orphaned sections that were removed.
    pub dropped: Vec<String>,
    /// Dotted paths of unknown keys that were carried over verbatim.
    pub preserved: Vec<String>,
    /// Dotted paths of unknown keys the merge could NOT place anywhere in the
    /// rendered document, and which are therefore absent from the result.
    ///
    /// Distinct from [`Self::dropped`], which names sections dux removes ON
    /// PURPOSE. This list names a failure, and it exists so the failure can
    /// never be silent. It is empty for every config the canonical renderer
    /// can produce (see `insert_at_path`), so a non-empty one is a bug report.
    pub unplaceable: Vec<String>,
}

impl RestoreMergeReport {
    pub fn is_empty(&self) -> bool {
        self.dropped.is_empty() && self.preserved.is_empty() && self.unplaceable.is_empty()
    }
}

/// Copy every key of `original` that the freshly-rendered `rendered` document
/// does not already carry into `rendered`, EXCEPT keys under
/// [`ORPHANED_CONFIG_SECTIONS`], which are dropped. Returns what happened.
///
/// This is the safety net under "restore the documentation": the canonical
/// renderer only emits keys dux knows about, so re-rendering a user's config
/// from its parsed [`Config`] would otherwise silently discard anything else in
/// the file. Values are moved as `toml_edit` items, so a preserved key keeps its
/// own formatting and its own trailing/leading comments.
pub fn merge_unmanaged_keys(
    rendered: &mut DocumentMut,
    original: &DocumentMut,
) -> RestoreMergeReport {
    let mut report = RestoreMergeReport::default();
    let mut carry: Vec<CarriedLeaf> = Vec::new();
    let mut path = Vec::new();
    let raw = original.to_string();
    collect_unmanaged(
        &raw,
        original.as_table(),
        Some(rendered.as_table()),
        &mut path,
        &mut carry,
        &mut report.dropped,
    );

    for leaf in carry {
        let display = path_display(&raw, &leaf.path);
        if insert_at_path(rendered, &leaf.path, leaf.key, leaf.item) {
            report.preserved.push(display);
        } else {
            // Neither preserved nor deliberately dropped: the key is GONE and
            // the user has to be told. Falling through to neither list is how
            // a merge loses data in silence.
            report.unplaceable.push(display);
        }
    }
    report.dropped.sort();
    report.dropped.dedup();
    report.preserved.sort();
    report.unplaceable.sort();
    report
}

/// A key the rendered document lacks, captured with its own `Key` so that the
/// comment attached to it (which lives on the key's decor, not the item's)
/// travels with it into the restored file.
struct CarriedLeaf {
    path: Vec<PathSeg>,
    key: Key,
    item: Item,
}

/// Walk `orig` alongside its counterpart in the rendered document, collecting
/// leaf keys the rendered document lacks and noting dropped orphan sections.
fn collect_unmanaged(
    raw: &str,
    orig: &Table,
    rendered: Option<&Table>,
    path: &mut Vec<PathSeg>,
    carry: &mut Vec<CarriedLeaf>,
    dropped: &mut Vec<String>,
) {
    for (key, item) in orig.iter() {
        path.push(PathSeg::Key(key.to_string()));

        // An orphaned section holding a password hash is never cleaned up:
        // that is a password in the wrong place, and the load refuses the
        // file over it (see `misplaced_auth_problems`) rather than lose it.
        if is_orphan_root(path) && !holds_password_hash(item) {
            // Report the section once and do not descend: everything beneath it
            // goes away with it.
            dropped.push(path_display(raw, path));
            path.pop();
            continue;
        }

        match item {
            Item::Table(table) => {
                let counterpart = rendered.and_then(|r| r.get(key)).and_then(Item::as_table);
                collect_unmanaged(raw, table, counterpart, path, carry, dropped);
            }
            Item::ArrayOfTables(arrays) => {
                let counterpart = rendered
                    .and_then(|r| r.get(key))
                    .and_then(Item::as_array_of_tables);
                for (index, table) in arrays.iter().enumerate() {
                    path.push(PathSeg::Index(index));
                    collect_unmanaged(
                        raw,
                        table,
                        counterpart.and_then(|a| a.get(index)),
                        path,
                        carry,
                        dropped,
                    );
                    path.pop();
                }
            }
            leaf => {
                let already_rendered = rendered.map(|r| r.contains_key(key)).unwrap_or(false);
                if !already_rendered && let Some(owned_key) = orig.key(key) {
                    carry.push(CarriedLeaf {
                        path: path.clone(),
                        key: owned_key.clone(),
                        item: leaf.clone(),
                    });
                }
            }
        }
        path.pop();
    }
}

/// Insert `item` at `path` in `doc`, creating intermediate tables as needed.
///
/// Returns false when the path cannot be materialized: the only such case is an
/// array-of-tables index the rendered document does not have (dux cannot invent
/// a `[[projects]]` entry that the canonical renderer did not emit).
fn insert_at_path(doc: &mut DocumentMut, path: &[PathSeg], key: Key, item: Item) -> bool {
    let Some((PathSeg::Key(_), parents)) = path.split_last() else {
        return false;
    };

    let mut table: &mut Table = doc.as_table_mut();
    let mut step = 0;
    while step < parents.len() {
        let PathSeg::Key(key) = &parents[step] else {
            // An index never leads a path: it always follows the key naming the
            // array it indexes into.
            return false;
        };

        if let Some(PathSeg::Index(index)) = parents.get(step + 1) {
            // `key[index]`: descend into an existing array-of-tables entry. dux
            // cannot invent an entry the canonical renderer did not emit, so a
            // missing array or index means "cannot preserve here".
            let Some(arrays) = table.get_mut(key).and_then(Item::as_array_of_tables_mut) else {
                return false;
            };
            let Some(next) = arrays.get_mut(*index) else {
                return false;
            };
            table = next;
            step += 2;
            continue;
        }

        // Plain table step, creating an implicit table when absent. An implicit
        // table emits no `[header]` line of its own, so a synthetic parent that
        // exists only to hold a preserved leaf adds no stray empty section.
        let entry = table.entry(key).or_insert_with(|| {
            let mut fresh = Table::new();
            fresh.set_implicit(true);
            Item::Table(fresh)
        });
        let Some(next) = entry.as_table_mut() else {
            return false;
        };
        table = next;
        step += 1;
    }

    // `insert_formatted` (rather than `insert`) is what carries the key's own
    // decor (including any comment written above it) into the restored file.
    table.insert_formatted(&key, item);
    true
}

/// Escape triple-quotes in a TOML multiline basic string.
///
/// Per the TOML spec, `"""` inside `"""..."""` can be included by escaping at
/// least one quote: `""\"`. Public because the TUI's canonical renderer reuses
/// it for the same multiline fields.
pub fn escape_toml_multiline(value: &str) -> String {
    value.replace("\"\"\"", "\"\"\\\"")
}

#[cfg(test)]
#[allow(deprecated)] // tests call the deprecated wrappers directly to verify their behaviour
mod tests {
    use super::*;

    /// A pure REORDER of the macro set must reach the file. `[macros]` order is
    /// meaningful (the macro bar, quick-picker, and web editor all render in
    /// declaration order), and the web editor now reorders by drag-and-drop
    /// through the same wholesale `update_macros` save. Writing an existing key
    /// with `table[name] = ...` keeps its old toml_edit position, so before this
    /// was pinned a reorder round-tripped through the in-memory config but the
    /// file kept the old order and a dux restart silently undid the drag.
    #[test]
    fn patch_macros_writes_entries_in_config_order() {
        let raw = "\
[macros]
# my review macro
review = { text = \"review this\", surface = \"agent\" }
build = { text = \"cargo build\", surface = \"terminal\" }
deploy = { text = \"deploy it\", surface = \"both\" }
";
        let mut doc: DocumentMut = raw.parse().expect("parse");

        // Same three entries, reordered: deploy, review, build.
        let mut macros = MacrosConfig::default();
        for (name, text, surface) in [
            ("deploy", "deploy it", crate::config::MacroSurface::Both),
            ("review", "review this", crate::config::MacroSurface::Agent),
            (
                "build",
                "cargo build",
                crate::config::MacroSurface::Terminal,
            ),
        ] {
            macros.entries.insert(
                name.to_string(),
                crate::config::MacroEntry {
                    text: text.to_string(),
                    surface,
                },
            );
        }
        patch_macros(&mut doc, &macros);

        let saved = doc.to_string();
        let pos = |needle: &str| {
            saved
                .find(needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        assert!(
            pos("deploy") < pos("review") && pos("review") < pos("build"),
            "entries must be written in the new order, got:\n{saved}"
        );
        // The comment above `review` travels with its key through the reorder.
        assert!(
            pos("# my review macro") < pos("review ="),
            "the key's comment must survive and stay attached, got:\n{saved}"
        );

        // The reparsed config sees the new order, so a restart keeps it.
        let reparsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(
            reparsed.macros.entries.keys().collect::<Vec<_>>(),
            vec!["deploy", "review", "build"]
        );
    }

    /// The reorder rewrite must not clobber entries that changed CONTENT in the
    /// same save, and stale keys still disappear.
    #[test]
    fn patch_macros_reorder_carries_edits_and_removals() {
        let raw = "\
[macros]
review = { text = \"review this\", surface = \"agent\" }
gone = { text = \"bye\", surface = \"both\" }
build = { text = \"cargo build\", surface = \"terminal\" }
";
        let mut doc: DocumentMut = raw.parse().expect("parse");

        let mut macros = MacrosConfig::default();
        macros.entries.insert(
            "build".to_string(),
            crate::config::MacroEntry {
                text: "cargo build --release".to_string(),
                surface: crate::config::MacroSurface::Terminal,
            },
        );
        macros.entries.insert(
            "review".to_string(),
            crate::config::MacroEntry {
                text: "review this".to_string(),
                surface: crate::config::MacroSurface::Agent,
            },
        );
        patch_macros(&mut doc, &macros);

        let saved = doc.to_string();
        assert!(
            !saved.contains("gone"),
            "stale key must be removed:\n{saved}"
        );
        let reparsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(
            reparsed.macros.entries.keys().collect::<Vec<_>>(),
            vec!["build", "review"]
        );
        assert_eq!(
            reparsed.macros.entries["build"].text,
            "cargo build --release"
        );
    }

    #[test]
    fn write_config_atomic_writes_0600_and_no_temp_left() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");

        write_config_atomic(&path, "[env]\nFOO = \"bar\"\n", Durability::Fsync).expect("write");

        let saved = fs::read_to_string(&path).expect("read");
        assert!(saved.contains("FOO = \"bar\""));
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config must be 0600, got {mode:o}");

        // No leftover temp files in the config directory. The write lock's
        // file stays by design (unlinking a lock file someone may be waiting
        // on would let two writers lock two different files), and is private.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != "config.toml" && e.file_name() != CONFIG_WRITE_LOCK_NAME)
            .collect();
        assert!(leftovers.is_empty(), "temp file leaked: {leftovers:?}");
        let lock_mode = fs::metadata(ConfigFileLock::lock_path(&path))
            .expect("lock meta")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            lock_mode, 0o600,
            "the lock file is private, got {lock_mode:o}"
        );
    }

    /// A REPLACE, not an edit in place, and the mode of the destination does
    /// not survive it. The write goes to a fresh `0600` temp file which is then
    /// `rename`d over the original, and a rename needs write permission on the
    /// DIRECTORY, not on the file, so a `config.toml` the user deliberately
    /// made read-only at `0400` is replaced anyway and comes back `0600`.
    ///
    /// This is pinned because the shipped template comment used to promise the
    /// opposite. "dux only ever removes group and world access, so a `0400`
    /// config stays read-only" is true of the tightening pass in
    /// [`crate::file_modes`] and false of this function, and the comment did
    /// not say which one it was talking about.
    ///
    /// Preserving the destination mode instead was considered and deliberately
    /// NOT done. It would carry a `0644` forward on every save, undoing the
    /// startup tightening for a file that holds `[env]` tokens, and preserving
    /// `0400` would make dux look as though it had honoured the read-only file
    /// while having replaced its contents regardless, which is the more
    /// misleading of the two outcomes. The honest fix was to stop claiming it.
    #[test]
    fn write_config_atomic_replaces_a_read_only_config_and_resets_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[env]\nOLD = \"1\"\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();

        write_config_atomic(&path, "[env]\nNEW = \"2\"\n", Durability::Fsync).expect("write");

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[env]\nNEW = \"2\"\n",
            "the read-only file was replaced; nothing here refuses the write"
        );
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "owner write comes back, got {mode:o}");
    }

    #[test]
    fn root_key_renders_before_tables_and_round_trips() {
        let rendered = render_config_plain(&Config::default());

        // The dotless root key must appear before the first table header, or TOML
        // would bind it to a table. Guards the toml_edit ordering assumption.
        let first_table = rendered.find('[').expect("rendered config has tables");
        let key_pos = rendered
            .find("shutdown_timeout_seconds")
            .expect("root shutdown_timeout_seconds present");
        assert!(
            key_pos < first_table,
            "root shutdown_timeout_seconds must render before any table:\n{rendered}"
        );

        // And it must parse back to the defaults (30 at root and under [server]).
        let parsed: Config = toml::from_str(&rendered).expect("rendered config re-parses");
        assert_eq!(parsed.shutdown_timeout_seconds, 30);
        assert_eq!(parsed.server.shutdown_timeout_seconds, 30);
    }

    #[test]
    fn patch_adds_root_key_to_existing_file_without_corruption() {
        // An existing user file already full of tables and comments, the worst
        // case for appending a dotless root key.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        let original = "# my dux config\n\
                        [defaults]\n\
                        provider = \"claude\"\n\n\
                        # keep this comment\n\
                        [server]\n\
                        port = 9000\n";
        fs::write(&path, original).expect("seed config");

        let config = Config {
            shutdown_timeout_seconds: 12,
            server: crate::config::ServerConfig {
                shutdown_timeout_seconds: 7,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");

        let saved = fs::read_to_string(&path).expect("read back");
        // Must still be valid TOML and the root key must not have been swallowed
        // into [server] (which would make it parse as 0/default, not 12).
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(parsed.shutdown_timeout_seconds, 12, "saved:\n{saved}");
        assert_eq!(parsed.server.shutdown_timeout_seconds, 7);
        // User comments are preserved by the surgical patch.
        assert!(saved.contains("# keep this comment"), "saved:\n{saved}");
    }

    #[test]
    fn zero_timeout_round_trips() {
        let config = Config {
            shutdown_timeout_seconds: 0,
            server: crate::config::ServerConfig {
                shutdown_timeout_seconds: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.shutdown_timeout_seconds, 0);
        assert_eq!(parsed.server.shutdown_timeout_seconds, 0);
    }

    #[test]
    fn compose_bar_renders_and_round_trips() {
        // The default ("auto") renders and re-parses.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.ui.compose_bar, "auto");

        // A user-set mode survives a regenerate. This is the half that catches
        // a missing `patch_table_str` line: with the key absent from the
        // render, the re-parse would silently fall back to the default.
        for mode in ["always", "never"] {
            let config = Config {
                ui: crate::config::UiConfig {
                    compose_bar: mode.to_string(),
                    ..Default::default()
                },
                ..Default::default()
            };
            let rendered = render_config_plain(&config);
            let parsed: Config = toml::from_str(&rendered).expect("re-parse");
            assert_eq!(parsed.ui.compose_bar, mode);
        }
    }

    /// The bool-to-enum migration survives the SAVE path, not just the load
    /// path: a config file still holding the old boolean is read through
    /// `deserialize_compose_bar` and written back out as the string form, so
    /// the legacy value is retired the first time anything saves.
    #[test]
    fn a_legacy_boolean_compose_bar_is_rewritten_as_a_mode_on_save() {
        for (legacy, expected) in [("true", "auto"), ("false", "never")] {
            let parsed: Config = toml::from_str(&format!("[ui]\ncompose_bar = {legacy}\n"))
                .expect("a legacy boolean must still parse");
            assert_eq!(parsed.ui.compose_bar, expected);

            let rendered = render_config_plain(&parsed);
            assert!(
                rendered.contains(&format!("compose_bar = \"{expected}\"")),
                "saved config must carry the string form, got:\n{rendered}"
            );
        }
    }

    #[test]
    fn the_accessory_bar_preference_renders_and_round_trips() {
        // Defaults true, and the default renders and re-parses.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert!(parsed.ui.mobile_accessory_bar);

        // A user-set false survives a regenerate. Same shape as
        // `compose_bar_renders_and_round_trips` above: this is the half that
        // catches a missing `patch_table_bool` line, where the re-parse would
        // silently fall back to the default (true).
        let config = Config {
            ui: crate::config::UiConfig {
                mobile_accessory_bar: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert!(!parsed.ui.mobile_accessory_bar);
    }

    #[test]
    fn first_load_screen_opt_outs_default_off_and_round_trip() {
        // Both screens are on by default, so both DISABLE flags default false.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert!(!parsed.ui.disable_automated_welcome_screen);
        assert!(!parsed.ui.disable_release_notes);

        // A user-set true survives a regenerate: without the `patch_table_bool`
        // lines the key would be absent and silently fall back to false, which
        // would re-enable a screen the user turned off.
        let config = Config {
            ui: crate::config::UiConfig {
                disable_automated_welcome_screen: true,
                disable_release_notes: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert!(parsed.ui.disable_automated_welcome_screen);
        assert!(parsed.ui.disable_release_notes);
    }

    #[test]
    fn search_index_max_files_defaults_and_round_trips() {
        // The default renders and re-parses to 50 000.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.server.search_index_max_files,
            crate::config::DEFAULT_SEARCH_INDEX_MAX_FILES
        );

        // A user-set value survives a regenerate.
        let config = Config {
            server: crate::config::ServerConfig {
                search_index_max_files: 1234,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.server.search_index_max_files, 1234);
    }

    #[test]
    fn log_viewer_lines_defaults_and_round_trips() {
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.server.log_viewer_lines,
            crate::config::DEFAULT_LOG_VIEWER_LINES
        );
        let config = Config {
            server: crate::config::ServerConfig {
                log_viewer_lines: 321,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.server.log_viewer_lines, 321);
    }

    #[test]
    fn log_viewer_lines_is_read_within_its_documented_bounds() {
        use crate::config::{LOG_VIEWER_LINES_MAX, effective_log_viewer_lines};
        assert_eq!(effective_log_viewer_lines(0), 1);
        assert_eq!(effective_log_viewer_lines(500), 500);
        assert_eq!(
            effective_log_viewer_lines(LOG_VIEWER_LINES_MAX + 1),
            LOG_VIEWER_LINES_MAX
        );
    }

    #[test]
    fn log_viewer_lines_user_value_survives_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[server]\nlog_viewer_lines = 777\n").expect("seed config");
        let config = Config {
            server: crate::config::ServerConfig {
                log_viewer_lines: 4321,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(parsed.server.log_viewer_lines, 4321, "saved:\n{saved}");
    }

    #[test]
    fn search_index_max_files_user_value_survives_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        // Seed a DIFFERENT value than the patch target below, so the assertion
        // can only pass if patch_config_file_with actually wrote the new value
        // rather than leaving the seeded file untouched.
        fs::write(&path, "[server]\nsearch_index_max_files = 777\n").expect("seed config");

        let config = Config {
            server: crate::config::ServerConfig {
                search_index_max_files: 4321,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(
            parsed.server.search_index_max_files, 4321,
            "saved:\n{saved}"
        );
    }

    /// The browser-side reconnect timings render at their defaults, parse
    /// back, and a user value survives a regenerate. They are read live by the
    /// browser off the bootstrap document, so a wrong value is silent until
    /// somebody's phone loses its socket.
    #[test]
    fn reconnect_timing_settings_default_and_round_trip() {
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.server.replay_wait_seconds,
            crate::config::DEFAULT_REPLAY_WAIT_SECONDS
        );
        assert_eq!(
            parsed.server.reconnect_backoff_cap_seconds,
            crate::config::DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS
        );
        assert_eq!(
            parsed.server.heartbeat_seconds,
            crate::config::DEFAULT_HEARTBEAT_SECONDS
        );
        assert_eq!(
            parsed.server.heartbeat_deadline_seconds,
            crate::config::DEFAULT_HEARTBEAT_DEADLINE_SECONDS
        );
        assert_eq!(
            parsed.server.pty_send_timeout_seconds,
            crate::config::DEFAULT_PTY_SEND_TIMEOUT_SECONDS
        );
        assert_eq!(
            parsed.server.reconnect_attempts,
            crate::config::DEFAULT_RECONNECT_ATTEMPTS
        );
        assert_eq!(
            parsed.server.reconnect_attempt_timeout_seconds,
            crate::config::DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS
        );
        assert_eq!(
            parsed.server.changes_request_timeout_seconds,
            crate::config::DEFAULT_CHANGES_REQUEST_TIMEOUT_SECONDS
        );

        let config = Config {
            server: crate::config::ServerConfig {
                replay_wait_seconds: 21,
                reconnect_backoff_cap_seconds: 22,
                heartbeat_seconds: 23,
                heartbeat_deadline_seconds: 24,
                pty_send_timeout_seconds: 25,
                reconnect_attempts: 26,
                reconnect_attempt_timeout_seconds: 27,
                changes_request_timeout_seconds: 28,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.server.replay_wait_seconds, 21);
        assert_eq!(parsed.server.reconnect_backoff_cap_seconds, 22);
        assert_eq!(parsed.server.heartbeat_seconds, 23);
        assert_eq!(parsed.server.heartbeat_deadline_seconds, 24);
        assert_eq!(parsed.server.pty_send_timeout_seconds, 25);
        assert_eq!(parsed.server.reconnect_attempts, 26);
        assert_eq!(parsed.server.reconnect_attempt_timeout_seconds, 27);
        assert_eq!(parsed.server.changes_request_timeout_seconds, 28);
    }

    /// And a patch rewrites each of them in a file that already carries other
    /// values, rather than leaving the seeded ones behind.
    #[test]
    fn reconnect_timing_user_values_survive_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[server]\nreplay_wait_seconds = 1\nreconnect_backoff_cap_seconds = 2\n\
             heartbeat_seconds = 3\nheartbeat_deadline_seconds = 4\n\
             pty_send_timeout_seconds = 5\nreconnect_attempts = 6\n\
             reconnect_attempt_timeout_seconds = 7\n\
             changes_request_timeout_seconds = 8\n",
        )
        .expect("seed config");

        let config = Config {
            server: crate::config::ServerConfig {
                replay_wait_seconds: 31,
                reconnect_backoff_cap_seconds: 32,
                heartbeat_seconds: 33,
                heartbeat_deadline_seconds: 34,
                pty_send_timeout_seconds: 35,
                reconnect_attempts: 36,
                reconnect_attempt_timeout_seconds: 37,
                changes_request_timeout_seconds: 38,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(parsed.server.replay_wait_seconds, 31, "saved:\n{saved}");
        assert_eq!(
            parsed.server.reconnect_backoff_cap_seconds, 32,
            "saved:\n{saved}"
        );
        assert_eq!(parsed.server.heartbeat_seconds, 33, "saved:\n{saved}");
        assert_eq!(
            parsed.server.heartbeat_deadline_seconds, 34,
            "saved:\n{saved}"
        );
        assert_eq!(
            parsed.server.pty_send_timeout_seconds, 35,
            "saved:\n{saved}"
        );
        assert_eq!(parsed.server.reconnect_attempts, 36, "saved:\n{saved}");
        assert_eq!(
            parsed.server.reconnect_attempt_timeout_seconds, 37,
            "saved:\n{saved}"
        );
        assert_eq!(
            parsed.server.changes_request_timeout_seconds, 38,
            "saved:\n{saved}"
        );
    }

    #[test]
    fn tree_list_max_concurrency_defaults_and_round_trips() {
        // The default renders and re-parses to 8.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.server.tree_list_max_concurrency,
            crate::config::DEFAULT_TREE_LIST_MAX_CONCURRENCY
        );

        // A user-set value survives a regenerate.
        let config = Config {
            server: crate::config::ServerConfig {
                tree_list_max_concurrency: 123,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.server.tree_list_max_concurrency, 123);
    }

    #[test]
    fn tree_list_max_concurrency_user_value_survives_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        // Seed a DIFFERENT value than the patch target below, so the assertion
        // can only pass if patch_config_file_with actually wrote the new value
        // rather than leaving the seeded file untouched.
        fs::write(&path, "[server]\ntree_list_max_concurrency = 3\n").expect("seed config");

        let config = Config {
            server: crate::config::ServerConfig {
                tree_list_max_concurrency: 16,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(
            parsed.server.tree_list_max_concurrency, 16,
            "saved:\n{saved}"
        );
    }

    #[test]
    fn file_drop_settings_default_and_round_trip() {
        // The size default is the one the maintainer chose against real
        // screenshots, and it is deliberately far above the web framework's own
        // 2 MB body limit, which is the whole reason the route sets it.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.server.file_drop_max_bytes,
            crate::config::DEFAULT_FILE_DROP_MAX_BYTES
        );
        assert_eq!(parsed.server.file_drop_max_bytes, 104_857_600);
        assert_eq!(
            parsed.server.file_drop_max_concurrency,
            crate::config::DEFAULT_FILE_DROP_MAX_CONCURRENCY
        );

        // Both survive a regenerate, including the `0` that switches file drop
        // off: a zero that silently reverted to the default would turn the
        // documented opt-out into a no-op.
        let config = Config {
            server: crate::config::ServerConfig {
                file_drop_max_bytes: 0,
                file_drop_max_concurrency: 7,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.server.file_drop_max_bytes, 0);
        assert_eq!(parsed.server.file_drop_max_concurrency, 7);
    }

    #[test]
    fn upload_settings_default_and_round_trip() {
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.ui.upload_directory,
            crate::config::DEFAULT_UPLOAD_DIRECTORY
        );
        assert!(parsed.ui.upload_write_gitignore);

        // The opt-out is the value that must survive: a `false` reverting to
        // the default would silently start hiding files from git again for a
        // user who deliberately wants to commit what they drop.
        let mut config = Config::default();
        config.ui.upload_directory = "tmp/drops".to_string();
        config.ui.upload_write_gitignore = false;
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.ui.upload_directory, "tmp/drops");
        assert!(!parsed.ui.upload_write_gitignore);
    }

    #[test]
    fn upload_user_values_survive_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[ui]\nupload_directory = \"old\"\nupload_write_gitignore = true\n",
        )
        .expect("seed config");

        let mut config = Config::default();
        config.ui.upload_directory = "tmp/drops".to_string();
        config.ui.upload_write_gitignore = false;
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(parsed.ui.upload_directory, "tmp/drops", "saved:\n{saved}");
        assert!(!parsed.ui.upload_write_gitignore, "saved:\n{saved}");
    }

    #[test]
    fn file_drop_user_values_survive_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[server]\nfile_drop_max_bytes = 1\nfile_drop_max_concurrency = 1\n",
        )
        .expect("seed config");

        let config = Config {
            server: crate::config::ServerConfig {
                file_drop_max_bytes: 5_000_000,
                file_drop_max_concurrency: 3,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(
            parsed.server.file_drop_max_bytes, 5_000_000,
            "saved:\n{saved}"
        );
        assert_eq!(
            parsed.server.file_drop_max_concurrency, 3,
            "saved:\n{saved}"
        );
    }

    #[test]
    fn release_notes_max_concurrency_defaults_and_round_trips() {
        // The default renders and re-parses to the documented small bound.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.server.release_notes_max_concurrency,
            crate::config::DEFAULT_RELEASE_NOTES_MAX_CONCURRENCY
        );

        // A user-set value survives a regenerate.
        let config = Config {
            server: crate::config::ServerConfig {
                release_notes_max_concurrency: 9,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.server.release_notes_max_concurrency, 9);
    }

    #[test]
    fn release_notes_max_concurrency_user_value_survives_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        // Seed a DIFFERENT value than the patch target below, so the assertion
        // can only pass if patch_config_file_with actually wrote the new value.
        fs::write(&path, "[server]\nrelease_notes_max_concurrency = 1\n").expect("seed config");

        let config = Config {
            server: crate::config::ServerConfig {
                release_notes_max_concurrency: 5,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(
            parsed.server.release_notes_max_concurrency, 5,
            "saved:\n{saved}"
        );
    }

    #[test]
    fn forward_scroll_tri_state_deserializes() {
        // Absent key -> None; explicit true/false -> Some(..).
        let absent: crate::config::ProviderCommandConfig =
            toml::from_str("command = \"claude\"\n").expect("parse absent");
        assert_eq!(absent.forward_scroll, None);

        let yes: crate::config::ProviderCommandConfig =
            toml::from_str("command = \"opencode\"\nforward_scroll = true\n").expect("parse true");
        assert_eq!(yes.forward_scroll, Some(true));

        let no: crate::config::ProviderCommandConfig =
            toml::from_str("command = \"codex\"\nforward_scroll = false\n").expect("parse false");
        assert_eq!(no.forward_scroll, Some(false));
    }

    #[test]
    fn patch_omits_forward_scroll_when_none_and_writes_when_some() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[defaults]\nprovider = \"claude\"\n").expect("write initial");

        let mut config = Config::default();
        // Set explicit tri-state values to exercise the writer (None omits the
        // key, Some writes it); defaults are all None (auto) regardless.
        if let Some(claude) = config.providers.commands.get_mut("claude") {
            claude.forward_scroll = None;
        }
        if let Some(opencode) = config.providers.commands.get_mut("opencode") {
            opencode.forward_scroll = Some(true);
        }
        let codex = config
            .providers
            .commands
            .get_mut("codex")
            .expect("codex provider exists");
        codex.forward_scroll = Some(false);

        patch_config_file(&config_path, &config).expect("patch");
        let saved = fs::read_to_string(&config_path).expect("read back");

        // Round-trips back to the same tri-state values.
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(
            parsed
                .providers
                .commands
                .get("claude")
                .unwrap()
                .forward_scroll,
            None,
            "absent key must parse back to None: {saved}"
        );
        assert_eq!(
            parsed
                .providers
                .commands
                .get("opencode")
                .unwrap()
                .forward_scroll,
            Some(true)
        );
        assert_eq!(
            parsed
                .providers
                .commands
                .get("codex")
                .unwrap()
                .forward_scroll,
            Some(false)
        );

        // The writer omits the key for None and writes it for Some.
        let claude_section = saved
            .split("[providers.claude]")
            .nth(1)
            .and_then(|s| s.split("[providers.").next())
            .unwrap_or("");
        assert!(
            !claude_section.contains("forward_scroll"),
            "None must omit forward_scroll; got: {claude_section}"
        );
    }

    #[test]
    fn patch_round_trips_web_dragdrop_paste_and_omits_it_when_unset() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[defaults]\nprovider = \"claude\"\n").expect("write initial");

        let mut config = Config::default();
        // A provider the user added themselves: no form of its own, so the key
        // must not appear at all rather than be invented as "bare".
        config.providers.commands.insert(
            "myagent".to_string(),
            crate::config::ProviderCommandConfig {
                command: "myagent".to_string(),
                ..Default::default()
            },
        );
        config
            .providers
            .commands
            .get_mut("opencode")
            .expect("opencode provider exists")
            .web_dragdrop_paste = Some("backslash_escaped".to_string());

        patch_config_file(&config_path, &config).expect("patch");
        let saved = fs::read_to_string(&config_path).expect("read back");

        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(
            parsed.providers.commands["codex"].resolved_web_dragdrop_paste(),
            crate::config::WebDragDropPaste::SingleQuoted,
            "the shipped form must survive a save: {saved}"
        );
        assert_eq!(
            parsed.providers.commands["opencode"].resolved_web_dragdrop_paste(),
            crate::config::WebDragDropPaste::BackslashEscaped,
            "an explicit user value must survive a save: {saved}"
        );
        assert_eq!(
            parsed.providers.commands["myagent"].web_dragdrop_paste, None,
            "a provider with no form must not gain one: {saved}"
        );
        assert_eq!(
            parsed.providers.commands["myagent"].resolved_web_dragdrop_paste(),
            crate::config::WebDragDropPaste::Bare,
            "and an absent key resolves to bare"
        );

        let myagent_section = saved
            .split("[providers.myagent]")
            .nth(1)
            .and_then(|s| s.split("[providers.").next())
            .unwrap_or("");
        assert!(
            !myagent_section.contains("web_dragdrop_paste"),
            "None must omit the key; got: {myagent_section}"
        );
    }

    #[test]
    fn patch_preserves_comments_and_unknown_keys() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "\
# A user comment that must survive
[env]
EXISTING = \"keep-me\"

[some_unknown_section]
unknown_key = \"untouched\"
",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.env.insert("FOO".to_string(), "bar".to_string());

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            saved.contains("# A user comment that must survive"),
            "user comment lost: {saved}"
        );
        assert!(
            saved.contains("unknown_key = \"untouched\""),
            "unknown key lost: {saved}"
        );
        assert!(
            saved.contains("FOO = \"bar\""),
            "new value missing: {saved}"
        );
    }

    #[test]
    fn patch_writes_env() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[defaults]\nprovider = \"claude\"\n").expect("write initial");

        let mut config = Config::default();
        config.env.insert("FOO".to_string(), "bar".to_string());

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.env.get("FOO").map(String::as_str), Some("bar"));
    }

    #[test]
    fn patch_writes_project_fields() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[defaults]\nprovider = \"claude\"\n").expect("write initial");

        let mut config = Config::default();
        let mut env = BTreeMap::new();
        env.insert("KEY".to_string(), "value".to_string());
        config.projects.push(ProjectConfig {
            id: "project-1".to_string(),
            path: "/home/user/project".to_string(),
            name: Some("test".to_string()),
            default_provider: Some("codex".to_string()),
            leading_branch: None,
            auto_reopen_agents: Some(true),
            startup_command: Some("npm install".to_string()),
            env,
        });

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.projects.len(), 1);
        let project = &parsed.projects[0];
        assert_eq!(project.default_provider.as_deref(), Some("codex"));
        assert_eq!(project.startup_command.as_deref(), Some("npm install"));
        assert_eq!(project.auto_reopen_agents, Some(true));
        assert_eq!(project.env.get("KEY").map(String::as_str), Some("value"));
    }

    /// A project entry is REBUILT on every save, so anything the user put in it
    /// that dux does not manage has to be captured first or it is deleted. This
    /// was a real loss: an upgrade test caught `custom_note` vanishing from a
    /// hand-edited `[[projects]]` block on the first save after the upgrade.
    #[test]
    fn patch_keeps_a_hand_added_key_inside_a_project_entry() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\n\
             id = \"project-1\"\n\
             path = \"/home/user/project\"\n\
             # why this project matters\n\
             custom_note = \"hand added\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(ProjectConfig {
            id: "project-1".to_string(),
            path: "/home/user/project".to_string(),
            name: Some("renamed".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
        });

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            saved.contains("custom_note = \"hand added\""),
            "the hand-added key was deleted by the save: {saved}"
        );
        // The comment lives on the key's decor, so it travels with it.
        assert!(
            saved.contains("# why this project matters"),
            "the comment attached to the carried key was lost: {saved}"
        );
        // ...and the managed edit still landed.
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.projects[0].name.as_deref(), Some("renamed"));
    }

    /// The carry must not resurrect `leading_branch`. It is a `ProjectConfig`
    /// field the loader reads (to repair SQLite) and the writer deliberately
    /// omits, so "not a managed key" is not enough to decide it belongs to the
    /// user.
    ///
    /// The second case is a base "Change base branch" just moved: the project
    /// in memory carries a base the file never had, and it stays out too.
    #[test]
    fn patch_still_drops_leading_branch_rather_than_carrying_it_over() {
        for (initial, base) in [
            (
                "[[projects]]\nid = \"project-1\"\npath = \"/p\"\nleading_branch = \"trunk\"\n",
                "trunk",
            ),
            (
                "[[projects]]\nid = \"project-1\"\npath = \"/p\"\n",
                "develop",
            ),
        ] {
            let dir = tempfile::TempDir::new().expect("tempdir");
            let config_path = dir.path().join("config.toml");
            fs::write(&config_path, initial).expect("write initial");

            let mut config = Config::default();
            config.projects.push(ProjectConfig {
                id: "project-1".to_string(),
                path: "/p".to_string(),
                name: None,
                default_provider: None,
                leading_branch: Some(base.to_string()),
                auto_reopen_agents: None,
                startup_command: None,
                env: BTreeMap::new(),
            });

            patch_config_file(&config_path, &config).expect("patch");

            let saved = fs::read_to_string(&config_path).expect("read back");
            assert!(
                !saved.contains("leading_branch") && !saved.contains(base),
                "derived branch state must never be pinned back into config: {saved}"
            );
        }
    }

    /// The carry is keyed by project id, not by position, so removing the first
    /// of two projects must not move the second one's keys onto a stranger.
    #[test]
    fn carried_project_keys_follow_the_id_when_a_project_is_removed() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\nid = \"a\"\npath = \"/a\"\nnote_a = 1\n\n\
             [[projects]]\nid = \"b\"\npath = \"/b\"\nnote_b = 2\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(ProjectConfig {
            id: "b".to_string(),
            path: "/b".to_string(),
            name: None,
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
        });

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(saved.contains("note_b = 2"), "{saved}");
        assert!(
            !saved.contains("note_a"),
            "a removed project's keys must go with it: {saved}"
        );
    }

    /// A bare `ProjectConfig` with only the two required fields, for the carry
    /// tests below.
    fn bare_project(id: &str, path: &str) -> ProjectConfig {
        ProjectConfig {
            id: id.to_string(),
            path: path.to_string(),
            name: None,
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
        }
    }

    /// `projects = [ { ... } ]` is the OTHER legal TOML spelling of the same
    /// array. `toml` loads it identically, so a user who writes it that way has a
    /// working config, and the carry has to see it too. It used to look only for
    /// the array-of-tables spelling, so the key survived one spelling and was
    /// deleted in the other.
    #[test]
    fn patch_keeps_a_hand_added_key_written_in_the_inline_array_spelling() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "projects = [ { id = \"project-1\", path = \"/p\", custom_note = \"hand added\" } ]\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("project-1", "/p"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            saved.contains("custom_note = \"hand added\""),
            "the inline-array spelling lost the hand-added key: {saved}"
        );
    }

    /// Two entries sharing an id is a hand-edit dux itself never writes, and the
    /// project sync rejects it. The writer still must not SCRAMBLE it: each
    /// entry's own extras belong to that entry, matched by occurrence, and a key
    /// name reused across the two must not be collapsed to one value.
    #[test]
    fn duplicate_project_ids_keep_their_own_keys_rather_than_pooling_them() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\nid = \"dup\"\npath = \"/a\"\nnote = \"from-first\"\n\n\
             [[projects]]\nid = \"dup\"\npath = \"/b\"\nnote = \"from-second\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("dup", "/a"));
        config.projects.push(bare_project("dup", "/b"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let doc: DocumentMut = saved.parse().expect("reparse");
        let entries = doc
            .get("projects")
            .and_then(Item::as_array_of_tables)
            .expect("projects array");
        assert_eq!(entries.len(), 2, "{saved}");
        assert_eq!(
            entries
                .get(0)
                .and_then(|t| t.get("note"))
                .and_then(Item::as_str),
            Some("from-first"),
            "the first entry lost its own key: {saved}"
        );
        assert_eq!(
            entries
                .get(1)
                .and_then(|t| t.get("note"))
                .and_then(Item::as_str),
            Some("from-second"),
            "the second entry's key was pooled onto the first: {saved}"
        );
    }

    /// A comment block written ABOVE a `[[projects]]` header is user data, and it
    /// used to be deleted on save: `toml_edit` files it on the entry's own decor
    /// rather than on any of its keys, so carrying the keys was not enough.
    #[test]
    fn patch_keeps_the_comment_written_above_a_project_header() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "# projects I care about\n\
             [[projects]]\n\
             id = \"project-1\"\n\
             path = \"/p\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("project-1", "/p"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let lines: Vec<&str> = saved.lines().collect();
        let header = lines
            .iter()
            .position(|l| l.trim() == "[[projects]]")
            .unwrap_or_else(|| panic!("the projects header vanished: {saved}"));
        assert_eq!(
            lines.get(header.wrapping_sub(1)).map(|l| l.trim()),
            Some("# projects I care about"),
            "the comment above the header was deleted or moved: {saved}"
        );
    }

    /// A comment written after the LAST key of the last entry is NOT the entry's
    /// data in `toml_edit`'s model: it becomes the prefix of whatever item follows
    /// it, or the DOCUMENT's trailing trivia when nothing does. So a save that
    /// appends sections (which every save does, materializing keys the file
    /// predates) leaves it at the very end of the file, below those new sections.
    ///
    /// It is not lost, and it is not the projects rebuild that moves it. Re-homing
    /// it onto the entry would mean guessing that document-trailing trivia belongs
    /// to whichever block happened to be last, which would just as readily steal a
    /// comment the user wrote about the file as a whole. Measured and pinned
    /// rather than "fixed", so the next reader knows which of the two it is.
    #[test]
    fn a_comment_below_the_last_project_key_survives_but_stays_document_trailing() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\n\
             id = \"project-1\"\n\
             path = \"/p\"\n\
             # a note at the end of this entry\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("project-1", "/p"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            saved.contains("# a note at the end of this entry"),
            "the trailing comment was deleted outright: {saved}"
        );
        assert_eq!(
            saved.trim_end().lines().last().map(str::trim),
            Some("# a note at the end of this entry"),
            "trailing trivia is expected to render last, below the appended sections: {saved}"
        );
    }

    /// An entry with no `id` is a legal, loadable hand-edit: `ProjectConfig::id`
    /// carries `#[serde(default = "new_project_id")]`, so the loader mints one and
    /// the file keeps working. The capture used to SKIP such an entry, which threw
    /// away both its unmanaged keys and the comment above its header. `path` is the
    /// only other stable field the rebuild writes, so it is the fallback identity.
    #[test]
    fn patch_keeps_the_keys_and_comment_of_an_entry_that_carries_no_id() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "# my only project\n\
             [[projects]]\n\
             path = \"/p\"\n\
             custom_note = \"hand added\"\n",
        )
        .expect("write initial");

        // What the loader does with that file: it mints an id and keeps the path.
        let loaded: Config = toml::from_str(&fs::read_to_string(&config_path).expect("read"))
            .expect("a project entry with no id must still load");
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].path, "/p");
        assert!(!loaded.projects[0].id.is_empty(), "the loader mints an id");

        patch_config_file(&config_path, &loaded).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            saved.contains("custom_note = \"hand added\""),
            "an entry with no id lost its hand-added key: {saved}"
        );
        assert!(
            saved.contains("# my only project"),
            "an entry with no id lost the comment above its header: {saved}"
        );
    }

    /// A comment sitting BETWEEN the `[[projects]]` header and the first key is
    /// filed by `toml_edit` as the prefix of that first key, which is `id`, a
    /// managed key rebuilt from scratch. Carrying only the header decor therefore
    /// deleted it. The fixture puts a comment in five different positions so the
    /// test says which ones survive rather than testing one and hoping.
    #[test]
    fn patch_keeps_a_comment_in_every_position_inside_a_project_entry() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "# 1 above the header\n\
             [[projects]]\n\
             # 2 between the header and the first key\n\
             id = \"project-1\"\n\
             path = \"/p\"\n\
             # 3 above an unmanaged key\n\
             custom_note = \"hand added\" # 4 at the end of its line\n\
             # 5 after the last key\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("project-1", "/p"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        for comment in [
            "# 1 above the header",
            "# 2 between the header and the first key",
            "# 3 above an unmanaged key",
            "# 4 at the end of its line",
            // Position 5 is document-trailing trivia in `toml_edit`'s model and is
            // pinned separately by
            // `a_comment_below_the_last_project_key_survives_but_stays_document_trailing`.
            "# 5 after the last key",
        ] {
            assert!(saved.contains(comment), "{comment:?} was deleted: {saved}");
        }
        // ...and position 2 is still where it was written, not floated elsewhere.
        let lines: Vec<&str> = saved.lines().collect();
        let header = lines
            .iter()
            .position(|l| l.trim() == "[[projects]]")
            .unwrap_or_else(|| panic!("the projects header vanished: {saved}"));
        assert_eq!(
            lines.get(header + 1).map(|l| l.trim()),
            Some("# 2 between the header and the first key"),
            "the comment under the header moved: {saved}"
        );
        // The file still parses, which a mis-placed decor prefix can break.
        let _: Config = toml::from_str(&saved).expect("reparse");

        // A carried prefix is re-captured on the next save, so it has to settle
        // rather than grow a blank line per save. Measured from the SECOND save
        // on: the first save also materializes sections the file predates (here
        // `[macros]`) and re-appends the rebuilt `projects` array after them, which
        // moves the array within the document exactly once and is unrelated to the
        // carry. Saves two and three are byte-identical.
        patch_config_file(&config_path, &config).expect("second save");
        let second = fs::read_to_string(&config_path).expect("read back");
        patch_config_file(&config_path, &config).expect("third save");
        assert_eq!(
            fs::read_to_string(&config_path).expect("read back"),
            second,
            "the carried comments must settle rather than drift on every save"
        );
        for comment in [
            "# 1 above the header",
            "# 2 between the header and the first key",
            "# 3 above an unmanaged key",
            "# 4 at the end of its line",
            "# 5 after the last key",
        ] {
            assert!(
                second.contains(comment),
                "{comment:?} survived one save and was lost by the next: {second}"
            );
        }
    }

    /// The comment above the entry's FIRST key is re-homed onto the rebuilt `id`,
    /// but only when the source's first key was one this writer rebuilds. When the
    /// user put an UNMANAGED key first, that same comment also travels on the key's
    /// own decor, and carrying it twice duplicates it further down the file.
    #[test]
    fn a_comment_above_an_unmanaged_first_key_is_carried_once_not_twice() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\n\
             # about the note\n\
             custom_note = \"hand added\"\n\
             id = \"project-1\"\n\
             path = \"/p\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("project-1", "/p"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert_eq!(
            saved.matches("# about the note").count(),
            1,
            "the comment was carried twice: {saved}"
        );
    }

    /// Two entries sharing an id is a hand-edit the project sync rejects, but the
    /// writer must not SCRAMBLE it. Matching by occurrence alone kept each entry's
    /// keys with whatever landed in the same SLOT, so merely reordering the two in
    /// memory moved one project's key onto the other, a different worktree.
    /// Identity is therefore the pair of id and path.
    #[test]
    fn duplicate_project_ids_keep_their_own_keys_when_the_entries_are_reordered() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\nid = \"dup\"\npath = \"/a\"\nnote = \"from-a\"\n\n\
             [[projects]]\nid = \"dup\"\npath = \"/b\"\nnote = \"from-b\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("dup", "/b"));
        config.projects.push(bare_project("dup", "/a"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let doc: DocumentMut = saved.parse().expect("reparse");
        let entries = doc
            .get("projects")
            .and_then(Item::as_array_of_tables)
            .expect("projects array");
        let note_at = |index: usize| {
            entries
                .get(index)
                .and_then(|t| t.get("note"))
                .and_then(Item::as_str)
                .map(str::to_string)
        };
        assert_eq!(
            entries
                .get(0)
                .and_then(|t| t.get("path"))
                .and_then(Item::as_str),
            Some("/b"),
            "{saved}"
        );
        assert_eq!(
            note_at(0).as_deref(),
            Some("from-b"),
            "/b took /a's key: {saved}"
        );
        assert_eq!(
            note_at(1).as_deref(),
            Some("from-a"),
            "/a took /b's key: {saved}"
        );
    }

    /// `projects = [ ... ]` spelled across several lines can carry a comment
    /// BETWEEN its elements, which is legal TOML and pure user data. `toml_edit`
    /// files it as the prefix of the element that follows, so it is carryable
    /// exactly like an array-of-tables header comment, and it used to be dropped.
    #[test]
    fn patch_keeps_comments_written_between_inline_array_entries() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "projects = [\n  \
             # about a\n  \
             { id = \"a\", path = \"/a\", note_a = 1 },\n  \
             # about b\n  \
             { id = \"b\", path = \"/b\" },\n  \
             # about c\n  \
             { id = \"c\", path = \"/c\" },\n]\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("a", "/a"));
        config.projects.push(bare_project("b", "/b"));
        config.projects.push(bare_project("c", "/c"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        for comment in ["# about a", "# about b", "# about c"] {
            assert!(
                saved.contains(comment),
                "the inline-array spelling dropped {comment:?}: {saved}"
            );
        }
        // ...and the key carried out of an inline table is re-spaced for the block
        // table it lands in, rather than bleeding its inline spacing. Asserted as a
        // whole LINE, because `contains("note_a = 1")` also matches the bleeding
        // form `" note_a = 1 "` and so would pass without the fix.
        assert!(
            saved.lines().any(|line| line == "note_a = 1"),
            "the carried key kept its inline spacing: {saved}"
        );
    }

    /// The carried prefix is normalized to the comment lines rather than pasted
    /// verbatim. The recorded prefix of the FILE'S FIRST entry has no leading blank
    /// line (nothing precedes it), so pasting it onto that project once it has moved
    /// to second position ran the two entries together.
    #[test]
    fn a_commented_project_moved_later_keeps_a_blank_line_before_its_header() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "# about a\n[[projects]]\nid = \"a\"\npath = \"/a\"\n\n\
             [[projects]]\nid = \"b\"\npath = \"/b\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("b", "/b"));
        config.projects.push(bare_project("a", "/a"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let lines: Vec<&str> = saved.lines().collect();
        let comment = lines
            .iter()
            .position(|l| l.trim() == "# about a")
            .unwrap_or_else(|| panic!("the comment was deleted: {saved}"));
        assert!(
            lines
                .get(comment.wrapping_sub(1))
                .is_some_and(|l| l.trim().is_empty()),
            "the moved entry ran into the one above it: {saved}"
        );
    }

    #[test]
    fn patch_materializes_tab_cap_keys_on_a_file_that_predates_them() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        // Simulate a config written before the tab-cap keys existed: the full
        // canonical render with those three keys stripped back out.
        let mut doc: DocumentMut = render_config_plain(&Config::default())
            .parse()
            .expect("parse render");
        doc["ui"]
            .as_table_mut()
            .expect("[ui] table")
            .remove("agent_tabs_max");
        let server = doc["server"].as_table_mut().expect("[server] table");
        server.remove("max_websocket_tab_connections");
        server.remove("max_websocket_tabs_per_agent");
        fs::write(&config_path, doc.to_string()).expect("write older config");
        assert!(
            !fs::read_to_string(&config_path)
                .unwrap()
                .contains("agent_tabs_max")
        );

        let mut config = Config::default();
        config.ui.agent_tabs_max = 7;
        config.server.max_websocket_tab_connections = 123;
        config.server.max_websocket_tabs_per_agent = 5;

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        // The patch path must materialize the new keys (parity with the sibling
        // ws caps), not rely on defaults being filled in at load time.
        assert!(saved.contains("agent_tabs_max"));
        assert!(saved.contains("max_websocket_tab_connections"));
        assert!(saved.contains("max_websocket_tabs_per_agent"));
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.ui.agent_tabs_max, 7);
        assert_eq!(parsed.server.max_websocket_tab_connections, 123);
        assert_eq!(parsed.server.max_websocket_tabs_per_agent, 5);
    }

    #[test]
    fn write_config_plain_overwrites() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        // Seed a corrupt/unparseable file that `save_config`'s patch path would
        // choke on. `write_config_plain` must overwrite it regardless.
        fs::write(&config_path, "this is not = valid toml [[[ \n broken").expect("write garbage");

        let mut config = Config::default();
        config.env.insert("FOO".to_string(), "bar".to_string());

        write_config_plain(&config_path, &config).expect("write_config_plain");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse valid config");
        assert_eq!(parsed.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(parsed.defaults.provider, config.defaults.provider);
    }

    #[test]
    fn write_config_plain_round_trips_server_section() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.server.host = "0.0.0.0".to_string();
        config.server.port = 9000;
        config.server.tailscale = "no".to_string();
        config.server.allowed_hosts = vec!["box.tailnet.ts.net".to_string()];
        config.server.color = "never".to_string();
        config.server.access_log = false;
        config.server.qr_codes = false;
        config.server.max_websocket_events_connections = 42;
        config.server.max_websocket_agent_connections = 43;
        config.server.max_websocket_terminal_connections = 44;
        config.server.title = "dux #1".to_string();
        config.server.favicon = "violet".to_string();

        write_config_plain(&config_path, &config).expect("write_config_plain");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.server.host, "0.0.0.0");
        assert_eq!(parsed.server.port, 9000);
        assert_eq!(parsed.server.tailscale, "no");
        assert_eq!(
            parsed.server.allowed_hosts,
            vec!["box.tailnet.ts.net".to_string()]
        );
        assert_eq!(parsed.server.color, "never");
        assert!(!parsed.server.access_log);
        assert!(!parsed.server.qr_codes);
        assert_eq!(parsed.server.max_websocket_events_connections, 42);
        assert_eq!(parsed.server.max_websocket_agent_connections, 43);
        assert_eq!(parsed.server.max_websocket_terminal_connections, 44);
        assert_eq!(parsed.server.title, "dux #1");
        assert_eq!(parsed.server.favicon, "violet");
        // The deprecated `bind` key is never re-emitted by the patcher.
        assert!(
            !saved.contains("bind ="),
            "patcher must not emit bind: {saved}"
        );
    }

    #[test]
    fn a_save_migrates_the_legacy_tailscale_enabled_key_out_of_the_file() {
        // A user upgrading from the boolean has `tailscale_enabled` on disk. The
        // save writes the tri-state key and takes the boolean OUT, so the file
        // stops carrying a key dux no longer reads. This is the on-disk half of
        // the compat story; the in-memory half is `config_migrate`.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[server]\nport = 9000\ntailscale_enabled = false\n",
        )
        .expect("seed an old config");

        let mut config = Config::default();
        config.server.port = 9000;
        config.server.tailscale = "no".to_string();
        patch_config_file(&config_path, &config).expect("patch the existing config");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            !saved.contains("tailscale_enabled"),
            "the legacy key must be gone after a save: {saved}"
        );
        assert!(
            saved.contains("tailscale = \"no\""),
            "the tri-state key must be written: {saved}"
        );
    }

    #[test]
    fn write_config_plain_round_trips_title_with_toml_specials() {
        // The instance title is free-form user text. Lock in the escaping contract
        // so a future toml_edit bump or patcher refactor can't silently emit a
        // value with an unescaped quote/backslash that fails to re-parse.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.server.title = r#"dux "prod" \ lab"#.to_string();

        write_config_plain(&config_path, &config).expect("write_config_plain");
        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.server.title, r#"dux "prod" \ lab"#);
    }

    #[test]
    fn patch_config_file_round_trips_title_with_toml_specials() {
        // The in-place patcher is the production save hot-path. Exercise its
        // read-parse-apply-write cycle from an existing [server] block with a
        // title containing a quote and a backslash, locking in the same escaping
        // contract the plain writer is held to.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[server]\ntitle = \"old\"\n").expect("write initial");

        let mut config = Config::default();
        config.server.title = r#"dux "prod" \ lab"#.to_string();

        patch_config_file(&config_path, &config).expect("patch");
        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.server.title, r#"dux "prod" \ lab"#);
    }

    #[test]
    fn write_config_plain_round_trips_host_and_allowed_hosts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.toml");
        let mut cfg = Config::default();
        cfg.server.host = "0.0.0.0".into();
        cfg.server.port = 9000;
        cfg.server.allowed_hosts = vec!["box.tailnet.ts.net".into()];
        write_config_plain(&path, &cfg).unwrap();
        let parsed: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.server.host, "0.0.0.0");
        assert_eq!(parsed.server.port, 9000);
        assert_eq!(
            parsed.server.allowed_hosts,
            vec!["box.tailnet.ts.net".to_string()]
        );
    }

    #[test]
    #[cfg(unix)]
    fn write_config_plain_sets_owner_only_perms() {
        // config.toml may carry secrets (tokens under [env]),
        // so every write must restrict it to 0600 (owner read/write only).
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");

        let config = Config::default();
        write_config_plain(&config_path, &config).expect("write_config_plain");

        let mode = fs::metadata(&config_path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "config.toml must be owner-read/write only, got {:o}",
            mode & 0o777
        );
    }

    #[test]
    #[cfg(unix)]
    fn write_config_secure_creates_fresh_file_owner_only() {
        // The create path must apply 0600 AT creation (OpenOptions::mode), so a
        // brand-new config holding secrets is never briefly world-readable. We
        // call the low-level helper directly to assert the create branch, not
        // just the post-write chmod.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        assert!(
            !config_path.exists(),
            "file must not exist before the write"
        );

        write_config_secure(&config_path, "[defaults]\nprovider = \"claude\"\n")
            .expect("write_config_secure");

        let mode = fs::metadata(&config_path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a freshly created config must be owner-read/write only, got {:o}",
            mode & 0o777
        );
    }

    #[test]
    #[cfg(unix)]
    fn patch_config_file_sets_owner_only_perms() {
        // The patch path (existing file) must also re-restrict perms to 0600 so a
        // previously-loose file is tightened on the next save.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        // Seed a world-readable file first.
        fs::write(&config_path, "[defaults]\nprovider = \"claude\"\n").expect("seed");
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o644)).expect("loosen");

        let config = Config::default();
        patch_config_file(&config_path, &config).expect("patch");

        let mode = fs::metadata(&config_path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "patching config.toml must tighten perms to 0600, got {:o}",
            mode & 0o777
        );
    }

    #[test]
    fn save_config_creates_file_when_missing() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        assert!(!config_path.exists());

        let mut config = Config::default();
        config.env.insert("FOO".to_string(), "bar".to_string());
        config.projects.push(ProjectConfig {
            id: "project-1".to_string(),
            path: "/home/user/project".to_string(),
            name: Some("test".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
        });

        save_config(&config_path, &config).expect("save");

        assert!(config_path.exists(), "save_config did not create the file");
        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(parsed.projects.len(), 1);
        assert_eq!(parsed.projects[0].id, "project-1");
    }

    // -----------------------------------------------------------------------
    // merge_unmanaged_keys
    // -----------------------------------------------------------------------

    #[test]
    fn merge_preserves_an_unknown_key_and_drops_an_orphaned_section() {
        let original: DocumentMut = "\
[server]
port = 8080
listen_addrs = []

[server.acme]
enabled = false
production = true

[auth]
users = []

[my_fork_section]
knob = 42
"
        .parse()
        .expect("parse original");
        let mut rendered: DocumentMut = "[server]\nport = 8080\n".parse().expect("parse rendered");

        let report = merge_unmanaged_keys(&mut rendered, &original);

        let out = rendered.to_string();
        // Unknown keys survive verbatim, whether beside a managed key or in a
        // section dux has never heard of.
        assert!(out.contains("listen_addrs = []"), "out:\n{out}");
        assert!(out.contains("knob = 42"), "out:\n{out}");
        // Orphaned sections are gone.
        assert!(!out.contains("acme"), "out:\n{out}");
        assert!(!out.contains("users"), "out:\n{out}");
        // And their removal is reported, not silent.
        assert_eq!(report.dropped, vec!["auth", "server.acme"]);
        // A key the schema does not know is placed by its line, never named:
        // its name may be a value pasted in the wrong place.
        assert_eq!(
            report.preserved,
            vec!["the entry on line 12", "the entry on line 3 of [server]"]
        );
        // The merged document is still valid TOML.
        let _: toml_edit::DocumentMut = out.parse().expect("merged output re-parses");
    }

    #[test]
    fn merge_reports_nothing_when_the_render_already_covers_everything() {
        let original: DocumentMut = "[server]\nport = 8080\n".parse().expect("parse");
        let mut rendered: DocumentMut = "# doc\n[server]\nport = 8080\n".parse().expect("parse");

        let report = merge_unmanaged_keys(&mut rendered, &original);

        assert!(report.is_empty(), "unexpected report: {report:?}");
        assert!(rendered.to_string().contains("# doc"));
    }

    #[test]
    fn merge_preserves_an_unknown_key_inside_an_array_of_tables() {
        // A hand-added key inside a `[[projects]]` block must survive, because
        // the canonical renderer only emits the project fields dux knows.
        let original: DocumentMut = "\
[[projects]]
id = \"a\"
custom_note = \"do not lose me\"

[[projects]]
id = \"b\"
"
        .parse()
        .expect("parse original");
        let mut rendered: DocumentMut = "[[projects]]\nid = \"a\"\n\n[[projects]]\nid = \"b\"\n"
            .parse()
            .expect("parse rendered");

        let report = merge_unmanaged_keys(&mut rendered, &original);

        let out = rendered.to_string();
        assert!(
            out.contains("custom_note = \"do not lose me\""),
            "out:\n{out}"
        );
        // A key the project schema does not know is placed by its line: it
        // may be anything pasted where a name goes.
        assert_eq!(
            report.preserved,
            vec!["the entry on line 3 of [projects[0]]"]
        );
        assert!(report.dropped.is_empty());
    }

    /// A key `insert_at_path` cannot place must be REPORTED, not silently
    /// vanished.
    ///
    /// **This is currently unreachable through any real config.** The canonical
    /// renderer emits one `[[projects]]` block per parsed project, so the
    /// rendered document always has an entry at every index the original has,
    /// and `insert_at_path` never returns false. The input below is doctored
    /// (a rendered document with FEWER array entries than the original) to
    /// exercise the branch directly. The guard exists so that if the renderer
    /// ever stops emitting one block per project, the failure is loud.
    #[test]
    fn merge_reports_a_key_it_could_not_place_instead_of_dropping_it_silently() {
        let original: DocumentMut = "\
[[projects]]
id = \"a\"
note = \"kept\"

[[projects]]
id = \"b\"
second_note = \"nowhere to go\"
"
        .parse()
        .expect("parse original");
        // Doctored: only ONE rendered project, so `projects[1]` has no home.
        let mut rendered: DocumentMut = "[[projects]]\nid = \"a\"\n".parse().expect("parse");

        let report = merge_unmanaged_keys(&mut rendered, &original);

        let out = rendered.to_string();
        assert!(out.contains("note = \"kept\""), "out:\n{out}");
        assert!(!out.contains("second_note"), "out:\n{out}");
        assert_eq!(
            report.preserved,
            vec!["the entry on line 3 of [projects[0]]"]
        );
        assert_eq!(
            report.unplaceable,
            vec!["projects[1].id", "the entry on line 7 of [projects[1]]"],
            "a key that could not be placed must be named, not vanish"
        );
        assert!(
            !report.is_empty(),
            "a report naming a lost key is not empty"
        );
    }

    #[test]
    fn merge_does_not_confuse_a_nested_auth_key_with_the_orphaned_auth_section() {
        // The drop-list matches TABLE paths. A key called `auth` nested inside a
        // live section is a user key and must be preserved, not dropped.
        let original: DocumentMut = "[server]\nauth = \"token\"\n".parse().expect("parse");
        let mut rendered: DocumentMut = "[server]\nport = 8080\n".parse().expect("parse");

        let report = merge_unmanaged_keys(&mut rendered, &original);

        assert!(rendered.to_string().contains("auth = \"token\""));
        assert_eq!(report.preserved, vec!["server.auth"]);
        assert!(report.dropped.is_empty());
    }

    #[test]
    fn merge_preserves_a_comment_attached_to_an_unknown_key() {
        let original: DocumentMut = "[server]\n# why this knob exists\nfork_knob = 3\n"
            .parse()
            .expect("parse");
        let mut rendered: DocumentMut = "[server]\nport = 8080\n".parse().expect("parse");

        merge_unmanaged_keys(&mut rendered, &original);

        let out = rendered.to_string();
        assert!(out.contains("# why this knob exists"), "out:\n{out}");
        assert!(out.contains("fork_knob = 3"), "out:\n{out}");
    }

    #[test]
    fn apply_patches_strips_removed_max_websocket_connections_key() {
        // Build a DocumentMut that still carries the obsolete key.
        let raw = "[server]\nmax_websocket_connections = 16\nport = 7878\n";
        assert!(
            crate::config::raw_has_removed_max_websocket_connections(raw),
            "precondition: raw must contain the removed key"
        );
        let mut doc: DocumentMut = raw.parse().expect("parse toml");
        let config = Config::default();
        apply_patches(&mut doc, &config);
        // The key must be stripped after apply_patches.
        let stripped = doc.to_string();
        assert!(
            !crate::config::raw_has_removed_max_websocket_connections(&stripped),
            "apply_patches must remove max_websocket_connections; got: {stripped}"
        );
        // Other server settings survive.
        assert!(
            stripped.contains("port"),
            "apply_patches must not wipe unrelated server keys; got: {stripped}"
        );
    }

    #[test]
    fn apply_patches_does_not_warn_when_key_is_absent() {
        // A config that never had max_websocket_connections must not trip the
        // detection predicate after patching.
        let raw = "[server]\nport = 7878\n";
        assert!(
            !crate::config::raw_has_removed_max_websocket_connections(raw),
            "precondition: raw must not contain the removed key"
        );
        let mut doc: DocumentMut = raw.parse().expect("parse toml");
        let config = Config::default();
        // Must not panic and must leave the key absent.
        apply_patches(&mut doc, &config);
        let stripped = doc.to_string();
        assert!(
            !crate::config::raw_has_removed_max_websocket_connections(&stripped),
            "key must remain absent; got: {stripped}"
        );
    }

    // -----------------------------------------------------------------------
    // [server.auth]: never overwritten from memory, never orphaned
    // -----------------------------------------------------------------------

    fn a_hash(text: &str) -> String {
        crate::auth::hash_password(&crate::auth::Password::new(text.to_string())).expect("hash")
    }

    /// Orphaned sections are matched segment by segment: a top-level key
    /// whose name is `"server.acme"` is a user's own key, kept, and never
    /// taken for the retired `[server.acme]` section.
    #[test]
    fn a_key_named_like_an_orphaned_section_is_not_that_section() {
        let original: DocumentMut = "\"server.acme\" = 1\n\n[server.acme]\nx = 1\n"
            .parse()
            .unwrap();
        let mut rendered = DocumentMut::new();
        let report = merge_unmanaged_keys(&mut rendered, &original);
        assert_eq!(
            report.dropped,
            vec!["server.acme".to_string()],
            "{report:?}"
        );
        assert_eq!(report.preserved.len(), 1, "{report:?}");
        assert!(
            rendered.to_string().contains("\"server.acme\" = 1"),
            "{rendered}"
        );
        assert!(
            !rendered.to_string().contains("[server.acme]"),
            "{rendered}"
        );
    }

    #[test]
    fn the_orphan_cleanup_never_touches_server_auth() {
        let hash = a_hash("correct horse battery staple");
        let original: DocumentMut = format!(
            "[auth]\nusers = []\n\n[server]\nport = 8080\n\n[server.auth]\n\
             password_hash = \"{hash}\"\nblocked_addresses = [\"203.0.113.7\"]\n"
        )
        .parse()
        .expect("parse original");
        // A render that does not carry the section at all is the worst case:
        // everything in it must come across from the original.
        let mut rendered: DocumentMut = "[server]\nport = 8080\n".parse().expect("parse");

        let report = merge_unmanaged_keys(&mut rendered, &original);

        let out = rendered.to_string();
        assert_eq!(
            report.dropped,
            vec!["auth"],
            "only the old top-level [auth] goes"
        );
        assert!(out.contains(&hash), "out:\n{out}");
        assert!(out.contains("203.0.113.7"), "out:\n{out}");
    }

    #[test]
    fn a_save_from_memory_never_overwrites_an_auth_value_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let on_disk = a_hash("the password somebody just set");
        std::fs::write(
            &path,
            format!(
                "[server]\nport = 4000\n\n[server.auth]\npassword_hash = \"{on_disk}\"\n\
                 blocked_addresses = [\"203.0.113.7\"]\n"
            ),
        )
        .expect("seed");

        // The running dux still remembers the OLD password and no bans.
        let mut stale = Config::default();
        stale.server.auth.password_hash = a_hash("the password from before");
        stale.server.port = 4001;
        patch_config_file_with(&path, &stale, Durability::NoFsync).expect("patch");

        let after = std::fs::read_to_string(&path).expect("read");
        assert!(
            after.contains("port = 4001"),
            "an ordinary setting still saves:\n{after}"
        );
        assert!(
            after.contains(&on_disk),
            "the newer password survives:\n{after}"
        );
        assert!(after.contains("203.0.113.7"), "the ban survives:\n{after}");
    }

    /// Saves from memory never write `[server.auth]`: not a changed key, not
    /// a missing one. A key the user deleted by hand stays deleted.
    #[test]
    fn a_save_from_memory_never_writes_server_auth_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        // The user removed `require` (and everything else) by hand.
        let before = "[server]\nport = 4000\n\n[server.auth]\nmax_failed_logins = 3\n";
        std::fs::write(&path, before).expect("seed");
        let mut config = Config::default();
        config.server.auth.require = crate::config::AuthRequire::Everywhere;
        config.server.auth.password_hash = a_hash("a stale password in memory");
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");

        let after = std::fs::read_to_string(&path).expect("read");
        let auth = after
            .split("[server.auth]")
            .nth(1)
            .expect("section kept")
            .split("\n[")
            .next()
            .unwrap_or_default();
        assert_eq!(auth.trim(), "max_failed_logins = 3", "untouched:\n{after}");
        let mut no_auth = DocumentMut::new();
        apply_patches(&mut no_auth, &config);
        assert!(
            no_auth.get("server").and_then(|s| s.get("auth")).is_none(),
            "apply_patches has no auth keys at all"
        );
    }

    /// Every write path refuses a result whose `[server.auth]` would stop dux
    /// from starting.
    #[test]
    fn writes_refuse_a_result_with_an_invalid_server_auth() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let broken = "[server.auth]\npassword_hash = \"hunter2\"\n";
        assert!(write_config_atomic(&path, broken, Durability::NoFsync).is_err());
        assert!(!path.exists(), "nothing was written");
        // A file the user left broken is the user's to fix: a save from
        // memory, which writes no auth key, goes ahead beside it and leaves
        // the user's line as it was.
        std::fs::write(&path, broken).expect("seed");
        let mut ours = Config::default();
        ours.ui.copy_on_select = false;
        patch_config_file_with(&path, &ours, Durability::NoFsync).expect("the save lands");
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("password_hash = \"hunter2\""), "{after}");
        assert!(after.contains("copy_on_select = false"), "{after}");
    }

    /// A save beside a `[server.auth]` mistake the user made logs it once,
    /// naming the line in the user's own file even though dux's change moves
    /// it down in the text written, and logs it no more on the next save.
    #[test]
    fn a_save_beside_a_users_auth_mistake_logs_it_once_by_the_users_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        // A distinct value, so this test's line is its own in the log.
        let user =
            "[ui]\ncopy_on_select = true\n\n[server.auth]\nminimum_password_length = \"27\"\n";
        std::fs::write(&path, user).expect("seed");
        let mut ours = Config::default();
        ours.ui.copy_on_select = false;
        ours.ui.left_width_pct = 31;
        let ((), lines) = crate::logger::capture_for_test(|| {
            patch_config_file_with(&path, &ours, Durability::NoFsync).expect("the save lands");
        });
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("left_width_pct = 31"), "{after}");
        let logged: Vec<&String> = lines
            .iter()
            .filter(|line| line.contains("a [server.auth] problem dux did not make"))
            .collect();
        assert_eq!(logged.len(), 1, "{lines:?}\n{after}");
        // Line 5 of the user's file; dux's copy has it on line 6.
        assert!(logged[0].contains("line 5"), "{logged:?}\n{after}");
        ours.ui.left_width_pct = 32;
        let ((), lines) = crate::logger::capture_for_test(|| {
            patch_config_file_with(&path, &ours, Durability::NoFsync).expect("the save lands");
        });
        assert!(
            lines
                .iter()
                .all(|line| !line.contains("a [server.auth] problem dux did not make")),
            "{lines:?}"
        );
    }

    /// The three-way save: a key changed on disk by someone else, and not in
    /// memory, keeps the disk value; a key changed in memory is written.
    fn base_and_file(path: &Path, body: &str) -> Config {
        std::fs::write(path, body).expect("seed");
        crate::config::config_from_text_as_written(body).expect("parse")
    }

    /// A setting a save changes keeps the documentation comment above it and
    /// the one trailing it.
    #[test]
    fn a_changed_key_keeps_its_comments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let base = base_and_file(
            &path,
            "[ui]\n# Copy on select.\ncopy_on_select = true # trailing\nleft_width_pct = 20\n",
        );
        let mut ours = base.clone();
        ours.ui.copy_on_select = false;
        for base in [Some(SaveBase::read(&base)), None] {
            std::fs::write(
                &path,
                "[ui]\n# Copy on select.\ncopy_on_select = true # trailing\nleft_width_pct = 20\n",
            )
            .unwrap();
            save_config_three_way(&path, base, &ours, Durability::NoFsync).expect("save");
            let after = std::fs::read_to_string(&path).unwrap();
            assert!(
                after.contains("# Copy on select.\ncopy_on_select = false # trailing\n"),
                "{after}"
            );
        }
    }

    /// A pure reorder of an ordered table is a change and is written.
    #[test]
    fn a_reorder_in_memory_is_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let base = base_and_file(
            &path,
            "[macros]\na = { text = \"a\", surface = \"agent\" }\nb = { text = \"b\", surface = \"agent\" }\nc = { text = \"c\", surface = \"agent\" }\n",
        );
        let mut ours = base.clone();
        ours.macros.entries.move_index(2, 0);
        save_config_three_way(
            &path,
            Some(SaveBase::read(&base)),
            &ours,
            Durability::NoFsync,
        )
        .expect("save");
        let parsed: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let order: Vec<&str> = parsed.macros.entries.keys().map(String::as_str).collect();
        assert_eq!(order, ["c", "a", "b"]);
    }

    /// A project added to the file by hand survives a save that adds another
    /// project from memory; projects are matched by id, not by position.
    fn project_ids(path: &Path) -> Vec<(String, String)> {
        let parsed: Config = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        parsed
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.path.clone()))
            .collect()
    }

    /// A hand-written project with no `id`: the base and memory each mint
    /// their own, so the entry is matched by path, written once, and given
    /// memory's id, which then stays.
    #[test]
    fn an_id_less_project_is_not_duplicated_and_gets_its_id_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let body = "[[projects]]\npath = \"/tmp/hand\"\nname = \"hand\"\n";
        let base = base_and_file(&path, body);
        let mut ours: Config = toml::from_str(body).unwrap();
        assert_ne!(
            base.projects[0].id, ours.projects[0].id,
            "two parses, two ids"
        );
        ours.ui.copy_on_select = false;
        save_config_three_way(
            &path,
            Some(SaveBase::read(&base)),
            &ours,
            Durability::NoFsync,
        )
        .expect("save");
        assert_eq!(
            project_ids(&path),
            vec![(ours.projects[0].id.clone(), "/tmp/hand".to_string())]
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("name = \"hand\"")
        );
    }

    /// Memory adopted the id SQLite has for the same path: one entry, with
    /// the adopted id.
    #[test]
    fn an_adopted_project_id_replaces_the_files_without_duplicating() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let base = base_and_file(
            &path,
            "[[projects]]\nid = \"from-file\"\npath = \"/tmp/p\"\n",
        );
        let mut ours = base.clone();
        ours.projects[0].id = "from-sqlite".to_string();
        save_config_three_way(
            &path,
            Some(SaveBase::read(&base)),
            &ours,
            Durability::NoFsync,
        )
        .expect("save");
        assert_eq!(
            project_ids(&path),
            vec![("from-sqlite".to_string(), "/tmp/p".to_string())]
        );
    }

    /// Renamed in memory while another field of the same project changed on
    /// disk: both changes land, field by field.
    #[test]
    fn a_project_entry_merges_field_by_field() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let base = base_and_file(
            &path,
            "[[projects]]\nid = \"p\"\npath = \"/tmp/p\"\nname = \"old\"\n",
        );
        std::fs::write(
            &path,
            "[[projects]]\nid = \"p\"\npath = \"/tmp/p\"\nname = \"old\"\nstartup_command = \"make\"\n",
        )
        .unwrap();
        let mut ours = base.clone();
        ours.projects[0].name = Some("new".to_string());
        save_config_three_way(
            &path,
            Some(SaveBase::read(&base)),
            &ours,
            Durability::NoFsync,
        )
        .expect("save");
        let parsed: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.projects.len(), 1);
        assert_eq!(parsed.projects[0].name.as_deref(), Some("new"));
        assert_eq!(parsed.projects[0].startup_command.as_deref(), Some("make"));
    }

    /// Two entries with one identity never make a save push one twice or
    /// lose one.
    /// A copy-pasted entry shares the original's id but not its path; a
    /// rename in memory lands on the entry with the matching id AND path.
    #[test]
    fn a_rename_lands_on_the_entry_matching_id_and_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let body = "[[projects]]\nid = \"same\"\npath = \"/tmp/copy\"\nname = \"copy\"\nstartup_command = \"a\"\n\n[[projects]]\nid = \"same\"\npath = \"/tmp/original\"\nname = \"original\"\n";
        let base = base_and_file(&path, body);
        // The copy's command is changed on disk meanwhile; memory never saw it.
        std::fs::write(
            &path,
            body.replace("startup_command = \"a\"", "startup_command = \"disk-edit\""),
        )
        .unwrap();
        let mut ours = base.clone();
        let original = ours
            .projects
            .iter_mut()
            .find(|p| p.path == "/tmp/original")
            .unwrap();
        original.name = Some("renamed".to_string());
        // Memory lists the original first, so a match by id alone would pair it
        // with the copy above it in the file.
        ours.projects.swap(0, 1);
        save_config_three_way(
            &path,
            Some(SaveBase::read(&base)),
            &ours,
            Durability::NoFsync,
        )
        .expect("save");
        let parsed: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let names: Vec<(String, Option<String>)> = parsed
            .projects
            .iter()
            .map(|p| (p.path.clone(), p.name.clone()))
            .collect();
        assert!(
            names.contains(&("/tmp/copy".to_string(), Some("copy".to_string()))),
            "{names:?}"
        );
        assert!(
            names.contains(&("/tmp/original".to_string(), Some("renamed".to_string()))),
            "{names:?}"
        );
        let copy = parsed
            .projects
            .iter()
            .find(|p| p.path == "/tmp/copy")
            .unwrap();
        assert_eq!(
            copy.startup_command.as_deref(),
            Some("disk-edit"),
            "the copy's own disk edit"
        );
    }

    #[test]
    fn duplicate_identities_are_neither_doubled_nor_dropped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let body = "[[projects]]\nid = \"same\"\npath = \"/tmp/a\"\n\n[[projects]]\nid = \"same\"\npath = \"/tmp/b\"\n";
        let base = base_and_file(&path, body);
        let mut ours = base.clone();
        ours.ui.copy_on_select = false;
        save_config_three_way(
            &path,
            Some(SaveBase::read(&base)),
            &ours,
            Durability::NoFsync,
        )
        .expect("save");
        assert_eq!(project_ids(&path).len(), 2);
    }

    /// A setting deleted from the file by hand stays deleted; one the file
    /// never had (new in this version) is filled in.
    #[test]
    fn a_hand_deleted_key_stays_deleted_and_a_new_one_is_filled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let at_read = "[ui]\nleft_width_pct = 20\n\n[env]\nFOO = \"bar\"\n";
        let base = base_and_file(&path, at_read);
        // The user deletes `left_width_pct` and the FOO variable.
        std::fs::write(&path, "[ui]\n\n[env]\n").unwrap();
        let mut ours = base.clone();
        ours.ui.copy_on_select = false;
        save_config_three_way(
            &path,
            Some(SaveBase::read(&base)),
            &ours,
            Durability::NoFsync,
        )
        .expect("save");
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(!after.contains("left_width_pct"), "{after}");
        assert!(!after.contains("FOO"), "{after}");
        assert!(after.contains("copy_on_select = false"), "{after}");
        assert!(
            after.contains("right_width_pct"),
            "a setting the file never had: {after}"
        );
    }

    #[test]
    fn projects_merge_by_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let base = base_and_file(&path, "[[projects]]\nid = \"one\"\npath = \"/tmp/one\"\n");
        std::fs::write(
            &path,
            "[[projects]]\nid = \"one\"\npath = \"/tmp/one\"\n\n[[projects]]\nid = \"hand\"\npath = \"/tmp/hand\"\n",
        )
        .unwrap();
        let mut ours = base.clone();
        let mut added = ours.projects[0].clone();
        added.id = "two".to_string();
        added.path = "/tmp/two".to_string();
        ours.projects.push(added);
        save_config_three_way(
            &path,
            Some(SaveBase::read(&base)),
            &ours,
            Durability::NoFsync,
        )
        .expect("save");
        let parsed: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let ids: Vec<&str> = parsed.projects.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["one", "two", "hand"]);
    }

    #[test]
    fn a_three_way_save_keeps_a_disk_change_memory_did_not_make() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[ui]\n# how wide\nleft_width_pct = 20\ncopy_on_select = true\n",
        )
        .expect("seed");
        let base =
            crate::config::config_from_text_as_written(&std::fs::read_to_string(&path).unwrap())
                .unwrap();
        // `dux config set ui.left_width_pct 33` happens on disk.
        std::fs::write(
            &path,
            "[ui]\n# how wide\nleft_width_pct = 33\ncopy_on_select = true\n",
        )
        .expect("external set");
        let mut ours = base.clone();
        ours.ui.copy_on_select = false;
        save_config_three_way(
            &path,
            Some(SaveBase::read(&base)),
            &ours,
            Durability::NoFsync,
        )
        .expect("save");
        let after: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after.ui.left_width_pct, 33, "the disk change survives");
        assert!(!after.ui.copy_on_select, "the memory change lands");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("# how wide")
        );
    }

    #[test]
    fn a_plain_render_carries_the_running_password() {
        let mut config = Config::default();
        config.server.auth.password_hash = a_hash("the running password");
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("valid");
        assert_eq!(parsed.server.auth, config.server.auth, "{rendered}");
    }

    // -----------------------------------------------------------------------
    // The coordinated mutation path
    // -----------------------------------------------------------------------

    #[test]
    fn concurrent_mutations_of_different_keys_never_lose_an_update() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[server]\nport = 4000\n").expect("seed");
        let rounds = 40;
        let threads: Vec<_> = ["a", "b", "c"]
            .into_iter()
            .map(|name| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for i in 0..rounds {
                        mutate_config_file(&path, |doc| {
                            let server = doc["server"].as_table_mut().expect("server");
                            let list = server
                                .entry("allowed_hosts")
                                .or_insert_with(|| toml_edit::value(Array::new()))
                                .as_array_mut()
                                .expect("array");
                            list.push(format!("{name}{i}.example"));
                            Ok(())
                        })
                        .expect("mutate");
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("join");
        }
        let parsed: Config =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).expect("valid");
        assert_eq!(
            parsed.server.allowed_hosts.len(),
            3 * rounds,
            "every append from every writer landed"
        );
    }

    #[test]
    fn a_save_from_memory_waits_for_a_held_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[server]\nport = 4000\n").expect("seed");
        let held = ConfigFileLock::acquire(&path).expect("lock");
        let saver = {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut config = Config::default();
                config.server.port = 4002;
                patch_config_file_with(&path, &config, Durability::NoFsync)
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("port = 4000"),
            "nothing is written while another writer holds the lock"
        );
        drop(held);
        saver.join().expect("join").expect("save");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("port = 4002")
        );
    }

    /// A whole-file replacement reads the current file only once it holds the
    /// lock, so a writer that finishes while it waits is seen, not lost.
    #[test]
    fn a_replacement_reads_the_file_inside_the_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").expect("seed");
        let held = ConfigFileLock::acquire(&path).expect("lock");
        let replacer = {
            let path = path.clone();
            std::thread::spawn(move || {
                replace_config_file(&path, |current| {
                    Ok((current.unwrap_or_default().replace("# x", "# seen"), ()))
                })
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(150));
        // The other writer lands while the replacement waits on the lock.
        write_config_atomic_unlocked(
            &path,
            "[ui]\n# x\nleft_width_pct = 33\n",
            Durability::NoFsync,
        )
        .expect("other writer");
        drop(held);
        replacer.join().expect("join").expect("replace");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\n# seen\nleft_width_pct = 33\n"
        );
    }

    /// The lock needs only read access to its file, so a lock file left
    /// read-only (or owned by root after a `sudo dux`) still works, and one
    /// that cannot be opened at all says how to fix it.
    #[test]
    fn the_lock_file_needs_only_read_access_and_says_when_it_has_none() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let lock = ConfigFileLock::lock_path(&path);
        std::fs::write(&lock, "").expect("lock file");
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o400)).unwrap();
        drop(ConfigFileLock::acquire(&path).expect("a read-only lock file locks"));
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o000)).unwrap();
        let err = ConfigFileLock::acquire(&path).expect_err("unreadable");
        let text = format!("{err:#}");
        assert!(text.contains(&lock.display().to_string()), "{text}");
        assert!(text.contains("owner"), "{text}");
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn a_lock_that_never_frees_is_an_error_not_a_hang() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").expect("seed");
        let _held = ConfigFileLock::acquire(&path).expect("lock");
        let path2 = path.clone();
        let err = std::thread::spawn(move || {
            ConfigFileLock::acquire_within(&path2, std::time::Duration::from_millis(150))
        })
        .join()
        .expect("join")
        .expect_err("times out");
        assert!(err.to_string().contains("another"), "{err:#}");
    }

    #[test]
    fn a_mutation_that_would_break_server_auth_is_refused_and_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let before = "[server]\nport = 4000\n";
        std::fs::write(&path, before).expect("seed");
        let err = mutate_config_file(&path, |doc| {
            doc["server"]["auth"]["password_hash"] = toml_edit::value("hunter2");
            Ok(())
        })
        .expect_err("refused");
        assert!(err.to_string().contains("server.auth"), "{err:#}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn a_failed_write_leaves_the_file_as_it_was() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let before = "[server]\nport = 4000\n";
        std::fs::write(&path, before).expect("seed");
        // Create the lock file first so only the temp file's creation fails.
        drop(ConfigFileLock::acquire(&path).expect("lock"));
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500))
            .expect("chmod");
        let result = mutate_config_file(&path, |doc| {
            doc["server"]["port"] = toml_edit::value(4001);
            Ok(())
        });
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .expect("chmod back");
        assert!(
            result.is_err(),
            "a directory dux cannot write to fails the write"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn a_mutation_of_a_file_that_is_not_toml_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[server\n").expect("seed");
        assert!(mutate_config_file(&path, |_| Ok(())).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[server\n");
    }

    #[test]
    fn a_mutation_with_no_file_starts_from_the_documented_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        mutate_config_file(&path, |doc| {
            doc["server"]["port"] = toml_edit::value(4005);
            Ok(())
        })
        .expect("mutate");
        let parsed: Config =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).expect("valid");
        assert_eq!(parsed.server.port, 4005);
        assert_eq!(
            parsed.ui.left_width_pct,
            Config::default().ui.left_width_pct
        );
    }

    /// A refusal names the shape that is actually wrong: a `server.auth`
    /// that is not a table is not called an invalid `[server.auth]` section.
    /// A `server` that is not a table holds no password and is written
    /// around, as dux always read it.
    #[test]
    fn a_refused_write_names_the_shape_that_is_actually_wrong() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "server = 1\n").unwrap();
        mutate_config_file(&path, |doc| {
            doc["ui"]["left_width_pct"] = toml_edit::value(30);
            Ok(())
        })
        .expect("a [server] that is not a table holds no password");
        // The write itself makes `server.auth` something other than a table.
        let clean = "[ui]\nleft_width_pct = 20\n";
        fs::write(&path, clean).unwrap();
        let text = "[server]\nauth = [1]\n";
        let err = mutate_config_file(&path, |doc| {
            doc["server"] = toml_edit::Item::Table(toml_edit::Table::new());
            doc["server"]["auth"] = toml_edit::value(toml_edit::Array::from_iter([1i64]));
            Ok(())
        })
        .expect_err("refused");
        let message = format!("{err:#}");
        assert!(message.contains("server.auth in"), "{message}");
        assert!(message.contains("is not a table"), "{message}");
        assert!(!message.contains("make [server.auth]"), "{message}");
        let err = replace_config_file(&path, |_| Ok((text.to_string(), ()))).expect_err("refused");
        let message = format!("{err:#}");
        assert!(message.contains("server.auth in"), "{message}");
        assert!(!message.contains("leave [server.auth]"), "{message}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            clean,
            "nothing was written"
        );
    }

    /// A config.toml kept as a symlink stays one: every write goes to the
    /// file the link points at, never replacing the link with a file.
    #[test]
    fn writes_go_through_a_config_symlink_to_its_target() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let dotfiles = dir.path().join("dotfiles");
        fs::create_dir_all(&dotfiles).unwrap();
        let target = dotfiles.join("dux.toml");
        let link = dir.path().join("config.toml");
        fs::write(&target, "[ui]\nleft_width_pct = 20\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let is_link = || {
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        };

        mutate_config_file(&link, |doc| {
            doc["ui"]["left_width_pct"] = toml_edit::value(30);
            Ok(())
        })
        .unwrap();
        assert!(is_link());
        assert!(
            fs::read_to_string(&target)
                .unwrap()
                .contains("left_width_pct = 30")
        );

        let loaded = crate::config::load_config_file(&link).unwrap();
        let mut config = loaded.clone();
        config.ui.left_width_pct = 35;
        save_config_three_way(
            &link,
            Some(SaveBase::read(&loaded)),
            &config,
            Durability::NoFsync,
        )
        .unwrap();
        assert!(is_link());
        assert!(
            fs::read_to_string(&target)
                .unwrap()
                .contains("left_width_pct = 35")
        );

        replace_config_file(&link, |_| {
            Ok(("[ui]\nleft_width_pct = 40\n".to_string(), ()))
        })
        .unwrap();
        assert!(is_link());
        assert!(
            fs::read_to_string(&target)
                .unwrap()
                .contains("left_width_pct = 40")
        );
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the target is kept owner-only");
    }

    /// Nothing is created or replaced at a config.toml symlink whose target
    /// is missing: every writer refuses, naming the link and its target.
    #[test]
    fn no_writer_creates_anything_at_a_dangling_config_symlink() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let target = dir.path().join("gone.toml");
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let results = [
            mutate_config_file(&link, |doc| {
                doc["ui"]["left_width_pct"] = toml_edit::value(30);
                Ok(())
            })
            .map(|_| ()),
            replace_config_file(&link, |_| Ok(("[ui]\n".to_string(), ()))),
            write_config_secure(&link, "[ui]\n"),
            save_config_three_way(&link, None, &Config::default(), Durability::NoFsync).map(|_| ()),
        ];
        for result in results {
            let message = format!("{:#}", result.expect_err("refused"));
            assert!(message.contains(&target.display().to_string()), "{message}");
            assert!(
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert!(!target.exists(), "nothing was created at the target");
        }
    }
}

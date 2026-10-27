//! Saved remotes: `remotes.toml` in the config folder, owner-only, holding
//! each remote's URL, whether plain HTTP to it was allowed, the default
//! remote and the CLI sign-in tokens, none of which live in `config.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::output::{Listing, Row};
use super::{CliError, Exit};

pub(crate) const REMOTES_FILE: &str = "remotes.toml";

/// The saved remotes, as `remotes.toml` holds them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remotes {
    /// The remote used when none is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Every saved remote, by name.
    #[serde(default)]
    pub remotes: BTreeMap<String, Remote>,
}

/// Where plain HTTP may go: only to this machine and the tailnet, or, for a
/// remote added with `--insecure`, anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlainHttp {
    /// Only to this machine and the tailnet.
    SafeNetworksOnly,
    /// Anywhere: the remote was added with `--insecure`.
    Anywhere,
}

/// One saved remote.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    /// The remote's address.
    pub url: String,
    /// Plain HTTP to an address that is neither this machine nor the tailnet
    /// was allowed when it was added.
    #[serde(default)]
    pub insecure: bool,
    /// The CLI sign-in token, while signed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The instance id of the dux this remote signed in to, so a different
    /// dux answering at the same URL is noticed. Learned at sign-in, or on
    /// the first connection for a remote saved without one. A consistency
    /// check only, never an auth or trust input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

impl Remote {
    /// Where plain HTTP to this remote may go, as it was added.
    pub fn plain_http(&self) -> PlainHttp {
        if self.insecure {
            PlainHttp::Anywhere
        } else {
            PlainHttp::SafeNetworksOnly
        }
    }
}

/// The file every change to [`REMOTES_FILE`] holds an exclusive lock on.
/// The remotes file itself is replaced by each save, so it cannot be locked.
pub(crate) const REMOTES_LOCK_FILE: &str = "remotes.toml.lock";

pub(crate) fn remotes_path(root: &Path) -> PathBuf {
    root.join(REMOTES_FILE)
}

/// Take the exclusive lock on [`REMOTES_LOCK_FILE`], waiting for whoever
/// holds it. Released when the returned file closes.
fn lock(root: &Path) -> Result<std::fs::File, CliError> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = root.join(REMOTES_LOCK_FILE);
    let fail = |error: &dyn std::fmt::Display| {
        CliError::new(
            Exit::Failed,
            format!("could not lock {}: {error}", path.display()),
        )
    };
    crate::file_modes::create_private_dir_all(root).map_err(|e| fail(&e))?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(crate::file_modes::PRIVATE_FILE_MODE)
        .open(&path)
        .map_err(|e| fail(&e))?;
    crate::io_retry::retry_on_interrupt_errno(|| {
        rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive)
    })
    .map_err(|e| fail(&e))?;
    Ok(file)
}

impl Remotes {
    /// The saved remotes, or none when the file does not exist yet.
    ///
    /// # Errors
    ///
    /// Fails when the file exists but cannot be read or parsed.
    pub fn load(root: &Path) -> Result<Self, CliError> {
        let path = remotes_path(root);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(CliError::new(
                    Exit::Failed,
                    format!("could not read {}: {error}", path.display()),
                ));
            }
        };
        // The parser's own message quotes the file, and the file holds
        // sign-in tokens, so only the place it broke is said.
        toml::from_str(&text).map_err(|error| {
            let place = match error.span() {
                Some(span) => {
                    let before = text.get(..span.start).unwrap_or(&text);
                    let line = before.matches('\n').count() + 1;
                    let column = before.rsplit('\n').next().unwrap_or_default().chars().count() + 1;
                    format!(" at line {line}, column {column}")
                }
                None => String::new(),
            };
            CliError::new(
                Exit::Failed,
                format!(
                    "{} is not valid{place}; fix it by hand, or remove it and add your remotes again",
                    path.display()
                ),
            )
        })
    }

    /// Write the file whole, owner-only from the moment it exists, replacing
    /// the old one in one rename. Every change goes through [`Self::update`].
    fn save(&self, root: &Path) -> Result<(), CliError> {
        let path = remotes_path(root);
        let fail = |error: &dyn std::fmt::Display| {
            CliError::new(
                Exit::Failed,
                format!("could not write {}: {error}", path.display()),
            )
        };
        crate::file_modes::create_private_dir_all(root).map_err(|e| fail(&e))?;
        let text = toml::to_string(self).map_err(|e| fail(&e))?;
        // A NamedTempFile is created 0600.
        let mut file = tempfile::NamedTempFile::new_in(root).map_err(|e| fail(&e))?;
        std::io::Write::write_all(&mut file, text.as_bytes()).map_err(|e| fail(&e))?;
        file.persist(&path).map_err(|e| fail(&e.error))?;
        Ok(())
    }

    /// Load the file, `change` it and save it under an exclusive lock, so two commands changing
    /// it at once never lose either change. A `change` that fails saves nothing.
    ///
    /// # Errors
    ///
    /// Fails when the lock cannot be taken, the file cannot be read or written, or `change` fails.
    pub fn update<T>(
        root: &Path,
        change: impl FnOnce(&mut Remotes) -> Result<T, CliError>,
    ) -> Result<T, CliError> {
        let _lock = lock(root)?;
        let mut saved = Self::load(root)?;
        let result = change(&mut saved)?;
        saved.save(root)?;
        Ok(result)
    }

    /// Keep `token` as the sign-in to `name`, issued by the remote at `url`.
    ///
    /// # Errors
    ///
    /// [`Exit::Failed`], keeping nothing, when `name` was removed or now names another URL.
    pub fn keep_token(
        root: &Path,
        name: &str,
        url: &str,
        token: &str,
        instance: Option<&str>,
    ) -> Result<(), CliError> {
        Self::update(root, |saved| match saved.remotes.get_mut(name) {
            Some(remote) if remote.url == url => {
                remote.token = Some(token.to_string());
                remote.instance = instance.map(str::to_string);
                Ok(())
            }
            _ => Err(CliError::new(
                Exit::Failed,
                format!(
                    "{name} was removed or now points elsewhere than {url}, which it pointed at \
                     when signing in began, so the sign-in was not kept; sign in again"
                ),
            )),
        })
    }

    /// The remote saved as `name`.
    ///
    /// # Errors
    ///
    /// Fails when no remote is saved as `name`.
    pub fn get(&self, name: &str) -> Result<&Remote, CliError> {
        self.remotes.get(name).ok_or_else(|| unknown(name))
    }

    /// Save `url` as `name`. Plain HTTP is allowed to this machine and the
    /// tailnet; anywhere else only with [`PlainHttp::Anywhere`].
    ///
    /// # Errors
    ///
    /// Fails when the name is not usable or already taken, or the address is not one dux may reach
    /// this way.
    pub fn add(&mut self, name: &str, url: &str, plain_http: PlainHttp) -> Result<(), CliError> {
        self.add_from(
            name,
            url,
            plain_http,
            &crate::client::transport::local_addresses(),
        )
    }

    /// [`Self::add`], judged against this machine's interface addresses `local`.
    fn add_from(
        &mut self,
        name: &str,
        url: &str,
        plain_http: PlainHttp,
        local: &[std::net::IpAddr],
    ) -> Result<(), CliError> {
        if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(CliError::new(
                Exit::Usage,
                "a remote's name cannot be empty or hold spaces",
            ));
        }
        if self.remotes.contains_key(name) {
            return Err(CliError::new(
                Exit::Failed,
                format!(
                    "a remote named {name} is already saved; remove it first with \"dux remote rm {name}\""
                ),
            ));
        }
        let url = checked_url(url, plain_http, local)?;
        self.remotes.insert(
            name.to_string(),
            Remote {
                url,
                insecure: plain_http == PlainHttp::Anywhere,
                token: None,
                instance: None,
            },
        );
        Ok(())
    }

    /// Forget `name`, and stop using it as the default.
    ///
    /// # Errors
    ///
    /// Fails when no remote is saved as `name`.
    pub fn remove(&mut self, name: &str) -> Result<Remote, CliError> {
        let removed = self.remotes.remove(name).ok_or_else(|| unknown(name))?;
        if self.default.as_deref() == Some(name) {
            self.default = None;
        }
        Ok(removed)
    }

    /// Make `name` the default, or with `None` clear it.
    ///
    /// # Errors
    ///
    /// Fails when no remote is saved as `name`.
    pub fn set_default(&mut self, name: Option<&str>) -> Result<(), CliError> {
        if let Some(name) = name {
            self.get(name)?;
        }
        self.default = name.map(str::to_string);
        Ok(())
    }

    /// The remotes as `dux remote ls` prints them, by name. Tokens are never
    /// printed.
    pub fn listing(&self) -> Listing {
        let rows = self
            .remotes
            .iter()
            .map(|(name, remote)| {
                let default = self.default.as_deref() == Some(name.as_str());
                let signed_in = remote.token.is_some();
                Row {
                    id: name.clone(),
                    cells: vec![
                        name.clone(),
                        remote.url.clone(),
                        if default { "yes" } else { "" }.to_string(),
                        if signed_in { "yes" } else { "no" }.to_string(),
                    ],
                    json: serde_json::json!({
                        "name": name,
                        "url": remote.url,
                        "insecure": remote.insecure,
                        "default": default,
                        "signed_in": signed_in,
                    }),
                }
            })
            .collect();
        Listing {
            headers: vec!["NAME", "URL", "DEFAULT", "SIGNED IN"],
            rows,
        }
    }
}

fn unknown(name: &str) -> CliError {
    CliError::new(
        Exit::Usage,
        format!("no remote is saved as {name}; \"dux remote ls\" lists the saved ones"),
    )
}

/// Whether the authority of `url`, as written, holds an `@`: a user name or a
/// password may sit before it. Read from the raw text, so it holds for a URL
/// that does not parse too, and generous about where the authority ends.
fn names_a_user(url: &str) -> bool {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    after_scheme
        .split(['/', '?', '#'])
        .next()
        .is_some_and(|authority| authority.contains('@'))
}

/// `url` as saved: http or https, with a host, without a trailing slash.
/// `local` is this machine's interface addresses.
fn checked_url(
    url: &str,
    plain_http: PlainHttp,
    local: &[std::net::IpAddr],
) -> Result<String, CliError> {
    // First, and never echoed: what sits before the host may be a password,
    // and every later refusal repeats the URL.
    if names_a_user(url) {
        return Err(CliError::new(
            Exit::Usage,
            "the URL carries a user name or password; a remote is saved as its address alone, \
             and its password is asked for at \"dux remote login\"",
        ));
    }
    let parsed = url::Url::parse(url).map_err(|error| {
        CliError::new(
            Exit::Usage,
            format!(
                "{url} is not a URL dux can reach ({error}); write it like https://dux.example.com"
            ),
        )
    })?;
    let Some(host) = parsed.host() else {
        return Err(CliError::new(Exit::Usage, format!("{url} names no host")));
    };
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(CliError::new(
            Exit::Usage,
            format!(
                "{url} carries a query or a fragment; a remote is saved as its address alone, \
                 like https://dux.example.com"
            ),
        ));
    }
    match parsed.scheme() {
        "https" => {}
        "http" if plain_http == PlainHttp::Anywhere || plain_http_allowed(&host, local) => {}
        "http" if matches!(host, url::Host::Ipv4(ip) if crate::tailscale::is_tailscale_cgnat(ip)) =>
        {
            return Err(CliError::new(
                Exit::Usage,
                format!(
                    "{url} is plain HTTP to {host}, which is in Tailscale's address range, but \
                     this machine has no address in that range, so it is not on the tailnet and \
                     your password could cross an ordinary carrier network unencrypted. Use \
                     https://, or add --insecure if you trust every network between you and it"
                ),
            ));
        }
        "http" => {
            return Err(CliError::new(
                Exit::Usage,
                format!(
                    "{url} is plain HTTP to {host}, which is neither this machine nor a Tailscale \
                     address, so your password would cross the network unencrypted. Use https://, \
                     or add --insecure if you trust every network between you and it"
                ),
            ));
        }
        other => {
            return Err(CliError::new(
                Exit::Usage,
                format!("{url} uses {other}; a remote is reached over http:// or https://"),
            ));
        }
    }
    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

/// Whether plain HTTP to `host` keeps the password off an open network: loopback, or Tailscale
/// addresses (see [`crate::client::transport::tailnet_address`]) and names, whose traffic is
/// encrypted in transit. `local` is this machine's interface addresses.
fn plain_http_allowed(host: &url::Host<&str>, local: &[std::net::IpAddr]) -> bool {
    use crate::client::transport::tailnet_address;
    match host {
        url::Host::Domain(name) => {
            let name = name.to_ascii_lowercase();
            name == "localhost" || name.ends_with(".ts.net")
        }
        url::Host::Ipv4(ip) => ip.is_loopback() || tailnet_address((*ip).into(), local),
        url::Host::Ipv6(ip) => ip.is_loopback() || tailnet_address((*ip).into(), local),
    }
}

/// What every login to a remote added with `--insecure` prints first.
pub fn insecure_login_warning(name: &str, remote: &Remote) -> Option<String> {
    (remote.insecure && remote.url.starts_with("http://")).then(|| {
        format!(
            "warning: {name} is reached over plain HTTP, so this password crosses the network \
             unencrypted"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::test_server::private_dir;

    #[test]
    fn plain_http_is_refused_off_this_machine_and_the_tailnet_unless_insecure() {
        let mut remotes = Remotes::default();
        let refused = remotes
            .add(
                "lan",
                "http://192.168.1.20:3890",
                PlainHttp::SafeNetworksOnly,
            )
            .unwrap_err();
        assert_eq!(refused.exit, Exit::Usage);
        assert!(
            refused.message.contains("--insecure"),
            "{}",
            refused.message
        );
        assert!(remotes.remotes.is_empty());

        for (name, url) in [
            ("loop", "http://127.0.0.1:3890"),
            ("named", "http://localhost:3890/"),
            ("v6", "http://[::1]:3890"),
            ("tail", "http://100.101.2.3:3890"),
            ("tail6", "http://[fd7a:115c:a1e0::5]:3890"),
            ("magic", "http://box.tail1234.ts.net:3890"),
            ("tls", "https://dux.example.com"),
            ("lan", "http://192.168.1.20:3890"),
        ] {
            remotes
                .add_from(
                    name,
                    url,
                    if name == "lan" {
                        PlainHttp::Anywhere
                    } else {
                        PlainHttp::SafeNetworksOnly
                    },
                    ON_THE_TAILNET,
                )
                .unwrap_or_else(|e| panic!("{url}: {e}"));
        }
        assert_eq!(remotes.get("named").unwrap().url, "http://localhost:3890");
        assert!(remotes.get("lan").unwrap().insecure);
        assert!(
            insecure_login_warning("lan", remotes.get("lan").unwrap()).is_some(),
            "every login to it warns"
        );
        assert!(insecure_login_warning("tls", remotes.get("tls").unwrap()).is_none());
        assert!(
            remotes
                .add("ftp", "ftp://example.com", PlainHttp::Anywhere)
                .is_err()
        );
        assert!(
            remotes
                .add(
                    "cgnat-edge",
                    "http://100.128.0.1",
                    PlainHttp::SafeNetworksOnly
                )
                .is_err(),
            "100.128/10 is past Tailscale's range"
        );
    }

    /// This machine's addresses while it is on the tailnet.
    const ON_THE_TAILNET: &[std::net::IpAddr] = &[std::net::IpAddr::V4(std::net::Ipv4Addr::new(
        100, 100, 1, 1,
    ))];

    /// 100.64.0.0/10 is carrier-grade NAT space too, so plain HTTP to it is
    /// taken for the tailnet only while this machine holds an address there.
    #[test]
    fn plain_http_to_tailscales_ipv4_range_needs_this_machine_on_the_tailnet() {
        let off: &[std::net::IpAddr] =
            &["192.168.1.5".parse().unwrap(), "127.0.0.1".parse().unwrap()];
        let mut remotes = Remotes::default();
        let refused = remotes
            .add_from(
                "tail",
                "http://100.101.2.3:3890",
                PlainHttp::SafeNetworksOnly,
                off,
            )
            .unwrap_err();
        assert_eq!(refused.exit, Exit::Usage);
        assert!(
            refused.message.contains("https://") && refused.message.contains("--insecure"),
            "{}",
            refused.message
        );
        assert!(remotes.remotes.is_empty());
        remotes
            .add_from(
                "tail",
                "http://100.101.2.3:3890",
                PlainHttp::SafeNetworksOnly,
                ON_THE_TAILNET,
            )
            .unwrap();
        remotes
            .add_from(
                "anyway",
                "http://100.101.2.4:3890",
                PlainHttp::Anywhere,
                off,
            )
            .unwrap();
    }

    #[test]
    fn a_url_with_a_query_a_fragment_or_a_user_is_refused() {
        let mut remotes = Remotes::default();
        for url in [
            "https://dux.example.com/?x=1",
            "https://dux.example.com/#top",
            "https://me@dux.example.com",
            "https://me:secret@dux.example.com",
            "https://me:secret@dux.example.com:99999",
            "me:secret@dux.example.com",
            "https://me:secret@",
        ] {
            let refused = remotes
                .add("work", url, PlainHttp::SafeNetworksOnly)
                .unwrap_err();
            assert_eq!(refused.exit, Exit::Usage, "{url}");
            assert!(!refused.message.contains("secret"), "{}", refused.message);
        }
        assert!(remotes.remotes.is_empty());
    }

    #[test]
    fn the_remotes_file_round_trips_owner_only_keeps_a_token_only_for_its_url_and_never_echoes_its_text()
     {
        use std::os::unix::fs::PermissionsExt;
        let dir = private_dir();
        let root = dir.path().join("home");
        assert_eq!(Remotes::load(&root).unwrap(), Remotes::default());

        let mut remotes = Remotes::default();
        remotes
            .add(
                "work",
                "https://work.example.com/",
                PlainHttp::SafeNetworksOnly,
            )
            .unwrap();
        remotes
            .add("home", "http://127.0.0.1:3890", PlainHttp::SafeNetworksOnly)
            .unwrap();
        remotes.set_default(Some("work")).unwrap();
        remotes.remotes.get_mut("work").unwrap().token = Some("t0k".into());
        remotes.save(&root).unwrap();

        let path = root.join("remotes.toml");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let loaded = Remotes::load(&root).unwrap();
        assert_eq!(loaded, remotes);

        let listing = loaded.listing();
        let names: Vec<&str> = listing.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(names, ["home", "work"]);
        assert_eq!(listing.rows[1].cells[2], "yes");
        assert_eq!(listing.rows[1].cells[3], "yes");
        assert!(
            !serde_json::to_string(&listing.rows[1].json)
                .unwrap()
                .contains("t0k"),
            "a listing never carries the token"
        );

        let mut changed = loaded;
        assert!(changed.set_default(Some("nowhere")).is_err());
        changed.remove("work").unwrap();
        assert_eq!(changed.default, None);
        assert!(changed.remove("work").is_err());

        // A sign-in is kept only while the remote still has the URL it was
        // issued by.
        Remotes::update(&root, |saved| {
            saved.remove("work")?;
            saved.add(
                "work",
                "https://other.example.com",
                PlainHttp::SafeNetworksOnly,
            )
        })
        .unwrap();
        let refused = Remotes::keep_token(&root, "work", "https://work.example.com", "new", None)
            .unwrap_err();
        assert_eq!(refused.exit, Exit::Failed);
        assert!(refused.message.contains("work"), "{}", refused.message);
        assert_eq!(
            Remotes::load(&root).unwrap().get("work").unwrap().token,
            None
        );
        Remotes::keep_token(&root, "work", "https://other.example.com", "new", None).unwrap();
        assert_eq!(
            Remotes::load(&root)
                .unwrap()
                .get("work")
                .unwrap()
                .token
                .as_deref(),
            Some("new")
        );

        // A broken file is reported by where it breaks, never by its text.
        for (text, line) in [
            (
                "[remotes.work]\nurl = \"https://x\"\ninsecure = \"SECRET-TOKEN\"\n",
                "line 3",
            ),
            ("[remotes.work]\ntoken = \"SECRET-TOKEN\n", "line 2"),
        ] {
            std::fs::write(&path, text).unwrap();
            let broken = Remotes::load(&root).unwrap_err();
            assert!(!broken.message.contains("SECRET"), "{}", broken.message);
            assert!(broken.message.contains(line), "{}", broken.message);
        }
    }

    #[test]
    fn concurrent_changes_to_the_remotes_file_are_never_lost() {
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = private_dir();
        let root = dir.path().to_path_buf();
        let (loaded, has_loaded) = mpsc::channel();
        let (go, may_go) = mpsc::channel::<()>();
        let first_root = root.clone();
        let first = std::thread::spawn(move || {
            Remotes::update(&first_root, |saved| {
                loaded.send(()).unwrap();
                let _ = may_go.recv_timeout(Duration::from_millis(500));
                saved.add("a", "https://a.example.com", PlainHttp::SafeNetworksOnly)
            })
        });
        has_loaded.recv().unwrap();
        let second_root = root.clone();
        let second = std::thread::spawn(move || {
            Remotes::update(&second_root, |saved| {
                saved.add("b", "https://b.example.com", PlainHttp::SafeNetworksOnly)
            })
        });
        std::thread::sleep(Duration::from_millis(200));
        let _ = go.send(());
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        let names: Vec<String> = Remotes::load(&root).unwrap().remotes.into_keys().collect();
        assert_eq!(names, ["a", "b"]);
    }
}

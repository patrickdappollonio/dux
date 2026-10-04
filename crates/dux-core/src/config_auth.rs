//! `[server.auth]`: the web login's settings.
//!
//! This section FAILS CLOSED. Every other section of `config.toml` recovers
//! from a bad value by resetting it to its default, but here the default is
//! "no password", so a typo would quietly open dux to anyone who can reach it.
//! Instead, an unknown key, a value of the wrong type, a value outside its
//! range or a password hash dux will not use makes the whole section invalid,
//! and an invalid section stops dux from starting and stops a reload from
//! changing the running config (see `crate::config::load_config`). The checks
//! live in [`ServerAuthConfig`]'s deserializer, so every reader of the file
//! (startup, reload, the raw config editor, `dux config set`) applies the same
//! rules with no way to skip them.

use std::fmt;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// Where the password applies once one is set. No effect without a password.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthRequire {
    /// Everything except this machine and the owner's own tailnet.
    #[default]
    Network,
    /// Everything except this machine.
    Tailnet,
    /// Every request, this machine included.
    Everywhere,
}

impl AuthRequire {
    /// Every value, in the order the config comment lists them.
    pub const ALL: [Self; 3] = [Self::Network, Self::Tailnet, Self::Everywhere];

    /// The config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Tailnet => "tailnet",
            Self::Everywhere => "everywhere",
        }
    }
}

/// Whether the session cookie carries the `Secure` flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CookieSecure {
    /// Decided per request: on when dux knows the browser reached it over
    /// HTTPS (a confirmed `tailscale serve` HTTPS route), off otherwise.
    #[default]
    Auto,
    /// Always set. A browser on plain HTTP then cannot keep the cookie, so
    /// it cannot stay signed in.
    Always,
    /// Never set.
    Never,
}

impl CookieSecure {
    /// Every value, in the order the config comment lists them.
    pub const ALL: [Self; 3] = [Self::Auto, Self::Always, Self::Never];

    /// The config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

pub const DEFAULT_MINIMUM_PASSWORD_LENGTH: u32 = 12;
pub const DEFAULT_MINIMUM_PASSWORD_SCORE: u8 = 2;
pub const DEFAULT_MAX_FAILED_LOGINS: u32 = 5;
pub const DEFAULT_SESSION_IDLE_SECONDS: u32 = 60;
pub const DEFAULT_MAX_CONCURRENT_PASSWORD_CHECKS: u32 = 2;
pub const DEFAULT_PASSWORD_CHECK_QUEUE: u32 = 8;
pub const DEFAULT_MAX_PASSWORD_BYTES: u32 = 1024;
/// The largest `max_password_bytes` accepted: a login body is read into
/// memory before anything else happens, so this bounds that read.
pub const MAX_PASSWORD_BYTES_LIMIT: u32 = 64 * 1024;
pub const DEFAULT_FAILED_LOGIN_WINDOW_SECONDS: u32 = 15 * 60;
pub const DEFAULT_FAILED_LOGIN_DELAY_SECONDS: u32 = 1;
pub const DEFAULT_FAILED_LOGIN_MAX_DELAY_SECONDS: u32 = 30;
pub const DEFAULT_MAX_FAILED_LOGINS_PER_MINUTE: u32 = 30;
pub const DEFAULT_MAX_TRACKED_ADDRESSES: u32 = 10_000;
pub const DEFAULT_MAX_BLOCKED_ADDRESSES: u32 = 1_000;

/// The `[server.auth]` table. Field meanings are documented once, in the
/// canonical config template (`crates/dux-tui/src/config.rs`); the summaries
/// here are for code readers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawServerAuthConfig")]
pub struct ServerAuthConfig {
    /// Argon2id PHC string, or empty for no password. Validated by
    /// [`crate::auth::validate_password_hash`] when read.
    pub password_hash: String,
    pub require: AuthRequire,
    /// Characters a new password must have.
    pub minimum_password_length: u32,
    /// zxcvbn score (0 to 4) a new password must reach.
    pub minimum_password_score: u8,
    /// Failed logins from one address, within `failed_login_window_seconds`,
    /// before dux adds it to `blocked_addresses`. 0 never blocks.
    pub max_failed_logins: u32,
    /// Addresses and CIDR ranges refused before anything else, with or
    /// without a password. Each entry parses as an [`AddressBlock`].
    ///
    /// A save from memory never writes this list: it is one plain value, so
    /// a three-way save would write it whole, and a ban another dux added
    /// meanwhile would be lost. Every change to it, an automatic ban
    /// included, goes through the locked mutation path
    /// ([`crate::config_write::mutate_config_file`]), which changes it in the
    /// file as it is under the config write lock.
    pub blocked_addresses: Vec<String>,
    /// Seconds a session survives with no request and no open socket.
    pub session_idle_seconds: u32,
    pub disable_no_auth_warning: bool,
    pub cookie_secure: CookieSecure,
    /// Password checks (Argon2id runs) at once, across every client.
    pub max_concurrent_password_checks: u32,
    /// Logins allowed to wait for a free check; beyond it, "too many requests".
    pub password_check_queue: u32,
    /// Largest password the login accepts, in bytes.
    pub max_password_bytes: u32,
    /// How long a failed login is remembered against its address.
    pub failed_login_window_seconds: u32,
    /// Wait imposed on an address after its first failure in the window,
    /// doubling with each further failure up to the next setting.
    pub failed_login_delay_seconds: u32,
    pub failed_login_max_delay_seconds: u32,
    /// Failed logins per minute across all addresses before every address
    /// but this machine gets "too many requests" until the minute passes.
    pub max_failed_logins_per_minute: u32,
    /// Addresses the failure tracker remembers at once; the oldest goes first.
    pub max_tracked_addresses: u32,
    /// Size `blocked_addresses` may reach through dux's own appends. Beyond
    /// it a new ban holds in memory for the run and dux warns.
    pub max_blocked_addresses: u32,
}

impl Default for ServerAuthConfig {
    fn default() -> Self {
        Self {
            password_hash: String::new(),
            require: AuthRequire::default(),
            minimum_password_length: DEFAULT_MINIMUM_PASSWORD_LENGTH,
            minimum_password_score: DEFAULT_MINIMUM_PASSWORD_SCORE,
            max_failed_logins: DEFAULT_MAX_FAILED_LOGINS,
            blocked_addresses: Vec::new(),
            session_idle_seconds: DEFAULT_SESSION_IDLE_SECONDS,
            disable_no_auth_warning: false,
            cookie_secure: CookieSecure::default(),
            max_concurrent_password_checks: DEFAULT_MAX_CONCURRENT_PASSWORD_CHECKS,
            password_check_queue: DEFAULT_PASSWORD_CHECK_QUEUE,
            max_password_bytes: DEFAULT_MAX_PASSWORD_BYTES,
            failed_login_window_seconds: DEFAULT_FAILED_LOGIN_WINDOW_SECONDS,
            failed_login_delay_seconds: DEFAULT_FAILED_LOGIN_DELAY_SECONDS,
            failed_login_max_delay_seconds: DEFAULT_FAILED_LOGIN_MAX_DELAY_SECONDS,
            max_failed_logins_per_minute: DEFAULT_MAX_FAILED_LOGINS_PER_MINUTE,
            max_tracked_addresses: DEFAULT_MAX_TRACKED_ADDRESSES,
            max_blocked_addresses: DEFAULT_MAX_BLOCKED_ADDRESSES,
        }
    }
}

impl ServerAuthConfig {
    /// The stored hash, or `None` when no password is set.
    pub fn password_hash(&self) -> Option<&str> {
        (!self.password_hash.is_empty()).then_some(self.password_hash.as_str())
    }

    /// Whether a password is set.
    pub fn has_password(&self) -> bool {
        self.password_hash().is_some()
    }

    /// The minimums a new password is checked against.
    pub fn password_policy(&self) -> crate::auth::PasswordPolicy {
        crate::auth::PasswordPolicy {
            minimum_length: self.minimum_password_length,
            minimum_score: self.minimum_password_score,
            maximum_bytes: self.max_password_bytes,
        }
    }

    /// `blocked_addresses`, parsed. Every entry already parsed when the
    /// section was read, so this cannot fail.
    pub fn blocked(&self) -> Vec<AddressBlock> {
        self.blocked_addresses
            .iter()
            .filter_map(|entry| AddressBlock::parse(entry).ok())
            .collect()
    }

    /// Whether `ip` falls inside any `blocked_addresses` entry. An
    /// IPv4-mapped IPv6 address is compared as the IPv4 address it carries.
    pub fn is_blocked(&self, ip: IpAddr) -> bool {
        self.blocked().iter().any(|block| block.contains(ip))
    }

    /// Every rule the deserializer enforces beyond types. Public so a writer
    /// can check a candidate before it lands on disk.
    pub fn validate(&self) -> Result<(), String> {
        match self.problems().into_iter().next() {
            Some(problem) => Err(problem),
            None => Ok(()),
        }
    }

    /// Every rule this section breaks, in [`Self::validate`]'s order.
    /// Problems name settings and positions, never values: they reach the
    /// status line, toasts and dux.log.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if !self.password_hash.is_empty()
            && let Err(error) = crate::auth::validate_password_hash(&self.password_hash)
        {
            problems.push(error.to_string());
        }
        if self.minimum_password_score > 4 {
            problems.push("minimum_password_score must be 0 to 4".to_string());
        }
        if self.session_idle_seconds == 0 {
            problems.push("session_idle_seconds must be at least 1".to_string());
        }
        if self.max_concurrent_password_checks == 0 {
            problems.push(
                "max_concurrent_password_checks must be at least 1, or nobody could log in"
                    .to_string(),
            );
        }
        let max_bytes_valid =
            self.max_password_bytes != 0 && self.max_password_bytes <= MAX_PASSWORD_BYTES_LIMIT;
        if !max_bytes_valid {
            problems.push(format!(
                "max_password_bytes must be 1 to {MAX_PASSWORD_BYTES_LIMIT}"
            ));
        }
        if max_bytes_valid && self.minimum_password_length > self.max_password_bytes {
            problems.push(
                "minimum_password_length is larger than max_password_bytes, so no \
                 password could meet both"
                    .to_string(),
            );
        }
        if self.max_tracked_addresses == 0 {
            problems.push(
                "max_tracked_addresses must be at least 1, or failed logins would never count"
                    .to_string(),
            );
        }
        for (index, entry) in self.blocked_addresses.iter().enumerate() {
            if let Err(reason) = AddressBlock::parse(entry) {
                problems.push(format!(
                    "blocked_addresses entry {} (counting from 1): {reason}",
                    index + 1
                ));
            }
        }
        problems
    }
}

/// Every rule a `server.auth` table breaks (see
/// [`ServerAuthConfig::problems`]), or the type error that stops it being
/// read at all.
pub fn rule_problems_of(auth: toml::Value) -> Result<Vec<String>, String> {
    let raw: RawServerAuthConfig = auth
        .try_into()
        .map_err(|e: toml::de::Error| e.message().to_string())?;
    Ok(ServerAuthConfig::from_raw(raw).problems())
}

/// The on-disk shape, read with no rules beyond types and known keys; the
/// conversion into [`ServerAuthConfig`] applies [`ServerAuthConfig::validate`].
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawServerAuthConfig {
    password_hash: String,
    require: AuthRequire,
    minimum_password_length: u32,
    minimum_password_score: u8,
    max_failed_logins: u32,
    blocked_addresses: Vec<String>,
    session_idle_seconds: u32,
    disable_no_auth_warning: bool,
    cookie_secure: CookieSecure,
    max_concurrent_password_checks: u32,
    password_check_queue: u32,
    max_password_bytes: u32,
    failed_login_window_seconds: u32,
    failed_login_delay_seconds: u32,
    failed_login_max_delay_seconds: u32,
    max_failed_logins_per_minute: u32,
    max_tracked_addresses: u32,
    max_blocked_addresses: u32,
}

impl Default for RawServerAuthConfig {
    fn default() -> Self {
        let d = ServerAuthConfig::default();
        Self {
            password_hash: d.password_hash,
            require: d.require,
            minimum_password_length: d.minimum_password_length,
            minimum_password_score: d.minimum_password_score,
            max_failed_logins: d.max_failed_logins,
            blocked_addresses: d.blocked_addresses,
            session_idle_seconds: d.session_idle_seconds,
            disable_no_auth_warning: d.disable_no_auth_warning,
            cookie_secure: d.cookie_secure,
            max_concurrent_password_checks: d.max_concurrent_password_checks,
            password_check_queue: d.password_check_queue,
            max_password_bytes: d.max_password_bytes,
            failed_login_window_seconds: d.failed_login_window_seconds,
            failed_login_delay_seconds: d.failed_login_delay_seconds,
            failed_login_max_delay_seconds: d.failed_login_max_delay_seconds,
            max_failed_logins_per_minute: d.max_failed_logins_per_minute,
            max_tracked_addresses: d.max_tracked_addresses,
            max_blocked_addresses: d.max_blocked_addresses,
        }
    }
}

impl TryFrom<RawServerAuthConfig> for ServerAuthConfig {
    type Error = String;

    fn try_from(raw: RawServerAuthConfig) -> Result<Self, String> {
        let config = Self::from_raw(raw);
        config.validate()?;
        Ok(config)
    }
}

impl ServerAuthConfig {
    /// The section as read, before any rule is checked.
    fn from_raw(raw: RawServerAuthConfig) -> Self {
        Self {
            password_hash: raw.password_hash,
            require: raw.require,
            minimum_password_length: raw.minimum_password_length,
            minimum_password_score: raw.minimum_password_score,
            max_failed_logins: raw.max_failed_logins,
            blocked_addresses: raw.blocked_addresses,
            session_idle_seconds: raw.session_idle_seconds,
            disable_no_auth_warning: raw.disable_no_auth_warning,
            cookie_secure: raw.cookie_secure,
            max_concurrent_password_checks: raw.max_concurrent_password_checks,
            password_check_queue: raw.password_check_queue,
            max_password_bytes: raw.max_password_bytes,
            failed_login_window_seconds: raw.failed_login_window_seconds,
            failed_login_delay_seconds: raw.failed_login_delay_seconds,
            failed_login_max_delay_seconds: raw.failed_login_max_delay_seconds,
            max_failed_logins_per_minute: raw.max_failed_logins_per_minute,
            max_tracked_addresses: raw.max_tracked_addresses,
            max_blocked_addresses: raw.max_blocked_addresses,
        }
    }
}

/// One `blocked_addresses` entry: a single address (`203.0.113.7`,
/// `2001:db8::1`) or a CIDR range (`203.0.113.0/24`, `2001:db8::/32`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressBlock {
    network: IpAddr,
    prefix: u8,
}

impl AddressBlock {
    /// Parse an entry. Surrounding whitespace is not accepted, so what the
    /// file says is exactly what is matched.
    ///
    /// An IPv4-mapped IPv6 entry (`::ffff:203.0.113.0/120`) whose prefix
    /// reaches into the mapped part (96 or more) is the IPv4 range it covers
    /// (`203.0.113.0/24`), so it reads and matches like the IPv4 form; with
    /// a shorter prefix it stays an IPv6 range. Its prefix is checked
    /// against 128, the length of what was written.
    pub fn parse(entry: &str) -> Result<Self, String> {
        let (addr, prefix) = match entry.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (entry, None),
        };
        let written: IpAddr = addr
            .parse()
            .map_err(|_| "not an IP address or CIDR range".to_string())?;
        let max: u8 = if written.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            None => max,
            Some(text) => {
                let value: u8 = text
                    .parse()
                    .map_err(|_| format!("the prefix length must be a number from 0 to {max}"))?;
                if value > max {
                    return Err(format!("the prefix length must be 0 to {max}"));
                }
                value
            }
        };
        if let IpAddr::V6(v6) = written
            && let Some(v4) = v6.to_ipv4_mapped()
            && prefix >= 96
        {
            return Ok(Self {
                network: IpAddr::V4(v4),
                prefix: prefix - 96,
            });
        }
        Ok(Self {
            network: written,
            prefix,
        })
    }

    /// Whether `ip` is inside this block. A client address is compared in
    /// its canonical form ([`canonical`]): an IPv4 client, however it was
    /// reported, is inside an IPv6 range that covers its mapped form.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, canonical(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                prefix_match(&net.octets(), &ip.octets(), self.prefix)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                prefix_match(&net.octets(), &ip.octets(), self.prefix)
            }
            (IpAddr::V6(net), IpAddr::V4(ip)) => {
                prefix_match(&net.octets(), &ip.to_ipv6_mapped().octets(), self.prefix)
            }
            (IpAddr::V4(_), IpAddr::V6(_)) => false,
        }
    }
}

impl fmt::Display for AddressBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let full = if self.network.is_ipv4() { 32 } else { 128 };
        if self.prefix == full {
            write!(f, "{}", self.network)
        } else {
            write!(f, "{}/{}", self.network, self.prefix)
        }
    }
}

/// An IPv4-mapped IPv6 address (`::ffff:203.0.113.7`) is the IPv4 address it
/// carries: a dual-stack listener reports IPv4 clients that way.
pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

fn prefix_match(net: &[u8], ip: &[u8], prefix: u8) -> bool {
    let full = usize::from(prefix / 8);
    let rest = prefix % 8;
    if net[..full] != ip[..full] {
        return false;
    }
    if rest == 0 {
        return true;
    }
    let mask = 0xffu8 << (8 - rest);
    (net[full] & mask) == (ip[full] & mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Result<ServerAuthConfig, String> {
        toml::from_str::<ServerAuthConfig>(body).map_err(|e| e.to_string())
    }

    fn real_hash() -> String {
        crate::auth::hash_password(&crate::auth::Password::new(
            "correct horse battery staple".to_string(),
        ))
        .expect("hash")
    }

    #[test]
    fn an_empty_section_is_the_defaults() {
        let config = parse("").expect("empty section");
        assert_eq!(config, ServerAuthConfig::default());
        assert!(!config.has_password());
        assert_eq!(config.require, AuthRequire::Network);
        assert_eq!(config.cookie_secure, CookieSecure::Auto);
        assert_eq!(config.minimum_password_length, 12);
        assert_eq!(config.minimum_password_score, 2);
        assert_eq!(config.max_failed_logins, 5);
        assert_eq!(config.session_idle_seconds, 60);
    }

    #[test]
    fn a_full_valid_section_reads_back() {
        let hash = real_hash();
        let body = format!(
            "password_hash = \"{hash}\"\nrequire = \"everywhere\"\ncookie_secure = \"always\"\n\
             blocked_addresses = [\"203.0.113.7\", \"2001:db8::/32\"]\n"
        );
        let config = parse(&body).expect("valid");
        assert_eq!(config.password_hash(), Some(hash.as_str()));
        assert_eq!(config.require, AuthRequire::Everywhere);
        assert_eq!(config.cookie_secure, CookieSecure::Always);
        assert!(config.is_blocked("203.0.113.7".parse().unwrap()));
        assert!(config.is_blocked("2001:db8:1::5".parse().unwrap()));
        assert!(!config.is_blocked("203.0.113.8".parse().unwrap()));
    }

    #[test]
    fn a_misspelled_key_is_an_error_not_a_missing_password() {
        let err = parse("pasword_hash = \"x\"\n").unwrap_err();
        assert!(err.contains("pasword_hash"), "{err}");
    }

    #[test]
    fn values_outside_their_rules_are_errors() {
        for (body, needle) in [
            ("password_hash = \"hunter2\"\n", "PHC"),
            ("require = \"lan\"\n", "lan"),
            ("cookie_secure = \"sometimes\"\n", "sometimes"),
            ("minimum_password_score = 5\n", "minimum_password_score"),
            ("session_idle_seconds = 0\n", "session_idle_seconds"),
            (
                "max_concurrent_password_checks = 0\n",
                "max_concurrent_password_checks",
            ),
            ("max_password_bytes = 0\n", "max_password_bytes"),
            ("max_password_bytes = 10\n", "minimum_password_length"),
            ("max_tracked_addresses = 0\n", "max_tracked_addresses"),
            ("blocked_addresses = [\"not-an-ip\"]\n", "entry 1"),
            ("blocked_addresses = [\"10.0.0.0/33\"]\n", "prefix"),
            ("session_idle_seconds = \"sixty\"\n", "session_idle_seconds"),
            ("blocked_addresses = \"203.0.113.7\"\n", "blocked_addresses"),
            ("max_failed_logins = -1\n", "max_failed_logins"),
        ] {
            let err = parse(body).expect_err(body);
            assert!(err.contains(needle), "{body:?}: {err}");
        }
    }

    #[test]
    fn address_blocks_match_cidr_ranges_and_mapped_addresses() {
        let block = AddressBlock::parse("203.0.113.0/24").unwrap();
        assert!(block.contains("203.0.113.255".parse().unwrap()));
        assert!(!block.contains("203.0.114.0".parse().unwrap()));
        assert!(block.contains("::ffff:203.0.113.9".parse().unwrap()));
        assert!(!block.contains("2001:db8::1".parse().unwrap()));

        let odd = AddressBlock::parse("10.0.0.0/9").unwrap();
        assert!(odd.contains("10.127.255.255".parse().unwrap()));
        assert!(!odd.contains("10.128.0.0".parse().unwrap()));

        let everything = AddressBlock::parse("0.0.0.0/0").unwrap();
        assert!(everything.contains("198.51.100.1".parse().unwrap()));

        let mapped = AddressBlock::parse("::ffff:198.51.100.4").unwrap();
        assert!(mapped.contains("198.51.100.4".parse().unwrap()));
        assert_eq!(mapped.to_string(), "198.51.100.4");
        assert_eq!(block.to_string(), "203.0.113.0/24");
    }

    #[test]
    fn surrounding_whitespace_is_not_an_address() {
        assert!(AddressBlock::parse(" 203.0.113.7").is_err());
    }

    #[test]
    fn serializing_round_trips() {
        let config = ServerAuthConfig {
            password_hash: real_hash(),
            require: AuthRequire::Tailnet,
            blocked_addresses: vec!["198.51.100.0/24".to_string()],
            ..ServerAuthConfig::default()
        };
        let text = toml::to_string(&config).expect("serialize");
        assert_eq!(parse(&text).expect("parse back"), config);
    }

    /// An IPv4-mapped IPv6 range is the IPv4 range it covers when its
    /// prefix reaches into the mapped part, and an IPv6 range otherwise;
    /// either way it matches the canonical (IPv4) form of a client address.
    #[test]
    fn ipv4_mapped_ranges_are_valid_blocked_addresses() {
        let block = AddressBlock::parse("::ffff:203.0.113.0/120").expect("valid");
        assert_eq!(block.to_string(), "203.0.113.0/24");
        assert!(block.contains("203.0.113.9".parse().unwrap()));
        assert!(block.contains("::ffff:203.0.113.9".parse().unwrap()));
        assert!(!block.contains("203.0.114.9".parse().unwrap()));

        let all_ipv4 = AddressBlock::parse("::ffff:0:0/96").expect("valid");
        assert!(all_ipv4.contains("198.51.100.1".parse().unwrap()));
        assert!(all_ipv4.contains("::ffff:198.51.100.1".parse().unwrap()));
        assert!(!all_ipv4.contains("2001:db8::1".parse().unwrap()));

        let wide = AddressBlock::parse("::ffff:0:0/80").expect("valid, as an IPv6 range");
        assert!(wide.contains("198.51.100.1".parse().unwrap()));
        assert!(!wide.contains("2001:db8::1".parse().unwrap()));

        let error = AddressBlock::parse("::ffff:203.0.113.0/129").expect_err("out of range");
        assert!(error.contains("0 to 128"), "{error}");
        assert!(
            ServerAuthConfig {
                blocked_addresses: vec!["::ffff:203.0.113.0/120".to_string()],
                ..ServerAuthConfig::default()
            }
            .validate()
            .is_ok()
        );
    }
}

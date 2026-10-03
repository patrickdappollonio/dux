//! The web login's password: hashing, verification, and strength.
//!
//! dux has one optional password for one owner (single tenancy is unchanged).
//! What is stored is an Argon2id hash in PHC string form at
//! `[server.auth] password_hash`; the plaintext never reaches config, a log, or
//! long-lived state. This module is the only place that turns a plaintext into a
//! hash or checks one against it, and every caller (`dux config set`, the web
//! login, the web password change) goes through it:
//!
//! - [`Password`] holds a plaintext in a buffer that is wiped when dropped.
//! - [`hash_password`] makes a new PHC string with [`ARGON2_M_COST_KIB`],
//!   [`ARGON2_T_COST`] and [`ARGON2_P_COST`].
//! - [`validate_password_hash`] checks a stored string before it is ever used:
//!   Argon2id, version 19, and cost parameters inside the bounds below, so a
//!   pasted hash cannot make every login allocate gigabytes or spin for minutes.
//! - [`verify_password`] answers whether a plaintext matches a stored hash. It
//!   is CPU and memory heavy by design; call it off any UI or async thread.
//! - [`strength`] scores a plaintext 0 to 4 with zxcvbn, and
//!   [`check_minimums`] compares it against the configured
//!   [`PasswordPolicy`].
//!
//! Honest limit: zeroizing narrows how long a plaintext sits in memory, it
//! cannot erase every copy (the strength estimator and the kernel's own buffers
//! make theirs), and a process running as the same user can read dux's memory.

use std::fmt;

use argon2::password_hash::PasswordHasher;
use argon2::password_hash::PasswordVerifier;
use argon2::password_hash::phc::PasswordHash;
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

/// Argon2id memory cost of a NEW hash, in KiB: OWASP's recommended
/// configuration (19 MiB, two passes, one lane). Existing hashes keep the
/// parameters written in their PHC string, so raising these later never
/// breaks a stored password.
pub const ARGON2_M_COST_KIB: u32 = 19 * 1024;
/// Argon2id passes of a NEW hash. See [`ARGON2_M_COST_KIB`].
pub const ARGON2_T_COST: u32 = 2;
/// Argon2id lanes of a NEW hash. See [`ARGON2_M_COST_KIB`].
pub const ARGON2_P_COST: u32 = 1;

/// The largest memory cost a stored hash may ask for, in KiB (256 MiB). A
/// login runs one verification per attempt, so an absurd value here would be
/// a memory bomb on every attempt.
pub const MAX_M_COST_KIB: u32 = 256 * 1024;
/// The largest number of passes a stored hash may ask for.
pub const MAX_T_COST: u32 = 16;
/// The largest number of lanes a stored hash may ask for.
pub const MAX_P_COST: u32 = 16;
/// The smallest memory cost a stored hash may use, in KiB: OWASP's floor
/// (7 MiB, paired there with five passes).
pub const MIN_M_COST_KIB: u32 = 7 * 1024;
/// The smallest total work (memory in KiB times passes) a stored hash may
/// use. Every configuration OWASP lists reaches about this much (19 MiB times
/// two passes, 7 MiB times five), so a weaker pasted hash is refused rather
/// than quietly protecting dux with a hash that is cheap to guess offline.
pub const MIN_WORK_KIB_PASSES: u64 = 35 * 1024;
/// Accepted digest sizes of a stored hash, in bytes.
const OUTPUT_LEN_RANGE: std::ops::RangeInclusive<usize> = 16..=64;

/// A plaintext password, wiped from memory when dropped. Its `Debug` never
/// prints the text, so it cannot leak through a `{:?}` in a log line.
pub struct Password(Zeroizing<String>);

impl Password {
    /// Take ownership of `text`. The original allocation becomes this
    /// buffer, so no unwiped copy is left behind by the move.
    pub fn new(text: String) -> Self {
        Self(Zeroizing::new(text))
    }

    /// Take ownership of raw bytes read from a request or a pipe. Bytes that
    /// are not UTF-8 are wiped before the error returns.
    pub fn from_utf8(bytes: Vec<u8>) -> Result<Self, AuthError> {
        match String::from_utf8(bytes) {
            Ok(text) => Ok(Self::new(text)),
            Err(error) => {
                drop(Zeroizing::new(error.into_bytes()));
                Err(AuthError::NotUtf8)
            }
        }
    }

    /// The plaintext. Borrow it only as long as a call needs it.
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }

    /// Its length in characters (Unicode scalar values), the unit
    /// `minimum_password_length` counts in.
    pub fn char_len(&self) -> usize {
        self.0.chars().count()
    }

    /// Its length in bytes, the unit `max_password_bytes` counts in.
    pub fn byte_len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(<redacted>)")
    }
}

/// Why a hash could not be made or checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// The stored hash is not one dux will use; the text says why, in words
    /// meant for the person who has to fix `config.toml`.
    InvalidHash(String),
    /// The password bytes are not valid UTF-8.
    NotUtf8,
    /// The hashing library failed (for example the system's random source).
    Hashing(String),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHash(reason) => write!(f, "{reason}"),
            Self::NotUtf8 => f.write_str("the password is not valid UTF-8 text"),
            Self::Hashing(reason) => write!(f, "could not hash the password: {reason}"),
        }
    }
}

impl std::error::Error for AuthError {}

/// Hash `password` into a new Argon2id PHC string with a fresh random salt.
/// Slow on purpose (tens of milliseconds); keep it off UI and async threads.
pub fn hash_password(password: &Password) -> Result<String, AuthError> {
    let params = Params::new(ARGON2_M_COST_KIB, ARGON2_T_COST, ARGON2_P_COST, None)
        .map_err(|e| AuthError::Hashing(e.to_string()))?;
    let hasher = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let hash = hasher
        .hash_password(password.expose().as_bytes())
        .map_err(|e| AuthError::Hashing(e.to_string()))?;
    Ok(hash.to_string())
}

/// Check that `phc` is a hash dux will verify against: an Argon2id PHC string,
/// version 19, with a salt and a digest, and cost parameters inside
/// [`MIN_M_COST_KIB`]..=[`MAX_M_COST_KIB`], at most [`MAX_T_COST`] passes and
/// [`MAX_P_COST`] lanes, and at least [`MIN_WORK_KIB_PASSES`] of total work.
/// Cheap: nothing is hashed.
pub fn validate_password_hash(phc: &str) -> Result<(), AuthError> {
    parse_checked(phc).map(|_| ())
}

fn parse_checked(phc: &str) -> Result<PasswordHash, AuthError> {
    let invalid = |reason: String| AuthError::InvalidHash(reason);
    let parsed = PasswordHash::new(phc).map_err(|e| {
        invalid(format!(
            "password_hash is not a PHC password hash string ({e}); expected one that starts \
             with $argon2id$v=19$"
        ))
    })?;
    if parsed.algorithm.as_str() != "argon2id" {
        return Err(invalid(format!(
            "password_hash uses {}, and dux only accepts argon2id",
            parsed.algorithm.as_str()
        )));
    }
    if parsed.version != Some(0x13) {
        return Err(invalid(
            "password_hash must name Argon2 version 19 (v=19)".to_string(),
        ));
    }
    let params = Params::try_from(&parsed).map_err(|e| {
        invalid(format!(
            "password_hash has Argon2 parameters dux cannot read ({e})"
        ))
    })?;
    let (m, t, p) = (params.m_cost(), params.t_cost(), params.p_cost());
    if !(MIN_M_COST_KIB..=MAX_M_COST_KIB).contains(&m) {
        return Err(invalid(format!(
            "password_hash asks for m={m} KiB of memory per check; dux accepts \
             {MIN_M_COST_KIB} to {MAX_M_COST_KIB}"
        )));
    }
    if t > MAX_T_COST {
        return Err(invalid(format!(
            "password_hash asks for t={t} passes; dux accepts at most {MAX_T_COST}"
        )));
    }
    if p > MAX_P_COST {
        return Err(invalid(format!(
            "password_hash asks for p={p} lanes; dux accepts at most {MAX_P_COST}"
        )));
    }
    if u64::from(m) * u64::from(t) < MIN_WORK_KIB_PASSES {
        return Err(invalid(format!(
            "password_hash is too cheap to guess offline (m={m} KiB with t={t}); use at least \
             m=19456 with t=2, which is what dux config set server.auth.password writes"
        )));
    }
    if parsed.salt.is_none() {
        return Err(invalid("password_hash has no salt".to_string()));
    }
    match &parsed.hash {
        Some(output) if OUTPUT_LEN_RANGE.contains(&output.len()) => {}
        Some(output) => {
            return Err(invalid(format!(
                "password_hash has a {}-byte digest; dux accepts 16 to 64",
                output.len()
            )));
        }
        None => return Err(invalid("password_hash has no digest".to_string())),
    }
    Ok(parsed)
}

/// Whether `password` matches the stored `phc` hash. The hash is validated
/// first ([`validate_password_hash`]), so an absurd one is refused before any
/// memory is allocated for it. `Ok(false)` is a wrong password; `Err` is a
/// hash dux will not use. Slow on purpose; keep it off UI and async threads.
pub fn verify_password(password: &Password, phc: &str) -> Result<bool, AuthError> {
    let parsed = parse_checked(phc)?;
    match Argon2::default().verify_password(password.expose().as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::PasswordInvalid) => Ok(false),
        Err(e) => Err(AuthError::Hashing(e.to_string())),
    }
}

/// The word for a zxcvbn score, the scale the strength meter shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StrengthLabel {
    /// Score 0.
    Weak,
    /// Score 1.
    Fair,
    /// Score 2.
    Good,
    /// Score 3.
    Strong,
    /// Score 4.
    Excellent,
}

impl StrengthLabel {
    /// The label for a score; anything above 4 is [`StrengthLabel::Excellent`].
    pub fn from_score(score: u8) -> Self {
        match score {
            0 => Self::Weak,
            1 => Self::Fair,
            2 => Self::Good,
            3 => Self::Strong,
            _ => Self::Excellent,
        }
    }

    /// The lowercase word shown to the user.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Weak => "weak",
            Self::Fair => "fair",
            Self::Good => "good",
            Self::Strong => "strong",
            Self::Excellent => "excellent",
        }
    }
}

/// How hard a password is to guess, from zxcvbn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Strength {
    /// 0 (guessable in about a thousand tries) to 4 (more than ten billion).
    pub score: u8,
    /// The word for `score`.
    pub label: StrengthLabel,
    /// zxcvbn's own advice (its warning, then its suggestions), or `None`
    /// when it has none, which it usually does not for a strong password.
    pub hint: Option<String>,
}

/// Score `password` with zxcvbn. `user_inputs` are words a guesser would try
/// first for this person (a user name, the machine's name); pass `&[]` when
/// there are none. zxcvbn reads only the first 100 characters.
pub fn strength(password: &Password, user_inputs: &[&str]) -> Strength {
    let entropy = zxcvbn::zxcvbn(password.expose(), user_inputs);
    let score = u8::from(entropy.score());
    let hint = entropy.feedback().and_then(|feedback| {
        let mut parts: Vec<String> = Vec::new();
        if let Some(warning) = feedback.warning() {
            parts.push(warning.to_string());
        }
        parts.extend(feedback.suggestions().iter().map(ToString::to_string));
        (!parts.is_empty()).then(|| parts.join(" "))
    });
    Strength {
        score,
        label: StrengthLabel::from_score(score),
        hint,
    }
}

/// The configured minimums a new password must meet, from `[server.auth]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswordPolicy {
    /// `minimum_password_length`, in characters.
    pub minimum_length: u32,
    /// `minimum_password_score`, 0 to 4.
    pub minimum_score: u8,
    /// `max_password_bytes`: a longer password could never be typed into the
    /// login, which refuses bigger bodies before hashing.
    pub maximum_bytes: u32,
}

/// One way a password falls short of the [`PasswordPolicy`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MinimumFailure {
    /// Fewer characters than `minimum_password_length`.
    TooShort { length: usize, minimum: u32 },
    /// A zxcvbn score below `minimum_password_score`.
    TooWeak { score: u8, minimum: u8 },
    /// More bytes than `max_password_bytes`.
    TooLong { bytes: usize, maximum: u32 },
}

impl fmt::Display for MinimumFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { length, minimum } => write!(
                f,
                "it is {length} characters long and the minimum is {minimum} \
                 (minimum_password_length)"
            ),
            Self::TooWeak { score, minimum } => write!(
                f,
                "it scores {score} ({}) and the minimum is {minimum} ({}) \
                 (minimum_password_score)",
                StrengthLabel::from_score(*score).as_str(),
                StrengthLabel::from_score(*minimum).as_str()
            ),
            Self::TooLong { bytes, maximum } => write!(
                f,
                "it is {bytes} bytes long and the most the login accepts is {maximum} \
                 (max_password_bytes)"
            ),
        }
    }
}

/// The outcome of [`check_minimums`]: the strength (always computed, so a
/// meter can show it) and every minimum the password misses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MinimumCheck {
    pub strength: Strength,
    pub failures: Vec<MinimumFailure>,
}

impl MinimumCheck {
    /// True when every minimum is met.
    pub fn passes(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Compare `password` against `policy`. Every failing minimum is listed, not
/// only the first, so one message can say everything that needs changing.
pub fn check_minimums(
    password: &Password,
    policy: &PasswordPolicy,
    user_inputs: &[&str],
) -> MinimumCheck {
    let strength = strength(password, user_inputs);
    let mut failures = Vec::new();
    let length = password.char_len();
    if length < policy.minimum_length as usize {
        failures.push(MinimumFailure::TooShort {
            length,
            minimum: policy.minimum_length,
        });
    }
    if strength.score < policy.minimum_score {
        failures.push(MinimumFailure::TooWeak {
            score: strength.score,
            minimum: policy.minimum_score,
        });
    }
    let bytes = password.byte_len();
    if bytes > policy.maximum_bytes as usize {
        failures.push(MinimumFailure::TooLong {
            bytes,
            maximum: policy.maximum_bytes,
        });
    }
    MinimumCheck { strength, failures }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pw(text: &str) -> Password {
        Password::new(text.to_string())
    }

    const STRONG: &str = "correct horse battery staple veranda";

    fn policy() -> PasswordPolicy {
        PasswordPolicy {
            minimum_length: 12,
            minimum_score: 2,
            maximum_bytes: 1024,
        }
    }

    /// A PHC string with the given costs, made by the library itself so only
    /// the parameters differ from a real one.
    fn phc_with(m: u32, t: u32, p: u32) -> String {
        let params = Params::new(m, t, p, None).expect("params");
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password(b"irrelevant")
            .expect("hash")
            .to_string()
    }

    #[test]
    fn a_new_hash_is_argon2id_v19_with_owasp_parameters() {
        let hash = hash_password(&pw(STRONG)).expect("hash");
        assert!(
            hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "unexpected PHC prefix: {hash}"
        );
        validate_password_hash(&hash).expect("dux accepts its own hash");
    }

    #[test]
    fn two_hashes_of_one_password_differ_by_salt() {
        assert_ne!(
            hash_password(&pw(STRONG)).unwrap(),
            hash_password(&pw(STRONG)).unwrap()
        );
    }

    #[test]
    fn verify_accepts_the_password_and_rejects_another() {
        let hash = hash_password(&pw(STRONG)).unwrap();
        assert_eq!(verify_password(&pw(STRONG), &hash), Ok(true));
        assert_eq!(
            verify_password(&pw("not the password at all"), &hash),
            Ok(false)
        );
    }

    #[test]
    fn garbage_is_not_a_hash() {
        for bad in [
            "",
            "hunter2",
            "$argon2id$",
            "$argon2id$v=19$m=19456,t=2,p=1$",
        ] {
            assert!(
                matches!(validate_password_hash(bad), Err(AuthError::InvalidHash(_))),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn other_argon2_variants_are_refused() {
        let params = Params::new(19456, 2, 1, None).unwrap();
        let argon2i = Argon2::new(Algorithm::Argon2i, Version::V0x13, params)
            .hash_password(b"x")
            .unwrap()
            .to_string();
        let err = validate_password_hash(&argon2i).unwrap_err();
        assert!(err.to_string().contains("argon2id"), "{err}");
    }

    #[test]
    fn an_old_argon2_version_is_refused() {
        let params = Params::new(19456, 2, 1, None).unwrap();
        let v16 = Argon2::new(Algorithm::Argon2id, Version::V0x10, params)
            .hash_password(b"x")
            .unwrap()
            .to_string();
        assert!(validate_password_hash(&v16).is_err(), "{v16}");
    }

    #[test]
    fn absurd_costs_are_refused_before_any_hashing() {
        // Written by hand: hashing with these would take the memory it asks for.
        let huge_memory = "$argon2id$v=19$m=4194304,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$\
             AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let err = validate_password_hash(huge_memory).unwrap_err();
        assert!(err.to_string().contains("m=4194304"), "{err}");
        let err = verify_password(&pw(STRONG), huge_memory).unwrap_err();
        assert!(matches!(err, AuthError::InvalidHash(_)), "{err:?}");

        let many_passes = "$argon2id$v=19$m=19456,t=100000,p=1$c29tZXNhbHRzb21lc2FsdA$\
             AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        assert!(validate_password_hash(many_passes).is_err());
        let many_lanes = "$argon2id$v=19$m=19456,t=2,p=64$c29tZXNhbHRzb21lc2FsdA$\
             AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        assert!(validate_password_hash(many_lanes).is_err());
    }

    #[test]
    fn a_hash_too_cheap_to_protect_anything_is_refused() {
        let cheap = phc_with(8192, 1, 1);
        let err = validate_password_hash(&cheap).unwrap_err();
        assert!(err.to_string().contains("too cheap"), "{err}");
        // The OWASP alternatives all pass.
        for (m, t) in [(47104, 1), (19456, 2), (12288, 3), (9216, 4), (7168, 5)] {
            validate_password_hash(&phc_with(m, t, 1))
                .unwrap_or_else(|e| panic!("m={m} t={t} must be accepted: {e}"));
        }
    }

    #[test]
    fn strength_labels_follow_the_score() {
        let labels: Vec<&str> = (0..=4)
            .map(|s| StrengthLabel::from_score(s).as_str())
            .collect();
        assert_eq!(labels, ["weak", "fair", "good", "strong", "excellent"]);
    }

    #[test]
    fn a_common_password_is_weak_with_a_hint_and_a_passphrase_is_excellent() {
        let weak = strength(&pw("password"), &[]);
        assert_eq!(weak.score, 0);
        assert_eq!(weak.label, StrengthLabel::Weak);
        assert!(weak.hint.is_some(), "zxcvbn explains a weak password");

        let leet = strength(&pw("P@ssw0rd!"), &[]);
        assert!(leet.score <= 1, "l33t substitutions fool nobody: {leet:?}");

        let phrase = strength(&pw(STRONG), &[]);
        assert_eq!(phrase.label, StrengthLabel::Excellent, "{phrase:?}");
    }

    #[test]
    fn user_inputs_make_a_password_built_from_them_weaker() {
        let alone = strength(&pw("patrickdux2026"), &[]);
        let with_inputs = strength(&pw("patrickdux2026"), &["patrick", "dux"]);
        assert!(with_inputs.score <= alone.score);
    }

    #[test]
    fn minimums_list_every_failure() {
        let check = check_minimums(&pw("password"), &policy(), &[]);
        assert!(!check.passes());
        assert_eq!(
            check.failures,
            vec![
                MinimumFailure::TooShort {
                    length: 8,
                    minimum: 12
                },
                MinimumFailure::TooWeak {
                    score: 0,
                    minimum: 2
                },
            ]
        );
        assert!(
            check.failures[0]
                .to_string()
                .contains("minimum_password_length")
        );
        assert!(check.failures[1].to_string().contains("weak"));
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        // Twelve characters, thirty-six bytes.
        let p = pw("日本語のパスワードです。");
        assert_eq!(p.char_len(), 12);
        let check = check_minimums(
            &p,
            &PasswordPolicy {
                minimum_score: 0,
                ..policy()
            },
            &[],
        );
        assert!(
            !check
                .failures
                .iter()
                .any(|f| matches!(f, MinimumFailure::TooShort { .. })),
            "{check:?}"
        );
    }

    #[test]
    fn a_password_longer_than_the_login_accepts_is_refused() {
        let long = "a long passphrase that keeps going ".repeat(40);
        let check = check_minimums(&pw(&long), &policy(), &[]);
        assert!(
            check
                .failures
                .iter()
                .any(|f| matches!(f, MinimumFailure::TooLong { .. }))
        );
    }

    #[test]
    fn a_strong_password_passes() {
        let check = check_minimums(&pw(STRONG), &policy(), &[]);
        assert!(check.passes(), "{check:?}");
    }

    #[test]
    fn debug_never_prints_the_plaintext() {
        let shown = format!("{:?}", pw("super secret words"));
        assert!(!shown.contains("secret"), "{shown}");
    }

    #[test]
    fn non_utf8_bytes_are_refused() {
        assert_eq!(
            Password::from_utf8(vec![0xff, 0xfe]).unwrap_err(),
            AuthError::NotUtf8
        );
        assert_eq!(
            Password::from_utf8(b"fine".to_vec()).unwrap().expose(),
            "fine"
        );
    }
}

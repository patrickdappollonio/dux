//! What a PERSON typed into the pull-request field.
//!
//! This is deliberately NOT [`crate::git::parse_remote_address`], and the two
//! must never be merged. They answer different questions:
//!
//! * A project's configured address is whatever git already put in the
//!   repository on disk. Nobody types it at dux. So the only sensible rule is
//!   git's own rule, and a trailing path segment there is part of the address
//!   git would really fetch from. Ignoring it would name a DIFFERENT
//!   repository than the one checked out.
//! * This field is the opposite. A person pastes into it, from a browser bar,
//!   from a chat message, from memory. A trailing `/issues` or
//!   `/security/dependabot` is a browser route and ignoring it is helpful,
//!   because every one of those addresses is a way of writing down the same
//!   repository.
//!
//! The accepted shapes follow `gc-rust`, the maintainer's own clone helper:
//!
//! ```text
//! example/application
//! github.com/example/application
//! git@github.com:example/application.git
//! https://github.com/example/application
//! https://github.com/example/application/issues
//! https://github.com/example/application/security/dependabot
//! https://github.com/example/application/this/is/a/made/up/path
//! ```
//!
//! all naming `example/application`, plus the pull-request spellings: a full
//! PR URL, `owner/repo#123`, `#123` and a bare `123`.
//!
//! Nothing here consults [`crate::gh::GithubHostPolicy`]. Parsing is about what
//! the text SAYS; whether dux may ask `gh` about the host it says is a separate
//! question, asked by the callers that have a host to ask about.

/// A repository, a pull request number, or both, as named by typed text.
///
/// Every field is optional because the accepted spellings genuinely differ in
/// what they pin down: `#123` names a number and no repository, `example/app`
/// names a repository and no number, and `example/app#123` names both but no
/// host. A value with neither a repository nor a number is never produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedReference {
    /// Lowercased host, when the text named one. `None` for `owner/repo`,
    /// `owner/repo#123` and a bare number, which name no host at all. A caller
    /// must NOT substitute `github.com` for `None`: someone whose only checkout
    /// of `acme/widget` lives on their company server would be sent to the
    /// wrong place, silently.
    pub host: Option<String>,
    /// `owner/repo` with any `.git` suffix removed, in the case it was typed
    /// (compare it case-insensitively; it is kept as written so a message can
    /// echo the user's own spelling back).
    pub owner_repo: Option<String>,
    /// The pull-request number, when the text carried one.
    pub number: Option<u64>,
}

impl TypedReference {
    /// How to name this reference's repository in a message, `host/owner/repo`
    /// when a host was given and `owner/repo` when it was not. `None` when the
    /// text named no repository (a bare number).
    pub fn repository_label(&self) -> Option<String> {
        let owner_repo = self.owner_repo.as_deref()?;
        Some(match self.host.as_deref() {
            Some(host) => format!("{host}/{owner_repo}"),
            None => owner_repo.to_string(),
        })
    }

    /// Whether this reference names the repository at `host` / `owner_repo`,
    /// which is a project's configured address as
    /// [`crate::git::remote_github_repo`] reports it.
    ///
    /// The host is compared ONLY when the reference gave one. `owner/repo#123`
    /// names no host, so it matches that repository on whatever host a project
    /// keeps it on. Both sides are compared case-insensitively with any `.git`
    /// suffix removed, because a host is case-insensitive and GitHub treats
    /// `Example/Application` and `example/application` as one repository.
    pub fn matches(&self, host: &str, owner_repo: &str) -> bool {
        let Some(mine) = self.owner_repo.as_deref() else {
            return false;
        };
        if !strip_dot_git(mine).eq_ignore_ascii_case(strip_dot_git(owner_repo)) {
            return false;
        }
        match self.host.as_deref() {
            Some(theirs) => theirs.eq_ignore_ascii_case(host),
            None => true,
        }
    }
}

/// The one message every unparseable spelling gets. It names the shapes rather
/// than describing the failure, because the failure is nearly always "that is
/// not one of these".
const UNPARSEABLE: &str =
    "Enter a pull request URL, owner/repo#123, or a PR number. A repository address works too.";

/// Parse typed text into the repository and/or pull request it names.
pub fn parse_typed_reference(raw: &str) -> Result<TypedReference, String> {
    let input = raw.trim();
    if input.is_empty() {
        return Err("Enter a pull request URL, owner/repo#123, or a PR number.".to_string());
    }
    // A control character cannot be part of a host or a repository name, and
    // letting one through is how a value that names nothing acquires the shape
    // of something. Refused before anything is split.
    if input.chars().any(char::is_control) {
        return Err(UNPARSEABLE.to_string());
    }

    // A number on its own, with or without the `#`. It names no repository,
    // which is a fact the caller has to deal with rather than an error here:
    // with a project already chosen it is exactly what the user meant.
    let digits = input.strip_prefix('#').unwrap_or(input);
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        return Ok(TypedReference {
            host: None,
            owner_repo: None,
            number: Some(parse_number(digits)?),
        });
    }

    let Parts {
        host,
        segments,
        fragment,
    } = match classify(input)? {
        Form::Url(kind) => parse_url_form(input, kind)?,
        Form::Schemeless => parse_schemeless_form(input)?,
    };

    if segments.len() < 2 {
        return Err(UNPARSEABLE.to_string());
    }
    // A `.` or a `..` anywhere in the path is refused outright. In the URL
    // forms none can survive, because the `url` crate has already resolved them
    // the way a browser would. The forms dux parses by hand (scp-like, and a
    // bare `owner/repo`) are NOT urls and nothing resolves them, so a dot
    // segment there would either become a repository NAME (`acme/..`) or sit in
    // a discarded trailing route while a server would have resolved it into a
    // different repository (`acme/widget/../gadget`). Both name something other
    // than what the text says, which is the one thing this parser must never do.
    if segments.iter().any(|part| part == "." || part == "..") {
        return Err(UNPARSEABLE.to_string());
    }
    let owner = segments[0].as_str();
    let repo = strip_dot_git(segments[1].as_str());
    if !is_repository_component(owner) || !is_repository_component(repo) {
        return Err(UNPARSEABLE.to_string());
    }

    // Everything after `owner/repo` is a browser route and is discarded, with
    // one exception: `pull/<n>` is the route that carries the number, and
    // `/files`, `/commits/<sha>` and the rest hang off it harmlessly.
    let mut number = None;
    if segments.len() >= 4
        && (segments[2] == "pull" || segments[2] == "pulls")
        && let Ok(parsed) = segments[3].parse::<u64>()
    {
        number = Some(parsed);
    }
    // A fragment of nothing but digits is the `owner/repo#123` spelling, and
    // that spelling is the WHOLE path: `owner/repo/issues#123` is a browser
    // route with an anchor on it, so reading its `123` as a pull request number
    // would invent a pull request nobody named. A number already read from the
    // path wins over a fragment either way.
    if number.is_none()
        && segments.len() == 2
        && let Some(fragment) = fragment.as_deref()
        && !fragment.is_empty()
        && fragment.chars().all(|c| c.is_ascii_digit())
    {
        number = Some(parse_number(fragment)?);
    }

    Ok(TypedReference {
        host,
        owner_repo: Some(format!("{owner}/{repo}")),
        number,
    })
}

fn parse_number(digits: &str) -> Result<u64, String> {
    digits
        .parse::<u64>()
        .map_err(|_| format!("\"{digits}\" is not a pull request number dux can use."))
}

/// What every accepted spelling boils down to: a host when one was named, the
/// path as a list of decoded, non-empty components, and the fragment.
struct Parts {
    host: Option<String>,
    segments: Vec<String>,
    fragment: Option<String>,
}

/// Which family of address the text was written in. Only the family matters
/// here: it decides where the host ends and whether a port belongs to a
/// transport rather than to the server dux would query.
#[derive(Clone, Copy)]
enum SchemeKind {
    Web,
    Ssh,
    Git,
}

/// Whether the text is a URL dux should hand to the `url` crate, or one of the
/// two shapes that are not urls at all and are parsed by hand.
enum Form {
    Url(SchemeKind),
    Schemeless,
}

/// Decide which of the two, refusing a scheme dux does not speak.
///
/// The refusal matters. A scheme that fell through to the schemeless rule met
/// git's scp-like shorthand, whose colon comes before its slash, so
/// `ftp://github.com/acme/widget` answered with the host `ftp` and the
/// repository `github.com/acme`: a repository nobody named, on a host nobody
/// named. Text carrying a scheme is a claim about a transport, and a claim dux
/// cannot honour is refused rather than reinterpreted.
fn classify(input: &str) -> Result<Form, String> {
    let Some((scheme, _)) = input.split_once("://") else {
        return Ok(Form::Schemeless);
    };
    // Not every colon-slash-slash is a scheme. If what precedes it is not a
    // scheme token, the text is something else entirely and the schemeless
    // rules get their turn.
    if !is_scheme_token(scheme) {
        return Ok(Form::Schemeless);
    }
    // Case-insensitively: a person may well type `HTTPS://`. Unlike a
    // configured address, which git matches case-sensitively against its own
    // table, this text is never handed to git.
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => Ok(Form::Url(SchemeKind::Web)),
        "ssh" | "git+ssh" | "ssh+git" => Ok(Form::Url(SchemeKind::Ssh)),
        "git" => Ok(Form::Url(SchemeKind::Git)),
        _ => Err(UNPARSEABLE.to_string()),
    }
}

/// A URL scheme as the spec spells one: a letter, then letters, digits, `+`,
/// `-` and `.`.
fn is_scheme_token(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// A scheme-qualified address, parsed by BROWSER rules rather than by splitting
/// the text by hand.
///
/// This is the whole point of the `url` crate being here. Hand-splitting reads
/// `https://github.com/acme/widget/../gadget` as `acme/widget`, and a browser
/// reads it as `acme/gadget`: two different repositories, one of which the user
/// very likely has a project for. Taking the crate's normalised host and its
/// path segments means the repository dux names is the repository the address
/// names, dot segments and percent escapes and all.
fn parse_url_form(input: &str, kind: SchemeKind) -> Result<Parts, String> {
    refuse_authority_git_reads_differently(input, kind)?;
    // A malformed authority (`https://[::1/...`, a space in the host, an empty
    // host) fails here, where it used to be read as a host called `[::1`.
    let url = url::Url::parse(input).map_err(|_| UNPARSEABLE.to_string())?;
    let Some(host) = url.host_str() else {
        return Err(UNPARSEABLE.to_string());
    };
    if host.is_empty() {
        return Err(UNPARSEABLE.to_string());
    }
    // Credentials never survive: `host_str` is the host alone, so a pasted
    // token cannot reach a log line or a match. The crate lowercases a special
    // scheme's host for us and leaves an opaque one alone, so lowercase again
    // rather than depending on which kind this is.
    let mut host = host.to_ascii_lowercase();
    match kind {
        // A web port is KEPT, because dropping it would match a project on the
        // default port, which is a different server, and answering about a
        // different server is worse than answering about none. `port()` is
        // already `None` for the scheme's default port, so `https://host:443`
        // and `https://host` name the same server, as they should.
        SchemeKind::Web => {
            if let Some(port) = url.port() {
                host.push(':');
                host.push_str(&port.to_string());
            }
        }
        // An ssh port is the transport's and says nothing about the server's
        // API, so it comes off.
        SchemeKind::Ssh => {
            // `gh` has never heard of `ssh.github.com`, GitHub's port-443 ssh
            // endpoint, and neither has a project's configured address once dux
            // has read it, so the same name is used here.
            if host == "ssh.github.com" {
                host = "github.com".to_string();
            }
        }
        SchemeKind::Git => {}
    }

    let mut segments = Vec::new();
    if let Some(raw) = url.path_segments() {
        for segment in raw {
            if segment.is_empty() {
                // A doubled or trailing slash is simply absent rather than an
                // empty component. Generous on purpose: an ordinary typing slip
                // that changes no repository.
                continue;
            }
            segments.push(decode_component(segment)?);
        }
    }
    let fragment = url.fragment().map(decode_component).transpose()?;
    Ok(Parts {
        host: Some(host),
        segments,
        fragment,
    })
}

/// Refuse the two authorities where git's own rule differs from the browser
/// rule this parser otherwise follows, rather than answering for a host the
/// address does not name.
///
/// The typed field is lenient because a person is typing a BROWSER address into
/// it, and a browser is the right authority on `https://` and on the shapes it
/// normalises. It is not the right authority on `ssh://` or on git's native
/// protocol, which no browser opens and which git parses by its own rule. Where
/// the two rules disagree, dux cannot faithfully reproduce git's, so it refuses.
/// That is the same decision, for the same reason, that
/// [`crate::git::parse_remote_address`] already documents for a configured
/// address, and both refusals are measured there.
///
/// The two disagreements:
///
/// * A PERCENT in an ssh-family or native-git authority. Git decodes the whole
///   address and separates host from path AFTERWARDS, in that order, so
///   `ssh://user%2Fx@github.com/acme/widget` reaches ssh as the host `user`
///   with the path `/x@github.com/acme/widget`, and `git://us%2Fer@host/o/r`
///   makes git look up the host `us`. The generic URL grammar splits first and
///   reports the written host, so it answers for a different server. Under
///   http(s) the same shape moves no boundary (curl splits the authority off
///   first and decodes each piece after), so a percent is left alone there:
///   refusing it would refuse an ordinary address whose password holds an
///   escape.
/// * An `@` in a NATIVE git authority. Git's own protocol has no user
///   component, unlike its ssh URL syntax, so `git://user@github.com/o/r` sends
///   git looking up `user@github.com` on port 9418. The URL grammar discards
///   the user and answers for `github.com`, a host the address never names.
///   Under ssh a user is legitimate, and under http(s) it is credentials that
///   are correctly dropped as credentials.
fn refuse_authority_git_reads_differently(input: &str, kind: SchemeKind) -> Result<(), String> {
    let percent_moves_the_boundary = match kind {
        SchemeKind::Ssh | SchemeKind::Git => true,
        SchemeKind::Web => return Ok(()),
    };
    // The authority has to be read from the ORIGINAL text: the crate has
    // already split and decoded it, which is exactly the split being questioned.
    // Both of these schemes are non-special, so an authority exists only after a
    // literal `://`; without one the crate reports no host at all and the
    // address is refused a moment later regardless.
    let Some(authority) = raw_url_authority(input) else {
        return Ok(());
    };
    if percent_moves_the_boundary && authority.contains('%') {
        return Err(UNPARSEABLE.to_string());
    }
    if matches!(kind, SchemeKind::Git) && authority.contains('@') {
        return Err(UNPARSEABLE.to_string());
    }
    Ok(())
}

/// The authority of a scheme-qualified address, sliced out of the ORIGINAL
/// input so no normalisation can reach it. Userinfo and an IPv6 literal cannot
/// hold an unescaped `/`, `?` or `#`, so the first of those after the `://`
/// ends the authority.
fn raw_url_authority(input: &str) -> Option<&str> {
    let after_scheme = input.split_once("://")?.1;
    Some(match after_scheme.find(['/', '?', '#']) {
        Some(end) => &after_scheme[..end],
        None => after_scheme,
    })
}

/// Percent-decode one path or fragment component, as a browser does. Invalid
/// UTF-8 is refused rather than lossily substituted: `U+FFFD` is neither a
/// control character nor whitespace, so a replacement character would survive
/// every later check and travel into a name nobody wrote.
fn decode_component(raw: &str) -> Result<String, String> {
    percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map(|decoded| decoded.into_owned())
        .map_err(|_| UNPARSEABLE.to_string())
}

/// The two shapes that are NOT urls: git's scp-like shorthand, and the bare
/// `owner/repo` / `host/owner/repo` a person writes from memory.
///
/// Nothing here is percent-decoded, deliberately. A percent in an scp path is a
/// literal character to git, and `owner/repo` is not an address at all, so
/// decoding would invent an escape the user did not write. Decoding belongs to
/// the URL form, where a browser really would decode.
fn parse_schemeless_form(input: &str) -> Result<Parts, String> {
    // The fragment comes off before the query, because a fragment may itself
    // contain a `?` and that `?` is fragment text rather than a query.
    let (body, fragment) = match input.split_once('#') {
        Some((before, after)) => (before, Some(after.to_string())),
        None => (input, None),
    };
    let body = body.split('?').next().unwrap_or(body);

    // `[user@]host:path`, git's scp-like shorthand, is ssh by definition. It is
    // recognised only when the colon comes before any slash, exactly as git
    // documents, so `example/application:tags` is not mistaken for an address.
    let (host, path) = match split_scp_like(body) {
        Some((authority, path)) => (Some(host_from_authority(authority, true)?), path),
        None => {
            // A schemeless value is `owner/repo[/...]` or
            // `host/owner/repo[/...]`, and the shape of the leading segment is
            // what tells them apart. A dot (or a port, or `localhost`) makes it
            // a host, because an owner cannot hold one: GitHub account and
            // organisation names are letters, digits and hyphens. So
            // `github.com/example` is a host with its repository missing and is
            // refused, rather than read as a repository named `example` owned
            // by `github.com`.
            let segments = path_segments(body);
            if looks_like_host(segments.first().copied().unwrap_or_default()) {
                let host = host_from_authority(segments[0], false)?;
                let rest = body.split_once('/').map(|(_, rest)| rest).unwrap_or("");
                (Some(host), rest)
            } else {
                (None, body)
            }
        }
    };

    Ok(Parts {
        host,
        segments: path_segments(path)
            .into_iter()
            .map(str::to_string)
            .collect(),
        fragment,
    })
}

/// Git's scp-like shorthand, `[user@]host:path`, recognised only when the colon
/// precedes any slash.
///
/// With ONE exception, which is a decision rather than an accident.
/// `github.com:8443/acme/widget` is not scp syntax: git's scp shorthand has no
/// notion of a port, so an all-digit run between the colon and the first slash
/// cannot be part of a path a person meant. It is a browser address with the
/// scheme rubbed off, which is exactly what dropping `https://` from an address
/// bar produces, and it is read as `host:port` + path by the schemeless branch,
/// keeping the port on the host as every other web address does. Read as scp it
/// answered with the host `github.com` and the repository `8443/acme`. A
/// `user@` prefix settles the ambiguity the other way, because that is
/// unambiguously scp and no browser address carries one before a port.
fn split_scp_like(input: &str) -> Option<(&str, &str)> {
    let colon = input.find(':')?;
    if let Some(slash) = input.find('/')
        && slash < colon
    {
        return None;
    }
    let (authority, path) = input.split_at(colon);
    let path = &path[1..];
    if !authority.contains('@') {
        let head = path.split('/').next().unwrap_or(path);
        if !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    Some((authority, path))
}

/// The host a message and a match should use, from a schemeless authority as
/// written.
///
/// Credentials come off, because a pasted address can carry a token and that
/// token must not reach a log line or a match. A port comes off only for the
/// ssh family, where it is the transport's port and says nothing about the
/// server's API; a web port is KEPT, because dropping it would match a project
/// on the default port, which is a different server, and answering about a
/// different server is worse than answering about none.
fn host_from_authority(authority: &str, ssh_like: bool) -> Result<String, String> {
    let host = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    let host = if ssh_like {
        host.split(':').next().unwrap_or(host)
    } else {
        host
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty() || host.chars().any(|c| c.is_whitespace()) {
        return Err(UNPARSEABLE.to_string());
    }
    // `gh` has never heard of `ssh.github.com`, GitHub's port-443 ssh endpoint,
    // and neither has a project's configured address once dux has read it, so
    // the same name is used here.
    if ssh_like && host == "ssh.github.com" {
        return Ok("github.com".to_string());
    }
    Ok(host)
}

/// Whether a leading schemeless segment reads as a host rather than as an
/// owner.
fn looks_like_host(segment: &str) -> bool {
    segment.eq_ignore_ascii_case("localhost") || segment.contains('.') || segment.contains(':')
}

/// Split a path into its non-empty segments, so a doubled or trailing slash is
/// simply absent rather than an empty component. Generous on purpose: all three
/// are ordinary typing slips and none of them changes which repository is
/// named.
fn path_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|part| !part.is_empty()).collect()
}

/// An owner or a repository name, as far as this parser needs to judge it. It
/// is not GitHub's own rule (which is narrower) because the answer is only ever
/// used to look for a project that already has this address; the check exists
/// to refuse text that plainly is not a name.
///
/// A separator is refused as well as whitespace, because a component is judged
/// AFTER percent-decoding: `acme%2Fwidget` decodes to `acme/widget`, and
/// letting one component hold a separator lets it pose as two.
fn is_repository_component(part: &str) -> bool {
    !part.is_empty()
        && !part
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\' || c == '/')
        && part != "."
        && part != ".."
}
fn strip_dot_git(value: &str) -> &str {
    let len = value.len();
    // The boundary check is not decoration. A repository name can hold
    // multi-byte characters, and slicing four bytes off the end of one panics
    // before the comparison it was going to feed.
    if len > 4 && value.is_char_boundary(len - 4) && value[len - 4..].eq_ignore_ascii_case(".git") {
        &value[..len - 4]
    } else {
        value
    }
}

/// Why a project could not be compared against the reference at all.
///
/// These are NOT non-matches. A project dux could not inspect might well be a
/// checkout of the repository; dux simply cannot say. Collapsing the two is how
/// a message ends up asserting "no project in dux is a checkout of that
/// repository" when the truth is "the only project that might have been was
/// unreadable".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Uninspectable {
    /// The project's directory is gone, so there is nothing to ask git about.
    PathMissing,
    /// git could not read an `origin`, or what it read is not an address dux
    /// can parse into a host and an `owner/repo`.
    AddressUnreadable,
    /// A readable address on a host `gh` is not signed in to. dux knows where
    /// this project pushes and knows it may not ask about it.
    HostNotAllowed,
}

impl Uninspectable {
    /// How to describe one of these in a sentence that already begins with a
    /// count, e.g. "2 have an address dux could not read".
    fn phrase(&self, count: usize) -> String {
        let verb = if count == 1 { "has" } else { "have" };
        match self {
            Self::PathMissing => format!("{count} {verb} a directory that is missing"),
            Self::AddressUnreadable => format!("{count} {verb} an address dux could not read"),
            Self::HostNotAllowed => {
                let verb = if count == 1 { "is" } else { "are" };
                format!("{count} {verb} on a host dux may not ask about")
            }
        }
    }
}

/// One project dux could not inspect, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UninspectedProject {
    pub name: String,
    pub reason: Uninspectable,
}

/// What asking every project produced: the ones that ARE checkouts of the
/// repository, and the ones dux could not ask about.
///
/// The second half is the honesty. "No match" and "no match, and I could not
/// look at four of them" are different answers, and only the first one licenses
/// a surface to say there is no checkout.
#[derive(Clone, Debug, Default)]
pub struct ReferenceResolution {
    pub matches: Vec<crate::model::Project>,
    pub uninspected: Vec<UninspectedProject>,
}

impl ReferenceResolution {
    /// Whether the answer is complete: every project was inspected, so "none of
    /// them" really means none of them.
    pub fn is_complete(&self) -> bool {
        self.uninspected.is_empty()
    }

    /// A clause naming what dux could not check, for a message that must not
    /// claim more than it knows. `None` when everything was inspected.
    ///
    /// Reasons are grouped rather than listed per project, because a workspace
    /// with thirty projects would otherwise produce a paragraph, and the thing
    /// the user needs to know is that the answer is incomplete and why.
    pub fn uninspected_summary(&self) -> Option<String> {
        if self.uninspected.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        for reason in [
            Uninspectable::AddressUnreadable,
            Uninspectable::HostNotAllowed,
            Uninspectable::PathMissing,
        ] {
            let count = self
                .uninspected
                .iter()
                .filter(|entry| entry.reason == reason)
                .count();
            if count > 0 {
                parts.push(reason.phrase(count));
            }
        }
        Some(parts.join(", "))
    }
}

/// Which of `projects` are checkouts of the repository `reference` names, and
/// which of them dux could not ask.
///
/// This is ONE `git` call per project, on an explicit user action, so it
/// belongs on a worker like every other git call and never on the interface
/// thread.
///
/// **There is no cache, deliberately.** The answer changes when an address is
/// edited, when git's rewrite configuration changes, when a project's path
/// moves under the same id, and when an unreadable address is repaired. None of
/// those are things dux watches, so a cached answer would go wrong quietly. It
/// is recomputed per operation, over the live project list.
///
/// Matching uses git's EFFECTIVE address, the rewritten one, which is what
/// [`crate::git::resolve_remote_github_repo`] already returns. That is the
/// correct anchor: it is the address git would really contact.
///
/// A project whose path is missing, whose address git cannot read, or whose
/// host the policy does not allow is reported as UNINSPECTED rather than
/// silently dropped. Each of those means "dux cannot tell", which is a
/// different answer from "this is not a checkout of that repository", and a
/// surface that cannot tell them apart states a certainty it does not have.
pub fn resolve_reference_projects(
    reference: &TypedReference,
    projects: &[crate::model::Project],
    policy: &crate::gh::GithubHostPolicy,
) -> ReferenceResolution {
    let mut resolution = ReferenceResolution::default();
    if reference.owner_repo.is_none() {
        // A bare number names no repository, so there is nothing to resolve. The
        // caller refuses it with an explanation rather than searching for a
        // repository nobody named, and reporting every project as uninspected
        // here would turn that refusal into a scare.
        return resolution;
    }
    for project in projects {
        let mut uninspected = |reason: Uninspectable| {
            resolution.uninspected.push(UninspectedProject {
                name: project.name.clone(),
                reason,
            });
        };
        if project.path_missing {
            uninspected(Uninspectable::PathMissing);
            continue;
        }
        match crate::git::resolve_remote_github_repo(std::path::Path::new(&project.path), policy) {
            crate::git::RemoteResolution::Allowed(remote) => {
                if reference.matches(&remote.host, &remote.owner_repo) {
                    resolution.matches.push(project.clone());
                }
            }
            crate::git::RemoteResolution::Unresolved => {
                uninspected(Uninspectable::AddressUnreadable)
            }
            crate::git::RemoteResolution::Denied => uninspected(Uninspectable::HostNotAllowed),
        }
    }
    resolution
}

/// [`resolve_reference_projects`] on a worker thread, posting
/// [`crate::worker::WorkerEvent::PullRequestReferenceResolved`] with the answer.
///
/// Parsing is NOT done here: it is pure and instant, so the surface does it
/// inline and can refuse a bare number (or unreadable text) without a round
/// trip. Only a reference that actually names a repository is worth a git call
/// per project, which is what this thread is for.
pub fn run_reference_resolution_job(
    reference: TypedReference,
    raw_input: String,
    projects: Vec<crate::model::Project>,
    policy: crate::gh::GithubHostPolicy,
    worker_tx: std::sync::mpsc::Sender<crate::worker::WorkerEvent>,
    status_op_id: Option<String>,
) {
    let repository = reference.repository_label().unwrap_or_default();
    let resolution = resolve_reference_projects(&reference, &projects, &policy);
    let _ = worker_tx.send(crate::worker::WorkerEvent::PullRequestReferenceResolved {
        raw_input,
        repository,
        result: Ok(resolution),
        status_op_id,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(input: &str) -> TypedReference {
        parse_typed_reference(input).unwrap_or_else(|err| panic!("{input:?} should parse: {err}"))
    }

    fn repo_of(input: &str) -> (Option<String>, Option<String>) {
        let reference = parsed(input);
        (reference.host, reference.owner_repo)
    }

    // --- the shapes gc-rust settles ------------------------------------------

    #[test]
    fn bare_owner_repo_names_a_repository_and_no_host() {
        assert_eq!(
            repo_of("example/application"),
            (None, Some("example/application".to_string()))
        );
    }

    #[test]
    fn host_qualified_owner_repo_names_the_host() {
        assert_eq!(
            repo_of("github.com/example/application"),
            (
                Some("github.com".to_string()),
                Some("example/application".to_string())
            )
        );
    }

    #[test]
    fn scp_like_address_with_dot_git_names_the_repository() {
        assert_eq!(
            repo_of("git@github.com:example/application.git"),
            (
                Some("github.com".to_string()),
                Some("example/application".to_string())
            )
        );
    }

    #[test]
    fn web_address_names_the_repository() {
        assert_eq!(
            repo_of("https://github.com/example/application"),
            (
                Some("github.com".to_string()),
                Some("example/application".to_string())
            )
        );
    }

    #[test]
    fn a_trailing_browser_route_is_ignored() {
        for input in [
            "https://github.com/example/application/issues",
            "https://github.com/example/application/security/dependabot",
            "https://github.com/example/application/this/is/a/made/up/path",
        ] {
            assert_eq!(
                repo_of(input),
                (
                    Some("github.com".to_string()),
                    Some("example/application".to_string())
                ),
                "{input}"
            );
            assert_eq!(parsed(input).number, None, "{input}");
        }
    }

    // --- the pull-request spellings ------------------------------------------

    #[test]
    fn a_full_pull_request_url_carries_its_number() {
        let reference = parsed("https://github.com/example/application/pull/123");
        assert_eq!(reference.host.as_deref(), Some("github.com"));
        assert_eq!(reference.owner_repo.as_deref(), Some("example/application"));
        assert_eq!(reference.number, Some(123));
    }

    #[test]
    fn a_pull_request_url_keeps_its_number_under_a_trailing_route() {
        for input in [
            "https://github.com/example/application/pull/123/files",
            "https://github.com/example/application/pull/123/commits/abc123",
            "https://github.com/example/application/pull/123#discussion_r1",
            "https://github.com/example/application/pull/123?w=1",
        ] {
            assert_eq!(parsed(input).number, Some(123), "{input}");
            assert_eq!(
                parsed(input).owner_repo.as_deref(),
                Some("example/application"),
                "{input}"
            );
        }
    }

    #[test]
    fn owner_repo_hash_number_names_no_host() {
        let reference = parsed("example/application#123");
        assert_eq!(
            reference.host, None,
            "guessing github.com here would send someone whose only checkout \
             is on a company server to the wrong place"
        );
        assert_eq!(reference.owner_repo.as_deref(), Some("example/application"));
        assert_eq!(reference.number, Some(123));
    }

    #[test]
    fn a_bare_number_names_only_a_number() {
        for input in ["123", "#123"] {
            let reference = parsed(input);
            assert_eq!(reference.host, None, "{input}");
            assert_eq!(reference.owner_repo, None, "{input}");
            assert_eq!(reference.number, Some(123), "{input}");
        }
    }

    // --- generosity that must not become wrongness ----------------------------

    #[test]
    fn credentials_never_survive_into_the_host() {
        let reference = parsed("https://ghp_secret@github.com/example/application/pull/7");
        assert_eq!(reference.host.as_deref(), Some("github.com"));
        assert_eq!(reference.number, Some(7));
    }

    #[test]
    fn an_ssh_port_is_not_the_hosts_api() {
        assert_eq!(
            repo_of("ssh://git@git.company.example:2222/team/service.git"),
            (
                Some("git.company.example".to_string()),
                Some("team/service".to_string())
            )
        );
    }

    #[test]
    fn a_web_port_stays_on_the_host_so_it_matches_no_other_server() {
        let reference = parsed("https://git.company.example:8443/team/service");
        assert_eq!(reference.host.as_deref(), Some("git.company.example:8443"));
        assert!(
            !reference.matches("git.company.example", "team/service"),
            "a port names a different server and must not match the port-less one"
        );
    }

    #[test]
    fn githubs_alternate_ssh_host_is_reported_as_github_com() {
        assert_eq!(
            repo_of("ssh://git@ssh.github.com:443/example/application.git"),
            (
                Some("github.com".to_string()),
                Some("example/application".to_string())
            )
        );
    }

    #[test]
    fn the_host_is_lowercased_and_the_repository_is_not() {
        let reference = parsed("https://GitHub.COM/Example/Application");
        assert_eq!(reference.host.as_deref(), Some("github.com"));
        assert_eq!(reference.owner_repo.as_deref(), Some("Example/Application"));
    }

    #[test]
    fn a_dotted_leading_segment_is_a_host_and_never_an_owner() {
        // An owner cannot contain a dot, so `my.org/application` is a host with
        // its repository missing. Reading it as the repository `application`
        // owned by `my.org` would invent a repository nobody named.
        assert!(parse_typed_reference("my.org/application").is_err());
        assert_eq!(
            repo_of("my.org/team/application"),
            (
                Some("my.org".to_string()),
                Some("team/application".to_string())
            )
        );
    }

    #[test]
    fn slashes_a_person_slips_in_do_not_change_the_repository() {
        for input in [
            "https://github.com/example/application/",
            "https://github.com//example//application",
            "github.com/example/application/",
        ] {
            assert_eq!(
                repo_of(input),
                (
                    Some("github.com".to_string()),
                    Some("example/application".to_string())
                ),
                "{input}"
            );
        }
    }

    // --- refusals -------------------------------------------------------------

    #[test]
    fn empty_input_asks_for_a_reference() {
        let err = parse_typed_reference("   ").unwrap_err();
        assert!(err.contains("Enter a pull request URL"), "{err}");
    }

    #[test]
    fn a_single_word_names_nothing() {
        for input in ["application", "https://github.com", "github.com/example"] {
            assert!(
                parse_typed_reference(input).is_err(),
                "{input} should be refused"
            );
        }
    }

    #[test]
    fn a_control_character_is_refused_rather_than_deleted() {
        assert!(parse_typed_reference("https://git\nhub.com/example/application").is_err());
        assert!(parse_typed_reference("example/appli\tcation#1").is_err());
    }

    #[test]
    fn whitespace_inside_a_name_is_refused() {
        assert!(parse_typed_reference("exa mple/application").is_err());
        assert!(parse_typed_reference("example/appli cation").is_err());
    }

    // --- matching -------------------------------------------------------------

    #[test]
    fn matching_ignores_case_and_a_dot_git_suffix() {
        let reference = parsed("https://GITHUB.com/Example/Application/pull/9");
        assert!(reference.matches("github.com", "example/application"));
        assert!(reference.matches("GitHub.com", "example/application.git"));
    }

    #[test]
    fn a_reference_with_no_host_matches_whatever_host_the_project_is_on() {
        let reference = parsed("acme/widget#4");
        assert!(
            reference.matches("git.company.example", "acme/widget"),
            "owner/repo#123 names no host, so it must reach a company server"
        );
        assert!(reference.matches("github.com", "acme/widget"));
    }

    #[test]
    fn a_reference_with_a_host_does_not_match_another_host() {
        let reference = parsed("https://github.com/acme/widget/pull/4");
        assert!(!reference.matches("git.company.example", "acme/widget"));
    }

    #[test]
    fn a_bare_number_matches_no_repository() {
        assert!(!parsed("#123").matches("github.com", "example/application"));
    }

    // --- browser URL rules, which the typed side must not diverge from -------
    //
    // Everything below is a way for hand-splitting to name a DIFFERENT
    // repository than the text names. That is the exact failure the two-rules
    // design exists to prevent, so it must not reappear here.

    #[test]
    fn a_dot_dot_in_a_url_path_names_the_repository_a_browser_would_open() {
        // A browser resolves this to https://github.com/acme/gadget#123. Naming
        // `acme/widget` instead would send dux at a repository the user did not
        // write down, and `acme/widget` is very likely a project they have.
        let reference = parsed("https://github.com/acme/widget/../gadget#123");
        assert_eq!(reference.host.as_deref(), Some("github.com"));
        assert_eq!(reference.owner_repo.as_deref(), Some("acme/gadget"));
        assert_eq!(reference.number, Some(123));
    }

    #[test]
    fn a_dot_segment_is_never_a_repository_name() {
        for input in [
            "acme/..",
            "./widget",
            "example/../application",
            "git@github.com:acme/widget/../gadget.git",
            "github.com/acme/./widget",
        ] {
            assert!(
                parse_typed_reference(input).is_err(),
                "{input} must be refused rather than read as a name"
            );
        }
    }

    #[test]
    fn percent_escapes_are_decoded_like_a_browser_decodes_them() {
        assert_eq!(
            repo_of("https://github.com/acme/wid%67et/pull/1"),
            (
                Some("github.com".to_string()),
                Some("acme/widget".to_string())
            )
        );
        assert_eq!(
            parsed("https://github.com/acme/wid%67et/pull/1").number,
            Some(1)
        );
    }

    #[test]
    fn a_percent_encoded_separator_is_refused_rather_than_folded_into_a_name() {
        // `acme%2Fwidget` decodes to `acme/widget`, one segment holding a
        // separator. Accepting it lets a single path component pose as two.
        assert!(parse_typed_reference("https://github.com/acme%2Fwidget/pull/1").is_err());
        assert!(parse_typed_reference("https://github.com/acme/wid%5Cget").is_err());
    }

    #[test]
    fn a_numeric_fragment_is_the_number_only_when_the_path_is_the_repository() {
        // `owner/repo#123` is a spelling. `owner/repo/issues#123` is a browser
        // route with an anchor on it, and reading 123 as the pull request
        // number invents a pull request nobody named.
        assert_eq!(
            parsed("https://github.com/acme/widget/issues#123").number,
            None
        );
        assert_eq!(
            parsed("https://github.com/acme/widget/security/dependabot#4").number,
            None
        );
        assert_eq!(
            parsed("https://github.com/acme/widget#123").number,
            Some(123)
        );
    }

    #[test]
    fn a_malformed_authority_is_refused_rather_than_read_as_a_host() {
        for input in [
            "https://[::1/acme/widget",
            "https://exa mple.com/acme/widget",
            "https:///acme/widget",
        ] {
            assert!(parse_typed_reference(input).is_err(), "{input}");
        }
    }

    #[test]
    fn a_scheme_dux_does_not_speak_is_refused_rather_than_reinterpreted() {
        // `ftp://github.com/acme/widget` used to fall through to the scp-like
        // rule, whose colon comes before its slash, and answer with the host
        // `ftp` and the repository `github.com/acme`.
        for input in [
            "ftp://github.com/acme/widget",
            "file:///acme/widget",
            "javascript://github.com/acme/widget",
        ] {
            assert!(parse_typed_reference(input).is_err(), "{input}");
        }
    }

    #[test]
    fn a_schemeless_host_with_a_port_is_a_host_with_a_port() {
        // git's scp-like shorthand has no port, so `github.com:8443/acme/widget`
        // cannot be scp syntax; it is a browser address with the scheme rubbed
        // off. Reading it as scp answered with the repository `8443/acme`.
        let reference = parsed("github.com:8443/acme/widget#123");
        assert_eq!(reference.host.as_deref(), Some("github.com:8443"));
        assert_eq!(reference.owner_repo.as_deref(), Some("acme/widget"));
        assert_eq!(reference.number, Some(123));
        // And a colon followed by anything else is still scp syntax.
        assert_eq!(
            repo_of("github.com:acme/widget"),
            (
                Some("github.com".to_string()),
                Some("acme/widget".to_string())
            )
        );
    }

    #[test]
    fn a_percent_in_an_ssh_family_authority_is_refused_rather_than_split_web_style() {
        // Git decodes a scheme-qualified ssh or native address and separates
        // host from path AFTERWARDS, in that order, so the escape is gone by
        // the time the cut is made: `ssh://user%2Fx@github.com/acme/widget`
        // really reaches ssh as the host `user` with the path
        // `/x@github.com/acme/widget`. A browser-style parser splits first and
        // answers `github.com` / `acme/widget`, a host and a repository the
        // address does not name.
        //
        // This is the same refusal, for the same reason, as the configured
        // address parser's: where dux cannot faithfully reproduce what git
        // does, it refuses rather than guessing.
        for input in [
            "ssh://user%2Fx@github.com/acme/widget/pull/7",
            "git+ssh://user%2Fx@github.com/acme/widget",
            "ssh+git://user%2Fx@github.com/acme/widget",
            "git://us%2Fer@github.com/acme/widget",
        ] {
            assert!(
                parse_typed_reference(input).is_err(),
                "{input} names a host dux cannot work out, so it must be refused"
            );
        }
        // And the ordinary spellings are untouched.
        assert_eq!(
            repo_of("ssh://git@github.com/acme/widget"),
            (
                Some("github.com".to_string()),
                Some("acme/widget".to_string())
            )
        );
        assert_eq!(
            repo_of("https://user%2Fx@github.com/acme/widget"),
            (
                Some("github.com".to_string()),
                Some("acme/widget".to_string())
            ),
            "curl splits the authority off FIRST and decodes each piece after, \
             so a percent under http(s) moves no boundary and is credentials"
        );
    }

    #[test]
    fn a_user_in_a_native_git_address_is_part_of_the_host_so_it_is_refused() {
        // Git's native protocol has no user component, unlike its ssh URL
        // syntax, so `git://user@github.com/acme/widget` sends git looking up
        // `user@github.com` on port 9418, not github.com.
        assert!(parse_typed_reference("git://user@github.com/acme/widget").is_err());
        assert_eq!(
            repo_of("git://github.com/acme/widget"),
            (
                Some("github.com".to_string()),
                Some("acme/widget".to_string())
            ),
            "the native protocol without a user still names what it says"
        );
    }

    #[test]
    fn repository_label_names_the_host_only_when_one_was_given() {
        assert_eq!(
            parsed("https://github.com/example/application/pull/1")
                .repository_label()
                .as_deref(),
            Some("github.com/example/application")
        );
        assert_eq!(
            parsed("example/application#1")
                .repository_label()
                .as_deref(),
            Some("example/application")
        );
        assert_eq!(parsed("#1").repository_label(), None);
    }
}

/// Resolution over real git repositories, written as the journeys a person
/// actually takes. Every project here is a real repository on disk with a real
/// `origin`, and the address is read by the same production call the surfaces
/// make, so the rewrite git applies in production applies here too.
#[cfg(test)]
mod resolution_tests {
    use super::*;
    use crate::gh::GithubHostPolicy;
    use crate::model::{Project, ProjectBranchStatus, ProviderKind};
    use std::path::Path;

    /// A host set naming both hosts these journeys use, standing in for what
    /// `gh auth status` reported.
    fn policy() -> GithubHostPolicy {
        GithubHostPolicy::Hosts(
            ["github.com".to_string(), "git.company.example".to_string()]
                .into_iter()
                .collect(),
        )
    }

    /// A real repository whose `origin` is `address`, wrapped as a project.
    ///
    /// The `git init` and `git remote add` run through the isolated command
    /// helper, so no developer's configuration decides what gets written. What
    /// READS the address afterwards is production code, which inherits the test
    /// process's environment on purpose: an `insteadOf` rewrite is meant to
    /// apply, because the rewritten address is the one git would really
    /// contact, and it is what a reference should be compared against.
    fn project_with_origin(name: &str, address: &str) -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        for args in [
            vec!["init", "-b", "main"],
            vec!["remote", "add", "origin", address],
        ] {
            let out = crate::git::test_support::git_command()
                .args(&args)
                .current_dir(&path)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let project = Project {
            id: format!("id-{name}"),
            name: name.to_string(),
            path: path.to_string_lossy().to_string(),
            explicit_default_provider: None,
            default_provider: ProviderKind::new("claude"),
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: false,
            created_at: None,
        };
        (dir, project)
    }

    /// Resolution under the journey policy, so each test reads as the question
    /// it is asking rather than as three arguments.
    fn resolve(reference: &TypedReference, projects: &[Project]) -> ReferenceResolution {
        resolve_reference_projects(reference, projects, &policy())
    }

    fn names(matched: &[Project]) -> Vec<&str> {
        matched.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn a_pasted_pull_request_link_finds_the_project_that_repository_is_open_in() {
        let (_a, widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        let (_b, other) = project_with_origin("other", "git@github.com:acme/gadget.git");
        let projects = vec![widget, other];

        let reference = parse_typed_reference("https://github.com/acme/widget/pull/17").unwrap();
        let matched = resolve(&reference, &projects).matches;
        assert_eq!(names(&matched), vec!["widget"]);
        assert_eq!(reference.number, Some(17));
    }

    #[test]
    fn a_browser_url_with_issues_on_the_end_still_finds_the_project() {
        let (_a, widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        let projects = vec![widget];

        let reference = parse_typed_reference("https://github.com/acme/widget/issues").unwrap();
        assert_eq!(
            names(&resolve(&reference, &projects).matches),
            vec!["widget"],
            "a trailing browser route must not change which repository is named"
        );
    }

    #[test]
    fn a_repository_dux_does_not_have_matches_nothing_and_can_be_named() {
        let (_a, widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        let projects = vec![widget];

        let reference = parse_typed_reference("https://github.com/acme/unknown/pull/3").unwrap();
        assert!(resolve(&reference, &projects).matches.is_empty());
        assert_eq!(
            reference.repository_label().as_deref(),
            Some("github.com/acme/unknown"),
            "the message has to be able to say WHICH repository dux has no project for"
        );
    }

    #[test]
    fn the_same_repository_checked_out_twice_returns_both_so_the_user_can_pick() {
        let (_a, first) = project_with_origin("widget", "git@github.com:acme/widget.git");
        let (_b, second) = project_with_origin("widget-review", "git@github.com:acme/widget.git");
        let projects = vec![first, second];

        let reference = parse_typed_reference("acme/widget#8").unwrap();
        let matched = resolve(&reference, &projects).matches;
        assert_eq!(names(&matched), vec!["widget", "widget-review"]);
    }

    #[test]
    fn owner_repo_hash_number_resolves_to_the_company_server_rather_than_github() {
        // The whole reason `owner/repo#123` must not assume a host: this user's
        // only checkout of acme/widget is on their company server.
        let (_a, company) =
            project_with_origin("widget", "git@git.company.example:acme/widget.git");
        let (_b, elsewhere) = project_with_origin("gadget", "git@github.com:acme/gadget.git");
        let projects = vec![company, elsewhere];

        let reference = parse_typed_reference("acme/widget#123").unwrap();
        assert_eq!(reference.host, None);
        let matched = resolve(&reference, &projects).matches;
        assert_eq!(names(&matched), vec!["widget"]);
    }

    #[test]
    fn a_reference_naming_github_does_not_reach_a_company_checkout() {
        let (_a, company) =
            project_with_origin("widget", "git@git.company.example:acme/widget.git");
        let projects = vec![company];

        let reference = parse_typed_reference("https://github.com/acme/widget/pull/1").unwrap();
        assert!(
            resolve(&reference, &projects).matches.is_empty(),
            "a host that was written down is a host that must be honoured"
        );
    }

    #[test]
    fn a_bare_number_resolves_to_nothing_because_it_names_nothing() {
        let (_a, widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        let projects = vec![widget];

        let reference = parse_typed_reference("#123").unwrap();
        assert!(reference.owner_repo.is_none());
        assert!(
            resolve(&reference, &projects).matches.is_empty(),
            "a bare number must be refused with an explanation, never resolved by guessing"
        );
    }

    #[test]
    fn case_and_a_dot_git_suffix_do_not_stop_a_project_being_found() {
        let (_a, widget) = project_with_origin("widget", "git@GitHub.com:Acme/Widget.git");
        let projects = vec![widget];

        let reference = parse_typed_reference("https://github.com/acme/widget.git/pull/2").unwrap();
        assert_eq!(
            names(&resolve(&reference, &projects).matches),
            vec!["widget"]
        );
    }

    #[test]
    fn a_project_on_a_host_the_policy_does_not_allow_is_reported_as_uninspected() {
        let (_a, gitlab) = project_with_origin("mirror", "git@gitlab.com:acme/widget.git");
        let projects = vec![gitlab];

        let reference = parse_typed_reference("acme/widget#5").unwrap();
        let resolution = resolve(&reference, &projects);
        assert!(
            resolution.matches.is_empty(),
            "dux may not name a host gh cannot serve, so such a project cannot be the answer"
        );
        assert_eq!(
            resolution.uninspected,
            vec![UninspectedProject {
                name: "mirror".to_string(),
                reason: Uninspectable::HostNotAllowed,
            }],
            "and it must not be reported as a non-match, or the message would say \
             there is no checkout of a repository this project may well be a checkout of"
        );
        assert!(!resolution.is_complete());
        assert_eq!(
            resolution.uninspected_summary().as_deref(),
            Some("1 is on a host dux may not ask about")
        );
    }

    #[test]
    fn a_project_whose_path_is_gone_is_reported_as_uninspected_rather_than_probed() {
        let (dir, mut widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        widget.path_missing = true;
        let projects = vec![widget];

        let reference = parse_typed_reference("acme/widget#1").unwrap();
        let resolution = resolve(&reference, &projects);
        assert!(resolution.matches.is_empty());
        assert_eq!(
            resolution.uninspected,
            vec![UninspectedProject {
                name: "widget".to_string(),
                reason: Uninspectable::PathMissing,
            }]
        );
        drop(dir);
    }

    #[test]
    fn editing_a_projects_address_changes_the_answer_with_nothing_to_invalidate() {
        // The reason there is no cache. Nothing here resets anything: the second
        // call simply reads git again, which is the only way an edited address,
        // a changed rewrite rule or a repaired remote can ever be noticed.
        let (dir, widget) = project_with_origin("widget", "git@github.com:acme/gadget.git");
        let projects = vec![widget];
        let reference = parse_typed_reference("acme/widget#1").unwrap();
        assert!(resolve(&reference, &projects).matches.is_empty());

        let out = crate::git::test_support::git_command()
            .args([
                "remote",
                "set-url",
                "origin",
                "git@github.com:acme/widget.git",
            ])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(out.status.success());

        assert_eq!(
            names(&resolve(&reference, &projects).matches),
            vec!["widget"]
        );
    }

    #[test]
    fn a_project_with_no_origin_at_all_is_simply_not_a_match() {
        let dir = tempfile::tempdir().unwrap();
        let out = crate::git::test_support::git_command()
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        let (_a, widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        let bare = Project {
            path: dir.path().to_string_lossy().to_string(),
            name: "no-origin".to_string(),
            ..widget.clone()
        };
        let projects = vec![bare, widget];

        let reference = parse_typed_reference("acme/widget#1").unwrap();
        let resolution = resolve(&reference, &projects);
        assert_eq!(names(&resolution.matches), vec!["widget"]);
        // And the unreadable one is genuinely unreadable, rather than passing by
        // accident because the loop never reached it.
        assert!(crate::git::remote_github_repo(Path::new(&projects[0].path), &policy()).is_none());
        // It is reported, not swallowed: dux could not tell whether it is a
        // checkout, and a message that says "no project is" would be wrong.
        assert_eq!(
            resolution.uninspected,
            vec![UninspectedProject {
                name: "no-origin".to_string(),
                reason: Uninspectable::AddressUnreadable,
            }]
        );
    }

    #[test]
    fn the_same_repository_on_two_allowed_hosts_is_told_apart_by_the_host_written_down() {
        // Two projects, the same `owner/repo`, different servers, and BOTH hosts
        // signed in to. The only thing that can separate them is whether the
        // reference named a host.
        let (_a, company) =
            project_with_origin("widget-company", "git@git.company.example:acme/widget.git");
        let (_b, github) = project_with_origin("widget-github", "git@github.com:acme/widget.git");
        let projects = vec![company, github];

        let hostless = parse_typed_reference("acme/widget#4").unwrap();
        assert_eq!(
            names(&resolve(&hostless, &projects).matches),
            vec!["widget-company", "widget-github"],
            "owner/repo#123 names no host, so both checkouts are candidates and the user picks"
        );

        let on_github = parse_typed_reference("https://github.com/acme/widget/pull/4").unwrap();
        assert_eq!(
            names(&resolve(&on_github, &projects).matches),
            vec!["widget-github"],
            "a host that was written down is a host that must be honoured"
        );

        let on_company =
            parse_typed_reference("https://git.company.example/acme/widget/pull/4").unwrap();
        assert_eq!(
            names(&resolve(&on_company, &projects).matches),
            vec!["widget-company"]
        );
    }

    #[test]
    fn a_complete_answer_says_so_and_names_nothing_it_could_not_check() {
        let (_a, widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        let projects = vec![widget];

        let reference = parse_typed_reference("https://github.com/acme/unknown/pull/3").unwrap();
        let resolution = resolve(&reference, &projects);
        assert!(resolution.matches.is_empty());
        assert!(
            resolution.is_complete(),
            "every project was inspected, so \"none of them\" really means none of them"
        );
        assert_eq!(resolution.uninspected_summary(), None);
    }

    #[test]
    fn a_bare_number_reports_nothing_uninspected_because_it_asked_nothing() {
        let (_a, mut widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        widget.path_missing = true;
        let projects = vec![widget];

        let reference = parse_typed_reference("#123").unwrap();
        let resolution = resolve(&reference, &projects);
        assert!(resolution.matches.is_empty());
        assert!(
            resolution.is_complete(),
            "a bare number is refused with an explanation; listing every project as \
             unchecked would turn that refusal into a scare"
        );
    }
}

#[cfg(test)]
mod independent_typed_parser_check {
    use super::*;

    fn repo(raw: &str) -> Option<String> {
        parse_typed_reference(raw)
            .ok()
            .and_then(|r| r.repository_label())
    }

    /// A typed address must name the repository a BROWSER would name. Every case
    /// here previously answered with a different, real repository, or read a
    /// scheme as a hostname.
    #[test]
    fn a_typed_address_names_what_a_browser_names() {
        // Dot segments must normalise, not be taken literally.
        assert_eq!(
            repo("https://github.com/acme/widget/../gadget#123").as_deref(),
            Some("github.com/acme/gadget")
        );
        assert_eq!(
            repo("https://github.com/acme/./widget").as_deref(),
            Some("github.com/acme/widget")
        );
        // Dot components as repository names are refused.
        for bad in [
            "acme/..",
            "example/../application",
            "acme/.",
            "github.com/acme/..",
        ] {
            assert_eq!(repo(bad), None, "must refuse {bad}");
        }
        // Percent escapes decode; an encoded separator is refused.
        assert_eq!(
            repo("https://github.com/acme/wid%67et/pull/1").as_deref(),
            Some("github.com/acme/widget")
        );
        assert_eq!(repo("https://github.com/acme%2Fwidget/pull/1"), None);
        // A scheme is never a hostname.
        for bad in [
            "ftp://github.com/acme/widget",
            "file:///acme/widget",
            "javascript://github.com/acme/widget",
            "data://github.com/acme/widget",
        ] {
            assert_eq!(repo(bad), None, "a scheme must not become a host: {bad}");
        }
        // A malformed authority is refused rather than guessed at.
        assert_eq!(repo("https://[::1/acme/widget"), None);
        // The lenient shapes still work.
        for good in [
            "example/application",
            "github.com/example/application",
            "https://github.com/example/application",
            "https://github.com/example/application/issues",
            "https://github.com/example/application/security/dependabot",
            "https://github.com/example/application/this/is/a/made/up/path",
            "git@github.com:example/application.git",
        ] {
            assert!(
                parse_typed_reference(good).is_ok(),
                "must still accept {good}"
            );
        }
    }
}

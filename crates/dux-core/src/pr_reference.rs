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

    let (body, scheme) = strip_scheme(input);

    // The fragment comes off before the query, because a fragment may itself
    // contain a `?` and that `?` is fragment text rather than a query.
    let (body, fragment) = match body.split_once('#') {
        Some((before, after)) => (before, Some(after)),
        None => (body, None),
    };
    let body = body.split('?').next().unwrap_or(body);

    let (host, path) = match scheme {
        // `[user@]host:path`, git's scp-like shorthand, is ssh by definition.
        // It is recognised only when the colon comes before any slash, exactly
        // as git documents, so `example/application:tags` is not mistaken for
        // an address.
        Scheme::None => match split_scp_like(body) {
            Some((authority, path)) => (Some(host_from_authority(authority, true)?), path),
            None => {
                // A schemeless value is `owner/repo[/...]` or
                // `host/owner/repo[/...]`, and the shape of the leading segment
                // is what tells them apart. A dot (or a port, or `localhost`)
                // makes it a host, because an owner cannot hold one: GitHub
                // account and organisation names are letters, digits and
                // hyphens. So `github.com/example` is a host with its
                // repository missing and is refused, rather than read as a
                // repository named `example` owned by `github.com`.
                let segments = path_segments(body);
                if looks_like_host(segments.first().copied().unwrap_or_default()) {
                    let host = host_from_authority(segments[0], false)?;
                    let rest = body.split_once('/').map(|(_, rest)| rest).unwrap_or("");
                    (Some(host), rest)
                } else {
                    (None, body)
                }
            }
        },
        Scheme::Web | Scheme::Ssh | Scheme::Git => {
            let ssh_like = matches!(scheme, Scheme::Ssh);
            let (authority, path) = match body.split_once('/') {
                Some((authority, path)) => (authority, path),
                None => return Err(UNPARSEABLE.to_string()),
            };
            (Some(host_from_authority(authority, ssh_like)?), path)
        }
    };

    let segments = path_segments(path);
    if segments.len() < 2 {
        return Err(UNPARSEABLE.to_string());
    }
    let owner = segments[0];
    let repo = strip_dot_git(segments[1]);
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
    // A fragment of nothing but digits is the `owner/repo#123` spelling. A
    // fragment on a pull URL (`#discussion_r1`, `#issuecomment-4`) is not, and
    // a number already read from the path wins over one either way.
    if number.is_none()
        && let Some(fragment) = fragment
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

/// Which family of address the text was written in. Only the family matters
/// here: it decides where the host ends and whether a port belongs to a
/// transport rather than to the server dux would query.
enum Scheme {
    None,
    Web,
    Ssh,
    Git,
}

/// Take a scheme off the front, case-insensitively. A person may well type
/// `HTTPS://`; unlike a configured address, which git matches case-sensitively
/// against its own table, this text is not handed to git.
fn strip_scheme(input: &str) -> (&str, Scheme) {
    let Some((scheme, rest)) = input.split_once("://") else {
        return (input, Scheme::None);
    };
    let lowered = scheme.to_ascii_lowercase();
    match lowered.as_str() {
        "http" | "https" => (rest, Scheme::Web),
        "ssh" | "git+ssh" | "ssh+git" => (rest, Scheme::Ssh),
        "git" => (rest, Scheme::Git),
        _ => (input, Scheme::None),
    }
}

/// Git's scp-like shorthand, `[user@]host:path`, recognised only when the colon
/// precedes any slash.
fn split_scp_like(input: &str) -> Option<(&str, &str)> {
    let colon = input.find(':')?;
    if let Some(slash) = input.find('/')
        && slash < colon
    {
        return None;
    }
    let (authority, path) = input.split_at(colon);
    Some((authority, &path[1..]))
}

/// The host a message and a match should use, from the authority as written.
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
fn is_repository_component(part: &str) -> bool {
    !part.is_empty() && !part.chars().any(|c| c.is_whitespace() || c == '\\')
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

/// Which of `projects` are checkouts of the repository `reference` names.
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
/// [`crate::git::remote_github_repo`] already returns. That is the correct
/// anchor: it is the address git would really contact.
///
/// A project whose path is missing is skipped rather than probed, and so is one
/// whose address git cannot read or whose host the policy does not allow. Each
/// of those means "not a checkout of this repository as far as dux can tell",
/// which is the same answer as not matching.
pub fn resolve_reference_projects(
    reference: &TypedReference,
    projects: &[crate::model::Project],
    policy: &crate::gh::GithubHostPolicy,
) -> Vec<crate::model::Project> {
    if reference.owner_repo.is_none() {
        // A bare number names no repository, so there is nothing to resolve. The
        // caller refuses it with an explanation rather than searching for a
        // repository nobody named.
        return Vec::new();
    }
    projects
        .iter()
        .filter(|project| !project.path_missing)
        .filter(|project| {
            match crate::git::remote_github_repo(std::path::Path::new(&project.path), policy) {
                Some(remote) => reference.matches(&remote.host, &remote.owner_repo),
                None => false,
            }
        })
        .cloned()
        .collect()
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
    let matches = resolve_reference_projects(&reference, &projects, &policy);
    let _ = worker_tx.send(crate::worker::WorkerEvent::PullRequestReferenceResolved {
        raw_input,
        repository,
        matches,
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

    fn names(matched: &[Project]) -> Vec<&str> {
        matched.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn a_pasted_pull_request_link_finds_the_project_that_repository_is_open_in() {
        let (_a, widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        let (_b, other) = project_with_origin("other", "git@github.com:acme/gadget.git");
        let projects = vec![widget, other];

        let reference = parse_typed_reference("https://github.com/acme/widget/pull/17").unwrap();
        let matched = resolve_reference_projects(&reference, &projects, &policy());
        assert_eq!(names(&matched), vec!["widget"]);
        assert_eq!(reference.number, Some(17));
    }

    #[test]
    fn a_browser_url_with_issues_on_the_end_still_finds_the_project() {
        let (_a, widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        let projects = vec![widget];

        let reference = parse_typed_reference("https://github.com/acme/widget/issues").unwrap();
        assert_eq!(
            names(&resolve_reference_projects(
                &reference,
                &projects,
                &policy()
            )),
            vec!["widget"],
            "a trailing browser route must not change which repository is named"
        );
    }

    #[test]
    fn a_repository_dux_does_not_have_matches_nothing_and_can_be_named() {
        let (_a, widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        let projects = vec![widget];

        let reference = parse_typed_reference("https://github.com/acme/unknown/pull/3").unwrap();
        assert!(resolve_reference_projects(&reference, &projects, &policy()).is_empty());
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
        let matched = resolve_reference_projects(&reference, &projects, &policy());
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
        let matched = resolve_reference_projects(&reference, &projects, &policy());
        assert_eq!(names(&matched), vec!["widget"]);
    }

    #[test]
    fn a_reference_naming_github_does_not_reach_a_company_checkout() {
        let (_a, company) =
            project_with_origin("widget", "git@git.company.example:acme/widget.git");
        let projects = vec![company];

        let reference = parse_typed_reference("https://github.com/acme/widget/pull/1").unwrap();
        assert!(
            resolve_reference_projects(&reference, &projects, &policy()).is_empty(),
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
            resolve_reference_projects(&reference, &projects, &policy()).is_empty(),
            "a bare number must be refused with an explanation, never resolved by guessing"
        );
    }

    #[test]
    fn case_and_a_dot_git_suffix_do_not_stop_a_project_being_found() {
        let (_a, widget) = project_with_origin("widget", "git@GitHub.com:Acme/Widget.git");
        let projects = vec![widget];

        let reference = parse_typed_reference("https://github.com/acme/widget.git/pull/2").unwrap();
        assert_eq!(
            names(&resolve_reference_projects(
                &reference,
                &projects,
                &policy()
            )),
            vec!["widget"]
        );
    }

    #[test]
    fn a_project_on_a_host_the_policy_does_not_allow_is_not_a_match() {
        let (_a, gitlab) = project_with_origin("mirror", "git@gitlab.com:acme/widget.git");
        let projects = vec![gitlab];

        let reference = parse_typed_reference("acme/widget#5").unwrap();
        assert!(
            resolve_reference_projects(&reference, &projects, &policy()).is_empty(),
            "dux may not name a host gh cannot serve, so such a project cannot be the answer"
        );
    }

    #[test]
    fn a_project_whose_path_is_gone_is_skipped_rather_than_probed() {
        let (dir, mut widget) = project_with_origin("widget", "git@github.com:acme/widget.git");
        widget.path_missing = true;
        let projects = vec![widget];

        let reference = parse_typed_reference("acme/widget#1").unwrap();
        assert!(resolve_reference_projects(&reference, &projects, &policy()).is_empty());
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
        assert!(resolve_reference_projects(&reference, &projects, &policy()).is_empty());

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
            names(&resolve_reference_projects(
                &reference,
                &projects,
                &policy()
            )),
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
        assert_eq!(
            names(&resolve_reference_projects(
                &reference,
                &projects,
                &policy()
            )),
            vec!["widget"]
        );
        // And the unreadable one is genuinely unreadable, rather than passing by
        // accident because the loop never reached it.
        assert!(crate::git::remote_github_repo(Path::new(&projects[0].path), &policy()).is_none());
    }
}

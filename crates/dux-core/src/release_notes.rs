//! Release notes: parsing a GitHub release body into the shape the what's-new
//! screen renders, fetching a release, and caching it on disk.
//!
//! [`load_release_notes`] is the entry point both surfaces call. It asks for the
//! RUNNING version's own tag, because the screen must describe the version the
//! user actually has — not whatever GitHub published most recently.
//! [`fetch_latest`] exists for the one case with no tag to ask for: a development
//! build.
//!
//! Every `fetch_*` / `load_*` function here BLOCKS. They must only ever be called
//! from a background worker, never from a UI thread — see CLAUDE.md.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The trimmed shape of a GitHub release body: everything the what's-new screen
/// needs and nothing it doesn't. Living in core means the TUI and the web render
/// identical data and neither needs a Markdown renderer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedBody {
    /// The release headline, from the leading `## ...` line.
    pub headline: String,
    /// Intro prose, one entry per paragraph, before the first `### ...`.
    pub paragraphs: Vec<String>,
    /// The `### ...` feature titles.
    pub sections: Vec<String>,
}

impl ParsedBody {
    /// Whether there is anything to render UNDER the title.
    ///
    /// The headline is deliberately excluded: both screens render it as their
    /// title, so a release whose body is only a headline leaves the body area
    /// empty, and an empty body area with no explanation is the failure this
    /// predicate exists to detect. See [`ReleaseNotes::has_renderable_body`],
    /// which is the one the screens actually call.
    ///
    /// Delegates rather than repeating the rule, so this and
    /// [`ReleaseNotes::has_renderable_body`] cannot drift apart with only one of
    /// them under test.
    pub fn has_renderable_body(&self) -> bool {
        body_is_renderable(&self.paragraphs, &self.sections)
    }
}

/// THE predicate: whether a parsed body has anything worth putting under the
/// title. The single definition behind both `has_renderable_body` methods and the
/// thing `hasRenderableBody` in `crates/dux-web/web/src/lib/releaseNotes.ts`
/// mirrors.
pub fn body_is_renderable(paragraphs: &[String], sections: &[String]) -> bool {
    has_content(paragraphs) || has_content(sections)
}

/// Whether any entry carries text a reader would actually see. A vector of empty
/// strings is not content: a release heading that was entirely inline markup
/// collapses to `""`, and rendering that as a lone blank bullet is the same empty
/// screen with extra steps.
fn has_content(entries: &[String]) -> bool {
    entries.iter().any(|entry| entry_is_renderable(entry))
}

/// The code points BOTH surfaces treat as invisible.
///
/// Neither language's own `trim` can be the definition, because they trim
/// DIFFERENT sets, which is a real divergence and not a theoretical one: Rust
/// trims U+0085 (next line) and JavaScript does not; JavaScript trims U+FEFF
/// (byte-order mark) and Rust does not. So the same release body emptied out in
/// the browser and kept a body in the terminal, or the reverse, and the two
/// screens disagreed about whether to show the no-notes explanation.
///
/// The set is Unicode `White_Space` (which is exactly what `char::is_whitespace`
/// answers, U+0085 included) plus the zero-width characters, which are worse than
/// whitespace: they render as literally nothing, so a body made of them is the
/// original blank-panel bug with an extra step. The web declares the same set as
/// `INVISIBLE_CODE_POINTS`, and a test here reads that declaration back.
///
/// Sharing the SET is not on its own enough, and for a while it was all that was
/// shared: [`html_line_break_at`] still asked `char::is_whitespace` about the gaps
/// inside a `<br>` while the web asked `\s`, so `<br` U+0085 `/>` and `<br` U+FEFF
/// `/>` still landed on opposite answers, measured. Both matchers now call this
/// function, and `tests/fixtures/release_notes_cross_language.json` pins the
/// ANSWERS from both languages rather than only the set.
pub fn is_invisible_char(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}')
}

/// Whether one parsed entry (an intro paragraph or a feature title) is worth
/// rendering.
///
/// Emptiness is not enough of a test. The release pipeline appends a horizontal
/// rule before its `## Installation` section, so the commonest degraded body in
/// practice parses to a headline plus a paragraph of `---`, and rendering a title
/// above a lone dash-run is the blank panel with one extra character. HTML line
/// breaks and comments are the same story: legal in a release body, meaningless
/// to a parser that renders plain text.
fn entry_is_renderable(entry: &str) -> bool {
    let visible = strip_invisible_markup(entry);
    !visible.is_empty() && !is_thematic_break(&visible)
}

/// Drops HTML comments, HTML line breaks, and every invisible code point, leaving
/// what a reader would actually see. Char-based throughout (see
/// [`parse_release_body`]).
fn strip_invisible_markup(entry: &str) -> String {
    let chars: Vec<char> = entry.chars().collect();
    let mut out = String::with_capacity(entry.len());
    let mut i = 0;
    while i < chars.len() {
        if matches_at(&chars, i, "<!--") {
            // An unterminated comment swallows the rest, which is what a Markdown
            // renderer does with it too.
            i = match find_at(&chars, i + 4, "-->") {
                Some(close) => close + 3,
                None => chars.len(),
            };
            continue;
        }
        if let Some(after) = html_line_break_at(&chars, i) {
            i = after;
            continue;
        }
        if !is_invisible_char(chars[i]) {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

/// The index just past a `<br>`, `<br/>`, or `<br />` starting at `i`, if there is
/// one. Case-insensitive, because release bodies are hand-written HTML-in-Markdown.
///
/// The gaps are measured with [`is_invisible_char`], NOT with `char::is_whitespace`.
/// Asking each language what whitespace is, is the very bug the shared set exists to
/// close: `char::is_whitespace` covers U+0085 and excludes U+FEFF, and JavaScript's
/// `\s` does the opposite, so `<br` U+0085 `/>` was empty here and renderable in the
/// browser while `<br` U+FEFF `/>` was the reverse. The cases are in
/// `tests/fixtures/release_notes_cross_language.json`, which both surfaces read.
fn html_line_break_at(chars: &[char], i: usize) -> Option<usize> {
    if chars.get(i) != Some(&'<') {
        return None;
    }
    if !chars
        .get(i + 1)
        .is_some_and(|c| c.eq_ignore_ascii_case(&'b'))
    {
        return None;
    }
    if !chars
        .get(i + 2)
        .is_some_and(|c| c.eq_ignore_ascii_case(&'r'))
    {
        return None;
    }
    let mut j = i + 3;
    while chars.get(j).is_some_and(|c| is_invisible_char(*c)) {
        j += 1;
    }
    if chars.get(j) == Some(&'/') {
        j += 1;
        while chars.get(j).is_some_and(|c| is_invisible_char(*c)) {
            j += 1;
        }
    }
    (chars.get(j) == Some(&'>')).then_some(j + 1)
}

/// Whether the visible remainder is a Markdown thematic break: three or more of
/// the same `-`, `*`, or `_`. The spaced forms (`- - -`) arrive here already
/// closed up, because the spaces are invisible and have been dropped.
fn is_thematic_break(visible: &str) -> bool {
    let mut chars = visible.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    visible.chars().count() >= 3 && chars.all(|c| c == first)
}

fn matches_at(chars: &[char], i: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(offset, want)| chars.get(i + offset) == Some(&want))
}

fn find_at(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    (from..chars.len()).find(|i| matches_at(chars, *i, needle))
}

/// Splits a release body into headline, intro paragraphs, and feature titles.
///
/// Stops at the SECOND `## ` heading, which is where the machine-written tail
/// begins. BOTH generators APPEND: GitHub's auto-generated `## What's Changed`
/// commit list lands after every human-written section (check the body of any
/// real dux release), and the release workflow then appends a horizontal rule and
/// `## Installation`. Everything after that second heading is machine-written and
/// not worth showing in a modal.
///
/// Getting that direction right matters for the next reader, not for the code: it
/// is why the human's own `## ` line comes FIRST and is therefore the headline,
/// and why a body with no human `## ` line hands the headline to the boilerplate
/// instead of merely stopping early.
///
/// Char-based throughout: release prose is full of multi-byte punctuation and
/// byte slicing would panic mid-character.
pub fn parse_release_body(body: &str) -> ParsedBody {
    let mut notes = ParsedBody::default();
    let mut para = String::new();
    let mut seen_top_heading = false;
    let mut in_code = false;

    for raw in body.lines() {
        let trimmed = raw.trim();

        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("## ") {
            if seen_top_heading {
                break; // "What's Changed" / "Installation" — stop here.
            }
            seen_top_heading = true;
            notes.headline = strip_inline_markup(rest);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            flush(&mut para, &mut notes.paragraphs);
            notes.sections.push(strip_inline_markup(rest));
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut para, &mut notes.paragraphs);
            continue;
        }
        // Only collect prose before the first feature section; the bodies of the
        // sections themselves are what we deliberately drop.
        if notes.sections.is_empty() {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(trimmed);
        }
    }
    flush(&mut para, &mut notes.paragraphs);
    notes
}

fn flush(para: &mut String, out: &mut Vec<String>) {
    if !para.trim().is_empty() {
        out.push(strip_inline_markup(para.trim()));
    }
    para.clear();
}

/// Removes the Markdown syntax the screens cannot render, keeping the readable
/// text: `*`, `` ` ``, and `_` are dropped, and `[text](url)` keeps `text`.
///
/// Char-based throughout (see [`parse_release_body`]).
///
/// The `]` lookahead is cached behind a FORWARD-ONLY cursor (`next_close` plus a
/// never-decreasing `scan_pos`), so a run of unmatched `[` costs one pass over
/// the string instead of one pass PER bracket. Semantics are unchanged: only the
/// FIRST `]` after the bracket is considered, the next char must be `(` for it to
/// count as a link, and anything else emits a literal `[`.
fn strip_inline_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    // The first `]` at or after `scan_pos`, and how far the `]` scan has reached.
    // `scan_pos` never decreases, which is what bounds the total scanning work to
    // O(n) no matter how many unmatched brackets appear or how far away the `]`
    // sits.
    let mut next_close: Option<usize> = None;
    let mut scan_pos = 0usize;
    while i < chars.len() {
        match chars[i] {
            '*' | '`' | '_' => {}
            '[' => {
                // `[text](url)` — keep `text`, drop the target.
                if next_close.is_none_or(|j| j < i) {
                    // Resume from wherever the scan left off; never restart at
                    // `i` alone, or the work becomes quadratic again. Anything
                    // between the last known `]` and `i` is already behind the
                    // bracket, so skipping it cannot change the answer.
                    let mut j = scan_pos.max(i);
                    while j < chars.len() && chars[j] != ']' {
                        j += 1;
                    }
                    if j < chars.len() {
                        next_close = Some(j);
                        scan_pos = j + 1;
                    } else {
                        // No `]` remains anywhere; the cursor parks at the end so
                        // every later bracket resolves in constant time.
                        next_close = None;
                        scan_pos = chars.len();
                    }
                }
                if let Some(j) = next_close
                    && chars.get(j + 1) == Some(&'(')
                {
                    let mut k = j + 2;
                    while k < chars.len() && chars[k] != ')' {
                        k += 1;
                    }
                    // Materialized only for a real link, so the total text copied
                    // is bounded by the input: a match jumps `i` past `k`.
                    out.extend(chars[i + 1..j].iter());
                    i = k + 1;
                    continue;
                }
                out.push('[');
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

/// One release, trimmed to what the what's-new screen renders.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseNotes {
    /// The release tag, e.g. `v0.6.0`.
    pub version: String,
    pub headline: String,
    pub paragraphs: Vec<String>,
    pub sections: Vec<String>,
    /// The release's own web page. Taken from the API's `html_url` when present,
    /// falling back to the releases index.
    pub html_url: String,
}

/// What the what's-new screen says when [`ReleaseNotes::has_renderable_body`] is
/// false: the release exists, its body had nothing the screen could read, and the
/// full notes are one click away. Never an empty pane with no explanation.
///
/// The TUI renders this string directly. The web keeps its own copy in
/// `crates/dux-web/web/src/lib/releaseNotes.ts` (a TS surface cannot import a Rust
/// const), which `FirstLoadDialog.tsx` imports; if you reword one, reword the
/// other. `the_web_mirror_of_the_no_notes_surface_has_not_drifted` reads that file
/// and fails if you do not.
pub const NO_NOTES_EXPLANATION: &str =
    "This release published no notes we could read. Open the full notes to see what changed.";

impl ReleaseNotes {
    /// Whether the what's-new screen has anything to render under its title.
    ///
    /// `false` means the release body was empty, or shaped in a way the parser
    /// could not read as prose or feature titles (see the required format in
    /// `CONTRIBUTING.md`). BOTH surfaces must then show an explanation and a link
    /// to the full notes instead of an empty pane: the TUI does it in
    /// `whats_new_lines`, and the web in `FirstLoadDialog`. Living here rather
    /// than being re-derived per surface is what keeps them saying the same thing.
    pub fn has_renderable_body(&self) -> bool {
        body_is_renderable(&self.paragraphs, &self.sections)
    }
}

/// The subset of GitHub's release payload dux reads. `#[serde(default)]` on the
/// optional fields keeps a payload with extra keys (GitHub sends dozens) working
/// while a payload with no `tag_name` at all is rejected as not-a-release.
#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

/// Connect and total-request budget. Short on purpose: this runs at startup and
/// its failure mode (show nothing, try again next launch) is cheap, so waiting is
/// worse than giving up.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Hard ceiling on the response body dux will read into memory. A release body
/// is a few kilobytes; 512 KiB is generous and still bounds a hostile or broken
/// endpoint.
pub const MAX_BODY_BYTES: usize = 512 * 1024;

/// The `User-Agent` dux presents. GitHub rejects API requests that send none.
fn user_agent() -> String {
    format!("dux/{} (+{})", crate::display_version(), crate::urls::REPO)
}

/// The latest-release endpoint under `api_base`.
///
/// `api_base` is injectable so tests can point at a local server; production
/// callers pass [`crate::urls::GITHUB_API_BASE`].
pub fn latest_release_endpoint(api_base: &str) -> String {
    format!(
        "{}/repos/{}/releases/latest",
        api_base.trim_end_matches('/'),
        crate::urls::REPO_SLUG
    )
}

/// The endpoint for ONE release, looked up by its tag. This is the endpoint the
/// automatic startup path uses, because the screen must describe the version the
/// user is actually running.
pub fn tag_release_endpoint(api_base: &str, tag: &str) -> String {
    format!(
        "{}/repos/{}/releases/tags/{}",
        api_base.trim_end_matches('/'),
        crate::urls::REPO_SLUG,
        tag
    )
}

/// Whether `tag` is safe to interpolate into a URL path.
///
/// dux's own tags are `vX.Y.Z`, but this is a guard, not a formality: a tag
/// containing `/` or `..` would rewrite the request path (`../../users/x`), and a
/// tag with a `?` or `#` would truncate it. Anything outside this set can never
/// name a dux release, so refusing it is both safe and definitive.
fn is_path_safe_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
}

/// Why a fetch did not produce notes.
///
/// Split into two variants because the caller must treat them oppositely: see
/// [`crate::first_load::NotesOutcome`], which this maps onto.
#[derive(Debug)]
pub enum FetchError {
    /// GitHub answered definitively: no release exists for this tag (HTTP 404).
    /// Retrying cannot change the answer.
    NoSuchRelease { tag: String },
    /// Anything that might succeed later: offline, DNS, timeout, 5xx, a rate
    /// limit, or a response dux could not parse.
    Transient(anyhow::Error),
}

impl FetchError {
    /// Whether the answer is final. `true` means stop asking.
    pub fn is_definitive(&self) -> bool {
        matches!(self, Self::NoSuchRelease { .. })
    }

    /// The [`crate::first_load`] outcome this failure implies.
    pub fn outcome(&self) -> crate::first_load::NotesOutcome {
        match self {
            Self::NoSuchRelease { .. } => crate::first_load::NotesOutcome::NoSuchRelease,
            Self::Transient(_) => crate::first_load::NotesOutcome::TemporarilyUnavailable,
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchRelease { tag } => write!(
                f,
                "GitHub has no published release for tag {tag}, so there are no release notes to show"
            ),
            Self::Transient(err) => write!(f, "{err:#}"),
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoSuchRelease { .. } => None,
            Self::Transient(err) => err.source(),
        }
    }
}

impl From<anyhow::Error> for FetchError {
    fn from(err: anyhow::Error) -> Self {
        Self::Transient(err)
    }
}

/// The outcome of a fetch, in the shape [`crate::first_load::after_fetch`] wants.
pub fn outcome_of(result: &Result<ReleaseNotes, FetchError>) -> crate::first_load::NotesOutcome {
    match result {
        Ok(_) => crate::first_load::NotesOutcome::Fetched,
        Err(err) => err.outcome(),
    }
}

fn agent(api_base: &str) -> ureq::Agent {
    let mut builder = ureq::Agent::config_builder()
        .timeout_connect(Some(FETCH_TIMEOUT))
        .timeout_global(Some(FETCH_TIMEOUT))
        // ureq's default turns a non-2xx into an opaque error; dux inspects the
        // status itself so the message can name it (and call out a rate limit),
        // per "prefer explicit failure with context".
        .http_status_as_error(false);
    // ureq reads HTTP(S)_PROXY from the environment and does NOT exempt
    // loopback unless NO_PROXY says so. Nobody proxies a request to their own
    // machine, and honoring a corporate proxy for 127.0.0.1 would break both
    // the tests and any local-mirror setup, so clear it for loopback bases only.
    // A real https://api.github.com fetch keeps the environment proxy.
    if is_loopback_base(api_base) {
        builder = builder.proxy(None);
    }
    builder.build().into()
}

/// Whether `base` addresses this machine. String-level on purpose: no DNS
/// lookup happens here.
fn is_loopback_base(base: &str) -> bool {
    let authority = base
        .split_once("//")
        .map(|(_, rest)| rest)
        .unwrap_or(base)
        .split('/')
        .next()
        .unwrap_or_default();
    // Strip the port, then the IPv6 brackets. Order matters: `[::1]:8080` has
    // colons inside the host, so the bracket form must be handled explicitly
    // rather than by splitting on the last colon.
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split_once(']').map(|(h, _)| h).unwrap_or(rest)
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority)
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Fetches ONE release by its tag. BLOCKING: worker only.
///
/// **This is the automatic startup path.** The what's-new screen must describe
/// the version the user is actually running, so it asks for that tag by name. The
/// `/releases/latest` endpoint would be wrong here: a user who upgrades to v0.7.0
/// while v0.8.0 is already published would be shown v0.8.0's features, which they
/// do not have.
///
/// A tag with no published release is a definitive
/// [`FetchError::NoSuchRelease`], not a transient failure.
pub fn fetch_release_by_tag(api_base: &str, tag: &str) -> Result<ReleaseNotes, FetchError> {
    if !is_path_safe_tag(tag) {
        // Definitive: such a string cannot name a dux release, and re-asking is
        // pointless. Never interpolated into the URL, so it cannot rewrite the path.
        return Err(FetchError::NoSuchRelease {
            tag: tag.to_string(),
        });
    }
    let url = tag_release_endpoint(api_base, tag);
    fetch_release(api_base, &url, Some(tag))
}

/// Fetches and parses the repository's NEWEST release. BLOCKING: worker only.
///
/// **This serves the case where there is no running release to ask for**: a
/// development build (`DUX_DISPLAY_VERSION == "development"`) has no tag, so when
/// the user explicitly opens the release notes there, the newest published
/// release is the only sensible answer. The automatic startup path uses
/// [`fetch_release_by_tag`] instead.
///
/// Unauthenticated: GitHub serves this endpoint with no token at a 60
/// requests/hour per-IP limit, which is why [`load_or_fetch_tag`] caches.
pub fn fetch_latest(api_base: &str) -> Result<ReleaseNotes, FetchError> {
    let url = latest_release_endpoint(api_base);
    fetch_release(api_base, &url, None)
}

/// The shared request/parse path. `tag` is `Some` when the caller asked for one
/// specific release, which is what makes a 404 definitive rather than transient.
fn fetch_release(api_base: &str, url: &str, tag: Option<&str>) -> Result<ReleaseNotes, FetchError> {
    let mut response = agent(api_base)
        .get(url)
        .header("User-Agent", user_agent())
        .header("Accept", "application/vnd.github+json")
        // Pin the API version so a future default change cannot reshape the payload.
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .with_context(|| format!("failed to fetch dux release notes from {url}"))?;

    let status = response.status().as_u16();
    if status == 404 {
        // GitHub's definitive "there is no such release". Legitimate and common:
        // a locally built or not-yet-published tagged binary has no release page.
        return Err(FetchError::NoSuchRelease {
            tag: tag.unwrap_or("latest").to_string(),
        });
    }
    if !(200..300).contains(&status) {
        // 403/429 from the unauthenticated endpoint is almost always the 60
        // requests/hour per-IP limit, so say so rather than leaving the reader
        // guessing at a bare number. Everything here is retryable.
        let hint = if status == 403 || status == 429 {
            " (GitHub's unauthenticated API allows 60 requests per hour per IP; \
             dux caches the notes to stay well under it)"
        } else {
            ""
        };
        return Err(FetchError::Transient(anyhow!(
            "GitHub returned HTTP {status} for {url}{hint}"
        )));
    }

    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES as u64)
        .read_to_string()
        .with_context(|| format!("failed to read the release response from {url}"))?;

    let release: ApiRelease = serde_json::from_str(&body)
        .with_context(|| format!("{url} did not return a GitHub release payload"))?;
    if release.tag_name.trim().is_empty() {
        return Err(FetchError::Transient(anyhow!(
            "{url} returned a release with no tag_name"
        )));
    }
    Ok(from_api(release))
}

fn from_api(release: ApiRelease) -> ReleaseNotes {
    let parsed = parse_release_body(release.body.as_deref().unwrap_or_default());
    ReleaseNotes {
        version: release.tag_name,
        headline: parsed.headline,
        paragraphs: parsed.paragraphs,
        sections: parsed.sections,
        // Prefer the API's own link; fall back to the releases index when the
        // payload omits it or leaves it blank.
        html_url: release
            .html_url
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| crate::urls::RELEASES.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Caching
// ---------------------------------------------------------------------------

/// How long a cached copy is trusted before dux refetches.
///
/// Six hours. Live fetching is the point of the feature — a typo in the release
/// body gets fixed within hours of publishing, and a cache that outlived the fix
/// would pin the mistake on screen — so the window has to be short. Six hours
/// also keeps dux to at most four requests a day per machine against the
/// unauthenticated 60/hour per-IP limit, however many times dux is launched, and
/// it comfortably covers a single working session (the realistic case: launch,
/// glance at the notes, dismiss, relaunch a dozen times).
pub const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// The cache file inside the dux state directory (`DuxPaths::root`).
///
/// Derived, disposable state: deleting it costs one HTTP request.
pub fn cache_path(root: &Path) -> PathBuf {
    root.join("release_notes.json")
}

/// What the cache had to say about a version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheLookup {
    /// A hit for this version, within the TTL. Authoritative: make no request.
    Fresh(ReleaseNotes),
    /// A hit for this version, past the TTL. Refetch; fall back to this if the
    /// refetch fails.
    Stale(ReleaseNotes),
    /// No usable entry: absent, unreadable, corrupt, or for another version.
    Missing,
}

/// A single cached release. One entry, not a map: only the newest release is
/// ever fetched and only the running version is ever consulted, so a second row
/// could never be the one we want.
#[derive(Serialize, Deserialize)]
struct CacheFile {
    fetched_at: DateTime<Utc>,
    notes: ReleaseNotes,
}

/// Looks up `version` in the cache. Never fails: an unreadable or corrupt file
/// is a [`CacheLookup::Missing`], because the fallback (one HTTP request) is
/// cheaper than surfacing an error nobody can act on.
pub fn cached_notes(path: &Path, version: &str, ttl: Duration, now: DateTime<Utc>) -> CacheLookup {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return CacheLookup::Missing;
    };
    let Ok(file) = serde_json::from_str::<CacheFile>(&raw) else {
        return CacheLookup::Missing;
    };
    if file.notes.version != version {
        return CacheLookup::Missing;
    }
    // `to_std` errors on a negative span, which is a clock that moved backwards;
    // treat that as "age unknown" and refetch rather than trusting the entry.
    let fresh = (now - file.fetched_at)
        .to_std()
        .map(|age| age < ttl)
        .unwrap_or(false);
    if fresh {
        CacheLookup::Fresh(file.notes)
    } else {
        CacheLookup::Stale(file.notes)
    }
}

/// Writes `notes` to the cache, stamped `now`, replacing any previous entry.
pub fn write_cache(path: &Path, notes: &ReleaseNotes, now: DateTime<Utc>) -> Result<()> {
    let file = CacheFile {
        fetched_at: now,
        notes: notes.clone(),
    };
    let json = serde_json::to_string_pretty(&file)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

/// The whole startup path for ONE tag: cache first, network second, stale cache
/// as a last resort. BLOCKING: worker only.
///
/// `tag` is BOTH the cache key and what gets requested, which is the point: the
/// fetched notes always carry the tag that was asked for, so the entry written
/// here is the entry the next launch looks up and the TTL actually takes effect.
/// (An earlier version keyed on the running version but fetched
/// `/releases/latest`; whenever the newest published tag differed, every lookup
/// missed and dux refetched on every single launch.)
///
/// A definitive [`FetchError::NoSuchRelease`] is returned as-is and does NOT fall
/// back to a stale entry: there is no release, so there is nothing to be stale
/// about.
pub fn load_or_fetch_tag(
    api_base: &str,
    cache: &Path,
    tag: &str,
    ttl: Duration,
    now: DateTime<Utc>,
) -> Result<ReleaseNotes, FetchError> {
    let cached = cached_notes(cache, tag, ttl, now);
    if let CacheLookup::Fresh(notes) = cached {
        return Ok(notes);
    }
    match fetch_release_by_tag(api_base, tag) {
        Ok(notes) => {
            debug_assert_eq!(
                notes.version, tag,
                "the by-tag endpoint must return the tag we asked for, or the \
                 cache key and the stored tag would disagree"
            );
            // A failed cache write must not fail the fetch: the notes are in
            // hand and the only cost is refetching next time.
            if let Err(err) = write_cache(cache, &notes, now) {
                crate::logger::warn(&format!("failed to cache release notes: {err:#}"));
            }
            Ok(notes)
        }
        Err(err) => match cached {
            // Stale-but-matching beats showing nothing when the network is down.
            CacheLookup::Stale(notes) if !err.is_definitive() => Ok(notes),
            _ => Err(err),
        },
    }
}

/// THE entry point both surfaces call. BLOCKING: worker only.
///
/// - A real release build asks for **its own tag**, cached under `root`, so the
///   screen describes the version the user is running and links to that release.
/// - A **development build** has no tag to ask for, so it falls back to the newest
///   published release. This can only be reached by an explicit user action (a dev
///   build never auto-shows the what's-new screen), and it is deliberately
///   uncached: there is no stable key to file it under.
pub fn load_release_notes(root: &Path, running_version: &str) -> Result<ReleaseNotes, FetchError> {
    load_release_notes_from(crate::urls::GITHUB_API_BASE, root, running_version)
}

/// [`load_release_notes`] with the API base injected, for tests and for the web
/// server's integration suite.
///
/// This exists so the dev-build-versus-tag dispatch above lives in exactly ONE
/// place. A surface that hardcoded its own copy of that `if` (because it needed
/// to point a test at a local server) would silently keep the old behaviour the
/// next time the rule changed. Pass [`crate::urls::GITHUB_API_BASE`] for the real
/// thing; `load_release_notes` is the convenience wrapper that does.
///
/// BLOCKING: worker only.
pub fn load_release_notes_from(
    api_base: &str,
    root: &Path,
    running_version: &str,
) -> Result<ReleaseNotes, FetchError> {
    if running_version == crate::first_load::DEVELOPMENT_VERSION {
        return fetch_latest(api_base);
    }
    load_or_fetch_tag(
        api_base,
        &cache_path(root),
        running_version,
        CACHE_TTL,
        Utc::now(),
    )
}

/// The page to send a user to for `notes`, or the releases index when there are
/// no notes at all (a dev build, or a failed fetch).
pub fn notes_url(notes: Option<&ReleaseNotes>) -> String {
    match notes {
        Some(n) if !n.html_url.trim().is_empty() => n.html_url.clone(),
        _ => crate::urls::RELEASES.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../tests/fixtures/sample_release_notes.md");

    #[test]
    fn parses_headline_intro_and_sections_from_the_real_release_body() {
        let n = parse_release_body(SAMPLE);
        assert_eq!(n.headline, "Quieter plumbing, louder failures");
        assert_eq!(n.paragraphs.len(), 2, "two intro paragraphs: {n:#?}");
        assert!(n.paragraphs[0].starts_with("Version 0.6.0 is a tune-up release"));
        assert_eq!(n.sections.len(), 6, "six feature titles: {:#?}", n.sections);
        assert_eq!(n.sections[0], "Environment config for agents and terminals");
        assert_eq!(n.sections[5], "A website exists now");
    }

    #[test]
    fn stops_before_the_autogenerated_boilerplate() {
        let n = parse_release_body(SAMPLE);
        // Pin what the parse DID produce first: a parse that yielded nothing at
        // all would satisfy every absence check below without reading a line.
        assert_eq!(
            n.sections.len(),
            6,
            "the real feature sections parsed: {n:#?}"
        );
        assert_eq!(n.paragraphs.len(), 2, "the real intro prose parsed: {n:#?}");
        assert!(
            !n.sections.iter().any(|s| s.contains("Installation")),
            "must not reach the appended Installation section"
        );
        for p in &n.paragraphs {
            assert!(
                !p.contains("install.sh"),
                "install boilerplate leaked into the intro: {p}"
            );
        }
    }

    #[test]
    fn drops_code_fences_and_inline_markup() {
        let body = "## Title\n\nSome `code` and **bold** and a [link](https://x.dev).\n\n```toml\nkey = 1\n```\n\n### A feature\n";
        let n = parse_release_body(body);
        assert_eq!(n.paragraphs, vec!["Some code and bold and a link."]);
        assert_eq!(n.sections, vec!["A feature"]);
    }

    #[test]
    fn empty_body_yields_empty_notes() {
        assert_eq!(parse_release_body(""), ParsedBody::default());
    }

    #[test]
    fn stripping_markup_is_char_safe_on_multibyte_prose() {
        // Box-drawing, CJK, and emoji would panic under byte slicing.
        let out = strip_inline_markup("**環境変数** ░██ `コード` [リンク](https://x.dev) 🦆_ok_");
        assert_eq!(out, "環境変数 ░██ コード リンク 🦆ok");
    }

    #[test]
    fn malformed_link_syntax_never_runs_off_the_end_of_the_string() {
        assert_eq!(strip_inline_markup("[unclosed"), "[unclosed");
        assert_eq!(
            strip_inline_markup("[text] not a link"),
            "[text] not a link"
        );
        // An opened-but-unterminated target (`[t](` with no `)`) swallows the
        // rest of the line and keeps only the link text. This is the prototype
        // parser's behavior, ported unchanged, and it is documented rather than
        // "fixed": it can only be reached by malformed Markdown in a release
        // body, and the important property — no panic, no byte slicing — holds.
        assert_eq!(strip_inline_markup("[t]("), "t");
        assert_eq!(strip_inline_markup("see [t](http and more"), "see t");
    }

    /// The ORIGINAL quadratic `strip_inline_markup`, kept verbatim in the tests
    /// as the byte-identity oracle for the cached-cursor rewrite. It rescans
    /// forward for `]` from scratch at every `[`, which is exactly the cost the
    /// rewrite removes; the OUTPUT must stay the same character for character.
    fn strip_inline_markup_reference(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '*' | '`' | '_' => {}
                '[' => {
                    let mut j = i + 1;
                    let mut text = String::new();
                    while j < chars.len() && chars[j] != ']' {
                        text.push(chars[j]);
                        j += 1;
                    }
                    if j < chars.len() && chars.get(j + 1) == Some(&'(') {
                        let mut k = j + 2;
                        while k < chars.len() && chars[k] != ')' {
                            k += 1;
                        }
                        out.push_str(&text);
                        i = k + 1;
                        continue;
                    }
                    out.push('[');
                }
                c => out.push(c),
            }
            i += 1;
        }
        out
    }

    #[test]
    fn stripping_markup_is_byte_identical_to_the_pre_cursor_implementation() {
        // Every shape the cached forward cursor could plausibly disagree on: a
        // well-formed link, brackets nested inside a link's text, unmatched `[`
        // runs with no `]` at all, many `[` resolved by ONE late `]` that is not
        // a link, many `[` followed by `](` with no closing `)`, a `]` before
        // any `[`, adjacent links, and the real release body fixture.
        let cases: Vec<String> = vec![
            String::new(),
            "no markup at all".to_string(),
            "[text](https://x.dev)".to_string(),
            "a [nested [inner] text](https://x.dev) b".to_string(),
            "[[[[[".to_string(),
            "[a[b]c".to_string(),
            "[text] not a link".to_string(),
            "] stray close [then](url)".to_string(),
            "[a](1)[b](2)[c](3)".to_string(),
            "[t](".to_string(),
            "see [t](http and more".to_string(),
            "**環境変数** ░██ `コード` [リンク](https://x.dev) 🦆_ok_".to_string(),
            "[".repeat(64) + "]",
            "[".repeat(64) + "](",
            "[".repeat(64) + "](x) tail",
            "[".repeat(64),
            format!("{}]{}", "[".repeat(32), "[".repeat(32)),
            SAMPLE.to_string(),
        ];
        for case in &cases {
            assert_eq!(
                strip_inline_markup(case),
                strip_inline_markup_reference(case),
                "output drifted from the reference for {case:?}"
            );
        }
    }

    #[test]
    fn stripping_markup_is_linear_not_quadratic() {
        // A run of unmatched `[` is the adversarial shape: the old code rescanned
        // to the end of the string for every one of them. Measured at 64,000
        // characters in an UNOPTIMIZED test build (which is how this test runs):
        // 27.7s before the forward cursor, 2.8ms after; 1,000,000 characters now
        // take 45ms. In release it is 0.45ms and 7ms. The 300ms bound is ~100x
        // the measured linear time — generous enough not to flake on slow CI —
        // and ~90x under the old quadratic time, which blew past it by four
        // orders of magnitude.
        let input = "[".repeat(64_000);
        let start = std::time::Instant::now();
        let out = strip_inline_markup(&input);
        let elapsed = start.elapsed();
        assert_eq!(out.chars().count(), 64_000, "every `[` is kept literally");
        assert!(
            elapsed < std::time::Duration::from_millis(300),
            "strip_inline_markup looks quadratic again: 64k unmatched brackets took {elapsed:?}"
        );
    }

    #[test]
    fn a_body_that_is_only_boilerplate_yields_nothing_rather_than_the_boilerplate() {
        let body = "## What's Changed\n* a PR by @someone\n\n## Installation\n\nbrew install\n";
        let n = parse_release_body(body);
        // The FIRST `## ` is taken as the headline (dux's own releases always
        // lead with a real one); what matters is that nothing after the second
        // heading leaks in.
        assert_eq!(n.headline, "What's Changed");
        assert!(n.sections.is_empty());
        assert!(
            !n.paragraphs.iter().any(|p| p.contains("brew install")),
            "{n:#?}"
        );
    }

    // -----------------------------------------------------------------------
    // The shapes a human actually publishes.
    //
    // The parser is a two-level heading reader, not a Markdown parser, so most
    // of these degrade rather than fail. Each test says what the screen ends up
    // showing, because that is the thing that breaks for every user who updates.
    // The format the parser needs is stated in CONTRIBUTING.md; these tests are
    // what keeps that statement true.
    // -----------------------------------------------------------------------

    #[test]
    fn a_body_with_no_headings_at_all_becomes_intro_prose() {
        // The commonest human shape: someone types two sentences and publishes.
        // There is no headline, so the screen falls back to a generic title, and
        // the prose is still shown.
        let n = parse_release_body("Fixes the thing that broke.\n\nAlso faster now.\n");
        assert_eq!(n.headline, "");
        assert_eq!(
            n.paragraphs,
            vec!["Fixes the thing that broke.", "Also faster now."]
        );
        assert!(n.sections.is_empty());
        assert!(n.has_renderable_body(), "prose is worth rendering");
    }

    #[test]
    fn a_body_that_is_only_a_headline_leaves_the_screen_with_nothing_but_a_title() {
        // Very reachable: GitHub APPENDS `## What's Changed` and dux's release
        // workflow APPENDS `## Installation` after it, so a one-line human
        // headline plus that boilerplate parses to a headline and nothing else.
        // Both screens must say so rather than showing a blank body.
        let n = parse_release_body("## Quieter plumbing\n\n## What's Changed\n* a PR\n");
        assert_eq!(n.headline, "Quieter plumbing");
        assert!(n.paragraphs.is_empty(), "{n:#?}");
        assert!(n.sections.is_empty(), "{n:#?}");
        assert!(
            !n.has_renderable_body(),
            "a headline alone is not a body; the screen owes the reader an explanation"
        );
    }

    #[test]
    fn a_heading_at_the_wrong_level_is_read_as_prose_hashes_and_all() {
        // `#` and `####` are not the two levels the parser knows, so they are
        // neither headline nor section. Nothing is lost, but the `#` characters
        // are shown literally, which is visibly wrong and is exactly why the
        // required format is written down.
        let n = parse_release_body("# Big title\n\nSome prose.\n\n#### Deep thing\n");
        assert_eq!(n.headline, "", "an h1 is not the headline");
        assert_eq!(
            n.paragraphs,
            vec!["# Big title", "Some prose.", "#### Deep thing"]
        );
        assert!(n.sections.is_empty());
    }

    #[test]
    fn a_heading_with_no_space_after_the_hashes_is_read_as_prose() {
        // `strip_prefix("## ")` requires the space. `##Title` is legal Markdown
        // to some renderers and is not recognized here.
        let n = parse_release_body("##Title\n\n###Feature\n");
        assert_eq!(n.headline, "");
        assert_eq!(n.paragraphs, vec!["##Title", "###Feature"]);
        assert!(n.sections.is_empty());
    }

    #[test]
    fn a_body_that_is_only_a_link_keeps_the_link_text_and_drops_the_target() {
        let n = parse_release_body("[Read the full notes](https://example.invalid/notes)\n");
        assert_eq!(n.paragraphs, vec!["Read the full notes"]);
        assert_eq!(n.headline, "");
        assert!(n.has_renderable_body());
    }

    #[test]
    fn a_body_that_is_only_whitespace_is_the_same_as_an_empty_one() {
        for raw in ["", "\n", "   \n\t\n  ", "\r\n\r\n"] {
            let n = parse_release_body(raw);
            assert_eq!(n, ParsedBody::default(), "{raw:?} should parse to nothing");
            assert!(!n.has_renderable_body(), "{raw:?}");
        }
    }

    #[test]
    fn sections_before_any_top_heading_hand_the_headline_to_the_boilerplate() {
        // A REAL trap, and the reason the required format has to be written down
        // rather than assumed. When the human forgets the leading `## ` line, the
        // FIRST `## ` in the file is GitHub's own "What's Changed", so that
        // becomes the headline. The parse does NOT stop there (the `break` fires
        // on the SECOND top heading, not the first), so any `### ` inside the
        // machine-written tail is merged into the feature list as though the human
        // had written it. That is measured, not inferred: an earlier version of
        // this test asserted the tail was dropped and was wrong.
        let body = "\
### First feature
Its description.

### Second feature
Its description.

## What's Changed
* a PR by @someone

### Bumped a dependency
";
        let n = parse_release_body(body);
        assert_eq!(
            n.headline, "What's Changed",
            "the boilerplate heading is promoted to the headline"
        );
        assert_eq!(
            n.sections,
            vec!["First feature", "Second feature", "Bumped a dependency"],
            "boilerplate subsections are merged into the feature list: {n:#?}"
        );
        // The commit bullets are dropped HERE, but not as a general property:
        // prose is only collected before the first feature section, and this
        // fixture happens to have one. Remove the `### ` lines and the same
        // bullets become intro prose. See
        // `a_body_of_only_generated_notes_renders_the_commit_list_as_prose`.
        assert!(
            !n.paragraphs.iter().any(|p| p.contains("a PR by")),
            "{n:#?}"
        );
    }

    /// The shape a release gets when the human writes NOTHING and only ticks
    /// "Generate release notes": no `## ` of their own, so the boilerplate heading
    /// becomes the title, and with no `### ` anywhere the commit bullets are
    /// collected as INTRO PROSE. They are also merged, because consecutive
    /// non-blank lines join into one paragraph, so the screen renders the commit
    /// list as a single run-on sentence with the `*` markers stripped.
    ///
    /// Measured and pinned rather than fixed: it is machine text rendered as human
    /// prose, it is ugly, and it is exactly what the format rules in
    /// CONTRIBUTING.md exist to prevent. Recorded here so the "bullets are
    /// dropped" claim above cannot be read as a general one again.
    ///
    /// The exact strings below are MEASURED. `strip_inline_markup` eats the `*`
    /// bullet marker along with the emphasis markers, so each line keeps the space
    /// that followed it and the join leaves a double space in the middle. A first
    /// draft of this test predicted the markers would survive and was wrong.
    #[test]
    fn a_body_of_only_generated_notes_renders_the_commit_list_as_prose() {
        let body = "\
## What's Changed
* First PR by @someone in https://example.invalid/1
* Second PR by @other in https://example.invalid/2

**Full Changelog**: https://example.invalid/compare
";
        let n = parse_release_body(body);
        assert_eq!(n.headline, "What's Changed");
        assert!(n.sections.is_empty(), "{n:#?}");
        assert_eq!(
            n.paragraphs,
            vec![
                " First PR by @someone in https://example.invalid/1  \
                 Second PR by @other in https://example.invalid/2",
                "Full Changelog: https://example.invalid/compare",
            ],
            "two separate bullets merged into one run-on paragraph: {n:#?}"
        );
        // ...and it counts as a body, so the screen shows this rather than the
        // no-notes explanation. Degraded, not blank.
        assert!(n.has_renderable_body());
    }

    #[test]
    fn a_leading_top_heading_is_what_protects_the_feature_list_from_the_boilerplate() {
        // The same body as above with the one required line restored. This is the
        // format CONTRIBUTING.md asks for, and it is the difference between a
        // correct screen and a wrong one.
        let body = "\
## The real headline

### First feature
Its description.

## What's Changed
* a PR by @someone

### Bumped a dependency
";
        let n = parse_release_body(body);
        assert_eq!(n.headline, "The real headline");
        assert_eq!(n.sections, vec!["First feature"]);
    }

    #[test]
    fn an_unterminated_code_fence_swallows_the_rest_of_the_body_without_panicking() {
        // A fence opened and never closed flips `in_code` on and nothing turns it
        // off, so everything after it is dropped. Degradation, not a crash, and
        // the headline before it still survives.
        let n = parse_release_body("## Title\n\nIntro.\n\n```toml\nkey = 1\n\n### Lost feature\n");
        assert_eq!(n.headline, "Title");
        assert_eq!(n.paragraphs, vec!["Intro."]);
        assert!(n.sections.is_empty(), "{n:#?}");
    }

    #[test]
    fn prose_after_the_first_feature_section_is_deliberately_dropped() {
        // The screen shows feature TITLES only; the bodies belong on the release
        // page. Pinned because it looks like a bug from the outside.
        let n = parse_release_body(
            "## Title\n\nIntro.\n\n### A feature\n\nThe long explanation.\n\n### Another\n",
        );
        assert_eq!(n.paragraphs, vec!["Intro."]);
        assert_eq!(n.sections, vec!["A feature", "Another"]);
    }

    #[test]
    fn a_very_long_body_is_parsed_whole_and_left_for_the_screen_to_scroll() {
        // No truncation here on purpose: both screens scroll, and silently
        // dropping half a release's features would be worse than a long scroll.
        // The cap that matters is on the HTTP read (`MAX_BODY_BYTES`).
        let mut body = String::from("## Title\n\n");
        for i in 0..500 {
            body.push_str(&format!("Paragraph {i} with some prose in it.\n\n"));
            body.push_str(&format!("### Feature number {i}\n\n"));
        }
        let n = parse_release_body(&body);
        assert_eq!(n.headline, "Title");
        // Only prose BEFORE the first `###` is kept, so exactly one paragraph.
        assert_eq!(n.paragraphs, vec!["Paragraph 0 with some prose in it."]);
        assert_eq!(n.sections.len(), 500);
        assert_eq!(n.sections[499], "Feature number 499");
    }

    #[test]
    fn a_headline_or_section_that_is_only_markup_collapses_to_nothing_rather_than_panicking() {
        // `strip_inline_markup` can empty a heading out entirely. An empty
        // headline is handled (the screens fall back), and an empty SECTION is a
        // blank bullet, which is ugly but harmless. Pinned so it stays harmless.
        let n = parse_release_body("## **__**\n\n### ``\n");
        assert_eq!(n.headline, "");
        assert_eq!(n.sections, vec![""]);
    }

    #[test]
    fn a_body_of_only_crlf_lines_parses_the_same_as_lf() {
        // GitHub stores release bodies with CRLF line endings. `str::lines`
        // strips the `\r`, and every branch trims, so the two must agree.
        let lf = parse_release_body("## Title\n\nIntro.\n\n### A feature\n");
        let crlf = parse_release_body("## Title\r\n\r\nIntro.\r\n\r\n### A feature\r\n");
        assert_eq!(lf, crlf);
    }

    #[test]
    fn has_renderable_body_is_true_exactly_when_there_is_something_under_the_title() {
        // The predicate both screens use to decide whether to show the
        // "no notes" explanation, so its boundaries are worth stating outright.
        assert!(!ReleaseNotes::default().has_renderable_body());
        assert!(
            !ReleaseNotes {
                headline: "Only a title".to_string(),
                ..Default::default()
            }
            .has_renderable_body(),
            "a headline is rendered as the dialog title, not as the body"
        );
        assert!(
            ReleaseNotes {
                paragraphs: vec!["prose".to_string()],
                ..Default::default()
            }
            .has_renderable_body()
        );
        assert!(
            ReleaseNotes {
                sections: vec!["a feature".to_string()],
                ..Default::default()
            }
            .has_renderable_body()
        );
        // Whitespace-only entries are not content. A release body that produced
        // one empty section used to count as a body and render a lone blank
        // bullet with no explanation.
        assert!(
            !ReleaseNotes {
                sections: vec![String::new(), "   ".to_string()],
                paragraphs: vec!["  ".to_string()],
                ..Default::default()
            }
            .has_renderable_body()
        );
    }

    /// THE SHAPE THE RELEASE PIPELINE ITSELF PRODUCES. `.github/workflows/release.yml`
    /// appends a horizontal rule and then `## Installation`, so a release whose
    /// human-written part is a single `## ` line leaves the parser a headline and
    /// that rule. The rule is not a heading, so it was collected as intro PROSE
    /// and the screen rendered a title above a literal `---`: the empty panel with
    /// one extra character.
    #[test]
    fn the_separator_the_release_pipeline_appends_is_not_a_body() {
        let body = "## Quieter plumbing\n\n---\n\n## Installation\n\nbrew install dux\n";
        let n = parse_release_body(body);
        assert_eq!(n.headline, "Quieter plumbing");
        assert!(
            !n.has_renderable_body(),
            "a horizontal rule is not notes; the screen owes the reader an explanation: {n:#?}"
        );
    }

    /// The same shape with GitHub's generated section present as well, which is
    /// what a release with "Generate release notes" ticked actually looks like.
    #[test]
    fn a_headline_a_rule_and_the_generated_section_still_leave_no_body() {
        let body = "## Quieter plumbing\n\n## What's Changed\n* a PR by @someone\n\n---\n\n## Installation\n";
        let n = parse_release_body(body);
        assert!(!n.has_renderable_body(), "{n:#?}");
    }

    /// Bodies a human would call empty that used to count as renderable. Each one
    /// put the screen back where it started: a title over something invisible or
    /// meaningless, with no explanation and nothing to do.
    #[test]
    fn bodies_that_look_empty_to_a_human_are_treated_as_empty() {
        let cases = [
            ("a horizontal rule", "## T\n\n---\n"),
            ("an asterisk rule", "## T\n\n***\n"),
            ("an underscore rule", "## T\n\n___\n"),
            ("a spaced rule", "## T\n\n- - -\n"),
            ("an HTML line break", "## T\n\n<br>\n"),
            ("a self-closed HTML line break", "## T\n\n<br />\n"),
            ("a zero-width space", "## T\n\n\u{200B}\n"),
            ("a byte-order mark", "## T\n\n\u{FEFF}\n"),
            ("a next-line character", "## T\n\n\u{0085}\n"),
            (
                "an HTML comment",
                "## T\n\n<!-- release notes go here -->\n",
            ),
            ("an unterminated HTML comment", "## T\n\n<!-- oops\n"),
        ];
        for (what, body) in cases {
            let n = parse_release_body(body);
            assert!(!n.has_renderable_body(), "{what} counted as a body: {n:#?}");
        }
    }

    /// ...and the guard must not swallow real notes that merely contain one of
    /// those characters.
    #[test]
    fn a_body_that_only_contains_a_rule_or_a_break_still_renders() {
        for body in [
            "## T\n\nBefore --- after.\n",
            "## T\n\nLine one<br>line two.\n",
            "## T\n\nA note <!-- with an aside --> that still reads.\n",
            "## T\n\n### A feature\n",
        ] {
            let n = parse_release_body(body);
            assert!(n.has_renderable_body(), "{body:?} -> {n:#?}");
        }
    }

    /// The two surfaces must agree on what counts as invisible, or the same
    /// release shows a body in the browser and the no-notes explanation in the
    /// terminal. They cannot share code, so they share an explicit code-point set
    /// instead: `String::trim` and JavaScript's `String.prototype.trim` do NOT
    /// trim the same characters (measured: Rust trims U+0085 and JS does not; JS
    /// trims U+FEFF and Rust does not), so neither language's own trim can be the
    /// definition.
    #[test]
    fn the_invisible_set_covers_what_each_languages_own_trim_would_miss() {
        // Rust's `trim` misses this one; the browser's would have emptied it.
        assert!(is_invisible_char('\u{FEFF}'), "byte-order mark");
        // JavaScript's `trim` misses this one; Rust's would have emptied it.
        assert!(is_invisible_char('\u{0085}'), "next line");
        // Zero-width characters render as literally nothing on both surfaces.
        for c in ['\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}'] {
            assert!(is_invisible_char(c), "{:04X}", c as u32);
        }
        // ...and ordinary text is not invisible.
        for c in ['a', '-', '#', '日', '🦆'] {
            assert!(!is_invisible_char(c), "{c}");
        }
    }

    /// CROSS-LANGUAGE PIN, in the same idiom as `editor_keys_match_the_typescript_menu`:
    /// the web mirrors this module's predicate and its explanation string, and a
    /// TS surface cannot import a Rust const. So a Rust test READS the TypeScript
    /// and fails when the two drift. Skips when the web tree isn't present (a
    /// crate build outside the workspace).
    #[test]
    fn the_web_mirror_of_the_no_notes_surface_has_not_drifted() {
        let ts_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../dux-web/web/src/lib/releaseNotes.ts");
        let Ok(source) = std::fs::read_to_string(&ts_path) else {
            eprintln!("skipping: {} not present", ts_path.display());
            return;
        };

        // 1. The explanation string, character for character.
        let quoted = format!("\"{NO_NOTES_EXPLANATION}\"");
        assert!(
            source.contains(&quoted),
            "releaseNotes.ts no longer carries NO_NOTES_EXPLANATION verbatim; \
             reword both sides in the same change. Expected to find {quoted}"
        );

        // 2. The invisible code-point set, as a set rather than as text: every
        //    code point the web calls invisible, this module must too, and the
        //    other way round.
        // The declaration, not the mentions of it in the comments. The formatter
        // is free to wrap after the `=`, so take everything up to the first quote
        // rather than assuming the literal is on the same line.
        let declared = source
            .split("INVISIBLE_CODE_POINTS =")
            .nth(1)
            .and_then(|rest| rest.split_once('"'))
            .and_then(|(_, rest)| rest.split('"').next())
            .expect("releaseNotes.ts must declare INVISIBLE_CODE_POINTS");
        let mut from_ts: Vec<u32> = Vec::new();
        for range in declared.split(',') {
            let mut ends = range.split('-').map(|hex| {
                u32::from_str_radix(hex.trim(), 16)
                    .unwrap_or_else(|_| panic!("not a code point: {hex:?}"))
            });
            let start = ends.next().expect("a range needs a start");
            let end = ends.next().unwrap_or(start);
            from_ts.extend(start..=end);
        }
        from_ts.sort_unstable();

        let mut from_rust: Vec<u32> = (0u32..=0x10FFFF)
            .filter(|point| char::from_u32(*point).is_some_and(is_invisible_char))
            .collect();
        from_rust.sort_unstable();

        assert_eq!(
            from_rust, from_ts,
            "the invisible-character set has drifted between release_notes.rs and \
             releaseNotes.ts, so the same release body can show a body on one \
             surface and the no-notes explanation on the other"
        );
    }

    /// The other half of the cross-language pin, and the half that catches a
    /// divergence the code-point comparison above cannot see: the two surfaces can
    /// hold the same invisible SET and still disagree, because each `<br>` matcher
    /// used to ask its own language what whitespace is, and because the web ran
    /// three sequential passes where this module runs one left-to-right.
    ///
    /// Both surfaces read the SAME fixture file, so a fixture case only passes when
    /// both agree with it (`releaseNotes.test.ts` reads it too, by relative path).
    #[test]
    fn the_two_surfaces_agree_on_every_shared_release_body_fixture() {
        let cases = shared_cross_language_cases();
        assert!(
            cases.len() >= 10,
            "the shared fixture lost its cases: {}",
            cases.len()
        );
        for case in cases {
            let what = case["what"].as_str().expect("every case names itself");
            let entry = case["entry"].as_str().expect("every case has an entry");
            let want = case["renderable"]
                .as_bool()
                .expect("every case says what it must answer");
            assert_eq!(
                entry_is_renderable(entry),
                want,
                "{what}: {entry:?} (visible remainder {:?})",
                strip_invisible_markup(entry)
            );
        }
    }

    /// The shared fixture, or an empty list when the file is missing (a crate build
    /// outside the workspace), in the same idiom as the TypeScript-reading test
    /// above.
    fn shared_cross_language_cases() -> Vec<serde_json::Value> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/release_notes_cross_language.json");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let parsed: serde_json::Value =
            serde_json::from_str(&source).expect("the shared fixture must be valid JSON");
        parsed["cases"]
            .as_array()
            .expect("the shared fixture must hold a `cases` array")
            .clone()
    }

    #[test]
    fn no_release_body_shape_makes_the_parser_panic() {
        // A blunt guard over the whole shape space, including the byte-slicing
        // hazards: multi-byte punctuation, lone surrogates' worth of emoji, very
        // long single lines, and heading markers with nothing after them.
        let shapes = [
            "",
            "#",
            "##",
            "## ",
            "###",
            "### ",
            "#### ",
            "```",
            "```\n```",
            "## \u{1F986}\n### \u{65E5}\u{672C}\u{8A9E}\n",
            "[",
            "]",
            "[](",
            "[]()",
            "## [](\n",
            "\0",
            "## Title\u{0}\n\u{7f}",
            "---\n***\n",
            "* a\n* b\n",
            "> quoted\n",
            "| a | b |\n|---|---|\n",
            "<!-- comment -->\n",
            "<h2>html heading</h2>\n",
        ];
        for shape in shapes {
            let n = parse_release_body(shape);
            // Touch every field so nothing is lazily unevaluated.
            let _ = (n.headline.len(), n.paragraphs.len(), n.sections.len());
            let _ = n.has_renderable_body();
        }
        // ...and one pathologically long single line.
        let long = "a".repeat(200_000);
        let _ = parse_release_body(&long);
    }

    #[test]
    fn the_endpoint_is_the_unauthenticated_latest_release_path() {
        assert_eq!(
            latest_release_endpoint(crate::urls::GITHUB_API_BASE),
            "https://api.github.com/repos/patrickdappollonio/dux/releases/latest"
        );
        // A trailing slash on an injected base must not double up.
        assert_eq!(
            latest_release_endpoint("http://127.0.0.1:1234/"),
            "http://127.0.0.1:1234/repos/patrickdappollonio/dux/releases/latest"
        );
    }

    #[test]
    fn the_by_tag_endpoint_names_the_running_version() {
        assert_eq!(
            tag_release_endpoint(crate::urls::GITHUB_API_BASE, "v0.6.0"),
            "https://api.github.com/repos/patrickdappollonio/dux/releases/tags/v0.6.0"
        );
        assert_eq!(
            tag_release_endpoint("http://127.0.0.1:1234/", "v0.6.0"),
            "http://127.0.0.1:1234/repos/patrickdappollonio/dux/releases/tags/v0.6.0"
        );
    }

    #[test]
    fn only_path_safe_tags_are_ever_interpolated_into_a_url() {
        for ok in ["v0.6.0", "0.6.0", "v1.0.0-rc.1", "v1.0.0+build_2", "V2"] {
            assert!(is_path_safe_tag(ok), "{ok} should be accepted");
        }
        // A `/` or `..` would rewrite the request path; `?`/`#` would truncate it.
        for bad in [
            "",
            "../../users/octocat",
            "release/1.0",
            "v1.0?x=1",
            "v1.0#frag",
            "v1 0",
            "v1.0%2f",
        ] {
            assert!(!is_path_safe_tag(bad), "{bad:?} must be refused");
        }
    }

    #[test]
    fn an_unsafe_tag_is_refused_definitively_and_without_a_request() {
        // No network involved: the guard fires before any URL is built.
        let err = fetch_release_by_tag("http://127.0.0.1:1", "../../etc/passwd")
            .expect_err("must be refused");
        assert!(err.is_definitive());
        assert_eq!(
            err.outcome(),
            crate::first_load::NotesOutcome::NoSuchRelease
        );
    }

    #[test]
    fn fetch_errors_map_onto_the_two_gate_outcomes_and_nothing_else() {
        let missing = FetchError::NoSuchRelease {
            tag: "v9.9.9".to_string(),
        };
        assert!(missing.is_definitive());
        assert_eq!(
            missing.outcome(),
            crate::first_load::NotesOutcome::NoSuchRelease
        );
        // The message explains itself instead of showing a bare status code.
        assert!(missing.to_string().contains("v9.9.9"), "{missing}");
        assert!(
            missing.to_string().contains("no published release"),
            "{missing}"
        );

        let transient = FetchError::Transient(anyhow!("connection refused"));
        assert!(!transient.is_definitive());
        assert_eq!(
            transient.outcome(),
            crate::first_load::NotesOutcome::TemporarilyUnavailable
        );
        assert!(transient.to_string().contains("connection refused"));
    }

    #[test]
    fn outcome_of_reads_a_success_as_fetched() {
        let ok: Result<ReleaseNotes, FetchError> = Ok(ReleaseNotes::default());
        assert_eq!(outcome_of(&ok), crate::first_load::NotesOutcome::Fetched);
        let err: Result<ReleaseNotes, FetchError> = Err(FetchError::NoSuchRelease {
            tag: "v1".to_string(),
        });
        assert_eq!(
            outcome_of(&err),
            crate::first_load::NotesOutcome::NoSuchRelease
        );
    }

    #[test]
    fn the_user_agent_identifies_dux_because_github_rejects_requests_without_one() {
        let ua = user_agent();
        assert!(ua.starts_with("dux/"), "{ua}");
        assert!(ua.contains(crate::display_version()), "{ua}");
    }

    #[test]
    fn only_loopback_bases_bypass_the_environment_proxy() {
        for base in [
            "http://127.0.0.1:8080",
            "http://127.0.0.1:8080/x",
            "http://localhost:9",
            "http://[::1]:8080",
        ] {
            assert!(is_loopback_base(base), "{base} should be loopback");
        }
        for base in [
            crate::urls::GITHUB_API_BASE,
            "https://api.github.com/",
            "https://192.168.1.10:8080",
            "https://evil.localhost.example.com",
        ] {
            assert!(!is_loopback_base(base), "{base} must keep the proxy");
        }
    }

    #[test]
    fn notes_url_prefers_the_api_link_and_otherwise_the_releases_index() {
        let notes = ReleaseNotes {
            html_url: "https://example.invalid/tag/v1".to_string(),
            ..Default::default()
        };
        assert_eq!(notes_url(Some(&notes)), "https://example.invalid/tag/v1");
        assert_eq!(notes_url(None), crate::urls::RELEASES);
        // A release payload with a blank link falls back rather than linking to "".
        assert_eq!(
            notes_url(Some(&ReleaseNotes {
                html_url: "  ".to_string(),
                ..Default::default()
            })),
            crate::urls::RELEASES
        );
    }

    #[test]
    fn a_release_body_becomes_release_notes_with_the_tag_as_the_version() {
        let notes = from_api(ApiRelease {
            tag_name: "v0.6.0".to_string(),
            body: Some(SAMPLE.to_string()),
            html_url: Some("https://example.invalid/v0.6.0".to_string()),
        });
        assert_eq!(notes.version, "v0.6.0");
        assert_eq!(notes.headline, "Quieter plumbing, louder failures");
        assert_eq!(notes.sections.len(), 6);
        assert_eq!(notes.html_url, "https://example.invalid/v0.6.0");

        // A release with no body at all is empty, not an error: the screen can
        // still show the title and a link.
        let bare = from_api(ApiRelease {
            tag_name: "v0.6.1".to_string(),
            body: None,
            html_url: None,
        });
        assert_eq!(bare.version, "v0.6.1");
        assert!(bare.headline.is_empty());
        assert_eq!(bare.html_url, crate::urls::RELEASES);
    }

    #[test]
    fn the_cache_ttl_is_short_enough_to_pick_up_an_edited_release_body() {
        // Live fetching is the point of the feature; a day-long TTL would pin a
        // typo on screen long after it was fixed.
        assert!(
            CACHE_TTL <= Duration::from_secs(12 * 60 * 60),
            "{CACHE_TTL:?}"
        );
        // ...and long enough that repeated launches in one session don't refetch.
        assert!(CACHE_TTL >= Duration::from_secs(60 * 60), "{CACHE_TTL:?}");
    }

    #[test]
    fn the_cache_file_sits_beside_the_other_dux_state() {
        assert_eq!(
            cache_path(Path::new("/home/ada/.config/dux")),
            Path::new("/home/ada/.config/dux/release_notes.json")
        );
    }

    #[test]
    fn a_cache_entry_stamped_in_the_future_is_treated_as_stale_not_fresh() {
        // A clock that moved backwards must not make an entry immortal.
        let tmp = tempfile::tempdir().unwrap();
        let path = cache_path(tmp.path());
        let now = Utc::now();
        let notes = ReleaseNotes {
            version: "v1".to_string(),
            ..Default::default()
        };
        write_cache(&path, &notes, now + chrono::Duration::hours(5)).unwrap();
        assert!(matches!(
            cached_notes(&path, "v1", CACHE_TTL, now),
            CacheLookup::Stale(_)
        ));
    }
}

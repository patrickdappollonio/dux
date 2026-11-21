//! Who an HTTP request asks as when it would change a row of the agents list
//! that a device drives, and the one answer every such refusal gets.
//!
//! [`identify`] wraps every route. It works out the asker once per request,
//! the pages the request names ([`PAGE_HEADER`]: the ids those pages' terminal
//! sockets opened with) and whether the request said to go ahead over
//! everybody (`force_connected=true`), and keeps it in a task local for the
//! length of the request. The engine handle reads
//! it for every change it sends ([`RowAsker::current`]), so no route passes it
//! along and none can leave it out; the question itself is the engine's
//! ([`dux_core::engine::row_guard`]). A refusal is written back into the asker,
//! and [`identify`] answers it as `423 Locked` with the engine's sentence,
//! whatever the route made of the error.
//!
//! Outside a request there is no asker: a change sent from there is judged as
//! a client naming no page, which a driven row refuses.

use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use dux_core::pty_owners::PageId;

use crate::rest_common::{ForceConnectedQuery, RouteRejection};

/// Names the pages a request acts as, comma separated: the sender's own, and
/// for an editor tab the page that opened it. A page id is a capability, the
/// same one a terminal socket presents as `?page=`.
pub(crate) const PAGE_HEADER: &str = "x-dux-page";

/// The most pages one request may name; the rest are not read.
const MAX_PAGES: usize = 4;

tokio::task_local! {
    static ASKING: RowAsker;
}

/// Who is asking for a change to a row, and where its refusal is left.
#[derive(Clone, Debug, Default)]
pub(crate) struct RowAsker {
    /// The pages the request names, empty for a client that names none.
    pages: Vec<PageId>,
    /// Whether the request said to go ahead over everybody.
    force: bool,
    refused: Arc<Mutex<Option<String>>>,
}

impl RowAsker {
    /// The asker of the request this task is answering, or a client with no
    /// connection when it is answering none.
    pub(crate) fn current() -> Self {
        ASKING.try_with(Clone::clone).unwrap_or_default()
    }

    /// The asker a terminal socket of `page` is: that page, or a client
    /// naming none. A socket has no request to say more, so it never forces.
    pub(crate) fn of_socket(page: Option<PageId>) -> Self {
        Self {
            pages: page.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Who the engine judges this asker's change to a driven row as.
    pub(crate) fn asker(&self) -> dux_core::attachments::Asker {
        if self.pages.is_empty() {
            dux_core::attachments::Asker::Nobody
        } else {
            dux_core::attachments::Asker::Pages(self.pages.clone())
        }
    }

    /// Whether the request said to go ahead over everybody.
    pub(crate) fn forces(&self) -> bool {
        self.force
    }

    /// The engine refused this asker's change with `sentence`. The first
    /// refusal of a request is the one it is answered with.
    pub(crate) fn refuse(&self, sentence: String) {
        self.refusal().get_or_insert(sentence);
    }

    /// Reword the refusal this request will be answered with, when it has one:
    /// for a route that had applied part of the request before the refusal,
    /// which the engine's sentence cannot know. Answers whether there was one.
    pub(crate) fn amend_refusal(&self, reword: impl FnOnce(&str) -> String) -> bool {
        let mut refusal = self.refusal();
        match refusal.as_deref().map(reword) {
            Some(reworded) => {
                *refusal = Some(reworded);
                true
            }
            None => false,
        }
    }

    fn refusal(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.refused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// `423 Locked` with the engine's sentence: another device drives the row.
pub(crate) fn driven_refusal(sentence: &str) -> RouteRejection {
    (StatusCode::LOCKED, sentence.to_string())
        .into_response()
        .into()
}

/// The well-formed page ids in the request's [`PAGE_HEADER`], read from its raw
/// bytes entry by entry so one malformed entry costs only itself.
fn named_pages(headers: &axum::http::HeaderMap) -> Vec<PageId> {
    headers
        .get_all(PAGE_HEADER)
        .iter()
        .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
        .filter_map(|entry| PageId::parse(std::str::from_utf8(entry).ok()?.trim()))
        .take(MAX_PAGES)
        .collect()
}

/// Run the request with its asker set, and answer any change the engine
/// refused because another device drives the row.
pub(crate) async fn identify(request: Request, next: Next) -> Response {
    let asker = RowAsker {
        pages: named_pages(request.headers()),
        // Read for every route, its own reader or not: where the route does
        // not read it, going ahead over a driver is its only effect.
        force: axum::extract::Query::<ForceConnectedQuery>::try_from_uri(request.uri())
            .is_ok_and(|axum::extract::Query(force)| force.over_everybody()),
        refused: Arc::default(),
    };
    let answered = asker.clone();
    let response = ASKING.scope(asker, next.run(request)).await;
    let refusal = answered.refusal().clone();
    match refusal {
        Some(sentence) => driven_refusal(&sentence).into_response(),
        None => response,
    }
}

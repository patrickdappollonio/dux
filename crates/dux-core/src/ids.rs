//! Distinct id types for the two keyspaces the tab machinery keeps confusing:
//! **session ids** (an agent's primary key) and **tab ids** (a provider tab's
//! key, which is also the key of every tab-scoped runtime map).
//!
//! An agent's first tab, its **session-slot tab**, is named by a stored pointer
//! (see [`crate::model::AgentSession::slot_tab_id`]). Its id used to BE the
//! session id, and while that held every `&str` id was silently
//! interchangeable: passing a session id positionally into a tab-id parameter
//! compiled, ran, and was invisible to a text search. The pointer ended the
//! coincidence, and these types are what turned the remaining mix-ups into
//! compile errors rather than runtime surprises.
//!
//! These types make that class of mistake a compile error at the seams where it
//! is dangerous, and nowhere else. They are an **in-engine discipline**: the
//! wire types, the HTTP routes and SQLite all keep plain strings, because an id
//! arriving from outside has not been classified yet.
//!
//! # Scope, and the three maps deliberately left out
//!
//! The tab-keyed runtime maps carry [`TabId`] keys: `providers`,
//! `running_provider_pins`, `agent_tabs`, `resume_fallback_candidates`,
//! `launched_drop_paste`, `needs_attention`, `agent_viewed` and `pty_progress`.
//!
//! `pty_activity`, `pty_input` and `pty_pointer` deliberately do NOT, and they
//! are the interesting case. They are keyed by the wider **PTY** keyspace: an
//! agent tab id OR a companion terminal id, which is why `clear_tab_runtime` and
//! `clear_terminal_runtime` both reach into them. Giving them a `TabId` key would
//! have required calling every terminal id a tab id, which is the same class of
//! lie this module exists to stop. They stay string-keyed until there is a
//! `PtyId` worth minting.
//!
//! [`SessionId`] is applied narrowly, at the seams where two ids sit side by side
//! and could be swapped without a word changing: `Engine::is_slot_tab_of` and
//! `Engine::slot_tab_id_of`, and the transport-facing callers that feed them.
//! Single-session-id functions (`session_by_id`, `tab_ids_for_session`) keep
//! `&str`: with one id there is nothing to swap it with.
//!
//! # Shape
//!
//! Each kind is an owned/borrowed pair modelled exactly on `String`/`str` and
//! `PathBuf`/`Path`: [`TabId`] owns, [`TabIdRef`] borrows, and `TabId` derefs
//! and borrows to `TabIdRef` so a `HashMap<TabId, V>` is looked up with a
//! `&TabIdRef` at no allocation cost.
//!
//! # Crossing back to a plain string
//!
//! `as_str` is the one way out, and it is deliberately the only one: there is no
//! `Deref<Target = str>`, no `From<&str>`, and no `AsRef<str>`. Constructing one
//! of these from a bare string means naming the kind at the call site
//! (`TabId::new(x)`, `SessionIdRef::new(x)`), which is the whole point: a
//! conversion you had to write down is a conversion a reviewer can see. Reach
//! for `as_str` at a storage or wire boundary, where the value stops being an id
//! dux reasons about and becomes bytes somebody else stores.
//!
//! # The guarantee, as a test
//!
//! The whole point is a compile error, which no ordinary `#[test]` can assert,
//! so both halves are pinned as doctests. Handing a session id to something that
//! wants a tab id is refused:
//!
//! ```compile_fail
//! use dux_core::ids::{SessionId, TabIdRef};
//! fn wants_a_tab(_: &TabIdRef) {}
//! wants_a_tab(&SessionId::new("s1"));
//! ```
//!
//! and so is the reverse, a tab id where a session id belongs:
//!
//! ```compile_fail
//! use dux_core::ids::{SessionIdRef, TabId};
//! fn wants_a_session(_: &SessionIdRef) {}
//! wants_a_session(&TabId::new("t1"));
//! ```
//!
//! while asking the resolver for the agent's slot tab is exactly what compiles:
//!
//! ```
//! use dux_core::ids::TabIdRef;
//! fn wants_a_tab(_: &TabIdRef) {}
//! # let session = dux_core::model::AgentSession {
//! #     id: "s1".to_string(),
//! #     slot_tab_id: "slot-1".to_string(),
//! #     provider: dux_core::model::ProviderKind::new("claude"),
//! #     workspace: dux_core::model::AgentWorkspace::Folder(dux_core::model::FolderWorkspace {
//! #         folder_path: "/tmp".to_string(),
//! #     }),
//! #     title: None,
//! #     started_providers: Vec::new(),
//! #     desired_running: false,
//! #     auto_reopen_enabled: false,
//! #     status: dux_core::model::SessionStatus::Detached,
//! #     created_at: chrono::Utc::now(),
//! #     updated_at: chrono::Utc::now(),
//! #     last_focused_tab: None,
//! # };
//! wants_a_tab(session.slot_tab_id());
//! ```

use std::borrow::Borrow;
use std::fmt;

/// Define an owned/borrowed id pair over `String`/`str`.
///
/// The borrowed half is an unsized `#[repr(transparent)]` wrapper around `str`,
/// which is the standard shape for this (`std`'s own `Path` and `OsStr` are
/// built the same way) and the only one that lets a map keyed by the owned form
/// be probed with a borrowed one without allocating.
macro_rules! id_pair {
    ($owned:ident, $borrowed:ident, $what:literal) => {
        #[doc = concat!("An owned ", $what, ". See the [module docs](self).")]
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $owned(String);

        #[doc = concat!("A borrowed ", $what, ". See the [module docs](self).")]
        #[derive(PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(transparent)]
        pub struct $borrowed(str);

        impl $owned {
            #[doc = concat!("Name a string as a ", $what, ".")]
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// The underlying string, for a storage or wire boundary. See the
            /// [module docs](self) on crossing back.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[doc = concat!("This id borrowed as a [`", stringify!($borrowed), "`].")]
            pub fn as_ref_id(&self) -> &$borrowed {
                $borrowed::new(&self.0)
            }
        }

        impl $borrowed {
            #[doc = concat!("Name a borrowed string as a ", $what, ".")]
            pub fn new(id: &str) -> &Self {
                // SAFETY: `Self` is `#[repr(transparent)]` over `str`, so the
                // two have identical layout and metadata and the cast is the
                // same one `Path::new` performs over `OsStr`.
                unsafe { &*(id as *const str as *const Self) }
            }

            /// The underlying string, for a storage or wire boundary. See the
            /// [module docs](self) on crossing back.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::ops::Deref for $owned {
            type Target = $borrowed;

            fn deref(&self) -> &$borrowed {
                self.as_ref_id()
            }
        }

        impl Borrow<$borrowed> for $owned {
            fn borrow(&self) -> &$borrowed {
                self.as_ref_id()
            }
        }

        impl AsRef<$borrowed> for $owned {
            fn as_ref(&self) -> &$borrowed {
                self.as_ref_id()
            }
        }

        impl AsRef<$borrowed> for $borrowed {
            fn as_ref(&self) -> &$borrowed {
                self
            }
        }

        impl ToOwned for $borrowed {
            type Owned = $owned;

            fn to_owned(&self) -> $owned {
                $owned(self.0.to_string())
            }
        }

        impl PartialEq<$borrowed> for $owned {
            fn eq(&self, other: &$borrowed) -> bool {
                self.as_str() == other.as_str()
            }
        }

        impl PartialEq<$owned> for $borrowed {
            fn eq(&self, other: &$owned) -> bool {
                self.as_str() == other.as_str()
            }
        }

        impl PartialEq<&$borrowed> for $owned {
            fn eq(&self, other: &&$borrowed) -> bool {
                self.as_str() == other.as_str()
            }
        }

        impl fmt::Display for $owned {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Display for $borrowed {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        // Debug prints the kind, so a log line or a failing assertion says which
        // keyspace the id came from rather than showing a bare string that could
        // have been either.
        impl fmt::Debug for $owned {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($owned), self.0)
            }
        }

        impl fmt::Debug for $borrowed {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($owned), &self.0)
            }
        }
    };
}

id_pair!(TabId, TabIdRef, "provider-tab id");
id_pair!(SessionId, SessionIdRef, "agent-session id");

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn an_owned_id_probes_a_map_through_its_borrowed_form() {
        // The property the whole shape exists for: a map keyed by the owned form
        // is looked up with a borrowed one, no allocation and no `to_string`.
        let mut map: HashMap<TabId, u32> = HashMap::new();
        map.insert(TabId::new("t1"), 7);
        assert_eq!(map.get(TabIdRef::new("t1")), Some(&7));
        assert_eq!(map.get(TabIdRef::new("t2")), None);
    }

    #[test]
    fn the_round_trip_through_a_storage_boundary_preserves_the_string() {
        let tab = TabId::new("t1");
        assert_eq!(tab.as_str(), "t1");
        assert_eq!(TabIdRef::new(tab.as_str()), tab.as_ref_id());
        assert_eq!(tab.as_ref_id().to_owned(), tab);
    }

    #[test]
    fn debug_says_which_keyspace_an_id_came_from() {
        // A bare string in a log line or an assertion failure could have been
        // either kind; these say.
        assert_eq!(format!("{:?}", TabId::new("x")), "TabId(\"x\")");
        assert_eq!(format!("{:?}", SessionId::new("x")), "SessionId(\"x\")");
        assert_eq!(format!("{:?}", SessionIdRef::new("x")), "SessionId(\"x\")");
        // Display stays bare, because that is what goes into user-facing text.
        assert_eq!(TabId::new("x").to_string(), "x");
    }

    #[test]
    fn the_two_keyspaces_are_not_interchangeable_even_when_the_strings_match() {
        // Two ids that happen to hold the same bytes are still two keyspaces;
        // the types keep them apart, which is the only reason the mix-up
        // becomes a compile error rather than a silent success.
        let session = SessionId::new("s1");
        let tab = TabId::new("s1");
        assert_eq!(session.as_str(), tab.as_str());
        let mut map: HashMap<TabId, u32> = HashMap::new();
        map.insert(tab, 1);
        // `map.get(session.as_ref_id())` does not compile: the map is keyed by
        // tab ids and a session id is not one. Crossing takes an explicit
        // rename, which is the visible act the module docs describe.
        assert_eq!(map.get(TabIdRef::new(session.as_str())), Some(&1));
    }
}

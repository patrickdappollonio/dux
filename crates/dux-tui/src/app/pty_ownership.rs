//! This surface's seat in the PTY-ownership registry.
//!
//! ## What ownership means here
//!
//! One PTY has one driver. While a background web server is serving, a browser
//! is a possible driver and so is this terminal, so "who may type into this
//! child, and who decides its grid" stops being a question this surface can
//! answer by itself. It asks
//! [`dux_core::pty_owners::PtySizeOwners`] instead, through the seat the
//! background-serve seam hands it, and obeys the answer: a refusal drops the
//! keystroke rather than writing it, and a refused resize leaves the child alone.
//!
//! READING IS NEVER GATED. A demoted terminal still renders the child's output,
//! still scrolls its scrollback, still selects and copies. Ownership is about
//! writing and sizing, and nothing else.
//!
//! ## Nothing serving, nothing to ask
//!
//! With no background server up there is no registry, no seat and no gate: every
//! helper here short-circuits to "allowed" before it touches anything. That is
//! not an optimisation, it is the contract: with the setting off this surface
//! behaves exactly as it did before any of this existed, and the tests below pin
//! both halves.
//!
//! ## Losing ownership is sticky
//!
//! Exactly as it is for a browser. Once another device is driving a pty, nothing
//! passive takes it back: not selecting the agent, not looking at it, not the
//! other device going quiet. The two ways back are the explicit take-over action
//! and typing into a pty that nobody owns. The web's socket-specific
//! self-succession rule has no counterpart here, because this surface has no
//! socket to have a ghost of.

use dux_core::background_serve::{PtyOwnershipEvent, TUI_DEVICE_LABEL, TuiOwnership};

use super::*;

/// Who is driving a pty, from this surface's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PtyDriver {
    /// Nobody has claimed it, or nothing is serving so the question does not
    /// arise. Typing claims it; resizing claims it.
    Free,
    /// This surface holds it.
    Mine,
    /// Another device holds it, named as it names itself. Typing and resizing are
    /// refused until an explicit take-over.
    Elsewhere { device: String },
}

/// What a watcher's screen calls a driver that gave dux no name for itself.
///
/// Reachable in practice: a browser that presented no `User-Agent` at its
/// upgrade is recorded with no device. Naming it honestly beats naming it wrongly.
pub(crate) const UNNAMED_DEVICE: &str = "another device";

impl App {
    /// This surface's seat in the registry, or `None` when nothing is serving.
    fn pty_ownership(&self) -> Option<TuiOwnership> {
        self.companion
            .as_ref()
            .and_then(|companion| companion.ownership())
    }

    /// Who is driving `pty_id` right now.
    ///
    /// A live read every time it is asked, never a latched flag: ownership moves
    /// between devices while nothing on this surface happens at all, and a cached
    /// verdict is how a screen ends up telling the user about a browser tab that
    /// closed ten minutes ago.
    pub(crate) fn pty_driver(&self, pty_id: &str) -> PtyDriver {
        let Some(seat) = self.pty_ownership() else {
            return PtyDriver::Free;
        };
        let (owner, _, device) = seat.owners.current_owner(pty_id);
        match owner {
            None => PtyDriver::Free,
            Some(conn) if conn == seat.conn_id => PtyDriver::Mine,
            Some(_) => PtyDriver::Elsewhere {
                device: device.unwrap_or_else(|| UNNAMED_DEVICE.to_string()),
            },
        }
    }

    /// The device driving the FOCUSED terminal surface, when it is not this one.
    ///
    /// The one question the demoted treatment asks, and it asks it of the live
    /// registry on every frame.
    pub(crate) fn focused_pty_driven_elsewhere(&self) -> Option<String> {
        let pty_id = self.selected_terminal_surface_id()?;
        match self.pty_driver(&pty_id) {
            PtyDriver::Elsewhere { device } => Some(device),
            PtyDriver::Free | PtyDriver::Mine => None,
        }
    }

    /// THE TYPING CHOKEPOINT: may this surface write to the focused terminal
    /// surface's PTY right now?
    ///
    /// Every path on this surface that puts bytes into a PTY asks this first, and
    /// there is deliberately one of it rather than one per path: a keystroke, a
    /// paste, a macro and a forwarded pointer report are all writes, and a path
    /// that forgot to ask would be a hole in the gate that only shows up as one
    /// device stealing another's prompt.
    ///
    /// An uncontested first write CLAIMS the pty, exactly as a browser's first
    /// keystroke does, and that claim is announced so watchers learn who is
    /// driving. A write against a pty another device holds is DROPPED (logged at
    /// debug, like the web's dropped non-owner keystroke) and the demoted
    /// treatment on screen is what tells the user why.
    pub(crate) fn may_type_into_focused_pty(&mut self) -> bool {
        match self.selected_terminal_surface_id() {
            Some(pty_id) => self.may_type_into_pty(&pty_id),
            // No live PTY under the cursor: there is nothing to own and nothing
            // to gate. The write paths fail on their own missing client.
            None => true,
        }
    }

    /// The same gate for a named pty. Split out so a test can name its target and
    /// so a future write path with an explicit target has somewhere to go.
    pub(crate) fn may_type_into_pty(&mut self, pty_id: &str) -> bool {
        let Some(seat) = self.pty_ownership() else {
            return true;
        };
        let claim = seat
            .owners
            .may_write(pty_id, seat.conn_id, Some(TUI_DEVICE_LABEL));
        if claim.claimed_new
            && let Some(epoch) = claim.epoch
        {
            self.publish_ownership(&[PtyOwnershipEvent::Claimed {
                pty_id: pty_id.to_string(),
                conn_id: seat.conn_id,
                epoch,
                device: TUI_DEVICE_LABEL.to_string(),
            }]);
        }
        if !claim.allowed {
            dux_core::logger::debug(&format!(
                "keystroke for pty {pty_id} dropped: another device currently owns its input"
            ));
        }
        claim.allowed
    }

    /// THE SIZING CHOKEPOINT: may this surface resize `pty_id` to `rows` x `cols`,
    /// and is the resize the one that should reach the child?
    ///
    /// Two questions in one call because they are decided together. The claim
    /// (may I) is resolved under the owners lock; the apply order (is mine still
    /// the newest) is resolved by the same gate a queued browser resize passes
    /// through, so one pty can never be sized for a device that no longer drives
    /// it. A granted resize also publishes its grid, which is how a web watcher
    /// learns to adopt this terminal's geometry.
    ///
    /// A refusal means the caller must NOT resize the child. It renders the
    /// authoritative grid instead, which it already does safely, and the demoted
    /// treatment says whose grid it is.
    pub(crate) fn may_resize_pty(&mut self, pty_id: &str, rows: u16, cols: u16) -> bool {
        let Some(seat) = self.pty_ownership() else {
            return true;
        };
        // An armed take-over is spent here, whether or not it is granted, so a
        // stale intent cannot sit around and surprise a later resize.
        let takeover = self
            .pending_pty_takeover
            .as_deref()
            .is_some_and(|armed| armed == pty_id);
        if takeover {
            self.pending_pty_takeover = None;
        }
        let outcome = seat.owners.claim_for_resize(
            pty_id,
            seat.conn_id,
            takeover,
            Some(TUI_DEVICE_LABEL),
            |_| {},
        );
        let mut events = Vec::new();
        if let Some(epoch) = outcome.epoch {
            events.push(PtyOwnershipEvent::Claimed {
                pty_id: pty_id.to_string(),
                conn_id: seat.conn_id,
                epoch,
                device: TUI_DEVICE_LABEL.to_string(),
            });
        }
        if !outcome.apply {
            dux_core::logger::debug(&format!(
                "resize of pty {pty_id} refused: another device currently owns its sizing, and a \
                 take-over must say so explicitly"
            ));
            self.publish_ownership(&events);
            return false;
        }
        // The one apply order. This surface applies immediately while a browser's
        // resize waits in the engine actor's queue, so a resize stamped before
        // this one can still be sitting there; offering the seq here is what stops
        // it landing afterwards, and what stops THIS resize landing if the roles
        // are reversed.
        let seq = outcome.seq.unwrap_or_default();
        if !seat.owners.accept_grid_apply(pty_id, seq) {
            self.publish_ownership(&events);
            return false;
        }
        events.push(PtyOwnershipEvent::GridApplied {
            pty_id: pty_id.to_string(),
            rows,
            cols,
            seq,
        });
        self.publish_ownership(&events);
        true
    }

    /// Take over the focused terminal surface's PTY: this device drives it from
    /// now on, and whichever device was driving becomes a watcher.
    ///
    /// Arms the claim rather than making it, because the claim has to carry this
    /// pane's real geometry and the render pass is what measures that. Clearing
    /// `last_pty_resize_target` is the other half: without it a take-over of a pty
    /// whose pane happens to measure exactly what it measured last time would be
    /// deduped away, and the claim would never be sent at all.
    pub(crate) fn take_over_focused_pty(&mut self) {
        if self.pty_ownership().is_none() {
            self.set_warning(
                "Nothing is serving the web UI in the background, so no other device can be \
                 driving this terminal and there is nothing to take over. Use \
                 start-background-server to serve."
                    .to_string(),
            );
            return;
        }
        let Some(pty_id) = self.selected_terminal_surface_id() else {
            self.set_warning(
                "There is no running terminal in the center pane to take over. Select an agent or \
                 a terminal that is running first."
                    .to_string(),
            );
            return;
        };
        match self.pty_driver(&pty_id) {
            PtyDriver::Mine => {
                self.set_info(
                    "You are already the device driving this terminal, so there is nothing to \
                     take over. Typing here reaches it."
                        .to_string(),
                );
                return;
            }
            PtyDriver::Free => {
                self.set_info(
                    "No other device is driving this terminal, so it is already yours to type \
                     into. Taking it over anyway, so browsers watching it know."
                        .to_string(),
                );
            }
            PtyDriver::Elsewhere { device } => {
                self.set_info(format!(
                    "Taking this terminal over from {device}. Typing here reaches it again, its \
                     size follows this window, and that device keeps watching without being able \
                     to type."
                ));
            }
        }
        self.pending_pty_takeover = Some(pty_id);
        self.last_pty_resize_target = None;
    }

    /// Let go of every pty this surface is driving, and say so to the browsers.
    ///
    /// Called when the participation itself ends: the background server stops, the
    /// terminal is handed over to the flip, or dux quits. Deliberately NOT called
    /// when the user merely selects a different agent: this surface has no socket
    /// to close, so only a deliberate end is an end, which is the same rule that
    /// keeps a backgrounded browser tab's ownership alive.
    ///
    /// Runs BEFORE the serve is torn down, because the announcements ride the
    /// serve's own buses: after the stop there is nothing left to announce on, and
    /// every browser's take-over card would keep naming this terminal.
    pub(crate) fn release_owned_ptys(&mut self) {
        self.pending_pty_takeover = None;
        let Some(seat) = self.pty_ownership() else {
            return;
        };
        let released = seat.owners.release_all(seat.conn_id);
        let events: Vec<PtyOwnershipEvent> = released
            .into_iter()
            .map(|(pty_id, epoch)| PtyOwnershipEvent::Released { pty_id, epoch })
            .collect();
        self.publish_ownership(&events);
    }

    /// Hand ownership facts to the companion, which is the only thing that can
    /// announce them. A no-op with nothing to say.
    fn publish_ownership(&mut self, events: &[PtyOwnershipEvent]) {
        if events.is_empty() {
            return;
        }
        if let Some(companion) = self.companion.as_mut() {
            companion.publish_ownership_events(events);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::background_server::tests::{FakeCompanion, Recorded};
    use crate::app::test_support::{default_bindings, test_app};

    /// An app with a serving companion, plus the registry seat the companion
    /// handed it and the record of what it published.
    fn serving_app() -> (
        App,
        std::sync::Arc<std::sync::Mutex<Recorded>>,
        TuiOwnership,
    ) {
        let mut app = test_app(default_bindings());
        let (companion, recorded, ownership) = FakeCompanion::serving_with_ownership();
        app.companion = Some(companion);
        (app, recorded, ownership)
    }

    /// Nothing serving: the gate never consults a registry, because there is not
    /// one, and every write and resize is allowed exactly as it was before this
    /// surface joined the ownership model at all.
    #[test]
    fn with_nothing_serving_every_gate_is_open_and_no_registry_is_consulted() {
        let mut app = test_app(default_bindings());
        assert!(app.companion.is_none(), "the fixture installs no companion");

        assert!(app.may_type_into_pty("s1"));
        assert!(app.may_resize_pty("s1", 24, 80));
        assert_eq!(app.pty_driver("s1"), PtyDriver::Free);
        assert_eq!(app.focused_pty_driven_elsewhere(), None);
        // Repeated calls stay open: with no registry there is no state to
        // accumulate and nothing that can start refusing.
        assert!(app.may_type_into_pty("s1"));
        assert!(app.may_resize_pty("s1", 30, 100));
    }

    /// The first write claims an unowned pty and announces it, exactly as a
    /// browser's uncontested first keystroke does. Without the announcement a
    /// browser watching the agent never learns this terminal is driving it.
    #[test]
    fn the_first_keystroke_claims_an_unowned_pty_and_announces_it_once() {
        let (mut app, recorded, seat) = serving_app();

        assert!(app.may_type_into_pty("s1"));
        assert!(seat.owners.is_owner("s1", seat.conn_id));
        let published = recorded.lock().expect("not poisoned").published.clone();
        assert_eq!(
            published,
            vec![PtyOwnershipEvent::Claimed {
                pty_id: "s1".to_string(),
                conn_id: seat.conn_id,
                epoch: 1,
                device: TUI_DEVICE_LABEL.to_string(),
            }],
            "the claim must be announced with this surface's device label"
        );

        // Every keystroke after it is an ordinary write by the owner: allowed,
        // and silent, or the bus would carry one handover per character.
        assert!(app.may_type_into_pty("s1"));
        assert_eq!(
            recorded.lock().expect("not poisoned").published.len(),
            1,
            "an owner's later keystrokes announce nothing"
        );
    }

    /// A browser is typing into the agent, so this surface's keystrokes are
    /// DROPPED rather than written, and the demoted treatment has a device to
    /// name. This is the refusal the whole participation exists to produce.
    #[test]
    fn typing_is_refused_while_a_browser_connection_owns_the_pty() {
        let (mut app, recorded, seat) = serving_app();
        let browser = seat.owners.next_conn_id();
        assert!(
            seat.owners
                .may_write("s1", browser, Some("Mozilla/5.0 (Macintosh) Chrome/120"))
                .claimed_new,
            "the browser claims the pty by typing into it first"
        );

        assert!(
            !app.may_type_into_pty("s1"),
            "a keystroke must not reach a pty another device is driving"
        );
        assert!(
            seat.owners.is_owner("s1", browser),
            "a refused keystroke must not steal ownership either"
        );
        assert!(
            recorded.lock().expect("not poisoned").published.is_empty(),
            "a refusal changes nothing, so it announces nothing"
        );
        assert_eq!(
            app.pty_driver("s1"),
            PtyDriver::Elsewhere {
                device: "Mozilla/5.0 (Macintosh) Chrome/120".to_string()
            },
            "the demoted treatment names the driving device from the registry"
        );
    }

    /// A driver that gave dux no name for itself is still named, honestly.
    #[test]
    fn an_unnamed_driver_is_described_rather_than_left_blank() {
        let (app, _recorded, seat) = serving_app();
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("s1", browser).expect("claimed");
        assert_eq!(
            app.pty_driver("s1"),
            PtyDriver::Elsewhere {
                device: UNNAMED_DEVICE.to_string()
            }
        );
    }

    /// A resize against a pty a browser owns is refused WHOLE: the child is not
    /// resized (the caller is told not to), and nothing is announced.
    #[test]
    fn a_resize_is_refused_while_a_browser_owns_the_sizing() {
        let (mut app, recorded, seat) = serving_app();
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("s1", browser).expect("claimed");

        assert!(
            !app.may_resize_pty("s1", 40, 120),
            "a non-owner must not resize the child"
        );
        assert!(recorded.lock().expect("not poisoned").published.is_empty());
    }

    /// A granted resize publishes its grid, which is what a web watcher adopts.
    /// Without this the watcher renders the child's bytes into a differently
    /// sized emulator and records wrapped garbage in its scrollback.
    #[test]
    fn a_granted_resize_publishes_the_grid_for_web_watchers() {
        let (mut app, recorded, seat) = serving_app();

        assert!(app.may_resize_pty("s1", 24, 80));
        let published = recorded.lock().expect("not poisoned").published.clone();
        assert_eq!(
            published,
            vec![
                PtyOwnershipEvent::Claimed {
                    pty_id: "s1".to_string(),
                    conn_id: seat.conn_id,
                    epoch: 1,
                    device: TUI_DEVICE_LABEL.to_string(),
                },
                PtyOwnershipEvent::GridApplied {
                    pty_id: "s1".to_string(),
                    rows: 24,
                    cols: 80,
                    seq: 1,
                },
            ],
            "an unowned pty is claimed and its new grid announced, in that order"
        );
    }

    /// AMENDMENT 8, both halves. The take-over transfers ownership in epoch
    /// order, and it clears the resize dedupe so a take-over at the geometry the
    /// pane already had still sends the resize that carries the claim.
    #[test]
    fn a_take_over_transfers_in_epoch_order_and_clears_the_resize_dedupe() {
        let (mut app, recorded, seat) = serving_app();
        let browser = seat.owners.next_conn_id();
        let browser_epoch = seat.owners.claim("s1", browser).expect("claimed");

        // The pane has already been sized for this pty, so without the clear the
        // take-over's resize would be deduped away.
        app.last_pty_resize_target = Some("s1".to_string());
        app.last_pty_size = (24, 80);
        app.session_surface = SessionSurface::Agent;

        // Arm it directly: `take_over_focused_pty` resolves the focused surface,
        // which needs a live PTY, and what is under test here is the claim.
        app.pending_pty_takeover = Some("s1".to_string());
        app.last_pty_resize_target = None;

        assert!(
            app.may_resize_pty("s1", 24, 80),
            "an explicit take-over is granted even at the geometry the pane already had"
        );
        assert!(seat.owners.is_owner("s1", seat.conn_id));
        assert_eq!(
            app.pending_pty_takeover, None,
            "the armed take-over is spent by the resize that carried it"
        );
        assert_eq!(
            app.last_pty_resize_target, None,
            "the dedupe was cleared, which is what let the resize through"
        );

        let published = recorded.lock().expect("not poisoned").published.clone();
        match published.first().expect("the handover was announced") {
            PtyOwnershipEvent::Claimed { epoch, .. } => assert!(
                *epoch > browser_epoch,
                "the take-over's epoch must be strictly newer than the claim it \
                 retires ({epoch} vs {browser_epoch}), or every client discards it"
            ),
            other => panic!("expected a claim first, got {other:?}"),
        }
    }

    /// Losing ownership is STICKY. Nothing passive takes a pty back: not asking
    /// who drives it, not resizing, not the driver going quiet. The two ways back
    /// are the explicit take-over and typing into a pty nobody owns.
    #[test]
    fn a_demoted_surface_never_re_claims_by_itself() {
        let (mut app, _recorded, seat) = serving_app();
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("s1", browser).expect("claimed");

        for _ in 0..3 {
            let _ = app.pty_driver("s1");
            assert!(!app.may_resize_pty("s1", 40, 120));
            assert!(!app.may_type_into_pty("s1"));
        }
        assert!(
            seat.owners.is_owner("s1", browser),
            "nothing on this surface may take a pty back without being asked to"
        );

        // The explicit action is the way back.
        app.pending_pty_takeover = Some("s1".to_string());
        assert!(app.may_resize_pty("s1", 40, 120));
        assert!(seat.owners.is_owner("s1", seat.conn_id));
    }

    /// AMENDMENT 8's ordering, end to end across the two surfaces. A browser's
    /// resize is stamped first and applied later; this surface's take-over is
    /// stamped later and applied at once. The queued one must be dropped when it
    /// finally arrives, or the child ends up sized for a device that no longer
    /// drives it.
    #[test]
    fn a_queued_browser_resize_never_lands_after_this_surfaces_later_claim() {
        let (mut app, _recorded, seat) = serving_app();
        let browser = seat.owners.next_conn_id();

        // The browser claims and ENQUEUES: its apply is still in the actor queue.
        let queued = seat
            .owners
            .claim_for_resize("s1", browser, false, Some("Chrome"), |_| {})
            .seq
            .expect("the browser's claim applied");

        // This surface takes over and applies immediately.
        app.pending_pty_takeover = Some("s1".to_string());
        assert!(app.may_resize_pty("s1", 50, 150));

        assert!(
            !seat.owners.accept_grid_apply("s1", queued),
            "the browser's queued resize must be dropped when the actor drains it"
        );
    }

    /// Releasing is what the participation's ends do, and it announces every pty
    /// it let go of. Without the announcement a browser's take-over card names a
    /// terminal that has stopped serving, forever.
    #[test]
    fn releasing_lets_go_of_every_owned_pty_and_announces_each_one() {
        let (mut app, recorded, seat) = serving_app();
        assert!(app.may_type_into_pty("s1"));
        assert!(app.may_type_into_pty("s2"));
        recorded.lock().expect("not poisoned").published.clear();

        app.release_owned_ptys();

        assert!(!seat.owners.is_owner("s1", seat.conn_id));
        assert!(!seat.owners.is_owner("s2", seat.conn_id));
        let published = recorded.lock().expect("not poisoned").published.clone();
        assert_eq!(published.len(), 2, "both releases announced: {published:?}");
        assert!(
            published
                .iter()
                .all(|event| matches!(event, PtyOwnershipEvent::Released { .. })),
            "a release announces an owner-cleared handover, nothing else: {published:?}"
        );
    }

    /// A release must not take a pty another device has since claimed, and a
    /// second release has nothing to say.
    #[test]
    fn releasing_leaves_another_devices_pty_alone_and_is_quiet_the_second_time() {
        let (mut app, recorded, seat) = serving_app();
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("theirs", browser).expect("claimed");
        assert!(app.may_type_into_pty("mine"));
        recorded.lock().expect("not poisoned").published.clear();

        app.release_owned_ptys();
        assert!(seat.owners.is_owner("theirs", browser));
        assert_eq!(recorded.lock().expect("not poisoned").published.len(), 1);

        recorded.lock().expect("not poisoned").published.clear();
        app.release_owned_ptys();
        assert!(
            recorded.lock().expect("not poisoned").published.is_empty(),
            "there was nothing left to release, so nothing may be announced"
        );
    }

    /// An app with a real PTY under the cursor in the center pane, so the focused
    /// terminal surface resolves to something ownable and the render pass has a
    /// child to (not) resize.
    fn app_with_a_live_pty() -> (
        App,
        std::sync::Arc<std::sync::Mutex<Recorded>>,
        TuiOwnership,
    ) {
        let (mut app, recorded, seat) = serving_app();
        app.selected_left = 1;
        app.center_mode = CenterMode::Agent;
        app.session_surface = SessionSurface::Agent;
        app.engine.providers.insert(
            "session-1".to_string(),
            crate::pty::PtyClient::spawn(
                "sh",
                &["-c".to_string(), "printf ready; sleep 5".to_string()],
                std::path::Path::new("."),
                10,
                10,
                100,
            )
            .expect("spawn pty"),
        );
        (app, recorded, seat)
    }

    /// THE DEMOTED TREATMENT, rendered. A browser holds the pty, so the center
    /// pane's hint bar stops listing keys that go nowhere and says who is driving
    /// and how to take it back, and the child is NOT re-gridded to this pane.
    ///
    /// A real render rather than a call to the cue builder, because the bug this
    /// guards against is the branch never being reached: a hint ladder that still
    /// picks "typing goes to the agent" is a screen telling the user their keys
    /// are landing when every one of them is being dropped.
    #[test]
    fn a_demoted_pane_renders_the_driving_device_and_leaves_the_child_grid_alone() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (mut app, _recorded, seat) = app_with_a_live_pty();
        let browser = seat.owners.next_conn_id();
        seat.owners
            .claim_for_resize("session-1", browser, false, Some("Chrome"), |_| {})
            .epoch
            .expect("the browser claimed the pty");

        let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render succeeds");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(
            rendered.contains("Chrome is driving this agent"),
            "the hint bar must name the device that holds the pty"
        );
        assert!(
            rendered.contains("take-over-terminal"),
            "and name the way to take it back, in full: the cue has to FIT in the \
             center pane, and a truncated one names a problem with no way out"
        );
        assert!(
            !rendered.contains("Typing goes to the agent"),
            "a demoted pane must not claim that typing reaches the agent"
        );

        let grid = app
            .engine
            .providers
            .get("session-1")
            .and_then(|client| client.grid_size());
        assert_eq!(
            grid,
            Some((10, 10)),
            "a refused resize must leave the child at the grid its driver set"
        );
    }

    /// The same pane, driving it itself: the resize lands and the demoted cue is
    /// nowhere. The other half of the render test, because "the cue never shows"
    /// and "the cue always shows" would both pass the one above on its own.
    #[test]
    fn a_driving_pane_resizes_the_child_and_shows_no_demoted_cue() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (mut app, _recorded, _seat) = app_with_a_live_pty();

        let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render succeeds");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(
            !rendered.contains("is driving this agent"),
            "nobody else holds this pty, so there is nothing to say about it"
        );
        let grid = app
            .engine
            .providers
            .get("session-1")
            .and_then(|client| client.grid_size());
        assert_ne!(
            grid,
            Some((10, 10)),
            "an uncontested pane claims the pty and sizes the child to itself"
        );
    }

    /// Every end of the participation lets go, and the ONE place that ends it is
    /// the quiet stop: the palette command, a config reload that turns the setting
    /// off, the flip, and quitting all route through it.
    #[test]
    fn stopping_the_background_server_releases_this_surfaces_ptys() {
        let (mut app, recorded, seat) = serving_app();
        assert!(app.may_type_into_pty("s1"));
        assert!(seat.owners.is_owner("s1", seat.conn_id));
        recorded.lock().expect("not poisoned").published.clear();

        app.stop_background_server_quietly();

        assert!(
            !seat.owners.is_owner("s1", seat.conn_id),
            "the serve is over, so nothing here may still be recorded as driving a pty"
        );
        let published = recorded.lock().expect("not poisoned").published.clone();
        assert!(
            published
                .iter()
                .any(|event| matches!(event, PtyOwnershipEvent::Released { .. })),
            "the release must be announced while the serve's buses are still up: {published:?}"
        );
    }

    /// The take-over action refuses honestly when there is no ownership model to
    /// take part in, rather than arming an intent that will never be spent.
    #[test]
    fn the_take_over_action_says_so_when_nothing_is_serving() {
        let mut app = test_app(default_bindings());
        app.take_over_focused_pty();
        assert_eq!(app.pending_pty_takeover, None);
        let (_, message) = app
            .status
            .most_recent_tui()
            .expect("the refusal must reach the status line");
        assert!(
            message.contains("start-background-server"),
            "the refusal must point at the thing that would make this possible: {message}"
        );
    }
}

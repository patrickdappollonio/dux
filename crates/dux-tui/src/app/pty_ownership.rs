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
    /// Another device holds it, named SHORT. Typing and resizing are refused
    /// until an explicit take-over.
    ///
    /// The registry records what the driver presented, which for a browser is a
    /// raw `User-Agent` of well over a hundred characters. This carries the label
    /// [`dux_core::device_label::short_device_label`] made of it, because the one
    /// place it is rendered is a single line inside the center pane that also has
    /// to carry the way to take the terminal back.
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
            // The recorded identity is SHORTENED here, at the one place it
            // becomes something a screen renders, rather than at the cue: every
            // reader of this verdict (the hint bar today, anything later) then
            // gets a label that fits by construction.
            Some(_) => PtyDriver::Elsewhere {
                device: device
                    .as_deref()
                    .and_then(dux_core::device_label::short_device_label)
                    .unwrap_or_else(|| UNNAMED_DEVICE.to_string()),
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
            // A first-writer claim TRANSFERS the pty to this surface, so it gets
            // the same treatment the explicit take-over gets: the resize dedupe
            // is cleared, which is what makes the next render send this pane's
            // geometry to a child the previous driver may have re-gridded.
            //
            // Without it the trap is: a browser re-grids the child to a phone's
            // shape and disconnects, this surface types and claims, and the
            // dedupe sees the same pane size against the same target and sends
            // nothing. The terminal then owns a phone-sized child indefinitely,
            // with nothing on screen to say so, because the demoted cue is gone
            // the moment the claim succeeds.
            self.last_pty_resize_target = None;
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

    /// THE SIZING CHOKEPOINT: may this surface resize `pty_id` to `rows` x
    /// `cols`, is that resize still the newest one, and if both, do it.
    ///
    /// Three things in one call because they are decided together and the third
    /// is what makes the first two mean anything. The claim (may I) is resolved
    /// under the owners lock; the apply order (is mine still the newest) is
    /// resolved by the same gate a queued browser resize passes through, so one
    /// pty can never be sized for a device that no longer drives it; and the
    /// child is resized HERE rather than by the caller, so the grid announcement
    /// that follows can be made only about a resize that actually happened.
    ///
    /// Returns whether the resize was granted, which is the caller's cue to
    /// record its dedupe. A refusal records nothing: the pane renders the
    /// authoritative grid instead, which it already does safely, and the demoted
    /// treatment says whose grid it is.
    pub(crate) fn resize_pty_if_permitted(&mut self, pty_id: &str, rows: u16, cols: u16) -> bool {
        let Some(seat) = self.pty_ownership() else {
            // Nothing serving means no registry, no gate and no apply order to
            // take part in: resize the child directly, exactly as this surface
            // did before any of this existed.
            if let Some(client) = self.pty_client_for(pty_id) {
                let _ = client.resize(rows, cols);
            }
            return true;
        };
        // An arm for a DIFFERENT pty is spent-or-dropped here as well, so an
        // intent cannot sit around while the user looks somewhere else and then
        // fire minutes later on a pane they were not thinking about.
        self.expire_stale_pty_takeover(Some(pty_id));
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
            // Logged only when the refusal is NEW information. This is asked from
            // the render pass, which runs every tick, and a refused resize must
            // not record the resize dedupe (recording it makes a stale geometry
            // permanent), so an unchanged refusal repeats for as long as the pane
            // is on screen.
            let refusal = (pty_id.to_string(), rows, cols);
            if self.last_refused_pty_resize.as_ref() != Some(&refusal) {
                dux_core::logger::debug(&format!(
                    "resize of pty {pty_id} refused: another device currently owns its sizing, \
                     and a take-over must say so explicitly"
                ));
                self.last_refused_pty_resize = Some(refusal);
            }
            self.publish_ownership(&events);
            return false;
        }
        self.last_refused_pty_resize = None;
        // The one apply order. This surface applies immediately while a browser's
        // resize waits in the engine actor's queue, so a resize stamped before
        // this one can still be sitting there; offering the seq here is what stops
        // it landing afterwards, and what stops THIS resize landing if the roles
        // are reversed.
        let seq = outcome.seq.unwrap_or_default();
        if !seat.owners.accept_grid_apply(pty_id, seq, rows, cols) {
            // Same diagnostic weight the engine actor gives its own dropped
            // apply: expected traffic under a genuine race, not an error.
            dux_core::logger::debug(&format!(
                "resize of pty {pty_id} at seq {seq} dropped: a newer claim's geometry has \
                 already reached the child"
            ));
            self.publish_ownership(&events);
            return false;
        }
        // THE RESIZE ITSELF, done here rather than left to the caller, so the
        // grid announcement below can be made only about a resize that actually
        // happened: a provider that vanished between the claim and the apply
        // would otherwise have its geometry announced to every watcher, and they
        // would adopt a grid no child is drawing for.
        let resized = match self.pty_client_for(pty_id) {
            Some(client) => {
                let resized = client.resize(rows, cols).is_ok();
                // The accept above and this resize are two critical sections (see
                // `accept_grid_apply`), so a newer apply can have overtaken this
                // one in between. Re-apply the winner's geometry if so; its own
                // check finds itself newest, so this converges rather than
                // ping-ponging.
                if let Some((rows, cols)) = seat.owners.superseding_grid(pty_id, seq) {
                    dux_core::logger::debug(&format!(
                        "resize of pty {pty_id} at seq {seq} was overtaken mid-apply, so the \
                         newer {rows}x{cols} is being re-applied"
                    ));
                    let _ = client.resize(rows, cols);
                }
                resized
            }
            None => false,
        };
        if resized {
            events.push(PtyOwnershipEvent::GridApplied {
                pty_id: pty_id.to_string(),
                rows,
                cols,
                seq,
            });
        }
        self.publish_ownership(&events);
        true
    }

    /// The PTY behind a pty id, agent tab or companion terminal, in the same
    /// order the web actor's own lookup uses.
    ///
    /// Resolved from the ID rather than from the selection, because the sizing
    /// chokepoint is given a target and must resize THAT child, not whatever the
    /// cursor happens to be on by the time it runs.
    fn pty_client_for(&self, pty_id: &str) -> Option<&PtyClient> {
        self.engine.providers.get(pty_id).or_else(|| {
            self.engine
                .companion_terminals
                .get(pty_id)
                .map(|terminal| &terminal.client)
        })
    }

    /// Drop an armed take-over that is no longer about the pane in front of the
    /// user, quietly.
    ///
    /// An arm carries no geometry of its own: it waits for the render pass that
    /// measures the pane. So it has to be spent or dropped at the FIRST render
    /// after arming, or it survives a change of selection and fires later on a
    /// pty the user has stopped thinking about, taking it away from whichever
    /// device is driving it by then. `rendered` is the target that render pass is
    /// actually about, `None` when the center pane is showing something with no
    /// pty behind it at all.
    pub(crate) fn expire_stale_pty_takeover(&mut self, rendered: Option<&str>) {
        let Some(armed) = self.pending_pty_takeover.as_deref() else {
            return;
        };
        if rendered == Some(armed) {
            return;
        }
        self.pending_pty_takeover = None;
        self.set_info(
            "The take-over was dropped because you moved to a different terminal before it \
             could be claimed. Run take-over-terminal again on the one you want."
                .to_string(),
        );
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
                // Already ours, so there is no ownership to move. The command is
                // still the way to RETARGET the child's geometry at this window:
                // it clears the resize dedupe, which is the one thing standing
                // between a child another device re-gridded and this pane's own
                // measurements. Refusing outright left the user with a terminal
                // sized for somebody else's phone and no gesture to fix it.
                self.last_pty_resize_target = None;
                self.set_info(
                    "This terminal is already yours to type into, so there was nothing to take \
                     over. Resizing it to this window anyway, in case another device left it at \
                     a different size."
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
        assert!(app.resize_pty_if_permitted("s1", 24, 80));
        assert_eq!(app.pty_driver("s1"), PtyDriver::Free);
        assert_eq!(app.focused_pty_driven_elsewhere(), None);
        // Repeated calls stay open: with no registry there is no state to
        // accumulate and nothing that can start refusing.
        assert!(app.may_type_into_pty("s1"));
        assert!(app.resize_pty_if_permitted("s1", 30, 100));
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
                .may_write("s1", browser, Some(REAL_CHROME_UA))
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
                device: "Chrome on macOS".to_string()
            },
            "the demoted treatment names the driving device from the registry, \
             shortened to something that fits a line inside the pane"
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
            !app.resize_pty_if_permitted("s1", 40, 120),
            "a non-owner must not resize the child"
        );
        assert!(recorded.lock().expect("not poisoned").published.is_empty());
    }

    /// A granted resize publishes its grid, which is what a web watcher adopts.
    /// Without this the watcher renders the child's bytes into a differently
    /// sized emulator and records wrapped garbage in its scrollback.
    #[test]
    fn a_granted_resize_publishes_the_grid_for_web_watchers() {
        let (mut app, recorded, seat) = app_with_a_live_pty();

        assert!(app.resize_pty_if_permitted("session-1", 24, 80));
        let published = recorded.lock().expect("not poisoned").published.clone();
        assert_eq!(
            published,
            vec![
                PtyOwnershipEvent::Claimed {
                    pty_id: "session-1".to_string(),
                    conn_id: seat.conn_id,
                    epoch: 1,
                    device: TUI_DEVICE_LABEL.to_string(),
                },
                PtyOwnershipEvent::GridApplied {
                    pty_id: "session-1".to_string(),
                    rows: 24,
                    cols: 80,
                    seq: 1,
                },
            ],
            "an unowned pty is claimed and its new grid announced, in that order"
        );
    }

    /// A grid is announced only for a resize that actually reached a child.
    ///
    /// The claim can be granted for a pty whose provider has since gone (an agent
    /// that exited between the claim and the apply). Announcing its geometry
    /// anyway makes every watcher adopt a grid no child is drawing for, and
    /// nothing later corrects it: the announcement is the correction.
    #[test]
    fn a_resize_with_no_child_left_to_apply_it_to_announces_no_grid() {
        let (mut app, recorded, seat) = serving_app();

        assert!(
            app.resize_pty_if_permitted("gone", 24, 80),
            "the claim itself is still granted: nobody else owns this pty"
        );
        assert!(seat.owners.is_owner("gone", seat.conn_id));
        let published = recorded.lock().expect("not poisoned").published.clone();
        assert!(
            published
                .iter()
                .all(|event| !matches!(event, PtyOwnershipEvent::GridApplied { .. })),
            "no child was resized, so no grid may be announced: {published:?}"
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
            app.resize_pty_if_permitted("s1", 24, 80),
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
            assert!(!app.resize_pty_if_permitted("s1", 40, 120));
            assert!(!app.may_type_into_pty("s1"));
        }
        assert!(
            seat.owners.is_owner("s1", browser),
            "nothing on this surface may take a pty back without being asked to"
        );

        // The explicit action is the way back.
        app.pending_pty_takeover = Some("s1".to_string());
        assert!(app.resize_pty_if_permitted("s1", 40, 120));
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
        assert!(app.resize_pty_if_permitted("s1", 50, 150));

        assert!(
            !seat.owners.accept_grid_apply("s1", queued, 24, 80),
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
        app_with_a_live_pty_running("printf ready; sleep 5")
    }

    /// The same fixture with the child's script named, so a test that needs real
    /// scrollback to scroll into can ask for a chatty one.
    fn app_with_a_live_pty_running(
        script: &str,
    ) -> (
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
                &["-c".to_string(), script.to_string()],
                std::path::Path::new("."),
                10,
                10,
                100,
            )
            .expect("spawn pty"),
        );
        (app, recorded, seat)
    }

    /// A real `User-Agent`, exactly as a browser presents one and exactly as the
    /// registry records it. Mirrored from the web's own `deviceLabel` fixtures.
    const REAL_CHROME_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
         AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

    /// Render the whole app with a browser holding the focused pty, and hand back
    /// the frame as text. Shared by the two demoted-cue tests, which differ only
    /// in what the driving connection called itself.
    fn render_with_a_browser_driving(device: &str) -> (App, String) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (mut app, _recorded, seat) = app_with_a_live_pty();
        let browser = seat.owners.next_conn_id();
        seat.owners
            .claim_for_resize("session-1", browser, false, Some(device), |_| {})
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
        (app, rendered)
    }

    /// THE DEMOTED TREATMENT, rendered, with a REAL `User-Agent` on the wire. A
    /// browser holds the pty, so the center pane's hint bar stops listing keys
    /// that go nowhere and says who is driving and how to take it back, and the
    /// child is NOT re-gridded to this pane.
    ///
    /// The UA is the point of the fixture. The registry records whatever the
    /// browser presented, which is ~130 characters, and the cue is one line
    /// inside a pane narrower than the window: rendered raw it pushed the command
    /// name off the right edge, so the cue named a problem with no way out of it.
    ///
    /// A real render rather than a call to the cue builder, because the bug this
    /// guards against is the branch never being reached: a hint ladder that still
    /// picks "typing goes to the agent" is a screen telling the user their keys
    /// are landing when every one of them is being dropped.
    #[test]
    fn a_demoted_pane_shortens_a_real_user_agent_and_leaves_the_child_grid_alone() {
        let (app, rendered) = render_with_a_browser_driving(REAL_CHROME_UA);

        assert!(
            rendered.contains("Chrome on macOS is driving this agent"),
            "the hint bar must name the device that holds the pty, shortened"
        );
        assert!(
            !rendered.contains("Mozilla/5.0"),
            "the raw User-Agent must never reach the frame: it does not fit"
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

    /// The cue is built to the width it is given, and what gives way is the
    /// DEVICE NAME: the way out has to survive every pane this can be rendered in.
    #[test]
    fn the_cue_fits_the_pane_it_is_given_and_keeps_its_way_out() {
        let app = test_app(default_bindings());
        for width in [46u16, 51, 60, 80, 89, 120] {
            let line = app.remote_driver_cue_line(REAL_CHROME_UA, width);
            let rendered: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert!(
                rendered.chars().count() <= width as usize,
                "the cue must fit {width} columns, got {} for {rendered:?}",
                rendered.chars().count()
            );
            assert!(
                rendered.contains("take-over-terminal"),
                "the way out must survive at {width} columns: {rendered:?}"
            );
        }
    }

    /// A driver that already calls itself something short is named verbatim, and
    /// the cue still carries its way out. Kept alongside the real-UA test because
    /// the shortener must not mangle a name that already fits.
    #[test]
    fn a_demoted_pane_names_a_short_device_verbatim() {
        let (_app, rendered) = render_with_a_browser_driving(TUI_DEVICE_LABEL);

        assert!(
            rendered.contains("the dux TUI is driving this agent"),
            "a label that already fits is copy, not something to parse"
        );
        assert!(rendered.contains("take-over-terminal"));
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

    /// THE TRAP, end to end. A browser drives the agent, re-grids the child to a
    /// phone's shape, and goes away. This surface types, which claims the pty. The
    /// child must end up at THIS window's geometry.
    ///
    /// Every step is the real one: a real claim, a real refusal, a real render
    /// pass measuring a real pane. What made this permanent was two bugs meeting.
    /// The refused resize recorded the dedupe, so the pane believed it had already
    /// sent this geometry to this target; and the claim by typing cleared nothing,
    /// so nothing ever asked again. The user was left driving a phone-sized child
    /// with no cue on screen, because the demoted cue goes the moment the claim
    /// succeeds.
    #[test]
    fn claiming_by_typing_after_a_foreign_re_grid_heals_the_childs_geometry() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (mut app, _recorded, seat) = app_with_a_live_pty();
        app.input_target = InputTarget::Agent;
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("terminal");

        // The browser claims the pty and re-grids the child to its own shape.
        let browser = seat.owners.next_conn_id();
        let browser_seq = seat
            .owners
            .claim_for_resize("session-1", browser, false, Some(REAL_CHROME_UA), |_| {})
            .seq
            .expect("the browser claimed the pty");
        assert!(
            seat.owners
                .accept_grid_apply("session-1", browser_seq, 40, 20)
        );
        app.engine
            .providers
            .get("session-1")
            .expect("the pty is live")
            .resize(40, 20)
            .expect("the browser's resize reaches the child");

        // This surface renders while demoted: its resize is refused, and the
        // child keeps the browser's grid.
        terminal
            .draw(|frame| app.render(frame))
            .expect("render succeeds");
        let demoted_grid = app
            .engine
            .providers
            .get("session-1")
            .and_then(|client| client.grid_size());
        assert_eq!(
            demoted_grid,
            Some((40, 20)),
            "a demoted pane must leave the driver's grid alone"
        );

        // The browser goes away, so the pty is unowned again.
        assert!(seat.owners.release("session-1", browser).is_some());

        // The user types. That claims the pty, and the next render must send this
        // pane's geometry even though the pane has not changed size at all.
        app.process_raw_input_bytes(b"x")
            .expect("the keystroke is handled");
        assert!(
            seat.owners.is_owner("session-1", seat.conn_id),
            "typing into an unowned pty claims it"
        );
        terminal
            .draw(|frame| app.render(frame))
            .expect("render succeeds");

        let healed = app
            .engine
            .providers
            .get("session-1")
            .and_then(|client| client.grid_size())
            .expect("the pty is live");
        assert_ne!(
            healed,
            (40, 20),
            "the child is still sized for a device that let go: the claim has to \
             clear the resize dedupe, and a refused resize must not record it"
        );
    }

    /// The take-over ACTION's own three transitions, driven through the action
    /// rather than by arming the intent by hand.
    #[test]
    fn the_take_over_action_arms_or_retargets_depending_on_who_drives() {
        // ANOTHER DEVICE drives it: the intent is armed and the message names the
        // device, and the dedupe is cleared so the claim's resize really is sent.
        let (mut app, _recorded, seat) = app_with_a_live_pty();
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("session-1", browser).expect("claimed");
        app.last_pty_resize_target = Some("session-1".to_string());
        app.take_over_focused_pty();
        assert_eq!(app.pending_pty_takeover.as_deref(), Some("session-1"));
        assert_eq!(app.last_pty_resize_target, None);
        let (_, message) = app.status.most_recent_tui().expect("a status was set");
        assert!(
            message.contains("Chrome on macOS") || message.contains("another device"),
            "the message must name whoever is losing the terminal: {message}"
        );

        // NOBODY drives it: still armed, so watchers are told who took it.
        let (mut app, _recorded, _seat) = app_with_a_live_pty();
        app.take_over_focused_pty();
        assert_eq!(app.pending_pty_takeover.as_deref(), Some("session-1"));

        // ALREADY OURS: nothing to arm, but the dedupe is cleared so the command
        // is still the way to retarget the child's geometry at this window.
        let (mut app, _recorded, seat) = app_with_a_live_pty();
        assert!(app.may_type_into_pty("session-1"));
        assert!(seat.owners.is_owner("session-1", seat.conn_id));
        app.last_pty_resize_target = Some("session-1".to_string());
        app.take_over_focused_pty();
        assert_eq!(
            app.pending_pty_takeover, None,
            "there is no ownership to move, so nothing is armed"
        );
        assert_eq!(
            app.last_pty_resize_target, None,
            "but the geometry can still be retargeted, which is what the dedupe \
             clear is for"
        );
    }

    /// An armed take-over is SPENT OR DROPPED by the first render after arming.
    /// Left alive it fires later on a pane the user has moved away from, taking a
    /// terminal away from whichever device is driving it by then.
    #[test]
    fn an_armed_take_over_is_dropped_when_the_user_moves_to_another_terminal() {
        let (mut app, _recorded, _seat) = app_with_a_live_pty();
        app.take_over_focused_pty();
        assert_eq!(app.pending_pty_takeover.as_deref(), Some("session-1"));

        // The render pass is about a different pty now.
        app.expire_stale_pty_takeover(Some("session-2"));
        assert_eq!(
            app.pending_pty_takeover, None,
            "an arm for a pane the user left must not survive to fire later"
        );
        let (_, message) = app.status.most_recent_tui().expect("a status was set");
        assert!(
            message.contains("take-over-terminal"),
            "the drop must say how to ask again: {message}"
        );

        // And a render pass about the armed pty leaves it alone, or the intent
        // could never be spent at all.
        app.pending_pty_takeover = Some("session-1".to_string());
        app.expire_stale_pty_takeover(Some("session-1"));
        assert_eq!(app.pending_pty_takeover.as_deref(), Some("session-1"));
    }

    /// Ctrl-g in interactive mode: the default ToggleFullscreen binding, which
    /// leaves fullscreen and writes NOTHING to the child.
    const TOGGLE_FULLSCREEN_BYTES: &[u8] = &[0x07];
    /// The PageUp key as the host sends it, which is the default ScrollPageUp
    /// binding.
    const PAGE_UP_BYTES: &[u8] = b"\x1b[5~";

    /// A KEYSTROKE THAT WRITES NOTHING MUST CLAIM NOTHING. Fullscreen is a dux
    /// concern: the child never sees the key.
    ///
    /// Claiming on it is not a cosmetic slip. The claim is broadcast, so every
    /// browser watching that pty flips to a take-over card and starts dropping
    /// its own keystrokes, from a keypress that never touched the child.
    #[test]
    fn a_fullscreen_toggle_claims_nothing_because_it_writes_nothing() {
        let (mut app, recorded, seat) = app_with_a_live_pty();
        app.input_target = InputTarget::Agent;

        app.process_raw_input_bytes(TOGGLE_FULLSCREEN_BYTES)
            .expect("the toggle is handled");

        assert!(
            !seat.owners.is_owner("session-1", seat.conn_id),
            "a key the child never sees must not make this surface the driver"
        );
        assert!(
            recorded.lock().expect("not poisoned").published.is_empty(),
            "and nothing may be announced, or watchers lose their keyboards to it"
        );
    }

    /// The same rule for a page key that dux answers itself by scrolling the
    /// local scrollback. Entering scroll mode is looking, not typing.
    #[test]
    fn a_page_key_that_scrolls_locally_claims_nothing() {
        let (mut app, recorded, seat) = app_with_a_live_pty();
        app.input_target = InputTarget::Agent;
        // The local scroll needs a page height to scroll by.
        app.last_pty_size = (10, 10);

        app.process_raw_input_bytes(PAGE_UP_BYTES)
            .expect("the page key is handled");

        assert!(
            !seat.owners.is_owner("session-1", seat.conn_id),
            "scrolling back is not driving the terminal"
        );
        assert!(recorded.lock().expect("not poisoned").published.is_empty());
    }

    /// The control: a real keystroke still claims an unowned pty and announces
    /// it. Without this the two tests above would pass on a gate that never runs.
    #[test]
    fn a_real_keystroke_still_claims_an_unowned_pty() {
        let (mut app, recorded, seat) = app_with_a_live_pty();
        app.input_target = InputTarget::Agent;

        app.process_raw_input_bytes(b"x")
            .expect("the keystroke is handled");

        assert!(
            seat.owners.is_owner("session-1", seat.conn_id),
            "typing into a pty nobody drives is how this surface claims it"
        );
        let published = recorded.lock().expect("not poisoned").published.clone();
        assert!(
            matches!(
                published.first(),
                Some(PtyOwnershipEvent::Claimed { pty_id, .. }) if pty_id == "session-1"
            ),
            "the claim must be announced: {published:?}"
        );
    }

    /// A page key that IS forwarded consults the gate at the forward site, so a
    /// demoted pane does not write it.
    ///
    /// The interesting case is being scrolled back at the same time: the batch's
    /// scroll-mode short circuit says "nothing in this batch reaches the child",
    /// which is true of the batched forwards and NOT true of a page key the
    /// provider's `forward_scroll` says to send. Reusing that verdict here sent a
    /// demoted surface's page keys straight into somebody else's terminal.
    /// The demoted cue names the palette key as the way out, so that key must
    /// work where the cue shows: in fullscreen the palette chord normally rides
    /// to the child verbatim, but a demoted pane has nowhere to send it, and
    /// dux owns it instead. With nobody else driving, the same bytes still
    /// reach the child untouched.
    #[test]
    fn the_palette_chord_opens_the_palette_over_a_demoted_fullscreen_pane() {
        for demoted in [true, false] {
            let (mut app, _recorded, seat) = app_with_a_live_pty_running("sleep 5");
            app.input_target = InputTarget::Agent;
            app.last_pty_size = (10, 10);
            if demoted {
                let browser = seat.owners.next_conn_id();
                seat.owners.claim("session-1", browser).expect("claimed");
            }
            let palette_key = app.bindings.label_for(Action::OpenPalette);
            assert_eq!(
                palette_key, "Ctrl-p",
                "test setup: the default palette chord"
            );

            app.process_raw_input_bytes(b"\x10")
                .expect("the chord is handled");

            assert_eq!(
                matches!(app.prompt, PromptState::Command { .. }),
                demoted,
                "the palette chord must open the palette exactly when this surface \
                 cannot write to the child (demoted: {demoted})"
            );
            assert_eq!(
                app.engine.is_typing("session-1"),
                !demoted,
                "the chord must reach the child exactly when this surface may write \
                 to it (demoted: {demoted})"
            );
        }
    }

    #[test]
    fn a_demoted_pane_does_not_forward_a_page_key_even_while_scrolled_back() {
        for demoted in [true, false] {
            let (mut app, _recorded, seat) = app_with_a_live_pty_running(
                "i=1; while [ $i -le 60 ]; do echo line$i; i=$((i+1)); done; sleep 5",
            );
            app.input_target = InputTarget::Agent;
            app.last_pty_size = (10, 10);
            // This provider forwards page keys unconditionally, which is the one
            // setting under which the forward site is reached at all.
            let mut provider = dux_core::config::ProviderCommandConfig {
                command: "codex".to_string(),
                ..Default::default()
            };
            provider.forward_scroll = Some(true);
            app.engine
                .config
                .providers
                .commands
                .insert("codex".to_string(), provider);
            crate::app::test_support::enter_scroll_mode(&mut app, 5);

            if demoted {
                let browser = seat.owners.next_conn_id();
                seat.owners.claim("session-1", browser).expect("claimed");
            }

            app.process_raw_input_bytes(PAGE_UP_BYTES)
                .expect("the page key is handled");

            assert_eq!(
                app.engine.is_typing("session-1"),
                !demoted,
                "a page key must reach the child only when this surface may write \
                 to it (demoted: {demoted})"
            );
        }
    }
}

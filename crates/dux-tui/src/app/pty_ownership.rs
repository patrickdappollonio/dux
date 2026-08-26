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
//! WRITING IS WHAT IS GATED. Ownership is about who may type into a child and
//! who decides its grid, and nothing else: the pty keeps streaming, and a
//! surface that is not driving one still receives every byte of it.
//!
//! What that surface SHOWS is a separate decision, and this one is the web's:
//! whenever the terminal in the center pane is NOT this surface's, the pane is
//! covered by the take-over card, the same card and the same words a browser
//! puts over its own terminal. All three of the web's states, word for word:
//! `Open on {device}` and `Active on another device` for a pty somebody else
//! drives, and `Take control` for one nobody drives. One deviation from the
//! web's, deliberate: the web suppresses its card while its socket is lost,
//! which has no counterpart here, because there is no socket between this
//! surface and its own engine to lose.
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
//! other device going quiet, and not typing into it either. The one way back is
//! the card's button, which is also the only way IN to a pty nobody drives. The
//! web's socket-specific self-succession rule has no counterpart here, because
//! this surface has no socket to have a ghost of.
//!
//! ## Drawing a pane claims nothing
//!
//! Not even a pty nobody owns. The render pass measures the center pane and
//! sends the child a resize, and a resize is a claim wherever it is allowed to
//! be one; but this surface draws every agent the workspace grows, including the
//! ones a browser just created and is a heartbeat away from attaching to. So a
//! render's resize applies only for a pty this surface already drives, or one an
//! armed take-over is transferring, and the FIRST claim always comes from a
//! deliberate act. There are exactly two of them: the card's button, and a
//! LAUNCH this surface started (creating an agent, forking one, opening a pull
//! request, adding a tab, spawning a terminal). Each clears the resize dedupe,
//! so the geometry follows on the next frame through the ordinary apply order.
//!
//! TYPING IS NOT ONE OF THEM. A keystroke into a pty this surface does not drive
//! is dropped, whoever holds it and even when nobody does, because the card is
//! already covering that pane and asking for the press. The cost is stated
//! rather than hidden: the startup auto-reopen sweep claims nothing, so every
//! agent reopened at startup shows `Take control` until somebody presses it,
//! which is exactly what a browser shows for a terminal it did not start. An
//! agent launched from this keyboard is this surface's immediately, with no card
//! over it at all.
//!
//! WHICH LAUNCHES COUNT, decided in [`launch_claims_its_pty`]. A launch claims
//! when somebody at this keyboard asked for it: creating an agent (a fork, a
//! pull request and a standalone agent included, which are all creates),
//! reconnecting or force-reconnecting a dormant one, opening a tab, and
//! spawning a terminal. The startup auto-reopen sweep does NOT: nobody has
//! touched anything yet, the web server's own startup pass claims nothing
//! either, and claiming there would hand this terminal every reopened agent in
//! the workspace at once, each of which a browser would then have to take back
//! by hand. A create is armed by a flag rather than by id, because its session
//! id is minted in a worker, and it is armed only once the engine has ACCEPTED
//! the dispatch: a refused create that armed anyway would spend its arm on the
//! create that really was in flight, which is the browser's.

use dux_core::background_serve::{PtyOwnershipEvent, TUI_DEVICE_LABEL, TuiOwnership};

use super::*;

/// Who is driving a pty, from this surface's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PtyDriver {
    /// Nobody has claimed it, or nothing is serving so the question does not
    /// arise. While something IS serving this is a card state like any other
    /// (`Take control`), and the two acts that claim it are the card's button
    /// and a launch started from here. Typing does not, merely looking does
    /// not, and neither does the resize that drawing its pane would send.
    Free,
    /// This surface holds it.
    Mine,
    /// Another device holds it. Typing and resizing are refused until an
    /// explicit take-over.
    ///
    /// `device` is `None` when the driver gave dux no name for itself, which a
    /// browser that presented no `User-Agent` at its upgrade really does. The
    /// ABSENCE is carried rather than a stand-in string, because the card has a
    /// different title for it: a screen that prints a sentinel where a device
    /// name goes reads as a device called that.
    ///
    /// A name that is there is SHORT. The registry records what the driver
    /// presented, which for a browser is a raw `User-Agent` of well over a
    /// hundred characters; this carries the label
    /// [`dux_core::device_label::short_device_label`] made of it, because the
    /// place it is rendered is the title bar of a card inside the center pane.
    Elsewhere { device: Option<String> },
}

/// What the take-over card over the shown pane is saying, when there is one.
///
/// One variant per title the web's card has (see `TerminalPane.tsx`), because
/// the two surfaces show the same three truths about the same registry. `None`
/// from [`App::focused_pty_takeover_card`] is the whole absence of a card: this
/// surface drives the pty, nothing is serving, or there is no live pty here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PtyTakeoverCard {
    /// Another device drives it. `device` is what that device called itself,
    /// `None` when it gave dux no name; the card has a title for each.
    Elsewhere { device: Option<String> },
    /// Nobody drives it. The card names the ACT rather than the absence, exactly
    /// as the web's third title does: "Active on another device" would name a
    /// device that is not there.
    Free,
}

/// Whether a launch of this kind claims the child it produces for this surface.
///
/// A launch claims when somebody at this keyboard asked for it, and only then.
/// Matched exhaustively rather than by a `matches!` at the call site, so a new
/// launch kind does not compile until somebody has said which it is.
pub(crate) fn launch_claims_its_pty(kind: &dux_core::worker::AgentLaunchKind) -> bool {
    use dux_core::worker::AgentLaunchKind;
    match kind {
        // NOBODY ACTED. The startup sweep reopens what was running last time,
        // before the user has touched anything, and the web server's own startup
        // pass claims nothing either. Claiming here would hand this terminal
        // every auto-reopened agent in the workspace the moment it starts, and
        // ownership is sticky, so a browser would have to take each one back by
        // hand. Each of them wears the `Take control` card instead, and the
        // press on it is what claims, exactly as it is for any other free pty.
        AgentLaunchKind::StartupAutoReopen => false,
        // A person asked for each of these: a create (including a fork, a pull
        // request and a standalone agent, which are all Create-kind), a
        // reconnect or forced reconnect of a dormant agent, and a new or
        // relaunched tab.
        AgentLaunchKind::Create { .. }
        | AgentLaunchKind::Reconnect { .. }
        | AgentLaunchKind::ForceReconnect { .. }
        | AgentLaunchKind::Tab { .. } => true,
        // The tail of a launch somebody DID ask for: the provider refused to
        // resume, so dux relaunches it fresh. dux-core dispatches this one
        // directly, so it never reaches this surface's dispatch and the answer
        // is moot in practice; it is `true` because the act it finishes was the
        // user's, not because anything here acts on it.
        AgentLaunchKind::ResumeFallback { .. } => true,
    }
}

/// What PROSE calls a driver that gave dux no name for itself.
///
/// The status line has to finish its sentence, so it needs a noun phrase where
/// the card simply changes its title. Naming it honestly beats naming it wrongly.
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
            // becomes something a screen renders, rather than at the card: every
            // reader of this verdict (the card today, anything later) then gets
            // a label that fits by construction.
            Some(_) => PtyDriver::Elsewhere {
                device: device
                    .as_deref()
                    .and_then(dux_core::device_label::short_device_label),
            },
        }
    }

    /// WHICH CARD, if any, is over the shown pane.
    ///
    /// The whole rule in one place: while a seat exists, every pty that is not
    /// this surface's is covered, and which sentence it is covered with follows
    /// from who holds it. With no seat there is no registry, no question and no
    /// card, which is what keeps this surface exactly what it was before it
    /// joined the ownership model.
    ///
    /// Asked of the live registry on every frame rather than of a latched flag:
    /// ownership moves between devices while nothing on this surface happens.
    pub(crate) fn focused_pty_takeover_card(&self) -> Option<PtyTakeoverCard> {
        self.pty_ownership()?;
        let pty_id = self.selected_terminal_surface_id()?;
        match self.pty_driver(&pty_id) {
            PtyDriver::Mine => None,
            PtyDriver::Free => Some(PtyTakeoverCard::Free),
            PtyDriver::Elsewhere { device } => Some(PtyTakeoverCard::Elsewhere { device }),
        }
    }

    /// The same question with the sentence thrown away, for the gates that only
    /// need to know whether the card is between the keyboard and the child.
    pub(crate) fn focused_pty_is_covered_by_card(&self) -> bool {
        self.focused_pty_takeover_card().is_some()
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
    /// It CLAIMS NOTHING. While a seat exists this surface may write only to a
    /// pty it already drives; every other write is DROPPED (logged at debug,
    /// like the web's dropped non-owner keystroke), and the take-over card
    /// covering the pane is what tells the user why. That is true of a pty
    /// nobody drives as well as one another device holds: the card is up in both
    /// states, and pressing its button is the deliberate act that claims. With
    /// nothing serving there is no seat and no gate, and every write is allowed.
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
        let allowed = seat.owners.is_owner(pty_id, seat.conn_id);
        if !allowed {
            // The two refusals are one rung with two stories, and the card over
            // the pane is already telling whichever is true.
            let (owner, _, _) = seat.owners.current_owner(pty_id);
            let reason = if owner.is_some() {
                "another device currently owns its input"
            } else {
                "nobody is driving it yet, and typing does not claim one: press the card's \
                 Take over button"
            };
            dux_core::logger::debug(&format!("keystroke for pty {pty_id} dropped: {reason}"));
        }
        allowed
    }

    /// Claim `pty_id` because THIS surface just started the child behind it.
    ///
    /// A launch is a deliberate act by the person at this keyboard, so it claims
    /// the pty, which is what spares them the card over a terminal they just
    /// started here: unclaimed, an agent launched from this keyboard would come
    /// up asking to be taken control of, and no window resize would reach its
    /// child until they had pressed the button.
    ///
    /// It never steals. A pty another device already drives is left alone, so a
    /// relaunch of a terminal a browser is driving does not quietly move the
    /// driver's seat to this window.
    pub(crate) fn claim_launched_pty(&mut self, pty_id: &str) {
        let Some(seat) = self.pty_ownership() else {
            return;
        };
        let claim = seat
            .owners
            .may_write(pty_id, seat.conn_id, Some(TUI_DEVICE_LABEL));
        if claim.claimed_new
            && let Some(epoch) = claim.epoch
        {
            // A launch's claim TRANSFERS the pty to this surface, so it gets the
            // same treatment the explicit take-over gets: the resize dedupe is
            // cleared, which is what makes the next render send this pane's
            // geometry to a child a previous driver may have re-gridded.
            //
            // Without it the trap is: a browser re-grids the child to a phone's
            // shape and disconnects, this surface relaunches and claims, and the
            // dedupe sees the same pane size against the same target and sends
            // nothing. The terminal then owns a phone-sized child indefinitely,
            // with nothing on screen to say so, because the take-over card is gone
            // the moment the claim succeeds.
            self.last_pty_resize_target = None;
            self.publish_ownership(&[PtyOwnershipEvent::Claimed {
                pty_id: pty_id.to_string(),
                conn_id: seat.conn_id,
                epoch,
                device: TUI_DEVICE_LABEL.to_string(),
            }]);
        }
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
    /// It never CLAIMS on its own. An unowned pty is refused here exactly as an
    /// owned one is, because the caller is the render pass and drawing a pane is
    /// not a decision to drive what is in it; the deliberate acts (the card's
    /// button and [`Self::claim_launched_pty`]) are what take a free pty, and
    /// only a take-over armed by that button takes one from another device.
    ///
    /// Returns whether the resize was granted, which is the caller's cue to
    /// record its dedupe. A refusal records nothing: the pane renders the
    /// authoritative grid instead, which it already does safely, and the card
    /// covering it names the device whose grid that is.
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
        // MERELY DRAWING A PANE IS NOT A CLAIM. This is asked from the render
        // pass, which runs whenever the pane's geometry or its target changes,
        // and a browser starting an agent changes both here: the new agent
        // arrives, the selection moves onto it, and this surface draws it. A
        // resize that claimed an unowned pty would make this terminal the driver
        // of a child the browser is about to attach to, and attaching never
        // steals, so the person who started the agent would be handed a take-over
        // card over their own new terminal.
        //
        // So a plain resize applies only for a pty this surface ALREADY drives,
        // or one an armed take-over is about to transfer. This surface's first
        // claim of a free pty is a deliberate act instead: pressing the card's
        // button, or starting the child from here (`claim_launched_pty`). Typing
        // is not one of them and reaches nothing while the card is up. Both acts
        // clear the resize dedupe, so the very next render sends this pane's
        // geometry through the ordinary apply order below.
        if !takeover && !seat.owners.is_owner(pty_id, seat.conn_id) {
            self.log_refused_resize_once(&seat, pty_id, rows, cols);
            return false;
        }
        // No `expected_owner`: this surface's take-over is always a deliberate
        // press of the card's button, never a delayed ghost succession, so
        // there is no predecessor to compare against.
        let outcome = seat.owners.claim_for_resize(
            pty_id,
            seat.conn_id,
            takeover,
            None,
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
            // Reachable only as a race: the ownership check above passed and
            // another device took the pty between it and this claim.
            self.log_refused_resize_once(&seat, pty_id, rows, cols);
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

    /// Say in the log why a resize was not sent, but only when it is NEW
    /// information.
    ///
    /// This is asked from the render pass, which repeats for as long as the pane
    /// is on screen, and a refused resize deliberately does not record the resize
    /// dedupe (recording it makes a stale geometry permanent). Without the guard
    /// that is tens of identical lines a second.
    ///
    /// The two refusals are different facts and get different sentences. Both are
    /// under a card, but reporting a pty nobody drives as another device's doing
    /// would be a lie about the user's own setup.
    fn log_refused_resize_once(&mut self, seat: &TuiOwnership, pty_id: &str, rows: u16, cols: u16) {
        let refusal = (pty_id.to_string(), rows, cols);
        if self.last_refused_pty_resize.as_ref() == Some(&refusal) {
            return;
        }
        let (owner, _, _) = seat.owners.current_owner(pty_id);
        let reason = if owner.is_some() {
            "another device currently owns its sizing, and a take-over must say so explicitly"
        } else {
            "nobody is driving it yet, and drawing a pane does not claim one: press Take over \
             on the card covering it, or let the device that starts driving it size it"
        };
        dux_core::logger::debug(&format!("resize of pty {pty_id} refused: {reason}"));
        self.last_refused_pty_resize = Some(refusal);
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
             could be claimed. Go back to the one you want and press Take over on the card \
             covering it."
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
    ///
    /// ONE STATE CAN REACH THIS: the card's button, and the card is on screen
    /// exactly while the shown pty is not this surface's. So a pty this surface
    /// already drives, and a pane with no pty at all, return QUIETLY rather than
    /// explaining themselves. There is no palette command behind this any more,
    /// so a status line about a terminal that is already yours would be dux
    /// answering a question nobody asked.
    ///
    /// BOTH CARD STATES ARRIVE HERE and arm the same intent, because both are
    /// the same act against the registry: a flagged claim, which an unowned pty
    /// grants outright and an owned one transfers. Only the sentence differs,
    /// because only one of them takes something away from somebody.
    pub(crate) fn take_over_focused_pty(&mut self) {
        let Some(card) = self.focused_pty_takeover_card() else {
            return;
        };
        let Some(pty_id) = self.selected_terminal_surface_id() else {
            return;
        };
        if self.pending_pty_takeover.as_deref() == Some(pty_id.as_str()) {
            // Already asked for, and the arm is spent by the very next render.
            // A second message would be dux reporting one act twice at a user
            // who pressed a button that has not visibly done anything yet.
            return;
        }
        let message = match card {
            PtyTakeoverCard::Elsewhere { device } => {
                let device = device.unwrap_or_else(|| UNNAMED_DEVICE.to_string());
                format!(
                    "Taking this terminal over from {device}. Typing here reaches it again, its \
                     size follows this window, and that device keeps watching without being able \
                     to type."
                )
            }
            // Nobody is losing anything here, so the sentence says what this
            // window gains rather than who it was taken from.
            //
            // The arm is still a FLAGGED claim, which matters in the one race
            // this state has: a browser's plain attach can land between the
            // press and the render that carries it, and the flag transfers the
            // pty anyway rather than losing to it. That is parity, not a
            // special power, because the web flags its own take-over claim for
            // exactly the same reason.
            PtyTakeoverCard::Free => "Taking control of this terminal. Nobody was driving it, so \
                 typing here reaches it now and its size follows this window."
                .to_string(),
        };
        self.set_info(message);
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
    use dux_core::engine::{AgentLaunchReadyOutcome, AgentLaunchReadyView};

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
        assert_eq!(app.focused_pty_takeover_card(), None);
        // Repeated calls stay open: with no registry there is no state to
        // accumulate and nothing that can start refusing.
        assert!(app.may_type_into_pty("s1"));
        assert!(app.resize_pty_if_permitted("s1", 30, 100));
    }

    /// A launch started here claims an unowned pty and announces it once.
    /// Without the announcement a browser watching the agent never learns this
    /// terminal is driving it.
    #[test]
    fn a_launch_claims_an_unowned_pty_and_announces_it_once() {
        let (mut app, recorded, seat) = serving_app();

        app.claim_launched_pty("s1");
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

        // And a relaunch of a pty this surface already drives announces nothing
        // either: there is no handover to report.
        app.claim_launched_pty("s1");
        assert_eq!(recorded.lock().expect("not poisoned").published.len(), 1);
    }

    /// A browser is typing into the agent, so this surface's keystrokes are
    /// DROPPED rather than written, and the take-over card has a device to
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
                device: Some("Chrome on macOS".to_string())
            },
            "the card names the driving device from the registry, shortened to \
             something that fits the card's title bar"
        );
    }

    /// A driver that gave dux no name for itself is recorded as HAVING no name,
    /// rather than being handed a sentinel string that a title would then print.
    /// The card's second title exists for exactly this answer.
    #[test]
    fn an_unnamed_driver_is_recorded_as_nameless_rather_than_given_a_sentinel() {
        let (app, _recorded, seat) = serving_app();
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("s1", browser).expect("claimed");
        assert_eq!(app.pty_driver("s1"), PtyDriver::Elsewhere { device: None });
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
        // The claim is a deliberate act (here, a launch started on this surface);
        // the resize that follows is the ordinary one the render pass sends.
        app.claim_launched_pty("session-1");

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
        app.claim_launched_pty("gone");

        assert!(
            app.resize_pty_if_permitted("gone", 24, 80),
            "the resize itself is still granted: this surface owns this pty"
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
            .claim_for_resize("s1", browser, false, None, Some("Chrome"), |_| {})
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
        app.claim_launched_pty("s1");
        app.claim_launched_pty("s2");
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
        app.claim_launched_pty("mine");
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

    /// Wait until the child has actually painted something, so a card test is
    /// really covering a live grid rather than the loading card.
    fn wait_for_child_output(app: &App) {
        for _ in 0..300 {
            if app
                .engine
                .providers
                .get("session-1")
                .is_some_and(|client| client.has_output())
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the child under test produced no output within 3s");
    }

    /// Read a drawn frame back as one string per screen row.
    ///
    /// Row by row rather than one flat string, because the card's prose is
    /// centred and wrapped: a flat string welds the end of one row onto the
    /// start of the next and onto whatever the panes beside it are showing.
    fn rows_of(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// Draw the whole app at `width` x `height` and hand back its rows.
    fn render_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render succeeds");
        rows_of(&terminal)
    }

    /// The card's prose is wrapped, centred and boxed, so a test that wants to
    /// compare it with the web's sentence has to undo all three: drop the box
    /// drawing, join the rows and squeeze the padding back to single spaces.
    fn flowed(rows: &[String]) -> String {
        rows.iter()
            .flat_map(|row| row.chars())
            .map(|ch| {
                if ('\u{2500}'..='\u{257f}').contains(&ch) {
                    ' '
                } else {
                    ch
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A real `User-Agent`, exactly as a browser presents one and exactly as the
    /// registry records it. Mirrored from the web's own `deviceLabel` fixtures.
    const REAL_CHROME_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
         AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

    /// Something the child prints that no piece of dux chrome ever would, so a
    /// test can say "none of the child's cells are on screen" and mean it.
    const CHILD_MARKER: &str = "CHILDMARKER";

    /// An app with a live, chatty child that a browser is driving, drawn once.
    /// `device` is what that browser presented at its upgrade; `None` is the
    /// browser that presented nothing.
    fn render_with_a_browser_driving(device: Option<&str>) -> (App, Vec<String>) {
        let (mut app, _recorded, seat) =
            app_with_a_live_pty_running(&format!("printf {CHILD_MARKER}; sleep 5"));
        let browser = seat.owners.next_conn_id();
        seat.owners
            .claim_for_resize("session-1", browser, false, None, device, |_| {})
            .epoch
            .expect("the browser claimed the pty");
        wait_for_child_output(&app);
        let rows = render_rows(&mut app, 160, 40);
        (app, rows)
    }

    /// THE TAKE-OVER CARD, rendered, with a REAL `User-Agent` on the wire. A
    /// browser holds the pty, so the pane it covers is the card's: the device
    /// that is driving, the web's sentence word for word, and the one button.
    ///
    /// The UA is the point of the fixture. The registry records whatever the
    /// browser presented, which is ~130 characters, and the title bar of a card
    /// inside the center pane cannot carry that; the shortener is what makes the
    /// title a name rather than a wall.
    ///
    /// A real render rather than a call to the card builder, because the bug this
    /// guards against is the branch never being reached: a pane that keeps
    /// showing the child while every keystroke is dropped is a screen telling the
    /// user their keys are landing.
    #[test]
    fn the_take_over_card_covers_the_pane_and_names_the_driving_device() {
        let (app, rows) = render_with_a_browser_driving(Some(REAL_CHROME_UA));
        let flat = flowed(&rows);

        assert!(
            flat.contains("Open on Chrome on macOS"),
            "the card's title must name the device that holds the pty, shortened: {flat}"
        );
        assert!(
            !flat.contains("Mozilla/5.0"),
            "the raw User-Agent must never reach the frame: it does not fit"
        );
        assert!(
            flat.contains(
                "Only one device can type at a time. Take over to drive this agent from here."
            ),
            "the description is the web's, word for word: {flat}"
        );
        assert!(
            flat.contains("Take over"),
            "the card carries the one button that takes the terminal back: {flat}"
        );
        assert!(
            !flat.contains(CHILD_MARKER),
            "the card covers the grid, so none of the child's cells may show: {flat}"
        );

        let grid = app
            .engine
            .providers
            .get("session-1")
            .and_then(|client| client.grid_size());
        assert_eq!(
            grid,
            Some((10, 10)),
            "a covered pane must leave the child at the grid its driver set"
        );
    }

    /// A driver that presented no name for itself gets the card's second title,
    /// which names the fact rather than printing a sentinel where a device
    /// should be.
    #[test]
    fn a_nameless_driver_gets_the_cards_second_title() {
        let (_app, rows) = render_with_a_browser_driving(None);
        let flat = flowed(&rows);

        assert!(
            flat.contains("Active on another device"),
            "a driver with no name still gets an honest title: {flat}"
        );
        assert!(
            !flat.contains("Open on"),
            "and never the named title with a blank in it: {flat}"
        );
        assert!(flat.contains("Take over"));
    }

    /// A driver that already calls itself something short is named verbatim.
    /// Kept alongside the real-UA test because the shortener must not mangle a
    /// name that already fits.
    #[test]
    fn the_card_names_a_short_device_verbatim() {
        let (_app, rows) = render_with_a_browser_driving(Some(TUI_DEVICE_LABEL));
        assert!(
            flowed(&rows).contains(&format!("Open on {TUI_DEVICE_LABEL}")),
            "a label that already fits is copy, not something to parse"
        );
    }

    /// The other half: driving it itself, the pane shows the CHILD and no card,
    /// and the resize lands. "The card never shows" and "the card always shows"
    /// would both pass the tests above on their own.
    #[test]
    fn a_driving_pane_shows_the_child_and_no_card() {
        let (mut app, _recorded, _seat) =
            app_with_a_live_pty_running(&format!("printf {CHILD_MARKER}; sleep 5"));
        // Driving it because this surface started it. Drawing the pane would not
        // have been enough, deliberately: see `claim_launched_pty`.
        app.claim_launched_pty("session-1");
        wait_for_child_output(&app);

        let rows = render_rows(&mut app, 160, 40);
        let flat = flowed(&rows);

        assert!(
            flat.contains(CHILD_MARKER),
            "nobody else holds this pty, so the child's own output is what shows: {flat}"
        );
        assert!(
            !flat.contains("Take over"),
            "and there is nothing to take over: {flat}"
        );
        let grid = app
            .engine
            .providers
            .get("session-1")
            .and_then(|client| client.grid_size());
        assert_ne!(
            grid,
            Some((10, 10)),
            "a pane that drives its pty sizes the child to itself"
        );
    }

    /// Nothing serving means no registry, no driver and no card: the pane is
    /// exactly what it was before this surface joined the ownership model.
    #[test]
    fn nothing_serving_shows_no_card_at_all() {
        let mut app = test_app(default_bindings());
        app.selected_left = 1;
        app.center_mode = CenterMode::Agent;
        app.session_surface = SessionSurface::Agent;
        app.engine.providers.insert(
            "session-1".to_string(),
            crate::pty::PtyClient::spawn(
                "sh",
                &["-c".to_string(), format!("printf {CHILD_MARKER}; sleep 5")],
                std::path::Path::new("."),
                10,
                10,
                100,
            )
            .expect("spawn pty"),
        );
        wait_for_child_output(&app);

        let flat = flowed(&render_rows(&mut app, 160, 40));
        assert!(flat.contains(CHILD_MARKER));
        assert!(!flat.contains("Take over"));
    }

    /// The card covers the FULLSCREEN pane too. Fullscreen is where a demoted
    /// terminal is most misleading: it is the whole screen, and every one of the
    /// keys it invites are being dropped.
    #[test]
    fn the_card_covers_a_fullscreen_pane_too() {
        let (mut app, _recorded, seat) =
            app_with_a_live_pty_running(&format!("printf {CHILD_MARKER}; sleep 5"));
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("session-1", browser).expect("claimed");
        wait_for_child_output(&app);
        app.fullscreen_overlay = FullscreenOverlay::Agent;

        let flat = flowed(&render_rows(&mut app, 160, 40));
        assert!(flat.contains("Take over"), "{flat}");
        assert!(!flat.contains(CHILD_MARKER), "{flat}");
    }

    /// PARITY WITH THE WEB, deliberately: the tab strip is OUTSIDE the pane the
    /// card covers there, so it stays visible and switchable here too. A user
    /// whose agent has two tabs can still move to the one nobody else is
    /// driving.
    #[test]
    fn the_tab_strip_still_renders_above_the_card() {
        let (mut app, _recorded, seat) =
            app_with_a_live_pty_running(&format!("printf {CHILD_MARKER}; sleep 5"));
        let session_id = app.engine.sessions[0].id.clone();
        app.engine.agent_tabs.insert(
            "tab-2".to_string(),
            dux_core::model::AgentTab {
                id: "tab-2".to_string(),
                session_id: session_id.clone(),
                provider: dux_core::model::ProviderKind::new("claude"),
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("session-1", browser).expect("claimed");
        wait_for_child_output(&app);

        let flat = flowed(&render_rows(&mut app, 160, 40));
        assert!(flat.contains("Take over"), "the card is up: {flat}");
        assert!(
            flat.contains("claude"),
            "the second tab's pill must still be on screen above the card: {flat}"
        );
    }

    /// The hint bar under the card names the key that presses the button, and
    /// nothing about the child: none of the child's keys reach it.
    #[test]
    fn the_cards_hint_bar_names_the_key_that_takes_over() {
        let (app, rows) = render_with_a_browser_driving(Some("Chrome"));
        let flat = flowed(&rows);
        let key = app.bindings.label_for(Action::FocusAgent);
        assert_eq!(key, "Enter", "test setup: the default FocusAgent binding");

        assert!(
            flat.contains(&format!("<{key}> take over")),
            "the hint must resolve the binding rather than hardcode a key: {flat}"
        );
        assert!(
            !flat.contains("Typing goes to the agent"),
            "a covered pane must not claim that typing reaches the agent: {flat}"
        );
    }

    /// The hint line is built to the width it is given, and what must survive
    /// every width is the way out: the key that presses the button.
    #[test]
    fn the_card_hint_fits_the_pane_it_is_given_and_keeps_its_way_out() {
        let app = test_app(default_bindings());
        for width in [46u16, 51, 60, 80, 120] {
            let line = app.takeover_hint_line(width);
            let rendered: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert!(
                rendered.chars().count() <= width as usize,
                "the hint must fit {width} columns, got {} for {rendered:?}",
                rendered.chars().count()
            );
            assert!(
                rendered.contains("take over"),
                "the way out must survive at {width} columns: {rendered:?}"
            );
        }
    }

    /// A pane too narrow for the prose keeps the BUTTON, and nothing panics on
    /// the way down. The button is the way out, so it is the half that survives;
    /// the arithmetic that centres a card is where an off-by-one becomes a crash.
    #[test]
    fn a_narrow_pane_keeps_the_button_and_never_panics() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::layout::Rect;

        for (w, h) in [(12u16, 10u16), (12, 3), (20, 4), (8, 2), (2, 1), (60, 12)] {
            let (mut app, _recorded, _seat) = serving_app();
            let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("terminal");
            terminal
                .draw(|frame| {
                    app.render_takeover_card(
                        frame,
                        Rect::new(0, 0, w, h),
                        &PtyTakeoverCard::Elsewhere {
                            device: Some("Chrome".to_string()),
                        },
                    );
                })
                .expect("render succeeds");
            let flat = flowed(&rows_of(&terminal));
            if w >= 12 && h >= 3 {
                assert!(
                    flat.contains("Take over"),
                    "a {w}x{h} pane must still carry the button: {flat:?}"
                );
            }
        }
    }

    /// AMENDMENT 3. The hardware cursor is the caret, and there is nothing to
    /// type into under the card, so it must not be parked on a cell the card is
    /// painted over: the IME anchor and the "your keys land here" cue both point
    /// at a pane that is refusing every key.
    #[test]
    fn the_card_leaves_the_hardware_cursor_at_the_origin() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        // The control first, on a terminal of its own: a backend remembers the
        // last position it was given, so re-drawing into the same one could not
        // tell "left at the origin" from "never moved".
        let (mut app, _recorded, _seat) = app_with_a_live_pty();
        app.focus = FocusPane::Center;
        app.input_target = InputTarget::Agent;
        app.claim_launched_pty("session-1");
        wait_for_child_output(&app);
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render succeeds");
        assert_ne!(
            ratatui::backend::Backend::get_cursor_position(terminal.backend_mut()).expect("cursor"),
            ratatui::layout::Position::new(0, 0),
            "test premise: a driving pane parks the caret on the child's cursor"
        );

        // Now the same pane with a browser driving it.
        let (mut app, _recorded, seat) = app_with_a_live_pty();
        app.focus = FocusPane::Center;
        app.input_target = InputTarget::Agent;
        wait_for_child_output(&app);
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("session-1", browser).expect("claimed");
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render succeeds");
        terminal.backend_mut().assert_cursor_position((0u16, 0u16));
    }

    /// AMENDMENT 9. Scroll mode is a way of reading a pane; the card covers the
    /// pane, so there is nothing left to read and the mode ends with it. Left on,
    /// the hint bar would be the scroll cue and the card would have no line of
    /// its own.
    #[test]
    fn the_card_ends_scroll_mode_and_owns_the_hint_line() {
        let (mut app, _recorded, seat) = app_with_a_live_pty_running(
            "i=1; while [ $i -le 60 ]; do echo line$i; i=$((i+1)); done; sleep 5",
        );
        app.focus = FocusPane::Center;
        app.last_pty_size = (10, 10);
        crate::app::test_support::enter_scroll_mode(&mut app, 5);
        assert!(app.scroll_mode_active(), "test setup: scrolled back");

        let browser = seat.owners.next_conn_id();
        seat.owners.claim("session-1", browser).expect("claimed");
        let flat = flowed(&render_rows(&mut app, 160, 40));

        assert!(
            !app.scroll_mode_active(),
            "the card covers what scroll mode was for, so the mode ends with it"
        );
        assert!(
            flat.contains("take over"),
            "the hint under the card is the card's: {flat}"
        );
        assert!(
            !flat.contains("Scrolled back"),
            "and not the scroll cue it replaced: {flat}"
        );
        assert_eq!(
            app.engine
                .providers
                .get("session-1")
                .map(|client| client.scrollback_offset()),
            Some(0),
            "the OFFSET has to go home with the mode. Retiring the mode alone \
             leaves the pane frozen on old history with nothing saying so, and \
             the next keystroke typing into a view that is not the live edge"
        );

        // And the pane really is live again once this surface drives it. The
        // browser letting go is not enough on its own: nothing passive claims,
        // so the card stays up saying "Take control" until it is pressed.
        assert!(seat.owners.release("session-1", browser).is_some());
        assert!(
            app.focused_pty_is_covered_by_card(),
            "a pty nobody drives is still covered"
        );
        app.claim_launched_pty("session-1");
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
            .expect("the key is handled");
        assert!(
            app.engine.is_typing("session-1"),
            "with the card gone and the offset home, typing reaches the child"
        );
    }

    /// RESIZE MODE WINS. It is a dux mode the user turned on themselves, its
    /// keys are arrows, and the card's rung sits above the ladder that runs it:
    /// unguarded, the card swallowed every arrow and answered Enter with a
    /// take-over the user was not asking for.
    #[test]
    fn resize_mode_wins_over_the_card() {
        let (mut app, _recorded, _seat) = app_with_the_card_up();
        app.resize_mode = true;
        let before = app.left_width_pct;

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
            .expect("the key is handled");
        assert_ne!(
            app.left_width_pct, before,
            "the arrow must still resize the panes while the card is up"
        );

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("the key is handled");
        assert_eq!(
            app.pending_pty_takeover, None,
            "and no key of resize mode's may arm a take-over"
        );
        assert!(
            app.resize_mode,
            "resize mode is left for its own key to end"
        );

        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL))
            .expect("the key is handled");
        assert!(!app.resize_mode);
        assert_eq!(app.pending_pty_takeover, None);
    }

    /// A CARRIAGE RETURN INSIDE A PASTE IS CONTENT. The card's key is matched on
    /// the raw byte stream, where a pasted body looks exactly like typing; a
    /// paste with a line break in it would otherwise take a terminal over from
    /// whoever is driving it, silently, on its second line.
    #[test]
    fn a_carriage_return_inside_a_paste_arms_nothing() {
        let (mut app, _recorded, _seat) = app_with_the_card_up();
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;

        let mut bytes = crate::raw_input::BRACKET_PASTE_START.to_vec();
        bytes.extend_from_slice(b"a\rb");
        // The palette chord is matched the same way and has the same hole.
        bytes.extend_from_slice(b"\x10");
        bytes.extend_from_slice(crate::raw_input::BRACKET_PASTE_END);
        app.process_raw_input_bytes(&bytes)
            .expect("the paste is handled");

        assert_eq!(
            app.pending_pty_takeover, None,
            "a line break in pasted text is content, never the card's key"
        );
        assert!(
            matches!(app.prompt, PromptState::None),
            "and neither is a pasted control byte the palette chord"
        );
    }

    /// A DRAG CAN PREDATE THE CARD: the user is selecting output when another
    /// device claims the pty. The release lands on a covered pane, so it must
    /// end the drag rather than leave a selection stuck to a hidden grid.
    #[test]
    fn a_release_under_the_card_ends_a_drag_that_predates_it() {
        use ratatui::layout::Rect;

        let (mut app, _recorded, seat) = app_with_a_live_pty_running("sleep 5");
        app.focus = FocusPane::Center;
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        app.mouse_layout.agent_term = Some(Rect::new(0, 0, 80, 20));
        // Driving it, so there is an uncovered pane to start the drag on.
        app.claim_launched_pty("session-1");

        app.process_raw_input_bytes(b"\x1b[<0;6;6M")
            .expect("the press is handled");
        assert!(
            app.terminal_selection
                .as_ref()
                .is_some_and(|selection| selection.dragging),
            "test setup: a drag is in flight before the card appears"
        );

        let browser = seat.owners.next_conn_id();
        seat.owners
            .claim_for_resize("session-1", browser, true, None, Some("Chrome"), |_| {})
            .epoch
            .expect("the browser takes the pty over mid-drag");
        app.process_raw_input_bytes(b"\x1b[<0;20;6m")
            .expect("the release is handled");

        assert!(
            app.terminal_selection.is_none(),
            "the card covers the text the selection was over, so the release \
             retires it rather than leaving a drag that never ends"
        );
    }

    /// Pressing the button twice says nothing the second time. The arm is
    /// already placed and spent by the next render, so a second message is dux
    /// reporting the same act twice at a user who pressed a button that had not
    /// visibly done anything yet.
    #[test]
    fn a_second_press_of_the_button_says_nothing_new() {
        let (mut app, _recorded, _seat) = app_with_the_card_up();

        app.take_over_focused_pty();
        assert_eq!(app.pending_pty_takeover.as_deref(), Some("session-1"));

        app.set_info("a marker nothing else writes".to_string());
        app.take_over_focused_pty();

        let (_, message) = app.status.most_recent_tui().expect("a status was set");
        assert_eq!(
            message, "a marker nothing else writes",
            "the second press must not write a second status: {message}"
        );
        assert_eq!(app.pending_pty_takeover.as_deref(), Some("session-1"));
    }

    // ── The card's keys and its button ──────────────────────────────────────

    /// An app showing a live agent a browser is driving, focused on the center
    /// pane: the exact state the card's key rule is about.
    fn app_with_the_card_up() -> (
        App,
        std::sync::Arc<std::sync::Mutex<Recorded>>,
        TuiOwnership,
    ) {
        let (mut app, recorded, seat) = app_with_a_live_pty_running("sleep 5");
        app.focus = FocusPane::Center;
        let browser = seat.owners.next_conn_id();
        seat.owners
            .claim_for_resize("session-1", browser, false, None, Some("Chrome"), |_| {})
            .epoch
            .expect("the browser claimed the pty");
        (app, recorded, seat)
    }

    /// The pull-request banner lives in its own lane OUTSIDE the pane, so the
    /// card covering the grid leaves it clickable, exactly as the web's does.
    /// The card owns its own area and nothing beyond it.
    #[test]
    fn the_pr_banner_stays_clickable_under_the_take_over_card() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (mut app, _recorded, _seat) = app_with_the_card_up();
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        app.url_opener = std::sync::Arc::new(move |url: &str| {
            let _ = tx
                .lock()
                .expect("the recording opener's channel")
                .send(url.to_string());
            Ok(())
        });
        app.engine.pr_statuses.insert(
            "session-1".to_string(),
            crate::model::PrInfo {
                number: 42,
                state: crate::model::PrState::Open,
                title: "Teach the banner to open its pull request".to_string(),
                host: "github.com".to_string(),
                owner_repo: "owner/repo".to_string(),
                url: "https://github.com/owner/repo/pull/42".to_string(),
            },
        );

        let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        assert!(
            app.focused_pty_is_covered_by_card(),
            "test setup: the card is between this surface and the child"
        );
        let band = app
            .mouse_layout
            .pr_banner
            .expect("the banner is painted beside the covered pane");
        let column = band.x + band.width / 2;
        let press = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: band.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        let release = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            ..press
        };
        app.handle_mouse(press);
        app.handle_mouse(release);

        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(10))
                .expect("the click under the card still opens the pull request"),
            "https://github.com/owner/repo/pull/42"
        );
        assert_eq!(
            app.pending_pty_takeover, None,
            "and it asks for the terminal no more than the web banner does"
        );
    }

    /// AMENDMENT 5. The FocusAgent binding presses the button, and so does Space:
    /// activating the focused control is the universal convention and the card
    /// has exactly one control.
    #[test]
    fn the_focus_key_and_space_press_the_cards_button() {
        for key in [
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        ] {
            let (mut app, _recorded, _seat) = app_with_the_card_up();
            app.handle_key(key).expect("the key is handled");
            assert_eq!(
                app.pending_pty_takeover.as_deref(),
                Some("session-1"),
                "{key:?} must arm the take-over the card's button arms"
            );
            assert!(
                !app.engine.is_typing("session-1"),
                "and it must never reach the child: {key:?}"
            );
        }
    }

    /// Every other typing-owned key is SWALLOWED. It goes nowhere today either
    /// (the gate drops it), but a key that falls through to the ladder while the
    /// card is up would act on a pane the user cannot see.
    #[test]
    fn an_ordinary_key_is_swallowed_while_the_card_is_up() {
        let (mut app, _recorded, _seat) = app_with_the_card_up();
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .expect("the key is handled");
        assert_eq!(app.pending_pty_takeover, None);
        assert!(
            !app.engine.is_typing("session-1"),
            "a covered pane writes nothing to the child"
        );
    }

    /// Tab ALWAYS moves panes while the card is up, even with `tab_reaches_agent`
    /// on: the option hands Tab to an agent this keyboard cannot reach.
    #[test]
    fn tab_moves_panes_while_the_card_is_up_even_when_it_is_the_agents() {
        for tab_reaches_agent in [false, true] {
            let (mut app, _recorded, _seat) = app_with_the_card_up();
            app.engine.config.ui.tab_reaches_agent = tab_reaches_agent;
            app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
                .expect("the key is handled");
            assert_ne!(
                app.focus,
                FocusPane::Center,
                "Tab must move focus out of the covered pane (tab_reaches_agent: \
                 {tab_reaches_agent})"
            );
        }
    }

    /// The dux chords keep working under the card: the tab switch and the
    /// palette are how a user gets anywhere else from here.
    #[test]
    fn the_dux_chords_keep_working_under_the_card() {
        let (mut app, _recorded, _seat) = app_with_the_card_up();
        let session_id = app.engine.sessions[0].id.clone();
        app.engine.agent_tabs.insert(
            "tab-2".to_string(),
            dux_core::model::AgentTab {
                id: "tab-2".to_string(),
                session_id,
                provider: dux_core::model::ProviderKind::new("claude"),
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL))
            .expect("the chord is handled");
        assert_eq!(
            app.focused_tab_id("session-1"),
            "tab-2",
            "the tab chord must still switch tabs while the card is up"
        );

        let (mut app, _recorded, _seat) = app_with_the_card_up();
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL))
            .expect("the chord is handled");
        assert!(
            matches!(app.prompt, PromptState::Command { .. }),
            "the palette chord must still open the palette while the card is up"
        );
    }

    /// From the LEFT pane the same key keeps its own meaning: it focuses the
    /// center pane. The card's rule is about the pane the card is on.
    #[test]
    fn the_left_panes_focus_key_focuses_without_taking_over() {
        let (mut app, _recorded, _seat) = app_with_the_card_up();
        app.focus = FocusPane::Left;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("the key is handled");
        assert_eq!(
            app.pending_pty_takeover, None,
            "focusing a pane is not asking for the terminal"
        );
        assert_eq!(app.focus, FocusPane::Center);
    }

    /// A DORMANT tab has no pty, so there is no ownership question, no card, and
    /// the key keeps its launch meaning.
    #[test]
    fn a_dormant_tab_has_no_card_and_keeps_its_launch_key() {
        let (mut app, _recorded, _seat) = app_with_the_card_up();
        app.engine.providers.remove("session-1");
        assert_eq!(
            app.focused_pty_takeover_card(),
            None,
            "with no live pty there is no ownership question and so no card"
        );
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("the key is handled");
        assert_eq!(
            app.pending_pty_takeover, None,
            "a dormant tab's key launches; it does not take anything over"
        );
    }

    /// AMENDMENT 6. In fullscreen the keyboard is a raw byte stream, so the
    /// binding's own BYTES are what press the button; a literal CR would be a
    /// hardcoded key by another name.
    #[test]
    fn the_focus_keys_bytes_press_the_button_on_the_raw_fullscreen_path() {
        let (mut app, recorded, seat) = app_with_the_card_up();
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        let bytes = app
            .bindings
            .byte_patterns_for(Action::FocusAgent)
            .first()
            .cloned()
            .expect("the FocusAgent binding has a byte form");

        app.process_raw_input_bytes(&bytes)
            .expect("the key is handled");

        assert_eq!(
            app.pending_pty_takeover.as_deref(),
            Some("session-1"),
            "the raw path must reach the same take-over the windowed key does"
        );
        assert!(
            !seat.owners.is_owner("session-1", seat.conn_id),
            "arming is not claiming: the resize that carries the claim is next"
        );
        assert!(
            recorded.lock().expect("not poisoned").published.is_empty(),
            "a key that writes nothing to the child announces nothing"
        );
    }

    /// SPACE PRESSES THE BUTTON ON THE RAW PATH TOO. It is the universal
    /// activation convention rather than a binding, so it has no byte pattern to
    /// look up and was missing here while the windowed path took it: the same
    /// key answered on one pane and typed nothing on the other.
    ///
    /// Both card states, because a key that works over one and not the other is
    /// the same divergence in miniature.
    #[test]
    fn space_presses_the_cards_button_on_the_raw_fullscreen_path() {
        for driven_elsewhere in [false, true] {
            let (mut app, _recorded, seat) = app_with_a_live_pty_running("sleep 5");
            app.focus = FocusPane::Center;
            app.input_target = InputTarget::Agent;
            app.fullscreen_overlay = FullscreenOverlay::Agent;
            if driven_elsewhere {
                let browser = seat.owners.next_conn_id();
                seat.owners.claim("session-1", browser).expect("claimed");
            }

            app.process_raw_input_bytes(b" ")
                .expect("the key is handled");

            assert_eq!(
                app.pending_pty_takeover.as_deref(),
                Some("session-1"),
                "Space must press the card's button here as it does in the \
                 windowed pane (driven elsewhere: {driven_elsewhere})"
            );
            assert!(
                !app.engine.is_typing("session-1"),
                "and it must never reach the child (driven elsewhere: \
                 {driven_elsewhere})"
            );
        }
    }

    /// The control: with no card up, a space is an ordinary keystroke and rides
    /// to the child untouched. Without this the rule above could be "Space never
    /// reaches a terminal", which would break the space bar.
    #[test]
    fn space_reaches_the_child_when_this_surface_drives_the_pty() {
        let (mut app, _recorded, _seat) = app_with_a_live_pty_running("sleep 5");
        app.focus = FocusPane::Center;
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        app.claim_launched_pty("session-1");

        app.process_raw_input_bytes(b" ")
            .expect("the key is handled");

        assert_eq!(app.pending_pty_takeover, None);
        assert!(
            app.engine.is_typing("session-1"),
            "a space typed into a terminal this surface drives is a space"
        );
    }

    /// And a PASTED space is content, never the button. Pasted bytes skip
    /// intercept matching entirely, which matters far more for a space than for
    /// any chord: almost every paste contains one.
    #[test]
    fn a_space_inside_a_paste_presses_nothing() {
        let (mut app, _recorded, _seat) = app_with_the_card_up();
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;

        let mut bytes = Vec::new();
        bytes.extend_from_slice(crate::raw_input::BRACKET_PASTE_START);
        bytes.extend_from_slice(b"a b");
        bytes.extend_from_slice(crate::raw_input::BRACKET_PASTE_END);
        app.process_raw_input_bytes(&bytes)
            .expect("the paste is handled");

        assert_eq!(
            app.pending_pty_takeover, None,
            "a space in pasted text is content, never the card's key"
        );
    }

    /// AMENDMENT 8. The button is a click target like any other: pressing it and
    /// releasing inside it takes the terminal over, and a click anywhere else on
    /// the covered pane does not.
    #[test]
    fn clicking_the_cards_button_takes_over_and_clicking_elsewhere_does_not() {
        use ratatui::layout::Rect;

        let (mut app, _recorded, _seat) = app_with_the_card_up();
        app.mouse_layout.agent_term = Some(Rect::new(0, 0, 80, 20));
        app.mouse_layout.takeover_button = Some(Rect::new(30, 10, 16, 3));

        // Outside the button: no take-over, and the press is not armed.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.pending_pty_takeover, None);

        // On the button: pressed, then released inside it.
        app.mouse_layout.takeover_button = Some(Rect::new(30, 10, 16, 3));
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 34,
            row: 11,
            modifiers: KeyModifiers::NONE,
        });
        assert!(
            app.takeover_press.is_some(),
            "the press must show as pressed while it is held"
        );
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 34,
            row: 11,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.pending_pty_takeover.as_deref(), Some("session-1"));
        assert_eq!(
            app.takeover_press, None,
            "the press is spent by the release"
        );
    }

    /// The card covers the grid, so there is no readable link under it to
    /// click: a press on a cell that carries one is swallowed with everything
    /// else the card covers, and dux opens nothing.
    #[test]
    fn a_press_on_a_linked_cell_under_the_card_opens_nothing() {
        use ratatui::layout::Rect;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (mut app, _recorded, _seat) = app_with_the_card_up();
        app.mouse_layout.agent_term = Some(Rect::new(0, 0, 80, 20));
        app.mouse_layout.takeover_button = Some(Rect::new(30, 10, 16, 3));
        app.snapshot_buf.rows = 20;
        app.snapshot_buf.cols = 80;
        app.snapshot_buf.links = vec!["https://example.com/pr/1".to_string()];
        app.snapshot_buf.cells = vec![crate::pty::SnapshotCell {
            row: 5,
            col: 5,
            symbol: "P".into(),
            fg: crate::pty::CellColor::Reset,
            bg: crate::pty::CellColor::Reset,
            modifier: crate::pty::CellModifier::default(),
            link: Some(0),
        }];
        let opens = std::sync::Arc::new(AtomicUsize::new(0));
        let counted = std::sync::Arc::clone(&opens);
        app.url_opener = std::sync::Arc::new(move |_url: &str| {
            counted.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(
            opens.load(Ordering::Relaxed),
            0,
            "the card owns every click over the area it covers"
        );
        assert!(app.pending_link_click.is_none());
    }

    /// Dragging off the button before releasing cancels the click, which is the
    /// universal convention every other button in dux already follows.
    #[test]
    fn dragging_off_the_cards_button_cancels_the_click() {
        use ratatui::layout::Rect;

        let (mut app, _recorded, _seat) = app_with_the_card_up();
        app.mouse_layout.agent_term = Some(Rect::new(0, 0, 80, 20));
        app.mouse_layout.takeover_button = Some(Rect::new(30, 10, 16, 3));

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 34,
            row: 11,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 4,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 4,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.pending_pty_takeover, None);
        assert_eq!(app.takeover_press, None);
    }

    /// AMENDMENT 4. There is nothing readable under the card, so a press inside
    /// the pane starts no selection: a highlight the user cannot see, over text
    /// they cannot see, that copies the child's cells on release.
    ///
    /// Driven through the interactive raw path, because that is the one path
    /// that starts a terminal selection at all.
    #[test]
    fn a_press_under_the_card_starts_no_selection() {
        use ratatui::layout::Rect;

        // The control: with nobody else driving, the same press DOES select, so
        // the assertion below is about the card and not about a path that never
        // selects anyway.
        for card_up in [false, true] {
            let (mut app, _recorded, seat) = app_with_a_live_pty_running("sleep 5");
            app.focus = FocusPane::Center;
            app.input_target = InputTarget::Agent;
            app.fullscreen_overlay = FullscreenOverlay::Agent;
            app.mouse_layout.agent_term = Some(Rect::new(0, 0, 80, 20));
            app.mouse_layout.takeover_button = Some(Rect::new(30, 10, 16, 3));
            if card_up {
                let browser = seat.owners.next_conn_id();
                seat.owners.claim("session-1", browser).expect("claimed");
            } else {
                app.claim_launched_pty("session-1");
            }

            // An SGR left press at column 6, row 6 (the wire is 1-based).
            app.process_raw_input_bytes(b"\x1b[<0;6;6M")
                .expect("the press is handled");

            assert_eq!(
                app.terminal_selection.is_some(),
                !card_up,
                "a press may start a selection exactly when there is text under \
                 it to select (card up: {card_up})"
            );
        }
    }

    /// Every end of the participation lets go, and the ONE place that ends it is
    /// the quiet stop: the palette command, a config reload that turns the setting
    /// off, the flip, and quitting all route through it.
    #[test]
    fn stopping_the_background_server_releases_this_surfaces_ptys() {
        let (mut app, recorded, seat) = serving_app();
        app.claim_launched_pty("s1");
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

    /// The take-over has exactly one way in, the card's button, and the card is
    /// on screen exactly while the shown pty is not this surface's. So the
    /// states the card cannot be up in arm nothing and, crucially, say nothing:
    /// a status line explaining a refusal the user never asked for is noise
    /// about a button they could not have pressed.
    #[test]
    fn a_take_over_with_no_card_behind_it_arms_nothing_and_stays_quiet() {
        // Nothing serving: no registry, no seat, no card.
        let mut app = test_app(default_bindings());
        app.take_over_focused_pty();
        assert_eq!(app.pending_pty_takeover, None);
        assert!(
            app.status.most_recent_tui().is_none(),
            "there was no card, so there is nothing to report"
        );

        // Serving with a live pty, but this surface is already the driver.
        let (mut app, _recorded, seat) = app_with_a_live_pty();
        app.claim_launched_pty("session-1");
        assert!(seat.owners.is_owner("session-1", seat.conn_id));
        app.take_over_focused_pty();
        assert_eq!(app.pending_pty_takeover, None);
        assert!(app.status.most_recent_tui().is_none());

        // Serving with NO live pty under the cursor: a dormant tab has no
        // ownership question and so no card either.
        let (mut app, _recorded, _seat) = app_with_a_live_pty();
        app.engine.providers.remove("session-1");
        app.take_over_focused_pty();
        assert_eq!(app.pending_pty_takeover, None);
        assert!(app.status.most_recent_tui().is_none());
    }

    /// THE TRAP, end to end. A browser drives the agent, re-grids the child to a
    /// phone's shape, and goes away. This surface presses the card's button,
    /// which claims the pty. The child must end up at THIS window's geometry.
    ///
    /// Every step is the real one: a real claim, a real refusal, a real render
    /// pass measuring a real pane. What made this permanent was two bugs meeting.
    /// The refused resize recorded the dedupe, so the pane believed it had already
    /// sent this geometry to this target; and the claim cleared nothing, so
    /// nothing ever asked again. The user was left driving a phone-sized child
    /// with nothing on screen to say so, because the card goes the moment the claim
    /// succeeds.
    #[test]
    fn claiming_after_a_foreign_re_grid_heals_the_childs_geometry() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (mut app, _recorded, seat) = app_with_a_live_pty();
        app.input_target = InputTarget::Agent;
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("terminal");

        // The browser claims the pty and re-grids the child to its own shape.
        let browser = seat.owners.next_conn_id();
        let browser_seq = seat
            .owners
            .claim_for_resize(
                "session-1",
                browser,
                false,
                None,
                Some(REAL_CHROME_UA),
                |_| {},
            )
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

        // The user presses the card's button. The claim rides the next render,
        // which must send this pane's geometry even though the pane has not
        // changed size at all.
        app.take_over_focused_pty();
        terminal
            .draw(|frame| app.render(frame))
            .expect("render succeeds");
        assert!(
            seat.owners.is_owner("session-1", seat.conn_id),
            "a flagged claim on an unowned pty is granted"
        );

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

    /// The take-over ACTION, driven through the one state it is reachable in.
    /// Another device drives the pty, so the intent is armed, the message names
    /// the device that is losing the terminal, and the resize dedupe is cleared
    /// so the claim's resize really is sent.
    #[test]
    fn the_take_over_action_arms_the_claim_and_names_the_device() {
        let (mut app, _recorded, seat) = app_with_a_live_pty();
        let browser = seat.owners.next_conn_id();
        seat.owners
            .claim_for_resize(
                "session-1",
                browser,
                false,
                None,
                Some(REAL_CHROME_UA),
                |_| {},
            )
            .epoch
            .expect("the browser claimed the pty");
        app.last_pty_resize_target = Some("session-1".to_string());

        app.take_over_focused_pty();

        assert_eq!(app.pending_pty_takeover.as_deref(), Some("session-1"));
        assert_eq!(app.last_pty_resize_target, None);
        let (_, message) = app.status.most_recent_tui().expect("a status was set");
        assert!(
            message.contains("Chrome on macOS"),
            "the message must name whoever is losing the terminal: {message}"
        );

        // A driver that presented no name is still described, with the prose
        // sentinel rather than a blank where a device should be.
        let (mut app, _recorded, seat) = app_with_a_live_pty();
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("session-1", browser).expect("claimed");
        app.take_over_focused_pty();
        let (_, message) = app.status.most_recent_tui().expect("a status was set");
        assert!(
            message.contains(UNNAMED_DEVICE),
            "a nameless driver is still named, honestly: {message}"
        );
    }

    /// An armed take-over is SPENT OR DROPPED by the first render after arming.
    /// Left alive it fires later on a pane the user has moved away from, taking a
    /// terminal away from whichever device is driving it by then.
    #[test]
    fn an_armed_take_over_is_dropped_when_the_user_moves_to_another_terminal() {
        let (mut app, _recorded, seat) = app_with_a_live_pty();
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("session-1", browser).expect("claimed");
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
            message.contains("Take over"),
            "the drop must point at the button that asks again: {message}"
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

    /// And a REAL keystroke claims nothing either, which is the whole change:
    /// the card is over this pane asking for its button, so the key goes nowhere
    /// and nobody's watchers lose a keyboard to it.
    ///
    /// The control that proves the raw path is reached at all is
    /// [`the_focus_keys_bytes_press_the_button_on_a_free_pty`], where the same
    /// path arms the take-over.
    #[test]
    fn a_real_keystroke_claims_nothing_on_a_free_pty() {
        let (mut app, recorded, seat) = app_with_a_live_pty();
        app.input_target = InputTarget::Agent;

        app.process_raw_input_bytes(b"x")
            .expect("the keystroke is handled");

        assert!(
            !seat.owners.is_owner("session-1", seat.conn_id),
            "only the card's button claims a pty nobody drives"
        );
        assert!(
            !app.engine.is_typing("session-1"),
            "and the key reaches nothing"
        );
        assert!(recorded.lock().expect("not poisoned").published.is_empty());
    }

    /// A page key that IS forwarded consults the gate at the forward site, so a
    /// demoted pane does not write it.
    ///
    /// The interesting case is being scrolled back at the same time: the batch's
    /// scroll-mode short circuit says "nothing in this batch reaches the child",
    /// which is true of the batched forwards and NOT true of a page key the
    /// provider's `forward_scroll` says to send. Reusing that verdict here sent a
    /// demoted surface's page keys straight into somebody else's terminal.
    /// The card leaves the palette chord dux's, so that key must
    /// work where the card shows: in fullscreen the palette chord normally rides
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
            } else {
                app.claim_launched_pty("session-1");
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
            } else {
                app.claim_launched_pty("session-1");
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

    /// THE REPORTED BUG, as a test. A browser starts an agent; this surface
    /// merely draws the new pane, and that draw must not make this terminal the
    /// agent's driver. The browser's own attach resize arrives afterwards, and
    /// attaching never steals, so a passive claim here leaves the browser that
    /// started the agent watching a take-over card over its own new terminal.
    #[test]
    fn merely_rendering_a_free_pty_leaves_it_free_for_the_browser_that_started_it() {
        let (mut app, _recorded, seat) = app_with_a_live_pty();

        let _ = render_rows(&mut app, 160, 40);

        assert_eq!(
            app.pty_driver("session-1"),
            PtyDriver::Free,
            "looking at a terminal is not driving it"
        );
        let browser = seat.owners.next_conn_id();
        let claim = seat.owners.claim_for_resize(
            "session-1",
            browser,
            false,
            None,
            Some(REAL_CHROME_UA),
            |_| {},
        );
        assert!(
            claim.apply,
            "the browser that started the agent must be able to attach to it"
        );
        assert!(
            seat.owners.is_owner("session-1", browser),
            "and its plain attach claims the pty nobody was driving"
        );
    }

    /// The other half of the reported bug: once the browser has attached, this
    /// surface keeps drawing the agent every frame, and none of those frames may
    /// hand it the pty back. Losing (or never having) a pty is sticky.
    #[test]
    fn a_browser_launched_agent_stays_browser_driven_however_often_it_is_drawn() {
        let (mut app, _recorded, seat) = app_with_a_live_pty();
        let browser = seat.owners.next_conn_id();
        seat.owners
            .claim_for_resize(
                "session-1",
                browser,
                false,
                None,
                Some(REAL_CHROME_UA),
                |_| {},
            )
            .epoch
            .expect("the browser attached first");

        for _ in 0..3 {
            let _ = render_rows(&mut app, 160, 40);
        }

        assert!(
            seat.owners.is_owner("session-1", browser),
            "repeated frames must not accumulate into a claim"
        );
    }

    /// A launch this surface started IS a claim, and the frame after it sizes the
    /// child. Without this half an agent started here would come up under the
    /// `Take control` card, and no window resize would reach its child until
    /// somebody had pressed the button.
    #[test]
    fn a_launch_started_here_claims_its_pty_and_the_next_frame_sizes_the_child() {
        let (mut app, _recorded, seat) = app_with_a_live_pty();

        app.claim_launched_pty("session-1");
        assert_eq!(app.pty_driver("session-1"), PtyDriver::Mine);

        let _ = render_rows(&mut app, 160, 40);
        let grid = app
            .engine
            .providers
            .get("session-1")
            .and_then(|client| client.grid_size());
        assert_ne!(
            grid,
            Some((10, 10)),
            "the render pass sized the child this surface drives"
        );
        assert!(seat.owners.is_owner("session-1", seat.conn_id));
    }

    /// A launch never steals. Starting a tab here while a browser drives that
    /// pty leaves the browser driving it.
    #[test]
    fn a_launch_started_here_never_takes_a_pty_another_device_drives() {
        let (mut app, _recorded, seat) = app_with_a_live_pty();
        let browser = seat.owners.next_conn_id();
        seat.owners.claim("session-1", browser).expect("claimed");

        app.claim_launched_pty("session-1");

        assert!(seat.owners.is_owner("session-1", browser));
    }

    /// With nothing serving there is no seat, no registry and no gate, so a
    /// render sizes the child exactly as it did before any of this existed.
    #[test]
    fn with_nothing_serving_a_render_still_sizes_the_child() {
        let (mut app, _recorded, _seat) = app_with_a_live_pty();
        app.companion = None;

        let _ = render_rows(&mut app, 160, 40);

        let grid = app
            .engine
            .providers
            .get("session-1")
            .and_then(|client| client.grid_size());
        assert_ne!(
            grid,
            Some((10, 10)),
            "no seat means no gate: the resize goes straight to the child"
        );
    }

    // ── The third card: a pty nobody is driving ─────────────────────────────

    /// While a background server is serving, a pty NOBODY drives is covered by
    /// the web's third card, word for word: the title that names the act, the
    /// description that names the absence, and the same one button.
    ///
    /// A real render rather than a call to the card builder, for the same reason
    /// the other card tests use one: the bug this guards against is a pane that
    /// keeps showing a child every keystroke is being dropped into.
    #[test]
    fn a_free_pty_shows_the_take_control_card_while_serving() {
        let (mut app, _recorded, _seat) =
            app_with_a_live_pty_running(&format!("printf {CHILD_MARKER}; sleep 5"));
        wait_for_child_output(&app);

        let flat = flowed(&render_rows(&mut app, 160, 40));

        assert!(
            flat.contains("Take control"),
            "a pty nobody drives gets the title that names the act: {flat}"
        );
        assert!(
            flat.contains(
                "No device is driving right now. Take over to drive this agent from here."
            ),
            "the description is the web's, word for word: {flat}"
        );
        assert!(
            flat.contains("Take over"),
            "the button is the same one every other card carries: {flat}"
        );
        assert!(
            !flat.contains(CHILD_MARKER),
            "the card covers the grid here exactly as it does over a driven pty: {flat}"
        );
    }

    /// A companion terminal's card says "terminal" where an agent's says
    /// "agent", the same word swap the web makes. Drawn through the terminal's
    /// own surface, which is an overlay: the windowed center pane is always the
    /// agent's.
    #[test]
    fn a_free_companion_terminals_card_names_a_terminal() {
        let (mut app, _recorded, _seat) = app_with_a_live_pty_running("sleep 5");
        app.session_surface = SessionSurface::Terminal;
        app.fullscreen_overlay = FullscreenOverlay::Terminal;
        app.active_terminal_id = Some("term-1".to_string());
        app.engine.companion_terminals.insert(
            "term-1".to_string(),
            dux_core::model::CompanionTerminal {
                owner: dux_core::model::TerminalOwner::Session("session-1".to_string()),
                label: "shell".to_string(),
                foreground_cmd: None,
                client: crate::pty::PtyClient::spawn(
                    "sh",
                    &["-c".to_string(), "sleep 5".to_string()],
                    std::path::Path::new("."),
                    10,
                    10,
                    100,
                )
                .expect("spawn pty"),
                sort_order: 0,
                created_at: chrono::Utc::now(),
            },
        );

        let flat = flowed(&render_rows(&mut app, 160, 40));
        assert!(
            flat.contains(
                "No device is driving right now. Take over to drive this terminal from here."
            ),
            "a terminal's card names a terminal: {flat}"
        );
    }

    /// The button is the one act that claims a free pty, and the frame after it
    /// sizes the child: the arm clears the resize dedupe exactly as a take-over
    /// from another device does.
    #[test]
    fn pressing_take_control_claims_a_free_pty_and_the_next_frame_sizes_the_child() {
        let (mut app, _recorded, seat) = app_with_a_live_pty();
        let _ = render_rows(&mut app, 160, 40);
        assert_eq!(
            app.pty_driver("session-1"),
            PtyDriver::Free,
            "the frame before the press claimed nothing"
        );

        app.take_over_focused_pty();
        assert_eq!(
            app.pending_pty_takeover.as_deref(),
            Some("session-1"),
            "the press arms the claim; the render pass carries it"
        );

        let _ = render_rows(&mut app, 160, 40);
        assert!(
            seat.owners.is_owner("session-1", seat.conn_id),
            "a flagged claim on an unowned pty is granted"
        );
        let grid = app
            .engine
            .providers
            .get("session-1")
            .and_then(|client| client.grid_size());
        assert_ne!(
            grid,
            Some((10, 10)),
            "the claim cleared the resize dedupe, so the next frame sent this \
             pane's geometry"
        );

        let flat = flowed(&render_rows(&mut app, 160, 40));
        assert!(
            !flat.contains("Take control"),
            "the card is gone once the pane is this surface's: {flat}"
        );
    }

    /// Typing no longer claims. While the card is up the gate refuses the write,
    /// which is the same rung every other card variant puts between this
    /// keyboard and the child.
    #[test]
    fn typing_under_the_take_control_card_reaches_nothing() {
        let (mut app, recorded, seat) = app_with_a_live_pty();
        app.focus = FocusPane::Center;

        assert!(
            !app.may_type_into_pty("session-1"),
            "the only act that claims a free pty is the card's button"
        );
        assert!(!seat.owners.is_owner("session-1", seat.conn_id));

        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .expect("the key is handled");
        assert!(
            !app.engine.is_typing("session-1"),
            "a covered pane writes nothing to the child"
        );
        assert!(
            recorded.lock().expect("not poisoned").published.is_empty(),
            "a refusal changes nothing, so it announces nothing"
        );
    }

    /// With nothing serving none of this exists: no seat, no card, and typing
    /// goes straight into the child exactly as it did before this surface joined
    /// the ownership model.
    #[test]
    fn with_nothing_serving_a_free_pty_is_typed_into_directly() {
        let (mut app, _recorded, _seat) = app_with_a_live_pty();
        app.companion = None;
        app.focus = FocusPane::Center;

        let flat = flowed(&render_rows(&mut app, 160, 40));
        assert!(!flat.contains("Take control"), "no seat, no card: {flat}");
        assert!(app.may_type_into_pty("session-1"));
    }

    /// The raw fullscreen path reaches the same button on a free pty, through
    /// the binding's own bytes.
    #[test]
    fn the_focus_keys_bytes_press_the_button_on_a_free_pty() {
        let (mut app, _recorded, _seat) = app_with_a_live_pty_running("sleep 5");
        app.focus = FocusPane::Center;
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        let bytes = app
            .bindings
            .byte_patterns_for(Action::FocusAgent)
            .first()
            .cloned()
            .expect("the FocusAgent binding has a byte form");

        app.process_raw_input_bytes(&bytes)
            .expect("the key is handled");

        assert_eq!(
            app.pending_pty_takeover.as_deref(),
            Some("session-1"),
            "the raw path must reach the same take-over the windowed key does"
        );
        assert!(
            !app.engine.is_typing("session-1"),
            "and the bytes must never reach the child"
        );
    }

    /// And the mouse: the button on a free pty's card is a click target like any
    /// other.
    #[test]
    fn clicking_take_control_claims_a_free_pty() {
        use ratatui::layout::Rect;

        let (mut app, _recorded, _seat) = app_with_a_live_pty_running("sleep 5");
        app.mouse_layout.agent_term = Some(Rect::new(0, 0, 80, 20));
        app.mouse_layout.takeover_button = Some(Rect::new(30, 10, 16, 3));

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 35,
            row: 11,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 35,
            row: 11,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(
            app.pending_pty_takeover.as_deref(),
            Some("session-1"),
            "a press and release inside the button takes the pty over"
        );
    }

    /// The card's shape: a blank row under the title border, the prose, a blank
    /// row, then the button. The top padding is the counterpart of the blank row
    /// above the button, so the body sits inside a ring of space instead of
    /// starting hard against the line that names the driving device.
    #[test]
    fn the_card_pads_one_blank_row_under_its_title_border() {
        let (app, rows) = render_with_a_browser_driving(Some(REAL_CHROME_UA));
        let button = app.mouse_layout.takeover_button.expect("the button is up");

        // The title border names the driving device, so it is the one row above
        // the button that cannot be confused with anything else on screen.
        let title_row = u16::try_from(
            rows.iter()
                .position(|row| row.contains("Open on Chrome on macOS"))
                .expect("the card's titled top border is on screen"),
        )
        .expect("a screen row index fits");
        assert!(title_row < button.y, "the title is above the button");

        let card_columns = |y: u16| {
            rows[y as usize]
                .chars()
                .skip(button.x as usize)
                .take(button.width as usize)
                .collect::<String>()
        };
        assert!(
            card_columns(title_row + 1).trim().is_empty(),
            "the row under the title border is blank: {:?}",
            card_columns(title_row + 1)
        );
        assert!(
            !card_columns(title_row + 2).trim().is_empty(),
            "and the prose starts on the row after it: {:?}",
            card_columns(title_row + 2)
        );
        assert!(
            card_columns(button.y - 1).trim().is_empty(),
            "the blank row above the button is still there: {:?}",
            card_columns(button.y - 1)
        );
    }

    /// The button's label is CENTRED in it, within the one column an odd slack
    /// leaves over, and that column falls on the right.
    ///
    /// Ratatui's own `Alignment::Center` halves each width separately
    /// (`area / 2 - label / 2`), which rounds an odd label's offset up and put
    /// this nine-character label one cell right of centre inside its fourteen
    /// column button. The shared button widget splits the slack itself now, so
    /// this measures the rendered cells rather than trusting the widget.
    #[test]
    fn the_cards_button_label_is_centred_in_it() {
        let (app, rows) = render_with_a_browser_driving(Some(REAL_CHROME_UA));
        let button = app.mouse_layout.takeover_button.expect("the button is up");

        let (left, right, inner_w) = label_padding(&rows, button);
        assert_eq!(
            left + right + "Take over".chars().count(),
            inner_w,
            "the label and its padding fill the button's inside"
        );
        assert!(
            right == left || right == left + 1,
            "padding must be even, or one column longer on the right: \
             left {left}, right {right}, inside {inner_w}"
        );
    }

    /// The padding on either side of a button's label, measured off the drawn
    /// frame: `(left, right, inner width)`.
    fn label_padding(rows: &[String], button: ratatui::layout::Rect) -> (usize, usize, usize) {
        let inner: Vec<char> = rows[(button.y + 1) as usize]
            .chars()
            .skip(button.x as usize + 1)
            .take(button.width as usize - 2)
            .collect();
        let left = inner.iter().take_while(|c| **c == ' ').count();
        let right = inner.iter().rev().take_while(|c| **c == ' ').count();
        (left, right, inner.len())
    }

    /// A ready view for `session-1`, built for a test that only cares which
    /// launch produced it.
    fn ready_view(app: &App, view: AgentLaunchReadyView) -> AgentLaunchReadyOutcome {
        AgentLaunchReadyOutcome {
            session: app.engine.sessions[0].clone(),
            tab_id: "session-1".to_string(),
            pty_size: (24, 80),
            detached_session_id: None,
            wants_fullscreen: false,
            view,
        }
    }

    /// A create THIS surface asked for and the engine REFUSED must not arm a
    /// claim, or the arm sits there waiting and lands on the browser's create
    /// instead: the refusal comes back as an ordinary status rather than an
    /// error, so a claim armed before the dispatch is never taken back.
    #[test]
    fn a_create_refused_here_never_claims_the_agent_the_browser_was_creating() {
        let (mut app, _recorded, _seat) = app_with_a_live_pty();
        // The browser's create is already in flight, which is precisely what
        // makes the engine refuse this surface's.
        assert!(
            app.engine
                .mark_in_flight(dux_core::engine::InFlightKey::CreateAgent),
            "test setup: no create was in flight yet"
        );
        let provider = app.engine.sessions[0].provider.clone();

        app.dispatch_create_agent_request(
            dux_core::worker::CreateAgentRequest::Standalone {
                folder: std::path::PathBuf::from("."),
                title: "refused".to_string(),
                provider,
            },
            "Creating an agent...".to_string(),
        )
        .expect("a refused create comes back as a status, not an error");
        assert!(
            !app.create_agent_started_here,
            "a create the engine refused must arm nothing"
        );

        // The browser's create lands. Its child has to stay free for the
        // browser that asked for it to attach to.
        let outcome = ready_view(
            &app,
            AgentLaunchReadyView::CreateCommitted {
                status_message: "Created.".to_string(),
                startup_result_error: None,
            },
        );
        app.apply_agent_launch_ready_view(outcome);

        assert_eq!(
            app.pty_driver("session-1"),
            PtyDriver::Free,
            "the browser's own create must not be claimed by this surface"
        );
    }

    /// WHICH LAUNCHES ARE DELIBERATE. Everything a person at this keyboard asks
    /// for claims the child it starts; the startup sweep does not, because
    /// nobody asked for it.
    #[test]
    fn only_a_launch_somebody_asked_for_claims_the_child_it_starts() {
        use dux_core::worker::AgentLaunchKind;

        assert!(
            !launch_claims_its_pty(&AgentLaunchKind::StartupAutoReopen),
            "reopening at startup is dux catching up, not a person acting"
        );
        for kind in [
            AgentLaunchKind::Reconnect {
                status_message: String::new(),
            },
            AgentLaunchKind::ForceReconnect {
                status_message: String::new(),
            },
            AgentLaunchKind::ResumeFallback {
                status_message: String::new(),
            },
            AgentLaunchKind::Tab {
                is_fresh: true,
                status_message: String::new(),
            },
            AgentLaunchKind::Create {
                status_message: String::new(),
                repo_path: String::new(),
                owns_worktree: false,
                startup_result: None,
                status_op_id: String::new(),
            },
        ] {
            assert!(
                launch_claims_its_pty(&kind),
                "a launch somebody asked for claims its child"
            );
        }
    }

    /// The outcome half of the same rule, and the cost of it stated as a test:
    /// an agent reopened by the startup sweep is claimed by nobody, so this
    /// surface shows it the `Take control` card until somebody presses the
    /// button. That is exactly what a browser shows for a terminal it did not
    /// start, and pressing is what makes it this window's.
    #[test]
    fn a_startup_auto_reopened_agent_shows_take_control_until_pressed() {
        let (mut app, _recorded, seat) = app_with_a_live_pty();

        let outcome = ready_view(&app, AgentLaunchReadyView::StartupAutoReopen);
        app.apply_agent_launch_ready_view(outcome);
        let flat = flowed(&render_rows(&mut app, 160, 40));

        assert_eq!(
            app.pty_driver("session-1"),
            PtyDriver::Free,
            "nobody acted, so nobody drives it yet"
        );
        assert!(
            flat.contains("Take control"),
            "and the pane says so rather than inviting keys it would drop: {flat}"
        );

        app.take_over_focused_pty();
        let flat = flowed(&render_rows(&mut app, 160, 40));
        assert!(
            seat.owners.is_owner("session-1", seat.conn_id),
            "the press is what claims it"
        );
        assert!(!flat.contains("Take control"), "and the card goes: {flat}");
    }
}

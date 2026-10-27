//! Blocking work a key asks for (git, the filesystem), run on a worker
//! thread with its continuation run back on the UI thread.
//!
//! The terminal UI's loop is also the engine servicer while dux serves the
//! web, so a stall here would freeze every browser too. A job shows a keyed
//! busy for as long as it runs; the busy is retired the moment the answer
//! lands, and whatever the continuation says is the final.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use dux_core::engine::{EventReaction, Final, HandlerStatusOp};
use dux_core::status_text::StatusText;

use super::App;

/// What a job's continuation does once it is back on the UI thread.
type Continuation = Box<dyn FnOnce(&mut App) + Send>;

/// How a job ended, for its busy's final.
pub(crate) enum UiJobEnd {
    /// The work answered; the continuation says what came of it.
    Answered,
    /// The work stopped before answering, with the sentence that says so.
    Stopped(String),
    /// The terminal UI handed the engine to the web server before the work
    /// answered, so nothing will act on it.
    HandedOver,
}

/// The jobs running now and the channel their answers come back on.
pub(crate) struct UiJobs {
    tx: mpsc::Sender<(u64, Continuation)>,
    rx: mpsc::Receiver<(u64, Continuation)>,
    next_id: u64,
    running: HashMap<u64, HandlerStatusOp<UiJobEnd>>,
    /// Quiet jobs out now (see [`App::spawn_quiet_ui_job`]).
    quiet_running: usize,
    /// The newest request of each kind, so an answer to one the user has
    /// since replaced is dropped.
    newest: HashMap<RequestKind, u64>,
}

/// A kind of request of which only the newest may act when it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RequestKind {
    /// Opening an agent's directory in an editor, or the editor picker.
    Editor,
}

impl Default for UiJobs {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            next_id: 0,
            running: HashMap::new(),
            quiet_running: 0,
            newest: HashMap::new(),
        }
    }
}

impl UiJobs {
    /// Count a new request of `kind`, which replaces every earlier one.
    pub(crate) fn begin_request(&mut self, kind: RequestKind) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.newest.insert(kind, id);
        id
    }

    /// Whether `id` is still the newest request of `kind`.
    pub(crate) fn is_newest(&self, kind: RequestKind, id: u64) -> bool {
        self.newest.get(&kind) == Some(&id)
    }

    /// Whether any job is still out.
    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.running.is_empty() && self.quiet_running == 0
    }
}

#[cfg(test)]
thread_local! {
    static FAIL_NEXT_QUIET_SPAWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Make the next quiet lookup started on this thread fail to start.
#[cfg(test)]
pub(crate) fn fail_next_quiet_spawn() {
    FAIL_NEXT_QUIET_SPAWN.with(|fail| fail.set(true));
}

/// A job's two sentences: the busy shown while it runs, and what is said if
/// the worker stops before answering.
pub(crate) struct UiJob {
    pub(crate) busy: StatusText,
    pub(crate) stopped: String,
}

/// A job's continuation, held where both the worker and a failed spawn can
/// reach it, so it runs exactly once whichever way the job ends.
type Then<C> = Arc<Mutex<Option<C>>>;

fn take_then<C>(then: &Then<C>) -> Option<C> {
    then.lock().map_or(None, |mut then| then.take())
}

impl App {
    /// Run `work` on a worker under `job`'s busy, then hand its answer to
    /// `then` on the UI thread: `Some` with what the work returned, or `None`
    /// when the worker stopped before answering (the busy's final says so),
    /// so state kept for the job can be let go either way. `then` must check
    /// that what it acts on is still there: the user may have moved on.
    pub(crate) fn spawn_ui_job<T, W, C>(&mut self, job: UiJob, work: W, then: C)
    where
        T: Send + 'static,
        W: FnOnce() -> T + Send + 'static,
        C: FnOnce(&mut App, Option<T>) + Send + 'static,
    {
        let UiJob { busy, stopped } = job;
        let op = dux_core::engine::status_op(busy).resolve_in_handler(|end: &UiJobEnd| match end {
            UiJobEnd::Answered => Final::clear(),
            UiJobEnd::Stopped(sentence) => Final::error(sentence.clone()),
            UiJobEnd::HandedOver => Final::warning(
                "The terminal UI handed over to the web server before this finished, so nothing \
                 came of it. Do it again from the browser or the terminal UI.",
            ),
        });
        let pending = self.engine.begin_status_op(&op);
        let id = self.ui_jobs.next_id;
        self.ui_jobs.next_id += 1;
        self.ui_jobs.running.insert(id, op);
        self.apply_reaction(EventReaction::Status(pending));

        let then: Then<C> = Arc::new(Mutex::new(Some(then)));
        let worker_then = Arc::clone(&then);
        let tx = self.ui_jobs.tx.clone();
        let spawned = thread::Builder::new()
            .name("dux-ui-job".into())
            .spawn(move || {
                let answer = std::panic::catch_unwind(AssertUnwindSafe(work));
                let continuation: Continuation = Box::new(move |app: &mut App| {
                    let answer = match answer {
                        Ok(answer) => {
                            app.finish_ui_job(id, &UiJobEnd::Answered);
                            Some(answer)
                        }
                        Err(payload) => {
                            let reason = dux_core::engine::format_panic_payload(&*payload);
                            dux_core::logger::error(&format!("ui job panicked: {reason}"));
                            app.finish_ui_job(id, &UiJobEnd::Stopped(stopped));
                            None
                        }
                    };
                    if let Some(then) = take_then(&worker_then) {
                        then(app, answer);
                    }
                });
                let _ = tx.send((id, continuation));
            });
        if let Err(err) = spawned {
            dux_core::logger::error(&format!("could not start a ui job worker: {err}"));
            self.finish_ui_job(
                id,
                &UiJobEnd::Stopped(format!("Could not start a background worker: {err}")),
            );
            if let Some(then) = take_then(&then) {
                then(self, None);
            }
        }
    }

    /// [`Self::spawn_ui_job`] for a lookup nobody asked for as an operation,
    /// one that only fills in something the screen shows as the user types
    /// (path completions): no busy, and a worker that stops is logged and
    /// answers `None`.
    pub(crate) fn spawn_quiet_ui_job<T, W, C>(&mut self, work: W, then: C)
    where
        T: Send + 'static,
        W: FnOnce() -> T + Send + 'static,
        C: FnOnce(&mut App, Option<T>) + Send + 'static,
    {
        self.ui_jobs.quiet_running += 1;
        let then: Then<C> = Arc::new(Mutex::new(Some(then)));
        let worker_then = Arc::clone(&then);
        let tx = self.ui_jobs.tx.clone();
        #[cfg(test)]
        let injected = FAIL_NEXT_QUIET_SPAWN.with(|fail| fail.replace(false));
        #[cfg(not(test))]
        let injected = false;
        let spawned = if injected {
            Err(std::io::Error::other("injected test failure"))
        } else {
            thread::Builder::new()
                .name("dux-ui-lookup".into())
                .spawn(move || {
                    let answer = std::panic::catch_unwind(AssertUnwindSafe(work));
                    let continuation: Continuation = Box::new(move |app: &mut App| {
                        app.ui_jobs.quiet_running -= 1;
                        let answer = answer
                            .map_err(|payload| {
                                let reason = dux_core::engine::format_panic_payload(&*payload);
                                dux_core::logger::error(&format!("ui lookup panicked: {reason}"));
                            })
                            .ok();
                        if let Some(then) = take_then(&worker_then) {
                            then(app, answer);
                        }
                    });
                    let _ = tx.send((u64::MAX, continuation));
                })
                .map(|_| ())
        };
        if let Err(err) = spawned {
            self.ui_jobs.quiet_running -= 1;
            dux_core::logger::error(&format!("could not start a ui lookup worker: {err}"));
            if let Some(then) = take_then(&then) {
                then(self, None);
            }
        }
    }

    /// Why a worker's answer may not open a dialog now, or `None` when it
    /// may: another dialog is open, or a pane is taking keystrokes, where a
    /// dialog appearing would take keys meant for the terminal. The answer is
    /// then dropped and the caller says so, rather than held for later.
    pub(crate) fn job_dialog_blocker(&self) -> Option<&'static str> {
        if !matches!(self.prompt, super::PromptState::None) {
            Some("another dialog is open")
        } else if self.input_target != super::InputTarget::None {
            Some("a pane is taking your keystrokes")
        } else {
            None
        }
    }

    /// Run every continuation whose job has answered.
    pub(crate) fn drain_ui_jobs(&mut self) {
        let mut any = false;
        while let Ok((_, continuation)) = self.ui_jobs.rx.try_recv() {
            continuation(self);
            any = true;
        }
        if any {
            self.mark_frame_dirty();
        }
    }

    /// End every job still out with a final that says nothing came of it,
    /// for a hand-over to the web server: nothing on this surface will drain
    /// their answers, so their busies must not outlive it.
    pub(crate) fn retire_ui_jobs(&mut self) {
        let ids: Vec<u64> = self.ui_jobs.running.keys().copied().collect();
        for id in ids {
            self.finish_ui_job(id, &UiJobEnd::HandedOver);
        }
    }

    fn finish_ui_job(&mut self, id: u64, end: &UiJobEnd) {
        if let Some(op) = self.ui_jobs.running.remove(&id) {
            self.apply_reaction(op.resolve(end).into_reaction());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, test_app};
    use crate::statusline::StatusTone;

    fn drain_until_idle(app: &mut App) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.ui_jobs.is_idle() {
            assert!(
                std::time::Instant::now() < deadline,
                "the job never answered"
            );
            app.drain_ui_jobs();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The UI thread is free while the work runs: the job shows its busy and
    /// returns at once, and the continuation runs only once the work answers.
    #[test]
    fn a_job_runs_off_the_ui_thread_and_continues_on_it() {
        let mut app = test_app(default_bindings());
        let (release, gate) = mpsc::channel::<()>();
        app.spawn_ui_job(
            UiJob {
                busy: "Working\u{2026}".into(),
                stopped: "stopped".to_string(),
            },
            move || {
                gate.recv().expect("released");
                7
            },
            |app: &mut App, answer: Option<i32>| {
                app.set_info(format!("answer {}", answer.expect("answered")));
            },
        );
        assert_eq!(app.status.tone(), StatusTone::Busy);
        app.drain_ui_jobs();
        assert_eq!(app.status.tone(), StatusTone::Busy, "nothing answered yet");

        release.send(()).expect("release");
        drain_until_idle(&mut app);
        assert_eq!(app.status.text(), "answer 7");
    }

    /// A hand-over to the web server ends the jobs still out, so their
    /// busies are not kept alive with nothing left to answer them.
    #[test]
    fn a_hand_over_retires_the_jobs_still_out() {
        let mut app = test_app(default_bindings());
        let (_release, gate) = mpsc::channel::<()>();
        app.spawn_ui_job(
            UiJob {
                busy: "Working\u{2026}".into(),
                stopped: "stopped".to_string(),
            },
            move || {
                let _ = gate.recv();
            },
            |_: &mut App, _: Option<()>| {},
        );
        let key = app
            .ui_jobs
            .running
            .values()
            .next()
            .expect("a job is out")
            .id()
            .to_string();
        assert!(app.engine.status_op_is_live(&key));

        let engine = app.into_engine();

        assert!(!engine.status_op_is_live(&key));
    }

    /// A job whose work panics retires its busy with the sentence it was
    /// given, rather than leaving a spinner nothing will answer, and its
    /// continuation still runs, with no answer, so what it kept is let go.
    #[test]
    fn a_job_that_panics_ends_on_its_stopped_sentence() {
        let mut app = test_app(default_bindings());
        let told = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&told);
        app.spawn_ui_job(
            UiJob {
                busy: "Working\u{2026}".into(),
                stopped: "dux could not finish the work.".to_string(),
            },
            || -> i32 { panic!("boom") },
            move |_: &mut App, answer: Option<i32>| {
                assert!(answer.is_none(), "a panicked job has no answer");
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
            },
        );
        drain_until_idle(&mut app);
        assert!(told.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(app.status.tone(), StatusTone::Error);
        assert_eq!(app.status.text(), "dux could not finish the work.");
    }
}

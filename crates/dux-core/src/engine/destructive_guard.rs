//! The engine thread never clears a destructive operation: it only claims
//! (in memory) and asks the occupancy question of its own state. Clearing
//! opens the session database and reads the process table, which can wait on
//! another writer's lock or on the kernel, and the engine thread is the
//! terminal UI's. Test builds enforce this: a destructive command marks the
//! engine thread for as long as it runs there, and the blocking reads panic
//! when they find the mark.

#[cfg(test)]
thread_local! {
    static ON_ENGINE_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The engine thread is running a destructive command until this drops.
pub struct EngineThreadMark {
    #[cfg(test)]
    previous: bool,
}

/// Mark the current thread as the engine thread running a destructive
/// command (test builds only; a no-op otherwise).
pub fn engine_thread() -> EngineThreadMark {
    EngineThreadMark {
        #[cfg(test)]
        previous: ON_ENGINE_THREAD.with(|mark| mark.replace(true)),
    }
}

impl Drop for EngineThreadMark {
    fn drop(&mut self) {
        #[cfg(test)]
        ON_ENGINE_THREAD.with(|mark| mark.set(self.previous));
    }
}

/// Panic, in test builds, when `what` (a blocking read a destructive
/// clearance does) runs on the engine thread during a destructive command.
pub fn assert_off_engine_thread(what: &str) {
    #[cfg(test)]
    if ON_ENGINE_THREAD.with(std::cell::Cell::get) {
        panic!("{what} ran on the engine thread during a destructive command");
    }
    #[cfg(not(test))]
    let _ = what;
}

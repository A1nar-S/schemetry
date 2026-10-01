//! Cancellation of the long-running task behind the busy overlay.
//!
//! A command wraps its work in [`begin_task`]; [`cancel_task`] (the overlay's Stop
//! button) then interrupts it. Each database connection opened by the task registers
//! how to abort its running statement via [`on_cancel`], and loops that issue many
//! statements call [`checkpoint`] between them.
//!
//! The task belongs to the thread that began it and to worker threads started with
//! [`spawn`] — work on other threads (e.g. background autocomplete fetches) is never
//! affected. Only the latest task can be stopped: the overlay is modal, so there is
//! never more than one to stop.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{bail, Result};

pub const CANCELLED: &str = "Cancelled by user.";

type Hook = Box<dyn Fn() + Send + Sync>;

struct Cancellation {
    /// `None` once cancelled.
    hooks: Mutex<Option<Vec<Hook>>>,
}

impl Cancellation {
    fn is_cancelled(&self) -> bool {
        self.hooks.lock().unwrap().is_none()
    }
}

/// The task the Stop button cancels.
static ACTIVE_TASK: Mutex<Option<Arc<Cancellation>>> = Mutex::new(None);

thread_local! {
    /// The task this thread's work belongs to.
    static CURRENT: RefCell<Option<Arc<Cancellation>>> = const { RefCell::new(None) };
}

fn current() -> Option<Arc<Cancellation>> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Makes `task` this thread's current task until dropped.
struct Scope {
    previous: Option<Arc<Cancellation>>,
}

impl Scope {
    fn enter(task: Option<Arc<Cancellation>>) -> Self {
        Self { previous: CURRENT.with(|c| c.replace(task)) }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|c| *c.borrow_mut() = self.previous.take());
    }
}

/// Keeps the task cancellable until dropped.
pub struct TaskGuard {
    cancellation: Arc<Cancellation>,
    _scope: Scope,
}

impl TaskGuard {
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    /// Fails with [`CANCELLED`] if the task was stopped.
    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() { Err(CANCELLED.to_string()) } else { Ok(()) }
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        let mut active = ACTIVE_TASK.lock().unwrap();
        if active.as_ref().is_some_and(|c| Arc::ptr_eq(c, &self.cancellation)) {
            *active = None;
        }
    }
}

/// Starts a cancellable task on this thread, replacing any previous one as the
/// task [`cancel_task`] stops.
pub fn begin_task() -> TaskGuard {
    let cancellation = Arc::new(Cancellation { hooks: Mutex::new(Some(Vec::new())) });
    *ACTIVE_TASK.lock().unwrap() = Some(Arc::clone(&cancellation));
    TaskGuard { _scope: Scope::enter(Some(Arc::clone(&cancellation))), cancellation }
}

/// Stops the active task, if any. Returns whether there was one.
pub fn cancel_task() -> bool {
    let Some(cancellation) = ACTIVE_TASK.lock().unwrap().clone() else { return false };
    let hooks = cancellation.hooks.lock().unwrap().take();
    for hook in hooks.into_iter().flatten() {
        hook();
    }
    true
}

/// `thread::spawn` whose thread belongs to the caller's task.
pub fn spawn<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> JoinHandle<T> {
    let task = current();
    thread::spawn(move || {
        let _scope = Scope::enter(task);
        f()
    })
}

/// Registers `hook` to interrupt work of this thread's task; fails if it was already
/// cancelled. A no-op outside a task.
pub fn on_cancel(hook: Hook) -> Result<()> {
    let Some(cancellation) = current() else { return Ok(()) };
    match cancellation.hooks.lock().unwrap().as_mut() {
        Some(hooks) => {
            hooks.push(hook);
            Ok(())
        }
        None => bail!(CANCELLED),
    }
}

/// Whether this thread's task was cancelled — e.g. to tell a Stop apart from a real
/// failure (Oracle reports the interrupted statement as ORA-01013).
pub fn is_cancelled() -> bool {
    current().is_some_and(|c| c.is_cancelled())
}

/// Fails once this thread's task was cancelled — call between statements.
pub fn checkpoint() -> Result<()> {
    if is_cancelled() {
        bail!(CANCELLED);
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/cancel.rs"]
mod tests;

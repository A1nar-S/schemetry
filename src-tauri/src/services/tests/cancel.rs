use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

// One test: `cancel_task` targets the process-wide latest task, so parallel tests
// beginning tasks would interfere.
#[test]
fn task_lifecycle() {
    assert!(checkpoint().is_ok(), "no task, nothing to cancel");
    assert!(on_cancel(Box::new(|| {})).is_ok());

    let calls = Arc::new(AtomicUsize::new(0));
    let task = begin_task();
    let counter = Arc::clone(&calls);
    on_cancel(Box::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }))
    .unwrap();
    assert!(checkpoint().is_ok());
    assert!(task.check().is_ok());

    assert!(cancel_task());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(task.is_cancelled());
    assert_eq!(task.check(), Err(CANCELLED.to_string()));
    assert_eq!(checkpoint().unwrap_err().to_string(), CANCELLED);
    // Late registrations (a connection opened after Stop) are refused.
    assert_eq!(on_cancel(Box::new(|| {})).unwrap_err().to_string(), CANCELLED);
    // Hooks run once only.
    assert!(cancel_task());
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Worker threads started with `spawn` share the task; plain threads don't.
    assert!(spawn(is_cancelled).join().unwrap());
    assert!(!thread::spawn(is_cancelled).join().unwrap());

    // A nested task shadows the outer one, which is current again once it ends.
    let newer = begin_task();
    assert!(checkpoint().is_ok());
    drop(newer);
    assert!(is_cancelled());
    drop(task);
    assert!(!is_cancelled());
    assert!(!cancel_task(), "no task left to stop");
}

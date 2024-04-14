//! A simple test case using a "counter" type.

use event_listener::Event;
use futures_lite::future::{block_on, poll_once};

use std::sync::atomic::{fence, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::thread;

struct Counter {
    counter: AtomicUsize,

    /// Signalled once `counter` has been changed.
    changed: Event,
}

impl Counter {
    fn new() -> Self {
        Self {
            counter: AtomicUsize::new(0),
            changed: Event::new(),
        }
    }

    /// Wait for the counter to be incremented.
    async fn change(&self) -> usize {
        let original = self.counter.load(Ordering::Acquire);
        let mut current = original;

        loop {
            if current != original {
                return current;
            }

            // Start listening.
            let listener = self.changed.listen();

            // Try again.
            current = self.counter.load(Ordering::Acquire);
            if current != original {
                return current;
            }

            // Wait for a change to be notified.
            listener.await;

            // Update the counter.
            current = self.counter.load(Ordering::Acquire);
        }
    }

    /// Increment the counter.
    fn increment(&self) {
        self.counter.fetch_add(1, Ordering::Relaxed);
        self.changed.notify_additional(usize::MAX);
    }
}

#[test]
fn counter() {
    let counter = Arc::new(Counter::new());
    let (send, recv) = mpsc::channel();

    thread::spawn({
        let counter = counter.clone();
        move || {
            // Test normal.
            recv.recv().unwrap();
            counter.increment();

            // Test relaxed.
            recv.recv().unwrap();
            counter.counter.fetch_add(1, Ordering::Relaxed);
            fence(Ordering::SeqCst);
            counter.changed.notify_additional_relaxed(usize::MAX);
            counter.changed.notify_additional_relaxed(usize::MAX);
        }
    });

    thread::spawn(move || {
        let waiter = counter.change();
        futures_lite::pin!(waiter);

        assert!(block_on(poll_once(waiter.as_mut())).is_none());
        send.send(()).unwrap();
        assert_eq!(block_on(waiter), 1);

        let waiter1 = counter.change();
        let waiter2 = counter.change();
        futures_lite::pin!(waiter1);
        futures_lite::pin!(waiter2);

        assert!(block_on(poll_once(waiter1.as_mut())).is_none());
        assert!(block_on(poll_once(waiter2.as_mut())).is_none());
        send.send(()).unwrap();
        assert_eq!(block_on(waiter1), 2);
        assert_eq!(block_on(waiter2), 2);
    });

    #[cfg(miri)]
    thread::sleep(std::time::Duration::from_secs(5));
}

#[test]
fn simultaneous_notification() {
    let thread_count = if cfg!(miri) { 10 } else { 1000 };
    let barrier = Arc::new(Barrier::new(thread_count + 1));
    let counter = Arc::new(Counter::new());

    for _ in 0..thread_count {
        let barrier = barrier.clone();
        let counter = counter.clone();
        thread::spawn(move || {
            // Wait for a listener to be created.
            barrier.wait();

            // Notify all at the same time.
            counter.increment();
        });
    }

    // Wait for a notification.
    let listener = counter.change();
    futures_lite::pin!(listener);
    assert!(block_on(poll_once(listener.as_mut())).is_none());

    // Signal the other threads.
    barrier.wait();

    // Wait to be notified.
    thread::sleep(std::time::Duration::from_secs(3));
    block_on(listener);
}

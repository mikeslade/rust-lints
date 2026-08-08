//! Blocking-in-async fixture: calls that genuinely park a Tokio worker thread,
//! next to async APIs that merely share a method NAME with them.
//!
//! Every case lives inside an `async` block, so the pass's async-context
//! precondition holds for all of them and the only thing under test is whether
//! the call itself is judged blocking. The async shapes here are hand-written
//! rather than borrowed from Tokio so the fixture workspace stays
//! dependency-free: what the pass has to distinguish is the *type* the call
//! produces, and a synthetic `async fn` produces the same shape Tokio's does.

use std::future::Future;
use std::pin::Pin;
use std::process::Command;
use std::sync::mpsc;
use std::task::{Context, Poll};

// ---------------------------------------------------------------------------
// Async APIs whose method names collide with the blocking ones.
// ---------------------------------------------------------------------------

pub struct AsyncReceiver;

impl AsyncReceiver {
    /// The `tokio::sync::mpsc::Receiver::recv` / `tokio::signal::unix::Signal::recv`
    /// shape: an `async fn`, so the call evaluates to a future and parks nothing.
    pub async fn recv(&mut self) -> Option<u8> {
        None
    }
}

pub struct ShutdownHandle;

impl ShutdownHandle {
    /// The `ShutdownHandle::wait` shape from the consuming repo.
    pub async fn wait(&self) {}
}

/// A hand-written future: the `futures` / `async-channel` shape, where a plain
/// `fn` returns a named struct that implements `Future`. Not an `async fn`, so
/// a check keyed on asyncness alone would still misjudge it.
pub struct RecvFuture;

impl Future for RecvFuture {
    type Output = Option<u8>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(None)
    }
}

pub struct PollReceiver;

impl PollReceiver {
    pub fn recv(&self) -> RecvFuture {
        RecvFuture
    }
}

/// The boxed-future shape: `Pin<Box<dyn Future>>` behind a plain `fn`.
pub struct BoxedReceiver;

impl BoxedReceiver {
    pub fn recv(&self) -> Pin<Box<dyn Future<Output = Option<u8>> + Send>> {
        Box::pin(async { None })
    }
}

// COMPLIANT: an awaited `async fn recv`.
pub fn async_recv_inside_async_context(mut rx: AsyncReceiver) {
    let _future = async move {
        let _message = rx.recv().await;
    };
}

// COMPLIANT: an awaited `async fn wait`.
pub fn async_wait_inside_async_context(shutdown: ShutdownHandle) {
    let _future = async move {
        shutdown.wait().await;
    };
}

// COMPLIANT: the future is constructed but polled elsewhere (the
// `tokio::select!` shape, where a branch expression is evaluated into a future
// the macro polls). There is no `.await` at this call site, so only the value's
// type distinguishes it from a blocking call.
pub fn unawaited_async_recv_inside_async_context(mut rx: AsyncReceiver) {
    let _future = async move {
        let pending = rx.recv();
        let _message = pending.await;
    };
}

// COMPLIANT: a plain `fn` returning a named `Future` struct.
pub fn poll_receiver_recv_inside_async_context(rx: PollReceiver) {
    let _future = async move {
        let _message = rx.recv().await;
    };
}

// COMPLIANT: a plain `fn` returning a boxed future.
pub fn boxed_receiver_recv_inside_async_context(rx: BoxedReceiver) {
    let _future = async move {
        let _message = rx.recv().await;
    };
}

// COMPLIANT: an `async fn` reached through a generic bound, where the resolved
// path names the trait rather than any known async library.
pub trait AsyncSource {
    fn recv(&self) -> impl Future<Output = u8> + Send;
}

pub fn generic_async_recv_inside_async_context<S: AsyncSource + Send + 'static>(source: S) {
    let _future = async move {
        let _value = source.recv().await;
    };
}

// ---------------------------------------------------------------------------
// Genuinely blocking calls. Every one of these must stay flagged.
// ---------------------------------------------------------------------------

// VIOLATION: `std::sync::mpsc::Receiver::recv` parks the worker thread.
pub fn std_mpsc_recv_inside_async_context(rx: mpsc::Receiver<u8>) {
    let _future = async move {
        let _message = rx.recv(); // blocking-in-async: expect
    };
}

// VIOLATION: the timeout variant parks the worker thread too.
pub fn std_mpsc_recv_timeout_inside_async_context(rx: mpsc::Receiver<u8>) {
    let _future = async move {
        let _message = rx.recv_timeout(std::time::Duration::from_secs(1)); // blocking-in-async: expect
    };
}

// VIOLATION: waiting on a child process parks the worker thread.
pub fn child_wait_inside_async_context(mut child: std::process::Child) {
    let _future = async move {
        let _status = child.wait(); // blocking-in-async: expect
    };
}

// VIOLATION: running a command to completion parks the worker thread.
pub fn command_output_inside_async_context(mut command: Command) {
    let _future = async move {
        let _output = command.output(); // blocking-in-async: expect
    };
}

// KNOWN GAP: joining a thread parks the worker thread, but `join` is in neither
// the method-name set nor the blocking-path prefixes, so this is not flagged
// today. Recorded here so the gap is visible rather than assumed covered;
// closing it is a coverage change, not part of the false-positive fix.
pub fn join_handle_join_inside_async_context(handle: std::thread::JoinHandle<u8>) {
    let _future = async move {
        let _value = handle.join();
    };
}

/// A synthetic executor entry point, standing in for `futures::executor::block_on`.
pub fn block_on<F: Future>(_future: F) -> F::Output {
    unimplemented!("fixture only")
}

// VIOLATION: driving an executor from inside one deadlocks the worker thread.
pub fn block_on_inside_async_context() {
    let _future = async {
        let _value = block_on(async { 1_u8 }); // blocking-in-async: expect
    };
}

/// A blocking channel wrapper of the kind a product crate writes around
/// `std::sync::mpsc`. Its resolved path names this crate, not `std`, so nothing
/// in the path allowlist can reach it.
pub struct BlockingChannel {
    inner: mpsc::Receiver<u8>,
}

impl BlockingChannel {
    pub fn recv(&self) -> Option<u8> {
        self.inner.recv().ok()
    }
}

// VIOLATION: a project-local blocking `recv`, caught by name alone.
pub fn wrapper_recv_inside_async_context(channel: BlockingChannel) {
    let _future = async move {
        let _message = channel.recv(); // blocking-in-async: expect
    };
}

/// A blocking trait, the shape a generic worker abstraction takes.
pub trait BlockingSource {
    fn recv(&self) -> u8;
    fn wait(&self);
}

// VIOLATION: a blocking call behind a generic bound, where the resolved path
// names the trait and so cannot be matched against `std::`. Only the method
// name identifies it, which is why the name arm has to survive the fix.
pub fn generic_blocking_recv_inside_async_context<S: BlockingSource + Send + 'static>(source: S) {
    let _future = async move {
        let _value = source.recv(); // blocking-in-async: expect
    };
}

// VIOLATION: the same, for `wait`.
pub fn generic_blocking_wait_inside_async_context<S: BlockingSource + Send + 'static>(source: S) {
    let _future = async move {
        source.wait(); // blocking-in-async: expect
    };
}

/// A synthetic blocking-pool entry point, standing in for
/// `tokio::task::spawn_blocking`.
pub fn spawn_blocking<F, R>(work: F) -> R
where
    F: FnOnce() -> R,
{
    work()
}

// COMPLIANT: the same blocking `recv`, quarantined on the blocking pool.
pub fn quarantined_std_mpsc_recv_inside_async_context(rx: mpsc::Receiver<u8>) {
    let _future = async move {
        let _message = spawn_blocking(move || rx.recv());
    };
}

use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::{Event, KeepAlive, Sse};
use futures::Stream;
use futures::future::BoxFuture;

/// Build a Server-Sent Events stream that emits a lightweight `change` ping: once on connect,
/// again whenever `wake` resolves (a store change notification), and at least every
/// `backstop` as a safety net. The browser uses each ping to invalidate a query and refetch,
/// so a page updates on change instead of polling on a fixed timer.
///
/// `wake` is a closure returning a fresh future each call (it is awaited in a loop). The
/// future typically awaits a notify channel such as `RunStore::await_runs_change`. Auth is by
/// the session cookie the browser `EventSource` sends; gate the calling handler accordingly.
pub fn change_stream(
    backstop: Duration,
    wake: impl Fn() -> BoxFuture<'static, ()> + Send + 'static,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        // Emit once on connect so the client refetches its initial state immediately.
        yield Ok(Event::default().event("change").data("change"));
        loop {
            tokio::select! {
                _ = wake() => {}
                _ = tokio::time::sleep(backstop) => {}
            }
            yield Ok(Event::default().event("change").data("change"));
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Build a Server-Sent Events stream that pushes a payload snapshot: on connect, whenever
/// `wake` resolves, and at least every `backstop`. `snapshot` computes the next `Event` and
/// whether the resource has reached a terminal state; the stream emits it and then closes
/// once terminal (or when `snapshot` returns `None`, e.g. the resource is gone). Use this
/// when the client needs the data itself (a single run's status/output, rotation progress)
/// rather than a refetch ping.
pub fn snapshot_stream(
    backstop: Duration,
    wake: impl Fn() -> BoxFuture<'static, ()> + Send + 'static,
    snapshot: impl Fn() -> BoxFuture<'static, Option<(Event, bool)>> + Send + 'static,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        loop {
            match snapshot().await {
                Some((event, terminal)) => {
                    yield Ok(event);
                    if terminal {
                        break;
                    }
                }
                None => break,
            }
            tokio::select! {
                _ = wake() => {}
                _ = tokio::time::sleep(backstop) => {}
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

//! A single-threaded actor around [`Runtime`], mirroring the serialized worker
//! queue in `src/event_loop.c`.
//!
//! All state mutation happens on one owned thread: callers hand work to the
//! actor over a channel, so events (from the macOS layer) and `-m` messages
//! (from the socket loop) are processed strictly in order against the same
//! [`AppState`], never concurrently. This is the threading wrapper the binary
//! will drive; the underlying [`Runtime`] stays synchronous and testable.

use std::sync::mpsc::{Sender, SyncSender, channel, sync_channel};
use std::thread::{self, JoinHandle};

use crate::app_state::{LayoutSink, Response, StateEvent};
use crate::runtime::Runtime;

/// A unit of serialized work for the actor thread.
enum Work<S: LayoutSink> {
    Event(StateEvent),
    Message {
        tokens: Vec<String>,
        reply: SyncSender<Response>,
    },
    Shutdown {
        reply: SyncSender<Runtime<S>>,
    },
}

/// Handle to a running actor thread. Drop or call [`Actor::shutdown`] to stop.
pub struct Actor<S: LayoutSink> {
    tx: Sender<Work<S>>,
    join: Option<JoinHandle<()>>,
}

impl<S: LayoutSink + Send + 'static> Actor<S> {
    /// Spawn the actor thread, taking ownership of `runtime`.
    pub fn spawn(mut runtime: Runtime<S>) -> Self {
        let (tx, rx) = channel::<Work<S>>();
        let join = thread::spawn(move || {
            while let Ok(work) = rx.recv() {
                match work {
                    Work::Event(event) => {
                        let _ = runtime.event(event);
                    }
                    Work::Message { tokens, reply } => {
                        let response = runtime.message(&tokens);
                        let _ = reply.send(response);
                    }
                    Work::Shutdown { reply } => {
                        let _ = reply.send(runtime);
                        return;
                    }
                }
            }
        });
        Self {
            tx,
            join: Some(join),
        }
    }

    /// Post an event to be applied on the actor thread (fire-and-forget).
    pub fn post_event(&self, event: StateEvent) {
        let _ = self.tx.send(Work::Event(event));
    }

    /// Send a `-m` token list and block until the actor returns its response.
    pub fn message(&self, tokens: Vec<String>) -> Response {
        let (reply, rx) = sync_channel(0);
        self.tx
            .send(Work::Message { tokens, reply })
            .map_err(|_| "actor thread is gone".to_string())?;
        rx.recv().map_err(|_| "actor thread is gone".to_string())?
    }

    /// Stop the actor and return the final [`Runtime`] (state + sink).
    pub fn shutdown(mut self) -> Runtime<S> {
        let (reply, rx) = sync_channel(0);
        // If the send fails the thread already exited; fall back to joining.
        let _ = self.tx.send(Work::Shutdown { reply });
        let runtime = rx.recv().expect("actor thread dropped without replying");
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        runtime
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::{AppState, RecordingSink};
    use yabai_core::Area;

    fn toks(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn actor_processes_events_then_messages_in_order() {
        let actor = Actor::spawn(Runtime::new(AppState::new(), RecordingSink::default()));
        actor.post_event(StateEvent::SpaceCreated {
            sid: 1,
            frame: Area::new(0.0, 0.0, 1000.0, 1000.0),
        });
        actor.post_event(StateEvent::WindowCreated { window_id: 1 });
        actor.post_event(StateEvent::WindowCreated { window_id: 2 });

        // A query-style message round-trips through the actor thread.
        let response = actor.message(toks(&["space", "--rotate", "180"]));
        assert_eq!(response, Ok(None));

        let runtime = actor.shutdown();
        // rotate 180 swaps the children, so the leaf order flips to [2, 1].
        assert_eq!(runtime.state.space(1).unwrap().window_list(), vec![2, 1]);
        // The last flush recorded both windows after the rotate.
        assert!(runtime.sink.moves.len() >= 2);
    }

    #[test]
    fn actor_returns_command_errors() {
        let actor = Actor::spawn(Runtime::new(AppState::new(), RecordingSink::default()));
        let response = actor.message(toks(&["bogus"]));
        assert_eq!(response, Err("unknown domain 'bogus'".to_string()));
        actor.shutdown();
    }
}

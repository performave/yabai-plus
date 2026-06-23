//! A thin owner that pairs [`AppState`] with a [`LayoutSink`] and re-flows the
//! active layout to the sink after every state change.
//!
//! This captures the C daemon's discipline of flushing the affected view once a
//! command or event has been handled, without yet introducing threads. A real
//! worker thread / socket loop (the actor in `src/event_loop.c`) will wrap a
//! `Runtime` and feed it serialized work in a later step; keeping the core
//! synchronous keeps it deterministic and unit-testable.

use crate::app_state::{AppState, LayoutSink, Response, StateEvent};

/// Owns the daemon state and its layout sink, flushing after each mutation.
#[derive(Debug, Default)]
pub struct Runtime<S: LayoutSink> {
    pub state: AppState,
    pub sink: S,
}

impl<S: LayoutSink> Runtime<S> {
    pub fn new(state: AppState, sink: S) -> Self {
        Self { state, sink }
    }

    /// Apply a system event, then flush the active layout to the sink.
    pub fn event(&mut self, event: StateEvent) -> Result<usize, String> {
        self.state.handle_event_and_flush(event, &mut self.sink)
    }

    /// Handle a raw `-m` token list, then flush the active layout to the sink.
    /// The response (query/get output or failure) is returned unchanged; the
    /// flush happens regardless so layout-mutating commands reach windows.
    pub fn message(&mut self, tokens: &[String]) -> Response {
        let response = self.state.handle_tokens(tokens);
        // Re-flow even on a command error: a partially-applied chain (as the C
        // daemon allows) should still settle the windows it did change.
        self.state.flush_active_to(&mut self.sink);
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::RecordingSink;
    use yabai_core::Area;

    fn toks(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn runtime() -> Runtime<RecordingSink> {
        let mut rt = Runtime::new(AppState::new(), RecordingSink::default());
        rt.event(StateEvent::SpaceCreated {
            sid: 1,
            frame: Area::new(0.0, 0.0, 1000.0, 1000.0),
        })
        .unwrap();
        rt
    }

    #[test]
    fn events_flush_through_the_sink() {
        let mut rt = runtime();
        rt.event(StateEvent::WindowCreated { window_id: 1 })
            .unwrap();
        let placed = rt
            .event(StateEvent::WindowCreated { window_id: 2 })
            .unwrap();
        assert_eq!(placed, 2);
        assert_eq!(rt.sink.moves.last().unwrap().window_id, 2);
    }

    #[test]
    fn message_reflows_after_a_layout_command() {
        let mut rt = runtime();
        rt.event(StateEvent::WindowCreated { window_id: 1 })
            .unwrap();
        rt.event(StateEvent::WindowCreated { window_id: 2 })
            .unwrap();
        rt.sink.moves.clear();

        // A successful space rotate re-flows both windows to the sink.
        let response = rt.message(&toks(&["space", "--rotate", "180"]));
        assert_eq!(response, Ok(None));
        assert_eq!(rt.sink.moves.len(), 2);
    }

    #[test]
    fn message_error_is_returned_but_still_flushes() {
        let mut rt = runtime();
        rt.event(StateEvent::WindowCreated { window_id: 1 })
            .unwrap();
        rt.sink.moves.clear();
        let response = rt.message(&toks(&["bogus"]));
        assert!(response.is_err());
        // The single window is still settled.
        assert_eq!(rt.sink.moves.len(), 1);
    }
}

//! The live connection: the UI side of a
//! [`LiveSession`](fasttick_protocol::session::LiveSession).
//!
//! [`LiveController`] holds the channel ends of the session thread. It
//! never blocks the UI thread: commands go through a channel, and events
//! are read with `try_recv` each frame. The session calls a waker that
//! asks egui for a repaint.

use std::sync::mpsc::Receiver;
use std::time::Duration;

use fasttick_protocol::session::{
    ConnectionState, LiveSession, PollConfig, SessionCommand, SessionConfig, SessionEvent,
    SessionHandle,
};

/// The live connection state of the app.
#[derive(Debug, Default)]
pub struct LiveController {
    session: Option<(SessionHandle, Receiver<SessionEvent>)>,
    state: Option<ConnectionState>,
    endpoint: Option<(String, u16)>,
    /// The last error or warning of the session.
    pub last_message: Option<String>,
}

impl LiveController {
    /// The connection state (disconnected if no session runs).
    pub fn state(&self) -> ConnectionState {
        self.state.unwrap_or(ConnectionState::Disconnected)
    }

    /// The host and port of the current or last connection.
    pub fn endpoint(&self) -> Option<(&str, u16)> {
        self.endpoint.as_ref().map(|(h, p)| (h.as_str(), *p))
    }

    /// The endpoint as `host:port`.
    pub fn endpoint_text(&self) -> Option<String> {
        self.endpoint.as_ref().map(|(h, p)| format!("{h}:{p}"))
    }

    /// True if a session is connected or tries to connect.
    pub fn is_active(&self) -> bool {
        !matches!(self.state(), ConnectionState::Disconnected)
    }

    /// Connects to an executor. Starts the session thread if necessary.
    pub fn connect(&mut self, ctx: &egui::Context, host: &str, port: u16, poll_interval_ms: u32) {
        let interval = Duration::from_millis(u64::from(poll_interval_ms.max(10)));
        if self.session.is_none() {
            let config = SessionConfig {
                poll: PollConfig {
                    status_interval: interval,
                    ..PollConfig::default()
                },
                ..SessionConfig::default()
            };
            let waker_ctx = ctx.clone();
            let waker: fasttick_protocol::session::Waker = Box::new(move || waker_ctx.request_repaint());
            self.session = Some(LiveSession::spawn(config, Some(waker)));
        } else if let Some((handle, _)) = &self.session {
            handle.send(SessionCommand::SetPollInterval(interval));
        }
        if let Some((handle, _)) = &self.session {
            handle.connect(host, port);
        }
        self.endpoint = Some((host.to_owned(), port));
        self.state = Some(ConnectionState::Connecting);
        self.last_message = None;
    }

    /// Disconnects. The session thread removes the hooks of this session.
    pub fn disconnect(&mut self) {
        if let Some((handle, _)) = &self.session {
            handle.disconnect();
        }
        self.state = Some(ConnectionState::Disconnected);
    }

    /// Changes the status poll interval.
    pub fn set_poll_interval(&mut self, poll_interval_ms: u32) {
        if let Some((handle, _)) = &self.session {
            handle.send(SessionCommand::SetPollInterval(Duration::from_millis(u64::from(
                poll_interval_ms.max(10),
            ))));
        }
    }

    /// The events since the last call. Updates the state.
    pub fn poll(&mut self) -> Vec<SessionEvent> {
        let Some((_, receiver)) = &self.session else {
            return Vec::new();
        };
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            match &event {
                SessionEvent::State(state) => self.state = Some(*state),
                SessionEvent::Error(message) | SessionEvent::Warning(message) => {
                    self.last_message = Some(message.clone());
                }
                _ => {}
            }
            events.push(event);
        }
        events
    }

    /// Stops the session thread and waits for its cleanup (short).
    pub fn shutdown(&mut self) {
        if let Some((handle, _)) = self.session.take() {
            handle.join();
        }
        self.state = Some(ConnectionState::Disconnected);
    }

    /// A short text for the status bar.
    pub fn state_text(&self) -> String {
        match self.state() {
            ConnectionState::Disconnected => "Not connected".to_owned(),
            ConnectionState::Connecting => match self.endpoint_text() {
                Some(e) => format!("Connecting to {e}"),
                None => "Connecting".to_owned(),
            },
            ConnectionState::Connected { .. } => match self.endpoint_text() {
                Some(e) => format!("Connected to {e}"),
                None => "Connected".to_owned(),
            },
            ConnectionState::Reconnecting { attempt } => format!("Reconnecting (attempt {attempt})"),
        }
    }
}

impl Drop for LiveController {
    fn drop(&mut self) {
        // Do not wait in drop; the session cleans up when the handle drops.
        if let Some((handle, _)) = &self.session {
            handle.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_to_nothing_and_shut_down() {
        let ctx = egui::Context::default();
        let mut live = LiveController::default();
        assert!(!live.is_active());
        assert_eq!(live.state_text(), "Not connected");
        // Port 9 (discard) on localhost: nothing answers there.
        live.connect(&ctx, "127.0.0.1", 9, 50);
        assert!(live.is_active());
        assert_eq!(live.endpoint(), Some(("127.0.0.1", 9)));
        assert!(live.state_text().starts_with("Connecting"));
        let _ = live.poll();
        live.set_poll_interval(100);
        live.disconnect();
        assert!(!live.is_active());
        live.shutdown();
        assert!(live.poll().is_empty());
    }
}

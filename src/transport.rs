// SPDX-License-Identifier: GPL-3.0-or-later

//! The pipe between this Passport and the companion on the phone.
//!
//! It carries the frames of `nip_ping_core::link` and nothing else, so the app logic does not
//! care what the pipe is:
//!
//! * Simulator: a WebSocket server on port 9878 (`sim_ws`), standing in for QuantumLink.
//! * Passport Prime, KeyOS 1.4: no pipe. QuantumLink exists, but its messages are
//!   Envoy-specific and Foundation-signed; there is no third-party app data channel until
//!   QLv2 (KeyOS 1.5). Messages are written, sealed and queued; they go out when a link is.
//! * KeyOS 1.5+: a QuantumLink implementation of the same `start` / `send` surface.

/// Link state for the UI. `conn` changes on every new connection, so the app can greet a
/// replacement companion even if the link never looked "down".
#[derive(Clone, Debug)]
pub struct Status {
    pub up: bool,
    pub conn: u64,
    pub text: String,
}

pub const PORT: u16 = 9878;

#[cfg(not(target_os = "xous"))]
mod sim_ws;
#[cfg(not(target_os = "xous"))]
pub use sim_ws::Link;

#[cfg(target_os = "xous")]
mod pending {
    use super::Status;

    /// Device build on KeyOS 1.4: there is no app data channel to the phone yet.
    #[derive(Clone)]
    pub struct Link;

    impl Link {
        pub fn start(_on_frame: impl Fn(String) + 'static, on_status: impl Fn(Status) + 'static) -> Self {
            on_status(Status {
                up: false,
                conn: 0,
                text: "QuantumLink opens to apps with QLv2 (KeyOS 1.5). Messages you write are sealed and queued until then."
                    .into(),
            });
            Link
        }

        pub fn send(&self, _frame: &str) -> bool { false }
    }
}
#[cfg(target_os = "xous")]
pub use pending::Link;

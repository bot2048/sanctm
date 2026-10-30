// SPDX-License-Identifier: GPL-3.0-or-later

//! Simulator link: a WebSocket server the companion page connects to, standing in for
//! QuantumLink. One companion at a time: a new connection replaces the old one (like
//! re-pairing a phone). Browser origins are checked, so other websites cannot connect.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use super::{Status, PORT};

/// The link port: 9878, or `NIP_PING_LINK_PORT` (to run several simulators side by side in tests).
fn port() -> u16 { std::env::var("NIP_PING_LINK_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(PORT) }

type Slot = Arc<Mutex<Option<(u64, mpsc::UnboundedSender<String>)>>>;

#[derive(Clone)]
pub struct Link {
    current: Slot,
}

static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

/// Host part of an Origin or Host header value ("http://10.0.0.5:8000" -> "10.0.0.5").
fn host_of(v: &str) -> &str {
    let v = v.split_once("://").map(|(_, rest)| rest).unwrap_or(v);
    if let Some(rest) = v.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default();
    }
    v.split([':', '/']).next().unwrap_or_default()
}

/// Accept the companion page served from this machine (localhost, or 0.0.0.0, the address
/// `python3 -m http.server` prints) or from the address the browser used to reach us, when
/// that is an IP address; refuse every other website, so a page you happen to visit cannot
/// drive the link. Not accepted: Origin "null" (a sandboxed iframe on any site sends it) and
/// host names matching the Host header (DNS rebinding: an attacker's name that resolves to
/// this machine). Non-browser clients send no Origin.
fn check_origin(req: &Request, resp: Response) -> Result<Response, ErrorResponse> {
    let header = |name: &str| req.headers().get(name).and_then(|v| v.to_str().ok());
    let Some(origin) = header("origin") else { return Ok(resp) };
    let host = host_of(origin);
    let ok = matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
        || (host.parse::<std::net::IpAddr>().is_ok() && header("host").map(host_of) == Some(host));
    if ok {
        return Ok(resp);
    }
    log::warn!("link: refused origin {origin}");
    let mut err = ErrorResponse::new(Some("origin not allowed".into()));
    *err.status_mut() = StatusCode::FORBIDDEN;
    Err(err)
}

impl Link {
    /// Start the server thread. Both callbacks run on the server thread and must hop to the
    /// UI thread themselves.
    pub fn start(on_frame: impl Fn(String) + Send + Sync + 'static, on_status: impl Fn(Status) + Send + Sync + 'static) -> Self {
        let link = Link { current: Arc::new(Mutex::new(None)) };
        let slot = link.current.clone();
        let on_frame = Arc::new(on_frame);
        let on_status = Arc::new(on_status);
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(2).build() {
                Ok(rt) => rt,
                Err(e) => {
                    on_status(Status { up: false, conn: 0, text: format!("Link failed to start: {e}") });
                    return;
                }
            };
            rt.block_on(async move {
                let port = port();
                let listener = match TcpListener::bind(("0.0.0.0", port)).await {
                    Ok(l) => l,
                    Err(e) => {
                        on_status(Status { up: false, conn: 0, text: format!("Port {port} is busy ({e}). Close the other app using it.") });
                        return;
                    }
                };
                log::info!("nip-ping link listening on 0.0.0.0:{port}");
                on_status(Status { up: false, conn: 0, text: format!("Open the companion page on your phone or laptop (link port {port}).") });
                // At most a few handshakes at a time, each given 10 s: sockets that never send
                // the upgrade request must not use up the process's descriptors.
                let handshakes = Arc::new(tokio::sync::Semaphore::new(8));
                loop {
                    let (stream, addr) = match listener.accept().await {
                        Ok(c) => c,
                        Err(e) => {
                            // Out of descriptors (EMFILE) fails at once: back off, do not spin.
                            log::warn!("link: accept failed: {e}");
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                            continue;
                        }
                    };
                    let Ok(permit) = handshakes.clone().try_acquire_owned() else {
                        log::info!("link: too many pending handshakes; dropped {addr}");
                        continue;
                    };
                    let (slot, on_frame, on_status) = (slot.clone(), on_frame.clone(), on_status.clone());
                    tokio::spawn(async move {
                        // Frames bigger than the Passport will parse are refused by the transport,
                        // before they are buffered whole.
                        let mut cfg = WebSocketConfig::default();
                        cfg.max_message_size = Some(nip_ping_core::link::MAX_FRAME_BYTES);
                        cfg.max_frame_size = Some(nip_ping_core::link::MAX_FRAME_BYTES);
                        let handshake = tokio_tungstenite::accept_hdr_async_with_config(stream, check_origin, Some(cfg));
                        let ws = match tokio::time::timeout(std::time::Duration::from_secs(10), handshake).await {
                            Ok(Ok(ws)) => ws,
                            Ok(Err(e)) => {
                                log::info!("link: handshake from {addr} failed: {e}");
                                return;
                            }
                            Err(_) => {
                                log::info!("link: no handshake from {addr} within 10 s");
                                return;
                            }
                        };
                        drop(permit);
                        let conn = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
                        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
                        // Replacing the sender drops the previous companion's channel, which ends its loop.
                        *slot.lock().unwrap() = Some((conn, tx));
                        log::info!("link: companion #{conn} connected from {addr}");
                        on_status(Status { up: true, conn, text: format!("Companion connected from {}", addr.ip()) });

                        let (mut sink, mut source) = ws.split();
                        loop {
                            tokio::select! {
                                out = rx.recv() => match out {
                                    Some(frame) => {
                                        if sink.send(Message::Text(frame)).await.is_err() { break; }
                                    }
                                    None => break, // replaced by a newer companion
                                },
                                inc = source.next() => match inc {
                                    Some(Ok(Message::Text(text))) => on_frame(text),
                                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                                    Some(Ok(_)) => {}
                                },
                            }
                        }
                        let _ = sink.close().await;
                        let mut cur = slot.lock().unwrap();
                        if matches!(&*cur, Some((c, _)) if *c == conn) {
                            *cur = None;
                            drop(cur);
                            log::info!("link: companion #{conn} disconnected");
                            on_status(Status { up: false, conn, text: format!("Companion disconnected. Waiting on port {port}.") });
                        }
                    });
                }
            });
        });
        link
    }

    /// Queue a frame for the current companion. False if none is connected.
    pub fn send(&self, frame: &str) -> bool {
        match &*self.current.lock().unwrap() {
            Some((_, tx)) => tx.send(frame.to_string()).is_ok(),
            None => false,
        }
    }
}

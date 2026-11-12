//! Local WebSocket server for plugins and property inspectors.
//! 127.0.0.1 only, host-issued registration UUIDs, Origin check against our own webview origins.

use crate::plugins::protocol::{parse_inbound, Inbound};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

/// Origins of our own webviews (editor + property-inspector scheme). Native plugins send no Origin.
pub const ALLOWED_ORIGINS: &[&str] = &[
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
    "gdplugin://localhost",
    "http://gdplugin.localhost",
    "https://gdplugin.localhost",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registrant {
    Plugin(String),
    Pi { plugin: String, context: String },
}

/// Registration UUIDs issued by the host; only these may register.
#[derive(Default)]
pub struct Tokens(Mutex<HashMap<String, Registrant>>);

impl Tokens {
    pub fn insert(&self, uuid: String, who: Registrant) {
        self.0.lock().unwrap().insert(uuid, who);
    }
    pub fn remove(&self, uuid: &str) {
        self.0.lock().unwrap().remove(uuid);
    }
    pub fn get(&self, uuid: &str) -> Option<Registrant> {
        self.0.lock().unwrap().get(uuid).cloned()
    }
}

/// A fresh 128-bit hex token.
pub fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::with_capacity(32);
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

#[derive(Debug)]
pub enum ServerEvent {
    Connected { uuid: String, who: Registrant, tx: UnboundedSender<String> },
    Message { uuid: String, who: Registrant, msg: Inbound },
    Disconnected { uuid: String, who: Registrant },
}

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub register_timeout: Duration,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self { register_timeout: Duration::from_secs(10) }
    }
}

#[derive(Debug, Clone)]
pub struct Server {
    pub port: u16,
    pub addr: SocketAddr,
}

pub async fn start(tokens: Arc<Tokens>, events: UnboundedSender<ServerEvent>, opts: ServerOptions) -> std::io::Result<Server> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            let (tokens, events, opts) = (tokens.clone(), events.clone(), opts.clone());
            tokio::spawn(async move {
                if let Err(e) = handle(stream, tokens, events, opts).await {
                    log::debug!("plugin socket ended: {e}");
                }
            });
        }
    });
    Ok(Server { port: addr.port(), addr })
}

fn check_origin(req: &Request, resp: Response) -> Result<Response, ErrorResponse> {
    match req.headers().get("origin").and_then(|o| o.to_str().ok()) {
        None => Ok(resp),
        Some(o) if ALLOWED_ORIGINS.iter().any(|a| a.eq_ignore_ascii_case(o)) => Ok(resp),
        Some(o) => {
            log::warn!("rejected plugin websocket from origin {o}");
            let mut r = ErrorResponse::new(Some("forbidden origin".into()));
            *r.status_mut() = StatusCode::FORBIDDEN;
            Err(r)
        }
    }
}

/// Diagnostic metadata only: never log SDK payloads or registration tokens.
fn wire_diagnostic(who: &Registrant, text: &str, msg: &Inbound) -> serde_json::Value {
    let plugin = match who {
        Registrant::Plugin(id) | Registrant::Pi { plugin: id, .. } => id,
    };
    let event = serde_json::from_str::<serde_json::Value>(text).ok().and_then(|v| {
        v.get("event").and_then(|e| e.as_str()).map(|s| s.chars().take(128).collect::<String>())
    });
    let classification = match msg {
        Inbound::Unknown => "unknown",
        Inbound::Invalid => "invalid",
        _ => "known",
    };
    serde_json::json!({"plugin":plugin,"kind":"inbound","event":event,"classification":classification})
}

async fn handle(stream: TcpStream, tokens: Arc<Tokens>, events: UnboundedSender<ServerEvent>, opts: ServerOptions) -> anyhow::Result<()> {
    let ws = tokio_tungstenite::accept_hdr_async(stream, check_origin).await?;
    let (mut sink, mut source) = ws.split();

    // first message must be a registration with a host-issued uuid of the right kind
    let first = tokio::time::timeout(opts.register_timeout, source.next()).await;
    let Ok(Some(Ok(Message::Text(text)))) = first else {
        let _ = sink.close().await;
        anyhow::bail!("no registration");
    };
    let (uuid, who) = match parse_inbound(text.as_str()) {
        Inbound::RegisterPlugin { uuid } => match tokens.get(&uuid) {
            Some(w @ Registrant::Plugin(_)) => (uuid, w),
            _ => (String::new(), Registrant::Plugin(String::new())),
        },
        Inbound::RegisterPropertyInspector { uuid } => match tokens.get(&uuid) {
            Some(w @ Registrant::Pi { .. }) => (uuid, w),
            _ => (String::new(), Registrant::Plugin(String::new())),
        },
        _ => (String::new(), Registrant::Plugin(String::new())),
    };
    if uuid.is_empty() {
        let _ = sink.close().await;
        anyhow::bail!("registration rejected");
    }
    if let Registrant::Plugin(id) = &who {
        log::debug!(target: "plugin_wire", "{}", serde_json::json!({"plugin":id,"kind":"registered"}));
    }

    let (tx, mut rx) = unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::text(text)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let _ = events.send(ServerEvent::Connected { uuid: uuid.clone(), who: who.clone(), tx });

    while let Some(frame) = source.next().await {
        match frame {
            Ok(Message::Text(text)) => {
                let msg = parse_inbound(text.as_str());
                if log::log_enabled!(target: "plugin_wire", log::Level::Debug) {
                    log::debug!(target: "plugin_wire", "{}", wire_diagnostic(&who, text.as_str(), &msg));
                }
                match msg {
                    Inbound::Invalid => log::debug!("invalid plugin message ignored"),
                    Inbound::Unknown => log::debug!("unsupported plugin event ignored"),
                    msg => {
                        let _ = events.send(ServerEvent::Message { uuid: uuid.clone(), who: who.clone(), msg });
                    }
                }
            },
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    writer.abort();
    let _ = events.send(ServerEvent::Disconnected { uuid, who });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::sync::mpsc::unbounded_channel;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;

    #[test]
    fn diagnostics_include_only_event_name_and_plugin_identity() {
        let text = r#"{"event":"customEvent","uuid":"secret-token","payload":{"password":"secret"}}"#;
        let value = wire_diagnostic(&Registrant::Plugin("test".into()), text, &Inbound::Unknown);
        assert_eq!(value, serde_json::json!({"plugin":"test","kind":"inbound","event":"customEvent","classification":"unknown"}));
        assert!(!value.to_string().contains("secret"));
    }

    async fn setup(timeout_ms: u64) -> (Server, Arc<Tokens>, tokio::sync::mpsc::UnboundedReceiver<ServerEvent>) {
        let tokens = Arc::new(Tokens::default());
        let (tx, rx) = unbounded_channel();
        let server = start(tokens.clone(), tx, ServerOptions { register_timeout: Duration::from_millis(timeout_ms) }).await.unwrap();
        (server, tokens, rx)
    }

    async fn connect(port: u16, origin: Option<&str>) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, ()> {
        let mut req = format!("ws://127.0.0.1:{port}").into_client_request().unwrap();
        if let Some(o) = origin {
            req.headers_mut().insert("Origin", o.parse().unwrap());
        }
        tokio_tungstenite::connect_async(req).await.map(|(ws, _)| ws).map_err(|_| ())
    }

    async fn next_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<ServerEvent>) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.expect("event timeout").expect("channel closed")
    }

    async fn closed(ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>) -> bool {
        match tokio::time::timeout(Duration::from_secs(2), ws.next()).await {
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => true,
            _ => false,
        }
    }

    #[tokio::test]
    async fn registered_plugin_exchanges_messages() {
        let (server, tokens, mut rx) = setup(2000).await;
        tokens.insert("tok1".into(), Registrant::Plugin("com.x".into()));
        let mut ws = connect(server.port, None).await.unwrap();
        ws.send(Message::text(r#"{"event":"registerPlugin","uuid":"tok1"}"#)).await.unwrap();
        let ServerEvent::Connected { who, tx, .. } = next_event(&mut rx).await else { panic!("expected Connected") };
        assert_eq!(who, Registrant::Plugin("com.x".into()));
        ws.send(Message::text(r#"{"event":"showOk","context":"c1"}"#)).await.unwrap();
        match next_event(&mut rx).await {
            ServerEvent::Message { msg, .. } => assert_eq!(msg, Inbound::ShowOk { context: "c1".into() }),
            other => panic!("unexpected {other:?}"),
        }
        tx.send(r#"{"event":"keyDown"}"#.into()).unwrap();
        let got = tokio::time::timeout(Duration::from_secs(2), ws.next()).await.unwrap().unwrap().unwrap();
        assert_eq!(got.into_text().unwrap().as_str(), r#"{"event":"keyDown"}"#);
        ws.close(None).await.unwrap();
        assert!(matches!(next_event(&mut rx).await, ServerEvent::Disconnected { .. }));
    }

    #[tokio::test]
    async fn unknown_uuid_is_closed() {
        let (server, _tokens, mut rx) = setup(2000).await;
        let mut ws = connect(server.port, None).await.unwrap();
        ws.send(Message::text(r#"{"event":"registerPlugin","uuid":"nope"}"#)).await.unwrap();
        assert!(closed(&mut ws).await);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn pi_token_cannot_register_as_plugin() {
        let (server, tokens, _rx) = setup(2000).await;
        tokens.insert("pi1".into(), Registrant::Pi { plugin: "com.x".into(), context: "c".into() });
        let mut ws = connect(server.port, None).await.unwrap();
        ws.send(Message::text(r#"{"event":"registerPlugin","uuid":"pi1"}"#)).await.unwrap();
        assert!(closed(&mut ws).await);
    }

    #[tokio::test]
    async fn silent_client_is_closed_after_timeout() {
        let (server, _tokens, _rx) = setup(200).await;
        let mut ws = connect(server.port, None).await.unwrap();
        assert!(closed(&mut ws).await);
    }

    #[tokio::test]
    async fn foreign_origin_is_rejected_and_own_origins_accepted() {
        let (server, _tokens, _rx) = setup(2000).await;
        assert!(connect(server.port, Some("https://evil.example")).await.is_err());
        assert!(connect(server.port, Some("http://tauri.localhost")).await.is_ok());
        assert!(connect(server.port, Some("http://gdplugin.localhost")).await.is_ok());
        assert!(connect(server.port, None).await.is_ok());
    }

    #[tokio::test]
    async fn unknown_events_do_not_disconnect() {
        let (server, tokens, mut rx) = setup(2000).await;
        tokens.insert("tok2".into(), Registrant::Plugin("com.y".into()));
        let mut ws = connect(server.port, None).await.unwrap();
        ws.send(Message::text(r#"{"event":"registerPlugin","uuid":"tok2"}"#)).await.unwrap();
        next_event(&mut rx).await;
        ws.send(Message::text(r#"{"event":"setFeedbackLayout","context":"c"}"#)).await.unwrap();
        ws.send(Message::text("garbage")).await.unwrap();
        ws.send(Message::text(r#"{"event":"showAlert","context":"c"}"#)).await.unwrap();
        match next_event(&mut rx).await {
            ServerEvent::Message { msg, .. } => assert_eq!(msg, Inbound::ShowAlert { context: "c".into() }),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn server_binds_loopback_only() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (server, _t, _rx) = setup(2000).await;
            assert!(server.addr.ip().is_loopback());
        });
    }
}

//! LocalSend protocol v2, natively: files and text between Nook and any
//! device running the open-source LocalSend app (Android, iOS, desktop), on
//! the local network only, with no cloud and no account.
//!
//! - **Discovery:** JSON announcements on UDP multicast `224.0.0.167:53317`.
//!   A device that hears an announcement registers back over HTTP(S), or
//!   answers on multicast when HTTP fails. A manual deep scan also tries
//!   `POST /register` on every address of the local /24 (multicast is often
//!   filtered on Wi-Fi).
//! - **Receiving:** a small HTTP server on TCP 53317 (`/register`,
//!   `/prepare-upload`, `/upload`, `/cancel`, `/info`). Nook advertises
//!   `"protocol": "http"`, so peers talk plain HTTP to it.
//! - **Sending:** phones serve HTTPS with a self-signed certificate (the
//!   protocol identifies devices by fingerprint, not by a CA chain), so the
//!   client accepts any certificate, as the official app does.
//!
//! All of it runs on plain threads; the UI only reads shared state and
//! drains events, and every thread wakes it through `request_repaint`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufWriter, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const PORT: u16 = 53317;
const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 167);
const API: &str = "/api/localsend/v2";
const VERSION: &str = "2.1";
/// How long a peer may take to answer a transfer request before it is
/// treated as declined.
const DECIDE_TIMEOUT: Duration = Duration::from_secs(120);

/// Device info as it travels in announcements, `/register` and
/// `prepare-upload`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub alias: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub device_model: Option<String>,
    #[serde(default)]
    pub device_type: Option<String>,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default)]
    pub download: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announce: Option<bool>,
}

/// A device found on the network.
#[derive(Clone, Debug)]
pub struct Peer {
    pub alias: String,
    pub model: Option<String>,
    pub kind: Option<String>,
    pub fingerprint: String,
    pub ip: IpAddr,
    pub port: u16,
    pub https: bool,
}

impl Peer {
    fn from_info(info: &Info, ip: IpAddr) -> Self {
        Self {
            alias: info.alias.clone(),
            model: info.device_model.clone(),
            kind: info.device_type.clone(),
            fingerprint: info.fingerprint.clone(),
            ip,
            port: info.port.unwrap_or(PORT),
            https: info.protocol.as_deref() != Some("http"),
        }
    }

    fn url(&self, path: &str) -> String {
        let scheme = if self.https { "https" } else { "http" };
        format!("{scheme}://{}:{}{API}{path}", self.ip, self.port)
    }

    pub fn is_mobile(&self) -> bool {
        self.kind.as_deref() == Some("mobile")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Send,
    Receive,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Waiting for the other side (or the user) to accept.
    Waiting,
    /// The receiver asks for a PIN.
    NeedPin,
    Running,
    Done,
    Declined,
    Cancelled,
    Failed(String),
}

impl State {
    pub fn finished(&self) -> bool {
        matches!(
            self,
            State::Done | State::Declined | State::Cancelled | State::Failed(_)
        )
    }
}

/// One transfer (a batch of files) in either direction. Shared between the
/// worker threads that move the bytes and the UI that draws the progress.
#[derive(Debug)]
pub struct Transfer {
    pub direction: Direction,
    pub peer: String,
    /// File names and sizes.
    pub files: Vec<(String, u64)>,
    pub total: u64,
    pub done: AtomicU64,
    pub state: Mutex<State>,
    pub cancel: AtomicBool,
    /// Where received files were written.
    pub saved: Mutex<Vec<PathBuf>>,
    pub started: Instant,
}

impl Transfer {
    fn new(direction: Direction, peer: &str, files: Vec<(String, u64)>) -> Arc<Self> {
        Arc::new(Self {
            direction,
            peer: peer.to_owned(),
            total: files.iter().map(|f| f.1).sum(),
            files,
            done: AtomicU64::new(0),
            state: Mutex::new(State::Waiting),
            cancel: AtomicBool::new(false),
            saved: Mutex::new(Vec::new()),
            started: Instant::now(),
        })
    }

    pub fn state(&self) -> State {
        self.state.lock().map_or(State::Waiting, |s| s.clone())
    }

    fn set(&self, s: State) {
        if let Ok(mut st) = self.state.lock() {
            // A cancel or failure is final; late progress does not undo it.
            if !st.finished() {
                *st = s;
            }
        }
    }

    pub fn progress(&self) -> f32 {
        if self.total == 0 {
            return if self.state() == State::Done {
                1.0
            } else {
                0.0
            };
        }
        (self.done.load(Ordering::Relaxed) as f64 / self.total as f64).clamp(0.0, 1.0) as f32
    }

    /// Bytes per second since the transfer started.
    pub fn speed(&self) -> f64 {
        let secs = self.started.elapsed().as_secs_f64().max(0.1);
        self.done.load(Ordering::Relaxed) as f64 / secs
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Something the UI should hear about.
pub enum Event {
    /// A device appeared (or announced itself again).
    Peer(Peer),
    /// A device wants to send files. Answer through `reply`.
    Incoming {
        transfer: Arc<Transfer>,
        reply: Sender<bool>,
    },
    /// A text message (LocalSend's "text" share).
    Message { from: String, text: String },
    /// A transfer was created (sending or auto-accepted receiving).
    Transfer(Arc<Transfer>),
    /// The send needs a PIN; retry with `send_files(.., Some(pin))`.
    NeedPin { peer: Peer, paths: Vec<PathBuf> },
}

struct FileSlot {
    token: String,
    name: String,
    size: u64,
    /// Bytes this file has added to the transfer's counter.
    done: bool,
}

struct Session {
    id: String,
    peer_ip: IpAddr,
    files: HashMap<String, FileSlot>,
    transfer: Arc<Transfer>,
}

struct Shared {
    me: Info,
    events: Mutex<Sender<Event>>,
    ctx: egui::Context,
    session: Mutex<Option<Session>>,
    save_dir: Mutex<PathBuf>,
    auto_accept: AtomicBool,
    agent: ureq::Agent,
    /// Short timeouts, for the deep scan.
    probe: ureq::Agent,
    /// Port the HTTP server actually listens on.
    port: u16,
}

impl Shared {
    fn emit(&self, e: Event) {
        if let Ok(tx) = self.events.lock()
            && tx.send(e).is_ok()
        {
            self.ctx.request_repaint();
        }
    }

    /// Our info as announced: with port and protocol.
    fn announcement(&self, announce: Option<bool>) -> Info {
        Info {
            port: Some(self.port),
            protocol: Some("http".into()),
            announce,
            ..self.me.clone()
        }
    }

    fn save_dir(&self) -> PathBuf {
        self.save_dir
            .lock()
            .map(|d| d.clone())
            .unwrap_or_else(|_| PathBuf::from("."))
    }
}

/// The running LocalSend service. Dropping it does not stop the threads
/// (they live for the whole process, like the app).
pub struct LocalSend {
    shared: Arc<Shared>,
    pub events: Receiver<Event>,
    /// The HTTP server could not start (port taken, firewall).
    pub server_error: Option<String>,
}

impl LocalSend {
    pub fn start(
        ctx: egui::Context,
        alias: String,
        fingerprint: String,
        save_dir: PathBuf,
        auto_accept: bool,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let me = Info {
            alias,
            version: Some(VERSION.into()),
            device_model: Some("Windows".into()),
            device_type: Some("desktop".into()),
            fingerprint,
            port: None,
            protocol: None,
            download: false,
            announce: None,
        };
        // The official port, else any free one (another LocalSend running).
        let server = tiny_http::Server::http(("0.0.0.0", PORT))
            .or_else(|_| tiny_http::Server::http(("0.0.0.0", 0)));
        let (server, port, server_error) = match server {
            Ok(s) => {
                let port = s.server_addr().to_ip().map_or(PORT, |a| a.port());
                (Some(s), port, None)
            }
            Err(e) => {
                tracing::warn!("localsend: HTTP server: {e}");
                (None, PORT, Some(e.to_string()))
            }
        };
        let shared = Arc::new(Shared {
            me,
            events: Mutex::new(tx),
            ctx,
            session: Mutex::new(None),
            save_dir: Mutex::new(save_dir),
            auto_accept: AtomicBool::new(auto_accept),
            agent: agent(Duration::from_secs(4)),
            probe: agent(Duration::from_millis(600)),
            port,
        });
        if let Some(server) = server {
            let s = shared.clone();
            spawn("nook-ls-http", move || serve(server, s));
        }
        let s = shared.clone();
        spawn("nook-ls-udp", move || listen(s));
        let this = Self {
            shared,
            events: rx,
            server_error,
        };
        this.announce();
        this
    }

    pub fn port(&self) -> u16 {
        self.shared.port
    }

    pub fn alias(&self) -> &str {
        &self.shared.me.alias
    }

    pub fn set_save_dir(&self, dir: PathBuf) {
        if let Ok(mut d) = self.shared.save_dir.lock() {
            *d = dir;
        }
    }

    pub fn set_auto_accept(&self, on: bool) {
        self.shared.auto_accept.store(on, Ordering::Relaxed);
    }

    /// Multicast "I'm here" on every interface, a few times (UDP is lossy).
    pub fn announce(&self) {
        let s = self.shared.clone();
        spawn("nook-ls-announce", move || {
            for delay in [0, 400, 1600] {
                std::thread::sleep(Duration::from_millis(delay));
                let msg = match serde_json::to_vec(&s.announcement(Some(true))) {
                    Ok(m) => m,
                    Err(_) => return,
                };
                multicast(&msg);
            }
        });
    }

    /// Asks every address of the local /24 networks directly (for networks
    /// that drop multicast).
    pub fn deep_scan(&self) {
        let s = self.shared.clone();
        spawn("nook-ls-scan", move || {
            let targets: Vec<Ipv4Addr> = interfaces()
                .into_iter()
                .flat_map(|(ip, _)| {
                    let [a, b, c, own] = ip.octets();
                    (1..=254u8)
                        .filter(move |&d| d != own)
                        .map(move |d| Ipv4Addr::new(a, b, c, d))
                })
                .collect();
            let queue = Arc::new(Mutex::new(targets));
            let workers: Vec<_> = (0..48)
                .filter_map(|_| {
                    let queue = queue.clone();
                    let s = s.clone();
                    std::thread::Builder::new()
                        .name("nook-ls-probe".into())
                        .spawn(move || {
                            while let Some(ip) = queue.lock().ok().and_then(|mut q| q.pop()) {
                                for https in [true, false] {
                                    if let Some(peer) =
                                        register_with(&s, &s.probe, IpAddr::V4(ip), PORT, https)
                                    {
                                        s.emit(Event::Peer(peer));
                                        break;
                                    }
                                }
                            }
                        })
                        .ok()
                })
                .collect();
            for w in workers {
                let _ = w.join();
            }
        });
    }

    /// Sends files to `peer` on a worker thread. The transfer arrives as an
    /// [`Event::Transfer`] right away.
    pub fn send_files(&self, peer: Peer, paths: Vec<PathBuf>, pin: Option<String>) {
        let s = self.shared.clone();
        spawn("nook-ls-send", move || send_files(&s, peer, paths, pin));
    }

    /// Sends a text message (shown on the other side, nothing is uploaded).
    pub fn send_text(&self, peer: Peer, text: String) {
        let s = self.shared.clone();
        spawn("nook-ls-text", move || {
            let id = random_hex(8);
            let body = serde_json::json!({
                "info": s.me,
                "files": {
                    id.clone(): {
                        "id": id,
                        "fileName": format!("{id}.txt"),
                        "size": text.len(),
                        "fileType": "text/plain",
                        "sha256": null,
                        "preview": text,
                    }
                }
            });
            match s.agent.post(&peer.url("/prepare-upload")).send_json(body) {
                Ok(_) => tracing::info!("localsend: text sent to {}", peer.alias),
                Err(e) => tracing::warn!("localsend: text to {}: {e}", peer.alias),
            }
        });
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        tracing::warn!("spawning {name}: {e}");
    }
}

/// Phones identify themselves by certificate fingerprint, not a CA chain:
/// accept any certificate (but still check the handshake signatures).
#[derive(Debug)]
struct AnyCert(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AnyCert {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn agent(connect: Duration) -> ureq::Agent {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ureq::AgentBuilder::new()
        .timeout_connect(connect)
        .timeout_read(Duration::from_secs(60))
        .timeout_write(Duration::from_secs(60));
    let tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map(|b| {
            b.dangerous()
                .with_custom_certificate_verifier(Arc::new(AnyCert(provider)))
                .with_no_client_auth()
        });
    match tls {
        Ok(tls) => builder.tls_config(Arc::new(tls)).build(),
        Err(e) => {
            tracing::warn!("localsend: TLS setup: {e}");
            builder.build()
        }
    }
}

/// IPv4 addresses (and their interface index) that can reach the LAN.
fn interfaces() -> Vec<(Ipv4Addr, Option<u32>)> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.is_oper_up() && !i.is_loopback() && !i.is_link_local())
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(a) => Some((a.ip, i.index)),
            if_addrs::IfAddr::V6(_) => None,
        })
        .collect()
}

fn multicast(msg: &[u8]) {
    use socket2::{Domain, Protocol, Socket, Type};
    let to: SocketAddr = SocketAddrV4::new(GROUP, PORT).into();
    for (ip, _) in interfaces() {
        let sock = match Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let _ = sock.set_multicast_if_v4(&ip);
        let _ = sock.set_multicast_ttl_v4(4);
        if let Err(e) = sock.send_to(msg, &to.into()) {
            tracing::debug!("localsend: multicast on {ip}: {e}");
        }
    }
}

/// Listens for announcements and answers them.
fn listen(s: Arc<Shared>) {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = (|| -> std::io::Result<UdpSocket> {
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        // Share the port with a LocalSend app running on this PC.
        sock.set_reuse_address(true)?;
        sock.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT)).into())?;
        Ok(sock.into())
    })();
    let sock = match sock {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("localsend: multicast listener: {e}");
            return;
        }
    };
    for (ip, _) in interfaces() {
        if let Err(e) = sock.join_multicast_v4(&GROUP, &ip) {
            tracing::debug!("localsend: join multicast on {ip}: {e}");
        }
    }
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("localsend: recv: {e}");
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        let Ok(info) = serde_json::from_slice::<Info>(&buf[..n]) else {
            continue;
        };
        if info.fingerprint == s.me.fingerprint {
            continue;
        }
        let peer = Peer::from_info(&info, from.ip());
        s.emit(Event::Peer(peer.clone()));
        if info.announce == Some(true) {
            let s = s.clone();
            spawn("nook-ls-answer", move || {
                if register_with(&s, &s.agent, peer.ip, peer.port, peer.https).is_none()
                    && let Ok(msg) = serde_json::to_vec(&s.announcement(Some(false)))
                {
                    multicast(&msg);
                }
            });
        }
    }
}

/// `POST /register` to a device; returns it if it answered.
fn register_with(
    s: &Shared,
    agent: &ureq::Agent,
    ip: IpAddr,
    port: u16,
    https: bool,
) -> Option<Peer> {
    let scheme = if https { "https" } else { "http" };
    let url = format!("{scheme}://{ip}:{port}{API}/register");
    let resp = agent.post(&url).send_json(s.announcement(None)).ok()?;
    let info: Info = resp.into_json().ok()?;
    if info.fingerprint == s.me.fingerprint {
        return None;
    }
    let mut peer = Peer::from_info(&info, ip);
    peer.port = port;
    peer.https = https;
    Some(peer)
}

fn json_response(value: &impl Serialize) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]);
    let resp = tiny_http::Response::from_data(body);
    match header {
        Ok(h) => resp.with_header(h),
        Err(()) => resp,
    }
}

fn status(code: u16) -> tiny_http::Response<std::io::Empty> {
    tiny_http::Response::empty(code)
}

fn query(url: &str) -> HashMap<String, String> {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), percent_decode(v)))
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn serve(server: tiny_http::Server, s: Arc<Shared>) {
    for req in server.incoming_requests() {
        let s = s.clone();
        spawn("nook-ls-req", move || handle(req, &s));
    }
}

fn handle(mut req: tiny_http::Request, s: &Shared) {
    let path = req.url().split('?').next().unwrap_or("").to_owned();
    let ip = req.remote_addr().map(|a| a.ip());
    let method = req.method().clone();
    let result = match (method, path.strip_prefix(API).unwrap_or(&path)) {
        (tiny_http::Method::Get, "/info") => req.respond(json_response(&s.me)),
        (tiny_http::Method::Post, "/register") => {
            if let (Some(info), Some(ip)) = (read_json::<Info>(&mut req), ip)
                && info.fingerprint != s.me.fingerprint
            {
                s.emit(Event::Peer(Peer::from_info(&info, ip)));
            }
            req.respond(json_response(&s.me))
        }
        (tiny_http::Method::Post, "/prepare-upload") => {
            let code_or_body = prepare_upload(&mut req, s, ip);
            match code_or_body {
                Ok(body) => req.respond(json_response(&body)),
                Err(code) => req.respond(status(code)),
            }
        }
        (tiny_http::Method::Post, "/upload") => {
            let code = upload(&mut req, s, ip);
            req.respond(status(code))
        }
        (tiny_http::Method::Post, "/cancel") => {
            let q = query(req.url());
            if let Ok(mut sess) = s.session.lock()
                && sess
                    .as_ref()
                    .is_some_and(|x| Some(&x.id) == q.get("sessionId"))
                && let Some(x) = sess.take()
            {
                x.transfer.cancel();
                x.transfer.set(State::Cancelled);
                s.ctx.request_repaint();
            }
            req.respond(status(200))
        }
        _ => req.respond(status(404)),
    };
    if let Err(e) = result {
        tracing::debug!("localsend: responding: {e}");
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(req: &mut tiny_http::Request) -> Option<T> {
    let mut body = Vec::new();
    req.as_reader()
        .take(16 * 1024 * 1024)
        .read_to_end(&mut body)
        .ok()?;
    serde_json::from_slice(&body).ok()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrepareFile {
    id: String,
    file_name: String,
    size: u64,
    #[serde(default)]
    file_type: String,
    #[serde(default)]
    preview: Option<String>,
}

#[derive(Deserialize)]
struct Prepare {
    info: Info,
    files: HashMap<String, PrepareFile>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PrepareReply {
    session_id: String,
    files: HashMap<String, String>,
}

fn prepare_upload(
    req: &mut tiny_http::Request,
    s: &Shared,
    ip: Option<IpAddr>,
) -> Result<PrepareReply, u16> {
    let ip = ip.ok_or(400u16)?;
    let p: Prepare = read_json(req).ok_or(400u16)?;
    if p.files.is_empty() {
        return Err(400);
    }
    s.emit(Event::Peer(Peer::from_info(&p.info, ip)));
    // Text shares carry their content in `preview`: show it, nothing to upload.
    if p.files
        .values()
        .all(|f| f.file_type.starts_with("text/") && f.preview.is_some())
    {
        for f in p.files.values() {
            s.emit(Event::Message {
                from: p.info.alias.clone(),
                text: f.preview.clone().unwrap_or_default(),
            });
        }
        return Err(204);
    }
    let busy = s
        .session
        .lock()
        .map_err(|_| 500u16)?
        .as_ref()
        .is_some_and(|x| !x.transfer.state().finished());
    if busy {
        return Err(409);
    }
    let mut list: Vec<&PrepareFile> = p.files.values().collect();
    list.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    let transfer = Transfer::new(
        Direction::Receive,
        &p.info.alias,
        list.iter().map(|f| (f.file_name.clone(), f.size)).collect(),
    );
    let accepted = if s.auto_accept.load(Ordering::Relaxed) {
        s.emit(Event::Transfer(transfer.clone()));
        true
    } else {
        let (tx, rx) = mpsc::channel();
        s.emit(Event::Incoming {
            transfer: transfer.clone(),
            reply: tx,
        });
        rx.recv_timeout(DECIDE_TIMEOUT).unwrap_or(false)
    };
    if !accepted {
        transfer.set(State::Declined);
        return Err(403);
    }
    transfer.set(State::Running);
    let session = Session {
        id: random_hex(16),
        peer_ip: ip,
        files: p
            .files
            .values()
            .map(|f| {
                (
                    f.id.clone(),
                    FileSlot {
                        token: random_hex(16),
                        name: f.file_name.clone(),
                        size: f.size,
                        done: false,
                    },
                )
            })
            .collect(),
        transfer,
    };
    let reply = PrepareReply {
        session_id: session.id.clone(),
        files: session
            .files
            .iter()
            .map(|(id, f)| (id.clone(), f.token.clone()))
            .collect(),
    };
    *s.session.lock().map_err(|_| 500u16)? = Some(session);
    Ok(reply)
}

/// A file name from a peer made safe: no drive, no `..`, no reserved
/// characters; sub-folders (folder shares) are kept.
fn safe_relative(name: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        let clean: String = part
            .chars()
            .map(|c| {
                if "<>:\"|?*".contains(c) || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let clean = clean.trim().trim_end_matches('.');
        if clean.is_empty() || clean == "." || clean == ".." {
            continue;
        }
        out.push(clean);
    }
    if out.as_os_str().is_empty() {
        out.push("file");
    }
    out
}

/// `dir/name`, or `name (2)` and so on when it exists.
pub fn unique_path(dir: &Path, rel: &Path) -> PathBuf {
    let first = dir.join(rel);
    if !first.exists() {
        return first;
    }
    let stem = first
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = first
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let parent = first
        .parent()
        .map_or_else(|| dir.to_path_buf(), Path::to_path_buf);
    (2..)
        .map(|n| parent.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

fn upload(req: &mut tiny_http::Request, s: &Shared, ip: Option<IpAddr>) -> u16 {
    let q = query(req.url());
    let (Some(sid), Some(fid), Some(token)) = (q.get("sessionId"), q.get("fileId"), q.get("token"))
    else {
        return 400;
    };
    let (name, size, transfer) = {
        let Ok(sess) = s.session.lock() else {
            return 500;
        };
        let Some(sess) = sess.as_ref().filter(|x| &x.id == sid) else {
            return 403;
        };
        if Some(sess.peer_ip) != ip {
            return 403;
        }
        let Some(slot) = sess.files.get(fid).filter(|f| &f.token == token && !f.done) else {
            return 403;
        };
        (slot.name.clone(), slot.size, sess.transfer.clone())
    };
    if transfer.cancel.load(Ordering::Relaxed) {
        return 409;
    }
    let dir = s.save_dir();
    let target = unique_path(&dir, &safe_relative(&name));
    let part = target.with_extension(format!(
        "{}nookpart",
        target
            .extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    let written = (|| -> std::io::Result<u64> {
        if let Some(p) = part.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut out = BufWriter::with_capacity(1 << 20, std::fs::File::create(&part)?);
        let mut buf = vec![0u8; 256 * 1024];
        let mut total = 0u64;
        let mut last_paint = Instant::now();
        let reader = req.as_reader();
        loop {
            if transfer.cancel.load(Ordering::Relaxed) {
                return Err(std::io::Error::other("cancelled"));
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            total += n as u64;
            transfer.done.fetch_add(n as u64, Ordering::Relaxed);
            if last_paint.elapsed() > Duration::from_millis(100) {
                last_paint = Instant::now();
                s.ctx.request_repaint();
            }
        }
        out.flush()?;
        Ok(total)
    })();
    let ok = match written {
        Ok(n) if n == size => std::fs::rename(&part, &target).is_ok(),
        Ok(n) => {
            tracing::warn!("localsend: {name}: got {n} of {size} bytes");
            false
        }
        Err(e) => {
            tracing::warn!("localsend: receiving {name}: {e}");
            false
        }
    };
    if !ok {
        let _ = std::fs::remove_file(&part);
        if !transfer.cancel.load(Ordering::Relaxed) {
            transfer.set(State::Failed(format!("{name} did not arrive complete")));
        }
        s.ctx.request_repaint();
        return 500;
    }
    if let Ok(mut saved) = transfer.saved.lock() {
        saved.push(target);
    }
    if let Ok(mut sess) = s.session.lock()
        && let Some(x) = sess.as_mut().filter(|x| &x.id == sid)
    {
        if let Some(f) = x.files.get_mut(fid) {
            f.done = true;
        }
        if x.files.values().all(|f| f.done) {
            x.transfer.set(State::Done);
            *sess = None;
        }
    }
    s.ctx.request_repaint();
    200
}

/// Counts the bytes of a file as they are read for an upload.
struct Counted<'a> {
    inner: std::fs::File,
    transfer: &'a Transfer,
    ctx: &'a egui::Context,
    last_paint: Instant,
}

impl Read for Counted<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.transfer.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("cancelled"));
        }
        let n = self.inner.read(buf)?;
        self.transfer.done.fetch_add(n as u64, Ordering::Relaxed);
        if self.last_paint.elapsed() > Duration::from_millis(100) {
            self.last_paint = Instant::now();
            self.ctx.request_repaint();
        }
        Ok(n)
    }
}

fn send_files(s: &Shared, peer: Peer, paths: Vec<PathBuf>, pin: Option<String>) {
    // Folders are sent as their files, keeping the folder structure.
    let mut files: Vec<(PathBuf, String, u64)> = Vec::new();
    for p in &paths {
        collect(p, p.parent().unwrap_or(p), &mut files);
    }
    if files.is_empty() {
        return;
    }
    let transfer = Transfer::new(
        Direction::Send,
        &peer.alias,
        files.iter().map(|(_, n, sz)| (n.clone(), *sz)).collect(),
    );
    s.emit(Event::Transfer(transfer.clone()));
    let ids: Vec<String> = files.iter().map(|_| random_hex(8)).collect();
    let mut meta = serde_json::Map::new();
    for ((path, name, size), id) in files.iter().zip(&ids) {
        meta.insert(
            id.clone(),
            serde_json::json!({
                "id": id,
                "fileName": name,
                "size": size,
                "fileType": mime(path),
                "sha256": null,
                "preview": null,
            }),
        );
    }
    let body = serde_json::json!({ "info": s.announcement(None), "files": meta });
    let mut url = peer.url("/prepare-upload");
    if let Some(pin) = &pin {
        url.push_str(&format!("?pin={pin}"));
    }
    // The receiver may wait for its user to accept: no read timeout here.
    let resp = s
        .agent
        .post(&url)
        .timeout(DECIDE_TIMEOUT + Duration::from_secs(10))
        .send_json(body);
    let reply: PrepareReply = match resp {
        Ok(r) if r.status() == 204 => {
            transfer.done.store(transfer.total, Ordering::Relaxed);
            transfer.set(State::Done);
            s.ctx.request_repaint();
            return;
        }
        Ok(r) => match r.into_json::<serde_json::Value>() {
            Ok(v) => match parse_reply(&v) {
                Some(r) => r,
                None => {
                    transfer.set(State::Failed("unexpected answer".into()));
                    s.ctx.request_repaint();
                    return;
                }
            },
            Err(e) => {
                transfer.set(State::Failed(e.to_string()));
                s.ctx.request_repaint();
                return;
            }
        },
        Err(ureq::Error::Status(401, _)) => {
            transfer.set(State::NeedPin);
            s.emit(Event::NeedPin { peer, paths });
            return;
        }
        Err(ureq::Error::Status(403, _)) => {
            transfer.set(State::Declined);
            s.ctx.request_repaint();
            return;
        }
        Err(ureq::Error::Status(409, _)) => {
            transfer.set(State::Failed(
                "the device is busy with another transfer".into(),
            ));
            s.ctx.request_repaint();
            return;
        }
        Err(e) => {
            transfer.set(State::Failed(short_error(&e)));
            s.ctx.request_repaint();
            return;
        }
    };
    if transfer.cancel.load(Ordering::Relaxed) {
        cancel_remote(s, &peer, &reply.session_id);
        transfer.set(State::Cancelled);
        return;
    }
    transfer.set(State::Running);
    s.ctx.request_repaint();
    for ((path, name, size), id) in files.iter().zip(&ids) {
        // The receiver may skip files it already has: no token, no upload.
        let Some(token) = reply.files.get(id) else {
            transfer.done.fetch_add(*size, Ordering::Relaxed);
            continue;
        };
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                transfer.set(State::Failed(format!("{name}: {e}")));
                cancel_remote(s, &peer, &reply.session_id);
                s.ctx.request_repaint();
                return;
            }
        };
        let url = format!(
            "{}?sessionId={}&fileId={}&token={}",
            peer.url("/upload"),
            reply.session_id,
            id,
            token
        );
        let body = Counted {
            inner: file,
            transfer: &transfer,
            ctx: &s.ctx,
            last_paint: Instant::now(),
        };
        let sent = s
            .agent
            .post(&url)
            .set("Content-Type", "application/octet-stream")
            .set("Content-Length", &size.to_string())
            .send(body);
        if let Err(e) = sent {
            if transfer.cancel.load(Ordering::Relaxed) {
                transfer.set(State::Cancelled);
            } else {
                transfer.set(State::Failed(format!("{name}: {}", short_error(&e))));
            }
            cancel_remote(s, &peer, &reply.session_id);
            s.ctx.request_repaint();
            return;
        }
    }
    transfer.set(State::Done);
    s.ctx.request_repaint();
}

fn parse_reply(v: &serde_json::Value) -> Option<PrepareReply> {
    let session_id = v.get("sessionId")?.as_str()?.to_owned();
    let files = v
        .get("files")?
        .as_object()?
        .iter()
        .filter_map(|(k, t)| Some((k.clone(), t.as_str()?.to_owned())))
        .collect();
    Some(PrepareReply { session_id, files })
}

fn cancel_remote(s: &Shared, peer: &Peer, session: &str) {
    let url = format!("{}?sessionId={session}", peer.url("/cancel"));
    let _ = s.agent.post(&url).call();
}

fn short_error(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, _) => format!("refused ({code})"),
        ureq::Error::Transport(t) => match t.kind() {
            ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::Dns => {
                "device not reachable".into()
            }
            _ => t
                .message()
                .map_or_else(|| t.kind().to_string(), str::to_owned),
        },
    }
}

/// Files under `path` (or `path` itself), named relative to `base`.
fn collect(path: &Path, base: &Path, out: &mut Vec<(PathBuf, String, u64)>) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            collect(&e, base, out);
        }
    } else {
        let name = path
            .strip_prefix(base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        out.push((path.to_path_buf(), name, meta.len()));
    }
}

fn mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "apk" => "application/vnd.android.package-archive",
        "txt" | "md" | "log" => "text/plain",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        "ppt" | "pptx" => "application/vnd.ms-powerpoint",
        _ => "application/octet-stream",
    }
}

/// Random lowercase hex from the OS-seeded hasher (no extra crate needed for
/// tokens and ids that only have to be unguessable on a LAN).
pub fn random_hex(bytes: usize) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::with_capacity(bytes * 2);
    while out.len() < bytes * 2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.write_usize(out.len());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out.truncate(bytes * 2);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_file_names_stay_inside_the_folder() {
        assert_eq!(safe_relative("../../evil.exe"), PathBuf::from("evil.exe"));
        assert_eq!(
            safe_relative("C:\\Windows\\x.dll"),
            PathBuf::from("C_").join("Windows").join("x.dll")
        );
        assert_eq!(
            safe_relative("photos/a:b.jpg"),
            PathBuf::from("photos").join("a_b.jpg")
        );
        assert_eq!(safe_relative(".."), PathBuf::from("file"));
    }

    #[test]
    fn query_strings_decode() {
        let q = query("/upload?sessionId=abc&fileId=f%201&token=t+2");
        assert_eq!(q["sessionId"], "abc");
        assert_eq!(q["fileId"], "f 1");
        assert_eq!(q["token"], "t 2");
    }

    #[test]
    fn announcements_parse() {
        let json = r#"{"alias":"Pixel","version":"2.0","deviceModel":"Pixel 8","deviceType":"mobile","fingerprint":"abc","port":53317,"protocol":"https","download":false,"announce":true}"#;
        let info: Info = serde_json::from_str(json).expect("parses");
        let peer = Peer::from_info(&info, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5)));
        assert!(peer.https && peer.is_mobile());
        assert_eq!(
            peer.url("/register"),
            "https://192.168.1.5:53317/api/localsend/v2/register"
        );
    }

    /// Two instances on this machine: one sends a folder to the other, which
    /// auto-accepts, and the bytes and the folder structure arrive intact.
    #[test]
    #[ignore = "opens network ports; run with --ignored"]
    fn sends_files_end_to_end() {
        let root = std::env::temp_dir().join(format!("nook-ls-test-{}", std::process::id()));
        let inbox = root.join("inbox");
        let src = root.join("src").join("album");
        std::fs::create_dir_all(&inbox).expect("inbox");
        std::fs::create_dir_all(&src).expect("src");
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(src.join("photo.bin"), &big).expect("write");
        std::fs::write(src.join("note.txt"), b"hello phone").expect("write");

        let ctx = egui::Context::default();
        let rx_side = LocalSend::start(
            ctx.clone(),
            "Receiver".into(),
            random_hex(16),
            inbox.clone(),
            true,
        );
        let tx_side = LocalSend::start(ctx, "Sender".into(), random_hex(16), root.clone(), false);
        let peer = Peer {
            alias: "Receiver".into(),
            model: None,
            kind: Some("desktop".into()),
            fingerprint: rx_side.shared.me.fingerprint.clone(),
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: rx_side.port(),
            https: false,
        };
        tx_side.send_files(peer, vec![src.clone()], None);
        let deadline = Instant::now() + Duration::from_secs(20);
        let sent = loop {
            if let Ok(Event::Transfer(t)) = tx_side.events.recv_timeout(Duration::from_secs(5))
                && t.direction == Direction::Send
            {
                break t;
            }
            assert!(Instant::now() < deadline, "no transfer started");
        };
        while !sent.state().finished() {
            assert!(
                Instant::now() < deadline,
                "transfer stuck: {:?}",
                sent.state()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(sent.state(), State::Done);
        assert_eq!(sent.done.load(Ordering::Relaxed), big.len() as u64 + 11);
        let got = std::fs::read(inbox.join("album").join("photo.bin")).expect("photo arrived");
        assert!(got == big, "photo bytes differ");
        let note = std::fs::read(inbox.join("album").join("note.txt")).expect("note arrived");
        assert_eq!(note, b"hello phone");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Against a LocalSend receiver serving HTTPS with a self-signed
    /// certificate, as phones do: `NOOK_LS_HTTPS_PORT=<port>` on 127.0.0.1.
    #[test]
    #[ignore = "needs an HTTPS LocalSend receiver; set NOOK_LS_HTTPS_PORT"]
    fn talks_to_https_receivers() {
        let Some(port) = std::env::var("NOOK_LS_HTTPS_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
        else {
            return;
        };
        let root = std::env::temp_dir().join(format!("nook-ls-tls-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("dir");
        let file = root.join("clip.bin");
        std::fs::write(&file, vec![42u8; 123_456]).expect("write");
        let ls = LocalSend::start(
            egui::Context::default(),
            "Nook test".into(),
            random_hex(16),
            root.clone(),
            false,
        );
        let peer = register_with(
            &ls.shared,
            &ls.shared.agent,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            true,
        )
        .expect("registered over HTTPS");
        assert_eq!(peer.alias, "FakePhone");
        assert!(peer.is_mobile() && peer.https);
        ls.send_text(peer.clone(), "hi from nook".into());
        ls.send_files(peer, vec![file], None);
        let t = loop {
            if let Ok(Event::Transfer(t)) = ls.events.recv_timeout(Duration::from_secs(10)) {
                break t;
            }
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        while !t.state().finished() {
            assert!(Instant::now() < deadline, "stuck: {:?}", t.state());
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(t.state(), State::Done);
        assert_eq!(t.done.load(Ordering::Relaxed), 123_456);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn random_hex_has_the_asked_length() {
        let a = random_hex(16);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, random_hex(16));
    }
}

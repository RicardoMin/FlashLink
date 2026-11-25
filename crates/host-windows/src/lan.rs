//! Nearby discovery is public. A device becomes trusted only through Sunshine's
//! pairing exchange after the local Windows user approves the matching request.
use if_addrs::IfAddr;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use sha2::{Digest, Sha256};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[path = "audio_wire.rs"]
pub mod audio_wire;
use audio_wire::{AudioSession, Report, Streams};

pub const DISCOVERY_PORT: u16 = 47980;
const LIFETIME: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(100);

pub struct Approval {
    pub pin: String,
    pub code: String,
    pub name: String,
}

struct Pending {
    peer: IpAddr,
    token: String,
    approval: Approval,
    created: Instant,
}

#[derive(Default)]
struct Inbox(Vec<Pending>);

impl Inbox {
    fn expire(&mut self) {
        self.0.retain(|p| p.created.elapsed() < LIFETIME);
    }

    fn store(&mut self, p: Pending) -> bool {
        self.expire();
        if self.0.len() >= 32
            || self.0.iter().any(|other| other.token == p.token)
            || self.0.iter().filter(|other| other.peer == p.peer).count() >= 3
        {
            return false;
        }
        self.0.push(p);
        true
    }

    fn take(&mut self, peer: IpAddr, pairing_name: &str) -> Option<Approval> {
        self.expire();
        let token = pairing_name.strip_prefix("FlashLink-")?;
        let index = self
            .0
            .iter()
            .position(|p| p.peer == peer && p.token == token)?;
        Some(self.0.swap_remove(index).approval)
    }
}

fn token_ok(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn display_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')).take(64).collect::<String>().trim().to_owned()
}

fn discover_nonce(query: &[u8]) -> Option<&str> {
    let query = std::str::from_utf8(query).ok()?.strip_suffix('\n')?;
    let nonce = query.strip_prefix("FLASHLINK_DISCOVER/1 ")?;
    token_ok(nonce).then_some(nonce)
}

fn parse_pair(line: &[u8], peer: IpAddr, host_code: &str) -> Option<Pending> {
    let line = std::str::from_utf8(line).ok()?.strip_suffix('\n')?;
    let mut fields = line.strip_prefix("FLASHLINK_PAIR/1 ")?.splitn(3, ' ');
    let token = fields.next()?;
    let pin = fields.next()?;
    let name = display_name(fields.next()?);
    if !token_ok(token)
        || pin.len() != 4
        || !pin.bytes().all(|b| b.is_ascii_digit())
        || name.is_empty()
    {
        return None;
    }
    Some(Pending {
        peer,
        token: token.into(),
        created: Instant::now(),
        approval: Approval {
            pin: pin.into(),
            code: format!("{host_code} {pin}"),
            name,
        },
    })
}

struct Subnets(Vec<(u32, u32)>);

impl Subnets {
    fn local() -> io::Result<Self> {
        let mut nets = vec![(u32::from(Ipv4Addr::LOCALHOST), 0xff000000)];
        for interface in if_addrs::get_if_addrs()? {
            if let IfAddr::V4(v4) = interface.addr {
                let mask = u32::from(v4.netmask);
                if mask != 0 {
                    nets.push((u32::from(v4.ip), mask));
                }
            }
        }
        Ok(Self(nets))
    }

    fn contains(&self, peer: IpAddr) -> bool {
        let IpAddr::V4(ip) = peer else { return false };
        !ip.is_unspecified()
            && !ip.is_multicast()
            && !ip.is_broadcast()
            && self
                .0
                .iter()
                .any(|(addr, mask)| u32::from(ip) & mask == addr & mask)
    }
}

struct DeadlineSocket(TcpStream, Instant);

impl DeadlineSocket {
    fn new(socket: TcpStream, lifetime: Duration) -> io::Result<Self> {
        // Some systems copy the listener's nonblocking flag to each accepted socket.
        socket.set_nonblocking(false)?;
        Ok(Self(socket, Instant::now() + lifetime))
    }

    fn remaining(&self) -> io::Result<Duration> {
        self.1
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "pairing request timed out"))
    }
}

impl Read for DeadlineSocket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.set_read_timeout(Some(self.remaining()?))?;
        self.0.read(buf)
    }
}

impl Write for DeadlineSocket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.set_write_timeout(Some(self.remaining()?))?;
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

fn receive(
    stream: TcpStream,
    tls: Arc<ServerConfig>,
    inbox: &Mutex<Inbox>,
    host_code: &str,
    streams: &Mutex<Streams>,
) -> io::Result<()> {
    let peer = stream.peer_addr()?.ip();
    let socket = DeadlineSocket::new(stream, Duration::from_secs(5))?;
    let conn = ServerConnection::new(tls).map_err(io::Error::other)?;
    let mut encrypted = StreamOwned::new(conn, socket);
    let mut line = Vec::new();
    BufReader::new((&mut encrypted).take(512)).read_until(b'\n', &mut line)?;
    let owner = encrypted
        .conn
        .peer_certificates()
        .and_then(|c| c.first())
        .map(|cert| <[u8; 32]>::from(Sha256::digest(cert.as_ref())));
    let accepted = if line == b"FLASHLINK_AUDIO_CAPS/1\n" {
        owner.is_some()
    } else if line == b"FLASHLINK_STREAM/1\n" {
        owner.is_some_and(|id| {
            streams.lock().is_ok_and(|mut s| {
                s.legacy(id, Instant::now());
                true
            })
        })
    } else if let Some(report) = Report::parse(&line) {
        owner.is_some_and(|id| {
            streams
                .lock()
                .is_ok_and(|mut s| s.update(id, report, Instant::now()))
        })
    } else {
        parse_pair(&line, peer, host_code)
            .is_some_and(|p| inbox.lock().is_ok_and(|mut q| q.store(p)))
    };
    encrypted.write_all(if accepted { b"OK\n" } else { b"NO\n" })?;
    encrypted.flush()
}

pub struct Nearby {
    streams: Arc<Mutex<Streams>>,
    inbox: Arc<Mutex<Inbox>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Nearby {
    pub fn start(base: u16, name: &str, cert_path: &Path, key_path: &Path) -> io::Result<Self> {
        Self::listen(Ipv4Addr::UNSPECIFIED, base, name, cert_path, key_path)
    }

    pub(crate) fn listen(
        bind_address: Ipv4Addr,
        base: u16,
        name: &str,
        cert_path: &Path,
        key_path: &Path,
    ) -> io::Result<Self> {
        let consent = base
            .checked_sub(8)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid Sunshine port"))?;
        let cert = CertificateDer::from_pem_file(cert_path).map_err(io::Error::other)?;
        let fingerprint = format!("{:x}", Sha256::digest(cert.as_ref()));
        let prefix = fingerprint[..12].to_ascii_uppercase();
        let host_code = format!("{} {} {}", &prefix[..4], &prefix[4..8], &prefix[8..12]);
        let key = PrivateKeyDer::from_pem_file(key_path).map_err(io::Error::other)?;
        let mut config = ServerConfig::builder()
            .with_client_cert_verifier(Arc::new(super::lease::PairedClients(
                cert_path.with_file_name("flashlink-state.json"),
            )))
            .with_single_cert(vec![cert], key)
            .map_err(io::Error::other)?;
        config.send_tls13_tickets = 0;
        config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
        let tls = Arc::new(config);
        let streams = Arc::new(Mutex::new(Streams::default()));
        let active_streams = streams.clone();
        let udp = UdpSocket::bind((bind_address, DISCOVERY_PORT))
            .map_err(|e| io::Error::new(e.kind(), format!("设备发现 UDP {DISCOVERY_PORT}: {e}")))?;
        udp.set_read_timeout(Some(POLL))?;
        let listener = TcpListener::bind((bind_address, consent))
            .map_err(|e| io::Error::new(e.kind(), format!("设备授权 TCP {consent}: {e}")))?;
        listener.set_nonblocking(true)?;
        let mut networks = Subnets::local()?;
        let name = display_name(name);
        let name = if name.is_empty() {
            "Windows 电脑"
        } else {
            &name
        };
        let info = format!("{base} {fingerprint} {name}\n");
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (queue, signal) = (inbox.clone(), stop.clone());
        let worker = thread::Builder::new()
            .name("FlashLink nearby".into())
            .spawn(move || {
                let active = Arc::new(AtomicUsize::new(0));
                let mut last_networks = Instant::now();
                let mut buf = [0; 256];
                while !signal.load(Ordering::Acquire) {
                    if last_networks.elapsed() >= Duration::from_secs(5) {
                        networks = Subnets::local().unwrap_or_else(|_| Subnets(Vec::new()));
                        last_networks = Instant::now();
                    }
                    if let Ok((n, peer)) = udp.recv_from(&mut buf) {
                        if networks.contains(peer.ip()) {
                            if let Some(nonce) = discover_nonce(&buf[..n]) {
                                let _ = udp.send_to(
                                    format!("FLASHLINK_HOST/1 {nonce} {info}").as_bytes(),
                                    peer,
                                );
                            }
                        }
                    }
                    if let Ok((stream, peer)) = listener.accept() {
                        if !networks.contains(peer.ip()) || active.load(Ordering::Acquire) >= 8 {
                            continue;
                        }
                        active.fetch_add(1, Ordering::AcqRel);
                        let (tls, queue, count) = (tls.clone(), queue.clone(), active.clone());
                        let host_code = host_code.clone();
                        let reports = active_streams.clone();
                        if thread::Builder::new()
                            .name("FlashLink pairing".into())
                            .spawn(move || {
                                let _ = receive(stream, tls, &queue, &host_code, &reports);
                                count.fetch_sub(1, Ordering::AcqRel);
                            })
                            .is_err()
                        {
                            active.fetch_sub(1, Ordering::AcqRel);
                        }
                    }
                }
            })?;
        Ok(Self {
            streams,
            inbox,
            stop,
            worker: Some(worker),
        })
    }

    pub fn heartbeat(&self, owner: &[u8; 32]) -> Option<Instant> {
        self.streams
            .lock()
            .ok()?
            .heartbeat
            .filter(|(id, _)| id == owner)
            .map(|(_, ts)| ts)
    }

    pub fn audio_session(&self) -> Option<AudioSession> {
        self.streams.lock().ok()?.audio
    }

    pub fn take_request(&self, source_ip: IpAddr, pairing_name: &str) -> Option<Approval> {
        self.inbox
            .lock()
            .ok()?
            .take(source_ip.to_canonical(), pairing_name)
    }
}

impl Drop for Nearby {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const TOKEN: &str = "281202ee7c001f0281202ee7c001f02a";

    #[test]
    fn discovery_echoes_only_valid_nonces() {
        let query = format!("FLASHLINK_DISCOVER/1 {TOKEN}\n");
        assert_eq!(discover_nonce(query.as_bytes()), Some(TOKEN));
        assert_eq!(discover_nonce(b"FLASHLINK_DISCOVER/1 extra fields\n"), None);
        assert_eq!(discover_nonce(format!("{query}extra").as_bytes()), None);
    }

    #[test]
    fn approval_requires_both_peer_and_one_time_token() {
        let line = format!("FLASHLINK_PAIR/1 {TOKEN} 0123 我的平板\n");
        let peer: IpAddr = "192.168.1.3".parse().unwrap();
        let mut q = Inbox::default();
        assert!(q.store(parse_pair(line.as_bytes(), peer, "AB12 CDEF 45AB").unwrap()));
        assert!(!q.store(parse_pair(line.as_bytes(), peer, "AB12 CDEF 45AB").unwrap()));
        let name = format!("FlashLink-{TOKEN}");
        assert!(q.take("192.168.1.4".parse().unwrap(), &name).is_none());
        assert!(q.take(peer, "Other").is_none());
        let approved = q.take(peer, &name).unwrap();
        assert_eq!(
            (
                approved.pin.as_str(),
                approved.code.as_str(),
                approved.name.as_str()
            ),
            ("0123", "AB12 CDEF 45AB 0123", "我的平板")
        );
        assert!(q.take(peer, &name).is_none());
        let mut old = parse_pair(line.as_bytes(), peer, "AB12 CDEF 45AB").unwrap();
        old.created -= LIFETIME;
        assert!(q.store(old));
        assert!(q.take(peer, &name).is_none());
        assert!(parse_pair(
            format!("FLASHLINK_PAIR/1 {TOKEN} 1234 bad\nextra").as_bytes(),
            peer,
            "AB12 CDEF 45AB"
        )
        .is_none());
    }

    #[test]
    fn accepted_socket_waits_for_data_after_the_tls_handshake() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        server.set_nonblocking(true).unwrap();
        let mut reader = DeadlineSocket::new(server, Duration::from_secs(3)).unwrap();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(80));
            client.write_all(b"test").unwrap();
        });
        let mut reply = [0; 4];
        reader.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"test");
        writer.join().unwrap();
    }

    #[test]
    fn discovery_uses_current_interface_prefix() {
        let subnets = Subnets(vec![(
            u32::from(Ipv4Addr::new(192, 168, 4, 10)),
            0xffffff00,
        )]);
        assert!(subnets.contains("192.168.4.19".parse().unwrap()));
        assert!(!subnets.contains("192.168.5.19".parse().unwrap()));
        assert!(!subnets.contains("0.0.0.0".parse().unwrap()));
    }
}

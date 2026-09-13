//! Wezel live: wykrywanie peerow w LAN, polaczenia TCP, replikacja.
//!
//! Watki (wszystkie poza watkiem okna):
//! - `live` - petla glowna: jedna kolejka `Cmd` z okna i z I/O, caly stan,
//!   dysk (`Replica`); jedyny watek, ktory cokolwiek decyduje,
//! - `live-beacon` - co 2 s rozglasza `SPCTLV1` na multicast i slucha innych,
//! - `live-accept` - `TcpListener::accept`,
//! - per polaczenie: czytnik (blokujacy `read`) i pisarz (kanal -> `write_all`).
//!
//! Polaczenie nawiazuje instancja o **mniejszym** id - druga czeka; dzieki temu
//! dwa wezly, ktore widza sie nawzajem, nie otwieraja dwoch polaczen.
//! `TCP_NODELAY`: probki mokrej kreski maja wychodzic natychmiast, nie po
//! 40 ms Nagle'a. Bez szyfrowania - granica zaufania v1 to LAN / Tailscale
//! (ADR 0007).

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use socket2::{Domain, Protocol, Socket, Type};
use spectre_proto::codec::{decode_str, encode_str};
use spectre_proto::varint::{put_u64, Reader};
use spectre_proto::{AuthorId, Op, StrokeData};

use crate::author::AuthorName;
use crate::live::replica::{encode_records, Key, Replica};
use crate::live::wire::{next_frame, Frame, Msg, PROTO_VERSION};

pub const MCAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 94, 94);
pub const MCAST_PORT: u16 = 47941;
const BEACON_MAGIC: &[u8; 8] = b"SPCTLV1\0";
const BEACON_EVERY: Duration = Duration::from_secs(2);
/// Po nieudanym polaczeniu do tego samego peera probujemy dopiero po tym czasie.
const CONNECT_RETRY: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Zadania z okna.
pub enum Job {
    /// Operacje lokalne - aplikacja juz zapisala je w swoim pliku.
    Local {
        note: String,
        ops: Vec<Op>,
    },
    /// Paczka probek biezacej kreski; `seq` 0 = nowa kreska.
    Wet {
        note: String,
        seq: u32,
        data: StrokeData,
    },
    /// Rysik nad notatka (`x = NaN` = zniknal).
    Cursor {
        note: String,
        x: f32,
        y: f32,
    },
    /// Po merge gita: pliki tych notatek mogly urosnac (pusty = wszystkie).
    Rescan(Vec<String>),
    /// Okno widoczne/ukryte: rozglaszanie tylko przy widocznym (Z2).
    Visible(bool),
    /// Ustawienie: live wl./wyl. Wylaczenie zrywa polaczenia.
    Enabled(bool),
    /// Polaczenie pod wskazany adres (testy; Tailscale bez multicastu).
    Connect(SocketAddr),
    Quit,
}

/// Zdarzenia do okna.
pub enum Event {
    Peer {
        instance: u64,
        author_dir: String,
        connected: bool,
    },
    /// Nowe operacje od peera, juz na dysku. `new_note` = notatki nie bylo.
    Ops {
        note: String,
        author: AuthorId,
        ops: Vec<Op>,
        new_note: bool,
    },
    Wet {
        note: String,
        author: AuthorId,
        seq: u32,
        /// Zegar nadawcy minus nasz; na jednej maszynie to realne opoznienie.
        latency_us: i64,
        data: StrokeData,
    },
    Cursor {
        note: String,
        author: AuthorId,
        x: f32,
        y: f32,
    },
    Error(String),
}

pub struct Node {
    tx: Sender<Cmd>,
    rx: Receiver<Event>,
    pub port: u16,
    pub instance: u64,
}

impl Node {
    /// Nasluch TCP na losowym porcie, multicast na `MCAST_PORT`, watki w tle.
    /// `wake` budzi okno po kazdej porcji zdarzen.
    pub fn start(
        root: &std::path::Path,
        me: &AuthorName,
        wake: Box<dyn Fn() + Send>,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        let port = listener.local_addr()?.port();
        let udp = multicast_socket()?;
        let instance = random_u64();
        let (tx, cmds) = mpsc::channel::<Cmd>();
        let (events, rx) = mpsc::channel::<Event>();
        let discovering = Arc::new(AtomicBool::new(true));

        let root = root.to_path_buf();
        let me = me.clone();
        let space = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        {
            let tx = tx.clone();
            thread::Builder::new()
                .name("live-accept".into())
                .spawn(move || {
                    for s in listener.incoming().flatten() {
                        if tx.send(Cmd::Stream(s)).is_err() {
                            break;
                        }
                    }
                })?;
        }
        {
            let tx = tx.clone();
            let beacon = Beacon {
                instance,
                port,
                space: space.clone(),
                author_dir: me.dir_name(),
            }
            .encode();
            let discovering = discovering.clone();
            thread::Builder::new()
                .name("live-beacon".into())
                .spawn(move || beacon_loop(udp, beacon, discovering, tx))?;
        }
        {
            let tx = tx.clone();
            thread::Builder::new().name("live".into()).spawn(move || {
                let replica = match Replica::open(&root, &me) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("live: {e}")));
                        wake();
                        return;
                    }
                };
                let mut st = State {
                    replica,
                    space,
                    instance,
                    conns: HashMap::new(),
                    next_conn: 1,
                    connecting: HashMap::new(),
                    enabled: true,
                    visible: true,
                    quit_requested: false,
                    discovering,
                    events,
                    wake,
                    tx,
                };
                st.run(cmds);
            })?;
        }
        Ok(Self {
            tx,
            rx,
            port,
            instance,
        })
    }

    pub fn send(&self, job: Job) {
        let _ = self.tx.send(Cmd::Job(job));
    }

    pub fn poll(&self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            out.push(e);
        }
        out
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Job(Job::Quit));
    }
}

// ----- wewnetrzne --------------------------------------------------------

enum Cmd {
    Job(Job),
    Stream(TcpStream),
    ConnectFailed,
    Frame(u64, Msg),
    Gone(u64),
    Beacon(Beacon, SocketAddr),
}

struct Beacon {
    instance: u64,
    port: u16,
    space: String,
    author_dir: String,
}

impl Beacon {
    fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(64);
        b.extend_from_slice(BEACON_MAGIC);
        b.extend_from_slice(&PROTO_VERSION.to_le_bytes());
        put_u64(&mut b, self.instance);
        b.extend_from_slice(&self.port.to_le_bytes());
        encode_str(&self.space, &mut b);
        encode_str(&self.author_dir, &mut b);
        b
    }

    fn decode(buf: &[u8]) -> Option<Self> {
        let mut r = Reader::new(buf);
        if r.bytes(8).ok()? != BEACON_MAGIC || r.u16_le().ok()? != PROTO_VERSION {
            return None;
        }
        Some(Self {
            instance: r.u64().ok()?,
            port: r.u16_le().ok()?,
            space: decode_str(&mut r).ok()?,
            author_dir: decode_str(&mut r).ok()?,
        })
    }
}

/// Gniazdo multicast: `SO_REUSEADDR`, zeby dwie instancje na jednej maszynie
/// mogly dzielic port; petla zwrotna, zeby sie nawzajem slyszaly.
fn multicast_socket() -> io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    s.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MCAST_PORT).into())?;
    s.join_multicast_v4(&MCAST_ADDR, &Ipv4Addr::UNSPECIFIED)?;
    s.set_multicast_loop_v4(true)?;
    s.set_read_timeout(Some(Duration::from_millis(500)))?;
    Ok(s.into())
}

fn beacon_loop(udp: UdpSocket, beacon: Vec<u8>, discovering: Arc<AtomicBool>, tx: Sender<Cmd>) {
    let target = SocketAddrV4::new(MCAST_ADDR, MCAST_PORT);
    let mut last_sent: Option<Instant> = None;
    let mut buf = [0u8; 512];
    loop {
        if discovering.load(Ordering::Relaxed)
            && last_sent
                .map(|t| t.elapsed() >= BEACON_EVERY)
                .unwrap_or(true)
        {
            let _ = udp.send_to(&beacon, target);
            last_sent = Some(Instant::now());
        }
        match udp.recv_from(&mut buf) {
            Ok((n, src)) => {
                if let Some(b) = Beacon::decode(&buf[..n]) {
                    if tx.send(Cmd::Beacon(b, src)).is_err() {
                        return;
                    }
                }
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
            }
            // Windows zglasza tu ICMP "port unreachable" z poprzednich wysylek -
            // nieistotne dla multicastu.
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

struct Conn {
    /// Znane po `Hello`.
    instance: Option<u64>,
    author_dir: String,
    author: AuthorId,
    tx: Sender<Vec<u8>>,
    stream: TcpStream,
    /// Co peer ma (z jego `Summary` + wszystko, co poszlo w obie strony).
    knows: BTreeMap<Key, u64>,
}

impl Conn {
    fn send(&self, m: &Msg) {
        let _ = self.tx.send(m.encode());
    }

    fn ready(&self) -> bool {
        self.instance.is_some()
    }
}

struct State {
    replica: Replica,
    space: String,
    instance: u64,
    conns: HashMap<u64, Conn>,
    next_conn: u64,
    /// Ostatnia proba polaczenia per instancja peera (odstep miedzy probami).
    connecting: HashMap<u64, Instant>,
    enabled: bool,
    visible: bool,
    quit_requested: bool,
    discovering: Arc<AtomicBool>,
    events: Sender<Event>,
    wake: Box<dyn Fn() + Send>,
    tx: Sender<Cmd>,
}

impl State {
    fn run(&mut self, cmds: Receiver<Cmd>) {
        while let Ok(cmd) = cmds.recv() {
            let mut emitted = self.handle(cmd);
            while let Ok(c) = cmds.try_recv() {
                emitted |= self.handle(c);
            }
            if emitted {
                (self.wake)();
            }
            if self.quit_requested {
                break;
            }
        }
    }

    /// Zwraca, czy poszlo zdarzenie do okna.
    fn handle(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Job(j) => self.job(j),
            Cmd::Stream(s) => {
                self.attach(s);
                false
            }
            Cmd::ConnectFailed => false,
            Cmd::Frame(id, m) => self.frame(id, m),
            Cmd::Gone(id) => self.drop_conn(id),
            Cmd::Beacon(b, src) => {
                self.beacon(b, src);
                false
            }
        }
    }

    fn emit(&self, e: Event) -> bool {
        self.events.send(e).is_ok()
    }

    fn job(&mut self, j: Job) -> bool {
        match j {
            Job::Local { note, ops } => {
                let me = self.replica.me().dir_name();
                let prev = self.replica.last(&note, &me);
                self.replica.note_local(&note, &ops);
                let last = self.replica.last(&note, &me);
                let key = (note.clone(), me.clone());
                let records = encode_records(ops.iter());
                let author = self.replica.me().id();
                for c in self.conns.values_mut().filter(|c| c.ready()) {
                    let theirs = c.knows.get(&key).copied().unwrap_or(0);
                    if theirs >= last {
                        continue;
                    }
                    let recs = if theirs == prev {
                        records.clone()
                    } else {
                        // Peer jest w tyle - doslac z dysku wszystko od jego stanu.
                        self.replica
                            .records_after(&note, &me, theirs)
                            .unwrap_or_default()
                    };
                    c.send(&Msg::Ops {
                        note: note.clone(),
                        author_dir: me.clone(),
                        author,
                        records: recs,
                    });
                    c.knows.insert(key.clone(), last);
                }
                false
            }
            Job::Wet { note, seq, data } => {
                let m = Msg::Wet {
                    note,
                    author: self.replica.me().id(),
                    seq,
                    t_sent_us: now_us(),
                    data,
                };
                self.broadcast(&m);
                false
            }
            Job::Cursor { note, x, y } => {
                self.broadcast(&Msg::Cursor { note, x, y });
                false
            }
            Job::Rescan(notes) => {
                let r = if notes.is_empty() {
                    self.replica.rescan_all()
                } else {
                    notes.iter().try_for_each(|n| self.replica.rescan_note(n))
                };
                if let Err(e) = r {
                    return self.emit(Event::Error(format!("live: scan: {e}")));
                }
                let ids: Vec<u64> = self.conns.keys().copied().collect();
                for id in ids {
                    self.push_missing(id, &notes);
                }
                false
            }
            Job::Visible(v) => {
                self.visible = v;
                self.update_discovery();
                false
            }
            Job::Enabled(e) => {
                self.enabled = e;
                self.update_discovery();
                if !e {
                    let ids: Vec<u64> = self.conns.keys().copied().collect();
                    let mut emitted = false;
                    for id in ids {
                        if let Some(c) = self.conns.get(&id) {
                            c.send(&Msg::Bye);
                        }
                        emitted |= self.drop_conn(id);
                    }
                    return emitted;
                }
                false
            }
            Job::Connect(addr) => {
                self.spawn_connect(addr);
                false
            }
            Job::Quit => {
                for c in self.conns.values() {
                    c.send(&Msg::Bye);
                }
                self.conns.clear();
                self.enabled = false;
                self.quit_requested = true;
                self.update_discovery();
                false
            }
        }
    }

    fn update_discovery(&self) {
        self.discovering
            .store(self.enabled && self.visible, Ordering::Relaxed);
    }

    fn broadcast(&self, m: &Msg) {
        if self.conns.values().any(Conn::ready) {
            let bytes = m.encode();
            for c in self.conns.values().filter(|c| c.ready()) {
                let _ = c.tx.send(bytes.clone());
            }
        }
    }

    /// Nowy strumien (przyjety albo nawiazany): watki I/O i `Hello`.
    fn attach(&mut self, stream: TcpStream) {
        if !self.enabled {
            return;
        }
        let _ = stream.set_nodelay(true);
        let id = self.next_conn;
        self.next_conn += 1;
        let (wtx, wrx) = mpsc::channel::<Vec<u8>>();
        let Ok(reader) = stream.try_clone() else {
            return;
        };
        let Ok(writer) = stream.try_clone() else {
            return;
        };
        let tx = self.tx.clone();
        let _ = thread::Builder::new()
            .name(format!("live-rd-{id}"))
            .spawn(move || read_loop(reader, id, tx));
        let _ = thread::Builder::new()
            .name(format!("live-wr-{id}"))
            .spawn(move || write_loop(writer, wrx));
        let c = Conn {
            instance: None,
            author_dir: String::new(),
            author: AuthorId(0),
            tx: wtx,
            stream,
            knows: BTreeMap::new(),
        };
        c.send(&Msg::Hello {
            version: PROTO_VERSION,
            space: self.space.clone(),
            author_dir: self.replica.me().dir_name(),
            author: self.replica.me().id(),
            instance: self.instance,
        });
        self.conns.insert(id, c);
    }

    fn drop_conn(&mut self, id: u64) -> bool {
        let Some(c) = self.conns.remove(&id) else {
            return false;
        };
        let _ = c.stream.shutdown(Shutdown::Both);
        match c.instance {
            Some(instance) => self.emit(Event::Peer {
                instance,
                author_dir: c.author_dir,
                connected: false,
            }),
            None => false,
        }
    }

    fn frame(&mut self, id: u64, m: Msg) -> bool {
        if !self.conns.contains_key(&id) {
            return false;
        }
        match m {
            Msg::Hello {
                version,
                space,
                author_dir,
                author,
                instance,
            } => {
                let dup = self
                    .conns
                    .iter()
                    .any(|(cid, c)| *cid != id && c.instance == Some(instance));
                // Ten sam autor na dwoch wezlach = dwa pisarze jednego pliku po
                // merge'u gita; odmawiamy, zanim cokolwiek poplynie.
                let same_author = author_dir == self.replica.me().dir_name();
                if version != PROTO_VERSION
                    || space != self.space
                    || instance == self.instance
                    || dup
                    || same_author
                {
                    if let Some(c) = self.conns.get(&id) {
                        c.send(&Msg::Bye);
                    }
                    self.conns.remove(&id);
                    return same_author
                        && self.emit(Event::Error(format!(
                            "live: {author_dir} on another device has the same author name"
                        )));
                }
                let summary = Msg::Summary(self.replica.summary());
                let c = self.conns.get_mut(&id).expect("conn");
                c.instance = Some(instance);
                c.author_dir = author_dir.clone();
                c.author = author;
                c.send(&summary);
                self.connecting.remove(&instance);
                self.emit(Event::Peer {
                    instance,
                    author_dir,
                    connected: true,
                })
            }
            Msg::Summary(list) => {
                if let Some(c) = self.conns.get_mut(&id) {
                    c.knows = list
                        .into_iter()
                        .map(|h| ((h.note, h.author_dir), h.last))
                        .collect();
                }
                self.push_missing(id, &[]);
                false
            }
            Msg::Ops {
                note,
                author_dir,
                author,
                records,
            } => {
                let (fresh, new_note) =
                    match self
                        .replica
                        .store_remote(&note, &author_dir, author, &records)
                    {
                        Ok(r) => r,
                        Err(e) => return self.emit(Event::Error(format!("live: write: {e}"))),
                    };
                let last = self.replica.last(&note, &author_dir);
                let key = (note.clone(), author_dir.clone());
                if let Some(c) = self.conns.get_mut(&id) {
                    let k = c.knows.entry(key.clone()).or_insert(0);
                    *k = (*k).max(last);
                }
                if fresh.is_empty() {
                    return false;
                }
                // Dalej do peerow, ktorzy tego nie maja (trzeci wezel bez
                // bezposredniego polaczenia z autorem). Konczy sie, bo kazdy
                // wezel przekazuje tylko to, co bylo dla niego nowe.
                for (cid, c) in self.conns.iter_mut() {
                    if *cid == id || !c.ready() {
                        continue;
                    }
                    if c.knows.get(&key).copied().unwrap_or(0) < last {
                        c.send(&Msg::Ops {
                            note: note.clone(),
                            author_dir: author_dir.clone(),
                            author,
                            records: records.clone(),
                        });
                        c.knows.insert(key.clone(), last);
                    }
                }
                self.emit(Event::Ops {
                    note,
                    author,
                    ops: fresh,
                    new_note,
                })
            }
            Msg::Wet {
                note,
                author,
                seq,
                t_sent_us,
                data,
            } => self.emit(Event::Wet {
                note,
                author,
                seq,
                latency_us: now_us() as i64 - t_sent_us as i64,
                data,
            }),
            Msg::Cursor { note, x, y } => {
                let author = self.conns.get(&id).map(|c| c.author).unwrap_or(AuthorId(0));
                self.emit(Event::Cursor { note, author, x, y })
            }
            Msg::Bye => self.drop_conn(id),
        }
    }

    /// Wysyla peerowi wszystko, czego wg naszej tabeli nie ma.
    fn push_missing(&mut self, id: u64, only: &[String]) {
        let Some(c) = self.conns.get(&id) else {
            return;
        };
        if !c.ready() {
            return;
        }
        let msgs = self.replica.missing_for(&c.knows, only);
        let c = self.conns.get_mut(&id).expect("conn");
        for m in msgs {
            if let Msg::Ops {
                note, author_dir, ..
            } = &m
            {
                let last = self.replica.last(note, author_dir);
                c.knows.insert((note.clone(), author_dir.clone()), last);
            }
            c.send(&m);
        }
    }

    fn beacon(&mut self, b: Beacon, src: SocketAddr) {
        if !self.enabled || b.instance == self.instance || b.space != self.space {
            return;
        }
        if self.conns.values().any(|c| c.instance == Some(b.instance)) {
            return;
        }
        // Laczy ten z mniejszym id; drugi czeka na `accept`.
        if self.instance > b.instance {
            return;
        }
        if let Some(t) = self.connecting.get(&b.instance) {
            if t.elapsed() < CONNECT_RETRY {
                return;
            }
        }
        self.connecting.insert(b.instance, Instant::now());
        self.spawn_connect(SocketAddr::new(src.ip(), b.port));
    }

    fn spawn_connect(&self, addr: SocketAddr) {
        let tx = self.tx.clone();
        let _ = thread::Builder::new()
            .name("live-connect".into())
            .spawn(move || {
                let cmd = match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                    Ok(s) => Cmd::Stream(s),
                    Err(_) => Cmd::ConnectFailed,
                };
                let _ = tx.send(cmd);
            });
    }
}

fn read_loop(mut stream: TcpStream, id: u64, tx: Sender<Cmd>) {
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut chunk = [0u8; 64 * 1024];
    'outer: loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        let mut pos = 0;
        loop {
            match next_frame(&buf[pos..]) {
                Frame::Msg(m, used) => {
                    pos += used;
                    if tx.send(Cmd::Frame(id, m)).is_err() {
                        break 'outer;
                    }
                }
                Frame::Partial => break,
                Frame::Broken => break 'outer,
            }
        }
        buf.drain(..pos);
    }
    let _ = stream.shutdown(Shutdown::Both);
    let _ = tx.send(Cmd::Gone(id));
}

fn write_loop(mut stream: TcpStream, rx: Receiver<Vec<u8>>) {
    while let Ok(frame) = rx.recv() {
        if stream.write_all(&frame).is_err() {
            break;
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// Losowy identyfikator instancji: czas, pid i adres na stosie przez FNV.
fn random_u64() -> u64 {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let marker = 0u8;
    let addr = &marker as *const u8 as u64;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in [t, std::process::id() as u64, addr] {
        for b in v.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Space;
    use spectre_core::Document;
    use spectre_proto::{Rgba, Sample};
    use std::sync::atomic::AtomicUsize;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("spectre-node-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p.join("default")
    }

    fn stroke(x: f32) -> StrokeData {
        StrokeData {
            tool: 0,
            color: Rgba::rgb(9, 8, 7),
            base_width: 2.0,
            samples: vec![Sample {
                x,
                y: 1.0,
                ..Default::default()
            }],
        }
    }

    /// Czeka na zdarzenie spelniajace warunek, zbierajac po drodze wszystkie.
    fn wait_for(node: &Node, wakes: &AtomicUsize, pred: impl Fn(&Event) -> bool) -> Vec<Event> {
        let t0 = Instant::now();
        let mut got = Vec::new();
        while t0.elapsed() < Duration::from_secs(10) {
            for e in node.poll() {
                let hit = pred(&e);
                got.push(e);
                if hit {
                    assert!(wakes.load(Ordering::Relaxed) > 0, "budzenie okna");
                    return got;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("brak zdarzenia po 10 s; odebrano {}", got.len());
    }

    #[test]
    fn dwa_wezly_na_petli_zwrotnej() {
        let ra = tmp("a");
        let rb = tmp("b");
        let adi = AuthorName::new("adi", "laptop");
        let kuba = AuthorName::new("kuba", "surface");

        // A ma juz jedna kreske na dysku zanim B sie pojawi.
        let space_a = Space::open_or_create(&ra).unwrap();
        let note = space_a.create_note().unwrap();
        let mut doc_a = Document::new(adi.id());
        {
            let (mut s, _) = crate::NoteStore::open(&space_a, &note, &adi).unwrap();
            s.append(&doc_a.add_stroke(stroke(1.0))).unwrap();
            s.sync().unwrap();
        }

        let wakes_a = Arc::new(AtomicUsize::new(0));
        let wakes_b = Arc::new(AtomicUsize::new(0));
        let wa = wakes_a.clone();
        let wb = wakes_b.clone();
        let a = Node::start(
            &ra,
            &adi,
            Box::new(move || {
                wa.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .unwrap();
        let b = Node::start(
            &rb,
            &kuba,
            Box::new(move || {
                wb.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .unwrap();
        // Bez czekania na multicast: B laczy sie wprost.
        b.send(Job::Connect(SocketAddr::from(([127, 0, 0, 1], a.port))));

        let evs = wait_for(&a, &wakes_a, |e| {
            matches!(
                e,
                Event::Peer {
                    connected: true,
                    ..
                }
            )
        });
        assert!(evs
            .iter()
            .any(|e| matches!(e, Event::Peer { author_dir, .. } if author_dir == "kuba@surface")));

        // B po Summary dostaje kreske A z dysku (dolaczenie w trakcie).
        let evs = wait_for(&b, &wakes_b, |e| matches!(e, Event::Ops { .. }));
        let Some(Event::Ops {
            note: n,
            ops,
            new_note,
            ..
        }) = evs.iter().find(|e| matches!(e, Event::Ops { .. }))
        else {
            panic!()
        };
        assert_eq!(n, &note);
        assert_eq!(ops.len(), 1);
        assert!(new_note);

        // A rysuje na zywo: mokra kreska i operacja koncowa docieraja do B.
        a.send(Job::Wet {
            note: note.clone(),
            seq: 0,
            data: stroke(2.0),
        });
        let evs = wait_for(&b, &wakes_b, |e| matches!(e, Event::Wet { .. }));
        let Some(Event::Wet {
            latency_us, seq, ..
        }) = evs.iter().find(|e| matches!(e, Event::Wet { .. }))
        else {
            panic!()
        };
        assert_eq!(*seq, 0);
        assert!(*latency_us < 500_000, "opoznienie {latency_us} us");

        let op = doc_a.add_stroke(stroke(2.0));
        {
            let (mut s, _) = crate::NoteStore::open(&space_a, &note, &adi).unwrap();
            s.append(&op).unwrap();
            s.sync().unwrap();
        }
        a.send(Job::Local {
            note: note.clone(),
            ops: vec![op.clone()],
        });
        let evs = wait_for(
            &b,
            &wakes_b,
            |e| matches!(e, Event::Ops { ops, .. } if ops[0].lamport == op.lamport),
        );
        assert!(evs.iter().all(|e| !matches!(e, Event::Error(_))));

        // Na dysku B: via-plik A, czytelny zwyklym NoteStore.
        let space_b = Space::open_or_create(&rb).unwrap();
        let (_, loaded) = crate::NoteStore::open(&space_b, &note, &kuba).unwrap();
        assert_eq!(loaded.len(), 2);

        // B rysuje - A dostaje (kierunek odwrotny, B jest inicjatorem polaczenia).
        let space_b = Space::open_or_create(&rb).unwrap();
        let mut doc_b = Document::new(kuba.id());
        let op_b = doc_b.add_stroke(stroke(3.0));
        {
            let (mut s, _) = crate::NoteStore::open(&space_b, &note, &kuba).unwrap();
            s.append(&op_b).unwrap();
            s.sync().unwrap();
        }
        b.send(Job::Local {
            note: note.clone(),
            ops: vec![op_b.clone()],
        });
        wait_for(
            &a,
            &wakes_a,
            |e| matches!(e, Event::Ops { author, .. } if *author == kuba.id()),
        );

        // Rozlaczenie: B wylacza live, A widzi odejscie.
        b.send(Job::Enabled(false));
        wait_for(&a, &wakes_a, |e| {
            matches!(
                e,
                Event::Peer {
                    connected: false,
                    ..
                }
            )
        });

        drop(a);
        drop(b);
        let _ = std::fs::remove_dir_all(ra.parent().unwrap());
        let _ = std::fs::remove_dir_all(rb.parent().unwrap());
    }

    #[test]
    fn beacon_roundtrip() {
        let b = Beacon {
            instance: 5,
            port: 1234,
            space: "default".into(),
            author_dir: "adi@laptop".into(),
        };
        let d = Beacon::decode(&b.encode()).unwrap();
        assert_eq!(
            (d.instance, d.port, d.space.as_str(), d.author_dir.as_str()),
            (5, 1234, "default", "adi@laptop")
        );
        assert!(Beacon::decode(b"xx").is_none());
    }
}

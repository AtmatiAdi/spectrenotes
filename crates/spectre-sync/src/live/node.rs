//! Wezel live: wykrywanie peerow w LAN, polaczenia TCP, sesje notatek.
//!
//! Watki (wszystkie poza watkiem okna):
//! - `live` - petla glowna: jedna kolejka `Cmd` z okna i z I/O, caly stan,
//!   dysk (`Replica`); jedyny watek, ktory cokolwiek decyduje,
//! - `live-beacon` - co 2 s rozglasza `SPCTLV2` na multicast i slucha innych,
//! - `live-accept` - `TcpListener::accept`,
//! - per polaczenie: czytnik (blokujacy `read`) i pisarz (kanal -> `write_all`).
//!
//! Polaczenie nawiazuje instancja o **mniejszym** id - druga czeka; dzieki temu
//! dwa wezly, ktore widza sie nawzajem, nie otwieraja dwoch polaczen. Peer bez
//! multicastu (Tailscale) to staly adres z ustawien: laczymy sie sami i
//! ponawiamy co `CONNECT_RETRY`.
//!
//! Polaczenie samo w sobie nic nie replikuje. Plyna tylko notatki **otwarte**
//! na nim: udostepnione przez jedna strone (`Job::Share`) i otwarte przez
//! druga (`Job::Open`, z dowodem hasla - `share.rs`). ADR 0008.
//!
//! `TCP_NODELAY`: probki mokrej kreski maja wychodzic natychmiast, nie po
//! 40 ms Nagle'a. Bez szyfrowania - granica zaufania v1 to LAN / Tailscale
//! (ADR 0007).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use socket2::{Domain, Protocol, Socket, Type};
use spectre_proto::codec::{decode_str, encode_str};
use spectre_proto::varint::{put_u64, Reader};
use spectre_proto::{AuthorId, Op, StrokeData};

use crate::author::AuthorName;
use crate::live::replica::{encode_records, Key, Replica};
use crate::live::share::{self, Key as ShareKey, Proof};
use crate::live::wire::{next_frame, Frame, Msg, SharedNote, PROTO_VERSION};

pub const MCAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 94, 94);
pub const MCAST_PORT: u16 = 47941;
const BEACON_MAGIC: &[u8; 8] = b"SPCTLV2\0";
const BEACON_EVERY: Duration = Duration::from_secs(2);
/// Po nieudanym polaczeniu do tego samego peera probujemy dopiero po tym czasie.
const CONNECT_RETRY: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Heartbeat: po tylu sekundach ciszy pytamy `Ping`; bez odpowiedzi do
/// `DEAD_AFTER` polaczenie uznajemy za zerwane (TCP sam zauwazylby to po
/// minutach). Petla glowna budzi sie co `TICK` na te sprawdzenia.
const TICK: Duration = Duration::from_secs(2);
const PING_AFTER: Duration = Duration::from_secs(5);
const DEAD_AFTER: Duration = Duration::from_secs(12);

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
    /// Udostepnij notatke (ponownie = zmiana tytulu/hasla). `key` = klucz
    /// z hasla (`share::key_from_password`), `None` = bez hasla.
    Share {
        note: String,
        title: String,
        key: Option<ShareKey>,
    },
    Unshare(String),
    /// Otworz notatke udostepniana przez peera (teraz albo gdy sie pojawi).
    Open {
        note: String,
        key: Option<ShareKey>,
    },
    Close(String),
    /// Stale adresy peerow (Tailscale, bez multicastu): pelna lista.
    Peers(Vec<SocketAddr>),
    /// Jednorazowe polaczenie pod adres (testy).
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
    /// Co peer udostepnia (pelna lista; po polaczeniu i po kazdej zmianie).
    Shared {
        instance: u64,
        author_dir: String,
        notes: Vec<SharedNote>,
    },
    /// Wynik `Job::Open` u tego peera. `ok = false` = zle haslo albo
    /// notatka juz nie jest udostepniana.
    Opened {
        instance: u64,
        author_dir: String,
        note: String,
        ok: bool,
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

        {
            let tx = tx.clone();
            thread::Builder::new()
                .name("live-accept".into())
                .spawn(move || {
                    for s in listener.incoming().flatten() {
                        if tx.send(Cmd::Stream(s, None)).is_err() {
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
                    instance,
                    conns: HashMap::new(),
                    next_conn: 1,
                    connecting: HashMap::new(),
                    shares: BTreeMap::new(),
                    wants: BTreeMap::new(),
                    static_peers: Vec::new(),
                    static_instance: HashMap::new(),
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
    /// Strumien przyjety (`None`) albo nawiazany pod adres.
    Stream(TcpStream, Option<SocketAddr>),
    ConnectFailed,
    Frame(u64, Msg),
    Gone(u64),
    Beacon(Beacon, SocketAddr),
}

struct Beacon {
    instance: u64,
    port: u16,
    author_dir: String,
}

impl Beacon {
    fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(64);
        b.extend_from_slice(BEACON_MAGIC);
        b.extend_from_slice(&PROTO_VERSION.to_le_bytes());
        put_u64(&mut b, self.instance);
        b.extend_from_slice(&self.port.to_le_bytes());
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

struct Share {
    title: String,
    key: Option<ShareKey>,
}

struct Conn {
    /// Znane po `Hello`.
    instance: Option<u64>,
    author_dir: String,
    author: AuthorId,
    nonce_mine: u64,
    nonce_theirs: u64,
    tx: Sender<Vec<u8>>,
    stream: TcpStream,
    /// Adres, pod ktory sami sie polaczylismy (staly peer / beacon).
    addr: Option<SocketAddr>,
    /// Co peer udostepnia (jego ostatnie `Shared`).
    offers: Vec<SharedNote>,
    /// Notatki w sesji na tym polaczeniu - tylko one plyna.
    open: BTreeSet<String>,
    /// Co peer ma w otwartych notatkach (z jego `Summary` + wszystko, co
    /// poszlo w obie strony).
    knows: BTreeMap<Key, u64>,
    last_rx: Instant,
    ping_sent: bool,
}

impl Conn {
    fn send(&self, m: &Msg) {
        let _ = self.tx.send(m.encode());
    }

    fn ready(&self) -> bool {
        self.instance.is_some()
    }

    fn has_open(&self, note: &str) -> bool {
        self.ready() && self.open.contains(note)
    }
}

struct State {
    replica: Replica,
    instance: u64,
    conns: HashMap<u64, Conn>,
    next_conn: u64,
    /// Ostatnia proba polaczenia per adres (odstep miedzy probami).
    connecting: HashMap<SocketAddr, Instant>,
    /// Moje udostepnienia.
    shares: BTreeMap<String, Share>,
    /// Cudze notatki, ktore chce miec otwarte (z kluczem, gdy chronione).
    wants: BTreeMap<String, Option<ShareKey>>,
    static_peers: Vec<SocketAddr>,
    /// Instancja widziana pod stalym adresem - zeby nie laczyc sie ponownie,
    /// gdy to polaczenie od tamtej strony przezylo deduplikacje.
    static_instance: HashMap<SocketAddr, u64>,
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
        let mut next_tick = Instant::now() + TICK;
        loop {
            let wait = next_tick.saturating_duration_since(Instant::now());
            let mut emitted = match cmds.recv_timeout(wait) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            while let Ok(c) = cmds.try_recv() {
                emitted |= self.handle(c);
            }
            if Instant::now() >= next_tick {
                emitted |= self.tick();
                next_tick = Instant::now() + TICK;
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
            Cmd::Stream(s, addr) => {
                self.attach(s, addr);
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

    /// Co `TICK`: heartbeat i ponawianie stalych peerow.
    fn tick(&mut self) -> bool {
        let mut emitted = false;
        let now = Instant::now();
        let dead: Vec<u64> = self
            .conns
            .iter()
            .filter(|(_, c)| now.duration_since(c.last_rx) >= DEAD_AFTER)
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            emitted |= self.drop_conn(id);
        }
        for c in self.conns.values_mut() {
            if !c.ping_sent && now.duration_since(c.last_rx) >= PING_AFTER {
                c.send(&Msg::Ping);
                c.ping_sent = true;
            }
        }
        if self.enabled {
            let peers = self.static_peers.clone();
            for addr in peers {
                self.connect_static(addr);
            }
        }
        emitted
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
                for c in self.conns.values_mut().filter(|c| c.has_open(&note)) {
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
                    note: note.clone(),
                    author: self.replica.me().id(),
                    seq,
                    t_sent_us: now_us(),
                    data,
                };
                self.broadcast(&note, &m);
                false
            }
            Job::Cursor { note, x, y } => {
                self.broadcast(&note.clone(), &Msg::Cursor { note, x, y });
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
            Job::Share { note, title, key } => {
                let changed_key = self
                    .shares
                    .get(&note)
                    .map(|s| s.key != key)
                    .unwrap_or(false);
                self.shares.insert(note.clone(), Share { title, key });
                if changed_key {
                    // Nowe haslo: kto mial otwarte, musi otworzyc od nowa.
                    self.close_everywhere(&note);
                }
                self.announce_shares();
                false
            }
            Job::Unshare(note) => {
                if self.shares.remove(&note).is_some() {
                    self.close_everywhere(&note);
                    self.announce_shares();
                }
                false
            }
            Job::Open { note, key } => {
                self.wants.insert(note, key);
                let ids: Vec<u64> = self.conns.keys().copied().collect();
                for id in ids {
                    self.try_open(id);
                }
                false
            }
            Job::Close(note) => {
                self.wants.remove(&note);
                self.close_everywhere(&note);
                false
            }
            Job::Peers(list) => {
                self.static_peers = list;
                if self.enabled {
                    let peers = self.static_peers.clone();
                    for addr in peers {
                        self.connect_static(addr);
                    }
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

    /// Do kazdego peera, ktory ma te notatke otwarta.
    fn broadcast(&self, note: &str, m: &Msg) {
        if self.conns.values().any(|c| c.has_open(note)) {
            let bytes = m.encode();
            for c in self.conns.values().filter(|c| c.has_open(note)) {
                let _ = c.tx.send(bytes.clone());
            }
        }
    }

    fn shared_list(&self) -> Vec<SharedNote> {
        self.shares
            .iter()
            .map(|(note, s)| SharedNote {
                note: note.clone(),
                title: s.title.clone(),
                protected: s.key.is_some(),
            })
            .collect()
    }

    fn announce_shares(&self) {
        let m = Msg::Shared(self.shared_list());
        for c in self.conns.values().filter(|c| c.ready()) {
            c.send(&m);
        }
    }

    /// Zamyka sesje notatki na wszystkich polaczeniach (w obie strony).
    fn close_everywhere(&mut self, note: &str) {
        for c in self.conns.values_mut() {
            if c.open.remove(note) {
                c.send(&Msg::Close {
                    note: note.to_string(),
                });
            }
        }
    }

    /// Prosi peera o kazda z jego notatek, ktora chcemy, a nie mamy otwartej.
    /// Chroniona bez klucza czeka - aplikacja musi najpierw dostac haslo.
    fn try_open(&mut self, id: u64) {
        let Some(c) = self.conns.get(&id) else {
            return;
        };
        if !c.ready() {
            return;
        }
        let mut reqs = Vec::new();
        for offer in &c.offers {
            if c.open.contains(&offer.note) {
                continue;
            }
            let Some(key) = self.wants.get(&offer.note) else {
                continue;
            };
            let proof: Proof = match (offer.protected, key) {
                (false, _) => [0u8; 32],
                (true, Some(k)) => share::proof(k, &offer.note, c.nonce_theirs, c.nonce_mine),
                (true, None) => continue,
            };
            reqs.push(Msg::Open {
                note: offer.note.clone(),
                proof,
            });
        }
        for m in reqs {
            c.send(&m);
        }
    }

    /// Notatka weszla w sesje na polaczeniu: od tej chwili plynie, a obie
    /// strony wymieniaja stan (`Summary`), zeby dosypac brakujace operacje.
    fn open_on(&mut self, id: u64, note: &str) {
        let summary = Msg::Summary {
            note: note.to_string(),
            haves: self.replica.summary_note(note),
        };
        if let Some(c) = self.conns.get_mut(&id) {
            c.open.insert(note.to_string());
            c.send(&summary);
        }
    }

    /// Nowy strumien (przyjety albo nawiazany): watki I/O i `Hello`.
    fn attach(&mut self, stream: TcpStream, addr: Option<SocketAddr>) {
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
        let nonce = random_u64();
        let c = Conn {
            instance: None,
            author_dir: String::new(),
            author: AuthorId(0),
            nonce_mine: nonce,
            nonce_theirs: 0,
            tx: wtx,
            stream,
            addr,
            offers: Vec::new(),
            open: BTreeSet::new(),
            knows: BTreeMap::new(),
            last_rx: Instant::now(),
            ping_sent: false,
        };
        c.send(&Msg::Hello {
            version: PROTO_VERSION,
            author_dir: self.replica.me().dir_name(),
            author: self.replica.me().id(),
            instance: self.instance,
            nonce,
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
        let Some(c) = self.conns.get_mut(&id) else {
            return false;
        };
        c.last_rx = Instant::now();
        c.ping_sent = false;
        match m {
            Msg::Hello {
                version,
                author_dir,
                author,
                instance,
                nonce,
            } => {
                let dup = self
                    .conns
                    .iter()
                    .any(|(cid, c)| *cid != id && c.instance == Some(instance));
                // Ten sam autor na dwoch wezlach = dwa pisarze jednego pliku po
                // merge'u gita; odmawiamy, zanim cokolwiek poplynie.
                let same_author = author_dir == self.replica.me().dir_name();
                if version != PROTO_VERSION || instance == self.instance || dup || same_author {
                    if let Some(c) = self.conns.get(&id) {
                        c.send(&Msg::Bye);
                        if let Some(addr) = c.addr {
                            // Staly peer juz polaczony z drugiej strony -
                            // zapamietaj, zeby nie pukac co tick.
                            self.static_instance.insert(addr, instance);
                        }
                    }
                    self.conns.remove(&id);
                    return same_author
                        && self.emit(Event::Error(format!(
                            "live: {author_dir} on another device has the same author name"
                        )));
                }
                let shared = Msg::Shared(self.shared_list());
                let c = self.conns.get_mut(&id).expect("conn");
                c.instance = Some(instance);
                c.author_dir = author_dir.clone();
                c.author = author;
                c.nonce_theirs = nonce;
                c.send(&shared);
                if let Some(addr) = c.addr {
                    self.static_instance.insert(addr, instance);
                    self.connecting.remove(&addr);
                }
                self.emit(Event::Peer {
                    instance,
                    author_dir,
                    connected: true,
                })
            }
            Msg::Shared(list) => {
                let Some(c) = self.conns.get_mut(&id) else {
                    return false;
                };
                if !c.ready() {
                    return false;
                }
                // Co znikneło z listy, nie jest juz w sesji.
                c.open
                    .retain(|n| list.iter().any(|s| &s.note == n) || self.shares.contains_key(n));
                c.offers = list.clone();
                let (instance, author_dir) = (c.instance.unwrap_or(0), c.author_dir.clone());
                self.try_open(id);
                self.emit(Event::Shared {
                    instance,
                    author_dir,
                    notes: list,
                })
            }
            Msg::Open { note, proof } => {
                let Some(c) = self.conns.get(&id) else {
                    return false;
                };
                if !c.ready() {
                    return false;
                }
                let ok = match self.shares.get(&note) {
                    None => false,
                    Some(Share { key: None, .. }) => true,
                    Some(Share { key: Some(k), .. }) => {
                        proof == share::proof(k, &note, c.nonce_mine, c.nonce_theirs)
                    }
                };
                c.send(&Msg::Opened {
                    note: note.clone(),
                    ok,
                });
                if ok {
                    self.open_on(id, &note);
                }
                false
            }
            Msg::Opened { note, ok } => {
                let Some(c) = self.conns.get(&id) else {
                    return false;
                };
                let (instance, author_dir) = (c.instance.unwrap_or(0), c.author_dir.clone());
                if ok && self.wants.contains_key(&note) {
                    self.open_on(id, &note);
                }
                self.emit(Event::Opened {
                    instance,
                    author_dir,
                    note,
                    ok,
                })
            }
            Msg::Close { note } => {
                if let Some(c) = self.conns.get_mut(&id) {
                    c.open.remove(&note);
                }
                false
            }
            Msg::Summary { note, haves } => {
                let Some(c) = self.conns.get_mut(&id) else {
                    return false;
                };
                if !c.has_open(&note) {
                    return false;
                }
                for h in haves.into_iter().filter(|h| h.note == note) {
                    let k = c.knows.entry((h.note, h.author_dir)).or_insert(0);
                    *k = (*k).max(h.last);
                }
                self.push_missing(id, &[note]);
                false
            }
            Msg::Ops {
                note,
                author_dir,
                author,
                records,
            } => {
                if !self.conns.get(&id).is_some_and(|c| c.has_open(&note)) {
                    return false;
                }
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
                // Dalej do peerow, ktorzy maja te notatke otwarta, a tego nie
                // maja (trzeci wezel bez bezposredniego polaczenia z autorem).
                // Konczy sie, bo kazdy wezel przekazuje tylko to, co bylo dla
                // niego nowe.
                for (cid, c) in self.conns.iter_mut() {
                    if *cid == id || !c.has_open(&note) {
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
            } => {
                if !self.conns.get(&id).is_some_and(|c| c.has_open(&note)) {
                    return false;
                }
                self.emit(Event::Wet {
                    note,
                    author,
                    seq,
                    latency_us: now_us() as i64 - t_sent_us as i64,
                    data,
                })
            }
            Msg::Cursor { note, x, y } => {
                let Some(c) = self.conns.get(&id) else {
                    return false;
                };
                if !c.has_open(&note) {
                    return false;
                }
                let author = c.author;
                self.emit(Event::Cursor { note, author, x, y })
            }
            Msg::Ping => {
                if let Some(c) = self.conns.get(&id) {
                    c.send(&Msg::Pong);
                }
                false
            }
            Msg::Pong => false,
            Msg::Bye => self.drop_conn(id),
        }
    }

    /// Wysyla peerowi wszystko, czego wg naszej tabeli nie ma - w notatkach
    /// otwartych na tym polaczeniu (`only` zaweza dodatkowo).
    fn push_missing(&mut self, id: u64, only: &[String]) {
        let Some(c) = self.conns.get(&id) else {
            return;
        };
        if !c.ready() || c.open.is_empty() {
            return;
        }
        let notes: Vec<String> = c
            .open
            .iter()
            .filter(|n| only.is_empty() || only.contains(n))
            .cloned()
            .collect();
        if notes.is_empty() {
            return;
        }
        let msgs = self.replica.missing_for(&c.knows, &notes);
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
        if !self.enabled || b.instance == self.instance {
            return;
        }
        if self.conns.values().any(|c| c.instance == Some(b.instance)) {
            return;
        }
        // Laczy ten z mniejszym id; drugi czeka na `accept`.
        if self.instance > b.instance {
            return;
        }
        let addr = SocketAddr::new(src.ip(), b.port);
        if let Some(t) = self.connecting.get(&addr) {
            if t.elapsed() < CONNECT_RETRY {
                return;
            }
        }
        self.connecting.insert(addr, Instant::now());
        self.spawn_connect(addr);
    }

    /// Staly adres: polacz, jesli nie ma polaczenia z ta instancja i minal
    /// odstep od ostatniej proby.
    fn connect_static(&mut self, addr: SocketAddr) {
        if self.conns.values().any(|c| c.addr == Some(addr)) {
            return;
        }
        if let Some(inst) = self.static_instance.get(&addr) {
            if self.conns.values().any(|c| c.instance == Some(*inst)) {
                return;
            }
        }
        if let Some(t) = self.connecting.get(&addr) {
            if t.elapsed() < CONNECT_RETRY {
                return;
            }
        }
        self.connecting.insert(addr, Instant::now());
        self.spawn_connect(addr);
    }

    fn spawn_connect(&self, addr: SocketAddr) {
        let tx = self.tx.clone();
        let _ = thread::Builder::new()
            .name("live-connect".into())
            .spawn(move || {
                let cmd = match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                    Ok(s) => Cmd::Stream(s, Some(addr)),
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

/// Losowy identyfikator (instancja, nonce): czas, pid, adres na stosie
/// i licznik wywolan przez FNV.
fn random_u64() -> u64 {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let marker = 0u8;
    let addr = &marker as *const u8 as u64;
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in [t, std::process::id() as u64, addr, n] {
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
        let names: Vec<&str> = got
            .iter()
            .map(|e| match e {
                Event::Peer { .. } => "Peer",
                Event::Shared { .. } => "Shared",
                Event::Opened { ok: true, .. } => "Opened(ok)",
                Event::Opened { ok: false, .. } => "Opened(fail)",
                Event::Ops { .. } => "Ops",
                Event::Wet { .. } => "Wet",
                Event::Cursor { .. } => "Cursor",
                Event::Error(_) => "Error",
            })
            .collect();
        panic!("brak zdarzenia po 10 s; odebrano {names:?}");
    }

    fn start(root: &std::path::Path, who: &AuthorName) -> (Node, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let n = Node::start(
            root,
            who,
            Box::new(move || {
                w.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .unwrap();
        (n, wakes)
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

        let (a, wakes_a) = start(&ra, &adi);
        let (b, wakes_b) = start(&rb, &kuba);
        // A udostepnia notatke z haslem, zanim B sie pojawi.
        a.send(Job::Share {
            note: note.clone(),
            title: "plan".into(),
            key: Some(share::key_from_password("sezam")),
        });
        // Bez czekania na multicast: A jest u B stalym peerem (sciezka Tailscale).
        b.send(Job::Peers(vec![SocketAddr::from(([127, 0, 0, 1], a.port))]));

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

        // B widzi oferte A: tytul i klodke. (Po instancji: inne testy w tym
        // procesie tez rozglaszaja sie multicastem i moga sie tu podlaczyc.)
        let from_a =
            |e: &Event| matches!(e, Event::Shared { instance, .. } if *instance == a.instance);
        let evs = wait_for(&b, &wakes_b, from_a);
        let Some(Event::Shared { notes, .. }) = evs.iter().find(|e| from_a(e)) else {
            panic!()
        };
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "plan");
        assert!(notes[0].protected);

        // Zle haslo: odmowa, nic nie plynie.
        b.send(Job::Open {
            note: note.clone(),
            key: Some(share::key_from_password("zle")),
        });
        let evs = wait_for(&b, &wakes_b, |e| matches!(e, Event::Opened { .. }));
        assert!(evs
            .iter()
            .any(|e| matches!(e, Event::Opened { ok: false, .. })));
        assert!(evs.iter().all(|e| !matches!(e, Event::Ops { .. })));

        // Dobre haslo: B po Summary dostaje kreske A z dysku (dolaczenie w trakcie).
        b.send(Job::Open {
            note: note.clone(),
            key: Some(share::key_from_password("sezam")),
        });
        let evs = wait_for(&b, &wakes_b, |e| matches!(e, Event::Ops { .. }));
        assert!(evs
            .iter()
            .any(|e| matches!(e, Event::Opened { ok: true, .. })));
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

        // Notatka nieudostepniona nie plynie: druga notatka A z operacja.
        let other = space_a.create_note().unwrap();
        let op_o = doc_a.add_stroke(stroke(4.0));
        {
            let (mut s, _) = crate::NoteStore::open(&space_a, &other, &adi).unwrap();
            s.append(&op_o).unwrap();
            s.sync().unwrap();
        }
        a.send(Job::Local {
            note: other.clone(),
            ops: vec![op_o],
        });
        // Cofniecie udostepnienia: B dostaje pusta liste, potem juz nic.
        a.send(Job::Unshare(note.clone()));
        let evs = wait_for(
            &b,
            &wakes_b,
            |e| matches!(e, Event::Shared { instance, notes, .. } if *instance == a.instance && notes.is_empty()),
        );
        assert!(evs.iter().all(|e| !matches!(e, Event::Ops { .. })));
        let op2 = doc_a.add_stroke(stroke(5.0));
        a.send(Job::Local {
            note: note.clone(),
            ops: vec![op2],
        });
        thread::sleep(Duration::from_millis(300));
        assert!(b.poll().iter().all(|e| !matches!(e, Event::Ops { .. })));

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

    /// Klient, ktory po `Hello` milczy: heartbeat ma go zrzucic w ~DEAD_AFTER,
    /// a wczesniej dostac `Ping`.
    #[test]
    fn heartbeat_zrzuca_milczacego_peera() {
        let ra = tmp("hb");
        let adi = AuthorName::new("adi", "laptop");
        let (a, wakes_a) = start(&ra, &adi);

        let mut s = TcpStream::connect(("127.0.0.1", a.port)).unwrap();
        s.write_all(
            &Msg::Hello {
                version: PROTO_VERSION,
                author_dir: "ghost@box".into(),
                author: AuthorId(3),
                instance: 77,
                nonce: 1,
            }
            .encode(),
        )
        .unwrap();
        wait_for(&a, &wakes_a, |e| {
            matches!(
                e,
                Event::Peer {
                    connected: true,
                    ..
                }
            )
        });

        // Czytamy wszystko, co przysle wezel, az zamknie polaczenie.
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let t0 = Instant::now();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match s.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let closed_after = t0.elapsed();
        let mut msgs = Vec::new();
        let mut pos = 0;
        while let Frame::Msg(m, n) = next_frame(&buf[pos..]) {
            msgs.push(m);
            pos += n;
        }
        assert!(msgs.iter().any(|m| matches!(m, Msg::Ping)), "{msgs:?}");
        assert!(
            closed_after >= PING_AFTER && closed_after < DEAD_AFTER + TICK * 2,
            "zamkniete po {closed_after:?}"
        );
        let evs = wait_for(&a, &wakes_a, |e| {
            matches!(
                e,
                Event::Peer {
                    connected: false,
                    ..
                }
            )
        });
        assert!(!evs.is_empty());

        drop(a);
        let _ = std::fs::remove_dir_all(ra.parent().unwrap());
    }

    #[test]
    fn beacon_roundtrip() {
        let b = Beacon {
            instance: 5,
            port: 1234,
            author_dir: "adi@laptop".into(),
        };
        let d = Beacon::decode(&b.encode()).unwrap();
        assert_eq!(
            (d.instance, d.port, d.author_dir.as_str()),
            (5, 1234, "adi@laptop")
        );
        assert!(Beacon::decode(b"xx").is_none());
    }
}

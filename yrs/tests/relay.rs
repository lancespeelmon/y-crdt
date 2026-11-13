//! A reference relay server, kept honest by the tests below it.
//!
//! It shows how the pieces a yrs server needs fit together, with nothing but the standard
//! library (a real server would put the same actor behind WebSockets):
//!
//! - one actor thread per document, which owns its `Doc` and `Awareness`, so no locks are shared
//!   between connections;
//! - the y-protocols sync and awareness messages, through `MessageReader`;
//! - limits on message size and on the blocks one update may decode into;
//! - awareness timeouts (`Awareness::check_timeouts`, driven by a tick) and removal of a closed
//!   connection's awareness states (`Awareness::remove_states`);
//! - eviction: when the last connection closes, the document is persisted and its actor stops,
//!   and the next connection loads it again.
//!
//! It needs the `sync` feature (on by default), which makes documents `Send`.
#![cfg(all(feature = "sync", feature = "protocol"))]

mod relay {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use yrs::encoding::read::Cursor;
    use yrs::protocol::{Awareness, Message, MessageReader, SyncMessage};
    use yrs::updates::decoder::{Decode, DecoderV1};
    use yrs::updates::encoder::Encode;
    use yrs::{ClientID, Doc, ReadTxn, StateVector, Transact, Update};

    /// Where evicted documents go, and come back from.
    pub trait Persistence: Send + Sync + 'static {
        fn load(&self, doc: &str) -> Option<Vec<u8>>;
        fn store(&self, doc: &str, state: Vec<u8>);
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Limits {
        /// Bigger messages close the connection.
        pub max_message_bytes: usize,
        /// Updates that decode into more blocks close the connection.
        pub max_update_blocks: usize,
    }

    /// What a connection receives: a message for the client, or the reason it was closed.
    #[derive(Debug, PartialEq)]
    pub enum Outgoing {
        Message(Vec<u8>),
        Closed(String),
    }

    enum Command {
        Join(u64, Sender<Outgoing>),
        Message(u64, Vec<u8>),
        Leave(u64),
        /// Expire silent awareness states, as of the actor's clock.
        Tick,
        /// Replies once everything sent before it is processed.
        Flush(Sender<()>),
    }

    type Registry = Arc<Mutex<HashMap<String, (Sender<Command>, JoinHandle<()>)>>>;

    pub struct Relay {
        docs: Registry,
        persistence: Arc<dyn Persistence>,
        limits: Limits,
        clock: Arc<AtomicU64>,
        next_conn: AtomicU64,
    }

    pub struct Connection {
        id: u64,
        commands: Sender<Command>,
        pub incoming: Receiver<Outgoing>,
    }

    impl Connection {
        pub fn send(&self, bytes: Vec<u8>) {
            let _ = self.commands.send(Command::Message(self.id, bytes));
        }

        pub fn close(self) {
            let _ = self.commands.send(Command::Leave(self.id));
        }
    }

    impl Relay {
        /// `clock` is the awareness clock in milliseconds, which the tests move by hand.
        pub fn new(
            persistence: Arc<dyn Persistence>,
            limits: Limits,
            clock: Arc<AtomicU64>,
        ) -> Self {
            Relay {
                docs: Arc::default(),
                persistence,
                limits,
                clock,
                next_conn: AtomicU64::new(1),
            }
        }

        pub fn connect(&self, doc: &str) -> Connection {
            let id = self.next_conn.fetch_add(1, Ordering::SeqCst);
            let (out, incoming) = channel();
            let mut docs = self.docs.lock().unwrap();
            // An actor that stopped (evicted) since its entry was read is replaced.
            let alive = docs
                .get(doc)
                .is_some_and(|(tx, _)| tx.send(Command::Join(id, out.clone())).is_ok());
            if !alive {
                let commands = self.spawn(doc);
                commands.0.send(Command::Join(id, out)).unwrap();
                docs.insert(doc.to_string(), commands);
            }
            let commands = docs[doc].0.clone();
            Connection {
                id,
                commands,
                incoming,
            }
        }

        /// Expires silent awareness states in every live document.
        pub fn tick(&self) {
            for (tx, _) in self.docs.lock().unwrap().values() {
                let _ = tx.send(Command::Tick);
            }
        }

        /// Waits until every document has processed what was sent to it.
        pub fn flush(&self) {
            let docs: Vec<_> = self
                .docs
                .lock()
                .unwrap()
                .values()
                .map(|(tx, _)| tx.clone())
                .collect();
            for tx in docs {
                let (done, wait) = channel();
                if tx.send(Command::Flush(done)).is_ok() {
                    let _ = wait.recv();
                }
            }
        }

        /// Whether a document is loaded (its actor is running).
        pub fn is_loaded(&self, doc: &str) -> bool {
            self.docs.lock().unwrap().contains_key(doc)
        }

        fn spawn(&self, name: &str) -> (Sender<Command>, JoinHandle<()>) {
            let (tx, rx) = channel();
            let doc = Doc::new();
            if let Some(state) = self.persistence.load(name) {
                let update = Update::decode_v1(&state).expect("stored state decodes");
                doc.transact_mut().apply_update(update).unwrap();
            }
            let clock = self.clock.clone();
            let awareness = Awareness::with_clock(doc, move || clock.load(Ordering::SeqCst));
            let actor = DocActor {
                name: name.to_string(),
                awareness,
                conns: HashMap::new(),
                persistence: self.persistence.clone(),
                limits: self.limits,
                registry: self.docs.clone(),
            };
            (tx, std::thread::spawn(move || actor.run(rx)))
        }
    }

    struct Conn {
        out: Sender<Outgoing>,
        /// The awareness clients this connection announced, removed when it closes.
        clients: HashSet<ClientID>,
    }

    struct DocActor {
        name: String,
        awareness: Awareness,
        conns: HashMap<u64, Conn>,
        persistence: Arc<dyn Persistence>,
        limits: Limits,
        registry: Registry,
    }

    impl DocActor {
        fn run(mut self, commands: Receiver<Command>) {
            while let Ok(command) = commands.recv() {
                match command {
                    Command::Join(id, out) => self.join(id, out),
                    Command::Message(id, bytes) => {
                        if let Err(reason) = self.receive(id, &bytes) {
                            self.close(id, Some(reason));
                        }
                    }
                    Command::Leave(id) => self.close(id, None),
                    Command::Tick => {
                        let check = self.awareness.check_timeouts();
                        if !check.removed.is_empty() {
                            self.broadcast_awareness(check.removed, None);
                        }
                    }
                    Command::Flush(done) => {
                        let _ = done.send(());
                    }
                }
                if self.conns.is_empty() && self.evict(&commands) {
                    return;
                }
            }
        }

        /// Persists the document and leaves the registry, unless a connection arrived meanwhile.
        /// `connect` sends `Join` while holding the registry lock, so with the lock held here
        /// every join is either already queued (and handled) or will find this actor gone.
        fn evict(&mut self, commands: &Receiver<Command>) -> bool {
            let registry = self.registry.clone();
            let mut docs = registry.lock().unwrap();
            while let Ok(command) = commands.try_recv() {
                match command {
                    Command::Join(id, out) => self.join(id, out),
                    Command::Flush(done) => {
                        let _ = done.send(());
                    }
                    // Nothing else concerns a document without connections.
                    Command::Message(..) | Command::Leave(_) | Command::Tick => {}
                }
            }
            if !self.conns.is_empty() {
                return false;
            }
            let state = self
                .awareness
                .doc()
                .transact()
                .encode_state_as_update_v1(&StateVector::default());
            self.persistence.store(&self.name, state);
            // Dropping our own handle detaches this thread, which is about to end.
            docs.remove(&self.name);
            true
        }

        fn join(&mut self, id: u64, out: Sender<Outgoing>) {
            // y-protocols' opening: our state vector, then every known awareness state.
            let sv = self.awareness.doc().transact().state_vector();
            let _ = out.send(Outgoing::Message(
                Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1(),
            ));
            if let Ok(update) = self.awareness.update() {
                if !update.clients.is_empty() {
                    let _ = out.send(Outgoing::Message(Message::Awareness(update).encode_v1()));
                }
            }
            self.conns.insert(
                id,
                Conn {
                    out,
                    clients: HashSet::new(),
                },
            );
        }

        fn receive(&mut self, id: u64, bytes: &[u8]) -> Result<(), String> {
            if bytes.len() > self.limits.max_message_bytes {
                return Err(format!("message of {} bytes is too large", bytes.len()));
            }
            let mut decoder = DecoderV1::new(Cursor::new(bytes));
            for message in MessageReader::new(&mut decoder) {
                match message.map_err(|e| e.to_string())? {
                    Message::Sync(SyncMessage::SyncStep1(sv)) => {
                        let diff = self
                            .awareness
                            .doc()
                            .transact()
                            .encode_state_as_update_v1(&sv);
                        self.send_to(id, Message::Sync(SyncMessage::SyncStep2(diff)).encode_v1());
                    }
                    Message::Sync(SyncMessage::SyncStep2(update))
                    | Message::Sync(SyncMessage::Update(update)) => {
                        self.apply(&update)?;
                        let forward = Message::Sync(SyncMessage::Update(update)).encode_v1();
                        self.send_to_others(id, forward);
                    }
                    Message::Awareness(update) => {
                        let summary = self
                            .awareness
                            .apply_update_summary(update)
                            .map_err(|e| e.to_string())?;
                        if let Some(summary) = summary {
                            let conn = self.conns.get_mut(&id).unwrap();
                            conn.clients.extend(summary.added.iter().copied());
                            let changed = summary.all_changes();
                            self.broadcast_awareness(changed, Some(id));
                        }
                    }
                    Message::AwarenessQuery => {
                        if let Ok(update) = self.awareness.update() {
                            self.send_to(id, Message::Awareness(update).encode_v1());
                        }
                    }
                    Message::Auth(_) | Message::Custom(_, _) => {}
                }
            }
            Ok(())
        }

        fn apply(&mut self, update: &[u8]) -> Result<(), String> {
            // A v1 block takes at least a byte, so the size limit bounds v1 updates already;
            // decoding v1 as the protocol sends it, and checking the count anyway, keeps the
            // limit explicit for servers that also accept v2 (`decode_v2_with_block_limit`).
            let update = Update::decode_v1(update).map_err(|e| e.to_string())?;
            let blocks: usize = update
                .insertions(true)
                .iter()
                .map(|(_, ranges)| ranges.iter().count())
                .sum();
            if blocks > self.limits.max_update_blocks {
                return Err(format!("update of {} block ranges is too large", blocks));
            }
            self.awareness
                .doc()
                .transact_mut()
                .apply_update(update)
                .map_err(|e| e.to_string())
        }

        fn close(&mut self, id: u64, reason: Option<String>) {
            if let Some(conn) = self.conns.remove(&id) {
                if let Some(reason) = reason {
                    let _ = conn.out.send(Outgoing::Closed(reason));
                }
                let removed = self.awareness.remove_states(conn.clients, None::<&str>);
                if !removed.is_empty() {
                    self.broadcast_awareness(removed, None);
                }
            }
        }

        fn broadcast_awareness(&self, clients: Vec<ClientID>, except: Option<u64>) {
            if let Ok(update) = self.awareness.update_with_clients(clients) {
                let bytes = Message::Awareness(update).encode_v1();
                for (&id, conn) in &self.conns {
                    if Some(id) != except {
                        let _ = conn.out.send(Outgoing::Message(bytes.clone()));
                    }
                }
            }
        }

        fn send_to(&self, id: u64, bytes: Vec<u8>) {
            if let Some(conn) = self.conns.get(&id) {
                let _ = conn.out.send(Outgoing::Message(bytes));
            }
        }

        fn send_to_others(&self, id: u64, bytes: Vec<u8>) {
            for (&other, conn) in &self.conns {
                if other != id {
                    let _ = conn.out.send(Outgoing::Message(bytes.clone()));
                }
            }
        }
    }
}

use relay::{Connection, Limits, Outgoing, Persistence, Relay};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use yrs::protocol::{Awareness, DefaultProtocol, Protocol};
use yrs::updates::encoder::{Encode, Encoder};
use yrs::{GetString, ReadTxn, Text, Transact};

#[derive(Default)]
struct MemoryStore(Mutex<HashMap<String, Vec<u8>>>);

impl Persistence for MemoryStore {
    fn load(&self, doc: &str) -> Option<Vec<u8>> {
        self.0.lock().unwrap().get(doc).cloned()
    }
    fn store(&self, doc: &str, state: Vec<u8>) {
        self.0.lock().unwrap().insert(doc.to_string(), state);
    }
}

/// A y-protocols client, as y-websocket would be.
struct Client {
    awareness: Awareness,
    conn: Connection,
    closed: Option<String>,
}

impl Client {
    fn join(relay: &Relay, doc: &str, client_id: u64) -> Client {
        let conn = relay.connect(doc);
        let awareness = Awareness::new(yrs::Doc::with_client_id(client_id));
        let mut start = yrs::updates::encoder::EncoderV1::new();
        DefaultProtocol.start(&awareness, &mut start).unwrap();
        conn.send(start.to_vec());
        Client {
            awareness,
            conn,
            closed: None,
        }
    }

    /// Handles everything the relay has sent, answering as the protocol says.
    fn pump(&mut self) {
        while let Ok(outgoing) = self.conn.incoming.try_recv() {
            match outgoing {
                Outgoing::Message(bytes) => {
                    let replies = DefaultProtocol.handle(&mut self.awareness, &bytes).unwrap();
                    for reply in replies {
                        self.conn.send(reply.encode_v1());
                    }
                }
                Outgoing::Closed(reason) => self.closed = Some(reason),
            }
        }
    }

    fn type_text(&mut self, at: u32, text: &str) {
        let doc = self.awareness.doc();
        let shared = doc.get_or_insert_text("text");
        let before = doc.transact().state_vector();
        shared.insert(&mut doc.transact_mut(), at, text);
        let update = doc.transact().encode_state_as_update_v1(&before);
        self.conn.send(
            yrs::protocol::Message::Sync(yrs::protocol::SyncMessage::Update(update)).encode_v1(),
        );
    }

    fn set_presence(&mut self, state: serde_json::Value) {
        self.awareness.set_local_state(state).unwrap();
        let update = self.awareness.update().unwrap();
        self.conn
            .send(yrs::protocol::Message::Awareness(update).encode_v1());
    }

    fn text(&self) -> String {
        let doc = self.awareness.doc();
        let text = doc.get_or_insert_text("text");
        let s = text.get_string(&doc.transact());
        s
    }

    fn sees(&self, client: u64) -> bool {
        self.awareness
            .state::<serde_json::Value>(yrs::ClientID::new(client))
            .is_some()
    }
}

fn setup() -> (Relay, Arc<MemoryStore>, Arc<AtomicU64>) {
    let store = Arc::new(MemoryStore::default());
    let clock = Arc::new(AtomicU64::new(1_000));
    let limits = Limits {
        max_message_bytes: 4096,
        max_update_blocks: 64,
    };
    (
        Relay::new(store.clone(), limits, clock.clone()),
        store,
        clock,
    )
}

/// Lets messages travel until nothing more moves: the relay processes, then each client answers.
fn settle(relay: &Relay, clients: &mut [&mut Client]) {
    for _ in 0..4 {
        relay.flush();
        for c in clients.iter_mut() {
            c.pump();
        }
    }
    relay.flush();
}

#[test]
fn clients_converge_through_the_relay_and_see_each_other() {
    let (relay, _, _) = setup();
    let mut alice = Client::join(&relay, "doc", 1);
    let mut bob = Client::join(&relay, "doc", 2);
    settle(&relay, &mut [&mut alice, &mut bob]);

    alice.type_text(0, "hello");
    bob.set_presence(json!({"name": "bob"}));
    alice.set_presence(json!({"name": "alice"}));
    settle(&relay, &mut [&mut alice, &mut bob]);
    bob.type_text(5, " world");
    settle(&relay, &mut [&mut alice, &mut bob]);

    assert_eq!(alice.text(), "hello world");
    assert_eq!(bob.text(), "hello world");
    assert!(alice.sees(2) && bob.sees(1));

    // A late joiner gets the state and the presence of both.
    let mut carol = Client::join(&relay, "doc", 3);
    settle(&relay, &mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(carol.text(), "hello world");
    assert!(carol.sees(1) && carol.sees(2));
}

#[test]
fn oversized_messages_close_the_connection_without_touching_the_document() {
    let (relay, _, _) = setup();
    let mut alice = Client::join(&relay, "doc", 1);
    let mut bob = Client::join(&relay, "doc", 2);
    settle(&relay, &mut [&mut alice, &mut bob]);

    // Too many bytes.
    alice.type_text(0, &"x".repeat(5000));
    settle(&relay, &mut [&mut alice, &mut bob]);
    assert!(alice.closed.as_deref().unwrap().contains("too large"));
    assert_eq!(bob.text(), "");

    // Under the byte limit, but more block ranges than allowed.
    let mut eve = Client::join(&relay, "doc", 4);
    settle(&relay, &mut [&mut bob, &mut eve]);
    let update = fragmented_update();
    assert!(update.len() < 4096);
    eve.conn
        .send(yrs::protocol::Message::Sync(yrs::protocol::SyncMessage::Update(update)).encode_v1());
    settle(&relay, &mut [&mut bob, &mut eve]);
    assert!(eve.closed.as_deref().unwrap().contains("block ranges"));
    assert_eq!(bob.text(), "");
}

/// An update with many separate block ranges: one insert from each of 100 clients.
fn fragmented_update() -> Vec<u8> {
    let docs: Vec<_> = (100..200).map(|id| yrs::Doc::with_client_id(id)).collect();
    let updates: Vec<_> = docs
        .iter()
        .map(|d| {
            d.get_or_insert_text("text")
                .insert(&mut d.transact_mut(), 0, "z");
            yrs::ReadTxn::encode_state_as_update_v1(&d.transact(), &yrs::StateVector::default())
        })
        .collect();
    yrs::merge_updates_v1(&updates).unwrap()
}

#[test]
fn silent_and_departed_clients_disappear_from_awareness() {
    let (relay, _, clock) = setup();
    let mut alice = Client::join(&relay, "doc", 1);
    let mut bob = Client::join(&relay, "doc", 2);
    let mut carol = Client::join(&relay, "doc", 3);
    settle(&relay, &mut [&mut alice, &mut bob, &mut carol]);
    alice.set_presence(json!({"name": "alice"}));
    bob.set_presence(json!({"name": "bob"}));
    carol.set_presence(json!({"name": "carol"}));
    settle(&relay, &mut [&mut alice, &mut bob, &mut carol]);
    assert!(alice.sees(2) && alice.sees(3));

    // Bob closes: the relay removes his state and tells the others.
    bob.conn.close();
    settle(&relay, &mut [&mut alice, &mut carol]);
    assert!(!alice.sees(2) && !carol.sees(2));

    // Carol goes silent for the awareness timeout; Alice keeps renewing.
    clock.fetch_add(20_000, Ordering::SeqCst);
    alice.set_presence(json!({"name": "alice", "cursor": 1}));
    settle(&relay, &mut [&mut alice, &mut carol]);
    clock.fetch_add(15_000, Ordering::SeqCst);
    relay.tick();
    settle(&relay, &mut [&mut alice, &mut carol]);
    assert!(!alice.sees(3), "carol timed out");
    assert!(carol.sees(1), "alice did not");
}

#[test]
fn an_idle_document_is_persisted_evicted_and_reloaded() {
    let (relay, store, _) = setup();
    let mut alice = Client::join(&relay, "notes", 1);
    settle(&relay, &mut [&mut alice]);
    alice.type_text(0, "kept");
    settle(&relay, &mut [&mut alice]);

    alice.conn.close();
    relay.flush();
    // The actor stops after persisting; wait for it.
    for _ in 0..100 {
        if !relay.is_loaded("notes") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!relay.is_loaded("notes"));
    assert!(store.load("notes").is_some());

    let mut bob = Client::join(&relay, "notes", 2);
    settle(&relay, &mut [&mut bob]);
    assert_eq!(bob.text(), "kept");
}

#[test]
fn a_connection_racing_an_eviction_is_never_lost() {
    let (relay, _, _) = setup();
    for i in 0..200 {
        let first = Client::join(&relay, "race", 1);
        first.conn.close();
        // Joins while the actor may be evicting.
        let second = relay.connect("race");
        let opening = second
            .incoming
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("round {}: the join was lost", i));
        assert!(matches!(opening, Outgoing::Message(_)));
        second.close();
    }
}

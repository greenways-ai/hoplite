use std::collections::BTreeSet;
use std::env;
use std::fmt;

const SEEDS: &str = include_str!("fixtures/cosocket_lifecycle_seeds.txt");
const DEFAULT_STEPS: usize = 256;
const MAX_SOCKETS: usize = 8;
const POOL_CAPACITY: usize = 2;
const BACKLOG_CAPACITY: usize = 2;
const OPERATION_COUNT: u64 = 26;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SocketState {
    Allocated,
    Resolving,
    Connecting,
    Established,
    Backlog,
    IdlePool,
    Retired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transport {
    Tcp,
    Unix,
}

impl Transport {
    fn pool_index(self) -> usize {
        match self {
            Self::Tcp => 0,
            Self::Unix => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Direction {
    Connect,
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Resource {
    Buffer(u64, Direction),
    Cleanup(u64),
    Descriptor(u64),
    Promise(u64, Direction),
    Resolver(u64),
    Timer(u64, Direction),
    Backlog(u64),
    PoolEntry(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutcomeClass {
    Accepted,
    Ordinary,
    Programming,
    Authority,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Outcome {
    class: OutcomeClass,
    detail: &'static str,
}

impl Outcome {
    const ACCEPTED: Self = Self {
        class: OutcomeClass::Accepted,
        detail: "accepted",
    };

    const fn ordinary(detail: &'static str) -> Self {
        Self {
            class: OutcomeClass::Ordinary,
            detail,
        }
    }

    const fn programming(detail: &'static str) -> Self {
        Self {
            class: OutcomeClass::Programming,
            detail,
        }
    }

    const fn authority(detail: &'static str) -> Self {
        Self {
            class: OutcomeClass::Authority,
            detail,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Allocate(usize, Transport),
    ResolveStart(usize),
    ResolveOk(usize),
    ConnectStart(usize),
    ConnectOk(usize),
    BacklogStart(usize),
    BacklogAdmit(usize),
    ReadStart(usize),
    ReadPartial(usize),
    ReadComplete(usize),
    WriteStart(usize),
    WriteComplete(usize),
    ShutdownWrite(usize),
    TimeoutConnect(usize),
    TimeoutRead(usize),
    TimeoutWrite(usize),
    Cancel(usize),
    Close(usize),
    PeerClose(usize),
    Keepalive(usize),
    Checkout(usize),
    StaleCallback(usize),
    UseForeign(usize),
    ClientAbort,
    WorkerReload,
}

impl fmt::Display for Operation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

#[derive(Clone, Debug)]
struct Socket {
    id: u64,
    transport: Transport,
    state: SocketState,
    read_pending: bool,
    write_pending: bool,
    send_shutdown: bool,
    generation: u64,
    reused: u32,
}

impl Socket {
    fn new(id: u64, transport: Transport) -> Self {
        Self {
            id,
            transport,
            state: SocketState::Allocated,
            read_pending: false,
            write_pending: false,
            send_shutdown: false,
            generation: 0,
            reused: 0,
        }
    }

    fn has_native_descriptor(&self) -> bool {
        matches!(
            self.state,
            SocketState::Connecting | SocketState::Established | SocketState::IdlePool
        ) || self.read_pending
            || self.write_pending
    }

    fn connect_pending(&self) -> bool {
        matches!(
            self.state,
            SocketState::Resolving | SocketState::Connecting | SocketState::Backlog
        )
    }
}

#[derive(Default)]
struct Model {
    sockets: Vec<Option<Socket>>,
    resources: BTreeSet<Resource>,
    next_id: u64,
    idle_by_transport: [usize; 2],
    backlog_count: usize,
}

impl Model {
    fn socket(&self, slot: usize) -> Option<&Socket> {
        self.sockets.get(slot).and_then(Option::as_ref)
    }

    fn socket_mut(&mut self, slot: usize) -> Option<&mut Socket> {
        self.sockets.get_mut(slot).and_then(Option::as_mut)
    }

    fn allocate(&mut self, slot: usize, transport: Transport) -> Outcome {
        if slot >= MAX_SOCKETS {
            return Outcome::programming("invalid socket slot");
        }
        if self
            .socket(slot)
            .is_some_and(|socket| !matches!(socket.state, SocketState::Retired))
        {
            return Outcome::programming("socket slot is occupied");
        }
        self.next_id += 1;
        let socket = Socket::new(self.next_id, transport);
        self.resources.insert(Resource::Cleanup(socket.id));
        if slot >= self.sockets.len() {
            self.sockets.resize_with(slot + 1, || None);
        }
        self.sockets[slot] = Some(socket);
        Outcome::ACCEPTED
    }

    fn begin_connect(&mut self, slot: usize, resolving: bool) -> Outcome {
        let Some(socket) = self.socket(slot) else {
            return Outcome::programming("unknown socket");
        };
        if !matches!(socket.state, SocketState::Allocated) {
            return Outcome::programming("connect on non-new socket");
        }
        let id = socket.id;
        if resolving {
            self.socket_mut(slot).unwrap().state = SocketState::Resolving;
            self.resources.insert(Resource::Resolver(id));
        } else {
            self.socket_mut(slot).unwrap().state = SocketState::Connecting;
            self.resources.insert(Resource::Descriptor(id));
        }
        self.resources
            .insert(Resource::Promise(id, Direction::Connect));
        self.resources
            .insert(Resource::Timer(id, Direction::Connect));
        Outcome::ACCEPTED
    }

    fn resolve_ok(&mut self, slot: usize) -> Outcome {
        let Some(socket) = self.socket(slot) else {
            return Outcome::programming("unknown socket");
        };
        if !matches!(socket.state, SocketState::Resolving) {
            return Outcome::programming("resolver is not pending");
        }
        let id = socket.id;
        self.socket_mut(slot).unwrap().state = SocketState::Connecting;
        self.resources.remove(&Resource::Resolver(id));
        self.resources.insert(Resource::Descriptor(id));
        Outcome::ACCEPTED
    }

    fn connect_ok(&mut self, slot: usize) -> Outcome {
        let Some(socket) = self.socket(slot) else {
            return Outcome::programming("unknown socket");
        };
        if !matches!(socket.state, SocketState::Connecting) {
            return Outcome::programming("connect is not pending");
        }
        let id = socket.id;
        self.socket_mut(slot).unwrap().state = SocketState::Established;
        self.resources
            .remove(&Resource::Promise(id, Direction::Connect));
        self.resources
            .remove(&Resource::Timer(id, Direction::Connect));
        Outcome::ACCEPTED
    }

    fn begin_backlog(&mut self, slot: usize) -> Outcome {
        let Some(socket) = self.socket(slot) else {
            return Outcome::programming("unknown socket");
        };
        if !matches!(socket.state, SocketState::Allocated) {
            return Outcome::programming("backlog admission on non-new socket");
        }
        if self.backlog_count >= BACKLOG_CAPACITY {
            return Outcome::ordinary("pool backlog full");
        }
        let id = socket.id;
        self.socket_mut(slot).unwrap().state = SocketState::Backlog;
        self.backlog_count += 1;
        self.resources.insert(Resource::Backlog(id));
        self.resources
            .insert(Resource::Promise(id, Direction::Connect));
        self.resources
            .insert(Resource::Timer(id, Direction::Connect));
        Outcome::ACCEPTED
    }

    fn backlog_admit(&mut self, slot: usize) -> Outcome {
        let Some(socket) = self.socket(slot) else {
            return Outcome::programming("unknown socket");
        };
        if !matches!(socket.state, SocketState::Backlog) {
            return Outcome::programming("backlog waiter is not pending");
        }
        let id = socket.id;
        self.socket_mut(slot).unwrap().state = SocketState::Connecting;
        self.backlog_count -= 1;
        self.resources.remove(&Resource::Backlog(id));
        self.resources.insert(Resource::Descriptor(id));
        Outcome::ACCEPTED
    }

    fn begin_read(&mut self, slot: usize) -> Outcome {
        let Some(socket) = self.socket(slot) else {
            return Outcome::programming("unknown socket");
        };
        if socket.read_pending {
            return Outcome::ordinary("socket busy reading");
        }
        if !matches!(socket.state, SocketState::Established) {
            return Outcome::programming("read on non-established socket");
        }
        let id = socket.id;
        self.socket_mut(slot).unwrap().read_pending = true;
        self.resources
            .insert(Resource::Promise(id, Direction::Read));
        self.resources.insert(Resource::Timer(id, Direction::Read));
        self.resources.insert(Resource::Buffer(id, Direction::Read));
        Outcome::ACCEPTED
    }

    fn begin_write(&mut self, slot: usize) -> Outcome {
        let Some(socket) = self.socket(slot) else {
            return Outcome::programming("unknown socket");
        };
        if socket.write_pending {
            return Outcome::ordinary("socket busy writing");
        }
        if !matches!(socket.state, SocketState::Established) || socket.send_shutdown {
            return Outcome::programming("write on closed send direction");
        }
        let id = socket.id;
        self.socket_mut(slot).unwrap().write_pending = true;
        self.resources
            .insert(Resource::Promise(id, Direction::Write));
        self.resources.insert(Resource::Timer(id, Direction::Write));
        self.resources
            .insert(Resource::Buffer(id, Direction::Write));
        Outcome::ACCEPTED
    }

    fn shutdown_write(&mut self, slot: usize) -> Outcome {
        let Some(socket) = self.socket(slot) else {
            return Outcome::programming("unknown socket");
        };
        if !matches!(socket.state, SocketState::Established) || socket.write_pending {
            return Outcome::programming("send direction cannot be shut down");
        }
        self.socket_mut(slot).unwrap().send_shutdown = true;
        Outcome::ACCEPTED
    }

    fn complete_direction(&mut self, slot: usize, direction: Direction) -> Outcome {
        let Some(socket) = self.socket(slot) else {
            return Outcome::programming("unknown socket");
        };
        let pending = match direction {
            Direction::Read => socket.read_pending,
            Direction::Write => socket.write_pending,
            Direction::Connect => socket.connect_pending(),
        };
        if !pending {
            return Outcome::programming("operation is not pending");
        }
        let id = socket.id;
        match direction {
            Direction::Read => self.socket_mut(slot).unwrap().read_pending = false,
            Direction::Write => self.socket_mut(slot).unwrap().write_pending = false,
            Direction::Connect => return self.connect_ok(slot),
        }
        self.clear_direction_resources(id, direction);
        Outcome::ACCEPTED
    }

    fn clear_direction_resources(&mut self, id: u64, direction: Direction) {
        self.resources.remove(&Resource::Promise(id, direction));
        self.resources.remove(&Resource::Timer(id, direction));
        self.resources.remove(&Resource::Buffer(id, direction));
    }

    fn cancel(&mut self, slot: usize) -> Outcome {
        let Some(socket) = self.socket(slot).cloned() else {
            return Outcome::programming("unknown socket");
        };
        let id = socket.id;
        match socket.state {
            SocketState::Resolving => {
                self.socket_mut(slot).unwrap().state = SocketState::Allocated;
                self.resources.remove(&Resource::Resolver(id));
                self.clear_direction_resources(id, Direction::Connect);
            }
            SocketState::Connecting => {
                self.socket_mut(slot).unwrap().state = SocketState::Allocated;
                self.resources.remove(&Resource::Descriptor(id));
                self.clear_direction_resources(id, Direction::Connect);
            }
            SocketState::Backlog => {
                self.socket_mut(slot).unwrap().state = SocketState::Allocated;
                self.backlog_count -= 1;
                self.resources.remove(&Resource::Backlog(id));
                self.clear_direction_resources(id, Direction::Connect);
            }
            _ => {
                if socket.read_pending {
                    self.socket_mut(slot).unwrap().read_pending = false;
                    self.clear_direction_resources(id, Direction::Read);
                }
                if socket.write_pending {
                    self.socket_mut(slot).unwrap().write_pending = false;
                    self.clear_direction_resources(id, Direction::Write);
                }
            }
        }
        Outcome::ACCEPTED
    }

    fn close(&mut self, slot: usize, detail: &'static str, accepted: bool) -> Outcome {
        let Some(socket) = self.socket(slot).cloned() else {
            return Outcome::programming("unknown socket");
        };
        if matches!(socket.state, SocketState::Retired) {
            return Outcome::ordinary(detail);
        }
        self.retire(slot, &socket);
        if accepted {
            Outcome::ACCEPTED
        } else {
            Outcome::ordinary(detail)
        }
    }

    fn retire(&mut self, slot: usize, socket: &Socket) {
        let id = socket.id;
        if socket.read_pending {
            self.clear_direction_resources(id, Direction::Read);
        }
        if socket.write_pending {
            self.clear_direction_resources(id, Direction::Write);
        }
        if socket.connect_pending() {
            self.clear_direction_resources(id, Direction::Connect);
        }
        if matches!(socket.state, SocketState::Resolving) {
            self.resources.remove(&Resource::Resolver(id));
        }
        if matches!(socket.state, SocketState::Backlog) {
            self.backlog_count -= 1;
            self.resources.remove(&Resource::Backlog(id));
        }
        if matches!(socket.state, SocketState::IdlePool) {
            self.idle_by_transport[socket.transport.pool_index()] -= 1;
            self.resources.remove(&Resource::PoolEntry(id));
        }
        self.resources.remove(&Resource::Descriptor(id));
        self.resources.remove(&Resource::Cleanup(id));
        self.socket_mut(slot).unwrap().state = SocketState::Retired;
        self.socket_mut(slot).unwrap().read_pending = false;
        self.socket_mut(slot).unwrap().write_pending = false;
    }

    fn keepalive(&mut self, slot: usize) -> Outcome {
        let Some(socket) = self.socket(slot).cloned() else {
            return Outcome::programming("unknown socket");
        };
        if !matches!(socket.state, SocketState::Established)
            || socket.read_pending
            || socket.write_pending
        {
            return Outcome::programming("connection is not poolable");
        }
        let pool_index = socket.transport.pool_index();
        if self.idle_by_transport[pool_index] >= POOL_CAPACITY {
            self.retire(slot, &socket);
            return Outcome::ordinary("keepalive pool full");
        }
        self.socket_mut(slot).unwrap().state = SocketState::IdlePool;
        self.idle_by_transport[pool_index] += 1;
        self.resources.insert(Resource::PoolEntry(socket.id));
        Outcome::ACCEPTED
    }

    fn checkout(&mut self, slot: usize) -> Outcome {
        let Some(socket) = self.socket(slot).cloned() else {
            return Outcome::programming("unknown socket");
        };
        if !matches!(socket.state, SocketState::IdlePool) {
            return Outcome::programming("socket is not idle");
        }
        self.socket_mut(slot).unwrap().state = SocketState::Established;
        self.socket_mut(slot).unwrap().generation += 1;
        self.socket_mut(slot).unwrap().reused += 1;
        self.idle_by_transport[socket.transport.pool_index()] -= 1;
        self.resources.remove(&Resource::PoolEntry(socket.id));
        Outcome::ACCEPTED
    }

    fn worker_shutdown(&mut self) {
        for slot in 0..self.sockets.len() {
            if let Some(socket) = self.socket(slot).cloned() {
                if !matches!(socket.state, SocketState::Retired) {
                    self.retire(slot, &socket);
                }
            }
        }
    }

    fn apply(&mut self, operation: Operation) -> Outcome {
        match operation {
            Operation::Allocate(slot, transport) => self.allocate(slot, transport),
            Operation::ResolveStart(slot) => self.begin_connect(slot, true),
            Operation::ResolveOk(slot) => self.resolve_ok(slot),
            Operation::ConnectStart(slot) => self.begin_connect(slot, false),
            Operation::ConnectOk(slot) => self.connect_ok(slot),
            Operation::BacklogStart(slot) => self.begin_backlog(slot),
            Operation::BacklogAdmit(slot) => self.backlog_admit(slot),
            Operation::ReadStart(slot) => self.begin_read(slot),
            Operation::ReadPartial(slot) => {
                if self.socket(slot).is_some_and(|socket| socket.read_pending) {
                    Outcome::ACCEPTED
                } else {
                    Outcome::programming("read is not pending")
                }
            }
            Operation::ReadComplete(slot) => self.complete_direction(slot, Direction::Read),
            Operation::WriteStart(slot) => self.begin_write(slot),
            Operation::WriteComplete(slot) => self.complete_direction(slot, Direction::Write),
            Operation::ShutdownWrite(slot) => self.shutdown_write(slot),
            Operation::TimeoutConnect(slot) => self.cancel_direction(slot, Direction::Connect),
            Operation::TimeoutRead(slot) => self.cancel_direction(slot, Direction::Read),
            Operation::TimeoutWrite(slot) => self.cancel_direction(slot, Direction::Write),
            Operation::Cancel(slot) => self.cancel(slot),
            Operation::Close(slot) => self.close(slot, "closed", true),
            Operation::PeerClose(slot) => self.close(slot, "peer closed", false),
            Operation::Keepalive(slot) => self.keepalive(slot),
            Operation::Checkout(slot) => self.checkout(slot),
            Operation::StaleCallback(_) => Outcome::ordinary("stale callback ignored"),
            Operation::UseForeign(_) => Outcome::authority("foreign socket owner"),
            Operation::ClientAbort => {
                self.worker_shutdown();
                Outcome::ordinary("client aborted")
            }
            Operation::WorkerReload => {
                self.worker_shutdown();
                Outcome::ordinary("worker reloaded")
            }
        }
    }

    fn cancel_direction(&mut self, slot: usize, direction: Direction) -> Outcome {
        let Some(socket) = self.socket(slot) else {
            return Outcome::programming("unknown socket");
        };
        let pending = match direction {
            Direction::Read => socket.read_pending,
            Direction::Write => socket.write_pending,
            Direction::Connect => socket.connect_pending(),
        };
        if !pending {
            return Outcome::programming("operation is not pending");
        }
        let id = socket.id;
        match direction {
            Direction::Read => self.socket_mut(slot).unwrap().read_pending = false,
            Direction::Write => self.socket_mut(slot).unwrap().write_pending = false,
            Direction::Connect => return self.cancel(slot),
        }
        self.clear_direction_resources(id, direction);
        Outcome::ordinary("timeout")
    }

    fn expected_resources(&self) -> BTreeSet<Resource> {
        let mut expected = BTreeSet::new();
        for socket in self.sockets.iter().flatten() {
            if matches!(socket.state, SocketState::Retired) {
                continue;
            }
            expected.insert(Resource::Cleanup(socket.id));
            if socket.has_native_descriptor() {
                expected.insert(Resource::Descriptor(socket.id));
            }
            if socket.read_pending {
                expected.insert(Resource::Buffer(socket.id, Direction::Read));
                expected.insert(Resource::Promise(socket.id, Direction::Read));
                expected.insert(Resource::Timer(socket.id, Direction::Read));
            }
            if socket.write_pending {
                expected.insert(Resource::Buffer(socket.id, Direction::Write));
                expected.insert(Resource::Promise(socket.id, Direction::Write));
                expected.insert(Resource::Timer(socket.id, Direction::Write));
            }
            if socket.connect_pending() {
                expected.insert(Resource::Promise(socket.id, Direction::Connect));
                expected.insert(Resource::Timer(socket.id, Direction::Connect));
            }
            if matches!(socket.state, SocketState::Resolving) {
                expected.insert(Resource::Resolver(socket.id));
            }
            if matches!(socket.state, SocketState::Backlog) {
                expected.insert(Resource::Backlog(socket.id));
            }
            if matches!(socket.state, SocketState::IdlePool) {
                expected.insert(Resource::PoolEntry(socket.id));
            }
        }
        expected
    }

    fn assert_invariants(&self, seed: u64, trace: &[String]) {
        let expected = self.expected_resources();
        assert_eq!(
            self.resources,
            expected,
            "seed {seed:#x} lifecycle resource mismatch after:\n{}",
            trace.join("\n")
        );
        let ids = self
            .sockets
            .iter()
            .flatten()
            .filter(|socket| !matches!(socket.state, SocketState::Retired))
            .map(|socket| socket.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            ids.len(),
            self.resources
                .iter()
                .filter(|resource| matches!(resource, Resource::Cleanup(_)))
                .count(),
            "seed {seed:#x} has an unowned or multiply-owned cleanup resource"
        );
        assert!(
            self.idle_by_transport
                .iter()
                .all(|count| *count <= POOL_CAPACITY)
                && self.backlog_count <= BACKLOG_CAPACITY,
            "seed {seed:#x} exceeded bounded pool or backlog capacity"
        );
    }
}

struct Generator {
    state: u64,
}

impl Generator {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.state = value;
        value
    }

    fn operation(&mut self) -> Operation {
        let slot = (self.next() as usize) % MAX_SOCKETS;
        match self.next() % OPERATION_COUNT {
            0 => Operation::Allocate(slot, Transport::Tcp),
            1 => Operation::Allocate(slot, Transport::Unix),
            2 => Operation::ResolveStart(slot),
            3 => Operation::ResolveOk(slot),
            4 => Operation::ConnectStart(slot),
            5 => Operation::ConnectOk(slot),
            6 => Operation::BacklogStart(slot),
            7 => Operation::BacklogAdmit(slot),
            8 => Operation::ReadStart(slot),
            9 => Operation::ReadPartial(slot),
            10 => Operation::ReadComplete(slot),
            11 => Operation::WriteStart(slot),
            12 => Operation::WriteComplete(slot),
            13 => Operation::ShutdownWrite(slot),
            14 => Operation::TimeoutConnect(slot),
            15 => Operation::TimeoutRead(slot),
            16 => Operation::TimeoutWrite(slot),
            17 => Operation::Cancel(slot),
            18 => Operation::Close(slot),
            19 => Operation::PeerClose(slot),
            20 => Operation::Keepalive(slot),
            21 => Operation::Checkout(slot),
            22 => Operation::StaleCallback(slot),
            23 => Operation::UseForeign(slot),
            24 => Operation::ClientAbort,
            25 => Operation::WorkerReload,
            _ => unreachable!("operation count is exhaustive"),
        }
    }
}

fn parse_seeds() -> Vec<(String, u64)> {
    SEEDS
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (name, value) = line.split_once('=')?;
            let value = value.trim().strip_prefix("0x").unwrap_or(value.trim());
            Some((
                name.trim().to_owned(),
                u64::from_str_radix(value, 16).expect("valid lifecycle seed"),
            ))
        })
        .collect()
}

fn run_trace(seed: u64, operations: impl IntoIterator<Item = Operation>) -> Vec<Outcome> {
    let mut model = Model::default();
    let mut trace = Vec::new();
    let mut outcomes = Vec::new();
    for operation in operations {
        let outcome = model.apply(operation);
        trace.push(format!("{operation} => {:?}", outcome));
        model.assert_invariants(seed, &trace);
        outcomes.push(outcome);
    }
    model.worker_shutdown();
    model.assert_invariants(seed, &trace);
    assert!(
        model.resources.is_empty(),
        "seed {seed:#x} left resources after teardown:\n{}",
        trace.join("\n")
    );
    outcomes
}

fn run_seed(seed: u64, steps: usize) {
    assert_ne!(seed, 0, "lifecycle seeds must be non-zero");
    let mut generator = Generator::new(seed);
    run_trace(seed, (0..steps).map(|_| generator.operation()));
}

#[test]
fn checked_in_lifecycle_seed_corpus_is_reproducible() {
    let seeds = parse_seeds();
    assert!(!seeds.is_empty(), "the lifecycle corpus must not be empty");
    for (name, seed) in seeds {
        run_seed(seed, DEFAULT_STEPS);
        assert!(name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        }));
    }
}

#[test]
fn required_lifecycle_regressions_preserve_cleanup_invariants() {
    let cases = [
        (
            "connect-cancel",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::ConnectStart(0),
                Operation::Cancel(0),
            ],
        ),
        (
            "connect-timeout",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::ConnectStart(0),
                Operation::TimeoutConnect(0),
            ],
        ),
        (
            "connect-close",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::ConnectStart(0),
                Operation::Close(0),
            ],
        ),
        (
            "resolve-client-abort",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::ResolveStart(0),
                Operation::ClientAbort,
            ],
        ),
        (
            "unix-peer-close",
            vec![
                Operation::Allocate(0, Transport::Unix),
                Operation::ConnectStart(0),
                Operation::ConnectOk(0),
                Operation::WriteStart(0),
                Operation::PeerClose(0),
            ],
        ),
        (
            "receive-timeout-partial",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::ConnectStart(0),
                Operation::ConnectOk(0),
                Operation::ReadStart(0),
                Operation::ReadPartial(0),
                Operation::TimeoutRead(0),
            ],
        ),
        (
            "simultaneous-read-write-close",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::ConnectStart(0),
                Operation::ConnectOk(0),
                Operation::ReadStart(0),
                Operation::WriteStart(0),
                Operation::Close(0),
            ],
        ),
        (
            "keepalive-checkout",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::ConnectStart(0),
                Operation::ConnectOk(0),
                Operation::Keepalive(0),
                Operation::Checkout(0),
            ],
        ),
        (
            "pool-transfer-rejects-pending-read",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::ConnectStart(0),
                Operation::ConnectOk(0),
                Operation::ReadStart(0),
                Operation::Keepalive(0),
                Operation::Close(0),
            ],
        ),
        (
            "pool-key-isolation",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::ConnectStart(0),
                Operation::ConnectOk(0),
                Operation::Keepalive(0),
                Operation::Allocate(1, Transport::Unix),
                Operation::ConnectStart(1),
                Operation::ConnectOk(1),
                Operation::Keepalive(1),
                Operation::Checkout(0),
                Operation::Checkout(1),
            ],
        ),
        (
            "half-close-with-live-read",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::ConnectStart(0),
                Operation::ConnectOk(0),
                Operation::ReadStart(0),
                Operation::ShutdownWrite(0),
                Operation::ReadComplete(0),
                Operation::Close(0),
            ],
        ),
        (
            "backlog-cancel",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::BacklogStart(0),
                Operation::Cancel(0),
            ],
        ),
        (
            "backlog-timeout",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::BacklogStart(0),
                Operation::TimeoutConnect(0),
            ],
        ),
        (
            "backlog-worker-reload",
            vec![
                Operation::Allocate(0, Transport::Tcp),
                Operation::BacklogStart(0),
                Operation::WorkerReload,
            ],
        ),
    ];
    for (index, (name, operations)) in cases.into_iter().enumerate() {
        let outcomes = run_trace(index as u64 + 1, operations);
        assert!(
            outcomes
                .iter()
                .any(|outcome| outcome.class == OutcomeClass::Accepted),
            "{name} did not exercise an accepted transition"
        );
    }
}

#[test]
fn directional_busy_and_authority_results_are_distinct() {
    let outcomes = run_trace(
        0xD1CE,
        [
            Operation::Allocate(0, Transport::Tcp),
            Operation::ConnectStart(0),
            Operation::ConnectOk(0),
            Operation::ReadStart(0),
            Operation::ReadStart(0),
            Operation::WriteStart(0),
            Operation::WriteStart(0),
            Operation::UseForeign(0),
            Operation::StaleCallback(0),
        ],
    );
    assert!(outcomes.contains(&Outcome::ordinary("socket busy reading")));
    assert!(outcomes.contains(&Outcome::ordinary("socket busy writing")));
    assert!(outcomes.contains(&Outcome::authority("foreign socket owner")));
    assert!(outcomes.contains(&Outcome::ordinary("stale callback ignored")));
}

#[test]
#[ignore = "larger deterministic stress run; use HOPLITE_COSOCKET_STRESS_SEEDS"]
fn opt_in_lifecycle_stress_corpus() {
    let seeds = env::var("HOPLITE_COSOCKET_STRESS_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1024);
    let steps = env::var("HOPLITE_COSOCKET_STRESS_STEPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4096);
    for index in 0..seeds {
        run_seed(0x9E37_79B9_7F4A_7C15u64.wrapping_add(index), steps);
    }
}

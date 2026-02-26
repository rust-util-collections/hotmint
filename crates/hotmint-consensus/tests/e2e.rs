/// End-to-end tests covering validator lifecycle, epoch transitions, and equivocation detection.
///
/// Each test uses in-process channel networking with a shared routing table so that
/// dynamic validator join/leave can be simulated without a real P2P layer.
///
/// IMPORTANT: all Application impls used here are deterministic — every node
/// calling end_block at the same height returns the same result.  This mirrors
/// real-world state machines (all replicas must agree).
use ruc::*;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use hotmint_consensus::application::Application;
use hotmint_consensus::engine::ConsensusEngine;
use hotmint_consensus::network::NetworkSink;
use hotmint_consensus::state::ConsensusState;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_crypto::Ed25519Signer;
use hotmint_types::context::BlockContext;
use hotmint_types::evidence::EquivocationProof;
use hotmint_types::validator_update::{EndBlockResponse, ValidatorUpdate};
use hotmint_types::*;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Dynamic network: shared routing table so joining nodes can be wired in
// ---------------------------------------------------------------------------

type MsgSender = mpsc::UnboundedSender<(ValidatorId, ConsensusMessage)>;

/// Shared mutable routing table used by all nodes in a test.
#[derive(Clone)]
struct SharedRouting(Arc<Mutex<HashMap<ValidatorId, MsgSender>>>);

impl SharedRouting {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    fn register(&self, id: ValidatorId, tx: MsgSender) {
        self.0.lock().unwrap().insert(id, tx);
    }

    fn deregister(&self, id: ValidatorId) {
        self.0.lock().unwrap().remove(&id);
    }
}

struct DynamicNetwork {
    self_id: ValidatorId,
    routing: SharedRouting,
}

impl NetworkSink for DynamicNetwork {
    fn broadcast(&self, msg: ConsensusMessage) {
        let table = self.routing.0.lock().unwrap();
        for (id, tx) in table.iter() {
            if *id != self.self_id {
                let _ = tx.send((self.self_id, msg.clone()));
            }
        }
    }

    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage) {
        let table = self.routing.0.lock().unwrap();
        if let Some(tx) = table.get(&target) {
            let _ = tx.send((self.self_id, msg));
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: spawn a single validator node
// ---------------------------------------------------------------------------

fn spawn_node(
    vid: ValidatorId,
    signer: Ed25519Signer,
    validator_set: ValidatorSet,
    routing: &SharedRouting,
    app: impl Application + 'static,
) -> (
    mpsc::UnboundedSender<(ValidatorId, ConsensusMessage)>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    routing.register(vid, tx.clone());

    let network = DynamicNetwork {
        self_id: vid,
        routing: routing.clone(),
    };
    let store = Arc::new(RwLock::new(
        Box::new(MemoryBlockStore::new()) as Box<dyn hotmint_consensus::store::BlockStore>
    ));
    let state = ConsensusState::new(vid, validator_set);
    let engine = ConsensusEngine::new(
        state,
        store,
        Box::new(network),
        Box::new(app),
        Box::new(signer),
        rx,
        None,
    );

    let handle = tokio::spawn(async move { engine.run().await });
    (tx, handle)
}

// ---------------------------------------------------------------------------
// Helper: build an initial N-validator set and signers
// ---------------------------------------------------------------------------

fn make_validator_set(n: u64) -> (ValidatorSet, Vec<Ed25519Signer>) {
    let signers: Vec<Ed25519Signer> = (0..n)
        .map(|i| Ed25519Signer::generate(ValidatorId(i)))
        .collect();
    let infos: Vec<ValidatorInfo> = signers
        .iter()
        .map(|s| ValidatorInfo {
            id: s.validator_id(),
            public_key: s.public_key(),
            power: 1,
        })
        .collect();
    (ValidatorSet::new(infos), signers)
}

// ---------------------------------------------------------------------------
// TEST 1: Basic 4-node consensus — sanity baseline with DynamicNetwork
// ---------------------------------------------------------------------------

struct CountingApp {
    commits: Arc<AtomicU64>,
}

impl Application for CountingApp {
    fn on_commit(&self, _block: &Block, _ctx: &BlockContext) -> Result<()> {
        self.commits.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[tokio::test]
async fn test_basic_four_node_dynamic_network() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();

    let (vs, signers) = make_validator_set(4);
    let routing = SharedRouting::new();
    let mut counters = Vec::new();
    let mut handles = Vec::new();

    for signer in signers {
        let vid = signer.validator_id();
        let commits = Arc::new(AtomicU64::new(0));
        counters.push(commits.clone());
        let app = CountingApp { commits };
        let (_, h) = spawn_node(vid, signer, vs.clone(), &routing, app);
        handles.push(h);
    }

    tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;

    for (i, c) in counters.iter().enumerate() {
        let n = c.load(Ordering::Relaxed);
        assert!(n >= 1, "validator {i} committed {n} blocks, expected >= 1");
    }

    let max = counters
        .iter()
        .map(|c| c.load(Ordering::Relaxed))
        .max()
        .unwrap();
    assert!(max >= 2, "expected >= 2 blocks committed, got {max}");

    for h in handles {
        h.abort();
    }
}

// ---------------------------------------------------------------------------
// TEST 2: Validator JOIN — 3-node start, 4th joins via epoch transition
//
// All nodes emit the same ValidatorUpdate at height 2 (deterministic).
// A shared signal fires in on_commit so the test harness can wait.
// ---------------------------------------------------------------------------

struct ValidatorJoinApp {
    commits: Arc<AtomicU64>,
    join_at_height: u64,
    new_validator: ValidatorUpdate,
    /// Observation signal — set by on_commit when height >= join_at_height
    join_observed: Arc<AtomicBool>,
}

impl Application for ValidatorJoinApp {
    fn on_commit(&self, _block: &Block, ctx: &BlockContext) -> Result<()> {
        self.commits.fetch_add(1, Ordering::Relaxed);
        if ctx.height.as_u64() >= self.join_at_height {
            self.join_observed.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn execute_block(&self, _txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        // Deterministic: every node at this height returns the same update
        if ctx.height.as_u64() == self.join_at_height {
            return Ok(EndBlockResponse {
                validator_updates: vec![self.new_validator.clone()],
                ..Default::default()
            });
        }
        Ok(EndBlockResponse::default())
    }
}

#[tokio::test]
async fn test_validator_join() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();

    let (vs3, signers3) = make_validator_set(3);
    let routing = SharedRouting::new();

    let new_signer = Ed25519Signer::generate(ValidatorId(3));
    let new_validator_update = ValidatorUpdate {
        id: new_signer.validator_id(),
        public_key: new_signer.public_key(),
        power: 1,
    };

    let join_observed = Arc::new(AtomicBool::new(false));
    let mut initial_commits: Vec<Arc<AtomicU64>> = Vec::new();
    let mut handles = Vec::new();

    for signer in signers3 {
        let vid = signer.validator_id();
        let commits = Arc::new(AtomicU64::new(0));
        initial_commits.push(commits.clone());
        let app = ValidatorJoinApp {
            commits,
            join_at_height: 2,
            new_validator: new_validator_update.clone(),
            join_observed: join_observed.clone(),
        };
        let (_, h) = spawn_node(vid, signer, vs3.clone(), &routing, app);
        handles.push(h);
    }

    // Wait for the join height to be committed
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !join_observed.load(Ordering::SeqCst) {
        if std::time::Instant::now() > deadline {
            panic!("join height was not committed within 15s");
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    // Wire up the 4th validator with the expanded set
    let new_vid = new_signer.validator_id();
    let mut vs4_validators = vs3.validators().to_vec();
    vs4_validators.push(ValidatorInfo {
        id: new_vid,
        public_key: new_signer.public_key(),
        power: 1,
    });
    let vs4 = ValidatorSet::new(vs4_validators);

    let new_commits = Arc::new(AtomicU64::new(0));
    let new_app = CountingApp {
        commits: new_commits.clone(),
    };
    let (_, new_handle) = spawn_node(new_vid, new_signer, vs4, &routing, new_app);
    handles.push(new_handle);

    tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;

    // All 3 initial validators should have committed multiple blocks
    for (i, c) in initial_commits.iter().enumerate() {
        let n = c.load(Ordering::Relaxed);
        assert!(
            n >= 2,
            "initial validator {i} committed {n} blocks, expected >= 2"
        );
    }

    assert!(join_observed.load(Ordering::SeqCst));

    for h in handles {
        h.abort();
    }
}

// ---------------------------------------------------------------------------
// TEST 3: Validator LEAVE — 4-node start, one removed via epoch transition
// ---------------------------------------------------------------------------

struct ValidatorLeaveApp {
    commits: Arc<AtomicU64>,
    leave_at_height: u64,
    validator_to_remove: ValidatorId,
    /// Observation signal — set by on_commit when height >= leave_at_height
    leave_observed: Arc<AtomicBool>,
}

impl Application for ValidatorLeaveApp {
    fn on_commit(&self, _block: &Block, ctx: &BlockContext) -> Result<()> {
        self.commits.fetch_add(1, Ordering::Relaxed);
        if ctx.height.as_u64() >= self.leave_at_height {
            self.leave_observed.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn execute_block(&self, _txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        // Deterministic: every node at this height returns the same update
        if ctx.height.as_u64() == self.leave_at_height {
            return Ok(EndBlockResponse {
                validator_updates: vec![ValidatorUpdate {
                    id: self.validator_to_remove,
                    public_key: hotmint_types::crypto::PublicKey(vec![0]),
                    power: 0,
                }],
                ..Default::default()
            });
        }
        Ok(EndBlockResponse::default())
    }
}

#[tokio::test]
async fn test_validator_leave() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();

    let (vs, signers) = make_validator_set(4);
    let routing = SharedRouting::new();

    let leave_observed = Arc::new(AtomicBool::new(false));
    let to_remove = ValidatorId(3);

    let mut counters: Vec<Arc<AtomicU64>> = Vec::new();
    let mut handles = Vec::new();

    for signer in signers {
        let vid = signer.validator_id();
        let commits = Arc::new(AtomicU64::new(0));
        counters.push(commits.clone());
        let app = ValidatorLeaveApp {
            commits,
            leave_at_height: 2,
            validator_to_remove: to_remove,
            leave_observed: leave_observed.clone(),
        };
        let (_, h) = spawn_node(vid, signer, vs.clone(), &routing, app);
        handles.push(h);
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !leave_observed.load(Ordering::SeqCst) {
        if std::time::Instant::now() > deadline {
            panic!("leave height was not committed within 15s");
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    // Disconnect V3 from routing and abort its task
    routing.deregister(to_remove);
    handles[3].abort();

    // Remaining 3 validators (quorum = ceil(2*3/3) = 2) should continue
    tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;

    for (i, c) in counters.iter().enumerate().take(3) {
        let n = c.load(Ordering::Relaxed);
        assert!(
            n >= 2,
            "validator {i} committed {n} blocks after leave, expected >= 2"
        );
    }

    for h in handles {
        h.abort();
    }
}

// ---------------------------------------------------------------------------
// TEST 4: Equivocation detection — inject a double-vote, verify callback fires
//
// Pre-load equivocating votes into the leader's channel before starting
// engines, guaranteeing the leader processes them in view 1.
// ---------------------------------------------------------------------------

struct EquivocationWatchApp {
    commits: Arc<AtomicU64>,
    equivocations: Arc<AtomicU64>,
}

impl Application for EquivocationWatchApp {
    fn on_commit(&self, _block: &Block, _ctx: &BlockContext) -> Result<()> {
        self.commits.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_evidence(&self, _proof: &EquivocationProof) -> Result<()> {
        self.equivocations.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[tokio::test]
async fn test_equivocation_detected_via_injected_votes() {
    use hotmint_types::vote::VoteType;

    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();

    let (vs, signers) = make_validator_set(4);
    let routing = SharedRouting::new();

    // Separate signer for injection (Ed25519Signer doesn't impl Clone).
    // Equivocation detection is structural, exact signature bytes don't matter.
    let injector_signer = Ed25519Signer::generate(ValidatorId(0));
    let view = ViewNumber(1);
    let hash_a = BlockHash([0xAAu8; 32]);
    let hash_b = BlockHash([0xBBu8; 32]);

    let bytes_a = Vote::signing_bytes(view, &hash_a, VoteType::Vote);
    let bytes_b = Vote::signing_bytes(view, &hash_b, VoteType::Vote);
    let vote_a = Vote {
        block_hash: hash_a,
        view,
        validator: injector_signer.validator_id(),
        signature: injector_signer.sign(&bytes_a),
        vote_type: VoteType::Vote,
    };
    let vote_b = Vote {
        block_hash: hash_b,
        view,
        validator: injector_signer.validator_id(),
        signature: injector_signer.sign(&bytes_b),
        vote_type: VoteType::Vote,
    };

    // Create channels manually so we can pre-load V1's queue
    let mut node_txs: Vec<mpsc::UnboundedSender<(ValidatorId, ConsensusMessage)>> = Vec::new();
    let mut node_rxs: Vec<Option<mpsc::UnboundedReceiver<(ValidatorId, ConsensusMessage)>>> =
        Vec::new();
    for i in 0..4u64 {
        let (tx, rx) = mpsc::unbounded_channel();
        routing.register(ValidatorId(i), tx.clone());
        node_txs.push(tx);
        node_rxs.push(Some(rx));
    }

    // Pre-load equivocating votes into V1's queue (V1 is leader of view 1)
    let _ = node_txs[1].send((ValidatorId(0), ConsensusMessage::VoteMsg(vote_a)));
    let _ = node_txs[1].send((ValidatorId(0), ConsensusMessage::VoteMsg(vote_b)));

    let mut equivocation_counters: Vec<Arc<AtomicU64>> = Vec::new();
    let mut handles = Vec::new();

    for (signer, rx) in signers.into_iter().zip(node_rxs.iter_mut()) {
        let vid = signer.validator_id();
        let equivocations = Arc::new(AtomicU64::new(0));
        equivocation_counters.push(equivocations.clone());
        let app = EquivocationWatchApp {
            commits: Arc::new(AtomicU64::new(0)),
            equivocations,
        };

        let network = DynamicNetwork {
            self_id: vid,
            routing: routing.clone(),
        };
        let store = Arc::new(RwLock::new(
            Box::new(MemoryBlockStore::new()) as Box<dyn hotmint_consensus::store::BlockStore>
        ));
        let state = ConsensusState::new(vid, vs.clone());
        let engine = ConsensusEngine::new(
            state,
            store,
            Box::new(network),
            Box::new(app),
            Box::new(signer),
            rx.take().unwrap(),
            None,
        );
        handles.push(tokio::spawn(async move { engine.run().await }));
    }

    // Give the engines time to process view 1
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // V1 (leader of view 1) should have detected the equivocation
    let detected = equivocation_counters[1].load(Ordering::Relaxed);
    assert!(
        detected >= 1,
        "V1 should have detected >=1 equivocation, detected {detected}"
    );

    for h in handles {
        h.abort();
    }
}

// ---------------------------------------------------------------------------
// TEST 5: Epoch transition correctness — verify epoch number and validator
//         set are correctly updated at the application level
// ---------------------------------------------------------------------------

struct EpochTrackingApp {
    commits: Arc<AtomicU64>,
    epoch_at_commit: Arc<Mutex<Vec<u64>>>,
    transition_at_height: u64,
    new_validator: ValidatorUpdate,
}

impl Application for EpochTrackingApp {
    fn on_commit(&self, _block: &Block, ctx: &BlockContext) -> Result<()> {
        self.commits.fetch_add(1, Ordering::Relaxed);
        let mut epochs = self.epoch_at_commit.lock().unwrap();
        epochs.push(ctx.epoch.as_u64());
        Ok(())
    }

    fn execute_block(&self, _txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        // Deterministic: every node at this height returns the same update
        if ctx.height.as_u64() == self.transition_at_height {
            return Ok(EndBlockResponse {
                validator_updates: vec![self.new_validator.clone()],
                ..Default::default()
            });
        }
        Ok(EndBlockResponse::default())
    }
}

#[tokio::test]
async fn test_epoch_transition_increments_correctly() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();

    let (vs, signers) = make_validator_set(3);
    let routing = SharedRouting::new();

    // Use a power change on an existing validator (V0: power 1 → 2).
    // This triggers an epoch without changing the validator count, avoiding
    // liveness issues from adding a non-running validator.
    let new_validator = ValidatorUpdate {
        id: ValidatorId(0),
        public_key: signers[0].public_key(),
        power: 2,
    };

    let mut epoch_logs: Vec<Arc<Mutex<Vec<u64>>>> = Vec::new();
    let mut handles = Vec::new();

    for signer in signers {
        let vid = signer.validator_id();
        let epoch_at_commit = Arc::new(Mutex::new(Vec::new()));
        epoch_logs.push(epoch_at_commit.clone());
        let app = EpochTrackingApp {
            commits: Arc::new(AtomicU64::new(0)),
            epoch_at_commit,
            transition_at_height: 2,
            new_validator: new_validator.clone(),
        };
        let (_, h) = spawn_node(vid, signer, vs.clone(), &routing, app);
        handles.push(h);
    }

    // Wait for enough commits to span the epoch transition
    tokio::time::sleep(tokio::time::Duration::from_secs(12)).await;

    // Verify at least one node saw epoch 0 commits followed by epoch 1 commits
    let mut any_saw_transition = false;
    for (i, log) in epoch_logs.iter().enumerate() {
        let epochs = log.lock().unwrap().clone();
        if epochs.len() >= 2 {
            let has_epoch_0 = epochs.contains(&0);
            let has_epoch_1 = epochs.contains(&1);
            if has_epoch_0 && has_epoch_1 {
                any_saw_transition = true;
            }
        }
        // Epochs must never decrease
        for w in epochs.windows(2) {
            assert!(
                w[1] >= w[0],
                "validator {i}: epoch went backwards: {} -> {}",
                w[0],
                w[1]
            );
        }
    }

    assert!(
        any_saw_transition,
        "no validator observed epoch 0 -> epoch 1 transition in commit log"
    );

    for h in handles {
        h.abort();
    }
}

// ---------------------------------------------------------------------------
// TEST 6: Consecutive epoch transitions — multiple validator set changes
// ---------------------------------------------------------------------------

struct MultiEpochApp {
    commits: Arc<AtomicU64>,
    epoch_transitions: Arc<AtomicU64>,
    dummy_key: hotmint_types::crypto::PublicKey,
}

impl Application for MultiEpochApp {
    fn on_commit(&self, _block: &Block, _ctx: &BlockContext) -> Result<()> {
        self.commits.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn execute_block(&self, _txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        // Deterministic: trigger epoch transition every 3 blocks
        if ctx.height.as_u64() > 0 && ctx.height.as_u64().is_multiple_of(3) {
            self.epoch_transitions.fetch_add(1, Ordering::Relaxed);
            let new_power = 1 + (ctx.height.as_u64() / 3);
            return Ok(EndBlockResponse {
                validator_updates: vec![ValidatorUpdate {
                    id: ValidatorId(0),
                    public_key: self.dummy_key.clone(),
                    power: new_power,
                }],
                ..Default::default()
            });
        }
        Ok(EndBlockResponse::default())
    }
}

#[tokio::test]
async fn test_multiple_consecutive_epoch_transitions() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();

    let (vs, signers) = make_validator_set(4);
    let routing = SharedRouting::new();

    let dummy_key = signers[0].public_key();

    let mut transition_counters: Vec<Arc<AtomicU64>> = Vec::new();
    let mut commit_counters: Vec<Arc<AtomicU64>> = Vec::new();
    let mut handles = Vec::new();

    for signer in signers {
        let vid = signer.validator_id();
        let epoch_transitions = Arc::new(AtomicU64::new(0));
        let commits = Arc::new(AtomicU64::new(0));
        transition_counters.push(epoch_transitions.clone());
        commit_counters.push(commits.clone());
        let app = MultiEpochApp {
            commits,
            epoch_transitions,
            dummy_key: dummy_key.clone(),
        };
        let (_, h) = spawn_node(vid, signer, vs.clone(), &routing, app);
        handles.push(h);
    }

    tokio::time::sleep(tokio::time::Duration::from_secs(20)).await;

    for (i, c) in commit_counters.iter().enumerate() {
        let n = c.load(Ordering::Relaxed);
        assert!(n >= 3, "validator {i} committed {n} blocks, expected >= 3");
    }

    let max_transitions = transition_counters
        .iter()
        .map(|c| c.load(Ordering::Relaxed))
        .max()
        .unwrap();
    assert!(
        max_transitions >= 2,
        "expected >= 2 epoch transitions, got {max_transitions}"
    );

    for h in handles {
        h.abort();
    }
}

// ---------------------------------------------------------------------------
// TEST 7: Node crash — one node crashes mid-run, others continue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_node_crash_does_not_halt_consensus() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();

    let (vs, signers) = make_validator_set(4);
    let routing = SharedRouting::new();

    let mut counters: Vec<Arc<AtomicU64>> = Vec::new();
    let mut handles = Vec::new();

    for signer in signers {
        let vid = signer.validator_id();
        let commits = Arc::new(AtomicU64::new(0));
        counters.push(commits.clone());
        let app = CountingApp {
            commits: counters.last().unwrap().clone(),
        };
        let (_, h) = spawn_node(vid, signer, vs.clone(), &routing, app);
        handles.push(h);
    }

    // Let consensus run, then crash V2
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    handles[2].abort();
    routing.deregister(ValidatorId(2));

    // With 3/4 nodes (quorum = 3), consensus must continue
    tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;

    for i in [0usize, 1, 3] {
        let n = counters[i].load(Ordering::Relaxed);
        assert!(
            n >= 2,
            "validator {i} committed {n} blocks after crash, expected >= 2"
        );
    }

    for h in handles {
        h.abort();
    }
}

// ---------------------------------------------------------------------------
// TEST 8: Validator set shrinks to 2 nodes — remove 2 validators from 4-node
//         set via epoch transition, verify the remaining 2 continue.
// ---------------------------------------------------------------------------

struct TwoRemoveApp {
    commits: Arc<AtomicU64>,
    remove_observed: Arc<AtomicBool>,
}

impl Application for TwoRemoveApp {
    fn on_commit(&self, _b: &Block, ctx: &BlockContext) -> Result<()> {
        self.commits.fetch_add(1, Ordering::Relaxed);
        if ctx.height.as_u64() >= 2 {
            self.remove_observed.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
    fn execute_block(&self, _txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        // Deterministic: every node at height 2 returns the same update
        if ctx.height.as_u64() == 2 {
            return Ok(EndBlockResponse {
                validator_updates: vec![
                    ValidatorUpdate {
                        id: ValidatorId(2),
                        public_key: hotmint_types::crypto::PublicKey(vec![2]),
                        power: 0,
                    },
                    ValidatorUpdate {
                        id: ValidatorId(3),
                        public_key: hotmint_types::crypto::PublicKey(vec![3]),
                        power: 0,
                    },
                ],
                ..Default::default()
            });
        }
        Ok(EndBlockResponse::default())
    }
}

#[tokio::test]
async fn test_validator_set_shrinks_to_two_nodes() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();

    let (vs, signers) = make_validator_set(4);
    let routing = SharedRouting::new();

    let remove_observed = Arc::new(AtomicBool::new(false));

    let mut counters: Vec<Arc<AtomicU64>> = Vec::new();
    let mut handles = Vec::new();

    for signer in signers {
        let vid = signer.validator_id();
        let commits = Arc::new(AtomicU64::new(0));
        counters.push(commits.clone());
        let app = TwoRemoveApp {
            commits,
            remove_observed: remove_observed.clone(),
        };
        let (_, h) = spawn_node(vid, signer, vs.clone(), &routing, app);
        handles.push(h);
    }

    // Wait for height 2 to be committed
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !remove_observed.load(Ordering::SeqCst) {
        if std::time::Instant::now() > deadline {
            panic!("remove height was not committed within 15s");
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    // Disconnect and abort V2 and V3
    routing.deregister(ValidatorId(2));
    routing.deregister(ValidatorId(3));
    handles[2].abort();
    handles[3].abort();

    // V0 and V1 now form a 2-node set (quorum = ceil(2*2/3) = 2)
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

    for (i, c) in counters.iter().enumerate().take(2) {
        let n = c.load(Ordering::Relaxed);
        assert!(
            n >= 2,
            "validator {i} committed {n} blocks in 2-node set, expected >= 2"
        );
    }

    for h in handles {
        h.abort();
    }
}

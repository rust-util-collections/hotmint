use criterion::{Criterion, black_box, criterion_group, criterion_main};
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_consensus::vote_collector::VoteCollector;
use hotmint_crypto::Ed25519Signer;
use hotmint_types::validator::{ValidatorId, ValidatorInfo, ValidatorSet};
use hotmint_types::view::ViewNumber;
use hotmint_types::vote::{Vote, VoteType};
use hotmint_types::{BlockHash, Signer};

fn make_env(n: u64) -> (ValidatorSet, Vec<Ed25519Signer>) {
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

fn bench_vote_collection_4(c: &mut Criterion) {
    let (vs, signers) = make_env(4);
    let hash = BlockHash([1u8; 32]);
    let view = ViewNumber(1);

    let votes: Vec<Vote> = signers
        .iter()
        .take(3)
        .map(|s| {
            let bytes = Vote::signing_bytes(view, &hash, VoteType::Vote);
            Vote {
                block_hash: hash,
                view,
                validator: s.validator_id(),
                signature: s.sign(&bytes),
                vote_type: VoteType::Vote,
            }
        })
        .collect();

    c.bench_function("vote_collect_3_of_4", |b| {
        b.iter(|| {
            let mut vc = VoteCollector::new();
            for vote in &votes {
                vc.add_vote(black_box(&vs), black_box(vote.clone()))
                    .unwrap();
            }
        })
    });
}

fn bench_vote_collection_100(c: &mut Criterion) {
    let (vs, signers) = make_env(100);
    let hash = BlockHash([2u8; 32]);
    let view = ViewNumber(1);

    let votes: Vec<Vote> = signers
        .iter()
        .take(67)
        .map(|s| {
            let bytes = Vote::signing_bytes(view, &hash, VoteType::Vote);
            Vote {
                block_hash: hash,
                view,
                validator: s.validator_id(),
                signature: s.sign(&bytes),
                vote_type: VoteType::Vote,
            }
        })
        .collect();

    c.bench_function("vote_collect_67_of_100", |b| {
        b.iter(|| {
            let mut vc = VoteCollector::new();
            for vote in &votes {
                vc.add_vote(black_box(&vs), black_box(vote.clone()))
                    .unwrap();
            }
        })
    });
}

fn bench_block_store(c: &mut Criterion) {
    use hotmint_consensus::store::BlockStore;
    use hotmint_types::{Block, Height};

    let mut store = MemoryBlockStore::new();
    let mut parent = BlockHash::GENESIS;
    let mut hashes = Vec::new();
    for i in 1..=1000u64 {
        let mut h = [0u8; 32];
        h[..8].copy_from_slice(&i.to_le_bytes());
        let hash = BlockHash(h);
        hashes.push(hash);
        let block = Block {
            height: Height(i),
            parent_hash: parent,
            view: ViewNumber(i),
            proposer: ValidatorId(0),
            payload: vec![0u8; 256],
            app_hash: BlockHash::GENESIS,
            hash,
        };
        parent = hash;
        store.put_block(block);
    }

    let lookup_hash = hashes[499];
    c.bench_function("block_store_get_by_hash", |b| {
        b.iter(|| store.get_block(black_box(&lookup_hash)))
    });

    c.bench_function("block_store_get_by_height", |b| {
        b.iter(|| store.get_block_by_height(black_box(Height(500))))
    });
}

criterion_group!(
    benches,
    bench_vote_collection_4,
    bench_vote_collection_100,
    bench_block_store
);
criterion_main!(benches);

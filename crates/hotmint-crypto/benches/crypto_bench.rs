use criterion::{Criterion, black_box, criterion_group, criterion_main};
use hotmint_crypto::{Ed25519Signer, Ed25519Verifier, compute_block_hash};
use hotmint_types::validator::{ValidatorId, ValidatorInfo, ValidatorSet};
use hotmint_types::{Block, BlockHash, Height, Signer, Verifier, ViewNumber};

fn make_signer() -> Ed25519Signer {
    Ed25519Signer::generate(ValidatorId(0))
}

fn make_block(payload_size: usize) -> Block {
    Block {
        height: Height(1),
        parent_hash: BlockHash::GENESIS,
        view: ViewNumber(1),
        proposer: ValidatorId(0),
        payload: vec![0u8; payload_size],
        hash: BlockHash::GENESIS,
    }
}

fn bench_sign(c: &mut Criterion) {
    let signer = make_signer();
    let msg = b"benchmark message for signing";

    c.bench_function("ed25519_sign", |b| b.iter(|| signer.sign(black_box(msg))));
}

fn bench_verify(c: &mut Criterion) {
    let signer = make_signer();
    let verifier = Ed25519Verifier;
    let msg = b"benchmark message for verification";
    let sig = signer.sign(msg);
    let pk = signer.public_key();

    c.bench_function("ed25519_verify", |b| {
        b.iter(|| verifier.verify(black_box(&pk), black_box(msg), black_box(&sig)))
    });
}

fn bench_hash_block(c: &mut Criterion) {
    let block = make_block(256);
    c.bench_function("blake3_hash_block_256b", |b| {
        b.iter(|| compute_block_hash(black_box(&block)))
    });

    let large_block = make_block(65536);
    c.bench_function("blake3_hash_block_64kb", |b| {
        b.iter(|| compute_block_hash(black_box(&large_block)))
    });
}

fn bench_aggregate_verify(c: &mut Criterion) {
    use hotmint_crypto::aggregate::{aggregate_votes, has_quorum};
    use hotmint_types::vote::{Vote, VoteType};

    let n = 100;
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
    let vs = ValidatorSet::new(infos);
    let hash = BlockHash([1u8; 32]);
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

    c.bench_function("aggregate_67_of_100_votes", |b| {
        b.iter(|| {
            let agg = aggregate_votes(black_box(&vs), black_box(&votes)).unwrap();
            has_quorum(&vs, &agg)
        })
    });
}

criterion_group!(
    benches,
    bench_sign,
    bench_verify,
    bench_hash_block,
    bench_aggregate_verify
);
criterion_main!(benches);

use std::collections::{HashSet, VecDeque};
use tokio::sync::Mutex;
use tracing::debug;

/// Transaction hash for deduplication
pub type TxHash = [u8; 32];

/// Simple mempool: FIFO queue with deduplication
pub struct Mempool {
    txs: Mutex<VecDeque<Vec<u8>>>,
    seen: Mutex<HashSet<TxHash>>,
    max_size: usize,
    max_tx_bytes: usize,
}

impl Mempool {
    pub fn new(max_size: usize, max_tx_bytes: usize) -> Self {
        Self {
            txs: Mutex::new(VecDeque::new()),
            seen: Mutex::new(HashSet::new()),
            max_size,
            max_tx_bytes,
        }
    }

    /// Add a transaction to the mempool. Returns false if rejected.
    pub async fn add_tx(&self, tx: Vec<u8>) -> bool {
        if tx.len() > self.max_tx_bytes {
            debug!(size = tx.len(), max = self.max_tx_bytes, "tx too large");
            return false;
        }

        let hash = Self::hash_tx(&tx);

        // Lock order: txs first, then seen (same as collect_payload)
        let mut txs = self.txs.lock().await;
        let mut seen = self.seen.lock().await;

        if seen.contains(&hash) {
            return false;
        }
        if txs.len() >= self.max_size {
            debug!(size = txs.len(), max = self.max_size, "mempool full");
            return false;
        }

        seen.insert(hash);
        txs.push_back(tx);
        true
    }

    /// Collect transactions for a block proposal (up to max_bytes total).
    /// Collected transactions are removed from the pool and the seen set.
    /// The payload is length-prefixed: `[u32_le len][bytes]...`
    pub async fn collect_payload(&self, max_bytes: usize) -> Vec<u8> {
        let mut txs = self.txs.lock().await;
        let mut seen = self.seen.lock().await;
        let mut payload = Vec::new();

        while let Some(tx) = txs.front() {
            // 4 bytes length prefix + tx bytes
            if payload.len() + 4 + tx.len() > max_bytes {
                break;
            }
            let tx = txs.pop_front().unwrap();
            seen.remove(&Self::hash_tx(&tx));
            let len = tx.len() as u32;
            payload.extend_from_slice(&len.to_le_bytes());
            payload.extend_from_slice(&tx);
        }

        payload
    }

    /// Reap collected payload back into individual transactions
    pub fn decode_payload(payload: &[u8]) -> Vec<Vec<u8>> {
        let mut txs = Vec::new();
        let mut offset = 0;
        while offset + 4 <= payload.len() {
            let len = u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            if offset + len > payload.len() {
                break;
            }
            txs.push(payload[offset..offset + len].to_vec());
            offset += len;
        }
        txs
    }

    pub async fn size(&self) -> usize {
        self.txs.lock().await.len()
    }

    fn hash_tx(tx: &[u8]) -> TxHash {
        blake3_hash(tx)
    }
}

fn blake3_hash(data: &[u8]) -> TxHash {
    *blake3::hash(data).as_bytes()
}

impl Default for Mempool {
    fn default() -> Self {
        Self::new(10_000, 1_048_576) // 10k txs, 1MB max per tx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_add_and_collect() {
        let pool = Mempool::new(100, 1024);
        assert!(pool.add_tx(b"tx1".to_vec()).await);
        assert!(pool.add_tx(b"tx2".to_vec()).await);
        assert_eq!(pool.size().await, 2);

        let payload = pool.collect_payload(1024).await;
        let txs = Mempool::decode_payload(&payload);
        assert_eq!(txs.len(), 2);
        assert_eq!(txs[0], b"tx1");
        assert_eq!(txs[1], b"tx2");
    }

    #[tokio::test]
    async fn test_dedup() {
        let pool = Mempool::new(100, 1024);
        assert!(pool.add_tx(b"tx1".to_vec()).await);
        assert!(!pool.add_tx(b"tx1".to_vec()).await); // duplicate
        assert_eq!(pool.size().await, 1);
    }

    #[tokio::test]
    async fn test_max_size() {
        let pool = Mempool::new(2, 1024);
        assert!(pool.add_tx(b"tx1".to_vec()).await);
        assert!(pool.add_tx(b"tx2".to_vec()).await);
        assert!(!pool.add_tx(b"tx3".to_vec()).await); // full
    }

    #[tokio::test]
    async fn test_tx_too_large() {
        let pool = Mempool::new(100, 4);
        assert!(!pool.add_tx(b"toolarge".to_vec()).await);
        assert!(pool.add_tx(b"ok".to_vec()).await);
    }

    #[tokio::test]
    async fn test_collect_respects_max_bytes() {
        let pool = Mempool::new(100, 1024);
        pool.add_tx(b"aaaa".to_vec()).await;
        pool.add_tx(b"bbbb".to_vec()).await;
        pool.add_tx(b"cccc".to_vec()).await;

        // Each tx: 4 bytes len prefix + 4 bytes data = 8 bytes
        // max_bytes = 17 should fit 2 txs (16 bytes) but not 3 (24 bytes)
        let payload = pool.collect_payload(17).await;
        let txs = Mempool::decode_payload(&payload);
        assert_eq!(txs.len(), 2);
    }

    #[test]
    fn test_decode_empty_payload() {
        let txs = Mempool::decode_payload(&[]);
        assert!(txs.is_empty());
    }

    #[test]
    fn test_decode_truncated_payload() {
        // Only 2 bytes when expecting at least 4 for length prefix
        let txs = Mempool::decode_payload(&[1, 2]);
        assert!(txs.is_empty());
    }

    #[test]
    fn test_decode_payload_with_truncated_data() {
        // Length prefix says 100 bytes but only 3 available
        let mut payload = vec![];
        payload.extend_from_slice(&100u32.to_le_bytes());
        payload.extend_from_slice(&[1, 2, 3]);
        let txs = Mempool::decode_payload(&payload);
        assert!(txs.is_empty());
    }

    #[tokio::test]
    async fn test_empty_tx() {
        let pool = Mempool::new(100, 1024);
        assert!(pool.add_tx(vec![]).await);
        let payload = pool.collect_payload(1024).await;
        let txs = Mempool::decode_payload(&payload);
        assert_eq!(txs.len(), 1);
        assert!(txs[0].is_empty());
    }
}

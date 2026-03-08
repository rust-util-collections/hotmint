//! Wire codec for hotmint P2P messages.
//!
//! Applies optional zstd compression based on payload size:
//!
//! ```text
//! [0x00][raw CBOR]     — uncompressed (small messages)
//! [0x01][zstd bytes]   — zstd-compressed CBOR
//! ```
//!
//! This is part of the hotmint wire protocol — all node implementations
//! (regardless of P2P library) must support this format.

use serde::{Deserialize, Serialize};

/// Payloads larger than this threshold are zstd-compressed.
const COMPRESS_THRESHOLD: usize = 256;

/// Zstd compression level (3 = good balance of speed and ratio).
const ZSTD_LEVEL: i32 = 3;

const TAG_RAW: u8 = 0x00;
const TAG_ZSTD: u8 = 0x01;

/// Serialize a value to CBOR, then conditionally zstd-compress.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_cbor_2::Error> {
    let cbor = serde_cbor_2::to_vec(value)?;
    if cbor.len() > COMPRESS_THRESHOLD {
        // Unwrap safety: zstd::encode_all only fails on invalid level or I/O error on a Vec
        let compressed = zstd::encode_all(cbor.as_slice(), ZSTD_LEVEL).unwrap();
        let mut out = Vec::with_capacity(1 + compressed.len());
        out.push(TAG_ZSTD);
        out.extend_from_slice(&compressed);
        Ok(out)
    } else {
        let mut out = Vec::with_capacity(1 + cbor.len());
        out.push(TAG_RAW);
        out.extend_from_slice(&cbor);
        Ok(out)
    }
}

/// Decode a wire frame: check tag byte, optionally decompress, then CBOR-decode.
pub fn decode<T: for<'de> Deserialize<'de>>(data: &[u8]) -> Result<T, DecodeError> {
    if data.is_empty() {
        return Err(DecodeError::EmptyFrame);
    }
    match data[0] {
        TAG_RAW => serde_cbor_2::from_slice(&data[1..]).map_err(DecodeError::Cbor),
        TAG_ZSTD => {
            let decompressed =
                zstd::decode_all(&data[1..]).map_err(|e| DecodeError::Zstd(e.to_string()))?;
            serde_cbor_2::from_slice(&decompressed).map_err(DecodeError::Cbor)
        }
        tag => Err(DecodeError::UnknownTag(tag)),
    }
}

#[derive(Debug)]
pub enum DecodeError {
    EmptyFrame,
    UnknownTag(u8),
    Cbor(serde_cbor_2::Error),
    Zstd(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyFrame => write!(f, "empty frame"),
            Self::UnknownTag(tag) => write!(f, "unknown codec tag: 0x{tag:02x}"),
            Self::Cbor(e) => write!(f, "cbor: {e}"),
            Self::Zstd(e) => write!(f, "zstd: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_message_not_compressed() {
        let data = vec![1u8, 2, 3];
        let encoded = encode(&data).unwrap();
        assert_eq!(encoded[0], TAG_RAW);
        let decoded: Vec<u8> = decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn large_message_compressed() {
        let data = vec![42u8; 1024];
        let encoded = encode(&data).unwrap();
        assert_eq!(encoded[0], TAG_ZSTD);
        // Compressed should be smaller than raw CBOR
        let cbor_len = serde_cbor_2::to_vec(&data).unwrap().len();
        assert!(encoded.len() < cbor_len);
        let decoded: Vec<u8> = decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn empty_frame_error() {
        let result: Result<Vec<u8>, _> = decode(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_tag_error() {
        let result: Result<Vec<u8>, _> = decode(&[0xFF, 0x00]);
        assert!(result.is_err());
    }
}

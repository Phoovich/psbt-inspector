use bitcoin::{
    Network, PublicKey,
    opcodes::all::{OP_CHECKMULTISIG, OP_PUSHNUM_2},
    script::Builder,
};
use thiserror::Error;

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum MultisigError {
    #[error("Not valid hex — expected a 66-char compressed pubkey (e.g. 02abc…): {0}")]
    InvalidHex(String),
    #[error("Invalid public key — expected a compressed secp256k1 point (33 bytes): {0}")]
    InvalidKey(String),
    #[error("Both keys are identical — 2-of-2 multisig requires two different pubkeys")]
    DuplicateKeys,
}

// ── Domain structs ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MultisigInfo {
    pub pubkey1: String,
    pub pubkey2: String,
    pub witness_script_hex: String,
    pub address: String,
    pub descriptor: String,
    pub network: Network,
    pub keys_were_sorted: bool,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Build a 2-of-2 P2WSH multisig address from two compressed public keys.
/// `sort_keys` applies BIP67 lexicographic ordering of the 33-byte compressed
/// serializations before constructing the witness script.
pub fn build_multisig(
    pubkey1_hex: &str,
    pubkey2_hex: &str,
    network: Network,
    sort_keys: bool,
) -> Result<MultisigInfo, MultisigError> {
    let pk1 = parse_compressed_pubkey(pubkey1_hex)?;
    let pk2 = parse_compressed_pubkey(pubkey2_hex)?;

    if pk1.inner.serialize() == pk2.inner.serialize() {
        return Err(MultisigError::DuplicateKeys);
    }

    let (pk1, pk2, keys_were_sorted) = if sort_keys {
        let s1 = pk1.inner.serialize();
        let s2 = pk2.inner.serialize();
        if s1 <= s2 {
            (pk1, pk2, false)
        } else {
            (pk2, pk1, true)
        }
    } else {
        (pk1, pk2, false)
    };

    // 2-of-2 witness script: OP_2 <pk1> <pk2> OP_2 OP_CHECKMULTISIG
    let witness_script = Builder::new()
        .push_opcode(OP_PUSHNUM_2)
        .push_key(&pk1)
        .push_key(&pk2)
        .push_opcode(OP_PUSHNUM_2)
        .push_opcode(OP_CHECKMULTISIG)
        .into_script();

    let address = bitcoin::Address::p2wsh(&witness_script, network).to_string();

    let pk1_hex = hex::encode(pk1.inner.serialize());
    let pk2_hex = hex::encode(pk2.inner.serialize());

    Ok(MultisigInfo {
        descriptor: format!("wsh(multi(2,{},{}))", pk1_hex, pk2_hex),
        witness_script_hex: hex::encode(witness_script.as_bytes()),
        address,
        pubkey1: pk1_hex,
        pubkey2: pk2_hex,
        network,
        keys_were_sorted,
    })
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn parse_compressed_pubkey(hex_str: &str) -> Result<PublicKey, MultisigError> {
    let bytes =
        hex::decode(hex_str.trim()).map_err(|e| MultisigError::InvalidHex(e.to_string()))?;

    if bytes.len() != 33 {
        return Err(MultisigError::InvalidKey(format!(
            "expected 33-byte compressed key, got {} bytes",
            bytes.len()
        )));
    }

    PublicKey::from_slice(&bytes).map_err(|e| MultisigError::InvalidKey(e.to_string()))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // secp256k1 generator point G and 2G — canonical test vectors
    const PK1: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    const PK2: &str = "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";

    #[test]
    fn rejects_invalid_hex() {
        let err = build_multisig("not-hex", PK2, Network::Testnet, true).unwrap_err();
        assert!(matches!(err, MultisigError::InvalidHex(_)));
    }

    #[test]
    fn rejects_uncompressed_key() {
        // 65-byte uncompressed key (prefix 04)
        let uncompressed = format!("04{}", "aa".repeat(64));
        let err = build_multisig(&uncompressed, PK2, Network::Testnet, true).unwrap_err();
        assert!(matches!(err, MultisigError::InvalidKey(_)));
    }

    #[test]
    fn rejects_invalid_key_bytes() {
        // 33 bytes but not a valid point on the curve
        let bad = "02".to_string() + &"ff".repeat(32);
        let err = build_multisig(&bad, PK2, Network::Testnet, true).unwrap_err();
        assert!(matches!(err, MultisigError::InvalidKey(_)));
    }

    #[test]
    fn rejects_duplicate_keys() {
        let err = build_multisig(PK1, PK1, Network::Testnet, true).unwrap_err();
        assert!(matches!(err, MultisigError::DuplicateKeys));
    }

    #[test]
    fn produces_testnet_p2wsh_address() {
        let info = build_multisig(PK1, PK2, Network::Testnet, true).unwrap();
        // testnet P2WSH: "tb1q" prefix, 62 chars total
        assert!(
            info.address.starts_with("tb1q"),
            "address: {}",
            info.address
        );
        assert_eq!(info.address.len(), 62, "address: {}", info.address);
    }

    #[test]
    fn descriptor_format_is_correct() {
        let info = build_multisig(PK1, PK2, Network::Testnet, true).unwrap();
        assert!(info.descriptor.starts_with("wsh(multi(2,"));
        assert!(info.descriptor.ends_with("))"));
        // descriptor must contain both pubkeys
        assert!(info.descriptor.contains(&info.pubkey1));
        assert!(info.descriptor.contains(&info.pubkey2));
    }

    #[test]
    fn bip67_sort_is_deterministic() {
        let a = build_multisig(PK1, PK2, Network::Testnet, true).unwrap();
        let b = build_multisig(PK2, PK1, Network::Testnet, true).unwrap();
        assert_eq!(a.address, b.address);
        assert_eq!(a.descriptor, b.descriptor);
    }

    #[test]
    fn unsorted_build_differs_when_keys_out_of_order() {
        // PK2 > PK1 lexicographically, so with sort_keys=false (PK2, PK1) ≠ (PK1, PK2)
        let sorted = build_multisig(PK1, PK2, Network::Testnet, false).unwrap();
        let unsorted = build_multisig(PK2, PK1, Network::Testnet, false).unwrap();
        assert_ne!(sorted.address, unsorted.address);
    }

    #[test]
    fn witness_script_hex_is_non_empty() {
        let info = build_multisig(PK1, PK2, Network::Testnet, true).unwrap();
        assert!(!info.witness_script_hex.is_empty());
        // 1 + 1 + 33 + 1 + 33 + 1 + 1 = 71 bytes → 142 hex chars
        assert_eq!(info.witness_script_hex.len(), 142);
    }
}

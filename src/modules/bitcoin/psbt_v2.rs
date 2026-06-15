//! Manual parser for PSBTv2 (BIP-370).
//!
//! `bitcoin::psbt::Psbt::deserialize` always rejects PSBTv2 because it
//! requires `PSBT_GLOBAL_UNSIGNED_TX` (BIP-174), which PSBTv2 omits in favor
//! of `PSBT_GLOBAL_{VERSION,INPUT_COUNT,OUTPUT_COUNT}`. This module hand-walks
//! the global/input/output key-value maps and feeds the results into the same
//! `finalize` helper used by the v0 path.

use super::psbt::{
    InputSummary, OutputSummary, PsbtError, PsbtSummary, ScriptType, finalize,
    script_type_from_script,
};
use bitcoin::{ScriptBuf, TxOut, Txid, consensus};

const MAGIC: &[u8] = b"psbt\xff";

// Global key types (BIP-174 / BIP-370)
const PSBT_GLOBAL_INPUT_COUNT: u8 = 0x04;
const PSBT_GLOBAL_OUTPUT_COUNT: u8 = 0x05;
const PSBT_GLOBAL_VERSION: u8 = 0xfb;

// Input key types
const PSBT_IN_NON_WITNESS_UTXO: u8 = 0x00;
const PSBT_IN_WITNESS_UTXO: u8 = 0x01;
const PSBT_IN_PARTIAL_SIG: u8 = 0x02;
const PSBT_IN_FINAL_SCRIPTSIG: u8 = 0x07;
const PSBT_IN_FINAL_SCRIPTWITNESS: u8 = 0x08;
const PSBT_IN_PREVIOUS_TXID: u8 = 0x0e;
const PSBT_IN_OUTPUT_INDEX: u8 = 0x0f;

// Output key types
const PSBT_OUT_AMOUNT: u8 = 0x03;
const PSBT_OUT_SCRIPT: u8 = 0x04;

/// Walks raw bytes and reads BIP-174 key-value pairs.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], PsbtError> {
        let remaining = self.bytes.len() - self.pos;
        if len > remaining {
            return Err(PsbtError::Decode("unexpected end of PSBT data".into()));
        }
        let end = self.pos + len;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, PsbtError> {
        Ok(self.read_bytes(1)?[0])
    }

    /// BIP-174 compact-size (varint): `<0xfd`→1 byte, `0xfd`→u16 LE,
    /// `0xfe`→u32 LE, `0xff`→u64 LE.
    fn read_compact_size(&mut self) -> Result<u64, PsbtError> {
        match self.read_u8()? {
            0xfd => Ok(u16::from_le_bytes(self.read_bytes(2)?.try_into().unwrap()) as u64),
            0xfe => Ok(u32::from_le_bytes(self.read_bytes(4)?.try_into().unwrap()) as u64),
            0xff => Ok(u64::from_le_bytes(self.read_bytes(8)?.try_into().unwrap())),
            n => Ok(n as u64),
        }
    }

    /// Reads one key-value pair, or `None` at a map separator (`0x00` keylen).
    fn read_pair(&mut self) -> Result<Option<(u8, Vec<u8>)>, PsbtError> {
        let key_len = self.read_compact_size()?;
        if key_len == 0 {
            return Ok(None);
        }
        let key_bytes = self.read_bytes(key_len as usize)?;
        let key_type = key_bytes[0];

        let val_len = self.read_compact_size()?;
        let value = self.read_bytes(val_len as usize)?.to_vec();

        Ok(Some((key_type, value)))
    }
}

/// Extracts the amount and scriptPubKey of output `vout` from a previous
/// transaction's raw network-serialized bytes (`PSBT_IN_NON_WITNESS_UTXO`).
///
/// Only the outputs are needed, so this skips over inputs and stops before
/// the witness/locktime. Hand-rolled (rather than
/// `bitcoin::consensus::deserialize::<Transaction>`) because that decoder
/// rejects segwit-marked transactions with empty witnesses, which some PSBTs
/// embed anyway.
fn extract_prev_output(tx_bytes: &[u8], vout: u32) -> Option<(u64, ScriptBuf)> {
    let mut cur = Cursor::new(tx_bytes);
    cur.read_bytes(4).ok()?; // version
    if tx_bytes.get(4) == Some(&0x00) {
        cur.read_bytes(2).ok()?; // segwit marker + flag
    }

    let input_count = cur.read_compact_size().ok()?;
    for _ in 0..input_count {
        cur.read_bytes(36).ok()?; // previous outpoint (txid + vout)
        let script_sig_len = cur.read_compact_size().ok()?;
        cur.read_bytes(script_sig_len as usize).ok()?;
        cur.read_bytes(4).ok()?; // sequence
    }

    let output_count = cur.read_compact_size().ok()?;
    for i in 0..output_count {
        let amount = u64::from_le_bytes(cur.read_bytes(8).ok()?.try_into().ok()?);
        let script_len = cur.read_compact_size().ok()?;
        let script = cur.read_bytes(script_len as usize).ok()?.to_vec();
        if i as u32 == vout {
            return Some((amount, ScriptBuf::from(script)));
        }
    }
    None
}

/// Result of verifying a `non_witness_utxo` against the input's previous txid.
enum PrevOutputResult {
    /// The embedded tx hashed to the expected txid; value is trustworthy.
    Verified(u64, ScriptBuf),
    /// The embedded tx hashed to a *different* txid — value is discarded.
    Mismatch,
    /// The embedded tx could not be deserialized (e.g. empty-witness segwit),
    /// so its hash could not be checked; value is kept but unverifiable.
    Unverifiable(u64, ScriptBuf),
    /// `vout` is out of range for the embedded tx's outputs.
    NotFound,
}

/// Verifies `prev_tx_bytes` hashes to `expected_txid` (S3 — fee-spoofing
/// protection) before trusting its output value. Tries the standard decoder
/// first since it gives us `compute_txid()`; falls back to the hand-rolled
/// `extract_prev_output` (which tolerates empty-witness segwit txs the
/// decoder rejects) when that fails, marking the value unverifiable.
fn verify_and_extract_prev_output(
    prev_tx_bytes: &[u8],
    vout: u32,
    expected_txid: Txid,
    _index: usize,
) -> PrevOutputResult {
    match consensus::deserialize::<bitcoin::Transaction>(prev_tx_bytes) {
        Ok(tx) => {
            if tx.compute_txid() != expected_txid {
                return PrevOutputResult::Mismatch;
            }
            match tx.output.get(vout as usize) {
                Some(out) => {
                    PrevOutputResult::Verified(out.value.to_sat(), out.script_pubkey.clone())
                }
                None => PrevOutputResult::NotFound,
            }
        }
        Err(_) => match extract_prev_output(prev_tx_bytes, vout) {
            Some((amount, script)) => PrevOutputResult::Unverifiable(amount, script),
            None => PrevOutputResult::NotFound,
        },
    }
}

/// Parse a PSBTv2 (BIP-370) from raw decoded bytes.
pub(super) fn parse(bytes: &[u8]) -> Result<PsbtSummary, PsbtError> {
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        return Err(PsbtError::Decode("missing PSBT magic bytes".into()));
    }
    let mut cur = Cursor::new(bytes);
    cur.pos = MAGIC.len();

    // ── Global map ──
    let mut version: Option<u32> = None;
    let mut input_count: Option<u64> = None;
    let mut output_count: Option<u64> = None;

    while let Some((key_type, value)) = cur.read_pair()? {
        match key_type {
            PSBT_GLOBAL_VERSION => {
                version = Some(u32::from_le_bytes(value.as_slice().try_into().map_err(
                    |_| PsbtError::Decode("invalid PSBT_GLOBAL_VERSION".into()),
                )?));
            }
            PSBT_GLOBAL_INPUT_COUNT => input_count = Some(Cursor::new(&value).read_compact_size()?),
            PSBT_GLOBAL_OUTPUT_COUNT => {
                output_count = Some(Cursor::new(&value).read_compact_size()?)
            }
            _ => {}
        }
    }

    if version != Some(2) {
        return Err(PsbtError::Decode(
            "not a PSBTv2 (missing PSBT_GLOBAL_VERSION=2)".into(),
        ));
    }
    let input_count = input_count
        .ok_or_else(|| PsbtError::Decode("missing PSBT_GLOBAL_INPUT_COUNT".into()))?
        as usize;
    let output_count = output_count
        .ok_or_else(|| PsbtError::Decode("missing PSBT_GLOBAL_OUTPUT_COUNT".into()))?
        as usize;

    // ── Per-input maps ──
    // Each input map needs at least 1 byte (its separator), so a count larger
    // than the remaining bytes cannot be real — reject before allocating.
    if input_count > cur.bytes.len() - cur.pos {
        return Err(PsbtError::Decode(
            "PSBT_GLOBAL_INPUT_COUNT exceeds remaining data".into(),
        ));
    }
    let mut inputs = Vec::with_capacity(input_count);
    let mut total_input_value: Option<u64> = Some(0);
    let mut signed_inputs = 0;
    let mut warnings: Vec<String> = Vec::new();

    for i in 0..input_count {
        let mut txid: Option<Txid> = None;
        let mut vout: Option<u32> = None;
        let mut value: Option<u64> = None;
        let mut script_type = ScriptType::Unknown;
        let mut non_witness_utxo: Option<Vec<u8>> = None;
        let mut partial_sigs = 0;
        let mut signed = false;

        while let Some((key_type, val)) = cur.read_pair()? {
            match key_type {
                PSBT_IN_PREVIOUS_TXID => {
                    txid = Some(consensus::deserialize::<Txid>(&val).map_err(|e| {
                        PsbtError::Decode(format!("input {i}: invalid PSBT_IN_PREVIOUS_TXID: {e}"))
                    })?);
                }
                PSBT_IN_OUTPUT_INDEX => {
                    vout = Some(u32::from_le_bytes(val.as_slice().try_into().map_err(
                        |_| PsbtError::Decode(format!("input {i}: invalid PSBT_IN_OUTPUT_INDEX")),
                    )?));
                }
                PSBT_IN_NON_WITNESS_UTXO => {
                    non_witness_utxo = Some(val);
                }
                PSBT_IN_WITNESS_UTXO => {
                    let txout = consensus::deserialize::<TxOut>(&val).map_err(|e| {
                        PsbtError::Decode(format!("input {i}: invalid PSBT_IN_WITNESS_UTXO: {e}"))
                    })?;
                    value = Some(txout.value.to_sat());
                    script_type = script_type_from_script(&txout.script_pubkey);
                    warnings.push(format!("input {i}: value from witness_utxo (unverifiable)"));
                }
                PSBT_IN_PARTIAL_SIG => partial_sigs += 1,
                PSBT_IN_FINAL_SCRIPTSIG | PSBT_IN_FINAL_SCRIPTWITNESS => signed = true,
                _ => {}
            }
        }

        let txid = txid.ok_or_else(|| {
            PsbtError::Decode(format!("input {i}: missing PSBT_IN_PREVIOUS_TXID"))
        })?;
        let vout = vout
            .ok_or_else(|| PsbtError::Decode(format!("input {i}: missing PSBT_IN_OUTPUT_INDEX")))?;

        // Fall back to the embedded previous transaction when no witness UTXO
        // was provided directly.
        if value.is_none()
            && let Some(prev_tx_bytes) = &non_witness_utxo
        {
            match verify_and_extract_prev_output(prev_tx_bytes, vout, txid, i) {
                PrevOutputResult::Verified(amount, script) => {
                    value = Some(amount);
                    script_type = script_type_from_script(&script);
                }
                PrevOutputResult::Mismatch => {
                    warnings.push(format!(
                        "input {i}: non_witness_utxo does not match input txid"
                    ));
                }
                PrevOutputResult::Unverifiable(amount, script) => {
                    value = Some(amount);
                    script_type = script_type_from_script(&script);
                    warnings.push(format!(
                        "input {i}: non_witness_utxo could not be verified against input txid"
                    ));
                }
                PrevOutputResult::NotFound => {}
            }
        }

        total_input_value = match (total_input_value, value) {
            (Some(total), Some(v)) => Some(total.saturating_add(v)),
            _ => None,
        };
        if signed || partial_sigs > 0 {
            signed_inputs += 1;
        }

        inputs.push(InputSummary {
            index: i,
            txid: txid.to_string(),
            vout,
            value,
            script_type,
            address: None,
            partial_sigs,
        });
    }

    // ── Per-output maps ──
    if output_count > cur.bytes.len() - cur.pos {
        return Err(PsbtError::Decode(
            "PSBT_GLOBAL_OUTPUT_COUNT exceeds remaining data".into(),
        ));
    }
    let mut outputs = Vec::with_capacity(output_count);

    for i in 0..output_count {
        let mut amount: Option<u64> = None;
        let mut script_type: Option<ScriptType> = None;

        while let Some((key_type, val)) = cur.read_pair()? {
            match key_type {
                PSBT_OUT_AMOUNT => {
                    amount = Some(u64::from_le_bytes(val.as_slice().try_into().map_err(
                        |_| PsbtError::Decode(format!("output {i}: invalid PSBT_OUT_AMOUNT")),
                    )?));
                }
                PSBT_OUT_SCRIPT => {
                    script_type = Some(script_type_from_script(&ScriptBuf::from(val)));
                }
                _ => {}
            }
        }

        let amount = amount
            .ok_or_else(|| PsbtError::Decode(format!("output {i}: missing PSBT_OUT_AMOUNT")))?;
        let script_type = script_type
            .ok_or_else(|| PsbtError::Decode(format!("output {i}: missing PSBT_OUT_SCRIPT")))?;

        outputs.push(OutputSummary {
            index: i,
            value: amount,
            script_type,
            address: None,
        });
    }

    Ok(finalize(
        2,
        input_count,
        output_count,
        inputs,
        outputs,
        total_input_value,
        signed_inputs,
        warnings,
    ))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::bitcoin::psbt::{FeeInfo, PsbtError, parse_psbt};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use bitcoin::{Amount, Transaction, absolute::LockTime, hashes::Hash, transaction::Version};

    fn push_compact_size(buf: &mut Vec<u8>, n: u64) {
        if n < 0xfd {
            buf.push(n as u8);
        } else if n <= 0xffff {
            buf.push(0xfd);
            buf.extend_from_slice(&(n as u16).to_le_bytes());
        } else if n <= 0xffff_ffff {
            buf.push(0xfe);
            buf.extend_from_slice(&(n as u32).to_le_bytes());
        } else {
            buf.push(0xff);
            buf.extend_from_slice(&n.to_le_bytes());
        }
    }

    fn push_pair(buf: &mut Vec<u8>, key_type: u8, value: &[u8]) {
        push_compact_size(buf, 1); // key length = 1 (just the key type byte)
        buf.push(key_type);
        push_compact_size(buf, value.len() as u64);
        buf.extend_from_slice(value);
    }

    /// Builds a minimal valid PSBTv2: 1 input (with witness UTXO), 1 output.
    fn build_minimal_v2_psbt() -> Vec<u8> {
        let mut buf = MAGIC.to_vec();

        // Global map
        push_pair(&mut buf, PSBT_GLOBAL_VERSION, &2u32.to_le_bytes());
        let mut count = Vec::new();
        push_compact_size(&mut count, 1);
        push_pair(&mut buf, PSBT_GLOBAL_INPUT_COUNT, &count);
        push_pair(&mut buf, PSBT_GLOBAL_OUTPUT_COUNT, &count);
        buf.push(0x00); // separator

        // Input map
        let txid_bytes: [u8; 32] = core::array::from_fn(|i| i as u8 + 1);
        let txid = Txid::from_byte_array(txid_bytes);
        push_pair(
            &mut buf,
            PSBT_IN_PREVIOUS_TXID,
            &consensus::serialize(&txid),
        );
        push_pair(&mut buf, PSBT_IN_OUTPUT_INDEX, &0u32.to_le_bytes());
        let witness_utxo = TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: ScriptBuf::new(),
        };
        push_pair(
            &mut buf,
            PSBT_IN_WITNESS_UTXO,
            &consensus::serialize(&witness_utxo),
        );
        buf.push(0x00); // separator

        // Output map
        push_pair(&mut buf, PSBT_OUT_AMOUNT, &90_000u64.to_le_bytes());
        push_pair(&mut buf, PSBT_OUT_SCRIPT, &[]); // empty script
        buf.push(0x00); // separator

        buf
    }

    /// Builds a PSBTv2 with one input carrying `PSBT_IN_NON_WITNESS_UTXO` set
    /// to `prev_tx`, claiming to spend `claimed_txid:vout`, and one output of
    /// `output_amount` sats.
    fn build_v2_psbt_with_non_witness_utxo(
        prev_tx: &Transaction,
        claimed_txid: Txid,
        vout: u32,
        output_amount: u64,
    ) -> Vec<u8> {
        let mut buf = MAGIC.to_vec();

        push_pair(&mut buf, PSBT_GLOBAL_VERSION, &2u32.to_le_bytes());
        let mut count = Vec::new();
        push_compact_size(&mut count, 1);
        push_pair(&mut buf, PSBT_GLOBAL_INPUT_COUNT, &count);
        push_pair(&mut buf, PSBT_GLOBAL_OUTPUT_COUNT, &count);
        buf.push(0x00); // separator

        push_pair(
            &mut buf,
            PSBT_IN_PREVIOUS_TXID,
            &consensus::serialize(&claimed_txid),
        );
        push_pair(&mut buf, PSBT_IN_OUTPUT_INDEX, &vout.to_le_bytes());
        push_pair(
            &mut buf,
            PSBT_IN_NON_WITNESS_UTXO,
            &consensus::serialize(prev_tx),
        );
        buf.push(0x00); // separator

        push_pair(&mut buf, PSBT_OUT_AMOUNT, &output_amount.to_le_bytes());
        push_pair(&mut buf, PSBT_OUT_SCRIPT, &[]); // empty script
        buf.push(0x00); // separator

        buf
    }

    fn tx_with_one_output(value_sats: u64) -> Transaction {
        Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![],
            output: vec![TxOut {
                value: Amount::from_sat(value_sats),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    // ─── S3 (v2 path): verify_and_extract_prev_output outcomes ───────────────

    #[test]
    fn psbtv2_non_witness_utxo_verified_matches_value() {
        let prev_tx = tx_with_one_output(100_000);
        let txid = prev_tx.compute_txid();
        let bytes = build_v2_psbt_with_non_witness_utxo(&prev_tx, txid, 0, 90_000);

        let summary = parse_psbt(&STANDARD.encode(&bytes)).unwrap();

        assert_eq!(summary.inputs[0].value, Some(100_000));
        assert!(matches!(summary.fee, FeeInfo::Known(10_000)));
        assert!(
            !summary
                .warnings
                .iter()
                .any(|w| w.contains("does not match"))
        );
    }

    #[test]
    fn psbtv2_non_witness_utxo_mismatch_is_rejected() {
        let prev_tx = tx_with_one_output(100_000);
        let fake_txid = tx_with_one_output(999_999).compute_txid();
        let bytes = build_v2_psbt_with_non_witness_utxo(&prev_tx, fake_txid, 0, 50_000);

        let summary = parse_psbt(&STANDARD.encode(&bytes)).unwrap();

        assert_eq!(summary.inputs[0].value, None);
        assert!(matches!(summary.fee, FeeInfo::Unknown));
        assert!(
            summary
                .warnings
                .iter()
                .any(|w| w.contains("does not match")),
            "warnings: {:?}",
            summary.warnings
        );
    }

    #[test]
    fn psbtv2_non_witness_utxo_vout_not_found() {
        let prev_tx = tx_with_one_output(100_000);
        let txid = prev_tx.compute_txid();
        // vout 5 doesn't exist on a 1-output tx
        let bytes = build_v2_psbt_with_non_witness_utxo(&prev_tx, txid, 5, 50_000);

        let summary = parse_psbt(&STANDARD.encode(&bytes)).unwrap();

        assert_eq!(summary.inputs[0].value, None);
        assert!(matches!(summary.fee, FeeInfo::Unknown));
    }

    #[test]
    fn parses_minimal_psbtv2() {
        let bytes = build_minimal_v2_psbt();
        let summary = parse_psbt(&STANDARD.encode(&bytes)).unwrap();

        assert_eq!(summary.version, 2);
        assert_eq!(summary.input_count, 1);
        assert_eq!(summary.output_count, 1);
        assert!(matches!(summary.fee, FeeInfo::Known(10_000)));

        // Txid Display reverses the raw 32 bytes [1, 2, ..., 32] -> [32, ..., 1].
        let expected_txid: [u8; 32] = core::array::from_fn(|i| 32 - i as u8);
        assert_eq!(summary.inputs[0].txid, hex::encode(expected_txid));
    }

    /// S1 PoC: a compact-size length of u64::MAX in `PSBT_IN_PREVIOUS_TXID`'s
    /// value would overflow `pos + len` before the old bounds check ran.
    #[test]
    fn read_bytes_overflow_is_decode_error_not_panic() {
        let b64 = "cHNidP8B+wQCAAAAAQQBAQEFAQEAAQD///////////8=";
        let err = parse_psbt(b64).unwrap_err();
        assert!(matches!(err, PsbtError::Decode(_)));
    }

    /// S2 PoC: `PSBT_GLOBAL_INPUT_COUNT` of u64::MAX would previously be
    /// passed straight into `Vec::with_capacity`, panicking with "capacity
    /// overflow" / OOM.
    #[test]
    fn huge_input_count_is_decode_error_not_panic() {
        let b64 = "cHNidP8B+wQCAAAAAQQJ////////////AQUBAQA=";
        let err = parse_psbt(b64).unwrap_err();
        assert!(matches!(err, PsbtError::Decode(_)));
    }

    /// S4 PoC: outputs (0xC3_5_0_0_0_0 sats) exceed inputs (0 sats, since this
    /// input carries no UTXO data) — must surface as `FeeInfo::Invalid`, not
    /// a silent "Fee: 0".
    #[test]
    fn psbtv2_outputs_exceeding_inputs_is_invalid_fee() {
        let b64 = "cHNidP8B+wQCAAAAAQQBAQEFAQEAAQ4gAQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyABDwQAAAAAAQEJECcAAAAAAAAAAAEDCFDDAAAAAAAAAQQAAA==";
        let summary = parse_psbt(b64).unwrap();
        assert!(matches!(summary.fee, FeeInfo::Invalid { .. }));
    }

    #[test]
    fn missing_input_count_is_decode_error() {
        let mut buf = MAGIC.to_vec();
        push_pair(&mut buf, PSBT_GLOBAL_VERSION, &2u32.to_le_bytes());
        let mut count = Vec::new();
        push_compact_size(&mut count, 1);
        push_pair(&mut buf, PSBT_GLOBAL_OUTPUT_COUNT, &count);
        buf.push(0x00); // separator

        let err = parse_psbt(&STANDARD.encode(&buf)).unwrap_err();
        assert!(matches!(err, PsbtError::Decode(_)));
    }

    /// Real-world PSBTv2 whose input carries `PSBT_IN_NON_WITNESS_UTXO`
    /// (no `PSBT_IN_WITNESS_UTXO`), with a finalized scriptSig signature.
    /// The embedded previous transaction has a segwit marker/flag but an
    /// empty witness, which `bitcoin::Transaction`'s decoder rejects —
    /// `extract_prev_output` must still recover the spent output's value
    /// and script so the fee can be computed.
    #[test]
    fn parses_psbtv2_with_non_witness_utxo() {
        let b64 = "cHNidP8BAgQCAAAAAQMEAAAAAAEEAQEBBQECAQYBAAH7BAIAAAAAAQB1AgAAAAABARgZ9Pl6whiJ8YuQ/YEuxTGMnwobGng4fT1Dv+vt3JKHAQAAAAD9////AiBOAAAAAAAAF6kUnJZbfUSQwWy61prz1fqVCXLAeCmH+OwBAAAAAAAWABRh59fli1CWPd+AVW0emqFNs9Zh4wCZ2Q0AAQfZAEcwRAIgOPpP9Wp2ETeaWw6l8D7wCiuqbLpsAWuVcIyuIyQfgIoCIAxe3R73Z77S4JELP0LYmjTYv+FbrvZGX0opwgh/YLQTAUcwRAIgUfnsUvww3h0G/Zsyw3OPCxBw2c+zuHaj3ExtpE1K6+QCIA9NJV2NZ8vcPKresOgPwl/trG5A1S1L4HQ4PzXWfurbAUdSIQOMF7aMsr8aW4oJab64OcsN+eH/pfhdP4WE4Wokl9W+diEDq8PftWQvM5o+JD5Qy5n2IeMWrXyCg+6uLK+JXwpECeZSrgEOIFkGRlWLVK0n/2FvgTqVTV5JjUqVUsZ/5+5x2NBu7TseAQ8EAAAAAAABAwgoIwAAAAAAAAEEFgAUR+KC+oLaI5TMrTrnvR+2ccA53t0AAQMIcBcAAAAAAAABBBl2qRT40IGCt+fHvE3w0PPOva2Y0WTVAYisAA==";
        let summary = parse_psbt(b64).unwrap();

        assert_eq!(summary.version, 2);
        assert_eq!(summary.input_count, 1);
        assert_eq!(summary.output_count, 2);
        assert_eq!(summary.inputs[0].value, Some(20_000));
        assert_eq!(summary.inputs[0].script_type, ScriptType::P2SH);
        assert!(matches!(summary.fee, FeeInfo::Known(5_000)));
        assert_eq!(summary.signing_progress.signed_inputs, 1);
    }
}

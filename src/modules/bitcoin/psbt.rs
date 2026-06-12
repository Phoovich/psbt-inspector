use super::psbt_v2;
use base64::{Engine, engine::general_purpose::STANDARD};
use bitcoin::psbt::Psbt as BitcoinPsbt;
use thiserror::Error;

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum PsbtError {
    #[error("Input is empty")]
    Empty,
    #[error("Could not decode — paste the PSBT as base64 or hex")]
    InvalidEncoding,
    #[error("Invalid PSBT: {0}")]
    Decode(String),
}

// ── Domain structs ────────────────────────────────────────────────────────────

/// Script type detected from an output or UTXO scriptPubKey.
#[allow(dead_code)] // variants for P2PKH/P2SH/P2TR populated during real parsing
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptType {
    P2PKH,
    P2SH,
    P2WPKH,
    P2WSH,
    P2TR,
    Unknown,
}

impl ScriptType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::P2PKH => "P2PKH",
            Self::P2SH => "P2SH",
            Self::P2WPKH => "P2WPKH",
            Self::P2WSH => "P2WSH",
            Self::P2TR => "P2TR",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct InputSummary {
    pub index: usize,
    pub txid: String,
    pub vout: u32,
    /// None when the PSBT does not embed the UTXO value for this input.
    pub value: Option<u64>,
    pub script_type: ScriptType,
    pub address: Option<String>,
    pub partial_sigs: usize,
}

#[derive(Debug, Clone)]
pub struct OutputSummary {
    pub index: usize,
    pub value: u64,
    pub script_type: ScriptType,
    pub address: Option<String>,
}

#[derive(Debug, Clone)]
pub enum FeeInfo {
    /// All inputs had embedded UTXO data; fee = sum(inputs) − sum(outputs).
    Known(u64),
    /// At least one input is missing UTXO data; fee cannot be calculated.
    Unknown,
}

#[derive(Debug, Clone)]
pub struct SigningProgress {
    pub signed_inputs: usize,
    pub total_inputs: usize,
}

#[derive(Debug, Clone)]
pub struct PsbtSummary {
    pub version: u8,
    pub input_count: usize,
    pub output_count: usize,
    pub inputs: Vec<InputSummary>,
    pub outputs: Vec<OutputSummary>,
    pub fee: FeeInfo,
    pub signing_progress: SigningProgress,
    pub warnings: Vec<String>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Parse a PSBT from a base64 or hex-encoded string.
/// Called from `tokio::spawn` so it is deliberately synchronous.
pub fn parse_psbt(input: &str) -> Result<PsbtSummary, PsbtError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(PsbtError::Empty);
    }
    let bytes = decode_bytes(input)?;
    match BitcoinPsbt::deserialize(&bytes) {
        Ok(psbt) => Ok(summarize(psbt)),
        // A v0 PSBT always embeds PSBT_GLOBAL_UNSIGNED_TX, and its
        // PSBT_GLOBAL_VERSION (if present) must be 0. PSBTv2 (BIP-370) sets
        // PSBT_GLOBAL_VERSION=2 and omits PSBT_GLOBAL_UNSIGNED_TX in favor of
        // PSBT_GLOBAL_{TX_VERSION,INPUT_COUNT,OUTPUT_COUNT}, so the v0 decoder
        // rejects it with one of these two errors depending on key order.
        Err(bitcoin::psbt::Error::MustHaveUnsignedTx | bitcoin::psbt::Error::Version(_)) => {
            psbt_v2::parse(&bytes)
        }
        Err(e) => Err(PsbtError::Decode(e.to_string())),
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Try base64 first (most common PSBT format), then hex.
fn decode_bytes(input: &str) -> Result<Vec<u8>, PsbtError> {
    if let Ok(bytes) = STANDARD.decode(input) {
        return Ok(bytes);
    }
    if let Ok(bytes) = hex::decode(input) {
        return Ok(bytes);
    }
    Err(PsbtError::InvalidEncoding)
}

fn summarize(psbt: BitcoinPsbt) -> PsbtSummary {
    let input_count = psbt.unsigned_tx.input.len();
    let output_count = psbt.unsigned_tx.output.len();

    let mut total_input_value: Option<u64> = Some(0);

    let inputs: Vec<InputSummary> = psbt
        .unsigned_tx
        .input
        .iter()
        .zip(&psbt.inputs)
        .enumerate()
        .map(|(i, (tx_in, psbt_in))| {
            let value = input_utxo_value(psbt_in, tx_in);
            total_input_value = match (total_input_value, value) {
                (Some(total), Some(v)) => Some(total.saturating_add(v)),
                _ => None,
            };
            InputSummary {
                index: i,
                txid: tx_in.previous_output.txid.to_string(),
                vout: tx_in.previous_output.vout,
                value,
                script_type: input_script_type(psbt_in, tx_in),
                address: None, // network-aware extraction deferred to M3
                partial_sigs: psbt_in.partial_sigs.len(),
            }
        })
        .collect();

    let outputs: Vec<OutputSummary> = psbt
        .unsigned_tx
        .output
        .iter()
        .enumerate()
        .map(|(i, tx_out)| OutputSummary {
            index: i,
            value: tx_out.value.to_sat(),
            script_type: script_type_from_script(&tx_out.script_pubkey),
            address: None,
        })
        .collect();

    let signed_inputs = psbt
        .inputs
        .iter()
        .filter(|inp| {
            !inp.partial_sigs.is_empty()
                || inp.final_script_witness.is_some()
                || inp.final_script_sig.is_some()
        })
        .count();

    finalize(
        0,
        input_count,
        output_count,
        inputs,
        outputs,
        total_input_value,
        signed_inputs,
    )
}

/// Shared by the PSBTv0 and PSBTv2 parsers: computes fee + warnings and
/// assembles the final `PsbtSummary`.
///
/// `total_input_value` is `None` if any input's UTXO value is unknown.
pub(super) fn finalize(
    version: u8,
    input_count: usize,
    output_count: usize,
    inputs: Vec<InputSummary>,
    outputs: Vec<OutputSummary>,
    total_input_value: Option<u64>,
    signed_inputs: usize,
) -> PsbtSummary {
    let total_output_value: u64 = outputs.iter().map(|o| o.value).sum();

    let fee = match total_input_value {
        Some(total_in) if input_count > 0 => {
            FeeInfo::Known(total_in.saturating_sub(total_output_value))
        }
        _ => FeeInfo::Unknown,
    };

    let mut warnings = Vec::new();
    if signed_inputs < input_count {
        warnings.push(format!(
            "{} of {} inputs signed — not ready to broadcast",
            signed_inputs, input_count
        ));
    }
    if matches!(fee, FeeInfo::Unknown) && input_count > 0 {
        warnings.push("Fee unknown — UTXO values not embedded in this PSBT".into());
    }

    PsbtSummary {
        version,
        input_count,
        output_count,
        inputs,
        outputs,
        fee,
        signing_progress: SigningProgress {
            signed_inputs,
            total_inputs: input_count,
        },
        warnings,
    }
}

/// Extract the UTXO value for a PSBT input, checking witness_utxo first
/// then falling back to non_witness_utxo.
fn input_utxo_value(psbt_in: &bitcoin::psbt::Input, tx_in: &bitcoin::TxIn) -> Option<u64> {
    if let Some(utxo) = &psbt_in.witness_utxo {
        return Some(utxo.value.to_sat());
    }
    if let Some(prev_tx) = &psbt_in.non_witness_utxo {
        let vout = tx_in.previous_output.vout as usize;
        return prev_tx.output.get(vout).map(|o| o.value.to_sat());
    }
    None
}

fn input_script_type(psbt_in: &bitcoin::psbt::Input, tx_in: &bitcoin::TxIn) -> ScriptType {
    if let Some(utxo) = &psbt_in.witness_utxo {
        return script_type_from_script(&utxo.script_pubkey);
    }
    if let Some(prev_tx) = &psbt_in.non_witness_utxo {
        let vout = tx_in.previous_output.vout as usize;
        if let Some(out) = prev_tx.output.get(vout) {
            return script_type_from_script(&out.script_pubkey);
        }
    }
    ScriptType::Unknown
}

pub(super) fn script_type_from_script(script: &bitcoin::Script) -> ScriptType {
    if script.is_p2pkh() {
        ScriptType::P2PKH
    } else if script.is_p2sh() {
        ScriptType::P2SH
    } else if script.is_p2wpkh() {
        ScriptType::P2WPKH
    } else if script.is_p2wsh() {
        ScriptType::P2WSH
    } else if script.is_p2tr() {
        ScriptType::P2TR
    } else {
        ScriptType::Unknown
    }
}

// ── Test helpers ──────────────────────────────────────────────────────────────

#[cfg(test)]
impl PsbtSummary {
    pub fn fake() -> Self {
        PsbtSummary {
            version: 0,
            input_count: 2,
            output_count: 2,
            inputs: vec![
                InputSummary {
                    index: 0,
                    txid: "abcd1234ef567890abcd1234ef567890abcd1234ef567890abcd1234ef567890".into(),
                    vout: 0,
                    value: Some(100_000),
                    script_type: ScriptType::P2WPKH,
                    address: Some("bc1qexampleaddress1fakedata".into()),
                    partial_sigs: 1,
                },
                InputSummary {
                    index: 1,
                    txid: "1234abcd5678ef901234abcd5678ef901234abcd5678ef901234abcd5678ef90".into(),
                    vout: 1,
                    value: Some(200_000),
                    script_type: ScriptType::P2WPKH,
                    address: Some("bc1qexampleaddress2fakedata".into()),
                    partial_sigs: 0,
                },
            ],
            outputs: vec![
                OutputSummary {
                    index: 0,
                    value: 285_000,
                    script_type: ScriptType::P2WPKH,
                    address: Some("bc1qrecipientaddressfakedata".into()),
                },
                OutputSummary {
                    index: 1,
                    value: 10_000,
                    script_type: ScriptType::P2WPKH,
                    address: Some("bc1qchangeaddressfakedata".into()),
                },
            ],
            fee: FeeInfo::Known(5_000),
            signing_progress: SigningProgress {
                signed_inputs: 1,
                total_inputs: 2,
            },
            warnings: vec!["1 of 2 inputs signed — not ready to broadcast".into()],
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use bitcoin::{
        Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
        absolute::LockTime, psbt::Psbt, transaction::Version,
    };

    /// Build a minimal PSBT with no UTXO data embedded (fee will be Unknown).
    fn make_psbt_no_utxo() -> String {
        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let psbt = Psbt::from_unsigned_tx(tx).expect("valid unsigned tx");
        STANDARD.encode(psbt.serialize())
    }

    #[test]
    fn parse_psbt_rejects_empty_string() {
        assert!(matches!(parse_psbt(""), Err(PsbtError::Empty)));
    }

    #[test]
    fn parse_psbt_rejects_whitespace_only() {
        assert!(matches!(parse_psbt("   "), Err(PsbtError::Empty)));
    }

    #[test]
    fn parse_psbt_rejects_garbage_input() {
        assert!(parse_psbt("this is definitely not a psbt").is_err());
    }

    #[test]
    fn parse_psbt_accepts_valid_base64() {
        assert!(parse_psbt(&make_psbt_no_utxo()).is_ok());
    }

    #[test]
    fn parse_psbt_input_and_output_counts_are_correct() {
        let summary = parse_psbt(&make_psbt_no_utxo()).unwrap();
        assert_eq!(summary.input_count, 1);
        assert_eq!(summary.output_count, 1);
        assert_eq!(summary.inputs.len(), 1);
        assert_eq!(summary.outputs.len(), 1);
    }

    #[test]
    fn fee_is_unknown_when_utxo_data_missing() {
        // Psbt::from_unsigned_tx creates inputs with no witness_utxo or non_witness_utxo.
        let summary = parse_psbt(&make_psbt_no_utxo()).unwrap();
        assert!(matches!(summary.fee, FeeInfo::Unknown));
    }

    #[test]
    fn signing_progress_is_zero_for_unsigned_psbt() {
        let summary = parse_psbt(&make_psbt_no_utxo()).unwrap();
        assert_eq!(summary.signing_progress.signed_inputs, 0);
        assert_eq!(summary.signing_progress.total_inputs, 1);
    }

    #[test]
    fn fake_summary_input_and_output_counts_match_vecs() {
        let s = PsbtSummary::fake();
        assert_eq!(s.input_count, s.inputs.len());
        assert_eq!(s.output_count, s.outputs.len());
    }

    #[test]
    fn fake_summary_fee_is_known() {
        let s = PsbtSummary::fake();
        assert!(matches!(s.fee, FeeInfo::Known(_)));
    }

    #[test]
    fn fake_summary_signing_progress_is_partial() {
        let s = PsbtSummary::fake();
        assert!(s.signing_progress.signed_inputs < s.signing_progress.total_inputs);
    }
}

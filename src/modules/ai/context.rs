use crate::modules::bitcoin::{
    multisig::MultisigInfo,
    psbt::{FeeInfo, PsbtSummary},
};

/// Build a plain-text context string for the AI assistant.
/// Never includes private keys — there are none in these structs.
/// Called from `AppState::draw()` (fast, pure) and `start_ai_query()` before spawning.
pub fn build_context(psbt: Option<&PsbtSummary>, multisig: Option<&MultisigInfo>) -> String {
    let mut parts: Vec<String> = Vec::new();

    match psbt {
        None => parts.push("No PSBT loaded.".into()),
        Some(s) => {
            let fee_str = match &s.fee {
                FeeInfo::Known(sats) => format!("{} sats", sats),
                FeeInfo::Unknown => "unknown".into(),
                FeeInfo::Invalid {
                    input_total,
                    output_total,
                } => format!("INVALID (outputs {output_total} sats > inputs {input_total} sats)"),
            };
            parts.push(format!(
                "PSBT: {} inputs, {} outputs, fee: {}, signing: {}/{}",
                s.input_count,
                s.output_count,
                fee_str,
                s.signing_progress.signed_inputs,
                s.signing_progress.total_inputs,
            ));
            for w in &s.warnings {
                parts.push(format!("Warning: {}", w));
            }
        }
    }

    match multisig {
        None => parts.push("No multisig address built.".into()),
        Some(m) => {
            let net = match m.network {
                bitcoin::Network::Bitcoin => "mainnet",
                bitcoin::Network::Testnet => "testnet",
                bitcoin::Network::Signet => "signet",
                bitcoin::Network::Regtest => "regtest",
                _ => "other",
            };
            parts.push(format!("Multisig address ({}): {}", net, m.address));
            parts.push(format!("Descriptor: {}", m.descriptor));
            parts.push(format!("Keys: {}, {}", m.pubkey1, m.pubkey2));
        }
    }

    parts.join("\n")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::bitcoin::multisig::build_multisig;
    use bitcoin::Network;

    const PK1: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    const PK2: &str = "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";

    #[test]
    fn context_is_non_empty_when_nothing_loaded() {
        let s = build_context(None, None);
        assert!(!s.is_empty());
    }

    #[test]
    fn context_mentions_no_psbt_when_absent() {
        let s = build_context(None, None);
        assert!(s.contains("No PSBT"), "got: {}", s);
    }

    #[test]
    fn context_includes_psbt_counts() {
        let psbt = PsbtSummary::fake();
        let s = build_context(Some(&psbt), None);
        assert!(s.contains("2 inputs"), "got: {}", s);
        assert!(s.contains("2 outputs"), "got: {}", s);
    }

    #[test]
    fn context_includes_fee_when_known() {
        let psbt = PsbtSummary::fake();
        let s = build_context(Some(&psbt), None);
        // fake() has FeeInfo::Known(5_000)
        assert!(s.contains("5000"), "got: {}", s);
    }

    #[test]
    fn context_includes_fee_when_invalid() {
        let mut psbt = PsbtSummary::fake();
        psbt.fee = FeeInfo::Invalid {
            input_total: 10_000,
            output_total: 50_000,
        };
        let s = build_context(Some(&psbt), None);
        assert!(s.contains("INVALID"), "got: {}", s);
        assert!(s.contains("50000"), "got: {}", s);
        assert!(s.contains("10000"), "got: {}", s);
    }

    #[test]
    fn context_includes_multisig_address() {
        let m = build_multisig(PK1, PK2, Network::Testnet, true).unwrap();
        let s = build_context(None, Some(&m));
        assert!(s.contains(&m.address), "got: {}", s);
    }

    #[test]
    fn context_includes_multisig_descriptor() {
        let m = build_multisig(PK1, PK2, Network::Testnet, true).unwrap();
        let s = build_context(None, Some(&m));
        assert!(s.contains("wsh(multi(2,"), "got: {}", s);
    }

    #[test]
    fn context_includes_both_when_both_loaded() {
        let psbt = PsbtSummary::fake();
        let m = build_multisig(PK1, PK2, Network::Testnet, true).unwrap();
        let s = build_context(Some(&psbt), Some(&m));
        assert!(s.contains("inputs"), "got: {}", s);
        assert!(s.contains(&m.address), "got: {}", s);
    }

    #[test]
    fn context_includes_both_keys() {
        let m = build_multisig(PK1, PK2, Network::Testnet, true).unwrap();
        let s = build_context(None, Some(&m));
        assert!(s.contains(&m.pubkey1), "got: {}", s);
        assert!(s.contains(&m.pubkey2), "got: {}", s);
    }
}

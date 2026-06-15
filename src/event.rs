use crate::modules::bitcoin::{
    multisig::{MultisigError, MultisigInfo},
    psbt::{PsbtError, PsbtSummary},
};

/// Events produced by background tasks and sent to the UI loop via mpsc.
/// Keyboard events are handled synchronously and do not pass through this enum.
pub enum AppEvent {
    PsbtParsed(Result<PsbtSummary, PsbtError>),
    MultisigBuilt(Result<MultisigInfo, MultisigError>),
    /// AI events carry the generation ID of the query that produced them, so
    /// the UI loop can drop responses from a stale/cancelled query.
    AiChunk(u64, String),
    AiDone(u64),
    AiError(u64, String),
}

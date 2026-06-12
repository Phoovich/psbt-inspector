use crate::modules::bitcoin::{
    multisig::{MultisigError, MultisigInfo},
    psbt::{PsbtError, PsbtSummary},
};

/// Events produced by background tasks and sent to the UI loop via mpsc.
/// Keyboard events are handled synchronously and do not pass through this enum.
pub enum AppEvent {
    PsbtParsed(Result<PsbtSummary, PsbtError>),
    MultisigBuilt(Result<MultisigInfo, MultisigError>),
    AiChunk(String),
    AiDone,
    AiError(String),
}

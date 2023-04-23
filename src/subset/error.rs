use thiserror::Error;

use std::result;

use crate::binary_agreement;
use crate::broadcast;

/// A subset error.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum Error {
    /// Error creating `BinaryAgreement`.
    #[error("Error creating BinaryAgreement: {0}")]
    NewAgreement(binary_agreement::Error),
    /// Error creating `Broadcast`.
    #[error("Error creating Broadcast: {0}")]
    NewBroadcast(broadcast::Error),
    /// Error handling a `Broadcast` input or message.
    #[error("Error handling Broadcast input/message: {0}")]
    HandleBroadcast(broadcast::Error),
    /// Error handling a `BinaryAgreement` input or message.
    #[error("Error handling BinaryAgreement input/message: {0}")]
    HandleAgreement(binary_agreement::Error),
    /// Unknown proposer.
    #[error("Unknown proposer ID")]
    UnknownProposer,
}

/// A subset result.
pub type Result<T> = result::Result<T, Error>;

/// Faults that can be detected in Subset.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum FaultKind {
    /// `Subset` received a faulty Broadcast message.
    #[error("`Subset` received a faulty Broadcast message.")]
    BroadcastFault(broadcast::FaultKind),
    /// `Subset` received a faulty Binary Agreement message.
    #[error("`Subset` received a faulty Binary Agreement message.")]
    BaFault(binary_agreement::FaultKind),
}

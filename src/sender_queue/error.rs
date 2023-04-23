use std::fmt::Debug;
use thiserror::Error;

/// Sender queue error variants.
#[derive(Debug, Error)]
pub enum Error<E>
where
    E: Debug + std::error::Error,
{
    /// Failed to apply a function to the managed algorithm.
    #[error("Function application failure: {0}")]
    Apply(E),
    /// Failed to restart `DynamicHoneyBadger` because it had not been removed.
    #[error("DynamicHoneyBadger was not removed before restarting")]
    DynamicHoneyBadgerNotRemoved,
    /// Failed to start a new joining `DynamicHoneyBadger`.
    #[error("Failed to start a new joining DynamicHoneyBadger: {0}")]
    DynamicHoneyBadgerNewJoining(E),
}

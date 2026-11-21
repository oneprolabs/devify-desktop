//! Shared chat access policy and the process-wide local-read budget.
pub(crate) mod access;

pub(crate) static LOCAL_READ_BUDGET: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

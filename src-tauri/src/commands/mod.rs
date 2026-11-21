//! Native command adapters grouped by product responsibility.
//! Import owners directly; this module does not re-export command functions.

pub(crate) mod analysis;
pub(crate) mod events;
pub(crate) mod questions;
pub(crate) mod topics;
pub(crate) mod agents;
pub(crate) mod summary_history;

//! Production HTTP egress and opaque secret brokerage.
//!
//! The crate is disabled unless an explicitly enabled [`tm_host::EgressConfig`] is installed.
//! Every hop is re-authorized after DNS resolution, automatic redirects are disabled, budgets are
//! reserved atomically, and secret values are read only while constructing an authorized request.

mod budget;
mod error;
mod host;
mod policy;
mod resolver;
mod runtime;
mod state;

pub use error::*;
pub use host::*;
pub use policy::*;
pub use resolver::*;
pub use runtime::*;
pub use state::*;

#[cfg(test)]
mod tests;

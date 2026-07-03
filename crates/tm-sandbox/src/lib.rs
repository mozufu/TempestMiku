//! Sandbox backends.
//!
//! M0 keeps [`StubSandbox`] for protocol tests. M1 adds [`DenoSandbox`], a
//! `deno_core`-backed persistent JS/TS session with no ambient host I/O.

mod deno;
mod ops;
mod prelude;
mod stub;
mod ts;

#[cfg(test)]
mod tests;

pub use deno::{DenoSandbox, DenoSandboxOptions, DenoSession};
pub use stub::{StubSandbox, StubSession};

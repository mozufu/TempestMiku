//! Implementation of the frozen `tm-conformance-v2` language.
//!
//! Parsing, checking, and pure evaluation live in this crate so the normal
//! Host effects are late-bound through the existing [`tm_host::HostRegistry`].

mod ast;
mod batch;
mod catalog;
mod check;
mod diagnostic;
mod lexer;
mod machine;
mod parser;
mod runtime;
mod sandbox;
mod span;
mod token;
mod value;

pub use ast::*;
pub use catalog::*;
pub use check::*;
pub use diagnostic::*;
pub use lexer::lex;
pub use machine::*;
pub use parser::parse;
pub use runtime::*;
pub use sandbox::*;
pub use span::*;
pub use token::*;
pub use value::*;

/// Frozen source contract implemented by this crate.
pub const CONFORMANCE_VERSION: &str = "tm-conformance-v2";

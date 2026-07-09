mod backend;
mod config;
mod normalize;
mod worker;

pub use backend::OmpAcpBackend;
pub use config::OmpAcpConfig;

#[cfg(test)]
mod tests;

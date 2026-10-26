#![deny(unsafe_op_in_unsafe_fn)]
//! Shared contracts, configuration storage and bounded background services.
pub mod core;
#[cfg(all(windows, feature = "config-store"))]
pub mod diagnostics;
#[cfg(all(windows, feature = "config-store"))]
pub mod services;
#[cfg(feature = "config-store")]
pub mod storage;

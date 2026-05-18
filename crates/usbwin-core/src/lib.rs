//! usbwin-core: the typed pipeline that turns an ISO + a target device into a
//! sequence of write operations.
//!
//! This crate is intentionally OS-agnostic. Concrete device I/O lives in
//! `usbwin-disk`; ISO inspection in `usbwin-iso`; boot-record bytes in
//! `usbwin-boot`. The orchestration here calls into those via traits so the
//! pipeline can run against an in-memory `Vec<u8>` for unit tests just as
//! easily as against `/dev/rdisk8`.

use std::path::PathBuf;
use thiserror::Error;

pub mod device;
pub mod plan;

pub use device::Device;
pub use plan::{BootMode, WritePlan};

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("device refuses to be written: {0}")]
    DeviceRefused(String),

    #[error("ISO does not fit on device (iso={iso_bytes} bytes, device={device_bytes} bytes)")]
    IsoTooLarge { iso_bytes: u64, device_bytes: u64 },

    #[error("ISO classification failed: {0}")]
    IsoClassify(String),

    #[error("boot record write failed: {0}")]
    BootRecord(String),

    #[error("verification failed at offset {offset}: expected {expected:02x?}, got {actual:02x?}")]
    VerifyMismatch { offset: u64, expected: Vec<u8>, actual: Vec<u8> },

    #[error("unsupported boot mode for this ISO: {0}")]
    UnsupportedMode(String),

    #[error("external command failed: {cmd}: {stderr}")]
    External { cmd: String, stderr: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Top-level config built from CLI args.
#[derive(Debug, Clone)]
pub struct Config {
    pub iso_path: PathBuf,
    pub device_path: PathBuf,
    pub mode: ModeRequest,
    pub label: Option<String>,
    pub dry_run: bool,
    pub force: bool,
    pub verify: bool,
    pub verbose: bool,
}

/// What the user asked for at the CLI. `Auto` triggers ISO inspection to
/// pick the actual `BootMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeRequest {
    Auto,
    Windows,
    WindowsXp,
    IsolinuxLinux,
    Hybrid,
    UefiOnly,
}

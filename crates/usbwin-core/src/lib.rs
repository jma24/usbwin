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
    /// XP-mode: directory containing `WaitBT.sys` and `Wait4UFD.sys`.
    /// If present, those drivers are copied to the USB's I386/ and
    /// declared in TXTSETUP.SIF (mitigates 0x7B BSOD on slow-USB-init
    /// hardware). See docs/V0.3_WINDOWS_XP.md chunk 6.
    pub xp_waiters_dir: Option<PathBuf>,
    /// XP-mode: generate a `winnt.sif` answer file at I386/winnt.sif on
    /// the USB. Skips most setup prompts at install time.
    pub xp_unattended: bool,
    /// XP-mode unattended-install fields (used only if `xp_unattended`).
    pub xp_product_key: Option<String>,
    pub xp_computer_name: Option<String>,
    pub xp_full_name: Option<String>,
    /// Which implementation writes the MBR boot code and partition boot
    /// record. `Bootrec` links the native Rust library in-process;
    /// `MsSys` shells out to the upstream tool. See
    /// docs/V1_BOOTREC_LIBRARY.md.
    pub boot_record_impl: BootRecordImpl,
}

/// Backend used to write MBR boot code and the partition boot record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootRecordImpl {
    /// Shell out to the external `ms-sys` binary. Legacy v0.2 path.
    MsSys,
    /// Use the in-process `bootrec` library. Default from v1.0.
    Bootrec,
}

impl BootRecordImpl {
    pub fn as_str(&self) -> &'static str {
        match self {
            BootRecordImpl::MsSys => "ms-sys",
            BootRecordImpl::Bootrec => "bootrec",
        }
    }
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

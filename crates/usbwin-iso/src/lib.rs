//! ISO9660 inspection. Two responsibilities:
//!
//! 1. **Classify** an ISO into one of the four `BootMode` families so the
//!    pipeline knows what to build (auto-mode resolution).
//! 2. **Inspect** an ISO well enough to dry-run the file copy step without
//!    actually mounting it via `hdiutil`.
//!
//! Implementation is intentionally stubbed in this commit. The classification
//! logic per the spec:
//!
//! - Hybrid: protective MBR / GPT signature at offset 0x1FE + EFI System
//!   Partition GUID present in protective MBR area.
//! - Windows: contains `bootmgr` AND `sources/install.wim` at the root.
//! - IsolinuxLinux: contains `isolinux/isolinux.bin`.
//! - UefiOnly: contains `EFI/BOOT/BOOTX64.EFI` (or other UEFI loader path)
//!   AND lacks an MBR signature.
//!
//! The first cut reads the ISO9660 root directory only. Later we'll teach it
//! to walk subdirectories for full pre-flight validation.

use std::path::Path;
use thiserror::Error;
use usbwin_core::plan::BootMode;

#[derive(Debug, Error)]
pub enum IsoError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("not a valid ISO9660 image: {0}")]
    NotIso9660(String),

    #[error("cannot determine boot mode automatically; pass --type explicitly")]
    Ambiguous,
}

pub type Result<T> = std::result::Result<T, IsoError>;

/// Inspect an ISO and return the boot mode that should be used.
///
/// TODO: real ISO9660 parsing. For now this is a placeholder so the workspace
/// compiles and downstream pipeline code can be wired up.
pub fn classify(_path: &Path) -> Result<BootMode> {
    Err(IsoError::Ambiguous)
}

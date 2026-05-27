//! Raw device access. The single chokepoint through which bootsmith touches
//! `/dev/rdiskN`. Lives behind the `Device` trait from `bootsmith-core`.
//!
//! Safety guards (the most important code in the project, after the boot
//! records themselves):
//!
//! - Refuse the boot disk.
//! - Refuse any disk flagged `internal: true` by DiskArbitration.
//! - Refuse disks larger than 256 GiB without `--force`.
//! - Always operate on `/dev/rdiskN`, never `/dev/diskN`.
//!
//! Implementations are gated by `cfg(target_os = ...)`. macOS is the only
//! target in v1; Linux is planned for v2.

use thiserror::Error;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub mod raw;

#[derive(Debug, Error)]
pub enum DiskError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("refusing to write to {0}: this looks like the boot disk")]
    RefusedBootDisk(String),

    #[error("refusing to write to {0}: marked as internal storage")]
    RefusedInternal(String),

    #[error(
        "refusing to write to {device} ({size_gb} GB): exceeds 256 GiB safety threshold. \
         Pass --force if you really mean it."
    )]
    RefusedTooLarge { device: String, size_gb: u64 },

    #[error("device path must be /dev/rdiskN, got: {0}")]
    BadDevicePath(String),

    #[error("DiskArbitration query failed: {0}")]
    DaError(String),

    #[error("external command failed: {cmd}: {stderr}")]
    External { cmd: String, stderr: String },
}

pub type Result<T> = std::result::Result<T, DiskError>;

/// Caller-provided safety overrides. `Default` is the safe configuration.
#[derive(Debug, Clone, Default)]
pub struct SafetyConfig {
    /// Skip the 256 GiB cap and the internal-disk check. CLI flag: `--force`.
    pub force: bool,
}

/// Description of a candidate target device, returned by enumeration so we
/// can show the user a clear confirm prompt.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub path: String,         // e.g. "/dev/rdisk8"
    pub size_bytes: u64,
    pub model: String,        // e.g. "SanDisk Cruzer Blade"
    pub internal: bool,
    pub is_boot_disk: bool,
    pub removable: bool,
}

impl DeviceInfo {
    /// Apply the safety policy. Returns Ok(()) if the device may be written.
    pub fn check_writable(&self, safety: &SafetyConfig) -> Result<()> {
        if self.is_boot_disk {
            return Err(DiskError::RefusedBootDisk(self.path.clone()));
        }
        if self.internal && !safety.force {
            return Err(DiskError::RefusedInternal(self.path.clone()));
        }
        let cap = 256u64 * 1024 * 1024 * 1024;
        if self.size_bytes > cap && !safety.force {
            return Err(DiskError::RefusedTooLarge {
                device: self.path.clone(),
                size_gb: self.size_bytes / 1_000_000_000,
            });
        }
        Ok(())
    }
}

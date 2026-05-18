//! macOS-specific device access. Stubbed in this scaffold commit; the
//! production wiring will land in follow-up commits.
//!
//! Planned layout:
//!
//! - `enumerate()` -> Vec<DeviceInfo> via DiskArbitration framework FFI
//!   (`DASessionCreate`, iterating `DADiskCreateFromBSDName` for each disk).
//! - `RawDevice` struct implementing `usbwin_core::Device` over an open
//!   file descriptor on `/dev/rdiskN`, with `F_NOCACHE` set, 4 MiB buffers,
//!   and `fsync` called at most once at the end.
//! - `unmount(disk)` / `mount(disk)` / `eject(disk)` thin wrappers around
//!   `diskutil`, each with retry + structured error.
//!
//! See docs/PERFORMANCE.md for the perf rules these primitives must obey.

use crate::{DeviceInfo, Result};

/// Enumerate all block devices visible to DiskArbitration. v1 stub.
pub fn enumerate() -> Result<Vec<DeviceInfo>> {
    Ok(Vec::new())
}

/// Look up a single device by BSD name (e.g. "disk8" or "rdisk8"). v1 stub.
///
/// Returns `None` until DiskArbitration enumeration is implemented. The
/// pipeline treats `None` as "unknown device" and refuses to write — a
/// deliberate fail-closed default during early development so an accidental
/// `usbwin foo.iso /dev/disk0` cannot nuke the boot volume even before the
/// guardrails are fully wired.
pub fn info_for(_bsd_name: &str) -> Result<Option<DeviceInfo>> {
    Ok(None)
}

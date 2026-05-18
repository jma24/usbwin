//! macOS device enumeration via `diskutil ... -plist`.
//!
//! The first cut shells out to `diskutil` rather than calling the
//! DiskArbitration framework directly via FFI. Both APIs expose the same
//! underlying data (DiskArbitration is what `diskutil` itself uses); the
//! shell-out is simpler to ship, simpler to test, and the difference in
//! complexity is meaningful (~100 lines of plist parsing vs ~500 lines of
//! CoreFoundation FFI + retain/release bookkeeping).
//!
//! Migration to native DA FFI is tracked as a future issue; the public
//! interface (`enumerate`, `info_for`) won't change.

use crate::{DeviceInfo, DiskError, Result};
use serde::Deserialize;
use std::process::Command;

/// Enumerate all block devices visible to `diskutil list`. Filters down to
/// "whole disks" (e.g. `disk8`, not `disk8s1`) since usbwin only ever writes
/// to whole devices.
pub fn enumerate() -> Result<Vec<DeviceInfo>> {
    let bsd_names = list_all_disks()?;
    let boot_disk_bsd = boot_disk_whole()?;
    let mut out = Vec::with_capacity(bsd_names.len());
    for name in bsd_names {
        match info_for_internal(&name, Some(&boot_disk_bsd)) {
            Ok(Some(info)) => out.push(info),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(disk = %name, error = %e, "skipping disk during enumeration");
            }
        }
    }
    Ok(out)
}

/// Look up a single device by BSD name or by raw `/dev/...` path.
///
/// Accepts any of: `disk8`, `/dev/disk8`, `/dev/rdisk8`, `rdisk8`. Returns
/// `None` if the device is not present (e.g. a stale path).
pub fn info_for(path_or_name: &str) -> Result<Option<DeviceInfo>> {
    let bsd = normalize_bsd_name(path_or_name);
    let boot_disk_bsd = boot_disk_whole().ok();
    info_for_internal(&bsd, boot_disk_bsd.as_deref())
}

/// Strip `/dev/`, leading `r`, and any trailing slice/partition suffix to get
/// the whole-disk BSD name. `"/dev/rdisk8s1"` -> `"disk8"`.
pub fn normalize_bsd_name(path_or_name: &str) -> String {
    let s = path_or_name.trim_start_matches("/dev/");
    let s = s.strip_prefix('r').unwrap_or(s);
    // Trim "sN" (and "sNsM" for APFS) suffixes to get the whole disk.
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if out == "disk" {
            // Now consume digits, stop at first non-digit.
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() {
                    out.push(c);
                    chars.next();
                } else {
                    return out;
                }
            }
            return out;
        }
    }
    out
}

fn info_for_internal(bsd: &str, boot_disk_bsd: Option<&str>) -> Result<Option<DeviceInfo>> {
    let plist_bytes = match run_diskutil_info(bsd) {
        Ok(b) => b,
        Err(DiskError::External { .. }) => return Ok(None), // disk doesn't exist
        Err(e) => return Err(e),
    };
    let info: DiskutilInfo = plist::from_bytes(&plist_bytes).map_err(|e| DiskError::DaError(
        format!("parsing diskutil info plist for {bsd}: {e}"),
    ))?;
    if !info.whole_disk {
        // Skip slices/partitions; we only write to whole disks.
        return Ok(None);
    }
    let path = format!("/dev/r{bsd}");
    // `IORegistryEntryName` is the human-readable disk label ("SanDisk
    // Extreme Media"). `MediaName` is the short product name ("Extreme").
    // Prefer the IORegistry label; fall back to MediaName; final fallback
    // is a generic descriptor.
    let model = info
        .io_registry_entry_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or(info.media_name.as_deref().filter(|s| !s.trim().is_empty()))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "(unknown model)".to_string());
    Ok(Some(DeviceInfo {
        path,
        size_bytes: info.size,
        model,
        internal: info.internal,
        is_boot_disk: boot_disk_bsd.map_or(false, |b| b == bsd),
        removable: info.removable_media,
    }))
}

/// Plist struct for `diskutil info -plist <bsd>`. Field names match the
/// dictionary keys diskutil emits.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskutilInfo {
    #[serde(default)]
    whole_disk: bool,
    #[serde(default, rename = "Size")]
    size: u64,
    #[serde(default)]
    internal: bool,
    #[serde(default, rename = "RemovableMedia")]
    removable_media: bool,
    #[serde(default, rename = "IORegistryEntryName")]
    io_registry_entry_name: Option<String>,
    #[serde(default, rename = "MediaName")]
    media_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskutilList {
    all_disks: Vec<String>,
}

fn list_all_disks() -> Result<Vec<String>> {
    let bytes = run_command("diskutil", &["list", "-plist"])?;
    let parsed: DiskutilList = plist::from_bytes(&bytes).map_err(|e| {
        DiskError::DaError(format!("parsing diskutil list plist: {e}"))
    })?;
    // Keep only whole-disk entries matching /^disk\d+$/. Slices (diskNsM,
    // diskNsMsK for APFS) are not write targets for usbwin.
    Ok(parsed
        .all_disks
        .into_iter()
        .filter(|d| is_whole_disk_name(d))
        .collect())
}

fn is_whole_disk_name(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("disk") else { return false };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

fn run_diskutil_info(bsd: &str) -> Result<Vec<u8>> {
    run_command("diskutil", &["info", "-plist", bsd])
}

fn run_command(cmd: &str, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new(cmd).args(args).output().map_err(DiskError::Io)?;
    if !output.status.success() {
        return Err(DiskError::External {
            cmd: format!("{cmd} {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    Ok(output.stdout)
}

/// Identify the whole-disk BSD name backing `/` (the boot volume).
///
/// `statfs("/")` gives us a mount-from path like `/dev/disk3s1s1` (APFS sealed
/// system volume). We strip back to the whole disk and return its BSD name.
fn boot_disk_whole() -> Result<String> {
    use nix::libc;
    use std::ffi::{CStr, CString};
    use std::mem::MaybeUninit;

    let path = CString::new("/").unwrap();
    let mut buf: MaybeUninit<libc::statfs> = MaybeUninit::uninit();
    // SAFETY: statfs writes a valid statfs struct on success; we check the
    // return code before calling assume_init.
    let rc = unsafe { libc::statfs(path.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(DiskError::DaError(format!("statfs(\"/\") failed: {err}")));
    }
    // SAFETY: rc == 0, so statfs initialized the buffer.
    let stat = unsafe { buf.assume_init() };
    let mnt = unsafe { CStr::from_ptr(stat.f_mntfromname.as_ptr()) };
    Ok(normalize_bsd_name(&mnt.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_bsd_name_handles_common_inputs() {
        assert_eq!(normalize_bsd_name("disk8"), "disk8");
        assert_eq!(normalize_bsd_name("rdisk8"), "disk8");
        assert_eq!(normalize_bsd_name("/dev/disk8"), "disk8");
        assert_eq!(normalize_bsd_name("/dev/rdisk8"), "disk8");
        assert_eq!(normalize_bsd_name("/dev/disk3s1"), "disk3");
        assert_eq!(normalize_bsd_name("/dev/disk3s1s1"), "disk3");
        assert_eq!(normalize_bsd_name("/dev/rdisk12s2"), "disk12");
    }

    #[test]
    fn normalize_bsd_name_preserves_multidigit() {
        assert_eq!(normalize_bsd_name("/dev/disk100"), "disk100");
    }
}

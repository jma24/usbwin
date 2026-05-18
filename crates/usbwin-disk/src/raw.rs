//! `RawDevice`: open a `/dev/rdiskN` and implement the `Device` trait over it.
//!
//! Performance rules (see docs/PERFORMANCE.md):
//! - Always operate on `/dev/rdiskN` (raw character device), never `/dev/diskN`.
//! - Set `F_NOCACHE` so writes bypass the unified buffer cache. We're writing
//!   exactly once; caching gives us nothing and costs throughput.
//! - Reads and writes are sector-aligned. macOS's raw device requires this;
//!   passing a non-aligned offset or length will error with EINVAL.
//! - Buffer sizes are caller-driven, but the pipeline uses 4 MiB chunks.
//!
//! Verification: `Device::read_at` reads the raw device, so the `verify_at`
//! helper on `usbwin-core::device` just works against the same bytes the
//! BIOS will see at boot time.

use crate::{DiskError, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use usbwin_core::Device;

/// Open mode for `RawDevice`. Read-only for dry-run / verify-only paths;
/// ReadWrite for the actual write pipeline.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum OpenMode {
    ReadOnly,
    ReadWrite,
}

/// A raw block device opened on `/dev/rdiskN`. Implements `Device`.
pub struct RawDevice {
    file: File,
    path: PathBuf,
    size_bytes: u64,
    block_size: u32,
    label: String,
}

impl RawDevice {
    /// Open the raw device at `path`. `path` must be a `/dev/rdiskN` style
    /// path — passing `/dev/diskN` (cached buffered device) is rejected.
    pub fn open(path: impl AsRef<Path>, mode: OpenMode, model_label: &str) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let path_str = path.to_string_lossy().into_owned();
        if !path_str.starts_with("/dev/rdisk") {
            return Err(DiskError::BadDevicePath(path_str));
        }

        let mut opts = OpenOptions::new();
        opts.read(true);
        if mode == OpenMode::ReadWrite {
            opts.write(true);
        }
        // O_EXCL would prevent re-opening; we want to fail if any other
        // process holds it (e.g. Finder, diskutil background). On macOS
        // O_EXCL on a block device requires the BLOCK_DEVICE_EXCLUSIVE
        // semantic which doesn't apply here; we rely on diskutil
        // unmountDisk + the raw device's open-arbitration instead.
        opts.custom_flags(nix::libc::O_SYNC);

        let file = opts.open(&path).map_err(DiskError::Io)?;

        // F_NOCACHE: tell the kernel we don't want this data cached. We're
        // writing the bytes exactly once.
        set_fnocache(&file)?;

        let (block_size, block_count) = block_geometry(&file, &path_str)?;
        let size_bytes = block_size as u64 * block_count;

        Ok(Self {
            file,
            path,
            size_bytes,
            block_size,
            label: format!("{path_str} ({model_label})"),
        })
    }

    /// Reported sector size (typically 512 or 4096).
    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    /// Path the device was opened on.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Device for RawDevice {
    fn size_bytes(&self) -> usbwin_core::Result<u64> {
        Ok(self.size_bytes)
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> usbwin_core::Result<()> {
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(usbwin_core::Error::Io)?;
        self.file.read_exact(buf).map_err(usbwin_core::Error::Io)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> usbwin_core::Result<()> {
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(usbwin_core::Error::Io)?;
        self.file.write_all(buf).map_err(usbwin_core::Error::Io)
    }

    fn sync(&mut self) -> usbwin_core::Result<()> {
        self.file.flush().map_err(usbwin_core::Error::Io)?;
        // fsync to push through any kernel-side buffering. F_NOCACHE means
        // the buffer cache is bypassed, but the device driver may still
        // hold writes in flight.
        let fd = self.file.as_raw_fd();
        // SAFETY: fd is owned by self.file and valid for the duration of
        // the call.
        let rc = unsafe { nix::libc::fsync(fd) };
        if rc != 0 {
            return Err(usbwin_core::Error::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}

fn set_fnocache(file: &File) -> Result<()> {
    let fd = file.as_raw_fd();
    // F_NOCACHE = 48 on macOS; defined in <sys/fcntl.h>.
    const F_NOCACHE: nix::libc::c_int = 48;
    // SAFETY: fd is valid for the duration of this call; F_NOCACHE accepts
    // an int argument (1 to enable).
    let rc = unsafe { nix::libc::fcntl(fd, F_NOCACHE, 1 as nix::libc::c_int) };
    if rc < 0 {
        let err = std::io::Error::last_os_error();
        return Err(DiskError::DaError(format!("fcntl(F_NOCACHE): {err}")));
    }
    Ok(())
}

/// Query block size and block count via `DKIOCGETBLOCKSIZE` (0x40046418)
/// and `DKIOCGETBLOCKCOUNT` (0x40086419) ioctls.
fn block_geometry(file: &File, label: &str) -> Result<(u32, u64)> {
    let fd = file.as_raw_fd();

    // The DKIOC ioctl numbers are stable in Apple's <sys/disk.h>:
    //   #define DKIOCGETBLOCKSIZE  _IOR('d', 24, uint32_t)
    //   #define DKIOCGETBLOCKCOUNT _IOR('d', 25, uint64_t)
    // _IOR(g,n,t) on macOS = IOC_OUT | ((sizeof(t)&IOCPARM_MASK)<<16)
    //                       | (g<<8) | n
    // IOC_OUT = 0x40000000, IOCPARM_MASK = 0x1fff, g = 'd' = 0x64.
    const DKIOCGETBLOCKSIZE: nix::libc::c_ulong = 0x40046418;
    const DKIOCGETBLOCKCOUNT: nix::libc::c_ulong = 0x40086419;

    let mut block_size: u32 = 0;
    // SAFETY: fd is valid; the ioctl writes a u32 into the output pointer.
    let rc = unsafe {
        nix::libc::ioctl(fd, DKIOCGETBLOCKSIZE, &mut block_size as *mut u32)
    };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(DiskError::DaError(format!(
            "DKIOCGETBLOCKSIZE on {label}: {err}"
        )));
    }

    let mut block_count: u64 = 0;
    // SAFETY: fd is valid; the ioctl writes a u64 into the output pointer.
    let rc = unsafe {
        nix::libc::ioctl(fd, DKIOCGETBLOCKCOUNT, &mut block_count as *mut u64)
    };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(DiskError::DaError(format!(
            "DKIOCGETBLOCKCOUNT on {label}: {err}"
        )));
    }

    Ok((block_size, block_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_rejects_buffered_device_path() {
        // /dev/diskN is the cached/buffered device; we only accept /dev/rdiskN.
        match RawDevice::open("/dev/disk8", OpenMode::ReadOnly, "test") {
            Err(DiskError::BadDevicePath(p)) => assert_eq!(p, "/dev/disk8"),
            Err(other) => panic!("wrong error: {other:?}"),
            Ok(_) => panic!("expected BadDevicePath rejection"),
        }
    }

    #[test]
    fn open_rejects_non_dev_path() {
        match RawDevice::open("/tmp/notadevice", OpenMode::ReadOnly, "test") {
            Err(DiskError::BadDevicePath(_)) => {}
            Err(other) => panic!("wrong error: {other:?}"),
            Ok(_) => panic!("expected BadDevicePath rejection"),
        }
    }
}

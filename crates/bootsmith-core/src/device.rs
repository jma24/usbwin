//! The `Device` trait: the only abstraction every other crate is allowed to use
//! when it needs to write to "a thing." The macOS raw-block-device impl lives
//! in `bootsmith-disk`; in tests we substitute an in-memory `Vec<u8>` impl so
//! `cargo test` works without root, without a USB stick, and without macOS.

use crate::Result;

pub trait Device: Send {
    /// Total addressable bytes of the underlying device.
    fn size_bytes(&self) -> Result<u64>;

    /// Read exactly `buf.len()` bytes at `offset`. Errors if read short.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()>;

    /// Write exactly `buf.len()` bytes at `offset`. Errors if write short.
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<()>;

    /// Force any buffered writes to the physical medium.
    fn sync(&mut self) -> Result<()>;

    /// Human-readable identifier for log messages and the confirm prompt.
    /// e.g. "/dev/rdisk8 (64 GB SanDisk Cruzer)" or "in-memory test device".
    fn describe(&self) -> String;
}

/// Read `len` bytes at `offset` into a freshly-allocated `Vec<u8>`. Convenience
/// wrapper around `Device::read_at`.
pub fn read_vec<D: Device + ?Sized>(dev: &mut D, offset: u64, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    dev.read_at(offset, &mut buf)?;
    Ok(buf)
}

/// Write `buf` at `offset`, then re-read and verify byte-equal. Returns
/// `Error::VerifyMismatch` on the first divergence.
///
/// This is the "verify-by-default" primitive. Callers that legitimately want
/// to skip verification (e.g. mid-pipeline writes that will be re-checked
/// holistically later) call `write_at` directly.
pub fn write_and_verify<D: Device + ?Sized>(
    dev: &mut D,
    offset: u64,
    buf: &[u8],
) -> Result<()> {
    dev.write_at(offset, buf)?;
    let mut check = vec![0u8; buf.len()];
    dev.read_at(offset, &mut check)?;
    if check != buf {
        let first_bad = check
            .iter()
            .zip(buf.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        return Err(crate::Error::VerifyMismatch {
            offset: offset + first_bad as u64,
            expected: buf[first_bad..(first_bad + 16).min(buf.len())].to_vec(),
            actual: check[first_bad..(first_bad + 16).min(check.len())].to_vec(),
        });
    }
    Ok(())
}

/// In-memory `Device` impl used by unit tests. Backed by a `Vec<u8>`.
pub struct MemoryDevice {
    pub bytes: Vec<u8>,
    pub label: String,
}

impl MemoryDevice {
    pub fn new(size: u64) -> Self {
        Self {
            bytes: vec![0u8; size as usize],
            label: format!("in-memory device ({size} bytes)"),
        }
    }
}

impl Device for MemoryDevice {
    fn size_bytes(&self) -> Result<u64> {
        Ok(self.bytes.len() as u64)
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset as usize + buf.len();
        if end > self.bytes.len() {
            return Err(crate::Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "read past end of memory device",
            )));
        }
        buf.copy_from_slice(&self.bytes[offset as usize..end]);
        Ok(())
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<()> {
        let end = offset as usize + buf.len();
        if end > self.bytes.len() {
            return Err(crate::Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "write past end of memory device",
            )));
        }
        self.bytes[offset as usize..end].copy_from_slice(buf);
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        Ok(())
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_device_round_trip() {
        let mut d = MemoryDevice::new(4096);
        write_and_verify(&mut d, 100, b"hello world").unwrap();
        assert_eq!(&d.bytes[100..111], b"hello world");
    }

    #[test]
    fn memory_device_rejects_past_end() {
        let mut d = MemoryDevice::new(16);
        assert!(d.write_at(8, b"more than 8 bytes").is_err());
    }
}

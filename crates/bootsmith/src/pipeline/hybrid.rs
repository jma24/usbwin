//! Hybrid-ISO mode: raw write of ISO -> device. The simplest of the four
//! modes; no boot-record manipulation, no partition table writing, just
//! the bytes of the ISO followed by a verify pass.
//!
//! Compatible with any "hybrid ISO" (most modern Linux/BSD distros) and
//! with bare disk images. For non-hybrid ISOs it'll write the bytes too,
//! but the result won't boot - which is why auto-classification matters
//! and why we encourage `--type=hybrid` to be explicit.

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::File;
use std::io::Read;
use std::time::Duration;
use bootsmith_core::{Device, WritePlan};
use bootsmith_disk::raw::{OpenMode, RawDevice};
use bootsmith_disk::DeviceInfo;

use super::diskutil;

const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4 MiB

pub fn run(plan: &WritePlan, info: &DeviceInfo, verify: bool) -> Result<()> {
    // 1. Unmount the whole disk so nothing's writing while we are.
    let bsd_path = info.path.replace("/dev/r", "/dev/");
    diskutil::unmount_disk(&bsd_path).context("unmount before raw write")?;

    // 2. Open the raw device. We re-use info.model for the label so error
    //    messages and verify mismatches print something human-readable.
    let mut dev = RawDevice::open(&info.path, OpenMode::ReadWrite, &info.model)
        .context("opening raw device for write")?;

    // 3. Stream ISO -> device in CHUNK_SIZE chunks.
    let mut iso = File::open(&plan.iso_path)
        .with_context(|| format!("opening ISO {}", plan.iso_path.display()))?;

    let pb = progress_bar(plan.iso_bytes, "writing");
    write_iso_to_device(&mut iso, &mut dev, plan.iso_bytes, &pb)
        .context("writing ISO bytes")?;
    dev.sync().map_err(anyhow_from_core)?;
    pb.finish_with_message("write complete");

    // 4. Verify (re-read the device, compare against the ISO byte-for-byte).
    if verify {
        let mut iso = File::open(&plan.iso_path)
            .with_context(|| format!("re-opening ISO for verify {}", plan.iso_path.display()))?;
        let pb = progress_bar(plan.iso_bytes, "verifying");
        verify_device_matches_iso(&mut iso, &mut dev, plan.iso_bytes, &pb)
            .context("verify pass")?;
        pb.finish_with_message("verify ok");
    } else {
        tracing::warn!("--no-verify: skipping read-back verification");
    }

    // 5. Eject. macOS aggressively auto-remounts the disk once the verify
    //    pass completes (the freshly-written ISO has a recognizable
    //    filesystem signature). Unmount first so eject doesn't trip over
    //    a re-mount race. We ignore the unmount error - if nothing is
    //    mounted that's fine; if something is mounted that we can't unmount,
    //    eject will surface a clearer error in a moment anyway.
    let _ = diskutil::unmount_disk(&bsd_path);
    diskutil::eject(&bsd_path).context("eject after write")?;

    println!("\nbootsmith: {} -> {} OK", plan.iso_path.display(), info.path);
    Ok(())
}

fn write_iso_to_device(
    iso: &mut File,
    dev: &mut RawDevice,
    total: u64,
    pb: &ProgressBar,
) -> Result<()> {
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut offset: u64 = 0;
    loop {
        let n = read_full(iso, &mut buf)?;
        if n == 0 {
            break;
        }
        // Pad the tail of the last chunk to a sector boundary - raw device
        // writes must be a multiple of the block size.
        let bs = dev.block_size() as usize;
        let aligned = if n % bs == 0 { n } else { ((n / bs) + 1) * bs };
        if aligned > n {
            for b in &mut buf[n..aligned] {
                *b = 0;
            }
        }
        dev.write_at(offset, &buf[..aligned]).map_err(anyhow_from_core)?;
        offset += n as u64;
        pb.set_position(offset.min(total));
        if (n as usize) < CHUNK_SIZE {
            break;
        }
    }
    Ok(())
}

fn verify_device_matches_iso(
    iso: &mut File,
    dev: &mut RawDevice,
    total: u64,
    pb: &ProgressBar,
) -> Result<()> {
    let mut iso_buf = vec![0u8; CHUNK_SIZE];
    let mut dev_buf = vec![0u8; CHUNK_SIZE];
    let bs = dev.block_size() as usize;
    let mut offset: u64 = 0;
    loop {
        let n = read_full(iso, &mut iso_buf)?;
        if n == 0 {
            break;
        }
        let aligned = if n % bs == 0 { n } else { ((n / bs) + 1) * bs };
        dev.read_at(offset, &mut dev_buf[..aligned])
            .map_err(anyhow_from_core)?;
        if dev_buf[..n] != iso_buf[..n] {
            let first_bad = (0..n)
                .find(|&i| dev_buf[i] != iso_buf[i])
                .unwrap_or(0);
            anyhow::bail!(
                "verify mismatch at offset {} (ISO byte 0x{:02x}, device byte 0x{:02x})",
                offset + first_bad as u64,
                iso_buf[first_bad],
                dev_buf[first_bad]
            );
        }
        offset += n as u64;
        pb.set_position(offset.min(total));
        if (n as usize) < CHUNK_SIZE {
            break;
        }
    }
    Ok(())
}

/// Read up to buf.len() bytes; returns 0 at EOF.
fn read_full(reader: &mut File, buf: &mut [u8]) -> Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        let n = reader.read(&mut buf[total..]).context("reading ISO")?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

fn progress_bar(total: u64, op: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "  {prefix:<6} {wide_bar:.cyan/blue} {bytes:>10}/{total_bytes:<10} @ {bytes_per_sec:>10}  ETA {eta:>5}",
        )
        .unwrap()
        .progress_chars("█▓▒░ "),
    );
    pb.set_prefix(op.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

fn anyhow_from_core(e: bootsmith_core::Error) -> anyhow::Error {
    anyhow::Error::new(e)
}

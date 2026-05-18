//! The "are you sure" prompt. Shown once, requires literal "yes" to proceed.
//!
//! Why "yes" instead of "y": typing one letter is a reflex; typing three
//! letters is a decision. The 256 GiB cap, the boot-disk guard, and the
//! internal-disk guard are all earlier in the chain; this prompt is the
//! last human-readable checkpoint between intent and a formatted USB.

use anyhow::{bail, Result};
use std::io::{self, BufRead, Write};
use usbwin_core::WritePlan;
use usbwin_disk::DeviceInfo;

pub fn prompt(plan: &WritePlan, info: &DeviceInfo) -> Result<()> {
    let gb = info.size_bytes as f64 / 1_000_000_000.0;
    let iso_gb = plan.iso_bytes as f64 / 1_000_000_000.0;

    println!();
    println!("usbwin is about to write:");
    println!("  ISO    : {} ({iso_gb:.2} GB)", plan.iso_path.display());
    println!("  Mode   : {}", plan.mode.as_str());
    println!("  Label  : {}", plan.label);
    println!();
    println!("To target:");
    println!("  Device : {}", info.path);
    println!("  Size   : {gb:.2} GB");
    println!("  Model  : {}", info.model);
    println!(
        "  Flags  : internal={} removable={} boot_disk={}",
        info.internal, info.removable, info.is_boot_disk
    );
    println!();
    println!("THIS WILL ERASE EVERYTHING ON {}.", info.path);
    print!("Type \"yes\" to continue: ");
    io::stdout().flush().ok();

    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| anyhow::anyhow!("reading confirmation: {e}"))?;

    if line.trim() != "yes" {
        bail!("aborted by user");
    }
    Ok(())
}

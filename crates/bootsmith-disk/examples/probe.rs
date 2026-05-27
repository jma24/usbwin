//! Manually verify enumeration + raw device open on the host.
//! `cargo run --example probe -p bootsmith-disk -- [/dev/rdiskN]`
//!
//! If a device path is passed, opens it read-only via RawDevice and prints
//! block geometry + the first 16 bytes. Read-only is safe (no writes).

#[cfg(target_os = "macos")]
fn main() {
    use std::env;
    use bootsmith_core::Device;
    use bootsmith_disk::raw::{OpenMode, RawDevice};

    let devices = bootsmith_disk::macos::enumerate().expect("enumeration failed");
    println!("found {} whole disk(s):", devices.len());
    for d in &devices {
        println!(
            "  {} - {} bytes ({}.{} GB), model={}, internal={}, boot={}, removable={}",
            d.path,
            d.size_bytes,
            d.size_bytes / 1_000_000_000,
            (d.size_bytes / 100_000_000) % 10,
            d.model,
            d.internal,
            d.is_boot_disk,
            d.removable,
        );
    }

    if let Some(target) = env::args().nth(1) {
        println!("\nopening {target} read-only:");
        let info = bootsmith_disk::macos::info_for(&target)
            .expect("lookup failed")
            .unwrap_or_else(|| panic!("no such device: {target}"));
        let mut raw = RawDevice::open(&info.path, OpenMode::ReadOnly, &info.model)
            .expect("RawDevice::open failed");
        println!("  block_size={}, size_bytes={}", raw.block_size(), raw.size_bytes().unwrap());
        let mut buf = vec![0u8; 512];
        raw.read_at(0, &mut buf).expect("read failed");
        print!("  first 16 bytes: ");
        for b in &buf[..16] {
            print!("{b:02x} ");
        }
        println!();
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("probe is macOS-only");
}

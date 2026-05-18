//! Manually verify enumeration on the host.
//! `cargo run --example probe -p usbwin-disk`

#[cfg(target_os = "macos")]
fn main() {
    let devices = usbwin_disk::macos::enumerate().expect("enumeration failed");
    println!("found {} whole disk(s):", devices.len());
    for d in devices {
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
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("probe is macOS-only");
}

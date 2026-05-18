//! QEMU smoke test for the FAT32 PBR boot code.
//!
//! Ignored by default. Run with:
//!
//!     cargo test -p usbwin-boot --test qemu_pbr --features embed-boot-asm -- --ignored
//!
//! Requires:
//!   - `nasm` to assemble the boot blobs and the fake-bootmgr stub
//!   - `qemu-system-i386` to actually boot the image
//!   - macOS (we use `hdiutil`, `newfs_msdos`, and Apple's `cp`)
//!
//! Flow:
//!   1. Build the fake bootmgr (NASM, prints "USBWIN OK\n" to COM1, halts).
//!   2. Create a 64 MiB raw FAT32 disk image with the fake bootmgr at root.
//!   3. Read the freshly-formatted PBR, splice in our FAT32 boot blob using
//!      `splice_fat32_pbr` (preserving the BPB), write it back.
//!   4. Boot the image under qemu-system-i386 with -serial stdio -nographic.
//!   5. Read serial output. Pass if it contains "USBWIN OK".
//!
//! This is the production verification loop for `fat32_pbr.asm` — when this
//! test passes, our PBR is byte-correct enough to chain-load an x86 binary
//! named BOOTMGR from a FAT32 volume. That's the contract.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const IMAGE_BYTES: u64 = 64 * 1024 * 1024;

#[test]
#[ignore]
fn fat32_pbr_loads_bootmgr_in_qemu() {
    if let Err(reason) = check_dependencies() {
        eprintln!("skipping qemu test: {reason}");
        return;
    }

    if usbwin_boot::FAT32_PBR_BOOT.is_empty() {
        panic!(
            "FAT32 PBR boot blob is empty (built without --features embed-boot-asm). \
             Re-run: cargo test -p usbwin-boot --test qemu_pbr --features embed-boot-asm -- --ignored"
        );
    }

    let workspace_root = workspace_root();
    let boot_asm = workspace_root.join("boot-asm");

    let fake_bootmgr = build_fake_bootmgr(&boot_asm).expect("building fake_bootmgr.bin");

    let tmp = tempdir();
    let image = tmp.join("usbwin-test.img");
    create_fat32_image(&image, &fake_bootmgr).expect("creating FAT32 image");
    splice_our_pbr(&image).expect("splicing usbwin PBR");

    let serial = boot_under_qemu(&image).expect("running qemu");
    assert!(
        serial.contains("USBWIN OK"),
        "qemu serial output missing 'USBWIN OK'. Got:\n---\n{serial}\n---"
    );
}

fn check_dependencies() -> Result<(), String> {
    // mtools (mformat, mcopy) lets us build a canonical FAT32 image without
    // going through macOS's FAT32 driver, which writes some non-trivial
    // metadata (.fseventsd directory, async writes) that complicates the
    // round-trip test.
    for tool in &["nasm", "qemu-system-i386", "mformat", "mcopy"] {
        which(tool).map_err(|e| format!("missing `{tool}`: {e}"))?;
    }
    Ok(())
}

fn which(tool: &str) -> Result<(), String> {
    let out = Command::new("/usr/bin/env")
        .args(["which", tool])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("`{tool}` not found in PATH"));
    }
    Ok(())
}

fn workspace_root() -> PathBuf {
    // crates/usbwin-boot/tests/qemu_pbr.rs -> walk up to workspace root
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap().parent().unwrap().to_path_buf()
}

fn build_fake_bootmgr(boot_asm: &Path) -> Result<PathBuf, String> {
    let status = Command::new("make")
        .args(["test-fixtures"])
        .current_dir(boot_asm)
        .status()
        .map_err(|e| format!("invoking make in {}: {e}", boot_asm.display()))?;
    if !status.success() {
        return Err("`make test-fixtures` failed".to_string());
    }
    let out = boot_asm.join("build").join("fake_bootmgr.bin");
    if !out.exists() {
        return Err(format!("expected output {} missing", out.display()));
    }
    Ok(out)
}

fn create_fat32_image(image: &Path, fake_bootmgr: &Path) -> Result<(), String> {
    // 1. Allocate the raw image as zeros.
    let f = std::fs::File::create(image).map_err(|e| format!("create image: {e}"))?;
    f.set_len(IMAGE_BYTES).map_err(|e| format!("set_len: {e}"))?;
    drop(f);

    // 2. Format as FAT32 via mformat (does not require root, no mount, no
    //    macOS auto-mount races, no .fseventsd metadata to confuse the
    //    bootloader test).
    let fmt = Command::new("mformat")
        .args(["-F", "-i"])
        .arg(image)
        .args(["-v", "USBWIN", "::"])
        .output()
        .map_err(|e| format!("mformat: {e}"))?;
    if !fmt.status.success() {
        return Err(format!(
            "mformat failed: {}",
            String::from_utf8_lossy(&fmt.stderr)
        ));
    }

    // 3. Copy fake bootmgr to root as BOOTMGR.
    let cp = Command::new("mcopy")
        .arg("-i")
        .arg(image)
        .arg(fake_bootmgr)
        .arg("::BOOTMGR")
        .output()
        .map_err(|e| format!("mcopy: {e}"))?;
    if !cp.status.success() {
        return Err(format!(
            "mcopy failed: {}",
            String::from_utf8_lossy(&cp.stderr)
        ));
    }

    Ok(())
}

fn splice_our_pbr(image: &Path) -> Result<(), String> {
    use std::fs::OpenOptions;
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(image)
        .map_err(|e| format!("opening image for splice: {e}"))?;
    let mut existing = [0u8; 512];
    file.read_exact(&mut existing)
        .map_err(|e| format!("reading existing PBR: {e}"))?;
    let spliced = usbwin_boot::splice_fat32_pbr(&existing, usbwin_boot::FAT32_PBR_BOOT)
        .map_err(|e| format!("splice_fat32_pbr: {e}"))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| format!("seek: {e}"))?;
    file.write_all(&spliced)
        .map_err(|e| format!("writing spliced PBR: {e}"))?;
    Ok(())
}

fn boot_under_qemu(image: &Path) -> Result<String, String> {
    use std::io::Read;
    use std::process::Stdio;

    // Attach as an HDD (if=ide). INT 13h extension function 0x42 (which
    // our PBR uses) is only valid for drive numbers >= 0x80 (HDDs); BIOS
    // refuses it on floppies, so we must not use if=floppy here.
    let drive = format!("file={},format=raw,if=ide", image.display());
    let mut child = Command::new("qemu-system-i386")
        .args(["-drive", &drive])
        .args([
            "-boot", "c",
            "-serial", "stdio",
            "-display", "none",
            "-no-reboot",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("spawning qemu: {e}"))?;

    // Drain stdout on a background thread; it'll terminate when qemu's stdout
    // closes (process exit or kill).
    let stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut r = stdout;
        let _ = r.read_to_string(&mut buf);
        buf
    });

    // Give qemu up to 10 seconds to print and halt. `hlt` in real mode
    // doesn't terminate qemu by itself, so we kill the process after the
    // deadline regardless. The reader thread will then see EOF.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("qemu wait error: {e}")),
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    let serial = reader.join().unwrap_or_default();
    Ok(serial)
}

fn tempdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("usbwin-qemu-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

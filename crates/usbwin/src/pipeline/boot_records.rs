//! Pure byte-producing functions for the MBR + FAT32 PBR. The Windows and
//! XP pipelines call these to compute the bytes they then hand to
//! `Device::write_at`. Separated out so golden tests can exercise the
//! integration with `bootrec` (which `cargo test` re-runs on every bump)
//! without needing a real USB stick.
//!
//! Conventions:
//!   - "Reserved area" = the formatter-written sectors at the start of the
//!     partition that the splice preserves the BPB / FSInfo of. For Win 7
//!     this is 1024 bytes (sector 0 + sector 1); for XP, 512 bytes.
//!   - All functions are infallible-modulo-input-validation: bootrec is
//!     the source of truth for byte layout, and any error from it
//!     propagates with context.
//!
//! Why not just call bootrec inline from windows.rs / windows_xp.rs?
//! Decoupling the byte production from the I/O makes golden testing
//! tractable. The actual write-to-device still lives in the pipeline.

use anyhow::{anyhow, Result};

const SECTOR_SIZE: u64 = 512;

/// Win 7+ MBR (sector 0 of the whole disk). 512 bytes.
/// Includes boot code, disk signature, partition table, and 0xAA55 signature.
pub fn build_mbr_win7(disk_size_bytes: u64) -> Result<Vec<u8>> {
    let disk_sectors = disk_size_bytes / SECTOR_SIZE;
    bootrec::mbr_win7(disk_sectors)
        .map(|arr| arr.to_vec())
        .map_err(|e| anyhow!("bootrec::mbr_win7: {e}"))
}

/// Win 2000/XP/2003 MBR (sector 0 of the whole disk). 512 bytes.
/// Layout matches `build_mbr_win7` but the boot code is the XP-era variant.
pub fn build_mbr_xp(disk_size_bytes: u64) -> Result<Vec<u8>> {
    let disk_sectors = disk_size_bytes / SECTOR_SIZE;
    bootrec::mbr_xp(disk_sectors)
        .map(|arr| arr.to_vec())
        .map_err(|e| anyhow!("bootrec::mbr_xp: {e}"))
}

/// Win 7+ multi-sector FAT32 PBR (BOOTMGR-loading). Takes the formatter's
/// first 1024 bytes (sector 0 + sector 1) and returns the spliced output:
/// BPB at bytes 3..90 preserved, FSInfo at LBA 1 preserved, stage 2 at LBA 2.
pub fn splice_pbr_bootmgr(formatter_reserved: &[u8]) -> Result<Vec<u8>> {
    bootrec::splice_fat32_pbr_multi(
        formatter_reserved,
        bootrec::FAT32_PBR_BOOTMGR_MULTI_BOOT,
    )
    .map_err(|e| anyhow!("bootrec::splice_fat32_pbr_multi: {e}"))
}

/// XP single-sector FAT32 PBR (NTLDR-loading). Takes the formatter's
/// sector 0 (512 bytes) and returns 512 bytes with the BPB preserved
/// and the boot code overwritten with the NTLDR variant.
pub fn splice_pbr_ntldr(formatter_sector0: &[u8]) -> Result<Vec<u8>> {
    bootrec::splice_fat32_pbr(formatter_sector0, bootrec::FAT32_PBR_NTLDR_BOOT)
        .map(|arr| arr.to_vec())
        .map_err(|e| anyhow!("bootrec::splice_fat32_pbr (NTLDR): {e}"))
}

/// Precondition check: bootrec was built with embedded boot blobs, so the
/// `*_BOOT` constants and `mbr_*` builders have real bytes to produce. The
/// `BootRecordImpl::Bootrec` branches in the pipelines bail out via this
/// before any destructive write.
pub fn ensure_embedded_blobs() -> Result<()> {
    if !bootrec::blobs::embedded() {
        Err(anyhow!(
            "bootrec was built without embedded boot blobs; rebuild \
             with --features embed-boot-asm or pass --boot-record=ms-sys"
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! Golden tests. The four functions above (`build_mbr_win7`,
    //! `build_mbr_xp`, `splice_pbr_bootmgr`, `splice_pbr_ntldr`) are usbwin's
    //! entire output surface for boot-record bytes — any bootrec bump that
    //! changes them is caught here, before the next hardware test.
    //!
    //! Goldens live at `tests/golden/` and are committed. Bootstrap them
    //! (or update after an intentional bootrec change) with:
    //!
    //!     UPDATE_GOLDENS=1 cargo test -p usbwin --lib boot_records
    //!
    //! Tests fail with a clear message if the golden file is missing.
    //!
    //! The synthetic FAT32 reserved area mirrors what `newfs_msdos -F 32`
    //! writes on a 64 GB USB at LBA 2048, captured from a real run on
    //! 2026-05-19. Splice tests use it as input; the resulting bytes are
    //! what bootrec produces "for real" on the user's hardware.
    use super::*;
    use std::path::PathBuf;

    /// 64 GB SanDisk Extreme Media, the reference test stick.
    /// 125_045_424 sectors × 512 bytes/sec.
    const DISK_SIZE_64GB: u64 = 64_023_257_088;

    fn goldens_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
    }

    fn compare_or_update(golden_name: &str, actual: &[u8]) {
        let path = goldens_dir().join(golden_name);
        if std::env::var("UPDATE_GOLDENS").is_ok() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, actual).unwrap_or_else(|e| {
                panic!("writing golden {}: {e}", path.display())
            });
            return;
        }
        let expected = std::fs::read(&path).unwrap_or_else(|e| {
            panic!(
                "reading golden {}: {e}\n\
                 First run? Capture goldens with: \
                 UPDATE_GOLDENS=1 cargo test -p usbwin --lib boot_records",
                path.display()
            )
        });
        if expected != actual {
            // Surface the first divergence; full-buffer diff is unreadable.
            let first_bad = expected
                .iter()
                .zip(actual.iter())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| expected.len().min(actual.len()));
            panic!(
                "golden {} mismatch at byte {:#x} \
                 (expected.len={}, actual.len={})\n\
                 expected[{:#x}..]={:02x?}\n\
                 actual[{:#x}..]  ={:02x?}\n\
                 If this is an intentional bootrec change: \
                 UPDATE_GOLDENS=1 cargo test ...",
                path.display(),
                first_bad,
                expected.len(),
                actual.len(),
                first_bad,
                &expected[first_bad..(first_bad + 16).min(expected.len())],
                first_bad,
                &actual[first_bad..(first_bad + 16).min(actual.len())],
            );
        }
    }

    /// Hand-crafted FAT32 reserved area (1024 bytes = sector 0 + sector 1).
    /// Values mirror `newfs_msdos -F 32 -v WIN7` output on a 64 GB stick.
    /// OEM is `BSD  4.4` because the splice preserves what's on disk; if
    /// bootrec ever overwrites OEM the splice output will diverge and the
    /// golden will trip. VolID and free-count are deterministic so tests
    /// are reproducible.
    fn synthetic_fat32_reserved() -> [u8; 1024] {
        let mut out = [0u8; 1024];

        // ── Sector 0 (offset 0..512): boot sector with BPB ──
        out[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]); // FAT32 jmp + nop
        out[3..11].copy_from_slice(b"BSD  4.4");        // OEM
        out[11..13].copy_from_slice(&512u16.to_le_bytes()); // BytsPerSec
        out[13] = 64;                                    // SecPerClus (32 KiB)
        out[14..16].copy_from_slice(&32u16.to_le_bytes()); // RsvdSecCnt
        out[16] = 2;                                     // NumFATs
        // bytes 17..21 RootEntCnt+TotSec16 left 0 for FAT32
        out[21] = 0xF8;                                  // Media (fixed disk)
        // bytes 22..24 FATSz16 left 0 for FAT32
        out[24..26].copy_from_slice(&32u16.to_le_bytes()); // SecPerTrk
        out[26..28].copy_from_slice(&255u16.to_le_bytes()); // NumHeads
        out[28..32].copy_from_slice(&2048u32.to_le_bytes()); // HiddSec
        out[32..36].copy_from_slice(&125_043_888u32.to_le_bytes()); // TotSec32
        out[36..40].copy_from_slice(&15_261u32.to_le_bytes()); // FATSz32
        // bytes 40..44 ExtFlags+FSVer left 0
        out[44..48].copy_from_slice(&2u32.to_le_bytes()); // RootClus
        out[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo sector
        out[50..52].copy_from_slice(&6u16.to_le_bytes()); // BkBootSec
        // bytes 52..64 reserved (12 zeros)
        out[64] = 0x80;                                  // DrvNum
        // byte 65 reserved
        out[66] = 0x29;                                  // BootSig (extended)
        out[67..71].copy_from_slice(&0x12345678u32.to_le_bytes()); // VolID
        out[71..82].copy_from_slice(b"WIN7       ");     // Volume label (11 bytes)
        out[82..90].copy_from_slice(b"FAT32   ");        // FS type (8 bytes)
        // bytes 90..510 left 0 (boot code area, overwritten by splice)
        out[510..512].copy_from_slice(&[0x55, 0xAA]);    // Boot signature

        // ── Sector 1 (offset 512..1024): FSInfo ──
        out[512..516].copy_from_slice(&[0x52, 0x52, 0x61, 0x41]); // "RRaA" lead sig
        // bytes 516..996 reserved
        out[996..1000].copy_from_slice(&[0x72, 0x72, 0x41, 0x61]); // "rrAa" struc sig
        out[1000..1004].copy_from_slice(&0x001D_6FA1u32.to_le_bytes()); // Free count
        out[1004..1008].copy_from_slice(&0x0000_5E98u32.to_le_bytes()); // Nxt free
        // bytes 1008..1022 reserved
        out[1022..1024].copy_from_slice(&[0x55, 0xAA]); // Trail sig

        out
    }

    #[test]
    fn mbr_win7_matches_golden() {
        let mbr = build_mbr_win7(DISK_SIZE_64GB).unwrap();
        assert_eq!(mbr.len(), 512, "MBR is exactly one sector");
        compare_or_update("mbr_win7_64gb.bin", &mbr);
    }

    #[test]
    fn mbr_xp_matches_golden() {
        let mbr = build_mbr_xp(DISK_SIZE_64GB).unwrap();
        assert_eq!(mbr.len(), 512, "MBR is exactly one sector");
        compare_or_update("mbr_xp_64gb.bin", &mbr);
    }

    #[test]
    fn pbr_bootmgr_multi_matches_golden() {
        let input = synthetic_fat32_reserved();
        let spliced = splice_pbr_bootmgr(&input).unwrap();
        // Multi-sector PBR spans sectors 0, 1, 2 → 1536 bytes.
        assert_eq!(spliced.len(), 1536, "BOOTMGR PBR is 3 sectors");
        // BPB *parameters* (bytes 11..90) preserved verbatim. OEM at 3..11
        // is intentionally overwritten to "MSWIN4.1" — bootrec splices it
        // defensively (see bootrec BACKLOG and USBWIN_NTLDR_FINDINGS).
        assert_eq!(&spliced[11..90], &input[11..90], "BPB params preserved");
        assert_eq!(&spliced[3..11], b"MSWIN4.1", "OEM overwritten to MSWIN4.1");
        // FSInfo at sector 1 is preserved verbatim.
        assert_eq!(&spliced[512..1024], &input[512..1024], "FSInfo preserved");
        compare_or_update("pbr_bootmgr_multi.bin", &spliced);
    }

    #[test]
    fn pbr_ntldr_matches_golden() {
        let input = synthetic_fat32_reserved();
        let sector0: [u8; 512] = input[..512].try_into().unwrap();
        let spliced = splice_pbr_ntldr(&sector0).unwrap();
        // Single-sector PBR.
        assert_eq!(spliced.len(), 512, "NTLDR PBR is 1 sector");
        // Same BPB-vs-OEM split as above.
        assert_eq!(&spliced[11..90], &input[11..90], "BPB params preserved");
        assert_eq!(&spliced[3..11], b"MSWIN4.1", "OEM overwritten to MSWIN4.1");
        // Boot signature intact.
        assert_eq!(&spliced[510..512], &[0x55, 0xAA], "boot signature");
        compare_or_update("pbr_ntldr.bin", &spliced);
    }

    #[test]
    fn synthetic_fixture_is_valid_fat32_bpb() {
        // Self-test on the fixture itself. If this fails the splice tests
        // will too with confusing errors; surface the root cause first.
        let area = synthetic_fat32_reserved();
        assert_eq!(&area[0..3], &[0xEB, 0x58, 0x90], "FAT32 jmp");
        assert_eq!(u16::from_le_bytes([area[11], area[12]]), 512, "BytsPerSec");
        assert_eq!(area[16], 2, "NumFATs");
        assert_eq!(area[21], 0xF8, "Media");
        assert_eq!(&area[510..512], &[0x55, 0xAA], "sector 0 boot sig");
        assert_eq!(&area[512..516], b"RRaA", "FSInfo lead sig");
        assert_eq!(&area[996..1000], b"rrAa", "FSInfo struc sig");
        assert_eq!(&area[1022..1024], &[0x55, 0xAA], "FSInfo trail sig");
    }
}

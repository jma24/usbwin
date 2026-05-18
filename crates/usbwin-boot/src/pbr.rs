//! The FAT32 PBR splice. The "preserve the BPB" rule from docs/BOOT_RECORDS.md
//! lives here as code.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PbrError {
    #[error("existing PBR is {got} bytes; expected exactly 512")]
    BadExistingSize { got: usize },

    #[error("boot blob is {got} bytes; expected exactly 512")]
    BadBlobSize { got: usize },

    #[error("boot blobs were not embedded; rebuild with --features embed-boot-asm")]
    NotEmbedded,
}

/// Splice the FAT32 PBR. Given:
///   - `existing`: the 512-byte sector currently at /dev/rdiskNs1 offset 0,
///     i.e. what newfs_msdos just wrote (BPB at 3..89 is what we keep).
///   - `boot`: the 512-byte blob from `boot-asm/build/fat32_pbr.bin`
///
/// Returns a new 512-byte sector ready to be written back to the partition:
///   bytes   0..2   = boot[0..2]       (jump)
///   bytes   3..89  = existing[3..89]  (BPB - preserved)
///   bytes  90..509 = boot[90..509]    (boot code)
///   bytes 510..511 = [0x55, 0xAA]     (signature)
pub fn splice_fat32_pbr(existing: &[u8], boot: &[u8]) -> Result<[u8; 512], PbrError> {
    if existing.len() != 512 {
        return Err(PbrError::BadExistingSize { got: existing.len() });
    }
    if boot.is_empty() {
        return Err(PbrError::NotEmbedded);
    }
    if boot.len() != 512 {
        return Err(PbrError::BadBlobSize { got: boot.len() });
    }

    let mut out = [0u8; 512];
    out[0..3].copy_from_slice(&boot[0..3]);
    out[3..90].copy_from_slice(&existing[3..90]);
    out[90..510].copy_from_slice(&boot[90..510]);
    out[510] = 0x55;
    out[511] = 0xAA;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_blob() -> Vec<u8> {
        let mut b = vec![0u8; 512];
        // Distinctive markers so we can assert what came from where.
        b[0] = 0xEB;
        b[1] = 0x58;
        b[2] = 0x90;
        for i in 90..510 {
            b[i] = 0xCC; // "code" filler
        }
        b
    }

    fn fake_existing() -> Vec<u8> {
        let mut e = vec![0u8; 512];
        // BPB filler so we can detect preservation.
        for i in 3..90 {
            e[i] = 0xBB;
        }
        e
    }

    #[test]
    fn splice_preserves_bpb() {
        let out = splice_fat32_pbr(&fake_existing(), &fake_blob()).unwrap();
        assert_eq!(&out[0..3], &[0xEB, 0x58, 0x90], "jump from blob");
        assert!(out[3..90].iter().all(|&b| b == 0xBB), "BPB from existing");
        assert!(out[90..510].iter().all(|&b| b == 0xCC), "boot code from blob");
        assert_eq!(&out[510..512], &[0x55, 0xAA], "boot signature");
    }

    #[test]
    fn splice_rejects_wrong_sizes() {
        assert!(splice_fat32_pbr(&vec![0u8; 256], &fake_blob()).is_err());
        assert!(splice_fat32_pbr(&fake_existing(), &vec![0u8; 256]).is_err());
    }

    #[test]
    fn splice_errors_when_blob_missing() {
        match splice_fat32_pbr(&fake_existing(), &[]) {
            Err(PbrError::NotEmbedded) => {}
            other => panic!("expected NotEmbedded, got {other:?}"),
        }
    }
}

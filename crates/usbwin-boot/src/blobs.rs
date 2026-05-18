//! The embedded boot-record blobs. Assembled at build time from `boot-asm/`
//! and `include_bytes!`'d here. Without the `embed-boot-asm` feature these
//! are empty slices — any code path that needs them will surface a clear
//! error at runtime ("usbwin was built without boot blobs; rebuild with
//! --features embed-boot-asm").

pub const MBR_BOOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mbr.bin"));
pub const FAT32_PBR_BOOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fat32_pbr.bin"));
pub const NTFS_PBR_BOOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ntfs_pbr.bin"));

/// Returns `true` if the boot blobs were embedded at build time.
pub fn embedded() -> bool {
    !MBR_BOOT.is_empty()
}

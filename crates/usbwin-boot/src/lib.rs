//! Boot record assembly. Pure byte manipulation; no I/O.
//!
//! The single most important function in this crate is `splice_fat32_pbr`:
//! it takes the existing PBR (which `newfs_msdos` just populated with the
//! correct BPB for this specific partition) and splices in our boot code
//! while preserving bytes 3..89 (the BPB). See docs/BOOT_RECORDS.md.

pub mod blobs;
pub mod pbr;

pub use blobs::{FAT32_PBR_BOOT, MBR_BOOT, NTFS_PBR_BOOT};
pub use pbr::splice_fat32_pbr;

// Wired in once bootrec ships build_xp_setup_chain_bootsect (see
// bootrec/docs/XP_SETUP_CHAIN_BOOTSECT_SPEC.md). Until then nothing in
// the release binary calls these items; tests exercise them in full.
#![allow(dead_code)]

//! Minimal FAT32 reader for finding a file's on-disk extent (LBAs +
//! cluster chain) without mounting the filesystem.
//!
//! The XP-USB-install boot chain needs a single-sector loader that reads
//! `\$LDR$` (= `\I386\SETUPLDR.BIN` renamed) from raw LBAs — there isn't
//! room in 512 bytes for both a FAT walker and the CHS read logic. So
//! usbwin walks FAT in advance (here) and hands the LBA list to bootrec,
//! which emits a sector that just reads those LBAs into memory and jumps.
//!
//! Scope: read-only, FAT32 only, root-directory 8.3 names only. No LFN
//! reassembly, no subdirectories, no FAT12/16. Caller passes a `Device`
//! pointing at the partition (LBA 0 = the FAT32 boot sector / BPB), not
//! the whole disk.

use anyhow::{anyhow, bail, Result};
use usbwin_core::Device;

/// Parsed FAT32 BIOS Parameter Block. Field names mirror the FAT32 spec
/// (FATGEN103.doc) so spec-vs-code cross-referencing is mechanical.
#[derive(Debug, Clone)]
#[allow(dead_code)] // hidden_sector_count is read by callers building BOOTSECT.DAT
pub struct Bpb {
    pub bytes_per_sector: u16,    // BPB_BytsPerSec @ 0x0B (always 512 here)
    pub sectors_per_cluster: u8,  // BPB_SecPerClus @ 0x0D
    pub reserved_sector_count: u16, // BPB_RsvdSecCnt @ 0x0E
    pub num_fats: u8,             // BPB_NumFATs @ 0x10
    pub hidden_sector_count: u32, // BPB_HiddSec @ 0x1C (= partition start LBA)
    pub fat_size_32: u32,         // BPB_FATSz32 @ 0x24
    pub root_cluster: u32,        // BPB_RootClus @ 0x2C
}

impl Bpb {
    /// Parse from the first 90 bytes of the FAT32 boot sector.
    /// Returns the BPB or a descriptive error if the sector doesn't look
    /// like FAT32.
    pub fn parse(sector0: &[u8]) -> Result<Self> {
        if sector0.len() < 90 {
            bail!(
                "BPB parse: input too short ({} bytes, need ≥90)",
                sector0.len()
            );
        }
        // Verify boot signature.
        if sector0[510..512] != [0x55, 0xAA] {
            bail!("BPB parse: missing 0xAA55 boot signature at offset 0x1FE");
        }
        let bytes_per_sector = u16::from_le_bytes([sector0[11], sector0[12]]);
        if bytes_per_sector != 512 {
            bail!(
                "BPB parse: unsupported sector size {bytes_per_sector} \
                 (only 512 is supported)"
            );
        }
        let sectors_per_cluster = sector0[13];
        if sectors_per_cluster == 0 || !sectors_per_cluster.is_power_of_two() {
            bail!(
                "BPB parse: invalid SecPerClus {sectors_per_cluster} \
                 (must be a power of two, 1..=128)"
            );
        }
        let reserved_sector_count = u16::from_le_bytes([sector0[14], sector0[15]]);
        let num_fats = sector0[16];
        if num_fats == 0 {
            bail!("BPB parse: NumFATs is 0");
        }
        let hidden_sector_count =
            u32::from_le_bytes([sector0[28], sector0[29], sector0[30], sector0[31]]);
        let fat_size_32 =
            u32::from_le_bytes([sector0[36], sector0[37], sector0[38], sector0[39]]);
        if fat_size_32 == 0 {
            bail!("BPB parse: FATSz32 is 0 — is this FAT12/16, not FAT32?");
        }
        let root_cluster =
            u32::from_le_bytes([sector0[44], sector0[45], sector0[46], sector0[47]]);
        if root_cluster < 2 {
            bail!("BPB parse: invalid RootClus {root_cluster} (must be ≥2)");
        }
        Ok(Self {
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sector_count,
            num_fats,
            hidden_sector_count,
            fat_size_32,
            root_cluster,
        })
    }

    /// LBA (partition-relative) of the first FAT.
    pub fn fat_start_lba(&self) -> u32 {
        self.reserved_sector_count as u32
    }

    /// LBA (partition-relative) of the start of the data area
    /// (i.e. cluster 2).
    pub fn data_start_lba(&self) -> u32 {
        self.fat_start_lba() + (self.num_fats as u32) * self.fat_size_32
    }

    /// Convert a cluster number to its starting LBA (partition-relative).
    pub fn cluster_to_lba(&self, cluster: u32) -> u32 {
        self.data_start_lba() + (cluster - 2) * (self.sectors_per_cluster as u32)
    }
}

/// On-disk extent of a file: where to find it, how big it is.
#[derive(Debug, Clone)]
pub struct FileExtent {
    /// Cluster numbers in FAT order (first cluster, then next, ...).
    pub clusters: Vec<u32>,
    /// Partition-relative LBAs covered by `clusters`, in order. Length =
    /// `clusters.len() * bpb.sectors_per_cluster`.
    pub lbas: Vec<u32>,
    /// File size in bytes from the directory entry.
    pub file_size_bytes: u32,
}

/// A contiguous run of partition-relative LBAs. Matches the shape
/// bootrec's `build_xp_setup_chain_bootsect` expects (see
/// bootrec/docs/XP_SETUP_CHAIN_BOOTSECT_SPEC.md). On the wire: 6 bytes
/// per run (4 + 2), so even a moderately-fragmented `$LDR$` fits in the
/// remaining sector-0 space after boot code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LbaRun {
    pub start_lba: u32,
    pub sector_count: u16,
}

/// Coalesce a flat ascending LBA list into the smallest set of `LbaRun`s
/// where consecutive LBAs collapse into one run. Splits a single
/// fragment if its length would overflow `u16` (≥65,536 sectors ≈ 32 MiB
/// — won't happen for `$LDR$` but defensively defined).
pub fn coalesce_lbas_to_runs(lbas: &[u32]) -> Vec<LbaRun> {
    if lbas.is_empty() {
        return Vec::new();
    }
    let mut runs = Vec::new();
    let mut start = lbas[0];
    let mut count: u16 = 1;
    for &lba in &lbas[1..] {
        if lba == start.saturating_add(count as u32) && count < u16::MAX {
            count += 1;
        } else {
            runs.push(LbaRun { start_lba: start, sector_count: count });
            start = lba;
            count = 1;
        }
    }
    runs.push(LbaRun { start_lba: start, sector_count: count });
    runs
}

/// Find a file in the FAT32 root directory by its 11-byte 8.3 name (e.g.
/// `b"$LDR$      "` for `$LDR$`). Walks the root cluster chain, scans
/// directory entries, then walks the FAT chain to enumerate every cluster
/// the file occupies. Returns extent or `None` if the file isn't there.
///
/// `device` must be opened against the *partition*, not the whole disk —
/// LBA 0 of `device` is the FAT32 boot sector. Read-only is fine.
pub fn find_file_extent(
    device: &mut dyn Device,
    bpb: &Bpb,
    name_8_3: &[u8; 11],
) -> Result<Option<FileExtent>> {
    let cluster_bytes = (bpb.sectors_per_cluster as usize) * (bpb.bytes_per_sector as usize);
    let sec_size = bpb.bytes_per_sector as u64;

    // Walk the root directory cluster chain looking for the file's
    // directory entry.
    let mut dir_cluster = bpb.root_cluster;
    let mut cluster_buf = vec![0u8; cluster_bytes];
    let dir_entry = loop {
        let lba = bpb.cluster_to_lba(dir_cluster);
        device
            .read_at((lba as u64) * sec_size, &mut cluster_buf)
            .map_err(|e| anyhow!("read root dir cluster {dir_cluster}: {e}"))?;

        if let Some(ent) = scan_dir_cluster(&cluster_buf, name_8_3) {
            break Some(ent);
        }

        // Hit a terminator entry (first byte 0x00)? End of directory.
        if cluster_buf
            .chunks(32)
            .any(|e| e.first().copied() == Some(0x00))
        {
            break None;
        }

        // Follow FAT to next cluster.
        match next_cluster(device, bpb, dir_cluster)? {
            Some(c) => dir_cluster = c,
            None => break None,
        }
    };

    let entry = match dir_entry {
        Some(e) => e,
        None => return Ok(None),
    };

    // Walk the file's cluster chain.
    let mut clusters = vec![entry.first_cluster];
    let mut current = entry.first_cluster;
    loop {
        match next_cluster(device, bpb, current)? {
            Some(c) => {
                clusters.push(c);
                current = c;
                // Sanity: don't run forever on a broken FAT.
                if clusters.len() > 1_000_000 {
                    bail!("FAT chain too long (>1M clusters) — likely corrupt");
                }
            }
            None => break,
        }
    }

    // Expand clusters → LBAs.
    let mut lbas = Vec::with_capacity(clusters.len() * (bpb.sectors_per_cluster as usize));
    for &c in &clusters {
        let start = bpb.cluster_to_lba(c);
        for i in 0..(bpb.sectors_per_cluster as u32) {
            lbas.push(start + i);
        }
    }

    Ok(Some(FileExtent {
        clusters,
        lbas,
        file_size_bytes: entry.file_size,
    }))
}

#[derive(Debug)]
struct MatchedEntry {
    first_cluster: u32,
    file_size: u32,
}

/// Scan one cluster of root-directory bytes for an 8.3 name. Skips LFN
/// entries (attr=0x0F) and deleted entries (first byte 0xE5).
fn scan_dir_cluster(cluster_buf: &[u8], name_8_3: &[u8; 11]) -> Option<MatchedEntry> {
    for entry in cluster_buf.chunks_exact(32) {
        let first = entry[0];
        if first == 0x00 {
            return None; // end-of-directory marker
        }
        if first == 0xE5 {
            continue; // deleted
        }
        let attr = entry[11];
        if attr == 0x0F {
            continue; // LFN entry
        }
        if &entry[..11] == name_8_3 {
            // FAT32 splits the first cluster across two fields.
            let hi = u16::from_le_bytes([entry[20], entry[21]]) as u32;
            let lo = u16::from_le_bytes([entry[26], entry[27]]) as u32;
            let first_cluster = (hi << 16) | lo;
            let file_size =
                u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]);
            return Some(MatchedEntry {
                first_cluster,
                file_size,
            });
        }
    }
    None
}

/// Read FAT entry for `cluster` and return the next cluster, or `None` if
/// this is the end of a chain. Mask the top 4 bits per FAT32 spec.
fn next_cluster(device: &mut dyn Device, bpb: &Bpb, cluster: u32) -> Result<Option<u32>> {
    let sec_size = bpb.bytes_per_sector as u32;
    let fat_offset = cluster * 4;
    let fat_sector_lba = bpb.fat_start_lba() + fat_offset / sec_size;
    let offset_in_sector = (fat_offset % sec_size) as usize;

    let mut buf = vec![0u8; sec_size as usize];
    device
        .read_at((fat_sector_lba as u64) * (sec_size as u64), &mut buf)
        .map_err(|e| anyhow!("read FAT sector LBA {fat_sector_lba}: {e}"))?;

    let raw = u32::from_le_bytes([
        buf[offset_in_sector],
        buf[offset_in_sector + 1],
        buf[offset_in_sector + 2],
        buf[offset_in_sector + 3],
    ]);
    let entry = raw & 0x0FFF_FFFF; // top 4 bits are reserved

    // End-of-chain markers: 0x0FFFFFF8..=0x0FFFFFFF (Microsoft uses
    // 0x0FFFFFF8 historically; treat anything ≥ 0x0FFFFFF8 as EOC).
    if entry >= 0x0FFF_FFF8 {
        Ok(None)
    } else if entry < 2 {
        // 0 = free, 1 = reserved — shouldn't appear in an allocated chain.
        bail!("FAT chain: cluster {cluster} → invalid next-cluster {entry}");
    } else {
        Ok(Some(entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use usbwin_core::device::MemoryDevice;

    /// Build a tiny synthetic FAT32 partition: 1 reserved sector (BPB),
    /// 1 FAT of 4 sectors, 8 sectors-per-cluster data area. Place one
    /// file "$LDR$" at cluster 2 chaining to cluster 3 then EOC.
    fn synthetic_partition() -> Vec<u8> {
        // Geometry choices: BytsPerSec=512, SecPerClus=8 (4KiB),
        // RsvdSecCnt=1, NumFATs=1, FATSz32=4. Means:
        //   FAT start LBA = 1
        //   Data start LBA = 1 + 1*4 = 5
        //   Root cluster = 2 → root dir at LBA 5..13
        //   $LDR$ cluster 2 lives in root area; let's instead put root at
        //   cluster 2, and $LDR$ at clusters 3+4. That avoids "$LDR$ is
        //   the root dir" weirdness.
        const SEC: usize = 512;
        const TOTAL_SECTORS: usize = 64;
        let mut img = vec![0u8; SEC * TOTAL_SECTORS];

        // ── Sector 0: BPB ──
        img[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        img[3..11].copy_from_slice(b"FAT32   ");
        img[11..13].copy_from_slice(&512u16.to_le_bytes()); // BytsPerSec
        img[13] = 8;                                          // SecPerClus
        img[14..16].copy_from_slice(&1u16.to_le_bytes());     // RsvdSecCnt
        img[16] = 1;                                          // NumFATs
        // RootEntCnt + TotSec16 + Media + FATSz16 — left 0 for FAT32
        img[21] = 0xF8;                                       // Media
        img[28..32].copy_from_slice(&0u32.to_le_bytes());     // HiddSec
        img[36..40].copy_from_slice(&4u32.to_le_bytes());     // FATSz32
        img[44..48].copy_from_slice(&2u32.to_le_bytes());     // RootClus
        img[510..512].copy_from_slice(&[0x55, 0xAA]);

        // ── FAT (LBA 1..5) ──
        // Cluster 0 = media + reserved high bits (0x0FFFFFF8)
        // Cluster 1 = 0x0FFFFFFF (reserved)
        // Cluster 2 = end-of-chain (root dir, single cluster)
        // Cluster 3 = 4 (chains to cluster 4)
        // Cluster 4 = end-of-chain
        let fat_base = SEC * 1;
        let put_fat = |img: &mut [u8], cluster: u32, value: u32| {
            let off = fat_base + (cluster as usize) * 4;
            img[off..off + 4].copy_from_slice(&value.to_le_bytes());
        };
        put_fat(&mut img, 0, 0x0FFFFFF8);
        put_fat(&mut img, 1, 0x0FFFFFFF);
        put_fat(&mut img, 2, 0x0FFFFFFF); // root dir single cluster
        put_fat(&mut img, 3, 4);
        put_fat(&mut img, 4, 0x0FFFFFFF);

        // ── Root dir cluster (cluster 2, LBA 5..13) ──
        // Directory entry for "$LDR$      " (11 bytes, padded).
        let root_lba = 5;
        let root_off = SEC * root_lba;
        let mut entry = [0u8; 32];
        entry[..11].copy_from_slice(b"$LDR$      ");
        entry[11] = 0x20; // ATTR_ARCHIVE
        // First cluster = 3 (split across hi @ 20..22, lo @ 26..28)
        entry[20..22].copy_from_slice(&0u16.to_le_bytes());
        entry[26..28].copy_from_slice(&3u16.to_le_bytes());
        // File size = 8192 bytes (= 16 sectors = 2 clusters worth)
        entry[28..32].copy_from_slice(&8192u32.to_le_bytes());
        img[root_off..root_off + 32].copy_from_slice(&entry);
        // Next entry: 0x00 first byte = end-of-dir
        // (already zero from initialization)

        img
    }

    #[test]
    fn parse_bpb_happy() {
        let img = synthetic_partition();
        let bpb = Bpb::parse(&img[..512]).unwrap();
        assert_eq!(bpb.bytes_per_sector, 512);
        assert_eq!(bpb.sectors_per_cluster, 8);
        assert_eq!(bpb.reserved_sector_count, 1);
        assert_eq!(bpb.num_fats, 1);
        assert_eq!(bpb.fat_size_32, 4);
        assert_eq!(bpb.root_cluster, 2);
        assert_eq!(bpb.fat_start_lba(), 1);
        assert_eq!(bpb.data_start_lba(), 5);
        assert_eq!(bpb.cluster_to_lba(2), 5);
        assert_eq!(bpb.cluster_to_lba(3), 13);
    }

    #[test]
    fn parse_bpb_rejects_bad_signature() {
        let mut img = synthetic_partition();
        img[511] = 0x00;
        let err = Bpb::parse(&img[..512]).unwrap_err();
        assert!(err.to_string().contains("boot signature"));
    }

    #[test]
    fn parse_bpb_rejects_non_fat32() {
        let mut img = synthetic_partition();
        img[36..40].copy_from_slice(&0u32.to_le_bytes()); // FATSz32 = 0
        let err = Bpb::parse(&img[..512]).unwrap_err();
        assert!(err.to_string().contains("FAT12/16"));
    }

    #[test]
    fn find_ldr_extent_in_synthetic() {
        let img = synthetic_partition();
        let bpb = Bpb::parse(&img[..512]).unwrap();
        let mut dev = MemoryDevice {
            bytes: img,
            label: "synthetic".into(),
        };

        let ext = find_file_extent(&mut dev, &bpb, b"$LDR$      ")
            .unwrap()
            .expect("$LDR$ should be present");
        assert_eq!(ext.clusters, vec![3, 4]);
        // Clusters 3 and 4 each contribute 8 LBAs. Cluster 3 starts at
        // LBA 5 + (3-2)*8 = 13; cluster 4 at LBA 5 + (4-2)*8 = 21.
        let expected_lbas: Vec<u32> = (13..21).chain(21..29).collect();
        assert_eq!(ext.lbas, expected_lbas);
        assert_eq!(ext.file_size_bytes, 8192);
    }

    #[test]
    fn coalesce_empty() {
        assert_eq!(coalesce_lbas_to_runs(&[]), Vec::<LbaRun>::new());
    }

    #[test]
    fn coalesce_single() {
        assert_eq!(
            coalesce_lbas_to_runs(&[42]),
            vec![LbaRun { start_lba: 42, sector_count: 1 }]
        );
    }

    #[test]
    fn coalesce_one_contiguous_run() {
        assert_eq!(
            coalesce_lbas_to_runs(&[100, 101, 102, 103]),
            vec![LbaRun { start_lba: 100, sector_count: 4 }]
        );
    }

    #[test]
    fn coalesce_fragmented() {
        // Two clusters of 8 sectors each, with a 100-sector gap between.
        let lbas: Vec<u32> = (13..21).chain(121..129).collect();
        assert_eq!(
            coalesce_lbas_to_runs(&lbas),
            vec![
                LbaRun { start_lba: 13, sector_count: 8 },
                LbaRun { start_lba: 121, sector_count: 8 },
            ]
        );
    }

    #[test]
    fn coalesce_synthetic_ldr_extent() {
        // The synthetic_partition test above gives $LDR$ at LBAs
        // 13..21 + 21..29 — that's one contiguous 16-sector run.
        let img = synthetic_partition();
        let bpb = Bpb::parse(&img[..512]).unwrap();
        let mut dev = MemoryDevice {
            bytes: img,
            label: "synthetic".into(),
        };
        let ext = find_file_extent(&mut dev, &bpb, b"$LDR$      ").unwrap().unwrap();
        let runs = coalesce_lbas_to_runs(&ext.lbas);
        assert_eq!(
            runs,
            vec![LbaRun { start_lba: 13, sector_count: 16 }],
            "freshly-allocated file should be a single run"
        );
    }

    #[test]
    fn coalesce_splits_at_u16_max() {
        // 70 000 consecutive LBAs would otherwise overflow u16 sector_count.
        // Must split into two runs.
        let lbas: Vec<u32> = (0..70_000).collect();
        let runs = coalesce_lbas_to_runs(&lbas);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0], LbaRun { start_lba: 0, sector_count: u16::MAX });
        assert_eq!(runs[1].start_lba, u16::MAX as u32);
        assert_eq!(
            runs[0].sector_count as u32 + runs[1].sector_count as u32,
            70_000
        );
    }

    #[test]
    fn find_missing_file_returns_none() {
        let img = synthetic_partition();
        let bpb = Bpb::parse(&img[..512]).unwrap();
        let mut dev = MemoryDevice {
            bytes: img,
            label: "synthetic".into(),
        };
        let ext = find_file_extent(&mut dev, &bpb, b"NOSUCH  TXT").unwrap();
        assert!(ext.is_none());
    }
}

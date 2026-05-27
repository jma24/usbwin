//! FAT12 helper for adding files to the FiraDisk floppy image.
//!
//! Originally written for injecting `A:\WINNT.SIF`; the file-add /
//! file-replace / directory-listing helpers were used by the floppy-side
//! AHCI auto-load attempt before that approach was abandoned for ISO
//! slipstreaming. They stay in the module as primitives that the
//! upcoming slipstream code (and any future floppy-side work) can build
//! on -- keep them around even if currently unused.
#![allow(dead_code)]

use anyhow::{bail, Context, Result};

const WINNT_SIF_DOS_NAME: &[u8; 11] = b"WINNT   SIF";
const ATTR_ARCHIVE: u8 = 0x20;
const FAT12_EOC: u16 = 0x0fff;

#[derive(Debug)]
struct Fat12Layout {
    bytes_per_sector: usize,
    reserved_sectors: usize,
    fat_count: usize,
    root_entries: usize,
    sectors_per_fat: usize,
    root_dir_offset: usize,
    data_offset: usize,
    cluster_size: usize,
    cluster_count: usize,
}

pub fn add_winnt_sif(image: &mut [u8], contents: &[u8]) -> Result<()> {
    add_file(image, WINNT_SIF_DOS_NAME, contents)
}

/// Encode an 8.3 filename ("IASTOR.SYS") as the 11-byte FAT directory form
/// ("IASTOR  SYS"). Case-insensitive; lower-case input is upcased.
pub fn dos_83_name(name: &str) -> Result<[u8; 11]> {
    let upper = name.to_ascii_uppercase();
    let (stem, ext) = match upper.rsplit_once('.') {
        Some((s, e)) => (s, e),
        None => (upper.as_str(), ""),
    };
    if stem.is_empty() || stem.len() > 8 || ext.len() > 3 {
        bail!("filename {name:?} does not fit FAT 8.3");
    }
    if !stem.bytes().all(valid_short_byte) || !ext.bytes().all(valid_short_byte) {
        bail!("filename {name:?} contains invalid FAT 8.3 characters");
    }
    let mut out = [b' '; 11];
    out[..stem.len()].copy_from_slice(stem.as_bytes());
    out[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Ok(out)
}

fn valid_short_byte(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'(' | b')' | b'@' | b'^' | b'`' | b'{' | b'}' | b'~')
}

pub fn add_file(image: &mut [u8], dos_name: &[u8; 11], contents: &[u8]) -> Result<()> {
    if contents.is_empty() {
        bail!("file contents are empty");
    }

    let layout = parse_layout(image)?;
    if find_root_entry(image, &layout, dos_name)?.is_some() {
        bail!(
            "floppy image already contains {}",
            format_dos_name(dos_name)
        );
    }

    let root_slot = find_free_root_slot(image, &layout)
        .ok_or_else(|| anyhow::anyhow!("floppy root directory has no free entries"))?;
    let needed_clusters = contents.len().div_ceil(layout.cluster_size).max(1);
    let clusters = allocate_clusters(image, &layout, needed_clusters)
        .with_context(|| format!("allocate FAT12 clusters for {}", format_dos_name(dos_name)))?;

    write_file_data(image, &layout, &clusters, contents)?;
    write_root_entry(image, &layout, root_slot, dos_name, clusters[0], contents.len())?;
    Ok(())
}

/// Read a root-level file from the FAT12 image. Returns Ok(None) if absent.
pub fn read_file(image: &[u8], dos_name: &[u8; 11]) -> Result<Option<Vec<u8>>> {
    let layout = parse_layout(image)?;
    let Some(slot) = find_root_entry(image, &layout, dos_name)? else {
        return Ok(None);
    };
    let entry_offset = layout.root_dir_offset + slot * 32;
    let first_cluster = u16_at(image, entry_offset + 26)?;
    let file_size = u32_at(image, entry_offset + 28)? as usize;
    if file_size == 0 {
        return Ok(Some(Vec::new()));
    }

    let mut out = Vec::with_capacity(file_size);
    let mut cluster = first_cluster;
    let mut steps = 0usize;
    while (cluster as usize) < layout.cluster_count + 2 {
        let offset = cluster_offset(&layout, cluster)?;
        let take = (file_size - out.len()).min(layout.cluster_size);
        out.extend_from_slice(&image[offset..offset + take]);
        if out.len() == file_size {
            break;
        }
        let next = read_fat12(image, &layout, cluster)?;
        if next == 0 || next >= FAT12_EOC - 8 {
            break;
        }
        cluster = next;
        steps += 1;
        if steps > layout.cluster_count {
            bail!("FAT12 cluster chain loop detected reading file");
        }
    }
    Ok(Some(out))
}

/// Remove a file from the FAT12 root directory and free its cluster chain.
/// Returns Ok(false) if the file was not present.
pub fn remove_file(image: &mut [u8], dos_name: &[u8; 11]) -> Result<bool> {
    let layout = parse_layout(image)?;
    let Some(slot) = find_root_entry(image, &layout, dos_name)? else {
        return Ok(false);
    };
    let entry_offset = layout.root_dir_offset + slot * 32;
    let first_cluster = u16_at(image, entry_offset + 26)?;
    free_cluster_chain(image, &layout, first_cluster)?;
    image[entry_offset] = 0xe5;
    Ok(true)
}

/// Convenience: remove the file if present, then add it back with the new
/// contents. Used to swap `txtsetup.oem` after a merge.
pub fn replace_file(image: &mut [u8], dos_name: &[u8; 11], contents: &[u8]) -> Result<()> {
    let _ = remove_file(image, dos_name)?;
    add_file(image, dos_name, contents)
}

/// List long-form filenames (e.g. `firadisk.sys`, `TXTSETUP.OEM`) of all
/// regular files in the FAT12 root directory, skipping volume labels,
/// LFN slots, and the embedded DOS-name `TXTSETUPOEM` (which has no dot
/// and would be returned as-is). Uppercase 8.3 entries are returned with
/// extension separator restored.
///
/// The pipeline uses this to filter the auto-load `[MassStorageDrivers]`
/// and `[OEMBootFiles]` sections to only include controllers/files whose
/// binaries actually landed on the floppy -- the embedded FiraDisk image
/// declares an x64 controller in its TXTSETUP.OEM but ships no x64 .sys,
/// and listing the missing file in `[OEMBootFiles]` causes XP text-mode
/// setup to bail with `oemdisk.c` error 18.
pub fn list_root_files(image: &[u8]) -> Result<Vec<String>> {
    let layout = parse_layout(image)?;
    let mut out = Vec::new();
    for slot in 0..layout.root_entries {
        let offset = layout.root_dir_offset + slot * 32;
        let first = image[offset];
        if first == 0x00 {
            break;
        }
        if first == 0xe5 {
            continue;
        }
        let attr = image[offset + 11];
        // Skip LFN slots (0x0f) and volume labels (0x08 bit).
        if attr == 0x0f || (attr & 0x08) != 0 {
            continue;
        }
        let raw = &image[offset..offset + 11];
        let stem = std::str::from_utf8(&raw[0..8])
            .unwrap_or("")
            .trim_end_matches(' ');
        let ext = std::str::from_utf8(&raw[8..11])
            .unwrap_or("")
            .trim_end_matches(' ');
        if stem.is_empty() {
            continue;
        }
        let name = if ext.is_empty() {
            stem.to_string()
        } else {
            format!("{stem}.{ext}")
        };
        out.push(name);
    }
    Ok(out)
}

fn free_cluster_chain(image: &mut [u8], layout: &Fat12Layout, first: u16) -> Result<()> {
    if first < 2 {
        return Ok(());
    }
    let mut cluster = first;
    let mut steps = 0usize;
    while (cluster as usize) < layout.cluster_count + 2 {
        let next = read_fat12(image, layout, cluster)?;
        write_fat12_all(image, layout, cluster, 0)?;
        if next == 0 || next >= FAT12_EOC - 8 {
            break;
        }
        cluster = next;
        steps += 1;
        if steps > layout.cluster_count {
            bail!("FAT12 cluster chain loop detected starting at cluster {first}");
        }
    }
    Ok(())
}

fn format_dos_name(name: &[u8; 11]) -> String {
    let stem = std::str::from_utf8(&name[0..8])
        .unwrap_or("????????")
        .trim_end();
    let ext = std::str::from_utf8(&name[8..11])
        .unwrap_or("???")
        .trim_end();
    if ext.is_empty() {
        stem.to_string()
    } else {
        format!("{stem}.{ext}")
    }
}

fn parse_layout(image: &[u8]) -> Result<Fat12Layout> {
    if image.len() < 512 {
        bail!("floppy image is too small for a FAT boot sector");
    }
    if image[510] != 0x55 || image[511] != 0xaa {
        bail!("floppy image has no FAT boot-sector signature");
    }

    let bytes_per_sector = u16_at(image, 11)? as usize;
    let sectors_per_cluster = image[13] as usize;
    let reserved_sectors = u16_at(image, 14)? as usize;
    let fat_count = image[16] as usize;
    let root_entries = u16_at(image, 17)? as usize;
    let total_sectors_16 = u16_at(image, 19)? as usize;
    let total_sectors_32 = u32_at(image, 32)? as usize;
    let total_sectors = if total_sectors_16 != 0 {
        total_sectors_16
    } else {
        total_sectors_32
    };
    let sectors_per_fat = u16_at(image, 22)? as usize;

    if bytes_per_sector == 0
        || sectors_per_cluster == 0
        || reserved_sectors == 0
        || fat_count == 0
        || root_entries == 0
        || total_sectors == 0
        || sectors_per_fat == 0
    {
        bail!("floppy image has invalid FAT12 BPB fields");
    }
    if image.len() < total_sectors * bytes_per_sector {
        bail!("floppy image is shorter than its FAT BPB total-sector count");
    }

    let root_dir_offset = (reserved_sectors + fat_count * sectors_per_fat) * bytes_per_sector;
    let root_dir_len = root_entries * 32;
    let data_offset = root_dir_offset + root_dir_len;
    let cluster_size = bytes_per_sector * sectors_per_cluster;
    if data_offset > image.len() {
        bail!("floppy image FAT/root layout exceeds image size");
    }
    let data_sectors = total_sectors
        .checked_sub(
            reserved_sectors
                + fat_count * sectors_per_fat
                + root_dir_len.div_ceil(bytes_per_sector),
        )
        .ok_or_else(|| anyhow::anyhow!("floppy image FAT layout underflows"))?;
    let cluster_count = data_sectors / sectors_per_cluster;
    if cluster_count >= 4085 {
        bail!("floppy image is not FAT12-sized");
    }

    Ok(Fat12Layout {
        bytes_per_sector,
        reserved_sectors,
        fat_count,
        root_entries,
        sectors_per_fat,
        root_dir_offset,
        data_offset,
        cluster_size,
        cluster_count,
    })
}

fn allocate_clusters(image: &mut [u8], layout: &Fat12Layout, needed: usize) -> Result<Vec<u16>> {
    let mut clusters = Vec::with_capacity(needed);
    for cluster in 2..(layout.cluster_count as u16 + 2) {
        if read_fat12(image, layout, cluster)? == 0 {
            clusters.push(cluster);
            if clusters.len() == needed {
                break;
            }
        }
    }
    if clusters.len() != needed {
        bail!("FiraDisk floppy image does not have enough free clusters for WINNT.SIF");
    }

    for (idx, &cluster) in clusters.iter().enumerate() {
        let value = clusters.get(idx + 1).copied().unwrap_or(FAT12_EOC);
        write_fat12_all(image, layout, cluster, value)?;
    }
    Ok(clusters)
}

fn write_file_data(
    image: &mut [u8],
    layout: &Fat12Layout,
    clusters: &[u16],
    contents: &[u8],
) -> Result<()> {
    let mut written = 0usize;
    for &cluster in clusters {
        let offset = cluster_offset(layout, cluster)?;
        let end = offset + layout.cluster_size;
        image[offset..end].fill(0);
        let remaining = contents.len() - written;
        let n = remaining.min(layout.cluster_size);
        image[offset..offset + n].copy_from_slice(&contents[written..written + n]);
        written += n;
    }
    Ok(())
}

fn write_root_entry(
    image: &mut [u8],
    layout: &Fat12Layout,
    slot: usize,
    dos_name: &[u8; 11],
    first_cluster: u16,
    file_size: usize,
) -> Result<()> {
    let offset = layout.root_dir_offset + slot * 32;
    let entry = &mut image[offset..offset + 32];
    entry.fill(0);
    entry[0..11].copy_from_slice(dos_name);
    entry[11] = ATTR_ARCHIVE;
    // Stable FAT timestamp keeps image diffs deterministic.
    let time = fat_time(20, 0, 0);
    let date = fat_date(2026, 5, 21);
    entry[22..24].copy_from_slice(&time.to_le_bytes());
    entry[24..26].copy_from_slice(&date.to_le_bytes());
    entry[26..28].copy_from_slice(&first_cluster.to_le_bytes());
    entry[28..32].copy_from_slice(
        &u32::try_from(file_size)
            .context("file is too large for FAT12 directory entry")?
            .to_le_bytes(),
    );
    Ok(())
}

fn find_root_entry(image: &[u8], layout: &Fat12Layout, name: &[u8; 11]) -> Result<Option<usize>> {
    for slot in 0..layout.root_entries {
        let offset = layout.root_dir_offset + slot * 32;
        let first = image[offset];
        if first == 0x00 {
            return Ok(None);
        }
        if first == 0xe5 {
            continue;
        }
        if &image[offset..offset + 11] == name {
            return Ok(Some(slot));
        }
    }
    Ok(None)
}

fn find_free_root_slot(image: &[u8], layout: &Fat12Layout) -> Option<usize> {
    for slot in 0..layout.root_entries {
        let offset = layout.root_dir_offset + slot * 32;
        let first = image[offset];
        if first == 0x00 || first == 0xe5 {
            return Some(slot);
        }
    }
    None
}

fn read_fat12(image: &[u8], layout: &Fat12Layout, cluster: u16) -> Result<u16> {
    let fat_offset = layout.reserved_sectors * layout.bytes_per_sector;
    let offset = fat_offset + (cluster as usize * 3) / 2;
    if offset + 1 >= image.len() {
        bail!("FAT12 entry for cluster {cluster} is outside the image");
    }
    let pair = u16::from_le_bytes([image[offset], image[offset + 1]]);
    if cluster & 1 == 0 {
        Ok(pair & 0x0fff)
    } else {
        Ok(pair >> 4)
    }
}

fn write_fat12_all(image: &mut [u8], layout: &Fat12Layout, cluster: u16, value: u16) -> Result<()> {
    for fat_idx in 0..layout.fat_count {
        let fat_offset =
            (layout.reserved_sectors + fat_idx * layout.sectors_per_fat) * layout.bytes_per_sector;
        write_fat12_one(image, fat_offset, cluster, value)?;
    }
    Ok(())
}

fn write_fat12_one(image: &mut [u8], fat_offset: usize, cluster: u16, value: u16) -> Result<()> {
    let offset = fat_offset + (cluster as usize * 3) / 2;
    if offset + 1 >= image.len() {
        bail!("FAT12 entry for cluster {cluster} is outside the image");
    }
    let value = value & 0x0fff;
    if cluster & 1 == 0 {
        image[offset] = (value & 0x00ff) as u8;
        image[offset + 1] = (image[offset + 1] & 0xf0) | ((value >> 8) as u8 & 0x0f);
    } else {
        image[offset] = (image[offset] & 0x0f) | ((value << 4) as u8 & 0xf0);
        image[offset + 1] = (value >> 4) as u8;
    }
    Ok(())
}

fn cluster_offset(layout: &Fat12Layout, cluster: u16) -> Result<usize> {
    if cluster < 2 || cluster as usize >= layout.cluster_count + 2 {
        bail!("FAT12 cluster {cluster} is outside the data area");
    }
    Ok(layout.data_offset + (cluster as usize - 2) * layout.cluster_size)
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| anyhow::anyhow!("short FAT image reading u16 at {offset}"))?;
    Ok(u16::from_le_bytes(slice.try_into().unwrap()))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow::anyhow!("short FAT image reading u32 at {offset}"))?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

fn fat_time(hour: u16, minute: u16, second: u16) -> u16 {
    (hour << 11) | (minute << 5) | (second / 2)
}

fn fat_date(year: u16, month: u16, day: u16) -> u16 {
    ((year - 1980) << 9) | (month << 5) | day
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_winnt_sif_to_embedded_firadisk_image() {
        let mut image = include_bytes!("ntxp_assets/firadisk.ima").to_vec();
        let contents = b"[Data]\r\nUnattendedInstall=\"Yes\"\r\n";

        add_winnt_sif(&mut image, contents).unwrap();

        let layout = parse_layout(&image).unwrap();
        let slot = find_root_entry(&image, &layout, WINNT_SIF_DOS_NAME)
            .unwrap()
            .unwrap();
        let offset = layout.root_dir_offset + slot * 32;
        let first_cluster = u16_at(&image, offset + 26).unwrap();
        let file_size = u32_at(&image, offset + 28).unwrap() as usize;
        let data_offset = cluster_offset(&layout, first_cluster).unwrap();

        assert_eq!(file_size, contents.len());
        assert_eq!(&image[data_offset..data_offset + contents.len()], contents);
    }

    #[test]
    fn rejects_duplicate_winnt_sif() {
        let mut image = include_bytes!("ntxp_assets/firadisk.ima").to_vec();

        add_winnt_sif(&mut image, b"one").unwrap();
        let err = add_winnt_sif(&mut image, b"two").unwrap_err().to_string();

        assert!(err.contains("already contains"));
        assert!(err.contains("WINNT.SIF"));
    }

    #[test]
    fn replace_file_swaps_contents_and_frees_clusters() {
        let mut image = include_bytes!("ntxp_assets/firadisk.ima").to_vec();
        let name = dos_83_name("TXTSETUP.OEM").unwrap();
        // FiraDisk floppy already contains TXTSETUP.OEM. Replace it with a
        // larger payload (12 clusters at 512 B each) so the new cluster chain
        // is guaranteed to span more than one cluster -- this is the
        // scenario the AHCI merge path triggers in practice.
        let big = vec![b'X'; 6_000];
        replace_file(&mut image, &name, &big).unwrap();

        let read = read_file(&image, &name).unwrap().unwrap();
        assert_eq!(read, big, "replace_file payload survives cluster-chain walk");

        // Replace again with something tiny and verify the old chain was
        // freed by counting free clusters before and after.
        let layout = parse_layout(&image).unwrap();
        let free_before = count_free_clusters(&image, &layout);
        replace_file(&mut image, &name, b"tiny").unwrap();
        let free_after = count_free_clusters(&image, &layout);
        assert!(
            free_after > free_before,
            "expected freed clusters after shrinking TXTSETUP.OEM (before={free_before}, after={free_after})"
        );
        let read = read_file(&image, &name).unwrap().unwrap();
        assert_eq!(read, b"tiny");
    }

    #[test]
    fn dos_83_name_rejects_bad_inputs() {
        assert!(dos_83_name("toolongname.sys").is_err());
        assert!(dos_83_name("name.toolong").is_err());
        assert!(dos_83_name("bad name.sys").is_err());
        let n = dos_83_name("iaStor.sys").unwrap();
        assert_eq!(&n, b"IASTOR  SYS");
        let n = dos_83_name("README").unwrap();
        assert_eq!(&n, b"README     ");
    }

    #[test]
    fn list_root_files_reports_embedded_firadisk_contents() {
        // The embedded FiraDisk image ships exactly the x86 driver files
        // plus the TXTSETUP.OEM that declares both x86 AND x64 entries.
        // list_root_files is what lets the AHCI pipeline drop the x64
        // entry from [MassStorageDrivers] -- the binaries aren't here.
        let image = include_bytes!("ntxp_assets/firadisk.ima");
        let files = list_root_files(image).unwrap();
        let has = |n: &str| files.iter().any(|f| f.eq_ignore_ascii_case(n));
        assert!(has("TXTSETUP.OEM"));
        assert!(has("firadisk.sys"));
        assert!(has("firadisk.inf"));
        assert!(has("firadisk.cat"));
        assert!(!has("firadi64.sys"));
        assert!(!has("firadi64.cat"));
    }

    fn count_free_clusters(image: &[u8], layout: &Fat12Layout) -> usize {
        (2..(layout.cluster_count as u16 + 2))
            .filter(|c| read_fat12(image, layout, *c).unwrap_or(0xfff) == 0)
            .count()
    }
}

//! FAT12 helper for adding `A:\WINNT.SIF` to the FiraDisk floppy image.

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
    if contents.is_empty() {
        bail!("generated WINNT.SIF is empty");
    }

    let layout = parse_layout(image)?;
    if find_root_entry(image, &layout, WINNT_SIF_DOS_NAME)?.is_some() {
        bail!("FiraDisk floppy image already contains WINNT.SIF");
    }

    let root_slot = find_free_root_slot(image, &layout)
        .ok_or_else(|| anyhow::anyhow!("FiraDisk floppy root directory has no free entries"))?;
    let needed_clusters = contents.len().div_ceil(layout.cluster_size).max(1);
    let clusters = allocate_clusters(image, &layout, needed_clusters)
        .context("allocate FAT12 clusters for WINNT.SIF")?;

    write_file_data(image, &layout, &clusters, contents)?;
    write_root_entry(image, &layout, root_slot, clusters[0], contents.len())?;
    Ok(())
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
    first_cluster: u16,
    file_size: usize,
) -> Result<()> {
    let offset = layout.root_dir_offset + slot * 32;
    let entry = &mut image[offset..offset + 32];
    entry.fill(0);
    entry[0..11].copy_from_slice(WINNT_SIF_DOS_NAME);
    entry[11] = ATTR_ARCHIVE;
    // 2026-05-21 20:00 local FAT timestamp. Setup does not care, but stable
    // bytes make image diffs easier to reason about.
    let time = fat_time(20, 0, 0);
    let date = fat_date(2026, 5, 21);
    entry[22..24].copy_from_slice(&time.to_le_bytes());
    entry[24..26].copy_from_slice(&date.to_le_bytes());
    entry[26..28].copy_from_slice(&first_cluster.to_le_bytes());
    entry[28..32].copy_from_slice(
        &u32::try_from(file_size)
            .context("WINNT.SIF is too large for FAT12 directory entry")?
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

        assert!(err.contains("already contains WINNT.SIF"));
    }
}

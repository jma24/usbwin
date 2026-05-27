//! Minimal UDF (Universal Disk Format) directory walker.
//!
//! Vista (and a few other Microsoft install DVDs) ship a stub ISO9660
//! filesystem whose PVD root only contains `README.TXT`; the real install
//! tree lives in UDF. We need just enough UDF support to walk that tree and
//! collect uppercased path strings for the classifier in [`super`].
//!
//! References: ECMA-167 (UDF on-disc structure) and the UDF 2.60 spec.
//! Only the read-only subset required to enumerate names is implemented:
//! Anchor Volume Descriptor Pointer → Main Volume Descriptor Sequence →
//! Partition Descriptor + Logical Volume Descriptor → File Set Descriptor →
//! root File Entry → File Identifier Descriptors.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const SECTOR_SIZE: u64 = 2048;
const ANCHOR_SECTOR: u64 = 256;
const MAX_DIR_DEPTH: usize = 8;

const TAG_ANCHOR_VDP: u16 = 2;
const TAG_PARTITION_DESC: u16 = 5;
const TAG_LOGICAL_VOLUME_DESC: u16 = 6;
const TAG_TERMINATING_DESC: u16 = 8;
const TAG_FILE_SET_DESC: u16 = 256;
const TAG_FILE_IDENTIFIER: u16 = 257;
const TAG_FILE_ENTRY: u16 = 261;
const TAG_EXTENDED_FILE_ENTRY: u16 = 266;

/// Try to walk the UDF tree and return uppercased `PATH/CHILD` strings.
/// Returns `None` for any I/O or structural problem — callers should treat
/// "no UDF found" as a soft failure and fall back to ISO9660-only results.
pub fn collect_paths(path: &Path) -> Option<BTreeSet<String>> {
    let mut file = File::open(path).ok()?;
    let anchor = read_sector(&mut file, ANCHOR_SECTOR)?;
    let tag = parse_tag(&anchor)?;
    if tag.identifier != TAG_ANCHOR_VDP {
        return None;
    }

    let mvds_len = u32::from_le_bytes(anchor[16..20].try_into().ok()?);
    let mvds_loc = u32::from_le_bytes(anchor[20..24].try_into().ok()?);
    let (part_start, fsd_loc) = scan_mvds(&mut file, mvds_loc, mvds_len)?;

    let fsd_abs = part_start as u64 + fsd_loc.block as u64;
    let fsd = read_sector(&mut file, fsd_abs)?;
    let fsd_tag = parse_tag(&fsd)?;
    if fsd_tag.identifier != TAG_FILE_SET_DESC {
        return None;
    }
    // root_directory_icb is at offset 400 in the FSD body.
    let root_icb = LongAd::parse(&fsd[400..416])?;

    let mut out = BTreeSet::new();
    walk_dir(&mut file, part_start, root_icb, "", 0, &mut out);
    Some(out)
}

fn scan_mvds(file: &mut File, loc: u32, len: u32) -> Option<(u32, LongAd)> {
    let mut partition_start: Option<u32> = None;
    let mut fsd_loc: Option<LongAd> = None;
    let sectors = (len as u64).div_ceil(SECTOR_SIZE);
    for i in 0..sectors {
        let buf = read_sector(file, loc as u64 + i)?;
        let tag = parse_tag(&buf)?;
        match tag.identifier {
            TAG_PARTITION_DESC => {
                // PartitionStartingLocation @ offset 188, length @ 192
                let start = u32::from_le_bytes(buf[188..192].try_into().ok()?);
                partition_start = Some(start);
            }
            TAG_LOGICAL_VOLUME_DESC => {
                // FileSetDescriptorLocation (LongAd) @ offset 248
                fsd_loc = Some(LongAd::parse(&buf[248..264])?);
            }
            TAG_TERMINATING_DESC => break,
            _ => {}
        }
        if partition_start.is_some() && fsd_loc.is_some() {
            break;
        }
    }
    Some((partition_start?, fsd_loc?))
}

fn walk_dir(
    file: &mut File,
    part_start: u32,
    icb: LongAd,
    prefix: &str,
    depth: usize,
    out: &mut BTreeSet<String>,
) {
    if depth > MAX_DIR_DEPTH {
        return;
    }
    let fe_abs = part_start as u64 + icb.block as u64;
    let fe = match read_sector(file, fe_abs) {
        Some(b) => b,
        None => return,
    };
    let tag = match parse_tag(&fe) {
        Some(t) => t,
        None => return,
    };
    let body_offset: usize = match tag.identifier {
        TAG_FILE_ENTRY => 176,
        TAG_EXTENDED_FILE_ENTRY => 216,
        _ => return,
    };
    // ICBTag flags bits 0..=2 = allocation descriptor type. We support
    // type 0 (ShortAd) and type 3 (data embedded in file entry itself).
    let icb_flags = u16::from_le_bytes(match fe[34..36].try_into() {
        Ok(b) => b,
        Err(_) => return,
    });
    let ad_type = icb_flags & 0x7;

    let l_ea = u32::from_le_bytes(match fe[body_offset - 8..body_offset - 4].try_into() {
        Ok(b) => b,
        Err(_) => return,
    }) as usize;
    let l_ad = u32::from_le_bytes(match fe[body_offset - 4..body_offset].try_into() {
        Ok(b) => b,
        Err(_) => return,
    }) as usize;
    let ad_start = body_offset + l_ea;

    // Collect directory content extents into a single Vec<u8>.
    let mut dir_bytes: Vec<u8> = Vec::new();
    if ad_type == 3 {
        // Embedded data lives in the AD area itself.
        let end = (ad_start + l_ad).min(fe.len());
        dir_bytes.extend_from_slice(&fe[ad_start..end]);
    } else if ad_type == 0 {
        // ShortAd entries are 8 bytes each: u32 length, u32 partition-relative block.
        let mut o = ad_start;
        while o + 8 <= ad_start + l_ad && o + 8 <= fe.len() {
            let raw_len = u32::from_le_bytes(match fe[o..o + 4].try_into() {
                Ok(b) => b,
                Err(_) => break,
            });
            let block = u32::from_le_bytes(match fe[o + 4..o + 8].try_into() {
                Ok(b) => b,
                Err(_) => break,
            });
            let ext_type = raw_len >> 30;
            let ext_len = (raw_len & 0x3FFF_FFFF) as u64;
            if ext_type == 3 {
                // Next extent of allocation descriptors; we don't follow these.
                break;
            }
            if ext_len > 0 && ext_type == 0 {
                // type 0 = recorded and allocated. FID parser stops on
                // non-FID bytes so reading whole sectors (sans trim) is fine.
                let nsec = ext_len.div_ceil(SECTOR_SIZE);
                for i in 0..nsec {
                    if let Some(buf) = read_sector(file, part_start as u64 + block as u64 + i) {
                        dir_bytes.extend_from_slice(&buf);
                    }
                }
            }
            o += 8;
        }
    } else {
        // LongAd / ExtAd not implemented; most install media use ShortAd.
        return;
    }

    // Parse File Identifier Descriptors.
    let mut o = 0usize;
    while o + 38 <= dir_bytes.len() {
        let tag = match parse_tag(&dir_bytes[o..]) {
            Some(t) => t,
            None => break,
        };
        if tag.identifier != TAG_FILE_IDENTIFIER {
            break;
        }
        let characteristics = dir_bytes[o + 18];
        let l_fi = dir_bytes[o + 19] as usize;
        let l_iu = u16::from_le_bytes(match dir_bytes[o + 36..o + 38].try_into() {
            Ok(b) => b,
            Err(_) => break,
        }) as usize;
        let fid_len = 38 + l_iu + l_fi;
        let padded = (fid_len + 3) & !3;
        if o + fid_len > dir_bytes.len() {
            break;
        }

        let is_parent = characteristics & 0x08 != 0;
        let is_deleted = characteristics & 0x04 != 0;
        let is_dir = characteristics & 0x02 != 0;

        let icb = match LongAd::parse(&dir_bytes[o + 20..o + 36]) {
            Some(a) => a,
            None => break,
        };

        if !is_parent && !is_deleted && l_fi > 0 {
            let name_bytes = &dir_bytes[o + 38 + l_iu..o + 38 + l_iu + l_fi];
            if let Some(name) = decode_d_string(name_bytes) {
                let upname = name.to_uppercase();
                let path = if prefix.is_empty() {
                    upname.clone()
                } else {
                    format!("{prefix}/{upname}")
                };
                out.insert(path.clone());
                if is_dir {
                    walk_dir(file, part_start, icb, &path, depth + 1, out);
                }
            }
        }

        o += padded;
    }
}

/// Decode a UDF "d-string" identifier. First byte indicates encoding:
/// 8 = 8-bit OSTA-CS0 (subset of Latin-1), 16 = 16-bit big-endian UCS-2.
fn decode_d_string(raw: &[u8]) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    match raw[0] {
        8 => Some(raw[1..].iter().map(|&b| b as char).collect()),
        16 => {
            let body = &raw[1..];
            if body.len() % 2 != 0 {
                return None;
            }
            let mut s = String::with_capacity(body.len() / 2);
            for chunk in body.chunks_exact(2) {
                let code = u16::from_be_bytes([chunk[0], chunk[1]]);
                if let Some(c) = char::from_u32(code as u32) {
                    s.push(c);
                }
            }
            Some(s)
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
struct LongAd {
    block: u32,
}

impl LongAd {
    fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 {
            return None;
        }
        // ExtentLength @ 0..4, LogicalBlockNumber @ 4..8.
        let block = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
        Some(Self { block })
    }
}

struct Tag {
    identifier: u16,
}

fn parse_tag(buf: &[u8]) -> Option<Tag> {
    if buf.len() < 16 {
        return None;
    }
    let identifier = u16::from_le_bytes(buf[0..2].try_into().ok()?);
    Some(Tag { identifier })
}

fn read_sector(file: &mut File, sector: u64) -> Option<[u8; SECTOR_SIZE as usize]> {
    let mut buf = [0u8; SECTOR_SIZE as usize];
    file.seek(SeekFrom::Start(sector * SECTOR_SIZE)).ok()?;
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

# Performance

## Design principle: verifiability first, speed second

Speed is a non-goal until correctness is locked in. Every optimization must preserve byte-equivalent output and pass the same verify-by-default re-read check. If a change makes usbwin 5× faster but skips verification or breaks the QEMU boot test, it does not ship.

The order of work for any new feature is:

1. Make it correct (byte-equal to the reference, passes verify, passes QEMU).
2. Make it fast (within ~10% of `dd`'s baseline for raw paths).

Not the other way around. Once a path is verifiably correct it becomes a regression test, and speed work proceeds against it without fear of silent breakage.

## The reality usbwin replaces

UNetbootin copies a 700 MB Windows XP ISO to USB in **~20 minutes** on a 2026 Mac. That's roughly 580 KB/s — about 1% of what the hardware can sustain. The slowness comes from small buffers (4 KB), Java I/O overhead, per-file `fsync`, the cached `/dev/disk` device instead of raw `/dev/rdisk`, *and* the entire program running translated x86 through Rosetta on top of that. None of these are hardware constraints — they're all software the native arm64 Rust binary simply doesn't pay.

Measured baseline on the same hardware: `sudo dd if=winxp.iso of=/dev/rdisk8 bs=4m` completes in **~7 seconds** for the same 700 MB ISO. That's ~100 MB/s — and that's the bar.

## Targets

| Operation                  | Target rate     | 700 MB XP ISO   | 4 GB Win10 ISO  |
|----------------------------|-----------------|-----------------|-----------------|
| Hybrid mode (raw write)    | 90–200 MB/s     | ≤ 10 s          | ≤ 50 s          |
| Windows mode (FAT32 fcopy) | 30–60 MB/s      | ≤ 25 s          | ≤ 2 min         |
| UEFI mode (FAT32 EFI dir)  | 30–60 MB/s      | n/a             | ≤ 30 s          |

"Hybrid mode target" = match `dd` within ~30% after subtracting unmount + verify + eject overhead. Reaching parity with `dd` is the explicit goal once correctness ships.

## The other bottleneck: per-file overhead

UNetbootin processes a 7000-file Windows ISO at roughly **2 files/sec** — that's fixed-cost per file, almost certainly an `fsync` after every file plus tiny synchronous syscalls. Total bandwidth is irrelevant when each file has a 500ms fixed cost.

Modern FAT32 on a decent USB 3 stick should sustain **hundreds of files/sec for small files**, and ~50 MB/s for large ones. Per-file targets:

- 500+ small files/sec (≤ 64 KB) on FAT32
- 50 MB/s sustained for large files (≥ 1 MB)

Per-file design rules (in addition to the byte-oriented rules above):

- **No `fsync` between files.** One `fsync()` at the very end, before unmount/eject. macOS's USB stack flushes correctly on eject.
- **Batch directory updates** by using `std::fs::copy` (which uses `copyfile(3)` on macOS) rather than open/write/close per-byte.
- **Show files/sec AND MB/sec** in the progress bar. A file-count-heavy ISO (Windows installer) and a few-large-files ISO (Linux squashfs) look totally different to the user; both numbers matter.
- **Pre-create the directory tree** in one pass, then stream file contents. Don't interleave directory creation with file writes — that thrashes the directory entries.

These are *targets*, not measured numbers. Verified during hardware testing; regressions caught in `tests/perf_smoke.rs` (TODO, gated behind `--features perf-tests`).

## macOS gotcha: sub-sector writes silently fail on /dev/rdiskN

Empirically verified (see `FIELD_FINDINGS_2026_05_18.md` §2): on macOS the raw character device `/dev/rdiskN` **silently drops writes** smaller than its sector size (typically 512 bytes). The `write()` call returns the requested byte count but the bytes never reach the disk. `/dev/diskN` (the buffered/cached variant) handles sub-sector writes correctly because the kernel buffers them.

usbwin's two write paths must respect this:

- **Full-sector and multi-sector writes** (ISO data, MBR sector 0, PBR sector splice, the whole pipeline data plane): use `/dev/rdiskN`. We get 3–5× the throughput and no silent failures because every write we make is sector-aligned.
- **Sub-sector writes** (e.g. patching a single byte in a partition table without rewriting the whole sector): never issued by usbwin. Where we conceptually want to change a few bytes, we read the affected sector, modify in memory, write the whole sector back. This is what `splice_fat32_pbr` does.

If a future code path ever needs sub-sector writes, it must implement read-modify-write of the whole sector — *or* fall back to `/dev/diskN` for that specific write and accept the throughput cost.

## Design rules

1. **Always use the raw device.** `/dev/rdiskN`, never `/dev/diskN`, for any block-level read or write. The cached device introduces a buffer cache pass that costs 3–5× throughput on large sequential writes.

2. **Large buffers.** 1–4 MiB chunks for raw writes, 1 MiB for file copies. The default `std::io::copy` buffer (8 KB) is wildly too small for USB writes.

3. **`F_NOCACHE` for source-side reads** where applicable. We're not going to read the data again; don't pollute the unified buffer cache.

4. **No `fsync` per file.** One `fsync()` at the end of the whole copy is enough. macOS's USB drivers flush correctly on eject.

5. **Sequential read hint.** Use `F_RDADVISE` on the ISO file to tell the kernel we'll read sequentially. Minor win but free.

6. **Single-pass copy.** For Windows mode, read each file from the mounted ISO and stream straight to the FAT32 USB. No staging directories. No intermediate buffers beyond the read/write chunk.

7. **Parallel copy is not worth it for FAT32.** Directory metadata lock contention eats the wins. Stick with one worker; the bottleneck is the USB controller, not CPU.

8. **Show progress.** `indicatif`-style progress bar with throughput readout. Users tolerate slow if they can see it moving; UNetbootin's silent 20 minutes is what trains people to assume the tool has hung.

## Where the bytes flow (Windows mode)

```
ISO file (mounted via hdiutil)
    │ read in 1 MiB chunks, F_NOCACHE on source
    ▼
1 MiB chunk in userspace
    │ write straight to mounted FAT32 USB
    ▼
/Volumes/WIN7/sources/install.wim
    │ (eventual) ejector triggers final fsync
    ▼
USB controller writeback complete
```

Boot-record writes are a separate code path on `/dev/rdiskN`, after the file copy, also with 1 MiB+ buffers. They're tiny (1 KB total) so buffer size doesn't matter for them — but the `/dev/rdiskN` vs `/dev/diskN` choice still does.

## How we benchmark

`tests/perf_smoke.rs` (TODO) writes a 100 MB synthetic ISO to a RAM-disk-backed loopback device and measures wall time. Goal: assert we're within 2× of a raw `dd` baseline. Wired into `cargo test --release --features perf-tests` to keep it out of the default test path.

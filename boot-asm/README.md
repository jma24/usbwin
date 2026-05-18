# boot-asm

Hand-written NASM source for the three boot-record blobs usbwin embeds.

| File           | What it does                                                    |
|----------------|-----------------------------------------------------------------|
| `mbr.asm`      | Generic MBR: find the active primary partition, chain-load it.  |
| `fat32_pbr.asm`| FAT32 PBR: read BPB, walk FAT, load `bootmgr`, jump.            |
| `ntfs_pbr.asm` | NTFS PBR: same shape but walks NTFS structures.                 |

Each file assembles to **exactly 512 bytes** of raw binary. The build is invoked from `crates/usbwin-boot/build.rs` when the `embed-boot-asm` feature is on.

## Manual build

```sh
brew install nasm
cd boot-asm
make
ls -l build/    # mbr.bin fat32_pbr.bin ntfs_pbr.bin, 512 bytes each
```

## Verifying correctness

Three layers, ordered by feedback-loop speed. See [`../docs/BOOT_RECORDS.md`](../docs/BOOT_RECORDS.md) for the full story.

1. **`cargo test`** in `crates/usbwin-boot/` — unit tests on the splice logic.
2. **Byte equality vs ms-sys** (gated): `cargo test --features compare-mssys` with `USBWIN_MSSYS_BLOBS_DIR` set.
3. **QEMU smoke test**: `cargo test --test qemu_boot` boots a synthetic FAT32 image whose first sector uses our PBR.
4. **Real hardware**: `docs/HARDWARE_TESTS.md`.

## Status

**These files are stubs.** They assemble to 512 bytes of mostly-NOPs with the boot signature, enough to keep the build green. Real bootloader code is the next major chunk of work — see `docs/BOOT_RECORDS.md` for the contract each blob must satisfy.

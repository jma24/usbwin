# usbwin

**This is the tool Rufus refuses to be.**

A native macOS arm64 CLI for writing bootable USB sticks from any ISO — Windows (7 through 11), Linux/BSD (hybrid or isolinux), or UEFI-only — without Rosetta, without a Windows VM, without Boot Camp Assistant.

## Status

Alpha. **Windows 7 install USB boots on real legacy-BIOS hardware** (Dell E6410, verified 2026-05-19), end-to-end from `.iso` to "Install now" screen. The Win 7 boot chain is shared with Win 8/10/11, so the same code path covers all four. Hybrid Linux/BSD mode is the v0.1 baseline. Windows XP mode (Grub4DOS-style chainloader + `txtsetup.sif` handling) is implemented as a separate `--type=windows-xp` path; isolinux Linux and UEFI-only modes come later.

The MBR + FAT32 PBR bytes come from the clean-room [`bootrec`](https://github.com/jma24/bootrec) library by default, linked in-process — no external `ms-sys` binary required. The legacy `ms-sys` shell-out is still available as `--boot-record=ms-sys` for byte-equality auditing.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the design and [`docs/V1_BOOTREC_LIBRARY.md`](docs/V1_BOOTREC_LIBRARY.md) for the bootrec spec, including why we splice the FAT32 PBR instead of replacing it (to preserve the BPB the formatter wrote).

## Why

- UNetbootin requires Rosetta.
- Rufus is Windows-only.
- Boot Camp Assistant was removed on Apple Silicon.
- `dd` works for hybrid ISOs but silently produces a non-bootable Windows USB.

There is currently no native macOS arm64 binary that writes a bootable Windows install USB. This is that binary.

## Install

```sh
# Build from source (requires Rust stable + NASM for bootrec's NASM blobs)
brew install nasm
git clone https://github.com/jma24/bootrec ../bootrec   # path dep — see Cargo.toml
git clone https://github.com/jma24/usbwin
cd usbwin
cargo build --release
sudo cp target/release/usbwin /usr/local/bin/
```

The `embed-boot-asm` feature on `bootrec` is enabled by usbwin's `Cargo.toml`,
so a plain `cargo build --release` is enough — no extra flags.

### Optional: ms-sys fallback

By default, usbwin uses the in-process `bootrec` library for MBR and FAT32
PBR bytes. If you want byte-for-byte equivalence with the upstream tool
(useful for auditing or comparison against a known-good Win 7 USB), pass
`--boot-record=ms-sys` and install ms-sys once:

```sh
git clone https://gitlab.com/cmaiolino/ms-sys.git /tmp/ms-sys
cd /tmp/ms-sys && make
sudo cp bin/ms-sys /usr/local/bin/
# Or without sudo: export USBWIN_MS_SYS=/tmp/ms-sys/bin/ms-sys
```

Hybrid mode (Linux/BSD ISOs) does not touch the boot-record path at all.

Notarized signed binaries via GitHub Releases: TODO.

## Test prerequisites

The default `cargo test` only needs Rust. Boot-record-level integration tests
(QEMU smoke, ms-sys byte-equality) live in the [bootrec](https://github.com/jma24/bootrec)
repo — run them there.

## Usage

```sh
usbwin <iso-path> <device>
       [--type=auto|windows|windows-xp|linux|hybrid|uefi]
       [--label=<volume-label>]
       [--boot-record=bootrec|ms-sys]
       [--dry-run]
       [--force]
       [--verbose]
       [--no-verify]
```

Examples:

```sh
sudo usbwin Win7_SP1.iso /dev/disk8
sudo usbwin ubuntu-22.04.iso /dev/disk8 --type=hybrid
usbwin --dry-run Win7_SP1.iso /dev/disk8     # no sudo needed; emits bytes to /tmp
```

## Safety

usbwin will **refuse** to write to:

- The boot disk
- Any disk flagged `internal: true` by DiskArbitration
- Disks larger than 256 GiB without `--force`

Every write is verified by re-reading and byte-comparing unless `--no-verify` is passed.

## License

MIT. See [`LICENSE`](LICENSE).

Boot record source code lives in the separate [`bootrec`](https://github.com/jma24/bootrec) repo, with its own clean-room provenance trail.

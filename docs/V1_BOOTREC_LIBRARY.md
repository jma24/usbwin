# V1.0 design: `bootrec` — a clean-room boot-record library

## One-paragraph summary

A standalone MIT-licensed Rust library that produces Microsoft-compatible MBR
and FAT32/NTFS boot record byte sequences, replacing usbwin's runtime
dependency on ms-sys. The library is developed **eval-first**: a verification
harness using ms-sys as a comparison oracle and QEMU as a boot tester is
written before any boot code. The library is "done" for a given variant when
its eval passes — predictable shipping, no more debugging-the-PBR-blind
sessions.

## Why this exists

The v0.2 / v0.3 path shells out to ms-sys for the MBR and PBR bytes. That
works but has three real costs:

1. **External GPL-2 dependency** — usbwin is MIT, ms-sys is GPL-2. The
   process-boundary separation keeps the licenses compatible, but a
   self-contained MIT binary would be cleaner for redistribution.
2. **Installation friction** — users must `git clone gitlab.com/cmaiolino/ms-sys`
   then build it. Hampers "one-command install."
3. **No control** — bug in ms-sys (e.g. the sub-sector-write-on-rdisk
   issue from FIELD_FINDINGS §2) requires upstream patching or working
   around in our shell-out layer. With our own implementation, we fix it.

Why we didn't do this in v0.2: trying to write a clean-room PBR without an
eval framework first led to a 6-hour debugging spiral against unreliable
test infrastructure (see chat history May 18). The lesson — test framework
**before** boot code — is now baked into this spec.

## Verifiability hierarchy

Four nested layers, from tightest signal to loosest. Each layer is an
independent test that any candidate boot record can be run against. A
production-quality boot record passes all four.

### Layer 1 — Byte-equality vs ms-sys (tightest)

For each variant (e.g. `--fat32pe`, `--mbr7`), `bootrec` produces N bytes
and we assert them byte-equal to ms-sys's output for the same input.

```rust
#[test]
fn fat32pe_matches_mssys() {
    let our_bytes  = bootrec::fat32_pbr_bootmgr(/*bpb*/...);
    let mssys_bytes = oracle::ms_sys_fat32pe(/*bpb*/...);
    assert_eq!(our_bytes, mssys_bytes);
}
```

**Confidence:** if the bytes match, we know we're producing Microsoft-
equivalent code (because ms-sys IS Microsoft's code, extracted).

**Limitation:** byte equality is *sufficient* but not *necessary*. We
might validly produce different bytes that boot equivalently (different
register-allocation strategy in NASM, different jump patterns, etc.).
Treat this layer as the **strongest** signal but not the **only** one.

### Layer 2 — QEMU boot smoke test (per variant)

For each variant, build a synthetic disk image with `bootrec`'s output
applied, boot it under `qemu-system-i386`, verify a specific success
signal on serial.

The synthetic test environments:

- **FAT32 + NTLDR stub**: tiny partition with a fake `NTLDR` that prints
  `BOOTREC OK\r\n` to COM1 and halts. Validates `fat32_pbr_ntldr`.
- **FAT32 + bootmgr stub**: same, fake `BOOTMGR`. Validates
  `fat32_pbr_bootmgr` (the multi-sector one).
- **MBR + active partition + dummy PBR that just prints to serial**:
  validates `mbr_7`, `mbr_xp`.

Test infrastructure already half-built (`crates/usbwin-boot/tests/qemu_pbr.rs`
from v0.2 work). Generalize it into a per-variant harness.

### Layer 3 — Real-content boot test

Same as Layer 2 but the disk image contains a real-but-stripped Windows
install tree (e.g. a 50 MB subset of Win 7 with the bootmgr loader chain).
Boots far enough to reach the Windows boot menu / installer welcome.

Slower than Layer 2 (~10s per run vs ~3s) but catches issues where
`bootmgr` itself doesn't like our PBR (the multi-sector handoff edge
cases that the spec doesn't fully document).

### Layer 4 — Real-hardware smoke

Pre-release checklist run manually on a small set of target machines:

- Dell E6410 (Phoenix BIOS, USB-as-HDD-with-quirks)
- Generic 2010-2015 Intel desktop (modern AMI BIOS)
- A 2005-vintage P4 box (legacy Phoenix BIOS, may treat USB as floppy)

For each: write a USB, boot, confirm reaches "Setup is loading drivers..."
within 30 seconds. Document the results in `HARDWARE_TESTS.md`.

## Library scope (v1.0 target)

Public API:

```rust
// Master Boot Records (whole-disk, sector 0).
pub fn mbr_win7(disk: DiskGeometry, partitions: &[PartitionEntry]) -> [u8; 512];
pub fn mbr_xp(disk: DiskGeometry, partitions: &[PartitionEntry]) -> [u8; 512];

// FAT32 Partition Boot Records (partition-local, 1 sector for XP, ~16 for Win7+).
pub fn fat32_pbr_ntldr(bpb: Fat32Bpb) -> [u8; 512];
pub fn fat32_pbr_bootmgr(bpb: Fat32Bpb) -> PbrBytes;       // multi-sector

// NTFS Partition Boot Records.
pub fn ntfs_pbr_bootmgr(bpb: NtfsBpb) -> PbrBytes;

// Optional: byte-level splice helpers (preserve existing BPB on a freshly
// formatted device while overwriting boot code).
pub fn splice_fat32_pbr(existing: [u8; 512], boot_code: &PbrBytes) -> [u8; 8192];
```

Out of scope for v1.0:
- exFAT boot records
- Other-OS boot records (syslinux, GRUB stage 1)
- UEFI boot variants
- Partitioning utilities (that's usbwin's job)

## Component breakdown — order of implementation

Sequenced by complexity. Each component is implementable AND testable
against Layers 1-2 before moving to the next.

| # | Component             | Bytes  | Complexity | Eval status target |
|---|------------------------|--------|------------|---------------------|
| 1 | `mbr_xp`               | 512    | Low        | Layer 1 + Layer 2 |
| 2 | `mbr_win7`             | 512    | Low        | Layer 1 + Layer 2 |
| 3 | `fat32_pbr_ntldr`      | 512    | Medium     | Layer 1 + Layer 2 + Layer 3 |
| 4 | `fat32_pbr_bootmgr`    | ~6 KB (sectors 0, 1, 12) | High | Layer 2 + Layer 3 + Layer 4 |
| 5 | `ntfs_pbr_bootmgr`     | ~16 KB | High       | Layer 2 + Layer 3 |

Items 1-2 are quick wins; they get us off ms-sys for MBR work within a
week. Item 3 (XP PBR) is the next-most-tractable. Item 4 is the hard one
(multi-sector, multi-stage, the thing we hit a wall on previously); it
gets the most eval scrutiny. Item 5 only matters if usbwin grows NTFS
support, which is not currently planned.

## Project layout

A new sibling Rust crate, either inside the usbwin workspace or
standalone:

```
bootrec/
├── Cargo.toml                # MIT-licensed
├── README.md
├── src/
│   ├── lib.rs                # Public API (above)
│   ├── mbr.rs                # MBR variant assembly + byte layout
│   ├── pbr/
│   │   ├── fat32_ntldr.rs    # PBR variant
│   │   ├── fat32_bootmgr.rs  # Multi-sector PBR
│   │   └── ntfs_bootmgr.rs
│   └── geometry.rs           # DiskGeometry, PartitionEntry, Fat32Bpb, NtfsBpb
├── boot-asm/                 # NASM source files
│   ├── mbr_xp.asm
│   ├── mbr_win7.asm
│   ├── fat32_pbr_ntldr.asm
│   ├── fat32_pbr_bootmgr/    # Multi-file because multi-sector
│   │   ├── sector0.asm
│   │   ├── sector1.asm
│   │   └── sector12.asm
│   └── ntfs_pbr_bootmgr/
├── tests/
│   ├── oracle/               # Layer 1: ms-sys comparison
│   │   ├── mod.rs            # Wraps ms-sys, parses output
│   │   ├── mbr_oracle.rs
│   │   └── pbr_oracle.rs
│   ├── qemu/                 # Layer 2: synthetic boot tests
│   │   ├── mod.rs            # Harness for spinning up qemu-system-i386
│   │   ├── fake_ntldr.asm    # 30-byte stub that prints to serial
│   │   ├── fake_bootmgr.asm  # similar
│   │   ├── mbr_smoke.rs
│   │   └── pbr_smoke.rs
│   ├── real_content/         # Layer 3: tests against real Windows files
│   │   └── ...               # See "Real-content fixtures" below
│   └── fixtures/             # Small data files (BPBs, partition tables)
└── docs/
    └── PROVENANCE.md         # Clean-room protocol (inherited from usbwin)
```

## Eval-first workflow (how to actually develop a variant)

This is the methodology that fixes the "blind debugging" failure mode.

### Step 0 — Before writing any boot code: wire up the eval

For variant N (e.g. `fat32_pbr_ntldr`):

1. Build the oracle: a function `expected_bytes(input) -> Vec<u8>` that
   runs ms-sys on a synthetic disk and extracts the resulting boot record.
2. Build the smoke test: a function `boots_ok(bytes) -> bool` that splices
   `bytes` into a synthetic image and runs it under QEMU, returning
   whether the success marker appeared on serial.
3. Write a stub function `our_bytes(input) -> Vec<u8>` that returns
   `vec![0; 512]` (or whatever the wrong answer is).
4. Confirm: `expected_bytes(...) != our_bytes(...)` (Layer 1 fails) and
   `!boots_ok(our_bytes(...))` (Layer 2 fails).

**The evals fail at this point. That's the point.** You can't accidentally
think you've shipped something when the eval still fails.

### Step 1 — Implement until Layer 2 passes

Write NASM code. Build the bytes. Re-run the eval. Iterate.

Layer 2 (QEMU boot smoke) is the **primary** signal during development.
It's a binary pass/fail and the tightest correctness check that doesn't
require ms-sys.

### Step 2 — Add Layer 1 (byte-equality vs ms-sys)

Once Layer 2 passes, compare to ms-sys's bytes. Three outcomes:

- **Byte-identical**: ship.
- **Byte-different but boot-equivalent**: ship; document why.
- **Byte-different and one doesn't boot**: figure out which one is right.

### Step 3 — Layer 3 + Layer 4 before release

Real-content fixtures and hardware smoke before the variant is declared
"production." See per-variant target in the table above.

## Real-content fixtures (Layer 3)

We need test inputs that look like actual Microsoft install media but are
small enough to commit. Per-variant:

- **NTLDR variant**: A ~5 MB synthetic FAT32 with real-shaped `NTLDR` file
  (just enough to load and print a marker). Built from the actual Win XP
  files extracted from the user's ISO into `tests/fixtures/xp_minimal/`.
- **BOOTMGR variant**: ~10 MB synthetic FAT32 with real Win 7 `bootmgr`
  plus `Boot/BCD`. Generated from a Win 7 ISO at test-fixture-build time.

Fixtures are reproducible (a `tests/fixtures/build.sh` script generates
them from an ISO path the developer supplies via env var). The repo
doesn't check in the fixtures themselves (license, size); it checks in
the build script and the SHA-256 of the expected outputs so we know when
the fixture changed.

## Clean-room protocol (air-gapped from ms-sys source)

This is the strictest form of the protocol described in usbwin's
`docs/PROVENANCE.md` — what intellectual-property law calls a "Chinese
Wall" or "clean-room" reimplementation, the same shape Compaq used in
1984 to reimplement the IBM PC BIOS without copyright infringement.

### The air gap

Two distinct roles exist in `bootrec` development:

| Role            | What they see                                           | What they produce               |
|-----------------|---------------------------------------------------------|----------------------------------|
| **Spec readers** | FAT32 spec, BIOS docs, ms-sys's *output bytes* (as a black box) | bootrec source code (NASM, Rust) |
| **Oracle plumbing** | Whatever they need; usually nothing | The test harness that invokes ms-sys as subprocess and compares its output to bootrec's |

The same person can do both *as long as the Spec-reader role never touches
ms-sys's source code*. ms-sys's `.c` and `.h` files (especially `inc/*.h`
which contain the actual boot-record byte arrays as C arrays) are
**forbidden reading** for anyone writing bootrec.

If a contributor has read ms-sys source code, they're tainted for the
duration of their useful memory of it (months, conservatively). They can
work on the oracle/test harness but not on the boot-code source files.

### Allowed references for the Spec-reader role

- Microsoft FAT32 spec (FATGEN103.doc) — publicly published
- Microsoft NTFS spec (public docs)
- IBM/Phoenix BIOS Interface Reference (INT 13h, INT 10h, etc.)
- USB Mass Storage Class spec
- USB / OHCI / UHCI / EHCI controller specifications
- OSDev wiki *prose and pseudocode only* (never code blocks)
- Microsoft's own SDK headers describing on-disk structures (e.g.
  `winioctl.h`'s partition table layout)
- IDA-decompiled views of *generic* bootloaders if no Microsoft binary
  is involved (still risky; ask first)

### Disallowed for the Spec-reader role

- **ms-sys source files** — `src/*.c`, `inc/*.h`, anything in the ms-sys
  repository besides the compiled binary
- syslinux, GRUB, GRUB4DOS, Linux kernel boot code, BSD bootloaders
- Microsoft Windows source leaks (even if not Microsoft-attributed)
- Any reverse-engineered disassembly of `bootmgr`, `ntldr`, `bootsect.exe`,
  or `sys.exe`
- Stack Overflow / forum posts that contain code blocks from the disallowed
  sources (prose-only reading is fine)

### How ms-sys appears in the codebase

**Only as a black-box subprocess in `tests/oracle/`.** The harness shape:

```rust
// tests/oracle/mod.rs
fn ms_sys_output(args: &[&str], target_device: &str) -> Result<Vec<u8>> {
    // Invoke `/usr/local/bin/ms-sys <args> <target_device>`
    // Then read back the bytes from <target_device>
    // Return the resulting boot record bytes
}
```

No `#include "ms-sys/inc/some_header.h"`, no `let bytes =
include_bytes!("../../ms-sys/inc/winnt5_fat32_bootcode.h")`, no
`Command::new("cat").arg("ms-sys/src/file_system.c")`. The library has
*no awareness* of how ms-sys produces its bytes; it only knows what they
are.

### Why this matters

ms-sys's boot-record byte arrays in `inc/*.h` are themselves derived from
Microsoft binaries. Their legal status was always murky — the ms-sys
maintainers shipped them under GPL-2 because that's what FSF's "make
everything free" philosophy says to do with code that's already in the
wild, not because they had a license from Microsoft to redistribute. A
clean-room reimplementation that never sees ms-sys's bytes (only their
output, which is observed behavior, not protected expression) sidesteps
this entire question.

bootrec's bytes will be derived solely from the FAT32 / NTFS / BIOS specs.
If they happen to be byte-identical to Microsoft's bytes (because the
space of "correct implementations of this small task" is small), that's
parallel invention, not copying.

## License

`bootrec` is MIT-2.0. Independent of usbwin (could be used by other
tools — e.g. a Linux LiveUSB creator, a forensic image preparation tool,
a retro-computing utility). Single self-contained crate.

ms-sys's GPL-2 license doesn't transit into `bootrec` because:
- We don't link, include, or distribute ms-sys
- Test-time subprocess invocation is mere aggregation (per FSF)
- Output bytes are not copyrightable (data, not creative expression — and
  even if it were, our implementation derives them from the FAT32 spec,
  not from observing ms-sys output)

## Timeline estimate

Honest, not optimistic.

| Phase                        | Weeks | Cumulative |
|------------------------------|-------|-------------|
| Eval framework (Layers 1, 2)  | 1     | 1           |
| `mbr_xp` + `mbr_win7`        | 1     | 2           |
| `fat32_pbr_ntldr` to L2       | 2     | 4           |
| `fat32_pbr_ntldr` to L3       | 1     | 5           |
| `fat32_pbr_bootmgr` to L2     | 4     | 9           |
| `fat32_pbr_bootmgr` to L3     | 2     | 11          |
| Real-hardware verification    | 2     | 13          |
| Documentation + 1.0 release   | 1     | 14          |

About 3-4 months part-time. Predictable because of the eval-first
methodology — at each milestone we either pass the layer's test or
we don't, no ambiguity.

## What kicks off v1.0 work

This spec lands today. v1.0 work doesn't start until:

1. v0.2 (Win 7 mode via ms-sys) is **real-hardware verified** on the
   Dell E6410.
2. v0.3 (XP mode via ms-sys) is **real-hardware verified** on the same.
3. There's a concrete reason to invest 3 months — public release plan,
   external interest, etc.

Until then, ms-sys is the right answer.

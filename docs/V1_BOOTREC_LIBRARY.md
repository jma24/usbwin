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

## Verifiable evidence

Two independent verifiability claims; each needs concrete mechanisms.
Policy without machinery is just a wish.

### Verifiably correct: machine-checked, CI-enforced

For each variant, the eval layers produce binary pass/fail signals that
CI runs on every commit:

```yaml
# .github/workflows/verify.yml (sketch)
correctness:
  - layer1_oracle:      # Byte-equality vs ms-sys
      gates: [all-variants-equal] | [variant-equal-or-justified]
      run-on: every PR
  - layer2_qemu:        # Synthetic boot smoke
      gates: [all-stubs-print-marker]
      run-on: every PR
  - layer3_real:        # Real-content boot test
      gates: [reaches-installer-welcome]
      run-on: PR + nightly
  - layer4_hardware:    # Manual sign-off
      gates: [signed-off-by HARDWARE_TESTS.md commit]
      run-on: release-gate
```

Concrete correctness deliverables per variant:

1. **Coverage matrix** (`COVERAGE.md`) — variant × eval-layer × pass/fail.
   No variant ships if any required layer is RED.
2. **Determinism check** — `cargo build` produces byte-identical output
   across runs / clean checkouts / different developer machines.
   Verified via `tests/determinism.sh` that runs `cargo clean && cargo
   build --release && sha256sum target/release/...`. CI fails on diff.
3. **Reproducible from spec** — `docs/SPEC_TRACE.md` maps each non-trivial
   constant in our boot code to the spec page that justifies it
   (FAT32 BPB offset 0x0B == BytsPerSec → FATGEN103 §3.1). Catch
   "magic number copied from somewhere" early.
4. **Regression fixtures** — for each variant, a fixed input → fixed
   output mapping in `tests/golden/`. Changes require explicit fixture
   updates with justification in the commit message.

### Verifiably green-room: process + machinery

Air-gap is a policy that humans can break (or be subtly tainted by).
The mechanisms that catch the breakage:

#### 1. Contributor reading declaration (per-PR)

Every PR that touches `bootrec/src/` or `bootrec/boot-asm/` includes
a YAML block in the description:

```yaml
clean_room:
  role: spec-reader              # or "oracle-plumbing"
  references_consulted:
    - "FATGEN103.doc §3.1-3.3 (BPB layout)"
    - "Phoenix BIOS Interface Reference §INT 13h, fn 0x42"
    - "osdev.org/FAT (prose only; verified no code blocks read)"
  forbidden_unread:
    - ms-sys/src/         # I have not read these files
    - ms-sys/inc/         # ever
    - syslinux/           # not in last 12 months
    - any-Windows-source-leak
  attestation: |
    I am not aware of having read ms-sys source code, leaked Microsoft
    boot record source, or any GPL/BSD bootloader source within the
    last 12 months. The code in this PR was derived solely from the
    references listed above.
  signed: $CONTRIBUTOR_NAME
  date: 2026-XX-XX
```

This goes in the PR description (not the commit, so it's separable
from the code). It's a sworn-style attestation, not legally binding,
but it creates a paper trail.

#### 2. Reading log (`CONTRIBUTORS_READING.md`)

A repository-tracked file listing, per contributor, what reference
sources they've read AT ALL, with timestamps. Append-only. A
contributor's eligibility for the spec-reader role is determined by
checking this log:

```markdown
## joa@example.com

| Source                                | Read     | Status        |
|----------------------------------------|----------|----------------|
| FATGEN103.doc (FAT32 spec)             | 2026-05  | ✓ allowed     |
| Phoenix BIOS Interface Ref             | 2026-05  | ✓ allowed     |
| osdev.org/FAT prose                    | 2026-06  | ✓ allowed     |
| ms-sys/inc/*.h byte arrays             | 2018     | ❌ tainted    |
```

If "tainted" sources appear, the contributor cannot work on
`bootrec/src/` (boot code) — only on `bootrec/tests/oracle/` (where
seeing ms-sys output is the whole point). The tainting half-life is
conservatively 24 months from last-read; after that, on case-by-case
basis with project-lead sign-off.

#### 3. Forbidden-symbol grep (CI gate)

A simple CI check that fails the build if any of these patterns appear
anywhere in `bootrec/src/` or `bootrec/boot-asm/`:

```sh
# .github/workflows/clean_room_check.sh
FORBIDDEN_PATTERNS=(
    "ms-sys"             # literal name (shouldn't appear in source)
    "mssys"
    "ilko-y"             # the WaitBT author
    "syslinux"
    "ldlinux"
    "include.*ms-sys"
    "/* extracted from"  # common "I copied this" comment style
)
for pattern in "${FORBIDDEN_PATTERNS[@]}"; do
    if grep -r "$pattern" bootrec/src/ bootrec/boot-asm/; then
        echo "FORBIDDEN PATTERN FOUND: $pattern"
        exit 1
    fi
done
```

Trivial check, but catches the dumbest "I copied a block of bytes from
that one .h file" leaks.

#### 4. Statistical similarity check (CI gate)

For each variant where ms-sys produces equivalent bytes, compute the
"non-trivial similarity" between our bytes and ms-sys's:

- Both files produce 512 bytes.
- ~250 of those are the BPB area (which both must produce identically — it's
  filesystem state, not boot code).
- The remaining ~260 bytes are the boot code itself.

Trivial similarity (same opcodes for the same operations) is fine. Excessive
similarity beyond what FAT32 setup-code structure mandates is a flag.

Concrete measurement: for the 260-byte boot-code region, compute
Hamming distance between our output and ms-sys's. Plot the distribution
across all variants. Flag any variant where the Hamming distance is
suspiciously low (e.g. fewer than 10 differing bytes when the function
is non-trivial — that suggests copy or extremely-likely accidental
parallel invention, both worth a manual review).

This is a soft signal, not a hard gate. It triggers an "investigate
this" rather than "fail the build."

#### 5. Independent code review (per release)

Before each `bootrec` release tag, the boot-code files (`bootrec/src/`,
`bootrec/boot-asm/`) are reviewed by a contributor who has *not*
written any of the code, with explicit focus on: "does this look
clean-room, or does anything look copy-pasted from somewhere
familiar?" The reviewer also confirms the contributor reading log
matches the PR claims.

For a single-contributor project, this step is "contributor reviews
their own code with the explicit checklist, in writing." Not as
strong as two-person review but catches the "I just did this without
thinking" cases.

#### 6. Public legal review (before 1.0)

Before declaring a 1.0 release, a one-time review by a lawyer
familiar with clean-room reverse engineering (or at minimum, a
public RFC review with knowledgeable hobbyist community input from
e.g. msfn.org). Document the review outcomes in
`docs/LEGAL_REVIEW.md`.

### Both verifiability properties combined

A pull request is mergeable to `main` only when ALL of these are
green:

| Check                          | Verifies      | Automated? |
|--------------------------------|---------------|------------|
| Layer 1 oracle test            | Correctness   | ✓ CI       |
| Layer 2 QEMU smoke             | Correctness   | ✓ CI       |
| Layer 3 real-content (if req)  | Correctness   | ✓ CI       |
| Determinism check              | Correctness   | ✓ CI       |
| Coverage matrix updated        | Correctness   | ✓ CI       |
| Clean-room declaration in PR    | Cleanroom     | semi (lint) |
| Reading log updated             | Cleanroom     | semi (lint) |
| Forbidden-symbol grep clean    | Cleanroom     | ✓ CI       |
| Statistical similarity below threshold | Cleanroom | ✓ CI       |
| Independent review sign-off    | Both          | manual      |

Layer 4 (real hardware) is required at release-gate, not per-PR.

The combination is what makes the claim **verifiable**: if any
reviewer in the next 30 years wants to challenge whether bootrec is
genuinely clean-room, they can read the reading log, audit the PR
attestations, run the forbidden-symbol checks themselves, and inspect
the similarity-distribution data. Nothing depends on trusting the
authors' word.

## Form factor: library AND binary

`bootrec` ships as a single Cargo crate that produces both a Rust library
and a CLI binary. The same code, two consumption modes.

### Library (`bootrec` Rust crate)

The canonical API. usbwin links against it directly, gets Rust-typed
input (`Fat32Bpb`, `DiskGeometry`, etc.) and Rust-typed output
(`[u8; 512]`, `PbrBytes`). No subprocess overhead, no string parsing,
no shell escaping. usbwin's `pipeline/windows.rs` switches from
`Command::new(ms_sys).args(...)` to `bootrec::fat32_pbr_bootmgr(bpb)`.

```rust
// In usbwin's Cargo.toml:
bootrec = { path = "../bootrec" }   // or version = "1.0" when published

// In usbwin's pipeline/windows.rs:
let pbr_bytes = bootrec::fat32_pbr_bootmgr(bpb);
dev.write_at(0, &pbr_bytes[0])?;    // sector 0
dev.write_at(512, &pbr_bytes[1])?;  // sector 1
dev.write_at(12 * 512, &pbr_bytes[12])?;  // sector 12
```

### Binary (`bootrec` CLI)

A thin wrapper that exposes the library as a command-line tool — a
drop-in replacement for ms-sys for the variants we support. ~50 lines
of clap-based argument parsing around library calls.

```sh
# usbwin's --mbr7 equivalent
bootrec --mbr-win7 /dev/rdisk6

# usbwin's --fat32pe equivalent
bootrec --fat32-bootmgr /dev/rdisk6s1

# Or by variant explicitly
bootrec --variant fat32-bootmgr --output /dev/rdisk6s1
```

The CLI uses the SAME library functions internally. The binary form
exists because:

1. **Drop-in for existing recipes** — anyone using ms-sys in a shell
   script can switch by changing `ms-sys --fat32pe` to `bootrec
   --fat32-bootmgr`. Lowers adoption friction for the broader
   USB-tool ecosystem (WinSetupFromUSB-likes, retro-computing folks).
2. **Cross-language interop** — Python/Go/Bash consumers don't need a
   Rust toolchain.
3. **Reproducibility verification** — for the audit case ("show me
   bootrec produces the same bytes ms-sys does"), an auditor can run
   `bootrec` and `ms-sys` side by side without setting up a Rust dev
   environment.
4. **Oracle-of-our-own-binary tests** — the test harness can use the
   CLI binary as the black-box subprocess, exactly like it uses
   ms-sys. This catches regressions in the public API surface where
   internal-library unit tests might pass but the public CLI
   contract has drifted.

### Crate layout

Same `bootrec/` workspace member from the earlier layout section, with
`Cargo.toml` declaring both targets:

```toml
[package]
name = "bootrec"
version = "1.0.0"
edition = "2021"
license = "MIT"

[lib]
name = "bootrec"
path = "src/lib.rs"

[[bin]]
name = "bootrec"
path = "src/bin/bootrec.rs"

[dependencies]
clap = { version = "4", features = ["derive"] }   # binary only; cargo
                                                   # will skip when used
                                                   # as a library dep
```

The library is the public interface that's stable across versions; the
binary's CLI is allowed to evolve more freely (deprecations announced
in release notes). usbwin always tracks the library version, not the
binary version.

### Naming for symmetry with ms-sys

Where it's obvious, the CLI flag names mirror ms-sys's so the muscle
memory transfers:

| ms-sys flag      | bootrec flag          | Library function             |
|------------------|------------------------|------------------------------|
| `--mbr7`         | `--mbr-win7`           | `mbr_win7(...)`              |
| `--mbr`          | `--mbr-xp`             | `mbr_xp(...)`                |
| `--fat32pe`      | `--fat32-bootmgr`      | `fat32_pbr_bootmgr(...)`     |
| `--fat32nt`      | `--fat32-ntldr`        | `fat32_pbr_ntldr(...)`       |
| `--ntfs`         | `--ntfs-bootmgr`       | `ntfs_pbr_bootmgr(...)`      |

bootrec's flags are slightly more verbose (-pe vs -bootmgr) because the
ms-sys names are domain-jargon-y; new users shouldn't have to know that
"PE" means "Preinstall Environment" to write a Win 7 boot record. The
old names are accepted as aliases for muscle memory.

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

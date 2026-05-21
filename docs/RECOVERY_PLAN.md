# XP install recovery plan

Living document. Started 2026-05-20 PM after a research pass concluded that
the previous-session diagnoses for the three open bugs (A: partitioner UI
skip, B: USB clobbered as install target, C: CD-prompt loop) were partly
wrong and the fixes attempted during the session were inert. A second
research pass on FAT32-dedupe approaches surfaced a fundamentally different
architectural option (GRUB4DOS + FiraDisk) which is now the primary plan.

Update this doc at the **start of every iteration** (re-snapshot current
state) and at the **end of every iteration** (write what the action
revealed, reorder the queue if needed).

Companion docs (read alongside):
- `docs/XP_REGRESSION_2026_05_20.md` — original session log + symptoms
- `docs/TECH_DEBT.md` — open kludges, several now obsoleted by this plan
- `docs/V0.3_WINDOWS_XP.md` — overall XP-USB recipe
- `docs/XP_FIRADISK_PIPELINE.md` — concrete GRUB4DOS/FiraDisk pipeline
  spec for the current pivot
- `/Users/joa/code/mkmsbr/docs/XP_INT13_DRIVE_SWAP_SPEC.md` — Bug B spec
  (subsumed into Iter 3 below; GRUB4DOS chain in this plan covers it)

## 1. Status snapshot — 2026-05-21 (production pipeline reached GUI-mode)

| Bug | Symptom | Status |
| --- | --- | --- |
| **A** | Setup welcome screen → Enter → **no partitioner UI shown** | ✅ fixed on Dell E6410 by GRUB4DOS+FiraDisk hand prototype; production `windows-ntxp` reached GUI-mode install |
| **B** | XP setup writes its own MBR/PBR/`\WINDOWS\` onto the USB | ✅ fixed on Dell E6410 by GRUB4DOS drive swap hand prototype; production `windows-ntxp` reached GUI-mode install |
| **C** | GUI-mode loops on "Insert the CD…" prompt | ✅ fixed on Dell E6410 by rebooting through GRUB4DOS continuation entry in the hand prototype; production `windows-ntxp` reached GUI-mode install; legacy `windows-xp-legacy` still fails |

Last green XP hardware install: hand-staged GRUB4DOS+FiraDisk prototype
(2026-05-20 PM), Dell E6410. Production `usbwin --type=windows-ntxp`
implementation reached GUI-mode `Installing Windows` with the driver loaded
on 2026-05-21; final desktop boot still pending.

## 2. Consolidated findings

Numbered statements so future iterations can cite them as `[F-n]`.

### Bug A — partitioner UI skip

- **[F-1]** The skip predicate is `IsUnattendedSetup && AutoPartition!=0`.
  The actual mode-flip key is **`[Data] UnattendedInstall=Yes`**, not the
  mere presence of an `[Unattended]` section.
- **[F-2]** `AutoPartition` **must live in `[Data]`**, not `[Unattended]`.
- **[F-3]** **`\winnt.sif` at the partition root is the RIS/floppy
  convention and is likely NOT READ on a bootable-CD-style chain.** The
  textmode source directory is what setupldr/setupdd consume. (Note: under
  the FiraDisk approach the source IS a CD, so `\I386\winnt.sif` inside the
  ISO is the correct location.)
- **[F-4]** `MsDosInitiated="1"` is a source-path hint, not a destination-
  selection hint. It does not skip the partitioner.
- **[F-5]** `unused=unused` is inert — INI parsers drop unknown keys.

### Bug B — USB clobbered as install target

- **[F-6]** No `DestinationDiskNumber` key exists. No pure-config fix.
- **[F-7]** Setup picks the target by walking NT's `\Device\HarddiskN`
  list, which disk.sys builds from BIOS INT 13h enumeration at PnP time —
  *before* any SIF parser runs.
- **[F-8]** The canonical fix is GRUB4DOS `map (hd0) (hd1) ; map (hd1)
  (hd0) ; map --hook` before chainloading.
- **[F-9]** Stopgap: physically disconnect the internal HDD before booting
  from USB. Side-steps but doesn't fix.
- **[F-10]** Even with Bug A fixed, partitioner default highlight is
  Harddisk0=USB; one accidental Enter still wipes the USB.

### Bug C — CD-prompt loop

- **[F-11]** setupdd's source path is hardcoded to `\$WIN_NT$.~LS\I386\`.
  `SetupSourcePath` in `txtsetup.sif` is ignored once running off HD media.
- **[F-12]** Text-mode copies `\$WIN_NT$.~LS\I386\` from the source to
  `C:\$WIN_NT$.~LS\I386\`. Unconditional. `OemPreinstall=Yes` is NOT the
  trigger (and is harmful without an `$OEM$` tree).
- **[F-13]** GUI-mode prefers `C:\$WIN_NT$.~LS\I386\`; if missing, falls
  back to walking drive letters for `\I386\layout.inf` as the primary
  sentinel.
- **[F-14]** Net consequence under our previous design: `\I386\` at USB
  root was load-bearing for the drive-walk fallback (because
  `ren_fold.cmd` rename made the local copy unreachable). Commit `8f68b44`
  broke this.

### Dedupe — how others solve the three-tree problem

- **[F-15]** FAT32 has no symlinks/hardlinks (no inode indirection). NTFS
  on the USB would solve dedupe but XP setupldr can't read NTFS. Confirmed
  dead end.
- **[F-16]** Hex-patching setupldr.bin AND setupdd.sys to read from one
  path has been attempted for 15+ years; no end-to-end-confirmed working
  patch pair has been published. Our own attempts BSOD'd at smss-init
  (TECH_DEBT.md). Forum lore only.
- **[F-17]** **GRUB4DOS `map --mem /XP.ISO (0xff) ; map --hook`** loads
  the unmodified XP ISO into RAM and exposes it via INT 13h as a virtual
  El Torito CD. setupldr.bin reads `\I386\` from the "CD" and the entire
  install proceeds as if booted from a real XP CD-ROM.
- **[F-18]** The real-mode → protected-mode handoff requires a virtual-
  disk driver (**FiraDisk** or WinVBlock) loaded via `txtsetup.oem` from a
  virtual floppy image. Without this, BSOD 0x7B at setupdd handoff. FiraDisk
  re-acquires the RAM-resident ISO at protected-mode entry so setupdd keeps
  reading the same "CD".
- **[F-19]** This is the WinSetupFromUSB / RMPrepUSB / Easy2Boot consensus
  approach, empirically confirmed across 15+ years on diverse hardware.
- **[F-20]** Footprint under FiraDisk: ~700 MB (ISO ~600 MB + grldr + 2
  small floppy images + menu.lst). Vs. our current ~1.74 GB. Eliminates
  `$WIN_NT$.~BT\` mirror, `$WIN_NT$.~LS\I386\`, `\I386\` at root, and the
  entire rename hook chain (`ren_fold.cmd`, `undoren.cmd`). Bug C is
  expected to disappear because the "CD" is always present.

## 3. Dependency graph (revised, post-FiraDisk finding)

```
Under the FiraDisk pivot:

  GRUB4DOS text-mode setup entry:
   ├─ map (hd0) (hd1) ; map (hd1) (hd0)   ←─ resolves Bug B [F-7,F-8]
   ├─ map --mem (fd) /FIRADISK.IMA (fd0/fd1)
   ├─ map --mem (cd) /XP.ISO (0xff)        ←─ resolves Bug C [F-17,F-20]
   └─ chainload (0xff)/I386/setupldr.bin   ←─ standard XP boot

  GRUB4DOS GUI-mode continuation entry:
   ├─ repeats the same drive-swap + FiraDisk + ISO maps
   └─ chainload (hd0)+1 after the swap      ←─ boots target HDD with CD present

  XP setup itself:
   ├─ reads \I386\winnt.sif on the virtual CD ─ Bug A is now a non-issue
   │  (winnt.sif lives inside the ISO at the standard location [F-3])
   └─ FiraDisk loaded via txtsetup.oem keeps source available post-handoff
```

The three bugs **collapse into a single architectural change**: replace
the build-an-XP-tree-on-FAT32 pipeline with a carry-the-ISO-and-emulate-
the-CD pipeline. Bugs A/B/C all become solved as side-effects.

## 4. Action queue (post-FiraDisk pivot)

Ordered by signal-per-effort, lowest cost and highest information first.

### ✅ Iteration 1: design spec for `--xp-mode=grub4dos-firadisk`

- **Hypothesis:** The FiraDisk approach is feasible to integrate into
  usbwin in roughly the same scope as the existing three-tree pipeline.
- **Actions:** Write `docs/XP_FIRADISK_PIPELINE.md` covering:
  - File set on USB (grldr, menu.lst, XP.ISO, firadisk.ima, optionally
    winvblock.ima)
  - MBR/PBR setup (mkmsbr work — likely just `mkmsbr --type=grub4dos`)
  - `menu.lst` template (drive-swap + map-iso + chainload)
  - How winnt.sif gets into the ISO (do we modify the ISO in-place at
    burn time, or carry a separate `\winnt.sif` outside the ISO that
    GRUB4DOS overlays via `--rd-base=` / similar?)
  - FiraDisk virtual floppy layout and `txtsetup.oem` content
  - Pipeline contract: input = ISO + USB, output = bootable USB
- **Expected signal:** confidence that the integration is bounded work,
  and a punch list of open sub-questions before any code.
- **Result:** `docs/XP_FIRADISK_PIPELINE.md` created. Research-agent
  challenge pass confirmed the pivot and tightened the mechanics:
  two GRUB4DOS entries are needed (text-mode and GUI-mode continuation),
  `FIRADISK.IMA` should be mapped to both `(fd0)` and `(fd1)`, and FiraDisk
  should use `map --mem /XP.ISO (0xff)` as the default. WinVBlock remains
  a fallback, especially for any non-RAM ISO experiments.
- **Cost:** ~1 hour of writing.
- **Safety:** Documentation only; no code or hardware action.

### ✅ Iteration 2: prototype in QEMU/hardware

- **Hypothesis:** A hand-assembled USB image (manually staged files,
  bypassing the usbwin pipeline) will boot a clean XP install in QEMU
  through to GUI-mode source-found.
- **Actions:**
  1. Hand-stage a USB image: grub4dos PBR, two-entry `menu.lst` with
     drive-swap + `--mem` ISO map in both entries, XP ISO, FiraDisk floppy
     image mapped to `(fd0)` and `(fd1)`, `txtsetup.oem` in the floppy.
  2. Run via `usbwin-eval` with two QEMU disks (USB image + blank HDD).
  3. Observe: does setupldr boot? does partitioner UI render? does
     text-mode complete? does GUI-mode pick up the source?
- **Expected signal:** binary go/no-go on the architectural pivot. If
  go: implement in usbwin (Iter 3). If no-go: investigate FiraDisk
  fallback (WinVBlock) or fall back to fallback track in §5.
- **Partial result (2026-05-20 17:36 EDT):** hand prototype reaches real
  Windows Setup in QEMU, but stops at `STOP 0x0000000A` shortly after the
  ASR prompt. Direct `SETUPLDR.BIN` chainload and ISO boot-image chainload
  fail identically. GRUB4DOS 0.4.4 `grldr.mbr` could not find `GRLDR`;
  chenall GRUB4DOS 0.4.6a 2020-08-09 boots correctly from the hand-written
  MBR/boot-track install. A no-drive-swap isolation run at 1024 MiB fails
  in GRUB4DOS with Error 28; at 2048 MiB it was still RAM-loading when the
  300s eval timed out, so the swap-vs-memory question remains open.
- **Hardware result (2026-05-20 PM):** Dell E6410 end-to-end install green
  with the hand-staged GRUB4DOS+FiraDisk image. Text-mode setup, manual
  partitioner, GUI-mode continuation, and final boot all worked.
- **Cost:** ~2–4 hours of manual staging + QEMU debug.
- **Safety:** QEMU first, then one controlled Dell burn.

### ✅ Iteration 3: implement the new pipeline in usbwin

- **Hypothesis:** A new `xp_mode=grub4dos-firadisk` build path produces an
  identical USB to the hand-staged Iter 2 prototype.
- **Actions:** Add the new mode as `--type=windows-ntxp`. Keep the old
  three-tree path available as `--type=windows-xp-legacy`. `--type=auto`
  and the compatibility alias `--type=windows-xp` should resolve to
  `windows-ntxp`.
- **Expected signal:** `usbwin` produces a USB byte-identical (or
  functionally equivalent) to Iter 2's hand-staged image. QEMU eval green.
- **Result:** Implemented locally. `--type=windows-ntxp` stages the
  production GRUB4DOS + FiraDisk layout (`GRLDR`, `menu.lst`, `XP.ISO`,
  `FIRADISK.IMA`) using embedded chenall GRUB4DOS 0.4.6a assets and the
  tested FiraDisk image. `--type=windows-xp` is now a compatibility alias
  for the new path; the old three-tree implementation is available as
  `--type=windows-xp-legacy`; XP-class auto-detect resolves to
  `windows-ntxp`. Validation passed locally:
  `/opt/homebrew/bin/cargo test -p usbwin` (45 tests) and
  `/opt/homebrew/bin/cargo build --release -p usbwin`.
- **Cost:** ~half-day code + tests.
- **Safety:** QEMU first, then Dell burn once byte-equivalent to prototype.

### → Iteration 4: production-pipeline hardware burn + verify on Dell E6410

- **Hypothesis:** The production `usbwin` implementation can reproduce the
  hand-staged hardware-green prototype.
- **Actions:** Burn USB via new pipeline, install on E6410, take it
  end-to-end to working desktop. Then post-mortem: dump USB MBR/PBR and
  staged files — these should be unchanged from what usbwin wrote.
- **Expected signal:** full hardware confirmation, or an identifiable
  hardware-specific failure to debug.
- **Current command under test:**
  `sudo ./target/release/usbwin ~/Downloads/en_windows_xp_professional_with_service_pack_3_x86_cd_vl_x14-73974.iso /dev/rdisk6 --type=windows-ntxp`
- **Current result:** reached GUI-mode `Installing Windows` with the driver
  loaded on 2026-05-21. Treat this as a strong production-path pass signal;
  reserve final green status until first desktop boot and post-test USB
  sanity check.
- **Cost:** Hardware time, ~30 min if it works on first try.
- **Safety:** USB write-protect or HDD-disconnect for first attempt, given
  history.

### → Iteration 5: decommission the three-tree pipeline

- **Hypothesis:** Once FiraDisk path is hardware-green, the three-tree
  path is pure tech debt.
- **Actions:** Delete (or move behind `--legacy-three-tree`) the rename
  hook code (`ren_fold.cmd`/`undoren.cmd`), `replicate_i386_to_bt`,
  `stage_ls_from_bt`, the WIPE entry hack, SIF-modifier assertions,
  three-copy TXTSETUP.SIF dance. Update `HARDWARE_TESTS.md`,
  `TECH_DEBT.md`, `V0.3_WINDOWS_XP.md`.
- **Expected signal:** workspace tests still green, code surface dropped
  by ~40-50%.
- **Cost:** ~1-2 hours.
- **Safety:** Pure code cleanup.

### → Iteration 6: unattended support for the FiraDisk ISO path

- **Hypothesis:** Once the GRUB4DOS+FiraDisk path is green, injecting a
  minimal `I386\WINNT.SIF` into a derived XP ISO can remove the inopportune
  GUI-mode prompts and expected unsigned-driver prompts without
  reintroducing the legacy root-level `winnt.sif` ambiguity.
- **Actions:** Add optional unattended settings for the FiraDisk path:
  product key, regional/timezone defaults, computer name, admin password
  policy, EULA acceptance, install mode, and driver-signing policy for the
  FiraDisk textmode driver. Generate a derived ISO with `I386\WINNT.SIF`
  injected; keep the input ISO immutable.
- **Expected signal:** GUI-mode setup proceeds without stopping for the
  usual interactive prompts or the expected unsigned FiraDisk warning,
  while the partitioner remains manual unless the user explicitly opts
  into full unattended partitioning.
- **Safety:** Default remains attended/manual partitioning. Never set
  `AutoPartition=1` unless the user explicitly asks for destructive full
  automation.

### → Iteration 7: AHCI/SATA/RAID textmode storage support

- **Hypothesis:** The `windows-ntxp` path can support XP installs with the
  BIOS SATA controller left in AHCI mode by adding a DPMS-style F6 mass-
  storage driver floppy alongside the FiraDisk RAM-ISO floppy.
- **Why:** XP SP3 does not include broad inbox AHCI support. WinSetupFromUSB
  and Easy2Boot solve this by detecting the mass-storage controller PCI ID,
  selecting a matching XP textmode driver, and presenting it to Setup via a
  virtual floppy (`txtsetup.oem`).
- **Actions:**
  1. Start narrow: Dell E6410 Intel SATA AHCI controller only.
  2. Add an explicit `--xp-storage-driver=<f6-floppy-or-dir>` or
     `--xp-ahci=dell-e6410` experimental flag.
  3. Generate/map a second virtual floppy containing `txtsetup.oem` plus
     the selected AHCI driver files.
  4. Ensure text-mode setup loads both required classes: FiraDisk/WinVBlock
     for the RAM ISO, and the AHCI/SATA driver for the internal disk.
  5. Later generalize to a DriverPack MassStorage/DPMS-style PCI-ID matcher.
- **Expected signal:** With BIOS SATA set to AHCI on the Dell E6410, XP
  text-mode setup sees the internal HDD and completes through GUI-mode.
- **Risks:** The automatic path may be limited by XP's practical F6-floppy
  constraints: only a small number of default drivers can be selected
  noninteractively, and multiple matching storage drivers may require a
  manual selection step.

### → Iteration 8: WinVBlock fallback and low-RAM modes

- **Hypothesis:** FiraDisk + RAM ISO should remain the default, but a
  WinVBlock fallback and a documented low-RAM mode will improve compatibility
  on machines where the RAM ISO path fails or where RAM is tight.
- **Actions:**
  1. Add a debug/experimental switch for FiraDisk-only, WinVBlock-only, and
     FiraDisk+WinVBlock floppy images.
  2. Add RAM requirement checks/warnings before burning. A full XP SP3 ISO
     mapped with `--mem` is not realistic on every 512 MB-era machine.
  3. Consider an optional "reduced XP ISO" helper that preserves the install
     path while trimming nonessential folders for low-RAM hardware.
  4. Keep the default path simple unless hardware evidence says otherwise.
- **Expected signal:** We have a controlled fallback for 0x7B/0xA/late GUI-
  mode failures without reviving the legacy three-tree pipeline.

### → Iteration 9: XP troubleshooting and hardware-workaround menu

- **Hypothesis:** Many XP USB failures are diagnosable if the boot menu can
  expose the same data that mature tools expose: detected PCI IDs, selected
  mass-storage driver, RAM-map mode, and known BIOS workarounds.
- **Actions:**
  1. Add a GRUB4DOS diagnostics menu entry to list mass-storage PCI IDs
     once DPMS-style support exists.
  2. Add optional menu variants for GRUB4DOS `map --e820cycles=...` memory-
     map workarounds if we hit "Setup is starting Windows" blank-screen
     hangs on specific BIOSes.
  3. Research and, if needed, support a custom `NTDETECT.COM` injection path
     for Dell-style USB/0x7B failures.
  4. Add a post-burn integrity command or checklist: verify staged ISO hash,
     file sizes, boot track readback, and expected root layout.
- **Expected signal:** Failed field tests produce enough evidence to choose
  the next experiment instead of blindly reburning.

### → Iteration 10: multiboot and ISO-library ergonomics

- **Hypothesis:** Once XP is green, usbwin can borrow a limited subset of
  Easy2Boot/WinSetupFromUSB ergonomics without becoming a general multiboot
  kitchen sink.
- **Actions:**
  1. Support multiple Windows sources on one USB only after the single-source
     burn path is stable.
  2. Generate deterministic GRUB4DOS menus for multiple XP/2003 sources.
  3. Keep Windows 7+ on BOOTMGR rather than forcing it through GRUB4DOS.
  4. Avoid dynamic menu generation at boot unless we actually need it.
- **Expected signal:** Common field workflow improves, but the core burn
  pipeline stays inspectable and testable.

## 5. Fallback track — if FiraDisk doesn't work out

If Iter 2 or Iter 4 fails for hardware-specific or driver-compat reasons,
we revert to debugging the three-tree pipeline. The original action queue
(pre-pivot) remains valid as a fallback:

- **Fallback-1:** Fix `winnt.sif` location and section semantics
  (`\$WIN_NT$.~BT\winnt.sif`; `AutoPartition=0` in `[Data]`; drop
  `unused=unused`).
- **Fallback-2:** Validate in QEMU with two disks.
- **Fallback-3:** Hardware burn + dual forensic dump.
- **Fallback-4:** GRUB4DOS INT 13h swap (Bug B) — needed regardless.
- **Fallback-5:** Diagnose Bug C on hardware (inspect
  `C:\$WIN_NT$.~LS\I386\` after a failed install).

These are documented in detail in the v0 of this plan (see git log of
this file). Don't re-derive them from scratch if we end up here.

## 6. Iteration protocol

At the start of every iteration:
1. **Re-read the Status snapshot table (§1)**. Update if state changed.
2. **Skim §2 findings.** Strikethrough any [F-n] invalidated by new
   evidence — don't delete; we want the audit trail.
3. **Check the Action queue (§4).** Top item = current play. Re-evaluate
   ordering against new evidence.
4. **Execute.**
5. **End-of-iteration:** update §1 with what changed; append a dated
   "Iteration N notes" entry recording what happened, what was learned,
   and what the new top of the queue is.

## 7. Things NOT to do

(Imported from `XP_REGRESSION_2026_05_20.md` "Things to NOT do" + new
constraints learned in this research pass.)

- **Don't try the rename-not-replicate refactor again.** Commit `8f68b44`
  broke GUI-mode setup's drive-walking; [F-13] explains why. (Moot under
  FiraDisk path, but stay reverted in legacy path.)
- **Don't byte-patch setupldr.bin alone.** [F-16] — multiple decade-old
  attempts BSOD'd because setupdd.sys also reads the paths verbatim. A
  working dual-patch has not been publicly demonstrated.
- **Don't drop the `[SetupParams] UserExecute` hook** in the legacy
  pipeline without a replacement plan. Under FiraDisk path, the entire
  hook chain disappears.
- **Don't add `UnattendedInstall=Yes` to `[Data]`** unless deliberately
  going fully unattended. [F-1] — it's the actual mode trigger; setting
  it skips the partitioner.
- **Don't put `AutoPartition` in `[Unattended]`.** [F-2] — `[Data]` key;
  inert anywhere else.
- **Don't add `OemPreinstall=Yes`** without an actual `$OEM$` tree.
  [F-12] — harmful otherwise.
- **Don't switch the USB filesystem to NTFS** to enable symlinks. [F-15]
  — setupldr can't read NTFS at boot; dead end.

## 8. Iteration notes

(Append new dated entries below as iterations complete.)

### 2026-05-20 PM — plan v0 created

Six external research agents + one internal audit completed. Findings
recorded as [F-1]..[F-14]. Original action queue spec'd around fixing the
existing three-tree pipeline incrementally.

### 2026-05-20 PM — plan v1 created (FiraDisk pivot)

One additional research agent on FAT32-dedupe surfaced the
GRUB4DOS+FiraDisk approach as the established community-canonical method.
Recorded as [F-15]..[F-20]. Action queue rewritten to spec the pipeline
pivot, with the original three-tree fixes preserved as §5 fallback.

Next: Iteration 1 (write `docs/XP_FIRADISK_PIPELINE.md`).

### 2026-05-20 PM — Iteration 1 completed

Created `docs/XP_FIRADISK_PIPELINE.md`. Two challenge/research passes found
no better path than GRUB4DOS + RAM ISO + FiraDisk, but did correct the
implementation sketch:

- Use a two-entry GRUB4DOS flow. Entry 1 starts text-mode setup from the
  RAM ISO. Entry 2 repeats the maps and chainloads the internal HDD so
  GUI-mode setup still sees the virtual CD.
- Map the FiraDisk floppy image to both `(fd0)` and `(fd1)`.
- Treat `map --mem /XP.ISO (0xff)` as mandatory for the primary FiraDisk
  path. Non-RAM mapping belongs to a WinVBlock fallback experiment.
- Keep WinVBlock as fallback, not a peer default. Prior art reports it as
  faster sometimes, but less reliable across QEMU and some chipsets.

Next: Iteration 2 (hand-stage a QEMU-only prototype).

### 2026-05-20 17:36 EDT — Iteration 2 started, first QEMU signal

Hand-staged `/private/tmp/usbwin-xp-proto/xp-firadisk-usb.img` with:

- XP ISO: `/Users/joa/Downloads/en_windows_xp_professional_with_service_pack_3_x86_cd_vl_x14-73974.iso`
- FiraDisk package: DriverPack Karyonix WinAll (`txtsetup.oem`,
  `firadisk.sys`, `firadisk.inf`, `firadisk.cat`) staged into
  `/private/tmp/usbwin-xp-proto/firadisk.ima`
- GRUB4DOS: chenall 0.4.6a 2020-08-09 (`grldr.mbr` + `GRLDR`)

Findings:

- SourceForge GRUB4DOS 0.4.4 `grldr.mbr` repeatedly reached its partition
  scan but failed with `Cannot find GRLDR`, even on a minimal FAT16 test
  image. Do not use it for this prototype.
- chenall GRUB4DOS 0.4.6a boots correctly when `grldr.mbr` is written to
  the MBR/boot track and the active partition entry is restored.
- The prototype reaches real XP text-mode setup (`Windows Setup` /
  `Press F2 to run Automated System Recovery`) with both direct
  `(0xff)/I386/SETUPLDR.BIN` and `chainloader (0xff)`.
- Both chainload forms then fail with `STOP 0x0000000A
  (0x00000016, 0x00000002, 0x00000000, 0x80812DE1)`.
- A no-drive-swap variant did not yet give a clean comparison: 1024 MiB
  hits GRUB4DOS Error 28 (`Selected item cannot fit into memory`), while
  2048 MiB was still mapping the ISO at the 300s timeout.
- `usbwin-eval` had an overly broad XP pass marker (`windows xp`) that
  false-passed on a GRUB menu title. Removed that marker in code.

Next: continue Iteration 2 by isolating the `STOP 0xA`: first run a longer
2048 MiB no-swap eval, then test FiraDisk mapped only as `(fd0)`, then test
without FiraDisk to distinguish GRUB4DOS mapping/drive-swap issues from the
driver floppy.

### 2026-05-20 PM — Dell hardware report: CD prompt still reproduced on legacy path

User report from Dell E6410: setup again reached
`Insert the CD labeled: Windows XP Professional Service Pack 3 CD into your
CD-ROM drive. Press ENTER when ready.`

Command used:

```text
cargo build --release && sudo time ./target/release/usbwin \
  ~/Downloads/en_windows_xp_professional_with_service_pack_3_x86_cd_vl_x14-73974.iso \
  /dev/rdisk6 --type=windows-xp
```

Interpretation:

- This is the original Bug C symptom: GUI-mode setup cannot see a valid
  source path / CD source.
- The command used `--type=windows-xp`, which is the legacy three-tree
  pipeline. It is not the GRUB4DOS+FiraDisk prototype.
- Therefore this hardware result confirms Bug C remains open in the
  legacy path, but it does not invalidate the FiraDisk pivot.
- The FiraDisk prototype remains QEMU-only for now; the current QEMU
  prototype has not reached GUI-mode source-found and stops earlier at
  `STOP 0x0000000A`.

Immediate next diagnostic before more hardware burns:

1. Continue QEMU Iteration 2 until a prototype reaches GUI-mode
   source-found; only then burn hardware again.
2. If we need another legacy-path burn, do it only after targeted Bug C
   fixes; otherwise it is expected to reproduce the prompt.

### 2026-05-20 PM — Dell hardware green through GUI-mode start

User tested the hand-staged GRUB4DOS+FiraDisk prototype on the Dell:

- GRUB4DOS bootloader works.
- Entry 1 reaches text-mode setup and file copy.
- The XP partitioner UI appears.
- After reboot, entry 2 remaps the RAM ISO and chainloads the HDD.
- GUI-mode reaches `Installing Windows`, with setup estimating
  approximately 39 minutes remaining.

This is the first hardware confirmation that the FiraDisk path fixes Bugs
A, B, and C on the Dell. Final forensic check still pending: confirm the
USB did not receive `\WINDOWS` and its boot records/files remain as staged.

Usability note: GUI-mode setup still prompts interactively at bad times.
Add unattended support for the FiraDisk path after the boot/install flow is
green. Keep partitioning manual by default. The FiraDisk/USB RAM disk
driver also triggers an unsigned-driver security warning; the unattended
answer-file work should include a deliberate driver-signing policy so this
does not block unattended GUI-mode setup.

### 2026-05-20 PM — Dell hardware install completed

The hand-staged GRUB4DOS+FiraDisk prototype completed the XP install on the
Dell E6410 and booted successfully.

Hardware verdict:

- Bug A fixed: partitioner UI appears.
- Bug B fixed in practice: install target was the internal HDD, not the
  SanDisk USB.
- Bug C fixed: GUI-mode setup reached `Installing Windows` and completed
  without the CD prompt when booted through GRUB4DOS entry 2.

Next top item: Iteration 3. Implement the prototype as a real usbwin mode
instead of `/private/tmp` hand staging.

### 2026-05-21 — Iteration 3 completed locally

Implemented the first-class production path:

- Added `--type=windows-ntxp` for the GRUB4DOS + FiraDisk pipeline.
- Kept the old three-tree implementation as `--type=windows-xp-legacy`.
- Kept `--type=windows-xp` as a compatibility alias to `windows-ntxp`.
- Changed XP-class auto-detect to resolve to `windows-ntxp`.
- Embedded the working chenall GRUB4DOS 0.4.6a `GRLDR`, `grldr.mbr`, and
  tested `FIRADISK.IMA` assets.
- Stages the proven layout: `GRLDR`, `menu.lst`, `XP.ISO`,
  `FIRADISK.IMA`.

Validation:

- `/opt/homebrew/bin/cargo test -p usbwin` passed: 45 tests.
- `/opt/homebrew/bin/cargo build --release -p usbwin` passed.

Next: Iteration 4. Production-pipeline hardware burn on the Dell E6410,
using:

```text
sudo ./target/release/usbwin \
  ~/Downloads/en_windows_xp_professional_with_service_pack_3_x86_cd_vl_x14-73974.iso \
  /dev/rdisk6 \
  --type=windows-ntxp
```

### 2026-05-21 — Iteration 4 reached GUI-mode install

User report from the first production `usbwin --type=windows-ntxp` burn:
the installer reached GUI-mode, the driver is loading, and Windows is
installing.

Interpretation:

- The production pipeline reproduced the core hand-staged prototype behavior.
- Bugs A/B/C are very likely closed for the production path.
- Keep final hardware status at "pending final desktop boot" until setup
  completes and the USB root/boot records are sanity-checked.

Next: checkpoint this state in git before deleting the legacy three-tree
pipeline.

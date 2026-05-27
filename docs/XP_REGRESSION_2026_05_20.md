# XP install regression — investigation log, 2026-05-20

Working session log for the XP-install-broken-end-to-end bug, captured
in case the next session picks up cold. Hardware: Dell E6410, BIOS,
SATA in ATA mode, SanDisk Extreme 64 GB on /dev/disk6. ISO: en_windows
_xp_professional_with_service_pack_3_x86_cd_vl_x14-73974.iso.

> **2026-05-21 update:** this log is historical. Active work now lives in
> [`BACKLOG.md`](BACKLOG.md). The recovery summary is archived in
> [`RECOVERY_PLAN.md`](RECOVERY_PLAN.md).

## State

- **2026-05-19 21:57** (75e16de): XP install marked ✅ verified
  end-to-end on the E6410. Three-tree layout (`\I386\` + `\$WIN_NT$.~BT\`
  + `\$WIN_NT$.~LS\I386\`). Recorded as such in `HARDWARE_TESTS.md`
  row 7.
- **2026-05-20 AM**: Four commits landed (`8f68b44` rename refactor,
  `679e41c` SIF assertions, `873d2d9` WIPE entry, `0f1abd4` eval bin).
- **2026-05-20 PM**: XP install fails end-to-end in two distinct
  ways. Investigation summarised here.

Net state at end of session: **XP install ❌ broken**. Fixes landed
during the session improved things but did not restore full
end-to-end. Take a break, come back, pick up from "Open questions"
section.

## Symptoms (in the order they surfaced)

### Symptom 1 — installer "clobbers its own USB"

Pre-session run:

- Boot USB, NTLDR menu shows three entries.
- Pick "1st, text mode setup". Setupldr runs, blue text-mode screens
  flick past, progress bar to 100%. No partitioner UI seen.
- Reboot.
- BIOS picks HDD: flashing `_` cursor only.
- F12 → USB boot: flashing `_` cursor only.

Forensic dump of `/dev/disk6` after the failure:

- **USB sector 0 (MBR)**: replaced with canonical Microsoft NT-era
  MBR (boot code starts `33 c0 8e d0 bc 00 7c`; strings "Invalid
  partition table", "Error loading operating system", "Missing
  operating system"). Disk signature rewritten from our `de ad be ef`
  to `2c 44 63 ef`.
- **USB partition sector 0 (PBR)**: replaced with canonical MS NT 5.x
  FAT32 NTLDR-loading PBR (boot code starts `33 c9 8e d1 bc f4 7b`;
  OEM ID `MSWIN4.1`; `NTLDR` literal at 0x170; strings "NTLDR is
  missing", "Disk error", "Press any key to restart").
- **`/Volumes/WINXP/boot.ini`**: completely rewritten. Default now
  `multi(0)disk(0)rdisk(0)partition(1)\WINDOWS` (= USB at boot
  time). Three of our entries still appended at the end.
  Timestamp `Aug 25 2011` (XP setup's internal build date for files
  it writes).
- **USB file listing**: `\WINDOWS\` directory created with timestamp
  `Aug 25 2011`. `bootsect.dos` file created with the same timestamp
  (XP setup writes this when it thinks it's preserving a previous
  DOS install).

Conclusion: XP setup ran, picked the USB as both **install target**
AND **system disk**, started installing Windows ONTO the USB,
rewriting its bootstrap and creating `\WINDOWS\` on it. The HDD got
nothing usable.

### Symptom 2 — CD prompt loop (surfaced after partial fixes)

After the winnt.sif and I386-revert fixes (below):

- Boot USB. NTLDR menu. Pick text-mode entry.
- Setupldr runs. Reaches the XP setup welcome screen ("To set up
  Windows XP now, press ENTER").
- Press Enter.
- **Partitioner UI still does not appear.**
- Setup prompts: "Insert the CD labeled Windows XP Professional
  Service Pack 3 CD into your CD-ROM drive — Press ENTER when
  ready." Pressing Enter loops the prompt.

So we made progress (got to the welcome screen, no longer silently
installs to USB) but Bug A (partitioner skip) still fires and a CD
prompt remains.

## Research findings

Two research agents dispatched during the session.

### Agent 1: how does XP pick the system disk?

> setupdd.sys picks the system disk (where MBR/PBR/NTLDR/boot.ini
> get written) as the disk with the lowest NT `\Device\HarddiskN`
> number — which in turn is assigned by the disk class driver in
> the order the BIOS reports drives via INT 13h fn 0x08, starting
> at 0x80.

On the E6410, BIOS enumerates USB at 0x80 and SATA HDD at 0x81. So
`\Device\Harddisk0` = USB → setupdd picks USB as system disk. This
is the canonical "WinSetupFromUSB drive-swap" problem; GRUB4DOS
solves it with `map (hd0) (hd1) ; map (hd1) (hd0) ; map --hook`.

Sources cited: ReactOS `base/setup/usetup/usetup.c` (clean-room
equivalent), Pete Batard's Rufus MBR notes (`pete.akeo.ie/2012/04/
crafting-mbr-from-scratch.html`), MSFN topic 119742, reboot.pro
topic 471.

### Agent 2: why is the partitioner UI being skipped?

Less conclusive. Primary findings:

- `MsDosInitiated="1"` does NOT skip the partitioner per KB Q123765.
- `AutoPartition=1` skips it; default (key absent) is 0 (prompt).
- The actual skip predicate (ReactOS-derived): UI bypasses only when
  `IsUnattendedSetup=TRUE` AND `DestinationDiskNumber` /
  `DestinationPartitionNumber` are both valid.
- **Empty `[Unattended]` section behavior is undocumented.** Strong
  circumstantial evidence: ruo91's canonical USB_MultiBoot template
  (`winnt_rec.sif`) uses `unused=unused` placeholder rather than
  leaving the section empty. The choice is deliberate.

Recommended fix (which we tried — see below): replace empty
`[Unattended]` with `unused=unused`, add explicit `AutoPartition=0`,
drop the `\I386\winnt.sif` duplicate (consumed only by winnt32.exe,
not text-mode setup).

## Fixes applied during the session

### Fix 1: winnt.sif tweaks

`pipeline/windows_xp_unattended.rs:generate_minimal()`:

- Added `AutoPartition=0` to `[Data]` (Microsoft default, set
  explicitly as belt-and-braces).
- Replaced empty `[Unattended]` section with the ruo91 placeholder
  `unused=unused`.

`pipeline/windows_xp.rs:write_unattended()`:

- Dropped the `\I386\winnt.sif` duplicate write. `\winnt.sif` at
  partition root remains (the only one text-mode actually reads).

### Fix 2: revert 8f68b44's rename refactor

Diagnosis: 8f68b44 replaced `replicate_i386_to_bt` (ditto) with
`move_i386_to_bt` (rename). After the rename, `\I386\` no longer
existed at the USB root. GUI-mode XP setup walks drive letters
looking for `\I386\setupreg.hiv` / `\I386\layout.inf` when its
primary source path (`\$WIN_NT$.~LS\I386\`) is unavailable — which
it always is post-text-mode because `ren_fold.cmd` renames `~LS`
to `WIN_NT.LS`. With both paths gone, GUI-mode dropped into the
CD-prompt loop.

Reverted:

- `pipeline/xp_staging.rs`: deleted `move_i386_to_bt`, restored
  `replicate_i386_to_bt` (ditto). Comment captures the failure
  story so future-us doesn't try this again without instrumenting
  setupdd first.
- `pipeline/windows_xp.rs`: pipeline call site updated.
- `docs/TECH_DEBT.md`: ✅-resolved entry flipped to ❌-reverted
  with the CD-prompt-loop explanation.

Trade-off: burn time ~170s (was 140s after the rename), USB size
~1.74 GB (was 1.17 GB). Worth it for a working install IF the
revert actually fixes the CD prompt.

### Fix 3: docs

- Created `/Users/joa/code/mkmsbr/docs/XP_INT13_DRIVE_SWAP_SPEC.md`
  — spec for the GRUB4DOS-style INT 13h drive-swap that mkmsbr
  would need to ship to fix Symptom 1 properly. Notes that the
  partitioner-skip is upstream and must be fixed first.

All workspace tests still green (43/43).

## Post-fix state

Re-burn after Fixes 1 + 2:

- ✅ Burn completed (140s pre-revert, ~170s post-revert presumed).
- ❌ Partitioner UI still does not appear.
- ❌ CD prompt loop still fires.
- ✅ Setup at least gets to the welcome screen (didn't silently
  install to USB this time — possibly because [Unattended] fix
  changed something, possibly because the USB hadn't been
  pre-wiped and the HDD was in a different state. Unclear.)

So both Bug A (partitioner skip) and Bug C (CD prompt) survived the
fixes. Bug B (drive-swap) wasn't directly attacked yet but its
effects may be masked by A and C.

## Open questions for next session

1. **Why is the partitioner UI still being skipped after the
   `unused=unused` fix?** ruo91's template was supposed to be the
   working pattern. Possibilities:
   - Some OTHER key in our minimal winnt.sif is triggering
     unattended mode (e.g. `Floppyless="1"`? `MsDosInitiated="1"`
     interacts with `[Unattended]` non-empty? `[SetupParams]
     UserExecute` makes setup think it's unattended?).
   - The empty-section theory was wrong; the real cause is something
     we haven't identified.
   - winnt.sif at the partition root isn't being read at all;
     setupdd is reading some other file or none.

   Diagnostic: try staging an even more minimal winnt.sif (just
   `[Data] MsDosInitiated="1"`, nothing else — no [Unattended]
   section at all, no [SetupParams], no [GuiRunOnce]). If the
   partitioner appears, bisect from there.

2. **Why does the CD prompt still fire after restoring `\I386\` at
   root?** Possibilities:
   - `\I386\` is restored but missing something specific
     (`setupreg.hiv`? Some `_x` file declared in TXTSETUP.SIF that
     ren_fold.cmd / undoren.cmd declarations point to a path that
     no longer exists?).
   - GUI-mode is looking at the HDD's `C:\$WIN_NT$.~LS\I386\`
     specifically (not falling back to USB), and text-mode setup
     didn't copy the I386 tree there. The 466be0a commit message
     asserted "text-mode setup copies the contents of this folder
     to the target HDD as `C:\$WIN_NT$.~LS\I386\`" but we never
     verified that claim empirically. Maybe it requires an
     `OemPreinstall=Yes` we don't set.
   - The `declare_ren_scripts` SIF mod (adds `100,,,,,,_x,2,0,0`
     entries for ren_fold.cmd / undoren.cmd) declares files at a
     `_x` destination that resolves to `\$WIN_NT$.~LS\I386\` —
     if those files are declared but the destination path on USB
     doesn't match, setupdd may abort copy → no GUI-mode source
     on HDD → CD prompt.

   Diagnostic: after a failed install, pull the USB and the HDD
   (if recoverable) and inspect what's actually on the HDD's C:
   partition. Specifically check for `C:\$WIN_NT$.~BT\`,
   `C:\$WIN_NT$.~LS\`, `C:\I386\`, `C:\WINDOWS\`. The shape of
   what's there tells us what text-mode setup actually copied.

3. **Is `MsDosInitiated="1"` doing what we think?** The comment
   in our code says "tells setup it was bootstrapped from MS-DOS /
   a custom loader" but KB Q123765 (cited by the research agent)
   suggests it specifically means "winnt.exe was invoked from
   DOS" — not "any non-CD bootstrap." If we're using it
   incorrectly, setup may be running in a mode that auto-picks
   defaults including AutoPartition.

   Diagnostic: try with `MsDosInitiated="0"` and see if behavior
   changes. (May break other things — this is the kind of test that
   needs a known-virgin HDD so failures are recoverable.)

4. **Is Bug B (system disk = first INT 13h drive = USB) still
   happening?** We didn't directly attack it this session. The
   ruo91/Rufus solution is an INT 13h swap hook in the bootstrap.
   The spec at `/Users/joa/code/mkmsbr/docs/XP_INT13_DRIVE_SWAP_
   SPEC.md` covers the mkmsbr-side work needed.

   Diagnostic for whether it's still firing: after the next failed
   install (whatever the symptom), dump the USB's MBR/PBR/boot.ini
   like we did this session. If they're rewritten by XP setup
   again, Bug B is firing and the INT 13h swap work needs to
   happen. If they're intact, Bug B may have been a downstream
   effect of Bug A (no partitioner → auto-pick first drive → USB)
   and fixing A makes B disappear.

## Things to NOT do next session

- **Don't try the rename refactor again** (8f68b44 reverted). It
  broke GUI-mode setup's drive-walking I386 lookup. Recovering the
  ~580 MB needs instrumentation of setupdd's source-discovery path
  first.
- **Don't byte-patch setupldr.bin** to redirect paths. Multiple
  attempts during the 2026-05-19/20 work (`I386` + 8 spaces and
  `$WIN_NT$.~LS` patches) all produced BSOD 0x6B / 0xC000003A at
  smss-init because setupdd.sys reads paths verbatim, not
  setupldr's patched paths. See TECH_DEBT.md for the full story.
- **Don't drop the [SetupParams] UserExecute hook** without a
  replacement plan. The hook is what runs `ren_fold.cmd` to keep
  GUI-mode's boot-volume sanity check happy. Removing it breaks
  the text→GUI transition in a different way.

## Code state at end-of-session

```
modified:   crates/bootsmith/Cargo.toml                                 (unrelated, eval bin)
modified:   crates/bootsmith/src/pipeline/windows_xp.rs                  (Fix 1, Fix 2)
modified:   crates/bootsmith/src/pipeline/windows_xp_unattended.rs       (Fix 1)
modified:   crates/bootsmith/src/pipeline/xp_staging.rs                  (Fix 2)
modified:   docs/TECH_DEBT.md                                         (Fix 2 doc)
new file:   docs/XP_REGRESSION_2026_05_20.md                          (this file)
new file:   /Users/joa/code/mkmsbr/docs/XP_INT13_DRIVE_SWAP_SPEC.md   (Fix 3)
new file:   crates/bootsmith/src/bin/                                    (unrelated eval bin)
```

All in working-tree, not committed.

## References

- The two research-agent reports are summarised inline above. Full
  text in the chat transcript for this session.
- Symptom 1 forensic dumps: USB MBR / PBR / boot.ini / `ls -la`
  output captured in chat transcript.
- `docs/V0.3_WINDOWS_XP.md` — overall XP-USB recipe.
- `docs/XP_BOOT_INI.md` — boot.ini design history and canonical
  USB_MultiBoot template comparison.
- `docs/FIELD_FINDINGS_2026_05_18.md` — original Win 7 mode bring-up
  notes; the diagnostic-string table at section 9 (flashing-cursor
  = MBR ran, PBR couldn't find boot file) is the playbook we used
  for Symptom 1.
- mkmsbr `XP_INT13_DRIVE_SWAP_SPEC.md` — drive-swap spec for the
  eventual proper fix to Bug B.
- WinSetupFromUSB / ruo91/USB_MultiBoot — canonical template
  source, particularly `makebt/winnt_rec.sif` for the `unused=
  unused` pattern.
- ReactOS `base/setup/usetup/usetup.c` `SelectPartitionPage()` —
  the only readable equivalent to setupdd's partitioner-show
  predicate.
</content>
</invoke>

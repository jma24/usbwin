; ntfs_pbr.asm — usbwin NTFS Partition Boot Record.
;
; STUB. Real implementation pending. The contract:
;
;   1. Loaded by the MBR at 0000:7C00 in real mode.
;   2. Bytes 0..2 are the initial JMP, bytes 3..89 are the NTFS BPB (runtime).
;   3. Locate the $MFT, walk it to find the bootmgr file.
;   4. Read bootmgr into memory and jump to it.
;   5. Total binary size: exactly 512 bytes, with 0x55 0xAA at offset 510.
;
; Reference: docs/BOOT_RECORDS.md.

BITS 16
ORG 0x7C00

start:
    jmp short body
    nop

    times 87 db 0       ; BPB placeholder; spliced at write time.

body:
    cli
    xor ax, ax
    mov ds, ax
    mov ss, ax
    mov sp, 0x7C00
    sti

.halt:
    hlt
    jmp .halt

    times 510 - ($ - $$) db 0
    dw 0xAA55

; mbr.asm — usbwin MBR boot code.
;
; STUB. Real implementation pending. The contract:
;
;   1. Loaded by BIOS at 0000:7C00 in real mode.
;   2. Find the active (boot flag = 0x80) primary partition in the partition
;      table at offset 0x1BE..0x1FE of this sector.
;   3. Read that partition's first sector to 0000:7C00 (relocating self first
;      to 0000:0600).
;   4. Jump to 0000:7C00 to chain-load the PBR.
;   5. Total binary size: exactly 512 bytes, with 0x55 0xAA at offset 510.
;
; Reference: docs/BOOT_RECORDS.md.

BITS 16
ORG 0x7C00

start:
    cli
    xor ax, ax
    mov ss, ax
    mov sp, 0x7C00
    sti

    ; STUB body: halt forever. Real implementation pending.
.halt:
    hlt
    jmp .halt

    ; Pad to byte 510, then signature.
    times 510 - ($ - $$) db 0
    dw 0xAA55

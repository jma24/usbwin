; fake_bootmgr.asm — a tiny stand-in for Microsoft's bootmgr, used by the
; QEMU smoke test. NOT shipped to users; only built when running the
; integration tests.
;
; Contract:
;   - Loaded by our FAT32 PBR at some real-mode segment:offset (Microsoft's
;     PBR uses 2000:0; the contract is "wherever the PBR jumps to, we
;     start here").
;   - Prints "USBWIN OK\n" via two channels:
;       * BIOS teletype (INT 10h, AH=0Eh) - visible if QEMU runs with
;         a graphical display.
;       * Serial port COM1 (port 0x3F8) - captured by QEMU's
;         `-serial stdio` redirection.
;   - Halts forever.
;
; The serial output is what the qemu_pbr.rs test asserts on.

BITS 16
ORG 0

start:
    mov si, msg
.loop:
    lodsb
    or al, al
    jz .done
    ; BIOS teletype (default video page)
    mov ah, 0x0E
    mov bh, 0x00
    int 0x10
    ; Serial COM1
    mov dx, 0x3F8
    out dx, al
    jmp .loop

.done:
    cli
.hang:
    hlt
    jmp .hang

msg: db 'USBWIN OK', 13, 10, 0

/* The app links above the factory Adafruit UF2 bootloader's S140 v7.3.0 SoftDevice:
   FLASH starts at 0x27000 (the SoftDevice occupies the low region) and RAM above the
   SoftDevice's reservation. The UF2 --base in scripts/techo-flash.sh MUST equal this
   FLASH ORIGIN.

   The RAM reservation grows with the connection config. At conn_count=4 / att_mtu=247
   the S140 needs ~30 KB (conn_count=6 ran at 52 KB, conn_count=12 needed >48 and <=64);
   reserved 0xB000 (44 KB) with margin, so the app keeps 0x35000 (212 KB) — most of it
   stack. The inline reactor/LoRa/render run loops plus the per-link handshake crypto are
   stack-hungry: at MEMBERS=10 they starved the stack to 79 KB and halted at startup, and
   at MEMBERS=4 a 3rd concurrent link HardFaulted ~111 KB. MEMBERS=2 caps concurrency and
   leaves ~136 KB. The board faults silently if this is too low. */
MEMORY
{
  FLASH : ORIGIN = 0x00027000, LENGTH = 0xC6000
  RAM   : ORIGIN = 0x2000B000, LENGTH = 0x35000
}

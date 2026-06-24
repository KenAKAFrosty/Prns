/* The app links above the factory Adafruit UF2 bootloader's S140 v7.3.0 SoftDevice:
   FLASH starts at 0x27000 (the SoftDevice occupies the low region) and RAM above the
   SoftDevice's reservation. The UF2 --base in scripts/techo-flash.sh MUST equal this
   FLASH ORIGIN.

   The RAM reservation grows with the connection config (conn_count = BLE_MEMBERS + 2 and
   att_mtu=247): conn_count=4 needs ~30 KB, conn_count=6 ~52 KB, conn_count=12 >48 and <=64.
   If the reservation is too low, `Softdevice::enable` panics with the exact required start
   address ("change your app's RAM start address to {:x}") — captured + re-emitted on the
   next boot, so this is now measured, not guessed. The remaining ceiling is the STACK: the
   inline reactor/LoRa/render run loops plus the per-link dalek handshake crypto are
   stack-hungry, and the stack is whatever app RAM the SD reservation + statics leave. At
   MEMBERS=10 that fell to 79 KB and overflowed at startup; MEMBERS=2 left ~136 KB. The board
   faults if the stack is too small. MEMBERS=4 (conn_count=6, 52 KB reserved, ~111 KB stack).
   The event-buffer panic that earlier looked like a MEMBERS=4 link fault is fixed separately
   (evt-max-size-512), so MEMBERS=4 is being re-validated post-fix. */
MEMORY
{
  FLASH : ORIGIN = 0x00027000, LENGTH = 0xC6000
  RAM   : ORIGIN = 0x2000D000, LENGTH = 0x33000
}

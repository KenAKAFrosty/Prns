/* The app links above the factory Adafruit UF2 bootloader's S140 v7.3.0 SoftDevice:
   FLASH starts at 0x27000 (the SoftDevice occupies the low region) and RAM above the
   SoftDevice's reservation. The UF2 --base in scripts/techo-flash.sh MUST equal this
   FLASH ORIGIN.

   The RAM reservation grows with the connection config (conn_count = BLE_MEMBERS + 2 and
   att_mtu=247): conn_count=4 needs ~30 KB, conn_count=6 ~52 KB, conn_count=8 fits in 56 KB,
   conn_count=12 >48 and <=64. If the reservation is too low, `Softdevice::enable` panics with
   the exact required start address ("change your app's RAM start address to {:x}").

   The binding ceiling is the STACK, not the SD reservation: the stack is whatever app RAM the
   SD reservation + statics leave, and the per-poll peak (the dalek handshake crypto on the
   dial/connect path) measured ~80 KB and is roughly MEMBERS-independent. Engine construction
   used to dominate, but is built in place now (StaticCell::init_with), so the runtime crypto
   path sets the peak. MEMBERS=6 (conn_count=8, 56 KB reserved) leaves a ~97 KB stack region —
   ~18 KB headroom over the ~80 KB peak, measured live by stack-painting under concurrent
   links. MEMBERS=8 would leave only ~84 KB (too tight); MEMBERS=10 ~70 KB (overflows). Going
   higher needs cutting the runtime crypto/connect stack, or a larger-RAM part. */
MEMORY
{
  FLASH : ORIGIN = 0x00027000, LENGTH = 0xC6000
  RAM   : ORIGIN = 0x2000E000, LENGTH = 0x32000
}

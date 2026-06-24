/* The app links above the factory Adafruit UF2 bootloader's S140 v7.3.0 SoftDevice:
   FLASH starts at 0x27000 (the SoftDevice occupies the low region) and RAM above the
   SoftDevice's reservation. The UF2 --base in scripts/techo-flash.sh MUST equal this
   FLASH ORIGIN.

   The RAM reservation grows with the connection config. At conn_count=6 / att_mtu=247
   the S140 needs ~36 KB (conn_count=12 needed >48 and <=64); reserved 0xD000 (52 KB)
   with margin, so the app keeps 0x33000 (204 KB) — most of it stack, since the inline
   reactor/LoRa/render run loops are stack-hungry (MEMBERS=10 starved them at 79 KB and
   halted as the loops started). The board panic-halts silently if this is too low. */
MEMORY
{
  FLASH : ORIGIN = 0x00027000, LENGTH = 0xC6000
  RAM   : ORIGIN = 0x2000D000, LENGTH = 0x33000
}

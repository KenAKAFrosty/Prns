/* The app links above the factory Adafruit UF2 bootloader's S140 v6 SoftDevice:
   FLASH starts at 0x26000 (the SoftDevice occupies the low region) and RAM at
   0x20006000 (the SoftDevice reserves the first 0x6000). The UF2 --base in
   scripts/techo-flash.sh MUST equal this FLASH ORIGIN. */
MEMORY
{
  FLASH : ORIGIN = 0x00026000, LENGTH = 0xC7000
  RAM   : ORIGIN = 0x20006000, LENGTH = 0x3A000
}

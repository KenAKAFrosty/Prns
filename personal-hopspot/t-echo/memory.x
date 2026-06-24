/* The app links above the factory Adafruit UF2 bootloader's S140 v7.3.0 SoftDevice:
   FLASH starts at 0x27000 (the SoftDevice occupies the low region) and RAM at
   0x20006000 (the SoftDevice reserves the first 0x6000). The UF2 --base in
   scripts/techo-flash.sh MUST equal this FLASH ORIGIN. */
MEMORY
{
  FLASH : ORIGIN = 0x00027000, LENGTH = 0xC6000
  RAM   : ORIGIN = 0x20006000, LENGTH = 0x3A000
}

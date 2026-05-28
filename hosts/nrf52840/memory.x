/* nRF52840 Dongle (PCA10059), Nordic Open USB bootloader v2 layout (read
   live from the dongle via `nrfdfu --get-images`):
   - Bootloader:    0xF4000–0xFE000 (40 KB at top)
   - Softdevice slot: 0x1000–0x1D000 (112 KB, S140 v7.2.0 slot — unused by us)
   - App slot:      0x1C000–0xF4000 (864 KB) */
MEMORY
{
  FLASH : ORIGIN = 0x0001C000, LENGTH = 864K
  RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}

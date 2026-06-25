RESERVE_ICACHE = 0x8000;


VECTORS_SIZE = 0x400;

/* Specify main memory areas

 40370000 <- IRAM/Icache -> 40378000 <- D/IRAM (I) -> 403E0000
                            3FC88000 <- D/IRAM (D) -> 3FCF0000 <- DRAM/DCache -> 3FD00000

 Startup code uses the IRAM from 0x403B9000 to 0x403E0000, which is not available for static
 memory, but can only be used after app starts.

 D cache use the memory from high address, so when it's configured to 16K/32K, the region
 0x3FCF0000 ~ (3FD00000 - DATA_CACHE_SIZE) should be available. This region is not used as
 static memory, leaving to the heap.

 Heltec V4 coex + ESP-NOW + SoftAP umbrella: dram2_seg ORIGIN raised +0x7800 from esp-hal's 0x3FCDB700
 so the core-0 main-task stack (the leftover at the top of dram_seg, pinned to ORIGIN(dram2_seg)) has
 room for the radios' static buffers, two simultaneous embassy-net netifs (station + SoftAP), AND the
 dual-role BLE driver's poll depth: that future runs trouble-host -> esp-radio HCI -> the controller
 blob's r_btdm_task_post synchronously on this stack (now also for scan/connect, not just advertise),
 and the GATT-client serve path made it the deepest core-0 frame. The fit at full coex+SoftAP is at the
 internal-SRAM ceiling, funded three ways: the GATT client + reassembler are boxed to PSRAM (ble.rs),
 the BLE packet pool is 8 (.cargo/config.toml), and ~6 KiB was rebalanced to this stack — +0x800 here
 (heap_allocator! 44->42 KiB, its overflow lands in the DMA-capable D-cache donation) plus 4 KiB off
 core 1's stack (CORE1_STACK_BYTES 84->80, soak-tested). The SoftAP folds in with NO extra core-0 stack
 (its run-loop MTU rx buffers are boxed onto the heap in run_core). ~1.5 KiB internal heap headroom +
 ~2 KiB core-0 stack margin at full coex + SoftAP — do not trim further without re-measuring + soaking.
*/
MEMORY
{
  vectors_seg ( RX )     : ORIGIN = 0x40370000 + RESERVE_ICACHE, len = VECTORS_SIZE
  iram_seg ( RX )        : ORIGIN = 0x40370000 + RESERVE_ICACHE + VECTORS_SIZE, len = 328k - VECTORS_SIZE - RESERVE_ICACHE

  /* memory available after the 2nd stage bootloader is finished */
  dram2_seg ( RW )       : ORIGIN = 0x3FCE2F00, len = 0x3FCED710 - 0x3FCE2F00
  dram_seg ( RW )        : ORIGIN = 0x3FC88000 , len = ORIGIN(dram2_seg) - 0x3FC88000

  /* external flash
     The 0x20 offset is a convenience for the app binary image generation.
     Flash cache has 64KB pages. The .bin file which is flashed to the chip
     has a 0x18 byte file header, and each segment has a 0x08 byte segment
     header. Setting this offset makes it simple to meet the flash cache MMU's
     constraint that (paddr % 64KB == vaddr % 64KB).)
  */
  irom_seg ( RX )        : ORIGIN = 0x42000020, len = 32M - 0x20
  drom_seg ( R )         : ORIGIN = 0x3C000020, len = 32M - 0x20


  /* RTC fast memory (executable). Persists over deep sleep. Only for core 0 (PRO_CPU) */
  rtc_fast_seg(RWX) : ORIGIN = 0x600fe000, len = 8k

  /* RTC slow memory (data accessible). Persists over deep sleep. */
  rtc_slow_seg(RW)       : ORIGIN = 0x50000000, len = 8k
}

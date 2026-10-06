# Board Thumbnails

These thumbnails are intentionally tiny so the hosted flash board catalog loads
quickly. `build.rs` embeds them as `data:` URIs so the catalog needs no separate
image requests.

Format: **WebP with alpha**, 160 px on the long edge. Each board renders on a
transparent background inside a dark "slot" (`.flash-board-slot--inset` in
`tailwind.css`), so the assets must be true cutouts, not white-background photos.

## How each cutout is made
- If the vendor source already has alpha (a real cutout), use it directly.
- Otherwise key the white background out of the **highest-resolution** catalog
  photo (downscaling after the key anti-aliases the edge; eroding the alpha trims
  the residual white ring), with `ffmpeg`:
  `ffmpeg -i hi-res.jpg -vf "format=rgba,colorkey=0xFFFFFF:0.13:0.04,split[m][a];[a]alphaextract,erosion[e];[m][e]alphamerge,scale=160:160:force_original_aspect_ratio=decrease:flags=lanczos" cut.png`
  (key at low resolution and a white fringe glows against the dark slot).
- For a white enclosure on a pure-white canvas, first flood-fill only the
  corner-connected canvas to magenta so the enclosure remains opaque, then key,
  erode, and downscale it:
  `ffmpeg -i source.png -vf "format=rgba,floodfill=x=0:y=0:s0=-1:s1=-1:s2=-1:s3=-1:d0=0:d1=255:d2=255:d3=255,colorkey=0xFF00FF:0.04:0.02,split[m][a];[a]alphaextract,erosion[e];[m][e]alphamerge,scale=160:160:flags=lanczos" -c:v libwebp -quality 90 -compression_level 6 -pix_fmt yuva420p board.webp`
- Encode to WebP with `cwebp` (`brew install webp`): `cwebp -q 90 cut.png -o board.webp`
  (many `ffmpeg` builds ship without the WebP encoder). The board's own white
  silkscreen survives the key because it is not pure `#FFFFFF`.

| Board | Product page | Source image | Cutout | Basis |
| --- | --- | --- | --- | --- |
| Heltec V4 | https://heltec.org/project/wifi-lora-32-v4/ | https://heltec.org/wp-content/uploads/2025/09/v4001-300x300.png | real alpha (vendor PNG) | vendor image, nominative use |
| LilyGO T-Beam Supreme | https://lilygo.cc/products/t-beam-supreme | https://cdn.shopify.com/s/files/1/0617/7190/7253/files/LILYGO-T-BEAM_10_3bb84be5-da09-4626-8b93-99be997d49b8.jpg | white-knockout | vendor catalog image, nominative use |
| Seeed XIAO ESP32-C6 | https://www.seeedstudio.com/Seeed-Studio-XIAO-ESP32C6-p-5884.html | https://media-cdn.seeedstudio.com/media/catalog/product/cache/bb49d3ec4ee05b6f018e93f896b8a25d/1/-/1-113991254-seeedxiao-esp32c6-45font_1.jpg | white-knockout | vendor catalog image, nominative use |
| LilyGO T-Echo | https://lilygo.cc/products/t-echo-lilygo | https://cdn.shopify.com/s/files/1/0617/7190/7253/products/K142_3.jpg | white-knockout | vendor catalog image, nominative use |
| SenseCAP Card Tracker T1000-E | https://www.seeedstudio.com/SenseCAP-Card-Tracker-T1000-E-for-Meshtastic-p-5913.html | https://media-cdn.seeedstudio.com/media/catalog/product/3/-/3-114993369-sensecap-card-tracker-t1000-e-for-meshtastic-45font.jpg | white-knockout (key 0.16: the 0.13 key left a gray studio-shadow halo; the dark shell tolerates the wider key and the silkscreen survives) | vendor catalog image, nominative use |
| Heltec Mesh Node T114 | https://heltec.org/project/mesh-node-t114/ | https://heltec.org/wp-content/uploads/2024/08/9-1.png | real alpha (vendor PNG) | vendor image, nominative use |
| Heltec MeshPocket | https://heltec.org/project/meshpocket/ | https://heltec.org/wp-content/uploads/2025/03/5.1-1.png | connected-background flood-fill (white enclosure preserved) | vendor catalog image, nominative use |
| Heltec Mesh Node T096 | https://heltec.org/project/t096/ | https://heltec.org/wp-content/uploads/2026/03/T096_1.png | real alpha (vendor PNG) | vendor image, nominative use |
| Heltec MeshTower V2 | https://heltec.org/project/meshtower/ | https://heltec.org/wp-content/uploads/2025/06/1-2.png | real alpha (vendor PNG, full solar + antenna + enclosure kit) | vendor image, nominative use |
| Heltec Vision Master E290 | https://heltec.org/project/vision-master-e290/ | https://heltec.org/wp-content/uploads/2024/06/0fe1c762582fa015eeb11bd46af9699.png | real alpha (vendor PNG); cropped to the board's alpha bounds below `y=175`, which drops the vendor logo watermark in the top-left corner | vendor image, nominative use |
| Heltec Wireless Stick Lite V3 | https://heltec.org/project/wireless-stick-lite-v2/ | https://heltec.org/wp-content/uploads/2023/09/wireless-stickLite-front.png | real alpha (vendor PNG); cropped to the board's alpha bounds | vendor image, nominative use |
| RAK WisBlock Starter Kit (RAK4631) | https://store.rakwireless.com/products/wisblock-starter-kit | https://cdn.shopify.com/s/files/1/0177/8784/6756/files/RAK4630.png?v=1770026490 | real alpha (vendor PNG, the RAK4631-variant kit image); cropped to the kit's alpha bounds | vendor catalog image, nominative use |
| muzi works Base Duo | https://muzi.works/products/base-duo | https://cdn.shopify.com/s/files/1/0657/6973/4201/files/base-duo.png?v=1765232558 | connected-background flood-fill (opaque render on a flat `#F2F2F2` canvas; the seed takes the corner color, so the white-canvas recipe applies unchanged) | vendor catalog image, nominative use |
| Elecrow ThinkNode G4 | https://www.elecrow.com/thinknode-g4-wi-fi-halow-gateway-support-wi-fi-wi-fi-halow-ethernet-connections-supports-ap-sta-mesh-etc.html | https://www.elecrow.com/media/catalog/product/t/h/thninknode_g4_wi-fi_halow_gateway.jpg | white-knockout, cropped to `500:960:240:25` before scaling | vendor catalog image, nominative use |
| Heltec HT-HD01 | https://heltec.org/project/ht-hd01/ | https://heltec.org/wp-content/uploads/2024/12/4-3.png | white-knockout, cropped to `280:730:250:40` before scaling | vendor catalog image, nominative use |
| Heltec V3 | https://heltec.org/project/wifi-lora-32-v3/ | https://heltec.org/wp-content/uploads/2023/09/2.png | real alpha (vendor PNG) | vendor image, nominative use |
| Seeed Wio Tracker L1 | https://wiki.seeedstudio.com/wio_tracker_l1_node/ | https://media-cdn.seeedstudio.com/media/catalog/product/cache/bb49d3ec4ee05b6f018e93f896b8a25d/1/-/1-114993648-wio-tracker-l1.jpg | white-knockout | vendor catalog image, nominative use |
| Seeed XIAO ESP32-S3 + Wio-SX1262 | https://www.seeedstudio.com/Wio-SX1262-with-XIAO-ESP32S3-p-5982.html | https://media-cdn.seeedstudio.com/media/catalog/product/2/-/2-102010611-wio-sx1262-with-xiao-esp32s3.jpg | built-in imagegen background extraction; WebP thumbnail | vendor product photo, nominative use; AI-assisted cutout |
| SenseCAP Solar Node P1 / P1-Pro | https://www.seeedstudio.com/SenseCAP-Solar-Node-P1-Pro-for-Meshtastic-LoRa-p-6412.html | https://media-cdn.seeedstudio.com/media/catalog/product/2/-/2-114993633-sensecap-solar-node-p1-pro-for-meshtastic_1.jpg | built-in imagegen background extraction; WebP thumbnail | vendor product photo, nominative use; AI-assisted cutout |
| RAK WisMesh 1W (RAK10724) | https://store.rakwireless.com/products/meshtastic-1w-lora-booster-kit-rak3401 | https://cdn.shopify.com/s/files/1/0177/8784/6756/files/RAK10724_WisMesh_1W-Booster-Starter-Kit-01_3.png?v=1785997006 | built-in imagegen board extraction; WebP thumbnail | vendor product photo, nominative use; AI-assisted cutout |
| Raspberry Pi Zero 2 W | https://www.raspberrypi.com/products/raspberry-pi-zero-2-w/ | https://assets.raspberrypi.com/static/51035ec4c2f8f630b3d26c32e90c93f1/6e7df/zero2-hero.png | original vendor alpha preserved; `cwebp -q 90 -resize 160 0` | vendor product image, nominative use |
| Elecrow ThinkNode M7 | https://www.elecrow.com/thinknode-m7-wireless-communication-gateway-for-meshtastic-support-poe-powered-powered-by-esp32-s3-and-lr1110.html | https://www.elecrow.com/media/catalog/product/cache/b6b9577937e6a96f50e53ddc42983628/t/h/thinknode_m7_for_meshtastic_1_1.jpg | built-in imagegen background extraction; WebP thumbnail | vendor product photo, nominative use; AI-assisted cutout |

Real-alpha vendor originals are not stored in the repo; the source URL above is
the pointer, and regeneration is a plain download, a lanczos `scale=160`, and a
quality-90 WebP encode. When the vendor canvas leaves the board small, crop to
its alpha bounds first so the thumbnail fills the slot (`cropdetect` on the
extracted alpha, then `crop=W:H:X:Y`), as the `cropped to ... alpha bounds` rows
above do.

All product images are shown nominatively, only to identify hardware (no
endorsement implied); see the site footer disclaimer.

A genuinely transparent XIAO ESP32-C6 render also exists on the Seeed wiki
(`https://files.seeedstudio.com/wiki/SeeedStudio-XIAO-ESP32C6/img/XIAO_ESP32-C6_front_pinout.png`,
**CC BY-SA 4.0**). It is a pinout image (needs a center-crop) and carries an
attribution + share-alike obligation, so the catalog uses the white-knockout for
a uniform basis instead. Swap to it for a crisper edge if wanted, and add the
Seeed CC BY-SA attribution here and in the footer.

The two Seeed cutouts added for the firmware integration used the built-in imagegen tool with the instruction: remove only the white backdrop and floor shadow to transparency; preserve the photographed hardware, labels, colors, proportions and viewing angle; add no text or objects. The transparent outputs were inspected and encoded with `cwebp -q 90 -resize 160 0`.

The WisMesh cutout used the built-in imagegen tool with this prompt: extract only the complete assembled circuit board in the lower left; remove the white background, loose accessories, callouts and marketing text; preserve the board silhouette, geometry, component placement, colors, printed labels, perspective and angle; center it on a transparent square with a small margin; add no parts, labels, redesign or shadows. It was visually inspected and encoded with the same WebP command. These AI-assisted cutouts identify products at thumbnail size; use the linked vendor photographs for component and label details.

The M7 cutout used built-in imagegen: remove only the white background, floor shadow and detached green Powered logo; preserve the entire enclosure and both antennas, ports, printed labels, colors, proportions and viewing angle; center the complete device on transparent canvas without adding objects or redesigning hardware. The result was inspected and encoded with `cwebp -q 90 -resize 160 0`.

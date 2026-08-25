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
| Heltec Mesh Node T096 | https://heltec.org/project/t096/ | https://heltec.org/wp-content/uploads/2026/03/T096_1.png | real alpha (vendor PNG) | vendor image, nominative use |
| Heltec MeshTower V2 | https://heltec.org/project/meshtower/ | https://heltec.org/wp-content/uploads/2025/06/1-2.png | real alpha (vendor PNG, full solar + antenna + enclosure kit) | vendor image, nominative use |

Real-alpha vendor originals are not stored in the repo; the source URL above is
the pointer, and regeneration is a plain download, a lanczos `scale=160`, and a
quality-90 WebP encode.

All product images are shown nominatively, only to identify hardware (no
endorsement implied); see the site footer disclaimer.

A genuinely transparent XIAO ESP32-C6 render also exists on the Seeed wiki
(`https://files.seeedstudio.com/wiki/SeeedStudio-XIAO-ESP32C6/img/XIAO_ESP32-C6_front_pinout.png`,
**CC BY-SA 4.0**). It is a pinout image (needs a center-crop) and carries an
attribution + share-alike obligation, so the catalog uses the white-knockout for
a uniform basis instead. Swap to it for a crisper edge if wanted, and add the
Seeed CC BY-SA attribution here and in the footer.

#!/bin/bash
#
# Regenerates the social / Open Graph card at public/assets/og.png (1200x630).
#
# The card is HTML rendered headless, then supersampled down for crisp edges.
# Platform logos are inlined from public/assets/logos at build time (tinted via
# the SVG `fill`), so the "Runs on" strip always matches the bundled set.
#
# Requires: Google Chrome (headless render) and macOS `sips` (downscale).
# Usage: bash tools/og/build-og.sh
set -e

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEBROOT="$(cd "$HERE/../.." && pwd)"
LOGODIR="$WEBROOT/public/assets/logos"
OUT_PNG="$WEBROOT/public/assets/og.png"
TMP="${TMPDIR:-/tmp}"
OUT_HTML="$TMP/prns-og-card.html"
OUT_2X="$TMP/prns-og-2x.png"

# Logo strip: desktop/mobile, then embedded (incl. nRF), web, languages.
LOGOS="linux apple windows android espressif raspberrypi nordicsemiconductor webassembly rust python typescript"
STRIP=""
for l in $LOGOS; do
  paths=$(grep -oE '<path[^>]*>' "$LOGODIR/$l.svg" | tr '\n' ' ')
  STRIP="$STRIP<svg class=\"logo\" viewBox=\"0 0 24 24\" fill=\"#aeb5c0\">$paths</svg>"
done

cat > "$OUT_HTML" <<HTMLEOF
<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<style>
  :root{--ink:#0b0e13;--surface:#11151c;--line:#1a1f28;--paper:#f4f6fa;--soft:#a5acb8;--mid:#6c7480;--accent:#6ee7b7;--accent-strong:#34d399;}
  *{margin:0;padding:0;box-sizing:border-box}
  html,body{width:1200px;height:630px;overflow:hidden}
  body{background:var(--ink);font-family:-apple-system,BlinkMacSystemFont,"SF Pro Display","Segoe UI",sans-serif;-webkit-font-smoothing:antialiased;position:relative}
  .glow{position:absolute;width:900px;height:900px;right:-260px;top:50%;transform:translateY(-50%);background:radial-gradient(circle, rgba(110,231,183,0.13), transparent 62%);pointer-events:none}
  .ghost{position:absolute;right:-150px;top:50%;transform:translateY(-50%);width:600px;height:600px;opacity:0.08}
  .topline{position:absolute;top:0;left:0;right:0;height:4px;background:linear-gradient(90deg,var(--accent-strong),var(--accent) 45%,transparent 88%)}
  .content{position:relative;z-index:2;height:100%;padding:70px 84px 64px;display:flex;flex-direction:column;justify-content:space-between}
  .lockup{display:flex;align-items:center;gap:14px}
  .lockup .wm{font-size:31px;font-weight:640;letter-spacing:-0.02em;color:var(--paper)}
  .lockup .wm .p{color:var(--accent)}
  .eyebrow{color:var(--accent);font-size:14.5px;font-weight:650;letter-spacing:0.22em;text-transform:uppercase;margin-bottom:20px}
  h1{font-size:57px;line-height:1.07;font-weight:700;letter-spacing:-0.025em;color:var(--paper)}
  h1 .hl{color:var(--accent)}
  .sub{margin-top:23px;font-size:22px;line-height:1.5;color:var(--soft);max-width:740px}
  .bottom{display:flex;flex-direction:column;gap:20px}
  .runson{display:flex;align-items:center;gap:18px}
  .runson .lbl{color:var(--accent);font-size:12.5px;font-weight:700;letter-spacing:0.2em;text-transform:uppercase;white-space:nowrap}
  .logos{display:flex;align-items:center;gap:17px}
  .logo{width:27px;height:27px;display:inline-block}
  .more{color:var(--mid);font-size:17px;white-space:nowrap}
  .metarow{display:flex;align-items:center;justify-content:space-between}
  .chips{display:flex;gap:12px}
  .chip{border:1px solid var(--line);border-radius:9px;padding:9px 15px;font-size:16px;color:var(--soft)}
  .chip.lic{color:var(--paper);border-color:rgba(110,231,183,0.32)}
  .url{font-family:ui-monospace,"SF Mono",Menlo,monospace;font-size:19px;color:var(--mid);letter-spacing:0.01em}
</style></head>
<body>
  <div class="glow"></div>
  <div class="topline"></div>
  <svg class="ghost" viewBox="0 0 100 100">
    <circle cx="50" cy="50" r="37" fill="none" stroke="#6ee7b7" stroke-width="2.4"/>
    <g stroke="#6ee7b7" stroke-width="2.4" stroke-linecap="round" transform="rotate(46 50 50)">
      <line x1="50" y1="7" x2="50" y2="16"/><line x1="50" y1="84" x2="50" y2="93"/>
      <line x1="7" y1="50" x2="16" y2="50"/><line x1="84" y1="50" x2="93" y2="50"/></g>
    <g fill="none" stroke="#6ee7b7" stroke-linecap="round" stroke-width="2.2">
      <path d="M57.46 39.35 A13 13 0 0 1 57.46 60.65"/><path d="M62.04 32.8 A21 21 0 0 1 62.04 67.2"/>
      <path d="M42.54 39.35 A13 13 0 0 0 42.54 60.65"/><path d="M37.96 32.8 A21 21 0 0 0 37.96 67.2"/></g>
    <circle cx="50" cy="50" r="6" fill="#34d399"/>
  </svg>
  <div class="content">
    <div class="lockup">
      <svg width="50" height="50" viewBox="0 0 100 100">
        <circle cx="50" cy="50" r="37" fill="none" stroke="#6ee7b7" stroke-width="3"/>
        <g stroke="#6ee7b7" stroke-width="3" stroke-linecap="round" transform="rotate(46 50 50)">
          <line x1="50" y1="7" x2="50" y2="16"/><line x1="50" y1="84" x2="50" y2="93"/>
          <line x1="7" y1="50" x2="16" y2="50"/><line x1="84" y1="50" x2="93" y2="50"/></g>
        <g fill="none" stroke="#6ee7b7" stroke-linecap="round" stroke-width="2.6">
          <path d="M57.46 39.35 A13 13 0 0 1 57.46 60.65" opacity="0.9"/><path d="M62.04 32.8 A21 21 0 0 1 62.04 67.2" opacity="0.45"/>
          <path d="M42.54 39.35 A13 13 0 0 0 42.54 60.65" opacity="0.9"/><path d="M37.96 32.8 A21 21 0 0 0 37.96 67.2" opacity="0.45"/></g>
        <circle cx="50" cy="50" r="6" fill="#34d399"/>
      </svg>
      <span class="wm"><span class="p">P</span>rns</span>
    </div>
    <div>
      <div class="eyebrow">Unstoppable mesh networks, for the people</div>
      <h1>Production-grade Reticulum.<br><span class="hl">Runs on any device.</span></h1>
      <p class="sub">A deterministic, no_std, alloc-free core. From a five-dollar microcontroller to a cloud server.</p>
    </div>
    <div class="bottom">
      <div class="runson">
        <span class="lbl">Runs on</span>
        <span class="logos">$STRIP</span>
        <span class="more">and many more</span>
      </div>
      <div class="metarow">
        <div class="chips">
          <span class="chip lic">MIT / Apache 2.0</span>
          <span class="chip">Zero unsafe</span>
          <span class="chip">Diff-tested</span>
          <span class="chip">Benchmarked</span>
        </div>
        <div class="url">reticulum.rs</div>
      </div>
    </div>
  </div>
</body></html>
HTMLEOF

CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
if [ ! -x "$CHROME" ]; then echo "error: Google Chrome not found at $CHROME" >&2; exit 1; fi
if ! command -v sips >/dev/null 2>&1; then echo "error: sips (macOS) required for downscale" >&2; exit 1; fi

"$CHROME" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=2 \
  --window-size=1200,630 --screenshot="$OUT_2X" "file://$OUT_HTML" >/dev/null 2>&1
sips -z 630 1200 "$OUT_2X" --out "$OUT_PNG" >/dev/null 2>&1
rm -f "$OUT_2X" "$OUT_HTML"
echo "wrote $OUT_PNG ($(sips -g pixelWidth -g pixelHeight "$OUT_PNG" 2>/dev/null | grep pixel | awk '{print $2}' | paste -sd x -))"

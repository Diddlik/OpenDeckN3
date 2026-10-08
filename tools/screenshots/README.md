# README-Screenshots erzeugen

Die Bilder in `docs/images/` entstehen mit dem virtuellen Gerät und einem Demo-Profil.
Voraussetzungen: Node.js ≥ 22, Python mit Pillow, Playwright (global), ImageMagick.

```sh
cargo build
./target/debug/opendeckn3d --virtual --no-hardware --plugins-dir plugins/examples --config-dir /tmp/n3demo &
python3 tools/screenshots/key-images.py > /tmp/key-images.json
node tools/screenshots/demo-profile.mjs /tmp/key-images.json          # Profile + Belegung anlegen
mkdir -p /tmp/frames
NODE_PATH=$(npm root -g) node tools/screenshots/capture.cjs docs/images /tmp/frames
convert -delay 14 -loop 0 /tmp/frames/f*.png -resize 760x -gravity center -crop 760x500+0+0 +repage \
  -fuzz 2% -layers Optimize -colors 160 docs/images/demo.gif
NODE_PATH=$(npm root -g) node tools/screenshots/hero.cjs                # Banner aus editor-dark.png
```

// Renders the device key images for built-in actions from the UI icon set,
// so the app preview and the hardware look the same.
//
// Usage: NODE_PATH=$(npm root -g) node tools/make-builtin-icons.cjs
// Output: crates/n3-daemon/assets/builtin/<name>.png (144×144)
const fs = require('fs');
const path = require('path');
const { chromium } = require('playwright');

const root = path.resolve(__dirname, '..');
const ui = fs.readFileSync(path.join(root, 'ui/index.html'), 'utf8');
const grab = (name) => {
  const m = ui.match(new RegExp(`const ${name} = (\\{[\\s\\S]*?\\n\\});`));
  if (!m) throw new Error(`${name} not found in ui/index.html`);
  return Function(`return ${m[1]}`)();
};
const ICONS = grab('ICONS');
const KCOL = grab('KCOL');

// device image name → UI icon kind
const MAP = {
  brightness: 'brightness', profile: 'profile', 'page-next': 'pageNext', 'page-previous': 'pagePrev', 'page-goto': 'pages', hotkey: 'keyboard', text: 'text',
  'volume-up': 'volume', 'volume-down': 'volumeDown', 'volume-mute': 'mute',
  'media-playpause': 'media', 'media-next': 'next', 'media-previous': 'previous', 'media-stop': 'stop',
  launch: 'app', url: 'globe', command: 'terminal',
};

(async () => {
  const out = path.join(root, 'crates/n3-daemon/assets/builtin');
  fs.mkdirSync(out, { recursive: true });
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 144, height: 144 } });
  for (const [name, kind] of Object.entries(MAP)) {
    const [bg, fg] = KCOL[kind];
    await page.setContent(`<body style="margin:0">
      <div style="width:144px;height:144px;display:flex;align-items:center;justify-content:center;
        background:radial-gradient(120% 90% at 50% 0%, color-mix(in srgb, ${fg} 16%, ${bg}) 0%, ${bg} 70%)">
        <svg width="66" height="66" viewBox="0 0 24 24" fill="none" stroke="${fg}" stroke-width="1.75"
          stroke-linecap="round" stroke-linejoin="round" style="margin-top:-18px">${ICONS[kind]}</svg>
      </div></body>`);
    await page.screenshot({ path: path.join(out, `${name}.png`) });
    console.log('written', name);
  }
  await browser.close();
})().catch((e) => { console.error(e); process.exit(1); });

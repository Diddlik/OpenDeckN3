// Renders the key images of the Discord plugin (one PNG per action state).
//
// Usage: NODE_PATH=$(npm root -g) node tools/make-discord-icons.cjs
// Output: plugins/discord/de.opendeckn3.discord.sdPlugin/imgs/<name>.png (144×144)
const fs = require('fs');
const path = require('path');
const { chromium } = require('playwright');

const MIC = '<rect x="9" y="2" width="6" height="12" rx="3"/><path d="M19 10v1a7 7 0 0 1-14 0v-1M12 18v4M8 22h8"/>';
const HEADPHONES = '<path d="M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a9 9 0 0 1 18 0v7a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3"/>';
const SPEAKER = '<path d="M4 9v6h4l5 4V5L8 9H4z"/>';
const SLASH = '<path d="M3 3l18 18"/>';
const USER = '<circle cx="9" cy="8" r="4"/><path d="M2 21a7 7 0 0 1 14 0"/>';
const VIDEO = '<rect x="2" y="6" width="14" height="12" rx="2"/><path d="M16 10l6-4v12l-6-4"/>';
const SCREEN = '<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8M12 17v4M9 10l3-3 3 3M12 7v6"/>';
const BELL = '<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/>';
const WAVE = '<path d="M2 13a2 2 0 0 0 2-2V7a2 2 0 0 1 4 0v13a2 2 0 0 0 4 0V4a2 2 0 0 1 4 0v13a2 2 0 0 0 4 0v-4a2 2 0 0 1 2-2"/>';
const KEYBOARD = '<rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M8 14h8"/>';
const SLIDERS = '<path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6"/>';

// Discord-like palette: normal, muted/off (red), active (green).
const NORMAL = ['#1E2140', '#8C96FF'];
const OFF = ['#3A1518', '#FF7A7D'];
const ON = ['#10301D', '#57E08A'];

const ICONS = {
  plugin: [NORMAL, '<path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1.1-4.6A8 8 0 1 1 21 12z"/><path d="M9 11h.01M12 11h.01M15 11h.01"/>'],
  mic: [NORMAL, MIC],
  'mic-off': [OFF, MIC + SLASH],
  headphones: [NORMAL, HEADPHONES],
  'headphones-off': [OFF, HEADPHONES + SLASH],
  voice: [NORMAL, SPEAKER + '<path d="M16.5 8.5a5 5 0 0 1 0 7"/><path d="M19 6a8.5 8.5 0 0 1 0 12"/>'],
  'voice-on': [ON, SPEAKER + '<path d="M16.5 8.5a5 5 0 0 1 0 7"/><path d="M19 6a8.5 8.5 0 0 1 0 12"/>'],
  text: [NORMAL, '<path d="M4 9h16M4 15h16M10 3L8 21M16 3l-2 18"/>'],
  bell: [NORMAL, BELL],
  'bell-dot': [ON, BELL + '<circle cx="18.5" cy="5" r="3" fill="currentColor" stroke="none"/>'],
  'user-volume': [NORMAL, USER + '<path d="M18 8.5a3 3 0 0 1 0 5M20.5 6a6.5 6.5 0 0 1 0 10"/>'],
  'user-volume-off': [OFF, USER + '<path d="M17 9l5 5M22 9l-5 5"/>'],
  volume: [NORMAL, SPEAKER + '<path d="M16.5 8.5a5 5 0 0 1 0 7M19 6a8.5 8.5 0 0 1 0 12"/>'],
  'volume-off': [OFF, SPEAKER + '<path d="M16 9l6 6M22 9l-6 6"/>'],
  soundboard: [NORMAL, '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><path d="M16 21a2 2 0 1 1 0-.01M18 19v-5l3 1"/>'],
  audiodevice: [NORMAL, SLIDERS],
  'audiodevice-on': [ON, SLIDERS],
  camera: [ON, VIDEO],
  'camera-off': [NORMAL, VIDEO + SLASH],
  screen: [NORMAL, SCREEN],
  'screen-on': [ON, SCREEN],
  ptt: [NORMAL, '<rect x="9.5" y="5" width="5" height="9" rx="2.5"/><path d="M17 11.5a5 5 0 0 1-10 0M12 16.5V19"/><circle cx="12" cy="12" r="10.5"/>'],
  'ptt-on': [ON, '<rect x="9.5" y="5" width="5" height="9" rx="2.5"/><path d="M17 11.5a5 5 0 0 1-10 0M12 16.5V19"/><circle cx="12" cy="12" r="10.5"/>'],
  voicemode: [NORMAL, WAVE],
  'voicemode-ptt': [NORMAL, KEYBOARD],
};

(async () => {
  const out = path.resolve(__dirname, '../plugins/discord/de.opendeckn3.discord.sdPlugin/imgs');
  fs.mkdirSync(out, { recursive: true });
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 144, height: 144 } });
  for (const [name, [[bg, fg], svg]] of Object.entries(ICONS)) {
    await page.setContent(`<body style="margin:0">
      <div style="width:144px;height:144px;display:flex;align-items:center;justify-content:center;color:${fg};
        background:radial-gradient(120% 90% at 50% 0%, color-mix(in srgb, ${fg} 18%, ${bg}) 0%, ${bg} 70%)">
        <svg width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"
          stroke-linecap="round" stroke-linejoin="round" style="margin-top:-18px">${svg}</svg>
      </div></body>`);
    await page.screenshot({ path: path.join(out, `${name}.png`) });
  }
  console.log('written', Object.keys(ICONS).length, 'icons to', out);
  await browser.close();
})().catch((e) => { console.error(e); process.exit(1); });

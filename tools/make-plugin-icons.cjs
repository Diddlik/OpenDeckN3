// Renders the key images of the OBS and Home Assistant plugins (144×144 PNG).
//
// Usage: NODE_PATH=$(npm root -g) node tools/make-plugin-icons.cjs
// Output: plugins/<name>/<uuid>.sdPlugin/imgs/<icon>.png
const fs = require('fs');
const path = require('path');
const { chromium } = require('playwright');

// OBS: text badges (like the scene dock); off = dark, on = filled colour.
const badge = (text, color, on) => `
  <div style="width:144px;height:144px;display:flex;align-items:center;justify-content:center;
    background:${on ? `radial-gradient(120% 90% at 50% 0%, ${color} 0%, color-mix(in srgb, ${color} 55%, #000) 100%)` : '#17181c'}">
    <div style="margin-top:-18px;padding:10px 14px;border-radius:12px;border:3px solid ${on ? '#fff' : color};
      font:700 30px/1 'Geist Mono','DejaVu Sans Mono',monospace;letter-spacing:1px;color:${on ? '#fff' : color}">${text}</div>
  </div>`;
const OBS = {
  plugin: badge('OBS', '#9aa4ff', true),
  'link-off': badge('LINK', '#7d8590', false), link: badge('LINK', '#3fb950', true),
  rec: badge('REC', '#ff6b6b', false), 'rec-on': badge('REC', '#e5484d', true),
  live: badge('LIVE', '#ff6b9a', false), 'live-on': badge('LIVE', '#e5484d', true),
  scene: badge('SCN', '#a78bfa', false), 'scene-on': badge('SCN', '#7c5cff', true),
  'source-off': badge('SRC', '#5fd3c6', false), source: badge('SRC', '#1f9e8f', true),
  'filter-off': badge('FLT', '#f0a35e', false), filter: badge('FLT', '#d9822b', true),
  audio: badge('AUD', '#5fd38a', false), 'audio-off': badge('MUTE', '#e5484d', true),
  studio: badge('STD', '#f5b841', false), 'studio-on': badge('STD', '#c98a00', true),
  transition: badge('TRN', '#7aa7ff', false), 'transition-on': badge('TRN', '#3b6fe0', true),
  cpu: badge('CPU', '#9aa4b2', false),
  replay: badge('RPL', '#c9a0ff', false), 'replay-on': badge('RPL', '#8a4fe0', true),
  vcam: badge('CAM', '#6fd0ff', false), 'vcam-on': badge('CAM', '#1f8fd0', true),
  macro: badge('MAC', '#ff8fd0', false),
};

// Home Assistant: line icons on HA blue / active yellow.
const icon = (svg, on) => {
  const [bg, fg] = on ? ['#3a2f05', '#ffd34d'] : ['#0f2233', '#5bc4ff'];
  return `<div style="width:144px;height:144px;display:flex;align-items:center;justify-content:center;color:${fg};
    background:radial-gradient(120% 90% at 50% 0%, color-mix(in srgb, ${fg} 18%, ${bg}) 0%, ${bg} 70%)">
    <svg width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"
      stroke-linecap="round" stroke-linejoin="round" style="margin-top:-18px">${svg}</svg></div>`;
};
const HOUSE = '<path d="M3 11l9-7 9 7"/><path d="M5 10v10h14V10"/><path d="M10 20v-6h4v6"/>';
const POWER = '<path d="M12 3v9"/><path d="M6.3 6.3a8 8 0 1 0 11.4 0"/>';
const BULB = '<path d="M9 18h6M10 21h4"/><path d="M12 3a6 6 0 0 0-3.5 10.9c.6.5 1 1.2 1 2.1h5c0-.9.4-1.6 1-2.1A6 6 0 0 0 12 3z"/>';
const GAUGE = '<path d="M12 14l4-4"/><path d="M3.5 18a9 9 0 1 1 17 0"/>';
const HA = {
  plugin: icon(HOUSE, false),
  webhook: icon('<circle cx="6" cy="17" r="2.5"/><circle cx="18" cy="17" r="2.5"/><circle cx="12" cy="6" r="2.5"/><path d="M10.7 8.2L7.3 14.8M8.5 17h7M13.3 8.2l3.4 6.6"/>', false),
  service: icon('<path d="M5 12h9M10 7l5 5-5 5"/><path d="M19 5v14"/>', false),
  power: icon(POWER, false), 'power-on': icon(POWER, true),
  dimmer: icon(BULB, false), 'dimmer-on': icon(BULB, true),
  sensor: icon(GAUGE, false), 'sensor-on': icon(GAUGE, true),
};

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 144, height: 144 } });
  for (const [dir, set] of [['plugins/obs/de.opendeckn3.obs.sdPlugin/imgs', OBS], ['plugins/homeassistant/de.opendeckn3.homeassistant.sdPlugin/imgs', HA]]) {
    const out = path.resolve(__dirname, '..', dir);
    fs.mkdirSync(out, { recursive: true });
    for (const [name, html] of Object.entries(set)) {
      await page.setContent(`<body style="margin:0">${html}</body>`);
      await page.screenshot({ path: path.join(out, `${name}.png`) });
    }
    console.log('written', Object.keys(set).length, 'icons to', dir);
  }
  await browser.close();
})().catch((e) => { console.error(e); process.exit(1); });

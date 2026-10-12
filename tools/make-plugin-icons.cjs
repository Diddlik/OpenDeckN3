// Renders the key images of the OBS, Home Assistant, mail & calendar, AudioSwitcher, HTTP, media, system monitor, mixer, VoiceMeeter, Twitch, YouTube and lights plugins (144×144 PNG).
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

// Line icons; Home Assistant uses HA blue / active yellow.
const icon = (svg, on, [bg, fg] = on ? ['#3a2f05', '#ffd34d'] : ['#0f2233', '#5bc4ff']) => {
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

// Mail & calendar: grey when idle, red with unread mail, yellow shortly before an appointment.
const IDLE = ['#1a1d24', '#c3cad5'];
const ENVELOPE = '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 7l9 6 9-6"/>';
const CALENDAR = '<rect x="3" y="5" width="18" height="16" rx="2"/><path d="M3 10h18M8 3v4M16 3v4"/><path d="M8 14h3v3H8z"/>';
const MAIL = {
  plugin: icon(ENVELOPE, false, IDLE),
  mail: icon(ENVELOPE, false, IDLE), 'mail-on': icon(ENVELOPE, true, ['#3a1012', '#ff6b6b']),
  calendar: icon(CALENDAR, false, IDLE), 'calendar-on': icon(CALENDAR, true),
};

// AudioSwitcher: speaker, lit in AudioSwitcher teal while the profile is active.
const SPEAKER = '<path d="M11 5L6 9H3v6h3l5 4z"/><path d="M15.5 8.5a5 5 0 0 1 0 7M18.5 5.5a9 9 0 0 1 0 13"/>';
const SWAP = '<path d="M4 8h13l-3-3M20 16H7l3 3"/>';
const AUDIO = {
  plugin: icon(SPEAKER, false, IDLE),
  profile: icon(SPEAKER, false, IDLE), 'profile-on': icon(SPEAKER, true, ['#06302c', '#4fe0c8']),
  toggle: icon(SWAP, false, IDLE),
};

// HTTP: globe for the plugin, arrow for a request, refresh for a polled value.
const GLOBE = '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>';
const ARROW = '<path d="M5 12h13M13 6l6 6-6 6"/>';
const REFRESH = '<path d="M20 12a8 8 0 1 1-2.3-5.7"/><path d="M20 4v5h-5"/>';
const HTTP = {
  plugin: icon(GLOBE, false),
  request: icon(ARROW, false),
  poll: icon(REFRESH, false),
};

// Media: disc for the plugin, play/pause (off = play symbol, on = pause symbol), skip arrows, note for now playing.
const DISC = '<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="2.5"/>';
const PLAY = '<path d="M8 5v14l11-7z"/>';
const PAUSE = '<path d="M8 5v14M16 5v14"/>';
const NEXT = '<path d="M5 5v14l9-7zM18 5v14"/>';
const PREV = '<path d="M19 5v14L10 12zM6 5v14"/>';
const NOTE = '<path d="M9 18V5l11-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="17" cy="16" r="3"/>';
const MEDIA = {
  plugin: icon(DISC, false),
  playpause: icon(PLAY, false),
  'playpause-on': icon(PAUSE, true),
  next: icon(NEXT, false),
  previous: icon(PREV, false),
  nowplaying: icon(NOTE, false),
};

// System monitor: pulse line for the plugin, bar chart for the value key.
const PULSE = '<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>';
const BARS = '<path d="M3 3v18h18"/><path d="M7 16v-4M12 16V8M17 16v-6"/>';
const SYSMON = {
  plugin: icon(PULSE, false),
  metric: icon(BARS, false),
};

// Mixer: sliders for the plugin, speaker and window for the app keys (filled = muted).
const SLIDERS = '<path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6"/>';
const MUTED_SPEAKER = '<path d="M11 5L6 9H3v6h3l5 4z"/><path d="M16 9l6 6M22 9l-6 6"/>';
const WINDOW = '<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8M12 17v4"/>';
const MIXER = {
  plugin: icon(SLIDERS, false),
  app: icon(SPEAKER, false),
  'app-on': icon(MUTED_SPEAKER, true),
  focused: icon(WINDOW, false),
  'focused-on': icon(WINDOW, true),
};

// VoiceMeeter: knob for the plugin, fader for levels, speaker for mute (red when muted), crossed lines for routing, prompt for scripts.
const KNOB = '<circle cx="12" cy="12" r="9"/><path d="M12 12l-3.5-3.5M12 7v1"/>';
const FADER = '<path d="M7 3v18M17 3v18"/><rect x="4" y="13" width="6" height="3" rx="1"/><rect x="14" y="7" width="6" height="3" rx="1"/>';
const ROUTE = '<circle cx="5" cy="6" r="2"/><circle cx="19" cy="18" r="2"/><path d="M7 6h5a5 5 0 0 1 5 5v5"/>';
const PROMPT = '<path d="M4 6l6 6-6 6M12 19h8"/>';
const VOICEMEETER = {
  plugin: icon(KNOB, false, IDLE),
  gain: icon(FADER, false, IDLE), 'gain-on': icon(FADER, true, ['#3a1012', '#ff6b6b']),
  mute: icon(SPEAKER, false, IDLE), 'mute-on': icon(MUTED_SPEAKER, true, ['#3a1012', '#ff6b6b']),
  route: icon(ROUTE, false, IDLE), 'route-on': icon(ROUTE, true, ['#06302c', '#4fe0c8']),
  script: icon(PROMPT, false, IDLE),
};

// Twitch: purple outline icons; the chat mode key fills in while its mode is on.
const TW = ['#1d1630', '#b58cff'];
const TW_ON = ['#4b2a8a', '#ffffff'];
const TV = '<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M9 21h6M12 17v4"/><path d="M10 8v5l4-2.5z"/>';
const FLAG = '<path d="M5 21V4"/><path d="M5 4h11l-2 4 2 4H5"/>';
const SCISSORS = '<circle cx="6" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M8.5 7.5L20 19M8.5 16.5L20 5"/>';
const BUBBLE = '<path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1.1-4.6A8 8 0 1 1 21 12z"/>';
const EYE = '<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>';
const MEGAPHONE = '<path d="M3 10v4h4l8 4V6L7 10z"/><path d="M19 9a4 4 0 0 1 0 6"/>';
const TWITCH = {
  plugin: icon(TV, false, TW),
  marker: icon(FLAG, false, TW),
  clip: icon(SCISSORS, false, TW),
  chat: icon(BUBBLE, false, TW), 'chat-on': icon(BUBBLE, true, TW_ON),
  viewers: icon(EYE, false, TW),
  ad: icon(MEGAPHONE, false, TW),
};

// YouTube: red outline icons; the status key fills in while a live stream runs.
const YT = ['#2a0d0d', '#ff4e45'];
const YT_ON = ['#c4161c', '#ffffff'];
const YT_PLAY = '<rect x="2" y="5" width="20" height="14" rx="4"/><path d="M10 9v6l5-3z"/>';
const BROADCAST = '<circle cx="12" cy="12" r="2"/><path d="M16.2 7.8a6 6 0 0 1 0 8.4M7.8 16.2a6 6 0 0 1 0-8.4M19.1 4.9a10 10 0 0 1 0 14.2M4.9 19.1a10 10 0 0 1 0-14.2"/>';
const USERS = '<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0"/><path d="M16 4.5a3.5 3.5 0 0 1 0 7M18.5 14.5a6.5 6.5 0 0 1 3 5.5"/>';
const YOUTUBE = {
  plugin: icon(YT_PLAY, false, YT),
  status: icon(BROADCAST, false, YT), 'status-on': icon(BROADCAST, true, YT_ON),
  viewers: icon(USERS, false, YT),
  chat: icon(BUBBLE, false, YT),
};

// Lights: bulb for the plugin, power symbol (filled = on), sun for brightness, thermometer for colour temperature, grid for scenes.
const LAMP = ['#241c08', '#f2b84b'];
const LAMP_ON = ['#5a4306', '#ffe066'];
const SUN = '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>';
const THERMO = '<path d="M14 4v10.54a4 4 0 1 1-4 0V4a2 2 0 0 1 4 0Z"/>';
const GRID = '<rect x="3" y="3" width="8" height="8" rx="1"/><rect x="13" y="3" width="8" height="8" rx="1"/><rect x="3" y="13" width="8" height="8" rx="1"/><rect x="13" y="13" width="8" height="8" rx="1"/>';
const LIGHTS = {
  plugin: icon(BULB, false, LAMP),
  power: icon(POWER, false, LAMP), 'power-on': icon(POWER, true, LAMP_ON),
  brightness: icon(SUN, false, LAMP), 'brightness-on': icon(SUN, true, LAMP_ON),
  temperature: icon(THERMO, false, LAMP),
  scene: icon(GRID, false, LAMP),
};

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 144, height: 144 } });
  for (const [dir, set] of [['plugins/obs/de.opendeckn3.obs.sdPlugin/imgs', OBS], ['plugins/homeassistant/de.opendeckn3.homeassistant.sdPlugin/imgs', HA],
    ['plugins/mailcalendar/de.opendeckn3.mailcalendar.sdPlugin/imgs', MAIL],
    ['plugins/audioswitcher/de.opendeckn3.audioswitcher.sdPlugin/imgs', AUDIO],
    ['plugins/http/de.opendeckn3.http.sdPlugin/imgs', HTTP],
    ['plugins/media/de.opendeckn3.media.sdPlugin/imgs', MEDIA],
    ['plugins/sysmon/de.opendeckn3.sysmon.sdPlugin/imgs', SYSMON],
    ['plugins/mixer/de.opendeckn3.mixer.sdPlugin/imgs', MIXER],
    ['plugins/voicemeeter/de.opendeckn3.voicemeeter.sdPlugin/imgs', VOICEMEETER],
    ['plugins/twitch/de.opendeckn3.twitch.sdPlugin/imgs', TWITCH],
    ['plugins/youtube/de.opendeckn3.youtube.sdPlugin/imgs', YOUTUBE],
    ['plugins/lights/de.opendeckn3.lights.sdPlugin/imgs', LIGHTS]]) {
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

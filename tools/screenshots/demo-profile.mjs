import { readFileSync } from "node:fs";
const icons = JSON.parse(readFileSync(process.argv[2], "utf8"));
const D = "virtual-n3", B = "opendeckn3.builtin", C = "de.opendeckn3.counter";
const ws = new WebSocket("ws://127.0.0.1:57131");
let id = 1; const pending = new Map();
ws.onmessage = (m) => { const x = JSON.parse(m.data); if (x.id && pending.has(x.id)) { pending.get(x.id)(x); pending.delete(x.id); } };
const call = (command, p = {}) => new Promise((res, rej) => { const i = id++; pending.set(i, (x) => x.ok ? res(x.result) : rej(new Error(command + ": " + x.error))); ws.send(JSON.stringify({ id: i, command, ...p })); });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const key = (position, plugin, action, extra = {}) => ({ controller: "Keypad", position, plugin, action, ...extra });
const enc = (position, plugin, action, extra = {}) => ({ controller: "Encoder", position, plugin, action, ...extra });
ws.onopen = async () => {
  try {
    await sleep(800);
    for (const p of ["gaming", "office", "streaming"]) await call("switchProfile", { device: D, profile: p });
    await call("deleteProfile", { device: D, profile: "default" }).catch(() => {});
    const slots = [
      key(0, C, C + ".count", { settings: { count: 12, hue: 170 } }),
      key(1, C, C + ".count", { settings: { count: 7, hue: 275 } }),
      key(2, B, B + ".brightness", { title: "Hell", image: icons.sun }),
      key(3, B, B + ".profile", { settings: { profile: "gaming" }, title: "Gaming", image: icons.gaming }),
      key(4, B, B + ".profile", { settings: { profile: "office" }, title: "Office", image: icons.office }),
      key(5, C, C + ".count", { settings: { count: 3, hue: 25 } }),
      key(6, B, B + ".profile", { settings: { profile: "gaming" } }),
      key(7, B, B + ".brightness"),
      key(8, C, C + ".count", { settings: { count: 0, hue: 120 } }),
      enc(0, C, C + ".count", { settings: { count: 4, hue: 200 } }),
      enc(1, B, B + ".brightness"),
      enc(2, C, C + ".count", { settings: { count: 9, hue: 320 } }),
    ];
    for (const s of slots) {
      const base = { device: D, controller: s.controller, position: s.position };
      await call("setAction", { ...base, plugin: s.plugin, action: s.action, settings: s.settings });
      if (s.title || s.image) await call("setActionAppearance", { ...base, title: s.title ?? null, image: s.image ?? null });
    }
    // settings arrive at the plugin via didReceiveSettings → it redraws the keys
    for (const s of slots.filter((s) => s.plugin === C)) await call("setActionSettings", { device: D, controller: s.controller, position: s.position, settings: s.settings });
    await call("setBrightness", { device: D, value: 80 });
    await sleep(800);
    console.log("populated");
    process.exit(0);
  } catch (e) { console.error(e.message); process.exit(1); }
};

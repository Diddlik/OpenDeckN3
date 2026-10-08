// Minimal OpenDeckN3 / Stream Deck SDK plugin without dependencies (Node >= 22).
//
// Started by the host as:
//   node plugin.js -port <p> -pluginUUID <uuid> -registerEvent registerPlugin -info <json>
"use strict";

const zlib = require("node:zlib");

const args = {};
for (let i = 2; i < process.argv.length; i += 2) {
  args[process.argv[i].replace(/^-/, "")] = process.argv[i + 1];
}

const ws = new WebSocket(`ws://127.0.0.1:${args.port}`);
const send = (msg) => ws.send(JSON.stringify(msg));
const log = (message) => send({ event: "logMessage", payload: { message } });

// Settings per context, mirrored from the host.
const contexts = new Map();

ws.addEventListener("open", () => {
  send({ event: args.registerEvent, uuid: args.pluginUUID });
  log("Zähler-Plugin verbunden");
});

ws.addEventListener("message", ({ data }) => {
  const msg = JSON.parse(data);
  const { event, context, payload } = msg;
  switch (event) {
    case "willAppear":
    case "didReceiveSettings":
      contexts.set(context, normalize(payload.settings));
      render(context);
      break;
    case "willDisappear":
      contexts.delete(context);
      break;
    case "keyDown":
    case "dialDown": {
      const s = contexts.get(context) ?? normalize(payload.settings);
      s.count += 1;
      update(context, s);
      break;
    }
    case "dialRotate": {
      const s = contexts.get(context) ?? normalize(payload.settings);
      s.hue = (s.hue + payload.ticks * 15 + 360) % 360;
      update(context, s);
      break;
    }
  }
});

ws.addEventListener("close", () => process.exit(0));

function normalize(settings) {
  return { count: Number(settings?.count ?? 0), hue: Number(settings?.hue ?? 200) };
}

function update(context, settings) {
  contexts.set(context, settings);
  send({ event: "setSettings", context, payload: settings });
  render(context);
}

function render(context) {
  const { count, hue } = contexts.get(context);
  const lightness = 0.25 + ((count % 5) / 5) * 0.5;
  send({ event: "setTitle", context, payload: { title: String(count) } });
  send({ event: "setImage", context, payload: { image: solidPng(72, hslToRgb(hue / 360, 0.7, lightness)) } });
}

// --- tiny PNG encoder -------------------------------------------------------

function solidPng(size, [r, g, b]) {
  const row = Buffer.alloc(1 + size * 3);
  for (let x = 0; x < size; x++) row.set([r, g, b], 1 + x * 3);
  const raw = Buffer.concat(Array.from({ length: size }, () => row));
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr.set([8, 2, 0, 0, 0], 8); // 8 bit, RGB
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
  return `data:image/png;base64,${png.toString("base64")}`;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

function crc32(buf) {
  let c = ~0;
  for (const byte of buf) {
    c ^= byte;
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}

function hslToRgb(h, s, l) {
  const f = (n) => {
    const k = (n + h * 12) % 12;
    const a = s * Math.min(l, 1 - l);
    return Math.round(255 * (l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1))));
  };
  return [f(0), f(8), f(4)];
}

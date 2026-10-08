// End-to-end smoke test against a running daemon (Node >= 22, no deps).
//
//   cargo run -p n3-daemon -- --virtual --no-hardware --plugins-dir plugins/examples --config-dir /tmp/n3cfg
//   node tools/smoke-test.mjs [ws://127.0.0.1:57131]
//
// Binds the example counter plugin to key 0 and the built-in brightness
// action to encoder 1 of the virtual device, simulates input and checks the
// results via the UI API.

const url = process.argv[2] ?? "ws://127.0.0.1:57131";
const DEVICE = "virtual-n3";
const ws = new WebSocket(url);
const pending = new Map();
const events = [];
let nextId = 1;

ws.addEventListener("message", ({ data }) => {
  const msg = JSON.parse(data);
  if (msg.id !== undefined && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.ok ? resolve(msg.result) : reject(new Error(msg.error));
  } else if (msg.event) {
    events.push(msg);
  }
});

const call = (command, params = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, command, ...params }));
  });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const assert = (cond, what) => {
  if (!cond) throw new Error(`FAILED: ${what}`);
  console.log(`ok - ${what}`);
};
// Polls getState until `check(device)` holds (plugins answer asynchronously).
const waitForDevice = async (check, what, timeoutMs = 5000) => {
  const until = Date.now() + timeoutMs;
  let dev;
  for (;;) {
    const state = await call("getState");
    dev = state.devices.find((d) => d.info.id === DEVICE);
    if (dev && check(dev)) return assert(true, what);
    if (Date.now() > until) break;
    await sleep(100);
  }
  const k0 = dev?.profile.keys["0"];
  throw new Error(`FAILED: ${what} (key 0: ${JSON.stringify({ settings: k0?.settings, title: dev?.titles["0"] })}, brightness: ${dev?.brightness})`);
};

ws.addEventListener("open", async () => {
  try {
    await sleep(1000); // give the plugin time to connect
    let state = await call("getState");
    assert(state.devices.some((d) => d.info.id === DEVICE), "virtual device present");
    const counter = state.catalog.find((p) => p.uuid === "de.opendeckn3.counter");
    assert(counter?.connected, "counter plugin connected");

    await call("setAction", { device: DEVICE, controller: "Keypad", position: 0,
      plugin: "de.opendeckn3.counter", action: "de.opendeckn3.counter.count" });
    await call("setAction", { device: DEVICE, controller: "Encoder", position: 1,
      plugin: "opendeckn3.builtin", action: "opendeckn3.builtin.brightness" });
    await call("setBrightness", { device: DEVICE, value: 50 });

    for (const type of ["keyDown", "keyUp", "keyDown", "keyUp"]) {
      await call("simulateInput", { device: DEVICE, input: { type, key: 0 } });
    }
    await call("simulateInput", { device: DEVICE, input: { type: "encoderTwist", encoder: 1, ticks: -2 } });

    await waitForDevice((d) => d.profile.keys["0"]?.settings.count === 2, "plugin persisted count=2 via setSettings");
    await waitForDevice((d) => d.previews["0"]?.startsWith("data:image/png"), "key 0 has a preview image");
    await waitForDevice((d) => d.titles["0"] === "2", "plugin title is 2");
    await waitForDevice((d) => d.brightness === 40, "encoder twist lowered brightness to 40");
    assert(events.some((e) => e.event === "keyImage"), "keyImage events were pushed");

    await call("switchProfile", { device: DEVICE, profile: "gaming" });
    state = await call("getState");
    assert(state.devices[0].profiles.includes("gaming"), "new profile created");
    await call("switchProfile", { device: DEVICE, profile: "default" });

    const started = await call("startVirtualDevice");
    assert(started.device === DEVICE, "startVirtualDevice is idempotent");

    const bad = await call("switchProfile", { device: DEVICE, profile: "../x" }).catch((e) => e);
    assert(bad instanceof Error, "invalid profile id rejected");
    console.log("all checks passed");
    process.exit(0);
  } catch (err) {
    console.error(err.message);
    process.exit(1);
  }
});
ws.addEventListener("error", () => {
  console.error(`cannot connect to ${url}`);
  process.exit(1);
});

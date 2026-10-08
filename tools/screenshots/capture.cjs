const { chromium } = require('playwright');
const out = process.argv[2], frames = process.argv[3];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  const browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1.5 });
  const page = await ctx.newPage();
  const errors = []; page.on('pageerror', (e) => errors.push(e.message));
  await page.addInitScript(() => { if (!sessionStorage.getItem('fresh')) { localStorage.setItem('odn3.onboarded', 'true'); } });
  await page.goto('http://127.0.0.1:57132/');
  await page.waitForSelector('#stage .device');
  await page.waitForTimeout(1500);
  // Test-Modus aus, damit Klicks nur auswählen
  await page.click('[data-act="test"]');
  await page.click('[data-slot="k0"]');
  await page.mouse.move(5, 5);
  await sleep(400);
  await page.screenshot({ path: out + '/editor-dark.png' });

  await page.click('[data-act="nav"][data-arg="plugins"]'); await sleep(300);
  await page.screenshot({ path: out + '/plugins.png' });

  await page.click('[data-act="nav"][data-arg="settings"]');
  await page.click('[data-act="theme"][data-arg="light"]');
  await page.click('[data-act="nav"][data-arg="editor"]'); await sleep(200);
  await page.click('[data-slot="k5"]'); await page.mouse.move(5, 5); await sleep(300);
  await page.screenshot({ path: out + '/editor-light.png' });
  await page.click('[data-act="nav"][data-arg="settings"]');
  await page.click('[data-act="theme"][data-arg="dark"]');
  await page.click('[data-act="nav"][data-arg="editor"]'); await sleep(200);

  // Animation: Test-Modus an, Tasten drücken, Regler drehen
  await page.click('[data-act="test"]'); await sleep(200);
  const clip = await page.locator('#stage').boundingBox();
  let n = 0;
  const snap = async (count = 1) => { for (let i = 0; i < count; i++) { await page.screenshot({ clip, path: `${frames}/f${String(n++).padStart(3, '0')}.png` }); } };
  await snap(4);
  for (let i = 0; i < 3; i++) { await page.click('[data-slot="k8"]'); await sleep(90); await snap(1); await sleep(160); await snap(2); }
  await page.click('[data-slot="k2"]'); await sleep(90); await snap(1); await sleep(400); await snap(2);
  const e1 = await page.$('[data-enc="1"]'); const b = await e1.boundingBox();
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
  for (let i = 0; i < 8; i++) { await page.mouse.wheel(0, 100); await sleep(150); await snap(1); }
  for (let i = 0; i < 8; i++) { await page.mouse.wheel(0, -100); await sleep(150); await snap(1); }
  await page.mouse.move(5, 5); await sleep(300); await snap(4);

  // Onboarding (frische Sitzung)
  await page.evaluate(() => { localStorage.removeItem('odn3.onboarded'); sessionStorage.setItem('fresh', '1'); });
  await page.reload(); await page.waitForSelector('text=Neun Tasten'); await sleep(1200);
  await page.screenshot({ path: out + '/onboarding.png' });
  await page.evaluate(() => localStorage.setItem('odn3.onboarded', 'true'));
  console.log('frames', n, 'errors', errors.length ? errors : 'none');
  await browser.close();
})().catch((e) => { console.error(e); process.exit(1); });

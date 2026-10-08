// Renders the README hero banner from hero.html + docs/images/editor-dark.png.
// Usage: NODE_PATH=$(npm root -g) node tools/screenshots/hero.cjs
const path = require('path');
const fs = require('fs');
const { chromium } = require('playwright');
const root = path.resolve(__dirname, '../..');
(async () => {
  const html = fs.readFileSync(path.join(__dirname, 'hero.html'), 'utf8')
    .replace('src="editor-dark.png"', `src="file://${path.join(root, 'docs/images/editor-dark.png')}"`);
  const tmp = path.join(require('os').tmpdir(), 'opendeckn3-hero.html');
  fs.writeFileSync(tmp, html);
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 }, deviceScaleFactor: 1.5 });
  await page.goto('file://' + tmp);
  await page.waitForTimeout(1500);
  await page.screenshot({ path: path.join(root, 'docs/images/hero.png') });
  await browser.close();
})();

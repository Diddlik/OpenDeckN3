// Run against a disposable virtual-device config: this changes idle settings.
const { chromium } = require('playwright');
(async()=>{
 const browser=await chromium.launch();
 const page=await browser.newPage({viewport:{width:1440,height:1000}});
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.addInitScript(port=>{localStorage.setItem('odn3.onboarded','true');localStorage.setItem('odn3.apiPort',JSON.stringify(port));}, Number(process.argv[3] || 57131));
 await page.goto(process.argv[2] || 'http://127.0.0.1:57132/');
 await page.locator('#stage .device').waitFor();
 await page.locator('[data-act="nav"][data-arg="settings"]').click();
 await page.locator('#idleMode:not([disabled])').waitFor();
 for(const mode of ['digital','analog','dim']) {
  await page.selectOption('#idleMode',mode);
  await page.waitForFunction(v=>document.querySelector('#idleMode').value===v,mode);
  await page.waitForTimeout(300);
  await page.reload();
  await page.locator('[data-act="nav"][data-arg="settings"]').click();
  await page.locator('#idleMode:not([disabled])').waitFor();
  if(await page.locator('#idleMode').inputValue()!==mode) throw Error('Mode not persisted: '+mode);
 }
 await page.selectOption('#idleMode','digital');
 await page.selectOption('#dimAfter','5');
 await page.waitForTimeout(300);
 await page.locator('#geraete').screenshot({path:process.argv[4] || 'docs/images/idle-settings.png'});
 if(errors.length)throw Error(errors.join('\n'));
 console.log('UI: all three modes persist after reload; timeout selectable; no JavaScript errors');
 await browser.close();
})().catch(e=>{console.error(e);process.exit(1)});

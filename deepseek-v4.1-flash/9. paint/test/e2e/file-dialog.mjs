/**
 * Native-dialog smoke check: proves the Open button really opens a file chooser
 * and that the chosen file loads (the e2e suite dispatches `change` directly, so
 * this covers the click -> chooser -> change path that only a real browser has).
 *
 * Usage: node test/e2e/file-dialog.mjs
 */
import { chromium } from 'playwright-core';
import { listen } from '../../tools/serve.mjs';

const server = await listen({ rootDir: '.', port: 0 });
const base = `http://127.0.0.1:${server.address().port}`;
let browser = null;
for (const attempt of [{ channel: 'chrome' }, { channel: 'msedge' }, {}]) {
  try { browser = await chromium.launch({ headless: true, ...attempt }); break; } catch { /* next */ }
}
if (!browser) {
  await new Promise((resolve) => server.close(resolve));
  process.stderr.write('No Chromium-family browser available.\n');
  process.exit(2);
}

let failed = false;
try {
  const page = await browser.newPage();
  await page.goto(`${base}/index.html`);
  const chooser = page.waitForEvent('filechooser');
  await page.click('#openButton');
  const fileChooser = await chooser;
  await fileChooser.setFiles('test/fixtures/alpha.png');
  await page.waitForFunction('window.__rasterEditor.hasImage === true', null, { timeout: 10000 });
  const state = await page.evaluate(`(() => {
    const ed = window.__rasterEditor;
    const p = ed.doc.sample(0, 0);
    return { width: ed.doc.width, height: ed.doc.height, transparent: p.a === 0,
      status: document.getElementById('statusText').textContent,
      dimensions: document.getElementById('statusDimensions').textContent };
  })()`);
  const ok = state.width === 4 && state.height === 4 && state.transparent && /Opened alpha\.png/.test(state.status);
  process.stdout.write(`${ok ? 'PASS' : 'FAIL'} file chooser -> load: ${JSON.stringify(state)}\n`);
  failed = !ok;
} catch (error) {
  process.stdout.write(`FAIL file chooser -> load: ${error.message}\n`);
  failed = true;
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
process.exit(failed ? 1 : 0);

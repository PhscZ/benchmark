#!/usr/bin/env node
/**
 * End-to-end runner.
 *
 * Serves the project, drives the harness page (test/e2e/harness.html) in a real
 * Chromium-family browser and reports the results. Uses playwright-core, which
 * does not download a browser: an installed Chrome, Edge or Chromium is reused.
 *
 * Usage:
 *   node test/e2e/runner.mjs [--only=<substring>] [--headed] [--browser-path=<exe>]
 *
 * Browser resolution order: --browser-path, E2E_BROWSER_PATH, channel "chrome",
 * channel "msedge".
 */
import { chromium } from 'playwright-core';
import { listen } from '../../tools/serve.mjs';

const args = process.argv.slice(2);
const option = (name) => args.find((arg) => arg.startsWith(`--${name}=`))?.split('=').slice(1).join('=');
const has = (name) => args.includes(`--${name}`);

const only = option('only');
const headed = has('headed');
const browserPath = option('browser-path') ?? process.env.E2E_BROWSER_PATH;
const timeoutMs = Number(option('timeout') ?? 180000);

const server = await listen({ rootDir: '.', port: 0 });
const base = `http://127.0.0.1:${server.address().port}`;

const attempts = browserPath
  ? [{ executablePath: browserPath }]
  : [{ channel: 'chrome' }, { channel: 'msedge' }, {}];

let browser = null;
let launchError = null;
for (const attempt of attempts) {
  try {
    browser = await chromium.launch({ headless: !headed, ...attempt });
    break;
  } catch (error) {
    launchError = error;
  }
}
if (!browser) {
  await new Promise((resolve) => server.close(resolve));
  process.stderr.write(
    'Could not launch a Chromium-family browser.\n'
    + 'Install Chrome or Edge, or point the runner at an executable:\n'
    + '  node test/e2e/runner.mjs --browser-path="/path/to/chrome"\n'
    + `Underlying error: ${launchError?.message ?? 'unknown'}\n`,
  );
  process.exit(2);
}

let exitCode = 0;
try {
  const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
  page.on('pageerror', (error) => process.stderr.write(`[page error] ${error.message}\n`));
  const url = `${base}/test/e2e/harness.html${only ? `?only=${encodeURIComponent(only)}` : ''}`;
  await page.goto(url);
  await page.waitForFunction('window.__e2eResult !== undefined', null, { timeout: timeoutMs });
  const result = await page.evaluate('window.__e2eResult');

  for (const failure of result.failures) {
    process.stdout.write(`FAIL ${failure.name}\n     ${failure.detail.split('\n')[0]}\n`);
  }
  process.stdout.write(`\nE2E: ${result.passed}/${result.total} passed${result.failed ? `, ${result.failed} failed` : ''}\n`);
  exitCode = result.failed ? 1 : 0;
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
process.exit(exitCode);

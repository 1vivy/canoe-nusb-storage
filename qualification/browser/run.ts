// Run from the repository root with Bun. Serves generated synthetic fixtures only.
import { chromium } from 'playwright-core';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const work = resolve(import.meta.dir, '../../.work/webapp-feasibility');
await mkdir(`${work}/results`, { recursive: true });
const headers = {
  'Cross-Origin-Opener-Policy': 'same-origin',
  'Cross-Origin-Embedder-Policy': 'require-corp',
  'Cache-Control': 'no-store',
};
const allow = new Set(['index.html', 'worker.js', 'fat.img', 'ext4.img', 'payload.bin',
  'pkg/canoe_browser_probe.js', 'pkg/canoe_browser_probe_bg.wasm']);
const server = Bun.serve({
  hostname: '127.0.0.1', port: 0,
  async fetch(request) {
    const name = new URL(request.url).pathname.slice(1) || 'index.html';
    if (request.method === 'PUT' && ['results/fat.img', 'results/ext4.img'].includes(name)) {
      const bytes = await request.arrayBuffer();
      const expected = name.endsWith('/fat.img') ? 32 * 1024 * 1024 : 128 * 1024 * 1024;
      if (bytes.byteLength !== expected) return new Response('unexpected fixture size', { status: 400 });
      await writeFile(`${work}/${name}`, new Uint8Array(bytes));
      return new Response('saved', { headers });
    }
    if (request.method !== 'GET' || !allow.has(name)) return new Response('not found', { status: 404 });
    return new Response(Bun.file(`${work}/public/${name}`), { headers });
  },
});
const browser = await chromium.launch({ headless: true });
const page = await browser.newPage();
const errors: string[] = [];
page.on('pageerror', error => errors.push(String(error)));
page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
const report: Record<string, unknown> = { browser: browser.version(), build: await Bun.file(`${work}/build.json`).json() };
let failed = false;
try {
  await page.goto(`http://127.0.0.1:${server.port}`);
  await page.waitForFunction(() => typeof (window as any).capabilities === 'function');
  report.capabilities = await page.evaluate(() => (window as any).capabilities());
  const cases: unknown[] = [];
  const nativeClock = (report.build as any).native_clock;
  const nativePid = (report.build as any).native_pid;
  const negativeBuild = nativeClock || nativePid;
  const probes = negativeBuild ? [['ext4', null]] : [
    ['fat', null], ['ext4', null], ['fat', 'short-read'], ['fat', 'disconnect'],
    ['fat', 'flush'], ['fat', 'timeout'], ['ext4', 'short-read'], ['ext4', 'flush'],
  ];
  for (const [filesystem, fault] of probes) {
    let watchdog: ReturnType<typeof setTimeout> | undefined;
    const result = await Promise.race([
      page.evaluate(async ({ filesystem, fault }) =>
        (window as any).runProbe(filesystem, fault), { filesystem, fault }),
      new Promise<never>((_, reject) => {
        watchdog = setTimeout(() => reject(new Error(`probe did not finish: ${filesystem}/${fault}`)), 60_000);
      }),
    ]).finally(() => clearTimeout(watchdog));
    const expectedOk = !negativeBuild && !fault;
    const preflightFault = fault === 'short-read' || fault === 'disconnect';
    const matched = result.ok === expectedOk
      && (expectedOk || Boolean(result.error))
      && (!fault || result.poisoned === true)
      && (!preflightFault || result.metrics.write === 0);
    failed ||= !matched;
    cases.push({ ...result, matchedExpectation: matched });
    console.log(JSON.stringify({ filesystem, fault, ok: result.ok, elapsedMs: result.elapsedMs, metrics: result.metrics }));
  }
  report.cases = cases;
  report.errors = errors;
  if (negativeBuild) {
    const expected = nativeClock ? 'time not implemented on this platform' : 'no pids on this platform';
    failed ||= !errors.some(error => error.includes(expected));
  } else failed ||= errors.length !== 0;
  report.matchedExpectations = !failed;
  await writeFile(`${work}/results/${nativeClock ? 'native-clock' : nativePid ? 'native-pid' : 'browser'}.json`, JSON.stringify(report, null, 2) + '\n');
} finally {
  await browser.close();
  server.stop(true);
}
if (failed) process.exitCode = 1;

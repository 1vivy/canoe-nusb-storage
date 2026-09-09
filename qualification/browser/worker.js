// A single-threaded WASM instance with private memory. Only the I/O mailbox is
// shared. The broker is in another context, so waiting cannot block its promises.
import init, { fat_probe, ext4_probe } from './pkg/canoe_browser_probe.js';
let control, bytes, poisoned = false;
function request(kind, offset, buffer) {
  if (poisoned) throw new Error('broker session is unusable');
  const length = buffer?.length ?? 0;
  if (length > bytes.length) throw new Error('request exceeds bounded mailbox');
  if (kind === 'write') bytes.set(buffer);
  Atomics.store(control, 0, 0);
  postMessage({ kind, offset, length });
  const deadline = performance.now() + 5000;
  while (Atomics.load(control, 0) === 0) {
    const remaining = deadline - performance.now();
    if (remaining <= 0 || Atomics.wait(control, 0, 0, remaining) === 'timed-out') {
      poisoned = true;
      throw new Error('broker timeout; session unusable');
    }
  }
  if (Atomics.load(control, 0) !== 1 || Atomics.load(control, 1) !== length) {
    poisoned = true;
    throw new Error(`broker ${kind} failed or returned a short transfer`);
  }
  if (kind === 'read') buffer.set(bytes.subarray(0, length));
}
globalThis.probeRead = (offset, out) => request('read', offset, out);
globalThis.probeWrite = (offset, data) => request('write', offset, data);
globalThis.probeFlush = () => request('flush', 0, null);
self.onmessage = async ({ data }) => {
  control = new Int32Array(data.mailbox, 0, 4);
  bytes = new Uint8Array(data.mailbox, 16);
  try {
    await init();
    const probe = data.filesystem === 'ext4' ? ext4_probe : fat_probe;
    probe(data.length, new Uint8Array(data.payload));
    postMessage({ kind: 'done', ok: true });
  } catch (error) {
    postMessage({ kind: 'done', ok: false, error: String(error), poisoned });
  }
};

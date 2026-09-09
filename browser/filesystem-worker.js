// WASM memory stays private. Only this bounded mailbox crosses contexts.
let control,
  bytes,
  token,
  capacity,
  ioTimeout,
  sequence = 0,
  poisoned = false,
  filesystem;
function retire(message) {
  poisoned = true;
  if (control) {
    Atomics.store(control, 0, -1);
    Atomics.notify(control, 0);
  }
  return new Error(message);
}
function request(kind, offset, buffer) {
  if (poisoned) throw new Error("filesystem broker retired");
  const length = buffer?.length ?? 0;
  if (
    !Number.isSafeInteger(offset) ||
    offset < 0 ||
    length > bytes.length ||
    offset + length > capacity
  )
    throw retire("invalid broker range");
  if (kind === "write") bytes.set(buffer);
  const id = ++sequence;
  Atomics.store(control, 1, id);
  Atomics.store(control, 2, 0);
  Atomics.store(control, 0, 0);
  postMessage({
    kind: "io",
    operation: kind,
    offset,
    length,
    sequence: id,
    token,
  });
  const deadline = performance.now() + ioTimeout;
  while (Atomics.load(control, 0) === 0) {
    const left = deadline - performance.now();
    if (left <= 0 || Atomics.wait(control, 0, 0, left) === "timed-out")
      throw retire("filesystem broker I/O timed out");
  }
  if (
    Atomics.load(control, 0) !== 1 ||
    Atomics.load(control, 1) !== id ||
    Atomics.load(control, 2) !== length
  )
    throw retire("filesystem broker failed or returned short I/O");
  if (kind === "read") buffer.set(bytes.subarray(0, length));
}
// Dependencies sometimes request more than one mailbox. Keep each USB range
// bounded without changing their block/filesystem logic.
function chunks(kind, offset, buffer) {
  for (let at = 0; at < buffer.length; at += bytes.length)
    request(kind, offset + at, buffer.subarray(at, at + bytes.length));
}
globalThis.canoeStorageRead = (offset, out) => chunks("read", offset, out);
globalThis.canoeStorageWrite = (offset, data) => chunks("write", offset, data);
globalThis.canoeStorageFlush = () => request("sync", 0, null);
const methods = new Set([
  "inspect",
  "stat",
  "list",
  "read",
  "createFile",
  "write",
  "truncate",
  "mkdir",
  "remove",
  "rename",
  "finish",
]);
self.onmessage = async ({ data }) => {
  if (data.kind === "initialize") {
    token = data.token;
    capacity = data.capacity;
    ioTimeout = data.ioTimeout;
    control = new Int32Array(data.mailbox, 0, 4);
    bytes = new Uint8Array(data.mailbox, 16);
    try {
      if (!self.crossOriginIsolated || typeof Atomics.wait !== "function")
        throw new Error("isolated worker with SharedArrayBuffer is required");
      const wasm = await import(data.moduleUrl);
      await wasm.default();
      filesystem = wasm.openFilesystem(
        data.filesystem,
        data.access === "read-write",
        BigInt(capacity),
      );
      postMessage({ kind: "ready", token });
    } catch (error) {
      poisoned = true;
      postMessage({ kind: "failed-open", token, error: String(error) });
    }
    return;
  }
  if (data.token !== token || data.kind !== "call") return;
  const { id, method, args } = data;
  if (poisoned || !filesystem) {
    postMessage({
      kind: "reply",
      id,
      token,
      ok: false,
      retired: true,
      error: "filesystem worker retired",
    });
    return;
  }
  try {
    if (!methods.has(method))
      throw new Error("unsupported filesystem operation");
    const value = filesystem[method](...args);
    const transfer = value instanceof Uint8Array ? [value.buffer] : [];
    postMessage(
      {
        kind: "reply",
        id,
        token,
        ok: true,
        value,
        finished: method === "finish",
      },
      transfer,
    );
    if (method === "finish") {
      filesystem.free();
      filesystem = null;
    }
  } catch (error) {
    const retired = poisoned || !filesystem.usable();
    if (retired) {
      poisoned = true;
      filesystem.abort();
    }
    postMessage({
      kind: "reply",
      id,
      token,
      ok: false,
      retired,
      error: String(error),
    });
  }
};

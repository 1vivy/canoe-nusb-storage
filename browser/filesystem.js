// Production asynchronous broker. No filesystem or USB protocol is reimplemented.
export const FILESYSTEM_API_VERSION = 2;
const owners = new WeakSet();
const consumed = new WeakSet();
const MAX_CHUNK = 4 * 1024 * 1024;
const mutations = new Set([
  "createFile",
  "write",
  "truncate",
  "mkdir",
  "remove",
  "rename",
]);
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
};
function positive(value, name, maximum) {
  if (!Number.isSafeInteger(value) || value <= 0 || value > maximum)
    throw new Error(`invalid ${name}`);
  return value;
}

/** Own one transport and one filesystem worker until finish/abort. The app must
 * confirm its independent backup BEFORE choosing read-write here:
 * an ext4 writable mount may replay its journal before the first file call. */
export async function openFilesystem({
  storage,
  kind,
  access = "read-only",
  workerUrl = new URL("./filesystem-worker.js", import.meta.url),
  moduleUrl = new URL("./pkg/canoe_fs.js", import.meta.url),
  mailboxBytes = 1024 * 1024,
  ioTimeout = 30000,
  operationTimeout = 120000,
}) {
  if (
    !globalThis.crossOriginIsolated ||
    typeof SharedArrayBuffer === "undefined"
  )
    throw new Error("COOP/COEP and SharedArrayBuffer are required");
  if (
    !["fat", "ext4"].includes(kind) ||
    !["read-only", "read-write"].includes(access)
  )
    throw new Error("invalid filesystem or access mode");
  if (!storage?.usable() || owners.has(storage) || consumed.has(storage))
    throw new Error(
      "storage is unavailable, already owned, or requires a fresh connection",
    );
  if (access === "read-write" && storage.access() !== "read-write")
    throw new Error("storage was opened read-only");
  positive(mailboxBytes, "mailbox size", MAX_CHUNK);
  positive(ioTimeout, "I/O timeout", 120000);
  positive(operationTimeout, "operation timeout", 300000);
  const capacity = positive(
    storage.capacity().bytes,
    "storage capacity",
    Number.MAX_SAFE_INTEGER,
  );
  // Retain cancellation independently of an async WASM session borrow.
  const usbDevice = storage.usbDevice?.();
  const abortPending =
    storage.abortPending?.bind(storage) ??
    (usbDevice ? () => usbDevice.close() : () => storage.close());
  owners.add(storage);
  const token = crypto.randomUUID(),
    mailbox = new SharedArrayBuffer(16 + mailboxBytes),
    control = new Int32Array(mailbox, 0, 4),
    bytes = new Uint8Array(mailbox, 16);
  let worker;
  try {
    worker = new Worker(workerUrl, { type: "module" });
  } catch (error) {
    owners.delete(storage);
    throw error;
  }
  const ready = deferred();
  let running = true,
    finished = false,
    ioBusy = false,
    activeIO = null,
    nextSequence = 1,
    nextCall = 0,
    queue = Promise.resolve(),
    shutdownPromise;
  const calls = new Map();
  function rejectAll(error) {
    ready.reject(error);
    for (const call of calls.values()) {
      clearTimeout(call.timer);
      call.reject(error);
    }
    calls.clear();
  }
  async function deadline(promise, label) {
    let timer;
    try {
      return await Promise.race([
        promise,
        new Promise((_, reject) => {
          timer = setTimeout(
            () => reject(new Error(`${label} timed out`)),
            5000,
          );
        }),
      ]);
    } finally {
      clearTimeout(timer);
    }
  }
  async function shutdown(reason, closeStorage) {
    if (shutdownPromise) return shutdownPromise;
    running = false;
    Atomics.store(control, 0, -1);
    Atomics.notify(control, 0);
    worker.terminate();
    if (closeStorage) consumed.add(storage);
    shutdownPromise = (async () => {
      let failure = reason;
      try {
        if (closeStorage) {
          if (activeIO) {
            const pending = activeIO;
            await deadline(abortPending(), "pending storage abort");
            await deadline(pending, "pending storage settlement");
          }
          await deadline(storage.close(), "storage close");
        }
      } catch (error) {
        failure = new Error(
          `${reason ?? "filesystem stopped"}; storage close failed: ${error}`,
        );
      } finally {
        owners.delete(storage);
        if (failure) rejectAll(failure);
      }
      if (failure) throw failure;
    })();
    // Event handlers also observe this rejection; prevent an unhandled promise
    // while retaining the original promise for the operation caller.
    shutdownPromise.catch(() => {});
    return shutdownPromise;
  }
  async function fail(error) {
    try {
      await shutdown(error, true);
    } catch {
      /* propagated to pending calls */
    }
  }
  async function io(message) {
    if (!running) return;
    if (
      ioBusy ||
      message.sequence !== nextSequence++ ||
      Atomics.load(control, 1) !== message.sequence ||
      Atomics.load(control, 0) !== 0
    )
      return fail(new Error("filesystem broker sequence mismatch"));
    ioBusy = true;
    try {
      const { operation, offset, length, sequence } = message;
      if (
        !Number.isSafeInteger(offset) ||
        offset < 0 ||
        !Number.isSafeInteger(length) ||
        length < 0 ||
        length > bytes.length ||
        offset + length > capacity
      )
        throw new Error("filesystem broker range invalid");
      if (operation === "read") {
        const result = await storage.readRange(BigInt(offset), length);
        if (!(result instanceof Uint8Array) || result.length !== length)
          throw new Error("short filesystem storage read");
        if (
          !running ||
          Atomics.load(control, 1) !== sequence ||
          Atomics.load(control, 0) !== 0
        )
          throw new Error("late filesystem read completion");
        bytes.set(result);
      } else if (operation === "write") {
        if (access !== "read-write")
          throw new Error("read-only filesystem attempted a write");
        await storage.writeRange(BigInt(offset), bytes.slice(0, length));
      } else if (operation === "sync") {
        if (length !== 0 || offset !== 0)
          throw new Error("invalid sync request");
        await storage.sync();
      } else throw new Error("unknown filesystem broker operation");
      if (
        !running ||
        Atomics.load(control, 1) !== sequence ||
        Atomics.load(control, 0) !== 0
      )
        throw new Error("late filesystem I/O completion");
      Atomics.store(control, 2, length);
      Atomics.store(control, 0, 1);
      Atomics.notify(control, 0);
    } catch (error) {
      // Teardown waits for this I/O to settle; never await teardown from here.
      void fail(error);
    } finally {
      ioBusy = false;
    }
  }
  worker.onmessage = ({ data }) => {
    if (data.token !== token || !running) return;
    if (data.kind === "io") {
      const pending = io(data);
      activeIO = pending;
      void pending.finally(() => {
        if (activeIO === pending) activeIO = null;
      });
      return;
    }
    if (data.kind === "ready") {
      if (data.apiVersion !== FILESYSTEM_API_VERSION) {
        void fail(new Error("filesystem worker API is incompatible; reload the application"));
        return;
      }
      ready.resolve();
      return;
    }
    if (data.kind === "failed-open") {
      void fail(new Error(data.error));
      return;
    }
    if (data.kind !== "reply") return;
    const call = calls.get(data.id);
    if (!call) return;
    if (data.retired) {
      void fail(new Error(data.error));
      return;
    }
    calls.delete(data.id);
    clearTimeout(call.timer);
    if (data.ok) {
      if (data.finished) {
        finished = true;
        void shutdown(null, false).then(
          () => call.resolve(data.value),
          call.reject,
        );
      } else call.resolve(data.value);
    } else call.reject(new Error(data.error));
  };
  worker.onerror = (event) => {
    event.preventDefault();
    void fail(new Error(event.message || "filesystem worker failed"));
  };
  const openTimer = setTimeout(
    () => void fail(new Error("filesystem mount timed out")),
    operationTimeout,
  );
  worker.postMessage({
    kind: "initialize-v2",
    token,
    mailbox,
    capacity,
    ioTimeout,
    filesystem: kind,
    access,
    moduleUrl: String(moduleUrl),
  });
  try {
    await ready.promise;
  } finally {
    clearTimeout(openTimer);
  }
  function call(method, args) {
    if (!running || finished)
      return Promise.reject(new Error("filesystem session is closed"));
    if (mutations.has(method) && access !== "read-write")
      return Promise.reject(new Error("filesystem was opened read-only"));
    const task = () => {
      if (!running || finished) throw new Error("filesystem session is closed");
      const item = deferred(),
        id = ++nextCall;
      item.timer = setTimeout(
        () => void fail(new Error(`filesystem ${method} timed out`)),
        operationTimeout,
      );
      calls.set(id, item);
      try {
        worker.postMessage({ kind: "call", token, id, method, args });
      } catch (error) {
        void fail(error);
      }
      return item.promise;
    };
    const result = queue.then(task);
    queue = result.catch(() => {});
    return result;
  }
  return Object.freeze({
    apiVersion: FILESYSTEM_API_VERSION,
    access: () => access,
    usable: () => running && !finished,
    inspect: () => call("inspect", []),
    stat: (path) => call("stat", [path]),
    list: (path = "/") => call("list", [path]),
    read: (path, offset = 0n, length = MAX_CHUNK) => {
      if (!Number.isSafeInteger(length) || length < 0 || length > MAX_CHUNK)
        return Promise.reject(new Error("invalid file read length"));
      return call("read", [path, BigInt(offset), length]);
    },
    createFile: (path) => call("createFile", [path]),
    write: (path, offset, data) => {
      if (!(data instanceof Uint8Array) || data.length > MAX_CHUNK)
        return Promise.reject(new Error("invalid file write bytes"));
      return call("write", [path, BigInt(offset), data.slice()]);
    },
    truncate: (path, length) => call("truncate", [path, BigInt(length)]),
    mkdir: (path) => call("mkdir", [path]),
    remove: (path) => call("remove", [path]),
    rename: (source, destination) => call("rename", [source, destination]),
    flush: () => call("flush", []),
    freshRead: () => call("freshRead", []),
    finish: () => call("finish", []),
    abort: () => {
      // A failed operation already rejects its caller with the primary error
      // and any close failure. Cleanup is idempotent: do not report that same
      // rejection again as if abort had performed another failing operation.
      if (shutdownPromise) return shutdownPromise.catch(() => {});
      const reason = new Error("filesystem aborted; reopen storage");
      return shutdown(reason, true).catch((error) => {
        if (error !== reason) throw error;
      });
    },
  });
}

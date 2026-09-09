import { chromium } from "playwright-core";
import { resolve } from "node:path";
import { mkdir, writeFile } from "node:fs/promises";
const root = resolve(import.meta.dir, "../.."),
  work = `${root}/.work/browser-storage`;
await mkdir(`${work}/results`, { recursive: true });
const headers = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
  "Cache-Control": "no-store",
};
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  async fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === "/")
      return new Response(
        "<!doctype html><title>Filesystem worker qualification</title>",
        { headers: { ...headers, "Content-Type": "text/html" } },
      );
    if (path === "/fake-device.js" && request.method === "GET")
      return new Response(
        Bun.file(`${root}/qualification/managed-browser/fake-device.js`),
        { headers },
      );
    if (
      request.method === "PUT" &&
      [
        "/results/fat.img",
        "/results/ext4.img",
        "/results/recovered.img",
      ].includes(path)
    ) {
      const bytes = new Uint8Array(await request.arrayBuffer());
      if (bytes.length !== 32 * 1024 * 1024)
        return new Response("size", { status: 400 });
      await writeFile(`${work}${path}`, bytes);
      return new Response("saved", { headers });
    }
    if (
      request.method !== "GET" ||
      path.includes("..") ||
      !/^\/(pkg\/[a-zA-Z0-9_./-]+|filesystem(?:-worker)?\.js|fixtures\/(fat|ext4|pending)\.img)$/.test(
        path,
      )
    )
      return new Response("not found", { status: 404 });
    return new Response(Bun.file(`${work}${path}`), { headers });
  },
});
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${server.port}`);
  const report = await page.evaluate(async () => {
    const { openFilesystem } = await import("/filesystem.js");
    const assert = (ok, message) => {
      if (!ok) throw new Error(message);
    };
    class Memory {
      constructor(bytes, access = "read-write") {
        this.bytes = bytes;
        this.mode = access;
        this.open = true;
        this.writes = 0;
        this.closed = 0;
        this.fault = null;
        this.pendingReject = null;
      }
      usable() {
        return this.open;
      }
      access() {
        return this.mode;
      }
      capacity() {
        return { bytes: this.bytes.length };
      }
      async readRange(offset, length) {
        if (this.fault === "timeout")
          return new Promise((_, reject) => {
            this.pendingReject = reject;
          });
        const out = this.bytes.slice(Number(offset), Number(offset) + length);
        return this.fault === "short-read" ? out.slice(0, -1) : out;
      }
      async writeRange(offset, bytes) {
        assert(this.mode === "read-write" && this.open, "invalid write");
        this.writes++;
        if (this.fault === "write") throw new Error("injected write");
        this.bytes.set(bytes, Number(offset));
      }
      async sync() {
        if (this.fault === "flush") throw new Error("injected flush");
      }
      async abortPending() {
        this.pendingReject?.(new Error("pending read aborted"));
        this.pendingReject = null;
      }
      async close() {
        assert(!this.pendingReject, "close raced pending request");
        this.open = false;
        this.closed++;
      }
    }
    const cases = [];
    let ticks = 0;
    const heartbeat = setInterval(() => ticks++, 2);
    try {
      for (const kind of ["fat", "ext4"]) {
        const bytes = new Uint8Array(
          await (await fetch(`/fixtures/${kind}.img`)).arrayBuffer(),
        );
        const storage = new Memory(bytes);
        const fs = await openFilesystem({ storage, kind });
        const original = bytes.slice();
        await fs.inspect();
        await fs.list("/");
        for (const action of [
          () => fs.createFile("/x"),
          () => fs.write("/x", 0n, Uint8Array.of(1)),
          () => fs.truncate("/x", 0n),
          () => fs.mkdir("/x"),
          () => fs.remove("/x"),
          () => fs.rename("/x", "/y"),
        ]) {
          let failed = false;
          try {
            await action();
          } catch {
            failed = true;
          }
          assert(failed, "read-only mutation admitted");
        }
        await fs.finish();
        assert(
          storage.writes === 0 &&
            bytes.every((byte, n) => byte === original[n]),
          "read-only mount altered image",
        );
        const rwStorage = new Memory(bytes),
          rw = await openFilesystem({
            storage: rwStorage,
            kind,
            access: "read-write",
          });
        let duplicate = false;
        try {
          await openFilesystem({ storage: rwStorage, kind });
        } catch {
          duplicate = true;
        }
        assert(duplicate, "exclusive broker");
        const info = await rw.inspect();
        assert(info.freeBytes > 16 * 1024 * 1024, "free space");
        await rw.createFile("/unrelated");
        await rw.write("/unrelated", 0n, new TextEncoder().encode("preserve"));
        await rw.mkdir("/managed");
        await rw.createFile("/managed/stage");
        const payload = Uint8Array.from({ length: 40000 }, (_, n) => n % 251);
        await rw.write("/managed/stage", 0n, payload);
        await rw.rename("/managed/stage", "/managed/result");
        await rw.createFile("/temporary");
        await rw.remove("/temporary");
        await rw.finish();
        const fresh = await openFilesystem({
          storage: new Memory(bytes, "read-only"),
          kind,
        });
        assert(
          (await fresh.read("/managed/result", 0n, payload.length)).every(
            (byte, n) => byte === payload[n],
          ),
          "fresh payload",
        );
        assert(
          new TextDecoder().decode(await fresh.read("/unrelated", 0n, 8)) ===
            "preserve",
          "unrelated bytes",
        );
        assert(
          (await fresh.list("/managed")).some(
            (entry) => entry.name === "result",
          ),
          "list",
        );
        await fresh.finish();
        await fetch(`/results/${kind}.img`, { method: "PUT", body: bytes });
        cases.push({ name: `${kind}-operations-fresh-readback`, ok: true });
      }
      const pending = new Uint8Array(
        await (await fetch("/fixtures/pending.img")).arrayBuffer(),
      );
      const ro = new Memory(pending, "read-only"),
        inspect = await openFilesystem({ storage: ro, kind: "ext4" });
      assert((await inspect.inspect()).needsRecovery, "pending evidence");
      await inspect.finish();
      assert(ro.writes === 0, "read-only replayed");
      const recovery = await openFilesystem({
        storage: new Memory(pending),
        kind: "ext4",
        access: "read-write",
      });
      await recovery.finish();
      const recovered = await openFilesystem({
        storage: new Memory(pending, "read-only"),
        kind: "ext4",
      });
      assert(!(await recovered.inspect()).needsRecovery, "recovery marker");
      await recovered.finish();
      await fetch("/results/recovered.img", { method: "PUT", body: pending });
      cases.push({ name: "plain-jbd2-committed-recovery", ok: true });
      for (const kind of ["fat", "ext4"])
        for (const fault of ["short-read", "timeout", "write", "flush"]) {
          const bytes = new Uint8Array(
              await (await fetch(`/fixtures/${kind}.img`)).arrayBuffer(),
            ),
            storage = new Memory(bytes);
          let failed = false;
          try {
            if (fault === "short-read" || fault === "timeout") {
              storage.fault = fault;
              await openFilesystem({
                storage,
                kind,
                access: "read-write",
                ioTimeout: 50,
                operationTimeout: 5000,
              });
            } else {
              const fs = await openFilesystem({
                storage,
                kind,
                access: "read-write",
              });
              await fs.createFile("/stage");
              storage.fault = fault;
              if (fault === "write")
                await fs.write("/stage", 0n, Uint8Array.of(9));
              else await fs.finish();
            }
          } catch {
            failed = true;
          }
          assert(
            failed && !storage.open && storage.closed > 0,
            `${kind}/${fault}: failed session did not close`,
          );
          cases.push({ name: `${kind}-${fault}`, ok: true });
        }
      // Full production composition: real WASM nusb/BOT/range code + worker
      // filesystem, with only the WebUSB device replaced by a synthetic target.
      const { FakeDevice } = await import("/fake-device.js");
      const usb = await import("/pkg/canoe_usb_web.js");
      await usb.default();
      for (const kind of ["fat", "ext4"]) {
        const image = new Uint8Array(
          await (await fetch(`/fixtures/${kind}.img`)).arrayBuffer(),
        );
        const device = new FakeDevice(null, image);
        const session = await usb.openManagedStorage(device, "read-write");
        const fs = await openFilesystem({
          storage: session,
          kind,
          access: "read-write",
        });
        await fs.createFile("/usb-composition");
        await fs.write("/usb-composition", 0n, Uint8Array.of(4, 2, 9));
        await fs.finish();
        await session.close();
        const reopened = await usb.openManagedStorage(device, "read-only");
        const verify = await openFilesystem({ storage: reopened, kind });
        assert(
          String(await verify.read("/usb-composition", 0n, 3)) === "4,2,9",
          "full USB composition readback",
        );
        await verify.finish();
        await reopened.close();
        cases.push({ name: `${kind}-nusb-scsi-worker-composition`, ok: true });
      }
      {
        const device = new FakeDevice(
          null,
          new Uint8Array(
            await (await fetch("/fixtures/fat.img")).arrayBuffer(),
          ),
        );
        const session = await usb.openManagedStorage(device, "read-only");
        device.fault = "read-timeout";
        let failed = false;
        try {
          await openFilesystem({
            storage: session,
            kind: "fat",
            ioTimeout: 50,
            operationTimeout: 5000,
          });
        } catch {
          failed = true;
        }
        assert(
          failed &&
            !device.opened &&
            !session.usable() &&
            device.pendingReads.size === 0,
          "pending WASM USB borrow did not drain",
        );
        cases.push({
          name: "nusb-pending-read-timeout-drains-before-close",
          ok: true,
        });
      }
      assert(ticks > 0, "broker blocked main thread");
      return { cases, heartbeatTicks: ticks, hardwareAccess: false };
    } finally {
      clearInterval(heartbeat);
    }
  });
  const wasmSha256 = new Bun.CryptoHasher("sha256")
    .update(await Bun.file(`${work}/pkg/canoe_fs_bg.wasm`).arrayBuffer())
    .digest("hex");
  await writeFile(
    `${work}/filesystem-browser.json`,
    JSON.stringify(
      { browser: browser.version(), wasmSha256, ...report },
      null,
      2,
    ) + "\n",
  );
  console.log(JSON.stringify(report));
} finally {
  await browser.close();
  server.stop(true);
}
for (const [kind, command, args] of [
  ["fat", "fsck.fat", ["-n"]],
  ["ext4", "e2fsck", ["-fn"]],
  ["recovered", "e2fsck", ["-fn"]],
] as const) {
  const process = Bun.spawnSync([
    command,
    ...args,
    `${work}/results/${kind}.img`,
  ]);
  await writeFile(
    `${work}/results/${kind}-oracle.log`,
    process.stdout.toString() + process.stderr.toString(),
  );
  if (process.exitCode !== 0)
    throw new Error(`${kind} independent filesystem check failed`);
}
const recovered = Bun.spawnSync([
  "debugfs",
  "-R",
  "stat /oracle",
  `${work}/results/recovered.img`,
]);
if (!recovered.stdout.toString().includes("0600"))
  throw new Error("committed inode update differs from oracle");

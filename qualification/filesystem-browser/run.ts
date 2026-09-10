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
    if (path === "/legacy-fs.js" && request.method === "GET")
      return new Response('export default async function(){}; export function openFilesystem(){throw new Error("legacy mount was executed")}', {headers:{...headers,"Content-Type":"text/javascript"}});
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
      !/^\/(pkg\/[a-zA-Z0-9_./-]+|filesystem(?:-worker)?\.js|fixtures\/(fat|ext4|pending|nested)\.img)$/.test(
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
        this.readBytes = 0;
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
        this.readBytes += length;
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
        if (this.fault === "write" || this.fault === "write-and-close") throw new Error("injected write");
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
        if (this.fault === "write-and-close") throw new Error("injected close");
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
        await fs.flush(); await fs.freshRead();
        await fs.finish();
        assert(
          storage.writes === 0 &&
            bytes.every((byte, n) => byte === original[n]),
          "read-only mount altered image",
        );
        assert(storage.usable() && storage.closed === 0, "clean finish consumed transport");
        const rwStorage = storage,
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
        if (kind === "ext4") {
          const entry = await rw.stat("/unrelated");
          assert(entry.inode > 2 && entry.generation > 0 && info.uuid.length === 16, "ext4 incarnation metadata");
        }
        await rw.mkdir("/managed");
        await rw.createFile("/managed/stage");
        const payload = Uint8Array.from({ length: 40000 }, (_, n) => n % 251);
        await rw.write("/managed/stage", 0n, payload);
        const writesBeforeNoop = rwStorage.writes;
        await rw.truncate("/managed/stage", BigInt(payload.length));
        assert(rwStorage.writes === writesBeforeNoop, "equal-size truncate wrote storage");
        if (kind === "ext4") {
          // Interleaved allocations produce a dense multi-level extent tree,
          // matching a large efisp.fat created on fragmented Android persist.
          await rw.createFile("/fragmented");
          await rw.createFile("/spacing");
          for (let n = 0; n < 32; n++) {
            await rw.write("/fragmented", BigInt(n * 4096), new Uint8Array(4096).fill(n));
            await rw.write("/spacing", BigInt(n * 4096), new Uint8Array(4096).fill(0x5a));
          }
          const beforeNoop = rwStorage.writes;
          await rw.truncate("/fragmented", 32n * 4096n);
          assert(rwStorage.writes === beforeNoop, "fragmented equal-size truncate wrote storage");
          await rw.truncate("/fragmented", 17n * 4096n + 19n);
          assert((await rw.read("/fragmented", 17n * 4096n, 19)).every(n => n === 17), "deep shrink lost retained bytes");
          await rw.remove("/fragmented");
          assert((await rw.read("/spacing", 0n, 4096)).every(n => n === 0x5a), "deep removal changed unrelated bytes");
          cases.push({ name: "fragmented-ext4-shrink-and-remove", ok: true });
        }
        await rw.rename("/managed/stage", "/managed/result");
        await rw.createFile("/temporary");
        await rw.remove("/temporary");
        await rw.flush(); await rw.freshRead();
        assert(rw.usable() && rwStorage.closed === 0, "flush released the mount");
        assert((await rw.read("/managed/result",0n,payload.length)).every((v,n)=>v===payload[n]), "live flush readback");
        // Change only a physical data byte after it has been read into any
        // driver cache. Explicit freshRead must observe it without remounting.
        const changedOffsets=[];
        for(let at=0;at+64<bytes.length;at++) {
          if(bytes[at]===payload[0] && payload.subarray(0,64).every((v,n)=>bytes[at+n]===v)) {
            changedOffsets.push(at); bytes[at]^=0xff; at+=63;
          }
        }
        assert(changedOffsets.length>0,"physical fixture payload absent");
        await rw.freshRead();
        const changed=await rw.read("/managed/result",0n,payload.length);
        assert(changed.some((v,n)=>v!==payload[n]),`${kind}: freshRead returned cached write bytes (${changedOffsets.length} physical payload locations)`);
        for(const at of changedOffsets)bytes[at]^=0xff;
        await rw.write("/managed/result",0n,payload);
        await rw.freshRead();
        assert((await rw.read("/managed/result",0n,payload.length)).every((v,n)=>v===payload[n]),"second live write readback");
        assert(rwStorage.closed===0,"live validation closed transport");
        cases.push({name:`${kind}-persistent-mount-and-physical-readback`,ok:true});
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
      {
        const backing = new Memory(new Uint8Array(await(await fetch('/fixtures/nested.img')).arrayBuffer()));
        const parent=await openFilesystem({storage:backing,kind:'ext4'});
        const entry=await parent.stat('/efisp.fat');
        const readsBefore=backing.readBytes;
        const borrowed={
          usable:()=>parent.usable(), access:()=> 'read-only', capacity:()=>({bytes:entry.size}),
          readRange:(offset,length)=>parent.read('/efisp.fat',offset,length),
          writeRange:async()=>{throw new Error('borrowed container is read-only')},
          sync:()=>parent.flush(), close:async()=>{},
        };
        const child=await openFilesystem({storage:borrowed,kind:'fat'});
        assert((await child.inspect()).kind==='fat','nested FAT inspection');
        await child.list('/'); await child.freshRead(); await child.finish();
        assert(parent.usable()&&backing.closed===0&&backing.writes===0,'nested read released or mutated persist');
        assert(backing.readBytes-readsBefore<4*1024*1024,'nested inspection fetched whole FAT container');
        assert((await parent.stat('/efisp.fat')).size===8*1024*1024,'parent remains usable');
        await parent.finish();
        cases.push({name:'range-backed-FAT-preflight-retains-parent-mount',ok:true,readBytes:backing.readBytes-readsBefore});
      }
      const obsoleteStorage=new Memory(new Uint8Array(await(await fetch('/fixtures/fat.img')).arrayBuffer()));
      let incompatible='';
      try { await openFilesystem({storage:obsoleteStorage,kind:'fat',access:'read-write',moduleUrl:new URL('/legacy-fs.js',location.href)}); }
      catch(error){incompatible=String(error)}
      assert(incompatible.includes('API is incompatible')&&!incompatible.includes('legacy mount was executed')&&obsoleteStorage.writes===0,'old engine mounted before capability rejection');
      cases.push({name:'obsolete-engine-rejected-before-mount',ok:true});
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
        for (const fault of ["short-read", "timeout", "write", "flush", "write-and-close"]) {
          const bytes = new Uint8Array(
              await (await fetch(`/fixtures/${kind}.img`)).arrayBuffer(),
            ),
            storage = new Memory(bytes);
          let failed = false, handle;
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
              handle = fs;
              await fs.createFile("/stage");
              storage.fault = fault;
              if (fault === "write" || fault === "write-and-close")
                await fs.write("/stage", 0n, Uint8Array.of(9));
              else await fs.finish();
            }
          } catch (error) {
            failed = true;
            if (fault === "write-and-close") assert(String(error).includes("injected close"), "lost cleanup error");
          }
          // Releasing an already-retired filesystem must not repeat its
          // original operation rejection as a second cleanup failure.
          if (handle) await handle.abort();
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

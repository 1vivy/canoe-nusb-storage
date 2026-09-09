import {chromium} from 'playwright-core';
import {mkdir,writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
const root=resolve(import.meta.dir,'../..');
const server=Bun.serve({hostname:'127.0.0.1',port:0,fetch(request){
  const path=new URL(request.url).pathname;
  if(path==='/')return new Response('<!doctype html><title>Managed USB synthetic qualification</title>',{headers:{'Content-Type':'text/html'}});
  if(path==='/fake-device.js')return new Response(Bun.file(`${import.meta.dir}/fake-device.js`));
  if(!/^\/pkg\/[a-zA-Z0-9_./-]+$/.test(path)||path.includes('..'))return new Response('not found',{status:404});
  return new Response(Bun.file(`${root}/.work/browser-storage${path}`));
}});
const browser=await chromium.launch({headless:true});
try{
  const page=await browser.newPage();
  await page.goto(`http://127.0.0.1:${server.port}`);
  const report=await page.evaluate(async()=>{
    const wasm=await import('/pkg/canoe_usb_web.js');await wasm.default();
    const{FakeDevice}=await import('/fake-device.js');
    const assert=(condition,message)=>{if(!condition)throw new Error(message);};
    const cases=[];
    const raw=new FakeDevice();const before=raw.bytes.slice();
    let session=await wasm.openManagedStorage(raw,'read-write');
    assert(session.capacity().bytes===8192,'capacity');assert(session.identity().maximumLun===0,'LUN');
    await session.writeRange(507n,Uint8Array.from({length:20},()=>77));await session.sync();
    await session.close();assert(!session.usable()&&!raw.opened,'close');
    session=await wasm.openManagedStorage(raw,'read-only');
    const read=await session.readRange(507n,20);assert(read.every(byte=>byte===77),'fresh readback');
    assert(raw.bytes.slice(0,507).every((byte,n)=>byte===before[n]),'prefix preserved');
    assert(raw.bytes.slice(527).every((byte,n)=>byte===before[n+527]),'suffix preserved');
    let failure=false;const count=raw.events.length;
    try{await session.writeRange(0n,Uint8Array.of(7));}catch{failure=true;}
    assert(failure&&session.usable()&&raw.events.length===count,'read-only before I/O');
    await session.eject();assert(!session.usable()&&!raw.opened,'eject retires');
    cases.push({name:'unaligned-write-fresh-read-only-eject',ok:true});
    for(const fault of ['bad-lun','init-timeout','short-out']){
      const device=new FakeDevice(fault);let failed=false;
      try{await wasm.openManagedStorage(device,'read-only');}catch{failed=true;}
      assert(failed&&!device.opened&&device.closeCount>0,`${fault}: failed open must close`);
      if(fault!=='short-out')assert(!device.events.some(event=>event.startsWith('cdb:')),`${fault}: no BOT before initialization`);
      cases.push({name:fault,ok:true});
    }
    for(const fault of ['short-read','disconnect-write','flush']){
      const device=new FakeDevice();const session=await wasm.openManagedStorage(device,'read-write');device.fault=fault;
      let failed=false;try{if(fault==='short-read')await session.readRange(0n,512);else if(fault==='flush')await session.sync();else await session.writeRange(0n,new Uint8Array(512));}catch{failed=true;}
      assert(failed&&!session.usable()&&!device.opened,`${fault}: uncertain operation must retire and close`);
      cases.push({name:fault,ok:true});
    }
    const device=new FakeDevice();const closeSession=await wasm.openManagedStorage(device,'read-only');device.fault='close';let failed=false;try{await closeSession.close();}catch{failed=true;}
    assert(failed&&!closeSession.usable(),'close rejection propagated');cases.push({name:'close-rejection',ok:true});
    return{cases,hardwareAccess:false};
  });
  await mkdir(`${root}/.work/browser-storage`,{recursive:true});
  const wasmSha256=new Bun.CryptoHasher('sha256').update(await Bun.file(`${root}/.work/browser-storage/pkg/canoe_usb_web_bg.wasm`).arrayBuffer()).digest('hex');
  await writeFile(`${root}/.work/browser-storage/managed-browser.json`,JSON.stringify({browser:browser.version(),wasmSha256,...report},null,2)+'\n');
  console.log(JSON.stringify(report));
}finally{await browser.close();server.stop(true);}

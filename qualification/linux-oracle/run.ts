// Cross-check the production filesystem worker against Linux e2fsprogs.
// The engine directory is explicit; only generated disposable bytes are opened.
import {chromium,type Browser} from 'playwright-core'
import {mkdir,mkdtemp,readFile,writeFile} from 'node:fs/promises'
import {createHash} from 'node:crypto'
import {resolve} from 'node:path'
const engine=resolve(process.argv[2]??'.work/browser-storage'),label=process.argv[3]??'current'
if(!/^[a-z0-9-]+$/.test(label))throw new Error('Invalid qualification label')
const fixtureRoot=resolve(process.env['CANOE_EXT4_FIXTURE_ROOT']??'.work/linux-oracle');await mkdir(fixtureRoot,{recursive:true})
const work=await mkdtemp(resolve(fixtureRoot,'ext4-worker-'+label+'-'))
const input=resolve(work,'input.img'),result=resolve(work,'result.img'),extracted=resolve(work,'extracted.fat')
function command(args:string[]){const p=Bun.spawnSync(args,{stdout:'pipe',stderr:'pipe'});return {code:p.exitCode,stdout:p.stdout.toString(),log:p.stdout.toString()+p.stderr.toString()}}
let p=command(['truncate','-s','128M',input]);if(p.code)throw new Error(p.log)
p=command(['mkfs.ext4','-q','-F','-b','1024','-O','^metadata_csum,uninit_bg,^64bit','-E','lazy_itable_init=0,lazy_journal_init=0',input]);if(p.code)throw new Error(p.log)
const headers={'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'require-corp','Cache-Control':'no-store'}
const allowed=new Set(['filesystem.js','filesystem-worker.js','pkg/canoe_fs.js','pkg/canoe_fs_bg.wasm'])
const server=Bun.serve({hostname:'127.0.0.1',port:0,maxRequestBodySize:128*1024*1024+1024,async fetch(request){
 const name=new URL(request.url).pathname
 if(name==='/'&&request.method==='GET')return new Response('<!doctype html><title>Disposable ext4 worker qualification</title>',{headers:{...headers,'Content-Type':'text/html'}})
 if(name==='/input.img'&&request.method==='GET')return new Response(Bun.file(input),{headers})
 if(name==='/result.img'&&request.method==='PUT'){
  const data=await request.arrayBuffer();if(data.byteLength!==128*1024*1024)return new Response('Wrong length',{status:400,headers})
  await writeFile(result,new Uint8Array(data));return new Response('Saved fixture',{headers})
 }
 if(name.startsWith('/engine/')&&request.method==='GET'&&allowed.has(name.slice(8)))return new Response(Bun.file(resolve(engine,name.slice(8))),{headers})
 return new Response('Not found',{status:404,headers})
}})
let browser:Browser|undefined
const browserLog:string[]=[]
try{
 browser=await chromium.launch({headless:true})
 const context=await browser.newContext(),page=await context.newPage(),origin=`http://127.0.0.1:${server.port}`
 page.on('console',message=>browserLog.push(`${message.type()}: ${message.text()}`))
 page.on('pageerror',error=>browserLog.push(error.stack??error.message))
 await context.route('**/*',route=>new URL(route.request().url()).origin===origin?route.continue():route.abort())
 await page.addInitScript(()=>Object.defineProperty(navigator,'usb',{value:{getDevices:async()=>[],requestDevice:async()=>{throw new Error('No USB in fixture')}}}))
 await page.goto(origin)
 const data=await page.evaluate(async()=>{
  if(!crossOriginIsolated)throw new Error('Not isolated')
  const {openFilesystem}=await import('/engine/filesystem.js')
  const disk=new Uint8Array(await(await fetch('/input.img')).arrayBuffer())
  let reads=0,writes=0
  const transport=(access:'read-only'|'read-write')=>{
   let closed=false
   return {usable:()=>!closed,access:()=>access,capacity:()=>({bytes:disk.length}),readRange:async(offset:bigint,length:number)=>{if(closed)throw new Error('Fixture transport closed');reads++;return disk.slice(Number(offset),Number(offset)+length)},writeRange:async(offset:bigint,data:Uint8Array)=>{if(closed||access!=='read-write')throw new Error('Fixture transport is not writable');writes++;disk.set(data,Number(offset))},sync:async()=>{},close:async()=>{closed=true}}
  }
  const storage=transport('read-write')
  const fs=await openFilesystem({storage,kind:'ext4',access:'read-write'})
  try{
   const inspection=await fs.inspect()
   await fs.createFile('/unrelated.txt');await fs.write('/unrelated.txt',0,new TextEncoder().encode('preserve this'))
   await fs.mkdir('/temporary');await fs.createFile('/temporary/stage.fat')
   const chunk=new Uint8Array(4*1024*1024).fill(0x5a)
   for(let i=0;i<8;i++)await fs.write('/temporary/stage.fat',i*chunk.length,chunk)
   await fs.rename('/temporary/stage.fat','/efisp.fat');await fs.remove('/temporary');await fs.finish()
   const finishReleased=!fs.usable()&&storage.usable()
   if(!finishReleased)throw new Error('Finish did not release the worker while preserving caller-owned transport')
   const saved=await fetch('/result.img',{method:'PUT',body:disk});if(!saved.ok)throw new Error('Fixture save failed')
   return {inspection,reads,writes,isolated:crossOriginIsolated,finishReleased}
  }finally{try{if(fs.usable())await fs.abort()}finally{await storage.close()}}
 })
 const fsck=command(['e2fsck','-fn',result]);await writeFile(resolve(work,'fsck.log'),fsck.log)
 const dump=command(['debugfs','-R',`dump /efisp.fat ${extracted}`,result]);if(dump.code)throw new Error(dump.log)
 const hash=(data:Uint8Array)=>createHash('sha256').update(data).digest('hex')
 const expected=hash(Buffer.alloc(32*1024*1024,0x5a)),actual=hash(await readFile(extracted))
 const unrelated=command(['debugfs','-R','cat /unrelated.txt',result])
 const workerSha256=hash(await readFile(resolve(engine,'pkg/canoe_fs_bg.wasm')))
 const report={directory:work,workerSha256,...data,fsckExit:fsck.code,payloadMatches:actual===expected,unrelatedPreserved:unrelated.code===0&&unrelated.stdout==='preserve this'}
 await writeFile(resolve(work,'report.json'),JSON.stringify(report,null,2)+'\n');console.info(JSON.stringify(report))
 if(fsck.code||actual!==expected||!report.unrelatedPreserved)process.exitCode=1
}catch(error){
 await writeFile(resolve(work,'report.json'),JSON.stringify({directory:work,error:error instanceof Error?error.stack:String(error)},null,2)+'\n')
 throw error
}finally{
 try{await writeFile(resolve(work,'browser.log'),browserLog.join('\n')+'\n')}
 finally{try{await browser?.close()}finally{await server.stop(true)}}
}

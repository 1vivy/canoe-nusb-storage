// Exercise the actual WASM facade with synthetic WebUSB failures only.
// Build canoe-usb-web for wasm32 and run wasm-bindgen --target web into
// .work/usb-error-probe first (or pass another generated directory).
import {chromium} from 'playwright-core'
import {resolve,sep} from 'node:path'

const directory=resolve(process.argv[2]??'.work/usb-error-probe')
const server=Bun.serve({hostname:'127.0.0.1',port:0,fetch(request){
  const name=new URL(request.url).pathname.slice(1)
  if(!name)return new Response('<!doctype html><title>USB error fixture</title>',{headers:{'Content-Type':'text/html'}})
  const path=resolve(directory,name)
  if(!path.startsWith(directory+sep))return new Response('not found',{status:404})
  return new Response(Bun.file(path))
}})
const browser=await chromium.launch({headless:true})
try{
  const page=await browser.newPage()
  await page.goto(`http://127.0.0.1:${server.port}`)
  const results=await page.evaluate(async()=>{
    Object.defineProperty(navigator,'usb',{get(){throw new Error('Physical USB forbidden in fixture')}})
    const moduleUrl='/canoe_usb_web.js',module=await import(moduleUrl)
    await module.default()
    const results=[]
    for(const kind of ['fastboot','managed','managed-native'])for(const failClose of [false,true]){
      let opens=0,closes=0
      const original=new DOMException("Failed to execute 'claimInterface' on 'USBDevice': Unable to claim interface.",'NetworkError')
      const alternate={interfaceClass:255,interfaceSubclass:6,interfaceProtocol:80,alternateSetting:0}
      const device={vendorId:0x1209,productId:0xca0f,opened:false,configuration:{configurationValue:1,interfaces:[{interfaceNumber:0,alternate,alternates:[alternate]}]},
        async open(){opens++;if(kind==='managed-native'&&opens===1){this.opened=true;return}throw original},
        async close(){closes++;if(failClose)throw new DOMException('fixture close rejected','NetworkError');this.opened=false}}
      let failure:unknown
      try{if(kind==='fastboot')await module.openFastboot(device);else await module.openManagedStorage(device,'read-only')}
      catch(error){failure=error}
      const detail=(value:unknown,depth=0):unknown=>{
        if(depth>3)return 'depth limit'
        if(value instanceof Error)return {name:value.name,message:value.message,cause:detail(value.cause,depth+1),cleanupErrors:(value as Error&{cleanupErrors?:unknown[]}).cleanupErrors?.map(error=>detail(error,depth+1))}
        return String(value)
      }
      const recorded=detail(failure),text=JSON.stringify(recorded)
      if(!text.includes('Unable to claim interface'))throw new Error(`Primary failure lost: ${text}`)
      if(failClose&&!text.includes('fixture close rejected'))throw new Error(`Cleanup evidence lost: ${text}`)
      if(kind==='managed'&&failClose&&(failure as Error).cause!==original)throw new Error('Original browser exception identity was lost')
      if(!closes)throw new Error('Failed open did not attempt close')
      results.push({kind,failClose,opens,closes,error:recorded})
    }
    return results
  })
  for(const result of results)console.log(JSON.stringify(result))
}finally{await browser.close();await server.stop(true)}

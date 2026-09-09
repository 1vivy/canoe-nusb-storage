// Synthetic USB/BOT target. No navigator.usb method here accesses real hardware.
export class FakeDevice {
  constructor(fault = null, bytes = null) {
    const alternate={alternateSetting:0,interfaceClass:255,interfaceSubclass:6,interfaceProtocol:80};
    const selected={interfaceNumber:0,claimed:false,alternate,alternates:[alternate]};
    const configuration={configurationValue:1,interfaces:[selected]};
    const fields={vendorId:0x1209,productId:0xca0f,opened:false,configuration,configurations:[configuration]};
    for(const [name,value]of Object.entries(fields)) Object.defineProperty(this,name,{value,writable:true});
    this.fault=fault;this.bytes=bytes??Uint8Array.from({length:8192},(_,n)=>n%251);
    this.events=[];this.queue=[];this.armed=false;this.pending=null;this.closeCount=0;
    this.deviceDescriptor=Uint8Array.from([18,1,0,2,0,0,0,64,9,0x12,0x0f,0xca,0,1,0,0,0,1]);
    this.configDescriptor=Uint8Array.from([9,2,32,0,1,1,0,0x80,50,9,4,0,0,2,255,6,80,0,7,5,0x81,2,0,2,0,7,5,0x02,2,0,2,0]);
  }
  async open(){this.events.push('open');this.opened=true;}
  async close(){this.events.push('close');this.closeCount++;if(this.fault==='close')throw new Error('injected close failure');this.opened=false;this.configuration.interfaces[0].claimed=false;this.armed=false;this.queue=[];this.pending=null;}
  async selectConfiguration(value){if(value!==1)throw new Error('bad configuration');this.events.push('configuration');}
  async claimInterface(value){if(value!==0)throw new Error('bad interface');this.configuration.interfaces[0].claimed=true;this.events.push('claim');}
  async releaseInterface(value){if(value!==0)throw new Error('bad interface');this.configuration.interfaces[0].claimed=false;this.events.push('release');}
  async controlTransferIn(setup,length){
    this.events.push(`control:${setup.request}:${setup.value}`);
    let bytes;
    if(setup.request===6)bytes=(setup.value===256?this.deviceDescriptor:this.configDescriptor).slice(0,length);
    else if(setup.request===254&&setup.requestType==='class'&&setup.recipient==='interface'&&setup.index===0){
      if(this.fault==='init-timeout')return new Promise(()=>{});
      if(this.fault==='bad-lun')bytes=Uint8Array.of(16);
      else {this.armed=true;bytes=Uint8Array.of(0);}
    }else throw new Error('unexpected control request');
    return{status:'ok',data:new DataView(bytes.buffer)};
  }
  status(tag,code=0){const bytes=new Uint8Array(13),view=new DataView(bytes.buffer);bytes.set([85,83,66,83]);view.setUint32(4,tag,true);bytes[12]=code;this.queue.push(bytes);}
  async transferOut(endpoint,data){
    if(endpoint!==2)throw new Error('bad OUT endpoint');
    const bytes=new Uint8Array(data.buffer??data,data.byteOffset??0,data.byteLength??data.length);
    if(this.pending){const{offset,length,tag}=this.pending;this.pending=null;if(bytes.length!==length)throw new Error('wrong write length');
      if(this.fault==='disconnect-write')throw new Error('device disconnected');
      this.bytes.set(bytes,offset);this.status(tag);this.events.push('data-write');return{status:'ok',bytesWritten:bytes.length};}
    if(!this.armed)throw new Error('GET_MAX_LUN must precede CBW');
    if(bytes.length!==31||String.fromCharCode(...bytes.slice(0,4))!=='USBC')throw new Error('invalid CBW');
    const view=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength),tag=view.getUint32(4,true),cdb=bytes.slice(15),op=cdb[0];
    this.events.push(`cdb:${op}`);
    if(this.fault==='short-out')return{status:'ok',bytesWritten:30};
    if(op===0||op===0x35||op===0x1b){if(op===0x35&&this.fault==='flush')throw new Error('flush disconnected');this.status(tag);}
    else if(op===0x25){const reply=new Uint8Array(8),rv=new DataView(reply.buffer);rv.setUint32(0,this.bytes.length/512-1);rv.setUint32(4,512);this.queue.push(reply);this.status(tag);}
    else if(op===0x28||op===0x2a){const cv=new DataView(cdb.buffer,cdb.byteOffset,cdb.byteLength),offset=cv.getUint32(2)*512,length=cv.getUint16(7)*512;
      if(op===0x28){this.queue.push(this.bytes.slice(offset,offset+length));this.status(tag);}
      else this.pending={offset,length,tag};}
    else throw new Error(`unexpected CDB${op}`);
    return{status:'ok',bytesWritten:bytes.length};
  }
  async transferIn(endpoint,length){
    if(endpoint!==1||length%512!==0)throw new Error('bad IN request');
    let bytes=this.queue.shift();if(!bytes)throw new Error('missing data');
    if(this.fault==='short-read'&&bytes.length>=512)bytes=bytes.slice(0,bytes.length-1);
    return{status:'ok',data:new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength)};
  }
}

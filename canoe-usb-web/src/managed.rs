//! Browser managed storage uses the same SCSI engine as native qualification.
use crate::{bounded, error};
use js_sys::{Object, Reflect, Uint8Array};
use nusb_scsi::{
    range::{Access, RangeSession, MAX_RANGE_BYTES},
    NusbIo, Scsi,
};
use std::time::Duration;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(inline_js = r#"
export async function chooseManagedDevice() {
  return navigator.usb.requestDevice({filters:[{vendorId:0x1209,productId:0xca0f,classCode:255,subclassCode:6,protocolCode:80}]});
}
export async function managedInterface(device) {
  if(device.vendorId!==0x1209 || device.productId!==0xca0f) throw new Error('Expected the managed1209:ca0f export');
  if(!device.opened) await device.open();
  let configuration=device.configuration;
  if(!configuration) {
    const candidates=device.configurations.filter(c=>c.interfaces.some(i=>i.alternates.some(a=>a.interfaceClass===255&&a.interfaceSubclass===6&&a.interfaceProtocol===80)));
    if(candidates.length!==1) throw new Error('Expected one managed USB configuration');
    await device.selectConfiguration(candidates[0].configurationValue);
    configuration=device.configuration;
  }
  const interfaces=configuration.interfaces.filter(i=>i.alternate.interfaceClass===255&&i.alternate.interfaceSubclass===6&&i.alternate.interfaceProtocol===80);
  if(interfaces.length!==1) throw new Error('Expected one active managed ff/06/50 interface');
  const selected=interfaces[0];
  if(selected.claimed) throw new Error('Managed USB interface is already claimed');
  return [selected.interfaceNumber,selected.alternate.alternateSetting];
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn chooseManagedDevice() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn managedInterface(device: &web_sys::UsbDevice) -> Result<JsValue, JsValue>;
}

fn access_mode(value: &str) -> Result<Access, JsValue> {
    match value {
        "read-only" => Ok(Access::ReadOnly),
        "read-write" => Ok(Access::ReadWrite),
        _ => Err(error("access must be read-only or read-write")),
    }
}
fn field(object: &Object, name: &str, value: impl Into<JsValue>) {
    Reflect::set(object, &JsValue::from_str(name), &value.into()).expect("new object property");
}

#[wasm_bindgen]
pub struct ManagedStorageSession {
    client: Option<RangeSession<NusbIo>>,
    device: web_sys::UsbDevice,
    interface: u8,
    alternate: u8,
    max_lun: u8,
    access: Access,
}

/// Invoke directly from a user gesture, before backup or image preparation.
/// Access is fixed for the resulting session; this function performs no writes.
#[wasm_bindgen(js_name = requestManagedStorage)]
pub async fn request_managed_storage(access: &str) -> Result<ManagedStorageSession, JsValue> {
    access_mode(access)?;
    let device: web_sys::UsbDevice = chooseManagedDevice().await?.dyn_into()?;
    open_managed_storage(device, access).await
}

/// Reopen the same user-granted export after close, with a fresh filesystem
/// mount. The application must retain/reopen its backup before requesting RW
/// mount; this transport does not manufacture a backup or review receipt.
#[wasm_bindgen(js_name = openManagedStorage)]
pub async fn open_managed_storage(
    device: web_sys::UsbDevice,
    access: &str,
) -> Result<ManagedStorageSession, JsValue> {
    let access = access_mode(access)?;
    let opened = bounded(
        async {
            let selected = js_sys::Array::from(&managedInterface(&device).await?);
            let interface = selected
                .get(0)
                .as_f64()
                .ok_or_else(|| error("missing interface number"))?
                as u8;
            let alternate = selected
                .get(1)
                .as_f64()
                .ok_or_else(|| error("missing alternate setting"))?
                as u8;
            let io =
                NusbIo::connect_web(device.clone(), interface, alternate, Duration::from_secs(5))
                    .await
                    .map_err(error)?;
            let max_lun = io.max_lun();
            let scsi = Scsi::new(io, 0).map_err(error)?;
            let client = RangeSession::connect(scsi, access).await.map_err(error)?;
            Ok::<_, JsValue>((client, interface, alternate, max_lun))
        },
        20_000,
    )
    .await;
    match opened {
        Some(Ok((client, interface, alternate, max_lun))) => Ok(ManagedStorageSession {
            client: Some(client),
            device,
            interface,
            alternate,
            max_lun,
            access,
        }),
        failed => {
            JsFuture::from(device.close())
                .await
                .map_err(|_| error("USB close failed after managed initialization"))?;
            Err(match failed {
                Some(Err(error)) => error,
                _ => error("managed USB initialization timed out; reconnect"),
            })
        }
    }
}

#[wasm_bindgen]
impl ManagedStorageSession {
    /// Retain this user-granted device only to reopen after this session closes.
    /// Concurrent calls through the raw handle violate exclusive ownership.
    #[wasm_bindgen(js_name = usbDevice)]
    pub fn usb_device(&self) -> web_sys::UsbDevice {
        self.device.clone()
    }
    pub fn usable(&self) -> bool {
        self.client
            .as_ref()
            .is_some_and(|client| client.is_usable())
    }
    pub fn access(&self) -> String {
        match self.access {
            Access::ReadOnly => "read-only",
            Access::ReadWrite => "read-write",
        }
        .into()
    }
    /// Observed USB descriptor/initialization facts. They do not assert the
    /// identity or filesystem contents of a different export or phone.
    pub fn identity(&self) -> JsValue {
        let object = Object::new();
        field(&object, "vendorId", u32::from(self.device.vendor_id()));
        field(&object, "productId", u32::from(self.device.product_id()));
        field(&object, "interface", u32::from(self.interface));
        field(&object, "alternate", u32::from(self.alternate));
        field(&object, "interfaceClass", 255u32);
        field(&object, "interfaceSubclass", 6u32);
        field(&object, "interfaceProtocol", 80u32);
        field(&object, "maximumLun", u32::from(self.max_lun));
        field(&object, "selectedLun", 0u32);
        field(&object, "access", self.access());
        field(&object, "transport", "usb-bot-scsi");
        field(&object, "maxRangeBytes", MAX_RANGE_BYTES as u32);
        object.into()
    }
    pub fn capacity(&self) -> Result<JsValue, JsValue> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| error("managed session is closed"))?;
        let geometry = client.geometry();
        let object = Object::new();
        // READ CAPACITY(10) and the 64KiB sector bound keep these below 2^53.
        field(&object, "blocks", geometry.blocks as f64);
        field(&object, "blockSize", geometry.block_size);
        field(&object, "bytes", client.size_bytes() as f64);
        Ok(object.into())
    }
    #[wasm_bindgen(js_name = readRange)]
    pub async fn read_range(&mut self, offset: u64, length: u32) -> Result<Vec<u8>, JsValue> {
        let mut client = self.take()?;
        let result = bounded(client.read_range(offset, length as usize), 60_000).await;
        self.finish(client, result).await
    }
    #[wasm_bindgen(js_name = writeRange)]
    pub async fn write_range(&mut self, offset: u64, bytes: Uint8Array) -> Result<(), JsValue> {
        if bytes.length() == 0 || bytes.length() as usize > MAX_RANGE_BYTES {
            return Err(error("range length must be 1 byte through 4 MiB"));
        }
        if self.access != Access::ReadWrite {
            return Err(error("managed storage was opened read-only"));
        }
        // Copy once before awaiting; later edits to the caller's array cannot
        // change the reviewed bytes in flight. Check length before allocation.
        let bytes = bytes.to_vec();
        let mut client = self.take()?;
        let result = bounded(client.write_range(offset, &bytes), 120_000).await;
        self.finish(client, result).await
    }
    pub async fn sync(&mut self) -> Result<(), JsValue> {
        let mut client = self.take()?;
        let result = bounded(client.sync(), 30_000).await;
        self.finish(client, result).await
    }
    /// Eject only after filesystem finish and sync. A successful eject ends
    /// the export and retires this session; it does not imply readback.
    pub async fn eject(&mut self) -> Result<(), JsValue> {
        let mut client = self.take()?;
        let result = bounded(client.eject(), 30_000).await;
        drop(client);
        JsFuture::from(self.device.close())
            .await
            .map_err(|_| error("managed USB close failed after eject"))?;
        match result {
            Some(result) => result.map_err(error),
            None => Err(error("managed eject timed out; reconnect")),
        }
    }
    /// Consuming connection boundary for later fresh read-only verification.
    /// No automatic sync, filesystem replay, eject or retry occurs here.
    pub async fn close(&mut self) -> Result<(), JsValue> {
        drop(self.client.take());
        JsFuture::from(self.device.close()).await.map(|_| ())
    }
}

impl ManagedStorageSession {
    fn take(&mut self) -> Result<RangeSession<NusbIo>, JsValue> {
        self.client
            .take()
            .ok_or_else(|| error("managed session is closed or uncertain; reconnect"))
    }
    async fn finish<T>(
        &mut self,
        client: RangeSession<NusbIo>,
        result: Option<Result<T, nusb_scsi::Error>>,
    ) -> Result<T, JsValue> {
        match result {
            Some(Ok(value)) => {
                self.client = Some(client);
                Ok(value)
            }
            Some(Err(failure)) if client.is_usable() => {
                // Fully framed SCSI refusal or pre-I/O validation failure.
                self.client = Some(client);
                Err(error(failure))
            }
            failure => {
                drop(client);
                JsFuture::from(self.device.close())
                    .await
                    .map_err(|_| error("managed USB close failed after uncertain operation"))?;
                Err(match failure {
                    Some(Err(failure)) => error(failure),
                    _ => error("managed USB operation timed out; reconnect"),
                })
            }
        }
    }
}

//! Browser session facade over the shared fastboot engine. No second codec.
#![cfg(target_arch = "wasm32")]
use base64::Engine;
use fastboot_protocol::nusb::{Device, NusbFastBoot};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

mod managed;
pub use managed::*;

#[wasm_bindgen(inline_js = r#"
export async function chooseFastbootDevice() {
  return navigator.usb.requestDevice({ filters: [{classCode: 255, subclassCode: 66, protocolCode: 3}] });
}
export async function fastbootInterface(device) {
  let config = device.configuration;
  if (!config) {
    config = device.configurations.find(c => c.interfaces.some(i => i.alternates.some(a => a.interfaceClass === 255 && a.interfaceSubclass === 66 && a.interfaceProtocol === 3)));
    if (!config) throw new Error('Fastboot configuration is missing');
    await device.selectConfiguration(config.configurationValue);
    config = device.configuration;
  }
  const found = config.interfaces.find(i => i.alternate.interfaceClass === 255 && i.alternate.interfaceSubclass === 66 && i.alternate.interfaceProtocol === 3);
  if (!found) throw new Error('Fastboot interface is missing');
  return found.interfaceNumber;
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn chooseFastbootDevice() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn fastbootInterface(device: &web_sys::UsbDevice) -> Result<u8, JsValue>;
}
fn error(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

#[wasm_bindgen]
pub struct FastbootSession {
    client: Option<NusbFastBoot>,
    device: web_sys::UsbDevice,
}

/// Must be called directly from a user gesture. Permission belongs to WebUSB.
#[wasm_bindgen(js_name = requestFastboot)]
pub async fn request_fastboot() -> Result<FastbootSession, JsValue> {
    let device: web_sys::UsbDevice = chooseFastbootDevice().await?.dyn_into()?;
    open_fastboot(device).await
}

/// Reopen an already user-granted device. A new session never resumes writes.
#[wasm_bindgen(js_name = openFastboot)]
pub async fn open_fastboot(device: web_sys::UsbDevice) -> Result<FastbootSession, JsValue> {
    let native = match Device::from_js(device.clone()).await {
        Ok(d) => d,
        Err(e) => {
            let _ = JsFuture::from(device.close()).await;
            return Err(error(e));
        }
    };
    let opened = async {
        let interface = fastbootInterface(&device).await?;
        NusbFastBoot::from_device(native, interface)
            .await
            .map_err(error)
    }
    .await;
    match opened {
        Ok(client) => Ok(FastbootSession {
            client: Some(client),
            device,
        }),
        Err(e) => {
            let _ = JsFuture::from(device.close()).await;
            Err(e)
        }
    }
}

#[wasm_bindgen]
impl FastbootSession {
    pub fn usable(&self) -> bool {
        self.client.is_some()
    }
    pub async fn close(&mut self) -> Result<(), JsValue> {
        drop(self.client.take());
        JsFuture::from(self.device.close()).await.map(|_| ())
    }
    #[wasm_bindgen(js_name = getVar)]
    pub async fn get_var(&mut self, name: &str) -> Result<String, JsValue> {
        let mut c = self.take()?;
        let result = bounded(c.get_var(name), 15_000).await;
        self.finish(c, result).await
    }
    pub async fn command(&mut self, command: &str) -> Result<String, JsValue> {
        let mut c = self.take()?;
        let result = bounded(c.command(command), 120_000).await;
        self.finish(c, result).await
    }
    pub async fn fetch(
        &mut self,
        partition: &str,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, JsValue> {
        let mut c = self.take()?;
        let result = bounded(c.fetch(partition, offset, length), 60_000).await;
        self.finish(c, result).await
    }
    /// Caller reviews and authorizes the partition and exact source bytes.
    /// No retry, slot inference, erase, hash verification or reboot is implied.
    pub async fn flash(&mut self, partition: &str, image: &[u8]) -> Result<(), JsValue> {
        let size = u32::try_from(image.len()).map_err(error)?;
        if size == 0 {
            return Err(error("empty image"));
        }
        let mut c = self.take()?;
        let result = bounded(
            async {
                let mut download = c.download(size).await.map_err(error)?;
                download.extend_from_slice(image).await.map_err(error)?;
                download.finish().await.map_err(error)?;
                c.flash(partition).await.map_err(error)
            },
            120_000,
        )
        .await;
        match result {
            Some(Ok(())) => {
                self.client = Some(c);
                Ok(())
            }
            Some(Err(e)) => {
                drop(c);
                let _ = JsFuture::from(self.device.close()).await;
                Err(e)
            }
            None => {
                drop(c);
                let _ = JsFuture::from(self.device.close()).await;
                Err(error("USB operation timed out; reconnect"))
            }
        }
    }
    /// CANOE-BDS extension, separate from the upstream fastboot crate.
    #[wasm_bindgen(js_name = hashRange)]
    pub async fn hash_range(
        &mut self,
        partition: &str,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, JsValue> {
        if self.get_var("canoe-hash").await? != "sha256-range-v1" {
            return Err(error("partition hash is not supported"));
        }
        if partition.contains(':') || length == 0 || offset.checked_add(length).is_none() {
            return Err(error("invalid hash range"));
        }
        let reply = self
            .command(&format!("hash:{partition}:{offset:x}:{length:x}"))
            .await?;
        let hash = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(reply)
            .map_err(error)?;
        if hash.len() != 32 {
            return Err(error("invalid SHA-256 reply length"));
        }
        Ok(hash)
    }
}
impl FastbootSession {
    fn take(&mut self) -> Result<NusbFastBoot, JsValue> {
        self.client
            .take()
            .ok_or_else(|| error("session is closed or uncertain; reconnect"))
    }
    async fn finish<T>(
        &mut self,
        client: NusbFastBoot,
        result: Option<Result<T, fastboot_protocol::nusb::NusbFastBootError>>,
    ) -> Result<T, JsValue> {
        match result {
            Some(Ok(value)) => {
                self.client = Some(client);
                Ok(value)
            }
            Some(Err(e @ fastboot_protocol::nusb::NusbFastBootError::FastbootFailed(_))) => {
                // A complete FAIL ends the command and preserves framing.
                // Stock fastboot legitimately rejects optional BDS getvars.
                self.client = Some(client);
                Err(error(e))
            }
            Some(Err(e)) => {
                drop(client);
                let _ = JsFuture::from(self.device.close()).await;
                Err(error(e))
            }
            None => {
                drop(client);
                let _ = JsFuture::from(self.device.close()).await;
                Err(error("USB operation timed out; reconnect"))
            }
        }
    }
}

async fn bounded<T>(future: impl std::future::Future<Output = T>, milliseconds: u32) -> Option<T> {
    futures_lite::future::race(async { Some(future.await) }, async {
        futures_timer::Delay::new(std::time::Duration::from_millis(u64::from(milliseconds))).await;
        None
    })
    .await
}

use crate::{Session, MAX_FILE_CHUNK};
use nusb_ext4::{fs_ext4 as ext4, BlockDevice};
use serde::Serialize;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_name = canoeStorageRead)]
    fn read_at(offset: f64, out: &mut [u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = canoeStorageWrite)]
    fn write_at(offset: f64, bytes: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = canoeStorageFlush)]
    fn flush() -> Result<(), JsValue>;
}
fn error(value: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&value.to_string())
}
fn io_error(value: JsValue) -> ext4::Error {
    std::io::Error::other(format!("storage broker: {value:?}")).into()
}
struct BrokerDevice {
    length: u64,
    writable: bool,
}
impl BlockDevice for BrokerDevice {
    fn size_bytes(&self) -> u64 {
        self.length
    }
    fn is_writable(&self) -> bool {
        self.writable
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> ext4::Result<()> {
        read_at(offset as f64, out).map_err(io_error)
    }
    fn write_at(&self, offset: u64, bytes: &[u8]) -> ext4::Result<()> {
        write_at(offset as f64, bytes).map_err(io_error)
    }
    fn flush(&self) -> ext4::Result<()> {
        flush().map_err(io_error)
    }
}
#[wasm_bindgen]
pub struct BrowserFilesystem {
    session: Session,
}
#[wasm_bindgen(js_name = openFilesystem)]
pub fn open_filesystem(
    kind: &str,
    writable: bool,
    length: u64,
) -> Result<BrowserFilesystem, JsValue> {
    if length == 0 || length > 9_007_199_254_740_991 {
        return Err(error("filesystem capacity is not exactly representable"));
    }
    let session = Session::open(kind, writable, Arc::new(BrokerDevice { length, writable }))
        .map_err(error)?;
    Ok(BrowserFilesystem { session })
}
#[wasm_bindgen]
impl BrowserFilesystem {
    pub fn usable(&self) -> bool {
        self.session.usable()
    }
    pub fn inspect(&self) -> Result<JsValue, JsValue> {
        self.session
            .inspect()
            .map_err(error)?
            .serialize(&serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true))
            .map_err(error)
    }
    pub fn stat(&self, path: &str) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(&self.session.stat(path).map_err(error)?).map_err(error)
    }
    pub fn list(&self, path: &str) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(&self.session.list(path).map_err(error)?).map_err(error)
    }
    pub fn read(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>, JsValue> {
        self.session
            .read(path, offset, length as usize)
            .map_err(error)
    }
    #[wasm_bindgen(js_name = createFile)]
    pub fn create_file(&self, path: &str) -> Result<(), JsValue> {
        self.session.create_file(path).map_err(error)
    }
    pub fn write(&self, path: &str, offset: u64, bytes: js_sys::Uint8Array) -> Result<(), JsValue> {
        if bytes.length() as usize > MAX_FILE_CHUNK {
            return Err(error("file write exceeds4MiB"));
        }
        self.session
            .write(path, offset, &bytes.to_vec())
            .map_err(error)
    }
    pub fn truncate(&self, path: &str, length: u64) -> Result<(), JsValue> {
        self.session.truncate(path, length).map_err(error)
    }
    pub fn mkdir(&self, path: &str) -> Result<(), JsValue> {
        self.session.mkdir(path).map_err(error)
    }
    pub fn remove(&self, path: &str) -> Result<(), JsValue> {
        self.session.remove(path).map_err(error)
    }
    pub fn rename(&self, source: &str, destination: &str) -> Result<(), JsValue> {
        self.session.rename(source, destination).map_err(error)
    }
    pub fn finish(&mut self) -> Result<(), JsValue> {
        self.session.finish().map_err(error)
    }
    pub fn abort(&mut self) {
        self.session.abort()
    }
}

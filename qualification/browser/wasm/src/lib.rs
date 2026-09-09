//! Disposable browser feasibility probe, not a product block-device adapter.
use fs_ext4::block_io::BlockDevice;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::Arc;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_name = probeRead)]
    fn read(offset: f64, data: &mut [u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = probeWrite)]
    fn write(offset: f64, data: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = probeFlush)]
    fn flush() -> Result<(), JsValue>;
}

fn io_error(error: JsValue) -> io::Error {
    io::Error::other(format!("browser broker: {error:?}"))
}
fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

struct Device {
    len: u64,
    pos: u64,
}
impl Device {
    fn bounds(&self, offset: u64, length: usize) -> io::Result<()> {
        if offset
            .checked_add(length as u64)
            .is_none_or(|end| end > self.len)
        {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "outside fixture",
            ));
        }
        Ok(())
    }
}
impl BlockDevice for Device {
    fn size_bytes(&self) -> u64 {
        self.len
    }
    fn is_writable(&self) -> bool {
        true
    }
    fn read_at(&self, offset: u64, data: &mut [u8]) -> fs_ext4::Result<()> {
        self.bounds(offset, data.len())?;
        read(offset as f64, data).map_err(io_error)?;
        Ok(())
    }
    fn write_at(&self, offset: u64, data: &[u8]) -> fs_ext4::Result<()> {
        self.bounds(offset, data.len())?;
        write(offset as f64, data).map_err(io_error)?;
        Ok(())
    }
    fn flush(&self) -> fs_ext4::Result<()> {
        flush().map_err(io_error)?;
        Ok(())
    }
}
impl Read for Device {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let length = out.len().min(self.len.saturating_sub(self.pos) as usize);
        read(self.pos as f64, &mut out[..length]).map_err(io_error)?;
        self.pos += length as u64;
        Ok(length)
    }
}
impl Write for Device {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.bounds(self.pos, data.len())?;
        write(self.pos as f64, data).map_err(io_error)?;
        self.pos += data.len() as u64;
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        flush().map_err(io_error)
    }
}
impl Seek for Device {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let pos = match from {
            SeekFrom::Start(pos) => pos as i128,
            SeekFrom::Current(delta) => self.pos as i128 + delta as i128,
            SeekFrom::End(delta) => self.len as i128 + delta as i128,
        };
        if pos < 0 || pos > self.len as i128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek outside fixture",
            ));
        }
        self.pos = pos as u64;
        Ok(self.pos)
    }
}

#[wasm_bindgen]
pub fn fat_probe(len: u32, payload: &[u8]) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let fs = fatfs::FileSystem::new(
        fatfs::StdIoWrapper::new(Device {
            len: len.into(),
            pos: 0,
        }),
        fatfs::FsOptions::new(),
    )
    .map_err(js_error)?;
    {
        let root = fs.root_dir();
        let mut file = root.create_file("browser-stage.bin").map_err(js_error)?;
        file.write_all(payload).map_err(js_error)?;
        file.flush().map_err(js_error)?;
        drop(file);
        root.rename("browser-stage.bin", &root, "browser-result.bin")
            .map_err(js_error)?;
        let mut file = root.open_file("browser-result.bin").map_err(js_error)?;
        let mut actual = vec![0u8; payload.len()];
        file.read_exact(&mut actual).map_err(js_error)?;
        if actual != payload {
            return Err(js_error("FAT readback differs"));
        }
        drop(root.create_file("remove-me.txt").map_err(js_error)?);
        root.remove("remove-me.txt").map_err(js_error)?;
    }
    fs.unmount().map_err(js_error)?;
    flush().map_err(io_error).map_err(js_error)
}

#[wasm_bindgen]
pub fn ext4_probe(len: u32, payload: &[u8]) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let dev: Arc<dyn BlockDevice> = Arc::new(Device {
        len: len.into(),
        pos: 0,
    });
    let fs = fs_ext4::Filesystem::mount(dev.clone()).map_err(js_error)?;
    let inode = fs
        .apply_create("/browser-stage.fat", 0o600)
        .map_err(js_error)?;
    fs.apply_pwrite("/browser-stage.fat", 0, payload)
        .map_err(js_error)?;
    fs.apply_rename("/browser-stage.fat", "/efisp.fat", false)
        .map_err(js_error)?;
    let (inode, _) = fs.read_inode_verified(inode).map_err(js_error)?;
    let actual = fs_ext4::file_io::read_all(&fs, &inode).map_err(js_error)?;
    if actual != payload {
        return Err(js_error("ext4 readback differs"));
    }
    fs.apply_create("/remove-me.txt", 0o600).map_err(js_error)?;
    fs.apply_unlink("/remove-me.txt").map_err(js_error)?;
    dev.flush().map_err(js_error)
}

// Real browser API call, but an empty permission list proves no device transfer.
#[wasm_bindgen]
pub async fn granted_usb_devices() -> Result<u32, JsValue> {
    let devices = nusb::list_devices().await.map_err(js_error)?;
    Ok(devices.count() as u32)
}

// Type-check the actual async fastboot client in the same browser artifact.
pub async fn fastboot_version(
    fb: &mut fastboot_protocol::nusb::NusbFastBoot,
) -> Result<String, fastboot_protocol::nusb::NusbFastBootError> {
    fb.get_var("version").await
}

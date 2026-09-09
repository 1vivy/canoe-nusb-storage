//! Confined file operations shared by native qualification and the WASM worker.
//! The block device is supplied by its owner; this crate does not acquire USB.
use nusb_ext4::{fs_ext4 as ext4, BlockDevice};
use nusb_fatfs::fatfs;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[cfg(target_arch = "wasm32")]
mod wasm;
pub const MAX_FILE_CHUNK: usize = 4 * 1024 * 1024;
const MAX_DIRECTORY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
pub type Result<T> = std::result::Result<T, String>;

struct Guarded {
    device: Arc<dyn BlockDevice>,
    writable: bool,
    retired: AtomicBool,
}
impl Guarded {
    fn check(&self, offset: u64, length: usize) -> ext4::Result<()> {
        if self.retired.load(Ordering::Relaxed) {
            return Err(ext4::Error::Io(io::Error::other(
                "filesystem device retired",
            )));
        }
        if offset
            .checked_add(length as u64)
            .is_none_or(|end| end > self.size_bytes())
        {
            return Err(ext4::Error::OutOfBounds);
        }
        Ok(())
    }
    fn complete(&self, result: ext4::Result<()>) -> ext4::Result<()> {
        if result.is_err() {
            self.retired.store(true, Ordering::Relaxed);
        }
        result
    }
}
impl BlockDevice for Guarded {
    fn size_bytes(&self) -> u64 {
        self.device.size_bytes()
    }
    fn is_writable(&self) -> bool {
        self.writable
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> ext4::Result<()> {
        self.check(offset, out.len())?;
        self.complete(self.device.read_at(offset, out))
    }
    fn write_at(&self, offset: u64, data: &[u8]) -> ext4::Result<()> {
        self.check(offset, data.len())?;
        if !self.writable {
            return Err(ext4::Error::ReadOnly);
        }
        self.complete(self.device.write_at(offset, data))
    }
    fn flush(&self) -> ext4::Result<()> {
        self.check(0, 0)?;
        self.complete(self.device.flush())
    }
}
struct Io {
    device: Arc<Guarded>,
    position: u64,
}
fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
impl Read for Io {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let length = out
            .len()
            .min(self.device.size_bytes().saturating_sub(self.position) as usize);
        self.device
            .read_at(self.position, &mut out[..length])
            .map_err(io_error)?;
        self.position += length as u64;
        Ok(length)
    }
}
impl Write for Io {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.device
            .write_at(self.position, bytes)
            .map_err(io_error)?;
        self.position += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.device.flush().map_err(io_error)
    }
}
impl Seek for Io {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.device.size_bytes()) + i128::from(n),
        };
        if position < 0 || position > i128::from(self.device.size_bytes()) {
            return Err(io::Error::other("seek outside filesystem device"));
        }
        self.position = position as u64;
        Ok(self.position)
    }
}
enum Mounted {
    Fat(fatfs::FileSystem<fatfs::StdIoWrapper<Io>>),
    Ext4(Box<ext4::Filesystem>),
}
#[derive(Debug, Serialize, Clone)]
pub struct Entry {
    pub name: String,
    pub kind: &'static str,
    pub size: u64,
}
pub struct Session {
    mounted: Option<Mounted>,
    device: Arc<Guarded>,
}

/// Absolute paths within this filesystem. Reject aliases/traversal before I/O;
/// ext4 resolution never follows symbolic links, including final components.
fn path(value: &str) -> Result<&str> {
    if value == "/" {
        return Ok(value);
    }
    if !value.starts_with('/') || value.len() > 4096 || value.ends_with('/') {
        return Err("invalid filesystem path".into());
    }
    for component in value[1..].split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.len() > 255
            || component.ends_with(['.', ' '])
            || component
                .chars()
                .any(|c| c.is_control() || "\\:*?\"<>|".contains(c))
        {
            return Err("invalid filesystem path component".into());
        }
    }
    Ok(value)
}
fn ext_inode(
    fs: &ext4::Filesystem,
    value: &str,
) -> ext4::Result<(u32, ext4::inode::Inode, Vec<u8>)> {
    let ino = ext4::path::lookup_with_csum(
        fs.dev.as_ref(),
        &fs.sb,
        &mut |n| fs.read_inode_verified(n).map(|(inode, _)| inode),
        value,
        &fs.csum,
    )?;
    let (inode, raw) = fs.read_inode_verified(ino)?;
    if inode.is_symlink() {
        return Err(ext4::Error::Unsupported("symbolic links are not followed"));
    }
    Ok((ino, inode, raw))
}
fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

impl Session {
    /// Calling this in RW mode can replay ext4 immediately. The owner must have
    /// already saved/reopened the independent backup and checked raw identity.
    pub fn open(kind: &str, writable: bool, device: Arc<dyn BlockDevice>) -> Result<Self> {
        if writable && !device.is_writable() {
            return Err("underlying storage is read-only".into());
        }
        let device = Arc::new(Guarded {
            device,
            writable,
            retired: AtomicBool::new(false),
        });
        let mounted = match kind {
            "fat" => Mounted::Fat(
                nusb_fatfs::mount(Io {
                    device: device.clone(),
                    position: 0,
                })
                .map_err(error)?,
            ),
            "ext4" => Mounted::Ext4(Box::new(nusb_ext4::mount(device.clone()).map_err(error)?)),
            _ => return Err("filesystem must be fat or ext4".into()),
        };
        Ok(Self {
            mounted: Some(mounted),
            device,
        })
    }
    pub fn usable(&self) -> bool {
        self.mounted.is_some() && !self.device.retired.load(Ordering::Relaxed)
    }
    fn mounted(&self) -> Result<&Mounted> {
        if !self.usable() {
            return Err("filesystem session is closed or retired".into());
        }
        Ok(self.mounted.as_ref().unwrap())
    }
    fn writable(&self) -> Result<()> {
        self.mounted()?;
        if !self.device.writable {
            return Err("filesystem was opened read-only".into());
        }
        Ok(())
    }
    fn mutation<T>(&self, result: Result<T>) -> Result<T> {
        if result.is_err() {
            self.device.retired.store(true, Ordering::Relaxed);
        }
        result
    }
    pub fn inspect(&self) -> Result<Value> {
        match self.mounted()? {
            Mounted::Fat(fs) => {
                let stats = fs.stats().map_err(error)?;
                Ok(
                    json!({"kind":"fat","writable":self.device.writable,"deviceBytes":self.device.size_bytes(),"allocationUnit":stats.cluster_size(),"totalUnits":stats.total_clusters(),"freeBytes":u64::from(stats.free_clusters())*u64::from(stats.cluster_size()),"dirty":fs.read_status_flags().map_err(error)?.dirty()}),
                )
            }
            Mounted::Ext4(fs) => {
                let sb = ext4::superblock::Superblock::read(fs.dev.as_ref()).map_err(error)?;
                let journal = ext4::jbd2::read_superblock(fs).map_err(error)?;
                let supported = journal.as_ref().is_some_and(|j| {
                    j.validate_plain_recovery(sb.block_size(), sb.blocks_count)
                        .is_ok()
                });
                let write_blocker =
                    if sb.state & 2 != 0 || journal.as_ref().is_some_and(|j| j.errno != 0) {
                        Some("filesystem or journal records an outstanding error")
                    } else if !supported {
                        Some("checked writes require a supported plain internal JBD2 journal")
                    } else {
                        None
                    };
                Ok(
                    json!({"kind":"ext4","writable":self.device.writable,"deviceBytes":self.device.size_bytes(),"allocationUnit":sb.block_size(),"totalUnits":sb.blocks_count,"freeBytes":sb.free_blocks_count*u64::from(sb.block_size()),"reservedBytes":sb.r_blocks_count*u64::from(sb.block_size()),"freeInodes":sb.free_inodes_count,"uuid":sb.uuid,"features":{"compat":sb.feature_compat,"incompat":sb.feature_incompat,"roCompat":sb.feature_ro_compat},"needsRecovery":sb.feature_incompat&4!=0,"orphanHead":sb.last_orphan,"journal":journal.map(|j|json!({"start":j.start,"sequence":j.sequence,"error":j.errno,"compat":j.feature_compat,"incompat":j.feature_incompat,"roCompat":j.feature_ro_compat})),"checkedWriteFormatSupported":supported,"writeBlocker":write_blocker}),
                )
            }
        }
    }
    pub fn stat(&self, value: &str) -> Result<Entry> {
        path(value)?;
        match self.mounted()? {
            Mounted::Fat(fs) => {
                if value == "/" {
                    return Ok(Entry {
                        name: "/".into(),
                        kind: "directory",
                        size: 0,
                    });
                }
                let (parent, name) = value.rsplit_once('/').unwrap();
                let dir = if parent.is_empty() {
                    fs.root_dir()
                } else {
                    fs.root_dir().open_dir(&parent[1..]).map_err(error)?
                };
                for (index, item) in dir.iter().enumerate() {
                    if index >= MAX_ENTRIES {
                        return Err("directory entry limit exceeded".into());
                    }
                    let item = item.map_err(error)?;
                    if item.file_name().to_uppercase() == name.to_uppercase()
                        || item.short_file_name().to_uppercase() == name.to_uppercase()
                    {
                        return Ok(Entry {
                            name: item.file_name(),
                            kind: if item.is_dir() { "directory" } else { "file" },
                            size: item.len(),
                        });
                    }
                }
                Err("not found".into())
            }
            Mounted::Ext4(fs) => {
                let (_, inode, _) = ext_inode(fs, value).map_err(error)?;
                Ok(Entry {
                    name: value.rsplit('/').next().unwrap_or("/").into(),
                    kind: if inode.is_dir() {
                        "directory"
                    } else if inode.is_file() {
                        "file"
                    } else {
                        "other"
                    },
                    size: inode.size,
                })
            }
        }
    }
    pub fn list(&self, value: &str) -> Result<Vec<Entry>> {
        path(value)?;
        let mut entries = Vec::new();
        match self.mounted()? {
            Mounted::Fat(fs) => {
                let dir = if value == "/" {
                    fs.root_dir()
                } else {
                    fs.root_dir().open_dir(&value[1..]).map_err(error)?
                };
                for item in dir.iter() {
                    let item = item.map_err(error)?;
                    let name = item.file_name();
                    if name == "." || name == ".." {
                        continue;
                    }
                    if entries.len() >= MAX_ENTRIES {
                        return Err("directory entry limit exceeded".into());
                    }
                    entries.push(Entry {
                        name,
                        kind: if item.is_dir() { "directory" } else { "file" },
                        size: item.len(),
                    });
                }
            }
            Mounted::Ext4(fs) => {
                let (ino, inode, _) = ext_inode(fs, value).map_err(error)?;
                if !inode.is_dir() {
                    return Err("not a directory".into());
                }
                if inode.size > MAX_DIRECTORY_BYTES || inode.has_inline_data() {
                    return Err("unsupported or oversized directory layout".into());
                }
                for logical in 0..inode.size.div_ceil(u64::from(fs.sb.block_size())) {
                    let Some(physical) = fs.map_inode_logical(&inode, logical).map_err(error)?
                    else {
                        continue;
                    };
                    let block = fs.read_block(physical).map_err(error)?;
                    for entry in ext4::dir::parse_block_verified(
                        &block,
                        fs.sb.feature_incompat & 2 != 0,
                        ino,
                        inode.generation,
                        &fs.csum,
                    )
                    .map_err(error)?
                    {
                        if entry.name == b"." || entry.name == b".." {
                            continue;
                        }
                        if entries.len() >= MAX_ENTRIES {
                            return Err("directory entry limit exceeded".into());
                        }
                        let (inode, _) = fs.read_inode_verified(entry.inode).map_err(error)?;
                        entries.push(Entry {
                            name: String::from_utf8(entry.name).map_err(error)?,
                            kind: if inode.is_symlink() {
                                "symlink"
                            } else if inode.is_dir() {
                                "directory"
                            } else if inode.is_file() {
                                "file"
                            } else {
                                "other"
                            },
                            size: inode.size,
                        });
                    }
                }
            }
        }
        Ok(entries)
    }
    pub fn read(&self, value: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        path(value)?;
        if length > MAX_FILE_CHUNK {
            return Err("file read exceeds4MiB".into());
        }
        let mut bytes = vec![0; length];
        match self.mounted()? {
            Mounted::Fat(fs) => {
                let mut file = fs.root_dir().open_file(&value[1..]).map_err(error)?;
                file.seek(SeekFrom::Start(offset)).map_err(error)?;
                let mut count = 0;
                while count < bytes.len() {
                    let read = file.read(&mut bytes[count..]).map_err(error)?;
                    if read == 0 {
                        break;
                    }
                    count += read;
                }
                bytes.truncate(count);
            }
            Mounted::Ext4(fs) => {
                let (ino, inode, raw) = ext_inode(fs, value).map_err(error)?;
                if !inode.is_file() {
                    return Err("not a regular file".into());
                }
                let count = ext4::file_io::read_with_raw_verified(
                    fs,
                    &inode,
                    &raw,
                    ino,
                    offset,
                    length as u64,
                    &mut bytes,
                )
                .map_err(error)?;
                bytes.truncate(count as usize);
            }
        }
        Ok(bytes)
    }
    pub fn create_file(&self, value: &str) -> Result<()> {
        path(value)?;
        self.writable()?;
        if value == "/" {
            return Err("cannot replace root".into());
        }
        // Exclusive creation is intentional. Existing targets are never silently
        // truncated; callers choose a new stage or explicitly review removal.
        match self.stat(value) {
            Ok(_) => return Err("already exists".into()),
            Err(e) if e == "not found" => {}
            Err(e) => return Err(e),
        }
        let result = match self.mounted()? {
            Mounted::Fat(fs) => fs
                .root_dir()
                .create_file(&value[1..])
                .map(|_| ())
                .map_err(error),
            Mounted::Ext4(fs) => fs.apply_create(value, 0o600).map(|_| ()).map_err(error),
        };
        self.mutation(result)
    }
    pub fn write(&self, value: &str, offset: u64, bytes: &[u8]) -> Result<()> {
        path(value)?;
        self.writable()?;
        if bytes.len() > MAX_FILE_CHUNK
            || offset
                .checked_add(bytes.len() as u64)
                .is_none_or(|end| end > self.device.size_bytes())
        {
            return Err("invalid file write range".into());
        }
        let entry = self.stat(value)?;
        if entry.kind != "file" {
            return Err("not a regular file".into());
        }
        // FAT seeks clamp at EOF; refusing gaps keeps the byte-offset contract
        // identical on both engines rather than silently writing elsewhere.
        if offset > entry.size {
            return Err("file write cannot leave a gap".into());
        }
        let result = (|| match self.mounted()? {
            Mounted::Fat(fs) => {
                let mut file = fs.root_dir().open_file(&value[1..]).map_err(error)?;
                file.seek(SeekFrom::Start(offset)).map_err(error)?;
                file.write_all(bytes).map_err(error)?;
                file.flush().map_err(error)
            }
            Mounted::Ext4(fs) => {
                let (_, inode, _) = ext_inode(fs, value).map_err(error)?;
                if inode.links_count != 1 {
                    return Err("writing multiply linked files is refused".into());
                }
                fs.apply_pwrite(value, offset, bytes)
                    .map(|_| ())
                    .map_err(error)
            }
        })();
        self.mutation(result)
    }
    pub fn truncate(&self, value: &str, length: u64) -> Result<()> {
        path(value)?;
        self.writable()?;
        let item = self.stat(value)?;
        if item.kind != "file" || length > item.size {
            return Err("truncate only shrinks existing regular files".into());
        }
        let result = (|| match self.mounted()? {
            Mounted::Fat(fs) => {
                let mut file = fs.root_dir().open_file(&value[1..]).map_err(error)?;
                file.seek(SeekFrom::Start(length)).map_err(error)?;
                file.truncate().map_err(error)
            }
            Mounted::Ext4(fs) => {
                let (ino, inode, _) = ext_inode(fs, value).map_err(error)?;
                if inode.links_count != 1 {
                    return Err("truncating multiply linked files is refused".into());
                }
                fs.apply_truncate_shrink(ino, length).map_err(error)
            }
        })();
        self.mutation(result)
    }
    pub fn mkdir(&self, value: &str) -> Result<()> {
        path(value)?;
        self.writable()?;
        match self.stat(value) {
            Ok(_) => return Err("already exists".into()),
            Err(e) if e == "not found" => {}
            Err(e) => return Err(e),
        }
        let result = match self.mounted()? {
            Mounted::Fat(fs) => fs
                .root_dir()
                .create_dir(&value[1..])
                .map(|_| ())
                .map_err(error),
            Mounted::Ext4(fs) => fs.apply_mkdir(value, 0o700).map(|_| ()).map_err(error),
        };
        self.mutation(result)
    }
    pub fn remove(&self, value: &str) -> Result<()> {
        path(value)?;
        self.writable()?;
        if value == "/" {
            return Err("cannot remove root".into());
        }
        let entry = self.stat(value)?;
        let result = match self.mounted()? {
            Mounted::Fat(fs) => fs.root_dir().remove(&value[1..]).map_err(error),
            Mounted::Ext4(fs) => {
                if entry.kind == "directory" {
                    fs.apply_rmdir(value).map_err(error)
                } else {
                    fs.apply_unlink(value).map_err(error)
                }
            }
        };
        self.mutation(result)
    }
    pub fn rename(&self, source: &str, destination: &str) -> Result<()> {
        path(source)?;
        path(destination)?;
        self.writable()?;
        if source == "/" || destination == "/" {
            return Err("cannot rename root".into());
        }
        self.stat(source)?;
        match self.stat(destination) {
            Ok(_) => return Err("already exists".into()),
            Err(e) if e == "not found" => {}
            Err(e) => return Err(e),
        }
        let result = match self.mounted()? {
            Mounted::Fat(fs) => {
                let root = fs.root_dir();
                root.rename(&source[1..], &root, &destination[1..])
                    .map_err(error)
            }
            Mounted::Ext4(fs) => fs.apply_rename(source, destination, false).map_err(error),
        };
        self.mutation(result)
    }
    pub fn finish(&mut self) -> Result<()> {
        if !self.usable() {
            return Err("filesystem session is closed or retired".into());
        }
        if !self.device.writable {
            // FAT32 stats can mark an in-memory FSInfo cache dirty even during
            // inspection. A read-only finish must never publish those changes.
            self.abort();
            return Ok(());
        }
        let result = match self.mounted.take().unwrap() {
            Mounted::Fat(fs) => fs.unmount().map_err(error),
            Mounted::Ext4(fs) => nusb_ext4::finish(*fs).map_err(error),
        };
        self.mutation(result)?;
        self.device.flush().map_err(error)
    }
    pub fn abort(&mut self) {
        self.device.retired.store(true, Ordering::Relaxed);
        self.mounted.take();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.abort();
    }
}

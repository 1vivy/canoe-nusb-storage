//! ext4 composition with a checked plain-JBD2 recovery and release lifecycle.
pub use fs_ext4;
pub use fs_ext4::block_io::{BlockDevice, CallbackDevice};

struct ReadOnly(std::sync::Arc<dyn BlockDevice>);
impl BlockDevice for ReadOnly {
    fn read_at(&self, offset: u64, bytes: &mut [u8]) -> fs_ext4::Result<()> {
        self.0.read_at(offset, bytes)
    }
    fn size_bytes(&self) -> u64 {
        self.0.size_bytes()
    }
}

#[derive(Debug, Clone)]
pub struct Inspection {
    pub needs_recovery: bool,
    pub orphan_head: u32,
    pub journal_start: Option<u32>,
    pub journal_error: Option<u32>,
    pub write_blocker: Option<&'static str>,
}

/// Inspect through a read-only device even when the owner has write access.
/// The upstream writable mount can replay journals before any explicit file write.
pub fn inspect(device: std::sync::Arc<dyn BlockDevice>) -> fs_ext4::Result<Inspection> {
    let fs = fs_ext4::Filesystem::mount(std::sync::Arc::new(ReadOnly(device)))?;
    let journal = fs_ext4::jbd2::read_superblock(&fs)?;
    let needs_recovery = fs.sb.feature_incompat & fs_ext4::features::Incompat::RECOVER.bits() != 0;
    let journal_start = journal.as_ref().map(|j| j.start);
    let journal_error = journal.as_ref().map(|j| j.errno);
    // RECOVER and a pending cursor are normal mounted-journal state. The
    // dependency owns replay; refuse only an unsupported format or actual error.
    let write_blocker = if fs.sb.state & 2 != 0 || journal_error.is_some_and(|errno| errno != 0) {
        Some("filesystem or journal records an outstanding error")
    } else if journal.as_ref().is_none_or(|journal| {
        journal
            .validate_plain_recovery(fs.sb.block_size(), fs.sb.blocks_count)
            .is_err()
    }) {
        Some("checked writes require a supported plain internal JBD2 journal")
    } else {
        None
    };
    Ok(Inspection {
        needs_recovery,
        orphan_head: fs.sb.last_orphan,
        journal_start,
        journal_error,
        write_blocker,
    })
}

/// Requires exclusive ownership and an independently retained/reopened backup.
/// This call itself can mutate persist through journal replay. Run the backup
/// gate and raw-source identity check BEFORE calling it, not before the first
/// file operation. Does not run fsck or fall back to whole-partition flashing.
/// Call `finish` before releasing ownership; drop does not promise clean release.
pub fn mount(device: std::sync::Arc<dyn BlockDevice>) -> fs_ext4::Result<fs_ext4::Filesystem> {
    if device.is_writable() {
        if let Some(reason) = inspect(device.clone())?.write_blocker {
            return Err(fs_ext4::Error::Corrupt(reason));
        }
    }
    if device.is_writable() {
        fs_ext4::Filesystem::mount_recovering(device)
    } else {
        fs_ext4::Filesystem::mount(device)
    }
}

/// Flush and finish the checked filesystem before releasing USB ownership.
pub fn finish(filesystem: fs_ext4::Filesystem) -> fs_ext4::Result<()> {
    filesystem.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    struct Memory {
        bytes: Mutex<Vec<u8>>,
        writes: AtomicUsize,
    }
    impl BlockDevice for Memory {
        fn read_at(&self, o: u64, b: &mut [u8]) -> fs_ext4::Result<()> {
            b.copy_from_slice(&self.bytes.lock().unwrap()[o as usize..o as usize + b.len()]);
            Ok(())
        }
        fn size_bytes(&self) -> u64 {
            self.bytes.lock().unwrap().len() as u64
        }
        fn is_writable(&self) -> bool {
            true
        }
        fn write_at(&self, o: u64, b: &[u8]) -> fs_ext4::Result<()> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            self.bytes.lock().unwrap()[o as usize..o as usize + b.len()].copy_from_slice(b);
            Ok(())
        }
    }
    #[test]
    fn unsupported_journal_is_inspected_without_any_write() {
        let device = Arc::new(Memory {
            bytes: Mutex::new(vec![0; 32 * 1024 * 1024]),
            writes: AtomicUsize::new(0),
        });
        fs_ext4::mkfs::format_filesystem(
            device.as_ref(),
            None,
            Some([8; 16]),
            32 * 1024 * 1024,
            4096,
        )
        .unwrap();
        {
            let mut bytes = device.bytes.lock().unwrap();
            let sb = &mut bytes[1024..2048];
            let flags = u32::from_le_bytes(sb[0x60..0x64].try_into().unwrap()) | 4;
            sb[0x60..0x64].copy_from_slice(&flags.to_le_bytes());
            let crc = fs_ext4::checksum::linux_crc32c(!0, &sb[..0x3fc]);
            sb[0x3fc..].copy_from_slice(&crc.to_le_bytes());
        }
        device.writes.store(0, Ordering::Relaxed);
        let state = inspect(device.clone()).unwrap();
        assert!(state.needs_recovery && state.write_blocker.is_some());
        assert!(mount(device.clone()).is_err());
        assert_eq!(device.writes.load(Ordering::Relaxed), 0);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn ordinary_recovery_is_supported_but_checksum_format_is_not() {
        let path =
            std::env::temp_dir().join(format!("canoe-ext4-adapter-{}.img", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.set_len(32 * 1024 * 1024).unwrap();
        drop(file);
        let output = std::process::Command::new("mkfs.ext4")
            .args([
                "-q",
                "-F",
                "-b",
                "4096",
                "-O",
                "^metadata_csum,^64bit,^orphan_file",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(output.status.success());
        let device = Arc::new(Memory {
            bytes: Mutex::new(std::fs::read(&path).unwrap()),
            writes: AtomicUsize::new(0),
        });
        std::fs::remove_file(path).unwrap();
        {
            let mut bytes = device.bytes.lock().unwrap();
            let flags = u32::from_le_bytes(bytes[1120..1124].try_into().unwrap()) | 4;
            bytes[1120..1124].copy_from_slice(&flags.to_le_bytes());
        }
        let state = inspect(device.clone()).unwrap();
        assert!(state.needs_recovery);
        assert!(
            state.write_blocker.is_none(),
            "ordinary recovery must be supported"
        );
        assert_eq!(device.writes.load(Ordering::Relaxed), 0);
        finish(mount(device.clone()).unwrap()).unwrap();
        assert!(!inspect(device.clone()).unwrap().needs_recovery);
        let fs = fs_ext4::Filesystem::mount(Arc::new(ReadOnly(device.clone()))).unwrap();
        let inode =
            fs_ext4::inode::Inode::parse(&fs.read_inode_raw(fs.sb.journal_inode).unwrap()).unwrap();
        let journal = fs_ext4::jbd2::journal_block_to_physical(&fs, &inode, 0)
            .unwrap()
            .unwrap() as usize
            * 4096;
        {
            let mut bytes = device.bytes.lock().unwrap();
            bytes[journal + 0x28..journal + 0x2c].copy_from_slice(&0x10u32.to_be_bytes());
        }
        device.writes.store(0, Ordering::Relaxed);
        assert!(inspect(device.clone()).unwrap().write_blocker.is_some());
        assert!(mount(device.clone()).is_err());
        assert_eq!(
            device.writes.load(Ordering::Relaxed),
            0,
            "unsupported checksum format must not write even with a clean cursor"
        );
    }
}

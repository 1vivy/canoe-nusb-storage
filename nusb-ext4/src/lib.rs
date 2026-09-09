//! ext4 composition with a pre-write recovery barrier during bring-up.
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
    let write_blocker = if needs_recovery || journal_start.is_some_and(|start| start != 0) {
        Some("journal recovery is not qualified for direct managed writes")
    } else if journal_error.is_some_and(|errno| errno != 0) || fs.sb.last_orphan != 0 {
        Some("filesystem recovery is required before direct managed writes")
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

/// Requires exclusive ownership and a retained backup. Does not run fsck,
/// recovery, or fall back to whole-partition flashing. Preserve upstream guards.
pub fn mount(device: std::sync::Arc<dyn BlockDevice>) -> fs_ext4::Result<fs_ext4::Filesystem> {
    if device.is_writable() {
        if let Some(reason) = inspect(device.clone())?.write_blocker {
            return Err(fs_ext4::Error::Corrupt(reason));
        }
    }
    fs_ext4::Filesystem::mount(device)
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
    fn recovery_marked_volume_is_inspected_without_any_write() {
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
}

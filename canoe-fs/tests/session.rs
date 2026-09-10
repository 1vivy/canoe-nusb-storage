#![cfg(target_os = "linux")]
use canoe_fs::Session;
use nusb_ext4::{fs_ext4 as ext4, BlockDevice};
use std::{
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
struct Memory {
    bytes: Mutex<Vec<u8>>,
    writes: AtomicUsize,
    fail: AtomicBool,
}
impl BlockDevice for Memory {
    fn size_bytes(&self) -> u64 {
        self.bytes.lock().unwrap().len() as u64
    }
    fn is_writable(&self) -> bool {
        true
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> ext4::Result<()> {
        out.copy_from_slice(
            &self.bytes.lock().unwrap()[offset as usize..offset as usize + out.len()],
        );
        Ok(())
    }
    fn write_at(&self, offset: u64, bytes: &[u8]) -> ext4::Result<()> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        if self.fail.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("injected write failure").into());
        }
        self.bytes.lock().unwrap()[offset as usize..offset as usize + bytes.len()]
            .copy_from_slice(bytes);
        Ok(())
    }
    fn flush(&self) -> ext4::Result<()> {
        if self.fail.load(Ordering::Relaxed) {
            Err(std::io::Error::other("injected flush failure").into())
        } else {
            Ok(())
        }
    }
}
fn fixture(kind: &str) -> Arc<Memory> {
    let path = std::env::temp_dir().join(format!(
        "canoe-fs-{}-{kind}-{:?}.img",
        std::process::id(),
        std::thread::current().id()
    ));
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(32 * 1024 * 1024).unwrap();
    drop(file);
    let status = if kind == "fat" {
        Command::new("mkfs.fat")
            .args(["-F", "16"])
            .arg(&path)
            .status()
            .unwrap()
    } else {
        Command::new("mkfs.ext4")
            .args([
                "-q",
                "-F",
                "-b",
                "4096",
                "-O",
                "^metadata_csum,^64bit,^orphan_file,uninit_bg",
                "-E",
                "lazy_itable_init=0,lazy_journal_init=0",
            ])
            .arg(&path)
            .status()
            .unwrap()
    };
    assert!(status.success());
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    Arc::new(Memory {
        bytes: Mutex::new(bytes),
        writes: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
    })
}
#[test]
fn both_filesystems_preserve_unrelated_files_and_reopen_verified() {
    for kind in ["fat", "ext4"] {
        let device = fixture(kind);
        let payload: Vec<_> = (0..20000).map(|n| (n % 251) as u8).collect();
        let mut fs = Session::open(kind, true, device.clone()).unwrap();
        assert!(fs.inspect().unwrap()["freeBytes"].as_u64().unwrap() > 16 * 1024 * 1024);
        fs.create_file("/unrelated.bin").unwrap();
        fs.write("/unrelated.bin", 0, b"preserve").unwrap();
        if kind == "ext4" {
            let entry = fs.stat("/unrelated.bin").unwrap();
            assert!(entry.inode.is_some_and(|n| n > 2));
            assert!(entry.generation.is_some_and(|n| n > 0));
        }
        fs.mkdir("/managed").unwrap();
        fs.create_file("/managed/stage.bin").unwrap();
        assert!(fs.write("/managed/stage.bin", 1, b"gap").is_err());
        assert_eq!(fs.stat("/managed/stage.bin").unwrap().size, 0);
        fs.write("/managed/stage.bin", 0, &payload).unwrap();
        let written = device.writes.load(Ordering::Relaxed);
        fs.truncate("/managed/stage.bin", payload.len() as u64)
            .unwrap();
        assert_eq!(device.writes.load(Ordering::Relaxed), written);
        assert!(fs.usable());
        fs.rename("/managed/stage.bin", "/managed/result.bin")
            .unwrap();
        assert!(fs.create_file("/managed/result.bin").is_err());
        fs.create_file("/temporary").unwrap();
        fs.remove("/temporary").unwrap();
        fs.finish().unwrap();
        assert!(!fs.usable());
        let mut fresh = Session::open(kind, false, device.clone()).unwrap();
        assert_eq!(
            fresh.read("/managed/result.bin", 0, payload.len()).unwrap(),
            payload
        );
        assert_eq!(fresh.read("/unrelated.bin", 0, 8).unwrap(), b"preserve");
        assert!(fresh
            .list("/managed")
            .unwrap()
            .iter()
            .any(|e| e.name == "result.bin"));
        let baseline = device.writes.load(Ordering::Relaxed);
        assert!(fresh.create_file("/new").is_err());
        assert!(fresh.write("/unrelated.bin", 0, b"x").is_err());
        assert!(fresh.truncate("/unrelated.bin", 0).is_err());
        assert!(fresh.mkdir("/other").is_err());
        assert!(fresh.remove("/unrelated.bin").is_err());
        assert!(fresh.rename("/unrelated.bin", "/renamed").is_err());
        for path in [
            "../escape",
            "/../escape",
            "/managed/../escape",
            "/managed//result.bin",
            "/managed/result.bin.",
            "/managed\\result.bin",
        ] {
            assert!(fresh.stat(path).is_err());
        }
        fresh.finish().unwrap();
        assert_eq!(device.writes.load(Ordering::Relaxed), baseline);
    }
}
#[test]
fn failed_write_retires_before_drop_or_further_commands() {
    for kind in ["fat", "ext4"] {
        let device = fixture(kind);
        let mut fs = Session::open(kind, true, device.clone()).unwrap();
        fs.create_file("/stage").unwrap();
        device.fail.store(true, Ordering::Relaxed);
        assert!(fs.write("/stage", 0, b"uncertain").is_err());
        assert!(!fs.usable());
        let baseline = device.writes.load(Ordering::Relaxed);
        assert!(fs.write("/stage", 0, b"retry").is_err());
        assert!(fs.finish().is_err());
        drop(fs);
        assert_eq!(device.writes.load(Ordering::Relaxed), baseline);
    }
}
#[test]
fn fat_unicode_alias_cannot_bypass_exclusive_create() {
    let device = fixture("fat");
    let mut fs = Session::open("fat", true, device).unwrap();
    fs.create_file("/évidence.txt").unwrap();
    assert!(fs.create_file("/ÉVIDENCE.TXT").is_err());
    fs.finish().unwrap();
}

#[test]
fn ext4_links_cannot_redirect_reads_or_unrelated_writes() {
    let device = fixture("ext4");
    let fs = nusb_ext4::mount(device.clone()).unwrap();
    fs.apply_create("/original", 0o600).unwrap();
    fs.apply_pwrite("/original", 0, b"preserve").unwrap();
    fs.apply_symlink("/original", "/alias").unwrap();
    fs.apply_link("/original", "/second-name").unwrap();
    nusb_ext4::finish(fs).unwrap();
    let mut fs = Session::open("ext4", true, device.clone()).unwrap();
    assert!(fs.read("/alias", 0, 8).is_err());
    assert!(fs.read("/alias/child", 0, 8).is_err());
    assert!(fs.write("/second-name", 0, b"changed!").is_err());
    fs.abort();
    let mut fresh = Session::open("ext4", false, device).unwrap();
    assert_eq!(fresh.read("/original", 0, 8).unwrap(), b"preserve");
    fresh.finish().unwrap();
}

#[test]
fn live_sessions_flush_and_refresh_without_reopening() {
    for kind in ["fat", "ext4"] {
        let device = fixture(kind);
        let mut ro = Session::open(kind, false, device.clone()).unwrap();
        ro.inspect().unwrap();
        ro.flush().unwrap();
        ro.fresh_read().unwrap();
        assert_eq!(device.writes.load(Ordering::Relaxed), 0);
        ro.finish().unwrap();
        let mut fs = Session::open(kind, true, device.clone()).unwrap();
        fs.create_file("/live").unwrap();
        for text in [b"first".as_slice(), b"later".as_slice()] {
            fs.write("/live", 0, text).unwrap();
            fs.flush().unwrap();
            fs.fresh_read().unwrap();
            assert!(fs.usable());
            assert_eq!(fs.read("/live", 0, text.len()).unwrap(), text);
        }
        device.fail.store(true, Ordering::Relaxed);
        assert!(fs.flush().is_err());
        assert!(!fs.usable());
    }
}

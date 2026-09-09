//! Bounded byte ranges over the existing SCSI engine. Access is fixed per
//! connection; the application owns backup, review and fresh-open verification.
use crate::{BulkIo, Error, Geometry, Scsi};

pub const MAX_RANGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

pub struct RangeSession<T> {
    scsi: Scsi<T>,
    geometry: Geometry,
    access: Access,
}

impl<T: BulkIo> RangeSession<T> {
    pub async fn connect(mut scsi: Scsi<T>, access: Access) -> Result<Self, Error> {
        scsi.initialize_ready().await?;
        let geometry = scsi.geometry().await?;
        // READ/WRITE(10) and bounded allocation are the deliberately supported
        // envelope. Larger-capacity transports need READ CAPACITY(16) first.
        if geometry.blocks == 0
            || geometry.blocks > u64::from(u32::MAX)
            || geometry.block_size > 65536
        {
            return Err(Error::Protocol("unsupported managed storage geometry"));
        }
        Ok(Self {
            scsi,
            geometry,
            access,
        })
    }

    pub fn geometry(&self) -> Geometry {
        self.geometry
    }
    pub fn access(&self) -> Access {
        self.access
    }
    pub fn is_usable(&self) -> bool {
        self.scsi.is_usable()
    }
    pub fn into_inner(self) -> Scsi<T> {
        self.scsi
    }
    pub fn size_bytes(&self) -> u64 {
        self.geometry.blocks * u64::from(self.geometry.block_size)
    }

    fn span(&self, offset: u64, length: usize) -> Result<(u32, u16, usize), Error> {
        if length == 0 || length > MAX_RANGE_BYTES {
            return Err(Error::Protocol("range length must be 1 byte through 4 MiB"));
        }
        let end = offset
            .checked_add(length as u64)
            .filter(|end| *end <= self.size_bytes())
            .ok_or(Error::Protocol("range lies outside managed storage"))?;
        let block_size = u64::from(self.geometry.block_size);
        let first = offset / block_size;
        let last = end.div_ceil(block_size);
        let count = u16::try_from(last - first)
            .map_err(|_| Error::Protocol("range exceeds READ/WRITE(10) block count"))?;
        Ok((first as u32, count, (offset % block_size) as usize))
    }

    pub async fn read_range(&mut self, offset: u64, length: usize) -> Result<Vec<u8>, Error> {
        let (lba, count, skip) = self.span(offset, length)?;
        let bytes = self
            .scsi
            .read_blocks(lba, count, self.geometry.block_size)
            .await?;
        Ok(bytes[skip..skip + length].to_vec())
    }

    /// Unaligned ranges preserve the untouched sector prefix/suffix. No flush
    /// or durability receipt is implicit; the owner must sync and reopen.
    pub async fn write_range(&mut self, offset: u64, bytes: &[u8]) -> Result<(), Error> {
        if self.access != Access::ReadWrite {
            return Err(Error::Protocol("managed storage was opened read-only"));
        }
        let (lba, count, skip) = self.span(offset, bytes.len())?;
        let transfer_length = usize::from(count) * self.geometry.block_size as usize;
        if skip == 0 && transfer_length == bytes.len() {
            self.scsi
                .write_blocks(lba, count, self.geometry.block_size, bytes)
                .await
        } else {
            let mut sectors = self
                .scsi
                .read_blocks(lba, count, self.geometry.block_size)
                .await?;
            sectors[skip..skip + bytes.len()].copy_from_slice(bytes);
            self.scsi
                .write_blocks(lba, count, self.geometry.block_size, &sectors)
                .await
        }
    }
    pub async fn sync(&mut self) -> Result<(), Error> {
        self.scsi.flush().await
    }
    pub async fn eject(&mut self) -> Result<(), Error> {
        self.scsi.eject().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };
    struct Disk {
        bytes: Arc<Mutex<Vec<u8>>>,
        commands: Arc<Mutex<Vec<u8>>>,
        reads: VecDeque<Vec<u8>>,
        write: Option<(usize, usize, u32)>,
        fail_write: bool,
    }
    impl Disk {
        fn status(&mut self, tag: u32, failed: bool) {
            let mut csw = b"USBS".to_vec();
            csw.extend_from_slice(&tag.to_le_bytes());
            csw.extend_from_slice(&0u32.to_le_bytes());
            csw.push(u8::from(failed));
            self.reads.push_back(csw);
        }
    }
    impl BulkIo for Disk {
        async fn write(&mut self, data: &[u8]) -> Result<usize, Error> {
            if let Some((offset, length, tag)) = self.write.take() {
                assert_eq!(data.len(), length);
                if self.fail_write {
                    return Err(Error::Usb(crate::TransferError::Disconnected));
                }
                self.bytes.lock().unwrap()[offset..offset + length].copy_from_slice(data);
                self.status(tag, false);
                return Ok(data.len());
            }
            assert_eq!(data.len(), 31);
            let tag = u32::from_le_bytes(data[4..8].try_into().unwrap());
            let cdb = &data[15..];
            self.commands.lock().unwrap().push(cdb[0]);
            match cdb[0] {
                0x00 | 0x35 | 0x1b => self.status(tag, false),
                0x25 => {
                    let blocks = (self.bytes.lock().unwrap().len() / 512) as u32;
                    let mut reply = (blocks - 1).to_be_bytes().to_vec();
                    reply.extend_from_slice(&512u32.to_be_bytes());
                    self.reads.push_back(reply);
                    self.status(tag, false);
                }
                0x28 | 0x2a => {
                    let offset = u32::from_be_bytes(cdb[2..6].try_into().unwrap()) as usize * 512;
                    let length = u16::from_be_bytes(cdb[7..9].try_into().unwrap()) as usize * 512;
                    if cdb[0] == 0x28 {
                        self.reads.push_back(
                            self.bytes.lock().unwrap()[offset..offset + length].to_vec(),
                        );
                        self.status(tag, false);
                    } else {
                        self.write = Some((offset, length, tag));
                    }
                }
                _ => panic!("unexpected SCSI command"),
            }
            Ok(data.len())
        }
        async fn read(&mut self, _: usize) -> Result<Vec<u8>, Error> {
            self.reads
                .pop_front()
                .ok_or(Error::Protocol("unexpected bulk read"))
        }
    }
    fn disk(fail_write: bool) -> Disk {
        Disk {
            bytes: Arc::new(Mutex::new((0..4096).map(|n| (n % 251) as u8).collect())),
            commands: Arc::new(Mutex::new(Vec::new())),
            reads: VecDeque::new(),
            write: None,
            fail_write,
        }
    }
    #[test]
    fn unaligned_write_preserves_other_bytes_and_fresh_reads_match() {
        futures_lite::future::block_on(async {
            let disk = disk(false);
            let bytes = disk.bytes.clone();
            let before = bytes.lock().unwrap().clone();
            let mut session = RangeSession::connect(Scsi::new(disk, 0).unwrap(), Access::ReadWrite)
                .await
                .unwrap();
            session.write_range(507, &[77; 20]).await.unwrap();
            session.sync().await.unwrap();
            let disk = session.into_inner().into_inner();
            let mut fresh = RangeSession::connect(Scsi::new(disk, 0).unwrap(), Access::ReadOnly)
                .await
                .unwrap();
            assert_eq!(fresh.read_range(507, 20).await.unwrap(), vec![77; 20]);
            let after = bytes.lock().unwrap();
            assert_eq!(&after[..507], &before[..507]);
            assert_eq!(&after[527..], &before[527..]);
        });
    }
    #[test]
    fn invalid_and_read_only_ranges_issue_no_commands() {
        futures_lite::future::block_on(async {
            let disk = disk(false);
            let commands = disk.commands.clone();
            let mut session = RangeSession::connect(Scsi::new(disk, 0).unwrap(), Access::ReadOnly)
                .await
                .unwrap();
            let baseline = commands.lock().unwrap().len();
            assert!(session.write_range(0, &[4]).await.is_err());
            assert!(session.read_range(4096, 1).await.is_err());
            assert!(session.read_range(u64::MAX, 1).await.is_err());
            assert!(session.read_range(0, 0).await.is_err());
            assert_eq!(commands.lock().unwrap().len(), baseline);
            assert!(session.is_usable());
        });
    }
    #[test]
    fn uncertain_write_retires_without_retry_or_flush() {
        futures_lite::future::block_on(async {
            let disk = disk(true);
            let commands = disk.commands.clone();
            let mut session = RangeSession::connect(Scsi::new(disk, 0).unwrap(), Access::ReadWrite)
                .await
                .unwrap();
            assert!(session.write_range(0, &[4; 512]).await.is_err());
            let baseline = commands.lock().unwrap().len();
            assert!(matches!(session.sync().await, Err(Error::Retired)));
            assert_eq!(commands.lock().unwrap().len(), baseline);
        });
    }
}

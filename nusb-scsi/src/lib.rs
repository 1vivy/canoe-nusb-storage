//! Checked BOT/SCSI over a caller-selected vendor USB interface.
//!
//! The caller serializes operations and owns reconnect and device identity.
//! A dropped command future leaves the session retired; it cannot be reused
//! while a transfer may still be executing on the device.
use nusb::{
    descriptors::TransferType,
    transfer::{Buffer, Bulk, ControlIn, ControlType, In, Out, Recipient, TransferError},
    Endpoint,
};
use std::{future::Future, time::Duration};

pub mod range;
pub type ClaimedInterface = nusb::Interface;

/// Await interface release before closing WebUSB. nusb's fallback Drop starts
/// releaseInterface without awaiting it; racing device.close against that
/// interface state change fails in Chromium. The owner must drop all endpoint
/// references first. Failed release/close never yields a successful receipt.
#[cfg(target_arch = "wasm32")]
pub async fn close_web(
    interface: Option<ClaimedInterface>,
    device: &web_sys::UsbDevice,
) -> Result<(), Error> {
    async fn deadline<T>(future: impl Future<Output = T>) -> Option<T> {
        futures_lite::future::race(async { Some(future.await) }, async {
            futures_timer::Delay::new(Duration::from_secs(5)).await;
            None
        })
        .await
    }
    let released = if let Some(interface) = interface {
        match deadline(async { interface.release().await }).await {
            Some(Err(_)) if !device.opened() => Ok(()),
            Some(result) => result.map_err(Error::from),
            None => Err(Error::Protocol("WebUSB interface release timed out")),
        }
    } else {
        Ok(())
    };
    let closed = match deadline(wasm_bindgen_futures::JsFuture::from(device.close())).await {
        Some(Ok(_)) => Ok(()),
        Some(Err(_)) => Err(Error::Protocol("WebUSB device close failed")),
        None => Err(Error::Protocol("WebUSB device close timed out")),
    };
    closed?;
    released
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("USB transfer: {0}")]
    Usb(#[from] nusb::transfer::TransferError),
    #[error("USB interface: {0}")]
    Open(#[from] nusb::Error),
    #[error("storage protocol: {0}")]
    Protocol(&'static str),
    #[error("SCSI command failed; request sense before retry")]
    CommandFailed,
    #[error("storage session is retired; reconnect required")]
    Retired,
    #[error("USB initialization timed out; close the device before reconnecting")]
    InitializationTimeout,
}

/// Implementations must return actual transferred bytes, never requested bytes.
/// No `Send` future requirement: WebUSB executes in its owning JS context.
pub trait BulkIo {
    fn max_lun(&self) -> u8 {
        15
    }
    fn write(&mut self, data: &[u8]) -> impl Future<Output = Result<usize, Error>>;
    fn read(&mut self, length: usize) -> impl Future<Output = Result<Vec<u8>, Error>>;
}

pub struct NusbIo {
    input: Endpoint<Bulk, In>,
    output: Endpoint<Bulk, Out>,
    input_packet_size: usize,
    max_lun: u8,
    #[cfg(target_arch = "wasm32")]
    web_device: Option<web_sys::UsbDevice>,
    #[cfg(target_arch = "wasm32")]
    web_interface: Option<nusb::Interface>,
}
impl NusbIo {
    /// Open BOT only after GET_MAX_LUN has completed. Native nusb enforces the
    /// timeout and returns the control completion; do not race/drop that future.
    /// The caller owns the claimed interface and releases it on failure.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn connect(
        interface: nusb::Interface,
        alternate: u8,
        timeout: Duration,
    ) -> Result<Self, Error> {
        Self::initialize(interface, alternate, timeout).await
    }

    /// Browser connection owns its USBDevice, so a failed/timed-out open can
    /// await device.close() and abort EP0 before returning. The device must not
    /// be used concurrently through another wrapper. No configuration, alternate
    /// setting or driver is changed. The caller supplies an already selected
    /// active interface from the user-granted device. Await this future to
    /// completion: a caller cancelling it must itself await device.close().
    #[cfg(target_arch = "wasm32")]
    pub async fn connect_web(
        device: web_sys::UsbDevice,
        interface_number: u8,
        alternate: u8,
        timeout: Duration,
    ) -> Result<Self, Error> {
        validate_timeout(timeout)?;
        let mut claimed = None;
        let opened = futures_lite::future::race(
            async {
                let configuration = device
                    .configuration()
                    .ok_or(Error::Protocol("no active USB configuration"))?;
                let selected = configuration
                    .interfaces()
                    .into_iter()
                    .find(|entry| entry.interface_number() == interface_number)
                    .ok_or(Error::Protocol("selected USB interface is missing"))?;
                if selected.alternate().alternate_setting() != alternate {
                    return Err(Error::Protocol("selected alternate setting is not active"));
                }
                let native = nusb::Device::from_js(device.clone()).await?;
                let interface = native.claim_interface(interface_number).await?;
                claimed = Some(interface.clone());
                Self::initialize(interface, alternate, timeout).await
            },
            async {
                futures_timer::Delay::new(timeout).await;
                Err(Error::InitializationTimeout)
            },
        )
        .await;
        match opened {
            Ok(mut io) => {
                io.web_device = Some(device);
                io.web_interface = claimed;
                Ok(io)
            }
            Err(error) => {
                // nusb's WebUSB control timeout is currently ignored. Merely
                // dropping its Rust future does not cancel the JS transfer.
                close_web(claimed, &device).await?;
                Err(error)
            }
        }
    }

    async fn initialize(
        interface: nusb::Interface,
        alternate: u8,
        timeout: Duration,
    ) -> Result<Self, Error> {
        validate_timeout(timeout)?;
        let mut io = Self::from_interface(&interface, alternate)?;
        io.max_lun = parse_max_lun(
            interface
                .control_in(get_max_lun_request(interface.interface_number()), timeout)
                .await,
        )?;
        Ok(io)
    }

    pub fn max_lun(&self) -> u8 {
        self.max_lun
    }

    /// An outer protocol constructor retains this owner across fallible async
    /// initialization. Drop its endpoints before passing this handle to close_web.
    #[cfg(target_arch = "wasm32")]
    pub fn retain_web_interface(&self) -> nusb::Interface {
        self.web_interface
            .as_ref()
            .expect("connected WebUSB owner")
            .clone()
    }

    /// Cancel and drain pending transfers before releasing the owning interface.
    /// The caller must impose a deadline; a vanished device must not hang teardown.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn close(mut self) -> Result<(), Error> {
        self.input.cancel_all();
        self.output.cancel_all();
        while self.input.pending() != 0 {
            let _ = self.input.next_complete().await;
        }
        while self.output.pending() != 0 {
            let _ = self.output.next_complete().await;
        }
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn close(mut self) -> Result<(), Error> {
        drop(self.input);
        drop(self.output);
        let device = self
            .web_device
            .take()
            .ok_or(Error::Protocol("missing WebUSB owner"))?;
        close_web(self.web_interface.take(), &device).await
    }

    /// Require the selected alternate setting to be already active. Never
    /// choose a different alternate setting or detach a kernel storage driver.
    fn from_interface(interface: &nusb::Interface, alternate: u8) -> Result<Self, Error> {
        if interface.get_alt_setting() != alternate {
            return Err(Error::Protocol("selected alternate setting is not active"));
        }
        let descriptor = interface
            .descriptors()
            .find(|d| d.alternate_setting() == alternate)
            .ok_or(Error::Protocol("selected alternate setting is missing"))?;
        if descriptor.class() != 0xff || descriptor.subclass() != 6 || descriptor.protocol() != 0x50
        {
            return Err(Error::Protocol("managed storage requires ff/06/50"));
        }
        let input = descriptor
            .endpoints()
            .find(|e| e.transfer_type() == TransferType::Bulk && e.address() & 0x80 != 0)
            .ok_or(Error::Protocol("bulk IN endpoint missing"))?;
        let output = descriptor
            .endpoints()
            .find(|e| e.transfer_type() == TransferType::Bulk && e.address() & 0x80 == 0)
            .ok_or(Error::Protocol("bulk OUT endpoint missing"))?;
        Ok(Self {
            input: interface.endpoint(input.address())?,
            output: interface.endpoint(output.address())?,
            input_packet_size: input.max_packet_size(),
            max_lun: 0,
            #[cfg(target_arch = "wasm32")]
            web_device: None,
            #[cfg(target_arch = "wasm32")]
            web_interface: None,
        })
    }
}
impl BulkIo for NusbIo {
    fn max_lun(&self) -> u8 {
        self.max_lun
    }
    async fn write(&mut self, data: &[u8]) -> Result<usize, Error> {
        self.output.submit(data.to_vec().into());
        let completion = self.output.next_complete().await;
        completion.status?;
        Ok(completion.actual_len)
    }
    async fn read(&mut self, length: usize) -> Result<Vec<u8>, Error> {
        self.input
            .submit(Buffer::new(length.next_multiple_of(self.input_packet_size)));
        let completion = self.input.next_complete().await;
        completion.status?;
        Ok(completion.buffer.to_vec())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub blocks: u64,
    pub block_size: u32,
}

pub struct Scsi<T> {
    io: T,
    tag: u32,
    lun: u8,
    usable: bool,
}
impl<T: BulkIo> Scsi<T> {
    pub fn new(io: T, lun: u8) -> Result<Self, Error> {
        if lun > 15 || lun > io.max_lun() {
            return Err(Error::Protocol("invalid LUN"));
        }
        Ok(Self {
            io,
            tag: 0,
            lun,
            usable: true,
        })
    }
    pub fn is_usable(&self) -> bool {
        self.usable
    }
    /// Consume this session to cancel/drain and release its transport.
    pub fn into_inner(self) -> T {
        self.io
    }

    async fn command(
        &mut self,
        cdb: &[u8],
        incoming: usize,
        outgoing: &[u8],
    ) -> Result<Vec<u8>, Error> {
        if !self.usable {
            return Err(Error::Retired);
        }
        if cdb.is_empty() || cdb.len() > 16 || (incoming > 0 && !outgoing.is_empty()) {
            return Err(Error::Protocol("invalid command shape"));
        }
        let length = u32::try_from(incoming.max(outgoing.len()))
            .map_err(|_| Error::Protocol("transfer exceeds BOT limit"))?;
        self.tag = self.tag.wrapping_add(1);
        let tag = self.tag;
        let mut cbw = [0u8; 31];
        cbw[..4].copy_from_slice(b"USBC");
        cbw[4..8].copy_from_slice(&tag.to_le_bytes());
        cbw[8..12].copy_from_slice(&length.to_le_bytes());
        cbw[12] = if incoming > 0 { 0x80 } else { 0 };
        cbw[13] = self.lun;
        cbw[14] = cdb.len() as u8;
        cbw[15..15 + cdb.len()].copy_from_slice(cdb);
        // Set before the first await: cancellation also retires the session.
        self.usable = false;
        if self.io.write(&cbw).await? != cbw.len() {
            return Err(Error::Protocol("short CBW"));
        }
        let mut data = Vec::with_capacity(incoming);
        if incoming > 0 {
            while data.len() < incoming {
                let chunk = self
                    .io
                    .read((incoming - data.len()).min(1024 * 1024))
                    .await?;
                if chunk.is_empty() || chunk.len() > incoming - data.len() {
                    return Err(Error::Protocol("short or oversized data stage"));
                }
                data.extend_from_slice(&chunk);
            }
        } else if !outgoing.is_empty() {
            for chunk in outgoing.chunks(1024 * 1024) {
                if self.io.write(chunk).await? != chunk.len() {
                    return Err(Error::Protocol("short data write"));
                }
            }
        }
        let csw = self.io.read(13).await?;
        if csw.len() != 13 || &csw[..4] != b"USBS" || csw[4..8] != tag.to_le_bytes() {
            return Err(Error::Protocol("invalid CSW length, signature or tag"));
        }
        let residue = u32::from_le_bytes(csw[8..12].try_into().unwrap());
        if residue > length {
            return Err(Error::Protocol("invalid CSW residue"));
        }
        match csw[12] {
            0 if residue == 0 => {
                self.usable = true;
                Ok(data)
            }
            1 => {
                self.usable = true;
                Err(Error::CommandFailed)
            }
            _ => Err(Error::Protocol("CSW phase error or incomplete transfer")),
        }
    }
    pub async fn test_unit_ready(&mut self) -> Result<(), Error> {
        self.command(&[0; 6], 0, &[]).await.map(|_| ())
    }
    pub async fn inquiry(&mut self) -> Result<Vec<u8>, Error> {
        self.command(&[0x12, 0, 0, 0, 36, 0], 36, &[]).await
    }
    /// START STOP UNIT with LOEJ=1, START=0. A successful eject retires BOT.
    pub async fn eject(&mut self) -> Result<(), Error> {
        self.command(&[0x1b, 0, 0, 0, 2, 0], 0, &[]).await?;
        self.usable = false;
        Ok(())
    }
    pub async fn request_sense(&mut self) -> Result<Vec<u8>, Error> {
        self.command(&[3, 0, 0, 0, 18, 0], 18, &[]).await
    }
    pub async fn geometry(&mut self) -> Result<Geometry, Error> {
        let bytes = self
            .command(&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], 8, &[])
            .await?;
        let last = u32::from_be_bytes(bytes[..4].try_into().unwrap());
        let size = u32::from_be_bytes(bytes[4..].try_into().unwrap());
        if last == u32::MAX || size == 0 || !size.is_power_of_two() {
            return Err(Error::Protocol("unsupported capacity or sector size"));
        }
        Ok(Geometry {
            blocks: u64::from(last) + 1,
            block_size: size,
        })
    }
    pub async fn read_blocks(
        &mut self,
        lba: u32,
        count: u16,
        block_size: u32,
    ) -> Result<Vec<u8>, Error> {
        let (cdb, length) = block_command(0x28, lba, count, block_size)?;
        self.command(&cdb, length, &[]).await
    }
    pub async fn write_blocks(
        &mut self,
        lba: u32,
        count: u16,
        block_size: u32,
        bytes: &[u8],
    ) -> Result<(), Error> {
        let (cdb, length) = block_command(0x2a, lba, count, block_size)?;
        if bytes.len() != length {
            return Err(Error::Protocol("write length differs from sector count"));
        }
        self.command(&cdb, 0, bytes).await.map(|_| ())
    }
    pub async fn flush(&mut self) -> Result<(), Error> {
        self.command(&[0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0], 0, &[])
            .await
            .map(|_| ())
    }
}

fn validate_timeout(timeout: Duration) -> Result<(), Error> {
    if timeout.is_zero() || timeout > Duration::from_secs(60) {
        return Err(Error::Protocol(
            "initialization timeout must be positive and at most 60 seconds",
        ));
    }
    Ok(())
}

fn get_max_lun_request(interface: u8) -> ControlIn {
    ControlIn {
        control_type: ControlType::Class,
        recipient: Recipient::Interface,
        request: 0xfe,
        value: 0,
        index: u16::from(interface),
        length: 1,
    }
}

fn parse_max_lun(response: Result<Vec<u8>, TransferError>) -> Result<u8, Error> {
    match response {
        Ok(bytes) if bytes.len() == 1 && bytes[0] <= 15 => Ok(bytes[0]),
        // USB MSC BOT permits single-LUN devices to stall GET_MAX_LUN.
        Err(TransferError::Stall) => Ok(0),
        Ok(_) => Err(Error::Protocol("invalid GET_MAX_LUN response")),
        Err(error) => Err(error.into()),
    }
}
fn block_command(
    op: u8,
    lba: u32,
    count: u16,
    block_size: u32,
) -> Result<([u8; 10], usize), Error> {
    if count == 0
        || block_size == 0
        || !block_size.is_power_of_two()
        || lba.checked_add(u32::from(count) - 1).is_none()
    {
        return Err(Error::Protocol("invalid block range"));
    }
    let length = usize::from(count)
        .checked_mul(block_size as usize)
        .ok_or(Error::Protocol("block range overflow"))?;
    let mut cdb = [0; 10];
    cdb[0] = op;
    cdb[2..6].copy_from_slice(&lba.to_be_bytes());
    cdb[7..9].copy_from_slice(&count.to_be_bytes());
    Ok((cdb, length))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    #[test]
    fn get_max_lun_targets_the_selected_interface() {
        let request = get_max_lun_request(7);
        assert_eq!(request.control_type, ControlType::Class);
        assert_eq!(request.recipient, Recipient::Interface);
        assert_eq!(
            (
                request.request,
                request.value,
                request.index,
                request.length
            ),
            (0xfe, 0, 7, 1)
        );
    }
    #[test]
    fn max_lun_requires_a_valid_reply_or_the_standard_single_lun_stall() {
        assert_eq!(parse_max_lun(Ok(vec![0])).unwrap(), 0);
        assert_eq!(parse_max_lun(Ok(vec![15])).unwrap(), 15);
        assert_eq!(parse_max_lun(Err(TransferError::Stall)).unwrap(), 0);
        for response in [vec![], vec![16], vec![0, 0]] {
            assert!(matches!(
                parse_max_lun(Ok(response)),
                Err(Error::Protocol(_))
            ));
        }
        assert!(matches!(
            parse_max_lun(Err(TransferError::Cancelled)),
            Err(Error::Usb(TransferError::Cancelled))
        ));
        assert!(validate_timeout(Duration::ZERO).is_err());
        assert!(validate_timeout(Duration::from_secs(61)).is_err());
        assert!(validate_timeout(Duration::from_secs(5)).is_ok());
    }
    #[test]
    fn selected_lun_cannot_exceed_the_connected_transport() {
        struct SingleLun;
        impl BulkIo for SingleLun {
            fn max_lun(&self) -> u8 {
                0
            }
            async fn write(&mut self, _: &[u8]) -> Result<usize, Error> {
                panic!("no transfer before LUN validation")
            }
            async fn read(&mut self, _: usize) -> Result<Vec<u8>, Error> {
                panic!("no transfer before LUN validation")
            }
        }
        assert!(Scsi::new(SingleLun, 0).is_ok());
        assert!(matches!(Scsi::new(SingleLun, 1), Err(Error::Protocol(_))));
    }
    struct Fake {
        reads: VecDeque<Vec<u8>>,
        short: bool,
    }
    impl BulkIo for Fake {
        async fn write(&mut self, b: &[u8]) -> Result<usize, Error> {
            Ok(b.len() - usize::from(self.short))
        }
        async fn read(&mut self, _: usize) -> Result<Vec<u8>, Error> {
            self.reads
                .pop_front()
                .ok_or(Error::Protocol("unexpected read"))
        }
    }
    fn status(tag: u32, residue: u32, status: u8) -> Vec<u8> {
        let mut b = b"USBS".to_vec();
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&residue.to_le_bytes());
        b.push(status);
        b
    }
    #[test]
    fn validates_csw_and_flush() {
        futures_lite::future::block_on(async {
            for bytes in [
                status(9, 0, 0),
                status(1, 1, 0),
                status(1, 0, 2),
                vec![0; 13],
            ] {
                let mut d = Scsi::new(
                    Fake {
                        reads: VecDeque::from([bytes]),
                        short: false,
                    },
                    0,
                )
                .unwrap();
                assert!(d.flush().await.is_err());
                assert!(!d.is_usable());
                assert!(matches!(d.flush().await, Err(Error::Retired)));
            }
            let mut d = Scsi::new(
                Fake {
                    reads: VecDeque::from([status(1, 0, 0)]),
                    short: false,
                },
                0,
            )
            .unwrap();
            d.flush().await.unwrap();
            assert!(d.is_usable());
        });
    }
    #[test]
    fn rejects_short_writes() {
        futures_lite::future::block_on(async {
            let mut d = Scsi::new(
                Fake {
                    reads: VecDeque::new(),
                    short: true,
                },
                0,
            )
            .unwrap();
            assert!(d.flush().await.is_err());
            assert!(!d.is_usable());
        });
    }
    #[test]
    fn failed_scsi_command_remains_available_for_sense() {
        futures_lite::future::block_on(async {
            let mut d = Scsi::new(
                Fake {
                    reads: VecDeque::from([status(1, 0, 1)]),
                    short: false,
                },
                0,
            )
            .unwrap();
            assert!(matches!(d.flush().await, Err(Error::CommandFailed)));
            assert!(d.is_usable());
        });
    }
    #[test]
    fn rejects_invalid_ranges() {
        assert!(block_command(0x28, u32::MAX, 2, 512).is_err());
        assert!(block_command(0x28, 0, 0, 512).is_err());
    }

    #[test]
    fn successful_eject_retires_the_session() {
        futures_lite::future::block_on(async {
            let mut device = Scsi::new(
                Fake {
                    reads: VecDeque::from([status(1, 0, 0)]),
                    short: false,
                },
                0,
            )
            .unwrap();
            device.eject().await.unwrap();
            assert!(!device.is_usable());
            assert!(matches!(
                device.test_unit_ready().await,
                Err(Error::Retired)
            ));
        });
    }
}

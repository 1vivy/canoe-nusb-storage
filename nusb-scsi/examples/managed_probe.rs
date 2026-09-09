//! Read-only qualification for CANOE's vendor-class managed BOT export.
//! Does not mount filesystems, detach drivers, select configurations, reset USB,
//! issue WRITE commands or print sampled partition contents.
use futures_lite::future::{block_on, race};
use nusb::MaybeFuture;
use nusb_scsi::{NusbIo, Scsi};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{future::Future, time::Duration};

async fn bounded<T>(
    stage: &'static str,
    future: impl Future<Output = Result<T, nusb_scsi::Error>>,
) -> Result<T, String> {
    race(
        async { future.await.map_err(|error| format!("{stage}: {error}")) },
        async {
            futures_timer::Delay::new(Duration::from_secs(5)).await;
            Err(format!(
                "{stage}: five-second deadline expired; session retired"
            ))
        },
    )
    .await
}
fn emit(stage: &str, detail: Value) {
    println!("{}", json!({"stage":stage,"detail":detail}));
}

async fn commands(scsi: &mut Scsi<NusbIo>) -> Result<(), String> {
    let inquiry = bounded("inquiry", scsi.inquiry()).await?;
    emit(
        "inquiry",
        json!({"peripheralType":inquiry[0]&31,"removable":inquiry[1]&128!=0,
        "vendor":String::from_utf8_lossy(&inquiry[8..16]).trim(),
        "product":String::from_utf8_lossy(&inquiry[16..32]).trim(),
        "revision":String::from_utf8_lossy(&inquiry[32..36]).trim()}),
    );
    bounded("test-unit-ready", scsi.test_unit_ready()).await?;
    emit("test-unit-ready", json!({"ok":true}));
    let geometry = bounded("capacity", scsi.geometry()).await?;
    if geometry.block_size > 4096 || geometry.blocks == 0 {
        return Err("sector geometry exceeds bounded sample".into());
    }
    let count = (4096 / geometry.block_size).min(geometry.blocks as u32) as u16;
    emit(
        "capacity",
        json!({"blocks":geometry.blocks,"blockSize":geometry.block_size,"bytes":geometry.blocks*u64::from(geometry.block_size)}),
    );
    let sample = bounded(
        "read-blocks",
        scsi.read_blocks(0, count, geometry.block_size),
    )
    .await?;
    emit(
        "sample",
        json!({"lba":0,"blocks":count,"bytes":sample.len(),"sha256":format!("{:x}",Sha256::digest(&sample)),"contentsPrinted":false}),
    );
    bounded("synchronize-cache", scsi.flush()).await?;
    emit(
        "synchronize-cache",
        json!({"ok":true,"writeCommandsIssued":0}),
    );
    bounded("eject", scsi.eject()).await?;
    emit(
        "eject",
        json!({"ok":true,"sessionRetired":!scsi.is_usable()}),
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let mut bus = None;
    let mut address = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bus" => bus = Some(args.next().ok_or("--bus needs a value")?),
            "--address" => {
                address = Some(
                    args.next()
                        .ok_or("--address needs a value")?
                        .parse::<u8>()
                        .map_err(|_| "invalid address")?,
                )
            }
            "--help" => {
                println!("managed_probe [--bus BUS_ID --address USB_ADDRESS]\nSelects exactly one 1209:ca0f ff/06/50 export; reads at most4096 bytes, syncs, ejects and closes. No filesystem mount or WRITE commands.");
                return Ok(());
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    if bus.is_some() != address.is_some() {
        return Err("--bus and --address must be supplied together".into());
    }
    let devices: Vec<_> = nusb::list_devices()
        .wait()
        .map_err(|e| e.to_string())?
        .filter(|device| device.vendor_id() == 0x1209 && device.product_id() == 0xca0f)
        .filter(|device| {
            bus.as_ref().is_none_or(|bus| device.bus_id() == bus)
                && address.is_none_or(|address| device.device_address() == address)
        })
        .collect();
    if devices.len() != 1 {
        return Err(format!(
            "expected exactly one managed1209:ca0f export; found {}",
            devices.len()
        ));
    }
    let info = &devices[0];
    emit(
        "device",
        json!({"vid":"1209","pid":"ca0f","bus":info.bus_id(),"address":info.device_address()}),
    );
    let device = info.open().wait().map_err(|e| e.to_string())?;
    let configuration = device.active_configuration().map_err(|e| e.to_string())?;
    let interfaces: Vec<_> = configuration
        .interface_alt_settings()
        .filter(|i| i.class() == 0xff && i.subclass() == 6 && i.protocol() == 0x50)
        .collect();
    if interfaces.len() != 1 {
        return Err("expected one active ff/06/50 alternate descriptor".into());
    }
    let descriptor = &interfaces[0];
    let interface = device
        .claim_interface(descriptor.interface_number())
        .wait()
        .map_err(|e| e.to_string())?;
    if interface.get_alt_setting() != descriptor.alternate_setting() {
        return Err("selected alternate setting is not active".into());
    }
    emit(
        "interface",
        json!({"configuration":configuration.configuration_value(),"interface":descriptor.interface_number(),"alternate":descriptor.alternate_setting(),"class":"ff/06/50","kernelDetach":false}),
    );
    let io = NusbIo::from_interface(interface.clone(), descriptor.alternate_setting())
        .map_err(|e| e.to_string())?;
    let mut scsi = Scsi::new(io, 0).map_err(|e| e.to_string())?;
    let result = block_on(commands(&mut scsi));
    if result.is_err() && scsi.is_usable() {
        match block_on(bounded("request-sense", scsi.request_sense())) {
            Ok(sense) => emit("sense", json!({"bytes":sense})),
            Err(error) => emit("sense", json!({"error":error})),
        }
    }
    let io = scsi.into_inner();
    let drained = block_on(race(
        async {
            io.close().await;
            true
        },
        async {
            futures_timer::Delay::new(Duration::from_secs(5)).await;
            false
        },
    ));
    emit("transfers-drained", json!({"ok":drained}));
    // Native nusb offers explicit waited interface release; the device closes
    // after every interface/endpoint reference and this final handle are dropped.
    let release = interface.release().wait().map_err(|e| e.to_string());
    emit(
        "interface-release",
        json!({"ok":release.is_ok(),"error":release.as_ref().err()}),
    );
    drop(device);
    emit(
        "device-close",
        json!({"nativeHandleDropped":true,"dedicatedAsyncDeviceCloseApi":false}),
    );
    result?;
    release?;
    if !drained {
        return Err("transfer teardown deadline expired".into());
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        emit("failed", json!({"error":error,"writeCommandsIssued":0}));
        std::process::exit(1)
    }
}

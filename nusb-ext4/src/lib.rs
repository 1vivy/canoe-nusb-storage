//! ext4 composition: preserve the driver's feature guards and journal semantics.
pub use fs_ext4;
pub use fs_ext4::block_io::{BlockDevice, CallbackDevice};
pub fn mount(device: std::sync::Arc<dyn BlockDevice>) -> fs_ext4::Result<fs_ext4::Filesystem> {
    fs_ext4::Filesystem::mount(device)
}

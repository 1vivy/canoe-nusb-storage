//! FAT composition: the filesystem worker supplies checked synchronous I/O.
pub use fatfs;
pub fn mount<T: std::io::Read + std::io::Write + std::io::Seek>(
    io: T,
) -> std::io::Result<fatfs::FileSystem<fatfs::StdIoWrapper<T>>> {
    Ok(fatfs::FileSystem::new(
        fatfs::StdIoWrapper::new(io),
        fatfs::FsOptions::new(),
    )?)
}

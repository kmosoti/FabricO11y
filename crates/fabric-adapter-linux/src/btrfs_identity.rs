//! Stable Btrfs namespace identity from one already opened file descriptor.
//!
//! ABI: Linux include/uapi/linux/btrfs.h. Both operations used here are
//! unprivileged; INO_LOOKUP uses only the containing-subvolume special case.
use fabric_frame::envelope::BtrfsIdentity;
use std::fs::File;
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::fd::AsRawFd;

#[repr(C)]
struct FsInfo {
    max_id: u64,
    num_devices: u64,
    fsid: [u8; 16],
    nodesize: u32,
    sectorsize: u32,
    clone_alignment: u32,
    csum_type: u16,
    csum_size: u16,
    flags: u64,
    generation: u64,
    metadata_uuid: [u8; 16],
    reserved: [u8; 944],
}

#[repr(C)]
struct InoLookup {
    treeid: u64,
    objectid: u64,
    name: [u8; 4080],
}

const _: () = assert!(size_of::<FsInfo>() == 1024);
const _: () = assert!(size_of::<InoLookup>() == 4096);

pub(crate) fn read(file: &File) -> io::Result<Option<BtrfsIdentity>> {
    let mut info = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: file owns a live descriptor; fstatfs writes a complete statfs on
    // success. The uninitialized value is never read on failure.
    if unsafe { libc::fstatfs(file.as_raw_fd(), info.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstatfs initialized the entire result.
    let info = unsafe { info.assume_init() };
    if info.f_type != libc::BTRFS_SUPER_MAGIC {
        return Ok(None);
    }
    // SAFETY: these repr(C) ABI records contain only integer/byte fields; all
    // zero is valid and requests no optional FS_INFO flags.
    let mut fs: FsInfo = unsafe { std::mem::zeroed() };
    let mut lookup = InoLookup {
        treeid: 0,
        objectid: 256, // BTRFS_FIRST_FREE_OBJECTID: unrestricted root-ID lookup
        name: [0; 4080],
    };
    // SAFETY: ioctl request sizes match the checked ABI structures. Both live,
    // writable buffers cover their full request size; the descriptor is owned
    // by file. Neither operation changes the file or filesystem.
    if unsafe { libc::ioctl(file.as_raw_fd(), libc::_IOR::<FsInfo>(0x94, 31), &mut fs) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: same bounded-buffer and live-descriptor obligations as above.
    if unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            libc::_IOWR::<InoLookup>(0x94, 18),
            &mut lookup,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if fs.fsid == [0; 16] || lookup.treeid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid Btrfs filesystem identity",
        ));
    }
    Ok(Some(BtrfsIdentity {
        uuid: fs.fsid.to_vec(),
        subvolume_id: lookup.treeid,
    }))
}

use std::io;
use std::os::fd::RawFd;
use std::ptr::NonNull;

use super::invalid_data;

// Darwin sys/acl.h. Ignore read-only grants and deny additional ACL grants
// that could change any reviewed object or route, independent of POSIX mode.
const ACL_TYPE_EXTENDED: libc::c_uint = 0x100;
const ACL_FIRST_ENTRY: libc::c_int = 0;
const ACL_NEXT_ENTRY: libc::c_int = -1;
const ACL_EXTENDED_ALLOW: libc::c_uint = 1;
const ACL_MUTATION: u64 = (1 << 2)   // write data / add file
    | (1 << 4)                     // delete
    | (1 << 5)                     // append data / add directory
    | (1 << 6)                     // delete child
    | (1 << 8)                     // write attributes
    | (1 << 10)                    // write extended attributes
    | (1 << 12)                    // write security
    | (1 << 13); // change owner

unsafe extern "C" {
    fn acl_get_fd_np(fd: RawFd, acl_type: libc::c_uint) -> *mut libc::c_void;
    fn acl_get_entry(
        acl: *mut libc::c_void,
        entry_id: libc::c_int,
        entry: *mut *mut libc::c_void,
    ) -> libc::c_int;
    fn acl_get_tag_type(entry: *mut libc::c_void, tag: *mut libc::c_uint) -> libc::c_int;
    fn acl_get_permset_mask_np(entry: *mut libc::c_void, mask: *mut u64) -> libc::c_int;
    fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
}

pub(super) fn validate(fd: RawFd) -> io::Result<()> {
    // SAFETY: fd is a live object opened by the review.
    let acl = unsafe { acl_get_fd_np(fd, ACL_TYPE_EXTENDED) };
    let Some(acl) = NonNull::new(acl) else {
        let error = io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOATTR)) {
            return Ok(());
        }
        return Err(error);
    };
    let acl = Acl(acl);
    let mut index = ACL_FIRST_ENTRY;
    loop {
        let mut entry = std::ptr::null_mut();
        // SAFETY: The ACL is live and entry is writable. Darwin returns EINVAL
        // at the end of a valid first/next iteration.
        if unsafe { acl_get_entry(acl.0.as_ptr(), index, &mut entry) } != 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::EINVAL) {
                Ok(())
            } else {
                Err(error)
            };
        }
        index = ACL_NEXT_ENTRY;
        let mut tag = 0;
        let mut permissions = 0;
        // SAFETY: A successful iteration stores an entry owned by the live ACL.
        if unsafe { acl_get_tag_type(entry, &mut tag) } != 0
            || unsafe { acl_get_permset_mask_np(entry, &mut permissions) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if tag == ACL_EXTENDED_ALLOW && permissions & ACL_MUTATION != 0 {
            return Err(invalid_data());
        }
    }
}

struct Acl(NonNull<libc::c_void>);

impl Drop for Acl {
    fn drop(&mut self) {
        // SAFETY: This value owns the ACL returned by acl_get_fd_np.
        unsafe { acl_free(self.0.as_ptr()) };
    }
}

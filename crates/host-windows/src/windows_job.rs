use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::process::Child;
use std::ptr;

// Layouts of winnt.h's JOBOBJECT_BASIC_LIMIT_INFORMATION, IO_COUNTERS (six
// ULONGLONGs), and JOBOBJECT_EXTENDED_LIMIT_INFORMATION. All fields are required
// even though the demo only sets JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.
#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    per_process_time: i64,
    per_job_time: i64,
    flags: u32,
    minimum_working_set: usize,
    maximum_working_set: usize,
    active_process_limit: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}

#[repr(C)]
#[derive(Default)]
struct ExtendedLimits {
    basic: BasicLimits,
    io_counters: [u64; 6],
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> RawHandle;
    fn SetInformationJobObject(job: RawHandle, class: i32, info: *const c_void, len: u32) -> i32;
    fn AssignProcessToJobObject(job: RawHandle, process: RawHandle) -> i32;
}

pub struct KillOnClose(OwnedHandle);

impl KillOnClose {
    pub fn new() -> io::Result<Self> {
        // Null inputs request an unnamed, non-inheritable job handle.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // CreateJobObjectW transfers one handle to the caller; OwnedHandle closes it once.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
        let limits = ExtendedLimits {
            basic: BasicLimits {
                flags: 0x2000,
                ..BasicLimits::default()
            },
            ..ExtendedLimits::default()
        };
        // Information class 9 expects a complete JOBOBJECT_EXTENDED_LIMIT_INFORMATION.
        let success = unsafe {
            SetInformationJobObject(
                raw,
                9,
                ptr::from_ref(&limits).cast(),
                size_of::<ExtendedLimits>() as u32,
            )
        };
        if success == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(job)
        }
    }

    pub fn attach(&self, child: &Child) -> io::Result<()> {
        // Both handles outlive the immediate call. The job keeps its own membership.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

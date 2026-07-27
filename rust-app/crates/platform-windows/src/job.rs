use std::{ffi::c_void, mem, os::windows::io::AsRawHandle, process::Child, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    },
};

pub struct WorkerJob {
    handle: HANDLE,
}

impl WorkerJob {
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast::<c_void>(),
                mem::size_of_val(&limits).try_into().unwrap_or(u32::MAX),
            )
        };
        if configured == 0 {
            let error = std::io::Error::last_os_error().to_string();
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(error);
        }
        Ok(Self { handle })
    }

    pub fn assign(&self, child: &Child) -> Result<(), String> {
        let process = child.as_raw_handle().cast::<c_void>();
        if unsafe { AssignProcessToJobObject(self.handle, process) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
}

impl Drop for WorkerJob {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
            self.handle = ptr::null_mut();
        }
    }
}

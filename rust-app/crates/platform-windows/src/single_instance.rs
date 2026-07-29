use std::ptr;

use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE},
    System::Threading::CreateMutexW,
};

pub struct SingleInstanceGuard(HANDLE);

impl SingleInstanceGuard {
    pub fn acquire(name: &str) -> Result<Self, String> {
        let name = name.encode_utf16().chain([0]).collect::<Vec<_>>();
        let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err("Windows could not create the single-instance mutex".to_owned());
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err("StyleFlex is already running".to_owned());
        }
        Ok(Self(handle))
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
            self.0 = ptr::null_mut();
        }
    }
}

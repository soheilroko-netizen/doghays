// Windows Job Object helper.
//
// A Job Object with KillOnJobClose ensures that every process we assign to it
// is terminated by the OS when the job handle is closed — which happens when
// THIS application process exits for ANY reason (user logout, crash, or a
// "End task" in Task Manager). This prevents an orphaned sing-box.exe from
// surviving after the app is killed.
//
// Note: process exit here means the Rust process is gone, which does NOT happen
// when the user clicks the window's close button (that only hides the window to
// the tray). It DOES happen on a real kill. For the graceful "close" path the
// normal stop()/quit handler still terminates sing-box explicitly.

#[cfg(target_os = "windows")]
mod imp {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_BASIC_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// Owns a KillOnJobClose job. When dropped, the job handle is closed and
    /// every assigned process is killed by the OS.
    pub struct WinJob {
        handle: HANDLE,
    }

    unsafe impl Send for WinJob {}

    impl WinJob {
        /// Create a new kill-on-close job object.
        pub fn create() -> Result<WinJob, String> {
            // SAFETY: standard Win32 calls with valid args; we own the returned handle.
            let handle = unsafe { CreateJobObjectW(None, None) }
                .map_err(|e| format!("CreateJobObjectW failed: {e}"))?;

            let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOB_OBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    ..Default::default()
                },
                ..Default::default()
            };

            // SAFETY: handle is valid; info is a valid struct; size matches.
            unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            }
            .map_err(|e| format!("SetInformationJobObject failed: {e}"))?;

            Ok(WinJob {
                handle,
            })
        }

        /// Assign a child process (by PID) to this job.
        pub fn assign_pid(&mut self, pid: u32) -> Result<(), String> {
            // SAFETY: we only need SET_QUOTA + TERMINATE to assign into a job.
            let proc = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) }
                .map_err(|e| format!("OpenProcess({pid}) failed: {e}"))?;

            // SAFETY: both handles are valid.
            unsafe { AssignProcessToJobObject(self.handle, proc) }
                .map_err(|e| format!("AssignProcessToJobObject failed: {e}"))?;

            // We don't need the process handle anymore; close it.
            unsafe {
                let _ = CloseHandle(proc);
            }
            Ok(())
        }

        pub fn assigned(&self) -> bool {
            self.assigned
        }
    }

    impl Drop for WinJob {
        fn drop(&mut self) {
            // Closing the job handle triggers KillOnJobClose for assigned
            // processes. The handle is closed by CloseHandle regardless.
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use imp::WinJob;

#[cfg(not(target_os = "windows"))]
/// No-op on non-Windows platforms.
pub struct WinJob;

#[cfg(not(target_os = "windows"))]
impl WinJob {
    pub fn create() -> Result<WinJob, String> {
        Ok(WinJob)
    }
    pub fn assign_pid(&mut self, _pid: u32) -> Result<(), String> {
        Ok(())
    }
}

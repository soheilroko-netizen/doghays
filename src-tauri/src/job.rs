// Windows Job Object helper (raw FFI, no `windows` crate feature gates).
//
// A Job Object with KillOnJobClose ensures that every process we assign to it
// is terminated by the OS when the job handle is closed — which happens when
// THIS application process exits for ANY reason (user logout, crash, or a
// "End task" in Task Manager). This prevents an orphaned sing-box.exe from
// surviving after the app is killed.
//
// We use raw `extern "system"` FFI with self-declared structs so the code
// compiles deterministically without depending on the `windows` crate's
// modular per-type feature flags (which differ across versions and cannot be
// type-checked on a non-Windows build host).

#[cfg(target_os = "windows")]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::io::RawHandle;

    // ── Win32 constants ───────────────────────────────────────────────
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000; // 8192
    const JobObjectExtendedLimitInformation: i32 = 9;
    const PROCESS_SET_QUOTA: u32 = 0x100;
    const PROCESS_TERMINATE: u32 = 0x1;

    // ── Structs (must match the Windows ABI exactly) ──────────────────
    #[repr(C)]
    struct IO_COUNTERS {
        ReadOperationCount: u64,
        WriteOperationCount: u64,
        OtherOperationCount: u64,
        ReadTransferCount: u64,
        WriteTransferCount: u64,
        OtherTransferCount: u64,
    }

    #[repr(C)]
    struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
        PerProcessUserTimeLimit: i64,
        PerJobUserTimeLimit: i64,
        LimitFlags: u32,
        MinimumWorkingSetSize: usize,
        MaximumWorkingSetSize: usize,
        ActiveProcessLimit: u32,
        Affinity: usize,
        PriorityClass: u32,
        SchedulingClass: u32,
    }

    #[repr(C)]
    struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION,
        IoInfo: IO_COUNTERS,
        ProcessMemoryLimit: usize,
        JobMemoryLimit: usize,
        PeakProcessMemoryUsed: usize,
        PeakJobMemoryUsed: usize,
    }

    // ── kernel32 imports ──────────────────────────────────────────────
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(
            lpjobattributes: *const c_void,
            lpname: *const u16,
        ) -> *mut c_void;
        fn AssignProcessToJobObject(hjob: *mut c_void, hprocess: *mut c_void) -> i32;
        fn SetInformationJobObject(
            hjob: *mut c_void,
            jobobjectinformationclass: i32,
            lpjobobjectinformation: *const c_void,
            cbjobobjectinformationlength: u32,
        ) -> i32;
        fn CloseHandle(hobject: *mut c_void) -> i32;
        fn OpenProcess(
            dwdesiredaccess: u32,
            binherithandle: i32,
            dwprocessid: u32,
        ) -> *mut c_void;
    }

    /// Owns a KillOnJobClose job. When dropped, the job handle is closed and
    /// every assigned process is killed by the OS.
    pub struct WinJob {
        handle: *mut c_void,
    }

    unsafe impl Send for WinJob {}

    impl WinJob {
        /// Create a new kill-on-close job object.
        pub fn create() -> Result<WinJob, String> {
            // SAFETY: standard Win32 call; we pass NULL for both args.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err("CreateJobObjectW returned NULL".to_string());
            }

            let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    ..unsafe { std::mem::zeroed() }
                },
                ..unsafe { std::mem::zeroed() }
            };

            // SAFETY: handle is valid; info is a valid struct; size matches.
            let ok = unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                // Non-zero is success for these APIs; 0 means failure.
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return Err("SetInformationJobObject failed".to_string());
            }

            Ok(WinJob { handle })
        }

        /// Assign a child process (by PID) to this job.
        pub fn assign_pid(&mut self, pid: u32) -> Result<(), String> {
            // SAFETY: we only need SET_QUOTA + TERMINATE to assign into a job.
            let proc = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
            if proc.is_null() {
                return Err(format!("OpenProcess({pid}) returned NULL"));
            }
            // SAFETY: both handles are valid.
            let ok = unsafe { AssignProcessToJobObject(self.handle, proc) };
            // We don't need the process handle anymore.
            unsafe {
                let _ = CloseHandle(proc);
            }
            if ok == 0 {
                return Err("AssignProcessToJobObject failed".to_string());
            }
            Ok(())
        }
    }

    impl Drop for WinJob {
        fn drop(&mut self) {
            // Closing the job handle triggers KillOnJobClose for assigned
            // processes. The handle is closed by CloseHandle regardless.
            if !self.handle.is_null() {
                unsafe {
                    let _ = CloseHandle(self.handle);
                }
                self.handle = std::ptr::null_mut();
            }
        }
    }

    // Silence an unused-import warning if RawHandle is not referenced.
    #[allow(dead_code)]
    fn _uses(_: RawHandle) {}
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

use hashbrown::HashSet;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Mutex, OnceLock};
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::Duration;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, STILL_ACTIVE,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

const LOCK_ACQUIRE_ATTEMPTS: usize = 8;
const PROCESS_STOP_WAIT_ATTEMPTS: usize = 20;
const PROCESS_STOP_WAIT_MILLIS: u64 = 25;

#[derive(Debug)]
pub enum ProjectLoadError {
    LockedByOtherInstance { pid: u32 },
    Other(String),
}

#[derive(Debug)]
pub enum ProjectLockError {
    RegistryUnavailable,
    AlreadyLockedByThisInstance,
    AlreadyLockedByOtherInstance { pid: u32 },
    CouldNotCreate(String),
}

static LOCKED_PROJECT_FILES: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

fn locked_project_files() -> &'static Mutex<HashSet<PathBuf>> {
    LOCKED_PROJECT_FILES.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_project_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("shrimp")
                || extension.eq_ignore_ascii_case("sjson")
                || extension.eq_ignore_ascii_case("json")
        })
}

pub fn normalized_project_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn project_lock_path(path: &Path) -> PathBuf {
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("project");
    path.with_extension(format!("{extension}.lock"))
}

/// Returns the live process owning a project's lock, if any.
pub fn project_lock_owner(path: &Path) -> Result<Option<u32>, String> {
    let path = normalized_project_path(path);
    let lock_path = project_lock_path(&path);
    if !lock_path.exists() {
        return Ok(None);
    }
    let pid = read_project_lock_pid(&lock_path).ok_or_else(|| {
        format!(
            "project lock {} does not contain a valid PID",
            lock_path.display()
        )
    })?;
    if !process_is_running(pid) {
        return Err(format!(
            "project lock {} is stale (PID {pid} is not running)",
            lock_path.display()
        ));
    }
    Ok(Some(pid))
}

pub fn acquire_project_lock(path: &Path) -> Result<(), ProjectLockError> {
    if !is_project_file(path) {
        return Ok(());
    }

    let canonical_path = normalized_project_path(path);
    let lock_path = project_lock_path(&canonical_path);
    let mut locks = locked_project_files()
        .lock()
        .map_err(|_| ProjectLockError::RegistryUnavailable)?;

    if locks.contains(&canonical_path) {
        return Err(ProjectLockError::AlreadyLockedByThisInstance);
    }

    for _ in 0..LOCK_ACQUIRE_ATTEMPTS {
        if let Some(pid) = read_project_lock_pid(&lock_path) {
            if process_is_running(pid) {
                return Err(ProjectLockError::AlreadyLockedByOtherInstance { pid });
            }
            let _ = fs::remove_file(&lock_path);
        } else if lock_path.exists() {
            let _ = fs::remove_file(&lock_path);
        }
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                file.write_all(process::id().to_string().as_bytes())
                    .map_err(|error| ProjectLockError::CouldNotCreate(error.to_string()))?;
                locks.insert(canonical_path);
                return Ok(());
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(ProjectLockError::CouldNotCreate(error.to_string())),
        }
    }
    Err(ProjectLockError::CouldNotCreate(format!(
        "could not create lock for {}",
        canonical_path.display()
    )))
}

pub fn release_project_lock(path: &Path) {
    let canonical_path = normalized_project_path(path);
    let mut locks = match locked_project_files().lock() {
        Ok(locks) => locks,
        Err(_) => return,
    };
    if !locks.remove(&canonical_path) {
        return;
    }
    let _ = fs::remove_file(project_lock_path(&canonical_path));
}

pub fn clear_project_file_locks() {
    let mut locks = match locked_project_files().lock() {
        Ok(locks) => locks,
        Err(_) => return,
    };
    for path in locks.drain() {
        let _ = fs::remove_file(project_lock_path(&path));
    }
}

pub fn terminate_project_process(pid: u32) -> bool {
    if checked_pid(pid).is_none() {
        return false;
    }
    #[cfg(unix)]
    {
        if !process_is_running(pid) {
            return true;
        }
        if !send_signal_to_process(pid, libc::SIGTERM) {
            return false;
        }
        if wait_for_process_to_stop(pid) {
            return true;
        }
        if !send_signal_to_process(pid, libc::SIGKILL) {
            return false;
        }
        wait_for_process_to_stop(pid)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(unix)]
fn send_signal_to_process(pid: u32, signal: libc::c_int) -> bool {
    let Some(pid) = checked_pid(pid) else {
        return false;
    };
    // SAFETY: `libc::kill` is called with a plain PID and standard signals only.
    // This mirrors existing libc usage in the codebase and does not involve pointers.
    let result = unsafe { libc::kill(pid, signal) };
    if result == 0 {
        return true;
    }
    let Some(code) = std::io::Error::last_os_error().raw_os_error() else {
        return false;
    };
    code == libc::ESRCH
}

#[cfg(unix)]
fn wait_for_process_to_stop(pid: u32) -> bool {
    for _ in 0..PROCESS_STOP_WAIT_ATTEMPTS {
        if !process_is_running(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(PROCESS_STOP_WAIT_MILLIS));
    }
    !process_is_running(pid)
}

fn read_project_lock_pid(lock_path: &Path) -> Option<u32> {
    fs::read_to_string(lock_path)
        .ok()
        .and_then(|contents| contents.trim().parse().ok())
        .filter(|pid| checked_pid(*pid).is_some())
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    let Some(pid) = checked_pid(pid) else {
        return false;
    };
    match unsafe { libc::kill(pid, 0) } {
        0 => true,
        _ => {
            matches!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EPERM) | Some(libc::EACCES)
            )
        }
    }
}

fn checked_pid(pid: u32) -> Option<i32> {
    i32::try_from(pid).ok().filter(|pid| *pid > 0)
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    if checked_pid(pid).is_none() {
        return false;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return matches!(
            std::io::Error::last_os_error()
                .raw_os_error()
                .and_then(|code| u32::try_from(code).ok()),
            Some(ERROR_ACCESS_DENIED)
        );
    }
    let mut exit_code = 0;
    let running = unsafe { GetExitCodeProcess(handle, &mut exit_code) } != 0
        && exit_code == STILL_ACTIVE as u32;
    unsafe {
        CloseHandle(handle);
    }
    running
}

#[cfg(all(not(unix), not(windows)))]
fn process_is_running(_pid: u32) -> bool {
    false
}

use std::ffi::{CString, c_void};

#[cfg(target_os = "windows")]
use std::{ffi::c_char, sync::OnceLock};

#[cfg(target_os = "linux")]
#[link(name = "GL")]
unsafe extern "C" {
    fn glXGetProcAddressARB(proc_name: *const u8) -> *const c_void;
}

#[cfg(target_os = "macos")]
#[link(name = "OpenGL", kind = "framework")]
unsafe extern "C" {}

#[cfg(target_os = "windows")]
#[link(name = "opengl32")]
unsafe extern "system" {
    fn wglGetProcAddress(proc_name: *const c_char) -> *const c_void;
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetProcAddress(module: *mut c_void, proc_name: *const c_char) -> *const c_void;
    fn LoadLibraryA(lib_file_name: *const c_char) -> *mut c_void;
}

#[cfg(target_os = "windows")]
fn invalid_wgl_proc_address(address: *const c_void) -> bool {
    matches!(address as usize, 0 | 1 | 2 | 3 | usize::MAX)
}

#[cfg(target_os = "windows")]
fn opengl32_module() -> Option<*mut c_void> {
    static OPENGL32: OnceLock<Option<usize>> = OnceLock::new();
    OPENGL32
        .get_or_init(|| {
            let module = unsafe { LoadLibraryA(c"opengl32.dll".as_ptr()) };
            (!module.is_null()).then_some(module as usize)
        })
        .map(|module| module as *mut c_void)
}

pub fn proc_address(symbol: &str) -> *const c_void {
    let Ok(symbol) = CString::new(symbol) else {
        return std::ptr::null();
    };
    #[cfg(target_os = "linux")]
    unsafe {
        glXGetProcAddressARB(symbol.as_ptr().cast())
    }
    #[cfg(target_os = "macos")]
    unsafe {
        libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr()).cast_const()
    }
    #[cfg(target_os = "windows")]
    unsafe {
        let address = wglGetProcAddress(symbol.as_ptr().cast());
        if !invalid_wgl_proc_address(address) {
            return address;
        }
        opengl32_module()
            .map(|module| GetProcAddress(module, symbol.as_ptr().cast()))
            .filter(|address| !address.is_null())
            .unwrap_or(std::ptr::null())
    }
}

pub fn context() -> glow::Context {
    unsafe { glow::Context::from_loader_function(proc_address) }
}

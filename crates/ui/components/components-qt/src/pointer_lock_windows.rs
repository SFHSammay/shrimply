use std::ffi::c_void;

#[unsafe(no_mangle)]
pub extern "C" fn shrimply_qt_number_begin_pointer_lock(
    _display: *mut c_void,
    _surface: *mut c_void,
    _seat: *mut c_void,
) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn shrimply_qt_number_poll_pointer_lock(
    _delta_x: *mut f64,
    _delta_y: *mut f64,
) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn shrimply_qt_number_end_pointer_lock() {}

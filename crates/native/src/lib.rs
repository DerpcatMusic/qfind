use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::ptr;
use std::sync::Mutex;

use qfind_core::{Catalog, Manager, ManagerRow, SearchOpts, default_snapshot_path};

pub struct QfindManager {
    inner: Mutex<Manager>,
}

#[repr(C)]
pub struct QfindRow {
    pub name: *const c_char,
    pub path: *const c_char,
    pub bytes: u64,
    pub entries: u64,
    pub id: u32,
    pub is_dir: u8,
}

pub type QfindRowCallback = extern "C" fn(*mut c_void, *const QfindRow);
pub type QfindTextCallback = extern "C" fn(*mut c_void, *const c_char);

fn text(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: Native adapters pass a live, NUL-terminated string for the call.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

fn with_manager(
    manager: *mut QfindManager,
    operation: impl FnOnce(&mut Manager) -> qfind_core::Result<()>,
) -> i32 {
    if manager.is_null() {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: The pointer comes from `qfind_manager_open` and remains caller-owned.
        let manager = unsafe { &*manager };
        let mut inner = manager.inner.lock().map_err(|_| ())?;
        operation(&mut inner).map_err(|_| ())
    }))
    .map_or(-3, |result| if result.is_ok() { 0 } else { -2 })
}

fn emit(rows: Vec<ManagerRow>, callback: QfindRowCallback, context: *mut c_void) {
    for row in rows {
        let Ok(name) = CString::new(row.name) else {
            continue;
        };
        let Ok(path) = CString::new(row.path.to_string_lossy().as_bytes()) else {
            continue;
        };
        let ffi = QfindRow {
            name: name.as_ptr(),
            path: path.as_ptr(),
            bytes: row.bytes,
            entries: row.entries,
            id: row.id.unwrap_or(u32::MAX),
            is_dir: u8::from(row.is_dir),
        };
        callback(context, &raw const ffi);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn qfind_manager_open(initial_directory: *const c_char) -> *mut QfindManager {
    catch_unwind(AssertUnwindSafe(|| {
        let catalog = Catalog::open(default_snapshot_path()).ok()?;
        let directory = text(initial_directory).map(PathBuf::from);
        Some(Box::into_raw(Box::new(QfindManager {
            inner: Mutex::new(Manager::new(catalog, directory)),
        })))
    }))
    .ok()
    .flatten()
    .unwrap_or(ptr::null_mut())
}

/// # Safety
/// `manager` must be null or a pointer returned by [`qfind_manager_open`] that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qfind_manager_free(manager: *mut QfindManager) {
    if !manager.is_null() {
        // SAFETY: Required by this function's contract.
        drop(unsafe { Box::from_raw(manager) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn qfind_manager_navigate(manager: *mut QfindManager, path: *const c_char) -> i32 {
    let Some(path) = text(path) else { return -1 };
    with_manager(manager, |inner| {
        inner.navigate(PathBuf::from(path)).map(|_| ())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn qfind_manager_back(manager: *mut QfindManager) -> i32 {
    with_manager(manager, |inner| {
        inner.back().map(|_| ()).ok_or(qfind_core::Error::Cancelled)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn qfind_manager_forward(manager: *mut QfindManager) -> i32 {
    with_manager(manager, |inner| {
        inner
            .forward()
            .map(|_| ())
            .ok_or(qfind_core::Error::Cancelled)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn qfind_manager_directory(
    manager: *mut QfindManager,
    callback: QfindTextCallback,
    context: *mut c_void,
) -> i32 {
    with_manager(manager, |inner| {
        let path = CString::new(
            inner
                .directory()
                .map_or_else(String::new, |path| path.to_string_lossy().into_owned()),
        )
        .map_err(|_| qfind_core::Error::Cancelled)?;
        callback(context, path.as_ptr());
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn qfind_manager_rows(
    manager: *mut QfindManager,
    query: *const c_char,
    recursive: u8,
    limit: u32,
    callback: QfindRowCallback,
    context: *mut c_void,
) -> i32 {
    let query = text(query).unwrap_or_default();
    let mut rows = None;
    let status = with_manager(manager, |inner| {
        let view = inner.view(
            &query,
            recursive != 0,
            SearchOpts {
                limit: limit as usize,
                ..SearchOpts::default()
            },
        )?;
        rows = Some(view.rows);
        Ok(())
    });
    if status == 0 {
        emit(rows.unwrap_or_default(), callback, context);
    }
    status
}

#[unsafe(no_mangle)]
pub extern "C" fn qfind_manager_chart(
    manager: *mut QfindManager,
    global: u8,
    limit: u32,
    callback: QfindRowCallback,
    context: *mut c_void,
) -> i32 {
    let mut rows = None;
    let status = with_manager(manager, |inner| {
        rows = Some(inner.chart(global != 0, limit as usize)?);
        Ok(())
    });
    if status == 0 {
        emit(rows.unwrap_or_default(), callback, context);
    }
    status
}

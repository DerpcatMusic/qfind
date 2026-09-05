//! Windows entry point for `qfind`.
//! No Win32 calls here (no windows-sys dependency): UTF-8 argv handling
//! comes from the CRT, and NUL-separated output uses the Windows-specific
//! write path at the call site.
pub fn init() {}

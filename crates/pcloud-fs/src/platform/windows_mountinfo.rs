//! Win32 volume enumeration for pcloud-rs orphan-mount discovery.

#![cfg(target_os = "windows")]

use std::io;

use windows::Win32::Foundation::{ERROR_NO_MORE_FILES, HANDLE};
use windows::Win32::Storage::FileSystem::{
    FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetVolumeInformationW,
    GetVolumePathNamesForVolumeNameW,
};
use windows::core::{HRESULT, PCWSTR};

const VOLUME_NAME_CAPACITY: usize = 1024;
const FS_NAME_CAPACITY: usize = 64;

struct VolumeFindHandle(HANDLE);

impl Drop for VolumeFindHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from `FindFirstVolumeW` and this guard is
        // its unique owner. Windows permits close after enumeration ends.
        let _ = unsafe { FindVolumeClose(self.0) };
    }
}

pub(super) fn read() -> io::Result<String> {
    let mut volume_name = vec![0u16; VOLUME_NAME_CAPACITY];
    // SAFETY: `volume_name` is a writable UTF-16 buffer and remains alive
    // until the enumeration handle is closed.
    let handle = unsafe { FindFirstVolumeW(&mut volume_name) }.map_err(win32_io_error)?;
    let _guard = VolumeFindHandle(handle);
    let mut payload = String::new();

    loop {
        let volume = nul_terminated_wide(&volume_name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "FindFirst/NextVolumeW returned an unterminated volume name",
            )
        })?;

        if volume_has_pcloud_fs_name(volume) {
            for mountpoint in volume_mount_paths(volume)? {
                let escaped = escape_mountinfo_field(&mountpoint);
                payload.push_str(&format!(
                    "0 0 0:0 / {escaped} rw - fuse.pcloud-rs pcloud-rs rw\n"
                ));
            }
        }

        volume_name.fill(0);
        // SAFETY: `handle` is live and the output buffer is writable.
        match unsafe { FindNextVolumeW(handle, &mut volume_name) } {
            Ok(()) => {}
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) => break,
            Err(error) => return Err(win32_io_error(error)),
        }
    }

    Ok(payload)
}

fn volume_has_pcloud_fs_name(volume_name: &[u16]) -> bool {
    let mut fs_name = [0u16; FS_NAME_CAPACITY];
    // SAFETY: `volume_name` is NUL-terminated, and `fs_name` is writable.
    let result = unsafe {
        GetVolumeInformationW(
            PCWSTR(volume_name.as_ptr()),
            None,
            None,
            None,
            None,
            Some(&mut fs_name),
        )
    };
    if result.is_err() {
        return false;
    }
    let end = fs_name
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(fs_name.len());
    String::from_utf16_lossy(&fs_name[..end]).eq_ignore_ascii_case("pcloud-rs")
}

fn volume_mount_paths(volume_name: &[u16]) -> io::Result<Vec<String>> {
    let mut required = 0u32;
    // Windows reports the required MultiSZ length through `required` when a
    // zero-sized buffer is supplied. A volume may legitimately have no
    // mounted paths, in which case the successful/zero result maps to empty.
    // SAFETY: `volume_name` is NUL-terminated and `required` is writable.
    let sizing = unsafe {
        GetVolumePathNamesForVolumeNameW(PCWSTR(volume_name.as_ptr()), None, &mut required)
    };
    if required == 0 {
        return match sizing {
            Ok(()) => Ok(Vec::new()),
            Err(error) => Err(win32_io_error(error)),
        };
    }

    let mut paths = vec![0u16; required as usize];
    // SAFETY: both UTF-16 buffers are valid for the duration of the call.
    unsafe {
        GetVolumePathNamesForVolumeNameW(
            PCWSTR(volume_name.as_ptr()),
            Some(&mut paths),
            &mut required,
        )
    }
    .map_err(win32_io_error)?;

    Ok(parse_wide_multisz(&paths))
}

fn nul_terminated_wide(buffer: &[u16]) -> Option<&[u16]> {
    let end = buffer.iter().position(|unit| *unit == 0)?;
    Some(&buffer[..=end])
}

pub(super) fn parse_wide_multisz(buffer: &[u16]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut start = 0usize;
    while start < buffer.len() && buffer[start] != 0 {
        let Some(relative_end) = buffer[start..].iter().position(|unit| *unit == 0) else {
            break;
        };
        let end = start + relative_end;
        paths.push(String::from_utf16_lossy(&buffer[start..end]));
        start = end + 1;
    }
    paths
}

pub(super) fn escape_mountinfo_field(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for character in path.chars() {
        match character {
            '\\' => escaped.push_str("\\134"),
            ' ' => escaped.push_str("\\040"),
            '\t' => escaped.push_str("\\011"),
            '\n' => escaped.push_str("\\012"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn win32_io_error(error: windows::core::Error) -> io::Error {
    io::Error::other(error.to_string())
}

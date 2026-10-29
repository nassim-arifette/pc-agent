//! The local time zone's offset at one instant, daylight saving included, from Windows itself.
use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
};

const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;

fn filetime(value: u64) -> FILETIME {
    FILETIME {
        dwLowDateTime: value as u32,
        dwHighDateTime: (value >> 32) as u32,
    }
}

fn value(time: &FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Minutes to add to UTC to get local time at `unix_ms`; `None` when Windows cannot convert it.
pub(crate) fn utc_offset_minutes(unix_ms: u64) -> Option<i32> {
    let utc_ticks = unix_ms
        .checked_mul(10_000)?
        .checked_add(UNIX_EPOCH_AS_FILETIME)?;
    let utc = filetime(utc_ticks);
    // SAFETY: every pointer is to a local, properly sized and initialised value that outlives
    // the call; a null time zone pointer asks for the current local time zone.
    unsafe {
        let mut utc_parts: SYSTEMTIME = std::mem::zeroed();
        if FileTimeToSystemTime(&utc, &mut utc_parts) == 0 {
            return None;
        }
        let mut local_parts: SYSTEMTIME = std::mem::zeroed();
        if SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc_parts, &mut local_parts) == 0 {
            return None;
        }
        let mut local: FILETIME = std::mem::zeroed();
        if SystemTimeToFileTime(&local_parts, &mut local) == 0 {
            return None;
        }
        let minutes = (value(&local) as i128 - utc_ticks as i128) / 600_000_000;
        i32::try_from(minutes).ok()
    }
}

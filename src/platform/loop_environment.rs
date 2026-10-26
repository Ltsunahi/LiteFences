//! Read-only description of this process's window station and this thread's desktop.

use windows_sys::Win32::{
    Foundation::HANDLE,
    System::{
        StationsAndDesktops::{
            GetProcessWindowStation, GetThreadDesktop, GetUserObjectInformationW, UOI_IO, UOI_NAME,
        },
        Threading::GetCurrentThreadId,
    },
};

const NAME_BUFFER_UNITS: usize = 256;
const NAME_BUFFER_BYTES: u32 = (NAME_BUFFER_UNITS * std::mem::size_of::<u16>()) as u32;

#[derive(Debug, Default)]
pub struct Context {
    pub station_name: Option<String>,
    pub desktop_name: Option<String>,
    pub receives_input: Option<bool>,
}

/// Read the current process window station and current thread desktop context.
/// The returned handles are borrowed from the system and are never closed here.
pub fn read() -> Context {
    // SAFETY: This only retrieves the calling process's existing window-station handle.
    // The handle is borrowed and is used only for the read-only information query below.
    let station = unsafe { GetProcessWindowStation() };
    // SAFETY: GetCurrentThreadId reads the ID of the calling thread.
    let thread_id = unsafe { GetCurrentThreadId() };
    // SAFETY: `thread_id` is this thread's ID, and GetThreadDesktop retrieves only its
    // existing desktop. This handle is borrowed and is not closed.
    let desktop = unsafe { GetThreadDesktop(thread_id) };

    Context {
        station_name: if station.is_null() {
            None
        } else {
            read_name(station)
        },
        desktop_name: if desktop.is_null() {
            None
        } else {
            read_name(desktop)
        },
        receives_input: if desktop.is_null() {
            None
        } else {
            read_receives_input(desktop)
        },
    }
}

fn read_name(handle: HANDLE) -> Option<String> {
    let mut buffer = [0u16; NAME_BUFFER_UNITS];
    let mut needed = 0u32;
    // SAFETY: `buffer` is writable for NAME_BUFFER_BYTES; `needed` is a valid output pointer.
    // The handle is one of the current process/thread's borrowed handles obtained above.
    // UOI_NAME only queries its name and does not transfer ownership or mutate the object.
    let ok = unsafe {
        GetUserObjectInformationW(
            handle,
            UOI_NAME,
            buffer.as_mut_ptr().cast(),
            NAME_BUFFER_BYTES,
            &mut needed,
        )
    };
    if ok == 0 {
        return None;
    }

    parse_name(&buffer, needed)
}

fn read_receives_input(handle: HANDLE) -> Option<bool> {
    let mut receives_input = 0i32;
    let mut needed = 0u32;
    // SAFETY: `receives_input` and `needed` are valid writable outputs of their declared
    // sizes. The current thread's borrowed desktop handle is queried for UOI_IO only.
    let ok = unsafe {
        GetUserObjectInformationW(
            handle,
            UOI_IO,
            (&mut receives_input as *mut i32).cast(),
            std::mem::size_of::<i32>() as u32,
            &mut needed,
        )
    };
    if ok == 0 || needed != std::mem::size_of::<i32>() as u32 {
        return None;
    }

    Some(receives_input != 0)
}

fn parse_name(buffer: &[u16; NAME_BUFFER_UNITS], needed_bytes: u32) -> Option<String> {
    if needed_bytes < std::mem::size_of::<u16>() as u32
        || needed_bytes > NAME_BUFFER_BYTES
        || !needed_bytes.is_multiple_of(std::mem::size_of::<u16>() as u32)
    {
        return None;
    }

    let units = (needed_bytes / std::mem::size_of::<u16>() as u32) as usize;
    let terminator = buffer[..units].iter().position(|&unit| unit == 0)?;
    if terminator == 0 {
        return None;
    }

    String::from_utf16(&buffer[..terminator]).ok()
}

#[cfg(test)]
mod tests {
    use super::{Context, NAME_BUFFER_UNITS, parse_name, read};

    fn buffer_with(units: &[u16]) -> [u16; NAME_BUFFER_UNITS] {
        let mut buffer = [0u16; NAME_BUFFER_UNITS];
        buffer[..units.len()].copy_from_slice(units);
        buffer
    }

    #[test]
    fn parses_nonempty_nul_terminated_utf16() {
        let buffer = buffer_with(&[b'W' as u16, b'i' as u16, 0]);
        assert_eq!(parse_name(&buffer, 6).as_deref(), Some("Wi"));
    }

    #[test]
    fn rejects_invalid_lengths_empty_names_missing_nul_and_invalid_utf16() {
        let ordinary = buffer_with(&[b'x' as u16, 0]);
        assert_eq!(parse_name(&ordinary, 0), None);
        assert_eq!(parse_name(&ordinary, 3), None);
        assert_eq!(parse_name(&ordinary, 514), None);

        let empty = buffer_with(&[0]);
        assert_eq!(parse_name(&empty, 2), None);

        let no_terminator = buffer_with(&[b'x' as u16]);
        assert_eq!(parse_name(&no_terminator, 2), None);

        let invalid_utf16 = buffer_with(&[0xD800, 0]);
        assert_eq!(parse_name(&invalid_utf16, 4), None);
    }

    #[test]
    fn context_is_defaultable_and_read_does_not_panic() {
        let _: Context = Default::default();
        let _context = read();
    }
}

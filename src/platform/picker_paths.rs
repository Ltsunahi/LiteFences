//! Bounded parser for the UTF-16 buffer returned by `GetOpenFileNameW` when
//! `OFN_EXPLORER | OFN_ALLOWMULTISELECT` is used. This module never accesses
//! the filesystem.

use litefences::core::model::{MAX_ITEMS, MAX_PATH_UNITS, MAX_TOTAL_PATH_UNITS, NativePath};

/// Errors in the picker-provided UTF-16 MULTI_SZ buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickerPathError {
    EmptyBuffer,
    BufferTooLarge,
    MissingTerminator,
    InvalidMultiString,
    TooManyPaths,
    PathBudgetExceeded,
    InvalidPath,
}

/// Parse the picker result into absolute, drive-letter paths, preserving UTF-16.
///
/// A single selection is `path\0\0`; multiple selections are
/// `directory\0name...\0\0`. Remaining capacity after the final double NUL
/// must contain only NULs. The input and output are bounded before allocation.
pub fn parse_picker_paths(buffer: &[u16]) -> Result<Vec<NativePath>, PickerPathError> {
    if buffer.is_empty() {
        return Err(PickerPathError::EmptyBuffer);
    }
    // A valid result needs at most the aggregate path budget, one separator
    // per returned path, and the final two NULs. Reject oversized input before
    // scanning or allocating, even if its prefix looks valid.
    let max_buffer = MAX_TOTAL_PATH_UNITS
        .saturating_add(MAX_ITEMS)
        .saturating_add(2);
    if buffer.len() > max_buffer {
        return Err(PickerPathError::BufferTooLarge);
    }

    let first_end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .ok_or(PickerPathError::MissingTerminator)?;
    if first_end == 0 {
        return Err(PickerPathError::InvalidMultiString);
    }
    let after_first = first_end + 1;
    let second = *buffer
        .get(after_first)
        .ok_or(PickerPathError::MissingTerminator)?;

    let mut paths = Vec::new();
    if second == 0 {
        // Single selection: the path itself must be followed by a second NUL.
        let path = make_path(&buffer[..first_end])?;
        paths.push(path);
        ensure_zero_padding(buffer, after_first + 1)?;
        return Ok(paths);
    }

    // Multi-selection: first field is the directory, remaining fields are
    // names. Require at least one name and an explicit empty field terminator.
    let directory = &buffer[..first_end];
    let mut offset = after_first;
    let mut total_units = 0usize;
    loop {
        let remaining = buffer
            .get(offset..)
            .ok_or(PickerPathError::MissingTerminator)?;
        let field_len = remaining
            .iter()
            .position(|unit| *unit == 0)
            .ok_or(PickerPathError::MissingTerminator)?;
        if field_len == 0 {
            if paths.is_empty() {
                return Err(PickerPathError::InvalidMultiString);
            }
            ensure_zero_padding(buffer, offset + 1)?;
            return Ok(paths);
        }
        if paths.len() >= MAX_ITEMS {
            return Err(PickerPathError::TooManyPaths);
        }
        let filename = &remaining[..field_len];
        if filename.iter().any(|unit| matches!(*unit, 92 | 47 | 58))
            || filename.iter().any(|unit| *unit < 32)
        {
            return Err(PickerPathError::InvalidMultiString);
        }
        let separator = usize::from(directory.last() != Some(&92));
        let path_len = directory
            .len()
            .checked_add(separator)
            .and_then(|length| length.checked_add(filename.len()))
            .ok_or(PickerPathError::PathBudgetExceeded)?;
        total_units = total_units
            .checked_add(path_len)
            .ok_or(PickerPathError::PathBudgetExceeded)?;
        if path_len > MAX_PATH_UNITS || total_units > MAX_TOTAL_PATH_UNITS {
            return Err(PickerPathError::PathBudgetExceeded);
        }
        let mut combined = Vec::with_capacity(path_len);
        combined.extend_from_slice(directory);
        if separator != 0 {
            combined.push(92);
        }
        combined.extend_from_slice(filename);
        paths.push(NativePath::from_utf16(combined).map_err(|_| PickerPathError::InvalidPath)?);
        offset = offset
            .checked_add(field_len + 1)
            .ok_or(PickerPathError::MissingTerminator)?;
    }
}

fn make_path(units: &[u16]) -> Result<NativePath, PickerPathError> {
    if units.len() > MAX_PATH_UNITS {
        return Err(PickerPathError::PathBudgetExceeded);
    }
    NativePath::from_utf16(units.to_vec()).map_err(|_| PickerPathError::InvalidPath)
}

fn ensure_zero_padding(buffer: &[u16], from: usize) -> Result<(), PickerPathError> {
    if buffer
        .get(from..)
        .ok_or(PickerPathError::MissingTerminator)?
        .iter()
        .any(|unit| *unit != 0)
    {
        return Err(PickerPathError::InvalidMultiString);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    fn result_fields(fields: &[&str]) -> Vec<u16> {
        let mut buffer = Vec::new();
        for field in fields {
            buffer.extend(units(field));
            buffer.push(0);
        }
        buffer.push(0);
        buffer
    }

    #[test]
    fn parses_single_absolute_path_and_zero_padded_capacity() {
        let mut buffer = result_fields(&[r"C:\Users\Ada\file.txt"]);
        buffer.extend([0; 8]);
        let paths = parse_picker_paths(&buffer).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].as_utf16(), units(r"C:\Users\Ada\file.txt"));
    }

    #[test]
    fn parses_multiple_names_under_directory() {
        let paths =
            parse_picker_paths(&result_fields(&[r"D:\資料", "第一.txt", "second.dat"])).unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].as_utf16(), units("D:\\資料\\第一.txt"));
        assert_eq!(paths[1].as_utf16(), units(r"D:\資料\second.dat"));
    }

    #[test]
    fn preserves_non_bmp_utf16_units() {
        let input = "C:\\Users\\🦀.txt";
        let paths = parse_picker_paths(&result_fields(&[input])).unwrap();
        assert_eq!(paths[0].as_utf16(), units(input));
    }

    #[test]
    fn rejects_empty_relative_device_and_malformed_buffers() {
        assert_eq!(parse_picker_paths(&[]), Err(PickerPathError::EmptyBuffer));
        assert_eq!(
            parse_picker_paths(&[67, 58, 92, 120]),
            Err(PickerPathError::MissingTerminator)
        );
        assert_eq!(
            parse_picker_paths(&result_fields(&[r"relative\file"])),
            Err(PickerPathError::InvalidPath)
        );
        assert_eq!(
            parse_picker_paths(&result_fields(&[r"\\?\C:\device.txt"])),
            Err(PickerPathError::InvalidPath)
        );
        assert_eq!(
            parse_picker_paths(&result_fields(&[r"C:\folder", "", "unexpected.txt"])),
            Err(PickerPathError::InvalidMultiString)
        );
        assert_eq!(
            parse_picker_paths(&[67, 58, 92, 97, 0, 0, 98]),
            Err(PickerPathError::InvalidMultiString)
        );
    }
}

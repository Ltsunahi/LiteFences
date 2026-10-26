//! Pure policy for classifying open failures and validating reference kinds.

use litefences::core::model::ReferenceKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenAction {
    Open,
    Reveal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenFailure {
    Missing,
    TypeChanged,
    NoAssociation,
    AccessDenied,
    InvalidPath,
    NoParent,
    ShellUnavailable,
    Failed,
}

/// Maps the Win32 errors relevant to opening a target to user-facing policy.
pub fn failure_from_win32(code: u32) -> OpenFailure {
    match code {
        2 | 3 => OpenFailure::Missing,
        5 => OpenFailure::AccessDenied,
        1155 => OpenFailure::NoAssociation,
        _ => OpenFailure::Failed,
    }
}

pub fn failure_from_hresult(code: i32) -> OpenFailure {
    let code = code as u32;
    if code & 0xffff_0000 == 0x8007_0000 {
        failure_from_win32(code & 0xffff)
    } else {
        OpenFailure::Failed
    }
}

/// Returns a concise Chinese message without exposing the target path.
pub fn failure_hint(failure: OpenFailure) -> &'static str {
    match failure {
        OpenFailure::Missing => "目标已不存在。",
        OpenFailure::TypeChanged => "目标类型已发生变化。",
        OpenFailure::NoAssociation => "没有可用于打开此目标的关联程序。",
        OpenFailure::AccessDenied => "没有权限打开此目标。",
        OpenFailure::InvalidPath => "本版暂不支持打开长路径；引用仍完整保留。",
        OpenFailure::NoParent => "该目标没有普通父目录，本版暂不支持定位。",
        OpenFailure::ShellUnavailable => "系统打开服务当前不可用。",
        OpenFailure::Failed => "打开目标失败。",
    }
}

/// Checks that the observed filesystem type still matches the saved kind.
pub fn validate_kind(kind: ReferenceKind, is_file: bool, is_dir: bool) -> Result<(), OpenFailure> {
    match kind {
        ReferenceKind::Directory if is_dir => Ok(()),
        ReferenceKind::File | ReferenceKind::Shortcut if is_file => Ok(()),
        _ => Err(OpenFailure::TypeChanged),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_each_reference_kind() {
        assert_eq!(validate_kind(ReferenceKind::Directory, false, true), Ok(()));
        assert_eq!(validate_kind(ReferenceKind::File, true, false), Ok(()));
        assert_eq!(validate_kind(ReferenceKind::Shortcut, true, false), Ok(()));
        assert_eq!(
            validate_kind(ReferenceKind::Directory, true, false),
            Err(OpenFailure::TypeChanged)
        );
        assert_eq!(
            validate_kind(ReferenceKind::File, false, true),
            Err(OpenFailure::TypeChanged)
        );
        assert_eq!(
            validate_kind(ReferenceKind::Shortcut, false, false),
            Err(OpenFailure::TypeChanged)
        );
    }

    #[test]
    fn maps_win32_errors_and_unknown_codes() {
        assert_eq!(failure_from_win32(2), OpenFailure::Missing);
        assert_eq!(failure_from_win32(3), OpenFailure::Missing);
        assert_eq!(failure_from_win32(5), OpenFailure::AccessDenied);
        assert_eq!(failure_from_win32(1155), OpenFailure::NoAssociation);
        assert_eq!(failure_from_win32(0), OpenFailure::Failed);
        assert_eq!(failure_from_win32(u32::MAX), OpenFailure::Failed);
    }

    #[test]
    fn every_failure_has_a_chinese_hint() {
        let failures = [
            OpenFailure::Missing,
            OpenFailure::TypeChanged,
            OpenFailure::NoAssociation,
            OpenFailure::AccessDenied,
            OpenFailure::InvalidPath,
            OpenFailure::NoParent,
            OpenFailure::ShellUnavailable,
            OpenFailure::Failed,
        ];
        for failure in failures {
            let hint = failure_hint(failure);
            assert!(!hint.is_empty());
            assert!(
                hint.chars()
                    .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
            );
        }
    }
    #[test]
    fn hresult_preserves_win32_facility_without_using_stale_last_error() {
        assert_eq!(
            failure_from_hresult(0x8007_0002_u32 as i32),
            OpenFailure::Missing
        );
        assert_eq!(
            failure_from_hresult(0x8007_0005_u32 as i32),
            OpenFailure::AccessDenied
        );
        assert_eq!(
            failure_from_hresult(0x8000_4005_u32 as i32),
            OpenFailure::Failed
        );
    }
}

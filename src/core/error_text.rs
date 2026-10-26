use super::{contracts::AppError, model::ValidationError};

/// Returns a stable, ASCII-only identifier for an application error.
pub fn error_code(error: &AppError) -> &'static str {
    match error {
        AppError::NotFound => "not_found",
        AppError::Unsupported => "unsupported",
        AppError::AccessDenied => "access_denied",
        AppError::Busy => "busy",
        AppError::Cancelled => "cancelled",
        AppError::InvalidIcon => "invalid_icon",
        AppError::Io => "io",
        AppError::StorageReadOnly => "read_only",
        AppError::Conflict => "conflict",
        AppError::ConfigTooLarge => "too_large",
        AppError::InvalidConfig => "invalid_config",
        AppError::Platform(_) => "platform",
        AppError::InvalidData(_) => "invalid_data",
    }
}

/// Returns a concise, actionable Simplified Chinese hint for an application error.
pub fn error_hint(error: &AppError) -> &'static str {
    match error {
        AppError::NotFound => "请确认目标仍然存在。",
        AppError::Unsupported => "请使用受支持的文件或操作。",
        AppError::AccessDenied => "请检查文件权限后重试。",
        AppError::Busy => "配置被占用，请关闭其他实例后重新启动。",
        AppError::Cancelled => "操作已取消，请重新发起。",
        AppError::InvalidIcon => "请重新加载图标。",
        AppError::Io => "请检查存储设备后重试。",
        AppError::StorageReadOnly => "只读保护，请保留原文件并检查版本或恢复状态。",
        AppError::Conflict => "文件已变更，请保留现场并重新启动检查。",
        AppError::ConfigTooLarge => "配置超过大小上限，请保留原文件并检查内容。",
        AppError::InvalidConfig => "请检查配置文件后重试。",
        AppError::Platform(_) => "平台操作失败，请重试。",
        AppError::InvalidData(ValidationError::DuplicateTarget) => {
            "所选路径已在该分组中，未重复添加。"
        }
        AppError::InvalidData(_) => "数据无效，请检查内容后重试。",
    }
}

#[cfg(test)]
mod tests {
    use super::{error_code, error_hint};
    use crate::core::{contracts::AppError, model::ValidationError};

    #[test]
    fn hints_are_nonempty() {
        let errors = [
            AppError::NotFound,
            AppError::Unsupported,
            AppError::AccessDenied,
            AppError::Busy,
            AppError::Cancelled,
            AppError::InvalidIcon,
            AppError::Io,
            AppError::StorageReadOnly,
            AppError::Conflict,
            AppError::ConfigTooLarge,
            AppError::InvalidConfig,
            AppError::Platform(-12345),
            AppError::InvalidData(ValidationError::InvalidPath),
        ];

        assert!(
            errors
                .iter()
                .all(|error| !error_hint(error).trim().is_empty())
        );
    }

    #[test]
    fn codes_are_stable_and_distinct() {
        let errors = [
            AppError::NotFound,
            AppError::Unsupported,
            AppError::AccessDenied,
            AppError::Busy,
            AppError::Cancelled,
            AppError::InvalidIcon,
            AppError::Io,
            AppError::StorageReadOnly,
            AppError::Conflict,
            AppError::ConfigTooLarge,
            AppError::InvalidConfig,
            AppError::Platform(7),
            AppError::InvalidData(ValidationError::InvalidPath),
        ];
        let codes: Vec<_> = errors.iter().map(error_code).collect();

        assert_eq!(
            codes
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            codes.len()
        );
        assert!(codes.iter().all(|code| code.is_ascii()));
        assert_eq!(error_code(&AppError::NotFound), "not_found");
        assert_eq!(error_code(&AppError::StorageReadOnly), "read_only");
    }

    #[test]
    fn platform_details_are_not_leaked() {
        let first = AppError::Platform(1);
        let second = AppError::Platform(-9876);

        assert_eq!(error_code(&first), "platform");
        assert_eq!(error_code(&second), "platform");
        assert_eq!(error_hint(&first), error_hint(&second));
        assert!(!error_hint(&first).contains("9876"));
    }

    #[test]
    fn invalid_data_uses_the_existing_variant_without_details() {
        let error = AppError::InvalidData(ValidationError::InvalidPath);

        assert_eq!(error_code(&error), "invalid_data");
        assert_eq!(error_hint(&error), "数据无效，请检查内容后重试。");
        assert!(!error_hint(&error).contains("InvalidPath"));
    }

    #[test]
    fn duplicate_reference_hint_is_specific_without_exposing_a_path() {
        let error = AppError::InvalidData(ValidationError::DuplicateTarget);

        assert_eq!(error_code(&error), "invalid_data");
        assert_eq!(error_hint(&error), "所选路径已在该分组中，未重复添加。");
    }
}

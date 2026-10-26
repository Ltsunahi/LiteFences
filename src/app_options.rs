use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageSmoke {
    Writable,
    ReadOnly,
    Error,
    Group,
    Rename,
    UiRename,
    Move,
    UiMove,
    UiDrag,
    Resize,
    UiResize,
    UiResizeDrag,
    Delete,
    UiDelete,
    Color,
    UiColor,
    Fold,
    UiFold,
    Add,
    UiAdd,
    UiShortcut,
    UiFolder,
    UiDrop,
    UiDropFolder,
    UiDropMixed,
    UiDropShortcut,
    UiRemove,
    UiUndoRemove,
    UiUndoDelete,
    UiTransfer,
    UiTransferCancel,
    UiTransferOutside,
    UiTransferCapture,
}
#[derive(Default)]
pub struct Options {
    pub smoke: bool,
    pub data_dir: Option<PathBuf>,
    pub storage_smoke: Option<StorageSmoke>,
}
impl Options {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Option<Self> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--smoke-test" && !options.smoke {
                options.smoke = true;
            } else if cfg!(feature = "config-store")
                && arg == "--data-dir"
                && options.data_dir.is_none()
            {
                let path = PathBuf::from(args.next()?);
                if !path.is_absolute() {
                    return None;
                }
                options.data_dir = Some(path);
            } else if cfg!(feature = "config-store")
                && arg == "--storage-smoke"
                && options.storage_smoke.is_none()
            {
                options.storage_smoke = Some(match args.next()?.to_str()? {
                    "writable" => StorageSmoke::Writable,
                    "readonly" => StorageSmoke::ReadOnly,
                    "error" => StorageSmoke::Error,
                    "group" => StorageSmoke::Group,
                    "rename" => StorageSmoke::Rename,
                    "ui-rename" => StorageSmoke::UiRename,
                    "move" => StorageSmoke::Move,
                    "ui-move" => StorageSmoke::UiMove,
                    "ui-drag" => StorageSmoke::UiDrag,
                    "resize" => StorageSmoke::Resize,
                    "ui-resize" => StorageSmoke::UiResize,
                    "ui-resize-drag" => StorageSmoke::UiResizeDrag,
                    "delete" => StorageSmoke::Delete,
                    "ui-delete" => StorageSmoke::UiDelete,
                    "color" => StorageSmoke::Color,
                    "ui-color" => StorageSmoke::UiColor,
                    "fold" => StorageSmoke::Fold,
                    "ui-fold" => StorageSmoke::UiFold,
                    "add" => StorageSmoke::Add,
                    "ui-add" => StorageSmoke::UiAdd,
                    "ui-shortcut" => StorageSmoke::UiShortcut,
                    "ui-folder" => StorageSmoke::UiFolder,
                    "ui-drop" => StorageSmoke::UiDrop,
                    "ui-drop-folder" => StorageSmoke::UiDropFolder,
                    "ui-drop-mixed" => StorageSmoke::UiDropMixed,
                    "ui-drop-shortcut" => StorageSmoke::UiDropShortcut,
                    "ui-remove" => StorageSmoke::UiRemove,
                    "ui-undo-remove" => StorageSmoke::UiUndoRemove,
                    "ui-undo-delete" => StorageSmoke::UiUndoDelete,
                    "ui-transfer" => StorageSmoke::UiTransfer,
                    "ui-transfer-cancel" => StorageSmoke::UiTransferCancel,
                    "ui-transfer-outside" => StorageSmoke::UiTransferOutside,
                    "ui-transfer-capture" => StorageSmoke::UiTransferCapture,
                    _ => return None,
                });
            } else {
                return None;
            }
        }
        if options.smoke && (options.data_dir.is_some() || options.storage_smoke.is_some()) {
            return None;
        }
        if options.storage_smoke.is_some() && options.data_dir.is_none() {
            return None;
        }
        Some(options)
    }
    pub fn hidden(&self) -> bool {
        self.smoke || self.storage_smoke.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "config-store")]
    #[test]
    fn reference_transfer_smokes_require_an_explicit_hidden_directory() {
        for (name, expected) in [
            ("ui-transfer", StorageSmoke::UiTransfer),
            ("ui-transfer-cancel", StorageSmoke::UiTransferCancel),
            ("ui-transfer-outside", StorageSmoke::UiTransferOutside),
            ("ui-transfer-capture", StorageSmoke::UiTransferCapture),
        ] {
            let options = parse(&["--data-dir", "E:\\test", "--storage-smoke", name]).unwrap();
            assert_eq!(options.storage_smoke, Some(expected));
            assert!(options.hidden());
            assert!(parse(&["--storage-smoke", name]).is_none());
            assert!(
                parse(&[
                    "--data-dir",
                    "E:\\test",
                    "--storage-smoke",
                    name,
                    "--show-window"
                ])
                .is_none()
            );
        }
    }
    fn parse(args: &[&str]) -> Option<Options> {
        Options::parse(args.iter().map(OsString::from))
    }
    #[test]
    fn smoke_never_accidentally_uses_user_directory() {
        assert!(parse(&["--smoke-test"]).unwrap().hidden());
        assert!(parse(&["--smoke-test", "--data-dir", "E:\\test"]).is_none());
        assert!(parse(&["--storage-smoke", "writable"]).is_none());
        assert!(parse(&["--data-dir", "relative"]).is_none());
        assert!(parse(&["--unknown"]).is_none());
        assert!(parse(&["--smoke-test", "--smoke-test"]).is_none());
    }
    #[cfg(feature = "config-store")]
    #[test]
    fn storage_smoke_requires_explicit_absolute_directory() {
        let args = parse(&[
            "--data-dir",
            "E:\\test with spaces",
            "--storage-smoke",
            "readonly",
        ])
        .unwrap();
        assert_eq!(args.storage_smoke, Some(StorageSmoke::ReadOnly));
        assert_eq!(args.data_dir, Some(PathBuf::from("E:\\test with spaces")));
        let rename = parse(&["--data-dir", "E:\\test", "--storage-smoke", "rename"]);
        assert_eq!(rename.unwrap().storage_smoke, Some(StorageSmoke::Rename));
        let ui_move = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-move"]);
        assert_eq!(ui_move.unwrap().storage_smoke, Some(StorageSmoke::UiMove));
        let ui_drag = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-drag"]);
        assert_eq!(ui_drag.unwrap().storage_smoke, Some(StorageSmoke::UiDrag));
        let resize = parse(&["--data-dir", "E:\\test", "--storage-smoke", "resize"]);
        assert_eq!(resize.unwrap().storage_smoke, Some(StorageSmoke::Resize));
        let ui_resize = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-resize"]);
        assert_eq!(
            ui_resize.unwrap().storage_smoke,
            Some(StorageSmoke::UiResize)
        );
        let ui_resize_drag = parse(&[
            "--data-dir",
            "E:\\test",
            "--storage-smoke",
            "ui-resize-drag",
        ]);
        assert_eq!(
            ui_resize_drag.unwrap().storage_smoke,
            Some(StorageSmoke::UiResizeDrag)
        );
        let delete = parse(&["--data-dir", "E:\\test", "--storage-smoke", "delete"]);
        assert_eq!(delete.unwrap().storage_smoke, Some(StorageSmoke::Delete));
        let ui_delete = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-delete"]);
        assert_eq!(
            ui_delete.unwrap().storage_smoke,
            Some(StorageSmoke::UiDelete)
        );
        let color = parse(&["--data-dir", "E:\\test", "--storage-smoke", "color"]);
        assert_eq!(color.unwrap().storage_smoke, Some(StorageSmoke::Color));
        let ui_color = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-color"]);
        assert_eq!(ui_color.unwrap().storage_smoke, Some(StorageSmoke::UiColor));
        let fold = parse(&["--data-dir", "E:\\test", "--storage-smoke", "fold"]);
        assert_eq!(fold.unwrap().storage_smoke, Some(StorageSmoke::Fold));
        let ui_fold = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-fold"]);
        assert_eq!(ui_fold.unwrap().storage_smoke, Some(StorageSmoke::UiFold));
        let add = parse(&["--data-dir", "E:\\test", "--storage-smoke", "add"]);
        assert_eq!(add.unwrap().storage_smoke, Some(StorageSmoke::Add));
        let ui_add = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-add"]);
        assert_eq!(ui_add.unwrap().storage_smoke, Some(StorageSmoke::UiAdd));
        let ui_shortcut = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-shortcut"]);
        assert_eq!(
            ui_shortcut.unwrap().storage_smoke,
            Some(StorageSmoke::UiShortcut)
        );
        let ui_folder = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-folder"]);
        assert_eq!(
            ui_folder.unwrap().storage_smoke,
            Some(StorageSmoke::UiFolder)
        );
        let ui_drop = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-drop"]);
        assert_eq!(ui_drop.unwrap().storage_smoke, Some(StorageSmoke::UiDrop));
        let ui_drop_folder = parse(&[
            "--data-dir",
            "E:\\test",
            "--storage-smoke",
            "ui-drop-folder",
        ]);
        assert_eq!(
            ui_drop_folder.unwrap().storage_smoke,
            Some(StorageSmoke::UiDropFolder)
        );
        let ui_drop_mixed = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-drop-mixed"]);
        assert_eq!(
            ui_drop_mixed.unwrap().storage_smoke,
            Some(StorageSmoke::UiDropMixed)
        );
        let ui_drop_shortcut = parse(&[
            "--data-dir",
            "E:\\test",
            "--storage-smoke",
            "ui-drop-shortcut",
        ]);
        assert_eq!(
            ui_drop_shortcut.unwrap().storage_smoke,
            Some(StorageSmoke::UiDropShortcut)
        );
        let ui_remove = parse(&["--data-dir", "E:\\test", "--storage-smoke", "ui-remove"]);
        assert_eq!(
            ui_remove.unwrap().storage_smoke,
            Some(StorageSmoke::UiRemove)
        );
        let ui_undo_remove = parse(&[
            "--data-dir",
            "E:\\test",
            "--storage-smoke",
            "ui-undo-remove",
        ]);
        assert_eq!(
            ui_undo_remove.unwrap().storage_smoke,
            Some(StorageSmoke::UiUndoRemove)
        );
        assert!(
            parse(&[
                "--data-dir",
                "E:\\test",
                "--storage-smoke",
                "ui-undo-remove",
                "--show-window",
            ])
            .is_none()
        );
        let ui_undo_delete = parse(&[
            "--data-dir",
            "E:\\test",
            "--storage-smoke",
            "ui-undo-delete",
        ]);
        assert_eq!(
            ui_undo_delete.unwrap().storage_smoke,
            Some(StorageSmoke::UiUndoDelete)
        );
        assert!(
            parse(&[
                "--data-dir",
                "E:\\test",
                "--storage-smoke",
                "ui-undo-delete",
                "--show-window",
            ])
            .is_none()
        );
    }
}

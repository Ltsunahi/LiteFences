//! Modern, single-directory picker. The returned path is a reference only.

use litefences::core::model::{MAX_PATH_UNITS, NativePath};
use windows::{
    Win32::{
        Foundation::HWND,
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoTaskMemFree, CoUninitialize,
        },
        UI::Shell::{
            FOS_DONTADDTORECENT, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
            FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
        },
    },
    core::{PWSTR, w},
};

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: created by a successful CoInitializeEx on this UI thread.
        unsafe { CoUninitialize() }
    }
}

struct CoOwnedString(PWSTR);
impl Drop for CoOwnedString {
    fn drop(&mut self) {
        // SAFETY: GetDisplayName allocates this with the COM task allocator.
        unsafe { CoTaskMemFree(Some(self.0.as_ptr().cast())) }
    }
}

/// Returns None only when the user cancels. Unsupported/nonlocal paths are rejected.
pub unsafe fn choose_folder(owner: HWND) -> Result<Option<NativePath>, String> {
    // SAFETY: UI thread owns `owner`, and no Session borrow spans this modal call.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|error| format!("无法初始化文件夹选择器（代码 {}）。", error.code().0))?;
        let _apartment = Apartment;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("无法创建文件夹选择器（代码 {}）。", error.code().0))?;
        let options = dialog
            .GetOptions()
            .map_err(|error| format!("无法读取文件夹选择选项（代码 {}）。", error.code().0))?;
        dialog
            .SetOptions(
                options
                    | FOS_PICKFOLDERS
                    | FOS_FORCEFILESYSTEM
                    | FOS_PATHMUSTEXIST
                    | FOS_DONTADDTORECENT,
            )
            .map_err(|error| format!("无法设置文件夹选择选项（代码 {}）。", error.code().0))?;
        dialog
            .SetTitle(w!("选择要添加为引用的文件夹（源目录不会移动）"))
            .map_err(|error| format!("无法设置文件夹选择标题（代码 {}）。", error.code().0))?;
        if let Err(error) = dialog.Show(Some(owner)) {
            return if error.code().0 == 0x8007_04c7_u32 as i32 {
                Ok(None)
            } else {
                Err(format!("文件夹选择器出错（代码 {}）。", error.code().0))
            };
        }
        let item = dialog
            .GetResult()
            .map_err(|error| format!("无法读取所选文件夹（代码 {}）。", error.code().0))?;
        let path = CoOwnedString(
            item.GetDisplayName(SIGDN_FILESYSPATH)
                .map_err(|error| format!("所选文件夹没有本地路径（代码 {}）。", error.code().0))?,
        );
        if path.0.is_null() || path.0.len() > MAX_PATH_UNITS {
            return Err("所选文件夹路径过长或无效。".into());
        }
        NativePath::from_utf16(path.0.as_wide().to_vec())
            .map(Some)
            .map_err(|_| "当前仅支持本地盘符下的规范文件夹路径。".into())
    }
}

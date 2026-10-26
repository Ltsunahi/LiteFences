//! 自抓屏：把浮层窗口当前画面存成 BMP，用于远程排障。
//!
//! 走的是屏幕 DC 的 BitBlt，看到的就是用户屏幕上真实合成后的像素，
//! 不依赖窗口自己的绘制表面。

use std::fs::File;
use std::io::Write;
use std::path::Path;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;

use crate::log;

pub fn capture_window(hwnd: HWND, path: &Path) -> Result<(), String> {
    let mut rect = RECT::default();
    // SAFETY: 句柄有效，rect 由系统填充。
    unsafe { GetWindowRect(hwnd, &mut rect) };
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return Err(format!("窗口尺寸无效 {width}x{height}"));
    }

    // SAFETY: 所有 GDI 对象在函数末尾成对释放。
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err("无法取得屏幕 DC".into());
        }
        let memory = CreateCompatibleDC(screen);
        let header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            memory,
            &header,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(memory);
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err("无法创建 DIB".into());
        }
        let previous = SelectObject(memory, bitmap as _);
        let copied = BitBlt(
            memory,
            0,
            0,
            width,
            height,
            screen,
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        );
        // 必须在释放 DIB 之前把像素写出去，否则 bits 已失效。
        if copied != 0 {
            write_bmp(path, width, height, bits)?;
        }
        SelectObject(memory, previous);
        DeleteObject(bitmap as _);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
        if copied == 0 {
            return Err("BitBlt 失败".into());
        }
    }
    log::info(&format!("已保存面板截图：{}", path.display()));
    Ok(())
}

unsafe fn write_bmp(
    path: &Path,
    width: i32,
    height: i32,
    bits: *const std::ffi::c_void,
) -> Result<(), String> {
    let stride = width as usize * 4;
    let size = stride * height as usize;
    // SAFETY: bits 指向 size 字节的 DIB 像素区（GDI 自下而上排列）。
    let pixels = unsafe { std::slice::from_raw_parts(bits as *const u8, size) };
    let mut file = File::create(path).map_err(|error| format!("创建文件失败：{error}"))?;
    let file_size = 54u32 + size as u32;
    let mut header = Vec::with_capacity(54);
    header.extend_from_slice(b"BM");
    header.extend_from_slice(&file_size.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&54u32.to_le_bytes());
    header.extend_from_slice(&40u32.to_le_bytes());
    header.extend_from_slice(&width.to_le_bytes());
    header.extend_from_slice(&height.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&32u16.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&(size as u32).to_le_bytes());
    header.extend_from_slice(&2835i32.to_le_bytes());
    header.extend_from_slice(&2835i32.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    file.write_all(&header)
        .and_then(|()| file.write_all(pixels))
        .map_err(|error| format!("写入失败：{error}"))
}

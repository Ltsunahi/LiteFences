//! 图标像素缓存：把 48×48 的位图存成二进制文件，启动时直接重建 HICON。
//!
//! 上一版只缓存了「系统图标列表索引」，实测热缓存仍要 818ms —— 瓶颈是
//! `ImageList_GetIcon` 每次都要解码一张位图。这里干脆把像素本身存下来：
//! 启动时 `CreateDIBSection + CreateIconIndirect` 直接重建，绕开系统图像列表。
//!
//! 文件格式（小端）：
//!   "LFPX" | u16 版本 | u16 尺寸 | u32 条目数
//!   每条： u16 路径字节数 | 路径 UTF-8 | u32 修改时间(秒) | u32 文件大小 | 48*48*4 像素
//!
//! 失效判断和索引缓存一致：修改时间 + 文件大小都要对得上。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DI_NORMAL, DrawIconEx, HICON, ICONINFO,
};

const MAGIC: &[u8; 4] = b"LFPX";
const FORMAT_VERSION: u16 = 1;
/// 缓存尺寸：和系统 EXTRALARGE 列表（48px）一致。
/// 试过 64px/JUMBO，但很多小图标在 256px 列表里不存在，会变成空白，只能退回 48。
pub const SIZE: i32 = 48;
const PIXELS: usize = (SIZE as usize) * (SIZE as usize) * 4;

pub struct PixelCache {
    map: HashMap<String, (u64, u64, Vec<u8>)>,
    dirty: bool,
    hits: usize,
    misses: usize,
}

fn cache_path() -> PathBuf {
    crate::log::base_dir().join("icons.bin")
}

fn key_of(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn bitmap_info() -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE,
            // 负数表示自上而下，省得再翻转一遍。
            biHeight: -SIZE,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 把 HICON 画进一块 32bpp DIB，取出 BGRA 像素。
fn snapshot(icon: HICON) -> Option<Vec<u8>> {
    if icon.is_null() {
        return None;
    }
    // SAFETY: 创建的 DC/位图在本函数内释放；bits 由 CreateDIBSection 返回，大小固定。
    unsafe {
        let screen = GetDC(ptr::null_mut());
        if screen.is_null() {
            return None;
        }
        let memory = CreateCompatibleDC(screen);
        let mut bits: *mut std::ffi::c_void = ptr::null_mut();
        let info = bitmap_info();
        let dib = CreateDIBSection(
            screen,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            ptr::null_mut(),
            0,
        );
        if dib.is_null() || bits.is_null() {
            DeleteDC(memory);
            ReleaseDC(ptr::null_mut(), screen);
            return None;
        }
        let previous = SelectObject(memory, dib as _);
        DrawIconEx(
            memory,
            0,
            0,
            icon,
            SIZE,
            SIZE,
            0,
            ptr::null_mut(),
            DI_NORMAL,
        );
        let mut pixels = vec![0u8; PIXELS];
        ptr::copy_nonoverlapping(bits as *const u8, pixels.as_mut_ptr(), PIXELS);
        SelectObject(memory, previous);
        DeleteObject(dib as _);
        DeleteDC(memory);
        ReleaseDC(ptr::null_mut(), screen);
        Some(pixels)
    }
}

/// 用缓存里的像素重建 HICON；32bpp 带 alpha，掩码用全 0 位图。
fn rebuild(pixels: &[u8]) -> HICON {
    if pixels.len() != PIXELS {
        return ptr::null_mut();
    }
    // SAFETY: 位图在本函数内创建；CreateIconIndirect 会复制内容，之后可以安全释放。
    unsafe {
        let mut bits: *mut std::ffi::c_void = ptr::null_mut();
        let info = bitmap_info();
        let dib = CreateDIBSection(
            ptr::null_mut(),
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            ptr::null_mut(),
            0,
        );
        if dib.is_null() || bits.is_null() {
            return ptr::null_mut();
        }
        ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, PIXELS);
        let mask = CreateBitmap(SIZE, SIZE, 1, 1, ptr::null());
        let icon_info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: dib,
        };
        let icon = CreateIconIndirect(&icon_info);
        if !mask.is_null() {
            DeleteObject(mask as _);
        }
        DeleteObject(dib as _);
        icon
    }
}

impl PixelCache {
    pub fn load() -> Self {
        let mut map = HashMap::new();
        if let Ok(bytes) = std::fs::read(cache_path()) {
            parse(&bytes, &mut map);
        }
        Self {
            map,
            dirty: false,
            hits: 0,
            misses: 0,
        }
    }

    /// 命中就用像素重建图标；返回 None 表示需要回退到 shell 取图标。
    pub fn lookup(&mut self, path: &Path, mtime: u64, size: u64) -> Option<HICON> {
        if mtime == 0 {
            self.misses += 1;
            return None;
        }
        let Some((known_mtime, known_size, pixels)) = self.map.get(&key_of(path)) else {
            self.misses += 1;
            return None;
        };
        if *known_mtime != mtime || *known_size != size {
            self.misses += 1;
            return None;
        }
        let icon = rebuild(pixels);
        if icon.is_null() {
            self.misses += 1;
            return None;
        }
        self.hits += 1;
        Some(icon)
    }

    /// 把图标画出来存进缓存；失败就跳过（下次继续走 shell）。
    pub fn remember(&mut self, path: &Path, mtime: u64, size: u64, icon: HICON) {
        if mtime == 0 {
            return;
        }
        let Some(pixels) = snapshot(icon) else {
            return;
        };
        self.map.insert(key_of(path), (mtime, size, pixels));
        self.dirty = true;
    }

    pub fn hits(&self) -> usize {
        self.hits
    }

    pub fn misses(&self) -> usize {
        self.misses
    }

    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let mut entries: Vec<(&String, &(u64, u64, Vec<u8>))> = self.map.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let mut out = Vec::with_capacity(16 + entries.len() * (PIXELS + 64));
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(SIZE as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (path, (mtime, size, pixels)) in entries {
            let bytes = path.as_bytes();
            let len = bytes.len().min(u16::MAX as usize);
            out.extend_from_slice(&(len as u16).to_le_bytes());
            out.extend_from_slice(&bytes[..len]);
            out.extend_from_slice(&(*mtime as u32).to_le_bytes());
            out.extend_from_slice(&(*size as u32).to_le_bytes());
            out.extend_from_slice(pixels);
        }
        let path = cache_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let temp = path.with_extension("bin.tmp");
        if std::fs::write(&temp, &out).is_ok() {
            let _ = std::fs::rename(&temp, &path);
        }
    }
}

fn parse(bytes: &[u8], map: &mut HashMap<String, (u64, u64, Vec<u8>)>) {
    if bytes.len() < 12 || &bytes[..4] != MAGIC {
        return;
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let size = u16::from_le_bytes([bytes[6], bytes[7]]);
    if version != FORMAT_VERSION || size as i32 != SIZE {
        return;
    }
    let count = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let mut cursor = 12;
    for _ in 0..count {
        if cursor + 2 > bytes.len() {
            return;
        }
        let len = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]) as usize;
        cursor += 2;
        if cursor + len + 8 + PIXELS > bytes.len() {
            return;
        }
        let Ok(path) = std::str::from_utf8(&bytes[cursor..cursor + len]) else {
            return;
        };
        let path = path.to_string();
        cursor += len;
        let mtime = u32::from_le_bytes([
            bytes[cursor],
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
        ]) as u64;
        cursor += 4;
        let size = u32::from_le_bytes([
            bytes[cursor],
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
        ]) as u64;
        cursor += 4;
        let pixels = bytes[cursor..cursor + PIXELS].to_vec();
        cursor += PIXELS;
        map.insert(path, (mtime, size, pixels));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_garbage() {
        let mut map = HashMap::new();
        parse(b"nope", &mut map);
        assert!(map.is_empty());
        parse(&[b'X'; 32], &mut map);
        assert!(map.is_empty());
    }

    #[test]
    fn format_round_trips() {
        let mut map = HashMap::new();
        map.insert("c:\\a.lnk".to_string(), (5u64, 6u64, vec![7u8; PIXELS]));
        // 手工拼一份和 save 相同格式的字节，确认 parse 能读回
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(SIZE as u16).to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        let path = b"c:\\a.lnk";
        out.extend_from_slice(&(path.len() as u16).to_le_bytes());
        out.extend_from_slice(path);
        out.extend_from_slice(&5u32.to_le_bytes());
        out.extend_from_slice(&6u32.to_le_bytes());
        out.extend_from_slice(&[7u8; PIXELS]);
        let mut parsed = HashMap::new();
        parse(&out, &mut parsed);
        assert_eq!(parsed.len(), 1);
        let (mtime, size, pixels) = parsed.get("c:\\a.lnk").unwrap();
        assert_eq!((*mtime, *size), (5, 6));
        assert_eq!(pixels.len(), PIXELS);
    }
}

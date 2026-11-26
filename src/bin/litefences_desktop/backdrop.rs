//! 磨砂底板：唤出面板前先抓一张桌面，缩小后模糊，再放大铺满。
//!
//! 为什么不用系统亚克力：在部分机器上，透明窗口里 GDI 画出的文字会在 DWM 合成时
//! 被整段丢弃（图标是位图所以还在）。自己画一张不透明的模糊底，观感接近，文字稳定。

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_OVER, AlphaBlend, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BitBlt,
    CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, HALFTONE, HDC, ReleaseDC, SRCCOPY, SelectObject,
    SetStretchBltMode, StretchBlt,
};

/// 缩小倍数：越大越省事，模糊越“糊”。
const DOWNSCALE: i32 = 10;
/// 模糊半径（在小图上做）。
const BLUR_RADIUS: i32 = 2;

/// 唤出瞬间的整屏原图。入场动画每帧先贴它当底，再按不透明度叠面板：
/// 既不会第一帧先黑一下（窗口新建时表面是空的），淡入也不会一帧帧叠加跑偏。
pub struct ScreenBase {
    dc: HDC,
    bitmap: isize,
    width: i32,
    height: i32,
}

impl ScreenBase {
    pub fn capture(area: RECT) -> Option<Self> {
        let width = area.right - area.left;
        let height = area.bottom - area.top;
        if width <= 0 || height <= 0 {
            return None;
        }
        // SAFETY: 所有 GDI 对象在函数内成对获取/释放。
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            if screen.is_null() {
                return None;
            }
            let dc = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            if dc.is_null() || bitmap.is_null() {
                if !dc.is_null() {
                    DeleteDC(dc);
                }
                if !bitmap.is_null() {
                    DeleteObject(bitmap as _);
                }
                ReleaseDC(std::ptr::null_mut(), screen);
                return None;
            }
            let previous = SelectObject(dc, bitmap as _);
            BitBlt(
                dc, 0, 0, width, height, screen, area.left, area.top, SRCCOPY | CAPTUREBLT,
            );
            SelectObject(dc, previous);
            ReleaseDC(std::ptr::null_mut(), screen);
            Some(Self {
                dc,
                bitmap: bitmap as isize,
                width,
                height,
            })
        }
    }

    /// 原样贴回窗口，作为这一帧的底。
    pub fn blit(&self, hdc: HDC) {
        // SAFETY: 两个 DC 都有效；尺寸来自创建时的屏幕区域。
        unsafe {
            BitBlt(hdc, 0, 0, self.width, self.height, self.dc, 0, 0, SRCCOPY);
        }
    }
}

impl Drop for ScreenBase {
    fn drop(&mut self) {
        // SAFETY: 位图与 DC 由本结构独占，只释放一次。
        unsafe {
            if !self.dc.is_null() {
                DeleteDC(self.dc);
            }
            if self.bitmap != 0 {
                DeleteObject(self.bitmap as _);
            }
        }
    }
}

pub struct Backdrop {
    bitmap: isize,
    width: i32,
    height: i32,
    scaled_dc: HDC,
    scaled_bitmap: isize,
    scaled_old: isize,
    scaled_width: i32,
    scaled_height: i32,
}

impl Backdrop {
    fn free_scaled(&mut self) {
        // SAFETY: 只释放本结构保存的全尺寸缓存 GDI 对象；先选回旧对象再销毁 DC 和位图。
        unsafe {
            if !self.scaled_dc.is_null() {
                if self.scaled_old != 0 {
                    SelectObject(self.scaled_dc, self.scaled_old as _);
                }
                DeleteDC(self.scaled_dc);
                self.scaled_dc = std::ptr::null_mut();
                self.scaled_old = 0;
            }
            if self.scaled_bitmap != 0 {
                DeleteObject(self.scaled_bitmap as _);
                self.scaled_bitmap = 0;
            }
        }
        self.scaled_width = 0;
        self.scaled_height = 0;
    }

    fn ensure_scaled(&mut self, hdc: HDC, width: i32, height: i32) -> bool {
        if width <= 0 || height <= 0 {
            return false;
        }
        if !self.scaled_dc.is_null() && self.scaled_width == width && self.scaled_height == height {
            return true;
        }
        self.free_scaled();
        // SAFETY: 所有 GDI 对象成对创建并在失败或释放时清理；32 位自上而下 DIB 与窗口双缓冲格式一致。
        unsafe {
            let scaled_dc = CreateCompatibleDC(hdc);
            if scaled_dc.is_null() {
                return false;
            }
            let header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let scaled_bitmap = CreateDIBSection(
                scaled_dc,
                &header,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if scaled_bitmap.is_null() || bits.is_null() {
                if !scaled_bitmap.is_null() {
                    DeleteObject(scaled_bitmap as _);
                }
                DeleteDC(scaled_dc);
                return false;
            }
            let scaled_old = SelectObject(scaled_dc, scaled_bitmap as _);
            let src_dc = CreateCompatibleDC(hdc);
            if src_dc.is_null() {
                SelectObject(scaled_dc, scaled_old);
                DeleteDC(scaled_dc);
                DeleteObject(scaled_bitmap as _);
                return false;
            }
            let src_old = SelectObject(src_dc, self.bitmap as _);
            SetStretchBltMode(scaled_dc, HALFTONE);
            let ok = StretchBlt(
                scaled_dc,
                0,
                0,
                width,
                height,
                src_dc,
                0,
                0,
                self.width,
                self.height,
                SRCCOPY,
            );
            SelectObject(src_dc, src_old);
            DeleteDC(src_dc);
            if ok == 0 {
                SelectObject(scaled_dc, scaled_old);
                DeleteDC(scaled_dc);
                DeleteObject(scaled_bitmap as _);
                return false;
            }
            self.scaled_dc = scaled_dc;
            self.scaled_bitmap = scaled_bitmap as isize;
            self.scaled_old = scaled_old as isize;
            self.scaled_width = width;
            self.scaled_height = height;
            true
        }
    }

    pub fn draw(&mut self, hdc: HDC, width: i32, height: i32) {
        if self.ensure_scaled(hdc, width, height) {
            // SAFETY: scaled_dc 与 hdc 均有效，且尺寸为 width x height 的 1:1 拷贝。
            unsafe {
                BitBlt(hdc, 0, 0, width, height, self.scaled_dc, 0, 0, SRCCOPY);
            }
            return;
        }
        // SAFETY: 所有 GDI 对象在函数内成对释放。
        unsafe {
            let memory = CreateCompatibleDC(hdc);
            if memory.is_null() {
                return;
            }
            let previous = SelectObject(memory, self.bitmap as _);
            SetStretchBltMode(hdc, HALFTONE);
            StretchBlt(
                hdc,
                0,
                0,
                width,
                height,
                memory,
                0,
                0,
                self.width,
                self.height,
                SRCCOPY,
            );
            SelectObject(memory, previous);
            DeleteDC(memory);
        }
    }
}

impl Backdrop {
    /// 按不透明度把底板（缩放到目标矩形）叠到 hdc 上：入场动画里顶部露出的那条用它垫底。
    pub fn blend_into(&self, hdc: HDC, rect: RECT, alpha: u8) {
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return;
        }
        // SAFETY: 所有 GDI 对象在函数内成对释放。
        unsafe {
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: 0,
            };
            if !self.scaled_dc.is_null()
                && rect.left >= 0
                && rect.top >= 0
                && rect.right <= self.scaled_width
                && rect.bottom <= self.scaled_height
            {
                let w = rect.right - rect.left;
                let h = rect.bottom - rect.top;
                AlphaBlend(
                    hdc,
                    rect.left,
                    rect.top,
                    w,
                    h,
                    self.scaled_dc,
                    rect.left,
                    rect.top,
                    w,
                    h,
                    blend,
                );
                return;
            }
            let memory = CreateCompatibleDC(hdc);
            if memory.is_null() {
                return;
            }
            let previous = SelectObject(memory, self.bitmap as _);
            AlphaBlend(
                hdc,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                memory,
                0,
                0,
                self.width,
                self.height,
                blend,
            );
            SelectObject(memory, previous);
            DeleteDC(memory);
        }
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        self.free_scaled();
        // SAFETY: 位图由本结构独占，只释放一次。
        unsafe {
            if self.bitmap != 0 {
                DeleteObject(self.bitmap as _);
            }
        }
    }
}

/// 抓取屏幕上的 `area` 区域，缩小、模糊、压暗后返回可直接铺满的底板。
pub fn capture(area: RECT, tint_color: (u8, u8, u8), tint_strength: f32) -> Option<Backdrop> {
    let width = area.right - area.left;
    let height = area.bottom - area.top;
    if width <= 0 || height <= 0 {
        return None;
    }
    let small_width = (width / DOWNSCALE).max(64);
    let small_height = (height / DOWNSCALE).max(36);

    // SAFETY: 所有 GDI 对象在函数内成对释放。
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return None;
        }
        let memory = CreateCompatibleDC(screen);
        let header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: small_width,
                // 负高度 = 自上而下，方便直接按行处理像素。
                biHeight: -small_height,
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
            return None;
        }
        let previous = SelectObject(memory, bitmap as _);
        SetStretchBltMode(memory, HALFTONE);
        StretchBlt(
            memory,
            0,
            0,
            small_width,
            small_height,
            screen,
            area.left,
            area.top,
            width,
            height,
            SRCCOPY,
        );
        SelectObject(memory, previous);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);

        let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, (small_width * small_height * 4) as usize);
        blur(pixels, small_width, small_height, BLUR_RADIUS);
        tint(pixels, small_width * small_height, tint_color, tint_strength);

        Some(Backdrop {
            bitmap: bitmap as isize,
            width: small_width,
            height: small_height,
            scaled_dc: std::ptr::null_mut(),
            scaled_bitmap: 0,
            scaled_old: 0,
            scaled_width: 0,
            scaled_height: 0,
        })
    }
}

/// 三次盒式模糊，对小图足够。
fn blur(pixels: &mut [u8], width: i32, height: i32, radius: i32) {
    let stride = (width * 4) as usize;
    let mut buffer = pixels.to_vec();
    for _ in 0..3 {
        // 横向
        for y in 0..height as usize {
            for x in 0..width as usize {
                let mut sum = [0u32; 3];
                let mut count = 0u32;
                for dx in -radius..=radius {
                    let nx = x as i32 + dx;
                    if nx < 0 || nx >= width {
                        continue;
                    }
                    let index = y * stride + nx as usize * 4;
                    for channel in 0..3 {
                        sum[channel] += u32::from(pixels[index + channel]);
                    }
                    count += 1;
                }
                let out = y * stride + x * 4;
                for channel in 0..3 {
                    buffer[out + channel] = (sum[channel] / count.max(1)) as u8;
                }
            }
        }
        pixels.copy_from_slice(&buffer);
        // 纵向
        for y in 0..height as usize {
            for x in 0..width as usize {
                let mut sum = [0u32; 3];
                let mut count = 0u32;
                for dy in -radius..=radius {
                    let ny = y as i32 + dy;
                    if ny < 0 || ny >= height {
                        continue;
                    }
                    let index = ny as usize * stride + x * 4;
                    for channel in 0..3 {
                        sum[channel] += u32::from(pixels[index + channel]);
                    }
                    count += 1;
                }
                let out = y * stride + x * 4;
                for channel in 0..3 {
                    buffer[out + channel] = (sum[channel] / count.max(1)) as u8;
                }
            }
        }
        pixels.copy_from_slice(&buffer);
    }
}

/// 压暗并轻微偏冷，让图标和文字更清楚。
fn tint(pixels: &mut [u8], count: i32, (tr, tg, tb): (u8, u8, u8), strength: f32) {
    let strength = strength.clamp(0.0, 1.0);
    for index in 0..count as usize {
        let base = index * 4;
        let r = f32::from(pixels[base]);
        let g = f32::from(pixels[base + 1]);
        let b = f32::from(pixels[base + 2]);
        let mix = |value: f32, target: f32| -> u8 {
            let mixed = value * (1.0 - strength) + target * strength;
            mixed.clamp(0.0, 255.0) as u8
        };
        pixels[base] = mix(r, f32::from(tr));
        pixels[base + 1] = mix(g, f32::from(tg));
        pixels[base + 2] = mix(b, f32::from(tb));
        pixels[base + 3] = 255;
    }
}

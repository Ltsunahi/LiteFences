//! 预乘图标+标签图集缓存：把每个图标和带阴影文字一次性烘焙进单张 32 位预乘 DIB，
//! 折叠动画与滚动重绘时每项只需一次 `AlphaBlend`，避免每帧重复执行 `DrawIconEx` 与双遍 `DrawTextW`。

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, AlphaBlend, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, DrawTextW,
    DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_WORDBREAK, HDC, HFONT, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx, HICON};

use crate::util::wide;

const ATLAS_COLS: usize = 8;

struct DibSurface {
    dc: HDC,
    bitmap: isize,
    old: isize,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl DibSurface {
    fn create(hdc: HDC, width: i32, height: i32) -> Option<Self> {
        if hdc.is_null() || width <= 0 || height <= 0 {
            return None;
        }
        // SAFETY: 所有 GDI 对象成对创建并在失败或 Drop 时释放；32 位自上而下 DIB 与双缓冲一致。
        unsafe {
            let dc = CreateCompatibleDC(hdc);
            if dc.is_null() {
                return None;
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
            let bitmap = CreateDIBSection(
                dc,
                &header,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if bitmap.is_null() || bits.is_null() {
                if !bitmap.is_null() {
                    DeleteObject(bitmap as _);
                }
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bitmap as _);
            Some(Self {
                dc,
                bitmap: bitmap as isize,
                old: old as isize,
                bits: bits as *mut u8,
                width,
                height,
            })
        }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        let len = (self.width as usize) * (self.height as usize) * 4;
        // SAFETY: bits 由 CreateDIBSection 分配且在 Drop 前有效。
        unsafe { std::slice::from_raw_parts_mut(self.bits, len) }
    }

    fn bytes(&self) -> &[u8] {
        let len = (self.width as usize) * (self.height as usize) * 4;
        // SAFETY: bits 由 CreateDIBSection 分配且在 Drop 前有效。
        unsafe { std::slice::from_raw_parts(self.bits, len) }
    }
}

impl Drop for DibSurface {
    fn drop(&mut self) {
        // SAFETY: 先选回旧对象再销毁内存 DC 和 DIB 位图。
        unsafe {
            if !self.dc.is_null() {
                if self.old != 0 {
                    SelectObject(self.dc, self.old as _);
                }
                DeleteDC(self.dc);
            }
            if self.bitmap != 0 {
                DeleteObject(self.bitmap as _);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TileSpec {
    pub cell_w: i32,
    pub icon: i32,
    pub scale_bits: u32,
    pub text_color: u32,
    pub shadow_color: u32,
}

#[derive(Clone, PartialEq, Eq)]
struct SlotKey {
    icon: usize,
    label: String,
}

pub struct TileCache {
    spec: TileSpec,
    slot_w: i32,
    slot_h: i32,
    icon_x: i32,
    icon_y: i32,
    label_left: i32,
    label_rel_y: i32,
    label_w: i32,
    label_h: i32,
    capacity: usize,
    atlas: DibSurface,
    black: DibSurface,
    white: DibSurface,
    text: DibSurface,
    slots: Vec<Option<SlotKey>>,
}

impl TileCache {
    pub fn new(hdc: HDC, spec: TileSpec, count: usize) -> Option<Self> {
        let capacity = count.max(1).div_ceil(16) * 16;
        let scale = f32::from_bits(spec.scale_bits);
        let icon_x = ((spec.cell_w - spec.icon) / 2).max(0);
        let icon_y = (12.0 * scale) as i32;
        let pad_x = (4.0 * scale) as i32;
        let label_left = pad_x.max(0);
        let label_w = (spec.cell_w - 2 * pad_x).max(1);
        let label_rel_y = (spec.icon + (6.0 * scale) as i32).max(0);
        let label_h = ((34.0 * scale) as i32).max(24);
        let slot_w = spec.cell_w.max(icon_x + spec.icon).max(label_left + label_w + 1).max(1);
        let slot_h = (label_rel_y + label_h + 1).max(spec.icon).max(1);

        let rows = capacity.div_ceil(ATLAS_COLS) as i32;
        let atlas = DibSurface::create(hdc, (ATLAS_COLS as i32) * slot_w, rows * slot_h)?;
        let black = DibSurface::create(hdc, spec.icon.max(1), spec.icon.max(1))?;
        let white = DibSurface::create(hdc, spec.icon.max(1), spec.icon.max(1))?;
        let text = DibSurface::create(hdc, label_w + 1, label_h + 1)?;

        Some(Self {
            spec,
            slot_w,
            slot_h,
            icon_x,
            icon_y,
            label_left,
            label_rel_y,
            label_w,
            label_h,
            capacity,
            atlas,
            black,
            white,
            text,
            slots: vec![None; capacity],
        })
    }

    pub fn prepare(&mut self, hdc: HDC, spec: TileSpec, count: usize, in_fold: bool) -> bool {
        let need_resize = count > self.capacity;
        let spec_changed = self.spec != spec;
        let allow_scale_reuse = in_fold
            && !need_resize
            && self.spec.scale_bits == spec.scale_bits
            && self.spec.text_color == spec.text_color
            && self.spec.shadow_color == spec.shadow_color;
        if !need_resize && (!spec_changed || allow_scale_reuse) {
            return true;
        }
        let Some(rebuilt) = Self::new(hdc, spec, count) else {
            return false;
        };
        *self = rebuilt;
        true
    }

    pub fn draw_tile(
        &mut self,
        hdc: HDC,
        index: usize,
        icon: HICON,
        label: &str,
        font: HFONT,
        tile: RECT,
        current_cell_w: i32,
        current_icon: i32,
        scale: f32,
    ) -> bool {
        if index >= self.capacity {
            return false;
        }
        let icon_id = icon as usize;
        let hit = self.slots[index]
            .as_ref()
            .is_some_and(|slot| slot.icon == icon_id && slot.label == label);
        if !hit {
            self.bake_slot(index, icon, label, font);
        }

        let (sx, sy) = atlas_slot_origin(index, self.slot_w, self.slot_h);
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        if current_cell_w == self.spec.cell_w && current_icon == self.spec.icon {
            let dst_y = tile.top + self.icon_y;
            let visible_h = (tile.bottom - dst_y).clamp(0, self.slot_h);
            if visible_h <= 0 {
                return true;
            }
            // SAFETY: 两个 DC 均为有效 32 位表面；atlas 中已存预乘像素。
            unsafe {
                AlphaBlend(
                    hdc,
                    tile.left,
                    dst_y,
                    self.slot_w,
                    visible_h,
                    self.atlas.dc,
                    sx,
                    sy,
                    self.slot_w,
                    visible_h,
                    blend,
                );
            }
        } else {
            let current_icon_y = (12.0 * scale) as i32;
            let dst_y = tile.top + current_icon_y;
            if tile.bottom <= dst_y {
                return true;
            }
            let dst_w = current_cell_w.max(1);
            let dst_h = (current_icon + (6.0 * scale) as i32 + self.label_h + 1).max(1);
            // SAFETY: 两个 DC 均为有效 32 位表面；缩放期间由外层 clip_fold_tile 裁剪边界。
            unsafe {
                AlphaBlend(
                    hdc,
                    tile.left,
                    dst_y,
                    dst_w,
                    dst_h,
                    self.atlas.dc,
                    sx,
                    sy,
                    self.slot_w,
                    self.slot_h,
                    blend,
                );
            }
        }
        true
    }

    fn bake_slot(&mut self, index: usize, icon: HICON, label: &str, font: HFONT) {
        let (sx, sy) = atlas_slot_origin(index, self.slot_w, self.slot_h);
        let atlas_stride = (self.atlas.width as usize) * 4;
        {
            let dst = self.atlas.bytes_mut();
            let row_bytes = (self.slot_w as usize) * 4;
            for y in 0..self.slot_h as usize {
                let off = (sy as usize + y) * atlas_stride + (sx as usize) * 4;
                dst[off..off + row_bytes].fill(0);
            }
        }

        if !icon.is_null() {
            let icon_size = self.spec.icon.max(1);
            self.black.bytes_mut().fill(0x00);
            self.white.bytes_mut().fill(0xFF);
            // SAFETY: black/white 均为有效的内存 DC；DrawIconEx 大小匹配 DIB 尺寸。
            unsafe {
                DrawIconEx(
                    self.black.dc,
                    0,
                    0,
                    icon,
                    icon_size,
                    icon_size,
                    0,
                    std::ptr::null_mut(),
                    DI_NORMAL,
                );
                DrawIconEx(
                    self.white.dc,
                    0,
                    0,
                    icon,
                    icon_size,
                    icon_size,
                    0,
                    std::ptr::null_mut(),
                    DI_NORMAL,
                );
            }
            let black = self.black.bytes();
            let white = self.white.bytes();
            let dst = self.atlas.bytes_mut();
            let icon_stride = (icon_size as usize) * 4;
            for y in 0..icon_size as usize {
                let src_row = y * icon_stride;
                let dst_row = (sy as usize + y) * atlas_stride + (sx as usize + self.icon_x as usize) * 4;
                for x in 0..icon_size as usize {
                    let sp = src_row + x * 4;
                    let dp = dst_row + x * 4;
                    let b = [black[sp], black[sp + 1], black[sp + 2], black[sp + 3]];
                    let w = [white[sp], white[sp + 1], white[sp + 2], white[sp + 3]];
                    let px = compose_icon_pixel(b, w);
                    dst[dp..dp + 4].copy_from_slice(&px);
                }
            }
        }

        self.text.bytes_mut().fill(0x00);
        let text_w = WideBuf::from_str(label);
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: self.label_w,
            bottom: self.label_h,
        };
        // SAFETY: text.dc 有效；字体只在本次 DrawTextW 期间临时选入并立刻恢复。
        unsafe {
            let prev_font = if !font.is_null() {
                SelectObject(self.text.dc, font as _)
            } else {
                std::ptr::null_mut()
            };
            SetBkMode(self.text.dc, TRANSPARENT as i32);
            SetTextColor(self.text.dc, 0x00FFFFFF);
            DrawTextW(
                self.text.dc,
                text_w.as_ptr(),
                -1,
                &mut rect,
                DT_CENTER | DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            if !prev_font.is_null() {
                SelectObject(self.text.dc, prev_font);
            }
        }
        let mask_w = (self.label_w + 1) as usize;
        let mask_h = (self.label_h + 1) as usize;
        compose_label_from_mask(
            self.text.bytes(),
            mask_w,
            mask_h,
            self.spec.text_color,
            self.spec.shadow_color,
            self.atlas.bytes_mut(),
            (sx + self.label_left) as usize,
            (sy + self.label_rel_y) as usize,
            atlas_stride,
        );

        self.slots[index] = Some(SlotKey {
            icon: icon as usize,
            label: label.to_owned(),
        });
    }
}

struct WideBuf(Vec<u16>);

impl WideBuf {
    fn from_str(text: &str) -> Self {
        Self(wide(text))
    }

    fn as_ptr(&self) -> *const u16 {
        self.0.as_ptr()
    }
}

fn atlas_slot_origin(slot: usize, slot_w: i32, slot_h: i32) -> (i32, i32) {
    let col = (slot % ATLAS_COLS) as i32;
    let row = (slot / ATLAS_COLS) as i32;
    (col * slot_w, row * slot_h)
}

/// 通过黑底/白底差分恢复真实 alpha（避免 GDI `DrawIconEx` 对某些非 32bpp 图标丢失 alpha 或破坏预乘）。
fn compose_icon_pixel(black: [u8; 4], white: [u8; 4]) -> [u8; 4] {
    let da = (white[0] as i32 - black[0] as i32)
        .max(white[1] as i32 - black[1] as i32)
        .max(white[2] as i32 - black[2] as i32)
        .clamp(0, 255) as u8;
    let a = 255 - da;
    [black[0].min(a), black[1].min(a), black[2].min(a), a]
}

fn colorref_bgr(color: u32) -> (u32, u32, u32) {
    let r = color & 0xFF;
    let g = (color >> 8) & 0xFF;
    let b = (color >> 16) & 0xFF;
    (b, g, r)
}

#[allow(clippy::too_many_arguments)]
fn compose_label_from_mask(
    mask: &[u8],
    w: usize,
    h: usize,
    text_color: u32,
    shadow_color: u32,
    dst: &mut [u8],
    dst_x: usize,
    dst_y: usize,
    dst_stride: usize,
) {
    let (tb, tg, tr) = colorref_bgr(text_color);
    let (sb, sg, sr) = colorref_bgr(shadow_color);
    let text_w = w.saturating_sub(1);
    let text_h = h.saturating_sub(1);
    for y in 0..h {
        let row_off = (dst_y + y) * dst_stride + dst_x * 4;
        for x in 0..w {
            let ta = if x < text_w && y < text_h {
                mask[(y * w + x) * 4 + 1] as u32
            } else {
                0
            };
            let sa = if x > 0 && y > 0 && (x - 1) < text_w && (y - 1) < text_h {
                mask[((y - 1) * w + (x - 1)) * 4 + 1] as u32
            } else {
                0
            };
            if ta == 0 && sa == 0 {
                continue;
            }
            let s_eff = (sa * (255 - ta) + 127) / 255;
            let out_a = (ta + s_eff).min(255);
            let out_b = ((tb * ta + sb * s_eff + 127) / 255).min(out_a) as u8;
            let out_g = ((tg * ta + sg * s_eff + 127) / 255).min(out_a) as u8;
            let out_r = ((tr * ta + sr * s_eff + 127) / 255).min(out_a) as u8;
            let dp = row_off + x * 4;
            dst[dp] = out_b;
            dst[dp + 1] = out_g;
            dst[dp + 2] = out_r;
            dst[dp + 3] = out_a as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_difference_recovers_alpha_and_premultiplies() {
        assert_eq!(compose_icon_pixel([0, 0, 0, 0], [255, 255, 255, 0]), [0, 0, 0, 0]);
        assert_eq!(
            compose_icon_pixel([40, 120, 220, 0], [40, 120, 220, 0]),
            [40, 120, 220, 255]
        );
        // 50% 透明度纯白：黑底约 128，白底 255，差值 127 -> alpha = 128
        assert_eq!(
            compose_icon_pixel([128, 128, 128, 0], [255, 255, 255, 0]),
            [128, 128, 128, 128]
        );
    }

    #[test]
    fn label_mask_composes_shadow_and_respects_colorref_order() {
        let w = 3;
        let h = 3;
        let mut mask = vec![0u8; w * h * 4];
        // 在 (0, 0) 放全不透明字素，在 (1, 0) 放半透明边缘
        mask[1] = 255;
        mask[4 + 1] = 128;

        // COLORREF: 0x00BBGGRR => B=0x30, G=0x20, R=0x10
        let text_color = 0x0030_2010;
        let shadow_color = 0x0008_0402;
        let stride = w * 4;
        let mut dst = vec![0u8; h * stride];
        compose_label_from_mask(&mask, w, h, text_color, shadow_color, &mut dst, 0, 0, stride);

        // (0, 0): 纯正文
        assert_eq!(&dst[0..4], &[0x30, 0x20, 0x10, 255]);
        // (1, 1): 右下方阴影来自 (0, 0)，正文此处 ta == 0
        let shadow_idx = stride + 4;
        assert_eq!(&dst[shadow_idx..shadow_idx + 4], &[0x08, 0x04, 0x02, 255]);
        // (1, 0): ta == 128, sa == 0 => 半透明正文预乘，且各通道 <= alpha
        let edge_idx = 4;
        let a = dst[edge_idx + 3];
        assert_eq!(a, 128);
        assert!(dst[edge_idx] <= a && dst[edge_idx + 1] <= a && dst[edge_idx + 2] <= a);
        assert!(dst[edge_idx] > dst[edge_idx + 1] && dst[edge_idx + 1] > dst[edge_idx + 2]);
    }

    #[test]
    fn atlas_slot_origin_wraps_by_columns() {
        assert_eq!(atlas_slot_origin(0, 100, 90), (0, 0));
        assert_eq!(atlas_slot_origin(7, 100, 90), (700, 0));
        assert_eq!(atlas_slot_origin(8, 100, 90), (0, 90));
        assert_eq!(atlas_slot_origin(11, 100, 90), (300, 90));
    }
}

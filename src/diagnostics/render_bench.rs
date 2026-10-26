//! Controlled rendering microbenchmark, deliberately separate from the production UI.
//! All HWND/GDI/COM objects stay on this one thread and are dropped before the window.
use std::{
    cell::{Cell, OnceCell},
    error::Error,
    mem::size_of,
    path::PathBuf,
    ptr::{null, null_mut},
};
use windows::Win32::{
    Foundation::HWND as ComHwnd,
    Graphics::{Direct2D::Common::*, Direct2D::*, DirectWrite::*, Dxgi::Common::*},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::*, WindowsAndMessaging::*},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const WIDTH: i32 = 1120;
const HEIGHT: i32 = 680;
const ICON: i32 = 48;
const ICON_COUNT: usize = 200;
const TITLE: &str = "LiteFences P0-02 / controlled render baseline";
const CLASS: windows_sys::core::PCWSTR = windows_sys::core::w!("LiteFences.RenderBench");

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scenario {
    Empty,
    GdiSurface,
    GdiIcons,
    D2dSurface,
    D2dIcons,
}
impl Scenario {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "empty" => Ok(Self::Empty),
            "gdi-surface" => Ok(Self::GdiSurface),
            "gdi-icons" => Ok(Self::GdiIcons),
            "d2d-surface" => Ok(Self::D2dSurface),
            "d2d-icons" => Ok(Self::D2dIcons),
            _ => Err("invalid scenario".into()),
        }
    }
    fn icons(self) -> bool {
        matches!(self, Self::GdiIcons | Self::D2dIcons)
    }
}

struct Options {
    scenario: Scenario,
    name: String,
    ready: Option<PathBuf>,
    smoke: bool,
    lifetime: u32,
}
impl Options {
    fn parse() -> Result<Self> {
        let mut args = std::env::args_os().skip(1);
        let mut name = None;
        let mut ready = None;
        let mut smoke = false;
        let mut lifetime = 900;
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--scenario") if name.is_none() => {
                    name = Some(
                        args.next()
                            .ok_or("missing scenario")?
                            .into_string()
                            .map_err(|_| "invalid scenario encoding")?,
                    );
                }
                Some("--ready-file") if ready.is_none() => {
                    ready = Some(PathBuf::from(args.next().ok_or("missing ready file")?));
                }
                Some("--lifetime-seconds") => {
                    lifetime = args
                        .next()
                        .ok_or("missing lifetime")?
                        .to_str()
                        .ok_or("invalid lifetime")?
                        .parse()?;
                    if !(1..=3600).contains(&lifetime) {
                        return Err("lifetime must be 1..3600".into());
                    }
                }
                Some("--smoke-test") if !smoke => smoke = true,
                _ => return Err("unknown or repeated option".into()),
            }
        }
        let name = name.ok_or("--scenario is required")?;
        Ok(Self {
            scenario: Scenario::parse(&name)?,
            name,
            ready,
            smoke,
            lifetime,
        })
    }
}

struct OwnedGdi(HGDIOBJ);
impl Drop for OwnedGdi {
    fn drop(&mut self) {
        // SAFETY: only owned, unselected GDI objects are wrapped here.
        unsafe {
            DeleteObject(self.0);
        }
    }
}
struct MemoryDc(HDC);
impl Drop for MemoryDc {
    fn drop(&mut self) {
        // SAFETY: only CreateCompatibleDC results are owned here, no borrowed paint DCs.
        unsafe {
            DeleteDC(self.0);
        }
    }
}
struct NativeWindow(HWND);
impl Drop for NativeWindow {
    fn drop(&mut self) {
        // SAFETY: called on window owner thread while its state is still live.
        unsafe {
            if IsWindow(self.0) != 0 {
                DestroyWindow(self.0);
            }
        }
    }
}
struct Class(HINSTANCE);
impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: window has already been destroyed at this point.
        unsafe {
            UnregisterClassW(CLASS, self.0);
        }
    }
}

fn check(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// Opaque BGRA pixels, 200 distinct deterministic icons. Not Shell extraction or thumbnails.
fn fixture(index: usize) -> Vec<u8> {
    let mut pixels = vec![0; (ICON * ICON * 4) as usize];
    for y in 0..ICON {
        for x in 0..ICON {
            let offset = ((y * ICON + x) * 4) as usize;
            let stripe = ((x + y + index as i32) % 11 < 3) as u8;
            pixels[offset..offset + 4].copy_from_slice(&[
                40 + (index % 120) as u8,
                100 + stripe * 90,
                60 + (index * 7 % 170) as u8,
                255,
            ]);
        }
    }
    pixels
}
fn position(index: usize) -> (i32, i32) {
    let group = index / 40;
    let item = index % 40;
    (
        20 + (item % 20) as i32 * 54,
        65 + group as i32 * 120 + (item / 20) as i32 * 54,
    )
}

struct GdiRenderer {
    surface: OwnedGdi,
    icons: Vec<OwnedGdi>,
    font: OwnedGdi,
    dc: MemoryDc,
    icon_dc: MemoryDc,
}
impl GdiRenderer {
    fn new(icons: bool) -> Result<Self> {
        // SAFETY: creates UI-thread-owned DIBs and compatible DCs. No bitmap remains selected.
        unsafe {
            let dc = MemoryDc(CreateCompatibleDC(null_mut()));
            check(!dc.0.is_null())?;
            let icon_dc = MemoryDc(CreateCompatibleDC(null_mut()));
            check(!icon_dc.0.is_null())?;
            let surface = Self::bitmap(WIDTH, HEIGHT, None)?;
            let font = OwnedGdi(CreateFontW(
                -20,
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                DEFAULT_PITCH as u32,
                windows_sys::core::w!("Segoe UI"),
            ));
            check(!font.0.is_null())?;
            let mut bitmaps = Vec::new();
            if icons {
                for index in 0..ICON_COUNT {
                    bitmaps.push(Self::bitmap(ICON, ICON, Some(&fixture(index)))?);
                }
            }
            Ok(Self {
                surface,
                icons: bitmaps,
                font,
                dc,
                icon_dc,
            })
        }
    }
    unsafe fn bitmap(width: i32, height: i32, pixels: Option<&[u8]>) -> Result<OwnedGdi> {
        // SAFETY: BI_RGB 32-bit top-down DIB; destination allocation matches exact byte count.
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = null_mut();
            let bitmap = OwnedGdi(CreateDIBSection(
                null_mut(),
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                null_mut(),
                0,
            ));
            check(!bitmap.0.is_null() && !bits.is_null())?;
            if let Some(pixels) = pixels {
                std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast(), pixels.len());
            }
            Ok(bitmap)
        }
    }
    fn draw(&self, target: HDC) -> Result<()> {
        // SAFETY: all objects belong to this thread. Restore every selected object before returning.
        unsafe {
            let previous = SelectObject(self.dc.0, self.surface.0);
            let old_font = SelectObject(self.dc.0, self.font.0);
            let background = RECT {
                left: 0,
                top: 0,
                right: WIDTH,
                bottom: HEIGHT,
            };
            FillRect(self.dc.0, &background, GetSysColorBrush(COLOR_WINDOW));
            SetTextColor(self.dc.0, 0x403020);
            SetBkMode(self.dc.0, TRANSPARENT as i32);
            let mut title_rect = RECT {
                left: 20,
                top: 12,
                right: WIDTH - 20,
                bottom: 50,
            };
            let title = wide(TITLE);
            DrawTextW(
                self.dc.0,
                title.as_ptr(),
                title.len() as i32 - 1,
                &mut title_rect,
                DT_SINGLELINE,
            );
            for group in 0..5 {
                let rect = RECT {
                    left: 12,
                    top: 57 + group * 120,
                    right: WIDTH - 12,
                    bottom: 169 + group * 120,
                };
                FillRect(self.dc.0, &rect, GetSysColorBrush(COLOR_BTNFACE));
            }
            let mut ok = true;
            for (index, bitmap) in self.icons.iter().enumerate() {
                let old = SelectObject(self.icon_dc.0, bitmap.0);
                let (x, y) = position(index);
                ok &= BitBlt(self.dc.0, x, y, ICON, ICON, self.icon_dc.0, 0, 0, SRCCOPY) != 0;
                SelectObject(self.icon_dc.0, old);
            }
            ok &= BitBlt(target, 0, 0, WIDTH, HEIGHT, self.dc.0, 0, 0, SRCCOPY) != 0;
            SelectObject(self.dc.0, old_font);
            SelectObject(self.dc.0, previous);
            check(ok)
        }
    }
}

struct D2dRenderer {
    target: ID2D1HwndRenderTarget,
    icons: Vec<ID2D1Bitmap>,
    text: IDWriteTextFormat,
    brush: ID2D1SolidColorBrush,
    panel: ID2D1SolidColorBrush,
}
impl D2dRenderer {
    fn new(hwnd: HWND, icons: bool) -> Result<Self> {
        // SAFETY: factory/target creation and all subsequent COM calls stay on this UI thread.
        // windows interface wrappers release their references on drop; upload buffers live through CreateBitmap.
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let target = factory.CreateHwndRenderTarget(
                &D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_IGNORE,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    ..Default::default()
                },
                &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd: ComHwnd(hwnd),
                    pixelSize: D2D_SIZE_U {
                        width: WIDTH as u32,
                        height: HEIGHT as u32,
                    },
                    presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                },
            )?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let text = dwrite.CreateTextFormat(
                windows::core::w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                20.0,
                windows::core::w!("en-US"),
            )?;
            let brush = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.125,
                    g: 0.188,
                    b: 0.25,
                    a: 1.0,
                },
                None,
            )?;
            let panel = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.94,
                    g: 0.94,
                    b: 0.94,
                    a: 1.0,
                },
                None,
            )?;
            let mut bitmaps = Vec::new();
            if icons {
                for index in 0..ICON_COUNT {
                    let pixels = fixture(index);
                    bitmaps.push(target.CreateBitmap(
                        D2D_SIZE_U {
                            width: ICON as u32,
                            height: ICON as u32,
                        },
                        Some(pixels.as_ptr().cast()),
                        ICON as u32 * 4,
                        &D2D1_BITMAP_PROPERTIES {
                            pixelFormat: D2D1_PIXEL_FORMAT {
                                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                            },
                            dpiX: 96.0,
                            dpiY: 96.0,
                        },
                    )?);
                }
            }
            Ok(Self {
                target,
                icons: bitmaps,
                text,
                brush,
                panel,
            })
        }
    }
    fn draw(&self) -> Result<()> {
        // SAFETY: BeginDraw/EndDraw balanced, resources all from this target, fixed valid rectangles.
        unsafe {
            self.target.BeginDraw();
            self.target.Clear(Some(&D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            }));
            for group in 0..5 {
                self.target.FillRectangle(
                    &D2D_RECT_F {
                        left: 12.0,
                        top: (57 + group * 120) as f32,
                        right: (WIDTH - 12) as f32,
                        bottom: (169 + group * 120) as f32,
                    },
                    &self.panel,
                );
            }
            let title: Vec<u16> = TITLE.encode_utf16().collect();
            self.target.DrawText(
                &title,
                &self.text,
                &D2D_RECT_F {
                    left: 20.0,
                    top: 12.0,
                    right: (WIDTH - 20) as f32,
                    bottom: 50.0,
                },
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            for (index, icon) in self.icons.iter().enumerate() {
                let (x, y) = position(index);
                self.target.DrawBitmap(
                    icon,
                    Some(&D2D_RECT_F {
                        left: x as f32,
                        top: y as f32,
                        right: (x + ICON) as f32,
                        bottom: (y + ICON) as f32,
                    }),
                    1.0,
                    D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                );
            }
            self.target.EndDraw(None, None)?;
            Ok(())
        }
    }
}

enum Renderer {
    Empty,
    Gdi(GdiRenderer),
    D2d(D2dRenderer),
}
impl Renderer {
    fn new(hwnd: HWND, scenario: Scenario) -> Result<Self> {
        Ok(match scenario {
            Scenario::Empty => Self::Empty,
            Scenario::GdiSurface | Scenario::GdiIcons => {
                Self::Gdi(GdiRenderer::new(scenario.icons())?)
            }
            Scenario::D2dSurface | Scenario::D2dIcons => {
                Self::D2d(D2dRenderer::new(hwnd, scenario.icons())?)
            }
        })
    }
    fn draw(&self, dc: HDC) -> Result<()> {
        match self {
            Self::Empty => {
                // SAFETY: borrowed paint DC, stock brush and valid client-space rectangle.
                unsafe {
                    check(
                        FillRect(
                            dc,
                            &RECT {
                                left: 0,
                                top: 0,
                                right: WIDTH,
                                bottom: HEIGHT,
                            },
                            GetSysColorBrush(COLOR_WINDOW),
                        ) != 0,
                    )
                }
            }
            Self::Gdi(renderer) => renderer.draw(dc),
            Self::D2d(renderer) => renderer.draw(),
        }
    }
}

#[derive(Default)]
struct State {
    renderer: OnceCell<Renderer>,
    paints: Cell<u32>,
    failed: Cell<bool>,
}
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: state has a stable Box address and outlives HWND. Only this thread accesses it.
    unsafe {
        if msg == WM_NCCREATE {
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                (*(l as *const CREATESTRUCTW)).lpCreateParams as isize,
            );
        }
        let state = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State).as_ref();
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                if let Some(state) = state
                    && let Some(renderer) = state.renderer.get()
                {
                    if dc.is_null() || renderer.draw(dc).is_err() {
                        state.failed.set(true);
                    }
                    state.paints.set(state.paints.get().saturating_add(1));
                }
                EndPaint(hwnd, &ps);
                0
            }
            WM_ERASEBKGND => 1,
            WM_TIMER | WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, msg, w, l)
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

pub fn run() -> Result<()> {
    let options = Options::parse()?;
    // SAFETY: every Win32 call runs on UI thread. Guards maintain destruction order.
    unsafe {
        check(SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) != 0)?;
        let instance = GetModuleHandleW(null());
        check(!instance.is_null())?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: CLASS,
            ..Default::default()
        };
        check(RegisterClassExW(&class) != 0)?;
        let _class = Class(instance);
        let state = Box::<State>::default();
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: WIDTH,
            bottom: HEIGHT,
        };
        check(
            AdjustWindowRectExForDpi(&mut rect, style, 0, WS_EX_NOACTIVATE, GetDpiForSystem()) != 0,
        )?;
        let title = wide(&format!("LiteFences P0-02 [{}]", options.name));
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE,
            CLASS,
            title.as_ptr(),
            style,
            20,
            20,
            rect.right - rect.left,
            rect.bottom - rect.top,
            null_mut(),
            null_mut(),
            instance,
            (&*state as *const State).cast(),
        );
        check(!hwnd.is_null())?;
        let window = NativeWindow(hwnd);
        state
            .renderer
            .set(Renderer::new(hwnd, options.scenario)?)
            .map_err(|_| "renderer already initialized")?;
        // Benchmark surface size uses physical pixels and Direct2D dpi=96 for comparable uploads.
        let mut client = RECT::default();
        check(GetClientRect(hwnd, &mut client) != 0)?;
        if client.right != WIDTH || client.bottom != HEIGHT {
            return Err("unexpected client dimensions".into());
        }
        // Hidden smoke tests validate calls; only non-smoke, visible runs are measured.
        if !options.smoke {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        InvalidateRect(hwnd, null(), 0);
        SendMessageW(hwnd, WM_PAINT, 0, 0);
        if state.failed.get() || state.paints.get() == 0 {
            return Err("first draw failed".into());
        }
        if let Some(path) = &options.ready {
            let icons = if options.scenario.icons() {
                ICON_COUNT
            } else {
                0
            };
            let contents = format!(
                "{{\"schema_version\":1,\"scenario\":\"{}\",\"pid\":{},\"client_width_px\":{},\"client_height_px\":{},\"window_dpi\":{},\"render_dpi\":96,\"icon_count\":{},\"icon_size_px\":48,\"icon_pixel_bytes\":{},\"smoke\":{},\"fixture\":\"opaque-bgra-deterministic-v1\"}}",
                options.name,
                std::process::id(),
                WIDTH,
                HEIGHT,
                GetDpiForWindow(hwnd),
                icons,
                icons * (ICON * ICON * 4) as usize,
                options.smoke
            );
            // Unique caller-provided output; do not overwrite an existing result.
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
        }
        if options.smoke {
            check(PostMessageW(hwnd, WM_CLOSE, 0, 0) != 0)?;
        } else {
            check(SetTimer(hwnd, 1, options.lifetime * 1000, None) != 0)?;
        }
        let mut message = MSG::default();
        loop {
            let result = GetMessageW(&mut message, null_mut(), 0, 0);
            if result == -1 {
                return Err(std::io::Error::last_os_error().into());
            }
            if result == 0 {
                break;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        let failed = state.failed.get();
        // Explicitly release drawing resources while HWND is no longer in use.
        drop(window);
        drop(state);
        if failed {
            return Err("render failed during message loop".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures_are_distinct_bounded_opaque_and_on_screen() {
        let images: Vec<_> = (0..ICON_COUNT).map(fixture).collect();
        for (i, image) in images.iter().enumerate() {
            assert_eq!(image.len(), (ICON * ICON * 4) as usize);
            assert!(image.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
            let (x, y) = position(i);
            assert!(x >= 0 && y >= 0 && x + ICON <= WIDTH && y + ICON <= HEIGHT);
            assert!(!images[..i].contains(image));
        }
    }
}

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use windows::Win32::{
    Foundation::{HWND, SIZE},
    Graphics::{
        DirectComposition::{IDCompositionEffectGroup, IDCompositionVisual2},
        DirectWrite::{DWRITE_TEXT_ALIGNMENT_CENTER, IDWriteFactory},
        Gdi::DeleteObject,
        Imaging::{
            GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, WICBitmapUsePremultipliedAlpha,
            WICConvertBitmapSource,
        },
    },
    UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
        SIIGBF_INCACHEONLY,
    },
};
use windows::core::{HSTRING, Result};

use super::canvas::{Canvas, Palette, Text, paint};
use super::{Bar, DURATION, Renderer, Tween};
use crate::ui::{icon::Symbol, launchpad::LaunchpadLayout, panel::Area};

const WHITE: (u8, u8, u8) = (255, 255, 255);
/// How far the launchpad darkens while an app from the Dock is held over it.
const DEEPEN: f32 = 0.26;

pub(super) struct LaunchpadView {
    hwnd: HWND,
    bar: Bar,
    highlight: IDCompositionVisual2,
    fade: IDCompositionEffectGroup,
    hovered: Option<usize>,
    /// A darker layer with a frame around the grid, shown while an app is held over the
    /// launchpad, above everything else in the window.
    veil: IDCompositionVisual2,
    veil_fade: IDCompositionEffectGroup,
    deep: Tween,
    /// Window size, DPI and content height the veil was painted for.
    veil_key: Option<(i32, i32, u32, u32)>,
    icons: HashMap<PathBuf, Option<IWICBitmapSource>>,
    icon_pixels: i32,
}

impl Renderer {
    pub fn draw_launchpad(
        &mut self,
        hwnd: HWND,
        layout: &LaunchpadLayout,
        paths: &[PathBuf],
        query: &str,
    ) -> Result<()> {
        if self.launchpad.as_ref().map(|v| v.hwnd) != Some(hwnd) {
            let bar = Bar::new(&self.device, hwnd)?;
            unsafe {
                let highlight = self.device.CreateVisual()?;
                let fade = self.device.CreateEffectGroup()?;
                highlight.SetEffect(&fade)?;
                bar.root.AddVisual(&highlight, true, None)?;
                let veil = self.device.CreateVisual()?;
                let veil_fade = self.device.CreateEffectGroup()?;
                veil_fade.SetOpacity2(0.)?;
                veil.SetEffect(&veil_fade)?;
                // In front of all siblings, the hover highlight included.
                bar.root.AddVisual(&veil, false, None)?;
                self.launchpad = Some(LaunchpadView {
                    hwnd,
                    bar,
                    highlight,
                    fade,
                    hovered: None,
                    veil,
                    veil_fade,
                    deep: Tween::new(0.),
                    veil_key: None,
                    icons: HashMap::new(),
                    icon_pixels: 0,
                });
            }
        }
        let scale = layout.dpi as f32 / 96.;
        let p = Palette::new(true);
        if let Some(tile) = layout.tiles.first() {
            let pixels = ((tile.icon.right - tile.icon.left) * scale).round() as i32;
            let view = self.launchpad.as_mut().unwrap();
            if view.icon_pixels != pixels {
                view.icons.clear();
                view.icon_pixels = pixels;
            }
            let area = tile.hit;
            let w = area.right - area.left;
            let h = area.bottom - area.top;
            let glow = paint(
                &self.device,
                p,
                (w * scale).ceil() as i32,
                (h * scale).ceil() as i32,
                layout.dpi,
                |c| c.fill(0., 0., w, h, 12., WHITE, 0.095),
            )?;
            unsafe { view.highlight.SetContent(&glow)? };
            for tile in &layout.tiles {
                let path = &paths[tile.index];
                if !self.launchpad.as_ref().unwrap().icons.contains_key(path) {
                    let bitmap = self.launchpad_icon(path, pixels)?;
                    self.launchpad
                        .as_mut()
                        .unwrap()
                        .icons
                        .insert(path.clone(), bitmap);
                }
            }
        }
        let view = self.launchpad.as_mut().unwrap();
        let key = (
            layout.width,
            layout.height,
            layout.dpi,
            layout.content.to_bits(),
        );
        if view.veil_key != Some(key) {
            let (w, h) = (layout.width as f32 / scale, layout.height as f32 / scale);
            let bottom = layout.content;
            let veil = paint(
                &self.device,
                p,
                layout.width,
                layout.height,
                layout.dpi,
                |c| {
                    c.fill(0., 0., w, h, 0., (0, 0, 0), DEEPEN)?;
                    // A soft frame around the part above the Dock, where the app lands.
                    c.stroke(9., 9., w - 9., bottom - 9., 18., WHITE, 0.12)?;
                    c.stroke(10., 10., w - 10., bottom - 10., 17., WHITE, 0.34)
                },
            )?;
            unsafe { view.veil.SetContent(&veil)? };
            view.veil_key = Some(key);
        }
        let surface = paint(
            &self.device,
            p,
            layout.width,
            layout.height,
            layout.dpi,
            |c| {
                let (w, h) = (layout.width as f32 / scale, layout.height as f32 / scale);
                c.gradient(0., 0., w, h, 0., ((29, 30, 33), 0.82), ((22, 23, 25), 0.84))?;
                let search = layout.search;
                c.fill(
                    search.left,
                    search.top,
                    search.right,
                    search.bottom,
                    7.,
                    WHITE,
                    0.09,
                )?;
                c.stroke(
                    search.left,
                    search.top,
                    search.right,
                    search.bottom,
                    7.,
                    WHITE,
                    0.34,
                )?;
                let searching = !query.is_empty();
                let glass_x = if searching {
                    search.left + 16.
                } else {
                    w / 2. - 22.
                };
                c.symbol(Symbol::Search, glass_x, search.middle(), 15., WHITE, 0.75)?;
                if searching {
                    c.text(
                        &self.write,
                        query,
                        c.rect(glass_x + 13., search.top, search.right - 10., search.bottom),
                        13.,
                        false,
                        WHITE,
                    )?;
                } else {
                    c.draw_text(
                        &self.write,
                        "搜索",
                        c.rect(glass_x + 12., search.top, search.right, search.bottom),
                        Text::default(),
                        WHITE,
                        0.72,
                    )?;
                }
                for tile in &layout.tiles {
                    let icon = tile.icon;
                    if let Some(bitmap) = &view.icons[&paths[tile.index]] {
                        c.bitmap(bitmap, c.rect(icon.left, icon.top, icon.right, icon.bottom))?;
                    } else {
                        c.fill(
                            icon.left,
                            icon.top,
                            icon.right,
                            icon.bottom,
                            14.,
                            WHITE,
                            0.14,
                        )?;
                        c.symbol_in(
                            Symbol::Window,
                            icon,
                            (icon.bottom - icon.top) * 0.52,
                            WHITE,
                            0.90,
                        )?;
                    }
                    centered(c, &self.write, &tile.name, tile.label, 13., true, 1.)?;
                }
                if layout.tiles.is_empty() {
                    let mid = layout.content / 2.;
                    if paths.is_empty() {
                        c.symbol(Symbol::Apps, w / 2., mid - 42., 44., WHITE, 0.78)?;
                        centered(
                            c,
                            &self.write,
                            "启动台",
                            Area::new(8., mid - 6., w - 8., mid + 26.),
                            23.,
                            true,
                            0.97,
                        )?;
                        centered(
                            c,
                            &self.write,
                            "将快捷方式或程序文件拖到这里",
                            Area::new(8., mid + 30., w - 8., mid + 56.),
                            14.,
                            false,
                            0.76,
                        )?;
                    } else {
                        centered(
                            c,
                            &self.write,
                            "没有匹配的程序",
                            Area::new(8., mid - 20., w - 8., mid + 20.),
                            17.,
                            false,
                            0.85,
                        )?;
                    }
                }
                for (page, area) in &layout.dots {
                    let x = (area.left + area.right) / 2.;
                    let y = area.middle();
                    let alpha = if layout.page == *page { 1. } else { 0.35 };
                    c.fill(x - 3., y - 3., x + 3., y + 3., 3., WHITE, alpha)?;
                }
                Ok(())
            },
        )?;
        view.hovered = None;
        unsafe {
            view.fade.SetOpacity2(0.)?;
            view.bar.root.SetContent(&surface)?;
            self.device.Commit()
        }
    }

    pub fn hover_launchpad(&mut self, layout: &LaunchpadLayout, item: Option<usize>) -> Result<()> {
        let Some(view) = &mut self.launchpad else {
            return Ok(());
        };
        if view.hovered == item {
            return Ok(());
        }
        let hit = layout.tiles.iter().find(|tile| Some(tile.index) == item);
        unsafe {
            if let Some(tile) = hit {
                let scale = layout.dpi as f32 / 96.;
                view.highlight
                    .SetOffsetX2((tile.hit.left * scale).round())?;
                view.highlight.SetOffsetY2((tile.hit.top * scale).round())?;
                view.fade.SetOpacity2(1.)?;
            } else {
                view.fade.SetOpacity2(0.)?;
            }
            self.device.Commit()?;
        }
        view.hovered = hit.map(|tile| tile.index);
        Ok(())
    }

    /// Darkens the launchpad while `on`, as the target for an app held over it.
    pub fn deepen_launchpad(&mut self, on: bool) -> Result<()> {
        let Some(view) = &mut self.launchpad else {
            return Ok(());
        };
        let goal = if on { 1. } else { 0. };
        if let Some(curve) = view.deep.retarget(&self.device, goal, DURATION)? {
            unsafe {
                view.veil_fade.SetOpacity(&curve)?;
                self.device.Commit()?;
            }
        }
        Ok(())
    }

    pub fn close_launchpad(&mut self) {
        if self.launchpad.take().is_some() {
            let _ = unsafe { self.device.Commit() };
        }
    }

    fn launchpad_icon(&self, path: &Path, pixels: i32) -> Result<Option<IWICBitmapSource>> {
        unsafe {
            let item: Result<IShellItemImageFactory> =
                SHCreateItemFromParsingName(&HSTRING::from(path), None);
            let cached = item.and_then(|item| {
                item.GetImage(
                    SIZE {
                        cx: pixels,
                        cy: pixels,
                    },
                    SIIGBF_INCACHEONLY | SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
                )
            });
            let Ok(handle) = cached else {
                return self.shell_icon(path);
            };
            // GetImage transfers the GDI bitmap. WIC copies it before the handle is released.
            let copied = self.imaging.CreateBitmapFromHBITMAP(
                handle,
                Default::default(),
                WICBitmapUsePremultipliedAlpha,
            );
            let _ = DeleteObject(handle.into());
            Ok(Some(WICConvertBitmapSource(
                &GUID_WICPixelFormat32bppPBGRA,
                &copied?,
            )?))
        }
    }
}

fn centered(
    c: &Canvas,
    write: &IDWriteFactory,
    text: &str,
    area: Area,
    size: f32,
    bold: bool,
    alpha: f32,
) -> Result<()> {
    let style = Text {
        size,
        bold,
        align: DWRITE_TEXT_ALIGNMENT_CENTER,
    };
    c.draw_text(
        write,
        text,
        c.rect(area.left, area.top, area.right, area.bottom),
        style,
        WHITE,
        alpha,
    )
}

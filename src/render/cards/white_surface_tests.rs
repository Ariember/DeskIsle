use super::*;
use crate::platform::{Desktop, create_bars};
use crate::render::canvas::paint_pixels;
use crate::settings::Settings;
use crate::ui::cards::{CARD_WIDTH_DIP, card_height_dip};

#[test]
#[ignore = "requires a Windows desktop with DirectComposition"]
fn magpie_stays_white_and_legible_over_dark_wallpaper() -> Result<()> {
    let _desktop = Desktop::init()?;
    let (top, bottom) = create_bars()?;
    let mut renderer = Renderer::new(top.0, bottom.0, true, &Settings::default())?;
    let mut card: Card = serde_json::from_value(serde_json::json!({
        "title": "Magpie 数据面板", "value": "128 次调用", "detail": "128 次请求 · 6 会话",
        "badge": "今日 · 10:15",
        "blocks": [
            {"type": "metrics", "title": "Token 统计", "items": [
                {"label": "总 Token", "value": "2.4M", "sub": "输入 1.9M · 输出 500K", "tone": "neutral"},
                {"label": "缓存命中", "value": "76%", "sub": "读取 1.4M", "tone": "neutral"}
            ]},
            {"type": "meters", "title": "模型占比", "items": [
                {"label": "opus-5-5", "value": "74%", "percent": 74, "tone": "neutral"},
                {"label": "其他", "value": "26%", "percent": 26, "tone": "neutral"}
            ]},
            {"type": "meters", "title": "订阅额度", "items": [
                {"label": "Codex", "value": "剩 42%", "detail": "2小时后重置", "percent": 42, "tone": "neutral"}
            ]}
        ]
    })).unwrap();
    card.white_surface = true;
    let scale = 2;
    let tile = Rect {
        x: 0,
        y: 0,
        width: CARD_WIDTH_DIP * scale,
        height: card_height_dip(&card) * scale,
    };
    let backdrop = Backdrop {
        width: tile.width as u32,
        height: tile.height as u32,
        pixels: [35, 16, 56, 255].repeat((tile.width * tile.height) as usize),
    };
    let mut opaque = Vec::new();
    for transparent in [false, true] {
        renderer.set_transparency(transparent);
        let pixels = paint_pixels(
            &renderer.d2d,
            renderer.palette,
            tile.width,
            tile.height,
            192,
            |c| {
                renderer.paint_card_tile(
                    c,
                    tile,
                    &card,
                    transparent.then_some(&backdrop),
                    100,
                    None,
                )
            },
        )?;
        let stride = tile.width as usize * 4;
        // At the right inset, no labels or progress bars cross the white ground.
        for y in 24 * scale..tile.height - 24 * scale {
            let i = y as usize * stride + (tile.width - 7 * scale) as usize * 4;
            let ground = &pixels[i..i + 4];
            assert_eq!(
                ground, [255; 4],
                "background at y={y}, transparent={transparent}"
            );
        }
        // The title and main reading must each contain dark glyph interiors on white.
        for (left, top, right, bottom) in [(49, 10, 168, 42), (16, 44, 166, 78)] {
            let ink = (top * scale..bottom * scale)
                .flat_map(|y| {
                    let row = &pixels[y as usize * stride..][..stride];
                    row[(left * scale * 4) as usize..(right * scale * 4) as usize].chunks_exact(4)
                })
                .filter(|p| p[3] == 255 && p[..3].iter().all(|&v| v < 95))
                .count();
            assert!(
                ink > 100,
                "text at y={top} has only {ink} high-contrast pixels"
            );
        }
        if transparent {
            assert!(pixels == opaque, "wallpaper changed the white card");
        } else {
            opaque = pixels;
        }
    }
    Ok(())
}

//! Shared by the `site_*` examples (the website's screenshots): open one
//! window natively, or with `--screenshot out.png` render it off-screen and
//! save it. `BUI_THEME=light` and `BUI_UI_SCALE=2` apply to both.

#![allow(dead_code)]

use blenderui::prelude::*;

pub fn screenshot_arg() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1)).cloned()
}

pub fn options(w: f32, h: f32, title: &str) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(w), px(h)),
        })),
        titlebar: Some(TitlebarOptions { title: Some(title.to_string().into()), ..Default::default() }),
        ..Default::default()
    }
}

/// Writes 0x00RRGGBB pixels as an RGB PNG.
pub fn save(path: &str, w: usize, h: usize, px: &[u32]) {
    let rgb: Vec<u8> = px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Best);
    enc.write_header().expect("png").write_image_data(&rgb).expect("png");
    println!("wrote {path} ({w}x{h})");
}

/// Runs one view: a native window, or (with `--screenshot`) one off-screen frame.
pub fn run<V: Render + 'static>(w: f32, h: f32, title: &str, make: impl FnOnce() -> V + 'static) {
    let shot = screenshot_arg();
    let app = if shot.is_some() { Application::headless() } else { Application::new() };
    let title = title.to_string();
    app.run(move |cx| {
        let win = cx.open_window(options(w, h, &title), |_, cx| cx.new(|_| make())).unwrap();
        if let Some(path) = shot {
            cx.draw(win.id);
            let (pw, ph, px) = cx.draw(win.id);
            save(&path, pw, ph, &px);
        }
    });
}

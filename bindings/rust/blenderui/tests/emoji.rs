//! Emoji through the per-glyph fallback face: characters the UI face lacks
//! come from the font registered as "emoji", in colour when it has COLR
//! layers (Segoe UI Emoji here). Writes what it drew to
//! `/tmp/dither-emoji/fork.ppm` (and `.png` when ImageMagick is installed).

use blenderui::prelude::*;

const TEXT: &str = "Hello \u{1F914} \u{1F600} \u{2764}\u{FE0F} \u{1F44D} world :)";

fn font() -> Option<Vec<u8>> {
    let home = std::env::var("HOME").ok()?;
    std::fs::read(format!("{home}/.local/share/fonts/windows/seguiemj.ttf")).ok()
}

fn open<V: Render>(cx: &mut App, w: f32, h: f32, v: V) -> WindowHandle<V> {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(w), px(h)),
        })),
        ..Default::default()
    };
    cx.open_window(opts, |_, cx| cx.new(|_| v)).unwrap()
}

struct Page;

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        body().m_0().p(px(10.0)).child(
            div().style("display: flex; flex-direction: column; gap: 10px")
                .child(div().id("small").style("font-size: 14px").child(TEXT))
                .child(div().id("large").style("font-size: 32px").child(TEXT))
                .child(textarea().id("area").attr("rows", "2").style("font-size: 20px").value(TEXT)),
        )
    }
}

/// Whether the pixel is coloured (its channels differ by more than a few units).
fn colored(p: u32) -> bool {
    let (r, g, b) = ((p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff);
    let lo = r.min(g).min(b);
    let hi = r.max(g).max(b);
    hi - lo > 24
}

/// Coloured pixels inside `b` (logical) of `img` drawn at `s` device pixels per pixel.
fn colored_in(img: &(usize, usize, Vec<u32>), b: &Bounds<f32>, s: f32) -> usize {
    let (x0, y0) = ((b.origin.x * s) as usize, (b.origin.y * s) as usize);
    let (x1, y1) = (((b.origin.x + b.size.width) * s) as usize, ((b.origin.y + b.size.height) * s) as usize);
    let mut n = 0;
    for y in y0..y1.min(img.1) {
        for x in x0..x1.min(img.0) {
            n += usize::from(colored(img.2[y * img.0 + x]));
        }
    }
    n
}

fn write_ppm(path: &str, img: &(usize, usize, Vec<u32>)) {
    let mut out = format!("P6\n{} {}\n255\n", img.0, img.1).into_bytes();
    for p in &img.2 {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    std::fs::write(path, out).unwrap();
}

fn measure(s: &str, size: f32) -> f32 {
    let mut w = 0.0f32;
    // SAFETY: the family is a valid C string; the text pointer is valid for the call.
    let ok = unsafe {
        blenderui_sys::BUI_text_measure(c"".as_ptr(), 0, size, s.as_ptr().cast(), s.len() as i32, &mut w)
    };
    assert_eq!(ok, 1);
    w
}

#[test]
fn emoji_fallback_in_div_and_textarea() {
    let Some(data) = font() else {
        eprintln!("emoji: no ~/.local/share/fonts/windows/seguiemj.ttf, skipping");
        return;
    };
    assert!(blenderui::add_font("emoji", false, false, &data));

    // the fallback's advance, not the UI face's .notdef box
    let em = measure("\u{1F914}", 32.0);
    let notdef = measure("\u{E000}", 32.0); // private use: no face has it
    assert!(em > 32.0 * 0.9 && em < 32.0 * 1.6, "em {em} notdef {notdef}");
    assert!((em - notdef).abs() > 4.0, "em {em} notdef {notdef}");
    // format characters alone draw nothing and take no space
    assert_eq!(measure("\u{FE0F}\u{200D}", 32.0), 0.0);
    // U+2764 is Inter's (text presentation); with U+FE0F it is the emoji face's
    let heart = measure("\u{2764}", 32.0);
    let heart_emoji = measure("\u{2764}\u{FE0F}", 32.0);
    assert!(heart > 0.0 && heart_emoji > heart, "heart {heart} emoji {heart_emoji}");

    Application::headless().run(|cx| {
        let win = open(cx, 520.0, 220.0, Page);
        let id = win.id;
        let img = cx.draw(id);
        let s = img.0 as f32 / 520.0;
        std::fs::create_dir_all("/tmp/dither-emoji").unwrap();
        write_ppm("/tmp/dither-emoji/fork.ppm", &img);
        let _ = std::process::Command::new("magick")
            .args(["/tmp/dither-emoji/fork.ppm", "/tmp/dither-emoji/fork-div-and-textarea.png"])
            .status();
        for (name, min) in [("small", 20), ("large", 150), ("area", 60)] {
            let b = cx.element_bounds(id, name).unwrap();
            let n = colored_in(&img, &b, s);
            assert!(n >= min, "{name}: {n} coloured pixels in {b:?}");
        }
        // the emoji sit on the text line: nothing coloured above the small line
        let b = cx.element_bounds(id, "small").unwrap();
        let above = Bounds { origin: point(0.0, 0.0), size: size(520.0, b.origin.y) };
        assert_eq!(colored_in(&img, &above, s), 0);
    });
}

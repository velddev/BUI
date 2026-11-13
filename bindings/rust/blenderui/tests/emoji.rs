//! Emoji through the per-glyph fallback face: characters the UI face lacks
//! come from the font registered as "emoji", in colour when it has COLR
//! layers (Segoe UI Emoji here). Writes what it drew to
//! `/tmp/dither-emoji/fork.ppm` (and `.png` when ImageMagick is installed).

use blenderui::prelude::*;

const TEXT: &str = "Hello \u{1F914} \u{1F600} \u{2764}\u{FE0F} \u{1F44D} world :)";

/// Sequences the emoji face joins: bald man, woman with red hair, thumbs up with a tone,
/// woman technologist, family, kiss with two tones, keycap, pirate flag.
const JOINED: &str = "\u{1F468}\u{200D}\u{1F9B2}|\u{1F469}\u{200D}\u{1F9B0}|\u{1F44D}\u{1F3FB}|\u{1F469}\u{200D}\u{1F4BB}|\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}|\u{1F469}\u{1F3FD}\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F48B}\u{200D}\u{1F468}\u{1F3FB}|1\u{FE0F}\u{20E3}|\u{1F3F4}\u{200D}\u{2620}\u{FE0F}|";

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
                .child(div().id("joined").style("font-size: 32px").child(JOINED))
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
    // sequences the emoji face joins (its GSUB ligatures) are one emoji wide, not two:
    // hair components, skin tones, professions, families, keycaps, couples
    for seq in [
        "\u{1F468}\u{200D}\u{1F9B2}",                         // bald man
        "\u{1F469}\u{200D}\u{1F9B0}",                         // woman, red hair
        "\u{1F9D1}\u{1F3FD}\u{200D}\u{1F9B2}",               // person, medium skin, bald
        "\u{1F44D}\u{1F3FB}",                                  // thumbs up, light skin
        "\u{1F469}\u{200D}\u{1F4BB}",                         // woman technologist
        "\u{1F3C3}\u{200D}\u{2642}\u{FE0F}",                 // man running
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}", // family (parts, GPOS)
        "\u{1F469}\u{1F3FD}\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F48B}\u{200D}\u{1F468}\u{1F3FB}", // kiss, two tones
        "\u{1F3F4}\u{200D}\u{2620}\u{FE0F}",                 // pirate flag
        "\u{2764}\u{FE0F}\u{200D}\u{1F525}",                 // heart on fire
        "#\u{FE0F}\u{20E3}",                                   // keycap
        "1\u{FE0F}\u{20E3}",
    ] {
        // (Segoe UI Emoji's families are a little narrower than one emoji, as HarfBuzz has them)
        let w = measure(seq, 32.0);
        assert!(w > em * 0.85 && w < em * 1.1, "{seq:?} is {w} wide, one emoji is {em}");
    }
    // a joiner nothing joins changes nothing: two emoji side by side
    let apart = measure("\u{1F914}\u{200D}\u{1F914}", 32.0);
    assert!((apart - 2.0 * em).abs() < 2.0, "apart {apart} em {em}");

    Application::headless().run(|cx| {
        let win = open(cx, 520.0, 280.0, Page);
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

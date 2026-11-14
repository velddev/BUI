//! The view end to end on synthetic chat data: anchors, bottom pinning,
//! jumps, resize, budgets, the file-backed log, and allocation counts.

#![allow(clippy::float_cmp)]

use blenderui_paint::testing::{count_allocs, Counting};
use blenderui_paint::{Font, PaintSink, Palette, Rect, Rgba};
use blenderui_vlist::source::{ItemSource, Message, Page, Record, VecSource};
use blenderui_vlist::synth;
use blenderui_vlist::{Align, Budget, ChatList, ChatStyle, Estimator, ItemPainter, Lines, List, ListView, Measurer, Options, Pos};

#[global_allocator]
static A: Counting = Counting;

fn tw(s: &str, size: f32) -> f32 {
    s.chars().count() as f32 * size * 0.5
}

fn chat(n: u64, opts: Options) -> ChatList {
    ChatList::from_messages(synth::messages(0, n), Palette::blender_dark(), 600.0, 400.0, opts)
}

fn end_y(c: &ChatList) -> f32 {
    let p = c.view.placed().last().unwrap();
    p.y + p.h
}

/// A sink that counts calls and draws nothing.
#[derive(Default)]
struct Null {
    calls: usize,
}

impl PaintSink for Null {
    fn fill(&mut self, _: Rect, _: Rgba) {
        self.calls += 1;
    }
    fn stroke(&mut self, _: Rect, _: Rgba, _: f32) {
        self.calls += 1;
    }
    fn round(&mut self, _: Rect, _: f32, _: Rgba) {
        self.calls += 1;
    }
    fn line(&mut self, _: (f32, f32), _: (f32, f32), _: Rgba, _: f32) {
        self.calls += 1;
    }
    fn text(&mut self, _: f32, _: f32, _: &str, _: f32, _: Rgba) {
        self.calls += 1;
    }
    fn text_width(&mut self, t: &str, size: f32) -> f32 {
        tw(t, size)
    }
    fn clipped(&mut self, _: Rect, f: &mut dyn FnMut(&mut dyn PaintSink)) {
        f(self);
    }
}

#[test]
fn first_frame_fills_the_viewport_from_the_top() {
    let mut c = chat(1000, Options::default());
    let placed = c.layout(&mut tw).to_vec();
    assert_eq!(placed[0].item, 0);
    assert_eq!(placed[0].y, 0.0);
    assert!(end_y(&c) >= 400.0);
    assert!(placed.iter().all(|p| p.exact), "unlimited budget measures everything shown");
    for w in placed.windows(2) {
        assert!((w[0].y + w[0].h - w[1].y).abs() < 0.01, "items are contiguous");
    }
}

#[test]
fn opens_at_the_newest_item_touching_only_the_tail() {
    let mut c = chat(200_000, Options { start_at_end: true, ..Options::default() });
    c.layout(&mut tw);
    assert_eq!(c.view.placed().last().unwrap().item, 199_999);
    assert!((end_y(&c) - 400.0).abs() < 0.01);
    assert!(c.view.index().measured_items() <= 1100, "{}", c.view.index().measured_items());
    assert!(c.view.at_bottom());
}

#[test]
fn heights_changing_above_the_viewport_do_not_move_it() {
    let mut c = chat(50_000, Options::default());
    c.view.jump_to(25_000, Align::Start);
    c.layout(&mut tw);
    let before: Vec<_> = c.view.placed().iter().map(|p| (p.item, p.y)).collect();
    // an image arrives above, a block is re-measured above
    c.view.set_known_height(24_990, 500.0);
    c.view.changed(24_000);
    c.view.set_known_height(10, 900.0);
    c.layout(&mut tw);
    let after: Vec<_> = c.view.placed().iter().map(|p| (p.item, p.y)).collect();
    assert_eq!(before, after);
    // below: the anchor item stays
    c.view.set_known_height(25_005, 700.0);
    c.layout(&mut tw);
    assert_eq!((c.view.placed()[0].item, c.view.placed()[0].y), before[0]);
}

#[test]
fn prepending_history_does_not_jump() {
    let mut c = ChatList::from_messages(synth::messages(1000, 3000), Palette::blender_dark(), 600.0, 400.0, Options::default());
    c.view.jump_to(500, Align::Start);
    c.view.scroll_by(7.0);
    c.layout(&mut tw);
    let id = c.view.anchor_id(&c.source);
    let first = (c.view.placed()[0].item, c.view.placed()[0].y);
    c.prepend(synth::messages(0, 1000));
    c.layout(&mut tw);
    assert_eq!(c.view.anchor_id(&c.source), id);
    assert_eq!(c.view.placed()[0].item, first.0 + 1000);
    assert!((c.view.placed()[0].y - first.1).abs() < 0.01);
    assert_eq!(c.source.len(), 3000);
}

#[test]
fn stick_to_bottom_follows_until_the_user_scrolls_up() {
    let mut c = chat(500, Options { start_at_end: true, ..Options::default() });
    c.layout(&mut tw);
    assert!(c.view.at_bottom());
    c.push(synth::message(500));
    c.layout(&mut tw);
    assert_eq!(c.view.placed().last().unwrap().item, 500);
    assert!((end_y(&c) - 400.0).abs() < 0.01, "follows the new message");
    // scroll up: stops following
    c.view.scroll_by(-120.0);
    c.layout(&mut tw);
    assert!(!c.view.at_bottom());
    let id = c.view.anchor_id(&c.source);
    c.push(synth::message(501));
    c.layout(&mut tw);
    assert_eq!(c.view.anchor_id(&c.source), id, "still where the user left it");
    assert!(c.view.placed().last().unwrap().item < 501);
    // back to the bottom: follows again
    c.view.scroll_by(1e9);
    c.layout(&mut tw);
    assert!(c.view.at_bottom());
    c.push(synth::message(502));
    c.layout(&mut tw);
    assert_eq!(c.view.placed().last().unwrap().item, 502);
}

#[test]
fn streaming_growth_of_the_last_message_stays_pinned() {
    let mut c = chat(300, Options { start_at_end: true, ..Options::default() });
    c.layout(&mut tw);
    let mut text = String::new();
    let mut last_h = 0.0;
    for k in 0..60 {
        text.push_str("tokens keep arriving and the message keeps growing ");
        c.edit(299, &text);
        c.layout(&mut tw);
        assert!((end_y(&c) - 400.0).abs() < 0.01, "step {k}");
        let h = c.view.placed().last().unwrap().h;
        assert!(h >= last_h);
        last_h = h;
    }
    assert!(last_h > 100.0);
}

#[test]
fn deleting_in_the_middle_keeps_the_anchor() {
    let mut c = chat(5000, Options::default());
    c.view.jump_to(2000, Align::Start);
    c.layout(&mut tw);
    let id = c.view.anchor_id(&c.source);
    c.remove(100);
    c.layout(&mut tw);
    assert_eq!(c.view.anchor_id(&c.source), id);
    // deleting the anchor item itself: the next item takes its place
    let at = match c.view.pos() {
        Pos::Anchor { item, .. } => item,
        Pos::Bottom => unreachable!(),
    };
    c.remove(at);
    c.layout(&mut tw);
    assert_eq!(c.view.placed()[0].item, at);
    assert_eq!(c.source.len(), 4998);
}

#[test]
fn resize_invalidates_heights_and_keeps_the_reader_in_place() {
    let mut c = chat(20_000, Options::default());
    c.view.jump_to(8000, Align::Start);
    c.layout(&mut tw);
    let id = c.view.anchor_id(&c.source);
    assert!(c.view.index().measured_items() > 0);
    c.resize(300.0, 400.0);
    assert_eq!(c.view.index().measured_items(), 0, "measured heights were for the old width");
    c.layout(&mut tw);
    assert_eq!(c.view.anchor_id(&c.source), id);
    assert!(c.view.placed().iter().all(|p| p.exact));
    assert!(c.view.placed().iter().map(|p| p.h).sum::<f32>() >= 400.0);
}

#[test]
fn jump_converges_on_the_target() {
    let mut c = chat(300_000, Options::default());
    for (item, align) in [(123_456usize, Align::Center), (200_000, Align::End), (7, Align::Start), (299_999, Align::End)] {
        c.view.jump_to(item, align);
        for _ in 0..4 {
            c.layout(&mut tw);
        }
        let p = c.view.placed().iter().find(|p| p.item == item).copied().unwrap_or_else(|| panic!("{item} not shown"));
        assert!(p.exact);
        match align {
            Align::Start => assert!(p.y.abs() < 0.5, "{p:?}"),
            Align::Center => assert!((p.y + p.h * 0.5 - 200.0).abs() < 2.0, "{p:?}"),
            Align::End => assert!((p.y + p.h - 400.0).abs() < 2.0, "{p:?}"),
        }
    }
}

#[test]
fn a_fling_draws_skeletons_within_the_budget_then_catches_up() {
    let mut c = chat(100_000, Options { fling_budget: Budget::Items(2), ..Options::default() });
    c.layout(&mut tw);
    c.view.scroll_by(1.0e5);
    c.view.scroll_by(1.0e5);
    assert!(c.view.flinging());
    c.layout(&mut tw);
    let skeletons = c.view.placed().iter().filter(|p| !p.exact).count();
    assert!(skeletons > 0, "the budget left some unmeasured");
    let mut sink = Null::default();
    c.paint(&mut sink);
    assert!(sink.calls > 0);
    for _ in 0..40 {
        c.layout(&mut tw);
    }
    assert!(c.view.placed().iter().all(|p| p.exact));
}

#[test]
fn idle_prefetch_measures_ahead_of_the_scroll() {
    let mut c = chat(20_000, Options::default());
    c.layout(&mut tw);
    let last = c.view.placed().last().unwrap().item;
    assert!(!c.view.index().height_of(last + 20).unwrap().1, "not measured yet");
    let mut rounds = 0;
    while c.prefetch(&mut tw, Budget::Items(10)) && rounds < 100 {
        rounds += 1;
    }
    assert!(c.view.index().height_of(last + 20).unwrap().1, "measured ahead");
    assert!(c.view.index().height_of(last + 1).unwrap().1);
}

#[test]
fn wheel_eases_and_goes_idle() {
    let mut c = chat(2000, Options::default());
    c.layout(&mut tw);
    c.view.wheel(300.0, false);
    let mut frames = 0;
    while c.view.tick(1.0 / 60.0) {
        c.layout(&mut tw);
        frames += 1;
        assert!(frames < 500);
    }
    assert!(frames > 3, "eased over several frames");
    let (top, _) = c.view.scroll_extent();
    assert!((top - 300.0).abs() < 1.0, "{top}");
    assert!(!c.view.tick(1.0 / 60.0), "idle: no more frames");
}

#[test]
fn hostile_inputs() {
    let mut c = chat(0, Options::default());
    assert!(c.layout(&mut tw).is_empty());
    c.view.scroll_by(f64::NAN);
    c.view.scroll_by(f64::INFINITY);
    c.view.jump_to(usize::MAX, Align::Center);
    c.view.scroll_to_fraction(f64::NAN);
    c.view.wheel(f64::NAN, false);
    let mut c = chat(10, Options::default());
    c.view.jump_to(usize::MAX, Align::End);
    c.view.scroll_by(f64::MAX);
    c.view.scroll_by(-f64::MAX);
    c.layout(&mut tw);
    c.resize(0.0, 0.0);
    c.layout(&mut tw);
    c.resize(f32::NAN, 100.0);
    c.layout(&mut tw);
    assert_eq!(c.view.index().len(), 10);
}

#[test]
fn a_source_changed_behind_the_view_is_resynced() {
    let mut c = chat(1000, Options::default());
    c.layout(&mut tw);
    c.source.push(synth::message(1000));
    c.layout(&mut tw);
    assert_eq!(c.view.index().len(), 1001);
}

#[test]
fn the_estimate_converges_as_the_user_scrolls() {
    let mut c = chat(100_000, Options::default());
    c.layout(&mut tw);
    let (_, first_total) = c.view.scroll_extent();
    for _ in 0..400 {
        c.view.scroll_by(350.0);
        c.layout(&mut tw);
    }
    let (_, later) = c.view.scroll_extent();
    // the true total, measured the slow way
    let style = c.style.clone();
    let mut layout = blenderui_vlist::TextLayout::new(1 << 20);
    let mut page = Page::new();
    c.source.fetch(0..100_000, &mut page);
    let mut real = 0.0f64;
    for i in 0..100_000 {
        real += layout.height(&page.record(i).unwrap(), &style, 600.0, &mut tw).px();
    }
    let (e0, e1) = ((first_total - real).abs() / real, (later - real).abs() / real);
    assert!(e1 < 0.08, "estimate off by {e1:.3} after scrolling (first guess {e0:.3})");
}

#[test]
fn log_source_round_trips_messages() {
    let dir = std::env::temp_dir().join(format!("bui-vlist-log-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join("chat");
    let msgs = synth::messages(0, 5000);
    let mut w = blenderui_vlist::log::LogWriter::create(&base).unwrap();
    for m in &msgs {
        w.append(m).unwrap();
    }
    w.finish().unwrap();
    let src = blenderui_vlist::log::LogSource::open(&base).unwrap();
    assert_eq!(src.len(), 5000);
    let mut page = Page::new();
    src.fetch(1234..1300, &mut page);
    for (i, m) in msgs.iter().enumerate().take(1300).skip(1234) {
        let r = page.record(i).unwrap();
        assert_eq!((r.id, r.time, r.sender, r.flags, r.text), (m.id, m.time, m.sender, m.flags, m.text.as_str()), "{i}");
        assert_eq!(r.media_h, m.media_h / 4 * 4);
    }
    let want: u64 = msgs[10..4000].iter().map(|m| m.text.len() as u64).sum();
    assert_eq!(src.bytes(10..4000), want);
    assert_eq!(src.index_of(77), Some(76));
    let style = ChatStyle::new(Palette::blender_dark());
    let mut v = ListView::new(&src, &style, 600.0, 400.0, Options { start_at_end: true, ..Options::default() }).unwrap();
    assert_eq!(v.layout(&src, &style, &mut tw).last().unwrap().item, 4999);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn steady_state_frames_do_not_allocate() {
    let mut c = chat(50_000, Options::default());
    c.view.jump_to(20_000, Align::Start);
    let mut sink = Null::default();
    for _ in 0..6 {
        c.layout(&mut tw);
        c.paint(&mut sink);
    }
    let n = count_allocs(|| {
        for k in 0..30 {
            c.view.scroll_by(if k % 2 == 0 { 2.0 } else { -2.0 });
            c.layout(&mut tw);
            c.paint(&mut sink);
        }
    });
    assert_eq!(n, 0, "a steady frame allocated");
}

#[test]
fn a_uniform_list_needs_no_measuring() {
    let style = ChatStyle::new(Palette::blender_dark());
    let mut v = ListView::uniform(10_000_000, 24.0, &style, 600.0, 480.0, Options::default()).unwrap();
    v.scroll_by(1.0e8);
    assert!(v.index().heap_bytes() < 2_000_000);
    assert_eq!(v.index().measured_items(), 0);
}

#[test]
fn tanstack_virtual_options_and_offsets() {
    // visible_range is the placed items; scroll_to_offset lands inside the content
    let mut c = chat(5000, Options::default());
    c.layout(&mut tw);
    let r = c.view.visible_range().unwrap();
    assert_eq!(r.start, 0);
    assert_eq!(r.end, c.view.placed().last().unwrap().item + 1);
    c.view.scroll_to_offset(2000.0);
    c.layout(&mut tw);
    let (top, total) = c.view.scroll_extent();
    assert!((top - 2000.0).abs() < 1.0 || top > 1500.0, "top {top}");
    assert!(total > 2000.0);
    c.view.scroll_to_offset(f64::NAN);
    c.view.scroll_to_offset(1e12);
    c.layout(&mut tw);
    assert!(c.view.at_bottom(), "past the end is the end");
    // estimate_size: the assumed height of an unmeasured one-line item moves the total
    let small = chat(5000, Options { estimate_size: Some(24.0), ..Options::default() });
    let big = chat(5000, Options { estimate_size: Some(96.0), ..Options::default() });
    assert!(big.view.scroll_extent().1 > small.view.scroll_extent().1 * 2.0);
}

/// Rows of 10 px text wrapped at 100 px; sender 1 is code (monospace).
/// Painting notes each row's line count (`None`: a skeleton).
struct Faces(Palette, std::cell::RefCell<Vec<Option<usize>>>);

impl Measurer for Faces {
    fn font_size(&self) -> f32 {
        10.0
    }
    fn line_height(&self) -> f32 {
        12.0
    }
    fn wrap_width(&self, _: &Record<'_>, _: f32) -> f32 {
        100.0
    }
    fn chrome(&self, _: &Record<'_>) -> f32 {
        0.0
    }
    fn estimator(&self, list_width: f32) -> Estimator {
        Estimator::new(0.0, 12.0, 5.0, f64::from(list_width))
    }
    fn fonts(&self, rec: &Record<'_>, face: &mut dyn FnMut(usize, Font)) {
        if rec.sender == 1 {
            face(0, Font::Mono);
        }
    }
}

impl ItemPainter for Faces {
    fn palette(&self) -> &Palette {
        &self.0
    }
    fn item(&self, _: &mut dyn PaintSink, _: Rect, _: &Record<'_>, lines: &Lines) {
        self.1.borrow_mut().push(Some(lines.count()));
    }
    fn skeleton(&self, _: &mut dyn PaintSink, _: Rect, _: Option<&Record<'_>>) {
        self.1.borrow_mut().push(None);
    }
}

#[test]
fn faces_are_measured_in_their_font() {
    // 30 chars: 150 px regular (2 lines), 300 px in the twice as wide monospace (3 lines)
    let text = "aaaa bbbb cccc dddd eeee ffff ".to_owned();
    let msgs = (0..2).map(|k| Message { id: k, time: 0, sender: k as u16, flags: 0, media_h: 0, text: text.clone(), media: String::new() });
    let list = || List::new(VecSource::from_messages(msgs.clone().collect()), Faces(Palette::blender_dark(), Default::default()), 600.0, 400.0, Options::default()).unwrap();
    let mut l = list();
    let mut fw = |s: &str, size: f32, f: Font| tw(s, size) * if f == Font::Mono { 2.0 } else { 1.0 };
    let placed = l.layout_fonts(&mut fw).to_vec();
    assert_eq!((placed[0].h, placed[1].h), (24.0, 36.0));
    // painting finds the lines wrapped in their faces
    l.paint(&mut Null::default());
    assert_eq!(*l.style.1.borrow(), [Some(2), Some(3)]);
    // the old call measures everything regular
    let mut l = list();
    let placed = l.layout(&mut tw).to_vec();
    assert_eq!((placed[0].h, placed[1].h), (24.0, 24.0));
}

/// Records what is painted, `over` last.
struct Over(Palette, std::cell::RefCell<Vec<String>>);

impl Measurer for Over {
    fn font_size(&self) -> f32 {
        10.0
    }
    fn line_height(&self) -> f32 {
        12.0
    }
    fn wrap_width(&self, _: &Record<'_>, _: f32) -> f32 {
        100.0
    }
    fn chrome(&self, _: &Record<'_>) -> f32 {
        0.0
    }
    fn estimator(&self, list_width: f32) -> Estimator {
        Estimator::new(0.0, 12.0, 5.0, f64::from(list_width))
    }
}

impl ItemPainter for Over {
    fn palette(&self) -> &Palette {
        &self.0
    }
    fn item(&self, _: &mut dyn PaintSink, _: Rect, rec: &Record<'_>, _: &Lines) {
        self.1.borrow_mut().push(format!("item {}", rec.id));
    }
    fn skeleton(&self, _: &mut dyn PaintSink, _: Rect, _: Option<&Record<'_>>) {}
    fn over(&self, _: &mut dyn PaintSink, w: f32, h: f32, placed: &[blenderui_vlist::Placed]) {
        let first = placed.first().map_or(usize::MAX, |p| p.item);
        self.1.borrow_mut().push(format!("over {w}x{h} from {first} y {}", placed.first().map_or(0.0, |p| p.y)));
    }
}

#[test]
fn over_paints_after_the_items_with_where_they_are() {
    let msgs = (0..100).map(|k| Message { id: k, time: 0, sender: 0, flags: 0, media_h: 0, text: "a".into(), media: String::new() }).collect();
    let mut l = List::new(VecSource::from_messages(msgs), Over(Palette::blender_dark(), Default::default()), 600.0, 120.0, Options::default()).unwrap();
    l.layout(&mut tw);
    l.paint(&mut Null::default());
    let painted = l.style.1.take();
    assert_eq!(painted.len(), 11, "ten 12 px items fill 120 px, then over: {painted:?}");
    assert_eq!(painted[0], "item 0");
    assert_eq!(painted.last().unwrap(), "over 600x120 from 0 y 0");
    // scrolled 18 px: item 1 half above the top
    l.view.scroll_by(18.0);
    l.layout(&mut tw);
    l.paint(&mut Null::default());
    assert_eq!(l.style.1.take().last().unwrap(), "over 600x120 from 1 y -6");
}

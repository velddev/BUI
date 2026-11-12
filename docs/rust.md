# Rust: gpui syntax, HTML and CSS

The `blenderui` crate (`bindings/rust/`) gives Rust programs Blender's UI with
the syntax of [gpui](https://www.gpui.rs): views implement `Render`, elements
are built with `div().flex().gap_2().child(...)`, and listeners come from
`cx.listener(...)`. On top of gpui's model it supports **every HTML element**
and **CSS**: `<style>` sheets, selectors, the cascade, flexbox, grid, tables,
inline text that wraps, lists, `<details>`, and form controls that look like a
browser's, or like Blender's real widgets inside `styled` and as the
capitalized Blender components.

<p align="center"><img src="images/rust-showcase.png" width="640" alt="The Rust showcase example"></p>

- [Install](#install)
- [The app model](#the-app-model)
- [Elements](#elements)
- [Styling](#styling)
- [Layout](#layout)
- [Form controls](#form-controls)
- [Events](#events)
- [Painting your own content](#painting-your-own-content)
- [Headless rendering and tests](#headless-rendering-and-tests)
- [Coverage](#coverage)
- [Performance vs gpui](#performance-vs-gpui)
- [Limitations](#limitations)
- [How it works](#how-it-works)

## Install

`blenderui` is on [crates.io](https://crates.io/crates/blenderui):

```sh
cargo add blenderui
```

or, by hand, `blenderui = "0.2"` under `[dependencies]`. (The Dear ImGui crate is
[`blenderui-imgui`](https://crates.io/crates/blenderui-imgui): `cargo add blenderui-imgui`.)

Building needs **CMake** and a **C compiler**: `blenderui-sys` compiles the C
kit with the `cmake` crate. On Windows the default Rust target
(`x86_64-pc-windows-msvc`) uses MSVC (Visual Studio Build Tools). To build
against another checkout of the C sources, set `BUI_SOURCE_DIR`.

From the repository:

```sh
cd bindings/rust
cargo run --example showcase                        # the window above
cargo run --example showcase -- --screenshot a.png   # render it off-screen to a PNG
cargo run --example counter                         # the README example
cargo test                                          # unit, coverage and headless tests
```

The workspace uses edition 2024 (Rust 1.88 or newer, the `rust-version` in
`bindings/rust/Cargo.toml`). On Linux, building also needs the X11 headers
(`libx11-dev` / `libX11-devel`).

## The app model

It follows gpui closely:

| gpui                                     | blenderui                                                     |
| ---------------------------------------- | ------------------------------------------------------------- |
| `Application::new().run(\|cx\| ...)`      | the same; `Application::headless()` renders off-screen        |
| `cx.open_window(options, \|window, cx\| cx.new(\|cx\| View))` | the same; returns `WindowHandle<View>`   |
| `impl Render for View`                    | the same: `fn render(&mut self, window, cx) -> impl IntoElement` |
| `Entity<T>`, `WeakEntity<T>`, `cx.new`    | `Copy` handles into the app's entity slots: `read` (returns `&T`), `update`, `downgrade`; `cx.release(e)` drops a value |
| `Context<T>`: `notify`, `listener`, `entity` | the same; it derefs to `App`                              |
| `WindowOptions { window_bounds, titlebar, .. }`, `WindowBounds::Windowed(..)` | the same, plus `is_resizable`, `window_min_size` |
| `cx.set_global`, `cx.global`              | the same                                                       |
| `cx.stop_propagation()`                   | the same                                                       |

```rust
use blenderui::prelude::*;

struct Todo { items: Vec<String>, draft: String }

impl Render for Todo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().p_4().flex().flex_col().gap_2()
            .child(h2().child(format!("{} items", self.items.len())))
            .child(ul().children(self.items.iter().map(|s| li().child(s.clone()))))
            .child(div().flex().gap_2()
                .child(input().id("draft").placeholder("New item").value(self.draft.clone())
                    .on_change(cx.listener(|this, e: &ChangeEvent, _, cx| {
                        this.draft = e.value.clone();
                        cx.notify();
                    })))
                .child(button().child("Add").on_click(cx.listener(|this, _, _, cx| {
                    this.items.push(std::mem::take(&mut this.draft));
                    cx.notify();
                }))))
    }
}
```

`cx.notify()` re-renders. Rendering builds a fresh element tree; elements with
an `.id(...)` keep their state across renders (scroll position, a text field's
contents, a checkbox you clicked, which `<details>` are open).

`Window` gives listeners `viewport_size()`, `scale_factor()`, `mouse_position()`,
`set_window_title()`, `focus_id()` / `focused_id()`, `is_hovered(id)`,
`scroll_offset(id)` / `set_scroll_offset(id, x, y)`, `refresh()` and
`request_animation_frame()` (call it from `render` to animate; frames are paced
to the display, `set_vsync(false)` lifts the cap).
A `<title>` element sets the window title too.

## Markup: `rsx!` / `view!`

JSX-style markup, lowered at compile time to the same builder calls (no runtime parsing):

```rust
rsx! {
    <div class="flex gap-2 p-4" onClick={|| println!("clicked")}>
        <h1 style="color: red">"Hello, " {name}</h1>
        <input type="checkbox" checked={done} />
        <ul>{for it in items { <li>{it}</li> }}</ul>
        {if done { <p>all done</p> }}
        {match mode { Mode::A => <A/>, Mode::B => <span>b</span> }}
        <Card title="Settings">{...rows}</Card>
    </div>
}
```

The lowering is uniform, so the whole API is reachable without special cases:
- **Tags:** any tag becomes its function (`<my-el>` becomes `Div::new("my-el")`).
- **Attributes:**
  - `name="text"` sets an HTML attribute, except `class`, `style` and `id`, which call their methods.
  - `name={expr}` calls the builder method of that name: `w={px(10.)}`, `hover={|s| s.bg(red())}`.
  - `onClick` becomes `.on_click`, and closures may omit the `(event, window, cx)` parameters.
- **Components:** a component tag builds its props through a builder, so `<Card title="x">..</Card>` becomes `Card::__builder().title("x").children(..)`. `#[component]` generates the builder; a hand-written struct component needs `#[derive(Props)]`. `key={..}` on any component ties its hook state to the key.
- **Fragments:** `<>..</>` is a `display: contents` wrapper.

Tailwind utilities can also be written as bare attributes (UnoCSS calls this "attributify"). They become classes at compile time:

```rust
rsx! { <div p-4 w-1/2 bg-[#4772b3] hover:bg-sky-500/50 md:flex -mt-2 text-sm/6 /> }
```

HTML boolean attributes (`disabled`, `checked`, `hidden`, `open` ...) stay attributes.
Anything that looks like a utility but isn't one is a compile error with suggestions,
in `rsx!` and in `tw!` alike:

```text
error: rsx!: `bg-sky-50O` is not a Tailwind utility; did you mean `bg-sky-500` or `bg-sky-50` or `bg-sky-100`?
error: rsx!: `hovr:bg-red-500` is not a Tailwind utility; did you mean `hover:bg-red-500` or `odd:bg-red-500`?
error: tw!: `roundd-lg` is not a Tailwind utility; did you mean `rounded-lg` or `rounded-s-lg` or `rounded-e-lg`?
```

Every Tailwind variant also works as an attribute prefix, chained as deep as you like:

```rust
rsx! {
    <button class="px-3 py-1 bg-zinc-800"
            hover:class="bg-sky-600 text-white"        // classes, prefixed
            active:style="transform: scale(0.97)"      // any CSS, as arbitrary properties
            focus-visible:outline="2px solid white"    // one CSS property
            md:dark:hover:class="bg-sky-400"           // chained variants
            hover:bg={rgb(0x4772b3)}>                  // typed value via .hover(|s| ..)
        "Save"
    </button>
}
```

`group-hover:`, `peer-checked:`, `data-[state=open]:`, `aria-expanded:`, `@md:`, `2xl:`,
`max-sm:`, `*:` and every other variant work the same way, because they all go through the
Tailwind engine. Typed `{expr}` values are limited to `hover`, `active` and `focus`; use
`variant:class` or `variant:style` with every other variant.

## Components and hooks (zopra / React / Leptos style)

```rust
use blenderui::prelude::*;

#[component]                                   // zopra style: lowercase, &str props
fn profile(username: &str) {
    let (likes, set_likes) = use_state(0);     // `likes()`, `set_likes(..)`: no cx
    use_effect!(move || println!("{} likes", likes()), [likes()]);
    let theme: Theme = use_context();
    rsx! { <div class="p-2" onClick={move || set_likes(likes() + 1)}>{username.to_string()} " " {likes().to_string()}</div> }
}

#[component]                                   // Leptos style
fn Counter(start: i32) {
    let (count, set_count) = signal(start);
    let doubled = Memo::new(move || count.get() * 2);          // recomputed when `count` changes
    Effect::new(move || println!("count is {}", count.get())); // re-runs when `count` changes
    rsx! { <button onClick={move || set_count.update(|c| *c += 1)}>{doubled.get().to_string()}</button> }
}

// in a view: provide_context(Theme(..)); rsx! { <profile username="Ada" /> <Counter start={1} /> }
```

These are the hooks and their counterparts in other frameworks:

| Hook | Also known as |
|---|---|
| `use_state(v)` | zopra / React `useState` (a getter and a setter closure) |
| `use_signal(v)`, `Signal::new(v)` | zopra `use_signal`, Leptos `RwSignal` |
| `signal(v)`, `create_signal(v)` | Leptos read and write halves |
| `Memo::new(f)`, `Effect::new(f)` | Leptos, tracking the signals they read automatically |
| `use_effect(f, deps)`, `use_effect!(f, [a, b])` | React / zopra |
| `use_memo(f, deps)` | React `useMemo` |
| `provide_context(v)`, `use_context()`, `try_use_context()` | Leptos / zopra context |

How it behaves:
- **Signals:** signals are `Copy` handles. They don't need `cx`, and writing one re-renders its window.
- **Hook identity:** like React, a hook's state is tied to its call site and to the order in which components render. In lists, give components a `key` (`<Row key={item.id} ../>`) so their state follows the item: without keys, removing an item from the middle shifts the state of the ones after it.
- **Components:** `#[component]` turns `fn name(props)` into a props struct with that name. Uppercase and lowercase names both work in `rsx!`. The body is a normal block that ends in markup, usually an `rsx!`.
- **Text children:** `{expr}` in markup takes anything that turns into an element: text (`&'static str`, `String`, `SharedString`), an element or a component. Numbers and borrowed `&str` props aren't text on their own, so write `{count().to_string()}` and `{label.to_string()}`.
- **Optional props:** `Option<T>` props (and `children`) may be left out in `rsx!`; they are `None`. Leaving out any other prop is a compile error naming it (`a required prop is missing: ... Row_missing_name`). Struct components get the same with `#[derive(Props)]`.
- **Lifetime:** a component's state is freed when it stops rendering.
- **Implementation:** there is no `Rc` or `RefCell` anywhere in it.

Not supported yet: async resources (`use_resource`) and generic components.

## Elements

There is **one element type, `Div`**, and a constructor for every element of
the WHATWG HTML index: `div()`, `span()`, `section()`, `table()`, `tr()`,
`details()`, `dialog()`, `input()`, `select()`, `video()` ... (113 elements,
plus `svg()` and `math()` as boxes). Tags only change the defaults: what the
user-agent stylesheet gives them and how they are realised.

| Kind      | Elements                                                                                  | Realised as                                                    |
| --------- | ----------------------------------------------------------------------------------------- | -------------------------------------------------------------- |
| Box       | `div`, `p`, `h1`-`h6`, `a`, `span`, `ul`/`ol`/`li`, `table` ..., `details`, `dialog`, `form` ... | laid out and painted from CSS                           |
| Widget    | `button`, `input`, `select`, `textarea`, `progress`, `meter` (+ `option`, `optgroup`, `datalist`) | Blender's own widgets ([below](#form-controls))        |
| Replaced  | `img`, `video`, `audio`, `iframe`, `embed`, `object`, `canvas`                            | an intrinsic size; `img` shows image files or RGBA pixels     |
| Metadata  | `head`, `title`, `style`, `script`, `meta`, `link`, `template` ...                        | not rendered (`<style>` and `<title>` are read)               |
| Break     | `br`, `wbr`                                                                               | line breaks                                                    |

Unknown names work too: `custom("my-widget")` is a box, like a custom element
in a browser.

Every element has `.child(..)`, `.children(..)`, `.id(..)`, `.class(..)`,
`.attr(name, value)` and typed attributes: `.href`, `.value`, `.placeholder`,
`.input_type`, `.name`, `.checked`, `.disabled`, `.selected`, `.open`, `.min`,
`.max`, `.step`, `.src`, `.alt`, `.title`, `.colspan`. `FluentBuilder` adds
`.when(cond, |el| ..)`, `.when_some(opt, |el, v| ..)` and `.map(..)`.
Text children are `&'static str`, `String` or `SharedString`.

> `main()` (the `<main>` element) has the same name as your program's
> `fn main`. Inside `fn main` call it as `blenderui::html::main()`.

The document is wrapped in `<html><body>` unless your root already is one, so
`body { margin: 0 }` and the user-agent defaults behave as in a browser.

### Blender styling: `styled`

Plain elements look like they do in a browser: black text on white, 16 px,
`#0000ee` links, and Chrome's form controls, in Blender's UI font (Inter). Mark an element `styled` and it, and everything inside it, gets
Blender's look instead: the `#303030` background, `#e6e6e6` 13 px UI-font
text, blue links, dark `code` / `pre`, themed tables, fieldsets and dialogs,
and Blender's widgets for the form controls.

```rust
body().styled().child(h1().child("Blender look"))     // gpui syntax
rsx! { <div styled class="p-4">"Blender look"</div> } // rsx! / view! (Leptos-style)
div().styled().child(div().attr("styled", "false"))   // opt a subtree back out
```

Your own CSS still wins over either set of defaults. From JavaScript use
`el.setAttribute('styled', '')`, or `<div styled>` in JSX.

**Dark or light.** `styled` follows Blender's theme, which is Dark by default.
Switch every window to Blender's Light preset with
`Application::new().theme("light")`, `blenderui::set_theme("light")` at run
time (open windows redraw), or `BUI_THEME=light` without code changes. The
widgets, panels and the `styled` sheet all follow; plain HTML keeps the
browser's look in both. `blenderui::theme_name()` and
`blenderui::theme_color("region_bg")` read the active theme.

### Blender components

The capitalized components are Blender's widgets, always, inside `styled` or
not: `Button`, `Checkbox`, `Radio`, `NumberField` (drag to scrub),
`NumberSlider`, `TextField`, `TextArea`, `ColorSwatch` (Blender's color
picker), `Dropdown` and `ProgressBar`. Each is the element it stands for, so
every element method and attribute works on it:

```rust
rsx! {
    <NumberSlider min={0.0} max={10.0} value="2.5" onChange={|e: &ChangeEvent| println!("{:?}", e.number)} />
    <Button onClick={|| println!("apply")}>"Apply"</Button>
    <Dropdown><option>"Cube"</option><option>"Sphere"</option></Dropdown>
}
div().child(Checkbox().checked(true)).child(Button().child("Apply"))   // gpui syntax
```

In JavaScript they're exported from `@blender-ui/react` (`<NumberSlider min={0} max={10} />`)
and work as tag names with the DOM API (`win.createElement('NumberSlider')`).
The lowercase HTML elements (`button`, `input type="range"` ...) look like a
browser's controls, and like Blender inside `styled`. Run
`cargo run --example forms` to see all three side by side.

## Styling

Three ways, one cascade:

```rust
// 1. gpui / Tailwind methods
div().flex().items_center().gap_2().px_3().py_1().rounded_md().bg(rgb(0x3a3a3a))
    .text_color(white()).shadow_md().hover(|s| s.bg(rgb(0x4772b3)))

// 2. CSS blocks, checked at compile time (a misspelled property doesn't compile)
div().style(css!("border-left: 3px solid #4772b3; padding: 4px 8px"))

// 3. CSS built at run time
div().css("grid-template-columns", format!("repeat({n}, 1fr)"))
     .style(format!("width: {w}px"))

// 4. stylesheets, anywhere in the tree
style().child(".card:hover > h4 { color: #e6a23c } @media (max-width: 600px) { nav { display: none } }")
```

**`Styled` methods** (~1,300, generated from Tailwind's scales): spacing and
sizing on the spacing scale (`p_4`, `mx_auto`, `w_64`, `h_full`, `w_1_3`,
`size(px(20.0))`, `gap_x_2`), display and position (`flex`, `grid`, `block`,
`hidden`, `absolute`, `inset_0`, `z_index`), flexbox (`flex_col`, `flex_1`,
`items_center`, `justify_between`, `flex_wrap`), grid (`grid_cols(3)`,
`col_span(2)`), color (`bg`, `text_color`, `border_color`, `opacity`),
borders and corners (`border_1`, `border_t_2`, `rounded_lg`, `rounded_full`),
shadows (`shadow_sm` ... `shadow_2xl`), text (`text_sm` ... `text_6xl`,
`font_bold`, `italic`, `underline`, `uppercase`, `truncate`, `leading_tight`,
`whitespace_nowrap`), overflow (`overflow_y_scroll`) and cursors
(`cursor_pointer`). Colors: `rgb(0xRRGGBB)`, `rgba(0xRRGGBBAA)`,
`hsla(h, s, l, a)`, named helpers, or any CSS color string.

**State styles**: `.hover(|s| ..)`, `.active(|s| ..)` and `.focus(|s| ..)`
take a refinement with the same methods (they are CSS `:hover` / `:active` /
`:focus` rules for that element).

**Selectors**: type, `*`, `#id`, `.class`, attributes (`[a]`, `=`, `~=`,
`|=`, `^=`, `$=`, `*=`), combinators (descendant, `>`, `+`, `~`), and
pseudo-classes: `:hover`, `:active`, `:focus`, `:focus-within`,
`:focus-visible`, `:checked`, `:disabled`, `:enabled`, `:link`, `:any-link`,
`:required`, `:optional`, `:read-only`, `:read-write`, `:open`,
`:placeholder-shown`, `:first-child`, `:last-child`, `:only-child`,
`:first-of-type`, `:last-of-type`, `:nth-child()`, `:nth-last-child()`,
`:nth-of-type()`, `:nth-last-of-type()`, `:empty`, `:root`, `:not()`,
`:is()`, `:where()`. `@media` with width and height conditions is evaluated against the
window.

**Cascade**: user-agent sheet, then author sheets, then inline declarations
(methods, `.css`, `.style`), then state refinements; `!important` wins over
all of them, and specificity and order decide within a level. `inherit`,
`initial`, `unset` and `revert` are understood (they copy or reset the text,
color, size and display properties). Custom properties (`--name`) inherit,
and `var()` (with fallbacks, `var(--missing, 20px)`) and `env()` are
substituted, also inside other functions (`calc(var(--gap) * 2)`). Units: `px`, `em`, `rem`, `%`, `vw`,
`vh`, `vmin`, `vmax`, `ch`, `ex`, `pt`, `pc`, `in`, `cm`, `mm`, `Q`, and
`calc()`, `min()`, `max()`, `clamp()`.

## Layout

Layout is computed with [taffy](https://github.com/DioxusLabs/taffy):

- **Block** flow with margins, padding, borders, `box-sizing`, min/max sizes
  and `aspect-ratio`.
- **Inline** content becomes line boxes: text breaks into words that wrap,
  inline elements (`b`, `a`, `code`, `mark` ...) style their words, and
  inline-blocks and widgets sit on the line. `white-space` (`normal`,
  `nowrap`, `pre`, `pre-wrap`, `pre-line`), `text-align`, `text-indent`,
  `letter-spacing`, `word-spacing`, `vertical-align: sub | super`,
  `text-transform` and `text-overflow: ellipsis` apply.
- **Flexbox** and **grid**: every property of both modules, including
  `grid-template-areas`, named lines, `repeat()`, `minmax()`, `fr`,
  `auto-fill` / `auto-fit`, spans and line placement.
- **Tables**: columns are as wide as their widest content and line up across
  rows; a table with an auto width shrinks to its columns (up to its
  container), like a browser's auto layout. `table-layout: fixed` takes the
  first row's widths. `colspan`, `border-spacing`, `border-collapse`,
  `caption-side` and `empty-cells` apply.
- **Positioning**: `relative`, `absolute`, `fixed` (laid out like `absolute`,
  placed against the window), `inset`
  properties, `z-index` for painting and hit testing, and every 2D transform
  (`translate`, `scale`, `rotate`, `skew`, `matrix`, `transform-origin`, the
  individual `rotate` / `scale` / `translate` properties).
- **Bidirectional text**: Hebrew and Arabic runs are reordered inside
  left-to-right text (a compact Unicode Bidirectional Algorithm), `direction: rtl`
  and `unicode-bidi: bidi-override` apply.
- **Scroll snapping**: `scroll-snap-type` (`x` / `y` / `both`, `mandatory` /
  `proximity`) and `scroll-snap-align` on the children.
- **Overflow**: `hidden` / `clip` clip, `scroll` / `auto` scroll with the
  wheel (Shift for horizontal) and show overlay scrollbars.
- **Lists**: `list-style-type` (`disc`, `circle`, `square`, `decimal`,
  `lower-alpha`, `upper-roman` ...), `list-style-position`, `<ol start>`.
- **`<details>` / `<summary>`**: clicking the summary toggles the details
  (`.open(true)` sets the initial state); `<dialog>` shows with `.open(true)`.

## Form controls

Outside `styled`, form elements look and behave like Chrome's (its colors and
shapes, typed number fields with spin buttons, a range thumb that jumps to
the click). Inside `styled`, and as the capitalized Blender components, they
are Blender's widgets, with Blender's interaction (drag to scrub numbers,
Ctrl+Z, double-click to type, Escape to cancel):

| HTML                                              | Blender widget                       | `ChangeEvent`                         |
| ------------------------------------------------- | ------------------------------------ | ------------------------------------- |
| `button()` with text, `input().input_type("submit" \| "button" \| "reset")` | button          | `on_click`                            |
| `input()`: `text`, `password` (masked), `email`, `search`, `url`, `tel`, `date` ... | text field  | `value`                               |
| `input().input_type("number")`                    | number field                          | `number`, `value`                     |
| `input().input_type("range")`                     | slider                                | `number`, `value`                     |
| `input().input_type("checkbox")`                  | checkbox                              | `checked`                             |
| `input().input_type("radio")` (grouped by `name`) | radio button                          | `checked`                             |
| `input().input_type("color")`                     | color swatch (opens Blender's picker) | `value` as `#rrggbb`                  |
| `select()` with `option()` / `optgroup()`         | dropdown menu                         | `number` (index), `value` (text)      |
| `textarea()`                                      | multi-line text editor                | `value`                               |
| `progress()`, `meter()`                           | progress bar                          | (display only)                        |

`on_input` and `on_change` both fire when the value changes. Values you pass
(`.value(..)`, `.checked(..)`) are pushed to the widget only when they change
between renders, so a field the user is typing in is not reset; bind it to your
state (as in the todo example) to control it. A `button()` holding elements
other than text is laid out as an ordinary box (style it with CSS) and still
gets `on_click`.

## Events

| Listener                                   | Event                                                   | Bubbles |
| ------------------------------------------ | ------------------------------------------------------- | ------- |
| `on_click`                                 | `ClickEvent { down, up }` (press and release on the element) | yes |
| `on_mouse_down(button, ..)`, `on_any_mouse_down` | `MouseDownEvent { button, position, modifiers, click_count }` | yes |
| `on_mouse_up(button, ..)`                  | `MouseUpEvent`                                          | yes     |
| `on_mouse_move`                            | `MouseMoveEvent { position, pressed_button, modifiers }` | yes    |
| `on_hover`                                 | `bool`: entered / left                                  | no      |
| `on_scroll_wheel`                          | `ScrollWheelEvent { position, delta, modifiers }`       | yes     |
| `on_key_down`, `on_key_up`                 | `KeyDownEvent { keystroke: Keystroke { key, key_char, modifiers } }` | yes (from the focused element) |
| `on_change`, `on_input`                    | `ChangeEvent { value, checked, number }`                | yes     |

Positions are logical pixels. Clicking an element focuses it; key events go
to the focused element and bubble up. `cx.stop_propagation()` stops bubbling.

## Images

`<img src>` and CSS background images read every format the kit reads (PNG,
JPEG, WebP, GIF, BMP, TGA, QOI, Radiance HDR, PNM, SVG). Layout takes the size
from the file's header; the pixels decode on the kit's worker threads at the
size drawn and the image appears when ready (`App::draw` waits for them, so
headless pixels are deterministic). `Image::open` / `Image::decode` decode
synchronously, `Painter::image_file` / `image_file_cover` draw a file in a
canvas, `CellRender::Image` is a data-table image column and
`Record::media` a chat message's picture. `blenderui::image_stats()` and
`set_image_budget` see and size the shared cache. Animated GIF and WebP
play (paused while off screen); `<img frame="2">` holds a frame,
`set_reduced_motion(true)` holds them all, and `Animation` steps one
yourself (`Image::update_texture` for Dear ImGui). See
[Images](https://bui.preetham.top/docs/concepts/images/).

`<video src="intro.mp4" loop poster="intro.jpg" />` plays a video file (muted
autoplay; `autoplay="false"` holds the first frame): the OS decodes it at the
size drawn, only while it shows. Keep a `blenderui::Video` to control it
(`play`, `pause`, `seek`, `set_rate`, `time`, `duration`, `on_state`) and
pass it with `video().video(rc.clone())` or `Painter::video`. Cargo features
(`png`, `jpeg`, `video`, `gpu`, ... from `features.json`, all default) drop
what an app doesn't use. See [Video](https://bui.preetham.top/docs/concepts/video/).

## Painting your own content

`canvas(|bounds, painter| ..)` is gpui's `canvas()`: it paints with a
`Painter` in logical pixels, clipped to its box.

```rust
canvas(|b, p| {
    p.fill(b, rgb(0x232323));
    p.quad(b, 6.0, rgba(0x4772b380), rgb(0x4772b3), 1.0);       // rounded rect + border
    p.line(&[(b.origin.x, b.origin.y), (b.origin.x + 80.0, b.origin.y + 40.0)], white(), 2.0);
    p.oval(Bounds { origin: point(10.0, 10.0), size: size(20.0, 20.0) }, red(), black(), 1.0);
    p.text(b.origin.x + 4.0, b.origin.y + 4.0, "hello", 13.0, white());
})
.w(px(200.0)).h(px(100.0))
```

`Painter` also has `polygon`, `arc` (pie slices), `image` (scaled) and
`with_clip`. Images: `img().src("file.png")` (PNG, decoded once and cached) or
`img().image_rgba(w, h, pixels)`.

## Data tables and long lists

`DataTable` is a virtualized table with the API of gpui-component's, drawn the way Blender draws its spreadsheet editor (`space_spreadsheet`): fixed row height, `first_row = scroll_y / row_height`, only the visible rows and the columns that intersect the region, drawn straight into the painter. A frame costs O(visible cells) at any row count, allocates nothing in its own drawing, and sorting or filtering produces a row index list (Blender's `IndexMask`) without copying data.

```rust
use blenderui::table::{Column, DataTable, TableDelegate, TableState};

impl CellSource for People {
    fn cell(&self, row: usize, col: usize) -> Cell<'_> { Cell::U64(row as u64) } // called for visible cells only
}
impl TableDelegate for People {
    fn columns_count(&self, _: &App) -> usize { 2 }
    fn rows_count(&self, _: &App) -> usize { 1_000_000 }
    fn column(&self, ix: usize, _: &App) -> Column { Column::new("id", "ID").sortable().fixed_left() }
}
let table = cx.new(|cx| TableState::new(People, window, cx));
// in render: DataTable::new(&table).stripe(true).on_select(|row, _, _| ...)
```

Features: sticky header and pinned columns, resize and move by dragging headers, click to sort, row (multi, Shift/Ctrl, Ctrl+A), cell and column selection, keyboard navigation, Ctrl+C as TSV, stripes, a row-number column, loading and empty states, `load_more`, Blender dark and light themes. The logic lives once, in the dependency-free `blenderui-table` crate (visible-range math and column culling, index masks and typed parallel sorts, lazy typed cells formatted without allocation, selection, keyboard navigation, column layout, a text-run cache, and the painting through a `PaintSink` trait). blenderui's `table` module is one adapter around it (`Host`, `PaintSink`, element); `bindings/rust/blenderui-table-gpui` is the same for real gpui, with gpui-component's names and a `render_td` escape hatch, outside the main workspace (it builds gpui). On real gpui, 1,000,000 x 20, it takes 1.45 ms per scrolled frame against gpui-component 0.9.0's 4.75 ms (see the website page "Data tables" for the method). `uniform_list(id, count, |range, window, cx| ...)` (gpui's signature) builds only the visible items of a list of real elements, from the same range math. Measured on 1,000,000 x 20: 0.1 ms to build, 1.1 ms per scrolled frame, about 22 ms for a header-click sort; see `examples/bench_table.rs` and the website page "Data tables". Not built yet in blenderui: element cells (`render_td`, available on gpui), a popup context menu, multi-cell ranges, JavaScript and Python wrappers.

### The shadcn Data Table

`blenderui::shadcn` puts the API of shadcn/ui's Data Table (TanStack Table's `ColumnDef`, table state and row models) on the same painted table, in `rsx!` and builder syntax: `<DataTable columns={columns} data={data} />` / `DataTable::new(columns, data)`, `ColumnDef::accessor_key` / `accessor_fn` (the row type implements `Record`), `select_column()` and `actions_column()`, `use_table()`, and the helper components `DataTableColumnHeader`, `DataTablePagination`, `DataTableViewOptions` and `DataTableToolbar`. Sorting, column and global filters, pagination, row selection and column visibility are TanStack's state, uncontrolled or controlled with `on_..._change` callbacks, over row models that are index lists. A shadcn `cell: ({ row }) => <JSX />` becomes a typed `CellRender` (text with a number format, checkbox, badge, progress, row actions) that the table paints straight from the data, so a steady frame still allocates nothing; there is no `flexRender`. The logic is in `blenderui_table::tanstack` (core, no framework); the components are in `blenderui/src/shadcn/`. Names, limits and numbers are on the website page "shadcn Data Table", and `examples/site_shadcn_table.rs` is the payments table of the recipe.

## Variable-height lists and chat

`blenderui::vlist` is a virtualized list whose items have different heights (wrapped text, images), built for chat and optimal from 100 items to 10M and beyond with one design. The logic lives once, in the dependency-free `blenderui-vlist` crate (on `blenderui-paint`, which also holds the `PaintSink` trait, the theme `Palette` and the bounded `TextCache` that `blenderui-table` uses):

- **`HeightIndex`**: blocks of up to 1024 items under a Fenwick tree with spare slots on both sides. A block is `Uniform` (a count and a height), `Estimated` (a count and the text bytes it holds; nothing per item) or `Measured` (`u32` prefix sums, 1/64 px, plus an exactness bit per item). A list of one block skips the tree. Offset to item and item to offset are O(log blocks) plus a binary search in a block; a height change is O(log blocks); adding or prepending a page adds blocks without shifting. Heights are fixed-point integers, so ten million rows of 60 px stay exact. Memory follows what was scrolled through.
- **Anchor-based scrolling** (`ListView`): the position is (item, offset inside it), or the end of the list (`stick_to_bottom`, open at the newest item). Heights changing above the viewport, prepended history and edits can't move what is on screen; resize keeps the reader's place; `jump_to` re-aims until the target is measured; the scrollbar thumb comes from the estimate (`Estimator`, calibrated by every measurement).
- **Measuring** (`Measurer`, `TextLayout`): Blender's `textview_wrap_offsets` in pixels, with an ASCII advance table, cached per (text, width, size), a per-frame time budget while flinging (unmeasured items draw as skeletons), idle `prefetch` in the scroll direction, and image heights carried in the record.
- **Sources** (`ItemSource`): `VecSource` and `log::LogSource`, a file-backed append-only log (16-byte records plus a text arena, positioned reads), so 10M messages need a few MB of RAM; `ensure_loaded` and `cancel` for asynchronous loading.

```rust
use blenderui::vlist::{chat_list, synth, theme_palette, ChatList, ListState, Options};

let opts = Options { start_at_end: true, ..Options::default() };
let chat = cx.new(|cx| ListState::new(ChatList::from_messages(synth::messages(0, 1_000_000), theme_palette(), 760.0, 436.0, opts), cx));
// in render: chat_list(&chat)            // click selects a whole message
// elsewhere: chat.update(cx, |s, _| s.push(message));   // follows while at the bottom
```

For items that are not messages, build a `List` from your own `ItemSource`, `Measurer` and `ItemPainter` and show it with `variable_list(&state)`. `bindings/rust/blenderui-vlist-gpui` is the same list for real gpui (outside the main workspace); its benchmark against gpui's `list` and gpui-component's `virtual_list` is in `bench/`. Measured numbers and the method are on the website page "Variable-height lists". Not built yet: selecting text across messages, a background measuring thread, damage regions, JavaScript and Python wrappers.

`blenderui::shadcn` also has `VirtualList` (`count` and an `item` closure that writes each item's text, so ten million rows cost a screenful) and `ChatList`, with TanStack Virtual's names: `estimate_size`, `overscan`, `get_item_key`, `on_range_change`, and `scroll_to_index` / `scroll_to_offset` / `measure_element` on the `ListHandle` that `use_virtual_list()` returns. They sit on `ListState` and `ItemSource` (the new `CountSource` and `RowStyle` in `blenderui-vlist`), so the file-backed source works under them. See the website page "Virtual list API" and `examples/site_shadcn_list.rs`.

## Headless rendering and tests

`Application::headless()` runs the same code without a native window (and is
what you get on platforms without one). Drive it from the `run` closure:

```rust
Application::headless().run(|cx| {
    let win = cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| Counter { count: 0 }))
        .unwrap();
    let b = cx.element_bounds(win.id, "plus").unwrap();         // the element with .id("plus")
    cx.simulate_click(win.id, b.origin.x + 2.0, b.origin.y + 2.0);
    assert_eq!(win.root().read(cx).count, 1);
    let (w, h, pixels) = cx.draw(win.id);                       // 0x00RRGGBB, device pixels
});
```

Also `simulate_mouse_move`, `simulate_scroll` and `simulate_key(text, keysym)`.
The kit has global state, so one `Application` runs at a time per process
(`run` takes a lock; parallel tests queue up).

## Coverage

From `cargo test --test parity -- --nocapture`:

| Area                                  | Coverage                                                                         |
| ------------------------------------- | -------------------------------------------------------------------------------- |
| HTML elements (WHATWG index)          | **113 / 113** constructible, plus `svg` / `math` roots; 9 are Blender widgets     |
| CSS properties                        | **271** change layout or rendering; **137** more parse but have no effect         |
| W3C property index (745 properties)   | **261** rendered (35%), **129** parsed without effect (17%), **355** not recognized (48%): most of these are drafts, speech, paged media and SVG |
| CSS 2.1, screen properties            | **90 / 90** rendered, 90 / 90 parsed                                              |
| Flexbox                               | **12 / 12**                                                                       |
| Grid                                  | **23 / 23** (including `grid-template-areas` and named lines)                     |
| Tailwind CSS v4                       | Classes compile on first use (v4 theme, variants, arbitrary values); `tw!` checks them at compile time |

Beyond properties, the engine supports:
- **Values:** every CSS math function (`calc`, `min`, `max`, `clamp`, `round`, `mod`, `rem`, trigonometry, `pow`, `hypot` ...) with mixed units, every length unit, and `var()` / `env()`.
- **Colors:** CSS Color 4/5: `oklch`, `lab`, `color()`, `color-mix()`, relative colors and `light-dark()`.
- **Selectors:** Selectors Level 4 (`:has()`, `:is()`, `:where()`, `:nth-child(of S)` ...), nesting, and `::before` / `::after` with `content`.
- **At-rules:** `@media` (Media Queries 4 ranges), `@supports`, `@layer` and `@scope`.

```rust
div().class(tw!("grid grid-cols-3 gap-4 md:grid-cols-6 hover:bg-sky-500/50"))
div().style("width: calc(100% - 2 * var(--gap)); background: color-mix(in oklch, var(--brand) 60%, white)")
```

Every CSS 2.1 screen property renders.

**Effects** work on the element's pixels: `filter` and `backdrop-filter`
(`blur`, `brightness`, `contrast`, `grayscale`, `hue-rotate`, `invert`,
`opacity`, `saturate`, `sepia`, `drop-shadow`), `mix-blend-mode` (all 16 modes
and `plus-lighter`), `clip-path` (`inset()`, `circle()`, `ellipse()`,
`polygon()`, reference boxes), `mask` / `mask-image` (gradients and PNGs,
alpha or luminance, with size, position and repeat) and rotate / skew / scale
/ matrix transforms. The element's subtree is drawn offscreen with the
canvas' own drawing code, filtered, clipped, masked and resampled (bilinear,
like Blender's image transform), then composited back with the W3C formulas;
the result is cached, so a page that doesn't change costs nothing.
`border-image` (gradients and PNGs, slice, width, outset, `stretch` /
`repeat` / `round` / `space`, `fill`) draws the nine slices.

Also rendered: every `border-style` and `outline-style` (`dashed`, `dotted`, `double`, `groove`, `ridge`, `inset`, `outset`, following rounded corners with Blender's corner tessellation), `text-decoration-style` (including `wavy`), `-thickness` and `text-underline-offset`, `background-clip` / `background-origin` (`border-box`, `padding-box`, `content-box`), `background-attachment: fixed`, `clip: rect()` on absolutely positioned boxes, counters (`counter-reset` / `-increment` / `-set` with `counter()` and `counters()` in `content`), `empty-cells`, `list-style-image`, `word-break: break-all`, `overflow-wrap: anywhere | break-word` and `tab-size`.

Also supported: `background-image: url()` (every image format) with `background-size` / `-position` (`-x` / `-y`) / `-repeat`, `object-fit` / `object-position`, logical border longhands (`border-inline-width` ...), `text-wrap-mode`, `white-space-collapse`, `scale`, `@container` size queries (`container-type`, `container-name`), `@font-face` (local font files) and `@property` initial values.

How much is left, against all 745 properties in the
[W3C index](https://www.w3.org/Style/CSS/all-properties.en.html):

| | Properties | Share |
|---|---:|---:|
| Change layout or rendering | 261 | 35% |
| Parsed, no effect yet | 129 | 17% |
| Not recognized | 355 | 48% |

The 355 unrecognized properties break down as:

| Group | Properties |
|---|---:|
| Draft border and corner shapes (`corner-shape`, `border-*-clip` ...) | ~78 |
| SVG fill, stroke and markers | ~40 |
| Gap decorations (`row-rule-*` ...) | 38 |
| Speech and aural | 31 |
| Animation triggers and scroll-driven animation | 26 |
| Paged media, regions, rhythm and others | ~25 |
| Everything else | ~100 |

The everyday screen CSS is much further along: Flexbox 12/12, Grid 23/23 and
CSS 2.1 90/90. The largest remaining gaps among common properties are 3D
transforms, OpenType font features and multi-column layout.

**Animations and transitions** run like a browser's: `@keyframes` (percent
lists, `from` / `to`, inside `@media`, per-keyframe
`animation-timing-function`), every `animation-*` longhand and the shorthand
(lists of animations, negative delays, `infinite`, every direction, fill mode
and play state), and `transition` / `transition-*` (`all` or property lists,
`transition-behavior: allow-discrete`; an interrupted transition starts from its
current value, and one sent back to where it came from takes only as long as it
ran). Easing: `linear`, `ease*`, `cubic-bezier()`, `steps()` with every jump
position, `step-start` / `-end` and `linear()` points. Lengths, percentages,
numbers, colors (in premultiplied sRGB), `box-shadow`, `text-shadow`,
`transform` (function by function), `rotate`, `scale`, `filter` /
`backdrop-filter` (function by function), `visibility` and `z-index` interpolate; other
properties flip at 50%. A window renders frames only while something moves:
finished or paused animations cost no CPU. Headless windows run on a virtual
clock that `App::advance_clock` moves, so tests step animations exactly.

Not done yet:
- **Parsed, not applied:** OpenType font features (`font-feature-settings`, `font-variant-numeric` ...): the text engine has no shaping, and Blender's text library doesn't either. Multi-column layout (`columns`), `writing-mode`, `scroll-padding` / `scroll-margin`.
- **3D transforms:** `perspective`, `rotateX` / `rotateY`, `matrix3d` have no effect; 2D transforms don't change hit testing (clicks use the untransformed box).
- **Animations:** no `animationend` / `transitionend` events, no animations on `::before` / `::after`, and an element starts transitions only when it already had a `transition` in the previous frame (e.g. not when `transition` appears only under `:hover` on the first hover). Lengths mixing units that resolve differently (`50%` to `100px`, `auto`) flip at 50% instead of going through `calc()`.
- **`@container` timing:** queries use the container's size from the previous layout; when it changes, the window renders once more.
- **Effects:** `filter: url(#svg)`, `clip-path: path()`, and `mix-blend-mode` across stacking contexts (it blends with what was painted before it).
- **The rest of the W3C property index:** 355 entries this engine doesn't know, mostly drafts, speech, paged media and SVG.
- **`all`:** counted as parsed-only. Its CSS-wide keywords (`all: initial`, `inherit`, `unset`, `revert`) reset an element's computed style; there's nothing else it can take.
- **Box sizing:** the default is `border-box` (like Tailwind's preflight), not CSS's `content-box`.

## Performance vs gpui

The same UI on blenderui (GPU renderer and default software raster),
[gpui-fast](https://github.com/longbridge/gpui-fast) 0.1.13 and gpui 0.2.2:
a window of N rounded, labelled cells that all change every frame. Ryzen 7
5800U laptop at 60 Hz, all apps back to back in one session.
[bindings/rust/bench](../bindings/rust/bench/README.md) has the full table and
how to re-run it.

| | blenderui GPU | blenderui CPU | gpui-fast | gpui |
|---|---|---|---|---|
| 1000 / 3000 animated cells | **60 / 59.9 fps** | 60 / 57.6 fps | 49.2 / 31.5 fps | 36.4 / 19.9 fps |
| CPU per frame, 100 / 1000 / 3000 cells | **1.2 / 4.7 / 11.6 ms** | 4.6 / 10.2 / 22.0 ms | 2.7 / 20.5 / 31.9 ms + GPU | 4.7 / 27.7 / 50.5 ms + GPU |
| RAM (working set) | 28-40 MB + 19-22 MB GPU | **21-30 MB**, no GPU | 45-65 MB + 19-24 MB GPU | 40-77 MB + 16-21 MB GPU |
| First frame after launch | **~5 ms** | **~5 ms** | ~200 ms | ~220 ms |
| Idle CPU | **0 %** | 0-0.9 % | 0-0.4 % | 0.9-1.4 % |

### The GPU renderer

```rust
Application::new().gpu(true).run(|cx| { /* every window on the GPU */ });
// or one window:
cx.open_window(WindowOptions { gpu: Some(true), ..Default::default() }, build);
// window.is_gpu() turns true a few frames in, once the device is ready
```

`BUI_GPU=1` / `0` overrides both. It is Direct3D 11 on Windows and OpenGL 3.3 on Linux. Elsewhere,
or without a device, windows stay on the software raster. Every primitive
becomes one instance of a single shader, following Blender's instanced
widget-base batches. Text runs and images are cached in atlases. Frames
render into a persistent target, so only damaged regions redraw. Both
renderers produce the same pixels (the `gpu_parity` test).

The software raster draws in parallel bands (`BUI_RASTER_THREADS=1` forces
one thread). `BUI_PROFILE=1` prints per-phase frame times for either
renderer.

### Text

Text uses Blender's UI font, Inter, embedded in the library, so it looks
the same on every platform. The default family (and `sans-serif`,
`system-ui` or an unknown family) is Inter. Named families such as
`"Segoe UI"` or `monospace` load from the platform's fonts. `BUI_FONT=path.ttf`
overrides the default face, and you can register more faces:

```rust
blenderui::add_font("Brand", false, false, include_bytes!("Brand-Regular.ttf"));
```

With `.gpu(true)`, glyphs are drawn straight from their outlines using
Eric Lengyel's [Slug](https://github.com/EricLengyel/Slug) algorithm: one
quad per glyph, exact coverage computed in the pixel shader, and no glyph
bitmaps. The alternative is a per-glyph bitmap atlas (Blender's approach):
about half the GPU work, but bitmaps per size. Choose with:

| | Slug (default) | Glyph atlas |
|---|---|---|
| Build time | `slug` feature (default) | `default-features = false` |
| Run time | `Application::new().slug(true)` / `blenderui::set_slug(true)` | `.slug(false)` / `set_slug(false)` |
| Environment (overrides) | `BUI_SLUG=1` | `BUI_SLUG=0` |

The `gdi-text` feature uses Windows' GDI engine and system fonts.

## Limitations

- **Floats** place block boxes, but text does not wrap around them. Right-to-left
  text is reordered (bidi) but not shaped (no Arabic joining).
- **Fonts**: `font-family` picks a face per element (registered, `@font-face`,
  installed or generic `monospace` / `serif`); there is no per-glyph fallback
  between faces of a list. Characters the face lacks come from the emoji face
  only (register one as `add_font("emoji", ...)`, else Segoe UI Emoji, Twemoji,
  Noto Emoji or OpenMoji from the font folders; COLR v0 fonts draw in colour).
  Emoji sequences (ZWJ, flags, skin tones) draw as their components.
- **Effects and animation**: see "Not done yet" under [Coverage](#coverage).
- **Transforms**: every 2D transform draws, but hit testing uses the
  untransformed box; 3D transforms have no effect.
- **Inline boxes** paint their background per word; borders and padding on
  inline elements (`<kbd>`, `<code>`) are not drawn.
- **Widgets draw above page content**, so a `z-index`ed box (a `<dialog>`)
  over a form field does not cover the field.
- **Images**: an animation inside a CSS effect layer (filters, masks, 3D
  transforms) shows its first frame. `video`, `audio`,
  `iframe` show a placeholder.
- **Windows** are native on Windows (Win32), Linux (X11) and macOS (Cocoa,
  new, tried by hand so far on three Macs (two Apple Silicon, one Intel); it needs the main thread). The GPU
  renderer runs on Windows and Linux.
- `text-align: justify` aligns left. Table columns are sized from their
  widest single-line content, a simpler version of browsers' auto layout.

## How it works

```
 Render::render ─► element tree (one Div type for every tag)
        │
  cascade: UA sheet ▸ <style> sheets ▸ inline ▸ :hover/:active/:focus ▸ !important
        │          one handler table applies every property
  box tree: blocks, line boxes of words, flex/grid items, tables as flex
        │
  taffy layout ─► absolute boxes, clips, scroll offsets
        │
  paint ─► one display list on a kit canvas      form controls ─► kit widget nodes,
        │  (rects, rounded rects, text, images)                 placed at their boxes
        ▼
  C kit: input, hit testing, Blender widget drawing, damage rendering, Win32 / X11 window
```

- `blenderui-sys` is the raw FFI over `kit/include/BUI_kit.h`; its build script
  builds the C kit with CMake and links it statically.
- Every style source (methods, `css!`, `.css`, `.style`, sheets, the
  user-agent sheet, state refinements) becomes CSS declarations applied by
  one table of property handlers, so a property has exactly one
  implementation.
- **Compile time**: the property table is a `const`. Style methods and
  `css!` resolve property names to indices when the program compiles (a
  typo is a compile error), typed methods (`.w(px(40.0))`, `.bg(rgb(..))`)
  build their value token directly, and fixed values (`.flex()`,
  `.shadow_md()`, `css!` values) are tokenized once per program.
- **No `Rc` / `RefCell`**: a frame is a set of flat arenas (boxes, element
  data, taffy nodes) linked by index, owned by the window and reused every
  frame. Computed styles live in a per-window store, shared by every element
  with the same inputs. Listeners are owned `Box`es; while one runs, the
  frame is moved out of the window so it can take `&mut Window`. Entities
  are `Copy` handles into an app-owned slot map; the app itself lives on
  `Application::run`'s stack. Small per-element lists (matched rules,
  declarations) stay on the stack. The only shared ownership left is
  `Arc<Image>` for image pixels.
- **Reuse across frames**: `render()` builds a fresh element tree each frame,
  but the work behind it is reused. Matched selectors are memoized by an
  element fingerprint (what selectors can see: tag, id, attributes, position,
  state, ancestors, previous siblings). Computed styles are shared by input.
  The taffy tree persists and is reconciled by key, so only changed subtrees
  are laid out again. Widget nodes are reconciled by key too.
- Rendering happens on the next turn of the native message loop after
  events.

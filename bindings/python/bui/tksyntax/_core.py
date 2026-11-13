"""tkinter's foundation on the BUI kit.

The classes here mirror ``tkinter/__init__.py``: ``Variable`` and friends,
``Event``, ``Misc`` (after, bind, focus, winfo ...), the ``Pack`` / ``Place``
/ ``Grid`` geometry mixins, ``XView`` / ``YView``, ``BaseWidget`` /
``Widget``, ``Wm``, ``Tk`` and ``Toplevel``.

A widget *wraps* one kit node (``self._node``) rather than being one, so
tkinter's names (``get``, ``set``, ``type``, ``children`` ...) never clash
with the node API. Layout, drawing and input run in C; this module keeps
what tkinter itself keeps in Tcl: options, variables, bindings, names.

SPDX-License-Identifier: GPL-2.0-or-later
"""

from __future__ import annotations

import collections
import enum
import functools
import itertools
import math
import re
import sys
import time
import traceback
from typing import Optional

import buipy

from bui import host
from bui.components.core import (EV_CONFIGURE, EV_DOUBLE, EV_ENTER, EV_KEY, EV_LEAVE,
                                 EV_MOTION, EV_PRESS, EV_RELEASE, EV_VIEW, EV_WHEEL)
from .constants import *  # noqa: F401,F403

TkVersion = 8.6
TclVersion = 8.6
wantobjects = 1
READABLE, WRITABLE, EXCEPTION = 2, 4, 8
EV_KEYRELEASE = 20  # BK_EV_KEYRELEASE

_PACK, _GRID, _PLACE = 1, 2, 3
_AN, _AS, _AW, _AE = 1, 2, 4, 8  # kit anchor bits


class TclError(Exception):
    """Raised where Tk would raise (bad index, unknown variable value ...)."""


# ==========================================================================
# Values: colors, fonts, distances, booleans

_NAMED_COLORS = {
    "white": "#ffffff", "black": "#000000", "red": "#ff0000", "green": "#008000",
    "blue": "#0000ff", "yellow": "#ffff00", "gray": "#bebebe", "grey": "#bebebe",
    "orange": "#ffa500", "purple": "#a020f0", "cyan": "#00ffff", "magenta": "#ff00ff",
    "lightgray": "#d3d3d3", "lightgrey": "#d3d3d3", "darkgray": "#a9a9a9",
    "darkgrey": "#a9a9a9", "pink": "#ffc0cb", "brown": "#a52a2a", "navy": "#000080",
    "gold": "#ffd700", "silver": "#c0c0c0", "lime": "#00ff00", "maroon": "#b03060",
    "teal": "#008080", "olive": "#808000", "darkblue": "#00008b", "darkgreen": "#006400",
    "darkred": "#8b0000", "lightblue": "#add8e6", "lightgreen": "#90ee90",
    "lightyellow": "#ffffe0", "skyblue": "#87ceeb", "steelblue": "#4682b4",
    "royalblue": "#4169e1", "dodgerblue": "#1e90ff", "tomato": "#ff6347",
    "salmon": "#fa8072", "coral": "#ff7f50", "khaki": "#f0e68c", "violet": "#ee82ee",
    "indigo": "#4b0082", "turquoise": "#40e0d0", "beige": "#f5f5dc", "ivory": "#fffff0",
    "snow": "#fffafa", "whitesmoke": "#f5f5f5", "gainsboro": "#dcdcdc",
    "dimgray": "#696969", "slategray": "#708090", "darkorange": "#ff8c00",
    "crimson": "#dc143c", "chocolate": "#d2691e", "tan": "#d2b48c", "wheat": "#f5deb3",
    "orchid": "#da70d6", "plum": "#dda0dd", "lavender": "#e6e6fa", "mintcream": "#f5fffa",
    "seagreen": "#2e8b57", "forestgreen": "#228b22", "limegreen": "#32cd32",
    "springgreen": "#00ff7f", "aquamarine": "#7fffd4", "deepskyblue": "#00bfff",
    "midnightblue": "#191970", "firebrick": "#b22222", "hotpink": "#ff69b4",
    "deeppink": "#ff1493", "goldenrod": "#daa520", "sienna": "#a0522d",
    # Tk's system colors map onto Blender's theme
    "systembuttonface": "#3d3d3d", "systemwindow": "#282828", "systemwindowtext": "#e6e6e6",
    "systembuttontext": "#e6e6e6", "systemhighlight": "#4772b3",
    "systemhighlighttext": "#ffffff", "systemdisabledtext": "#808080",
}


_COLOR_CACHE = {}


def color(value):
    """A Tk color (``#rgb``, ``#rrggbb``, ``#rrrrggggbbbb``, a name, or an
    RGB(A) tuple) as RGBA floats; None for "" / None. Color strings are
    parsed once (canvases re-send their items' colors on every change)."""
    if value is None or value == "":
        return None
    if isinstance(value, str):
        hit = _COLOR_CACHE.get(value)
        if hit is None:
            hit = _parse_color(value)
            if len(_COLOR_CACHE) > 4096:
                _COLOR_CACHE.clear()
            _COLOR_CACHE[value] = hit
        return hit
    return _parse_color(value)


def _parse_color(value):
    if isinstance(value, (tuple, list)):
        v = tuple(float(c) / (255.0 if max(value) > 1 else 1.0) for c in value)
        return v + (1.0,) if len(v) == 3 else v
    s = str(value).strip()
    key = s.lower().replace(" ", "")
    s = _NAMED_COLORS.get(key, s)
    if not s.startswith("#"):
        m = re.match(r"^(gr[ae]y)(\d+)$", key)  # gray0 .. gray100
        if m:
            v = round(int(m.group(2)) * 2.55)
            s = "#%02x%02x%02x" % (v, v, v)
        else:
            raise TclError('unknown color name "%s"' % value)
    hexs = s[1:]
    if len(hexs) in (3, 6, 9, 12):
        n = len(hexs) // 3
        vals = [int(hexs[i * n:(i + 1) * n], 16) / float(16 ** n - 1) for i in range(3)]
        return tuple(vals) + (1.0,)
    if len(hexs) == 8:
        return tuple(int(hexs[i:i + 2], 16) / 255.0 for i in range(0, 8, 2))
    raise TclError('invalid color name "%s"' % value)


def _try_color(value):
    try:
        return color(value)
    except TclError:
        return None


def _hex(rgba) -> str:
    return "#%02x%02x%02x" % tuple(round(c * 255) for c in rgba[:3])


_TK_DEFAULT_POINTS = 9.0  # TkDefaultFont on Windows: the kit's UI font size


def _font_parts(font):
    """(family, points, styles) of a Tk font spec: a tuple, a string
    ("Arial 12 bold", "{Courier New} 10"), a Font object or a named font."""
    if font is None or font == "":
        return "", 0.0, ()
    actual = getattr(font, "actual", None)
    if callable(actual) and not isinstance(font, (str, tuple, list)):
        a = actual()
        styles = tuple(s for s in (a.get("weight"), a.get("slant")) if s not in ("normal", "roman"))
        return a.get("family", ""), float(a.get("size", 0)), styles
    if isinstance(font, str):
        named = _named_fonts().get(font)
        if named is not None:
            return _font_parts(named)
        m = re.match(r"^\s*\{([^}]*)\}\s*(.*)$", font)
        if m:
            parts = [m.group(1)] + m.group(2).split()
        else:
            parts = font.split()
    else:
        parts = list(font)
    family = str(parts[0]) if parts else ""
    size = 0.0
    if len(parts) > 1:
        try:
            size = float(parts[1])
        except (TypeError, ValueError):
            size = 0.0
    styles = tuple(str(p).lower() for p in parts[2:])
    if len(parts) > 2 and isinstance(parts[2], (tuple, list)):
        styles = tuple(str(p).lower() for p in parts[2])
    return family, size, styles


def _named_fonts():
    from . import font as _font  # late: font imports this module
    return _font._named


def _font_scale(font) -> float:
    """Kit text scale for a Tk font (points; negative sizes are pixels)."""
    _, size, _ = _font_parts(font)
    if not size:
        return 0.0
    points = size if size > 0 else -size * 0.75
    return max(0.5, points / _TK_DEFAULT_POINTS)


def _font_bold(font) -> bool:
    return "bold" in _font_parts(font)[2]


def _pixels(value) -> float:
    """Tk screen distance ("10", "2c", "1i", "5m", "12p") in pixels."""
    if isinstance(value, (int, float)):
        return float(value)
    s = str(value).strip()
    m = re.match(r"^(-?[\d.]+)\s*([cimp]?)$", s)
    if not m:
        raise TclError('bad screen distance "%s"' % value)
    v, unit = float(m.group(1)), m.group(2)
    dpi = 96.0
    return v * {"": 1.0, "c": dpi / 2.54, "i": dpi, "m": dpi / 25.4, "p": dpi / 72.0}[unit]


def getint(s):
    try:
        return int(s)
    except (TypeError, ValueError):
        try:
            return int(str(s), 0)
        except ValueError:
            raise TclError('expected integer but got "%s"' % s) from None


def getdouble(s):
    try:
        return float(s)
    except (TypeError, ValueError):
        raise TclError('expected floating-point number but got "%s"' % s) from None


_TRUE = ("1", "true", "yes", "on", "t", "y")
_FALSE = ("0", "false", "no", "off", "f", "n")


def getboolean(s):
    if isinstance(s, bool):
        return s
    if isinstance(s, (int, float)):
        return bool(s)
    v = str(s).strip().lower()
    if v in _TRUE:
        return True
    if v in _FALSE:
        return False
    raise TclError('expected boolean value but got "%s"' % s)


def _anchor_bits(anchor) -> int:
    a = str(anchor or "center").lower()
    if a == "center":
        return 0
    return (_AN if "n" in a else 0) | (_AS if "s" in a else 0) | (_AW if "w" in a else 0) | \
        (_AE if "e" in a else 0)


def _pads(v):
    if isinstance(v, (tuple, list)):
        return int(_pixels(v[0])), int(_pixels(v[1] if len(v) > 1 else v[0]))
    if isinstance(v, str) and " " in v.strip():
        return _pads(v.split())
    p = int(_pixels(v or 0))
    return p, p


def _chars(n) -> int:
    """Tk text widths are in average characters."""
    return int(round(int(n) * buipy.kit_unit() * 0.42)) + round(0.8 * buipy.kit_unit())


def _font_px() -> float:
    return buipy.kit_unit() * 0.55


def _text_width(text: str) -> float:
    w = buipy.measure_text(text, _font_px()) if text else 0
    return float(w) if w is not None else len(text) * _font_px() * 0.55


def _wrap(text: str, width_px: float) -> str:
    """Word-wrap `text` to `width_px` (Tk's wraplength / Message)."""
    if width_px <= 0:
        return text
    out = []
    for para in str(text).split("\n"):
        line = ""
        for word in para.split(" "):
            cand = word if not line else line + " " + word
            if line and _text_width(cand) > width_px:
                out.append(line)
                line = word
            else:
                line = cand
        out.append(line)
    return "\n".join(out)


# ==========================================================================
# Variables

_var_counter = itertools.count()
_variables: dict = {}


class Variable:
    """tkinter.Variable: a named value with read/write/unset traces."""

    _default = ""

    def __init__(self, master=None, value=None, name=None):
        self._name = name or "PY_VAR%d" % next(_var_counter)
        self._root = master
        self._traces: dict = {}
        old = _variables.get(self._name)
        if value is not None:
            self._value = value
        elif old is not None:
            self._value = old._value
        else:
            self._value = self._default
        _variables[self._name] = self

    def __str__(self):
        return self._name

    def __repr__(self):
        return "<%s object %s>" % (type(self).__name__, self._name)

    def __eq__(self, other):
        return isinstance(other, Variable) and type(self) is type(other) and \
            self._name == other._name

    def __hash__(self):
        return hash(self._name)

    def _fire(self, mode):
        for cbname, (modes, fn) in list(self._traces.items()):
            if mode in modes and cbname in self._traces:
                fn(self._name, "", mode)

    def set(self, value):
        """Set the variable to VALUE."""
        self._value = value
        self._fire("write")

    initialize = set

    def _raw(self):  # read without firing read traces (widget sync)
        return self._value

    def get(self):
        """Return value of variable."""
        self._fire("read")
        return self._value

    def trace_add(self, mode, callback):
        modes = (mode,) if isinstance(mode, str) else tuple(mode)
        for m in modes:
            if m not in ("array", "read", "write", "unset"):
                raise TclError('bad operation "%s"' % m)
        cbname = "%d%s" % (next(_var_counter), getattr(callback, "__name__", "cb"))
        self._traces[cbname] = (modes, callback)
        return cbname

    def trace_remove(self, mode, cbname):
        self._traces.pop(cbname, None)

    def trace_info(self):
        return [(modes, cbname) for cbname, (modes, _fn) in self._traces.items()]

    _LETTERS = {"r": "read", "w": "write", "u": "unset", "a": "array"}

    def trace_variable(self, mode, callback):
        return self.trace_add(tuple(self._LETTERS[c] for c in mode), callback)

    trace = trace_variable

    def trace_vdelete(self, mode, cbname):
        self.trace_remove(mode, cbname)

    def trace_vinfo(self):
        rev = {v: k for k, v in self._LETTERS.items()}
        return [("".join(rev[m] for m in modes), cb) for modes, cb in self.trace_info()]

    def __del__(self):
        pass


class StringVar(Variable):
    _default = ""

    def get(self):
        v = super().get()
        return v if isinstance(v, str) else str(v)


class IntVar(Variable):
    _default = 0

    def get(self):
        v = super().get()
        try:
            return int(v)
        except (TypeError, ValueError):
            try:
                return int(getdouble(v))
            except TclError:
                raise TclError('expected integer but got "%s"' % v) from None


class DoubleVar(Variable):
    _default = 0.0

    def get(self):
        return getdouble(super().get())


class BooleanVar(Variable):
    _default = False

    def set(self, value):
        super().set(getboolean(value) if value != "" else value)

    initialize = set

    def get(self):
        v = super().get()
        try:
            return getboolean(v)
        except TclError:
            raise ValueError("invalid literal for getboolean()") from None


def _var_value(var):
    return var._raw() if isinstance(var, Variable) else _variables[str(var)]._raw()


# ==========================================================================
# Events

class EventType(enum.StrEnum):
    KeyPress = '2'
    Key = KeyPress
    KeyRelease = '3'
    ButtonPress = '4'
    Button = ButtonPress
    ButtonRelease = '5'
    Motion = '6'
    Enter = '7'
    Leave = '8'
    FocusIn = '9'
    FocusOut = '10'
    Keymap = '11'
    Expose = '12'
    GraphicsExpose = '13'
    NoExpose = '14'
    Visibility = '15'
    Create = '16'
    Destroy = '17'
    Unmap = '18'
    Map = '19'
    MapRequest = '20'
    Reparent = '21'
    Configure = '22'
    ConfigureRequest = '23'
    Gravity = '24'
    ResizeRequest = '25'
    Circulate = '26'
    CirculateRequest = '27'
    Property = '28'
    SelectionClear = '29'
    SelectionRequest = '30'
    Selection = '31'
    Colormap = '32'
    ClientMessage = '33'
    Mapping = '34'
    VirtualEvent = '35'
    Activate = '36'
    Deactivate = '37'
    MouseWheel = '38'

    def __str__(self):
        return self.name


class Event:
    """Container for the properties of an event (tkinter.Event)."""

    def __init__(self, **kw):
        self.serial = 0
        self.num = "??"
        self.focus = False
        self.height = self.width = "??"
        self.keycode = "??"
        self.state = 0
        self.time = int(time.monotonic() * 1000) & 0x7fffffff
        self.x = self.y = self.x_root = self.y_root = 0
        self.char = ""
        self.send_event = False
        self.keysym = "??"
        self.keysym_num = "??"
        self.type = EventType.VirtualEvent
        self.widget = None
        self.delta = 0
        self.__dict__.update(kw)

    def __repr__(self):
        attrs = {k: v for k, v in self.__dict__.items() if v != "??" and not k.startswith("_")}
        attrs.pop("widget", None)
        body = " ".join("%s=%r" % (k, v) for k, v in attrs.items()
                        if k not in ("serial", "time", "send_event", "focus"))
        return "<%s event %s>" % (getattr(self.type, "name", self.type), body)


# modifier bits (Tk's event.state)
_SHIFT, _LOCK, _CONTROL = 0x1, 0x2, 0x4
_ALT = 0x20000 if sys.platform == "win32" else 0x8
_B1, _B2, _B3 = 0x100, 0x200, 0x400
_MOD_BITS = {"shift": _SHIFT, "lock": _LOCK, "control": _CONTROL, "ctrl": _CONTROL,
             "alt": _ALT, "meta": _ALT, "mod1": _ALT, "option": _ALT, "command": _CONTROL,
             "b1": _B1, "button1": _B1, "b2": _B2, "button2": _B2, "b3": _B3, "button3": _B3}
_COUNTS = {"double": 2, "triple": 3, "quadruple": 4}
_TYPES = {"keypress": "KeyPress", "key": "KeyPress", "keyrelease": "KeyRelease",
          "buttonpress": "ButtonPress", "button": "ButtonPress",
          "buttonrelease": "ButtonRelease", "motion": "Motion", "enter": "Enter",
          "leave": "Leave", "mousewheel": "MouseWheel", "configure": "Configure",
          "focusin": "FocusIn", "focusout": "FocusOut", "destroy": "Destroy", "map": "Map",
          "unmap": "Unmap", "expose": "Expose", "visibility": "Visibility",
          "activate": "Activate", "deactivate": "Deactivate"}


def _kit_mods_to_state(mods: int) -> int:
    return ((_SHIFT if mods & 1 else 0) | (_CONTROL if mods & 2 else 0) |
            (_ALT if mods & 4 else 0) | (_B1 if mods & 8 else 0))


@functools.lru_cache(maxsize=512)
def _parse_sequence(sequence: str):
    """'<Control-Button-1>' -> (type, detail, modifier mask, click count).
    Virtual events -> ('Virtual', '<<Name>>', 0, 1)."""
    s = sequence.strip()
    if s.startswith("<<") and s.endswith(">>"):
        return ("Virtual", s, 0, 1)
    if not s.startswith("<"):  # "a": a key press
        return ("KeyPress", s[-1:] if s else "", 0, 1)
    parts = s.strip("<>").split("-")
    etype, detail, mask, count = None, None, 0, 1
    for i, p in enumerate(parts):
        low = p.lower()
        if detail is None and etype is None and low in _COUNTS:
            count = _COUNTS[low]
        elif etype is None and detail is None and low in _MOD_BITS and i < len(parts) - 1:
            mask |= _MOD_BITS[low]
        elif etype is None and low in _TYPES:
            etype = _TYPES[low]
        elif p == "" and i == len(parts) - 1:  # "<Control-->" (minus key)
            detail = "minus"
        else:
            detail = "-".join(parts[i:])
            break
    if etype is None:
        etype = "ButtonPress" if detail and detail.isdigit() else "KeyPress"
    if etype in ("ButtonPress", "ButtonRelease") and detail and not detail.isdigit():
        raise TclError('bad event type or keysym "%s"' % detail)
    return (etype, detail, mask, count)


def _matches(pattern, ev) -> bool:
    etype, detail, mask, count = pattern
    if etype != ev.type.name:
        return False
    if (ev.state & mask) != mask or count > getattr(ev, "_count", 1):
        return False
    if detail is None:
        return True
    if etype in ("ButtonPress", "ButtonRelease"):
        return str(ev.num) == detail
    if ev.keysym == detail:
        return True
    return bool(mask & (_CONTROL | _ALT)) and ev.keysym.lower() == detail.lower()


def _specificity(pattern):
    etype, detail, mask, count = pattern
    return (count, bin(mask).count("1"), detail is not None)


class _BindTable:
    """Bindings of one tag: sequence -> callbacks (Tk keeps one script per
    sequence; ``add`` appends)."""

    __slots__ = ("seqs",)

    def __init__(self):
        self.seqs: dict = {}

    def bind(self, sequence, func, add):
        _parse_sequence(sequence)  # validate now (Tk raises at bind time)
        funcs = self.seqs.setdefault(sequence, [])
        if not add or add == "":
            funcs.clear()
        funcs.append(func)
        return "%d%s" % (id(func), getattr(func, "__name__", ""))

    def unbind(self, sequence, funcid=None):
        funcs = self.seqs.get(sequence)
        if funcs is None:
            return
        if funcid is None:
            del self.seqs[sequence]
        else:
            funcs[:] = [f for f in funcs if "%d%s" % (id(f), getattr(f, "__name__", "")) != funcid]

    def match(self, ev):
        """Callbacks of the most specific sequence matching `ev` (Tk runs
        one binding per tag)."""
        best, best_key = None, None
        for seq, funcs in self.seqs.items():
            pat = _parse_sequence(seq)
            if _matches(pat, ev):
                key = _specificity(pat)
                if best_key is None or key > best_key:
                    best, best_key = funcs, key
        return best or ()

    def virtual(self, name):
        return self.seqs.get(name, ())


_bindtables: dict = collections.defaultdict(_BindTable)
_virtual_events: dict = {}  # "<<Copy>>" -> [physical sequences]
_class_tags_bound: set = set()  # tags other than widget paths that have bindings


def _dispatch(widget, ev):
    """Run the bindings of every bindtag of `widget` (widget, class,
    toplevel, all); "break" stops (Tk semantics)."""
    for tag in widget.bindtags():
        table = _bindtables.get(tag)
        if table is None:
            continue
        funcs = table.virtual(ev._virtual) if getattr(ev, "_virtual", None) else table.match(ev)
        for fn in list(funcs):
            if _call(widget, fn, ev) == "break":
                return "break"
    if not getattr(ev, "_virtual", None):
        for name, seqs in list(_virtual_events.items()):
            if any(_matches(_parse_sequence(s), ev) for s in seqs):
                widget.event_generate(name)
    return None


def _call(widget, fn, *args):
    try:
        return fn(*args)
    except Exception:
        root = widget._root() if widget is not None else _default_root
        if root is not None:
            root.report_callback_exception(*sys.exc_info())
        else:
            raise
        return None


# ==========================================================================
# Misc: services shared by every widget and window


_after_ids = itertools.count(1)
_commands: dict = {}
_option_db: list = []
_selection = {"PRIMARY": None, "CLIPBOARD": None}

_TK_CURSORS = {"": -1, "arrow": 0, "left_ptr": 0, "top_left_arrow": 0, "xterm": 3, "ibeam": 3,
               "hand1": 5, "hand2": 5, "pointinghand": 5, "crosshair": 4, "cross": 4,
               "tcross": 4, "sb_h_double_arrow": 1, "sb_v_double_arrow": 2,
               "size_we": 1, "size_ns": 2, "fleur": 6, "size": 6, "sizing": 6,
               "watch": 0, "wait": 0, "none": 7}


class Misc:
    """Methods shared by all widgets and windows (tkinter.Misc)."""

    _last_child_ids = None
    _name = ""
    _w = ""

    # -- identity -------------------------------------------------------------
    def __str__(self):
        return self._w

    def _root(self):
        w = self
        while w.master is not None:
            w = w.master
        return w

    def _toplevel_window(self):
        w = self
        while not getattr(w, "_is_window", False):
            w = w.master
        return w

    def winfo_toplevel(self):
        return self._toplevel_window()

    def nametowidget(self, name):
        name = str(name)
        if not name.startswith("."):
            w = self
        else:
            w = self._root()
            name = name[1:]
        for part in filter(None, name.split(".")):
            w = w.children[part]
        return w

    _nametowidget = nametowidget

    # -- timers -----------------------------------------------------------------
    def after(self, ms, func=None, *args):
        """Call FUNC with ARGS after MS milliseconds (sleep when no FUNC)."""
        if func is None:
            time.sleep(int(ms) / 1000.0)
            return None
        root = self._root()
        name = "after#%d" % next(_after_ids)

        def fire():
            root._afters.pop(name, None)
            _call(self, func, *args)
        win = self._toplevel_window()._window
        root._afters[name] = (win, win.after(max(0, int(ms)), fire), "timer")
        return name

    def after_idle(self, func, *args):
        name = self.after(0, func, *args)
        win, token, _ = self._root()._afters[name]
        self._root()._afters[name] = (win, token, "idle")
        return name

    def after_cancel(self, id):  # noqa: A002 - tkinter name
        if not id:
            raise ValueError("id must be a valid identifier returned from after or after_idle")
        entry = self._root()._afters.pop(id, None)
        if entry is not None:
            entry[0].after_cancel(entry[1])

    def after_info(self, id=None):  # noqa: A002
        afters = self._root()._afters
        if id is None:
            return tuple(afters)
        if id not in afters:
            raise TclError('event "%s" doesn\'t exist' % id)
        return (None, afters[id][2])

    # -- event loop ------------------------------------------------------------
    def mainloop(self, n=0):
        _show_windows()
        host.run()

    def quit(self):
        host.quit()

    def update(self):
        _show_windows()
        host.update()

    def update_idletasks(self):
        host.update()

    def wait_variable(self, name="PY_VAR"):
        var = name if isinstance(name, Variable) else _variables[str(name)]
        done = []
        cb = var.trace_add("write", lambda *a: done.append(1))
        try:
            _wait_until(lambda: done)
        finally:
            var.trace_remove("write", cb)

    waitvar = wait_variable

    def wait_window(self, window=None):
        window = window or self
        _wait_until(lambda: not window.winfo_exists())

    def wait_visibility(self, window=None):
        window = window or self
        _wait_until(lambda: window.winfo_ismapped() or not window.winfo_exists())

    # -- bindings ------------------------------------------------------------------
    def _bind(self, tag, sequence, func, add):
        if sequence is None:
            return tuple(_bindtables[tag].seqs) if tag in _bindtables else ()
        if func is None:
            funcs = _bindtables[tag].seqs.get(sequence) if tag in _bindtables else None
            return " ".join(getattr(f, "__name__", "?") for f in funcs) if funcs else ""
        funcid = _bindtables[tag].bind(sequence, func, add)
        if not tag.startswith("."):
            _class_tags_bound.add(tag)
        _events_changed(self._root(), tag)
        return funcid

    def bind(self, sequence=None, func=None, add=None):
        """Bind FUNC to SEQUENCE on this widget ('<Button-1>', '<Key>' ...)."""
        return self._bind(self._w, sequence, func, add)

    def unbind(self, sequence, funcid=None):
        if self._w in _bindtables:
            _bindtables[self._w].unbind(sequence, funcid)

    def bind_all(self, sequence=None, func=None, add=None):
        return self._bind("all", sequence, func, add)

    def unbind_all(self, sequence):
        if "all" in _bindtables:
            _bindtables["all"].unbind(sequence)

    def bind_class(self, className, sequence=None, func=None, add=None):
        return self._bind(className, sequence, func, add)

    def unbind_class(self, className, sequence):
        if className in _bindtables:
            _bindtables[className].unbind(sequence)

    def bindtags(self, tagList=None):
        if tagList is None:
            if self.__dict__.get("_tags") is not None:
                return self._tags
            top = self._toplevel_window()
            if top is self:
                return (self._w, self.winfo_class(), "all")
            return (self._w, self.winfo_class(), top._w, "all")
        self._tags = tuple(tagList)
        _events_changed(self._root(), None)
        return None

    def event_add(self, virtual, *sequences):
        _virtual_events.setdefault(virtual, [])
        for s in sequences:
            _parse_sequence(s)
            if s not in _virtual_events[virtual]:
                _virtual_events[virtual].append(s)
        _events_changed(self._root(), None)

    def event_delete(self, virtual, *sequences):
        if not sequences:
            _virtual_events.pop(virtual, None)
        else:
            _virtual_events[virtual] = [s for s in _virtual_events.get(virtual, ())
                                        if s not in sequences]

    def event_info(self, virtual=None):
        if virtual is None:
            return tuple(_virtual_events)
        return tuple(_virtual_events.get(virtual, ()))

    def event_generate(self, sequence, **kw):
        """Generate an event: virtual ('<<Name>>') or physical ('<Button-1>')."""
        etype, detail, mask, count = _parse_sequence(sequence)
        if etype == "Virtual":
            ev = Event(type=EventType.VirtualEvent, widget=self, _virtual=sequence, **kw)
        else:
            fields = dict(type=EventType[etype], widget=self, state=mask | kw.pop("state", 0),
                          _count=count)
            if etype in ("ButtonPress", "ButtonRelease"):
                fields["num"] = int(detail or 1)
            elif etype in ("KeyPress", "KeyRelease"):
                fields["keysym"] = detail or kw.get("keysym", "")
                fields["char"] = detail if detail and len(detail) == 1 else ""
            fields.update(kw)
            ev = Event(**fields)
            if etype in ("KeyPress", "KeyRelease"):  # Tk sends keys to the focus widget
                target = self._toplevel_window()._focus
                if target is None:
                    return
                ev.widget = target
                _dispatch(target, ev)
                return
        _dispatch(self, ev)

    # -- focus -----------------------------------------------------------------------
    def focus_set(self):
        """Direct keyboard input to this widget."""
        top = self._toplevel_window()
        old = top._focus
        top._focus = self
        node = getattr(self, "_node", None)
        if node is not None and hasattr(self, "_focus_node"):
            top._ctx.focus_node(self._focus_node())
        if old is not self:
            if old is not None and old.winfo_exists():
                old.event_generate("<FocusOut>")
            self.event_generate("<FocusIn>")

    focus = focus_set

    def focus_force(self):
        self._toplevel_window()._window.wm("focus")
        self.focus_set()

    def focus_get(self):
        for top in _windows():
            if top._focus is not None and top._focus.winfo_exists():
                return top._focus
        return None

    def focus_displayof(self):
        return self._toplevel_window()._focus

    def focus_lastfor(self):
        top = self._toplevel_window()
        return top._focus or top

    def _focus_order(self):
        top = self._toplevel_window()
        return [w for w in top._descendants() if w.cget("takefocus") not in (0, "0", False) and
                getattr(w, "_TAKEFOCUS", False) and w.winfo_ismapped()]

    def tk_focusNext(self):
        order = self._focus_order()
        if not order:
            return None
        return order[(order.index(self) + 1) % len(order)] if self in order else order[0]

    def tk_focusPrev(self):
        order = self._focus_order()
        if not order:
            return None
        return order[(order.index(self) - 1) % len(order)] if self in order else order[-1]

    def tk_focusFollowsMouse(self):
        pass  # the kit routes keys to the field being edited (Blender)

    # -- grabs (modal windows disable their siblings) --------------------------------------
    def grab_set(self):
        top = self._toplevel_window()
        for w in _windows():
            if w is not top:
                w._window.wm("disabled", 1)
        _grab["window"] = top
        _grab["widget"] = self

    grab_set_global = grab_set

    def grab_release(self):
        if _grab.get("widget") is self or _grab.get("window") is self._toplevel_window():
            for w in _windows():
                w._window.wm("disabled", 0)
            _grab.clear()

    def grab_current(self):
        return _grab.get("widget")

    def grab_status(self):
        return "local" if _grab.get("widget") is self else None

    # -- clipboard & selection -----------------------------------------------------------------
    def clipboard_clear(self, **kw):
        self._toplevel_window()._ctx.clipboard_set("")

    def clipboard_append(self, string, **kw):
        ctx = self._toplevel_window()._ctx
        ctx.clipboard_set(ctx.clipboard_get() + str(string))

    def clipboard_get(self, **kw):
        text = self._toplevel_window()._ctx.clipboard_get()
        if not text:
            raise TclError("CLIPBOARD selection doesn't exist or form \"STRING\" not defined")
        return text

    def selection_own(self, **kw):
        _selection[kw.get("selection", "PRIMARY")] = self

    def selection_own_get(self, **kw):
        owner = _selection.get(kw.get("selection", "PRIMARY"))
        if owner is None:
            raise TclError("selection isn't owned")
        return owner

    def selection_clear(self, **kw):
        _selection[kw.get("selection", "PRIMARY")] = None

    def selection_get(self, **kw):
        sel = kw.get("selection", "PRIMARY")
        if sel == "CLIPBOARD":
            return self.clipboard_get()
        owner = _selection.get(sel)
        text = owner._selected_text() if owner is not None and hasattr(owner, "_selected_text") \
            else None
        if not text:
            raise TclError("PRIMARY selection doesn't exist or form \"STRING\" not defined")
        return text

    def selection_handle(self, command, **kw):
        self._selection_handler = command

    # -- variables / commands / interpreter ------------------------------------------------------
    def getvar(self, name="PY_VAR"):
        try:
            return _variables[str(name)]._raw()
        except KeyError:
            raise TclError("can't read \"%s\": no such variable" % name) from None

    def setvar(self, name="PY_VAR", value="1"):
        var = _variables.get(str(name))
        if var is None:
            var = StringVar(name=str(name))
        var.set(value)

    getint = staticmethod(getint)
    getdouble = staticmethod(getdouble)

    def getboolean(self, s):
        return getboolean(s)

    def register(self, func, subst=None, needcleanup=1):
        name = "%d%s" % (id(func), getattr(func, "__name__", "cmd"))
        _commands[name] = func
        return name

    _register = register

    def deletecommand(self, name):
        _commands.pop(name, None)

    def info_patchlevel(self):
        return _VersionInfo(8, 6, 13, "final", 0)

    def image_names(self):
        from ._images import image_names
        return image_names()

    def image_types(self):
        return ("photo", "bitmap")

    # -- option database (kept; the look is Blender's theme) -----------------------------------------
    def option_add(self, pattern, value, priority=None):
        _option_db.append((str(pattern), value))

    def option_clear(self):
        _option_db.clear()

    def option_get(self, name, className):
        for pattern, value in reversed(_option_db):
            last = pattern.split(".")[-1].split("*")[-1]
            if last in (name, className):
                return value
        return ""

    def option_readfile(self, fileName, priority=None):
        with open(fileName, encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if line and not line.startswith(("!", "#")) and ":" in line:
                    pattern, _, value = line.partition(":")
                    self.option_add(pattern.strip(), value.strip())

    # -- palette & misc (the theme is Blender's) ---------------------------------------------------
    def tk_setPalette(self, *args, **kw):
        pass

    def tk_bisque(self):
        pass

    def tk_strictMotif(self, boolean=None):
        return 0

    def bell(self, displayof=0):
        if sys.platform == "win32":
            import ctypes
            ctypes.windll.user32.MessageBeep(0)
        else:
            sys.stdout.write("\a")
            sys.stdout.flush()

    def send(self, interp, cmd, *args):
        raise TclError('invalid command name "send"')  # as on Windows Tk

    # -- busy (Tk 8.7) --------------------------------------------------------------------------
    def tk_busy_hold(self, **kw):
        self._busy = True
        if getattr(self, "_node", None) is not None:
            self._node.set("cursor", 0)
            self._node.set("enabled", False)

    busy_hold = busy = tk_busy = tk_busy_hold

    def tk_busy_forget(self):
        self._busy = False
        if getattr(self, "_node", None) is not None:
            self._node.set("enabled", True)
            self._node.set("cursor", -1)

    busy_forget = tk_busy_forget

    def tk_busy_status(self):
        return bool(getattr(self, "_busy", False))

    busy_status = tk_busy_status

    def tk_busy_current(self, pattern=None):
        return [w for w in self._root()._descendants() if getattr(w, "_busy", False)]

    busy_current = tk_busy_current

    def tk_busy_cget(self, option):
        return "watch" if option.lstrip("-") == "cursor" else ""

    busy_cget = tk_busy_cget

    def tk_busy_configure(self, cnf=None, **kw):
        return {}

    tk_busy_config = busy_configure = busy_config = tk_busy_configure

    # -- stacking -----------------------------------------------------------------------------
    def tkraise(self, aboveThis=None):
        """Raise this widget in the stacking order (place-managed siblings)."""
        node = getattr(self, "_node", None)
        if node is not None and node.parent is not None and node.geom_get("manager") == _PLACE:
            node.parent.add(node)

    lift = tkraise

    def lower(self, belowThis=None):
        node = getattr(self, "_node", None)
        if node is not None and node.parent is not None and node.geom_get("manager") == _PLACE:
            node.parent.add(node, 0)

    # -- geometry info on containers (tkinter defines these on Misc) -------------------------------
    def _container(self):
        return self._node

    def _managed_children(self, manager):
        return [c for c in self.winfo_children() if c.winfo_manager() == manager]

    def pack_propagate(self, flag=None):
        if flag is None:
            return not self._container().get("nopropagate")
        self._container().set("nopropagate", not getboolean(flag))
        self._size_changed()
        return None

    propagate = pack_propagate

    def pack_slaves(self):
        return self._managed_children("pack")

    slaves = pack_slaves

    def place_slaves(self):
        return self._managed_children("place")

    def grid_propagate(self, flag=None):
        return self.pack_propagate(flag)

    def grid_slaves(self, row=None, column=None):
        out = []
        for c in reversed(self._managed_children("grid")):
            info = c._ginfo
            r0, c0 = int(info.get("row", 0)), int(info.get("column", 0))
            rs, cs = int(info.get("rowspan", 1)), int(info.get("columnspan", 1))
            if (row is None or r0 <= int(row) < r0 + rs) and \
                    (column is None or c0 <= int(column) < c0 + cs):
                out.append(c)
        return out

    def grid_size(self):
        cols = rows = 0
        for c in self._managed_children("grid"):
            info = c._ginfo
            cols = max(cols, int(info.get("column", 0)) + int(info.get("columnspan", 1)))
            rows = max(rows, int(info.get("row", 0)) + int(info.get("rowspan", 1)))
        return (cols, rows)

    size = grid_size

    def _grid_cells(self):
        """Measured column/row edges from the gridded children."""
        x0, y0 = self._container().rect[:2]
        cols, rows = {}, {}
        for c in self._managed_children("grid"):
            info, r = c._ginfo, c._node.rect
            ci, ri = int(info.get("column", 0)), int(info.get("row", 0))
            cols.setdefault(ci, [r[0] - x0, r[0] - x0 + r[2]])
            rows.setdefault(ri, [r[1] - y0, r[1] - y0 + r[3]])
            cols[ci] = [min(cols[ci][0], r[0] - x0), max(cols[ci][1], r[0] - x0 + r[2])]
            rows[ri] = [min(rows[ri][0], r[1] - y0), max(rows[ri][1], r[1] - y0 + r[3])]
        return cols, rows

    def grid_bbox(self, column=None, row=None, col2=None, row2=None):
        cols, rows = self._grid_cells()
        if not cols:
            return None
        c0 = min(cols) if column is None else int(column)
        r0 = min(rows) if row is None else int(row)
        c1 = (max(cols) if column is None else c0) if col2 is None else int(col2)
        r1 = (max(rows) if row is None else r0) if row2 is None else int(row2)
        xs = [cols[c] for c in cols if c0 <= c <= c1]
        ys = [rows[r] for r in rows if r0 <= r <= r1]
        if not xs or not ys:
            return (0, 0, 0, 0)
        x, y = min(a for a, _ in xs), min(a for a, _ in ys)
        return (x, y, max(b for _, b in xs) - x, max(b for _, b in ys) - y)

    bbox = grid_bbox

    def grid_location(self, x, y):
        cols, rows = self._grid_cells()

        def find(edges, v):
            for i in sorted(edges):
                if v < edges[i][1]:
                    return i
            return max(edges) + 1 if edges else 0
        return (find(cols, int(x)) if int(x) >= 0 else -1,
                find(rows, int(y)) if int(y) >= 0 else -1)

    def grid_anchor(self, anchor=None):
        if anchor is None:
            return getattr(self, "_grid_anchor", "nw")
        self._grid_anchor = anchor  # the kit grid is anchored nw

    anchor = grid_anchor

    def grid_columnconfigure(self, index, cnf=None, **kw):
        return self._grid_weight(1, index, cnf, kw)

    columnconfigure = grid_columnconfigure

    def grid_rowconfigure(self, index, cnf=None, **kw):
        return self._grid_weight(0, index, cnf, kw)

    rowconfigure = grid_rowconfigure

    def _grid_weight(self, axis, index, cnf, kw):
        if cnf:
            kw = dict(cnf, **kw)
        store = self.__dict__.setdefault("_grid_conf", {})
        indices = index if isinstance(index, (tuple, list)) else (index,)
        if not kw:
            return dict(store.get((axis, int(indices[0]) if str(indices[0]).isdigit()
                                   else indices[0]), {"weight": 0, "minsize": 0, "pad": 0,
                                                       "uniform": None}))
        for i in indices:
            if isinstance(i, BaseWidget):
                info = i._ginfo
                i = int(info.get("column" if axis else "row", 0))
            conf = store.setdefault((axis, int(i)), {"weight": 0, "minsize": 0, "pad": 0,
                                                      "uniform": None})
            conf.update(kw)
            self._container().grid_weight(axis, int(i), float(conf["weight"]),
                                          int(_pixels(conf["minsize"])))
        return None

    # -- window information ---------------------------------------------------------------
    def _rect(self):
        node = getattr(self, "_node", None)
        return node.rect if node is not None else (0, 0, 0, 0)

    def winfo_width(self):
        return max(1, self._rect()[2])

    def winfo_height(self):
        return max(1, self._rect()[3])

    def winfo_reqwidth(self):
        return self._node.natural()[0]

    def winfo_reqheight(self):
        return self._node.natural()[1]

    def winfo_x(self):
        parent = self.master
        px = parent._rect()[0] if parent is not None and not getattr(self, "_is_window", 0) else 0
        return self._rect()[0] - px

    def winfo_y(self):
        parent = self.master
        py = parent._rect()[1] if parent is not None and not getattr(self, "_is_window", 0) else 0
        return self._rect()[1] - py

    def winfo_rootx(self):
        pos = self._toplevel_window()._window.wm_get("pos") or (0, 0)
        return int(pos[0]) + self._rect()[0]

    def winfo_rooty(self):
        pos = self._toplevel_window()._window.wm_get("pos") or (0, 0)
        return int(pos[1]) + self._rect()[1]

    def winfo_geometry(self):
        return "%dx%d+%d+%d" % (self.winfo_width(), self.winfo_height(), self.winfo_x(),
                                self.winfo_y())

    def winfo_ismapped(self):
        return 1 if self.winfo_exists() and self._rect()[2] > 0 and self._shown() else 0

    def _shown(self):
        node = getattr(self, "_node", None)
        while node is not None:
            if not node.get("visible"):
                return False
            node = node.parent
        return True

    winfo_viewable = winfo_ismapped

    def winfo_exists(self):
        return 0 if getattr(self, "_destroyed", False) else 1

    def winfo_manager(self):
        return getattr(self, "_ginfo", {}).get("manager", "")

    def winfo_children(self):
        return list(self.children.values())

    def winfo_class(self):
        return getattr(self, "_class", None) or type(self).__name__

    def winfo_name(self):
        return self._name

    def winfo_parent(self):
        return self.master._w if self.master is not None else ""

    def winfo_pathname(self, id, displayof=0):  # noqa: A002
        for w in self._root()._descendants():
            if w.winfo_id() == id:
                return w._w
        raise TclError('window id "%s" doesn\'t exist in this application' % id)

    def winfo_id(self):
        if getattr(self, "_is_window", False):
            return int(self._window.handle or id(self))
        return id(self._node)

    def winfo_containing(self, rootX, rootY, displayof=0):
        best = None
        for w in [self._root()] + self._root()._descendants():
            if not w.winfo_ismapped():
                continue
            x, y = w.winfo_rootx(), w.winfo_rooty()
            if x <= rootX < x + w.winfo_width() and y <= rootY < y + w.winfo_height():
                best = w  # descendants come after their parents
        return best

    def _screen(self):
        top = self._toplevel_window()
        return top._window.wm_get("screen") or (1920, 1080)

    def winfo_screenwidth(self):
        return int(self._screen()[0])

    def winfo_screenheight(self):
        return int(self._screen()[1])

    def winfo_screenmmwidth(self):
        return int(self.winfo_screenwidth() * 25.4 / 96.0)

    def winfo_screenmmheight(self):
        return int(self.winfo_screenheight() * 25.4 / 96.0)

    def winfo_vrootwidth(self):
        return self.winfo_screenwidth()

    def winfo_vrootheight(self):
        return self.winfo_screenheight()

    def winfo_vrootx(self):
        return 0

    winfo_vrooty = winfo_vrootx

    def winfo_pointerxy(self):
        p = self._toplevel_window()._window.wm_get("pointer") or (0, 0)
        return (int(p[0]), int(p[1]))

    def winfo_pointerx(self):
        return self.winfo_pointerxy()[0]

    def winfo_pointery(self):
        return self.winfo_pointerxy()[1]

    def winfo_rgb(self, color_):
        c = color(color_)
        return tuple(int(round(v * 65535)) for v in c[:3])

    def winfo_fpixels(self, number):
        return _pixels(number)

    def winfo_pixels(self, number):
        return int(round(_pixels(number)))

    def winfo_depth(self):
        return 24

    winfo_screendepth = winfo_depth

    def winfo_cells(self):
        return 256

    winfo_screencells = winfo_cells

    def winfo_visual(self):
        return "truecolor"

    winfo_screenvisual = winfo_visual

    def winfo_visualid(self):
        return "0x21"

    def winfo_visualsavailable(self, includeids=False):
        return [("truecolor", 24)]

    def winfo_colormapfull(self):
        return 0

    def winfo_screen(self):
        return ":0"

    def winfo_server(self):
        return "BUI kit %s" % sys.platform

    def winfo_interps(self, displayof=0):
        return ()

    _atoms: dict = {}

    def winfo_atom(self, name, displayof=0):
        return Misc._atoms.setdefault(name, len(Misc._atoms) + 1)

    def winfo_atomname(self, id, displayof=0):  # noqa: A002
        for name, value in Misc._atoms.items():
            if value == id:
                return name
        raise TclError('no atom exists with id "%s"' % id)

    # -- tkinter internals some programs call ------------------------------------------------
    def _descendants(self):
        out = []
        for c in self.children.values():
            out.append(c)
            out.extend(c._descendants())
        return out

    def _size_changed(self):
        pass

    def _grid_next_row(self):
        rows = [int(c._ginfo.get("row", 0)) + int(c._ginfo.get("rowspan", 1))
                for c in self.winfo_children() if c.winfo_manager() == "grid"]
        return max(rows, default=0)

    def keys(self):
        return sorted(getattr(self, "_opts", {}))

    def __repr__(self):
        return "<%s.%s object %s>" % (self.__class__.__module__, self.__class__.__qualname__,
                                     self._w)


_VersionInfo = collections.namedtuple("_VersionInfoType",
                                      "major minor micro releaselevel serial")
_grab: dict = {}


def _wait_until(cond):
    _show_windows()
    while not cond() and host._windows:
        host.update()
        time.sleep(0.004)


# ==========================================================================
# Geometry managers (tkinter.Pack / Place / Grid)


def _gm_kw(cnf, kw):
    kw = dict(cnf, **kw) if cnf else dict(kw)
    return {k[:-1] if k in ("in_",) else k: v for k, v in kw.items()}


class Pack:
    """Tk's packer (runs in C: kit/intern/kit_frame.c)."""

    def pack_configure(self, cnf={}, **kw):  # noqa: B006 - tkinter signature
        kw = _gm_kw(cnf, kw)
        info = self._ginfo if self.winfo_manager() == "pack" else \
            {"side": "top", "fill": "none", "expand": 0, "anchor": "center", "padx": 0,
             "pady": 0, "ipadx": 0, "ipady": 0}
        info = dict(info, **kw)
        info.pop("before", None)
        info.pop("after", None)
        target = kw.get("in") or kw.get("before", None) and kw["before"].master or \
            kw.get("after", None) and kw["after"].master or self._gm_parent()
        info["in"] = target
        self._manage(target, "pack", info)
        px, py = _pads(info["padx"]), _pads(info["pady"])
        self._node.geom(_PACK, side={"top": 0, "bottom": 1, "left": 2, "right": 3}[
            str(info["side"])], fill={"none": 0, "x": 1, "y": 2, "both": 3}[
            str(info["fill"] or "none")], expand=1 if getboolean(info["expand"]) else 0,
            anchor=_anchor_bits(info["anchor"]), padx0=px[0], padx1=px[1], pady0=py[0],
            pady1=py[1], ipadx=int(_pixels(info["ipadx"])), ipady=int(_pixels(info["ipady"])))
        ref = kw.get("before") or kw.get("after")
        if ref is not None:
            parent = self._node.parent
            sibs = parent.children
            sibs.remove(self._node)
            i = sibs.index(ref._node) + (1 if kw.get("after") is not None else 0)
            parent.add(self._node, i)

    pack = configure = config = pack_configure

    def pack_forget(self):
        if self.winfo_manager() == "pack":
            self._unmanage()

    forget = pack_forget

    def pack_info(self):
        if self.winfo_manager() != "pack":
            raise TclError('window "%s" isn\'t packed' % self._w)
        return {k: v for k, v in self._ginfo.items() if k != "manager"}

    info = pack_info
    propagate = Misc.pack_propagate
    slaves = Misc.pack_slaves


class Place:
    """Tk's placer."""

    def place_configure(self, cnf={}, **kw):  # noqa: B006
        kw = _gm_kw(cnf, kw)
        info = dict(self._ginfo if self.winfo_manager() == "place" else
                    {"x": 0, "y": 0, "anchor": "nw", "bordermode": "inside"}, **kw)
        target = kw.get("in") or self._gm_parent()
        info["in"] = target
        self._manage(target, "place", info)
        params = {"reset_place": 1, "anchor": _anchor_bits(info.get("anchor", "nw"))}
        for k in ("x", "y", "width", "height"):
            if info.get(k) not in (None, ""):
                params[k] = _pixels(info[k])
        for k in ("relx", "rely", "relwidth", "relheight"):
            if info.get(k) not in (None, ""):
                params[k] = float(info[k])
        self._node.geom(_PLACE, **params)

    place = place_configure

    def place_forget(self):
        if self.winfo_manager() == "place":
            self._unmanage()

    forget = place_forget

    def place_info(self):
        return {k: v for k, v in self._ginfo.items() if k != "manager"} \
            if self.winfo_manager() == "place" else {}

    info = place_info
    slaves = Misc.place_slaves


class Grid:
    """Tk's gridder."""

    def grid_configure(self, cnf={}, **kw):  # noqa: B006
        kw = _gm_kw(cnf, kw)
        info = dict(self._ginfo if self.winfo_manager() in ("grid", "grid_removed") else
                    {"row": None, "column": 0, "rowspan": 1, "columnspan": 1, "sticky": "",
                     "padx": 0, "pady": 0, "ipadx": 0, "ipady": 0}, **kw)
        target = kw.get("in") or self._gm_parent()
        if info.get("row") is None:  # next free row (Tk)
            info["row"] = target._grid_next_row()
        info["in"] = target
        self._manage(target, "grid", info)
        px, py = _pads(info["padx"]), _pads(info["pady"])
        self._node.geom(_GRID, row=int(info["row"]), column=int(info["column"]),
                        rowspan=int(info["rowspan"]), columnspan=int(info["columnspan"]),
                        sticky=_anchor_bits(info["sticky"]) if info["sticky"] else 0,
                        padx0=px[0], padx1=px[1], pady0=py[0], pady1=py[1],
                        ipadx=int(_pixels(info["ipadx"])), ipady=int(_pixels(info["ipady"])))

    grid = grid_configure

    def grid_forget(self):
        if self.winfo_manager() in ("grid", "grid_removed"):
            self._unmanage()

    forget = grid_forget

    def grid_remove(self):
        """Unmap but remember the grid options (grid() restores them)."""
        if self.winfo_manager() == "grid":
            info = dict(self._ginfo)
            self._unmanage()
            self._ginfo = dict(info, manager="grid_removed")

    def grid_info(self):
        if self.winfo_manager() != "grid":
            return {}
        return {k: v for k, v in self._ginfo.items() if k != "manager"}

    info = grid_info
    location = Misc.grid_location
    size = Misc.grid_size
    bbox = Misc.grid_bbox
    slaves = Misc.grid_slaves
    propagate = Misc.grid_propagate
    columnconfigure = Misc.grid_columnconfigure
    rowconfigure = Misc.grid_rowconfigure
    anchor = Misc.grid_anchor


# ==========================================================================
# Scrolling (tkinter.XView / YView over the kit's view protocol)


class _View:
    _AXIS = "y"

    def _view(self, axis):
        """(first, last) visible fractions along 'x' or 'y'."""
        return (float(self._node.get(axis + "first")), float(self._node.get(axis + "last")))

    def _view_set(self, axis, op, args):
        node = self._node
        if op == "moveto":
            node.set(axis + "view", float(args[0]))
        elif op == "scroll":
            n, what = int(float(args[0])), str(args[1])
            if what.startswith("page"):
                n *= max(1, int(node.get(axis + "visible")) - 1)
            node.set(axis + "scroll", float(n))
        else:
            raise TclError('bad option "%s": must be moveto or scroll' % op)
        self._view_changed()

    def _view_cmd(self, axis, args):
        if not args:
            return self._view(axis)
        if isinstance(args[0], (int, float)) or str(args[0]).replace(".", "", 1).isdigit():
            return self._view_set(axis, "moveto", args)
        return self._view_set(axis, str(args[0]), args[1:])

    def _view_changed(self):
        """Tell x/yscrollcommand about the current view (Tk sends strings)."""
        for axis in ("x", "y"):
            cmd = self._opts.get(axis + "scrollcommand")
            if cmd:
                first, last = self._view(axis)
                _call(self, _command(cmd), repr(first), repr(last))


class XView(_View):
    def xview(self, *args):
        return self._view_cmd("x", args)

    def xview_moveto(self, fraction):
        self._view_set("x", "moveto", (fraction,))

    def xview_scroll(self, number, what):
        self._view_set("x", "scroll", (number, what))


class YView(_View):
    def yview(self, *args):
        return self._view_cmd("y", args)

    def yview_moveto(self, fraction):
        self._view_set("y", "moveto", (fraction,))

    def yview_scroll(self, number, what):
        self._view_set("y", "scroll", (number, what))


def _command(cmd):
    """A widget command option: a callable or a name from register()."""
    if callable(cmd):
        return cmd
    name = cmd[0] if isinstance(cmd, (tuple, list)) else str(cmd).split()[0]
    return _commands.get(name, lambda *a: None)


# ==========================================================================
# Widgets


class _Node(buipy.KitNode):
    """The kit node of a widget; forwards kit events to it."""

    def __init__(self, kind, widget):
        super().__init__(kind)
        self.widget = widget

    def _on_event(self, kind, ref, a, b):
        self.widget._on_node_event(kind, ref, a, b)


_RAW_KINDS = (EV_PRESS, EV_RELEASE, EV_DOUBLE, EV_MOTION, EV_ENTER, EV_LEAVE, EV_KEY,
              EV_WHEEL, EV_KEYRELEASE)
_CHAR_FOR_KEYSYM = {"Return": "\r", "KP_Enter": "\r", "BackSpace": "\b", "Tab": "\t",
                    "Escape": "\x1b", "Delete": "\x7f", "space": " "}


class BaseWidget(Misc):
    """Internal base of widgets: names, options, the kit node."""

    _KIND: Optional[str] = "frame"  # kit node class (None: no node, e.g. Menu)
    _DEFAULTS: dict = {}            # cget() values of unset options
    _TAKEFOCUS = False

    def __init__(self, master=None, widgetName=None, cnf={}, kw={}, extra=()):  # noqa: B006
        kw = _gm_kw(cnf, kw)
        if master is None:
            master = _get_default_root()
        self.master = master
        self.children = {}
        self._opts = {}
        self._ginfo = {}
        self._destroyed = False
        self._class = kw.pop("class", None)
        name = kw.pop("name", None)
        if name is None:
            base = "!" + type(self).__name__.lower()
            counts = master.__dict__.setdefault("_last_child_ids", {})
            counts[base] = counts.get(base, 0) + 1
            name = base if counts[base] == 1 else "%s%d" % (base, counts[base])
        self._name = str(name)
        self._w = ("." if master._w == "." else master._w + ".") + self._name
        old = master.children.get(self._name)
        if old is not None:
            old.destroy()
        master.children[self._name] = self
        self._node = _Node(self._KIND, self) if self._KIND else None
        self._setup()
        if self._node is not None:
            parent = master._container_for(self)
            if parent is not None:
                parent.add(self._node)
            if _needs_events(self):
                self._node.set("events", True)
        self.configure(**kw)

    def _setup(self):
        """Class-specific node setup before options apply."""

    def _container_for(self, child):
        """Kit node that receives `child`'s node (None: added later)."""
        return self._node

    def _gm_parent(self):
        return self.master

    # -- geometry management (shared by Pack / Place / Grid) ----------------------------------
    def _manage(self, target, manager, info):
        desired = target._container_for(self) if target is self.master else target._container()
        if desired is not None and self._node.parent is not desired:
            desired.add(self._node)
        old = self._ginfo.get("in")
        self._ginfo = dict(info, manager=manager)
        target._size_changed()
        if old is not None and old is not target:
            old._size_changed()

    def _unmanage(self):
        target = self._ginfo.get("in") or self.master
        self._ginfo = {}
        self._node.geom(0)
        target._size_changed()

    # -- options --------------------------------------------------------------------------------
    def configure(self, cnf=None, **kw):
        """Configure resources of a widget (tkinter.Misc.configure)."""
        kw = _gm_kw(cnf, kw)
        if not kw:
            if cnf is not None and isinstance(cnf, str):
                return self._option_tuple(cnf)
            keys = sorted(set(self._DEFAULTS) | set(self._opts))
            return {k: self._option_tuple(k) for k in keys}
        for k, v in kw.items():
            k = _ALIASES.get(k, k)
            self._opts[k] = v
            self._apply(k, v)
        return None

    config = configure

    def _option_tuple(self, key):
        key = _ALIASES.get(key.lstrip("-"), key.lstrip("-"))
        return (key, key, key.capitalize(), self._DEFAULTS.get(key, ""), self.cget(key))

    def cget(self, key):
        """Return the resource value for a KEY given as string."""
        key = _ALIASES.get(key.lstrip("-"), key.lstrip("-"))
        if key == "text" and self._opts.get("textvariable") not in (None, ""):
            try:  # Tk: -text shows the variable's value
                return str(_var_value(self._opts["textvariable"]))
            except KeyError:
                pass
        if key in self._opts:
            return self._opts[key]
        if key in self._DEFAULTS:
            return self._DEFAULTS[key]
        return ""

    def __getitem__(self, key):
        return self.cget(key)

    def __setitem__(self, key, value):
        self.configure({key: value})

    def keys(self):
        return sorted(set(self._DEFAULTS) | set(self._opts))

    def _apply(self, key, value):
        """Apply one option to the node. Options the Blender look has no use
        for (relief, borderwidth, highlight*, activebackground ...) are kept
        and returned by cget."""
        node = self._node
        if node is None:
            return
        if key == "state":
            node.set("enabled", str(value) not in ("disabled",))
        elif key == "font":
            node.set("font_scale", _font_scale(value))
            _set_quiet(node, "bold", _font_bold(value))
        elif key == "cursor":
            node.set("cursor", float(_TK_CURSORS.get(str(value), 0)))
        elif key in ("bg", "fg") and value not in (None, ""):
            c = color(value)
            if key == "bg" and self._KIND in ("frame", "canvas"):
                node.set("bg", c)
            elif key == "fg":
                _set_quiet(node, "text_color", c)

    # -- events from the kit ------------------------------------------------------------------
    def _on_node_event(self, kind, ref, a, b):
        if kind in _RAW_KINDS:
            ev = self._make_event(kind, ref, a, b)
            self._raw_event(ev)
            _dispatch(self, ev)
            if kind == EV_WHEEL and sys.platform not in ("win32", "darwin"):
                ev4 = self._make_event(EV_PRESS, "%d %d" % (0, 4 if a > 0 else 5), 0, 0)
                _dispatch(self, ev4)
        elif kind == EV_CONFIGURE:
            first = not getattr(self, "_mapped", False)
            self._mapped = True
            x, y = self.winfo_x(), self.winfo_y()
            if first:
                _dispatch(self, Event(type=EventType.Map, widget=self))
            _dispatch(self, Event(type=EventType.Configure, widget=self, width=int(a),
                                  height=int(b), x=x, y=y))
            self._configured(int(a), int(b))
        elif kind == EV_VIEW and isinstance(self, _View):
            self._view_changed()
        else:
            self._event(kind, ref, a, b)

    def _make_event(self, kind, ref, a, b):
        if kind in (EV_KEY, EV_KEYRELEASE):
            char, _, keysym = (ref or "").partition("\t")
            char = char or _CHAR_FOR_KEYSYM.get(keysym, "")
            return Event(type=EventType.KeyPress if kind == EV_KEY else EventType.KeyRelease,
                         widget=self, char=char, keysym=keysym,
                         keysym_num=ord(char) if len(char) == 1 else 0,
                         keycode=ord(keysym.upper()) if len(keysym) == 1 else 0,
                         state=_kit_mods_to_state(int(a)), x=0, y=0)
        # "state button", then from a third click in a row on the presses' count
        parts = (ref or "").split()
        mods, num = (parts[0] if parts else "0"), (parts[1] if len(parts) > 1 else "1")
        state = _kit_mods_to_state(int(mods or 0))
        if kind == EV_WHEEL:
            return Event(type=EventType.MouseWheel, widget=self, delta=int(a) * 120,
                         state=state)
        etype = {EV_PRESS: EventType.ButtonPress, EV_DOUBLE: EventType.ButtonPress,
                 EV_RELEASE: EventType.ButtonRelease, EV_MOTION: EventType.Motion,
                 EV_ENTER: EventType.Enter, EV_LEAVE: EventType.Leave}[kind]
        x, y = int(a), int(b)
        rx, ry = self.winfo_rootx(), self.winfo_rooty()
        return Event(type=etype, widget=self, x=x, y=y, x_root=rx + x, y_root=ry + y,
                     num=int(num or 1) if etype in (EventType.ButtonPress,
                                                    EventType.ButtonRelease) else "??",
                     state=state, _count=2 if kind == EV_DOUBLE else 1)

    def _raw_event(self, ev):
        """Widget hook before bindings run (canvas items, text tags)."""

    def _event(self, kind, ref, a, b):
        """Widget events (value changed, clicked ...)."""

    def _configured(self, width, height):
        """Size known/changed (canvas scroll region, wrapping ...)."""

    # -- life cycle ----------------------------------------------------------------------------
    def destroy(self):
        """Destroy this and all descendants widgets."""
        if self._destroyed:
            return
        for c in list(self.children.values()):
            c.destroy()
        _dispatch(self, Event(type=EventType.Destroy, widget=self))
        self._destroyed = True
        if self._node is not None and self._node.parent is not None:
            self._node.parent.remove(self._node)
        if self.master is not None and self.master.children.get(self._name) is self:
            del self.master.children[self._name]
        _bindtables.pop(self._w, None)
        top = self._toplevel_window() if not getattr(self, "_is_window", False) else None
        if top is not None and top._focus is self:
            top._focus = None


class Widget(BaseWidget, Pack, Place, Grid):
    """Internal class: a widget that can be managed by pack, place or grid."""


_ALIASES = {"background": "bg", "foreground": "fg", "borderwidth": "bd",
            "from_": "from", "class_": "class", "in_": "in"}


def _set_quiet(node, key, value):
    """Set a property the node's class may not support."""
    try:
        node.set(key, value)
    except (KeyError, ValueError, TypeError):
        pass


def _needs_events(widget) -> bool:
    """Raw events are forwarded only to widgets with bindings in their tags
    (keeps hover/motion in C for everything else)."""
    if getattr(widget, "_ALWAYS_EVENTS", False):
        return True
    tags = widget.bindtags()
    return any(t in _bindtables and _bindtables[t].seqs for t in tags) or \
        bool(_virtual_events)


def _events_changed(root, tag):
    """A binding was added: switch on raw events where it can now fire."""
    if root is None:
        return
    for w in [root] + root._descendants():
        node = getattr(w, "_node", None)
        if node is not None and (tag is None or tag in w.bindtags()) and _needs_events(w):
            node.set("events", True)


# ==========================================================================
# Windows (tkinter.Wm, Tk, Toplevel)


_STATE_CODES = {"normal": 0, "iconic": 1, "withdrawn": 2, "zoomed": 3}


class Wm:
    """Window-manager methods (tkinter.Wm); short names alias wm_*."""

    def wm_aspect(self, minNumer=None, minDenom=None, maxNumer=None, maxDenom=None):
        if minNumer is None:
            return self._wm_store.get("aspect")
        self._wm_store["aspect"] = (minNumer, minDenom, maxNumer, maxDenom)

    aspect = wm_aspect

    def wm_attributes(self, *args, return_python_dict=False, **kw):
        """'-topmost', '-alpha', '-fullscreen', '-disabled', '-toolwindow' ..."""
        attrs = self._wm_store.setdefault("attributes", {"-alpha": 1.0, "-topmost": 0,
                                                         "-fullscreen": 0, "-disabled": 0,
                                                         "-toolwindow": 0,
                                                         "-transparentcolor": ""})
        for k, v in kw.items():
            args += ("-" + k, v)
        if not args:
            return dict(attrs) if return_python_dict else \
                tuple(itertools.chain.from_iterable(attrs.items()))
        if len(args) == 1:
            key = args[0] if str(args[0]).startswith("-") else "-" + str(args[0])
            return attrs.get(key, "")
        for key, value in zip(args[0::2], args[1::2]):
            key = key if str(key).startswith("-") else "-" + str(key)
            attrs[key] = value
            if key == "-topmost":
                self._window.wm("topmost", 1 if getboolean(value) else 0)
            elif key == "-alpha":
                self._window.wm("alpha", float(value))
            elif key == "-fullscreen":
                full = getboolean(value)
                self._window.wm("override", 1 if full else 0)
                self._window.wm("state", 3 if full else 0)
            elif key == "-disabled":
                self._window.wm("disabled", 1 if getboolean(value) else 0)
        return None

    attributes = wm_attributes

    def wm_client(self, name=None):
        return self._wm_get_set("client", name)

    client = wm_client

    def wm_colormapwindows(self, *wlist):
        return self._wm_get_set("colormapwindows", list(wlist) or None) or []

    colormapwindows = wm_colormapwindows

    def wm_command(self, value=None):
        return self._wm_get_set("command", value)

    command = wm_command

    def wm_deiconify(self):
        self._withdrawn = False
        self._show()
        self._window.show()
        self._window.wm("state", 0)

    deiconify = wm_deiconify

    def wm_focusmodel(self, model=None):
        return self._wm_get_set("focusmodel", model) or "passive"

    focusmodel = wm_focusmodel

    def wm_forget(self, window):
        pass  # windows can't be turned into frames

    forget = wm_forget

    def wm_frame(self):
        return hex(self.winfo_id())

    frame = wm_frame

    def wm_geometry(self, newGeometry=None):
        """'WxH+X+Y' (any part optional); without an argument, the current."""
        if newGeometry is None:
            scale = buipy.kit_scale()
            w, h = self._window.size
            pos = self._window.wm_get("pos") or (0, 0)
            return "%dx%d+%d+%d" % (round(w / scale), round(h / scale), pos[0], pos[1])
        m = re.match(r"^\s*(?:(\d+)x(\d+))?\s*(?:([+-]-?\d+)([+-]-?\d+))?\s*$",
                     str(newGeometry))
        if not m:
            raise TclError('bad geometry specifier "%s"' % newGeometry)
        if newGeometry == "":
            self._geometry = None
            return None
        if m.group(1):
            self._geometry = (int(m.group(1)), int(m.group(2)))
            self._window.set_size(*self._geometry)
        if m.group(3):
            x, y = int(m.group(3)), int(m.group(4))
            if m.group(3).startswith("-"):
                x = self.winfo_screenwidth() - self._window.size[0] + x
            if m.group(4).startswith("-"):
                y = self.winfo_screenheight() - self._window.size[1] + y
            self._window.wm("pos", x, y)
        return None

    geometry = wm_geometry

    def wm_grid(self, baseWidth=None, baseHeight=None, widthInc=None, heightInc=None):
        return self._wm_get_set("grid", baseWidth and (baseWidth, baseHeight, widthInc,
                                                        heightInc))

    grid = wm_grid

    def wm_group(self, pathName=None):
        return self._wm_get_set("group", pathName)

    group = wm_group

    def wm_iconbitmap(self, bitmap=None, default=None):
        return self._wm_get_set("iconbitmap", bitmap or default)

    iconbitmap = wm_iconbitmap

    def wm_iconify(self):
        self._show()
        self._window.wm("state", 1)

    iconify = wm_iconify

    def wm_iconmask(self, bitmap=None):
        return self._wm_get_set("iconmask", bitmap)

    iconmask = wm_iconmask

    def wm_iconname(self, newName=None):
        return self._wm_get_set("iconname", newName) or ""

    iconname = wm_iconname

    def wm_iconphoto(self, default=False, *args):
        self._wm_store["iconphoto"] = args

    iconphoto = wm_iconphoto

    def wm_iconposition(self, x=None, y=None):
        return self._wm_get_set("iconposition", x is not None and (x, y) or None)

    iconposition = wm_iconposition

    def wm_iconwindow(self, pathName=None):
        return self._wm_get_set("iconwindow", pathName)

    iconwindow = wm_iconwindow

    def wm_manage(self, widget):
        pass

    manage = wm_manage

    def wm_maxsize(self, width=None, height=None):
        if width is None:
            return self._wm_store.get("maxsize", (self.winfo_screenwidth(),
                                                  self.winfo_screenheight()))
        self._wm_store["maxsize"] = (int(width), int(height))
        self._window.wm("maxsize", int(width), int(height))

    maxsize = wm_maxsize

    def wm_minsize(self, width=None, height=None):
        if width is None:
            return self._wm_store.get("minsize", (1, 1))
        self._wm_store["minsize"] = (int(width), int(height))
        self._window.wm("minsize", int(width), int(height))

    minsize = wm_minsize

    def wm_overrideredirect(self, boolean=None):
        if boolean is None:
            return self._wm_store.get("override", False)
        self._wm_store["override"] = getboolean(boolean)
        self._window.wm("override", 1 if self._wm_store["override"] else 0)

    overrideredirect = wm_overrideredirect

    def wm_positionfrom(self, who=None):
        return self._wm_get_set("positionfrom", who) or ""

    positionfrom = wm_positionfrom

    def wm_protocol(self, name=None, func=None):
        if name is None:
            return tuple(self._protocols)
        if func is None:
            return self._protocols.get(name, "")
        self._protocols[name] = func
        return None

    protocol = wm_protocol

    def wm_resizable(self, width=None, height=None):
        if width is None:
            return self._wm_store.get("resizable", (True, True))
        if height is None:
            height = width
        self._wm_store["resizable"] = (getboolean(width), getboolean(height))
        self._window.wm("resizable", 1 if getboolean(width) else 0,
                        1 if getboolean(height) else 0)

    resizable = wm_resizable

    def wm_sizefrom(self, who=None):
        return self._wm_get_set("sizefrom", who) or ""

    sizefrom = wm_sizefrom

    def wm_state(self, newstate=None):
        if newstate is None:
            if not self._shown_window:
                return "withdrawn"
            code = (self._window.wm_get("state") or (0, 0))[0]
            return {v: k for k, v in _STATE_CODES.items()}[int(code)]
        if newstate != "withdrawn":
            self._show()
        self._window.wm("state", _STATE_CODES[str(newstate)])

    state = wm_state

    def wm_title(self, string=None):
        if string is None:
            return self._title
        self._title = str(string)
        self._window.title(self._title)

    title = wm_title

    def wm_transient(self, master=None):
        if master is None:
            return self._wm_store.get("transient", "")
        self._wm_store["transient"] = master
        if self._wm_store.get("_placed") is None:  # centre over the master once shown
            self._wm_store["_placed"] = False

    transient = wm_transient

    def wm_withdraw(self):
        self._window.wm("state", 2)
        self._withdrawn = True

    withdraw = wm_withdraw

    def _wm_get_set(self, key, value):
        if value is None:
            return self._wm_store.get(key)
        self._wm_store[key] = value
        return None


class _WindowHost:
    """A top-level window: kit context + host window (Tk and Toplevel)."""

    _is_window = True

    def _init_window(self, class_name):
        self._focus = None
        self._afters = {}
        self._protocols = {}
        self._wm_store = {}
        self._title = class_name.lower() if class_name else "tk"
        self._geometry = None
        self._shown_window = False
        self._withdrawn = False
        self._menubar = None
        self._ctx = buipy.KitContext(self._node)
        self._window = host.Window(self._ctx, self._title, (200, 200),
                                   gpu=getattr(self, "_gpu", None))
        self._window.on_close = self._on_close_button
        _window_list.append(self)

    def _on_close_button(self):
        handler = self._protocols.get("WM_DELETE_WINDOW")
        if handler:
            _call(self, handler)
            return False  # the handler decides (it calls destroy())
        self.destroy()
        return False

    def _fit_size(self):
        """Logical size fitting the content (+ menu bar). natural() measures raster
        pixels, which include the UI scale; set_size and geometry scale again."""
        w, h = self._node.natural()
        if self._menubar is not None and self._menubar._bar is not None:
            bw, bh = self._menubar._bar.natural()
            w, h = max(w, bw), h + bh
        scale = buipy.kit_scale()
        return max(1, math.ceil(w / scale)), max(1, math.ceil(h / scale))

    def _show(self):
        if self._shown_window or self._destroyed:
            return
        self._shown_window = True
        if self._geometry is None:  # Tk: the window fits its content
            self._window.set_size(*self._fit_size())
        master = self._wm_store.get("transient")
        if master and self._wm_store.get("_placed") is False:
            self._wm_store["_placed"] = True
            mw, mh = master._window.size
            w, h = self._window.size
            self._window.wm("pos", master.winfo_rootx() + (mw - w) // 2,
                            master.winfo_rooty() + (mh - h) // 2)
        if not self._withdrawn:
            self._window.show()

    def _set_menu(self, menu):
        self._menubar = menu
        if menu is None:
            self._ctx.set_header(None)
        else:
            self._ctx.set_header(menu._bar_node())

    def _close_window(self):
        if self in _window_list:
            _window_list.remove(self)
        for name, (win, token, _) in list(self._afters.items()) if self is self._root() else ():
            win.after_cancel(token)
        self._window.close()

    def report_callback_exception(self, exc, val, tb):
        """Report callback exception on sys.stderr (tkinter's default)."""
        sys.stderr.write("Exception in Tkinter callback\n")
        traceback.print_exception(exc, val, tb)


_window_list: list = []


def _windows():
    return [w for w in _window_list if not w._destroyed]


def _show_windows():
    for w in _windows():
        w._show()


class _TkApp:
    """``root.tk``: the few Tcl commands programs commonly call directly."""

    def __init__(self, root):
        self._root = root

    def call(self, *args):
        args = tuple(str(a) for a in (args[0] if len(args) == 1 and
                                      isinstance(args[0], (tuple, list)) else args))
        if args[:2] == ("tk", "scaling"):
            return buipy.kit_scale() * 1.333 if len(args) == 2 else None
        if args[:2] == ("info", "patchlevel"):
            return "8.6.13"
        if args[:1] == ("wm",) and len(args) >= 3:
            w = self._root.nametowidget(args[2])
            return getattr(w, "wm_" + args[1])(*args[3:])
        if args[:2] == ("tk", "windowingsystem"):
            return {"win32": "win32", "darwin": "aqua"}.get(sys.platform, "x11")
        if args[:1] == ("set",) and len(args) >= 2:
            if len(args) == 2:
                return self._root.getvar(args[1])
            self._root.setvar(args[1], args[2])
            return args[2]
        raise TclError('invalid command name "%s"' % args[0])

    def eval(self, script):
        return self.call(*script.split())

    def splitlist(self, value):
        return tuple(value) if isinstance(value, (tuple, list)) else tuple(str(value).split())

    split = splitlist

    def getboolean(self, s):
        return getboolean(s)

    def getint(self, s):
        return getint(s)

    def getdouble(self, s):
        return getdouble(s)

    def globalgetvar(self, name):
        return self._root.getvar(name)

    getvar = globalgetvar

    def globalsetvar(self, name, value):
        self._root.setvar(name, value)

    setvar = globalsetvar

    def mainloop(self, n=0):
        self._root.mainloop(n)

    def dooneevent(self, flags=0):
        host.update()
        return 1

    def createcommand(self, name, func):
        _commands[name] = func

    def deletecommand(self, name):
        _commands.pop(name, None)

    def wantobjects(self, *a):
        return 1

    def willdispatch(self):
        pass

    def interpaddr(self):
        return id(self)


_ROOT_BG = (0.188, 0.188, 0.188, 1.0)  # Blender's editor background (#303030)


def _root_bg():
    """The window background: the theme's region color (bui.set_theme / BUI_THEME)."""
    if buipy.kit_theme_name() == "dark":
        return _ROOT_BG
    return buipy.kit_theme_color("region_bg")


class Tk(_WindowHost, Misc, Wm):
    """Toplevel widget of Tk which represents mostly the main window of an
    application (a native window on Windows, Blender-styled)."""

    _w = "."
    _name = "."

    def __init__(self, screenName=None, baseName=None, className="Tk", useTk=True, sync=False,
                 use=None, gpu=None):
        """``gpu=True`` draws this window on the GPU (Direct3D 11 / OpenGL) instead of
        the software raster: same pixels, rasterization off the CPU. ``None``
        follows ``bui.set_gpu()`` / the ``BUI_GPU`` environment variable."""
        global _default_root
        self._gpu = gpu
        self.master = None
        self.children = {}
        self._destroyed = False
        self._opts = {}
        self._className = className
        self._node = _Node("frame", self)
        self._node.set("bg", _root_bg())
        self.tk = _TkApp(self)
        self._init_window(className)
        if baseName is None:  # tkinter titles the window after the script
            import os
            baseName = os.path.splitext(os.path.basename(sys.argv[0] or ""))[0] or "tk"
        self._title = baseName if className == "Tk" else className.lower()
        self._window.title(self._title)
        if _support_default_root and _default_root is None:
            _default_root = self

    def winfo_class(self):
        return self._className

    def _container_for(self, child):
        return self._node

    def _gm_parent(self):
        return self

    def configure(self, cnf=None, **kw):
        kw = _gm_kw(cnf, kw)
        if not kw:
            return {k: (k, k, k.capitalize(), "", v) for k, v in self._opts.items()}
        for k, v in kw.items():
            k = _ALIASES.get(k, k)
            self._opts[k] = v
            if k == "menu":
                self._set_menu(v)
            elif k == "bg":
                self._node.set("bg", color(v))
            elif k in ("width", "height"):
                self._node.geom(0, **{"req" + k: int(_pixels(v))})
            elif k == "cursor":
                self._node.set("cursor", float(_TK_CURSORS.get(str(v), 0)))
        return None

    config = configure

    def cget(self, key):
        return self._opts.get(_ALIASES.get(key.lstrip("-"), key.lstrip("-")), "")

    __getitem__ = cget

    def __setitem__(self, key, value):
        self.configure({key: value})

    def _on_node_event(self, kind, ref, a, b):
        BaseWidget._on_node_event(self, kind, ref, a, b)

    _make_event = BaseWidget._make_event
    _raw_event = BaseWidget._raw_event
    _configured = BaseWidget._configured
    _event = BaseWidget._event

    def destroy(self):
        """Destroy this and all descendants widgets (ends mainloop)."""
        global _default_root
        if self._destroyed:
            return
        for c in list(self.children.values()):
            c.destroy()
        _dispatch(self, Event(type=EventType.Destroy, widget=self))
        self._destroyed = True
        for w in _windows():
            if w is not self and w._root() is self:
                w.destroy()
        self._close_window()
        if _default_root is self:
            _default_root = None
        if not _windows():
            host.quit()

    def readprofile(self, baseName, className):
        pass

    def loadtk(self):
        pass

    def __getattr__(self, attr):
        """Delegate unknown attributes to the interpreter (as tkinter does)."""
        if attr.startswith("__") or "tk" not in self.__dict__:
            raise AttributeError(attr)
        return getattr(self.__dict__["tk"], attr)


class Toplevel(_WindowHost, BaseWidget, Wm):
    """Toplevel widget, e.g. for dialogs (a second native window)."""

    _KIND = "frame"

    def __init__(self, master=None, cnf={}, **kw):  # noqa: B006
        kw = _gm_kw(cnf, kw)
        menu = kw.pop("menu", None)
        BaseWidget.__init__(self, master, "toplevel", {}, {})
        self._node.set("bg", _root_bg())
        self._init_window("Toplevel")
        self._window.title(self._root().title())
        self._title = self._root().title()
        if menu is not None:
            self._set_menu(menu)
        self.configure(**kw)
        self._window.after(0, self._show)  # Tk maps new toplevels when idle

    def _container_for(self, child):
        return self._node

    def _gm_parent(self):
        return self

    def _apply(self, key, value):
        if key in ("width", "height"):
            self._node.geom(0, **{"req" + key: int(_pixels(value))})
        elif key == "menu":
            self._set_menu(value)
        else:
            super()._apply(key, value)

    def destroy(self):
        if self._destroyed:
            return
        BaseWidget.destroy(self)
        self._close_window()
        if _grab.get("window") is self:
            self.grab_release()


# ==========================================================================
# Default root

_support_default_root = True
_default_root: Optional[Tk] = None


def NoDefaultRoot():  # noqa: N802
    global _support_default_root, _default_root
    _support_default_root = False
    _default_root = None


def _get_default_root(what=None):
    global _default_root
    if not _support_default_root:
        raise RuntimeError("No master specified and tkinter is configured to not support "
                           "default root")
    if _default_root is None:
        if what:
            raise RuntimeError("Too early to %s: no default root window" % what)
        _default_root = Tk()
    return _default_root


def _get_temp_root():
    return _default_root or Tk()


def mainloop(n=0):
    """Run the main loop of Tcl."""
    _get_default_root("run the main loop").mainloop(n)


class CallWrapper:
    """Internal class: calls a function with exception reporting."""

    def __init__(self, func, subst, widget):
        self.func, self.subst, self.widget = func, subst, widget

    def __call__(self, *args):
        if self.subst:
            args = self.subst(*args)
        return _call(self.widget, self.func, *args)


class _setit:
    """Internal class: sets a variable to a value (OptionMenu commands)."""

    def __init__(self, var, value, callback=None):
        self.__value = value
        self.__var = var
        self.__callback = callback

    def __call__(self, *args):
        self.__var.set(self.__value)
        if self.__callback is not None:
            self.__callback(self.__value, *args)

#!/usr/bin/env python3
"""Generate per-theme symbolic icon sets for Flaclify.

Each icon is described once as primitives on a 16×16 grid. Each style decides
stroke weight, caps, whether closed shapes are outlined or filled, and which
typeface renders text and glyph icons. Everything is emitted as filled paths
(strokes are expanded to quads and joints), because GTK's symbolic recolouring
is only guaranteed for fills.

Outputs:
  src/iconsets/<set>/icons/scalable/actions/<name>-symbolic.svg
  the iconsets section of src/flaclify.gresource.xml (between markers)
  build/iconsheets/<set>.html   review sheet (Claude Design @dsCard)
  build/iconsheets/<set>.png    contact sheet via rsvg-convert (if available)

Run from the repo root with the build venv:  build/venv/bin/python tools/gen_icons.py
"""

import math
import os
import re
import subprocess
import sys
from pathlib import Path

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "src" / "iconsets"
SHEETS = ROOT / "build" / "iconsheets"
GRESOURCE = ROOT / "src" / "flaclify.gresource.xml"
PREFIX = "/io/github/h3303/Flaclify/iconsets"

FONTS = {
    "special-elite": ROOT / "data/fonts/SpecialElite-Regular.ttf",
    "old-standard-bold": ROOT / "data/fonts/OldStandard-Bold.ttf",
    "old-standard-italic": ROOT / "data/fonts/OldStandard-Italic.ttf",
    "fell": ROOT / "data/fonts/IMFeENsc28P.ttf",
}


def fc_match(pattern):
    return Path(subprocess.run(["fc-match", "--format=%{file}", pattern],
                               capture_output=True, text=True).stdout.strip())


FONTS["mono"] = fc_match("DejaVu Sans Mono")
FONTS["sans-bold"] = fc_match("DejaVu Sans:bold")
FONTS["plex-mono"] = fc_match("IBM Plex Mono")   # blueprint annotations; falls back to the system mono

# ---------------------------------------------------------------------------
# Styles
# ---------------------------------------------------------------------------
# w: stroke width; cap: round|square|butt; shapes: stroke|fill (for 'shape'
# primitives); dot: dot radius; text: font key; glyphs: use typographic glyph
# icons where a spec offers one.
STYLES = {
    "dttw": dict(w=1.0, cap="square", shapes="stroke", dot=0.9, text="special-elite",
                 glyphs=True, ground="#e8e5dc", ink="#141412",
                 title="Broadsheet / Reversed Spine", blurb="Typewriter glyphs where a glyph exists, hairline ruled marks elsewhere."),
    "terminal": dict(w=1.0, cap="square", shapes="stroke", dot=1.0, text="mono",
                     glyphs=False, ground="#0b0f0c", ink="#39ff88",
                     title="Terminal", blurb="One-pixel hairlines, square caps, nothing filled that can be outlined."),
    "neon": dict(w=2.0, cap="round", shapes="stroke", dot=1.5, text="sans-bold",
                 glyphs=False, ground="#150a24", ink="#ff7ac0",
                 title="Neon", blurb="Two-pixel strokes with round caps and joins."),
    "bauhaus": dict(w=2.5, cap="butt", shapes="fill", dot=1.75, text="sans-bold",
                    glyphs=False, ground="#f4f1ea", ink="#111111",
                    title="Bauhaus", blurb="Solid geometry: closed shapes fill, lines are heavy bars."),
    "folio": dict(w=1.0, cap="round", shapes="stroke", dot=1.0, text="old-standard-bold",
                  glyphs=False, ground="#ece3cc", ink="#221b0f",
                  title="Folio", blurb="Engraver's hairlines with round terminals."),
    "blueprint": dict(w=1.0, cap="butt", shapes="stroke", dot=0.8, text="plex-mono",
                      glyphs=False, ground="#10294a", ink="#d9e8f7",
                      title="Blueprint", blurb="Pen-plotter hairlines with butt caps; drafting line-work."),
    "nocturne": dict(w=1.25, cap="round", shapes="stroke", dot=1.0, text="old-standard-italic",
                     glyphs=False, ground="#1a1511", ink="#d9a441",
                     title="Nocturne", blurb="Inked hairlines with round terminals; engraved, candlelit."),
    "cassette": dict(w=1.5, cap="round", shapes="fill", dot=1.2, text="sans-bold",
                     glyphs=False, ground="#e9dfcc", ink="#291f16",
                     title="Cassette", blurb="Silk-screened control-panel pictograms: filled and rounded."),
    "marshmallow": dict(w=1.75, cap="round", shapes="fill", dot=1.5, text="sans-bold",
                        glyphs=False, ground="#fbf3f1", ink="#c2557a",
                        title="Marshmallow", blurb="Plump rounded strokes, closed shapes filled; toy-like."),
}

# ---------------------------------------------------------------------------
# Geometry helpers
# ---------------------------------------------------------------------------

def arc(cx, cy, r, a0, a1, n=12):
    """Points along an arc, angles in degrees, clockwise on screen (y down)."""
    return [(cx + r * math.cos(math.radians(a)), cy + r * math.sin(math.radians(a)))
            for a in [a0 + (a1 - a0) * i / n for i in range(n + 1)]]


def star(cx, cy, r_out, r_in, n=5, rot=-90):
    pts = []
    for i in range(2 * n):
        r = r_out if i % 2 == 0 else r_in
        a = math.radians(rot + 180 * i / n)
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    return pts


def mirror_x(pts):
    return [(16 - x, y) for x, y in pts]


def fmt(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def path_of(pts, close=True):
    d = "M" + " L".join(f"{fmt(x)},{fmt(y)}" for x, y in pts)
    return d + (" Z" if close else "")


def circle_d(cx, cy, r):
    return (f"M{fmt(cx - r)},{fmt(cy)} a{fmt(r)},{fmt(r)} 0 1,0 {fmt(2 * r)},0 "
            f"a{fmt(r)},{fmt(r)} 0 1,0 {fmt(-2 * r)},0 Z")


def seg_quad(p, q, w, extend):
    (x1, y1), (x2, y2) = p, q
    dx, dy = x2 - x1, y2 - y1
    L = math.hypot(dx, dy) or 1e-6
    ux, uy = dx / L, dy / L
    nx, ny = -uy * w / 2, ux * w / 2
    ex, ey = ux * extend, uy * extend
    return [(x1 - ex + nx, y1 - ey + ny), (x2 + ex + nx, y2 + ey + ny),
            (x2 + ex - nx, y2 + ey - ny), (x1 - ex - nx, y1 - ey - ny)]


# ---------------------------------------------------------------------------
# Renderer: primitives → list of (d, fill_rule)
# ---------------------------------------------------------------------------

class Renderer:
    def __init__(self, style):
        self.s = style
        self.paths = []
        self._fonts = {}

    def emit(self, d, rule=None):
        self.paths.append((d, rule))

    # -- strokes ------------------------------------------------------------
    def stroke(self, pts, closed=False, w=None):
        s = self.s
        w = w or s["w"]
        pts = list(pts)
        if closed:
            pts = pts + [pts[0]]
        cap = s["cap"]
        for p, q in zip(pts, pts[1:]):
            extend = w / 2 if (cap == "square" and not closed) else 0
            self.emit(path_of(seg_quad(p, q, w, extend)))
        joints = pts[1:-1] if not closed else pts[:-1]
        ends = [] if closed else ([pts[0], pts[-1]] if cap == "round" else [])
        for (x, y) in joints + ends:
            if cap == "round":
                self.emit(circle_d(x, y, w / 2))
            elif cap == "square" and len(pts) > 2:
                h = w / 2
                self.emit(path_of([(x - h, y - h), (x + h, y - h), (x + h, y + h), (x - h, y + h)]))

    def ring(self, cx, cy, r, w=None):
        w = w or self.s["w"]
        self.emit(circle_d(cx, cy, r + w / 2) + " " + circle_d(cx, cy, max(r - w / 2, 0.01)), "evenodd")

    def disc(self, cx, cy, r):
        self.emit(circle_d(cx, cy, r))

    def dot(self, cx, cy, r=None):
        r = r or self.s["dot"]
        if self.s["cap"] == "square":
            self.emit(path_of([(cx - r, cy - r), (cx + r, cy - r), (cx + r, cy + r), (cx - r, cy + r)]))
        else:
            self.disc(cx, cy, r)

    # -- text -----------------------------------------------------------------
    def font(self, key):
        if key not in self._fonts:
            f = TTFont(str(FONTS[key]))
            self._fonts[key] = (f, f.getGlyphSet(), f.getBestCmap(), f["head"].unitsPerEm, f["hmtx"])
        return self._fonts[key]

    def text(self, string, box=(1, 1, 14, 14), font=None, align="center", tracking=0.0):
        """Render a string as filled outlines, scaled to fit `box` (x, y, w, h)."""
        key = font or self.s["text"]
        f, gs, cmap, upm, hmtx = self.font(key)
        # lay out glyph outlines in font units
        pen_paths = []
        x = 0
        minx = miny = 1e9
        maxx = maxy = -1e9
        from fontTools.pens.boundsPen import BoundsPen
        for ch in string:
            gname = cmap.get(ord(ch))
            if gname is None:
                continue
            adv = hmtx[gname][0]
            bp = BoundsPen(gs)
            gs[gname].draw(bp)
            if bp.bounds:
                bx0, by0, bx1, by1 = bp.bounds
                minx, miny = min(minx, x + bx0), min(miny, by0)
                maxx, maxy = max(maxx, x + bx1), max(maxy, by1)
            pen_paths.append((gname, x))
            x += adv + tracking * upm
        if maxx < minx:
            return
        bw, bh = maxx - minx, maxy - miny
        bx, by, bw_t, bh_t = box
        scale = min(bw_t / bw, bh_t / bh)
        # centre in box (y flipped: font y up → svg y down)
        ox = bx + (bw_t - bw * scale) / 2 - minx * scale
        if align == "left":
            ox = bx - minx * scale
        oy = by + (bh_t - bh * scale) / 2 + maxy * scale
        for gname, gx in pen_paths:
            sp = SVGPathPen(gs, ntos=lambda v: fmt(v))
            tp = TransformPen(sp, (scale, 0, 0, -scale, ox + gx * scale, oy))
            gs[gname].draw(tp)
            d = sp.getCommands()
            if d:
                self.emit(d, "nonzero")

    # -- primitives dispatcher ------------------------------------------------
    def draw(self, prim):
        kind, *args = prim
        s = self.s
        if kind == "line":
            (x1, y1, x2, y2) = args[:4]
            self.stroke([(x1, y1), (x2, y2)], w=args[4] if len(args) > 4 else None)
        elif kind == "pline":
            self.stroke(args[0], closed=False, w=args[1] if len(args) > 1 else None)
        elif kind == "outline":               # closed, always stroked
            self.stroke(args[0], closed=True)
        elif kind == "shape":                 # closed, style decides
            if s["shapes"] == "fill":
                self.emit(path_of(args[0]))
            else:
                self.stroke(args[0], closed=True)
        elif kind == "solid":                 # closed, always filled
            self.emit(path_of(args[0]))
        elif kind == "ring":
            self.ring(*args)
        elif kind == "circle":                # style decides
            cx, cy, r = args
            if s["shapes"] == "fill":
                self.disc(cx, cy, r)
            else:
                self.ring(cx, cy, r)
        elif kind == "disc":
            self.disc(*args)
        elif kind == "dot":
            self.dot(*args)
        elif kind == "text":
            self.text(*args)
        else:
            raise ValueError(kind)

    def svg(self):
        body = "".join(
            f'<path d="{d}"' + (f' fill-rule="{rule}"' if rule else "") + ' fill="#222222"/>'
            for d, rule in self.paths)
        return ('<?xml version="1.0" encoding="UTF-8"?>\n'
                '<svg xmlns="http://www.w3.org/2000/svg" width="16px" height="16px" viewBox="0 0 16 16">'
                f'{body}</svg>\n')


# ---------------------------------------------------------------------------
# Icon specs
# ---------------------------------------------------------------------------
# name → dict(geo=[primitives], glyph=(string, box) optional; the glyph is used
# by styles with glyphs=True, geometry by the rest.

STRIKE = ("line", 2.5, 13.5, 13.5, 2.5)
TRI_R = [(3, 2.5), (13.5, 8), (3, 13.5)]
TRI_R_SMALL = [(4.5, 3.5), (12.5, 8), (4.5, 12.5)]
CHEV_L = [(10, 3), (5, 8), (10, 13)]
CHEV_R = mirror_x(CHEV_L)
CHEV_U = [(3, 10), (8, 5), (13, 10)]
CHEV_D = [(3, 6), (8, 11), (13, 6)]
LOOP = [(4.5, 11.5), (4.5, 4.5), (11.5, 4.5), (11.5, 11.5), (4.5, 11.5)]
OCT = [(5.5, 2), (10.5, 2), (14, 5.5), (14, 10.5), (10.5, 14), (5.5, 14), (2, 10.5), (2, 5.5)]

ICONS = {
    # transport ---------------------------------------------------------------
    "play-large": dict(geo=[("solid", TRI_R)]),
    "play": dict(geo=[("solid", TRI_R_SMALL)]),
    "pause-large": dict(geo=[("solid", [(3, 2.5), (6.5, 2.5), (6.5, 13.5), (3, 13.5)]),
                             ("solid", [(9.5, 2.5), (13, 2.5), (13, 13.5), (9.5, 13.5)])]),
    "skip-forward-large": dict(geo=[("solid", [(2.5, 3), (10.5, 8), (2.5, 13)]),
                                    ("solid", [(11.5, 3), (13.5, 3), (13.5, 13), (11.5, 13)])]),
    "skip-backward-large": dict(geo=[("solid", [(13.5, 3), (5.5, 8), (13.5, 13)]),
                                     ("solid", [(2.5, 3), (4.5, 3), (4.5, 13), (2.5, 13)])]),
    "media-playlist-shuffle": dict(geo=[("pline", [(2, 4.5), (5, 4.5), (11, 11.5), (13, 11.5)]),
                                        ("pline", [(2, 11.5), (5, 11.5), (11, 4.5), (13, 4.5)]),
                                        ("solid", [(12, 2.5), (14.5, 4.5), (12, 6.5)]),
                                        ("solid", [(12, 9.5), (14.5, 11.5), (12, 13.5)])]),
    "playlist-consecutive": dict(geo=[("line", 2, 8, 11, 8), ("solid", [(10, 5), (14, 8), (10, 11)])]),
    # A circular arrow survives every stroke weight; the boxed loop did not.
    "playlist-repeat": dict(geo=[("pline", arc(8, 8, 5.5, -60, 250, 16)),
                                 ("solid", [(10.5, 1), (14.5, 4), (10.5, 7)])]),
    "playlist-repeat-song": dict(geo=[("pline", arc(8, 8, 5.5, -60, 250, 16)),
                                      ("solid", [(10.5, 1), (14.5, 4), (10.5, 7)]),
                                      ("line", 8.25, 5.5, 8.25, 10.5), ("line", 6.75, 6.75, 8.25, 5.5)]),
    "consume-on": dict(geo=[("solid", [(8, 8)] + arc(8, 8, 5.5, 35, 325, 20))]),
    "consume-off": dict(geo=[("outline", [(8, 8)] + arc(8, 8, 5.5, 35, 325, 20))]),
    "crossfade": dict(geo=[("pline", [(2, 12), (5, 12), (11, 4), (14, 4)]),
                           ("pline", [(2, 4), (5, 4), (11, 12), (14, 12)])]),
    "crossfade-off": dict(geo=[("pline", [(2, 12), (5, 12), (11, 4), (14, 4)]),
                               ("pline", [(2, 4), (5, 4), (11, 12), (14, 12)]), STRIKE]),
    "mixramp": dict(geo=[("pline", [(2, 12.5), (8, 3.5), (14, 12.5)])]),
    "mixramp-off": dict(geo=[("pline", [(2, 12.5), (8, 3.5), (14, 12.5)]), STRIKE]),
    "rg-off": dict(geo=[("ring", 8, 8, 5.5), STRIKE]),
    "rg-track": dict(geo=[("ring", 8, 8, 5.5), ("dot", 8, 8, 1.75)]),
    "rg-album": dict(geo=[("ring", 8, 8, 5.5), ("ring", 8, 8, 2.5)]),
    "rg-auto": dict(geo=[("ring", 8, 8, 5.5), ("solid", [(8, 5), (11, 11), (5, 11)])]),
    "stop-sign-outline": dict(geo=[("outline", OCT)]),
    # chrome --------------------------------------------------------------------
    "open-menu": dict(geo=[("line", 3, 4, 13, 4), ("line", 3, 8, 13, 8), ("line", 3, 12, 13, 12)]),
    "edit-find": dict(geo=[("ring", 6.5, 6.5, 4), ("line", 9.5, 9.5, 13.5, 13.5)],
                      glyph=("?", (2, 1, 12, 14))),
    "view-sort-ascending": dict(geo=[("line", 3, 4, 7, 4), ("line", 3, 8, 10, 8), ("line", 3, 12, 13, 12)]),
    "view-sort-descending": dict(geo=[("line", 3, 4, 13, 4), ("line", 3, 8, 10, 8), ("line", 3, 12, 7, 12)]),
    "dock-left": dict(geo=[("outline", [(2, 3), (14, 3), (14, 13), (2, 13)]), ("line", 6, 3, 6, 13)]),
    "view-more": dict(geo=[("dot", 3.5, 8), ("dot", 8, 8), ("dot", 12.5, 8)], glyph=("…", (1, 1, 14, 14))),
    "cross-small": dict(geo=[("line", 4, 4, 12, 12), ("line", 12, 4, 4, 12)], glyph=("×", (2, 1, 12, 14))),
    "left": dict(geo=[("pline", CHEV_L)], glyph=("‹", (1, 0, 14, 16))),
    "right": dict(geo=[("pline", CHEV_R)], glyph=("›", (1, 0, 14, 16))),
    "up": dict(geo=[("pline", CHEV_U)]),
    "down": dict(geo=[("pline", CHEV_D)]),
    "list-add": dict(geo=[("line", 8, 3, 8, 13), ("line", 3, 8, 13, 8)], glyph=("+", (1, 1, 14, 14))),
    "plus": dict(geo=[("line", 8, 3, 8, 13), ("line", 3, 8, 13, 8)], glyph=("+", (1, 1, 14, 14))),
    "list-remove": dict(geo=[("line", 3, 8, 13, 8)], glyph=("−", (1, 1, 14, 14))),
    "user-trash": dict(geo=[("line", 2.5, 4.5, 13.5, 4.5), ("outline", [(6.5, 2), (9.5, 2), (9.5, 4.5), (6.5, 4.5)]),
                            ("shape", [(4, 4.5), (12, 4.5), (11, 14), (5, 14)])]),
    "copy": dict(geo=[("outline", [(2.5, 2.5), (10, 2.5), (10, 10), (2.5, 10)]),
                      ("outline", [(6, 6), (13.5, 6), (13.5, 13.5), (6, 13.5)])]),
    "document-edit": dict(geo=[("line", 3.5, 12.5, 11.5, 4.5, 2.5), ("solid", [(2.5, 13.5), (3, 11), (5, 13), ]),
                               ("line", 11.5, 4.5, 13, 6)]),
    "edit-select-all": dict(geo=[("outline", [(2.5, 2.5), (13.5, 2.5), (13.5, 13.5), (2.5, 13.5)]),
                                 ("pline", [(5, 8), (7.5, 10.5), (11.5, 5.5)])]),
    "edit-select-none": dict(geo=[("outline", [(2.5, 2.5), (13.5, 2.5), (13.5, 13.5), (2.5, 13.5)])]),
    "view-refresh": dict(geo=[("pline", arc(8, 8, 5, 20, 320, 16)), ("solid", [(10, 1.5), (14.5, 3.5), (11, 7)])]),
    "arrow-pointing-at-line-down": dict(geo=[("line", 3, 13.5, 13, 13.5), ("line", 8, 2, 8, 9),
                                             ("solid", [(4.5, 7.5), (8, 11.5), (11.5, 7.5)])]),
    "arrow-pointing-away-from-line-up": dict(geo=[("line", 3, 13.5, 13, 13.5), ("line", 8, 5, 8, 11),
                                                  ("solid", [(4.5, 6.5), (8, 2.5), (11.5, 6.5)])]),
    "text-insert": dict(geo=[("line", 3, 4, 13, 4), ("line", 3, 8, 9, 8), ("line", 3, 12, 13, 12), ("line", 12, 6.5, 12, 9.5)]),
    "exclamation-mark": dict(geo=[("line", 8, 2.5, 8, 10, 2), ("dot", 8, 13, 1.25)], glyph=("!", (2, 1, 12, 14))),
    "info-outline": dict(geo=[("ring", 8, 8, 6), ("line", 8, 7, 8, 11.5), ("dot", 8, 4.75, 0.9)]),
    "enabled-feature": dict(geo=[("ring", 8, 8, 6), ("pline", [(5, 8), (7.25, 10.25), (11, 6)])]),
    "disabled-feature": dict(geo=[("ring", 8, 8, 6), ("line", 5.5, 5.5, 10.5, 10.5), ("line", 10.5, 5.5, 5.5, 10.5)]),
    "circle-outline-thick": dict(geo=[("ring", 8, 8, 5.5, 2)]),
    "hourglass": dict(geo=[("outline", [(3.5, 2.5), (12.5, 2.5), (8, 8), (12.5, 13.5), (3.5, 13.5), (8, 8)])]),
    "content-loading": dict(geo=[("dot", 3.5, 8), ("dot", 8, 8), ("dot", 12.5, 8)]),
    # library -------------------------------------------------------------------
    "format-cd": dict(geo=[("text", "CD", (0.5, 3.5, 15, 9))]),
    "format-hires": dict(geo=[("text", "HR", (0.5, 3.5, 15, 9))]),
    "format-dsd": dict(geo=[("text", "DSD", (0.5, 4, 15, 8))]),
    "format-base": dict(geo=[("text", "SD", (0.5, 3.5, 15, 9))]),
    "star-large": dict(geo=[("solid", star(8, 8.5, 7, 3))]),
    "star-outline-rounded": dict(geo=[("outline", star(8, 8.5, 6.5, 2.8))]),
    "star-outline-half-left": dict(geo=[("outline", star(8, 8.5, 6.5, 2.8)),
                                        ("solid", [p for p in star(8, 8.5, 6.5, 2.8) if p[0] <= 8.01] + [(8, 15), (8, 2)])]),
    "music-note-single": dict(geo=[("disc", 5.5, 12, 2.5), ("line", 7.5, 12, 7.5, 3), ("pline", [(7.5, 3), (12.5, 4.5), (12.5, 8)])]),
    "music-artist": dict(geo=[("circle", 8, 5, 3), ("pline", [(2.5, 14), (2.5, 11.5), (5, 9.5), (11, 9.5), (13.5, 11.5), (13.5, 14)])]),
    "library-music": dict(geo=[("outline", [(2.5, 3.5), (13.5, 3.5), (13.5, 13.5), (2.5, 13.5)]), ("line", 4.5, 1.5, 11.5, 1.5),
                               ("disc", 6.5, 10, 1.5), ("line", 8, 10, 8, 6), ("line", 8, 6, 11, 7)]),
    "library-artists": dict(geo=[("circle", 5.5, 5, 2.5), ("circle", 11, 5, 2.5),
                                 ("pline", [(1.5, 13.5), (1.5, 11), (4, 9.5), (12, 9.5), (14.5, 11), (14.5, 13.5)])]),
    "recent": dict(geo=[("ring", 8, 8, 6), ("pline", [(8, 4.5), (8, 8), (11, 9.5)])]),
    "folder": dict(geo=[("shape", [(2, 3.5), (6.5, 3.5), (8, 5.5), (14, 5.5), (14, 13), (2, 13)])]),
    "playlist": dict(geo=[("dot", 3, 4), ("line", 6, 4, 13, 4), ("dot", 3, 8), ("line", 6, 8, 13, 8), ("dot", 3, 12), ("line", 6, 12, 13, 12)]),
    "paper": dict(geo=[("outline", [(3.5, 1.5), (12.5, 1.5), (12.5, 14.5), (3.5, 14.5)]), ("line", 6, 5, 10, 5), ("line", 6, 8, 10, 8), ("line", 6, 11, 10, 11)]),
    "lyrics-on": dict(geo=[("outline", [(2, 2.5), (14, 2.5), (14, 10.5), (7, 10.5), (4, 13.5), (4, 10.5), (2, 10.5)]),
                           ("line", 5, 5.5, 11, 5.5), ("line", 5, 8, 9, 8)]),
    "lyrics-off": dict(geo=[("outline", [(2, 2.5), (14, 2.5), (14, 10.5), (7, 10.5), (4, 13.5), (4, 10.5), (2, 10.5)]), STRIKE]),
    "brush": dict(geo=[("line", 9.5, 6.5, 13.5, 2.5, 2), ("shape", [(9.5, 6.5), (6, 8), (4, 11), (2.5, 13.5), (5, 12), (8, 10), (9.5, 6.5)])]),
}

# Theme-specific concepts. `glyphs` maps a style to a typographic rendering,
# `styles` maps a style to alternative geometry; anything unlisted uses `geo`.
STAR4 = [(8, 1.5), (9.6, 6.4), (14.5, 8), (9.6, 9.6), (8, 14.5), (6.4, 9.6), (1.5, 8), (6.4, 6.4)]
HATCH_PLATE = [("outline", [(2, 3.5), (14, 3.5), (14, 12.5), (2, 12.5)]),
               ("line", 2, 9.5, 8, 3.5), ("line", 2, 12.5, 11, 3.5), ("line", 5, 12.5, 14, 3.5), ("line", 8, 12.5, 14, 6.5), ("line", 11, 12.5, 14, 9.5)]
HATCH_DISC = [("ring", 8, 8, 6), ("line", 3, 9.5, 9.5, 3), ("line", 3.5, 12, 12, 3.5), ("line", 6.5, 13, 13, 6.5)]
ICONS["avatar"] = dict(geo=[("circle", 8, 5, 3), ("pline", [(2.5, 14), (2.5, 11.5), (5, 9.5), (11, 9.5), (13.5, 11.5), (13.5, 14)])],
                       styles={"dttw": HATCH_DISC}, glyphs={"terminal": ("@", (1, 1, 14, 14))})
ICONS["folder"]["styles"] = {"dttw": HATCH_PLATE}
# Nocturne's avatar reads better as an engraved bust ring than the stock figure.
ICONS["avatar"]["styles"]["nocturne"] = [("ring", 8, 8, 6), ("circle", 8, 6, 2), ("pline", arc(8, 12.5, 3.5, 200, 340, 10))]
ICONS["star-large"]["styles"] = {"dttw": [("solid", STAR4)]}
ICONS["star-outline-rounded"]["styles"] = {"dttw": [("outline", STAR4)]}
ICONS["star-outline-half-left"]["styles"] = {"dttw": [("outline", STAR4), ("solid", [pt for pt in STAR4 if pt[0] <= 8.01] + [(8, 14.5), (8, 1.5)])]}
ICONS["info-outline"]["glyphs"] = {"dttw": ("i", (2, 1, 12, 14)), "terminal": ("i", (2, 1, 12, 14))}
for name, ch in {"left": "<", "right": ">", "up": "^", "down": "v", "list-add": "+", "plus": "+", "list-remove": "-",
                 "open-menu": "≡", "view-more": "…", "edit-find": "/", "cross-small": "x", "exclamation-mark": "!",
                 "star-large": "*", "star-outline-rounded": "*", "music-note-single": "♪"}.items():
    ICONS[name].setdefault("glyphs", {})["terminal"] = (ch, (1, 1, 14, 14))
# the dttw glyphs declared inline above move into the per-style dict
for name, spec in ICONS.items():
    if "glyph" in spec:
        spec.setdefault("glyphs", {})["dttw"] = spec.pop("glyph")

# Names on disk carry the -symbolic suffix. Names that also exist in Adwaita get
# an `fl-` prefix (mirrored in the UI files): the system icon theme is searched
# before app resources, so an unprefixed override would never be picked.
ADWAITA_CLASH = {"media-playlist-shuffle", "open-menu", "edit-find", "view-sort-ascending",
                 "view-sort-descending", "view-more", "list-add", "list-remove", "user-trash",
                 "document-edit", "edit-select-all", "view-refresh", "content-loading", "folder", "avatar"}
NAMES = {name: (f"fl-{name}-symbolic" if name in ADWAITA_CLASH else f"{name}-symbolic") for name in ICONS}


def render_icon(style_key, name, spec):
    style = STYLES[style_key]
    r = Renderer(style)
    glyph = spec.get("glyphs", {}).get(style_key)
    if glyph:
        text, box = glyph
        r.text(text, box)
    else:
        for prim in spec.get("styles", {}).get(style_key, spec["geo"]):
            r.draw(prim)
    return r.svg()


# ---------------------------------------------------------------------------
# Album-art placeholders: one 512px plate per set, in that theme's own idiom
# ---------------------------------------------------------------------------

def placeholder_svg(key):
    S = 512
    r = Renderer(STYLES[key])
    body = []
    if key == "dttw":
        # the design system's plate: paper, 1px line ink, halftone fill, typewriter label
        r.text("NO LOCAL ART", (96, 236, 320, 40), font="special-elite", tracking=0.12)
        label = "".join(f'<path d="{d}" fill="#141412"/>' for d, _ in r.paths)
        body = [f'<rect width="{S}" height="{S}" fill="#e8e5dc"/>',
                '<defs><pattern id="tone" width="10" height="10" patternUnits="userSpaceOnUse"><circle cx="2" cy="2" r="2.2" fill="#111"/></pattern></defs>',
                f'<rect x="1" y="1" width="{S-2}" height="{S-2}" fill="url(#tone)" opacity=".38"/>',
                '<rect x="86" y="222" width="340" height="68" fill="#e8e5dc"/>', label,
                f'<rect x=".5" y=".5" width="{S-1}" height="{S-1}" fill="none" stroke="#141412" stroke-width="1"/>']
    elif key == "terminal":
        r.text("NO ART", (116, 226, 280, 60), font="mono")
        label = "".join(f'<path d="{d}" fill="#39ff88"/>' for d, _ in r.paths)
        body = [f'<rect width="{S}" height="{S}" fill="#070a08"/>',
                '<defs><pattern id="grid" width="32" height="32" patternUnits="userSpaceOnUse"><path d="M32 0H0V32" fill="none" stroke="#39ff88" stroke-opacity=".12" stroke-width="1"/></pattern></defs>',
                f'<rect width="{S}" height="{S}" fill="url(#grid)"/>', label,
                f'<rect x=".5" y=".5" width="{S-1}" height="{S-1}" fill="none" stroke="#39ff88" stroke-opacity=".3"/>']
    elif key == "neon":
        for prim in ICONS["music-note-single"]["geo"]:
            r.draw(prim)
        note = "".join(f'<path d="{d}" fill="#ff7ac0"/>' for d, _ in r.paths)
        body = [f'<rect width="{S}" height="{S}" fill="#1c0f30"/>',
                '<defs><radialGradient id="glow"><stop offset="0" stop-color="#ff4fa3" stop-opacity=".45"/><stop offset="1" stop-color="#ff4fa3" stop-opacity="0"/></radialGradient></defs>',
                f'<circle cx="256" cy="256" r="220" fill="url(#glow)"/>',
                f'<g transform="translate(160,160) scale(12)">{note}</g>']
    elif key == "bauhaus":
        body = [f'<rect width="{S}" height="{S}" fill="#f4f1ea"/>',
                '<circle cx="210" cy="230" r="150" fill="#111111"/>',
                '<rect x="290" y="110" width="150" height="150" fill="#d7263d"/>',
                '<rect x="60" y="400" width="392" height="28" fill="#ffd23f"/>',
                '<rect x="60" y="428" width="392" height="6" fill="#111111"/>']
    elif key == "folio":
        r.text("no plate", (136, 232, 240, 46), font="old-standard-italic")
        label = "".join(f'<path d="{d}" fill="#6f6350"/>' for d, _ in r.paths)
        body = [f'<rect width="{S}" height="{S}" fill="#f6f0df"/>',
                '<rect x="10.5" y="10.5" width="491" height="491" fill="none" stroke="#b3a683"/>',
                '<rect x="16.5" y="16.5" width="479" height="479" fill="none" stroke="#c8bc9f"/>', label]
    elif key == "blueprint":
        # drafting sheet: grid, centre crosshair, mono annotation
        r.text("NO DRAWING ON FILE", (76, 240, 360, 26), font="plex-mono", tracking=0.08)
        label = "".join(f'<path d="{d}" fill="#d9e8f7"/>' for d, _ in r.paths)
        body = [f'<rect width="{S}" height="{S}" fill="#10294a"/>',
                '<defs><pattern id="bpgrid" width="32" height="32" patternUnits="userSpaceOnUse">'
                '<path d="M32 0H0V32" fill="none" stroke="#d9e8f7" stroke-opacity=".14"/></pattern></defs>',
                f'<rect width="{S}" height="{S}" fill="url(#bpgrid)"/>',
                '<path d="M256 176V336 M176 256H336" stroke="#d9e8f7" stroke-opacity=".5"/>',
                '<circle cx="256" cy="256" r="60" fill="none" stroke="#d9e8f7" stroke-opacity=".5"/>',
                label,
                f'<rect x="8.5" y="8.5" width="{S-17}" height="{S-17}" fill="none" stroke="#d9e8f7" stroke-opacity=".55"/>']
    elif key == "nocturne":
        # a bookplate: double rule, italic label
        r.text("ex libris", (166, 236, 180, 40), font="old-standard-italic")
        label = "".join(f'<path d="{d}" fill="#d9a441"/>' for d, _ in r.paths)
        body = [f'<rect width="{S}" height="{S}" fill="#221b15"/>',
                '<rect x="14.5" y="14.5" width="483" height="483" fill="none" stroke="#d9a441" stroke-opacity=".55"/>',
                '<rect x="22.5" y="22.5" width="467" height="467" fill="none" stroke="#d9a441" stroke-opacity=".3"/>', label]
    elif key == "cassette":
        # a tape label: cream card, orange stripe, two reels
        body = [f'<rect width="{S}" height="{S}" fill="#262019"/>',
                '<rect x="48" y="96" width="416" height="320" fill="#f2ead9"/>',
                '<rect x="48" y="150" width="416" height="34" fill="#d95f18"/>',
                '<circle cx="180" cy="300" r="52" fill="none" stroke="#291f16" stroke-width="8"/>',
                '<circle cx="332" cy="300" r="52" fill="none" stroke="#291f16" stroke-width="8"/>',
                '<rect x="48.5" y="96.5" width="415" height="319" fill="none" stroke="#291f16" stroke-width="3"/>']
    elif key == "marshmallow":
        # a soft rounded blob with a note-dot
        body = [f'<rect width="{S}" height="{S}" fill="#f8e7ec"/>',
                '<rect x="72" y="72" width="368" height="368" rx="96" fill="#ffffff"/>',
                '<circle cx="230" cy="316" r="46" fill="#c2557a"/>',
                '<rect x="268" y="150" width="16" height="170" rx="8" fill="#c2557a"/>']
    return (f'<?xml version="1.0" encoding="UTF-8"?>\n<svg xmlns="http://www.w3.org/2000/svg" width="{S}" height="{S}" viewBox="0 0 {S} {S}">'
            + "".join(body) + "</svg>\n")


def write_placeholders():
    for key in STYLES:
        (OUT / key / "albumart-placeholder.svg").write_text(placeholder_svg(key))
    print("placeholders written")


def write_sets():
    for key in STYLES:
        d = OUT / key / "icons" / "scalable" / "actions"
        d.mkdir(parents=True, exist_ok=True)
        for name, spec in ICONS.items():
            (d / f"{NAMES[name]}.svg").write_text(render_icon(key, name, spec))
    print(f"wrote {len(ICONS)} icons × {len(STYLES)} sets under {OUT.relative_to(ROOT)}")


def write_gresource():
    xml = GRESOURCE.read_text()
    begin, end = "  <!-- iconsets:begin (generated by tools/gen_icons.py) -->\n", "  <!-- iconsets:end -->\n"
    blocks = []
    for key in STYLES:
        files = "".join(
            f'    <file preprocess="xml-stripblanks" alias="{NAMES[n]}.svg">iconsets/{key}/icons/scalable/actions/{NAMES[n]}.svg</file>\n'
            for n in ICONS)
        blocks.append(f'  <gresource prefix="{PREFIX}/{key}/icons/scalable/actions/">\n{files}  </gresource>\n'
                      f'  <gresource prefix="{PREFIX}/{key}/">\n    <file preprocess="xml-stripblanks" alias="albumart-placeholder.svg">iconsets/{key}/albumart-placeholder.svg</file>\n  </gresource>\n')
    section = begin + "".join(blocks) + end
    if begin in xml:
        xml = re.sub(re.escape(begin) + r".*?" + re.escape(end), lambda m: section, xml, flags=re.S)
    else:
        xml = xml.replace("</gresources>", section + "</gresources>")
    GRESOURCE.write_text(xml)
    print("gresource section updated")


def write_sheets():
    SHEETS.mkdir(parents=True, exist_ok=True)
    for key, style in STYLES.items():
        cells = []
        for name in ICONS:
            svg = (OUT / key / "icons/scalable/actions" / f"{NAMES[name]}.svg").read_text()
            svg = svg.split("\n", 1)[1].replace('fill="#222222"', 'fill="currentColor"')
            cells.append(f'<div class="cell"><div class="row">'
                         f'<span class="i s16">{svg}</span><span class="i s24">{svg}</span><span class="i s32">{svg}</span>'
                         f'</div><div class="name">{name}</div></div>')
        html = f'''<!-- @dsCard group="Icons" name="{style["title"]}" -->
<!doctype html><meta charset="utf-8"><title>Flaclify icons — {style["title"]}</title>
<style>
body{{margin:0;background:{style["ground"]};color:{style["ink"]};font:12px/1.4 {'"Special Elite",monospace' if key == "dttw" else '"DejaVu Sans Mono",monospace'};padding:24px}}
h1{{font-size:14px;letter-spacing:.14em;text-transform:uppercase;margin:0 0 4px}}
p{{margin:0 0 20px;opacity:.7}}
.grid{{display:grid;grid-template-columns:repeat(auto-fill,minmax(150px,1fr));gap:18px 14px}}
.cell{{display:flex;flex-direction:column;gap:6px}}
.row{{display:flex;align-items:flex-end;gap:10px}}
.i svg{{display:block}} .s16 svg{{width:16px;height:16px}} .s24 svg{{width:24px;height:24px}} .s32 svg{{width:32px;height:32px}}
.name{{font-size:10px;opacity:.65;letter-spacing:.04em}}
</style>
<h1>{style["title"]}</h1><p>{style["blurb"]}</p>
<div style="display:flex;gap:24px;align-items:flex-start;margin-bottom:22px">
  <img src="data:image/svg+xml;utf8,{__import__("urllib.parse").parse.quote((OUT / key / "albumart-placeholder.svg").read_text())}" width="160" height="160" alt="placeholder">
  <div><div class="name" style="font-size:11px;opacity:.8;margin-bottom:6px">ALBUM-ART PLACEHOLDER</div><div class="name">shown for any release without local or fetched art; also the artist plate ground</div></div>
</div>
<div class="grid">{"".join(cells)}</div>
'''
        (SHEETS / f"{key}.html").write_text(html)
        # PNG contact sheet for a quick look without a browser
        cols, cell, pad = 12, 40, 8
        rows = math.ceil(len(ICONS) / cols)
        w, h = cols * cell + pad * 2, rows * cell + pad * 2
        items = []
        for i, name in enumerate(ICONS):
            svg = (OUT / key / "icons/scalable/actions" / f"{NAMES[name]}.svg").read_text().split("\n", 1)[1]
            inner = re.sub(r"^<svg[^>]*>|</svg>\s*$", "", svg).replace('fill="#222222"', f'fill="{style["ink"]}"')
            x, y = pad + (i % cols) * cell + 4, pad + (i // cols) * cell + 4
            items.append(f'<g transform="translate({x},{y}) scale(2)">{inner}</g>')
        sheet = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}">'
                 f'<rect width="100%" height="100%" fill="{style["ground"]}"/>{"".join(items)}</svg>')
        (SHEETS / f"{key}.svg").write_text(sheet)
        try:
            subprocess.run(["rsvg-convert", "-o", str(SHEETS / f"{key}.png"), str(SHEETS / f"{key}.svg")], check=True)
        except (OSError, subprocess.CalledProcessError) as e:
            print("png sheet skipped:", e)
    print(f"sheets in {SHEETS.relative_to(ROOT)}")


if __name__ == "__main__":
    write_sets()
    write_placeholders()
    write_gresource()
    write_sheets()

# Theming Flaclify

A theme is one GTK CSS file with a small header comment. It can recolour the whole
window, change shapes, density and type, bring its own icon set and album-art
placeholder, and decide how the album-art background and accent colours behave.
Themes apply live: from the **Theme** submenu, with `Ctrl+T` / `Ctrl+Shift+T`, from
the settings key, or by saving a file in the user theme directory.

This document is the guideline for writing one, followed by notes for developers
working on the engine itself.

---

## 1. Anatomy of a theme

```css
/* @name: Midnight
 * @scheme: dark
 * @auto-accent: off
 * @art-background: off
 * @icons: terminal
 *
 * Free text after the keys is ignored; use it to say what the theme is for.
 */

:root {
  --window-bg-color: #101418;
  --window-fg-color: #e6edf3;
  --accent-bg-color: #6fb3ff;
  --accent-fg-color: #08111c;
  /* ...every other libadwaita variable you want to own */
}

window { font-family: "Inter", sans-serif; }
button { border-radius: 0; }
```

### Header keys

| Key | Values | Meaning |
| --- | --- | --- |
| `@name` | text | Shown in the Theme menu. Defaults to the file stem. |
| `@scheme` | `light` `dark` `follow` | Forces libadwaita's colour scheme while the theme is active. Set it whenever you hard-code a palette; otherwise the user's light/dark choice and your colours can disagree. |
| `@auto-accent` | `off` `on` | `off` stops the album-art accent (and the system accent fallback) from overriding your `--accent-*` variables. Leave it out to let album art tint the accent, as Neon does. |
| `@art-background` | `off` `on` | `off` hides the blurred album-art wash behind the window without changing the user's setting. Flat, print-like themes want this. |
| `@icons` | set name | A bundled icon set under `src/iconsets/<name>/`, searched before the stock icons. Also supplies the album-art placeholder if the set ships one. |

Keys go in the **first** comment block. Anything unknown is ignored.

### Where themes live

- Bundled: `src/themes/<stem>.css`, listed in `src/flaclify.gresource.xml`. The stem is
  the theme id (`gsettings set io.github.h3303.Flaclify.ui theme <stem>`).
- User: `~/.config/flaclify/themes/<stem>.css`. The directory is watched; saving the file
  re-applies it while the app runs. Its id is `user/<stem>` and it appears under
  **Custom** in the menu.
- A theme may build on a bundled one:
  `@import url("resource:///io/github/h3303/Flaclify/themes/broadsheet.css");`
  then override what differs. Reversed Spine is built this way from Broadsheet.

---

## 2. What to set

### Colours

Set the libadwaita variables on `:root`. The full list is in libadwaita's docs; the ones
that matter here, grouped by what they paint:

- Window and views: `--window-bg-color` `--window-fg-color` `--view-bg-color` `--view-fg-color`
- Header bars: `--headerbar-bg-color` `--headerbar-fg-color` `--headerbar-backdrop-color`
  `--headerbar-border-color` `--headerbar-shade-color` `--headerbar-darker-shade-color`
- Sidebar and the queue's nested pane: `--sidebar-bg-color` `--sidebar-fg-color`
  `--sidebar-backdrop-color` `--sidebar-border-color` `--sidebar-shade-color`, plus the
  `--secondary-sidebar-*` set
- Cards, popovers, dialogs: `--card-bg-color` `--card-fg-color` `--card-shade-color`
  `--popover-bg-color` `--popover-fg-color` `--popover-shade-color` `--dialog-bg-color`
  `--dialog-fg-color` `--thumbnail-bg-color`
- Accent and semantic: `--accent-bg-color` `--accent-fg-color` `--accent-color`
  `--destructive-*` `--success-color` `--warning-color` `--error-color`
- Misc: `--shade-color` `--border-color` `--scrollbar-outline-color` `--dim-opacity`
- Long-form text (album wiki, artist bio) does **not** use the window font. Set
  `--document-font-family` and `--document-font-size`, and mirror them on
  `.document, textview, textview text`.

Give the `*-shade-color` variables a translucent version of the pane colour: they are what
the sidebar and player bar show when the album-art wash is on.

### Type

`window { font-family; font-size }` sets the base. Headings use `.title-1` … `.title-4`
and `.heading`; small labels use `.caption`, `.caption-heading`, `.dim-label`, `.dimmed`.
The sidebar masthead is the sidebar page's header title:
`.light-right-edge headerbar .title` and `… .subtitle`. Scope it to `.light-right-edge`,
not `.sidebar-pane`: the queue's "Now Playing" pane is also a `.sidebar-pane`.

**Never put `letter-spacing` on a label that can ellipsize.** GTK measures ellipsized
labels without their tracking, so tracked text gets cut short. Sidebar labels, album
and artist captions, window titles and the player bar all ellipsize. Tracking is safe on
button labels, tooltips, toasts and headings that have room.

### Shape

The app adds a few hooks on top of GTK's node names:

| Selector | What it is |
| --- | --- |
| `.sidebar-btn` | The sidebar navigation buttons (`:checked` is the current view) |
| `.queue-btn` | The Queue toggle at the foot of the sidebar. A plain flat button, not a `.sidebar-btn`, so give its `:checked` state its own rule |
| `.flaclify-sidebar` | The sidebar widget under the masthead |
| `.light-right-edge` | The sidebar page; owns the vertical rule (see §4) |
| `.light-left-edge` | The queue view's content page, same role |
| `.player-bar` | The transport bar along the foot |
| `.seekbar2` | The seek bar and its time labels |
| `.play-button` | The main play/pause button |
| `.cover-shadow` | Album-art plates in grids and rows |
| `.border-radius-6` `.border-radius-12` `.radius-6` | Rounded boxes the stock look uses; zero them for hard edges |
| `.caption-heading` | Album titles in cells (an `EuphonicaMarquee`; style its inner `label` for a chip that hugs the text) |
| `.discography-line` | The year rule in artist discographies |
| `.no-shading` | Set on the window while the album-art wash is on; stock CSS makes panes transparent under it |
| `.fg-auto-accent` | Labels the engine tints with the album accent |

Everything else is plain GTK and libadwaita: `headerbar`, `button`, `button.flat`,
`button.circular`, `.pill`, `entry`, `popover > contents`, `listview > row`,
`gridview > child`, `scale > trough > highlight`, `switch`, `checkbutton > check`,
`avatar`, `tooltip`, `.toast`, `.osd`.

### Icons and placeholders

Point `@icons` at a set. Each set lives in `src/iconsets/<name>/icons/scalable/actions/`
with an optional `albumart-placeholder.svg` beside `icons/`. The sets are generated, not
drawn by hand; see §6. A theme with no `@icons` uses the stock icons.

Icon names the app also gets from Adwaita are prefixed `fl-` in the UI (`fl-edit-find`,
`fl-open-menu`, `fl-folder`, `fl-avatar`, …). Use those names in a set: the system icon
theme is searched before app resources, so an unprefixed override would never be picked.

---

## 3. Contrast in reversed states

Captions and dim labels are tricky in two directions. libadwaita already dims `.dim-label`
and `.dimmed` with `opacity: var(--dim-opacity)`, so do not also pin a translucent colour on
them: colour alpha × opacity dims the label twice and drops 9pt text under 4.5:1 (set
`--dim-opacity` instead, and keep any pinned colour opaque). Inside anything you reverse (a
checked sidebar button, a selected row, an active toggle) a pinned colour does not follow
the reversal and the label vanishes. Every theme therefore ends with a block that sets label
and icon colours explicitly for those states:

```css
button:checked label, button:checked image,
button:active label, button:active image,
listview > row:selected label, listview > row:selected image,
gridview > child:selected label, gridview > child:selected image {
  color: <your reversed foreground>;
}
button:checked .caption, listview > row:selected .caption,
gridview > child:selected .caption {
  color: <your reversed foreground>; opacity: .75;
}
```

If you import a bundled theme and reverse something the other way (Reversed Spine's
paper-on-black sidebar), your selector needs to **out-rank** the parent's, e.g.
`button.flat.sidebar-btn:checked label`. Check a checked nav item, a selected album,
a selected queue row and an active toggle in the player pane before calling it done.

---

## 4. Corner geometry

Where the sidebar header, the content header and the sidebar edge meet, three things
draw lines: libadwaita's inset sidebar line, the split-view pane border, and the
navigation page's own edge. A theme that draws rules must let exactly one of them own
the vertical rule and pin both header bars to the same height:

```css
:root { --sidebar-border-color: transparent; --secondary-sidebar-border-color: transparent; }
.sidebar-pane, .flaclify-sidebar { border-right: none; box-shadow: none; }
.light-right-edge { border-right: 1px solid <ink>; }
.light-left-edge  { border-left:  1px solid <ink>; }
headerbar { min-height: 43px; }                 /* content header: ~3px more chrome */
.sidebar-pane headerbar { min-height: 46px; }   /* sidebar header: two-line title */
toolbarview > revealer.top-bar, toolbarview > .top-bar,
.top-bar > box, .top-bar { box-shadow: none; }
undershoot.top { background-image: none; box-shadow: none; }
```

The two heights depend on your title fonts and were measured, not derived; if you change
the masthead size, re-measure. The recipe is at the foot of every bundled theme.

---

## 5. Checklist before shipping a theme

- [ ] `@scheme` set if the palette is hard-coded; `@auto-accent: off` if the theme owns its accent.
- [ ] `@art-background: off` if the design is flat; otherwise every pane has a `*-shade-color`.
- [ ] No `letter-spacing` on ellipsizing labels (sidebar, cells, titles, player bar).
- [ ] Reversed-state contrast block present; checked nav, selected row, selected cell, active toggle all readable.
- [ ] Masthead scoped to `.light-right-edge`, not `.sidebar-pane`.
- [ ] Corner block present if the theme draws rules; header rules meet the divider in one cross.
- [ ] `--document-font-family` set so wikis and bios follow the theme.
- [ ] Album-art wash **on and off** both look intentional.
- [ ] Popovers, tooltips, toasts, the preferences dialog and the queue view checked, not only the album grid.

---

## 6. For developers

### The engine

`src/theme/mod.rs` owns one `gtk::CssProvider` at `STYLE_PROVIDER_PRIORITY_USER`, installed
in `main.rs` from the app's startup handler **before** any window exists. Order matters:
the window's accent provider is added later at the same priority, so it cascades over the
theme, which is what lets album-art accents win when a theme allows them.

`theme::init()` scans bundled themes (gresource `/themes/`) and user themes
(`~/.config/flaclify/themes`), applies the saved theme, watches the user directory with a
`GFileMonitor` (debounced 250 ms; a rescan re-applies the current theme if it is a user
theme), and listens to the `theme` settings key so external `gsettings` writes apply live.

`ThemeManager::apply(id)` in order: load the CSS, switch the icon set, force the colour
scheme if declared, save the id, notify listeners. Listeners are how the rest of the app
reacts:

- `window.rs` refreshes the accent provider (respecting `suppresses_auto_accent()`), re-evaluates
  the album-art wash (`art_bg_enabled()` = user setting AND theme allows it) and rebuilds the
  Theme submenu when the theme list changes.
- `application.rs` keeps the stateful `app.theme` action in step so the menu's radio mark
  follows whatever changed the theme (menu, shortcut, hot reload, gsettings).

### Icon sets and placeholders

`apply_icon_set()` rewrites the display's `IconTheme` resource path with the set's
`/iconsets/<set>/icons` first and everything previously there after. Widgets pick the
change up through the icon theme's `changed` signal.

`cache/placeholders.rs` exposes `albumart(thumb)`, a `ThemedPlaceholder` paintable that
wraps the set's `albumart-placeholder.svg` (or the stock artwork). `refresh(set)` swaps the
texture and calls `invalidate_contents()`, so every cell holding the paintable repaints.
Do not hand widgets a `Texture` for a placeholder; hand them the paintable.

### Generating a set

`tools/gen_icons.py` is the source of truth for every non-stock icon. Each icon is one
geometry spec on a 16×16 grid (`line`, `pline`, `outline`, `shape`, `solid`, `ring`,
`circle`, `disc`, `dot`, `text`), rendered through each style in `STYLES` (stroke width,
cap, whether closed shapes fill, which typeface renders text). A spec may add `glyphs`
(a typographic rendering for a given style) or `styles` (different geometry for a given
style) when a theme wants a different concept, not just a different weight.

Output is fill-only paths: strokes are expanded to quads and joints, because GTK's symbolic
recolouring is only guaranteed for fills. Run it with the build venv:

```sh
python3 -m venv build/venv && build/venv/bin/pip install fonttools
build/venv/bin/python tools/gen_icons.py
```

It writes the sets, the `iconsets` section of `flaclify.gresource.xml`, and review sheets
(HTML with `@dsCard` markers for Claude Design, plus PNG contact sheets) under
`build/iconsheets/`. To add a style: add an entry to `STYLES`, give any icons that need a
different concept a `styles`/`glyphs` entry, and point a theme's `@icons` at it.

### Adding a bundled theme

1. `src/themes/<stem>.css` with the header keys.
2. Add `<file alias="<stem>.css">themes/<stem>.css</file>` to the `/themes/` block in
   `src/flaclify.gresource.xml`.
3. If it needs its own icons: a new style in `gen_icons.py`, regenerate, and `@icons: <style>`.
4. Rebuild, then walk the checklist in §5 with the album-art wash on and off.

### Verifying on screen

Screenshots beat trusting CSS. The bundled themes were checked with `grim` on the app
window, cropping the top-left corner at 5× for the rule geometry and sampling pixel columns
for header heights. Views can be switched from outside the app:

```sh
gdbus call --session --dest io.github.h3303.Flaclify \
  --object-path /io/github/h3303/Flaclify/window/1 \
  --method org.gtk.Actions.Activate view-albums '[]' '{}'
```

`view-recent`, `view-albums`, `view-artists`, `view-folders`, `view-playlists`, `view-get`, `view-requests`, `view-tidy` and
`view-queue` exist; `app.cycle-theme` is on the application object path.

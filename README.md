# Flaclify

A personal fork of [Euphonica](https://github.com/htkhiem/euphonica), the GTK4 + libadwaita MPD
client by htkhiem, renamed so it can be installed and hacked on alongside the upstream package.

Additions over upstream:

- **Themes.** Switchable, strongly contrasting looks (palette, shape, density, type) applied live
  from the primary menu or by keyboard, plus user theme files in `~/.config/flaclify/themes/`
  that hot-reload on save.

## Themes

Pick one from the primary menu (**Theme** submenu) or cycle with `Ctrl+T` / `Ctrl+Shift+T`.
Bundled: Adwaita (stock), Broadsheet, Reversed Spine, Folio, Terminal, Neon, Bauhaus.
Broadsheet is direction 1a of the "Euphonica × Moirai Digital" canvas (Death to the World
design system): toner on paper, typewriter labels, blackletter masthead. Reversed Spine is
direction 1b: it `@import`s Broadsheet and reverses the sidebar and transport to toner. Its five typefaces ship in `data/fonts/` and
install to `share/fonts/flaclify` (OFL, Special Elite under Apache 2.0).

A theme is one GTK CSS file. Drop your own into `~/.config/flaclify/themes/`; the directory is
watched, so saving the file re-applies it while the app runs. Metadata goes in the leading
comment:

```css
/* @name: Midnight
 * @scheme: dark          light | dark | follow  (optional; forces the colour scheme)
 * @auto-accent: off      off | on               (optional; off stops album-art accents
 *                                                overriding the theme's own accent)
 * @art-background: off   off | on               (optional; off hides the blurred album-art
 *                                                wash while the theme is active)
 */
:root {
  --window-bg-color: #101418;
  --accent-bg-color: #6fb3ff;
  /* any libadwaita variable: --view-bg-color, --headerbar-bg-color, --sidebar-bg-color,
     --card-bg-color, --popover-bg-color, --border-color, ... */
}
window { font-family: "Inter", sans-serif; }
button { border-radius: 0; }
```

The bundled themes in `src/themes/` are the reference for which selectors the app exposes
(`.sidebar-btn`, `.player-bar`, `.cover-shadow`, `.border-radius-6`, and so on). One GTK
quirk to know: ellipsized labels are measured without `letter-spacing`, so tracked text gets
cut short. Keep tracking to buttons, tooltips and other labels that never ellipsize.
A theme may also `@import url("resource:///io/github/h3303/Flaclify/themes/<stem>.css");`
to build on a bundled one. Any state that reverses a surface (checked, active, selected)
must set its own label and icon colours, since captions carry their own ink colour. Themes can
also be switched from outside the app:

```sh
gsettings set io.github.h3303.Flaclify.ui theme folio      # or user/<file-stem>
```

## Build

Dependencies are the same as upstream: `gtk4` >= 4.18, `libadwaita` >= 1.7, `meson`, `gettext`,
`sqlite`, `pipewire`, `libsecret`, an MPD >= 0.24 to talk to, and a stable Rust toolchain.

```sh
meson setup build -Dprofile=dev -Dprefix=$HOME/.local
ninja -C build install
flaclify
```

The app ID is `io.github.h3303.Flaclify` and it keeps its own config, cache and settings, so it
does not interfere with a system Euphonica install. The one thing deliberately shared is the MPD
password in the keyring.

## License

GPL-3.0-or-later, as upstream. See `COPYING`.

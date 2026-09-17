#!/usr/bin/env bash
# Screenshots of the running Flaclify for the site. The view is switched over D-Bus (the window's
# org.gtk.Actions, no focus change) and the window is grabbed with grim, so Flaclify must be on the
# current Hyprland workspace, visible and unobstructed. Output: <out>/ui-<view>.jpg at 1600 px wide.
#
#   tools/shoot_ui.sh                       # albums artists get requests tidy ask queue -> site/img
#   tools/shoot_ui.sh site/img current:artist   # grab what is on screen now, as ui-artist.jpg
set -euo pipefail

OUT=${1:-site/img}
shift || true
VIEWS=${*:-"albums artists get requests tidy ask queue"}
APP=io.github.h3303.Flaclify
WINDOW=/io/github/h3303/Flaclify/window/1

geometry() {
  hyprctl clients -j | python3 -c "
import json, sys
clients = [c for c in json.load(sys.stdin) if c['class'] == '$APP']
if not clients:
    sys.exit('Flaclify is not running')
c = clients[0]
print(f\"{c['at'][0]},{c['at'][1]} {c['size'][0]}x{c['size'][1]}\")
"
}

mkdir -p "$OUT"
for spec in $VIEWS; do
  case "$spec" in
    current:*) name=${spec#current:} ;;
    *)
      name=$spec
      gdbus call --session --dest "$APP" --object-path "$WINDOW" \
        --method org.gtk.Actions.Activate "view-$spec" '[]' '{}' >/dev/null
      sleep 2.5
      ;;
  esac
  grim -g "$(geometry)" - | magick - -resize '1600x>' -quality 88 "$OUT/ui-$name.jpg"
  echo "$OUT/ui-$name.jpg"
done

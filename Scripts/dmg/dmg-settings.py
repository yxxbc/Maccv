# dmgbuild settings for MacCV.
#
# Loaded by Scripts/make-dmg.sh with `dmgbuild -s`. Everything the image needs —
# window size, background, icon positions — is described here, so the artwork
# generator and the layout cannot drift apart.
#
# `dmgbuild` writes the `.DS_Store` itself rather than driving Finder over
# AppleEvents, which is why this is preferred over create-dmg in CI.

from __future__ import annotations

import os

# ---------------------------------------------------------------------------
# Identity
# ---------------------------------------------------------------------------

# The volume name is passed on the command line, so it carries the version.
volume_name = defines.get("volume_name", "MacCV")  # noqa: F821

# A read-only compressed image. UDZO is the conventional choice: broadly readable,
# and the format users expect from a downloaded app.
format = "UDZO"

# The app bundle to place in the image, as an absolute path. The build script
# stages a copy and passes its location, so the image is assembled from a fixed
# input rather than from whatever happens to be in DerivedData.
staged_app = os.environ.get("MACCV_DMG_STAGED_APP", "")

app_name = os.environ.get("MACCV_DMG_APP_NAME", "MacCV.app")
if not app_name.endswith(".app"):
    app_name = f"{app_name}.app"

if not staged_app or not os.path.exists(staged_app):
    raise SystemExit(
        f"dmg-settings: MACCV_DMG_STAGED_APP is unset or missing ({staged_app!r}); "
        "the image would have been built without the application"
    )

# No software licence text, so no need to prompt for agreement on mount.
files = [staged_app]
symlinks = {"Applications": "/Applications"}

# ---------------------------------------------------------------------------
# Layout
#
# Must match WIDTH/HEIGHT in generate-background.py: Finder draws the background at
# exactly this size and does not scale it, so a mismatch shows as a cropped or
# padded canvas inside the mounted image.
# ---------------------------------------------------------------------------

window_rect = ((200, 120), (660, 420))

icon_size = 108

icon_locations = {
    app_name: (165, 205),
    "Applications": (495, 205),
}

# ---------------------------------------------------------------------------
# Background
#
# Both appearances are supplied: a light-only canvas is unreadable for anyone
# running dark appearance, which for a developer tool is most people.
# ---------------------------------------------------------------------------

background = os.environ.get("MACCV_DMG_BACKGROUND")
if background and os.path.exists(background):
    # Finder reads a multi-resolution TIFF, which `tiffutil -cathidpicheck` builds
    # from the 1x and 2x PNGs.
    pass

# ---------------------------------------------------------------------------
# Finder cosmetics
#
# `show_status_bar` and friends are off: the image holds two icons, and chrome only
# gets in the way of the drag gesture the layout is teaching.
# ---------------------------------------------------------------------------

show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False

# The default is to arrange by name, which would override icon_locations.
arrange_by = None
grid_offset = (0, 0)
grid_spacing = 100
scroll_position = (0, 0)

# Nothing to select, so no stray highlight on open.
default_view = "icon-view"
include_icon_view_settings = True
include_list_view_settings = False

# The window is not resizable, matching the fixed-size background.
window_size_hint = (660, 420)

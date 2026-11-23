# dmgbuild settings for Aleno.dmg. Run through scripts/dmg/build.sh, which passes app= and background= with -D.
#
# The icon positions must match ICON_Y / APP_X / FOLDER_X in render-background.swift, so the app, the arrow and the
# Applications folder sit on one horizontal line. The window is 68 pt taller than its 340 pt content area, which
# leaves room for the title bar and toolbar Finder draws on macOS 26.
import os

app = defines["app"]                      # noqa: F821  (dmgbuild injects `defines`)
background = defines["background"]        # noqa: F821

format = "UDZO"
files = [app]
symlinks = {"Applications": "/Applications"}
icon_locations = {os.path.basename(app): (180, 140), "Applications": (480, 140)}

window_rect = ((200, 120), (660, 408))
default_view = "icon-view"
icon_size = 112
text_size = 13
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
arrange_by = None

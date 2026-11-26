# dmgbuild settings for Dromon's DMG (build-app.sh runs: dmgbuild -s settings.py -D app=<Dromon.app>
# -D background=<background.tiff> Dromon <out.dmg>). Finder opens it as the installer window: the
# app on the left, an Applications link on the right, the background's arrow between them.
# The icon centres must match make-background.py's APP and APPLICATIONS.
# ruff: noqa: F821 (dmgbuild runs this with `defines` in scope)
import os.path

app = defines["app"]

format = "UDZO"
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
icon_locations = {
    os.path.basename(app): (165, 175),
    "Applications": (495, 175),
}

background = defines["background"]
# The window's frame, title bar included: the 660x400 background plus Finder's 28 pt title bar
# (no toolbar).
window_rect = ((200, 160), (660, 428))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
show_icon_preview = False
icon_size = 128
text_size = 13
arrange_by = None

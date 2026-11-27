//! Scenes of the quick preview (Space): a photo fitted to the panel, the start of a long text
//! file with its line numbers and the note that it was cut (dark and light), and an archive,
//! which shows its details. The demo's files hold what `fixture/preview.rs` makes up.

use dromon_core::{HostId, RemotePath};

use crate::state::{Action, App, PreviewAction};

/// prod's site with `name` selected and its preview open.
fn previewed(s: &mut App, name: &str) {
    super::listing(s);
    super::click(s, name, false);
    s.act(Action::Preview(PreviewAction::Toggle));
}

/// A photo of the demo's big folder on the panel, its pixel size in the header.
pub fn preview_image(s: &mut App) {
    *s = super::base();
    s.act(Action::SelectHost(HostId("prod".into())));
    s.act(Action::GoTo(RemotePath::new(super::BIG_FOLDER)));
    super::click(s, "IMG_00001.jpg", false);
    s.act(Action::Preview(PreviewAction::Toggle));
    // Decoded on Rune's workers: waited for, so the picture is the same on every run.
    s.ui.preview.wait_for_image();
}

/// package-lock.json (412 KB): its first 256 KB, numbered, and the note that says so.
pub fn preview_text(s: &mut App) {
    previewed(s, "package-lock.json");
}

/// An archive: no preview, its kind, size and date.
pub fn preview_unsupported(s: &mut App) {
    previewed(s, "backup-2026-11-01.tar.gz");
}

/// [`preview_text`] in the light theme.
pub fn preview_text_light(s: &mut App) {
    super::lit(s, preview_text);
}

//! Scenes of the bug report sheet: opened from a failed delete's Report (dark and light), so it
//! shows the error with its path replaced; a window recording its steps (the sidebar's pill);
//! and the sheet with a recording too long for the link (the fixture is copied) and a folder
//! too big to include whole. This copy, the computer and the display (as the goldens render it,
//! at 2×) are pinned, so the pictures don't change with the release or the machine: a scene's
//! report is built before its first frame is drawn.

use dromon_core::{HostId, RemotePath};

use crate::state::report::{About, ReportAction, Seen};
use crate::state::toast::ToastId;
use crate::state::update::{Install, UpdateEvent};
use crate::state::{Action, App, PreviewAction, ServiceEvent};
use crate::theme::{Access, Appearance};

/// This copy, the computer and the display, pinned.
fn pin(s: &mut App, dark: bool) {
    s.report.about = Some(About {
        version: "9.9.0".into(),
        os: "Ubuntu 24.04.1 LTS (Linux 6.8.0-49-generic)".into(),
        arch: "x86_64".into(),
        desktop: Some("GNOME on wayland".into()),
    });
    s.report.saw(Seen { scale: 2.0, dark, access: Access::default() });
    s.apply(ServiceEvent::Update(UpdateEvent::Install(Install::Deb)));
}

/// The failed delete's toast's Report, in the dark theme or the light one.
fn opened(s: &mut App, dark: bool) {
    super::delete_failed(s);
    if !dark {
        s.ui.appearance = Appearance::Light;
    }
    pin(s, dark);
    let subject = s.toasts().iter().find(|t| matches!(t.id, ToastId::Op(_))).and_then(|t| t.report());
    s.act(Action::Report(ReportAction::Open(subject)));
}

/// Recording on prod, already open: two steps in, robots.txt previewed.
pub fn recording(s: &mut App) {
    super::listing(s);
    s.act(Action::Report(ReportAction::Record));
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: false });
    s.act(Action::Preview(PreviewAction::Toggle));
}

/// Recorded from the start: prod, its releases, a few folders, then the 10,000 photos (too big
/// to include whole) and one of them; stopped. The link can't hold the fixture: Open GitHub
/// will copy it.
pub fn report_recorded(s: &mut App) {
    super::welcome(s);
    pin(s, true);
    s.act(Action::Report(ReportAction::Record));
    s.act(Action::SelectHost(HostId("prod".into())));
    s.act(Action::Open("releases".into()));
    for path in ["/", "/var", "/home", super::BIG_FOLDER] {
        s.act(Action::GoTo(RemotePath::new(path)));
    }
    s.act(Action::Click { name: "IMG_00042.jpg".into(), extend: false, toggle: false });
    s.act(Action::Report(ReportAction::StopRecording));
}

pub fn report(s: &mut App) {
    opened(s, true);
}

/// [`report`] in the light theme.
pub fn report_light(s: &mut App) {
    opened(s, false);
}

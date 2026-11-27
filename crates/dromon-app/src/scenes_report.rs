//! Scenes of the bug report sheet: opened from a failed delete's Report (dark and light), so it
//! shows the error with its path replaced. This copy, the computer and the display (as the
//! goldens render it, at 2×) are pinned, so the pictures don't change with the release or the
//! machine: a scene's report is built before its first frame is drawn.

use crate::state::report::{About, ReportAction, Seen};
use crate::state::toast::ToastId;
use crate::state::update::{Install, UpdateEvent};
use crate::state::{Action, App, ServiceEvent};
use crate::theme::{Access, Appearance};

/// The failed delete's toast's Report, in the dark theme or the light one.
fn opened(s: &mut App, dark: bool) {
    super::delete_failed(s);
    if !dark {
        s.ui.appearance = Appearance::Light;
    }
    s.report.about = Some(About {
        version: "9.9.0".into(),
        os: "Ubuntu 24.04.1 LTS (Linux 6.8.0-49-generic)".into(),
        arch: "x86_64".into(),
        desktop: Some("GNOME on wayland".into()),
    });
    s.report.saw(Seen { scale: 2.0, dark, access: Access::default() });
    s.apply(ServiceEvent::Update(UpdateEvent::Install(Install::Deb)));
    let subject = s.toasts().iter().find(|t| matches!(t.id, ToastId::Op(_))).and_then(|t| t.report());
    s.act(Action::Report(ReportAction::Open(subject)));
}

pub fn report(s: &mut App) {
    opened(s, true);
}

/// [`report`] in the light theme.
pub fn report_light(s: &mut App) {
    opened(s, false);
}

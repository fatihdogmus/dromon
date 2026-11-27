//! Updates, pure: versions, when a check is due, how each install takes an update, the
//! window's machine (toast, Settings line, the sheet under transfers), and the app's wiring
//! (commands sent, effects returned).

use std::path::PathBuf;

use dromon_core::HostId;
use dromon_transfer::{JobId, JobKind, JobView, Progress, TaskState};

use super::*;
use crate::fixture;
use crate::state::{Action, Recorder, Saved, ServiceEvent, Settings, SettingsChange, Shortcut};

fn v(s: &str) -> Version {
    Version::parse(s).unwrap_or_else(|| panic!("{s} is a version"))
}

fn offer(method: Method) -> Offer {
    Offer { version: v("0.3.0"), page: "https://github.com/fatihdogmus/dromon/releases/tag/0.3.0".into(), method }
}

fn installable() -> Offer {
    offer(Method::Install(Package::Dmg))
}

fn ev(u: &mut Update, ev: UpdateEvent) -> Vec<Out> {
    step(u, Input::Event(ev))
}

#[test]
fn versions_parse_only_plain_semver() {
    let table: &[(&str, Option<&str>)] = &[
        ("0.2.0", Some("0.2.0")),
        ("10.20.30", Some("10.20.30")),
        ("1.0.0-rc.1", Some("1.0.0-rc.1")),
        ("1.0.0-alpha-2.x", Some("1.0.0-alpha-2.x")),
        ("v0.2.0", None),
        ("0.2", None),
        ("0.2.0.1", None),
        ("0.2.x", None),
        ("0.2.0-", None),
        ("0.2.0-rc..1", None),
        ("0.2.0+build", None),
        ("0.2.0-rc_1", None),
        ("", None),
        (" 0.2.0", None),
        ("-1.0.0", None),
    ];
    for (input, shown) in table {
        assert_eq!(Version::parse(input).map(|v| v.to_string()).as_deref(), *shown, "{input:?}");
    }
    assert!(v("1.0.0-rc.1").is_pre() && !v("1.0.0").is_pre());
    assert_eq!(Version::running().to_string(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn versions_order_by_semver_precedence() {
    // Each is older than the next (semver.org §11's own example among them).
    let ascending = [
        "0.9.9",
        "0.10.0",
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
        "1.0.1",
        "1.1.0",
        "2.0.0",
    ];
    for pair in ascending.windows(2) {
        assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        assert!(v(pair[1]) > v(pair[0]));
    }
    assert_eq!(v("1.2.3").cmp(&v("1.2.3")), Ordering::Equal);
}

#[test]
fn only_a_newer_full_release_is_offered() {
    let table = [
        ("0.3.0", "0.2.0", true),
        ("0.2.1", "0.2.0", true),
        ("1.0.0", "0.99.99", true),
        // Never the same, never a downgrade.
        ("0.2.0", "0.2.0", false),
        ("0.1.9", "0.2.0", false),
        // Pre-releases are never offered, whatever their number.
        ("9.0.0-rc.1", "0.2.0", false),
        // A pre-release user is offered the release it led to.
        ("0.3.0", "0.3.0-rc.2", true),
    ];
    for (latest, running, offered) in table {
        assert_eq!(is_newer(&v(latest), &v(running)), offered, "{latest} for {running}");
    }
}

#[test]
fn a_check_is_due_when_on_and_none_ran_today() {
    let hour = Duration::from_secs(3600);
    let table = [
        (true, None, true),
        (true, Some(hour), false),
        (true, Some(DAY - Duration::from_secs(1)), false),
        (true, Some(DAY), true),
        (true, Some(DAY * 3), true),
        (false, None, false),
        (false, Some(DAY * 3), false),
    ];
    for (auto, since, due_now) in table {
        assert_eq!(due(auto, since), due_now, "auto {auto}, since {since:?}");
    }
}

#[test]
fn each_install_takes_an_update_its_own_way() {
    let version = v("0.3.0");
    let team = Install::MacApp { team: Some("ABCDE12345".into()) };
    let table = [
        (team.clone(), true, Method::Install(Package::Dmg)),
        (team, false, Method::Page(Manual::NoKey)),
        (Install::MacApp { team: None }, true, Method::Page(Manual::Unsigned)),
        (Install::MacHomebrew, true, Method::Command("brew upgrade --cask dromon".into())),
        (Install::MacHomebrew, false, Method::Command("brew upgrade --cask dromon".into())),
        (Install::WindowsMsi, true, Method::Install(Package::Msi)),
        (Install::WindowsMsi, false, Method::Page(Manual::NoKey)),
        (Install::Deb, true, Method::Command("sudo apt install ./Dromon-0.3.0-linux-x64.deb".into())),
        (Install::Arch, true, Method::Command("sudo pacman -U ./Dromon-0.3.0-linux-x64.pkg.tar.zst".into())),
        (Install::Other, true, Method::Page(Manual::Unknown)),
    ];
    for (install, key, expected) in table {
        assert_eq!(method(&install, key, &version), expected, "{install:?} with key {key}");
    }
}

#[test]
fn package_files_are_named_as_the_release_names_them() {
    assert_eq!(Package::Dmg.asset(&v("0.3.0")), "Dromon-0.3.0-macos-universal.dmg");
    assert_eq!(Package::Msi.asset(&v("0.3.0")), "Dromon-0.3.0-windows-x64.msi");
}

#[test]
fn a_check_that_finds_a_release_shows_the_toast_until_later() {
    let mut u = Update::default();
    assert_eq!(step(&mut u, Input::CheckNow), [Out::Send(UpdateCommand::Check)]);
    ev(&mut u, UpdateEvent::Checking);
    assert_eq!(u.phase, Phase::Checking);
    assert!(u.toast().is_none());
    ev(&mut u, UpdateEvent::Available(installable()));
    let toast = u.toast().expect("a toast");
    assert_eq!(toast.text, "Dromon 0.3.0 is available");
    assert_eq!(toast.buttons, [Button::Update, Button::Later, Button::Notes]);
    assert_eq!(toast.tone, Tone::Note);

    step(&mut u, Input::Later);
    assert!(u.toast().is_none(), "Later hides it");
    assert!(matches!(u.phase, Phase::Available(_)), "Settings still has it");
    // The next check that finds it shows it again.
    ev(&mut u, UpdateEvent::Checking);
    ev(&mut u, UpdateEvent::Available(installable()));
    assert!(u.toast().is_some());
}

#[test]
fn checks_that_find_nothing_or_fail_show_no_toast() {
    let mut u = Update::default();
    ev(&mut u, UpdateEvent::Checking);
    ev(&mut u, UpdateEvent::UpToDate);
    assert_eq!(u.phase, Phase::UpToDate);
    assert!(u.toast().is_none());
    assert_eq!(u.status(&v("0.2.0")), "Dromon 0.2.0 is up to date");

    ev(&mut u, UpdateEvent::Checking);
    ev(&mut u, UpdateEvent::CheckFailed("couldn't reach GitHub".into()));
    assert!(u.toast().is_none(), "a failed check says so on the Settings page only");
    assert_eq!(u.status(&v("0.2.0")), "Couldn't check for updates: couldn't reach GitHub");
    // Answers of a check that didn't start here change nothing.
    let mut idle = Update::default();
    ev(&mut idle, UpdateEvent::UpToDate);
    assert_eq!((idle.phase.clone(), idle.status(&v("0.2.0"))), (Phase::Idle, "You have Dromon 0.2.0".to_owned()));
}

#[test]
fn update_installs_at_once_without_transfers() {
    let mut u = Update::default();
    ev(&mut u, UpdateEvent::Available(installable()));
    assert_eq!(step(&mut u, Input::Update { transfers: 0 }), [Out::Send(UpdateCommand::Install)]);
    assert_eq!(u.phase, Phase::Installing(installable()));
    let toast = u.toast().unwrap();
    assert_eq!((toast.text.as_str(), toast.tone, toast.buttons.len()), ("Updating to Dromon 0.3.0…", Tone::Busy, 0));
    // A check meanwhile doesn't start, and news of one doesn't replace it.
    assert!(step(&mut u, Input::CheckNow).is_empty());
    ev(&mut u, UpdateEvent::Checking);
    ev(&mut u, UpdateEvent::Installing);
    assert_eq!(u.phase, Phase::Installing(installable()));
    assert_eq!(ev(&mut u, UpdateEvent::Installed { relaunch: true }), [Out::Quit { relaunch: true }]);
    assert_eq!(u.phase, Phase::Quitting);
}

#[test]
fn with_transfers_running_update_asks_first() {
    let mut u = Update::default();
    ev(&mut u, UpdateEvent::Available(installable()));
    assert!(step(&mut u, Input::Update { transfers: 2 }).is_empty(), "nothing installs yet");
    assert!(u.confirming());
    assert!(u.toast().is_none(), "the sheet asks instead");
    // Cancel keeps going, as quitting's does.
    step(&mut u, Input::Cancel);
    assert_eq!(u.phase, Phase::Available(installable()));
    step(&mut u, Input::Update { transfers: 2 });
    // News meanwhile doesn't take the question away.
    ev(&mut u, UpdateEvent::Available(installable()));
    assert!(u.confirming());
    assert_eq!(step(&mut u, Input::Confirm), [Out::Send(UpdateCommand::Install)]);
    assert_eq!(u.phase, Phase::Installing(installable()));
}

#[test]
fn a_failed_install_says_why_and_offers_it_again() {
    let mut u = Update::default();
    ev(&mut u, UpdateEvent::Available(installable()));
    step(&mut u, Input::Update { transfers: 0 });
    ev(&mut u, UpdateEvent::InstallFailed("its checksums aren't signed with Dromon's release key".into()));
    let toast = u.toast().unwrap();
    assert_eq!(toast.tone, Tone::Error);
    assert_eq!(toast.text, "Couldn't update to Dromon 0.3.0: its checksums aren't signed with Dromon's release key");
    assert_eq!(toast.buttons, [Button::Update, Button::Later, Button::Notes]);
    assert_eq!(u.buttons(), [Button::Update, Button::Notes]);
    step(&mut u, Input::Later);
    assert!(u.toast().is_none());
    assert_eq!(step(&mut u, Input::Update { transfers: 0 }), [Out::Send(UpdateCommand::Install)], "again");
}

#[test]
fn copies_that_dont_install_themselves_offer_the_command_or_the_page() {
    let mut u = Update::default();
    ev(&mut u, UpdateEvent::Available(offer(Method::Command(BREW_UPGRADE.into()))));
    let toast = u.toast().unwrap();
    assert_eq!(toast.text, "Dromon 0.3.0 is available. Update with brew upgrade --cask dromon");
    assert_eq!(toast.buttons, [Button::Copy, Button::Later, Button::Notes]);
    assert_eq!(step(&mut u, Input::Copy), [Out::Copy(BREW_UPGRADE.into())]);
    assert!(step(&mut u, Input::Update { transfers: 0 }).is_empty(), "never installs it itself");
    let page = "https://github.com/fatihdogmus/dromon/releases/tag/0.3.0";
    assert_eq!(step(&mut u, Input::Notes), [Out::Open(page.into())]);

    // No usable release key: found and shown, never installed, and Settings says why.
    let mut u = Update::default();
    ev(&mut u, UpdateEvent::Available(offer(Method::Page(Manual::NoKey))));
    assert_eq!(u.toast().unwrap().buttons, [Button::Later, Button::Notes]);
    assert!(step(&mut u, Input::Update { transfers: 0 }).is_empty());
    assert!(step(&mut u, Input::Copy).is_empty());
    assert_eq!(u.buttons(), [Button::Notes]);
    let status = u.status(&v("0.2.0"));
    assert!(status.starts_with("Dromon 0.3.0 is available. This build has no release key"), "{status}");
    let unsigned = Update { phase: Phase::Available(offer(Method::Page(Manual::Unsigned))), ..Update::default() };
    assert!(unsigned.status(&v("0.2.0")).contains("isn't signed by Dromon's developer"));
}

#[test]
fn button_keys_and_actions() {
    let keys: Vec<String> =
        [Button::Update, Button::Copy, Button::Later, Button::Notes].iter().map(|b| b.key("toast-update")).collect();
    assert_eq!(keys, ["toast-update-install", "toast-update-copy", "toast-update-later", "toast-update-notes"]);
    assert_eq!(Button::Later.action(), UpdateAction::Later);
    assert_eq!((Button::Update.label(), Button::Copy.label()), ("Update", "Copy command"));
}

// ---- The app's wiring -------------------------------------------------------------------

fn running(id: u64) -> JobView {
    JobView {
        id: JobId(id),
        kind: JobKind::Download,
        host: HostId("prod".into()),
        title: "big.iso".into(),
        state: TaskState::Running,
        progress: Progress { done: 1, total: 9 },
        files_done: 0,
        files_total: 1,
        speed: None,
        eta: None,
        waiting_for_connection: false,
        conflict: None,
        reveal: None,
        errors: Vec::new(),
        skipped: Vec::new(),
        folders: Vec::new(),
    }
}

fn recorded() -> (App, Recorder) {
    let mut s = fixture::app(fixture::demo());
    let rec = Recorder::default();
    s.backend = Box::new(rec.clone());
    (s, rec)
}

fn updates(rec: &Recorder) -> Vec<UpdateCommand> {
    rec.sent()
        .into_iter()
        .filter_map(|c| match c {
            Command::Update(u) => Some(u),
            _ => None,
        })
        .collect()
}

fn update(s: &mut App, a: UpdateAction) -> Vec<Effect> {
    s.act(Action::Update(a))
}

#[test]
fn restored_settings_tell_the_service_whether_to_check_and_so_does_the_switch() {
    let (mut s, rec) = recorded();
    s.remember(Saved::default(), Box::new(crate::state::NoStore));
    assert_eq!(updates(&rec), [UpdateCommand::Auto(true)], "on by default: the launch check");
    s.act(Action::Setting(SettingsChange::CheckUpdates(false)));
    s.act(Action::Setting(SettingsChange::CheckUpdates(false)));
    assert_eq!(updates(&rec), [UpdateCommand::Auto(true), UpdateCommand::Auto(false)], "once per change");
    assert!(!s.settings.check_updates);

    let (mut s, rec) = recorded();
    let saved = Saved { settings: Some(Settings { check_updates: false, ..Settings::default() }), ..Saved::default() };
    s.remember(saved, Box::new(crate::state::NoStore));
    assert_eq!(updates(&rec), [UpdateCommand::Auto(false)]);
    // A window without remembered settings (scenes, tests, headless runs) never asks.
    let (_, rec) = recorded();
    assert!(updates(&rec).is_empty());
}

#[test]
fn the_app_sends_checks_and_installs_and_carries_out_the_rest() {
    let (mut s, rec) = recorded();
    update(&mut s, UpdateAction::CheckNow);
    s.apply(ServiceEvent::Update(UpdateEvent::Available(offer(Method::Command(BREW_UPGRADE.into())))));
    assert_eq!(update(&mut s, UpdateAction::Copy), [Effect::Copy(BREW_UPGRADE.into())]);
    let page = "https://github.com/fatihdogmus/dromon/releases/tag/0.3.0".to_owned();
    assert_eq!(update(&mut s, UpdateAction::Notes), [Effect::OpenUrl(page)]);
    s.apply(ServiceEvent::Update(UpdateEvent::Available(installable())));
    assert!(update(&mut s, UpdateAction::Update).is_empty());
    assert_eq!(updates(&rec), [UpdateCommand::Check, UpdateCommand::Install]);
    // In place: the news says quit and start it again, which the sink carries out.
    s.apply(ServiceEvent::Update(UpdateEvent::Installed { relaunch: true }));
    assert_eq!(s.take_update_effects(), [Effect::Quit { relaunch: true }]);
    assert!(s.take_update_effects().is_empty(), "once");
}

#[test]
fn the_update_sheet_is_modal_and_escape_cancels_it() {
    let (mut s, rec) = recorded();
    s.act(Action::SelectHost(HostId("prod".into())));
    s.apply(ServiceEvent::Transfers(vec![running(1)]));
    s.apply(ServiceEvent::Update(UpdateEvent::Available(installable())));
    update(&mut s, UpdateAction::Update);
    assert!(s.update.confirming());
    assert!(updates(&rec).is_empty(), "nothing installs while transfers run, unasked");
    // The list's shortcuts sleep behind it.
    let cursor = s.conn().and_then(|c| c.selection.cursor().map(str::to_owned));
    s.shortcut(Shortcut::Move { delta: 1, extend: false }, 0.0);
    assert_eq!(s.conn().and_then(|c| c.selection.cursor().map(str::to_owned)), cursor);
    // Escape (the sheet's modal dismissal) cancels it.
    s.act(Action::DismissSheet);
    assert!(!s.update.confirming());
    assert!(matches!(s.update.phase, Phase::Available(_)));
    // Asked again and confirmed: it installs.
    update(&mut s, UpdateAction::Update);
    update(&mut s, UpdateAction::Confirm);
    assert_eq!(updates(&rec), [UpdateCommand::Install]);
    assert_eq!(s.downloads, PathBuf::from("Downloads"), "nothing else changed");
}

#[test]
fn the_fixture_finds_nothing_newer() {
    let mut s = fixture::app(fixture::demo());
    update(&mut s, UpdateAction::CheckNow);
    assert_eq!(s.update.phase, Phase::UpToDate);
}

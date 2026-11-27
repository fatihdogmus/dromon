//! Updates (§11): Dromon checks GitHub's latest release, tells the user, and installs it where
//! the platform allows. This file is the pure half: versions and how they compare, how this
//! copy takes a newer version ([`Method`], which the service decides from how it was installed,
//! [`method`]), and each window's [`Update`] machine, moved by the pure [`step`], with the toast
//! and the Settings line it shows. The network, the checks of what's downloaded and the
//! installers are the service's (`service/update.rs`).
//!
//! - The service checks at launch and then once a day while Settings' "Check for updates
//!   automatically" is on ([`UpdateCommand::Auto`]: sent when the settings are restored and when
//!   the switch changes), and whenever the user asks ([`UpdateCommand::Check`]: Settings' "Check
//!   now", the macOS app menu's "Check for Updates…").
//! - A newer release shows a toast, "Dromon 0.3.0 is available", with Update (Copy command for a
//!   package manager's install), Later and Release notes. A check that fails on its own says so
//!   on the Settings page only.
//! - Update never runs under transfers: while some run, a sheet asks first, as quitting does.
//! - Once the new version is in place the app quits, starting the new one on macOS.
//!
//! Every window mirrors the service's news ([`UpdateEvent`] goes to all of them, `Installed` to
//! the window that asked); Later hides the toast in its window until a check finds the release
//! again.

use std::cmp::Ordering;
use std::fmt;
use std::time::Duration;

use super::toast::Tone;
use super::{App, Command, Effect};

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;

/// How often the service checks while automatic checks are on.
pub const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The command that updates a Homebrew install.
pub const BREW_UPGRADE: &str = "brew upgrade --cask dromon";

/// A release's version: plain semver, `MAJOR.MINOR.PATCH` with an optional `-pre.release`
/// (release tags have no `v` and no build metadata). Ordered by semver precedence.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    core: [u64; 3],
    pre: Vec<String>,
}

impl Version {
    /// `None` unless `s` is exactly a version: three numbers, maybe a pre-release of dotted
    /// alphanumeric identifiers.
    pub fn parse(s: &str) -> Option<Version> {
        let (core, pre) = match s.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (s, None),
        };
        let number = |p: &str| (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).then(|| p.parse().ok())?;
        let parts: Vec<&str> = core.split('.').collect();
        let [major, minor, patch] = parts.as_slice() else { return None };
        let core = [number(major)?, number(minor)?, number(patch)?];
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => {
                let ids: Vec<String> = pre.split('.').map(str::to_owned).collect();
                let ok = |id: &String| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
                if !ids.iter().all(ok) {
                    return None;
                }
                ids
            }
        };
        Some(Version { core, pre })
    }

    /// The version of this build (the workspace's).
    pub fn running() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("the workspace version is plain semver")
    }

    /// Whether it's a pre-release (`0.3.0-rc.1`), which updates never offer.
    pub fn is_pre(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.core;
        write!(f, "{major}.{minor}.{patch}")?;
        if self.is_pre() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| match (self.is_pre(), other.is_pre()) {
            (false, false) => Ordering::Equal,
            // A release comes after its pre-releases.
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (true, true) => {
                let ids = self.pre.iter().zip(&other.pre).map(|(a, b)| pre_id(a, b));
                ids.fold(Ordering::Equal, Ordering::then).then(self.pre.len().cmp(&other.pre.len()))
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Two pre-release identifiers: numbers by value and before words, words by their bytes.
fn pre_id(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

/// Whether a user running `running` is offered `latest`: a full release, newer than it. Never a
/// pre-release, never the same version or an older one.
pub fn is_newer(latest: &Version, running: &Version) -> bool {
    !latest.is_pre() && latest > running
}

/// Whether the service checks now: automatic checks are on and none ran in the last [`DAY`]
/// (`since_last`: how long ago the last one did; `None`: none this run).
pub fn due(auto: bool, since_last: Option<Duration>) -> bool {
    auto && since_last.is_none_or(|d| d >= DAY)
}

/// A release file the app installs itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Package {
    /// macOS: the universal app in its DMG, put in place of the running bundle.
    Dmg,
    /// Windows: the MSI, which upgrades the installed version.
    Msi,
}

impl Package {
    /// The file's name in the release (`.github/release/assemble.sh`).
    pub fn asset(self, version: &Version) -> String {
        match self {
            Package::Dmg => format!("Dromon-{version}-macos-universal.dmg"),
            Package::Msi => format!("Dromon-{version}-windows-x64.msi"),
        }
    }
}

/// Why this copy sends the user to the release page instead of updating itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manual {
    /// The build has no usable release key, so it can't check what it would install.
    NoKey,
    /// The running macOS app isn't signed with a Developer ID (a development build).
    Unsigned,
    /// Dromon can't tell how this copy was installed (run from its build folder, say).
    Unknown,
}

/// How this copy of Dromon takes a newer version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    /// Download the package, check it, put it in place, quit.
    Install(Package),
    /// A package manager's install: the command that updates it, offered to copy.
    Command(String),
    /// Download it from the release page.
    Page(Manual),
}

/// How this copy was installed, as far as updating goes (`service/update_platform.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// A macOS app bundle outside Homebrew; `team` is the Developer ID team it's signed by,
    /// `None` when it has none (ad-hoc signed development builds).
    MacApp { team: Option<String> },
    /// A macOS app Homebrew's cask installed.
    MacHomebrew,
    /// Windows, installed by the MSI.
    WindowsMsi,
    /// Linux, from the `.deb`.
    Deb,
    /// Linux, from the Arch package.
    Arch,
    /// Anything else.
    Other,
}

/// How an install of kind `install` takes `version`; `key`: the build has a usable release key.
pub fn method(install: &Install, key: bool, version: &Version) -> Method {
    let install_with = |package| if key { Method::Install(package) } else { Method::Page(Manual::NoKey) };
    match install {
        Install::MacApp { team: Some(_) } => install_with(Package::Dmg),
        Install::MacApp { team: None } => Method::Page(Manual::Unsigned),
        Install::MacHomebrew => Method::Command(BREW_UPGRADE.to_owned()),
        Install::WindowsMsi => install_with(Package::Msi),
        Install::Deb => Method::Command(format!("sudo apt install ./Dromon-{version}-linux-x64.deb")),
        Install::Arch => Method::Command(format!("sudo pacman -U ./Dromon-{version}-linux-x64.pkg.tar.zst")),
        Install::Other => Method::Page(Manual::Unknown),
    }
}

/// A newer release, offered to this copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub version: Version,
    /// Its release page (GitHub's, built by the service, never taken from the response).
    pub page: String,
    pub method: Method,
}

/// What the UI asks of the service's updater.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateCommand {
    /// Settings' "Check for updates automatically": at launch (the settings restored) and when
    /// it changes. On, the service checks now if it hasn't today, then once a day.
    Auto(bool),
    /// Check now (Settings, the app menu), automatic checks on or off.
    Check,
    /// Download, check and install the offer the last check found. Answered with `Installing`,
    /// then `Installed` (to the sending window) or `InstallFailed`.
    Install,
}

/// What the service's updater tells every window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateEvent {
    /// A check started.
    Checking,
    /// The latest release isn't newer than this one.
    UpToDate,
    /// A newer release.
    Available(Offer),
    /// The check couldn't reach GitHub or read its answer: why.
    CheckFailed(String),
    /// The offer is downloading, being checked or put in place.
    Installing,
    /// It stopped and changed nothing: why.
    InstallFailed(String),
    /// The new version is in place (macOS) or its installer runs (Windows): quit, starting the
    /// app again when `relaunch`.
    Installed { relaunch: bool },
}

impl UpdateEvent {
    /// The event's name, for `DROMON_TRACE` (never its versions or errors).
    pub fn name(&self) -> &'static str {
        match self {
            UpdateEvent::Checking => "Checking",
            UpdateEvent::UpToDate => "UpToDate",
            UpdateEvent::Available(_) => "Available",
            UpdateEvent::CheckFailed(_) => "CheckFailed",
            UpdateEvent::Installing => "Installing",
            UpdateEvent::InstallFailed(_) => "InstallFailed",
            UpdateEvent::Installed { .. } => "Installed",
        }
    }
}

/// Where a window's update stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Nothing checked yet this run.
    #[default]
    Idle,
    Checking,
    UpToDate,
    CheckFailed(String),
    Available(Offer),
    /// Update was pressed while transfers run: the sheet asks.
    Confirm(Offer),
    Installing(Offer),
    InstallFailed {
        offer: Offer,
        error: String,
    },
    /// Installed: the app is quitting.
    Quitting,
}

impl Phase {
    /// The offer it's about, if any.
    pub fn offer(&self) -> Option<&Offer> {
        match self {
            Phase::Available(o) | Phase::Confirm(o) | Phase::Installing(o) | Phase::InstallFailed { offer: o, .. } => {
                Some(o)
            }
            _ => None,
        }
    }

    /// Installing, asking to, or quitting for it: no check starts and no other news replaces it.
    fn busy(&self) -> bool {
        matches!(self, Phase::Confirm(_) | Phase::Installing(_) | Phase::Quitting)
    }
}

/// A window's update state.
#[derive(Clone, Debug, Default)]
pub struct Update {
    pub phase: Phase,
    /// Later was pressed: the toast stays away until the release is found again.
    hidden: bool,
    /// What the service's news asked of the view (quitting); [`App::take_update_effects`].
    pending: Vec<Effect>,
}

/// What moves a window's [`Update`]: the service's news, or the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Event(UpdateEvent),
    CheckNow,
    /// Update, with `transfers` running.
    Update {
        transfers: usize,
    },
    /// The sheet's Update anyway.
    Confirm,
    /// The sheet's Cancel (or Escape).
    Cancel,
    Later,
    /// Release notes.
    Notes,
    /// Copy command.
    Copy,
}

/// What a step asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Send(UpdateCommand),
    Open(String),
    Copy(String),
    Quit { relaunch: bool },
}

/// The machine: `input` moves `u`; returns what to do.
pub fn step(u: &mut Update, input: Input) -> Vec<Out> {
    match input {
        Input::Event(ev) => return event(u, ev),
        Input::CheckNow if !u.phase.busy() => return vec![Out::Send(UpdateCommand::Check)],
        Input::Update { transfers } => {
            let Some(offer) = installable(&u.phase) else { return Vec::new() };
            if transfers > 0 {
                u.phase = Phase::Confirm(offer);
                return Vec::new();
            }
            u.phase = Phase::Installing(offer);
            return vec![Out::Send(UpdateCommand::Install)];
        }
        Input::Confirm => {
            if let Phase::Confirm(offer) = &u.phase {
                u.phase = Phase::Installing(offer.clone());
                return vec![Out::Send(UpdateCommand::Install)];
            }
        }
        Input::Cancel => {
            if let Phase::Confirm(offer) = &u.phase {
                u.phase = Phase::Available(offer.clone());
            }
        }
        Input::Later => u.hidden = true,
        Input::Notes => return u.phase.offer().map(|o| Out::Open(o.page.clone())).into_iter().collect(),
        Input::Copy => {
            if let Some(Offer { method: Method::Command(command), .. }) = u.phase.offer() {
                return vec![Out::Copy(command.clone())];
            }
        }
        Input::CheckNow => {}
    }
    Vec::new()
}

/// The offer Update installs: one found or one that failed, which this copy installs itself.
fn installable(phase: &Phase) -> Option<Offer> {
    match phase {
        Phase::Available(o) | Phase::InstallFailed { offer: o, .. } if matches!(o.method, Method::Install(_)) => {
            Some(o.clone())
        }
        _ => None,
    }
}

fn event(u: &mut Update, ev: UpdateEvent) -> Vec<Out> {
    match ev {
        UpdateEvent::Installed { relaunch } => {
            u.phase = Phase::Quitting;
            return vec![Out::Quit { relaunch }];
        }
        // About the offer this window has (every window heard the same `Available`).
        UpdateEvent::Installing if u.phase != Phase::Quitting => {
            if let Some(offer) = u.phase.offer().cloned() {
                u.phase = Phase::Installing(offer);
            }
        }
        UpdateEvent::InstallFailed(error) if u.phase != Phase::Quitting => {
            if let Some(offer) = u.phase.offer().cloned() {
                u.phase = Phase::InstallFailed { offer, error };
                u.hidden = false;
            }
        }
        _ if u.phase.busy() => {}
        UpdateEvent::Checking => u.phase = Phase::Checking,
        UpdateEvent::Available(offer) => {
            u.phase = Phase::Available(offer);
            u.hidden = false;
        }
        UpdateEvent::UpToDate if u.phase == Phase::Checking => u.phase = Phase::UpToDate,
        UpdateEvent::CheckFailed(why) if u.phase == Phase::Checking => u.phase = Phase::CheckFailed(why),
        _ => {}
    }
    Vec::new()
}

/// A button of the update toast or the Settings page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Update,
    Copy,
    Later,
    Notes,
}

impl Button {
    pub fn label(self) -> &'static str {
        match self {
            Button::Update => "Update",
            Button::Copy => "Copy command",
            Button::Later => "Later",
            Button::Notes => "Release notes",
        }
    }

    /// The button's key, after the key of what it's in (`toast-update`, `settings`).
    pub fn key(self, within: &str) -> String {
        let name = match self {
            Button::Update => "install",
            Button::Copy => "copy",
            Button::Later => "later",
            Button::Notes => "notes",
        };
        format!("{within}-{name}")
    }

    pub fn action(self) -> UpdateAction {
        match self {
            Button::Update => UpdateAction::Update,
            Button::Copy => UpdateAction::Copy,
            Button::Later => UpdateAction::Later,
            Button::Notes => UpdateAction::Notes,
        }
    }
}

/// The update toast as the view draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateToast {
    pub text: String,
    pub tone: Tone,
    pub buttons: Vec<Button>,
}

/// "Dromon 0.3.0 is available", and what this copy does about it.
fn available(offer: &Offer) -> String {
    let v = &offer.version;
    match &offer.method {
        Method::Command(command) => format!("Dromon {v} is available. Update with {command}"),
        _ => format!("Dromon {v} is available"),
    }
}

/// The button that takes the offer: Update, Copy command, or none (the release page).
fn take(offer: &Offer) -> Option<Button> {
    match offer.method {
        Method::Install(_) => Some(Button::Update),
        Method::Command(_) => Some(Button::Copy),
        Method::Page(_) => None,
    }
}

impl Update {
    /// The toast, while there's one: an offer (until Later), an install under way, or one that
    /// failed (until dismissed). None while the sheet asks.
    pub fn toast(&self) -> Option<UpdateToast> {
        match &self.phase {
            Phase::Available(offer) if !self.hidden => {
                let buttons = take(offer).into_iter().chain([Button::Later, Button::Notes]).collect();
                Some(UpdateToast { text: available(offer), tone: Tone::Note, buttons })
            }
            Phase::Installing(offer) => Some(UpdateToast {
                text: format!("Updating to Dromon {}…", offer.version),
                tone: Tone::Busy,
                buttons: Vec::new(),
            }),
            Phase::InstallFailed { offer, error } if !self.hidden => Some(UpdateToast {
                text: format!("Couldn't update to Dromon {}: {error}", offer.version),
                tone: Tone::Error,
                buttons: vec![Button::Update, Button::Later, Button::Notes],
            }),
            _ => None,
        }
    }

    /// The Settings page's line about updates.
    pub fn status(&self, running: &Version) -> String {
        match &self.phase {
            Phase::Idle => format!("You have Dromon {running}"),
            Phase::Checking => "Checking for updates…".to_owned(),
            Phase::UpToDate => format!("Dromon {running} is up to date"),
            Phase::CheckFailed(why) => format!("Couldn't check for updates: {why}"),
            Phase::Available(offer) => match offer.method {
                Method::Page(why) => format!("Dromon {} is available. {}", offer.version, manual(why)),
                _ => format!("{}.", available(offer)),
            },
            Phase::Confirm(offer) | Phase::Installing(offer) => format!("Updating to Dromon {}…", offer.version),
            Phase::InstallFailed { offer, error } => format!("Couldn't update to Dromon {}: {error}", offer.version),
            Phase::Quitting => "Restarting…".to_owned(),
        }
    }

    /// The Settings page's buttons beside "Check now": what takes the offer, and its notes.
    pub fn buttons(&self) -> Vec<Button> {
        match &self.phase {
            Phase::Available(offer) | Phase::InstallFailed { offer, .. } => {
                take(offer).into_iter().chain([Button::Notes]).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The sheet "Update anyway?" is up.
    pub fn confirming(&self) -> bool {
        matches!(self.phase, Phase::Confirm(_))
    }
}

/// Why this copy doesn't install the update itself, and what to do instead.
fn manual(why: Manual) -> &'static str {
    match why {
        Manual::NoKey => {
            "This build has no release key to check updates with, so it doesn't install them: \
             download it from the release page."
        }
        Manual::Unsigned => {
            "This build isn't signed by Dromon's developer, so it doesn't update itself: \
             download it from the release page."
        }
        Manual::Unknown => "Download it from the release page.",
    }
}

/// The user's side of updates (`Action::Update`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    CheckNow,
    Update,
    /// The sheet's Update anyway, and its Cancel.
    Confirm,
    Cancel,
    Later,
    Notes,
    Copy,
}

impl App {
    /// An update button, the sheet's or Settings' Check now.
    pub(super) fn update_action(&mut self, action: UpdateAction, fx: &mut Vec<Effect>) {
        let input = match action {
            UpdateAction::CheckNow => Input::CheckNow,
            UpdateAction::Update => Input::Update { transfers: self.transfers.active() },
            UpdateAction::Confirm => Input::Confirm,
            UpdateAction::Cancel => Input::Cancel,
            UpdateAction::Later => Input::Later,
            UpdateAction::Notes => Input::Notes,
            UpdateAction::Copy => Input::Copy,
        };
        let out = step(&mut self.update, input);
        fx.extend(self.carry_out(out));
    }

    /// The service's update news.
    pub(super) fn update_event(&mut self, ev: UpdateEvent) {
        let out = step(&mut self.update, Input::Event(ev));
        let fx = self.carry_out(out);
        self.update.pending.extend(fx);
    }

    /// Sends what goes to the service; returns what the view does.
    fn carry_out(&mut self, out: Vec<Out>) -> Vec<Effect> {
        let mut fx = Vec::new();
        for o in out {
            match o {
                Out::Send(cmd) => self.send(Command::Update(cmd)),
                Out::Open(url) => fx.push(Effect::OpenUrl(url)),
                Out::Copy(text) => fx.push(Effect::Copy(text)),
                Out::Quit { relaunch } => fx.push(Effect::Quit { relaunch }),
            }
        }
        fx
    }

    /// What the service's update news asked of the view since it last looked (`RuneSink`
    /// carries it out after each event): quitting once the update is in place.
    pub fn take_update_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.update.pending)
    }

    /// Tells the service whether to check by itself (the settings were restored, or changed).
    pub(super) fn auto_updates(&mut self, on: bool) {
        self.send(Command::Update(UpdateCommand::Auto(on)));
    }
}

//! The fixture backend for several main windows (⌘N), served as the service serves them: one
//! [`Core`] (connections, folder trees, transfers, saved hosts) and a [`FixtureBackend`] per
//! window, which says which window each command comes from.
//!
//! A command's events go to the window that sent it at once (the answer of `Backend::send`),
//! and to every other window through its [`Backend::pending`] list (calling the wake, so it
//! catches up): hosts, statuses, log lines, listings, operations, transfers. Two kinds go to
//! one window only, as the service sends them: a prompt (to the focused window, else the
//! sender), and `Opened` for a host that was up already for another window (to the sender).
//! A window that closes hands its open prompts on.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use dromon_core::Prompt;
use dromon_sftp::Visibility;

use super::Fixture;
use super::backend::Core;
use crate::state::{Backend, Command, ServiceEvent, WindowId};

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;

/// One main window's end of a fixture.
pub struct FixtureBackend {
    shared: Rc<RefCell<Shared>>,
    window: WindowId,
}

struct Shared {
    core: Core,
    /// Events waiting for each window but the one that caused them.
    pending: BTreeMap<WindowId, Vec<ServiceEvent>>,
    /// The prompts on screen, and the window each is in.
    prompts: Vec<(Prompt, WindowId)>,
    wake: Option<Rc<dyn Fn(WindowId)>>,
}

impl FixtureBackend {
    /// The first window's end of a fresh fixture.
    pub fn new(fixture: Arc<Fixture>) -> Self {
        let shared = Shared { core: Core::new(fixture), pending: BTreeMap::new(), prompts: Vec::new(), wake: None };
        FixtureBackend { shared: Rc::new(RefCell::new(shared)), window: WindowId::FIRST }
    }
}

impl Backend for FixtureBackend {
    fn send(&mut self, cmd: Command) -> Vec<ServiceEvent> {
        let (mine, woken, wake) = {
            let mut s = self.shared.borrow_mut();
            let (mine, woken) = s.send(self.window, cmd);
            (mine, woken, s.wake.clone())
        };
        if let Some(wake) = wake {
            for w in woken {
                wake(w);
            }
        }
        mine
    }

    fn another(&mut self, window: WindowId) -> Option<Box<dyn Backend>> {
        self.shared.borrow_mut().core.windows.entry(window).or_insert(Visibility::Hidden);
        Some(Box::new(FixtureBackend { shared: self.shared.clone(), window }))
    }

    fn pending(&mut self) -> Vec<ServiceEvent> {
        self.shared.borrow_mut().pending.remove(&self.window).unwrap_or_default()
    }

    fn set_wake(&mut self, wake: Box<dyn Fn(WindowId)>) {
        self.shared.borrow_mut().wake = Some(Rc::from(wake));
    }
}

impl Shared {
    /// Runs `cmd` from `from`: the events for `from`, and the other windows that got some.
    fn send(&mut self, from: WindowId, cmd: Command) -> (Vec<ServiceEvent>, BTreeSet<WindowId>) {
        let joins = matches!(&cmd, Command::Connect { host } if self.core.up(host));
        let closing = cmd == Command::WindowClosed;
        if let Command::AnswerPrompt { id, .. } = &cmd {
            self.prompts.retain(|(p, _)| p.id != *id);
        }
        self.core.from = from;
        let events = self.core.run(cmd);
        let windows: Vec<WindowId> = self.core.windows.keys().copied().collect();
        let focused = self.core.windows.iter().find(|(_, v)| **v == Visibility::Focused).map(|(w, _)| *w);
        let (mut mine, mut woken) = (Vec::new(), BTreeSet::new());
        let mut to = |s: &mut Shared, w: WindowId, ev: ServiceEvent| {
            if w == from {
                mine.push(ev);
            } else {
                s.pending.entry(w).or_default().push(ev);
                woken.insert(w);
            }
        };
        for ev in events {
            match &ev {
                ServiceEvent::Prompt(p) => {
                    let at = focused.unwrap_or(from);
                    self.prompts.push((p.clone(), at));
                    to(self, at, ev);
                }
                ServiceEvent::Opened { .. } if joins => to(self, from, ev),
                _ => {
                    if let ServiceEvent::PromptClosed(id) = &ev {
                        self.prompts.retain(|(p, _)| p.id != *id);
                    }
                    for w in windows.iter().copied().chain((!windows.contains(&from)).then_some(from)) {
                        to(self, w, ev.clone());
                    }
                }
            }
        }
        if closing {
            self.pending.remove(&from);
            let next = focused.filter(|w| *w != from).or_else(|| windows.first().copied());
            if let Some(next) = next {
                let moved: Vec<Prompt> = self.prompts.iter_mut().filter(|(_, at)| *at == from).map(|(p, at)| {
                    *at = next;
                    p.clone()
                }).collect();
                for p in moved {
                    to(self, next, ServiceEvent::Prompt(p));
                }
            }
        }
        (mine, woken)
    }
}

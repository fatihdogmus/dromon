//! A connection task's live updates (`live.rs`): it steps the machine after anything that may
//! change what to watch, carries out its effects, answers the watch's sign-in from memory, tells
//! the windows (and the poller, which stretches the watched folder's polls), and starts a poll of
//! the watched folder once its changes have settled.

use dromon_core::RemotePath;
use tokio::time::Instant;

use super::super::Sink;
use super::super::lifecycle::Phase;
use super::super::live::{self, Effect, Event, Heard, Watcher};
use super::Task;
use crate::state::ServiceEvent;

impl<S: Sink> Task<S> {
    /// The folder to watch: the windows' (`Watch::live_folder`), while the session is up and the
    /// setting is on.
    fn live_target(&self) -> Option<RemotePath> {
        let up = self.live.on && self.life.phase == Phase::Ready && self.sftp().is_some();
        if up { self.watch.live_folder() } else { None }
    }

    /// After anything that may have changed what to watch.
    pub(super) async fn live_sync(&mut self) {
        let target = self.live_target();
        self.live_step(Event::Target(target)).await;
    }

    /// Steps the machine and carries out its effects, then tells the windows what changed.
    pub(super) async fn live_step(&mut self, event: Event) {
        let mut events = vec![event];
        while let Some(event) = events.pop() {
            for fx in live::step(&mut self.live.phase, event) {
                match fx {
                    Effect::Spawn(path) => match Watcher::start(&self.spec, &self.shared.cfg, &path).await {
                        Ok(w) => self.live.watcher = Some(w),
                        Err(e) => {
                            self.log(format!("Live updates couldn't start: {e}"));
                            events.push(Event::Exited);
                        }
                    },
                    Effect::Kill => self.live.watcher = None,
                    Effect::Changed => self.live.relist.changed(Instant::now()),
                    Effect::Log(line) => self.log(line),
                }
            }
        }
        let shown = self.live.phase.shown();
        if shown != self.live.shown {
            self.live.shown = shown.clone();
            let watching = self.live.phase.watching().cloned();
            if watching.is_none() {
                self.live.relist.clear();
            }
            self.watch.set_live(watching, Instant::now());
            self.emit(ServiceEvent::Live { host: self.host.clone(), live: shown });
        }
    }

    /// What the watch heard.
    pub(super) async fn on_live(&mut self, heard: Heard) {
        match heard {
            Heard::Ask(req) => {
                let store = self.shared.store.clone();
                let answered = self.live.watcher.as_mut().is_some_and(|w| w.answer(req, &store));
                if !answered {
                    self.live_step(Event::CantSignIn).await;
                }
            }
            Heard::Event(Event::Exited) => {
                // Its output ended: nothing more comes from it.
                self.live.watcher = None;
                self.live_step(Event::Exited).await;
            }
            Heard::Event(event) => self.live_step(event).await,
        }
    }

    /// The watched folder, if its changes' poll is due at `now` and it may be polled (listed,
    /// small enough).
    pub(super) fn live_poll_due(&mut self, now: Instant) -> Option<RemotePath> {
        if !self.live.relist.take(now) {
            return None;
        }
        self.live.phase.watching().filter(|p| self.watch.may_poll(p)).cloned()
    }
}

//! The server matrix (`.plans/dromon-plan.md` §9, M5): the same compatibility checks against
//! several real servers. A test is one check; [`run`] runs it against every [`Variant`] of
//! [`variants`] at once (one fixture per variant and test process, see
//! [`fixture_of`](crate::fixture_of)) and prints one line per variant:
//!
//! ```text
//! matrix: sftp capabilities            bullseye  ok      0.3 s  SSH-2.0-OpenSSH_8.4p1 …
//! matrix: sftp listing 100k            trixie    skip    0.0 s  the matrix only (DROMON_SERVER_MATRIX=1)
//! ```
//!
//! A check returns an [`Outcome`] (ok with a note, or skip with the reason) and fails by
//! panicking (assertions). Every variant runs to the end; then the test fails if any variant did,
//! naming them.
//!
//! Without `DROMON_SERVER_MATRIX=1` ([`MATRIX_ENV`]) only [`Variant::Trixie`] runs, the server
//! the rest of the suite uses, so a plain `cargo test` starts no other containers. With the
//! native backend (`DROMON_FIXTURE_BACKEND=native`) the matrix is skipped: its servers are
//! Docker images.

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use futures_util::FutureExt as _;

use crate::{BACKEND_ENV, Fixture, env_flag, native_backend, note, try_fixture};

/// Set to `1` to run the checks against every [`Variant`], not just [`Variant::Trixie`].
pub const MATRIX_ENV: &str = "DROMON_SERVER_MATRIX";

/// A server of the matrix. All run on the Docker backend with the users, keys and seed data of
/// `tests/docker/` (see `tests/docker/README.md`); what one lacks is a
/// [`Feature`](crate::Feature).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variant {
    /// Debian trixie's OpenSSH 10 with PAM (`tests/docker/Dockerfile`): the default server.
    Trixie,
    /// Debian bullseye's OpenSSH 8.4p1 (`tests/docker/bullseye/`): no `copy-data`, no
    /// `limits@openssh.com`.
    Bullseye,
    /// Alpine's OpenSSH on musl, without PAM (`tests/docker/alpine/`).
    Alpine,
    /// The trixie image behind toxiproxy (`tests/docker/toxiproxy/`): a slow, choppy link
    /// ([`BASE_TOXICS`](crate::toxiproxy::BASE_TOXICS)) that tests can cut.
    Toxic,
}

impl Variant {
    pub const ALL: [Variant; 4] = [Variant::Trixie, Variant::Bullseye, Variant::Alpine, Variant::Toxic];

    /// `trixie`, `bullseye`, `alpine`, `toxic`.
    pub fn name(self) -> &'static str {
        match self {
            Variant::Trixie => "trixie",
            Variant::Bullseye => "bullseye",
            Variant::Alpine => "alpine",
            Variant::Toxic => "toxic",
        }
    }

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

/// Whether `DROMON_SERVER_MATRIX=1`.
pub fn enabled() -> bool {
    env_flag(MATRIX_ENV)
}

/// The variants [`run`] uses: all of them with `DROMON_SERVER_MATRIX=1`, else [`Variant::Trixie`].
pub fn variants() -> Vec<Variant> {
    if enabled() { Variant::ALL.to_vec() } else { vec![Variant::Trixie] }
}

/// How a check ended on one variant (failures are panics).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Passed; the note says how (`copy-data`, `streamed`, timings), or is empty.
    Ok(String),
    /// Didn't apply to this server; the reason.
    Skip(String),
}

impl Outcome {
    pub fn ok() -> Outcome {
        Outcome::Ok(String::new())
    }

    /// Skips with the reason when `fixture` lacks `feature`.
    pub fn unless(fixture: &Fixture, feature: crate::Feature) -> Option<Outcome> {
        fixture.lacks(feature).map(Outcome::Skip)
    }

    /// Skips unless `DROMON_SERVER_MATRIX=1` (checks too slow for every `cargo test`).
    pub fn matrix_only() -> Option<Outcome> {
        (!enabled()).then(|| Outcome::Skip(format!("the matrix only ({MATRIX_ENV}=1)")))
    }
}

/// Runs `check` against every variant of [`variants`] at once, each in a task of its own while
/// holding its fixture shared (see [`run_exclusive`]). Prints one line per variant and fails
/// after all ran if any failed.
pub async fn run<F, Fut>(name: &str, check: F)
where
    F: Fn(&'static Fixture) -> Fut,
    Fut: Future<Output = Outcome> + Send + 'static,
{
    run_with(name, false, check).await;
}

/// [`run`], holding each fixture exclusively: no other check of this test process uses that
/// server meanwhile (for toxics that cut every connection, [`crate::Toxiproxy::reset_peers`]).
pub async fn run_exclusive<F, Fut>(name: &str, check: F)
where
    F: Fn(&'static Fixture) -> Fut,
    Fut: Future<Output = Outcome> + Send + 'static,
{
    run_with(name, true, check).await;
}

/// One variant's result: the outcome, or the panic message; and how long the check ran.
type Ran = (Result<Outcome, String>, Duration);

async fn run_with<F, Fut>(name: &str, exclusive: bool, check: F)
where
    F: Fn(&'static Fixture) -> Fut,
    Fut: Future<Output = Outcome> + Send + 'static,
{
    if native_backend() {
        note(format_args!("skipping {name}: the server matrix runs on the Docker backend ({BACKEND_ENV}=native)"));
        return;
    }
    let variants = variants();
    // Start (or wait for) every variant's fixture at once; a fixture that doesn't start fails
    // that variant only.
    let starts = variants.iter().map(|&v| {
        let label = format!("{name} [{}]", v.name());
        async move { AssertUnwindSafe(try_fixture(v, &label)).catch_unwind().await }
    });
    let fixtures = futures_util::future::join_all(starts).await;

    let mut tasks = Vec::new();
    for started in fixtures {
        let task = match started {
            Ok(Ok(fx)) => {
                let run = check(fx);
                Some(tokio::spawn(async move {
                    let _shared;
                    let _exclusive;
                    if exclusive {
                        _exclusive = fx.access.write().await;
                    } else {
                        _shared = fx.access.read().await;
                    }
                    let start = Instant::now();
                    let outcome = run.await;
                    (outcome, start.elapsed())
                }))
            }
            Ok(Err(skipped)) => {
                tasks.push(Err((Ok(Outcome::Skip(skipped.0)), Duration::ZERO)));
                None
            }
            Err(panic) => {
                tasks.push(Err((Err(panic_message(&*panic)), Duration::ZERO)));
                None
            }
        };
        if let Some(task) = task {
            tasks.push(Ok(task));
        }
    }

    let mut results: Vec<Ran> = Vec::new();
    for task in tasks {
        results.push(match task {
            Ok(handle) => match handle.await {
                Ok((outcome, took)) => (Ok(outcome), took),
                Err(e) if e.is_panic() => (Err(panic_message(&*e.into_panic())), Duration::ZERO),
                Err(e) => (Err(format!("the check's task ended: {e}")), Duration::ZERO),
            },
            Err(done) => done,
        });
    }

    let mut failed = Vec::new();
    for (variant, (result, took)) in variants.iter().zip(&results) {
        let (word, text) = match result {
            Ok(Outcome::Ok(note)) => ("ok", note.clone()),
            Ok(Outcome::Skip(why)) => ("skip", why.clone()),
            Err(message) => {
                failed.push(variant.name());
                ("FAIL", message.clone())
            }
        };
        note(format_args!(
            "matrix: {name:<34} {:<9} {word:<5} {:>6.1} s  {text}",
            variant.name(),
            took.as_secs_f64()
        ));
    }
    if !failed.is_empty() {
        panic!("{name} failed on {} (see the matrix lines above)", failed.join(", "));
    }
}

/// A panic's message on one line (a check's assertion), shortened.
fn panic_message(panic: &(dyn Any + Send)) -> String {
    let text = if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic without a message".to_owned()
    };
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 400 { format!("{}…", flat.chars().take(400).collect::<String>()) } else { flat }
}

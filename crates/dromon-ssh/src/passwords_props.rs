//! Properties of the password store, after any sequence of remembering and forgetting: it holds
//! exactly what a plain map would, and neither its `Debug`, which shows only counts, nor that of a
//! secret it hands out ever shows a secret.

use std::collections::HashMap;

use hegel::TestCase;
use hegel::generators as gs;
use secrecy::ExposeSecret;

use super::*;

/// What every secret here starts with, so a check that it doesn't show means something.
const MARKER: &str = "s3cr3t·";

/// A few hosts, so steps meet the same one; one named like a secret.
fn hosts() -> impl hegel::PrintableGenerator<String> {
    gs::sampled_from(vec!["prod".to_owned(), "box".to_owned(), format!("{MARKER}host")])
}

/// A few identities and key paths, so steps meet the same one.
fn names() -> impl hegel::PrintableGenerator<String> {
    gs::sampled_from(vec!["deploy@10.0.0.4".to_owned(), "jump@bastion".to_owned(), "/k/id".to_owned()])
}

#[hegel::composite]
fn secrets(tc: &TestCase) -> String {
    format!("{MARKER}{}", tc.draw(gs::text()))
}

/// The store and a plain model of it, side by side.
struct Store {
    store: PasswordStore,
    passwords: HashMap<(String, String), String>,
    passphrases: HashMap<(String, Option<String>), String>,
}

#[hegel::state_machine]
impl Store {
    #[rule]
    fn set(&mut self, tc: TestCase) {
        let host = tc.draw(hosts());
        let identity = tc.draw(names());
        let secret = tc.draw(secrets());
        self.store.set(HostId(host.clone()), identity.clone(), secret.clone().into());
        self.passwords.insert((host, identity), secret);
    }

    #[rule]
    fn set_passphrase(&mut self, tc: TestCase) {
        let host = tc.draw(hosts());
        let key = tc.draw(gs::optional(names()));
        let secret = tc.draw(secrets());
        self.store.set_passphrase(HostId(host.clone()), key.clone(), secret.clone().into());
        self.passphrases.insert((host, key), secret);
    }

    #[rule]
    fn clear_password(&mut self, tc: TestCase) {
        let host = tc.draw(hosts());
        let identity = tc.draw(names());
        self.store.clear_password(&HostId(host.clone()), &identity);
        self.passwords.remove(&(host, identity));
    }

    #[rule]
    fn clear_passphrase(&mut self, tc: TestCase) {
        let host = tc.draw(hosts());
        let key = tc.draw(gs::optional(names()));
        self.store.clear_passphrase(&HostId(host.clone()), key.as_deref());
        self.passphrases.remove(&(host, key));
    }

    #[rule]
    fn clear_host(&mut self, tc: TestCase) {
        let host = tc.draw(hosts());
        self.store.clear(&HostId(host.clone()));
        self.passwords.retain(|(h, _), _| *h != host);
        self.passphrases.retain(|(h, _), _| *h != host);
    }

    #[rule]
    fn clear_all(&mut self, _: TestCase) {
        self.store.clear_all();
        self.passwords.clear();
        self.passphrases.clear();
    }

    #[invariant(always_run)]
    fn the_store_holds_what_the_model_does_and_shows_no_secret(&self, _: TestCase) {
        let debug = format!("{:?}", self.store);
        let counts = (self.passwords.len(), self.passphrases.len());
        assert_eq!(debug, format!("PasswordStore {{ passwords: {}, passphrases: {} }}", counts.0, counts.1));
        assert_eq!(self.store.is_empty(), counts == (0, 0));
        for ((host, identity), secret) in &self.passwords {
            let got = self.store.get(&HostId(host.clone()), identity).expect("remembered");
            assert_eq!(got.expose_secret(), secret);
            assert!(!format!("{got:?}").contains(MARKER), "{got:?}");
        }
        for ((host, key), secret) in &self.passphrases {
            let got = self.store.passphrase(&HostId(host.clone()), key.as_deref()).expect("remembered");
            assert_eq!(got.expose_secret(), secret);
            assert!(!format!("{got:?}").contains(MARKER), "{got:?}");
        }
    }
}

#[hegel::test]
fn the_store_remembers_like_a_map_and_never_shows_a_secret(tc: TestCase) {
    let store = Store { store: PasswordStore::new(), passwords: HashMap::new(), passphrases: HashMap::new() };
    hegel::stateful::machine(store).run(tc);
}

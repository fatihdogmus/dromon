//! Properties of the report's link, for any report: encoding then decoding gives back any text,
//! and leaves only unreserved characters and escapes; the link keeps within its budget whenever
//! the report without its events does, leaving out the fewest of the oldest events; and a
//! recording's blocks go in the first of the three tiers that fits (both in their fields, the
//! fixture copied, both copied), what the link leaves out being what Open GitHub copies.

use hegel::TestCase;
use hegel::generators as gs;

use super::*;

/// `%XX` back to text, as GitHub fills the field; `None` for what isn't an encoding of text.
fn decode(s: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

/// The link's field `id`, decoded.
fn field_of(url: &str, id: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|kv| kv.strip_prefix(&format!("{id}="))).and_then(decode)
}

/// Report text: plain words, unicode, newlines, and characters a query must escape.
#[hegel::composite]
fn line(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::from_regex("[A-Za-z0-9 =&%+#?/-]{0,40}"),
        gs::text().max_size(40),
        gs::text().min_size(150).max_size(400),
    ))
}

#[hegel::composite]
fn diagnostics(tc: &TestCase) -> Diagnostics {
    let head = tc.draw(gs::vecs(line()).max_size(12));
    let counts = gs::integers::<u32>().min_value(1).max_value(10_000);
    let events = tc.draw(gs::vecs(hegel::tuples!(line(), counts)).max_size(EVENTS));
    let steps = tc.draw(gs::vecs(line()).max_size(8));
    let events = events.into_iter().map(|(text, events)| Line { text, events }).collect();
    Diagnostics { head, events, steps }
}

#[hegel::composite]
fn budget(tc: &TestCase) -> usize {
    tc.draw(hegel::one_of!(gs::just(URL_BUDGET), gs::integers::<usize>().max_value(20_000)))
}

#[hegel::test]
fn encoding_then_decoding_gives_back_any_text(tc: TestCase) {
    let text = tc.draw(gs::text());
    let encoded = encode(&text);
    assert_eq!(decode(&encoded).as_deref(), Some(text.as_str()));
    let unreserved = |c: char| c.is_ascii_alphanumeric() || "-_.~".contains(c);
    let mut chars = encoded.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            assert!(hex.len() == 2 && hex.chars().all(|h| h.is_ascii_digit() || h.is_ascii_uppercase()), "%{hex}");
        } else {
            assert!(unreserved(c), "{c:?} in {encoded:?}");
        }
    }
}

#[hegel::test]
fn the_link_leaves_out_the_fewest_oldest_events_that_fit_its_budget(tc: TestCase) {
    let d = tc.draw(diagnostics());
    let budget = tc.draw(budget());
    let link = d.link(budget, None);
    let all = d.events.len();
    let fits = |left_out: usize| issue_url(&d.render(left_out)).len() <= budget;
    if fits(all) {
        assert!(link.url.len() <= budget, "{} characters over {budget}", link.url.len());
        assert!(link.left_out == 0 || !fits(link.left_out - 1), "left out {} when fewer fit", link.left_out);
    } else {
        assert_eq!(link.left_out, all, "even the head doesn't fit: every event goes");
    }
    assert_eq!(link.copies, None);
    let sent = field_of(&link.url, DIAGNOSTICS).expect("the diagnostics field");
    assert_eq!(sent, d.render(link.left_out));
    let kept: String = d.events[link.left_out..].iter().map(|l| format!("\n  {}", l.text)).collect();
    assert!(sent.ends_with(&kept), "the newest events stay");
    let omitted: u32 = d.events[..link.left_out].iter().map(|l| l.events).sum();
    assert_eq!(sent.contains(&format!("\n  ({omitted} older events omitted)")), omitted > 0);
}

#[hegel::test]
fn a_recordings_blocks_go_in_the_first_tier_that_fits(tc: TestCase) {
    let d = tc.draw(diagnostics());
    let budget = tc.draw(budget());
    let fixture = format!("```json fixture\n{}\n```", tc.draw(gs::text().max_size(3_000)));
    let script = format!("```json script\n{}\n```", tc.draw(gs::text().max_size(3_000)));
    let link = d.link(budget, Some((&fixture, &script)));
    let both = format!("{fixture}\n\n{script}");
    let tiers: [(&str, &str, Option<&str>); 3] =
        [(&fixture, &script, None), (PASTE_FIXTURE, &script, Some(&fixture)), (PASTE_BOTH, PASTED_WITH, Some(&both))];
    let url = |f: &str, s: &str, left_out: usize| {
        issue_url_with(&[(DIAGNOSTICS, &d.render(left_out)), (FIXTURE, f), (SCRIPT, s)])
    };
    let all = d.events.len();
    let tier = tiers.iter().position(|(f, s, _)| url(f, s, all).len() <= budget);
    let (f, s, copies) = tiers[tier.unwrap_or(2)];
    tc.note(&format!("tier {tier:?}, left out {}", link.left_out));
    assert_eq!(link.copies.as_deref(), copies, "what Open GitHub copies");
    assert_eq!(field_of(&link.url, FIXTURE).as_deref(), Some(f));
    assert_eq!(field_of(&link.url, SCRIPT).as_deref(), Some(s));
    match tier {
        Some(_) => {
            assert!(link.url.len() <= budget, "{} characters over {budget}", link.url.len());
            assert!(link.left_out == 0 || url(f, s, link.left_out - 1).len() > budget, "fewest events left out");
        }
        None => assert_eq!(link.left_out, all),
    }
    assert_eq!(link.url, url(f, s, link.left_out));
}

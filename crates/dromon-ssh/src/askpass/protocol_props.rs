//! Properties of the askpass wire format. Every request and reply reads back as itself from the
//! one line it's encoded as, or is refused for being over the size limit. Decoding any bytes
//! never panics, refuses anything over the limit and never quotes what it read in an error.
//! Reading messages gives each line in turn. No `Debug` shows a token or an answer.

use std::io::Cursor;

use hegel::TestCase;
use hegel::generators::{self as gs, Generator};

use super::*;

/// What every secret here starts with, so a check that it doesn't show means something.
const MARKER: &str = "s3cr3t·";

/// Any text, or text made of what JSON escapes and what's awkward in a line.
#[hegel::composite]
fn texts(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(gs::text(), gs::text().alphabet("\"\\\n\r\t\0\u{1}\u{7f}\u{2028}{}[]:,é🚀 ")))
}

/// [`texts`], now and then padded to about the size limit, either side of it.
#[hegel::composite]
fn long_texts(tc: &TestCase) -> String {
    let text = tc.draw(texts());
    if !tc.draw(gs::weighted_booleans(0.2)) {
        return text;
    }
    let pad = tc.draw(gs::integers::<usize>().min_value(MAX_MESSAGE_BYTES - 64).max_value(MAX_MESSAGE_BYTES + 16));
    format!("{text}{}", "x".repeat(pad))
}

/// A secret: [`MARKER`], then any text.
#[hegel::composite]
fn secrets(tc: &TestCase) -> String {
    format!("{MARKER}{}", tc.draw(long_texts()))
}

/// Bytes a peer could send: any at all, or JSON-like text with the protocol's keys.
#[hegel::composite]
fn wire_bytes(tc: &TestCase) -> Vec<u8> {
    let json =
        r#"\{?\s*("(answer|cancel|token|prompt|hint)"\s*:\s*(true|false|null|-?[0-9]+|"[^"\\]*"?|\{|\[)?\s*,?\s*)*\}?"#;
    tc.draw(hegel::one_of!(gs::binary(), gs::from_regex(json).map(String::into_bytes)))
}

#[hegel::test]
#[hegel::explicit_test_case(token = MARKER.to_owned(), prompt = "x".repeat(MAX_MESSAGE_BYTES - 44), hint = None::<String>)]
fn every_request_reads_back_as_itself_from_its_one_line(tc: TestCase) {
    let token = tc.draw(secrets());
    let prompt = tc.draw(long_texts());
    let hint = tc.draw(gs::optional(texts()));
    tc.assume(!prompt.contains(MARKER) && !hint.as_deref().is_some_and(|h| h.contains(MARKER)));
    let request = Request { token, prompt, hint };
    assert!(!format!("{request:?}").contains(MARKER), "Debug shows the token");
    let json_len = serde_json::to_vec(&request).unwrap().len();
    let Ok(line) = encode_request(&request) else {
        assert!(json_len > MAX_MESSAGE_BYTES, "refused at {json_len} bytes");
        return;
    };
    assert!(json_len <= MAX_MESSAGE_BYTES);
    assert_eq!(line.iter().position(|b| *b == b'\n'), Some(line.len() - 1), "one line");
    assert_eq!(decode_request(&line).unwrap(), request);
    let read = read_message(&mut Cursor::new(line)).unwrap();
    assert_eq!(decode_request(&read).unwrap(), request);
}

#[hegel::test]
#[hegel::explicit_test_case(answer = Some(format!("{MARKER}{}{}", "\"".repeat(9), "x".repeat(MAX_MESSAGE_BYTES - 39))))]
fn every_reply_reads_back_as_itself_and_never_shows_its_answer(tc: TestCase) {
    let answer = tc.draw(gs::optional(secrets()));
    let json_len =
        serde_json::to_vec(&ReplyOut { answer: answer.as_deref(), cancel: answer.is_none().then_some(true) })
            .unwrap()
            .len();
    let reply = answer.map_or(Reply::Cancel, |a| Reply::Answer(Zeroizing::new(a)));
    assert!(!format!("{reply:?}").contains(MARKER), "Debug shows the answer");
    let Ok(line) = encode_reply(&reply) else {
        assert!(json_len > MAX_MESSAGE_BYTES, "refused at {json_len} bytes");
        return;
    };
    assert!(json_len <= MAX_MESSAGE_BYTES);
    assert_eq!(line.iter().position(|b| *b == b'\n'), Some(line.len() - 1), "one line");
    assert_eq!(decode_reply(&line).unwrap(), reply);
    let read = read_message(&mut Cursor::new(line.to_vec())).unwrap();
    assert_eq!(decode_reply(&read).unwrap(), reply);
}

/// Bytes with a secret somewhere in them never panic a decoder, and neither an error nor a
/// reply's `Debug` shows the secret.
#[hegel::test]
fn decoding_any_bytes_never_panics_or_shows_what_it_read(tc: TestCase) {
    let before = tc.draw(wire_bytes());
    let after = tc.draw(wire_bytes());
    let line = [before, MARKER.as_bytes().to_vec(), after].concat();
    if let Err(e) = decode_request(&line) {
        assert!(!e.to_string().contains(MARKER), "{e}");
    }
    match decode_reply(&line) {
        Err(e) => assert!(!e.to_string().contains(MARKER), "{e}"),
        Ok(reply) => assert!(!format!("{reply:?}").contains(MARKER)),
    }
}

#[hegel::test]
fn decoding_refuses_anything_over_the_size_limit(tc: TestCase) {
    let start = tc.draw(wire_bytes());
    let over = tc.draw(gs::integers::<usize>().min_value(1).max_value(4096));
    let fill = tc.draw(gs::sampled_from(vec![b' ', b'a', b'"', b'\n', b'}']));
    let mut line = start;
    line.resize(MAX_MESSAGE_BYTES + over, fill);
    // The limit is on the message: a last newline doesn't count.
    tc.assume(line.strip_suffix(b"\n").unwrap_or(&line).len() > MAX_MESSAGE_BYTES);
    assert!(decode_request(&line).unwrap_err().to_string().contains("64 KiB"));
    assert!(decode_reply(&line).unwrap_err().to_string().contains("64 KiB"));
}

/// Lines (with or without a last newline) come back one by one, then the end; a line over the
/// limit is refused where it comes.
#[hegel::test]
fn reading_messages_gives_each_line_in_turn_and_refuses_a_long_one(tc: TestCase) {
    let lines = tc.draw(
        gs::vecs(gs::binary().map(|b: Vec<u8>| b.into_iter().filter(|c| *c != b'\n').collect::<Vec<u8>>())).max_size(5),
    );
    let last = tc.draw(gs::sampled_from(vec!["newline", "none", "too long"]));
    let mut stream: Vec<u8> = lines.iter().flat_map(|l| l.iter().copied().chain(std::iter::once(b'\n'))).collect();
    match last {
        "none" => stream.extend(b"end"),
        "too long" => stream.extend(vec![b'a'; MAX_MESSAGE_BYTES + 1]),
        _ => {}
    }
    let mut reader = Cursor::new(stream);
    for line in &lines {
        assert_eq!(&**read_message(&mut reader).unwrap(), &line[..]);
    }
    let end = match last {
        "none" => {
            assert_eq!(&**read_message(&mut reader).unwrap(), b"end");
            read_message(&mut reader).unwrap_err()
        }
        _ => read_message(&mut reader).unwrap_err(),
    };
    let expected = if last == "too long" { io::ErrorKind::InvalidData } else { io::ErrorKind::UnexpectedEof };
    assert_eq!(end.kind(), expected);
}

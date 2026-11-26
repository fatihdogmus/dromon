use std::io::{BufReader, Cursor};

use zeroize::Zeroizing;

use super::protocol::*;

fn req(hint: Option<&str>) -> Request {
    Request { token: "t0k".into(), prompt: "deploy@h's password: ".into(), hint: hint.map(str::to_owned) }
}

#[test]
fn request_wire_format() {
    assert_eq!(
        encode_request(&req(None)).unwrap(),
        b"{\"token\":\"t0k\",\"prompt\":\"deploy@h's password: \",\"hint\":null}\n"
    );
    assert_eq!(
        encode_request(&req(Some("confirm"))).unwrap(),
        b"{\"token\":\"t0k\",\"prompt\":\"deploy@h's password: \",\"hint\":\"confirm\"}\n"
    );
}

#[test]
fn request_round_trips_including_newlines_in_the_prompt() {
    let r = Request { prompt: "line1\nline2\u{1}\"quoted\"".into(), ..req(Some("none")) };
    let line = encode_request(&r).unwrap();
    assert_eq!(line.iter().filter(|b| **b == b'\n').count(), 1, "exactly one line");
    assert_eq!(decode_request(&line).unwrap(), r);
}

#[test]
fn request_hint_may_be_missing() {
    let r = decode_request(br#"{"token":"a","prompt":"p"}"#).unwrap();
    assert_eq!(r.hint, None);
}

#[test]
fn reply_wire_format() {
    let answer = Reply::Answer(Zeroizing::new("hunter2".into()));
    assert_eq!(&**encode_reply(&answer).unwrap(), b"{\"answer\":\"hunter2\"}\n");
    assert_eq!(&**encode_reply(&Reply::Cancel).unwrap(), b"{\"cancel\":true}\n");
}

#[test]
fn reply_round_trips() {
    for reply in [
        Reply::Answer(Zeroizing::new("pä\"ss\nword".into())),
        Reply::Answer(Zeroizing::new(String::new())),
        Reply::Cancel,
    ] {
        let line = encode_reply(&reply).unwrap();
        assert_eq!(decode_reply(&line).unwrap(), reply);
    }
}

#[test]
fn cancel_wins_and_empty_reply_is_rejected() {
    assert_eq!(decode_reply(br#"{"answer":"x","cancel":true}"#).unwrap(), Reply::Cancel);
    assert_eq!(decode_reply(br#"{"answer":"x","cancel":false}"#).unwrap(), Reply::Answer(Zeroizing::new("x".into())));
    assert!(decode_reply(b"{}").is_err());
}

#[test]
fn malformed_messages_are_rejected_without_quoting_content() {
    for bad in [&br#"{"answer":true}"#[..], br#"{"cancel":"hunter2"}"#, b"not json", b"", br#"{"token":1}"#] {
        let err = decode_reply(bad).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(!err.to_string().contains("hunter2"), "{err}");
    }
    assert!(decode_request(br#"{"prompt":"p"}"#).is_err(), "token is required");
}

#[test]
fn size_limit() {
    let at_limit = vec![b'a'; MAX_MESSAGE_BYTES];
    let over = vec![b'a'; MAX_MESSAGE_BYTES + 1];
    assert!(decode_request(&over).unwrap_err().to_string().contains("64 KiB"));
    assert!(decode_reply(&over).is_err());

    let big = Request { prompt: "x".repeat(MAX_MESSAGE_BYTES), ..req(None) };
    assert!(encode_request(&big).is_err());

    let mut line = at_limit.clone();
    line.push(b'\n');
    assert_eq!(&**read_message(&mut Cursor::new(line)).unwrap(), &at_limit[..]);

    let mut line = over.clone();
    line.push(b'\n');
    assert!(read_message(&mut Cursor::new(line)).is_err());
    assert!(read_message(&mut Cursor::new(over)).is_err());
}

#[test]
fn read_message_stops_at_the_first_newline() {
    let mut r = BufReader::new(Cursor::new(b"first\nsecond\n".to_vec()));
    assert_eq!(&**read_message(&mut r).unwrap(), b"first");
    assert_eq!(&**read_message(&mut r).unwrap(), b"second");
    assert_eq!(read_message(&mut r).unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn read_message_accepts_eof_terminated_line() {
    assert_eq!(&**read_message(&mut Cursor::new(b"abc".to_vec())).unwrap(), b"abc");
}

#[tokio::test]
async fn async_reader_matches_sync() {
    let mut r = tokio::io::BufReader::new(&b"one\ntwo"[..]);
    assert_eq!(&**read_message_async(&mut r).await.unwrap(), b"one");
    assert_eq!(&**read_message_async(&mut r).await.unwrap(), b"two");
    assert!(read_message_async(&mut r).await.is_err());

    let mut over = vec![b'a'; MAX_MESSAGE_BYTES + 1];
    over.push(b'\n');
    let mut r = tokio::io::BufReader::new(&over[..]);
    assert!(read_message_async(&mut r).await.is_err());
}

#[test]
fn debug_redacts_token_and_answer() {
    assert!(!format!("{:?}", req(None)).contains("t0k"));
    assert!(!format!("{:?}", Reply::Answer(Zeroizing::new("hunter2".into()))).contains("hunter2"));
}

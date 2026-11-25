//! The inline name field's machine, step by step.

use dromon_core::{Error, HostId, RemotePath};

use super::*;

fn prod() -> HostId {
    HostId("prod".into())
}

fn site() -> RemotePath {
    RemotePath::new("/var/www/site")
}

fn type_in(edit: &mut Edit, text: &str) {
    let f = edit.field_mut().expect("a field");
    f.edit.replace_text(text);
    f.edit.mark_edited();
    step(edit, EditEvent::Changed);
}

fn started(f: Field) -> Edit {
    let mut e = Edit::Idle;
    assert_eq!(step(&mut e, EditEvent::Start(Box::new(f))), None);
    e
}

#[test]
fn a_new_folder_prefills_a_free_name_all_selected_and_focused() {
    let e = started(Field::new_folder(prod(), site(), "New folder (1)"));
    let f = e.field().unwrap();
    assert_eq!((f.edit.text.as_str(), f.edit.range(), f.edit.focused), ("New folder (1)", (0, 14), true));
    assert_eq!(f.renaming(), None);
}

#[test]
fn rename_selects_a_files_stem_and_a_whole_folder() {
    let file = Field::rename(prod(), site(), "report.final.pdf", false);
    assert_eq!(file.edit.range(), (0, "report.final".len()));
    let dir = Field::rename(prod(), site(), "assets.old", true);
    assert_eq!(dir.edit.range(), (0, "assets.old".len()));
    for (name, stem) in [(".env", 4), ("Makefile", 8), ("file.", 5), ("a.b", 1)] {
        assert_eq!(stem_len(name), stem, "{name}");
    }
    assert_eq!(file.renaming(), Some("report.final.pdf"));
}

#[test]
fn return_sends_mkdir_then_done_finishes_with_the_name() {
    let mut e = started(Field::new_folder(prod(), site(), "New folder"));
    let fx = step(&mut e, EditEvent::Submit);
    assert_eq!(fx, Some(EditEffect::Send { host: prod(), op: FileOp::Mkdir { path: RemotePath::new("/var/www/site/New folder") } }));
    assert!(e.is_sending());
    // Return and focus changes do nothing while it runs.
    assert_eq!(step(&mut e, EditEvent::Submit), None);
    assert_eq!(step(&mut e, EditEvent::Blur), None);
    assert!(e.is_sending());
    step(&mut e, EditEvent::Sent(OpId(4)));
    // Another operation's answer isn't this one's.
    assert_eq!(step(&mut e, EditEvent::Done(OpId(3))), None);
    let fx = step(&mut e, EditEvent::Done(OpId(4)));
    assert_eq!(fx, Some(EditEffect::Finished { host: prod(), folder: site(), name: "New folder".into() }));
    assert!(matches!(e, Edit::Idle));
}

#[test]
fn bad_names_stay_with_an_error_and_send_nothing() {
    let mut e = started(Field::new_folder(prod(), site(), "New folder"));
    for (name, error) in [
        ("", "Type a name."),
        (".", "“.” and “..” can't be used as names."),
        ("..", "“.” and “..” can't be used as names."),
        ("a/b", "A name can't contain “/”."),
        ("a\0b", "A name can't contain a NUL character."),
    ] {
        type_in(&mut e, name);
        assert_eq!(e.field().unwrap().error, None, "typing clears the error");
        assert_eq!(step(&mut e, EditEvent::Submit), None, "{name:?}");
        assert_eq!(e.field().unwrap().error.as_deref(), Some(error));
        assert!(matches!(e, Edit::Editing(_)));
    }
    assert_eq!(e.field().unwrap().shakes, 5, "each error shakes once");
}

#[test]
fn already_exists_comes_back_as_an_inline_error_and_the_field_stays() {
    let mut e = started(Field::new_folder(prod(), site(), "New folder"));
    type_in(&mut e, "assets");
    step(&mut e, EditEvent::Submit);
    step(&mut e, EditEvent::Sent(OpId(1)));
    let error = Error::AlreadyExists(RemotePath::new("/var/www/site/assets"));
    assert_eq!(step(&mut e, EditEvent::Failed(OpId(1), error)), None);
    let f = e.field().unwrap();
    assert_eq!((f.error.as_deref(), f.shakes, f.edit.text.as_str()), (Some("‘assets’ already exists here"), 1, "assets"));
    assert!(matches!(e, Edit::Editing(_)), "editable again");
    // Other refusals say what the server said.
    let denied = Error::PermissionDenied(RemotePath::new("/var/www/site/x"));
    assert_eq!(error_text("x", &denied), "You don't have access to /var/www/site/x");
}

#[test]
fn rename_to_the_same_name_cancels_and_another_name_renames_a_sibling() {
    let mut e = started(Field::rename(prod(), site(), "index.html", false));
    assert_eq!(step(&mut e, EditEvent::Submit), None);
    assert!(matches!(e, Edit::Idle), "same name: nothing to do");

    let mut e = started(Field::rename(prod(), site(), "index.html", false));
    type_in(&mut e, "home.html");
    let fx = step(&mut e, EditEvent::Submit);
    let op = FileOp::Rename { from: RemotePath::new("/var/www/site/index.html"), to: RemotePath::new("/var/www/site/home.html") };
    assert_eq!(fx, Some(EditEffect::Send { host: prod(), op }));
}

#[test]
fn escape_and_an_untouched_blur_cancel_a_typed_blur_submits() {
    let mut e = started(Field::new_folder(prod(), site(), "New folder"));
    assert_eq!(step(&mut e, EditEvent::Cancel), None);
    assert!(matches!(e, Edit::Idle));

    let mut e = started(Field::new_folder(prod(), site(), "New folder"));
    assert_eq!(step(&mut e, EditEvent::Blur), None);
    assert!(matches!(e, Edit::Idle), "nothing typed: nothing created");

    let mut e = started(Field::new_folder(prod(), site(), "New folder"));
    type_in(&mut e, "logs");
    assert!(matches!(step(&mut e, EditEvent::Blur), Some(EditEffect::Send { .. })));
}

#[test]
fn one_edit_at_a_time_and_gone_closes_it() {
    let mut e = started(Field::new_folder(prod(), site(), "New folder"));
    step(&mut e, EditEvent::Start(Box::new(Field::rename(prod(), site(), "README.md", false))));
    assert_eq!(e.field().and_then(Field::renaming), Some("README.md"), "a new edit replaces one being typed");
    type_in(&mut e, "READ.md");
    step(&mut e, EditEvent::Submit);
    step(&mut e, EditEvent::Start(Box::new(Field::new_folder(prod(), site(), "New folder"))));
    assert!(e.is_sending(), "never one that's sending");
    step(&mut e, EditEvent::Gone);
    assert!(matches!(e, Edit::Idle));
    // Late answers for it change nothing.
    assert_eq!(step(&mut e, EditEvent::Done(OpId(1))), None);
    assert!(matches!(e, Edit::Idle));
}

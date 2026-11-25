//! The drop pill's text.

use super::{pill_text, short_path};
use crate::state::{DropHover, DropTarget};

#[test]
fn the_pill_names_the_count_and_where() {
    let list = |count| DropHover { target: DropTarget::List, count };
    assert_eq!(pill_text(&list(3), "/var/www"), "Upload 3 items to /var/www");
    assert_eq!(pill_text(&list(1), "/var/www"), "Upload 1 item to /var/www");
    assert_eq!(pill_text(&list(1200), "/"), "Upload 1,200 items to /");
    let folder = DropHover { target: DropTarget::Folder("uploads".into()), count: 2 };
    assert_eq!(pill_text(&folder, "/var/www/site"), "Upload 2 items to uploads");
}

#[test]
fn long_paths_keep_their_last_folders() {
    assert_eq!(short_path("/var/www/site"), "/var/www/site");
    assert_eq!(short_path("/srv/media/photos/2026/november/holiday-in-the-mountains"), "…/november/holiday-in-the-mountains");
    let one = format!("/a/{}", "x".repeat(60));
    assert_eq!(short_path(&one), format!("…/{}", "x".repeat(60)), "the last folder stays whole");
}

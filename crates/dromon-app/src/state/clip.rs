//! The in-app clipboard (§3.7): remote items copied or cut with ⌘C / ⌘X, pasted with ⌘V or
//! moved with ⌥⌘V ("Move here"). Copying also writes the items' full paths to the OS clipboard
//! as text ([`Clip::text`]), so which clipboard is newer is the OS's call: a paste uses the clip
//! only while the OS clipboard still holds exactly that text ([`Clip::matches`]); anything
//! copied since (in this app or another) wins.

use dromon_core::{HostId, RemotePath};
use dromon_transfer::{JobRequest, RemoteItem};

#[cfg(test)]
#[path = "clip_tests.rs"]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub host: HostId,
    pub items: Vec<RemoteItem>,
    /// Cut (⌘X): the paste moves them, and the rows draw dimmed until then.
    pub cut: bool,
    /// What went onto the OS clipboard: the full paths, one per line.
    pub text: String,
}

/// Full paths, one per line, as copy path and ⌘C write them.
pub fn paths_text<'a>(paths: impl IntoIterator<Item = &'a RemotePath>) -> String {
    paths.into_iter().map(RemotePath::as_str).collect::<Vec<_>>().join("\n")
}

/// Text as compared: Windows' clipboard hands `\n` back as `\r\n`, and some apps add a final
/// newline.
fn normalized(text: &str) -> String {
    text.replace("\r\n", "\n").trim_end_matches('\n').to_owned()
}

impl Clip {
    pub fn new(host: HostId, items: Vec<RemoteItem>, cut: bool) -> Clip {
        let text = paths_text(items.iter().map(|i| &i.path));
        Clip { host, items, cut, text }
    }

    /// Whether the OS clipboard, holding `text`, still holds this clip.
    pub fn matches(&self, text: &str) -> bool {
        !self.items.is_empty() && normalized(text) == normalized(&self.text)
    }

    /// Whether `path` is one of the items cut (so its row draws dimmed).
    pub fn is_cut(&self, host: &HostId, path: &RemotePath) -> bool {
        self.cut && self.host == *host && self.items.iter().any(|i| i.path == *path)
    }

    /// The job that pastes it into `into` on `to`: within a host a copy is `RemoteCopy` and a
    /// move `Move`; to another host `CrossHostCopy` / `CrossHostMove`. A cut always moves;
    /// `as_move` (⌥⌘V) moves a copied clip too.
    pub fn paste(&self, to: &HostId, into: RemotePath, as_move: bool) -> JobRequest {
        let (items, moving) = (self.items.clone(), self.cut || as_move);
        match (self.host == *to, moving) {
            (true, false) => JobRequest::RemoteCopy { host: to.clone(), items, into },
            (true, true) => JobRequest::Move { host: to.clone(), items, into },
            (false, false) => JobRequest::CrossHostCopy { from: self.host.clone(), items, to: to.clone(), into },
            (false, true) => JobRequest::CrossHostMove { from: self.host.clone(), items, to: to.clone(), into },
        }
    }
}

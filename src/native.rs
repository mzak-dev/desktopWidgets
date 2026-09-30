//! The user-facing OS seams: file/folder pickers and clipboard text (`Native`), and modal
//! questions and notes (`Prompt`). Both are blocking, user-initiated calls on the
//! event-loop thread and carry per-window state (the parent window), so they are passed
//! per call, not bundled in `Ambient`. Each has three adapters: the Win32 one
//! (`dialog::WinNative`), `Scripted` (queued answers, in-memory clipboard, records
//! what it was asked) and `Headless` (no pick, "no" to every question, empty clipboard).

use std::collections::VecDeque;
use std::path::PathBuf;

/// What a picker asks for. `Filtered` lists `(name, "*.a;*.b")` file types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    File,
    Folder,
    Filtered(&'static [(&'static str, &'static str)]),
}

pub trait Native {
    /// The chosen path, `None` when cancelled.
    fn pick(&mut self, what: Pick) -> Option<PathBuf>;
    /// Clipboard text as one line (see [`one_line`]); `None` when empty or not text.
    fn clipboard(&mut self) -> Option<String>;
    fn set_clipboard(&mut self, text: &str);
}

pub trait Prompt {
    /// A modal OK/Cancel question; true on OK.
    fn confirm(&mut self, title: &str, text: &str) -> bool;
    /// A modal note; `error` shows it as an error.
    fn tell(&mut self, title: &str, text: &str, error: bool);
}

/// Clipboard text with newlines flattened: every field here is one line.
pub fn one_line(text: &str) -> Option<String> {
    Some(text.replace(['\r', '\n'], " ").trim().to_string()).filter(|s| !s.is_empty())
}

/// Answers from a script, for tests and windowless renders. Picks and confirmations are
/// taken from their queues in order (an empty queue cancels / says no); everything asked
/// or written is recorded.
#[derive(Default, Debug)]
pub struct Scripted {
    pub picks: VecDeque<Option<PathBuf>>,
    pub answers: VecDeque<bool>,
    pub text: Option<String>,
    pub picked: Vec<Pick>,
    pub written: Vec<String>,
    pub asked: Vec<(String, String)>,
    pub told: Vec<(String, String, bool)>,
}

impl Scripted {
    pub fn new() -> Self {
        Self::default()
    }

    /// The next picker answers with `path`.
    pub fn pick_next(mut self, path: impl Into<PathBuf>) -> Self {
        self.picks.push_back(Some(path.into()));
        self
    }

    /// The clipboard starts holding `text`.
    pub fn with_clipboard(mut self, text: &str) -> Self {
        self.text = Some(text.to_string());
        self
    }

    /// The next question is answered `yes`.
    pub fn answer_next(mut self, yes: bool) -> Self {
        self.answers.push_back(yes);
        self
    }
}

impl Native for Scripted {
    fn pick(&mut self, what: Pick) -> Option<PathBuf> {
        self.picked.push(what);
        self.picks.pop_front().flatten()
    }

    fn clipboard(&mut self) -> Option<String> {
        self.text.as_deref().and_then(one_line)
    }

    fn set_clipboard(&mut self, text: &str) {
        self.text = Some(text.to_string());
        self.written.push(text.to_string());
    }
}

impl Prompt for Scripted {
    fn confirm(&mut self, title: &str, text: &str) -> bool {
        self.asked.push((title.to_string(), text.to_string()));
        self.answers.pop_front().unwrap_or(false)
    }

    fn tell(&mut self, title: &str, text: &str, error: bool) {
        self.told.push((title.to_string(), text.to_string(), error));
    }
}

/// No user at all: picks cancel, questions are declined, the clipboard is empty and
/// writes go nowhere.
#[derive(Default, Clone, Copy, Debug)]
pub struct Headless;

impl Native for Headless {
    fn pick(&mut self, _: Pick) -> Option<PathBuf> {
        None
    }

    fn clipboard(&mut self) -> Option<String> {
        None
    }

    fn set_clipboard(&mut self, _: &str) {}
}

impl Prompt for Headless {
    fn confirm(&mut self, _: &str, _: &str) -> bool {
        false
    }

    fn tell(&mut self, _: &str, _: &str, _: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_text_is_one_trimmed_line() {
        assert_eq!(one_line("  a\r\nb\nc  ").as_deref(), Some("a  b c"));
        assert_eq!(one_line(" \r\n "), None);
        assert_eq!(one_line(""), None);
    }

    #[test]
    fn scripted_picks_come_in_order_then_cancel() {
        let mut n = Scripted::new().pick_next("a.txt");
        n.picks.push_back(None);
        n.picks.push_back(Some("c".into()));
        assert_eq!(n.pick(Pick::File), Some("a.txt".into()));
        assert_eq!(n.pick(Pick::Folder), None, "a scripted cancel");
        assert_eq!(n.pick(Pick::File), Some("c".into()));
        assert_eq!(n.pick(Pick::File), None, "an empty script cancels");
        assert_eq!(n.picked, [Pick::File, Pick::Folder, Pick::File, Pick::File]);
    }

    #[test]
    fn scripted_clipboard_reads_what_it_wrote() {
        let mut n = Scripted::new();
        assert_eq!(n.clipboard(), None);
        n.set_clipboard("one\r\ntwo");
        assert_eq!(n.clipboard().as_deref(), Some("one  two"));
        assert_eq!(n.written, ["one\r\ntwo"], "the write is recorded as given");
    }

    #[test]
    fn scripted_prompt_records_and_answers_in_order() {
        let mut p = Scripted::new().answer_next(true);
        assert!(p.confirm("T", "q1"));
        assert!(!p.confirm("T", "q2"), "an empty script says no");
        p.tell("T", "done", false);
        assert_eq!(p.asked, [("T".into(), "q1".into()), ("T".into(), "q2".into())]);
        assert_eq!(p.told, [("T".into(), "done".into(), false)]);
    }

    #[test]
    fn headless_does_nothing() {
        let mut h = Headless;
        h.set_clipboard("x");
        assert_eq!((h.pick(Pick::File), h.clipboard(), h.confirm("a", "b")), (None, None, false));
        h.tell("a", "b", true);
    }
}

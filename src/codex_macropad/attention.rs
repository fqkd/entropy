//! Completion acknowledgement belongs to a particular result, not just a thread.
use super::Task;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
struct SavedReads {
    version: u8,
    opened: HashMap<String, String>,
}
struct Pending {
    token: String,
    seen_unread: bool,
}
pub(super) struct Attention {
    path: Option<PathBuf>,
    saved: SavedReads,
    pending: HashMap<String, Pending>,
}
impl Attention {
    pub fn new(path: Option<PathBuf>) -> Self {
        let saved = path
            .as_ref()
            .and_then(|p| File::open(p).ok())
            .and_then(|f| serde_json::from_reader::<_, SavedReads>(f.take(256 * 1024)).ok())
            .filter(|v| v.version == 1)
            .unwrap_or_default();
        Self {
            path,
            saved,
            pending: HashMap::new(),
        }
    }
    pub fn opened(&mut self, task: &Task) {
        let Some(token) = task.completion.as_ref() else {
            return;
        };
        self.saved.opened.insert(task.id.clone(), token.clone());
        if self
            .pending
            .get(&task.id)
            .is_some_and(|p| &p.token == token)
        {
            self.pending.remove(&task.id);
        }
        self.save();
    }
    pub fn apply(&mut self, task: &mut Task, unread: Option<&HashSet<String>>) {
        if task.state == 2 || task.state == 5 {
            self.pending.remove(&task.id); // A new run supersedes the previous result.
            return;
        }
        if task.state == 3 {
            if let Some(token) = &task.completion {
                if self.saved.opened.get(&task.id) == Some(token) {
                    task.state = 1;
                    return;
                }
                if self.pending.get(&task.id).is_none_or(|p| &p.token != token) {
                    self.pending.insert(
                        task.id.clone(),
                        Pending {
                            token: token.clone(),
                            seen_unread: false,
                        },
                    );
                }
            }
        }
        if let Some(pending) = self.pending.get_mut(&task.id) {
            task.state = 3; // Hold completion even when a journal sample has no status.
            task.completion = Some(pending.token.clone());
            if let Some(unread) = unread {
                if unread.contains(&task.id) {
                    pending.seen_unread = true;
                } else if pending.seen_unread {
                    // Require a read transition: missing or delayed app flags alone
                    // must never extinguish a newly completed notification.
                    self.opened(task);
                    task.state = 1;
                }
            }
        }
    }
    pub fn retain(&mut self, tasks: &[Task]) {
        self.pending
            .retain(|id, _| tasks.iter().any(|t| &t.id == id));
        if self.saved.opened.len() > 256 {
            self.saved
                .opened
                .retain(|id, _| tasks.iter().any(|t| &t.id == id));
            self.save();
        }
    }
    fn save(&mut self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        self.saved.version = 1;
        let result = (|| -> anyhow::Result<()> {
            let parent = path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("Missing settings directory"))?;
            std::fs::create_dir_all(parent)?;
            let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
            serde_json::to_writer(&mut tmp, &self.saved)?;
            tmp.flush()?;
            tmp.persist(path)?;
            Ok(())
        })();
        if let Err(e) = result {
            log::warn!("Cannot save Macropad read state: {e}");
        }
    }
}

// Read the app's persisted flags, never change them. With multiple identities,
// don't guess which account owns a missing flag; keypad opens still acknowledge.
#[derive(Deserialize)]
struct GlobalState {
    #[serde(rename = "electron-thread-read-state-v1")]
    reads: ReadState,
}
#[derive(Deserialize)]
struct ReadState {
    version: u8,
    #[serde(rename = "unreadByIdentity")]
    identities: HashMap<String, HashMap<String, Vec<String>>>,
}
fn parse_unread(reader: impl Read) -> Option<HashSet<String>> {
    let state: GlobalState = serde_json::from_reader(reader).ok()?;
    if state.reads.version != 1 || state.reads.identities.len() != 1 {
        return None;
    }
    let hosts = state.reads.identities.into_values().next()?;
    let mut found = false;
    let mut ids = HashSet::new();
    for (host, threads) in hosts {
        if host.starts_with("local:") || host.starts_with("durable:") {
            found = true;
            ids.extend(
                threads
                    .into_iter()
                    .filter(|id| super::uuid_bytes(id).is_some()),
            );
        }
    }
    found.then_some(ids)
}
pub(super) fn unread_tasks(path: &Path) -> Option<HashSet<String>> {
    let f = File::open(path).ok()?;
    if f.metadata().ok()?.len() > 16 * 1024 * 1024 {
        return None;
    }
    parse_unread(f.take(16 * 1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn done(token: &str) -> Task {
        Task {
            id: "task".into(),
            title: "Title".into(),
            state: 3,
            completion: Some(token.into()),
        }
    }
    #[test]
    fn completion_stays_lit_until_open_and_does_not_return_on_poll() {
        let mut a = Attention::new(None);
        let mut t = done("one");
        a.apply(&mut t, None);
        t.state = 1;
        t.completion = None;
        a.apply(&mut t, None);
        assert_eq!(t.state, 3);
        a.opened(&t);
        t = done("one");
        a.apply(&mut t, None);
        assert_eq!(t.state, 1);
        t = done("two");
        a.apply(&mut t, None);
        assert_eq!(t.state, 3);
    }
    #[test]
    fn only_an_observed_app_read_transition_clears_completion() {
        let mut a = Attention::new(None);
        let empty = HashSet::new();
        let mut t = done("one");
        a.apply(&mut t, Some(&empty));
        assert_eq!(t.state, 3);
        let unread = HashSet::from(["task".into()]);
        a.apply(&mut t, Some(&unread));
        a.apply(&mut t, None);
        assert_eq!(t.state, 3);
        a.apply(&mut t, Some(&empty));
        assert_eq!(t.state, 1);
        t = done("two");
        a.apply(&mut t, Some(&empty));
        assert_eq!(t.state, 3);
        for _ in 0..1000 {
            a.apply(&mut t, Some(&empty));
        }
        assert_eq!(t.state, 3);
    }
    #[test]
    fn an_old_open_does_not_acknowledge_a_new_completion() {
        let mut a = Attention::new(None);
        let old = done("one");
        let mut new = done("two");
        a.apply(&mut new, None);
        a.opened(&old);
        a.apply(&mut new, None);
        assert_eq!(new.state, 3);
        new.state = 2;
        new.completion = None;
        a.apply(&mut new, None);
        assert_eq!(new.state, 2);
        new.state = 1;
        a.apply(&mut new, None);
        assert_eq!(new.state, 1);
    }
    #[test]
    fn acknowledgement_survives_restart() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("reads.json");
        Attention::new(Some(p.clone())).opened(&done("one"));
        let mut a = Attention::new(Some(p));
        let mut t = done("one");
        a.apply(&mut t, None);
        assert_eq!(t.state, 1);
    }
    #[test]
    fn unsupported_or_ambiguous_read_state_is_not_treated_as_read() {
        assert!(parse_unread(b"{}".as_slice()).is_none());
        assert!(parse_unread(br#"{"electron-thread-read-state-v1":{"version":2,"unreadByIdentity":{"a":{"local:x":[]}}}}"#.as_slice()).is_none());
        assert!(parse_unread(br#"{"electron-thread-read-state-v1":{"version":1,"unreadByIdentity":{"a":{},"b":{}}}}"#.as_slice()).is_none());
        let value=br#"{"electron-thread-read-state-v1":{"version":1,"unreadByIdentity":{"a":{"local:x":["12345678-1234-1234-1234-123456789abc"],"durable:y":[]}}}}"#;
        assert_eq!(parse_unread(value.as_slice()).unwrap().len(), 1);
    }
}

//! Completion acknowledgement belongs to a particular result, not just a thread.
use super::Task;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};

#[derive(Default, Serialize, Deserialize)]
struct SavedReads {
    version: u8,
    opened: HashMap<String, String>,
}
struct Pending {
    token: String,
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
            .and_then(|f| {
                // Read only Entropy's receipt file; close it before parsing.
                let mut bytes = Vec::new();
                f.take(256 * 1024 + 1).read_to_end(&mut bytes).ok()?;
                (bytes.len() <= 256 * 1024).then_some(bytes)
            })
            .and_then(|bytes| serde_json::from_slice::<SavedReads>(&bytes).ok())
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
    pub fn apply(&mut self, task: &mut Task) {
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
                        },
                    );
                }
            }
        }
        if let Some(pending) = self.pending.get(&task.id) {
            task.state = 3; // Hold completion even when a journal sample has no status.
            task.completion = Some(pending.token.clone());
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
        a.apply(&mut t);
        t.state = 1;
        t.completion = None;
        a.apply(&mut t);
        assert_eq!(t.state, 3);
        a.opened(&t);
        t = done("one");
        a.apply(&mut t);
        assert_eq!(t.state, 1);
        t = done("two");
        a.apply(&mut t);
        assert_eq!(t.state, 3);
    }
    #[test]
    fn an_old_open_does_not_acknowledge_a_new_completion() {
        let mut a = Attention::new(None);
        let old = done("one");
        let mut new = done("two");
        a.apply(&mut new);
        a.opened(&old);
        a.apply(&mut new);
        assert_eq!(new.state, 3);
        new.state = 2;
        new.completion = None;
        a.apply(&mut new);
        assert_eq!(new.state, 2);
        new.state = 1;
        a.apply(&mut new);
        assert_eq!(new.state, 1);
    }
    #[test]
    fn acknowledgement_survives_restart() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("reads.json");
        Attention::new(Some(p.clone())).opened(&done("one"));
        let mut a = Attention::new(Some(p));
        let mut t = done("one");
        a.apply(&mut t);
        assert_eq!(t.state, 1);
    }
    #[test]
    fn invalid_or_oversized_receipts_do_not_acknowledge_results() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("reads.json");
        let mut oversized = br#"{"version":1,"opened":{"task":"one"}}"#.to_vec();
        oversized.resize(256 * 1024 + 1, b' ');
        for data in [
            b"broken".to_vec(),
            br#"{"version":2,"opened":{"task":"one"}}"#.to_vec(),
            oversized,
        ] {
            std::fs::write(&p, data).unwrap();
            let mut a = Attention::new(Some(p.clone()));
            let mut t = done("one");
            a.apply(&mut t);
            assert_eq!(t.state, 3);
        }
    }
}

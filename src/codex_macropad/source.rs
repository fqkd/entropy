use super::{
    attention::{unread_tasks, Attention},
    uuid_bytes, Task,
};
use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    time::Duration,
};

const TASK_QUERY: &str = "SELECT id, COALESCE(NULLIF(name,''), NULLIF(title,''), 'Untitled'), rollout_path FROM threads WHERE archived=0 AND (agent_path IS NULL OR agent_path='') AND lower(source) NOT LIKE '%subagent%' ORDER BY COALESCE(recency_at_ms,updated_at_ms,updated_at*1000) DESC, id LIMIT 6";
const TAIL_BYTES: u64 = 512 * 1024;

struct JournalState {
    path: PathBuf,
    bytes: u64,
    phase: u8,
    completion: Option<String>,
}

pub(super) struct Source {
    home: PathBuf,
    db: Connection,
    phases: HashMap<String, JournalState>,
    attention: Attention,
}
impl Source {
    pub fn new() -> Result<Self> {
        let home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|p| p.join(".codex")))
            .context("Codex home not found")?;
        let db = Connection::open_with_flags(
            home.join("state_5.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .context("Cannot read local Codex tasks / Не удалось прочитать локальные задачи Codex")?;
        db.busy_timeout(Duration::from_millis(100))?;
        Ok(Self {
            home: home.canonicalize()?,
            db,
            phases: HashMap::new(),
            attention: Attention::new(
                dirs::config_dir().map(|p| p.join("entropy/codex_task_reads.json")),
            ),
        })
    }
    pub fn opened(&mut self, task: &Task) {
        self.attention.opened(task);
    }
    pub fn sample(&mut self, previous: &[Task]) -> Result<Vec<Task>> {
        if !desktop_running() {
            bail!("Open Codex / Откройте Codex");
        }
        // Explicit schema contract. Fail closed if a later app changes it.
        let mut statement = self.db.prepare(TASK_QUERY)?;
        let values = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut recent = Vec::new();
        let unread = unread_tasks(&self.home.join(".codex-global-state.json"));
        for (id, title, rollout) in values {
            if uuid_bytes(&id).is_none() {
                continue;
            }
            let path = PathBuf::from(rollout);
            let mut completion = None;
            let state = match path.canonicalize() {
                Ok(p) if p.starts_with(self.home.join("sessions")) => {
                    let mut f = File::open(&p)?;
                    let metadata = f.metadata()?;
                    let start = metadata.len().saturating_sub(TAIL_BYTES);
                    f.seek(SeekFrom::Start(start))?;
                    let mut bytes = Vec::new();
                    f.take(TAIL_BYTES).read_to_end(&mut bytes)?;
                    let previous = self.phases.get(&id).filter(|state| state.path == p);
                    let observed = event_from_tail(&bytes, start);
                    let phase = next_phase(
                        observed.as_ref().map(|event| event.0),
                        previous.map(|state| (state.bytes, state.phase)),
                        metadata.len(),
                    );
                    if phase == 3 {
                        completion = observed
                            .and_then(|event| event.1)
                            .or_else(|| previous.and_then(|state| state.completion.clone()));
                    }
                    self.phases.insert(
                        id.clone(),
                        JournalState {
                            path: p,
                            bytes: metadata.len(),
                            phase,
                            completion: completion.clone(),
                        },
                    );
                    // A journal is not a heartbeat: do not claim "working" indefinitely.
                    if phase == 2
                        && metadata.modified()?.elapsed().unwrap_or_default()
                            > Duration::from_secs(120)
                    {
                        1
                    } else {
                        phase
                    }
                }
                _ => 1,
            };
            let mut task = Task {
                id,
                title,
                state,
                completion,
            };
            self.attention.apply(&mut task, unread.as_ref());
            recent.push(task);
        }
        self.phases
            .retain(|id, _| recent.iter().any(|t| &t.id == id));
        self.attention.retain(&recent);
        Ok(stable_order(previous, recent))
    }
}
fn next_phase(observed: Option<u8>, previous: Option<(u64, u8)>, size: u64) -> u8 {
    // Large tool outputs can push a new task_started out of the bounded tail.
    // Never reuse "complete" (or another phase) across an unobserved gap.
    observed
        .or_else(|| {
            previous
                .filter(|(old_size, _)| size >= *old_size && size - *old_size <= TAIL_BYTES)
                .map(|(_, phase)| phase)
        })
        .unwrap_or(1)
}
fn stable_order(previous: &[Task], recent: Vec<Task>) -> Vec<Task> {
    let mut remaining = recent;
    let mut ordered = Vec::new();
    for old in previous {
        if let Some(i) = remaining.iter().position(|t| t.id == old.id) {
            ordered.push(remaining.remove(i));
        }
    }
    ordered.extend(remaining);
    ordered
}
fn event_from_tail(bytes: &[u8], start: u64) -> Option<(u8, Option<String>)> {
    let mut event = None;
    let mut offset = start;
    for (index, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
        let line_offset = offset;
        offset += line.len() as u64;
        if start != 0 && index == 0 {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|v| v.as_str()) != Some("event_msg") {
            continue;
        }
        let phase = match v.pointer("/payload/type").and_then(|v| v.as_str()) {
            Some("task_started") => 2,
            Some("task_complete") => 3,
            Some("turn_aborted") => 5,
            _ => continue,
        };
        let completion = (phase == 3).then(|| {
            format!(
                "{}:{line_offset}",
                v.get("timestamp").and_then(|v| v.as_str()).unwrap_or("")
            )
        });
        event = Some((phase, completion));
    }
    event
}
#[cfg(test)]
fn phase_from_tail(bytes: &[u8], skip_first: bool) -> Option<u8> {
    event_from_tail(bytes, u64::from(skip_first)).map(|event| event.0)
}
fn desktop_running() -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
    };
    unsafe {
        let h = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if h == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(h, &mut entry);
        let mut found = false;
        while ok != 0 {
            let end = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..end]).eq_ignore_ascii_case("ChatGPT.exe")
            {
                found = true;
                break;
            }
            ok = Process32NextW(h, &mut entry);
        }
        CloseHandle(h);
        found
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_identity_is_stable_across_tail_windows_and_changes_on_new_result() {
        let prefix = b"ignored\n";
        let done = b"{\"timestamp\":\"2026-10-01T12:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n";
        let mut journal = prefix.to_vec();
        journal.extend_from_slice(done);
        let first = event_from_tail(&journal, 0).unwrap();
        assert!(first.1.is_some());
        assert_eq!(event_from_tail(&journal[3..], 3), Some(first.clone()));
        journal.extend_from_slice(b"{\"type\":\"response_item\"}\n");
        assert_eq!(event_from_tail(&journal, 0), Some(first.clone()));
        // Even equal timestamps must not reuse an acknowledgement of an old result.
        journal.extend_from_slice(done);
        assert_ne!(event_from_tail(&journal, 0).unwrap().1, first.1);
        journal.extend_from_slice(
            b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
        );
        assert_eq!(event_from_tail(&journal, 0), Some((2, None)));
    }
    #[test]
    fn missing_large_append_or_truncation_cannot_keep_a_completed_state() {
        assert_eq!(next_phase(None, Some((10, 3)), TAIL_BYTES + 11), 1);
        assert_eq!(next_phase(None, Some((100, 3)), 50), 1);
        assert_eq!(next_phase(None, Some((100, 2)), 101), 2);
        assert_eq!(next_phase(Some(3), Some((100, 2)), TAIL_BYTES + 1000), 3);
    }
    #[test]
    fn event_order_and_incomplete_tail() {
        let data=b"garbage\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n{\"type\":";
        assert_eq!(phase_from_tail(data, true), Some(3));
        assert_eq!(
            phase_from_tail(
                b"{\"type\":\"response_item\",\"payload\":{\"type\":\"task_complete\"}}",
                false
            ),
            None
        );
    }
    #[test]
    fn recent_updates_do_not_shuffle_existing_rows() {
        let t = |id: &str| Task {
            id: id.into(),
            title: id.into(),
            state: 1,
            completion: None,
        };
        let ordered = stable_order(&[t("a"), t("b")], vec![t("b"), t("c"), t("a")]);
        assert_eq!(
            ordered.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }
    #[test]
    fn cancellation_and_restart_take_precedence_over_completion() {
        let data=b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_aborted\"}}\n";
        assert_eq!(phase_from_tail(data, false), Some(5));
        assert_eq!(
            phase_from_tail(
                b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
                false
            ),
            Some(2)
        );
    }
    #[test]
    fn canonical_windows_journals_remain_inside_sessions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sessions")).unwrap();
        let path = dir.path().join("sessions/task.jsonl");
        std::fs::write(&path, b"test").unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(path
            .canonicalize()
            .unwrap()
            .starts_with(root.join("sessions")));
    }
    #[test]
    fn query_excludes_archived_and_subagents_and_prefers_renamed_title() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE threads (id TEXT, name TEXT, title TEXT, rollout_path TEXT, archived INT, agent_path TEXT, source TEXT, recency_at_ms INT, updated_at_ms INT, updated_at INT);
        INSERT INTO threads VALUES ('a','Renamed','Original','a.jsonl',0,NULL,'vscode',10,10,0);
        INSERT INTO threads VALUES ('b',NULL,'Second','b.jsonl',0,NULL,'vscode',20,20,0);
        INSERT INTO threads VALUES ('c',NULL,'Hidden','c.jsonl',1,NULL,'vscode',30,30,0);
        INSERT INTO threads VALUES ('d',NULL,'Subagent','d.jsonl',0,NULL,'subagent',40,40,0);
        INSERT INTO threads VALUES ('e',NULL,'Child','e.jsonl',0,'/root/child','vscode',50,50,0);").unwrap();
        let mut q = db.prepare(TASK_QUERY).unwrap();
        let titles = q
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(titles, vec!["Second", "Renamed"]);
    }
    #[test]
    fn database_queries_are_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state_5.sqlite");
        {
            let db = Connection::open(&path).unwrap();
            db.execute("CREATE TABLE sample(id TEXT)", []).unwrap();
        }
        let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert!(db
            .execute("INSERT INTO sample VALUES ('test')", [])
            .is_err());
    }
}

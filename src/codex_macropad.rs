//! Opt-in local task list. Entropy owns both labels and open-by-ID events;
//! it never guesses the desktop's private Micro slot order.
use crate::hid::SharedHidOutput;
use anyhow::{bail, Context, Result};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
mod source;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Task {
    pub id: String,
    pub title: String,
    pub state: u8,
}

pub(crate) fn uuid_bytes(id: &str) -> Option<[u8; 16]> {
    if id.len() != 36 {
        return None;
    }
    let mut compact = String::new();
    for (i, c) in id.chars().enumerate() {
        if [8, 13, 18, 23].contains(&i) {
            if c != '-' {
                return None;
            }
        } else if c.is_ascii_hexdigit() {
            compact.push(c);
        } else {
            return None;
        }
    }
    let mut out = [0; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&compact[i * 2..i * 2 + 2], 16).ok()?;
    }
    (out != [0; 16]).then_some(out)
}
fn title_bytes(title: &str) -> Vec<u8> {
    let clean: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut end = clean.len().min(96);
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    clean.as_bytes()[..end].to_vec()
}

pub(crate) struct Bridge {
    output: SharedHidOutput,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<String>>,
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
impl Bridge {
    pub fn matches(&self, output: &SharedHidOutput) -> bool {
        self.output.shares_owner_with(output)
    }
    pub fn status(&self) -> String {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn start(output: SharedHidOutput) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new("Connecting / Подключение".to_owned()));
        let (worker_output, worker_stop, worker_status) =
            (output.clone(), stop.clone(), status.clone());
        thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) && worker_output.is_available() {
                let result = run(&worker_output, &worker_stop, &worker_status);
                if let Err(e) = result {
                    *worker_status.lock().unwrap_or_else(|e| e.into_inner()) = format!("{e:#}");
                }
                for _ in 0..20 {
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        });
        Self {
            output,
            stop,
            status,
        }
    }
}
struct Wire<'a> {
    output: &'a SharedHidOutput,
    serial: u8,
}
impl Wire<'_> {
    fn request(&mut self, op: u8, gen: u32, payload: &[u8]) -> Result<[u8; 32]> {
        if payload.len() > 24 {
            bail!("Macropad payload too large");
        }
        self.serial = self.serial.wrapping_add(1);
        let mut p = [0; 32];
        p[0] = 0xD7;
        p[1] = 1;
        p[2] = op;
        p[3] = self.serial;
        p[4..8].copy_from_slice(&gen.to_le_bytes());
        p[8..8 + payload.len()].copy_from_slice(payload);
        let r = self.output.codex_exchange(&p)?;
        if r[..8] != p[..8] {
            bail!("Macropad reply mismatch; update firmware");
        }
        Ok(r)
    }
    fn ok(&mut self, op: u8, gen: u32, p: &[u8]) -> Result<[u8; 32]> {
        let r = self.request(op, gen, p)?;
        if r[8] != 0 {
            bail!("Macropad protocol error {}", r[8]);
        }
        Ok(r)
    }
    fn snapshot(&mut self, gen: u32, tasks: &[Task]) -> Result<bool> {
        self.ok(1, gen, &[])?;
        for slot in 0..6 {
            let mut meta = [0; 20];
            meta[0] = slot as u8;
            let mut title = Vec::new();
            if let Some(t) = tasks.get(slot) {
                let id = uuid_bytes(&t.id).context("Invalid local task ID")?;
                if !(1..=5).contains(&t.state) {
                    bail!("Invalid task status");
                }
                title = title_bytes(&t.title);
                if title.is_empty() {
                    title = b"Untitled".to_vec();
                }
                meta[1] = t.state;
                meta[2..18].copy_from_slice(&id);
                meta[18] = title.len() as u8;
            }
            self.ok(2, gen, &meta)?;
            for (chunk, data) in title.chunks(21).enumerate() {
                let mut p = vec![slot as u8, (chunk * 21) as u8, data.len() as u8];
                p.extend_from_slice(data);
                self.ok(3, gen, &p)?;
            }
        }
        match self.request(4, gen, &[])?[8] {
            0 => Ok(true),
            2 => Ok(false),
            e => bail!("Snapshot rejected: {e}"),
        }
    }
}
fn run(output: &SharedHidOutput, stop: &AtomicBool, status: &Mutex<String>) -> Result<()> {
    let mut wire = Wire { output, serial: 0 };
    let cap = wire.ok(0, 0, &[])?;
    if &cap[9..13] != b"MPT1" {
        bail!("Update Macropad firmware / Обновите прошивку Macropad");
    }
    let mut source = source::Source::new()?;
    let mut generation = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u32;
    let mut applied: Vec<Task> = Vec::new();
    let mut established = false;
    let mut next_sample = Instant::now();
    let mut next_snapshot = Instant::now();
    let mut desired = Vec::new();
    let mut opened = 0;
    let result = (|| -> Result<()> {
        while !stop.load(Ordering::Acquire) && output.is_available() {
            if Instant::now() >= next_sample {
                desired = source.sample(&applied)?;
                next_sample = Instant::now() + Duration::from_secs(1);
            }
            if established {
                let r = wire.ok(5, generation, &[])?;
                if r[17] != 255 {
                    let event = u32::from_le_bytes(r[9..13].try_into()?);
                    let event_gen = u32::from_le_bytes(r[13..17].try_into()?);
                    if event_gen != generation {
                        bail!("Stale task selection rejected");
                    }
                    let task = applied
                        .get(r[17] as usize)
                        .context("Invalid selected slot")?;
                    if opened != event {
                        open_task(&task.id)?;
                        opened = event;
                    }
                    wire.ok(6, generation, &event.to_le_bytes())?;
                }
            }
            if (!established || desired != applied) && Instant::now() >= next_snapshot {
                let candidate = generation.wrapping_add(1);
                if wire.snapshot(candidate, &desired)? {
                    generation = candidate;
                    applied = desired.clone();
                    established = true;
                    *status.lock().unwrap_or_else(|e| e.into_inner()) =
                        format!("Local tasks / Локальные задачи: {}", applied.len());
                }
                next_snapshot = Instant::now() + Duration::from_secs(1);
            }
            thread::sleep(Duration::from_millis(150));
        }
        Ok(())
    })();
    if established {
        let _ = wire.request(7, generation, &[]);
    }
    result
}
fn open_task(id: &str) -> Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if uuid_bytes(id).is_none() {
        bail!("Invalid task ID");
    }
    let uri: Vec<u16> = format!("codex://threads/{id}\0").encode_utf16().collect();
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            uri.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        bail!("Windows could not open Codex ({})", result as isize);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_cannot_be_urls_or_commands() {
        assert!(uuid_bytes("12345678-1234-1234-1234-123456789abc").is_some());
        assert!(uuid_bytes("00000000-0000-0000-0000-000000000000").is_none());
        for id in [
            "https://example.com",
            "../test",
            "12345678-1234-1234-1234-123456789abc?x",
            "zzzzzzzz-1234-1234-1234-123456789abc",
        ] {
            assert!(uuid_bytes(id).is_none());
        }
    }
    #[test]
    fn titles_do_not_split_utf8() {
        let s = title_bytes(&"я🙂".repeat(30));
        assert!(s.len() <= 96);
        assert!(std::str::from_utf8(&s).is_ok());
        assert_eq!(title_bytes("A\nB\0C"), b"A B C");
    }
}

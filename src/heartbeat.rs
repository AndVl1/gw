use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct HeartbeatState {
    pub current_task: Option<String>,
    pub task_started_at: Option<Instant>,
    pub last_output_at: Instant,
    pub started_at: Instant,
    pub stop: bool,
}

impl HeartbeatState {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            current_task: None,
            task_started_at: None,
            last_output_at: now,
            started_at: now,
            stop: false,
        }
    }
}

pub struct Heartbeat {
    pub state: Arc<Mutex<HeartbeatState>>,
    handle: Option<JoinHandle<()>>,
}

impl Heartbeat {
    pub fn start(silent_threshold: Duration, tick: Duration) -> Self {
        let state = Arc::new(Mutex::new(HeartbeatState::new()));
        let s2 = Arc::clone(&state);
        let handle = thread::spawn(move || heartbeat_loop(s2, silent_threshold, tick));
        Self {
            state,
            handle: Some(handle),
        }
    }

    pub fn note_output(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.last_output_at = Instant::now();
        }
    }

    pub fn set_task(&self, task: Option<String>) {
        if let Ok(mut s) = self.state.lock() {
            if s.current_task != task {
                s.task_started_at = task.as_ref().map(|_| Instant::now());
                s.current_task = task;
                s.last_output_at = Instant::now();
            }
        }
    }

    pub fn stop(mut self) {
        if let Ok(mut s) = self.state.lock() {
            s.stop = true;
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn heartbeat_loop(state: Arc<Mutex<HeartbeatState>>, silent_threshold: Duration, tick: Duration) {
    loop {
        thread::sleep(tick);
        let mut emit: Option<(Option<String>, Duration)> = None;
        {
            let mut s = match state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if s.stop {
                return;
            }
            let now = Instant::now();
            let silent_for = now.duration_since(s.last_output_at);
            if silent_for >= silent_threshold {
                let elapsed = match (s.current_task.as_ref(), s.task_started_at) {
                    (Some(_), Some(started)) => now.duration_since(started),
                    _ => now.duration_since(s.started_at),
                };
                emit = Some((s.current_task.clone(), elapsed));
                s.last_output_at = now;
            }
        }
        if let Some((task, elapsed)) = emit {
            let stdout = std::io::stdout();
            let mut h = stdout.lock();
            let label = task.as_deref().unwrap_or("building");
            let _ = writeln!(h, "▸ {} ({}s)", label, elapsed.as_secs());
            let _ = h.flush();
        }
    }
}

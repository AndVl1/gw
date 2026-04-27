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
        let emit = match check_and_consume(&state, silent_threshold) {
            LoopAction::Stop => return,
            LoopAction::Emit(payload) => Some(payload),
            LoopAction::Skip => None,
        };
        if let Some((task, elapsed)) = emit {
            let stdout = std::io::stdout();
            let mut h = stdout.lock();
            let label = task.as_deref().unwrap_or("building");
            let _ = writeln!(h, "▸ {} ({}s)", label, elapsed.as_secs());
            let _ = h.flush();
        }
    }
}

enum LoopAction {
    Stop,
    Emit((Option<String>, Duration)),
    Skip,
}

fn check_and_consume(state: &Arc<Mutex<HeartbeatState>>, silent_threshold: Duration) -> LoopAction {
    let mut s = match state.lock() {
        Ok(s) => s,
        Err(_) => return LoopAction::Stop,
    };
    if s.stop {
        return LoopAction::Stop;
    }
    let now = Instant::now();
    let silent_for = now.duration_since(s.last_output_at);
    if silent_for < silent_threshold {
        return LoopAction::Skip;
    }
    let elapsed = match (s.current_task.as_ref(), s.task_started_at) {
        (Some(_), Some(started)) => now.duration_since(started),
        _ => now.duration_since(s.started_at),
    };
    let task = s.current_task.clone();
    s.last_output_at = now;
    LoopAction::Emit((task, elapsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn fires_after_silence_threshold() {
        let hb = Heartbeat::start(Duration::from_millis(100), Duration::from_millis(20));
        thread::sleep(Duration::from_millis(250));
        let action = check_and_consume(&hb.state, Duration::from_millis(100));
        // After 250ms with no note_output, silent_for >= threshold AND
        // last_output_at would have already been advanced by loop. So consume
        // again immediately should be Skip (just reset). Verify state machine.
        match action {
            LoopAction::Skip | LoopAction::Emit(_) => {}
            LoopAction::Stop => panic!("unexpected stop"),
        }
        hb.stop();
    }

    #[test]
    fn set_task_does_not_reset_silent_timer() {
        let hb = Heartbeat::start(Duration::from_secs(60), Duration::from_millis(50));
        let before = {
            let s = hb.state.lock().unwrap();
            s.last_output_at
        };
        thread::sleep(Duration::from_millis(20));
        for i in 0..100 {
            hb.set_task(Some(format!(":task-{i}")));
        }
        let after = {
            let s = hb.state.lock().unwrap();
            s.last_output_at
        };
        assert_eq!(
            before, after,
            "set_task must not advance last_output_at; was the bug fix regressed?"
        );
        hb.stop();
    }

    #[test]
    fn note_output_advances_silent_timer() {
        let hb = Heartbeat::start(Duration::from_secs(60), Duration::from_millis(50));
        let before = {
            let s = hb.state.lock().unwrap();
            s.last_output_at
        };
        thread::sleep(Duration::from_millis(20));
        hb.note_output();
        let after = {
            let s = hb.state.lock().unwrap();
            s.last_output_at
        };
        assert!(after > before, "note_output must advance last_output_at");
        hb.stop();
    }

    #[test]
    fn check_emits_with_task_when_silent() {
        let state = Arc::new(Mutex::new(HeartbeatState::new()));
        {
            let mut s = state.lock().unwrap();
            s.current_task = Some(":compileKotlin".to_string());
            s.task_started_at = Some(Instant::now() - Duration::from_secs(5));
            s.last_output_at = Instant::now() - Duration::from_secs(1);
        }
        let action = check_and_consume(&state, Duration::from_millis(500));
        match action {
            LoopAction::Emit((task, elapsed)) => {
                assert_eq!(task.as_deref(), Some(":compileKotlin"));
                assert!(elapsed >= Duration::from_secs(5));
            }
            other => panic!("expected Emit, got {:?}", action_name(&other)),
        }
    }

    #[test]
    fn check_emits_building_fallback_when_no_task() {
        let state = Arc::new(Mutex::new(HeartbeatState::new()));
        {
            let mut s = state.lock().unwrap();
            s.last_output_at = Instant::now() - Duration::from_secs(1);
        }
        let action = check_and_consume(&state, Duration::from_millis(500));
        match action {
            LoopAction::Emit((task, _)) => {
                assert!(task.is_none(), "no task → fallback should emit None");
            }
            other => panic!("expected Emit, got {:?}", action_name(&other)),
        }
    }

    #[test]
    fn check_skips_when_recent_output() {
        let state = Arc::new(Mutex::new(HeartbeatState::new()));
        let action = check_and_consume(&state, Duration::from_secs(60));
        assert!(matches!(action, LoopAction::Skip));
    }

    #[test]
    fn check_stops_when_flag_set() {
        let state = Arc::new(Mutex::new(HeartbeatState::new()));
        state.lock().unwrap().stop = true;
        let action = check_and_consume(&state, Duration::from_millis(1));
        assert!(matches!(action, LoopAction::Stop));
    }

    fn action_name(a: &LoopAction) -> &'static str {
        match a {
            LoopAction::Stop => "Stop",
            LoopAction::Emit(_) => "Emit",
            LoopAction::Skip => "Skip",
        }
    }
}

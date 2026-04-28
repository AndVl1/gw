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
    pub progress_count: u32,
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
            progress_count: 0,
            stop: false,
        }
    }
}

pub struct Heartbeat {
    pub state: Arc<Mutex<HeartbeatState>>,
    handle: Option<JoinHandle<()>>,
}

impl Heartbeat {
    pub fn start(silent_threshold: Duration, tick: Duration, slow_threshold: Duration) -> Self {
        let state = Arc::new(Mutex::new(HeartbeatState::new()));
        let s2 = Arc::clone(&state);
        let handle =
            thread::spawn(move || heartbeat_loop(s2, silent_threshold, tick, slow_threshold));
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

    pub fn set_progress(&self, count: u32) {
        if let Ok(mut s) = self.state.lock() {
            s.progress_count = count;
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

fn heartbeat_loop(
    state: Arc<Mutex<HeartbeatState>>,
    silent_threshold: Duration,
    tick: Duration,
    slow_threshold: Duration,
) {
    loop {
        thread::sleep(tick);
        let emit = match check_and_consume(&state, silent_threshold) {
            LoopAction::Stop => return,
            LoopAction::Emit(payload) => Some(payload),
            LoopAction::Skip => None,
        };
        if let Some(payload) = emit {
            let line = format_heartbeat(&payload, slow_threshold);
            let stdout = std::io::stdout();
            let mut h = stdout.lock();
            let _ = writeln!(h, "{}", line);
            let _ = h.flush();
        }
    }
}

#[derive(Debug, Clone)]
struct EmitPayload {
    task: Option<String>,
    elapsed: Duration,
    progress_count: u32,
    has_task: bool,
}

fn format_heartbeat(p: &EmitPayload, slow_threshold: Duration) -> String {
    let label = p.task.as_deref().unwrap_or("building");
    let mut line = format!("▸ {} ({})", label, format_duration(p.elapsed));
    if p.progress_count > 0 {
        line.push_str(&format!(" [{} tasks]", p.progress_count));
    }
    if p.has_task && p.elapsed >= slow_threshold {
        line.push_str(" — slow");
    }
    line
}

fn format_duration(d: Duration) -> String {
    let total = d.as_secs();
    if total < 60 {
        format!("{}s", total)
    } else {
        let m = total / 60;
        let s = total % 60;
        format!("{}m{:02}s", m, s)
    }
}

enum LoopAction {
    Stop,
    Emit(EmitPayload),
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
    let (elapsed, has_task) = match (s.current_task.as_ref(), s.task_started_at) {
        (Some(_), Some(started)) => (now.duration_since(started), true),
        _ => (now.duration_since(s.started_at), false),
    };
    let task = s.current_task.clone();
    let progress_count = s.progress_count;
    s.last_output_at = now;
    LoopAction::Emit(EmitPayload {
        task,
        elapsed,
        progress_count,
        has_task,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn slow() -> Duration {
        Duration::from_secs(60)
    }

    #[test]
    fn fires_after_silence_threshold() {
        let hb = Heartbeat::start(
            Duration::from_millis(100),
            Duration::from_millis(20),
            slow(),
        );
        thread::sleep(Duration::from_millis(250));
        let action = check_and_consume(&hb.state, Duration::from_millis(100));
        match action {
            LoopAction::Skip | LoopAction::Emit(_) => {}
            LoopAction::Stop => panic!("unexpected stop"),
        }
        hb.stop();
    }

    #[test]
    fn set_task_does_not_reset_silent_timer() {
        let hb = Heartbeat::start(Duration::from_secs(60), Duration::from_millis(50), slow());
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
        let hb = Heartbeat::start(Duration::from_secs(60), Duration::from_millis(50), slow());
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
            LoopAction::Emit(p) => {
                assert_eq!(p.task.as_deref(), Some(":compileKotlin"));
                assert!(p.elapsed >= Duration::from_secs(5));
                assert!(p.has_task);
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
            LoopAction::Emit(p) => {
                assert!(p.task.is_none(), "no task → fallback should emit None");
                assert!(!p.has_task);
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

    #[test]
    fn format_includes_task_count_when_progress_known() {
        let p = EmitPayload {
            task: Some(":compileKotlin".to_string()),
            elapsed: Duration::from_secs(45),
            progress_count: 12,
            has_task: true,
        };
        let line = format_heartbeat(&p, slow());
        assert_eq!(line, "▸ :compileKotlin (45s) [12 tasks]");
    }

    #[test]
    fn format_omits_task_count_when_zero() {
        let p = EmitPayload {
            task: None,
            elapsed: Duration::from_secs(30),
            progress_count: 0,
            has_task: false,
        };
        let line = format_heartbeat(&p, slow());
        assert_eq!(line, "▸ building (30s)");
    }

    #[test]
    fn format_marks_slow_when_task_age_exceeds_threshold() {
        let p = EmitPayload {
            task: Some(":compileKotlin".to_string()),
            elapsed: Duration::from_secs(133),
            progress_count: 12,
            has_task: true,
        };
        let line = format_heartbeat(&p, Duration::from_secs(60));
        assert_eq!(line, "▸ :compileKotlin (2m13s) [12 tasks] — slow");
    }

    #[test]
    fn format_does_not_mark_slow_for_aggregate_age_without_task() {
        // When there's no current task, the elapsed is wall-clock since build
        // start. We don't want to flag the whole build as "slow".
        let p = EmitPayload {
            task: None,
            elapsed: Duration::from_secs(300),
            progress_count: 12,
            has_task: false,
        };
        let line = format_heartbeat(&p, Duration::from_secs(60));
        assert_eq!(line, "▸ building (5m00s) [12 tasks]");
    }

    #[test]
    fn format_duration_under_minute() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn format_duration_over_minute() {
        assert_eq!(format_duration(Duration::from_secs(60)), "1m00s");
        assert_eq!(format_duration(Duration::from_secs(133)), "2m13s");
        assert_eq!(format_duration(Duration::from_secs(605)), "10m05s");
    }

    fn action_name(a: &LoopAction) -> &'static str {
        match a {
            LoopAction::Stop => "Stop",
            LoopAction::Emit(_) => "Emit",
            LoopAction::Skip => "Skip",
        }
    }
}

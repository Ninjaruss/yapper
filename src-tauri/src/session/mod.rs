//! Pure session lifecycle clock. No I/O, no system time — callers pass
//! timestamps in, which keeps every path deterministic under test.
//! Callers must ensure timestamps are monotonically increasing per session.

use crate::error::YapperError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockState {
    Recording,
    Paused,
    Ended,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionTotals {
    pub ended_at_ms: i64,
    pub paused_ms: i64,
}

pub struct SessionClock {
    started_at_ms: i64,
    state: ClockState,
    paused_accum_ms: i64,
    paused_since_ms: Option<i64>,
}

impl SessionClock {
    pub fn start(now_ms: i64) -> Self {
        Self {
            started_at_ms: now_ms,
            state: ClockState::Recording,
            paused_accum_ms: 0,
            paused_since_ms: None,
        }
    }

    pub fn state(&self) -> ClockState {
        self.state
    }

    pub fn pause(&mut self, now_ms: i64) -> Result<(), YapperError> {
        if self.state != ClockState::Recording {
            return Err(YapperError::State("can only pause while recording".into()));
        }
        self.state = ClockState::Paused;
        self.paused_since_ms = Some(now_ms);
        Ok(())
    }

    pub fn resume(&mut self, now_ms: i64) -> Result<(), YapperError> {
        if self.state != ClockState::Paused {
            return Err(YapperError::State("can only resume while paused".into()));
        }
        self.paused_accum_ms += now_ms - self.paused_since_ms.take().unwrap();
        self.state = ClockState::Recording;
        Ok(())
    }

    /// Speaking time: wall time minus everything spent paused.
    pub fn elapsed_ms(&self, now_ms: i64) -> i64 {
        now_ms - self.started_at_ms - self.paused_ms(now_ms)
    }

    pub fn paused_ms(&self, now_ms: i64) -> i64 {
        match (self.state, self.paused_since_ms) {
            (ClockState::Paused, Some(since)) => self.paused_accum_ms + (now_ms - since),
            _ => self.paused_accum_ms,
        }
    }

    /// Finalize the session. Can be called from Recording or Paused state.
    pub fn end(&mut self, now_ms: i64) -> SessionTotals {
        let paused_ms = self.paused_ms(now_ms);
        self.state = ClockState::Ended;
        self.paused_since_ms = None;
        SessionTotals { ended_at_ms: now_ms, paused_ms }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_recording() {
        let c = SessionClock::start(1000);
        assert_eq!(c.state(), ClockState::Recording);
        assert_eq!(c.elapsed_ms(3000), 2000);
        assert_eq!(c.paused_ms(3000), 0);
    }

    #[test]
    fn pause_freezes_elapsed_and_accumulates_paused() {
        let mut c = SessionClock::start(1000);
        c.pause(2000).unwrap();
        assert_eq!(c.state(), ClockState::Paused);
        assert_eq!(c.elapsed_ms(5000), 1000); // frozen at pause point
        assert_eq!(c.paused_ms(5000), 3000);
        c.resume(6000).unwrap();
        assert_eq!(c.elapsed_ms(8000), 3000); // 1000 before + 2000 after
        assert_eq!(c.paused_ms(8000), 4000);
    }

    #[test]
    fn double_pause_and_resume_while_recording_are_errors() {
        let mut c = SessionClock::start(0);
        assert!(c.resume(10).is_err());
        c.pause(10).unwrap();
        assert!(c.pause(20).is_err());
    }

    #[test]
    fn end_returns_totals_and_ends_from_paused_too() {
        let mut c = SessionClock::start(1000);
        c.pause(2000).unwrap();
        let totals = c.end(4000);
        assert_eq!(totals.ended_at_ms, 4000);
        assert_eq!(totals.paused_ms, 2000);
        assert_eq!(c.state(), ClockState::Ended);
    }

    #[test]
    fn end_from_recording_without_pause() {
        let mut c = SessionClock::start(1000);
        let totals = c.end(6000);
        assert_eq!(totals.ended_at_ms, 6000);
        assert_eq!(totals.paused_ms, 0);
    }

    #[test]
    fn multiple_pause_resume_cycles_accumulate_correctly() {
        let mut c = SessionClock::start(1000);
        c.pause(2000).unwrap();
        c.resume(4000).unwrap();
        c.pause(5000).unwrap();
        c.resume(6000).unwrap();
        assert_eq!(c.paused_ms(6000), 3000);
        assert_eq!(c.elapsed_ms(6000), 2000);
    }
}

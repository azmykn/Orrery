//! In-memory activity log — recent app events (scans, fleet ops, push/pull
//! outcomes, soft CI errors) for the Log view. Ring buffer; not persisted.

use std::collections::VecDeque;

use gpui::SharedString;

use crate::toast::ToastKind;

/// How many entries the ring keeps (newest at the front for the Log view).
pub const CAPACITY: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    /// Soft problems that aren't hard failures (e.g. skipped ops).
    Warn,
    Error,
}

impl LogLevel {
    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

#[derive(Clone)]
pub struct LogEntry {
    pub id: u64,
    /// Unix seconds when the event was recorded.
    pub at: i64,
    pub level: LogLevel,
    pub message: SharedString,
}

#[derive(Default)]
pub struct ActivityLog {
    entries: VecDeque<LogEntry>,
    seq: u64,
}

impl ActivityLog {
    /// Newest-first snapshot for the Log view.
    pub fn entries(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Append one line. `at` is unix seconds (caller supplies so tests stay
    /// deterministic and toast paths can share a clock).
    pub fn push(&mut self, at: i64, level: LogLevel, message: impl Into<SharedString>) {
        self.seq += 1;
        self.entries.push_front(LogEntry {
            id: self.seq,
            at,
            level,
            message: message.into(),
        });
        while self.entries.len() > CAPACITY {
            self.entries.pop_back();
        }
    }

    /// Map a resolved toast into a log line (skips Progress — in-flight noise).
    pub fn record_toast(
        &mut self,
        at: i64,
        kind: ToastKind,
        title: &str,
        detail: Option<&str>,
    ) -> bool {
        let level = match kind {
            ToastKind::Progress => return false,
            ToastKind::Success => LogLevel::Info,
            ToastKind::Info => LogLevel::Warn,
            ToastKind::Error => LogLevel::Error,
        };
        let message = match detail.filter(|d| !d.is_empty()) {
            Some(d) => format!("{title} — {d}"),
            None => title.to_string(),
        };
        self.push(at, level, message);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_drops_oldest() {
        let mut log = ActivityLog::default();
        for i in 0..(CAPACITY + 5) {
            log.push(i as i64, LogLevel::Info, format!("e{i}"));
        }
        assert_eq!(log.entries().count(), CAPACITY);
        let newest = log.entries().next().unwrap();
        assert_eq!(newest.message.as_ref(), format!("e{}", CAPACITY + 4));
        let oldest = log.entries().last().unwrap();
        assert_eq!(oldest.message.as_ref(), "e5");
    }

    #[test]
    fn skips_progress_toasts() {
        let mut log = ActivityLog::default();
        assert!(!log.record_toast(1, ToastKind::Progress, "Pushing…", None));
        assert!(log.record_toast(1, ToastKind::Error, "Push failed", Some("denied")));
        assert_eq!(log.entries().count(), 1);
        assert_eq!(log.entries().next().unwrap().level, LogLevel::Error);
    }
}

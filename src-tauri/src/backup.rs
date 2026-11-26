//! When an automatic backup run is allowed to happen.
//!
//! The scheduler wakes up on a short fixed tick; this module answers "is a run due now",
//! so the rules stay testable without a clock or a running app. All times are local time.

use chrono::{DateTime, Datelike, Local, Timelike};

use crate::settings::BackupSettings;

/// ``"18:30"`` → minutes since midnight. Malformed values return ``None``.
pub fn minutes_of_day(value: &str) -> Option<u32> {
    let (hour, minute) = value.split_once(':')?;
    if hour.len() != 2 || minute.len() != 2 {
        return None;
    }
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

fn clock_minutes(now: DateTime<Local>) -> u32 {
    now.hour() * 60 + now.minute()
}

/// ``23:00 - 07:00`` wraps past midnight; ``start == end`` covers the whole day.
pub fn in_window(start: u32, end: u32, minute: u32) -> bool {
    if start == end {
        return true;
    }
    if start < end {
        (start..end).contains(&minute)
    } else {
        minute >= start || minute < end
    }
}

/// Weekday and upload-window rules. Only enforced when the window is turned on.
pub fn schedule_allows(backup: &BackupSettings, weekday: u32, minute: u32) -> bool {
    if !backup.window_enabled {
        return true;
    }
    if !backup.days.contains(&(weekday as u8)) {
        return false;
    }
    let (Some(start), Some(end)) = (
        minutes_of_day(&backup.window_start),
        minutes_of_day(&backup.window_end),
    ) else {
        return true;
    };
    let inside = in_window(start, end, minute);
    if backup.window_mode == "deny" {
        !inside
    } else {
        inside
    }
}

pub(super) fn automatic_allowed_now(backup: &BackupSettings, now: DateTime<Local>) -> bool {
    backup.mode != "manual"
        && schedule_allows(
            backup,
            now.weekday().num_days_from_monday(),
            clock_minutes(now),
        )
}

/// True when the scheduler should start a run now.
///
/// ``interval`` mode compares against the last run, ``times`` mode catches up to the newest
/// scheduled time of the day (so a sleeping Mac still uploads once it wakes), and ``manual``
/// never runs on its own.
pub fn due(
    backup: &BackupSettings,
    interval_seconds: u64,
    last_run: Option<DateTime<Local>>,
    now: DateTime<Local>,
) -> bool {
    if !automatic_allowed_now(backup, now) {
        return false;
    }
    match backup.mode.as_str() {
        "times" => {
            let now_minute = clock_minutes(now);
            let Some(scheduled) = backup
                .times
                .iter()
                .filter_map(|time| minutes_of_day(time))
                .filter(|minute| *minute <= now_minute)
                .max()
            else {
                return false;
            };
            match last_run {
                None => true,
                Some(last) => {
                    if last.date_naive() != now.date_naive() {
                        true
                    } else {
                        clock_minutes(last) < scheduled
                    }
                }
            }
        }
        _ => match last_run {
            None => true,
            Some(last) => (now - last).num_seconds() >= interval_seconds as i64,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, 2, hour, minute, 0) // 2026-11-02 is a Monday.
            .single()
            .expect("valid local time")
    }

    fn times(times: &[&str]) -> BackupSettings {
        BackupSettings {
            mode: "times".into(),
            times: times.iter().map(|value| value.to_string()).collect(),
            ..BackupSettings::default()
        }
    }

    #[test]
    fn parses_and_rejects_times() {
        assert_eq!(minutes_of_day("00:00"), Some(0));
        assert_eq!(minutes_of_day("23:59"), Some(1439));
        assert_eq!(minutes_of_day("24:00"), None);
        assert_eq!(minutes_of_day("8:00"), None);
        assert_eq!(minutes_of_day("18-30"), None);
    }

    #[test]
    fn window_wraps_past_midnight() {
        assert!(in_window(23 * 60, 7 * 60, 23 * 60 + 30));
        assert!(in_window(23 * 60, 7 * 60, 6 * 60));
        assert!(!in_window(23 * 60, 7 * 60, 12 * 60));
        assert!(in_window(9 * 60, 18 * 60, 9 * 60));
        assert!(!in_window(9 * 60, 18 * 60, 18 * 60));
        assert!(in_window(0, 0, 13 * 60));
    }

    #[test]
    fn deny_and_allow_windows_are_mirrors() {
        let mut backup = BackupSettings {
            window_enabled: true,
            window_start: "23:00".into(),
            window_end: "07:00".into(),
            ..BackupSettings::default()
        };
        let minute = 23 * 60 + 30;
        assert!(!schedule_allows(&backup, 0, minute));
        backup.window_mode = "allow".into();
        assert!(schedule_allows(&backup, 0, minute));
        assert!(!schedule_allows(&backup, 0, 12 * 60));
    }

    #[test]
    fn weekdays_only_apply_with_the_window() {
        let mut backup = BackupSettings::default();
        backup.days = vec![0];
        assert!(schedule_allows(&backup, 3, 12 * 60));
        backup.window_enabled = true;
        assert!(schedule_allows(&backup, 0, 12 * 60));
        assert!(!schedule_allows(&backup, 3, 12 * 60));
    }

    #[test]
    fn interval_mode_waits_for_the_interval() {
        let backup = BackupSettings::default();
        assert!(due(&backup, 1800, None, at(12, 0)));
        assert!(!due(&backup, 1800, Some(at(11, 45)), at(12, 0)));
        assert!(due(&backup, 1800, Some(at(11, 30)), at(12, 0)));
    }

    #[test]
    fn wake_scan_completion_restarts_the_interval_and_preserves_later_daily_slots() {
        let completed_at = at(10, 20);
        let interval = BackupSettings::default();
        assert!(!due(&interval, 1800, Some(completed_at), at(10, 49)));
        assert!(due(&interval, 1800, Some(completed_at), at(10, 50)));
        assert!(!due(&interval, 300, Some(completed_at), at(10, 24)));
        assert!(due(&interval, 300, Some(completed_at), at(10, 25)));

        let daily = times(&["09:00", "18:00"]);
        assert!(!due(&daily, 1800, Some(completed_at), at(17, 59)));
        assert!(due(&daily, 1800, Some(completed_at), at(18, 0)));
    }

    #[test]
    fn times_mode_catches_up_once_per_slot() {
        let backup = times(&["09:00", "18:00"]);
        assert!(!due(&backup, 1800, None, at(8, 0)));
        assert!(due(&backup, 1800, None, at(9, 5)));
        assert!(!due(&backup, 1800, Some(at(9, 5)), at(12, 0)));
        assert!(due(&backup, 1800, Some(at(9, 5)), at(18, 0)));
        assert!(!due(&backup, 1800, Some(at(18, 1)), at(23, 0)));
        // Yesterday's run does not cover today's first slot.
        let yesterday = Local
            .with_ymd_and_hms(2026, 11, 1, 18, 0, 0)
            .single()
            .expect("valid local time");
        assert!(due(&backup, 1800, Some(yesterday), at(9, 0)));
    }

    #[test]
    fn manual_mode_never_runs() {
        let backup = BackupSettings {
            mode: "manual".into(),
            ..BackupSettings::default()
        };
        assert!(!due(&backup, 60, None, at(12, 0)));
    }
}

use std::{fmt, str::FromStr};

const DAYS: [&str; 7] = [
    "sunday",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
];

pub struct Time {
    minute: u8,
    hour: Option<u8>,
    day: Option<usize>,
}

impl Time {
    pub fn cron(&self) -> String {
        let hour = self.hour.map_or("*".to_owned(), |hour| hour.to_string());
        let day = self.day.map_or("*".to_owned(), |day| day.to_string());
        format!("{} {hour} * * {day}", self.minute)
    }
}

impl FromStr for Time {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        // ASVS 2.2.1: only parsed numbers and day names reach the cron expression.
        let input = input.to_ascii_lowercase();
        let words: Vec<_> = input.split_whitespace().collect();
        let (clock, day) = match words.as_slice() {
            ["hourly"] | ["every", "hour"] => {
                return Ok(Self {
                    minute: 0,
                    hour: None,
                    day: None,
                });
            }
            [clock] | ["daily" | "everyday", "at", clock] | ["every", "day", "at", clock] => {
                (*clock, None)
            }
            ["every", day, "at", clock] => {
                let day = DAYS.iter().position(|name| name == day).ok_or_else(|| {
                    format!("\"{day}\" is not a weekday. Try Monday or Saturday.")
                })?;
                (*clock, Some(day))
            }
            _ => return Err("Use a time like 02:30, hourly, or every Sunday at 02:30.".to_owned()),
        };
        let (hour, minute) = clock
            .split_once(':')
            .ok_or_else(|| "Enter a 24-hour time like 02:30.".to_owned())?;
        let hour = hour
            .parse::<u8>()
            .ok()
            .filter(|_| hour.len() <= 2 && hour.bytes().all(|b| b.is_ascii_digit()))
            .filter(|hour| *hour < 24)
            .ok_or_else(|| "The hour must be between 00 and 23.".to_owned())?;
        let minute = minute
            .parse::<u8>()
            .ok()
            .filter(|_| minute.len() == 2 && minute.bytes().all(|b| b.is_ascii_digit()))
            .filter(|minute| *minute < 60)
            .ok_or_else(|| "The minute must be between 00 and 59.".to_owned())?;
        Ok(Self {
            minute,
            hour: Some(hour),
            day,
        })
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(hour) = self.hour {
            if let Some(day) = self.day {
                write!(f, "every {} at {hour:02}:{:02}", DAYS[day], self.minute)
            } else {
                write!(f, "daily at {hour:02}:{:02}", self.minute)
            }
        } else {
            f.write_str("hourly")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Time;

    #[test]
    fn accepts_daily_weekly_and_hourly_schedules() {
        for (input, cron) in [
            ("7:05", "5 7 * * *"),
            ("daily at 02:30", "30 2 * * *"),
            ("Every Sunday at 02:30", "30 2 * * 0"),
            ("every hour", "0 * * * *"),
        ] {
            assert_eq!(input.parse::<Time>().unwrap().cron(), cron);
        }
    }

    #[test]
    fn explains_invalid_times() {
        for (input, reason) in [
            ("daily at 25:30", "The hour"),
            ("every Sunday at 02:90", "The minute"),
            ("every Funday at 02:30", "is not a weekday"),
            ("tomorrow", "24-hour time"),
        ] {
            assert!(input.parse::<Time>().err().unwrap().contains(reason));
        }
    }
}

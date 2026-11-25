//! Bikram Sambat <-> Gregorian conversion.
//!
//! The month-length table is authoritative and generated (see `data.rs`); this
//! module is the only place that does calendar arithmetic, and it is tested
//! against every day in the supported range plus per-year anchors.

mod data;

pub use data::{BS_MAX_YEAR, BS_MIN_YEAR, EPOCH_AD, MONTH_DAYS};

use chrono::{Datelike, Duration, NaiveDate, Weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BsDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BsError {
    OutOfRange,
    InvalidDate,
}

impl std::fmt::Display for BsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BsError::OutOfRange => write!(
                f,
                "outside the supported range (BS {BS_MIN_YEAR}-{BS_MAX_YEAR})"
            ),
            BsError::InvalidDate => write!(f, "not a real date in the Bikram Sambat calendar"),
        }
    }
}

impl std::error::Error for BsError {}

pub const MONTH_NAMES_EN: [&str; 12] = [
    "Baisakh", "Jestha", "Ashar", "Shrawan", "Bhadra", "Asoj", "Kartik", "Mangsir", "Poush",
    "Magh", "Falgun", "Chait",
];

pub const MONTH_NAMES_NP: [&str; 12] = [
    "बैशाख",
    "जेठ",
    "असार",
    "साउन",
    "भदौ",
    "असोज",
    "कार्तिक",
    "मंसिर",
    "पुष",
    "माघ",
    "फागुन",
    "चैत",
];

pub const WEEKDAYS_NP: [&str; 7] = [
    "आइतबार",
    "सोमबार",
    "मंगलबार",
    "बुधबार",
    "बिहीबार",
    "शुक्रबार",
    "शनिबार",
];

pub const WEEKDAYS_EN: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateStyle {
    /// 2082-08-06
    Iso,
    /// 06/08/2082
    DayFirst,
    /// 2082 Ashoj 6 (Gregorian month order preserved: month/day)
    Long,
    /// २०८२-०८-०६
    Devanagari,
}

fn epoch() -> NaiveDate {
    let (y, m, d) = EPOCH_AD;
    NaiveDate::from_ymd_opt(y, m, d).expect("epoch is a valid date")
}

pub fn month_days(year: i32, month: u32) -> Option<u32> {
    if !(BS_MIN_YEAR..=BS_MAX_YEAR).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    Some(MONTH_DAYS[(year - BS_MIN_YEAR) as usize][(month - 1) as usize] as u32)
}

pub fn days_in_year(year: i32) -> Option<u32> {
    if !(BS_MIN_YEAR..=BS_MAX_YEAR).contains(&year) {
        return None;
    }
    Some(
        MONTH_DAYS[(year - BS_MIN_YEAR) as usize]
            .iter()
            .map(|d| *d as u32)
            .sum(),
    )
}

/// Total days covered by the table.
pub fn total_days() -> i64 {
    MONTH_DAYS
        .iter()
        .map(|year| year.iter().map(|d| *d as i64).sum::<i64>())
        .sum()
}

pub fn to_ad(year: i32, month: u32, day: u32) -> Result<NaiveDate, BsError> {
    let limit = month_days(year, month).ok_or(BsError::OutOfRange)?;
    if day < 1 || day > limit {
        return Err(BsError::InvalidDate);
    }
    let mut offset: i64 = 0;
    for y in BS_MIN_YEAR..year {
        offset += days_in_year(y).ok_or(BsError::OutOfRange)? as i64;
    }
    for m in 1..month {
        offset += month_days(year, m).ok_or(BsError::OutOfRange)? as i64;
    }
    offset += day as i64 - 1;
    Ok(epoch() + Duration::days(offset))
}

pub fn from_ad(date: NaiveDate) -> Result<BsDate, BsError> {
    let mut remaining = (date - epoch()).num_days();
    if remaining < 0 || remaining >= total_days() {
        return Err(BsError::OutOfRange);
    }
    for year in BS_MIN_YEAR..=BS_MAX_YEAR {
        let year_len = days_in_year(year).ok_or(BsError::OutOfRange)? as i64;
        if remaining < year_len {
            for month in 1..=12 {
                let month_len = month_days(year, month).ok_or(BsError::OutOfRange)? as i64;
                if remaining < month_len {
                    return Ok(BsDate {
                        year,
                        month,
                        day: remaining as u32 + 1,
                    });
                }
                remaining -= month_len;
            }
            return Err(BsError::OutOfRange);
        }
        remaining -= year_len;
    }
    Err(BsError::OutOfRange)
}

pub fn today() -> BsDate {
    from_ad(chrono::Local::now().date_naive()).unwrap_or(BsDate {
        year: BS_MAX_YEAR,
        month: 1,
        day: 1,
    })
}

impl BsDate {
    pub fn new(year: i32, month: u32, day: u32) -> Result<Self, BsError> {
        to_ad(year, month, day).map(|_| BsDate { year, month, day })
    }

    pub fn to_ad(self) -> Result<NaiveDate, BsError> {
        to_ad(self.year, self.month, self.day)
    }

    pub fn weekday(self) -> Option<Weekday> {
        self.to_ad().ok().map(|d| d.weekday())
    }

    pub fn weekday_en(self) -> &'static str {
        self.weekday()
            .map(|w| WEEKDAYS_EN[w.num_days_from_sunday() as usize])
            .unwrap_or("")
    }

    pub fn weekday_np(self) -> &'static str {
        self.weekday()
            .map(|w| WEEKDAYS_NP[w.num_days_from_sunday() as usize])
            .unwrap_or("")
    }

    pub fn month_en(self) -> &'static str {
        MONTH_NAMES_EN
            .get(self.month.saturating_sub(1) as usize)
            .copied()
            .unwrap_or("")
    }

    pub fn month_np(self) -> &'static str {
        MONTH_NAMES_NP
            .get(self.month.saturating_sub(1) as usize)
            .copied()
            .unwrap_or("")
    }

    pub fn days_in_month(self) -> u32 {
        month_days(self.year, self.month).unwrap_or(30)
    }

    pub fn add_days(self, days: i64) -> Result<BsDate, BsError> {
        from_ad(self.to_ad()? + Duration::days(days))
    }

    pub fn days_until(self, other: BsDate) -> Result<i64, BsError> {
        Ok((other.to_ad()? - self.to_ad()?).num_days())
    }

    pub fn format(self, style: DateStyle) -> String {
        match style {
            DateStyle::Iso => format!("{:04}-{:02}-{:02}", self.year, self.month, self.day),
            DateStyle::DayFirst => format!("{:02}/{:02}/{:04}", self.day, self.month, self.year),
            DateStyle::Long => format!("{} {}, {} BS", self.month_en(), self.day, self.year),
            DateStyle::Devanagari => to_devanagari_digits(&format!(
                "{:04}-{:02}-{:02}",
                self.year, self.month, self.day
            )),
        }
    }

    /// A stable, sortable key for calendar grids.
    pub fn key(self) -> i32 {
        self.year * 10_000 + self.month as i32 * 100 + self.day as i32
    }
}

/// Parse the shapes people actually type: 2082-08-06, 2082/8/6, ०८/०८/२०८२.
pub fn parse(input: &str) -> Option<BsDate> {
    let normalized = from_devanagari_digits(input.trim());
    let cleaned: String = normalized
        .chars()
        .map(|c| if c.is_ascii_digit() { c } else { ' ' })
        .collect();
    let parts: Vec<&str> = cleaned.split_whitespace().collect();
    let nums: Vec<i32> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    if nums.len() != 3 {
        return None;
    }
    let (a, b, c) = (nums[0], nums[1], nums[2]);
    // A 4-digit first or last component is the year.
    let (year, month, day) = if a > 99 {
        (a, b, c)
    } else if c > 99 {
        (c, b, a)
    } else {
        return None;
    };
    BsDate::new(year, month as u32, day as u32).ok()
}

pub fn to_devanagari_digits(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            '0'..='9' => char::from_u32('०' as u32 + (c as u32 - '0' as u32)).unwrap_or(c),
            _ => c,
        })
        .collect()
}

pub fn from_devanagari_digits(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            '\u{0966}'..='\u{096F}' => {
                char::from_u32('0' as u32 + (c as u32 - '\u{0966}' as u32)).unwrap_or(c)
            }
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_correct() {
        assert_eq!(
            to_ad(BS_MIN_YEAR, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(1918, 4, 13).unwrap()
        );
    }

    #[test]
    fn known_anchors() {
        let cases = [
            ((1975, 1, 1), (1918, 4, 13)),
            ((2000, 1, 1), (1943, 4, 14)),
            ((2082, 8, 6), (2025, 11, 22)),
            ((1990, 1, 15), (1933, 4, 27)),
            ((2100, 12, 30), (2044, 4, 12)),
        ];
        for ((by, bm, bd), (ay, am, ad)) in cases {
            let expected = NaiveDate::from_ymd_opt(ay, am, ad).unwrap();
            assert_eq!(to_ad(by, bm, bd).unwrap(), expected, "BS {by}-{bm}-{bd}");
            assert_eq!(
                from_ad(expected).unwrap(),
                BsDate {
                    year: by,
                    month: bm,
                    day: bd
                },
                "AD {ay}-{am}-{ad}"
            );
        }
    }

    #[test]
    fn every_year_start_matches_reference() {
        let csv = include_str!("../../tests/fixtures/bs_anchors.csv");
        let mut checked = 0;
        for line in csv.lines().skip(1) {
            let mut parts = line.split(',');
            let year: i32 = parts.next().unwrap().parse().unwrap();
            let ad: NaiveDate = parts.next().unwrap().parse().unwrap();
            assert_eq!(to_ad(year, 1, 1).unwrap(), ad, "year {year}");
            assert_eq!(from_ad(ad).unwrap().year, year);
            checked += 1;
        }
        assert_eq!(checked, (BS_MAX_YEAR - BS_MIN_YEAR + 1) as usize);
    }

    #[test]
    fn every_day_round_trips() {
        let mut date = to_ad(BS_MIN_YEAR, 1, 1).unwrap();
        let mut count = 0i64;
        while count < total_days() {
            let bs = from_ad(date).expect("in range");
            assert_eq!(bs.to_ad().unwrap(), date, "round trip failed at {date}");
            date += Duration::days(1);
            count += 1;
        }
        assert_eq!(count, total_days(), "table and iteration disagree");
    }

    #[test]
    fn rejects_bad_dates() {
        assert_eq!(to_ad(1974, 1, 1), Err(BsError::OutOfRange));
        assert_eq!(to_ad(2101, 1, 1), Err(BsError::OutOfRange));
        assert_eq!(to_ad(2082, 13, 1), Err(BsError::OutOfRange));
        assert_eq!(to_ad(2082, 1, 32), Err(BsError::InvalidDate));
        assert_eq!(
            from_ad(NaiveDate::from_ymd_opt(1918, 4, 12).unwrap()),
            Err(BsError::OutOfRange)
        );
        assert_eq!(
            from_ad(NaiveDate::from_ymd_opt(2044, 4, 13).unwrap()),
            Err(BsError::OutOfRange)
        );
    }

    #[test]
    fn month_lengths_are_plausible() {
        for year in BS_MIN_YEAR..=BS_MAX_YEAR {
            let total = days_in_year(year).unwrap();
            // The canonical table gives 2096 only 364 days; every other year is 365 or 366.
            assert!((364..=366).contains(&total), "{year} has {total} days");
            for month in 1..=12 {
                let days = month_days(year, month).unwrap();
                assert!((29..=32).contains(&days), "{year}-{month} has {days}");
            }
        }
    }

    #[test]
    fn weekday_matches_gregorian() {
        let bs = BsDate {
            year: 2082,
            month: 8,
            day: 6,
        };
        assert_eq!(bs.to_ad().unwrap().weekday(), bs.weekday().unwrap());
        assert_eq!(bs.weekday_np(), "शनिबार");
    }

    #[test]
    fn parsing_and_formatting() {
        assert_eq!(
            parse("2082-08-06"),
            Some(BsDate {
                year: 2082,
                month: 8,
                day: 6
            })
        );
        assert_eq!(
            parse("०६/०८/२०८२"),
            Some(BsDate {
                year: 2082,
                month: 8,
                day: 6
            })
        );
        assert_eq!(parse("6/8/2082").unwrap().day, 6);
        assert_eq!(parse("hello"), None);
        let d = BsDate {
            year: 2082,
            month: 8,
            day: 6,
        };
        assert_eq!(d.format(DateStyle::Iso), "2082-08-06");
        assert_eq!(d.format(DateStyle::DayFirst), "06/08/2082");
        assert_eq!(d.format(DateStyle::Devanagari), "२०८२-०८-०६");
        assert_eq!(to_devanagari_digits("2025"), "२०२५");
        assert_eq!(from_devanagari_digits("२०२५"), "2025");
    }

    #[test]
    fn arithmetic() {
        let d = BsDate {
            year: 2082,
            month: 8,
            day: 6,
        };
        assert_eq!(d.add_days(1).unwrap().day, 7);
        assert_eq!(d.add_days(-6).unwrap().day, 30); // previous month is 30 days
        assert_eq!(d.days_until(d.add_days(45).unwrap()).unwrap(), 45);
    }
}

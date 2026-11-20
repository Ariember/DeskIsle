//! The month the clock menu shows: six weeks from Sunday, each day with its lunar date,
//! solar term or festival.

use super::lunar;

/// Weeks in every month grid, so turning the month keeps the menu's size.
pub const WEEKS: usize = 6;
pub const WEEKDAYS: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];
/// The months the calendar turns through; the lunar tables end on either side.
pub const FIRST: (i32, u8) = (1901, 1);
pub const LAST: (i32, u8) = (2100, 12);

/// What the small line under a day's number says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    Lunar,
    Term,
    Festival,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Day {
    pub day: u8,
    /// The lunar day, the lunar month on its first day, a solar term or a festival.
    /// Empty outside the lunar tables.
    pub note: String,
    pub kind: Note,
    /// Belongs to the month shown rather than to a neighbor filling the first or last week.
    pub current: bool,
    pub today: bool,
    pub weekend: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Month {
    pub year: i32,
    pub month: u8,
    /// `WEEKS` weeks from the Sunday on or before the first of the month.
    pub days: Vec<Day>,
}

fn index((year, month): (i32, u8)) -> i32 {
    year * 12 + i32::from(month) - 1
}

/// The month `by` months after `year`-`month`, or `None` past the ends of the tables.
pub fn shift(year: i32, month: u8, by: i32) -> Option<(i32, u8)> {
    let target = index((year, month)) + by;
    let shifted = (target.div_euclid(12), (target.rem_euclid(12) + 1) as u8);
    (index(FIRST)..=index(LAST))
        .contains(&target)
        .then_some(shifted)
}

/// What a calendar writes under a date.
fn note(year: i32, month: u8, day: u8) -> (String, Note) {
    let date = lunar::lunar(year, month, day);
    if let Some(name) = lunar::festival(year, month, day, date) {
        return (name.into(), Note::Festival);
    }
    if let Some(term) = lunar::solar_term(year, month, day) {
        return (term.into(), Note::Term);
    }
    let text = match date {
        Some(l) if l.day == 1 => lunar::month_name(l.month, l.leap),
        Some(l) => lunar::day_name(l.day),
        None => String::new(),
    };
    (text, Note::Lunar)
}

/// The lunar date, year and any term or festival of `today`, for the clock menu's
/// header: "农历十月十三 · 丙午马年".
pub fn describe((year, month, day): (i32, u8, u8)) -> String {
    let Some(date) = lunar::lunar(year, month, day) else {
        return format!("{year}年");
    };
    let mut text = format!(
        "农历{}{} · {}",
        lunar::month_name(date.month, date.leap),
        lunar::day_name(date.day),
        lunar::year_name(date.year)
    );
    let (note, kind) = note(year, month, day);
    if kind != Note::Lunar {
        text.push_str(" · ");
        text.push_str(&note);
    }
    text
}

impl Month {
    pub fn new(year: i32, month: u8, today: (i32, u8, u8)) -> Self {
        let first = lunar::days_from_civil(year, month, 1);
        let start = first - i64::from(lunar::weekday(year, month, 1));
        let days = (0..WEEKS as i64 * 7)
            .map(|offset| {
                let (y, m, d) = lunar::civil_from_days(start + offset);
                let (note, kind) = note(y, m, d);
                Day {
                    day: d,
                    note,
                    kind,
                    current: (y, m) == (year, month),
                    today: (y, m, d) == today,
                    weekend: matches!(offset % 7, 0 | 6),
                }
            })
            .collect();
        Self { year, month, days }
    }

    pub fn title(&self) -> String {
        format!("{}年{}月", self.year, self.month)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_starts_on_sunday_and_marks_the_month_and_today() {
        let month = Month::new(2026, 11, (2026, 11, 21));
        assert_eq!(month.title(), "2026年11月");
        assert_eq!(month.days.len(), WEEKS * 7);
        // November 2026 begins on a Sunday, so the grid ends in December.
        assert_eq!((month.days[0].day, month.days[0].current), (1, true));
        assert_eq!(month.days[0].note, "廿三");
        let at = |day: u8| {
            month
                .days
                .iter()
                .find(|d| d.current && d.day == day)
                .unwrap()
        };
        assert_eq!((at(7).note.as_str(), at(7).kind), ("立冬", Note::Term));
        assert_eq!(at(9).note, "十月");
        assert_eq!(at(22).note, "小雪");
        assert!(at(21).today && at(21).weekend && at(21).note == "十三");
        assert!(!at(20).weekend && at(29).weekend);
        assert_eq!(month.days.iter().filter(|d| d.today).count(), 1);
        let last = month.days.last().unwrap();
        assert!(!last.current && last.day == 12);
    }

    #[test]
    fn leading_days_come_from_the_previous_month() {
        let month = Month::new(2026, 10, (2026, 11, 21));
        // October 2026 begins on a Thursday.
        let leading: Vec<u8> = month.days[..4].iter().map(|d| d.day).collect();
        assert_eq!(leading, [27, 28, 29, 30]);
        assert!(month.days[..4].iter().all(|d| !d.current));
        assert!(month.days[4].current && month.days[4].day == 1);
        let national = month.days.iter().find(|d| d.current && d.day == 1).unwrap();
        assert_eq!(
            (national.note.as_str(), national.kind),
            ("国庆节", Note::Festival)
        );
        assert!(month.days.iter().all(|d| !d.today));
    }

    #[test]
    fn months_turn_across_years_and_stop_at_the_tables() {
        assert_eq!(shift(2026, 12, 1), Some((2027, 1)));
        assert_eq!(shift(2026, 1, -1), Some((2025, 12)));
        assert_eq!(shift(2026, 11, -23), Some((2024, 12)));
        assert_eq!(shift(2100, 12, 1), None);
        assert_eq!(shift(1901, 1, -1), None);
    }

    #[test]
    fn header_reads_the_lunar_date_and_year() {
        assert_eq!(describe((2026, 11, 21)), "农历十月十三 · 丙午马年");
        assert_eq!(describe((2026, 11, 22)), "农历十月十四 · 丙午马年 · 小雪");
        // The lunar year changes at the Spring Festival, not on January 1.
        assert_eq!(describe((2026, 2, 1)), "农历腊月十四 · 乙巳蛇年");
        assert_eq!(describe((2200, 1, 1)), "2200年");
    }
}

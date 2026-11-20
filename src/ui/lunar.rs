//! The Chinese calendar beside the Gregorian one: lunar dates, the 24 solar terms and
//! the festivals a desktop calendar marks, from tables that cover 1900 to 2100.

use super::lunar_data::{TERMS, YEARS};

const FIRST_YEAR: i32 = 1900;
const LAST_YEAR: i32 = 2100;

const TERM_NAMES: [&str; 24] = [
    "小寒", "大寒", "立春", "雨水", "惊蛰", "春分", "清明", "谷雨", "立夏", "小满", "芒种", "夏至",
    "小暑", "大暑", "立秋", "处暑", "白露", "秋分", "寒露", "霜降", "立冬", "小雪", "大雪", "冬至",
];

/// Days from 1970-01-01 to a proleptic Gregorian date, after Howard Hinnant's algorithm.
pub fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let (m, d) = (i64::from(month), i64::from(day));
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The Gregorian date `days` after 1970-01-01.
pub fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year as i32, month as u8, day as u8)
}

/// The day of the week, 0 for Sunday.
pub fn weekday(year: i32, month: u8, day: u8) -> u8 {
    // 1970-01-01 was a Thursday.
    (days_from_civil(year, month, day) + 4).rem_euclid(7) as u8
}

/// A day of the Chinese lunar calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LunarDate {
    /// The Gregorian year the lunar year begins in.
    pub year: i32,
    pub month: u8,
    pub day: u8,
    /// The intercalary month that repeats `month`.
    pub leap: bool,
    /// Days in this lunar month, 29 or 30.
    pub days: u8,
}

fn info(year: i32) -> u32 {
    YEARS[(year - FIRST_YEAR) as usize]
}

fn leap_month(year: i32) -> u8 {
    (info(year) & 0xf) as u8
}

fn month_days(year: i32, month: u8) -> u8 {
    if info(year) & (0x10000 >> month) != 0 {
        30
    } else {
        29
    }
}

fn leap_days(year: i32) -> u8 {
    match (leap_month(year), info(year) & 0x10000 != 0) {
        (0, _) => 0,
        (_, true) => 30,
        (_, false) => 29,
    }
}

fn year_days(year: i32) -> i64 {
    (1..=12)
        .map(|month| i64::from(month_days(year, month)))
        .sum::<i64>()
        + i64::from(leap_days(year))
}

/// The lunar date of a Gregorian date from 1900-01-31, the first day of lunar 1900,
/// through 2100-12-31.
pub fn lunar(year: i32, month: u8, day: u8) -> Option<LunarDate> {
    if year > LAST_YEAR {
        return None;
    }
    let mut offset = days_from_civil(year, month, day) - days_from_civil(FIRST_YEAR, 1, 31);
    if offset < 0 {
        return None;
    }
    let mut year = FIRST_YEAR;
    while offset >= year_days(year) {
        offset -= year_days(year);
        year += 1;
        if year > LAST_YEAR {
            return None;
        }
    }
    let leap = leap_month(year);
    for month in 1..=12 {
        for is_leap in [false, true] {
            let days = match is_leap {
                false => month_days(year, month),
                true if month == leap => leap_days(year),
                true => continue,
            };
            if offset < i64::from(days) {
                return Some(LunarDate {
                    year,
                    month,
                    day: offset as u8 + 1,
                    leap: is_leap,
                    days,
                });
            }
            offset -= i64::from(days);
        }
    }
    None
}

/// The solar term that falls on a Gregorian date, by the time in China.
pub fn solar_term(year: i32, month: u8, day: u8) -> Option<&'static str> {
    if !(FIRST_YEAR..=LAST_YEAR).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    let days = &TERMS[(year - FIRST_YEAR) as usize];
    let first = usize::from(month - 1) * 2;
    (first..first + 2)
        .find(|&term| days[term] == day)
        .map(|term| TERM_NAMES[term])
}

/// A lunar month as a calendar names it, such as "正月", "闰六月" or "腊月".
pub fn month_name(month: u8, leap: bool) -> String {
    const NAMES: [&str; 12] = [
        "正", "二", "三", "四", "五", "六", "七", "八", "九", "十", "冬", "腊",
    ];
    let leap = if leap { "闰" } else { "" };
    format!("{leap}{}月", NAMES[usize::from(month.clamp(1, 12) - 1)])
}

/// A lunar day as a calendar names it, from "初一" to "三十".
pub fn day_name(day: u8) -> String {
    const DIGITS: [&str; 10] = ["一", "二", "三", "四", "五", "六", "七", "八", "九", "十"];
    const TENS: [&str; 3] = ["初", "十", "廿"];
    match day {
        10 => "初十".into(),
        20 => "二十".into(),
        30 => "三十".into(),
        _ => {
            let index = usize::from(day.clamp(1, 29) - 1);
            format!("{}{}", TENS[index / 10], DIGITS[index % 10])
        }
    }
}

/// The sexagenary name and zodiac animal of a lunar year, such as "丙午马年".
pub fn year_name(year: i32) -> String {
    const STEMS: [&str; 10] = ["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸"];
    const BRANCHES: [&str; 12] = [
        "子", "丑", "寅", "卯", "辰", "巳", "午", "未", "申", "酉", "戌", "亥",
    ];
    const ANIMALS: [&str; 12] = [
        "鼠", "牛", "虎", "兔", "龙", "蛇", "马", "羊", "猴", "鸡", "狗", "猪",
    ];
    let cycle = (year - 4).rem_euclid(60) as usize;
    let branch = cycle % 12;
    format!(
        "{}{}{}年",
        STEMS[cycle % 10],
        BRANCHES[branch],
        ANIMALS[branch]
    )
}

/// The festival on a Gregorian date whose lunar date is `lunar`. Traditional festivals
/// come before the Gregorian ones.
pub fn festival(year: i32, month: u8, day: u8, lunar: Option<LunarDate>) -> Option<&'static str> {
    if let Some(l) = lunar.filter(|l| !l.leap) {
        let name = match (l.month, l.day) {
            (1, 1) => Some("春节"),
            (1, 15) => Some("元宵节"),
            (5, 5) => Some("端午节"),
            (7, 7) => Some("七夕"),
            (8, 15) => Some("中秋节"),
            (9, 9) => Some("重阳节"),
            (12, 8) => Some("腊八"),
            (12, 23) => Some("小年"),
            (12, day) if day == l.days => Some("除夕"),
            _ => None,
        };
        if name.is_some() {
            return name;
        }
    }
    let sunday = weekday(year, month, day) == 0;
    match (month, day) {
        (1, 1) => Some("元旦"),
        (2, 14) => Some("情人节"),
        (3, 8) => Some("妇女节"),
        (5, 1) => Some("劳动节"),
        (5, 4) => Some("青年节"),
        // The second Sunday of May and the third of June.
        (5, 8..=14) if sunday => Some("母亲节"),
        (6, 15..=21) if sunday => Some("父亲节"),
        (6, 1) => Some("儿童节"),
        (9, 10) => Some("教师节"),
        (10, 1) => Some("国庆节"),
        (12, 24) => Some("平安夜"),
        (12, 25) => Some("圣诞节"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u8, day: u8, leap: bool) -> (i32, u8, u8, bool) {
        (year, month, day, leap)
    }

    fn lunar_of(year: i32, month: u8, day: u8) -> (i32, u8, u8, bool) {
        let l = lunar(year, month, day).expect("inside the tables");
        (l.year, l.month, l.day, l.leap)
    }

    #[test]
    fn gregorian_days_round_trip_and_name_weekdays() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(
            civil_from_days(days_from_civil(2026, 11, 21)),
            (2026, 11, 21)
        );
        assert_eq!(
            civil_from_days(days_from_civil(2024, 2, 29) + 1),
            (2024, 3, 1)
        );
        assert_eq!(
            civil_from_days(days_from_civil(1900, 2, 28) + 1),
            (1900, 3, 1)
        );
        assert_eq!(weekday(2026, 11, 21), 6);
        assert_eq!(weekday(2026, 11, 1), 0);
        assert_eq!(weekday(2000, 1, 1), 6);
    }

    #[test]
    fn lunar_dates_match_published_calendars() {
        assert_eq!(lunar_of(2026, 11, 21), date(2026, 10, 13, false));
        assert_eq!(lunar_of(2026, 11, 9), date(2026, 10, 1, false));
        assert_eq!(lunar_of(2026, 2, 17), date(2026, 1, 1, false));
        assert_eq!(lunar_of(2026, 2, 16), date(2025, 12, 29, false));
        assert_eq!(lunar_of(2025, 1, 29), date(2025, 1, 1, false));
        // Leap months: the second month of 2023, the sixth of 2025, the eleventh of 2033.
        assert_eq!(lunar_of(2023, 3, 22), date(2023, 2, 1, true));
        assert_eq!(lunar_of(2025, 7, 25), date(2025, 6, 1, true));
        assert_eq!(lunar_of(2033, 12, 22), date(2033, 11, 1, true));
        assert_eq!(lunar_of(1900, 1, 31), date(1900, 1, 1, false));
        assert_eq!(lunar_of(2100, 12, 31), date(2100, 12, 1, false));
        assert_eq!(lunar(1900, 1, 30), None);
        assert_eq!(lunar(2101, 1, 1), None);
    }

    #[test]
    fn solar_terms_fall_on_their_days() {
        assert_eq!(solar_term(2026, 11, 7), Some("立冬"));
        assert_eq!(solar_term(2026, 11, 22), Some("小雪"));
        assert_eq!(solar_term(2026, 11, 21), None);
        assert_eq!(solar_term(2025, 2, 3), Some("立春"));
        assert_eq!(solar_term(2024, 4, 4), Some("清明"));
        assert_eq!(solar_term(2025, 12, 21), Some("冬至"));
        assert_eq!(solar_term(2026, 12, 22), Some("冬至"));
        assert_eq!(solar_term(2101, 1, 5), None);
    }

    #[test]
    fn names_read_as_a_calendar_writes_them() {
        assert_eq!(day_name(1), "初一");
        assert_eq!(day_name(10), "初十");
        assert_eq!(day_name(13), "十三");
        assert_eq!(day_name(20), "二十");
        assert_eq!(day_name(21), "廿一");
        assert_eq!(day_name(30), "三十");
        assert_eq!(month_name(1, false), "正月");
        assert_eq!(month_name(6, true), "闰六月");
        assert_eq!(month_name(11, false), "冬月");
        assert_eq!(month_name(12, false), "腊月");
        assert_eq!(year_name(2026), "丙午马年");
        assert_eq!(year_name(2024), "甲辰龙年");
    }

    #[test]
    fn festivals_follow_both_calendars() {
        let on = |y, m, d| festival(y, m, d, lunar(y, m, d));
        assert_eq!(on(2026, 2, 17), Some("春节"));
        assert_eq!(on(2026, 2, 16), Some("除夕"));
        assert_eq!(on(2025, 1, 28), Some("除夕"));
        assert_eq!(on(2026, 9, 25), Some("中秋节"));
        assert_eq!(on(2026, 10, 1), Some("国庆节"));
        assert_eq!(on(2026, 5, 10), Some("母亲节"));
        assert_eq!(on(2026, 6, 21), Some("父亲节"));
        assert_eq!(on(2026, 11, 21), None);
        // 2025 repeats its sixth month, which moves 七夕 into late August.
        assert_eq!(on(2025, 8, 29), Some("七夕"));
        // A repeated month keeps its festivals on the first one.
        let repeated = LunarDate {
            year: 2028,
            month: 5,
            day: 5,
            leap: true,
            days: 29,
        };
        assert_eq!(festival(2028, 6, 27, Some(repeated)), None);
    }
}

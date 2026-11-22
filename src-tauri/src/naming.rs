//! What a new clip's file is called, from the template in Settings → Recording.
//!
//! Tokens: {game} {date} (2026-11-22) {time} (14-05-33) {date-us} (11-22-2026)
//! {date-eu} (22-11-2026) {time12} (2-05-33 PM) {year} {month} {day} {hour}
//! {minute} {second} {pc} (this PC's name). Anything else is kept as typed;
//! characters Windows won't allow in a file name become spaces.

use chrono::{DateTime, Local};

/// "Valorant 2026-11-22 14-05-33", as Hindsight has always named clips.
pub const DEFAULT_TEMPLATE: &str = "{game} {date} {time}";

/// The presets Settings offers (the template, and what it's called there).
pub const PRESETS: [(&str, &str); 5] = [
    ("{game} {date} {time}", "Game, date, time"),
    ("{date} {time} {game}", "Date, time, game"),
    ("{game} {date-us} {time12}", "Game, US date, 12-hour time"),
    ("{game} {date-eu} {time}", "Game, day-month-year, time"),
    ("{game} {month}-{day} {time12}", "Game, month-day, 12-hour time"),
];

/// The file name (no extension) for a clip of `game` saved at `now`. A recording
/// gets "Recording" after the game's name (or first, if the template has no {game}).
pub fn file_stem(template: &str, game: &str, recording: bool, now: DateTime<Local>, pc: &str) -> String {
    let template = if template.trim().is_empty() { DEFAULT_TEMPLATE } else { template };
    let game_part = if recording { format!("{game} Recording") } else { game.to_string() };
    let mut s = template.to_string();
    if recording && !template.contains("{game}") {
        s = format!("Recording {s}");
    }
    let hour12 = match now.format("%I").to_string().trim_start_matches('0') {
        "" => "12".to_string(),
        h => h.to_string(),
    };
    for (token, value) in [
        ("{game}", game_part),
        ("{date-us}", now.format("%m-%d-%Y").to_string()),
        ("{date-eu}", now.format("%d-%m-%Y").to_string()),
        ("{date}", now.format("%Y-%m-%d").to_string()),
        ("{time12}", format!("{hour12}-{} {}", now.format("%M-%S"), now.format("%p"))),
        ("{time}", now.format("%H-%M-%S").to_string()),
        ("{year}", now.format("%Y").to_string()),
        ("{month}", now.format("%m").to_string()),
        ("{day}", now.format("%d").to_string()),
        ("{hour}", now.format("%H").to_string()),
        ("{minute}", now.format("%M").to_string()),
        ("{second}", now.format("%S").to_string()),
        ("{pc}", pc.to_string()),
    ] {
        s = s.replace(token, &value);
    }
    let cleaned = crate::consolidate::safe_name(&s);
    // Nothing usable typed (only symbols, or empty): the default instead.
    if cleaned == "Other" && !s.contains("Other") {
        return file_stem(DEFAULT_TEMPLATE, game, recording, now, pc);
    }
    cleaned.chars().take(150).collect::<String>().trim_end_matches(['.', ' ']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 22, 14, 5, 33).unwrap()
    }

    #[test]
    fn the_default_is_the_old_name() {
        assert_eq!(file_stem(DEFAULT_TEMPLATE, "Valorant", false, at(), ""), "Valorant 2026-11-22 14-05-33");
        assert_eq!(file_stem("", "Valorant", false, at(), ""), "Valorant 2026-11-22 14-05-33");
        assert_eq!(file_stem(DEFAULT_TEMPLATE, "Valorant", true, at(), ""), "Valorant Recording 2026-11-22 14-05-33");
    }

    #[test]
    fn every_preset_and_token() {
        assert_eq!(file_stem("{date} {time} {game}", "Apex Legends", false, at(), ""), "2026-11-22 14-05-33 Apex Legends");
        assert_eq!(file_stem("{game} {date-us} {time12}", "Apex", false, at(), ""), "Apex 11-22-2026 2-05-33 PM");
        assert_eq!(file_stem("{game} {date-eu} {time}", "Apex", false, at(), ""), "Apex 22-11-2026 14-05-33");
        assert_eq!(file_stem("{game} {month}-{day} {time12}", "Apex", false, at(), ""), "Apex 11-22 2-05-33 PM");
        assert_eq!(file_stem("{year}.{month}.{day} {hour}h{minute} {pc}", "Apex", false, at(), "DESK"), "2026.11.22 14h05 DESK");
        let midnight = Local.with_ymd_and_hms(2026, 1, 2, 0, 7, 9).unwrap();
        assert_eq!(file_stem("{time12}", "x", false, midnight, ""), "12-07-09 AM");
    }

    #[test]
    fn odd_templates_stay_valid_file_names() {
        assert_eq!(file_stem("{game}: <best> {date}", "Apex", false, at(), ""), "Apex best 2026-11-22");
        assert_eq!(file_stem("???", "Apex", false, at(), ""), "Apex 2026-11-22 14-05-33");
        assert_eq!(file_stem("clip {date}", "Apex", true, at(), ""), "Recording clip 2026-11-22");
    }
}

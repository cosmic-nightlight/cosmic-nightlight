// SPDX-License-Identifier: MPL-2.0

//! Real sunrise and sunset times for wherever the machine is.
//!
//! Backing "Sunset to Sunrise" with the actual sun is what makes it a schedule
//! nobody has to configure, so the location has to come from somewhere. The
//! obvious somewheres are all bad: a location service is another daemon and
//! another portal hole, a network lookup is a privacy cost for a night light,
//! and asking the user for coordinates is the chore this mode exists to avoid.
//!
//! So this takes the fourth option, which is the one `cosmic-settings-daemon`
//! itself takes for the light/dark theme switch: read the configured time zone,
//! look its coordinates up in the tz database already on disk, and do the solar
//! math in process. No daemon, no network, no permission, and no question put to
//! the user. A time zone locates you to a few hundred kilometers, which moves
//! sunset by minutes — far inside the margin that matters for tinting a screen.

use std::sync::Mutex;

use chrono::{DateTime, Datelike, Local, NaiveDate, Timelike};

/// Where the tz database keeps its zone tables, in the order they are tried.
///
/// `zone1970.tab` is the current one and lists a canonical zone per region;
/// `zone.tab` is the backward-compatible one that still carries the older names
/// (`US/Central` and friends), which is what a long-lived install can be set to.
/// Reading both means either name resolves.
const ZONE_TABLES: [&str; 2] = [
    "/usr/share/zoneinfo/zone1970.tab",
    "/usr/share/zoneinfo/zone.tab",
];

/// The zenith angle of the sun's center at the moment we call it sunrise.
///
/// Not 90°. The sun is a disc rather than a point, so its upper limb clears the
/// horizon while the center is still below it, and the atmosphere refracts the
/// whole thing upward on top of that. The two together are worth about 50
/// arcminutes, which is the convention every almanac uses.
const SUNRISE_ZENITH_DEGREES: f64 = 90.833;

/// A position on the globe, in degrees: latitude north-positive, longitude
/// east-positive. Both are the sign convention the solar math below assumes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coordinates {
    pub latitude: f64,
    pub longitude: f64,
}

/// Sunrise and sunset as minutes since local midnight, matching how the rest of
/// the app stores times of day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SunTimes {
    pub sunrise_minutes: u32,
    pub sunset_minutes: u32,
}

/// Everything a day's worth of sun costs a file read to work out.
#[derive(Clone, Copy)]
struct Today {
    date: NaiveDate,
    /// Whether a location could be worked out at all — a different question
    /// from whether there were any times, since a polar winter has a location
    /// and no sunrise. Kept here rather than re-derived on demand so that
    /// asking costs nothing: the settings window asks on every render.
    located: bool,
    times: Option<SunTimes>,
}

/// Today's answer, remembered until the local date rolls over.
///
/// Every run mode asks for this on its tick, and the two GUIs ask again on every
/// render, so it must not re-read the zone tables each time. It changes once a
/// day, so cache it for exactly that long: the entry is keyed by the local date,
/// which also means a machine left running overnight recomputes on its own
/// rather than tinting to yesterday's sun — and picks up a time zone changed by
/// travel within a day of it happening.
static TODAY: Mutex<Option<Today>> = Mutex::new(None);

/// Today's sunrise and sunset here, or `None` if there is no location to work
/// from or the sun doesn't cross the horizon today.
pub fn today() -> Option<SunTimes> {
    current().times
}

/// Whether a location could be worked out at all, which is a different question
/// from whether the sun rose today. The settings window asks so it can say which
/// of the two happened.
pub fn have_location() -> bool {
    current().located
}

/// The cached day, computing it if the date has moved on.
fn current() -> Today {
    let date = Local::now().date_naive();

    // A panic elsewhere while this was held would poison the lock, and a night
    // light that stops following the sun for the rest of the session is a worse
    // outcome than reusing whatever was cached. Nothing here can leave the entry
    // half-written anyway: it is one assignment of a `Copy` value.
    let mut cached = TODAY.lock().unwrap_or_else(|err| err.into_inner());

    if let Some(today) = *cached {
        if today.date == date {
            return today;
        }
    }

    let coordinates = local_coordinates();
    let today = Today {
        date,
        located: coordinates.is_some(),
        times: coordinates.and_then(|at| sun_times_on(at, date)),
    };
    *cached = Some(today);
    today
}

/// The coordinates of the configured time zone, or `None` if the zone can't be
/// read or isn't in the tables.
fn local_coordinates() -> Option<Coordinates> {
    let zone = timezone_name()?;
    coordinates_for(&zone)
}

/// The configured time zone's name, e.g. `"America/Chicago"`.
///
/// `/etc/localtime` is the authority and is a symlink into the zone directory on
/// every current distribution, including inside a flatpak sandbox, where it is
/// bind-mounted from the host. `/etc/timezone` is the Debian-family fallback for
/// the installs that copy the zone file into place instead of linking it.
fn timezone_name() -> Option<String> {
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        if let Some((_, zone)) = target.to_string_lossy().split_once("/zoneinfo/") {
            return Some(zone.to_owned());
        }
    }

    let name = std::fs::read_to_string("/etc/timezone").ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Looks a zone name up in the tz database's zone tables.
fn coordinates_for(zone: &str) -> Option<Coordinates> {
    for path in ZONE_TABLES {
        let Ok(table) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(found) = find_zone(&table, zone) {
            return Some(found);
        }
    }
    None
}

/// Scans one zone table for `zone`. Split from the file reading so the parsing
/// can be tested against a table written out by hand.
///
/// The format is tab-separated `country codes`, `coordinates`, `TZ`, and an
/// optional comment, with `#` comment lines throughout.
fn find_zone(table: &str, zone: &str) -> Option<Coordinates> {
    for line in table.lines() {
        if line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(_codes), Some(coordinates), Some(name)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if name == zone {
            return parse_coordinates(coordinates);
        }
    }
    None
}

/// Parses the table's ISO 6709 coordinate field, which is a latitude and a
/// longitude run together with no separator beyond the second one's sign —
/// `+415100-0873900`, or `+4439-06336` where the seconds are omitted.
///
/// They are told apart by width rather than by any delimiter: latitude takes two
/// degree digits and longitude three, so the split is found by looking for the
/// second sign.
fn parse_coordinates(field: &str) -> Option<Coordinates> {
    // From index 1, so the leading sign of the latitude isn't what we find.
    let split = field.get(1..)?.find(['+', '-'])? + 1;
    let (latitude, longitude) = field.split_at(split);

    Some(Coordinates {
        latitude: parse_angle(latitude, 2)?,
        longitude: parse_angle(longitude, 3)?,
    })
}

/// Parses one `±DD[D]MM[SS]` angle into degrees. `degree_digits` is 2 for a
/// latitude and 3 for a longitude, which is the only thing distinguishing them.
fn parse_angle(text: &str, degree_digits: usize) -> Option<f64> {
    let sign = match text.as_bytes().first()? {
        b'+' => 1.0,
        b'-' => -1.0,
        _ => return None,
    };

    let digits = text.get(1..)?;
    if digits.len() < degree_digits + 2 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    let part = |from: usize, to: usize| -> Option<f64> { digits.get(from..to)?.parse().ok() };
    let degrees = part(0, degree_digits)?;
    let minutes = part(degree_digits, degree_digits + 2)?;
    // Seconds are optional, and absent in about half the table's rows.
    let seconds = match digits.len() >= degree_digits + 4 {
        true => part(degree_digits + 2, degree_digits + 4)?,
        false => 0.0,
    };

    Some(sign * (degrees + minutes / 60.0 + seconds / 3600.0))
}

/// Sunrise and sunset at `at` on `date`, as minutes since local midnight.
///
/// `None` at latitudes where the sun doesn't cross the horizon on `date` — a
/// polar summer or winter, where there is genuinely no sunset to schedule from.
fn sun_times_on(at: Coordinates, date: NaiveDate) -> Option<SunTimes> {
    let (sunrise, sunset) = sun_times_utc(at, date)?;

    Some(SunTimes {
        sunrise_minutes: to_local_minutes(date, sunrise)?,
        sunset_minutes: to_local_minutes(date, sunset)?,
    })
}

/// The solar math proper: sunrise and sunset as minutes from *UTC* midnight on
/// `date`. Either can fall outside `0..1440`, since UTC midnight has nothing to
/// do with the local day.
///
/// This is NOAA's general solar position calculation: approximate the sun's
/// declination and the equation of time from the day of the year, then solve for
/// the hour angle at which the sun sits at [`SUNRISE_ZENITH_DEGREES`]. It is
/// good to about a minute anywhere outside the polar circles, which is finer
/// than the schedule's own resolution.
///
/// Kept separate from the local-time conversion so it can be checked against
/// published almanac times, which are a fact about the sky rather than about
/// whichever time zone the test happens to run in.
fn sun_times_utc(at: Coordinates, date: NaiveDate) -> Option<(f64, f64)> {
    let year = date.year();
    let days_in_year = match (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
        true => 366.0,
        false => 365.0,
    };

    // How far around its orbit the year has gone, in radians, taken at solar
    // noon — which is what makes this good enough to compute once for the whole
    // day rather than iterating towards each event separately.
    let gamma = std::f64::consts::TAU / days_in_year * (date.ordinal() as f64 - 1.0);
    let (sin1, cos1) = gamma.sin_cos();
    let (sin2, cos2) = (2.0 * gamma).sin_cos();
    let (sin3, cos3) = (3.0 * gamma).sin_cos();

    // The gap between clock noon and the sun's actual noon, in minutes. It runs
    // to a quarter of an hour either way over a year, from the orbit being an
    // ellipse and the axis being tilted.
    let equation_of_time =
        229.18 * (0.000075 + 0.001868 * cos1 - 0.032077 * sin1 - 0.014615 * cos2 - 0.040849 * sin2);

    // The sun's declination — how far north or south of the equator it stands
    // today — in radians.
    let declination = 0.006918 - 0.399912 * cos1 + 0.070257 * sin1 - 0.006758 * cos2
        + 0.000907 * sin2
        - 0.002697 * cos3
        + 0.001480 * sin3;

    let latitude = at.latitude.to_radians();
    let hour_angle = SUNRISE_ZENITH_DEGREES.to_radians().cos()
        / (latitude.cos() * declination.cos())
        - latitude.tan() * declination.tan();

    // Outside ±1 the sun's path misses the horizon altogether: it is up all day
    // or down all day, and `acos` has no answer because there isn't one.
    if !(-1.0..=1.0).contains(&hour_angle) {
        return None;
    }
    let hour_angle = hour_angle.acos().to_degrees();

    // Four minutes per degree of rotation, measured out from solar noon in both
    // directions.
    Some((
        720.0 - 4.0 * (at.longitude + hour_angle) - equation_of_time,
        720.0 - 4.0 * (at.longitude - hour_angle) - equation_of_time,
    ))
}

/// Turns minutes from UTC midnight on `date` into minutes since *local*
/// midnight, which is what the schedule compares against.
///
/// Going through a real instant is what makes the local offset — including
/// whether daylight saving is in force on that date — come out of the tz
/// database rather than out of arithmetic here.
fn to_local_minutes(date: NaiveDate, utc_minutes: f64) -> Option<u32> {
    let midnight = date.and_hms_opt(0, 0, 0)?.and_utc();
    let instant = DateTime::from_timestamp(
        midnight.timestamp() + (utc_minutes * 60.0).round() as i64,
        0,
    )?;
    let local = instant.with_timezone(&Local);
    Some(local.hour() * 60 + local.minute())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two rows in each of the table's two shapes: seconds present and omitted.
    const TABLE: &str = "\
#codes\tcoordinates\tTZ\tcomments
US\t+404251-0740023\tAmerica/New_York\tEastern (most areas)
CA\t+4439-06336\tAmerica/Halifax\tAtlantic - NS (most areas), PE
";

    fn close(actual: f64, expected: f64, tolerance: f64, what: &str) {
        assert!(
            (actual - expected).abs() < tolerance,
            "{what}: got {actual}, expected about {expected}"
        );
    }

    #[test]
    fn coordinates_parse_with_seconds() {
        let at = find_zone(TABLE, "America/New_York").expect("New York is in the table");
        close(at.latitude, 40.7142, 0.001, "latitude");
        close(at.longitude, -74.0064, 0.001, "longitude");
    }

    /// About half the table's rows give degrees and minutes only.
    #[test]
    fn coordinates_parse_without_seconds() {
        let at = find_zone(TABLE, "America/Halifax").expect("Halifax is in the table");
        close(at.latitude, 44.65, 0.001, "latitude");
        close(at.longitude, -63.6, 0.001, "longitude");
    }

    #[test]
    fn an_unlisted_zone_is_not_invented() {
        assert_eq!(find_zone(TABLE, "Mars/Olympus_Mons"), None);
        assert_eq!(find_zone(TABLE, "#codes"), None, "the header is not a zone");
    }

    #[test]
    fn malformed_coordinates_are_rejected() {
        assert_eq!(parse_coordinates(""), None);
        assert_eq!(parse_coordinates("+4042"), None, "no longitude");
        assert_eq!(parse_coordinates("404251-0740023"), None, "no leading sign");
        assert_eq!(parse_coordinates("+40425x-0740023"), None, "not digits");
    }

    /// The real zone table, if this machine has one. Guards against the format
    /// or the path drifting out from under the parser.
    #[test]
    fn the_installed_zone_table_still_parses() {
        let Some(zone) = timezone_name() else {
            return;
        };
        let Some(at) = coordinates_for(&zone) else {
            return;
        };
        assert!((-90.0..=90.0).contains(&at.latitude), "latitude {at:?}");
        assert!((-180.0..=180.0).contains(&at.longitude), "longitude {at:?}");
    }

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a real date")
    }

    const NEW_YORK: Coordinates = Coordinates {
        latitude: 40.7128,
        longitude: -74.0060,
    };

    /// Checked against published almanac times. Five minutes of tolerance is
    /// far tighter than the error a time zone's worth of position introduces,
    /// so this is testing the math rather than the location.
    #[test]
    fn solstice_times_match_the_almanac() {
        // 2026-06-21, New York: sunrise 05:25 EDT, sunset 20:31 EDT — 09:25 and
        // 00:31 the next day in UTC.
        let (sunrise, sunset) = sun_times_utc(NEW_YORK, day(2026, 6, 21)).expect("a June sunrise");
        close(sunrise, 9.0 * 60.0 + 25.0, 5.0, "June sunrise");
        close(sunset, 24.0 * 60.0 + 31.0, 5.0, "June sunset");

        // 2026-12-21, New York: sunrise 07:17 EST, sunset 16:32 EST — 12:17 and
        // 21:32 in UTC.
        let (sunrise, sunset) =
            sun_times_utc(NEW_YORK, day(2026, 12, 21)).expect("a December sunrise");
        close(sunrise, 12.0 * 60.0 + 17.0, 5.0, "December sunrise");
        close(sunset, 21.0 * 60.0 + 32.0, 5.0, "December sunset");
    }

    /// The southern hemisphere runs the other way round, which catches a sign
    /// error in the declination that the New York checks alone would not.
    #[test]
    fn the_southern_hemisphere_runs_the_other_way() {
        let sydney = Coordinates {
            latitude: -33.8688,
            longitude: 151.2093,
        };
        let daylight = |date| {
            let (sunrise, sunset) = sun_times_utc(sydney, date).expect("Sydney has a sunrise");
            sunset - sunrise
        };
        // Sydney sits closer to the equator than New York, so its solstices are
        // only about four and a half hours apart rather than six. Three is the
        // margin that tests the direction without pinning the exact figure.
        let (december, june) = (daylight(day(2026, 12, 21)), daylight(day(2026, 6, 21)));
        assert!(
            december > june + 3.0 * 60.0,
            "December is Sydney's long day, not its short one: \
             got {december} vs {june} minutes"
        );
    }

    /// The whole point of the mode: the window has to move over the year. If
    /// these came out equal the schedule would be a fixed one wearing a
    /// different name.
    #[test]
    fn the_window_moves_across_the_year() {
        let daylight = |date| {
            let (sunrise, sunset) = sun_times_utc(NEW_YORK, date).expect("New York has a sunrise");
            sunset - sunrise
        };
        let june = daylight(day(2026, 6, 21));
        let december = daylight(day(2026, 12, 21));
        assert!(
            june - december > 5.0 * 60.0,
            "New York's longest day should beat its shortest by hours, \
             got {june} vs {december} minutes"
        );
    }

    /// Above the Arctic Circle in midsummer the sun never sets, and the caller
    /// has to be told that rather than handed a `NaN`.
    #[test]
    fn polar_summer_has_no_sunset() {
        let tromso = Coordinates {
            latitude: 69.6496,
            longitude: 18.9560,
        };
        assert_eq!(sun_times_on(tromso, day(2026, 6, 21)), None, "polar day");
        assert_eq!(sun_times_on(tromso, day(2026, 12, 21)), None, "polar night");
    }

    /// Whatever comes back has to be a time of day, since it is compared
    /// directly against the clock. This one goes through the local conversion,
    /// so it holds whatever time zone the test runs in.
    #[test]
    fn times_land_inside_a_single_day() {
        let times = sun_times_on(NEW_YORK, day(2026, 3, 15)).expect("a March sunrise");
        assert!(times.sunrise_minutes < 24 * 60, "{times:?}");
        assert!(times.sunset_minutes < 24 * 60, "{times:?}");
    }
}

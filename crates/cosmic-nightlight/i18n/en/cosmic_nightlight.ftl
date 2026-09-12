### Night Light for COSMIC — source strings.
###
### This is the fallback catalog: every message the app can show must exist
### here, and `fl!("id")` is a compile error if it doesn't. Translations live
### beside it at `i18n/<language>/cosmic_nightlight.ftl` and may omit any
### message, which then falls back to the English below.
###
### Two things to know before editing a value:
###
### - Fluent keeps the line breaks you type. A value wrapped across two lines
###   renders as two lines, so the long descriptions here each sit on one long
###   line on purpose. Wrap only where you want a break on screen.
### - `{ $name }` is a placeholder the app fills in. Keep every one that appears
###   in the English, spelled the same; move them freely within the sentence.
###
### See docs/translating.md for how to add a language.

## The application itself

# The app's name. Used for the panel tooltip, the toggle that turns the tint on
# and off, and the heading of the section holding the controls.
app-name = Night Light
settings-title = Night Light Settings
# The row at the foot of the applet popup that opens the settings window. The
# trailing "..." is the desktop convention for "this opens another window" and
# matches COSMIC's own applets; use whatever your language does for that.
applet-settings-button = Night Light Settings...

## Controls shared by the applet popup and the settings window

# Sits under the whole group of controls rather than under any one of them: the
# toggle and both sliders all cost a flicker. "May" is accuracy rather than
# softening — how long the flicker lasts depends on the graphics card.
flicker-note = Night light changes may briefly flicker the screen

# { $kelvin } is a colour temperature, already formatted, e.g. "5300". "K" is
# the unit symbol for kelvin.
temperature = Temperature: { $kelvin }K
# The captions under the two ends of the temperature slider. The Kelvin readout
# is exact but says nothing about which way is warmer, and a lower number
# reading as *more* orange is not something to make anyone infer.
warmth-less = Less warm
warmth-more = More warm

# { $percent } is already formatted, e.g. "80".
brightness = Brightness: { $percent }%
brightness-description = Dims the screen while the night light is on

## The line under the Night Light toggle

# Shown when no schedule is set, so there is nothing to count down to.
status-on = On
status-off = Off
# { $time } is a time of day, formatted to the user's 12- or 24-hour setting,
# e.g. "6:45AM" or "06:45".
status-on-until = On Until { $time }
status-off-until = Off Until { $time }

## Schedule

# Both the heading of the Schedule section and the label of the row holding the
# dropdown, which say the same word.
schedule = Schedule

# The three schedule dropdown options, in the order they appear.
schedule-off = Off
schedule-solar = Sunset to Sunrise
schedule-custom = Custom Schedule

# Labels for the two time pickers: when the tint turns on, and when it turns off.
schedule-from = From
schedule-to = To

# One line under the dropdown saying what the schedule works out to today.
# { $from } and { $to } are times of day. The all-day case is a schedule whose
# two ends are the same time.
schedule-summary-window = Warm from { $from } to { $to }
schedule-summary-all-day = Warm all day from { $from }

# Shown instead of that summary when "Sunset to Sunrise" has no sun to follow
# and has fallen back to the times in the pickers below. Both name the cause:
# the pickers appear alongside them, and without a reason their turning up
# under "Sunset to Sunrise" reads as a bug.
schedule-no-sunset = The sun doesn't set here today, using the times below
schedule-no-location = No location for your time zone, using the times below

# The AM/PM dropdown, shown only where the user's clock is set to 12 hours, and
# appended to times in that format ("6:45AM"). Languages that don't normally use
# a 12-hour clock can still be asked for these, because the clock format comes
# from a COSMIC setting rather than from the language.
meridiem-am = AM
meridiem-pm = PM

## The one-time host setup (flatpak builds only)

# Says *why* a password is about to be asked for, because the place it actually
# gets asked cannot: the prompt takes its wording from the program's path, and a
# flatpak's path carries a commit hash, so no rule can be written to match it.
# This row is the only place the reason fits.
setup-title = Set up Night Light
setup-description = Turning on the night light needs a one-time system permission.
setup-action = Set Up

setup-outdated-title = Update the installed helper
setup-outdated-description = The helper Night Light installed was put there by an older version and no longer understands this one. Running the setup again replaces it.
setup-action-update = Update

# On the button while the password prompt is up.
setup-working = Working…

# Appended after a blank line when an action didn't take. { $error } is a
# technical reason, currently always in English — see docs/translating.md.
action-failed = That didn't work: { $error }.

## The banner shown when a schedule has nothing running it

banner-title = Your schedule isn't running
banner-description = Night Light keeps to its schedule through the applet on your panel, or while this window is open. Add the applet, or let it run in the background instead.
banner-add-to-panel = Add to Panel

## The Background section

background = Background
# Named for what it does: it starts a background process now *and* arranges for
# one at each login. A name mentioning only the login half would describe the
# half a user turning it on today is not asking for. Used both as the label of
# the toggle and on the banner button above, so it has to read as a button too.
background-toggle = Run in Background
background-description = Keeps the schedule running when this window is closed, and starts again at each login.
# Shown instead when the applet is on the panel and already doing this job.
background-description-redundant = Not needed: the Night Light applet is on your panel and already keeps the schedule. Turning this off stops the duplicate background process.

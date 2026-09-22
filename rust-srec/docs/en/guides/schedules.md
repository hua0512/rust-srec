# Recording schedules

Use a time-based filter for weekly recording hours or a Cron filter for recurring rules. Configure the filter for the streamer, select its timezone, and confirm the effective schedule shown on the filter card.

## Example: evening recordings

1. Add a time-based filter to the streamer.
2. Select the weekdays and a recording interval, such as 20:00–23:00.
3. Select **Server local** or enter an IANA timezone such as `Europe/Madrid`.
4. Save the filter and check the timezone shown on its card.

To record across midnight, use an overnight interval such as 22:00–02:00. It belongs to the weekday on which it starts. The schedule permits recording; the channel must also be live and a recording slot available.

## Filter Timezones and Boundaries {#filter-timezones-and-boundaries}

Backend `TIME_BASED` and `CRON` filter JSON accept an optional IANA `timezone`, for
example `"Europe/Madrid"`, or `"local"` for the backend system timezone and its DST
rules. Matching and next-start/next-stop use that zone consistently.
An overnight interval belongs to its starting weekday. During a repeated clock
hour, its start uses the earlier instant and its end uses the later instant;
overlapping or touching intervals remain continuously matched. A missing local
boundary advances to the first valid minute within three hours; larger skipped
date windows are omitted.

Omitted or null timezone now means UTC for both rule types. Upgrades preserve
existing TimeBased omissions by storing explicit `"local"`; existing explicit
IANA zones and Cron rules keep their meaning. Both filter forms let you choose
UTC or **Server local**, or enter an IANA name. Server local means the backend
server's timezone, which may differ from the browser's timezone. The filter cards
show the effective timezone beside the schedule. Editing preserves the stored
zone; clearing the timezone control explicitly selects UTC. New filters and
filter-type changes also start with UTC. Changing the zone keeps the entered
clock times, interpreting those times in the newly selected zone.

For API clients, a same-type TimeBased update that omits the timezone member
retains its stored value. Send explicit null or `"UTC"` to switch it to UTC.
Replacing a Cron configuration without a timezone uses UTC.

Backup schema `0.1.8` exports an explicit timezone for both types. Importing a
TimeBased omission from schema `0.1.7` or earlier retains local-time behavior;
new-schema omissions mean UTC. `"local"` continues to follow the destination
server's timezone, as legacy local rules did. Unknown JSON members are retained.

Cron matching has minute granularity, including expressions containing
seconds; stop scans remain bounded to eight days.

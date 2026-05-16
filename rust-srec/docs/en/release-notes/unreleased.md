# Release Notes

## `unreleased`

### Reliability

- Fixed a case where a streamer would appear offline on the web UI (and stop recording) after a transient CDN failure such as an HTTP 404 on the signed FLV URL. The hysteresis-resume path now restores the streamer's live state in the database, and the container no longer aborts the resumed download on a stale cached state.

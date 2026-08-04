# Release Notes

## `unreleased`

## Streamers and sites

- **Bigo streamers are picked up as soon as they go live**

  Bigo checks shared one access token, but Bigo grants such a token a single use — every later check got a reply without the stream's address and read it as "not live". Each check now takes its own token, and a missing address is reported as a failed check rather than an offline room.

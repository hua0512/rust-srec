# Release Notes

## `unreleased`

- New: GPU health is now tracked on the System Health page. If your container loses GPU access (a known issue with the NVIDIA Container Toolkit on cgroup v2 hosts), you'll get a notification right away instead of finding out from the next failed remux job. The probe interval is configurable from the global settings page.

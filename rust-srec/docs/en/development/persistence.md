# Persistence contracts

For contributors changing database transactions and import behavior. Operators should start with [Backup and restore](../operations/backup-restore.md) and [Deletion semantics](../operations/data-governance.md).

## Import Persistence Ownership {#import-persistence-ownership}

Import and ordinary repository writes share the same typed field bindings. Import
reserves one SQLite write transaction for all eleven configuration domains,
subscription/filter replacement, email-slot swaps and authentication invalidation.
Existing IDs and creation times remain stable; caller-selected update times and
raw JSON are preserved. A backup without an extractor setting sets it to
inherit, like other settings a backup leaves out.

Cache invalidation and runtime notifications occur only after commit. A late
validation or database failure rolls back the import without publishing changes;
a post-commit reload failure remains a warning about already-committed state.

## Concurrent Database Updates {#concurrent-database-updates}

Concurrent deletions of the same media-output record subtract its size from the
session total once. If updating that total fails, the record deletion rolls back.
This database guarantee does not make optional filesystem deletion transactional.

Credential refreshes (`CredentialExecutionService` / `CredentialProfileRepository`)
commit updated cookies, tokens, and `credential_profile_health` inside one write
transaction only when the `credential_profiles` row is still active and its
material `revision` matches the snapshot that started the refresh; matching
refreshes advance `revision` and `version` atomically, whereas stale refreshes
are discarded instead of overwriting newer edits or imports. Concurrent streamer
error increments each return the count produced by their own update.

## Query batching

Pipeline job pages resolve display names through deduplicated streamer batches,
with at most 500 IDs per query. Missing streamers have no display name. A failed batch is retried by owner,
retaining names that can be read without failing the job page.

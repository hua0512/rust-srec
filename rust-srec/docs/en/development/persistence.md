# Persistence contracts

For contributors changing database transactions and import behavior. Operators should start with [Backup and restore](../operations/backup-restore.md) and [Deletion semantics](../operations/data-governance.md).

## Import Persistence Ownership {#import-persistence-ownership}

Import and ordinary repository writes share the same typed field bindings. Import
still reserves one SQLite write transaction for all ten configuration domains,
subscription/filter replacement, email-slot swaps and authentication invalidation.
Existing IDs and creation times remain stable; caller-selected update times and
raw JSON are preserved. Extractor settings absent from the backup format retain
their existing values, and new rows keep the same omitted-column defaults.

Cache invalidation and runtime notifications occur only after commit. A late
validation or database failure rolls back the import without publishing changes;
a post-commit reload failure remains a warning about already-committed state.

## Concurrent Database Updates {#concurrent-database-updates}

Concurrent deletions of the same media-output record subtract its size from the
session total once. If updating that total fails, the record deletion rolls back.
This database guarantee does not make optional filesystem deletion transactional.

Template credential refresh reads and updates the template in one reserved write
transaction, preserving other configuration edits committed before it. Concurrent
streamer error increments each return the count produced by their own update.

## Query batching

Pipeline job pages resolve display names through deduplicated streamer batches,
with at most 500 IDs per query. Job order and response fields are unchanged.
Missing streamers still have no display name. A failed batch is retried by owner,
retaining names that can be read without failing the job page.

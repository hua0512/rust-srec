# Multiple cookie profiles: decision record

Status: implemented on `main` (unreleased). This record describes the design as it stands and the decisions behind it. User-facing behavior is documented in [configuration](../rust-srec/docs/en/concepts/configuration.md#account-profiles-and-selection) and the [override reference](../rust-srec/docs/en/reference/configuration-overrides.md).

## Scope

A credential profile is one account's complete authentication bundle for one platform: a cookie string (which may hold many `name=value` pairs) plus optional refresh/access tokens and platform login material. Several profiles are several accounts; their cookies are never concatenated.

Users save profiles on a platform and choose, per platform, template override or streamer, which account(s) to use. Out of scope: weighted/random/least-loaded selection, per-account recording quotas, proxy pools (a scope or account chooses one connection), recording one streamer with several accounts at once, distributed multi-process coordination, a secrets-encryption system, and multi-account CLI configuration.

## Profiles and storage

- Profiles belong to a platform ([migration](../rust-srec/migrations/20260930120000_add_credential_profiles.sql), [repository](../rust-srec/src/database/repositories/credential_profiles.rs)). Templates and streamers select from their platform's profiles but own none; a selection can never name another platform's profile.
- `revision` changes when authentication material, the account's proxy route, the address or username of the saved proxy it names, or the enabled state changes (including a successful refresh); `version` changes on every edit and guards optimistic management writes. Label edits do not change the revision.
- Material is validated at the boundary: cookies must be a valid HTTP header value, login-only bundles must carry the platform's required fields, labels are 1–128 characters. Material types never derive `Serialize`/`Debug`; summaries expose only presence flags and the account's proxy route.
- `credential_profile_health` holds one row per profile at the revision that produced it: validity (`unknown`, `valid`, `needs_refresh`, `invalid`, CHECK-constrained), last check/refresh, refresh-failure count and window, and a reason code. A revision change discards stale health. Throttling is not account health: it belongs to a connection (see [Proxies and routes](#proxies-and-routes)). Validity and reason codes are typed enums whose snake_case spelling is identical in SQLite and JSON.
- Selections are rows ([repository](../rust-srec/src/database/repositories/credential_selections.rs)): `credential_selections` holds one row per platform, per template and platform, or per streamer, with mode and pool settings; `credential_selection_members` holds the ordered profile IDs. A scope without a row inherits, so `inherit` is never stored.
- SQLite enforces the references: a member's composite key `(profile_id, platform_config_id)` must match a profile of the selection's own platform, a selected profile cannot be deleted (`RESTRICT`), and deleting a platform, template or streamer removes its selections (`CASCADE`). Triggers remove a streamer's selection when it is marked deleted and when it moves to another platform, so it inherits there.
- Rust checks what SQLite cannot express, when a selection is written: member counts per mode, profiles being retired, and that the owner exists (a template not retired, a streamer not deleted) on the selection's platform. A missing or foreign member fails with `CREDENTIAL_REFERENCE_INACCESSIBLE` naming the scope. Writing the selection already stored is a no-op.
- The API and backups keep the selection inside the configuration it belongs to: `credential_selection` on the platform (a JSON string), inside `platform_overrides[<exact platform name>]` of a template, and inside a streamer's `streamer_specific_config`. Configuration writes split it out of the stored document and readers put the stored one back, so the wire and backup formats did not change. An omitted selection keeps the stored one; `inherit` removes it. A streamer form that repeats the old platform's selection while its URL moves it to another platform does not restore it.
- The Streamlink pseudo-platform (`platform_name` `streamlink`, seeded as `platform-streamlink`) takes selections only per streamer, and only `none` or `fixed`. The repository write refuses a platform or template selection there, or a pool, with `CREDENTIAL_SELECTION_PER_STREAMER` (422), and resolution ignores platform and template layers on it, so inheriting is anonymous. Profiles are still created and managed on its platform page.
- A profile selected by a configuration or bound to an active session cannot be deleted; the refusal lists the selecting scopes and sessions. Disabling is always allowed.
- `live_sessions.credential_binding` holds the non-secret binding (identity, revision, policy owner/generation, epoch), never material.

## Selection policies

[`selection.rs`](../rust-srec/src/credentials/selection.rs)

| Mode | Behavior |
| --- | --- |
| `inherit` | Continue to the next layer; at the platform it means no account. |
| `none` | Stop inheritance; no stored account material or automatic login. |
| `fixed` | Exactly one profile; repair and re-login target it; never switches. |
| `pool`, `priority` (default) | Start with the first eligible profile in saved order; later ones are backups. |
| `pool`, `round_robin` | Start each independent operation at the next eligible profile after the policy's cursor. |

- Resolution order: streamer, the template's selection for the streamer's platform, the platform. The first stored selection wins; lists are never merged. No accounts at global scope. Unavailable credentials never fall back to anonymous access.
- Resolved policies are cached with the merged configuration. Platform and template writes invalidate it through the existing owner publication; a streamer write whose only change is its selection marks the streamer reconfigured, because the stored row compares equal.
- Pools: nonempty, unique ordered IDs; `failover` (default `true`); `max_attempts` 1–10 (default 3). Omitted pool fields are written out explicitly on save, so the policy generation does not depend on whether a client omitted them.
- Ineligible: disabled or `invalid`. Unknown/unvalidated profiles are eligible.
- Round-robin cursors are in memory, keyed by policy generation, shared by every streamer inheriting the same policy, and reset on restart. Status reads never advance them.
- The policy generation is a fingerprint of canonical policy content plus resolved owner/platform; session epochs, not the fingerprint, order binding changes.

## Providers

Everything platform-specific about accounts lives in one [`CredentialProvider`](../rust-srec/src/credentials/provider.rs) per platform: Bilibili, Douyu, SOOP and Twitch have their own, and every other platform uses the cookie-only default.

- `capabilities()` states which material a profile accepts (refresh token, access token, token-only sign-in, username/password) and which provider calls exist (check, refresh, QR login). Profile validation, login-field isolation, the account UI (`GET /api/credentials/capabilities`) and the execution service all follow it.
- `extractor_authentication()` maps the account to extractor settings: SOOP's login and Twitch's OAuth token.
- `check()` reports `valid`, `repairable` (a refresh may recover it), `revoked` (only a new sign-in does) or `unverifiable` (nothing to check; health stays unchecked and recording proceeds).
- `refresh()` takes and returns material; QR login is `start_qr_login()`/`poll_qr_login()`.
- `refreshable()` says whether one account can be refreshed; the profile view offers refresh only then. `renew_after()` is an age after which a refreshable account is refreshed before use, for sessions that lapse without any request failing. The age counts from the last refresh at the current revision, else from the profile's last change. A renewal failure that does not demand a login keeps the current material and waits an hour before the next attempt.
- Douyu ([provider](../rust-srec/src/credentials/platforms/douyu.rs), [passport protocol](../crates/platforms/src/extractor/platforms/douyu/passport.rs)): QR login creates a web device ID, stored as both `dy_did` and `acf_did` so app playback signs with it. The main-site cookies (`acf_uid`, `acf_auth`, …) become the profile's cookies and the passport's `LTP0` its refresh token, kept out of the cookies sent with recordings. Between polls, the login's provider auth code holds the scan code and the passport cookies. Sessions last about six days and are renewed through `safeAuth` with `dy_did` and `LTP0` once four days old. Pasted cookies renew too when they include `dy_did` and `LTP0` is the refresh token or a cookie. Douyu has no account check. A signed-in account (one whose cookies hold `acf_auth`) goes with the play requests: the app request adds `acf_uid`, `acf_auth` and a matching `dy_did` to its device cookie but keeps an empty `token`, the Android app's own login, which web cookies do not provide; the web request sends the account's cookies and signs with its `dy_did`. Without `acf_auth` both requests are the anonymous ones.
- Every provider call receives the HTTP client for the account's connection, so checks, refreshes, renewals and QR sign-in leave through the account's own route, else the platform's, else the global route.

The rules are a static lookup by platform name. Provider calls go through the registry, which enables the built-in providers only in the application, so tests reach no provider unless they register a stub.

## Proxies and routes

[`proxies`](../rust-srec/src/proxies.rs), [migration](../rust-srec/migrations/20261007120000_named_proxies.sql), [repository](../rust-srec/src/database/repositories/proxies.rs)

- A saved proxy is a `proxies` row: a name unique without regard to case, a canonical `scheme://host[:port]` URL (`http`, `https`, `socks5`, `socks5h`), and an optional username and password stored as a pair. A unique index on (URL, username) makes one entry one exit: proxy services often assign one exit address per login. Entries are validated on every write, including import, so no client falls back to a direct connection on an unparsable address; a login embedded in the URL is refused in favour of the separate fields.
- Global, platform, template, streamer and account rows store a route, `inherit | direct | system | proxy(id)`, in `proxy_route` and `proxy_id` columns, with `REFERENCES proxies ON DELETE RESTRICT` and a CHECK pairing them. The global route cannot inherit; for an account, inherit means following the operation's route. A trigger resets the route of a streamer marked deleted. The API carries a streamer's route inside `streamer_specific_config`, split out on write and put back on read; writes change a route only when the request gives one. A request carrying a non-null `proxy_config` is rejected with `PROXY_CONFIG_REPLACED`.
- Resolution: an operation uses the account's own route if it has one, else the first non-inheriting route among streamer, template, platform and global. Account management (check, refresh, renewal, QR login) uses the account's route, else the platform's, else global. A route naming a missing entry fails; it never falls back to direct. The result, a `ResolvedRoute`, carries the `ProxyTarget` (direct, system, or an explicit endpoint whose login stays in separate fields), the `RouteKey` and the deciding source; it is fixed in the credential snapshot, so extraction, download, danmu and managed playback of one attempt share it.
- Direct means no proxy anywhere: in-process clients disable proxies, and FFmpeg, Streamlink and the Streamlink extractor are started without `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY`. System leaves in-process clients and engines on the environment (and, in-process, the operating system's settings); danmu uses the environment snapshot taken at startup, honouring `NO_PROXY`, and connects directly when none is set. FFmpeg refuses explicit proxies other than `http`. In-process clients, mesio and danmu receive an explicit proxy's login as fields; only the FFmpeg and Streamlink command lines carry it, percent-encoded, in the proxy URL.
- Danmu tunnels its WebSocket through the recording's route (HTTP CONNECT; TLS to the proxy, then CONNECT, for `https`; SOCKS5 with login, resolving names itself for `socks5` and leaving them to the proxy for `socks5h`) and hands protocols an HTTP client on the same route for their pre-connect requests. A proxy it cannot use leaves danmu off rather than connecting directly. A later attempt that changes the route reconnects the running collector.
- A connection is a `RouteKey`: Direct, System, or one entry. System shares Direct's key when no environment proxy was detected. Admission backoff is keyed by (platform, route key); the token-bucket rate stays per platform. Every pause sends `PlatformThrottled` with the route kind and the entry's name.
- No extractor can attribute a throttle to one account (a 429 says nothing about another account from the same address), so throttles are never account health. With failover, a throttle drops the remaining candidates on the same route and continues with the rest; candidates whose route is still paused are ordered last.
- Editing an entry applies at the next resolution; in-flight snapshots keep their route. A URL or username change starts a new revision of every account pinned to the entry, drops their health and clears the entry's backoff; a rename or password change does not.
- QR login sessions store the target's route, never a secret; a route whose entry was deleted meanwhile is a conflict.
- Responses show an entry's username, never its password. Backups carry entries with their logins and name them in routes; IDs never appear in bundles.

## Execution

[`execution.rs`](../rust-srec/src/credentials/execution.rs) is the single entry for monitor checks, parse, recovery and URL renewal. Callers pass an extraction closure; the service owns selection, status checks, repair, health publication, failover and attempt accounting.

1. Resolve the policy from the stored selections of the streamer, its template and its platform. No selection, or `none`, runs once anonymously under platform admission.
2. Select candidates: a bound session's account first (or, during owned recovery, the next saved account after it), otherwise the round-robin start or the priority order. Without failover only the first candidate is tried.
3. Per attempt: run at most one status check and one repair per account per operation (status is cached per UTC day), wait for admission on the account's connection, re-read the account, then extract.
4. Classify: success clears unavailability and stores minted session cookies at the exact revision; an account authentication failure queues one repair or marks the account invalid; a throttle pauses the account's connection and continues with candidates on other connections, ending the operation when none remain; anything else is an ordinary error.
5. A change to the account mid-attempt retries it once from its new state. Exhaustion is reported once as `CredentialUnavailable` with a typed reason.

Failure handling:

| Outcome | Health | Operation |
| --- | --- | --- |
| Expired/revoked login | `needs_refresh` while repair is possible, else `invalid` | One repair, then failover if enabled. |
| Provider says login required | `invalid` | Skipped until material changes or manual validation succeeds. |
| Throttle | Unchanged | The connection pauses for the provider delay (capped) or 60 s; failover to an account on another connection if enabled. |
| Network, 5xx, parsing, offline, content restrictions, generic 403 | Unchanged | Existing handling; no failover. |

Only typed extractor errors (`ExtractorError::Authentication`, `RateLimited`) drive health or failover; error text is never parsed. URL-resolution failures that are not typed are errors, not "offline". `CredentialUnavailable` bypasses the streamer error circuit breaker: no error count, no temporary disable, and a running recording is not ended.

## Admission and deadlines

[`admission.rs`](../rust-srec/src/credentials/admission.rs)

- One shared per-platform admission covers anonymous, managed and raw-cookie extraction, refresh, validation and QR provider calls. More accounts do not multiply the platform rate. Throttle backoff is per connection (see [Proxies and routes](#proxies-and-routes)).
- Remote `Retry-After` values are capped at 15 minutes.
- One logical deadline starts before admission and covers lock waits, status checks, repair, extraction and failover. Nested calls are not charged twice.

## Recording binding

- The binding travels with the extracted media (live status, monitor event, outbox, download-start sidecar). Durable events carry only the binding; media and material stay in memory and are re-extracted on a cache miss, replay or restart.
- Session creation commits the binding with the session after revalidating the profile and revision. Epochs prevent an old event from replacing a newer binding.
- Download, danmu, URL renewal and periodic polls use the bound account; ordinary polls never rotate a running recording. Before engine start the account and revision are rechecked; a change forces fresh extraction.
- Account switching happens only in owned recovery: the old attempt settles, fresh media is extracted, the new epoch is committed and danmu reconnects with the new account.
- Engine feedback: a mesio HTTP 401/403, or an unexplained ffmpeg/streamlink failure, allows one bound diagnostic extraction per failed attempt within the existing retry budget. The status alone does not invalidate the account.
- Policy edits and disabling affect new acquisitions; a running engine finishes its attempt. A pending start waiting for credentials wakes when an account is edited, re-enabled or logged in, or the selection changes.

## Managed playback

[`playback_context.rs`](../rust-srec/src/services/playback_context.rs)

- A managed parse returns display metadata, a binding summary, an opaque playback handle and the resolved stream URLs. Cookies, provider headers and extractor extras stay server-side behind the handle. Media URLs, including signed ones, are visible, as they already are on the unmanaged path.
- Handles have 128 bits of randomness and are bound to the authenticated principal, scope, profile/revision and policy generation. The store holds at most 1,024 contexts with 15-minute idle and 12-hour absolute expiry and keeps no per-resource state.
- Proxy requests carry the handle, the stream index and the real upstream URL. Credentials are attached only for the stream's or source page's origin; other origins are refused (`PLAYBACK_HOST_NOT_ALLOWED`), and cross-origin redirect hops drop them. HLS references to allowed origins are rewritten with the handle; others become plain relay URLs. HLS variable substitution is unsupported.
- A revision or policy change returns a renewal-required result; current cookies are never attached to previously extracted URLs. Responses use `no-store` and `Referrer-Policy: no-referrer`; handles are redacted from logs.
- `/api/parse/resolve` serves only client-owned media and refuses managed sources. Explicit raw cookies remain an ephemeral fixed override that never reads stored material.

## Management, QR login and notifications

- REST lives under `/api/credentials` ([routes](../rust-srec/src/api/routes/credential_profiles.rs)): profile CRUD with expected versions (profiles are listed by platform alone, since every scope on it selects from the same accounts), targeted validate/refresh (no rotation; unsupported providers return a capability result), each platform's account capabilities, and an effective-selection view with candidates, the active binding and the unavailable reason. Status reads have no selection side effects.
- Profile details name what uses the account: each selecting scope (its owner, display name and the selection's platform) and each live recording (session, streamer ID while the streamer exists, and its current or recorded name). Delete refusals (`CREDENTIAL_PROFILE_REFERENCED`) carry the same typed list. A listing reads health, selections and open sessions once for all its profiles rather than per profile. Deleting a platform that streamers still use, or that templates select accounts on, is refused separately as `PLATFORM_IN_USE` with their IDs.
- Details also carry the stored health times (last check, last refresh, consecutive refresh failures and the reason), the next renewal for providers with `renew_after()` (computed by the execution service's own [`next_renewal_at`](../rust-srec/src/credentials/execution.rs), so the view and the renewal decision cannot disagree; absent when the provider never renews the account or it needs a new login), and the profile's `last_used_at`.
- `last_used_at` lives on the profile row, not on health: it describes the account, not one revision, so it survives material changes, refreshes and re-enabling, and a use does not change the revision or version. It is set when the execution service hands an account's material to an extraction attempt, the one point where monitor checks, parse, recovery, URL renewal and managed playback acquire it (downloads and danmu use the material of the extraction that started them), at most once per five minutes per account: an in-process record skips the write, and the conditional `UPDATE` only moves a stored time that is at least that old. It is informational, so a failed write is logged and the operation proceeds. Like health, it is not exported.
- QR login ([`login_sessions.rs`](../rust-srec/src/credentials/login_sessions.rs)) binds a target (new profile on a platform, or a profile at an expected version) and the initiating principal at generation. Profile write and receipt commit together; repeated polls are idempotent. Pending sessions expire after at most 5 minutes, receipts after 10; each request is limited to 30 seconds.
- Notifications carry profile ID/label, scope, platform and typed reason: invalid on transition, refresh failure on the first and every third failure in a six-hour window, unavailable at most once per policy every 10 minutes, and a once-per-profile notice when minted session cookies cannot be saved.
- Blocked streamers and accounts needing the user are surfaced without polling. The monitor keeps a per-streamer, in-memory [record](../rust-srec/src/credentials/blocks.rs) of the last acquisition that found no usable account (typed reason, platform, start of the run), set by checks, queued starts and recoveries in `check_streamer_for` and lifted by any acquisition that reaches an account or fails for another reason, by an anonymous check, and when the streamer stops being monitored; a mid-check `SourceChanged` leaves it as it was. It is per streamer rather than the policy-keyed unavailability cache, which streamers inheriting one policy share and which an old generation can leave behind. Streamer responses carry it as `credential_blocked` (absent for unmonitored streamers), and the download WebSocket sends transitions as `STREAMER_CREDENTIAL_BLOCK`; it is not persisted, so the first check after a restart re-establishes it. `GET /api/credentials/attention` lists, in one query, enabled and non-retiring profiles whose health at the current revision is `invalid`, or `needs_refresh` with at least three consecutive refresh failures (the failure notification's first repeat), with platform names and no material; health, material, enabled-state and deletion writes in the repository wake `CREDENTIAL_ATTENTION_CHANGED` on the same socket. Throttles and unverifiable accounts never appear in either.

## Upgrade, import and export

- [Automatic upgrade](../rust-srec/src/database/legacy_credential_upgrade.rs): the first start after upgrading converts every configuration-embedded cookie, token and account login (Twitch OAuth, Douyin TTWID, Douyu device ID, SOOP username/password) into platform profiles and fixed selections, once, in one transaction that drops the [marker table](../rust-srec/migrations/20261003120000_legacy_credential_upgrade.sql). Each converted scope gets what its streamers used before; identical bundles on a platform share a profile; blank fields are unset and inherit; a template's top-level cookie converts only for platforms its streamers use; invalid material is dropped with a warning.
- Configuration writes carrying account fields are rejected; there is no legacy credential path.
- Export uses schema `1.0.0` when profiles, explicit selections or saved proxies are present, otherwise `0.1.8`, which writes routes back as the old `proxy_config` so earlier releases can read it. Profiles keep their UUIDs; an ID collision with a different platform rejects the import. Older backups go through the same conversion on import. Health, cursors, throttle pauses, bindings, QR sessions and playback contexts are not exported.
- Import writes the bundle's profiles before the configuration that selects them, in one transaction. Merge keeps a selection the bundle omits; replace takes the bundle's selections as written. Replace import retires omitted profiles and owners (a retained template stops selecting), settles affected sessions after commit, and deletes material only once nothing references it. Committed publication survives a disconnected HTTP client.
- On Streamlink, the upgrade and import give each Streamlink streamer without its own account the converted platform or template account it inherited, as a fixed selection, and create no platform or template selection. Import applies the same to a `1.0.0` bundle carrying a Streamlink platform or template selection, and reduces a Streamlink pool to its first account.
- [Proxy upgrade](../rust-srec/src/database/legacy_proxy_upgrade.rs): after the credential upgrade, the first start converts every `proxy_config` (global, platform, template, retiring templates, streamer key) once, in one transaction that drops the `legacy_proxy_upgrade_pending` marker. Each distinct (URL, username) becomes an entry named after its `host:port` with ` 2`, ` 3`… on collision; a second password for the same exit keeps the first. Disabled becomes Direct, the system flag System, absent or empty Inherit; a scheme-less `host:port` gets `http://`, a login in the URL is split out and decoded, and an unusable value (such as `socks4`) becomes Direct with a warning. A global setting that asked for no proxy becomes System when proxy environment variables are set at that start, else Direct. The old columns are cleared and never read; the never-read template `platform_overrides.*.proxy_config` keys are removed.
- Import of a `1.0.0` bundle upserts entries by name (or exit) before the scopes that name them; replace deletes omitted entries nothing references. Older bundles go through the same proxy conversion, reusing an installation entry with the same exit and skipping the environment rule.
- Rollback to an older binary means restoring the pre-upgrade database backup.

## Decision log

Accepted:

- Profiles are separate from selection policy, and belong to their platform.
- Modes are `inherit`, `none`, `fixed` and `pool` with `priority` or `round_robin`; pools default to `priority`, failover on, 3 attempts. A fixed main account with automatic backups is less exposed to anti-abuse checks than spreading every poll across accounts.
- Cache policy, acquire material at execution time; one execution service owns selection, repair, health and failover.
- Fail over on typed account authentication failures, and on throttles only towards an account on another connection. There is no per-account cooldown: no provider reports a throttle that belongs to an account rather than to the address it came from.
- Proxies are named, reusable entries, and every scope and account chooses a route to one: one model replaces the per-scope `proxy_config` JSON and the inline account proxy, which had separate validation, secrets handling and throttle keys. An entry is one exit (address and username).
- Routes are columns with a foreign key, not JSON: SQLite refuses deleting an entry in use, and the column survives table rebuilds.
- An account's own route wins over the scope route for everything done with the account, danmu included. Account management follows the account, then its platform, then global, so account checks no longer bypass the configured proxy.
- Throttles back off the (platform, route key) pair, whichever scope or account chose the route, which is what makes pools and round robin useful across proxies. Every pause notifies, naming the route.
- Direct is direct for every client: subprocesses lose the proxy environment variables too. System is the environment's proxy; danmu uses it, honouring `NO_PROXY`.
- The upgrade maps a global proxy that was off or empty to System when proxy environment variables are set at that start, because download engines used them before; otherwise Direct. Backup imports skip that rule, since a restored setting means what the backup says.
- Usernames are shown; passwords are never returned. Logins are stored in plain text, like cookies.
- One shared platform admission and one logical deadline per operation; remote delays capped at 15 minutes.
- A recording is pinned to one account; switching only through owned recovery with fresh extraction.
- Credential unavailability is separate from the streamer circuit breaker.
- A failure to store minted session cookies keeps the live result, logs and notifies once per profile.
- Managed playback keeps credentials server-side behind bounded, principal-bound handles with an origin check; media URLs stay visible.
- Configuration-embedded credentials are upgraded automatically once; the legacy path is removed.
- Health validity and reason codes are typed enums; stored text outside the known set fails decoding.
- Selections are table rows with foreign keys, not JSON inside configuration. SQLite then guarantees that no selection names a deleted or foreign profile and that removed owners take their selections with them, without scanning every configuration document on each write. The API and backup formats keep the embedded `credential_selection`.
- A streamer that moves to another platform, or is marked deleted, loses its own selection rather than blocking the edit or keeping its profiles in use.
- Streamlink accounts are chosen per streamer, as `none` or one fixed account. One platform hosts every site without a built-in extractor, so a platform or template account would send one site's cookies to all of them; Streamlink errors are not classified as account failures, so a pool could never fail over, and no account provider checks it, so health stays unknown. Streamlink receives only cookies, except that a Twitch streamer forced onto the Streamlink extractor also passes its profile's access token as `--twitch-api-header Authorization=OAuth <token>`; the download engine receives the already resolved media URL and needs no token. Credentials in Streamlink `extra_args` are outside profiles.
- Douyu accounts are renewed by age (four of about six days), not when a check or extraction fails: Douyu serves a lapsed session as a logged-out viewer, and no endpoint that reports login state has been verified. A passport refusal during renewal counts as a refresh failure, not as a revoked login, because Douyu documents no error codes for it; the account keeps working on its current cookies while failures are reported.
- Bundles that carry a Streamlink platform or template selection are converted to per-streamer selections rather than rejected: such bundles could be written before the rule existed, the conversion matches what each streamer used, and rejecting would block restoring an otherwise valid backup.

Rejected or superseded:

- Coexisting legacy and managed credentials with per-scope manual conversion: superseded by the automatic upgrade.
- An inline proxy on each account, and a `proxy_config` JSON object on each scope: replaced by saved proxies and routes. The account proxy could not connect directly or follow the platform, account checks without one bypassed the configured proxy, and every request without one shared a single throttle key.
- "Disabled" meaning direct for extraction but the environment's proxy for FFmpeg and Streamlink: replaced by Direct and System as distinct routes.
- Template- and streamer-owned profiles, owner-based access rules and clone remapping: superseded by platform ownership.
- Selections as JSON in `platform_config.credential_selection`, template overrides and streamer documents, kept consistent by Rust scans of every configuration on each write (whole-graph validation, reference enumeration, preserve-on-omit merging): replaced by tables with foreign keys. The scans were the only guard, so any writer that skipped them could strand a reference, and each write paid for the whole graph.
- Honoring arbitrarily long provider `Retry-After` delays: replaced by the 15-minute cap.
- Opaque per-resource IDs hiding media URLs in managed playback: dropped with the per-context resource registry.
- Round-robin as the pool default: replaced by priority.

Deferred:

- Streamlink profiles per site (matched by URL host), so one profile could apply to every streamer on that site.
- Passing Streamlink cookies and tokens through a 0600 configuration file instead of command-line arguments, which other local users can read from the process list.
- Classifying Streamlink authentication failures, which would allow health and failover there, only if Streamlink reports machine-readable errors; its text output is not parsed.

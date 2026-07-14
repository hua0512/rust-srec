# SOOP (숲, formerly AfreecaTV) Platform Integration

## Status

| Item | Value |
|------|--------|
| Plan date | 2026-07-14 |
| Research | `/home/hua/Personal/soop` (FINDINGS.md, CHAT_PROTOCOL.md, BILIBILI_FORMAT.md, `scripts/soop_danmaku.py`) |
| Local WIP | Substantial untracked draft (extractor + mesio filter + partial UI/docs); **not registered** |
| Target | One implementation PR after this plan is approved |

This document supersedes the earlier July 2026 draft of the same path. It folds in live research from 2026-07-14 (stream + **guest chat**) and the current WIP inventory.

---

## Context

SOOP (`play.sooplive.co.kr` / `play.sooplive.com`, formerly AfreecaTV) is a major Korean live platform. rust-srec has no native support yet (Streamlink fallback only where used).

**Goal:** first-class native support:

1. Live detection + multi-quality HLS recording (lazy AID resolution)
2. Account login (19+ / login-only) + password rooms
3. Mesio filter for SOOP “preloading” placeholder segments
4. **Guest danmaku** (chat + gifts) — research complete; **in scope for v1**
5. Full app wiring: factory, DB seed, frontend config UI, i18n, docs

### Research conclusions (2026-07-14)

1. **No client-side integrity / bot token** for open HLS. Auth is the **server-issued AID** from `type=aid`.
2. `_au*` cookies are analytics only; ignore for extract/chat.
3. Prefer **streamlink-style** CDN map + per-quality AID over streamget’s hardcoded `gcp_cdn` + `master`.
4. Chat is a **binary WebSocket** protocol; guest receive works on open rooms **without** account login when `type=live` returns `RESULT=1` and chat fields.
5. Join requires **`FTK`** from `type=live` (not AID). Same live API feeds both video and chat metadata.

### References

| Concern | Reference |
|---------|-----------|
| streamlink SOOP plugin | `streamlink/plugins/soop.py` — VIEWPRESET, CDN map, RESULT codes, preloading filter |
| streamget SOOP | `streamget/platforms/soop/live_stream.py` — login, station status, master approach |
| Local research | `/home/hua/Personal/soop` |
| Lazy `get_url` pattern | Douyu (`crates/platforms/.../douyu/builder.rs`) |
| Password / `?pwd=` | PandaTV |
| Login credentials config UI | Twitch / Twitcasting fields |
| Binary danmu provider | Huya / Douyu `DanmuProtocol` + `WebSocketDanmuProvider` |
| Guest chat prototype | `/home/hua/Personal/soop/scripts/soop_danmaku.py` |
| Full platform wiring template | Recent Bigo work (`feat/bigo-platform-support` / PR when merged) |

---

## Protocol contract

### Stream extract

```
POST https://live.sooplive.com/afreeca/player_live_api.php?bjid={channel}
Content-Type: application/x-www-form-urlencoded
Origin: https://play.sooplive.com
Referer: {streamer page URL}
```

Common form fields: `bid`, `bno`, `pwd`, `player_type=html5`, `stream_type=common`, `mode=landing`, `from_api=0`.

| type | Extra | Returns under `CHANNEL` |
|------|--------|-------------------------|
| `live` | quality often `master` / empty | `RESULT`, `BNO`, `BJNICK`, `TITLE`, `RMD`, `CDN`, `BPWD`, `VIEWPRESET`, **chat:** `CHDOMAIN`/`CHIP`, `CHPT`, `CHATNO`, `FTK`, `BJID` |
| `aid` | `quality={preset name}`, `pwd` | `RESULT`, `AID` |

**RESULT codes:** `1` OK/live · `0` offline / fail / geo stub · `-6` login required · `-8` adult-related gate (treat like login-gated for video).

**CDN assign:**

```
GET {RMD}/broad_stream_assign.html?return_type={mapped_cdn}&broad_key={BNO}-common-{quality}-hls
→ { "view_url": "https://....m3u8" }
```

CDN map (substring): `gs_cdn` → `gs_cdn_pc_web`, `lg_cdn` → `lg_cdn_pc_web`, else pass-through.

**Final HLS:** `{view_url}?aid={urlencoded AID}`  
AID is **short-lived and per-quality** → resolve in `get_url()`, not at extract time.

**Login (video gate):**

```
POST https://login.sooplive.com/app/LoginAction.php
szWork=login, szType=json, szUid, szPassword, isSaveId=true, isSavePw=false, isSaveJoin=false, isLoginRetain=Y
→ RESULT=1 + Set-Cookie (AuthTicket…)
```

Optional cookie check: `GET https://afevent2.sooplive.com/api/get_private_info.php` → `CHANNEL.LOGIN_ID`.

**Station nick (offline):**  
`GET https://st.sooplive.com/api/get_station_status.php?szBjId={channel}` → `DATA.user_nick`.

### HLS quirk — preloading segments

SOOP media playlists interleave placeholder segments whose URI contains `preloading`.  
**Do not remove them from `MediaPlaylist.segments`** — that shifts MSN attribution in `engine::planner::plan` (`msn = window_start + idx`).  
Skip them at the **planner download stage** (streamlink `should_filter_segment` semantics).

### Danmaku (guest WebSocket)

```
1. type=live → CHDOMAIN, CHPT, CHATNO, FTK, BJID, BPWD
2. WSS wss://{CHDOMAIN}:{CHPT+1}/Websocket/{BJID}
   Sec-WebSocket-Protocol: chat
3. SVC_LOGIN  fields: ["", "", 16]     # guest flag
4. SVC_JOINCH fields: [CHATNO, FTK, 0, "", mode_payload]
5. Receive SVC_CHATMESG (5), gifts (18/33/37/41/108/…), notices
6. SVC_KEEPALIVE (0) every ~60s
```

**Packet:** 14-byte ASCII header + body:

| Bytes | Content |
|-------|---------|
| 0–1 | `\x1b\x09` |
| 2–5 | 4-digit svc (`0005`) |
| 6–11 | 6-digit body length |
| 12–13 | `00` |
| body | fields joined with `\x0c` (leading+trailing FF) |

**Guest receive needs:** `FTK` + chat host/port/room from `type=live`.  
**Does not need:** account, `AuthTicket`, `_au*`, AID, integrity tokens.

| Room type | Guest chat |
|-----------|------------|
| Open live (`RESULT=1`, `BPWD=N`) | Yes |
| Password (`BPWD=Y`) | Yes with room password in JOIN |
| Adult / login (`RESULT=-6/-8`) | Only if API still returns chat fields (often needs cookies) |
| Offline (`RESULT=0`) | No |

**v1 danmaku scope (match Bigo product bar):**

| Record | Skip (v1) |
|--------|-----------|
| Chat `SVC_CHATMESG` (5) | Enter notices / user list as primary product |
| Gifts: balloon 18/33, chocolate 37, superchat 41, subscription 108 | Sending chat |
| Optional: fan letter 20, gem 120 | Full manager/notice taxonomy |

Map into `DanmuItem::Message` / gift events like Douyu/Bigo. Full bilibili NDJSON mapping is research-only (`BILIBILI_FORMAT.md`); rust-srec uses its own danmu item model.

---

## Design decisions

1. **Platform id:** registry `soop`, display `"SOOP"` (`StreamerUrl::platform()`).
2. **Lazy URL resolution:** `extract()` emits empty `url` per VIEWPRESET (skip `auto`); `get_url()` fetches AID + assign. Call sites already exist (parse + detector).
3. **Credentials:** `username` / `password` / `stream_password` via platform extras (Twitch/Twitcasting pattern). Prefer **stored cookies** for permanently restricted channels (document rate limits on reactive login).
4. **Reactive login:** on `RESULT == -6` (and treat `-8` similarly for video), if creds present → login once → retry; without creds → clear validation error. Thread login cookie via request header (`extract` is `&self`).
5. **Password precedence:** URL `?pwd=` overrides `stream_password`. `BPWD=Y` without password → `PrivateContent`.
6. **Quality:** `StreamInfo.quality = VIEWPRESET.name` (`original`, `hd4000`, …); human label in stream extras; `priority = preset index`; bitrate hint from trailing digits (`original` above max numbered).
7. **Danmu join key:** put chat metadata in `MediaInfo.extras` from the same `type=live` response: at least `chatno`, `ftk`, `chdomain`, `chpt`, `bjid` (and `stream_password` / pwd when present). `DanmuService` arm for `"soop"` should prefer extras (channel id alone is not enough for JOIN).
8. **Danmu binary:** implement `DanmuProtocol` with binary frames (subprotocol `chat`). No separate integrity mint.
9. **Mesio:** host check `sooplive` / `afreeca` + skip `preloading` in planner — **not** by deleting playlist entries.
10. **No integrity token module** (unlike Bigo).

---

## Local WIP inventory (do not ship as-is without review)

Untracked drafts exist on disk from earlier work. Use as starting point, re-verify against current master and this plan.

| Path | Approx. state | Gap vs plan |
|------|---------------|-------------|
| `crates/platforms/.../soop/{mod,builder,models}.rs` | Full extract + login + `get_url` + unit tests | Not registered; models lack chat fields (`FTK`, `CHDOMAIN`, `CHPT`, `CHATNO`, …); no `danmu.rs` |
| `crates/mesio/src/hls/soop_processor.rs` | `is_soop_playlist` + `is_preloading_segment` + tests | **Not wired** into `hls/mod.rs` / planner; correct “don’t strip playlist” approach |
| `rust-srec/migrations/20260704000000_add_soop_platform.sql` | Seed draft | Confirm timestamp vs existing migrations when landing |
| `frontend/.../soop-config-fields.tsx` | Auth + stream password form | Schema/tab/icons/utils/supported-platforms not wired |
| `docs/{en,zh}/platforms/soop.md` | Draft pages (danmaku ❌) | Need registration in index/vitepress + danmaku ✅ + release notes |

**Must still implement for a complete PR:**

- Factory / `mod.rs` / `SoopConfig` registration  
- `streamer_url` detection  
- Mesio module + planner integration  
- Frontend schema/tab/constants/utils + i18n extract/compile  
- Docs index + vitepress + unreleased  
- **`soop/danmu.rs`** + danmaku registry + `DanmuService` extras arm  
- Live ignored tests (extract + danmu)  
- Clippy/fmt/frontend gates  

---

## Implementation phases

### Phase 1 — Extractor finish + register

1. Extend `models.rs` with chat fields from `type=live` (all optional strings / tolerant ints).
2. On live extract success, populate `MediaInfo.extras` with chat join fields + `channel_id` / `bno`.
3. Keep lazy stream extras as today (`bid`, `bno`, `quality`, `label`, `rmd`, `cdn`, `pwd`).
4. Register: `platforms/mod.rs`, `factory.rs`, `platform_configs.rs` (`SoopConfig`: username, password, stream_password).
5. Unit tests already partially present; add fixtures for chat fields / string `RESULT`.

### Phase 2 — Backend

1. `streamer_url.rs` → `"SOOP"`.
2. Migration seed `platform-soop` (refresh migration timestamp if needed).
3. `danmu/service.rs` arm `"soop"` → room id from extras (`chatno` or agreed key) + ensure FTK/host available to provider via connection extras.

### Phase 3 — Mesio preloading

1. `mod soop_processor` in `hls/mod.rs`.
2. In planner (and any other segment-schedule site), when playlist URL is SOOP, **skip** segments where `is_preloading_segment` is true without removing them from the media playlist vector.
3. Unit tests for host detection + preloading URI.

### Phase 4 — Danmaku

New `soop/danmu.rs`:

| Method | Behavior |
|--------|----------|
| `platform` | `"soop"` |
| `websocket_url` | Build from extras / re-fetch `type=live` if needed |
| `headers` | Origin/UA; subprotocol `chat` (see how other providers set protocols) |
| `handshake_messages` | SVC_LOGIN guest packet (or challenge-driven if required by framing) |
| `heartbeat_message` | SVC_KEEPALIVE ~60s |
| `decode_message` | Parse binary frames; reply JOIN on LOGIN ok via `tx`; emit chat/gifts |

Register in `danmaku/registry.rs`.  
Port packet codec from research script carefully (UTF-8 body, multi-packet WS messages).

### Phase 5 — Frontend

1. `utils.ts` — sooplive / afreecatv → `soop`
2. `pipeline/constants.tsx` — icon (e.g. `Tv`) + emerald color
3. `supported-platforms.tsx`
4. `SoopConfigSchema` + `soop-config-fields.tsx` + `platform-specific-tab.tsx`
5. `pnpm extract` → zh-CN for new strings → `pnpm compile` → `pnpm fmt:check` / lint / build

### Phase 6 — Docs + release notes

1. Finalize `docs/{en,zh}/platforms/soop.md` (HLS multi-quality, login, password, **danmaku ✅ chat+gifts**, guest limits)
2. Index count/table + vitepress nav  
3. User-facing unreleased bullets (no struct/file names)

---

## File checklist

| Area | File | Change |
|------|------|--------|
| crate | `platforms/.../soop/{mod,builder,models,danmu}.rs` | finish + **danmu** |
| crate | `platforms/.../mod.rs`, `factory.rs`, `platform_configs.rs` | register |
| crate | `danmaku/registry.rs` | provider |
| mesio | `hls/soop_processor.rs`, `hls/mod.rs`, planner/watcher as needed | preloading skip |
| backend | `streamer_url.rs`, `danmu/service.rs`, migration | detect + room extras + seed |
| frontend | utils, constants, supported-platforms, schemas, fields, tab, locales | UI + i18n |
| docs | soop.md en/zh, index, vitepress, unreleased | product docs |

---

## Verification

1. `cargo fmt --check && cargo clippy -p platforms-parser -p mesio -p rust-srec -- -D warnings`
2. Unit: regex, models (live/offline/-6/aid/string RESULT), CDN map, preloading filter, packet codec
3. Live ignored: extract multi-quality + `get_url` m3u8; guest danmu join on open room
4. App: migration seeds platform; add live SOOP URL; qualities listed; recording without preloading loops; danmaku file when enabled
5. Password room: no pwd → PrivateContent; `?pwd=` / config → stream + chat join
6. Login gate: clear error without creds; with cookies/username works when account valid
7. Frontend: `pnpm lint && pnpm fmt:check && pnpm build`; i18n no missing zh-CN for SOOP strings
8. Docs: vitepress en/zh sidebar

---

## Non-goals (v1)

- Sending chat / logged-in identity beyond video unlock  
- Enter notices, full notice/manager taxonomy as first-class UI  
- Session-cookie persistence subsystem after reactive login (document cookies preferred)  
- VOD / catch-up  
- SOOP Global if API diverges  
- PPV / subscription-only private auth paths  
- Colony side-channel  
- Streamlink plugin changes  

---

## Suggested PR split

| PR | Content |
|----|---------|
| **This PR** | Plan only (`rust-srec/plans/soop-platform-integration.md`) |
| **Implementation** | Phases 1–6 in one PR if size is acceptable; optional split: (A) extract+mesio+wiring without danmu, (B) danmu follow-up — prefer single PR if review bandwidth allows, since chat metadata is on the same live API |

---

## Open questions for implementer

1. **Subprotocol negotiation:** confirm `WebSocketDanmuProvider` can set `Sec-WebSocket-Protocol: chat` (extend provider if missing).
2. **`RESULT=-8`:** treat identically to `-6` for video login path unless live tests show otherwise.
3. **Geo/`RESULT=0` stubs:** surface offline/unavailable cleanly (no infinite login retry).
4. **Migration timestamp:** rename if `20260704000000` collides or is too old relative to Bigo migrations.
5. **Danmu room id for service:** prefer stable `chatno` or `bjid` string for logging/session keys — document chosen key in code comments (code-local, not PR labels).

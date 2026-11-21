# Release Notes Writing Style

Use this guide when adding, editing, or auditing entries in `rust-srec/docs/{en,zh}/release-notes/unreleased.md` and versioned `vX.Y.Z.md` pages.

## Core principles

- **Write for users, not contributors.** Lead with what users will see differently, why it matters, and whether they need to change a setting or workflow after upgrading.
- **Describe observable behavior, not internals.** Never cite Rust/TypeScript types, struct fields, file paths, traits, commit/PR history, or internal architecture. Avoid engineering and UI-implementation jargon (such as *keyframe*, *container*, *scheduler*, *Web Worker*, *structured lists*, *direct row actions*, `全宽卡片`, `代理行`, `引用详情`, `降频重试`).
- **Keep each bullet scannable.** Aim for 2–4 crisp sentences. State the outcome and key caveats, then link to the docs site (`https://docs.srec.rs/{en,zh}/...`) for detailed procedures instead of pasting full documentation into a release note.
- **Skip unreleased regressions.** A fix for a regression whose cause is in no release (`git tag --contains <sha>` is empty) gets no entry.

## Formatting and UI labels

- **Verify real UI labels.** Check `rust-srec/frontend/src/locales/en/messages.po` and `rust-srec/frontend/src/locales/zh-CN/messages.po` before naming settings, tabs, buttons, or notification events. Do not leave translated UI terms in English in `zh` notes (for example, use **执行** / **执行超时** and **替换** 模式, not `execute` or `replace 模式`).
- **Bold UI elements consistently.** Use **bold** for settings, buttons, page/tab names, and notification event names (such as **Streamer checks failing** / **主播检测持续失败**, **Max Retries** / **最大重试次数**). Do not mix quotes, italics, and plain text for UI names.
- **Use inline code for technical literals only.** Reserve backticks for environment variables (`HTTP_PROXY`), CLI flags (`--retries`), URL patterns, and short IDs (`#6c333b6b`).
- **Keep `en` and `zh` strictly aligned.** Both files must cover the same items under the same sections in the same order, with matching links.

## Chinese (`zh`) conventions

- **Pronoun:** Always address the reader as `你`, never `您`.
- **Natural phrasing:** Write idiomatic Chinese directly instead of translating English syntax word for word. Avoid filler tautologies (e.g. “设为直连时直接连接，设为已保存的代理时经由该代理连接”) and awkward literal calques (e.g. “询问……是否在直播”, “上传可以到达云存储”, “接受单独检测”, “以……失败”, “选用每个已保存代理的渠道”).
- **Consistent terms:**
  - `会话` (not `场次`) for recording sessions; use `登录会话` when disambiguating an auth session in the same bullet.
  - `标签页` (not `选项卡`) for in-app UI tabs.
  - `每条代理` / `快捷操作按钮` (not `每个代理行` / `直接操作按钮`).
- **Bold spacing:** Put a single space on each side of `**粗体**` when adjacent to Chinese characters (for example `**每页** 选择器`, `**系统健康** 页面`, `点击 **返回**`), and no extra space next to Chinese punctuation (`，。：；、（`).
- **Cross-references:** Use `详见[...]` for same-page section anchors (`#升级前须知`, `#录制与恢复`) and `参见[...]` for documentation links (`https://docs.srec.rs/zh/...`).

## English (`en`) conventions

- **Spelling and register:** Use British spelling consistent with existing release notes (`colour`, `cancelled`, `recognising`, `capitalised`). Avoid contractions (`cannot`, `does not` instead of `can't`, `doesn't`).
- **Punctuation:** Omit the Oxford comma in simple series (`platform, template and streamer`), using commas only when needed for clarity in longer clauses.
- **Direct openings:** Start fix bullets directly with `Fixed ...` or `Prevented ...` (avoid `Fixed an issue where ...`), and keep grammatical subjects aligned (`Saving changes ... now keeps you on that page`, not `Saving changes ... now stays on that page`).
- **Clear verbs:** Prefer natural user-facing verbs (`using each saved proxy`, `focus the field`, `announce which option was selected`) over literal or vague phrasing (`choose each saved proxy`, `move to the field`, `say which option`).

## Good vs. avoid examples

| Avoid | Prefer |
| --- | --- |
| 重新设计了 **引擎** 和 **代理** 设置页，统一采用全宽设置卡片与分割列表布局，补充直接操作按钮，支持在代理行内展开查看引用详情。 | **引擎** 和 **代理** 设置页已改用与其他设置页一致的列表布局：每条引擎现在会显示配置摘要和快捷操作按钮，**系统代理** 拥有独立的状态卡片，每条代理也可以按需展开查看正在使用它的各项设置。 |
| 代理页面会列出选用每个已保存代理的渠道，仍被渠道选用的代理不能删除。 | **代理** 页面会列出各代理正在被哪些渠道使用，仍在使用中的代理无法删除。 |
| Fixed an issue where deleting a job preset that a pipeline still uses caused runtime failures. | Prevented deleting or renaming a job preset or workflow still used by a pipeline, which previously caused those pipelines to fail at runtime. |
| Saving changes on a platform settings page now stays on that page. | Saving changes on a platform settings page now keeps you on that page instead of returning to the list. |

## Audit checklist

Before finalizing `unreleased.md` or promoting notes to `vX.Y.Z.md`:

1. Every bullet explains the user-visible outcome first and requires no source-code context to understand.
2. Every UI setting, button, tab, and event name matches the frontend locale files and uses **bold** consistently.
3. No leftover `您`, `选项卡`, `场次`, untranslated UI labels, or unspaced `**粗体**` in `zh`.
4. No `Fixed an issue where`, contractions, or mismatched US spelling in `en`.
5. `en` and `zh` have identical section structure, bullet order, and valid `https://docs.srec.rs/{en,zh}/...` links.

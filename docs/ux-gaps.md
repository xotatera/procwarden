# UX Gaps Audit

## Critical

| # | Gap | Location | Status |
|---|-----|----------|--------|
| 1 | No empty state message when filter matches nothing — blank table with no explanation | `rendering.rs:189` | |
| 2 | No process count / filter count display (e.g. "12/542 processes") | `rendering.rs:189-228` | |
| 3 | No Page Up/Down, Home/End navigation in process list | `process_list/handlers.rs:5-45` | |
| 4 | No Ctrl+C to quit — only `q` works | `app.rs:100`, `main.rs:101` | |
| 5 | `s` key doesn't toggle settings closed (contradicts README) | `app.rs:209-223` | |
| 6 | Filter OR syntax (comma-separated) undocumented in UI help text | `rendering.rs:205-221` | |
| 7 | No `?` or `F1` key for in-app help overlay | N/A | |
| 8 | Settings auto-save silently — no feedback on save or failure | `app.rs:217-220` | |
| 9 | Selected process exits → selection jumps without any feedback | `app.rs:49-62` | |
| 10 | No high-CPU color warning (all rows same color regardless of CPU) | `rendering.rs:134-140` | |

## Medium

| # | Gap | Location | Status |
|---|-----|----------|--------|
| 11 | No vim-style j/k navigation | `process_list/handlers.rs` | |
| 12 | No `/` key shortcut to jump into filter mode | `app.rs:93-113` | |
| 13 | Settings can't be accessed from Log view | `app.rs:226-235` | |
| 14 | No settings grouping/categories — 10 flat options | `rendering.rs:251-397` | |
| 15 | No log scrolling — auto-scrolls to bottom, user can't scroll up | `rendering.rs:470-474` | |
| 16 | No settings reset-to-defaults option | `settings/handlers.rs` | |
| 17 | Relative CPU column only appears during filter — confusing | `rendering.rs:122-123` | |
| 18 | Log has no search/filter capability | `rendering.rs:421-493` | |
| 19 | Sort direction change (Space) has no visual flash/feedback | `process_list/handlers.rs:37-39` | |
| 20 | Toggle vs numeric settings not visually distinguished | `rendering.rs:254-394` | |

## Low

| # | Gap | Location | Status |
|---|-----|----------|--------|
| 21 | Help text doesn't mention uppercase keys work | `rendering.rs:205-221` | |
| 22 | No terminal-too-small warning | `rendering.rs:54-67` | |
| 23 | EMA alpha / relative CPU unexplained in UI | `rendering.rs:355-365` | |
| 24 | Log auto-scroll has no position indicator | `rendering.rs:470-474` | |
| 25 | Process name truncation has no ellipsis indicator | `rendering.rs:162-187` | |

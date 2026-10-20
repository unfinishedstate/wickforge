# mexc-chart

A single-process Rust/iced desktop chart for native **Binance, OKX, Coinbase, and MEXC spot and perpetual markets**, quoted in **USD, USDT, or USDC where actually listed**. Includes linear and inverse perpetuals; quote and settlement currencies are distinct. No substituted venue, dated-futures substitute, currency-converted prices, or resampled bars. A fresh workspace starts at MEXC perpetual BTC_USDC / 5m; subsequent launches restore the saved workspace automatically. The desktop calls native exchange providers directly: no localhost server, local REST/WebSocket API, or serialized inter-process market data. Nine indicators are calculated locally, including historical/live CVD and resettable VWAP; a tenth plots native historical/live open interest. Anchored VWAP is also an ordinary price-chart drawing. TPO and priced-execution Volume Profiles have independent instances, source coverage, and auction/reference levels. CVD and OI instances can independently use the chart, another native market, or a strict base-asset aggregate. Committed workspace configuration, profiles, and drawings are saved locally by default. No API key, account, trading, or database.

## Run

Requires **Rust 1.89+** and a graphical **Linux, Windows, or macOS** desktop. **Omarchy is optional.** Linux supports the native Wayland/X11 backends; Windows and macOS use iced's native windowing/graphics backends.

For native builds, Linux needs its windowing development libraries (the CI workflow lists Ubuntu packages); Windows needs the Rust MSVC toolchain with Visual Studio C++ Build Tools and the Windows SDK; macOS needs Xcode Command Line Tools.

### macOS launcher

From a clone or extracted source directory, run the executable **`run-macos.command`** in the repository root:

```sh
./run-macos.command
```

You can also double-click it in Finder. The launcher finds Rust in `CARGO_HOME` or `~/.cargo/bin` when Finder's PATH does not include it, checks Xcode Command Line Tools, changes to the repository directory, and builds/runs the locked release version. It works when started from another directory and forwards arguments to the app. The first build can take several minutes; subsequent launches reuse Cargo's incremental artifacts. Missing prerequisites produce installation instructions rather than installing tools automatically.

### Manual build

```sh
cargo build --release --locked --workspace
```

Run the single executable from this directory on Linux/macOS:

```sh
./target/release/mexc-chart
```

On Windows, run `.\target\release\mexc-chart.exe` from PowerShell. `cargo run --release -p mexc-chart` also works on each platform.

`.github/workflows/ci.yml` defines native Linux/Windows/macOS workspace tests, strict Clippy, and optimized builds on stable Rust, with OS/architecture-qualified executable artifacts. Linux also checks the Rust 1.89 minimum separately from current lint rules. Artifacts are executables, not installers or signed/notarized application bundles; normal native runtime/graphics libraries are still required.

Close the window to exit the app and its native feeds/replays. For development, use `cargo run -p mexc-chart`.

On this workstation, `~/Apps/mexc-chart.sh` executes `~/Work/mexc-chart/target/release/mexc-chart`. It uses the existing release build and does not rebuild it or start a companion process.

## Controls

| Action | Keyboard | Mouse |
| --- | --- | --- |
| Show all shortcuts/actions | F1 anywhere; `?` outside editors | ? in the compact header |
| Search and execute contextual commands | Ctrl+K; type, Up/Down/Tab, Enter; Esc cancels | Commands in F1; click a search result |
| Pan time | Left / Right; Shift for larger steps | Drag chart horizontally; popup pan-time actions |
| Choose value-scale target | Ctrl+Tab / Ctrl+Shift+Tab cycles price and visible indicator panes | Click a plot/indicator legend or use its scale; active lower legend shows “scale target” |
| Pan target value scale | Up / Down; Shift for larger steps | Drag the selected plot vertically |
| Zoom shared time | `=` / `-` | Wheel over any plot; Ctrl-wheel/pixels; wheel/drag any bottom axis |
| Zoom target value scale | Ctrl+Up / Ctrl+Down; Shift+`=` / Shift+`-` | Shift-wheel over a plot; wheel/drag its right axis |
| Toggle target auto-fit | F | Popup fit action; double-click any right axis to restore auto-fit |
| Return to realtime | L / End | Go live appears beside connection status when time following is stopped; popup live action |
| Reset time and all value scales, auto-fit, and follow live | R / Home | Popup reset action |
| Inspect previous / next candle | `[` / `]` (also comma / period) | Hover chart; popup inspection actions |
| Clear inspection | Escape | Right-click a plotted pane; popup clear-inspection action |
| Load an older page | B | Popup older-page action |
| Jump to a UTC date/time | Ctrl+G; enter date/time; Enter | Search “UTC” in Commands; Jump |
| Retry connection/history/theme now | Y | Popup retry action |
| Open indicator manager | I | Indicators in the compact header; popup indicator-editor action |
| Add / manage TPO and Volume Profiles | Indicator chooser: TPO / Visible Range Volume Profile; Ctrl+T / Ctrl+V outside text inputs | Profiles tab, exact-instance Settings / Hide, or Ctrl+K profile commands |
| Add / reset VWAP | Ctrl+K → Add VWAP; Ctrl+Shift+R in its indicator settings cycles the reset | VWAP in Indicators; typed UTC day/week/month or custom-session selector |
| Change native market | M | Click the header symbol; popup market-selector action |
| Change only the native interval | Ctrl+Left / Ctrl+Right outside editors | Header interval dropdown; choices belong to the active exact market, not a cancelled editor draft |
| Settings / hide / remove active lower indicator | Ctrl+E / Ctrl+H / Ctrl+D outside editors; Ctrl+Tab chooses the target | Settings / Hide / Remove on that indicator's title |
| Open status / indicator details | S outside editors; Ctrl+J for active lower pane or selected manager instance | Status summary / Details on the exact indicator |
| Resize active plot | Ctrl+Alt+Up / Ctrl+Alt+Down, in 32-pixel steps | Drag the grip below that plot |
| Restore active plot height | Ctrl+Alt+0 | Double-click its grip |
| Collapse / expand active lower plot | Ctrl+Alt+C | Collapse / Expand on that exact indicator's title |
| Reorder active lower pane | Ctrl+Alt+Shift+Up / Down | Search “Move active pane” in Commands |
| Temporarily maximize / restore active plot | Ctrl+Alt+M | Commands; Restore panes in the footer |
| Scroll lower panes | PageUp / PageDown | Scrollbar or Alt-wheel over plots; axes retain their own gestures |
| Scroll overlay legends | Shift+PageUp / Shift+PageDown | Legend scrollbar or wheel |

The chart-first header groups the exact native symbol and venue/product/quote on the left, with interval, **Indicators**, **Draw**, and **?** actions on the right. Native OHLC values use grouped monospace readouts with UTC and latest/inspection context; full native volume remains available in its tooltip. Quiet controls and theme-derived surfaces reserve emphasis for selection, focus, and Add/Apply. Every visible indicator retains exact-instance **Details / Settings / Hide / Remove** controls and scale-target selection; compact summaries retain native units, CVD quality/origin, and OI provenance/as-of. **F1**, **?** (outside editors), or the header **?** opens context-first keyboard/mouse help with aligned keycaps and descriptions.

Close is selected initially. **Tab / Shift+Tab** selects actions and wraps; **Enter / Space** activates the selected action or closes the popup. **Up / Down / PageUp / PageDown** scrolls the list; **Escape** closes it. The background widget trees stay mounted: pane/legend/instance-list scroll positions, editor/search drafts, and the workspace layout survive the popup. Input and background dropdown overlays are blocked while live data continues. Closing restores editor focus. Chart actions are disabled in editor-context help; editor-switch actions remain available.

Market, indicator, drawing, and detail dialogs adapt to the available window, bounded to 760×720 logical pixels. Drawing palettes and simple indicator editors use shorter content-dependent heights. A single outer shell leaves the full-width chart mounted behind a dimmed, input-blocked backdrop; opening/cancelling does not change numerical scales or source ownership. Headers, validation, and action footers remain fixed while content scrolls with room for its scrollbar. Indicator instances/parameters sit side by side when inner width is at least 600 pixels and stack when narrow; an empty manager gives its full body to parameters rather than reserving an unused list. Drawing anchor A/B cards likewise sit side by side when wide and stack when narrow.

Clicks inside a dialog—including padding, labels, and an already-selected source radio—stay inside that dialog. Close with Escape, its Close/Cancel action, or a press outside the panel; releasing outside after pressing inside does not dismiss it. Dropdowns retain their own selection handling.

The footer groups price connectivity, candle history, indicator attention/quality/freshness, catalog discovery and theme errors into concise clickable summaries. **S** opens Status details; it distinguishes live upstream prices from realtime viewport following and retains raw progress/errors. **Ctrl+J** opens details for the active lower pane, or for the selected manager instance (including overlays and hidden instances). **Escape** closes details; Up/Down/PageUp/PageDown scroll. In indicator details, **Ctrl+E** opens exact-instance settings, **Ctrl+R** retries its selected native sources, and **Ctrl+Shift+C** explicitly switches only that instance to Chart source. In Status details, **Y** retries connection/history/theme.

Wheel gestures are identical over the price and lower plots: bare/Ctrl-wheel zooms shared time; Shift-wheel zooms only that plot's values. Unmodified trackpad pixels pan shared time using the dominant direction, without panning values; Ctrl-pixels zoom time and Shift-pixels zoom values. Alt-wheel over plots scrolls the pane stack. Right/bottom axes always own value/time gestures, including with Alt. On layouts with an unshifted `+`, that also zooms time in. All timestamps are UTC. Prices retain the selected instrument's native quote precision; quantities retain native base-asset or contract units.

The crosshair, drawing hit testing, and resize grips use the same scroll-translated pointer coordinates as the rendered plots. A grip drag remains anchored to its starting height through rapid motion and delayed view updates; keyboard layout changes cancel an older held grip rather than competing with it.

Panning or zooming time stops following live without stopping data updates. **Go live** appears beside connection status while following is stopped; it and **L / End** resume time following without changing manual value bounds or CVD origins. The status describes upstream connectivity, not whether the viewport follows live. Manual value movement disables auto-fit only for that plot; **R** resets time and every value scale. Navigating to the left edge automatically fetches older candles with keyboard or mouse; prepending history does not shift the viewport.

The app initially requests one native page, up to 1,000 candles. Limits differ: Binance and MEXC perpetuals return up to 1,000, MEXC spot 500, OKX recent history 300 and older history 100, and Coinbase up to 300. Short nonempty pages retain an older cursor. Older pages and live updates remain in memory for the selected market; switching clears its candle history. There is no disk cache or hard session-memory cap. The time viewport supports 10–10,000 candles. Connection failures remain visible; connections and failed history requests recover automatically. Reconnection backfills missed candles, including multi-page gaps.

### Fast command and navigation workflows

**Ctrl+K → EMA → Enter** adds a native-default EMA(20) without opening the manager. The palette searches case-insensitively across all entered words and shows exact defaults, shortcuts, indicator IDs/specifications, pane actions, drawing tools, and quick markets. **MACD settings** selects an existing MACD's settings rather than adding another instance; **Open Interest details** opens that exact instance. Up/Down and PageUp/PageDown move through matches, Tab/Shift+Tab wrap, Enter executes, and Escape cancels. In an editor, chart/drawing mutations are omitted. Cancelling preserves the draft and restores the exact previously focused field, including a second or third numeric parameter.

The market selector keeps the **12 most recent successful exact markets** and explicit **favorites**, restored automatically across launches. **Ctrl+F** favorites/unfavorites its selected exchange/product/native-symbol/interval; **Ctrl+Shift+Up/Down** chooses a quick entry. The palette also searches these entries. Every quick switch revalidates the saved full identity against its native catalog; unavailable symbols or intervals stay visible for repair, with no substitution.

**Ctrl+G** accepts `YYYY-MM-DD` (midnight UTC) or `YYYY-MM-DD HH:MM[:SS]`, with optional `T` and trailing `Z`. Invalid dates, offsets, and fractional/leap seconds are rejected without losing the input. A jump centers the time viewport, stops following live, and pages native history chronologically when necessary. Native gaps stay gaps; exhausted/nonadvancing history and future targets are explicitly reported. Manual time navigation, **L / End**, reset, or a market switch cancels pending jump intent without inventing candles or inspection prices.

**Ctrl+Alt+Shift+Up/Down** swaps the active lower pane with its adjacent visible lower pane. **Ctrl+Alt+M** temporarily gives the active plot the chart area; use it again or **Restore panes** to return. Ctrl+Tab still selects a visible target while maximized. Reordering/maximization preserve numerical scales, saved heights/collapse, indicator configurations, histories, and source ownership.

Contributor diagnostics distinguish current discovery, missing input, disconnection, staleness, incomplete native coverage, and configuration repair. Historical proven values never excuse a current blocker. Details name the exact contributor and offer its scoped **Retry contributor**, **Retry discovery**, or settings repair; unrelated healthy producers and the price chart remain intact. **Ctrl+R** in indicator details still explicitly refreshes all that instance's selected sources. Explicit catalog retry evicts only that scope's completed failed cache result, retaining successful and in-flight sharing.

### UX rollout

- **Phase 1 — implemented:** direct native interval selection, visible Indicators entry, per-instance title controls, conditional Return to realtime, and typed CVD reset selection.
- **Phase 2 — implemented:** adaptive market/settings dialogs, fixed action footers, compact legends, grouped status/details, and actionable source incompatibility.
- **Phase 3 — implemented:** visible source-mode radios, direct exact-native-market radios/checkboxes, an exchange eligibility/selection checklist, and fail-closed All-source revalidation after context or discovery changes.
- **Phase 4 — implemented:** session-only resizable/collapsible panes, uniform plot gestures, nice value/calendar UTC ticks, compact native quantities with unabridged disclosure, and theme-derived plot contrast.
- **Phase 5 — implemented:** session instrument drawing books, seven price-chart drawing tools, absolute time/price anchors, transactional gesture history, candle magnet, and adaptive object/property controls.
- **Phase 6 — implemented:** shared theme-derived UI styling, a single-row chart header, grouped native readouts, quiet contextual actions, numbered responsive drawing tiles, compact selectable lists, grouped properties, aligned keycap help, shorter simple dialogs, and keyboard parameter/source scrolling.
- **Phase 7 — implemented:** concise market/source rows, theme-colored semantic badges, reduced repeated instructions, full-width empty editors, and deduplicated native OI detail disclosures. F1 retains the complete keyboard flows.
- **Phase 8 — implemented:** contextual command palette, exact session quick markets, UTC navigation, pane reorder/maximize, repeated drawing creation, contextual styles, actual-native-bar measurement, typed contributor remedies, and explicit synthetic renderer-load measurement.
- **Phase 9 — implemented:** default-on versioned workspace persistence, native-first revalidation, committed drawing/configuration restore, historical viewport/pane restoration, atomic debounced saves, close flush/retry, and separately confirmed current-chart/global resets.
- **Phase 10 — implemented:** independent native TPO/Volume Profiles, exact execution quantities, POC/value areas, auction structure and reference lifecycles, custom IANA sessions, keyboard range placement/settings, explicit coverage/provenance, and schema-2 profile persistence/reset support.
- **Phase 11 — implemented:** native-candle VWAP with UTC/calendar or IANA daily resets, ordinary anchored VWAP drawings, shared incremental volume weighting, explicit origin/partial history, automatic native-origin paging, and schema-4 configuration persistence.
- **Phase 12 — implemented:** colorful standalone defaults, optional Linux Omarchy integration, portable native workspace paths/locking/atomic writes, platform-aware diagnostics, and native Linux/Windows/macOS build CI.

## Workspace persistence and clean starts

Persistence is **on by default**. Committed changes autosave after roughly 500 ms of inactivity; normal window closure waits for the latest revision. Save failures remain visible through **Workspace attention → Details → Retry workspace save**. Closing after a failure offers Retry (initial keyboard selection) or explicit Exit without saving.

An absolute **`XDG_STATE_HOME`** overrides storage on every platform. Otherwise the checkpoint uses the native per-user location:

| Platform | Workspace checkpoint |
| --- | --- |
| Linux | `~/.local/state/mexc-chart/workspace.json` |
| macOS | `~/Library/Application Support/mexc-chart/workspace.json` |
| Windows | `%LOCALAPPDATA%\mexc-chart\workspace.json`, with native Known Folder/profile fallback |

Empty/relative overrides never create state relative to the working directory. Unix application directories/files use **0700/0600**; Windows inherits its containing per-user directory's ACLs, including for explicit storage overrides. The desktop owns versioned JSON checkpoints, synchronized same-directory atomic replacements, a last-good `.backup`, and a retained native single-writer `.lock`. A second instance cannot overwrite the first workspace; it shows a persistence-unavailable warning.

Unix replacements synchronize the parent directory; macOS file writes additionally request `F_FULLFSYNC`. Windows uses native write-through replacement without deleting the destination first. Completed Windows reset markers are durably retired before unlinking so an interrupted cleanup cannot replay an obsolete reset over newer saves. Windows cleanup unlinks can leave ignored temporary artifacts after interruption; Reset everything removes them. Filesystems that reject required synchronization report a save failure rather than silently weakening the write.

Restored configuration includes exact market/interval; indicator IDs/order/specs/source membership/visibility; profile IDs/specifications/visibility/fixed native bindings; pane height/collapse/active target/manual scales; time zoom/live-following or historical position; committed drawing books across instruments including hidden/locked objects; favorites/recents; and magnet. Auto-fit bounds are recomputed. Candles, profile outputs, priced executions, numerical outputs, raw source observations, CVD/OI/profile coverage, connections, caches, undo/redo, inspection, dialogs/drafts, temporary maximization, and armed tool/Repeat state are not saved.

The saved native market is catalog-validated before its feeds start. Missing listings/intervals remain explicit for repair, never replaced with default-market prices. Indicators recalculate and native contributors reestablish coverage; old CVD counters or OI values are not restored as live proof. Historical views page ordinary native history toward their saved visible range without inventing missing bars or moving manual bounds.

Search these actions with **Ctrl+K**, or use their **F1** buttons:

| Action | Clears | Preserves |
| --- | --- | --- |
| **Clear current chart** | Every indicator, chart-following profiles, fixed profiles bound to the current native instrument, current instrument drawings across all intervals (hidden/locked included), undo/redo, layouts/scales, drafts and interactions; returns to live/auto-fit | Exact current market/interval, other instruments' fixed profiles and drawing books, favorites/recents, magnet |
| **Reset everything** | All indicators/profiles/books/layouts, favorites/recents, preferences and owned recovery copies; returns to fresh BTC_USDC/5m defaults | Omarchy theme and desktop configuration |

Both require confirmation. Enter initially cancels; Tab/Shift+Tab switches the two choices, Enter activates the selected choice, and Escape cancels. Confirmed resets immediately record an authoritative reset intent, fence older writers/callbacks, and replace both checkpoint and backup before completing. Interrupted reset recovery cannot resurrect removed objects. These resets cannot be undone.

Corrupt checkpoints recover from a validated backup when possible, retaining a warning and the corrupt copy. Unknown newer schema files are preserved read-only: use a newer app, or explicitly **Reset everything**. A partial clear cannot safely interpret and delete only part of an unknown newer workspace. **Schema 4** adds VWAP resets and anchored drawing starts, safely migrating schemas 1/2/3 without losing existing indicators, drawings, profiles or presentation. Missing older profile-estimation settings default to automatic fallback; explicit policies remain intact. Version bumps protect newer checkpoints from readers that cannot represent their configuration. No older session-only files are guessed or migrated. Tests use private temporary storage, and synthetic stress launches bypass workspace storage entirely.

## VWAP

**Ctrl+K → Add VWAP → Enter** adds a normal price overlay, defaulting to **UTC day**. **Indicators → VWAP** exposes independent instances with **UTC day, week, month, or custom IANA daily-session** resets. In its settings, **Ctrl+Shift+R** cycles the reset; custom sessions expose timezone and local open/close fields with Tab-driven focus reveal. Apply edits only the selected instance; New starts another. Normal visibility, removal, Details and workspace restore apply.

- **Calculation:** cumulative `Σ(HLC3 × native candle volume) / Σ(native candle volume)`, where `HLC3 = (high + low + close) / 3`. This is an explicitly labeled **candle approximation**, not execution-price/tick VWAP. It uses the active chart's actual native interval and volume units, not a foreign/aggregate source or invented base conversion; changing interval can change the approximation.
- **Sessions:** UTC calendar resets and the same IANA/DST window rules described below for profiles. Custom windows run on all calendar days, not a holiday/exchange calendar. Candles crossing session boundaries are excluded rather than split; outside-session points have no value. Zero accumulated volume has no value, not zero price.
- **History:** the legend/Details expose the true origin and **covered window / Partial** state. Native history pages automatically toward required visible/latest reset origins and anchored starts without moving the camera. Unavailable origins, interior gaps, invalid input and coarse/unaligned boundaries stay partial; they never silently create a new full origin. Gaps/reset origins are not connected by a line, and an isolated value remains visible as a dot. The forming native candle remains provisional.

**Draw → Anchored VWAP**, tool **9**, or the command-palette drawing action starts one-anchor placement. Choose a native candle with the mouse, or use **Left/Right → Enter** (Shift moves ten native candles). The curve has **only a start time**: prices are derived from candle HLC3 and volume. Click the curve to select; drag its start handle, nudge with Left/Right, or use **Ctrl+E → UTC start → Ctrl+Enter** to change the origin. Exact ISO-8601/millisecond starts persist; a typed between-bar start uses the first later native opening and is labeled partial, without prorating a candle.

Anchored VWAP uses ordinary drawing appearance, hide/show, locking, deletion and per-instrument undo/redo—not profile configuration history. It binds to the exact native exchange/product/symbol, survives timeframe changes and returning to that instrument, and restores across launches. Only configuration is saved; native values and coverage are recomputed. Standard and anchored VWAP share the same incremental engine; tail replacements/corrections update the affected suffix, while prepends/reset changes reseed. Neither painting nor crosshair movement calculates the series.

## TPO and Volume Profiles

**Indicators** includes **TPO** and **Visible Range Volume Profile** in its ordinary type chooser. In that editor, Ctrl+T adds TPO; Ctrl+V adds visible VP outside text inputs (parameter paste remains normal). **Draw → Selected Range Volume Profile**, or Ctrl+P in Draw, starts two-anchor fixed placement. **Ctrl+K → Add TPO profile / Add Volume profile / Add Visible Range Volume Profile / Selected Range Volume Profile** exposes the same profile engine. **Configure new … profile**, the Profiles tab, and exact-ID results expose settings, visibility, deletion, and source details. Apply keeps the editor open; Ctrl+N starts another instance. Histograms remain passive, clipped, theme-native graphics and do not change price auto-fit.

### Calculation and source identity

- **TPO:** native **1-minute bar-range TPO**, independent of the displayed candle interval—not tick-by-tick trade reconstruction. A row counts once per block even if many native bars revisit it. Default blocks are 30 minutes. Compact blocks and expanded letters share the same calculation; letters require enough row height/time width to be readable.
- **Volume:** prefers actual priced native executions/aggregates with native identities and Decimal quantities. **Automatic estimation is enabled by default:** a requested native minute with incomplete execution proof uses its available native 1-minute candle quantity, distributed uniformly across its occupied price rows. It replaces that minute's partial executions, never adds them twice. Fully proven execution intervals retain exact contributions; recovery replaces estimates. Closed partial-minute edges are time-prorated; forming candles already report elapsed quantity, not forecast volume. Invalid/missing candles retain only observed executions and an explicit gap. **Estimated / Mixed** badges, row inspection and quantity disclosure identify approximations. Ctrl+9 in Volume settings toggles fallback; Ctrl+S commits an exact-only choice. Native base/contract units are preserved; neither dollar notional nor native execution coverage is invented.
- Rows are anchored to a zero-origin grid of **authoritative native tick size × ticks per row**. Automatic row sizing resolves a saved configuration once; zooming or changing the display interval does not silently rebin. Ctrl+R explicitly resolves new rows from the current native price reference. Missing authoritative tick metadata makes the profile unavailable rather than guessing from decimal places.
- **POC** is the maximum-weight row; ties choose the row nearest the distribution midpoint, then the lower row. The displayed price is its lower-middle native tick. Default **70% value area** expands one adjacent whole row at a time toward greater weight, choosing the lower side on a tie. VAH/VAL include the boundary rows; these are documented conventions, not vendor-exact parity claims.

### Ranges, sessions, and auction structure

Calendar sessions use UTC days/weeks/months. Custom daily sessions use an **IANA timezone**, inclusive local open and exclusive local close, on all calendar days—not an equity-market holiday/weekend calendar. A close before the open runs overnight; equal times mean one full local day. DST uses the earliest ambiguous open and latest ambiguous close; nonexistent local minutes advance to the first valid minute. Collapsed windows are skipped and the latest opening wins overlaps. Details disclose the actual UTC bounds.

Session and visible-range profiles follow the active native instrument. Fixed ranges bind to **exchange/product/native symbol**, excluding the candle interval, and remain dormant on other instruments. Return to their bound instrument before chart-relative rebinning or range placement. Fixed UTC fields preserve milliseconds; TPO requires whole-minute bounds. Visible range means **visible time**, not the manual price scale; TPO uses only complete native minutes inside that time range. Fixed/visible Volume histograms dock on the right of the price plot; TPO stays time-anchored.

- **Developing / completed:** open formations remain provisional. Developing TPO distinguishes covered closed native minutes plus an observed provisional current minute from genuinely missing source history. A closed formation is complete only with proven source coverage.
- **Previous / naked levels:** certified closed origins only. Previous POC/VAH/VAL and naked POC-row references are session features, including monitoring between custom sessions. A naked POC retires when its row is touched. Origins are limited to loaded, certified native history and the 33-session calculation budget—not an assertion that every older naked level has been discovered.
- **Single Prints / tails:** one-block interior runs are distinct from terminal auction tails and retain their owner block. Certified interior zones extend forward, including when their origin is offscreen. Touch state distinguishes untouched, partially filled, filled, and unproved; filled zones no longer extend forward. Positive observed touches survive later coverage gaps. Tails stay formation-local.
- **Initial Balance:** actual native high/low over the first two TPO blocks—one hour at the default block size—not rounded outer bin edges. **Poor highs/lows** identify multi-block occupation of the extreme profile rows; interpretation depends on the chosen row grid.

Inspect a price for its TPO block count or profile quantity, including observed/estimated/mixed provenance. The single-line inspection slot stays stable while profiles are visible; hovering changes content, not chart layout. **Applied profile / provenance** shows bounds, as-of time, coverage/reasons, contributed Decimal quantities, levels, structure, and execution/candle history/live status. Estimated totals include candle approximations and are not exact traded-at-price quantities. A connected price feed or estimated profile never certifies historical execution completeness.

### Native Volume Profile capabilities

| Native scope | Priced source / limitations |
| --- | --- |
| Binance Spot, USDⓈ-M, COIN-M | Native aggregates through REST/WebSocket and official daily aggregate archives. Preserve native identity/quantity semantics; market aggregates are not raw individual fills. |
| OKX Spot / Swap | Native raw trades, `trades-all`, recent history and instrument-specific official UTC+8 archives. Live observations alone are not automatically certified complete. |
| Coinbase Exchange Spot | Native matches, REST trade-ID pagination/replay and heartbeat proof. Incomplete continuity/retention remains explicit. |
| Coinbase International | Public unauthenticated priced-execution profiles are unavailable; no substitution with Coinbase spot. |
| MEXC Perpetual | Native recent deals/live capture; no pageable deep execution history. Default candle fallback can show Estimated/Mixed profiles; exact-only historical sessions usually remain Partial. |
| MEXC Spot | Native priced tape is exposed with separate REST/protobuf identity namespaces, but authoritative native tick-size metadata is unavailable; profile rows are currently **unavailable**, not guessed. |

Coverage and native API limits are explicit. Binance futures REST retention is short; older requests use official archives where available. Archive publication, listing availability, failed requests, disconnections, and decode/request budgets can leave partial history. Native inputs are memory-only: restarting does not turn an earlier MEXC capture into durable historical proof.

### Profile keyboard controls

| In the profile editor | Action |
| --- | --- |
| Ctrl+Left / Right; Ctrl+Up / Down | Cycle type; select the exact existing instance |
| Alt+Left / Right; Alt+Up / Down | Cycle session/fixed/visible range; cycle UTC/custom session |
| Ctrl+N / Ctrl+S | New draft / apply selected instance without closing |
| Tab / Shift+Tab; PageUp / PageDown | Traverse fields with layout-driven focus reveal; scroll content/details |
| Ctrl+1…8 | Toggle histogram, levels, previous, naked, Single Prints/tails, IB, letters, poor extremes; unavailable options stay disabled |
| Ctrl+9 (Volume) | Toggle automatic candle fallback; Ctrl+S applies the setting |
| Ctrl+H / Ctrl+D | Show/hide / remove selected instance |
| Ctrl+R / Ctrl+Shift+R | Explicit rebin / retry shared native profile history without clearing observed tape |
| Ctrl+P | Transfer to two-anchor fixed-range placement |

For placement, click two times or use **Left/Right → Enter → Left/Right → Enter**; Shift moves ten displayed native bars. Escape cancels without committing. TPO anchors snap to whole UTC minutes. Profile settings and fixed bindings persist automatically; outputs, native tape, coverage, and uncommitted placement do not.

Fixed-profile range rails are explicit selection/body-drag targets; selected vertical endpoint lines resize the time range. Ordinary drawing hits, active tools and held gestures retain priority; **Space-drag** pans. **Draw → Profile ranges** lists existing fixed profiles; **Ctrl+K → “4 Volume select range”** selects an exact ID without a mouse. Selected profiles support **Ctrl+E** settings, **Ctrl+H** visibility and **Delete** removal; Escape cancels an uncommitted drag. Range edits preserve duration when translated, native binding and chart view. They save as profile configurations, not drawing-book undo/redo objects.

The worker shares source lifetimes per native instrument, independently of candle interval and data kind, including hidden instances. Priced consumers can share an existing CVD execution producer; Binance CVD remains on cheap native kline quantities. Closed fixed ranges without ongoing lifecycle work do not keep unnecessary live profile producers running. Aggregation stays off the GUI thread, uses identity-aware replacement/correction, and preserves inflight bulk history while repairing only the newer tail. Explicit profile limits include 250,000 source minute bars, 2,000,000 executions, 200,000 rows per price span/profile, 1,000,000 cached contribution rows, and 33 session origins. Limits report errors rather than silently changing rows or certifying truncated data.

The contribution budget includes retained fallback-minute entries, including zero-volume minutes, not just occupied price rows. Estimation does not change CVD or OI calculations.

## Drawings

**Draw** in the header or **Ctrl+Shift+D** opens the tool menu and object list. Drawings belong only to the price plot. Books are keyed by exact **exchange / product / native symbol**, not interval: switching instruments retains the previous book, returning restores it, and interval changes reproject the same anchors onto native bars. All committed books restore across launches, including hidden/locked objects and semantic styles. Selection, unfinished edits, and undo/redo history start fresh.

Tool keys **0–9**, while Draw is open, choose **Select, horizontal line, vertical line, trend segment, ray, rectangle/zone, Fibonacci retracement, text, measurement, and anchored VWAP** respectively. Creation tools clear the previous selection. Horizontal, vertical, text, and anchored VWAP tools need one anchor; the others need two. Click anchors, or use arrows to position a keyboard anchor and **Enter** to confirm it. Keyboard placement starts at the inspected/current real candle; Shift makes larger steps. Anchored VWAP snaps time to actual native candles and derives prices. Text opens a focused, validated text editor. Completing creation returns to Select unless **Repeat** is enabled. Repeat defaults off; toggle it with **Ctrl+Shift+R** in Draw or while a price drawing/tool is active. Successful text Apply also rearms a repeated Text tool; Escape or cancelling text disarms it and returns to Select.

Anchors are absolute **UTC milliseconds and native quote prices**, never candle indices or pixels. Free placement supports between-bar and offscreen/future coordinates. Optional **Magnet** snaps near an actual visible candle's OHLC point; it does not invent grid prices or candles. Native precision is retained in drawing labels, including nine-decimal instruments.

| Action | Keyboard | Mouse |
| --- | --- | --- |
| Select and edit a drawing | Draw list Up/Down; Enter opens properties | Click its body; drag body to translate or handles/corners to resize |
| Nudge selected unlocked drawing | Arrows with price selected; Shift ×10 | Drag; one completed gesture is one undo step |
| Open exact properties | Ctrl+E with a price drawing selected | Properties in the contextual row; Edit in the object list |
| Undo / redo this instrument's drawings | Ctrl+Z / Ctrl+Shift+Z outside property editing | Undo / Redo in Draw |
| Hide / show; lock / unlock | Ctrl+H / Ctrl+L in the object list | Contextual row or object-list controls |
| Delete selected drawing | Delete outside editors or in the object list | Delete in contextual row/list |
| Toggle candle magnet | Ctrl+M in the object list | Magnet checkbox |
| Apply property/text draft | Ctrl+Enter | Apply |
| Cancel / deselect | Escape cancels an active gesture/tool, then clears selection; in properties it cancels the draft, then closes Draw | Select / clear; Cancel edit; Close |
| Traverse / scroll properties | Tab / Shift+Tab; PageUp / PageDown | Fields; content scrollbar |
| Cycle ink / stroke pattern | Ctrl+Alt+K / Ctrl+Alt+P for an unlocked selected price drawing or in properties | Contextual Ink / Stroke controls; property dropdowns |
| Adjust selected drawing line width | Ctrl+Alt+`[` / `]` outside property editing | Contextual minus / plus |
| Repeat the chosen tool | Ctrl+Shift+R in Draw or with a price drawing/tool active | Repeat checkbox |
| Reverse Fibonacci / toggle right extension | Ctrl+Alt+R / Ctrl+Alt+E in Fibonacci properties | Reverse / Extend right checkboxes |
| Temporarily navigate over drawings or with a creation tool active | Hold Space before pressing the mouse button | Space-drag pans normally; an already-started drawing drag keeps ownership |

Context controls float over the price plot without resizing or replacing its canvas. They yield to held drawing/chart gestures, so selecting an object and clearing selection cannot swallow the first drag. A press chooses one owner and coordinate frame until release or cancellation; native updates, relayout, and redraw do not invent drawing movement. Movable unlocked bodies use Grab/Grabbing cursors; an anchored VWAP curve uses Pointer for selection and its start handle owns movement. Cursor and press picking share the same cached derived geometry and edit policy. Locked objects show NotAllowed and never pan the chart behind them. Chart inspection is cleared when drawing editing starts, and wheel navigation/arrow nudges cannot interleave with a held drawing edit.

Window focus loss, resize, and native-context changes cancel held input; Escape ends drawing edits and chart pans. Each plot/grip explicitly resets its own input epoch without resetting neighboring scroll state; this does not rely on iced 0.13 list-key behavior. Release after a canceled drawing edit cannot commit its old preview.

The object list includes hidden, locked, overlapping, and offscreen drawings. Properties accept exact UTC ISO-8601 values or integer milliseconds, prices, ink, line width/pattern, rectangle fill opacity, text/label size, text, and Fibonacci ratios as appropriate. Locking prevents movement, content, and appearance edits; explicit visibility, unlock, and deletion remain available. Invalid Apply leaves both the committed drawing and editable draft intact; validation and actions remain visible while content scrolls.

Fibonacci defaults are **0%, 23.6%, 38.2%, 50%, 61.8%, 78.6%, 100%**, with **0% at B/end and 100% at A/start**: `price = b + ratio * (a - b)`. Reverse uses `a + ratio * (b - a)`. Ratios are editable, finite, and unique; negative and extension ratios are supported. Right extension is optional. Labels use actual computed native prices.

Measurement reports signed native-price change, percentage (undefined when the first price is zero), elapsed time, and the **inclusive count of actual loaded native bars** between its UTC anchors. It does not divide elapsed time by the nominal interval to fabricate a count. Anchors beyond loaded history mark the count partial. Its bounded, native-shaped two-line label stays inside the visible price plot.

Drawing commands and transient previews have separate geometry layers/history; they do not restart native sources, recalculate indicators, refit scales, or change CVD origins. Contextual styles use exact drawing IDs/book generations and style-only undo deltas. Axis/grip ownership, existing wheel/pixel navigation, and lower-pane controls retain precedence. Modal entry cancels held drawing edits and rejects stale gesture messages while native data continues. The command palette specifically preserves creation anchors/tool/repeat and unapplied property/text drafts while suspending held drag/hover input.



## Market selection

**M** opens the market editor with search focused. Choose exchange, spot/perpetual product, and quote currency, then search native symbols or base coins. Only active native listings are selectable. Unlisted combinations explicitly show no available markets; neither the current chart nor the quote currency is silently substituted. Intervals come from the selected venue/product catalog: the union is **1m, 5m, 15m, 30m, 1h, 4h, 6h, 8h, 1d**, but not every scope offers every interval.

| Action | Keyboard | Mouse |
| --- | --- | --- |
| Choose exchange | Alt+Left / Alt+Right | Exchange dropdown |
| Choose spot / perpetual | Alt+Up / Alt+Down | Product dropdown |
| Cycle quote USD / USDT / USDC | Ctrl+Q | Quote dropdown |
| Search native instruments | Type in focused search | Search field |
| Choose a matching instrument | Up / Down; highlight scrolls into view | Instrument row / scrollbar |
| Choose native timeframe | Ctrl+Left / Ctrl+Right | Native interval dropdown |
| Apply selection | Enter in search; Ctrl+Enter anywhere in editor | Apply market |
| Cancel without switching | Escape | Cancel |
| Move focus | Tab / Shift+Tab | Click control |
| Retry scoped discovery | Ctrl+R | Retry catalog |

For example, within the default MEXC perpetual USDC scope: **M**, type `ETH_USDC`, **Ctrl+Right**, **Ctrl+Enter** selects ETH_USDC / 15m. Search ignores case and `_`, `-`, `/` separators, but selection still uses the exact native symbol. Interval shortcuts issued during catalog loading are queued for that scope, never guessed; changing scope clears stale interval intent. Successful Apply closes the editor, including applying the current market. Invalid/unavailable selections remain visible without changing the chart. Cancelling another scope does not leave its catalog warning under the active chart. USD inverse perpetual rows distinguish quote from settlement. Typing within an editor never pans or zooms the chart; market and indicator editors are mutually exclusive.

A switch of exchange, product, symbol, or interval preserves indicator instance IDs, parameters, and visibility, but clears old candles, inspection, pagination, and calculated outputs. Candle-derived indicators recalculate on the new bars, with periods measured in bars. Native OI observations survive interval-only changes of the same instrument; changing exchange/product/symbol clears them. Viewport resets to the latest 120 bars, live following, and auto-fit. Loading never shows another instrument's old prices or OI. Every history/subscription callback carries the full market identity and selection generation; catalog callbacks have an independent scope/generation. Delayed responses—including a previous visit to the same market—cannot modify the new selection.

Price axes and OHLC readouts use native instrument precision, including nine-decimal instruments such as PEPE_USDC. Indicator values on price-based series retain that precision; RSI/Stochastic use their own numeric scales. Fractional native volume is not rounded to whole contracts. Discovery and invalid-selection errors remain visible without changing the current chart.

Value ticks use bounded nice-number spacing appropriate to the available plot height. UTC ticks adapt to viewport width/span, including Monday-aligned weeks and real month/year boundaries; clipped or overlapping labels are omitted rather than piled onto an edge. An uninitialized time/price viewport has no fabricated epoch or 0–1 tick labels. Native volume, CVD and OI use compact K/M/B/T magnitudes without changing units or erasing tiny nonzero quantities. Indicator Details shows unabridged quantities with floating-point tails trimmed; hovering the OHLC readout exposes its unabridged native candle volume. Actual prices and price-derived readouts retain native precision.

Coinbase spot uses Coinbase Exchange; perpetual discovery/history uses **Coinbase International's true PERP instruments**, not expiring US futures. At verification, its public catalog contained 264 perpetuals, all PAUSED or DELISTED, and zero TRADING perpetuals. The selector therefore reports Coinbase perpetuals unavailable. A real historical BTC-PERP candle was verified through the adapter; inactive instruments are not made selectable.

## Indicators

A fresh workspace has no indicators; **I** opens the manager to add only those you want. Subsequent launches restore committed instance IDs, order, specifications, source choices, and visibility, then recalculate from fresh native data.

| Indicator | Initial parameters | Placement | Calculation |
| --- | --- | --- | --- |
| SMA | Period 20 | Price overlay | Arithmetic mean of closes |
| EMA | Period 20 | Price overlay | Initial period SMA, then alpha `2 / (period + 1)` |
| Bollinger Bands | Period 20, deviations 2 | Price overlay | SMA middle, upper/lower bands using population standard deviation |
| RSI | Period 14 | Lower pane, 0–100 | Wilder-smoothed gains/losses, seeded from 14 price changes |
| MACD | Fast 12, slow 26, signal 9 | Lower pane | Independently SMA-seeded EMAs; MACD line, EMA signal, difference histogram |
| ATR | Period 14 | Lower pane | Initial mean true range, then Wilder smoothing; first candle contributes high–low |
| Stochastic | Period 14, smooth K 3, smooth D 3 | Lower pane, 0–100 | High/low-range %K, SMA smoothing of K followed by SMA smoothing of D |
| CVD | Reset `continuous` | Signed lower pane with zero reference | Compensated cumulative sum of native aggressor-buy volume minus aggressor-sell volume |
| Open Interest | None; native quantity | Independent lower pane | Actual venue OI observations; no candle-derived calculation or notional conversion |

Parameters are editable. Add multiple instances of any type, such as EMA(20) and EMA(50), then independently configure, hide/show, or remove them. IDs and line patterns distinguish instances even with a monochrome theme. Bollinger plots three lines; no band fill. RSI shows 30/70 references and Stochastic 20/80.

### Manager controls

| Action | Keyboard | Mouse |
| --- | --- | --- |
| Choose indicator type | Ctrl+Left / Ctrl+Right | Type dropdown |
| Start a new instance | Ctrl+N | New |
| Select an existing instance | Ctrl+Up / Ctrl+Down | Instance row |
| Edit numeric parameters | Opening/type/instance changes focus and select the first numeric field where present; type to replace it; Tab / Shift+Tab reaches other fields; Ctrl+A replaces contents | Parameter fields |
| Add or apply changes | Ctrl+S or Enter | Add / Apply |
| Show or hide selected instance | Ctrl+H | Show / hide |
| Remove selected instance | Ctrl+D | Remove |
| Open selected instance details | Ctrl+J | Details in the fixed footer |
| Close manager | Escape; I when no text field is focused | Close |
| Cycle CVD / OI source mode | Ctrl+Shift+M | Chart / Single / Selected / All radios |
| Browse source exchange without choosing a market | Ctrl+Shift+Left / Ctrl+Shift+Right | Exchange checklist / Browse |
| Move exact native source cursor | Ctrl+Shift+Up / Ctrl+Shift+Down | Direct market selection also moves the cursor |
| Select / unselect exact native market | Ctrl+Shift+Space toggles cursor market | Single radio; Selected / All checkbox |
| Retry failed source discovery | Ctrl+R | Retry source discovery |
| Choose CVD reset | Ctrl+Shift+R cycles Continuous / UTC day | Typed reset dropdown |
| Scroll parameter / source content | PageUp / PageDown | Parameter/source scrollbar or wheel |

Mode radios stay visible. **Single** replaces draft membership only when an exact native market is explicitly chosen; **Selected / All** allow direct check/uncheck. The `>` keyboard cursor is separate from inclusion. Each exchange checklist row shows pending/failed discovery, native eligibility, chosen count, unavailable choices, and All required/missing state. Clicking an unchecked venue or Browse only reveals its markets; it never guesses a symbol. Unchecking an included venue explicitly removes all its draft choices. Unavailable chosen markets remain individually removable. Changes stay drafts until Apply, and invalid Apply leaves committed instances untouched.

Opening the manager, choosing a different type/instance, and starting New focus and select the first numeric parameter field where present; CVD and OI have no numeric text field. Stable field IDs preserve multi-parameter Tab/Shift+Tab editing; Ctrl+A replaces a field's contents. Successful Add selects the new instance and enters Edit mode, so subsequent Apply updates it instead of creating duplicates; Ctrl+N explicitly starts another. Local Settings opens its exact instance; reselecting that same instance preserves unapplied edits. Ctrl shortcuts remain active inside inputs, while plain typing/caret keys never move the chart. Invalid parameters leave the existing instance unchanged and focus/select the offending field. Changing a draft clears stale errors. Selection scrolls into view; removal selects a surviving neighbor, or starts a clean default draft after the last edited instance is removed. Periods must be 1–10,000, Bollinger deviations greater than 0 and at most 100, and MACD fast must be less than slow.

CVD has a typed reset selector instead of a period: **Continuous** (default) or **UTC day**. **Ctrl+Shift+R** cycles those two choices; **Ctrl+R** still retries source discovery. UTC day resets at 00:00 UTC and requires proven trade coverage from midnight. Invalid reset tokens cannot be entered. Multiple instances can use different resets independently without changing their source membership.

Open Interest has no period or reset parameters. Add it through the same type selector; instances share required feeds but retain independent source selections, visibility, and value scales.

When a selected source no longer matches the chart underlying/product or native CVD interval, or All coverage becomes unvalidated/incomplete, its legend names the blocking context and offers **Repair source** and **Use Chart**. Repair opens that exact instance without silently replacing its membership. Use Chart explicitly changes only its source, retaining ID, reset/parameters and visibility, and releases external feeds no longer required. Incomplete external details also offer Retry sources: owned source generations restart, old callbacks are rejected, and shared contributors affect all dependent totals until freshness recovers. No partial current total, substitute market or resampled CVD bar is introduced. The exchange checklist exposes every venue's state; the legend keeps the first actionable blocker compact.

Every lower-pane instance has an independent automatic/manual value scale and shares the main time viewport. Plot dragging pans shared time and that pane's values; right-axis wheel/drag zooms its values, and bottom-axis wheel/drag zooms shared time, anchored at the pointer/initial press. Manual bounds survive live updates, history prepends, time navigation, configuration changes, and hide/show; market switches reset numerical scales, and removal/overlay conversion discards obsolete pane scales. **F** targets the active plot; lower right-axis double-click restores that pane's auto-fit. Use PageUp/PageDown or the scrollbar to reach all panes. Inspection synchronizes timestamps and per-series readouts; an edge-clamped `YYYY-MM-DD HH:MM UTC` badge appears below cached ticks on each bottom axis. Escape/right-click clears inspection. Overlay values participate in automatic price fitting.

Pane layout is independent session presentation, not data configuration. When any lower pane is visible, the price plot defaults to **340 logical pixels** (bounded **160–1,000**); lower plots default to **190** (bounded **100–800**). Without visible lower panes, price fills the workspace. Drag or double-click each full-width grip below its Canvas; **Ctrl+Alt+Up/Down** adjusts the active plot by 32 pixels and **Ctrl+Alt+0** restores its default. **Collapse / Expand** and **Ctrl+Alt+C** unmount only a lower plot and its grip, retaining its title, units, quality/origin/as-of summaries, visibility, feeds and automatic/manual scale. Heights/collapse survive hide/show, parameter/source edits and native market/interval changes; numerical scales still obey their market-reset contract. Removal or conversion to an overlay discards that pane's layout, and a new ID never inherits it. Layout is not persisted, and modal entry cancels held resize gestures.

### Data and calculation behavior

`mexc-indicators` is an iced-independent Rust library. An `IndicatorSpec` carries typed parameters; each instance has a stable ID and visibility. Calculated outputs use candle-aligned `IndicatorPoint { time, values }` storage; unavailable values are `None`, never fabricated zeroes. Price-derived indicators render `-- (warming up)`; CVD renders `-- (incomplete)` when execution coverage cannot prove the value. OI uses a separate native millisecond-timestamped series, with no dummy per-candle outputs.

The engine borrows the chart's contiguous, sorted candle slice. Live replacement recomputes the tail from the preceding finalized state, rather than feeding the forming candle into EMA/RSI repeatedly. Corrections recompute the affected suffix; loading older candles reseeds from the expanded history. Therefore recursive values can change after more historical context is loaded. Calculation does not run during rendering or crosshair movement, and same-length tail replacement does not clone the history or allocate output storage.

RSI is 50 for an entirely flat window, 100 with only gains, and 0 with only losses. Stochastic is 50 when its high/low range is zero. These documented seed conventions do **not** promise exact parity with TradingView or another vendor. Indicators operate on loaded candles; periods exceeding available history remain in warmup until enough older data is loaded. Hidden instances continue updating so showing them is immediate.

### CVD accuracy, coverage, and resets

CVD uses execution **volume**, not trade counts, price direction, candle shape, or a taker-count ratio. Native decimal quantities are summed/subtracted before one final `f64` conversion; the cumulative indicator uses compensated summation. **Chart mode** follows native volume units: base asset for spot and Binance USDⓈ-M, contracts for Binance COIN-M, OKX swaps, and MEXC perpetuals. External modes use the authoritative base normalization described below; no inverse conversion is guessed.

Each pane labels the native units, origin, quality, and latest covered timestamp. **Exact** means confirmed native taker-side aggregates or complete native execution replay; **Reported (unconfirmed side)** means MEXC's reported direction is retained without claiming confirmed aggressor semantics. An unavailable range is **Missing**, not zero. A zero delta is valid only when complete coverage proves balanced buys/sells or no executions.

| Native scope | Historical flow | Live flow |
| --- | --- | --- |
| Binance Spot, USDⓈ-M, COIN-M | Native kline total/taker-buy quantities | Native kline total/taker-buy quantities |
| OKX Spot | Complete raw history or official daily execution archives, reconciled to candle volume | `trades-all`, with closed-bar raw replay and volume reconciliation |
| OKX Swap | Instrument-specific contract-taker statistics only when native volume reconciles; otherwise complete raw history/archives | `trades-all`, with closed-bar raw replay and volume reconciliation |
| Coinbase Exchange Spot | Complete native trade-ID replay; maker side is inverted to aggressor side | Public matches/heartbeats repaired through REST trade-ID replay |
| Coinbase International Perpetual | Unavailable through the public unauthenticated sources | Unavailable; authenticated `MATCH` is outside this application's scope |
| MEXC Spot | Exact `aggTrades` only within retained, unsaturated, volume-reconciled ranges | Reported direction, unconfirmed, session-only |
| MEXC Perpetual | Unavailable; the recent-trades endpoint is not arbitrary historical coverage | Reported direction, unconfirmed, session-only |

OKX archives are published on the venue's UTC+8 schedule and are not immediately available for today's bars. Coinbase and raw OKX backfills can require many native pages: prices arrive first, replay progress remains visible, and cancellation/market switches stop enrichment. Never interpret a partially replayed range as a completed CVD.

- **Continuous:** begins at the first fully covered bar after a leading unavailable prefix, with an explicit origin. A missing interval after that origin invalidates subsequent cumulative values until coverage is genuinely repaired; it does not silently restart. Loading older proven history rebases the origin and sum.
- **UTC day:** resets at 00:00 UTC and requires proven coverage from midnight. An incomplete earlier part of the day cannot be skipped. The next fully covered day can recover independently. Proven empty execution intervals need not have fabricated price candles.
- A flow snapshot is an absolute bucket delta with its own unsigned volume and as-of timestamp, not an increment and not necessarily contemporaneous with the OHLCV snapshot. Replacement/correction recomputes only the affected CVD suffix. An unfinished previous bucket cannot be carried forward as though complete.
- Starting capture mid-bar leaves that bar incomplete unless native replay repairs it. Disconnects revoke unfinalized capture, retain completed history, and expose incompleteness. Historical unavailability cannot overwrite independently measured newer live flow; stale responses cannot overwrite a newer invalidation.

Adding chart-sourced CVD late backfills the loaded range without discarding prices. Hidden CVD instances keep updating. Additional native trade feeds/replays run only for required CVD sources; Binance's embedded native flow comes free with its ordinary klines. **L** and **R** change the viewport, not the CVD reset or origin.

### Open interest: native quantity, history, and live observations

OI is the venue's outstanding native quantity, not volume, CVD, estimated position changes, or price-converted notional. Real zero is valid; missing history is not zero. Units and timestamp provenance appear in each pane; source capabilities/errors wrap in the shared footer.

| Perpetual source | Native unit | Historical OI | Live observations |
| --- | --- | --- | --- |
| Binance USDⓈ-M | Base asset, e.g. BTC | Finest 5m, latest month, up to 500 observations/page | Public native OI, polled every 5s; venue timestamp |
| Binance COIN-M | Contracts | Finest 5m, latest 30 days, perpetual-only pair statistics, 500/page | Public native OI, polled every 5s; venue timestamp |
| OKX Swap | Contracts | Instrument-specific 5m history, latest 1,440 entries, 100/page | Public native OI, polled every 5s; venue timestamp |
| MEXC Perpetual | Contracts (`holdVol`) | Unavailable: no documented public OI history | Actual session snapshots every 5s; explicitly **observed** receipt time |
| Coinbase International | Venue-described native positions (`open_interest`) | Unavailable: no documented public OI history | Public snapshots every 5s with observed receipt time, only when an eligible native PERP is listed; none active at verification |
| All spot markets | Not applicable | Unavailable | Unsupported, not a zero-valued series |

Native history is backfilled independently to the loaded candle range within source retention. Older cursors are exclusive milliseconds. Historical observations keep their native timestamps on every chart interval: a 1m chart does not create 1m OI from 5m samples, and a larger interval does not aggregate away source observations. Live snapshots are retained separately from the venue's periodic historical statistics; short-cadence observations win timestamp collisions over delayed historical pages. Visible gaps exceeding expected cadence plus timing jitter break the path. A singleton observation is plotted immediately.

MEXC's ticker timestamp is documented as last-trade time, not OI as-of; Coinbase's quote timestamp is not OI as-of either. Neither is relabeled as an OI timestamp. Their real quantities are recorded at receipt time and labeled **observed**, not claimed exchange-as-of history.

Hidden instances continue updating; native OI I/O runs only while an OI instance exists, separately from price/CVD. Removing the last instance cancels the producer and older requests. Market changes and removal/re-addition reject stale callbacks. Native observations survive an interval-only switch of the same instrument. Manual value bounds survive arrivals/backfill/hide/show; time stays shared. When OI is the active target, `[` / `]` steps actual native observations, and cursor readouts show the last actual observation at or before the cursor. History failure does not stop live OI; its warning remains visible until a successful native refresh.

Sources: [Binance API documentation](https://developers.binance.com/en/docs/llms.txt), [OKX contract OI history](https://www.okx.com/docs-v5/en/#trading-statistics-rest-api-get-contract-open-interest-history), [MEXC ticker](https://www.mexc.com/api-docs/futures/market-endpoints/get-ticker-contract-market-data), [Coinbase instrument details](https://docs.cdp.coinbase.com/api-reference/international-exchange-api/rest-api/instruments/get-instrument-details).

### Per-instance CVD / OI sources and strict aggregates

Source configuration is independent of the price chart and other indicator instances:

- **Chart** preserves the existing chart-native feed and its units.
- **Single** plots one explicitly chosen native market in base-asset quantity, e.g. Binance BTCUSDT CVD/OI on MEXC BTC_USDC prices.
- **Selected** plots one aggregate over the explicitly chosen markets.
- **All eligible exchanges** plots one aggregate and requires an explicit native-market choice on every eligible discovered exchange. It does not guess BTCUSDT versus BTCUSDC or substitute a quote/settlement currency. Multiple explicit markets on a venue are allowed.

Sources must have the chart's catalog-confirmed underlying and product; spot and perpetuals are never mixed. CVD also requires the exact native chart interval and usable native flow capability; no candle resampling. OI retains real millisecond observations and does not require that exchange to offer the price chart's candle interval. Discovery failures/pending catalogs block All rather than silently dropping a venue. Coinbase had no active eligible perpetual listings during verification.

All eligibility is revalidated after native underlying/product/interval changes, committed source/spec edits, and accepted discovery outcomes, even with settings closed. The editor and running aggregate share one native eligibility rule. For example, OKX can be ineligible for 8h CVD but required at 5m: an existing Binance+MEXC All instance becomes nonnumeric until an OKX native market is explicitly chosen. Returning to 8h keeps that choice but marks it ineligible rather than dropping it to restore a subset. OI does not acquire a false candle-interval requirement.

Unknown, failed or malformed discovery and incomplete All membership invalidate current and historical aggregate values for the uncertified universe, without choosing/removing markets. Compatible selected inputs remain available for rebuilding after explicit repair. Discovery retries invalidate old ownership generations; stale native responses and redundant source completions cannot replace accepted catalogs.

The independent indicator library starts new/changed All configurations pending. Integrators must certify the entire native exchange universe with `IndicatorSet::set_all_source_error(id, None)` before refreshing accepted All output, and pass `Some(reason)` whenever coverage is unvalidated/incomplete. Revalidate on native context/catalog/spec/membership changes; raw input freshness/coverage is still independently required. The desktop performs this outside unchanged tick refreshes.

External CVD/OI use **base-asset quantities**, never sums of incompatible raw contract counts. Spot and Binance USDⓈ-M quantities already use base units. Linear OKX/MEXC contract quantities use native contract-size metadata. Binance COIN-M CVD uses actual base total/taker-base kline fields; its base OI uses native `sumOpenInterestValue` statistics with honest **5m**, not 5s, cadence. OKX OI uses `oiCcy`/historical base quantity, and inverse CVD converts each execution using its actual price while independently reconciling native contract volume. No candle-close/average-price conversion of net deltas.

MEXC inverse contracts lack a supported authoritative base conversion and are explicitly excluded from external source selection; native Chart mode remains available. Coinbase International's documented native positions do not establish base-asset OI and cannot be guessed. Unsupported normalized requests remain explicit missing/status outputs, not zeroes or approximations.

**Every selected source is required.** CVD adds raw proven bucket deltas before a single compensated accumulation, with a common origin/reset and weakest contributor quality. It never adds independently originated CVDs. Continuous starts at the first common proven bucket and cannot silently restart after a missing middle interval; UTC-day requires every source to prove midnight coverage. Membership changes reset the aggregate origin. MEXC's missing historical flow may make the first common origin a newly captured full bucket; its Reported side remains unconfirmed. An unrepairable native gap leaves a continuous aggregate incomplete. Isolated proven CVD points and the latest point are marked when visible, so a one-bucket origin/reset appears before another point can form a line.

OI sums each source's latest actual observation only at real native event timestamps, when all are fresh within **3× declared cadence**. Stale/disconnected/missing contributors suppress the current total; proven history remains inspectable. Source as-of minimum/maximum and venue versus observed provenance are disclosed for current and inspected values. Asynchronous totals are **not exact simultaneous snapshots**.

One pane per instance, not one per contributor. Chart, foreign, selected, and All instances can coexist with independent manual/automatic value scales and shared time. Hidden instances remain subscribed. External CVD and OI have separate owned per-native-market lifetimes; duplicate external requirements share a producer, removing one instance does not stop another, and last-reference removal/drop cancels native requests and children. Parameter/reset changes reuse feeds. History follows the loaded chart range within native retention; unavailable history is not synthesized.

Sources: [Binance inverse base kline fields](https://developers.binance.com/legacy-docs/derivatives/coin-margined-futures/market-data/rest-api/Kline-Candlestick-Data), [Binance inverse OI base units](https://developers.binance.com/legacy-docs/derivatives/coin-margined-futures/market-data/rest-api/Open-Interest-Statistics), [OKX OI](https://www.okx.com/docs-v5/en/#public-data-rest-api-get-open-interest), [MEXC contract metadata](https://www.mexc.com/api-docs/futures/market-endpoints/get-contract-info).




## Colors and optional Omarchy integration

Without a configured theme, the app uses a colorful built-in dark palette: navy background **`#111827`**, near-white text **`#E5E7EB`**, mint rising candles **`#34D399`**, coral falling candles **`#F87171`**, blue accents **`#60A5FA`**, and slate secondary text **`#94A3B8`**. **No Omarchy installation or theme file is required, and absence is not a theme error.**

On Linux, optional Omarchy discovery polls once per second and updates chart/interface colors without restarting:

- `$XDG_STATE_HOME/omarchy/current/theme/colors.toml`, defaulting to `~/.local/state/omarchy/current/theme/colors.toml`.
- Older installations: `$XDG_CONFIG_HOME/omarchy/current/theme/colors.toml`, defaulting to `~/.config/omarchy/current/theme/colors.toml`.

macOS and Windows use the built-in palette unless **`MEXC_CHART_THEME`** specifies an explicit TOML path. That override works on all platforms and takes precedence over automatic discovery. An explicitly configured missing file, or an unreadable/malformed/disappearing selected theme, remains an error: retain the last good palette (or the built-in palette at startup), show the failure, and retry at runtime. Never silently replace a selected broken theme with another file.

Theme files use semantic background, foreground, red, green, accent/blue, and muted colors; legacy `bg`/`fg` and ANSI `colorN` aliases are accepted. Contrast is corrected once at palette load/reload toward a readable foreground. Rising candle bodies are hollow and falling bodies filled; IDs, labels and line patterns distinguish series without relying on color alone.

Market/source badges inherit the active palette: accents identify selection, green identifies eligibility/coverage, red identifies failures/missing requirements, and neutral states identify pending/ineligible sources. State labels and native quantity/provenance remain explicit. Monochrome Omarchy themes remain monochrome rather than receiving forced standalone hues. The application never edits desktop configuration or installs hooks.

## Structure

- `crates/model`: native market, authoritative tick metadata, catalog, candle/flow/OI, and priced execution/coverage contracts. Shared models have no serialization dependency.
- `crates/indicators`: typed configurations, instance lifecycle, and incremental numerical calculations; no iced or networking dependencies.
- `crates/indicators/src/profiles.rs`: deterministic native-tick TPO/Volume calculations, exact quantities/value areas, cached contributions, session/DST bounds, and certified reference/mitigation lifecycles.
- `crates/indicators/src/sources.rs`: per-instance source configuration, base-normalized inputs, strict common-origin CVD and native-time OI aggregation, contributor/as-of state, revision/dirty-suffix tracking.
- `crates/data`: in-process `DataService` and four explicit native providers. Eight single-flight catalog slots share `Arc<Catalog>` results; successful discovery is cached for 60 seconds, failures for one second. Pooled clients handle ordinary requests and longer execution-archive downloads. Native subscriptions/replays use bounded channels, visible status, owned cancellation, and shared request pacing.
- `crates/data/src/open_interest.rs`: public native per-instrument OI history/snapshots, capability and timestamp provenance, retention/cursor handling, and cancellation-safe polling.
- `crates/data/src/executions.rs` and provider execution adapters: shared native execution ownership, priced history/live/archive ingestion, identity reconciliation, explicit coverage/revocation, and cancellation.
- `crates/desktop/src/state.rs`: chronological contiguous candles, merge-generation protection, viewport, independent pane scales, indicator instances, inspection, and typed `Action` commands. Ordered flow batches walk candles once and recalculate one affected CVD suffix.
- `crates/desktop/src/chart.rs`: visible-range cached Canvas geometry, pointer-to-action translation, and independent value/shared time transforms. Series and histogram geometry are clipped before rendering; far-offscreen manual scales cannot trigger unbounded dashed-path expansion.
- `crates/desktop/src/chart_surface.rs`: stable canvas layout, capture-aware floating chrome/layers, and per-plot/grip input epochs that preserve unrelated widget state.
- `crates/desktop/src/pointer_input.rs`: window-root event-time cursor routing before scroll transforms; prevents a batch's final cursor position from relocating earlier presses.
- `crates/desktop/src/main.rs`: command dispatch, request sequencing, keyboard/button adapters, and the small interface.
- `crates/desktop/src/indicator_manager.rs`: keyboard/mouse editor that translates configuration operations into the same typed actions.
- `crates/desktop/src/market_selector.rs`: searchable exchange/product/quote/native-timeframe editor; validated selection uses the same typed actions as keyboard shortcuts.
- `crates/desktop/src/profiles_runtime.rs`, `profile_controls.rs`, and `profile_layer.rs`: independent execution/native-minute jobs, generation-fenced background calculations, estimation/provenance controls, stable inspection, and passive histograms with explicit editable fixed-range rails.
- `crates/desktop/src/key_help.rs`: borrowed shortcut catalog, context ordering, and state-preserving modal input/overlay isolation.
- `crates/desktop/src/command_palette.rs`: contextual searchable typed-command targets, query/navigation, and keyboard-help registry reuse.
- `crates/desktop/src/goto_time.rs`: strict UTC editor; the window controller owns sequential native paging and coverage feedback.
- `crates/desktop/src/stress.rs`: explicitly labeled synthetic scenarios, bounded timing samples, and process-memory reporting; disabled in normal launches.
- `crates/desktop/src/feeds.rs`: owned native producers and ordered Iced batches, capped at 64 events or 16 ms; one shared subscription scope rather than per-event market-string copies. Cancellation/drop stops native I/O and provider children.
- `crates/desktop/src/source_feeds.rs` / `source_controller.rs`: scoped base-quantity producers, independent CVD/OI ownership, shared source demand, discovery, live/history merging, pagination targets, and targeted external-pane invalidation.
- `crates/desktop/src/history.rs`: grouped request, cancellation, progress, retry, and repair state; history pages are shared through `Arc<HistoryPage>`.
- `crates/desktop/src/palette.rs`: colorful built-in palette, optional Linux Omarchy discovery, and explicit cross-platform theme overrides with live reload.
- `crates/desktop/src/pane_resize.rs`: Canvas grips, anchored height adjustment, cancellation and double-click reset; committed heights are saved by workspace persistence.
- `crates/desktop/src/presentation.rs`: native quantity disclosure, bounded nice-number ticks and UTC calendar ticks.
- `crates/desktop/src/workspace.rs`: desktop-only versioned durable DTOs, structural/native-configuration validation, and scoped reset snapshots.
- `crates/desktop/src/workspace_store.rs` / `workspace_store_platform.rs`: native per-user storage, process locks, synchronized atomic checkpoint/backup writes, recovery and authoritative reset replay.
- `crates/desktop/src/workspace_runtime.rs`: startup revalidation, dirty/debounced save lifecycle, normal-close flush, revision fencing, and restore/reset integration.
- `crates/desktop/src/workspace_controls.rs`: themed keyboard-accessible confirmation and failed-close choices, shared by palette/F1/button commands.

State is the source of truth, not rendered geometry. Built-in indicators extend the typed kind/spec and manager; calculated indicators use structured candle-aligned points, while native-backed OI retains its independent source series. No dynamic plugin registry or scripting runtime is included.

## Native exchange sources

Only external exchange REST/WebSocket protocols remain:

| Exchange | Spot | True perpetuals |
| --- | --- | --- |
| Binance | `api.binance.com/api/v3`; spot kline WebSocket | USDⓈ-M `fapi.binance.com/fapi/v1`, COIN-M `dapi.binance.com/dapi/v1`; native kline WebSockets |
| OKX | `www.okx.com/api/v5`, `SPOT` | Same API, `SWAP`; native candle business WebSocket |
| Coinbase | `api.exchange.coinbase.com/products` | `api.international.coinbase.com/api/v1/instruments`, `PERP` |
| MEXC | `api.mexc.com/api/v3`; native candle REST polling | `api.mexc.com/api/v1/contract`; `wss://contract.mexc.com/edge`, `sub.kline` |

Binance USDⓈ-M kline streams use the documented [`/market/ws` route](https://developers.binance.com/en/docs/products/derivatives-trading-usds-futures/websocket-market-streams/Important-WebSocket-Change-Notice.md); the retired route can connect without delivering candles. Coinbase native candles are polled every two seconds; MEXC spot every five seconds. These are native OHLCV responses, not local aggregation or fallback feeds. Coinbase trade-only candles may have gaps. MEXC [changed its futures REST base domain on January 19, 2026](https://www.mexc.com/api-docs/futures/update-log#2026-01-19); the futures WebSocket host remains separate. Standard trade candles are used, not index-price or mark/fair-price candles.

## In-process data contract

`mexc_data::DataService` is a cheaply cloned handle to pooled native clients and shared catalog state:

```rust
data.catalog(exchange, product).await?;
data.retry_catalog(exchange, product).await?; // Explicit retry of completed failed discovery
data.history(&market, before).await?;
data.history_with_flow(market, before, history_sender).await?;
data.stream(market, cvd, market_sender, shutdown).await?;
data.open_interest_history(&market, before_ms).await?;
data.open_interest_stream(market, oi_sender, shutdown).await?;
data.execution_history(&market, start_ms, end_ms).await?;
data.execution_stream(market, execution_sender, shutdown).await?;
data.history_in_base(&market, before).await?;
data.history_with_flow_in_base(market, before, history_sender).await?;
data.stream_in_base(market, cvd, market_sender, shutdown).await?;
data.open_interest_history_in_base(&market, before_ms).await?;
data.open_interest_stream_in_base(market, oi_sender, shutdown).await?;
```

Catalogs contain only eligible native instruments and supported intervals. History validates the full market identity and returns ascending candles with an exclusive Unix-second `next_before` cursor; `None` means the native request returned no candles. Native validation failures are Rust errors, not localhost HTTP responses.

Optional CVD enrichment sends `HistoryEvent::History { page: Arc<HistoryPage> }` first, followed by native flow/progress and exactly one `Finished` or `Error` terminal event while the consumer is active. Binance embeds native taker flow in its candles and needs no separate replay. Dropping the request/receiver cancels enrichment; dropping the live subscription stops its producer and native children.

OI history returns ascending `OpenInterestSample` observations, support/unit/cadence metadata, and an exclusive `next_before_ms` cursor. The independent stream sends `OpenInterestEvent::Support`, a shared native history page when available, actual samples, and source status. Quantities remain finite/nonnegative; venue and observed receipt timestamps are distinct. Watch shutdown or receiver-drop cancels validation, HTTP reads, queue sends, and timers. Historical errors remain explicit while live observations continue.

Execution history returns native decimal `Execution { time_ms, id, price, quantity }` rows and half-open positive-proof coverage spans. Streams publish shared execution pages, native status, and explicit invalidations. Identity, coverage and source ownership are per native instrument, not displayed candle interval. Subscribers deduplicate their own rows so recovery replay can restore a lagged consumer's missing quantities before coverage becomes complete.

The explicit `_in_base` APIs preserve native OHLC/candle volume while normalizing flow `delta`/`volume` and OI quantity into the catalog base asset. Normalization does not alter quality, coverage, timestamps, cancellation, or terminal ownership. Base OI validates the instrument independently of candle timeframe. `mexc_data::base_quantity_factor` exposes only authoritative spot/linear scaling; inverse/undocumented conversions are rejected.

`VolumeFlow` contains paired optional `delta`/`volume`, quality, `covered_from` in Unix seconds, and `through_ms` in Unix milliseconds. Numeric snapshots prove capture from bucket opening through their as-of time; finalization proves the bucket end. Missing coverage is never numeric zero. Corrections can target earlier candles.

The desktop retains full market/selection/flow generations in the subscription scope and rejects stale callbacks before modifying pipeline state. Ordered batches preserve candle/status/revocation boundaries; consecutive flow snapshots are merged with one CVD suffix recalculation. Native `MarketEvent::Resync` repairs recent history and pages back across gaps. There are no local sockets, NDJSON/JSON copies, companion server, or compatibility endpoints.

## Verification

```sh
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace
```

Standalone/portability verification passed workspace/all-target tests, strict stable Clippy, an optimized desktop build, and the full Rust 1.89 workspace/all-target compile check. Full workspace/all-target cross-checks passed for **Windows x86-64 GNU** and **macOS ARM64** with a temporary cross C compiler; those are compilation checks, not native GUI runs. The native CI matrix defines real platform test/build jobs but was not executed from this Linux session. macOS currently inherits an upstream `block 0.1.6` future-Rust-compatibility warning, which did not fail the cross-check.

Real Linux Wayland smoke used isolated HOME/XDG storage and native MEXC prices: the former neutral palette/missing-theme warning were reproduced, then the new colorful chart/settings ran without Omarchy or a theme warning. Existing VWAP identity restored, RSI autosaved/restored, and application-directory/checkpoint/backup/lock permissions remained 0700/0600. Optional live discovery loaded the actual desktop palette copied only into the private smoke root; a malformed replacement retained its colors with an error. Native Windows/macOS window/input/graphics execution still requires those operating systems; no equivalent runtime claim is made from cross-checks.

An explicitly missing theme override showed an error alongside the built-in colors instead of silently selecting an automatic theme. Creating the requested file loaded it live and cleared the warning while both saved indicator instances and native prices remained active.

VWAP acceptance passed **503 workspace tests**, strict all-target Clippy, and an optimized desktop build. Native Wayland checks used real MEXC BTC_USDC candles, the active Omarchy palette and private XDG storage at **749×814 and 520×560 logical pixels**: independent day/week/month/custom resets, real month-origin history paging from Partial to covered, source/formula/units disclosure, and focused custom-session fields revealed above fixed actions. Anchored checks covered native-candle keyboard placement, start-handle movement without camera/native-binding changes, exact millisecond UTC editing, undo/redo, hide/lock, deletion/undo, 5m→15m recalculation and relaunch restoration. Cursor regression reproduced uncached derived-curve picking before the fix; hover and held feedback now share actual curve geometry/edit capability. A genuine schema-3 checkpoint upgraded to 4 through native VWAP addition while preserving all four existing profile configurations, including exact-only policy, hidden native binding and millisecond fixed bounds.

Scroll/modal/resize acceptance used real Wayland input, native BTC_USDC data, the active Omarchy palette, and isolated workspace storage at **749×814 logical pixels**. Scrolling retained pointer-aligned price crosshairs and shared lower-pane inspection. Burst price resizing continued across scrollbar appearance (340→420), a scrolled price drag reached 480, and a burst RSI grip drag reached 250 from 190 without changing the time viewport or automatic scales. Repeated CVD source radios, labels/padding, nested reset selection, invalid All-source Apply, and an inside press released outside retained the editor/draft; a valid Chart-source UTC-day Apply retained the instance ID and committed its reset. A fast combined modal sequence also reproduced iced's final-cursor batching error before window-root routing fixed it.

Drawing-input reliability acceptance used real Wayland pointer/keyboard events and isolated XDG storage at **749×814 and 520×560 logical windows**. It reproduced first-selection plot displacement and a lost first background pan, then verified immediate line dragging without camera changes, the first deselection/background drag, rectangle body/corner edits, selected Fibonacci endpoint priority over an overlapping rectangle, and locked-object rejection without camera movement. Space-pan released over floating controls did not leave a ghost drag. Holding Space across an actual focus transfer and releasing it in another owned window no longer stole the first post-focus drawing drag. Resize canceled an unfinished drawing edit and a held pane grip; RSI panning retained independent value bounds, shared time, and unchanged drawing geometry. Opaque themed chrome remained above cached candle meshes rather than losing its background to renderer-layer ordering.

Phase 9 native Wayland acceptance used isolated XDG storage with the active Omarchy palette: default restart restored exact ETH_USDC/5m, stable indicator/drawing IDs, a collapsed manually scaled RSI pane, and an older historical viewport after real native history paging. A normal close immediately after adding MACD flushed the addition before the debounce elapsed. Clear-current cancellation preserved everything; confirmation removed the current instrument's hidden/locked drawing and indicators while retaining the BTC book and exact saved markets across another restart. Reset-everything restored empty defaults. An actual filesystem write failure kept the window open with Retry selected; repairing the path and retrying saved the newest state before exit. A damaged main file was quarantined and its validated backup restored with a visible warning. An unknown newer schema remained byte-for-byte untouched after a rejected partial clear.

The final optimized release was also checked at **749×814 and 520×560 logical windows**: reset scope text and keyboard selection remained readable, an unavailable saved symbol stayed offline until explicit native-market repair, and the repaired exact identity was automatically saved. A clean protected workspace closed without a false save-failure prompt; editing it instead required an explicit exit without saving and left the newer file untouched. Explicit Reset everything replaced that newer file only after confirmation. A final ordinary restart restored the historical chart centered on `2026-10-10 12:00 UTC`, including the correct center notice. A 20-second synthetic stress launch created no workspace directory in its isolated state root.

Phase 8 native Wayland acceptance exercised **749×814 and 520×560 logical windows**: searched EMA/RSI/MACD additions, existing-instance settings/details, editor-context command exclusion, second-field MACD draft/focus restoration, keyboard pane reorder/maximize/restore, exact session favorites and native BTC→ETH→BTC quick switching, invalid UTC date correction, sequential history loading to an older UTC target, and return-live cancellation. Drawing checks covered repeated Trend and Text commits, mouse/keyboard anchors, first-anchor preservation through palette cancellation, contextual style/undo, Escape disarming Repeat, and a complete right-edge measurement label showing signed price/time and eleven actual loaded bars. Native external MEXC OI displayed base BTC quantities and exact observation provenance, then an explicit retry exposed the named disconnected contributor and remedy while prices remained live.

Renderer-load commands use the real iced/Wayland window but **explicitly synthetic fixtures, with no native feeds**:

```sh
./target/release/mexc-chart --stress-small
./target/release/mexc-chart --stress
```

Each runs for 20 seconds, automatically pans every 200 ms, prints timings and exits. **Linux** additionally reports RSS/high-water marks from `/proc/self/status`; other platforms explicitly mark those Linux-only memory metrics unavailable. Small is **1,000 bars / 10 drawings / 3 indicators**; large is **100,000 bars / 1,000 drawings / 24 indicators**. Normal launches have neither fixture data nor timing collection. Timing sample rings are bounded to 4,096 entries.

Observed optimized runs at the same **749×814 logical Wayland size**, with no concurrent build/native chart:

| Metric | Small final | Large before cache fix | Large final |
| --- | ---: | ---: | ---: |
| Input handler CPU p95 (ms) | 0.0343 | 0.2467 | 0.2378 |
| Widget-tree CPU p95 (ms) | 0.1328 | 0.6150 | 0.5916 |
| Mixed canvas build/cache-call p95 (ms) | 0.8617 | 83.0772 | 0.1397 |
| Mixed canvas build/cache-call max (ms) | 3.1338 | 142.2365 | 75.3730 |
| Input→redraw-event p95 (ms) | 0.2807 | 1.8737 | 1.6668 |
| Process RSS / high-water mark (KiB) | 78,348 | 258,940 | 260,080 |

Dense native-price labels exceeded the former 64-entry annotation LRU and reshaped on each pan. The cache now retains an **8,192-shaped-glyph budget**, with oversized layouts uncached and the active annotation pinned separately. Mixed canvas samples include both cache lookups and rebuilds; their different hit/rebuild proportions mean these percentiles are **not a per-frame speedup ratio**. Startup/rebuild costs are included, and the measured maximum remains visible in the table. Input→redraw is event-loop notification, **not GPU presentation latency**. These short fixtures do not establish a memory-leak result, session-memory bound, sustained GPU frame rate, or live-feed performance guarantee.

The integrated workspace passes its behavioral tests, strict Clippy, formatting, and a release build. Behavioral coverage includes numerical seeds/warmup, provisional replacement, corrections/prepends, CVD coverage precedence and origins, independent scales, anchored axis gestures, extreme segment clipping, active-pane keyboard zoom, stale callbacks, repair cursors, modal input isolation, native OI identity/units/timestamp provenance, exclusive history, zero/invalid samples, deduplication/live precedence, gap rendering, and instrument-versus-timeframe transitions. Source coverage includes authoritative inverse/linear base conversion, strict shared CVD origins, as-of skew, OI freshness/gap recovery, transactional source edits, per-source lifetimes, and connected-resync repair without falsely disconnecting. Phase 1 regressions cover direct interval validation against the active catalog after cancelled foreign discovery, unsupported intervals leaving the chart intact, native OI retention, and independent typed CVD resets with unchanged IDs/source membership. Profile regressions cover candle fallback, Decimal conservation/headroom, partial-minute bounds, exact recovery without double counting, zero-volume provenance, bounded contribution caches, paste-safe shortcuts, explicit range hit targets and native-bound endpoint/translation edits. Exact-ID settings search retains precedence over range-selection metadata.

Workspace regressions cover validated DTO round trips, stable IDs, native-data exclusion, atomic checkpoint/backup recovery, private storage and locking, future-schema preservation, authoritative reset replay, stale epoch/revision rejection, close-save failures and retries, and protected read-only workspaces prompting on close only after actual configuration edits.

Profile regressions cover block deduplication/replacement, exact quantity conservation/corrections, deterministic POC/value-area boundaries, genuine reference origins, Single Prints versus tails and mitigation, exact Initial Balance, native-minute visible bounds, custom-session DST/gap/overnight behavior, independent source windows, bounded tail repair, stale publication, millisecond range round trips, and schema-1→2 configuration migration.

Native profile smoke exercised priced history/live feeds alongside CVD for Binance Spot/USDⓈ-M/COIN-M, OKX Spot/linear/inverse swaps, Coinbase spot, and MEXC futures; quantities were checked against canonical native executions. Official historical archives were exercised for Binance spot/linear/inverse and OKX spot. MEXC spot's missing tick metadata and Coinbase International's unavailable native listing/source stayed explicit. Real Wayland workflows exercised default persistence/relaunch, legacy migration with existing indicators/drawings, independent display intervals, mouse/keyboard fixed ranges, native covered Volume Profiles, source retry, native bindings, scrolled/narrow keyboard focus, drawing drags, crosshair alignment and resize capture. No profile frame-rate benchmark or vendor-parity claim is made.

Native workspace checks confirmed that Enter initially cancels, scoped clear removes chart-following/current-native profiles while retaining another instrument's fixed profiles/drawing book, and full reset empties both checkpoint and recovery backup with a newer reset epoch. A native fixed Volume Profile disclosed its exact decimal quantity without floating noise; repeated display-option Apply preserved manually edited millisecond boundaries.

The profile improvements were also exercised in native Wayland: ordinary indicator type selection created TPO/visible VP, Draw created a native-bound fixed VP, body/endpoint drags preserved chart view and binding, Escape canceled a range preview, exact-only toggled persistently, and populated/empty inspection bands retained the same header/plot origin. Compact 520×560 settings kept the estimate toggle and fixed actions accessible.

Native schema-2 → 3 migration and relaunch retained fixed millisecond bounds, native bindings, row grids and exact-only policy. Ctrl+1/2/7/9 changed only the intended flags while numeric fields were focused; they no longer typed digits into row/block settings. The compact editor reserves the same scrollbar gutter as the wide editor. Palette selection, Ctrl+E settings, Ctrl+H visibility and Delete removal were exercised on a persisted fixed profile; unrelated profile IDs remained intact.

Pointer regressions cover scroll-translated inspection/drawing picks and drag commits, clipped-cursor rejection, dialog padding/reselected radios, nested dropdown selection, both directions of batched inside/outside presses, rapid and partially rendered resize requests, and keyboard resize canceling held pointer input without canceling ordinary pointer height updates.

Phase 2 regressions cover editor/details navigation isolation while native callbacks advance the chart, manual bounds preserved through modal closure, explicit Chart-source repair retaining instance ID/reset/visibility and other source ownership, and targeted retries making shared dependent totals incomplete until fresh data arrives while rejecting old-generation samples.

Phase 3 regressions reproduce the previously numeric incomplete All subset: valid Binance+MEXC CVD totals 12 at 8h, switching to 5m blocks it without changing membership, explicit OKX repair totals 15, and returning to 8h fails closed on the retained incompatible choice. Other boundaries cover pending/failed/malformed discovery, closed-editor discovery, superseded/redundant callbacks, retained-input CVD/OI recovery, unchanged refreshes, interval-independent OI eligibility/observations, exact direct uncheck independent of cursor, and atomic Single replacement rejecting stale native choices.

Phase 4 regressions cover height/collapse lifecycle and finite bounds, unchanged numerical scales/inspection/CVD storage under layout changes, live CVD updates while collapsed, moving-bound drag anchoring and stale/window-exit cancellation, wheel/pixel modifier and axis precedence on price and lower plots, stable shared gutters, native tiny-price precision, quantity suffix promotion and nonzero tiny values, distinct deeply zoomed quantity ticks, bounded finite tick generation, UTC calendar/week boundaries, and a narrow quarter-range date label clear of clipped edges. Low-contrast light/dark/monochrome palettes retain readable semantic inks.

Drawing regressions cover exact instrument-book identity, timeframe independence, atomic invalid edits, stable IDs/order and per-book undo/redo, lock enforcement, no-op history, projection/clipping, ray direction, rectangle bounds/corners, Fibonacci direction/results, actual-candle OHLC snapping, native Unicode text bounds/cache behavior, keyboard first-anchor positioning, transactional previews, modal cancellation/stale releases, captured modal-key ownership, and selection across relayout without accidental geometry edits. Desktop transactions leave viewport/scales, market/source generations, indicator outputs, and CVD origins unchanged.

Actual drawing Wayland smoke exercised all seven tools at **749×814 and 520×560 logical pixels**, mouse and keyboard creation, Unicode text and empty-text validation, exact Fibonacci prices/ratios/reverse/extension, endpoint and rectangle-corner resizing, body translation, one-step undo/redo, configured fill opacity and selected color/dash preservation, hidden/locked object management, deletion/restoration, actual OHLC magnet placement, property scrolling and fixed validation/actions, and clean creation-mode guidance. It verified native PEPE_USDC nine-decimal labels, timeframe reprojection, BTC→ETH→BTC book restoration, restart clearing, modal cancellation during a held edit with live prices advancing, low-contrast dark/light palettes, Space-drag over drawings, axis zoom, grip resizing, true continuous trackpad pixel panning, and a drawing-free RSI lower pane sharing time/inspection.

UI-polish release smoke exercised **749×814 and 520×560 logical Wayland windows** with native BTC_USDC and nine-decimal PEPE_USDC prices. It covered mouse/keyboard drawing creation, wide A/B cards versus narrow stacked anchors, exact-price and ratio edits, duplicate-ratio validation without draft loss, hidden/locked object management, multi-instance EMA/RSI creation and exact-instance editing, native symbol search/Apply, explicit source-mode eligibility, PageDown access to source content, empty-manager field visibility, help selection/scrolling/modal shading, and dark/light/monochrome surfaces. Native canvas antialiasing is enabled; no performance benchmark is claimed. Existing numerical, source-generation, history, drawing-transaction, and modal-isolation tests remain passing.

Theme/density Wayland smoke exercised market search, native Apply and nine-decimal price readouts, native-contract and external base-quantity OI, exact contributor coverage/provenance, All-source missing-venue validation, fixed errors through PageDown, and help without losing the source draft. It verified full-width empty editors and intrinsic-width source badges at 749×814 and 520×560, with app-only live reloads between Tokyo Night, Catppuccin Latte, and the active monochrome Omarchy palette. Native OI Details retained cadence, history limitations, and receipt-time provenance without repeating its status/support text.

Direct-service runtime smoke exercised all eight exchange/product catalogs with shared `Arc` identity, recent and exclusive older history on eight liquid feeds across the seven active scopes, price-first enrichment, exactly-once completion, and receiver/watch cancellation. Every live feed delivered native prices and finalized flow buckets: Binance Spot/USDⓈ-M/COIN-M, OKX Spot/Swap, Coinbase Spot, and MEXC Spot/Perpetual. MEXC historical perpetual flow remained explicitly missing; OKX/Coinbase replay progress and cancellation were observed.

Actual Wayland verification used the single-app launcher with no listener on the former local-service port. It exercised normalized symbol search, queued interval selection, successful Apply closing the selector, all eight indicator renderers, Add-to-Edit, multi-field editing, independent CVD resets, hide/show/removal, instance/pane scrolling, pagination and live updates. Modal verification preserved unapplied drafts and both scroll positions, blocked typing/wheel input behind the popup, hid an existing dropdown overlay, and restored field focus without stopping live data.

Scale verification exercised right-axis wheel/drag, synchronized bottom-axis dragging, active-target keyboard zoom, extreme manual zoom without a rendering freeze, and right-axis double-click auto-fit with unchanged CVD origin. Other indicator value scales remained independent while their time ticks stayed synchronized. No performance benchmark or vendor-exact numerical parity is claimed.

OI direct-service smoke exercised Binance USDⓈ-M/COIN-M and OKX native recent/exclusive older pages, live updates on those sources plus MEXC, watch shutdown, and receiver-drop cancellation. It confirmed explicit missing MEXC history, unsupported spot OI, and zero active Coinbase perpetual listings. Actual Wayland smoke exercised manager Add-to-Edit, multiple OI instances, hide/show/removal, native BTC quantities on a 1m Binance chart with 5m OI history, shared-time navigation/pagination, independent keyboard/right-axis value zoom, axis auto-fit, native observations retained through an interval-only switch, explicit unsupported spot panes, and wrapped source labels. No synthetic history or notional approximation was used.

Per-source native runtime smoke proved all six Single/Selected/All CVD+OI instances against actual normalized Binance, OKX, and MEXC producers. Numeric totals were checked against contributing raw deltas/observations; CVD retained Reported MEXC quality and a common session origin. It checked actual Binance COIN-M base CVD against native contract/price bounds and observed native 5m base OI. Binance+OKX historical CVD exposed a real continuity gap without a partial numeric replacement; a stale-required-source transition hid current OI while retaining history.

Actual source-selection Wayland smoke kept MEXC prices while simultaneously plotting foreign Binance, selected Binance+OKX, and All Binance+OKX+MEXC OI, with explicit exact-market choices, base units, native/observed provenance, min/max as-of, independent keyboard value scales, and pane scrolling. It observed explicit unavailable MEXC inverse base conversion before choosing a supported linear market. Source OI history/live observations survived an interval-only switch to MEXC **8h**, including OKX OI despite no native OKX 8h candles.

Source CVD Wayland smoke kept the MEXC price chart while adding Single Binance, Selected Binance+MEXC, and All Binance+MEXC+OKX inverse-swap instances. Selected/All remained explicitly incomplete during startup; All later displayed a numeric BTC total with the first fully captured shared 5m origin, weakest Reported quality, per-source coverage, and actual min/max as-of ranges. No partial total or historical MEXC flow was substituted. A final release smoke used foreign Binance UTC-day CVD on a MEXC 1d chart and observed isolated proven reset points and the live point as markers, without joining separate daily origins.

Phase 1 release smoke exercised mouse and keyboard at a 749×814 logical Wayland window: direct 5m→15m selection, cancelled foreign search followed by an active-market 30m keyboard change, visible Indicators entry, exact-overlay Settings, hide/restore with an unapplied numeric draft, local removal selecting the surviving neighbor, two independent typed CVD resets, active lower-pane keyboard Settings/hide/removal, and native foreign Binance OI with wrapped title controls and retained source/provenance. Return to realtime restored following while retaining manual ±0.8 indicator bounds. The final interval dropdown showed its first 1m choice unobstructed and selecting it kept the MEXC native instrument unchanged.

Phase 2 release smoke exercised 749×814 and 520×560 logical Wayland windows: wide side-by-side settings versus narrow stacked content; fixed actions/validation through result and parameter scrolling; unavailable market selection; numeric validation and an unapplied draft preserved through help; compact overlays/CVD summaries; keyboard status/details and exact-instance source retry. Foreign Binance OI showed actual BTC quantities, venue timestamps, freshness/as-of range and contributor support. Switching the chart to ETH left the BTC source visibly incompatible and nonnumeric; Repair source opened the same instance, incompatible Apply was rejected, and explicit Use Chart retained its ID/visibility while native ETH OI arrived in contracts with observed timestamp provenance. Opening/closing details and attempted background navigation retained the price chart's manual bounds/time geometry while native OI observations continued updating. Truth-critical disclosure text remained readable on the final dark modal surface.

Phase 3 release smoke exercised direct mode and market selection at 749×814 and 520×560 logical Wayland windows: checkbox inclusion/removal independent of cursor, explicit Single radio replacement, two native choices on one venue followed by explicit venue removal, and fixed validation/actions while content scrolls. Actual 8h All CVD accepted explicit Binance BTCUSDC + MEXC BTC_USDC choices; switching to 5m exposed missing required OKX, and Repair opened the same instance with both choices retained. Explicit OKX BTC-USD-SWAP cleared the scope blocker; returning to 8h rejected that retained choice. OKX BTC-USDT-SWAP OI displayed real BTC quantities with native timestamps on the MEXC 8h chart despite no native OKX 8h candles. ETH underlying and perpetual→spot transitions preserved instance IDs/source choices and stayed nonnumeric, including observed pending spot catalog discovery. This smoke did not wait for a complete new shared CVD execution bucket; deterministic regressions proved the repaired numeric totals.

The final refined release was rechecked at both window sizes: Browse buttons and native labels stayed clear of the scrollbar, the border-only keyboard cursor remained readable, the All blocker named the missing venue/underlying/product/chart interval concisely, and direct narrow-window OKX repair cleared that blocker without changing instance #1. Live native prices continued while CVD honestly remained incomplete pending a fully proven common bucket.

Phase 4 release smoke exercised **749×814** and **520×560** logical Wayland windows: price/lower grip dragging, bottom-stack shrinking, keyboard height changes/minimum bounds, double-click/keyboard resets, mouse/keyboard collapse and expansion, and help opened while a resize was held without a stale drag after closure. Native MEXC OI quantities and observed timestamps continued changing while its plot stayed collapsed; Details retained the unabridged contract quantity and provenance. Manual RSI/OI scales survived collapse/expansion, compact OHLC volume exposed its full quantity on hover, and layout survived native interval/instrument switches. Native daily BTC/PEPE charts exercised calendar labels, startup without fabricated ticks, and PEPE's nine-decimal prices. App-only all-equal light/dark palette overrides exercised contrast correction, hollow/filled candles, patterned series and histogram readability without editing desktop configuration.

The final refined release was rechecked at both sizes. The narrow approximately quarter-long daily viewport now retains an interior UTC month label after zooming. Native continuous-axis input exercised vertical pixel time-only panning on price and lower plots, Ctrl pixel shared-time zoom and Shift pixel own-value zoom: lower manual value ticks stayed fixed during time pan/zoom while both plots' timestamps moved together. Wheel/Alt plot-stack and Alt axis precedence, axis double-click auto-fit, and Return to realtime were also exercised. No physical-trackpad hardware test or performance benchmark is claimed.

The final viewport-bounded readiness pass exercised numeric Binance-native CVD in BTC, including its unabridged Details value and Exact origin/as-of while collapsed, then explicitly switched the same instance to native MEXC 1d. The latter retained layout and showed Missing coverage/origin unavailable, valid shared UTC ticks, and no fabricated CVD value ticks. Hiding the final lower pane made price fill the workspace; restoring the same instance recovered its retained plot height.

Coinbase keepalive uses WebSocket control Ping frames, not an unsupported JSON subscription-like ping. Runtime verification observed no repeat of that rejection. Native trade-ID reconciliation can still fail when REST does not provide the heartbeat target; the app revokes unfinished coverage and reconnects rather than claiming an unproven CVD.

Exchange references: [contract metadata](https://www.mexc.com/api-docs/futures/market-endpoints/get-contract-info), [historical candles](https://www.mexc.com/api-docs/futures/market-endpoints/get-candlestick-data), [live candles](https://www.mexc.com/api-docs/futures/websocket-api/k-line-data), [heartbeat](https://www.mexc.com/api-docs/futures/websocket-api/command-details-for-data-exchange).

# mexc-chart

A single-process Rust/iced desktop chart for native **Binance, OKX, Coinbase, and MEXC spot and perpetual markets**, quoted in **USD, USDT, or USDC where actually listed**. Includes linear and inverse perpetuals; quote and settlement currencies are distinct. No substituted venue, dated-futures substitute, currency-converted prices, or resampled bars. Starts at MEXC perpetual BTC_USDC / 5m. The desktop calls native exchange providers directly: no localhost server, local REST/WebSocket API, or serialized inter-process market data. Eight indicators are calculated locally, including historical/live CVD; a ninth plots native historical/live open interest. CVD and OI instances can independently use the chart, another native market, or a strict base-asset aggregate. Price-chart drawings are session-only. No API key, account, trading, database, or saved layouts.

## Run

Requires a current Rust toolchain and a graphical Linux session. Built and exercised on Omarchy/Wayland.

```sh
cargo build --release --workspace
```

Run the single executable from this directory:

```sh
./target/release/mexc-chart
```

Close the window to exit the app and its native feeds/replays. For development, use `cargo run -p mexc-chart`.

On this workstation, `~/Apps/mexc-chart.sh` executes `~/Work/mexc-chart/target/release/mexc-chart`. It uses the existing release build and does not rebuild it or start a companion process.

## Controls

| Action | Keyboard | Mouse |
| --- | --- | --- |
| Show all shortcuts/actions | F1 anywhere; `?` outside editors | Keys [?] in the compact header |
| Pan time | Left / Right; Shift for larger steps | Drag chart horizontally; popup pan-time actions |
| Choose value-scale target | Ctrl+Tab / Ctrl+Shift+Tab cycles price and visible indicator panes | Click a plot/indicator legend or use its scale; active lower legend shows “scale target” |
| Pan target value scale | Up / Down; Shift for larger steps | Drag the selected plot vertically |
| Zoom shared time | `=` / `-` | Wheel over any plot; Ctrl-wheel/pixels; wheel/drag any bottom axis |
| Zoom target value scale | Ctrl+Up / Ctrl+Down; Shift+`=` / Shift+`-` | Shift-wheel over a plot; wheel/drag its right axis |
| Toggle target auto-fit | F | Popup fit action; double-click any right axis to restore auto-fit |
| Return to realtime | L / End | Return to realtime appears beside connection status when time following is stopped; popup live action |
| Reset time and all value scales, auto-fit, and follow live | R / Home | Popup reset action |
| Inspect previous / next candle | `[` / `]` (also comma / period) | Hover chart; popup inspection actions |
| Clear inspection | Escape | Right-click a plotted pane; popup clear-inspection action |
| Load an older page | B | Popup older-page action |
| Retry connection/history/theme now | Y | Popup retry action |
| Open indicator manager | I | Indicators in the compact header; popup indicator-editor action |
| Change native market | M | Click the header symbol; popup market-selector action |
| Change only the native interval | Ctrl+Left / Ctrl+Right outside editors | Header interval dropdown; choices belong to the active exact market, not a cancelled editor draft |
| Settings / hide / remove active lower indicator | Ctrl+E / Ctrl+H / Ctrl+D outside editors; Ctrl+Tab chooses the target | Settings / Hide / Remove on that indicator's title |
| Open status / indicator details | S outside editors; Ctrl+J for active lower pane or selected manager instance | Status summary / Details on the exact indicator |
| Resize active plot | Ctrl+Alt+Up / Ctrl+Alt+Down, in 32-pixel steps | Drag the grip below that plot |
| Restore active plot height | Ctrl+Alt+0 | Double-click its grip |
| Collapse / expand active lower plot | Ctrl+Alt+C | Collapse / Expand on that exact indicator's title |
| Scroll lower panes | PageUp / PageDown | Scrollbar or Alt-wheel over plots; axes retain their own gestures |
| Scroll overlay legends | Shift+PageUp / Shift+PageDown | Legend scrollbar or wheel |

The compact header exposes the native symbol, venue/product/quote, native interval, **Indicators**, **Draw**, and **Keys [?]**, without a large action/navigation bar. Every visible overlay and lower indicator has exact-instance **Details / Settings / Hide / Remove** title controls; the title/body still selects its value-scale target. Compact summaries retain units, inspection/current context, CVD quality/origin, and OI observation provenance/as-of. Contributor identities and verbose source explanations live in Details rather than permanently occupying the chart. Use the manager to restore hidden instances or operate on overlays by keyboard. **F1**, **?** (outside editors), or **Keys [?]** opens the which-key-style popup with chart, drawing, indicator-editor, market-selector, and details bindings, active context first.

Close is selected initially. **Tab / Shift+Tab** selects actions and wraps; **Enter / Space** activates the selected action or closes the popup. **Up / Down / PageUp / PageDown** scrolls the list; **Escape** closes it. The background widget trees stay mounted: pane/legend/instance-list scroll positions, editor/search drafts, and the workspace layout survive the popup. Input and background dropdown overlays are blocked while live data continues. Closing restores editor focus. Chart actions are disabled in editor-context help; editor-switch actions remain available.

Market, indicator, and detail dialogs adapt to the available window, bounded to 720×680 logical pixels. They leave the full-width chart mounted behind a dimmed, input-blocked backdrop; opening or cancelling a dialog does not change the viewport, manual scales or pane/legend scroll state. Native data continues updating. Headers, validation and action footers stay fixed while content scrolls. Indicator settings use side-by-side instances/parameters when the inner width is at least 600 pixels and stack them at narrower widths.

The footer groups price connectivity, candle history, indicator attention/quality/freshness, catalog discovery and theme errors into concise clickable summaries. **S** opens Status details; it distinguishes live upstream prices from realtime viewport following and retains raw progress/errors. **Ctrl+J** opens details for the active lower pane, or for the selected manager instance (including overlays and hidden instances). **Escape** closes details; Up/Down/PageUp/PageDown scroll. In indicator details, **Ctrl+E** opens exact-instance settings, **Ctrl+R** retries its selected native sources, and **Ctrl+Shift+C** explicitly switches only that instance to Chart source. In Status details, **Y** retries connection/history/theme.

Wheel gestures are identical over the price and lower plots: bare/Ctrl-wheel zooms shared time; Shift-wheel zooms only that plot's values. Unmodified trackpad pixels pan shared time using the dominant direction, without panning values; Ctrl-pixels zoom time and Shift-pixels zoom values. Alt-wheel over plots scrolls the pane stack. Right/bottom axes always own value/time gestures, including with Alt. On layouts with an unshifted `+`, that also zooms time in. All timestamps are UTC. Prices retain the selected instrument's native quote precision; quantities retain native base-asset or contract units.

Panning or zooming time stops following live without stopping data updates. **Return to realtime** appears beside the connection status while following is stopped; it and **L / End** resume time following without changing manual value bounds or CVD origins. The status still describes upstream connectivity, not whether the viewport follows live. Manual value movement disables auto-fit only for that plot; **R** resets time and every value scale. Navigating to the left edge automatically fetches older candles with keyboard or mouse; prepending history does not shift the viewport.

The app initially requests one native page, up to 1,000 candles. Limits differ: Binance and MEXC perpetuals return up to 1,000, MEXC spot 500, OKX recent history 300 and older history 100, and Coinbase up to 300. Short nonempty pages retain an older cursor. Older pages and live updates remain in memory for the selected market; switching clears its candle history. There is no disk cache or hard session-memory cap. The time viewport supports 10–10,000 candles. Connection failures remain visible; connections and failed history requests recover automatically. Reconnection backfills missed candles, including multi-page gaps.

### UX rollout

- **Phase 1 — implemented:** direct native interval selection, visible Indicators entry, per-instance title controls, conditional Return to realtime, and typed CVD reset selection.
- **Phase 2 — implemented:** adaptive market/settings dialogs, fixed action footers, compact legends, grouped status/details, and actionable source incompatibility.
- **Phase 3 — implemented:** visible source-mode radios, direct exact-native-market radios/checkboxes, an exchange eligibility/selection checklist, and fail-closed All-source revalidation after context or discovery changes.
- **Phase 4 — implemented:** session-only resizable/collapsible panes, uniform plot gestures, nice value/calendar UTC ticks, compact native quantities with unabridged disclosure, and theme-derived plot contrast.
- **Phase 5 — implemented:** session instrument drawing books, seven price-chart drawing tools, absolute time/price anchors, transactional gesture history, candle magnet, and adaptive object/property controls.

## Drawings

**Draw** in the header or **Ctrl+Shift+D** opens the tool menu and object list. Drawings belong only to the price plot. Books are keyed by exact **exchange / product / native symbol**, not interval: switching instruments leaves the previous book in memory, returning restores it, and interval changes reproject the same anchors onto native bars. Restarting clears every book and its undo/redo history; nothing is saved to disk.

Tool keys **0–7**, while Draw is open, choose **Select, horizontal line, vertical line, trend segment, ray, rectangle/zone, Fibonacci retracement, and text** respectively. Creation tools clear the previous selection. Horizontal, vertical, and text tools need one anchor; the others need two. Click anchors, or use arrows to position a keyboard anchor and **Enter** to confirm it. Keyboard placement starts at the inspected/current real candle; Shift makes larger steps. Text opens a focused, validated text editor. Completing creation returns to Select.

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
| Cycle ink / stroke pattern | Ctrl+Alt+K / Ctrl+Alt+P in properties | Ink / Stroke dropdowns |
| Reverse Fibonacci / toggle right extension | Ctrl+Alt+R / Ctrl+Alt+E in Fibonacci properties | Reverse / Extend right checkboxes |
| Temporarily navigate over drawings or with a creation tool active | Hold Space while dragging | Space-drag pans normally; axes retain their own gestures |

The object list includes hidden, locked, overlapping, and offscreen drawings. Properties accept exact UTC ISO-8601 values or integer milliseconds, prices, ink, line width/pattern, rectangle fill opacity, text/label size, text, and Fibonacci ratios as appropriate. Locking prevents movement and anchor/text/Fibonacci edits; explicit visibility, appearance, unlock, and deletion remain available. Invalid Apply leaves both the committed drawing and editable draft intact; validation and actions remain visible while content scrolls.

Fibonacci defaults are **0%, 23.6%, 38.2%, 50%, 61.8%, 78.6%, 100%**, with **0% at B/end and 100% at A/start**: `price = b + ratio * (a - b)`. Reverse uses `a + ratio * (b - a)`. Ratios are editable, finite, and unique; negative and extension ratios are supported. Right extension is optional. Labels use actual computed native prices.

Drawing commands and transient previews have separate geometry layers/history; they do not restart native sources, recalculate indicators, refit scales, or change CVD origins. Axis/grip ownership, existing wheel/pixel navigation, and lower-pane controls retain precedence. Opening a modal cancels held drawing edits and rejects stale gesture messages while native data continues updating.



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

Start with an empty chart and add only the indicators you want. **I** opens the manager. All instances are session-only; nothing is restored on restart.

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




## Omarchy colors

The desktop reads the active palette once per second and updates chart and interface colors without restarting:

- `$XDG_STATE_HOME/omarchy/current/theme/colors.toml`, defaulting to `~/.local/state/omarchy/current/theme/colors.toml`.
- Older installations: `$XDG_CONFIG_HOME/omarchy/current/theme/colors.toml`, defaulting to `~/.config/omarchy/current/theme/colors.toml`.

Uses semantic background, foreground, red, green, accent/blue, and muted colors; legacy `bg`/`fg` and ANSI `colorN` aliases are accepted. Contrast is corrected once at palette load/reload toward a readable foreground, without changing desktop configuration; grids remain subordinate to series and inspection labels have background backing. Rising candle bodies are hollow and falling bodies filled, so direction remains distinguishable when semantic colors coincide. IDs, labels and line patterns continue to distinguish indicator series. An unreadable or malformed replacement retains the last valid palette and displays an error. If no palette has loaded yet, a built-in neutral palette is used with the error visible.

`MEXC_CHART_THEME=/path/to/colors.toml` overrides palette discovery, useful for testing without changing the desktop theme. The application never edits Omarchy configuration or installs hooks.

## Structure

- `crates/model`: native market, instrument, catalog, candle, flow, and independently timestamped OI contracts. Shared models have no serialization dependency.
- `crates/indicators`: typed configurations, instance lifecycle, and incremental numerical calculations; no iced or networking dependencies.
- `crates/indicators/src/sources.rs`: per-instance source configuration, base-normalized inputs, strict common-origin CVD and native-time OI aggregation, contributor/as-of state, revision/dirty-suffix tracking.
- `crates/data`: in-process `DataService` and four explicit native providers. Eight single-flight catalog slots share `Arc<Catalog>` results; successful discovery is cached for 60 seconds, failures for one second. Pooled clients handle ordinary requests and longer execution-archive downloads. Native subscriptions/replays use bounded channels, visible status, owned cancellation, and shared request pacing.
- `crates/data/src/open_interest.rs`: public native per-instrument OI history/snapshots, capability and timestamp provenance, retention/cursor handling, and cancellation-safe polling.
- `crates/desktop/src/state.rs`: chronological contiguous candles, merge-generation protection, viewport, independent pane scales, indicator instances, inspection, and typed `Action` commands. Ordered flow batches walk candles once and recalculate one affected CVD suffix.
- `crates/desktop/src/chart.rs`: visible-range cached Canvas geometry, pointer-to-action translation, and independent value/shared time transforms. Series and histogram geometry are clipped before rendering; far-offscreen manual scales cannot trigger unbounded dashed-path expansion.
- `crates/desktop/src/main.rs`: command dispatch, request sequencing, keyboard/button adapters, and the small interface.
- `crates/desktop/src/indicator_manager.rs`: keyboard/mouse editor that translates configuration operations into the same typed actions.
- `crates/desktop/src/market_selector.rs`: searchable exchange/product/quote/native-timeframe editor; validated selection uses the same typed actions as keyboard shortcuts.
- `crates/desktop/src/key_help.rs`: borrowed shortcut catalog, context ordering, and state-preserving modal input/overlay isolation.
- `crates/desktop/src/feeds.rs`: owned native producers and ordered Iced batches, capped at 64 events or 16 ms; one shared subscription scope rather than per-event market-string copies. Cancellation/drop stops native I/O and provider children.
- `crates/desktop/src/source_feeds.rs` / `source_controller.rs`: scoped base-quantity producers, independent CVD/OI ownership, shared source demand, discovery, live/history merging, pagination targets, and targeted external-pane invalidation.
- `crates/desktop/src/history.rs`: grouped request, cancellation, progress, retry, and repair state; history pages are shared through `Arc<HistoryPage>`.
- `crates/desktop/src/palette.rs`: runtime Omarchy palette loading.
- `crates/desktop/src/pane_resize.rs`: session-only Canvas grips, anchored height adjustment, cancellation and double-click reset.
- `crates/desktop/src/presentation.rs`: native quantity disclosure, bounded nice-number ticks and UTC calendar ticks.

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
data.history(&market, before).await?;
data.history_with_flow(market, before, history_sender).await?;
data.stream(market, cvd, market_sender, shutdown).await?;
data.open_interest_history(&market, before_ms).await?;
data.open_interest_stream(market, oi_sender, shutdown).await?;
data.history_in_base(&market, before).await?;
data.history_with_flow_in_base(market, before, history_sender).await?;
data.stream_in_base(market, cvd, market_sender, shutdown).await?;
data.open_interest_history_in_base(&market, before_ms).await?;
data.open_interest_stream_in_base(market, oi_sender, shutdown).await?;
```

Catalogs contain only eligible native instruments and supported intervals. History validates the full market identity and returns ascending candles with an exclusive Unix-second `next_before` cursor; `None` means the native request returned no candles. Native validation failures are Rust errors, not localhost HTTP responses.

Optional CVD enrichment sends `HistoryEvent::History { page: Arc<HistoryPage> }` first, followed by native flow/progress and exactly one `Finished` or `Error` terminal event while the consumer is active. Binance embeds native taker flow in its candles and needs no separate replay. Dropping the request/receiver cancels enrichment; dropping the live subscription stops its producer and native children.

OI history returns ascending `OpenInterestSample` observations, support/unit/cadence metadata, and an exclusive `next_before_ms` cursor. The independent stream sends `OpenInterestEvent::Support`, a shared native history page when available, actual samples, and source status. Quantities remain finite/nonnegative; venue and observed receipt timestamps are distinct. Watch shutdown or receiver-drop cancels validation, HTTP reads, queue sends, and timers. Historical errors remain explicit while live observations continue.

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

The integrated workspace passes **217 tests**, strict Clippy, formatting, and a release build. Behavioral coverage includes numerical seeds/warmup, provisional replacement, corrections/prepends, CVD coverage precedence and origins, independent scales, anchored axis gestures, extreme segment clipping, active-pane keyboard zoom, stale callbacks, repair cursors, modal input isolation, native OI identity/units/timestamp provenance, exclusive history, zero/invalid samples, deduplication/live precedence, gap rendering, and instrument-versus-timeframe transitions. Source coverage includes authoritative inverse/linear base conversion, strict shared CVD origins, as-of skew, OI freshness/gap recovery, transactional source edits, per-source lifetimes, and connected-resync repair without falsely disconnecting. Phase 1 regressions cover direct interval validation against the active catalog after cancelled foreign discovery, unsupported intervals leaving the chart intact, native OI retention, and independent typed CVD resets with unchanged IDs/source membership.

Phase 2 regressions cover editor/details navigation isolation while native callbacks advance the chart, manual bounds preserved through modal closure, explicit Chart-source repair retaining instance ID/reset/visibility and other source ownership, and targeted retries making shared dependent totals incomplete until fresh data arrives while rejecting old-generation samples.

Phase 3 regressions reproduce the previously numeric incomplete All subset: valid Binance+MEXC CVD totals 12 at 8h, switching to 5m blocks it without changing membership, explicit OKX repair totals 15, and returning to 8h fails closed on the retained incompatible choice. Other boundaries cover pending/failed/malformed discovery, closed-editor discovery, superseded/redundant callbacks, retained-input CVD/OI recovery, unchanged refreshes, interval-independent OI eligibility/observations, exact direct uncheck independent of cursor, and atomic Single replacement rejecting stale native choices.

Phase 4 regressions cover height/collapse lifecycle and finite bounds, unchanged numerical scales/inspection/CVD storage under layout changes, live CVD updates while collapsed, moving-bound drag anchoring and stale/window-exit cancellation, wheel/pixel modifier and axis precedence on price and lower plots, stable shared gutters, native tiny-price precision, quantity suffix promotion and nonzero tiny values, distinct deeply zoomed quantity ticks, bounded finite tick generation, UTC calendar/week boundaries, and a narrow quarter-range date label clear of clipped edges. Low-contrast light/dark/monochrome palettes retain readable semantic inks.

Drawing regressions cover exact instrument-book identity, timeframe independence, atomic invalid edits, stable IDs/order and per-book undo/redo, lock enforcement, no-op history, projection/clipping, ray direction, rectangle bounds/corners, Fibonacci direction/results, actual-candle OHLC snapping, native Unicode text bounds/cache behavior, keyboard first-anchor positioning, transactional previews, modal cancellation/stale releases, captured modal-key ownership, and selection across relayout without accidental geometry edits. Desktop transactions leave viewport/scales, market/source generations, indicator outputs, and CVD origins unchanged.

Actual drawing Wayland smoke exercised all seven tools at **749×814 and 520×560 logical pixels**, mouse and keyboard creation, Unicode text and empty-text validation, exact Fibonacci prices/ratios/reverse/extension, endpoint and rectangle-corner resizing, body translation, one-step undo/redo, configured fill opacity and selected color/dash preservation, hidden/locked object management, deletion/restoration, actual OHLC magnet placement, property scrolling and fixed validation/actions, and clean creation-mode guidance. It verified native PEPE_USDC nine-decimal labels, timeframe reprojection, BTC→ETH→BTC book restoration, restart clearing, modal cancellation during a held edit with live prices advancing, low-contrast dark/light palettes, Space-drag over drawings, axis zoom, grip resizing, true continuous trackpad pixel panning, and a drawing-free RSI lower pane sharing time/inspection.

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

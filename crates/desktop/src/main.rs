mod chart;
mod drawing_controls;
mod drawing_geometry;
mod drawing_interaction;
mod drawing_layer;
mod drawing_types;
mod drawings;
mod feeds;
mod history;
mod indicator_manager;
mod key_help;
mod market_selector;
mod palette;
mod pane_resize;
mod presentation;
mod source_controller;
mod source_feeds;
mod state;

use arrayvec::ArrayVec;
use drawing_types::{Command as DrawingCommand, DrawingId, Tool as DrawingTool};
use history::{HistoryState, OpenInterestHistoryState, Request, repair_request};
use iced::widget::{
    button, canvas, column, container, mouse_area, pick_list, row, scrollable, text, tooltip,
};
use iced::{Element, Length, Subscription, Task, Theme, keyboard};
use mexc_data::DataService;
use mexc_indicators::{IndicatorSource, IndicatorSpec, SourceData};
use mexc_model::{
    Catalog, Exchange, FlowBar, HistoryEvent, HistoryPage, Market, MarketEvent, OpenInterestEvent,
    OpenInterestPage, OpenInterestSample, Product, Quote, Timeframe,
};
use state::{Action, ChartState};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
enum Message {
    Action(Action),
    DrawingInput {
        generation: u64,
        event: drawing_interaction::Input,
    },
    DrawingBlank {
        generation: u64,
    },
    DrawingCommand {
        generation: u64,
        command: DrawingCommand,
    },
    DrawingControl(drawing_controls::Event),
    DrawingMenu,
    DrawingProperties(DrawingId),
    DrawingCancel,
    DrawingClose,
    Manager(indicator_manager::Event),
    Selector(market_selector::Event),
    SelectInterval(Timeframe),
    IndicatorSettings(u64),
    IndicatorDetails(u64),
    StatusDetails,
    DetailsClose,
    EditorClose,
    ModalBackdrop,
    UseChartSource(u64),
    RetryIndicatorSources(u64),
    Key {
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
        captured: bool,
    },
    HelpOpen,
    HelpClose,
    HelpActivate,
    HelpAction(Action),
    HelpBackdrop,
    MarketData(feeds::Event),
    OpenInterest(feeds::OpenInterestBatch),
    OpenInterestHistory {
        market: Market,
        generation: u64,
        open_interest_generation: u64,
        before_ms: i64,
        result: Result<OpenInterestPage, String>,
    },
    History {
        request: Request,
        market: Market,
        generation: u64,
        history_id: u64,
        epoch: u64,
        result: Result<HistoryPage, String>,
    },
    HistoryFlow {
        request: Request,
        market: Market,
        generation: u64,
        history_id: u64,
        epoch: u64,
        flow_generation: u64,
        events: Result<Box<[HistoryEvent]>, String>,
    },
    Catalog {
        exchange: Exchange,
        product: Product,
        generation: u64,
        result: Result<Arc<Catalog>, String>,
    },
    SourceCatalog {
        exchange: Exchange,
        product: Product,
        generation: u64,
        result: Result<Arc<Catalog>, String>,
    },
    SourceData(source_feeds::Batch),
    Tick,
}

#[derive(Debug, Clone, Copy)]
enum Details {
    Status,
    Indicator(u64),
}

struct Desktop {
    chart: ChartState,
    cache: canvas::Cache,
    pane_caches: BTreeMap<u64, canvas::Cache>,
    drawings: drawings::Store,
    drawing_interaction: drawing_interaction::Interaction,
    drawing_controls: drawing_controls::Controls,
    drawing_scene: drawing_layer::Scene,
    drawing_projection: Cell<Option<drawing_geometry::Projection>>,
    drawing_error: Option<String>,
    manager: indicator_manager::Manager,
    selector: market_selector::Selector,
    active_catalog: Option<Arc<Catalog>>,
    help_open: bool,
    help_selection: usize,
    details: Option<Details>,
    theme_file: palette::ThemeFile,
    data: DataService,
    market_generation: u64,
    flow_generation: u64,
    catalog_generation: u64,
    native_quote: String,
    native_volume_unit: String,
    upstream_connected: bool,
    status: String,
    history: HistoryState,
    open_interest_history: OpenInterestHistoryState,
    flow_status: Option<String>,
    source_catalogs: source_controller::Catalogs,
    source_catalog_pending: source_controller::PendingCatalogs,
    source_catalog_generation: u64,
    source_requests: source_controller::Requests,
    source_inputs: HashMap<Market, SourceData>,
    next_source_generation: u64,
}

impl Desktop {
    fn new() -> (Self, Task<Message>) {
        let data = DataService::new().expect("valid native data configuration");
        let chart = ChartState::default();
        let drawings = drawings::Store::new(&chart.market);
        let mut desktop = Self {
            chart,
            cache: canvas::Cache::new(),
            pane_caches: BTreeMap::new(),
            drawings,
            drawing_interaction: Default::default(),
            drawing_controls: Default::default(),
            drawing_scene: Default::default(),
            drawing_projection: Cell::new(None),
            drawing_error: None,
            manager: indicator_manager::Manager::default(),
            selector: market_selector::Selector::default(),
            active_catalog: None,
            help_open: false,
            help_selection: 0,
            details: None,
            theme_file: palette::ThemeFile::new(),
            data,
            catalog_generation: 0,
            market_generation: 0,
            flow_generation: 0,
            upstream_connected: false,
            status: "Connecting to native exchange…".into(),
            history: HistoryState::default(),
            open_interest_history: OpenInterestHistoryState::default(),
            flow_status: None,
            native_quote: "native quote".into(),
            native_volume_unit: "native volume".into(),
            source_catalogs: Default::default(),
            source_catalog_pending: Default::default(),
            source_catalog_generation: 0,
            source_requests: Default::default(),
            source_inputs: Default::default(),
            next_source_generation: 0,
        };
        let startup = Task::batch([
            desktop.fetch(Request::Recent),
            desktop.load_catalog(desktop.chart.market.exchange, desktop.chart.market.product),
        ]);
        (desktop, startup)
    }

    fn has_cvd(&self) -> bool {
        self.chart.indicators.instances.iter().any(|instance| {
            instance.source.is_chart() && matches!(instance.spec, IndicatorSpec::Cvd { .. })
        })
    }

    fn has_open_interest(&self) -> bool {
        self.chart.indicators.instances.iter().any(|instance| {
            instance.source.is_chart() && matches!(instance.spec, IndicatorSpec::OpenInterest)
        })
    }

    fn fetch_older_open_interest(&mut self) -> Task<Message> {
        if !self.has_open_interest()
            || self.open_interest_history.in_flight
            || self.open_interest_history.retry_ticks != 0
        {
            return Task::none();
        }
        let Some(before_ms) = self.open_interest_history.next_before_ms else {
            return Task::none();
        };
        let Some(target) = self.chart.oldest().map(|time| time.saturating_mul(1000)) else {
            return Task::none();
        };
        if before_ms <= target {
            return Task::none();
        }
        self.open_interest_history.in_flight = true;
        let (cancel, receiver) = tokio::sync::oneshot::channel();
        self.open_interest_history.cancel = Some(cancel);
        let market = self.chart.market.clone();
        let generation = self.market_generation;
        let open_interest_generation = self.open_interest_history.generation;
        let data = self.data.clone();
        let request_market = market.clone();
        Task::perform(
            async move {
                tokio::select! {
                    _ = receiver => Err("OI history request cancelled".into()),
                    result = data.open_interest_history(&request_market, Some(before_ms)) =>
                        result.map_err(|error| format!("{error:#}")),
                }
            },
            move |result| Message::OpenInterestHistory {
                market: market.clone(),
                generation,
                open_interest_generation,
                before_ms,
                result,
            },
        )
    }

    fn clear_open_interest_geometry(&self) {
        for instance in &self.chart.indicators.instances {
            if instance.visible
                && matches!(instance.spec, IndicatorSpec::OpenInterest)
                && let Some(cache) = self.pane_caches.get(&instance.id)
            {
                cache.clear();
            }
        }
    }

    fn flush_open_interest(&mut self, samples: &mut ArrayVec<OpenInterestSample, 64>) {
        if !samples.is_empty() && self.chart.merge_open_interest_samples(samples.as_slice()) {
            self.clear_open_interest_geometry();
        }
        samples.clear();
    }

    fn cancel_flow_history(&mut self) {
        self.history.cancel();
    }

    fn fetch(&mut self, request: Request) -> Task<Message> {
        self.history.in_flight = true;
        self.history.failed = None;
        self.history.error = None;
        self.history.page = None;
        self.history.generation = self.history.generation.wrapping_add(1);
        let history_id = self.history.generation;
        let epoch = self.chart.epoch;
        let market = self.chart.market.clone();
        let generation = self.market_generation;
        if self.has_cvd() {
            self.cancel_flow_history();
            let (cancel, receiver) = tokio::sync::oneshot::channel();
            self.history.cancel = Some(cancel);
            let flow_generation = self.flow_generation;
            Task::run(
                feeds::enriched_history(
                    self.data.clone(),
                    market.clone(),
                    request.before(),
                    receiver,
                ),
                move |events| Message::HistoryFlow {
                    request,
                    market: market.clone(),
                    generation,
                    history_id,
                    epoch,
                    flow_generation,
                    events,
                },
            )
        } else {
            self.cancel_flow_history();
            let data = self.data.clone();
            let source_market = market.clone();
            Task::perform(
                async move {
                    data.history(&source_market, request.before())
                        .await
                        .map_err(|error| format!("{error:#}"))
                },
                move |result| Message::History {
                    request,
                    market: market.clone(),
                    generation,
                    epoch,
                    history_id,
                    result,
                },
            )
        }
    }

    fn load_catalog(&mut self, exchange: Exchange, product: Product) -> Task<Message> {
        self.catalog_generation = self.catalog_generation.wrapping_add(1);
        self.selector.begin_catalog();
        let generation = self.catalog_generation;
        let data = self.data.clone();
        Task::perform(
            async move {
                data.catalog(exchange, product)
                    .await
                    .map_err(|error| format!("{error:#}"))
            },
            move |result| Message::Catalog {
                exchange,
                product,
                generation,
                result,
            },
        )
    }

    fn accepts(&self, market: &Market, generation: u64) -> bool {
        generation == self.market_generation && market == &self.chart.market
    }

    fn restart_connection(&mut self) {
        self.cancel_flow_history();
        self.open_interest_history.reset();
        self.chart.open_interest.connected = false;
        if self.has_open_interest() {
            self.chart.open_interest.message = "Connecting to native open interest…".into();
        }
        self.market_generation += 1;
        self.upstream_connected = false;
        if self.chart.revoke_live_flow() {
            self.clear_flow_geometry();
        }
        self.history.in_flight = false;
        self.history.pending_sync = false;
        self.history.anchor = None;
        self.history.page = None;
        self.history.flow_target = self.has_cvd().then(|| self.chart.oldest()).flatten();
        self.history.failed = None;
        self.history.retry_ticks = 0;
        self.history.error = None;
        self.history.flow_error = None;
        self.flow_status = None;
        self.history.progress = None;
    }

    fn remember_anchor(&mut self) {
        if let Some(time) = self.chart.latest().map(|candle| candle.time) {
            self.history.anchor = Some(self.history.anchor.map_or(time, |anchor| anchor.min(time)));
        }
    }

    fn sync(&mut self) -> Task<Message> {
        self.remember_anchor();
        self.chart.epoch += 1;
        if self.history.in_flight {
            self.history.pending_sync = true;
            Task::none()
        } else {
            self.fetch(Request::Recent)
        }
    }

    fn apply_history_page(&mut self, page: Arc<HistoryPage>, request: Request, epoch: u64) {
        self.history.error = None;
        let previous_oldest = self.chart.oldest();
        let oldest = page.candles.first().map(|candle| candle.time);
        if previous_oldest.is_none()
            || oldest.is_some_and(|time| previous_oldest.is_some_and(|old| time <= old))
        {
            self.chart.next_before = page.next_before;
        } else if matches!(request, Request::Older { .. }) && page.candles.is_empty() {
            self.chart.next_before = None;
        }
        self.chart.merge_page(&page, epoch);
        self.update_source_history_target();
        self.clear_geometry();
        self.history.page = Some(page);
    }

    fn fail_history(&mut self, request: Request, error: String) {
        self.history.in_flight = false;
        self.history.progress = None;
        self.history.error = Some(format!(
            "History: {error} — retrying every 5s; Y / Retry now"
        ));
        self.history.failed = Some(request);
        self.history.retry_ticks = 0;
        self.history.page = None;
        self.cancel_flow_history();
    }

    fn finish_history(&mut self, request: Request) -> Task<Message> {
        self.history.in_flight = false;
        let Some(page) = self.history.page.take() else {
            self.fail_history(
                request,
                "CVD history ended before its native price page".into(),
            );
            return Task::none();
        };
        if self.history.pending_sync {
            self.history.pending_sync = false;
            return self.fetch(Request::Recent);
        }
        let target = match request {
            Request::Recent => self.history.anchor,
            Request::Repair { target, .. } | Request::FlowBackfill { target, .. } => Some(target),
            Request::Older { .. } => None,
        };
        if let Some(next) = repair_request(request, &page, target) {
            return self.fetch(next);
        }
        if matches!(request, Request::FlowBackfill { .. }) {
            self.history.flow_target = None;
        } else if let Some(target) = self.history.flow_target {
            if self.has_cvd()
                && page
                    .candles
                    .first()
                    .is_some_and(|candle| candle.time > target)
                && let Some(before) = page.next_before
            {
                return self.fetch(Request::FlowBackfill { before, target });
            }
            self.history.flow_target = None;
        }
        if !matches!(request, Request::Older { .. }) {
            self.history.anchor = None;
        }
        if self.chart.needs_older() {
            return self.older();
        }
        Task::none()
    }

    fn finish_stale_flow_history(&mut self) -> Task<Message> {
        self.history.in_flight = false;
        self.history.page = None;
        if self.history.pending_sync {
            self.history.pending_sync = false;
            self.fetch(Request::Recent)
        } else {
            Task::none()
        }
    }

    fn older(&mut self) -> Task<Message> {
        if self.history.in_flight || self.history.failed.is_some() {
            return Task::none();
        }
        self.chart
            .next_before
            .map(|before| self.fetch(Request::Older { before }))
            .unwrap_or_else(Task::none)
    }

    fn clear_geometry(&mut self) {
        self.cache.clear();
        for cache in self.pane_caches.values() {
            cache.clear();
        }
    }

    fn clear_flow_geometry(&self) {
        for instance in &self.chart.indicators.instances {
            if instance.visible
                && matches!(instance.spec, IndicatorSpec::Cvd { .. })
                && let Some(cache) = self.pane_caches.get(&instance.id)
            {
                cache.clear();
            }
        }
    }

    fn flush_flows(&mut self, flows: &mut ArrayVec<FlowBar, 64>, live: bool) {
        if !flows.is_empty() && self.chart.merge_flows(flows.drain(..), live) {
            self.clear_flow_geometry();
        }
    }

    fn scale_action(&self, action: Action) -> Action {
        let Some(id) = self.chart.active_pane.filter(|id| {
            self.chart.indicators.instances.iter().any(|instance| {
                instance.id == *id && instance.visible && !instance.spec.is_overlay()
            })
        }) else {
            return action;
        };
        match action {
            Action::Pan { time, price } => Action::PanePan {
                id,
                time,
                value: price,
            },
            Action::Zoom { time, price, x, y } => Action::PaneZoom {
                id,
                time,
                value: price,
                x,
                y,
            },
            Action::AdjustPaneHeight { id: None, delta } => Action::AdjustPaneHeight {
                id: Some(id),
                delta,
            },
            Action::ResetPaneHeight(None) => Action::ResetPaneHeight(Some(id)),
            other => other,
        }
    }

    fn cycle_scale_target(&mut self, direction: i32) {
        let panes = || {
            self.chart
                .indicators
                .instances
                .iter()
                .filter(|instance| instance.visible && !instance.spec.is_overlay())
        };
        let count = panes().count() + 1;
        let current = self
            .chart
            .active_pane
            .and_then(|id| panes().position(|instance| instance.id == id))
            .map_or(0, |index| index + 1);
        let next = (current as i32 + direction).rem_euclid(count as i32) as usize;
        let id = next
            .checked_sub(1)
            .and_then(|index| panes().nth(index).map(|instance| instance.id));
        self.chart.dispatch(Action::ActivatePane(id));
    }

    fn drawing_blocked(&self) -> bool {
        self.help_open
            || self.manager.open
            || self.selector.open
            || self.details.is_some()
            || self.drawing_controls.is_open()
    }

    fn drawing_focus(&self) -> Task<Message> {
        self.drawing_controls
            .focus_id()
            .map_or_else(Task::none, iced::widget::text_input::focus)
    }

    fn commit_drawing(&mut self, command: DrawingCommand) -> bool {
        match self.drawings.apply(command) {
            Ok(_) => {
                self.drawing_error = None;
                true
            }
            Err(error) => {
                if self.drawing_controls.is_open() {
                    self.drawing_controls.set_error(error.clone());
                }
                self.drawing_error = Some(error);
                false
            }
        }
    }

    fn native_intervals(&self) -> &[Timeframe] {
        self.active_catalog
            .as_deref()
            .filter(|catalog| {
                (catalog.exchange, catalog.product)
                    == (self.chart.market.exchange, self.chart.market.product)
            })
            .map_or(&[], |catalog| catalog.intervals.as_slice())
    }

    fn switch_market(&mut self, market: Market, catalog: Arc<Catalog>) -> Task<Message> {
        if (market.exchange, market.product) != (catalog.exchange, catalog.product)
            || !catalog.intervals.contains(&market.interval)
        {
            return Task::none();
        }
        let Some(instrument) = catalog.instruments.iter().find(|item| {
            item.symbol == market.symbol
                && item.exchange == market.exchange
                && item.product == market.product
        }) else {
            return Task::none();
        };
        let price_scale = instrument.price_scale;
        let native_quote = instrument.quote_coin.to_string();
        let native_volume_unit = instrument.volume_unit.clone();
        let base_asset = instrument.base_coin.clone();
        self.active_catalog = Some(catalog);
        self.selector.open = false;
        if market == self.chart.market {
            return Task::none();
        }
        self.native_quote = native_quote;
        self.native_volume_unit = native_volume_unit;
        self.drawing_interaction.cancel();
        self.drawing_controls.close();
        self.drawings.switch_context(&market);
        self.drawing_projection.set(None);
        self.drawing_error = None;
        self.chart.select_market(market, price_scale);
        self.chart.base_asset = base_asset;
        self.restart_connection();
        self.reset_source_discovery();
        self.sync_indicator_sources();
        self.status = "Loading selected native market…".into();
        self.clear_geometry();
        Task::batch([self.fetch(Request::Recent), self.ensure_source_catalogs()])
    }

    fn close_help(&mut self) -> Task<Message> {
        self.help_open = false;
        if self.manager.open {
            self.manager.show()
        } else if self.selector.open {
            iced::widget::text_input::focus("market-search")
        } else if self.drawing_controls.is_open() {
            self.drawing_focus()
        } else {
            Task::none()
        }
    }
    fn help_context(&self) -> key_help::Context {
        if self.details.is_some() {
            key_help::Context::Details
        } else if self.manager.open {
            key_help::Context::Indicators
        } else if self.selector.open {
            key_help::Context::Market
        } else if self.drawing_controls.is_open() {
            key_help::Context::Drawings
        } else {
            key_help::Context::Chart
        }
    }

    fn activate_help_selection(&mut self) -> Task<Message> {
        if !self.help_open {
            return Task::none();
        }
        if self.help_selection == 0 {
            return self.close_help();
        }
        let Some(action) = key_help::action_at(self.help_context(), self.help_selection - 1) else {
            return Task::none();
        };
        self.update(Message::HelpAction(action))
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        if self.drawing_controls.is_open()
            && matches!(
                &message,
                Message::IndicatorSettings(_)
                    | Message::IndicatorDetails(_)
                    | Message::StatusDetails
                    | Message::SelectInterval(_)
                    | Message::UseChartSource(_)
                    | Message::RetryIndicatorSources(_)
            )
        {
            return Task::none();
        }
        match message {
            Message::DrawingCancel => {
                if !self.drawing_blocked() {
                    self.drawing_interaction.cancel();
                    self.drawings.select(None);
                }
            }
            Message::DrawingMenu => {
                if self.drawing_blocked() {
                    return Task::none();
                }
                self.drawing_interaction.cancel();
                self.drawing_controls.open_menu(&self.drawings);
                return scrollable::scroll_to(
                    scrollable::Id::new(drawing_controls::BODY_ID),
                    scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                );
            }
            Message::DrawingProperties(id) => {
                if self.drawing_blocked() || self.drawings.find(id).is_none() {
                    return Task::none();
                }
                self.drawing_interaction.cancel();
                self.drawings.select(Some(id));
                if self.drawing_controls.open_properties(&self.drawings, id) {
                    return Task::batch([
                        self.drawing_focus(),
                        scrollable::scroll_to(
                            scrollable::Id::new(drawing_controls::BODY_ID),
                            scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                        ),
                    ]);
                }
            }
            Message::DrawingClose => {
                if !self.help_open {
                    self.drawing_controls.close();
                }
            }
            Message::DrawingCommand {
                generation,
                command,
            } => {
                if self.drawing_blocked() || generation != self.drawings.generation() {
                    return Task::none();
                }
                self.drawing_interaction.cancel();
                self.commit_drawing(command);
            }
            Message::DrawingBlank { generation } => {
                if !self.drawing_blocked() && generation == self.drawing_interaction.generation() {
                    self.drawings.select(None);
                    self.chart.dispatch(Action::ActivatePane(None));
                }
            }
            Message::DrawingInput { generation, event } => {
                if self.drawing_blocked() || generation != self.drawing_interaction.generation() {
                    return Task::none();
                }
                let inspection = match &event {
                    drawing_interaction::Input::Move {
                        point, projection, ..
                    } if self.drawing_interaction.tool() == DrawingTool::Select
                        && !self.drawing_interaction.is_dragging()
                        && projection.contains(*point) =>
                    {
                        projection.anchor(*point)
                    }
                    _ => None,
                };
                if matches!(
                    &event,
                    drawing_interaction::Input::Down { .. }
                        | drawing_interaction::Input::Key { .. }
                ) {
                    self.chart.dispatch(Action::ActivatePane(None));
                    self.drawing_error = None;
                }
                let effect = self.drawing_interaction.update(event, &self.drawings);
                match effect {
                    drawing_interaction::Effect::None => {}
                    drawing_interaction::Effect::Select(id) => {
                        self.drawings.select(id);
                    }
                    drawing_interaction::Effect::Commit(command) => {
                        self.commit_drawing(command);
                    }
                    drawing_interaction::Effect::Text(anchor) => {
                        self.drawing_interaction.cancel();
                        self.drawing_controls.open_text(&self.drawings, anchor);
                        return Task::batch([
                            self.drawing_focus(),
                            scrollable::scroll_to(
                                scrollable::Id::new(drawing_controls::BODY_ID),
                                scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                            ),
                        ]);
                    }
                }
                if let Some(anchor) = inspection {
                    self.chart.dispatch(Action::InspectPane {
                        id: None,
                        point: Some((anchor.utc_ms as f64 / 1000.0, anchor.price)),
                    });
                }
            }
            Message::DrawingControl(event) => {
                if self.help_open
                    || self.manager.open
                    || self.selector.open
                    || self.details.is_some()
                    || !self.drawing_controls.is_open()
                    || self.drawing_controls.context_generation() != self.drawings.generation()
                {
                    return Task::none();
                }
                let editing = self.drawing_controls.is_editing();
                let effect = self.drawing_controls.update(event, &self.drawings);
                match effect {
                    drawing_controls::Effect::None => {}
                    drawing_controls::Effect::ChooseTool(tool) => {
                        if tool != DrawingTool::Select && self.chart.latest().is_none() {
                            self.drawing_controls
                                .set_error("Waiting for native price data".into());
                            return Task::none();
                        }
                        self.drawing_controls.close();
                        self.drawing_interaction.choose_tool(tool);
                        if tool != DrawingTool::Select {
                            self.drawings.select(None);
                        }
                        self.chart.dispatch(Action::ActivatePane(None));
                        self.drawing_error = None;
                    }
                    drawing_controls::Effect::Select(id) => {
                        self.drawings.select(id);
                        self.chart.dispatch(Action::ActivatePane(None));
                        if !editing && self.drawing_controls.is_editing() {
                            return Task::batch([
                                self.drawing_focus(),
                                scrollable::scroll_to(
                                    scrollable::Id::new(drawing_controls::BODY_ID),
                                    scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                                ),
                            ]);
                        }
                    }
                    drawing_controls::Effect::Magnet(enabled) => {
                        self.drawing_interaction.set_magnet(enabled)
                    }
                    drawing_controls::Effect::ToggleMagnet => {
                        self.drawing_interaction
                            .set_magnet(!self.drawing_interaction.magnet());
                    }
                    drawing_controls::Effect::Commit(command) => {
                        self.drawing_interaction.cancel();
                        if self.commit_drawing(command) && editing {
                            self.drawing_controls.close();
                        }
                    }
                    drawing_controls::Effect::Close => self.drawing_controls.close(),
                }
                if editing && self.drawing_controls.is_open() && !self.drawing_controls.is_editing()
                {
                    return scrollable::scroll_to(
                        scrollable::Id::new(drawing_controls::BODY_ID),
                        scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                    );
                }
            }
            Message::Key {
                key,
                modifiers,
                captured,
            } => {
                use keyboard::{Key, key::Named};
                if matches!(key.as_ref(), Key::Named(Named::F1))
                    || (!self.selector.open
                        && !self.manager.open
                        && !captured
                        && matches!(key.as_ref(), Key::Character("?")))
                {
                    if !self.help_open {
                        self.drawing_interaction.cancel();
                        self.help_open = true;
                        self.help_selection = 0;
                    }
                    return Task::none();
                }
                if self.help_open {
                    if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                        return self.update(Message::HelpClose);
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        let count = key_help::action_count(self.help_context()) + 1;
                        self.help_selection = if modifiers.shift() {
                            (self.help_selection + count - 1) % count
                        } else {
                            (self.help_selection + 1) % count
                        };
                        let offset = if self.help_selection == 0 {
                            0.0
                        } else {
                            key_help::action_scroll_offset(
                                self.help_context(),
                                self.help_selection - 1,
                            )
                            .unwrap_or(0.0)
                        };
                        return scrollable::scroll_to(
                            scrollable::Id::new(key_help::LIST_SCROLL_ID),
                            scrollable::AbsoluteOffset { x: 0.0, y: offset },
                        );
                    }
                    if matches!(
                        key.as_ref(),
                        Key::Named(Named::Enter | Named::Space) | Key::Character(" ")
                    ) {
                        return self.update(Message::HelpActivate);
                    }
                    let scroll = match key.as_ref() {
                        Key::Named(Named::PageUp) => Some(-450.0),
                        Key::Named(Named::PageDown) => Some(450.0),
                        Key::Named(Named::ArrowUp) => Some(-100.0),
                        Key::Named(Named::ArrowDown) => Some(100.0),
                        _ => None,
                    };
                    if let Some(amount) = scroll {
                        return scrollable::scroll_by(
                            scrollable::Id::new(key_help::LIST_SCROLL_ID),
                            scrollable::AbsoluteOffset { x: 0.0, y: amount },
                        );
                    }
                    return Task::none();
                } else if self.details.is_some() {
                    if let Some(Details::Indicator(id)) = self.details {
                        if modifiers.control() {
                            let action = match key.as_ref() {
                                Key::Character("e" | "E") => Some(Message::IndicatorSettings(id)),
                                Key::Character("r" | "R") => {
                                    Some(Message::RetryIndicatorSources(id))
                                }
                                Key::Character("c" | "C") if modifiers.shift() => {
                                    Some(Message::UseChartSource(id))
                                }
                                _ => None,
                            };
                            if let Some(action) = action {
                                return self.update(action);
                            }
                        }
                    } else if matches!(key.as_ref(), Key::Character("y" | "Y")) {
                        return self.update(Message::HelpAction(Action::Retry));
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                        return self.update(Message::DetailsClose);
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        return if modifiers.shift() {
                            iced::widget::focus_previous()
                        } else {
                            iced::widget::focus_next()
                        };
                    }
                    let amount = match key.as_ref() {
                        Key::Named(Named::PageUp) => Some(-350.0),
                        Key::Named(Named::PageDown) => Some(350.0),
                        Key::Named(Named::ArrowUp) => Some(-80.0),
                        Key::Named(Named::ArrowDown) => Some(80.0),
                        _ => None,
                    };
                    if let Some(amount) = amount {
                        return scrollable::scroll_by(
                            scrollable::Id::new("details-body"),
                            scrollable::AbsoluteOffset { x: 0.0, y: amount },
                        );
                    }
                } else if self.selector.open {
                    if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                        return self.update(Message::Action(Action::ManageMarket));
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        return if modifiers.shift() {
                            iced::widget::focus_previous()
                        } else {
                            iced::widget::focus_next()
                        };
                    }
                    if let Some(event) = self.selector.key(key.as_ref(), modifiers) {
                        return self.update(Message::Selector(event));
                    }
                    if !captured && matches!(key.as_ref(), Key::Named(Named::Enter)) {
                        return self.update(Message::Selector(market_selector::Event::Submit));
                    }
                } else if self.manager.open {
                    if modifiers.control() && matches!(key.as_ref(), Key::Character("j" | "J")) {
                        if let Some(id) = self.manager.selected() {
                            return self.update(Message::IndicatorDetails(id));
                        }
                        return Task::none();
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Escape))
                        || (!captured && matches!(key.as_ref(), Key::Character("i" | "I")))
                    {
                        return self.update(Message::Action(Action::ManageIndicators));
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        return if modifiers.shift() {
                            iced::widget::focus_previous()
                        } else {
                            iced::widget::focus_next()
                        };
                    }
                    if let Some(event) =
                        self.manager
                            .key(key.as_ref(), modifiers, captured, &self.chart)
                    {
                        return self.update(Message::Manager(event));
                    }
                } else if self.drawing_controls.is_open() {
                    if matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        return if modifiers.shift() {
                            iced::widget::focus_previous()
                        } else {
                            iced::widget::focus_next()
                        };
                    }
                    if matches!(key.as_ref(), Key::Named(Named::PageUp | Named::PageDown)) {
                        let y = if matches!(key.as_ref(), Key::Named(Named::PageUp)) {
                            -350.0
                        } else {
                            350.0
                        };
                        return scrollable::scroll_by(
                            scrollable::Id::new(drawing_controls::BODY_ID),
                            scrollable::AbsoluteOffset { x: 0.0, y },
                        );
                    }
                    if let Some(event) =
                        self.drawing_controls
                            .key(key.as_ref(), modifiers, &self.drawings)
                    {
                        return self.update(Message::DrawingControl(event));
                    }
                } else if !captured {
                    if modifiers.control()
                        && modifiers.shift()
                        && matches!(key.as_ref(), Key::Character("d" | "D"))
                    {
                        return self.update(Message::DrawingMenu);
                    }
                    if modifiers.control()
                        && !modifiers.alt()
                        && matches!(key.as_ref(), Key::Character("z" | "Z"))
                    {
                        self.drawing_interaction.cancel();
                        self.commit_drawing(if modifiers.shift() {
                            DrawingCommand::Redo
                        } else {
                            DrawingCommand::Undo
                        });
                        return Task::none();
                    }
                    if self.chart.active_pane.is_none() {
                        if matches!(key.as_ref(), Key::Named(Named::Escape))
                            && (self.drawing_interaction.cancel() || self.drawings.select(None))
                        {
                            return Task::none();
                        }
                        if let Some(id) = self.drawings.selected() {
                            if matches!(key.as_ref(), Key::Named(Named::Delete)) {
                                self.drawing_interaction.cancel();
                                self.commit_drawing(DrawingCommand::Delete(id));
                                return Task::none();
                            }
                            if modifiers.control()
                                && matches!(key.as_ref(), Key::Character("e" | "E"))
                            {
                                return self.update(Message::DrawingProperties(id));
                            }
                        }
                    }
                    if modifiers.control()
                        && modifiers.alt()
                        && matches!(key.as_ref(), Key::Character("c" | "C"))
                    {
                        return self.chart.active_pane.map_or_else(Task::none, |id| {
                            self.update(Message::Action(Action::TogglePaneCollapsed(id)))
                        });
                    }
                    if !modifiers.control()
                        && !modifiers.alt()
                        && matches!(key.as_ref(), Key::Character("s" | "S"))
                    {
                        return self.update(Message::StatusDetails);
                    }
                    if modifiers.control()
                        && matches!(
                            key.as_ref(),
                            Key::Named(Named::ArrowLeft | Named::ArrowRight)
                        )
                    {
                        let intervals = self.native_intervals();
                        if let Some(index) = intervals
                            .iter()
                            .position(|interval| *interval == self.chart.market.interval)
                        {
                            let next = if matches!(key.as_ref(), Key::Named(Named::ArrowLeft)) {
                                (index + intervals.len() - 1) % intervals.len()
                            } else {
                                (index + 1) % intervals.len()
                            };
                            return self.update(Message::SelectInterval(intervals[next]));
                        }
                        return Task::none();
                    }
                    if modifiers.control()
                        && let Some(id) = self.chart.active_pane
                    {
                        let local = match key.as_ref() {
                            Key::Character("e" | "E") => Some(Message::IndicatorSettings(id)),
                            Key::Character("j" | "J") => Some(Message::IndicatorDetails(id)),
                            Key::Character("h" | "H") => {
                                Some(Message::Action(Action::ToggleIndicator(id)))
                            }
                            Key::Character("d" | "D") => {
                                Some(Message::Action(Action::RemoveIndicator(id)))
                            }
                            _ => None,
                        };
                        if let Some(message) = local {
                            return self.update(message);
                        }
                    }
                    if modifiers.control() && matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        self.cycle_scale_target(if modifiers.shift() { -1 } else { 1 });
                    } else if let Some(action) = key_action(key.as_ref(), modifiers) {
                        return self.update(Message::Action(self.scale_action(action)));
                    }
                }
            }
            Message::HelpOpen => {
                self.drawing_interaction.cancel();
                self.help_open = true;
                self.help_selection = 0;
            }
            Message::HelpClose => {
                return self.close_help();
            }
            Message::HelpActivate => {
                return self.activate_help_selection();
            }
            Message::HelpBackdrop => {}
            Message::HelpAction(action) => {
                if matches!(&action, Action::ManageIndicators | Action::ManageMarket) {
                    self.help_open = false;
                    return self.update(Message::Action(action));
                }
                let focus = self.close_help();
                self.details = None;
                let task = self.update(Message::Action(self.scale_action(action)));
                return Task::batch([task, focus]);
            }
            Message::SelectInterval(interval) => {
                if self.help_open
                    || self.manager.open
                    || self.selector.open
                    || self.details.is_some()
                {
                    return Task::none();
                }
                let Some(catalog) = self.active_catalog.clone() else {
                    return Task::none();
                };
                let mut market = self.chart.market.clone();
                market.interval = interval;
                return self.switch_market(market, catalog);
            }
            Message::IndicatorSettings(id) => {
                if self.help_open
                    || !self
                        .chart
                        .indicators
                        .instances
                        .iter()
                        .any(|item| item.id == id)
                {
                    return Task::none();
                }
                self.drawing_interaction.cancel();
                self.selector.open = false;
                self.details = None;
                self.chart.indicator_error = None;
                let focus = self.manager.edit(id, &self.chart);
                return Task::batch([focus, self.ensure_source_catalogs()]);
            }
            Message::IndicatorDetails(id) => {
                if !self.help_open
                    && !self.selector.open
                    && self
                        .chart
                        .indicators
                        .instances
                        .iter()
                        .any(|item| item.id == id)
                {
                    self.drawing_interaction.cancel();
                    self.manager.open = false;
                    self.details = Some(Details::Indicator(id));
                }
            }
            Message::StatusDetails => {
                if !self.help_open && !self.manager.open && !self.selector.open {
                    self.drawing_interaction.cancel();
                    self.details = Some(Details::Status);
                }
            }
            Message::DetailsClose => {
                if !self.help_open {
                    self.details = None;
                }
            }
            Message::EditorClose => {
                if !self.help_open {
                    self.manager.open = false;
                    self.selector.open = false;
                }
            }
            Message::ModalBackdrop => {}
            Message::UseChartSource(id) => {
                if self.help_open || self.manager.open || self.selector.open {
                    return Task::none();
                }
                let Some(instance) = self
                    .chart
                    .indicators
                    .instances
                    .iter()
                    .find(|item| item.id == id)
                else {
                    return Task::none();
                };
                if instance.source.is_chart() {
                    return Task::none();
                }
                let spec = instance.spec;
                self.details = None;
                return self.update(Message::Action(Action::ApplyIndicator {
                    id: Some(id),
                    spec,
                    source: IndicatorSource::default(),
                }));
            }
            Message::RetryIndicatorSources(id) => {
                if self.help_open || self.manager.open || self.selector.open {
                    return Task::none();
                }
                let Some(instance) = self
                    .chart
                    .indicators
                    .instances
                    .iter()
                    .find(|item| item.id == id)
                else {
                    return Task::none();
                };
                let kind = match instance.spec {
                    IndicatorSpec::Cvd { .. } if !instance.source.is_chart() => {
                        source_feeds::Kind::Cvd
                    }
                    IndicatorSpec::OpenInterest if !instance.source.is_chart() => {
                        source_feeds::Kind::OpenInterest
                    }
                    _ => return Task::none(),
                };
                for market in &instance.source.markets {
                    // Stored source intervals are rebound to the active native context.
                    let key = self
                        .source_requests
                        .iter_mut()
                        .find(|((wanted, wanted_kind), _)| {
                            *wanted_kind == kind
                                && wanted.exchange == market.exchange
                                && wanted.product == market.product
                                && wanted.symbol == market.symbol
                        });
                    if let Some(((market, _), request)) = key {
                        self.next_source_generation = self.next_source_generation.wrapping_add(1);
                        request.generation = self.next_source_generation;
                        if let Some(input) = self.source_inputs.get_mut(market) {
                            match kind {
                                source_feeds::Kind::Cvd => {
                                    input.flow_connected = false;
                                    input.revoke_unfinished(source_controller::now_ms());
                                }
                                source_feeds::Kind::OpenInterest => input.oi_connected = false,
                            }
                            input.revision = input.revision.wrapping_add(1);
                        }
                    }
                }
                self.refresh_source_panes();
                let focus = self.update(Message::IndicatorSettings(id));
                return Task::batch([
                    focus,
                    self.update(Message::Manager(indicator_manager::Event::SourceRetry)),
                ]);
            }
            Message::Manager(event) => {
                if matches!(&event, indicator_manager::Event::SourceRetry) {
                    self.retry_source_discovery();
                    self.revalidate_all_sources();
                    self.refresh_source_panes();
                }
                if matches!(
                    &event,
                    indicator_manager::Event::Kind(_)
                        | indicator_manager::Event::Field(..)
                        | indicator_manager::Event::Reset(_)
                        | indicator_manager::Event::Select(_)
                        | indicator_manager::Event::New
                ) {
                    self.chart.indicator_error = None;
                }
                let (action, focus) = self.manager.update(event, &self.chart);
                if let Some(action) = action {
                    return Task::batch([self.update(Message::Action(action)), focus]);
                }
                return Task::batch([focus, self.ensure_source_catalogs()]);
            }
            Message::Selector(event) => {
                if matches!(&event, market_selector::Event::Retry) {
                    let (exchange, product) = self.selector.scope();
                    return self.load_catalog(exchange, product);
                }
                let prior_scope = self.selector.scope();
                let submitting = matches!(&event, market_selector::Event::Submit);
                if let Some(action) = self.selector.update(event) {
                    return self.update(Message::Action(action));
                }
                let scope = self.selector.scope();
                if scope != prior_scope {
                    return Task::batch([
                        self.selector.follow_selection(),
                        self.load_catalog(scope.0, scope.1),
                    ]);
                }
                if submitting {
                    return self.selector.focus_search();
                }
                return self.selector.follow_selection();
            }
            Message::SourceCatalog {
                exchange,
                product,
                generation,
                result,
            } => {
                if generation != self.source_catalog_generation
                    || product != self.chart.market.product
                    || !self.source_catalog_pending.remove(&(exchange, product))
                {
                    return Task::none();
                }
                let result = source_controller::validated_source_catalog(exchange, product, result);
                self.source_catalogs
                    .insert((exchange, product), result.clone());
                self.manager
                    .accept_source_catalog(exchange, product, result);
                self.sync_indicator_sources();
            }
            Message::SourceData(batch) => {
                self.accept_source_batch(batch);
                return Task::none();
            }
            Message::Catalog {
                exchange,
                product,
                generation,
                result,
            } => {
                // Only authoritative current discovery can change the All exchange universe.
                if generation != self.catalog_generation
                    || (exchange, product) != self.selector.scope()
                {
                    return Task::none();
                }
                let result = source_controller::validated_source_catalog(exchange, product, result);
                self.source_catalog_pending.remove(&(exchange, product));
                self.source_catalogs
                    .insert((exchange, product), result.clone());
                self.manager
                    .accept_source_catalog(exchange, product, result.clone());
                self.selector.set_catalog(exchange, product, result);
                if (exchange, product) == (self.chart.market.exchange, self.chart.market.product)
                    && let Some(instrument) =
                        self.selector.active_instrument(&self.chart.market.symbol)
                {
                    self.active_catalog = self.selector.catalog().cloned();
                    self.chart.price_scale = instrument.price_scale;
                    self.native_quote = instrument.quote_coin.to_string();
                    self.native_volume_unit = instrument.volume_unit.clone();
                    self.chart.base_asset = instrument.base_coin.clone();
                    self.clear_geometry();
                }
                self.sync_indicator_sources();
                return Task::batch([
                    if self.selector.open {
                        self.selector.follow_selection()
                    } else {
                        Task::none()
                    },
                    self.ensure_source_catalogs(),
                ]);
            }
            Message::Tick => {
                if self.theme_file.reload() {
                    self.clear_geometry();
                }
                self.open_interest_history.retry_ticks =
                    self.open_interest_history.retry_ticks.saturating_sub(1);
                self.refresh_source_panes();
                if self.history.failed.is_some() && !self.history.in_flight {
                    self.history.retry_ticks += 1;
                    if self.history.retry_ticks >= 5 {
                        self.history.retry_ticks = 0;
                        if self.history.pending_sync {
                            self.history.pending_sync = false;
                            return self.fetch(Request::Recent);
                        }
                        return self.fetch(self.history.failed.unwrap());
                    }
                }
            }
            Message::Action(action) => {
                if self.help_open {
                    return Task::none();
                }
                // Widget input is blocked behind dialogs; also reject queued canvas actions.
                if (self.manager.open
                    || self.selector.open
                    || self.details.is_some()
                    || self.drawing_controls.is_open())
                    && !matches!(&action, Action::ManageIndicators | Action::ManageMarket)
                    && !(self.selector.open && matches!(&action, Action::SelectMarket(_)))
                    && !(self.manager.open
                        && matches!(
                            &action,
                            Action::AddIndicator(_)
                                | Action::ConfigureIndicator { .. }
                                | Action::RemoveIndicator(_)
                                | Action::ApplyIndicator { .. }
                                | Action::ToggleIndicator(_)
                        ))
                {
                    return Task::none();
                }
                if matches!(&action, Action::ManageIndicators | Action::ManageMarket) {
                    self.drawing_interaction.cancel();
                    self.drawing_controls.close();
                } else if matches!(&action, Action::ActivatePane(Some(_))) {
                    self.drawing_interaction.cancel();
                }
                match action {
                    Action::ManageIndicators => {
                        self.details = None;
                        if self.manager.open {
                            self.manager.open = false;
                        } else {
                            self.selector.open = false;
                            let focus = self.manager.show();
                            return Task::batch([focus, self.ensure_source_catalogs()]);
                        }
                    }
                    Action::ManageMarket => {
                        self.details = None;
                        if self.selector.open {
                            self.selector.open = false;
                        } else {
                            self.manager.open = false;
                            let focus = self.selector.show(
                                &self.chart.market,
                                Quote::parse(&self.native_quote).unwrap_or_default(),
                            );
                            let (exchange, product) = self.selector.scope();
                            let catalog = if self.selector.catalog_error.is_some()
                                || self
                                    .selector
                                    .active_instrument(&self.chart.market.symbol)
                                    .is_none()
                            {
                                self.load_catalog(exchange, product)
                            } else {
                                Task::none()
                            };
                            return Task::batch([focus, catalog]);
                        }
                    }
                    Action::SelectMarket(market) => {
                        if self.selector.instrument(&market.symbol).is_none()
                            || (market.exchange, market.product) != self.selector.scope()
                            || !self.selector.intervals().contains(&market.interval)
                        {
                            self.selector.reject("Choose an available native market and interval from the selected scope");
                            return self.selector.focus_search();
                        }
                        let Some(catalog) = self.selector.catalog().cloned() else {
                            return Task::none();
                        };
                        return self.switch_market(market, catalog);
                    }
                    Action::ScrollPanes { amount, legends } => {
                        return scrollable::scroll_by(
                            scrollable::Id::new(if legends {
                                "indicator-legends"
                            } else {
                                "indicator-panes"
                            }),
                            scrollable::AbsoluteOffset { x: 0.0, y: amount },
                        );
                    }
                    Action::Retry => {
                        self.theme_file.reload();
                        self.clear_geometry();
                        // One generation invalidates old native subscriptions and history tasks.
                        let repair_anchor = self.history.anchor;
                        self.restart_connection();
                        let now = source_controller::now_ms();
                        for source in self.source_inputs.values_mut() {
                            source.flow_connected = false;
                            source.oi_connected = false;
                            source.revoke_unfinished(now);
                            source.revision = source.revision.wrapping_add(1);
                        }
                        for request in self.source_requests.values_mut() {
                            self.next_source_generation =
                                self.next_source_generation.wrapping_add(1);
                            request.generation = self.next_source_generation;
                        }
                        self.history.anchor = repair_anchor;
                        self.status = "Reconnecting to native market data…".into();
                        return Task::batch([
                            self.sync(),
                            self.load_catalog(
                                self.chart.market.exchange,
                                self.chart.market.product,
                            ),
                        ]);
                    }
                    Action::Older => return self.older(),
                    _ => {
                        let changes_instances = matches!(
                            &action,
                            Action::AddIndicator(_)
                                | Action::ConfigureIndicator { .. }
                                | Action::RemoveIndicator(_)
                                | Action::ApplyIndicator { .. }
                        );
                        let applies_parameters = matches!(
                            &action,
                            Action::AddIndicator(_)
                                | Action::ConfigureIndicator { .. }
                                | Action::ApplyIndicator { .. }
                        );
                        let pane_only = match &action {
                            Action::PanePan { id, time, .. } if *time == 0.0 => Some(*id),
                            Action::PaneZoom { id, time, .. } if *time == 1.0 => Some(*id),
                            Action::FitPane(id) => Some(*id),
                            Action::AutoFit => self.chart.active_pane,
                            _ => None,
                        };
                        let had_cvd = self.has_cvd();
                        let had_open_interest = self.has_open_interest();
                        let removal_focus = match &action {
                            Action::RemoveIndicator(id) => {
                                self.manager.prepare_removal(*id, &self.chart)
                            }
                            _ => Task::none(),
                        };
                        if self.chart.dispatch(action) {
                            if let Some(id) = pane_only {
                                if let Some(cache) = self.pane_caches.get(&id) {
                                    cache.clear();
                                }
                            } else {
                                self.clear_geometry();
                            }
                        }
                        let focus = Task::batch([
                            removal_focus,
                            if applies_parameters && self.chart.indicator_error.is_none() {
                                self.manager.applied(&self.chart)
                            } else {
                                if applies_parameters
                                    && let Some(error) = &self.chart.indicator_error
                                {
                                    self.manager.reject_source(error.clone());
                                }
                                Task::none()
                            },
                        ]);
                        if changes_instances {
                            self.pane_caches.retain(|id, _| {
                                self.chart
                                    .indicators
                                    .instances
                                    .iter()
                                    .any(|item| item.id == *id && !item.spec.is_overlay())
                            });
                            for instance in &self.chart.indicators.instances {
                                if !instance.spec.is_overlay() {
                                    self.pane_caches.entry(instance.id).or_default();
                                }
                            }
                            self.sync_indicator_sources();
                        }
                        let focus = Task::batch([
                            focus,
                            if changes_instances {
                                self.ensure_source_catalogs()
                            } else {
                                Task::none()
                            },
                        ]);
                        if had_open_interest != self.has_open_interest() {
                            self.open_interest_history.reset();
                            self.chart.open_interest.connected = false;
                            self.chart.open_interest.message =
                                "Loading native open interest…".into();
                            if !self.has_open_interest() {
                                self.chart.open_interest.samples.clear();
                                self.chart.open_interest.support = None;
                            }
                        }
                        let has_cvd = self.has_cvd();
                        if had_cvd != has_cvd {
                            self.flow_generation = self.flow_generation.wrapping_add(1);
                            self.history.flow_error = None;
                            if has_cvd {
                                self.history.progress = Some("Loading native CVD history…".into());
                                self.history.flow_target = self.chart.oldest();
                            } else {
                                self.cancel_flow_history();
                                self.flow_status = None;
                                self.history.progress = None;
                                self.history.flow_target = None;
                            }
                            return Task::batch([focus, self.sync()]);
                        }
                        if self.chart.needs_older() {
                            return Task::batch([focus, self.older()]);
                        }
                        return focus;
                    }
                }
            }
            Message::MarketData(event) => {
                if !self.accepts(&event.scope.market, event.scope.generation)
                    || event.scope.flow_generation != self.flow_generation
                    || event.scope.cvd != self.has_cvd()
                {
                    return Task::none();
                }
                let mut flows = ArrayVec::<FlowBar, 64>::new();
                let mut repair = false;
                for event in event.events.into_vec() {
                    if let MarketEvent::Flow { bar } = event {
                        flows.push(bar);
                        continue;
                    }
                    self.flush_flows(&mut flows, true);
                    match event {
                        MarketEvent::Status { connected, message } => {
                            self.upstream_connected = connected;
                            self.status = message;
                            if !connected {
                                self.remember_anchor();
                                if self.chart.revoke_live_flow() {
                                    self.clear_flow_geometry();
                                }
                            }
                        }
                        MarketEvent::FlowStatus { connected, message } => {
                            if !connected && self.chart.revoke_live_flow() {
                                self.clear_flow_geometry();
                            }
                            self.flow_status = Some(if connected {
                                message
                            } else {
                                format!("Flow unavailable: {message}")
                            });
                        }
                        MarketEvent::Resync => {
                            self.remember_anchor();
                            repair = true;
                        }
                        MarketEvent::Candle { candle } => {
                            if self.chart.latest().is_some_and(|latest| {
                                candle.time > latest.time + self.chart.market.interval.seconds()
                            }) {
                                self.remember_anchor();
                                repair = true;
                            }
                            if self.chart.merge_stream(candle) {
                                self.clear_geometry();
                            }
                        }
                        MarketEvent::Flow { .. } => unreachable!(),
                    }
                }
                self.flush_flows(&mut flows, true);
                if repair {
                    return self.sync();
                }
            }
            Message::OpenInterest(batch) => {
                if !self.accepts(&batch.scope.market, batch.scope.generation)
                    || batch.scope.open_interest_generation != self.open_interest_history.generation
                    || !self.has_open_interest()
                {
                    return Task::none();
                }
                let mut samples = ArrayVec::<OpenInterestSample, 64>::new();
                for event in batch.events.into_vec() {
                    if let OpenInterestEvent::Sample(sample) = event {
                        samples.push(sample);
                        continue;
                    }
                    self.flush_open_interest(&mut samples);
                    match event {
                        OpenInterestEvent::Support(support) => {
                            self.chart.open_interest.support = Some(support);
                        }
                        OpenInterestEvent::History(page) if page.market == self.chart.market => {
                            if self.chart.merge_open_interest_samples(&page.samples) {
                                self.clear_open_interest_geometry();
                            }
                            self.chart.open_interest.support = Some(page.support.clone());
                            if !self.open_interest_history.initialized {
                                self.open_interest_history.initialized = true;
                                self.open_interest_history.next_before_ms = page.next_before_ms;
                            } else if let (Some(cursor), Some(next)) = (
                                self.open_interest_history.next_before_ms,
                                page.next_before_ms,
                            ) {
                                self.open_interest_history.next_before_ms = Some(next.min(cursor));
                            }
                        }
                        OpenInterestEvent::History(_) => {
                            self.chart.open_interest.connected = false;
                            self.chart.open_interest.message =
                                "Native OI returned history for a different market".into();
                        }
                        OpenInterestEvent::Status { connected, message } => {
                            self.chart.open_interest.connected = connected;
                            self.chart.open_interest.message = message;
                        }
                        OpenInterestEvent::Sample(_) => unreachable!(),
                    }
                    self.clear_open_interest_geometry();
                }
                self.flush_open_interest(&mut samples);
                return self.fetch_older_open_interest();
            }
            Message::OpenInterestHistory {
                market,
                generation,
                open_interest_generation,
                before_ms,
                result,
            } => {
                if !self.accepts(&market, generation)
                    || open_interest_generation != self.open_interest_history.generation
                    || !self.has_open_interest()
                {
                    return Task::none();
                }
                self.open_interest_history.in_flight = false;
                self.open_interest_history.cancel = None;
                match result {
                    Ok(page) if page.market == self.chart.market => {
                        self.open_interest_history.next_before_ms =
                            page.next_before_ms.filter(|cursor| *cursor < before_ms);
                        self.open_interest_history.error = None;
                        if self.chart.merge_open_interest_samples(&page.samples) {
                            self.clear_open_interest_geometry();
                        }
                    }
                    Ok(_) => {
                        self.open_interest_history.error =
                            Some("Native OI returned history for a different market".into());
                        self.open_interest_history.retry_ticks = 5;
                    }
                    Err(error) => {
                        self.open_interest_history.error = Some(error);
                        self.open_interest_history.retry_ticks = 5;
                    }
                }
                return self.fetch_older_open_interest();
            }
            Message::History {
                request,
                market,
                generation,
                history_id,
                epoch,
                result,
            } => {
                if !self.accepts(&market, generation) || history_id != self.history.generation {
                    return Task::none();
                }
                match result {
                    Ok(page) if page.market == self.chart.market => {
                        self.apply_history_page(Arc::new(page), request, epoch);
                        return self.finish_history(request);
                    }
                    Ok(_) => {
                        self.fail_history(
                            request,
                            "Native source returned history for a different market".into(),
                        );
                    }
                    Err(error) => self.fail_history(request, error),
                }
            }
            Message::HistoryFlow {
                request,
                market,
                generation,
                history_id,
                epoch,
                flow_generation,
                events,
            } => {
                if !self.accepts(&market, generation)
                    || history_id != self.history.generation
                    || !self.history.in_flight
                {
                    return Task::none();
                }
                let current_flow = flow_generation == self.flow_generation && self.has_cvd();
                let events = match events {
                    Ok(events) => events,
                    Err(error) if current_flow => {
                        self.history.flow_error = Some(error.clone());
                        self.fail_history(request, format!("CVD history: {error}"));
                        return Task::none();
                    }
                    Err(_) => return self.finish_stale_flow_history(),
                };
                let mut flows = ArrayVec::<FlowBar, 64>::new();
                for event in events.into_vec() {
                    if let HistoryEvent::Flow { bar } = event {
                        if current_flow {
                            flows.push(bar);
                        }
                        continue;
                    }
                    self.flush_flows(&mut flows, false);
                    match event {
                        HistoryEvent::History { page } => {
                            if page.market != self.chart.market {
                                self.fail_history(
                                    request,
                                    "Native source returned history for a different market".into(),
                                );
                                return Task::none();
                            }
                            self.apply_history_page(page, request, epoch);
                        }
                        HistoryEvent::FlowStatus { message } if current_flow => {
                            self.history.progress = Some(message);
                        }
                        HistoryEvent::Finished if current_flow => {
                            self.history.flow_error = None;
                            self.history.progress = None;
                            return self.finish_history(request);
                        }
                        HistoryEvent::Error { message } if current_flow => {
                            self.history.flow_error = Some(message.clone());
                            self.fail_history(request, format!("CVD history: {message}"));
                            return Task::none();
                        }
                        HistoryEvent::Finished | HistoryEvent::Error { .. } => {
                            return self.finish_stale_flow_history();
                        }
                        HistoryEvent::FlowStatus { .. } => {}
                        HistoryEvent::Flow { .. } => unreachable!(),
                    }
                }
                self.flush_flows(&mut flows, false);
            }
        }
        self.fetch_older_open_interest()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run_with_id(
                (
                    self.market_generation,
                    self.flow_generation,
                    self.chart.market.clone(),
                    self.has_cvd(),
                ),
                feeds::live(
                    self.data.clone(),
                    feeds::Scope {
                        market: self.chart.market.clone(),
                        generation: self.market_generation,
                        flow_generation: self.flow_generation,
                        cvd: self.has_cvd(),
                    },
                ),
            )
            .map(Message::MarketData),
            if self.has_open_interest() {
                Subscription::run_with_id(
                    (
                        "open_interest",
                        self.market_generation,
                        self.open_interest_history.generation,
                        self.chart.market.clone(),
                    ),
                    feeds::open_interest(
                        self.data.clone(),
                        feeds::OpenInterestScope {
                            market: self.chart.market.clone(),
                            generation: self.market_generation,
                            open_interest_generation: self.open_interest_history.generation,
                        },
                    ),
                )
                .map(Message::OpenInterest)
            } else {
                Subscription::none()
            },
            self.source_subscription(),
            iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
            iced::event::listen_with(|event, status, _| {
                if let iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key, modifiers, ..
                }) = event
                {
                    Some(Message::Key {
                        key,
                        modifiers,
                        captured: status == iced::event::Status::Captured,
                    })
                } else {
                    None
                }
            }),
        ])
    }

    fn theme(&self) -> Theme {
        self.theme_file.theme.clone()
    }

    fn title(&self) -> String {
        format!(
            "{} · {} · {} · {} · {}",
            self.chart.market.exchange,
            self.chart.market.product,
            self.chart.market.symbol,
            self.chart.market.interval,
            self.native_quote
        )
    }

    fn source_problem(&self, instance: &mexc_indicators::IndicatorInstance) -> Option<String> {
        if instance.source.is_chart() {
            return None;
        }
        if let Some(error) = instance.all_source_error() {
            return Some(error.to_owned());
        }
        for market in &instance.source.markets {
            let name = || format!("{} {}", market.exchange, market.symbol);
            if market.product != self.chart.market.product {
                return Some(format!(
                    "{} is {}; chart is {}. Choose a matching source or use Chart.",
                    name(),
                    market.product,
                    self.chart.market.product
                ));
            }
            let Some(Ok(catalog)) = self.source_catalogs.get(&(market.exchange, market.product))
            else {
                continue;
            };
            let Some(native) = catalog
                .instruments
                .iter()
                .find(|item| item.symbol == market.symbol)
            else {
                return Some(format!(
                    "{} is no longer listed. Choose an available native source or use Chart.",
                    name()
                ));
            };
            if native.base_coin != self.chart.base_asset {
                return Some(format!(
                    "{} supplies {}; chart is {}. Choose a matching source or use Chart.",
                    name(),
                    native.base_coin,
                    self.chart.base_asset
                ));
            }
            if matches!(instance.spec, IndicatorSpec::Cvd { .. })
                && !catalog.intervals.contains(&self.chart.market.interval)
            {
                return Some(format!(
                    "{} has no native {} CVD interval. Choose a compatible source or use Chart; bars are not resampled.",
                    market.exchange, self.chart.market.interval
                ));
            }
        }
        None
    }

    fn status_details_view(&self) -> Element<'_, Message> {
        let mut details = column![
            text("Prices & candle history").size(17),
            text(format!(
                "{} {} · {} · {} · {} candles",
                self.chart.market.exchange,
                self.chart.market.product,
                self.chart.market.symbol,
                self.chart.market.interval,
                self.chart.len()
            ))
            .size(13),
            text(format!(
                "Connection: {} · {}",
                if self.upstream_connected {
                    "live"
                } else {
                    "offline"
                },
                self.status
            ))
            .size(13),
            text(if self.chart.viewport.live {
                "Viewport follows realtime."
            } else {
                "Viewport is historical; data continues updating."
            })
            .size(13),
            text(if self.history.in_flight {
                "Loading candle history…"
            } else if self.chart.initialized && self.chart.next_before.is_none() {
                "Start of native candle history reached."
            } else {
                "History idle; older pages load at the left edge."
            })
            .size(13),
        ]
        .spacing(8);
        if let Some(error) = &self.history.error {
            details = details.push(text(format!("History error: {error}")).size(13));
        }
        if self.has_cvd() {
            details = details.push(text("Chart CVD").size(17));
            if let Some(status) = &self.flow_status {
                details = details.push(text(format!("Flow: {status}")).size(13));
            }
            if let Some(progress) = &self.history.progress {
                details = details.push(text(format!("History: {progress}")).size(13));
            }
            if let Some(error) = &self.history.flow_error {
                details = details.push(text(format!("History error: {error}")).size(13));
            }
            details = details.push(text("Per-instance legends show measured quality and accumulation origin; price connectivity does not imply complete CVD coverage.").size(12));
        }
        if self.has_open_interest() {
            details = details.push(text("Chart open interest").size(17));
            let native = &self.chart.open_interest;
            if let Some(support) = &native.support {
                details = details.push(text(format!("Source: {}", support.description)).size(13));
            }
            if !native.message.is_empty() {
                details = details.push(text(format!("Status: {}", native.message)).size(13));
            }
            if let Some(error) = &self.open_interest_history.error {
                details = details.push(text(format!("History error: {error}")).size(13));
            }
            details = details.push(text("Freshness, actual observation times and timestamp provenance are shown on each OI instance.").size(12));
        }
        if self
            .chart
            .indicators
            .instances
            .iter()
            .any(|item| !item.source.is_chart())
        {
            details = details.push(text("Independent indicator sources").size(17));
            for instance in self
                .chart
                .indicators
                .instances
                .iter()
                .filter(|item| !item.source.is_chart())
            {
                details = details.push(
                    button(
                        text(format!(
                            "#{} {} · {} · {}",
                            instance.id,
                            instance.spec.label(),
                            instance.source.mode,
                            instance
                                .external
                                .as_ref()
                                .map_or("Awaiting native data", |output| output.status.as_str())
                        ))
                        .size(13),
                    )
                    .on_press(Message::IndicatorDetails(instance.id))
                    .width(Length::Fill),
                );
            }
        }
        details = details.push(text("Market discovery").size(17));
        if self.selector.scope() == (self.chart.market.exchange, self.chart.market.product) {
            if self.selector.loading {
                details = details.push(text("Loading native market catalog…").size(13));
            } else if let Some(error) = &self.selector.catalog_error {
                details = details
                    .push(text(format!("Catalog error: {error}")).size(13))
                    .push(
                        button("Retry catalog")
                            .on_press(Message::Selector(market_selector::Event::Retry)),
                    );
            } else {
                details = details.push(text("Native catalog available.").size(13));
            }
        } else {
            details = details.push(
                text(if self.active_catalog.is_some() {
                    "Active native catalog retained independently of the cancelled market draft."
                } else {
                    "Awaiting active native catalog."
                })
                .size(13),
            );
        }
        details = details.push(text("Theme").size(17));
        details = details.push(
            text(self.theme_file.error.as_ref().map_or_else(
                || "Palette loaded.".to_owned(),
                |error| format!("{error} — keeping last valid palette; retrying every second"),
            ))
            .size(13),
        );
        details.into()
    }

    fn details_view(&self, details: Details) -> Element<'_, Message> {
        let (title, body) = match details {
            Details::Status => ("Status details".to_owned(), self.status_details_view()),
            Details::Indicator(id) => {
                let Some(instance) = self
                    .chart
                    .indicators
                    .instances
                    .iter()
                    .find(|item| item.id == id)
                else {
                    return text("Indicator removed").into();
                };
                (
                    format!("#{} {} details", id, instance.spec.label()),
                    chart::indicator_details_view(
                        &self.chart,
                        instance,
                        self.theme_file.palette,
                        &self.native_volume_unit,
                        self.source_problem(instance),
                    ),
                )
            }
        };
        let mut footer = row![].spacing(6);
        match details {
            Details::Status => {
                footer = footer.push(
                    button("Retry connection/history/theme")
                        .on_press(Message::HelpAction(Action::Retry)),
                );
            }
            Details::Indicator(id) => {
                footer = footer.push(button("Settings").on_press(Message::IndicatorSettings(id)));
            }
        }
        footer = footer.push(button("Close").on_press(Message::DetailsClose));
        column![
            row![
                text(title).size(20),
                iced::widget::Space::new(Length::Fill, Length::Shrink),
                button("Keys [F1]").on_press(Message::HelpOpen)
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            scrollable(body)
                .id(scrollable::Id::new("details-body"))
                .height(Length::Fill),
            footer.wrap(),
        ]
        .spacing(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn view(&self) -> Element<'_, Message> {
        let blocked = self.drawing_blocked();
        let candle = self.chart.inspected().or_else(|| self.chart.latest());
        let ohlc = candle
            .map(|c| {
                format!(
                    "{} UTC  O {}  H {}  L {}  C {}  Vol {} {}",
                    chart::utc(c.time, "%Y-%m-%d %H:%M"),
                    self.chart.format_price(c.open),
                    self.chart.format_price(c.high),
                    self.chart.format_price(c.low),
                    self.chart.format_price(c.close),
                    presentation::quantity(c.volume),
                    self.native_volume_unit
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "Waiting for {} · {} candles…",
                    self.chart.market.symbol, self.chart.market.interval
                )
            });
        let readout: Element<'_, Message> = if let Some(candle) = candle {
            tooltip(
                text(ohlc).size(13),
                text(format!(
                    "Native volume: {} {}",
                    presentation::quantity_exact(candle.volume),
                    self.native_volume_unit
                ))
                .size(12),
                tooltip::Position::Top,
            )
            .into()
        } else {
            text(ohlc).size(13).into()
        };
        let mut charts = column![].spacing(6);
        let has_lower = self
            .chart
            .indicators
            .instances
            .iter()
            .any(|item| item.visible && !item.spec.is_overlay());
        let mut legends = column![].spacing(2);
        let mut overlay_height = 0.0_f32;
        for instance in self
            .chart
            .indicators
            .instances
            .iter()
            .filter(|item| item.visible && item.spec.is_overlay())
        {
            overlay_height += chart::legend_height(instance) + 2.0;
            legends = legends.push(chart::legend_view(
                &self.chart,
                instance,
                self.theme_file.palette,
                &self.native_volume_unit,
                blocked,
                self.source_problem(instance),
            ));
        }
        if overlay_height > 0.0 {
            charts = charts.push(
                scrollable(legends)
                    .id(scrollable::Id::new("indicator-legends"))
                    .height(overlay_height.min(120.0)),
            );
        }
        charts = charts.push(chart::view(
            chart::Chart {
                data: &self.chart,
                cache: &self.cache,
                palette: self.theme_file.palette,
                indicator: None,
                quote: &self.native_quote,
                volume_unit: &self.native_volume_unit,
                drawings: Some(drawing_layer::View {
                    store: &self.drawings,
                    interaction: &self.drawing_interaction,
                    scene: &self.drawing_scene,
                    projection: &self.drawing_projection,
                }),
            },
            if has_lower {
                Length::Fixed(self.chart.plot_height(None))
            } else {
                Length::Fill
            },
            blocked,
        ));
        if has_lower {
            charts = charts.push(pane_resize::view(
                None,
                self.chart.plot_height(None),
                self.theme_file.palette,
                blocked,
            ));
        }
        for instance in self
            .chart
            .indicators
            .instances
            .iter()
            .filter(|item| item.visible && !item.spec.is_overlay())
        {
            if let Some(cache) = self.pane_caches.get(&instance.id) {
                let mut pane = column![chart::legend_view(
                    &self.chart,
                    instance,
                    self.theme_file.palette,
                    &self.native_volume_unit,
                    blocked,
                    self.source_problem(instance),
                )];
                if !self.chart.pane_collapsed(instance.id) {
                    let height = self.chart.plot_height(Some(instance.id));
                    pane = pane
                        .push(chart::view(
                            chart::Chart {
                                data: &self.chart,
                                cache,
                                palette: self.theme_file.palette,
                                indicator: Some(instance),
                                quote: &self.native_quote,
                                volume_unit: &self.native_volume_unit,
                                drawings: None,
                            },
                            Length::Fixed(height),
                            blocked,
                        ))
                        .push(pane_resize::view(
                            Some(instance.id),
                            height,
                            self.theme_file.palette,
                            blocked,
                        ));
                }
                charts = charts.push(pane.spacing(2));
            }
        }
        let charts: Element<'_, Message> = if has_lower {
            scrollable(charts.width(Length::Fill))
                .id(scrollable::Id::new("indicator-panes"))
                .height(Length::Fill)
                .into()
        } else {
            charts.width(Length::Fill).height(Length::Fill).into()
        };
        let market_control = button(text(&self.chart.market.symbol).size(19))
            .on_press_maybe((!self.help_open).then_some(Message::Action(Action::ManageMarket)))
            .padding([3, 6]);
        let keys_control = button(text("Keys [?]").size(12))
            .on_press_maybe((!self.help_open).then_some(Message::HelpOpen))
            .padding([3, 6]);
        let interval_control: Element<'_, Message> = if !self.native_intervals().is_empty() {
            pick_list(
                self.native_intervals(),
                Some(self.chart.market.interval),
                Message::SelectInterval,
            )
            .text_size(14)
            .padding([3, 6])
            .into()
        } else {
            text(self.chart.market.interval.to_string()).size(14).into()
        };
        let indicators_control = button(text("Indicators").size(14))
            .on_press_maybe((!self.help_open).then_some(Message::Action(Action::ManageIndicators)))
            .padding([3, 6]);
        let header = row![
            market_control,
            text(format!(
                "{} {} · {}",
                self.chart.market.exchange, self.chart.market.product, self.native_quote
            ))
            .size(14),
            interval_control,
            indicators_control,
            button(text("Draw").size(14))
                .padding([3, 6])
                .on_press_maybe((!blocked).then_some(Message::DrawingMenu)),
            iced::widget::Space::new(Length::Fill, Length::Shrink),
            keys_control,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .wrap();
        let status_control = |label: String| {
            button(text(label).size(12))
                .style(button::text)
                .padding([3, 5])
                .on_press(Message::StatusDetails)
        };
        let history_state = if self.history.error.is_some() {
            "error"
        } else if self.history.in_flight {
            "loading"
        } else if self.chart.initialized && self.chart.next_before.is_none() {
            "start reached"
        } else {
            "ready"
        };
        let mut status_row = row![
            status_control(format!(
                "Prices {}",
                if self.upstream_connected {
                    "live"
                } else {
                    "offline"
                }
            )),
            status_control(format!("History {history_state}")),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        let cvd = self
            .chart
            .indicators
            .instances
            .iter()
            .filter(|item| matches!(item.spec, IndicatorSpec::Cvd { .. }))
            .count();
        let oi = self
            .chart
            .indicators
            .instances
            .iter()
            .filter(|item| item.spec == IndicatorSpec::OpenInterest)
            .count();
        if cvd != 0 {
            let problem = self.history.flow_error.is_some()
                || self.chart.indicators.instances.iter().any(|item| {
                    matches!(item.spec, IndicatorSpec::Cvd { .. })
                        && !item.source.is_chart()
                        && item.external.as_ref().is_none_or(|output| !output.complete)
                });
            status_row = status_row.push(status_control(format!(
                "CVD {cvd} · {}",
                if problem {
                    "needs attention"
                } else {
                    "see quality"
                }
            )));
        }
        if oi != 0 {
            let problem = self.open_interest_history.error.is_some()
                || self.chart.indicators.instances.iter().any(|item| {
                    item.spec == IndicatorSpec::OpenInterest
                        && !item.source.is_chart()
                        && item.external.as_ref().is_none_or(|output| !output.complete)
                });
            status_row = status_row.push(status_control(format!(
                "OI {oi} · {}",
                if problem {
                    "needs attention"
                } else {
                    "see freshness"
                }
            )));
        }
        let active_catalog_scope =
            self.selector.scope() == (self.chart.market.exchange, self.chart.market.product);
        if active_catalog_scope && (self.selector.loading || self.selector.catalog_error.is_some())
        {
            status_row = status_row.push(status_control(format!(
                "Catalog {}",
                if self.selector.loading {
                    "loading"
                } else {
                    "error"
                }
            )));
        }
        if self.theme_file.error.is_some() {
            status_row = status_row.push(status_control("Theme error".into()));
        }
        status_row = status_row.push(status_control("Details".into()));
        if !self.chart.viewport.live {
            status_row = status_row.push(tooltip(
                button(text("Return to realtime").size(12))
                    .on_press_maybe((!self.help_open).then_some(Message::Action(Action::Live)))
                    .padding([3, 6]),
                text("Resume live following · L / End · keep manual value scales").size(12),
                tooltip::Position::Top,
            ));
        }
        if let Some(error) = &self.drawing_error {
            status_row = status_row.push(text(format!("Draw: {error}")).size(12));
        }
        let mut content = column![header, readout].spacing(7);
        if self.drawing_interaction.tool() != DrawingTool::Select
            || self.drawings.selected().is_some()
        {
            let mut controls =
                row![text(format!("Draw · {}", self.drawing_interaction.tool())).size(12)];
            if let Some(drawing) = self.drawings.selected_drawing() {
                let generation = self.drawings.generation();
                controls =
                    controls
                        .push(
                            text(format!(
                                "#{} · {}{}",
                                drawing.id,
                                drawing.shape.tool(),
                                if drawing.locked { " · locked" } else { "" }
                            ))
                            .size(12),
                        )
                        .push(button("Properties").padding([2, 5]).on_press_maybe(
                            (!blocked).then_some(Message::DrawingProperties(drawing.id)),
                        ))
                        .push(
                            button(if drawing.visible { "Hide" } else { "Show" })
                                .padding([2, 5])
                                .on_press_maybe((!blocked).then_some(Message::DrawingCommand {
                                    generation,
                                    command: DrawingCommand::SetVisible {
                                        id: drawing.id,
                                        visible: !drawing.visible,
                                    },
                                })),
                        )
                        .push(
                            button(if drawing.locked { "Unlock" } else { "Lock" })
                                .padding([2, 5])
                                .on_press_maybe((!blocked).then_some(Message::DrawingCommand {
                                    generation,
                                    command: DrawingCommand::SetLocked {
                                        id: drawing.id,
                                        locked: !drawing.locked,
                                    },
                                })),
                        )
                        .push(button("Delete").padding([2, 5]).on_press_maybe(
                            (!blocked).then_some(Message::DrawingCommand {
                                generation,
                                command: DrawingCommand::Delete(drawing.id),
                            }),
                        ));
            } else {
                controls = controls
                    .push(text("Click anchors / arrows + Enter · Space-drag pans").size(12));
            }
            controls = controls.push(
                button("Select / clear")
                    .padding([2, 5])
                    .on_press_maybe((!blocked).then_some(Message::DrawingCancel)),
            );
            content = content.push(controls.spacing(5).align_y(iced::Alignment::Center).wrap());
        }
        let content = content.push(charts).push(status_row.wrap());
        let base: Element<'_, Message> = container(content)
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        let mut layers = iced::widget::stack![key_help::modal_background(base, blocked)]
            .width(Length::Fill)
            .height(Length::Fill);
        let dialog = if self.manager.open {
            Some((self.manager.view(&self.chart), Message::EditorClose))
        } else if self.selector.open {
            Some((self.selector.view(), Message::EditorClose))
        } else if self.drawing_controls.is_open() {
            Some((
                self.drawing_controls.view(
                    &self.drawings,
                    self.drawing_interaction.tool(),
                    self.drawing_interaction.magnet(),
                    self.chart.latest().is_some(),
                    self.theme_file.palette,
                    self.chart.price_scale,
                    &self.native_quote,
                ),
                Message::DrawingClose,
            ))
        } else {
            self.details
                .map(|details| (self.details_view(details), Message::DetailsClose))
        };
        if let Some((body, close)) = dialog {
            layers = layers.push(key_help::modal_background(
                dialog_layer(body, close, self.theme_file.palette),
                self.help_open,
            ));
        }
        if self.help_open {
            layers = layers.push(key_help::view(self.help_context(), self.help_selection));
        }
        layers.into()
    }
}

fn dialog_layer<'a>(
    body: Element<'a, Message>,
    close: Message,
    palette: palette::Palette,
) -> Element<'a, Message> {
    let backdrop = mouse_area(
        container(iced::widget::Space::new(Length::Fill, Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.55).into()),
                ..Default::default()
            }),
    )
    .on_press(close)
    .on_move(|_| Message::ModalBackdrop);
    let panel = container(body)
        .padding(16)
        .width(Length::Fill)
        .max_width(720)
        .height(Length::Fill)
        .max_height(680)
        .style(move |_| container::Style {
            background: Some(palette.background.into()),
            border: iced::Border {
                color: palette.muted,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        });
    iced::widget::stack![
        backdrop,
        container(panel)
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::Alignment::Center)
            .align_y(iced::Alignment::Center),
    ]
    .into()
}

fn zoom(time: f64, price: f64) -> Action {
    Action::Zoom {
        time,
        price,
        x: 0.5,
        y: 0.5,
    }
}

fn key_action(key: keyboard::Key<&str>, modifiers: keyboard::Modifiers) -> Option<Action> {
    use keyboard::{Key, key::Named};
    if modifiers.control() && modifiers.alt() {
        return match key {
            Key::Named(Named::ArrowUp) => Some(Action::AdjustPaneHeight {
                id: None,
                delta: 32.0,
            }),
            Key::Named(Named::ArrowDown) => Some(Action::AdjustPaneHeight {
                id: None,
                delta: -32.0,
            }),
            Key::Character("0") => Some(Action::ResetPaneHeight(None)),
            _ => None,
        };
    }
    let pan = if modifiers.shift() { 0.5 } else { 0.12 };
    match key {
        Key::Named(Named::PageUp | Named::PageDown) => Some(Action::ScrollPanes {
            amount: if matches!(key, Key::Named(Named::PageUp)) {
                -280.0
            } else {
                280.0
            },
            legends: modifiers.shift(),
        }),
        Key::Named(Named::ArrowLeft) => Some(Action::Pan {
            time: -pan,
            price: 0.0,
        }),
        Key::Named(Named::ArrowRight) => Some(Action::Pan {
            time: pan,
            price: 0.0,
        }),
        Key::Named(Named::ArrowUp) if modifiers.control() => Some(zoom(1.0, 0.8)),
        Key::Named(Named::ArrowDown) if modifiers.control() => Some(zoom(1.0, 1.25)),
        Key::Named(Named::ArrowUp) => Some(Action::Pan {
            time: 0.0,
            price: pan,
        }),
        Key::Named(Named::ArrowDown) => Some(Action::Pan {
            time: 0.0,
            price: -pan,
        }),
        Key::Named(Named::Home) => Some(Action::Reset),
        Key::Named(Named::End) => Some(Action::Live),
        Key::Named(Named::Escape) => Some(Action::Inspect(None)),
        Key::Character("+" | "=") => Some(if modifiers.shift() {
            zoom(1.0, 0.8)
        } else {
            zoom(0.8, 1.0)
        }),
        Key::Character("-" | "_") => Some(if modifiers.shift() {
            zoom(1.0, 1.25)
        } else {
            zoom(1.25, 1.0)
        }),
        Key::Character("[" | ",") => Some(Action::InspectStep(-1)),
        Key::Character("]" | ".") => Some(Action::InspectStep(1)),
        Key::Character(value) => match value {
            "r" | "R" => Some(Action::Reset),
            "f" | "F" => Some(Action::AutoFit),
            "l" | "L" => Some(Action::Live),
            "b" | "B" => Some(Action::Older),
            "y" | "Y" => Some(Action::Retry),
            "i" | "I" => Some(Action::ManageIndicators),
            "m" | "M" => Some(Action::ManageMarket),
            _ => None,
        },
        _ => None,
    }
}

fn main() -> iced::Result {
    iced::application(Desktop::title, Desktop::update, Desktop::view)
        .theme(Desktop::theme)
        .subscription(Desktop::subscription)
        .window_size((1200.0, 760.0))
        .run_with(Desktop::new)
}

#[cfg(test)]
mod tests {
    fn flow_support() -> mexc_model::FlowSupport {
        mexc_model::FlowSupport {
            history: mexc_model::FlowAvailability::Unavailable,
            live: mexc_model::FlowAvailability::Unavailable,
            description: "test fixture".into(),
        }
    }
    use super::*;

    fn page(oldest: i64, next_before: Option<i64>) -> HistoryPage {
        HistoryPage {
            market: Market::default(),
            candles: vec![mexc_model::Candle {
                time: oldest,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 1.0,
                flow: None,
            }],
            next_before,
        }
    }

    #[test]
    fn reconnect_repairs_multiple_pages_until_prior_watermark() {
        let first =
            repair_request(Request::Recent, &page(900_000, Some(900_000)), Some(300)).unwrap();
        let second = repair_request(first, &page(600_000, Some(600_000)), Some(300)).unwrap();
        let third = repair_request(second, &page(300_000, Some(300_000)), Some(300)).unwrap();
        assert!(repair_request(third, &page(0, Some(0)), Some(300)).is_none());
    }

    #[test]
    fn exhausted_and_nonprogressing_repair_cursors_stop() {
        assert!(repair_request(Request::Recent, &page(900, None), Some(300)).is_none());
        assert!(
            repair_request(
                Request::Repair {
                    before: 900,
                    target: 300
                },
                &page(900, Some(900)),
                Some(300)
            )
            .is_none()
        );
    }

    #[test]
    fn flow_backfill_keeps_using_the_older_page_cursor_until_loaded_history_is_covered() {
        let first = Request::FlowBackfill {
            before: 1_200,
            target: 300,
        };
        let next = repair_request(first, &page(900, Some(800)), Some(300)).unwrap();
        assert_eq!(
            next,
            Request::FlowBackfill {
                before: 800,
                target: 300
            }
        );
        let last = repair_request(next, &page(300, Some(200)), Some(300));
        assert!(last.is_none());
    }

    #[derive(Debug, PartialEq)]
    struct PipelineSnapshot {
        market: Market,
        generation: u64,
        latest: Option<mexc_model::Candle>,
        candles: usize,
        epoch: u64,
        next_before: Option<i64>,
        inspection: Option<(i64, f64)>,
        in_flight: bool,
        pending_sync: bool,
        sync_anchor: Option<i64>,
        failed: Option<Request>,
        retry_ticks: u8,
        upstream_connected: bool,
        status: String,
        history_error: Option<String>,
        catalog_loading: bool,
        catalog_error: Option<String>,
    }

    fn pipeline_snapshot(desktop: &Desktop) -> PipelineSnapshot {
        PipelineSnapshot {
            market: desktop.chart.market.clone(),
            generation: desktop.market_generation,
            latest: desktop.chart.latest().copied(),
            candles: desktop.chart.len(),
            epoch: desktop.chart.epoch,
            next_before: desktop.chart.next_before,
            inspection: desktop.chart.inspection,
            in_flight: desktop.history.in_flight,
            pending_sync: desktop.history.pending_sync,
            sync_anchor: desktop.history.anchor,
            failed: desktop.history.failed,
            retry_ticks: desktop.history.retry_ticks,
            upstream_connected: desktop.upstream_connected,
            status: desktop.status.clone(),
            history_error: desktop.history.error.clone(),
            catalog_loading: desktop.selector.loading,
            catalog_error: desktop.selector.catalog_error.clone(),
        }
    }

    fn data_message(market: &Market, generation: u64, event: MarketEvent) -> Message {
        Message::MarketData(feeds::Event {
            scope: Arc::new(feeds::Scope {
                market: market.clone(),
                generation,
                flow_generation: 0,
                cvd: false,
            }),
            events: vec![event].into_boxed_slice(),
        })
    }

    fn assert_stale_callbacks_ignored(desktop: &mut Desktop, market: &Market, generation: u64) {
        let mut stale_page = page(30_000, Some(0));
        stale_page.market = market.clone();
        let snapshot = pipeline_snapshot(desktop);
        let events = [
            Message::History {
                market: market.clone(),
                generation,
                history_id: desktop.history.generation.wrapping_sub(1),
                request: Request::Recent,
                epoch: 0,
                result: Err("old native request failure".into()),
            },
            Message::History {
                market: market.clone(),
                generation,
                history_id: desktop.history.generation.wrapping_sub(1),
                request: Request::Older { before: 30_000 },
                epoch: 0,
                result: Ok(stale_page),
            },
            Message::Catalog {
                exchange: market.exchange,
                product: market.product,
                generation: desktop.catalog_generation.wrapping_sub(1),
                result: Err("old catalog failure".into()),
            },
            Message::Catalog {
                exchange: market.exchange,
                product: market.product,
                generation: desktop.catalog_generation.wrapping_sub(1),
                result: Ok(Arc::new(Catalog {
                    exchange: market.exchange,
                    product: market.product,
                    intervals: vec![mexc_model::Timeframe::Min5],
                    flow_support: flow_support(),
                    instruments: Vec::new(),
                })),
            },
            data_message(
                market,
                generation,
                MarketEvent::Status {
                    connected: false,
                    message: "old upstream status".into(),
                },
            ),
            data_message(market, generation, MarketEvent::Resync),
            data_message(
                market,
                generation,
                MarketEvent::Candle {
                    candle: page(60_000, None).candles[0],
                },
            ),
        ];
        for event in events {
            let _ = desktop.update(event);
            assert_eq!(pipeline_snapshot(desktop), snapshot);
            assert!(desktop.selector.active_instrument(&market.symbol).is_some());
        }
    }

    #[test]
    fn rapid_a_b_a_and_retry_discard_old_callbacks_before_pipeline_side_effects() {
        let (mut desktop, _) = Desktop::new();
        let a = Market::default();
        let b = Market {
            exchange: a.exchange,
            product: a.product,
            symbol: "ETH_USDC".into(),
            interval: mexc_model::Timeframe::Min5,
        };
        desktop.selector.set_catalog(
            a.exchange,
            a.product,
            Ok(Arc::new(Catalog {
                exchange: a.exchange,
                product: a.product,
                flow_support: flow_support(),
                intervals: vec![mexc_model::Timeframe::Min5],
                instruments: [&a, &b]
                    .into_iter()
                    .map(|market| mexc_model::Instrument {
                        exchange: market.exchange,
                        product: market.product,
                        symbol: market.symbol.clone(),
                        base_coin: market.symbol.split('_').next().unwrap().into(),
                        quote_coin: mexc_model::Quote::USDC,
                        settle_coin: "USDC".into(),
                        price_scale: 2,
                        contract_size: 1.0,
                        inverse: false,
                        volume_unit: "contracts".into(),
                    })
                    .collect(),
            })),
        );
        let old_a_generation = desktop.market_generation;
        let _ = desktop.update(Message::Action(Action::SelectMarket(b)));
        let _ = desktop.update(Message::Action(Action::SelectMarket(a.clone())));
        assert!(desktop.market_generation > old_a_generation);
        assert_eq!(desktop.chart.market, a);
        assert_eq!(desktop.chart.len(), 0);
        assert!(desktop.history.in_flight);
        let current_generation = desktop.market_generation;
        let _ = desktop.update(Message::History {
            market: a.clone(),
            generation: current_generation,
            history_id: desktop.history.generation,
            request: Request::Recent,
            epoch: 0,
            result: Ok(page(300, None)),
        });
        assert_eq!(desktop.chart.latest().unwrap().time, 300);
        let _ = desktop.update(data_message(
            &a,
            current_generation,
            MarketEvent::Status {
                connected: true,
                message: "current upstream".into(),
            },
        ));
        let _ = desktop.update(data_message(&a, current_generation, MarketEvent::Resync));
        let _ = desktop.load_catalog(a.exchange, a.product);
        assert_eq!(desktop.history.anchor, Some(300));
        assert!(desktop.history.in_flight);
        assert_stale_callbacks_ignored(&mut desktop, &a, old_a_generation);

        let _ = desktop.update(Message::Action(Action::Retry));
        assert!(desktop.market_generation > current_generation);
        assert!(desktop.history.in_flight);
        assert_stale_callbacks_ignored(&mut desktop, &a, current_generation);
    }

    #[test]
    fn modal_help_preserves_chart_state_from_keyboard_and_canvas_input() {
        use keyboard::key::Named;
        let (mut desktop, _) = Desktop::new();
        let press = |key, modifiers| Message::Key {
            key: keyboard::Key::Named(key),
            modifiers,
            captured: true,
        };
        let plain = keyboard::Modifiers::default();
        let _ = desktop.update(Message::History {
            request: Request::Recent,
            market: desktop.chart.market.clone(),
            generation: desktop.market_generation,
            history_id: desktop.history.generation,
            epoch: 0,
            result: Ok(page(300, None)),
        });
        let _ = desktop.update(Message::Action(Action::Inspect(Some((300.0, 1.0)))));
        assert_eq!(desktop.chart.inspection, Some((300, 1.0)));
        let original = desktop.chart.viewport;

        let _ = desktop.update(press(Named::F1, plain));
        let _ = desktop.update(Message::Action(Action::Inspect(Some((300.0, 1.05)))));
        let _ = desktop.update(Message::Action(Action::Pan {
            time: -0.12,
            price: 0.12,
        }));
        let _ = desktop.update(Message::Action(zoom(1.2, 1.2)));
        let _ = desktop.update(press(Named::ArrowLeft, plain));
        let _ = desktop.update(press(Named::ArrowUp, plain));
        let _ = desktop.update(press(Named::PageDown, plain));
        let _ = desktop.update(press(Named::Tab, plain));
        let _ = desktop.update(press(Named::Tab, keyboard::Modifiers::SHIFT));
        assert!(desktop.help_open);
        let _ = desktop.update(press(Named::Enter, plain));
        assert!(!desktop.help_open);

        let _ = desktop.update(press(Named::F1, plain));
        let _ = desktop.update(press(Named::Tab, keyboard::Modifiers::SHIFT));
        let _ = desktop.update(press(Named::Tab, plain));
        let _ = desktop.update(press(Named::Space, plain));
        assert!(!desktop.help_open);
        assert!(!desktop.selector.open);
        assert!(!desktop.manager.open);
        assert_eq!(desktop.chart.viewport.left, original.left);
        assert_eq!(desktop.chart.viewport.span, original.span);
        assert_eq!(desktop.chart.viewport.low, original.low);
        assert_eq!(desktop.chart.viewport.high, original.high);
        assert_eq!(desktop.chart.viewport.live, original.live);
        assert_eq!(desktop.chart.viewport.auto_fit, original.auto_fit);
        assert_eq!(desktop.chart.inspection, Some((300, 1.0)));
    }

    #[test]
    fn stale_cvd_history_completion_cannot_finish_a_newer_request() {
        let (mut desktop, _) = Desktop::new();
        let old_history_id = desktop.history.generation;
        let old_market_generation = desktop.market_generation;
        let old_flow_generation = desktop.flow_generation;
        let market = desktop.chart.market.clone();
        let _ = desktop.update(Message::Action(Action::AddIndicator(IndicatorSpec::Cvd {
            reset: mexc_indicators::CvdReset::Continuous,
        })));
        let _ = desktop.fetch(Request::Recent);
        let active_history_id = desktop.history.generation;
        assert!(desktop.history.in_flight);

        let _ = desktop.update(Message::HistoryFlow {
            request: Request::Recent,
            market,
            generation: old_market_generation,
            history_id: old_history_id,
            epoch: 0,
            flow_generation: old_flow_generation,
            events: Ok(vec![HistoryEvent::Finished].into_boxed_slice()),
        });
        assert!(desktop.history.in_flight);
        assert_eq!(desktop.history.generation, active_history_id);
    }

    #[test]
    fn retry_resumes_native_backfill_and_rebases_previously_missing_cvd_prefix() {
        let (mut desktop, _) = Desktop::new();
        let mut loaded = page(300, Some(300));
        loaded.candles.push(page(600, None).candles[0]);
        loaded.candles.push(page(900, None).candles[0]);
        desktop.chart.merge_page(&loaded, 0);
        desktop
            .chart
            .dispatch(Action::AddIndicator(IndicatorSpec::Cvd {
                reset: mexc_indicators::CvdReset::Continuous,
            }));
        desktop.chart.viewport.live = false;
        desktop.chart.viewport.left = 900.0;
        desktop.chart.viewport.span = 300.0;
        desktop.restart_connection();
        let _ = desktop.fetch(Request::Recent);
        let recent_id = desktop.history.generation;
        let deliver = |desktop: &mut Desktop, request, history_id, event| {
            let _ = desktop.update(Message::HistoryFlow {
                request,
                market: desktop.chart.market.clone(),
                generation: desktop.market_generation,
                history_id,
                epoch: 0,
                flow_generation: desktop.flow_generation,
                events: Ok(vec![event].into_boxed_slice()),
            });
        };
        let flow = |time, delta: f64| HistoryEvent::Flow {
            bar: mexc_model::FlowBar {
                time,
                flow: mexc_model::VolumeFlow {
                    delta: Some(delta),
                    volume: Some(delta.abs()),
                    quality: mexc_model::FlowQuality::Exact,
                    covered_from: time,
                    through_ms: (time + 300) * 1000,
                },
            },
        };
        let mut recent = page(600, Some(600));
        recent.candles.push(page(900, None).candles[0]);
        deliver(
            &mut desktop,
            Request::Recent,
            recent_id,
            HistoryEvent::History {
                page: Arc::new(recent),
            },
        );
        deliver(&mut desktop, Request::Recent, recent_id, flow(600, 2.0));
        deliver(&mut desktop, Request::Recent, recent_id, flow(900, 1.0));
        assert_eq!(
            desktop.chart.indicators.instances[0].points[0].values[0],
            None
        );
        deliver(
            &mut desktop,
            Request::Recent,
            recent_id,
            HistoryEvent::Finished,
        );

        let older = Request::FlowBackfill {
            before: 600,
            target: 300,
        };
        deliver(
            &mut desktop,
            older,
            recent_id + 1,
            HistoryEvent::History {
                page: Arc::new(page(300, Some(300))),
            },
        );
        deliver(&mut desktop, older, recent_id + 1, flow(300, 1.0));
        assert_eq!(
            desktop.chart.indicators.instances[0].points[0].values[0],
            Some(1.0)
        );
        assert_eq!(
            desktop.chart.indicators.instances[0].points[2].values[0],
            Some(4.0)
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .cvd_context(2)
                .unwrap()
                .origin,
            300
        );
    }
    #[test]
    fn control_arrows_zoom_the_target_pane_without_moving_other_scales() {
        let (mut desktop, _) = Desktop::new();
        desktop
            .chart
            .dispatch(Action::AddIndicator(IndicatorSpec::Rsi { period: 2 }));
        let id = desktop.chart.indicators.instances.last().unwrap().id;
        let _ = desktop.update(Message::Action(Action::ActivatePane(Some(id))));
        let before = desktop.chart.pane_scale(id);
        let price = desktop.chart.viewport;
        let key = |key| Message::Key {
            key: keyboard::Key::Named(key),
            modifiers: keyboard::Modifiers::CTRL,
            captured: false,
        };
        let _ = desktop.update(key(keyboard::key::Named::ArrowUp));
        let after = desktop.chart.pane_scale(id);
        assert!(!after.auto_fit);
        assert!(((after.high - after.low) / (before.high - before.low) - 0.8).abs() < 1e-12);
        assert!((after.low + after.high - before.low - before.high).abs() < 1e-12);
        assert_eq!(desktop.chart.viewport.left, price.left);
        assert_eq!(desktop.chart.viewport.span, price.span);
        assert_eq!(desktop.chart.viewport.low, price.low);
        assert_eq!(desktop.chart.viewport.high, price.high);
        let _ = desktop.update(key(keyboard::key::Named::ArrowDown));
        let restored = desktop.chart.pane_scale(id);
        assert!((restored.low - before.low).abs() < 1e-12);
        assert!((restored.high - before.high).abs() < 1e-12);
    }

    fn open_interest_message(desktop: &Desktop, events: Vec<OpenInterestEvent>) -> Message {
        Message::OpenInterest(feeds::OpenInterestBatch {
            scope: Arc::new(feeds::OpenInterestScope {
                market: desktop.chart.market.clone(),
                generation: desktop.market_generation,
                open_interest_generation: desktop.open_interest_history.generation,
            }),
            events: events.into_boxed_slice(),
        })
    }

    fn open_interest_page(desktop: &Desktop, cursor: Option<i64>) -> OpenInterestPage {
        OpenInterestPage {
            market: desktop.chart.market.clone(),
            support: mexc_model::OpenInterestSupport {
                unit: "contracts".into(),
                history_interval_ms: Some(300_000),
                live_interval_ms: Some(10_000),
                description: "Native historical and live quantities".into(),
            },
            samples: Vec::new(),
            next_before_ms: cursor,
        }
    }

    #[test]
    fn late_open_interest_cannot_cross_removed_and_readded_instance_lifetimes() {
        let (mut desktop, _) = Desktop::new();
        let _ = desktop.update(Message::Action(Action::AddIndicator(
            IndicatorSpec::OpenInterest,
        )));
        let id = desktop.chart.indicators.instances.last().unwrap().id;
        let sample = OpenInterestSample {
            timestamp_ms: 1_000_000,
            quantity: 12.0,
            interval_ms: 10_000,
            timestamp_source: mexc_model::OpenInterestTimestamp::Venue,
        };
        let stale = open_interest_message(&desktop, vec![OpenInterestEvent::Sample(sample)]);
        let stale_history = Message::OpenInterestHistory {
            market: desktop.chart.market.clone(),
            generation: desktop.market_generation,
            open_interest_generation: desktop.open_interest_history.generation,
            before_ms: 1_000_000,
            result: Err("old market history failed".into()),
        };
        let _ = desktop.update(Message::Action(Action::RemoveIndicator(id)));
        let _ = desktop.update(Message::Action(Action::AddIndicator(
            IndicatorSpec::OpenInterest,
        )));
        let current = OpenInterestSample {
            quantity: 34.0,
            ..sample
        };
        let message = open_interest_message(&desktop, vec![OpenInterestEvent::Sample(current)]);
        let _ = desktop.update(message);
        let _ = desktop.update(stale);
        let _ = desktop.update(stale_history);
        assert_eq!(desktop.chart.open_interest.samples, vec![current]);
        assert_eq!(desktop.open_interest_history.error, None);
    }

    #[test]
    fn refreshed_recent_oi_history_never_rewinds_or_reopens_the_older_cursor() {
        let (mut desktop, _) = Desktop::new();
        let _ = desktop.update(Message::Action(Action::AddIndicator(
            IndicatorSpec::OpenInterest,
        )));
        let page = open_interest_page(&desktop, Some(5_000_000));
        let message =
            open_interest_message(&desktop, vec![OpenInterestEvent::History(Arc::new(page))]);
        let _ = desktop.update(message);
        desktop.open_interest_history.next_before_ms = Some(1_000_000);
        let page = open_interest_page(&desktop, Some(6_000_000));
        let message =
            open_interest_message(&desktop, vec![OpenInterestEvent::History(Arc::new(page))]);
        let _ = desktop.update(message);
        assert_eq!(
            desktop.open_interest_history.next_before_ms,
            Some(1_000_000)
        );
        desktop.open_interest_history.next_before_ms = None;
        let page = open_interest_page(&desktop, Some(7_000_000));
        let message =
            open_interest_message(&desktop, vec![OpenInterestEvent::History(Arc::new(page))]);
        let _ = desktop.update(message);
        assert_eq!(desktop.open_interest_history.next_before_ms, None);
    }

    #[test]
    fn external_source_lifetimes_preserve_shared_totals_and_reject_old_samples() {
        use mexc_indicators::{IndicatorSource, SourceMode};
        use mexc_model::{OpenInterestSupport, OpenInterestTimestamp};
        let (mut desktop, _) = Desktop::new();
        let binance = Market {
            exchange: Exchange::Binance,
            symbol: "BTCUSDT".into(),
            ..Market::default()
        };
        let mexc = desktop.chart.market.clone();
        let apply = |markets| Action::ApplyIndicator {
            id: None,
            spec: IndicatorSpec::OpenInterest,
            source: IndicatorSource {
                mode: SourceMode::Selected,
                markets,
            },
        };
        let _ = desktop.update(Message::Action(apply(vec![binance.clone()])));
        let single = desktop.chart.indicators.instances[0].id;
        let _ = desktop.update(Message::Action(apply(vec![binance.clone(), mexc.clone()])));
        let aggregate = desktop.chart.indicators.instances[1].id;
        let batch = |desktop: &Desktop, market: &Market, quantity| {
            let now = source_controller::now_ms();
            Message::SourceData(source_feeds::Batch {
                scope: Arc::new(source_feeds::Scope {
                    market: market.clone(),
                    kind: source_feeds::Kind::OpenInterest,
                    generation: desktop.source_requests
                        [&(market.clone(), source_feeds::Kind::OpenInterest)]
                        .generation,
                    base_asset: "BTC".into(),
                }),
                events: vec![
                    source_feeds::Event::Metadata {
                        base_asset: "BTC".into(),
                    },
                    source_feeds::Event::OpenInterest(OpenInterestEvent::Support(
                        OpenInterestSupport {
                            unit: "BTC".into(),
                            history_interval_ms: None,
                            live_interval_ms: Some(5000),
                            description: String::new(),
                        },
                    )),
                    source_feeds::Event::OpenInterest(OpenInterestEvent::Sample(
                        OpenInterestSample {
                            timestamp_ms: now,
                            timestamp_source: OpenInterestTimestamp::Venue,
                            quantity,
                            interval_ms: 5000,
                        },
                    )),
                    source_feeds::Event::OpenInterest(OpenInterestEvent::Status {
                        connected: true,
                        message: String::new(),
                    }),
                ]
                .into_boxed_slice(),
            })
        };
        let first = batch(&desktop, &binance, 5.0);
        let second = batch(&desktop, &mexc, 10.0);
        let stale = batch(&desktop, &mexc, 999.0);
        let _ = desktop.update(first);
        let _ = desktop.update(second);
        assert_eq!(
            desktop.chart.indicators.instances[1]
                .external
                .as_ref()
                .unwrap()
                .oi
                .last()
                .unwrap()
                .quantity,
            15.0
        );
        let old_binance = batch(&desktop, &binance, 999.0);
        let mexc_generation =
            desktop.source_requests[&(mexc.clone(), source_feeds::Kind::OpenInterest)].generation;
        let _ = desktop.update(Message::RetryIndicatorSources(single));
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert!(
            !desktop.chart.indicators.instances[1]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert_eq!(
            desktop.source_requests[&(mexc.clone(), source_feeds::Kind::OpenInterest)].generation,
            mexc_generation
        );
        let _ = desktop.update(old_binance);
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        let recovered = batch(&desktop, &binance, 5.0);
        let _ = desktop.update(recovered);
        assert_eq!(
            desktop.chart.indicators.instances[1]
                .external
                .as_ref()
                .unwrap()
                .oi
                .last()
                .unwrap()
                .quantity,
            15.0
        );
        let _ = desktop.update(Message::EditorClose);
        let _ = desktop.update(Message::Action(Action::RemoveIndicator(aggregate)));
        let _ = desktop.update(stale.clone());
        assert_eq!(desktop.chart.indicators.instances[0].id, single);
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .oi
                .last()
                .unwrap()
                .quantity,
            5.0
        );
        let _ = desktop.update(Message::Action(apply(vec![binance, mexc.clone()])));
        let current = batch(&desktop, &mexc, 17.0);
        let _ = desktop.update(current);
        let _ = desktop.update(stale);
        let external = desktop.chart.indicators.instances[1]
            .external
            .as_ref()
            .unwrap();
        assert!(external.complete);
        assert_eq!(external.oi.last().unwrap().quantity, 22.0);
    }

    #[test]
    fn native_resync_repairs_cvd_coverage_without_falsely_disconnecting() {
        use mexc_indicators::{CvdReset, IndicatorSource, SourceMode};
        use mexc_model::{FlowQuality, VolumeFlow};
        let (mut desktop, _) = Desktop::new();
        let market = Market {
            exchange: Exchange::Binance,
            symbol: "BTCUSDT".into(),
            ..Market::default()
        };
        let _ = desktop.update(Message::Action(Action::ApplyIndicator {
            id: None,
            spec: IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            source: IndicatorSource {
                mode: SourceMode::Single,
                markets: vec![market.clone()],
            },
        }));
        let now = source_controller::now_ms();
        let time = now.div_euclid(300_000) * 300;
        let generation =
            desktop.source_requests[&(market.clone(), source_feeds::Kind::Cvd)].generation;
        let _ = desktop.update(Message::SourceData(source_feeds::Batch {
            scope: Arc::new(source_feeds::Scope {
                market,
                generation,
                kind: source_feeds::Kind::Cvd,
                base_asset: "BTC".into(),
            }),
            events: vec![
                source_feeds::Event::Metadata {
                    base_asset: "BTC".into(),
                },
                source_feeds::Event::Market(MarketEvent::Status {
                    connected: true,
                    message: String::new(),
                }),
                source_feeds::Event::Market(MarketEvent::Resync),
                source_feeds::Event::Market(MarketEvent::Flow {
                    bar: FlowBar {
                        time,
                        flow: VolumeFlow {
                            delta: Some(4.0),
                            volume: Some(8.0),
                            quality: FlowQuality::Exact,
                            covered_from: time,
                            through_ms: now,
                        },
                    },
                }),
            ]
            .into_boxed_slice(),
        }));
        let instance = &desktop.chart.indicators.instances[0];
        assert!(instance.external.as_ref().unwrap().complete);
        assert_eq!(instance.points.last().unwrap().values[0], Some(4.0));
    }

    #[test]
    fn direct_interval_uses_active_catalog_after_cancelled_foreign_search() {
        use mexc_model::{Instrument, OpenInterestTimestamp};
        let (mut desktop, _) = Desktop::new();
        let market = desktop.chart.market.clone();
        let catalog = Arc::new(Catalog {
            exchange: market.exchange,
            product: market.product,
            intervals: vec![Timeframe::Min5, Timeframe::Hour8],
            flow_support: flow_support(),
            instruments: vec![Instrument {
                exchange: market.exchange,
                product: market.product,
                symbol: market.symbol.clone(),
                base_coin: "BTC".into(),
                quote_coin: Quote::USDC,
                settle_coin: "USDC".into(),
                price_scale: 2,
                contract_size: 0.0001,
                inverse: false,
                volume_unit: "contracts".into(),
            }],
        });
        let _ = desktop.update(Message::Catalog {
            exchange: market.exchange,
            product: market.product,
            generation: desktop.catalog_generation,
            result: Ok(catalog),
        });
        desktop.chart.merge_page(&page(300, None), 0);
        let sample = OpenInterestSample {
            timestamp_ms: 312_345,
            timestamp_source: OpenInterestTimestamp::Observed,
            quantity: 7.5,
            interval_ms: 5000,
        };
        desktop.chart.open_interest.samples.push(sample);
        let _ = desktop.update(Message::Action(Action::AddIndicator(
            IndicatorSpec::OpenInterest,
        )));
        let id = desktop.chart.indicators.instances[0].id;
        // Cancelled search belongs to another venue/product. It must not supply
        // intervals or metadata for the still-active price market.
        desktop.selector.exchange = Exchange::Okx;
        desktop.selector.product = Product::Spot;
        desktop.selector.set_catalog(
            Exchange::Okx,
            Product::Spot,
            Ok(Arc::new(Catalog {
                exchange: Exchange::Okx,
                product: Product::Spot,
                intervals: vec![Timeframe::Day1],
                flow_support: flow_support(),
                instruments: Vec::new(),
            })),
        );
        let before = pipeline_snapshot(&desktop);
        let _ = desktop.update(Message::SelectInterval(Timeframe::Day1));
        assert_eq!(pipeline_snapshot(&desktop), before);
        assert_eq!(desktop.chart.open_interest.samples, vec![sample]);
        let _ = desktop.update(Message::SelectInterval(Timeframe::Hour8));
        assert_eq!(
            desktop.chart.market,
            Market {
                interval: Timeframe::Hour8,
                ..market
            }
        );
        assert_eq!(desktop.chart.base_asset, "BTC");
        assert_eq!(desktop.native_quote, "USDC");
        assert_eq!(desktop.chart.open_interest.samples, vec![sample]);
        assert_eq!(desktop.chart.indicators.instances[0].id, id);
        assert_eq!(
            desktop.chart.indicators.instances[0].spec,
            IndicatorSpec::OpenInterest
        );
        assert_eq!(desktop.chart.len(), 0);
    }

    #[test]
    fn dialogs_block_navigation_without_stopping_native_data() {
        let (mut desktop, _) = Desktop::new();
        let market = desktop.chart.market.clone();
        let generation = desktop.market_generation;
        desktop
            .chart
            .merge_page(&page(300, None), desktop.chart.epoch);
        desktop.chart.dispatch(Action::Pan {
            time: -0.12,
            price: 0.12,
        });
        let viewport = desktop.chart.viewport;
        for dialog in 0..3 {
            desktop.manager.open = dialog == 0;
            desktop.selector.open = dialog == 1;
            desktop.details = (dialog == 2).then_some(Details::Status);
            for action in [
                Action::Live,
                Action::Reset,
                Action::Pan {
                    time: 0.5,
                    price: 0.5,
                },
                Action::Inspect(Some((300.0, 1.0))),
            ] {
                let _ = desktop.update(Message::Action(action));
            }
            let current = desktop.chart.viewport;
            assert_eq!(
                (
                    current.left,
                    current.span,
                    current.low,
                    current.high,
                    current.auto_fit,
                    current.live
                ),
                (
                    viewport.left,
                    viewport.span,
                    viewport.low,
                    viewport.high,
                    viewport.auto_fit,
                    viewport.live
                )
            );
            assert_eq!(desktop.chart.inspection, None);
            let _ = desktop.update(data_message(
                &market,
                generation,
                MarketEvent::Candle {
                    candle: page(600 + dialog * 300, None).candles[0],
                },
            ));
            assert_eq!(desktop.chart.latest().unwrap().time, 600 + dialog * 300);
        }
        let _ = desktop.update(Message::DetailsClose);
        let _ = desktop.update(Message::Action(Action::Live));
        assert!(desktop.chart.viewport.live);
        assert_eq!(desktop.chart.viewport.low, viewport.low);
        assert_eq!(desktop.chart.viewport.high, viewport.high);
    }

    #[test]
    fn source_repair_changes_only_the_named_instance_and_releases_its_feed() {
        use mexc_indicators::{CvdReset, SourceMode};
        let (mut desktop, _) = Desktop::new();
        let cvd_market = Market {
            exchange: Exchange::Binance,
            symbol: "BTCUSDT".into(),
            ..Market::default()
        };
        let oi_market = Market {
            exchange: Exchange::Okx,
            symbol: "BTC-USDT-SWAP".into(),
            ..Market::default()
        };
        for (spec, market) in [
            (
                IndicatorSpec::Cvd {
                    reset: CvdReset::UtcDay,
                },
                cvd_market.clone(),
            ),
            (IndicatorSpec::OpenInterest, oi_market.clone()),
        ] {
            let _ = desktop.update(Message::Action(Action::ApplyIndicator {
                id: None,
                spec,
                source: IndicatorSource {
                    mode: SourceMode::Single,
                    markets: vec![market],
                },
            }));
        }
        let cvd = desktop.chart.indicators.instances[0].id;
        let other = desktop.chart.indicators.instances[1].source.clone();
        let oi_generation = desktop.source_requests
            [&(oi_market.clone(), source_feeds::Kind::OpenInterest)]
            .generation;
        desktop.chart.dispatch(Action::ToggleIndicator(cvd));
        // A native context change invalidates BTC contributors, never substitutes ETH.
        desktop.chart.base_asset = "ETH".into();
        desktop.chart.market.symbol = "ETH_USDC".into();
        desktop.sync_indicator_sources();
        let _ = desktop.update(Message::UseChartSource(cvd));
        let repaired = &desktop.chart.indicators.instances[0];
        assert_eq!(repaired.id, cvd);
        assert_eq!(
            repaired.spec,
            IndicatorSpec::Cvd {
                reset: CvdReset::UtcDay
            }
        );
        assert!(!repaired.visible);
        assert!(repaired.source.is_chart());
        assert_eq!(desktop.chart.market.symbol, "ETH_USDC");
        assert_eq!(desktop.chart.indicators.instances[1].source, other);
        assert!(
            !desktop
                .source_requests
                .contains_key(&(cvd_market, source_feeds::Kind::Cvd))
        );
        assert_eq!(
            desktop.source_requests[&(oi_market, source_feeds::Kind::OpenInterest)].generation,
            oi_generation
        );
    }

    #[test]
    fn all_context_change_rechecks_newly_eligible_exchanges() {
        use mexc_indicators::{CvdReset, SourceMode};
        use mexc_model::{FlowAvailability, FlowQuality, FlowSupport, Instrument, VolumeFlow};
        let (mut desktop, _) = Desktop::new();
        desktop.chart.market.interval = Timeframe::Hour8;
        let market = |exchange, symbol: &str| Market {
            exchange,
            symbol: symbol.into(),
            ..desktop.chart.market.clone()
        };
        let a = market(Exchange::Binance, "BTCUSDC");
        let b = market(Exchange::Mexc, "BTC_USDC");
        for exchange in Exchange::ALL {
            desktop.source_catalogs.insert(
                (exchange, Product::Perpetual),
                Ok(Arc::new(Catalog {
                    exchange,
                    product: Product::Perpetual,
                    intervals: if exchange == Exchange::Okx {
                        vec![Timeframe::Min5]
                    } else {
                        vec![Timeframe::Hour8, Timeframe::Min5]
                    },
                    flow_support: FlowSupport {
                        history: FlowAvailability::Exact,
                        live: FlowAvailability::Exact,
                        description: String::new(),
                    },
                    instruments: if exchange == Exchange::Coinbase {
                        Vec::new()
                    } else {
                        vec![Instrument {
                            exchange,
                            product: Product::Perpetual,
                            symbol: match exchange {
                                Exchange::Binance => "BTCUSDC",
                                Exchange::Mexc => "BTC_USDC",
                                _ => "BTC-USDC-SWAP",
                            }
                            .into(),
                            base_coin: "BTC".into(),
                            quote_coin: Quote::USDC,
                            settle_coin: "USDC".into(),
                            price_scale: 2,
                            contract_size: 1.0,
                            inverse: false,
                            volume_unit: "BTC".into(),
                        }]
                    },
                })),
            );
        }
        desktop.active_catalog = desktop
            .source_catalogs
            .get(&(Exchange::Mexc, Product::Perpetual))
            .unwrap()
            .as_ref()
            .ok()
            .cloned();
        let _ = desktop.update(Message::Action(Action::ApplyIndicator {
            id: None,
            spec: IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            source: IndicatorSource {
                mode: SourceMode::All,
                markets: vec![a.clone(), b.clone()],
            },
        }));
        let seed = |desktop: &mut Desktop| {
            for (mut market, delta) in [(a.clone(), 5.0), (b.clone(), 7.0)] {
                market.interval = desktop.chart.market.interval;
                let mut source = SourceData::new(market.clone(), "BTC".into());
                source.flow_connected = true;
                source.merge_flows(
                    &[FlowBar {
                        time: 0,
                        flow: VolumeFlow {
                            delta: Some(delta),
                            volume: Some(delta),
                            quality: FlowQuality::Exact,
                            covered_from: 0,
                            through_ms: 1_000,
                        },
                    }],
                    true,
                );
                desktop.source_inputs.insert(market, source);
            }
            desktop
                .chart
                .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        };
        seed(&mut desktop);
        assert!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .points
                .last()
                .unwrap()
                .values[0],
            Some(12.0)
        );
        let membership = desktop.chart.indicators.instances[0].source.clone();
        let _ = desktop.update(Message::SelectInterval(Timeframe::Min5));
        seed(&mut desktop);
        assert_eq!(desktop.chart.indicators.instances[0].source, membership);
        // OKX is now eligible, but it was never explicitly selected.
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert!(
            desktop.chart.indicators.instances[0]
                .points
                .iter()
                .all(|point| point.values[0].is_none())
        );
        assert!(
            desktop
                .source_problem(&desktop.chart.indicators.instances[0])
                .is_some()
        );
        let id = desktop.chart.indicators.instances[0].id;
        let okx = Market {
            exchange: Exchange::Okx,
            symbol: "BTC-USDC-SWAP".into(),
            ..desktop.chart.market.clone()
        };
        let mut repaired = membership.clone();
        repaired.markets.push(okx.clone());
        let _ = desktop.update(Message::Action(Action::ApplyIndicator {
            id: Some(id),
            spec: IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            source: repaired.clone(),
        }));
        let mut source = SourceData::new(okx.clone(), "BTC".into());
        source.flow_connected = true;
        source.merge_flows(
            &[FlowBar {
                time: 0,
                flow: VolumeFlow {
                    delta: Some(3.0),
                    volume: Some(3.0),
                    quality: FlowQuality::Exact,
                    covered_from: 0,
                    through_ms: 1_000,
                },
            }],
            true,
        );
        desktop.source_inputs.insert(okx, source);
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert_eq!(desktop.chart.indicators.instances[0].id, id);
        assert!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .points
                .last()
                .unwrap()
                .values[0],
            Some(15.0)
        );
        assert!(
            desktop
                .source_problem(&desktop.chart.indicators.instances[0])
                .is_none()
        );

        let _ = desktop.update(Message::SelectInterval(Timeframe::Hour8));
        seed(&mut desktop);
        // A now-ineligible explicit OKX choice is not silently dropped to recover a subset.
        assert_eq!(desktop.chart.indicators.instances[0].source, repaired);
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert!(
            desktop.chart.indicators.instances[0]
                .points
                .iter()
                .all(|point| point.values[0].is_none())
        );
    }

    #[test]
    fn all_discovery_fails_closed_and_rejects_superseded_catalog_callbacks() {
        use mexc_indicators::SourceMode;
        use mexc_model::{Instrument, OpenInterestTimestamp};
        let (mut desktop, _) = Desktop::new();
        let market = desktop.chart.market.clone();
        let native = Arc::new(Catalog {
            exchange: market.exchange,
            product: market.product,
            intervals: vec![Timeframe::Min5],
            flow_support: flow_support(),
            instruments: vec![Instrument {
                exchange: market.exchange,
                product: market.product,
                symbol: market.symbol.clone(),
                base_coin: "BTC".into(),
                quote_coin: Quote::USDC,
                settle_coin: "USDC".into(),
                price_scale: 2,
                contract_size: 1.0,
                inverse: false,
                volume_unit: "BTC".into(),
            }],
        });
        let empty = |exchange| {
            Arc::new(Catalog {
                exchange,
                product: Product::Perpetual,
                intervals: vec![Timeframe::Min5],
                flow_support: flow_support(),
                instruments: Vec::new(),
            })
        };
        let membership = IndicatorSource {
            mode: SourceMode::All,
            markets: vec![market.clone()],
        };
        let _ = desktop.update(Message::Action(Action::ApplyIndicator {
            id: None,
            spec: IndicatorSpec::OpenInterest,
            source: membership.clone(),
        }));
        assert!(!desktop.manager.open);
        let generation = desktop.source_catalog_generation;
        let mut input = SourceData::new(market.clone(), "BTC".into());
        input.oi_connected = true;
        input.oi_cadence_ms = Some(5_000);
        input.merge_oi(&[OpenInterestSample {
            timestamp_ms: 1_000,
            quantity: 5.0,
            interval_ms: 5_000,
            timestamp_source: OpenInterestTimestamp::Venue,
        }]);
        desktop.source_inputs.insert(market.clone(), input);
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        for exchange in [Exchange::Binance, Exchange::Okx, Exchange::Coinbase] {
            let _ = desktop.update(Message::SourceCatalog {
                exchange,
                product: Product::Perpetual,
                generation,
                result: if exchange == Exchange::Binance {
                    Err("discovery failed".into())
                } else {
                    Ok(empty(exchange))
                },
            });
        }
        // Native discovery fulfills the concurrent MEXC source request.
        let _ = desktop.update(Message::Catalog {
            exchange: market.exchange,
            product: market.product,
            generation: desktop.catalog_generation,
            result: Ok(Arc::clone(&native)),
        });
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        let _ = desktop.update(Message::Manager(indicator_manager::Event::SourceRetry));
        let current_generation = desktop.source_catalog_generation;
        assert_ne!(generation, current_generation);
        let _ = desktop.update(Message::SourceCatalog {
            exchange: Exchange::Binance,
            product: Product::Perpetual,
            generation,
            result: Ok(empty(Exchange::Binance)),
        });
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        let _ = desktop.update(Message::SourceCatalog {
            exchange: Exchange::Binance,
            product: Product::Perpetual,
            generation: current_generation,
            result: Ok(empty(Exchange::Binance)),
        });
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .oi
                .last()
                .unwrap()
                .quantity,
            5.0
        );
        assert_eq!(desktop.chart.indicators.instances[0].source, membership);
        // Both redundant source completion and stale native discovery must leave accepted coverage intact.
        let _ = desktop.update(Message::SourceCatalog {
            exchange: market.exchange,
            product: market.product,
            generation: current_generation,
            result: Err("redundant old request".into()),
        });
        let _ = desktop.update(Message::Catalog {
            exchange: market.exchange,
            product: market.product,
            generation: desktop.catalog_generation.wrapping_sub(1),
            result: Ok(empty(market.exchange)),
        });
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .oi
                .last()
                .unwrap()
                .quantity,
            5.0
        );
        assert_eq!(desktop.chart.indicators.instances[0].source, membership);
    }

    #[test]
    fn drawing_transactions_leave_price_pipeline_scales_and_cvd_unchanged() {
        use crate::drawing_types::{Anchor, Anchors, Shape, Style};
        use mexc_indicators::CvdReset;
        let (mut desktop, _) = Desktop::new();
        let mut history = page(300, None);
        history.candles[0].flow = Some(mexc_model::VolumeFlow {
            delta: Some(1.25),
            volume: Some(3.0),
            quality: mexc_model::FlowQuality::Exact,
            covered_from: 300,
            through_ms: 600_000,
        });
        desktop.chart.merge_page(&history, 0);
        desktop
            .chart
            .dispatch(Action::AddIndicator(IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            }));
        desktop.chart.dispatch(Action::Pan {
            time: -0.1,
            price: 0.2,
        });
        let before = pipeline_snapshot(&desktop);
        let viewport = desktop.chart.viewport;
        let points = desktop.chart.indicators.instances[0].points[0].values;
        let origin = desktop.chart.indicators.instances[0]
            .cvd_context(0)
            .unwrap()
            .origin;
        let generation = desktop.drawings.generation();
        let _ = desktop.update(Message::DrawingCommand {
            generation,
            command: DrawingCommand::Create {
                shape: Shape::Horizontal(Anchor {
                    utc_ms: 300_000,
                    price: 1.0,
                }),
                style: Style::default(),
            },
        });
        let id = desktop.drawings.selected().unwrap();
        for command in [
            DrawingCommand::Move {
                id,
                anchors: Anchors::One(Anchor {
                    utc_ms: 300_000,
                    price: 1.25,
                }),
            },
            DrawingCommand::SetLocked { id, locked: true },
            DrawingCommand::Undo,
        ] {
            let _ = desktop.update(Message::DrawingCommand {
                generation,
                command,
            });
        }
        assert_eq!(
            desktop.drawings.find(id).unwrap().shape,
            Shape::Horizontal(Anchor {
                utc_ms: 300_000,
                price: 1.25
            })
        );
        assert_eq!(pipeline_snapshot(&desktop), before);
        let after = desktop.chart.viewport;
        assert_eq!(
            (
                after.left,
                after.span,
                after.low,
                after.high,
                after.live,
                after.auto_fit
            ),
            (
                viewport.left,
                viewport.span,
                viewport.low,
                viewport.high,
                viewport.live,
                viewport.auto_fit
            )
        );
        assert_eq!(
            desktop.chart.indicators.instances[0].points[0].values,
            points
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .cvd_context(0)
                .unwrap()
                .origin,
            origin
        );
    }

    #[test]
    fn modal_cancels_drawing_transaction_and_rejects_queued_release_while_prices_advance() {
        use crate::drawing_types::{Anchor, Hit, Part, Shape, Style};
        let (mut desktop, _) = Desktop::new();
        desktop.chart.merge_page(&page(300, None), 0);
        let anchor = Anchor {
            utc_ms: 300_000,
            price: 1.0,
        };
        desktop
            .drawings
            .apply(DrawingCommand::Create {
                shape: Shape::Horizontal(anchor),
                style: Style::default(),
            })
            .unwrap();
        let id = desktop.drawings.selected().unwrap();
        let projection = drawing_geometry::Projection::new(
            desktop.chart.viewport,
            iced::Rectangle::with_size(iced::Size::new(500.0, 300.0)),
        )
        .unwrap();
        let start = projection.point(anchor);
        let end = iced::Point::new(start.x, start.y - 30.0);
        let generation = desktop.drawing_interaction.generation();
        let _ = desktop.update(Message::DrawingInput {
            generation,
            event: drawing_interaction::Input::Down {
                point: start,
                projection,
                hit: Some(Hit {
                    id,
                    part: Part::Body,
                }),
                snap: None,
            },
        });
        let _ = desktop.update(Message::DrawingInput {
            generation,
            event: drawing_interaction::Input::Move {
                point: end,
                projection,
                hit: None,
                snap: None,
            },
        });
        assert!(desktop.drawing_interaction.is_dragging());
        let revision = desktop.drawings.revision();
        let _ = desktop.update(Message::DrawingMenu);
        assert!(!desktop.drawing_interaction.is_dragging());
        let market = desktop.chart.market.clone();
        let _ = desktop.update(data_message(
            &market,
            desktop.market_generation,
            MarketEvent::Candle {
                candle: page(600, None).candles[0],
            },
        ));
        assert_eq!(desktop.chart.latest().unwrap().time, 600);
        for closed in [false, true] {
            if closed {
                let _ = desktop.update(Message::DrawingClose);
            }
            let _ = desktop.update(Message::DrawingInput {
                generation,
                event: drawing_interaction::Input::Up {
                    point: end,
                    projection,
                    snap: None,
                },
            });
            assert_eq!(
                desktop.drawings.find(id).unwrap().shape,
                Shape::Horizontal(anchor)
            );
            assert_eq!(desktop.drawings.revision(), revision);
        }
    }

    #[test]
    fn drawing_modal_owns_captured_menu_shortcuts_without_reinterpreting_properties_text() {
        use crate::drawing_types::{Anchor, Shape, Style};
        use iced::keyboard::{Key, key::Named};
        let (mut desktop, _) = Desktop::new();
        desktop.chart.merge_page(&page(300, None), 0);
        let previous = Shape::Horizontal(Anchor {
            utc_ms: 300_000,
            price: 0.875,
        });
        desktop
            .drawings
            .apply(DrawingCommand::Create {
                shape: previous.clone(),
                style: Style::default(),
            })
            .unwrap();
        let previous_id = desktop.drawings.selected().unwrap();
        let before = pipeline_snapshot(&desktop);
        let _ = desktop.update(Message::DrawingMenu);
        let _ = desktop.update(Message::Key {
            key: Key::Character("1".into()),
            modifiers: keyboard::Modifiers::default(),
            captured: true,
        });
        assert_eq!(desktop.drawings.selected(), None);
        assert_eq!(desktop.drawings.find(previous_id).unwrap().shape, previous);
        let generation = desktop.drawing_interaction.generation();
        let anchor = Anchor {
            utc_ms: 300_000,
            price: 1.125,
        };
        let _ = desktop.update(Message::DrawingInput {
            generation,
            event: drawing_interaction::Input::Key {
                key: Key::Named(Named::Enter),
                modifiers: keyboard::Modifiers::default(),
                seed: Some(anchor),
                step_ms: 300_000,
                price_step: 0.01,
            },
        });
        let id = desktop.drawings.selected().unwrap();
        assert_eq!(
            desktop.drawings.find(id).unwrap().shape,
            Shape::Horizontal(anchor)
        );
        assert_eq!(pipeline_snapshot(&desktop), before);
        let _ = desktop.update(Message::DrawingProperties(id));
        let _ = desktop.update(Message::DrawingControl(drawing_controls::Event::Field(
            drawing_controls::Field::PriceFirst,
            "2.25".into(),
        )));
        let _ = desktop.update(Message::Key {
            key: Key::Character("7".into()),
            modifiers: keyboard::Modifiers::default(),
            captured: true,
        });
        let _ = desktop.update(Message::Key {
            key: Key::Named(Named::Enter),
            modifiers: keyboard::Modifiers::CTRL,
            captured: true,
        });
        assert_eq!(
            desktop.drawings.find(id).unwrap().shape,
            Shape::Horizontal(Anchor {
                price: 2.25,
                ..anchor
            })
        );
        assert!(!desktop.drawing_controls.is_open());
    }
}

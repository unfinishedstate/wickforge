use crate::{
    Message,
    source_controller::{eligible_source_instruments, validated_source_catalog},
    state::{Action, ChartState},
};
use iced::widget::{
    button, checkbox, column, container, pick_list, radio, row, scrollable, text, text_input,
};
use iced::{Element, Length, Task, keyboard};
use mexc_indicators::{CvdReset, IndicatorKind, IndicatorSource, IndicatorSpec, SourceMode};
use mexc_model::{Catalog, Exchange, Instrument, Market, Product};
use std::sync::Arc;

const INSTANCES_ID: &str = "indicator-editor-instances";
const INSTANCE_HEIGHT: f32 = 54.0;
const INSTANCE_SPACING: f32 = 5.0;
const SOURCE_EDITOR_ID: &str = "indicator-editor-parameters";
const SOURCE_CANDIDATES_ID: &str = "indicator-source-candidates";
const SOURCE_ROW_HEIGHT: f32 = 72.0;
const SOURCE_ROW_SPACING: f32 = 4.0;
const SOURCE_VISIBLE_ROWS: usize = 3;

#[derive(Debug, Clone)]
pub enum Event {
    Kind(IndicatorKind),
    Field(usize, String),
    Reset(CvdReset),
    Select(u64),
    New,
    Submit,
    Toggle,
    Remove,
    SourceMode(SourceMode),
    SourceExchange(Exchange),
    SourceVenueIncluded { exchange: Exchange, included: bool },
    SetSourceMarket { market: Market, chosen: bool },
    StepSourceCandidate(i32),
    ToggleSourceCandidate,
    SourceRetry,
}

pub struct Manager {
    pub open: bool,
    selected: Option<u64>,
    kind: IndicatorKind,
    fields: [String; 3],
    cvd_reset: CvdReset,
    field_ids: [text_input::Id; 3],
    error: Option<String>,
    source: IndicatorSource,
    source_exchange: Exchange,
    source_candidate: Option<String>,
    source_product: Product,
    source_catalogs: [Option<Result<Arc<Catalog>, String>>; 4],
}

impl Default for Manager {
    fn default() -> Self {
        let mut manager = Self {
            open: false,
            selected: None,
            kind: IndicatorKind::Sma,
            fields: Default::default(),
            cvd_reset: CvdReset::Continuous,
            field_ids: [
                text_input::Id::new("indicator-parameter-0"),
                text_input::Id::new("indicator-parameter-1"),
                text_input::Id::new("indicator-parameter-2"),
            ],
            error: None,
            source: IndicatorSource::default(),
            source_exchange: Exchange::default(),
            source_candidate: None,
            source_product: Product::default(),
            source_catalogs: Default::default(),
        };
        manager.load(IndicatorKind::Sma.default_spec());
        manager
    }
}

impl Manager {
    fn load(&mut self, spec: IndicatorSpec) {
        self.kind = spec.kind();
        self.fields = match spec {
            IndicatorSpec::Sma { period }
            | IndicatorSpec::Ema { period }
            | IndicatorSpec::Rsi { period }
            | IndicatorSpec::Atr { period } => [period.to_string(), String::new(), String::new()],
            IndicatorSpec::Bollinger { period, deviations } => {
                [period.to_string(), deviations.to_string(), String::new()]
            }
            IndicatorSpec::Macd { fast, slow, signal } => {
                [fast.to_string(), slow.to_string(), signal.to_string()]
            }
            IndicatorSpec::Stochastic {
                period,
                smooth_k,
                smooth_d,
            } => [
                period.to_string(),
                smooth_k.to_string(),
                smooth_d.to_string(),
            ],
            IndicatorSpec::Cvd { reset } => {
                self.cvd_reset = reset;
                Default::default()
            }
            IndicatorSpec::OpenInterest => Default::default(),
        };
        self.error = None;
    }

    pub fn accept_source_catalog(
        &mut self,
        exchange: Exchange,
        product: Product,
        result: Result<Arc<Catalog>, String>,
    ) {
        if product != self.source_product {
            return;
        }
        self.source_catalogs[exchange.index()] =
            Some(validated_source_catalog(exchange, product, result));
    }

    pub fn reset_source_catalogs(&mut self, product: Product) {
        self.source_product = product;
        self.source_catalogs = Default::default();
        self.source_candidate = None;
    }

    pub fn source_catalogs_needed(&self) -> bool {
        self.source_catalogs.iter().any(Option::is_none)
    }

    pub fn reject_source(&mut self, message: String) {
        self.error = Some(message);
    }

    fn supports_source(&self) -> bool {
        matches!(self.kind, IndicatorKind::Cvd | IndicatorKind::OpenInterest)
    }

    fn eligible_candidates<'a>(
        &'a self,
        exchange: Exchange,
        chart: &'a ChartState,
    ) -> impl Iterator<Item = &'a Instrument> {
        self.source_catalogs[exchange.index()]
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .into_iter()
            .flat_map(move |catalog| {
                eligible_source_instruments(
                    catalog,
                    self.kind,
                    chart.market.product,
                    chart.market.interval,
                    &chart.base_asset,
                )
            })
    }

    fn candidates<'a>(&'a self, exchange: Exchange, chart: &'a ChartState) -> Vec<&'a Instrument> {
        let mut items: Vec<_> = self.eligible_candidates(exchange, chart).collect();
        items.sort_unstable_by(|a, b| a.symbol.cmp(&b.symbol));
        items
    }

    fn eligible_market(&self, market: &Market, chart: &ChartState) -> bool {
        market.product == chart.market.product
            && self
                .eligible_candidates(market.exchange, chart)
                .any(|item| item.symbol == market.symbol)
    }

    fn set_source_market(&mut self, market: Market, chosen: bool, chart: &ChartState) {
        if !self.supports_source()
            || self.source.is_chart()
            || chosen && !self.eligible_market(&market, chart)
        {
            return;
        }
        self.source_exchange = market.exchange;
        self.source_candidate = Some(market.symbol.clone());
        if chosen {
            if self.source.mode == SourceMode::Single {
                self.source.markets.clear();
            }
            if !self.selected_market(&market) {
                self.source.markets.push(Market {
                    interval: chart.market.interval,
                    ..market
                });
            }
        } else {
            self.source.markets.retain(|item| {
                !(item.exchange == market.exchange
                    && item.product == market.product
                    && item.symbol == market.symbol)
            });
        }
    }

    fn source_label(item: &Instrument) -> String {
        format!(
            "{} · quote {} · settle {} · {}",
            item.symbol,
            item.quote_coin,
            item.settle_coin,
            if item.inverse {
                "inverse"
            } else {
                "linear / spot"
            }
        )
    }

    fn selected_market(&self, market: &Market) -> bool {
        self.source.markets.iter().any(|item| {
            item.exchange == market.exchange
                && item.product == market.product
                && item.symbol == market.symbol
        })
    }

    fn validated_source(&self, chart: &ChartState) -> Result<IndicatorSource, String> {
        if !self.supports_source() || self.source.is_chart() {
            return Ok(IndicatorSource::default());
        }
        if self.source.mode == SourceMode::Single && self.source.markets.len() != 1 {
            return Err("Single requires exactly one explicit native market".into());
        }
        if self.source.markets.is_empty() {
            return Err("Choose at least one explicit native market".into());
        }
        for market in &self.source.markets {
            if !self.eligible_market(market, chart) {
                return Err(format!(
                    "{} {} is unavailable or mismatches {} {} / {}",
                    market.exchange,
                    market.symbol,
                    chart.base_asset,
                    chart.market.product,
                    chart.market.interval
                ));
            }
        }
        if self.source.mode == SourceMode::All {
            for exchange in Exchange::ALL {
                match &self.source_catalogs[exchange.index()] {
                    None => {
                        return Err(format!(
                            "All requires completed discovery: {exchange} pending"
                        ));
                    }
                    Some(Err(error)) => {
                        return Err(format!("All discovery failed for {exchange}: {error}"));
                    }
                    Some(Ok(_)) => {}
                }
                if self.eligible_candidates(exchange, chart).next().is_some()
                    && !self
                        .source
                        .markets
                        .iter()
                        .any(|market| market.exchange == exchange)
                {
                    return Err(format!(
                        "All requires an explicit native market on {exchange}"
                    ));
                }
            }
        }
        let mut source = self.source.clone();
        for market in &mut source.markets {
            market.interval = chart.market.interval;
        }
        Ok(source)
    }

    fn focus_field(&self, index: usize) -> Task<Message> {
        let id = self.field_ids[index].clone();
        Task::batch([text_input::focus(id.clone()), text_input::select_all(id)])
    }

    fn focus_first_field(&self) -> Task<Message> {
        if self.labels().is_empty() {
            Task::none()
        } else {
            self.focus_field(0)
        }
    }

    fn spec(&self) -> Result<IndicatorSpec, (usize, String)> {
        let period = |index: usize| {
            self.fields[index].trim().parse::<usize>().map_err(|_| {
                (
                    index,
                    format!("{} must be a whole number", self.labels()[index]),
                )
            })
        };
        let spec = match self.kind {
            IndicatorKind::Sma => IndicatorSpec::Sma { period: period(0)? },
            IndicatorKind::Ema => IndicatorSpec::Ema { period: period(0)? },
            IndicatorKind::Bollinger => IndicatorSpec::Bollinger {
                period: period(0)?,
                deviations: self.fields[1]
                    .trim()
                    .parse()
                    .map_err(|_| (1, "Deviations must be a number".into()))?,
            },
            IndicatorKind::Rsi => IndicatorSpec::Rsi { period: period(0)? },
            IndicatorKind::Macd => IndicatorSpec::Macd {
                fast: period(0)?,
                slow: period(1)?,
                signal: period(2)?,
            },
            IndicatorKind::Atr => IndicatorSpec::Atr { period: period(0)? },
            IndicatorKind::Stochastic => IndicatorSpec::Stochastic {
                period: period(0)?,
                smooth_k: period(1)?,
                smooth_d: period(2)?,
            },
            IndicatorKind::Cvd => IndicatorSpec::Cvd {
                reset: self.cvd_reset,
            },
            IndicatorKind::OpenInterest => IndicatorSpec::OpenInterest,
        };
        spec.validate().map_err(|error| {
            // Validation remains owned by IndicatorSpec. These names only
            // route focus to the field the user needs to fix.
            let field = match self.kind {
                IndicatorKind::Bollinger if error.starts_with("Deviations") => 1,
                IndicatorKind::Macd if error.starts_with("Signal") => 2,
                IndicatorKind::Macd if error.starts_with("Slow") || error.contains("less than") => {
                    1
                }
                IndicatorKind::Stochastic if error.starts_with("K smoothing") => 1,
                IndicatorKind::Stochastic if error.starts_with("D smoothing") => 2,
                _ => 0,
            };
            (field, error)
        })?;
        Ok(spec)
    }

    fn labels(&self) -> &'static [&'static str] {
        match self.kind {
            IndicatorKind::Bollinger => &["Period", "Deviations"],
            IndicatorKind::Macd => &["Fast", "Slow", "Signal"],
            IndicatorKind::Stochastic => &["Period", "Smooth K", "Smooth D"],
            IndicatorKind::Cvd | IndicatorKind::OpenInterest => &[],
            _ => &["Period"],
        }
    }

    pub fn selected(&self) -> Option<u64> {
        self.selected
    }

    pub fn show(&mut self) -> Task<Message> {
        self.open = true;
        self.focus_first_field()
    }

    pub fn edit(&mut self, id: u64, chart: &ChartState) -> Task<Message> {
        self.open = true;
        self.update(Event::Select(id), chart).1
    }

    pub fn prepare_removal(&mut self, id: u64, chart: &ChartState) -> Task<Message> {
        if self.selected == Some(id) {
            self.update(Event::Remove, chart).1
        } else {
            Task::none()
        }
    }

    fn follow_selection(&self, chart: &ChartState) -> Task<Message> {
        let index = self
            .selected
            .and_then(|id| {
                chart
                    .indicators
                    .instances
                    .iter()
                    .position(|item| item.id == id)
            })
            .unwrap_or(0);
        Self::scroll_instance(index)
    }

    fn scroll_instance(index: usize) -> Task<Message> {
        scrollable::scroll_to(
            scrollable::Id::new(INSTANCES_ID),
            scrollable::AbsoluteOffset {
                x: 0.0,
                y: index as f32 * (INSTANCE_HEIGHT + INSTANCE_SPACING),
            },
        )
    }

    /// Called by the controller after a successful Add or Configure action.
    pub fn applied(&mut self, chart: &ChartState) -> Task<Message> {
        if self.selected.is_none() {
            self.selected = chart
                .indicators
                .instances
                .last()
                .map(|instance| instance.id);
        }
        if let Some(instance) = chart
            .indicators
            .instances
            .iter()
            .find(|instance| Some(instance.id) == self.selected)
        {
            self.load(instance.spec);
            self.source = instance.source.clone();
        }
        Task::batch([self.focus_first_field(), self.follow_selection(chart)])
    }

    pub fn update(&mut self, event: Event, chart: &ChartState) -> (Option<Action>, Task<Message>) {
        self.error = None;
        match event {
            Event::Kind(kind) => {
                if kind != self.kind {
                    self.load(kind.default_spec());
                    if !self.supports_source() {
                        self.source = IndicatorSource::default();
                    }
                }
                (None, self.focus_first_field())
            }
            Event::Field(index, value) => {
                if index < self.labels().len() {
                    self.fields[index] = value;
                }
                (None, Task::none())
            }
            Event::Reset(reset) => {
                if self.kind == IndicatorKind::Cvd {
                    self.cvd_reset = reset;
                }
                (None, Task::none())
            }
            Event::Select(id) => {
                if self.selected == Some(id) {
                    return (
                        None,
                        Task::batch([self.focus_first_field(), self.follow_selection(chart)]),
                    );
                }
                if let Some(instance) = chart.indicators.instances.iter().find(|item| item.id == id)
                {
                    self.selected = Some(id);
                    self.load(instance.spec);
                    self.source = instance.source.clone();
                    (
                        None,
                        Task::batch([self.focus_first_field(), self.follow_selection(chart)]),
                    )
                } else {
                    (None, Task::none())
                }
            }
            Event::New => {
                self.selected = None;
                self.load(self.kind.default_spec());
                self.source = IndicatorSource::default();
                (None, self.focus_first_field())
            }
            Event::Submit => match self.spec() {
                Ok(spec) => match self.validated_source(chart) {
                    Ok(source) => {
                        let action = match self.selected {
                            None if source.is_chart() => Action::AddIndicator(spec),
                            Some(id)
                                if source.is_chart()
                                    && chart.indicators.instances.iter().any(|instance| {
                                        instance.id == id && instance.source.is_chart()
                                    }) =>
                            {
                                Action::ConfigureIndicator { id, spec }
                            }
                            id => Action::ApplyIndicator { id, spec, source },
                        };
                        (Some(action), Task::none())
                    }
                    Err(error) => {
                        self.error = Some(error);
                        (None, Task::none())
                    }
                },
                Err((field, error)) => {
                    self.error = Some(error);
                    (None, self.focus_field(field))
                }
            },
            Event::Toggle => (self.selected.map(Action::ToggleIndicator), Task::none()),
            Event::Remove => {
                if let Some(id) = self.selected {
                    let items = &chart.indicators.instances;
                    let index = items.iter().position(|item| item.id == id).unwrap_or(0);
                    let next = items
                        .get(index + 1)
                        .or_else(|| index.checked_sub(1).and_then(|index| items.get(index)));
                    self.selected = next.map(|item| item.id);
                    if let Some(next) = next {
                        self.load(next.spec);
                        self.source = next.source.clone();
                    } else {
                        self.load(self.kind.default_spec());
                        self.source = IndicatorSource::default();
                        self.source_candidate = None;
                    }
                    (
                        Some(Action::RemoveIndicator(id)),
                        Task::batch([
                            self.focus_first_field(),
                            Self::scroll_instance(index.min(items.len().saturating_sub(2))),
                        ]),
                    )
                } else {
                    (None, Task::none())
                }
            }
            Event::SourceMode(mode) => {
                if self.supports_source() {
                    self.source.mode = mode;
                    if mode == SourceMode::Chart {
                        self.source.markets.clear();
                    }
                }
                (None, Task::none())
            }
            Event::SourceExchange(exchange) => {
                self.source_exchange = exchange;
                self.source_candidate = None;
                (None, Task::none())
            }
            Event::SourceVenueIncluded { exchange, included } => {
                self.source_exchange = exchange;
                self.source_candidate = None;
                if !included {
                    self.source
                        .markets
                        .retain(|market| market.exchange != exchange);
                }
                (None, Task::none())
            }
            Event::SetSourceMarket { market, chosen } => {
                self.set_source_market(market, chosen, chart);
                (None, Task::none())
            }
            Event::StepSourceCandidate(direction) => {
                if self.source.is_chart() {
                    return (None, Task::none());
                }
                let candidates = self.candidates(self.source_exchange, chart);
                if !candidates.is_empty() {
                    let index = self.source_candidate.as_ref().and_then(|symbol| {
                        candidates.iter().position(|item| &item.symbol == symbol)
                    });
                    let next = match (index, direction < 0) {
                        (Some(index), true) => index.saturating_sub(1),
                        (Some(index), false) => (index + 1).min(candidates.len() - 1),
                        (None, _) => 0,
                    };
                    let task = if candidates.len() > SOURCE_VISIBLE_ROWS {
                        scrollable::scroll_to(
                            scrollable::Id::new(SOURCE_CANDIDATES_ID),
                            scrollable::AbsoluteOffset {
                                x: 0.0,
                                y: next as f32 * (SOURCE_ROW_HEIGHT + SOURCE_ROW_SPACING),
                            },
                        )
                    } else {
                        Task::none()
                    };
                    self.source_candidate = Some(candidates[next].symbol.clone());
                    return (
                        None,
                        Task::batch([
                            scrollable::snap_to(
                                scrollable::Id::new(SOURCE_EDITOR_ID),
                                scrollable::RelativeOffset { x: 0.0, y: 1.0 },
                            ),
                            task,
                        ]),
                    );
                }
                (None, Task::none())
            }
            Event::ToggleSourceCandidate => {
                if let Some(symbol) = &self.source_candidate {
                    let market = Market {
                        exchange: self.source_exchange,
                        product: chart.market.product,
                        symbol: symbol.clone(),
                        interval: chart.market.interval,
                    };
                    let chosen =
                        self.source.mode == SourceMode::Single || !self.selected_market(&market);
                    self.set_source_market(market, chosen, chart);
                }
                (None, Task::none())
            }
            Event::SourceRetry => {
                for slot in &mut self.source_catalogs {
                    if slot.as_ref().is_some_and(Result::is_err) {
                        *slot = None;
                    }
                }
                (None, Task::none())
            }
        }
    }

    pub fn key(
        &self,
        key: keyboard::Key<&str>,
        modifiers: keyboard::Modifiers,
        captured: bool,
        chart: &ChartState,
    ) -> Option<Event> {
        use keyboard::{Key, key::Named};
        if self.supports_source() && modifiers.control() && modifiers.shift() {
            return match key {
                Key::Character("r" | "R") if self.kind == IndicatorKind::Cvd => {
                    Some(Event::Reset(match self.cvd_reset {
                        CvdReset::Continuous => CvdReset::UtcDay,
                        CvdReset::UtcDay => CvdReset::Continuous,
                    }))
                }
                Key::Character("m" | "M") => {
                    let index = SourceMode::ALL
                        .iter()
                        .position(|mode| *mode == self.source.mode)
                        .unwrap_or(0);
                    Some(Event::SourceMode(
                        SourceMode::ALL[(index + 1) % SourceMode::ALL.len()],
                    ))
                }
                Key::Named(Named::ArrowLeft | Named::ArrowRight) => {
                    let step = if matches!(key, Key::Named(Named::ArrowLeft)) {
                        3
                    } else {
                        1
                    };
                    Some(Event::SourceExchange(
                        Exchange::ALL[(self.source_exchange.index() + step) % 4],
                    ))
                }
                Key::Named(Named::ArrowUp) => Some(Event::StepSourceCandidate(-1)),
                Key::Named(Named::ArrowDown) => Some(Event::StepSourceCandidate(1)),
                Key::Named(Named::Space) => Some(Event::ToggleSourceCandidate),
                _ => None,
            };
        }
        // Control shortcuts remain usable while a text field owns focus. Plain
        // keys never steal characters or cursor movement from parameter inputs.
        if modifiers.control() {
            match key {
                Key::Character("n" | "N") => return Some(Event::New),
                Key::Character("s" | "S") => return Some(Event::Submit),
                Key::Character("h" | "H") => return Some(Event::Toggle),
                Key::Character("d" | "D") => return Some(Event::Remove),
                Key::Character("r" | "R") if self.supports_source() => {
                    return Some(Event::SourceRetry);
                }
                _ => {}
            }
        }
        if captured && !modifiers.control() {
            return None;
        }
        match key {
            Key::Named(Named::ArrowUp | Named::ArrowDown) => {
                let items = &chart.indicators.instances;
                if items.is_empty() {
                    return None;
                }
                let current = self
                    .selected
                    .and_then(|id| items.iter().position(|item| item.id == id));
                let index = match (current, key) {
                    (Some(index), Key::Named(Named::ArrowUp)) => index.saturating_sub(1),
                    (Some(index), _) => (index + 1).min(items.len() - 1),
                    (None, _) => 0,
                };
                Some(Event::Select(items[index].id))
            }
            Key::Named(Named::ArrowLeft | Named::ArrowRight) => {
                let index = IndicatorKind::ALL
                    .iter()
                    .position(|kind| *kind == self.kind)
                    .unwrap_or(0);
                let count = IndicatorKind::ALL.len();
                let next = if matches!(key, Key::Named(Named::ArrowLeft)) {
                    (index + count - 1) % count
                } else {
                    (index + 1) % count
                };
                Some(Event::Kind(IndicatorKind::ALL[next]))
            }
            Key::Named(Named::Enter) => Some(Event::Submit),
            Key::Named(Named::Delete) => Some(Event::Remove),
            Key::Named(Named::Space) => Some(Event::Toggle),
            Key::Character("n" | "N") => Some(Event::New),
            _ => None,
        }
    }

    pub fn view<'a>(&'a self, chart: &'a ChartState) -> Element<'a, Message> {
        iced::widget::responsive(move |size| self.dialog_view(chart, size.width >= 600.0)).into()
    }

    fn dialog_view<'a>(
        &'a self,
        chart: &'a ChartState,
        side_by_side: bool,
    ) -> Element<'a, Message> {
        let header = column![
            row![
                text("Indicators").size(20),
                iced::widget::Space::new(Length::Fill, Length::Shrink),
                button("Keys [F1]").on_press(Message::HelpOpen)
            ]
            .align_y(iced::Alignment::Center),
            text(
                self.selected
                    .map_or_else(|| "New instance".to_owned(), |id| format!("Editing #{id}"))
            )
            .size(14),
        ]
        .spacing(4);
        let mut editor = column![
            text("Parameters").size(14),
            pick_list(
                IndicatorKind::ALL,
                Some(self.kind),
                |kind| Message::Manager(Event::Kind(kind))
            )
            .width(Length::Fill),
        ]
        .spacing(8);
        for (index, label) in self.labels().iter().enumerate() {
            editor = editor.push(
                column![
                    text(*label).size(13),
                    text_input(label, &self.fields[index])
                        .id(self.field_ids[index].clone())
                        .on_input(move |value| Message::Manager(Event::Field(index, value)))
                        .on_submit(Message::Manager(Event::Submit))
                        .padding(5),
                ]
                .spacing(6)
                .width(Length::Fill),
            );
        }
        if self.kind == IndicatorKind::Cvd {
            editor = editor
                .push(text("Reset").size(13))
                .push(
                    pick_list(
                        [CvdReset::Continuous, CvdReset::UtcDay],
                        Some(self.cvd_reset),
                        |reset| Message::Manager(Event::Reset(reset)),
                    )
                    .width(Length::Fill),
                )
                .push(
                    text("UTC day resets at 00:00 UTC. A complete daily total requires trade coverage from UTC midnight.")
                        .size(12),
                );
        }
        if self.kind == IndicatorKind::OpenInterest {
            editor = editor.push(
                text("Chart: native quantity · external sources: base asset\nPerpetual observations, not candle-aligned or derived notional.").size(13),
            );
        }
        if self.supports_source() {
            let mut modes = row![].spacing(10);
            for mode in SourceMode::ALL {
                modes = modes.push(
                    radio(
                        match mode {
                            SourceMode::Chart => "Chart",
                            SourceMode::Single => "Single",
                            SourceMode::Selected => "Selected",
                            SourceMode::All => "All",
                        },
                        mode,
                        Some(self.source.mode),
                        |mode| Message::Manager(Event::SourceMode(mode)),
                    )
                    .size(16)
                    .text_size(13),
                );
            }
            editor = editor
                .push(text("Source · chart stays independent").size(14))
                .push(modes.wrap());
            if !self.source.is_chart() {
                editor = editor
                    .push(
                        text(format!(
                            "{} {} · base-asset quantity · every source required",
                            chart.base_asset, chart.market.product
                        ))
                        .size(12),
                    )
                    .push(
                        text(if self.source.mode == SourceMode::Single {
                            "Single: choosing a native market replaces all other draft choices."
                        } else {
                            "Selected / All: check each exact native market to include it."
                        })
                        .size(12),
                    )
                    .push(
                        text("Exchanges · browse first, then choose exact native markets").size(13),
                    );
                for exchange in Exchange::ALL {
                    let chosen = self
                        .source
                        .markets
                        .iter()
                        .filter(|market| market.exchange == exchange)
                        .count();
                    let valid_chosen = self
                        .source
                        .markets
                        .iter()
                        .filter(|market| {
                            market.exchange == exchange && self.eligible_market(market, chart)
                        })
                        .count();
                    let mut status = match &self.source_catalogs[exchange.index()] {
                        None => "discovery pending".to_owned(),
                        Some(Err(error)) => format!("discovery failed: {error}"),
                        Some(Ok(_)) => {
                            let count = self.eligible_candidates(exchange, chart).count();
                            if count == 0 {
                                "ineligible for this underlying/product/interval".into()
                            } else {
                                format!("{count} eligible native markets")
                            }
                        }
                    };
                    status.push_str(&format!(" · {chosen} chosen"));
                    if chosen > valid_chosen {
                        status.push_str(&format!(" · {} unavailable", chosen - valid_chosen));
                    }
                    if self.source.mode == SourceMode::All {
                        status.push_str(match &self.source_catalogs[exchange.index()] {
                            None => " · All pending",
                            Some(Err(_)) => " · All failed: retry required",
                            Some(Ok(_))
                                if self.eligible_candidates(exchange, chart).next().is_none() =>
                            {
                                " · All not required"
                            }
                            Some(Ok(_)) if valid_chosen == 0 => " · All required: MISSING",
                            Some(Ok(_)) => " · All required: chosen",
                        });
                    }
                    let mut venue = row![
                        checkbox(format!("{exchange}: {status}"), chosen > 0)
                            .on_toggle(move |included| Message::Manager(
                                Event::SourceVenueIncluded { exchange, included }
                            ))
                            .size(16)
                            .text_size(12)
                            .width(Length::Fill),
                        button(if exchange == self.source_exchange {
                            "Browsing"
                        } else {
                            "Browse"
                        })
                        .on_press(Message::Manager(Event::SourceExchange(exchange)))
                        .style(if exchange == self.source_exchange {
                            button::primary
                        } else {
                            button::secondary
                        }),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center);
                    if matches!(&self.source_catalogs[exchange.index()], Some(Err(_))) {
                        venue = venue
                            .push(button("Retry").on_press(Message::Manager(Event::SourceRetry)));
                    }
                    editor = editor.push(venue);
                }
                if let Some(Ok(catalog)) = &self.source_catalogs[Exchange::Mexc.index()]
                    && catalog
                        .instruments
                        .iter()
                        .any(|item| item.base_coin == chart.base_asset && item.inverse)
                {
                    editor = editor.push(text("MEXC inverse contracts excluded: no authoritative base-quantity conversion. Choose a listed linear native market.").size(12));
                }
                for market in &self.source.markets {
                    if self.eligible_market(market, chart) {
                        continue;
                    }
                    let label = self.source_catalogs[market.exchange.index()]
                        .as_ref()
                        .and_then(|result| result.as_ref().ok())
                        .and_then(|catalog| {
                            catalog.instruments.iter().find(|item| {
                                item.product == market.product && item.symbol == market.symbol
                            })
                        })
                        .map_or_else(
                            || {
                                format!(
                                    "{} · {} · native metadata unavailable",
                                    market.symbol, market.product
                                )
                            },
                            Self::source_label,
                        );
                    editor = editor.push(
                        checkbox(
                            format!(
                                "{} {label} · unavailable: uncheck to remove",
                                market.exchange
                            ),
                            true,
                        )
                        .on_toggle(move |chosen| {
                            Message::Manager(Event::SetSourceMarket {
                                market: market.clone(),
                                chosen,
                            })
                        })
                        .size(16)
                        .text_size(12)
                        .width(Length::Fill),
                    );
                }
                editor = editor
                    .push(text("Checks = included; > = keyboard cursor. Unchecking an exchange removes all its draft markets. All requires an exact choice on every eligible exchange.").size(12))
                    .push(text("Ctrl+Shift+Left/Right: browse venue · Up/Down: cursor · Space: choose/uncheck · M: source mode · Ctrl+R: retry discovery").size(12))
                    .push(text(format!("{} native markets", self.source_exchange)).size(13));
                let items = self.candidates(self.source_exchange, chart);
                let count = items.len();
                let mut candidates = column![].spacing(SOURCE_ROW_SPACING);
                for item in items {
                    let market = Market {
                        exchange: item.exchange,
                        product: item.product,
                        symbol: item.symbol.clone(),
                        interval: chart.market.interval,
                    };
                    let checked = self.selected_market(&market);
                    let highlighted =
                        self.source_candidate.as_deref() == Some(item.symbol.as_str());
                    let label = Self::source_label(item);
                    let control: Element<'a, Message> = if self.source.mode == SourceMode::Single {
                        radio(
                            label,
                            true,
                            (checked && self.source.markets.len() == 1).then_some(true),
                            move |_| {
                                Message::Manager(Event::SetSourceMarket {
                                    market: market.clone(),
                                    chosen: true,
                                })
                            },
                        )
                        .size(16)
                        .text_size(12)
                        .width(Length::Fill)
                        .into()
                    } else {
                        checkbox(label, checked)
                            .on_toggle(move |chosen| {
                                Message::Manager(Event::SetSourceMarket {
                                    market: market.clone(),
                                    chosen,
                                })
                            })
                            .size(16)
                            .text_size(12)
                            .width(Length::Fill)
                            .into()
                    };
                    candidates = candidates.push(
                        container(
                            row![
                                text(if highlighted { ">" } else { " " }).size(14).width(12),
                                control
                            ]
                            .spacing(4)
                            .align_y(iced::Alignment::Center),
                        )
                        .padding(4)
                        .height(SOURCE_ROW_HEIGHT)
                        .width(Length::Fill)
                        .style(move |theme| {
                            if highlighted {
                                container::Style {
                                    background: None,
                                    text_color: None,
                                    ..container::bordered_box(theme)
                                }
                            } else {
                                container::transparent(theme)
                            }
                        }),
                    );
                }
                if count > SOURCE_VISIBLE_ROWS {
                    editor = editor.push(
                        scrollable(candidates)
                            .id(scrollable::Id::new(SOURCE_CANDIDATES_ID))
                            .height(
                                SOURCE_VISIBLE_ROWS as f32
                                    * (SOURCE_ROW_HEIGHT + SOURCE_ROW_SPACING),
                            ),
                    );
                } else if count > 0 {
                    editor = editor.push(candidates);
                } else {
                    editor = editor.push(
                        text("No eligible native markets; discovery status is shown above.")
                            .size(12),
                    );
                }
            }
        }
        let mut actions = row![
            button(if self.selected.is_some() {
                "Apply"
            } else {
                "Add"
            })
            .on_press(Message::Manager(Event::Submit)),
            button("New").on_press(Message::Manager(Event::New)),
        ]
        .spacing(6);
        if self.selected.is_some() {
            actions = actions
                .push(
                    button(
                        if chart
                            .indicators
                            .instances
                            .iter()
                            .find(|instance| Some(instance.id) == self.selected)
                            .is_some_and(|instance| instance.visible)
                        {
                            "Hide"
                        } else {
                            "Show"
                        },
                    )
                    .on_press(Message::Manager(Event::Toggle)),
                )
                .push(button("Remove").on_press(Message::Manager(Event::Remove)));
        }
        if let Some(id) = self.selected {
            actions = actions.push(button("Details").on_press(Message::IndicatorDetails(id)));
        }
        actions = actions.push(button("Close").on_press(Message::Action(Action::ManageIndicators)));
        let mut footer = column![].spacing(6);
        if let Some(error) = self.error.as_ref().or(chart.indicator_error.as_ref()) {
            footer = footer.push(text(error).size(13));
        }
        footer = footer.push(actions.width(Length::Fill).wrap());
        let mut instances = column![].spacing(INSTANCE_SPACING);
        for instance in &chart.indicators.instances {
            let selected = self.selected == Some(instance.id);
            instances = instances.push(
                button(
                    text(format!(
                        "{} #{} {}\n{} · {}",
                        if selected { ">" } else { " " },
                        instance.id,
                        instance.spec.label(),
                        if instance.visible { "shown" } else { "hidden" },
                        instance.source.label()
                    ))
                    .size(13),
                )
                .on_press(Message::Manager(Event::Select(instance.id)))
                .width(Length::Fill)
                .height(INSTANCE_HEIGHT)
                .style(if selected {
                    button::primary
                } else {
                    button::secondary
                }),
            );
        }
        if chart.indicators.instances.is_empty() {
            instances = instances.push(text("No indicators. Choose a type and Add.").size(13));
        }
        let middle: Element<'a, Message> = if side_by_side {
            row![
                column![
                    text("Instances · select to edit").size(14),
                    scrollable(instances)
                        .id(scrollable::Id::new(INSTANCES_ID))
                        .height(Length::Fill),
                ]
                .spacing(8)
                .width(220)
                .height(Length::Fill),
                scrollable(container(editor).padding(iced::Padding {
                    right: 14.0,
                    ..Default::default()
                }))
                .id(scrollable::Id::new(SOURCE_EDITOR_ID))
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(16)
            .height(Length::Fill)
            .into()
        } else {
            column![
                scrollable(container(editor).padding(iced::Padding {
                    right: 14.0,
                    ..Default::default()
                }))
                .id(scrollable::Id::new(SOURCE_EDITOR_ID))
                .height(Length::FillPortion(3)),
                text("Instances · select to edit").size(14),
                scrollable(instances)
                    .id(scrollable::Id::new(INSTANCES_ID))
                    .height(Length::FillPortion(2)),
            ]
            .spacing(8)
            .height(Length::Fill)
            .into()
        };
        container(column![header, middle, footer].spacing(10))
            .padding(10)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn native_oi_edits_preserve_identity_and_visibility() {
        let mut manager = Manager::default();
        let mut chart = ChartState::default();
        manager.load(IndicatorSpec::OpenInterest);
        let (action, _) = manager.update(Event::Submit, &chart);
        chart.dispatch(action.unwrap());
        let _ = manager.applied(&chart);
        let id = chart.indicators.instances[0].id;
        let (action, _) = manager.update(Event::Submit, &chart);
        chart.dispatch(action.unwrap());
        assert_eq!(chart.indicators.instances[0].id, id);
        assert_eq!(chart.indicators.instances.len(), 1);
        chart.dispatch(manager.update(Event::Toggle, &chart).0.unwrap());
        assert!(!chart.indicators.instances[0].visible);
        chart.dispatch(manager.update(Event::Toggle, &chart).0.unwrap());
        assert!(chart.indicators.instances[0].visible);
        let (action, _) = manager.update(Event::Remove, &chart);
        chart.dispatch(action.unwrap());
        assert!(chart.indicators.instances.is_empty());
    }

    use super::*;
    #[test]
    fn cvd_reset_edits_are_independent_and_preserve_native_source() {
        let mut chart = ChartState::default();
        let mut manager = Manager::default();
        manager.load(IndicatorKind::Cvd.default_spec());
        chart.dispatch(manager.update(Event::Submit, &chart).0.unwrap());
        let _ = manager.applied(&chart);
        let chart_id = chart.indicators.instances[0].id;

        let _ = manager.update(Event::New, &chart);
        let _ = manager.update(Event::SourceMode(SourceMode::Single), &chart);
        let _ = manager.update(Event::SourceExchange(Exchange::Okx), &chart);
        manager.accept_source_catalog(
            Exchange::Okx,
            Product::Perpetual,
            Ok(source_catalog(Exchange::Okx, "BTC")),
        );
        let _ = manager.update(Event::StepSourceCandidate(1), &chart);
        let _ = manager.update(Event::ToggleSourceCandidate, &chart);
        let native_source = manager.source.clone();
        let _ = manager.update(Event::Reset(CvdReset::UtcDay), &chart);
        chart.dispatch(manager.update(Event::Submit, &chart).0.unwrap());
        let _ = manager.applied(&chart);
        let native_id = chart.indicators.instances[1].id;

        let _ = manager.update(Event::Select(chart_id), &chart);
        let _ = manager.update(Event::Reset(CvdReset::UtcDay), &chart);
        let _ = manager.update(Event::Select(chart_id), &chart);
        chart.dispatch(manager.update(Event::Submit, &chart).0.unwrap());
        let _ = manager.applied(&chart);
        let _ = manager.update(Event::Select(native_id), &chart);
        assert_eq!(manager.cvd_reset, CvdReset::UtcDay);
        assert_eq!(manager.source, native_source);
        let _ = manager.update(Event::Reset(CvdReset::Continuous), &chart);
        chart.dispatch(manager.update(Event::Submit, &chart).0.unwrap());
        let _ = manager.applied(&chart);

        assert_eq!(chart.indicators.instances.len(), 2);
        assert_eq!(chart.indicators.instances[0].id, chart_id);
        assert_eq!(
            chart.indicators.instances[0].spec,
            IndicatorSpec::Cvd {
                reset: CvdReset::UtcDay
            },
        );
        assert!(chart.indicators.instances[0].source.is_chart());
        assert_eq!(chart.indicators.instances[1].id, native_id);
        assert_eq!(
            chart.indicators.instances[1].spec,
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous
            },
        );
        assert_eq!(chart.indicators.instances[1].source, native_source);
    }

    #[test]
    fn indicator_kind_keyboard_wraps_using_all_kinds() {
        let chart = ChartState::default();
        let mut manager = Manager::default();
        let modifiers = keyboard::Modifiers::default();
        manager.kind = IndicatorKind::ALL[0];
        assert!(matches!(
            manager.key(
                keyboard::Key::Named(keyboard::key::Named::ArrowLeft),
                modifiers,
                false,
                &chart
            ),
            Some(Event::Kind(kind)) if kind == *IndicatorKind::ALL.last().unwrap()
        ));
        manager.kind = *IndicatorKind::ALL.last().unwrap();
        assert!(matches!(
            manager.key(
                keyboard::Key::Named(keyboard::key::Named::ArrowRight),
                modifiers,
                false,
                &chart
            ),
            Some(Event::Kind(kind)) if kind == IndicatorKind::ALL[0]
        ));
    }

    #[test]
    fn applying_a_new_instance_transitions_to_editing_that_instance() {
        let mut chart = ChartState::default();
        let mut manager = Manager::default();
        let (action, _) = manager.update(Event::Submit, &chart);
        chart.dispatch(action.unwrap());
        let _ = manager.applied(&chart);
        let id = chart.indicators.instances[0].id;
        assert_eq!(manager.selected, Some(id));
        let _ = manager.update(Event::Field(0, "25".into()), &chart);
        let (action, _) = manager.update(Event::Submit, &chart);
        chart.dispatch(action.unwrap());
        assert_eq!(chart.indicators.instances[0].id, id);
        assert_eq!(
            chart.indicators.instances[0].spec,
            IndicatorSpec::Sma { period: 25 }
        );
        assert_eq!(chart.indicators.instances.len(), 1);
    }

    #[test]
    fn reselecting_current_instance_keeps_its_unapplied_draft() {
        let mut chart = ChartState::default();
        chart.dispatch(Action::AddIndicator(IndicatorKind::Macd.default_spec()));
        let mut manager = Manager::default();
        let id = chart.indicators.instances[0].id;
        let _ = manager.update(Event::Select(id), &chart);
        let _ = manager.update(Event::Field(1, "not a number".into()), &chart);
        let _ = manager.update(Event::Select(id), &chart);
        let _ = manager.update(Event::Kind(IndicatorKind::Macd), &chart);
        assert_eq!(manager.fields[1], "not a number");
        assert_eq!(manager.spec().unwrap_err().0, 1);
        let (action, _) = manager.update(Event::Submit, &chart);
        assert!(action.is_none());
        assert!(manager.error.is_some());
    }

    #[test]
    fn removing_a_selected_instance_selects_a_surviving_neighbor() {
        let mut chart = ChartState::default();
        chart.dispatch(Action::AddIndicator(IndicatorKind::Sma.default_spec()));
        chart.dispatch(Action::AddIndicator(IndicatorKind::Cvd.default_spec()));
        let mut manager = Manager::default();
        let first = chart.indicators.instances[0].id;
        let second = chart.indicators.instances[1].id;
        let _ = manager.update(Event::Select(first), &chart);
        let (action, _) = manager.update(Event::Remove, &chart);
        assert_eq!(manager.selected, Some(second));
        chart.dispatch(action.unwrap());
        let (action, _) = manager.update(Event::Submit, &chart);
        chart.dispatch(action.unwrap());
        assert_eq!(chart.indicators.instances.len(), 1);
        assert_eq!(chart.indicators.instances[0].id, second);
    }

    fn source_catalog(exchange: Exchange, underlying: &str) -> Arc<Catalog> {
        Arc::new(Catalog {
            exchange,
            product: Product::Perpetual,
            intervals: vec![mexc_model::Timeframe::Min5],
            instruments: vec![Instrument {
                exchange,
                product: Product::Perpetual,
                symbol: format!("{underlying}-USD-native"),
                base_coin: underlying.into(),
                quote_coin: mexc_model::Quote::USD,
                settle_coin: underlying.into(),
                price_scale: 2,
                contract_size: 1.0,
                inverse: matches!(exchange, Exchange::Binance | Exchange::Okx),
                volume_unit: "contracts".into(),
            }],
            flow_support: mexc_model::FlowSupport {
                history: mexc_model::FlowAvailability::Exact,
                live: mexc_model::FlowAvailability::Exact,
                description: String::new(),
            },
        })
    }

    #[test]
    fn all_sources_require_explicit_choices_and_completed_discovery() {
        let chart = ChartState::default();
        let mut manager = Manager::default();
        manager.load(IndicatorKind::Cvd.default_spec());
        let _ = manager.update(Event::SourceMode(SourceMode::All), &chart);
        for exchange in Exchange::ALL {
            manager.accept_source_catalog(
                exchange,
                Product::Perpetual,
                Ok(source_catalog(exchange, "BTC")),
            );
        }
        assert!(manager.source.markets.is_empty());
        assert!(manager.update(Event::Submit, &chart).0.is_none());
        for exchange in Exchange::ALL {
            let _ = manager.update(Event::SourceExchange(exchange), &chart);
            let _ = manager.update(Event::StepSourceCandidate(1), &chart);
            let _ = manager.update(Event::ToggleSourceCandidate, &chart);
        }
        assert!(matches!(manager.update(Event::Submit, &chart).0,
            Some(Action::ApplyIndicator { source, .. }) if source.mode == SourceMode::All && source.markets.len() == 3));
        let selected_markets = manager.source.markets.clone();
        manager.accept_source_catalog(Exchange::Okx, Product::Perpetual, Err("offline".into()));
        assert!(manager.update(Event::Submit, &chart).0.is_none());
        let _ = manager.update(Event::SourceRetry, &chart);
        assert!(manager.source_catalogs_needed());
        assert_eq!(manager.source.markets, selected_markets);
    }

    #[test]
    fn direct_uncheck_targets_membership_not_the_keyboard_cursor() {
        let chart = ChartState::default();
        let mut manager = Manager::default();
        manager.load(IndicatorSpec::OpenInterest);
        let _ = manager.update(Event::SourceMode(SourceMode::Selected), &chart);
        for exchange in [Exchange::Okx, Exchange::Binance] {
            manager.accept_source_catalog(
                exchange,
                Product::Perpetual,
                Ok(source_catalog(exchange, "BTC")),
            );
        }
        let okx = Market {
            exchange: Exchange::Okx,
            product: Product::Perpetual,
            symbol: "BTC-USD-native".into(),
            interval: chart.market.interval,
        };
        let binance = Market {
            exchange: Exchange::Binance,
            ..okx.clone()
        };
        let _ = manager.update(
            Event::SourceVenueIncluded {
                exchange: Exchange::Okx,
                included: true,
            },
            &chart,
        );
        assert!(manager.source.markets.is_empty());
        for market in [&okx, &binance] {
            let (action, _) = manager.update(
                Event::SetSourceMarket {
                    market: market.clone(),
                    chosen: true,
                },
                &chart,
            );
            assert!(action.is_none());
        }
        assert_eq!(manager.source_exchange, Exchange::Binance);
        let _ = manager.update(
            Event::SetSourceMarket {
                market: okx.clone(),
                chosen: false,
            },
            &chart,
        );
        assert_eq!(manager.source.markets, vec![binance.clone()]);
        assert_eq!(manager.source_exchange, Exchange::Okx);
        assert_eq!(
            manager.source_candidate.as_deref(),
            Some(okx.symbol.as_str())
        );
        manager.accept_source_catalog(Exchange::Binance, Product::Perpetual, Err("offline".into()));
        assert!(manager.validated_source(&chart).is_err());
        let _ = manager.update(
            Event::SetSourceMarket {
                market: binance.clone(),
                chosen: false,
            },
            &chart,
        );
        assert!(manager.source.markets.is_empty());
        assert!(chart.indicators.instances.is_empty());
    }

    #[test]
    fn single_replacement_is_atomic_and_rejects_stale_native_choices() {
        let chart = ChartState::default();
        let mut manager = Manager::default();
        manager.load(IndicatorSpec::OpenInterest);
        let _ = manager.update(Event::SourceMode(SourceMode::Single), &chart);
        for exchange in [Exchange::Okx, Exchange::Binance] {
            manager.accept_source_catalog(
                exchange,
                Product::Perpetual,
                Ok(source_catalog(exchange, "BTC")),
            );
        }
        let okx = Market {
            exchange: Exchange::Okx,
            product: Product::Perpetual,
            symbol: "BTC-USD-native".into(),
            interval: chart.market.interval,
        };
        let binance = Market {
            exchange: Exchange::Binance,
            ..okx.clone()
        };
        let _ = manager.update(
            Event::SetSourceMarket {
                market: okx.clone(),
                chosen: true,
            },
            &chart,
        );
        let _ = manager.update(Event::SourceExchange(Exchange::Binance), &chart);
        assert_eq!(manager.source.markets, vec![okx.clone()]);
        let _ = manager.update(
            Event::SetSourceMarket {
                market: binance.clone(),
                chosen: true,
            },
            &chart,
        );
        assert_eq!(manager.source.markets, vec![binance.clone()]);
        assert_eq!(
            manager.source_candidate.as_deref(),
            Some(binance.symbol.as_str())
        );
        // A previously rendered exact choice can become ineligible before its
        // click is handled. It must not destroy the existing Single selection.
        manager.accept_source_catalog(
            Exchange::Okx,
            Product::Perpetual,
            Ok(source_catalog(Exchange::Okx, "ETH")),
        );
        let before = manager.source.clone();
        let _ = manager.update(
            Event::SetSourceMarket {
                market: okx,
                chosen: true,
            },
            &chart,
        );
        assert_eq!(manager.source, before);
        assert_eq!(manager.source_exchange, Exchange::Binance);
        assert_eq!(
            manager.source_candidate.as_deref(),
            Some(binance.symbol.as_str())
        );
        let _ = manager.update(
            Event::SourceVenueIncluded {
                exchange: Exchange::Binance,
                included: false,
            },
            &chart,
        );
        assert!(manager.source.markets.is_empty());
        assert!(manager.update(Event::Submit, &chart).0.is_none());
        assert!(chart.indicators.instances.is_empty());
    }

    #[test]
    fn catalog_arrival_and_show_preserve_exact_inverse_source_draft() {
        let chart = ChartState::default();
        let mut manager = Manager::default();
        manager.load(IndicatorKind::OpenInterest.default_spec());
        let _ = manager.update(Event::SourceMode(SourceMode::Single), &chart);
        manager.source_exchange = Exchange::Okx;
        manager.accept_source_catalog(
            Exchange::Okx,
            Product::Perpetual,
            Ok(source_catalog(Exchange::Okx, "BTC")),
        );
        let _ = manager.update(Event::StepSourceCandidate(1), &chart);
        let _ = manager.update(Event::ToggleSourceCandidate, &chart);
        let draft = manager.source.clone();
        let _ = manager.show();
        manager.accept_source_catalog(
            Exchange::Binance,
            Product::Perpetual,
            Ok(source_catalog(Exchange::Binance, "ETH")),
        );
        assert_eq!(manager.source, draft);
        assert!(manager.candidates(Exchange::Binance, &chart).is_empty());
        let source = manager.validated_source(&chart).unwrap();
        assert_eq!(source.markets[0].symbol, "BTC-USD-native");
        assert_eq!(source.markets[0].exchange, Exchange::Okx);
    }
}

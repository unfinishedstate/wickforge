use crate::workspace::{RECENT_MARKETS_LIMIT, SavedMarket, validate_quick_markets};
use crate::{Message, state::Action};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Element, Length, Task, keyboard};
use mexc_model::{Catalog, Exchange, Instrument, Market, Product, Quote, Timeframe};
use std::sync::Arc;

const SEARCH_ID: &str = "market-search";
const RESULTS_ID: &str = "market-results";
const RESULT_HEIGHT: f32 = 58.0;
const RESULT_SPACING: f32 = 4.0;

#[derive(Debug, Clone)]
pub enum Event {
    Quick(Market),
    ToggleFavorite,
    Search(String),
    Select(String),
    Interval(Timeframe),
    StepInterval(i32),
    Exchange(Exchange),
    Product(Product),
    Quote(Quote),
    Submit,
    Retry,
}
#[derive(Default)]
pub struct Selector {
    pub open: bool,
    pub loading: bool,
    pub catalog_error: Option<String>,
    catalog: Option<Arc<Catalog>>,
    ordered: Vec<usize>,
    matching: Vec<usize>,
    pub exchange: Exchange,
    pub product: Product,
    pub quote: Quote,
    query: String,
    search_key: String,
    selected: Option<String>,
    interval: Timeframe,
    pending_interval_steps: i32,
    error: Option<String>,
    recent_markets: Vec<Market>,
    favorite_markets: Vec<Market>,
    queued_exact: Option<Market>,
    quick_auto_submit: bool,
}

impl Selector {
    /// Install exact MRU/favorite order without observing an additional market.
    /// Catalog availability is deliberately deferred to explicit selection.
    pub fn restore_quick_markets(
        &mut self,
        recents: &[SavedMarket],
        favorites: &[SavedMarket],
    ) -> Result<(), String> {
        validate_quick_markets(recents, favorites)?;
        let recent_markets = recents
            .iter()
            .map(SavedMarket::to_market)
            .collect::<Result<_, _>>()?;
        let favorite_markets = favorites
            .iter()
            .map(SavedMarket::to_market)
            .collect::<Result<_, _>>()?;
        self.recent_markets = recent_markets;
        self.favorite_markets = favorite_markets;
        Ok(())
    }

    pub fn show(&mut self, market: &Market, quote: Quote) -> Task<Message> {
        self.open = true;
        self.query.clear();
        self.selected = Some(market.symbol.clone());
        self.exchange = market.exchange;
        self.product = market.product;
        self.interval = market.interval;
        self.quote = quote;
        self.error = None;
        self.pending_interval_steps = 0;
        self.queued_exact = None;
        self.quick_auto_submit = false;
        self.search_key.clear();
        self.refresh_results();
        Task::batch([self.focus_search(), self.follow_selection()])
    }

    pub fn show_exact(&mut self, market: &Market) -> Task<Message> {
        self.open = true;
        self.query.clear();
        self.search_key.clear();
        self.exchange = market.exchange;
        self.product = market.product;
        self.interval = market.interval;
        self.selected = Some(market.symbol.clone());
        self.pending_interval_steps = 0;
        self.queued_exact = Some(market.clone());
        self.quick_auto_submit = true;
        self.catalog_error = None;
        self.loading = false;
        self.clear_catalog();
        Task::batch([self.focus_search(), self.follow_selection()])
    }

    pub fn observe_market(&mut self, market: &Market) {
        self.recent_markets.retain(|entry| entry != market);
        self.recent_markets.insert(0, market.clone());
        self.recent_markets.truncate(RECENT_MARKETS_LIMIT);
    }

    pub fn recent_markets(&self) -> &[Market] {
        &self.recent_markets
    }

    pub fn favorite_markets(&self) -> &[Market] {
        &self.favorite_markets
    }

    fn selected_market(&self) -> Option<Market> {
        let symbol = self.selected.as_deref()?;
        let instrument = self.instrument(symbol)?;
        self.intervals().contains(&self.interval).then(|| Market {
            exchange: self.exchange,
            product: self.product,
            symbol: instrument.symbol.clone(),
            interval: self.interval,
        })
    }

    fn toggle_favorite(&mut self) {
        let Some(market) = self.selected_market() else {
            self.reject("Select an available native market and interval before favoriting");
            return;
        };
        if let Some(index) = self
            .favorite_markets
            .iter()
            .position(|entry| entry == &market)
        {
            self.favorite_markets.remove(index);
        } else {
            self.favorite_markets.push(market);
        }
    }

    fn quick_markets(&self) -> Vec<Market> {
        let mut markets = self.favorite_markets.clone();
        for market in &self.recent_markets {
            if !markets.contains(market) {
                markets.push(market.clone());
            }
        }
        markets
    }

    pub fn scope(&self) -> (Exchange, Product) {
        (self.exchange, self.product)
    }

    pub fn focus_search(&self) -> Task<Message> {
        text_input::focus(SEARCH_ID)
    }

    pub fn begin_catalog(&mut self) {
        self.loading = true;
        self.catalog_error = None;
        self.error = None;
    }

    pub fn set_catalog(
        &mut self,
        exchange: Exchange,
        product: Product,
        result: Result<Arc<Catalog>, String>,
    ) {
        if (exchange, product) != self.scope() {
            return;
        }
        self.loading = false;
        match result {
            Ok(catalog) => {
                if catalog.exchange != exchange
                    || catalog.product != product
                    || catalog.instruments.iter().any(|instrument| {
                        instrument.exchange != exchange || instrument.product != product
                    })
                {
                    self.clear_catalog();
                    self.catalog_error =
                        Some("Data service returned a catalog for a different scope".into());
                    return;
                }
                if self.queued_exact.is_none()
                    && !catalog.intervals.contains(&self.interval)
                    && let Some(interval) = catalog.intervals.first()
                {
                    self.interval = *interval;
                }
                self.catalog_error = if catalog.instruments.is_empty() {
                    Some("No native markets are available for this scope".into())
                } else if catalog.intervals.is_empty() {
                    Some("No native intervals are available for this scope".into())
                } else {
                    None
                };
                self.ordered = (0..catalog.instruments.len()).collect();
                self.ordered.sort_unstable_by(|left, right| {
                    catalog.instruments[*left]
                        .symbol
                        .cmp(&catalog.instruments[*right].symbol)
                });
                self.catalog = Some(catalog);
                if self.pending_interval_steps != 0 {
                    self.step_interval(self.pending_interval_steps);
                }
                self.pending_interval_steps = 0;
                self.refresh_results();
            }
            Err(error) => {
                self.clear_catalog();
                self.catalog_error = Some(error);
            }
        }
    }

    pub fn take_ready_quick(&mut self) -> Option<Market> {
        if !self.quick_auto_submit {
            return None;
        }
        self.quick_auto_submit = false;
        let exact = self.queued_exact.as_ref()?.clone();
        if !self.intervals().contains(&exact.interval) {
            self.reject(&format!(
                "Saved interval {} is unavailable for this native scope",
                exact.interval
            ));
            return None;
        }
        let Some(catalog) = self.current_catalog() else {
            self.reject("Wait for the exact native market catalog before restoring this choice");
            return None;
        };
        let mut native_quote = None;
        let mut ambiguous = false;
        for instrument in &catalog.instruments {
            if instrument.symbol == exact.symbol {
                if native_quote.is_some() {
                    ambiguous = true;
                    break;
                }
                native_quote = Some(instrument.quote_coin);
            }
        }
        let Some(native_quote) = native_quote else {
            self.reject("Saved native market is unavailable; choose an exact listing to repair it");
            return None;
        };
        if ambiguous {
            self.reject(
                "Saved symbol has multiple native quote listings; choose the exact listing to repair it",
            );
            return None;
        }
        self.quote = native_quote;
        self.queued_exact = None;
        self.error = None;
        self.refresh_results();
        Some(exact)
    }

    fn current_catalog(&self) -> Option<&Catalog> {
        self.catalog
            .as_deref()
            .filter(|catalog| (catalog.exchange, catalog.product) == self.scope())
    }

    pub fn catalog(&self) -> Option<&Arc<Catalog>> {
        self.catalog
            .as_ref()
            .filter(|catalog| (catalog.exchange, catalog.product) == self.scope())
    }

    pub fn instrument(&self, symbol: &str) -> Option<&Instrument> {
        self.current_catalog()?
            .instruments
            .iter()
            .find(|instrument| instrument.symbol == symbol && instrument.quote_coin == self.quote)
    }

    pub fn active_instrument(&self, symbol: &str) -> Option<&Instrument> {
        self.current_catalog()?
            .instruments
            .iter()
            .find(|instrument| instrument.symbol == symbol)
    }

    pub fn intervals(&self) -> &[Timeframe] {
        self.current_catalog()
            .map(|catalog| catalog.intervals.as_slice())
            .unwrap_or(&[])
    }

    pub fn reject(&mut self, error: &str) {
        self.error = Some(error.into());
    }

    fn clear_catalog(&mut self) {
        self.catalog = None;
        self.ordered.clear();
        self.matching.clear();
    }

    fn matches(&self, instrument: &Instrument) -> bool {
        if instrument.quote_coin != self.quote {
            return false;
        }
        let query = self.search_key.as_bytes();
        if query.is_empty() {
            return true;
        }
        // Ignore separators only for discovery; selection always uses the
        // exact native symbol, quote and product from the catalog.
        let contains = |value: &str| {
            value.as_bytes().iter().enumerate().any(|(start, _)| {
                let mut native = value.as_bytes()[start..]
                    .iter()
                    .copied()
                    .filter(|byte| !matches!(byte, b'_' | b'-' | b'/'));
                query.iter().all(|byte| {
                    native
                        .next()
                        .is_some_and(|next| next.eq_ignore_ascii_case(byte))
                })
            })
        };
        contains(&instrument.symbol) || contains(&instrument.base_coin)
    }

    fn results(&self) -> impl Iterator<Item = &Instrument> {
        self.current_catalog().into_iter().flat_map(|catalog| {
            self.matching
                .iter()
                .map(|index| &catalog.instruments[*index])
        })
    }

    fn refresh_results(&mut self) {
        let mut matching = std::mem::take(&mut self.matching);
        matching.clear();
        if let Some(catalog) = self.current_catalog() {
            matching.extend(
                self.ordered
                    .iter()
                    .copied()
                    .filter(|index| self.matches(&catalog.instruments[*index])),
            );
        }
        self.matching = matching;
        if self.queued_exact.is_none()
            && self.current_catalog().is_some()
            && !self
                .results()
                .any(|instrument| self.selected.as_deref() == Some(instrument.symbol.as_str()))
        {
            let selected = self
                .results()
                .next()
                .map(|instrument| instrument.symbol.clone());
            self.selected = selected;
        }
    }

    fn step_interval(&mut self, steps: i32) {
        self.quick_auto_submit = false;
        let intervals = self.intervals();
        if intervals.is_empty() {
            self.pending_interval_steps = self.pending_interval_steps.saturating_add(steps);
            return;
        }
        let index = intervals
            .iter()
            .position(|interval| *interval == self.interval)
            .unwrap_or(0);
        let next = (index as i64 + i64::from(steps)).rem_euclid(intervals.len() as i64) as usize;
        self.interval = intervals[next];
        if let Some(exact) = self.queued_exact.as_mut() {
            exact.interval = self.interval;
        }
    }

    pub fn follow_selection(&self) -> Task<Message> {
        let index = self
            .results()
            .position(|instrument| self.selected.as_deref() == Some(instrument.symbol.as_str()))
            .unwrap_or(0);
        scrollable::scroll_to(
            scrollable::Id::new(RESULTS_ID),
            scrollable::AbsoluteOffset {
                x: 0.0,
                y: index as f32 * (RESULT_HEIGHT + RESULT_SPACING),
            },
        )
    }

    pub fn update(&mut self, event: Event) -> Option<Action> {
        self.error = None;
        match event {
            Event::Quick(market) => return Some(Action::SelectMarket(market)),
            Event::ToggleFavorite => self.toggle_favorite(),
            Event::Search(value) => {
                self.queued_exact = None;
                self.quick_auto_submit = false;
                self.search_key = value
                    .trim()
                    .chars()
                    .filter(|character| !matches!(character, '_' | '-' | '/'))
                    .collect();
                self.query = value;
                self.refresh_results();
            }
            Event::Select(symbol) => {
                if self.results().any(|instrument| instrument.symbol == symbol) {
                    self.selected = Some(symbol);
                    self.queued_exact = None;
                    self.quick_auto_submit = false;
                }
            }
            Event::Interval(interval) => {
                if self.intervals().contains(&interval) {
                    self.interval = interval;
                    if let Some(exact) = self.queued_exact.as_mut() {
                        exact.interval = interval;
                    }
                    self.quick_auto_submit = false;
                }
            }
            Event::StepInterval(steps) => self.step_interval(steps),
            Event::Exchange(exchange) => {
                if exchange != self.exchange {
                    self.exchange = exchange;
                    self.clear_catalog();
                    self.pending_interval_steps = 0;
                    self.queued_exact = None;
                    self.quick_auto_submit = false;
                    self.catalog_error = None;
                    self.loading = false;
                    self.selected = None;
                }
            }
            Event::Product(product) => {
                if product != self.product {
                    self.product = product;
                    self.clear_catalog();
                    self.pending_interval_steps = 0;
                    self.queued_exact = None;
                    self.quick_auto_submit = false;
                    self.catalog_error = None;
                    self.loading = false;
                    self.selected = None;
                }
            }
            Event::Quote(quote) => {
                self.queued_exact = None;
                self.quick_auto_submit = false;
                self.quote = quote;
                self.refresh_results();
            }
            Event::Submit => {
                if self.loading {
                    self.reject("Wait for the native catalog to finish loading");
                    return None;
                }
                if let Some(error) = self.catalog_error.as_ref() {
                    self.error = Some(format!("Catalog unavailable: {error}. Retry with Ctrl+R."));
                    return None;
                }
                if self.queued_exact.is_some() {
                    self.quick_auto_submit = true;
                    let exact = self.take_ready_quick()?;
                    return Some(Action::SelectMarket(exact));
                }
                if !self.intervals().contains(&self.interval) {
                    self.reject("Choose an interval supported by this market scope");
                    return None;
                }
                let Some(instrument) = self
                    .selected
                    .as_deref()
                    .and_then(|symbol| self.instrument(symbol))
                else {
                    self.reject(
                        "Choose an available native market. Clear the search or change scope.",
                    );
                    return None;
                };
                return Some(Action::SelectMarket(Market {
                    exchange: self.exchange,
                    product: self.product,
                    symbol: instrument.symbol.clone(),
                    interval: self.interval,
                }));
            }
            Event::Retry => {}
        }
        None
    }

    pub fn key(&self, key: keyboard::Key<&str>, modifiers: keyboard::Modifiers) -> Option<Event> {
        use keyboard::{Key, key::Named};

        if modifiers.alt() {
            match key {
                Key::Named(Named::ArrowLeft) => {
                    let previous =
                        (self.exchange.index() + Exchange::ALL.len() - 1) % Exchange::ALL.len();
                    return Some(Event::Exchange(Exchange::ALL[previous]));
                }
                Key::Named(Named::ArrowRight) => {
                    let next = (self.exchange.index() + 1) % Exchange::ALL.len();
                    return Some(Event::Exchange(Exchange::ALL[next]));
                }
                Key::Named(Named::ArrowUp) => return Some(Event::Product(Product::Spot)),
                Key::Named(Named::ArrowDown) => return Some(Event::Product(Product::Perpetual)),
                _ => {}
            }
        }

        if modifiers.control()
            && modifiers.shift()
            && matches!(key, Key::Named(Named::ArrowUp | Named::ArrowDown))
        {
            let markets = self.quick_markets();
            if !markets.is_empty() {
                let current = self.queued_exact.clone().or_else(|| self.selected_market());
                let index =
                    current.and_then(|market| markets.iter().position(|entry| entry == &market));
                let next = match (index, key) {
                    (Some(index), Key::Named(Named::ArrowUp)) => {
                        (index + markets.len() - 1) % markets.len()
                    }
                    (Some(index), _) => (index + 1) % markets.len(),
                    (None, Key::Named(Named::ArrowUp)) => markets.len() - 1,
                    (None, _) => 0,
                };
                return Some(Event::Quick(markets[next].clone()));
            }
        }

        if modifiers.control() {
            match key {
                Key::Character("q" | "Q") => {
                    let index = Quote::ALL
                        .iter()
                        .position(|quote| *quote == self.quote)
                        .unwrap_or(0);
                    return Some(Event::Quote(Quote::ALL[(index + 1) % Quote::ALL.len()]));
                }
                Key::Character("f" | "F") => return Some(Event::ToggleFavorite),
                Key::Character("r" | "R") => return Some(Event::Retry),
                Key::Named(Named::Enter) => return Some(Event::Submit),
                Key::Named(Named::ArrowLeft) => return Some(Event::StepInterval(-1)),
                Key::Named(Named::ArrowRight) => return Some(Event::StepInterval(1)),
                _ => {}
            }
        }

        match key {
            Key::Named(Named::ArrowUp | Named::ArrowDown) => {
                let current = self.results().position(|instrument| {
                    self.selected.as_deref() == Some(instrument.symbol.as_str())
                });
                let count = self.matching.len();
                let index = match (current, key) {
                    (Some(index), Key::Named(Named::ArrowUp)) => index.saturating_sub(1),
                    (Some(index), _) => (index + 1).min(count.saturating_sub(1)),
                    (None, _) => 0,
                };
                self.results()
                    .nth(index)
                    .map(|instrument| Event::Select(instrument.symbol.clone()))
            }
            _ => None,
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let header = column![
            row![
                text("Market").size(20),
                iced::widget::Space::new(Length::Fill, Length::Shrink),
                button("Keys [F1]")
                    .on_press(Message::HelpOpen)
                    .style(crate::ui::quiet)
            ]
            .align_y(iced::Alignment::Center),
            crate::ui::separator(),
        ]
        .spacing(5);
        let intervals = self.intervals();
        let interval_supported = intervals.contains(&self.interval);
        let search = text_input("Search symbol or base coin", &self.query)
            .id(SEARCH_ID)
            .on_input(|value| Message::Selector(Event::Search(value)))
            .on_submit(Message::Selector(Event::Submit))
            .padding(6)
            .style(crate::ui::input);
        let exchange = column![
            text("Venue").size(11).style(crate::ui::muted_text),
            pick_list(Exchange::ALL, Some(self.exchange), |value| {
                Message::Selector(Event::Exchange(value))
            })
            .width(Length::Fill)
            .style(crate::ui::pick),
        ]
        .spacing(3)
        .width(Length::Fill);
        let product = column![
            text("Product").size(11).style(crate::ui::muted_text),
            pick_list(Product::ALL, Some(self.product), |value| {
                Message::Selector(Event::Product(value))
            })
            .width(Length::Fill)
            .style(crate::ui::pick),
        ]
        .spacing(3)
        .width(Length::Fill);
        let quote = column![
            text("Quote").size(11).style(crate::ui::muted_text),
            pick_list(Quote::ALL, Some(self.quote), |value| {
                Message::Selector(Event::Quote(value))
            })
            .width(Length::Fill)
            .style(crate::ui::pick),
        ]
        .spacing(3)
        .width(Length::Fill);
        let interval = column![
            text("Native interval")
                .size(11)
                .style(crate::ui::muted_text),
            pick_list(
                intervals,
                interval_supported.then_some(self.interval),
                |value| Message::Selector(Event::Interval(value))
            )
            .width(Length::Fill)
            .style(crate::ui::pick),
        ]
        .spacing(3)
        .width(Length::Fill);
        let filters = column![
            row![exchange, product].spacing(8),
            row![quote, interval].spacing(8),
        ]
        .spacing(6);

        let mut body = column![
            text("Search").size(11).style(crate::ui::muted_text),
            search,
            filters,
        ]
        .spacing(5)
        .height(Length::Fill);
        let quick_markets = self.quick_markets();
        if !quick_markets.is_empty() {
            let mut quick = row![].spacing(4);
            for market in quick_markets {
                let is_favorite = self.favorite_markets.contains(&market);
                quick = quick.push(
                    button(
                        text(format!(
                            "{} · {} · {} · {} · {}",
                            if is_favorite { "Favorite" } else { "Recent" },
                            market.exchange,
                            market.product,
                            market.symbol,
                            market.interval,
                        ))
                        .size(11),
                    )
                    .on_press(Message::Selector(Event::Quick(market)))
                    .style(crate::ui::quiet),
                );
            }
            body = body.push(
                column![
                    row![
                        text("Quick markets").size(11).style(crate::ui::muted_text),
                        iced::widget::Space::new(Length::Fill, Length::Shrink),
                        button(
                            if self
                                .selected_market()
                                .is_some_and(|market| self.favorite_markets.contains(&market))
                            {
                                "Unfavorite · Ctrl+F"
                            } else {
                                "Favorite · Ctrl+F"
                            }
                        )
                        .on_press(Message::Selector(Event::ToggleFavorite))
                        .style(crate::ui::quiet),
                    ]
                    .align_y(iced::Alignment::Center),
                    scrollable(quick.wrap()).height(76).width(Length::Fill),
                ]
                .spacing(4),
            );
        } else {
            body = body.push(
                row![
                    text("Quick markets").size(11).style(crate::ui::muted_text),
                    iced::widget::Space::new(Length::Fill, Length::Shrink),
                    button("Favorite · Ctrl+F")
                        .on_press(Message::Selector(Event::ToggleFavorite))
                        .style(crate::ui::quiet),
                ]
                .align_y(iced::Alignment::Center),
            );
        }

        if self.loading {
            body = body.push(text("Loading native market catalog…").size(13));
        }

        let mut results = column![].spacing(RESULT_SPACING);
        let mut count = 0;
        for instrument in self.results() {
            count += 1;
            let selected = self.selected.as_deref() == Some(instrument.symbol.as_str());
            let kind = if instrument.product == Product::Spot {
                "spot"
            } else if instrument.inverse {
                "inverse"
            } else {
                "linear"
            };
            results = results.push(
                button(
                    column![
                        row![
                            text(&instrument.symbol).size(14),
                            crate::ui::badge(kind, crate::ui::Tone::Accent),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                        text(format!(
                            "Quote {}   ·   Settle {}   ·   Vol {}",
                            instrument.quote_coin, instrument.settle_coin, instrument.volume_unit,
                        ))
                        .size(11)
                        .style(crate::ui::muted_text),
                    ]
                    .spacing(4),
                )
                .on_press(Message::Selector(Event::Select(instrument.symbol.clone())))
                .style(if selected {
                    crate::ui::selected
                } else {
                    crate::ui::quiet
                })
                .width(Length::Fill)
                .height(RESULT_HEIGHT),
            );
        }
        if count == 0 && !self.loading && self.catalog_error.is_none() {
            results = results.push(
                text(if self.search_key.is_empty() {
                    "No native markets available for this quote currency"
                } else {
                    "No matching native markets. Clear search or change quote."
                })
                .size(12)
                .style(crate::ui::muted_text),
            );
        }
        body = body
            .push(
                text(format!("{count} native markets"))
                    .size(11)
                    .style(crate::ui::muted_text),
            )
            .push(
                scrollable(container(results).padding(iced::Padding {
                    right: 18.0,
                    ..Default::default()
                }))
                .id(scrollable::Id::new(RESULTS_ID))
                .width(Length::Fill)
                .height(Length::Fill),
            );

        let mut footer = column![
            crate::ui::separator(),
            text(match self.queued_exact.as_ref() {
                Some(market) => format!(
                    "Saved exact {} · {} · {} · {}",
                    market.exchange, market.product, market.symbol, market.interval
                ),
                None => match self.selected_market() {
                    Some(market) => format!(
                        "Selected {} · {} · {} · {}",
                        market.exchange, market.product, market.symbol, market.interval
                    ),
                    None => format!("Select a native market · {}", self.interval),
                },
            })
            .size(12),
        ]
        .spacing(5);
        if self.pending_interval_steps != 0 {
            footer = footer.push(
                text("Interval navigation queued until native intervals arrive")
                    .size(11)
                    .style(crate::ui::muted_text),
            );
        }
        if let Some(error) = &self.catalog_error {
            footer = footer
                .push(
                    text(format!("Catalog unavailable: {error}"))
                        .size(11)
                        .style(crate::ui::negative_text),
                )
                .push(
                    button("Retry catalog")
                        .on_press(Message::Selector(Event::Retry))
                        .style(crate::ui::quiet),
                );
        }
        if let Some(error) = &self.error {
            footer = footer.push(text(error).size(11).style(crate::ui::negative_text));
        }
        let apply = button("Apply").style(crate::ui::primary);
        let apply = if !self.loading
            && self.catalog_error.is_none()
            && self.selected.is_some()
            && interval_supported
        {
            apply.on_press(Message::Selector(Event::Submit))
        } else {
            apply
        };
        footer = footer.push(
            row![
                button("Cancel")
                    .on_press(Message::Action(Action::ManageMarket))
                    .style(crate::ui::quiet),
                apply,
                iced::widget::Space::new(Length::Fill, Length::Shrink),
                crate::ui::key("↑/↓"),
                text("select").size(11),
                crate::ui::key("Ctrl+F"),
                text("favorite").size(11),
                crate::ui::key("Ctrl+Shift+↑/↓"),
                text("quick").size(11),
                crate::ui::key("Enter"),
                text("apply").size(11),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center)
            .wrap(),
        );

        container(column![header, body, footer].spacing(7))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mexc_model::{FlowAvailability, FlowSupport};

    fn catalog(exchange: Exchange, product: Product) -> Arc<Catalog> {
        Arc::new(Catalog {
            exchange,
            product,
            intervals: vec![Timeframe::Min1, Timeframe::Min5, Timeframe::Min60],
            instruments: vec![Instrument {
                exchange,
                product,
                symbol: "BTC_USDC".into(),
                base_coin: "BTC".into(),
                quote_coin: Quote::default(),
                settle_coin: "USDC".into(),
                price_scale: 2,
                tick_size: None,
                contract_size: 1.0,
                inverse: false,
                volume_unit: "BTC".into(),
            }],
            flow_support: FlowSupport {
                history: FlowAvailability::Unavailable,
                live: FlowAvailability::Unavailable,
                description: "Not used by selector tests".into(),
            },
        })
    }

    #[test]
    fn workspace_quick_restore_keeps_exact_order_and_does_not_observe_startup_market() {
        let first = Market {
            exchange: Exchange::Coinbase,
            product: Product::Spot,
            symbol: "btc-USDC".into(),
            interval: Timeframe::Min1,
        };
        let second = Market {
            interval: Timeframe::Day1,
            ..first.clone()
        };
        let unavailable = Market {
            product: Product::Perpetual,
            symbol: "native-unavailable".into(),
            ..first.clone()
        };
        let recents: Vec<_> = [&second, &first]
            .into_iter()
            .map(SavedMarket::from)
            .collect();
        let favorites = vec![SavedMarket::from(&unavailable), SavedMarket::from(&first)];
        let mut selector = Selector::default();
        selector.observe_market(&Market::default());
        selector
            .restore_quick_markets(&recents, &favorites)
            .unwrap();
        assert_eq!(selector.recent_markets(), &[second, first.clone()]);
        assert_eq!(selector.favorite_markets(), &[unavailable, first]);
        assert!(!selector.recent_markets().contains(&Market::default()));
        assert!(selector.catalog.is_none());
        assert!(!selector.open);
    }

    #[test]
    fn workspace_quick_restore_rejects_duplicates_and_oversized_mru_transactionally() {
        let saved = SavedMarket::from(&Market::default());
        let mut selector = Selector::default();
        selector
            .restore_quick_markets(std::slice::from_ref(&saved), std::slice::from_ref(&saved))
            .unwrap();
        let before_recents = selector.recent_markets().to_vec();
        let before_favorites = selector.favorite_markets().to_vec();
        assert!(
            selector
                .restore_quick_markets(&[saved.clone(), saved.clone()], &[])
                .is_err()
        );
        assert!(
            selector
                .restore_quick_markets(&[], &[saved.clone(), saved.clone()])
                .is_err()
        );
        let too_many: Vec<_> = (0..=RECENT_MARKETS_LIMIT)
            .map(|index| SavedMarket {
                symbol: format!("EXACT_{index}"),
                ..saved.clone()
            })
            .collect();
        assert!(selector.restore_quick_markets(&too_many, &[]).is_err());
        let mut invalid = saved;
        invalid.symbol.clear();
        assert!(selector.restore_quick_markets(&[], &[invalid]).is_err());
        assert_eq!(selector.recent_markets(), before_recents.as_slice());
        assert_eq!(selector.favorite_markets(), before_favorites.as_slice());
        selector.restore_quick_markets(&[], &[]).unwrap();
        assert!(selector.recent_markets().is_empty());
        assert!(selector.favorite_markets().is_empty());
    }
    #[test]
    fn interval_shortcuts_apply_after_native_catalog_arrives() {
        let mut selector = Selector::default();
        let scope = selector.scope();
        selector.begin_catalog();
        let event = selector
            .key(
                keyboard::Key::Named(keyboard::key::Named::ArrowRight),
                keyboard::Modifiers::CTRL,
            )
            .unwrap();
        let _ = selector.update(event);
        assert_eq!(selector.pending_interval_steps, 1);
        assert!(selector.update(Event::Submit).is_none());

        selector.set_catalog(scope.0, scope.1, Ok(catalog(scope.0, scope.1)));
        assert!(matches!(
            selector.update(Event::Submit),
            Some(Action::SelectMarket(Market {
                interval: Timeframe::Min60,
                ..
            }))
        ));
        assert_eq!(selector.pending_interval_steps, 0);
    }

    #[test]
    fn scoped_catalog_rejects_stale_results_and_pending_navigation() {
        let mut selector = Selector::default();
        let old_scope = selector.scope();
        let _ = selector.update(Event::StepInterval(1));
        let next = Exchange::ALL[(selector.exchange.index() + 1) % Exchange::ALL.len()];
        let _ = selector.update(Event::Exchange(next));
        selector.begin_catalog();
        selector.set_catalog(
            old_scope.0,
            old_scope.1,
            Ok(catalog(old_scope.0, old_scope.1)),
        );
        assert!(selector.loading);
        assert!(selector.intervals().is_empty());
        assert!(selector.update(Event::Submit).is_none());

        selector.set_catalog(next, selector.product, Ok(catalog(next, selector.product)));
        assert_eq!(selector.interval, Timeframe::Min5);
        assert!(
            matches!(selector.update(Event::Submit), Some(Action::SelectMarket(market)) if market.exchange == next)
        );
    }

    #[test]
    fn normalized_discovery_preserves_native_symbols_and_shared_catalog() {
        let mut selector = Selector::default();
        let scope = selector.scope();
        let catalog = catalog(scope.0, scope.1);
        selector.set_catalog(scope.0, scope.1, Ok(Arc::clone(&catalog)));
        let _ = selector.update(Event::Search(" btc/usdc ".into()));
        assert_eq!(selector.matching.len(), 1);
        assert!(Arc::ptr_eq(selector.catalog.as_ref().unwrap(), &catalog));
        assert!(matches!(
            selector.update(Event::Submit),
            Some(Action::SelectMarket(market)) if market.symbol == "BTC_USDC"
        ));
        let _ = selector.update(Event::Search("does-not-exist".into()));
        assert!(selector.update(Event::Submit).is_none());
        let _ = selector.update(Event::Search(String::new()));
        assert!(selector.update(Event::Submit).is_some());
    }

    #[test]
    fn recent_identity_is_full_market_and_most_recent_is_first() {
        let mut selector = Selector::default();
        let mut market = Market::default();
        selector.observe_market(&market);
        selector.observe_market(&market);
        market.interval = Timeframe::Min1;
        selector.observe_market(&market);
        market.exchange = Exchange::ALL[(market.exchange.index() + 1) % Exchange::ALL.len()];
        selector.observe_market(&market);

        assert_eq!(selector.recent_markets().len(), 3);
        assert_eq!(selector.recent_markets()[0], market);
        assert_eq!(selector.recent_markets()[1].interval, Timeframe::Min1);
        assert_eq!(selector.recent_markets()[2].exchange, Exchange::default());

        for index in 0..RECENT_MARKETS_LIMIT + 2 {
            selector.observe_market(&Market {
                symbol: format!("NATIVE_{index}"),
                ..Market::default()
            });
        }
        assert_eq!(selector.recent_markets().len(), RECENT_MARKETS_LIMIT);
    }

    #[test]
    fn exact_quick_choice_rejects_unsupported_interval_without_replacement() {
        let mut selector = Selector::default();
        let saved = Market {
            interval: Timeframe::Min60,
            ..Market::default()
        };
        let _ = selector.show_exact(&saved);
        let mut native = (*catalog(saved.exchange, saved.product)).clone();
        native.intervals = vec![Timeframe::Min1, Timeframe::Min5];
        selector.set_catalog(saved.exchange, saved.product, Ok(Arc::new(native)));

        assert_eq!(selector.interval, Timeframe::Min60);
        assert!(selector.take_ready_quick().is_none());
        assert_eq!(selector.queued_exact.as_ref(), Some(&saved));
        assert!(
            selector
                .error
                .as_deref()
                .is_some_and(|error| error.contains("interval"))
        );
        assert!(selector.update(Event::Submit).is_none());
        selector.update(Event::Interval(Timeframe::Min5));
        assert!(matches!(
            selector.update(Event::Submit),
            Some(Action::SelectMarket(market))
                if market == Market::default()
        ));
    }

    #[test]
    fn quick_choice_rejects_missing_native_symbol_instead_of_guessing() {
        let mut selector = Selector::default();
        let saved = Market {
            symbol: "NOT_LISTED".into(),
            ..Market::default()
        };
        let _ = selector.show_exact(&saved);
        selector.set_catalog(
            saved.exchange,
            saved.product,
            Ok(catalog(saved.exchange, saved.product)),
        );

        assert!(selector.take_ready_quick().is_none());
        assert_eq!(selector.selected.as_deref(), Some("NOT_LISTED"));
        assert_eq!(selector.queued_exact.as_ref(), Some(&saved));
        selector.update(Event::Select("BTC_USDC".into()));
        assert!(matches!(
            selector.update(Event::Submit),
            Some(Action::SelectMarket(market)) if market == Market::default()
        ));
    }

    #[test]
    fn favorite_toggle_uses_exact_interval_identity() {
        let mut selector = Selector::default();
        let (exchange, product) = selector.scope();
        selector.set_catalog(exchange, product, Ok(catalog(exchange, product)));
        let min5 = selector.selected_market().unwrap();
        let _ = selector.update(Event::ToggleFavorite);
        let _ = selector.update(Event::Interval(Timeframe::Min1));
        let min1 = selector.selected_market().unwrap();
        let _ = selector.update(Event::ToggleFavorite);
        assert_eq!(selector.favorite_markets(), &[min5.clone(), min1.clone()]);

        let _ = selector.update(Event::Interval(Timeframe::Min5));
        let _ = selector.update(Event::ToggleFavorite);
        assert_eq!(selector.favorite_markets(), &[min1]);
    }

    #[test]
    fn quick_event_returns_the_exact_typed_market_candidate() {
        let mut selector = Selector::default();
        let mut market = Market::default();
        market.exchange = Exchange::ALL[(market.exchange.index() + 1) % Exchange::ALL.len()];
        market.interval = Timeframe::Min60;

        assert!(matches!(
            selector.update(Event::Quick(market.clone())),
            Some(Action::SelectMarket(candidate)) if candidate == market
        ));
    }

    #[test]
    fn manual_filters_prevent_late_exact_catalog_from_overriding_user_choice() {
        let other_quote = Quote::ALL
            .into_iter()
            .find(|quote| *quote != Quote::default())
            .unwrap();
        for event in [
            Event::Search("NOT_LISTED".into()),
            Event::Quote(other_quote),
        ] {
            let mut selector = Selector::default();
            let saved = Market::default();
            let _ = selector.show_exact(&saved);
            selector.update(event);
            selector.set_catalog(
                saved.exchange,
                saved.product,
                Ok(catalog(saved.exchange, saved.product)),
            );
            assert!(selector.take_ready_quick().is_none());
            assert!(selector.update(Event::Submit).is_none());
        }
    }

    #[test]
    fn interval_repair_before_catalog_arrives_preserves_the_exact_symbol() {
        let mut selector = Selector::default();
        let saved = Market {
            interval: Timeframe::Min60,
            ..Market::default()
        };
        let _ = selector.show_exact(&saved);
        selector.update(Event::StepInterval(1));
        let mut native = (*catalog(saved.exchange, saved.product)).clone();
        native.intervals = vec![Timeframe::Min1, Timeframe::Min5];
        selector.set_catalog(saved.exchange, saved.product, Ok(Arc::new(native)));
        assert!(selector.take_ready_quick().is_none());
        assert!(matches!(
            selector.update(Event::Submit),
            Some(Action::SelectMarket(market)) if market == Market::default()
        ));
    }
}

use crate::{Message, state::Action};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Element, Length, Task, keyboard};
use mexc_model::{Catalog, Exchange, Instrument, Market, Product, Quote, Timeframe};
use std::sync::Arc;

const SEARCH_ID: &str = "market-search";
const RESULTS_ID: &str = "market-results";
const RESULT_HEIGHT: f32 = 68.0;
const RESULT_SPACING: f32 = 4.0;

#[derive(Debug, Clone)]
pub enum Event {
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
}

impl Selector {
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
        self.search_key.clear();
        self.refresh_results();
        Task::batch([self.focus_search(), self.follow_selection()])
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
                if !catalog.intervals.contains(&self.interval)
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
                self.step_interval(self.pending_interval_steps);
                self.pending_interval_steps = 0;
                self.refresh_results();
            }
            Err(error) => {
                self.clear_catalog();
                self.catalog_error = Some(error);
            }
        }
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
        if self.current_catalog().is_some()
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
            Event::Search(value) => {
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
                }
            }
            Event::Interval(interval) => {
                if self.intervals().contains(&interval) {
                    self.interval = interval;
                }
            }
            Event::StepInterval(steps) => self.step_interval(steps),
            Event::Exchange(exchange) => {
                if exchange != self.exchange {
                    self.exchange = exchange;
                    self.clear_catalog();
                    self.pending_interval_steps = 0;
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
                    self.catalog_error = None;
                    self.loading = false;
                    self.selected = None;
                }
            }
            Event::Quote(quote) => {
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

        if modifiers.control() {
            match key {
                Key::Character("q" | "Q") => {
                    let index = Quote::ALL
                        .iter()
                        .position(|quote| *quote == self.quote)
                        .unwrap_or(0);
                    return Some(Event::Quote(Quote::ALL[(index + 1) % Quote::ALL.len()]));
                }
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
                button("Keys [F1]").on_press(Message::HelpOpen)
            ]
            .align_y(iced::Alignment::Center),
            text("Native exchange markets").size(13),
        ]
        .spacing(4);
        let intervals = self.intervals();
        let interval_supported = intervals.contains(&self.interval);
        let mut body = column![
            text_input("Search symbol or base coin", &self.query)
                .id(SEARCH_ID)
                .on_input(|value| Message::Selector(Event::Search(value)))
                .on_submit(Message::Selector(Event::Submit))
                .padding(7),
            row![
                text("Exchange"),
                pick_list(Exchange::ALL, Some(self.exchange), |value| {
                    Message::Selector(Event::Exchange(value))
                }),
            ]
            .spacing(8),
            row![
                text("Product"),
                pick_list(Product::ALL, Some(self.product), |value| {
                    Message::Selector(Event::Product(value))
                }),
            ]
            .spacing(8),
            row![
                text("Quote"),
                pick_list(Quote::ALL, Some(self.quote), |value| {
                    Message::Selector(Event::Quote(value))
                }),
            ]
            .spacing(8),
            row![
                text("Native interval"),
                pick_list(
                    intervals,
                    interval_supported.then_some(self.interval),
                    |value| { Message::Selector(Event::Interval(value)) }
                ),
            ]
            .spacing(8),
            text("Alt+Left/Right exchange · Alt+Up/Down product · Ctrl+Q quote · Ctrl+Left/Right interval").size(12),
        ]
        .spacing(8)
        .height(Length::Fill);

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
                    text(format!(
                        "{} · {}\nquote {} · settles {}\nvolume {}",
                        instrument.symbol,
                        kind,
                        instrument.quote_coin,
                        instrument.settle_coin,
                        instrument.volume_unit,
                    ))
                    .size(14),
                )
                .on_press(Message::Selector(Event::Select(instrument.symbol.clone())))
                .style(if selected {
                    button::primary
                } else {
                    button::secondary
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
                    "No matching native markets. Clear the search or change quote."
                })
                .size(13),
            );
        }
        body = body
            .push(text(format!("{count} matching markets")).size(12))
            .push(
                scrollable(results)
                    .id(scrollable::Id::new(RESULTS_ID))
                    .width(Length::Fill)
                    .height(Length::Fill),
            );

        let mut footer = column![
            text(match &self.selected {
                Some(symbol) => format!("Selection: {symbol} · {}", self.interval),
                None => format!("No market selected · {}", self.interval),
            })
            .size(13),
        ]
        .spacing(6);
        if self.pending_interval_steps != 0 {
            footer = footer
                .push(text("Interval navigation queued until native intervals arrive").size(12));
        }
        if let Some(error) = &self.catalog_error {
            footer = footer
                .push(text(format!("Catalog: {error}")).size(12))
                .push(button("Retry catalog").on_press(Message::Selector(Event::Retry)));
        }
        if let Some(error) = &self.error {
            footer = footer.push(text(error).size(12));
        }
        let apply = button("Apply");
        let apply = if !self.loading
            && self.catalog_error.is_none()
            && self.selected.is_some()
            && interval_supported
        {
            apply.on_press(Message::Selector(Event::Submit))
        } else {
            apply
        };
        footer = footer
            .push(
                row![
                    button("Cancel").on_press(Message::Action(Action::ManageMarket)),
                    apply,
                ]
                .spacing(8),
            )
            .push(text("Up/Down select · Enter apply · Esc cancel · Ctrl+R retry").size(12));

        container(column![header, body, footer].spacing(12))
            .padding(10)
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
}

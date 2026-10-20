use crate::ui;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{
    button, checkbox, column, container, pick_list, row, scrollable, text, text_input,
};
use iced::{Alignment, Element, Length, Task};
use mexc_indicators::profiles::{Instance, Kind, Period, Range, Snapshot, Spec, VolumeMethod};
use std::collections::BTreeMap;

const SCROLL_ID: &str = "profile-controls-scroll";

fn cycle_index(index: usize, length: usize, forward: bool) -> usize {
    if forward {
        (index + 1) % length
    } else {
        (index + length - 1) % length
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeMode {
    Session,
    Fixed,
    Visible,
}
impl std::fmt::Display for RangeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Session => "Calendar / custom session",
            Self::Fixed => "Fixed range",
            Self::Visible => "Visible range",
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    BlockMinutes,
    TicksPerRow,
    ValueArea,
    Width,
    Start,
    End,
    Timezone,
    SessionStart,
    SessionEnd,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Histogram,
    Levels,
    Previous,
    Naked,
    SinglePrints,
    InitialBalance,
    Letters,
    PoorExtremes,
}
#[derive(Debug, Clone)]
pub enum Event {
    Open(Option<u64>),
    QuickAdd(Kind),
    New(Kind),
    Select(u64),
    Apply,
    Delete,
    Remove(u64),
    ToggleVisible(u64),
    Rebin,
    Retry,
    PlaceRange,
    Close,
    Kind(Kind),
    Mode(RangeMode),
    Period(Period),
    Edit(Field, String),
    Toggle(Flag, bool),
    EstimateMissing(bool),
    Focus { backwards: bool },
    Scroll(f32),
    RangePoint(i64),
    RangeCursor(i64),
    RangeCancel,
}
#[derive(Debug, Clone)]
pub enum Command {
    Add(Spec),
    Apply { id: u64, spec: Spec },
    Delete(u64),
    SetVisible { id: u64, visible: bool },
    Rebin(u64),
    Retry,
    PlaceRange { id: Option<u64>, spec: Spec },
    Focus { backwards: bool },
    Scroll(f32),
}
#[derive(Debug, Clone)]
pub struct Controls {
    pub open: bool,
    selected: Option<u64>,
    spec: Spec,
    mode: RangeMode,
    period: Period,
    block: String,
    ticks: String,
    value_area: String,
    width: String,
    start: String,
    end: String,
    timezone: String,
    session_start: String,
    session_end: String,
    error: Option<String>,
    statuses: BTreeMap<u64, String>,
}
impl Default for Controls {
    fn default() -> Self {
        let spec = Spec::default_for(Kind::Tpo);
        let mut controls = Self {
            open: false,
            selected: None,
            spec,
            mode: RangeMode::Session,
            period: Period::Day,
            block: String::new(),
            ticks: String::new(),
            value_area: String::new(),
            width: String::new(),
            start: String::new(),
            end: String::new(),
            timezone: "UTC".into(),
            session_start: "00:00".into(),
            session_end: "00:00".into(),
            error: None,
            statuses: BTreeMap::new(),
        };
        controls.load(None, Spec::default_for(Kind::Tpo));
        controls
    }
}
impl Controls {
    fn load(&mut self, id: Option<u64>, spec: Spec) {
        self.selected = id;
        self.block = spec.block_minutes.to_string();
        self.ticks = spec.ticks_per_row.to_string();
        self.value_area = spec.value_area_percent.to_string();
        self.width = spec.display.width_percent.to_string();
        self.period = Period::Day;
        self.timezone = "UTC".into();
        self.session_start = "00:00".into();
        self.session_end = "00:00".into();
        self.start.clear();
        self.end.clear();
        self.mode = match spec.range {
            Range::Session(period) => {
                self.period = period;
                if let Period::Custom {
                    timezone,
                    start_minute,
                    end_minute,
                } = period
                {
                    self.timezone = timezone.name().into();
                    self.session_start =
                        format!("{:02}:{:02}", start_minute / 60, start_minute % 60);
                    self.session_end = format!("{:02}:{:02}", end_minute / 60, end_minute % 60);
                }
                RangeMode::Session
            }
            Range::Fixed { start_ms, end_ms } => {
                self.start = format_time(start_ms);
                self.end = format_time(end_ms);
                RangeMode::Fixed
            }
            Range::Visible => RangeMode::Visible,
        };
        self.spec = spec;
        self.error = None;
    }
    pub fn show(&mut self, instances: &[Instance], id: Option<u64>) {
        self.open = true;
        let instance = id
            .and_then(|id| instances.iter().find(|item| item.id == id))
            .or_else(|| {
                if id.is_none() {
                    instances.first()
                } else {
                    None
                }
            });
        if let Some(instance) = instance {
            self.load(Some(instance.id), instance.spec.clone());
        } else {
            self.load(None, Spec::default_for(Kind::Tpo));
        }
    }
    pub fn set_status(&mut self, id: u64, status: Option<String>) {
        if let Some(status) = status {
            self.statuses.insert(id, status);
        } else {
            self.statuses.remove(&id);
        }
    }
    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
        self.open = true;
    }
    pub fn applied(&mut self, instances: &[Instance], id: u64) {
        if let Some(instance) = instances.iter().find(|instance| instance.id == id) {
            self.load(Some(id), instance.spec.clone());
        }
    }
    pub fn suggest_rows(&mut self, ticks: u32) {
        if self.selected.is_none() {
            self.spec.ticks_per_row = ticks;
            self.ticks = ticks.to_string();
        }
    }
    pub fn focus<Message: 'static>(&self) -> Task<Message> {
        let id = if self.spec.kind == Kind::Tpo {
            "profile-block"
        } else {
            "profile-ticks"
        };
        Task::batch([
            iced::widget::text_input::focus(id),
            iced::widget::text_input::select_all(id),
        ])
    }

    pub fn focus_visible<Message: Send + 'static>(&self) -> Task<Message> {
        self.focus().chain(Self::reveal_focus())
    }

    pub fn reveal_focus<Message: Send + 'static>() -> Task<Message> {
        ui::reveal_focus(SCROLL_ID)
    }
    fn draft(&self, require_range: bool) -> Result<Spec, String> {
        let mut spec = self.spec.clone();
        spec.block_minutes = if spec.kind == Kind::Tpo {
            self.block
                .parse()
                .map_err(|_| "Block duration must be a positive whole number of minutes")?
        } else {
            30
        };
        spec.ticks_per_row = self
            .ticks
            .parse()
            .map_err(|_| "Ticks per row must be a positive whole number")?;
        spec.value_area_percent = self
            .value_area
            .parse()
            .map_err(|_| "Value area must be a percentage from 1 to 100")?;
        spec.display.width_percent = self
            .width
            .parse()
            .map_err(|_| "Width must be a percentage from 1 to 100")?;
        spec.range = match self.mode {
            RangeMode::Session => Range::Session(if matches!(self.period, Period::Custom { .. }) {
                Period::custom(&self.timezone, &self.session_start, &self.session_end)?
            } else {
                self.period
            }),
            RangeMode::Visible => Range::Visible,
            RangeMode::Fixed if require_range => Range::Fixed {
                start_ms: parse_time(&self.start)?,
                end_ms: parse_time(&self.end)?,
            },
            RangeMode::Fixed => Range::Visible,
        };
        spec.validate()?;
        Ok(spec)
    }
    pub fn update(&mut self, event: Event, instances: &[Instance]) -> Option<Command> {
        match event {
            Event::Open(id) => self.show(instances, id),
            Event::QuickAdd(kind) => return Some(Command::Add(Spec::default_for(kind))),
            Event::New(kind) => {
                self.open = true;
                self.load(None, Spec::default_for(kind));
            }
            Event::Select(id) => {
                if let Some(item) = instances.iter().find(|item| item.id == id) {
                    self.load(Some(id), item.spec.clone());
                }
            }
            Event::Close => self.open = false,
            Event::Kind(kind) => self.spec.kind = kind,
            Event::Mode(mode) => self.mode = mode,
            Event::Period(period) => self.period = period,
            Event::EstimateMissing(enabled) => self.spec.estimate_missing = enabled,
            Event::Edit(field, value) => {
                *match field {
                    Field::BlockMinutes => &mut self.block,
                    Field::TicksPerRow => &mut self.ticks,
                    Field::ValueArea => &mut self.value_area,
                    Field::Width => &mut self.width,
                    Field::Start => &mut self.start,
                    Field::End => &mut self.end,
                    Field::Timezone => &mut self.timezone,
                    Field::SessionStart => &mut self.session_start,
                    Field::SessionEnd => &mut self.session_end,
                } = value;
                self.error = None;
            }
            Event::Toggle(flag, value) => {
                *match flag {
                    Flag::Histogram => &mut self.spec.display.histogram,
                    Flag::Levels => &mut self.spec.display.levels,
                    Flag::Previous => &mut self.spec.display.previous,
                    Flag::Naked => &mut self.spec.display.naked,
                    Flag::SinglePrints => &mut self.spec.display.single_prints,
                    Flag::InitialBalance => &mut self.spec.display.initial_balance,
                    Flag::Letters => &mut self.spec.display.letters,
                    Flag::PoorExtremes => &mut self.spec.display.poor_extremes,
                } = value;
            }
            Event::Apply | Event::PlaceRange => {
                let placement = matches!(event, Event::PlaceRange);
                if self
                    .selected
                    .is_some_and(|id| !instances.iter().any(|item| item.id == id))
                {
                    self.error = Some(
                        "This profile was removed. Select an existing profile or create a new one."
                            .into(),
                    );
                    return None;
                }
                match self.draft(!placement) {
                    Ok(spec) => {
                        if placement {
                            self.open = false;
                        }
                        return Some(if placement {
                            Command::PlaceRange {
                                id: self.selected,
                                spec,
                            }
                        } else if let Some(id) = self.selected {
                            Command::Apply { id, spec }
                        } else {
                            Command::Add(spec)
                        });
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Event::Delete => {
                if let Some(id) = self
                    .selected
                    .filter(|id| instances.iter().any(|item| item.id == *id))
                {
                    self.load(None, Spec::default_for(self.spec.kind));
                    return Some(Command::Delete(id));
                }
            }
            Event::Remove(id) => {
                if instances.iter().any(|item| item.id == id) {
                    if self.selected == Some(id) {
                        self.load(None, Spec::default_for(self.spec.kind));
                    }
                    return Some(Command::Delete(id));
                }
            }
            Event::ToggleVisible(id) => {
                if let Some(item) = instances.iter().find(|item| item.id == id) {
                    return Some(Command::SetVisible {
                        id,
                        visible: !item.visible,
                    });
                }
            }
            Event::Rebin => {
                if let Some(id) = self
                    .selected
                    .filter(|id| instances.iter().any(|item| item.id == *id))
                {
                    return Some(Command::Rebin(id));
                }
                self.error = Some(
                    "Add the profile before rebinning from the current native price reference."
                        .into(),
                );
            }
            Event::Retry => return Some(Command::Retry),
            Event::Focus { backwards } => return Some(Command::Focus { backwards }),
            Event::Scroll(amount) => return Some(Command::Scroll(amount)),
            Event::RangePoint(_) | Event::RangeCursor(_) | Event::RangeCancel => {}
        }
        None
    }
    pub fn key(
        &self,
        key: Key<&str>,
        modifiers: Modifiers,
        instances: &[Instance],
    ) -> Option<Event> {
        if !self.open {
            return None;
        }
        if matches!(key, Key::Named(Named::Escape)) {
            return Some(Event::Close);
        }
        if modifiers.logo() {
            return None;
        }
        let forward = matches!(key, Key::Named(Named::ArrowRight | Named::ArrowDown));
        if modifiers.alt() {
            if modifiers.control() {
                return None;
            }
            return match key {
                Key::Named(Named::ArrowLeft | Named::ArrowRight) => {
                    let modes = [RangeMode::Session, RangeMode::Fixed, RangeMode::Visible];
                    let index = modes
                        .iter()
                        .position(|mode| *mode == self.mode)
                        .unwrap_or(0);
                    Some(Event::Mode(modes[cycle_index(index, modes.len(), forward)]))
                }
                Key::Named(Named::ArrowUp | Named::ArrowDown)
                    if self.mode == RangeMode::Session =>
                {
                    let index = match self.period {
                        Period::Day => 0,
                        Period::Week => 1,
                        Period::Month => 2,
                        Period::Custom { .. } => 3,
                    };
                    Some(Event::Period(
                        Period::ALL[cycle_index(index, Period::ALL.len(), forward)],
                    ))
                }
                _ => None,
            };
        }
        if modifiers.control() {
            return match key {
                Key::Named(Named::ArrowLeft | Named::ArrowRight) => {
                    Some(Event::Kind(if self.spec.kind == Kind::Tpo {
                        Kind::Volume
                    } else {
                        Kind::Tpo
                    }))
                }
                Key::Named(Named::ArrowUp | Named::ArrowDown) if !instances.is_empty() => {
                    let index = self
                        .selected
                        .and_then(|id| instances.iter().position(|item| item.id == id))
                        .unwrap_or(if forward { instances.len() - 1 } else { 0 });
                    Some(Event::Select(
                        instances[cycle_index(index, instances.len(), forward)].id,
                    ))
                }
                Key::Character(value)
                    if value.len() == 1 && matches!(value.as_bytes()[0], b'1'..=b'8') =>
                {
                    let (_, flag, enabled) =
                        self.options()[usize::from(value.as_bytes()[0] - b'1')];
                    self.option_available(flag)
                        .then_some(Event::Toggle(flag, !enabled))
                }
                Key::Character("9") if self.spec.kind == Kind::Volume => {
                    Some(Event::EstimateMissing(!self.spec.estimate_missing))
                }
                Key::Character("n" | "N") => Some(Event::New(self.spec.kind)),
                Key::Character("s" | "S") | Key::Named(Named::Enter) => Some(Event::Apply),
                Key::Character("d" | "D") => Some(Event::Delete),
                Key::Character("h" | "H") => self.selected.map(Event::ToggleVisible),
                Key::Character("r" | "R") if modifiers.shift() => Some(Event::Retry),
                Key::Character("r" | "R") => Some(Event::Rebin),
                Key::Character("p" | "P") => Some(Event::PlaceRange),
                _ => None,
            };
        }
        match key {
            Key::Named(Named::Tab) => Some(Event::Focus {
                backwards: modifiers.shift(),
            }),
            Key::Named(Named::PageUp) => Some(Event::Scroll(-240.0)),
            Key::Named(Named::PageDown) => Some(Event::Scroll(240.0)),
            _ => None,
        }
    }

    fn option_available(&self, flag: Flag) -> bool {
        !(self.spec.kind == Kind::Volume
            && matches!(
                flag,
                Flag::SinglePrints | Flag::InitialBalance | Flag::Letters | Flag::PoorExtremes
            ))
            && (self.mode == RangeMode::Session || !matches!(flag, Flag::Previous | Flag::Naked))
    }

    fn options(&self) -> [(&'static str, Flag, bool); 8] {
        let d = &self.spec.display;
        [
            (
                "Histogram / TPO blocks · Ctrl+1",
                Flag::Histogram,
                d.histogram,
            ),
            ("POC / VAH / VAL · Ctrl+2", Flag::Levels, d.levels),
            (
                "Previous session levels · Ctrl+3",
                Flag::Previous,
                d.previous,
            ),
            ("Naked POCs and touch status · Ctrl+4", Flag::Naked, d.naked),
            (
                "Single Prints and tails · Ctrl+5",
                Flag::SinglePrints,
                d.single_prints,
            ),
            (
                "Initial Balance · first two blocks · Ctrl+6",
                Flag::InitialBalance,
                d.initial_balance,
            ),
            ("Expanded TPO letters · Ctrl+7", Flag::Letters, d.letters),
            (
                "Poor highs / lows · Ctrl+8",
                Flag::PoorExtremes,
                d.poor_extremes,
            ),
        ]
    }
    pub fn scroll<Message: 'static>(amount: f32) -> Task<Message> {
        scrollable::scroll_by(
            scrollable::Id::new(SCROLL_ID),
            scrollable::AbsoluteOffset { x: 0.0, y: amount },
        )
    }
    pub fn view<'a>(
        &'a self,
        instances: &'a [Instance],
        outputs: &'a BTreeMap<u64, Vec<Snapshot>>,
    ) -> Element<'a, Event> {
        let mut footer = column![].spacing(6);
        if let Some(error) = &self.error {
            footer = footer.push(text(error).size(13).style(ui::negative_text));
        }
        footer = footer.push(
            row![
                button(if self.selected.is_some() {
                    "Apply · Ctrl+S"
                } else {
                    "Add · Ctrl+S"
                })
                .on_press(Event::Apply)
                .style(ui::primary),
                button("Place range · Ctrl+P")
                    .on_press(Event::PlaceRange)
                    .style(ui::quiet),
                button("Rebin · Ctrl+R")
                    .on_press_maybe(self.selected.map(|_| Event::Rebin))
                    .style(ui::quiet),
                button("Retry sources")
                    .on_press(Event::Retry)
                    .style(ui::quiet),
                button("Delete · Ctrl+D")
                    .on_press_maybe(self.selected.map(|_| Event::Delete))
                    .style(ui::danger),
            ]
            .spacing(6)
            .wrap(),
        );
        column![
            row![
                text("TPO / Volume Profile").size(19),
                iced::widget::Space::with_width(Length::Fill),
                button("Close · Esc")
                    .on_press(Event::Close)
                    .style(ui::quiet)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            iced::widget::responsive(move |size| self.body(instances, outputs, size.width)),
            separator(),
            footer,
            text("Ctrl+Left/Right type · Ctrl+Up/Down instance · Alt+Left/Right range · Alt+Up/Down session")
                .size(11)
                .style(ui::muted_text),
            text("Tab fields · PageUp/Down scroll · Ctrl+1–8 options · Ctrl+9 volume estimates · Ctrl+H show/hide · Ctrl+Shift+R retry")
                .size(11).style(ui::muted_text),
        ]
        .spacing(10)
        .height(Length::Fill)
        .into()
    }

    fn body<'a>(
        &'a self,
        instances: &'a [Instance],
        outputs: &'a BTreeMap<u64, Vec<Snapshot>>,
        width: f32,
    ) -> Element<'a, Event> {
        let mut list = column![
            row![
                button("New TPO")
                    .on_press(Event::New(Kind::Tpo))
                    .style(ui::quiet),
                button("New Volume")
                    .on_press(Event::New(Kind::Volume))
                    .style(ui::quiet),
            ]
            .spacing(6)
        ]
        .spacing(8);
        for item in instances {
            let title = format!(
                "#{} · {} · {}",
                item.id,
                item.spec.kind,
                range_label(&item.spec.range)
            );
            list = list.push(
                column![
                    button(text(title).size(13))
                        .width(Length::Fill)
                        .on_press(Event::Select(item.id))
                        .style(if self.selected == Some(item.id) {
                            ui::selected
                        } else {
                            ui::quiet
                        }),
                    button(if item.visible { "Hide" } else { "Show" })
                        .on_press(Event::ToggleVisible(item.id))
                        .style(ui::quiet),
                ]
                .spacing(3),
            );
        }
        if instances.is_empty() {
            list = list.push(
                text("No profiles. Choose New TPO or New Volume.")
                    .size(12)
                    .style(ui::muted_text),
            );
        }
        let mut editor = column![
            text(
                self.selected
                    .map_or_else(|| "New profile".into(), |id| format!("Settings · #{id}"))
            )
            .size(17),
            labeled(
                "Profile type",
                pick_list([Kind::Tpo, Kind::Volume], Some(self.spec.kind), Event::Kind)
                    .style(ui::pick)
            ),
            labeled(
                "Range",
                pick_list(
                    [RangeMode::Session, RangeMode::Fixed, RangeMode::Visible],
                    Some(self.mode),
                    Event::Mode
                )
                .style(ui::pick)
            ),
        ]
        .spacing(9)
        .width(Length::Fill);
        match self.mode {
            RangeMode::Session => {
                editor = editor.push(labeled(
                    "Session",
                    pick_list(
                        Period::ALL,
                        Some(if matches!(self.period, Period::Custom { .. }) { Period::ALL[3] } else { self.period }),
                        Event::Period,
                    )
                    .style(ui::pick),
                ));
                if matches!(self.period, Period::Custom { .. }) {
                    editor = editor
                        .push(input("IANA timezone", &self.timezone, Field::Timezone, "America/New_York"))
                        .push(input("Local open · inclusive", &self.session_start, Field::SessionStart, "09:30"))
                        .push(input("Local close · exclusive", &self.session_end, Field::SessionEnd, "16:00"))
                        .push(text("Daily local windows. Close ≤ open runs overnight; equal times mean a full local day. DST: ambiguous opens use the earliest instant, closes the latest; nonexistent minutes advance to the first valid minute. Collapsed empty windows are skipped; the latest open wins in overlaps.").size(12).style(ui::muted_text));
                }
            }
            RangeMode::Fixed => {
                editor = editor.push(input("Start UTC · inclusive", &self.start, Field::Start, "YYYY-MM-DD HH:MM"))
                    .push(input("End UTC · exclusive", &self.end, Field::End, "YYYY-MM-DD HH:MM"))
                    .push(text(if self.spec.kind == Kind::Tpo {
                        "TPO bounds must align to whole UTC minutes; fixed ranges bind to this exact instrument."
                    } else {
                        "UTC bounds accept HH:MM or HH:MM:SS.mmm; native executions use exact inclusive/exclusive millisecond bounds."
                    }).size(12).style(ui::muted_text));
            }
            RangeMode::Visible => editor = editor.push(
                text("Uses the visible time range without changing price bounds. TPO uses only whole native minutes inside that range.")
                    .size(12)
                    .style(ui::muted_text),
            ),
        }
        if self.spec.kind == Kind::Tpo {
            editor = editor.push(input(
                "TPO block · minutes",
                &self.block,
                Field::BlockMinutes,
                "30",
            ));
        }
        editor = editor
            .push(input(
                "Native ticks per row",
                &self.ticks,
                Field::TicksPerRow,
                "1",
            ))
            .push(input(
                "Value area · percent",
                &self.value_area,
                Field::ValueArea,
                "70",
            ))
            .push(input(
                "Histogram width · percent",
                &self.width,
                Field::Width,
                "20",
            ));
        if self.spec.kind == Kind::Volume {
            editor = editor.push(
                checkbox("Estimate missing executions · Ctrl+9", self.spec.estimate_missing)
                    .on_toggle(Event::EstimateMissing).size(14)
            ).push(text("Automatic fallback spreads available native 1-minute candle quantity uniformly over its price rows. Incomplete execution minutes are replaced, not added twice. Estimated / Mixed values are approximations; disable for exact-only data. Missing candles remain unavailable.")
                .width(Length::Fill).size(12).style(ui::muted_text));
        }
        for (label, flag, value) in self.options() {
            if !self.option_available(flag) {
                continue;
            }
            editor = editor.push(
                checkbox(label, value)
                    .on_toggle(move |value| Event::Toggle(flag, value))
                    .size(14),
            );
        }
        editor = editor.push(text("Previous and naked levels require certified native coverage. TPO uses native 1-minute bar ranges independently of the chart interval. Volume prefers priced native executions; candle fallback is explicitly labeled Estimated / Mixed and never certifies exact coverage.").width(Length::Fill).size(12).style(ui::muted_text));
        if let Some(id) = self.selected {
            let applied_kind = instances
                .iter()
                .find(|item| item.id == id)
                .map_or(self.spec.kind, |item| item.spec.kind);
            editor = editor
                .push(separator())
                .push(text("Applied profile / provenance").size(15));
            if let Some(item) = instances.iter().find(|item| item.id == id) {
                editor = editor.push(
                    text(match &item.instrument {
                        Some(instrument) => format!(
                            "Bound instrument: {} · {:?} · {:?}",
                            instrument.symbol, instrument.exchange, instrument.product
                        ),
                        None => "Instrument: follows the active chart".into(),
                    })
                    .size(12),
                );
            }
            if let Some(status) = self.statuses.get(&id) {
                editor = editor.push(
                    text(status)
                        .width(Length::Fill)
                        .size(12)
                        .style(ui::muted_text),
                );
            }
            if let Some(snapshots) = outputs.get(&id).filter(|items| !items.is_empty()) {
                for snapshot in snapshots.iter().rev() {
                    editor = editor.push(
                        text(format!(
                            "{} → {} UTC · {} rows · {} · {}",
                            format_time(snapshot.start_ms),
                            format_time(snapshot.end_ms),
                            snapshot.rows.len(),
                            if snapshot.as_of_ms < snapshot.end_ms {
                                "developing"
                            } else {
                                "closed"
                            },
                            if snapshot.covered {
                                if applied_kind == Kind::Tpo && !snapshot.complete {
                                    "closed minutes covered; live minute provisional"
                                } else {
                                    "covered through as-of"
                                }
                            } else {
                                "partial / unproved coverage"
                            }
                        ))
                        .size(12)
                        .width(Length::Fill),
                    );
                    editor = editor.push(
                        text(format!(
                            "As of {} · POC {} · VAH {} · VAL {}",
                            format_time(snapshot.as_of_ms),
                            value(snapshot.poc),
                            value(snapshot.vah),
                            value(snapshot.val)
                        ))
                        .size(12)
                        .width(Length::Fill),
                    );
                    if let Some(reason) = &snapshot.reason {
                        editor = editor.push(
                            text(reason)
                                .width(Length::Fill)
                                .size(12)
                                .style(ui::muted_text),
                        );
                    }
                    if applied_kind == Kind::Volume {
                        editor = editor.push(
                            text(match snapshot.native_volume {
                                Some(quantity) => format!(
                                    "{}: {}",
                                    match snapshot.volume_method {
                                        VolumeMethod::Estimated => "Estimated quantity",
                                        VolumeMethod::Mixed => "Mixed exact / estimated quantity",
                                        VolumeMethod::Exact if snapshot.covered =>
                                            "Native quantity",
                                        VolumeMethod::Exact => "Observed native quantity",
                                    },
                                    quantity.normalize()
                                ),
                                None => "Native quantity unavailable".into(),
                            })
                            .size(12)
                            .width(Length::Fill),
                        );
                    } else {
                        editor = editor.push(
                            text(format!(
                                "{} Single Print zones · {} tail zones · poor high {} / low {}",
                                snapshot.single_prints.len(),
                                snapshot.tails.len(),
                                snapshot.poor_high,
                                snapshot.poor_low
                            ))
                            .size(12)
                            .width(Length::Fill),
                        );
                    }
                }
            } else {
                editor = editor.push(text("No computed rows available. Coverage and source status above determine availability; estimates require available native candles and tick metadata.").size(12).style(ui::muted_text));
            }
        }
        if width >= 600.0 {
            row![
                scrollable(list)
                    .id(scrollable::Id::new("profile-list"))
                    .width(Length::FillPortion(1)),
                scrollable(
                    container(editor)
                        .padding(iced::Padding {
                            right: 12.0,
                            ..iced::Padding::default()
                        })
                        .width(Length::Fill)
                )
                .id(scrollable::Id::new(SCROLL_ID))
                .width(Length::FillPortion(2)),
            ]
            .spacing(16)
            .height(Length::Fill)
            .into()
        } else {
            scrollable(
                container(column![list, separator(), editor].spacing(12))
                    .padding(iced::Padding {
                        right: 12.0,
                        ..iced::Padding::default()
                    })
                    .width(Length::Fill),
            )
            .id(scrollable::Id::new(SCROLL_ID))
            .height(Length::Fill)
            .into()
        }
    }
}
fn separator<'a>() -> Element<'a, Event> {
    container(iced::widget::Space::new(Length::Fill, 1))
        .style(|theme: &iced::Theme| container::Style {
            background: Some(ui::border_color(theme).into()),
            ..container::Style::default()
        })
        .into()
}
fn labeled<'a>(label: &'static str, control: impl Into<Element<'a, Event>>) -> Element<'a, Event> {
    column![text(label).size(12).style(ui::muted_text), control.into()]
        .spacing(4)
        .into()
}
fn input<'a>(
    label: &'static str,
    value: &'a str,
    field: Field,
    placeholder: &'a str,
) -> Element<'a, Event> {
    let id = match field {
        Field::BlockMinutes => "profile-block",
        Field::TicksPerRow => "profile-ticks",
        Field::ValueArea => "profile-value-area",
        Field::Width => "profile-width",
        Field::Start => "profile-start",
        Field::End => "profile-end",
        Field::Timezone => "profile-timezone",
        Field::SessionStart => "profile-session-start",
        Field::SessionEnd => "profile-session-end",
    };
    labeled(
        label,
        text_input(placeholder, value)
            .id(id)
            .on_input(move |value| Event::Edit(field, value))
            .on_submit(Event::Apply)
            .padding(7)
            .style(ui::input),
    )
}
fn value(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), |value| value.to_string())
}
fn format_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map_or_else(
        || ms.to_string(),
        |time| {
            time.format(if ms.rem_euclid(60_000) == 0 {
                "%Y-%m-%d %H:%M"
            } else {
                "%Y-%m-%d %H:%M:%S%.3f"
            })
            .to_string()
        },
    )
}
fn parse_time(value: &str) -> Result<i64, String> {
    let time = [
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d %H:%M:%S%.3f",
        "%Y-%m-%d %H:%M:%S",
    ]
    .into_iter()
    .find_map(|format| chrono::NaiveDateTime::parse_from_str(value.trim(), format).ok())
    .ok_or_else(|| "Enter UTC bounds as YYYY-MM-DD HH:MM or YYYY-MM-DD HH:MM:SS.mmm".to_string())?
    .and_utc();
    if time.timestamp_subsec_nanos() % 1_000_000 != 0 {
        return Err("UTC bounds support milliseconds, not sub-millisecond precision".into());
    }
    Ok(time.timestamp_millis())
}
fn range_label(range: &Range) -> String {
    match range {
        Range::Session(period) => format!("{period}"),
        Range::Fixed { .. } => "Fixed".into(),
        Range::Visible => "Visible".into(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn instance(id: u64) -> Instance {
        Instance {
            id,
            spec: Spec::default_for(Kind::Tpo),
            visible: true,
            instrument: None,
        }
    }
    #[test]
    fn duplicate_kinds_edit_and_delete_only_selected_identity() {
        let instances = [instance(4), instance(9)];
        let mut controls = Controls::default();
        controls.show(&instances, Some(9));
        controls.update(Event::Edit(Field::TicksPerRow, "8".into()), &instances);
        assert!(
            matches!(controls.update(Event::Apply, &instances), Some(Command::Apply { id: 9, spec }) if spec.ticks_per_row == 8)
        );
        controls.show(&instances, Some(4));
        assert!(matches!(
            controls.update(Event::Delete, &instances),
            Some(Command::Delete(4))
        ));
    }
    #[test]
    fn fixed_range_rejects_reversed_bounds_without_closing() {
        let mut controls = Controls::default();
        controls.show(&[], None);
        controls.update(Event::Mode(RangeMode::Fixed), &[]);
        controls.update(Event::Edit(Field::Start, "2026-10-17 12:00".into()), &[]);
        controls.update(Event::Edit(Field::End, "2026-10-17 11:00".into()), &[]);
        assert!(controls.update(Event::Apply, &[]).is_none());
        assert!(controls.open);
        assert!(controls.error.is_some());
        assert!(matches!(
            controls.update(Event::PlaceRange, &[]),
            Some(Command::PlaceRange { id: None, .. })
        ));
    }
    #[test]
    fn removed_selection_never_mutates_another_instance() {
        let mut controls = Controls::default();
        controls.show(&[instance(7)], Some(7));
        assert!(controls.update(Event::Apply, &[instance(8)]).is_none());
        assert!(controls.update(Event::Delete, &[instance(8)]).is_none());
    }
    #[test]
    fn committed_draft_refresh_preserves_closed_state_and_errors_reopen_editor() {
        let mut controls = Controls::default();
        let mut committed = instance(19);
        committed.spec.ticks_per_row = 24;
        controls.applied(&[committed], 19);
        assert!(!controls.open);
        assert_eq!(controls.selected, Some(19));
        assert_eq!(controls.ticks, "24");
        controls.set_error("Native tick metadata unavailable".into());
        assert!(controls.open);
        assert_eq!(
            controls.error.as_deref(),
            Some("Native tick metadata unavailable")
        );
    }
    #[test]
    fn direct_removal_does_not_depend_on_editor_selection() {
        let instances = [instance(7), instance(12)];
        let mut controls = Controls::default();
        controls.show(&instances, Some(7));
        assert!(matches!(
            controls.update(Event::Remove(12), &instances),
            Some(Command::Delete(12))
        ));
        assert_eq!(controls.selected, Some(7));
        assert!(controls.update(Event::Remove(99), &instances).is_none());
    }
    #[test]
    fn custom_session_validation_preserves_drafts_and_reloads_committed_fields() {
        let mut controls = Controls::default();
        controls.show(&[], None);
        controls.update(Event::Period(Period::ALL[3]), &[]);
        controls.update(Event::Edit(Field::Timezone, "Invalid/Zone".into()), &[]);
        controls.update(Event::Edit(Field::SessionStart, "9:30".into()), &[]);
        controls.update(Event::Edit(Field::SessionEnd, "16:00".into()), &[]);
        assert!(controls.update(Event::Apply, &[]).is_none());
        assert!(controls.open && controls.error.is_some());
        assert_eq!(controls.timezone, "Invalid/Zone");
        assert_eq!(controls.session_start, "9:30");
        controls.update(Event::Edit(Field::Timezone, "America/New_York".into()), &[]);
        assert!(controls.update(Event::Apply, &[]).is_none());
        controls.update(Event::Edit(Field::SessionStart, "09:30".into()), &[]);
        let Some(Command::Add(spec)) = controls.update(Event::Apply, &[]) else {
            panic!("valid custom draft must add a complete profile");
        };
        assert_eq!(
            spec.range,
            Range::Session(Period::custom("America/New_York", "09:30", "16:00").unwrap())
        );
        let mut saved = instance(31);
        saved.spec = spec;
        controls.applied(&[saved.clone()], 31);
        controls.show(&[saved], Some(31));
        assert_eq!(controls.timezone, "America/New_York");
        assert_eq!(controls.session_start, "09:30");
        assert_eq!(controls.session_end, "16:00");
    }
    #[test]
    fn custom_drafts_survive_mode_toggles_but_new_and_selection_reset_stale_fields() {
        let mut saved = instance(44);
        saved.spec.range =
            Range::Session(Period::custom("America/New_York", "18:00", "09:30").unwrap());
        let instances = [saved, instance(45)];
        let mut controls = Controls::default();
        controls.show(&instances, Some(44));
        controls.update(Event::Edit(Field::SessionStart, "19:15".into()), &instances);
        controls.update(Event::Mode(RangeMode::Visible), &instances);
        assert_eq!(controls.draft(true).unwrap().range, Range::Visible);
        controls.update(Event::Mode(RangeMode::Session), &instances);
        controls.update(Event::Period(Period::Week), &instances);
        assert_eq!(
            controls.draft(true).unwrap().range,
            Range::Session(Period::Week)
        );
        controls.update(Event::Period(Period::ALL[3]), &instances);
        assert_eq!(
            controls.draft(true).unwrap().range,
            Range::Session(Period::custom("America/New_York", "19:15", "09:30").unwrap())
        );
        controls.update(Event::Select(44), &instances);
        assert_eq!(controls.session_start, "18:00");
        controls.update(Event::Select(45), &instances);
        assert_eq!(controls.period, Period::Day);
        assert_eq!(controls.timezone, "UTC");
        assert_eq!(controls.session_start, "00:00");
        assert_eq!(controls.session_end, "00:00");
        controls.update(Event::Select(44), &instances);
        controls.update(Event::New(Kind::Volume), &instances);
        assert_eq!(controls.selected, None);
        assert_eq!(controls.period, Period::Day);
        assert_eq!(controls.timezone, "UTC");
        assert_eq!(controls.session_start, "00:00");
        assert_eq!(controls.session_end, "00:00");
        controls.update(Event::Period(Period::ALL[3]), &instances);
        let Some(Command::Add(spec)) = controls.update(Event::Apply, &instances) else {
            panic!("reset custom draft must create a full UTC day");
        };
        assert_eq!(
            spec.range,
            Range::Session(Period::custom("UTC", "00:00", "00:00").unwrap())
        );
    }

    #[test]
    fn changing_display_does_not_round_or_collapse_subminute_execution_ranges() {
        let mut saved = instance(8);
        saved.spec = Spec::default_for(Kind::Volume);
        saved.spec.range = Range::Fixed {
            start_ms: 1_792_310_400_123,
            end_ms: 1_792_310_400_456,
        };
        saved.instrument = Some(mexc_model::executions::NativeInstrument::from_market(
            &mexc_model::Market::default(),
        ));
        let instances = [saved];
        let mut controls = Controls::default();
        controls.show(&instances, Some(8));
        controls.update(Event::Toggle(Flag::Levels, false), &instances);
        let Some(Command::Apply { spec, .. }) = controls.update(Event::Apply, &instances) else {
            panic!("valid millisecond ranges must remain editable");
        };
        assert_eq!(spec.range, instances[0].spec.range);
        assert!(!spec.display.levels);
        assert!(parse_time("2026-10-18 08:00:00.123456").is_err());
    }

    #[test]
    fn successful_add_and_edit_keep_manager_open_but_placement_transfers_focus_to_chart() {
        let mut controls = Controls::default();
        controls.update(Event::New(Kind::Tpo), &[]);
        let Some(Command::Add(spec)) = controls.update(Event::Apply, &[]) else {
            panic!("new profile must add");
        };
        assert!(controls.open);
        let instances = [Instance {
            spec,
            ..instance(7)
        }];
        controls.applied(&instances, 7);
        assert_eq!(controls.selected, Some(7));
        assert!(matches!(
            controls.update(Event::Apply, &instances),
            Some(Command::Apply { id: 7, .. })
        ));
        assert!(controls.open);
        assert!(matches!(
            controls.update(Event::PlaceRange, &instances),
            Some(Command::PlaceRange { id: Some(7), .. })
        ));
        assert!(!controls.open);
    }
}

use crate::{Message, ui, workspace::ResetKind};
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Element, Length};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Open(ResetKind),
    Cancel,
    Confirm,
    RetrySave,
    ExitWithoutSaving,
}

#[derive(Debug, Clone)]
enum Surface {
    Confirmation {
        kind: ResetKind,
        market: String,
        indicators: usize,
        profiles: usize,
        drawings: usize,
    },
    SaveFailure(String),
}
#[derive(Debug, Clone, Default)]
pub struct Controls {
    surface: Option<Surface>,
    confirm_selected: bool,
}

impl Controls {
    pub fn show(
        &mut self,
        kind: ResetKind,
        market: &str,
        indicators: usize,
        profiles: usize,
        drawings: usize,
    ) {
        self.surface = Some(Surface::Confirmation {
            kind,
            market: market.to_owned(),
            indicators,
            profiles,
            drawings,
        });
        self.confirm_selected = false;
    }

    pub fn show_save_error(&mut self, error: &str) {
        self.surface = Some(Surface::SaveFailure(error.to_owned()));
        self.confirm_selected = true;
    }

    pub fn is_open(&self) -> bool {
        self.surface.is_some()
    }

    pub fn close(&mut self) {
        self.surface = None;
        self.confirm_selected = false;
    }

    pub fn key(&mut self, key: Key<&str>, modifiers: Modifiers, pending: bool) -> Option<Event> {
        let save_failure = matches!(self.surface.as_ref(), Some(Surface::SaveFailure(_)));
        if self.surface.is_none()
            || pending
            || modifiers.control()
            || modifiers.alt()
            || modifiers.logo()
        {
            return None;
        }
        match key {
            Key::Named(Named::Escape) if !save_failure => Some(Event::Cancel),
            Key::Named(Named::Tab) => {
                self.confirm_selected = !self.confirm_selected;
                None
            }
            Key::Named(Named::Enter) if save_failure && self.confirm_selected => {
                Some(Event::RetrySave)
            }
            Key::Named(Named::Enter) if save_failure => Some(Event::ExitWithoutSaving),
            Key::Named(Named::Enter) if self.confirm_selected => Some(Event::Confirm),
            Key::Named(Named::Enter) => Some(Event::Cancel),
            _ => None,
        }
    }

    pub fn view(&self, pending: bool, error: Option<&str>) -> Element<'_, Message> {
        let Some(surface) = self.surface.as_ref() else {
            return iced::widget::Space::new(Length::Shrink, Length::Shrink).into();
        };
        let (title, body, destructive, primary_label) = match surface {
            Surface::Confirmation { kind: ResetKind::Current, market, indicators, profiles, drawings } => (
                "Clear current chart",
                format!("Current market and timeframe: {market}. This removes {indicators} indicators, {profiles} chart-following or current-instrument profiles, and {drawings} current-instrument drawings across all intervals, including hidden and locked drawings. Other instruments' fixed profiles and drawing books, favorites, and recent markets are retained. This cannot be undone."),
                true,
                "Clear chart",
            ),
            Surface::Confirmation { kind: ResetKind::All, .. } => (
                "Reset everything",
                "Restore the default market and remove all saved indicators, profiles, and drawing books, including hidden and locked drawings. Favorites, recent markets, chart layout, and workspace preferences are also reset. This cannot be undone.".to_owned(),
                true,
                "Reset everything",
            ),
            Surface::SaveFailure(_) => (
                "Workspace save failed",
                "Workspace changes could not be saved. Retry before closing, or explicitly exit without saving.".to_owned(),
                false,
                "Retry save",
            ),
        };
        let save_failure = matches!(surface, Surface::SaveFailure(_));
        let error = error.map(str::to_owned).or_else(|| match surface {
            Surface::SaveFailure(error) => Some(error.clone()),
            _ => None,
        });
        let selected = self.confirm_selected;
        iced::widget::responsive(move |size| {
            let mut details =
                column![text(title).size(19), text(body.clone()).size(13)].spacing(10);
            if let Some(error) = error.as_ref() {
                details = details.push(ui::separator()).push(
                    text(format!("Error: {error}"))
                        .size(12)
                        .style(ui::negative_text),
                );
            }
            if pending {
                details = details.push(text("Saving workspace…").size(12).style(ui::muted_text));
            }
            let cancel = button(
                text(if save_failure {
                    "Exit without saving"
                } else {
                    "Cancel"
                })
                .size(13),
            )
            .style(if selected { ui::quiet } else { ui::selected })
            .on_press_maybe((!pending).then_some(if save_failure {
                Message::Workspace(Event::ExitWithoutSaving)
            } else {
                Message::Workspace(Event::Cancel)
            }))
            .padding([7, 12]);
            let confirm = button(text(primary_label).size(13))
                .style(if selected {
                    if destructive {
                        ui::selected
                    } else {
                        ui::primary
                    }
                } else if destructive {
                    ui::danger
                } else {
                    ui::quiet
                })
                .on_press_maybe((!pending).then_some(if save_failure {
                    Message::Workspace(Event::RetrySave)
                } else {
                    Message::Workspace(Event::Confirm)
                }))
                .padding([7, 12]);
            let panel_body = if size.width < 520.0 {
                column![details, ui::separator(), cancel, confirm].spacing(12)
            } else {
                column![
                    details,
                    ui::separator(),
                    row![cancel, confirm].spacing(8).align_y(Alignment::Center),
                ]
                .spacing(12)
            };
            container(
                container(panel_body)
                    .padding(16)
                    .width(Length::Fill)
                    .max_width(540)
                    .style(ui::panel),
            )
            .padding(if size.width < 520.0 { 8 } else { 18 })
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into()
        })
        .into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn key(
        controls: &mut Controls,
        named: Named,
        modifiers: Modifiers,
        pending: bool,
    ) -> Option<Event> {
        controls.key(Key::Named(named), modifiers, pending)
    }

    #[test]
    fn confirmation_starts_on_cancel_and_requires_traversal_to_confirm() {
        let mut controls = Controls::default();
        controls.show(ResetKind::Current, "BTC · 1m", 2, 1, 3);
        assert_eq!(
            key(&mut controls, Named::Enter, Modifiers::default(), false),
            Some(Event::Cancel)
        );
        assert_eq!(
            key(&mut controls, Named::Enter, Modifiers::default(), true),
            None
        );
        assert_eq!(
            key(&mut controls, Named::Tab, Modifiers::default(), false),
            None
        );
        assert_eq!(
            key(&mut controls, Named::Enter, Modifiers::default(), false),
            Some(Event::Confirm)
        );
        assert_eq!(
            key(&mut controls, Named::Tab, Modifiers::SHIFT, false),
            None
        );
        assert_eq!(
            key(&mut controls, Named::Enter, Modifiers::default(), false),
            Some(Event::Cancel)
        );
        key(&mut controls, Named::Tab, Modifiers::default(), false);
        key(&mut controls, Named::Tab, Modifiers::default(), false);
        assert_eq!(
            key(&mut controls, Named::Enter, Modifiers::default(), false),
            Some(Event::Cancel)
        );
        assert_eq!(
            key(&mut controls, Named::Enter, Modifiers::CTRL, false),
            None
        );
    }

    #[test]
    fn save_failure_requires_explicit_retry_or_exit_choice() {
        let mut controls = Controls::default();
        controls.show_save_error("disk unavailable");
        assert_eq!(
            key(&mut controls, Named::Enter, Modifiers::default(), false),
            Some(Event::RetrySave)
        );
        assert_eq!(
            key(&mut controls, Named::Tab, Modifiers::SHIFT, false),
            None
        );
        assert_eq!(
            key(&mut controls, Named::Enter, Modifiers::default(), false),
            Some(Event::ExitWithoutSaving)
        );
    }
}

use iced::{
    Element, Event, Length, Rectangle, Size, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, overlay, renderer,
        widget::{Operation, Tree, tree},
    },
    event, mouse,
};

/// Mount once at the window root, before any scrolling/translation. iced 0.13
/// gives a whole input batch its final cursor snapshot; route each mouse event
/// with its own position before descendants transform it into content space.
pub fn view<'a, Message: 'a, Theme: 'a, R: renderer::Renderer + 'a>(
    content: Element<'a, Message, Theme, R>,
) -> Element<'a, Message, Theme, R> {
    Element::new(WindowCursor { content })
}

struct WindowCursor<'a, Message, Theme, R> {
    content: Element<'a, Message, Theme, R>,
}

#[derive(Default)]
struct CursorState(Option<mouse::Cursor>);

impl CursorState {
    fn route(&mut self, event: &Event, snapshot: mouse::Cursor) -> mouse::Cursor {
        if matches!(event, Event::Window(iced::window::Event::Unfocused)) {
            self.0 = None;
        }
        let Event::Mouse(event) = event else {
            return snapshot;
        };
        if snapshot.position().is_none() {
            // The framework hides the base cursor while an overlay owns it.
            // Never revive that pointer or retain a position across the overlay.
            self.0 = None;
            return snapshot;
        }
        match event {
            mouse::Event::CursorMoved { position } => {
                self.0 = Some(mouse::Cursor::Available(*position));
            }
            mouse::Event::CursorLeft => self.0 = Some(mouse::Cursor::Unavailable),
            mouse::Event::CursorEntered => self.0 = Some(snapshot),
            _ => {}
        }
        self.0.unwrap_or(snapshot)
    }
}

impl<Message, Theme, R: renderer::Renderer> Widget<Message, Theme, R>
    for WindowCursor<'_, Message, Theme, R>
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<CursorState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(CursorState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.content]);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(&self, tree: &mut Tree, renderer: &R, limits: &layout::Limits) -> layout::Node {
        self.content
            .as_widget()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut R,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &R,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        let cursor = tree
            .state
            .downcast_mut::<CursorState>()
            .route(&event, cursor);
        self.content.as_widget_mut().on_event(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        )
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'_>,
        renderer: &R,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, R>> {
        let overlay = self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            translation,
        );
        if overlay.is_some() {
            // Overlay events can be captured before the base tree sees them.
            // Do not reuse a pre-menu cursor after such a move or selection.
            tree.state.downcast_mut::<CursorState>().0 = None;
        }
        overlay
    }
}

use crate::{Message, chart::Interaction};
use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, overlay, renderer,
        widget::{Operation, Tree, tree},
    },
    event, mouse,
};

/// Keep the canvas in a stable layout slot. Floating controls yield to the
/// canvas that owns a press, including motion/release outside its plot.
pub fn view<'a>(
    canvas: Element<'a, Message>,
    toolbar: Option<Element<'a, Message>>,
    height: Length,
    blocked: bool,
    drawing_generation: Option<u64>,
    input_scope: (u64, Option<u64>),
) -> Element<'a, Message> {
    Element::new(Surface {
        canvas,
        toolbar,
        height,
        blocked,
        drawing_generation,
        input_scope,
    })
}

struct Surface<'a> {
    canvas: Element<'a, Message>,
    toolbar: Option<Element<'a, Message>>,
    height: Length,
    blocked: bool,
    drawing_generation: Option<u64>,
    input_scope: (u64, Option<u64>),
}

struct InputScope((u64, Option<u64>));

impl Surface<'_> {
    fn captured(&self, tree: &Tree) -> bool {
        !self.blocked
            && self.toolbar.is_some()
            && tree.children[0]
                .state
                .downcast_ref::<Interaction>()
                .owns_pointer(self.drawing_generation)
    }
}

impl Widget<Message, Theme, Renderer> for Surface<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<InputScope>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(InputScope(self.input_scope))
    }

    fn children(&self) -> Vec<Tree> {
        match &self.toolbar {
            Some(toolbar) => vec![Tree::new(&self.canvas), Tree::new(toolbar)],
            None => vec![Tree::new(&self.canvas)],
        }
    }

    fn diff(&self, tree: &mut Tree) {
        let scope = tree.state.downcast_mut::<InputScope>();
        if scope.0 != self.input_scope {
            // List keys in iced 0.13 do not reset equal-length child lists.
            // Drop only held canvas input, not neighboring scroll/editor state.
            scope.0 = self.input_scope;
            tree.children[0] = Tree::new(&self.canvas);
        }
        match &self.toolbar {
            Some(toolbar) => tree.diff_children(&[&self.canvas, toolbar]),
            None => tree.diff_children(&[&self.canvas]),
        }
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, self.height)
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let limits = limits.width(Length::Fill).height(self.height);
        let size = limits.resolve(Length::Fill, self.height, Size::ZERO);
        let child_limits = layout::Limits::new(Size::ZERO, size);
        let canvas = self
            .canvas
            .as_widget()
            .layout(&mut tree.children[0], renderer, &child_limits);
        let mut children = vec![canvas];
        if let Some(toolbar) = &self.toolbar {
            children.push(toolbar.as_widget().layout(
                &mut tree.children[1],
                renderer,
                &child_limits,
            ));
        }
        layout::Node::with_children(size, children)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let mut children = layout.children();
        self.canvas.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            children.next().unwrap(),
            cursor,
            viewport,
        );
        if !self.captured(tree)
            && let Some(toolbar) = &self.toolbar
        {
            // Keep chrome quads in a later layer than cached canvas meshes.
            renderer::Renderer::with_layer(renderer, *viewport, |renderer| {
                toolbar.as_widget().draw(
                    &tree.children[1],
                    renderer,
                    theme,
                    style,
                    children.next().unwrap(),
                    cursor,
                    viewport,
                );
            });
        }
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let mut children = layout.children();
        self.canvas.as_widget().operate(
            &mut tree.children[0],
            children.next().unwrap(),
            renderer,
            operation,
        );
        if let Some(toolbar) = &self.toolbar {
            toolbar.as_widget().operate(
                &mut tree.children[1],
                children.next().unwrap(),
                renderer,
                operation,
            );
        }
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        let captured = self.captured(tree);
        let mut children = layout.children();
        let canvas_layout = children.next().unwrap();
        if !captured && let Some(toolbar) = &mut self.toolbar {
            let toolbar_layout = children.next().unwrap();
            let status = toolbar.as_widget_mut().on_event(
                &mut tree.children[1],
                event.clone(),
                toolbar_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
            if status == event::Status::Captured
                || matches!(event, Event::Mouse(_)) && cursor.is_over(toolbar_layout.bounds())
            {
                return event::Status::Captured;
            }
        }
        self.canvas.as_widget_mut().on_event(
            &mut tree.children[0],
            event,
            canvas_layout,
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
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let mut children = layout.children();
        let canvas_layout = children.next().unwrap();
        if !self.captured(tree)
            && let Some(toolbar) = &self.toolbar
        {
            let toolbar_layout = children.next().unwrap();
            if cursor.is_over(toolbar_layout.bounds()) {
                let interaction = toolbar.as_widget().mouse_interaction(
                    &tree.children[1],
                    toolbar_layout,
                    cursor,
                    viewport,
                    renderer,
                );
                return if interaction == mouse::Interaction::None {
                    mouse::Interaction::Idle
                } else {
                    interaction
                };
            }
        }
        self.canvas.as_widget().mouse_interaction(
            &tree.children[0],
            canvas_layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        if self.blocked || self.captured(tree) {
            return None;
        }
        self.toolbar.as_mut()?.as_widget_mut().overlay(
            &mut tree.children[1],
            layout.children().nth(1).unwrap(),
            renderer,
            translation,
        )
    }
}

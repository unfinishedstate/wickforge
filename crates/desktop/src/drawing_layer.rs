use crate::{
    drawing_geometry::{self, Projection},
    drawing_interaction::Interaction,
    drawing_types::DrawingId,
    drawings::Store,
    palette::Palette,
};
use iced::widget::canvas::{Cache, Frame, Geometry};
use iced::{Point, Renderer, Size};
use mexc_model::Quote;
use std::cell::Cell;

#[derive(Clone, Copy, PartialEq)]
struct Key {
    context: u64,
    revision: u64,
    projection: Projection,
    palette: Palette,
    price_scale: u32,
    quote: Option<Quote>,
    edited: Option<DrawingId>,
}

#[derive(Default)]
pub struct Scene {
    cache: Cache,
    key: Cell<Option<Key>>,
}

pub struct View<'a> {
    pub store: &'a Store,
    pub interaction: &'a Interaction,
    pub scene: &'a Scene,
    pub projection: &'a Cell<Option<Projection>>,
}

impl Scene {
    #[allow(clippy::too_many_arguments)]
    fn committed(
        &self,
        renderer: &Renderer,
        size: Size,
        view: &View<'_>,
        projection: Projection,
        palette: Palette,
        price_scale: u32,
        quote: &str,
    ) -> Geometry {
        let key = Key {
            context: view.store.generation(),
            revision: view.store.revision(),
            projection,
            palette,
            price_scale,
            quote: Quote::parse(quote),
            edited: view.interaction.dragging_id(),
        };
        if self.key.get() != Some(key) {
            self.key.set(Some(key));
            self.cache.clear();
        }
        self.cache.draw(renderer, size, |frame| {
            drawing_geometry::draw_committed(
                frame,
                view.store.objects(),
                projection,
                palette,
                price_scale,
                quote,
                key.edited,
            );
        })
    }
}

impl View<'_> {
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        renderer: &Renderer,
        size: Size,
        projection: Projection,
        cursor: Option<Point>,
        palette: Palette,
        price_scale: u32,
        quote: &str,
    ) -> [Geometry; 2] {
        let committed = self.scene.committed(
            renderer,
            size,
            self,
            projection,
            palette,
            price_scale,
            quote,
        );
        let mut overlay = Frame::new(renderer, size);
        drawing_geometry::draw_overlay(
            &mut overlay,
            self.store.objects(),
            self.store.selected(),
            if cursor.is_some_and(|point| projection.contains(point)) {
                self.interaction.hover()
            } else {
                None
            },
            self.interaction.preview(self.store, projection, cursor),
            projection,
            palette,
            price_scale,
            quote,
        );
        [committed, overlay.into_geometry()]
    }
}

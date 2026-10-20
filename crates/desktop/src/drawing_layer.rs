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
    vwap_revision: u64,
    projection: Projection,
    palette: Palette,
    price_scale: u32,
    quote: Option<Quote>,
    edited: Option<DrawingId>,
    candle_range: (usize, Option<i64>, Option<i64>),
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
    pub native_candles: &'a [mexc_model::Candle],
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
            vwap_revision: view.store.vwap_revision(),
            projection,
            palette,
            price_scale,
            quote: Quote::parse(quote),
            edited: view.interaction.dragging_id(),
            candle_range: (
                view.native_candles.len(),
                view.native_candles.first().map(|candle| candle.time),
                view.native_candles.last().map(|candle| candle.time),
            ),
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
                &drawing_geometry::NativeLabels {
                    price_scale,
                    quote,
                    candles: view.native_candles,
                },
                key.edited,
                Some(view.store),
            );
        })
    }
}

impl View<'_> {
    pub fn snap(
        &self,
        projection: Projection,
        point: Point,
    ) -> Option<crate::drawing_types::Anchor> {
        let anchored = self.interaction.tool() == crate::drawing_types::Tool::AnchoredVwap
            || (self.interaction.tool() == crate::drawing_types::Tool::Select
                && self.store.selected_drawing().is_some_and(|drawing| {
                    matches!(drawing.shape, crate::drawing_types::Shape::AnchoredVwap(_))
                }));
        if anchored {
            drawing_geometry::native_anchor(projection, point, self.native_candles)
        } else if self.interaction.magnet() && projection.contains(point) {
            drawing_geometry::magnet(projection, point, self.native_candles.iter())
        } else {
            None
        }
    }

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
            self.native_candles,
            Some(self.store),
        );
        [committed, overlay.into_geometry()]
    }
}

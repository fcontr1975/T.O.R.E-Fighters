//! Aspect-responsive flight composition; menus keep their original 640x480 canvas.
use crate::canvas_rows::RowSet;
use crate::{
    aim_box,
    aircraft::Airframe,
    flight::State,
    instruments::{Instruments, PageKey},
    menu::Sprite,
};
use std::ops::Range;
use std::sync::Arc;
use tore_formats::text::GlyphCodes;
#[cfg(test)]
mod benchmark;
#[cfg(test)]
mod key_tests;
mod partial;
#[cfg(test)]
mod worker_tests;
// Fifteen percent smaller than the prior 0.85 layout. Angular cues retain world alignment.
pub const HUD_SCALE: f64 = 0.85 * 0.85;
struct PanelCache {
    /// What the page drew from ([`crate::instruments::PageKey`]); `None`
    /// for a page that always redraws.
    key: Option<PageKey>,
    source: Arc<Vec<u8>>,
    size: [u32; 2],
    image: Arc<Sprite>,
    /// Every clear pixel of `image` is all zero, so composing it over a
    /// clear canvas is a plain copy.
    clear_is_zero: bool,
}
struct PanelJob {
    page: u8,
    cached: Option<Arc<PanelCache>>,
    slots: Vec<(usize, [u32; 2])>,
}

// A single page has no independent work to share. Both rasterisation and
// dirty-page scaling belong to each job, while composition remains ordered.
const MIN_PANEL_JOBS: usize = 2;

/// Debug builds, and release builds run with `TORE_PAGE_KEY_CHECK` set, draw
/// a page whose key is unchanged anyway and compare it with the kept pixels.
fn page_key_check() -> bool {
    static CHECK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    cfg!(debug_assertions)
        || *CHECK.get_or_init(|| std::env::var_os("TORE_PAGE_KEY_CHECK").is_some())
}
/// Pages kept by key, kept pages drawn again to check, and checks that
/// found different pixels.
static PAGE_KEY_STATS: [std::sync::atomic::AtomicU64; 3] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 3];
/// `(kept, checked, differing)` since the process started.
pub fn page_key_stats() -> [u64; 3] {
    PAGE_KEY_STATS
        .each_ref()
        .map(|n| n.load(std::sync::atomic::Ordering::Relaxed))
}
fn count_page_key(stat: usize) -> u64 {
    PAGE_KEY_STATS[stat].fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

fn prepare_panel(
    cached: Option<&Arc<PanelCache>>,
    page: u8,
    size: [u32; 2],
    h: &Airframe,
    s: &State,
    panels: &Instruments,
) -> Arc<PanelCache> {
    let key = panels.page_key(page, s);
    if let Some(cached) = cached
        && cached.size == size
        && key.is_some()
        && cached.key == key
    {
        count_page_key(0);
        if !page_key_check() {
            return Arc::clone(cached);
        }
        let raster = panels.page(page, h, s);
        let checked = count_page_key(1);
        if raster.pixels == *cached.source {
            if checked.is_multiple_of(5000) && !cfg!(test) {
                let [kept, checked, differing] = page_key_stats();
                eprintln!("Page keys: {kept} pages kept, {checked} checked, {differing} differed");
            }
            return Arc::clone(cached);
        }
        count_page_key(2);
        assert!(
            !cfg!(debug_assertions),
            "page {page}: its key is unchanged but a redraw differs from the kept pixels"
        );
        eprintln!("Page keys: page {page} kept pixels that a redraw changes; drawing it");
        return scaled_panel(Some(cached), key, size, raster.pixels);
    }
    scaled_panel(cached, key, size, panels.page(page, h, s).pixels)
}

/// The page raster `source` scaled to `size`. A kept image of the same size
/// is reused where the source is unchanged: only the pixels whose filter
/// taps read a changed source pixel are worked out again.
fn scaled_panel(
    cached: Option<&Arc<PanelCache>>,
    key: Option<PageKey>,
    size: [u32; 2],
    source: Vec<u8>,
) -> Arc<PanelCache> {
    let (width, height) = (crate::instruments::WIDTH, crate::instruments::HEIGHT);
    let rect = (0., 0., size[0] as f64, size[1] as f64);
    let cached = cached.filter(|cached| cached.size == size && cached.source.len() == source.len());
    if let Some(cached) = cached
        && *cached.source == source
    {
        if cached.key == key {
            return Arc::clone(cached);
        }
        return Arc::new(PanelCache {
            key,
            source: Arc::clone(&cached.source),
            size,
            image: Arc::clone(&cached.image),
            clear_is_zero: cached.clear_is_zero,
        });
    }
    let sprite = Sprite {
        width,
        height,
        rgba: source,
        glyphs: vec![],
    };
    let mut canvas = FlightCanvas {
        size,
        ..Default::default()
    };
    let clear_is_zero = |pixels: &[u8]| pixels.chunks_exact(4).all(|p| p[3] != 0 || p == [0; 4]);
    let partial = cached.and_then(|cached| {
        let spans = partial::changed_spans(&cached.source, &sprite.rgba, width);
        let segments = partial::reach(size, [width, height], rect, &spans);
        let area: usize = segments.iter().map(|(_, columns)| columns.len()).sum();
        (area * 2 < size[0] as usize * size[1] as usize).then_some((cached, segments))
    });
    let clear_is_zero = if let Some((cached, segments)) = partial {
        canvas.pixels = cached.image.rgba.clone();
        let stride = size[0] as usize * 4;
        for (row, columns) in &segments {
            canvas.pixels[row * stride + columns.start * 4..row * stride + columns.end * 4].fill(0);
        }
        canvas.blit_within(&sprite, rect, Some(&segments));
        cached.clear_is_zero
            && segments.iter().all(|(row, columns)| {
                clear_is_zero(
                    &canvas.pixels
                        [row * stride + columns.start * 4..row * stride + columns.end * 4],
                )
            })
    } else {
        canvas.pixels = vec![0; size[0] as usize * size[1] as usize * 4];
        canvas.blit(&sprite, rect);
        clear_is_zero(&canvas.pixels)
    };
    Arc::new(PanelCache {
        key,
        source: Arc::new(sprite.rgba),
        size,
        image: Arc::new(Sprite {
            width: size[0] as usize,
            height: size[1] as usize,
            rgba: canvas.pixels,
            glyphs: vec![],
        }),
        clear_is_zero,
    })
}

/// What every page draws from the airframe: its window frame picture, its
/// instrument font and its HUD colours. Kept pages are dropped when these
/// change, since page keys leave them out.
struct AirframeStamp {
    font_height: usize,
    glyphs: Vec<(usize, Vec<(usize, usize)>)>,
    panel: (usize, usize, Vec<u8>, Vec<bool>),
    hud: tore_formats::hud::Hud,
}
impl AirframeStamp {
    fn of(h: &Airframe) -> Self {
        Self {
            font_height: h.font.height,
            glyphs: h
                .font
                .glyphs
                .iter()
                .map(|g| (g.advance, g.pixels.clone()))
                .collect(),
            panel: (
                h.panel.width,
                h.panel.height,
                h.panel.pixels.clone(),
                h.panel.mask.clone(),
            ),
            hud: h.hud.clone(),
        }
    }
    fn matches(&self, h: &Airframe) -> bool {
        self.font_height == h.font.height
            && self.glyphs.len() == h.font.glyphs.len()
            && self
                .glyphs
                .iter()
                .zip(&h.font.glyphs)
                .all(|((advance, pixels), g)| *advance == g.advance && *pixels == g.pixels)
            && self.panel.0 == h.panel.width
            && self.panel.1 == h.panel.height
            && self.panel.2 == h.panel.pixels
            && self.panel.3 == h.panel.mask
            && self.hud == h.hud
    }
}

#[derive(Default)]
pub struct FlightCanvas {
    pub pixels: Vec<u8>,
    pub size: [u32; 2],
    panels: std::collections::BTreeMap<u8, Arc<PanelCache>>,
    /// The airframe the kept panels were drawn for.
    stamp: Option<AirframeStamp>,
    /// Rows drawn on since the canvas was last cleared. Every row outside
    /// this set is all zero, so the next frame clears only these.
    touched: RowSet,
    /// Rows that may differ from what the renderer last uploaded: the rows
    /// touched this frame and in frames it did not upload.
    pub pending: RowSet,
    /// Scratch for the filter in `blit`.
    premultiplied: Vec<[f64; 4]>,
    /// The layers `legacy_layer` drew last, oldest first.
    layers: Vec<LayerCache>,
    /// How often `legacy_layer` copied a kept result.
    #[cfg(test)]
    layer_hits: usize,
    /// How often `legacy_layer` drew only what a change reached.
    #[cfg(test)]
    layer_redraws: usize,
}
/// A 640 by 480 layer `legacy_layer` drew and what came of it.
#[derive(Default)]
struct LayerCache {
    /// The canvas size and scale it was drawn at.
    key: Option<([u32; 2], u64)>,
    layer: Vec<u8>,
    /// Where the layer has anything drawn: left, top, right, bottom.
    bounds: Option<(usize, usize, usize, usize)>,
    /// Drawn on before, so worth keeping a copy of its surroundings.
    seen: bool,
    /// `before` and `after` hold the canvas rows `region` (columns, rows)
    /// either side of drawing the layer, one row after another.
    valid: bool,
    region: (std::ops::Range<usize>, std::ops::Range<usize>),
    before: Vec<u8>,
    after: Vec<u8>,
}
impl FlightCanvas {
    pub fn begin(&mut self, size: [u32; 2], h: &Airframe, s: &State, panels: &Instruments) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Panels);
        self.begin_with(tore_workers::shared(), size, h, s, panels);
    }

    fn begin_with(
        &mut self,
        workers: &tore_workers::Executor,
        size: [u32; 2],
        h: &Airframe,
        s: &State,
        panels: &Instruments,
    ) {
        self.clear_for(size);
        if !self.stamp.as_ref().is_some_and(|stamp| stamp.matches(h)) {
            self.panels.clear();
            self.stamp = Some(AirframeStamp::of(h));
        }
        let (w, height) = (size[0] as f64, size[1] as f64);
        let distinct = panels
            .pages
            .iter()
            .enumerate()
            .filter(|(index, page)| !panels.pages[..*index].contains(page))
            .count();
        if !workers.should_dispatch(distinct, MIN_PANEL_JOBS) {
            for (i, page) in panels.pages.iter().enumerate() {
                let rect = panels.screen_rect(i, [w, height]);
                let size = [rect.2.round() as u32, rect.3.round() as u32];
                let cached = prepare_panel(self.panels.get(page), *page, size, h, s, panels);
                self.compose_panel(*page, rect, cached);
            }
            return;
        }

        // The call point follows this frame's preview, readout, palette and
        // hover updates. All jobs borrow those same immutable inputs until
        // preparation joins; nothing from the window or GPU crosses here.
        let mut jobs: Vec<PanelJob> = Vec::with_capacity(distinct);
        let mut rects = Vec::with_capacity(panels.pages.len());
        for (index, &page) in panels.pages.iter().enumerate() {
            let rect = panels.screen_rect(index, [w, height]);
            rects.push(rect);
            let slot = (index, [rect.2.round() as u32, rect.3.round() as u32]);
            if let Some(job) = jobs.iter_mut().find(|job| job.page == page) {
                job.slots.push(slot);
            } else {
                jobs.push(PanelJob {
                    page,
                    cached: self.panels.get(&page).cloned(),
                    slots: vec![slot],
                });
            }
        }
        let prepared = workers.ordered_map(&jobs, MIN_PANEL_JOBS, |_, job| {
            let mut cached = job.cached.clone();
            job.slots
                .iter()
                .map(|&(index, size)| {
                    let next = prepare_panel(cached.as_ref(), job.page, size, h, s, panels);
                    cached = Some(Arc::clone(&next));
                    (index, next)
                })
                .collect::<Vec<_>>()
        });
        let mut slots = vec![None; panels.pages.len()];
        for (index, cached) in prepared.into_iter().flatten() {
            slots[index] = Some(cached);
        }
        for ((&page, rect), cached) in panels.pages.iter().zip(rects).zip(slots) {
            self.compose_panel(page, rect, cached.expect("every panel slot was prepared"));
        }
    }

    /// A clear canvas of `size`. When the size is unchanged only the rows
    /// drawn on since the last clear are cleared; the rest are zero already.
    fn clear_for(&mut self, size: [u32; 2]) {
        let height = size[1] as usize;
        let len = size[0] as usize * height * 4;
        if self.size == size && self.pixels.len() == len && self.touched.len() == height {
            let stride = size[0] as usize * 4;
            for run in self.touched.runs() {
                self.pixels[run.start * stride..run.end * stride].fill(0);
            }
            self.pending.merge(&self.touched);
            self.touched.clear();
            return;
        }
        self.size = size;
        self.pixels.clear();
        self.pixels.resize(len, 0);
        self.touched.reset(height);
        self.pending.reset_all(height);
    }
    /// A clear canvas of `size` with no panels: what `begin` gives before
    /// the panels go on, for frames that cover or wipe all of them.
    pub fn clear(&mut self, size: [u32; 2]) {
        self.clear_for(size);
    }
    /// Panics if a row holds a pixel but is not noted as drawn on, which
    /// would leave it uncleared and unsent.
    #[cfg(test)]
    pub fn assert_drawn_rows_noted(&self) {
        let lit = crate::canvas_rows::lit_rows(&self.pixels, self.size[0] as usize);
        for run in lit.runs() {
            for row in run {
                assert!(self.touched.contains(row), "row {row} drawn but not noted");
            }
        }
    }
    /// Notes that rows `from..to` were drawn on.
    pub fn touch_rows(&mut self, from: usize, to: usize) {
        self.touched.mark(from, to);
    }
    /// Makes `pending` cover everything drawn so far this frame, for the
    /// renderer to upload.
    pub fn prepare_upload(&mut self) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Ui);
        self.pending.merge(&self.touched);
    }
    /// Fills the whole canvas with one opaque `color`.
    pub fn fill_opaque(&mut self, color: [u8; 3]) {
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
        self.touched.mark_all();
    }
    fn compose_panel(&mut self, page: u8, rect: (f64, f64, f64, f64), cached: Arc<PanelCache>) {
        let (x, y) = (rect.0.round(), rect.1.round());
        let [w, h] = cached.size.map(|n| n as usize);
        let rect = (x, y, w as f64, h as f64);
        // Over a clear canvas a panel whose clear pixels are all zero is its
        // own pixels: `blit`'s whole-pixel paths leave a clear source pixel
        // alone and give any other the source pixel itself over a clear one
        // (see the test of every colour and alpha). A row with anything
        // under it goes through `blit`.
        let fits = cached.clear_is_zero
            && x >= 0.
            && y >= 0.
            && x + w as f64 <= self.size[0] as f64
            && y + h as f64 <= self.size[1] as f64
            && self.size.iter().all(|&dimension| dimension <= 1 << 24);
        if !fits {
            self.blit(&cached.image, rect);
            self.panels.insert(page, cached);
            return;
        }
        let (x0, y0) = (x as usize, y as usize);
        let stride = self.size[0] as usize * 4;
        let mut under = Vec::new();
        for row in 0..h {
            let at = (y0 + row) * stride + x0 * 4;
            let destination = &mut self.pixels[at..at + w * 4];
            if destination.iter().fold(0, |any, &b| any | b) == 0 {
                destination.copy_from_slice(&cached.image.rgba[row * w * 4..(row + 1) * w * 4]);
            } else {
                under.push((y0 + row, x0..x0 + w));
            }
        }
        self.touched.mark(y0, y0 + h);
        if !under.is_empty() {
            self.blit_within(&cached.image, rect, Some(&under));
        }
        self.panels.insert(page, cached);
    }
    /// Blend `color` at `alpha` over one screen pixel; outside the view is ignored.
    pub fn blend(&mut self, x: i32, y: i32, color: [u8; 3], alpha: f64) {
        let [width, height] = self.size.map(|n| n as i32);
        if x < 0 || y < 0 || x >= width || y >= height {
            return;
        }
        let at = (y * width + x) as usize * 4;
        self.touched.mark(y as usize, y as usize + 1);
        let p = &mut self.pixels[at..at + 4];
        // Straight-alpha "over", as in `veil`.
        let under = f64::from(p[3]) / 255. * (1. - alpha);
        let out = alpha + under;
        for c in 0..3 {
            p[c] = ((f64::from(color[c]) * alpha + f64::from(p[c]) * under) / out).round() as u8;
        }
        p[3] = (out * 255.).round() as u8;
    }
    /// Blend `color` at `alpha` over the pixels `x0..x1` by `y0..y1` (clipped
    /// to the view), the same as [`blend`](Self::blend) on each of them. A
    /// pixel with no alpha yet takes the one result that formula gives it
    /// without working it out again.
    pub fn blend_rect(
        &mut self,
        (x0, y0, x1, y1): (i32, i32, i32, i32),
        color: [u8; 3],
        alpha: f64,
    ) {
        let [width, height] = self.size.map(|n| n as i32);
        let (x0, x1) = (x0.clamp(0, width), x1.clamp(0, width));
        let (y0, y1) = (y0.clamp(0, height), y1.clamp(0, height));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.touched.mark(y0 as usize, y1 as usize);
        // What `blend` makes of a pixel whose alpha is zero: it weighs none
        // of that pixel's colour.
        let over_nothing = {
            let out = alpha + 0.;
            let mut pixel = [0; 4];
            for c in 0..3 {
                pixel[c] = ((f64::from(color[c]) * alpha + 0.) / out).round() as u8;
            }
            pixel[3] = (out * 255.).round() as u8;
            pixel
        };
        for y in y0..y1 {
            let at = (y * width + x0) as usize * 4;
            let row = &mut self.pixels[at..at + (x1 - x0) as usize * 4];
            for p in row.chunks_exact_mut(4) {
                if p[3] == 0 && alpha > 0. {
                    p.copy_from_slice(&over_nothing);
                    continue;
                }
                let under = f64::from(p[3]) / 255. * (1. - alpha);
                let out = alpha + under;
                for c in 0..3 {
                    p[c] = ((f64::from(color[c]) * alpha + f64::from(p[c]) * under) / out).round()
                        as u8;
                }
                p[3] = (out * 255.).round() as u8;
            }
        }
    }
    /// Blend `color` over the whole flight view, `coverage(radius)` per pixel
    /// with radius 0 at the centre and 1 at the corners.
    pub fn veil(&mut self, color: [u8; 3], coverage: impl Fn(f64) -> f64) {
        let [w, h] = self.size.map(f64::from);
        let (cx, cy) = (w / 2., h / 2.);
        let corner = cx.hypot(cy).max(1.);
        self.touched.mark_all();
        for (i, p) in self.pixels.chunks_exact_mut(4).enumerate() {
            let x = (i % self.size[0] as usize) as f64 + 0.5;
            let y = (i / self.size[0] as usize) as f64 + 0.5;
            let a = coverage((x - cx).hypot(y - cy) / corner);
            if a <= 0. {
                continue;
            }
            // Straight-alpha "over", so the result still blends onto the world.
            let under = f64::from(p[3]) / 255. * (1. - a);
            let out = a + under;
            for c in 0..3 {
                p[c] = ((f64::from(color[c]) * a + f64::from(p[c]) * under) / out).round() as u8;
            }
            p[3] = (out * 255.).round() as u8;
        }
    }
    /// How many canvas pixels one HUD pixel covers at `zoom`.
    pub fn hud_pixel(&self, zoom: f64) -> f64 {
        (f64::from(self.size[0]) / 640.).min(f64::from(self.size[1]) / 480.) * HUD_SCALE * zoom
    }
    /// Strokes given in HUD pixels about `at`, smoothed like the HUD so they
    /// are no brighter or bolder.
    fn hud_strokes(
        &mut self,
        [x, y]: [f64; 2],
        zoom: f64,
        color: [u8; 3],
        strokes: &[aim_box::Stroke],
    ) {
        let scale = self.hud_pixel(zoom);
        for &(a, b) in strokes {
            let steps = ((b.0 - a.0).hypot(b.1 - a.1) * scale * 2.).ceil().max(1.) as usize;
            for step in 0..=steps {
                let t = step as f64 / steps as f64;
                let px = x + (a.0 + (b.0 - a.0) * t) * scale;
                let py = y + (a.1 + (b.1 - a.1) * t) * scale;
                self.dot(px, py, scale, color);
            }
        }
    }
    /// The target square outside the HUD (Easy targeting's, and the AC-130's
    /// aim-point box): the HUD's own 14-pixel square and one-pixel line at the
    /// HUD's on-screen scale for `zoom`, with the friendly X for a friendly
    /// target, a centre dot for a pinned point or corner brackets for a point
    /// the sight only sweeps over.
    pub fn target_square(
        &mut self,
        at: [f64; 2],
        zoom: f64,
        color: [u8; 3],
        style: aim_box::Style,
    ) {
        self.hud_strokes(at, zoom, color, &aim_box::box_strokes(style));
    }
    /// The small diamond where the AC-130's guns will hit.
    pub fn aim_diamond(&mut self, at: [f64; 2], zoom: f64, color: [u8; 3]) {
        self.hud_strokes(
            at,
            zoom,
            color,
            &aim_box::diamond_strokes(aim_box::DIAMOND_HALF),
        );
    }
    /// An arrow at the screen edge towards a box that is off screen, the
    /// shape of the HUD's own edge arrow. `direction` is a unit vector, x
    /// right and y down.
    /// It keeps clear of the `windows` (the instrument panels), which would
    /// hide it.
    pub fn edge_chevron(
        &mut self,
        direction: [f64; 2],
        zoom: f64,
        color: [u8; 3],
        windows: &[aim_box::Rect],
    ) {
        // Far enough in that the whole arrow, and the line's width, show.
        let margin = aim_box::EDGE_ARROW * 1.5 * self.hud_pixel(zoom);
        let tip =
            aim_box::edge_point_clear(self.size.map(f64::from), margin, direction, windows, margin);
        self.hud_strokes(
            tip,
            zoom,
            color,
            &aim_box::chevron_strokes(direction, aim_box::EDGE_ARROW, aim_box::EDGE_ARROW / 2.),
        );
    }
    /// A `size`-wide square pen centred on (x, y), with partial pixels at its
    /// edges, blended so overlapping stamps never exceed full coverage.
    fn dot(&mut self, x: f64, y: f64, size: f64, color: [u8; 3]) {
        let half = size / 2.;
        let [w, h] = self.size.map(|v| v as i64);
        let rows = ((y - half).floor() as i64).max(0)..((y + half).ceil() as i64).min(h);
        if rows.start < rows.end {
            self.touched.mark(rows.start as usize, rows.end as usize);
        }
        for py in rows {
            for px in ((x - half).floor() as i64).max(0)..((x + half).ceil() as i64).min(w) {
                let cover =
                    |p: i64, c: f64| ((p + 1) as f64).min(c + half) - (p as f64).max(c - half);
                let a =
                    (cover(px, x).clamp(0., 1.) * cover(py, y).clamp(0., 1.) * 255.).round() as u8;
                let i = (py * w + px) as usize * 4;
                if a > self.pixels[i + 3] {
                    self.pixels[i..i + 4].copy_from_slice(&[color[0], color[1], color[2], a]);
                }
            }
        }
    }
    pub fn weapon_debug(&mut self, pixels: &[u8]) {
        let mut rgba = Vec::with_capacity(250 * 96 * 4);
        for y in 0..96 {
            rgba.extend_from_slice(&pixels[y * 640 * 4..(y * 640 + 250) * 4]);
        }
        let scale = (f64::from(self.size[0]) / 640.).min(f64::from(self.size[1]) / 480.);
        self.blit(
            &Sprite {
                width: 250,
                height: 96,
                rgba,
                glyphs: vec![],
            },
            (
                f64::from(self.size[0]) - 258. * scale,
                8. * scale,
                250. * scale,
                96. * scale,
            ),
        );
    }
    /// An empty, fully transparent canvas of `size`: the mission replay's
    /// view has no instruments.
    pub fn blank(&mut self, size: [u32; 2]) {
        if self.size != size || self.touched.len() != size[1] as usize {
            self.size = size;
            self.touched.reset(size[1] as usize);
            self.pending.reset_all(size[1] as usize);
        }
        self.pixels.clear();
        self.pixels
            .resize(size[0] as usize * size[1] as usize * 4, 0);
    }
    /// A 640x480 layer drawn in two halves at the scale `legacy_layer` uses,
    /// centred across: the top half against the top edge and the bottom half
    /// against the bottom edge. On a canvas at least 4:3 wide this is exactly
    /// where `legacy_layer` puts it; on a taller one the halves part, so a
    /// bar along the layer's bottom stays on the view's bottom edge.
    pub fn anchored_layer(&mut self, pixels: &[u8]) {
        let (w, h) = (self.size[0] as f64, self.size[1] as f64);
        let scale = (w / 640.).min(h / 480.);
        for (top, y) in [(0, 0.), (240, h - 240. * scale)] {
            let (mut left, mut first, mut right, mut last) = (640usize, 480usize, 0usize, 0usize);
            for row in top..top + 240 {
                for (x, p) in pixels[row * 640 * 4..(row + 1) * 640 * 4]
                    .chunks_exact(4)
                    .enumerate()
                {
                    if p[3] != 0 {
                        left = left.min(x);
                        right = right.max(x + 1);
                        first = first.min(row);
                        last = last.max(row + 1);
                    }
                }
            }
            if right == 0 {
                continue;
            }
            let (width, height) = (right - left, last - first);
            let mut rgba = Vec::with_capacity(width * height * 4);
            for row in first..last {
                rgba.extend_from_slice(&pixels[(row * 640 + left) * 4..(row * 640 + right) * 4]);
            }
            self.scaled(
                &Sprite {
                    width,
                    height,
                    rgba,
                    glyphs: vec![],
                },
                (
                    (w - 640. * scale) / 2. + left as f64 * scale,
                    y + (first - top) as f64 * scale,
                    width as f64 * scale,
                    height as f64 * scale,
                ),
            );
        }
    }
    /// Rectangles of a 640x480 layer, placed where `legacy_layer` puts the
    /// whole layer (centred and scaled to fit the view) but drawn by nearest
    /// pixel like `anchored_layer`, so small text stays crisp. Only those
    /// rectangles are read: the debug panels and menus know where they drew.
    pub fn centered_rects(&mut self, pixels: &[u8], rects: &[(i32, i32, i32, i32)]) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Ui);
        let (w, h) = (self.size[0] as f64, self.size[1] as f64);
        let scale = (w / 640.).min(h / 480.);
        let (left, top) = ((w - 640. * scale) / 2., (h - 480. * scale) / 2.);
        for &(x, y, rw, rh) in rects {
            let (x0, y0) = (x.clamp(0, 640) as usize, y.clamp(0, 480) as usize);
            let (x1, y1) = (
                (x + rw).clamp(0, 640) as usize,
                (y + rh).clamp(0, 480) as usize,
            );
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let (width, height) = (x1 - x0, y1 - y0);
            let mut rgba = Vec::with_capacity(width * height * 4);
            for row in y0..y1 {
                rgba.extend_from_slice(&pixels[(row * 640 + x0) * 4..(row * 640 + x1) * 4]);
            }
            self.scaled(
                &Sprite {
                    width,
                    height,
                    rgba,
                    glyphs: vec![],
                },
                (
                    left + x0 as f64 * scale,
                    top + y0 as f64 * scale,
                    width as f64 * scale,
                    height as f64 * scale,
                ),
            );
        }
    }
    /// Draws `s` scaled into the rectangle by nearest pixel, the way the
    /// menus' 640x480 canvas is scaled, blending partly transparent pixels
    /// over what is there. Cheap enough for the replay's interface layer,
    /// which is drawn every frame.
    fn scaled(&mut self, s: &Sprite, (x, y, w, h): (f64, f64, f64, f64)) {
        let dw = self.size[0] as usize;
        let taps = |start: i32, end: i32, origin: f64, span: f64, source: usize| {
            (start..end)
                .map(|d| {
                    let u = ((d as f64 + 0.5 - origin) * source as f64 / span).floor();
                    (d as usize, u.clamp(0., (source - 1) as f64) as usize)
                })
                .collect::<Vec<_>>()
        };
        let columns = taps(
            (x.round() as i32).max(0),
            ((x + w).round() as i32).min(self.size[0] as i32),
            x,
            w,
            s.width,
        );
        let rows = taps(
            (y.round() as i32).max(0),
            ((y + h).round() as i32).min(self.size[1] as i32),
            y,
            h,
            s.height,
        );
        if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
            self.touched.mark(first.0, last.0 + 1);
        }
        for &(yy, sy) in &rows {
            for &(xx, sx) in &columns {
                let from = (sy * s.width + sx) * 4;
                let p = &s.rgba[from..from + 4];
                let at = (yy * dw + xx) * 4;
                if p[3] == 0 {
                    continue;
                }
                if p[3] == 255 || self.pixels[at + 3] == 0 {
                    self.pixels[at..at + 4].copy_from_slice(p);
                    continue;
                }
                // Straight-alpha "over".
                let a = f64::from(p[3]) / 255.;
                let old = f64::from(self.pixels[at + 3]) / 255.;
                let out = a + old * (1. - a);
                for (c, value) in p.iter().enumerate().take(3) {
                    self.pixels[at + c] = ((f64::from(*value) * a
                        + f64::from(self.pixels[at + c]) * old * (1. - a))
                        / out)
                        .round() as u8;
                }
                self.pixels[at + 3] = (out * 255.).round() as u8;
            }
        }
    }
    /// The width of `text` in `font` drawn at `scale`, in canvas pixels.
    pub fn text_width(font: &tore_formats::font::Font, text: &str, scale: f64) -> f64 {
        text.glyph_codes()
            .filter_map(|c| font.glyphs.get(usize::from(c)))
            .map(|g| g.advance as f64 * scale)
            .sum()
    }
    /// `text` in `font` with its top left at (x, y), each font pixel drawn
    /// as a `scale`-wide smoothed dot, then a dark shadow a font pixel down
    /// and right where the text left the canvas clear, so labels read over
    /// sky and ground alike.
    pub fn text(
        &mut self,
        font: &tore_formats::font::Font,
        text: &str,
        [x, y]: [f64; 2],
        scale: f64,
        color: [u8; 3],
    ) {
        for (offset, color) in [(0., color), (scale, [0, 0, 0])] {
            let mut pen = x + offset;
            for c in text.glyph_codes() {
                let Some(glyph) = font.glyphs.get(usize::from(c)) else {
                    continue;
                };
                for &(gx, gy) in &glyph.pixels {
                    self.dot(
                        pen + (gx as f64 + 0.5) * scale,
                        y + offset + (gy as f64 + 0.5) * scale,
                        scale,
                        color,
                    );
                }
                pen += glyph.advance as f64 * scale;
            }
        }
    }
    pub fn hud_zoom(&self, zoom: f32) -> f32 {
        let [w, h] = self.size;
        zoom / HUD_SCALE as f32 * (h as f32 / (480. * (w as f32 / 640.).min(h as f32 / 480.)))
    }
    pub fn legacy_layer(&mut self, pixels: &[u8], factor: f64) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Ui);
        let key = (self.size, factor.to_bits());
        // The two layers drawn most recently (the map, the menus over it)
        // keep their scaled result while neither they nor what lies under
        // them change.
        let mut cache = match self
            .layers
            .iter()
            .position(|c| c.key == Some(key) && c.layer == pixels)
        {
            Some(found) => self.layers.remove(found),
            None => {
                let bounds = layer_bounds(pixels);
                // A layer that changed in place (the map as aircraft move)
                // starts from its last result over the same picture beneath.
                if let Some(base) = self
                    .layers
                    .iter()
                    .rposition(|c| c.key == Some(key) && c.valid && c.bounds == Some(bounds))
                    && self.redraw_changed(base, pixels, factor)
                {
                    return;
                }
                let same_place = self
                    .layers
                    .iter()
                    .any(|c| c.key == Some(key) && c.bounds == Some(bounds));
                let mut fresh = if self.layers.len() >= 2 {
                    self.layers.remove(0)
                } else {
                    LayerCache::default()
                };
                fresh.key = Some(key);
                fresh.layer.clear();
                fresh.layer.extend_from_slice(pixels);
                fresh.valid = false;
                // A layer drawn where the last one was is worth keeping a
                // copy of its surroundings for, as one seen before is.
                fresh.seen = same_place;
                fresh.bounds = Some(bounds);
                fresh
            }
        };
        let seen = std::mem::replace(&mut cache.seen, true);
        let (left, top, right, bottom) = *cache.bounds.get_or_insert_with(|| layer_bounds(pixels));
        if right == 0 {
            self.layers.push(cache);
            return;
        }
        let (rect, columns, rows) = self.layer_geometry((left, top, right, bottom), factor);
        let stride = self.size[0] as usize * 4;
        let region = |rows: &std::ops::Range<usize>| {
            rows.clone()
                .map(|row| row * stride + columns.start * 4..row * stride + columns.end * 4)
        };
        if columns.start < columns.end && rows.start < rows.end {
            // Under the same layer, the same picture beneath gives the same
            // result: copy the one kept from last time.
            let mut at = 0;
            let same_beneath = cache.valid
                && cache.region == (columns.clone(), rows.clone())
                && region(&rows).all(|range| {
                    let len = range.len();
                    let same = self.pixels[range] == cache.before[at..at + len];
                    at += len;
                    same
                });
            if same_beneath {
                let mut at = 0;
                for range in region(&rows) {
                    let len = range.len();
                    self.pixels[range].copy_from_slice(&cache.after[at..at + len]);
                    at += len;
                }
                self.touched.mark(rows.start, rows.end);
                #[cfg(test)]
                {
                    self.layer_hits += 1;
                }
                self.layers.push(cache);
                return;
            }
            // Keep the picture beneath only for a layer seen twice running.
            if seen {
                cache.before.clear();
                for range in region(&rows) {
                    cache.before.extend_from_slice(&self.pixels[range]);
                }
            }
        }
        self.blit(&crop(pixels, (left, top, right, bottom)), rect);
        cache.valid = false;
        if seen && columns.start < columns.end && rows.start < rows.end {
            cache.after.clear();
            for range in region(&rows) {
                cache.after.extend_from_slice(&self.pixels[range]);
            }
            cache.region = (columns, rows);
            cache.valid = true;
        }
        self.layers.push(cache);
    }
    /// Where `legacy_layer` puts a layer drawn within `bounds` (left, top,
    /// right, bottom), and the destination columns and rows `blit` can touch.
    fn layer_geometry(
        &self,
        (left, top, right, bottom): (usize, usize, usize, usize),
        factor: f64,
    ) -> ((f64, f64, f64, f64), Range<usize>, Range<usize>) {
        let (w, h) = (self.size[0] as f64, self.size[1] as f64);
        let scale = (w / 640.).min(h / 480.) * factor;
        let rect = (
            (w - 640. * scale) / 2. + left as f64 * scale,
            (h - 480. * scale) / 2. + top as f64 * scale,
            (right - left) as f64 * scale,
            (bottom - top) as f64 * scale,
        );
        let columns =
            (rect.0.floor().max(0.) as usize)..((rect.0 + rect.2).ceil().clamp(0., w) as usize);
        let rows =
            (rect.1.floor().max(0.) as usize)..((rect.1 + rect.3).ceil().clamp(0., h) as usize);
        (rect, columns, rows)
    }
    /// Draws `pixels` from the kept layer `base` (same scale, same bounds):
    /// when the picture beneath is the one it was drawn over, its kept
    /// result is copied and only the pixels a changed layer pixel reaches
    /// are drawn again, over the picture beneath. False, with nothing drawn,
    /// when that does not apply or would not save work.
    fn redraw_changed(&mut self, base: usize, pixels: &[u8], factor: f64) -> bool {
        let Some(bounds @ (left, top, right, bottom)) = self.layers[base].bounds else {
            return false;
        };
        if right == 0 {
            return false;
        }
        let (rect, columns, rows) = self.layer_geometry(bounds, factor);
        let cache = &self.layers[base];
        if columns.start >= columns.end
            || rows.start >= rows.end
            || cache.region != (columns.clone(), rows.clone())
        {
            return false;
        }
        let stride = self.size[0] as usize * 4;
        let span = columns.len() * 4;
        let region = |row: usize| row * stride + columns.start * 4..row * stride + columns.end * 4;
        let kept = |row: usize| (row - rows.start) * span..(row - rows.start + 1) * span;
        if !rows
            .clone()
            .all(|row| self.pixels[region(row)] == cache.before[kept(row)])
        {
            return false;
        }
        let width = right - left;
        let changed: Vec<_> = (top..bottom)
            .map(|y| {
                let at = (y * 640 + left) * 4..(y * 640 + right) * 4;
                partial::changed_spans(&cache.layer[at.clone()], &pixels[at], width)
                    .pop()
                    .flatten()
            })
            .collect();
        let segments = partial::reach(self.size, [width, bottom - top], rect, &changed);
        let area: usize = segments.iter().map(|(_, c)| c.len()).sum();
        if area * 2 >= columns.len() * rows.len() {
            return false;
        }
        let mut cache = self.layers.remove(base);
        for row in rows.clone() {
            self.pixels[region(row)].copy_from_slice(&cache.after[kept(row)]);
        }
        let offset = |row: usize, c: &Range<usize>| {
            let at = (row - rows.start) * span + (c.start - columns.start) * 4;
            (row * stride + c.start * 4, at, c.len() * 4)
        };
        for (row, c) in &segments {
            let (to, from, len) = offset(*row, c);
            self.pixels[to..to + len].copy_from_slice(&cache.before[from..from + len]);
        }
        self.blit_within(&crop(pixels, bounds), rect, Some(&segments));
        for (row, c) in &segments {
            let (from, to, len) = offset(*row, c);
            cache.after[to..to + len].copy_from_slice(&self.pixels[from..from + len]);
        }
        cache.layer.clear();
        cache.layer.extend_from_slice(pixels);
        cache.seen = true;
        self.touched.mark(rows.start, rows.end);
        #[cfg(test)]
        {
            self.layer_redraws += 1;
        }
        self.layers.push(cache);
        true
    }
    pub(crate) fn blit(&mut self, s: &Sprite, rect: (f64, f64, f64, f64)) {
        self.blit_within(s, rect, None);
    }
    /// `blit`, writing only the destination pixels in `only` (row, columns)
    /// when it is given. Each pixel written is the one `blit` writes.
    fn blit_within(
        &mut self,
        s: &Sprite,
        (x, y, w, h): (f64, f64, f64, f64),
        only: Option<&[partial::Segment]>,
    ) {
        let dw = self.size[0] as usize;
        let touched =
            (y.floor().max(0.) as usize)..((y + h).ceil().clamp(0., self.size[1] as f64) as usize);
        self.touched.mark(touched.start, touched.end);
        let whole_pixels = w == s.width as f64
            && h == s.height as f64
            && x >= 0.
            && y >= 0.
            && x.fract() == 0.
            && y.fract() == 0.
            && x + w <= self.size[0] as f64
            && y + h <= self.size[1] as f64;
        // The segments to draw: every row whole, or the ones asked for, cut
        // to `rows` by `columns`.
        let segments = |rows: Range<usize>, columns: Range<usize>| -> Vec<partial::Segment> {
            match only {
                None => rows.map(|row| (row, columns.clone())).collect(),
                Some(only) => only
                    .iter()
                    .filter(|(row, _)| rows.contains(row))
                    .map(|(row, c)| (*row, c.start.max(columns.start)..c.end.min(columns.end)))
                    .filter(|(_, c)| c.start < c.end)
                    .collect(),
            }
        };
        if whole_pixels {
            let (x0, y0) = (x as usize, y as usize);
            let runs = segments(y0..y0 + s.height, x0..x0 + s.width);
            // Retain the row-copy path for completely opaque cached panels.
            if s.rgba.chunks_exact(4).all(|p| p[3] == 255) {
                for (row, columns) in runs {
                    let at = (row * dw + columns.start) * 4;
                    let from = ((row - y0) * s.width + columns.start - x0) * 4;
                    let len = columns.len() * 4;
                    self.pixels[at..at + len].copy_from_slice(&s.rgba[from..from + len]);
                }
                return;
            }
            // Transparent bezel corners must not make an already-sized panel
            // run through the four-tap filter again. Below this coordinate
            // bound its intermediate products are half-integers below 2^48,
            // exactly stored in f64, so the old filter's weights are exactly
            // [1, 0, 0, 0]. The bound also keeps its signed pixel-loop
            // coordinates in range.
            if self.size.iter().all(|&dimension| dimension <= 1 << 24) {
                for (row, columns) in runs {
                    let at = (row * dw + columns.start) * 4;
                    let from = ((row - y0) * s.width + columns.start - x0) * 4;
                    let len = columns.len() * 4;
                    let source = &s.rgba[from..from + len];
                    let destination = &mut self.pixels[at..at + len];
                    for (source, destination) in
                        source.chunks_exact(4).zip(destination.chunks_exact_mut(4))
                    {
                        over_whole(source, destination);
                    }
                }
                return;
            }
        }

        // The four-tap filter. What each destination column and row takes
        // from the source (its two source columns or rows and the weight of
        // the second) depends only on that column or row, so it is worked
        // out once, and each source pixel's colour times its alpha is too.
        // Every product and sum below is the same as the per-pixel original,
        // in the same order.
        let rows = partial::filter_taps(y, h, s.height, self.size[1]);
        let columns = partial::filter_taps(x, w, s.width, self.size[0]);
        let (Some(first_row), Some(first_column)) = (rows.first(), columns.first()) else {
            return;
        };
        let runs = segments(
            first_row.0..first_row.0 + rows.len(),
            first_column.0..first_column.0 + columns.len(),
        );
        let premultiply = |p: &[u8]| {
            let alpha = p[3] as f64 / 255.;
            [
                p[0] as f64 * alpha,
                p[1] as f64 * alpha,
                p[2] as f64 * alpha,
                alpha,
            ]
        };
        let mut premultiplied = std::mem::take(&mut self.premultiplied);
        if only.is_none() {
            premultiplied.clear();
            premultiplied.extend(s.rgba.chunks_exact(4).map(premultiply));
        } else {
            // Only the source rows the segments read; the rest are never
            // looked at.
            premultiplied.resize(s.width * s.height, [0.; 4]);
            let mut needed = vec![false; s.height];
            for (row, _) in &runs {
                let (_, sy, sy1, _) = rows[row - first_row.0];
                needed[sy] = true;
                needed[sy1] = true;
            }
            for (row, _) in needed.iter().enumerate().filter(|(_, n)| **n) {
                let stride = s.width * 4;
                for (to, p) in premultiplied[row * s.width..(row + 1) * s.width]
                    .iter_mut()
                    .zip(s.rgba[row * stride..(row + 1) * stride].chunks_exact(4))
                {
                    *to = premultiply(p);
                }
            }
        }
        for (row, span) in runs {
            let (yy, sy, sy1, fy) = rows[row - first_row.0];
            for &(xx, sx, sx1, fx) in
                &columns[span.start - first_column.0..span.end - first_column.0]
            {
                let taps = [
                    (&premultiplied[sy * s.width + sx], (1. - fx) * (1. - fy)),
                    (&premultiplied[sy * s.width + sx1], fx * (1. - fy)),
                    (&premultiplied[sy1 * s.width + sx], (1. - fx) * fy),
                    (&premultiplied[sy1 * s.width + sx1], fx * fy),
                ];
                // Four clear pixels sum to nothing whatever their (finite)
                // weights.
                if taps
                    .iter()
                    .all(|(tap, weight)| tap[3] == 0. && weight.is_finite())
                {
                    continue;
                }
                let mut rgba = [0.; 4];
                for (tap, weight) in taps {
                    for (sum, value) in rgba.iter_mut().zip(tap) {
                        *sum += value * weight;
                    }
                }
                if rgba[3] == 0. {
                    continue;
                }
                let at = (yy * dw + xx) * 4;
                let old = self.pixels[at + 3] as f64 / 255.;
                let alpha = rgba[3] + old * (1. - rgba[3]);
                for (c, value) in rgba.iter().enumerate().take(3) {
                    self.pixels[at + c] =
                        ((value + self.pixels[at + c] as f64 * old * (1. - rgba[3])) / alpha)
                            .round() as u8;
                }
                self.pixels[at + 3] = (alpha * 255.).round() as u8;
            }
        }
        self.premultiplied = premultiplied;
    }
}

/// Where a 640x480 layer has anything drawn: left, top, right, bottom
/// (right 0 when nothing is).
fn layer_bounds(pixels: &[u8]) -> (usize, usize, usize, usize) {
    let (mut left, mut top, mut right, mut bottom) = (640usize, 480usize, 0usize, 0usize);
    for (i, p) in pixels.chunks_exact(4).enumerate() {
        if p[3] != 0 {
            left = left.min(i % 640);
            right = right.max(i % 640 + 1);
            top = top.min(i / 640);
            bottom = bottom.max(i / 640 + 1);
        }
    }
    (left, top, right, bottom)
}
/// The part of a 640x480 layer within `bounds`, as a sprite.
fn crop(pixels: &[u8], (left, top, right, bottom): (usize, usize, usize, usize)) -> Sprite {
    let mut rgba = Vec::with_capacity((right - left) * (bottom - top) * 4);
    for y in top..bottom {
        rgba.extend_from_slice(&pixels[(y * 640 + left) * 4..(y * 640 + right) * 4]);
    }
    Sprite {
        width: right - left,
        height: bottom - top,
        rgba,
        glyphs: vec![],
    }
}
/// One source pixel over one destination pixel, as `blit` draws a sprite
/// at its own size.
fn over_whole(source: &[u8], destination: &mut [u8]) {
    match source[3] {
        0 => {}
        255 => destination.copy_from_slice(source),
        _ => {
            let source_alpha = f64::from(source[3]) / 255.;
            let old = f64::from(destination[3]) / 255.;
            let alpha = source_alpha + old * (1. - source_alpha);
            for c in 0..3 {
                let value = f64::from(source[c]) * source_alpha;
                destination[c] = ((value + f64::from(destination[c]) * old * (1. - source_alpha))
                    / alpha)
                    .round() as u8;
            }
            destination[3] = (alpha * 255.).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The pre-optimization filter, independent of blit's integer-size paths.
    fn filtered_reference(
        canvas: &mut FlightCanvas,
        s: &Sprite,
        (x, y, w, h): (f64, f64, f64, f64),
    ) {
        let dw = canvas.size[0] as usize;
        for yy in (y.floor() as i32).max(0)..((y + h).ceil() as i32).min(canvas.size[1] as i32) {
            for xx in (x.floor() as i32).max(0)..((x + w).ceil() as i32).min(canvas.size[0] as i32)
            {
                let u = ((xx as f64 + 0.5 - x) * s.width as f64 / w - 0.5)
                    .clamp(0., (s.width - 1) as f64);
                let v = ((yy as f64 + 0.5 - y) * s.height as f64 / h - 0.5)
                    .clamp(0., (s.height - 1) as f64);
                let (sx, sy) = (u.floor() as usize, v.floor() as usize);
                let (fx, fy) = (u - sx as f64, v - sy as f64);
                let mut rgba = [0.; 4];
                for (px, py, weight) in [
                    (sx, sy, (1. - fx) * (1. - fy)),
                    ((sx + 1).min(s.width - 1), sy, fx * (1. - fy)),
                    (sx, (sy + 1).min(s.height - 1), (1. - fx) * fy),
                    (
                        (sx + 1).min(s.width - 1),
                        (sy + 1).min(s.height - 1),
                        fx * fy,
                    ),
                ] {
                    let at = (py * s.width + px) * 4;
                    let alpha = s.rgba[at + 3] as f64 / 255.;
                    for (c, sum) in rgba.iter_mut().enumerate().take(3) {
                        *sum += s.rgba[at + c] as f64 * alpha * weight;
                    }
                    rgba[3] += alpha * weight;
                }
                if rgba[3] == 0. {
                    continue;
                }
                let at = (yy as usize * dw + xx as usize) * 4;
                let old = canvas.pixels[at + 3] as f64 / 255.;
                let alpha = rgba[3] + old * (1. - rgba[3]);
                for (c, value) in rgba.iter().enumerate().take(3) {
                    canvas.pixels[at + c] =
                        ((value + canvas.pixels[at + c] as f64 * old * (1. - rgba[3])) / alpha)
                            .round() as u8;
                }
                canvas.pixels[at + 3] = (alpha * 255.).round() as u8;
            }
        }
    }

    #[test]
    fn integer_blit_matches_filter_for_every_source_and_destination_alpha() {
        for seed in [0u8, 1, 127, 255] {
            let mut source = Sprite {
                width: 256,
                height: 256,
                rgba: Vec::new(),
                glyphs: Vec::new(),
            };
            let mut destination = Vec::new();
            for source_alpha in 0..=255u8 {
                for destination_alpha in 0..=255u8 {
                    source.rgba.extend_from_slice(&[
                        source_alpha.wrapping_mul(37) ^ destination_alpha ^ seed,
                        destination_alpha.wrapping_add(31).wrapping_add(seed),
                        source_alpha ^ destination_alpha.rotate_left(1),
                        source_alpha,
                    ]);
                    destination.extend_from_slice(&[
                        destination_alpha.wrapping_mul(11) ^ seed,
                        255 - source_alpha,
                        source_alpha
                            .wrapping_add(destination_alpha)
                            .wrapping_add(seed),
                        destination_alpha,
                    ]);
                }
            }
            let mut actual = FlightCanvas {
                size: [256, 256],
                pixels: destination.clone(),
                ..Default::default()
            };
            let mut expected = FlightCanvas {
                size: [256, 256],
                pixels: destination,
                ..Default::default()
            };
            actual.blit(&source, (0., 0., 256., 256.));
            filtered_reference(&mut expected, &source, (0., 0., 256., 256.));
            assert_eq!(actual.pixels, expected.pixels, "RGB seed {seed}");
        }
    }

    fn patterned_sprite(width: usize, height: usize) -> Sprite {
        Sprite {
            width,
            height,
            rgba: (0..width * height)
                .flat_map(|index| {
                    let value = index as u8;
                    [
                        value.wrapping_mul(37),
                        value.wrapping_add(127),
                        255 - value,
                        value.wrapping_mul(17),
                    ]
                })
                .collect(),
            glyphs: Vec::new(),
        }
    }

    fn compare_blit(source: &Sprite, size: [u32; 2], rect: (f64, f64, f64, f64)) {
        let pixels = patterned_sprite(size[0] as usize, size[1] as usize).rgba;
        let mut actual = FlightCanvas {
            size,
            pixels: pixels.clone(),
            ..Default::default()
        };
        let mut expected = FlightCanvas {
            size,
            pixels,
            ..Default::default()
        };
        actual.blit(source, rect);
        filtered_reference(&mut expected, source, rect);
        assert_eq!(
            actual.pixels, expected.pixels,
            "canvas {size:?}, rect {rect:?}"
        );
    }

    #[test]
    fn integer_blit_matches_panel_sizes_and_canvas_boundaries() {
        // Large panels at 640, 1280 and 1920 window widths, plus a small
        // 1920-wide panel. The rounded width may be odd while height is even.
        for (width, height) in [(1, 1), (162, 160), (243, 240), (365, 360), (216, 214)] {
            let source = patterned_sprite(width, height);
            for (x, y) in [(0., 0.), (7., 0.), (0., 5.), (7., 5.)] {
                compare_blit(
                    &source,
                    [width as u32 + 7, height as u32 + 5],
                    (x, y, width as f64, height as f64),
                );
            }
        }
    }

    #[test]
    fn integer_blit_preserves_scaled_fractional_and_clipped_filters() {
        let source = patterned_sprite(8, 5);
        for rect in [
            (-1., 0., 8., 5.),
            (0., -1., 8., 5.),
            (10., 10., 8., 5.),
            (0.5, 1., 8., 5.),
            (1., 0.5, 8., 5.),
            (1., 1., 12., 8.),
            (1., 1., 4., 3.),
        ] {
            compare_blit(&source, [17, 13], rect);
        }
    }

    #[test]
    fn target_square_scales_with_the_window_and_clips_at_its_edge() {
        let mut canvas = FlightCanvas {
            size: [96, 96],
            pixels: vec![0; 96 * 96 * 4],
            ..Default::default()
        };
        canvas.size = [640, 480];
        canvas.pixels = vec![0; 640 * 480 * 4];
        canvas.target_square(
            [320., 240.],
            1.,
            [0, 255, 0],
            aim_box::Style::Tracked { friendly: false },
        );
        let alpha = |c: &FlightCanvas, x: usize, y: usize| c.pixels[(y * 640 + x) * 4 + 3];
        // At 640 by 480 the HUD scale is 0.7225: a 10-pixel square of thin,
        // partly covered lines, never a solid bold outline.
        let edge = (320. + 7. * HUD_SCALE) as usize;
        assert!(alpha(&canvas, edge, 240) > 0);
        assert!(alpha(&canvas, edge, 240) < 255);
        assert_eq!(alpha(&canvas, 320, 240), 0);
        assert_eq!(alpha(&canvas, edge + 3, 240), 0);
        canvas.target_square(
            [0., 0.],
            1.,
            [0, 255, 0],
            aim_box::Style::Tracked { friendly: true },
        );
    }
    #[test]
    fn edge_chevron_sits_inside_the_edge_and_points_outward() {
        let mut canvas = FlightCanvas {
            size: [640, 480],
            pixels: vec![0; 640 * 480 * 4],
            ..Default::default()
        };
        canvas.edge_chevron([1., 0.], 1., [0, 255, 0], &[]);
        let lit: Vec<(usize, usize)> = canvas
            .pixels
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, p)| p[3] > 0)
            .map(|(i, _)| (i % 640, i / 640))
            .collect();
        assert!(!lit.is_empty());
        let right = lit.iter().map(|p| p.0).max().unwrap();
        let left = lit.iter().map(|p| p.0).min().unwrap();
        // The tip is 12 HUD pixels in from the right edge, the arms trail
        // back towards the middle, and the arrow is centred on the row.
        assert!(right < 640 && right > 640 - 20, "{right}");
        assert!(left < right - 4);
        let rows = (
            lit.iter().map(|p| p.1).min().unwrap(),
            lit.iter().map(|p| p.1).max().unwrap(),
        );
        assert!(rows.0 < 240 && rows.1 > 240 && (rows.0 + rows.1).abs_diff(480) <= 2);
    }
    #[test]
    fn aim_diamond_is_smaller_than_the_box() {
        let extent = |draw: &dyn Fn(&mut FlightCanvas)| {
            let mut canvas = FlightCanvas {
                size: [640, 480],
                pixels: vec![0; 640 * 480 * 4],
                ..Default::default()
            };
            draw(&mut canvas);
            let xs: Vec<usize> = canvas
                .pixels
                .chunks_exact(4)
                .enumerate()
                .filter(|(_, p)| p[3] > 0)
                .map(|(i, _)| i % 640)
                .collect();
            xs.iter().max().unwrap() - xs.iter().min().unwrap()
        };
        let diamond = extent(&|c| c.aim_diamond([320., 240.], 1., [0, 255, 0]));
        let square = extent(&|c| {
            c.target_square([320., 240.], 1., [0, 255, 0], aim_box::Style::Free);
        });
        assert!(
            diamond * 2 <= square + 2 && diamond >= 3,
            "{diamond} {square}"
        );
        // A pinned point is the square plus a centre dot.
        let mut canvas = FlightCanvas {
            size: [640, 480],
            pixels: vec![0; 640 * 480 * 4],
            ..Default::default()
        };
        canvas.target_square([320., 240.], 1., [0, 255, 0], aim_box::Style::Pinned);
        assert!(canvas.pixels[(240 * 640 + 320) * 4 + 3] > 0);
    }
    #[test]
    fn veil_darkens_the_world_and_instruments_edges_first() {
        let mut canvas = FlightCanvas {
            size: [4, 2],
            pixels: vec![0; 4 * 2 * 4],
            ..Default::default()
        };
        // An opaque white instrument pixel in the top-left corner.
        canvas.pixels[..4].copy_from_slice(&[255; 4]);
        canvas.veil([0, 0, 0], |radius| if radius > 0.5 { 0.5 } else { 0. });
        assert_eq!(&canvas.pixels[..4], &[128, 128, 128, 255]);
        // An empty corner now darkens the world under it.
        assert_eq!(&canvas.pixels[12..16], &[0, 0, 0, 128]);
        // The centre is untouched.
        assert_eq!(&canvas.pixels[4..8], &[0; 4]);
    }
    #[test]
    fn transparent_filtering_keeps_color_without_dark_halos() {
        let mut canvas = FlightCanvas {
            size: [4, 4],
            pixels: vec![0; 64],
            ..Default::default()
        };
        let sprite = Sprite {
            width: 2,
            height: 2,
            rgba: vec![0, 200, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            glyphs: vec![],
        };
        canvas.blit(&sprite, (0., 0., 4., 4.));
        for p in canvas.pixels.chunks_exact(4).filter(|p| p[3] > 0) {
            assert_eq!(p[1], 200);
        }
    }
    #[test]
    fn an_anchored_layer_keeps_its_bottom_on_the_bottom_edge() {
        let mut layer = vec![0u8; 640 * 480 * 4];
        for (x, y) in [(15, 5), (15, 470)] {
            layer[(y * 640 + x) * 4..][..4].copy_from_slice(&[200, 10, 10, 255]);
        }
        let alpha =
            |c: &FlightCanvas, x: usize, y: usize| c.pixels[(y * c.size[0] as usize + x) * 4 + 3];
        // 4:3 and wide: where the centred layer draws it.
        for (size, left, scale) in [([1280, 960], 0., 2.), ([1920, 1080], 240., 2.25)] {
            let mut canvas = FlightCanvas::default();
            canvas.blank(size);
            canvas.anchored_layer(&layer);
            for y in [5., 470.] {
                let at = |v: f64| (v * scale + scale / 2.) as usize;
                assert_eq!(
                    alpha(&canvas, left as usize + at(15.), at(y)),
                    255,
                    "{size:?}"
                );
            }
        }
        // Tall: the top half at the top, the bottom half at the bottom.
        let mut canvas = FlightCanvas::default();
        canvas.blank([640, 1000]);
        canvas.anchored_layer(&layer);
        assert_eq!(alpha(&canvas, 15, 5), 255);
        assert_eq!(alpha(&canvas, 15, 1000 - 10), 255);
        assert_eq!(alpha(&canvas, 15, 260 + 470), 0);
        // Blanking clears the canvas for the next frame.
        canvas.blank([640, 1000]);
        assert!(canvas.pixels.iter().all(|v| *v == 0));
    }
    #[test]
    fn centered_rects_draw_only_their_part_of_the_layer_centred() {
        let mut layer = vec![0u8; 640 * 480 * 4];
        for (x, y) in [(20, 30), (20, 400), (600, 30)] {
            layer[(y * 640 + x) * 4..][..4].copy_from_slice(&[200, 10, 10, 255]);
        }
        let alpha =
            |c: &FlightCanvas, x: usize, y: usize| c.pixels[(y * c.size[0] as usize + x) * 4 + 3];
        // Wide: centred across; tall: centred up and down, unlike the
        // anchored layer, so a tall panel never parts in the middle.
        for (size, left, top, scale) in [
            ([1920, 1080], 240., 0., 2.25),
            ([640, 1000], 0., 260., 1.),
            ([1280, 960], 0., 0., 2.),
        ] {
            let mut canvas = FlightCanvas::default();
            canvas.blank(size);
            canvas.centered_rects(&layer, &[(10, 20, 100, 400), (-5, -5, 2, 2)]);
            let at = |v: f64, origin: f64| (origin + v * scale + scale / 2.) as usize;
            assert_eq!(alpha(&canvas, at(20., left), at(30., top)), 255, "{size:?}");
            assert_eq!(
                alpha(&canvas, at(20., left), at(400., top)),
                255,
                "{size:?}"
            );
            // Outside the rectangles nothing is drawn.
            assert_eq!(alpha(&canvas, at(600., left), at(30., top)), 0, "{size:?}");
        }
    }
    #[test]
    fn scaled_layers_keep_hard_pixels_and_blend_translucent_ones() {
        let sprite = Sprite {
            width: 2,
            height: 1,
            rgba: vec![200, 10, 10, 255, 0, 0, 255, 128],
            glyphs: vec![],
        };
        let mut canvas = FlightCanvas::default();
        canvas.blank([9, 3]);
        // Under the translucent pixel, an opaque white one.
        canvas.pixels[(9 + 6) * 4..][..4].copy_from_slice(&[255; 4]);
        canvas.scaled(&sprite, (0.5, 0., 9., 3.));
        let at = |x: usize, y: usize| canvas.pixels[(y * 9 + x) * 4..][..4].to_vec();
        // Each source pixel covers four and a half columns: 0.5..5 and 5..9.5.
        assert_eq!(at(0, 0), [0; 4]);
        assert_eq!(at(1, 1), [200, 10, 10, 255]);
        assert_eq!(at(4, 2), [200, 10, 10, 255]);
        assert_eq!(at(7, 0), [0, 0, 255, 128]);
        assert_eq!(at(6, 1), [127, 127, 255, 255]);
    }
    #[test]
    fn labels_draw_scaled_with_a_shadow() {
        let font = tore_formats::font::Font {
            height: 2,
            glyphs: (0..256)
                .map(|c| tore_formats::font::Glyph {
                    advance: 3,
                    pixels: if c == usize::from(b'A') {
                        vec![(0, 0), (1, 1)]
                    } else {
                        vec![]
                    },
                })
                .collect(),
        };
        assert_eq!(FlightCanvas::text_width(&font, "AA", 2.), 12.);
        let mut canvas = FlightCanvas::default();
        canvas.blank([20, 20]);
        canvas.text(&font, "A", [4., 4.], 2., [255, 255, 255]);
        let at = |x: usize, y: usize| &canvas.pixels[(y * 20 + x) * 4..][..4];
        assert_eq!(at(4, 4), [255, 255, 255, 255]);
        assert_eq!(at(7, 7), [255, 255, 255, 255]);
        // The shadow fills in beside the text, never over it.
        assert_eq!(at(9, 9), [0, 0, 0, 255]);
        assert_eq!(at(6, 4), [0; 4]);
    }
    #[test]
    fn shrinking_hud_preserves_camera_projection_on_wide_and_tall_screens() {
        for size in [[1920, 1080], [800, 1200], [960, 720]] {
            let canvas = FlightCanvas {
                size,
                ..Default::default()
            };
            let scale = (size[0] as f64 / 640.).min(size[1] as f64 / 480.) * HUD_SCALE;
            let focal = 240. * 3f64.sqrt() * canvas.hud_zoom(1.) as f64 * scale;
            assert!((focal - size[1] as f64 * 0.5 * 3f64.sqrt()).abs() < 0.001);
        }
    }

    /// A pseudo-random sequence, so the scribbles are the same every run.
    struct Dice(u32);
    impl Dice {
        fn next(&mut self) -> usize {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 12) as usize
        }
    }

    fn sprite(dice: &mut Dice, width: usize, height: usize) -> Sprite {
        let mut rgba = Vec::new();
        for _ in 0..width * height {
            let alpha = [0, 0, 255, 128, 77, 255][dice.next() % 6];
            rgba.extend_from_slice(&[
                (dice.next() % 256) as u8,
                (dice.next() % 256) as u8,
                (dice.next() % 256) as u8,
                alpha,
            ]);
        }
        Sprite {
            width,
            height,
            rgba,
            glyphs: vec![],
        }
    }

    /// Draws a frame's worth of everything that writes to the canvas.
    fn scribble(canvas: &mut FlightCanvas, frame: usize) {
        let mut dice = Dice(frame as u32 * 7919 + 1);
        let [w, h] = canvas.size.map(|n| n as usize);
        let layer: Vec<u8> = {
            let mut layer = vec![0u8; 640 * 480 * 4];
            for _ in 0..40 {
                let at = (dice.next() % (640 * 480)) * 4;
                layer[at..at + 4].copy_from_slice(&[200, 90, 10, 255]);
            }
            layer
        };
        for _ in 0..dice.next() % 5 {
            let sprite = sprite(&mut dice, 6, 4);
            let place = (
                (dice.next() % (w + 20)) as f64 - 10.,
                (dice.next() % (h + 20)) as f64 - 10.,
            );
            // Alternately a whole-pixel copy and a filtered one.
            let size = if dice.next().is_multiple_of(2) {
                (6., 4.)
            } else {
                (9.7, 6.3)
            };
            canvas.blit(&sprite, (place.0.round(), place.1.round(), size.0, size.1));
            canvas.scaled(&sprite, (place.0, place.1, size.0 * 1.5, size.1 * 1.5));
        }
        for _ in 0..dice.next() % 20 {
            canvas.blend(
                (dice.next() % (w + 4)) as i32 - 2,
                (dice.next() % (h + 4)) as i32 - 2,
                [10, 200, 30],
                0.1 + (dice.next() % 9) as f64 / 10.,
            );
            canvas.dot(
                (dice.next() % w) as f64,
                (dice.next() % h) as f64,
                1.5,
                [0, 255, 0],
            );
        }
        canvas.blend_rect(
            (
                (dice.next() % w) as i32 - 3,
                (dice.next() % h) as i32 - 3,
                (dice.next() % w) as i32 + 3,
                (dice.next() % h) as i32 + 3,
            ),
            [0, 0, 0],
            0.65,
        );
        if frame.is_multiple_of(4) {
            canvas.anchored_layer(&layer);
        }
        if frame % 5 == 3 {
            canvas.centered_rects(&layer, &[(100, 100, 300, 200)]);
        }
        if frame % 6 == 2 {
            canvas.veil([150, 0, 0], |radius| radius * 0.5);
        }
        if frame % 11 == 7 {
            canvas.fill_opaque([72, 72, 72]);
        }
        if frame % 13 == 9 {
            canvas.blank(canvas.size);
        }
        if frame % 3 == 1 {
            canvas.legacy_layer(&layer, 1.);
        }
    }

    /// The renderer's side of an upload, on a CPU copy of the texture.
    #[derive(Default)]
    struct FakeTexture {
        size: [u32; 2],
        texture: Vec<u8>,
        held: Option<Vec<u8>>,
        rows_sent: usize,
    }
    impl FakeTexture {
        fn upload(&mut self, canvas: &mut FlightCanvas) {
            canvas.prepare_upload();
            let stride = canvas.size[0] as usize * 4;
            let height = canvas.size[1] as usize;
            if self.size != canvas.size {
                self.size = canvas.size;
                self.texture = vec![0; canvas.pixels.len()];
                self.held = None;
            }
            let bands = match &mut self.held {
                Some(held) if canvas.pending.len() == height => crate::canvas_rows::changed_bands(
                    &canvas.pixels,
                    held,
                    stride,
                    &canvas.pending,
                    crate::canvas_rows::GAP,
                ),
                held => {
                    *held = Some(canvas.pixels.clone());
                    std::iter::once(0..height).collect()
                }
            };
            canvas.pending.clear();
            for band in bands {
                self.rows_sent += band.len();
                self.texture[band.start * stride..band.end * stride]
                    .copy_from_slice(&canvas.pixels[band.start * stride..band.end * stride]);
            }
        }
    }

    #[test]
    fn clearing_only_the_rows_drawn_on_gives_the_same_canvas_as_clearing_all() {
        let mut reused = FlightCanvas::default();
        let mut gpu = FakeTexture::default();
        let sizes = [[64, 48], [64, 48], [64, 48], [80, 60], [80, 60], [64, 48]];
        for frame in 0..90 {
            let size = sizes[frame / 15];
            reused.clear_for(size);
            scribble(&mut reused, frame);
            let mut fresh = FlightCanvas::default();
            fresh.blank(size);
            scribble(&mut fresh, frame);
            assert_eq!(reused.pixels, fresh.pixels, "frame {frame}");
            // Rows nobody drew on are clear, which is what lets the next
            // frame leave them alone.
            let lit = crate::canvas_rows::lit_rows(&reused.pixels, size[0] as usize);
            for run in lit.runs() {
                for row in run {
                    assert!(reused.touched.contains(row), "frame {frame} row {row}");
                }
            }
            // Some frames are not presented (a lost surface), so their rows
            // must still go up with the next one.
            if frame % 4 != 1 {
                gpu.upload(&mut reused);
                assert_eq!(gpu.texture, reused.pixels, "frame {frame}");
            }
        }
    }

    #[test]
    fn an_unchanged_canvas_uploads_no_rows_and_a_small_change_uploads_few() {
        let mut canvas = FlightCanvas::default();
        let mut gpu = FakeTexture::default();
        let still = |canvas: &mut FlightCanvas| {
            canvas.clear_for([64, 48]);
            canvas.blend_rect((4, 10, 30, 14), [0, 0, 0], 0.5);
        };
        still(&mut canvas);
        gpu.upload(&mut canvas);
        assert_eq!(gpu.rows_sent, 48);
        for _ in 0..3 {
            still(&mut canvas);
            gpu.upload(&mut canvas);
        }
        assert_eq!(gpu.rows_sent, 48, "redrawn identically");
        canvas.clear_for([64, 48]);
        canvas.blend_rect((4, 10, 30, 14), [0, 0, 0], 0.5);
        canvas.blend_rect((4, 40, 8, 41), [0, 0, 0], 0.5);
        gpu.upload(&mut canvas);
        assert_eq!(gpu.rows_sent, 49);
        // Drawing it away again sends those rows once more and no others.
        canvas.clear_for([64, 48]);
        gpu.upload(&mut canvas);
        assert_eq!(gpu.rows_sent, 49 + 5);
        assert_eq!(gpu.texture, canvas.pixels);
    }

    #[test]
    fn blending_a_rectangle_matches_blending_each_pixel() {
        for (alpha, color) in [
            (0.65, [0, 0, 0]),
            (0.65, [90, 200, 17]),
            (1., [3, 4, 5]),
            (0.3, [255, 255, 255]),
            (0.01, [10, 20, 30]),
            (0., [10, 20, 30]),
        ] {
            let mut dice = Dice(5);
            let mut base = FlightCanvas::default();
            base.blank([20, 12]);
            for p in base.pixels.chunks_exact_mut(4) {
                let alpha = [0, 0, 255, 17, 128, 254][dice.next() % 6];
                p.copy_from_slice(&[
                    (dice.next() % 256) as u8,
                    (dice.next() % 256) as u8,
                    (dice.next() % 256) as u8,
                    alpha,
                ]);
            }
            for rect in [
                (2, 3, 15, 9),
                (-4, -4, 30, 30),
                (5, 5, 5, 8),
                (18, 1, 40, 3),
            ] {
                let mut fast = FlightCanvas {
                    pixels: base.pixels.clone(),
                    size: base.size,
                    ..Default::default()
                };
                let mut each = FlightCanvas {
                    pixels: base.pixels.clone(),
                    size: base.size,
                    ..Default::default()
                };
                fast.blend_rect(rect, color, alpha);
                for y in rect.1..rect.3 {
                    for x in rect.0..rect.2 {
                        each.blend(x, y, color, alpha);
                    }
                }
                assert_eq!(fast.pixels, each.pixels, "{rect:?} alpha {alpha}");
            }
        }
    }

    #[test]
    fn the_hoisted_filter_matches_the_per_pixel_original_on_random_layers() {
        let mut dice = Dice(99);
        for case in 0..60 {
            let (sw, sh) = (1 + dice.next() % 24, 1 + dice.next() % 20);
            let source = sprite(&mut dice, sw, sh);
            let size = [
                30 + (dice.next() % 20) as u32,
                26 + (dice.next() % 12) as u32,
            ];
            let mut actual = FlightCanvas::default();
            actual.blank(size);
            for p in actual.pixels.chunks_exact_mut(4) {
                let alpha = [0, 255, 90][dice.next() % 3];
                p.copy_from_slice(&[
                    (dice.next() % 256) as u8,
                    (dice.next() % 256) as u8,
                    (dice.next() % 256) as u8,
                    alpha,
                ]);
            }
            let mut expected = FlightCanvas {
                pixels: actual.pixels.clone(),
                size,
                ..Default::default()
            };
            let rect = (
                (dice.next() % 400) as f64 / 10. - 12.,
                (dice.next() % 300) as f64 / 10. - 9.,
                0.5 + (dice.next() % 500) as f64 / 10.,
                0.5 + (dice.next() % 400) as f64 / 10.,
            );
            actual.blit(&source, rect);
            filtered_reference(&mut expected, &source, rect);
            assert_eq!(actual.pixels, expected.pixels, "case {case} {rect:?}");
        }
    }

    /// `legacy_layer` as it was before it kept its results.
    fn legacy_layer_reference(canvas: &mut FlightCanvas, pixels: &[u8], factor: f64) {
        let (mut left, mut top, mut right, mut bottom) = (640usize, 480usize, 0usize, 0usize);
        for (i, p) in pixels.chunks_exact(4).enumerate() {
            if p[3] != 0 {
                left = left.min(i % 640);
                right = right.max(i % 640 + 1);
                top = top.min(i / 640);
                bottom = bottom.max(i / 640 + 1);
            }
        }
        if right == 0 {
            return;
        }
        let (width, height) = (right - left, bottom - top);
        let mut rgba = Vec::new();
        for y in top..bottom {
            rgba.extend_from_slice(&pixels[(y * 640 + left) * 4..(y * 640 + right) * 4]);
        }
        let (w, h) = (canvas.size[0] as f64, canvas.size[1] as f64);
        let scale = (w / 640.).min(h / 480.) * factor;
        canvas.blit(
            &Sprite {
                width,
                height,
                rgba,
                glyphs: vec![],
            },
            (
                (w - 640. * scale) / 2. + left as f64 * scale,
                (h - 480. * scale) / 2. + top as f64 * scale,
                width as f64 * scale,
                height as f64 * scale,
            ),
        );
    }

    #[test]
    fn a_layer_drawn_again_gives_the_pixels_a_fresh_scaling_would() {
        let mut dice = Dice(31);
        let layer = |dice: &mut Dice, lit: usize| {
            let mut layer = vec![0u8; 640 * 480 * 4];
            for _ in 0..lit {
                let at = (60 + dice.next() % 500 + 640 * (50 + dice.next() % 350)) * 4;
                layer[at..at + 4].copy_from_slice(&[
                    (dice.next() % 256) as u8,
                    90,
                    10,
                    [255, 128, 40][dice.next() % 3],
                ]);
            }
            layer
        };
        let (a, b) = (layer(&mut dice, 800), layer(&mut dice, 800));
        let empty = vec![0u8; 640 * 480 * 4];
        let base = |dice: &mut Dice, size: [u32; 2]| {
            let mut canvas = FlightCanvas::default();
            canvas.blank(size);
            for p in canvas.pixels.chunks_exact_mut(4) {
                p.copy_from_slice(&[
                    (dice.next() % 256) as u8,
                    40,
                    50,
                    [0, 255, 200][dice.next() % 3],
                ]);
            }
            canvas.pixels
        };
        let mut cached = FlightCanvas::default();
        let other = base(&mut dice, [160, 120]);
        // The sequence: stills, a change underneath, alternating layers (the
        // map, then a menu), a new size, an empty layer, a new factor.
        let steps: Vec<(&[u8], f64, [u32; 2], usize)> = vec![
            (&a, 1., [160, 120], 0),
            (&a, 1., [160, 120], 0),
            (&a, 1., [160, 120], 0),
            (&a, 1., [160, 120], 1),
            (&a, 1., [160, 120], 1),
            (&b, 1., [160, 120], 1),
            (&a, 1., [160, 120], 1),
            (&b, 1., [160, 120], 1),
            (&a, 1., [160, 120], 1),
            (&b, 1., [160, 120], 1),
            (&a, 1., [200, 90], 2),
            (&a, 1., [200, 90], 2),
            (&empty, 1., [200, 90], 2),
            (&a, 0.5, [200, 90], 2),
            (&a, 0.5, [200, 90], 2),
            (&a, 1., [200, 90], 2),
        ];
        let mut hits = Vec::new();
        let bases: Vec<Vec<u8>> = vec![
            base(&mut dice, [160, 120]),
            other,
            base(&mut dice, [200, 90]),
        ];
        for (n, (layer, factor, size, which)) in steps.into_iter().enumerate() {
            let under = bases[which].clone();
            let mut expected = FlightCanvas::default();
            expected.blank(size);
            expected.pixels = under.clone();
            legacy_layer_reference(&mut expected, layer, factor);
            cached.blank(size);
            cached.pixels = under;
            cached.legacy_layer(layer, factor);
            assert_eq!(cached.pixels, expected.pixels, "step {n}");
            // The rows it drew on are noted, hit or miss.
            for row in 0..size[1] as usize {
                let stride = size[0] as usize * 4;
                if cached.pixels[row * stride..(row + 1) * stride]
                    != bases[which][row * stride..(row + 1) * stride]
                {
                    assert!(cached.touched.contains(row), "step {n} row {row}");
                }
            }
            hits.push(cached.layer_hits);
        }
        // Stills hit; so do two layers taking turns over an unchanged canvas.
        assert_eq!(hits[2], 1, "{hits:?}");
        assert!(hits[4] > hits[3], "{hits:?}");
        assert!(hits[9] >= hits[5] + 2, "{hits:?}");
    }

    #[test]
    fn a_layer_changing_in_place_gives_the_pixels_a_fresh_scaling_would() {
        let mut dice = Dice(77);
        // A map-like layer: an opaque field with a few marks that move.
        let mut layer = vec![0u8; 640 * 480 * 4];
        for y in 30..450 {
            for x in 40..600 {
                let at = (y * 640 + x) * 4;
                layer[at..at + 4].copy_from_slice(&[(x % 251) as u8, (y % 13) as u8 * 9, 70, 255]);
            }
        }
        let mark = |layer: &mut Vec<u8>, dice: &mut Dice| {
            let (x, y) = (45 + dice.next() % 540, 35 + dice.next() % 400);
            let alpha = [255, 128, 3][dice.next() % 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (2, 2)] {
                let at = ((y + dy) * 640 + x + dx) * 4;
                layer[at..at + 4].copy_from_slice(&[255, (dice.next() % 256) as u8, 0, alpha]);
            }
        };
        for (size, factor) in [
            ([1920, 1080], 1.),
            ([700, 500], 1.),
            ([640, 480], 1.),
            ([900, 700], 0.75),
        ] {
            let mut under = vec![0u8; size[0] as usize * size[1] as usize * 4];
            for p in under.chunks_exact_mut(4) {
                p.copy_from_slice(&[72, 72, 72, 255]);
            }
            let mut cached = FlightCanvas::default();
            let mut current = layer.clone();
            for step in 0..14 {
                match step {
                    // Still, then small moves, a change beneath, a big change.
                    0 | 1 => {}
                    7 => under[size[0] as usize * 4 * 3 + 40] ^= 1,
                    10 => {
                        for p in current.chunks_exact_mut(4).step_by(3) {
                            p[0] = p[0].wrapping_add(1);
                        }
                    }
                    _ => {
                        for _ in 0..3 {
                            mark(&mut current, &mut dice);
                        }
                    }
                }
                let mut expected = FlightCanvas::default();
                expected.blank(size);
                expected.pixels = under.clone();
                legacy_layer_reference(&mut expected, &current, factor);
                cached.blank(size);
                cached.pixels = under.clone();
                cached.legacy_layer(&current, factor);
                assert!(
                    cached.pixels == expected.pixels,
                    "{size:?} step {step}: first difference {:?}",
                    cached
                        .pixels
                        .iter()
                        .zip(&expected.pixels)
                        .position(|(a, b)| a != b)
                );
            }
            assert!(
                cached.layer_redraws >= 5,
                "{size:?} {}",
                cached.layer_redraws
            );
        }
    }

    #[test]
    fn a_page_scaled_again_from_its_last_image_matches_scaling_it_whole() {
        let mut dice = Dice(5);
        let (width, height) = (crate::instruments::WIDTH, crate::instruments::HEIGHT);
        for size in [[364, 360], [162, 160], [97, 95], [729, 720], [250, 400]] {
            let mut source = sprite(&mut dice, width, height).rgba;
            let mut kept: Option<Arc<PanelCache>> = None;
            let mut partial_frames = 0;
            for frame in 0..12 {
                // A few changed pixels (a number, a blink), sometimes a row,
                // once the whole page.
                let changes = if frame == 6 {
                    width * height
                } else {
                    frame % 4 * 3
                };
                for _ in 0..changes {
                    let at = (dice.next() % (width * height)) * 4;
                    source[at..at + 4].copy_from_slice(&[
                        (dice.next() % 256) as u8,
                        9,
                        (dice.next() % 256) as u8,
                        [0, 255, 128, 1][dice.next() % 4],
                    ]);
                }
                if frame == 9 {
                    let row = dice.next() % height;
                    source[row * width * 4..(row + 1) * width * 4].fill(200);
                }
                let next = scaled_panel(kept.as_ref(), None, size, source.clone());
                let whole = scaled_panel(None, None, size, source.clone());
                assert!(
                    next.image.rgba == whole.image.rgba,
                    "{size:?} frame {frame}: first difference {:?}",
                    next.image
                        .rgba
                        .iter()
                        .zip(&whole.image.rgba)
                        .position(|(a, b)| a != b)
                );
                assert_eq!(
                    next.clear_is_zero, whole.clear_is_zero,
                    "{size:?} frame {frame}"
                );
                if kept.as_ref().is_some_and(|kept| *kept.source != source) && changes < 100 {
                    partial_frames += 1;
                }
                kept = Some(next);
            }
            assert!(partial_frames > 0);
        }
    }

    #[test]
    fn a_whole_pixel_blit_over_a_clear_pixel_is_the_source_pixel() {
        // What lets a panel go onto a clear canvas as a copy.
        for alpha in 1..=255u8 {
            for value in 0..=255u8 {
                let mut destination = [0u8; 4];
                over_whole(&[value, 255 - value, value / 3, alpha], &mut destination);
                assert_eq!(destination, [value, 255 - value, value / 3, alpha]);
            }
        }
        let mut destination = [0u8; 4];
        over_whole(&[0, 0, 0, 0], &mut destination);
        assert_eq!(destination, [0; 4]);
    }

    #[test]
    fn composing_a_panel_matches_blitting_it_over_anything_beneath() {
        let mut dice = Dice(19);
        for size in [[640, 480], [300, 200]] {
            for clear_pixels in [false, true] {
                let mut image = sprite(&mut dice, 120, 90);
                if clear_pixels {
                    // Clear pixels with colour in them must not be copied.
                    for p in image.rgba.chunks_exact_mut(4).filter(|p| p[3] == 0) {
                        p[0] = 7;
                    }
                }
                let cache = Arc::new(PanelCache {
                    key: None,
                    source: Arc::new(Vec::new()),
                    size: [120, 90],
                    clear_is_zero: image.rgba.chunks_exact(4).all(|p| p[3] != 0 || p == [0; 4]),
                    image: Arc::new(image),
                });
                for (rect, mark) in [
                    ((10.2, 20.6, 120., 90.), None),
                    ((0., 0., 120., 90.), Some((5, 3))),
                    (
                        (size[0] as f64 - 120., size[1] as f64 - 90., 120., 90.),
                        Some((300, 1)),
                    ),
                    ((-4., 30., 120., 90.), None),
                ] {
                    let mut actual = FlightCanvas::default();
                    actual.blank(size);
                    if let Some((x, y)) = mark {
                        actual.blend(x.min(size[0] as i32 - 1), y + 25, [9, 9, 9], 0.5);
                    }
                    let mut expected = FlightCanvas::default();
                    expected.blank(size);
                    expected.pixels = actual.pixels.clone();
                    actual.compose_panel(1, rect, Arc::clone(&cache));
                    expected.blit(&cache.image, (rect.0.round(), rect.1.round(), 120., 90.));
                    assert!(
                        actual.pixels == expected.pixels,
                        "{size:?} {rect:?} {clear_pixels}"
                    );
                    actual.assert_drawn_rows_noted();
                }
            }
        }
    }

    #[test]
    fn a_frame_covered_whole_starts_from_the_same_clear_canvas() {
        let (airframe, state, panels) = benchmark::fixture(true);
        let mut with_panels = FlightCanvas::default();
        let mut without = FlightCanvas::default();
        for _ in 0..2 {
            with_panels.begin([960, 720], &airframe, &state, &panels);
            without.clear([960, 720]);
            // The map's letterbox.
            with_panels.fill_opaque([72, 72, 72]);
            without.fill_opaque([72, 72, 72]);
            assert!(with_panels.pixels == without.pixels);
            // The console's wipe.
            with_panels.begin([960, 720], &airframe, &state, &panels);
            with_panels.blank([960, 720]);
            without.clear([960, 720]);
            without.clear([960, 720]);
            assert!(with_panels.pixels == without.pixels);
        }
    }
}

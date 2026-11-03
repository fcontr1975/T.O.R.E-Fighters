//! Bounded worker and stable GPU pages. All candidate selection and packing
//! runs on the worker; the render thread only copies changed pages.
use super::{CacheFrame, INST_STRIDE, Library, Patch, Placement, Terrain, pack};
use crate::vegetation::{Atlas, Inst};
use crate::vegetation_pages::PageBounds;
use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
};

pub const PAGE: usize = 256;
// Two maximum 8 nm rings with a two-chunk guard fit within 5120 chunks.
// Each can waste one partial page.
// Two object budgets plus this slack hold the published and staged selections.
pub const SPARE_PAGES: usize = 5120;
pub const UPLOAD_BYTES: usize = 256 * 1024;
pub const UPLOAD_WRITES: usize = 16;
pub type Key = (i64, i64);
pub type Chunks = Vec<(Key, Arc<Vec<u8>>)>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    pub epoch: u64,
    pub eye: [f64; 2],
    pub ahead: [f64; 2],
    pub full: f32,
    pub draw: f32,
    pub density: f32,
    pub capacity: usize,
    pub grass_radius: f32,
    pub settle: bool,
}

pub struct Ready {
    pub epoch: u64,
    pub chunks: Chunks,
    pub grass: Vec<Patch>,
    pub squeeze: f32,
    pub reach: f32,
    pub available: f32,
    pub eye: [f64; 2],
    pub done: bool,
    pub cache: CacheFrame,
}

pub struct Worker {
    send: mpsc::SyncSender<Request>,
    receive: mpsc::Receiver<Ready>,
    pub busy: bool,
    last_request: Option<Request>,
    done: bool,
    /// Requests sent to the worker thread so far.
    pub sent: u64,
}
impl Worker {
    pub fn new(atlas: Arc<Atlas>, library: Arc<Library>, world: &Terrain) -> Self {
        // Only immutable placement inputs are retained. No live simulation state
        // is shared with or modified by the presentation worker.
        let world = Terrain {
            foundations: world.foundations.clone(),
            theater: tore_formats::theater::Theater {
                name: world.theater.name.clone(),
                map: world.theater.map.clone(),
                tiles: world.theater.tiles,
                cells_per_tile: world.theater.cells_per_tile,
                cols: world.theater.cols,
                rows: world.theater.rows,
                cells: world.theater.cells.clone(),
                coarse: world.theater.coarse.clone(),
            },
            layout: world.layout.clone(),
            condition: world.condition,
            environment: Default::default(),
            airport_scene: world.airport_scene.clone(),
            placement_objects: Vec::new(),
            placement_runways: Vec::new(),
            airfield_anchors: BTreeMap::new(),
            static_manifest: Vec::new(),
            catalog: Vec::new(),
            weather: world.weather.clone(),
            surface: Default::default(),
            city_fill: world.city_fill.clone(),
            redrawn: Vec::new(),
            redrawn_airports: world.redrawn_airports,
            redrawn_templates: world.redrawn_templates,
            merged_airports: world.merged_airports,
            neutral_airfield_ground: world.neutral_airfield_ground.clone(),
        };
        let (send, requests) = mpsc::sync_channel::<Request>(1);
        let (results, receive) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("vegetation".into())
            .spawn(move || {
                let mut placement = Placement::new(atlas);
                let mut epoch = u64::MAX;
                let mut selected_generation = u64::MAX;
                let mut chunks = Vec::new();
                let mut previous: BTreeMap<Key, (Vec<Inst>, Arc<Vec<u8>>)> = BTreeMap::new();
                let mut coverage = (1., 0., 0., [0.; 2]);
                let trace = std::env::var_os("TORE_VEG_STREAM_TRACE").is_some();
                while let Ok(request) = requests.recv() {
                    if request.epoch != epoch {
                        placement.reset_cache();
                        previous.clear();
                        chunks.clear();
                        coverage = (1., 0., 0., request.eye);
                        selected_generation = u64::MAX;
                        epoch = request.epoch;
                    }
                    placement.set_density(request.density);
                    placement.set_budget(request.capacity);
                    let before = (placement.builds, placement.grass_builds, placement.build_ns);
                    placement.prefetch(request.ahead);
                    let done = placement.update(
                        &world,
                        request.eye,
                        request.full,
                        request.draw,
                        if request.settle { None } else { Some(3) },
                    );
                    let (grass, grass_done) = placement.grass(
                        &world,
                        request.eye,
                        request.grass_radius,
                        if request.settle { None } else { Some(2) },
                    );
                    let mut packed = 0;
                    // A partial ring changes the global rank cutoff every few
                    // chunks. Publishing each partial selection reuploads the
                    // same resident chunks hundreds of times. Finish placement
                    // and density fitting before packing one coherent selection.
                    // Keep the previous selection's coverage while work continues.
                    if done && selected_generation != placement.generation {
                        let selected = placement.object_chunks(request.eye);
                        let mut next = BTreeMap::new();
                        chunks.clear();
                        for (key, objects) in selected {
                            let bytes = match previous.remove(&key) {
                                Some((old, bytes)) if old == objects => bytes,
                                _ => {
                                    packed += 1;
                                    if trace {
                                        eprintln!("vegetation pack epoch={epoch} chunk={key:?} objects={}", objects.len());
                                    }
                                    let mut bytes =
                                        Vec::with_capacity(objects.len() * INST_STRIDE as usize);
                                    for i in &objects {
                                        let mesh = library.mesh_for(i.species, i.variant);
                                        for value in
                                            pack(i, mesh, &library.meshes[usize::from(mesh)])
                                        {
                                            bytes.extend_from_slice(&value.to_le_bytes());
                                        }
                                    }
                                    Arc::new(bytes)
                                }
                            };
                            chunks.push((key, bytes.clone()));
                            next.insert(key, (objects, bytes));
                        }
                        previous = next;
                        selected_generation = placement.generation;
                    }
                    if done {
                        coverage = (placement.squeezed(1.), placement.kept_reach(request.eye), placement.available(request.eye), request.eye);
                    }
                    if trace {
                        eprintln!("vegetation worker epoch={epoch} eye={:?} objects_done={done} grass_done={grass_done} builds={} grass_builds={} packed={packed} squeeze={} available={}", request.eye, placement.builds-before.0, placement.grass_builds-before.1, coverage.0, coverage.2);
                    }
                    let ready = Ready {
                        epoch,
                        chunks: chunks.clone(),
                        grass,
                        squeeze: coverage.0,
                        reach: coverage.1,
                        available: coverage.2,
                        eye: coverage.3,
                        done: done && grass_done,
                        cache: CacheFrame {
                            object_chunks: placement.builds - before.0,
                            grass_chunks: placement.grass_builds - before.1,
                            build_ms: (placement.build_ns - before.2) as f64 / 1_000_000.,
                            pending: !done || !grass_done,
                            packed_chunks: packed,
                            submit_ms: 0.,
                            ..Default::default()
                        },
                    };
                    if results.send(ready).is_err() {
                        break;
                    }
                }
            })
            .expect("vegetation worker");
        Self {
            send,
            receive,
            busy: false,
            last_request: None,
            done: false,
            sent: 0,
        }
    }
    pub fn request(&mut self, request: Request) {
        if self.busy || self.done && self.last_request == Some(request) {
            return;
        }
        if self.send.try_send(request).is_ok() {
            self.sent += 1;
            self.busy = true;
            self.last_request = Some(request);
            self.done = false;
        }
    }
    pub fn poll(&mut self, wait: bool) -> Option<Ready> {
        if !self.busy {
            return None;
        }
        let ready = if wait {
            self.receive.recv().ok()
        } else {
            self.receive.try_recv().ok()
        };
        if let Some(ready) = &ready {
            self.busy = false;
            self.done = ready.done;
        }
        ready
    }
}

/// Pages an instance buffer holds for `capacity` objects and `spare` pages.
pub fn pages(capacity: usize, spare: usize) -> usize {
    2 * capacity.div_ceil(PAGE) + spare
}

/// One streamed circle of vegetation on the CPU side: its worker, the page
/// bookkeeping for its GPU instance buffer and the reach drawn from it. The
/// main view owns one; a camera panel far from the main eye owns a small one
/// (optimization S10). GPU writes go through the caller's closures, so the
/// same code runs in CPU tests.
pub struct Stream {
    worker: Option<Worker>,
    pub ready: Option<Ready>,
    incoming: Option<Ready>,
    pub pages: Slots,
    /// Objects and page-table entries published to the GPU.
    pub count: u32,
    pub page_count: u32,
    /// The published page table (first object, count pairs) and each
    /// page's cull bounds; each view uploads the pages it may draw from.
    pub table: Vec<u32>,
    pub bounds: Vec<PageBounds>,
    /// How far the drawn circle reaches, feet.
    pub visible_reach: f32,
    /// The eye of the last view that streamed.
    pub last_eye: Option<[f64; 2]>,
    last_prepare: std::time::Instant,
    /// Work done by the last `advance`.
    pub cache: CacheFrame,
}

/// What one `advance` asks of the worker, apart from the eye.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub epoch: u64,
    pub full: f32,
    pub draw: f32,
    pub density: f32,
    pub capacity: usize,
    pub grass_radius: f32,
    pub settle: bool,
}

impl Stream {
    pub fn new(capacity: usize, spare: usize) -> Self {
        Self {
            worker: None,
            ready: None,
            incoming: None,
            pages: Slots::with_spare(capacity, spare),
            count: 0,
            page_count: 0,
            table: Vec::new(),
            bounds: Vec::new(),
            visible_reach: 0.,
            last_eye: None,
            last_prepare: std::time::Instant::now(),
            cache: CacheFrame::default(),
        }
    }

    /// Forgets everything streamed (a new epoch follows); keeps the worker.
    pub fn reset(&mut self, capacity: usize, spare: usize) {
        self.ready = None;
        self.incoming = None;
        self.reset_pages(capacity, spare);
        self.cache = CacheFrame::default();
    }

    /// Requests this stream has sent to its worker.
    #[cfg(test)]
    pub fn worker_runs(&self) -> u64 {
        self.worker.as_ref().map_or(0, |w| w.sent)
    }

    /// New, empty GPU pages for `capacity` objects.
    pub fn reset_pages(&mut self, capacity: usize, spare: usize) {
        self.pages = Slots::with_spare(capacity, spare);
        self.page_count = 0;
        self.count = 0;
        self.table.clear();
        self.bounds.clear();
    }

    /// The request for a view at `eye`, with `ahead` taken from this
    /// stream's previous eye. Records `eye` as the stream's eye.
    pub fn request(&mut self, shape: Shape, eye: [f64; 2]) -> Request {
        let dt = self.last_prepare.elapsed().as_secs_f64().max(0.001);
        let ahead = self.last_eye.map_or([0.; 2], |last| {
            let delta = [eye[0] - last[0], eye[1] - last[1]];
            let distance = delta[0].hypot(delta[1]);
            // Five seconds at 480 kt is about 4,050 ft, within the guard ring.
            let scale = (5. / dt).min(4096. / distance.max(1.));
            let ahead = delta.map(|v| (v * scale / 512.).round() * 512.);
            let scale = 4096. / ahead[0].hypot(ahead[1]).max(4096.);
            ahead.map(|v| v * scale)
        });
        self.last_eye = Some(eye);
        Request {
            epoch: shape.epoch,
            eye,
            ahead,
            full: shape.full,
            draw: shape.draw,
            density: shape.density,
            capacity: shape.capacity,
            grass_radius: shape.grass_radius,
            settle: shape.settle,
        }
    }

    /// Takes the worker's finished result, uploads pages within the frame's
    /// budget (`write`), publishes a complete selection (its page table and
    /// bounds, which each view culls and uploads), asks the
    /// worker for `request` and moves the visible circle. `snap` sets the
    /// circle to everything available at once instead of growing it, as a
    /// small panel does.
    pub fn advance(
        &mut self,
        request: Request,
        spawn: impl FnOnce() -> Worker,
        snap: bool,
        mut write: impl FnMut(u64, &[u8]),
    ) {
        let settle = request.settle;
        self.cache = CacheFrame::default();
        if settle {
            self.pages.upload(usize::MAX, &mut write);
            if self.incoming.is_some() {
                self.ready = self.incoming.take();
            }
        }
        let worker = self.worker.get_or_insert_with(spawn);
        // Normal frames never wait. Captures explicitly request a settled view.
        if settle && worker.busy {
            let _ = worker.poll(true);
        }
        if settle {
            worker.request(request);
        }
        if let Some(ready) = worker.poll(settle)
            && ready.epoch == request.epoch
        {
            self.cache = ready.cache;
            self.pages.sync(&ready.chunks);
            self.incoming = Some(ready);
        }
        let mut uploaded = 0;
        self.pages.upload(
            if settle { usize::MAX } else { UPLOAD_BYTES },
            |offset, bytes| {
                write(offset, bytes);
                uploaded += bytes.len();
            },
        );
        self.cache.uploaded_bytes += uploaded;
        if !self.pages.pending() && (self.incoming.is_some() || settle) {
            if self.incoming.is_some() {
                self.ready = self.incoming.take();
            }
            let (entries, count) = self.pages.table();
            self.count = count;
            self.page_count = (entries.len() / 2) as u32;
            self.bounds = self.pages.table_bounds();
            debug_assert_eq!(self.bounds.len() * 2, entries.len());
            self.table = entries;
        }
        if !self.pages.pending() && !settle {
            worker.request(request);
        }
        self.cache.worker_pending = worker.busy || self.ready.as_ref().is_none_or(|r| !r.done);
        (self.cache.upload_chunks, self.cache.upload_bytes) = self.pages.remaining();
        self.cache.pending = self.pages.pending() || self.cache.worker_pending;
        // Uploaded coverage defines a circle, never individual patch visibility.
        // The outer fade stays wholly inside that circle, including during a
        // teleport or a worker delay. A new circle expands continuously.
        let eye = request.eye;
        let available = self
            .ready
            .as_ref()
            .map_or(0., |r| {
                (r.available - (r.eye[0] - eye[0]).hypot(r.eye[1] - eye[1]) as f32).max(0.)
            })
            .min(request.draw);
        let dt = self.last_prepare.elapsed().as_secs_f32().min(0.1);
        self.last_prepare = std::time::Instant::now();
        let (reach, range_pending) = super::advance_visible_range(
            self.visible_reach,
            available,
            request.draw,
            dt,
            settle || snap,
        );
        let reach = if available >= request.full {
            reach.max(request.full)
        } else {
            0.
        };
        self.visible_reach = reach;
        self.cache.available = available;
        self.cache.visible = reach;
        self.cache.range_pending = range_pending;
        self.cache.pending |= range_pending;
    }
}

#[derive(Clone)]
struct Resident {
    bytes: Arc<Vec<u8>>,
    pages: Vec<usize>,
    /// Each page's coarse cull bounds (round 2 tree page culling).
    bounds: Arc<Vec<PageBounds>>,
    uploaded: usize,
}

/// Fixed pages avoid fragmentation. A chunk retains its pages until its
/// selected candidates change or it leaves. The table hides incomplete chunks.
pub struct Slots {
    residents: BTreeMap<Key, Resident>,
    order: Vec<Key>,
    staged: Option<(BTreeMap<Key, Resident>, Vec<Key>)>,
    free: Vec<usize>,
}
impl Slots {
    pub fn new(capacity: usize) -> Self {
        Self::with_spare(capacity, SPARE_PAGES)
    }
    /// Pages for two selections of `capacity` objects plus `spare` pages for
    /// their partial pages (one per chunk in each selection).
    pub fn with_spare(capacity: usize, spare: usize) -> Self {
        Self {
            residents: BTreeMap::new(),
            order: Vec::new(),
            staged: None,
            free: (0..pages(capacity, spare)).rev().collect(),
        }
    }
    pub fn sync(&mut self, chunks: &Chunks) {
        assert!(self.staged.is_none());
        let mut next = BTreeMap::new();
        let mut order = Vec::new();
        for (key, bytes) in chunks {
            order.push(*key);
            let resident = if let Some(old) = self
                .residents
                .get(key)
                .filter(|r| Arc::ptr_eq(bytes, &r.bytes))
            {
                old.clone()
            } else {
                let count = bytes.len().div_ceil(PAGE * INST_STRIDE as usize);
                assert!(count <= self.free.len(), "vegetation page budget");
                let pages = (0..count).map(|_| self.free.pop().unwrap()).collect();
                Resident {
                    bytes: bytes.clone(),
                    pages,
                    bounds: Arc::new(PageBounds::pages(bytes, PAGE)),
                    uploaded: 0,
                }
            };
            next.insert(*key, resident);
        }
        self.staged = Some((next, order));
    }
    pub fn upload(&mut self, budget: usize, mut write: impl FnMut(u64, &[u8])) {
        let Some((next, order)) = &mut self.staged else {
            return;
        };
        let mut left = budget;
        let mut calls = if budget == usize::MAX {
            usize::MAX
        } else {
            UPLOAD_WRITES
        };
        for key in order.iter() {
            let resident = next.get_mut(key).unwrap();
            while resident.uploaded < resident.bytes.len() {
                let start = resident.uploaded;
                let end = (start + PAGE * INST_STRIDE as usize).min(resident.bytes.len());
                if end - start > left || calls == 0 {
                    return;
                }
                let page = resident.pages[start / (PAGE * INST_STRIDE as usize)];
                write(
                    (page * PAGE) as u64 * INST_STRIDE,
                    &resident.bytes[start..end],
                );
                left -= end - start;
                calls -= 1;
                resident.uploaded = end;
            }
        }
        // Publish the whole selection atomically. The previous view remains
        // drawable during uploads, including chunks whose selection changed.
        let (next, order) = self.staged.take().unwrap();
        for (key, resident) in &self.residents {
            if !next
                .get(key)
                .is_some_and(|r| Arc::ptr_eq(&r.bytes, &resident.bytes))
            {
                self.free.extend(resident.pages.iter().copied());
            }
        }
        self.residents = next;
        self.order = order;
    }
    pub fn pending(&self) -> bool {
        self.staged.is_some()
    }
    /// Unpublished chunks with bytes still to upload, and their total bytes.
    pub fn remaining(&self) -> (usize, usize) {
        self.staged.as_ref().map_or((0, 0), |(next, _)| {
            next.values().fold((0, 0), |(chunks, bytes), r| {
                let left = r.bytes.len() - r.uploaded;
                (chunks + usize::from(left > 0), bytes + left)
            })
        })
    }
    pub fn table(&self) -> (Vec<u32>, u32) {
        let mut table = Vec::new();
        let mut total = 0;
        for key in &self.order {
            let r = &self.residents[key];
            if r.uploaded != r.bytes.len() {
                continue;
            }
            let count = r.bytes.len() / INST_STRIDE as usize;
            total += count as u32;
            for (index, page) in r.pages.iter().enumerate() {
                table.extend([
                    (page * PAGE) as u32,
                    (count - index * PAGE).min(PAGE) as u32,
                ]);
            }
        }
        (table, total)
    }
    /// The cull bounds of each entry of [`Self::table`], in its order.
    pub fn table_bounds(&self) -> Vec<PageBounds> {
        let mut bounds = Vec::new();
        for key in &self.order {
            let r = &self.residents[key];
            if r.uploaded == r.bytes.len() {
                bounds.extend(r.bounds.iter().copied());
            }
        }
        bounds
    }
}

#[derive(Debug)]
pub struct ProbeStats {
    pub frames: usize,
    pub builds: u64,
    pub packed: u64,
    pub uploaded: usize,
    pub selected: u32,
}

/// CPU-only production worker/page-budget regression. Each simulated 60 Hz
/// frame waits for its bounded worker job, so this measures convergence and
/// upload churn, not scheduling speed or GPU performance.
pub fn probe(
    atlas: Arc<Atlas>,
    library: Arc<Library>,
    world: &Terrain,
    request: Request,
    out: &mut impl std::io::Write,
) -> crate::AppResult<ProbeStats> {
    let mut worker = Worker::new(atlas, library, world);
    let mut slots = Slots::new(request.capacity);
    let mut ready = None;
    let mut incoming = None;
    let mut visible = 0.;
    let mut idle = 0;
    let mut stats = ProbeStats {
        frames: 0,
        builds: 0,
        packed: 0,
        uploaded: 0,
        selected: 0,
    };
    write!(out, "{}", CacheFrame::CSV_HEADER)?;
    for frame in 0..1000 {
        let mut cache = CacheFrame::default();
        if !slots.pending() {
            worker.request(request);
            if let Some(result) = worker.poll(true) {
                cache = result.cache;
                slots.sync(&result.chunks);
                incoming = Some(result);
            }
        }
        slots.upload(UPLOAD_BYTES, |_, bytes| cache.uploaded_bytes += bytes.len());
        if !slots.pending() && incoming.is_some() {
            ready = incoming.take();
        }
        cache.worker_pending = worker.busy || ready.as_ref().is_none_or(|r| !r.done);
        (cache.upload_chunks, cache.upload_bytes) = slots.remaining();
        cache.available = ready.as_ref().map_or(0., |r| r.available).min(request.draw);
        (visible, cache.range_pending) =
            super::advance_visible_range(visible, cache.available, request.draw, 1. / 60., false);
        cache.visible = visible;
        cache.pending = cache.worker_pending || slots.pending() || cache.range_pending;
        cache.write_csv(out, frame)?;
        stats.builds += cache.object_chunks;
        stats.packed += cache.packed_chunks;
        stats.uploaded += cache.uploaded_bytes;
        if !cache.pending && cache.object_chunks == 0 && cache.grass_chunks == 0 {
            if idle == 0 {
                stats.frames = frame + 1;
            }
            idle += 1;
        } else if idle > 0 {
            return Err(
                format!("stationary vegetation resumed work at frame {frame}: {cache:?}").into(),
            );
        }
        if idle > 0 && (cache.packed_chunks > 0 || cache.uploaded_bytes > 0) {
            return Err(
                format!("stationary vegetation reuploaded at frame {frame}: {cache:?}").into(),
            );
        }
        if idle == 120 {
            stats.selected = slots.table().1;
            if stats.uploaded != stats.selected as usize * INST_STRIDE as usize {
                return Err(format!(
                    "stationary vegetation repacked intermediate selections: {stats:?}"
                )
                .into());
            }
            return Ok(stats);
        }
    }
    Err("CPU vegetation did not converge and stay idle for 120 frames within 1000 frames".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn library() -> Arc<Library> {
        Arc::new(Library {
            vertices: Vec::new(),
            meshes: vec![crate::vegetation_shapes::Mesh {
                range: 0..6,
                radius: 0.5,
                height_scale: 1.,
                look: crate::vegetation_shapes::Look::Foliage,
                recolour: None,
                source: "synthetic".into(),
                sprite_layer: 0,
                sprite_tile: None,
                top: [0., 0.8, 0.5, 0.5],
                crowns: Vec::new(),
                mean_color: [0.; 3],
            }],
            variants: vec![vec![0]; crate::vegetation::Species::ALL.len()],
        })
    }

    #[test]
    fn stationary_stream_converges_without_reuploads_and_stays_idle() {
        let mut world = tore_world::test_support::terrain();
        for cell in &mut world.theater.cells {
            cell.elevation = 0;
        }
        let atlas = Arc::new(Atlas::synthetic(vec![10; 256 * 256], 0));
        // Deliberately oversubscribed, so publishing partial rank selections
        // would repack and reupload resident chunks while this camera is still.
        let request = Request {
            epoch: 0,
            ahead: [0.; 2],
            eye: [4000., 4000.],
            full: 3000.,
            draw: 6000.,
            density: 0.7,
            capacity: 2048,
            grass_radius: 950.,
            settle: false,
        };
        let mut trace = Vec::new();
        let stats = probe(atlas, library(), &world, request, &mut trace).unwrap();
        assert!(stats.frames < 200, "{stats:?}");
        assert!(stats.builds > 3 && stats.packed > 0 && stats.selected > 0);
        assert_eq!(
            stats.uploaded,
            stats.selected as usize * INST_STRIDE as usize
        );
        assert!(stats.selected as usize <= request.capacity);
    }

    /// One view as the production renderer handles it (`Renderer::prepare`):
    /// route the view, then advance the stream it streams, if any. Settled
    /// requests wait for the worker, the kindest timing there is. Returns the
    /// route taken.
    fn draw_view(
        main: &mut Stream,
        panel: &mut Stream,
        kind: super::super::ViewKind,
        eye: [f64; 2],
        atlas: &Arc<Atlas>,
        world: &Terrain,
    ) -> super::super::Route {
        let shape = Shape {
            epoch: 0,
            full: 3000.,
            draw: 6000.,
            density: 0.7,
            capacity: 2048,
            grass_radius: 0.,
            settle: true,
        };
        let route = super::super::route(kind, eye, main.last_eye);
        let spawn = || Worker::new(atlas.clone(), library(), world);
        match route {
            super::super::Route::Stream => {
                let request = main.request(shape, eye);
                main.advance(request, spawn, false, |_, _| {});
            }
            super::super::Route::Share => {}
            super::super::Route::Panel(_) => {
                let (full, draw, capacity, _) =
                    super::super::panel_shape(shape.full, shape.draw, shape.capacity);
                let shape = Shape {
                    full,
                    draw,
                    capacity,
                    ..shape
                };
                let request = panel.request(shape, eye);
                panel.advance(request, spawn, true, |_, _| {});
            }
        }
        route
    }

    /// Optimization S0 found that every camera view (main, mirror, camera
    /// panels) asked one worker for its own eye: a mirror 12 ft off defeated
    /// the "same request, nothing to do" check on every view, and one far
    /// panel view threw the main view's chunks away. S10 routes the views:
    /// only the main view streams, the mirror and near panels read it, and a
    /// far panel streams its own circle. Extra views now cost the main
    /// stream nothing.
    #[test]
    fn extra_views_no_longer_redo_the_main_stream_work() {
        use super::super::{Route, ViewKind};
        let mut world = tore_world::test_support::terrain();
        for cell in &mut world.theater.cells {
            cell.elevation = 0;
        }
        let atlas = Arc::new(Atlas::synthetic(vec![10; 256 * 256], 0));
        let main_eye = [4000., 4000.];
        let mut main = Stream::new(2048, SPARE_PAGES);
        let mut panel = Stream::new(2048, SPARE_PAGES);
        let view = |main: &mut Stream, panel: &mut Stream, kind, eye| {
            draw_view(main, panel, kind, eye, &atlas, &world)
        };
        // The main view alone settles, then asks for nothing more.
        for _ in 0..300 {
            view(&mut main, &mut panel, ViewKind::Main, main_eye);
            if main.ready.as_ref().is_some_and(|r| r.done) {
                break;
            }
        }
        assert!(main.ready.as_ref().is_some_and(|r| r.done));
        let settled = main.worker_runs();
        let selected = main.count;
        let reach = main.visible_reach;
        assert!(selected > 0 && reach > 0.);
        view(&mut main, &mut panel, ViewKind::Main, main_eye);
        assert_eq!(
            main.worker_runs(),
            settled,
            "a settled still camera asks for nothing"
        );

        // Mirror, about 12 ft from the head, alternating with the main view:
        // it reads the main stream and sends nothing.
        let mirror = [4012., 4000.];
        for _ in 0..20 {
            assert_eq!(
                view(&mut main, &mut panel, ViewKind::Mirror, mirror),
                Route::Share
            );
            assert_eq!(
                view(&mut main, &mut panel, ViewKind::Main, main_eye),
                Route::Stream
            );
        }
        assert_eq!(
            main.worker_runs(),
            settled,
            "the mirror costs no worker run"
        );

        // A panel near the main eye (the front view) reads the main stream.
        assert_eq!(
            view(&mut main, &mut panel, ViewKind::Panel(2), [5000., 4000.]),
            Route::Share
        );

        // The target camera 100,000 ft away streams its own small circle;
        // the main stream keeps its chunks and its reach.
        let target = [104_000., 4000.];
        assert_eq!(
            view(&mut main, &mut panel, ViewKind::Panel(4), target),
            Route::Panel(4)
        );
        assert!(panel.worker_runs() > 0);
        assert!(panel.ready.as_ref().is_some_and(|r| r.eye == target));
        for _ in 0..10 {
            view(&mut main, &mut panel, ViewKind::Panel(4), target);
            view(&mut main, &mut panel, ViewKind::Main, main_eye);
        }
        assert_eq!(main.worker_runs(), settled, "the far panel moved nothing");
        assert_eq!(main.count, selected);
        assert_eq!(main.visible_reach, reach);
        assert!(
            main.ready
                .as_ref()
                .is_some_and(|r| r.eye == main_eye && r.done)
        );
        assert!(panel.visible_reach > 0. && panel.count > 0);
        assert!(panel.count <= super::super::panel_shape(3000., 6000., 2048).2 as u32);
    }

    #[test]
    fn views_route_to_the_stream_their_eye_needs() {
        use super::super::{PANEL_SHARE_FT, Route, ViewKind, route};
        let main = Some([0., 0.]);
        assert_eq!(route(ViewKind::Main, [9e9, 0.], main), Route::Stream);
        assert_eq!(route(ViewKind::Main, [0., 0.], None), Route::Stream);
        // The mirror never streams, wherever its eye is.
        assert_eq!(route(ViewKind::Mirror, [12., 0.], main), Route::Share);
        assert_eq!(route(ViewKind::Mirror, [0., 0.], None), Route::Share);
        // Panels: the main stream within its guard ring, their own beyond.
        assert_eq!(
            route(ViewKind::Panel(2), [PANEL_SHARE_FT, 0.], main),
            Route::Share
        );
        assert_eq!(
            route(ViewKind::Panel(4), [PANEL_SHARE_FT + 1., 0.], main),
            Route::Panel(4)
        );
        assert_eq!(route(ViewKind::Panel(3), [0., 0.], None), Route::Panel(3));
    }

    #[test]
    fn a_panel_stream_is_small_and_as_dense_as_the_main_one() {
        use crate::vegetation::{NM, ring_chunks};
        // The default settings: 4 nm drawn, 2 nm at full density, High.
        let (full, draw, capacity) = (2. * NM as f32, 4. * NM as f32, 131_072);
        let (p_full, p_draw, p_capacity, spare) = super::super::panel_shape(full, draw, capacity);
        assert_eq!((p_full, p_draw), (NM as f32, 2. * NM as f32));
        assert_eq!(p_capacity % PAGE, 0);
        // At least the main stream's objects per chunk over its own ring.
        let quota = capacity / ring_chunks(draw);
        assert!(p_capacity / ring_chunks(p_draw) >= quota);
        assert!(p_capacity < capacity / 2);
        // A selection's partial pages fit: one per chunk, two selections.
        assert!(spare >= 2 * ring_chunks(p_draw));
        // GPU instance memory: a small share of the main stream's.
        let main_bytes = pages(capacity, SPARE_PAGES) * PAGE * INST_STRIDE as usize;
        let panel_bytes = pages(p_capacity, spare) * PAGE * INST_STRIDE as usize;
        assert!(
            panel_bytes * 5 < main_bytes,
            "{panel_bytes} of {main_bytes}"
        );
        // Short settings: never more than the main view draws or holds.
        let (f, d, c, _) = super::super::panel_shape(1000., 3000., 32_768);
        assert_eq!((f, d), (1000., 3000.));
        assert!(c <= 32_768);
        // A panel stream at the smallest budget runs to a settled selection
        // within its pages (Slots panics if a selection outgrows them).
        let mut world = tore_world::test_support::terrain();
        for cell in &mut world.theater.cells {
            cell.elevation = 0;
        }
        let atlas = Arc::new(Atlas::synthetic(vec![10; 256 * 256], 0));
        let (full, draw, capacity, spare) = super::super::panel_shape(3000., 12_000., 4096);
        let mut panel = Stream::new(capacity, spare);
        let shape = Shape {
            epoch: 0,
            full,
            draw,
            density: 1.,
            capacity,
            grass_radius: 0.,
            settle: true,
        };
        for _ in 0..50 {
            let request = panel.request(shape, [20_000., 20_000.]);
            panel.advance(
                request,
                || Worker::new(atlas.clone(), library(), &world),
                true,
                |_, _| {},
            );
            assert_eq!(panel.bounds.len() * 2, panel.table.len());
            if panel.ready.as_ref().is_some_and(|r| r.done) {
                break;
            }
        }
        assert!(panel.ready.as_ref().is_some_and(|r| r.done));
        assert!(panel.count > 0 && panel.count as usize <= capacity);
        assert_eq!(panel.visible_reach, draw);
    }

    fn chunk(n: usize, value: u8) -> Arc<Vec<u8>> {
        Arc::new(vec![value; n * INST_STRIDE as usize])
    }
    #[test]
    fn insert_retire_reuse_without_repacking_existing_chunks() {
        let a = chunk(300, 1);
        let b = chunk(60, 2);
        let mut slots = Slots::new(1024);
        slots.sync(&vec![((0, 0), a.clone())]);
        let mut writes = Vec::new();
        slots.upload(usize::MAX, |offset, bytes| {
            writes.push((offset, bytes.to_vec()))
        });
        let old = slots.table();
        assert_eq!(old.1, 300);
        slots.sync(&vec![((0, 0), a.clone()), ((1, 0), b.clone())]);
        writes.clear();
        slots.upload(usize::MAX, |offset, bytes| {
            writes.push((offset, bytes.to_vec()))
        });
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].1, *b);
        assert_eq!(&slots.table().0[..old.0.len()], &old.0);
        let released = slots.residents[&(0, 0)].pages.clone();
        slots.sync(&vec![((1, 0), b.clone())]);
        slots.upload(usize::MAX, |_, _| {});
        slots.sync(&vec![((1, 0), b), ((2, 0), chunk(300, 3))]);
        slots.upload(usize::MAX, |_, _| {});
        assert!(
            slots.residents[&(2, 0)]
                .pages
                .iter()
                .all(|p| released.contains(p))
        );
    }
    #[test]
    fn tiny_chunks_are_bounded_by_upload_call_count_as_well_as_bytes() {
        let mut slots = Slots::new(1024);
        let chunks: Chunks = (0..40).map(|x| ((x, 0), chunk(1, x as u8))).collect();
        slots.sync(&chunks);
        let mut calls = 0;
        slots.upload(UPLOAD_BYTES, |_, _| calls += 1);
        assert_eq!(calls, UPLOAD_WRITES);
        assert!(slots.pending());
        assert_eq!(slots.table().1, 0);
        slots.upload(usize::MAX, |_, _| {});
        assert!(!slots.pending());
        assert_eq!(slots.table().1, 40);
    }

    #[test]
    fn replacement_keeps_previous_selection_until_all_pages_arrive() {
        let mut slots = Slots::new(1024);
        slots.sync(&vec![((0, 0), chunk(300, 1))]);
        slots.upload(usize::MAX, |_, _| {});
        let previous = slots.table();
        slots.sync(&vec![((0, 0), chunk(500, 2))]);
        let mut uploaded = 0;
        slots.upload(PAGE * INST_STRIDE as usize, |_, bytes| {
            uploaded += bytes.len()
        });
        assert_eq!(uploaded, PAGE * INST_STRIDE as usize);
        assert_eq!(slots.table(), previous);
        assert!(slots.pending());
        slots.upload(PAGE * INST_STRIDE as usize, |_, _| {});
        assert!(!slots.pending());
        assert_eq!(slots.table().1, 500);
        slots.sync(&Vec::new());
        slots.upload(0, |_, _| panic!("retirement must not upload"));
        assert_eq!(slots.table().1, 0);
    }

    #[test]
    fn budget_hides_partial_chunks_and_table_recovers_exact_selected_set() {
        let mut slots = Slots::new(1024);
        let chunks = vec![((0, 0), chunk(300, 1)), ((1, 0), chunk(20, 2))];
        slots.sync(&chunks);
        let mut memory = vec![0; (2048 + SPARE_PAGES * PAGE) * INST_STRIDE as usize];
        slots.upload(PAGE * INST_STRIDE as usize, |offset, bytes| {
            memory[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
        });
        assert!(slots.pending());
        assert_eq!(slots.table().1, 0);
        slots.upload(usize::MAX, |offset, bytes| {
            memory[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
        });
        let (table, count) = slots.table();
        assert_eq!(count, 320);
        let mut gathered = Vec::new();
        for page in table.chunks_exact(2) {
            let start = page[0] as usize * INST_STRIDE as usize;
            gathered
                .extend_from_slice(&memory[start..start + page[1] as usize * INST_STRIDE as usize]);
        }
        let expected: Vec<u8> = chunks.iter().flat_map(|(_, b)| b.iter().copied()).collect();
        assert_eq!(gathered, expected);
    }
}

//! Aircraft-forward cockpit and HUD with flat translation and directional fading.
use crate::gpu_setup::NamedSteps as _;
use crate::{
    attitude::{Basis, dot},
    camera::Camera,
    flight::State,
    menu::Sprite,
};

pub struct CockpitRenderer {
    pipeline: wgpu::RenderPipeline,
    bind: Option<wgpu::BindGroup>,
    hud: wgpu::Texture,
    /// What the HUD texture holds, so an update sends only the rows that
    /// changed (the HUD fills a fraction of its 640 by 480 layer).
    hud_held: Option<Vec<u8>>,
    palette: wgpu::Texture,
    /// The palette last uploaded, so an unchanged one is not sent again.
    palette_uploaded: Option<[[u8; 3]; 256]>,
    uniform: wgpu::Buffer,
    art_size: [u32; 2],
    enabled: bool,
    /// The canvas-pixel rectangle outside which the shader returns exact
    /// zero (`[x0, y0, x1, y1]`), and the canvas size, from the last
    /// `update`; `None` when neither the art nor the HUD is drawn.
    bounds: Option<([f32; 4], [u32; 2])>,
    pub mirror_target: wgpu::Texture,
    mirror_rects: [[f32; 4]; 3],
    pub mirrors_visible: bool,
}

fn texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Directional cockpit source"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}
fn upload(queue: &wgpu::Queue, texture: &wgpu::Texture, pixels: &[u8]) {
    queue.write_texture(
        texture.as_image_copy(),
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(texture.width() * 4),
            rows_per_image: Some(texture.height()),
        },
        texture.size(),
    );
}

/// Uploads the rows of `pixels` that differ from `held`, which is what the
/// texture holds (nothing known: the whole layer), and brings `held` up to
/// date.
fn upload_changed_rows(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    held: &mut Option<Vec<u8>>,
    pixels: &[u8],
) {
    let (width, height) = (texture.width() as usize, texture.height() as usize);
    let stride = width * 4;
    let Some(held_pixels) = held.as_mut().filter(|h| h.len() == pixels.len()) else {
        upload(queue, texture, pixels);
        *held = Some(pixels.to_vec());
        return;
    };
    let mut all = crate::canvas_rows::RowSet::default();
    all.reset_all(height);
    for band in crate::canvas_rows::changed_bands(
        pixels,
        held_pixels,
        stride,
        &all,
        crate::canvas_rows::GAP,
    ) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: band.start as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &pixels[band.start * stride..band.end * stride],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride as u32),
                rows_per_image: Some((band.end - band.start) as u32),
            },
            wgpu::Extent3d {
                width: width as u32,
                height: (band.end - band.start) as u32,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// Local camera axes expressed in aircraft coordinates, independent of aircraft attitude.
pub fn relative_axes(body: Basis, eye: Basis) -> [[f32; 3]; 3] {
    [eye.right, eye.up, eye.forward].map(|axis| {
        [
            dot(axis, body.right) as f32,
            dot(axis, body.up) as f32,
            dot(axis, body.forward) as f32,
        ]
    })
}
/// Fitted screen-space presentation. Bottom anchoring avoids a floating image edge;
/// the forward datum projects opposite head-look without clamping to the screen.
pub fn layout(size: [u32; 2], art: [u32; 2], yaw: f32, pitch: f32, zoom: f32) -> [f32; 4] {
    let [w, h] = size.map(|v| v as f32);
    let scale = ((w / art[0] as f32).max(h / art[1] as f32) * zoom).max(w / art[0] as f32);
    let opacity = ((65. - yaw.to_degrees().abs()) / 20.)
        .min((55. - pitch.to_degrees().abs()) / 20.)
        .clamp(0., 1.);
    // Project the aircraft-forward datum with the world camera focal length,
    // but translate the whole raster rather than tilting its rectangular plane.
    // Invisible rear/up views need no projection across the tangent singularity.
    if opacity == 0. {
        return [0., 0., scale, 0.];
    }
    let focal = h * 0.5 * 3f32.sqrt() * zoom;
    [
        -focal * yaw.tan() / pitch.cos(),
        focal * pitch.tan(),
        scale,
        opacity,
    ]
}
/// Zoom scales the HUD about its forward datum at screen center. Head-look
/// translates that datum with the camera, independently of magnification.
fn hud_layout(size: [u32; 2], translation: [f32; 2], zoom: f32) -> [f32; 4] {
    let [w, h] = size.map(|v| v as f32);
    let scale = (w / 640.).min(h / 480.) * crate::flight_canvas::HUD_SCALE as f32 * zoom;
    [
        640. * scale,
        480. * scale,
        w * 0.5 + translation[0],
        h * 0.5 + translation[1],
    ]
}
impl CockpitRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Directional cockpit"),
            source: wgpu::ShaderSource::Wgsl(include_str!("cockpit.wgsl").into()),
        });
        let pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Cockpit and HUD projection"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let hud = texture(device, 640, 480);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cockpit projection"),
            size: 128,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mirror_target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Live rear mirrors"),
            size: wgpu::Extent3d {
                width: crate::mirrors::SIZE[0],
                height: crate::mirrors::SIZE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Self {
            mirror_target,
            mirror_rects: [[0.; 4]; 3],
            mirrors_visible: false,
            pipeline,
            bind: None,
            hud,
            hud_held: Some(vec![0; 640 * 480 * 4]),
            palette: texture(device, 256, 1),
            palette_uploaded: None,
            uniform,
            art_size: [1, 1],
            enabled: false,
            bounds: None,
        }
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &Sprite,
        indexed: &tore_formats::Pic,
        id: tore_formats::aircraft::AircraftId,
    ) {
        // Called when an aircraft is prepared, including selection changes.
        // A previous binding belongs to the previous aircraft's cockpit.
        self.art_size = [source.width as u32, source.height as u32];
        let art = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Indexed original cockpit"),
            size: wgpu::Extent3d {
                width: self.art_size[0],
                height: self.art_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let pixels: Vec<u8> = indexed
            .pixels
            .iter()
            .zip(&indexed.mask)
            .flat_map(|(&index, &visible)| [index, u8::from(visible)])
            .collect();
        queue.write_texture(
            art.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.art_size[0] * 2),
                rows_per_image: Some(self.art_size[1]),
            },
            art.size(),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let masks = if std::env::var("TORE_MIRRORS").as_deref() == Ok("0") {
            None
        } else {
            crate::mirrors::masks(source, id)
        };
        self.mirror_rects = masks.as_ref().map_or([[0.; 4]; 3], |m| m.rects);
        println!(
            "Cockpit mirrors: {} reviewed regions, {}x{} rear feed, every other visible frame",
            self.mirror_rects
                .iter()
                .filter(|r| r[2] > 0. && r[3] > 0.)
                .count(),
            crate::mirrors::SIZE[0],
            crate::mirrors::SIZE[1]
        );
        let mask = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Source mirror silhouettes"),
            size: art.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mask_pixels = masks.map_or_else(|| vec![0; source.width * source.height], |m| m.pixels);
        queue.write_texture(
            mask.as_image_copy(),
            &mask_pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.art_size[0]),
                rows_per_image: Some(self.art_size[1]),
            },
            mask.size(),
        );
        let aperture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Source HUD glass aperture"),
            size: art.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let aperture_pixels = crate::hud_aperture::mask(source, id);
        debug_assert_eq!(aperture_pixels.len(), source.width * source.height);
        queue.write_texture(
            aperture.as_image_copy(),
            &aperture_pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.art_size[0]),
                rows_per_image: Some(self.art_size[1]),
            },
            aperture.size(),
        );
        self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Body-fixed cockpit art and HUD"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(
                        &self.palette.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &art.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &self.hud.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(
                        &mask.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(
                        &self.mirror_target.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(
                        &aperture.create_view(&Default::default()),
                    ),
                },
            ],
        }));
    }
    pub fn lighting(&self, queue: &wgpu::Queue, gain: f32, flood: f32, ambient: f32) {
        crate::sim_renderer::write_floats(
            queue,
            &self.uniform,
            112,
            &[gain - 1., flood, ambient - 1., 0.],
        );
    }
    /// Upload the neutral HUD-brightness cockpit art palette.
    pub fn weather(&mut self, queue: &wgpu::Queue, colors: &[[u8; 3]; 256]) {
        if self.palette_uploaded.as_ref() == Some(colors) {
            return;
        }
        self.palette_uploaded = Some(*colors);
        let pixels: Vec<u8> = colors
            .iter()
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect();
        upload(queue, &self.palette, &pixels);
    }
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        size: [u32; 2],
        state: &State,
        camera: &Camera,
        art: bool,
        hud: bool,
        pixels: &[u8],
    ) {
        let art = art && camera.zoom >= 1.;
        self.enabled = art || hud;
        self.mirrors_visible = false;
        if !self.enabled {
            return;
        }
        let axes = relative_axes(
            Basis::new(state.yaw, state.pitch, state.bank),
            Basis::new(camera.yaw as f64, camera.pitch as f64, -camera.roll as f64),
        );
        let yaw = axes[2][0].atan2(axes[2][2]);
        let pitch = axes[2][1].clamp(-1., 1.).asin();
        let placement = layout(size, self.art_size, yaw, pitch, camera.zoom);
        self.bounds = drawn_bounds(
            size,
            self.art_size,
            placement,
            hud_layout(size, [placement[0], placement[1]], camera.zoom),
            art,
            hud,
        )
        .map(|b| (b, size));
        self.mirrors_visible = art
            && placement[3] > 0.
            && self.mirror_rects.iter().any(|r| {
                let x = (size[0] as f32 - self.art_size[0] as f32 * placement[2]) * 0.5
                    + placement[0]
                    + r[0] * placement[2];
                let base = size[1] as f32 - self.art_size[1] as f32 * placement[2];
                let y = base.max(base * 0.5) + placement[1] + r[1] * placement[2];
                r[2] > 0.
                    && x < size[0] as f32
                    && x + r[2] * placement[2] > 0.
                    && y < size[1] as f32
                    && y + r[3] * placement[2] > 0.
            });
        let mut values = Vec::with_capacity(28);
        values.extend_from_slice(&placement);
        values.extend_from_slice(&[size[0] as f32, size[1] as f32, camera.zoom, 0.]);
        values.extend_from_slice(&[
            self.art_size[0] as f32,
            self.art_size[1] as f32,
            f32::from(art),
            f32::from(hud),
        ]);
        values.extend_from_slice(&hud_layout(size, [placement[0], placement[1]], camera.zoom));
        for rect in self.mirror_rects {
            values.extend_from_slice(&rect);
        }
        crate::sim_renderer::write_floats(queue, &self.uniform, 0, &values);
        if hud {
            upload_changed_rows(queue, &self.hud, &mut self.hud_held, pixels);
        }
    }
    /// Promo reel: the next draw emits only the visible HUD symbols, for a
    /// separate symbol layer. Submit the normal picture first, since queue
    /// writes precede submitted work; the next `update` restores it.
    pub fn symbols_only(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.uniform, 28, &1f32.to_le_bytes());
    }
    /// Draws into its own pass over `view`, a `target`-sized picture.
    pub fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target: [u32; 2],
    ) {
        if !self.enabled || self.bind.is_none() {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Directional cockpit and HUD"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        self.draw_in(&mut pass, target);
    }
    /// Draws into `pass`, whose target is `target` pixels with the default
    /// viewport, scissored to the art and HUD rectangles: outside them the
    /// shader returns exact zero, which the premultiplied blend leaves as is.
    /// The scissor is reset to the whole target afterwards.
    pub fn draw_in(&self, pass: &mut wgpu::RenderPass<'_>, target: [u32; 2]) {
        if !self.enabled {
            return;
        }
        let Some(bind) = &self.bind else {
            return;
        };
        let Some([x, y, w, h]) = self
            .bounds
            .and_then(|(bounds, size)| scissor(bounds, size, target))
        else {
            return;
        };
        pass.set_scissor_rect(x, y, w, h);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
        pass.set_scissor_rect(0, 0, target[0], target[1]);
    }
}
/// The union of the art and HUD rectangles in canvas pixels, as the shader
/// tests them, or `None` when neither is drawn. Outside it every output is 0.
fn drawn_bounds(
    size: [u32; 2],
    art_size: [u32; 2],
    placement: [f32; 4],
    hud_rect: [f32; 4],
    art: bool,
    hud: bool,
) -> Option<[f32; 4]> {
    let [w, h] = size.map(|v| v as f32);
    let [aw, ah] = art_size.map(|v| v as f32);
    let scale = placement[2];
    let mut rects = Vec::with_capacity(2);
    if art {
        let x = (w - aw * scale) * 0.5 + placement[0];
        let y = (h - ah * scale).max((h - ah * scale) * 0.5) + placement[1];
        rects.push([x, y, x + aw * scale, y + ah * scale]);
    }
    if hud {
        let [hw, hh, cx, cy] = hud_rect;
        rects.push([cx - hw * 0.5, cy - hh * 0.5, cx + hw * 0.5, cy + hh * 0.5]);
    }
    rects.into_iter().reduce(|a, b| {
        [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]
    })
}
/// Window pixels whose centres can map into `bounds` (canvas pixels of a
/// `size` canvas stretched over `target`), widened by two pixels for the
/// GPU's rounding: `[x, y, width, height]`, or `None` when empty. A
/// non-finite bound covers the whole target.
fn scissor(bounds: [f32; 4], size: [u32; 2], target: [u32; 2]) -> Option<[u32; 4]> {
    if bounds.iter().any(|v| !v.is_finite()) || size.contains(&0) {
        return Some([0, 0, target[0], target[1]]);
    }
    let axis = |lo: f32, hi: f32, canvas: u32, window: u32| {
        let k = f64::from(window) / f64::from(canvas);
        let (lo, hi) = (f64::from(lo.min(hi)), f64::from(lo.max(hi)));
        let start = ((lo * k - 0.5).floor() - 2.).clamp(0., f64::from(window)) as u32;
        let end = ((hi * k).ceil() + 2.).clamp(0., f64::from(window)) as u32;
        (start, end)
    };
    let (x0, x1) = axis(bounds[0], bounds[2], size[0], target[0]);
    let (y0, y1) = axis(bounds[1], bounds[3], size[1], target[1]);
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scissor_keeps_every_pixel_the_shader_draws() {
        // The shader's tests (cockpit.wgsl) at each window pixel centre.
        for (size, target) in [
            ([1920, 1080], [1920, 1080]),
            ([1920, 1080], [3840, 2160]),
            ([827, 1080], [1000, 1306]),
            ([640, 480], [641, 479]),
        ] {
            for (yaw, pitch, zoom, art, hud) in [
                (0., 0., 1., true, true),
                (0.4, -0.2, 1., true, true),
                (-0.7, 0.5, 2., true, false),
                (0.3, 0.1, 0.5, false, true),
                (0., 0., 4., false, true),
                (1.4, 0., 1., true, true),
            ] {
                let art_size = [1280, 490];
                let placement = layout(size, art_size, yaw, pitch, zoom);
                let hud_rect = hud_layout(size, [placement[0], placement[1]], zoom);
                let bounds = drawn_bounds(size, art_size, placement, hud_rect, art, hud).unwrap();
                let rect = scissor(bounds, size, target);
                let [w, h] = size.map(|v| v as f32);
                let [aw, ah] = art_size.map(|v| v as f32);
                let origin = [
                    (w - aw * placement[2]) * 0.5 + placement[0],
                    (h - ah * placement[2]).max((h - ah * placement[2]) * 0.5) + placement[1],
                ];
                for wy in 0..target[1] {
                    for wx in 0..target[0] {
                        let pixel = [
                            (wx as f32 + 0.5) / target[0] as f32 * w,
                            (wy as f32 + 0.5) / target[1] as f32 * h,
                        ];
                        let inside = |uv: [f32; 2]| uv.iter().all(|v| (0. ..=1.).contains(v));
                        let art_uv =
                            [0, 1].map(|k| (pixel[k] - origin[k]) / ([aw, ah][k] * placement[2]));
                        let hud_uv =
                            [0, 1].map(|k| 0.5 + (pixel[k] - hud_rect[k + 2]) / hud_rect[k]);
                        if !(art && inside(art_uv) || hud && inside(hud_uv)) {
                            continue;
                        }
                        let [x, y, rw, rh] = rect.expect("a drawn pixel needs a scissor");
                        assert!(
                            (x..x + rw).contains(&wx) && (y..y + rh).contains(&wy),
                            "{size:?} {target:?} {yaw} {pitch} {zoom}: ({wx}, {wy}) outside {rect:?}"
                        );
                    }
                }
            }
        }
        // Nothing on screen: no draw. Non-finite bounds: the whole target.
        assert_eq!(
            scissor([-500., -500., -10., -10.], [640, 480], [640, 480]),
            None
        );
        assert_eq!(
            scissor([f32::NAN, 0., 1., 1.], [640, 480], [800, 600]),
            Some([0, 0, 800, 600])
        );
        assert_eq!(
            drawn_bounds([640, 480], [1, 1], [0.; 4], [0.; 4], false, false),
            None
        );
    }
    #[test]
    fn hud_zoom_preserves_the_center_at_every_aspect_ratio() {
        for size in [[640, 480], [1920, 1080], [827, 1080]] {
            for zoom in [0.5, 0.75, 1., 2., 4.] {
                let [w, h, x, y] = hud_layout(size, [0., 0.], zoom);
                assert_eq!([x, y], size.map(|v| v as f32 * 0.5));
                let normal = hud_layout(size, [0., 0.], 1.);
                assert!((w - normal[0] * zoom).abs() < 0.001);
                assert!((h - normal[1] * zoom).abs() < 0.001);
            }
        }
        let panned = hud_layout([640, 480], [-100., 50.], 2.);
        assert_eq!([panned[2], panned[3]], [220., 290.]);
    }
    #[test]
    fn cockpit_and_hud_anchor_do_not_move_when_aircraft_attitude_changes() {
        for (yaw, pitch, bank) in [(0., 0., 0.), (2., 1.57, 1.), (4., 2., -2.)] {
            let body = Basis::new(yaw, pitch, bank);
            let axes = relative_axes(body, body);
            for (i, axis) in axes.iter().enumerate() {
                for (j, value) in axis.iter().enumerate() {
                    assert!((*value - f32::from(i == j)).abs() < 1e-6);
                }
            }
            let head = body.rotated(body.up.map(|v| v * 0.2));
            let axes = relative_axes(body, head);
            // The fixed forward datum moves left in a rightward head view.
            let x = axes[0][2] / axes[2][2];
            assert!((x + 0.2f32.tan()).abs() < 1e-6);
        }
    }
    #[test]
    fn forward_datum_moves_opposite_head_look_without_edge_clamping() {
        for size in [[1920, 1080], [827, 1080], [1920, 800]] {
            for zoom in [0.5, 1., 2., 4.] {
                let center = layout(size, [1280, 490], 0., 0., zoom);
                let right = layout(size, [1280, 490], 30f32.to_radians(), 0., zoom);
                let left = layout(size, [1280, 490], -30f32.to_radians(), 0., zoom);
                // At 30 degrees, the forward datum is half a viewport height
                // to the opposite side, scaled by zoom, even beyond art margins.
                assert!((right[0] + size[1] as f32 * 0.5 * zoom).abs() < 0.001);
                assert_eq!(left[0], -right[0]);
                assert_eq!(right[1], 0.);
                assert_eq!(right[2], center[2]);
                let farther = layout(size, [1280, 490], 40f32.to_radians(), 0., zoom);
                assert!(farther[0] < right[0]);
                let up = layout(size, [1280, 490], 0., 20f32.to_radians(), zoom);
                assert!(up[1] > 0.);
                assert_eq!(up[0], 0.);
                for (yaw, pitch) in [
                    (std::f32::consts::PI, 0.),
                    (0., std::f32::consts::FRAC_PI_2),
                ] {
                    let hidden = layout(size, [1280, 490], yaw, pitch, zoom);
                    assert_eq!(hidden[3], 0.);
                    assert!(hidden.iter().all(|v| v.is_finite()));
                }
            }
        }
    }
}

//! Minimal presenter for a 640x480 CPU canvas, used before any media exists.
//! Work package F owns this file; behaviour: docs/spec/first-run-import.md.
//!
//! The game renderer needs a terrain world, which a first run does not have
//! yet, so the locate screen gets its own blit-only path: one texture, one
//! sampler, one full-screen triangle, no world and no depth buffer. The
//! adapter and present-mode choices mirror `renderer.rs` so that a first run
//! exercises the same GPU selection a game run does, and the 4:3 canvas is
//! letterboxed through the same [`Viewport`] the menu uses.
use crate::gpu_setup::NamedSteps as _;
use crate::{
    AppResult,
    menu::{HEIGHT, WIDTH},
    renderer::{Viewport, WindowSurface},
};
use std::sync::Arc;
use winit::window::Window;

/// Full-screen triangle with the canvas sampled over it. Inline, because this
/// path has no other shader and nothing shares it.
const SHADER: &str = r"
@group(0) @binding(0) var canvas: texture_2d<f32>;
@group(0) @binding(1) var canvas_sampler: sampler;
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let points = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    let p = points[index];
    var result: VertexOutput;
    result.position = vec4(p, 0.0, 1.0);
    result.uv = vec2((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return result;
}
@fragment fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(canvas, canvas_sampler, input.uv);
}
";

pub(crate) struct CanvasPresenter {
    first_frame: crate::startup::FirstFrame,
    /// Presents in a row whose surface gave no image (see
    /// [`CanvasPresenter::surface_retry`]).
    surface_failures: u32,
    surface: WindowSurface,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    texture: wgpu::Texture,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    pub(crate) window: Arc<Window>,
}

impl CanvasPresenter {
    pub(crate) async fn new(window: Arc<Window>) -> AppResult<Self> {
        crate::diagnostics::stage("first-run graphics instance and surface");
        let instance = crate::graphics_api::instance();
        let surface = {
            let _step = crate::gpu_setup::begin("surface", "first-run window");
            instance.create_surface(window.clone())?
        };
        crate::diagnostics::stage_done();
        crate::diagnostics::stage("first-run graphics adapter selection");
        let adapter = {
            let _step = crate::gpu_setup::begin("adapter request", "first-run window");
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                })
                .await?
        };
        crate::diagnostics::stage_done();
        log::info!("first-run adapter: {:?}", adapter.get_info());
        crate::gpu_setup::note_adapter(&adapter.get_info());
        crate::diagnostics::stage("first-run graphics device creation");
        let (device, queue) = {
            let _step = crate::gpu_setup::begin("device request", "first-run window");
            adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await?
        };
        crate::diagnostics::stage_done();
        crate::diagnostics::reassert_crash_reporter();
        device.set_device_lost_callback(|reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                log::error!("first-run graphics device lost: {reason:?}: {message}");
            }
        });
        crate::diagnostics::stage("first-run graphics resources and pipelines");
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface has no configuration")?;
        let modes = surface.get_capabilities(&adapter).present_modes;
        config.present_mode = if modes.contains(&wgpu::PresentMode::Immediate) {
            wgpu::PresentMode::Immediate
        } else if modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        config.desired_maximum_frame_latency = 1;
        {
            let _step = crate::gpu_setup::begin("surface configuration", "first-run window");
            surface.configure(&device, &config);
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Locate canvas"),
            size: wgpu::Extent3d {
                width: WIDTH as u32,
                height: HEIGHT as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Locate compositor"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Locate"),
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
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let view = texture.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Locate canvas image"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        crate::diagnostics::stage_done();
        Ok(Self {
            first_frame: Default::default(),
            surface_failures: 0,
            surface: WindowSurface::new(surface),
            device,
            queue,
            config,
            texture,
            pipeline,
            bind_group,
            window,
        })
    }

    /// Where the 640x480 canvas sits inside the window, for pointer mapping.
    pub(crate) fn viewport(&self) -> Viewport {
        let size = self.window.inner_size();
        Viewport::new(size.width, size.height)
    }

    pub(crate) fn resize(&mut self) {
        if self.configure() {
            self.window.request_redraw();
        }
    }

    /// Configures the surface for the window's size; false at no size.
    fn configure(&mut self) -> bool {
        let size = self.window.inner_size();
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
            true
        } else {
            false
        }
    }

    /// After a present the surface gave no image for: how long to wait
    /// before trying again, as the game's renderer does
    /// ([`crate::frame_pacing::surface_retry`]); None after a good one.
    pub(crate) fn surface_retry(&self) -> Option<std::time::Duration> {
        (self.surface_failures > 0)
            .then(|| crate::frame_pacing::surface_retry(self.surface_failures))
    }

    /// Upload one 640x480 RGBA frame and show it, letterboxed.
    pub(crate) fn present(&mut self, pixels: &[u8]) -> AppResult<bool> {
        self.first_frame.begin("first-run first frame presentation");
        if pixels.len() != WIDTH * HEIGHT * 4 {
            return Err("the locate canvas must be 640x480 RGBA".into());
        }
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(false);
        }
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => {
                self.surface_failures = 0;
                frame
            }
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.configure();
                self.surface_failures = self.surface_failures.saturating_add(1);
                return Ok(false);
            }
            Err(wgpu::SurfaceError::Timeout | wgpu::SurfaceError::Other) => {
                self.surface_failures = self.surface_failures.saturating_add(1);
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH as u32 * 4),
                rows_per_image: Some(HEIGHT as u32),
            },
            wgpu::Extent3d {
                width: WIDTH as u32,
                height: HEIGHT as u32,
                depth_or_array_layers: 1,
            },
        );
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Locate screen"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let v = self.viewport();
            pass.set_viewport(v.x, v.y, v.width, v.height, 0., 1.);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        // Same Wayland pacing rule as the game renderer: only ask for frame
        // callbacks when the surface has fallen back to FIFO.
        if self.config.present_mode == wgpu::PresentMode::Fifo {
            self.window.pre_present_notify();
        }
        frame.present();
        self.first_frame.complete();
        Ok(true)
    }
}

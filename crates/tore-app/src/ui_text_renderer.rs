//! The GPU side of `ui_text`: draws the recorded glyph rectangles over the
//! menu canvas, in the same render pass, at the window's resolution.
use crate::gpu_setup::NamedSteps as _;
use crate::ui_text::{Quad, atlas};

/// Mipmap levels uploaded: glyphs are drawn between about 1:1 and a quarter of
/// the atlas's size, and the atlas keeps four pixels clear round each glyph,
/// one at the third level.
const LEVELS: u32 = 3;
/// Floats in one instance: destination, atlas rectangle, clip, colour.
const INSTANCE_FLOATS: usize = 16;

pub(crate) struct UiTextRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: usize,
    count: u32,
    /// Whether the target converts linear colour to sRGB, so colours go in
    /// linear.
    linear: bool,
    /// The frame-rate readout, in window pixels. Its own buffer, so writing
    /// it does not replace the menu glyphs already recorded for this frame.
    fps_pipeline: wgpu::RenderPipeline,
    fps_bind: wgpu::BindGroup,
    fps_instances: wgpu::Buffer,
    fps_capacity: usize,
    fps_size: wgpu::Buffer,
}

impl UiTextRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        let atlas = atlas();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("UI text atlas"),
            size: wgpu::Extent3d {
                width: atlas.width as u32,
                height: atlas.height as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let (mut width, mut height) = (atlas.width, atlas.height);
        let mut level = atlas.plane.to_vec();
        for mip in 0..LEVELS {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &level,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width as u32),
                    rows_per_image: Some(height as u32),
                },
                wgpu::Extent3d {
                    width: width as u32,
                    height: height as u32,
                    depth_or_array_layers: 1,
                },
            );
            // The next level: each pixel the mean of a 2 by 2 block.
            let (next_w, next_h) = ((width / 2).max(1), (height / 2).max(1));
            let mut next = vec![0u8; next_w * next_h];
            for y in 0..next_h {
                for x in 0..next_w {
                    let at = |dx: usize, dy: usize| {
                        u32::from(
                            level[((y * 2 + dy).min(height - 1)) * width
                                + (x * 2 + dx).min(width - 1)],
                        )
                    };
                    next[y * next_w + x] =
                        ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8;
                }
            }
            (width, height, level) = (next_w, next_h, next);
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UI text"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ui_text.wgsl").into()),
        });
        let pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("UI text"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: (INSTANCE_FLOATS * 4) as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4,
                        1 => Float32x4,
                        2 => Float32x4,
                        3 => Float32x4,
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
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
            label: Some("UI text atlas"),
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
        let capacity = 1024;
        let fps_shader = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Frame rate"),
            source: wgpu::ShaderSource::Wgsl(include_str!("frame_rate.wgsl").into()),
        });
        let fps_pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Frame rate"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &fps_shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: (INSTANCE_FLOATS * 4) as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4,
                        1 => Float32x4,
                        2 => Float32x4,
                        3 => Float32x4,
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &fps_shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
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
        let fps_size = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Frame rate size"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fps_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Frame rate atlas"),
            layout: &fps_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: fps_size.as_entire_binding(),
                },
            ],
        });
        Self {
            pipeline,
            bind_group,
            instances: Self::buffer(device, capacity),
            capacity,
            count: 0,
            linear: format.is_srgb(),
            fps_pipeline,
            fps_bind,
            fps_instances: Self::buffer(device, 64),
            fps_capacity: 64,
            fps_size,
        }
    }

    fn buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("UI text glyphs"),
            size: (capacity * INSTANCE_FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Whether colours go to the shader in linear light.
    pub(crate) fn linear(&self) -> bool {
        self.linear
    }

    /// The glyphs to draw until the next call.
    pub(crate) fn set(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, quads: &[Quad]) {
        if quads.len() > self.capacity {
            self.capacity = quads.len().next_power_of_two();
            self.instances = Self::buffer(device, self.capacity);
        }
        let mut bytes = Vec::with_capacity(quads.len() * INSTANCE_FLOATS * 4);
        for quad in quads {
            for value in quad
                .dst
                .iter()
                .chain(&quad.uv)
                .chain(&quad.clip)
                .chain(&quad.color)
            {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.instances, 0, &bytes);
        }
        self.count = quads.len() as u32;
    }

    /// Draws the glyphs into `pass`, whose viewport must be the canvas's.
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..6, 0..self.count);
    }

    /// Uploads `quads` in window pixels for [`draw_window_in`], returning
    /// the glyph count, or `None` when there is nothing to draw. A separate
    /// buffer from [`set`], so the menu text of this frame is left as it is.
    pub(crate) fn prepare_window(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quads: &[Quad],
        size: [f32; 2],
    ) -> Option<u32> {
        if quads.is_empty() || size[0] < 1.0 || size[1] < 1.0 {
            return None;
        }
        if quads.len() > self.fps_capacity {
            self.fps_capacity = quads.len().next_power_of_two();
            self.fps_instances = Self::buffer(device, self.fps_capacity);
        }
        let mut bytes = Vec::with_capacity(quads.len() * INSTANCE_FLOATS * 4);
        for quad in quads {
            for value in quad
                .dst
                .iter()
                .chain(&quad.uv)
                .chain(&quad.clip)
                .chain(&quad.color)
            {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        queue.write_buffer(&self.fps_instances, 0, &bytes);
        let mut size_bytes = [0u8; 16];
        size_bytes[..4].copy_from_slice(&size[0].to_le_bytes());
        size_bytes[4..8].copy_from_slice(&size[1].to_le_bytes());
        queue.write_buffer(&self.fps_size, 0, &size_bytes);
        Some(quads.len() as u32)
    }

    /// Draws the `count` glyphs of [`prepare_window`] into `pass`, whose
    /// viewport must be the whole window.
    pub(crate) fn draw_window_in(&self, pass: &mut wgpu::RenderPass<'_>, count: u32) {
        pass.set_pipeline(&self.fps_pipeline);
        pass.set_bind_group(0, &self.fps_bind, &[]);
        pass.set_vertex_buffer(0, self.fps_instances.slice(..));
        pass.draw(0..6, 0..count);
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::menu::Canvas;
    use crate::ui_text::{self, composite, srgb_to_linear};
    use crate::widgets::test_kit::{blank, kit};

    /// Draws recorded text through the real pipeline into an offscreen sRGB
    /// target and compares it with the CPU stand-in `ui_text::composite`
    /// (which the review pictures use). Needs a GPU adapter, so it is not run
    /// by default:
    ///
    /// ```text
    /// cargo test -p tore-app --locked ui_text_renderer -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a GPU adapter"]
    fn the_gpu_draws_what_the_cpu_stand_in_does() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("no GPU adapter, nothing to compare");
            return;
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("a device");
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let (scale, width, height) = (3u32, 1920u32, 1440u32);

        let kit = kit();
        let font = kit.sprite("SMLFONT");
        let mut canvas = blank();
        for px in canvas.chunks_exact_mut(4) {
            px.copy_from_slice(&[90, 90, 90, 255]);
        }
        ui_text::begin();
        let mut drawing = Canvas(&mut canvas);
        let text = ui_text::text;
        text(
            &mut drawing,
            &kit,
            font,
            "Direct Network Connection 0123",
            (30, 40),
            None,
            None,
        );
        text(
            &mut drawing,
            &kit,
            font,
            "Clipped at the box edge",
            (30, 60),
            Some((30, 60, 70, 12)),
            None,
        );
        text(
            &mut drawing,
            &kit,
            font,
            "Hidden under a panel",
            (30, 80),
            None,
            None,
        );
        ui_text::occlude((50, 78, 40, 16));
        text(
            &mut drawing,
            &kit,
            font,
            "Over the panel",
            (60, 80),
            None,
            Some([255, 120, 120]),
        );
        // Icons go through the same pipeline: three in a row, the middle one
        // half hidden by a panel drawn after it.
        let icon = crate::widgets::Icon::Crown;
        icon.draw(&mut drawing, (30, 110), None);
        crate::widgets::Icon::MacOs.draw(&mut drawing, (50, 110), None);
        crate::widgets::Icon::Linux.draw(&mut drawing, (70, 110), Some([118; 3]));
        ui_text::occlude((50, 110, 6, 12));
        let layer = ui_text::finish().expect("a layer");

        let mut renderer = UiTextRenderer::new(&device, &queue, format);
        assert!(renderer.linear(), "an sRGB target takes linear colours");
        renderer.set(&device, &queue, &layer.quads(renderer.linear()));
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let grey = f64::from(srgb_to_linear(90));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: grey,
                            g: grey,
                            b: grey,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);
            renderer.draw(&mut pass);
        }
        let stride = (width * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test readback"),
            size: u64::from(stride * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .expect("poll");
        rx.recv().unwrap().expect("mapped");
        let gpu: Vec<u8> = {
            let mapped = buffer.slice(..).get_mapped_range();
            mapped
                .chunks_exact(stride as usize)
                .flat_map(|row| row[..width as usize * 4].to_vec())
                .collect()
        };
        let cpu = composite(&canvas, &layer, scale as usize);

        if let Some(out) = std::env::var_os("TORE_MOCK_OUT") {
            for (name, pixels) in [("gpu", &gpu), ("cpu", &cpu)] {
                let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
                for px in pixels.chunks_exact(4) {
                    ppm.extend_from_slice(&px[..3]);
                }
                std::fs::write(
                    std::path::Path::new(&out).join(format!("text-{name}.ppm")),
                    ppm,
                )
                .unwrap();
            }
        }
        let (mut total, mut worst, mut inked, mut mismatched) = (0u64, 0u8, 0u64, 0u64);
        for (g, c) in gpu.chunks_exact(4).zip(cpu.chunks_exact(4)) {
            if g[..3] != [90; 3] || c[..3] != [90; 3] {
                inked += 1;
                let diff = (0..3).map(|i| g[i].abs_diff(c[i])).max().unwrap();
                total += u64::from(diff);
                worst = worst.max(diff);
                mismatched += u64::from(diff > 24);
            }
        }
        eprintln!(
            "{inked} pixels differ from the background; mean difference {:.2}, worst {worst}, {mismatched} over 24",
            total as f64 / inked.max(1) as f64
        );
        assert!(inked > 2000, "text was drawn: {inked}");
        // The sampler's edges and the stand-in's differ a little; the layout,
        // clip and cut-out must agree.
        assert!(
            total as f64 / (inked as f64) < 16.0,
            "the GPU and the stand-in agree"
        );
        assert!(
            (mismatched as f64) < inked as f64 * 0.2,
            "few pixels are far off"
        );
        // The clipped line (box x 30 to 100 on the canvas, 90 to 300 here)
        // shows nothing past its box.
        let at = |x: u32, y: u32| gpu[((y * width + x) * 4) as usize];
        assert!(
            (304..560).all(|x| (180..216).all(|y| at(x, y) == 90)),
            "clip held"
        );
    }
}

//! Static texture upload plus the grass draw: a candidate pass (compute)
//! evaluates each candidate blade once and an indirect draw shapes the
//! grown ones; without compute, every candidate is drawn and each vertex
//! evaluates its blade (the original path, `TORE_GRASS_PATH=vertex`).
use super::{Map, PAGE, VERTICES, candidates, grown_bound, root_lift, side, spacing};
use crate::gpu_setup::NamedSteps as _;
use crate::{graphics::VegDensity, terrain::Terrain};

pub struct Blades {
    layout: wgpu::BindGroupLayout,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
    /// The candidate pass, when the device can run it.
    cull: Option<Cull>,
    /// Draw with the vertex path even where the candidate pass runs
    /// (`TORE_GRASS_PATH=vertex`, a comparison aid).
    vertex_only: bool,
    area: [f32; 4],
    pages_x: u32,
    count: u32,
    /// Ground elevation per terrain vertex, as the shader reads it, for the
    /// altitude bound in [`Blades::prepare`].
    heights: Vec<f32>,
    terrain_size: [u32; 2],
}

/// Bytes per grown blade in the candidate pass's list: four vec4.
const BLADE_BYTES: u64 = 64;
/// Candidate pass threads per workgroup.
const GROUP: u32 = 64;

/// The candidate pass and its indirect draw.
struct Cull {
    pipeline: wgpu::ComputePipeline,
    draw: wgpu::RenderPipeline,
    /// Group 1 is not read by the pass; an empty group fills its slot.
    empty: wgpu::BindGroup,
    out_layout: wgpu::BindGroupLayout,
    /// The indirect draw (five vertices, the grown count), then the list
    /// slots the pass handed out.
    args: wgpu::Buffer,
    /// The grown blades, its bindings and capacity in blades.
    list: Option<(wgpu::Buffer, wgpu::BindGroup, u32)>,
    /// This view uses the pass (its candidates fit the device limits).
    active: bool,
}

/// A grown-blade list for `capacity` blades and its pass bindings.
fn grown_list(
    device: &wgpu::Device,
    cull: &Cull,
    capacity: u32,
) -> (wgpu::Buffer, wgpu::BindGroup, u32) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Blade grass grown blades"),
        size: u64::from(capacity) * BLADE_BYTES,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Blade grass candidate pass output"),
        layout: &cull.out_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: cull.args.as_entire_binding(),
            },
        ],
    });
    (buffer, bind, capacity)
}

/// Whether the device runs the candidate pass: compute with a 64-thread
/// workgroup and two storage buffers per stage. WebGPU guarantees these;
/// downlevel (WebGL2-class) adapters report zero and keep the vertex path.
/// The city fill and tree culls already need compute and indirect draws.
fn cull_supported(limits: &wgpu::Limits) -> bool {
    limits.max_compute_workgroup_size_x >= GROUP
        && limits.max_compute_invocations_per_workgroup >= GROUP
        && limits.max_storage_buffers_per_shader_stage >= 2
        && limits.max_compute_workgroups_per_dimension > 0
}

/// Whether `candidates` fit one dispatch and one list binding.
fn cull_fits(limits: &wgpu::Limits, candidates: u32) -> bool {
    let bytes = u64::from(candidates) * BLADE_BYTES;
    candidates.div_ceil(GROUP) <= limits.max_compute_workgroups_per_dimension
        && bytes <= u64::from(limits.max_storage_buffer_binding_size)
        && bytes <= limits.max_buffer_size
}

impl Blades {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        samples: u32,
        material: &wgpu::BindGroupLayout,
        lighting: &wgpu::BindGroupLayout,
        map: &Map,
    ) -> Self {
        let limits = device.limits();
        let texture_entry = |binding, dimension, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: dimension,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blade grass resources"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(64),
                    },
                    count: None,
                },
                texture_entry(
                    1,
                    wgpu::TextureViewDimension::D2Array,
                    wgpu::TextureSampleType::Uint,
                ),
                texture_entry(
                    2,
                    wgpu::TextureViewDimension::D2,
                    wgpu::TextureSampleType::Float { filterable: false },
                ),
            ],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Blade grass uniforms"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let upload = |label, size: wgpu::Extent3d, format, bytes: &[u8], stride| {
            crate::vegetation_limits::texture(label, size, &limits).expect("grass texture limits");
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.width * stride),
                    rows_per_image: Some(size.height),
                },
                size,
            );
            texture
        };
        let empty_mask = vec![0; PAGE * PAGE * 4];
        let mask = if map.mask.is_empty() {
            &empty_mask
        } else {
            &map.mask
        };
        let packing = crate::vegetation_limits::Pages::new(mask.len() / (PAGE * PAGE * 4), &limits)
            .expect("grass habitat layout");
        let habitat = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Grass habitat, packed 128 ft classifications"),
            size: packing.size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // Upload logical pages directly into their atlas slots. No second
        // theater-sized CPU allocation, and the cached bytes stay unchanged.
        for (page, bytes) in mask.chunks_exact(PAGE * PAGE * 4).enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &habitat,
                    mip_level: 0,
                    origin: packing.origin(page as u32),
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(PAGE as u32 * 4),
                    rows_per_image: Some(PAGE as u32),
                },
                wgpu::Extent3d {
                    width: PAGE as u32,
                    height: PAGE as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
        let habitat = habitat.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let ground = upload(
            "Grass terrain vertices and picture lookup",
            wgpu::Extent3d {
                width: map.terrain_size[0].max(1),
                height: map.terrain_size[1].max(1),
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba32Float,
            &crate::vegetation_renderer::bytes_of_f32(if map.terrain.is_empty() {
                &[0.; 4]
            } else {
                &map.terrain
            }),
            16,
        )
        .create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blade grass"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&habitat),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&ground),
                },
            ],
        });
        let vertex_pipeline = pipeline(
            device, shader, format, samples, material, lighting, &layout, false,
        );
        let cull = cull_supported(&limits).then(|| {
            let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Blade grass candidate pass, unused group"),
                entries: &[],
            });
            let empty = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Blade grass candidate pass, unused group"),
                layout: &empty_layout,
                entries: &[],
            });
            let storage = |binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            };
            let out_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Blade grass candidate pass output"),
                entries: &[storage(0), storage(1)],
            });
            let compute_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Blade grass candidate pass"),
                bind_group_layouts: &[material, &empty_layout, &layout, &out_layout],
                push_constant_ranges: &[],
            });
            let compute = device.named_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Blade grass candidates"),
                layout: Some(&compute_layout),
                module: shader,
                entry_point: Some("blade_cull"),
                compilation_options: Default::default(),
                cache: None,
            });
            use wgpu::util::DeviceExt;
            let args = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Blade grass indirect draw"),
                contents: &[VERTICES, 0, 0, 0, 0]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<u8>>(),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            });
            Cull {
                pipeline: compute,
                draw: pipeline(
                    device, shader, format, samples, material, lighting, &layout, true,
                ),
                empty,
                out_layout,
                args,
                list: None,
                active: false,
            }
        });
        Self {
            layout,
            bind,
            uniform,
            pipeline: vertex_pipeline,
            cull,
            vertex_only: std::env::var("TORE_GRASS_PATH").as_deref() == Ok("vertex"),
            area: map.area,
            pages_x: map.pages_x,
            count: 0,
            heights: map.terrain.chunks_exact(4).map(|v| v[0]).collect(),
            terrain_size: map.terrain_size,
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn rebuild(
        &mut self,
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        samples: u32,
        material: &wgpu::BindGroupLayout,
        lighting: &wgpu::BindGroupLayout,
    ) {
        self.pipeline = pipeline(
            device,
            shader,
            format,
            samples,
            material,
            lighting,
            &self.layout,
            false,
        );
        if let Some(cull) = &mut self.cull {
            cull.draw = pipeline(
                device,
                shader,
                format,
                samples,
                material,
                lighting,
                &self.layout,
                true,
            );
        }
    }
    /// Fixed-size stack uniform. No placement, chunk enumeration, packing or
    /// blade buffer upload is allowed here, including while the camera moves.
    /// `eye` is this view's camera position in world feet.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &Terrain,
        density: VegDensity,
        radius: f32,
        eye: [f64; 3],
    ) {
        let wind = world.wind();
        self.count = if self.pages_x > 0
            && radius > 0.
            && !above_blades(&self.heights, self.terrain_size, eye, radius)
        {
            candidates(density, radius)
        } else {
            0
        };
        // The grown list holds the most blades this eye's circles can grow,
        // with an eighth spare so small moves do not reallocate it.
        let count = self.count;
        let mut capacity = 0;
        if let Some(cull) = &mut self.cull {
            cull.active = count > 0 && !self.vertex_only && cull_fits(&device.limits(), count);
            if cull.active {
                let bound = grown_bound(density, radius, [eye[0], eye[2]]);
                if cull.list.as_ref().is_none_or(|l| l.2 < bound) {
                    let size = (bound + bound / 8).clamp(1, count);
                    cull.list = Some(grown_list(device, cull, size));
                }
                capacity = cull.list.as_ref().map_or(0, |l| l.2);
            }
        }
        let values = [
            self.area[0],
            self.area[1],
            self.area[2],
            self.area[3],
            side(density, radius) as f32,
            radius,
            self.pages_x as f32,
            root_lift(),
            (world.weather.ticks() as f64 / 120.) as f32,
            wind[0] as f32,
            wind[2] as f32,
            spacing(density),
            self.count as f32,
            capacity as f32,
            0.,
            0.,
        ];
        let mut bytes = [0; 64];
        for (i, v) in values.into_iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        queue.write_buffer(&self.uniform, 0, &bytes);
    }
    /// Records the candidate pass for the prepared view (before its world
    /// pass): `material` is the view's world bind group, as the draw uses.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, material: &wgpu::BindGroup) {
        let Some(cull) = self.cull.as_ref().filter(|c| c.active) else {
            return;
        };
        let Some((_, out, _)) = &cull.list else {
            return;
        };
        encoder.clear_buffer(&cull.args, 4, Some(16));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Blade grass candidates"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&cull.pipeline);
        pass.set_bind_group(0, material, &[]);
        pass.set_bind_group(1, &cull.empty, &[]);
        pass.set_bind_group(2, &self.bind, &[]);
        pass.set_bind_group(3, out, &[]);
        pass.dispatch_workgroups(self.count.div_ceil(GROUP), 1, 1);
    }
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.count == 0 {
            return;
        }
        pass.set_bind_group(2, &self.bind, &[]);
        if let Some(cull) = self.cull.as_ref().filter(|c| c.active)
            && let Some((list, _, _)) = &cull.list
        {
            pass.set_pipeline(&cull.draw);
            pass.set_vertex_buffer(0, list.slice(..));
            pass.draw_indirect(&cull.args, 0);
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.draw(0..VERTICES, 0..self.count);
    }
    /// Draws with the vertex path only (a comparison aid).
    #[cfg(test)]
    pub fn set_vertex_only(&mut self, on: bool) {
        self.vertex_only = on;
    }
    /// The blades the candidate pass grew in the last view, or none when
    /// it did not run (waits for the GPU; a test aid).
    #[cfg(test)]
    pub fn grown(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<u32> {
        let cull = self.cull.as_ref().filter(|c| c.active)?;
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&cull.args, 0, &read, 0, 16);
        queue.submit([encoder.finish()]);
        read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .ok()?;
        let words = read.slice(..).get_mapped_range();
        Some(u32::from_le_bytes([words[4], words[5], words[6], words[7]]))
    }
    pub fn count(&self) -> u64 {
        u64::from(self.count)
    }
}
/// Feet added to the altitude bound: more than the tallest blade (under
/// 8 ft) plus the rounding of the shader's single-precision distance.
const ALTITUDE_MARGIN: f64 = 64.;

/// Whether the eye is so high above the highest ground within `radius` that
/// every blade root already fails the vertex shader's `eye_distance` test,
/// so drawing them would change no pixel. Conservative: the shader's ground
/// height interpolates the four vertices of its cell, never above their
/// maximum, and its root sits `root_lift` above that.
fn above_blades(heights: &[f32], size: [u32; 2], eye: [f64; 3], radius: f32) -> bool {
    let [w, h] = size.map(|n| n as i64);
    if heights.len() as i64 != w * h || w < 2 || h < 2 {
        return false;
    }
    let reach = f64::from(radius) + 1.;
    let cell = |v: f64| (v / 8192.).floor() as i64;
    let x = (
        cell(eye[0] - reach).clamp(0, w - 1),
        (cell(eye[0] + reach) + 1).clamp(0, w - 1),
    );
    let z = (
        cell(eye[2] - reach).clamp(0, h - 1),
        (cell(eye[2] + reach) + 1).clamp(0, h - 1),
    );
    let mut top = f64::NEG_INFINITY;
    for row in z.0..=z.1 {
        for column in x.0..=x.1 {
            let v = f64::from(heights[(row * w + column) as usize]);
            if v.is_nan() {
                return false;
            }
            top = top.max(v);
        }
    }
    eye[1] - (top + f64::from(root_lift()).max(0.)) > f64::from(radius) + ALTITUDE_MARGIN
}

/// The blade pipeline: the fallback vertex path, or (`listed`) the
/// candidate pass's draw reading grown blades as instances.
#[allow(clippy::too_many_arguments)]
fn pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
    material: &wgpu::BindGroupLayout,
    lighting: &wgpu::BindGroupLayout,
    resources: &wgpu::BindGroupLayout,
    listed: bool,
) -> wgpu::RenderPipeline {
    let blade = [wgpu::VertexBufferLayout {
        array_stride: BLADE_BYTES,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x4,3=>Float32x4],
    }];
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Procedural blades"),
        bind_group_layouts: &[material, lighting, resources],
        push_constant_ranges: &[],
    });
    device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Opaque grass blades"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(if listed {
                "blade_list_vertex"
            } else {
                "blade_vertex"
            }),
            compilation_options: Default::default(),
            buffers: if listed { &blade } else { &[] },
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("blade_fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::GreaterEqual,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn altitude_bound_skips_only_when_every_root_is_out_of_reach() {
        // 4 x 4 vertices 8,192 ft apart; one 3,000 ft peak at vertex (2, 1).
        let mut heights = vec![100.; 16];
        heights[4 + 2] = 3_000.;
        let size = [4, 4];
        let radius = 500.;
        // Over flat ground far from the peak's cells.
        let flat = [1_000., 0., 1_000.];
        let at = |y: f64| [flat[0], y, flat[2]];
        let lift = f64::from(root_lift()).max(0.);
        let edge = 100. + lift + f64::from(radius) + ALTITUDE_MARGIN;
        assert!(!above_blades(&heights, size, at(edge), radius));
        assert!(above_blades(&heights, size, at(edge + 1.), radius));
        // Grass on the ground just below: never skipped.
        assert!(!above_blades(&heights, size, at(150.), radius));
        // Within reach of the peak's cell the peak sets the bound.
        let near_peak = [2. * 8192. - 200., 2_000., 8192. + 100.];
        assert!(!above_blades(&heights, size, near_peak, radius));
        let high = [near_peak[0], 3_000. + lift + 600., near_peak[2]];
        assert!(above_blades(&heights, size, high, radius));
        // A cell boundary within the radius pulls in the next vertex column.
        let beside = [8192. - 400., 2_000., 8192. + 100.];
        assert!(!above_blades(&heights, size, beside, radius));
        // No terrain table: no bound.
        assert!(!above_blades(&[], size, at(1e6), radius));
    }
}

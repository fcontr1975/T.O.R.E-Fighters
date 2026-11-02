//! Synthetic offscreen regression using the production effect vertex shader.
//! No retail art or simulation. Solid vehicle roofs stand six feet above a
//! ground plane. A fresh crater must change ground pixels but no roof pixel.
#![allow(clippy::disallowed_methods)]
use super::*;
use wgpu::util::DeviceExt;

fn shader(before: bool, hillside: bool) -> String {
    let source = include_str!("../terrain.wgsl");
    let depth = source
        .split("fn world_depth_clip")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap();
    // Only the production struct and vertex function: the glow helpers that
    // follow them in terrain.wgsl pull in the weather and haze bindings,
    // which this fixture does not declare and the vertex stage never calls.
    let effect = source
        .split("struct EffectOut")
        .nth(1)
        .unwrap()
        .split("\n}\n")
        .next()
        .unwrap();
    let mut effect = format!("struct EffectOut{effect}\n}}");
    if before {
        // The hillside baseline is the first fix: horizontal, no pull,
        // with the same 16-unit raster bias. The level baseline predates it.
        effect = effect.replace("ground_heights[ground_corner[vertex]]", "0.0");
        if !hillside {
            effect = effect.replace("p=offset+vec3<f32>(across,0.0,(corner.y*2.0-1.0)*extent.y);", "p=offset+vec3<f32>(across,0.0,(corner.y*2.0-1.0)*extent.y); let d=length(p);p=p*(1.0-min(0.02,20.0/max(d,1.0)));");
        }
    }
    format!(
        r#"
struct Scene {{ eye:vec4<f32>, right:vec4<f32>, up:vec4<f32>, forward:vec4<f32>, view:vec4<f32> }}
@group(0) @binding(0) var<uniform> scene:Scene;
@group(0) @binding(1) var tiles:texture_2d_array<u32>;
fn world_depth_clip{depth}
}}
{effect}
@fragment fn effect_fragment(in:EffectOut)->@location(0) vec4<f32> {{
 if length(in.uv-vec2<f32>(0.5))>0.48 {{discard;}}
 return vec4<f32>(0.32,0.24,0.18,1.0);
}}
struct Solid {{ @builtin(position) clip:vec4<f32>, @location(0) color:vec3<f32> }}
@vertex fn solid_vertex(@location(0) position:vec3<f32>,@location(1) color:vec3<f32>)->Solid {{
 let p=position-scene.eye.xyz;let z=dot(p,scene.forward.xyz);let f=1.7320508*scene.up.w;
 return Solid(vec4<f32>(dot(p,scene.right.xyz)*f/scene.eye.w,dot(p,scene.up.xyz)*f,world_depth_clip(z),z),color);
}}
@fragment fn solid_fragment(in:Solid)->@location(0) vec4<f32>{{return vec4<f32>(in.color,1.0);}}
"#
    )
}

/// Run explicitly on a display-free GPU host. TORE_DECAL_RENDERS writes the
/// matched synthetic before/after PPMs alongside the pixel assertions.
#[test]
#[ignore = "requires an offscreen GPU adapter"]
fn gpu_ground_decals_preserve_vehicle_pixels() {
    render_depth_cases(0.);
}

#[test]
#[ignore = "requires an offscreen GPU adapter"]
fn gpu_hillside_decals_cover_ground_and_preserve_vehicle_pixels() {
    render_depth_cases(15.);
}

#[test]
#[ignore = "requires an offscreen GPU adapter"]
fn gpu_steep_hillside_decals_cover_ground_and_preserve_vehicle_pixels() {
    render_depth_cases(30.);
}

fn render_depth_cases(slope_degrees: f64) {
    let hillside = slope_degrees > 0.;
    pollster::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap();
        println!("decal adapter: {:?}", adapter.get_info());
        // Ranges from the external view to the longest sensor zoom, then
        // the same ground seen from almost straight above.
        // The last field says whether the old behaviour is expected to
        // show its bug there (looking steeply down or along the ground it
        // does not).
        let mut failures = Vec::new();
        for (name, range, fov, pitch, exposes, half) in [
            ("external", 1_200_f32, 18_f32, -0.5_f32, true, 40_f64),
            ("sensor", 30_000., 0.6, -0.5, true, 40.),
            ("long-sensor", 150_000., 0.12, -0.5, true, 40.),
            ("far-sensor", 600_000., 0.03, -0.5, true, 40.),
            ("grazing", 30_000., 0.6, -0.12, false, 40.),
            ("skimming", 30_000., 0.6, -0.06, false, 40.),
            ("overhead", 1_200., 18., -1.5, false, 40.),
            ("overhead-sensor", 30_000., 0.6, -1.5, false, 40.),
            // A gun round's pock mark, a small crater, on the ground beside
            // each vehicle as well as under it.
            ("pock", 300., 18., -0.5, false, 8.),
        ] {
            let name = if hillside {
                format!("hill{slope_degrees}-{name}")
            } else {
                name.to_string()
            };
            let slope = slope_degrees.to_radians().tan();
            let ground = |x: f64, z: f64| slope * (0.6 * x + 0.8 * z);
            let (sin, cos) = pitch.sin_cos();
            let eye = [0., -sin * range, -cos * range];
            let values = [
                eye[0],
                eye[1],
                eye[2],
                2.,
                1.,
                0.,
                0.,
                0.,
                0.,
                cos,
                -sin,
                (30_f32.to_radians().tan()) / (fov.to_radians() / 2.).tan(),
                0.,
                sin,
                cos,
                0.,
                // GL remaps clip depth through [-1, 1], losing the near=1
                // precision of Vulkan/Metal/DX12. Move only this fixture's
                // near plane out so it can test six-foot separation there.
                if adapter.get_info().backend == wgpu::Backend::Gl {
                    1000.
                } else {
                    1.
                },
                0.,
                0.,
                0.,
            ];
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: &values
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect::<Vec<_>>(),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let tiles = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 128,
                    height: 128,
                    depth_or_array_layers: 2,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tiles.create_view(&Default::default());
            let mut geometry = Vec::<f32>::new();
            let mut quad = |x: f32, y: f32, z: f32, w: f32, h: f32, color: [f32; 3]| {
                for [a, b] in [
                    [-1., -1.],
                    [1., -1.],
                    [1., 1.],
                    [-1., -1.],
                    [1., 1.],
                    [-1., 1.],
                ] {
                    let (px, pz) = (x + a * w, z + b * h);
                    geometry.extend([px, y + ground(f64::from(px), f64::from(pz)) as f32, pz]);
                    geometry.extend(color);
                }
            };
            quad(0., 0., 0., 10_000., 10_000., [0.15, 0.18, 0.12]);
            for x in [-55., 0., 55.] {
                quad(x, 6., 0., 12., 24., [0.8, 0.8, 0.8]);
            }
            let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: &geometry
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect::<Vec<_>>(),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let mut sprites = Vec::new();
            let spots: &[f64] = if half < 20. { &[0., 36.] } else { &[0.] };
            for (x, z) in [-55., 0., 55.]
                .into_iter()
                .flat_map(|x| spots.iter().map(move |z| (x, *z)))
            {
                let mark = Sprite {
                    position: [x, ground(x, z), z],
                    extent: [half, half],
                    mode: Mode::Flat,
                    cell: [0., 0., 128., 128.],
                    layer: 0,
                    opacity: 1.,
                    emissive: false,
                    tint: PLAIN,
                    glow: 0.,
                    heat: 0.,
                    ember: 0.,
                    fresh: 1.,
                };
                sprites.extend(
                    instance_values(&mark, eye.map(f64::from), &ground)
                        .into_iter()
                        .flat_map(f32::to_le_bytes),
                );
            }
            let marks = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: &sprites,
                usage: wgpu::BufferUsages::VERTEX,
            });
            let texture = |format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 512,
                        height: 256,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let image = texture(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let depth = texture(
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 512 * 256 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut outputs = Vec::new();
            for (label, before, draw_marks) in [
                ("reference", false, false),
                ("before", true, true),
                ("after", false, true),
                ("coverage", false, true),
            ] {
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: None,
                    source: wgpu::ShaderSource::Wgsl(shader(before, hillside).into()),
                });
                let (normal, flat) =
                    EffectRenderer::pipelines(&device, wgpu::TextureFormat::Rgba8Unorm, &module, 1);
                let shared_layout = normal.get_bind_group_layout(0);
                let effect = if before && !hillside { normal } else { flat };
                let solid = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: None,
                    layout: None,
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("solid_vertex"),
                        compilation_options: Default::default(),
                        buffers: &[wgpu::VertexBufferLayout {
                            array_stride: 24,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3],
                        }],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("solid_fragment"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: true,
                        depth_compare: wgpu::CompareFunction::Greater,
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    multiview: None,
                    cache: None,
                });
                let solid_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &solid.get_bind_group_layout(0),
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    }],
                });
                let effect_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &shared_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                    ],
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let color = image.create_view(&Default::default());
                    let depth = depth.create_view(&Default::default());
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &color,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.15,
                                    g: 0.18,
                                    b: 0.12,
                                    a: 1.,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(0.),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        ..Default::default()
                    });
                    pass.set_pipeline(&solid);
                    pass.set_bind_group(0, &solid_bind, &[]);
                    pass.set_vertex_buffer(0, vertices.slice(..));
                    // The coverage oracle omits terrain depth, but keeps roofs.
                    // Its crater footprint must survive with terrain present.
                    pass.draw(if label == "coverage" { 6..24 } else { 0..24 }, 0..1);
                    if draw_marks {
                        pass.set_pipeline(&effect);
                        pass.set_bind_group(0, &effect_bind, &[]);
                        pass.set_vertex_buffer(0, marks.slice(..));
                        pass.draw(0..6, 0..(3 * spots.len() as u32));
                    }
                }
                encoder.copy_texture_to_buffer(
                    image.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &readback,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(512 * 4),
                            rows_per_image: Some(256),
                        },
                    },
                    image.size(),
                );
                queue.submit([encoder.finish()]);
                let (tx, rx) = std::sync::mpsc::channel();
                readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                rx.recv().unwrap().unwrap();
                let pixels = readback.slice(..).get_mapped_range().to_vec();
                readback.unmap();
                if let Ok(out) = std::env::var("TORE_DECAL_RENDERS") {
                    std::fs::create_dir_all(&out).unwrap();
                    let mut ppm = b"P6\n512 256\n255\n".to_vec();
                    ppm.extend(pixels.chunks_exact(4).flat_map(|p| p[..3].iter().copied()));
                    std::fs::write(
                        std::path::Path::new(&out).join(format!("{name}-{label}.ppm")),
                        ppm,
                    )
                    .unwrap();
                }
                outputs.push(pixels);
            }
            let vehicle = |p: &[u8]| p[0] > 190 && p[1] > 190;
            let roof_count = outputs[0].chunks_exact(4).filter(|p| vehicle(p)).count();
            let lost = |image: &[u8]| {
                outputs[0]
                    .chunks_exact(4)
                    .zip(image.chunks_exact(4))
                    .filter(|(a, b)| vehicle(a) && a != b)
                    .count()
            };
            let mut check = |ok: bool, what: String| {
                if !ok {
                    failures.push(format!("{name}: {what}"));
                }
            };
            check(roof_count > 100, "fixture must resolve the vehicles".into());
            if !hillside && exposes {
                check(
                    lost(&outputs[1]) > roof_count / 2,
                    "old pull must reproduce the bug".into(),
                );
            }
            check(
                lost(&outputs[2]) == 0,
                format!("crater covered {} live vehicle pixels", lost(&outputs[2])),
            );
            let changed = outputs[0]
                .chunks_exact(4)
                .zip(outputs[2].chunks_exact(4))
                .filter(|(a, b)| !vehicle(a) && a != b)
                .count();
            let mut expected = 0;
            let mut missing = 0;
            let mut before_missing = 0;
            for (((reference, after), before), coverage) in outputs[0]
                .chunks_exact(4)
                .zip(outputs[2].chunks_exact(4))
                .zip(outputs[1].chunks_exact(4))
                .zip(outputs[3].chunks_exact(4))
            {
                if !vehicle(reference) && coverage != reference {
                    expected += 1;
                    missing += usize::from(after != coverage);
                    before_missing += usize::from(before != coverage);
                }
            }
            check(
                changed * 10 >= expected * 9,
                format!("craters must remain visible on terrain ({changed} marked of {expected})"),
            );
            if hillside {
                check(
                    missing == 0,
                    format!("terrain clipped {missing} of {expected} crater pixels"),
                );
                if exposes {
                    check(
                        before_missing > expected / 3,
                        "horizontal baseline must expose the hillside bug".into(),
                    );
                }
            }
            println!(
                "{name}: expected ground pixels {expected}, before missing {before_missing}, after missing {missing}"
            );
            println!(
                "{name}: {roof_count} roof pixels, before lost {}, after lost {}, {changed} ground pixels marked",
                lost(&outputs[1]),
                lost(&outputs[2])
            );
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

#[test]
fn ground_corners_follow_slopes_and_leave_standing_effects_unchanged() {
    let mut sprite = sprites(
        &Art::synthetic(),
        &[],
        &[MarkPose {
            position: [1_000_000., 500., 800_000.],
            kind: MarkKind::Crater(9),
            age: 0,
            strength: 1.,
            scale: 100,
        }],
        crate::blast_climate::Climate::default(),
    )[0];
    for angle in [10_f64, 15., 20.] {
        let slope = angle.to_radians().tan();
        let ground =
            |x: f64, z: f64| 500. + slope * (0.6 * (x - 1_000_000.) + 0.8 * (z - 800_000.));
        let corners = ground_corners(&sprite, &ground);
        for (i, [x, z]) in [[-1., -1.], [1., -1.], [1., 1.], [-1., 1.]]
            .into_iter()
            .enumerate()
        {
            let expected = slope * (0.6 * x * sprite.extent[0] + 0.8 * z * sprite.extent[1]);
            assert!((f64::from(corners[i]) - expected).abs() < 0.000_01);
        }
        // A plane's diagonal midpoints agree too, not just the corners.
        assert!((corners[0] + corners[2]).abs() < 0.000_01);
        assert!((corners[1] + corners[3]).abs() < 0.000_01);
    }
    for mode in [Mode::Standing, Mode::Billboard] {
        sprite.mode = mode;
        assert_eq!(
            ground_corners(&sprite, &|_, _| panic!(
                "standing effects must not sample terrain"
            )),
            [0.; 4]
        );
    }
}

#[test]
fn corner_sampling_keeps_subfoot_precision_far_from_the_world_origin() {
    use tore_formats::theater::{CELL_FEET, TerrainCell};
    let mut world = tore_world::test_support::terrain();
    world.theater.cols = 130;
    world.theater.rows = 100;
    world.theater.cells = vec![
        TerrainCell {
            color: 100,
            class: 2,
            elevation: 0
        };
        130 * 100
    ];
    for (dx, dz, elevation) in [(0, 0, 0), (1, 0, 6), (0, 1, 7), (1, 1, 13)] {
        world.theater.cells[(97 + dz) * 130 + 123 + dx].elevation = elevation;
    }
    let base = [123. * f64::from(CELL_FEET), 97. * f64::from(CELL_FEET)];
    // Both halves of the terrain cell, on a 16.07 degree plane.
    for fraction in [0.25, 0.75] {
        let x = base[0] + fraction * f64::from(CELL_FEET) + 0.031;
        let z = base[1] + fraction * f64::from(CELL_FEET) + 0.027;
        let expected = (x - base[0]) * 6. / 32. + (z - base[1]) * 7. / 32.;
        assert!((ground_height(&world, x, z) - expected).abs() < 1e-9);
        assert!((15.0..17.0).contains(&world.slope_degrees(x, z)));
    }
}

// The GPU cases above fit these two terms; this keeps either from being
// dropped on a machine without an adapter.
const _: () = {
    assert!(GROUND_DEPTH_BIAS.constant > 0);
    assert!(GROUND_DEPTH_BIAS.slope_scale >= 0.1 && GROUND_DEPTH_BIAS.slope_scale <= 4.5);
    assert!(GROUND_DEPTH_BIAS.clamp == 0.);
};

//! Runtime graphics validation using only synthetic scenery and weather.
#![allow(clippy::disallowed_methods)]
use super::*;
use crate::graphics::*;
use std::collections::BTreeMap;
use tore_formats::weather::shape::{Primitive, WeatherShape};

#[test]
#[ignore = "requires a GPU adapter"]
fn gpu_runtime_graphics_changes_keep_sun_and_spotting_valid() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            flags: wgpu::InstanceFlags::VALIDATION,
            ..Default::default()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        eprintln!("Runtime graphics adapter: {:?}", adapter.get_info());
        let specific =
            adapter.features() & wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: specific,
                ..Default::default()
            })
            .await
            .unwrap();
        let mut w = tore_world::test_support::terrain();
        let mut s = crate::scenery::tests::scenery();
        s.smooth_weather = false;
        s.vertices = plane(0., 20_000.);
        s.texture_indices = vec![100; 65536];
        s.sky_indices = vec![100; 65536];
        s.fill = Some(Arc::new(crate::fill_renderer::tests::synthetic(
            &[(0, 1000)],
            60,
            300,
        )));
        let atlas = crate::vegetation::Atlas::synthetic(vec![10; 65536], 0);
        let first = s.texture_indices.len() / 65536;
        s.vegetation_meshes = Some(Arc::new(crate::vegetation_shapes::Library::build(
            atlas.region,
            &mut s.texture_indices,
            first,
        )));
        s.vegetation = Some(Arc::new(atlas));
        let mut module =
            tore_formats::weather::Module::parse(&tore_formats::weather::synthetic_module(1))
                .unwrap();
        let l = &mut module.layers[0];
        l.flags = 8;
        l.start_seconds = 0;
        l.end_seconds = 86399;
        l.sunrise_seconds = 0;
        l.sunset_seconds = 86400;
        let angles = tore_sim::environment::sun_angles(l, 9 * 3600).unwrap();
        w.weather = tore_sim::environment::Environment::new(
            tore_sim::environment::Configuration::new(module, 9, 0, 0, None).unwrap(),
        );
        let empty = WeatherShape {
            primitives: vec![],
            scale_exponent: 8,
            publishes_point: false,
        };
        s.celestial = Some(crate::celestial::Celestial {
            sun: WeatherShape {
                primitives: vec![Primitive::Circle {
                    center: [0., 0., 160.],
                    diameter: 4,
                    fill: 254,
                }],
                ..empty.clone()
            },
            moon: empty.clone(),
            stars: empty,
            sun_effects: true,
            moon_texture: 0,
            moon_uv: [0., 0., 1., 1.],
            sun_remap: 0,
            shade_rows: BTreeMap::from([(
                w.weather
                    .configuration()
                    .shade_remap(w.weather.sample(5000.).unwrap().shade)
                    .color,
                0,
            )]),
            light_rows: [0; 2],
            flare: tore_formats::weather::flare::Layout {
                circles: vec![tore_formats::weather::flare::Circle {
                    offset_percent: 50,
                    radius: 10,
                    fill: 265,
                }],
            },
        });
        let sun = crate::celestial::rotate([0., 0., 1.], angles);
        let mut forward = Camera::new();
        forward.position[1] = 5000.;
        forward.yaw = sun[0].atan2(sun[2]);
        forward.pitch = sun[1].asin() - 0.1;

        // Exercise each surface format with a renderer of its own, as at startup.
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ] {
            let mut options = Options {
                anti_aliasing: AntiAliasing::Off,
                spotting_aid: SpottingAid::Strong,
                ..Default::default()
            };
            let mut renderer = SimRenderer::new(&device, &queue, format, &s, options, 1);
            // A synthetic contact uses the normal aircraft material and rim pass.
            let vertices = plane(4500., 56.);
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Runtime graphics contact"),
                contents: &bytes(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            renderer.aircraft = Some((renderer.bind.clone(), buffer.clone(), 6));
            renderer.battle = Some((buffer, 6));
            renderer.battle_contacts = vec![Contact::new(0, 6, [0., 4500., 0.], 112.).unwrap()];
            let levels: Vec<_> = AntiAliasing::ALL
                .into_iter()
                .filter(|level| {
                    let n = level.samples();
                    (n == 1 || !specific.is_empty() || n == 4)
                        && [format, wgpu::TextureFormat::Depth32Float]
                            .into_iter()
                            .all(|f| {
                                adapter
                                    .get_texture_format_features(f)
                                    .flags
                                    .sample_count_supported(n)
                            })
                })
                .chain([AntiAliasing::Off])
                .collect();
            assert!(levels.contains(&AntiAliasing::X4));
            for level in levels {
                options.anti_aliasing = level;
                eprintln!("Runtime graphics {format:?}: {}", level.label());
                // Each level draws several frames before and after changing scale,
                // shadows, detail and sizes. Repeated samples exercise the early return.
                for change in 0..5 {
                    options.render_scale = RENDER_SCALES[change];
                    options.shadow_quality = ShadowQuality::ALL[change];
                    options.spotting_aid = SpottingAid::ALL[(change + 2) % 3];
                    options.ground_texture = GroundTexture::ALL[change % 3];
                    options.terrain_filtering = change % 2 == 0;
                    options.vegetation_density = VegDensity::ALL[change % 4];
                    options.vegetation_cards = VegCards::ALL[change % 4];
                    options.vegetation_distance_nm = VEGETATION_DISTANCES[change % 4];
                    options.vegetation_detail_nm = VEGETATION_DETAILS[change % 4];
                    options.building_distance_nm = BUILDING_DISTANCES[change % 3];
                    options.building_detail_nm = BUILDING_DETAILS[change];
                    options.ground_texture_distance_nm = GROUND_TEXTURE_DISTANCES[change % 3];
                    options.grass = Grass::ALL[change % 3];
                    options.grass_density = (change != 0).then_some(VegDensity::ALL[change % 4]);
                    options.grass_distance_ft = GRASS_DISTANCES[change % 4];
                    options.shadow_distance = ShadowDistance::ALL[change % 6];
                    renderer.set_graphics(&device, options, level.samples());
                    for (kind, slot, size, sensor) in [
                        (ViewKind::Main, 0, [192 + change as u32 * 8, 128], false),
                        (ViewKind::Mirror, 1, [96, 64], false),
                        (ViewKind::Panel(2), 2, [80, 60], true),
                    ] {
                        forward.weather_slot = slot;
                        forward.sensor = sensor;
                        assert!(
                            !crate::lens_flare::circles(&w, &s, &forward, size).is_empty(),
                            "fixture must dispatch sun visibility"
                        );
                        let output = device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("Runtime graphics output"),
                            size: wgpu::Extent3d {
                                width: size[0],
                                height: size[1],
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                            view_formats: &[],
                        });
                        let mut encoder = device.create_command_encoder(&Default::default());
                        renderer.draw_view(
                            &device,
                            &queue,
                            &mut encoder,
                            kind,
                            &output.create_view(&Default::default()),
                            size,
                            &forward,
                            &w,
                            &s,
                        );
                        queue.submit([encoder.finish()]);
                        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                    }
                }
            }
        }
    });
}

fn plane(y: f32, radius: f32) -> Vec<f32> {
    [
        (-1., -1.),
        (-1., 1.),
        (1., -1.),
        (1., -1.),
        (-1., 1.),
        (1., 1.),
    ]
    .into_iter()
    .flat_map(|(x, z)| [x * radius, y, z * radius, 0., 0., -1., 0.6, 0.6, 0.6, -1.])
    .collect()
}

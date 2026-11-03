//! Runs the production sampling functions against synthetic material cutouts.
#![allow(clippy::disallowed_methods)]

#[test]
#[ignore = "requires a GPU adapter"]
fn vegetation_gpu_cutout_never_uses_transparency_as_a_material() {
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
        // Only tile access is synthetic. Execute the actual colour and alpha
        // functions, including the exact boundary that produced white bark.
        let production = include_str!("terrain.wgsl").replace("\r\n", "\n");
        let mut shader = String::from(
            r#"
@group(0) @binding(0) var<storage,read_write> result:array<vec4<f32>>;
fn tile_size()->vec2<u32> {return vec2<u32>(2u);}
fn read_tile(at:vec2<i32>,layer:i32)->u32 {
 if layer==4 {return 255u;}
 if layer==5 {
  if at.y==0 {return select(40u,16u,at.x==1);}
  return 255u;
 }
 if all(at==vec2<i32>(0)) {return u32(layer)*32u+8u;}
 return 255u;
}
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
 let uv=array<vec2<f32>,4>(vec2<f32>(0.25,0.25),vec2<f32>(0.51,0.25),
                           vec2<f32>(0.49,0.25),vec2<f32>(0.9,0.9));
 result[id.x]=veg_sample(uv[id.x%4u],i32(id.x/4u),
                         vec3<f32>(1.0),vec3<f32>(0.2,0.1,0.05));
}
"#,
        );
        for name in ["veg_alpha", "veg_material", "veg_sample"] {
            let start = production.find(&format!("fn {name}(")).unwrap();
            let end = start + production[start..].find("\n}").unwrap() + 2;
            shader.push_str(&production[start..end]);
            shader.push('\n');
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("production vegetation cutout regression"),
            source: wgpu::ShaderSource::Wgsl(shader.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let size = 24 * 16;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: output.as_entire_binding(),
            }],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(24, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &read, 0, size);
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        read.slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = read.slice(..).get_mapped_range();
        let values: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        for layer in 0..6 {
            for sample in 0..4 {
                let actual = &values[(layer * 4 + sample) * 4..][..4];
                let alpha = if layer == 4 || sample == 3 {
                    0.
                } else if layer == 5 || sample == 0 {
                    1.
                } else if sample == 1 {
                    0.48
                } else {
                    0.52
                };
                let shade = if layer == 5 && sample == 1 { 16. } else { 8. };
                let tint = if layer == 1 || (layer == 5 && sample != 1) {
                    [0.2, 0.1, 0.05]
                } else {
                    [1.; 3]
                };
                let rgb = if alpha == 0. {
                    [0.; 3]
                } else {
                    tint.map(|c| c * (0.25 + 1.25 * shade / 31.))
                };
                for (got, want) in actual.iter().zip([rgb[0], rgb[1], rgb[2], alpha]) {
                    assert!(
                        (got - want).abs() < 1e-5,
                        "layer {layer}, sample {sample}: {actual:?}, expected {rgb:?} alpha {alpha}"
                    );
                }
            }
        }
    });
}

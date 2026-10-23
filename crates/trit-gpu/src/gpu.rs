//! wgpu 后端：同一个 loader 上跑三值与 f32 两个点积内核。

use std::time::Instant;

use wgpu::util::DeviceExt;

use crate::{Report, dot_scalar, pack_dual_rail};

const WORKGROUP: u32 = 256;
const WORDS_PER_TRIT_BLOCK: usize = 32;

fn storage(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// 跑一次完整实验；没有可用 adapter 时返回 `None`。
pub fn run(trits: usize) -> Option<Report> {
    if trits == 0 {
        return None;
    }
    pollster::block_on(run_async(trits))
}

async fn run_async(trits: usize) -> Option<Report> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .ok()?;
    let info = adapter.get_info();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .ok()?;

    // 用确定性随机数据：周期模式会被显存压缩，f32 那侧会假性变快。
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % 3) as i32 - 1
    };
    let values_a: Vec<i32> = (0..trits).map(|_| next()).collect();
    let values_b: Vec<i32> = (0..trits).map(|_| next()).collect();
    let (a_pos_data, a_neg_data) = pack_dual_rail(&values_a);
    let (b_pos_data, b_neg_data) = pack_dual_rail(&values_b);
    let expected = dot_scalar(&values_a, &values_b);
    let blocks = a_pos_data.len();
    debug_assert!(blocks * WORDS_PER_TRIT_BLOCK >= trits, "打包尺寸");

    let usage = wgpu::BufferUsages::STORAGE;
    let make_words = |data: &[u32]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(data),
            usage: usage | wgpu::BufferUsages::COPY_SRC,
        })
    };
    let a_pos = make_words(&a_pos_data);
    let a_neg = make_words(&a_neg_data);
    let b_pos = make_words(&b_pos_data);
    let b_neg = make_words(&b_neg_data);
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: usage | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ternary"),
        source: wgpu::ShaderSource::Wgsl(include_str!("ternary.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            storage(0, true),
            storage(1, true),
            storage(2, true),
            storage(3, true),
            storage(4, false),
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("ternary"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: a_pos.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: a_neg.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: b_pos.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: b_neg.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: out.as_entire_binding(),
            },
        ],
    });
    let grid = blocks.div_ceil(WORKGROUP as usize) as u32;

    // 一次「提交但不等待」的派发；计时时连续提交后统一等待。
    let submit = |zero: bool| {
        if zero {
            queue.write_buffer(&out, 0, &[0u8; 4]);
        }
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(grid, 1, 1);
        }
        queue.submit([encoder.finish()]);
    };

    let gpu_ns_per_trit = median_ns_per(&device, trits, || submit(false));

    submit(true);
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let gpu_value = read_i32(&device, &queue, &out)? as i64;

    // 同设备 f32 点积。
    let fa: Vec<f32> = values_a.iter().map(|v| *v as f32).collect();
    let fb: Vec<f32> = values_b.iter().map(|v| *v as f32).collect();
    let fa_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&fa),
        usage,
    });
    let fb_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&fb),
        usage,
    });
    let fgrid = (trits.div_ceil(WORKGROUP as usize) as u32).min(16384);
    let partials = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(fgrid) * 4,
        usage: usage | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let params: [u32; 2] = [trits as u32, fgrid];
    let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&params),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let fshader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("f32"),
        source: wgpu::ShaderSource::Wgsl(include_str!("float.wgsl").into()),
    });
    let flayout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            storage(0, true),
            storage(1, true),
            storage(2, false),
            uniform(3),
        ],
    });
    let fpipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&flayout)],
        immediate_size: 0,
    });
    let fpipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("f32"),
        layout: Some(&fpipeline_layout),
        module: &fshader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let fbind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &flayout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: fa_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: fb_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: partials.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: params_buf.as_entire_binding(),
            },
        ],
    });
    let submit_float = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&fpipeline);
            pass.set_bind_group(0, &fbind, &[]);
            pass.dispatch_workgroups(fgrid, 1, 1);
        }
        queue.submit([encoder.finish()]);
    };
    let gpu_f32_ns_per_element = median_ns_per(&device, trits, submit_float);

    submit_float();
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let partial_values = read_f32(&device, &queue, &partials, fgrid as usize)?;
    let gpu_float: f32 = partial_values.iter().sum();
    let cpu_float: f32 = fa.iter().zip(fb.iter()).map(|(x, y)| x * y).sum();

    // CPU 位并行对照。
    let start = Instant::now();
    let mut cpu_value = 0i64;
    for _ in 0..5 {
        cpu_value = std::hint::black_box(crate::dot_words(
            std::hint::black_box(&a_pos_data),
            std::hint::black_box(&a_neg_data),
            std::hint::black_box(&b_pos_data),
            std::hint::black_box(&b_neg_data),
        ));
    }
    let cpu_ns_per_trit = start.elapsed().as_nanos() as f64 / (5.0 * trits as f64);
    debug_assert_eq!(cpu_value, expected);

    Some(Report {
        adapter: info.name,
        backend: format!("{:?}", info.backend),
        trits,
        expected,
        cpu: cpu_value,
        gpu: gpu_value,
        matches: gpu_value == expected,
        gpu_ns_per_trit,
        cpu_ns_per_trit,
        gpu_f32_ns_per_element,
        float_relative_error: if cpu_float == 0.0 {
            0.0
        } else {
            ((gpu_float - cpu_float) / cpu_float).abs() as f64
        },
    })
}

/// 5 批 × 每批 20 次派发，取中位数（GPU 计时抖动大）。
fn median_ns_per(device: &wgpu::Device, trits: usize, mut submit: impl FnMut()) -> f64 {
    let iters = 20;
    let mut samples = Vec::with_capacity(5);
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..iters {
            submit();
        }
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        samples.push(start.elapsed().as_nanos() as f64 / (iters as f64 * trits as f64));
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("有限值"));
    samples[samples.len() / 2]
}

fn read_i32(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer) -> Option<i32> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, 4);
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let mapped = slice.get_mapped_range().expect("map");
    let value = i32::from_le_bytes([mapped[0], mapped[1], mapped[2], mapped[3]]);
    drop(mapped);
    staging.unmap();
    Some(value)
}

fn read_f32(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    len: usize,
) -> Option<Vec<f32>> {
    let size = (len * 4) as u64;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let mapped = slice.get_mapped_range().expect("map");
    let values = bytemuck::cast_slice::<u8, f32>(&mapped).to_vec();
    drop(mapped);
    staging.unmap();
    Some(values)
}

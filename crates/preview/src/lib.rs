//! GPU preview: the whole scene (follow points + hit circles) is one instanced draw over a pre-rendered frame atlas,
//! so a refresh costs a few microseconds of CPU no matter how many animation frames the skin has.
use bytemuck::{Pod, Zeroable};
use engine::atlas::Atlas;
use engine::lazer::{self, Kind, Obj};

pub mod game_renderer;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default, Debug)]
pub struct Instance {
    pub pos: [f32; 2],
    pub half_size: [f32; 2],
    pub rot_alpha: [f32; 2],
    pub _pad: [f32; 2],
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    viewport: [f32; 2],
    atlas: [f32; 2],
}

#[derive(Clone)]
pub struct Settings {
    pub cs: f32,
    pub ar: f32,
    /// Image resolution: 1 for followpoint-N.png, 2 for @2x files.
    pub ratio: f32,
    pub game_fx: bool,
    pub ini_set: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { cs: 4.0, ar: 9.0, ratio: 1.0, game_fx: true, ini_set: true }
    }
}

/// Build every instance for the live-map view at map time `t` (ms). `ss` is the render scale (2 = supersampled).
pub fn build_scene(
    objs: &[Obj],
    atlas: &Atlas,
    t: f32,
    st: &Settings,
    ss: f32,
    scratch: &mut Vec<lazer::PointSprite>,
    out: &mut Vec<Instance>,
) {
    out.clear();
    let off = lazer::FIELD_OFF;
    let so = lazer::circle_scale(st.cs);
    let tp = lazer::time_preempt(st.ar);
    let fade_in = 400.0 * (tp / 450.0).min(1.0);
    let n = atlas.frames.len();
    for pair in objs.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if b.new_combo || a.kind == Kind::Spinner || b.kind == Kind::Spinner {
            continue;
        }
        let ts = a.end_time;
        if t < ts - 1000.0 || t > b.time + lazer::FADE + 50.0 {
            continue;
        }
        scratch.clear();
        lazer::follow_points(
            [a.end_x + off[0], a.end_y + off[1]],
            [b.x + off[0], b.y + off[1]],
            ts,
            b.time,
            t,
            st.ar,
            st.cs,
            st.game_fx,
            scratch,
        );
        for q in scratch.iter() {
            let fi = lazer::frame_index(n, atlas.fps, st.ini_set, q.tt);
            let f = atlas.frames[fi];
            let Some((ox, oy)) = f.origin else { continue };
            let k = q.sc / st.ratio * ss;
            out.push(Instance {
                pos: [q.pos[0] * ss, q.pos[1] * ss],
                half_size: [f.w as f32 * k * 0.5, f.h as f32 * k * 0.5],
                rot_alpha: [q.rot_deg.to_radians(), q.alpha],
                uv: [ox as f32, oy as f32, (ox + f.w) as f32, (oy + f.h) as f32],
                ..Default::default()
            });
        }
    }
    // hit circles on top, later objects first so earlier ones end up above
    let r = 64.0 * so * ss;
    for o in objs.iter().rev() {
        let appear = o.time - tp;
        if o.kind == Kind::Spinner || t < appear || t > o.end_time + 150.0 {
            continue;
        }
        let mut al = ((t - appear) / fade_in).min(1.0);
        if t > o.end_time {
            al *= 1.0 - (t - o.end_time) / 150.0;
        }
        let c = [(o.x + off[0]) * ss, (o.y + off[1]) * ss];
        let shape = |color: [f32; 4], kind: f32, radius: f32, width: f32| Instance {
            pos: c,
            half_size: [radius + 1.0, radius + 1.0],
            rot_alpha: [0.0, al],
            color,
            params: [kind, width, 0.0, 0.0],
            ..Default::default()
        };
        out.push(shape([25.0 / 255.0, 25.0 / 255.0, 25.0 / 255.0, 1.0], 1.0, r, 0.0));
        out.push(shape([235.0 / 255.0, 235.0 / 255.0, 235.0 / 255.0, 1.0], 2.0, r, 3.0 * ss));
        if t < o.time {
            let ap = r * (1.0 + 3.0 * (1.0 - ((t - appear) / tp).min(1.0)));
            out.push(shape([235.0 / 255.0, 235.0 / 255.0, 235.0 / 255.0, 0.8], 2.0, ap, ss));
        }
    }
}

pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct Renderer {
    format: wgpu::TextureFormat,
    queue: wgpu::Queue,
    device: wgpu::Device,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    instance_buf: wgpu::Buffer,
    capacity: usize,
    pub target: wgpu::Texture,
    target_view: wgpu::TextureView,
    pub size: (u32, u32),
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, atlas: &Atlas, size: (u32, u32)) -> Self {
        Self::with_format(device, queue, atlas, size, TARGET_FORMAT)
    }

    /// Like `new`, for rendering straight into a swapchain of the given (non-sRGB) format.
    pub fn with_format(
        device: wgpu::Device,
        queue: wgpu::Queue,
        atlas: &Atlas,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> Self {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame atlas"),
            size: wgpu::Extent3d { width: atlas.width, height: atlas.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            tex.as_image_copy(),
            &atlas.rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(atlas.width * 4), rows_per_image: Some(atlas.height) },
            wgpu::Extent3d { width: atlas.width, height: atlas.height, depth_or_array_layers: 1 },
        );
        let view = tex.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let globals = Globals { viewport: [size.0 as f32, size.1 as f32], atlas: [atlas.width as f32, atlas.height as f32] };
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ubuf, 0, bytemuck::bytes_of(&globals));
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprites"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        // `_pad` keeps the vec4 fields 16-byte aligned, so the uv attribute starts at byte 32.
        let attributes = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 8, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 16, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 3 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 48, shader_location: 4 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 64, shader_location: 5 },
        ];
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprites"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let capacity = 4096;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&Default::default());
        Renderer { format, queue, device, pipeline, bind_group, instance_buf, capacity, target, target_view, size }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Upload the instances and draw the scene into `target`. Returns once the commands are submitted (not finished).
    pub fn render(&mut self, instances: &[Instance], clear: [f64; 4]) {
        let view = self.target_view.clone();
        self.render_to_view(&view, instances, clear);
    }

    /// Same, into any view of the renderer's format (for example the current swapchain texture).
    pub fn render_to_view(&mut self, view: &wgpu::TextureView, instances: &[Instance], clear: [f64; 4]) {
        let n = instances.len().min(self.capacity);
        if n > 0 {
            self.queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&instances[..n]));
        }
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: clear[3] }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.instance_buf.slice(..));
            pass.draw(0..6, 0..n as u32);
        }
        self.queue.submit(Some(enc.finish()));
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }
}

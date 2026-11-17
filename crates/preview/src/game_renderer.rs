//! Renders a `game::scene::Frame`: skin sprites in one instanced draw per run, slider bodies with a depth buffer
//! (depth pre-pass, then color where depth is equal, so overlapping parts of one slider never blend twice).
use bytemuck::{Pod, Zeroable};
use game::scene::{Frame, Op, SliderGeom};

use crate::Instance;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    viewport: [f32; 2],
    sheet: [f32; 2],
    scale: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct SliderInst {
    border: [f32; 4],
    outer: [f32; 4],
    inner: [f32; 4],
    misc: [f32; 4],
}

const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;

pub struct GameRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sprite_pipe: wgpu::RenderPipeline,
    add_pipe: wgpu::RenderPipeline,
    prepass_pipe: wgpu::RenderPipeline,
    body_pipe: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    globals: wgpu::Buffer,
    instances: wgpu::Buffer,
    inst_cap: usize,
    slider_insts: wgpu::Buffer,
    slider_cap: usize,
    verts: wgpu::Buffer,
    /// First vertex of each slider's geometry in `verts`.
    base: Vec<u32>,
    depth: wgpu::TextureView,
    msaa: wgpu::TextureView,
    format: wgpu::TextureFormat,
    scratch: Vec<Instance>,
    sinst: Vec<SliderInst>,
    sheet_size: (u32, u32),
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pub size: (u32, u32),
    pub scale: f32,
    /// Pixel position of the view's top-left corner.
    pub origin: [f32; 2],
}

fn blend() -> wgpu::BlendState {
    let c = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };
    wgpu::BlendState { color: c, alpha: c }
}

fn msaa_tex(device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("msaa"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn depth_tex(device: &wgpu::Device, size: (u32, u32)) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

impl GameRenderer {
    /// `sheet` is premultiplied RGBA8 of `sheet_size`; `geoms` the slider meshes; `scale` the view-to-pixel factor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat, size: (u32, u32), sheet: &[u8], sheet_size: (u32, u32), geoms: &[Option<SliderGeom>], scale: f32) -> Self {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("skin sheet"),
            size: wgpu::Extent3d { width: sheet_size.0, height: sheet_size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            tex.as_image_copy(),
            sheet,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(sheet_size.0 * 4), rows_per_image: Some(sheet_size.1) },
            wgpu::Extent3d { width: sheet_size.0, height: sheet_size.1, depth_or_array_layers: 1 },
        );
        let view = tex.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                entry(1, wgpu::ShaderStages::FRAGMENT, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
                entry(2, wgpu::ShaderStages::FRAGMENT, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("game"), source: wgpu::ShaderSource::Wgsl(include_str!("game.wgsl").into()) });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });

        let sprite_attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 8, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 16, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 3 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 48, shader_location: 4 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 64, shader_location: 5 },
        ];
        let vert_attrs = [wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 }];
        let inst_attrs: Vec<_> = (0..4u64).map(|i| wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: i * 16, shader_location: 1 + i as u32 }).collect();
        let color_target = |write: wgpu::ColorWrites| Some(wgpu::ColorTargetState { format, blend: Some(blend()), write_mask: write });
        let ds = |compare, write| Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(write), depth_compare: Some(compare), stencil: Default::default(), bias: Default::default() });

        let sprite_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprites"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sprite"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Instance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &sprite_attrs })],
            },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_sprite"), compilation_options: Default::default(), targets: &[color_target(wgpu::ColorWrites::ALL)] }),
            primitive: Default::default(),
            depth_stencil: ds(wgpu::CompareFunction::Always, false),
            multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            multiview_mask: None,
            cache: None,
        });
        let add_blend = {
            let c = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
            wgpu::BlendState { color: c, alpha: c }
        };
        let add_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprites additive"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sprite"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Instance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &sprite_attrs })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sprite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(add_blend), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: Default::default(),
            depth_stencil: ds(wgpu::CompareFunction::Always, false),
            multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            multiview_mask: None,
            cache: None,
        });
        let slider_buffers = [
            Some(wgpu::VertexBufferLayout { array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vert_attrs }),
            Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<SliderInst>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &inst_attrs }),
        ];
        let slider_vertex = || wgpu::VertexState { module: &shader, entry_point: Some("vs_slider"), compilation_options: Default::default(), buffers: &slider_buffers };
        let prepass_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("slider depth"),
            layout: Some(&layout),
            vertex: slider_vertex(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_slider"), compilation_options: Default::default(), targets: &[color_target(wgpu::ColorWrites::empty())] }),
            primitive: Default::default(),
            depth_stencil: ds(wgpu::CompareFunction::Less, true),
            multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            multiview_mask: None,
            cache: None,
        });
        let body_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("slider body"),
            layout: Some(&layout),
            vertex: slider_vertex(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_slider"), compilation_options: Default::default(), targets: &[color_target(wgpu::ColorWrites::ALL)] }),
            primitive: Default::default(),
            depth_stencil: ds(wgpu::CompareFunction::LessEqual, false),
            multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            multiview_mask: None,
            cache: None,
        });

        // all slider meshes in one static buffer
        let mut all: Vec<[f32; 3]> = Vec::new();
        let mut base = Vec::new();
        for g in geoms {
            base.push(all.len() as u32);
            if let Some(g) = g {
                all.extend_from_slice(&g.verts);
            }
        }
        let verts = device.create_buffer(&wgpu::BufferDescriptor { label: Some("slider verts"), size: (all.len().max(1) * 12) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        if !all.is_empty() {
            queue.write_buffer(&verts, 0, bytemuck::cast_slice(&all));
        }
        let inst_cap = 8192;
        let slider_cap = 256;
        let instances = device.create_buffer(&wgpu::BufferDescriptor { label: Some("instances"), size: (inst_cap * std::mem::size_of::<Instance>()) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let slider_insts = device.create_buffer(&wgpu::BufferDescriptor { label: Some("slider instances"), size: (slider_cap * std::mem::size_of::<SliderInst>()) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let depth = depth_tex(&device, size);
        let msaa = msaa_tex(&device, size, format);
        GameRenderer { device, queue, sprite_pipe, add_pipe, prepass_pipe, body_pipe, bind_group, globals, instances, inst_cap, slider_insts, slider_cap, verts, base, depth, msaa, format, scratch: Vec::new(), sinst: Vec::new(), sheet_size, bgl, sampler, size, scale, origin: [0.0, 0.0] }
    }

    /// Replace the skin sheet (a new follow-point animation was appended to it).
    pub fn set_sheet(&mut self, sheet: &[u8], sheet_size: (u32, u32)) {
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("skin sheet"),
            size: wgpu::Extent3d { width: sheet_size.0, height: sheet_size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            tex.as_image_copy(),
            sheet,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(sheet_size.0 * 4), rows_per_image: Some(sheet_size.1) },
            wgpu::Extent3d { width: sheet_size.0, height: sheet_size.1, depth_or_array_layers: 1 },
        );
        let view = tex.create_view(&Default::default());
        self.bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.sheet_size = sheet_size;
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Place the 640x480 view inside the target, centerd in the rectangle (x, y, w, h) in pixels, as large as fits.
    pub fn set_viewport(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.scale = (w / 640.0).min(h / 480.0).max(0.01);
        self.origin = [x + (w - 640.0 * self.scale) / 2.0, y + (h - 480.0 * self.scale) / 2.0];
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.size = size;
        self.depth = depth_tex(&self.device, size);
        self.msaa = msaa_tex(&self.device, size, self.format);
    }

    /// Draw `frame` into `view`. `geoms` must be the same list the renderer was built with.
    pub fn render(&mut self, view: &wgpu::TextureView, frame: &Frame, geoms: &[Option<SliderGeom>], clear: [f64; 4]) {
        let ss = self.scale;
        let g = Globals { viewport: [self.size.0 as f32, self.size.1 as f32], sheet: [self.sheet_size.0 as f32, self.sheet_size.1 as f32], scale: [ss, self.origin[0], self.origin[1], 0.0] };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
        self.scratch.clear();
        for s in &frame.sprites {
            let (tint, c) = match s.tint {
                Some(c) => (1.0, [c[0], c[1], c[2], 1.0]),
                None => (0.0, [0.0; 4]),
            };
            self.scratch.push(Instance {
                pos: [s.pos[0] * ss + self.origin[0], s.pos[1] * ss + self.origin[1]],
                half_size: [s.half[0] * ss, s.half[1] * ss],
                rot_alpha: [s.rot, s.alpha],
                uv: s.rect,
                color: c,
                params: [tint, 0.0, 0.0, 0.0],
                ..Default::default()
            });
        }
        let n = self.scratch.len().min(self.inst_cap);
        if n > 0 {
            self.queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.scratch[..n]));
        }
        self.sinst.clear();
        for s in &frame.sliders {
            self.sinst.push(SliderInst {
                border: [s.border[0], s.border[1], s.border[2], s.border_portion],
                outer: [s.outer[0], s.outer[1], s.outer[2], s.shadow_portion],
                inner: [s.inner[0], s.inner[1], s.inner[2], 1.0],
                misc: [s.alpha, s.body_alpha, s.z, s.z_range],
            });
        }
        let ns = self.sinst.len().min(self.slider_cap);
        if ns > 0 {
            self.queue.write_buffer(&self.slider_insts, 0, bytemuck::cast_slice(&self.sinst[..ns]));
        }
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa,
                    depth_slice: None,
                    resolve_target: Some(view),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: clear[3] }), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            for op in &frame.ops {
                match op {
                    Op::Sprites(r, additive) => {
                        let (a, b) = (r.start.min(n), r.end.min(n));
                        if b > a {
                            pass.set_pipeline(if *additive { &self.add_pipe } else { &self.sprite_pipe });
                            pass.set_vertex_buffer(0, self.instances.slice(..));
                            pass.draw(0..6, a as u32..b as u32);
                        }
                    }
                    Op::Slider(si) => {
                        if *si >= ns {
                            continue;
                        }
                        let s = &frame.sliders[*si];
                        let Some(g) = geoms.get(s.geom).and_then(|g| g.as_ref()) else { continue };
                        if s.alpha <= 0.0 || s.to < s.from || g.caps.is_empty() {
                            continue;
                        }
                        let last = s.to.min(g.caps.len() - 1);
                        let first = s.from.min(last);
                        let v0 = self.base[s.geom] + g.caps[first].start;
                        let v1 = self.base[s.geom] + g.caps[last].end;
                        pass.set_vertex_buffer(0, self.verts.slice(..));
                        pass.set_vertex_buffer(1, self.slider_insts.slice(..));
                        // the strip, then the spare caps where the body is trimmed to a point that has none in the strip
                        let mut ranges = vec![v0..v1];
                        for i in [first, last] {
                            if let Some(Some(x)) = g.extra.get(i) {
                                ranges.push(self.base[s.geom] + x.start..self.base[s.geom] + x.end);
                            }
                        }
                        for pipe in [&self.prepass_pipe, &self.body_pipe] {
                            pass.set_pipeline(pipe);
                            for r in &ranges {
                                pass.draw(r.clone(), *si as u32..*si as u32 + 1);
                            }
                        }
                    }
                }
            }
        }
        self.queue.submit(Some(enc.finish()));
    }
}

extern crate pigeon_2d as pigeon;
extern crate pigeon_parrot as parrot;
use egui::{
    epaint::{ImageDelta, Vertex},
    ImageData, TextureId,
};
use euclid::{Point2D, Rect, Size2D};
use parrot::{
    binding::{Binding, BindingGroup, BindingType},
    pipeline::{Blending, PipelineCore, PipelineDescription, Set},
    transform::*,
    vertex::VertexFormat,
    buffers::index::IndexBuffer32, Painter, Plumber, RenderPassExtention, Rgba8, Sampler, Texture, UniformBuffer,
    VertexBuffer,
};
use pigeon::pigeon::Container;
use std::{collections::HashMap, ops::Deref};

pub const VERTEX_LAYOUT: [VertexFormat; 3] = [
    VertexFormat::Floatx2,
    VertexFormat::Floatx2,
    VertexFormat::Uint32,
];

#[allow(dead_code)]
pub struct CallbackFn {
    add: Box<AddCallback>,
    draw: Box<PaintCallback>,
}
type AddCallback = dyn Fn(&mut Container) + Send + Sync;
type PaintCallback = dyn Fn(&mut Container) + Send + Sync;

impl Default for CallbackFn {
    fn default() -> Self {
        Self {
            add: Box::new(|_| ()),
            draw: Box::new(|_| ()),
        }
    }
}

/// Information about the screen used for rendering.
pub struct ScreenDescriptor {
    /// Size of the window in physical pixels.
    pub size_in_pixels: [u32; 2],

    /// HiDPI scale factor (pixels per point).
    pub pixels_per_point: f32,
}

impl ScreenDescriptor {
    fn screen_size_in_points(&self) -> [f32; 2] {
        [
            self.size_in_pixels[0] as f32 / self.pixels_per_point,
            self.size_in_pixels[1] as f32 / self.pixels_per_point,
        ]
    }
}

/// Helps [`EguiPipe`] know which texture to set depending on how many indices deep it is in the buffer
#[derive(Debug)]
pub struct Group {
    range: std::ops::Range<u32>,
    tex_id: TextureId,
    pixel_rect: Rect<u32, ScreenSpace>,
}

/// Pipeline for egui
#[derive(Debug)]
pub struct EguiPipe {
    /// Vertex buffer
    pub vertex_buffer: VertexBuffer,
    /// Index bufer
    pub index_buffer: IndexBuffer32,
    /// Egui textures
    pub egui_texture: HashMap<egui::TextureId, (Texture, BindingGroup)>,
    /// Groups
    pub groups: Vec<Group>,
    /// Sampler used by egui textures
    pub sampler: Sampler,
    /// Container to hold shapes to be drawn with paint callback
    pub container: Option<Container>,
    /// core
    pub core: PipelineCore,
}

/// Uniform buffer for rendering
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable, Default)]
pub struct Uniform {
    screen_size_in_points: [f32; 2],
    // padding as uniform buffers must be at least 16 bytes
    _padding: [u32; 2],
}

impl Deref for EguiPipe {
    type Target = PipelineCore;

    fn deref(&self) -> &Self::Target {
        &self.core
    }
}

impl<'a> Plumber<'a> for EguiPipe {
    type PrepareContext = (
        egui::TexturesDelta,
        Vec<egui::ClippedPrimitive>,
        ScreenDescriptor,
    );
    type Uniforms = Uniform;

    fn description() -> parrot::pipeline::PipelineDescription<'a> {
        PipelineDescription {
            vertex_layout: &VERTEX_LAYOUT,
            pipeline_layout: Some(&[
                Set(
                    &[Binding {
                        binding: BindingType::UniformBuffer,
                        stage: wgpu::ShaderStages::VERTEX,
                    }],
                    Some("Egui screen bind group"),
                ),
                Set(
                    &[
                        Binding {
                            binding: BindingType::Texture {
                                multisampled: false,
                            },
                            stage: wgpu::ShaderStages::FRAGMENT,
                        },
                        Binding {
                            binding: BindingType::Sampler,
                            stage: wgpu::ShaderStages::FRAGMENT,
                        },
                    ],
                    Some("Egui texture bind group"),
                ),
            ]),
            shader: parrot::shader::ShaderFile::Wgsl(include_str!("./egui_2.wgsl")),
            name: Some("Egui pipe"),
        }
    }

    fn setup(pipe: parrot::pipeline::Pipeline, paint: &parrot::Painter) -> Self {
        let vertex_buffer =
            paint.vertex_buffer::<egui::epaint::Vertex>(&[], Some("Egui vertex buffer"));
        let index_buffer = paint.index_buffer_32(&[], Some("Egui index buffer"));
        let egui_texture = HashMap::new();
        let uniform_buffer =
            paint.uniform_buffer(&[Uniform::default()], Some("Egui uniform buffer"));
        let sampler = paint.sampler(
            wgpu::FilterMode::Nearest,
            wgpu::FilterMode::Linear,
            Some("Egui texture sampler"),
        );
        let binding = paint.binding_group(
            &pipe.layout.b_layouts[0],
            &[&uniform_buffer],
            Some("Egui uniform binding group"),
        );
        let core = PipelineCore {
            pipeline: pipe,
            bindings: vec![binding],
            uniforms: vec![uniform_buffer],
        };
        let container = None;

        Self {
            vertex_buffer,
            index_buffer,
            egui_texture,
            groups: vec![],
            sampler,
            container,
            core,
        }
    }

    fn prepare(
        &'a mut self,
        context: Self::PrepareContext,
        paint: &mut parrot::Painter,
    ) -> Vec<(&'a mut UniformBuffer, Vec<Self::Uniforms>)> {
        let mut vertices: Vec<egui::epaint::Vertex> = vec![];
        let mut indices: Vec<u32> = vec![];
        let mut groups: Vec<Group> = vec![];
        let pixels_per_point = context.2.pixels_per_point;
        let size_in_pixels = context.2.size_in_pixels;
        for primative in context.1 {
            match primative.primitive {
                egui::epaint::Primitive::Callback(_) => {
                    log::warn!("Callback not implemented");
                    continue;
                }
                egui::epaint::Primitive::Mesh(mut mesh) => {
                    let si = indices.len() as u32;
                    let si2 = vertices.len() as u32;

                    // Get clipping rect
                    let pixel_rect = calculate_pixel_rect(
                        &primative.clip_rect,
                        pixels_per_point,
                        size_in_pixels,
                    );
                    indices.append(&mut mesh.indices.iter().map(|i| i + si2 as u32).collect());
                    vertices.append(&mut mesh.vertices);
                    groups.push(Group {
                        range: si..indices.len() as u32,
                        tex_id: mesh.texture_id,
                        pixel_rect,
                    });
                }
            }
        }

        // Update buffers
        if let Some(v) = paint.update_vertex_buffer(vertices.as_slice(), &mut self.vertex_buffer) {
            self.vertex_buffer = v;
        }
        if let Some(i) = paint.update_index_buffer_32(indices, &mut self.index_buffer) {
            self.index_buffer = i;
        }
        self.groups = groups;

        for set in context.0.set {
            // Function to get color from image delta
            let set_data = |d: ImageDelta| -> Vec<egui::Color32> {
                match d {
                    ImageDelta {
                        image: ImageData::Color(c),
                        ..
                    } => {
                        let data = c.pixels;
                        data
                    }
                    ImageDelta {
                        image: ImageData::Font(f),
                        ..
                    } => {
                        let data = f.srgba_pixels(1f32).collect::<Vec<egui::Color32>>();
                        data
                    }
                }
            };

            if let Some(t) = self.egui_texture.get_mut(&set.0) {
                if let Some(pos) = set.1.pos {
                    // Fill part of the texture
                    let (width, height) = (set.1.image.width() as u32, set.1.image.height() as u32);
                    let b = set_data(set.1);
                    let data = Rgba8::align(b.as_slice());
                    Texture::transfer(
                        &t.0,
                        data,
                        Rect::new(
                            Point2D::new(pos[0] as u32, pos[1] as u32),
                            Size2D::new(width, height),
                        ),
                        &paint.device,
                    );
                } else {
                    // Fill whole texture
                    let b = set_data(set.1);
                    let data = Rgba8::align(b.as_slice());
                    Texture::fill(&t.0, data, &paint.device);
                }
            } else {
                // Create new texture
                let size: Size2D<u32, ScreenSpace> =
                    Size2D::new(set.1.image.width() as u32, set.1.image.height() as u32);
                let b = set_data(set.1);
                let data = Rgba8::align(b.as_slice());
                let tex = paint.texture(
                    size,
                    wgpu::TextureFormat::Bgra8UnormSrgb,
                    wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    Some(format!("Egui texture {:?}", set.0).as_str()),
                    false,
                );
                let binding = paint.binding_group(
                    &self.core.pipeline.layout.b_layouts[1],
                    &[&tex, &self.sampler],
                    Some(format!("Egui texture {:?} binding group", set.0).as_str()),
                );
                Texture::fill(&tex, data, &paint.device);
                self.egui_texture.insert(set.0, (tex, binding));
            }
        }

        // Create and update uniform
        let uniform = Uniform {
            screen_size_in_points: context.2.screen_size_in_points(),
            _padding: Default::default(),
        };
        vec![(&mut self.core.uniforms[0], vec![uniform])]
    }
}

impl pigeon_2d::pipeline::Render for EguiPipe {
    type Vertex = Vertex;

    fn render<'a>(&'a mut self, _paint: &mut parrot::Painter, pass: &mut wgpu::RenderPass<'a>) {
        // Set pipeline
        pass.set_parrot_pipeline(self);

        // Set buffers
        pass.set_parrot_vertex_buffer(&self.vertex_buffer);
        pass.set_parrot_index_buffer_32(&self.index_buffer);

        for group in &self.groups {
            if !group.pixel_rect.is_empty() {
                if let Some(binding) = self.egui_texture.get(&group.tex_id) {
                    pass.set_binding(&binding.1, &[]);
                } else {
                    log::warn!("Unknown texture >> {:?}", group.tex_id);
                }

                // Set scissor rect
                pass.set_scissor_rect(
                    group.pixel_rect.origin.x,
                    group.pixel_rect.origin.y,
                    group.pixel_rect.width(),
                    group.pixel_rect.height(),
                );
                pass.draw_parrot_indexed(group.range.clone(), 0..1);
            }
        }
    }
}

pub fn setup(paint: &Painter) -> EguiPipe {
    paint.pipeline_no_depth(
        Blending::default(),
        wgpu::TextureFormat::Bgra8UnormSrgb,
        Some("Egui shader"),
    )
}

// Convert egui clip rect to a physical pixel rect
fn calculate_pixel_rect(
    clip_rect: &egui::Rect,
    pixels_per_point: f32,
    target_size: [u32; 2],
) -> Rect<u32, ScreenSpace> {
    // Transform to physical pixels
    let clip_min_x = pixels_per_point * clip_rect.min.x;
    let clip_min_y = pixels_per_point * clip_rect.min.y;
    let clip_max_x = pixels_per_point * clip_rect.max.x;
    let clip_max_y = pixels_per_point * clip_rect.max.y;

    // Make sure clip rect can fit within an `u32`.
    let clip_min_x = clip_min_x.clamp(0.0, target_size[0] as f32);
    let clip_min_y = clip_min_y.clamp(0.0, target_size[1] as f32);
    let clip_max_x = clip_max_x.clamp(clip_min_x, target_size[0] as f32);
    let clip_max_y = clip_max_y.clamp(clip_min_y, target_size[1] as f32);

    let clip_min_x = clip_min_x.round() as u32;
    let clip_min_y = clip_min_y.round() as u32;
    let clip_max_x = clip_max_x.round() as u32;
    let clip_max_y = clip_max_y.round() as u32;

    let width = (clip_max_x - clip_min_x).max(1);
    let height = (clip_max_y - clip_min_y).max(1);

    // Clip scissor rectangle to target size.
    let x = clip_min_x.min(target_size[0]);
    let y = clip_min_y.min(target_size[1]);
    let width = width.min(target_size[0] - x);
    let height = height.min(target_size[1] - y);

    Rect::new(Point2D::new(x, y), Size2D::new(width, height))
}

use parrot::{device::Device, pipeline::PipelineLayout, vertex::VertexLayout};
use wgpu::{MultisampleState, ShaderModule};
// Custom pipeline function
pub fn egui_parrot_pipeline(
    dev: &Device,
    pipe_l: PipelineLayout,
    vert_l: VertexLayout,
    shader: ShaderModule,
    multi: MultisampleState,
    name: Option<&str>,
) -> parrot::pipeline::Pipeline {
    let vert_attrs = vert_l.to_wgpu();

    let mut b_layouts = Vec::new();

    for s in pipe_l.b_layouts.iter() {
        b_layouts.push(&s.wgpu);
    }

    let layout = &dev
        .wgpu
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: name,
            bind_group_layouts: b_layouts.as_slice(),
            push_constant_ranges: &[],
        });

    let (src_factor, dst_factor, operation) = parrot::pipeline::Blending::default().as_wgpu();
    let targets = [Some(wgpu::ColorTargetState {
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        blend: Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor,
                dst_factor,
                operation,
            },
            alpha: wgpu::BlendComponent {
                src_factor,
                dst_factor,
                operation,
            },
        }),
        write_mask: wgpu::ColorWrites::ALL,
    })];

    let desc = wgpu::RenderPipelineDescriptor {
        label: name,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            buffers: &[vert_attrs],
        },
        layout: Some(layout),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            unclipped_depth: false,
            conservative: false,
            cull_mode: None,
            front_face: wgpu::FrontFace::default(),
            polygon_mode: wgpu::PolygonMode::default(),
            strip_index_format: None,
        },
        depth_stencil: None,
        multisample: multi,
        multiview: None,
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            targets: &targets,
        }),
    };

    let wgpu = dev.wgpu.create_render_pipeline(&desc);

    parrot::pipeline::Pipeline {
        wgpu,
        layout: pipe_l,
        vertex_layout: vert_l,
    }
}

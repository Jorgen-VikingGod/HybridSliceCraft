//! Draws resin layers on the graphics card.
//!
//! Each outline segment becomes a triangle fanned from a fixed point. Counting
//! triangles up and down in the stencil buffer gives the winding number at every
//! sample, so the fill matches the software rasteriser with no triangulation.
//! Only the outline at each height is sent, so cost follows the outline length
//! rather than the size of the model.

use super::raster::{encode_into, LayerImage, RasterSpec, RleWriter};
use egui_wgpu::wgpu;

const SHADER: &str = r#"
struct Tile { scale: vec2<f32>, offset: vec2<f32> };
@group(0) @binding(0) var<uniform> tile: Tile;

@vertex
fn vs_fan(@location(0) p: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(p * tile.scale + tile.offset, 0.0, 1.0);
}

@vertex
fn vs_cover(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i & 2u) * 2 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment fn fs_none() -> @location(0) vec4<f32> { return vec4<f32>(0.0); }
@fragment fn fs_white() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
@fragment fn fs_black() -> @location(0) vec4<f32> { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }
"#;

const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
const STENCIL: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// Handle to the graphics card for slicing, safe to use from a worker thread.
#[derive(Clone)]
pub struct GpuRaster {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub name: String,
    max_tile: u32,
    msaa4: bool,
    is_software: bool,
}

/// Textures and pipelines sized for one printer screen.
pub struct RasterState {
    samples: u32,
    tile_w: u32,
    tile_h: u32,
    tiles: Vec<(u32, u32)>,
    multi_view: Option<wgpu::TextureView>,
    resolve_tex: wgpu::Texture,
    resolve_view: wgpu::TextureView,
    stencil_view: wgpu::TextureView,
    fan: wgpu::RenderPipeline,
    cover_white: wgpu::RenderPipeline,
    cover_black: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    vertex_capacity: u64,
    readback: wgpu::Buffer,
    bytes_per_row: u32,
    scratch: Vec<u8>,
}

impl GpuRaster {
    pub fn new(adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let info = adapter.get_info();
        let flags = adapter.get_texture_format_features(COLOR).flags;
        let stencil_flags = adapter.get_texture_format_features(STENCIL).flags;
        let msaa4 = flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4)
            && flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE)
            && stencil_flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4);
        Self {
            device: device.clone(),
            queue: queue.clone(),
            name: info.name,
            max_tile: device.limits().max_texture_dimension_2d.max(1024),
            msaa4,
            is_software: info.device_type == wgpu::DeviceType::Cpu,
        }
    }

    /// Graphics cards win for mono and four-times smoothing. Eight-times and
    /// software renderers go through the processor path instead.
    pub fn suits(&self, spec: &RasterSpec) -> bool {
        if self.is_software {
            return false;
        }
        match spec.samples {
            1 => true,
            2 | 4 => self.msaa4,
            _ => false,
        }
    }

    pub fn begin(&self, spec: &RasterSpec) -> Result<RasterState, String> {
        let samples = if spec.samples > 1 { 4 } else { 1 };
        let tile_w = spec.width.min(self.max_tile);
        let tile_h = spec.height.min(self.max_tile);
        let mut tiles = Vec::new();
        let mut y = 0;
        while y < spec.height {
            let mut x = 0;
            while x < spec.width {
                tiles.push((x, y));
                x += tile_w;
            }
            y += tile_h;
        }
        let dev = &self.device;
        let size = wgpu::Extent3d { width: tile_w, height: tile_h, depth_or_array_layers: 1 };
        let tex = |label, format, samples, usage| {
            dev.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let resolve_tex = tex("layer", COLOR, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let resolve_view = resolve_tex.create_view(&Default::default());
        let multi_view = (samples > 1).then(|| {
            tex("layer-msaa", COLOR, samples, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default())
        });
        let stencil_view = tex("layer-stencil", STENCIL, samples, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default());

        let shader = dev.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("layer"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniform = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tile"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tile"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind = dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tile"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let pipe_layout = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layer"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let multisample = wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false };
        let keep = |compare, pass_op| wgpu::StencilFaceState {
            compare,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op,
        };
        let vertex_layout = [Some(wgpu::VertexBufferLayout {
            array_stride: 8,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        })];
        let fan = dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fan"),
            layout: Some(&pipe_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fan"),
                buffers: &vertex_layout,
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: STENCIL,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState {
                    front: keep(wgpu::CompareFunction::Always, wgpu::StencilOperation::IncrementWrap),
                    back: keep(wgpu::CompareFunction::Always, wgpu::StencilOperation::DecrementWrap),
                    read_mask: 0xff,
                    write_mask: 0xff,
                },
                bias: Default::default(),
            }),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_none"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let cover = |entry: &str| {
            dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("cover"),
                layout: Some(&pipe_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_cover"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: STENCIL,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: wgpu::StencilState {
                        front: keep(wgpu::CompareFunction::NotEqual, wgpu::StencilOperation::Keep),
                        back: keep(wgpu::CompareFunction::NotEqual, wgpu::StencilOperation::Keep),
                        read_mask: 0xff,
                        write_mask: 0,
                    },
                    bias: Default::default(),
                }),
                multisample,
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let cover_white = cover("fs_white");
        let cover_black = cover("fs_black");
        let vertex_capacity = 1 << 20;
        let vertices = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fan vertices"),
            size: vertex_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bytes_per_row = tile_w.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layer readback"),
            size: bytes_per_row as u64 * tile_h as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let scratch = if tiles.len() > 1 {
            vec![0u8; spec.width as usize * spec.height as usize]
        } else {
            Vec::new()
        };
        Ok(RasterState {
            samples,
            tile_w,
            tile_h,
            tiles,
            multi_view,
            resolve_tex,
            resolve_view,
            stencil_view,
            fan,
            cover_white,
            cover_black,
            bind,
            uniform,
            vertices,
            vertex_capacity,
            readback,
            bytes_per_row,
            scratch,
        })
    }

    pub fn render(&self, st: &mut RasterState, pos: &[[f32; 4]], neg: &[[f32; 4]], spec: &RasterSpec) -> Result<LayerImage, String> {
        let mut writer = RleWriter::new(spec.width);
        if pos.is_empty() {
            writer.push_run(0, spec.width as u64 * spec.height as u64);
            return Ok(writer.finish());
        }
        // Fan vertices for both groups, in pixels.
        let mut verts: Vec<[f32; 2]> = Vec::with_capacity((pos.len() + neg.len()) * 3);
        let origin = [0.0f32, 0.0];
        for s in pos.iter().chain(neg.iter()) {
            let (ax, ay) = spec.to_px(s[0], s[1]);
            let (bx, by) = spec.to_px(s[2], s[3]);
            verts.push(origin);
            verts.push([ax, ay]);
            verts.push([bx, by]);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&verts);
        if bytes.len() as u64 > st.vertex_capacity {
            st.vertex_capacity = (bytes.len() as u64).next_power_of_two();
            st.vertices = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fan vertices"),
                size: st.vertex_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue.write_buffer(&st.vertices, 0, bytes);
        let pos_verts = pos.len() as u32 * 3;
        let total_verts = verts.len() as u32;

        for t in 0..st.tiles.len() {
            let (tx, ty) = st.tiles[t];
            let scale = [2.0 / st.tile_w as f32, -2.0 / st.tile_h as f32];
            let offset = [-1.0 - tx as f32 * scale[0], 1.0 - ty as f32 * scale[1]];
            self.queue.write_buffer(&st.uniform, 0, bytemuck::cast_slice(&[scale[0], scale[1], offset[0], offset[1]]));
            let mut enc = self.device.create_command_encoder(&Default::default());
            let (color_view, resolve) = match &st.multi_view {
                Some(m) => (m, Some(&st.resolve_view)),
                None => (&st.resolve_view, None),
            };
            let stencil_attach = wgpu::RenderPassDepthStencilAttachment {
                view: &st.stencil_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
            };
            {
                let has_neg = !neg.is_empty();
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("layer solid"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: color_view,
                        depth_slice: None,
                        resolve_target: if has_neg { None } else { resolve },
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(stencil_attach.clone()),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_bind_group(0, &st.bind, &[]);
                pass.set_vertex_buffer(0, st.vertices.slice(..));
                pass.set_pipeline(&st.fan);
                pass.set_stencil_reference(0);
                pass.draw(0..pos_verts, 0..1);
                pass.set_pipeline(&st.cover_white);
                pass.draw(0..3, 0..1);
            }
            if !neg.is_empty() {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("layer holes"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: color_view,
                        depth_slice: None,
                        resolve_target: resolve,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(stencil_attach),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_bind_group(0, &st.bind, &[]);
                pass.set_vertex_buffer(0, st.vertices.slice(..));
                pass.set_pipeline(&st.fan);
                pass.set_stencil_reference(0);
                pass.draw(pos_verts..total_verts, 0..1);
                pass.set_pipeline(&st.cover_black);
                pass.draw(0..3, 0..1);
            }
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &st.resolve_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &st.readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(st.bytes_per_row),
                        rows_per_image: Some(st.tile_h),
                    },
                },
                wgpu::Extent3d { width: st.tile_w, height: st.tile_h, depth_or_array_layers: 1 },
            );
            self.queue.submit([enc.finish()]);
            let slice = st.readback.slice(..);
            let (tx_done, rx_done) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx_done.send(r);
            });
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|e| format!("the graphics card stopped responding: {e}"))?;
            match rx_done.recv() {
                Ok(Ok(())) => {}
                _ => return Err("couldn't read a layer back from the graphics card".into()),
            }
            {
                let data = slice.get_mapped_range().map_err(|e| format!("couldn't read a layer from the graphics card: {e:?}"))?;
                let bpr = st.bytes_per_row as usize;
                let tw = st.tile_w as usize;
                if st.tiles.len() == 1 {
                    for row in 0..st.tile_h as usize {
                        let r = &data[row * bpr..row * bpr + tw];
                        encode_into(&mut writer, &r[..(spec.width as usize).min(tw)]);
                    }
                } else {
                    let w = spec.width as usize;
                    for row in 0..st.tile_h as usize {
                        let dy = ty as usize + row;
                        if dy >= spec.height as usize {
                            break;
                        }
                        let width = tw.min(w - tx as usize);
                        let at = dy * w + tx as usize;
                        st.scratch[at..at + width].copy_from_slice(&data[row * bpr..row * bpr + width]);
                    }
                }
            }
            st.readback.unmap();
        }
        if st.tiles.len() > 1 {
            encode_into(&mut writer, &st.scratch);
        }
        let _ = st.samples;
        Ok(writer.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resin::raster::raster_cpu;

    fn device() -> Option<GpuRaster> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        // A software adapter (build machines without a graphics card) proves nothing
        // about the card path, and can crash; leave these tests out there.
        if adapter.get_info().device_type == wgpu::DeviceType::Cpu {
            return None;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .ok()?;
        Some(GpuRaster::new(&adapter, &device, &queue))
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 4]> {
        vec![
            [x0, y0, x1, y0],
            [x1, y0, x1, y1],
            [x1, y1, x0, y1],
            [x0, y1, x0, y0],
        ]
    }

    #[test]
    fn card_and_software_agree() {
        let Some(g) = device() else {
            eprintln!("no real graphics card here; skipping");
            return;
        };
        for samples in [1u32, 4] {
            let spec = RasterSpec {
                width: 640,
                height: 400,
                size_x: 64.0,
                size_y: 40.0,
                samples,
                mirror_x: false,
                mirror_y: false,
            };
            let mut st = g.begin(&spec).unwrap();
            // Outer shape, hole in it, overlap, and a drain cut.
            let mut segs = rect(10.0, 5.0, 50.0, 35.0);
            segs.extend(rect(20.0, 12.0, 40.0, 28.0).iter().map(|s| [s[2], s[3], s[0], s[1]]));
            segs.extend(rect(5.0, 15.0, 15.0, 25.0));
            let neg = rect(44.0, 18.0, 47.0, 22.0);
            let a = g.render(&mut st, &segs, &neg, &spec).unwrap();
            let b = raster_cpu(&segs, &neg, &spec);
            assert!((a.lit - b.lit).abs() / b.lit < 0.01, "samples {samples}: {} vs {}", a.lit, b.lit);
            let (mut ba, mut bb) = (vec![0u8; 256000], vec![0u8; 256000]);
            a.decode(&mut ba);
            b.decode(&mut bb);
            let differ = ba.iter().zip(&bb).filter(|(x, y)| (**x as i32 - **y as i32).abs() > 128).count();
            assert!(differ < 40, "samples {samples}: {differ} pixels disagree");
            let empty = g.render(&mut st, &[], &[], &spec).unwrap();
            assert_eq!(empty.lit, 0.0);
        }
    }

    #[test]
    fn tiles_cover_very_large_screens() {
        let Some(mut g) = device() else { return };
        g.max_tile = 256;
        let spec = RasterSpec {
            width: 640,
            height: 400,
            size_x: 64.0,
            size_y: 40.0,
            samples: 1,
            mirror_x: false,
            mirror_y: false,
        };
        let mut st = g.begin(&spec).unwrap();
        assert!(st.tiles.len() > 1);
        let segs = rect(10.0, 5.0, 50.0, 35.0);
        let a = g.render(&mut st, &segs, &[], &spec).unwrap();
        let b = raster_cpu(&segs, &[], &spec);
        assert!((a.lit - b.lit).abs() / b.lit < 0.01, "{} vs {}", a.lit, b.lit);
    }
}

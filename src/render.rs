//! The 3D view, drawn directly on the graphics card into an off-screen picture
//! with its own depth buffer and edge smoothing, then shown inside the window.

use ahash::AHashMap;
use egui_wgpu::wgpu;
use glam::{Mat4, Vec2, Vec3};
use wgpu::util::DeviceExt;

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;
/// Each block of toolpath data stays under common buffer limits.
const SEGS_PER_CHUNK: usize = 3_000_000;

const SHADER: &str = r#"
struct Globals {
    vp: mat4x4<f32>,
    eye: vec4<f32>,
    clip: vec4<f32>,      // x: height, y: enabled, z: overhang cosine
    bed: vec4<f32>,       // x, y, round, unused
    accent: vec4<f32>,
    mask: vec4<u32>,
};
@group(0) @binding(0) var<uniform> g: Globals;

fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    return pow(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2));
}
fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

fn shade(base: vec3<f32>, n: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    let key = normalize(vec3<f32>(-0.35, -0.55, 0.76));
    let fill = normalize(vec3<f32>(0.7, 0.4, 0.25));
    let sky = mix(vec3<f32>(0.20, 0.20, 0.23), vec3<f32>(0.46, 0.49, 0.54), n.z * 0.5 + 0.5);
    let diff = max(dot(n, key), 0.0) * 0.72 + max(dot(n, fill), 0.0) * 0.2;
    let h = normalize(key + v);
    let spec = pow(max(dot(n, h), 0.0), 48.0) * 0.16;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0) * 0.08;
    return base * (sky + vec3<f32>(diff)) + vec3<f32>(spec + rim);
}

// ---- background
@vertex
fn vs_back(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i & 2u) * 2 - 1);
    return vec4<f32>(x, y, 1.0, 1.0);
}
@fragment
fn fs_back(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let t = clamp(p.y / max(g.bed.w, 1.0), 0.0, 1.0);
    let top = vec3<f32>(0.105, 0.118, 0.142);
    let bottom = vec3<f32>(0.058, 0.066, 0.082);
    return vec4<f32>(mix(top, bottom, t), 1.0);
}

// ---- models
struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) flags: vec4<f32>,
};
@vertex
fn vs_mesh(
    @location(0) pos: vec3<f32>,
    @location(1) m0: vec4<f32>,
    @location(2) m1: vec4<f32>,
    @location(3) m2: vec4<f32>,
    @location(4) m3: vec4<f32>,
    @location(5) color: vec4<f32>,
    @location(6) flags: vec4<f32>,
) -> MeshOut {
    var out: MeshOut;
    let world = mat4x4<f32>(m0, m1, m2, m3) * vec4<f32>(pos, 1.0);
    out.world = world.xyz;
    out.clip = g.vp * world;
    out.color = color;
    out.flags = flags;
    return out;
}
@fragment
fn fs_mesh(in: MeshOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (g.clip.y > 0.5 && in.world.z > g.clip.x) {
        discard;
    }
    let v = normalize(g.eye.xyz - in.world);
    var n = normalize(cross(dpdx(in.world), dpdy(in.world)));
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    var facing_out = front;
    if (in.flags.z > 0.5) {
        facing_out = !front;
    }
    if (!facing_out) {
        // Looking into the model through the cut: draw a flat solid cap.
        let cap = mix(to_linear(in.color.rgb), to_linear(g.accent.rgb), 0.55);
        return vec4<f32>(to_gamma(cap * 0.85), in.color.a);
    }
    var base = to_linear(in.color.rgb);
    if (in.flags.y > 0.5 && n.z < -g.clip.z) {
        // Surfaces facing down too steeply show a warm warning colour.
        let strength = clamp((-n.z - g.clip.z) / 0.25 + 0.35, 0.35, 1.0);
        base = mix(base, vec3<f32>(0.85, 0.16, 0.10), strength);
    }
    var lit = shade(base, n, v);
    if (in.flags.x > 0.5) {
        let edge = pow(1.0 - max(dot(n, v), 0.0), 2.0);
        lit = lit * 1.04 + to_linear(g.accent.rgb) * (0.015 + edge * 0.22);
    }
    return vec4<f32>(to_gamma(lit), in.color.a);
}

// ---- the plate
struct BedOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) xy: vec2<f32>,
};
@vertex
fn vs_bed(@builtin(vertex_index) i: u32) -> BedOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let xy = corners[i] * g.bed.xy;
    var out: BedOut;
    out.xy = xy;
    out.clip = g.vp * vec4<f32>(xy, 0.0, 1.0);
    return out;
}
fn grid_line(x: f32, step: f32) -> f32 {
    let d = abs(fract(x / step - 0.5) - 0.5) * step;
    let w = fwidth(x) * 0.9;
    return 1.0 - smoothstep(0.0, w, d);
}
@fragment
fn fs_bed(in: BedOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let size = g.bed.xy;
    var dist_edge = min(min(in.xy.x, in.xy.y), min(size.x - in.xy.x, size.y - in.xy.y));
    if (g.bed.z > 0.5) {
        let r = size.x * 0.5;
        let d = length(in.xy - size * 0.5);
        if (d > r) {
            discard;
        }
        dist_edge = r - d;
    }
    var c = vec3<f32>(0.135, 0.150, 0.178);
    let tick = max(grid_line(in.xy.x, 10.0), grid_line(in.xy.y, 10.0));
    let major = max(grid_line(in.xy.x, 50.0), grid_line(in.xy.y, 50.0));
    c = mix(c, vec3<f32>(0.205, 0.225, 0.265), tick * 0.8);
    c = mix(c, vec3<f32>(0.33, 0.37, 0.43), major * 0.9);
    let edge_w = fwidth(in.xy.x) * 1.5 + 0.25;
    let edge = 1.0 - smoothstep(0.0, edge_w, dist_edge);
    c = mix(c, g.accent.rgb, edge);
    var a = 0.96;
    if (!front) {
        a = 0.35;
    }
    return vec4<f32>(c, a);
}

// ---- lines
struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};
@vertex
fn vs_line(@location(0) pos: vec3<f32>, @location(1) color: vec4<f32>) -> LineOut {
    var out: LineOut;
    out.clip = g.vp * vec4<f32>(pos, 1.0);
    out.color = color;
    return out;
}
@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return in.color;
}

// ---- toolpaths: every segment is one instance of a small diamond prism.
struct PathOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};
@vertex
fn vs_path(
    @location(0) shape: vec3<f32>,
    @location(1) shape_n: vec3<f32>,
    @location(2) a: vec4<f32>,
    @location(3) b: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) feature: u32,
) -> PathOut {
    var out: PathOut;
    let on = (g.mask[feature / 32u] >> (feature % 32u)) & 1u;
    if (on == 0u) {
        out.clip = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        return out;
    }
    let delta = b.xyz - a.xyz;
    let len = max(length(delta), 1e-5);
    let dir = delta / len;
    var side = cross(dir, vec3<f32>(0.0, 0.0, 1.0));
    if (dot(side, side) < 1e-5) {
        side = vec3<f32>(1.0, 0.0, 0.0);
    }
    side = normalize(side);
    let up = cross(side, dir);
    let w = a.w;
    let h = b.w;
    let ext = min(w * 0.35, len);
    let along = -ext + shape.x * (len + 2.0 * ext);
    let world = a.xyz + dir * along + side * (shape.y * w * 0.5) + up * (shape.z * h * 0.5) - vec3<f32>(0.0, 0.0, h * 0.5);
    out.world = world;
    out.clip = g.vp * vec4<f32>(world, 1.0);
    out.normal = dir * shape_n.x + side * shape_n.y * h + up * shape_n.z * w;
    out.color = color.rgb;
    return out;
}
@fragment
fn fs_path(in: PathOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let v = normalize(g.eye.xyz - in.world);
    let key = normalize(vec3<f32>(-0.35, -0.55, 0.76));
    let l = 0.5 + 0.42 * max(dot(n, key), 0.0) + 0.14 * max(n.z, 0.0);
    let spec = pow(max(dot(n, normalize(key + v)), 0.0), 32.0) * 0.12;
    return vec4<f32>(in.color * l + vec3<f32>(spec), 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    vp: [[f32; 4]; 4],
    eye: [f32; 4],
    clip: [f32; 4],
    bed: [f32; 4],
    accent: [f32; 4],
    mask: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    flags: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuSeg {
    pub a: [f32; 3],
    pub width: f32,
    pub b: [f32; 3],
    pub height: f32,
    pub color: [u8; 4],
    pub feature: u32,
}

#[derive(Clone, Copy)]
pub struct Camera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub aspect: f32,
}

impl Camera {
    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cp * sy, -cp * cy, sp) * self.dist
    }
    pub fn view_proj(&self) -> Mat4 {
        let near = (self.dist * 0.01).max(0.5);
        let far = self.dist * 6.0 + 2000.0;
        let up = if self.pitch.abs() > 1.55 { Vec3::Y } else { Vec3::Z };
        Mat4::perspective_rh(32f32.to_radians(), self.aspect.max(0.1), near, far)
            * Mat4::look_at_rh(self.eye(), self.target, up)
    }
    /// A ray from the camera through a point given in -1..1 screen units.
    pub fn ray(&self, ndc: Vec2) -> (Vec3, Vec3) {
        let inv = self.view_proj().inverse();
        let near = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.0));
        let far = inv.project_point3(Vec3::new(ndc.x, ndc.y, 1.0));
        (near, (far - near).normalize_or_zero())
    }
    /// World point to -1..1 screen units; None when behind the camera.
    pub fn project(&self, p: Vec3) -> Option<Vec2> {
        let c = self.view_proj() * p.extend(1.0);
        (c.w > 1e-4).then(|| Vec2::new(c.x / c.w, c.y / c.w))
    }
    /// Pan so the view keeps its size in the window.
    pub fn pan(&mut self, dx: f32, dy: f32, view_h: f32) {
        let scale = self.dist * 2.0 * 16f32.to_radians().tan() / view_h.max(1.0);
        let (sy, cy) = self.yaw.sin_cos();
        let right = Vec3::new(cy, sy, 0.0);
        let (sp, cp) = self.pitch.sin_cos();
        let up = Vec3::new(-sp * sy, sp * cy, cp);
        self.target += (-right * dx + up * dy) * scale;
    }
}

pub struct DrawObject {
    pub mesh_id: u64,
    pub model: Mat4,
    pub color: [f32; 4],
    pub selected: bool,
    pub overhang: bool,
    pub ghost: bool,
}

pub struct DrawList {
    pub camera: Camera,
    pub clip_z: Option<f32>,
    pub overhang_cos: f32,
    pub bed: [f32; 2],
    pub round_bed: bool,
    pub accent: [f32; 3],
    pub objects: Vec<DrawObject>,
    pub lines: Vec<LineVertex>,
    pub show_bed: bool,
    pub toolpath: Option<(u32, u32)>,
    pub feature_mask: u32,
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    last_used: u64,
}

struct Target {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve_view: wgpu::TextureView,
}

pub struct Viewport3d {
    device: wgpu::Device,
    queue: wgpu::Queue,
    globals: wgpu::Buffer,
    bind: wgpu::BindGroup,
    back: wgpu::RenderPipeline,
    mesh_opaque: wgpu::RenderPipeline,
    mesh_ghost: wgpu::RenderPipeline,
    bed: wgpu::RenderPipeline,
    line: wgpu::RenderPipeline,
    path: wgpu::RenderPipeline,
    prism: wgpu::Buffer,
    prism_count: u32,
    meshes: AHashMap<u64, GpuMesh>,
    instances: Option<wgpu::Buffer>,
    instance_capacity: usize,
    lines: Option<wgpu::Buffer>,
    line_capacity: usize,
    toolpath: Vec<wgpu::Buffer>,
    target: Option<Target>,
    pub texture: Option<egui::TextureId>,
    frame: u64,
}

fn prism_vertices() -> Vec<[f32; 6]> {
    // Diamond cross-section; x runs along the path.
    let ring = [(1.0f32, 0.0f32), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];
    let mut v = Vec::with_capacity(36);
    for i in 0..4 {
        let (y0, z0) = ring[i];
        let (y1, z1) = ring[(i + 1) % 4];
        // Facing outward from the middle of this side.
        let (ny, nz) = (y0 + y1, z0 + z1);
        let n = [0.0, ny, nz];
        let quad = [
            [0.0, y0, z0],
            [1.0, y0, z0],
            [1.0, y1, z1],
            [0.0, y0, z0],
            [1.0, y1, z1],
            [0.0, y1, z1],
        ];
        for q in quad {
            v.push([q[0], q[1], q[2], n[0], n[1], n[2]]);
        }
    }
    for (x, nx) in [(0.0f32, -1.0f32), (1.0, 1.0)] {
        let tris = [(0usize, 1usize, 2usize), (0, 2, 3)];
        for (a, b, c) in tris {
            for k in [a, b, c] {
                let (y, z) = ring[k];
                v.push([x, y, z, nx, 0.0, 0.0]);
            }
        }
    }
    v
}

impl Viewport3d {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("view"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("view"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let make = |label: &str,
                    vs: &str,
                    fs: &str,
                    buffers: &[Option<wgpu::VertexBufferLayout>],
                    topology: wgpu::PrimitiveTopology,
                    depth_write: bool,
                    compare: wgpu::CompareFunction,
                    blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipe_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers,
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let mesh_layout = [
            Some(wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<InstanceRaw>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![
                    1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
                    5 => Float32x4, 6 => Float32x4
                ],
            }),
        ];
        let line_layout = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4],
        })];
        let path_layout = [
            Some(wgpu::VertexBufferLayout {
                array_stride: 24,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuSeg>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![2 => Float32x4, 3 => Float32x4, 4 => Unorm8x4, 5 => Uint32],
            }),
        ];
        use wgpu::CompareFunction as C;
        use wgpu::PrimitiveTopology as T;
        let back = make("back", "vs_back", "fs_back", &[], T::TriangleList, false, C::Always, None);
        let mesh_opaque = make("mesh", "vs_mesh", "fs_mesh", &mesh_layout, T::TriangleList, true, C::Less, None);
        let mesh_ghost = make("ghost", "vs_mesh", "fs_mesh", &mesh_layout, T::TriangleList, false, C::Less, blend);
        let bed = make("bed", "vs_bed", "fs_bed", &[], T::TriangleList, false, C::Less, blend);
        let line = make("line", "vs_line", "fs_line", &line_layout, T::LineList, false, C::LessEqual, blend);
        let path = make("path", "vs_path", "fs_path", &path_layout, T::TriangleList, true, C::Less, None);
        let prism_data = prism_vertices();
        let prism = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("prism"),
            contents: bytemuck::cast_slice(&prism_data),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            device: device.clone(),
            queue: queue.clone(),
            globals,
            bind,
            back,
            mesh_opaque,
            mesh_ghost,
            bed,
            line,
            path,
            prism,
            prism_count: prism_data.len() as u32,
            meshes: AHashMap::new(),
            instances: None,
            instance_capacity: 0,
            lines: None,
            line_capacity: 0,
            toolpath: Vec::new(),
            target: None,
            texture: None,
            frame: 0,
        }
    }

    pub fn has_mesh(&self, id: u64) -> bool {
        self.meshes.contains_key(&id)
    }

    pub fn upload_mesh(&mut self, id: u64, mesh: &crate::mesh::Mesh) {
        let vertices = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("model vertices"),
            contents: bytemuck::cast_slice(&mesh.v),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("model indices"),
            contents: bytemuck::cast_slice(&mesh.t),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.meshes.insert(
            id,
            GpuMesh {
                vertices,
                indices,
                count: mesh.t.len() as u32 * 3,
                last_used: self.frame,
            },
        );
    }

    /// Load toolpath data onto the card, split into manageable blocks.
    pub fn set_toolpath(&mut self, segs: &[GpuSeg]) {
        self.toolpath.clear();
        for chunk in segs.chunks(SEGS_PER_CHUNK) {
            self.toolpath.push(self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("toolpath"),
                contents: bytemuck::cast_slice(chunk),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            }));
        }
    }

    pub fn update_toolpath_colors(&mut self, segs: &[GpuSeg]) {
        for (buf, chunk) in self.toolpath.iter().zip(segs.chunks(SEGS_PER_CHUNK)) {
            self.queue.write_buffer(buf, 0, bytemuck::cast_slice(chunk));
        }
    }

    fn ensure_target(&mut self, size: [u32; 2]) -> bool {
        if self.target.as_ref().map(|t| t.size == size).unwrap_or(false) {
            return false;
        }
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let tex = |label, format, samples, usage| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let resolve = tex(
            "view",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let msaa = tex("view msaa", COLOR_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let depth = tex("view depth", DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        self.target = Some(Target {
            size,
            msaa: msaa.create_view(&Default::default()),
            depth: depth.create_view(&Default::default()),
            resolve_view: resolve.create_view(&Default::default()),
        });
        true
    }

    /// Draw everything, returning the picture to show in the window.
    pub fn render(&mut self, rs: &egui_wgpu::RenderState, size: [u32; 2], list: &DrawList) -> egui::TextureId {
        let size = [size[0].clamp(1, 8192), size[1].clamp(1, 8192)];
        let resized = self.ensure_target(size);
        self.frame += 1;
        let target = self.target.as_ref().expect("view target exists after ensure_target");
        {
            let mut renderer = rs.renderer.write();
            match self.texture {
                None => {
                    let id = renderer.register_native_texture(&self.device, &target.resolve_view, wgpu::FilterMode::Linear);
                    self.texture = Some(id);
                }
                Some(id) if resized => {
                    renderer.update_egui_texture_from_wgpu_texture(&self.device, &target.resolve_view, wgpu::FilterMode::Linear, id);
                }
                _ => {}
            }
        }
        let cam = &list.camera;
        let globals = Globals {
            vp: cam.view_proj().to_cols_array_2d(),
            eye: cam.eye().extend(1.0).to_array(),
            clip: [
                list.clip_z.unwrap_or(0.0),
                if list.clip_z.is_some() { 1.0 } else { 0.0 },
                list.overhang_cos,
                0.0,
            ],
            bed: [list.bed[0], list.bed[1], if list.round_bed { 1.0 } else { 0.0 }, size[1] as f32],
            accent: [list.accent[0], list.accent[1], list.accent[2], 1.0],
            mask: [list.feature_mask, 0, 0, 0],
        };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        // Per-object data.
        let raws: Vec<InstanceRaw> = list
            .objects
            .iter()
            .map(|o| InstanceRaw {
                model: o.model.to_cols_array_2d(),
                color: o.color,
                flags: [
                    if o.selected { 1.0 } else { 0.0 },
                    if o.overhang { 1.0 } else { 0.0 },
                    if o.model.determinant() < 0.0 { 1.0 } else { 0.0 },
                    0.0,
                ],
            })
            .collect();
        if raws.len() > self.instance_capacity || self.instances.is_none() {
            self.instance_capacity = (raws.len().max(16)).next_power_of_two();
            self.instances = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instances"),
                size: (self.instance_capacity * std::mem::size_of::<InstanceRaw>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if !raws.is_empty() {
            if let Some(b) = &self.instances {
                self.queue.write_buffer(b, 0, bytemuck::cast_slice(&raws));
            }
        }
        if list.lines.len() > self.line_capacity || self.lines.is_none() {
            self.line_capacity = (list.lines.len().max(1024)).next_power_of_two();
            self.lines = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("lines"),
                size: (self.line_capacity * std::mem::size_of::<LineVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if !list.lines.is_empty() {
            if let Some(b) = &self.lines {
                self.queue.write_buffer(b, 0, bytemuck::cast_slice(&list.lines));
            }
        }

        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("view"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.msaa,
                    depth_slice: None,
                    resolve_target: Some(&target.resolve_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_pipeline(&self.back);
            pass.draw(0..3, 0..1);

            if let Some((first, count)) = list.toolpath {
                if count > 0 && !self.toolpath.is_empty() {
                    pass.set_pipeline(&self.path);
                    pass.set_vertex_buffer(0, self.prism.slice(..));
                    let (mut at, end) = (first as usize, first as usize + count as usize);
                    while at < end {
                        let block = at / SEGS_PER_CHUNK;
                        let Some(buf) = self.toolpath.get(block) else { break };
                        let in_block = at - block * SEGS_PER_CHUNK;
                        let take = (end - at).min(SEGS_PER_CHUNK - in_block);
                        let stride = std::mem::size_of::<GpuSeg>() as u64;
                        pass.set_vertex_buffer(1, buf.slice(in_block as u64 * stride..(in_block + take) as u64 * stride));
                        pass.draw(0..self.prism_count, 0..take as u32);
                        at += take;
                    }
                }
            }
            let frame = self.frame;
            if let Some(instances) = &self.instances {
                let stride = std::mem::size_of::<InstanceRaw>() as u64;
                for ghost_pass in [false, true] {
                    pass.set_pipeline(if ghost_pass { &self.mesh_ghost } else { &self.mesh_opaque });
                    for (i, o) in list.objects.iter().enumerate() {
                        if o.ghost != ghost_pass {
                            continue;
                        }
                        let Some(m) = self.meshes.get_mut(&o.mesh_id) else { continue };
                        m.last_used = frame;
                        pass.set_vertex_buffer(0, m.vertices.slice(..));
                        pass.set_vertex_buffer(1, instances.slice(i as u64 * stride..(i as u64 + 1) * stride));
                        pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..m.count, 0, 0..1);
                    }
                    if !ghost_pass && list.show_bed {
                        pass.set_pipeline(&self.bed);
                        pass.draw(0..6, 0..1);
                    }
                }
            } else if list.show_bed {
                pass.set_pipeline(&self.bed);
                pass.draw(0..6, 0..1);
            }
            if !list.lines.is_empty() {
                if let Some(b) = &self.lines {
                    pass.set_pipeline(&self.line);
                    pass.set_vertex_buffer(0, b.slice(..));
                    pass.draw(0..list.lines.len() as u32, 0..1);
                }
            }
        }
        self.queue.submit([enc.finish()]);
        if self.frame % 600 == 0 {
            let horizon = self.frame.saturating_sub(1800);
            self.meshes.retain(|_, m| m.last_used >= horizon);
        }
        self.texture.expect("texture registered above")
    }
}

/// Handy helpers for building guide lines.
pub fn push_line(out: &mut Vec<LineVertex>, a: Vec3, b: Vec3, color: [u8; 4]) {
    out.push(LineVertex { pos: a.to_array(), color });
    out.push(LineVertex { pos: b.to_array(), color });
}

pub fn push_box(out: &mut Vec<LineVertex>, a: Vec3, b: Vec3, color: [u8; 4]) {
    let c = |x: bool, y: bool, z: bool| {
        Vec3::new(if x { b.x } else { a.x }, if y { b.y } else { a.y }, if z { b.z } else { a.z })
    };
    for &(p, q) in &[
        ((false, false, false), (true, false, false)),
        ((true, false, false), (true, true, false)),
        ((true, true, false), (false, true, false)),
        ((false, true, false), (false, false, false)),
        ((false, false, true), (true, false, true)),
        ((true, false, true), (true, true, true)),
        ((true, true, true), (false, true, true)),
        ((false, true, true), (false, false, true)),
        ((false, false, false), (false, false, true)),
        ((true, false, false), (true, false, true)),
        ((true, true, false), (true, true, true)),
        ((false, true, false), (false, true, true)),
    ] {
        push_line(out, c(p.0, p.1, p.2), c(q.0, q.1, q.2), color);
    }
}

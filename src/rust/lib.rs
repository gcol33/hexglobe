//! hexglobe: the globe of hex_globe() drawn offscreen through wgpu.
//!
//! The scene arrives as the buffers of a hexify globe widget (meshes, lines,
//! uniforms), and the pipelines are those of hexify's
//! inst/htmlwidgets/hex_globe.js, built on the shader the scene carries,
//! hexify's inst/wgsl/globe.wgsl. The frame is drawn into a multisampled
//! texture, resolved, read back and handed over as straight-alpha RGBA.
//! Everything crosses the boundary as a C ABI, called from src/init.c.

use std::collections::HashMap;
use std::ffi::{c_char, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

const SAMPLES: u32 = 4;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

// ---------------------------------------------------------------------------
// C ABI
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HgBuf {
    pub ptr: *const u8,
    pub len: usize,
}

impl HgBuf {
    fn bytes(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }
}

/// One layer of the globe. `kind`: 0 an opaque mesh (ocean, land), 1 a
/// blended mesh coloured by value (H3 cells), 2 the ISEA grid found per
/// pixel on the faces mesh, 3 polylines. `count` is the number of indices of
/// a mesh or of segments of a polyline.
#[repr(C)]
pub struct HgLayer {
    pub kind: u32,
    pub count: u32,
    pub uniform: HgBuf,
    pub pos: HgBuf,
    pub item: HgBuf,
    pub tri: HgBuf,
    pub index: HgBuf,
    pub values: HgBuf,
    pub ramp: HgBuf,
    pub grid: HgBuf,
    pub keys: HgBuf,
    pub points: HgBuf,
    pub segment: HgBuf,
}

#[repr(C)]
pub struct HgScene {
    pub shader: HgBuf,
    pub camera: HgBuf,
    pub width: u32,
    pub height: u32,
    pub layers: *const HgLayer,
    pub n_layers: usize,
}

/// Bytes allocated here, or an error message; released by hexglobe_free().
#[repr(C)]
pub struct HgResult {
    pub data: *mut u8,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    pub error: *mut c_char,
}

fn fill_result(out: *mut HgResult, r: Result<(Vec<u8>, u32, u32), String>) -> i32 {
    let out = unsafe { &mut *out };
    out.data = std::ptr::null_mut();
    out.len = 0;
    out.width = 0;
    out.height = 0;
    out.error = std::ptr::null_mut();
    match r {
        Ok((bytes, w, h)) => {
            let mut b = bytes.into_boxed_slice();
            out.len = b.len();
            out.data = b.as_mut_ptr();
            std::mem::forget(b);
            out.width = w;
            out.height = h;
            0
        }
        Err(msg) => {
            let clean = msg.replace('\0', " ");
            out.error = CString::new(clean).unwrap_or_default().into_raw();
            1
        }
    }
}

fn guarded<F: FnOnce() -> Result<(Vec<u8>, u32, u32), String>>(out: *mut HgResult, f: F) -> i32 {
    let r = match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => Err(if let Some(s) = p.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = p.downcast_ref::<String>() {
            s.clone()
        } else {
            "the renderer panicked".to_string()
        }),
    };
    fill_result(out, r)
}

/// The globe as straight-alpha RGBA, rows from the top.
#[no_mangle]
pub extern "C" fn hexglobe_render(scene: *const HgScene, out: *mut HgResult) -> i32 {
    guarded(out, || {
        let scene = unsafe { &*scene };
        let layers = if scene.layers.is_null() || scene.n_layers == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(scene.layers, scene.n_layers) }
        };
        let rgba = render(scene, layers)?;
        Ok((rgba, scene.width, scene.height))
    })
}

/// The name and backend of the graphics adapter the renderer draws on.
#[no_mangle]
pub extern "C" fn hexglobe_adapter(out: *mut HgResult) -> i32 {
    guarded(out, || {
        let mut gpu = GPU.lock().map_err(|_| "the renderer's lock is poisoned".to_string())?;
        let gpu = gpu_get(&mut gpu)?;
        let info = &gpu.info;
        let text = format!("{} ({:?})", info.name, info.backend);
        Ok((text.into_bytes(), 0, 0))
    })
}

/// RGBA as a PNG file's bytes.
#[no_mangle]
pub extern "C" fn hexglobe_png_encode(rgba: HgBuf, width: u32, height: u32, out: *mut HgResult) -> i32 {
    guarded(out, || {
        let px = rgba.bytes();
        if px.len() != 4 * width as usize * height as usize {
            return Err("the image is not width x height RGBA pixels".to_string());
        }
        let mut file = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut file, width, height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().map_err(|e| e.to_string())?;
            w.write_image_data(px).map_err(|e| e.to_string())?;
        }
        Ok((file, width, height))
    })
}

/// A PNG file's bytes as RGBA.
#[no_mangle]
pub extern "C" fn hexglobe_png_decode(file: HgBuf, out: *mut HgResult) -> i32 {
    guarded(out, || {
        let mut dec = png::Decoder::new(std::io::Cursor::new(file.bytes()));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
        let mut reader = dec.read_info().map_err(|e| e.to_string())?;
        let size = reader.output_buffer_size().ok_or("the image is too large")?;
        let mut buf = vec![0u8; size];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        if info.bit_depth != png::BitDepth::Eight {
            return Err("only 8-bit PNG images are read".to_string());
        }
        let (w, h) = (info.width, info.height);
        let px = &buf[..info.buffer_size()];
        let rgba = match info.color_type {
            png::ColorType::Rgba => px.to_vec(),
            png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
            other => return Err(format!("unexpected PNG colour type {:?}", other)),
        };
        Ok((rgba, w, h))
    })
}

#[no_mangle]
pub extern "C" fn hexglobe_free(res: *mut HgResult) {
    let res = unsafe { &mut *res };
    if !res.data.is_null() {
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(res.data, res.len)));
        }
        res.data = std::ptr::null_mut();
    }
    if !res.error.is_null() {
        unsafe {
            drop(CString::from_raw(res.error));
        }
        res.error = std::ptr::null_mut();
    }
}

// ---------------------------------------------------------------------------
// Device and pipelines, made once
// ---------------------------------------------------------------------------

struct Pipelines {
    shader: Vec<u8>,
    // surface, overlay, grid, line
    list: [wgpu::RenderPipeline; 4],
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: wgpu::AdapterInfo,
    pipelines: Option<Pipelines>,
}

static GPU: Mutex<Option<Gpu>> = Mutex::new(None);

fn gpu_get(slot: &mut Option<Gpu>) -> Result<&mut Gpu, String> {
    if slot.is_none() {
        *slot = Some(gpu_new()?);
    }
    Ok(slot.as_mut().unwrap())
}

fn gpu_new() -> Result<Gpu, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance),
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .map_err(|e| format!("no graphics adapter: {}", e))?;
    let limits = adapter.limits();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("hexglobe"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits {
            max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size,
            max_buffer_size: limits.max_buffer_size,
            ..wgpu::Limits::default()
        },
        ..Default::default()
    }))
    .map_err(|e| format!("no graphics device: {}", e))?;
    Ok(Gpu { device, queue, info: adapter.get_info(), pipelines: None })
}

fn mesh_vertex_attrs() -> [wgpu::VertexAttribute; 2] {
    [
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
    ]
}

fn build_pipelines(device: &wgpu::Device, code: &str) -> [wgpu::RenderPipeline; 4] {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("globe.wgsl"),
        source: wgpu::ShaderSource::Wgsl(code.into()),
    });
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
    let pos_attrs = mesh_vertex_attrs();
    let item_attrs = [wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 0, shader_location: 2 }];
    let tri_attrs = [wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 3 }];
    let seg_attrs = [wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 0, shader_location: 0 }];
    let pos = wgpu::VertexBufferLayout { array_stride: 24, step_mode: wgpu::VertexStepMode::Vertex, attributes: &pos_attrs };
    let item = wgpu::VertexBufferLayout { array_stride: 4, step_mode: wgpu::VertexStepMode::Vertex, attributes: &item_attrs };
    let tri = wgpu::VertexBufferLayout { array_stride: 8, step_mode: wgpu::VertexStepMode::Vertex, attributes: &tri_attrs };
    let seg = wgpu::VertexBufferLayout { array_stride: 4, step_mode: wgpu::VertexStepMode::Instance, attributes: &seg_attrs };

    let make = |vs: &str, fs: &str, buffers: &[Option<wgpu::VertexBufferLayout>], blended: bool, depth_write: bool| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(fs),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: if blended { Some(blend) } else { None },
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(depth_write),
                depth_compare: Some(if depth_write { wgpu::CompareFunction::Less } else { wgpu::CompareFunction::LessEqual }),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            multiview_mask: None,
            cache: None,
        })
    };
    [
        make("vs_mesh", "fs_mesh", &[Some(pos.clone()), Some(item.clone())], false, true),
        make("vs_mesh", "fs_mesh", &[Some(pos.clone()), Some(item.clone())], true, false),
        make("vs_grid", "fs_grid", &[Some(pos), Some(item), Some(tri)], true, false),
        make("vs_line", "fs_line", &[Some(seg)], true, false),
    ]
}

// ---------------------------------------------------------------------------
// The frame
// ---------------------------------------------------------------------------

/// A buffer of at least 16 bytes, a multiple of 4, holding `data`.
fn upload(device: &wgpu::Device, data: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    let size = std::cmp::max(16, data.len().div_ceil(4) * 4) as u64;
    let buf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage, mapped_at_creation: true });
    let mut padded = vec![0u8; size as usize];
    padded[..data.len()].copy_from_slice(data);
    buf.get_mapped_range_mut(..).expect("a buffer mapped at creation").copy_from_slice(&padded);
    buf.unmap();
    buf
}

/// Vertex and index buffers, made once per array and shared by the layers
/// drawn on one mesh.
struct Uploads<'a> {
    device: &'a wgpu::Device,
    made: HashMap<(usize, usize, u32), wgpu::Buffer>,
}

impl<'a> Uploads<'a> {
    fn get(&mut self, b: &HgBuf, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        let key = (b.ptr as usize, b.len, usage.bits());
        let device = self.device;
        self.made.entry(key).or_insert_with(|| upload(device, b.bytes(), usage)).clone()
    }
}

fn render(scene: &HgScene, layers: &[HgLayer]) -> Result<Vec<u8>, String> {
    let (w, h) = (scene.width, scene.height);
    if w == 0 || h == 0 {
        return Err("the image needs a positive width and height".to_string());
    }
    if scene.camera.len != 112 {
        return Err("the camera uniform is 28 floats".to_string());
    }
    let mut slot = GPU.lock().map_err(|_| "the renderer's lock is poisoned".to_string())?;
    let gpu = gpu_get(&mut slot)?;
    let max_side = gpu.device.limits().max_texture_dimension_2d;
    if w > max_side || h > max_side {
        return Err(format!("the image is at most {} pixels on a side on this graphics card", max_side));
    }
    let shader = scene.shader.bytes();
    let device = gpu.device.clone();
    let queue = gpu.queue.clone();

    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    if gpu.pipelines.as_ref().map(|p| p.shader.as_slice() != shader).unwrap_or(true) {
        let code = std::str::from_utf8(shader).map_err(|_| "the shader is not UTF-8".to_string())?;
        gpu.pipelines = Some(Pipelines { shader: shader.to_vec(), list: build_pipelines(&device, code) });
    }
    let pipelines = &gpu.pipelines.as_ref().unwrap().list;

    let usage_uniform = wgpu::BufferUsages::UNIFORM;
    let usage_storage = wgpu::BufferUsages::STORAGE;
    let usage_vertex = wgpu::BufferUsages::VERTEX;
    let usage_index = wgpu::BufferUsages::INDEX;
    let mut uploads = Uploads { device: &device, made: HashMap::new() };

    let camera = upload(&device, scene.camera.bytes(), usage_uniform);
    let camera_groups: Vec<wgpu::BindGroup> = pipelines
        .iter()
        .map(|p| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &p.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera.as_entire_binding() }],
            })
        })
        .collect();

    struct Draw {
        pipeline: usize,
        group: wgpu::BindGroup,
        vertex: Vec<wgpu::Buffer>,
        index: Option<wgpu::Buffer>,
        count: u32,
    }
    let mut draws = Vec::with_capacity(layers.len());
    for layer in layers {
        let p = layer.kind as usize;
        if p > 3 {
            return Err(format!("unknown layer kind {}", layer.kind));
        }
        let uniform = upload(&device, layer.uniform.bytes(), usage_uniform);
        fn entry(binding: u32, buf: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry { binding, resource: buf.as_entire_binding() }
        }
        let layout = pipelines[p].get_bind_group_layout(1);
        let draw = if p == 3 {
            let points = upload(&device, layer.points.bytes(), usage_storage);
            let segment = upload(&device, layer.segment.bytes(), usage_vertex);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[entry(0, &uniform), entry(3, &points)],
            });
            Draw { pipeline: p, group, vertex: vec![segment], index: None, count: layer.count }
        } else {
            let values = upload(&device, layer.values.bytes(), usage_storage);
            let ramp = upload(&device, layer.ramp.bytes(), usage_storage);
            let mut vertex = vec![uploads.get(&layer.pos, usage_vertex), uploads.get(&layer.item, usage_vertex)];
            let group = if p == 2 {
                let grid = upload(&device, layer.grid.bytes(), usage_uniform);
                let keys = upload(&device, layer.keys.bytes(), usage_storage);
                vertex.push(uploads.get(&layer.tri, usage_vertex));
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &layout,
                    entries: &[entry(0, &uniform), entry(1, &values), entry(2, &ramp), entry(4, &grid), entry(5, &keys)],
                })
            } else {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &layout,
                    entries: &[entry(0, &uniform), entry(1, &values), entry(2, &ramp)],
                })
            };
            let index = uploads.get(&layer.index, usage_index);
            Draw { pipeline: p, group, vertex, index: Some(index), count: layer.count }
        };
        draws.push(draw);
    }

    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let texture = |samples: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let msaa = texture(SAMPLES, COLOR_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let depth = texture(SAMPLES, DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let target = texture(1, COLOR_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let row = (4 * w).div_ceil(256) * 256;
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: row as u64 * h as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let msaa_view = msaa.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());
    let target_view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &msaa_view,
                depth_slice: None,
                resolve_target: Some(&target_view),
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Discard },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for d in &draws {
            pass.set_pipeline(&pipelines[d.pipeline]);
            pass.set_bind_group(0, &camera_groups[d.pipeline], &[]);
            pass.set_bind_group(1, &d.group, &[]);
            for (k, v) in d.vertex.iter().enumerate() {
                pass.set_vertex_buffer(k as u32, v.slice(..));
            }
            match &d.index {
                Some(index) => {
                    pass.set_index_buffer(index.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..d.count, 0, 0..1);
                }
                None => pass.draw(0..6, 0..d.count),
            }
        }
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &read,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    if let Some(err) = pollster::block_on(scope.pop()) {
        return Err(format!("the graphics card refused the scene: {}", err));
    }

    let (tx, rx) = std::sync::mpsc::channel();
    read.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("the graphics card stopped: {}", e))?;
    rx.recv()
        .map_err(|_| "the image was never read back".to_string())?
        .map_err(|e| format!("the image could not be read back: {}", e))?;
    let src = read.get_mapped_range(..).map_err(|e| e.to_string())?;
    Ok(unpremultiply(&src, w, h, row))
}

/// Premultiplied pixels, rows `row` bytes apart, as straight alpha, the way
/// the widget's snapshot() reads them.
fn unpremultiply(src: &[u8], w: u32, h: u32, row: u32) -> Vec<u8> {
    let (w, h, row) = (w as usize, h as usize, row as usize);
    let mut px = vec![0u8; 4 * w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * row + 4 * x;
            let o = 4 * (y * w + x);
            let a = src[i + 3];
            let k = if a > 0 { 255.0 / a as f64 } else { 0.0 };
            for c in 0..3 {
                px[o + c] = (src[i + c] as f64 * k).round().min(255.0) as u8;
            }
            px[o + 3] = a;
        }
    }
    px
}

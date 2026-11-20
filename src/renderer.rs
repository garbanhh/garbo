//! Renderer GPU mínimo: un pipeline, una textura atlas, un buffer de instancias, un draw call.
//! Solo se redibuja cuando algo cambia (salida del PTY, input, resize), nunca en bucle.

use crate::atlas::ATLAS_SIZE;
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::window::Window;

/// Variables de entorno que pusimos nosotros y que no deben heredar los shells.
static ENV_OVERRIDES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();

pub fn env_overrides() -> &'static [&'static str] {
    ENV_OVERRIDES.get().map_or(&[], |v| v.as_slice())
}

/// El cargador de Vulkan abre *todos* los drivers instalados para enumerarlos, y algunos
/// (radeon, lavapipe) arrastran libLLVM: ~25 MB residentes para nada. Detectamos el fabricante
/// de la GPU y limitamos el cargador a sus drivers. Si no se reconoce, solo se descartan
/// lavapipe y dzn. Sin GPU usable queda el fallback a OpenGL.
pub fn limit_vulkan_drivers() {
    let mut set = Vec::new();
    #[cfg(target_os = "linux")]
    if std::env::var_os("VK_LOADER_DRIVERS_SELECT").is_none()
        && std::env::var_os("VK_LOADER_DRIVERS_DISABLE").is_none()
    {
        let mut select: Vec<&str> = Vec::new();
        if let Ok(dir) = std::fs::read_dir("/sys/class/drm") {
            for entry in dir.flatten() {
                let vendor =
                    std::fs::read_to_string(entry.path().join("device/vendor")).unwrap_or_default();
                let drivers: &[&str] = match vendor.trim() {
                    "0x8086" => &["*intel*"],
                    "0x1002" => &["*radeon*", "*amd*"],
                    "0x10de" => &["*nvidia*", "*nouveau*"],
                    "0x1af4" => &["*virtio*"],
                    _ => &[],
                };
                for d in drivers {
                    if !select.contains(d) {
                        select.push(d);
                    }
                }
            }
        }
        // SAFETY (edición 2021): se llama al principio de main, antes de crear hilos.
        if select.is_empty() {
            std::env::set_var("VK_LOADER_DRIVERS_DISABLE", "*lvp*,*dzn*");
            set.push("VK_LOADER_DRIVERS_DISABLE");
        } else {
            std::env::set_var("VK_LOADER_DRIVERS_SELECT", select.join(","));
            set.push("VK_LOADER_DRIVERS_SELECT");
        }
    }
    let _ = ENV_OVERRIDES.set(set);
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Instance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub kind: u32,
    pub radius: f32,
}

impl Instance {
    pub fn rect(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Self {
        Self {
            pos: [x, y],
            size: [w, h],
            uv: [0.0; 4],
            color,
            kind: 0,
            radius: 0.0,
        }
    }

    pub fn rounded(x: f32, y: f32, w: f32, h: f32, color: [f32; 4], radius: f32) -> Self {
        Self {
            radius,
            ..Self::rect(x, y, w, h, color)
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    screen: [f32; 2],
    _pad: [f32; 2],
}

impl Uniforms {
    fn for_surface(config: &wgpu::SurfaceConfiguration) -> Self {
        Self {
            screen: [config.width as f32, config.height as f32],
            _pad: [0.0; 2],
        }
    }
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    instances: wgpu::Buffer,
    instance_cap: usize,
    pub atlas: wgpu::Texture,
    capture: Option<std::path::PathBuf>,
}

impl Renderer {
    pub fn new(window: Arc<Window>, transparent: bool) -> Self {
        pollster::block_on(Self::new_async(window, transparent))
    }

    async fn new_async(window: Arc<Window>, transparent: bool) -> Self {
        let size = window.inner_size();
        // Primero Vulkan/Metal/DX12; OpenGL solo si no hay otra opción (inicializar EGL cuesta RAM).
        let flags = if std::env::var_os("GARBO_GPU_DEBUG").is_some() {
            wgpu::InstanceFlags::debugging()
        } else {
            wgpu::InstanceFlags::empty()
        };
        let mut found = None;
        for backends in [wgpu::Backends::PRIMARY, wgpu::Backends::GL] {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends,
                flags,
                ..Default::default()
            });
            let Ok(surface) = instance.create_surface(window.clone()) else {
                continue;
            };
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    // GPU integrada = menos consumo
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                })
                .await;
            if let Ok(adapter) = adapter {
                found = Some((surface, adapter));
                break;
            }
        }
        let (surface, adapter) = found.expect("no hay adaptador GPU compatible");
        log::info!("GPU: {:?}", adapter.get_info());

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("garbo"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .expect("no se pudo abrir el dispositivo GPU");

        let caps = surface.get_capabilities(&adapter);
        // Herramienta de desarrollo: GARBO_SCREENSHOT=/ruta.ppm vuelca cada frame a disco.
        let capture = std::env::var_os("GARBO_SCREENSHOT").map(std::path::PathBuf::from);
        // Formato no-sRGB: los colores de terminal se mezclan en espacio sRGB como en el resto de terminales.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let alpha_mode = if transparent
            && caps
                .alpha_modes
                .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            caps.alpha_modes[0]
        };
        let config = wgpu::SurfaceConfiguration {
            usage: if capture.is_some() && caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
            } else {
                wgpu::TextureUsages::RENDER_ATTACHMENT
            },
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 1,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));

        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniforms"),
            contents: bytemuck::bytes_of(&Uniforms::for_surface(&config)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
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
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quads"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2, 1 => Float32x2, 2 => Float32x4,
                        3 => Float32x4, 4 => Uint32, 5 => Float32
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let instance_cap = 1024;
        let instances = Self::make_instance_buffer(&device, instance_cap);

        Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group,
            uniforms,
            instances,
            instance_cap,
            atlas,
            capture,
        }
    }

    fn make_instance_buffer(device: &wgpu::Device, cap: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (cap * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if (self.config.width, self.config.height) == (width.max(1), height.max(1)) {
            return;
        }
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        self.queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&Uniforms::for_surface(&self.config)),
        );
    }

    pub fn size(&self) -> (f32, f32) {
        (self.config.width as f32, self.config.height as f32)
    }

    pub fn render(&mut self, instances: &[Instance], clear: [f32; 4]) {
        if instances.len() > self.instance_cap {
            self.instance_cap = instances.len().next_power_of_two();
            self.instances = Self::make_instance_buffer(&self.device, self.instance_cap);
        }
        if !instances.is_empty() {
            self.queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(instances));
        }

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(e) => {
                log::warn!("frame perdido: {e:?}");
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            // Color de limpieza premultiplicado
            let a = clear[3] as f64;
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0] as f64 * a,
                            g: clear[1] as f64 * a,
                            b: clear[2] as f64 * a,
                            a,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.draw(0..4, 0..instances.len() as u32);
        }
        let readback =
            self.capture.is_some() && self.config.usage.contains(wgpu::TextureUsages::COPY_SRC);
        let padded_row = (self.config.width * 4).div_ceil(256) * 256;
        let buffer = readback.then(|| {
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("capture"),
                size: (padded_row * self.config.height) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            enc.copy_texture_to_buffer(
                frame.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row),
                        rows_per_image: None,
                    },
                },
                frame.texture.size(),
            );
            buf
        });
        self.queue.submit(Some(enc.finish()));
        if let Some(buf) = buffer {
            self.save_capture(&buf, padded_row);
        }
        frame.present();
    }

    fn save_capture(&self, buf: &wgpu::Buffer, padded_row: u32) {
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait);
        let data = slice.get_mapped_range();
        let (w, h) = (self.config.width, self.config.height);
        let bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
        for y in 0..h {
            let row = &data[(y * padded_row) as usize..];
            for x in 0..w as usize {
                let px = &row[x * 4..x * 4 + 4];
                if bgra {
                    out.extend_from_slice(&[px[2], px[1], px[0]]);
                } else {
                    out.extend_from_slice(&px[..3]);
                }
            }
        }
        if let Some(path) = &self.capture {
            let _ = std::fs::write(path, out);
        }
    }
}

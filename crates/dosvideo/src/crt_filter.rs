//! A GPU post-filter that makes the picture look like it is on a CRT: scanlines with a beam
//! that widens on bright pixels, horizontal beam blur (which melts dither patterns into
//! colors, as artists of the era counted on), an RGB phosphor mask, bloom, curvature and a
//! vignette.

use game_toolkit::prelude::*;

/// The phosphor arrangement behind the glass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PhosphorMask {
    /// No mask: monochrome monitors, or a perfect screen.
    #[default]
    None,
    /// Vertical R, G, B stripes (Trinitron, many later VGA monitors).
    ApertureGrille,
    /// R, G, B slots in staggered columns (most TVs and consumer monitors).
    SlotMask,
    /// R, G, B dot triads in offset rows (classic shadow mask).
    ShadowMask,
}

/// CRT filter settings. Start from a preset ([`CrtFilter::vga_monitor`], [`CrtFilter::tv`],
/// ...) and adjust.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrtFilter {
    /// 0 = no scanline structure, 1 = full beam profile with dark gaps.
    pub scanlines: f32,
    /// Beam height (Gaussian sigma, in scanlines) of dark and of bright pixels. Bright pixels
    /// bloom wider and close the gaps, like a real beam.
    pub beam_min: f32,
    pub beam_max: f32,
    /// Horizontal blur of the beam spot, in dots (sigma). ~0.3 is sharp, ~1 is a TV.
    pub blur: f32,
    pub mask: PhosphorMask,
    /// 0..1, how dark the "wrong" phosphors are.
    pub mask_strength: f32,
    /// Width of one R+G+B triad in window pixels. Below ~3 the mask fades out.
    pub mask_pitch: f32,
    /// Overall gain, to make up for light lost to the mask and the gaps.
    pub brightness: f32,
    /// Glow around bright areas.
    pub bloom: f32,
    /// Barrel distortion of a curved tube, 0 = flat.
    pub curvature: f32,
    /// Darkening towards the edges, 0..1.
    pub vignette: f32,
    /// Corner rounding as a fraction of the picture height.
    pub corner_radius: f32,
}

impl CrtFilter {
    /// Sharp pixels with clean scanlines: a high-quality monitor, or a modern "scanline" look.
    pub fn crisp() -> Self {
        Self {
            scanlines: 0.7,
            beam_min: 0.28,
            beam_max: 0.42,
            blur: 0.3,
            mask: PhosphorMask::None,
            mask_strength: 0.0,
            mask_pitch: 3.0,
            brightness: 1.1,
            bloom: 0.0,
            curvature: 0.0,
            vignette: 0.0,
            corner_radius: 0.0,
        }
    }

    /// A 14" VGA monitor: fine aperture grille, light blur, slight curvature.
    pub fn vga_monitor() -> Self {
        Self {
            scanlines: 0.55,
            beam_min: 0.3,
            beam_max: 0.55,
            blur: 0.5,
            mask: PhosphorMask::ApertureGrille,
            mask_strength: 0.3,
            mask_pitch: 3.0,
            brightness: 1.3,
            bloom: 0.1,
            curvature: 0.02,
            vignette: 0.2,
            corner_radius: 0.02,
        }
    }

    /// A consumer TV on composite or RF: blurry, slot mask, strong glow, curved glass.
    pub fn tv() -> Self {
        Self {
            scanlines: 0.85,
            beam_min: 0.25,
            beam_max: 0.6,
            blur: 1.0,
            mask: PhosphorMask::SlotMask,
            mask_strength: 0.4,
            mask_pitch: 4.0,
            brightness: 1.5,
            bloom: 0.3,
            curvature: 0.07,
            vignette: 0.4,
            corner_radius: 0.06,
        }
    }

    /// A monochrome monitor (Hercules, MDA): no mask, long-glowing phosphor.
    pub fn mono_monitor() -> Self {
        Self {
            scanlines: 0.6,
            beam_min: 0.3,
            beam_max: 0.55,
            blur: 0.55,
            mask: PhosphorMask::None,
            mask_strength: 0.0,
            mask_pitch: 3.0,
            brightness: 1.15,
            bloom: 0.35,
            curvature: 0.03,
            vignette: 0.25,
            corner_radius: 0.03,
        }
    }

    /// The presets with a name each, in the order Alt+C cycles them (after "off").
    pub fn presets() -> [(&'static str, CrtFilter); 4] {
        [
            ("crisp scanlines", Self::crisp()),
            ("VGA monitor", Self::vga_monitor()),
            ("TV", Self::tv()),
            ("mono monitor", Self::mono_monitor()),
        ]
    }

    fn uniforms(&self, rect: [f32; 4], tex: [f32; 2], region: [f32; 4]) -> [u8; 96] {
        let mask = match self.mask {
            PhosphorMask::None => 0.0,
            PhosphorMask::ApertureGrille => 1.0,
            PhosphorMask::SlotMask => 2.0,
            PhosphorMask::ShadowMask => 3.0,
        };
        let v: [f32; 24] = [
            rect[0],
            rect[1],
            rect[2],
            rect[3],
            tex[0],
            tex[1],
            region[0],
            region[1],
            region[2],
            region[3],
            0.0,
            0.0,
            self.scanlines,
            self.beam_min.max(0.05),
            self.beam_max.max(0.05),
            self.blur,
            mask,
            self.mask_strength,
            self.mask_pitch,
            self.brightness,
            self.bloom,
            self.curvature,
            self.vignette,
            self.corner_radius,
        ];
        let mut out = [0u8; 96];
        for (i, f) in v.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
        }
        out
    }
}

/// GPU resources of the filter.
pub(crate) struct CrtPass {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    texture: Option<(wgpu::Texture, wgpu::BindGroup, u32, u32)>,
}

impl CrtPass {
    pub fn new(gfx: &Graphics) -> Self {
        let device = gfx.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dosvideo.crt.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("crt_filter.wgsl").into()),
        });
        let tex_entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty,
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dosvideo.crt.bgl"),
            entries: &[
                tex_entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                tex_entry(
                    1,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                tex_entry(
                    2,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dosvideo.crt.layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dosvideo.crt.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gfx.surface_format(),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("dosvideo.crt.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dosvideo.crt.uniforms"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            bgl,
            sampler,
            uniforms,
            texture: None,
        }
    }

    /// Upload a frame (RGBA8, sRGB).
    pub fn upload(&mut self, gfx: &Graphics, w: u32, h: u32, rgba: &[u8]) {
        let device = gfx.device();
        if !matches!(&self.texture, Some((_, _, tw, th)) if *tw == w && *th == h) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("dosvideo.crt.source"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("dosvideo.crt.bg"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.uniforms.as_entire_binding(),
                    },
                ],
            });
            self.texture = Some((texture, bg, w, h));
        }
        let Some((texture, ..)) = &self.texture else {
            return;
        };
        gfx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Draw the uploaded frame into `rect` (window pixels), showing `region` (dots) of it.
    pub fn draw(
        &self,
        gfx: &Graphics,
        frame: &mut Frame,
        filter: &CrtFilter,
        rect: [f32; 4],
        region: [f32; 4],
    ) {
        let Some((_, bg, tw, th)) = &self.texture else {
            return;
        };
        let (ww, wh) = gfx.size();
        // Clip the rectangle to the window for the viewport; the shader works in absolute
        // window coordinates, so clipping does not move the picture.
        let x0 = rect[0].max(0.0);
        let y0 = rect[1].max(0.0);
        let x1 = (rect[0] + rect[2]).min(ww as f32);
        let y1 = (rect[1] + rect[3]).min(wh as f32);
        if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
            return;
        }
        gfx.queue().write_buffer(
            &self.uniforms,
            0,
            &filter.uniforms(rect, [*tw as f32, *th as f32], region),
        );
        let (view, encoder) = frame.view_and_encoder();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("dosvideo.crt.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        pass.set_viewport(x0, y0, x1 - x0, y1 - y0, 0.0, 1.0);
        pass.set_scissor_rect(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

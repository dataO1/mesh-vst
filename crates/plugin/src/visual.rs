//! The lane visual (design D33): a 100 × 100 field of points in the style of
//! Output Portal.
//!
//! Every point is always on. Bass: a row is a frequency bin (low at the
//! bottom) and each point moves along a slowly turning flow field by its
//! bin's level over a volume range in dB. Drums: the points are short lines
//! and the field is split into one subgrid per kit slot; a subgrid moves
//! with its slot's hits on the spectrum's texture, bleeding a little into
//! its neighbours. Brightness, size and the red/blue split follow each
//! point's own level. Drawn on the GPU in one instanced call (three passes:
//! red, blue, ink); the editor only uploads the levels each frame.

use std::sync::Arc;

use nice_plug_iced::iced::widget::shader::{self, Viewport};
use nice_plug_iced::iced::{mouse, Rectangle};
use nice_plug_iced::iced::wgpu;
use realfft::{RealFftPlanner, RealToComplex};

/// Points per side.
pub const N: u32 = 100;
/// Frequency bins (one per row).
pub const BINS: usize = N as usize;
/// Kit slots of the drum subgrids (4 × 2).
pub const SLOTS: usize = 8;
const FFT: usize = 2048;
/// Lowest and highest bin frequency.
const F_LO: f32 = 40.0;
const F_HI: f32 = 16_000.0;
// MUSICAL: the visual's feel, from the design mockup.
/// Volume range the bins map over, in dB.
pub const RANGE_DB: f32 = 42.0;
const ATTACK: f32 = 0.65;
const RELEASE: f32 = 0.18;
const TILT_DB_PER_OCT: f32 = 4.5;
const SENSITIVITY_DB: f32 = 18.0;

/// What one frame of the visual shows.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub levels: [f32; BINS],
    pub slots: [f32; SLOTS],
    /// Overall envelope and transient amount, 0..1.
    pub env: f32,
    pub punch: f32,
    pub time: f32,
    pub drums: bool,
    pub aberration: f32,
}

/// Turns the plugin's output into per-bin levels (on the editor thread).
pub struct Analyser {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    history: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex<f32>>,
    bins: [(usize, usize); BINS],
    levels: [f32; BINS],
    env: f32,
    punch: f32,
    sample_rate: f32,
}

impl Analyser {
    pub fn new(sample_rate: f32) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT);
        let window = (0..FFT).map(|k| 0.5 - 0.5 * (std::f32::consts::TAU * k as f32 / FFT as f32).cos()).collect();
        let hz = sample_rate / FFT as f32;
        let bins = std::array::from_fn(|b| {
            let lo = F_LO * (F_HI / F_LO).powf(b as f32 / BINS as f32);
            let hi = F_LO * (F_HI / F_LO).powf((b + 1) as f32 / BINS as f32);
            let a = (lo / hz).floor() as usize;
            (a.max(1), ((hi / hz).ceil() as usize).max(a + 2).min(FFT / 2))
        });
        Self {
            input: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            fft,
            window,
            history: vec![0.0; FFT],
            bins,
            levels: [0.0; BINS],
            env: 0.0,
            punch: 0.0,
            sample_rate,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Take new mono samples and update the levels.
    pub fn push(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            // Silence: let everything fall back.
            for l in &mut self.levels {
                *l *= 1.0 - RELEASE;
            }
            self.env *= 0.85;
            self.punch *= 0.8;
            return;
        }
        let n = samples.len().min(FFT);
        self.history.drain(..n);
        self.history.extend_from_slice(&samples[samples.len() - n..]);
        for (i, (x, w)) in self.history.iter().zip(&self.window).enumerate() {
            self.input[i] = x * w;
        }
        if self.fft.process(&mut self.input, &mut self.spectrum).is_err() {
            return;
        }
        let norm = 2.0 / FFT as f32;
        for (b, &(lo, hi)) in self.bins.iter().enumerate() {
            let p = self.spectrum[lo..hi].iter().map(|c| c.norm_sqr()).sum::<f32>() / (hi - lo) as f32;
            // A spectrum display's tilt (+4.5 dB per octave above 1 kHz, as
            // analysers show music flat) and the field's sensitivity.
            let f = F_LO * (F_HI / F_LO).powf((b as f32 + 0.5) / BINS as f32);
            let db = 10.0 * (p * norm * norm + 1e-12).log10() + TILT_DB_PER_OCT * (f / 1000.0).log2() + SENSITIVITY_DB;
            let target = ((db + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0);
            let k = if target > self.levels[b] { ATTACK } else { RELEASE };
            self.levels[b] += (target - self.levels[b]) * k;
        }
        let rms = (samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32).sqrt();
        let e = ((20.0 * (rms + 1e-9).log10() + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0);
        self.punch = (self.punch * 0.8).max((e - self.env).max(0.0) * 3.0).min(1.0);
        self.env += (e - self.env) * if e > self.env { 0.6 } else { 0.15 };
    }

    pub fn levels(&self) -> [f32; BINS] {
        self.levels
    }

    pub fn env(&self) -> f32 {
        self.env
    }

    pub fn punch(&self) -> f32 {
        self.punch
    }
}

/// The shader widget's program.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub frame: Frame,
}

impl<Message> shader::Program<Message> for Field {
    type State = ();
    type Primitive = FieldPrimitive;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> FieldPrimitive {
        FieldPrimitive { frame: self.frame }
    }
}

#[derive(Debug)]
pub struct FieldPrimitive {
    frame: Frame,
}

/// Uniforms, laid out for WGSL (16-byte rows).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    /// width, height, time, aberration
    a: [f32; 4],
    /// mode (0 bass, 1 drums), env, punch, scale
    b: [f32; 4],
    levels: [[f32; 4]; BINS / 4],
    slots: [[f32; 4]; SLOTS / 4],
}

pub struct FieldPipeline {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl shader::Pipeline for FieldPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh vst field"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(SHADER)),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh vst field uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&layout], push_constant_ranges: &[] });
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh vst field"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(additive), write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        Self { pipeline, uniforms, bind_group }
    }
}

impl shader::Primitive for FieldPrimitive {
    type Pipeline = FieldPipeline;

    fn prepare(&self, pipeline: &mut FieldPipeline, _device: &wgpu::Device, queue: &wgpu::Queue, bounds: &Rectangle, viewport: &Viewport) {
        let f = &self.frame;
        let scale = viewport.scale_factor() as f32;
        let u = Uniforms {
            a: [bounds.width * scale, bounds.height * scale, f.time, f.aberration],
            b: [if f.drums { 1.0 } else { 0.0 }, f.env, f.punch, scale],
            levels: std::array::from_fn(|r| std::array::from_fn(|c| f.levels[r * 4 + c])),
            slots: std::array::from_fn(|r| std::array::from_fn(|c| f.slots[r * 4 + c])),
        };
        queue.write_buffer(&pipeline.uniforms, 0, bytemuck::bytes_of(&u));
    }

    fn draw(&self, pipeline: &FieldPipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &pipeline.bind_group, &[]);
        // Six vertices per point, three passes (red, blue, ink).
        pass.draw(0..6, 0..3 * N * N);
        true
    }
}

const SHADER: &str = r#"
struct U {
    a: vec4<f32>,
    b: vec4<f32>,
    levels: array<vec4<f32>, 25>,
    slots: array<vec4<f32>, 2>,
};
@group(0) @binding(0) var<uniform> u: U;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
};

fn level(b: u32) -> f32 {
    let v = u.levels[b / 4u];
    let i = b % 4u;
    if i == 0u { return v.x; }
    if i == 1u { return v.y; }
    if i == 2u { return v.z; }
    return v.w;
}

fn slot(s: u32) -> f32 {
    let v = u.slots[s / 4u];
    let i = s % 4u;
    if i == 0u { return v.x; }
    if i == 1u { return v.y; }
    if i == 2u { return v.z; }
    return v.w;
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Out {
    let n = 100u;
    let k = ii % (n * n);
    let pass_id = ii / (n * n);
    let col = k % n;
    let row = k / n;
    let size = u.a.xy;
    let t = u.a.z;
    let ab = u.a.w;
    let drums = u.b.x > 0.5;
    let env = u.b.y;
    let punch = u.b.z;
    let scale = u.b.w;

    var lv = level(row);
    if drums {
        // A subgrid per slot (4 × 2); its hits move it, the spectrum gives texture,
        // the neighbours bleed in a little.
        let sx = col / 25u;
        let sy = row / 50u;
        let s = sx + 4u * (1u - sy);
        var e = slot(s);
        let left = select(s, s - 1u, sx > 0u);
        let right = select(s, s + 1u, sx < 3u);
        let other = select(s - 4u, s + 4u, s < 4u);
        e = max(e, 0.25 * max(slot(left), max(slot(right), slot(other))));
        lv = clamp(e * (0.45 + 0.75 * lv), 0.0, 1.0);
    }
    let l = pow(lv, 1.6);

    let margin = 14.0 * scale;
    let cell = (min(size.x, size.y) - 2.0 * margin) / f32(n - 1u);
    let origin = (size - vec2<f32>(cell * f32(n - 1u))) * 0.5;
    let fi = f32(col);
    let fb = f32(row);
    let th = 2.1 * sin(fi * 0.071 + t * 0.7) + 1.9 * cos(fb * 0.053 - t * 0.5) + 1.4 * sin((fi + fb) * 0.031 + t * 0.3);
    let d = (l * 26.0 * (0.55 + 0.45 * sin(fi * 0.19 + fb * 0.07 + t * 2.2)) + punch * lv * 10.0) * scale;
    let dir = vec2<f32>(cos(th), sin(th) * 0.8);
    var center = origin + vec2<f32>(fi * cell, f32(n - 1u - row) * cell) + dir * d - vec2<f32>(0.0, l * 6.0 * scale);

    // The red/blue split, per point by its level.
    var tint = vec3<f32>((150.0 + 80.0 * fb / 99.0) / 255.0, (190.0 - 20.0 * fb / 99.0) / 255.0, (200.0 + 30.0 * fb / 99.0) / 255.0);
    var alpha = 0.16 + 0.7 * lv;
    let split = ab * lv * (1.5 + 9.0 * punch + 3.0 * env) * scale;
    if pass_id == 0u {
        center.x += split;
        tint = vec3<f32>(0.95, 0.35, 0.32);
        alpha = select(0.0, 0.5 * lv, lv >= 0.12);
    } else if pass_id == 1u {
        center.x -= split;
        tint = vec3<f32>(0.35, 0.55, 0.98);
        alpha = select(0.0, 0.5 * lv, lv >= 0.12);
    }

    // A dot (bass) or a short line along the flow (drums).
    let corner = array<vec2<f32>, 6>(vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    let c = corner[vi];
    var offset: vec2<f32>;
    let s = (1.0 + 1.8 * lv) * 0.5 * scale;
    if drums {
        let len = (2.5 + 6.0 * lv) * scale;
        let along = normalize(dir + vec2<f32>(1e-4, 0.0));
        let across = vec2<f32>(-along.y, along.x);
        offset = along * c.x * len + across * c.y * 0.6 * scale;
    } else {
        offset = c * s;
    }
    let p = center + offset;

    var o: Out;
    o.pos = vec4<f32>(p.x / size.x * 2.0 - 1.0, 1.0 - p.y / size.y * 2.0, 0.0, 1.0);
    o.color = vec4<f32>(tint, alpha);
    o.local = c;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return i.color;
}
"#;

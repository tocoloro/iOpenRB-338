//! The panel drawn inside an Audio Unit view.
//!
//! The host (AUM) already has a `UIApplication`, so this cannot run eframe's event
//! loop. A display link calls [`Hosted::frame`] with the view's `CAMetalLayer`, and
//! the same [`App`] the desktop window uses is drawn into it. The [`Audio`] half of
//! the player is the host's render callback, not a speaker stream.

use std::ffi::c_void;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Pos2, TouchDeviceId, TouchId, TouchPhase};
use eframe::egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};
use eframe::wgpu;
use orb_engine::song::Device;
use orb_engine::tables::Tables;
use orb_player::Audio;

use crate::app::App;
use crate::link::Link;
use crate::model::Panel;

/// The panel and the GPU that paints it. The main thread is the only user.
pub struct Hosted {
    app: App,
    ctx: egui::Context,
    events: Vec<egui::Event>,
    primary: Option<u64>,
    start: Instant,
    gpu: Option<Gpu>,
    failed: bool,
}

struct Gpu {
    layer: *mut c_void,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

impl Hosted {
    /// The panel, and the audio half the host's callback renders. The song is a short
    /// TR-909 pattern on the built-in kit. The panel shows those steps.
    pub fn new() -> (Self, Audio) {
        let samples = Some(rbm::standard::builtin_samples());
        let (mut link, mut audio) = Link::new(Arc::new(Tables::new()), samples.clone());
        seed_pattern(audio.current_mut());
        link.refresh_patterns(audio.current_mut());
        let mut app = App::with_link(Panel::new(), link, None);
        app.samples = samples;
        app.menu = true;
        app.enable_logo_anim();
        let ctx = egui::Context::default();
        ctx.set_theme(egui::ThemePreference::Dark);
        (
            Self {
                app,
                ctx,
                events: Vec::new(),
                primary: None,
                start: Instant::now(),
                gpu: None,
                failed: false,
            },
            audio,
        )
    }

    /// One touch from the view. `phase` is 0 began, 1 moved, 2 ended, 3 cancelled.
    pub fn touch(&mut self, id: u64, phase: i32, x: f32, y: f32) {
        let pos = Pos2::new(x, y);
        let touch_phase = match phase {
            0 => TouchPhase::Start,
            1 => TouchPhase::Move,
            2 => TouchPhase::End,
            _ => TouchPhase::Cancel,
        };
        self.events.push(egui::Event::Touch {
            device_id: TouchDeviceId(0),
            id: TouchId(id),
            phase: touch_phase,
            pos,
            force: None,
        });
        let primary = match touch_phase {
            TouchPhase::Start if self.primary.is_none() => {
                self.primary = Some(id);
                true
            }
            TouchPhase::Move | TouchPhase::End | TouchPhase::Cancel => self.primary == Some(id),
            TouchPhase::Start => false,
        };
        if !primary {
            return;
        }
        match touch_phase {
            TouchPhase::Start => {
                self.events.push(egui::Event::PointerMoved(pos));
                self.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                });
            }
            TouchPhase::Move => self.events.push(egui::Event::PointerMoved(pos)),
            TouchPhase::End | TouchPhase::Cancel => {
                self.primary = None;
                self.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                });
                self.events.push(egui::Event::PointerGone);
            }
        }
    }

    /// Draws one frame into `layer` (`CAMetalLayer`). Sizes are in points.
    pub fn frame(
        &mut self,
        layer: *mut c_void,
        width_pts: f32,
        height_pts: f32,
        scale: f32,
        bottom: f32,
    ) {
        if self.failed || width_pts < 1.0 || height_pts < 1.0 || scale <= 0.0 {
            return;
        }
        let width = (width_pts * scale).round().max(1.0) as u32;
        let height = (height_pts * scale).round().max(1.0) as u32;
        if self.gpu.as_ref().is_none_or(|g| g.layer != layer) {
            match Gpu::open(layer, width, height) {
                Ok(gpu) => self.gpu = Some(gpu),
                Err(e) => {
                    self.failed = true;
                    log::error!("OpenRB Audio Unit view: {e}");
                    return;
                }
            }
        }
        {
            let Some(gpu) = self.gpu.as_mut() else {
                return;
            };
            if gpu.config.width != width || gpu.config.height != height {
                gpu.config.width = width;
                gpu.config.height = height;
                gpu.surface.configure(&gpu.device, &gpu.config);
            }
        }
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(width_pts, height_pts),
            )),
            time: Some(self.start.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.events),
            focused: true,
            safe_area_insets: Some(egui::SafeAreaInsets(egui::epaint::MarginF32 {
                left: 0.0,
                right: 0.0,
                top: 0.0,
                bottom,
            })),
            ..Default::default()
        };
        if let Some(view) = raw.viewports.get_mut(&egui::ViewportId::ROOT) {
            view.native_pixels_per_point = Some(scale);
            view.focused = Some(true);
        }
        let ctx = self.ctx.clone();
        let app = &mut self.app;
        let full = ctx.run_ui(raw, |ui| {
            egui_extras::install_image_loaders(ui.ctx());
            app.show(ui);
        });
        let paint = self.ctx.tessellate(full.shapes, full.pixels_per_point);
        let screen = ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point: full.pixels_per_point,
        };
        let painted = self
            .gpu
            .as_mut()
            .map(|gpu| gpu.paint(&full.textures_delta, &paint, &screen));
        if let Some(Err(e)) = painted {
            log::error!("OpenRB Audio Unit view: {e}");
            self.gpu = None;
        }
    }

    /// Drops the metal surface. The song stays.
    pub fn detach(&mut self) {
        self.gpu = None;
        self.failed = false;
    }
}

/// Bass on the beat, snare on the backbeat, closed hats on the offbeats.
fn seed_pattern(engine: &mut orb_engine::frame::Engine) {
    for step in [0, 4, 8, 12] {
        engine.step_field(Device::Tr909, 0, step, 1, 1);
    }
    for step in [4, 12] {
        engine.step_field(Device::Tr909, 0, step, 2, 1);
    }
    for step in [2, 6, 10, 14] {
        engine.step_field(Device::Tr909, 0, step, 8, 1);
    }
}

impl Gpu {
    fn open(layer: *mut c_void, width: u32, height: u32) -> Result<Self, String> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::METAL;
        let instance = wgpu::Instance::new(desc);
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
        }
        .map_err(|e| e.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| e.to_string())?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("openrb-au"),
            required_limits: limits_for(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        let config = surface
            .get_default_config(&adapter, width, height)
            .ok_or("the metal layer has no surface configuration")?;
        surface.configure(&device, &config);
        let renderer = Renderer::new(&device, config.format, RendererOptions::default());
        Ok(Self {
            layer,
            device,
            queue,
            surface,
            config,
            renderer,
        })
    }

    fn paint(
        &mut self,
        delta: &egui::epaint::textures::TexturesDelta,
        paint: &[egui::epaint::ClippedPrimitive],
        screen: &ScreenDescriptor,
    ) -> Result<(), String> {
        for (id, images) in &delta.set {
            for image in images {
                self.renderer
                    .update_texture(&self.device, &self.queue, *id, image);
            }
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("openrb-au"),
            });
        let extra =
            self.renderer
                .update_buffers(&self.device, &self.queue, &mut encoder, paint, screen);
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err("the metal surface was lost".to_string());
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return Ok(()),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("openrb-au"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0x1d as f64 / 255.0,
                                g: 0x21 as f64 / 255.0,
                                b: 0x27 as f64 / 255.0,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, paint, screen);
        }
        self.queue
            .submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        self.queue.present(frame);
        for id in &delta.free {
            self.renderer.free_texture(id);
        }
        Ok(())
    }
}

/// WebGPU's default asks for 16 inter-stage shader variables. The iPad 7th
/// generation reports 15, and the device request then fails.
fn limits_for(supported: wgpu::Limits) -> wgpu::Limits {
    wgpu::Limits::default().or_worse_values_from(&supported)
}

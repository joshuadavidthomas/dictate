//! Wayland layer-shell integration

use anyhow::{anyhow, Result};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_seat,
    delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{Capability, SeatHandler, SeatState},
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::time::{Duration, Instant};
use wayland_client::{
    protocol::{wl_buffer::WlBuffer, wl_output, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use crate::osd::{
    render,
    socket::{OsdMessage, OsdSocket},
    state::{OsdState, State as OsdStateEnum},
};

const OSD_WIDTH: u32 = 420;
const OSD_HEIGHT: u32 = 36;
const SHADOW_PADDING: u32 = 10; // Extra space around edges for shadow

/// Main OSD application state
pub struct OsdApp {
    // Registry state
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm: Shm,
    layer_shell: LayerShell,

    // OSD specific
    osd_state: OsdState,
    socket: OsdSocket,
    last_frame: Instant,

    // Wayland surface
    layer_surface: Option<LayerSurface>,
    pool: Option<SlotPool>,
    width: u32,
    height: u32,
    need_frame: bool,
    pub exit: bool,
    configured: bool, // Has the compositor sent us a configure event?
}

impl OsdApp {
    pub fn new(
        globals: wayland_client::globals::GlobalList,
        qh: &QueueHandle<Self>,
        socket_path: String,
    ) -> Result<Self> {
        let registry_state = RegistryState::new(&globals);
        let seat_state = SeatState::new(&globals, qh);
        let output_state = OutputState::new(&globals, qh);
        let compositor_state = CompositorState::bind(&globals, qh)?;
        let shm = Shm::bind(&globals, qh)?;
        let layer_shell = LayerShell::bind(&globals, qh)?;

        Ok(Self {
            registry_state,
            seat_state,
            output_state,
            compositor_state,
            shm,
            layer_shell,
            osd_state: OsdState::new(),
            socket: OsdSocket::new(socket_path),
            last_frame: Instant::now(),
            layer_surface: None,
            pool: None,
            width: OSD_WIDTH + SHADOW_PADDING * 2,   // Add padding for shadow on both sides
            height: OSD_HEIGHT + SHADOW_PADDING * 2, // Add padding for shadow top and bottom
            need_frame: false, // Don't draw until configured
            exit: false,
            configured: false,
        })
    }

    pub fn create_layer_surface(&mut self, qh: &QueueHandle<Self>) -> Result<()> {
        let surface = self.compositor_state.create_surface(qh);

        let layer_surface = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("dictate-osd"),
            None, // None = compositor chooses output
        );

        // Configure layer surface
        layer_surface.set_anchor(Anchor::TOP);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.set_size(self.width, self.height);
        layer_surface.set_exclusive_zone(0);

        // Commit initial configuration
        layer_surface.wl_surface().commit();

        self.layer_surface = Some(layer_surface);

        Ok(())
    }

    pub fn handle_socket_messages(&mut self) {
        // Try to reconnect if needed
        if self.socket.should_reconnect(Instant::now()) {
            match self.socket.connect() {
                Ok(()) => {
                    eprintln!("OSD: Connected to server");
                }
                Err(e) => {
                    eprintln!("OSD: Failed to connect: {}", e);
                    self.socket.schedule_reconnect();
                    self.osd_state.set_error();
                    self.need_frame = true;
                }
            }
        }

        // Read messages
        loop {
            match self.socket.read_message() {
                Ok(Some(msg)) => {
                    self.handle_message(msg);
                }
                Ok(None) => break, // No message available
                Err(e) => {
                    eprintln!("OSD: Socket error: {}", e);
                    self.socket.schedule_reconnect();
                    self.osd_state.set_error();
                    self.need_frame = true;
                    break;
                }
            }
        }

        // Check for timeout
        if self.osd_state.has_timeout() && self.osd_state.state != OsdStateEnum::Error {
            self.osd_state.set_error();
            self.need_frame = true;
        }
    }

    fn handle_message(&mut self, msg: OsdMessage) {
        eprintln!("OSD: Handling message: {:?}", msg);
        match msg {
            OsdMessage::Status {
                state,
                level,
                idle_hot,
                ts: _,
            } => {
                eprintln!("OSD: Status message - state={}, level={}, idle_hot={}", state, level, idle_hot);
                let osd_state = parse_state(&state);
                self.osd_state.update_state(osd_state, idle_hot);
                self.osd_state.update_level(level);
                self.need_frame = true;
                eprintln!("OSD: need_frame set to true");
            }
            OsdMessage::State {
                state,
                idle_hot,
                ts: _,
            } => {
                eprintln!("OSD: State message - state={}, idle_hot={}", state, idle_hot);
                let osd_state = parse_state(&state);
                self.osd_state.update_state(osd_state, idle_hot);
                self.need_frame = true;
                eprintln!("OSD: need_frame set to true");
            }
            OsdMessage::Level { v, ts: _ } => {
                eprintln!("OSD: Level message - v={}", v);
                self.osd_state.update_level(v);
                self.need_frame = true;
                eprintln!("OSD: need_frame set to true");
            }
        }
    }

    pub fn draw(&mut self, _qh: &QueueHandle<Self>) -> Result<()> {
        eprintln!("OSD: draw() called");
        let Some(layer_surface) = &self.layer_surface else {
            eprintln!("OSD: No layer surface!");
            return Ok(());
        };
        eprintln!("OSD: Layer surface exists, proceeding with draw");

        // Initialize pool if needed
        if self.pool.is_none() {
            let pool = SlotPool::new(
                (self.width * self.height * 4) as usize,
                &self.shm,
            )?;
            self.pool = Some(pool);
        }

        let pool = self.pool.as_mut().unwrap();

        // Tick animations
        let visual = self.osd_state.tick(Instant::now());
        eprintln!("OSD: Visual state: {:?}, ratio: {}", visual.state, visual.content_ratio);

        // Create pixmap and render
        let mut pixmap =
            tiny_skia::Pixmap::new(self.width, self.height).ok_or_else(|| anyhow!("Failed to create pixmap"))?;
        eprintln!("OSD: Pixmap created ({}x{})", self.width, self.height);

        render::render(&mut pixmap, &visual, self.width as f32);
        eprintln!("OSD: Render complete");

        // Get buffer from pool
        let (buffer, canvas) = pool
            .create_buffer(
                self.width as i32,
                self.height as i32,
                (self.width * 4) as i32,
                wl_shm::Format::Argb8888,
            )?;

        // Copy pixmap data to canvas (convert RGBA to ARGB)
        let pixmap_data = pixmap.data();
        for i in 0..(self.width * self.height) as usize {
            let idx = i * 4;
            let r = pixmap_data[idx];
            let g = pixmap_data[idx + 1];
            let b = pixmap_data[idx + 2];
            let a = pixmap_data[idx + 3];

            // Convert RGBA to ARGB (not premultiplied for simplicity)
            canvas[idx] = b;
            canvas[idx + 1] = g;
            canvas[idx + 2] = r;
            canvas[idx + 3] = a;
        }

        // Attach buffer and commit
        let wl_buffer: &WlBuffer = buffer.wl_buffer();
        layer_surface
            .wl_surface()
            .attach(Some(wl_buffer), 0, 0);

        layer_surface
            .wl_surface()
            .damage_buffer(0, 0, self.width as i32, self.height as i32);

        layer_surface.wl_surface().commit();

        self.last_frame = Instant::now();
        self.need_frame = false;
        eprintln!("OSD: Frame committed, need_frame set to false");
        Ok(())
    }

    pub fn should_draw(&self) -> bool {
        // Don't draw until we've been configured by the compositor
        if !self.configured {
            return false;
        }
        
        let result = self.need_frame
            || self.osd_state.is_animating()
            || self.last_frame.elapsed() > Duration::from_millis(16);
        
        if !result {
            eprintln!("OSD: should_draw=false (need_frame={}, is_animating={}, elapsed={}ms)", 
                self.need_frame, 
                self.osd_state.is_animating(),
                self.last_frame.elapsed().as_millis()
            );
        }
        result
    }
}

fn parse_state(state_str: &str) -> OsdStateEnum {
    match state_str {
        "Idle" => OsdStateEnum::Idle,
        "Recording" => OsdStateEnum::Recording,
        "Transcribing" => OsdStateEnum::Transcribing,
        "Error" => OsdStateEnum::Error,
        _ => OsdStateEnum::Idle,
    }
}

// Implement required trait delegates
delegate_compositor!(OsdApp);
delegate_output!(OsdApp);
delegate_shm!(OsdApp);
delegate_seat!(OsdApp);
delegate_layer!(OsdApp);
delegate_registry!(OsdApp);

impl CompositorHandler for OsdApp {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
        // Handle scale factor changes if needed
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        eprintln!("OSD: Compositor frame callback received!");
        self.need_frame = true; // Compositor is ready for another frame
        if let Err(e) = self.draw(qh) {
            eprintln!("OSD: Draw error: {}", e);
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for OsdApp {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for OsdApp {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        eprintln!("OSD: Received configure event: {:?}", configure.new_size);
        
        let (width, height) = configure.new_size;
        if width > 0 && height > 0 {
            self.width = width;
            self.height = height;
            self.pool = None; // Recreate pool with new size
        }

        // Mark as configured and request a frame
        self.configured = true;
        self.need_frame = true;
        
        eprintln!("OSD: Configured! Size: {}x{}", self.width, self.height);

        // Initial draw after configure
        if let Err(e) = self.draw(qh) {
            eprintln!("OSD: Initial draw error: {}", e);
        }
    }
}

impl SeatHandler for OsdApp {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl ShmHandler for OsdApp {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for OsdApp {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

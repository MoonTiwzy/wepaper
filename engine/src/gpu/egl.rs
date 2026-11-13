//! EGL: the display connection, the one context, and window surfaces.

use std::ffi::c_void;
use std::rc::Rc;

use khronos_egl as egl;

use super::{Gl, GpuError};

type Egl = egl::DynamicInstance<egl::EGL1_5>;

const PLATFORM_DEVICE: egl::Enum = 0x313F;
const PLATFORM_SURFACELESS: egl::Enum = 0x31DD;
const PLATFORM_WAYLAND: egl::Enum = 0x31D8;
const DRM_DEVICE_FILE: egl::Int = 0x3233;

/// The process's OpenGL context and the display it lives on.
pub struct Context {
    egl: Rc<Egl>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    gl: Gl,
    /// A human-readable note on what the context runs on.
    pub description: String,
}

/// A window the context can draw to.
pub struct Surface {
    egl: Rc<Egl>,
    display: egl::Display,
    surface: egl::Surface,
}

fn fail(what: &str, error: impl std::fmt::Display) -> GpuError {
    GpuError(format!("{what}: {error}"))
}

fn load() -> Result<Rc<Egl>, GpuError> {
    // SAFETY: libEGL's initialisers have no preconditions; the library stays loaded
    // for as long as the returned instance lives.
    let egl = unsafe { Egl::load_required() }.map_err(|e| fail("libEGL could not be loaded", e))?;
    Ok(Rc::new(egl))
}

impl Context {
    /// A context that draws to no window: for rendering frames to files and for tests.
    ///
    /// `device` picks a GPU by its DRM node (`/dev/dri/card1`, `/dev/dri/renderD128`);
    /// without it the first device that gives a desktop OpenGL context is used.
    pub fn headless(device: Option<&str>) -> Result<Self, GpuError> {
        let egl = load()?;
        let mut errors = Vec::new();
        for (display, name) in headless_displays(&egl, device) {
            match Self::on_display(egl.clone(), display, false, name.clone()) {
                Ok(context) => return Ok(context),
                Err(error) => errors.push(format!("{name}: {error}")),
            }
        }
        Err(GpuError(format!("no GPU gave an OpenGL 3.3 context ({})", errors.join("; "))))
    }

    /// A context on a Wayland connection.
    ///
    /// # Safety
    /// `wl_display` must be a live `wl_display*` that outlives the context.
    pub unsafe fn wayland(wl_display: *mut c_void) -> Result<Self, GpuError> {
        let egl = load()?;
        // SAFETY: the caller vouches for `wl_display`.
        let display = unsafe { egl.get_platform_display(PLATFORM_WAYLAND, wl_display, &[egl::ATTRIB_NONE]) }
            .map_err(|e| fail("no EGL display for the Wayland connection", e))?;
        Self::on_display(egl, display, true, "wayland".to_owned())
    }

    fn on_display(egl: Rc<Egl>, display: egl::Display, window: bool, name: String) -> Result<Self, GpuError> {
        egl.initialize(display).map_err(|e| fail("eglInitialize", e))?;
        egl.bind_api(egl::OPENGL_API).map_err(|e| fail("desktop OpenGL is not available", e))?;
        let surface_type = if window { egl::WINDOW_BIT } else { egl::PBUFFER_BIT };
        #[rustfmt::skip]
        let config_attributes = [
            egl::SURFACE_TYPE, surface_type,
            egl::RENDERABLE_TYPE, egl::OPENGL_BIT,
            egl::RED_SIZE, 8, egl::GREEN_SIZE, 8, egl::BLUE_SIZE, 8,
            egl::NONE,
        ];
        let config = egl
            .choose_first_config(display, &config_attributes)
            .map_err(|e| fail("eglChooseConfig", e))?
            .ok_or_else(|| GpuError("no matching EGL config".to_owned()))?;
        #[rustfmt::skip]
        let context_attributes = [
            egl::CONTEXT_MAJOR_VERSION, 3, egl::CONTEXT_MINOR_VERSION, 3,
            egl::CONTEXT_OPENGL_PROFILE_MASK, egl::CONTEXT_OPENGL_CORE_PROFILE_BIT,
            egl::NONE,
        ];
        let context = egl.create_context(display, config, None, &context_attributes).map_err(|e| fail("eglCreateContext", e))?;
        egl.make_current(display, None, None, Some(context)).map_err(|e| fail("eglMakeCurrent without a surface", e))?;
        // SAFETY: the context made current above is the one these functions belong to.
        let raw = unsafe {
            glow::Context::from_loader_function(|symbol| {
                egl.get_proc_address(symbol).map_or(std::ptr::null(), |address| address as *const c_void)
            })
        };
        let gl = Gl::new(raw);
        let description = format!("{} ({}, {name})", gl.caps().renderer, gl.caps().version);
        Ok(Self { egl, display, config, context, gl, description })
    }

    /// The handle everything else draws through.
    pub fn gl(&self) -> &Gl {
        &self.gl
    }

    /// A surface for a `wl_egl_window`.
    ///
    /// # Safety
    /// `wl_egl_window` must be a live `wl_egl_window*` that outlives the surface.
    pub unsafe fn window_surface(&self, wl_egl_window: *mut c_void) -> Result<Surface, GpuError> {
        // SAFETY: the caller vouches for the window.
        let surface = unsafe { self.egl.create_platform_window_surface(self.display, self.config, wl_egl_window, &[egl::ATTRIB_NONE]) }
            .map_err(|e| fail("eglCreatePlatformWindowSurface", e))?;
        Ok(Surface { egl: self.egl.clone(), display: self.display, surface })
    }

    /// Directs drawing at `surface`, or at nothing but framebuffer objects.
    pub fn make_current(&self, surface: Option<&Surface>) -> Result<(), GpuError> {
        let surface = surface.map(|s| s.surface);
        self.egl.make_current(self.display, surface, surface, Some(self.context)).map_err(|e| fail("eglMakeCurrent", e))?;
        self.gl.surface_changed();
        Ok(())
    }

    /// Shows what was drawn to `surface`. Never waits for the display: pacing is the caller's.
    pub fn present(&self, surface: &Surface) -> Result<(), GpuError> {
        self.egl.swap_buffers(self.display, surface.surface).map_err(|e| fail("eglSwapBuffers", e))
    }

    /// Makes [`present`](Self::present) return at once instead of waiting for the display.
    pub fn disable_vsync(&self) {
        let _ = self.egl.swap_interval(self.display, 0);
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        let _ = self.egl.make_current(self.display, None, None, None);
        let _ = self.egl.destroy_context(self.display, self.context);
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        let _ = self.egl.destroy_surface(self.display, self.surface);
    }
}

/// The displays a headless context can be made on, most specific first.
fn headless_displays(egl: &Egl, wanted: Option<&str>) -> Vec<(egl::Display, String)> {
    type QueryDevices = unsafe extern "system" fn(egl::Int, *mut *mut c_void, *mut egl::Int) -> egl::Boolean;
    type QueryDeviceString = unsafe extern "system" fn(*mut c_void, egl::Int) -> *const std::ffi::c_char;
    let mut displays = Vec::new();
    if let (Some(query), Some(describe)) = (egl.get_proc_address("eglQueryDevicesEXT"), egl.get_proc_address("eglQueryDeviceStringEXT")) {
        // SAFETY: these are the documented signatures of EGL_EXT_device_enumeration
        // and EGL_EXT_device_query, which the driver said it has by returning them.
        let (query, describe): (QueryDevices, QueryDeviceString) = unsafe { (std::mem::transmute(query), std::mem::transmute(describe)) };
        let mut devices = [std::ptr::null_mut(); 16];
        let mut count = 0;
        // SAFETY: `devices` has room for the 16 entries asked for.
        if unsafe { query(16, devices.as_mut_ptr(), &mut count) } != 0 {
            for &device in &devices[..count.clamp(0, 16) as usize] {
                // SAFETY: `device` came from the query above; the string is the driver's and outlives the call.
                let file = unsafe {
                    let text = describe(device, DRM_DEVICE_FILE);
                    if text.is_null() { String::new() } else { std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned() }
                };
                if wanted.is_some_and(|w| !same_gpu(w, &file)) {
                    continue;
                }
                // SAFETY: a device handle is the native display of the device platform.
                if let Ok(display) = unsafe { egl.get_platform_display(PLATFORM_DEVICE, device, &[egl::ATTRIB_NONE]) } {
                    displays.push((display, if file.is_empty() { "device".to_owned() } else { file }));
                }
            }
        }
    }
    if wanted.is_none() {
        // SAFETY: the surfaceless platform takes no native display.
        if let Ok(display) = unsafe { egl.get_platform_display(PLATFORM_SURFACELESS, std::ptr::null_mut(), &[egl::ATTRIB_NONE]) } {
            displays.push((display, "surfaceless".to_owned()));
        }
    }
    displays
}

/// Whether two DRM nodes (`card1`, `renderD128`) belong to one GPU.
fn same_gpu(a: &str, b: &str) -> bool {
    let device = |node: &str| {
        let name = std::path::Path::new(node).file_name()?.to_owned();
        std::fs::canonicalize(std::path::Path::new("/sys/class/drm").join(name).join("device")).ok()
    };
    a == b || (device(a).is_some() && device(a) == device(b))
}

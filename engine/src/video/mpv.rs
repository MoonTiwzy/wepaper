//! The part of libmpv's C API the engine uses, bound by hand.
//!
//! The API is small and stable (opaque handles, strings, a few plain structs), so
//! there is nothing a generator would add but build time.

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

/// `mpv_handle`: one player.
#[repr(C)]
pub(crate) struct Handle {
    _opaque: [u8; 0],
}

/// `mpv_render_context`: a player's renderer on our GL context.
#[repr(C)]
pub(crate) struct RenderContext {
    _opaque: [u8; 0],
}

/// `mpv_render_param`: a tagged pointer.
#[repr(C)]
pub(crate) struct RenderParam {
    pub(crate) kind: c_int,
    pub(crate) data: *mut c_void,
}

/// `mpv_opengl_init_params`.
#[repr(C)]
pub(crate) struct OpenglInitParams {
    pub(crate) get_proc_address: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    pub(crate) get_proc_address_ctx: *mut c_void,
}

/// `mpv_opengl_fbo`.
#[repr(C)]
pub(crate) struct OpenglFbo {
    pub(crate) fbo: c_int,
    pub(crate) w: c_int,
    pub(crate) h: c_int,
    pub(crate) internal_format: c_int,
}

/// `mpv_event`.
#[repr(C)]
pub(crate) struct Event {
    pub(crate) event_id: c_int,
    pub(crate) error: c_int,
    pub(crate) reply_userdata: u64,
    pub(crate) data: *mut c_void,
}

pub(crate) const RENDER_PARAM_INVALID: c_int = 0;
pub(crate) const RENDER_PARAM_API_TYPE: c_int = 1;
pub(crate) const RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
pub(crate) const RENDER_PARAM_OPENGL_FBO: c_int = 3;
pub(crate) const RENDER_PARAM_FLIP_Y: c_int = 4;
pub(crate) const RENDER_PARAM_BLOCK_FOR_TARGET_TIME: c_int = 12;
pub(crate) const RENDER_UPDATE_FRAME: u64 = 1;
pub(crate) const EVENT_NONE: c_int = 0;
pub(crate) const EVENT_SHUTDOWN: c_int = 1;
pub(crate) const EVENT_END_FILE: c_int = 7;
pub(crate) const EVENT_FILE_LOADED: c_int = 8;
pub(crate) const FORMAT_INT64: c_int = 4;
pub(crate) const FORMAT_DOUBLE: c_int = 5;

/// The functions, resolved once.
pub(crate) struct Api {
    _library: libloading::Library,
    pub(crate) create: unsafe extern "C" fn() -> *mut Handle,
    pub(crate) initialize: unsafe extern "C" fn(*mut Handle) -> c_int,
    pub(crate) terminate_destroy: unsafe extern "C" fn(*mut Handle),
    pub(crate) set_option_string: unsafe extern "C" fn(*mut Handle, *const c_char, *const c_char) -> c_int,
    pub(crate) set_property_string: unsafe extern "C" fn(*mut Handle, *const c_char, *const c_char) -> c_int,
    pub(crate) get_property: unsafe extern "C" fn(*mut Handle, *const c_char, c_int, *mut c_void) -> c_int,
    pub(crate) command: unsafe extern "C" fn(*mut Handle, *mut *const c_char) -> c_int,
    pub(crate) wait_event: unsafe extern "C" fn(*mut Handle, f64) -> *mut Event,
    pub(crate) error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    pub(crate) render_context_create: unsafe extern "C" fn(*mut *mut RenderContext, *mut Handle, *mut RenderParam) -> c_int,
    pub(crate) render_context_set_update_callback: unsafe extern "C" fn(*mut RenderContext, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    pub(crate) render_context_update: unsafe extern "C" fn(*mut RenderContext) -> u64,
    pub(crate) render_context_render: unsafe extern "C" fn(*mut RenderContext, *mut RenderParam) -> c_int,
    pub(crate) render_context_free: unsafe extern "C" fn(*mut RenderContext),
}

// SAFETY: the table holds function pointers into a library that stays loaded, and the
// library handle itself; libmpv's functions may be called from any thread.
unsafe impl Send for Api {}
// SAFETY: as above; nothing in the table is mutated after it is built.
unsafe impl Sync for Api {}

/// libmpv, loaded on first use; `None` when it is not installed.
pub(crate) fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let api = ["libmpv.so.2", "libmpv.so"].into_iter().find_map(load);
        if api.is_none() {
            log::warn!("libmpv is not installed: no video, no sound");
        }
        api
    })
    .as_ref()
}

fn load(name: &str) -> Option<Api> {
    // SAFETY: loading libmpv runs its initialisers, which have no preconditions. Each
    // symbol is given the signature mpv's headers declare for it.
    unsafe {
        let library = libloading::Library::new(name).ok()?;
        macro_rules! symbol {
            ($name:literal) => {
                *library.get(concat!($name, "\0").as_bytes()).ok()?
            };
        }
        Some(Api {
            create: symbol!("mpv_create"),
            initialize: symbol!("mpv_initialize"),
            terminate_destroy: symbol!("mpv_terminate_destroy"),
            set_option_string: symbol!("mpv_set_option_string"),
            set_property_string: symbol!("mpv_set_property_string"),
            get_property: symbol!("mpv_get_property"),
            command: symbol!("mpv_command"),
            wait_event: symbol!("mpv_wait_event"),
            error_string: symbol!("mpv_error_string"),
            render_context_create: symbol!("mpv_render_context_create"),
            render_context_set_update_callback: symbol!("mpv_render_context_set_update_callback"),
            render_context_update: symbol!("mpv_render_context_update"),
            render_context_render: symbol!("mpv_render_context_render"),
            render_context_free: symbol!("mpv_render_context_free"),
            _library: library,
        })
    }
}

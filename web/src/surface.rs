//! The layer-shell window: one output's desktop background.

use gtk4 as gtk;

use gtk::prelude::*;
use gtk::{cairo, gdk};
use gtk4_layer_shell::{Edge, KeyboardMode, LayerShell};

/// Which of the two layers under the windows the wallpaper sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Below everything: where a wallpaper normally lives.
    Background,
    /// One above, for a compositor whose background layer is taken.
    Bottom,
}

impl Layer {
    /// The layer called `name` on the command line.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "background" => Some(Self::Background),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }
}

/// Connects GTK to the Wayland compositor; an error says why there is nothing to draw on.
pub fn connect() -> Result<gdk::Display, String> {
    // Layer surfaces exist on Wayland only; an X11 fallback would put a window on screen.
    gdk::set_allowed_backends("wayland");
    gtk::init().map_err(|error| format!("no Wayland display: {error}"))?;
    if !gtk4_layer_shell::is_supported() {
        return Err("the compositor has no wlr-layer-shell".to_owned());
    }
    gdk::Display::default().ok_or_else(|| "no Wayland display".to_owned())
}

/// The connector names of the outputs `display` has now.
pub fn connectors(display: &gdk::Display) -> Vec<String> {
    monitors(display).iter().filter_map(|monitor| monitor.connector()).map(String::from).collect()
}

/// The output whose connector is `name`.
pub fn monitor_named(display: &gdk::Display, name: &str) -> Option<gdk::Monitor> {
    monitors(display).into_iter().find(|monitor| monitor.connector().as_deref() == Some(name))
}

fn monitors(display: &gdk::Display) -> Vec<gdk::Monitor> {
    display
        .monitors()
        .into_iter()
        .filter_map(|monitor| monitor.ok()?.downcast::<gdk::Monitor>().ok())
        .collect()
}

/// The refresh rate of `monitor` in whole frames a second; 60 when it does not say.
pub fn refresh_rate(monitor: &gdk::Monitor) -> u32 {
    match monitor.refresh_rate() {
        millihertz if millihertz > 0 => ((millihertz + 500) / 1000).max(1) as u32,
        _ => 60,
    }
}

/// What a wallpaper's surface is made with.
pub struct SurfaceOptions<'a> {
    /// The output to cover.
    pub monitor: &'a gdk::Monitor,
    /// The layer to sit on.
    pub layer: Layer,
    /// The layer surface's namespace, which compositor rules match on.
    pub namespace: &'a str,
    /// Whether pointer input goes past the surface.
    pub passthrough: bool,
}

/// A window covering one output, on a layer under the windows, showing `content`.
///
/// It is not presented yet: the caller does that once it is wired up.
pub fn create(options: &SurfaceOptions, content: &impl IsA<gtk::Widget>) -> gtk::Window {
    let window = gtk::Window::new();
    window.set_decorated(false);
    window.set_child(Some(content));

    window.init_layer_shell();
    window.set_monitor(Some(options.monitor));
    window.set_namespace(Some(options.namespace));
    window.set_layer(match options.layer {
        Layer::Background => gtk4_layer_shell::Layer::Background,
        Layer::Bottom => gtk4_layer_shell::Layer::Bottom,
    });
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    // -1: cover the whole output, panels' reserved areas included.
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::None);

    paint_black(&window);
    if options.passthrough {
        content.set_can_target(false);
        // The surface exists from realize on; an empty input region there makes the
        // compositor send pointer events to whatever is beneath instead.
        window.connect_realize(|window| {
            if let Some(surface) = window.surface() {
                surface.set_input_region(Some(&cairo::Region::create()));
            }
        });
    }
    window
}

/// Black behind the page, so that a wallpaper that is still loading is not a white flash.
fn paint_black(window: &gtk::Window) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string("window.wepaper-web { background: black; }");
    window.add_css_class("wepaper-web");
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::WidgetExt::display(window),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_layers_under_the_windows_have_names() {
        assert_eq!(Layer::from_name("background"), Some(Layer::Background));
        assert_eq!(Layer::from_name("bottom"), Some(Layer::Bottom));
        assert_eq!(Layer::from_name("top"), None);
        assert_eq!(Layer::from_name("overlay"), None);
        assert_eq!(Layer::from_name("Background"), None);
    }
}

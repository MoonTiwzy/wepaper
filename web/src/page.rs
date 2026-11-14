//! The web view: one wallpaper's page, kept inside its folder.
//!
//! The page is opened as `file://`, with access between local files allowed, and not
//! through a custom scheme. Wallpaper Engine itself opens web wallpapers from disk, so
//! that is what they are written against: relative URLs, `XMLHttpRequest`/`fetch` of
//! their own files and WebGL textures from them work the way their authors saw, and
//! `<audio>`/`<video>` play through WebKit's native file source with seeking, which
//! WebKitGTK's custom schemes do not give media.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4 as gtk;

use gtk::{gdk, glib};
use webkit6::prelude::*;
use webkit6::{
    AutoplayPolicy, CacheModel, HardwareAccelerationPolicy, LoadEvent, NavigationPolicyDecision,
    NavigationType, NetworkError, NetworkSession, PolicyDecisionType, PolicyError, ResponsePolicyDecision,
    Settings, UserContentInjectedFrames, UserContentManager, UserScript, UserScriptInjectionTime, WebContext,
    WebView, WebsitePolicies,
};

use crate::bridge::{self, FromPage, PageState};
use crate::log;

/// Sets what WebKit and GTK read from the environment, where the daemon has not set it.
///
/// Has to be the first thing `main` does: the environment may only be written while
/// no other thread exists.
pub fn prepare_environment() {
    let mut defaults = vec![
        // A wallpaper is not something a screen reader walks; no accessibility bus.
        ("GTK_A11Y", "none"),
    ];
    // With NVIDIA's driver, WebKit's OpenGL video sink shows every <video> as a flat
    // green rectangle (seen with WebKitGTK 2.54 on driver 615). Without the sink frames
    // are converted on the CPU, which costs a little and is always right.
    if Path::new("/sys/module/nvidia_drm").exists() {
        defaults.push(("WEBKIT_GST_DISABLE_GL_SINK", "1"));
    }
    for (name, value) in defaults {
        if std::env::var_os(name).is_none() {
            // SAFETY: called at the top of `main`, before GTK starts and before any
            // thread is spawned, so nothing reads the environment concurrently.
            unsafe { std::env::set_var(name, value) };
        }
    }
}

/// What happens to a page, for whoever shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum PageEvent {
    /// The document and what it pulls in have loaded.
    Loaded,
    /// The page itself could not be loaded.
    Failed(String),
    /// The process drawing the page died.
    Crashed(String),
    /// The bridge in the page said something.
    Message(FromPage),
}

/// Where a wallpaper's page is: checked paths, and the URLs they are opened by.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    /// The wallpaper's folder, resolved.
    dir: PathBuf,
    /// The `file://` URL of the folder, without a trailing slash.
    root: String,
    /// The URL of the page inside it.
    entry: String,
}

impl Location {
    /// The page `file` of the wallpaper in `dir`, which has to exist inside it.
    pub fn find(dir: &Path, file: &str) -> Result<Self, String> {
        let dir = dir.canonicalize().map_err(|error| format!("{}: {error}", dir.display()))?;
        let page = dir.join(file);
        let page = page.canonicalize().map_err(|error| format!("{}: {error}", page.display()))?;
        if !page.is_file() || !page.starts_with(&dir) {
            return Err(format!("{} is not a page of the wallpaper in {}", page.display(), dir.display()));
        }
        let url = |path: &Path| {
            glib::filename_to_uri(path, None)
                .map(|uri| uri.trim_end_matches('/').to_owned())
                .map_err(|error| format!("{}: {error}", path.display()))
        };
        let (root, entry) = (url(&dir)?, url(&page)?);
        Ok(Self { dir, root, entry })
    }
}

/// A wallpaper's page in a web view.
pub struct Page {
    view: WebView,
    scripts: UserContentManager,
    /// The `file://` URL of the wallpaper's folder, without a trailing slash.
    root: String,
    /// The URL of the page inside it.
    entry: String,
}

impl Page {
    /// A view for the page at `location`; nothing is loaded yet. GTK has to be running.
    pub fn new(location: Location) -> Self {
        let Location { dir, root, entry } = location;

        let context = WebContext::new();
        // The smallest caches WebKit has: a wallpaper is one document that never navigates.
        context.set_cache_model(CacheModel::DocumentViewer);
        context.set_spell_checking_enabled(false);
        // The web process is sandboxed and its media player reads files itself, not
        // through the network process: without this, pictures load and videos do not.
        context.add_path_to_sandbox(&dir, true);

        let scripts = UserContentManager::new();
        scripts.register_script_message_handler(bridge::HANDLER, None);

        let policies = WebsitePolicies::builder().autoplay(AutoplayPolicy::Allow).build();
        let view = WebView::builder()
            .web_context(&context)
            // Nothing on disk: several outputs run several of these processes, and
            // WebKit's storage is not made for two network processes in one directory.
            .network_session(&NetworkSession::new_ephemeral())
            .settings(&settings())
            .user_content_manager(&scripts)
            .website_policies(&policies)
            .build();
        view.set_background_color(&gdk::RGBA::BLACK);
        refuse_everything_but_the_page(&view, &root);

        Self { view, scripts, root, entry }
    }

    /// The widget to put on the surface.
    pub fn widget(&self) -> &WebView {
        &self.view
    }

    /// Reports what happens to the page to `handler`.
    pub fn on_event(&self, handler: impl Fn(PageEvent) + 'static) {
        let handler = Rc::new(handler);

        let (root, entry) = (self.root.clone(), self.entry.clone());
        let report = handler.clone();
        self.view.connect_load_changed(move |view, event| {
            let uri = view.uri().unwrap_or_default();
            match event {
                // The policy below refuses to leave the wallpaper; should a load get
                // past it all the same, the wallpaper is put back.
                LoadEvent::Committed if !stays_inside(&uri, &root) => {
                    log::warning(&format!("the page left for {uri}: loading the wallpaper again"));
                    view.load_uri(&entry);
                }
                LoadEvent::Finished if stays_inside(&uri, &root) => report(PageEvent::Loaded),
                _ => {}
            }
        });

        let entry = self.entry.clone();
        let report = handler.clone();
        self.view.connect_load_failed(move |_, _, uri, error| {
            // Cancelled is a navigation the policy refused.
            if !error.matches(NetworkError::Cancelled)
                && !error.matches(PolicyError::FrameLoadInterruptedByPolicyChange)
            {
                if uri == entry {
                    report(PageEvent::Failed(format!("{uri}: {error}")));
                } else {
                    log::warning(&format!("{uri} did not load: {error}"));
                }
            }
            false
        });

        let report = handler.clone();
        self.view.connect_web_process_terminated(move |_, reason| {
            report(PageEvent::Crashed(format!("the web process ended: {reason:?}")));
        });

        self.scripts.connect_script_message_received(Some(bridge::HANDLER), move |_, value| {
            if let Some(message) = FromPage::decode(&value.to_str()) {
                handler(PageEvent::Message(message));
            }
        });
    }

    /// Loads the page (again), starting it in `state`.
    pub fn load(&self, state: &PageState) {
        self.scripts.remove_all_scripts();
        self.scripts.add_script(&UserScript::new(
            &bridge::document_start_script(state),
            UserContentInjectedFrames::TopFrame,
            UserScriptInjectionTime::Start,
            &[],
            &[],
        ));
        self.view.load_uri(&self.entry);
    }

    /// Runs `script`, one of [`bridge`]'s messages, in the page.
    pub fn run(&self, script: &str) {
        self.view.evaluate_javascript(script, None, None, gtk::gio::Cancellable::NONE, |result| {
            if let Err(error) = result {
                log::debug(&format!("a message did not reach the page: {error}"));
            }
        });
    }

    /// Silences the page or lets it sound.
    pub fn set_muted(&self, muted: bool) {
        self.view.set_is_muted(muted);
    }
}

/// The settings of a wallpaper's view: what a page needs to animate and sound, and
/// nothing a browser has for people.
fn settings() -> Settings {
    let settings = Settings::new();
    // Local files reading local files: the wallpaper's own scripts, data and textures.
    settings.set_allow_file_access_from_file_urls(true);
    settings.set_allow_universal_access_from_file_urls(true);
    settings.set_media_playback_requires_user_gesture(false);
    settings.set_enable_webgl(true);
    settings.set_enable_webaudio(true);
    settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Always);
    // stdout is the daemon's; the bridge reports page errors to the log instead.
    settings.set_enable_write_console_messages_to_stdout(false);
    // Memory and work a wallpaper has no use for.
    settings.set_enable_page_cache(false);
    settings.set_enable_developer_extras(false);
    settings.set_enable_back_forward_navigation_gestures(false);
    settings.set_enable_dns_prefetching(false);
    settings.set_enable_smooth_scrolling(false);
    settings.set_enable_site_specific_quirks(false);
    settings.set_enable_fullscreen(false);
    settings.set_enable_media_stream(false);
    settings.set_enable_webrtc(false);
    settings.set_enable_encrypted_media(false);
    settings.set_javascript_can_open_windows_automatically(false);
    settings.set_javascript_can_access_clipboard(false);
    settings.set_allow_modal_dialogs(false);
    settings
}

/// A wallpaper is one page: it opens no windows, shows no dialogs and menus, asks for
/// no permissions, and links lead nowhere.
fn refuse_everything_but_the_page(view: &WebView, root: &str) {
    let root = root.to_owned();
    view.connect_decide_policy(move |_, decision, kind| match kind {
        PolicyDecisionType::NavigationAction => {
            let Some(action) = decision
                .downcast_ref::<NavigationPolicyDecision>()
                .and_then(NavigationPolicyDecision::navigation_action)
            else {
                return false;
            };
            // What a person does on the desktop (a click on a link, a form) must not
            // turn the wallpaper into a browser, in whichever frame it happens.
            let by_hand = matches!(
                action.navigation_type(),
                NavigationType::LinkClicked | NavigationType::FormSubmitted | NavigationType::FormResubmitted
            );
            let target = action.request().and_then(|request| request.uri()).unwrap_or_default();
            if by_hand && !stays_inside(&target, &root) {
                log::debug(&format!("navigation to {target} refused"));
                decision.ignore();
                return true;
            }
            false
        }
        PolicyDecisionType::NewWindowAction => {
            decision.ignore();
            true
        }
        // A script can send the main frame anywhere without a click (`location = ...`).
        // The action does not say which frame it is for; the response does. Frames
        // inside the page may show what they like.
        PolicyDecisionType::Response => {
            let Some(response) = decision.downcast_ref::<ResponsePolicyDecision>() else {
                return false;
            };
            let target = response.request().and_then(|request| request.uri()).unwrap_or_default();
            if response.is_main_frame_main_resource() && !stays_inside(&target, &root) {
                log::warning(&format!("the page tried to leave for {target}: refused"));
                decision.ignore();
                return true;
            }
            false
        }
        _ => false,
    });
    view.connect_create(|_, _| None);
    view.connect_context_menu(|_, _, _| true);
    view.connect_script_dialog(|_, _| true);
    view.connect_run_file_chooser(|_, _| true);
    view.connect_print(|_, _| true);
    view.connect_enter_fullscreen(|_| true);
    view.connect_show_notification(|_, _| true);
    view.connect_permission_request(|_, request| {
        request.deny();
        true
    });
}

/// Whether `uri` is the wallpaper's folder or something in it.
fn stays_inside(uri: &str, root: &str) -> bool {
    let path = uri.split(['?', '#']).next().unwrap_or_default();
    let below = path.strip_prefix(root).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
    below && !path.split('/').any(|segment| segment == ".." || segment.eq_ignore_ascii_case("%2e%2e"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "file:///home/u/workshop/431960/1731760875";

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/e2e/wallpaper")
    }

    #[test]
    fn a_page_is_found_by_its_name_in_the_folder() {
        let location = Location::find(&fixture(), "global.html").unwrap();
        assert!(location.root.starts_with("file:///"));
        assert!(location.root.ends_with("/tests/e2e/wallpaper"));
        assert_eq!(location.entry, format!("{}/global.html", location.root));
    }

    #[test]
    fn a_folder_reached_through_dot_dot_is_resolved() {
        let roundabout = fixture().join("../wallpaper");
        assert_eq!(Location::find(&roundabout, "index.html"), Location::find(&fixture(), "index.html"));
    }

    #[test]
    fn a_missing_folder_or_page_is_an_error_that_names_it() {
        let problem = Location::find(&fixture(), "nothing.html").unwrap_err();
        assert!(problem.contains("nothing.html"), "{problem}");
        assert!(Location::find(Path::new("/nonexistent/wallpaper"), "index.html").is_err());
    }

    #[test]
    fn a_page_outside_the_folder_is_refused() {
        assert!(Location::find(&fixture(), "../run.sh").is_err());
        assert!(Location::find(&fixture(), "/etc/hostname").is_err());
    }

    #[test]
    fn a_folder_is_not_a_page() {
        assert!(Location::find(&fixture(), ".").is_err());
    }

    #[test]
    fn the_folder_and_everything_under_it_is_inside() {
        assert!(stays_inside(ROOT, ROOT));
        assert!(stays_inside(&format!("{ROOT}/index.html"), ROOT));
        assert!(stays_inside(&format!("{ROOT}/js/main.js?v=3#top"), ROOT));
        assert!(stays_inside(&format!("{ROOT}/"), ROOT));
    }

    #[test]
    fn other_places_are_outside() {
        assert!(!stays_inside("https://example.com/", ROOT));
        assert!(!stays_inside("file:///etc/passwd", ROOT));
        assert!(!stays_inside("about:blank", ROOT));
        assert!(!stays_inside("", ROOT));
    }

    #[test]
    fn a_sibling_folder_with_the_same_prefix_is_outside() {
        assert!(!stays_inside("file:///home/u/workshop/431960/17317608750/index.html", ROOT));
    }

    #[test]
    fn climbing_out_with_dot_dot_is_outside() {
        assert!(!stays_inside(&format!("{ROOT}/../855787646/index.html"), ROOT));
        assert!(!stays_inside(&format!("{ROOT}/js/%2E%2E/%2e%2e/../x"), ROOT));
    }

    #[test]
    fn a_query_that_names_another_place_does_not_count() {
        assert!(stays_inside(&format!("{ROOT}/index.html?next=https://example.com/../"), ROOT));
    }
}

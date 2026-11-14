// The page's side of wepaper-web: Wallpaper Engine's JavaScript API for web wallpapers.
//
// Injected at document start, before any script of the wallpaper. The plugin talks to
// it through wepaperBridge.receive(message); it answers through the "wepaper" script
// message handler. Everything a page may call is defined here, so a wallpaper written
// for Wallpaper Engine finds the API it expects and never throws for the lack of it.
(function () {
    'use strict';
    if (window.wepaperBridge) {
        return;
    }

    // The page may replace any of these later; the bridge keeps the originals.
    const native = {
        requestFrame: window.requestAnimationFrame.bind(window),
        setTimeout: window.setTimeout.bind(window),
        clearTimeout: window.clearTimeout.bind(window),
        setInterval: window.setInterval.bind(window),
        clearInterval: window.clearInterval.bind(window),
    };

    const state = {
        // Every user property, in the listener's shape: {name: {value}}.
        user: {},
        // The frame rate the page is told about.
        fps: 60,
        // The frame rate frames are held to; 0 is no cap.
        limit: 0,
        paused: false,
        // The master volume, 0 to 1.
        volume: 1,
        // Whether the document has loaded: nothing is delivered to the page before.
        loaded: false,
        // The page is drawn for a device, in a window nobody sees: the browser shows
        // it no frames, so its animation frames are run from a timer instead.
        unseen: false,
    };

    function post(message) {
        try {
            window.webkit.messageHandlers.wepaper.postMessage(JSON.stringify(message));
        } catch (_) {
            // No handler: the page was opened outside the plugin.
        }
    }

    // An error in a page's callback must reach the page's own error handling the way it
    // would without the bridge in between, and must not stop the bridge.
    function rethrow(error) {
        if (typeof window.reportError === 'function') {
            window.reportError(error);
        } else {
            native.setTimeout(function () {
                throw error;
            }, 0);
        }
    }

    function copy(value) {
        return JSON.parse(JSON.stringify(value));
    }

    // ---- The property listener ----

    let listener;
    // What the current listener has not been given yet. A page gets the whole state
    // once after load; from then on only changes.
    let owed = { user: true, general: true, paused: true };
    let retry = 0;
    // A listener whose methods are attached after the object was assigned gets a few
    // more chances at its first call.
    const RETRY_DELAYS = [50, 250, 1000, 3000];

    function call(name, ...values) {
        const method = listener && listener[name];
        if (typeof method !== 'function') {
            return false;
        }
        try {
            method.apply(listener, values);
        } catch (error) {
            rethrow(error);
        }
        return true;
    }

    function settle() {
        if (!state.loaded || !listener) {
            return;
        }
        if (owed.user && call('applyUserProperties', copy(state.user))) {
            owed.user = false;
        }
        if (owed.general && call('applyGeneralProperties', { fps: state.fps })) {
            owed.general = false;
        }
        if (owed.paused && (!state.paused || call('setPaused', true))) {
            owed.paused = false;
        }
        if ((owed.user || owed.general) && retry < RETRY_DELAYS.length) {
            native.setTimeout(settle, RETRY_DELAYS[retry]);
            retry += 1;
        }
    }

    Object.defineProperty(window, 'wallpaperPropertyListener', {
        configurable: true,
        enumerable: true,
        get() {
            return listener;
        },
        set(value) {
            listener = value;
            owed = { user: true, general: true, paused: true };
            retry = 0;
            // After this task, so that methods the page adds right after the assignment exist.
            native.setTimeout(settle, 0);
        },
    });

    window.addEventListener('load', function () {
        // After the page's own load handlers: many assign the listener in one.
        native.setTimeout(function () {
            state.loaded = true;
            settle();
            if (state.paused) {
                freeze();
            }
        }, 0);
    });

    // ---- Frames: one pump for every requestAnimationFrame of the page ----
    //
    // Holding the callbacks here is what makes pause and the frame cap real: a paused
    // page asks the browser for no frames at all, and a capped one only runs its
    // callbacks on the frames it is allowed.

    let frames = new Map();
    let running = null;
    let nextFrame = 1;
    let pumping = false;
    let lastFrame = -Infinity;
    // When the timer that stands in for the browser's frames last ran.
    let lastTimed = -Infinity;

    function pump() {
        if (!pumping && !state.paused && frames.size > 0) {
            pumping = true;
            if (state.unseen) {
                // A window nobody sees is never painted, and a page that is not
                // painted gets no animation frames. A timer gives them, at the cap
                // (or sixty a second without one).
                const interval = 1000 / (state.limit > 0 ? state.limit : 60);
                const wait = Math.max(0, lastTimed + interval - performance.now());
                native.setTimeout(function () {
                    lastTimed = performance.now();
                    tick(lastTimed);
                }, wait);
            } else {
                native.requestFrame(tick);
            }
        }
    }

    function tick(now) {
        pumping = false;
        if (state.paused) {
            return;
        }
        // The stand-in timer runs at the cap already.
        if (state.limit > 0 && !state.unseen) {
            const interval = 1000 / state.limit;
            const elapsed = now - lastFrame;
            // Two milliseconds of slack: display frames do not arrive on an exact grid.
            if (elapsed < interval - 2) {
                pump();
                return;
            }
            // Stepping by the interval, not to `now`, keeps the average at the cap when
            // the display's rate is not a multiple of it.
            lastFrame = elapsed > 2 * interval ? now : lastFrame + interval;
        }
        running = frames;
        frames = new Map();
        for (const callback of running.values()) {
            try {
                callback(now);
            } catch (error) {
                rethrow(error);
            }
        }
        running = null;
        pump();
    }

    window.requestAnimationFrame = function (callback) {
        if (typeof callback !== 'function') {
            throw new TypeError('requestAnimationFrame: the callback is not a function');
        }
        const id = nextFrame;
        nextFrame += 1;
        frames.set(id, callback);
        pump();
        return id;
    };
    window.webkitRequestAnimationFrame = window.requestAnimationFrame;

    window.cancelAnimationFrame = function (id) {
        frames.delete(id);
        if (running) {
            running.delete(id);
        }
    };
    window.webkitCancelAnimationFrame = window.cancelAnimationFrame;

    // ---- Timers: nothing of the page runs while paused ----
    //
    // A timeout that comes due during a pause runs on resume; an interval skips its
    // ticks. Handlers given as strings of code are left to the browser.

    const heldTimeouts = new Map();

    window.setTimeout = function (handler, delay, ...values) {
        if (typeof handler !== 'function') {
            return native.setTimeout(handler, delay, ...values);
        }
        const id = native.setTimeout(function () {
            if (state.paused) {
                heldTimeouts.set(id, function () {
                    handler.apply(window, values);
                });
            } else {
                handler.apply(window, values);
            }
        }, delay);
        return id;
    };

    window.setInterval = function (handler, delay, ...values) {
        if (typeof handler !== 'function') {
            return native.setInterval(handler, delay, ...values);
        }
        return native.setInterval(function () {
            if (!state.paused) {
                handler.apply(window, values);
            }
        }, delay);
    };

    // Browsers accept either function for either kind of timer.
    window.clearTimeout = function (id) {
        heldTimeouts.delete(id);
        native.clearTimeout(id);
    };
    window.clearInterval = function (id) {
        heldTimeouts.delete(id);
        native.clearInterval(id);
    };

    function releaseTimeouts() {
        const due = Array.from(heldTimeouts.values());
        heldTimeouts.clear();
        for (const run of due) {
            try {
                run();
            } catch (error) {
                rethrow(error);
            }
        }
    }

    // ---- Sound: a master volume over media elements and Web Audio ----
    //
    // WebKitGTK can mute a page but has no volume for it, so the volume is applied
    // here: a media element plays at what the page asked for times the master, and
    // everything a page connects to an audio context's output goes through a gain.

    const mediaVolume = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'volume');
    const askedVolume = new WeakMap();
    const mediaElements = new Set();
    const adopted = new WeakSet();

    function applyMediaVolume(element) {
        const asked = askedVolume.has(element) ? askedVolume.get(element) : 1;
        mediaVolume.set.call(element, asked * state.volume);
    }

    function adopt(element) {
        if (!(element instanceof HTMLMediaElement) || adopted.has(element)) {
            return;
        }
        adopted.add(element);
        mediaElements.add(new WeakRef(element));
        applyMediaVolume(element);
    }

    function eachMediaElement(visit) {
        document.querySelectorAll('audio, video').forEach(adopt);
        for (const reference of mediaElements) {
            const element = reference.deref();
            if (element) {
                visit(element);
            } else {
                mediaElements.delete(reference);
            }
        }
    }

    if (mediaVolume && mediaVolume.set) {
        Object.defineProperty(HTMLMediaElement.prototype, 'volume', {
            configurable: true,
            enumerable: true,
            get() {
                return askedVolume.has(this) ? askedVolume.get(this) : 1;
            },
            set(value) {
                const asked = Number(value);
                if (!(asked >= 0 && asked <= 1)) {
                    // Out of range: the browser's own setter throws the right error.
                    mediaVolume.set.call(this, value);
                    return;
                }
                askedVolume.set(this, asked);
                adopt(this);
                applyMediaVolume(this);
            },
        });

        const play = HTMLMediaElement.prototype.play;
        HTMLMediaElement.prototype.play = function (...values) {
            adopt(this);
            return play.apply(this, values);
        };

        // Elements that start by themselves (autoplay in the markup) announce it here;
        // media events do not bubble, so they are caught on the way down.
        for (const event of ['loadstart', 'play']) {
            document.addEventListener(event, (occurrence) => adopt(occurrence.target), true);
        }
    }

    const contexts = new Set();
    const masterGains = new Map();

    function trackContext(name) {
        const Native = window[name];
        if (typeof Native !== 'function') {
            return;
        }
        const Tracked = class extends Native {
            constructor(...values) {
                super(...values);
                contexts.add(this);
            }
        };
        Object.defineProperty(Tracked, 'name', { value: name });
        window[name] = Tracked;
    }
    trackContext('AudioContext');
    if (window.webkitAudioContext) {
        window.webkitAudioContext = window.AudioContext;
    }

    if (window.AudioNode && window.AudioDestinationNode) {
        const connect = AudioNode.prototype.connect;
        const disconnect = AudioNode.prototype.disconnect;

        const throughMaster = function (target) {
            const offline = window.OfflineAudioContext && target.context instanceof window.OfflineAudioContext;
            if (!(target instanceof AudioDestinationNode) || offline) {
                return target;
            }
            let gain = masterGains.get(target.context);
            if (!gain) {
                gain = target.context.createGain();
                gain.gain.value = state.volume;
                connect.call(gain, target);
                masterGains.set(target.context, gain);
            }
            return gain;
        };

        AudioNode.prototype.connect = function (target, ...values) {
            const result = connect.call(this, target instanceof AudioNode ? throughMaster(target) : target, ...values);
            // connect() returns its target so calls can be chained: the page's, not the gain.
            return target instanceof AudioNode ? target : result;
        };
        AudioNode.prototype.disconnect = function (target, ...values) {
            if (target instanceof AudioNode) {
                return disconnect.call(this, throughMaster(target), ...values);
            }
            return disconnect.apply(this, arguments);
        };
    }

    function applyVolume() {
        eachMediaElement(applyMediaVolume);
        for (const gain of masterGains.values()) {
            gain.gain.value = state.volume;
        }
    }

    // ---- Pause ----

    const PAUSE_STYLE = 'wepaper-paused';
    let stoppedMedia = [];
    let stoppedContexts = [];

    function freeze() {
        eachMediaElement(function (element) {
            if (!element.paused && !element.ended) {
                stoppedMedia.push(element);
                element.pause();
            }
        });
        for (const context of contexts) {
            if (context.state === 'running') {
                stoppedContexts.push(context);
                context.suspend().catch(function () {});
            }
        }
        if (document.documentElement && !document.getElementById(PAUSE_STYLE)) {
            const style = document.createElement('style');
            style.id = PAUSE_STYLE;
            style.textContent = '*, *::before, *::after { animation-play-state: paused !important; }';
            document.documentElement.appendChild(style);
        }
    }

    function thaw() {
        const style = document.getElementById(PAUSE_STYLE);
        if (style) {
            style.remove();
        }
        for (const context of stoppedContexts) {
            context.resume().catch(function () {});
        }
        stoppedContexts = [];
        for (const element of stoppedMedia) {
            const playing = element.play();
            if (playing && typeof playing.catch === 'function') {
                playing.catch(function () {});
            }
        }
        stoppedMedia = [];
        call('setPaused', false);
        releaseTimeouts();
        pump();
    }

    function setPaused(paused) {
        if (paused === state.paused) {
            return;
        }
        state.paused = paused;
        if (!state.loaded) {
            // The page will learn it with the rest of the state once it has loaded.
            return;
        }
        if (paused) {
            call('setPaused', true);
            freeze();
        } else {
            thaw();
        }
    }

    // ---- The audio spectrum ----

    let audioListener = null;

    window.wallpaperRegisterAudioListener = function (callback) {
        const next = typeof callback === 'function' ? callback : null;
        const changed = (next === null) !== (audioListener === null);
        audioListener = next;
        if (changed) {
            post({ type: 'audio', wanted: next !== null });
        }
    };

    // ---- Parts of the API that have nothing behind them here ----
    //
    // Media integration (the song playing on the system) and file properties are not
    // provided: the registrations exist so that pages do not throw, and never fire.

    for (const name of [
        'wallpaperRegisterMediaStatusListener',
        'wallpaperRegisterMediaPropertiesListener',
        'wallpaperRegisterMediaThumbnailListener',
        'wallpaperRegisterMediaPlaybackListener',
        'wallpaperRegisterMediaTimelineListener',
        'wallpaperRequestRandomFileForProperty',
    ]) {
        window[name] = function () {};
    }
    window.wallpaperMediaIntegration = Object.freeze({
        PLAYBACK_STOPPED: 0,
        PLAYBACK_PLAYING: 1,
        PLAYBACK_PAUSED: 2,
    });

    // ---- What the page gets wrong, for the plugin's log ----

    let reports = 0;
    function report(text) {
        if (reports < 20) {
            reports += 1;
            post({ type: 'log', message: String(text) });
        }
    }
    window.addEventListener('error', function (occurrence) {
        if (occurrence.message) {
            const file = occurrence.filename || '';
            report(occurrence.message + ' (' + file.slice(file.lastIndexOf('/') + 1) + ':' + occurrence.lineno + ')');
        } else if (occurrence.target && occurrence.target !== window) {
            // Markup with an empty src="" filled in later by a script is common, and not news.
            const target = occurrence.target;
            const address = target.getAttribute && (target.getAttribute('src') || target.getAttribute('href'));
            if (address) {
                report('could not load ' + address);
            }
        }
    }, true);
    window.addEventListener('unhandledrejection', function (occurrence) {
        report('unhandled rejection: ' + occurrence.reason);
    });

    // ---- Messages from the plugin ----

    function receive(message) {
        switch (message.type) {
            case 'init':
                if (state.loaded) {
                    // Late: the page already has the state this document started
                    // with, so what differs reaches it as ordinary changes.
                    receive({ type: 'properties', values: message.properties });
                    receive({ type: 'general', fps: message.fps, limit: message.limit });
                    receive({ type: 'volume', value: message.volume });
                    receive({ type: 'paused', value: message.paused });
                    break;
                }
                state.user = message.properties;
                state.fps = message.fps;
                state.limit = message.limit;
                state.volume = message.volume;
                state.paused = message.paused;
                state.unseen = message.unseen === true;
                // The page's scripts may have run already and started their sound at
                // the volume nobody had told yet.
                applyVolume();
                break;
            case 'properties':
                Object.assign(state.user, message.values);
                if (owed.user) {
                    settle();
                } else {
                    call('applyUserProperties', copy(message.values));
                }
                break;
            case 'general':
                state.fps = message.fps;
                state.limit = message.limit;
                lastFrame = -Infinity;
                if (owed.general) {
                    settle();
                } else {
                    call('applyGeneralProperties', { fps: state.fps });
                }
                break;
            case 'paused':
                setPaused(message.value);
                break;
            case 'volume':
                state.volume = message.value;
                applyVolume();
                break;
            case 'audio':
                if (audioListener && !state.paused) {
                    try {
                        audioListener(message.samples);
                    } catch (error) {
                        rethrow(error);
                    }
                }
                break;
        }
    }

    // WebKit's fetch does not read local files; XMLHttpRequest does. A wallpaper is
    // local files, so a fetch of one is answered through XMLHttpRequest, with the type
    // a server would have said: a WebAssembly module is only compiled from a response
    // that says it is one (a Unity build asks for its module this way and nothing else).
    const types = {
        wasm: 'application/wasm', js: 'text/javascript', mjs: 'text/javascript', json: 'application/json',
        html: 'text/html', css: 'text/css', txt: 'text/plain', xml: 'application/xml',
        png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp',
        svg: 'image/svg+xml', mp3: 'audio/mpeg', ogg: 'audio/ogg', wav: 'audio/wav', mp4: 'video/mp4',
        webm: 'video/webm',
    };
    const nativeFetch = window.fetch ? window.fetch.bind(window) : null;
    function localFetch(url) {
        return new Promise(function (resolve, reject) {
            const request = new XMLHttpRequest();
            request.open('GET', url, true);
            request.responseType = 'arraybuffer';
            request.onload = function () {
                // A local file has no status: 0 with a body is a file that was read.
                const read = request.response && (request.status === 0 || (request.status >= 200 && request.status < 300));
                if (!read) {
                    reject(new TypeError('Failed to fetch ' + url));
                    return;
                }
                const extension = (url.split(/[?#]/)[0].match(/\.([A-Za-z0-9]+)$/) || [])[1] || '';
                const response = new Response(request.response, {
                    status: 200,
                    statusText: 'OK',
                    headers: {
                        'Content-Type': types[extension.toLowerCase()] || 'application/octet-stream',
                        'Content-Length': String(request.response.byteLength),
                    },
                });
                try {
                    Object.defineProperty(response, 'url', { value: url });
                } catch (_) {
                    // The response keeps an empty address: nothing a wallpaper reads.
                }
                resolve(response);
            };
            request.onerror = function () {
                reject(new TypeError('Failed to fetch ' + url));
            };
            request.send();
        });
    }
    window.fetch = function (input, init) {
        let url = '';
        try {
            url = new URL(typeof input === 'string' ? input : input.url, document.baseURI).href;
        } catch (_) {
            // Not an address: the browser's own fetch says so.
        }
        const method = (init && init.method) || (input && typeof input === 'object' && input.method) || 'GET';
        if (url.indexOf('file:') === 0 && String(method).toUpperCase() === 'GET') {
            return localFetch(url);
        }
        return nativeFetch ? nativeFetch(input, init) : Promise.reject(new TypeError('fetch is not available'));
    };

    Object.defineProperty(window, 'wepaperBridge', { value: Object.freeze({ receive: receive }) });
    // The plugin answers with the state as it is now: a command that arrived while
    // this document was still on its way had nowhere to go.
    post({ type: 'hello' });
})();

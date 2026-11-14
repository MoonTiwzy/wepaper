// Tests of src/bridge.js in a scripted stand-in for a page: a window whose clock,
// timers and display frames the test moves by hand. Run by tests/bridge.rs
// (`cargo test -p wepaper-web`) when node is installed, or directly:
//
//   node web/tests/bridge.test.js
//
// What needs a real browser (media, Web Audio, navigation, the surface) is covered by
// tests/e2e/run.sh instead.
'use strict';

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const SOURCE = fs.readFileSync(path.join(__dirname, '..', 'src', 'bridge.js'), 'utf8');

// Values made inside the page belong to another realm, where even a plain object has
// a different prototype: they are compared by what they hold.
function same(actual, expected, message) {
    assert.strictEqual(JSON.stringify(actual), JSON.stringify(expected), message);
}

// A page with the bridge injected. `init` is the state the plugin primes it with.
// `early`, when given, runs in the page after the bridge is there and before its
// first message: what a page's own scripts do while the document still loads.
function page(init = {}, early = null) {
    let now = 0;
    let nextTimer = 1;
    const timers = new Map();
    let displayFrames = [];
    const listeners = {};
    const posted = [];
    const errors = [];

    class HTMLMediaElement {
        constructor() {
            this.native = 1;
            this.paused = true;
            this.ended = false;
        }
        get volume() {
            return this.native;
        }
        set volume(value) {
            if (!(value >= 0 && value <= 1)) {
                throw new RangeError('volume out of range');
            }
            this.native = value;
        }
        play() {
            this.paused = false;
            return Promise.resolve();
        }
        pause() {
            this.paused = true;
        }
    }

    // Web Audio, as far as the bridge looks at it: nodes that remember what they
    // were connected to.
    class AudioNode {
        constructor(context) {
            this.context = context;
            this.to = [];
        }
        connect(target) {
            this.to.push(target);
            return target;
        }
        disconnect(target) {
            this.to = this.to.filter((node) => node !== target);
        }
    }
    class AudioDestinationNode extends AudioNode {}
    class GainNode extends AudioNode {
        constructor(context) {
            super(context);
            this.gain = { value: 1 };
        }
    }
    class AudioContext {
        constructor() {
            this.destination = new AudioDestinationNode(this);
        }
        createGain() {
            return new GainNode(this);
        }
    }

    const window = {
        requestAnimationFrame(callback) {
            displayFrames.push(callback);
            return displayFrames.length;
        },
        setTimeout(handler, delay, ...values) {
            const id = nextTimer++;
            timers.set(id, { at: now + (delay || 0), handler, values });
            return id;
        },
        setInterval(handler, delay, ...values) {
            const id = nextTimer++;
            timers.set(id, { at: now + delay, every: delay, handler, values });
            return id;
        },
        clearTimeout(id) {
            timers.delete(id);
        },
        clearInterval(id) {
            timers.delete(id);
        },
        addEventListener(type, handler) {
            (listeners[type] = listeners[type] || []).push(handler);
        },
        reportError(error) {
            errors.push(error);
        },
        webkit: { messageHandlers: { wepaper: { postMessage: (text) => posted.push(JSON.parse(text)) } } },
        document: {
            documentElement: null,
            addEventListener() {},
            querySelectorAll: () => [],
            getElementById: () => null,
        },
        HTMLMediaElement,
        AudioNode,
        AudioDestinationNode,
        AudioContext,
    };
    window.window = window;
    vm.createContext(window);
    vm.runInContext(SOURCE, window);

    const send = (message) => window.wepaperBridge.receive(JSON.parse(JSON.stringify(message)));
    if (early) {
        early(window);
    }
    send({ type: 'init', properties: {}, fps: 60, limit: 0, paused: false, volume: 1, ...init });

    return {
        window,
        posted,
        errors,
        send,
        HTMLMediaElement,
        // Moves the clock, running the timers that come due, in order.
        advance(milliseconds) {
            const until = now + milliseconds;
            for (;;) {
                let next = null;
                for (const [id, timer] of timers) {
                    if (timer.at <= until && (next === null || timer.at < timers.get(next).at)) {
                        next = id;
                    }
                }
                if (next === null) {
                    break;
                }
                const timer = timers.get(next);
                now = Math.max(now, timer.at);
                if (timer.every) {
                    timer.at += timer.every;
                } else {
                    timers.delete(next);
                }
                timer.handler.apply(window, timer.values);
            }
            now = until;
        },
        // One frame of the display at `timestamp`; whether the page had asked for one.
        displayFrame(timestamp) {
            const due = displayFrames;
            displayFrames = [];
            due.forEach((callback) => callback(timestamp));
            return due.length > 0;
        },
        // Runs the display at `hertz` for `seconds`.
        display(hertz, seconds, from = 0) {
            for (let frame = 0; frame < hertz * seconds; frame += 1) {
                this.displayFrame(from + (frame * 1000) / hertz);
            }
        },
        load() {
            (listeners.load || []).forEach((handler) => handler());
            this.advance(0);
        },
    };
}

// A listener that writes down every call.
function recorder() {
    const calls = [];
    return {
        calls,
        applyUserProperties: (properties) => calls.push(['user', properties]),
        applyGeneralProperties: (properties) => calls.push(['general', properties]),
        setPaused: (paused) => calls.push(['paused', paused]),
    };
}

const tests = {
    'a listener assigned before load gets every property and the frame rate once, after load'() {
        const p = page({ properties: { tint: { value: '1 0 0' }, on: { value: true } }, fps: 30, limit: 30 });
        const listener = recorder();
        p.window.wallpaperPropertyListener = listener;
        p.advance(10);
        same(listener.calls, [], 'nothing before load');
        p.load();
        same(listener.calls, [
            ['user', { tint: { value: '1 0 0' }, on: { value: true } }],
            ['general', { fps: 30 }],
        ]);
        p.advance(10000);
        assert.strictEqual(listener.calls.length, 2, 'and not again');
    },

    'the page reads back the listener it assigned'() {
        const p = page();
        const listener = recorder();
        p.window.wallpaperPropertyListener = listener;
        assert.strictEqual(p.window.wallpaperPropertyListener, listener);
    },

    'a listener assigned in a load handler of the page gets the state'() {
        const p = page({ properties: { speed: { value: 3 } } });
        const listener = recorder();
        p.window.addEventListener('load', () => {
            p.window.wallpaperPropertyListener = listener;
        });
        p.load();
        same(listener.calls[0], ['user', { speed: { value: 3 } }]);
    },

    'a listener assigned long after load gets the state as it is by then'() {
        const p = page({ properties: { speed: { value: 3 } } });
        p.load();
        p.send({ type: 'properties', values: { speed: { value: 9 }, extra: { value: 'x' } } });
        p.advance(5000);
        const listener = recorder();
        p.window.wallpaperPropertyListener = listener;
        p.advance(0);
        same(listener.calls[0], ['user', { speed: { value: 9 }, extra: { value: 'x' } }]);
    },

    'methods attached after the assignment are still called'() {
        const p = page({ properties: { speed: { value: 3 } } });
        p.load();
        const calls = [];
        p.window.wallpaperPropertyListener = {};
        p.advance(100);
        p.window.wallpaperPropertyListener.applyUserProperties = (properties) => calls.push(properties);
        p.advance(1000);
        same(calls, [{ speed: { value: 3 } }]);
    },

    'a change reaches the listener with only what changed'() {
        const p = page({ properties: { a: { value: 1 }, b: { value: 2 } } });
        const listener = recorder();
        p.window.wallpaperPropertyListener = listener;
        p.load();
        listener.calls.length = 0;
        p.send({ type: 'properties', values: { b: { value: 5 } } });
        same(listener.calls, [['user', { b: { value: 5 } }]]);
    },

    'a change before load is folded into the first call'() {
        const p = page({ properties: { a: { value: 1 }, b: { value: 2 } } });
        const listener = recorder();
        p.window.wallpaperPropertyListener = listener;
        p.send({ type: 'properties', values: { b: { value: 5 } } });
        same(listener.calls, []);
        p.load();
        same(listener.calls[0], ['user', { a: { value: 1 }, b: { value: 5 } }]);
        assert.strictEqual(listener.calls.filter(([kind]) => kind === 'user').length, 1);
    },

    'a page that changes what it was handed does not change what the next listener gets'() {
        const p = page({ properties: { list: { value: [1, 2] } } });
        p.window.wallpaperPropertyListener = { applyUserProperties: (properties) => properties.list.value.push(3) };
        p.load();
        const listener = recorder();
        p.window.wallpaperPropertyListener = listener;
        p.advance(0);
        same(listener.calls[0], ['user', { list: { value: [1, 2] } }]);
    },

    'a listener that throws does not stop the bridge, and the page hears of the error'() {
        const p = page({ properties: { a: { value: 1 } } });
        const general = [];
        p.window.wallpaperPropertyListener = {
            applyUserProperties() {
                throw new Error('page bug');
            },
            applyGeneralProperties: (properties) => general.push(properties),
        };
        p.load();
        same(general, [{ fps: 60 }]);
        assert.strictEqual(p.errors.length, 1);
        assert.strictEqual(p.errors[0].message, 'page bug');
    },

    'a page without a listener is left alone'() {
        const p = page({ properties: { a: { value: 1 } } });
        p.load();
        p.send({ type: 'properties', values: { a: { value: 2 } } });
        p.send({ type: 'general', fps: 30, limit: 30 });
        p.send({ type: 'paused', value: true });
        p.send({ type: 'paused', value: false });
        p.advance(10000);
        same(p.errors, []);
    },

    'a frame rate change is announced'() {
        const p = page();
        const listener = recorder();
        p.window.wallpaperPropertyListener = listener;
        p.load();
        listener.calls.length = 0;
        p.send({ type: 'general', fps: 24, limit: 24 });
        same(listener.calls, [['general', { fps: 24 }]]);
    },

    'every animation frame callback runs once, on the next display frame'() {
        const p = page();
        const seen = [];
        p.window.requestAnimationFrame((time) => seen.push(['a', time]));
        p.window.requestAnimationFrame((time) => seen.push(['b', time]));
        same(seen, []);
        p.displayFrame(16);
        same(seen, [['a', 16], ['b', 16]]);
        p.displayFrame(32);
        assert.strictEqual(seen.length, 2);
    },

    'a callback that asks for another frame gets the next one, not the same'() {
        const p = page();
        let count = 0;
        const loop = () => {
            count += 1;
            p.window.requestAnimationFrame(loop);
        };
        p.window.requestAnimationFrame(loop);
        p.displayFrame(0);
        assert.strictEqual(count, 1);
        p.displayFrame(16);
        assert.strictEqual(count, 2);
    },

    'a cancelled frame does not run, even when cancelled by an earlier callback of the same frame'() {
        const p = page();
        const seen = [];
        const first = p.window.requestAnimationFrame(() => seen.push('first'));
        p.window.cancelAnimationFrame(first);
        let later = 0;
        p.window.requestAnimationFrame(() => p.window.cancelAnimationFrame(later));
        later = p.window.requestAnimationFrame(() => seen.push('later'));
        p.displayFrame(0);
        same(seen, []);
    },

    'a page that draws nothing asks the display for nothing'() {
        const p = page();
        assert.strictEqual(p.displayFrame(0), false);
        p.window.requestAnimationFrame(() => {});
        assert.strictEqual(p.displayFrame(16), true);
        assert.strictEqual(p.displayFrame(32), false);
    },

    'a callback that throws does not take the frame loop down'() {
        const p = page();
        let ran = false;
        p.window.requestAnimationFrame(() => {
            throw new Error('draw bug');
        });
        p.window.requestAnimationFrame(() => {
            ran = true;
        });
        p.displayFrame(0);
        assert.strictEqual(ran, true);
        assert.strictEqual(p.errors.length, 1);
    },

    'a frame cap holds the callbacks to its rate on any display'() {
        for (const [hertz, limit] of [[60, 30], [60, 24], [144, 60], [144, 30], [60, 60], [75, 50], [60, 1]]) {
            const p = page({ fps: limit, limit });
            let count = 0;
            const loop = () => {
                count += 1;
                p.window.requestAnimationFrame(loop);
            };
            p.window.requestAnimationFrame(loop);
            p.display(hertz, 10);
            const rate = count / 10;
            assert.ok(Math.abs(rate - limit) <= Math.max(0.5, limit * 0.02), `${limit} fps on ${hertz} Hz ran at ${rate}`);
        }
    },

    'without a cap the callbacks run on every display frame'() {
        const p = page();
        let count = 0;
        const loop = () => {
            count += 1;
            p.window.requestAnimationFrame(loop);
        };
        p.window.requestAnimationFrame(loop);
        p.display(144, 2);
        assert.strictEqual(count, 288);
    },

    'lifting a cap takes effect'() {
        const p = page({ fps: 30, limit: 30 });
        let count = 0;
        const loop = () => {
            count += 1;
            p.window.requestAnimationFrame(loop);
        };
        p.window.requestAnimationFrame(loop);
        p.display(60, 1);
        p.send({ type: 'general', fps: 60, limit: 0 });
        count = 0;
        p.display(60, 1, 1000);
        assert.strictEqual(count, 60);
    },

    'a paused page draws no frames and asks for none, and goes on where it was'() {
        const p = page();
        p.load();
        let count = 0;
        const loop = () => {
            count += 1;
            p.window.requestAnimationFrame(loop);
        };
        p.window.requestAnimationFrame(loop);
        p.displayFrame(0);
        p.send({ type: 'paused', value: true });
        assert.strictEqual(p.displayFrame(16), true, 'the frame asked for before the pause arrives');
        assert.strictEqual(count, 1, 'and is not drawn');
        assert.strictEqual(p.displayFrame(32), false, 'and no more are asked for');
        p.window.requestAnimationFrame(() => {
            count += 100;
        });
        assert.strictEqual(p.displayFrame(48), false);
        p.send({ type: 'paused', value: false });
        p.displayFrame(64);
        assert.strictEqual(count, 102, 'both held callbacks ran on resume');
    },

    'the listener hears of a pause and of its end'() {
        const p = page();
        const listener = recorder();
        p.window.wallpaperPropertyListener = listener;
        p.load();
        listener.calls.length = 0;
        p.send({ type: 'paused', value: true });
        p.send({ type: 'paused', value: true });
        p.send({ type: 'paused', value: false });
        same(listener.calls, [['paused', true], ['paused', false]]);
    },

    'a page that starts paused is told so after load'() {
        const p = page({ paused: true });
        const listener = recorder();
        p.window.wallpaperPropertyListener = listener;
        p.load();
        same(listener.calls.filter(([kind]) => kind === 'paused'), [['paused', true]]);
    },

    'intervals skip their ticks during a pause'() {
        const p = page();
        p.load();
        let ticks = 0;
        p.window.setInterval(() => {
            ticks += 1;
        }, 100);
        p.advance(350);
        assert.strictEqual(ticks, 3);
        p.send({ type: 'paused', value: true });
        p.advance(1000);
        assert.strictEqual(ticks, 3);
        p.send({ type: 'paused', value: false });
        p.advance(220);
        assert.strictEqual(ticks, 5);
    },

    'a timeout that comes due during a pause runs on resume, with its arguments'() {
        const p = page();
        p.load();
        const seen = [];
        p.window.setTimeout((a, b) => seen.push([a, b]), 100, 'x', 2);
        p.send({ type: 'paused', value: true });
        p.advance(500);
        same(seen, []);
        p.send({ type: 'paused', value: false });
        same(seen, [['x', 2]]);
        p.advance(500);
        assert.strictEqual(seen.length, 1);
    },

    'a timeout cleared while it is held does not run'() {
        const p = page();
        p.load();
        let ran = false;
        const id = p.window.setTimeout(() => {
            ran = true;
        }, 100);
        p.send({ type: 'paused', value: true });
        p.advance(500);
        p.window.clearTimeout(id);
        p.send({ type: 'paused', value: false });
        assert.strictEqual(ran, false);
    },

    'timers run as usual when not paused, and can be cleared'() {
        const p = page();
        const seen = [];
        p.window.setTimeout(() => seen.push('once'), 50);
        const cleared = p.window.setTimeout(() => seen.push('never'), 50);
        p.window.clearTimeout(cleared);
        const every = p.window.setInterval(() => seen.push('tick'), 40);
        p.advance(100);
        p.window.clearInterval(every);
        p.advance(1000);
        same(seen, ['tick', 'once', 'tick']);
    },

    'registering an audio listener is reported, and so is dropping it'() {
        const p = page();
        p.posted.length = 0;
        p.window.wallpaperRegisterAudioListener(() => {});
        p.window.wallpaperRegisterAudioListener(() => {});
        p.window.wallpaperRegisterAudioListener(null);
        p.window.wallpaperRegisterAudioListener(undefined);
        same(p.posted, [{ type: 'audio', wanted: true }, { type: 'audio', wanted: false }]);
    },

    'audio frames reach the listener as they are, but not during a pause'() {
        const p = page();
        p.load();
        const frames = [];
        p.window.wallpaperRegisterAudioListener((samples) => frames.push(samples));
        const samples = Array.from({ length: 128 }, (_, band) => band / 128);
        p.send({ type: 'audio', samples });
        assert.strictEqual(frames.length, 1);
        assert.strictEqual(frames[0].length, 128);
        assert.strictEqual(frames[0][64], 0.5);
        p.send({ type: 'paused', value: true });
        p.send({ type: 'audio', samples });
        assert.strictEqual(frames.length, 1);
    },

    'a media element plays at what the page asked for times the master volume'() {
        const p = page({ volume: 0.5 });
        const element = new p.HTMLMediaElement();
        element.volume = 0.8;
        assert.strictEqual(element.volume, 0.8, 'the page reads back its own value');
        assert.strictEqual(element.native, 0.4);
        p.send({ type: 'volume', value: 0.25 });
        assert.strictEqual(element.native, 0.2);
        assert.strictEqual(element.volume, 0.8);
    },

    'an element whose volume the page never set follows the master from its first play'() {
        const p = page({ volume: 0.3 });
        const element = new p.HTMLMediaElement();
        assert.strictEqual(element.volume, 1);
        element.play();
        assert.strictEqual(element.native, 0.3);
        assert.strictEqual(element.paused, false);
    },

    'sound a page starts before the first message is as loud as that message says'() {
        let master;
        let element;
        const p = page({ volume: 0 }, (window) => {
            const context = new window.AudioContext();
            const own = context.createGain();
            own.connect(context.destination);
            // What the page's node was really connected to: the bridge's gain.
            master = own.to[0];
            element = new window.HTMLMediaElement();
            element.volume = 0.8;
        });
        assert.ok(master && master.gain, 'the page goes through a master gain');
        assert.strictEqual(master.gain.value, 0);
        assert.strictEqual(element.native, 0);
        p.send({ type: 'volume', value: 0.5 });
        assert.strictEqual(master.gain.value, 0.5);
        assert.strictEqual(element.native, 0.4);
    },

    'a volume out of range is refused the way the browser refuses it'() {
        const p = page();
        const element = new p.HTMLMediaElement();
        assert.throws(() => {
            element.volume = 1.5;
        }, RangeError);
        assert.strictEqual(element.volume, 1);
    },

    'playing media stops for a pause and plays again after it'() {
        const p = page();
        p.load();
        const playing = new p.HTMLMediaElement();
        const stopped = new p.HTMLMediaElement();
        playing.play();
        stopped.play();
        stopped.pause();
        p.send({ type: 'paused', value: true });
        assert.strictEqual(playing.paused, true);
        p.send({ type: 'paused', value: false });
        assert.strictEqual(playing.paused, false);
        assert.strictEqual(stopped.paused, true, 'what the page had stopped stays stopped');
    },

    'the bridge says hello when a document starts'() {
        const p = page();
        same(p.posted[0], { type: 'hello' });
    },

    'the state sent in answer to hello after load arrives as changes'() {
        const p = page({ properties: { a: { value: 1 } } });
        const listener = recorder();
        p.window.wallpaperPropertyListener = listener;
        p.load();
        listener.calls.length = 0;
        p.send({ type: 'init', properties: { a: { value: 2 } }, fps: 30, limit: 30, paused: true, volume: 1 });
        same(listener.calls, [['user', { a: { value: 2 } }], ['general', { fps: 30 }], ['paused', true]]);
    },

    'the parts of the API with nothing behind them can be called'() {
        const p = page();
        for (const name of [
            'wallpaperRegisterMediaStatusListener',
            'wallpaperRegisterMediaPropertiesListener',
            'wallpaperRegisterMediaThumbnailListener',
            'wallpaperRegisterMediaPlaybackListener',
            'wallpaperRegisterMediaTimelineListener',
            'wallpaperRequestRandomFileForProperty',
        ]) {
            assert.strictEqual(typeof p.window[name], 'function', name);
            p.window[name](() => {
                throw new Error('must never fire');
            });
        }
        assert.strictEqual(p.window.wallpaperMediaIntegration.PLAYBACK_PLAYING, 1);
        p.load();
        p.advance(60000);
        same(p.errors, []);
    },

    'injecting the bridge twice keeps the first'() {
        const p = page();
        const first = p.window.wepaperBridge;
        vm.runInContext(SOURCE, p.window);
        assert.strictEqual(p.window.wepaperBridge, first);
    },
};

let failed = 0;
for (const [name, test] of Object.entries(tests)) {
    try {
        test();
        console.log(`ok    ${name}`);
    } catch (error) {
        failed += 1;
        console.log(`FAIL  ${name}\n      ${String(error.message).split('\n').join('\n      ')}`);
    }
}
console.log(failed === 0 ? `\n${Object.keys(tests).length} passed` : `\n${failed} failed`);
process.exit(failed === 0 ? 0 : 1);

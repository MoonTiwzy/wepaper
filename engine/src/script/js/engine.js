// The globals that are not about layers: `engine`, `input`, `console`, `localStorage`,
// the timers and the audio buffers. `we` is the runtime's private object: `we.host` holds
// the native functions, `we.env` the frame's environment, `we.current` the running script.
(function (we) {
	'use strict';
	const host = we.host;
	const env = we.env;
	const LOG_ERROR = 1, LOG_INFO = 3;

	// A member the runtime does not implement: says so once and answers with something harmless.
	we.unsupported = (name) => host.unsupported(name);
	we.stub = (name, answer) => function () {
		host.unsupported(name);
		return typeof answer === 'function' ? answer() : answer;
	};

	// Wallpaper Engine hands colour properties to scripts as vectors. The host gives values
	// without their types, so a string of exactly three numbers is taken for a colour.
	const COLOR = /^\s*[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?(\s+[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?){2}\s*$/i;
	we.userValue = (value) => (typeof value === 'string' && COLOR.test(value) ? new Vec3(value) : value);

	const userProperties = new Proxy({}, {
		get(_, key) {
			if (typeof key !== 'string') return undefined;
			const value = host.userProperty(key);
			return value === null ? Object.prototype[key] : we.userValue(value);
		},
		has(_, key) {
			return typeof key === 'string' && host.userProperty(key) !== null;
		},
		getOwnPropertyDescriptor(_, key) {
			const value = typeof key === 'string' ? host.userProperty(key) : null;
			return value === null ? undefined : { value: we.userValue(value), writable: true, enumerable: true, configurable: true };
		},
		set() {
			return true;
		},
	});

	// Timers run on the scene's clock, so a paused wallpaper pauses them too.
	const timers = [];
	const schedule = (callback, delay, repeats) => {
		const wait = Math.max(0, Number(delay) || 0);
		const timer = { due: env.time * 1000 + wait, every: repeats ? wait : -1, run: callback, owner: we.current, frame: -1, dead: typeof callback !== 'function' };
		timers.push(timer);
		return () => { timer.dead = timer.cancelled = true; };
	};
	const cancel = (handle) => {
		if (typeof handle === 'function') handle();
	};

	// Runs the next timer that is due, at most once a frame each; false when none is left.
	// A callback that throws or is interrupted leaves its timer dead.
	we.tick = () => {
		const now = env.time * 1000;
		for (let i = 0; i < timers.length; i++) {
			const timer = timers[i];
			if (timer.dead) {
				timers.splice(i--, 1);
				continue;
			}
			if (timer.due > now || timer.frame === env.frame) continue;
			timer.frame = env.frame;
			timer.due = now + timer.every;
			timer.dead = true;
			we.current = timer.owner;
			timer.run();
			timer.dead = timer.every < 0 || timer.cancelled === true;
			return true;
		}
		return false;
	};

	const buffers = [];
	// `samples` is 64 bands of the left channel followed by 64 of the right; a buffer of
	// fewer bands holds the mean of the bands each of its own covers.
	we.audio = (samples) => {
		for (const buffer of buffers) {
			const bands = buffer.left.length, width = 64 / bands;
			for (let band = 0; band < bands; band++) {
				let left = 0, right = 0;
				for (let i = band * width; i < (band + 1) * width; i++) {
					left += samples[i];
					right += samples[64 + i];
				}
				buffer.left[band] = left / width;
				buffer.right[band] = right / width;
				buffer.average[band] = (left + right) / (2 * width);
			}
		}
	};

	const engine = {
		AUDIO_RESOLUTION_16: 16,
		AUDIO_RESOLUTION_32: 32,
		AUDIO_RESOLUTION_64: 64,
		get frametime() { return env.frametime; },
		get runtime() { return env.time; },
		get timeOfDay() { return env.timeOfDay; },
		get screenResolution() { return new Vec2(env.screenWidth, env.screenHeight); },
		get canvasSize() { return new Vec2(env.canvasWidth, env.canvasHeight); },
		get userProperties() { return userProperties; },
		isDesktopDevice() { return true; },
		isMobileDevice() { return false; },
		isWallpaper() { return true; },
		isScreensaver() { return false; },
		isRunningInEditor() { return false; },
		isPortrait() { return env.screenHeight > env.screenWidth; },
		isLandscape() { return env.screenHeight <= env.screenWidth; },
		setTimeout(callback, delay) { return schedule(callback, delay, false); },
		setInterval(callback, delay) { return schedule(callback, delay, true); },
		clearTimeout: cancel,
		clearInterval: cancel,
		registerAudioBuffers(resolution) {
			const bands = resolution === 16 || resolution === 32 ? resolution : 64;
			const buffer = { left: new Float32Array(bands), right: new Float32Array(bands), average: new Float32Array(bands) };
			buffers.push(buffer);
			host.wantAudio();
			return buffer;
		},
		// Nothing is loaded for a handle: it only names the file for `thisScene.createLayer`.
		registerAsset(file) {
			const name = String(file);
			return { file: name, toString: () => name, toConfigString: () => name };
		},
		openUserShortcut: we.stub('engine.openUserShortcut', false),
	};

	const input = {
		get cursorWorldPosition() { return new Vec3(env.cursorX, env.cursorY, 0); },
		get cursorScreenPosition() { return new Vec2(env.pointerX, env.pointerY); },
		get cursorLeftDown() { return env.cursorDown; },
	};

	const show = (value) => {
		if (typeof value === 'string') return value;
		if (value !== null && typeof value === 'object' && value.toString === Object.prototype.toString) {
			try {
				return JSON.stringify(value);
			} catch (error) {
				return String(value);
			}
		}
		return String(value);
	};
	const print = (level) => (...values) => host.log(level, values.map(show).join(' '));
	const console = { log: print(LOG_INFO), info: print(LOG_INFO), debug: print(LOG_INFO), warn: print(LOG_ERROR), error: print(LOG_ERROR) };

	// Values are kept as JSON under `<location>:<key>`; `<location>#keys` lists what a
	// location holds, since the host's storage cannot be enumerated.
	const place = (location) => (location === 'global' ? 'global' : 'screen');
	const keysOf = (location) => {
		try {
			const keys = JSON.parse(host.storageGet(location + '#keys') || '[]');
			return Array.isArray(keys) ? keys : [];
		} catch (error) {
			return [];
		}
	};
	const localStorage = {
		LOCATION_GLOBAL: 'global',
		LOCATION_SCREEN: 'screen',
		get(key, location) {
			const text = host.storageGet(place(location) + ':' + key);
			if (typeof text !== 'string') return undefined;
			try {
				return JSON.parse(text);
			} catch (error) {
				return text;
			}
		},
		set(key, value, location) {
			const text = JSON.stringify(value, (_, item) => (item && typeof item.toConfigString === 'function' ? item.toConfigString() : item));
			if (text === undefined) return;
			const where = place(location), keys = keysOf(where);
			host.storageSet(where + ':' + key, text);
			if (!keys.includes(String(key))) host.storageSet(where + '#keys', JSON.stringify(keys.concat(String(key))));
		},
		delete(key, location) {
			const where = place(location), keys = keysOf(where);
			const existed = typeof host.storageGet(where + ':' + key) === 'string';
			host.storageSet(where + ':' + key, null);
			if (keys.includes(String(key))) host.storageSet(where + '#keys', JSON.stringify(keys.filter((other) => other !== String(key))));
			return existed;
		},
		clear(location) {
			const where = place(location);
			for (const key of keysOf(where)) host.storageSet(where + ':' + key, null);
			host.storageSet(where + '#keys', null);
		},
	};

	Object.assign(globalThis, {
		engine,
		input,
		console,
		localStorage,
		setTimeout: engine.setTimeout,
		setInterval: engine.setInterval,
		clearTimeout: cancel,
		clearInterval: cancel,
	});
})

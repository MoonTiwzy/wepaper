// The scripts themselves: which layer each belongs to, the functions it exports, its
// script properties, and the entry points the runtime calls them through.
(function (we) {
	'use strict';
	const host = we.host;
	const env = we.env;

	// The order of `Handler` in event.rs.
	const HANDLERS = [
		'init', 'update', 'applyUserProperties', 'cursorEnter', 'cursorLeave', 'cursorMove', 'cursorDown', 'cursorUp', 'cursorClick',
		'destroy', 'resizeScreen', 'mediaStatusChanged', 'mediaPlaybackChanged', 'mediaPropertiesChanged', 'mediaThumbnailChanged',
		'mediaTimelineChanged', 'animationEvent', 'applyGeneralSettings',
	];
	const CURSOR_FIRST = 3, CURSOR_LAST = 8, RESIZE = 10, THUMBNAIL = 14, ANIMATION = 16;
	// How the runtime sends a value: the kind, then its components.
	const BOOL = 1, NUMBER = 2, TEXT = 3, VEC2 = 4, VEC3 = 5, VEC4 = 6;

	const scripts = [];

	// Wallpaper Engine has one `thisLayer` for the whole scene and points it at whichever
	// script is running, so a function shared with another script sees the caller's layer.
	Object.defineProperty(globalThis, 'thisLayer', { get: () => (we.current ? we.current.layer : undefined), configurable: true });
	Object.defineProperty(globalThis, 'thisObject', { get: () => (we.current ? we.current.object : undefined), configurable: true });

	const fromWire = (kind, a, b, c, d) => {
		switch (kind) {
			case BOOL: case NUMBER: case TEXT: return a;
			case VEC2: return new Vec2(a, b);
			case VEC3: return new Vec3(a, b, c);
			case VEC4: return new Vec4(a, b, c, d);
			default: return undefined;
		}
	};
	// What a handler returned, as the runtime reads it: vectors as arrays of components.
	const toWire = (value) => {
		if (value === null || typeof value !== 'object') return value;
		if (typeof value.x !== 'number' || typeof value.y !== 'number') return undefined;
		return typeof value.w === 'number' ? [value.x, value.y, value.z, value.w] : typeof value.z === 'number' ? [value.x, value.y, value.z] : [value.x, value.y];
	};

	// A script property is a plain value or `{user, value}`: the user property it follows
	// and what it is without one. Names every user property on the way into `users`.
	const resolve = (raw, users) => {
		while (raw !== null && typeof raw === 'object' && !Array.isArray(raw) && 'user' in raw) {
			const link = raw.user;
			const name = link !== null && typeof link === 'object' ? link.name : link;
			if (typeof name === 'string') {
				users.push(name);
				const value = host.userProperty(name);
				if (value !== null) return link !== null && typeof link === 'object' && 'condition' in link ? String(value) === String(link.condition) : value;
			}
			raw = raw.value;
		}
		return raw;
	};
	// Keeps the type the script declared: a colour stays a vector, a slider a number.
	const assign = (properties, key, value) => {
		const declared = properties[key];
		if (value === undefined || value === null) return;
		if (declared !== null && typeof declared === 'object' && typeof declared.x === 'number') {
			if (typeof value === 'string') value = new declared.constructor(value);
			else if (typeof value !== 'object') return;
		} else if (typeof declared === 'number' && typeof value !== 'number') {
			if (Number.isNaN(Number(value)) || value === '') return;
			value = Number(value);
		} else if (typeof declared === 'boolean' && typeof value !== 'boolean') {
			value = !(value === 0 || value === '0' || value === 'false' || value === '');
		}
		properties[key] = value;
	};
	const applyProperties = (script, changed) => {
		const properties = script.properties, raw = script.raw;
		if (properties === null || typeof properties !== 'object' || raw === null || typeof raw !== 'object') return;
		for (const key of Object.keys(raw)) {
			if (!Object.prototype.hasOwnProperty.call(properties, key)) continue;
			const users = [];
			const value = resolve(raw[key], users);
			if (changed === null || users.some((name) => Object.prototype.hasOwnProperty.call(changed, name))) assign(properties, key, value);
		}
	};

	const cursorEvent = (script) => {
		const world = new Vec3(env.cursorX, env.cursorY, 0);
		const event = { worldPosition: world, hitBox: undefined };
		// Rarely read and it costs a matrix inverse, so worked out when asked for.
		Object.defineProperty(event, 'localPosition', {
			get() {
				const local = script.layer.getTransformMatrix().inverse().transformPoint(world);
				return local.isFinite() ? local : world.subtract(script.layer.origin);
			},
			enumerable: true,
		});
		return event;
	};
	const colors = (event) => {
		for (const key of Object.keys(event)) {
			if (Array.isArray(event[key])) event[key] = new Vec3(event[key][0], event[key][1], event[key][2]);
		}
		return event;
	};

	globalThis.createScriptProperties = () => {
		const values = {};
		const add = (options) => {
			if (options && options.name !== undefined) values[options.name] = options.value;
			return builder;
		};
		const builder = {
			addSlider: add,
			addCheckbox: add,
			addText: add,
			addColor: add,
			addFile: add,
			addDirectory: add,
			// A combo starts on its first option, as it does in Wallpaper Engine.
			addCombo(options) {
				const first = options && Array.isArray(options.options) ? options.options[0] : undefined;
				if (options && options.name !== undefined) values[options.name] = first && typeof first === 'object' ? first.value : options.value;
				return builder;
			},
			finish: () => values,
		};
		return builder;
	};
	if (typeof globalThis.shared !== 'object' || globalThis.shared === null) globalThis.shared = {};

	we.api = {
		// Before a script's module runs: its top level may already use `thisLayer`.
		begin(id, layer, code, index, name) {
			we.current = scripts[id] = {
				id, code, index, name,
				layer: we.layer(layer),
				object: we.object(layer, code, index),
				handlers: new Array(HANDLERS.length).fill(null),
				properties: null,
				raw: null,
			};
		},
		// After it ran: picks up what it exports; answers with a bit for every handler it has.
		finish(id, exports, raw) {
			const script = scripts[id];
			let mask = 0;
			HANDLERS.forEach((name, i) => {
				if (typeof exports[name] !== 'function') return;
				script.handlers[i] = exports[name];
				mask |= 1 << i;
			});
			script.properties = exports.scriptProperties;
			script.raw = raw;
			applyProperties(script, null);
			return mask;
		},
		update(id, kind, a, b, c, d) {
			const script = we.current = scripts[id];
			const update = script.handlers[1];
			return toWire(update(fromWire(kind, a, b, c, d)));
		},
		// `init` and `animationEvent`: handlers that are given the value and may return it.
		value(id, handler, kind, a, b, c, d, event) {
			const script = we.current = scripts[id];
			const run = script.handlers[handler];
			const value = fromWire(kind, a, b, c, d);
			return toWire(handler === ANIMATION ? run(event, value) : run(value));
		},
		event(id, handler, payload) {
			const script = we.current = scripts[id];
			const run = script.handlers[handler];
			if (handler >= CURSOR_FIRST && handler <= CURSOR_LAST) payload = cursorEvent(script);
			else if (handler === RESIZE) payload = new Vec2(env.screenWidth, env.screenHeight);
			else if (handler === THUMBNAIL) payload = colors(payload);
			run(payload);
		},
		// Turns changed user properties into what scripts see and moves the script
		// properties that follow them.
		userProperties(changed) {
			for (const key of Object.keys(changed)) changed[key] = we.userValue(changed[key]);
			for (const script of scripts) {
				if (script) applyProperties(script, changed);
			}
			return changed;
		},
		// Takes in the environment. With `advance` a new frame starts, and the answer is
		// whether the screen changed size since the one before.
		environment(numbers, cursorDown, advance) {
			const resized = advance && env.frame > 0 && (env.screenWidth !== numbers[5] || env.screenHeight !== numbers[6]);
			[env.time, env.frametime, env.timeOfDay, env.canvasWidth, env.canvasHeight, env.screenWidth, env.screenHeight, env.cursorX, env.cursorY, env.pointerX, env.pointerY] = numbers;
			env.cursorDown = cursorDown;
			if (advance) env.frame += 1;
			return resized;
		},
		audio: we.audio,
		tick: we.tick,
		current: () => (we.current ? we.current.id : -1),
	};
})

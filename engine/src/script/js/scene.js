// `thisScene` and what it hands out: layers, effects, particle instances, animations.
// Each is a live view: reading a property asks the host, assigning one tells it.
(function (we) {
	'use strict';
	const host = we.host;

	// Field codes; `Field::code` in host.rs is the other end.
	const ORIGIN = 0, SCALE = 1, ANGLES = 2, VISIBLE = 3, ALPHA = 4, COLOR = 5, TEXT = 6, SIZE = 7, NAME = 8, PARALLAX = 9, PARENT = 10;
	const NAMED = 20, EFFECT_VISIBLE = 21, EFFECT_NAME = 22, EFFECT_CONSTANT = 23, INSTANCE = 24, ANIMATION_LAYER = 25;
	const NO_FIELD = -1;

	const ID = Symbol('layer');
	const number = (value) => typeof value === 'number';
	const vec2 = (a) => (a ? new Vec2(a[0], a[1]) : new Vec2(0, 0));
	const vec3 = (a) => (a ? new Vec3(a[0], a[1], a[2]) : new Vec3(0, 0, 0));
	// What a script may assign to a vector property: a vector, a number, or a config string.
	const toVec3 = (value) => (value instanceof Vec3 ? value : value !== null && typeof value === 'object' ? new Vec3(+value.x || 0, +value.y || 0, +value.z || 0) : new Vec3(value));
	const toVec2 = (value) => (value instanceof Vec2 ? value : value !== null && typeof value === 'object' ? new Vec2(+value.x || 0, +value.y || 0) : new Vec2(value));

	// The host's values come as plain numbers, strings and arrays of components.
	const fromWire = (value) => {
		if (!Array.isArray(value)) return value === null ? undefined : value;
		return value.length === 2 ? new Vec2(value[0], value[1]) : value.length === 3 ? new Vec3(value[0], value[1], value[2]) : new Vec4(value[0], value[1], value[2], value[3]);
	};
	const toWire = (value) => {
		if (value === null || typeof value !== 'object') return value === undefined ? null : value;
		if (value instanceof Layer) return value[ID];
		if (Array.isArray(value.m)) return value.m.join(' ');
		if (!number(value.x) || !number(value.y)) return String(value);
		return number(value.w) ? [value.x, value.y, value.z, value.w] : number(value.z) ? [value.x, value.y, value.z] : [value.x, value.y];
	};
	const getPart = (layer, code, index, name) => fromWire(host.getPart(layer, code, index, name));
	const setPart = (layer, code, index, name, value) => host.setPart(layer, code, index, name, toWire(value));

	// A method the host may or may not carry out: when it does not, the script is told so
	// once and gets `fallback`. `field` names the property an animation is bound to.
	const invoke = (layer, label, method, fallback, args, field) => {
		const [code, index, name] = field || [NO_FIELD, 0, null];
		const result = host.call(layer, code, index, name, method, args.map(toWire));
		if (result !== undefined) return fromWire(result);
		we.unsupported(label);
		return typeof fallback === 'function' ? fallback() : fallback;
	};

	const accessors = (target, table) => {
		for (const name of Object.keys(table)) {
			Object.defineProperty(target, name, { get: table[name][0], set: table[name][1], enumerable: true, configurable: true });
		}
	};
	const methods = (target, prefix, table) => {
		for (const name of Object.keys(table)) {
			const fallback = table[name];
			Object.defineProperty(target, name, {
				value(...args) { return invoke(this[ID], prefix + name, this.method ? this.method + '.' + name : name, fallback, this.key ? [this.key, ...args] : args, this.field); },
				writable: true,
				configurable: true,
			});
		}
	};

	// Timeline animations, texture animations, video textures and puppet animation layers
	// share one shape. Until the host drives them they keep what the script set, so a
	// script that plays, seeks and asks back sees its own state.
	class Playback {
		constructor(layer, method, key, field) {
			this[ID] = layer;
			this.method = method;
			this.key = key;
			this.field = field;
			this.state = { rate: 1, loop: true, blend: 1, visible: true, playing: false, frame: 0, time: 0 };
		}

		ask(method, fallback, ...args) {
			const result = host.call(this[ID], ...(this.field || [NO_FIELD, 0, null]), this.method + '.' + method, [this.key, ...args].map(toWire));
			if (result !== undefined) return fromWire(result);
			we.unsupported(this.method + '.' + method);
			return fallback;
		}

		play() { this.state.playing = true; this.ask('play'); }
		pause() { this.state.playing = false; this.ask('pause'); }
		stop() { this.state.playing = false; this.state.frame = 0; this.state.time = 0; this.ask('stop'); }
		isPlaying() { return this.ask('isPlaying', this.state.playing); }
		getFrame() { return this.ask('getFrame', this.state.frame); }
		setFrame(frame) { this.state.frame = Number(frame) || 0; this.ask('setFrame', undefined, this.state.frame); }
		getCurrentTime() { return this.ask('getCurrentTime', this.state.time); }
		setCurrentTime(time) { this.state.time = Number(time) || 0; this.ask('setCurrentTime', undefined, this.state.time); }
		join() { this.ask('join'); }
		addEndedCallback() { we.unsupported(this.method + '.addEndedCallback'); }
		getAnimation(name) { return animation(this[ID], name); }
	}
	for (const name of ['fps', 'frameCount', 'duration']) {
		Object.defineProperty(Playback.prototype, name, { get() { return this.ask('get', 0, name); }, enumerable: true, configurable: true });
	}
	Object.defineProperty(Playback.prototype, 'name', {
		get() { return this.ask('get', typeof this.key === 'string' ? this.key : '', 'name'); },
		set(value) { this.ask('set', undefined, 'name', String(value)); },
		enumerable: true,
		configurable: true,
	});
	for (const name of ['rate', 'loop', 'blend', 'visible']) {
		Object.defineProperty(Playback.prototype, name, {
			get() { return this.ask('get', this.state[name], name); },
			set(value) { this.state[name] = value; this.ask('set', undefined, name, value); },
			enumerable: true,
			configurable: true,
		});
	}

	// With no name, the animation on the property the running script is bound to.
	const animation = (layer, name) => {
		const script = we.current;
		const field = name === undefined && script ? [script.code, script.index, script.name] : null;
		return new Playback(layer, 'animation', name === undefined ? null : name, field);
	};

	// A part of a layer whose members are named by the scene: an effect's constants or a
	// particle system's instance overrides. Unknown names go to the host as they are.
	const part = (base, layer, code, index) => new Proxy(base, {
		get(target, key, receiver) {
			if (typeof key !== 'string' || key in target) return Reflect.get(target, key, receiver);
			return getPart(layer, code, index, key);
		},
		set(target, key, value, receiver) {
			if (typeof key !== 'string' || key in target) return Reflect.set(target, key, value, receiver);
			setPart(layer, code, index, key, value);
			return true;
		},
	});

	class Effect {
		constructor(layer, index) {
			this[ID] = layer;
			this.index = index;
		}

		get visible() { return getPart(this[ID], EFFECT_VISIBLE, this.index, null) !== false; }
		set visible(value) { setPart(this[ID], EFFECT_VISIBLE, this.index, null, !!value); }
		get name() { return getPart(this[ID], EFFECT_NAME, this.index, null) || ''; }
		set name(value) { we.unsupported('IEffect.name'); }
		getMaterialCount() { return 1; }
		getMaterial() { return effect(this[ID], this.index); }
		setMaterialProperty(name, value) { setPart(this[ID], EFFECT_CONSTANT, this.index, String(name), value); }
		executeMaterialFunction() { we.unsupported('IEffect.executeMaterialFunction'); }
		getAnimation(name) { return animation(this[ID], name); }
	}
	const effect = (layer, index) => part(new Effect(layer, index), layer, EFFECT_CONSTANT, index);

	class Instance {
		constructor(layer) {
			this[ID] = layer;
		}

		getAnimation(name) { return animation(this[ID], name); }
	}
	const instance = (layer) => part(new Instance(layer), layer, INSTANCE, 0);

	class Layer {
		constructor(id) {
			this[ID] = id;
		}

		getAnimation(name) { return animation(this[ID], name); }
		getParent() {
			const parent = host.get(this[ID], PARENT);
			return number(parent) ? we.layer(parent) : undefined;
		}
		getChildren() { return thisScene.enumerateLayers().filter((other) => host.get(other[ID], PARENT) === this[ID]); }
		// Composed from the layer's own values and its parents'; attachments are not followed.
		getTransformMatrix() {
			const local = Mat4.compose(this.origin, this.angles, this.scale);
			const parent = this.getParent();
			return parent ? parent.getTransformMatrix().multiply(local) : local;
		}
		getEffectCount() {
			let count = 0;
			while (count < 256 && host.getPart(this[ID], EFFECT_NAME, count, null) !== null) count++;
			return count;
		}
		getEffect(which) {
			const count = this.getEffectCount();
			const index = number(which) ? which : Array.from({ length: count }, (_, i) => host.getPart(this[ID], EFFECT_NAME, i, null)).indexOf(String(which));
			return Number.isInteger(index) && index >= 0 && index < count ? effect(this[ID], index) : undefined;
		}
		get instance() { return instance(this[ID]); }
		getTextureAnimation() { return new Playback(this[ID], 'textureAnimation', null, null); }
		getVideoTexture() { return new Playback(this[ID], 'videoTexture', null, null); }
		getAnimationLayer(which) { return new Playback(this[ID], 'animationLayer', which, null); }
		createAnimationLayer(config) {
			we.unsupported('thisLayer.createAnimationLayer');
			return new Playback(this[ID], 'animationLayer', typeof config === 'string' ? config : -1, null);
		}
		playSingleAnimation(config) {
			we.unsupported('thisLayer.playSingleAnimation');
			return new Playback(this[ID], 'animationLayer', typeof config === 'string' ? config : -1, null);
		}
	}

	const vector3 = (code) => [function () { return vec3(host.get(this[ID], code)); }, function (value) { const v = toVec3(value); host.set(this[ID], code, v.x, v.y, v.z); }];
	const vector2 = (code) => [function () { return vec2(host.get(this[ID], code)); }, function (value) { const v = toVec2(value); host.set(this[ID], code, v.x, v.y); }];
	const named = (name, fallback) => [
		function () { const value = getPart(this[ID], NAMED, 0, name); return value === undefined ? fallback : value; },
		function (value) { setPart(this[ID], NAMED, 0, name, value); },
	];
	accessors(Layer.prototype, {
		origin: vector3(ORIGIN),
		scale: vector3(SCALE),
		angles: vector3(ANGLES),
		color: vector3(COLOR),
		parallaxDepth: vector2(PARALLAX),
		size: vector2(SIZE),
		visible: [function () { return host.get(this[ID], VISIBLE) !== false; }, function (value) { host.setBool(this[ID], VISIBLE, !!value); }],
		alpha: [function () { const value = host.get(this[ID], ALPHA); return number(value) ? value : 1; }, function (value) { host.set(this[ID], ALPHA, Number(value)); }],
		text: [function () { const value = host.get(this[ID], TEXT); return typeof value === 'string' ? value : ''; }, function (value) { host.setText(this[ID], TEXT, String(value)); }],
		name: [function () { const value = host.get(this[ID], NAME); return typeof value === 'string' ? value : ''; }, function (value) { host.setText(this[ID], NAME, String(value)); }],
		// Text layers.
		pointsize: named('pointsize', 0),
		font: named('font', ''),
		padding: named('padding', 0),
		horizontalalign: named('horizontalalign', 'center'),
		verticalalign: named('verticalalign', 'center'),
		anchor: named('anchor', 'none'),
		opaquebackground: named('opaquebackground', false),
		backgroundcolor: named('backgroundcolor', undefined),
		limitrows: named('limitrows', false),
		maxrows: named('maxrows', 0),
		limitwidth: named('limitwidth', false),
		maxwidth: named('maxwidth', 0),
		// Image, model, sound and camera layers.
		alignment: named('alignment', 'center'),
		perspective: named('perspective', false),
		solid: named('solid', true),
		rootmotion: named('rootmotion', false),
		volume: named('volume', 1),
		fov: named('fov', 50),
		zoom: named('zoom', 1),
	});
	methods(Layer.prototype, 'thisLayer.', {
		rotateObjectSpace: undefined,
		lookAt: undefined,
		lookAtYaw: undefined,
		setParent: undefined,
		getAttachmentIndex: -1,
		getAttachmentMatrix: () => new Mat4(),
		getAttachmentOrigin: () => new Vec3(0, 0, 0),
		getAttachmentAngles: () => new Vec3(0, 0, 0),
		transformAttachmentToTexture: () => new Mat3(),
		play: undefined,
		pause: undefined,
		stop: undefined,
		isPlaying: true,
		emitParticles: undefined,
		getAnimationLayerCount: 0,
		destroyAnimationLayer: false,
		getBoneCount: 0,
		getBoneIndex: -1,
		getBoneParentIndex: -1,
		setBoneTransform: undefined,
		setLocalBoneTransform: undefined,
		getLocalBoneAngles: () => new Vec3(0, 0, 0),
		setLocalBoneAngles: undefined,
		getLocalBoneOrigin: () => new Vec3(0, 0, 0),
		setLocalBoneOrigin: undefined,
		applyBonePhysicsImpulse: undefined,
		resetBonePhysicsSimulation: undefined,
		getBlendShapeIndex: -1,
		getBlendShapeWeight: 0,
		setBlendShapeWeight: undefined,
	});
	// The host answers these with sixteen numbers in a string.
	for (const name of ['getBoneTransform', 'getLocalBoneTransform']) {
		Layer.prototype[name] = function (bone) {
			return new Mat4(invoke(this[ID], 'thisLayer.' + name, name, undefined, [bone], null));
		};
	}

	// One object per layer for the whole scene, so what a script hangs on a layer is there
	// for the next script that looks the layer up.
	const layers = new Map();
	we.layer = (id) => {
		let layer = layers.get(id);
		if (!layer) layers.set(id, layer = new Layer(id));
		return layer;
	};
	// What `thisObject` is for a script bound to `code`: the part of the layer that owns it.
	we.object = (layer, code, index) => {
		if (code === EFFECT_VISIBLE || code === EFFECT_NAME || code === EFFECT_CONSTANT) return effect(layer, index);
		if (code === INSTANCE) return instance(layer);
		if (code === ANIMATION_LAYER) return new Playback(layer, 'animationLayer', index, null);
		return we.layer(layer);
	};

	const idOf = (which) => {
		if (which instanceof Layer) return which[ID];
		if (number(which)) {
			const all = host.layers();
			return Number.isInteger(which) && which >= 0 && which < all.length ? all[which] : -1;
		}
		return host.layerNamed(String(which));
	};
	// A layer nothing is drawn for: what creating one returns until the host can make them.
	const detached = () => new Layer(-1);

	const scene = {
		getLayer(which) {
			const id = idOf(which);
			return id < 0 ? undefined : we.layer(id);
		},
		getLayerByID(id) {
			const found = host.layerWithId(Number(id));
			return found < 0 ? undefined : we.layer(found);
		},
		getLayerCount() { return host.layers().length; },
		enumerateLayers() { return host.layers().map(we.layer); },
		getLayerIndex(which) { return host.layers().indexOf(idOf(which)); },
		sortLayer(which, index) { return invoke(idOf(which), 'thisScene.sortLayer', 'sortLayer', false, [index], null); },
		destroyLayer(which) { return invoke(idOf(which), 'thisScene.destroyLayer', 'destroyLayer', false, [], null); },
		createLayer() {
			we.unsupported('thisScene.createLayer');
			return detached();
		},
		getInitialLayerConfig: we.stub('thisScene.getInitialLayerConfig', () => ({})),
		getCameraTransforms: we.stub('thisScene.getCameraTransforms', () => ({ eye: new Vec3(0, 0, 1), center: new Vec3(0, 0, 0), up: new Vec3(0, 1, 0), zoom: 1 })),
		setCameraTransforms: we.stub('thisScene.setCameraTransforms'),
		getAnimation(name) { return animation(-1, name); },
		createModelData: we.stub('thisScene.createModelData', () => ({ applyData() {}, replaceData() {}, getAnimation: (name) => animation(-1, name) })),
		destroyModelData: we.stub('thisScene.destroyModelData'),
	};
	const sceneValue = (name, fallback) => [
		() => { const value = fromWire(host.sceneGet(name)); return value === undefined ? (typeof fallback === 'function' ? fallback() : fallback) : value; },
		(value) => host.sceneSet(name, toWire(value)),
	];
	const black = () => new Vec3(0, 0, 0);
	accessors(scene, {
		bloom: sceneValue('bloom', false),
		bloomstrength: sceneValue('bloomstrength', 0),
		bloomthreshold: sceneValue('bloomthreshold', 0),
		clearenabled: sceneValue('clearenabled', true),
		clearcolor: sceneValue('clearcolor', black),
		ambientcolor: sceneValue('ambientcolor', black),
		skylightcolor: sceneValue('skylightcolor', black),
		fov: sceneValue('fov', 50),
		nearz: sceneValue('nearz', 0.01),
		farz: sceneValue('farz', 10000),
		camerafade: sceneValue('camerafade', false),
		camerashake: sceneValue('camerashake', false),
		camerashakespeed: sceneValue('camerashakespeed', 0),
		camerashakeamplitude: sceneValue('camerashakeamplitude', 0),
		camerashakeroughness: sceneValue('camerashakeroughness', 0),
		cameraparallax: sceneValue('cameraparallax', false),
		cameraparallaxamount: sceneValue('cameraparallaxamount', 0),
		cameraparallaxdelay: sceneValue('cameraparallaxdelay', 0),
		cameraparallaxmouseinfluence: sceneValue('cameraparallaxmouseinfluence', 0),
	});

	globalThis.thisScene = scene;
	globalThis.renderContext = {};
})

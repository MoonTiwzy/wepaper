// The vector and matrix classes of SceneScript, for a host that cannot supply Wallpaper
// Engine's own `scripts/jsclasses/baseclasses.js`. Matrices are column-major, angles are
// in degrees, and every operation returns a new object, as documented.

class Vec2 {
	constructor(...args) {
		Vec2.fill(this, args);
	}
}

class Vec3 {
	constructor(...args) {
		Vec3.fill(this, args);
	}

	static fromSpherical(r, theta, phi) {
		const t = theta * Math.PI / 180, p = phi * Math.PI / 180;
		return new Vec3(r * Math.sin(t) * Math.cos(p), r * Math.cos(t), r * Math.sin(t) * Math.sin(p));
	}
}

class Vec4 {
	constructor(...args) {
		Vec4.fill(this, args);
	}
}

class Mat3 {
	constructor(from) {
		this.m = Mat3.elements(from);
	}
}

class Mat4 {
	constructor(from) {
		this.m = Mat4.elements(from);
	}
}

class MediaPlaybackEvent {
	static PLAYBACK_STOPPED = 0;
	static PLAYBACK_PLAYING = 1;
	static PLAYBACK_PAUSED = 2;
}

{
	const EPSILON = 0.00001;
	const RAD = Math.PI / 180, DEG = 180 / Math.PI;
	const number = (value) => typeof value === 'number';
	const methods = (Class, table, target = Class.prototype) => {
		for (const name of Object.keys(table)) {
			Object.defineProperty(target, name, { value: table[name], writable: true, configurable: true });
		}
	};

	const vector = (Class, keys) => {
		const make = (pick) => {
			const out = new Class();
			for (const key of keys) out[key] = pick(key);
			return out;
		};
		// A component of an operand: the number itself, the vector's component, or
		// `missing` where a shorter vector has none (so a Vec3 keeps its z against a Vec2).
		const part = (operand, key, missing) => (number(operand) ? operand : number(operand[key]) ? operand[key] : missing);
		const zip = (self, operand, missing, op) => make((key) => op(self[key], part(operand, key, missing)));
		const map = (self, op) => make((key) => op(self[key]));
		const hermite = (t) => t * t * (3 - 2 * t);

		methods(Class, {
			fill(self, args) {
				const first = args[0];
				if (typeof first === 'string') {
					const parts = first.trim().split(/\s+/);
					keys.forEach((key, i) => { self[key] = i < parts.length ? parseFloat(parts[i]) : 0; });
				} else if (first !== null && typeof first === 'object') {
					for (const key of keys) self[key] = number(first[key]) ? first[key] : 0;
				} else if (first === undefined) {
					for (const key of keys) self[key] = 0;
				} else {
					// One number fills every component; several leave the rest at zero.
					const given = number(args[1]) ? args.findIndex((arg) => !number(arg)) : 1;
					const count = given < 0 ? args.length : given;
					keys.forEach((key, i) => { self[key] = i < count ? args[i] : count === 1 ? first : 0; });
				}
			},
		}, Class);

		methods(Class, {
			lengthSqr() { return keys.reduce((sum, key) => sum + this[key] * this[key], 0); },
			length() { return Math.sqrt(this.lengthSqr()); },
			distanceSqr(other) { return keys.reduce((sum, key) => sum + (this[key] - other[key]) ** 2, 0); },
			distance(other) { return Math.sqrt(this.distanceSqr(other)); },
			normalize() { return this.divide(this.length()); },
			copy() { return map(this, (a) => a); },
			equals(other) { return other instanceof Class && keys.every((key) => Math.abs(this[key] - other[key]) < EPSILON); },
			isFinite() { return keys.every((key) => Number.isFinite(this[key])); },
			negate() { return map(this, (a) => -a); },
			add(value) { return zip(this, value, 0, (a, b) => a + b); },
			subtract(value) { return zip(this, value, 0, (a, b) => a - b); },
			multiply(value) { return zip(this, value, 1, (a, b) => a * b); },
			divide(value) { return zip(this, value, 1, (a, b) => a / b); },
			dot(value) { return keys.reduce((sum, key) => sum + this[key] * value[key], 0); },
			reflect(normal) { return this.subtract(normal.multiply(2 * this.dot(normal))); },
			project(onto) {
				const d = onto.lengthSqr();
				return d === 0 ? new Class() : onto.multiply(this.dot(onto) / d);
			},
			mix(other, amount) { return make((key) => this[key] + (other[key] - this[key]) * part(amount, key, 0)); },
			min(value) { return zip(this, value, Infinity, Math.min); },
			max(value) { return zip(this, value, -Infinity, Math.max); },
			clamp(min, max) { return make((key) => Math.max(part(min, key, -Infinity), Math.min(part(max, key, Infinity), this[key]))); },
			abs() { return map(this, Math.abs); },
			sign() { return map(this, Math.sign); },
			round() { return map(this, Math.round); },
			floor() { return map(this, Math.floor); },
			ceil() { return map(this, Math.ceil); },
			fract() { return map(this, (a) => a - Math.floor(a)); },
			mod(value) { return zip(this, value, 1, (a, b) => a - b * Math.floor(a / b)); },
			step(edge) { return zip(this, edge, 0, (a, b) => (a < b ? 0 : 1)); },
			smoothStep(min, max) {
				return make((key) => {
					const from = part(min, key, 0), to = part(max, key, 1);
					return hermite(Math.max(0, Math.min(1, (this[key] - from) / (to - from))));
				});
			},
			toString() { return keys.map((key) => this[key]).join(' '); },
			toConfigString() { return this.toString(); },
		});
	};

	vector(Vec2, ['x', 'y']);
	vector(Vec3, ['x', 'y', 'z']);
	vector(Vec4, ['x', 'y', 'z', 'w']);

	methods(Vec2, {
		perpendicular() { return new Vec2(this.y, -this.x); },
		angle() { return Math.atan2(this.y, this.x) * DEG; },
		angleBetween(other) { return Math.atan2(this.x * other.y - this.y * other.x, this.x * other.x + this.y * other.y) * DEG; },
		rotate(angle) {
			const c = Math.cos(angle * RAD), s = Math.sin(angle * RAD);
			return new Vec2(c * this.x - s * this.y, s * this.x + c * this.y);
		},
	});

	methods(Vec3, {
		cross(other) {
			return new Vec3(this.y * other.z - this.z * other.y, this.z * other.x - this.x * other.z, this.x * other.y - this.y * other.x);
		},
		refract(normal, eta) {
			const d = normal.dot(this);
			const k = 1 - eta * eta * (1 - d * d);
			return k < 0 ? new Vec3(0, 0, 0) : this.multiply(eta).subtract(normal.multiply(eta * d + Math.sqrt(k)));
		},
		angleBetween(other) {
			const d = Math.sqrt(this.lengthSqr() * other.lengthSqr());
			return d === 0 ? 0 : Math.acos(Math.max(-1, Math.min(1, this.dot(other) / d))) * DEG;
		},
		toSpherical() {
			const r = this.length();
			return r === 0 ? new Vec3(0, 0, 0) : new Vec3(r, Math.acos(this.y / r) * DEG, Math.atan2(this.z, this.x) * DEG);
		},
	});

	// What both matrix sizes share. `n` is the side; element (row, column) is m[column * n + row].
	const matrix = (Class, n) => {
		const identity = () => Array.from({ length: n * n }, (_, i) => (i % (n + 1) === 0 ? 1 : 0));
		const wrap = (m) => {
			const out = new Class();
			out.m = m;
			return out;
		};
		const product = (a, b) => {
			const out = new Array(n * n).fill(0);
			for (let column = 0; column < n; column++) {
				for (let row = 0; row < n; row++) {
					for (let k = 0; k < n; k++) out[column * n + row] += a[k * n + row] * b[column * n + k];
				}
			}
			return out;
		};
		// Gauss-Jordan elimination on [m | identity]; also yields the determinant.
		const eliminate = (m) => {
			const a = m.slice(), inverse = identity();
			let determinant = 1;
			for (let column = 0; column < n; column++) {
				let pivot = column;
				for (let row = column + 1; row < n; row++) {
					if (Math.abs(a[column * n + row]) > Math.abs(a[column * n + pivot])) pivot = row;
				}
				if (a[column * n + pivot] === 0) return { determinant: 0, inverse: inverse.map(() => NaN) };
				if (pivot !== column) {
					for (let k = 0; k < n; k++) {
						[a[k * n + column], a[k * n + pivot]] = [a[k * n + pivot], a[k * n + column]];
						[inverse[k * n + column], inverse[k * n + pivot]] = [inverse[k * n + pivot], inverse[k * n + column]];
					}
					determinant = -determinant;
				}
				const scale = a[column * n + column];
				determinant *= scale;
				for (let k = 0; k < n; k++) {
					a[k * n + column] /= scale;
					inverse[k * n + column] /= scale;
				}
				for (let row = 0; row < n; row++) {
					const factor = a[column * n + row];
					if (row === column || factor === 0) continue;
					for (let k = 0; k < n; k++) {
						a[k * n + row] -= factor * a[k * n + column];
						inverse[k * n + row] -= factor * inverse[k * n + column];
					}
				}
			}
			return { determinant, inverse };
		};

		methods(Class, {
			elements(from) {
				if (from instanceof Class) return from.m.slice();
				if (Array.isArray(from) && from.length === n * n) return from.slice();
				if (typeof from === 'string') {
					const parts = from.trim().split(/\s+/).map(parseFloat);
					if (parts.length === n * n) return parts;
				}
				return identity();
			},
			identity() { return new Class(); },
			wrap,
			product,
		}, Class);

		methods(Class, {
			add(other) { return wrap(this.m.map((a, i) => a + other.m[i])); },
			subtract(other) { return wrap(this.m.map((a, i) => a - other.m[i])); },
			transpose() { return wrap(this.m.map((_, i) => this.m[(i % n) * n + Math.floor(i / n)])); },
			determinant() { return eliminate(this.m).determinant; },
			inverse() { return wrap(eliminate(this.m).inverse); },
			copy() { return new Class(this); },
			equals(other) { return other instanceof Class && this.m.every((a, i) => Math.abs(a - other.m[i]) < EPSILON); },
			toString() { return this.m.join(' '); },
			toConfigString() { return this.toString(); },
		});
	};

	matrix(Mat3, 3);
	matrix(Mat4, 4);

	methods(Mat3, {
		fromTranslation(v) {
			const out = new Mat3();
			out.m[6] = v.x;
			out.m[7] = v.y;
			return out;
		},
		fromScale(v) {
			const out = new Mat3();
			out.m[0] = number(v) ? v : v.x;
			out.m[4] = number(v) ? v : v.y;
			return out;
		},
		fromRotation(angle) {
			const c = Math.cos(angle * RAD), s = Math.sin(angle * RAD);
			return Mat3.wrap([c, s, 0, -s, c, 0, 0, 0, 1]);
		},
		fromBasis(right, up) { return Mat3.wrap([right.x, right.y, 0, up.x, up.y, 0, 0, 0, 1]); },
		fromMat4(matrix) {
			const a = matrix.m;
			return Mat3.wrap([a[0], a[1], a[2], a[4], a[5], a[6], a[8], a[9], a[10]]);
		},
		compose(translation, rotation, scale) { return Mat3.fromTranslation(translation).rotate(rotation).scale(scale); },
	}, Mat3);

	methods(Mat3, {
		right() { return new Vec3(this.m[0], this.m[1], this.m[2]); },
		up() { return new Vec3(this.m[3], this.m[4], this.m[5]); },
		forward() { return new Vec3(this.m[6], this.m[7], this.m[8]); },
		translation(position) {
			if (!(position instanceof Vec2)) return new Vec2(this.m[6], this.m[7]);
			this.m[6] = position.x;
			this.m[7] = position.y;
			return this;
		},
		angle() { return Math.atan2(this.m[0], -this.m[1]) * DEG; },
		multiply(value) {
			if (value instanceof Mat3) return Mat3.wrap(Mat3.product(this.m, value.m));
			if (number(value)) return Mat3.wrap(this.m.map((a) => a * value));
			const m = this.m;
			return new Vec3(
				m[0] * value.x + m[3] * value.y + m[6] * value.z,
				m[1] * value.x + m[4] * value.y + m[7] * value.z,
				m[2] * value.x + m[5] * value.y + m[8] * value.z,
			);
		},
		translate(v) { return this.multiply(Mat3.fromTranslation(v)); },
		rotate(angle) { return this.multiply(Mat3.fromRotation(angle)); },
		scale(v) { return this.multiply(Mat3.fromScale(v)); },
		transformPoint(v) {
			const m = this.m;
			return new Vec2(m[0] * v.x + m[3] * v.y + m[6], m[1] * v.x + m[4] * v.y + m[7]);
		},
		transformDirection(v) {
			const m = this.m;
			return new Vec2(m[0] * v.x + m[3] * v.y, m[1] * v.x + m[4] * v.y);
		},
		decompose() {
			const m = this.m;
			const mirrored = m[0] * m[4] - m[3] * m[1] < 0;
			const sx = Math.hypot(m[0], m[1]) * (mirrored ? -1 : 1);
			return {
				translation: new Vec2(m[6], m[7]),
				rotation: sx !== 0 ? Math.atan2(m[1] / sx, m[0] / sx) * DEG : 0,
				scale: new Vec2(sx, Math.hypot(m[3], m[4])),
			};
		},
	});

	methods(Mat4, {
		fromTranslation(v) {
			const out = new Mat4();
			out.m[12] = v.x;
			out.m[13] = v.y;
			out.m[14] = number(v.z) ? v.z : 0;
			return out;
		},
		fromScale(v) {
			const out = new Mat4();
			out.m[0] = number(v) ? v : v.x;
			out.m[5] = number(v) ? v : v.y;
			out.m[10] = number(v) ? v : v.z;
			return out;
		},
		fromRotation(angle, axis) {
			const { x, y, z } = axis.normalize();
			const c = Math.cos(angle * RAD), s = Math.sin(angle * RAD), t = 1 - c;
			return Mat4.wrap([
				c + x * x * t, x * y * t + z * s, x * z * t - y * s, 0,
				y * x * t - z * s, c + y * y * t, y * z * t + x * s, 0,
				z * x * t + y * s, z * y * t - x * s, c + z * z * t, 0,
				0, 0, 0, 1,
			]);
		},
		// Roll about z, then pitch about y, then yaw about x, applied right to left.
		fromEuler(x, y, z) {
			const angles = x instanceof Vec3 ? x : new Vec3(x, y, z);
			return Mat4.fromRotation(angles.z, new Vec3(0, 0, 1))
				.multiply(Mat4.fromRotation(angles.y, new Vec3(0, 1, 0)))
				.multiply(Mat4.fromRotation(angles.x, new Vec3(1, 0, 0)));
		},
		fromBasis(right, up, forward) {
			return Mat4.wrap([right.x, right.y, right.z, 0, up.x, up.y, up.z, 0, forward.x, forward.y, forward.z, 0, 0, 0, 0, 1]);
		},
		lookAt(eye, center, up) {
			const f = center.subtract(eye).normalize();
			const s = f.cross(up).normalize();
			const u = s.cross(f);
			return Mat4.wrap([s.x, u.x, -f.x, 0, s.y, u.y, -f.y, 0, s.z, u.z, -f.z, 0, -s.dot(eye), -u.dot(eye), f.dot(eye), 1]);
		},
		compose(translation, rotation, scale) {
			return Mat4.fromTranslation(translation).multiply(Mat4.fromEuler(rotation)).scale(scale);
		},
	}, Mat4);

	methods(Mat4, {
		translation(position) {
			if (!(position instanceof Vec3 || position instanceof Vec2)) return new Vec3(this.m[12], this.m[13], this.m[14]);
			this.m[12] = position.x;
			this.m[13] = position.y;
			this.m[14] = number(position.z) ? position.z : 0;
			return this;
		},
		right() { return new Vec3(this.m[0], this.m[1], this.m[2]); },
		up() { return new Vec3(this.m[4], this.m[5], this.m[6]); },
		forward() { return new Vec3(this.m[8], this.m[9], this.m[10]); },
		multiply(value) {
			if (value instanceof Mat4) return Mat4.wrap(Mat4.product(this.m, value.m));
			if (number(value)) return Mat4.wrap(this.m.map((a) => a * value));
			const m = this.m;
			return new Vec4(
				m[0] * value.x + m[4] * value.y + m[8] * value.z + m[12] * value.w,
				m[1] * value.x + m[5] * value.y + m[9] * value.z + m[13] * value.w,
				m[2] * value.x + m[6] * value.y + m[10] * value.z + m[14] * value.w,
				m[3] * value.x + m[7] * value.y + m[11] * value.z + m[15] * value.w,
			);
		},
		translate(v) { return this.multiply(Mat4.fromTranslation(v)); },
		rotate(angle, axis) { return this.multiply(Mat4.fromRotation(angle, axis)); },
		scale(v) { return this.multiply(Mat4.fromScale(v)); },
		transformPoint(v) {
			const m = this.m;
			return new Vec3(
				m[0] * v.x + m[4] * v.y + m[8] * v.z + m[12],
				m[1] * v.x + m[5] * v.y + m[9] * v.z + m[13],
				m[2] * v.x + m[6] * v.y + m[10] * v.z + m[14],
			);
		},
		transformDirection(v) {
			const m = this.m;
			return new Vec3(m[0] * v.x + m[4] * v.y + m[8] * v.z, m[1] * v.x + m[5] * v.y + m[9] * v.z, m[2] * v.x + m[6] * v.y + m[10] * v.z);
		},
		extractEuler() {
			const m = this.m;
			return new Vec3(Math.atan2(m[6], m[10]) * DEG, Math.atan2(-m[2], Math.hypot(m[6], m[10])) * DEG, Math.atan2(m[1], m[0]) * DEG);
		},
		normalMatrix() { return Mat3.fromMat4(this).inverse().transpose(); },
		decompose() {
			const m = this.m;
			const mirrored = Mat3.fromMat4(this).determinant() < 0;
			const scale = new Vec3(Math.hypot(m[0], m[1], m[2]) * (mirrored ? -1 : 1), Math.hypot(m[4], m[5], m[6]), Math.hypot(m[8], m[9], m[10]));
			const rotation = new Mat4(this);
			[scale.x, scale.y, scale.z].forEach((s, column) => {
				for (let row = 0; row < 3 && s !== 0; row++) rotation.m[column * 4 + row] /= s;
			});
			return { translation: new Vec3(m[12], m[13], m[14]), rotation: rotation.extractEuler(), scale };
		},
	});
}

this.IModelData = class IModelData {
	static POSITION = 'position';
	static NORMAL = 'normal';
	static UV = 'uv';
	static TANGENT_SIGNED = 'tangentSigned';
	static COLOR = 'color';
};
this.shared = {};

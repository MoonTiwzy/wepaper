// `WEColor`, for a host that cannot supply Wallpaper Engine's own module.
export function rgb2hsv(rgb) {
	const max = Math.max(rgb.x, rgb.y, rgb.z), min = Math.min(rgb.x, rgb.y, rgb.z);
	const range = max - min;
	let hue = 0;
	if (range !== 0) {
		if (max === rgb.x) hue = ((rgb.y - rgb.z) / range + 6) % 6;
		else if (max === rgb.y) hue = (rgb.z - rgb.x) / range + 2;
		else hue = (rgb.x - rgb.y) / range + 4;
	}
	return new Vec3(hue / 6, max === 0 ? 0 : range / max, max);
}

export function hsv2rgb(hsv) {
	const sector = hsv.x * 6;
	const channel = (offset) => {
		const k = (offset + sector) % 6;
		return hsv.z - hsv.z * hsv.y * Math.max(0, Math.min(k, 4 - k, 1));
	};
	return new Vec3(channel(5), channel(3), channel(1));
}

export function normalizeColor(color) {
	return new Vec3(color.x / 255, color.y / 255, color.z / 255);
}

export function expandColor(color) {
	return new Vec3(color.x * 255, color.y * 255, color.z * 255);
}

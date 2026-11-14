// `WEVector`, for a host that cannot supply Wallpaper Engine's own module.
export function angleVector2(angle) {
	const radians = angle * Math.PI / 180;
	return new Vec2(Math.cos(radians), Math.sin(radians));
}

export function vectorAngle2(direction) {
	return Math.atan2(direction.y, direction.x) * 180 / Math.PI;
}

//! The lighting a shader asks for with `#require LightingV1`.
//!
//! Wallpaper Engine's lit shaders (`generic4`, `genericimage4`, `genericparticle`, ...)
//! call `PerformLighting_V1` without defining it: the editor writes the function for
//! each scene, from the lights it has. Here it is written once, for any number of
//! lights, and the scene says how many with the combo `WEPAPER_LIGHTS`. A pass that is
//! not lit, or a scene without lights, defines it as 0: the function returns no light,
//! declares no uniforms and the driver drops it.
//!
//! Each light is two `vec4`s, set by the scene every frame:
//!
//! - `g_LightsColorRadius`: the colour times the intensity, then the radius. A radius
//!   of 0 marks light from infinitely far away (a directional light).
//! - `g_LightsPositionExponent`: where the light is in the world, then how its light
//!   falls off; for a directional light, the way to the light instead.
//!
//! The light itself is the stock shaders' own physically based light
//! (`ComputePBRLightShadow` and `ComputePBRLightShadowInfinite` of `common_pbr_2.h`,
//! which every shader that requires the lighting includes first), without shadows.

/// The text that stands where a shader says `#require LightingV1`.
pub(super) const LIGHTING_V1: &str = "\
#ifndef WEPAPER_LIGHTS
#define WEPAPER_LIGHTS 0
#endif
#if WEPAPER_LIGHTS
uniform vec4 g_LightsColorRadius[WEPAPER_LIGHTS];
uniform vec4 g_LightsPositionExponent[WEPAPER_LIGHTS];
#endif
vec3 PerformLighting_V1(vec3 worldPos, vec3 albedo, vec3 normal, vec3 viewVector, vec3 specularTint,
	vec3 baseReflectance, float roughness, float metallic)
{
	vec3 light = CAST3(0.0);
#if WEPAPER_LIGHTS
	for (int l = 0; l < WEPAPER_LIGHTS; ++l)
	{
		vec4 colorRadius = g_LightsColorRadius[l];
		vec4 positionExponent = g_LightsPositionExponent[l];
		if (colorRadius.w > 0.0)
		{
			light += ComputePBRLightShadow(normal, positionExponent.xyz - worldPos, viewVector, albedo,
				colorRadius.rgb, colorRadius.w, positionExponent.w, specularTint, baseReflectance, roughness,
				metallic, 1.0);
		}
		else
		{
			light += ComputePBRLightShadowInfinite(normal, positionExponent.xyz, viewVector, albedo,
				colorRadius.rgb, specularTint, baseReflectance, roughness, metallic, 1.0);
		}
	}
#endif
	return light;
}
";

/// What a `#require` line is replaced by: the text for a requirement this module
/// writes, `None` for one it does not know.
pub(super) fn required(name: &str) -> Option<&'static str> {
    (name == "LightingV1").then_some(LIGHTING_V1)
}

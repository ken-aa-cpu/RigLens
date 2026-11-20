use egui_glow::glow::{self, HasContext};
use egui_glow::ShaderVersion;

const PART_VERT_BODY: &str = r#"
uniform mat4 mvp;
uniform vec2 offset;

layout(location = 0) in vec2 verts;
layout(location = 1) in vec2 uvs;
layout(location = 2) in vec2 deform;

out vec2 texUVs;

void main() {
	gl_Position = mvp * vec4(verts - offset + deform, 0.0, 1.0);
	texUVs = uvs;
}
"#;

const QUAD_VERT_BODY: &str = r#"
layout(location = 0) in vec2 aPos;
layout(location = 1) in vec2 aUv;

out vec2 texUVs;

void main() {
	gl_Position = vec4(aPos, 0.0, 1.0);
	texUVs = aUv;
}
"#;

const BLEND_GLSL_HELPERS: &str = r#"
vec3 blend_channel(int mode, vec3 s, vec3 d) {
	if (mode == 1) {
		return s * d;
	} else if (mode == 2) {
		return 1.0 - (1.0 - s) * (1.0 - d);
	} else if (mode == 3) {
		vec3 lo = 2.0 * s * d;
		vec3 hi = 1.0 - 2.0 * (1.0 - s) * (1.0 - d);
		return mix(lo, hi, step(vec3(0.5), d));
	} else if (mode == 4) {
		vec3 lo = 2.0 * s * d;
		vec3 hi = 1.0 - 2.0 * (1.0 - s) * (1.0 - d);
		return mix(lo, hi, step(vec3(0.5), s));
	} else if (mode == 5) {
		vec3 lo = d - (1.0 - 2.0 * s) * d * (1.0 - d);
		vec3 g_lo = ((16.0 * d - 12.0) * d + 4.0) * d;
		vec3 g = mix(sqrt(max(d, vec3(0.0))), g_lo, step(d, vec3(0.25)));
		vec3 hi = d + (2.0 * s - 1.0) * (g - d);
		return mix(hi, lo, step(s, vec3(0.5)));
	} else if (mode == 6) {
		return min(s, d);
	} else if (mode == 7) {
		return max(s, d);
	} else if (mode == 8) {
		return mix(d / max(vec3(0.001), 1.0 - s), vec3(1.0), step(vec3(0.999), s));
	} else if (mode == 9) {
		return s + d;
	} else if (mode == 10) {
		return mix(1.0 - (1.0 - d) / max(vec3(0.001), s), vec3(0.0), step(s, vec3(0.001)));
	} else if (mode == 11) {
		return abs(s - d);
	} else if (mode == 12) {
		return s + d - 2.0 * s * d;
	} else if (mode == 13) {
		return d - s;
	} else if (mode == 14) {
		return 1.0 - d;
	}
	return s;
}

vec4 apply_backdrop_blend(int mode, vec3 straight, float sa, vec4 dstPremul) {
	if (mode == 15) {
		float out_a = clamp(dstPremul.a * sa, 0.0, 1.0);
		if (out_a <= 0.001) {
			return vec4(0.0);
		}
		return vec4(dstPremul.rgb * sa, out_a);
	}
	if (sa <= 0.001) {
		return dstPremul;
	}
	float da = clamp(dstPremul.a, 0.0, 1.0);
	vec3 d = (da > 0.0005) ? clamp(dstPremul.rgb / da, 0.0, 1.0) : vec3(0.0);
	vec3 b = clamp(blend_channel(mode, straight, d), 0.0, 1.0);
	float out_a = clamp(sa + da * (1.0 - sa), 0.0, 1.0);
	vec3 out_premul = straight * sa * (1.0 - da) + dstPremul.rgb * (1.0 - sa) + b * sa * da;
	return vec4(out_premul, out_a);
}
"#;

const PART_FRAG_BODY: &str = r#"
in vec2 texUVs;
out vec4 outColor;

uniform sampler2D albedo;
uniform sampler2D backdrop;
uniform float opacity;
uniform vec3 multColor;
uniform vec3 screenColor;
uniform int blendMode;

void main() {
	if (blendMode == 16) {
		outColor = vec4(0.43 * 0.55, 0.91 * 0.55, 0.72 * 0.55, 0.55);
		return;
	}
	vec4 texColor = texture(albedo, texUVs);
	float ta = clamp(texColor.a, 0.0, 1.0);
	float sa = clamp(ta * opacity, 0.0, 1.0);

	if (blendMode == 0) {
		// Exact match to inox2d-opengl basic.frag on premultiplied RGBA8 textures
		vec3 screenPremul = vec3(1.0) - ((vec3(1.0) - texColor.rgb) * (vec3(1.0) - screenColor * ta));
		outColor = vec4(clamp(screenPremul * multColor, 0.0, 1.0) * opacity, sa);
		return;
	}
	vec3 texStraight = (ta > 0.0005) ? clamp(texColor.rgb / ta, 0.0, 1.0) : vec3(0.0);
	vec3 screenOut = vec3(1.0) - ((vec3(1.0) - texStraight) * (vec3(1.0) - screenColor));
	vec3 straight = clamp(screenOut * multColor, 0.0, 1.0);
	vec4 dst = texelFetch(backdrop, ivec2(gl_FragCoord.xy), 0);
	outColor = apply_backdrop_blend(blendMode, straight, sa, dst);
}
"#;

const PART_MASK_FRAG_BODY: &str = r#"
in vec2 texUVs;
out vec4 outColor;

uniform sampler2D tex;
uniform float threshold;

void main() {
	vec4 color = texture(tex, texUVs);
	if (color.a <= threshold) {
		discard;
	}
	outColor = vec4(1.0);
}
"#;

const COMP_FRAG_BODY: &str = r#"
in vec2 texUVs;
out vec4 outColor;

uniform sampler2D fbo;
uniform sampler2D backdrop;
uniform float opacity;
uniform vec3 multColor;
uniform vec3 screenColor;
uniform int blendMode;

void main() {
	vec4 c = texture(fbo, texUVs);
	float a = clamp(c.a, 0.0, 1.0);
	vec3 straight = (a > 0.0005) ? clamp(c.rgb / a, 0.0, 1.0) : vec3(0.0);
	vec3 screenOut = vec3(1.0) - ((vec3(1.0) - straight) * (vec3(1.0) - screenColor * a));
	vec3 finalStraight = clamp(screenOut * multColor, 0.0, 1.0);
	float sa = clamp(a * opacity, 0.0, 1.0);

	if (blendMode == 0) {
		outColor = vec4(finalStraight * sa, sa);
		return;
	}
	vec4 dst = texelFetch(backdrop, ivec2(gl_FragCoord.xy), 0);
	outColor = apply_backdrop_blend(blendMode, finalStraight, sa, dst);
}
"#;

const COMP_MASK_FRAG_BODY: &str = r#"
in vec2 texUVs;
out vec4 outColor;

uniform sampler2D fbo;
uniform float threshold;
uniform float opacity;

void main() {
	vec4 c = texture(fbo, texUVs);
	if (c.a * opacity <= threshold) {
		discard;
	}
	outColor = vec4(1.0);
}
"#;

const BLIT_FRAG_BODY: &str = r#"
in vec2 texUVs;
out vec4 outColor;

uniform sampler2D fbo;

void main() {
	outColor = texture(fbo, texUVs);
}
"#;

fn shader_prefix(gl: &glow::Context) -> &'static str {
	let sv = ShaderVersion::get(gl);
	match sv {
		ShaderVersion::Es100 | ShaderVersion::Es300 => "#version 300 es\nprecision highp float;\n",
		ShaderVersion::Gl120 | ShaderVersion::Gl140 => "#version 330 core\n",
	}
}

pub(super) fn is_gles(gl: &glow::Context) -> bool {
	matches!(ShaderVersion::get(gl), ShaderVersion::Es100 | ShaderVersion::Es300)
}

unsafe fn compile_program(gl: &glow::Context, prefix: &str, vert_src: &str, frag_src: &str) -> Result<glow::Program, String> {
	let prog = gl.create_program()?;
	let vs = gl.create_shader(glow::VERTEX_SHADER)?;
	let fs = gl.create_shader(glow::FRAGMENT_SHADER)?;

	let v_full = format!("{prefix}{vert_src}");
	let f_full = format!("{prefix}{frag_src}");
	gl.shader_source(vs, &v_full);
	gl.compile_shader(vs);
	if !gl.get_shader_compile_status(vs) {
		let err = gl.get_shader_info_log(vs);
		gl.delete_shader(vs);
		gl.delete_shader(fs);
		gl.delete_program(prog);
		return Err(format!("Vertex shader compile error: {err}"));
	}

	gl.shader_source(fs, &f_full);
	gl.compile_shader(fs);
	if !gl.get_shader_compile_status(fs) {
		let err = gl.get_shader_info_log(fs);
		gl.delete_shader(vs);
		gl.delete_shader(fs);
		gl.delete_program(prog);
		return Err(format!("Fragment shader compile error: {err}"));
	}

	gl.attach_shader(prog, vs);
	gl.attach_shader(prog, fs);
	gl.link_program(prog);
	gl.detach_shader(prog, vs);
	gl.detach_shader(prog, fs);
	gl.delete_shader(vs);
	gl.delete_shader(fs);

	if !gl.get_program_link_status(prog) {
		let err = gl.get_program_info_log(prog);
		gl.delete_program(prog);
		return Err(format!("Shader link error: {err}"));
	}
	Ok(prog)
}

pub(super) struct PartProgram {
	pub(super) program: glow::Program,
	pub(super) u_mvp: Option<glow::UniformLocation>,
	pub(super) u_offset: Option<glow::UniformLocation>,
	pub(super) u_albedo: Option<glow::UniformLocation>,
	pub(super) u_backdrop: Option<glow::UniformLocation>,
	pub(super) u_opacity: Option<glow::UniformLocation>,
	pub(super) u_mult_color: Option<glow::UniformLocation>,
	pub(super) u_screen_color: Option<glow::UniformLocation>,
	pub(super) u_blend_mode: Option<glow::UniformLocation>,
}

pub(super) struct PartMaskProgram {
	pub(super) program: glow::Program,
	pub(super) u_mvp: Option<glow::UniformLocation>,
	pub(super) u_offset: Option<glow::UniformLocation>,
	pub(super) u_tex: Option<glow::UniformLocation>,
	pub(super) u_threshold: Option<glow::UniformLocation>,
}

pub(super) struct CompProgram {
	pub(super) program: glow::Program,
	pub(super) u_fbo: Option<glow::UniformLocation>,
	pub(super) u_backdrop: Option<glow::UniformLocation>,
	pub(super) u_opacity: Option<glow::UniformLocation>,
	pub(super) u_mult_color: Option<glow::UniformLocation>,
	pub(super) u_screen_color: Option<glow::UniformLocation>,
	pub(super) u_blend_mode: Option<glow::UniformLocation>,
}

pub(super) struct CompMaskProgram {
	pub(super) program: glow::Program,
	pub(super) u_fbo: Option<glow::UniformLocation>,
	pub(super) u_threshold: Option<glow::UniformLocation>,
	pub(super) u_opacity: Option<glow::UniformLocation>,
}

pub(super) struct BlitProgram {
	pub(super) program: glow::Program,
	pub(super) u_fbo: Option<glow::UniformLocation>,
}

pub(super) struct StagePrograms {
	pub(super) part: PartProgram,
	pub(super) part_mask: PartMaskProgram,
	pub(super) comp: CompProgram,
	pub(super) comp_mask: CompMaskProgram,
	pub(super) blit: BlitProgram,
}

impl StagePrograms {
	pub(super) unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
		let prefix = shader_prefix(gl);
		let part_frag = format!("{BLEND_GLSL_HELPERS}\n{PART_FRAG_BODY}");
		let comp_frag = format!("{BLEND_GLSL_HELPERS}\n{COMP_FRAG_BODY}");

		let part_p = compile_program(gl, prefix, PART_VERT_BODY, &part_frag)?;
		let part_mask_p = compile_program(gl, prefix, PART_VERT_BODY, PART_MASK_FRAG_BODY)?;
		let comp_p = compile_program(gl, prefix, QUAD_VERT_BODY, &comp_frag)?;
		let comp_mask_p = compile_program(gl, prefix, QUAD_VERT_BODY, COMP_MASK_FRAG_BODY)?;
		let blit_p = compile_program(gl, prefix, QUAD_VERT_BODY, BLIT_FRAG_BODY)?;

		Ok(Self {
			part: PartProgram {
				program: part_p,
				u_mvp: gl.get_uniform_location(part_p, "mvp"),
				u_offset: gl.get_uniform_location(part_p, "offset"),
				u_albedo: gl.get_uniform_location(part_p, "albedo"),
				u_backdrop: gl.get_uniform_location(part_p, "backdrop"),
				u_opacity: gl.get_uniform_location(part_p, "opacity"),
				u_mult_color: gl.get_uniform_location(part_p, "multColor"),
				u_screen_color: gl.get_uniform_location(part_p, "screenColor"),
				u_blend_mode: gl.get_uniform_location(part_p, "blendMode"),
			},
			part_mask: PartMaskProgram {
				program: part_mask_p,
				u_mvp: gl.get_uniform_location(part_mask_p, "mvp"),
				u_offset: gl.get_uniform_location(part_mask_p, "offset"),
				u_tex: gl.get_uniform_location(part_mask_p, "tex"),
				u_threshold: gl.get_uniform_location(part_mask_p, "threshold"),
			},
			comp: CompProgram {
				program: comp_p,
				u_fbo: gl.get_uniform_location(comp_p, "fbo"),
				u_backdrop: gl.get_uniform_location(comp_p, "backdrop"),
				u_opacity: gl.get_uniform_location(comp_p, "opacity"),
				u_mult_color: gl.get_uniform_location(comp_p, "multColor"),
				u_screen_color: gl.get_uniform_location(comp_p, "screenColor"),
				u_blend_mode: gl.get_uniform_location(comp_p, "blendMode"),
			},
			comp_mask: CompMaskProgram {
				program: comp_mask_p,
				u_fbo: gl.get_uniform_location(comp_mask_p, "fbo"),
				u_threshold: gl.get_uniform_location(comp_mask_p, "threshold"),
				u_opacity: gl.get_uniform_location(comp_mask_p, "opacity"),
			},
			blit: BlitProgram {
				program: blit_p,
				u_fbo: gl.get_uniform_location(blit_p, "fbo"),
			},
		})
	}

	pub(super) unsafe fn destroy(&self, gl: &glow::Context) {
		gl.delete_program(self.part.program);
		gl.delete_program(self.part_mask.program);
		gl.delete_program(self.comp.program);
		gl.delete_program(self.comp_mask.program);
		gl.delete_program(self.blit.program);
	}
}

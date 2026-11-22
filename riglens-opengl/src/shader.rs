use glow::HasContext;

#[derive(thiserror::Error, Debug)]
#[error("Could not compile shader: {0}")]
pub struct ShaderCompileError(String);

/// Compiles a shader program composed of a vertex and fragment shader.
pub(crate) fn compile(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, ShaderCompileError> {
	unsafe {
		let program = gl.create_program().map_err(ShaderCompileError)?;

		// Use GLSL ES 3.00 on WASM for WebGL
		#[cfg(target_arch = "wasm32")]
		let (vertex, fragment) = (
			&format!(
				"#version 300 es\nprecision highp float;\n{}",
				vertex.replace("#version 330", "")
			),
			&format!(
				"#version 300 es\nprecision highp float;\n{}",
				fragment.replace("#version 330", "")
			),
		);

		let vs = gl.create_shader(glow::VERTEX_SHADER).map_err(|e| {
			gl.delete_program(program);
			ShaderCompileError(e)
		})?;
		gl.shader_source(vs, vertex);
		gl.compile_shader(vs);
		if let Err(e) = verify_shader(gl, vs) {
			gl.delete_shader(vs);
			gl.delete_program(program);
			return Err(e);
		}
		gl.attach_shader(program, vs);

		let fs = gl.create_shader(glow::FRAGMENT_SHADER).map_err(|e| {
			gl.detach_shader(program, vs);
			gl.delete_shader(vs);
			gl.delete_program(program);
			ShaderCompileError(e)
		})?;
		gl.shader_source(fs, fragment);
		gl.compile_shader(fs);
		if let Err(e) = verify_shader(gl, fs) {
			gl.delete_shader(fs);
			gl.detach_shader(program, vs);
			gl.delete_shader(vs);
			gl.delete_program(program);
			return Err(e);
		}
		gl.attach_shader(program, fs);

		gl.link_program(program);
		gl.detach_shader(program, vs);
		gl.detach_shader(program, fs);
		gl.delete_shader(vs);
		gl.delete_shader(fs);

		if let Err(e) = verify_program(gl, program) {
			gl.delete_program(program);
			return Err(e);
		}

		Ok(program)
	}
}

unsafe fn verify_shader(gl: &glow::Context, shader: glow::Shader) -> Result<(), ShaderCompileError> {
	if gl.get_shader_compile_status(shader) {
		Ok(())
	} else {
		Err(ShaderCompileError(gl.get_shader_info_log(shader)))
	}
}

unsafe fn verify_program(gl: &glow::Context, program: glow::Program) -> Result<(), ShaderCompileError> {
	if gl.get_program_link_status(program) {
		Ok(())
	} else {
		Err(ShaderCompileError(gl.get_program_info_log(program)))
	}
}
